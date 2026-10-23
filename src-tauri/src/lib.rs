//! Tokenrunway's desktop app: the tray icon, its popover and native menu, the dashboard window,
//! polling, notifications and update checks. Everything that isn't windows and menus lives in
//! tokenrunway-core (see SPEC.md §7).
//!
//! Events the windows get: `tr://state` (the AppView, whenever it changes), `tr://stats-changed`
//! (an ingest added events), `tr://show` (the popover view to show: "now", "settings",
//! "update"), `tr://info` and `tr://update-progress` (updates).

mod activity;
mod commands;
mod notify;
mod poll;
mod state;
mod tray;
mod updates;

pub use state::AppState;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Rect, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

/// The project page: the credit in the popover footer, the tray menu and the tooltip.
pub const SITE: &str = "https://www.iokig.com/tokenrunway";
/// The popover window. The UI draws the panel (rounded, with its own edge) to fill it; on macOS
/// the window itself is the vibrant material with a 12 px radius and the system shadow.
pub const POPOVER: (f64, f64) = (380.0, 560.0);

/// Where the tray icon was last seen, and when the popover last hid (a click on the icon first
/// blurs the open popover; it shouldn't reopen straight away).
struct TrayPlace {
    rect: Option<Rect>,
    hidden_at: Option<Instant>,
}

static PLACE: Mutex<TrayPlace> = Mutex::new(TrayPlace { rect: None, hidden_at: None });

/// Debug log in $TMPDIR/tokenrunway-debug.log (and the last AppView in tokenrunway-state.json): debug builds, or TOKENRUNWAY_DEBUG=1. Never gets
/// tokens, keys or prompt text.
pub fn debugging() -> bool {
    cfg!(debug_assertions) || std::env::var("TOKENRUNWAY_DEBUG").as_deref() == Ok("1")
}

pub fn debug(msg: &str) {
    if !debugging() {
        return;
    }
    use std::io::Write;
    let path = std::env::temp_dir().join("tokenrunway-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let t = chrono::Local::now().format("%H:%M:%S%.3f");
        let _ = writeln!(f, "{t} {msg}");
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    version: &'static str,
    site: &'static str,
    /// The newer version on offer, if the last check found one
    update: Option<String>,
    /// How this copy updates: "install", "homebrew" or "download"
    update_how: &'static str,
}

#[tauri::command]
fn get_info(app: AppHandle) -> Info {
    Info { version: env!("CARGO_PKG_VERSION"), site: SITE, update: updates::available(&app), update_how: updates::how() }
}

#[tauri::command]
fn open_site(app: AppHandle) {
    use tauri_plugin_opener::OpenerExt;
    let _ = app.opener().open_url(SITE, None::<&str>);
}

/// The popover: made hidden at launch, shown under (or above) the tray icon, hidden again when
/// it loses focus. Transparent, so the UI's rounded panel is all you see.
fn create_popover(app: &AppHandle) -> tauri::Result<()> {
    let b = WebviewWindowBuilder::new(app, "popover", WebviewUrl::App("popover.html".into()))
        .title("Tokenrunway")
        .inner_size(POPOVER.0, POPOVER.1)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .accept_first_mouse(true)
        .visible(false);
    #[cfg(target_os = "macos")]
    let b = {
        use tauri::utils::config::WindowEffectsConfig;
        use tauri::window::{Effect, EffectState};
        b.effects(WindowEffectsConfig {
            effects: vec![Effect::Popover],
            state: Some(EffectState::Active),
            radius: Some(12.0),
            color: None,
            ..Default::default()
        })
    };
    #[cfg(not(target_os = "macos"))]
    let b = b.shadow(false);
    let w = b.build()?;
    let _ = w.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
    Ok(())
}

/// Shows the popover at the tray icon (`rect`), or hides it if it's showing.
fn toggle_popover(app: &AppHandle, rect: Option<Rect>, view: &str) {
    let Some(w) = app.get_webview_window("popover") else { return };
    if rect.is_some() {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
            return;
        }
        if PLACE.lock().unwrap().hidden_at.is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
            return;
        }
    }
    let anchor = rect.or_else(|| PLACE.lock().unwrap().rect);
    place(app, &w, anchor);
    let _ = w.eval(format!("window.TR && TR.show && TR.show({view:?})"));
    let _ = app.emit_to("popover", "tr://show", view);
    let _ = w.show();
    let _ = w.set_focus();
}

/// Under the icon on a top menu bar (macOS, most Linux panels), above it on a bottom taskbar
/// (Windows), kept inside the work area. Without an icon position, the top-right (macOS) or
/// bottom-right corner.
fn place(app: &AppHandle, w: &tauri::WebviewWindow, rect: Option<Rect>) {
    let mon = match rect {
        Some(r) => {
            let p = r.position.to_physical::<f64>(1.0);
            app.monitor_from_point(p.x, p.y).ok().flatten()
        }
        None => None,
    }
    .or_else(|| app.primary_monitor().ok().flatten());
    let Some(mon) = mon else { return };
    let (wa, scale) = (*mon.work_area(), mon.scale_factor());
    let size = w.outer_size().unwrap_or_default();
    let (pw, ph) = (size.width as f64, size.height as f64);
    let margin = 8.0 * scale;
    let (x0, y0) = (wa.position.x as f64, wa.position.y as f64);
    let (x1, y1) = (x0 + wa.size.width as f64, y0 + wa.size.height as f64);
    let (x, y) = match rect {
        Some(r) => {
            let (pos, sz) = (r.position.to_physical::<f64>(scale), r.size.to_physical::<f64>(scale));
            let below = pos.y < y0 + (y1 - y0) / 2.0;
            let x = (pos.x + sz.width / 2.0 - pw / 2.0).clamp(x0 + margin, (x1 - pw - margin).max(x0));
            let y =
                if below { (pos.y + sz.height + 4.0 * scale).max(y0 + 4.0 * scale) } else { (pos.y - ph - margin).min(y1 - ph - margin) };
            (x, y)
        }
        None if cfg!(target_os = "macos") => (x1 - pw - margin, y0 + 4.0 * scale),
        None => (x1 - pw - margin, y1 - ph - margin),
    };
    let _ = w.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
}

pub fn open_view(app: &AppHandle, view: &str) {
    toggle_popover(app, None, view);
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| open_view(app, "now")))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            get_info,
            open_site,
            updates::check_for_updates,
            updates::update_notes,
            updates::install_update,
            updates::open_update,
            commands::get_state,
            commands::get_stats,
            commands::get_harness,
            commands::get_dashboard,
            commands::list_detected,
            commands::list_aws_profiles,
            commands::login_account,
            commands::add_account,
            commands::remove_account,
            commands::rename_account,
            commands::refresh,
            commands::open_provider,
            commands::request_draft,
            commands::send_request,
            commands::copy_text,
            commands::get_settings,
            commands::set_settings,
            commands::snooze,
            commands::open_dashboard,
            commands::open_settings,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let h = app.handle().clone();
            let dir = app.path().app_data_dir()?;
            debug(&format!("launch v{} data {}", env!("CARGO_PKG_VERSION"), dir.display()));
            app.manage(AppState::new(dir));
            create_popover(&h)?;
            notify::init(&h);
            TrayIconBuilder::with_id("main")
                .icon(tray::image(tray::Level::Normal).expect("tray icon"))
                .icon_as_template(cfg!(target_os = "macos"))
                .tooltip("Tokenrunway · iokig.com")
                .menu(&tray::menu(&h)?)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| tray::on_menu(app, e.id().as_ref()))
                .on_tray_icon_event(|tray, event| {
                    match &event {
                        TrayIconEvent::Click { rect, .. } | TrayIconEvent::Enter { rect, .. } | TrayIconEvent::Move { rect, .. } => {
                            PLACE.lock().unwrap().rect = Some(*rect);
                        }
                        _ => {}
                    }
                    if let TrayIconEvent::Click { rect, button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        toggle_popover(tray.app_handle(), Some(rect), "now");
                    }
                })
                .build(app)?;
            // Keep the saved start-at-login choice and the system's in step.
            {
                use tauri_plugin_autostart::ManagerExt;
                let want = app.state::<AppState>().settings.lock().unwrap().start_at_login;
                let al = h.autolaunch();
                if al.is_enabled().unwrap_or(false) != want {
                    let _ = if want { al.enable() } else { al.disable() };
                }
            }
            activity::start(&h);
            tauri::async_runtime::spawn(poll::run(h.clone()));
            tauri::async_runtime::spawn(updates::run(h));
            Ok(())
        })
        .on_window_event(|w, e| {
            if w.label() == "popover" {
                if let WindowEvent::Focused(false) = e {
                    let _ = w.hide();
                    PLACE.lock().unwrap().hidden_at = Some(Instant::now());
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Tokenrunway");
    app.run(|_app, event| {
        // Closing the dashboard must not quit a tray app.
        if let RunEvent::ExitRequested { api, code: None, .. } = event {
            api.prevent_exit();
        }
    });
}
