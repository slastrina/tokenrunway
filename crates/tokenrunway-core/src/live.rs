//! What's happening right now, from the agents' logs: spend so far this month (for the budget
//! goals) and sessions burning much faster than usual (the burn-rate warning).
//!
//! "Usual" is the median of every 20-minute slice of every session over the last 14 days that
//! spent anything, so idle gaps don't drag it down. A session is hot when its last 20 minutes
//! spent at least `MIN_COST` and at least `RATIO` times that median, and it wrote in the last
//! 5 minutes (it's still going).

use crate::stats::{harness_name, local_date, midnight, Stats};
use crate::store::Store;
use crate::view::HotSession;
use chrono::Datelike;
use rusqlite::params;

const MIN: i64 = 60_000;
/// The slice a session's pace is measured over.
pub const SLICE_MS: i64 = 20 * MIN;
const HISTORY_MS: i64 = 14 * 24 * 60 * MIN;
/// Below this, a fast session isn't worth interrupting for.
pub const MIN_COST: f64 = 5.0;
pub const RATIO: f64 = 3.0;
/// Without enough history to compare against, only this much in 20 minutes counts as hot.
const COLD_START_COST: f64 = 15.0;
const MIN_SLICES: usize = 10;

/// The first instant of this local calendar month.
pub fn month_start(now: i64) -> i64 {
    let d = local_date(now);
    midnight(d.with_day(1).unwrap_or(d))
}

/// API-equivalent spend in [from, ∞).
pub fn spend_since(store: &Store, from: i64) -> f64 {
    store.conn().query_row("SELECT IFNULL(SUM(cost_usd), 0) FROM events WHERE ts >= ?1", params![from], |r| r.get(0)).unwrap_or(0.0)
}

/// API-equivalent spend in [from, ∞) by these harnesses only, and only models whose id contains
/// `model` when given ("opus" for a Weekly Opus window).
pub fn spend_since_by(store: &Store, from: i64, harnesses: &[&str], model: Option<&str>) -> f64 {
    let sql = "SELECT IFNULL(SUM(cost_usd), 0) FROM events WHERE ts >= ?1 AND harness = ?2 AND instr(lower(model), ?3) > 0";
    let Ok(mut q) = store.conn().prepare_cached(sql) else { return 0.0 };
    let m = model.unwrap_or("").to_ascii_lowercase();
    harnesses.iter().map(|h| q.query_row(params![from, h, m], |r| r.get::<_, f64>(0)).unwrap_or(0.0)).sum()
}

/// The median cost of a 20-minute session slice over the 14 days before `before`; None
/// without enough history.
pub fn usual_slice_cost(store: &Store, before: i64) -> Option<f64> {
    let sql = "SELECT SUM(IFNULL(cost_usd, 0)) AS c FROM events WHERE ts >= ?1 AND ts < ?2
               GROUP BY session_id, ts / ?3 HAVING c > 0 ORDER BY c";
    let mut q = store.conn().prepare_cached(sql).ok()?;
    let costs: Vec<f64> = q.query_map(params![before - HISTORY_MS, before, SLICE_MS], |r| r.get(0)).ok()?.flatten().collect();
    if costs.len() < MIN_SLICES {
        return None;
    }
    Some(costs[costs.len() / 2])
}

/// Sessions spending much faster than usual right now, most expensive first.
pub fn hot_sessions(store: &Store, now: i64) -> Vec<HotSession> {
    let from = now - SLICE_MS;
    let usual = usual_slice_cost(store, from);
    let sql = "SELECT harness, session_id, project, SUM(IFNULL(cost_usd, 0)), MAX(ts),
               (SELECT MIN(ts) FROM events s WHERE s.session_id = e.session_id AND s.harness = e.harness)
               FROM events e WHERE ts >= ?1 AND ts <= ?2 GROUP BY harness, session_id";
    let Ok(mut q) = store.conn().prepare_cached(sql) else { return Vec::new() };
    let rows = q.query_map(params![from, now], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, f64>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
        ))
    });
    let mut out: Vec<HotSession> = rows
        .map(|r| r.flatten().collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, _, _, cost, last, _)| now - last <= 5 * MIN && *cost >= MIN_COST)
        .filter_map(|(harness, session_id, project, cost, _, started)| {
            let ratio = match usual {
                Some(u) if u > 0.0 => cost / u,
                _ if cost >= COLD_START_COST => 0.0,
                _ => return None,
            };
            if usual.is_some() && ratio < RATIO {
                return None;
            }
            Some(HotSession {
                harness: harness_name(&harness).to_string(),
                session_id,
                project,
                cost,
                minutes: (SLICE_MS / MIN) as u32,
                ratio: (ratio * 10.0).round() / 10.0,
                started,
            })
        })
        .collect();
    out.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    out
}

/// LM Studio since `from`: requests, tokens, and its last request ever (ms).
pub fn local_usage(store: &Store, from: i64) -> (u32, u64, Option<i64>) {
    let (n, tokens) = store
        .conn()
        .query_row(
            "SELECT COUNT(*), IFNULL(SUM(input + output + cache_read + cache_write_5m + cache_write_1h), 0) FROM events WHERE harness = 'lmstudio' AND ts >= ?1",
            params![from],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .unwrap_or((0, 0));
    let last =
        store.conn().query_row("SELECT MAX(ts) FROM events WHERE harness = 'lmstudio'", [], |r| r.get::<_, Option<i64>>(0)).ok().flatten();
    (n as u32, tokens as u64, last)
}

impl Stats {
    /// API-equivalent work since `from` by these harnesses (an allowance window's cycle).
    pub fn spend_since(&self, from: i64, harnesses: &[&str], model: Option<&str>) -> f64 {
        spend_since_by(&self.store(), from, harnesses, model)
    }

    /// LM Studio today: requests, tokens, and its last request ever.
    pub fn local_today(&self, now: i64) -> (u32, u64, Option<i64>) {
        local_usage(&self.store(), midnight(local_date(now)))
    }

    /// API-equivalent spend since the first of this local month (the monthly budget goal).
    pub fn spend_month(&self, now: i64) -> f64 {
        spend_since(&self.store(), month_start(now))
    }

    pub fn hot_sessions(&self, now: i64) -> Vec<HotSession> {
        hot_sessions(&self.store(), now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(s: &Store, key: &str, session: &str, ts: i64, cost: f64) {
        s.conn()
            .execute(
                "INSERT INTO events (key, ts, harness, model, session_id, project, cost_usd) VALUES (?1, ?2, 'claude-code', 'claude-opus-5-5', ?3, 'third-eye', ?4)",
                params![key, ts, session, cost],
            )
            .unwrap();
    }

    const NOW: i64 = 1_792_807_612_967;

    #[test]
    fn hot_only_against_a_usual_pace() {
        let s = Store::open_in_memory().unwrap();
        // Two weeks of ordinary sessions: $1.50 per 20 minutes.
        for i in 0..40 {
            event(&s, &format!("h{i}"), &format!("old{}", i / 4), NOW - 2 * 24 * 60 * MIN - i * SLICE_MS, 1.5);
        }
        assert_eq!(usual_slice_cost(&s, NOW - SLICE_MS), Some(1.5));
        // Now: one session at $6 in 20 minutes (4×), one at $2.
        for i in 0..6 {
            event(&s, &format!("a{i}"), "hot", NOW - i * 3 * MIN, 1.0);
            event(&s, &format!("b{i}"), "calm", NOW - i * 3 * MIN, 0.33);
        }
        let hot = hot_sessions(&s, NOW);
        assert_eq!(hot.len(), 1, "{hot:?}");
        assert_eq!((hot[0].session_id.as_str(), hot[0].harness.as_str(), hot[0].ratio), ("hot", "Claude Code", 4.0));
        assert!((hot[0].cost - 6.0).abs() < 1e-9);
        assert_eq!(hot[0].started, NOW - 15 * MIN);
        // Ten minutes after its last write it's no longer "still going".
        assert!(hot_sessions(&s, NOW + 10 * MIN).is_empty());
    }

    #[test]
    fn without_history_only_big_spends_count() {
        let s = Store::open_in_memory().unwrap();
        event(&s, "x", "first", NOW - MIN, 8.0);
        assert!(hot_sessions(&s, NOW).is_empty(), "$8 with nothing to compare");
        event(&s, "y", "first", NOW, 9.0);
        assert_eq!(hot_sessions(&s, NOW)[0].ratio, 0.0, "$17: hot, no ratio to show");
    }

    #[test]
    fn cycle_spend_by_agent_and_model() {
        let s = Store::open_in_memory().unwrap();
        event(&s, "old", "a", NOW - 2 * 60 * MIN, 5.0);
        event(&s, "opus", "a", NOW - MIN, 2.0);
        s.conn()
            .execute(
                "INSERT INTO events (key, ts, harness, model, session_id, project, cost_usd) VALUES ('cx', ?1, 'codex', 'gpt-5.6-terra', 'c', 'p', 1.25)",
                params![NOW],
            )
            .unwrap();
        let since = NOW - 60 * MIN;
        assert_eq!(spend_since_by(&s, since, &["claude-code"], None), 2.0);
        assert_eq!(spend_since_by(&s, since, &["claude-code"], Some("opus")), 2.0);
        assert_eq!(spend_since_by(&s, since, &["claude-code"], Some("fable")), 0.0, "a Weekly Fable window");
        assert_eq!(spend_since_by(&s, since, &["codex"], None), 1.25);
        assert_eq!(spend_since_by(&s, since, &[], None), 0.0);
    }

    #[test]
    fn local_usage_today() {
        let s = Store::open_in_memory().unwrap();
        s.conn()
            .execute(
                "INSERT INTO events (key, ts, harness, model, session_id, project, input, output) VALUES ('l1', ?1, 'lmstudio', 'lmstudio/gpt-oss-20b', 'x', 'LM Studio', 100, 25)",
                params![NOW],
            )
            .unwrap();
        event(&s, "paid", "a", NOW, 2.0);
        assert_eq!(local_usage(&s, NOW - MIN), (1, 125, Some(NOW)));
        assert_eq!(local_usage(&s, NOW + MIN), (0, 0, Some(NOW)), "nothing since, but still the last use");
    }

    #[test]
    fn spend_this_month() {
        let s = Store::open_in_memory().unwrap();
        let start = month_start(NOW);
        event(&s, "last-month", "a", start - 1, 100.0);
        event(&s, "this-month", "a", start, 2.5);
        event(&s, "today", "a", NOW, 1.0);
        assert!((spend_since(&s, start) - 3.5).abs() < 1e-9);
        assert!(month_start(NOW) <= NOW && NOW - month_start(NOW) < 32 * 24 * 60 * MIN);
    }
}
