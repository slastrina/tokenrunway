//! Insights from the activity logs (SPEC.md §10.5): cost per project and git branch, cache tips
//! for projects whose prompt cache works far worse than usual, the weekly recap, and what each
//! subscription did this month at API prices.
//!
//! Only totals, project and branch names and timings are used, as elsewhere; never prompt text.

use crate::allowance::usd;
use crate::pricing;
use crate::stats::{harness_name, local_date, midnight, sessions, Span, Stats};
use crate::store::{Agg, Call};
use crate::view::{BranchRow, CacheTip, InsightsView, PlanValue, ProjectRow, Ranked, Recap, SessionRow};
use chrono::{Datelike, Duration, NaiveDate};
use std::collections::{BTreeMap, HashMap, HashSet};

/// A project needs this much in the period before it gets a cache tip.
pub const TIP_MIN_COST: f64 = 5.0;
pub const TIP_MIN_TOKENS: u64 = 1_000_000;
/// The prompt cache's default lifetime (Anthropic: 5 minutes; OpenAI's is similar), and the
/// long one Claude Code can ask for.
const IDLE_MS: i64 = 5 * 60_000;
const IDLE_1H_MS: i64 = 60 * 60_000;
/// "Short" sessions end before the cache they wrote is read back much.
const SHORT_CALLS: u64 = 3;

/// A subscription account whose work the local logs show (the app passes these from its accounts).
#[derive(Debug, Clone, Default)]
pub struct Subscription {
    pub account_id: String,
    /// "anthropic" | "openai" | "gemini"
    pub provider: String,
    pub name: String,
    /// "Max 20x", "Plus", "Enterprise"…
    pub plan: String,
}

/// The harnesses that run on a provider's subscription login. opencode and others can use a
/// subscription or an API key, and the logs don't say which, so they aren't counted.
pub fn subscription_harnesses(provider: &str) -> &'static [&'static str] {
    match provider {
        "anthropic" => &["claude-code"],
        "openai" => &["codex"],
        "gemini" => &["gemini-cli"],
        _ => &[],
    }
}

/// List price per month (USD) for the plans with one price per person. Team, Enterprise and
/// Business vary by contract: those are set in Settings.
pub fn plan_price(provider: &str, plan: &str) -> Option<f64> {
    let p = plan.trim().to_ascii_lowercase();
    match (provider, p.as_str()) {
        ("anthropic", "pro") => Some(20.0),
        ("anthropic", "max 5x") | ("anthropic", "max") => Some(100.0),
        ("anthropic", "max 20x") => Some(200.0),
        ("openai", "plus") => Some(20.0),
        ("openai", "pro") => Some(200.0),
        ("gemini", "ai pro") | ("gemini", "google ai pro") => Some(19.99),
        ("gemini", "ai ultra") | ("gemini", "google ai ultra") => Some(249.99),
        (_, "free") => Some(0.0),
        _ => None,
    }
}

fn provider_word(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "Claude",
        "openai" => "ChatGPT",
        "gemini" => "Gemini",
        _ => "AI",
    }
}

/// cache read / (input + cache read + cache write), 0–100.
fn hit_rate(input: u64, cache_read: u64, cache_write: u64) -> Option<f64> {
    let all = input + cache_read + cache_write;
    (all > 0).then(|| cache_read as f64 / all as f64 * 100.0)
}

/// "16×", "2.4×", "0.6×"
fn times(m: f64) -> String {
    if m >= 10.0 {
        format!("{m:.0}×")
    } else {
        format!("{m:.1}×")
    }
}

impl Stats {
    /// Everything the dashboard's insight cards and the popover's extras show. `project` narrows
    /// the projects to that one, with all its branches (the dashboard's drill-down).
    pub fn insights(
        &self,
        period: &str,
        project: Option<&str>,
        subs: &[Subscription],
        prices: &BTreeMap<String, f64>,
        now: i64,
    ) -> InsightsView {
        let (projects, projects_total, usual_cache_hit, tips) = self.projects(period, project, now);
        InsightsView {
            period: period.into(),
            projects,
            projects_total,
            tips,
            usual_cache_hit,
            recap: self.recap(now),
            value: self.plan_value(subs, prices, now),
        }
    }

    /// Projects by cost with their top branches, the usual cache hit rate, and the cache tips.
    pub fn projects(&self, period: &str, only: Option<&str>, now: i64) -> (Vec<ProjectRow>, u32, Option<f64>, Vec<CacheTip>) {
        let p = Span::of(period, now);
        let parts = self.store().branch_parts(p.start, i64::MAX);
        // Cache rates only count harnesses that report caching at all (local models don't).
        let caching: HashSet<&str> = parts.iter().filter(|x| x.cache_read > 0).map(|x| x.harness.as_str()).collect();

        #[derive(Default)]
        struct Branch<'a> {
            cost: f64,
            tokens: u64,
            sessions: HashSet<(&'a str, &'a str)>,
            last: i64,
        }
        #[derive(Default)]
        struct Acc<'a> {
            cost: f64,
            tokens: u64,
            sessions: HashSet<(&'a str, &'a str)>,
            input: u64,
            cache_read: u64,
            cache_write: u64,
            last: i64,
            branches: HashMap<&'a str, Branch<'a>>,
        }
        let mut by: HashMap<&str, Acc> = HashMap::new();
        let (mut ui, mut ur, mut uw) = (0u64, 0u64, 0u64);
        for x in &parts {
            let tokens = x.input + x.output + x.cache_read + x.cache_write;
            let a = by.entry(x.project.as_str()).or_default();
            a.cost += x.cost;
            a.tokens += tokens;
            a.sessions.insert((&x.harness, &x.session_id));
            a.last = a.last.max(x.last);
            if caching.contains(x.harness.as_str()) {
                a.input += x.input;
                a.cache_read += x.cache_read;
                a.cache_write += x.cache_write;
                ui += x.input;
                ur += x.cache_read;
                uw += x.cache_write;
            }
            let b = a.branches.entry(x.branch.as_str()).or_default();
            b.cost += x.cost;
            b.tokens += tokens;
            b.sessions.insert((&x.harness, &x.session_id));
            b.last = b.last.max(x.last);
        }
        let usual = hit_rate(ui, ur, uw);

        let mut rows: Vec<ProjectRow> = by
            .iter()
            .map(|(name, a)| {
                let mut branches: Vec<BranchRow> = a
                    .branches
                    .iter()
                    .map(|(name, b)| BranchRow {
                        name: name.to_string(),
                        cost: b.cost,
                        tokens: b.tokens,
                        sessions: b.sessions.len() as u32,
                        last: b.last,
                    })
                    .collect();
                branches.sort_by(|x, y| y.cost.total_cmp(&x.cost).then(y.tokens.cmp(&x.tokens)).then(x.name.cmp(&y.name)));
                let branches_total = branches.len() as u32;
                if only.is_none() {
                    branches.truncate(3);
                }
                ProjectRow {
                    name: name.to_string(),
                    cost: a.cost,
                    tokens: a.tokens,
                    sessions: a.sessions.len() as u32,
                    cache_hit: hit_rate(a.input, a.cache_read, a.cache_write),
                    last: a.last,
                    branches,
                    branches_total,
                }
            })
            .collect();
        rows.sort_by(|x, y| y.cost.total_cmp(&x.cost).then(y.tokens.cmp(&x.tokens)).then(x.name.cmp(&y.name)));
        let total = rows.len() as u32;

        let mut tips: Vec<CacheTip> = Vec::new();
        if let Some(usual) = usual {
            for r in &rows {
                let Some(hit) = r.cache_hit else { continue };
                let big = r.cost >= TIP_MIN_COST || r.tokens >= TIP_MIN_TOKENS;
                if !big || hit >= usual / 2.0 || usual - hit < 20.0 {
                    continue;
                }
                let calls = self.store().project_calls(&r.name, p.start, i64::MAX);
                let (cause, detail) = cause(&calls);
                tips.push(CacheTip {
                    project: r.name.clone(),
                    hit,
                    usual,
                    cost: r.cost,
                    title: format!("Sessions in {} get {:.0}% cache hits vs your usual {:.0}%", r.name, hit, usual),
                    detail,
                    cause: cause.into(),
                });
                if tips.len() == 3 {
                    break;
                }
            }
        }

        if let Some(name) = only {
            rows.retain(|r| r.name == name);
        } else {
            rows.truncate(12);
        }
        (rows, total, usual, tips)
    }

    /// The last full week, Monday to Sunday in local time, and the change from the week before.
    /// None when nothing happened that week.
    pub fn recap(&self, now: i64) -> Option<Recap> {
        let today = local_date(now);
        let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
        let (from, to) = (monday - Duration::days(7), monday);
        let prev = from - Duration::days(7);
        let (rows, prev_rows, parts) = {
            let s = self.store();
            (s.agg(midnight(from), midnight(to)), s.agg(midnight(prev), midnight(from)), s.session_parts(midnight(from), midnight(to)))
        };
        recap_from(
            from,
            &rows,
            &prev_rows,
            &sessions(&parts)
                .into_iter()
                .map(|s| SessionRow {
                    started: s.first,
                    harness: harness_name(&s.harness).to_string(),
                    project: s.project,
                    model: s.model,
                    kind: s.kind,
                    duration_ms: s.active,
                    tokens: s.tokens,
                    cost: s.cost,
                    harness_id: s.harness,
                    session_id: s.session_id,
                })
                .collect::<Vec<_>>(),
        )
    }

    /// API-equivalent work this calendar month per subscription provider, against its price.
    /// Several accounts of one provider are counted together: the logs don't say which account a
    /// session used.
    pub fn plan_value(&self, subs: &[Subscription], prices: &BTreeMap<String, f64>, now: i64) -> Vec<PlanValue> {
        let today = local_date(now);
        let first = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap_or(today);
        let rows = self.store().agg(midnight(first), i64::MAX);
        let mut by_harness: HashMap<&str, f64> = HashMap::new();
        for r in &rows {
            *by_harness.entry(r.harness.as_str()).or_default() += r.cost;
        }
        let mut providers: Vec<&str> = subs.iter().map(|s| s.provider.as_str()).collect();
        providers.sort();
        providers.dedup();
        let mut out = Vec::new();
        for provider in providers {
            let harnesses = subscription_harnesses(provider);
            let spend: f64 = harnesses.iter().map(|h| by_harness.get(h).copied().unwrap_or(0.0)).sum();
            if spend <= 0.0 {
                continue;
            }
            let mine: Vec<&Subscription> = subs.iter().filter(|s| s.provider == provider).collect();
            let each: Vec<Option<f64>> =
                mine.iter().map(|s| prices.get(&s.account_id).copied().or_else(|| plan_price(provider, &s.plan))).collect();
            let price = each.iter().copied().sum::<Option<f64>>();
            let plan = match mine.as_slice() {
                [one] if !one.plan.is_empty() => one.plan.clone(),
                [_] => provider_word(provider).into(),
                many => format!("{} {} plans", many.len(), provider_word(provider)),
            };
            let multiple = price.filter(|p| *p > 0.0).map(|p| spend / p);
            let work = format!("{} of API-equivalent work this month", usd(spend));
            let line = match (price, multiple, mine.len()) {
                (Some(p), Some(m), 1) => format!("Your {} {plan} plan did {work} ({})", usd(p), times(m)),
                (Some(p), Some(m), _) => format!("Your {plan} ({}/month) did {work} ({})", usd(p), times(m)),
                (Some(_), None, _) => format!("Your free {} plan did {work}", provider_word(provider)),
                (None, _, 1) => format!("Your {plan} plan did {work}. Add its price in Settings to compare."),
                (None, _, _) => format!("Your {plan} did {work}. Add their prices in Settings to compare."),
            };
            out.push(PlanValue {
                provider: provider.into(),
                accounts: mine.iter().map(|s| s.name.clone()).collect(),
                plan,
                price,
                spend,
                multiple,
                harnesses: harnesses.iter().map(|h| harness_name(h).to_string()).collect(),
                line,
            });
        }
        out.sort_by(|a, b| b.spend.total_cmp(&a.spend));
        out
    }
}

fn recap_from(from: NaiveDate, rows: &[Agg], prev_rows: &[Agg], sessions: &[SessionRow]) -> Option<Recap> {
    let spend: f64 = rows.iter().map(|r| r.cost).sum();
    let tokens: u64 = rows.iter().map(Agg::tokens).sum();
    if spend <= 0.0 && tokens == 0 {
        return None;
    }
    let prev_spend: f64 = prev_rows.iter().map(|r| r.cost).sum();
    let change_pct = (prev_spend > 0.0).then(|| (spend - prev_spend) / prev_spend * 100.0);
    let biggest_of = |key: &dyn Fn(&Agg) -> String| -> Option<Ranked> {
        let mut m: HashMap<String, (f64, u64)> = HashMap::new();
        for r in rows {
            let e = m.entry(key(r)).or_default();
            e.0 += r.cost;
            e.1 += r.tokens();
        }
        m.into_iter()
            .map(|(name, (cost, tokens))| Ranked { name, cost, tokens })
            .max_by(|a, b| a.cost.total_cmp(&b.cost).then(a.tokens.cmp(&b.tokens)).then(b.name.cmp(&a.name)))
    };
    let top_project = biggest_of(&|r| r.project.clone());
    let top_model = biggest_of(&|r| pricing::display_name(&r.model));
    let biggest = sessions.iter().max_by(|a, b| a.cost.total_cmp(&b.cost).then(a.tokens.cmp(&b.tokens))).cloned();

    let title = format!("Last week: {} of AI work", usd(spend));
    let mut body = Vec::new();
    body.push(match change_pct {
        Some(c) if c.abs() < 3.0 => "About the same as the week before.".to_string(),
        Some(c) if c > 0.0 => format!("Up {c:.0}% on the week before."),
        Some(c) => format!("Down {:.0}% on the week before.", -c),
        None => "Nothing the week before to compare with.".to_string(),
    });
    if let Some(p) = &top_project {
        body.push(format!("Most went to {} ({}).", p.name, usd(p.cost)));
    }
    if let Some(s) = &biggest {
        body.push(format!("Biggest session: {} in {}, mostly on {}.", usd(s.cost), s.harness, s.project));
    }
    Some(Recap {
        from: from.format("%Y-%m-%d").to_string(),
        to: (from + Duration::days(6)).format("%Y-%m-%d").to_string(),
        spend,
        tokens,
        sessions: sessions.len() as u32,
        prev_spend,
        change_pct,
        top_project,
        top_model,
        biggest,
        title,
        body: body.join(" "),
    })
}

/// What the calls show about why the cache missed: calls that came back to a cold cache after a
/// pause, calls that switched model mid-session (each model has its own cache), or tokens spent
/// in short sessions. Weighed by the tokens that weren't read from the cache.
fn cause(calls: &[Call]) -> (&'static str, String) {
    let (mut idle, mut idle_n, mut switch, mut switch_n) = (0u64, 0u32, 0u64, 0u32);
    let mut total = 0u64;
    let mut per_session: HashMap<&str, (u64, u64)> = HashMap::new();
    let mut long_cache: HashSet<&str> = HashSet::new();
    for (i, c) in calls.iter().enumerate() {
        let prompt = c.input + c.cache_read + c.cache_write;
        let missed = c.input + c.cache_write;
        total += missed;
        let s = per_session.entry(&c.session_id).or_default();
        s.0 += 1;
        s.1 += missed;
        let prev = calls[..i].last().filter(|p| p.session_id == c.session_id);
        if c.cache_write_1h > 0 {
            long_cache.insert(&c.session_id);
        }
        let Some(prev) = prev else { continue };
        // A cold call: most of its prompt wasn't read from the cache, and it was big enough to matter.
        if prompt < 2_000 || c.cache_read * 2 >= prompt {
            continue;
        }
        let ttl = if long_cache.contains(c.session_id.as_str()) { IDLE_1H_MS } else { IDLE_MS };
        if prev.model != c.model {
            switch += missed;
            switch_n += 1;
        } else if c.ts - prev.ts > ttl {
            idle += missed;
            idle_n += 1;
        }
    }
    let short: u64 = per_session.values().filter(|(n, _)| *n <= SHORT_CALLS).map(|(_, m)| m).sum();
    let short_n = per_session.values().filter(|(n, _)| *n <= SHORT_CALLS).count();
    let share = |x: u64| if total > 0 { x as f64 / total as f64 } else { 0.0 };
    let best = [("idle", share(idle)), ("model-switch", share(switch)), ("short-sessions", share(short))]
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|b| b.1 >= 0.25);
    let tok = |n: u64| match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.0}k", n as f64 / 1e3),
        n => n.to_string(),
    };
    match best.map(|b| b.0) {
        Some("idle") => (
            "idle",
            format!(
                "{idle_n} call{} came back after a pause of more than {} and found the cache expired, so {} tokens were sent again at full price. Breaks mid-task cost the most; a fresh session after a long break is cheaper.",
                if idle_n == 1 { "" } else { "s" },
                if long_cache.is_empty() { "5 minutes" } else { "the cache's lifetime" },
                tok(idle)
            ),
        ),
        Some("model-switch") => (
            "model-switch",
            format!(
                "{switch_n} call{} switched model mid-session. Each model keeps its own cache, so the conversation ({} tokens) was sent again in full. Picking the model at the start of a session keeps the cache warm.",
                if switch_n == 1 { "" } else { "s" },
                tok(switch)
            ),
        ),
        Some(_) => (
            "short-sessions",
            format!(
                "Most of its tokens are in {short_n} short session{} ({SHORT_CALLS} calls or fewer), which write the cache but end before reading much of it back. Longer sessions on one task reuse it.",
                if short_n == 1 { "" } else { "s" }
            ),
        ),
        None => ("", "The logs don't show one cause: no long pauses, model switches or short sessions stand out.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(s: &str, ts_min: i64, model: &str, input: u64, read: u64, write: u64) -> Call {
        Call {
            session_id: s.into(),
            ts: ts_min * 60_000,
            model: model.into(),
            input,
            cache_read: read,
            cache_write: write,
            cache_write_1h: 0,
        }
    }

    #[test]
    fn prices_and_lines() {
        assert_eq!(plan_price("anthropic", "Max 20x"), Some(200.0));
        assert_eq!(plan_price("openai", "Plus"), Some(20.0));
        assert_eq!(plan_price("anthropic", "Enterprise"), None);
        assert_eq!(times(15.995), "16×");
        assert_eq!(times(2.44), "2.4×");
        assert_eq!(subscription_harnesses("anthropic"), &["claude-code"]);
    }

    #[test]
    fn causes() {
        // Pauses of 20 minutes between calls on the same model: idle.
        let idle: Vec<Call> = (0..4).map(|i| call("s", i * 20, "opus", 50_000, 1_000, 10_000)).collect();
        assert_eq!(cause(&idle).0, "idle");
        assert!(cause(&idle).1.starts_with("3 calls came back after a pause of more than 5 minutes"), "{}", cause(&idle).1);
        // Model switching every call, a minute apart.
        let sw: Vec<Call> = (0..6).map(|i| call("s", i, if i % 2 == 0 { "opus" } else { "sonnet" }, 50_000, 0, 0)).collect();
        assert_eq!(cause(&sw).0, "model-switch");
        // Many one-call sessions.
        let short: Vec<Call> = (0..5).map(|i| call(&format!("s{i}"), i, "opus", 5_000, 0, 40_000)).collect();
        assert_eq!(cause(&short).0, "short-sessions");
        // Warm all along: nothing stands out.
        let warm: Vec<Call> = (0..8).map(|i| call("s", i, "opus", 1_000, 90_000, 1_000)).collect();
        assert_eq!(cause(&warm).0, "");
    }

    #[test]
    fn recap_wording() {
        let from = NaiveDate::from_ymd_opt(2026, 10, 12).unwrap();
        let row = |project: &str, cost: f64| Agg {
            project: project.into(),
            model: "claude-opus-5-5".into(),
            cost,
            input: 1000,
            ..Default::default()
        };
        let rows = vec![row("third-eye", 151.2), row("tokenrunway", 60.0)];
        let prev = vec![row("third-eye", 180.0)];
        let s = SessionRow { harness: "Claude Code".into(), project: "third-eye".into(), cost: 22.1, ..Default::default() };
        let r = recap_from(from, &rows, &prev, &[s]).unwrap();
        assert_eq!((r.from.as_str(), r.to.as_str()), ("2026-10-12", "2026-10-18"));
        assert_eq!(r.title, "Last week: $211 of AI work");
        assert_eq!(
            r.body,
            "Up 17% on the week before. Most went to third-eye ($151). Biggest session: $22.10 in Claude Code, mostly on third-eye."
        );
        assert_eq!(r.top_model.unwrap().name, "Opus 5.5");
        assert!(recap_from(from, &[], &prev, &[]).is_none());
    }
}
