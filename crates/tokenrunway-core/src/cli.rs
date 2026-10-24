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

pub const HELP: &str = "\
Tokenrunway: how much AI runway you have left.

Usage: tokenrunway [command]

Commands:
  status [--json]   Every account's windows, resets and forecasts, and today's spend
  statusline        One short line for a shell prompt or Claude Code's status line
  today [--json]    Today's spend by harness, model and project
  --help            This help
  --version         The version

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
