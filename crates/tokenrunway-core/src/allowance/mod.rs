//! The allowance half (SPEC.md §5.1, §7.1, §8): the accounts you've added, the providers that
//! read each one's windows, and the snapshot history the forecasts learn from.
//!
//! Every provider reads only the signed-in user's own data, at a human rate, with Tokenrunway's
//! honest User-Agent. Tokens are read when needed and never logged, written or refreshed: a CLI
//! login stays the CLI's.

pub mod accounts;
pub mod admin;
pub mod alerts;
pub mod bedrock;
pub mod claude;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod gemini;
pub mod grok;
pub mod login;
pub mod openrouter;
pub mod request;
pub mod secrets;
pub mod snapshots;

use crate::forecast::{self, Thresholds, WindowInput};
use crate::model::WindowReading;
use crate::view::{AccountState, AccountView, Detected, WindowView};
pub use accounts::{AccountRecord, Accounts};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

pub const USER_AGENT: &str = concat!("Tokenrunway/", env!("CARGO_PKG_VERSION"), " (+https://www.iokig.com/tokenrunway)");

pub const MIN_MS: i64 = 60 * 1000;
pub const HOUR_MS: i64 = 60 * MIN_MS;
pub const DAY_MS: i64 = 24 * HOUR_MS;

/// The HTTP client every provider uses.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .build()
        .expect("http client")
}

/// What a provider needs besides the account: an HTTP client and a directory for its caches.
#[derive(Clone)]
pub struct Ctx {
    pub http: reqwest::Client,
    /// The app data dir (caches such as Bedrock's Cost Explorer reading go here)
    pub data_dir: PathBuf,
    /// The user's home, overridable so tests can point it at fixtures
    pub home: PathBuf,
}

impl Ctx {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { http: client(), data_dir, home: dirs::home_dir().unwrap_or_default() }
    }
}

/// Why a reading failed. Each maps to an account state the UI explains (States.dc.html).
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderError {
    /// The login expired or was revoked; the text says how to fix it.
    AuthExpired(String),
    /// The provider asked us to slow down; retry after this many seconds, if it said.
    RateLimited(Option<u64>),
    /// The response no longer looks like what this reader knows.
    SchemaChanged(String),
    /// Couldn't reach the provider.
    Network(String),
    /// Something on this machine is missing or not allowed (no aws CLI, no permission).
    Unavailable(String),
}

impl ProviderError {
    pub fn state(&self) -> AccountState {
        match self {
            Self::AuthExpired(_) => AccountState::Login,
            Self::RateLimited(_) | Self::Network(_) => AccountState::Stale,
            Self::SchemaChanged(_) | Self::Unavailable(_) => AccountState::Broken,
        }
    }

    /// What kind of problem, so the UI can offer the right fix: "auth" | "ratelimit" | "network" |
    /// "schema" (a Tokenrunway update fixes it) | "setup" (something on the user's side: no aws
    /// CLI, an ended subscription).
    pub fn cause(&self) -> &'static str {
        match self {
            Self::AuthExpired(_) => "auth",
            Self::RateLimited(_) => "ratelimit",
            Self::Network(_) => "network",
            Self::SchemaChanged(_) => "schema",
            Self::Unavailable(_) => "setup",
        }
    }

    /// One line for the account card.
    pub fn problem(&self, provider: &str) -> String {
        let name = provider_name(provider);
        match self {
            Self::AuthExpired(s) | Self::Unavailable(s) => s.clone(),
            Self::RateLimited(Some(s)) => {
                format!("{name} asked Tokenrunway to slow down; trying again in {}.", forecast::duration(*s as i64 * 1000))
            }
            Self::RateLimited(None) => format!("{name} asked Tokenrunway to slow down; trying again shortly."),
            Self::Network(host) => format!("Couldn't reach {host} from this network."),
            Self::SchemaChanged(_) => format!("{name} changed the usage endpoint this reader uses. A Tokenrunway update will fix it."),
        }
    }

    /// Maps an HTTP status to an error (None for success).
    pub fn from_status(status: reqwest::StatusCode, retry_after: Option<u64>, host: &str, login_hint: &str) -> Option<Self> {
        match status.as_u16() {
            200..=299 => None,
            401 | 403 => Some(Self::AuthExpired(login_hint.into())),
            429 => Some(Self::RateLimited(retry_after)),
            500..=599 => Some(Self::Network(host.into())),
            s => Some(Self::SchemaChanged(format!("HTTP {s}"))),
        }
    }
}

/// The Retry-After header in seconds (only the delta-seconds form).
pub fn retry_after(h: &reqwest::header::HeaderMap) -> Option<u64> {
    h.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim().parse().ok()
}

pub fn network_error(host: &str, e: &reqwest::Error) -> ProviderError {
    if e.is_decode() {
        ProviderError::SchemaChanged(e.to_string())
    } else {
        ProviderError::Network(host.into())
    }
}

/// Money in a window, when it's a spend or a budget.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Money {
    pub used: f64,
    pub limit: Option<f64>,
}

/// One window as a provider read it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Reading {
    /// "5h", "weekly", "weekly-opus", "monthly", "budget"
    pub id: String,
    /// "5-hour", "Weekly", "Weekly Opus", "Codex 5-hour", "Monthly budget"
    pub label: String,
    /// Used, 0.0–1.0 (may pass 1.0)
    pub used: f64,
    pub abs: Option<String>,
    pub resets_at: Option<i64>,
    /// The window's fixed length, for the pace tick (5 h, 7 d, this calendar month)
    pub length_ms: Option<i64>,
    pub money: Option<Money>,
    /// When the provider's number is from (ms)
    pub at: i64,
}

impl Reading {
    pub fn snapshot(&self) -> WindowReading {
        WindowReading { window: self.id.clone(), used: self.used, limit: None, resets_at: self.resets_at, at: self.at }
    }
}

/// A successful reading of one account.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fetched {
    /// "Max", "Business", "On-demand"
    pub plan: Option<String>,
    /// The account's email or profile, if the source says
    pub who: Option<String>,
    pub windows: Vec<Reading>,
    /// Readings that came with a problem (e.g. Codex windows from local logs while the remote
    /// login has expired)
    pub caveat: Option<ProviderError>,
    /// A line for the card when there's nothing to draw a bar for ("Unlimited premium requests
    /// on this plan.", "xAI has no spend endpoint Tokenrunway can read yet.")
    pub note: Option<String>,
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// An allowance source for one kind of account (SPEC.md §7.1).
pub trait Provider: Send + Sync {
    /// "anthropic-oauth", "codex", "bedrock", "anthropic-admin", "openai-admin"
    fn id(&self) -> &'static str;
    fn fetch<'a>(&'a self, acct: &'a AccountRecord, ctx: &'a Ctx) -> BoxFuture<'a, Result<Fetched, ProviderError>>;
    /// The provider's own usage page, opened by `open_provider`.
    fn usage_url(&self, acct: &AccountRecord) -> String;
    /// The page that holds the provider's own "request more" flow, if it has one for this plan.
    fn request_url(&self, _acct: &AccountRecord) -> Option<String> {
        None
    }
    /// How often to poll at most, regardless of activity (Bedrock is cached and slow-moving).
    fn min_interval_ms(&self) -> i64 {
        60_000
    }
}

/// The provider for an account, or None for a provider/method this version can't read.
pub fn provider_for(acct: &AccountRecord) -> Option<Box<dyn Provider>> {
    match (acct.provider.as_str(), acct.method.as_str()) {
        ("anthropic", "cli") => Some(Box::new(claude::ClaudeCode)),
        ("openai", "cli") => Some(Box::new(codex::CodexCli)),
        ("bedrock", "aws") => Some(Box::new(bedrock::Bedrock)),
        ("anthropic-api", "key") => Some(Box::new(admin::AnthropicAdmin)),
        ("openai-api", "key") => Some(Box::new(admin::OpenaiAdmin)),
        ("copilot", "cli" | "key") => Some(Box::new(copilot::Copilot)),
        ("gemini", "cli") => Some(Box::new(gemini::GeminiCli)),
        ("cursor", "login" | "cli" | "key") => Some(Box::new(cursor::Cursor)),
        ("xai", "login") => Some(Box::new(grok::GrokWeb)),
        ("xai", "key") => Some(Box::new(grok::XaiApi)),
        ("openrouter", "key") => Some(Box::new(openrouter::OpenRouter)),
        _ => None,
    }
}

/// "Claude", "ChatGPT", "AWS Bedrock", "Anthropic API", "OpenAI API"
pub fn provider_name(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "Claude",
        "openai" => "ChatGPT",
        "bedrock" => "AWS Bedrock",
        "anthropic-api" => "Anthropic API",
        "openai-api" => "OpenAI API",
        "xai" => "Grok",
        "gemini" => "Gemini",
        "copilot" => "Copilot",
        "cursor" => "Cursor",
        "openrouter" => "OpenRouter",
        _ => "Provider",
    }
}

pub fn mono(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "C",
        "openai" => "O",
        "bedrock" => "B",
        "anthropic-api" | "openai-api" => "A",
        "xai" => "X",
        "gemini" => "G",
        "copilot" => "P",
        "cursor" => "Cu",
        "openrouter" => "R",
        _ => "?",
    }
}

/// Logins on this machine that could become accounts, minus the ones already added.
pub fn detect(home: &std::path::Path, existing: &[AccountRecord]) -> Vec<Detected> {
    let has = |provider: &str, method: &str| existing.iter().any(|a| a.provider == provider && a.method == method);
    let mut out = Vec::new();
    if !has("anthropic", "cli") {
        if let Some(d) = claude::detect(home) {
            out.push(d);
        }
    }
    if !has("openai", "cli") {
        if let Some(d) = codex::detect(home) {
            out.push(d);
        }
    }
    if !has("copilot", "cli") {
        if let Some(d) = copilot::detect(home) {
            out.push(d);
        }
    }
    if !has("gemini", "cli") {
        if let Some(d) = gemini::detect(home) {
            out.push(d);
        }
    }
    if !has("cursor", "cli") {
        if let Some(d) = cursor::detect(home) {
            out.push(d);
        }
    }
    let profiles: Vec<String> = bedrock::profiles(home)
        .into_iter()
        .filter(|p| !existing.iter().any(|a| a.provider == "bedrock" && a.profile.as_deref() == Some(p)))
        .collect();
    if !profiles.is_empty() {
        out.push(Detected {
            kind: "aws".into(),
            label: "AWS profiles".into(),
            detail: format!("{} in ~/.aws", profiles.len()),
            provider: "bedrock".into(),
            mono: "B".into(),
        });
    }
    out
}

/// What the app knows about one account between readings.
#[derive(Debug, Clone, Default)]
pub struct Live {
    /// The last good reading
    pub last: Option<Fetched>,
    pub updated_at: Option<i64>,
    /// The last attempt's error, if it failed
    pub error: Option<ProviderError>,
    /// True once a reading (good or bad) has been attempted
    pub tried: bool,
}

/// The card for one account: its windows with forecasts, and its state.
pub fn account_view(
    acct: &AccountRecord,
    live: &Live,
    history: &dyn Fn(&str) -> Vec<WindowReading>,
    th: &Thresholds,
    now: i64,
) -> AccountView {
    let problem_of = |e: &ProviderError| e.problem(&acct.provider);
    let (state, problem, cause) = match (&live.error, live.last.as_ref().and_then(|f| f.caveat.as_ref())) {
        (Some(e), _) => (e.state(), Some(problem_of(e)), Some(e.cause().to_string())),
        (None, Some(c)) => (c.state(), Some(problem_of(c)), Some(c.cause().to_string())),
        (None, None) if !live.tried => (AccountState::Loading, None, None),
        (None, None) => (AccountState::Ok, None, None),
    };
    let stale = matches!(state, AccountState::Stale);
    let windows: Vec<WindowView> = live
        .last
        .as_ref()
        .map(|f| {
            f.windows
                .iter()
                .map(|r| {
                    let input = WindowInput {
                        id: &r.id,
                        label: &r.label,
                        abs: r.abs.clone(),
                        used: r.used,
                        resets_at: r.resets_at,
                        length_ms: r.length_ms,
                        money: r.money,
                    };
                    let mut w = forecast::window_view(&input, &history(&r.id), now, th);
                    if stale {
                        // Forecasts are paused until a fresh reading (States.dc.html).
                        w.proj = None;
                        w.out_at = None;
                    }
                    w
                })
                .collect()
        })
        .unwrap_or_default();
    let plan = live.last.as_ref().and_then(|f| f.plan.clone()).filter(|p| !p.is_empty()).unwrap_or_else(|| acct.plan.clone());
    AccountView {
        id: acct.id.clone(),
        provider: acct.provider.clone(),
        name: acct.label.clone(),
        plan,
        mono: mono(&acct.provider).into(),
        updated_at: live.updated_at,
        state,
        problem,
        cause,
        note: live.last.as_ref().and_then(|f| f.note.clone()),
        method: acct.method.clone(),
        windows,
    }
}

/// Sorts cards most urgent first: a critical window, then a problem, then warnings, then the
/// highest projection.
pub fn sort_urgent(accounts: &mut [AccountView]) {
    fn key(a: &AccountView) -> (u8, i64) {
        let worst = a.windows.iter().map(|w| forecast::severity(w.status)).max().unwrap_or(0);
        let problem = !matches!(a.state, AccountState::Ok | AccountState::Loading);
        let rank = match (worst, problem) {
            (3, _) => 4,
            (_, true) => 3,
            (2, _) => 2,
            _ => 1,
        };
        let proj = a.windows.iter().map(|w| w.proj.unwrap_or(w.pct)).fold(0.0, f64::max);
        (rank, (proj * 100.0) as i64)
    }
    accounts.sort_by_key(|a| std::cmp::Reverse(key(a)));
}

/// Decodes base64 or base64url (padding optional): JWT claims, nothing secret is verified.
pub fn b64decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => return None,
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The claims of a JWT, decoded but not verified (for display only: plan, email, expiry).
pub fn jwt_claims(token: &str) -> Option<serde_json::Value> {
    let part = token.split('.').nth(1)?;
    serde_json::from_slice(&b64decode(part)?).ok()
}

/// "max" → "Max", "business" → "Business"
pub fn title_case(s: &str) -> String {
    s.split(['_', '-', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// "$184", "$1,240", "$12.40"
pub fn usd(v: f64) -> String {
    if v.abs() >= 100.0 {
        let n = v.round() as i64;
        let s = n.abs().to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        format!("{}${out}", if n < 0 { "-" } else { "" })
    } else {
        format!("${v:.2}").replace(".00", "")
    }
}

/// Finds a program on PATH or in the usual install places (a GUI app on macOS starts with a
/// minimal PATH, so Homebrew's bin isn't on it).
pub fn find_program(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if cfg!(unix) {
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/snap/bin"].map(PathBuf::from));
        if let Some(h) = dirs::home_dir() {
            dirs.push(h.join(".local/bin"));
        }
    }
    if cfg!(windows) {
        dirs.push(PathBuf::from(r"C:\Program Files\Amazon\AWSCLIV2"));
    }
    dirs.into_iter().map(|d| d.join(&exe)).find(|p| p.is_file())
}

/// Start of the current calendar month and of the next one, local time (ms).
pub fn month_bounds(now: i64) -> (i64, i64) {
    use chrono::{Datelike, Local, TimeZone};
    let t = Local.timestamp_millis_opt(now).single().unwrap_or_else(Local::now);
    let start = Local.with_ymd_and_hms(t.year(), t.month(), 1, 0, 0, 0).earliest();
    let (ny, nm) = if t.month() == 12 { (t.year() + 1, 1) } else { (t.year(), t.month() + 1) };
    let next = Local.with_ymd_and_hms(ny, nm, 1, 0, 0, 0).earliest();
    match (start, next) {
        (Some(s), Some(n)) => (s.timestamp_millis(), n.timestamp_millis()),
        _ => (now - 15 * DAY_MS, now + 15 * DAY_MS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_jwt() {
        assert_eq!(b64decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(b64decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(b64decode("-_8").unwrap(), vec![0xfb, 0xff]);
        let claims = r#"{"exp":5,"email":"a@b"}"#;
        let enc: String = {
            // Encode by hand for the test.
            const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let b = claims.as_bytes();
            let mut s = String::new();
            for ch in b.chunks(3) {
                let n = ch.iter().enumerate().fold(0u32, |acc, (i, &x)| acc | (x as u32) << (16 - 8 * i));
                for i in 0..=ch.len() {
                    s.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
                }
            }
            s
        };
        let v = jwt_claims(&format!("x.{enc}.y")).unwrap();
        assert_eq!(v["exp"], 5);
        assert_eq!(v["email"], "a@b");
    }

    #[test]
    fn words() {
        assert_eq!(title_case("max"), "Max");
        assert_eq!(title_case("claude_max_20x"), "Claude Max 20x");
        assert_eq!(usd(184.0), "$184");
        assert_eq!(usd(1240.4), "$1,240");
        assert_eq!(usd(12.4), "$12.40");
        assert_eq!(usd(3.0), "$3");
    }

    #[test]
    fn errors_map_to_states() {
        assert_eq!(ProviderError::AuthExpired("x".into()).state(), AccountState::Login);
        assert_eq!(ProviderError::RateLimited(None).state(), AccountState::Stale);
        assert_eq!(ProviderError::Network("claude.ai".into()).state(), AccountState::Stale);
        assert_eq!(ProviderError::SchemaChanged("x".into()).state(), AccountState::Broken);
        assert_eq!(ProviderError::Network("claude.ai".into()).problem("anthropic"), "Couldn't reach claude.ai from this network.");
        assert_eq!(ProviderError::RateLimited(Some(120)).problem("openai"), "ChatGPT asked Tokenrunway to slow down; trying again in 2m.");
        assert_eq!(ProviderError::Unavailable("x".into()).cause(), "setup");
        assert_eq!(ProviderError::SchemaChanged("x".into()).cause(), "schema");
    }

    #[test]
    fn new_providers_are_wired() {
        let rec = |provider: &str, method: &str| AccountRecord { provider: provider.into(), method: method.into(), ..Default::default() };
        for (p, m, id) in [
            ("copilot", "cli", "copilot"),
            ("copilot", "key", "copilot"),
            ("gemini", "cli", "gemini-cli"),
            ("cursor", "login", "cursor"),
            ("cursor", "cli", "cursor"),
            ("cursor", "key", "cursor"),
            ("xai", "login", "grok-web"),
            ("xai", "key", "xai-api"),
            ("openrouter", "key", "openrouter"),
        ] {
            assert_eq!(provider_for(&rec(p, m)).map(|x| x.id()), Some(id), "{p}/{m}");
        }
        assert!(provider_for(&rec("gemini", "key")).is_none());
        assert!(provider_for(&rec("openrouter", "cli")).is_none());
        assert_eq!((provider_name("openrouter"), mono("openrouter")), ("OpenRouter", "R"));
        assert_eq!((mono("copilot"), mono("gemini"), mono("cursor"), mono("xai")), ("P", "G", "Cu", "X"));
    }

    #[test]
    fn month_bounds_contain_now() {
        let now = crate::now_ms();
        let (s, n) = month_bounds(now);
        assert!(s <= now && now < n);
        assert!((28 * DAY_MS - HOUR_MS..=31 * DAY_MS + HOUR_MS).contains(&(n - s)));
    }
}
