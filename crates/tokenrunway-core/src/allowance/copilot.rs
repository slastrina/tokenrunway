//! GitHub Copilot (SPEC.md §5.1, Observed): `GET https://api.github.com/copilot_internal/user`,
//! what the Copilot editor extensions call, with `Authorization: token <GitHub token>`.
//!
//! Verified (October 2026) for an account whose subscription had ended: `copilot_plan`
//! "individual", `access_type_sku` "subscription_ended" and no quota fields at all. The quota
//! shapes below are as documented by the extensions' own use, not verified here:
//! - paid plans: `quota_snapshots.{premium_interactions, chat, completions}` each with
//!   `entitlement`, `remaining`, `percent_remaining`, `unlimited`, `overage_count`,
//!   `overage_permitted`, and `quota_reset_date` ("YYYY-MM-DD");
//! - Copilot Free: `limited_user_quotas.{chat, completions}` (what's left), `monthly_quotas` (the
//!   allowance) and `limited_user_reset_date`.
//!
//! The token: method "cli" reads it on each poll from `gh auth token` (the GitHub CLI login),
//! else the Copilot editor login (`~/.config/github-copilot/apps.json` / `hosts.json`,
//! `oauth_token`), else `GH_TOKEN` / `GITHUB_TOKEN`. Method "key" is a pasted token in the
//! keychain. Read-only, never logged.

use super::{AccountRecord, BoxFuture, Ctx, Fetched, Provider, ProviderError, Reading, DAY_MS};
use crate::view::Detected;
use serde_json::Value;
use std::path::{Path, PathBuf};

const URL: &str = "https://api.github.com/copilot_internal/user";
const HOST: &str = "api.github.com";
pub const LOGIN_HINT: &str = "GitHub refused the GitHub CLI login. Run `gh auth login` to sign in again.";
const KEY_HINT: &str = "GitHub refused the pasted token. Remove this account and add it again with a new token.";
pub const ENDED: &str = "Your Copilot subscription has ended. Renew it on github.com/settings/copilot, then refresh.";

pub struct Copilot;

/// A GitHub token. Kept in here, never in Debug output.
pub struct Token(String);

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(…)")
    }
}

fn gh_config_dir(home: &Path) -> PathBuf {
    if let Some(d) = std::env::var_os("GH_CONFIG_DIR") {
        return d.into();
    }
    if cfg!(windows) {
        if let Some(d) = dirs::config_dir() {
            return d.join("GitHub CLI");
        }
    }
    home.join(".config/gh")
}

fn copilot_dir(home: &Path) -> PathBuf {
    if cfg!(windows) {
        if let Some(d) = dirs::data_local_dir() {
            return d.join("github-copilot");
        }
    }
    home.join(".config/github-copilot")
}

/// The github.com user in gh's hosts.yml ("github.com:\n    user: octocat"), if signed in.
pub fn gh_user(hosts_yml: &str) -> Option<String> {
    let mut in_github = false;
    for line in hosts_yml.lines() {
        if !line.starts_with(' ') && !line.starts_with('\t') {
            in_github = line.trim_end().trim_end_matches(':').trim() == "github.com";
            continue;
        }
        if in_github {
            if let Some(u) = line.trim().strip_prefix("user:") {
                let u = u.trim().trim_matches('"');
                if !u.is_empty() {
                    return Some(u.to_string());
                }
            }
        }
    }
    None
}

/// The Copilot editor login: `{"github.com:Iv1.…": {"user": "…", "oauth_token": "…"}}`.
pub fn editor_token(json: &str) -> Option<(Option<String>, String)> {
    let v: Value = serde_json::from_str(json).ok()?;
    v.as_object()?.iter().filter(|(k, _)| k.starts_with("github.com")).find_map(|(_, e)| {
        let t = e.get("oauth_token")?.as_str().filter(|t| !t.is_empty())?;
        Some((e.get("user").and_then(Value::as_str).map(String::from), t.to_string()))
    })
}

fn editor_login(home: &Path) -> Option<(Option<String>, String)> {
    ["apps.json", "hosts.json"].iter().find_map(|f| editor_token(&std::fs::read_to_string(copilot_dir(home).join(f)).ok()?))
}

/// `gh auth token`, if gh is installed and signed in.
async fn gh_token() -> Option<String> {
    let gh = super::find_program("gh")?;
    let mut cmd = tokio::process::Command::new(gh);
    cmd.args(["auth", "token", "--hostname", "github.com"]).kill_on_drop(true).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000);
    }
    let out = tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output()).await.ok()?.ok()?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !t.is_empty() && !t.contains(char::is_whitespace)).then_some(t)
}

/// The token for an account, from wherever its method says.
pub async fn token(acct: &AccountRecord, home: &Path) -> Result<Token, ProviderError> {
    if acct.method == "key" {
        return super::secrets::get(&super::secrets::key_name(&acct.id))
            .map(Token)
            .ok_or_else(|| ProviderError::AuthExpired("The GitHub token is missing from the keychain. Add the account again.".into()));
    }
    if let Some(t) = gh_token().await {
        return Ok(Token(t));
    }
    if let Some((_, t)) = editor_login(home) {
        return Ok(Token(t));
    }
    ["GH_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|t| !t.trim().is_empty()))
        .map(Token)
        .ok_or_else(|| ProviderError::AuthExpired("No GitHub login on this machine. Run `gh auth login` to sign in.".into()))
}

pub fn detect(home: &Path) -> Option<Detected> {
    let gh = super::find_program("gh")
        .and_then(|_| std::fs::read_to_string(gh_config_dir(home).join("hosts.yml")).ok())
        .and_then(|h| gh_user(&h))
        .map(|u| ("GitHub CLI login", u));
    let (label, user) = gh.or_else(|| editor_login(home).map(|(u, _)| ("Copilot editor login", u.unwrap_or_default())))?;
    Some(Detected { kind: "gh".into(), label: label.into(), detail: user, provider: "copilot".into(), mono: "P".into() })
}

/// "2026-11-01" or an RFC 3339 time → ms (a bare date is midnight UTC).
fn reset_ms(v: Option<&Value>) -> Option<i64> {
    let s = v?.as_str()?;
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(d.timestamp_millis());
    }
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis())
}

/// A month before the reset (the quotas are monthly).
fn month_before(reset: i64) -> i64 {
    use chrono::{Months, TimeZone, Utc};
    Utc.timestamp_millis_opt(reset)
        .single()
        .and_then(|t| t.checked_sub_months(Months::new(1)))
        .map(|t| t.timestamp_millis())
        .unwrap_or(reset - 30 * DAY_MS)
}

fn count(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{n:.0}")
    } else {
        format!("{n:.1}")
    }
}

fn window(id: &str, label: &str, used: f64, total: f64, reset: Option<i64>, now: i64) -> Reading {
    Reading {
        id: id.into(),
        label: label.into(),
        used: if total > 0.0 { used / total } else { 0.0 },
        abs: Some(format!("{} / {}", count(used), count(total))),
        resets_at: reset,
        length_ms: reset.map(|r| r - month_before(r)),
        money: None,
        at: now,
    }
}

/// Maps the answer to a reading (or the problem it shows).
pub fn parse(v: &Value, now: i64) -> Result<Fetched, ProviderError> {
    if !v.is_object() {
        return Err(ProviderError::SchemaChanged("not an object".into()));
    }
    let plan = v.get("copilot_plan").and_then(Value::as_str).map(super::title_case);
    let who = v.get("login").and_then(Value::as_str).map(String::from);
    let sku = v.get("access_type_sku").and_then(Value::as_str).unwrap_or_default();
    let mut windows = Vec::new();
    let mut unlimited = Vec::new();
    if let Some(q) = v.get("quota_snapshots").filter(|q| q.is_object()) {
        let reset = reset_ms(v.get("quota_reset_date_utc")).or_else(|| reset_ms(v.get("quota_reset_date")));
        for (key, id, label) in [
            ("premium_interactions", "premium", "Premium requests"),
            ("chat", "chat", "Chat"),
            ("completions", "completions", "Completions"),
        ] {
            let Some(s) = q.get(key).filter(|s| s.is_object()) else { continue };
            if s.get("unlimited").and_then(Value::as_bool) == Some(true) {
                unlimited.push(label.to_ascii_lowercase());
                continue;
            }
            let Some(total) = s.get("entitlement").and_then(Value::as_f64).filter(|t| *t > 0.0) else { continue };
            let used = match s.get("remaining").and_then(Value::as_f64) {
                Some(r) => total - r.max(0.0),
                None => s.get("percent_remaining").and_then(Value::as_f64).map(|p| total * (1.0 - p / 100.0)).unwrap_or(0.0),
            } + s.get("overage_count").and_then(Value::as_f64).unwrap_or(0.0);
            windows.push(window(id, label, used, total, reset, now));
        }
    }
    if let (Some(left), Some(total)) = (v.get("limited_user_quotas"), v.get("monthly_quotas")) {
        let reset = reset_ms(v.get("limited_user_reset_date"));
        for (key, id, label) in [("chat", "chat", "Chat"), ("completions", "completions", "Completions")] {
            if windows.iter().any(|w| w.id == id) {
                continue;
            }
            let (Some(l), Some(t)) = (left.get(key).and_then(Value::as_f64), total.get(key).and_then(Value::as_f64)) else { continue };
            if t > 0.0 {
                windows.push(window(id, label, t - l.max(0.0), t, reset, now));
            }
        }
    }
    if windows.is_empty() && unlimited.is_empty() {
        if sku.contains("ended") || sku.contains("expired") || sku.contains("cancel") {
            return Err(ProviderError::Unavailable(ENDED.into()));
        }
        if plan.is_none() {
            return Err(ProviderError::SchemaChanged("no plan or quotas".into()));
        }
    }
    let note = match (windows.is_empty(), unlimited.is_empty()) {
        (_, false) => Some(format!("Unlimited {} on this plan.", unlimited.join(" and "))),
        (true, true) => Some("Copilot reports no quotas for this plan.".into()),
        _ => None,
    };
    let plan = match plan.as_deref() {
        Some("Individual") if sku.contains("free") => Some("Free".into()),
        _ => plan,
    };
    Ok(Fetched { plan, who, windows, caveat: None, note })
}

impl Provider for Copilot {
    fn id(&self) -> &'static str {
        "copilot"
    }

    fn fetch<'a>(&'a self, acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>> {
        Box::pin(async move {
            let t = token(acct, &ctx.home).await?;
            let resp = ctx
                .http
                .get(URL)
                .header("Authorization", format!("token {}", t.0))
                .header("X-GitHub-Api-Version", "2025-04-01")
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|e| super::network_error(HOST, &e))?;
            let status = resp.status().as_u16();
            if status == 404 {
                return Err(ProviderError::Unavailable("This GitHub account has no Copilot access.".into()));
            }
            let hint = if acct.method == "key" { KEY_HINT } else { LOGIN_HINT };
            if let Some(e) = ProviderError::from_status(resp.status(), super::retry_after(resp.headers()), HOST, hint) {
                return Err(e);
            }
            let v: Value = resp.json().await.map_err(|e| super::network_error(HOST, &e))?;
            parse(&v, crate::now_ms())
        })
    }

    fn usage_url(&self, _acct: &AccountRecord) -> String {
        "https://github.com/settings/copilot".into()
    }

    fn request_url(&self, acct: &AccountRecord) -> Option<String> {
        let p = acct.plan.to_ascii_lowercase();
        (p.contains("business") || p.contains("enterprise")).then(|| "https://github.com/settings/copilot".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::AccountState;

    /// What the endpoint answered for this account in October 2026 (ids changed).
    const ENDED_SAMPLE: &str = r#"{"id": 1, "login": "someone", "access_type_sku": "subscription_ended", "assigned_date": null,
        "can_signup_for_limited": true, "chat_enabled": false, "copilot_plan": "individual", "enterprise_list": [],
        "organization_list": [], "endpoints": {"api": "https://api.individual.githubcopilot.com"}, "can_upgrade_plan": false}"#;

    #[test]
    fn ended_subscription_is_a_setup_problem() {
        let v: Value = serde_json::from_str(ENDED_SAMPLE).unwrap();
        let e = parse(&v, 0).unwrap_err();
        assert_eq!(e, ProviderError::Unavailable(ENDED.into()));
        assert_eq!(e.state(), AccountState::Broken, "quiet: no 'sign in again' notification");
        assert_eq!(e.cause(), "setup");
        assert!(e.problem("copilot").starts_with("Your Copilot subscription has ended"));
    }

    #[test]
    fn premium_requests() {
        let v = serde_json::json!({
            "login": "octo", "copilot_plan": "individual", "access_type_sku": "monthly_subscriber",
            "quota_reset_date": "2026-11-01",
            "quota_snapshots": {
                "chat": {"entitlement": 0, "remaining": 0, "percent_remaining": 100, "unlimited": true},
                "completions": {"entitlement": 0, "remaining": 0, "percent_remaining": 100, "unlimited": true},
                "premium_interactions": {"entitlement": 300, "remaining": 88, "percent_remaining": 29.3, "unlimited": false, "overage_count": 0, "overage_permitted": false}
            }
        });
        let f = parse(&v, 5).unwrap();
        assert_eq!(f.plan.as_deref(), Some("Individual"));
        assert_eq!(f.who.as_deref(), Some("octo"));
        assert_eq!(f.windows.len(), 1);
        let w = &f.windows[0];
        assert_eq!((w.id.as_str(), w.label.as_str(), w.abs.as_deref()), ("premium", "Premium requests", Some("212 / 300")));
        assert!((w.used - 212.0 / 300.0).abs() < 1e-9);
        let reset = chrono::DateTime::parse_from_rfc3339("2026-11-01T00:00:00Z").unwrap().timestamp_millis();
        assert_eq!(w.resets_at, Some(reset));
        assert_eq!(w.length_ms, Some(31 * DAY_MS), "October");
        assert_eq!(f.note.as_deref(), Some("Unlimited chat and completions on this plan."));
    }

    #[test]
    fn unlimited_only_is_a_note() {
        let v = serde_json::json!({"copilot_plan": "enterprise", "quota_snapshots": {
            "premium_interactions": {"entitlement": 0, "remaining": 0, "unlimited": true}}});
        let f = parse(&v, 0).unwrap();
        assert!(f.windows.is_empty());
        assert_eq!(f.note.as_deref(), Some("Unlimited premium requests on this plan."));
    }

    #[test]
    fn free_tier() {
        let v = serde_json::json!({"copilot_plan": "individual", "access_type_sku": "free_limited_copilot",
            "limited_user_quotas": {"chat": 40, "completions": 1500}, "monthly_quotas": {"chat": 50, "completions": 2000},
            "limited_user_reset_date": "2026-11-12"});
        let f = parse(&v, 0).unwrap();
        assert_eq!(f.plan.as_deref(), Some("Free"));
        let abs: Vec<_> = f.windows.iter().map(|w| (w.id.as_str(), w.abs.as_deref().unwrap_or(""))).collect();
        assert_eq!(abs, [("chat", "10 / 50"), ("completions", "500 / 2000")]);
        assert!(parse(&serde_json::json!({"message": "x"}), 0).is_err());
    }

    #[test]
    fn logins_on_disk() {
        let hosts = "github.com:\n    users:\n        octo:\n    git_protocol: https\n    user: octo\nghe.example.com:\n    user: other\n";
        assert_eq!(gh_user(hosts).as_deref(), Some("octo"));
        assert_eq!(gh_user("ghe.example.com:\n    user: x\n"), None);
        let apps =
            r#"{"github.com:Iv1.b507a08c87ecfe98": {"user": "octo", "oauth_token": "gho_x", "githubAppId": "Iv1.b507a08c87ecfe98"}}"#;
        assert_eq!(editor_token(apps), Some((Some("octo".into()), "gho_x".into())));
        assert_eq!(editor_token(r#"{"ghe.example.com": {"oauth_token": "x"}}"#), None);
        assert_eq!(format!("{:?}", Token("gho_secret".into())), "Token(…)");
    }
}
