//! Google Antigravity's built-in, non-interactive quota report.

use super::{find_program, AccountRecord, BoxFuture, Ctx, Fetched, Provider, ProviderError, Reading, DAY_MS, HOUR_MS, MIN_MS};
use crate::view::Detected;
use chrono::DateTime;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

const DOCS: &str = "https://antigravity.google/docs/cli/commands/usage";
const INSTALL: &str = "Install or update the Antigravity CLI to agy 1.1.11 or later, then check again.";
const SIGN_IN: &str = "Run `agy` in a terminal and sign in with Google, then check again.";
const MAX_OUTPUT: u64 = 1024 * 1024;

pub struct AntigravityCli;

impl Provider for AntigravityCli {
    fn id(&self) -> &'static str {
        "antigravity-cli"
    }

    fn fetch<'a>(&'a self, _acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>> {
        Box::pin(async move { fetch(&ctx.home).await })
    }

    fn usage_url(&self, _acct: &AccountRecord) -> String {
        DOCS.into()
    }

    fn limit_settings(&self) -> Option<(&'static str, &'static str)> {
        Some(("Open Google One AI credits", "https://one.google.com/ai/credits"))
    }

    fn min_interval_ms(&self) -> i64 {
        5 * MIN_MS
    }
}

fn program(home: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    let local = dirs::data_local_dir().unwrap_or_else(|| home.join("AppData/Local")).join("agy/bin/agy.exe");
    #[cfg(not(windows))]
    let local = home.join(".local/bin/agy");
    local.is_file().then_some(local).or_else(|| find_program("agy"))
}

pub fn detect(home: &Path) -> Option<Detected> {
    // The install alone does not tell us whether there is a CLI profile to connect.
    let gemini_home = std::env::var_os("GEMINI_CLI_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".gemini"));
    if !gemini_home.join("antigravity-cli").is_dir() {
        return None;
    }
    program(home)?;
    Some(Detected {
        kind: "antigravity-cli".into(),
        label: "Antigravity CLI".into(),
        detail: String::new(),
        provider: "antigravity".into(),
        mono: "Ag".into(),
    })
}

async fn fetch(home: &Path) -> Result<Fetched, ProviderError> {
    let program = program(home).ok_or_else(|| ProviderError::Unavailable(INSTALL.into()))?;
    let work = tempfile::tempdir().map_err(|_| ProviderError::Unavailable("Couldn't prepare the Antigravity quota check.".into()))?;
    #[cfg(unix)]
    let paths = {
        // Keep passive refreshes from opening a sign-in window when CLI credentials expire.
        for name in ["open", "xdg-open"] {
            std::os::unix::fs::symlink("/usr/bin/false", work.path().join(name))
                .map_err(|_| ProviderError::Unavailable("Couldn't prepare the Antigravity quota check.".into()))?;
        }
        let paths = std::env::var_os("PATH").unwrap_or_default();
        std::env::join_paths(std::iter::once(work.path().to_path_buf()).chain(std::env::split_paths(&paths)))
            .map_err(|_| ProviderError::Unavailable("Couldn't prepare the Antigravity quota check.".into()))?
    };
    let command = || {
        let mut cmd = Command::new(&program);
        cmd.current_dir(work.path()).env("AGY_CLI_DISABLE_AUTO_UPDATE", "true");
        #[cfg(unix)]
        cmd.env("BROWSER", work.path().join("open")).env("PATH", &paths);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        cmd
    };

    let version = run(command(), &["--version"], Duration::from_secs(5)).await?;
    if !version.success || !supports_usage(&String::from_utf8_lossy(&version.stdout)) {
        return Err(ProviderError::Unavailable(INSTALL.into()));
    }
    let report = run(command(), &["-p", "/usage", "--output-format", "json"], Duration::from_secs(30)).await?;
    if report.success {
        if let Ok(fetched) = parse_report(&report.stdout, crate::now_ms()) {
            return Ok(fetched);
        }
    }
    if sign_in_required(&report.stderr) || sign_in_required(&report.stdout) {
        return Err(ProviderError::AuthExpired(SIGN_IN.into()));
    }
    if !report.success {
        return Err(ProviderError::Network("Antigravity".into()));
    }
    Err(ProviderError::SchemaChanged("Unrecognized Antigravity CLI quota report".into()))
}

struct Output {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn run(mut command: Command, args: &[&str], limit: Duration) -> Result<Output, ProviderError> {
    let mut child = command
        .args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ProviderError::Unavailable(INSTALL.into()))?;
    let out = child.stdout.take().expect("piped stdout").take(MAX_OUTPUT + 1);
    let err = child.stderr.take().expect("piped stderr").take(MAX_OUTPUT + 1);
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let status = timeout(limit, async {
        tokio::try_join!(
            async {
                let mut out = out;
                out.read_to_end(&mut stdout).await
            },
            async {
                let mut err = err;
                err.read_to_end(&mut stderr).await
            },
            child.wait()
        )
    })
    .await
    .map_err(|_| ProviderError::Network("Antigravity".into()))?
    .map_err(|_| ProviderError::Network("Antigravity".into()))?
    .2;
    if stdout.len() as u64 > MAX_OUTPUT || stderr.len() as u64 > MAX_OUTPUT {
        return Err(ProviderError::SchemaChanged("Antigravity CLI output exceeded the report limit".into()));
    }
    Ok(Output { success: status.success(), stdout, stderr })
}

fn supports_usage(version: &str) -> bool {
    // Before 1.1.11, print mode could interpret slash commands as agent prompts.
    let version = version.trim().strip_prefix("agy ").unwrap_or(version.trim());
    let version = version.strip_prefix("version ").unwrap_or(version);
    version
        .split_whitespace()
        .next()
        .and_then(|word| semver::Version::parse(word.trim_start_matches('v')).ok())
        .is_some_and(|v| v >= semver::Version::new(1, 1, 11))
}

fn sign_in_required(output: &[u8]) -> bool {
    let text = String::from_utf8_lossy(output).to_ascii_lowercase();
    ["authentication required", "not logged in", "not signed in", "failed to authenticate", "sign in with google", "invalid_grant"]
        .iter()
        .any(|message| text.contains(message))
}

fn parse_report(bytes: &[u8], at: i64) -> Result<Fetched, ProviderError> {
    let invalid = || ProviderError::SchemaChanged("Unrecognized Antigravity CLI quota report".into());
    let report: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if report["status"] != "SUCCESS" || report["command"]["name"] != "usage" || report["num_turns"].as_u64().is_some_and(|n| n > 0) {
        return Err(invalid());
    }
    let groups = report["command"]["data"]["groups"].as_array().ok_or_else(invalid)?;
    let mut windows = Vec::new();
    let mut seen = BTreeSet::new();
    for (group_index, group) in groups.iter().enumerate() {
        let Some(buckets) = group["buckets"].as_array() else { continue };
        let group_key = group_index.to_string();
        let name = group["name"].as_str().filter(|s| !s.is_empty()).unwrap_or("Model");
        for bucket in buckets {
            let (Some(id), Some(remaining)) = (bucket["id"].as_str(), bucket["remaining_fraction"].as_f64()) else { continue };
            if id.is_empty() || bucket["enabled"].as_bool() == Some(false) || !(0.0..=1.0).contains(&remaining) {
                continue;
            }
            let (cadence, length_ms, title) = match bucket["window"].as_str() {
                Some("5h") => (Some("5h"), Some(5 * HOUR_MS), "5-hour"),
                Some("weekly") => (Some("weekly"), Some(7 * DAY_MS), "Weekly"),
                _ => (None, None, bucket["name"].as_str().unwrap_or("Quota")),
            };
            let id = cadence.map_or_else(|| id.to_owned(), |cadence| format!("{cadence}-{id}"));
            if !seen.insert(id.clone()) {
                continue;
            }
            windows.push(Reading {
                group: Some(group_key.clone()),
                id,
                label: format!("{name} · {title}"),
                used: 1.0 - remaining,
                abs: None,
                resets_at: bucket["reset_time"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|t| t.timestamp_millis()),
                length_ms,
                money: None,
                at,
            });
        }
    }
    if windows.is_empty() {
        return Ok(Fetched {
            note: Some("Antigravity did not report any measured model limits for this CLI account.".into()),
            ..Default::default()
        });
    }
    Ok(Fetched { windows, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allowance::{account_view, Live};
    use crate::forecast::Thresholds;

    const REPORT: &str = r#"{
        "status": "SUCCESS", "num_turns": 0,
        "command": {"name": "usage", "data": {"groups": [
            {"name": "Gemini Models", "buckets": [
                {"id": "gemini-weekly", "window": "weekly", "remaining_fraction": 0.75, "reset_time": "2026-11-30T00:00:00Z"},
                {"id": "gemini-5h", "window": "5h", "remaining_fraction": 0.0}
            ]},
            {"name": "Claude and GPT models", "buckets": [
                {"id": "3p-weekly", "window": "weekly", "remaining_fraction": 1.0},
                {"id": "3p-5h", "window": "5h", "remaining_fraction": 1.0},
                {"id": "disabled", "window": "5h", "remaining_fraction": 0.0, "enabled": false},
                {"id": "unknown", "window": "weekly"},
                {"id": "invalid", "window": "weekly", "remaining_fraction": 1.1}
            ]}
        ]}}
    }"#;

    #[test]
    fn reads_each_measured_bucket_and_its_reset() {
        let fetched = parse_report(REPORT.as_bytes(), 42).unwrap();
        assert_eq!(fetched.windows.len(), 4);
        let weekly = &fetched.windows[0];
        assert_eq!(weekly.label, "Gemini Models · Weekly");
        assert_eq!(weekly.id, "weekly-gemini-weekly");
        assert_eq!(weekly.used, 0.25);
        assert_eq!(weekly.resets_at, Some(1_795_996_800_000));
        assert_eq!(weekly.length_ms, Some(7 * DAY_MS));
        assert_eq!(weekly.at, 42);
        assert_eq!(fetched.windows[1].length_ms, Some(5 * HOUR_MS));
        assert_eq!(fetched.windows[1].used, 1.0);
        assert_eq!(fetched.windows[2].used, 0.0);
    }

    #[test]
    fn account_card_shows_the_shorter_period_first_in_each_model_group() {
        let at = 42;
        let fetched = parse_report(REPORT.as_bytes(), at).unwrap();
        let acct = AccountRecord { provider: "antigravity".into(), method: "cli".into(), ..Default::default() };
        let live = Live { last: Some(fetched), tried: true, ..Default::default() };
        let view = account_view(&acct, &live, &|_| Vec::new(), &Thresholds::default(), at);
        assert_eq!(
            view.windows.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
            ["5h-gemini-5h", "weekly-gemini-weekly", "5h-3p-5h", "weekly-3p-weekly"],
        );
    }

    #[test]
    fn rejects_agent_responses_and_old_clis() {
        assert!(parse_report(br#"{"status":"SUCCESS","usage":{"total_tokens":0}}"#, 42).is_err());
        assert!(supports_usage("agy version v1.1.11"));
        assert!(supports_usage("1.5.12\n"));
        assert!(!supports_usage("1.1.10"));
        assert!(!supports_usage("1.1.11-beta.1"));
        assert!(!supports_usage("unknown"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fetch_runs_the_quota_command_only_on_a_supported_cli() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let program = home.path().join(".local/bin/agy");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        let script = |version: &str, body: &str| {
            std::fs::write(
                &program,
                format!(
                    "#!/bin/sh\ncase \"$*\" in\n--version) echo {version};;\n'-p /usage --output-format json') {body};;\n*) exit 1;;\nesac\n"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        };

        script("1.5.12", &format!("cat <<'REPORT'\n{REPORT}\nREPORT\n"));
        assert_eq!(fetch(home.path()).await.unwrap().windows.len(), 4);
        script("1.5.12", "echo 'Authentication required' >&2; exit 1");
        assert_eq!(fetch(home.path()).await.unwrap_err(), ProviderError::AuthExpired(SIGN_IN.into()));
        script("1.1.10", "echo 'Authentication required' >&2; exit 1");
        assert_eq!(fetch(home.path()).await.unwrap_err(), ProviderError::Unavailable(INSTALL.into()));
    }
}
