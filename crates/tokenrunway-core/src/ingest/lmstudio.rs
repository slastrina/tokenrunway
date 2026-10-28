//! LM Studio: ~/.lmstudio/server-logs/YYYY-MM/YYYY-MM-DD.N.log (older versions:
//! ~/.cache/lm-studio/server-logs), plain-text logs that rotate at ~10 MB (`.1.log`, `.2.log`…).
//!
//! Lines start `[YYYY-MM-DD HH:MM:SS][LEVEL]` in local time; continuation lines have no stamp, so
//! the last one carries over. What we use, nothing else (DEBUG lines can hold prompt text, which
//! is never kept):
//! - `[INFO][<model id>] …`: the model the following request runs on ("qwen/qwen3.6-35b-a3b",
//!   "qwen3.5-9b-…:2" for a second instance)
//! - `load_model: loading model '<…>/models/<publisher>/<repo>/<file>.gguf'` (llama.cpp) and
//!   `Loading model from <…>/models/<publisher>/<repo>` (MLX): the model, when no request names it
//! - llama.cpp's `slot print_timing: id S | task T | prompt eval time = … / N tokens` (new prompt
//!   tokens, i.e. uncached input) then `eval time = … / M tokens` (output), one pair per request
//! - `slot release: id S | task T | stop processing: n_tokens = K`: the slot's whole context
//!   after the request, so K − N − M is the prompt prefix it reused (a cache read)
//!
//! The MLX engine logs no token counts for streamed requests, so those aren't counted. Local
//! models cost nothing: they're stored as "lmstudio/<id>", which pricing never prices.

use super::{Batch, Event, Tokens};
use crate::model::Harness;
use chrono::{Local, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A new session after this long without a request on the same model.
const IDLE_MS: i64 = 30 * 60 * 1000;
/// How much of the previous rotation to look at for the model in use.
const TAIL_BYTES: u64 = 2 << 20;
/// The prefix that marks a local model (pricing::lookup never prices it).
pub const MODEL_PREFIX: &str = "lmstudio/";
/// The project LM Studio's requests are filed under: the logs don't say which client asked.
pub const PROJECT: &str = "LM Studio";

/// Carried between passes over one file.
#[derive(Serialize, Deserialize, Default, Debug)]
struct State {
    /// The last timestamp seen (ms)
    ts: i64,
    /// The model in use, cleaned ("qwen3.6-35b-a3b")
    model: String,
    session: String,
    session_model: String,
    session_last: i64,
    /// The request between its prompt eval line and its release
    pending: Option<Pending>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Pending {
    key: String,
    slot: i64,
    task: i64,
    ts: i64,
    model: String,
    session: String,
    input: u64,
    output: u64,
}

/// `YYYY-MM-DD.N.log`
pub fn is_log(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".log") else { return false };
    let Some((date, n)) = stem.rsplit_once('.') else { return false };
    date.len() == 10 && date.as_bytes()[4] == b'-' && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
}

pub fn read(path: &Path, from: u64, state: &str) -> std::io::Result<(Batch, u64, String)> {
    let mut st: State = serde_json::from_str(state).unwrap_or_default();
    if from == 0 && st.model.is_empty() {
        // A rotated file starts mid-run: the model is whatever the previous part last named.
        st.model = previous_part(path).and_then(|p| last_model(&p)).unwrap_or_default();
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let mut b = Batch::default();
    let end = super::read_lines(path, from, |l| line(l, &stem, &mut st, &mut b))?;
    Ok((b, end, serde_json::to_string(&st).unwrap_or_default()))
}

fn line(l: &str, stem: &str, st: &mut State, b: &mut Batch) {
    if let Some(ts) = stamp(l) {
        st.ts = ts;
    }
    if l.contains("print_timing:") {
        timing(l, stem, st, b);
    } else if l.contains("stop processing: n_tokens") {
        release(l, st, b);
    } else if let Some(m) = info_model(l).or_else(|| loaded_model(l)) {
        st.model = m;
    }
}

/// "[2026-10-11 17:15:00][DEBUG] …" → ms, read as local time.
fn stamp(l: &str) -> Option<i64> {
    let s = l.strip_prefix('[')?.get(..19)?;
    if l.as_bytes().get(20) != Some(&b']') {
        return None;
    }
    let t = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()?;
    Local.from_local_datetime(&t).earliest().map(|d| d.timestamp_millis())
}

/// "[…][INFO][qwen/qwen3.6-35b-a3b] Running chat completion…" → "qwen3.6-35b-a3b".
fn info_model(l: &str) -> Option<String> {
    let rest = l.get(21..)?.strip_prefix("[INFO][")?;
    let (tag, _) = rest.split_once(']')?;
    if tag.is_empty() || tag == "LM STUDIO SERVER" || tag.starts_with("Plugin(") || tag.contains(' ') {
        return None;
    }
    // A second instance is "<id>:2".
    let tag = match tag.rsplit_once(':') {
        Some((id, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => id,
        _ => tag,
    };
    let id = tag.rsplit('/').next().unwrap_or(tag).trim().to_ascii_lowercase();
    (!id.is_empty()).then_some(id)
}

/// A model load line → the repo's name, the way LM Studio names the model
/// ("…/mradermacher/Qwen3.5-27B-heretic-GGUF/Qwen3.5-27B-heretic.Q4_K_S.gguf" → "qwen3.5-27b-heretic").
fn loaded_model(l: &str) -> Option<String> {
    let path = if let Some((_, rest)) = l.split_once("load_model: loading model '") {
        let p = rest.split('\'').next()?;
        Path::new(p).parent()?.to_path_buf()
    } else if let Some((_, rest)) = l.split_once("Loading model from ") {
        PathBuf::from(rest.trim())
    } else {
        return None;
    };
    let repo = path.file_name()?.to_str()?;
    Some(clean_repo(repo)).filter(|s| !s.is_empty())
}

fn clean_repo(repo: &str) -> String {
    let mut s = repo.trim().to_ascii_lowercase();
    if let Some(i) = s.find("-mlx") {
        s.truncate(i);
    }
    for suffix in ["-gguf", "-ggml"] {
        if let Some(rest) = s.strip_suffix(suffix) {
            s = rest.to_string();
        }
    }
    s
}

/// " id  3 | task 140 | " → (3, 140)
fn slot_task(l: &str) -> Option<(i64, i64)> {
    let (_, rest) = l.split_once(" id ")?;
    let (slot, rest) = rest.split_once('|')?;
    let (_, rest) = rest.split_once("task ")?;
    let (task, _) = rest.split_once('|').unwrap_or((rest, ""));
    Some((slot.trim().parse().ok()?, task.trim().parse().ok()?))
}

/// "… time =  231.93 ms /    33 tokens (…" → 33
fn tokens_after(l: &str, label: &str) -> Option<u64> {
    let (_, rest) = l.split_once(label)?;
    let (_, rest) = rest.split_once('/')?;
    let (n, _) = rest.trim_start().split_once(' ')?;
    n.parse().ok()
}

fn timing(l: &str, stem: &str, st: &mut State, b: &mut Batch) {
    let Some((slot, task)) = slot_task(l) else { return };
    if let Some(n) = tokens_after(l, "prompt eval time =") {
        st.pending = None;
        let model = if st.model.is_empty() { "unknown".to_string() } else { st.model.clone() };
        if st.session.is_empty() || st.session_model != model || st.ts - st.session_last > IDLE_MS {
            st.session = format!("lms:{stem}:{}", st.ts);
            st.session_model = model.clone();
        }
        st.session_last = st.ts;
        st.pending = Some(Pending {
            key: format!("lms:{stem}:{}:{slot}:{task}", st.ts),
            slot,
            task,
            ts: st.ts,
            model,
            session: st.session.clone(),
            input: n,
            output: 0,
        });
    } else if let Some(m) = tokens_after(l, " eval time =") {
        let Some(p) = st.pending.as_mut().filter(|p| p.slot == slot && p.task == task) else { return };
        p.output = m;
        b.events.push(event(p, 0));
    }
}

fn release(l: &str, st: &mut State, b: &mut Batch) {
    let Some((slot, task)) = slot_task(l) else { return };
    let Some(p) = st.pending.take() else { return };
    if p.slot != slot || p.task != task || p.output == 0 {
        st.pending = Some(p);
        return;
    }
    let n_ctx =
        l.split_once("n_tokens =").and_then(|(_, r)| r.trim_start().split(|c: char| !c.is_ascii_digit()).next()?.parse::<u64>().ok());
    if let Some(cached) = n_ctx.map(|n| n.saturating_sub(p.input + p.output)).filter(|c| *c > 0) {
        // The same key again: the store keeps the larger counts.
        b.events.push(event(&p, cached));
    }
}

fn event(p: &Pending, cache_read: u64) -> Event {
    Event {
        key: p.key.clone(),
        ts: p.ts,
        harness: Harness::Lmstudio,
        model: format!("{MODEL_PREFIX}{}", p.model),
        session_id: p.session.clone(),
        turn_id: None,
        cwd: PROJECT.into(),
        branch: None,
        sidechain: false,
        fast: false,
        tokens: Tokens { input: p.input, output: p.output, cache_read, ..Default::default() },
        cost_actual: None,
        account: None,
    }
}

/// "2026-08-19.3.log" → (2026-08-19, 3), for ordering parts across days.
fn order(path: &Path) -> Option<(String, u32)> {
    let name = path.file_name()?.to_str()?;
    let (date, n) = name.strip_suffix(".log")?.rsplit_once('.')?;
    Some((date.to_string(), n.parse().ok()?))
}

/// The log written just before this one: the previous part of the day, or the last part of an
/// earlier day (a server that kept running past midnight). Logs live in
/// <server-logs>/<YYYY-MM>/, so the walk starts two levels up.
fn previous_part(path: &Path) -> Option<PathBuf> {
    let me = order(path)?;
    let root = path.parent()?.parent()?;
    super::files_under(root, &is_log)
        .into_iter()
        .filter_map(|p| Some((order(&p)?, p)))
        .filter(|(o, _)| *o < me)
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, p)| p)
}

/// The last model named near the end of a log.
fn last_model(path: &Path) -> Option<String> {
    let len = std::fs::metadata(path).ok()?.len();
    let mut model = None;
    super::read_lines(path, len.saturating_sub(TAIL_BYTES), |l| {
        if let Some(m) = info_model(l).or_else(|| loaded_model(l)) {
            model = Some(m);
        }
    })
    .ok()?;
    model
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(is_log("2026-10-22.1.log"));
        assert!(is_log("2026-08-19.11.log"));
        assert!(!is_log("2026-10-22.log"));
        assert!(!is_log("server-logs-state.json"));
        assert_eq!(
            info_model("[2026-10-11 17:15:00][INFO][qwen/qwen3.6-35b-a3b] Running chat completion").as_deref(),
            Some("qwen3.6-35b-a3b")
        );
        assert_eq!(info_model("[2026-10-11 17:15:00][INFO][qwen3.5-9b-x:2] Streaming response...").as_deref(), Some("qwen3.5-9b-x"));
        assert_eq!(info_model("[2026-10-11 17:15:00][INFO][LM STUDIO SERVER] Client disconnected."), None);
        assert_eq!(info_model("[2026-10-11 17:15:00][INFO] Returning {"), None);
        assert_eq!(
            loaded_model("[2026-10-11 17:15:27][DEBUG] 0.00.043.215 I srv    load_model: loading model '/u/.lmstudio/models/mradermacher/Qwen3.5-27B-heretic-GGUF/Qwen3.5-27B-heretic.Q4_K_S.gguf'").as_deref(),
            Some("qwen3.5-27b-heretic")
        );
        assert_eq!(
            loaded_model("[2026-08-02 10:00:00][DEBUG][batched_model_kit][INFO]: Loading model from /u/.lmstudio/models/lmstudio-community/Qwen3-Coder-Next-MLX-4bit").as_deref(),
            Some("qwen3-coder-next")
        );
        assert_eq!(
            slot_task("0.26.808.914 I slot print_timing: id  3 | task 0 |        eval time =    1497.27 ms /   125 tokens"),
            Some((3, 0))
        );
        assert_eq!(
            tokens_after("I slot print_timing: id  3 | task 0 | prompt eval time =     231.93 ms /    33 tokens (", "prompt eval time ="),
            Some(33)
        );
    }
}
