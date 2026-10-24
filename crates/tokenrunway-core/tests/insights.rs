//! Insights end to end over synthetic Claude Code logs written here (no real prompts): cost per
//! project and branch, a cache tip with its cause, the weekly recap, subscription value and the
//! dashboard filter.

use std::collections::BTreeMap;
use std::path::PathBuf;
use tokenrunway_core::ingest::Roots;
use tokenrunway_core::insights::Subscription;
use tokenrunway_core::stats::{Filter, Stats};

struct Env {
    dir: PathBuf,
    stats: Stats,
}

impl Drop for Env {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// One assistant call: `at` is RFC 3339, usage in tokens.
#[allow(clippy::too_many_arguments)]
fn call(n: &mut u32, session: &str, cwd: &str, branch: &str, at: &str, model: &str, input: u64, read: u64, write: u64) -> String {
    *n += 1;
    format!(
        r#"{{"type":"assistant","requestId":"r{n}","uuid":"a{n}","timestamp":"{at}","cwd":"{cwd}","sessionId":"{session}","gitBranch":"{branch}","message":{{"id":"m{n}","model":"{model}","content":[],"usage":{{"input_tokens":{input},"output_tokens":1000,"cache_read_input_tokens":{read},"cache_creation_input_tokens":{write}}}}}}}"#
    )
}

fn env(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("tr-insights-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let logs = dir.join("claude").join("projects").join("-tmp-x");
    std::fs::create_dir_all(&logs).unwrap();
    let mut n = 0;
    let mut lines = Vec::new();
    let opus = "claude-opus-5-5";
    // alpha: warm cache, two branches, last week (Wed 14 Oct) and the week before (Wed 7 Oct).
    for i in 0..10 {
        lines.push(call(&mut n, "a1", "/tmp/alpha", "main", &format!("2026-10-14T03:{:02}:00.000Z", i), opus, 2_000, 400_000, 10_000));
    }
    for i in 0..4 {
        lines.push(call(&mut n, "a2", "/tmp/alpha", "feat/x", &format!("2026-10-20T03:{:02}:00.000Z", i), opus, 2_000, 300_000, 5_000));
    }
    lines.push(call(&mut n, "a0", "/tmp/alpha", "main", "2026-10-07T03:00:00.000Z", opus, 2_000, 100_000, 5_000));
    // beta: comes back after 20-minute pauses to a cold cache (1M tokens a call, $5+ in all).
    for i in 0..4 {
        lines.push(call(
            &mut n,
            "b1",
            "/tmp/beta",
            "main",
            &format!("2026-10-20T0{}:{:02}:00.000Z", 4 + i / 3, (i % 3) * 20),
            opus,
            900_000,
            10_000,
            100_000,
        ));
    }
    std::fs::write(logs.join("s.jsonl"), lines.join("\n") + "\n").unwrap();
    let roots = Roots { claude: vec![dir.join("claude").join("projects")], codex: vec![], opencode: vec![], lmstudio: vec![] };
    let stats = Stats::open_with(&dir.join("activity.db"), roots).unwrap();
    let r = stats.ingest();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.events_added, n);
    Env { dir, stats }
}

/// Wednesday 21 Oct 2026, midday UTC: last week is Mon 12 to Sun 18 Oct in any time zone.
fn now() -> i64 {
    chrono::DateTime::parse_from_rfc3339("2026-10-21T12:00:00Z").unwrap().timestamp_millis()
}

#[test]
fn projects_branches_and_cache_tip() {
    let e = env("projects");
    let (rows, total, usual, tips) = e.stats.projects("30d", None, now());
    assert_eq!(total, 2);
    let beta = rows.iter().find(|r| r.name == "beta").expect("beta");
    let alpha = rows.iter().find(|r| r.name == "alpha").expect("alpha");
    assert_eq!(rows[0].name, "beta", "biggest cost first");
    assert_eq!((alpha.sessions, alpha.branches_total), (3, 2));
    assert_eq!(alpha.branches[0].name, "main");
    assert!(alpha.cache_hit.unwrap() > 90.0);
    assert!(beta.cache_hit.unwrap() < 5.0);
    assert!(usual.unwrap() > beta.cache_hit.unwrap());
    assert_eq!(tips.len(), 1);
    assert_eq!(tips[0].project, "beta");
    assert_eq!(tips[0].cause, "idle");
    assert!(tips[0].title.starts_with("Sessions in beta get 1% cache hits vs your usual"), "{}", tips[0].title);

    let (only, _, _, _) = e.stats.projects("30d", Some("alpha"), now());
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].branches.len(), 2);
}

#[test]
fn weekly_recap() {
    let e = env("recap");
    let r = e.stats.recap(now()).expect("last week had work");
    assert_eq!((r.from.as_str(), r.to.as_str()), ("2026-10-12", "2026-10-18"));
    assert_eq!(r.sessions, 1);
    assert_eq!(r.top_project.as_ref().unwrap().name, "alpha");
    assert!(r.prev_spend > 0.0 && r.spend > r.prev_spend);
    assert!(r.body.starts_with("Up "), "{}", r.body);
    assert_eq!(r.biggest.as_ref().unwrap().harness, "Claude Code");
}

#[test]
fn subscription_value() {
    let e = env("value");
    let subs = vec![Subscription { account_id: "a1".into(), provider: "anthropic".into(), name: "Claude".into(), plan: "Max 20x".into() }];
    let v = e.stats.plan_value(&subs, &BTreeMap::new(), now());
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].price, Some(200.0));
    assert!(v[0].line.starts_with("Your $200 Max 20x plan did $"), "{}", v[0].line);
    assert!(v[0].line.ends_with("×)"), "{}", v[0].line);
    // An Enterprise seat has no list price until one is set.
    let ent = vec![Subscription { plan: "Enterprise".into(), ..subs[0].clone() }];
    let v = e.stats.plan_value(&ent, &BTreeMap::new(), now());
    assert_eq!(v[0].price, None);
    assert!(v[0].line.ends_with("Add its price in Settings to compare."));
    let prices = BTreeMap::from([("a1".to_string(), 60.0)]);
    assert_eq!(e.stats.plan_value(&ent, &prices, now())[0].price, Some(60.0));
    // Two Claude accounts are counted together.
    let two = vec![subs[0].clone(), Subscription { account_id: "a2".into(), plan: "Pro".into(), ..subs[0].clone() }];
    let v = e.stats.plan_value(&two, &BTreeMap::new(), now());
    assert_eq!((v.len(), v[0].plan.as_str(), v[0].price), (1, "2 Claude plans", Some(220.0)));
    // No local work for a provider: nothing to say.
    let oa = vec![Subscription { provider: "openai".into(), plan: "Plus".into(), ..subs[0].clone() }];
    assert!(e.stats.plan_value(&oa, &BTreeMap::new(), now()).is_empty());
}

#[test]
fn dashboard_and_stats_filters() {
    let e = env("filters");
    let all = e.stats.stats("30d", now());
    let f = Filter { project: Some("alpha".into()), model: None };
    let alpha = e.stats.stats_filtered("30d", &f, now());
    assert!(alpha.spend_api > 0.0 && alpha.spend_api < all.spend_api);
    assert_eq!(alpha.projects.len(), 1);
    let d = e.stats.dashboard_filtered("30d", "model", &f, now());
    assert!(d.sessions.iter().all(|s| s.project == "alpha"));
    assert_eq!(d.sessions_total, 3);
    let m = Filter { project: None, model: Some("Opus 5.5".into()) };
    assert!((e.stats.stats_filtered("30d", &m, now()).spend_api - all.spend_api).abs() < 1e-9);
    let none = Filter { project: None, model: Some("Haiku 5.5".into()) };
    assert_eq!(e.stats.dashboard_filtered("30d", "model", &none, now()).sessions_total, 0);
}
