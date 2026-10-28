//! The newest price table (SPEC.md §10.1): fetched from the repository about 20 s after launch
//! and then once a day while Settings → Fetch the latest prices is on, laid over the bundled
//! table (`tokenrunway_core::pricing`), and kept in `prices.json` in the data folder for offline
//! starts. Failing to fetch is quiet: costs keep the newest table we have (the bundled one at
//! worst), and only the line under the setting says so.

use crate::state::{broadcast, AppState};
use std::path::Path;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokenrunway_core::pricing::{self, Cached};
use tokenrunway_core::view::PriceStatus;
use tokenrunway_core::{now_ms, update};

const FILE: &str = "prices.json";
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

fn github(at: i64) -> PriceStatus {
    PriceStatus { source: "github".into(), updated_at: Some(at), reason: None }
}

fn bundled(reason: Option<String>) -> PriceStatus {
    PriceStatus { source: "bundled".into(), updated_at: None, reason }
}

/// At launch, before activity.db opens: the cached table, if fetching is on and there is one.
pub fn startup(data_dir: &Path, on: bool) -> PriceStatus {
    if !on {
        return bundled(None);
    }
    match Cached::load(&data_dir.join(FILE)) {
        Some(c) if pricing::use_remote(&c.table).is_ok() => github(c.fetched_at),
        _ => bundled(None),
    }
}

pub async fn run(app: AppHandle) {
    tokio::time::sleep(Duration::from_secs(20)).await;
    loop {
        if app.state::<AppState>().settings.lock().unwrap().fetch_prices {
            refresh(&app).await;
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// Fetches the newest table and puts it in use. On a failure the table in use stays, and the
/// status says why only if that's the bundled one.
pub async fn refresh(app: &AppHandle) {
    let result = pricing::fetch(&update::client()).await;
    let st = app.state::<AppState>();
    if !st.settings.lock().unwrap().fetch_prices {
        return; // turned off meanwhile
    }
    match result {
        Ok(raw) => {
            let now = now_ms();
            if let Err(e) = pricing::use_remote(&raw) {
                crate::debug(&format!("prices: {e}"));
                return;
            }
            if let Err(e) = (Cached { fetched_at: now, table: raw }).save(&st.data_dir.join(FILE)) {
                crate::debug(&format!("prices.json: {e}"));
            }
            *st.prices.lock().unwrap() = github(now);
            reprice(app).await;
        }
        Err(e) => {
            crate::debug(&format!("prices: {e}"));
            let mut p = st.prices.lock().unwrap();
            if p.source != "github" {
                *p = bundled(Some(e));
            }
        }
    }
    broadcast(app);
}

/// Settings → Fetch the latest prices changed.
pub fn setting_changed(app: &AppHandle, on: bool) {
    if on {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { refresh(&app).await });
    } else {
        pricing::use_bundled();
        *app.state::<AppState>().prices.lock().unwrap() = bundled(None);
        let app = app.clone();
        tauri::async_runtime::spawn(async move { reprice(&app).await });
    }
}

/// Prices what's stored again with the table now in use, and tells the windows if that changed
/// anything.
async fn reprice(app: &AppHandle) {
    let Some(stats) = crate::activity::stats(app) else { return }; // priced when it opens
    let changed = tauri::async_runtime::spawn_blocking(move || stats.reprice()).await;
    match changed {
        Ok(Ok(true)) => {
            let _ = app.emit("tr://stats-changed", ());
            broadcast(app);
        }
        Ok(Err(e)) => crate::debug(&format!("reprice: {e}")),
        _ => {}
    }
}
