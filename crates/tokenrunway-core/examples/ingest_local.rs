//! Reads this machine's real harness logs into a throwaway database and prints the Stats and
//! Harness views for "7d" and "lifetime", with timings. Only totals and names are printed.
//!
//!     cargo run --release -p tokenrunway-core --example ingest_local
//!
//! TR_DUMP_DIR=<dir> also writes every view as JSON there (get_stats-7d.json, get_harness-30d.json,
//! get_dashboard-7d-model.json…), which the UI's mock mode can show: popover.html?fixture=<dir>.

use std::time::Instant;
use tokenrunway_core::stats::Stats;

fn main() {
    let dir = std::env::temp_dir().join(format!("tokenrunway-ingest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let stats = Stats::open(&dir.join("activity.db")).expect("open");
    println!("watching: {:?}", stats.watch_paths());

    let t = Instant::now();
    let r = stats.ingest();
    println!("first ingest: {:?}, {} files, {} events, errors: {:?}", t.elapsed(), r.files_read, r.events_added, r.errors);
    let t = Instant::now();
    let r = stats.ingest();
    println!("second ingest (nothing changed): {:?}, {} files, {} events", t.elapsed(), r.files_read, r.events_added);

    let now = tokenrunway_core::now_ms();
    let t = Instant::now();
    let today = stats.spend_today(now);
    println!("spend today: ${today:.2} ({:?}; the tray's \"$ today\" label)", t.elapsed());
    let t = Instant::now();
    let (month, hot) = (stats.spend_month(now), stats.hot_sessions(now));
    println!("spend this month: ${month:.2}; fast sessions now: {} ({:?})", hot.len(), t.elapsed());
    for h in &hot {
        println!("  {} in {}: ${:.2} in {} min, {}× usual", h.harness, h.project, h.cost, h.minutes, h.ratio);
    }
    for period in ["7d", "lifetime"] {
        let t = Instant::now();
        let s = stats.stats(period, now);
        let h = stats.harness(period, now);
        let d = stats.dashboard(period, "model", now);
        println!("\n== {period} (views in {:?})", t.elapsed());
        println!("{}", serde_json::to_string_pretty(&s).unwrap());
        println!("{}", serde_json::to_string_pretty(&h).unwrap());
        println!(
            "dashboard: {} days, series {:?}, {} sessions (of {}), heatmap {} days",
            d.days.len(),
            d.series.iter().map(|s| (&s.name, s.values.iter().sum::<f64>().round())).collect::<Vec<_>>(),
            d.sessions.len(),
            d.sessions_total,
            d.heatmap.len()
        );
    }
    if let Some(out) = std::env::var_os("TR_DUMP_DIR").map(std::path::PathBuf::from) {
        std::fs::create_dir_all(&out).expect("dump dir");
        let put = |name: String, v: String| std::fs::write(out.join(format!("{name}.json")), v).expect("write");
        for period in ["today", "7d", "30d", "90d", "lifetime"] {
            put(format!("get_stats-{period}"), serde_json::to_string(&stats.stats(period, now)).unwrap());
            put(format!("get_harness-{period}"), serde_json::to_string(&stats.harness(period, now)).unwrap());
            for group in ["model", "harness", "category"] {
                put(format!("get_dashboard-{period}-{group}"), serde_json::to_string(&stats.dashboard(period, group, now)).unwrap());
            }
        }
        println!("wrote views to {}", out.display());
    }
    if std::env::var_os("TR_KEEP_DB").is_some() {
        println!("kept {}", dir.join("activity.db").display());
    } else {
        std::fs::remove_dir_all(&dir).ok();
    }
}
