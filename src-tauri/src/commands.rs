//! The commands the UI calls (src/*.js). Names and argument names are the contract: JS passes
//! camelCase (`accountId`), Tauri maps it to these snake_case parameters.

use crate::activity;
use crate::state::{app_view, broadcast, AppState};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;
use tokenrunway_core::allowance::{self, request, secrets, AccountRecord, ProviderError};
use tokenrunway_core::forecast;
use tokenrunway_core::insights::{self, Subscription};
use tokenrunway_core::now_ms;
use tokenrunway_core::stats::Filter;
use tokenrunway_core::view::{
    AccountView, AdminContact, AppView, Burndown, DashboardView, Detected, HarnessView, InsightsView, RequestDraft, ResetRow, Settings,
    StatsView,
};

#[tauri::command]
pub fn get_state(app: AppHandle) -> AppView {
    app_view(&app)
}

#[tauri::command]
pub async fn get_stats(app: AppHandle, period: String) -> StatsView {
    let Some(s) = activity::stats(&app) else { return StatsView { period, ..Default::default() } };
    tauri::async_runtime::spawn_blocking(move || s.stats(&period, now_ms())).await.unwrap_or_default()
}

#[tauri::command]
pub async fn get_harness(app: AppHandle, period: String) -> HarnessView {
    let Some(s) = activity::stats(&app) else { return HarnessView { period, ..Default::default() } };
    tauri::async_runtime::spawn_blocking(move || s.harness(&period, now_ms())).await.unwrap_or_default()
}

/// The subscription accounts whose work the local logs show (insights.rs: subscription value).
pub fn subscriptions(accounts: &[AccountView]) -> Vec<Subscription> {
    accounts
        .iter()
        .filter(|a| !insights::subscription_harnesses(&a.provider).is_empty() && matches!(a.method.as_str(), "cli" | "login"))
        .map(|a| Subscription { account_id: a.id.clone(), provider: a.provider.clone(), name: a.name.clone(), plan: a.plan.clone() })
        .collect()
}

/// The dashboard's insight cards and the popover's extras. `project`: that project only, with
/// all its branches.
#[tauri::command]
pub async fn get_insights(app: AppHandle, period: String, project: Option<String>) -> InsightsView {
    let Some(s) = activity::stats(&app) else { return InsightsView { period, ..Default::default() } };
    let now = now_ms();
    let st = app.state::<AppState>();
    let subs = subscriptions(&st.account_views(now));
    let prices = st.settings.lock().unwrap().insights.plan_prices.clone();
    tauri::async_runtime::spawn_blocking(move || s.insights(&period, project.as_deref(), &subs, &prices, now)).await.unwrap_or_default()
}

/// `project` / `model` (a display name, "Opus 5.5") narrow the activity parts to that one.
#[tauri::command]
pub async fn get_dashboard(
    app: AppHandle,
    period: String,
    group: String,
    account: Option<String>,
    project: Option<String>,
    model: Option<String>,
) -> DashboardView {
    let now = now_ms();
    // An account's activity is its own agent's (a Claude login: Claude Code). The only account
    // for a CLI gets all of it, as before; with several, each gets the sessions made under the
    // logins it has read (calls from before Tokenrunway noted the login can't be told apart).
    let (harnesses, logins, scope) = match account.as_deref().and_then(|id| record(&app, id).ok()) {
        Some(rec) => {
            let hs: Vec<String> = insights::subscription_harnesses(if rec.method == "cli" { &rec.provider } else { "" })
                .iter()
                .map(|h| h.to_string())
                .collect();
            let names = hs.iter().map(|h| tokenrunway_core::stats::harness_name(h)).collect::<Vec<_>>().join(" + ");
            let shared = !app.state::<AppState>().accounts.lock().unwrap().peers(&rec.id).is_empty();
            let first = || activity::stats(&app).and_then(|s| s.first_for_logins(&hs, &rec.logins));
            use chrono::TimeZone;
            let day = |t: i64| chrono::Local.timestamp_millis_opt(t).single().map(|d| d.format("%-d %b").to_string()).unwrap_or_default();
            let (logins, scope) = match (hs.is_empty(), shared) {
                (true, _) => (None, "No agent logs for this account".to_string()),
                (false, false) => (None, names.clone()),
                (false, true) => {
                    let scope = match first() {
                        Some(t) if t > period_start(&period, now) => format!("{names} · since {}", day(t)),
                        Some(_) => format!("{names} · this login"),
                        None => "No sessions from this login yet".to_string(),
                    };
                    (Some(rec.logins.clone()), scope)
                }
            };
            (Some(hs), logins, Some(scope))
        }
        None => (None, None, None),
    };
    let filter = Filter { project, model, harnesses, logins };
    let mut d = match activity::stats(&app) {
        Some(s) => {
            let (p, g) = (period.clone(), group.clone());
            let (mut d, sv) = tauri::async_runtime::spawn_blocking(move || {
                (s.dashboard_filtered(&p, &g, &filter, now), s.stats_filtered(&p, &filter, now))
            })
            .await
            .unwrap_or_default();
            d.billed = sv.spend_actual;
            d.billed_note = sv.actual_note;
            d.cache_share = (sv.tokens.total > 0).then(|| sv.tokens.cache as f64 / sv.tokens.total as f64 * 100.0);
            d
        }
        None => DashboardView { period: period.clone(), group, ..Default::default() },
    };
    let st = app.state::<AppState>();
    d.scope = scope;
    // The allowance parts follow the account filter too.
    let accounts: Vec<AccountView> = st.account_views(now).into_iter().filter(|a| account.as_ref().is_none_or(|id| *id == a.id)).collect();
    let since = period_start(&period, now);
    d.ran_out = Some(st.snaps.ran_out(since, account.as_deref()));
    d.ran_out_note = Some(match st.snaps.first_at() {
        Some(first) if first > since => format!("since {}", forecast::when(first, now)),
        Some(_) => "in range".into(),
        None => "no readings yet".into(),
    });
    let mut resets: Vec<ResetRow> = accounts
        .iter()
        .flat_map(|a| {
            a.windows.iter().filter_map(move |w| {
                Some(ResetRow {
                    account: a.name.clone(),
                    provider: a.provider.clone(),
                    window: w.label.clone(),
                    resets_at: w.resets_at.filter(|r| *r > now)?,
                    pct: w.pct,
                    status: w.status,
                })
            })
        })
        .collect();
    resets.sort_by_key(|r| r.resets_at);
    d.resets = resets;
    d.burndown = forecast::tightest(&accounts).and_then(|(a, w)| {
        let reset = w.resets_at?;
        let length = {
            let live = st.live.lock().unwrap();
            live.get(&a.id)?.last.as_ref()?.windows.iter().find(|r| r.id == w.id)?.length_ms?
        };
        let start = reset - length;
        let points = st
            .snaps
            .history(&a.id, &w.id, start)
            .into_iter()
            .filter(|r| r.resets_at.is_some_and(|x| (x - reset).abs() < 120_000))
            .map(|r| (r.at, r.used * 100.0))
            .collect();
        // The cone: ±20% of the runway left, until the hour-of-week profile can give a real spread.
        let cone = w.out_at.map(|o| {
            let m = (o - now).max(0) / 5;
            (o - m, (o + m).min(reset))
        });
        Some(Burndown { title: format!("{} · {}", a.name, w.label), start, reset, points, out_at: w.out_at, cone })
    });
    d
}

/// Where a period starts: local midnight today, N days back, or the beginning.
fn period_start(period: &str, now: i64) -> i64 {
    use chrono::{Local, TimeZone};
    let day = 24 * 3600 * 1000;
    match period {
        "today" => Local
            .timestamp_millis_opt(now)
            .single()
            .and_then(|t| t.date_naive().and_hms_opt(0, 0, 0))
            .and_then(|m| Local.from_local_datetime(&m).earliest())
            .map(|m| m.timestamp_millis())
            .unwrap_or(now - day),
        "lifetime" => 0,
        p => p.strip_suffix('d').and_then(|n| n.parse::<i64>().ok()).map(|n| now - n * day).unwrap_or(0),
    }
}

#[tauri::command]
pub fn list_aws_profiles(app: AppHandle) -> Vec<String> {
    allowance::bedrock::profiles(&app.state::<AppState>().ctx.home)
}

#[tauri::command]
pub fn list_detected(app: AppHandle) -> Vec<Detected> {
    let st = app.state::<AppState>();
    st.redetect();
    let d = st.detected.lock().unwrap().clone();
    d
}

fn local_part(email: &str) -> &str {
    email.split('@').next().unwrap_or(email)
}

fn unique_label(existing: &[AccountRecord], base: String) -> String {
    if !existing.iter().any(|a| a.label == base) {
        return base;
    }
    (2..).map(|n| format!("{base} {n}")).find(|l| !existing.iter().any(|a| &a.label == l)).unwrap_or(base)
}

/// The provider/method pairs this version can add.
fn can_add(provider: &str, method: &str) -> bool {
    matches!(
        (provider, method),
        ("anthropic", "cli")
            | ("openai", "cli")
            | ("bedrock", "aws")
            | ("anthropic-api", "key")
            | ("openai-api", "key")
            | ("copilot", "cli" | "key")
            | ("gemini", "cli")
            | ("cursor", "login" | "cli" | "key")
            | ("xai", "login" | "key")
            | ("openrouter", "key")
    )
}

/// "the Claude Code login", for "… is already added".
fn cli_name(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "Claude Code",
        "openai" => "Codex CLI",
        "copilot" => "GitHub CLI",
        "gemini" => "Gemini CLI",
        "cursor" => "Cursor app",
        _ => "CLI",
    }
}

/// What to say when a pasted key or cookie is refused on the first reading.
fn refused(provider: &str) -> &'static str {
    match provider {
        "copilot" => "GitHub refused that token. Check it has access to your account and paste it again.",
        "cursor" => "Cursor refused that cookie. Copy the WorkosCursorSessionToken value from cursor.com again.",
        "xai" => "xAI refused that API key. Check it on console.x.ai and paste it again.",
        "openrouter" => "OpenRouter refused that API key. Check it on openrouter.ai/settings/keys and paste it again.",
        _ => "That key was refused. Check it's an admin key for the organisation and paste it again.",
    }
}

/// Shows the popover again on the view it was on (after a login window).
fn show_popover(app: &AppHandle) {
    crate::open_view(app, "current");
}

#[tauri::command]
pub async fn add_account(
    app: AppHandle,
    provider: String,
    method: String,
    label: Option<String>,
    key: Option<String>,
    profile: Option<String>,
) -> Result<AccountView, String> {
    let st = app.state::<AppState>();
    let name = allowance::provider_name(&provider);
    if !can_add(&provider, &method) {
        return Err(format!("{name} can't be added that way yet."));
    }
    let existing = st.accounts.lock().unwrap().list.clone();
    let home = st.ctx.home.clone();
    let profile = if method == "aws" {
        let taken = |p: &String| existing.iter().any(|a| a.provider == "bedrock" && a.profile.as_ref() == Some(p));
        let p = profile
            .filter(|p| !p.trim().is_empty())
            .map(|p| p.trim().to_string())
            .or_else(|| allowance::bedrock::profiles(&home).into_iter().find(|p| !taken(p)))
            .ok_or("No AWS profile found in ~/.aws. Run `aws configure` or `aws configure sso` first.")?;
        if taken(&p) {
            return Err(format!("The AWS profile {p} is already added."));
        }
        Some(p)
    } else {
        None
    };
    if method == "cli" && existing.iter().any(|a| a.provider == provider && a.method == "cli") {
        return Err(format!("The {} login is already added.", cli_name(&provider)));
    }
    let key = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    let key = match (method.as_str(), provider.as_str(), key) {
        ("key", "cursor", Some(k)) => Some(allowance::cursor::normalize_pasted(&k)?),
        ("key", "cursor", None) => return Err("Paste the session cookie first.".into()),
        ("key", _, None) => return Err("Paste the key first.".into()),
        (_, _, k) => k,
    };
    let detail =
        |kind: &str| allowance::detect(&home, &[]).into_iter().find(|d| d.kind == kind).map(|d| d.detail).filter(|d| !d.is_empty());
    let default_label = match (provider.as_str(), method.as_str(), &profile) {
        ("anthropic", _, _) => match allowance::claude::email(&home) {
            Some(e) => format!("Claude — {}", local_part(&e)),
            None => "Claude".into(),
        },
        ("openai", _, _) => match allowance::codex::detect(&home).map(|d| d.detail).filter(|d| !d.is_empty()) {
            Some(e) => format!("ChatGPT — {}", local_part(&e)),
            None => "ChatGPT".into(),
        },
        ("bedrock", _, Some(p)) => format!("AWS Bedrock — {p}"),
        ("copilot", "cli", _) => detail("gh").map(|u| format!("Copilot — {u}")).unwrap_or_else(|| "Copilot".into()),
        ("gemini", _, _) => detail("gemini-cli").map(|e| format!("Gemini — {}", local_part(&e))).unwrap_or_else(|| "Gemini".into()),
        ("cursor", "cli", _) => detail("cursor-app").map(|e| format!("Cursor — {}", local_part(&e))).unwrap_or_else(|| "Cursor".into()),
        ("xai", "key", _) => "xAI API".into(),
        _ => name.to_string(),
    };
    let label = unique_label(&existing, label.map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).unwrap_or(default_label));
    let now = now_ms();
    let id = st.accounts.lock().unwrap().new_id(now);
    let rec = AccountRecord { id: id.clone(), provider, method: method.clone(), label, profile, created_at: now, ..Default::default() };
    if method == "login" {
        // Sign in first: the account is only saved once the session is in the keychain.
        let signed_in = crate::login::run(&app, &rec).await;
        show_popover(&app);
        if let Err(e) = signed_in {
            crate::login::forget(&app, &id);
            return Err(e);
        }
    }
    if let Some(k) = &key {
        secrets::set(&secrets::key_name(&id), k)?;
    }
    {
        let mut accts = st.accounts.lock().unwrap();
        accts.list.push(rec.clone());
        accts.save()?;
    }
    st.sched.lock().unwrap().entry(id.clone()).or_default().in_flight = true;
    crate::poll::poll_one(&app, &id).await;
    // A pasted key that's refused isn't kept.
    let refused_now = matches!(st.live.lock().unwrap().get(&id).and_then(|l| l.error.clone()), Some(ProviderError::AuthExpired(_)));
    if method == "key" && refused_now {
        remove_account(app.clone(), id.clone())?;
        return Err(refused(&rec.provider).into());
    }
    st.redetect();
    broadcast(&app);
    st.account_views(now_ms()).into_iter().find(|a| a.id == id).ok_or_else(|| "The account wasn't saved.".into())
}

#[tauri::command]
pub fn remove_account(app: AppHandle, account_id: String) -> Result<(), String> {
    let id = account_id;
    let st = app.state::<AppState>();
    let removed = {
        let mut accts = st.accounts.lock().unwrap();
        let r = accts.remove(&id);
        accts.save()?;
        r
    };
    if let Some(rec) = &removed {
        secrets::delete(&secrets::key_name(&id));
        if rec.method == "login" {
            crate::login::forget(&app, &id);
        }
        st.snaps.remove_account(&id);
        st.live.lock().unwrap().remove(&id);
        st.sched.lock().unwrap().remove(&id);
        st.settings.lock().unwrap().admins.remove(&id);
        st.save_settings();
    }
    st.redetect();
    broadcast(&app);
    Ok(())
}

#[tauri::command]
pub fn rename_account(app: AppHandle, account_id: String, name: String) -> Result<(), String> {
    let id = account_id;
    let name = name.trim();
    if name.is_empty() {
        return Err("The name can't be empty.".into());
    }
    {
        let st = app.state::<AppState>();
        let mut accts = st.accounts.lock().unwrap();
        accts.get_mut(&id).ok_or("No such account.")?.label = name.to_string();
        accts.save()?;
    }
    broadcast(&app);
    Ok(())
}

#[tauri::command]
pub fn refresh(app: AppHandle, account_id: Option<String>) {
    crate::poll::force(&app, account_id.as_deref());
}

/// "Log in again" / "I've run codex login": a CLI or AWS login is re-read straight away (the
/// CLI did the signing in); an in-app login opens its login window again; a refused key has to
/// be pasted again.
#[tauri::command]
pub async fn login_account(app: AppHandle, account_id: String) -> Result<AccountView, String> {
    let rec = record(&app, &account_id)?;
    match rec.method.as_str() {
        "key" => return Err("Remove this account and add it again with a new key.".into()),
        "login" => {
            crate::login::run(&app, &rec).await?;
            show_popover(&app);
        }
        _ => {}
    }
    let st = app.state::<AppState>();
    {
        let mut sched = st.sched.lock().unwrap();
        let s = sched.entry(account_id.clone()).or_default();
        if s.in_flight {
            drop(sched);
            crate::poll::force(&app, Some(&account_id));
            return st.account_views(now_ms()).into_iter().find(|a| a.id == account_id).ok_or_else(|| "No such account.".into());
        }
        s.in_flight = true;
        s.failures = 0;
        s.last_forced = now_ms();
    }
    crate::poll::poll_one(&app, &account_id).await;
    st.account_views(now_ms()).into_iter().find(|a| a.id == account_id).ok_or_else(|| "No such account.".into())
}

fn record(app: &AppHandle, id: &str) -> Result<AccountRecord, String> {
    app.state::<AppState>().accounts.lock().unwrap().get(id).cloned().ok_or_else(|| "No such account.".into())
}

#[tauri::command]
pub fn open_provider(app: AppHandle, account_id: String) -> Result<(), String> {
    let rec = record(&app, &account_id)?;
    let p = allowance::provider_for(&rec).ok_or("This account has no provider page.")?;
    app.opener().open_url(p.usage_url(&rec), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn request_draft(app: AppHandle, account_id: String, window_id: String) -> Result<RequestDraft, String> {
    let st = app.state::<AppState>();
    let rec = record(&app, &account_id)?;
    let now = now_ms();
    let a = st.account_views(now).into_iter().find(|a| a.id == account_id).ok_or("No such account.")?;
    let w = a.windows.iter().find(|w| w.id == window_id).ok_or("That window has no reading yet.")?;
    let s = st.settings.lock().unwrap().clone();
    let to = s.admins.get(&account_id).cloned().unwrap_or_default();
    let url = allowance::provider_for(&rec).and_then(|p| p.request_url(&rec));
    Ok(request::draft(&a, w, to, &s.my_name, url.as_deref(), now))
}

/// The address for a channel: `to` is either the text the user typed or the whole contact.
fn address(to: &serde_json::Value, channel: &str) -> String {
    match to {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(o) => {
            let key = match channel {
                "email" => "email",
                "slack" => "slack",
                "teams" => "teams",
                _ => "email",
            };
            o.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string()
        }
        _ => String::new(),
    }
}

#[tauri::command]
pub fn send_request(app: AppHandle, account_id: String, channel: String, to: serde_json::Value, body: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let st = app.state::<AppState>();
    let rec = record(&app, &account_id)?;
    let addr = address(&to, &channel).trim().to_string();
    let a = st.account_views(now_ms()).into_iter().find(|a| a.id == account_id).ok_or("No such account.")?;
    let open = |url: String| app.opener().open_url(url, None::<&str>).map_err(|e| format!("Couldn't open it: {e}"));
    match channel.as_str() {
        "email" => open(request::mailto(&addr, &request::subject(&a), &body))?,
        "slack" => {
            app.clipboard().write_text(body.clone()).map_err(|e| e.to_string())?;
            open(request::slack(&addr))?;
        }
        "teams" => open(request::teams(&addr, &body))?,
        "provider" => {
            let p = allowance::provider_for(&rec).ok_or("This account has no request page.")?;
            open(p.request_url(&rec).unwrap_or_else(|| p.usage_url(&rec)))?;
        }
        other => return Err(format!("Unknown channel {other}")),
    }
    // Remember who to ask next time, per channel.
    if !addr.is_empty() && channel != "provider" {
        let mut s = st.settings.lock().unwrap();
        let c = s.admins.entry(account_id.clone()).or_insert_with(AdminContact::default);
        match channel.as_str() {
            "email" => c.email = addr.clone(),
            "slack" => c.slack = addr.clone(),
            "teams" => c.teams = addr.clone(),
            _ => {}
        }
        if let Some(n) = to.get("name").and_then(|v| v.as_str()).filter(|n| !n.trim().is_empty()) {
            c.name = n.trim().to_string();
        }
    }
    st.save_settings();
    // Every request is logged locally: when, which account, which channel (not the message).
    let line = serde_json::json!({ "at": now_ms(), "accountId": account_id, "channel": channel, "to": addr }).to_string();
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(st.data_dir.join("requests.jsonl")) {
        let _ = writeln!(f, "{line}");
    }
    broadcast(&app);
    Ok(())
}

#[tauri::command]
pub fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    app.state::<AppState>().settings.lock().unwrap().clone()
}

/// Merges a partial settings object (camelCase keys) into the settings.
pub fn merge(s: &Settings, patch: &serde_json::Value) -> Result<Settings, String> {
    let mut v = serde_json::to_value(s).map_err(|e| e.to_string())?;
    let (Some(obj), Some(p)) = (v.as_object_mut(), patch.as_object()) else { return Err("Settings must be an object.".into()) };
    for (k, val) in p {
        if k == "rattleboard" {
            // Only these change here; the connection itself goes through rattleboard_connect.
            let (Some(cur), Some(inner)) = (obj.get_mut(k).and_then(|c| c.as_object_mut()), val.as_object()) else {
                return Err("rattleboard must be an object.".into());
            };
            for (ik, iv) in inner {
                if matches!(ik.as_str(), "alerts" | "card" | "target" | "unused") {
                    cur.insert(ik.clone(), iv.clone());
                }
            }
        } else if obj.contains_key(k) {
            obj.insert(k.clone(), val.clone());
        }
    }
    let mut out: Settings = serde_json::from_value(v).map_err(|e| format!("Invalid settings: {e}"))?;
    out.warn_at = out.warn_at.clamp(1.0, 100.0);
    out.crit_at = out.crit_at.clamp(out.warn_at, 100.0);
    out.insights.plan_prices.retain(|_, p| p.is_finite() && *p >= 0.0);
    if !matches!(out.rattleboard.alerts.as_str(), "off" | "alerts" | "warnings" | "all") {
        return Err(format!("Unknown alert level {}", out.rattleboard.alerts));
    }
    Ok(out)
}

#[tauri::command]
pub fn set_settings(app: AppHandle, patch: serde_json::Value) -> Result<Settings, String> {
    let st = app.state::<AppState>();
    let before = st.settings.lock().unwrap().clone();
    let next = merge(&before, &patch)?;
    if next.rattleboard.card && !before.rattleboard.card {
        crate::board::card_possible(&app, &next.rattleboard)?;
    }
    let after = {
        let mut s = st.settings.lock().unwrap();
        *s = next;
        s.clone()
    };
    st.save_settings();
    crate::board::settings_changed(&app, &before.rattleboard, &after.rattleboard);
    if before.start_at_login != after.start_at_login {
        use tauri_plugin_autostart::ManagerExt;
        let al = app.autolaunch();
        let r = if after.start_at_login { al.enable() } else { al.disable() };
        if let Err(e) = r {
            crate::debug(&format!("autostart: {e}"));
        }
    }
    if before.paused_until != after.paused_until {
        crate::tray::refresh_menu(&app);
    }
    if before.fetch_prices != after.fetch_prices {
        crate::prices::setting_changed(&app, after.fetch_prices);
    }
    broadcast(&app);
    Ok(after)
}

/// Pauses notifications for `minutes`; None = until resumed, 0 = resume.
#[tauri::command]
pub fn snooze(app: AppHandle, minutes: Option<i64>) {
    let until = match minutes {
        None => i64::MAX,
        Some(0) => 0,
        Some(m) => now_ms() + m.max(1) * 60_000,
    };
    let _ = set_settings(app, serde_json::json!({ "pausedUntil": until }));
}

/// What the dashboard should show when it opens: the popover's Top models and Top projects rows
/// open it filtered to that model or project.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DashTarget {
    pub period: Option<String>,
    pub project: Option<String>,
    pub model: Option<String>,
}

/// The target for a dashboard window that's still loading; it asks with `take_dashboard_target`.
static DASH_TARGET: std::sync::Mutex<Option<DashTarget>> = std::sync::Mutex::new(None);

#[tauri::command]
pub fn take_dashboard_target() -> Option<DashTarget> {
    DASH_TARGET.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Opens (or brings forward) the dashboard; with a `target`, switched to it (`tr://dash-target`
/// for an open one).
#[tauri::command]
pub fn open_dashboard(app: AppHandle, target: Option<DashTarget>) {
    if let Some(w) = app.get_webview_window("dashboard") {
        if let Some(t) = target {
            let _ = app.emit_to("dashboard", "tr://dash-target", t);
            if let Some(p) = app.get_webview_window("popover") {
                let _ = p.hide();
            }
        }
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    *DASH_TARGET.lock().unwrap_or_else(|e| e.into_inner()) = target;
    if let Some(p) = app.get_webview_window("popover") {
        let _ = p.hide();
    }
    let built = WebviewWindowBuilder::new(&app, "dashboard", WebviewUrl::App("dashboard.html".into()))
        .title("Tokenrunway")
        .inner_size(1100.0, 720.0)
        .min_inner_size(760.0, 480.0)
        .resizable(true)
        .decorations(true)
        .center()
        .focused(true)
        .build();
    if let Err(e) = built {
        crate::debug(&format!("dashboard window: {e}"));
    }
}

#[tauri::command]
pub fn open_settings(app: AppHandle) {
    crate::open_view(&app, "settings");
    let _ = app.emit("tr://show", "settings");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_patch() {
        let s = Settings::default();
        let out = merge(&s, &serde_json::json!({"menubarLabel": "percent", "warnAt": 60, "unknown": 1, "myName": "Sam"})).unwrap();
        assert_eq!(out.menubar_label, "percent");
        assert_eq!(out.warn_at, 60.0);
        assert_eq!(out.my_name, "Sam");
        assert_eq!(out.crit_at, 90.0);
        assert!(merge(&s, &serde_json::json!({"warnAt": "x"})).is_err());
        assert!(merge(&s, &serde_json::json!([1])).is_err());
    }

    #[test]
    fn insights_patch() {
        let s = Settings::default();
        assert!(s.insights.weekly_recap);
        let out = merge(&s, &serde_json::json!({"insights": {"weeklyRecap": false, "planPrices": {"a": 60, "b": -5}}})).unwrap();
        assert!(!out.insights.weekly_recap);
        assert_eq!(out.insights.plan_prices.get("a"), Some(&60.0));
        assert!(!out.insights.plan_prices.contains_key("b"), "negative prices are dropped");
    }

    #[test]
    fn rattleboard_patch() {
        let s = Settings::default();
        let out =
            merge(&s, &serde_json::json!({"rattleboard": {"alerts": "warnings", "card": true, "mode": "network", "url": "http://x"}}))
                .unwrap();
        assert_eq!(out.rattleboard.alerts, "warnings");
        assert!(out.rattleboard.card);
        assert_eq!(out.rattleboard.mode, "off", "the connection isn't changed here");
        assert_eq!(out.rattleboard.url, "");
        assert_eq!(out.rattleboard.rows, 6, "the rest is kept");
        assert!(merge(&s, &serde_json::json!({"rattleboard": {"alerts": "loud"}})).is_err());
        assert!(!merge(&s, &serde_json::json!({"rattleboard": {"unused": false}})).unwrap().rattleboard.unused);
        assert!(merge(&s, &serde_json::json!({"notifyUnused": true})).unwrap().notify_unused);
        assert!(merge(&s, &serde_json::json!({"rattleboard": 1})).is_err());
    }

    #[test]
    fn addresses() {
        let c = serde_json::json!({"name": "Jordan", "email": "jo@x.com", "slack": "@jo"});
        assert_eq!(address(&c, "email"), "jo@x.com");
        assert_eq!(address(&c, "slack"), "@jo");
        assert_eq!(address(&serde_json::json!("a@b"), "teams"), "a@b");
    }
}
