//! Gemini through the Gemini CLI login (SPEC.md §5.1, Observed, unverified here: built from the
//! Gemini CLI's own calls; no Gemini CLI login was available to test against).
//!
//! The CLI keeps its Google OAuth login in `~/.gemini/oauth_creds.json` (`access_token`,
//! `refresh_token`, `expiry_date` in ms, `id_token`). Tokenrunway reads the access token on each
//! poll and never refreshes it: refreshing would rotate the CLI's own tokens (the same rule as
//! Claude Code and Codex). When it has expired, running `gemini` refreshes it.
//!
//! Two calls to the Code Assist API the CLI uses (observed, unverified here):
//! - `POST https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist` with the CLI's
//!   metadata → `currentTier.{id, name}` and `cloudaicompanionProject` (the project quotas are
//!   counted against);
//! - `POST …/v1internal:retrieveUserQuota` with `{"project": …}` → `buckets[]` with `modelId`,
//!   `tokenType`, `remainingFraction` (0–1) and `resetTime` (RFC 3339), one per model, daily.

use super::{AccountRecord, BoxFuture, Ctx, Fetched, Provider, ProviderError, Reading, DAY_MS};
use crate::view::Detected;
use serde_json::Value;
use std::path::{Path, PathBuf};

const BASE: &str = "https://cloudcode-pa.googleapis.com/v1internal";
const HOST: &str = "cloudcode-pa.googleapis.com";
pub const LOGIN_HINT: &str = "The Gemini CLI login expired. Run `gemini` to sign in again.";

pub struct GeminiCli;

fn gemini_dir(home: &Path) -> PathBuf {
    home.join(".gemini")
}

/// What we use from oauth_creds.json. The token stays in here.
pub struct Creds {
    token: String,
    pub expires_at: Option<i64>,
    pub email: Option<String>,
}

impl std::fmt::Debug for Creds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Creds").field("expires_at", &self.expires_at).field("email", &self.email).finish_non_exhaustive()
    }
}

pub fn parse_creds(json: &str) -> Option<Creds> {
    let v: Value = serde_json::from_str(json).ok()?;
    let token = v.get("access_token")?.as_str().filter(|t| !t.is_empty())?.to_string();
    let email =
        v.get("id_token").and_then(Value::as_str).and_then(super::jwt_claims).and_then(|c| c.get("email")?.as_str().map(String::from));
    Some(Creds { token, expires_at: v.get("expiry_date").and_then(Value::as_i64), email })
}

fn read_creds(home: &Path) -> Option<Creds> {
    parse_creds(&std::fs::read_to_string(gemini_dir(home).join("oauth_creds.json")).ok()?)
}

/// The signed-in account: the id token's email, else `google_accounts.json`'s `active`.
fn email(home: &Path, creds: Option<&Creds>) -> Option<String> {
    creds.and_then(|c| c.email.clone()).or_else(|| {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(gemini_dir(home).join("google_accounts.json")).ok()?).ok()?;
        v.get("active")?.as_str().map(String::from)
    })
}

pub fn detect(home: &Path) -> Option<Detected> {
    let c = read_creds(home)?;
    Some(Detected {
        kind: "gemini-cli".into(),
        label: "Gemini CLI login".into(),
        detail: email(home, Some(&c)).unwrap_or_default(),
        provider: "gemini".into(),
        mono: "G".into(),
    })
}

/// The tier name and the project from loadCodeAssist.
pub fn parse_load(v: &Value) -> Result<(Option<String>, Option<String>), ProviderError> {
    if !v.is_object() {
        return Err(ProviderError::SchemaChanged("not an object".into()));
    }
    let tier = v.pointer("/currentTier/name").and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    let project = match v.get("cloudaicompanionProject") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Object(o)) => o.get("id").and_then(Value::as_str).map(String::from),
        _ => None,
    };
    Ok((tier, project))
}

/// "gemini-2.5-pro" → "Gemini 2.5 Pro"
fn model_name(id: &str) -> String {
    super::title_case(id.trim_end_matches("_vertex"))
}

/// The per-model buckets as daily windows.
pub fn parse_quota(v: &Value, now: i64) -> Result<Vec<Reading>, ProviderError> {
    let buckets = v.get("buckets").and_then(Value::as_array).ok_or_else(|| ProviderError::SchemaChanged("no buckets".into()))?;
    let mut out: Vec<Reading> = Vec::new();
    for b in buckets {
        let Some(model) = b.get("modelId").and_then(Value::as_str).filter(|m| !m.is_empty()) else { continue };
        let Some(left) = b.get("remainingFraction").and_then(Value::as_f64) else { continue };
        let kind = b.get("tokenType").and_then(Value::as_str).unwrap_or("REQUESTS");
        let (mut id, mut label) = (format!("{}-daily", model.to_ascii_lowercase()), format!("{} · daily", model_name(model)));
        if out.iter().any(|r| r.id == id) || kind != "REQUESTS" {
            id = format!("{}-{}-daily", model.to_ascii_lowercase(), kind.to_ascii_lowercase());
            label = format!("{} {} · daily", model_name(model), kind.to_ascii_lowercase());
        }
        let resets_at = b
            .get("resetTime")
            .and_then(Value::as_str)
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|d| d.timestamp_millis());
        out.push(Reading {
            id,
            label,
            used: (1.0 - left).clamp(0.0, 1.0),
            abs: None,
            resets_at,
            length_ms: Some(DAY_MS),
            money: None,
            at: now,
        });
    }
    Ok(out)
}

async fn post(ctx: &Ctx, token: &str, method: &str, body: Value) -> Result<Value, ProviderError> {
    let resp = ctx
        .http
        .post(format!("{BASE}:{method}"))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| super::network_error(HOST, &e))?;
    if let Some(e) = ProviderError::from_status(resp.status(), super::retry_after(resp.headers()), HOST, LOGIN_HINT) {
        return Err(e);
    }
    resp.json().await.map_err(|e| super::network_error(HOST, &e))
}

impl Provider for GeminiCli {
    fn id(&self) -> &'static str {
        "gemini-cli"
    }

    fn fetch<'a>(&'a self, _acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>> {
        Box::pin(async move {
            let Some(creds) = read_creds(&ctx.home) else {
                return Err(ProviderError::AuthExpired("No Gemini CLI login on this machine. Run `gemini` to sign in with Google.".into()));
            };
            let now = crate::now_ms();
            if creds.expires_at.is_some_and(|e| e < now) {
                return Err(ProviderError::AuthExpired(LOGIN_HINT.into()));
            }
            let meta = serde_json::json!({"ideType": "IDE_UNSPECIFIED", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"});
            let load = post(ctx, &creds.token, "loadCodeAssist", serde_json::json!({ "metadata": meta })).await?;
            let (tier, project) = parse_load(&load)?;
            let project = project.or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT").ok().filter(|p| !p.is_empty()));
            let body = match &project {
                Some(p) => serde_json::json!({ "project": p }),
                None => serde_json::json!({}),
            };
            let quota = post(ctx, &creds.token, "retrieveUserQuota", body).await?;
            let windows = parse_quota(&quota, now)?;
            let note = windows.is_empty().then(|| "Gemini reports no quotas for this login.".to_string());
            Ok(Fetched { plan: tier, who: email(&ctx.home, Some(&creds)), windows, caveat: None, note })
        })
    }

    fn usage_url(&self, _acct: &AccountRecord) -> String {
        "https://aistudio.google.com/usage".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creds_file() {
        let home = std::env::temp_dir().join(format!("tokenrunway-gemini-{}", std::process::id()));
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        assert!(detect(&home).is_none());
        std::fs::write(
            home.join(".gemini/oauth_creds.json"),
            r#"{"access_token": "ya29.secret", "refresh_token": "1//r", "scope": "x", "token_type": "Bearer", "expiry_date": 1800000000000}"#,
        )
        .unwrap();
        std::fs::write(home.join(".gemini/google_accounts.json"), r#"{"active": "sam@example.com", "old": []}"#).unwrap();
        let c = read_creds(&home).unwrap();
        assert_eq!(c.expires_at, Some(1_800_000_000_000));
        assert!(!format!("{c:?}").contains("ya29"), "the token isn't in Debug");
        let d = detect(&home).unwrap();
        assert_eq!((d.kind.as_str(), d.detail.as_str(), d.provider.as_str()), ("gemini-cli", "sam@example.com", "gemini"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn load_and_quota() {
        let load = serde_json::json!({"currentTier": {"id": "standard-tier", "name": "Gemini Code Assist"}, "cloudaicompanionProject": "proj-123"});
        assert_eq!(parse_load(&load).unwrap(), (Some("Gemini Code Assist".into()), Some("proj-123".into())));
        let load2 = serde_json::json!({"currentTier": {"id": "free-tier"}, "cloudaicompanionProject": {"id": "p2"}});
        assert_eq!(parse_load(&load2).unwrap(), (None, Some("p2".into())));
        let q = serde_json::json!({"buckets": [
            {"remainingFraction": 0.25, "resetTime": "2026-10-24T07:00:00Z", "tokenType": "REQUESTS", "modelId": "gemini-2.5-pro"},
            {"remainingFraction": 1, "resetTime": "2026-10-24T07:00:00Z", "tokenType": "REQUESTS", "modelId": "gemini-2.5-flash"},
            {"resetTime": "x", "modelId": "broken"}
        ]});
        let w = parse_quota(&q, 9).unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].id.as_str(), w[0].label.as_str()), ("gemini-2.5-pro-daily", "Gemini 2.5 Pro · daily"));
        assert!((w[0].used - 0.75).abs() < 1e-9);
        assert_eq!(w[0].length_ms, Some(DAY_MS));
        assert_eq!(w[0].resets_at, Some(chrono::DateTime::parse_from_rfc3339("2026-10-24T07:00:00Z").unwrap().timestamp_millis()));
        assert_eq!(w[1].used, 0.0);
        assert!(parse_quota(&serde_json::json!({"error": 1}), 0).is_err());
    }
}
