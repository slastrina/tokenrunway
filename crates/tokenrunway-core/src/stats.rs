//! The activity half's facade: the app opens one `Stats`, calls `ingest` when logs change (and on
//! a timer), and asks it for the Stats, Harness and Dashboard views. Thread-safe (`&self`).
//!
//! STUB: signatures are the contract with src-tauri; the bodies are being filled in.

use crate::view::{DashboardView, HarnessView, StatsView};
use std::path::{Path, PathBuf};

pub struct Stats {
    _db: PathBuf,
}

/// What one ingest pass did.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestReport {
    pub files_read: u32,
    pub events_added: u32,
    pub errors: Vec<String>,
}

impl Stats {
    /// Opens (or creates) the database at `db` (e.g. <app data>/activity.db).
    pub fn open(db: &Path) -> Result<Self, String> {
        Ok(Self { _db: db.to_path_buf() })
    }

    /// Reads whatever is new in every harness's logs (incremental; cheap when nothing changed).
    pub fn ingest(&self) -> IngestReport {
        IngestReport::default()
    }

    /// Directories to watch for changes (each harness's log root that exists).
    pub fn watch_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// `period`: "today" | "7d" | "30d" | "90d" | "lifetime"; `now` in ms, local time zone.
    pub fn stats(&self, period: &str, _now: i64) -> StatsView {
        StatsView { period: period.into(), ..Default::default() }
    }

    pub fn harness(&self, period: &str, _now: i64) -> HarnessView {
        HarnessView { period: period.into(), ..Default::default() }
    }

    /// `group`: "model" | "harness" | "category". Leaves `resets` and `burndown` empty (the app
    /// fills them from allowance data).
    pub fn dashboard(&self, period: &str, group: &str, _now: i64) -> DashboardView {
        DashboardView { period: period.into(), group: group.into(), ..Default::default() }
    }
}
