//! The `tokenrunway` command line (SPEC.md §6.6): `status`, `statusline` and `today`.
//!
//! The running app writes `status.json` to its data folder on every state broadcast: the accounts,
//! their windows and the summary, without settings or anything secret. The command line reads
//! that for allowances (they need the app's logins and polling), and reads the local stats
//! straight from `activity.db`. Everything here is pure formatting so it can be tested; the
//! binary's side (arguments, files, the Windows console) is in src-tauri/src/cli.rs.

use crate::forecast;
use crate::view::{AccountState, AppView, HarnessView, StatsView, Status};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The app's bundle identifier; Tauri's app data folder is `<data dir>/<identifier>`.
pub const IDENTIFIER: &str = "io.github.slastrina.tokenrunway";
/// The file the app writes and the command line reads.
pub const STATUS_FILE: &str = "status.json";
/// Older than this and the app is probably not running.
pub const STALE_MS: i64 = 10 * 60 * 1000;

/// The app's data folder, as Tauri's `app_data_dir()` resolves it: `~/Library/Application
/// Support/<id>` on macOS, `%APPDATA%\<id>` on Windows, `$XDG_DATA_HOME/<id>` (or
/// `~/.local/share/<id>`) on Linux. `TOKENRUNWAY_DATA_DIR` overrides it.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("TOKENRUNWAY_DATA_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    dirs::data_dir().map(|d| d.join(IDENTIFIER))
}

/// What the app shares with the command line.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusFile {
    pub version: String,
    /// When the app wrote it (ms)
    pub written_at: i64,
    pub summary: Option<StatusSummary>,
    pub accounts: Vec<StatusAccount>,
    /// API-equivalent spend since local midnight; None until the logs are read
    pub spend_today: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusSummary {
    /// "1h 26m"
    pub label: String,
    pub status: Status,
    /// "Claude — work"
    pub account: String,
    /// "5-hour"
    pub window: String,
    /// The tightest window's % used
    pub pct: f64,
    pub crit: u32,
    pub warn: u32,
    pub ok: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusAccount {
    pub name: String,
    pub provider: String,
    pub plan: String,
    pub state: AccountState,
    pub problem: Option<String>,
    pub updated_at: Option<i64>,
    pub windows: Vec<StatusWindow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusWindow {
    pub id: String,
    pub label: String,
    pub abs: Option<String>,
    pub pct: f64,
    pub resets_at: Option<i64>,
    pub out_at: Option<i64>,
    pub status: Status,
    pub forecast: String,
}

impl StatusFile {
    pub fn from_view(v: &AppView, now: i64) -> Self {
        let tight = forecast::tightest(&v.accounts);
        Self {
            version: v.version.clone(),
            written_at: now,
            summary: v.summary.as_ref().map(|s| StatusSummary {
                label: s.label.clone(),
                status: s.status,
                account: tight.map(|(a, _)| a.name.clone()).unwrap_or_default(),
                window: tight.map(|(_, w)| w.label.clone()).unwrap_or_default(),
                pct: tight.map(|(_, w)| w.pct).unwrap_or_default(),
                crit: s.crit,
                warn: s.warn,
                ok: s.ok,
            }),
            accounts: v
                .accounts
                .iter()
                .map(|a| StatusAccount {
                    name: a.name.clone(),
                    provider: a.provider.clone(),
                    plan: a.plan.clone(),
                    state: a.state,
                    problem: a.problem.clone(),
                    updated_at: a.updated_at,
                    windows: a
                        .windows
                        .iter()
                        .map(|w| StatusWindow {
                            id: w.id.clone(),
                            label: w.label.clone(),
                            abs: w.abs.clone(),
                            pct: w.pct,
                            resets_at: w.resets_at,
                            out_at: w.out_at,
                            status: w.status,
                            forecast: w.forecast.clone(),
                        })
                        .collect(),
                })
                .collect(),
            spend_today: v.spend_today,
        }
    }

    pub fn is_stale(&self, now: i64) -> bool {
        now - self.written_at > STALE_MS
    }
}

/// ANSI colours, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paint(pub bool);

impl Paint {
    fn wrap(self, code: &str, s: &str) -> String {
        if self.0 {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn bold(self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn status(self, st: Status, s: &str) -> String {
        match st {
            Status::Crit => self.wrap("31", s),
            Status::Warn => self.wrap("33", s),
            Status::Ok => self.wrap("36", s),
            Status::Est => s.to_string(),
        }
    }
}

/// "$12.40", "$318", "$1,240"
pub fn money(v: f64) -> String {
    if v.abs() < 100.0 {
        format!("${v:.2}")
    } else {
        crate::allowance::usd(v)
    }
}

/// "6.3M", "481k", "900"
pub fn tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.1}B", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.0}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// "3 min ago", "2 h ago", "4 days ago"
fn ago(ms: i64) -> String {
    let m = (ms / 60_000).max(0);
    if m < 60 {
        format!("{m} min ago")
    } else if m < 48 * 60 {
        format!("{} h ago", m / 60)
    } else {
        format!("{} days ago", m / 1440)
    }
}

/// "Claude — work" → "Claude"
fn short_account(name: &str) -> &str {
    name.split(" — ").next().unwrap_or(name).trim()
}

/// Why allowances are missing or old, if they are.
pub fn freshness(status: Option<&StatusFile>, now: i64) -> Option<String> {
    match status {
        None => Some("Tokenrunway isn't running, so there are no allowances to show. Start the app to see them.".into()),
        Some(s) if s.is_stale(now) => {
            Some(format!("Tokenrunway last updated {}; allowances may be out of date. Is the app running?", ago(now - s.written_at)))
        }
        _ => None,
    }
}

fn pad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(w - n))
    }
}

fn rpad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w {
        s.to_string()
    } else {
        format!("{}{s}", " ".repeat(w - n))
    }
}

/// `tokenrunway status`. `spend_today` comes from the status file or, without one, from the
/// local stats.
pub fn status_text(status: Option<&StatusFile>, spend_today: Option<f64>, now: i64, p: Paint) -> String {
    let mut out = String::new();
    if let Some(note) = freshness(status, now) {
        out += &p.dim(&note);
        out += "\n\n";
    }
    if let Some(s) = status {
        if let Some(sum) = &s.summary {
            let mut counts = Vec::new();
            if sum.crit > 0 {
                counts.push(p.status(Status::Crit, &format!("{} critical", sum.crit)));
            }
            if sum.warn > 0 {
                counts.push(p.status(Status::Warn, &format!("{} tight", sum.warn)));
            }
            if sum.ok > 0 {
                counts.push(format!("{} on track", sum.ok));
            }
            out += &format!(
                "{} {}  {}\n{}\n\n",
                p.bold("Shortest runway"),
                p.status(sum.status, &p.bold(&sum.label)),
                p.dim(&format!("{} · {}", sum.account, sum.window)),
                counts.join(" · ")
            );
        } else if !s.accounts.is_empty() {
            out += "No windows to show yet.\n\n";
        }
        let lw = s.accounts.iter().flat_map(|a| a.windows.iter()).map(|w| w.label.chars().count()).max().unwrap_or(0).max(6);
        for a in &s.accounts {
            let plan = if a.plan.is_empty() { String::new() } else { format!("  {}", p.dim(&a.plan)) };
            out += &format!("{}{plan}\n", p.bold(&a.name));
            if a.state != AccountState::Ok {
                if let Some(pr) = &a.problem {
                    out += &format!("  {}\n", p.status(Status::Warn, pr));
                }
            }
            if a.windows.is_empty() && a.state == AccountState::Ok {
                out += &format!("  {}\n", p.dim("No usage limits reported"));
            }
            for w in &a.windows {
                let resets = match w.resets_at {
                    Some(r) if r > now => format!("resets in {} ({})", forecast::duration(r - now), forecast::when(r, now)),
                    Some(_) => "resetting now".into(),
                    None => String::new(),
                };
                let abs = w.abs.as_deref().map(|x| format!("  {}", p.dim(x))).unwrap_or_default();
                out += &format!(
                    "  {} {}{abs}\n    {}{}\n",
                    pad(&w.label, lw),
                    p.status(w.status, &rpad(&format!("{:.0}%", w.pct), 4)),
                    p.dim(&resets),
                    if w.forecast.is_empty() {
                        String::new()
                    } else {
                        format!("{}{}", if resets.is_empty() { "" } else { " · " }, p.status(w.status, &w.forecast))
                    }
                );
            }
        }
        if !s.accounts.is_empty() {
            out.push('\n');
        }
    }
    if let Some(d) = spend_today {
        out += &format!("{} {}  {}\n", p.bold("Today"), money(d), p.dim("API-equivalent, from your agents' logs"));
    }
    out += &p.dim("Tokenrunway · iokig.com");
    out.push('\n');
    out
}

/// `tokenrunway statusline`: one line for a shell prompt or Claude Code's status line, like
/// `▲ 1h 26m · Claude 5-hour 78% · $12.40 today`.
pub fn statusline(status: Option<&StatusFile>, spend_today: Option<f64>, now: i64, p: Paint) -> String {
    let mut parts = Vec::new();
    let stale = status.is_some_and(|s| s.is_stale(now));
    if let Some(sum) = status.and_then(|s| s.summary.as_ref()) {
        parts.push(p.status(sum.status, &format!("▲ {}", sum.label)));
        let who = short_account(&sum.account);
        parts.push(
            format!("{}{} {:.0}%", who, if who.is_empty() { String::new() } else { format!(" {}", sum.window) }, sum.pct)
                .trim()
                .to_string(),
        );
    }
    if let Some(d) = spend_today {
        parts.push(format!("{} today", money(d)));
    }
    if parts.is_empty() {
        return "Tokenrunway: no data".into();
    }
    let mut line = parts.join(" · ");
    if stale {
        line += &p.dim(" (stale)");
    }
    line
}

/// `tokenrunway today`: today's spend by harness, model and project.
pub fn today_text(stats: &StatsView, harness: &HarnessView, p: Paint) -> String {
    let mut out = format!(
        "{} {}  {}\n",
        p.bold("Today"),
        money(stats.spend_api),
        p.dim(&format!("API-equivalent · {} tokens", tokens(stats.tokens.total)))
    );
    if let Some(a) = stats.spend_actual.filter(|a| *a > 0.0) {
        out += &format!("Actually billed {}{}\n", money(a), stats.actual_note.as_deref().map(|n| format!(" ({n})")).unwrap_or_default());
    }
    if stats.spend_api <= 0.0 && stats.tokens.total == 0 {
        out += &p.dim("Nothing yet today. Stats come from your agents' local logs (Claude Code, Codex, opencode…).\n");
        return out;
    }
    let section = |title: &str, rows: Vec<(String, f64, String)>| -> String {
        if rows.is_empty() {
            return String::new();
        }
        let w = rows.iter().map(|r| r.0.chars().count()).max().unwrap_or(0);
        let mut s = format!("\n{}\n", p.bold(title));
        for (name, cost, extra) in rows {
            s += &format!("  {}  {}  {}\n", pad(&name, w), rpad(&money(cost), 9), p.dim(&extra));
        }
        s
    };
    out += &section(
        "By harness",
        harness
            .harnesses
            .iter()
            .filter(|h| h.cost > 0.0 || h.tokens > 0)
            .map(|h| (h.name.clone(), h.cost, format!("{:.0}% · {} sessions", h.share, h.sessions)))
            .collect(),
    );
    out += &section("By model", stats.models.iter().map(|m| (m.name.clone(), m.cost, format!("{} tokens", tokens(m.tokens)))).collect());
    out +=
        &section("By project", stats.projects.iter().map(|m| (m.name.clone(), m.cost, format!("{} tokens", tokens(m.tokens)))).collect());
    out
}

/// `tokenrunway guard`'s options.
#[derive(Debug, Clone, PartialEq)]
pub struct GuardOpts {
    /// Stop at this % used or more
    pub max: f64,
    /// Only accounts whose name contains this (any case), or whose provider is it
    pub account: Option<String>,
    /// Only windows whose id is this, or whose label contains it ("5h", "weekly")
    pub window: Option<String>,
    /// Also stop when a window is forecast to run out before it resets
    pub runout: bool,
    /// Go ahead when it can't tell (the app isn't running, or nothing matches)
    pub allow_unknown: bool,
}

impl Default for GuardOpts {
    fn default() -> Self {
        Self { max: 90.0, account: None, window: None, runout: false, allow_unknown: false }
    }
}

impl GuardOpts {
    /// Parses `--max 80`, `--max=80`, `--account work`, `--window 5h`, `--runout` and
    /// `--allow-unknown`; `--json` and `-v` are the caller's. Err says what's wrong.
    pub fn parse<'a>(args: impl IntoIterator<Item = &'a str>) -> Result<Self, String> {
        let mut o = Self::default();
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            let (flag, inline) = match a.split_once('=') {
                Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
                _ => (a, None),
            };
            let mut value = |name: &str| inline.clone().or_else(|| it.next().map(str::to_string)).ok_or(format!("{name} needs a value"));
            match flag {
                "--max" => {
                    let v = value("--max")?;
                    o.max = v
                        .trim_end_matches('%')
                        .parse::<f64>()
                        .ok()
                        .filter(|m| (0.0..=1000.0).contains(m))
                        .ok_or(format!("--max {v} isn't a percentage"))?;
                }
                "--account" => o.account = Some(value("--account")?),
                "--window" => o.window = Some(value("--window")?),
                "--runout" => o.runout = true,
                "--allow-unknown" => o.allow_unknown = true,
                "--json" | "-v" | "--verbose" => {}
                _ => return Err(format!("unknown option {a}")),
            }
        }
        Ok(o)
    }
}

/// What `guard` decided. Exit codes: 0 go, 1 stop, 3 can't tell (2 is a usage error).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Go,
    Stop,
    Unknown,
}

impl Verdict {
    pub fn code(self) -> i32 {
        match self {
            Self::Go => 0,
            Self::Stop => 1,
            Self::Unknown => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GuardWindow {
    pub account: String,
    pub window: String,
    pub pct: f64,
    pub resets_at: Option<i64>,
    pub out_at: Option<i64>,
    /// Why this window stops the run, if it does
    pub stop: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Guard {
    pub verdict: Verdict,
    /// The exit code (`--allow-unknown` turns "can't tell" into 0)
    pub code: i32,
    /// One plain line: why it stopped, why it can't tell, or the tightest window it checked
    pub message: String,
    pub max: f64,
    /// The windows it checked
    pub windows: Vec<GuardWindow>,
}

/// `tokenrunway guard`: is it safe to start work? Reads only the status file, so it's quick.
/// "Can't tell" stops by default: a loop that can't see its allowance shouldn't assume room.
pub fn guard(status: Option<&StatusFile>, o: &GuardOpts, now: i64) -> Guard {
    let done = |verdict: Verdict, message: String, windows: Vec<GuardWindow>| {
        let code = if verdict == Verdict::Unknown && o.allow_unknown { 0 } else { verdict.code() };
        Guard { verdict, code, message, max: o.max, windows }
    };
    let s = match status {
        Some(s) if !s.is_stale(now) => s,
        _ => return done(Verdict::Unknown, freshness(status, now).unwrap_or_default(), Vec::new()),
    };
    let has = |hay: &str, needle: &str| hay.to_lowercase().contains(&needle.to_lowercase());
    let windows: Vec<GuardWindow> = s
        .accounts
        .iter()
        .filter(|a| o.account.as_deref().is_none_or(|q| has(&a.name, q) || a.provider.eq_ignore_ascii_case(q)))
        .flat_map(|a| {
            a.windows.iter().filter(|w| o.window.as_deref().is_none_or(|q| w.id.eq_ignore_ascii_case(q) || has(&w.label, q))).map(|w| {
                let resets = w.resets_at.map(|r| format!(", resets in {}", forecast::duration(r - now))).unwrap_or_default();
                // The same forecast the app shows; not one it marks as an estimate (too little history).
                let runs_out = w.out_at.filter(|out| o.runout && w.status != Status::Est && w.resets_at.is_none_or(|r| *out < r));
                let stop = if w.pct >= o.max {
                    Some(format!("{} {} is at {:.0}% (limit {:.0}%){resets}.", a.name, w.label, w.pct, o.max))
                } else {
                    runs_out.map(|out| format!("{} {} runs out in {}{resets}.", a.name, w.label, forecast::duration(out - now)))
                };
                GuardWindow { account: a.name.clone(), window: w.label.clone(), pct: w.pct, resets_at: w.resets_at, out_at: w.out_at, stop }
            })
        })
        .collect();
    if windows.is_empty() {
        let what = match (&o.account, &o.window) {
            (None, None) => "No account has a usage window to check.".to_string(),
            (a, w) => format!(
                "No window matches{}{}.",
                a.as_ref().map(|a| format!(" --account {a}")).unwrap_or_default(),
                w.as_ref().map(|w| format!(" --window {w}")).unwrap_or_default()
            ),
        };
        return done(Verdict::Unknown, what, windows);
    }
    let stops: Vec<&String> = windows.iter().filter_map(|w| w.stop.as_ref()).collect();
    if let Some(first) = stops.first() {
        let msg = match stops.len() {
            1 => first.to_string(),
            n => format!("{first} ({} more over the limit)", n - 1),
        };
        return done(Verdict::Stop, msg, windows);
    }
    let tight = windows.iter().max_by(|a, b| a.pct.total_cmp(&b.pct)).expect("not empty");
    let msg = format!("OK: the tightest is {} {} at {:.0}% (limit {:.0}%).", tight.account, tight.window, tight.pct, o.max);
    done(Verdict::Go, msg, windows)
}

pub const HELP: &str = "\
Tokenrunway: how much AI runway you have left.

Usage: tokenrunway [command]

Commands:
  status [--json]   Every account's windows, resets and forecasts, and today's spend
  statusline        One short line for a shell prompt or Claude Code's status line
  today [--json]    Today's spend by harness, model and project
  guard [options]   Exit 0 if it's safe to start work, 1 if a window is over the limit
  --help            This help
  --version         The version

guard options:
  --max <percent>     Stop at this % used or more (default 90)
  --account <name>    Only accounts whose name contains this, or a provider (anthropic, openai…)
  --window <id>       Only this window: 5h, weekly, or part of its label
  --runout            Also stop when a window will run out before it resets
  --allow-unknown     Go ahead when it can't tell (exit 0 instead of 3)
  -v, --json          Say what it checked; as JSON

  guard prints nothing when it's safe, so `tokenrunway guard && claude -p ...` reads well.
  Exit codes: 0 go, 1 over the limit, 2 bad options, 3 can't tell (the app isn't running or
  nothing matches).

With no command, it opens the app. Allowances come from the running app; spend comes from
your agents' local logs. Set NO_COLOR to turn colours off.
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{AccountView, HarnessRow, Ranked, Summary, TokenTotals, WindowView};

    const NOW: i64 = 1_792_807_612_967;
    const H: i64 = 3_600_000;

    fn sample() -> StatusFile {
        let view = AppView {
            version: "0.1.2".into(),
            summary: Some(Summary {
                label: "1h 26m".into(),
                status: Status::Crit,
                window: "Claude — work · 5-hour".into(),
                crit: 1,
                warn: 1,
                ok: 1,
            }),
            accounts: vec![
                AccountView {
                    name: "Claude — work".into(),
                    plan: "Enterprise".into(),
                    windows: vec![
                        WindowView {
                            id: "5h".into(),
                            label: "5-hour".into(),
                            abs: Some("412k / 530k tokens".into()),
                            pct: 78.0,
                            resets_at: Some(NOW + 2 * H + 41 * 60_000),
                            out_at: Some(NOW + H + 26 * 60_000),
                            status: Status::Crit,
                            forecast: "Out at 13:05, 1h 15m before reset".into(),
                            ..Default::default()
                        },
                        WindowView {
                            id: "weekly".into(),
                            label: "Weekly".into(),
                            pct: 64.0,
                            status: Status::Ok,
                            forecast: "On track".into(),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
                AccountView { name: "ChatGPT — personal".into(), plan: "Business".into(), ..Default::default() },
            ],
            spend_today: Some(12.4),
            ..Default::default()
        };
        StatusFile::from_view(&view, NOW)
    }

    #[test]
    fn status_file_round_trips_without_settings() {
        let s = sample();
        assert_eq!(s.summary.as_ref().map(|x| (x.account.as_str(), x.window.as_str(), x.pct)), Some(("Claude — work", "5-hour", 78.0)));
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("settings") && !json.contains("rattleboard"));
        assert_eq!(serde_json::from_str::<StatusFile>(&json).unwrap(), s);
        // Unknown and missing fields are fine (an older or newer app).
        let old: StatusFile = serde_json::from_str(r#"{"version":"0.1.0","writtenAt":5,"extra":true}"#).unwrap();
        assert_eq!((old.written_at, old.accounts.len()), (5, 0));
    }

    #[test]
    fn statusline_shapes() {
        let s = sample();
        let off = Paint(false);
        assert_eq!(statusline(Some(&s), s.spend_today, NOW, off), "▲ 1h 26m · Claude 5-hour 78% · $12.40 today");
        assert_eq!(statusline(Some(&s), s.spend_today, NOW + STALE_MS + 1, off), "▲ 1h 26m · Claude 5-hour 78% · $12.40 today (stale)");
        assert_eq!(statusline(None, Some(318.4), NOW, off), "$318 today");
        assert_eq!(statusline(None, None, NOW, off), "Tokenrunway: no data");
        let red = statusline(Some(&s), None, NOW, Paint(true));
        assert!(red.starts_with("\x1b[31m▲ 1h 26m\x1b[0m"), "{red:?}");
    }

    #[test]
    fn status_text_lists_everything() {
        let s = sample();
        let t = status_text(Some(&s), s.spend_today, NOW, Paint(false));
        assert!(t.starts_with("Shortest runway 1h 26m  Claude — work · 5-hour\n1 critical · 1 tight · 1 on track\n"), "{t}");
        assert!(t.contains("  5-hour  78%  412k / 530k tokens\n    resets in 2h 41m ("), "{t}");
        assert!(t.contains(" · Out at 13:05, 1h 15m before reset\n"));
        assert!(t.contains("  Weekly  64%\n    On track\n"), "{t}");
        assert!(t.contains("ChatGPT — personal  Business\n  No usage limits reported\n"));
        assert!(t.contains("Today $12.40  API-equivalent"));
        assert!(t.trim_end().ends_with("Tokenrunway · iokig.com"));
        assert!(!t.contains('\x1b'));
        let none = status_text(None, Some(3.0), NOW, Paint(false));
        assert!(none.starts_with("Tokenrunway isn't running"), "{none}");
        assert!(status_text(Some(&s), None, NOW + 3 * H, Paint(false)).starts_with("Tokenrunway last updated 3 h ago"));
    }

    #[test]
    fn guard_decides() {
        let s = sample();
        let opts = |args: &[&str]| GuardOpts::parse(args.iter().copied()).unwrap();
        let g = guard(Some(&s), &opts(&[]), NOW);
        assert_eq!((g.verdict, g.code, g.windows.len()), (Verdict::Go, 0, 2), "78% is under the default 90%");
        assert_eq!(g.message, "OK: the tightest is Claude — work 5-hour at 78% (limit 90%).");

        let g = guard(Some(&s), &opts(&["--max", "75"]), NOW);
        assert_eq!((g.verdict, g.code), (Verdict::Stop, 1));
        assert_eq!(g.message, "Claude — work 5-hour is at 78% (limit 75%), resets in 2h 41m.");
        let g = guard(Some(&s), &opts(&["--max=60%"]), NOW);
        assert!(g.message.ends_with("(1 more over the limit)"), "{}", g.message);

        // Forecast: out in 1h 26m, before the reset.
        let g = guard(Some(&s), &opts(&["--runout"]), NOW);
        assert_eq!(g.message, "Claude — work 5-hour runs out in 1h 26m, resets in 2h 41m.");
        let mut est = s.clone();
        est.accounts[0].windows[0].status = Status::Est;
        assert_eq!(guard(Some(&est), &opts(&["--runout"]), NOW).verdict, Verdict::Go, "not on an estimate");

        // Filters: a window, an account, a provider, case and part of the label.
        assert_eq!(guard(Some(&s), &opts(&["--window", "WEEKLY", "--max", "70"]), NOW).verdict, Verdict::Go);
        assert_eq!(guard(Some(&s), &opts(&["--window", "5h", "--max", "70"]), NOW).verdict, Verdict::Stop);
        assert_eq!(guard(Some(&s), &opts(&["--account", "work", "--window", "5-hour"]), NOW).windows.len(), 1);
        let none = guard(Some(&s), &opts(&["--account", "chatgpt"]), NOW);
        assert_eq!((none.verdict, none.code, none.message.as_str()), (Verdict::Unknown, 3, "No window matches --account chatgpt."));
        assert_eq!(guard(Some(&s), &opts(&["--account", "chatgpt", "--allow-unknown"]), NOW).code, 0);

        // The app isn't running, or stopped writing: can't tell, which stops unless allowed.
        let off = guard(None, &opts(&[]), NOW);
        assert_eq!((off.verdict, off.code), (Verdict::Unknown, 3));
        assert!(off.message.starts_with("Tokenrunway isn't running"));
        let stale = guard(Some(&s), &opts(&["--allow-unknown"]), NOW + STALE_MS + 1);
        assert_eq!((stale.verdict, stale.code), (Verdict::Unknown, 0));
        assert!(stale.message.contains("Is the app running?"));
    }

    #[test]
    fn guard_options() {
        let o =
            GuardOpts::parse(["--max", "80", "--account=work", "--window", "5h", "--runout", "--allow-unknown", "-v", "--json"]).unwrap();
        assert_eq!(o, GuardOpts { max: 80.0, account: Some("work".into()), window: Some("5h".into()), runout: true, allow_unknown: true });
        assert_eq!(GuardOpts::parse([]).unwrap(), GuardOpts::default());
        assert_eq!(GuardOpts::parse(["--max"]).unwrap_err(), "--max needs a value");
        assert_eq!(GuardOpts::parse(["--max", "lots"]).unwrap_err(), "--max lots isn't a percentage");
        assert_eq!(GuardOpts::parse(["--nope"]).unwrap_err(), "unknown option --nope");
        assert_eq!(Verdict::Stop.code(), 1);
    }

    #[test]
    fn today_breakdown() {
        let stats = StatsView {
            spend_api: 53.66,
            tokens: TokenTotals { total: 6_300_000, ..Default::default() },
            models: vec![Ranked { name: "Opus 5.5".into(), cost: 32.25, tokens: 3_000_000 }],
            projects: vec![Ranked { name: "third-eye".into(), cost: 19.66, tokens: 2_000_000 }],
            ..Default::default()
        };
        let harness = HarnessView {
            harnesses: vec![
                HarnessRow { name: "Claude Code".into(), cost: 45.0, share: 84.0, sessions: 5, ..Default::default() },
                HarnessRow { name: "Codex".into(), cost: 0.0, ..Default::default() },
            ],
            ..Default::default()
        };
        let t = today_text(&stats, &harness, Paint(false));
        assert!(t.starts_with("Today $53.66  API-equivalent · 6.3M tokens\n"), "{t}");
        assert!(t.contains("By harness\n  Claude Code     $45.00  84% · 5 sessions\n"), "{t}");
        assert!(!t.contains("Codex"), "harnesses with nothing today are left out");
        assert!(t.contains("By model\n  Opus 5.5     $32.25  3.0M tokens\n"), "{t}");
        let empty = today_text(&StatsView::default(), &HarnessView::default(), Paint(false));
        assert!(empty.contains("Nothing yet today"));
    }

    #[test]
    fn numbers() {
        assert_eq!(
            (money(0.0), money(12.4), money(318.4), money(1240.0)),
            ("$0.00".into(), "$12.40".into(), "$318".into(), "$1,240".into())
        );
        assert_eq!((tokens(900), tokens(481_000), tokens(6_300_000)), ("900".into(), "481k".into(), "6.3M".into()));
        assert_eq!(short_account("Claude — work"), "Claude");
    }
}
