//! Cursor (SPEC.md §5.1, Observed): the cursor.com dashboard's own API, with the
//! `WorkosCursorSessionToken` session cookie.
//!
//! Verified (October 2026, against a real account with no usage yet):
//! - `GET https://cursor.com/api/usage-summary` → `billingCycleStart` / `billingCycleEnd` (ISO),
//!   `membershipType`, `limitType`, `isUnlimited`, `individualUsage.plan.{enabled, used, limit,
//!   remaining, breakdown, autoPercentUsed, apiPercentUsed, totalPercentUsed}` and
//!   `individualUsage.onDemand.{enabled, used, limit|null, remaining|null}`.
//! - `POST https://cursor.com/api/dashboard/get-plan-info` (`{}`) → `planInfo.planName`.
//! - `POST https://cursor.com/api/dashboard/get-current-period-usage` (`{}`) →
//!   `billingCycleStart` / `billingCycleEnd` (strings, epoch ms) and
//!   `planUsage.totalPercentUsed`; used here only if usage-summary loses its shape.
//!
//! Assumptions (the account had zero usage, so the units couldn't be seen moving):
//! `used` / `limit` are US cents of the plan's included amount, and the `*PercentUsed` fields
//! are 0–100. The cookie POSTs send `Origin` / `Referer: https://cursor.com`, as the dashboard
//! does.
//!
//! The cookie comes from (method) "login": the in-app login window, kept in the keychain;
//! "key": pasted by the user, kept in the keychain; "cli": the Cursor desktop app's own login
//! (`…/Cursor/User/globalStorage/state.vscdb`, ItemTable `cursorAuth/accessToken`), read-only on
//! each poll, as "<user id>%3A%3A<access token>" where the user id is the part of the token's
//! `sub` after "|". The app path is built from that description; no Cursor app was installed to
//! test against.

use super::{AccountRecord, BoxFuture, Ctx, Fetched, Money, Provider, ProviderError, Reading};
use crate::view::Detected;
use serde_json::Value;
use std::path::{Path, PathBuf};

const ORIGIN: &str = "https://cursor.com";
const HOST: &str = "cursor.com";
pub const COOKIE: &str = "WorkosCursorSessionToken";
const LOGIN_HINT: &str = "The Cursor session expired. Log in again.";
const APP_HINT: &str = "The Cursor app login expired. Open Cursor and sign in again.";
const KEY_HINT: &str = "Cursor refused the pasted session cookie. Remove this account and add it again with a new one.";

pub struct Cursor;

/// The Cursor app's state database.
pub fn state_db(home: &Path) -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else if cfg!(windows) {
        dirs::config_dir().unwrap_or_else(|| home.join("AppData/Roaming"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"))
    };
    base.join("Cursor/User/globalStorage/state.vscdb")
}

/// One value from the app's ItemTable (stored as text, sometimes JSON-quoted, sometimes a blob).
fn item(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    let v: rusqlite::types::Value = conn.query_row("SELECT value FROM ItemTable WHERE key = ?1", [key], |r| r.get(0)).ok()?;
    let s = match v {
        rusqlite::types::Value::Text(s) => s,
        rusqlite::types::Value::Blob(b) => String::from_utf8(b).ok()?,
        _ => return None,
    };
    let s = s.trim();
    let s = serde_json::from_str::<String>(s).unwrap_or_else(|_| s.to_string());
    (!s.is_empty()).then_some(s)
}

/// The Cursor app's login: the session cookie value, its expiry and the email.
pub struct AppLogin {
    cookie: String,
    pub expires_at: Option<i64>,
    pub email: Option<String>,
}

impl std::fmt::Debug for AppLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppLogin").field("expires_at", &self.expires_at).field("email", &self.email).finish_non_exhaustive()
    }
}

/// "<user id>%3A%3A<token>", the user id being the part of the JWT's `sub` after "|".
pub fn cookie_from_token(token: &str) -> Option<(String, Option<i64>)> {
    let claims = super::jwt_claims(token)?;
    let sub = claims.get("sub")?.as_str()?;
    let user = sub.rsplit('|').next().filter(|u| !u.is_empty())?;
    Some((format!("{user}%3A%3A{token}"), claims.get("exp").and_then(Value::as_i64).map(|s| s * 1000)))
}

pub fn read_app(db: &Path) -> Option<AppLogin> {
    if !db.is_file() {
        return None;
    }
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .ok()?;
    let token = item(&conn, "cursorAuth/accessToken")?;
    let (cookie, expires_at) = cookie_from_token(&token)?;
    Some(AppLogin { cookie, expires_at, email: item(&conn, "cursorAuth/cachedEmail") })
}

pub fn detect(home: &Path) -> Option<Detected> {
    let a = read_app(&state_db(home))?;
    Some(Detected {
        kind: "cursor-app".into(),
        label: "Cursor app login".into(),
        detail: a.email.unwrap_or_default(),
        provider: "cursor".into(),
        mono: "Cu".into(),
    })
}

/// A pasted cookie as the Cookie header value we keep: accepts the bare value, "name=value" or a
/// whole Cookie header, and the decoded "user::token" form.
pub fn normalize_pasted(s: &str) -> Result<String, String> {
    let s = s.trim().trim_start_matches("Cookie:").trim();
    let v = super::login::cookie_value(s, COOKIE).unwrap_or(s).trim().trim_matches('"');
    let v = v.replace("::", "%3A%3A");
    if v.len() < 20 || v.contains(char::is_whitespace) || v.contains(';') {
        return Err(format!("Paste the value of the {COOKIE} cookie from cursor.com."));
    }
    Ok(format!("{COOKIE}={v}"))
}

/// The Cookie header for an account.
async fn cookie(acct: &AccountRecord, home: &Path) -> Result<String, ProviderError> {
    match acct.method.as_str() {
        "cli" => {
            let db = state_db(home);
            let app = tokio::task::spawn_blocking(move || read_app(&db))
                .await
                .ok()
                .flatten()
                .ok_or_else(|| ProviderError::AuthExpired("No Cursor app login on this machine. Open Cursor and sign in.".into()))?;
            if app.expires_at.is_some_and(|e| e < crate::now_ms()) {
                return Err(ProviderError::AuthExpired(APP_HINT.into()));
            }
            Ok(format!("{COOKIE}={}", app.cookie))
        }
        "key" => super::secrets::get(&super::secrets::key_name(&acct.id))
            .ok_or_else(|| ProviderError::AuthExpired("The Cursor cookie is missing from the keychain. Add the account again.".into())),
        _ => super::secrets::get(&super::secrets::session_name(&acct.id)).ok_or_else(|| ProviderError::AuthExpired(LOGIN_HINT.into())),
    }
}

/// An ISO time, or epoch s/ms as a number or a string → ms.
fn ts_any(v: Option<&Value>) -> Option<i64> {
    let n = match v? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => match s.trim().parse::<f64>() {
            Ok(n) => n,
            Err(_) => return chrono::DateTime::parse_from_rfc3339(s.trim()).ok().map(|d| d.timestamp_millis()),
        },
        _ => return None,
    };
    Some(if n > 1e11 { n as i64 } else { (n * 1000.0) as i64 })
}

fn dollars(v: Option<&Value>) -> Option<f64> {
    v?.as_f64().map(|c| c / 100.0)
}

/// usage-summary → the windows and the plan.
pub fn parse_summary(v: &Value, now: i64) -> Result<(Vec<Reading>, Option<String>), ProviderError> {
    let iu = v.get("individualUsage").filter(|i| i.is_object()).ok_or_else(|| ProviderError::SchemaChanged("no individualUsage".into()))?;
    let (start, end) = (ts_any(v.get("billingCycleStart")), ts_any(v.get("billingCycleEnd")));
    let length_ms = match (start, end) {
        (Some(s), Some(e)) if e > s => Some(e - s),
        _ => None,
    };
    let plan = v.get("membershipType").and_then(Value::as_str).filter(|s| !s.is_empty()).map(super::title_case);
    let mut out = Vec::new();
    if let Some(p) = iu.get("plan").filter(|p| p.is_object() && p.get("enabled").and_then(Value::as_bool) != Some(false)) {
        let (used, limit) = (dollars(p.get("used")), dollars(p.get("limit")).filter(|l| *l > 0.0));
        let pct = p
            .get("totalPercentUsed")
            .and_then(Value::as_f64)
            .or_else(|| Some(used? / limit? * 100.0))
            .ok_or_else(|| ProviderError::SchemaChanged("plan without usage".into()))?;
        out.push(Reading {
            id: "monthly".into(),
            label: "Monthly plan".into(),
            used: pct / 100.0,
            abs: match (used, limit) {
                (Some(u), Some(l)) => Some(format!("{} / {}", super::usd(u), super::usd(l))),
                _ => None,
            },
            resets_at: end,
            length_ms,
            money: None,
            at: now,
        });
    }
    if let Some(d) = iu.get("onDemand").filter(|d| d.get("enabled").and_then(Value::as_bool) == Some(true)) {
        let used = dollars(d.get("used")).unwrap_or(0.0);
        let limit = dollars(d.get("limit")).filter(|l| *l > 0.0);
        out.push(Reading {
            id: "on-demand".into(),
            label: "On-demand".into(),
            used: limit.map(|l| used / l).unwrap_or(0.0),
            abs: Some(match limit {
                Some(l) => format!("{} / {}", super::usd(used), super::usd(l)),
                None => format!("{} spent", super::usd(used)),
            }),
            resets_at: end,
            length_ms,
            money: Some(Money { used, limit }),
            at: now,
        });
    }
    Ok((out, plan))
}

/// get-current-period-usage → the plan window (the fallback).
pub fn parse_period(v: &Value, now: i64) -> Result<Vec<Reading>, ProviderError> {
    let pct = v
        .pointer("/planUsage/totalPercentUsed")
        .and_then(Value::as_f64)
        .ok_or_else(|| ProviderError::SchemaChanged("no planUsage".into()))?;
    let (start, end) = (ts_any(v.get("billingCycleStart")), ts_any(v.get("billingCycleEnd")));
    Ok(vec![Reading {
        id: "monthly".into(),
        label: "Monthly plan".into(),
        used: pct / 100.0,
        abs: None,
        resets_at: end,
        length_ms: start.zip(end).map(|(s, e)| e - s).filter(|l| *l > 0),
        money: None,
        at: now,
    }])
}

async fn call(ctx: &Ctx, cookie: &str, path: &str, post: bool, hint: &str) -> Result<Value, ProviderError> {
    let url = format!("{ORIGIN}{path}");
    let req = if post { ctx.http.post(url).json(&serde_json::json!({})) } else { ctx.http.get(url) };
    let resp = req
        .header("Cookie", cookie)
        .header("Origin", ORIGIN)
        .header("Referer", format!("{ORIGIN}/dashboard"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| super::network_error(HOST, &e))?;
    if let Some(e) = ProviderError::from_status(resp.status(), super::retry_after(resp.headers()), HOST, hint) {
        return Err(e);
    }
    resp.json().await.map_err(|e| super::network_error(HOST, &e))
}

impl Provider for Cursor {
    fn id(&self) -> &'static str {
        "cursor"
    }

    fn fetch<'a>(&'a self, acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>> {
        Box::pin(async move {
            let hint = match acct.method.as_str() {
                "cli" => APP_HINT,
                "key" => KEY_HINT,
                _ => LOGIN_HINT,
            };
            let c = cookie(acct, &ctx.home).await?;
            let now = crate::now_ms();
            let summary = call(ctx, &c, "/api/usage-summary", false, hint).await?;
            let (windows, mut plan) = match parse_summary(&summary, now) {
                Ok(r) => r,
                Err(ProviderError::SchemaChanged(why)) => {
                    let p = call(ctx, &c, "/api/dashboard/get-current-period-usage", true, hint).await?;
                    (parse_period(&p, now).map_err(|_| ProviderError::SchemaChanged(why))?, None)
                }
                Err(e) => return Err(e),
            };
            // The plan's display name, when the dashboard gives it (best effort).
            if let Ok(info) = call(ctx, &c, "/api/dashboard/get-plan-info", true, hint).await {
                if let Some(n) = info.pointer("/planInfo/planName").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                    plan = Some(n.to_string());
                }
            }
            let who = if acct.method == "cli" {
                let db = state_db(&ctx.home);
                tokio::task::spawn_blocking(move || read_app(&db).and_then(|a| a.email)).await.ok().flatten()
            } else {
                None
            };
            let note = windows.is_empty().then(|| "Cursor reports no usage limits for this plan.".to_string());
            Ok(Fetched { plan, who, windows, caveat: None, note })
        })
    }

    fn usage_url(&self, _acct: &AccountRecord) -> String {
        "https://cursor.com/dashboard?tab=usage".into()
    }

    fn request_url(&self, acct: &AccountRecord) -> Option<String> {
        let p = acct.plan.to_ascii_lowercase();
        (p.contains("team") || p.contains("business") || p.contains("enterprise")).then(|| "https://cursor.com/dashboard?tab=usage".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape usage-summary answered with in October 2026 (a Pro account, nothing used yet).
    const SUMMARY: &str = r#"{"billingCycleStart": "2026-10-05T10:00:00.000Z", "billingCycleEnd": "2026-11-05T10:00:00.000Z",
        "membershipType": "pro", "limitType": "user", "isUnlimited": false,
        "autoModelSelectedDisplayMessage": "x", "namedModelSelectedDisplayMessage": "y",
        "individualUsage": {
          "plan": {"enabled": true, "used": 500, "limit": 2000, "remaining": 1500, "breakdown": {"included": 500, "bonus": 0, "total": 500},
                   "autoPercentUsed": 10, "apiPercentUsed": 15, "totalPercentUsed": 25},
          "onDemand": {"enabled": true, "used": 1240, "limit": null, "remaining": null}},
        "teamUsage": {}}"#;

    #[test]
    fn usage_summary() {
        let v: Value = serde_json::from_str(SUMMARY).unwrap();
        let (w, plan) = parse_summary(&v, 7).unwrap();
        assert_eq!(plan.as_deref(), Some("Pro"));
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].id.as_str(), w[0].label.as_str(), w[0].abs.as_deref()), ("monthly", "Monthly plan", Some("$5 / $20")));
        assert!((w[0].used - 0.25).abs() < 1e-9);
        let end = chrono::DateTime::parse_from_rfc3339("2026-11-05T10:00:00Z").unwrap().timestamp_millis();
        assert_eq!(w[0].resets_at, Some(end));
        assert_eq!(w[0].length_ms, Some(31 * super::super::DAY_MS));
        assert_eq!((w[1].id.as_str(), w[1].abs.as_deref()), ("on-demand", Some("$12.40 spent")));
        assert_eq!(w[1].money, Some(Money { used: 12.4, limit: None }));
        assert!(matches!(parse_summary(&serde_json::json!({"error": "x"}), 0), Err(ProviderError::SchemaChanged(_))));
    }

    #[test]
    fn current_period_fallback() {
        let v = serde_json::json!({"billingCycleStart": "1759658400000", "billingCycleEnd": "1762336800000",
            "planUsage": {"remainingBonus": false, "autoPercentUsed": 0, "apiPercentUsed": 0, "totalPercentUsed": 40}});
        let w = parse_period(&v, 0).unwrap();
        assert!((w[0].used - 0.4).abs() < 1e-9);
        assert_eq!(w[0].resets_at, Some(1_762_336_800_000));
        assert_eq!(w[0].length_ms, Some(1_762_336_800_000 - 1_759_658_400_000));
    }

    #[test]
    fn pasted_cookies() {
        let want = "WorkosCursorSessionToken=user_01ABC%3A%3AeyJhbGciOiJIUzI1NiJ9.e30.sig";
        assert_eq!(normalize_pasted("user_01ABC%3A%3AeyJhbGciOiJIUzI1NiJ9.e30.sig").unwrap(), want);
        assert_eq!(normalize_pasted("user_01ABC::eyJhbGciOiJIUzI1NiJ9.e30.sig").unwrap(), want);
        assert_eq!(
            normalize_pasted("Cookie: a=1; WorkosCursorSessionToken=user_01ABC%3A%3AeyJhbGciOiJIUzI1NiJ9.e30.sig; b=2").unwrap(),
            want
        );
        assert!(normalize_pasted("short").is_err());
    }

    fn jwt(claims: &str) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut s = String::new();
        for ch in claims.as_bytes().chunks(3) {
            let n = ch.iter().enumerate().fold(0u32, |acc, (i, &x)| acc | (x as u32) << (16 - 8 * i));
            for i in 0..=ch.len() {
                s.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            }
        }
        format!("eyJhbGciOiJIUzI1NiJ9.{s}.sig")
    }

    #[test]
    fn app_login_from_a_state_db() {
        let dir = std::env::temp_dir().join(format!("tokenrunway-cursor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("state.vscdb");
        let _ = std::fs::remove_file(&db);
        assert!(read_app(&db).is_none());
        let token = jwt(r#"{"sub":"auth0|user_01XYZ","exp":1800000000}"#);
        {
            let c = rusqlite::Connection::open(&db).unwrap();
            c.execute_batch("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)").unwrap();
            c.execute("INSERT INTO ItemTable VALUES ('cursorAuth/accessToken', ?1)", [&token]).unwrap();
            c.execute("INSERT INTO ItemTable VALUES ('cursorAuth/cachedEmail', '\"sam@example.com\"')", []).unwrap();
        }
        let a = read_app(&db).unwrap();
        assert_eq!(a.cookie, format!("user_01XYZ%3A%3A{token}"));
        assert_eq!(a.expires_at, Some(1_800_000_000_000));
        assert_eq!(a.email.as_deref(), Some("sam@example.com"));
        assert!(!format!("{a:?}").contains(&token), "the token isn't in Debug");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
