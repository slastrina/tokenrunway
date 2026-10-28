//! The view model: exactly what the UI (src/*.js) receives, as JSON (camelCase). This file is the
//! contract between the Rust side and the UI; docs/design/*.dc.html is what it's drawn as.
//!
//! Times are ms since the epoch and the UI formats them ("resets in 2h 41m · 14:20"), so
//! countdowns tick without a round trip. Money is USD as f64. Percentages are 0–100 (and may pass
//! 100 with extra usage).

use serde::{Deserialize, Serialize};

/// Status colour + icon + words (docs/design/Main.dc.html): ok = check circle, warn = triangle,
/// crit = octagon, est = dashed circle (not enough history; neutral).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Ok,
    Warn,
    Crit,
    Est,
}

/// Everything the popover's Now tab, the footer and the tray need; pushed as the `tr://state`
/// event whenever it changes, and returned by the `get_state` command.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppView {
    pub version: String,
    /// "mac" | "windows" | "linux": the UI picks the macOS popover or the Windows flyout look
    pub os: String,
    /// The newer version on offer, if any
    pub update: Option<String>,
    /// "install" | "homebrew" | "download"
    pub update_how: String,
    /// True until every account has had its first reading (the "Reading 3 accounts…" state)
    pub loading: bool,
    /// The "Shortest runway" header; None when there are no windows
    pub summary: Option<Summary>,
    /// Most urgent first
    pub accounts: Vec<AccountView>,
    /// Logins found on this machine that aren't accounts yet (empty state's "Found on this Mac")
    pub detected: Vec<Detected>,
    pub settings: Settings,
    /// Rattleboard: the last thing that happened ("Sent an alert at 14:02", or what went wrong)
    pub rattleboard_status: Option<String>,
    /// Rattleboard: its key is in the keychain (the card needs it)
    pub rattleboard_key: bool,
    /// API-equivalent spend since local midnight from the agents' logs; None until they're read
    pub spend_today: Option<f64>,
    /// The same since the first of the month (the monthly budget goal)
    pub spend_month: Option<f64>,
    /// Sessions spending much faster than usual right now (live.rs)
    pub hot: Vec<HotSession>,
    /// "Switch to save": where there's room when a window is tight (allowance/tips.rs)
    pub tips: Vec<Tip>,
    /// LM Studio on this computer, while its server answers or it ran today (local.rs)
    pub local: Option<LocalServer>,
}

/// A local model server: the Now tab's LM Studio card.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalServer {
    /// "LM Studio"
    pub name: String,
    pub running: bool,
    pub port: u16,
    /// Every downloaded model it reported, loaded first
    pub models: Vec<LocalModel>,
    /// False when the server only says which models exist, not which are loaded
    pub knows_loaded: bool,
    /// Since local midnight, from its logs
    pub today_requests: u32,
    pub today_tokens: u64,
    /// The last request in its logs (ms)
    pub last_used: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalModel {
    pub id: String,
    /// "llm" | "vlm" | "embeddings"
    pub kind: String,
    pub loaded: bool,
    /// "Q4_K_S"
    pub quant: Option<String>,
    /// The loaded context length, else the model's maximum
    pub context: Option<u64>,
}

/// A session burning much faster than usual (the burn-rate warning).
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HotSession {
    /// "Claude Code"
    pub harness: String,
    pub session_id: String,
    pub project: String,
    /// API-equivalent spend in the last `minutes`
    pub cost: f64,
    pub minutes: u32,
    /// Times the usual 20-minute spend; 0 when there's no history to compare with yet
    pub ratio: f64,
    /// The session's first event (ms)
    pub started: i64,
}

/// A suggestion to move work where there's room.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tip {
    /// The tight window it's about
    pub account_id: String,
    pub window_id: String,
    /// "Codex weekly is at 12%: move your next task there."
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// "1h 26m": runway left on the tightest window, or time to its reset if all are on track
    pub label: String,
    pub status: Status,
    /// "Claude — work · 5-hour"
    pub window: String,
    pub crit: u32,
    pub warn: u32,
    pub ok: u32,
}

/// Account health, for the problem states in docs/design/States.dc.html.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AccountState {
    #[default]
    Ok,
    /// No reading yet
    Loading,
    /// The last reading is old (network); `problem` says why
    Stale,
    /// Login expired or revoked; `problem` says how to fix it
    Login,
    /// The provider changed something this reader relies on
    Broken,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: String,
    /// "anthropic" | "openai" | "bedrock" | "anthropic-api" | "openai-api" | "xai" | "gemini" | "copilot" | "cursor" | "openrouter"
    /// The UI maps it to the provider colour (--p-claude, --p-openai, --p-bedrock, --p-api…)
    pub provider: String,
    /// "Claude — work"
    pub name: String,
    /// "Enterprise", "Pro", "On-demand"; "" if unknown
    pub plan: String,
    /// Monogram for the provider tile: "C", "O", "B", "A", "X", "G", "P", "Cu"
    pub mono: String,
    /// When the last good reading was taken
    pub updated_at: Option<i64>,
    pub state: AccountState,
    /// One line for stale/login/broken: "Couldn't reach claude.ai from this network."
    pub problem: Option<String>,
    /// What kind of problem, when there is one: "auth" | "ratelimit" | "network" | "schema" (a
    /// Tokenrunway update fixes it) | "setup" (the fix is on the user's side, e.g. an ended
    /// subscription or a missing CLI)
    pub cause: Option<String>,
    /// A line shown when the account has nothing to draw a bar for ("Unlimited premium requests
    /// on this plan.")
    pub note: Option<String>,
    /// "How": "cli" | "login" | "key" | "aws"
    pub method: String,
    pub windows: Vec<WindowView>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WindowView {
    /// Stable within the account: "5h", "weekly", "weekly-opus", "monthly", "budget"
    pub id: String,
    /// "5-hour", "Weekly", "Weekly Opus", "Codex 5-hour", "Monthly budget"
    pub label: String,
    /// "412k / 530k tokens", "$184 / $500", or None
    pub abs: Option<String>,
    /// Used, 0–100
    pub pct: f64,
    /// Share of the window elapsed, 0–100 (the pace tick); None if the window has no fixed length
    pub pace: Option<f64>,
    /// Projected % at reset at the recent pace (the hatched part runs from pct to proj)
    pub proj: Option<f64>,
    pub resets_at: Option<i64>,
    pub status: Status,
    /// Forecast words: "On track", "Tight · ~97% at reset", "Out at 13:05, 1h 15m before reset",
    /// "Rough estimate · ~$255"
    pub forecast: String,
    /// When it's projected to run out, if before the reset
    pub out_at: Option<i64>,
    /// When this cycle started (reset time minus the window's length); None without a fixed length
    pub cycle_start: Option<i64>,
    /// Filled in by the app: API-equivalent work this cycle by the account's own agent (Claude Code
    /// for a Claude login), from the local logs
    pub cycle_spend: Option<f64>,
    /// Dollars left: limit minus spent for a money window; for a seat window, filled in by the app
    /// as an estimate from this cycle's work (`left_estimate`)
    pub left_usd: Option<f64>,
    pub left_estimate: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    /// "claude-code" | "codex" | "aws" | "gh" | "gemini-cli" | "cursor-app"
    pub kind: String,
    /// "Claude Code login", "Codex CLI login", "AWS profiles", "GitHub CLI login", "Gemini CLI
    /// login", "Cursor app login"
    pub label: String,
    /// "2 in ~/.aws", or the account email if known
    pub detail: String,
    /// Provider for the tile colour and the add_account call
    pub provider: String,
    pub mono: String,
}

/// User settings the UI shows and changes (`set_settings` takes a partial object).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Tray label: "none" | "percent" (% left) | "left" ($ left) | "runway" | "spend" (today's spend so far)
    pub menubar_label: String,
    /// What the label follows: a window "account-id/window-id", one account's lowest window
    /// "account-id/", or "" for the lowest of all (auto)
    pub menubar_window: String,
    /// Spend goals, API-equivalent dollars; 0 = off. Notified at 80% and 100%.
    pub budget_daily: f64,
    pub budget_monthly: f64,
    pub notify_budget: bool,
    /// Warn about a session spending much faster than usual
    pub notify_burn: bool,
    /// Remind before a weekly or monthly seat window resets with a lot left (use it or lose it)
    pub notify_unused: bool,
    /// "comfortable" | "compact"
    pub density: String,
    /// Warn at this % used (default 75), critical at (default 90)
    pub warn_at: f64,
    pub crit_at: f64,
    /// Notify when projected to run out within this many minutes before reset (default 60)
    pub lead_minutes: u32,
    pub notify_thresholds: bool,
    pub notify_projection: bool,
    pub notify_reset: bool,
    /// Notifications paused until (ms), 0 = not paused, i64::MAX = until resumed
    pub paused_until: i64,
    /// Prompt to request more at this % (default 85)
    pub request_at: f64,
    /// Admin contact per account id: { "acct-1": { "name": "Jordan", "email": "…", "slack": "…" } }
    pub admins: std::collections::BTreeMap<String, AdminContact>,
    /// The name signed under request messages ("Sam")
    pub my_name: String,
    pub update_check: bool,
    pub start_at_login: bool,
    /// Alerts and the AI runway card on a Rattleboard (SPEC.md §8.4); its key is in the keychain
    pub rattleboard: RattleboardSettings,
    /// The weekly recap and plan prices (insights.rs)
    pub insights: InsightSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RattleboardSettings {
    /// "off" | "local" (the Rattleboard app on this computer) | "network" (a board's API)
    pub mode: String,
    /// The board's API ("http://192.168.1.20:7070"); for "local", this computer's server
    pub url: String,
    /// "Living room TV"
    pub name: String,
    pub rows: u32,
    pub cols: u32,
    /// "rattleboard" | "vestaboard" (a real one: alerts only, no card), "" before connecting
    pub kind: String,
    /// "local" only: a saved board or group in Rattleboard for `--notify --target`
    pub target: String,
    /// "off" | "alerts" (critical, run-out, login) | "warnings" (also warnings) | "all" (also resets)
    pub alerts: String,
    /// The AI runway card in the board's rotation (needs the key)
    pub card: bool,
    /// Use-it-or-lose-it reminders go to the board too (when they're on and alerts aren't "off")
    pub unused: bool,
}

impl Default for RattleboardSettings {
    fn default() -> Self {
        Self {
            mode: "off".into(),
            url: String::new(),
            name: String::new(),
            rows: 6,
            cols: 22,
            kind: String::new(),
            target: String::new(),
            alerts: "alerts".into(),
            card: false,
            unused: true,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            menubar_label: "runway".into(),
            menubar_window: String::new(),
            budget_daily: 0.0,
            budget_monthly: 0.0,
            notify_budget: true,
            notify_burn: true,
            notify_unused: false,
            density: "comfortable".into(),
            warn_at: 75.0,
            crit_at: 90.0,
            lead_minutes: 60,
            notify_thresholds: true,
            notify_projection: true,
            notify_reset: false,
            paused_until: 0,
            request_at: 85.0,
            admins: Default::default(),
            my_name: String::new(),
            update_check: true,
            start_at_login: false,
            rattleboard: RattleboardSettings::default(),
            insights: InsightSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AdminContact {
    pub name: String,
    pub email: String,
    /// Slack user/channel deep link or handle
    pub slack: String,
    /// Teams chat deep link or email
    pub teams: String,
}

// ---------- Stats tab (docs/design/Stats.dc.html): `get_stats { period }` ----------

/// "today" | "7d" | "30d" | "90d" | "lifetime"
pub type Period = String;

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatsView {
    pub period: Period,
    /// API-equivalent cost of everything in the period
    pub spend_api: f64,
    /// Actually billed, where a source reports it (Bedrock, opencode, Cline); None if none do
    pub spend_actual: Option<f64>,
    /// "Bedrock billed"
    pub actual_note: Option<String>,
    pub tokens: TokenTotals,
    /// One entry per day in the period (local dates, oldest first; lifetime: last 30 days)
    pub daily: Vec<DayCost>,
    /// Top 4, biggest first (w = % of the biggest, for the bar)
    pub models: Vec<Ranked>,
    /// Local models (LM Studio), top 4 by tokens; empty when none ran in the period
    pub local_models: Vec<Ranked>,
    pub projects: Vec<Ranked>,
    /// All ten categories, biggest first, share of tokens 0–100
    pub kinds: Vec<KindShare>,
    /// Cloud models in the period that the price table doesn't know yet (counted at $0)
    pub unpriced: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotals {
    pub total: u64,
    /// cache read + cache write
    pub cache: u64,
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DayCost {
    /// "2026-10-23"
    pub day: String,
    pub cost: f64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ranked {
    /// "Opus 5.5", "tokenrunway"
    pub name: String,
    pub cost: f64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KindShare {
    /// "Build", "Bug fix", "Discovery", "Refactor", "Test", "Review", "Docs", "Ops", "Planning", "Chat"
    pub kind: String,
    pub share: f64,
}

// ---------- Harness tab (docs/design/Harness.dc.html): `get_harness { period }` ----------

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HarnessView {
    pub period: Period,
    pub total_cost: f64,
    /// Biggest share first
    pub harnesses: Vec<HarnessRow>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HarnessRow {
    /// "claude-code" | "codex" | "opencode" | "gemini-cli" | "cline"
    pub id: String,
    /// "Claude Code"
    pub name: String,
    /// Share of spend 0–100
    pub share: f64,
    pub cost: f64,
    pub tokens: u64,
    pub sessions: u32,
    pub avg_session_ms: i64,
    /// cache read / (cache read + input), 0–100
    pub cache_hit: f64,
    pub top_model: String,
    pub top_kind: String,
    /// Cost per day for the last 7 days, oldest first
    pub daily: Vec<f64>,
}

// ---------- Dashboard window (docs/design/Dashboard.dc.html): `get_dashboard { period, group, account }` ----------

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DashboardView {
    pub period: Period,
    /// "model" | "harness" | "category"
    pub group: String,
    /// Stacked daily spend: days oldest first, and one series per group (top 5 + "Other")
    pub days: Vec<String>,
    pub series: Vec<Series>,
    /// Tokens per day for the last 26 weeks (182 days, oldest first), for the heatmap
    pub heatmap: Vec<DayCost>,
    /// Newest first, at most 200
    pub sessions: Vec<SessionRow>,
    pub sessions_total: u32,
    /// Filled in by the app from allowance data (not by stats): upcoming resets, soonest first
    pub resets: Vec<ResetRow>,
    /// Filled in by the app: burn-down of the most urgent window
    pub burndown: Option<Burndown>,
    /// Filled in by the app: actually billed in the period, where a source reports it
    pub billed: Option<f64>,
    pub billed_note: Option<String>,
    /// Filled in by the app: cache reads as a share of all tokens in the period, 0–100
    pub cache_share: Option<f64>,
    /// Filled in by the app: how many window instances hit 100% in the period (from the readings
    /// kept since Tokenrunway started tracking)
    pub ran_out: Option<u32>,
    pub ran_out_note: Option<String>,
    /// Filled in by the app with an account filter: whose work the activity parts show
    /// ("Claude Code", "Claude Code · shared by 2 Claude accounts", "No agent logs for this account")
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub name: String,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub started: i64,
    pub harness: String,
    pub project: String,
    pub model: String,
    pub kind: String,
    pub duration_ms: i64,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResetRow {
    pub account: String,
    pub provider: String,
    pub window: String,
    pub resets_at: i64,
    pub pct: f64,
    pub status: Status,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Burndown {
    /// "Claude — work · 5-hour"
    pub title: String,
    /// Window start and reset (ms)
    pub start: i64,
    pub reset: i64,
    /// (time ms, % used) readings so far
    pub points: Vec<(i64, f64)>,
    pub out_at: Option<i64>,
    /// Earliest and latest projected run-out (the cone)
    pub cone: Option<(i64, i64)>,
}

// ---------- Request more (docs/design/RequestMore.dc.html): `request_draft { accountId, windowId }` ----------

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RequestDraft {
    pub account_id: String,
    /// "Claude — work · 5-hour at 78%"
    pub title: String,
    /// "Out at 13:05, 1h 15m before reset"
    pub forecast: String,
    /// Channels offered, first is the default: "provider" (only when it has a request flow),
    /// "email", "slack", "teams"
    pub channels: Vec<String>,
    /// Admin contact shown in "To" (editable in the UI)
    pub to: AdminContact,
    /// The message, plain text with "• " bullets
    pub body: String,
}

// ---------- Insights (insights.rs; SPEC.md §10.5): `get_insights { period, project }` ----------

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InsightsView {
    pub period: Period,
    /// Biggest first: at most 12, or just `project` when one was asked for
    pub projects: Vec<ProjectRow>,
    pub projects_total: u32,
    /// Projects whose prompt cache works far worse than usual, biggest cost first (at most 3)
    pub tips: Vec<CacheTip>,
    /// Cache hit rate across everything in the period, 0–100; None when nothing was cacheable
    pub usual_cache_hit: Option<f64>,
    /// The last full week (Monday to Sunday, local)
    pub recap: Option<Recap>,
    /// This calendar month's API-equivalent work per subscription
    pub value: Vec<PlanValue>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRow {
    pub name: String,
    pub cost: f64,
    pub tokens: u64,
    pub sessions: u32,
    /// cache read / (input + cache read + cache write), 0–100; None when nothing was cacheable
    pub cache_hit: Option<f64>,
    /// Its newest event (ms)
    pub last: i64,
    /// Biggest first: the top 3, or all of them for the project asked for
    pub branches: Vec<BranchRow>,
    pub branches_total: u32,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BranchRow {
    /// "feat/insights"; "" where the harness didn't record one
    pub name: String,
    pub cost: f64,
    pub tokens: u64,
    pub sessions: u32,
    /// Its newest event (ms)
    pub last: i64,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CacheTip {
    pub project: String,
    /// Its hit rate and the usual one, 0–100
    pub hit: f64,
    pub usual: f64,
    pub cost: f64,
    /// "Sessions in third-eye get 12% cache hits vs your usual 70%"
    pub title: String,
    /// What the logs show about why, and what to do
    pub detail: String,
    /// "idle" | "model-switch" | "short-sessions" | "" (no cause found)
    pub cause: String,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Recap {
    /// Monday and Sunday, "2026-10-12"
    pub from: String,
    pub to: String,
    pub spend: f64,
    pub tokens: u64,
    pub sessions: u32,
    /// The week before
    pub prev_spend: f64,
    /// Change from the week before, %; None when that week had nothing
    pub change_pct: Option<f64>,
    pub top_project: Option<Ranked>,
    pub top_model: Option<Ranked>,
    pub biggest: Option<SessionRow>,
    /// The notification: "Last week: $412 of AI work" and its body
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanValue {
    /// "anthropic" | "openai" | "gemini"
    pub provider: String,
    /// The subscription accounts it covers (the logs don't say which account a session used)
    pub accounts: Vec<String>,
    /// "Max 20x", or "2 Claude plans"
    pub plan: String,
    /// Monthly price in USD: built in, or set in Settings; None when unknown
    pub price: Option<f64>,
    /// API-equivalent spend this calendar month from the harnesses that use it
    pub spend: f64,
    /// spend / price
    pub multiple: Option<f64>,
    /// "Claude Code"
    pub harnesses: Vec<String>,
    /// "Your $200 Max 20x plan did $3,199 of API-equivalent work this month (16×)"
    pub line: String,
}

/// Settings → Notifications → Weekly recap, and the plan prices for subscription value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct InsightSettings {
    pub weekly_recap: bool,
    /// Monthly price (USD) per account id, for plans the built-in table doesn't know (Team,
    /// Enterprise, Business) or that cost something else
    pub plan_prices: std::collections::BTreeMap<String, f64>,
}

impl Default for InsightSettings {
    fn default() -> Self {
        Self { weekly_recap: true, plan_prices: Default::default() }
    }
}
