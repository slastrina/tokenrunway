//! The `tokenrunway` command line: the app's own executable with a subcommand (`status`,
//! `statusline`, `today`, `guard`, `--help`, `--version`) prints and exits before any window, tray icon
//! or single-instance check starts. The formatting is in tokenrunway_core::cli; this reads the
//! app's `status.json` and `activity.db`, and on Windows borrows the parent's console (the app
//! is a GUI-subsystem program, which has none of its own).

use std::io::{IsTerminal, Write};
use std::path::Path;
use tokenrunway_core::cli::{self, Paint, StatusFile};
use tokenrunway_core::stats::Stats;
use tokenrunway_core::view::AppView;

/// Runs the command line if the arguments ask for it; the exit code, or None to start the app.
pub fn run_from_args() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first()?.as_str();
    if !matches!(cmd, "status" | "statusline" | "today" | "guard" | "help" | "--help" | "-h" | "--version" | "-V" | "version") {
        // Anything else (macOS's -psn_…, autostart's flags) is for the app.
        return None;
    }
    attach_console();
    let rest: Vec<&str> = args[1..].iter().map(String::as_str).collect();
    let json = rest.contains(&"--json");
    if cmd == "guard" {
        return Some(guard(&rest, json));
    }
    if let Some(bad) = rest.iter().find(|a| **a != "--json") {
        eprintln!("tokenrunway {cmd}: unknown option {bad}\n\n{}", cli::HELP);
        return Some(2);
    }
    let out = match cmd {
        "--version" | "-V" | "version" => format!("tokenrunway {}\n", env!("CARGO_PKG_VERSION")),
        "help" | "--help" | "-h" => cli::HELP.to_string(),
        _ => match command(cmd, json) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tokenrunway: {e}");
                return Some(1);
            }
        },
    };
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(out.as_bytes());
    let _ = stdout.flush();
    Some(0)
}

fn command(cmd: &str, json: bool) -> Result<String, String> {
    let now = tokenrunway_core::now_ms();
    let dir = cli::data_dir().ok_or("couldn't find the app's data folder")?;
    let status = read_status(&dir);
    let paint = Paint(!json && std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()));
    // Claude Code's status line and most prompts render colour without a terminal on stdout.
    let line_paint = Paint(std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()));
    let fresh = status.as_ref().is_some_and(|s| !s.is_stale(now));
    // The running app keeps activity.db current; without it, read what's new first.
    let stats = || open_stats(&dir, !fresh);
    match cmd {
        "status" => {
            let spend = match status.as_ref().filter(|_| fresh).and_then(|s| s.spend_today) {
                Some(d) => Some(d),
                None => stats().map(|s| s.spend_today(now)),
            };
            if json {
                let v = serde_json::json!({ "stale": !fresh, "status": status, "spendToday": spend });
                return Ok(serde_json::to_string_pretty(&v).unwrap_or_default() + "\n");
            }
            Ok(cli::status_text(status.as_ref(), spend, now, paint))
        }
        "statusline" => {
            // Claude Code sends the session as JSON on stdin; it isn't needed, and isn't read, so
            // a prompt that leaves stdin open never waits on it.
            let spend = match status.as_ref().filter(|_| fresh).and_then(|s| s.spend_today) {
                Some(d) => Some(d),
                None => stats().map(|s| s.spend_today(now)),
            };
            Ok(cli::statusline(status.as_ref(), spend, now, line_paint) + "\n")
        }
        "today" => {
            let s = stats().ok_or("couldn't read the activity database")?;
            let (st, h) = (s.stats("today", now), s.harness("today", now));
            if json {
                let v = serde_json::json!({ "stats": st, "harness": h });
                return Ok(serde_json::to_string_pretty(&v).unwrap_or_default() + "\n");
            }
            Ok(cli::today_text(&st, &h, paint))
        }
        _ => Err(format!("unknown command {cmd}")),
    }
}

/// `tokenrunway guard`: silent when it's safe (exit 0), one line on stderr when it isn't (1) or
/// can't tell (3, or 0 with --allow-unknown); `-v` says what it checked, `--json` all of it.
fn guard(args: &[&str], json: bool) -> i32 {
    let opts = match cli::GuardOpts::parse(args.iter().copied()) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("tokenrunway guard: {e}\n\n{}", cli::HELP);
            return 2;
        }
    };
    let status = cli::data_dir().and_then(|d| read_status(&d));
    let g = cli::guard(status.as_ref(), &opts, tokenrunway_core::now_ms());
    if json {
        println!("{}", serde_json::to_string_pretty(&g).unwrap_or_default());
    } else if g.verdict != cli::Verdict::Go || args.iter().any(|a| matches!(*a, "-v" | "--verbose")) {
        eprintln!("{}", g.message);
    }
    g.code
}

fn read_status(dir: &Path) -> Option<StatusFile> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(cli::STATUS_FILE)).ok()?).ok()
}

/// The app's activity.db (shared safely: WAL with a busy timeout), or a throwaway one when the
/// app has never run here. `ingest` when the app isn't keeping it current.
fn open_stats(dir: &Path, ingest: bool) -> Option<Stats> {
    let db = dir.join("activity.db");
    let s = if db.exists() { Stats::open(&db).ok()? } else { Stats::in_memory().ok()? };
    if ingest || !db.exists() {
        s.ingest();
    }
    Some(s)
}

/// Written by the app on every state broadcast (atomically, so a reader never sees half).
pub fn write_status(dir: &Path, view: &AppView) {
    let s = StatusFile::from_view(view, tokenrunway_core::now_ms());
    let Ok(json) = serde_json::to_string_pretty(&s) else { return };
    let tmp = dir.join(format!("{}.tmp", cli::STATUS_FILE));
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(cli::STATUS_FILE));
    }
}

#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // A GUI-subsystem program starts without a console; print into the one it was run from.
    // When stdout is already redirected (a pipe or file), the handles work as they are.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_console() {}
