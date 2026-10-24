//! Tokenrunway's engine, with no Tauri in it, so it builds and tests on every platform and could
//! back a headless CLI later. See SPEC.md §7 for where each piece is going.
//!
//! Two halves meet in `view` (the JSON the UI gets):
//! - activity (local logs): `ingest`, `pricing`, `categorise`, `store`, behind the `stats::Stats` facade
//! - allowances (providers): `allowance`, `forecast`

pub mod allowance;
pub mod categorise;
pub mod cli;
pub mod forecast;
pub mod ingest;
pub mod insights;
pub mod live;
pub mod model;
pub mod pricing;
pub mod rattleboard;
pub mod stats;
pub mod store;
pub mod update;
pub mod view;

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
