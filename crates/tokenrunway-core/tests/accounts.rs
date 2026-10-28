//! Telling logins apart end to end: Claude Code calls go to the login `.claude.json` named when
//! they were made, Codex calls to the account their `session_meta` names, and the dashboard's
//! account filter keeps one login's work.

use std::path::{Path, PathBuf};
use tokenrunway_core::ingest::Roots;
use tokenrunway_core::stats::{Filter, Stats};

struct Env {
    dir: PathBuf,
    stats: Stats,
    claude_json: PathBuf,
    log: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn env() -> Env {
    let dir = std::env::temp_dir().join(format!("tr-accounts-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let projects = dir.join(".claude").join("projects");
    let logs = projects.join("-tmp-x");
    std::fs::create_dir_all(&logs).unwrap();
    let codex = dir.join(".codex").join("sessions").join("2026").join("10").join("20");
    std::fs::create_dir_all(&codex).unwrap();
    let rollout = "rollout-2026-10-20T10-00-00-019a0000-0000-7000-8000-000000000009.jsonl";
    let meta = r#"{"timestamp":"2026-10-20T11:00:00.000Z","type":"session_meta","payload":{"id":"cx-9","cwd":"/tmp/codexproj","creator_account_id":"acc-1"}}"#;
    let ctx = r#"{"timestamp":"2026-10-20T11:00:00.200Z","type":"turn_context","payload":{"turn_id":"t1","cwd":"/tmp/codexproj","model":"gpt-5.6-terra"}}"#;
    let count = r#"{"timestamp":"2026-10-20T11:00:05.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":1500},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":500,"total_tokens":1500}}}}"#;
    std::fs::write(codex.join(rollout), format!("{meta}\n{ctx}\n{count}\n")).unwrap();
    let roots = Roots { claude: vec![projects], codex: vec![dir.join(".codex").join("sessions")], ..Default::default() };
    let stats = Stats::open_with(&dir.join("activity.db"), roots).unwrap();
    Env { claude_json: dir.join(".claude.json"), log: logs.join("s.jsonl"), dir, stats }
}

fn sign_in(path: &Path, uuid: &str) {
    std::fs::write(path, format!(r#"{{"oauthAccount":{{"accountUuid":"{uuid}","emailAddress":"x@example.com"}}}}"#)).unwrap();
}

/// One Claude Code call in session `session`, made now.
fn call(path: &Path, n: u32, session: &str) {
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let line = format!(
        r#"{{"type":"assistant","requestId":"r{n}","uuid":"a{n}","timestamp":"{at}","cwd":"/tmp/x","sessionId":"{session}","message":{{"id":"m{n}","model":"claude-opus-5-5","content":[],"usage":{{"input_tokens":1000,"output_tokens":100}}}}}}"#
    );
    let mut old = std::fs::read_to_string(path).unwrap_or_default();
    old.push_str(&line);
    old.push('\n');
    std::fs::write(path, old).unwrap();
}

fn sessions(e: &Env, harness: &str, logins: Option<&[&str]>) -> u32 {
    let f = Filter {
        harnesses: Some(vec![harness.into()]),
        logins: logins.map(|l| l.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    };
    e.stats.dashboard_filtered("lifetime", "model", &f, tokenrunway_core::now_ms()).sessions_total
}

#[test]
fn each_login_gets_its_own_sessions() {
    let e = env();
    // A session from before Tokenrunway saw any login: can't be told apart.
    call(&e.log, 1, "early");
    e.stats.ingest();
    sign_in(&e.claude_json, "u-work");
    e.stats.ingest(); // first check of the login
    call(&e.log, 2, "work");
    e.stats.ingest();
    std::thread::sleep(std::time::Duration::from_millis(5));
    sign_in(&e.claude_json, "u-home");
    e.stats.ingest(); // the switch is seen
    std::thread::sleep(std::time::Duration::from_millis(5));
    call(&e.log, 3, "home");
    e.stats.ingest();

    assert_eq!(sessions(&e, "claude-code", None), 3, "the harness alone: everything, as before");
    assert_eq!(sessions(&e, "claude-code", Some(&["u-work"])), 1);
    assert_eq!(sessions(&e, "claude-code", Some(&["u-home"])), 1);
    assert_eq!(sessions(&e, "claude-code", Some(&["u-work", "u-home"])), 2, "the early one belongs to neither");
    assert_eq!(sessions(&e, "claude-code", Some(&[])), 0);
    // Codex says which account each session ran under, history included.
    assert_eq!(sessions(&e, "codex", Some(&["acc-1"])), 1);
    assert_eq!(sessions(&e, "codex", Some(&["acc-2"])), 0);
    let first = e.stats.first_for_logins(&["claude-code".into()], &["u-home".into()]);
    assert!(first.is_some_and(|t| t > 0));
}
