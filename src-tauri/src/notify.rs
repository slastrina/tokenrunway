//! Native notifications (SPEC.md §8.3) through tauri-plugin-notification. The rules and their
//! wording live in `tokenrunway_core::allowance::alerts`; this only shows them. Clicking one
//! opens the popover. Action buttons aren't reliable on desktop, so "Request more" lives in the
//! popover's critical banner instead.
//!
//! Each one is also sent to the open windows as `tr://toast`, which they show in-app (the popover
//! and dashboard toasts), so an alert isn't missed when system notifications are off.

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;
use tokenrunway_core::allowance::alerts::{Alert, Kind};

/// Opens the popover when a notification is clicked (registered once at launch).
pub fn init(app: &AppHandle) {
    let h = app.clone();
    let _ = app.notification().on_action(move |_| {
        let h2 = h.clone();
        let _ = h.run_on_main_thread(move || crate::open_view(&h2, "now"));
    });
}

/// What a window's toast shows.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Toast {
    pub title: String,
    pub body: String,
    /// "crit" | "warn" | "info": the colour
    pub tone: &'static str,
    /// The account it's about, for its Request more / Open actions ("" for spend alerts)
    pub account_id: String,
}

fn tone(k: Kind) -> &'static str {
    match k {
        Kind::Crit | Kind::Projection | Kind::Burn => "crit",
        Kind::Warn | Kind::Budget | Kind::Login => "warn",
        Kind::Reset => "info",
    }
}

pub fn show(app: &AppHandle, alerts: &[Alert]) {
    let mut toasts = Vec::new();
    for a in alerts.iter().filter(|a| a.desktop) {
        crate::debug(&format!("notify: {} — {}", a.title, a.body));
        let _ = app.notification().builder().title(&a.title).body(&a.body).show();
        toasts.push(Toast { title: a.title.clone(), body: a.body.clone(), tone: tone(a.kind), account_id: a.account_id.clone() });
    }
    if !toasts.is_empty() {
        let _ = app.emit("tr://toast", &toasts);
    }
}

/// Settings → Notifications → Send a test.
#[tauri::command]
pub fn test_notification(app: AppHandle) {
    test(&app);
}

fn test(app: &AppHandle) {
    let t = Toast {
        title: "Tokenrunway notifications are on".into(),
        body: "This is how alerts look before you run out.".into(),
        tone: "info",
        account_id: String::new(),
    };
    let _ = app.notification().builder().title(&t.title).body(&t.body).show();
    let _ = app.emit("tr://toast", [t]);
}
