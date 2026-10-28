//! Claude Code: <config>/projects/<encoded cwd>/<session>.jsonl, plus
//! <session>/subagents/agent-*.jsonl for subagents (their lines say `isSidechain`).
//!
//! Each API response is written as several lines, one per content block, all with the same
//! `requestId` + `message.id` and the same usage, except that `output_tokens` can grow on the
//! later lines while it streams. So the event key is `requestId:message.id` and the store keeps
//! the largest count it has seen for each.
//!
//! A turn starts at each real user prompt (not tool results, meta lines, task notifications or
//! command output). Its id is the prompt line's `uuid`, so re-reading a file finds the same turns.

use super::{parse_ts, Batch, Event, Tokens};
use crate::model::Harness;
use serde::Deserialize;
use std::borrow::Cow;
use std::path::Path;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Line<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    timestamp: Option<Cow<'a, str>>,
    #[serde(borrow)]
    session_id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    cwd: Option<Cow<'a, str>>,
    #[serde(borrow)]
    git_branch: Option<Cow<'a, str>>,
    #[serde(default)]
    is_sidechain: bool,
    #[serde(borrow)]
    request_id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    uuid: Option<Cow<'a, str>>,
    #[serde(default)]
    is_meta: bool,
    #[serde(default)]
    is_compact_summary: bool,
    #[serde(borrow)]
    permission_mode: Option<Cow<'a, str>>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
    content: Option<Content>,
}

/// A message's content: a plain string or a list of blocks. Hand-written rather than
/// `#[serde(untagged)]`, which would buffer the whole value (tool inputs can be megabytes).
enum Content {
    Text(String),
    Blocks(Vec<Block>),
}

impl<'de> Deserialize<'de> for Content {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Content;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string or a list of content blocks")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Content, E> {
                Ok(Content::Text(v.to_string()))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Content, A::Error> {
                let mut out = Vec::new();
                while let Some(b) = seq.next_element::<Block>()? {
                    out.push(b);
                }
                Ok(Content::Blocks(out))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
    name: Option<String>,
    input: Option<ToolInput>,
}

/// Only the fields that say which file or command a tool touched; the rest is skipped unread.
#[derive(Deserialize, Default)]
struct ToolInput {
    #[serde(default, deserialize_with = "lenient")]
    file_path: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    notebook_path: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    path: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    command: Option<String>,
}

/// A string, or None for anything else (an MCP tool's `command` can be an object).
fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum S {
        Str(String),
        Other(serde::de::IgnoredAny),
    }
    Ok(match S::deserialize(d)? {
        S::Str(s) => Some(s),
        S::Other(_) => None,
    })
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    cache_creation: Option<CacheCreation>,
    output_tokens_details: Option<OutputDetails>,
    speed: Option<String>,
}

#[derive(Deserialize, Default)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

#[derive(Deserialize, Default)]
struct OutputDetails {
    #[serde(default)]
    thinking_tokens: u64,
}

/// Carried between passes over one file.
#[derive(serde::Serialize, Deserialize, Default)]
struct State {
    turn: Option<String>,
}

/// Reads `path` from byte `from`. Returns what it found, the new offset and the new state.
pub fn read(path: &Path, from: u64, state: &str) -> std::io::Result<(Batch, u64, String)> {
    let mut st: State = serde_json::from_str(state).unwrap_or_default();
    let mut b = Batch::default();
    let end = super::read_lines(path, from, |l| line(l, &mut st, &mut b))?;
    Ok((b, end, serde_json::to_string(&st).unwrap_or_default()))
}

fn line(l: &str, st: &mut State, b: &mut Batch) {
    // Cheap filters before parsing: only user and assistant lines matter, and user lines that
    // carry a tool result (often huge) never start a turn. Inside a JSON string a quote is
    // escaped, so these exact byte sequences only occur as real keys/values.
    let assistant = l.contains("\"type\":\"assistant\"");
    if !assistant && (!l.contains("\"type\":\"user\"") || l.contains("\"toolUseResult\":")) {
        return;
    }
    let Ok(d) = serde_json::from_str::<Line>(l) else { return };
    let Some(m) = d.message else { return };
    let session = d.session_id.as_deref().unwrap_or("").to_string();
    match d.kind.as_deref() {
        Some("assistant") => {
            let turn = st.turn.clone().unwrap_or_else(|| format!("cc:{session}:start"));
            let sig = b.turn(&turn);
            if let Some(br) = d.git_branch.as_deref().filter(|b| !b.is_empty() && *b != "HEAD") {
                sig.branch(br);
            }
            if let Some(Content::Blocks(blocks)) = &m.content {
                for bl in blocks.iter().filter(|bl| bl.kind == "tool_use") {
                    let i = bl.input.as_ref();
                    let path = i.and_then(|i| i.file_path.as_deref().or(i.notebook_path.as_deref()).or(i.path.as_deref()));
                    sig.tool(bl.name.as_deref().unwrap_or(""), path, i.and_then(|i| i.command.as_deref()));
                }
            }
            let model = m.model.unwrap_or_default();
            let Some(u) = m.usage else { return };
            if model.is_empty() || model == "<synthetic>" {
                return;
            }
            let (w5, w1) = match &u.cache_creation {
                Some(c) if c.ephemeral_5m_input_tokens + c.ephemeral_1h_input_tokens > 0 => {
                    (c.ephemeral_5m_input_tokens, c.ephemeral_1h_input_tokens)
                }
                _ => (u.cache_creation_input_tokens, 0),
            };
            let tokens = Tokens {
                input: u.input_tokens,
                output: u.output_tokens,
                cache_read: u.cache_read_input_tokens,
                cache_write_5m: w5,
                cache_write_1h: w1,
                reasoning: u.output_tokens_details.map(|o| o.thinking_tokens).unwrap_or(0),
            };
            if tokens.total() == 0 {
                return;
            }
            let key = match (d.request_id.as_deref(), m.id.as_deref()) {
                (Some(r), Some(id)) => format!("cc:{r}:{id}"),
                (None, Some(id)) => format!("cc:{id}"),
                (Some(r), None) => format!("cc:{r}"),
                (None, None) => match d.uuid.as_deref() {
                    Some(u) => format!("cc:{u}"),
                    None => return,
                },
            };
            b.events.push(Event {
                key,
                ts: d.timestamp.as_deref().and_then(parse_ts).unwrap_or(0),
                harness: Harness::ClaudeCode,
                model,
                session_id: session,
                turn_id: Some(turn),
                cwd: d.cwd.as_deref().unwrap_or("").to_string(),
                branch: d.git_branch.as_deref().filter(|b| !b.is_empty()).map(str::to_string),
                sidechain: d.is_sidechain,
                fast: u.speed.as_deref() == Some("fast"),
                tokens,
                cost_actual: None,
                account: None,
            });
        }
        Some("user") => {
            if d.is_meta || d.is_compact_summary {
                return;
            }
            let text = match &m.content {
                Some(Content::Text(t)) => t.clone(),
                Some(Content::Blocks(bs)) => {
                    if bs.iter().any(|b| b.kind == "tool_result") {
                        return;
                    }
                    bs.iter().filter_map(|b| b.text.as_deref()).collect::<Vec<_>>().join("\n")
                }
                None => return,
            };
            if !is_prompt(&text) {
                return;
            }
            let Some(uuid) = d.uuid.as_deref() else { return };
            let turn = format!("cc:{uuid}");
            let sig = b.turn(&turn);
            sig.prompt(&text);
            if d.permission_mode.as_deref() == Some("plan") {
                sig.plan_mode();
            }
            if let Some(br) = d.git_branch.as_deref().filter(|b| !b.is_empty() && *b != "HEAD") {
                sig.branch(br);
            }
            st.turn = Some(turn);
        }
        _ => {}
    }
}

/// A prompt someone typed, as opposed to what Claude Code adds to the transcript: task
/// notifications, system reminders, command output, interruptions. Slash commands count.
fn is_prompt(t: &str) -> bool {
    let t = t.trim_start();
    if t.is_empty() {
        return false;
    }
    if t.starts_with('<') {
        return t.starts_with("<command-name>") || t.starts_with("<command-message>");
    }
    !(t.starts_with("[Request interrupted") || t.starts_with("[SYSTEM"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_prompts_from_transcript_noise() {
        assert!(is_prompt("fix the login bug"));
        assert!(is_prompt("<command-name>/review</command-name>"));
        assert!(is_prompt("[Image #1] what is this"));
        assert!(!is_prompt("<task-notification>done</task-notification>"));
        assert!(!is_prompt("<local-command-stdout>ok</local-command-stdout>"));
        assert!(!is_prompt("[Request interrupted by user]"));
        assert!(!is_prompt("  "));
    }
}
