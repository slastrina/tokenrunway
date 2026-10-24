//! OpenRouter (SPEC.md §5.1), method "key": an OpenRouter API key, kept in the keychain.
//!
//! - `GET https://openrouter.ai/api/v1/key` (any key) → `data`: the key's own `limit`,
//!   `limit_remaining` and `limit_reset` ("daily" | "weekly" | "monthly" | null), the budgets
//!   enforced on it (`effective_budget`: `limit_usd`, `spend_usd`, `reset_interval`, `resets_at`),
//!   its spend (`usage`, `usage_daily`, `usage_weekly`, `usage_monthly`, UTC periods with weeks
//!   from Monday), `is_free_tier` and `free_model_daily_requests` (`used`, `limit`).
//! - `GET https://openrouter.ai/api/v1/credits` → `data.total_credits`, `data.total_usage`: the
//!   account's balance. Only management keys may read it (403 otherwise), so it's an extra.
//!
//! Windows: the limit or budget on the key (with its reset), the account's credit balance when
//! the key can read it, and free-model requests on the free tier. The card's note gives the key's
//! spend today, this week and this month.

use super::{AccountRecord, BoxFuture, Ctx, Fetched, Provider, ProviderError, Reading, DAY_MS};
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use serde_json::Value;

const KEY_URL: &str = "https://openrouter.ai/api/v1/key";
const CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";
const HOST: &str = "openrouter.ai";
const KEY_HINT: &str = "OpenRouter refused the API key. Remove this account and add it again with a new key.";

pub struct OpenRouter;

fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

/// The next start of a UTC day, week (Monday) or month after `now`, and that period's length.
fn next_reset(interval: &str, now: i64) -> Option<(i64, i64)> {
    let t = Utc.timestamp_millis_opt(now).single()?;
    let day = Utc.with_ymd_and_hms(t.year(), t.month(), t.day(), 0, 0, 0).single()?;
    let (start, end) = match interval {
        "daily" => (day, day + Duration::days(1)),
        "weekly" => {
            let monday = day - Duration::days(t.weekday().num_days_from_monday() as i64);
            (monday, monday + Duration::days(7))
        }
        "monthly" => {
            let first = Utc.with_ymd_and_hms(t.year(), t.month(), 1, 0, 0, 0).single()?;
            let (ny, nm) = if t.month() == 12 { (t.year() + 1, 1) } else { (t.year(), t.month() + 1) };
            (first, Utc.with_ymd_and_hms(ny, nm, 1, 0, 0, 0).single()?)
        }
        _ => return None,
    };
    Some((end.timestamp_millis(), (end - start).num_milliseconds()))
}

/// The window for a spend cap: "Daily limit", "Monthly limit", "Key limit"…
fn limit_window(spent: f64, limit: f64, interval: Option<&str>, resets_at: Option<i64>, now: i64) -> Reading {
    let (id, label) = match interval {
        Some("daily") => ("daily", "Daily limit"),
        Some("weekly") => ("weekly", "Weekly limit"),
        Some("monthly") => ("monthly", "Monthly limit"),
        _ => ("limit", "Key limit"),
    };
    let next = interval.and_then(|i| next_reset(i, now));
    Reading {
        id: id.into(),
        label: label.into(),
        used: spent / limit,
        abs: Some(format!("{} / {}", super::usd(spent), super::usd(limit))),
        resets_at: resets_at.or(next.map(|n| n.0)),
        length_ms: next.map(|n| n.1),
        money: None,
        at: now,
    }
}

/// The `/key` answer → the key's windows and the card's note.
pub fn parse_key(v: &Value, now: i64) -> Result<Fetched, ProviderError> {
    let d = v.get("data").filter(|d| d.is_object()).ok_or_else(|| ProviderError::SchemaChanged("no data".into()))?;
    if d.get("usage").is_none() && d.get("limit").is_none() {
        return Err(ProviderError::SchemaChanged("no usage".into()));
    }
    let mut windows = Vec::new();
    // The most restrictive budget, else the key's own limit.
    let budget = d.get("effective_budget").filter(|b| num(b, "limit_usd").is_some_and(|l| l > 0.0));
    if let Some(b) = budget {
        let interval = b.get("reset_interval").and_then(Value::as_str);
        let resets_at =
            b.get("resets_at").and_then(Value::as_str).and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|t| t.timestamp_millis());
        let limit = num(b, "limit_usd").unwrap_or(0.0);
        let spent = num(b, "spend_usd").or_else(|| num(b, "remaining_usd").map(|r| limit - r)).unwrap_or(0.0);
        windows.push(limit_window(spent.max(0.0), limit, interval, resets_at, now));
    } else if let Some(limit) = num(d, "limit").filter(|l| *l > 0.0) {
        let interval = d.get("limit_reset").and_then(Value::as_str);
        let spent = match num(d, "limit_remaining") {
            Some(r) => limit - r,
            None => match interval {
                Some("daily") => num(d, "usage_daily"),
                Some("weekly") => num(d, "usage_weekly"),
                Some("monthly") => num(d, "usage_monthly"),
                _ => num(d, "usage"),
            }
            .unwrap_or(0.0),
        };
        windows.push(limit_window(spent.max(0.0), limit, interval, None, now));
    }
    let free = d.get("is_free_tier").and_then(Value::as_bool) == Some(true);
    if let Some(f) = d.get("free_model_daily_requests").filter(|_| free) {
        if let (Some(used), Some(limit)) = (num(f, "used"), num(f, "limit").filter(|l| *l > 0.0)) {
            let next = next_reset("daily", now);
            windows.push(Reading {
                id: "free-daily".into(),
                label: "Free requests".into(),
                used: used / limit,
                abs: Some(format!("{used:.0} / {limit:.0} requests")),
                resets_at: next.map(|n| n.0),
                length_ms: Some(DAY_MS),
                money: None,
                at: now,
            });
        }
    }
    let usd = |k: &str| super::usd(num(d, k).unwrap_or(0.0));
    let mut note =
        format!("This key: {} today · {} this week · {} this month", usd("usage_daily"), usd("usage_weekly"), usd("usage_monthly"));
    if windows.is_empty() {
        note += ". No spend limit is set on it.";
    }
    Ok(Fetched { plan: Some(if free { "Free tier" } else { "Credits" }.into()), who: None, windows, caveat: None, note: Some(note) })
}

/// The `/credits` answer → the account's balance (None when nothing was ever bought).
pub fn parse_credits(v: &Value, now: i64) -> Option<Reading> {
    let d = v.get("data")?;
    let total = num(d, "total_credits").filter(|t| *t > 0.0)?;
    let used = num(d, "total_usage").unwrap_or(0.0);
    Some(Reading {
        id: "credits".into(),
        label: "Credits".into(),
        used: used / total,
        abs: Some(format!("{} / {}", super::usd(used), super::usd(total))),
        resets_at: None,
        length_ms: None,
        money: None,
        at: now,
    })
}

impl Provider for OpenRouter {
    fn id(&self) -> &'static str {
        "openrouter"
    }

    fn fetch<'a>(&'a self, acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>> {
        Box::pin(async move {
            let key = super::secrets::get(&super::secrets::key_name(&acct.id)).ok_or_else(|| {
                ProviderError::AuthExpired("The OpenRouter key is missing from the keychain. Add the account again.".into())
            })?;
            let now = crate::now_ms();
            let resp = ctx.http.get(KEY_URL).bearer_auth(&key).send().await.map_err(|e| super::network_error(HOST, &e))?;
            if let Some(e) = ProviderError::from_status(resp.status(), super::retry_after(resp.headers()), HOST, KEY_HINT) {
                return Err(e);
            }
            let v: Value = resp.json().await.map_err(|e| super::network_error(HOST, &e))?;
            let mut f = parse_key(&v, now)?;
            // The balance needs a management key; any other answer just leaves it out.
            if let Ok(r) = ctx.http.get(CREDITS_URL).bearer_auth(&key).send().await {
                if r.status().is_success() {
                    if let Some(c) = r.json::<Value>().await.ok().and_then(|v| parse_credits(&v, now)) {
                        f.windows.push(c);
                        f.note = f.note.map(|n| n.replace(". No spend limit is set on it.", ""));
                    }
                }
            }
            Ok(f)
        })
    }

    fn usage_url(&self, _acct: &AccountRecord) -> String {
        "https://openrouter.ai/settings/credits".into()
    }

    fn min_interval_ms(&self) -> i64 {
        5 * 60_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-23 12:00 UTC, a Friday.
    const NOW: i64 = 1_792_756_800_000;

    #[test]
    fn key_with_monthly_limit() {
        let v = serde_json::json!({"data": {"label": "sk-or-v1-abc...xyz", "limit": 50, "limit_reset": "monthly", "limit_remaining": 37.5,
            "include_byok_in_limit": false, "budgets": [], "effective_budget": null, "usage": 210.4, "usage_daily": 1.25,
            "usage_weekly": 8.6, "usage_monthly": 12.5, "byok_usage": 0, "byok_usage_daily": 0, "byok_usage_weekly": 0,
            "byok_usage_monthly": 0, "is_free_tier": false, "free_model_daily_requests": {"used": 3, "limit": 1000, "remaining": 997}}});
        let f = parse_key(&v, NOW).unwrap();
        assert_eq!(f.plan.as_deref(), Some("Credits"));
        assert_eq!(f.windows.len(), 1, "free requests only on the free tier");
        let w = &f.windows[0];
        assert_eq!((w.id.as_str(), w.label.as_str(), w.abs.as_deref()), ("monthly", "Monthly limit", Some("$12.50 / $50")));
        assert!((w.used - 0.25).abs() < 1e-9);
        // 1 Nov 2026 00:00 UTC, and October's 31 days.
        assert_eq!(w.resets_at, Some(1_793_491_200_000));
        assert_eq!(w.length_ms, Some(31 * DAY_MS));
        assert_eq!(f.note.as_deref(), Some("This key: $1.25 today · $8.60 this week · $12.50 this month"));
    }

    #[test]
    fn budget_wins_over_key_limit() {
        let v = serde_json::json!({"data": {"limit": 100, "limit_reset": null, "limit_remaining": 90, "usage": 10,
            "usage_daily": 2, "usage_weekly": 4, "usage_monthly": 10, "is_free_tier": false,
            "effective_budget": {"scope": "member", "limit_usd": 20, "spend_usd": 4, "remaining_usd": 16,
                "reset_interval": "weekly", "resets_at": "2026-10-26T00:00:00Z", "include_byok": false}}});
        let f = parse_key(&v, NOW).unwrap();
        assert_eq!(f.windows.len(), 1);
        let w = &f.windows[0];
        assert_eq!((w.id.as_str(), w.abs.as_deref()), ("weekly", Some("$4 / $20")));
        assert_eq!(w.resets_at, Some(1_792_972_800_000));
        assert_eq!(w.length_ms, Some(7 * DAY_MS));
    }

    #[test]
    fn unlimited_and_free_tier() {
        let v = serde_json::json!({"data": {"limit": null, "limit_reset": null, "limit_remaining": null, "usage": 0,
            "usage_daily": 0, "usage_weekly": 0, "usage_monthly": 0, "is_free_tier": true,
            "free_model_daily_requests": {"used": 12, "limit": 50, "remaining": 38}}});
        let f = parse_key(&v, NOW).unwrap();
        assert_eq!(f.plan.as_deref(), Some("Free tier"));
        let w = &f.windows[0];
        assert_eq!((w.id.as_str(), w.abs.as_deref()), ("free-daily", Some("12 / 50 requests")));
        assert_eq!(w.resets_at, Some(NOW + 12 * 3_600_000));

        let none = serde_json::json!({"data": {"limit": null, "usage": 3.2, "usage_daily": 0.5, "usage_weekly": 1, "usage_monthly": 3.2, "is_free_tier": false}});
        let f = parse_key(&none, NOW).unwrap();
        assert!(f.windows.is_empty());
        assert!(f.note.unwrap().ends_with(". No spend limit is set on it."));

        assert!(parse_key(&serde_json::json!({"error": {"code": 401}}), NOW).is_err());
        assert!(parse_key(&serde_json::json!({"data": {}}), NOW).is_err());
    }

    #[test]
    fn credits_balance() {
        let v = serde_json::json!({"data": {"total_credits": 100, "total_usage": 42.5}});
        let r = parse_credits(&v, NOW).unwrap();
        assert_eq!((r.id.as_str(), r.abs.as_deref(), r.resets_at), ("credits", Some("$42.50 / $100"), None));
        assert!((r.used - 0.425).abs() < 1e-9);
        assert!(parse_credits(&serde_json::json!({"data": {"total_credits": 0, "total_usage": 0}}), NOW).is_none());
    }

    #[test]
    fn resets() {
        assert_eq!(next_reset("daily", NOW), Some((NOW + 12 * 3_600_000, DAY_MS)));
        // Friday → Monday 26 Oct.
        assert_eq!(next_reset("weekly", NOW).map(|r| r.0), Some(1_792_972_800_000));
        assert_eq!(next_reset("never", NOW), None);
    }
}
