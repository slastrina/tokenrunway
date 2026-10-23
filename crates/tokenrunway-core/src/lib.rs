//! Tokenrunway's engine, with no Tauri in it, so it builds and tests on every platform and could
//! back a headless CLI later. See SPEC.md §7 for where each piece is going.

pub mod forecast;
pub mod model;
pub mod update;

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
