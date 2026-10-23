//! Codex CLI / app: <CODEX_HOME>/sessions/YYYY/MM/DD/rollout-*.jsonl, one file per session.
//!
//! Lines are `{timestamp, type, payload}`. What we use:
//! - `session_meta`: the session id, cwd and git branch
//! - `turn_context`: the turn id, model and cwd for the calls that follow
//! - `event_msg` / `token_count`: `info.last_token_usage` is one call's usage (`input_tokens`
//!   includes `cached_input_tokens`; `output_tokens` includes `reasoning_output_tokens`).
//!   `info` is null on the rate-limit-only updates, and an update can repeat, so the key is the
//!   session plus the running total (`total_token_usage.total_tokens`), which only moves when
//!   there's a new call.
//! - `response_item` user messages (the prompt) and function / custom tool calls (shell
//!   commands, apply_patch) for the turn's category.
//!
//! Rollouts from before the `payload` wrapper (early 2025) aren't read.

use super::{command_text, parse_ts, patch_paths, Batch, Event, Tokens};
use crate::categorise::Signals;
use crate::model::Harness;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// Carried between passes over one file.
#[derive(Serialize, Deserialize, Default)]
struct State {
    session: String,
    turn: Option<String>,
    model: String,
    cwd: String,
    branch: Option<String>,
    last_total: u64,
}

/// Lines we never need, skipped before parsing (tool output and encrypted reasoning are the bulk).
const SKIP: [&str; 6] = [
    "\"type\":\"function_call_output\"",
    "\"type\":\"custom_tool_call_output\"",
    "\"type\":\"reasoning\"",
    "\"type\":\"world_state\"",
    "\"type\":\"token_usage_record\"",
    "\"type\":\"item_completed\"",
];

pub fn read(path: &Path, from: u64, state: &str) -> std::io::Result<(Batch, u64, String)> {
    let mut st: State = serde_json::from_str(state).unwrap_or_default();
    if st.session.is_empty() {
        st.session = session_from_name(path);
    }
    let mut b = Batch::default();
    let end = super::read_lines(path, from, |l| line(l, &mut st, &mut b))?;
    Ok((b, end, serde_json::to_string(&st).unwrap_or_default()))
}

/// "rollout-2026-09-21T13-47-56-<uuid>.jsonl" → the uuid, until session_meta says.
fn session_from_name(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    // the uuid is the last five dash-separated groups
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() > 5 {
        parts[parts.len() - 5..].join("-")
    } else {
        stem.to_string()
    }
}

fn line(l: &str, st: &mut State, b: &mut Batch) {
    if SKIP.iter().any(|s| l.contains(s)) {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(l) else { return };
    let p = &v["payload"];
    let ts = v["timestamp"].as_str().and_then(parse_ts).unwrap_or(0);
    match v["type"].as_str() {
        Some("session_meta") => {
            if let Some(id) = p["id"].as_str().or(p["session_id"].as_str()) {
                st.session = id.to_string();
            }
            if let Some(c) = p["cwd"].as_str() {
                st.cwd = c.to_string();
            }
            if let Some(br) = p["git"]["branch"].as_str() {
                st.branch = Some(br.to_string());
            }
        }
        Some("turn_context") => {
            if let Some(t) = p["turn_id"].as_str() {
                st.turn = Some(format!("cx:{t}"));
            }
            if let Some(m) = p["model"].as_str() {
                st.model = m.to_string();
            }
            if let Some(c) = p["cwd"].as_str() {
                st.cwd = c.to_string();
            }
        }
        Some("event_msg") if p["type"] == "token_count" => {
            let info = &p["info"];
            if info.is_null() {
                return;
            }
            let last = &info["last_token_usage"];
            let n = |k: &str| last[k].as_u64().unwrap_or(0);
            let total = info["total_token_usage"]["total_tokens"].as_u64().unwrap_or(0);
            if last.is_null() || (total != 0 && total == st.last_total) {
                return;
            }
            st.last_total = total;
            let (input, cached, write) = (n("input_tokens"), n("cached_input_tokens"), n("cache_write_input_tokens"));
            let tokens = Tokens {
                input: input.saturating_sub(cached).saturating_sub(write),
                output: n("output_tokens"),
                cache_read: cached,
                cache_write_5m: write,
                cache_write_1h: 0,
                reasoning: n("reasoning_output_tokens"),
            };
            if tokens.total() == 0 {
                return;
            }
            let turn = current_turn(st, ts);
            b.events.push(Event {
                key: format!("cx:{}:{}", st.session, if total != 0 { total.to_string() } else { ts.to_string() }),
                ts,
                harness: Harness::Codex,
                model: if st.model.is_empty() { "unknown".into() } else { st.model.clone() },
                session_id: st.session.clone(),
                turn_id: Some(turn),
                cwd: st.cwd.clone(),
                branch: st.branch.clone(),
                sidechain: false,
                fast: false,
                tokens,
                cost_actual: None,
            });
        }
        Some("response_item") => {
            let turn = current_turn(st, ts);
            let branch = st.branch.clone();
            let sig = b.turn(&turn);
            if let Some(br) = &branch {
                sig.branch(br);
            }
            match p["type"].as_str() {
                Some("message") if p["role"] == "user" => {
                    let text: Vec<&str> = p["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect();
                    let text = text.join("\n");
                    let t = text.trim_start();
                    // Codex sends its environment and AGENTS.md as user messages too.
                    if !t.is_empty() && !t.starts_with('<') && !t.starts_with("# AGENTS.md") {
                        sig.prompt(&text);
                    }
                }
                Some("function_call") => {
                    let name = p["name"].as_str().unwrap_or("");
                    let args: Value = p["arguments"].as_str().and_then(|a| serde_json::from_str(a).ok()).unwrap_or(Value::Null);
                    let cmd = command_text(&args["command"]).or_else(|| command_text(&args["cmd"]));
                    tool(sig, name, &args, cmd.as_deref());
                }
                Some("custom_tool_call") | Some("local_shell_call") => {
                    let name = p["name"].as_str().unwrap_or("shell");
                    let input = p["input"].as_str().map(str::to_string).or_else(|| command_text(&p["action"]["command"]));
                    tool(sig, name, &Value::Null, input.as_deref());
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn current_turn(st: &mut State, ts: i64) -> String {
    st.turn.get_or_insert_with(|| format!("cx:{}:{ts}", st.session)).clone()
}

/// apply_patch arrives as its own tool or inside a shell command; either way it's an edit of
/// the files it names, not a command.
fn tool(sig: &mut Signals, name: &str, args: &Value, cmd: Option<&str>) {
    let patch = cmd.filter(|c| c.contains("*** Begin Patch"));
    if name == "apply_patch" || patch.is_some() {
        let paths = patch.or(cmd).map(patch_paths).unwrap_or_default();
        if paths.is_empty() {
            sig.tool("apply_patch", None, None);
        }
        for p in paths {
            sig.tool("apply_patch", Some(p), None);
        }
        return;
    }
    let path = args["path"].as_str().or(args["file_path"].as_str());
    sig.tool(name, path, cmd);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_from_file_name() {
        let p = Path::new("/x/rollout-2026-09-21T13-47-56-01a0c213-dc63-7a50-bb31-76059fd798d3.jsonl");
        assert_eq!(session_from_name(p), "01a0c213-dc63-7a50-bb31-76059fd798d3");
    }
}
