//! ChatGPT / Codex through the Codex CLI login (SPEC.md §5.1).
//!
//! Two sources, the remote one first:
//! - remote (Observed): `GET https://chatgpt.com/backend-api/wham/usage` with the access token
//!   and account id from `~/.codex/auth.json` (read-only; the Codex CLI refreshes it when it
//!   runs). Answers `plan_type`, `rate_limit.{primary_window, secondary_window}` (`used_percent`,
//!   `limit_window_seconds`, `reset_after_seconds`, `reset_at` in s) and `credits`.
//! - local: the newest `rate_limits` on a `token_count` event in
//!   `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` (`primary` / `secondary` with
//!   `used_percent`, `window_minutes`, `resets_at` in s or `resets_in_seconds`).
//!
//! The plan comes from the id token's claims (decoded, not verified: it's only shown).

use super::{AccountRecord, BoxFuture, Ctx, Fetched, Provider, ProviderError, Reading, MIN_MS};
use crate::view::Detected;
use serde_json::Value;
use std::path::{Path, PathBuf};

const URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const HOST: &str = "chatgpt.com";
pub const LOGIN_HINT: &str = "The Codex CLI session expired. Run `codex login` to sign in again.";

pub struct CodexCli;

fn codex_home(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME").map(Into::into).unwrap_or_else(|| home.join(".codex"))
}

/// What we use from auth.json. The token stays in here.
pub struct Auth {
    token: String,
    account_id: Option<String>,
    pub expires_at: Option<i64>,
    pub plan: Option<String>,
    pub email: Option<String>,
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Auth").field("expires_at", &self.expires_at).field("plan", &self.plan).finish_non_exhaustive()
    }
}

pub fn parse_auth(json: &str) -> Option<Auth> {
    let v: Value = serde_json::from_str(json).ok()?;
    let t = v.get("tokens")?;
    let token = t.get("access_token")?.as_str()?.to_string();
    let id = t.get("id_token").and_then(Value::as_str).and_then(super::jwt_claims);
    let access = super::jwt_claims(&token);
    let auth_claim =
        |c: &Option<Value>, k: &str| c.as_ref().and_then(|c| c.pointer(&format!("/https:~1~1api.openai.com~1auth/{k}"))).cloned();
    Some(Auth {
        account_id: t
            .get("account_id")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| auth_claim(&id, "chatgpt_account_id").and_then(|v| v.as_str().map(String::from))),
        expires_at: access.as_ref().and_then(|c| c.get("exp")).and_then(Value::as_i64).map(|s| s * 1000),
        plan: auth_claim(&id, "chatgpt_plan_type")
            .or_else(|| auth_claim(&access, "chatgpt_plan_type"))
            .and_then(|v| v.as_str().map(super::title_case)),
        email: id.as_ref().and_then(|c| c.get("email")).and_then(Value::as_str).map(String::from),
        token,
    })
}

fn read_auth(home: &Path) -> Option<Auth> {
    parse_auth(&std::fs::read_to_string(codex_home(home).join("auth.json")).ok()?)
}

pub fn detect(home: &Path) -> Option<Detected> {
    let a = read_auth(home)?;
    Some(Detected {
        kind: "codex".into(),
        label: "Codex CLI login".into(),
        detail: a.email.unwrap_or_default(),
        provider: "openai".into(),
        mono: "O".into(),
    })
}

/// ("5h", "Codex 5-hour") for 300 minutes, ("weekly", "Codex weekly") for a week…
fn window_name(minutes: Option<i64>, fallback: &str) -> (String, String) {
    match minutes {
        Some(m) if (290..=310).contains(&m) => ("5h".into(), "Codex 5-hour".into()),
        Some(m) if (10_000..=10_200).contains(&m) => ("weekly".into(), "Codex weekly".into()),
        Some(m) if (40_000..=45_000).contains(&m) => ("monthly".into(), "Codex monthly".into()),
        Some(m) if m >= 60 && m % 60 == 0 => (format!("{}h", m / 60), format!("Codex {}-hour", m / 60)),
        Some(m) => (format!("{m}m"), format!("Codex {m}-minute")),
        None => (fallback.into(), format!("Codex {fallback}")),
    }
}

/// One window from either source: percent, length in minutes, reset in ms.
fn reading(used_percent: f64, minutes: Option<i64>, resets_at: Option<i64>, fallback: &str, at: i64) -> Reading {
    let (id, label) = window_name(minutes, fallback);
    Reading { id, label, used: used_percent / 100.0, abs: None, resets_at, length_ms: minutes.map(|m| m * MIN_MS), money: None, at }
}

/// The remote answer.
pub fn parse_remote(v: &Value, now: i64) -> Result<(Vec<Reading>, Option<String>), ProviderError> {
    if !v.is_object() {
        return Err(ProviderError::SchemaChanged("not an object".into()));
    }
    let plan = v.get("plan_type").and_then(Value::as_str).map(super::title_case);
    let Some(rl) = v.get("rate_limit") else {
        return Err(ProviderError::SchemaChanged("no rate_limit".into()));
    };
    let mut out = Vec::new();
    for (key, fallback) in [("primary_window", "primary"), ("secondary_window", "secondary")] {
        let Some(w) = rl.get(key).filter(|w| w.is_object()) else { continue };
        let Some(pct) = w.get("used_percent").and_then(Value::as_f64) else { continue };
        let minutes = w.get("limit_window_seconds").and_then(Value::as_i64).map(|s| s / 60);
        let resets_at = w
            .get("reset_at")
            .and_then(Value::as_i64)
            .map(|s| s * 1000)
            .or_else(|| w.get("reset_after_seconds").and_then(Value::as_i64).map(|s| now + s * 1000));
        out.push(reading(pct, minutes, resets_at, fallback, now));
    }
    Ok((out, plan))
}

/// The windows in one `rate_limits` object of a rollout event at `at` (ms). Windows that have
/// already reset are dropped: what's used since is unknown.
pub fn parse_local(rl: &Value, at: i64, now: i64) -> Vec<Reading> {
    let mut out = Vec::new();
    for key in ["primary", "secondary"] {
        let Some(w) = rl.get(key).filter(|w| w.is_object()) else { continue };
        let Some(pct) = w.get("used_percent").and_then(Value::as_f64) else { continue };
        let minutes = w.get("window_minutes").and_then(Value::as_i64);
        let resets_at = w
            .get("resets_at")
            .and_then(Value::as_i64)
            .map(|s| s * 1000)
            .or_else(|| w.get("resets_in_seconds").and_then(Value::as_i64).map(|s| at + s * 1000));
        if resets_at.is_some_and(|r| r <= now) {
            continue;
        }
        out.push(reading(pct, minutes, resets_at, key, at));
    }
    out
}

/// The newest rollout files, newest day first (sessions/YYYY/MM/DD), at most `n`.
fn newest_rollouts(home: &Path, n: usize) -> Vec<PathBuf> {
    fn sorted_dirs(p: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        v.sort();
        v.reverse();
        v
    }
    let mut out = Vec::new();
    for y in sorted_dirs(&codex_home(home).join("sessions")) {
        for m in sorted_dirs(&y) {
            for d in sorted_dirs(&m) {
                let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&d)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.file_name().and_then(|f| f.to_str()).is_some_and(|f| f.starts_with("rollout-") && f.ends_with(".jsonl")))
                    .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
                    .collect();
                files.sort_by_key(|f| std::cmp::Reverse(f.0));
                out.extend(files.into_iter().map(|f| f.1));
                if out.len() >= n {
                    out.truncate(n);
                    return out;
                }
            }
        }
    }
    out
}

/// The newest rate limits Codex wrote locally, with when it wrote them.
pub fn local_limits(home: &Path, now: i64) -> Option<(i64, Vec<Reading>)> {
    for path in newest_rollouts(home, 5) {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for line in text.lines().rev() {
            if !line.contains("\"rate_limits\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let payload = v.get("payload").unwrap_or(&v);
            if payload.get("type").and_then(Value::as_str) != Some("token_count") {
                continue;
            }
            let Some(rl) = payload.get("rate_limits").filter(|r| r.is_object()) else { continue };
            let at = v
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|d| d.timestamp_millis())
                .unwrap_or(now);
            let windows = parse_local(rl, at, now);
            if !windows.is_empty() {
                return Some((at, windows));
            }
            // The newest event says this plan has no windows: no point reading older ones.
            return None;
        }
    }
    None
}

async fn remote(auth: &Auth, ctx: &Ctx) -> Result<(Vec<Reading>, Option<String>), ProviderError> {
    let now = crate::now_ms();
    if auth.expires_at.is_some_and(|e| e < now) {
        return Err(ProviderError::AuthExpired(LOGIN_HINT.into()));
    }
    let mut req = ctx.http.get(URL).bearer_auth(&auth.token).header("Accept", "application/json");
    if let Some(id) = &auth.account_id {
        req = req.header("ChatGPT-Account-Id", id);
    }
    let resp = req.send().await.map_err(|e| super::network_error(HOST, &e))?;
    if let Some(e) = ProviderError::from_status(resp.status(), super::retry_after(resp.headers()), HOST, LOGIN_HINT) {
        return Err(e);
    }
    let v: Value = resp.json().await.map_err(|e| super::network_error(HOST, &e))?;
    parse_remote(&v, now)
}

impl Provider for CodexCli {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn fetch<'a>(&'a self, _acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>> {
        Box::pin(async move {
            let now = crate::now_ms();
            let auth = read_auth(&ctx.home);
            let remote = match &auth {
                Some(a) => remote(a, ctx).await,
                None => Err(ProviderError::AuthExpired("No Codex CLI login on this machine. Run `codex login` to sign in.".into())),
            };
            let plan_claim = auth.as_ref().and_then(|a| a.plan.clone());
            let who = auth.as_ref().and_then(|a| a.email.clone());
            match remote {
                Ok((windows, plan)) => Ok(Fetched { plan: plan.or(plan_claim), who, windows, caveat: None, note: None }),
                Err(e) => {
                    let home = ctx.home.clone();
                    let local = tokio::task::spawn_blocking(move || local_limits(&home, now)).await.ok().flatten();
                    match local {
                        Some((_, windows)) => Ok(Fetched { plan: plan_claim, who, windows, caveat: Some(e), note: None }),
                        None => Err(e),
                    }
                }
            }
        })
    }

    fn usage_url(&self, _acct: &AccountRecord) -> String {
        "https://chatgpt.com/codex/settings/usage".into()
    }

    fn request_url(&self, acct: &AccountRecord) -> Option<String> {
        let p = acct.plan.to_ascii_lowercase();
        (p.contains("business") || p.contains("enterprise") || p.contains("team") || p.contains("edu"))
            .then(|| "https://chatgpt.com/codex/settings/usage".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_shape() {
        let v: Value = serde_json::from_str(
            r#"{"plan_type":"pro","rate_limit":{"allowed":true,"limit_reached":false,
                "primary_window":{"used_percent":12,"limit_window_seconds":18000,"reset_after_seconds":600,"reset_at":1800000000},
                "secondary_window":{"used_percent":31,"limit_window_seconds":604800,"reset_after_seconds":6000,"reset_at":1800500000}},
                "credits":{"has_credits":false,"unlimited":false,"balance":null}}"#,
        )
        .unwrap();
        let (w, plan) = parse_remote(&v, 0).unwrap();
        assert_eq!(plan.as_deref(), Some("Pro"));
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].id.as_str(), w[0].label.as_str()), ("5h", "Codex 5-hour"));
        assert_eq!((w[1].id.as_str(), w[1].label.as_str()), ("weekly", "Codex weekly"));
        assert_eq!(w[0].resets_at, Some(1_800_000_000_000));
        assert!((w[1].used - 0.31).abs() < 1e-9);
        assert!(parse_remote(&serde_json::json!({"x":1}), 0).is_err());
    }

    #[test]
    fn local_shape_and_expired_windows() {
        let rl = serde_json::json!({
            "primary": {"used_percent": 40.0, "window_minutes": 300, "resets_in_seconds": 3600},
            "secondary": {"used_percent": 10.0, "window_minutes": 10080, "resets_at": 1}
        });
        let w = parse_local(&rl, 1_000_000, 1_000_000);
        assert_eq!(w.len(), 1, "the secondary has already reset");
        assert_eq!(w[0].resets_at, Some(1_000_000 + 3_600_000));
        let none = serde_json::json!({"primary": null, "secondary": null, "credits": {"has_credits": true}});
        assert!(parse_local(&none, 0, 0).is_empty());
    }

    #[test]
    fn rollout_files() {
        let home = std::env::temp_dir().join(format!("tokenrunway-codex-{}", std::process::id()));
        let day = home.join(".codex/sessions/2026/10/23");
        std::fs::create_dir_all(&day).unwrap();
        let now = crate::now_ms();
        let reset = now / 1000 + 3600;
        let line = format!(
            r#"{{"timestamp":"2026-10-23T01:00:00Z","type":"event_msg","payload":{{"type":"token_count","info":null,"rate_limits":{{"primary":{{"used_percent":55.0,"window_minutes":300,"resets_at":{reset}}},"secondary":null}}}}}}"#
        );
        std::fs::write(day.join("rollout-a.jsonl"), format!("{{\"type\":\"session_meta\"}}\n{line}\n")).unwrap();
        // CODEX_HOME may be set on a dev machine; only the default path is tested here.
        if std::env::var_os("CODEX_HOME").is_none() {
            let (_, w) = local_limits(&home, now).unwrap();
            assert_eq!(w[0].id, "5h");
            assert!((w[0].used - 0.55).abs() < 1e-9);
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn auth_claims() {
        // header.claims.sig, with claims {"exp": 10, "https://api.openai.com/auth": {"chatgpt_plan_type": "business"}, "email": "x@y"}
        let claims =
            serde_json::json!({"exp": 10, "email": "x@y", "https://api.openai.com/auth": {"chatgpt_plan_type": "business"}}).to_string();
        let enc = b64(claims.as_bytes());
        let json = format!(r#"{{"tokens":{{"access_token":"a.{enc}.s","id_token":"h.{enc}.s","account_id":"acc"}}}}"#);
        let a = parse_auth(&json).unwrap();
        assert_eq!(a.plan.as_deref(), Some("Business"));
        assert_eq!(a.email.as_deref(), Some("x@y"));
        assert_eq!(a.expires_at, Some(10_000));
        assert!(!format!("{a:?}").contains(&enc));
    }

    fn b64(b: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut s = String::new();
        for ch in b.chunks(3) {
            let n = ch.iter().enumerate().fold(0u32, |acc, (i, &x)| acc | (x as u32) << (16 - 8 * i));
            for i in 0..=ch.len() {
                s.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            }
        }
        s
    }
}
