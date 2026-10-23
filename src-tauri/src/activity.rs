//! The activity half's wiring: opens `Stats` at `<app data>/activity.db`, ingests at launch, then
//! again when a harness's logs change (file watcher, debounced 2 s) and on a 60 s sweep. After an
//! ingest that added events, the windows get `tr://stats-changed`.

use crate::state::AppState;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokenrunway_core::stats::Stats;

const DEBOUNCE: Duration = Duration::from_secs(2);
const SWEEP: Duration = Duration::from_secs(60);
const MAX_SETTLE: Duration = Duration::from_secs(10);

pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::Builder::new().name("activity".into()).spawn(move || run(app)).ok();
}

pub fn stats(app: &AppHandle) -> Option<Arc<Stats>> {
    app.state::<AppState>().stats.get().cloned()
}

fn ingest(app: &AppHandle, stats: &Stats) {
    let t = Instant::now();
    let r = stats.ingest();
    if r.events_added > 0 || !r.errors.is_empty() {
        crate::debug(&format!(
            "ingest: {} files, {} events added, {} errors, {:?}",
            r.files_read,
            r.events_added,
            r.errors.len(),
            t.elapsed()
        ));
    }
    if r.events_added > 0 {
        let _ = app.emit("tr://stats-changed", ());
        // Today's spend may be in the tray label.
        crate::state::broadcast(app);
    }
}

fn run(app: AppHandle) {
    let dir = app.state::<AppState>().data_dir.clone();
    let stats = match Stats::open(&dir.join("activity.db")) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            crate::debug(&format!("activity.db: {e}"));
            return;
        }
    };
    let _ = app.state::<AppState>().stats.set(stats.clone());
    ingest(&app, &stats);

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |e| {
        let _ = tx.send(e);
    })
    .ok();
    let mut watched: HashSet<PathBuf> = HashSet::new();
    let mut watch_new = |w: &mut Option<notify::RecommendedWatcher>| {
        use notify::Watcher;
        let Some(w) = w.as_mut() else { return };
        for p in stats.watch_paths() {
            if !watched.contains(&p) && p.exists() && w.watch(&p, notify::RecursiveMode::Recursive).is_ok() {
                watched.insert(p);
            }
        }
    };
    watch_new(&mut watcher);
    let mut last_sweep = Instant::now();
    loop {
        let wait = SWEEP.saturating_sub(last_sweep.elapsed());
        match rx.recv_timeout(wait) {
            Ok(Ok(ev)) if is_write(&ev) => {
                app.state::<AppState>().last_activity.store(tokenrunway_core::now_ms(), Ordering::Relaxed);
                // Wait for the writes to settle (but not forever while an agent streams).
                let t = Instant::now();
                while t.elapsed() < MAX_SETTLE && rx.recv_timeout(DEBOUNCE).is_ok() {}
                ingest(&app, &stats);
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                ingest(&app, &stats);
                watch_new(&mut watcher);
                last_sweep = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(SWEEP);
                ingest(&app, &stats);
            }
        }
    }
}

fn is_write(ev: &notify::Event) -> bool {
    use notify::EventKind;
    matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_))
}
