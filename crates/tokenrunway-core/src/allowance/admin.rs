//! Anthropic API and OpenAI API organisations through an admin key (SPEC.md §5.1, Official):
//! month-to-date spend from the cost report endpoints. The key is pasted in once and kept in the
//! OS keychain.
//!
//! - Anthropic: `GET /v1/organizations/cost_report` (`x-api-key`, `anthropic-version`); buckets
//!   in `data[].results[].amount`, a decimal string in cents.
//! - OpenAI: `GET /v1/organization/costs` (Bearer); buckets in `data[].results[].amount.value`,
//!   in dollars.

use super::{AccountRecord, BoxFuture, Ctx, Fetched, Money, Provider, ProviderError, Reading};
use serde_json::Value;

pub struct AnthropicAdmin;
pub struct OpenaiAdmin;

const ANTHROPIC: &str = "https://api.anthropic.com/v1/organizations/cost_report";
const OPENAI: &str = "https://api.openai.com/v1/organization/costs";
const BAD_KEY: &str = "The admin key was refused. Paste a new one in Settings → Accounts.";

fn key(acct: &AccountRecord) -> Result<String, ProviderError> {
    super::secrets::get(&super::secrets::key_name(&acct.id))
        .ok_or_else(|| ProviderError::AuthExpired("The admin key is missing from the keychain. Add the account again.".into()))
}

/// Anthropic: sum of every bucket's amounts (cents) in dollars.
pub fn sum_anthropic(v: &Value) -> Result<f64, ProviderError> {
    let data = v.get("data").and_then(Value::as_array).ok_or_else(|| ProviderError::SchemaChanged("no data".into()))?;
    Ok(data
        .iter()
        .flat_map(|b| b.get("results").and_then(Value::as_array).into_iter().flatten())
        .filter_map(|r| match r.get("amount") {
            Some(Value::String(s)) => s.parse::<f64>().ok(),
            Some(Value::Number(n)) => n.as_f64(),
            _ => None,
        })
        .sum::<f64>()
        / 100.0)
}

/// OpenAI: sum of every bucket's `amount.value` (dollars).
pub fn sum_openai(v: &Value) -> Result<f64, ProviderError> {
    let data = v.get("data").and_then(Value::as_array).ok_or_else(|| ProviderError::SchemaChanged("no data".into()))?;
    Ok(data
        .iter()
        .flat_map(|b| b.get("results").and_then(Value::as_array).into_iter().flatten())
        .filter_map(|r| r.pointer("/amount/value").and_then(Value::as_f64))
        .sum())
}

fn month_window(spend: f64, limit: Option<f64>, now: i64) -> Reading {
    let (start, next) = super::month_bounds(now);
    let limit = limit.filter(|l| *l > 0.0);
    Reading {
        id: "monthly".into(),
        label: "Monthly spend".into(),
        used: limit.map(|l| spend / l).unwrap_or(0.0),
        abs: Some(match limit {
            Some(l) => format!("{} / {}", super::usd(spend), super::usd(l)),
            None => format!("{} this month", super::usd(spend)),
        }),
        resets_at: Some(next),
        length_ms: Some(next - start),
        money: Some(Money { used: spend, limit }),
        at: now,
    }
}

async fn get_json(req: reqwest::RequestBuilder, host: &str) -> Result<Value, ProviderError> {
    let resp = req.send().await.map_err(|e| super::network_error(host, &e))?;
    if let Some(e) = ProviderError::from_status(resp.status(), super::retry_after(resp.headers()), host, BAD_KEY) {
        return Err(e);
    }
    resp.json().await.map_err(|e| super::network_error(host, &e))
}

fn rfc3339(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Utc.timestamp_millis_opt(ms).single().map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string()).unwrap_or_default()
}

impl Provider for AnthropicAdmin {
    fn id(&self) -> &'static str {
        "anthropic-admin"
    }

    fn fetch<'a>(&'a self, acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>> {
        Box::pin(async move {
            let key = key(acct)?;
            let now = crate::now_ms();
            let (start, _) = super::month_bounds(now);
            let (mut total, mut page): (f64, Option<String>) = (0.0, None);
            for _ in 0..5 {
                let mut q = vec![("starting_at", rfc3339(start)), ("bucket_width", "1d".into()), ("limit", "31".into())];
                if let Some(p) = &page {
                    q.push(("page", p.clone()));
                }
                let req = ctx.http.get(ANTHROPIC).query(&q).header("x-api-key", &key).header("anthropic-version", "2023-06-01");
                let v = get_json(req, "api.anthropic.com").await?;
                total += sum_anthropic(&v)?;
                page = v
                    .get("next_page")
                    .and_then(Value::as_str)
                    .map(String::from)
                    .filter(|_| v.get("has_more").and_then(Value::as_bool) == Some(true));
                if page.is_none() {
                    break;
                }
            }
            Ok(Fetched {
                plan: Some("API".into()),
                who: None,
                windows: vec![month_window(total, acct.limit_usd, now)],
                caveat: None,
                note: None,
            })
        })
    }

    fn usage_url(&self, _acct: &AccountRecord) -> String {
        "https://console.anthropic.com/settings/cost".into()
    }

    fn min_interval_ms(&self) -> i64 {
        15 * 60_000
    }
}

impl Provider for OpenaiAdmin {
    fn id(&self) -> &'static str {
        "openai-admin"
    }

    fn fetch<'a>(&'a self, acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>> {
        Box::pin(async move {
            let key = key(acct)?;
            let now = crate::now_ms();
            let (start, _) = super::month_bounds(now);
            let (mut total, mut page): (f64, Option<String>) = (0.0, None);
            for _ in 0..5 {
                let mut q = vec![("start_time", (start / 1000).to_string()), ("bucket_width", "1d".into()), ("limit", "31".into())];
                if let Some(p) = &page {
                    q.push(("page", p.clone()));
                }
                let v = get_json(ctx.http.get(OPENAI).query(&q).bearer_auth(&key), "api.openai.com").await?;
                total += sum_openai(&v)?;
                page = v
                    .get("next_page")
                    .and_then(Value::as_str)
                    .map(String::from)
                    .filter(|_| v.get("has_more").and_then(Value::as_bool) == Some(true));
                if page.is_none() {
                    break;
                }
            }
            Ok(Fetched {
                plan: Some("API".into()),
                who: None,
                windows: vec![month_window(total, acct.limit_usd, now)],
                caveat: None,
                note: None,
            })
        })
    }

    fn usage_url(&self, _acct: &AccountRecord) -> String {
        "https://platform.openai.com/usage".into()
    }

    fn min_interval_ms(&self) -> i64 {
        15 * 60_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums() {
        let a = serde_json::json!({"data": [{"starting_at": "x", "results": [{"currency": "USD", "amount": "12345.6"}, {"amount": "100"}]}], "has_more": false});
        assert!((sum_anthropic(&a).unwrap() - 124.456).abs() < 1e-9);
        let o = serde_json::json!({"object": "page", "data": [{"object": "bucket", "results": [{"amount": {"value": 1.5, "currency": "usd"}}]}, {"results": [{"amount": {"value": 2.0}}]}]});
        assert!((sum_openai(&o).unwrap() - 3.5).abs() < 1e-9);
        assert!(sum_openai(&serde_json::json!({"error": {}})).is_err());
        let w = month_window(184.0, None, crate::now_ms());
        assert_eq!(w.abs.as_deref(), Some("$184 this month"));
        assert_eq!(month_window(184.0, Some(500.0), crate::now_ms()).abs.as_deref(), Some("$184 / $500"));
    }
}
