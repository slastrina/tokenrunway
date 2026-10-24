//! The tray icon (docs/design/MenubarIcon.dc.html): normal (a template image on macOS), warning
//! (amber + triangle) or critical (red + dot) for the tightest window; the optional label next to
//! it on macOS and Linux (in the tooltip on Windows, which can't show text in the tray); the
//! tooltip with the iokig.com credit; and the native menu.

use crate::state::AppState;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, Wry};
use tokenrunway_core::forecast;
use tokenrunway_core::view::{AccountView, AppView, Status, WindowView};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Normal,
    Warn,
    Crit,
}

pub fn image(level: Level) -> Option<Image<'static>> {
    let mac = cfg!(target_os = "macos");
    let bytes: &'static [u8] = match (mac, level) {
        (true, Level::Normal) => include_bytes!("../icons/tray-mac.png"),
        (true, Level::Warn) => include_bytes!("../icons/tray-mac-warn.png"),
        (true, Level::Crit) => include_bytes!("../icons/tray-mac-crit.png"),
        (false, Level::Normal) => include_bytes!("../icons/tray-win.png"),
        (false, Level::Warn) => include_bytes!("../icons/tray-win-warn.png"),
        (false, Level::Crit) => include_bytes!("../icons/tray-win-crit.png"),
    };
    Image::from_bytes(bytes).ok()
}

/// The window the label is about: the pinned one (Settings → Menubar label → Window) while it
/// exists, otherwise the most urgent.
pub fn target(view: &AppView) -> Option<(&AccountView, &WindowView)> {
    let pinned = view.settings.menubar_window.split_once('/').and_then(|(a, w)| {
        let acct = view.accounts.iter().find(|x| x.id == a)?;
        Some((acct, acct.windows.iter().find(|x| x.id == w)?))
    });
    pinned.or_else(|| forecast::tightest(&view.accounts))
}

/// What the tray shows for a view: icon level, label and tooltip.
pub fn describe(view: &AppView) -> (Level, Option<String>, String) {
    let now = tokenrunway_core::now_ms();
    let mut level = match view.summary.as_ref().map(|s| s.status) {
        Some(Status::Crit) => Level::Crit,
        Some(Status::Warn) => Level::Warn,
        _ => Level::Normal,
    };
    let pinned = !view.settings.menubar_window.is_empty();
    let tight = target(view);
    let spend = view.settings.menubar_label == "spend";
    let label = match (view.settings.menubar_label.as_str(), &view.summary, tight) {
        ("percent", _, Some((_, w))) => Some(format!("{:.0}%", (100.0 - w.pct).max(0.0))),
        ("runway", _, Some((_, w))) if pinned => Some(match (w.out_at, w.resets_at) {
            (Some(o), _) => forecast::duration(o - now),
            (None, Some(r)) => forecast::duration(r - now),
            _ => format!("{:.0}%", (100.0 - w.pct).max(0.0)),
        }),
        ("runway", Some(s), _) => Some(s.label.clone()),
        ("spend", _, _) => view.spend_today.map(money),
        _ => None,
    };
    // With "$ today" showing, the icon also warns as the daily goal gets close.
    let goal = view.settings.budget_daily;
    if let (true, Some(d), true) = (spend, view.spend_today, goal > 0.0) {
        let budget = if d >= goal {
            Level::Crit
        } else if d >= goal * 0.8 {
            Level::Warn
        } else {
            Level::Normal
        };
        if budget == Level::Crit || (budget == Level::Warn && level == Level::Normal) {
            level = budget;
        }
    }
    let mut parts = vec!["Tokenrunway".to_string()];
    if let Some(v) = &view.update {
        parts.push(format!("v{v} available"));
    }
    if spend {
        if let Some(d) = view.spend_today {
            parts.push(if goal > 0.0 { format!("Today {} of {}", money(d), money(goal)) } else { format!("Today {}", money(d)) });
        }
    } else if cfg!(windows) {
        if let Some(l) = &label {
            parts.push(l.clone());
        }
    }
    if let Some((a, w)) = tight {
        let mut s = format!("{} {}: {:.0}% used", a.name, w.label, w.pct);
        match (w.out_at, w.resets_at) {
            (Some(o), _) => s += &format!(", out {}", forecast::when(o, now)),
            (None, Some(r)) => s += &format!(", resets {}", forecast::when(r, now)),
            _ => {}
        }
        parts.push(s);
    }
    parts.push("iokig.com".into());
    let mut tip = parts.join(" · ");
    if cfg!(windows) && tip.chars().count() > 127 {
        // Windows cuts tooltips at 128 characters; keep the credit.
        let keep: String = tip.chars().take(112).collect();
        tip = format!("{keep}… · iokig.com");
    }
    (level, label, tip)
}

/// Short dollars for the tray: cents under $100, whole dollars above, then "$1.2k".
fn money(d: f64) -> String {
    match d {
        d if d < 100.0 => format!("${d:.2}"),
        d if d < 10_000.0 => format!("${d:.0}"),
        d => format!("${:.1}k", d / 1000.0),
    }
}

/// Applies the view to the tray icon, its label and tooltip.
pub fn update(app: &AppHandle, view: &AppView) {
    let (level, label, tip) = describe(view);
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(t) = h.tray_by_id("main") else { return };
        let _ = t.set_icon(image(level));
        #[cfg(target_os = "macos")]
        let _ = t.set_icon_as_template(level == Level::Normal);
        #[cfg(not(windows))]
        let _ = t.set_title(label.as_deref());
        #[cfg(windows)]
        let _ = label;
        let _ = t.set_tooltip(Some(tip));
    });
}

/// The native menu: the iokig.com credit first (SPEC.md §6.5), then the actions.
pub fn menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let paused = {
        let st = app.state::<AppState>();
        let p = st.settings.lock().unwrap().paused_until;
        p > tokenrunway_core::now_ms()
    };
    let update = crate::updates::available(app);
    let pause = Submenu::with_items(
        app,
        if paused { "Notifications paused" } else { "Pause notifications" },
        true,
        &[
            &MenuItem::with_id(app, "pause_60", "For 1 hour", true, None::<&str>)?,
            &MenuItem::with_id(app, "pause_120", "For 2 hours", true, None::<&str>)?,
            &MenuItem::with_id(app, "pause_forever", "Until resumed", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "pause_resume", "Resume", paused, None::<&str>)?,
        ],
    )?;
    let m = Menu::new(app)?;
    if let Some(v) = update {
        m.append(&MenuItem::with_id(app, "update", format!("Update available: v{v}…"), true, None::<&str>)?)?;
    }
    m.append_items(&[
        &MenuItem::with_id(app, "site", concat!("Tokenrunway v", env!("CARGO_PKG_VERSION"), " on iokig.com"), true, None::<&str>)?,
        &PredefinedMenuItem::separator(app)?,
        // Left-click isn't reliable on every Linux desktop, so the menu can open the popover too.
        &MenuItem::with_id(app, "open", "Open Tokenrunway", true, None::<&str>)?,
        &MenuItem::with_id(app, "dashboard", "Open dashboard", true, None::<&str>)?,
        &MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?,
        &pause,
        &PredefinedMenuItem::separator(app)?,
        &MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?,
        &MenuItem::with_id(app, "check_updates", "Check for updates…", true, None::<&str>)?,
        &PredefinedMenuItem::separator(app)?,
        &MenuItem::with_id(app, "quit", "Quit Tokenrunway", true, None::<&str>)?,
    ])?;
    Ok(m)
}

/// Rebuilds the menu (after pausing, or when an update turns up).
pub fn refresh_menu(app: &AppHandle) {
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let (Some(t), Ok(m)) = (h.tray_by_id("main"), menu(&h)) {
            let _ = t.set_menu(Some(m));
        }
    });
}

pub fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "site" => crate::open_site(app.clone()),
        "open" => crate::open_view(app, "last"),
        "dashboard" => crate::commands::open_dashboard(app.clone()),
        "refresh" => crate::poll::force(app, None),
        "settings" => crate::open_view(app, "settings"),
        "update" | "check_updates" => crate::open_view(app, "update"),
        "pause_60" => crate::commands::snooze(app.clone(), Some(60)),
        "pause_120" => crate::commands::snooze(app.clone(), Some(120)),
        "pause_forever" => crate::commands::snooze(app.clone(), None),
        "pause_resume" => crate::commands::snooze(app.clone(), Some(0)),
        "quit" => app.exit(0),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenrunway_core::view::{AccountView, Settings, Summary, WindowView};

    #[test]
    fn label_and_tooltip() {
        let mut view = AppView {
            summary: Some(Summary {
                label: "1h 26m".into(),
                status: Status::Crit,
                window: "Claude — work · 5-hour".into(),
                ..Default::default()
            }),
            accounts: vec![AccountView {
                name: "Claude — work".into(),
                windows: vec![WindowView { label: "5-hour".into(), pct: 78.0, status: Status::Crit, ..Default::default() }],
                ..Default::default()
            }],
            settings: Settings::default(),
            ..Default::default()
        };
        let (level, label, tip) = describe(&view);
        assert_eq!(level, Level::Crit);
        assert_eq!(label.as_deref(), Some("1h 26m"));
        assert!(tip.starts_with("Tokenrunway · "));
        assert!(tip.contains("Claude — work 5-hour: 78% used"));
        assert!(tip.ends_with(" · iokig.com"));
        view.settings.menubar_label = "percent".into();
        assert_eq!(describe(&view).1.as_deref(), Some("22%"), "% left");
        view.settings.menubar_label = "spend".into();
        assert_eq!(describe(&view).1, None, "nothing until the logs are read");
        view.spend_today = Some(12.4);
        let (_, label, tip) = describe(&view);
        assert_eq!(label.as_deref(), Some("$12.40"));
        assert!(tip.contains(" · Today $12.40 · "));
        view.settings.budget_daily = 15.0;
        let (level, _, tip) = describe(&view);
        assert!(tip.contains(" · Today $12.40 of $15.00 · "), "{tip}");
        assert_eq!(level, Level::Crit, "the window is critical anyway");
        view.summary.as_mut().unwrap().status = Status::Ok;
        assert_eq!(describe(&view).0, Level::Warn, "83% of the daily goal");
        view.spend_today = Some(15.0);
        assert_eq!(describe(&view).0, Level::Crit, "goal reached");
        view.settings.budget_daily = 0.0;
        view.settings.menubar_label = "none".into();
        assert_eq!(describe(&view).1, None);
        assert_eq!(describe(&AppView::default()).2, "Tokenrunway · iokig.com");
        assert_eq!(money(0.0), "$0.00");
        assert_eq!(money(318.4), "$318");
        assert_eq!(money(12_340.0), "$12.3k");
        for l in [Level::Normal, Level::Warn, Level::Crit] {
            assert!(image(l).is_some());
        }
    }

    #[test]
    fn pinned_window() {
        let now = tokenrunway_core::now_ms();
        let mut view = AppView {
            summary: Some(Summary { label: "1h 26m".into(), status: Status::Crit, ..Default::default() }),
            accounts: vec![AccountView {
                id: "c".into(),
                name: "Claude".into(),
                windows: vec![
                    WindowView { id: "5h".into(), label: "5-hour".into(), pct: 78.0, status: Status::Crit, ..Default::default() },
                    WindowView {
                        id: "weekly".into(),
                        label: "Weekly".into(),
                        pct: 40.0,
                        resets_at: Some(now + (2 * 24 * 60 + 90) * 60_000 + 30_000),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        view.settings.menubar_window = "c/weekly".into();
        assert_eq!(describe(&view).1.as_deref(), Some("2d 1h"), "time to its reset");
        view.settings.menubar_label = "percent".into();
        assert_eq!(describe(&view).1.as_deref(), Some("60%"));
        assert!(describe(&view).2.contains("Claude Weekly: 40% used"));
        // A pinned window that's gone falls back to the most urgent.
        view.settings.menubar_window = "c/gone".into();
        assert_eq!(describe(&view).1.as_deref(), Some("22%"));
    }
}
