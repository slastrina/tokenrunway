//! AWS Bedrock through an AWS profile (SPEC.md §5.1, Official), using the `aws` CLI so SSO,
//! credential processes and role chains work as they do in the terminal.
//!
//! - Month-to-date spend: `aws ce get-cost-and-usage`, grouped by service, summing every service
//!   with "Bedrock" in its name (Anthropic models bill as "Claude … (Amazon Bedrock Edition)").
//!   Each call costs $0.01, so it's cached on disk and made at most every 6 hours (4 a day).
//! - The allowance: a monthly AWS Budget that covers Bedrock (`aws budgets describe-budgets`),
//!   cached for an hour. Without one, the window is "Monthly spend" with no limit.

use super::{AccountRecord, BoxFuture, Ctx, Fetched, Money, Provider, ProviderError, Reading, HOUR_MS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub const CE_EVERY_MS: i64 = 6 * HOUR_MS;
const BUDGET_EVERY_MS: i64 = HOUR_MS;
pub const MISSING_CLI: &str = "Install the AWS CLI to read Bedrock spend.";

pub struct Bedrock;

/// Profile names from ~/.aws/config (`[profile x]`, `[default]`) and ~/.aws/credentials (`[x]`).
pub fn profiles(home: &Path) -> Vec<String> {
    let aws = std::env::var_os("AWS_CONFIG_FILE").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".aws/config"));
    let creds =
        std::env::var_os("AWS_SHARED_CREDENTIALS_FILE").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".aws/credentials"));
    let mut out: Vec<String> = Vec::new();
    for (path, prefixed) in [(aws, true), (creds, false)] {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        for line in text.lines() {
            let l = line.trim();
            let Some(name) = l.strip_prefix('[').and_then(|l| l.strip_suffix(']')) else { continue };
            let name = name.trim();
            let name = if prefixed { name.strip_prefix("profile ").map(str::trim).unwrap_or(name) } else { name };
            if name.starts_with("sso-session") || name.starts_with("services") || name.is_empty() {
                continue;
            }
            if !out.iter().any(|p| p == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// What's cached per profile, in `<data>/bedrock-<profile>.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Cache {
    /// The month the spend is for ("2026-10") and when it was read
    month: String,
    spend: f64,
    spend_at: i64,
    budget: Option<f64>,
    budget_name: Option<String>,
    budget_at: i64,
    account: Option<String>,
}

fn cache_path(dir: &Path, profile: &str) -> std::path::PathBuf {
    let safe: String = profile.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dir.join(format!("bedrock-{safe}.json"))
}

/// Runs `aws <args> --profile p --output json` and parses stdout.
async fn aws(cli: &Path, profile: &str, args: &[&str]) -> Result<Value, ProviderError> {
    let mut cmd = tokio::process::Command::new(cli);
    cmd.args(args).args(["--profile", profile, "--output", "json"]).env("AWS_PAGER", "").kill_on_drop(true);
    #[cfg(windows)]
    {
        // No console window flashing up for each call.
        cmd.creation_flags(0x0800_0000);
    }
    let out = tokio::time::timeout(std::time::Duration::from_secs(45), cmd.output())
        .await
        .map_err(|_| ProviderError::Network("AWS (the aws CLI timed out)".into()))?
        .map_err(|_| ProviderError::Unavailable(MISSING_CLI.into()))?;
    if !out.status.success() {
        return Err(classify(&String::from_utf8_lossy(&out.stderr), profile));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| ProviderError::SchemaChanged(e.to_string()))
}

/// The aws CLI's error, as a state and words.
pub fn classify(stderr: &str, profile: &str) -> ProviderError {
    let s = stderr.to_ascii_lowercase();
    if s.contains("sso") && (s.contains("expired") || s.contains("token") || s.contains("refresh")) {
        ProviderError::AuthExpired(format!("The AWS SSO session expired. Run `aws sso login --profile {profile}`."))
    } else if s.contains("expiredtoken")
        || s.contains("token has expired")
        || s.contains("security token included in the request is expired")
    {
        ProviderError::AuthExpired(format!("The AWS credentials for {profile} expired. Sign in again and retry."))
    } else if s.contains("unable to locate credentials") || s.contains("could not be found") || s.contains("no credentials") {
        ProviderError::AuthExpired(format!(
            "No AWS credentials for the profile {profile}. Run `aws configure` or `aws sso login --profile {profile}`."
        ))
    } else if s.contains("accessdenied") || s.contains("not authorized") || s.contains("unauthorizedoperation") {
        ProviderError::Unavailable(format!(
            "The profile {profile} can't read Cost Explorer (it needs ce:GetCostAndUsage and budgets:ViewBudget)."
        ))
    } else if s.contains("throttl") || s.contains("limitexceeded") || s.contains("too many requests") {
        ProviderError::RateLimited(None)
    } else if s.contains("could not connect")
        || s.contains("connect timeout")
        || s.contains("name resolution")
        || s.contains("endpoint url")
    {
        ProviderError::Network("AWS".into())
    } else {
        ProviderError::Unavailable(format!("The aws CLI couldn't read Bedrock spend for {profile}."))
    }
}

/// Sums the month's cost of every service whose name contains "Bedrock".
pub fn parse_cost(v: &Value) -> Result<f64, ProviderError> {
    let results =
        v.get("ResultsByTime").and_then(Value::as_array).ok_or_else(|| ProviderError::SchemaChanged("no ResultsByTime".into()))?;
    let mut total = 0.0;
    for r in results {
        for g in r.get("Groups").and_then(Value::as_array).into_iter().flatten() {
            let name = g.get("Keys").and_then(|k| k.get(0)).and_then(Value::as_str).unwrap_or_default();
            if !name.to_ascii_lowercase().contains("bedrock") {
                continue;
            }
            total += g.pointer("/Metrics/UnblendedCost/Amount").and_then(Value::as_str).and_then(|a| a.parse::<f64>().ok()).unwrap_or(0.0);
        }
    }
    Ok(total)
}

/// The monthly cost budget that covers Bedrock: one filtered to a Bedrock service, else one
/// with Bedrock in its name.
pub fn pick_budget(v: &Value) -> Option<(String, f64)> {
    let budgets = v.get("Budgets")?.as_array()?;
    let monthly_cost = |b: &&Value| {
        b.get("BudgetType").and_then(Value::as_str) == Some("COST")
            && b.get("TimeUnit").and_then(Value::as_str).is_none_or(|t| t == "MONTHLY")
    };
    let covers = |b: &&Value| {
        let filtered = b
            .pointer("/CostFilters/Service")
            .and_then(Value::as_array)
            .is_some_and(|s| s.iter().any(|x| x.as_str().is_some_and(|x| x.to_ascii_lowercase().contains("bedrock"))));
        let named = b.get("BudgetName").and_then(Value::as_str).is_some_and(|n| n.to_ascii_lowercase().contains("bedrock"));
        filtered || named
    };
    let b = budgets.iter().filter(monthly_cost).find(covers)?;
    let amount = b.pointer("/BudgetLimit/Amount").and_then(Value::as_str)?.parse().ok()?;
    Some((b.get("BudgetName").and_then(Value::as_str).unwrap_or("Budget").to_string(), amount))
}

fn ymd(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Utc.timestamp_millis_opt(ms).single().map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()
}

async fn read(profile: &str, ctx: &Ctx) -> Result<Cache, ProviderError> {
    let Some(cli) = super::find_program("aws") else { return Err(ProviderError::Unavailable(MISSING_CLI.into())) };
    let path = cache_path(&ctx.data_dir, profile);
    let mut c: Cache = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let now = crate::now_ms();
    let (start, _) = super::month_bounds(now);
    let month = ymd(start + 12 * HOUR_MS)[..7].to_string();
    if c.month != month || now - c.spend_at >= CE_EVERY_MS {
        // Cost Explorer works in UTC days; the end is exclusive, so tomorrow includes today.
        let (from, to) = (format!("{month}-01"), ymd(now + 24 * HOUR_MS));
        let v = aws(
            &cli,
            profile,
            &[
                "ce",
                "get-cost-and-usage",
                "--time-period",
                &format!("Start={from},End={to}"),
                "--granularity",
                "MONTHLY",
                "--metrics",
                "UnblendedCost",
                "--group-by",
                "Type=DIMENSION,Key=SERVICE",
            ],
        )
        .await?;
        c.spend = parse_cost(&v)?;
        c.month = month;
        c.spend_at = now;
        let _ = super::accounts::write_atomic(&path, &serde_json::to_string(&c).unwrap_or_default());
    }
    if now - c.budget_at >= BUDGET_EVERY_MS {
        if c.account.is_none() {
            c.account = aws(&cli, profile, &["sts", "get-caller-identity"])
                .await
                .ok()
                .and_then(|v| v.get("Account").and_then(Value::as_str).map(String::from));
        }
        if let Some(acct) = c.account.clone() {
            // Budgets are optional: without read access there's just no limit.
            match aws(&cli, profile, &["budgets", "describe-budgets", "--account-id", &acct]).await {
                Ok(v) => {
                    let b = pick_budget(&v);
                    c.budget_name = b.as_ref().map(|b| b.0.clone());
                    c.budget = b.map(|b| b.1);
                }
                Err(ProviderError::Network(h)) => return Err(ProviderError::Network(h)),
                Err(_) => {}
            }
        }
        c.budget_at = now;
        let _ = super::accounts::write_atomic(&path, &serde_json::to_string(&c).unwrap_or_default());
    }
    Ok(c)
}

/// The month's window from a cached reading.
fn window(c: &Cache, limit_override: Option<f64>, now: i64) -> Reading {
    let (start, next) = super::month_bounds(now);
    let limit = c.budget.or(limit_override).filter(|l| *l > 0.0);
    let (label, used, abs) = match limit {
        Some(l) => ("Monthly budget", c.spend / l, format!("{} / {}", super::usd(c.spend), super::usd(l))),
        None => ("Monthly spend", 0.0, format!("{} this month", super::usd(c.spend))),
    };
    Reading {
        id: if limit.is_some() { "budget".into() } else { "monthly".into() },
        label: label.into(),
        used,
        abs: Some(abs),
        resets_at: Some(next),
        length_ms: Some(next - start),
        money: Some(Money { used: c.spend, limit }),
        at: c.spend_at,
    }
}

impl Provider for Bedrock {
    fn id(&self) -> &'static str {
        "bedrock"
    }

    fn fetch<'a>(&'a self, acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>> {
        Box::pin(async move {
            let profile = acct.profile.clone().unwrap_or_else(|| "default".into());
            let c = read(&profile, ctx).await?;
            Ok(Fetched {
                plan: Some("On-demand".into()),
                who: Some(profile),
                windows: vec![window(&c, acct.limit_usd, crate::now_ms())],
                caveat: None,
            })
        })
    }

    fn usage_url(&self, _acct: &AccountRecord) -> String {
        "https://console.aws.amazon.com/costmanagement/home#/cost-explorer".into()
    }

    fn min_interval_ms(&self) -> i64 {
        5 * 60_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_profiles() {
        let home = std::env::temp_dir().join(format!("tokenrunway-aws-{}", std::process::id()));
        std::fs::create_dir_all(home.join(".aws")).unwrap();
        std::fs::write(home.join(".aws/config"), "[default]\nregion=x\n[profile work]\nregion=y\n[sso-session corp]\n").unwrap();
        std::fs::write(home.join(".aws/credentials"), "[default]\naws_access_key_id=x\n[personal]\n").unwrap();
        if std::env::var_os("AWS_CONFIG_FILE").is_none() && std::env::var_os("AWS_SHARED_CREDENTIALS_FILE").is_none() {
            assert_eq!(profiles(&home), ["default", "work", "personal"]);
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn cost_and_budget() {
        let ce = serde_json::json!({"ResultsByTime": [{"Groups": [
            {"Keys": ["Amazon Bedrock"], "Metrics": {"UnblendedCost": {"Amount": "84.5", "Unit": "USD"}}},
            {"Keys": ["Claude 3.5 Sonnet (Amazon Bedrock Edition)"], "Metrics": {"UnblendedCost": {"Amount": "100", "Unit": "USD"}}},
            {"Keys": ["Amazon EC2"], "Metrics": {"UnblendedCost": {"Amount": "999", "Unit": "USD"}}}
        ]}]});
        assert!((parse_cost(&ce).unwrap() - 184.5).abs() < 1e-9);
        assert!(parse_cost(&serde_json::json!({})).is_err());
        let b = serde_json::json!({"Budgets": [
            {"BudgetName": "Everything", "BudgetType": "COST", "TimeUnit": "MONTHLY", "BudgetLimit": {"Amount": "5000", "Unit": "USD"}},
            {"BudgetName": "AI", "BudgetType": "COST", "TimeUnit": "MONTHLY", "BudgetLimit": {"Amount": "500.0", "Unit": "USD"}, "CostFilters": {"Service": ["Amazon Bedrock"]}}
        ]});
        assert_eq!(pick_budget(&b), Some(("AI".into(), 500.0)));
        let c = Cache { spend: 184.0, budget: Some(500.0), spend_at: 1, ..Default::default() };
        let w = window(&c, None, crate::now_ms());
        assert_eq!((w.label.as_str(), w.abs.as_deref()), ("Monthly budget", Some("$184 / $500")));
        let w = window(&Cache { spend: 184.0, ..Default::default() }, None, crate::now_ms());
        assert_eq!(w.label, "Monthly spend");
    }

    #[test]
    fn errors() {
        assert!(matches!(classify("Error loading SSO Token: Token for x does not exist", "p"), ProviderError::AuthExpired(_)));
        assert!(matches!(
            classify("Unable to locate credentials. You can configure credentials by running \"aws configure\".", "p"),
            ProviderError::AuthExpired(_)
        ));
        assert!(matches!(classify("An error occurred (AccessDeniedException) when calling", "p"), ProviderError::Unavailable(_)));
        assert!(matches!(classify("Could not connect to the endpoint URL", "p"), ProviderError::Network(_)));
    }
}
