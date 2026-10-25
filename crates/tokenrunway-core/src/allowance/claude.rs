//! Claude (Pro/Max/Team/Enterprise) through the Claude Code login (SPEC.md §5.1, Observed).
//!
//! Reads Claude Code's OAuth access token, read-only, on each poll: from the macOS Keychain item
//! "Claude Code-credentials" (via /usr/bin/security; macOS asks once whether Tokenrunway may) or
//! `~/.claude/.credentials.json`. It's never logged, written or refreshed: when it has expired,
//! running `claude` refreshes it.
//!
//! `GET https://api.anthropic.com/api/oauth/usage` (what Claude Code's `/usage` shows) answers
//! with one object per window, e.g. `five_hour` / `seven_day` / `seven_day_opus` /
//! `seven_day_sonnet` (`utilization` 0–100, `resets_at` RFC 3339, null when the plan doesn't
//! have it), a `limits` list (`kind`, `group`, `percent`, `resets_at`, `scope.model.display_name`)
//! and `spend` / `extra_usage` for extra usage.

use super::{AccountRecord, BoxFuture, Ctx, Fetched, Money, Provider, ProviderError, Reading, DAY_MS, HOUR_MS};
use crate::view::Detected;
use serde_json::Value;
use std::path::Path;

const URL: &str = "https://api.anthropic.com/api/oauth/usage";
const HOST: &str = "api.anthropic.com";
pub const LOGIN_HINT: &str = "The Claude Code login expired. Run `claude` to sign in again.";

pub struct ClaudeCode;

/// What we use from Claude Code's credentials. The token never leaves this struct except into
/// the Authorization header.
pub struct Creds {
    token: String,
    pub expires_at: Option<i64>,
    pub subscription: Option<String>,
    pub tier: Option<String>,
}

impl std::fmt::Debug for Creds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Creds").field("expires_at", &self.expires_at).field("subscription", &self.subscription).finish_non_exhaustive()
    }
}

pub fn parse_creds(json: &str) -> Option<Creds> {
    let v: Value = serde_json::from_str(json).ok()?;
    let o = v.get("claudeAiOauth")?;
    let token = o.get("accessToken")?.as_str()?.to_string();
    if token.is_empty() {
        return None;
    }
    Some(Creds {
        token,
        expires_at: o.get("expiresAt").and_then(Value::as_i64),
        subscription: o.get("subscriptionType").and_then(Value::as_str).map(String::from),
        tier: o.get("rateLimitTier").and_then(Value::as_str).map(String::from),
    })
}

fn config_dir(home: &Path) -> std::path::PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(Into::into).unwrap_or_else(|| home.join(".claude"))
}

/// Reads the credentials: the Keychain on macOS, the credentials file elsewhere (and as a
/// fallback on macOS).
pub async fn read_creds(home: &Path) -> Option<Creds> {
    #[cfg(target_os = "macos")]
    {
        let out = tokio::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .kill_on_drop(true)
            .output();
        if let Ok(Ok(o)) = tokio::time::timeout(std::time::Duration::from_secs(60), out).await {
            if o.status.success() {
                if let Some(c) = parse_creds(String::from_utf8_lossy(&o.stdout).trim()) {
                    return Some(c);
                }
            }
        }
    }
    let raw = tokio::fs::read_to_string(config_dir(home).join(".credentials.json")).await.ok()?;
    parse_creds(&raw)
}

/// `oauthAccount` from `~/.claude.json`: the email and the plan hints, if Claude Code saved them.
fn oauth_account(home: &Path) -> Option<Value> {
    let p = std::env::var_os("CLAUDE_CONFIG_DIR").map(|d| Path::new(&d).join(".claude.json")).unwrap_or_else(|| home.join(".claude.json"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
    v.get("oauthAccount").cloned().filter(Value::is_object)
}

pub fn email(home: &Path) -> Option<String> {
    oauth_account(home)?.get("emailAddress")?.as_str().map(String::from)
}

/// "Max 20x", "Pro", "Enterprise", "Team"
pub fn plan_name(subscription: Option<&str>, tier: Option<&str>) -> Option<String> {
    let s = subscription?.to_ascii_lowercase();
    let base = match s.as_str() {
        "max" | "claude_max" => "Max",
        "pro" | "claude_pro" => "Pro",
        "team" | "claude_team" => "Team",
        "enterprise" | "claude_enterprise" => "Enterprise",
        "free" => "Free",
        other => return Some(super::title_case(other)),
    };
    let mult = tier.and_then(|t| t.rsplit('_').next()).filter(|m| m.ends_with('x') && m[..m.len() - 1].chars().all(|c| c.is_ascii_digit()));
    Some(match (base, mult) {
        ("Max", Some(m)) => format!("Max {m}"),
        _ => base.to_string(),
    })
}

pub fn detect(home: &Path) -> Option<Detected> {
    let acct = oauth_account(home);
    let file = config_dir(home).join(".credentials.json").is_file();
    if acct.is_none() && !file {
        return None;
    }
    Some(Detected {
        kind: "claude-code".into(),
        label: "Claude Code login".into(),
        detail: acct.and_then(|a| a.get("emailAddress").and_then(Value::as_str).map(String::from)).unwrap_or_default(),
        provider: "anthropic".into(),
        mono: "C".into(),
    })
}

fn ts(v: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(v.as_str()?).ok().map(|d| d.timestamp_millis())
}

/// "weekly-opus" from "Opus", "Weekly Opus"
fn slug(s: &str) -> String {
    s.to_ascii_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join("-")
}

/// Maps the usage response to windows. Unknown windows are skipped; if none of the known ones
/// are there at all, the reader is out of date.
pub fn parse_usage(v: &Value, at: i64) -> Result<Vec<Reading>, ProviderError> {
    if !v.is_object() {
        return Err(ProviderError::SchemaChanged("not an object".into()));
    }
    let mut out: Vec<Reading> = Vec::new();
    let known = [
        ("five_hour", "5h", "5-hour", 5 * HOUR_MS),
        ("seven_day", "weekly", "Weekly", 7 * DAY_MS),
        ("seven_day_opus", "weekly-opus", "Weekly Opus", 7 * DAY_MS),
        ("seven_day_sonnet", "weekly-sonnet", "Weekly Sonnet", 7 * DAY_MS),
    ];
    let mut recognised = false;
    for (key, id, label, len) in known {
        let Some(w) = v.get(key) else { continue };
        recognised = true;
        let Some(u) = w.get("utilization").and_then(Value::as_f64) else { continue };
        out.push(Reading {
            id: id.into(),
            label: label.into(),
            used: u / 100.0,
            abs: None,
            resets_at: w.get("resets_at").and_then(ts),
            length_ms: Some(len),
            money: None,
            at,
        });
    }
    // The newer list: per-model weekly limits that the named keys above don't cover.
    if let Some(limits) = v.get("limits").and_then(Value::as_array) {
        recognised = true;
        for l in limits {
            let kind = l.get("kind").and_then(Value::as_str).unwrap_or_default();
            let Some(pct) = l.get("percent").and_then(Value::as_f64) else { continue };
            let resets_at = l.get("resets_at").and_then(ts);
            let (id, label, len) = match kind {
                "session" => ("5h".to_string(), "5-hour".to_string(), 5 * HOUR_MS),
                "weekly_all" => ("weekly".to_string(), "Weekly".to_string(), 7 * DAY_MS),
                "weekly_scoped" => {
                    let Some(name) = l.pointer("/scope/model/display_name").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                        continue;
                    };
                    (format!("weekly-{}", slug(name)), format!("Weekly {name}"), 7 * DAY_MS)
                }
                _ => continue,
            };
            if out.iter().any(|r| r.id == id) {
                continue;
            }
            out.push(Reading { id, label, used: pct / 100.0, abs: None, resets_at, length_ms: Some(len), money: None, at });
        }
    }
    // Extra usage (spend beyond the plan), when it's on.
    if let Some(s) = v.get("spend").filter(|s| s.get("enabled").and_then(Value::as_bool) == Some(true)) {
        let amount = |m: Option<&Value>| -> Option<f64> {
            let m = m?;
            let minor = m.get("amount_minor")?.as_f64()?;
            let exp = m.get("exponent").and_then(Value::as_i64).unwrap_or(2) as i32;
            Some(minor / 10f64.powi(exp))
        };
        let used = amount(s.get("used")).unwrap_or(0.0);
        let limit = amount(s.get("limit")).or_else(|| amount(s.get("cap")));
        let pct = s.get("percent").and_then(Value::as_f64).or_else(|| limit.filter(|l| *l > 0.0).map(|l| used / l * 100.0)).unwrap_or(0.0);
        out.push(Reading {
            id: "extra".into(),
            label: "Extra usage".into(),
            used: pct / 100.0,
            abs: Some(match limit {
                Some(l) => format!("{} / {}", super::usd(used), super::usd(l)),
                None => super::usd(used),
            }),
            resets_at: None,
            length_ms: None,
            money: Some(Money { used, limit }),
            at,
        });
    }
    if !recognised {
        return Err(ProviderError::SchemaChanged("no known windows".into()));
    }
    Ok(out)
}

impl Provider for ClaudeCode {
    fn id(&self) -> &'static str {
        "anthropic-oauth"
    }

    fn fetch<'a>(&'a self, _acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>> {
        Box::pin(async move {
            let Some(creds) = read_creds(&ctx.home).await else {
                return Err(ProviderError::AuthExpired("No Claude Code login on this machine. Run `claude` to sign in.".into()));
            };
            let now = crate::now_ms();
            if creds.expires_at.is_some_and(|e| e < now) {
                return Err(ProviderError::AuthExpired(LOGIN_HINT.into()));
            }
            let resp = ctx
                .http
                .get(URL)
                .bearer_auth(&creds.token)
                .header("anthropic-beta", "oauth-2025-04-20")
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|e| super::network_error(HOST, &e))?;
            if let Some(e) = ProviderError::from_status(resp.status(), super::retry_after(resp.headers()), HOST, LOGIN_HINT) {
                return Err(e);
            }
            let v: Value = resp.json().await.map_err(|e| super::network_error(HOST, &e))?;
            let windows = parse_usage(&v, now)?;
            let acct = oauth_account(&ctx.home);
            Ok(Fetched {
                plan: plan_name(creds.subscription.as_deref(), creds.tier.as_deref()),
                who: acct.as_ref().and_then(|a| a.get("emailAddress")).and_then(Value::as_str).map(String::from),
                windows,
                caveat: None,
                note: None,
            })
        })
    }

    fn usage_url(&self, _acct: &AccountRecord) -> String {
        "https://claude.ai/settings/usage".into()
    }

    fn request_url(&self, acct: &AccountRecord) -> Option<String> {
        // Team and Enterprise seats ask their admin from the usage page.
        let p = acct.plan.to_ascii_lowercase();
        (p.contains("enterprise") || p.contains("team")).then(|| "https://claude.ai/settings/usage".into())
    }

    /// The usage endpoint rate-limits hard, per login, and Claude Code (on every computer
    /// signed in) uses it too: every 5 minutes at most.
    fn min_interval_ms(&self) -> i64 {
        5 * 60_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the endpoint answered with in October 2026 (values changed).
    const SAMPLE: &str = r#"{
      "five_hour": {"utilization": 78.0, "resets_at": "2026-10-23T04:59:59.660213+00:00", "limit_dollars": null},
      "seven_day": {"utilization": 64.0, "resets_at": "2026-10-27T10:59:59.660233+00:00"},
      "seven_day_oauth_apps": null, "seven_day_opus": null, "seven_day_sonnet": null,
      "extra_usage": {"is_enabled": false, "monthly_limit": null},
      "limits": [
        {"kind": "session", "group": "session", "percent": 78, "resets_at": "2026-10-23T04:59:59.660213+00:00", "scope": null},
        {"kind": "weekly_all", "group": "weekly", "percent": 64, "resets_at": "2026-10-27T10:59:59.660233+00:00", "scope": null},
        {"kind": "weekly_scoped", "group": "weekly", "percent": 83, "resets_at": "2026-10-27T11:00:00+00:00",
         "scope": {"model": {"id": null, "display_name": "Opus"}, "surface": null}}
      ],
      "spend": {"used": {"amount_minor": 1240, "currency": "USD", "exponent": 2}, "limit": {"amount_minor": 5000, "currency": "USD", "exponent": 2}, "percent": 24.8, "enabled": true}
    }"#;

    #[test]
    fn maps_windows() {
        let v: Value = serde_json::from_str(SAMPLE).unwrap();
        let w = parse_usage(&v, 1).unwrap();
        let ids: Vec<&str> = w.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["5h", "weekly", "weekly-opus", "extra"]);
        assert!((w[0].used - 0.78).abs() < 1e-9);
        assert_eq!(w[0].length_ms, Some(5 * HOUR_MS));
        assert_eq!(
            w[0].resets_at,
            Some(chrono::DateTime::parse_from_rfc3339("2026-10-23T04:59:59.660213+00:00").unwrap().timestamp_millis())
        );
        assert_eq!(w[2].label, "Weekly Opus");
        assert_eq!(w[3].abs.as_deref(), Some("$12.40 / $50"));
    }

    #[test]
    fn unknown_shape_is_schema_change() {
        let v: Value = serde_json::from_str(r#"{"something_else": 1}"#).unwrap();
        assert!(matches!(parse_usage(&v, 1), Err(ProviderError::SchemaChanged(_))));
    }

    #[test]
    fn creds_and_plan() {
        let c = parse_creds(
            r#"{"claudeAiOauth":{"accessToken":"t","expiresAt":5,"subscriptionType":"max","rateLimitTier":"default_claude_max_20x"}}"#,
        )
        .unwrap();
        assert_eq!(c.expires_at, Some(5));
        assert_eq!(plan_name(c.subscription.as_deref(), c.tier.as_deref()).as_deref(), Some("Max 20x"));
        assert_eq!(plan_name(Some("enterprise"), None).as_deref(), Some("Enterprise"));
        assert!(!format!("{c:?}").contains("\"t\""), "the token isn't in Debug");
        assert!(parse_creds(r#"{"other":{}}"#).is_none());
    }
}
