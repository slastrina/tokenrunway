//! API-equivalent cost (SPEC.md §10.1): what a call would have cost at the provider's list price,
//! whatever plan actually paid for it. The table is a bundled snapshot (pricing.json, USD per
//! million tokens); a refreshed copy from the latest release can replace it later.
//!
//! Model ids arrive in many spellings for the same model ("claude-opus-4-5-20251101",
//! "us.anthropic.claude-opus-4-5-20251101-v1:0", "claude-opus-4-5@20251101",
//! "anthropic/claude-opus-4.5"), so `normalise` folds them to one key before the lookup. Lookups
//! are exact after that: a prefix match would price a new "claude-opus-4-9" as Opus 4, which is
//! worse than admitting we don't know it.

use crate::ingest::Tokens;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

const BUNDLED: &str = include_str!("pricing.json");

/// Prices for one model, USD per million tokens.
#[derive(Debug, Clone, Deserialize)]
pub struct Price {
    pub name: String,
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write_5m: Option<f64>,
    pub cache_write_1h: Option<f64>,
    /// Multiplier for fast mode (usage.speed == "fast")
    pub fast: Option<f64>,
    pub long_context: Option<LongContext>,
}

/// The higher tier some models charge once the prompt passes `above` tokens.
#[derive(Debug, Clone, Deserialize)]
pub struct LongContext {
    pub above: u64,
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
}

#[derive(Deserialize)]
struct Table {
    models: HashMap<String, Price>,
    #[serde(default)]
    aliases: HashMap<String, String>,
}

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| serde_json::from_str(BUNDLED).expect("bundled pricing.json is valid"))
}

/// Changes whenever the bundled table does, so the store knows to re-price what it already has.
pub fn version() -> String {
    // FNV-1a: stable across runs and Rust versions, unlike DefaultHasher.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in BUNDLED.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Folds the spellings of one model id into the table's key: lower case, no provider path or
/// Bedrock region prefix, no version/date suffix, and dots as dashes in Claude ids.
pub fn normalise(model: &str) -> String {
    let mut s = model.trim().to_ascii_lowercase();
    if let Some(i) = s.rfind('/') {
        s = s[i + 1..].to_string();
    }
    if let Some(i) = s.find('[') {
        s.truncate(i); // "claude-opus-4-8[1m]"
    }
    if let Some(i) = s.find('@') {
        s.truncate(i); // Vertex: "claude-opus-4-5@20251101"
    }
    for p in ["us-gov.", "us.", "eu.", "apac.", "au.", "jp.", "ca.", "global."] {
        if let Some(rest) = s.strip_prefix(p) {
            s = rest.to_string();
            break;
        }
    }
    if let Some(rest) = s.strip_prefix("anthropic.") {
        s = rest.to_string();
    }
    // Bedrock: "-v1:0", "-v2"
    if let Some(i) = s.rfind("-v") {
        let tail = &s[i + 2..];
        if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit() || b == b':') {
            s.truncate(i);
        }
    }
    s = strip_date(&s).to_string();
    if s.starts_with("claude") {
        s = s.replace('.', "-");
    }
    s
}

/// Drops a trailing "-20251001" or "-2025-08-07".
fn strip_date(s: &str) -> &str {
    let b = s.as_bytes();
    let digits = |r: &[u8]| r.iter().all(|c| c.is_ascii_digit());
    if b.len() > 9 && b[b.len() - 9] == b'-' && digits(&b[b.len() - 8..]) {
        return &s[..s.len() - 9];
    }
    if b.len() > 11 {
        let t = &b[b.len() - 11..];
        if t[0] == b'-' && digits(&t[1..5]) && t[5] == b'-' && digits(&t[6..8]) && t[8] == b'-' && digits(&t[9..]) {
            return &s[..s.len() - 11];
        }
    }
    s
}

/// The price for a model id, or None if the table doesn't know it.
pub fn lookup(model: &str) -> Option<&'static Price> {
    let t = table();
    let key = normalise(model);
    t.models.get(&key).or_else(|| t.aliases.get(&key).and_then(|k| t.models.get(k)))
}

/// "Opus 5.5" for "claude-opus-5-5-20260101"; for models the table doesn't know, the id as the
/// harness wrote it, without a provider path ("lmstudio/Qwen3-30B" → "Qwen3-30B").
pub fn display_name(model: &str) -> String {
    lookup(model).map(|p| p.name.clone()).unwrap_or_else(|| model.rsplit('/').next().unwrap_or(model).trim().to_string())
}

/// API-equivalent USD for one call. Reasoning is already inside `output` and is billed as output.
/// Unknown models cost 0 (the caller reports them).
pub fn cost(model: &str, t: &Tokens, fast: bool) -> f64 {
    let Some(p) = lookup(model) else { return 0.0 };
    let (mut input, mut output, mut read) = (p.input, p.output, p.cache_read.unwrap_or(p.input * 0.1));
    let (mut w5, mut w1) = (p.cache_write_5m.unwrap_or(p.input * 1.25), p.cache_write_1h.unwrap_or(p.input * 2.0));
    if let Some(lc) = &p.long_context {
        if t.input + t.cache_read + t.cache_write_5m + t.cache_write_1h > lc.above {
            // Cache prices keep their ratio to input in the higher tier.
            let k = lc.input / p.input;
            input = lc.input;
            output = lc.output;
            read = lc.cache_read.unwrap_or(read * k);
            w5 *= k;
            w1 *= k;
        }
    }
    let usd = (t.input as f64 * input
        + t.output as f64 * output
        + t.cache_read as f64 * read
        + t.cache_write_5m as f64 * w5
        + t.cache_write_1h as f64 * w1)
        / 1e6;
    if fast {
        usd * p.fast.unwrap_or(1.0)
    } else {
        usd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(input: u64, output: u64, read: u64, w5: u64, w1: u64) -> Tokens {
        Tokens { input, output, cache_read: read, cache_write_5m: w5, cache_write_1h: w1, reasoning: 0 }
    }

    #[test]
    fn normalises_spellings() {
        for id in [
            "claude-opus-4-5-20251101",
            "us.anthropic.claude-opus-4-5-20251101-v1:0",
            "global.anthropic.claude-opus-4-5-20251101-v1:0",
            "claude-opus-4-5@20251101",
            "anthropic/claude-opus-4.5",
            "Claude-Opus-4-5",
        ] {
            assert_eq!(normalise(id), "claude-opus-4-5", "{id}");
        }
        assert_eq!(normalise("claude-opus-4-8[1m]"), "claude-opus-4-8");
        assert_eq!(normalise("anthropic.claude-opus-5-5"), "claude-opus-5-5");
        assert_eq!(normalise("gpt-5-2025-08-07"), "gpt-5");
        assert_eq!(normalise("openai/gpt-5.1-codex"), "gpt-5.1-codex");
        assert_eq!(normalise("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
    }

    #[test]
    fn looks_up_aliases_and_names() {
        assert_eq!(display_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(display_name("claude-sonnet-4-20250514"), "Sonnet 4");
        assert_eq!(display_name("gpt-5-codex"), "GPT-5 Codex");
        assert_eq!(display_name("big-pickle"), "big-pickle");
        assert_eq!(display_name("qwen/Qwen3-Coder-30B"), "Qwen3-Coder-30B");
        assert!(lookup("claude-opus-4-9").is_none(), "no prefix guessing");
    }

    #[test]
    fn prices_every_token_kind() {
        // Opus 5.5: $4 in, $20 out, $0.20 read, 1.25x / 2x writes.
        let c = cost("claude-opus-5-5", &toks(1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000), false);
        assert!((c - (4.0 + 20.0 + 0.2 + 5.0 + 8.0)).abs() < 1e-9, "{c}");
        // Codex: cached input at the cached price, no writes.
        let c = cost("gpt-5-codex", &toks(1_000_000, 100_000, 1_000_000, 0, 0), false);
        assert!((c - (1.25 + 1.0 + 0.125)).abs() < 1e-9, "{c}");
        assert_eq!(cost("big-pickle", &toks(5, 5, 5, 5, 5), false), 0.0);
    }

    #[test]
    fn fast_mode_and_long_context() {
        let t = toks(1_000_000, 0, 0, 0, 0);
        assert!((cost("claude-opus-5-5", &t, true) - 8.0).abs() < 1e-9);
        // Sonnet 4.5 above 200k prompt tokens: $6 input.
        assert!((cost("claude-sonnet-4-5", &t, false) - 6.0).abs() < 1e-9);
        assert!((cost("claude-sonnet-4-5", &toks(100_000, 0, 0, 0, 0), false) - 0.3).abs() < 1e-9);
    }

    #[test]
    fn version_is_stable() {
        assert_eq!(version(), version());
        assert_eq!(version().len(), 16);
    }
}
