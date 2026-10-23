//! Native notifications (SPEC.md §8.3) through tauri-plugin-notification. The rules and their
//! wording live in `tokenrunway_core::allowance::alerts`; this only shows them. Clicking one
//! opens the popover. Action buttons aren't reliable on desktop, so "Request more" lives in the
//! popover's critical banner instead.

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;
use tokenrunway_core::allowance::alerts::Alert;

/// Opens the popover when a notification is clicked (registered once at launch).
pub fn init(app: &AppHandle) {
    let h = app.clone();
    let _ = app.notification().on_action(move |_| {
        let h2 = h.clone();
        let _ = h.run_on_main_thread(move || crate::open_view(&h2, "now"));
    });
}

pub fn show(app: &AppHandle, alerts: &[Alert]) {
    for a in alerts.iter().filter(|a| a.desktop) {
        crate::debug(&format!("notify: {} — {}", a.title, a.body));
        let _ = app.notification().builder().title(&a.title).body(&a.body).show();
    }
}
