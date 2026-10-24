//! The weekly recap notification (`Stats::recap` in tokenrunway-core's insights.rs): once a
//! week, the first time the app is running on or after Monday 09:00 local, when last week had any
//! work. The week it was sent for is kept in recap.json, so restarting doesn't repeat it.
//! Settings → Notifications → Weekly recap turns it off; pausing notifications holds it.

use crate::state::AppState;
use chrono::{Datelike, Duration, Local, NaiveDate, Timelike, Weekday};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tokenrunway_core::allowance;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Sent {
    /// The Monday of the week it was last sent in, "2026-10-19"
    week: String,
}

/// The Monday of this week once it's due (Monday 09:00 or later), else None.
fn due_week(now: chrono::DateTime<Local>) -> Option<NaiveDate> {
    let d = now.date_naive();
    if now.weekday() == Weekday::Mon && now.hour() < 9 {
        return None;
    }
    Some(d - Duration::days(now.weekday().num_days_from_monday() as i64))
}

/// Called from the poll loop every 30 s; cheap unless a recap is due.
pub fn tick(app: &AppHandle) {
    let Some(week) = due_week(Local::now()).map(|d| d.format("%Y-%m-%d").to_string()) else { return };
    let st = app.state::<AppState>();
    let path = st.data_dir.join("recap.json");
    let sent: Sent = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if sent.week == week {
        return;
    }
    let (on, paused) = {
        let s = st.settings.lock().unwrap();
        (s.insights.weekly_recap, s.paused_until > tokenrunway_core::now_ms())
    };
    if !on || paused {
        return;
    }
    let Some(stats) = crate::activity::stats(app) else { return };
    if let Some(r) = stats.recap(tokenrunway_core::now_ms()) {
        crate::debug(&format!("recap: {} — {}", r.title, r.body));
        let _ = app.notification().builder().title(&r.title).body(&r.body).show();
    }
    // Sent (or nothing to say) for this week either way.
    if let Ok(json) = serde_json::to_string(&Sent { week }) {
        let _ = allowance::accounts::write_atomic(&path, &json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn due_from_monday_nine() {
        let at = |d, h| Local.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap();
        assert_eq!(due_week(at(19, 8)), None, "Monday before 09:00");
        let monday = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        assert_eq!(due_week(at(19, 9)), Some(monday));
        assert_eq!(due_week(at(22, 15)), Some(monday), "Thursday: still this week's");
        assert_eq!(due_week(at(25, 23)), Some(monday), "Sunday night");
        assert_eq!(due_week(at(26, 10)), Some(monday + Duration::days(7)));
    }
}
