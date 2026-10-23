//! Every reading of every window, kept in `snapshots.db` (SQLite, WAL), for the forecasts and
//! the dashboard's burn-down. Readings older than 30 days are thinned to one an hour.

use crate::model::WindowReading;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

pub struct Snapshots {
    db: Mutex<Connection>,
}

const KEEP_ALL_MS: i64 = 30 * 24 * 3600 * 1000;

impl Snapshots {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let db = Connection::open(path).map_err(|e| e.to_string())?;
        Self::init(db)
    }

    pub fn in_memory() -> Self {
        Self::init(Connection::open_in_memory().expect("sqlite")).expect("schema")
    }

    fn init(db: Connection) -> Result<Self, String> {
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS snapshots(
               account_id TEXT NOT NULL, window TEXT NOT NULL, ts INTEGER NOT NULL,
               used REAL NOT NULL, resets_at INTEGER,
               PRIMARY KEY(account_id, window, ts));",
        )
        .map_err(|e| e.to_string())?;
        Ok(Self { db: Mutex::new(db) })
    }

    /// Keeps the readings (one per window and reading time; a cached reading isn't kept twice).
    pub fn add(&self, account_id: &str, readings: &[WindowReading]) {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        for r in readings {
            let _ = db.execute(
                "INSERT OR IGNORE INTO snapshots(account_id, window, ts, used, resets_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![account_id, r.window, r.at, r.used, r.resets_at],
            );
        }
    }

    /// A window's readings since `since` (ms), oldest first.
    pub fn history(&self, account_id: &str, window: &str, since: i64) -> Vec<WindowReading> {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(mut st) =
            db.prepare_cached("SELECT ts, used, resets_at FROM snapshots WHERE account_id=?1 AND window=?2 AND ts>=?3 ORDER BY ts")
        else {
            return Vec::new();
        };
        st.query_map(params![account_id, window, since], |r| {
            Ok(WindowReading { window: window.to_string(), at: r.get(0)?, used: r.get(1)?, resets_at: r.get(2)?, limit: None })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    /// How many window instances (account, window, reset) reached 100% since `since`, for one
    /// account or all of them.
    pub fn ran_out(&self, since: i64, account_id: Option<&str>) -> u32 {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        db.query_row(
            "SELECT COUNT(*) FROM (SELECT DISTINCT account_id, window, resets_at / 600000 FROM snapshots
               WHERE ts >= ?1 AND used >= 1.0 AND (?2 IS NULL OR account_id = ?2))",
            params![since, account_id],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n as u32)
        .unwrap_or(0)
    }

    /// When the first reading was kept, if any.
    pub fn first_at(&self) -> Option<i64> {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        db.query_row("SELECT MIN(ts) FROM snapshots", [], |r| r.get::<_, Option<i64>>(0)).ok().flatten()
    }

    pub fn remove_account(&self, account_id: &str) {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db.execute("DELETE FROM snapshots WHERE account_id=?1", params![account_id]);
    }

    /// Thins readings older than 30 days to the first one per hour per window.
    pub fn thin(&self, now: i64) {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = db.execute(
            "DELETE FROM snapshots WHERE ts < ?1 AND rowid NOT IN (
               SELECT MIN(rowid) FROM snapshots WHERE ts < ?1 GROUP BY account_id, window, ts / 3600000)",
            params![now - KEEP_ALL_MS],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(at: i64, used: f64) -> WindowReading {
        WindowReading { window: "5h".into(), used, limit: None, resets_at: Some(99), at }
    }

    #[test]
    fn keeps_and_reads() {
        let s = Snapshots::in_memory();
        s.add("a", &[r(1, 0.1), r(2, 0.2)]);
        s.add("a", &[r(2, 0.2)]);
        s.add("b", &[r(3, 0.3)]);
        let h = s.history("a", "5h", 0);
        assert_eq!(h.len(), 2);
        assert_eq!(h[1].used, 0.2);
        assert_eq!(h[0].resets_at, Some(99));
        assert_eq!(s.history("a", "5h", 2).len(), 1);
        assert_eq!(s.ran_out(0, None), 0);
        s.add("a", &[r(5, 1.0), r(6, 1.02)]);
        assert_eq!(s.ran_out(0, None), 1, "one window instance");
        assert_eq!(s.ran_out(0, Some("b")), 0);
        assert_eq!(s.first_at(), Some(1));
        s.remove_account("a");
        assert!(s.history("a", "5h", 0).is_empty());
        let hour = 3_600_000;
        s.add("c", &[r(0, 0.1), r(60_000, 0.2), r(2 * hour, 0.3)]);
        s.thin(KEEP_ALL_MS + 3 * hour);
        assert_eq!(s.history("c", "5h", 0).len(), 2, "one per hour once old");
    }
}
