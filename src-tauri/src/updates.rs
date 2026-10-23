//! Updates, as in Rattleboard: checks GitHub for a newer release (see `tokenrunway_core::update`)
//! and, when there is one, offers it in the tray menu and the popover footer. The update view
//! shows what's new in each release since this one. Nothing is downloaded or installed until the
//! user says so.
//!
//! Installing goes through Tauri's updater: it reads `latest.json` from the latest release (made
//! by the release workflow), downloads this platform's bundle, checks its signature against the
//! public key in tauri.conf.json, replaces this copy and restarts. Homebrew copies update with
//! `brew upgrade`, and a .deb (or a debug build) from the release page.

use crate::AppState;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokenrunway_core::update;

const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// The newer version the last check found, if any.
pub fn available(app: &AppHandle) -> Option<String> {
    let st = app.state::<AppState>();
    let u = st.update.lock().unwrap();
    u.as_ref().filter(|r| update::is_newer(&r.version, CURRENT)).map(|r| r.version.clone())
}

/// Debug builds only: TOKENRUNWAY_UPDATE_FAKE_LATEST=9.9.9 pretends GitHub answered that.
fn fake_latest() -> Option<update::Release> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let v = update::Version::parse(&std::env::var("TOKENRUNWAY_UPDATE_FAKE_LATEST").ok()?)?;
    Some(update::Release { version: v.to_string(), url: update::RELEASES.into() })
}

async fn check(app: &AppHandle) -> Result<update::Release, String> {
    let r = match fake_latest() {
        Some(r) => r,
        None => update::latest(&update::client()).await?,
    };
    *app.state::<AppState>().update.lock().unwrap() = Some(r.clone());
    sync_tray(app);
    let _ = app.emit("tr://info", ());
    Ok(r)
}

/// Automatic checks: about 30 s after launch, then once a day. Failures are quiet; the next
/// tick tries again.
pub async fn run(app: AppHandle) {
    tokio::time::sleep(Duration::from_secs(30)).await;
    loop {
        let _ = check(&app).await;
        tokio::time::sleep(Duration::from_millis(update::EVERY_MS as u64)).await;
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checked {
    current: &'static str,
    latest: String,
    newer: bool,
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<Checked, String> {
    let r = check(&app).await?;
    Ok(Checked { current: CURRENT, newer: update::is_newer(&r.version, CURRENT), latest: r.version })
}

/// How this copy updates: "install" (in place), "homebrew" (`brew upgrade`) or "download".
pub fn how() -> &'static str {
    if cfg!(debug_assertions) && std::env::var("TOKENRUNWAY_UPDATE_TEST").as_deref() != Ok("1") {
        "download"
    } else if from_homebrew() {
        "homebrew"
    } else if cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_none() {
        "download"
    } else {
        "install"
    }
}

/// The notes of every release since this one, newest first.
#[tauri::command]
pub async fn update_notes() -> Result<Vec<update::Notes>, String> {
    let since = if fake_latest().is_some() { "0.0.0" } else { CURRENT };
    update::since(&update::client(), since).await
}

/// "Update and restart". Progress goes to the popover as tr://update-progress { got, total }.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    if how() != "install" {
        return Err("This copy of Tokenrunway can't update itself; download the new version instead.".into());
    }
    let found = app
        .updater()
        .map_err(|e| format!("Updates aren't set up in this build ({e})"))?
        .check()
        .await
        .map_err(|e| format!("Couldn't check for the update: {e}"))?;
    let Some(up) = found else { return Err("You're already up to date.".into()) };
    let (a, mut got) = (app.clone(), 0usize);
    up.download_and_install(
        move |n, total| {
            got += n;
            let _ = a.emit_to("popover", "tr://update-progress", serde_json::json!({ "got": got, "total": total }));
        },
        || {},
    )
    .await
    .map_err(|e| format!("Couldn't install the update: {e}"))?;
    app.restart();
}

/// Opens a release page: only ever a github.com/slastrina/tokenrunway https page.
#[tauri::command]
pub fn open_update(app: AppHandle, url: Option<String>) {
    use tauri_plugin_opener::OpenerExt;
    let url = url.filter(|u| update::is_release_url(u)).unwrap_or_else(|| update::RELEASES.to_string());
    let _ = app.opener().open_url(url, None::<&str>);
}

/// Whether this copy came from the Homebrew cask, which leaves its record in the Caskroom.
pub fn from_homebrew() -> bool {
    static BREW: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *BREW.get_or_init(|| {
        cfg!(target_os = "macos")
            && ["/opt/homebrew/Caskroom/tokenrunway", "/usr/local/Caskroom/tokenrunway"].iter().any(|p| std::path::Path::new(p).is_dir())
    })
}

/// Puts "Update available" at the top of the tray menu and in the tooltip.
pub fn sync_tray(app: &AppHandle) {
    if available(app).is_none() {
        return;
    }
    crate::tray::refresh_menu(app);
    crate::state::broadcast(app);
}
