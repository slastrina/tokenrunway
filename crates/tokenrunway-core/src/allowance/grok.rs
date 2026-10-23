//! Grok / xAI (SPEC.md §5.1). **Unverified**: nobody was signed in to grok.com or had an xAI
//! key to test against, so both readers are built from what grok.com and the xAI API were seen
//! to answer elsewhere, and fail softly (SchemaChanged → "needs an update") if that's wrong.
//!
//! - Consumer plans (SuperGrok, X Premium), method "login": the grok.com session cookies (`sso`,
//!   `sso-rw`) from the in-app login window. `POST https://grok.com/rest/rate-limits` with
//!   `{"requestKind": "DEFAULT" | "REASONING" | "DEEPSEARCH", "modelName": "grok-4"}` →
//!   `windowSizeSeconds`, `remainingQueries`, `totalQueries`, (`remainingTokens`, `totalTokens`)
//!   and `waitTimeSeconds`. The reset time is only known once a window is used up (now + wait).
//! - The xAI API, method "key": an API key. `GET https://api.x.ai/v1/api-key` says whether the key
//!   is active and which team it's in. xAI has no documented spend endpoint for API keys yet, so
//!   the card shows the key's status and says spend isn't available.

use super::{AccountRecord, BoxFuture, Ctx, Fetched, Provider, ProviderError, Reading};
use serde_json::Value;

const RATE_LIMITS: &str = "https://grok.com/rest/rate-limits";
const GROK_HOST: &str = "grok.com";
const API_KEY: &str = "https://api.x.ai/v1/api-key";
const API_HOST: &str = "api.x.ai";
const LOGIN_HINT: &str = "The grok.com session expired. Log in again.";
const KEY_HINT: &str = "xAI refused the API key. Remove this account and add it again with a new key.";

/// The request kinds read, and their window names.
const KINDS: [(&str, &str, &str); 3] =
    [("DEFAULT", "queries", "Grok queries"), ("REASONING", "reasoning", "Grok reasoning"), ("DEEPSEARCH", "deepsearch", "DeepSearch")];

pub struct GrokWeb;
pub struct XaiApi;

/// One rate-limits answer as a window (None if it has no limit to show).
pub fn parse_limit(v: &Value, id: &str, label: &str, now: i64) -> Result<Option<Reading>, ProviderError> {
    if !v.is_object() {
        return Err(ProviderError::SchemaChanged("not an object".into()));
    }
    let num = |k: &str| v.get(k).and_then(Value::as_f64);
    let (left, total, unit) = match (num("remainingQueries"), num("totalQueries"), num("remainingTokens"), num("totalTokens")) {
        (Some(l), Some(t), _, _) if t > 0.0 => (l, t, "queries"),
        (_, _, Some(l), Some(t)) if t > 0.0 => (l, t, "tokens"),
        (None, None, None, None) => return Err(ProviderError::SchemaChanged("no limits".into())),
        _ => return Ok(None),
    };
    let used = (total - left.max(0.0)).max(0.0);
    let wait = num("waitTimeSeconds").filter(|w| *w > 0.0);
    Ok(Some(Reading {
        id: id.into(),
        label: label.into(),
        used: used / total,
        abs: Some(format!("{used:.0} / {total:.0} {unit}")),
        // The window rolls; its end is only known once it's used up.
        resets_at: wait.filter(|_| left <= 0.0).map(|w| now + (w * 1000.0) as i64),
        length_ms: num("windowSizeSeconds").filter(|s| *s > 0.0).map(|s| (s * 1000.0) as i64),
        money: None,
        at: now,
    }))
}

impl Provider for GrokWeb {
    fn id(&self) -> &'static str {
        "grok-web"
    }

    fn fetch<'a>(&'a self, acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>> {
        Box::pin(async move {
            let cookie = super::secrets::get(&super::secrets::session_name(&acct.id))
                .ok_or_else(|| ProviderError::AuthExpired(LOGIN_HINT.into()))?;
            let now = crate::now_ms();
            let mut windows = Vec::new();
            for (i, (kind, id, label)) in KINDS.iter().enumerate() {
                let resp = ctx
                    .http
                    .post(RATE_LIMITS)
                    .header("Cookie", &cookie)
                    .header("Origin", "https://grok.com")
                    .header("Referer", "https://grok.com/")
                    .header("Accept", "application/json")
                    .json(&serde_json::json!({ "requestKind": kind, "modelName": "grok-4" }))
                    .send()
                    .await
                    .map_err(|e| super::network_error(GROK_HOST, &e));
                // The first kind decides whether the session works; the others are extras.
                let resp = match resp {
                    Ok(r) => r,
                    Err(e) if i == 0 => return Err(e),
                    Err(_) => continue,
                };
                if let Some(e) = ProviderError::from_status(resp.status(), super::retry_after(resp.headers()), GROK_HOST, LOGIN_HINT) {
                    if i == 0 || matches!(e, ProviderError::RateLimited(_)) {
                        return Err(e);
                    }
                    continue;
                }
                let v: Value = match resp.json().await {
                    Ok(v) => v,
                    Err(e) if i == 0 => return Err(super::network_error(GROK_HOST, &e)),
                    Err(_) => continue,
                };
                match parse_limit(&v, id, label, now) {
                    Ok(Some(r)) => windows.push(r),
                    Ok(None) => {}
                    Err(e) if i == 0 => return Err(e),
                    Err(_) => {}
                }
            }
            let note = windows.is_empty().then(|| "grok.com reports no limits for this account.".to_string());
            Ok(Fetched { plan: None, who: None, windows, caveat: None, note })
        })
    }

    fn usage_url(&self, _acct: &AccountRecord) -> String {
        "https://grok.com/".into()
    }
}

/// The key's status → the card (no windows: there's no spend to read yet).
pub fn parse_key(v: &Value) -> Result<Fetched, ProviderError> {
    if !v.is_object() || (v.get("api_key_id").is_none() && v.get("team_id").is_none()) {
        return Err(ProviderError::SchemaChanged("no key info".into()));
    }
    let flag = |k: &str| v.get(k).and_then(Value::as_bool) == Some(true);
    if flag("api_key_blocked") || flag("api_key_disabled") || flag("team_blocked") {
        return Err(ProviderError::AuthExpired("This xAI API key is disabled or blocked. Check it on console.x.ai.".into()));
    }
    let name = v.get("name").and_then(Value::as_str).filter(|n| !n.is_empty());
    let key = name.map(|n| format!("Key “{n}”")).unwrap_or_else(|| "The key".into());
    Ok(Fetched {
        plan: Some("API".into()),
        who: None,
        windows: vec![],
        caveat: None,
        note: Some(format!("{key} is active. xAI has no spend endpoint Tokenrunway can read yet, so there's no usage bar.")),
    })
}

impl Provider for XaiApi {
    fn id(&self) -> &'static str {
        "xai-api"
    }

    fn fetch<'a>(&'a self, acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>> {
        Box::pin(async move {
            let key = super::secrets::get(&super::secrets::key_name(&acct.id))
                .ok_or_else(|| ProviderError::AuthExpired("The xAI key is missing from the keychain. Add the account again.".into()))?;
            let resp = ctx.http.get(API_KEY).bearer_auth(&key).send().await.map_err(|e| super::network_error(API_HOST, &e))?;
            if let Some(e) = ProviderError::from_status(resp.status(), super::retry_after(resp.headers()), API_HOST, KEY_HINT) {
                return Err(e);
            }
            let v: Value = resp.json().await.map_err(|e| super::network_error(API_HOST, &e))?;
            parse_key(&v)
        })
    }

    fn usage_url(&self, _acct: &AccountRecord) -> String {
        "https://console.x.ai/".into()
    }

    fn min_interval_ms(&self) -> i64 {
        15 * 60_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limits() {
        let v = serde_json::json!({"windowSizeSeconds": 7200, "remainingQueries": 12, "totalQueries": 30, "waitTimeSeconds": 0});
        let r = parse_limit(&v, "queries", "Grok queries", 1000).unwrap().unwrap();
        assert_eq!(r.abs.as_deref(), Some("18 / 30 queries"));
        assert!((r.used - 0.6).abs() < 1e-9);
        assert_eq!(r.length_ms, Some(7_200_000));
        assert_eq!(r.resets_at, None, "unknown until it's used up");
        let out = serde_json::json!({"windowSizeSeconds": 7200, "remainingQueries": 0, "totalQueries": 30, "waitTimeSeconds": 600});
        assert_eq!(parse_limit(&out, "queries", "Grok queries", 1000).unwrap().unwrap().resets_at, Some(1000 + 600_000));
        let tokens = serde_json::json!({"windowSizeSeconds": 3600, "remainingTokens": 50000, "totalTokens": 200000});
        assert_eq!(parse_limit(&tokens, "x", "X", 0).unwrap().unwrap().abs.as_deref(), Some("150000 / 200000 tokens"));
        assert!(parse_limit(&serde_json::json!({"error": "x"}), "x", "X", 0).is_err());
    }

    #[test]
    fn api_key_status() {
        let v = serde_json::json!({"redacted_api_key": "xai-…abcd", "user_id": "u", "name": "laptop", "team_id": "t", "api_key_id": "k",
            "acls": ["api-key:model:*"], "team_blocked": false, "api_key_blocked": false, "api_key_disabled": false});
        let f = parse_key(&v).unwrap();
        assert!(f.windows.is_empty());
        assert!(f.note.unwrap().starts_with("Key “laptop” is active."));
        let blocked = serde_json::json!({"api_key_id": "k", "api_key_disabled": true});
        assert!(matches!(parse_key(&blocked), Err(ProviderError::AuthExpired(_))));
        assert!(parse_key(&serde_json::json!({"code": 1})).is_err());
    }
}
