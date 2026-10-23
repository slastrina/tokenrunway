//! End to end over synthetic logs (tests/fixtures; no real prompts anywhere): every reader,
//! the store's dedupe, incremental passes and the three views.

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use tokenrunway_core::ingest::Roots;
use tokenrunway_core::stats::Stats;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), t).unwrap();
        }
    }
}

/// A scratch directory with copies of the fixtures (so tests can append to them) and an
/// opencode.db built here, since a binary fixture would hide what's in it.
struct Env {
    dir: PathBuf,
    stats: Stats,
}

impl Env {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tr-it-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        copy_dir(&fixtures(), &dir);
        let oc = dir.join("opencode");
        std::fs::create_dir_all(&oc).unwrap();
        make_opencode_db(&oc.join("opencode.db"));
        let roots = Roots {
            claude: vec![dir.join("claude").join("projects")],
            codex: vec![dir.join("codex").join("sessions")],
            opencode: vec![oc, dir.join("opencode-old")],
        };
        let stats = Stats::open_with(&dir.join("activity.db"), roots).unwrap();
        Self { dir, stats }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn make_opencode_db(path: &Path) {
    let c = Connection::open(path).unwrap();
    c.execute_batch(
        "CREATE TABLE session (id TEXT PRIMARY KEY, project_id TEXT, parent_id TEXT, directory TEXT NOT NULL, title TEXT,
             time_created INTEGER, time_updated INTEGER);
         CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
         CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
         INSERT INTO session VALUES ('ses1', 'p', NULL, '/tmp/ocproj', 't', 1792490000000, 1792490000000);
         INSERT INTO session VALUES ('ses2', 'p', 'ses1', '/tmp/ocproj', 't', 1792490100000, 1792490100000);",
    )
    .unwrap();
    let msg = |id: &str, ses: &str, t: i64, data: serde_json::Value| {
        c.execute("INSERT INTO message VALUES (?1, ?2, ?3, ?3, ?4)", rusqlite::params![id, ses, t, data.to_string()]).unwrap();
    };
    let part = |id: &str, mid: &str, ses: &str, t: i64, data: serde_json::Value| {
        c.execute("INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?4, ?5)", rusqlite::params![id, mid, ses, t, data.to_string()]).unwrap();
    };
    let asst = |parent: &str, model: &str, input: u64, output: u64, reasoning: u64, cost: f64, t: i64| {
        serde_json::json!({"role": "assistant", "parentID": parent, "modelID": model, "providerID": "anthropic", "cost": cost,
            "path": {"cwd": "/tmp/ocproj", "root": "/"}, "time": {"created": t},
            "tokens": {"input": input, "output": output, "reasoning": reasoning, "cache": {"read": 0, "write": 0}}})
    };
    msg("msg_u1", "ses1", 1792490000000, serde_json::json!({"role": "user", "time": {"created": 1792490000000_i64}}));
    part("prt_u1", "msg_u1", "ses1", 1792490000000, serde_json::json!({"type": "text", "text": "Write docs for the public API"}));
    msg("msg_a1", "ses1", 1792490001000, asst("msg_u1", "claude-sonnet-5-5", 1000, 100, 50, 0.0125, 1792490001000));
    part(
        "prt_a1",
        "msg_a1",
        "ses1",
        1792490001000,
        serde_json::json!({"type": "tool", "tool": "write", "state": {"input": {"filePath": "docs/api.md"}}}),
    );
    // a subagent session: counts towards ses1
    msg("msg_u2", "ses2", 1792490100000, serde_json::json!({"role": "user", "time": {"created": 1792490100000_i64}}));
    msg("msg_a2", "ses2", 1792490101000, asst("msg_u2", "claude-sonnet-5-5", 10, 10, 0, 0.001, 1792490101000));
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

// API-equivalent cost of each fixture's calls (see the fixture builder for the token counts).
const CLAUDE: f64 = 0.013040 + 0.003220 + 0.000200 + 0.001012;
const CODEX: f64 = 0.005800 + 0.002475;
const OPENCODE: f64 = 0.003500 + 0.000120 + 0.000225;

/// After every fixture's timestamp, so "lifetime" and "90d" hold everything.
fn now() -> i64 {
    chrono::DateTime::parse_from_rfc3339("2026-10-21T12:00:00Z").unwrap().timestamp_millis()
}

#[test]
fn reads_every_harness_once() {
    let env = Env::new("all");
    let r = env.stats.ingest();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    // Claude: m1 (two lines, one call), m2, m3, m4 (subagent); <synthetic> skipped.
    // Codex: two calls (a repeated token_count and a null one skipped). opencode: two in the
    // database, one in the old layout.
    assert_eq!(r.events_added, 4 + 2 + 3);
    assert_eq!(r.files_read, 2 + 1 + 1 + 1);

    let again = env.stats.ingest();
    assert_eq!((again.files_read, again.events_added), (0, 0), "nothing changed");

    let s = env.stats.stats("lifetime", now());
    assert!(close(s.spend_api, CLAUDE + CODEX + OPENCODE), "{}", s.spend_api);
    assert!(close(s.spend_actual.unwrap(), 0.0125 + 0.001 + 0.0002));
    assert_eq!(s.actual_note.as_deref(), Some("opencode reported"));
    // m1 counts its streamed output once, at the final 200
    let claude_tokens = (10 + 200 + 5000 + 1000) + (5 + 100 + 6000) + 120 + (3 + 30 + 2000);
    let codex_tokens = 1500 + 2200;
    let oc_tokens = 1150 + 20 + 110;
    assert_eq!(s.tokens.total, claude_tokens + codex_tokens + oc_tokens);
    assert_eq!(s.tokens.total, s.tokens.input + s.tokens.output + s.tokens.cache);
    assert_eq!(s.daily.len(), 30);
    assert!(close(s.daily.iter().map(|d| d.cost).sum(), s.spend_api));
    assert_eq!(s.models[0].name, "Opus 5.5");
    assert!(s.models.iter().any(|m| m.name == "GPT-5 Codex"));
    assert!(s.models.len() <= 4);
    let projects: Vec<&str> = s.projects.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(projects[0], "demo");
    assert!(projects.contains(&"codexproj"));
    assert_eq!(s.kinds.len(), 10);
    assert!(close(s.kinds.iter().map(|k| k.share).sum(), 100.0));
}

#[test]
fn turns_get_categories() {
    let env = Env::new("cats");
    env.stats.ingest();
    let s = env.stats.stats("lifetime", now());
    let used: Vec<&str> = s.kinds.iter().filter(|k| k.share > 0.0).map(|k| k.kind.as_str()).collect();
    // the crash prompt on a fix/ branch, the cat-name chat, the subagent's search, the CSV
    // export, the API docs and the old layout's test run
    let mut want = vec!["Bug fix", "Chat", "Discovery", "Build", "Docs", "Test"];
    let mut got = used.clone();
    want.sort();
    got.sort();
    assert_eq!(got, want);
    let d = env.stats.dashboard("lifetime", "category", now());
    assert_eq!(d.series.len(), 6, "top five and Other");
    assert_eq!(d.series[5].name, "Other");
    let kinds = |h: &str| d.sessions.iter().filter(|s| s.harness == h).map(|s| s.kind.clone()).collect::<Vec<_>>();
    assert_eq!(kinds("Claude Code"), ["Bug fix"], "most of the session's tokens went on the fix");
    assert_eq!(kinds("Codex"), ["Build"]);
}

#[test]
fn harness_and_dashboard_views() {
    let env = Env::new("views");
    env.stats.ingest();
    let h = env.stats.harness("lifetime", now());
    assert!(close(h.total_cost, CLAUDE + CODEX + OPENCODE));
    let ids: Vec<&str> = h.harnesses.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["claude-code", "codex", "opencode"], "biggest share first");
    let cc = &h.harnesses[0];
    assert_eq!(cc.name, "Claude Code");
    assert_eq!(cc.sessions, 1, "the subagent is part of its session");
    assert_eq!(cc.top_model, "Opus 5.5");
    assert_eq!(cc.daily.len(), 7);
    // 2 hours from the first prompt's reply to the last reply
    assert_eq!(cc.avg_session_ms, (10 * 3600 + 2 - (9 * 3600 + 5)) * 1000);
    assert!(close(cc.cache_hit, 13000.0 / (13000.0 + 118.0) * 100.0), "{}", cc.cache_hit);
    let oc = h.harnesses.iter().find(|r| r.id == "opencode").unwrap();
    assert_eq!(oc.sessions, 2, "ses1 with its subagent, and the old layout's session");
    assert!(close(h.harnesses.iter().map(|r| r.share).sum(), 100.0));

    let d = env.stats.dashboard("lifetime", "model", now());
    assert_eq!(d.heatmap.len(), 182);
    assert_eq!(d.sessions_total, 4);
    assert!(d.sessions.windows(2).all(|w| w[0].started >= w[1].started), "newest first");
    assert_eq!(d.series.len(), 5, "Opus 5.5, GPT-5 Codex, Sonnet 5.5, Haiku 4.5, GPT-5");
    assert!(d.series.iter().all(|s| s.values.len() == d.days.len()));
    let total: f64 = d.series.iter().flat_map(|s| &s.values).sum();
    assert!(close(total, CLAUDE + CODEX + OPENCODE));
    assert!(d.resets.is_empty() && d.burndown.is_none(), "the app fills those");
    let h = env.stats.dashboard("7d", "harness", now());
    assert_eq!(h.days.len(), 7);
    assert_eq!(h.series[0].name, "Claude Code");
}

#[test]
fn appended_lines_are_read_incrementally() {
    let env = Env::new("append");
    env.stats.ingest();
    let file = env.dir.join("claude/projects/-tmp-demo/s1.jsonl");
    let before = std::fs::read_to_string(&file).unwrap();
    // A half-written line isn't read until it's finished.
    let line = r#"{"type":"assistant","requestId":"r5","uuid":"a5","timestamp":"2026-10-20T10:05:00.000Z","cwd":"/tmp/demo","sessionId":"s1","message":{"id":"m5","model":"claude-opus-5-5","content":[],"usage":{"input_tokens":1000000,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    std::fs::write(&file, format!("{before}{}", &line[..40])).unwrap();
    let r = env.stats.ingest();
    assert_eq!((r.files_read, r.events_added), (1, 0));
    std::fs::write(&file, format!("{before}{line}\n")).unwrap();
    let r = env.stats.ingest();
    assert_eq!((r.files_read, r.events_added), (1, 1));
    let s = env.stats.stats("lifetime", now());
    assert!(close(s.spend_api, CLAUDE + CODEX + OPENCODE + 4.0), "$4 for a million Opus 5.5 input tokens");
    // Rewritten shorter (e.g. compacted): read again from the start without double counting.
    std::fs::write(&file, before.lines().take(3).map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    let r = env.stats.ingest();
    assert_eq!((r.files_read, r.events_added), (1, 0));
    assert!(close(env.stats.stats("lifetime", now()).spend_api, s.spend_api));
}

#[test]
fn reopening_keeps_everything() {
    let env = Env::new("reopen");
    env.stats.ingest();
    let a = env.stats.stats("lifetime", now());
    let roots = Roots { claude: vec![env.dir.join("claude").join("projects")], ..Default::default() };
    let again = Stats::open_with(&env.dir.join("activity.db"), roots).unwrap();
    assert_eq!(again.ingest().files_read, 0, "cursors survive a restart");
    assert_eq!(again.stats("lifetime", now()), a);
}

#[test]
fn nothing_stored_but_counts() {
    let env = Env::new("privacy");
    env.stats.ingest();
    drop(Stats::open_with(&env.dir.join("activity.db"), Roots::default()));
    let c = Connection::open(env.dir.join("activity.db")).unwrap();
    let mut dump = String::new();
    for table in ["events", "turns", "cursors"] {
        let mut q = c.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let n = q.column_count();
        let rows = q.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect::<Vec<_>>().join("|"))).unwrap();
        for r in rows {
            dump.push_str(&r.unwrap());
            dump.push('\n');
        }
    }
    for secret in ["login page", "name for a cat", "session token", "CSV export", "public API", "Run the tests", "synthetic", "Miso"] {
        assert!(!dump.contains(secret), "{secret:?} leaked into the store");
    }
}

#[test]
fn spend_today_matches_the_today_stats() {
    let env = Env::new("today");
    env.stats.ingest();
    let day = |ms: i64| (env.stats.spend_today(ms), env.stats.stats("today", ms).spend_api);
    // Every local "today" from the first fixture day to a week later: the cheap sum and the full
    // stats agree, and at least one of them saw spend.
    let mut seen = 0.0;
    for d in -3..7 {
        let (a, b) = day(now() + d * 86_400_000);
        assert!(close(a, b), "day {d}: {a} vs {b}");
        seen += a;
    }
    assert!(seen > 0.0);
    assert_eq!(env.stats.spend_today(now() + 30 * 86_400_000), 0.0);
}
