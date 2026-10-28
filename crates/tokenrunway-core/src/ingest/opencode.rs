//! opencode: <data>/opencode/opencode.db (SQLite, opened read-only), and the older layout of one
//! JSON file per message under storage/message/<session>/ with its parts under
//! storage/part/<message>/.
//!
//! Each assistant message carries `modelID`, `tokens` {input, output, reasoning, cache {read,
//! write}} and `cost`, opencode's own price for it, which we keep as the reported cost. Here
//! `input` excludes cache reads and `output` excludes reasoning, so reasoning is added to output.
//! A turn is a user message; assistant messages point at theirs with `parentID`. Subagent
//! sessions (`parent_id`) count towards their root session.
//!
//! Messages and parts change after they're written (a tool part goes pending → running →
//! completed), so a changed session is re-read whole and its turns' signals replace the stored
//! ones rather than adding to them.

use super::{Batch, Event, Tokens};
use crate::model::Harness;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Reads the sessions with anything updated at or after `since` (ms). Returns the batch and the
/// newest `time_updated` seen.
pub fn read_db(db: &Path, since: u64) -> Result<(Batch, u64), String> {
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| format!("opencode.db: {e}"))?;
    c.busy_timeout(std::time::Duration::from_secs(2)).ok();
    let err = |e: rusqlite::Error| format!("opencode.db: {e}");
    let since = since as i64;

    // id → (parent, directory), small enough to hold whole
    let mut sessions: HashMap<String, (Option<String>, String)> = HashMap::new();
    let mut q = c.prepare("SELECT id, parent_id, directory FROM session").map_err(err)?;
    let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?))).map_err(err)?;
    for r in rows.flatten() {
        sessions.insert(r.0, (r.1, r.2));
    }

    let mut touched: Vec<String> = Vec::new();
    let mut newest = since;
    let mut q = c
        .prepare(
            "SELECT session_id, MAX(time_updated) FROM message WHERE time_updated >= ?1 GROUP BY session_id
             UNION ALL SELECT session_id, MAX(time_updated) FROM part WHERE time_updated >= ?1 GROUP BY session_id",
        )
        .map_err(err)?;
    let rows = q.query_map([since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).map_err(err)?;
    for (s, t) in rows.flatten() {
        newest = newest.max(t);
        if !touched.contains(&s) {
            touched.push(s);
        }
    }

    let mut out = Batch { replace_turns: true, ..Default::default() };
    let mut mq = c.prepare("SELECT id, data FROM message WHERE session_id = ?1 ORDER BY time_created, id").map_err(err)?;
    let mut pq = c.prepare("SELECT message_id, data FROM part WHERE session_id = ?1 ORDER BY time_created, id").map_err(err)?;
    for s in touched {
        let msgs: Vec<(String, Value)> = mq
            .query_map([&s], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(err)?
            .flatten()
            .filter_map(|(id, d)| Some((id, serde_json::from_str(&d).ok()?)))
            .collect();
        let parts: Vec<(String, Value)> = pq
            .query_map([&s], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(err)?
            .flatten()
            .filter_map(|(id, d)| Some((id, serde_json::from_str(&d).ok()?)))
            .collect();
        let dir = sessions.get(&s).map(|x| x.1.clone()).unwrap_or_default();
        build(&root(&sessions, &s), &dir, &msgs, &parts, &mut out);
    }
    Ok((out, newest.max(0) as u64))
}

/// The top of a session's parent chain.
fn root(sessions: &HashMap<String, (Option<String>, String)>, id: &str) -> String {
    let mut cur = id.to_string();
    for _ in 0..16 {
        match sessions.get(&cur).and_then(|s| s.0.clone()) {
            Some(p) if !p.is_empty() => cur = p,
            _ => break,
        }
    }
    cur
}

/// The older layout: one session's directory under storage/message, with parts under
/// storage/part/<message id>/.
pub fn read_storage_session(session_dir: &Path, part_root: &Path) -> Batch {
    let mut msgs = Vec::new();
    let mut parts = Vec::new();
    for f in super::files_under(session_dir, &|n| n.ends_with(".json")) {
        let Some(v) = std::fs::read(&f).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        let id = v["id"].as_str().map(str::to_string).unwrap_or_else(|| f.file_stem().unwrap_or_default().to_string_lossy().into_owned());
        for pf in super::files_under(&part_root.join(&id), &|n| n.ends_with(".json")) {
            if let Some(p) = std::fs::read(&pf).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
                parts.push((id.clone(), p));
            }
        }
        msgs.push((id, v));
    }
    msgs.sort_by_key(|(_, v)| v["time"]["created"].as_i64().unwrap_or(0));
    let session = session_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out = Batch { replace_turns: true, ..Default::default() };
    build(&session, "", &msgs, &parts, &mut out);
    out
}

/// One session's messages and parts → events and turn signals.
fn build(session: &str, dir: &str, msgs: &[(String, Value)], parts: &[(String, Value)], out: &mut Batch) {
    // message id → its turn (the user message it answers)
    let mut turn_of: HashMap<&str, String> = HashMap::new();
    for (id, m) in msgs {
        let t = match m["role"].as_str() {
            Some("user") => format!("oc:{id}"),
            _ => match m["parentID"].as_str() {
                Some(p) => format!("oc:{p}"),
                None => format!("oc:{session}:start"),
            },
        };
        out.turn(&t);
        turn_of.insert(id.as_str(), t);
    }
    for (mid, p) in parts {
        let Some(t) = turn_of.get(mid.as_str()) else { continue };
        let sig = out.turns.entry(t.clone()).or_default();
        match p["type"].as_str() {
            Some("text") if p["synthetic"] != true => {
                let user = msgs.iter().any(|(id, m)| id == mid && m["role"] == "user");
                if let (true, Some(text)) = (user, p["text"].as_str()) {
                    sig.prompt(text);
                }
            }
            Some("tool") => {
                let input = &p["state"]["input"];
                let path = input["filePath"].as_str().or(input["path"].as_str());
                sig.tool(p["tool"].as_str().unwrap_or(""), path, input["command"].as_str());
            }
            Some("patch") => {
                for f in p["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str()) {
                    sig.tool("patch", Some(f), None);
                }
            }
            _ => {}
        }
    }
    for (id, m) in msgs {
        if m["role"] != "assistant" {
            continue;
        }
        let t = &m["tokens"];
        let n = |v: &Value| v.as_u64().unwrap_or(0);
        let reasoning = n(&t["reasoning"]);
        let tokens = Tokens {
            input: n(&t["input"]),
            output: n(&t["output"]) + reasoning,
            cache_read: n(&t["cache"]["read"]),
            cache_write_5m: n(&t["cache"]["write"]),
            cache_write_1h: 0,
            reasoning,
        };
        if tokens.total() == 0 {
            continue;
        }
        let cwd = m["path"]["cwd"].as_str().filter(|s| !s.is_empty()).unwrap_or(dir);
        out.events.push(Event {
            key: format!("oc:{id}"),
            ts: m["time"]["created"].as_i64().unwrap_or(0),
            harness: Harness::Opencode,
            model: m["modelID"].as_str().unwrap_or("unknown").to_string(),
            session_id: session.to_string(),
            turn_id: turn_of.get(id.as_str()).cloned(),
            cwd: cwd.to_string(),
            branch: None,
            sidechain: false,
            fast: false,
            tokens,
            cost_actual: m["cost"].as_f64(),
            account: None,
        });
    }
}
