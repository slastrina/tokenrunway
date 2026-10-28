//! The app's state: accounts and their latest readings, settings, the snapshot history and the
//! activity stats, plus building the `AppView` the popover gets and pushing it when it changes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager};
use tokenrunway_core::allowance::{self, alerts, snapshots::Snapshots, Accounts, Ctx, Live};
use tokenrunway_core::forecast::{self, Thresholds};
use tokenrunway_core::stats::Stats;
use tokenrunway_core::view::{AppView, Detected, Settings};

/// When to poll an account next, and how it's been going.
#[derive(Debug, Clone, Default)]
pub struct Sched {
    pub next_due: i64,
    pub failures: u32,
    pub last_forced: i64,
    pub in_flight: bool,
    /// The provider asked us to slow down: "Refresh now" waits until then too.
    pub limited_until: i64,
}

pub struct AppState {
    /// The newest release the last check found, and its page.
    pub update: Mutex<Option<tokenrunway_core::update::Release>>,
    pub data_dir: PathBuf,
    pub ctx: Ctx,
    pub accounts: Mutex<Accounts>,
    pub live: Mutex<HashMap<String, Live>>,
    pub sched: Mutex<HashMap<String, Sched>>,
    pub settings: Mutex<Settings>,
    pub snaps: Snapshots,
    pub stats: OnceLock<Arc<Stats>>,
    pub alerts: Mutex<alerts::Memory>,
    pub detected: Mutex<Vec<Detected>>,
    /// Wakes the poller (refresh, a new account)
    pub wake: tokio::sync::Notify,
    /// The last time a harness wrote to its logs (ms)
    pub last_activity: AtomicI64,
    /// Rattleboard: the key (cached), the card as last sent, the status line
    pub board: Mutex<crate::board::Runtime>,
    /// The last AppView pushed, to push only changes
    last_json: Mutex<String>,
    /// LM Studio's server as last probed (poll.rs, every 15 s while it's installed)
    pub local: Mutex<LocalProbe>,
    /// Which price table is in use (prices.rs)
    pub prices: Mutex<tokenrunway_core::view::PriceStatus>,
}

#[derive(Default)]
pub struct LocalProbe {
    pub port: u16,
    /// None while nothing answers
    pub probe: Option<tokenrunway_core::local::Probe>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&data_dir);
        let snaps = Snapshots::open(&data_dir.join("snapshots.db")).unwrap_or_else(|e| {
            crate::debug(&format!("snapshots.db: {e}; keeping readings in memory"));
            Snapshots::in_memory()
        });
        let settings: Settings =
            std::fs::read_to_string(data_dir.join("settings.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let alerts =
            std::fs::read_to_string(data_dir.join("alerts.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let ctx = Ctx::new(data_dir.clone());
        let accounts = Accounts::load(&data_dir);
        let detected = allowance::detect(&ctx.home, &accounts.list);
        // Before activity.db opens, so stored costs are priced with the newest table we have.
        let prices = crate::prices::startup(&data_dir, settings.fetch_prices);
        Self {
            update: Mutex::new(None),
            ctx,
            accounts: Mutex::new(accounts),
            live: Mutex::new(HashMap::new()),
            sched: Mutex::new(HashMap::new()),
            settings: Mutex::new(settings),
            snaps,
            stats: OnceLock::new(),
            alerts: Mutex::new(alerts),
            detected: Mutex::new(detected),
            wake: tokio::sync::Notify::new(),
            last_activity: AtomicI64::new(0),
            board: Mutex::new(Default::default()),
            last_json: Mutex::new(String::new()),
            local: Mutex::new(LocalProbe::default()),
            prices: Mutex::new(prices),
            data_dir,
        }
    }

    pub fn save_settings(&self) {
        let s = self.settings.lock().unwrap().clone();
        if let Ok(json) = serde_json::to_string_pretty(&s) {
            let _ = allowance::accounts::write_atomic(&self.data_dir.join("settings.json"), &json);
        }
    }

    fn save_alerts(&self) {
        let m = self.alerts.lock().unwrap().clone();
        if let Ok(json) = serde_json::to_string(&m) {
            let _ = allowance::accounts::write_atomic(&self.data_dir.join("alerts.json"), &json);
        }
    }

    pub fn redetect(&self) {
        let list = self.accounts.lock().unwrap().list.clone();
        *self.detected.lock().unwrap() = allowance::detect(&self.ctx.home, &list);
    }

    pub fn harness_active(&self, now: i64) -> bool {
        now - self.last_activity.load(Ordering::Relaxed) < 5 * 60_000
    }

    /// Every account's card, most urgent first.
    pub fn account_views(&self, now: i64) -> Vec<tokenrunway_core::view::AccountView> {
        let th = Thresholds::from(&*self.settings.lock().unwrap());
        let list = self.accounts.lock().unwrap().list.clone();
        let live = self.live.lock().unwrap().clone();
        let mut views: Vec<_> = list
            .iter()
            .map(|a| {
                let l = live.get(&a.id).cloned().unwrap_or_default();
                let history = |w: &str| self.snaps.history(&a.id, w, now - forecast::LOOKBACK_MS - 60_000);
                allowance::account_view(a, &l, &history, &th, now)
            })
            .collect();
        allowance::sort_urgent(&mut views);
        views
    }
}

#[cfg(target_os = "macos")]
pub const OS: &str = "mac";
#[cfg(target_os = "windows")]
pub const OS: &str = "windows";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const OS: &str = "linux";

pub fn app_view(app: &AppHandle) -> AppView {
    let st = app.state::<AppState>();
    let now = tokenrunway_core::now_ms();
    let accounts = st.account_views(now);
    let loading = {
        let live = st.live.lock().unwrap();
        accounts.iter().any(|a| !live.get(&a.id).is_some_and(|l| l.tried))
    };
    let detected = st.detected.lock().unwrap().clone();
    let settings = st.settings.lock().unwrap().clone();
    let rattleboard_key = crate::board::has_key(app, &settings.rattleboard);
    let prices = st.prices.lock().unwrap().clone();
    AppView {
        version: env!("CARGO_PKG_VERSION").into(),
        os: OS.into(),
        update: crate::updates::available(app),
        update_how: crate::updates::how().into(),
        loading,
        summary: forecast::summary(&accounts, now),
        tips: tokenrunway_core::allowance::tips::tips(&accounts),
        accounts,
        detected,
        settings,
        rattleboard_status: crate::board::status(app),
        rattleboard_key,
        spend_today: st.stats.get().map(|s| s.spend_today(now)),
        spend_month: st.stats.get().map(|s| s.spend_month(now)),
        hot: st.stats.get().map(|s| s.hot_sessions(now)).unwrap_or_default(),
        local: local_server(&st, now),
        prices,
    }
}

/// The Now tab's LM Studio card: while its server answers, or when it ran today.
fn local_server(st: &AppState, now: i64) -> Option<tokenrunway_core::view::LocalServer> {
    let (today_requests, today_tokens, last_used) = st.stats.get().map(|s| s.local_today(now)).unwrap_or_default();
    let l = st.local.lock().unwrap();
    if l.probe.is_none() && today_requests == 0 {
        return None;
    }
    Some(tokenrunway_core::view::LocalServer {
        name: "LM Studio".into(),
        running: l.probe.is_some(),
        port: l.port,
        models: l.probe.as_ref().map(|p| p.models.clone()).unwrap_or_default(),
        knows_loaded: l.probe.as_ref().is_some_and(|p| p.knows_state),
        today_requests,
        today_tokens,
        last_used,
    })
}

/// Rebuilds the view and, if anything changed, pushes `tr://state` to every window, updates the
/// tray and runs the notification rules (desktop and Rattleboard). The Rattleboard card is
/// checked every time (its runway moves with the clock).
pub fn broadcast(app: &AppHandle) {
    let view = app_view(app);
    crate::board::card_tick(app, &view);
    let json = serde_json::to_string(&view).unwrap_or_default();
    let st = app.state::<AppState>();
    // Every time, changed or not, so the command line can tell the app is running.
    crate::cli::write_status(&st.data_dir, &view);
    {
        let mut last = st.last_json.lock().unwrap();
        if *last == json {
            return;
        }
        *last = json;
    }
    if crate::debugging() {
        let _ =
            std::fs::write(std::env::temp_dir().join("tokenrunway-state.json"), serde_json::to_string_pretty(&view).unwrap_or_default());
    }
    let _ = app.emit("tr://state", &view);
    crate::tray::update(app, &view);
    let now = tokenrunway_core::now_ms();
    let fired = {
        let mut m = st.alerts.lock().unwrap();
        let mut f = m.evaluate(&view.accounts, &view.settings, now);
        f.extend(m.evaluate_live(view.spend_today, view.spend_month, &view.hot, &view.settings, now));
        f
    };
    st.save_alerts();
    crate::notify::show(app, &fired);
    crate::board::on_alerts(app, &fired, &view.accounts);
}
