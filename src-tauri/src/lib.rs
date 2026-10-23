//! Tokenrunway's desktop app: the tray icon, its popover and native menu, and update checks.
//! Everything that isn't windows and menus lives in tokenrunway-core (see SPEC.md §7).

mod updates;

use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, Rect, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::MacosLauncher;

/// The project page: the credit in the popover footer, the tray menu and the tooltip.
pub const SITE: &str = "https://www.iokig.com/tokenrunway";
const POPOVER: (f64, f64) = (380.0, 560.0);

#[derive(Default)]
pub struct AppState {
    /// The newest release the last check found, and its page.
    pub update: Mutex<Option<tokenrunway_core::update::Release>>,
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
/// it loses focus.
fn create_popover(app: &AppHandle) -> tauri::Result<()> {
    let w = WebviewWindowBuilder::new(app, "popover", WebviewUrl::App("popover.html".into()))
        .title("Tokenrunway")
        .inner_size(POPOVER.0, POPOVER.1)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()?;
    let h = w.clone();
    w.on_window_event(move |e| {
        if let tauri::WindowEvent::Focused(false) = e {
            let _ = h.hide();
        }
    });
    Ok(())
}

/// Shows the popover at the tray icon (`rect`), or hides it if it's showing.
fn toggle_popover(app: &AppHandle, rect: Option<Rect>, view: &str) {
    let Some(w) = app.get_webview_window("popover") else { return };
    if rect.is_some() && w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        return;
    }
    if let (Some(rect), Ok(Some(mon))) = (rect, w.current_monitor()) {
        let scale = mon.scale_factor();
        let (pos, size) = (rect.position.to_physical::<f64>(scale), rect.size.to_physical::<f64>(scale));
        let (pw, ph) = (POPOVER.0 * scale, POPOVER.1 * scale);
        let (mp, ms) = (mon.position(), mon.size());
        // A menubar or top panel puts it below the icon; a taskbar at the bottom, above.
        let below = pos.y < mp.y as f64 + ms.height as f64 / 2.0;
        let x = (pos.x + size.width / 2.0 - pw / 2.0).clamp(mp.x as f64, mp.x as f64 + ms.width as f64 - pw);
        let y = if below { pos.y + size.height + 4.0 } else { pos.y - ph - 4.0 };
        let _ = w.set_position(PhysicalPosition::new(x, y));
    }
    let _ = w.eval(format!("window.TR && TR.show({view:?})"));
    let _ = w.show();
    let _ = w.set_focus();
}

pub fn open_view(app: &AppHandle, view: &str) {
    toggle_popover(app, None, view);
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| open_view(app, "now")))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_info,
            open_site,
            updates::check_for_updates,
            updates::update_notes,
            updates::install_update,
            updates::open_update,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let h = app.handle().clone();
            create_popover(&h)?;
            let menu = Menu::with_items(
                app,
                &[
                    &MenuItem::with_id(
                        app,
                        "site",
                        concat!("Tokenrunway v", env!("CARGO_PKG_VERSION"), " on iokig.com"),
                        true,
                        None::<&str>,
                    )?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "open", "Open Tokenrunway", true, None::<&str>)?,
                    &MenuItem::with_id(app, "check_updates", "Check for updates…", true, None::<&str>)?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "quit", "Quit Tokenrunway", true, None::<&str>)?,
                ],
            )?;
            TrayIconBuilder::with_id("main")
                .icon(tauri::include_image!("icons/tray.png"))
                .icon_as_template(true)
                .tooltip("Tokenrunway · iokig.com")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id().as_ref() {
                    "site" => open_site(app.clone()),
                    "open" => open_view(app, "now"),
                    "update" | "check_updates" => open_view(app, "update"),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { rect, button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        toggle_popover(tray.app_handle(), Some(rect), "now");
                    }
                })
                .build(app)?;
            tauri::async_runtime::spawn(updates::run(h));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Tokenrunway");
}
