//! Work categories (SPEC.md §10.4): what a turn (one user prompt and the agent work that follows
//! it) was for. Rule-based and local. The readers feed a turn's prompt, tool calls, files and
//! commands into its `Signals` as they read them; only the counts are kept (and stored), never
//! the prompt or the tool output. `classify` scores the counts with the weights in
//! categories.json and picks the best category, with a confidence the UI can show as "≈".
//!
//! TODO: the spec's "split the tokens when two scores are within 15%" — each turn goes wholly to
//! its winner for now.

use crate::model::Category;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

const BUNDLED: &str = include_str!("categories.json");

/// All ten, in a fixed order (the index into `Signals::kw`).
pub const ALL: [Category; 10] = [
    Category::Build,
    Category::Bugfix,
    Category::Discovery,
    Category::Refactor,
    Category::Test,
    Category::Review,
    Category::Docs,
    Category::Ops,
    Category::Planning,
    Category::Chat,
];

/// What the UI shows.
pub fn label(c: Category) -> &'static str {
    match c {
        Category::Build => "Build",
        Category::Bugfix => "Bug fix",
        Category::Discovery => "Discovery",
        Category::Refactor => "Refactor",
        Category::Test => "Test",
        Category::Review => "Review",
        Category::Docs => "Docs",
        Category::Ops => "Ops",
        Category::Planning => "Planning",
        Category::Chat => "Chat",
    }
}

/// What the store keeps ("bugfix"), the same as the serde name.
pub fn id(c: Category) -> &'static str {
    match c {
        Category::Build => "build",
        Category::Bugfix => "bugfix",
        Category::Discovery => "discovery",
        Category::Refactor => "refactor",
        Category::Test => "test",
        Category::Review => "review",
        Category::Docs => "docs",
        Category::Ops => "ops",
        Category::Planning => "planning",
        Category::Chat => "chat",
    }
}

pub fn from_id(s: &str) -> Option<Category> {
    ALL.into_iter().find(|c| id(*c) == s)
}

fn index(c: Category) -> usize {
    ALL.iter().position(|x| *x == c).unwrap_or(0)
}

#[derive(Deserialize)]
struct RawRules {
    keywords: HashMap<String, Vec<String>>,
    branches: HashMap<String, String>,
    tools: HashMap<String, Vec<String>>,
    commands: HashMap<String, Vec<String>>,
    paths: HashMap<String, Vec<String>>,
    weights: Weights,
}

#[derive(Deserialize, Clone)]
struct Weights {
    keyword: f64,
    stack_trace: f64,
    branch: f64,
    edit: f64,
    read: f64,
    web: f64,
    command: f64,
    todo: f64,
    plan_mode: f64,
    many_files: f64,
    many_files_at: usize,
    no_tools_chat: f64,
}

/// categories.json, ready to match against.
struct Rules {
    /// One-word keywords → category index
    words: HashMap<String, Vec<usize>>,
    /// Multi-word keywords ("clean up") → category index
    phrases: Vec<(Vec<String>, usize)>,
    branches: HashMap<String, Category>,
    tools: HashMap<String, ToolKind>,
    commands: Vec<(String, Vec<String>)>,
    paths: Vec<(String, Vec<String>)>,
    w: Weights,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    Edit,
    Read,
    Web,
    Shell,
    Todo,
    Plan,
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| {
        let raw: RawRules = serde_json::from_str(BUNDLED).expect("bundled categories.json is valid");
        let mut words: HashMap<String, Vec<usize>> = HashMap::new();
        let mut phrases = Vec::new();
        for (cat, list) in &raw.keywords {
            let Some(c) = from_id(cat) else { continue };
            for k in list {
                let parts: Vec<String> = k.split_whitespace().map(str::to_lowercase).collect();
                if parts.len() == 1 {
                    words.entry(parts[0].clone()).or_default().push(index(c));
                } else if parts.len() > 1 {
                    phrases.push((parts, index(c)));
                }
            }
        }
        let branches = raw.branches.iter().filter_map(|(p, c)| Some((p.to_lowercase(), from_id(c)?))).collect();
        let mut tools = HashMap::new();
        for (kind, list) in &raw.tools {
            let k = match kind.as_str() {
                "edit" => ToolKind::Edit,
                "read" => ToolKind::Read,
                "web" => ToolKind::Web,
                "shell" => ToolKind::Shell,
                "todo" => ToolKind::Todo,
                "plan" => ToolKind::Plan,
                _ => continue,
            };
            for t in list {
                tools.insert(t.to_lowercase(), k);
            }
        }
        // Fixed check order: the first class a path or command matches wins.
        let ordered = |m: &HashMap<String, Vec<String>>, order: &[&str]| {
            order.iter().filter_map(|k| Some((k.to_string(), m.get(*k)?.iter().map(|s| s.to_lowercase()).collect()))).collect()
        };
        Rules {
            words,
            phrases,
            branches,
            tools,
            commands: ordered(&raw.commands, &["test", "ops", "review", "read"]),
            paths: ordered(&raw.paths, &["planning", "test", "ops", "docs"]),
            w: raw.weights,
        }
    })
}

/// What a turn did, as counts. Stored per turn (JSON) so a turn that spans two ingest passes
/// keeps adding up; nothing in here can be turned back into the prompt.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Signals {
    pub prompts: u32,
    /// Prompt keyword hits per category (in `ALL` order), capped
    pub kw: [u8; 10],
    pub stack_trace: bool,
    pub branch: Option<Category>,
    pub plan_mode: bool,
    pub tools: u32,
    pub edit_src: u32,
    pub edit_test: u32,
    pub edit_docs: u32,
    pub edit_ops: u32,
    pub edit_plan: u32,
    /// Distinct edited files (hashes of the path), capped
    pub files: Vec<u32>,
    pub reads: u32,
    pub web: u32,
    pub todo: u32,
    pub cmd_test: u32,
    pub cmd_ops: u32,
    pub cmd_review: u32,
}

const KW_CAP: u8 = 3;
const FILES_CAP: usize = 64;

impl Signals {
    /// A user prompt. Read here and dropped; only the keyword counts stay.
    pub fn prompt(&mut self, text: &str) {
        let r = rules();
        self.prompts += 1;
        // Long pastes (logs, files) say little about intent past the first few thousand chars.
        let text: String = text.chars().take(4000).collect::<String>().to_lowercase();
        if looks_like_stack_trace(&text) {
            self.stack_trace = true;
        }
        let words: Vec<&str> = text.split(|c: char| !(c.is_alphanumeric() || c == '\'')).filter(|w| !w.is_empty()).collect();
        let mut hit = [0u8; 10];
        for w in &words {
            if let Some(cats) = r.words.get(*w) {
                for &c in cats {
                    hit[c] = hit[c].saturating_add(1);
                }
            }
        }
        for (phrase, c) in &r.phrases {
            if words.windows(phrase.len()).any(|win| win.iter().zip(phrase).all(|(a, b)| *a == b)) {
                hit[*c] = hit[*c].saturating_add(1);
            }
        }
        for (k, h) in self.kw.iter_mut().zip(hit) {
            *k = (*k).saturating_add(h).min(KW_CAP);
        }
    }

    /// The git branch, for its prefix ("feat/…", "fix-…").
    pub fn branch(&mut self, branch: &str) {
        if self.branch.is_some() {
            return;
        }
        let b = branch.to_lowercase();
        let prefix = b.split(['/', '-', '_']).next().unwrap_or("");
        if let Some(c) = rules().branches.get(prefix) {
            self.branch = Some(*c);
        }
    }

    pub fn plan_mode(&mut self) {
        self.plan_mode = true;
    }

    /// One tool call: its name, the file it touched (if any) and the shell command (if any).
    pub fn tool(&mut self, name: &str, path: Option<&str>, command: Option<&str>) {
        let r = rules();
        self.tools += 1;
        let name = name.to_lowercase();
        match r.tools.get(&name).copied() {
            Some(ToolKind::Edit) => {
                if let Some(p) = path {
                    self.edit(p);
                } else {
                    self.edit_src += 1;
                }
            }
            Some(ToolKind::Read) => self.reads += 1,
            Some(ToolKind::Web) => self.web += 1,
            Some(ToolKind::Todo) => self.todo += 1,
            Some(ToolKind::Plan) => self.plan_mode = true,
            Some(ToolKind::Shell) => {
                if let Some(c) = command {
                    self.command(c);
                }
            }
            None => {}
        }
    }

    /// An edited file, from an edit tool or a patch.
    pub fn edit(&mut self, path: &str) {
        let p = path.to_lowercase();
        let class = rules().paths.iter().find(|(_, pats)| pats.iter().any(|x| p.contains(x.as_str()))).map(|(k, _)| k.as_str());
        match class {
            Some("planning") => self.edit_plan += 1,
            Some("test") => self.edit_test += 1,
            Some("ops") => self.edit_ops += 1,
            Some("docs") => self.edit_docs += 1,
            _ => self.edit_src += 1,
        }
        let h = fnv(&p);
        if self.files.len() < FILES_CAP && !self.files.contains(&h) {
            self.files.push(h);
        }
    }

    fn command(&mut self, cmd: &str) {
        let c = cmd.to_lowercase();
        let class = rules().commands.iter().find(|(_, pats)| pats.iter().any(|x| contains_word(&c, x))).map(|(k, _)| k.as_str());
        match class {
            Some("test") => self.cmd_test += 1,
            Some("ops") => self.cmd_ops += 1,
            Some("review") => self.cmd_review += 1,
            Some("read") => self.reads += 1,
            _ => {}
        }
    }

    /// Adds another pass's counts for the same turn.
    pub fn merge(&mut self, o: &Signals) {
        self.prompts += o.prompts;
        for (a, b) in self.kw.iter_mut().zip(o.kw) {
            *a = a.saturating_add(b).min(KW_CAP);
        }
        self.stack_trace |= o.stack_trace;
        self.branch = self.branch.or(o.branch);
        self.plan_mode |= o.plan_mode;
        self.tools += o.tools;
        self.edit_src += o.edit_src;
        self.edit_test += o.edit_test;
        self.edit_docs += o.edit_docs;
        self.edit_ops += o.edit_ops;
        self.edit_plan += o.edit_plan;
        for h in &o.files {
            if self.files.len() < FILES_CAP && !self.files.contains(h) {
                self.files.push(*h);
            }
        }
        self.reads += o.reads;
        self.web += o.web;
        self.todo += o.todo;
        self.cmd_test += o.cmd_test;
        self.cmd_ops += o.cmd_ops;
        self.cmd_review += o.cmd_review;
    }

    fn edits(&self) -> u32 {
        self.edit_src + self.edit_test + self.edit_docs + self.edit_ops + self.edit_plan
    }
}

/// The winning category and how sure we are (its share of all the score, 0–1).
pub fn classify(s: &Signals) -> (Category, f64) {
    let w = &rules().w;
    let mut score = [0.0f64; 10];
    let at = |c: Category| index(c);
    for (i, k) in s.kw.iter().enumerate() {
        score[i] += *k as f64 * w.keyword;
    }
    let edits = s.edits();
    if edits > 0 {
        // "How do I…" prompts that end in edits are building, not exploring.
        score[at(Category::Discovery)] *= 0.5;
    }
    if s.stack_trace {
        score[at(Category::Bugfix)] += w.stack_trace;
    }
    if let Some(b) = s.branch {
        score[at(b)] += w.branch;
    }
    // Diminishing returns, so one busy signal can't drown the rest.
    let damp = |n: u32| (n as f64).sqrt();
    score[at(Category::Build)] += damp(s.edit_src) * w.edit;
    score[at(Category::Test)] += damp(s.edit_test) * w.edit + damp(s.cmd_test) * w.command;
    score[at(Category::Docs)] += damp(s.edit_docs) * w.edit;
    score[at(Category::Ops)] += damp(s.edit_ops) * w.edit + damp(s.cmd_ops) * w.command;
    score[at(Category::Planning)] += damp(s.edit_plan) * w.edit + damp(s.todo) * w.todo;
    score[at(Category::Review)] += damp(s.cmd_review) * w.command;
    if s.files.len() >= w.many_files_at {
        score[at(Category::Refactor)] += w.many_files;
    }
    if edits == 0 {
        score[at(Category::Discovery)] += damp(s.reads) * w.read + damp(s.web) * w.web;
    }
    if s.plan_mode {
        score[at(Category::Planning)] += w.plan_mode;
    }
    if s.tools == 0 {
        score[at(Category::Chat)] += w.no_tools_chat;
    }
    let total: f64 = score.iter().sum();
    let (best, top) = score.iter().enumerate().fold((0, 0.0), |acc, (i, v)| if *v > acc.1 { (i, *v) } else { acc });
    if total <= 0.0 {
        // Tools ran but nothing we recognise (browser automation, MCP calls…).
        return (if s.tools == 0 { Category::Chat } else { Category::Build }, 0.0);
    }
    (ALL[best], (top / total).clamp(0.0, 1.0))
}

fn looks_like_stack_trace(t: &str) -> bool {
    t.contains("traceback (most recent call last)")
        || t.contains("panicked at")
        || t.contains("error[e")
        || t.contains("exception in thread")
        || t.contains("uncaught ")
        || t.lines().filter(|l| l.trim_start().starts_with("at ") && l.contains('(')).count() >= 2
}

/// `pat` in `hay` starting at a word boundary (and ending at one, unless the pattern ends in a space).
fn contains_word(hay: &str, pat: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(i) = hay[from..].find(pat) {
        let s = from + i;
        let e = s + pat.len();
        let before = hay[..s].chars().next_back().is_none_or(|c| !is_word(c));
        let after = pat.ends_with(' ') || hay[e..].chars().next().is_none_or(|c| !is_word(c));
        if before && after {
            return true;
        }
        from = s + pat.chars().next().map(char::len_utf8).unwrap_or(1);
    }
    false
}

fn fnv(s: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in s.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x01000193);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(prompt: &str, tools: &[(&str, Option<&str>, Option<&str>)]) -> Category {
        let mut s = Signals::default();
        if !prompt.is_empty() {
            s.prompt(prompt);
        }
        for (n, p, c) in tools {
            s.tool(n, *p, *c);
        }
        classify(&s).0
    }

    #[test]
    fn rules_parse_and_cover_every_category() {
        assert_eq!(ALL.iter().map(|c| id(*c)).collect::<std::collections::HashSet<_>>().len(), 10);
        let r = rules();
        assert!(!r.words.is_empty() && !r.tools.is_empty());
        for c in ALL {
            assert_eq!(from_id(id(c)), Some(c));
            // the serde name and our id agree
            assert_eq!(serde_json::to_value(c).unwrap(), serde_json::Value::String(id(c).into()));
        }
    }

    #[test]
    fn classifies_synthetic_turns() {
        assert_eq!(turn("what's the capital of France", &[]), Category::Chat);
        assert_eq!(
            turn(
                "add a settings page with a dark mode toggle",
                &[("Write", Some("src/settings.js"), None), ("Edit", Some("src/app.js"), None)]
            ),
            Category::Build
        );
        assert_eq!(
            turn("the app crashes on launch, fix it", &[("Read", Some("src/main.rs"), None), ("Edit", Some("src/main.rs"), None)]),
            Category::Bugfix
        );
        assert_eq!(
            turn("where is the token refreshed? explain the flow", &[("Grep", None, None), ("Read", None, None), ("Read", None, None)]),
            Category::Discovery
        );
        assert_eq!(
            turn(
                "",
                &[
                    ("Bash", None, Some("cargo test -p core")),
                    ("Edit", Some("tests/store_test.rs"), None),
                    ("Bash", None, Some("cargo test"))
                ]
            ),
            Category::Test
        );
        assert_eq!(turn("review this PR", &[("Bash", None, Some("gh pr diff 12"))]), Category::Review);
        assert_eq!(turn("", &[("Edit", Some("README.md"), None), ("Write", Some("docs/setup.md"), None)]), Category::Docs);
        assert_eq!(turn("", &[("Bash", None, Some("docker compose up -d")), ("Bash", None, Some("kubectl get pods"))]), Category::Ops);
        assert_eq!(turn("", &[("TodoWrite", None, None), ("ExitPlanMode", None, None)]), Category::Planning);
        let files = ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs", "f.rs"];
        let edits: Vec<_> = files.iter().map(|f| ("Edit", Some(*f), None)).collect();
        assert_eq!(turn("refactor: rename Foo to Bar everywhere", &edits), Category::Refactor);
    }

    #[test]
    fn stack_traces_and_branches_count() {
        let mut s = Signals::default();
        s.prompt("Traceback (most recent call last):\n  File \"x.py\", line 3");
        s.tool("Edit", Some("x.py"), None);
        assert_eq!(classify(&s).0, Category::Bugfix);

        let mut s = Signals::default();
        s.branch("feat/new-thing");
        s.tool("Read", None, None);
        s.tool("Edit", Some("src/lib.rs"), None);
        assert_eq!(s.branch, Some(Category::Build));
        assert_eq!(classify(&s).0, Category::Build);
    }

    #[test]
    fn signals_merge_and_confidence() {
        let mut a = Signals::default();
        a.prompt("fix the bug");
        let mut b = Signals::default();
        b.tool("Edit", Some("src/a.rs"), None);
        a.merge(&b);
        assert_eq!(a.tools, 1);
        assert_eq!(a.edit_src, 1);
        let (c, conf) = classify(&a);
        assert_eq!(c, Category::Bugfix);
        assert!(conf > 0.5 && conf <= 1.0, "{conf}");
        // round-trips as JSON for the store, and keeps no text
        let j = serde_json::to_string(&a).unwrap();
        assert!(!j.contains("fix the bug"));
        assert_eq!(serde_json::from_str::<Signals>(&j).unwrap(), a);
    }

    #[test]
    fn commands_match_whole_words() {
        assert!(contains_word("cd x && cargo test --all", "cargo test"));
        assert!(!contains_word("cargo testing", "cargo test"));
        assert!(contains_word("ls -la", "ls"));
        assert!(!contains_word("false", "ls"));
        assert!(contains_word("aws s3 ls", "aws "));
    }
}
