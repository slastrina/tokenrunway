//! Harness log readers (SPEC.md §5.2). Each reader turns one source (a JSONL file, a SQLite
//! database) into usage events plus per-turn category signals, starting from where the last
//! pass stopped. They keep only counts, model ids, timestamps, the working directory, the branch
//! and the turn's signals; prompts and tool output are read in memory to classify the turn and
//! then dropped.
//!
//! Readers are pure (no store access) so they can run without holding the database lock; the
//! `stats::Stats` pass decides what to read and writes what they return.
//!
//! Not yet read, because there are no logs on the development machine to confirm the formats
//! against (don't guess them):
//! - TODO Gemini CLI: ~/.gemini/tmp/<project hash>/chats/*.json (and telemetry when enabled).
//! - TODO Cline / Roo Code / Kilo: VS Code globalStorage/<extension>/tasks/*/ui_messages.json,
//!   whose `api_req_started` entries carry tokens and cost.

pub mod claude_code;
pub mod codex;
pub mod lmstudio;
pub mod opencode;

use crate::categorise::Signals;
use crate::model::Harness;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// One call's tokens. `input` is uncached input only; `output` includes reasoning, and
/// `reasoning` says how much of it was (it's billed as output, so it's never added twice).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub reasoning: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write_5m + self.cache_write_1h
    }
}

/// One model call.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Stable dedupe key, prefixed by harness ("cc:<requestId>:<messageId>")
    pub key: String,
    /// ms since the epoch
    pub ts: i64,
    pub harness: Harness,
    pub model: String,
    pub session_id: String,
    /// The turn whose category these tokens count towards
    pub turn_id: Option<String>,
    /// The working directory as the reader saw it; the pass turns it into a project name
    pub cwd: String,
    pub branch: Option<String>,
    /// A subagent's call (Claude Code `isSidechain`)
    pub sidechain: bool,
    /// Fast mode (priced higher)
    pub fast: bool,
    pub tokens: Tokens,
    /// What the harness itself says the call cost (opencode), if it says
    pub cost_actual: Option<f64>,
    /// The login the call ran under, when the log says (Codex `session_meta`
    /// `creator_account_id`); otherwise the store works it out from `logins`
    pub account: Option<String>,
}

/// What a reader got from one source in one pass.
#[derive(Debug, Default)]
pub struct Batch {
    pub events: Vec<Event>,
    /// Signals seen this pass, per turn id
    pub turns: HashMap<String, Signals>,
    /// True when `turns` holds each turn's full signals (re-read from scratch), so they replace
    /// what's stored instead of adding to it.
    pub replace_turns: bool,
}

impl Batch {
    pub fn turn(&mut self, id: &str) -> &mut Signals {
        self.turns.entry(id.to_string()).or_default()
    }
}

/// Where each harness keeps its logs.
#[derive(Debug, Clone, Default)]
pub struct Roots {
    /// Claude Code `projects` directories
    pub claude: Vec<PathBuf>,
    /// Codex `sessions` (and `archived_sessions`) directories
    pub codex: Vec<PathBuf>,
    /// opencode data directories (holding opencode.db and/or storage/)
    pub opencode: Vec<PathBuf>,
    /// LM Studio `server-logs` directories
    pub lmstudio: Vec<PathBuf>,
}

impl Roots {
    /// The default locations plus CLAUDE_CONFIG_DIR (comma-separated, like Claude Code) and CODEX_HOME.
    pub fn default_for_this_machine() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let mut claude = Vec::new();
        if let Ok(v) = std::env::var("CLAUDE_CONFIG_DIR") {
            claude.extend(v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| PathBuf::from(s).join("projects")));
        }
        claude.push(home.join(".claude").join("projects"));
        claude.push(dirs::config_dir().unwrap_or_else(|| home.join(".config")).join("claude").join("projects"));
        claude.push(home.join(".config").join("claude").join("projects"));
        let codex_home = std::env::var("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|_| home.join(".codex"));
        let codex = vec![codex_home.join("sessions"), codex_home.join("archived_sessions")];
        let mut opencode = vec![home.join(".local").join("share").join("opencode")];
        if let Some(d) = dirs::data_dir() {
            opencode.push(d.join("opencode"));
        }
        // LM Studio 0.3+ keeps everything in ~/.lmstudio on every platform (LMSTUDIO_HOME moves
        // it); 0.2 used ~/.cache/lm-studio.
        let mut lmstudio = Vec::new();
        if let Ok(v) = std::env::var("LMSTUDIO_HOME") {
            lmstudio.push(PathBuf::from(v).join("server-logs"));
        }
        lmstudio.push(home.join(".lmstudio").join("server-logs"));
        lmstudio.push(home.join(".cache").join("lm-studio").join("server-logs"));
        let mut r = Self { claude, codex, opencode, lmstudio };
        for v in [&mut r.claude, &mut r.codex, &mut r.opencode, &mut r.lmstudio] {
            let mut seen = std::collections::HashSet::new();
            v.retain(|p| seen.insert(p.clone()));
        }
        r
    }

    /// The roots that exist, for the file watcher.
    pub fn existing(&self) -> Vec<PathBuf> {
        self.claude.iter().chain(&self.codex).chain(&self.opencode).chain(&self.lmstudio).filter(|p| p.is_dir()).cloned().collect()
    }
}

/// The file holding the login a Claude Code `projects` directory's sessions run under:
/// `~/.claude.json` beside the default `~/.claude`, `<dir>/.claude.json` for a
/// `CLAUDE_CONFIG_DIR`.
pub fn claude_login_file(projects: &Path) -> Option<PathBuf> {
    let dir = projects.parent()?;
    if dir.file_name().is_some_and(|n| n == ".claude") {
        let beside = dir.parent()?.join(".claude.json");
        if beside.is_file() {
            return Some(beside);
        }
    }
    Some(dir.join(".claude.json"))
}

/// Codex's `auth.json`, beside its `sessions` directory.
pub fn codex_login_file(sessions: &Path) -> Option<PathBuf> {
    Some(sessions.parent()?.join("auth.json"))
}

/// The login a harness is signed in as right now: Claude Code's account uuid
/// (`oauthAccount.accountUuid`), Codex's ChatGPT account id. Ids only, never tokens.
pub fn login_in(harness: &str, file: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(file).ok()?;
    match harness {
        "claude-code" => crate::allowance::claude::account_uuid(&raw),
        "codex" => crate::allowance::codex::account_id(&raw),
        _ => None,
    }
}

/// Where a reader got to in one source.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    pub size: u64,
    pub mtime: i64,
    /// Bytes read (JSONL) or the last `time_updated` seen (opencode)
    pub offset: u64,
    /// The reader's own carry-over (the current turn, model, cwd…), as JSON
    pub state: String,
}

/// A file's size and mtime (ms), the cheap "did it change" check.
pub fn stat(path: &Path) -> Option<(u64, i64)> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
    Some((m.len(), mtime))
}

/// Every file under `root` (recursively) whose name passes `keep`. Symlinked directories aren't
/// followed, so a loop can't trap the walk.
pub fn files_under(root: &Path, keep: &dyn Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() && p.file_name().and_then(|n| n.to_str()).is_some_and(keep) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Calls `f` for each complete line from byte `from`, and returns the offset just past the last
/// complete line. A half-written last line is left for the next pass.
pub fn read_lines(path: &Path, from: u64, mut f: impl FnMut(&str)) -> io::Result<u64> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(from))?;
    let mut r = BufReader::with_capacity(1 << 20, file);
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut pos = from;
    loop {
        buf.clear();
        let n = r.read_until(b'\n', &mut buf)?;
        if n == 0 || buf.last() != Some(&b'\n') {
            return Ok(pos);
        }
        pos += n as u64;
        if let Ok(s) = std::str::from_utf8(&buf) {
            let s = s.trim_end();
            if !s.is_empty() {
                f(s);
            }
        }
    }
}

/// RFC 3339 → ms since the epoch.
pub fn parse_ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}

/// Names the project a working directory belongs to: the git repository's folder if the
/// directory is inside one (a few stat() calls, cached), otherwise the directory's own name.
#[derive(Default)]
pub struct ProjectNames {
    cache: HashMap<String, String>,
    home: Option<PathBuf>,
}

impl ProjectNames {
    pub fn new() -> Self {
        Self { cache: HashMap::new(), home: dirs::home_dir() }
    }

    pub fn name(&mut self, cwd: &str) -> String {
        if let Some(n) = self.cache.get(cwd) {
            return n.clone();
        }
        let n = self.resolve(cwd);
        self.cache.insert(cwd.to_string(), n.clone());
        n
    }

    fn resolve(&self, cwd: &str) -> String {
        if cwd.is_empty() {
            return "unknown".into();
        }
        // Agent worktrees (<repo>/.claude/worktrees/agent-…) belong to their repo, and are often
        // deleted by the time we read the log, so go by the path.
        let cwd = cwd.split_once("/.claude/worktrees/").map(|(repo, _)| repo).unwrap_or(cwd);
        let p = Path::new(cwd);
        for dir in p.ancestors() {
            if self.home.as_deref() == Some(dir) || dir.parent().is_none() {
                break;
            }
            let git = dir.join(".git");
            if git.is_dir() {
                return last_component(dir);
            }
            if git.is_file() {
                // A linked worktree: ".git" says "gitdir: <repo>/.git/worktrees/<name>".
                let main = std::fs::read_to_string(&git).ok().and_then(|s| {
                    let g = s.trim().strip_prefix("gitdir:")?.trim().to_string();
                    let (repo, _) = g.split_once("/.git/worktrees/")?;
                    Some(last_component(Path::new(repo)))
                });
                return main.unwrap_or_else(|| last_component(dir));
            }
        }
        last_component(p)
    }
}

fn last_component(p: &Path) -> String {
    p.file_name().and_then(|n| n.to_str()).map(str::to_string).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Shell-ish text from a JSON value: a string, or an argv array joined with spaces.
pub(crate) fn command_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(a) => Some(a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")),
        _ => None,
    }
}

/// The files an apply_patch body touches ("*** Update File: src/a.rs").
pub(crate) fn patch_paths(patch: &str) -> Vec<&str> {
    patch
        .lines()
        .filter_map(|l| {
            l.strip_prefix("*** Update File: ").or_else(|| l.strip_prefix("*** Add File: ")).or_else(|| l.strip_prefix("*** Delete File: "))
        })
        .map(str::trim)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_complete_lines() {
        let dir = std::env::temp_dir().join(format!("tr-lines-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.jsonl");
        std::fs::write(&p, "one\ntwo\nthr").unwrap();
        let mut seen = Vec::new();
        let off = read_lines(&p, 0, |l| seen.push(l.to_string())).unwrap();
        assert_eq!(seen, ["one", "two"]);
        assert_eq!(off, 8);
        std::fs::write(&p, "one\ntwo\nthree\n").unwrap();
        seen.clear();
        let off = read_lines(&p, off, |l| seen.push(l.to_string())).unwrap();
        assert_eq!(seen, ["three"]);
        assert_eq!(off, 14);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn project_names_prefer_the_git_root() {
        let dir = std::env::temp_dir().join(format!("tr-proj-{}", std::process::id()));
        let sub = dir.join("myrepo").join("src").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(dir.join("myrepo").join(".git")).unwrap();
        let mut n = ProjectNames::new();
        assert_eq!(n.name(sub.to_str().unwrap()), "myrepo");
        assert_eq!(n.name("/no/such/place/thing"), "thing");
        assert_eq!(n.name("/gone/rattleboard/.claude/worktrees/agent-a1b2"), "rattleboard");
        let wt = dir.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: /src/mainrepo/.git/worktrees/wt\n").unwrap();
        assert_eq!(n.name(wt.to_str().unwrap()), "mainrepo");
        assert_eq!(n.name(""), "unknown");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finds_patch_paths() {
        let p = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-x\n+y\n*** Add File: docs/b.md\n*** End Patch";
        assert_eq!(patch_paths(p), ["src/a.rs", "docs/b.md"]);
    }
}
