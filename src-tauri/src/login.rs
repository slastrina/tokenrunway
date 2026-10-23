//! The in-app login window (SPEC.md §8.1, method "login"): a separate webview, labelled
//! `login-<account id>`, that loads the provider's own sign-in page, so SSO, passkeys and 2FA
//! work as they do in a browser.
//!
//! - **No IPC.** The window isn't in capabilities/default.json, and it loads a remote page, so
//!   it can't call any Tauri command.
//! - **Its own web data per account**, so work and personal sessions (and the app's own windows)
//!   never mix: `data_store_identifier` on macOS 14+, a private (non-persistent) store on older
//!   macOS, `data_directory` (`<app data>/login/<account id>`) on Windows and Linux.
//! - Every second it reads the window's cookies and, once the provider's session cookie is there
//!   (allowance::login::spec), keeps **only that cookie** in the OS keychain, closes the window
//!   and returns. Closing the window or 10 minutes without a login cancels it. Cookie values are
//!   never logged.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tokenrunway_core::allowance::{self, login, secrets, AccountRecord};

const TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const CANCELLED: &str = "The login window was closed before signing in finished.";

pub fn label(account_id: &str) -> String {
    format!("login-{account_id}")
}

/// The major macOS version (sw_vers), read once.
#[cfg(target_os = "macos")]
fn macos_major() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().split('.').next()?.parse().ok())
            .unwrap_or(0)
    })
}

#[cfg(target_os = "macos")]
fn isolate<'a>(
    _app: &AppHandle,
    b: WebviewWindowBuilder<'a, tauri::Wry, AppHandle>,
    account_id: &str,
) -> Result<WebviewWindowBuilder<'a, tauri::Wry, AppHandle>, String> {
    Ok(if macos_major() >= 14 { b.data_store_identifier(login::store_id(account_id)) } else { b.incognito(true) })
}

#[cfg(not(target_os = "macos"))]
fn isolate<'a>(
    app: &AppHandle,
    b: WebviewWindowBuilder<'a, tauri::Wry, AppHandle>,
    account_id: &str,
) -> Result<WebviewWindowBuilder<'a, tauri::Wry, AppHandle>, String> {
    Ok(b.data_directory(data_dir(app, account_id)?))
}

#[cfg(not(target_os = "macos"))]
fn data_dir(app: &AppHandle, account_id: &str) -> Result<std::path::PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("login").join(account_id))
}

/// Opens the login window for the account and waits until its session cookie is in the
/// keychain (Ok), or the window is closed or times out (Err).
pub async fn run(app: &AppHandle, acct: &AccountRecord) -> Result<(), String> {
    let spec = login::spec(&acct.provider).ok_or("This provider has no sign-in page Tokenrunway can use.")?;
    let label = label(&acct.id);
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Err("The login window is already open.".into());
    }
    let url = spec.start_url.parse().map_err(|e| format!("Bad sign-in address: {e}"))?;
    let b = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .title(format!("Log in to {} · Tokenrunway", allowance::provider_name(&acct.provider)))
        .inner_size(520.0, 760.0)
        .min_inner_size(380.0, 480.0)
        .resizable(true)
        .center()
        .focused(true);
    if let Some(p) = app.get_webview_window("popover") {
        let _ = p.hide();
    }
    let w = isolate(app, b, &acct.id)?.build().map_err(|e| format!("Couldn't open the login window: {e}"))?;
    crate::debug(&format!("login {}: window open ({})", acct.id, spec.host));
    let closed = Arc::new(AtomicBool::new(false));
    {
        let closed = closed.clone();
        w.on_window_event(move |e| {
            if matches!(e, WindowEvent::Destroyed) {
                closed.store(true, Ordering::SeqCst);
            }
        });
    }
    let previous = secrets::get(&secrets::session_name(&acct.id));
    let started = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if closed.load(Ordering::SeqCst) {
            crate::debug(&format!("login {}: closed by the user", acct.id));
            return Err(CANCELLED.into());
        }
        if started.elapsed() > TIMEOUT {
            let _ = w.destroy();
            crate::debug(&format!("login {}: timed out", acct.id));
            return Err("Signing in timed out after 10 minutes. Try again.".into());
        }
        // Reading cookies waits on the main thread, so it runs off the async workers (on Windows
        // it would deadlock from a synchronous context).
        let wc = w.clone();
        let Ok(Ok(jar)) = tauri::async_runtime::spawn_blocking(move || wc.cookies()).await else { continue };
        let jar: Vec<(String, String, String)> =
            jar.iter().map(|c| (c.domain().unwrap_or_default().to_string(), c.name().to_string(), c.value().to_string())).collect();
        if let Some(session) = login::session_from(&spec, &jar, previous.as_deref()) {
            secrets::set(&secrets::session_name(&acct.id), &session)?;
            let _ = w.destroy();
            crate::debug(&format!("login {}: signed in, session saved", acct.id));
            return Ok(());
        }
    }
}

/// Removes an account's login: its keychain entry, an open login window and its web data.
pub fn forget(app: &AppHandle, account_id: &str) {
    secrets::delete(&secrets::session_name(account_id));
    if let Some(w) = app.get_webview_window(&label(account_id)) {
        let _ = w.destroy();
    }
    #[cfg(target_os = "macos")]
    if macos_major() >= 14 {
        let (app, id) = (app.clone(), login::store_id(account_id));
        tauri::async_runtime::spawn(async move {
            // The store can't be removed while its window is still closing; try again shortly.
            for _ in 0..3 {
                if app.remove_data_store(id).await.is_ok() {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    if let Ok(dir) = data_dir(app, account_id) {
        let _ = std::fs::remove_dir_all(dir);
    }
}
