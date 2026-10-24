//! The shared vocabulary (SPEC.md §4): accounts with providers, the windows that limit them, and
//! the harnesses whose local logs say what the tokens went on.

use serde::{Deserialize, Serialize};

/// Who the account is with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    Anthropic,
    Openai,
    Bedrock,
    Xai,
    Google,
    Copilot,
    Cursor,
    Openrouter,
}

/// The tool that ran the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    ClaudeCode,
    Codex,
    Opencode,
    GeminiCli,
    Cline,
    Cursor,
    /// LM Studio's local server (its server logs)
    Lmstudio,
    Web,
    Other,
}

/// What the work was (SPEC.md §10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Build,
    Bugfix,
    Discovery,
    Refactor,
    Test,
    Review,
    Docs,
    Ops,
    Planning,
    Chat,
}

/// One login with a provider. Its secrets live in the OS keychain, never here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub provider: Provider,
    /// "Claude — work"
    pub label: String,
    /// "Enterprise", "Max", "Plus", "API"…, or "" until the provider says
    #[serde(default)]
    pub plan: String,
}

/// One reading of a limit that resets: a 5-hour window, a week, a month's budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowReading {
    /// "5h", "weekly", "weekly-opus", "monthly", "credits", "budget"
    pub window: String,
    /// How much of it is used, 0.0–1.0 (may pass 1.0 with extra usage)
    pub used: f64,
    /// The absolute limit and its unit, when the provider gives one: (1_200_000.0, "tokens"), (500.0, "usd")
    #[serde(default)]
    pub limit: Option<(f64, String)>,
    /// When it resets, ms since the epoch, if it does
    #[serde(default)]
    pub resets_at: Option<i64>,
    /// When this was read, ms since the epoch
    pub at: i64,
}
