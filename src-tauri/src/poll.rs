//! Remote polling (SPEC.md §7.3): per account, every 60 s while a window is at 70% or more or a
//! harness wrote to its logs in the last 5 minutes, every 5 minutes otherwise (never faster than
//! the provider allows). Errors back off exponentially, and a Retry-After is honoured. "Refresh
//! now" polls straight away, at most once per 15 s per account.

use crate::state::{broadcast, AppState};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokenrunway_core::allowance::{self, ProviderError};
use tokenrunway_core::now_ms;

const FAST_MS: i64 = 60_000;
const SLOW_MS: i64 = 5 * 60_000;
const MAX_BACKOFF_MS: i64 = 30 * 60_000;
pub const REFRESH_DEBOUNCE_MS: i64 = 15_000;

pub async fn run(app: AppHandle) {
    let mut last_broadcast = 0;
    loop {
        let now = now_ms();
        let due: Vec<String> = {
            let st = app.state::<AppState>();
            let ids: Vec<String> = st.accounts.lock().unwrap().list.iter().map(|a| a.id.clone()).collect();
            let mut sched = st.sched.lock().unwrap();
            ids.into_iter()
                .filter(|id| {
                    let s = sched.entry(id.clone()).or_default();
                    if !s.in_flight && s.next_due <= now {
                        s.in_flight = true;
                        true
                    } else {
                        false
                    }
                })
                .collect()
        };
        for id in due {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { poll_one(&app, &id).await });
        }
        // The summary's runway and the tray label move with the clock.
        if now - last_broadcast >= 30_000 {
            broadcast(&app);
            last_broadcast = now;
        }
        let st = app.state::<AppState>();
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = st.wake.notified() => {}
        }
    }
}

/// Reads one account now and schedules its next reading. The caller has set `in_flight`.
pub async fn poll_one(app: &AppHandle, id: &str) {
    let st = app.state::<AppState>();
    let Some(rec) = st.accounts.lock().unwrap().get(id).cloned() else {
        st.sched.lock().unwrap().remove(id);
        return;
    };
    let provider = allowance::provider_for(&rec);
    let result = match &provider {
        Some(p) => p.fetch(&rec, &st.ctx).await,
        None => Err(ProviderError::Unavailable("This version of Tokenrunway can't read this account yet.".into())),
    };
    let now = now_ms();
    crate::debug(&format!(
        "poll {id} ({}/{}): {}",
        rec.provider,
        rec.method,
        match &result {
            Ok(f) => format!(
                "ok, plan {:?}, windows [{}]{}",
                f.plan,
                f.windows.iter().map(|w| format!("{} {:.1}%", w.id, w.used * 100.0)).collect::<Vec<_>>().join(", "),
                f.caveat.as_ref().map(|c| format!(", caveat {c:?}")).unwrap_or_default()
            ),
            Err(e) => format!("error {e:?}"),
        }
    ));
    let hot;
    let error = result.as_ref().err().cloned();
    match result {
        Ok(f) => {
            let snaps: Vec<_> = f.windows.iter().map(|w| w.snapshot()).collect();
            st.snaps.add(id, &snaps);
            hot = f.windows.iter().any(|w| w.used >= 0.7);
            if let Some(plan) = f.plan.clone().filter(|p| *p != rec.plan) {
                let mut accts = st.accounts.lock().unwrap();
                if let Some(a) = accts.get_mut(id) {
                    a.plan = plan;
                    let _ = accts.save();
                }
            }
            let updated = f.windows.iter().map(|w| w.at).min().filter(|_| f.caveat.is_some() || rec.provider == "bedrock").unwrap_or(now);
            let mut live = st.live.lock().unwrap();
            let l = live.entry(id.to_string()).or_default();
            l.last = Some(f);
            l.updated_at = Some(updated);
            l.error = None;
            l.tried = true;
        }
        Err(e) => {
            let mut live = st.live.lock().unwrap();
            let l = live.entry(id.to_string()).or_default();
            hot = l.last.as_ref().is_some_and(|f| f.windows.iter().any(|w| w.used >= 0.7));
            l.error = Some(e);
            l.tried = true;
        }
    }
    let min = provider.as_ref().map(|p| p.min_interval_ms()).unwrap_or(SLOW_MS);
    let base = if hot || st.harness_active(now) { FAST_MS } else { SLOW_MS }.max(min);
    {
        let mut sched = st.sched.lock().unwrap();
        let s = sched.entry(id.to_string()).or_default();
        s.in_flight = false;
        let wait = match &error {
            None => {
                s.failures = 0;
                base
            }
            Some(e) => {
                s.failures = s.failures.saturating_add(1);
                let backoff = (base << s.failures.min(5)).min(MAX_BACKOFF_MS);
                match e {
                    ProviderError::RateLimited(Some(secs)) => backoff.max(*secs as i64 * 1000),
                    // A CLI login is re-read locally (no network) until it's fixed; check often
                    // so signing in again shows up quickly. A refused key or session cookie
                    // backs off instead of knocking on the provider's door.
                    ProviderError::AuthExpired(_) if matches!(rec.method.as_str(), "cli" | "aws") => 2 * 60_000,
                    _ => backoff,
                }
            }
        };
        s.next_due = now + wait;
    }
    broadcast(app);
}

/// "Refresh now": polls the account (or every account) at once, unless it was forced within
/// the last 15 s.
pub fn force(app: &AppHandle, id: Option<&str>) {
    let st = app.state::<AppState>();
    let now = now_ms();
    let ids: Vec<String> = match id {
        Some(i) => vec![i.to_string()],
        None => st.accounts.lock().unwrap().list.iter().map(|a| a.id.clone()).collect(),
    };
    {
        let mut sched = st.sched.lock().unwrap();
        for i in ids {
            let s = sched.entry(i).or_default();
            if now - s.last_forced >= REFRESH_DEBOUNCE_MS {
                s.last_forced = now;
                s.next_due = 0;
            }
        }
    }
    st.wake.notify_one();
}
