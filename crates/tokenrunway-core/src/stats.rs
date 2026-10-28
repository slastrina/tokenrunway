//! The activity half's facade: the app opens one `Stats`, calls `ingest` when logs change (and on
//! a timer), and asks it for the Stats, Harness and Dashboard views. Thread-safe (`&self`).
//!
//! Periods are in local time: "today" is since local midnight, "7d" is today and the six days
//! before, and so on. The store adds events up per 15-minute bucket and this file turns buckets
//! into local days with chrono, so day edges follow the time zone (and its DST) exactly.

use crate::categorise::{self, ALL};
use crate::ingest::{self, claude_code, codex, lmstudio, opencode, Batch, Cursor, ProjectNames, Roots};
use crate::pricing;
use crate::store::{Agg, SessionPart, Store, BUCKET_MS};
use crate::view::{DashboardView, DayCost, HarnessRow, HarnessView, KindShare, Ranked, Series, SessionRow, StatsView};
use chrono::{Duration, Local, NaiveDate, TimeZone};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Stats {
    store: Mutex<Store>,
    roots: Roots,
}

/// Narrows the Stats and Dashboard views to one project and/or one model (the popover's Top
/// models and Top projects rows open the dashboard this way). `model` is the display name
/// ("Opus 5.5"), as those rows show it.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Filter {
    pub project: Option<String>,
    pub model: Option<String>,
    /// Only these harnesses (an account's own agent: "claude-code" for a Claude login); None for all
    pub harnesses: Option<Vec<String>>,
    /// Only calls made under these CLI logins (`AccountRecord::logins`); None for any, including
    /// calls whose login can't be told
    pub logins: Option<Vec<String>>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.project.is_none() && self.model.is_none() && self.harnesses.is_none() && self.logins.is_none()
    }

    fn keeps(&self, harness: &str, account: Option<&str>, project: &str, model: &str) -> bool {
        self.harnesses.as_ref().is_none_or(|hs| hs.iter().any(|h| h == harness))
            && self.logins.as_ref().is_none_or(|ls| account.is_some_and(|a| ls.iter().any(|l| l == a)))
            && self.project.as_deref().is_none_or(|p| p == project)
            && self.model.as_deref().is_none_or(|m| m == pricing::display_name(model))
    }
}

/// What one ingest pass did.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestReport {
    pub files_read: u32,
    pub events_added: u32,
    pub errors: Vec<String>,
}

type Reader = fn(&Path, u64, &str) -> std::io::Result<(Batch, u64, String)>;

impl Stats {
    /// Opens (or creates) the database at `db` (e.g. <app data>/activity.db).
    pub fn open(db: &Path) -> Result<Self, String> {
        Self::open_with(db, Roots::default_for_this_machine())
    }

    /// The same, reading logs from `roots` instead of this machine's default places (tests).
    pub fn open_with(db: &Path, roots: Roots) -> Result<Self, String> {
        Ok(Self { store: Mutex::new(Store::open(db)?), roots })
    }

    /// A throwaway database over this machine's logs (the command line, with no app data).
    pub fn in_memory() -> Result<Self, String> {
        Ok(Self { store: Mutex::new(Store::open_in_memory()?), roots: Roots::default_for_this_machine() })
    }

    pub(crate) fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads whatever is new in every harness's logs (incremental; cheap when nothing changed:
    /// one stat() per file). Files are parsed on a few threads without the database lock, which
    /// is only held to write each file's results.
    pub fn ingest(&self) -> IngestReport {
        let cursors = self.store().cursors();
        let mut jobs: Vec<(&'static str, PathBuf, Reader, Option<String>)> = Vec::new();
        // Which login each source is signed in as, checked before its new lines are written, so
        // they go to the login of the time (`Store::observe_login`).
        let now = crate::now_ms();
        let scope = |harness: &str, file: Option<PathBuf>| -> Option<String> {
            let file = file?;
            let key = file.to_string_lossy().into_owned();
            let _ = self.store().observe_login(&key, ingest::login_in(harness, &file).as_deref(), now);
            Some(key)
        };
        for root in &self.roots.claude {
            let s = scope("claude-code", ingest::claude_login_file(root));
            for p in ingest::files_under(root, &|n| n.ends_with(".jsonl")) {
                jobs.push(("claude-code", p, claude_code::read, s.clone()));
            }
        }
        for root in &self.roots.codex {
            let s = scope("codex", ingest::codex_login_file(root));
            for p in ingest::files_under(root, &|n| n.starts_with("rollout-") && n.ends_with(".jsonl")) {
                jobs.push(("codex", p, codex::read, s.clone()));
            }
        }
        for root in &self.roots.lmstudio {
            for p in ingest::files_under(root, &lmstudio::is_log) {
                jobs.push(("lmstudio", p, lmstudio::read, None));
            }
        }
        self.backfill_codex_accounts(&jobs);
        // Only the files that changed since their cursor.
        let jobs: Vec<_> = jobs
            .into_iter()
            .filter_map(|(src, p, f, scope)| {
                let (size, mtime) = ingest::stat(&p)?;
                let key = (src.to_string(), p.to_string_lossy().into_owned());
                let cur = cursors.get(&key).cloned().unwrap_or_default();
                (cur.size != size || cur.mtime != mtime).then_some((src, p, f, scope, size, mtime, cur))
            })
            .collect();

        let report = Mutex::new(IngestReport::default());
        let names = Mutex::new(ProjectNames::new());
        let next = std::sync::atomic::AtomicUsize::new(0);
        let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 8).min(jobs.len().max(1));
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((src, p, read, scope, size, mtime, cur)) = jobs.get(i) else { break };
                    // A file that shrank was rewritten: start again (the keys make it idempotent).
                    let (from, state) = if *size < cur.offset { (0, "") } else { (cur.offset, cur.state.as_str()) };
                    let res = read(p, from, state);
                    let path = p.to_string_lossy();
                    match res {
                        Ok((batch, offset, state)) => {
                            let c = Cursor { size: *size, mtime: *mtime, offset, state };
                            self.write(src, &path, scope.as_deref(), &batch, &c, &names, &report);
                        }
                        Err(e) => report.lock().unwrap_or_else(|e| e.into_inner()).errors.push(format!("{path}: {e}")),
                    }
                });
            }
        });
        self.ingest_opencode(&cursors, &names, &report);

        let mut r = report.into_inner().unwrap_or_else(|e| e.into_inner());
        if r.events_added > 0 {
            for m in self.store().unpriced_models() {
                r.errors.push(format!("no price for model {m} (counted at $0)"));
            }
        }
        r
    }

    /// Once: puts Codex sessions read before Tokenrunway kept the account down to the account
    /// their `session_meta` names. Later sessions get it as they're read.
    fn backfill_codex_accounts(&self, jobs: &[(&'static str, PathBuf, Reader, Option<String>)]) {
        const KEY: &str = "codex-accounts";
        if self.store().meta(KEY).is_some() {
            return;
        }
        for (_, p, ..) in jobs.iter().filter(|j| j.0 == "codex") {
            if let Some((session, account)) = codex::session_meta(p) {
                let _ = self.store().set_session_account("codex", &session, &account);
            }
        }
        let _ = self.store().set_meta(KEY, "1");
    }

    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        src: &str,
        path: &str,
        scope: Option<&str>,
        b: &Batch,
        c: &Cursor,
        names: &Mutex<ProjectNames>,
        report: &Mutex<IngestReport>,
    ) {
        let res = self.store().apply(src, path, scope, b, c, &mut |cwd| names.lock().unwrap_or_else(|e| e.into_inner()).name(cwd));
        let mut r = report.lock().unwrap_or_else(|e| e.into_inner());
        r.files_read += 1;
        match res {
            Ok(n) => r.events_added += n,
            Err(e) => r.errors.push(format!("{path}: {e}")),
        }
    }

    fn ingest_opencode(&self, cursors: &HashMap<(String, String), Cursor>, names: &Mutex<ProjectNames>, report: &Mutex<IngestReport>) {
        for root in &self.roots.opencode {
            let db = root.join("opencode.db");
            if let Some((size, mtime)) = ingest::stat(&db) {
                // The WAL holds the latest writes until a checkpoint, so it counts too.
                let (ws, wm) = ingest::stat(&root.join("opencode.db-wal")).unwrap_or((0, 0));
                let (size, mtime) = (size + ws, mtime.max(wm));
                let path = db.to_string_lossy().into_owned();
                let cur = cursors.get(&("opencode".to_string(), path.clone())).cloned().unwrap_or_default();
                if cur.size != size || cur.mtime != mtime {
                    match opencode::read_db(&db, cur.offset) {
                        Ok((b, newest)) => {
                            let c = Cursor { size, mtime, offset: newest, state: String::new() };
                            self.write("opencode", &path, None, &b, &c, names, report);
                        }
                        Err(e) => report.lock().unwrap_or_else(|e| e.into_inner()).errors.push(e),
                    }
                }
            }
            // The older layout: one directory per session.
            let msg_root = root.join("storage").join("message");
            let Ok(rd) = std::fs::read_dir(&msg_root) else { continue };
            for dir in rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
                let files = ingest::files_under(&dir, &|n| n.ends_with(".json"));
                let stats: Vec<(u64, i64)> = files.iter().filter_map(|f| ingest::stat(f)).collect();
                let size = stats.iter().map(|s| s.0).sum::<u64>() + stats.len() as u64;
                let mtime = stats.iter().map(|s| s.1).max().unwrap_or(0);
                let path = dir.to_string_lossy().into_owned();
                let cur = cursors.get(&("opencode-storage".to_string(), path.clone())).cloned().unwrap_or_default();
                if cur.size == size && cur.mtime == mtime {
                    continue;
                }
                let b = opencode::read_storage_session(&dir, &root.join("storage").join("part"));
                self.write("opencode-storage", &path, None, &b, &Cursor { size, mtime, ..Default::default() }, names, report);
            }
        }
    }

    /// When the first call made under any of `logins` on `harnesses` was, for "since 29 Oct".
    pub fn first_for_logins(&self, harnesses: &[String], logins: &[String]) -> Option<i64> {
        self.store().first_for_logins(harnesses, logins)
    }

    /// Directories to watch for changes (each harness's log root that exists).
    pub fn watch_paths(&self) -> Vec<PathBuf> {
        self.roots.existing()
    }

    /// API-equivalent spend since local midnight (the tray's "Spend today" label): a cheap sum, so
    /// it can run on every state broadcast.
    pub fn spend_today(&self, now: i64) -> f64 {
        let p = Span::of("today", now);
        self.store().agg(p.start, i64::MAX).iter().map(|r| r.cost).sum()
    }

    /// `period`: "today" | "7d" | "30d" | "90d" | "lifetime"; `now` in ms, local time zone.
    pub fn stats(&self, period: &str, now: i64) -> StatsView {
        self.stats_filtered(period, &Filter::default(), now)
    }

    /// `stats` for one project and/or model only.
    pub fn stats_filtered(&self, period: &str, f: &Filter, now: i64) -> StatsView {
        let p = Span::of(period, now);
        let mut rows = self.store().agg(p.start, i64::MAX);
        rows.retain(|r| f.keeps(&r.harness, r.account.as_deref(), &r.project, &r.model));
        let mut days = Days::new();
        let mut v = StatsView { period: period.into(), ..Default::default() };
        let (mut actual, mut actual_n, mut actual_from) = (0.0, 0u64, Vec::<String>::new());
        let mut models: HashMap<String, (f64, u64)> = HashMap::new();
        let mut local: HashMap<String, (f64, u64)> = HashMap::new();
        let mut projects: HashMap<String, (f64, u64)> = HashMap::new();
        let mut kinds: HashMap<String, u64> = HashMap::new();
        let mut daily: HashMap<NaiveDate, (f64, u64)> = HashMap::new();
        let mut unpriced = std::collections::BTreeSet::new();
        for r in &rows {
            if r.tokens() > 0 && pricing::is_cloud(&r.model) && pricing::lookup(&r.model).is_none() {
                unpriced.insert(pricing::display_name(&r.model));
            }
            v.spend_api += r.cost;
            v.tokens.input += r.input;
            v.tokens.output += r.output;
            v.tokens.cache += r.cache_read + r.cache_write;
            if r.actual_n > 0 {
                actual += r.cost_actual;
                actual_n += r.actual_n;
                let h = harness_name(&r.harness).to_string();
                if !actual_from.contains(&h) {
                    actual_from.push(h);
                }
            }
            // Local models (free) get their own list, ranked by tokens.
            add(if r.harness == "lmstudio" { &mut local } else { &mut models }, pricing::display_name(&r.model), r);
            add(&mut projects, r.project.clone(), r);
            *kinds.entry(r.category.clone()).or_default() += r.tokens();
            let d = daily.entry(days.of(r.bucket)).or_default();
            d.0 += r.cost;
            d.1 += r.tokens();
        }
        v.tokens.total = v.tokens.input + v.tokens.output + v.tokens.cache;
        if actual_n > 0 {
            v.spend_actual = Some(actual);
            actual_from.sort();
            v.actual_note = Some(format!("{} reported", actual_from.join(" + ")));
        }
        v.daily = p.days.iter().map(|d| day_cost(d, daily.get(d).copied().unwrap_or_default())).collect();
        v.models = top(models, 4);
        v.local_models = top(local, 4);
        v.projects = top(projects, 4);
        v.kinds = kind_shares(&kinds);
        v.unpriced = unpriced.into_iter().collect();
        v
    }

    pub fn harness(&self, period: &str, now: i64) -> HarnessView {
        let p = Span::of(period, now);
        let week = Span::of("7d", now);
        let (rows, parts, week_rows) = {
            let s = self.store();
            (s.agg(p.start, i64::MAX), s.session_parts(p.start, i64::MAX), s.agg(week.start, i64::MAX))
        };
        #[derive(Default)]
        struct H {
            cost: f64,
            tokens: u64,
            input: u64,
            cache_read: u64,
            models: HashMap<String, (f64, u64)>,
            kinds: HashMap<String, u64>,
        }
        let mut hs: BTreeMap<String, H> = BTreeMap::new();
        for r in &rows {
            let h = hs.entry(r.harness.clone()).or_default();
            h.cost += r.cost;
            h.tokens += r.tokens();
            h.input += r.input;
            h.cache_read += r.cache_read;
            add(&mut h.models, pricing::display_name(&r.model), r);
            *h.kinds.entry(r.category.clone()).or_default() += r.tokens();
        }
        let sessions = sessions(&parts);
        let mut days = Days::new();
        let mut week_cost: HashMap<(String, NaiveDate), f64> = HashMap::new();
        for r in &week_rows {
            *week_cost.entry((r.harness.clone(), days.of(r.bucket))).or_default() += r.cost;
        }
        let total_cost: f64 = hs.values().map(|h| h.cost).sum();
        let total_tokens: u64 = hs.values().map(|h| h.tokens).sum();
        let mut out: Vec<HarnessRow> = hs
            .into_iter()
            .map(|(id, h)| {
                let mine: Vec<&Session> = sessions.iter().filter(|s| s.harness == id).collect();
                let avg = if mine.is_empty() { 0 } else { mine.iter().map(|s| s.active).sum::<i64>() / mine.len() as i64 };
                // Share of spend; by tokens when nothing has a price (local models).
                let share = if total_cost > 0.0 {
                    h.cost / total_cost * 100.0
                } else if total_tokens > 0 {
                    h.tokens as f64 / total_tokens as f64 * 100.0
                } else {
                    0.0
                };
                HarnessRow {
                    name: harness_name(&id).to_string(),
                    share,
                    cost: h.cost,
                    tokens: h.tokens,
                    sessions: mine.len() as u32,
                    avg_session_ms: avg,
                    cache_hit: if h.cache_read + h.input > 0 { h.cache_read as f64 / (h.cache_read + h.input) as f64 * 100.0 } else { 0.0 },
                    top_model: top(h.models, 1).into_iter().next().map(|r| r.name).unwrap_or_default(),
                    top_kind: top_kind(&h.kinds),
                    daily: week.days.iter().map(|d| week_cost.get(&(id.clone(), *d)).copied().unwrap_or(0.0)).collect(),
                    id,
                }
            })
            .collect();
        out.sort_by(|a, b| b.share.total_cmp(&a.share).then(b.tokens.cmp(&a.tokens)));
        HarnessView { period: period.into(), total_cost, harnesses: out }
    }

    /// `group`: "model" | "harness" | "category". Leaves `resets` and `burndown` empty (the app
    /// fills them from allowance data). Lifetime charts run from the first event's day, at most
    /// a year back.
    pub fn dashboard(&self, period: &str, group: &str, now: i64) -> DashboardView {
        self.dashboard_filtered(period, group, &Filter::default(), now)
    }

    /// `dashboard` for one project and/or model only: the chart, heatmap and sessions.
    pub fn dashboard_filtered(&self, period: &str, group: &str, f: &Filter, now: i64) -> DashboardView {
        let mut p = Span::of(period, now);
        let today = local_date(now);
        if period == "lifetime" {
            let first = self.store().first_ts().map(local_date).unwrap_or(today);
            let first = first.max(today - Duration::days(364));
            p.days = date_range(first, today);
            p.start = i64::MIN;
        }
        let heat_from = today - Duration::days(181);
        let (mut rows, mut heat_rows, mut parts) = {
            let s = self.store();
            (s.agg(p.start, i64::MAX), s.agg(midnight(heat_from), i64::MAX), s.session_parts(p.start, i64::MAX))
        };
        if !f.is_empty() {
            rows.retain(|r| f.keeps(&r.harness, r.account.as_deref(), &r.project, &r.model));
            heat_rows.retain(|r| f.keeps(&r.harness, r.account.as_deref(), &r.project, &r.model));
            parts.retain(|r| f.keeps(&r.harness, r.account.as_deref(), &r.project, &r.model));
        }
        let mut days = Days::new();
        let key = |r: &Agg| match group {
            "harness" => harness_name(&r.harness).to_string(),
            "category" => category_label(&r.category).to_string(),
            _ => pricing::display_name(&r.model),
        };
        let first_day = p.days.first().copied().unwrap_or(today);
        let mut by_key: HashMap<String, f64> = HashMap::new();
        let mut cells: HashMap<(String, NaiveDate), f64> = HashMap::new();
        for r in &rows {
            let d = days.of(r.bucket);
            if d < first_day {
                continue;
            }
            let k = key(r);
            *by_key.entry(k.clone()).or_default() += r.cost;
            *cells.entry((k, d)).or_default() += r.cost;
        }
        let mut keys: Vec<(String, f64)> = by_key.into_iter().collect();
        keys.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let shown: Vec<String> = keys.iter().take(5).map(|k| k.0.clone()).collect();
        let mut series: Vec<Series> = shown
            .iter()
            .map(|k| Series {
                name: k.clone(),
                values: p.days.iter().map(|d| cells.get(&(k.clone(), *d)).copied().unwrap_or(0.0)).collect(),
            })
            .collect();
        if keys.len() > 5 {
            let values =
                p.days.iter().map(|d| keys[5..].iter().map(|k| cells.get(&(k.0.clone(), *d)).copied().unwrap_or(0.0)).sum()).collect();
            series.push(Series { name: "Other".into(), values });
        }

        let mut heat: HashMap<NaiveDate, (f64, u64)> = HashMap::new();
        for r in &heat_rows {
            let e = heat.entry(days.of(r.bucket)).or_default();
            e.0 += r.cost;
            e.1 += r.tokens();
        }
        let heatmap = date_range(heat_from, today).iter().map(|d| day_cost(d, heat.get(d).copied().unwrap_or_default())).collect();

        let mut ss = sessions(&parts);
        ss.sort_by_key(|s| std::cmp::Reverse(s.first));
        let sessions_total = ss.len() as u32;
        let sessions = ss
            .into_iter()
            .take(200)
            .map(|s| SessionRow {
                started: s.first,
                harness: harness_name(&s.harness).to_string(),
                project: s.project,
                model: s.model,
                kind: s.kind,
                duration_ms: s.active,
                tokens: s.tokens,
                cost: s.cost,
            })
            .collect();
        DashboardView {
            period: period.into(),
            group: group.into(),
            days: p.days.iter().map(|d| d.format("%Y-%m-%d").to_string()).collect(),
            series,
            heatmap,
            sessions,
            sessions_total,
            ..Default::default()
        }
    }
}

/// A period: where it starts (ms) and its local days, oldest first.
pub(crate) struct Span {
    pub(crate) start: i64,
    pub(crate) days: Vec<NaiveDate>,
}

impl Span {
    pub(crate) fn of(period: &str, now: i64) -> Self {
        let today = local_date(now);
        let back = match period {
            "today" => 0,
            "30d" => 29,
            "90d" => 89,
            "lifetime" => {
                return Self { start: i64::MIN, days: date_range(today - Duration::days(29), today) };
            }
            _ => 6, // "7d", and anything we don't know
        };
        let first = today - Duration::days(back);
        Self { start: midnight(first), days: date_range(first, today) }
    }
}

pub(crate) fn local_date(ms: i64) -> NaiveDate {
    Local.timestamp_millis_opt(ms).earliest().map(|d| d.date_naive()).unwrap_or_default()
}

/// The first instant of a local day (the earliest one, if DST makes it ambiguous; an hour in if
/// the day starts inside a DST gap).
pub(crate) fn midnight(d: NaiveDate) -> i64 {
    let t = d.and_hms_opt(0, 0, 0).unwrap_or_default();
    Local
        .from_local_datetime(&t)
        .earliest()
        .or_else(|| Local.from_local_datetime(&(t + Duration::hours(1))).earliest())
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

fn date_range(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    from.iter_days().take_while(|d| *d <= to).collect()
}

/// Bucket → local day, remembered (many rows share a bucket).
struct Days(HashMap<i64, NaiveDate>);

impl Days {
    fn new() -> Self {
        Self(HashMap::new())
    }
    fn of(&mut self, bucket: i64) -> NaiveDate {
        *self.0.entry(bucket).or_insert_with(|| local_date(bucket * BUCKET_MS))
    }
}

fn day_cost(d: &NaiveDate, (cost, tokens): (f64, u64)) -> DayCost {
    DayCost { day: d.format("%Y-%m-%d").to_string(), cost, tokens }
}

fn add(m: &mut HashMap<String, (f64, u64)>, k: String, r: &Agg) {
    let e = m.entry(k).or_default();
    e.0 += r.cost;
    e.1 += r.tokens();
}

/// The biggest `n` by cost, then by tokens (so unpriced local models still rank).
fn top(m: HashMap<String, (f64, u64)>, n: usize) -> Vec<Ranked> {
    let mut v: Vec<Ranked> = m.into_iter().map(|(name, (cost, tokens))| Ranked { name, cost, tokens }).collect();
    v.sort_by(|a, b| b.cost.total_cmp(&a.cost).then(b.tokens.cmp(&a.tokens)).then(a.name.cmp(&b.name)));
    v.truncate(n);
    v
}

/// All ten categories, biggest share of tokens first (ties in the usual order).
fn kind_shares(kinds: &HashMap<String, u64>) -> Vec<KindShare> {
    let total: u64 = kinds.values().sum();
    let mut v: Vec<(u64, KindShare)> = ALL
        .iter()
        .map(|c| {
            let t = kinds.get(categorise::id(*c)).copied().unwrap_or(0);
            (t, KindShare { kind: categorise::label(*c).into(), share: if total > 0 { t as f64 / total as f64 * 100.0 } else { 0.0 } })
        })
        .collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.0));
    v.into_iter().map(|x| x.1).collect()
}

fn top_kind(kinds: &HashMap<String, u64>) -> String {
    kinds.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(k, _)| category_label(k).to_string()).unwrap_or_default()
}

fn category_label(id: &str) -> &'static str {
    categorise::from_id(id).map(categorise::label).unwrap_or("Chat")
}

pub fn harness_name(id: &str) -> &str {
    match id {
        "claude-code" => "Claude Code",
        "codex" => "Codex",
        "opencode" => "opencode",
        "gemini-cli" => "Gemini CLI",
        "cline" => "Cline",
        "cursor" => "Cursor",
        "lmstudio" => "LM Studio",
        other => other,
    }
}

/// A session added up from its parts: the model with the most cost, the project and kind with
/// the most tokens.
pub(crate) struct Session {
    pub(crate) harness: String,
    pub(crate) first: i64,
    /// Working time, without breaks over 30 minutes (store::IDLE_MS)
    pub(crate) active: i64,
    pub(crate) model: String,
    pub(crate) project: String,
    pub(crate) kind: String,
    pub(crate) tokens: u64,
    pub(crate) cost: f64,
}

pub(crate) fn sessions(parts: &[SessionPart]) -> Vec<Session> {
    #[derive(Default)]
    struct Acc {
        first: i64,
        active: i64,
        tokens: u64,
        cost: f64,
        models: HashMap<String, (f64, u64)>,
        projects: HashMap<String, u64>,
        kinds: HashMap<String, u64>,
    }
    let mut m: HashMap<(&str, &str), Acc> = HashMap::new();
    for p in parts {
        let a = m.entry((&p.harness, &p.session_id)).or_insert_with(|| Acc { first: i64::MAX, ..Default::default() });
        a.first = a.first.min(p.first);
        a.active = a.active.max(p.active_ms);
        a.tokens += p.tokens;
        a.cost += p.cost;
        let e = a.models.entry(pricing::display_name(&p.model)).or_default();
        e.0 += p.cost;
        e.1 += p.tokens;
        *a.projects.entry(p.project.clone()).or_default() += p.tokens;
        *a.kinds.entry(p.category.clone()).or_default() += p.tokens;
    }
    m.into_iter()
        .map(|((h, _), a)| Session {
            harness: h.to_string(),
            first: a.first,
            active: a.active,
            model: top(a.models, 1).into_iter().next().map(|r| r.name).unwrap_or_default(),
            project: a.projects.into_iter().max_by(|x, y| x.1.cmp(&y.1).then(y.0.cmp(&x.0))).map(|x| x.0).unwrap_or_default(),
            kind: top_kind(&a.kinds),
            tokens: a.tokens,
            cost: a.cost,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_are_local_days() {
        let now = Local.with_ymd_and_hms(2026, 10, 23, 15, 30, 0).unwrap().timestamp_millis();
        let today = Span::of("today", now);
        assert_eq!(today.days.len(), 1);
        assert_eq!(today.start, Local.with_ymd_and_hms(2026, 10, 23, 0, 0, 0).unwrap().timestamp_millis());
        let week = Span::of("7d", now);
        assert_eq!(week.days.len(), 7);
        assert_eq!(week.days[0].format("%Y-%m-%d").to_string(), "2026-10-17");
        assert_eq!(Span::of("30d", now).days.len(), 30);
        assert_eq!(Span::of("90d", now).days.len(), 90);
        let life = Span::of("lifetime", now);
        assert_eq!(life.days.len(), 30);
        assert_eq!(life.start, i64::MIN);
    }

    #[test]
    fn kinds_list_all_ten() {
        let mut k = HashMap::new();
        k.insert("bugfix".to_string(), 30);
        k.insert("build".to_string(), 70);
        let v = kind_shares(&k);
        assert_eq!(v.len(), 10);
        assert_eq!(v[0].kind, "Build");
        assert!((v[0].share - 70.0).abs() < 1e-9);
        assert_eq!(v[1].kind, "Bug fix");
        assert_eq!(v[9].share, 0.0);
    }
}
