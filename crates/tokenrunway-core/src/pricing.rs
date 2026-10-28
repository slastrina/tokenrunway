//! API-equivalent cost (SPEC.md §10.1): what a call would have cost at the provider's list price,
//! whatever plan actually paid for it. The table (pricing.json, USD per million tokens) is
//! compiled in, and the app fetches the newest copy from the repository once a day
//! (`REMOTE_URL`): its entries are laid over the bundled ones, so a price change pushed to main
//! reaches everyone without a release, and the bundled table still answers when GitHub can't be
//! reached.
//!
//! Model ids arrive in many spellings for the same model ("claude-opus-4-5-20251101",
//! "us.anthropic.claude-opus-4-5-20251101-v1:0", "claude-opus-4-5@20251101",
//! "anthropic/claude-opus-4.5"), so `normalise` folds them to one key before the lookup. Lookups
//! are exact after that: a prefix match would price a new "claude-opus-4-9" as Opus 4, which is
//! worse than admitting we don't know it.

use crate::ingest::Tokens;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const BUNDLED: &str = include_str!("pricing.json");

/// The newest table, on the main branch.
pub const REMOTE_URL: &str = "https://raw.githubusercontent.com/slastrina/tokenrunway/main/crates/tokenrunway-core/src/pricing.json";

/// The table layout this version reads; a remote table with a higher `schema` is ignored.
pub const SCHEMA: u32 = 1;

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

#[derive(Debug, Clone, Deserialize)]
struct Table {
    #[serde(default = "first_schema")]
    schema: u32,
    models: HashMap<String, Price>,
    #[serde(default)]
    aliases: HashMap<String, String>,
}

fn first_schema() -> u32 {
    1
}

impl Price {
    fn valid(&self) -> bool {
        let ok = |v: f64| v.is_finite() && v >= 0.0;
        let lc = self.long_context.as_ref().is_none_or(|l| ok(l.input) && ok(l.output) && l.cache_read.is_none_or(ok));
        ok(self.input)
            && ok(self.output)
            && [self.cache_read, self.cache_write_5m, self.cache_write_1h, self.fast].into_iter().flatten().all(ok)
            && lc
    }
}

/// A fetched table, checked: it parses, this version understands its schema, it has models, and
/// every price is a finite number of zero or more.
fn parse_remote(raw: &str) -> Result<Table, String> {
    let t: Table = serde_json::from_str(raw).map_err(|e| format!("not a price table: {e}"))?;
    if t.schema > SCHEMA {
        return Err(format!("needs a newer Tokenrunway (schema {})", t.schema));
    }
    if t.models.is_empty() {
        return Err("no models".into());
    }
    if let Some((id, _)) = t.models.iter().find(|(_, p)| !p.valid()) {
        return Err(format!("bad price for {id}"));
    }
    Ok(t)
}

/// The table in use and its version.
struct Active {
    table: Table,
    version: String,
}

impl Active {
    fn bundled() -> Self {
        Self { table: serde_json::from_str(BUNDLED).expect("bundled pricing.json is valid"), version: fnv(&[BUNDLED]) }
    }

    /// The bundled table with a fetched one laid over it: the fetched entries win, and a model
    /// it leaves out keeps its bundled price.
    fn with_remote(raw: &str) -> Result<Self, String> {
        let remote = parse_remote(raw)?;
        let mut a = Self::bundled();
        a.table.models.extend(remote.models);
        a.table.aliases.extend(remote.aliases);
        a.version = fnv(&[BUNDLED, raw]);
        Ok(a)
    }
}

/// FNV-1a: stable across runs and Rust versions, unlike DefaultHasher.
fn fnv(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h = (h ^ 0xff).wrapping_mul(0x100000001b3);
        }
        for b in p.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    }
    format!("{h:016x}")
}

static ACTIVE: RwLock<Option<Arc<Active>>> = RwLock::new(None);

fn active() -> Arc<Active> {
    if let Some(a) = ACTIVE.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return a.clone();
    }
    let mut w = ACTIVE.write().unwrap_or_else(|e| e.into_inner());
    w.get_or_insert_with(|| Arc::new(Active::bundled())).clone()
}

fn set_active(a: Active) {
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(a));
}

/// Changes whenever the table in use does (a new bundled table, or a fetched one), so the store
/// knows to re-price what it already has.
pub fn version() -> String {
    active().version.clone()
}

/// Puts a fetched table (pricing.json's text) in use over the bundled one. On an error the
/// table in use doesn't change.
pub fn use_remote(raw: &str) -> Result<(), String> {
    set_active(Active::with_remote(raw)?);
    Ok(())
}

/// Back to the bundled table only.
pub fn use_bundled() {
    set_active(Active::bundled());
}

/// Fetches the newest table's text and checks it (without putting it in use).
pub async fn fetch(c: &reqwest::Client) -> Result<String, String> {
    let res = c.get(REMOTE_URL).timeout(Duration::from_secs(15)).send().await.map_err(|e| {
        if e.is_timeout() {
            "timed out".to_string()
        } else if e.is_connect() {
            "couldn't reach GitHub".into()
        } else {
            "network error".into()
        }
    })?;
    if !res.status().is_success() {
        return Err(format!("GitHub answered HTTP {}", res.status().as_u16()));
    }
    let raw = res.text().await.map_err(|_| "network error".to_string())?;
    parse_remote(&raw)?;
    Ok(raw)
}

/// The last good fetched table, kept in the data folder so an offline start has it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cached {
    /// When it was fetched (ms)
    pub fetched_at: i64,
    /// pricing.json's text as fetched
    pub table: String,
}

impl Cached {
    /// The cache at `path`, if there is one this version can use.
    pub fn load(path: &Path) -> Option<Self> {
        let c: Self = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        parse_remote(&c.table).ok()?;
        Some(c)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, path)
    }
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
pub fn lookup(model: &str) -> Option<Price> {
    if is_local(model) {
        return None;
    }
    let a = active();
    let t = &a.table;
    let key = normalise(model);
    t.models.get(&key).or_else(|| t.aliases.get(&key).and_then(|k| t.models.get(k))).cloned()
}

/// A model run locally (LM Studio): free, and never matched to a cloud model's price, even when
/// it shares a name with one ("lmstudio/gpt-oss-20b").
pub fn is_local(model: &str) -> bool {
    model.starts_with(crate::ingest::lmstudio::MODEL_PREFIX)
}

/// A model id from a cloud provider whose list price the table should have (Claude, GPT and
/// o-series, Gemini, Grok), as opposed to an open model run through opencode or similar.
pub fn is_cloud(model: &str) -> bool {
    if is_local(model) {
        return false;
    }
    let k = normalise(model);
    (["claude-", "gpt-", "codex-", "gemini-", "grok-"].iter().any(|p| k.starts_with(p)) && !k.starts_with("gpt-oss"))
        || (k.starts_with('o') && k[1..].starts_with(|c: char| c.is_ascii_digit()))
}

/// "Opus 5.5" for "claude-opus-5-5-20260101"; for models the table doesn't know, the id as the
/// harness wrote it, without a provider path ("lmstudio/Qwen3-30B" → "Qwen3-30B").
pub fn display_name(model: &str) -> String {
    lookup(model).map(|p| p.name).unwrap_or_else(|| model.rsplit('/').next().unwrap_or(model).trim().to_string())
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
    fn current_openai_models() {
        // Codex's default models in 2026: these counted at $0 before the table had them.
        for (id, name) in [
            ("gpt-5.6-terra", "GPT-5.6 Terra"),
            ("gpt-5.6-sol", "GPT-5.6 Sol"),
            ("gpt-5.5", "GPT-5.5"),
            ("gpt-5.3-codex", "GPT-5.3 Codex"),
            ("openai/gpt-6.1-terra", "GPT-6.1 Terra"),
        ] {
            assert_eq!(lookup(id).map(|p| p.name).as_deref(), Some(name), "{id}");
        }
        let c = cost("gpt-5.6-terra", &toks(1_000_000, 100_000, 2_000_000, 0, 0), false);
        assert!((c - (2.0 + 1.2 + 0.4)).abs() < 1e-9, "{c}");
        let long = cost("gpt-5.5", &toks(300_000, 0, 0, 0, 0), false);
        assert!((long - 3.0).abs() < 1e-9, "long-context tier above 272K: {long}");
    }

    #[test]
    fn cloud_or_open_model() {
        for m in ["gpt-9-nova", "claude-opus-9", "o5-mini", "gemini-4-pro", "grok-6", "openai/gpt-7"] {
            assert!(is_cloud(m), "{m}");
        }
        for m in ["Qwen3.8-27B-Heretic-GGUF", "qwen/qwen3-coder-30b", "big-pickle", "lmstudio/gpt-oss-20b", "gpt-oss-120b", "ollama"] {
            assert!(!is_cloud(m), "{m}");
        }
    }

    #[test]
    fn version_is_stable() {
        assert_eq!(Active::bundled().version, Active::bundled().version);
        assert_eq!(Active::bundled().version.len(), 16);
    }

    #[test]
    fn bundled_table_is_valid() {
        let t = parse_remote(BUNDLED).unwrap();
        assert_eq!(t.schema, SCHEMA);
    }

    #[test]
    fn fetched_table_is_laid_over_the_bundled_one() {
        let raw = r#"{"schema":1,"models":{
            "gpt-5.6-terra": {"name":"GPT-5.6 Terra","input":3,"output":12},
            "brand-new-model": {"name":"Brand New","input":1,"output":2}
        },"aliases":{"brand-new":"brand-new-model"}}"#;
        let a = Active::with_remote(raw).unwrap();
        assert_eq!(a.table.models["gpt-5.6-terra"].input, 3.0, "the fetched price wins");
        assert!(a.table.models.contains_key("claude-opus-5-5"), "a model it leaves out keeps its bundled price");
        assert_eq!(a.table.aliases["brand-new"], "brand-new-model");
        assert!(a.table.aliases.contains_key("gpt-5-chat-latest"));
        assert_ne!(a.version, Active::bundled().version);
        assert_eq!(a.version, Active::with_remote(raw).unwrap().version);
        assert_ne!(a.version, Active::with_remote(&raw.replace("\"input\":3", "\"input\":4")).unwrap().version);
    }

    #[test]
    fn bad_tables_are_refused() {
        for (raw, why) in [
            ("not json", "not a price table"),
            (r#"{"schema":2,"models":{"a":{"name":"A","input":1,"output":1}}}"#, "newer Tokenrunway"),
            (r#"{"schema":1,"models":{}}"#, "no models"),
            (r#"{"schema":1,"models":{"a":{"name":"A","input":-1,"output":1}}}"#, "bad price"),
            (
                r#"{"schema":1,"models":{"a":{"name":"A","input":1,"output":1,"long_context":{"above":1,"input":1,"output":-2}}}}"#,
                "bad price",
            ),
            (r#"{"models":{"a":{"name":"A","input":"1","output":1}}}"#, "not a price table"),
        ] {
            let e = Active::with_remote(raw).err().unwrap_or_default();
            assert!(e.contains(why), "{raw}: {e}");
        }
        // No schema field reads as the first.
        assert!(Active::with_remote(r#"{"models":{"a":{"name":"A","input":1,"output":1}}}"#).is_ok());
    }

    #[test]
    fn swapping_the_table_in_use() {
        // A model only this test knows, so the other tests' lookups don't change meanwhile.
        let raw = r#"{"schema":1,"models":{"swap-test-model":{"name":"Swap Test","input":1,"output":1}}}"#;
        assert!(use_remote("{}").is_err());
        use_remote(raw).unwrap();
        assert_eq!(lookup("swap-test-model").map(|p| p.name).as_deref(), Some("Swap Test"));
        assert_eq!(version(), Active::with_remote(raw).unwrap().version);
        use_bundled();
        assert!(lookup("swap-test-model").is_none());
    }

    /// Fetches the table on main: `cargo test -p tokenrunway-core -- --ignored live_prices`
    #[tokio::test]
    #[ignore]
    async fn live_prices() {
        let raw = fetch(&crate::update::client()).await.unwrap();
        let a = Active::with_remote(&raw).unwrap();
        println!("{} models on main", a.table.models.len());
        assert!(a.table.models.len() >= Active::bundled().table.models.len());
    }

    #[test]
    fn cache_round_trip() {
        let path = std::env::temp_dir().join(format!("tr-prices-{}.json", std::process::id()));
        let c = Cached { fetched_at: 1_800_000_000_000, table: BUNDLED.to_string() };
        c.save(&path).unwrap();
        assert_eq!(Cached::load(&path), Some(c));
        std::fs::write(&path, r#"{"fetchedAt":1,"table":"{\"schema\":9,\"models\":{}}"}"#).unwrap();
        assert_eq!(Cached::load(&path), None, "a cache this version can't use is ignored");
        std::fs::remove_file(&path).ok();
        assert_eq!(Cached::load(&path), None);
    }
}
