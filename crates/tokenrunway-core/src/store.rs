//! The activity database (SPEC.md §7.2): SQLite in WAL mode, holding
//! - `events`: one row per model call, keyed by the reader's dedupe key, with its tokens and
//!   API-equivalent cost (and the harness's own cost where it reports one)
//! - `turns`: each turn's category signals (counts only), category and confidence
//! - `cursors`: how far each source file has been read
//! - `meta`: the price table version the stored costs were worked out with
//! - `logins`: which CLI login each log source was signed in as, and when (account ids only),
//!   so `events.account` can say which login a call ran under
//!
//! Nothing here can hold prompt text: events have counts, ids, paths and names; turns have
//! keyword counts.

use crate::categorise::{self, Signals};
use crate::ingest::{Batch, Cursor, Event, Tokens};
use crate::pricing;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS events (
    key TEXT PRIMARY KEY,
    ts INTEGER NOT NULL,
    harness TEXT NOT NULL,
    model TEXT NOT NULL,
    session_id TEXT NOT NULL,
    turn_id TEXT,
    project TEXT NOT NULL,
    branch TEXT,
    sidechain INTEGER NOT NULL DEFAULT 0,
    fast INTEGER NOT NULL DEFAULT 0,
    input INTEGER NOT NULL DEFAULT 0,
    output INTEGER NOT NULL DEFAULT 0,
    cache_read INTEGER NOT NULL DEFAULT 0,
    cache_write_5m INTEGER NOT NULL DEFAULT 0,
    cache_write_1h INTEGER NOT NULL DEFAULT 0,
    reasoning INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL,
    cost_actual REAL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS events_ts ON events(ts);
CREATE INDEX IF NOT EXISTS events_unpriced ON events(key) WHERE cost_usd IS NULL;
CREATE TABLE IF NOT EXISTS turns (
    id TEXT PRIMARY KEY,
    signals TEXT NOT NULL,
    category TEXT NOT NULL,
    confidence REAL NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS cursors (
    source TEXT NOT NULL,
    path TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    offset INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (source, path)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS logins (
    scope TEXT NOT NULL,
    login TEXT NOT NULL,
    from_ts INTEGER NOT NULL,
    seen_ts INTEGER NOT NULL,
    PRIMARY KEY (scope, from_ts)
) WITHOUT ROWID;
";

/// An event written just after the login was last checked still counts as that login's, unless
/// a different login has been seen since.
const LOGIN_GRACE_MS: i64 = 60_000;

pub struct Store {
    conn: Connection,
}

/// One group of events, as the stats views add them up: a 15-minute bucket × harness × model ×
/// project × category. Fifteen minutes so every time zone's midnight falls on a bucket edge.
#[derive(Debug, Clone, Default)]
pub struct Agg {
    pub bucket: i64,
    pub harness: String,
    pub model: String,
    pub project: String,
    pub category: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    pub cost_actual: f64,
    /// How many of the events had a reported cost
    pub actual_n: u64,
    /// The login the calls ran under, where it's known
    pub account: Option<String>,
}

pub const BUCKET_MS: i64 = 15 * 60 * 1000;

impl Agg {
    pub fn tokens(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// One session's events in one project and branch (`Store::branch_parts`).
#[derive(Debug, Clone, Default)]
pub struct BranchPart {
    pub project: String,
    pub branch: String,
    pub harness: String,
    pub session_id: String,
    pub first: i64,
    pub last: i64,
    pub calls: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
}

/// One model call, for the cache analysis (`Store::project_calls`).
#[derive(Debug, Clone, Default)]
pub struct Call {
    pub session_id: String,
    pub ts: i64,
    pub model: String,
    pub input: u64,
    pub cache_read: u64,
    /// All cache writes; `cache_write_1h` is the part written with the one-hour lifetime
    pub cache_write: u64,
    pub cache_write_1h: u64,
}

/// One session × model × project × category slice, for the session list and averages.
#[derive(Debug, Clone, Default)]
pub struct SessionPart {
    pub harness: String,
    pub session_id: String,
    pub first: i64,
    pub last: i64,
    pub model: String,
    pub project: String,
    pub category: String,
    pub tokens: u64,
    pub cost: f64,
    /// The whole session's working time in [from, to): the gaps between its events, leaving out
    /// any longer than `IDLE_MS` (a session resumed days later isn't days long). The same on
    /// every part of a session.
    pub active_ms: i64,
    /// The login the session ran under, where it's known
    pub account: Option<String>,
}

/// A gap between two events longer than this is a break, not work.
pub const IDLE_MS: i64 = 30 * 60 * 1000;

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let conn = Connection::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        Self::init(conn)
    }

    /// For queries that live next to the feature using them (live.rs).
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn open_in_memory() -> Result<Self, String> {
        Self::init(Connection::open_in_memory().map_err(|e| e.to_string())?)
    }

    fn init(conn: Connection) -> Result<Self, String> {
        conn.busy_timeout(std::time::Duration::from_secs(5)).ok();
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        conn.execute_batch(SCHEMA).map_err(|e| format!("schema: {e}"))?;
        // Added in 0.2.4: the login a call ran under (NULL where it can't be told).
        let has_account = conn.prepare("SELECT account FROM events LIMIT 0").is_ok();
        if !has_account {
            conn.execute_batch("ALTER TABLE events ADD COLUMN account TEXT").map_err(|e| format!("schema: {e}"))?;
        }
        let s = Self { conn };
        s.reprice_if_needed()?;
        Ok(s)
    }

    /// Works the costs out again when the price table has changed since they were stored.
    fn reprice_if_needed(&self) -> Result<(), String> {
        let v = pricing::version();
        let have: Option<String> =
            self.conn.query_row("SELECT v FROM meta WHERE k = 'pricing'", [], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
        if have.as_deref() == Some(v.as_str()) {
            return Ok(());
        }
        self.conn.execute("UPDATE events SET cost_usd = NULL", []).map_err(|e| e.to_string())?;
        self.price_missing()?;
        self.conn.execute("INSERT OR REPLACE INTO meta (k, v) VALUES ('pricing', ?1)", [v]).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Prices every event whose cost is unset (new, grown, or after a price table change).
    fn price_missing(&self) -> Result<(), String> {
        let tx = self.conn.unchecked_transaction().map_err(|e| e.to_string())?;
        {
            let mut q = tx
                .prepare(
                    "SELECT key, model, fast, input, output, cache_read, cache_write_5m, cache_write_1h FROM events WHERE cost_usd IS NULL",
                )
                .map_err(|e| e.to_string())?;
            let rows: Vec<(String, f64)> = q
                .query_map([], |r| {
                    let t = Tokens {
                        input: r.get::<_, i64>(3)? as u64,
                        output: r.get::<_, i64>(4)? as u64,
                        cache_read: r.get::<_, i64>(5)? as u64,
                        cache_write_5m: r.get::<_, i64>(6)? as u64,
                        cache_write_1h: r.get::<_, i64>(7)? as u64,
                        reasoning: 0,
                    };
                    let model: String = r.get(1)?;
                    Ok((r.get(0)?, pricing::cost(&model, &t, r.get::<_, i64>(2)? != 0)))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?;
            let mut up = tx.prepare("UPDATE events SET cost_usd = ?2 WHERE key = ?1").map_err(|e| e.to_string())?;
            for (k, c) in rows {
                up.execute(params![k, c]).map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// Every stored cursor, keyed by (source, path).
    pub fn cursors(&self) -> HashMap<(String, String), Cursor> {
        let mut out = HashMap::new();
        let Ok(mut q) = self.conn.prepare("SELECT source, path, size, mtime, offset, state FROM cursors") else { return out };
        let rows = q.query_map([], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                Cursor { size: r.get::<_, i64>(2)? as u64, mtime: r.get(3)?, offset: r.get::<_, i64>(4)? as u64, state: r.get(5)? },
            ))
        });
        if let Ok(rows) = rows {
            out.extend(rows.flatten());
        }
        out
    }

    /// Notes that log source `scope` (a login file's path) is signed in as `login` now: extends
    /// the current stretch, or starts a new one when the login changed. Signed out (None) leaves
    /// it, so calls made meanwhile aren't put down to anyone.
    pub fn observe_login(&self, scope: &str, login: Option<&str>, now: i64) -> Result<(), String> {
        let Some(login) = login else { return Ok(()) };
        let e = |e: rusqlite::Error| e.to_string();
        let last: Option<(String, i64)> = self
            .conn
            .query_row("SELECT login, from_ts FROM logins WHERE scope = ?1 ORDER BY from_ts DESC LIMIT 1", [scope], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(e)?;
        match last {
            Some((l, from)) if l == login => self
                .conn
                .execute("UPDATE logins SET seen_ts = MAX(seen_ts, ?3) WHERE scope = ?1 AND from_ts = ?2", params![scope, from, now])
                .map_err(e)?,
            _ => self
                .conn
                .execute(
                    "INSERT OR REPLACE INTO logins (scope, login, from_ts, seen_ts) VALUES (?1, ?2, ?3, ?3)",
                    params![scope, login, now],
                )
                .map_err(e)?,
        };
        Ok(())
    }

    /// Puts the calls of earlier Codex sessions down to the account their `session_meta` names
    /// (`codex::session_meta`), where nothing says yet.
    pub fn set_session_account(&self, harness: &str, session_id: &str, account: &str) -> Result<usize, String> {
        self.conn
            .execute(
                "UPDATE events SET account = ?3 WHERE harness = ?1 AND session_id = ?2 AND account IS NULL",
                params![harness, session_id, account],
            )
            .map_err(|e| e.to_string())
    }

    pub fn meta(&self, k: &str) -> Option<String> {
        self.conn.query_row("SELECT v FROM meta WHERE k = ?1", [k], |r| r.get(0)).optional().ok().flatten()
    }

    pub fn set_meta(&self, k: &str, v: &str) -> Result<(), String> {
        self.conn.execute("INSERT OR REPLACE INTO meta (k, v) VALUES (?1, ?2)", [k, v]).map(|_| ()).map_err(|e| e.to_string())
    }

    /// The earliest call put down to any of `logins` on `harnesses`.
    pub fn first_for_logins(&self, harnesses: &[String], logins: &[String]) -> Option<i64> {
        if harnesses.is_empty() || logins.is_empty() {
            return None;
        }
        let list = |n: usize, at: usize| (0..n).map(|i| format!("?{}", at + i)).collect::<Vec<_>>().join(", ");
        let sql = format!(
            "SELECT MIN(ts) FROM events WHERE harness IN ({}) AND account IN ({})",
            list(harnesses.len(), 1),
            list(logins.len(), harnesses.len() + 1)
        );
        let args: Vec<&dyn rusqlite::ToSql> = harnesses.iter().chain(logins).map(|s| s as &dyn rusqlite::ToSql).collect();
        self.conn.query_row(&sql, args.as_slice(), |r| r.get::<_, Option<i64>>(0)).ok().flatten()
    }

    /// Writes one source's batch and its new cursor in one transaction, so a crash can't leave
    /// events counted without the cursor moving (or the other way round). `project` names each
    /// event's working directory; `scope` is the source's login file (`observe_login`), for the
    /// calls whose log doesn't name the login. Returns how many events were new.
    pub fn apply(
        &mut self,
        source: &str,
        path: &str,
        scope: Option<&str>,
        b: &Batch,
        cursor: &Cursor,
        project: &mut dyn FnMut(&str) -> String,
    ) -> Result<u32, String> {
        let e = |e: rusqlite::Error| e.to_string();
        let tx = self.conn.transaction().map_err(e)?;
        let mut added = 0;
        {
            // The login whose stretch the call falls in; just past the last check only if no
            // other login has been seen since.
            let mut ins = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO events (key, ts, harness, model, session_id, turn_id, project, branch, sidechain, fast,
                     input, output, cache_read, cache_write_5m, cache_write_1h, reasoning, cost_usd, cost_actual, account)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                     COALESCE(?19, (SELECT l.login FROM logins l WHERE l.scope = ?20 AND l.from_ts <= ?2
                        AND (?2 <= l.seen_ts OR (?2 <= l.seen_ts + ?21
                             AND NOT EXISTS (SELECT 1 FROM logins n WHERE n.scope = l.scope AND n.from_ts > l.from_ts)))
                        ORDER BY l.from_ts DESC LIMIT 1)))",
                )
                .map_err(e)?;
            // A later line for the same message (Claude Code streams) or an updated opencode
            // message: keep the larger counts and price it again.
            let mut grow = tx
                .prepare_cached(
                    "UPDATE events SET input = MAX(input, ?2), output = MAX(output, ?3), cache_read = MAX(cache_read, ?4),
                     cache_write_5m = MAX(cache_write_5m, ?5), cache_write_1h = MAX(cache_write_1h, ?6), reasoning = MAX(reasoning, ?7),
                     cost_actual = COALESCE(?8, cost_actual), cost_usd = NULL
                     WHERE key = ?1 AND (input < ?2 OR output < ?3 OR cache_read < ?4 OR cache_write_5m < ?5 OR cache_write_1h < ?6
                     OR reasoning < ?7 OR (?8 IS NOT NULL AND (cost_actual IS NULL OR cost_actual <> ?8)))",
                )
                .map_err(e)?;
            for ev in &b.events {
                let t = &ev.tokens;
                let n = ins
                    .execute(params![
                        ev.key,
                        ev.ts,
                        harness_id(ev),
                        ev.model,
                        ev.session_id,
                        ev.turn_id,
                        project(&ev.cwd),
                        ev.branch,
                        ev.sidechain,
                        ev.fast,
                        t.input as i64,
                        t.output as i64,
                        t.cache_read as i64,
                        t.cache_write_5m as i64,
                        t.cache_write_1h as i64,
                        t.reasoning as i64,
                        pricing::cost(&ev.model, t, ev.fast),
                        ev.cost_actual,
                        ev.account,
                        scope,
                        LOGIN_GRACE_MS,
                    ])
                    .map_err(e)?;
                if n > 0 {
                    added += 1;
                } else {
                    grow.execute(params![
                        ev.key,
                        t.input as i64,
                        t.output as i64,
                        t.cache_read as i64,
                        t.cache_write_5m as i64,
                        t.cache_write_1h as i64,
                        t.reasoning as i64,
                        ev.cost_actual
                    ])
                    .map_err(e)?;
                }
            }
            let mut get = tx.prepare_cached("SELECT signals FROM turns WHERE id = ?1").map_err(e)?;
            let mut put =
                tx.prepare_cached("INSERT OR REPLACE INTO turns (id, signals, category, confidence) VALUES (?1, ?2, ?3, ?4)").map_err(e)?;
            for (id, sig) in &b.turns {
                let mut s = sig.clone();
                if !b.replace_turns {
                    let old: Option<String> = get.query_row([id], |r| r.get(0)).optional().map_err(e)?;
                    if let Some(old) = old.and_then(|o| serde_json::from_str::<Signals>(&o).ok()) {
                        let mut merged = old;
                        merged.merge(&s);
                        s = merged;
                    }
                }
                let (cat, conf) = categorise::classify(&s);
                put.execute(params![id, serde_json::to_string(&s).unwrap_or_default(), categorise::id(cat), conf]).map_err(e)?;
            }
            tx.execute(
                "INSERT OR REPLACE INTO cursors (source, path, size, mtime, offset, state) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![source, path, cursor.size as i64, cursor.mtime, cursor.offset as i64, cursor.state],
            )
            .map_err(e)?;
        }
        tx.commit().map_err(e)?;
        self.price_missing()?;
        Ok(added)
    }

    /// Models in the store the price table doesn't know.
    pub fn unpriced_models(&self) -> Vec<String> {
        let mut out = BTreeSet::new();
        if let Ok(mut q) = self.conn.prepare("SELECT DISTINCT model FROM events") {
            if let Ok(rows) = q.query_map([], |r| r.get::<_, String>(0)) {
                out.extend(rows.flatten().filter(|m| !pricing::is_local(m) && pricing::lookup(m).is_none()));
            }
        }
        out.into_iter().collect()
    }

    /// Events in [from, to), added up per 15-minute bucket × harness × model × project × category.
    pub fn agg(&self, from: i64, to: i64) -> Vec<Agg> {
        let sql = "SELECT e.ts / ?3, e.harness, e.model, e.project, IFNULL(t.category, 'chat'),
                   SUM(e.input), SUM(e.output), SUM(e.cache_read), SUM(e.cache_write_5m + e.cache_write_1h),
                   SUM(IFNULL(e.cost_usd, 0)), SUM(IFNULL(e.cost_actual, 0)), COUNT(e.cost_actual), e.account
                   FROM events e LEFT JOIN turns t ON t.id = e.turn_id
                   WHERE e.ts >= ?1 AND e.ts < ?2
                   GROUP BY 1, 2, 3, 4, 5, 13";
        let Ok(mut q) = self.conn.prepare_cached(sql) else { return Vec::new() };
        let rows = q.query_map(params![from, to, BUCKET_MS], |r| {
            Ok(Agg {
                bucket: r.get(0)?,
                harness: r.get(1)?,
                model: r.get(2)?,
                project: r.get(3)?,
                category: r.get(4)?,
                input: r.get::<_, i64>(5)? as u64,
                output: r.get::<_, i64>(6)? as u64,
                cache_read: r.get::<_, i64>(7)? as u64,
                cache_write: r.get::<_, i64>(8)? as u64,
                cost: r.get(9)?,
                cost_actual: r.get(10)?,
                actual_n: r.get::<_, i64>(11)? as u64,
                account: r.get(12)?,
            })
        });
        rows.map(|r| r.flatten().collect()).unwrap_or_default()
    }

    /// Sessions with events in [from, to), split by model × project × category.
    pub fn session_parts(&self, from: i64, to: i64) -> Vec<SessionPart> {
        let sql = "WITH gaps AS (
                       SELECT harness, session_id, ts - LAG(ts) OVER (PARTITION BY harness, session_id ORDER BY ts) AS gap
                       FROM events WHERE ts >= ?1 AND ts < ?2),
                   active AS (
                       SELECT harness, session_id, SUM(CASE WHEN gap <= ?3 THEN gap ELSE 0 END) AS ms FROM gaps GROUP BY 1, 2)
                   SELECT e.harness, e.session_id, MIN(e.ts), MAX(e.ts), e.model, e.project, IFNULL(t.category, 'chat'),
                   SUM(e.input + e.output + e.cache_read + e.cache_write_5m + e.cache_write_1h), SUM(IFNULL(e.cost_usd, 0)), IFNULL(MAX(a.ms), 0),
                   e.account
                   FROM events e LEFT JOIN turns t ON t.id = e.turn_id
                   LEFT JOIN active a ON a.harness = e.harness AND a.session_id = e.session_id
                   WHERE e.ts >= ?1 AND e.ts < ?2
                   GROUP BY 1, 2, 5, 6, 7, 11";
        let Ok(mut q) = self.conn.prepare_cached(sql) else { return Vec::new() };
        let rows = q.query_map(params![from, to, IDLE_MS], |r| {
            Ok(SessionPart {
                harness: r.get(0)?,
                session_id: r.get(1)?,
                first: r.get(2)?,
                last: r.get(3)?,
                model: r.get(4)?,
                project: r.get(5)?,
                category: r.get(6)?,
                tokens: r.get::<_, i64>(7)? as u64,
                cost: r.get(8)?,
                active_ms: r.get(9)?,
                account: r.get(10)?,
            })
        });
        rows.map(|r| r.flatten().collect()).unwrap_or_default()
    }

    /// Events in [from, to) added up per project × branch × harness × session (insights.rs: cost
    /// per project and branch, cache hit rates). `branch` is "" where the harness didn't say, or
    /// said "HEAD" (a detached checkout, or not a git repo).
    pub fn branch_parts(&self, from: i64, to: i64) -> Vec<BranchPart> {
        let sql = "SELECT e.project, CASE WHEN e.branch IS NULL OR e.branch = 'HEAD' THEN '' ELSE e.branch END, e.harness, e.session_id, MIN(e.ts), MAX(e.ts), COUNT(*),
                   SUM(e.input), SUM(e.output), SUM(e.cache_read), SUM(e.cache_write_5m + e.cache_write_1h), SUM(IFNULL(e.cost_usd, 0))
                   FROM events e WHERE e.ts >= ?1 AND e.ts < ?2
                   GROUP BY 1, 2, 3, 4";
        let Ok(mut q) = self.conn.prepare_cached(sql) else { return Vec::new() };
        let rows = q.query_map(params![from, to], |r| {
            Ok(BranchPart {
                project: r.get(0)?,
                branch: r.get(1)?,
                harness: r.get(2)?,
                session_id: r.get(3)?,
                first: r.get(4)?,
                last: r.get(5)?,
                calls: r.get::<_, i64>(6)? as u64,
                input: r.get::<_, i64>(7)? as u64,
                output: r.get::<_, i64>(8)? as u64,
                cache_read: r.get::<_, i64>(9)? as u64,
                cache_write: r.get::<_, i64>(10)? as u64,
                cost: r.get(11)?,
            })
        });
        rows.map(|r| r.flatten().collect()).unwrap_or_default()
    }

    /// One project's main-thread model calls in [from, to), per session in time order
    /// (insights.rs looks for what breaks the prompt cache: idle gaps and model switches).
    pub fn project_calls(&self, project: &str, from: i64, to: i64) -> Vec<Call> {
        let sql = "SELECT e.session_id, e.ts, e.model, e.input, e.cache_read, e.cache_write_5m + e.cache_write_1h, e.cache_write_1h
                   FROM events e WHERE e.project = ?1 AND e.ts >= ?2 AND e.ts < ?3 AND e.sidechain = 0
                   ORDER BY e.session_id, e.ts";
        let Ok(mut q) = self.conn.prepare_cached(sql) else { return Vec::new() };
        let rows = q.query_map(params![project, from, to], |r| {
            Ok(Call {
                session_id: r.get(0)?,
                ts: r.get(1)?,
                model: r.get(2)?,
                input: r.get::<_, i64>(3)? as u64,
                cache_read: r.get::<_, i64>(4)? as u64,
                cache_write: r.get::<_, i64>(5)? as u64,
                cache_write_1h: r.get::<_, i64>(6)? as u64,
            })
        });
        rows.map(|r| r.flatten().collect()).unwrap_or_default()
    }

    /// The earliest event's time, if there are any.
    pub fn first_ts(&self) -> Option<i64> {
        self.conn.query_row("SELECT MIN(ts) FROM events", [], |r| r.get::<_, Option<i64>>(0)).ok().flatten()
    }
}

fn harness_id(ev: &Event) -> String {
    serde_json::to_value(ev.harness).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "other".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Harness;

    fn ev(key: &str, output: u64) -> Event {
        Event {
            key: key.into(),
            ts: 1_000,
            harness: Harness::ClaudeCode,
            model: "claude-opus-5-5".into(),
            session_id: "s1".into(),
            turn_id: Some("t1".into()),
            cwd: "/x/proj".into(),
            branch: None,
            sidechain: false,
            fast: false,
            tokens: Tokens { input: 1_000_000, output, ..Default::default() },
            cost_actual: None,
            account: None,
        }
    }

    #[test]
    fn dedupes_and_keeps_the_larger_count() {
        let mut s = Store::open_in_memory().unwrap();
        let mut name = |c: &str| c.rsplit('/').next().unwrap_or("").to_string();
        let mut b = Batch::default();
        b.events.push(ev("k1", 10));
        b.events.push(ev("k1", 1_000_000)); // the same message, later and longer
        b.turn("t1").prompt("fix the bug");
        let added = s.apply("cc", "/a.jsonl", None, &b, &Cursor::default(), &mut name).unwrap();
        assert_eq!(added, 1);
        // again: nothing new
        let added = s.apply("cc", "/a.jsonl", None, &b, &Cursor::default(), &mut name).unwrap();
        assert_eq!(added, 0);
        let a = s.agg(0, 10_000);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].output, 1_000_000);
        assert_eq!(a[0].project, "proj");
        assert_eq!(a[0].category, "bugfix");
        assert!((a[0].cost - 24.0).abs() < 1e-9, "{}", a[0].cost); // $4 in + $20 out
    }

    #[test]
    fn turn_signals_add_up_across_passes() {
        let mut s = Store::open_in_memory().unwrap();
        let mut name = |_: &str| "p".to_string();
        let mut b = Batch::default();
        b.events.push(ev("k1", 10));
        b.turn("t1").tool("Bash", None, Some("docker build ."));
        s.apply("cc", "/a", None, &b, &Cursor::default(), &mut name).unwrap();
        let mut b = Batch::default();
        b.turn("t1").tool("Bash", None, Some("kubectl apply -f x.yaml"));
        s.apply("cc", "/a", None, &b, &Cursor::default(), &mut name).unwrap();
        let sig: String = s.conn.query_row("SELECT signals FROM turns WHERE id = 't1'", [], |r| r.get(0)).unwrap();
        let sig: Signals = serde_json::from_str(&sig).unwrap();
        assert_eq!(sig.cmd_ops, 2);
        assert_eq!(s.agg(0, 10_000)[0].category, "ops");
    }

    #[test]
    fn calls_go_to_the_login_signed_in_at_the_time() {
        let mut s = Store::open_in_memory().unwrap();
        let mut name = |_: &str| "p".to_string();
        let at = |key: &str, ts: i64| Event { ts, ..ev(key, 10) };
        let scope = Some("/home/.claude.json");
        // Before Tokenrunway first saw the login: can't be told.
        let mut b = Batch::default();
        b.events.push(at("before", 500));
        s.observe_login("/home/.claude.json", Some("work"), 1_000).unwrap();
        s.observe_login("/home/.claude.json", Some("work"), 5_000).unwrap();
        b.events.push(at("work-1", 2_000));
        b.events.push(at("work-2", 5_000 + 30_000)); // just after the last check
        s.apply("cc", "/a", scope, &b, &Cursor::default(), &mut name).unwrap();
        // Signed in as "home" at 200 s; the gap since the last "work" check can't be told.
        s.observe_login("/home/.claude.json", Some("home"), 200_000).unwrap();
        s.observe_login("/home/.claude.json", None, 250_000).unwrap(); // signed out: no change
        let mut b = Batch::default();
        b.events.push(at("gap", 120_000));
        b.events.push(at("home-1", 210_000));
        // A log that names its login wins (Codex `session_meta`).
        b.events.push(Event { account: Some("named".into()), ..at("named", 210_000) });
        // Another source's login says nothing about this one.
        s.apply("cc", "/a", scope, &b, &Cursor::default(), &mut name).unwrap();
        s.apply(
            "cc",
            "/b",
            Some("/other/.claude.json"),
            &Batch { events: vec![at("elsewhere", 2_000)], ..Default::default() },
            &Cursor::default(),
            &mut name,
        )
        .unwrap();
        let account =
            |k: &str| -> Option<String> { s.conn.query_row("SELECT account FROM events WHERE key = ?1", [k], |r| r.get(0)).unwrap() };
        assert_eq!(account("before"), None);
        assert_eq!(account("work-1").as_deref(), Some("work"));
        assert_eq!(account("work-2").as_deref(), Some("work"));
        assert_eq!(account("gap"), None);
        assert_eq!(account("home-1").as_deref(), Some("home"));
        assert_eq!(account("named").as_deref(), Some("named"));
        assert_eq!(account("elsewhere"), None);
        assert_eq!(s.first_for_logins(&["claude-code".into()], &["home".into()]), Some(210_000));
        // Older Codex sessions get their account from session_meta, without touching known ones.
        assert_eq!(s.set_session_account("claude-code", "s1", "late").unwrap(), 3);
        assert_eq!(account("before").as_deref(), Some("late"));
        assert_eq!(account("work-1").as_deref(), Some("work"));
        assert!(s.agg(0, i64::MAX).iter().any(|a| a.account.as_deref() == Some("home")));
    }

    #[test]
    fn adds_the_account_column_to_an_older_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE events (key TEXT PRIMARY KEY, ts INTEGER NOT NULL, harness TEXT NOT NULL, model TEXT NOT NULL,
             session_id TEXT NOT NULL, turn_id TEXT, project TEXT NOT NULL, branch TEXT, sidechain INTEGER NOT NULL DEFAULT 0,
             fast INTEGER NOT NULL DEFAULT 0, input INTEGER NOT NULL DEFAULT 0, output INTEGER NOT NULL DEFAULT 0,
             cache_read INTEGER NOT NULL DEFAULT 0, cache_write_5m INTEGER NOT NULL DEFAULT 0, cache_write_1h INTEGER NOT NULL DEFAULT 0,
             reasoning INTEGER NOT NULL DEFAULT 0, cost_usd REAL, cost_actual REAL) WITHOUT ROWID;
             INSERT INTO events (key, ts, harness, model, session_id, project) VALUES ('old', 1, 'codex', 'gpt-5', 's', 'p');",
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        assert_eq!(s.agg(0, 10)[0].account, None);
        assert!(Store::open_in_memory().is_ok(), "and a new one is created with it");
    }

    #[test]
    fn cursors_round_trip() {
        let mut s = Store::open_in_memory().unwrap();
        let c = Cursor { size: 10, mtime: 20, offset: 5, state: "{}".into() };
        s.apply("cc", "/a", None, &Batch::default(), &c, &mut |_| String::new()).unwrap();
        assert_eq!(s.cursors().get(&("cc".into(), "/a".into())), Some(&c));
    }
}
