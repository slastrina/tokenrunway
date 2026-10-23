//! Rattleboard (../rattleboard, the split-flap board app): alerts pushed to the board and an "AI
//! runway" card in its Sources rotation (SPEC.md §8.4).
//!
//! Two ways in:
//! - **This computer**: the `rattleboard --notify` CLI, which uses the app's own key, so nothing
//!   needs setting up. With the key as well (from a pair link), the local API server too.
//! - **A board on the network**: its API (`/rattleboard/notify`, the Sources API) with the key
//!   from a pair link or typed in; a real Vestaboard (or an old Rattleboard) gets a plain
//!   `/local-api/message` layout instead.
//!
//! The text is pure functions here (tested at 6×22, 3×15 and 8×30); the calls are at the end.
//! Only window names, percentages and times ever reach the board: no prompts, no costs, no keys.

use crate::allowance::alerts::{Alert, Kind};
use crate::forecast;
use crate::view::{AccountState, AccountView, Status, Summary, WindowView};
use chrono::{Local, TimeZone};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

pub const PLUGIN_ID: &str = "com.iokig.tokenrunway";
/// The board's history labels what we send with this.
pub const SOURCE: &str = "Tokenrunway";
/// Rattleboard's API port unless its settings say otherwise (a real Vestaboard uses 7000).
pub const DEFAULT_PORT: u16 = 7070;
pub const KEY_HEADER: &str = "X-Vestaboard-Local-Api-Key";
/// The keychain entry for the board's key (secrets.rs).
pub const KEY_NAME: &str = "rattleboard-key";
/// How long an alert stays up, and the Test message.
pub const ALERT_SECONDS: u64 = 30;
pub const TEST_SECONDS: u64 = 15;
pub const TEST_TEXT: &str = "TOKENRUNWAY CONNECTED {66}";
/// The card is re-sent at most this often (sooner when a window turns critical).
pub const CARD_EVERY_MS: i64 = 5 * 60_000;
const MAX_ROWS: usize = 12;
const MAX_COLS: usize = 40;
/// Rattleboard's app identifier, for its settings folder (the API port).
const RB_IDENTIFIER: &str = "io.github.slastrina.rattleboard";

pub const RED: u8 = 63;
pub const ORANGE: u8 = 64;
pub const GREEN: u8 = 66;
pub const BLUE: u8 = 67;
pub const VIOLET: u8 = 68;
const CHIPS: [(&str, u8); 8] =
    [("red", 63), ("orange", 64), ("yellow", 65), ("green", 66), ("blue", 67), ("violet", 68), ("white", 69), ("black", 70)];

pub fn chip_name(c: u8) -> &'static str {
    CHIPS.iter().find(|x| x.1 == c).map(|x| x.0).unwrap_or("")
}

pub fn status_chip(s: Status) -> u8 {
    match s {
        Status::Crit => RED,
        Status::Warn => ORANGE,
        Status::Ok => GREEN,
        Status::Est => BLUE,
    }
}

// ---------------------------------------------------------------- pair links and addresses

#[derive(Debug, Clone, PartialEq)]
pub struct PairLink {
    /// "http://192.168.1.20:7070"
    pub url: String,
    pub key: String,
    pub name: String,
    pub rows: usize,
    pub cols: usize,
}

/// `rattleboard://pair?v=2&url=…&key=…&name=…&rows=…&cols=…` (Rattleboard → Settings → API
/// server → Copy link). Errors never repeat the key.
pub fn parse_pair(link: &str) -> Result<PairLink, String> {
    let u = url::Url::parse(link.trim()).map_err(|_| "That isn't a pair link. Copy it from Rattleboard → Settings → API server.")?;
    if u.scheme() != "rattleboard" || u.host_str() != Some("pair") {
        return Err("Pair links start with rattleboard://pair".into());
    }
    let q = |k: &str| u.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    if q("v").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0) < 2 {
        return Err("This pair link is from an older Rattleboard. Update it and copy the link again.".into());
    }
    let url = normalize_url(&q("url").ok_or("The pair link has no address")?)?;
    let key = q("key").map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).ok_or("The pair link has no key")?;
    let rows = q("rows").and_then(|v| v.parse::<usize>().ok()).unwrap_or(6).clamp(1, MAX_ROWS);
    let cols = q("cols").and_then(|v| v.parse::<usize>().ok()).unwrap_or(22).clamp(1, MAX_COLS);
    let name = q("name").map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Rattleboard".into());
    Ok(PairLink { url, key, name, rows, cols })
}

/// "192.168.1.20", "tv.local:7070" or "http://tv.local:7070/" → "http://tv.local:7070".
pub fn normalize_url(addr: &str) -> Result<String, String> {
    let a = addr.trim().trim_end_matches('/');
    if a.is_empty() {
        return Err("Enter the board's address, like 192.168.1.20:7070".into());
    }
    let full = if a.contains("://") { a.to_string() } else { format!("http://{a}") };
    let u = url::Url::parse(&full).map_err(|_| format!("{a} isn't an address Tokenrunway can use"))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err("The address must be http:// or https://".into());
    }
    let host = u.host_str().filter(|h| !h.is_empty()).ok_or("The address has no host")?;
    let port = u.port().unwrap_or(if u.scheme() == "https" { 443 } else { DEFAULT_PORT });
    let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_string() };
    Ok(format!("{}://{host}:{port}", u.scheme()))
}

/// "192.168.1.20:7070" for the status line.
pub fn host_of(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).trim_end_matches('/').to_string()
}

// ---------------------------------------------------------------- /rattleboard/info

#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub name: String,
    pub rows: usize,
    pub cols: usize,
    /// Has POST /rattleboard/notify
    pub notify: bool,
    /// Has the Sources API (plugins and the rotation)
    pub sources: bool,
}

/// GET /rattleboard/info's answer; None if it isn't a Rattleboard's.
pub fn parse_info(v: &Value) -> Option<Info> {
    if v["app"] != "rattleboard" {
        return None;
    }
    let has = |f: &str| v["features"].as_array().is_some_and(|a| a.iter().any(|x| x == f));
    Some(Info {
        name: v["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Rattleboard").to_string(),
        rows: (v["board"]["rows"].as_u64().unwrap_or(6) as usize).clamp(1, MAX_ROWS),
        cols: (v["board"]["cols"].as_u64().unwrap_or(22) as usize).clamp(1, MAX_COLS),
        notify: has("notify"),
        sources: has("sources"),
    })
}

/// What's at an address with a key.
#[derive(Debug, Clone, PartialEq)]
pub enum Board {
    Rattleboard(Info),
    /// A real Vestaboard's Local API (or a Rattleboard too old for /rattleboard/info)
    Vestaboard {
        rows: usize,
        cols: usize,
    },
}

impl Board {
    pub fn name(&self) -> String {
        match self {
            Board::Rattleboard(i) => i.name.clone(),
            Board::Vestaboard { .. } => "Vestaboard".into(),
        }
    }
    pub fn size(&self) -> (usize, usize) {
        match self {
            Board::Rattleboard(i) => (i.rows, i.cols),
            Board::Vestaboard { rows, cols } => (*rows, *cols),
        }
    }
    pub fn notify(&self) -> bool {
        matches!(self, Board::Rattleboard(i) if i.notify)
    }
}

// ---------------------------------------------------------------- tiles

/// Upper-cased, with only what a board has tiles for (A–Z, 0–9 and its punctuation); `{63}`
/// codes are kept, dashes of any kind become "-", anything else a space (runs collapsed).
pub fn clean(s: &str) -> String {
    const PUNCT: &str = "!@#$()-+&=;:'\"%,./?°";
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let code: String = chars.clone().take_while(|d| d.is_ascii_digit()).collect();
            if (1..=2).contains(&code.len()) && chars.clone().nth(code.len()) == Some('}') {
                out.push_str(&format!("{{{code}}}"));
                for _ in 0..=code.len() {
                    chars.next();
                }
                continue;
            }
        }
        let c = match c {
            '—' | '–' | '‐' | '‑' => '-',
            '’' | '‘' => '\'',
            '“' | '”' => '"',
            '·' => ' ',
            c => c,
        };
        let up: String = c.to_uppercase().collect();
        for u in up.chars() {
            if u.is_ascii_uppercase() || u.is_ascii_digit() || PUNCT.contains(u) || u == '\n' {
                out.push(u);
            } else if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
        }
    }
    out.split('\n').map(|l| l.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n")
}

/// How many tiles a line takes: a `{63}` code is one.
pub fn tiles(line: &str) -> usize {
    tokens(line).len()
}

/// A line as tiles: a char, or a code from `{nn}`.
fn tokens(line: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        if c == '{' {
            if let Some(end) = rest.find('}') {
                let inner = &rest[1..end];
                if (1..=2).contains(&inner.len()) && inner.bytes().all(|b| b.is_ascii_digit()) {
                    out.push(rest[..=end].to_string());
                    rest = &rest[end + 1..];
                    continue;
                }
            }
        }
        out.push(c.to_string());
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// The first `n` tiles of a line.
fn cut(line: &str, n: usize) -> String {
    tokens(line).into_iter().take(n).collect::<String>().trim_end().to_string()
}

/// A Vestaboard character code (0 for anything without a tile).
fn code(t: &str) -> u8 {
    const SYM: [(char, u8); 20] = [
        ('!', 37),
        ('@', 38),
        ('#', 39),
        ('$', 40),
        ('(', 41),
        (')', 42),
        ('-', 44),
        ('+', 46),
        ('&', 47),
        ('=', 48),
        (';', 49),
        (':', 50),
        ('\'', 52),
        ('"', 53),
        ('%', 54),
        (',', 55),
        ('.', 56),
        ('/', 59),
        ('?', 60),
        ('°', 62),
    ];
    if let Some(n) = t.strip_prefix('{').and_then(|x| x.strip_suffix('}')).and_then(|x| x.parse::<u8>().ok()) {
        return if n <= 71 { n } else { 0 };
    }
    match t.chars().next() {
        Some(c @ 'A'..='Z') => c as u8 - b'A' + 1,
        Some(c @ '1'..='9') => c as u8 - b'1' + 27,
        Some('0') => 36,
        Some(c) => SYM.iter().find(|s| s.0 == c).map(|s| s.1).unwrap_or(0),
        None => 0,
    }
}

/// Text as a `rows`×`cols` layout of character codes: each line word-wrapped, centred, and the
/// block centred vertically. For boards without `{"text"}` posts (a real Vestaboard).
pub fn layout(text: &str, rows: usize, cols: usize) -> Vec<Vec<u8>> {
    let mut lines: Vec<String> = vec![];
    for para in clean(text).split('\n') {
        let mut line = String::new();
        for word in para.split(' ').filter(|w| !w.is_empty()) {
            let word = if tiles(word) > cols { cut(word, cols) } else { word.to_string() };
            if line.is_empty() {
                line = word;
            } else if tiles(&line) + 1 + tiles(&word) <= cols {
                line = format!("{line} {word}");
            } else {
                lines.push(std::mem::take(&mut line));
                line = word;
            }
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.truncate(rows);
    let top = (rows - lines.len()) / 2;
    let mut grid = vec![vec![0u8; cols]; rows];
    for (i, l) in lines.iter().enumerate() {
        let t = tokens(l);
        let left = (cols - t.len()) / 2;
        for (j, tok) in t.iter().enumerate() {
            grid[top + i][left + j] = code(tok);
        }
    }
    grid
}

// ---------------------------------------------------------------- short names

/// "CLAUDE" for "Claude — work", "AWS" for "AWS Bedrock — prod"; a window label that starts
/// with a product of its own ("Codex weekly") gives that instead ("CODEX").
pub fn short_account(a: &AccountView, w: Option<&WindowView>) -> String {
    let first = |s: &str| s.split(|c: char| c.is_whitespace() || c == '—' || c == '–' || c == '·').find(|p| !p.is_empty()).map(clean);
    if let Some(w) = w {
        let mut words = w.label.split_whitespace();
        if let (Some(lead), Some(_)) = (words.next(), words.next()) {
            let l = lead.to_lowercase();
            let id_word = w.id.split('-').next().unwrap_or("");
            if l.chars().all(|c| c.is_alphabetic()) && l != id_word && !matches!(l.as_str(), "weekly" | "monthly" | "daily" | "hourly") {
                return clean(lead);
            }
        }
    }
    first(&a.name).filter(|s| !s.is_empty()).unwrap_or_else(|| clean(&a.provider))
}

/// 5h → "5H", weekly → "WK", weekly-opus → "WK OPUS", a monthly budget → "MO $".
pub fn short_window(w: &WindowView) -> String {
    let id = w.id.as_str();
    let money = w.abs.as_deref().is_some_and(|a| a.contains('$')) || w.label.to_lowercase().contains("budget");
    match id {
        "weekly" => "WK".into(),
        "monthly" | "budget" if money => "MO $".into(),
        "monthly" => "MO".into(),
        "budget" => "$".into(),
        "daily" => "DAY".into(),
        _ => match id.strip_prefix("weekly-") {
            Some(rest) => format!("WK {}", clean(&rest.replace('-', " "))),
            None => clean(&id.replace('-', " ")),
        },
    }
}

/// When a window resets, in as few tiles as possible: "14:20" today, "TUE" this week, "1 NOV".
pub fn short_reset_in<Tz: TimeZone>(at: i64, now: i64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let (Some(a), Some(n)) = (tz.timestamp_millis_opt(at).single(), tz.timestamp_millis_opt(now).single()) else { return String::new() };
    if a.date_naive() == n.date_naive() {
        a.format("%H:%M").to_string()
    } else if at - now < 6 * 24 * 3600 * 1000 {
        a.format("%a").to_string().to_uppercase()
    } else {
        a.format("%-d %b").to_string().to_uppercase()
    }
}

fn pct(p: f64) -> String {
    format!("{:.0}%", p.clamp(0.0, 999.0))
}

// ---------------------------------------------------------------- the card

/// One line per item, the first parts that fit `cols` tiles (the chip is one): the account is
/// shortened before the reset is dropped, and the account dropped last.
fn fit(chip: u8, acct: &str, rest: &[&str], tail: &str, cols: usize) -> String {
    let join = |parts: &[&str]| parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(" ");
    let line = |body: String| format!("{{{chip}}} {body}");
    let body_cols = cols.saturating_sub(2);
    let rest_s = join(rest);
    let fits = |s: &str| tiles(s) <= body_cols;
    let trunc = |min: usize, with_tail: bool| -> Option<String> {
        let after = join(&[&rest_s, if with_tail { tail } else { "" }]);
        let room = body_cols.checked_sub(tiles(&after) + 1)?;
        (room >= min && room < tiles(acct)).then(|| join(&[&cut(acct, room), &after]))
    };
    let candidates = [
        Some(join(&[acct, &rest_s, tail])),
        trunc(4, true),
        Some(join(&[acct, &rest_s])),
        trunc(3, false),
        Some(join(&[&rest_s, tail])),
        Some(rest_s.clone()),
    ];
    for c in candidates.into_iter().flatten() {
        if !c.is_empty() && fits(&c) {
            return line(c);
        }
    }
    cut(&line(rest_s), cols)
}

/// The "AI runway" card: the shortest runway on top, then one line per window, most urgent
/// first, as many as fit. Never more than `cols` tiles a line or `rows` lines.
pub fn card_text(accounts: &[AccountView], summary: Option<&Summary>, rows: usize, cols: usize, now: i64) -> String {
    card_text_in(accounts, summary, rows, cols, now, &Local)
}

pub fn card_text_in<Tz: TimeZone>(
    accounts: &[AccountView],
    summary: Option<&Summary>,
    rows: usize,
    cols: usize,
    now: i64,
    tz: &Tz,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let (rows, cols) = (rows.clamp(1, MAX_ROWS), cols.clamp(1, MAX_COLS));
    let head = match summary {
        Some(s) => {
            let label = clean(&s.label);
            [format!("AI RUNWAY {label}"), format!("RUNWAY {label}"), label].into_iter().find(|h| tiles(h) <= cols).unwrap_or_default()
        }
        None => "AI RUNWAY".into(),
    };
    let mut lines = vec![cut(&head, cols)];
    // (rank, run-out, -pct, line)
    let mut items: Vec<(u8, i64, i64, String)> = vec![];
    for a in accounts {
        if a.state == AccountState::Login {
            items.push((1, i64::MAX, 0, fit(VIOLET, &short_account(a, None), &["SIGN IN"], "", cols)));
            continue;
        }
        for w in &a.windows {
            let rank = match w.status {
                Status::Crit => 0,
                Status::Warn => 2,
                Status::Ok => 3,
                Status::Est => 4,
            };
            let reset = w.resets_at.filter(|r| *r > now).map(|r| short_reset_in(r, now, tz)).unwrap_or_default();
            let line = fit(status_chip(w.status), &short_account(a, Some(w)), &[&short_window(w), &pct(w.pct)], &reset, cols);
            items.push((rank, w.out_at.unwrap_or(i64::MAX), -(w.pct * 10.0) as i64, line));
        }
    }
    items.sort_by_key(|x| (x.0, x.1, x.2));
    if items.is_empty() && rows > 1 {
        lines.push(cut("NO ALLOWANCES YET", cols));
    }
    lines.extend(items.into_iter().map(|i| i.3).take(rows.saturating_sub(1)));
    lines.join("\n")
}

/// The card's chip colour: the most urgent window's.
pub fn card_chip(accounts: &[AccountView]) -> &'static str {
    let worst = accounts.iter().flat_map(|a| a.windows.iter()).map(|w| w.status).max_by_key(|s| forecast::severity(*s));
    chip_name(status_chip(worst.unwrap_or(Status::Ok)))
}

/// The plugin manifest (POST /rattleboard/sources/plugins; the same id updates it). A `text`
/// card has `every` at the top level (rattleboard-core plugins.rs).
pub fn manifest(text: &str, chip: &str) -> Value {
    json!({
        "format": 1,
        "id": PLUGIN_ID,
        "name": "Tokenrunway",
        "description": "AI runway: allowances and resets, from Tokenrunway",
        "kind": "text",
        "every": "1m",
        "card": { "template": text, "align": "left", "chip": chip },
        "dwell": "30s",
    })
}

/// The PUT /rattleboard/sources/rotation body that adds the card, from GET /rattleboard/sources;
/// None if it's already there. Every other source goes back exactly as it came (unknown fields
/// such as `label` or `secrets` are ignored by the board), and nothing but the rotation is sent.
pub fn rotation_with_card(sources: &Value) -> Option<Value> {
    let rotation = sources["rotation"].as_array().cloned().unwrap_or_default();
    if rotation.iter().any(|x| x["plugin"] == PLUGIN_ID) {
        return None;
    }
    let mut out = rotation;
    out.push(json!({ "plugin": PLUGIN_ID, "dwell": "30s" }));
    Some(json!({ "rotation": out }))
}

// ---------------------------------------------------------------- alerts

/// Which notifications go to the board: "off" | "alerts" (critical, projected run-out, login
/// expired) | "warnings" (also warnings) | "all" (also resets).
pub fn wanted(level: &str, kind: Kind) -> bool {
    let rank = match level {
        "alerts" => 1,
        "warnings" => 2,
        "all" => 3,
        _ => return false,
    };
    let need = match kind {
        Kind::Crit | Kind::Projection | Kind::Login => 1,
        Kind::Warn => 2,
        Kind::Reset => 3,
    };
    rank >= need
}

/// Something to put on the board once.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// Without the chip: "CLAUDE 5H OUT AT 13:05\n1H 15M BEFORE RESET"
    pub text: String,
    pub chip: Option<u8>,
    /// Gets through a Focus (priority "alert")
    pub alert: bool,
    /// The provider's usage page
    pub link: Option<String>,
}

impl Note {
    pub fn test() -> Self {
        Note { text: TEST_TEXT.into(), chip: None, alert: false, link: None }
    }

    /// With the chip in front: "{63} CLAUDE 5H OUT AT 13:05\n…"
    pub fn board_text(&self) -> String {
        match self.chip {
            Some(c) => format!("{{{c}}} {}", self.text),
            None => self.text.clone(),
        }
    }

    /// POST /rattleboard/notify
    pub fn notify_body(&self, seconds: u64) -> Value {
        let mut v = json!({
            "text": self.board_text(),
            "seconds": seconds,
            "source": SOURCE,
            "priority": if self.alert { "alert" } else { "normal" },
        });
        if let Some(l) = &self.link {
            v["link"] = json!(l);
        }
        v
    }

    /// `rattleboard --notify …` (after the program). `--seconds` can't go with `--target`.
    pub fn cli_args(&self, seconds: u64, target: &str) -> Vec<String> {
        let mut a = vec!["--notify".to_string(), self.text.clone()];
        if let Some(c) = self.chip {
            a.extend(["--color".into(), chip_name(c).into()]);
        }
        let target = target.trim();
        if target.is_empty() {
            a.extend(["--seconds".into(), seconds.to_string()]);
        }
        a.extend(["--source".into(), SOURCE.into()]);
        if !target.is_empty() {
            a.extend(["--target".into(), target.into()]);
        }
        a
    }
}

/// "13:05", "THU 15:00", "1 NOV"
fn at_words<Tz: TimeZone>(at: i64, now: i64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    forecast::when_in(at, now, tz).to_uppercase()
}

/// The board's version of a notification, if the level wants it.
pub fn note_for(alert: &Alert, accounts: &[AccountView], level: &str, link: Option<String>, now: i64) -> Option<Note> {
    note_for_in(alert, accounts, level, link, now, &Local)
}

pub fn note_for_in<Tz: TimeZone>(
    alert: &Alert,
    accounts: &[AccountView],
    level: &str,
    link: Option<String>,
    now: i64,
    tz: &Tz,
) -> Option<Note>
where
    Tz::Offset: std::fmt::Display,
{
    if !wanted(level, alert.kind) {
        return None;
    }
    let a = accounts.iter().find(|a| a.id == alert.account_id)?;
    if alert.kind == Kind::Login {
        let text = format!("{} LOGIN EXPIRED\nSIGN IN AGAIN", short_account(a, None));
        return Some(Note { text, chip: Some(VIOLET), alert: false, link });
    }
    let w = a.windows.iter().find(|w| Some(&w.id) == alert.window_id.as_ref())?;
    let name = format!("{} {}", short_account(a, Some(w)), short_window(w));
    let resets = w.resets_at.map(|r| format!("RESETS {}", at_words(r, now, tz)));
    let (text, chip) = match alert.kind {
        Kind::Projection => {
            let (o, r) = (w.out_at?, w.resets_at?);
            (format!("{name} OUT AT {}\n{} BEFORE RESET", at_words(o, now, tz), clean(&forecast::duration(r - o))), RED)
        }
        Kind::Crit | Kind::Warn => {
            let first = if w.pct >= 100.0 { format!("{name} USED UP") } else { format!("{name} AT {}", pct(w.pct)) };
            let second = match w.out_at {
                Some(o) if w.pct < 100.0 => Some(format!("OUT AT {}", at_words(o, now, tz))),
                _ => resets,
            };
            let text = match second {
                Some(s) => format!("{first}\n{s}"),
                None => first,
            };
            (text, if alert.kind == Kind::Crit { RED } else { ORANGE })
        }
        Kind::Reset => (format!("{name} RESET\nBACK TO {}", pct(w.pct)), GREEN),
        Kind::Login => unreachable!(),
    };
    Some(Note { text: clean(&text), chip: Some(chip), alert: matches!(alert.kind, Kind::Crit | Kind::Projection), link })
}

// ---------------------------------------------------------------- this computer

/// Where the Rattleboard app's binary is, if it's installed here.
pub fn find_binary() -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = vec![];
    if cfg!(target_os = "macos") {
        c.push("/Applications/Rattleboard.app/Contents/MacOS/rattleboard".into());
        if let Some(h) = dirs::home_dir() {
            c.push(h.join("Applications/Rattleboard.app/Contents/MacOS/rattleboard"));
        }
    }
    if cfg!(windows) {
        for var in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(d) = std::env::var_os(var) {
                c.push(PathBuf::from(d).join("Rattleboard").join("rattleboard.exe"));
            }
        }
    }
    if cfg!(target_os = "linux") {
        c.push("/usr/bin/rattleboard".into());
    }
    c.into_iter().find(|p| p.is_file()).or_else(|| crate::allowance::find_program("rattleboard"))
}

/// The API port in Rattleboard's settings.json ("serverPort"), or 7070.
pub fn port_from_settings(json: &str) -> u16 {
    serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|v| v["serverPort"].as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(DEFAULT_PORT)
}

/// This computer's Rattleboard API server.
pub fn local_url() -> String {
    let settings = dirs::config_dir().map(|d| d.join(RB_IDENTIFIER).join("settings.json"));
    let port = settings.and_then(|p| std::fs::read_to_string(p).ok()).map(|s| port_from_settings(&s)).unwrap_or(DEFAULT_PORT);
    format!("http://127.0.0.1:{port}")
}

/// Sends a note with `rattleboard --notify` (the app's own key; nothing to set up).
pub async fn notify_cli(bin: &std::path::Path, note: &Note, seconds: u64, target: &str) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(note.cli_args(seconds, target)).stdin(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000);
    }
    let out = tokio::time::timeout(Duration::from_secs(20), cmd.output())
        .await
        .map_err(|_| "rattleboard --notify didn't finish in 20 s".to_string())?
        .map_err(|e| format!("Couldn't run rattleboard: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let line = err.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("Usage:")).unwrap_or("");
    let line = line.strip_prefix("rattleboard --notify: ").unwrap_or(line);
    Err(match out.status.code() {
        Some(c) if line.is_empty() => format!("rattleboard --notify failed (exit {c})"),
        Some(c) => format!("rattleboard --notify failed (exit {c}): {line}"),
        None => "rattleboard --notify was stopped".into(),
    })
}

// ---------------------------------------------------------------- the API

fn describe(e: &reqwest::Error, url: &str) -> String {
    let host = host_of(url);
    if e.is_timeout() {
        format!("{host} didn't answer in time")
    } else if e.is_connect() {
        format!("Couldn't connect to {host}. Is the board on, with its API server turned on?")
    } else {
        format!("Couldn't reach {host}")
    }
}

/// The reason in an error answer ({"error": "…"} or text), short.
async fn reason(r: reqwest::Response) -> String {
    let status = r.status();
    let body = r.text().await.unwrap_or_default();
    let msg = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v["error"].as_str().or(v["message"].as_str()).map(str::to_string))
        .unwrap_or_else(|| body.trim().to_string());
    let msg: String = msg.chars().take(160).collect();
    if msg.is_empty() {
        format!("the board answered {}", status.as_u16())
    } else {
        format!("the board answered {}: {msg}", status.as_u16())
    }
}

const REFUSED: &str = "The board refused the key. Copy the pair link from Rattleboard → Settings → API server again.";

/// Whether a Rattleboard API server answers at `url` at all (401 without a key is a yes).
pub async fn server_there(http: &reqwest::Client, url: &str) -> bool {
    match http.get(format!("{url}/rattleboard/info")).timeout(Duration::from_secs(2)).send().await {
        Ok(r) => matches!(r.status().as_u16(), 200 | 401),
        Err(_) => false,
    }
}

/// What's at `url` with `key`: a Rattleboard (its info) or a Vestaboard (a 404 or 401 on
/// /rattleboard/info but a layout from /local-api/message).
pub async fn probe(http: &reqwest::Client, url: &str, key: &str) -> Result<Board, String> {
    let r = http
        .get(format!("{url}/rattleboard/info"))
        .header(KEY_HEADER, key)
        .timeout(Duration::from_secs(6))
        .send()
        .await
        .map_err(|e| describe(&e, url))?;
    if r.status().is_success() {
        if let Some(info) = r.json::<Value>().await.ok().as_ref().and_then(parse_info) {
            return Ok(Board::Rattleboard(info));
        }
    }
    let r = http
        .get(format!("{url}/local-api/message"))
        .header(KEY_HEADER, key)
        .timeout(Duration::from_secs(6))
        .send()
        .await
        .map_err(|e| describe(&e, url))?;
    match r.status().as_u16() {
        200 => {
            let v: Value = r.json().await.unwrap_or(Value::Null);
            // A layout array, or {"message": [[…]]} as some firmware answers
            let grid = if v.is_array() { &v } else { &v["message"] };
            let rows = grid.as_array().map(|a| a.len()).filter(|n| *n > 0).unwrap_or(6).min(MAX_ROWS);
            let cols = grid[0].as_array().map(|a| a.len()).filter(|n| *n > 0).unwrap_or(22).min(MAX_COLS);
            Ok(Board::Vestaboard { rows, cols })
        }
        401 | 403 => Err(REFUSED.into()),
        _ => Err(format!("That doesn't look like a Rattleboard or a Vestaboard ({})", reason(r).await)),
    }
}

async fn check(r: Result<reqwest::Response, reqwest::Error>, url: &str) -> Result<reqwest::Response, String> {
    let r = r.map_err(|e| describe(&e, url))?;
    match r.status().as_u16() {
        200..=299 => Ok(r),
        401 | 403 => Err(REFUSED.into()),
        _ => Err(reason(r).await),
    }
}

/// Shows a note once: POST /rattleboard/notify where the board has it (`notify`), else (or on
/// a 404) a plain layout to /local-api/message at `rows`×`cols`.
pub async fn notify(
    http: &reqwest::Client,
    url: &str,
    key: &str,
    note: &Note,
    seconds: u64,
    notify: bool,
    size: (usize, usize),
) -> Result<(), String> {
    if notify {
        let r = http.post(format!("{url}/rattleboard/notify")).header(KEY_HEADER, key).json(&note.notify_body(seconds)).send().await;
        match r {
            Ok(r) if r.status().as_u16() == 404 => {}
            r => return check(r, url).await.map(|_| ()),
        }
    }
    let grid = layout(&note.board_text(), size.0, size.1);
    let r = http.post(format!("{url}/local-api/message")).header(KEY_HEADER, key).json(&grid).send().await;
    check(r, url).await.map(|_| ())
}

/// Installs (or updates) the card's plugin, and with `ensure` puts it in the rotation if it
/// isn't there. Returns whether it was added to the rotation.
pub async fn push_card(http: &reqwest::Client, url: &str, key: &str, manifest: &Value, ensure: bool) -> Result<bool, String> {
    let r = http.post(format!("{url}/rattleboard/sources/plugins")).header(KEY_HEADER, key).json(manifest).send().await;
    check(r, url).await?;
    if !ensure {
        return Ok(false);
    }
    let r = http.get(format!("{url}/rattleboard/sources")).header(KEY_HEADER, key).send().await;
    let sources: Value = check(r, url).await?.json().await.map_err(|_| "The board's sources didn't make sense".to_string())?;
    let Some(body) = rotation_with_card(&sources) else { return Ok(false) };
    let r = http.put(format!("{url}/rattleboard/sources/rotation")).header(KEY_HEADER, key).json(&body).send().await;
    check(r, url).await?;
    Ok(true)
}

/// Removes the card's plugin (and its source in the rotation, which the board does itself).
pub async fn remove_card(http: &reqwest::Client, url: &str, key: &str) -> Result<(), String> {
    let r = http.delete(format!("{url}/rattleboard/sources/plugins/{PLUGIN_ID}")).header(KEY_HEADER, key).send().await;
    match r {
        Ok(r) if r.status().as_u16() == 404 => Ok(()),
        r => check(r, url).await.map(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    const MIN: i64 = 60_000;
    const H: i64 = 60 * MIN;
    const D: i64 = 24 * H;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    /// Thu 22 Oct 2026, 11:54 UTC
    const NOW: i64 = 1_792_670_040_000;

    #[allow(clippy::too_many_arguments)]
    fn win(id: &str, label: &str, abs: Option<&str>, pct: f64, reset_in: i64, status: Status, out_in: Option<i64>) -> WindowView {
        WindowView {
            id: id.into(),
            label: label.into(),
            abs: abs.map(str::to_string),
            pct,
            resets_at: Some(NOW + reset_in),
            status,
            out_at: out_in.map(|o| NOW + o),
            ..Default::default()
        }
    }

    fn sample() -> (Vec<AccountView>, Summary) {
        let claude = AccountView {
            id: "acct-1".into(),
            provider: "anthropic".into(),
            name: "Claude — work".into(),
            windows: vec![
                win("5h", "5-hour", Some("412k / 530k tokens"), 78.0, 2 * H + 26 * MIN, Status::Crit, Some(H + 26 * MIN)),
                win("weekly", "Weekly", None, 64.0, 4 * D + 21 * H, Status::Ok, None),
                win("weekly-opus", "Weekly Opus", None, 83.0, 4 * D + 21 * H, Status::Warn, None),
            ],
            ..Default::default()
        };
        let chatgpt = AccountView {
            id: "acct-2".into(),
            provider: "openai".into(),
            name: "ChatGPT — personal".into(),
            windows: vec![
                win("5h", "Codex 5-hour", None, 12.0, 3 * H + 52 * MIN, Status::Ok, None),
                win("weekly", "Codex weekly", None, 31.0, 5 * D, Status::Ok, None),
            ],
            ..Default::default()
        };
        let bedrock = AccountView {
            id: "acct-3".into(),
            provider: "bedrock".into(),
            name: "AWS Bedrock — iokig-prod".into(),
            windows: vec![win("monthly", "Monthly budget", Some("$184 / $500"), 37.0, 9 * D + 13 * H, Status::Est, None)],
            ..Default::default()
        };
        let s = Summary { label: "1h 26m".into(), status: Status::Crit, ..Default::default() };
        (vec![claude, chatgpt, bedrock], s)
    }

    fn check_fits(text: &str, rows: usize, cols: usize) {
        let lines: Vec<&str> = text.split('\n').collect();
        assert!(lines.len() <= rows, "{} lines on {rows} rows:\n{text}", lines.len());
        for l in &lines {
            assert!(tiles(l) <= cols, "{l:?} is {} tiles on {cols}", tiles(l));
        }
        assert_eq!(clean(text), text, "only board characters");
    }

    #[test]
    fn card_6x22() {
        let (accts, s) = sample();
        let t = card_text_in(&accts, Some(&s), 6, 22, NOW, &tz());
        assert_eq!(
            t,
            "AI RUNWAY 1H 26M\n\
             {63} CLAUDE 5H 78% 14:20\n\
             {64} CLAU WK OPUS 83% TUE\n\
             {66} CLAUDE WK 64% TUE\n\
             {66} CODEX WK 31% TUE\n\
             {66} CODEX 5H 12% 15:46"
        );
        check_fits(&t, 6, 22);
    }

    #[test]
    fn card_3x15() {
        let (accts, s) = sample();
        let t = card_text_in(&accts, Some(&s), 3, 15, NOW, &tz());
        assert_eq!(t, "RUNWAY 1H 26M\n{63} CLAUDE 5H 78%\n{64} WK OPUS 83%");
        check_fits(&t, 3, 15);
    }

    #[test]
    fn card_8x30() {
        let (mut accts, s) = sample();
        accts.push(AccountView { id: "acct-4".into(), name: "Gemini".into(), state: AccountState::Login, ..Default::default() });
        let t = card_text_in(&accts, Some(&s), 8, 30, NOW, &tz());
        assert_eq!(
            t,
            "AI RUNWAY 1H 26M\n\
             {63} CLAUDE 5H 78% 14:20\n\
             {68} GEMINI SIGN IN\n\
             {64} CLAUDE WK OPUS 83% TUE\n\
             {66} CLAUDE WK 64% TUE\n\
             {66} CODEX WK 31% TUE\n\
             {66} CODEX 5H 12% 15:46\n\
             {67} AWS MO $ 37% 1 NOV"
        );
        check_fits(&t, 8, 30);
    }

    #[test]
    fn card_edges() {
        assert_eq!(card_text_in(&[], None, 6, 22, NOW, &tz()), "AI RUNWAY\nNO ALLOWANCES YET");
        assert_eq!(card_text_in(&[], None, 1, 8, NOW, &tz()), "AI RUNWA");
        // A long name and a tiny board still fit.
        let a = AccountView {
            name: "Supercalifragilistic — team".into(),
            windows: vec![win("weekly-sonnet-extra", "Weekly Sonnet extra", None, 100.0, 30 * MIN, Status::Crit, None)],
            ..Default::default()
        };
        for (r, c) in [(2, 8), (3, 15), (6, 22), (12, 40)] {
            check_fits(&card_text_in(std::slice::from_ref(&a), None, r, c, NOW, &tz()), r, c);
        }
        assert_eq!(card_chip(&sample().0), "red");
        assert_eq!(card_chip(&[]), "green");
    }

    #[test]
    fn short_names() {
        let (accts, _) = sample();
        assert_eq!(short_account(&accts[0], Some(&accts[0].windows[0])), "CLAUDE");
        assert_eq!(short_account(&accts[1], Some(&accts[1].windows[1])), "CODEX");
        assert_eq!(short_account(&accts[1], None), "CHATGPT");
        assert_eq!(short_account(&accts[2], Some(&accts[2].windows[0])), "AWS");
        assert_eq!(short_window(&accts[0].windows[2]), "WK OPUS");
        assert_eq!(short_window(&accts[2].windows[0]), "MO $");
        assert_eq!(short_reset_in(NOW + 2 * H, NOW, &tz()), "13:54");
        assert_eq!(short_reset_in(NOW + 2 * D, NOW, &tz()), "SAT");
        assert_eq!(short_reset_in(NOW + 10 * D, NOW, &tz()), "1 NOV");
    }

    #[test]
    fn tiles_and_clean() {
        assert_eq!(tiles("{63} CLAUDE"), 8);
        assert_eq!(tiles("{x} A"), 5, "not a code");
        assert_eq!(clean("Claude — work · 5-hour"), "CLAUDE - WORK 5-HOUR");
        assert_eq!(clean("{63} héllo wörld™"), "{63} H LLO W RLD");
        assert_eq!(clean("a\nb"), "A\nB");
        assert_eq!(cut("{63} ABC", 3), "{63} A");
        let g = layout("{66} OK", 3, 5);
        assert_eq!(g, vec![vec![0; 5], vec![66, 0, 15, 11, 0], vec![0; 5]]);
        let g = layout("TOKENRUNWAY CONNECTED {66}", 6, 22);
        assert_eq!(g.len(), 6);
        assert!(g.iter().all(|r| r.len() == 22));
        assert_eq!(g[2][0..11], [20, 15, 11, 5, 14, 18, 21, 14, 23, 1, 25], "TOKENRUNWAY");
    }

    #[test]
    fn pair_links() {
        let p = parse_pair("rattleboard://pair?v=2&url=http%3A%2F%2F192.168.1.20%3A7070&key=k%2By&name=Living%20room%20TV&rows=6&cols=22")
            .unwrap();
        assert_eq!(
            p,
            PairLink { url: "http://192.168.1.20:7070".into(), key: "k+y".into(), name: "Living room TV".into(), rows: 6, cols: 22 }
        );
        let e = parse_pair("rattleboard://pair?v=1&url=http%3A%2F%2Fx%3A7070&key=secret").unwrap_err();
        assert!(e.contains("older"), "{e}");
        let e = parse_pair("rattleboard://pair?v=2&url=http%3A%2F%2Fx%3A7070&key=").unwrap_err();
        assert!(e.contains("no key"));
        assert!(parse_pair("https://example.com").is_err());
        assert!(!parse_pair("rattleboard://pair?v=2&url=nope%20nope&key=supersecret").unwrap_err().contains("supersecret"));
        assert_eq!(parse_pair("rattleboard://pair?v=2&url=http%3A%2F%2Ftv%3A7070&key=k&rows=99").unwrap().rows, 12);
    }

    #[test]
    fn addresses() {
        assert_eq!(normalize_url("192.168.1.20").unwrap(), "http://192.168.1.20:7070");
        assert_eq!(normalize_url(" tv.local:7000/ ").unwrap(), "http://tv.local:7000");
        assert_eq!(normalize_url("http://TV.local:7070/local-api").unwrap(), "http://tv.local:7070");
        assert_eq!(normalize_url("https://board.example").unwrap(), "https://board.example:443");
        assert!(normalize_url("").is_err());
        assert!(normalize_url("ftp://x").is_err());
        assert_eq!(host_of("http://192.168.1.20:7070"), "192.168.1.20:7070");
        assert_eq!(port_from_settings(r#"{"serverEnabled":true,"serverPort":7071}"#), 7071);
        assert_eq!(port_from_settings("{}"), 7070);
        assert_eq!(port_from_settings("nope"), 7070);
    }

    #[test]
    fn info() {
        let v = json!({"app":"rattleboard","protocol":3,"name":"Living room TV","board":{"rows":3,"cols":15},"features":["sequence","notify","sources"]});
        assert_eq!(parse_info(&v), Some(Info { name: "Living room TV".into(), rows: 3, cols: 15, notify: true, sources: true }));
        let old = parse_info(&json!({"app":"rattleboard","board":{}})).unwrap();
        assert_eq!((old.rows, old.cols, old.notify, old.name.as_str()), (6, 22, false, "Rattleboard"));
        assert_eq!(parse_info(&json!({"currentMessage": {}})), None);
    }

    fn alert(kind: Kind, account: &str, window: Option<&str>) -> Alert {
        Alert {
            account_id: account.into(),
            window_id: window.map(str::to_string),
            kind,
            desktop: true,
            title: String::new(),
            body: String::new(),
        }
    }

    #[test]
    fn alert_notes() {
        let (mut accts, _) = sample();
        let link = Some("https://claude.ai/settings/usage".to_string());
        let n = note_for_in(&alert(Kind::Projection, "acct-1", Some("5h")), &accts, "alerts", link.clone(), NOW, &tz()).unwrap();
        assert_eq!(n.text, "CLAUDE 5H OUT AT 13:20\n1H 0M BEFORE RESET");
        assert_eq!(n.board_text(), "{63} CLAUDE 5H OUT AT 13:20\n1H 0M BEFORE RESET");
        assert!(n.alert);
        let b = n.notify_body(30);
        assert_eq!(b["priority"], "alert");
        assert_eq!(b["seconds"], 30);
        assert_eq!(b["source"], "Tokenrunway");
        assert_eq!(b["link"], "https://claude.ai/settings/usage");

        let warn = alert(Kind::Warn, "acct-1", Some("weekly-opus"));
        assert_eq!(note_for_in(&warn, &accts, "alerts", None, NOW, &tz()), None, "warnings need the warnings level");
        let n = note_for_in(&warn, &accts, "warnings", None, NOW, &tz()).unwrap();
        assert_eq!((n.text.as_str(), n.chip, n.alert), ("CLAUDE WK OPUS AT 83%\nRESETS TUE 08:54", Some(ORANGE), false));
        assert_eq!(n.notify_body(30)["priority"], "normal");
        assert!(n.notify_body(30).get("link").is_none());

        accts[0].windows[0].pct = 100.0;
        let n = note_for_in(&alert(Kind::Crit, "acct-1", Some("5h")), &accts, "alerts", None, NOW, &tz()).unwrap();
        assert_eq!(n.text, "CLAUDE 5H USED UP\nRESETS 14:20");
        let reset = alert(Kind::Reset, "acct-2", Some("weekly"));
        assert_eq!(note_for_in(&reset, &accts, "warnings", None, NOW, &tz()), None);
        let n = note_for_in(&reset, &accts, "all", None, NOW, &tz()).unwrap();
        assert_eq!((n.text.as_str(), n.chip), ("CODEX WK RESET\nBACK TO 31%", Some(GREEN)));
        let n = note_for_in(&alert(Kind::Login, "acct-2", None), &accts, "alerts", None, NOW, &tz()).unwrap();
        assert_eq!((n.text.as_str(), n.chip, n.alert), ("CHATGPT LOGIN EXPIRED\nSIGN IN AGAIN", Some(VIOLET), false));
        assert_eq!(note_for_in(&alert(Kind::Crit, "acct-1", Some("5h")), &accts, "off", None, NOW, &tz()), None);
        assert_eq!(note_for_in(&alert(Kind::Crit, "gone", Some("5h")), &accts, "all", None, NOW, &tz()), None);
    }

    #[test]
    fn cli_arguments() {
        let n = Note { text: "CLAUDE 5H OUT AT 13:05".into(), chip: Some(RED), alert: true, link: None };
        assert_eq!(
            n.cli_args(30, ""),
            ["--notify", "CLAUDE 5H OUT AT 13:05", "--color", "red", "--seconds", "30", "--source", "Tokenrunway"]
        );
        assert_eq!(
            n.cli_args(30, " Kitchen "),
            ["--notify", "CLAUDE 5H OUT AT 13:05", "--color", "red", "--source", "Tokenrunway", "--target", "Kitchen"]
        );
        assert_eq!(Note::test().cli_args(15, ""), ["--notify", "TOKENRUNWAY CONNECTED {66}", "--seconds", "15", "--source", "Tokenrunway"]);
    }

    #[test]
    fn manifest_and_rotation() {
        let m = manifest("AI RUNWAY 1H 26M\n{63} CLAUDE 5H 78% 14:20", "red");
        assert_eq!(m["id"], PLUGIN_ID);
        assert_eq!(m["kind"], "text");
        assert_eq!(m["every"], "1m", "text cards take every at the top level");
        assert_eq!(m["card"]["template"], "AI RUNWAY 1H 26M\n{63} CLAUDE 5H 78% 14:20");
        assert_eq!(m["card"]["align"], "left");
        assert_eq!(m["dwell"], "30s");
        let clock = json!({"id": "clock", "plugin": "rattleboard.clock", "enabled": false, "weight": 3, "dwell": "2m", "label": "Clock",
            "schedule": {"days": ["sat"], "from": "07:00", "to": "12:00"}, "secrets": {"token": {"set": true}}, "popup": 0, "chip": "yellow"});
        let sources = json!({"plugins": [], "rotation": [clock.clone()], "quietHours": {"enabled": true}, "holdMinutes": 30});
        let body = rotation_with_card(&sources).unwrap();
        assert_eq!(body.as_object().unwrap().len(), 1, "only the rotation is sent");
        assert_eq!(body["rotation"][0], clock, "kept exactly");
        assert_eq!(body["rotation"][1], json!({"plugin": PLUGIN_ID, "dwell": "30s"}));
        let with = json!({"rotation": [clock, {"id": "x", "plugin": PLUGIN_ID}]});
        assert_eq!(rotation_with_card(&with), None, "added once");
        assert_eq!(rotation_with_card(&json!({})).unwrap(), json!({"rotation": [{"plugin": PLUGIN_ID, "dwell": "30s"}]}));
    }

    // ---------- against a mock board ----------

    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<(String, String, Option<String>, String)>>>;

    /// A one-thread HTTP server: answers each "METHOD /path" from `routes` (status, body) and
    /// records (method, path, key header, body). Unknown routes get 404.
    fn mock(routes: Vec<(&'static str, u16, String)>) -> (String, Seen) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let seen: Seen = Arc::default();
        let s2 = seen.clone();
        std::thread::spawn(move || {
            for stream in l.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = vec![];
                let mut chunk = [0u8; 8192];
                let (head, body) = loop {
                    let n = s.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break (String::new(), String::new());
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(i) = text.find("\r\n\r\n") {
                        let head = text[..i].to_string();
                        let len = head
                            .lines()
                            .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if buf.len() >= i + 4 + len {
                            break (head, String::from_utf8_lossy(&buf[i + 4..i + 4 + len]).to_string());
                        }
                    }
                };
                let mut first = head.lines().next().unwrap_or("").split(' ');
                let (m, p) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
                let key = head
                    .lines()
                    .find_map(|l| l.to_lowercase().starts_with("x-vestaboard-local-api-key:").then(|| l[27..].trim().to_string()));
                s2.lock().unwrap().push((m.clone(), p.clone(), key, body));
                let route = format!("{m} {p}");
                let (status, out) = routes.iter().find(|r| r.0 == route).map(|r| (r.1, r.2.clone())).unwrap_or((404, String::new()));
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                    out.len()
                );
            }
        });
        (url, seen)
    }

    #[tokio::test]
    async fn card_against_a_mock_board() {
        let clock = json!({"id": "clock", "plugin": "rattleboard.clock", "enabled": true, "weight": 2, "dwell": "2m", "label": "Clock",
            "schedule": {"days": [], "from": "06:00", "to": "23:00"}, "settings": {"format": "24h"}, "secrets": {}, "error": null, "fetched": null});
        let weather = json!({"id": "w1", "plugin": "rattleboard.weather", "enabled": false, "settings": {"city": "Melbourne"}, "chip": "yellow", "popup": 5});
        let sources = json!({"plugins": [], "rotation": [clock, weather], "quietHours": {"enabled": true, "from": "22:00", "to": "07:00"}, "holdMinutes": 30, "shuffle": true});
        let (url, seen) = mock(vec![
            ("POST /rattleboard/sources/plugins", 201, r#"{"status":"ok","id":"com.iokig.tokenrunway"}"#.into()),
            ("GET /rattleboard/sources", 200, sources.to_string()),
            ("PUT /rattleboard/sources/rotation", 200, "{}".into()),
            ("DELETE /rattleboard/sources/plugins/com.iokig.tokenrunway", 204, String::new()),
        ]);
        let http = crate::allowance::client();
        let (accts, s) = sample();
        let m = manifest(&card_text(&accts, Some(&s), 6, 22, NOW), card_chip(&accts));
        assert!(push_card(&http, &url, "k3y", &m, true).await.unwrap(), "added to the rotation");
        assert!(!push_card(&http, &url, "k3y", &m, false).await.unwrap(), "an update only re-posts the manifest");
        remove_card(&http, &url, "k3y").await.unwrap();
        let seen = seen.lock().unwrap().clone();
        let calls: Vec<String> = seen.iter().map(|c| format!("{} {}", c.0, c.1)).collect();
        assert_eq!(
            calls,
            [
                "POST /rattleboard/sources/plugins",
                "GET /rattleboard/sources",
                "PUT /rattleboard/sources/rotation",
                "POST /rattleboard/sources/plugins",
                "DELETE /rattleboard/sources/plugins/com.iokig.tokenrunway"
            ]
        );
        assert!(seen.iter().all(|c| c.2.as_deref() == Some("k3y")), "every call has the key");
        let posted: Value = serde_json::from_str(&seen[0].3).unwrap();
        assert_eq!(posted, m);
        let put: Value = serde_json::from_str(&seen[2].3).unwrap();
        assert_eq!(put.as_object().unwrap().keys().collect::<Vec<_>>(), ["rotation"], "quiet hours, shuffle etc. aren't touched");
        assert_eq!(put["rotation"][0], sources["rotation"][0], "the clock comes back exactly");
        assert_eq!(put["rotation"][1], sources["rotation"][1], "the weather too");
        assert_eq!(put["rotation"][2], json!({"plugin": PLUGIN_ID, "dwell": "30s"}));
    }

    #[tokio::test]
    async fn notify_against_mock_boards() {
        let http = crate::allowance::client();
        let note = Note {
            text: "CLAUDE 5H OUT AT 13:05".into(),
            chip: Some(RED),
            alert: true,
            link: Some("https://claude.ai/settings/usage".into()),
        };
        // A Rattleboard with /rattleboard/notify
        let info = r#"{"app":"rattleboard","name":"Living room TV","board":{"rows":6,"cols":22},"features":["notify","sources"]}"#;
        let (url, seen) = mock(vec![
            ("GET /rattleboard/info", 200, info.into()),
            ("POST /rattleboard/notify", 201, r#"{"status":"ok","id":"note-1","seconds":30}"#.into()),
        ]);
        let b = probe(&http, &url, "k").await.unwrap();
        assert_eq!((b.name(), b.size(), b.notify()), ("Living room TV".to_string(), (6, 22), true));
        notify(&http, &url, "k", &note, 30, b.notify(), b.size()).await.unwrap();
        let body: Value = serde_json::from_str(&seen.lock().unwrap()[1].3).unwrap();
        assert_eq!(body["text"], "{63} CLAUDE 5H OUT AT 13:05");
        assert_eq!(body["priority"], "alert");

        // A real Vestaboard: 404 on /rattleboard/info, a layout from the Local API
        let grid = serde_json::to_string(&vec![vec![0; 22]; 6]).unwrap();
        let (url, seen) = mock(vec![("GET /local-api/message", 200, grid), ("POST /local-api/message", 201, "{}".into())]);
        let b = probe(&http, &url, "k").await.unwrap();
        assert_eq!(b, Board::Vestaboard { rows: 6, cols: 22 });
        notify(&http, &url, "k", &note, 30, b.notify(), b.size()).await.unwrap();
        let posted: Vec<Vec<u8>> = serde_json::from_str(&seen.lock().unwrap()[2].3).unwrap();
        assert_eq!(posted, layout("{63} CLAUDE 5H OUT AT 13:05", 6, 22));
        assert!(posted[2].contains(&63));

        // A wrong key
        let (url, _) = mock(vec![("GET /rattleboard/info", 401, "{}".into()), ("GET /local-api/message", 401, "{}".into())]);
        assert!(probe(&http, &url, "nope").await.unwrap_err().contains("refused the key"));
        assert!(server_there(&http, &url).await, "401 without a key: a server is there");
        // Nothing there
        let gone = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        assert!(!server_there(&http, &gone).await);
        assert!(notify(&http, &gone, "k", &note, 30, true, (6, 22)).await.unwrap_err().contains("Couldn't connect"));
    }
}
