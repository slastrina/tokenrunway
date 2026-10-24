//! Local model servers, found without setup: LM Studio's server on this computer (the Now tab's
//! LM Studio card). Only ever 127.0.0.1, on the port in LM Studio's own server config.
//!
//! - `GET /api/v0/models` (LM Studio's REST API) lists the downloaded models with their state
//!   ("loaded" / "not-loaded"), quantisation and context length.
//! - Older versions without it answer `GET /v1/models` (OpenAI-style, ids only): running, but which
//!   model is loaded is unknown.

use crate::view::LocalModel;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 1234;

/// LM Studio's home: `LMSTUDIO_HOME`, else ~/.lmstudio.
pub fn lmstudio_home(home: &Path) -> PathBuf {
    std::env::var_os("LMSTUDIO_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".lmstudio"))
}

/// LM Studio is (or was) installed here.
pub fn lmstudio_installed(home: &Path) -> bool {
    lmstudio_home(home).is_dir()
}

/// The server port from `.internal/http-server-config.json` ({"port": 1234, …}), else 1234.
pub fn lmstudio_port(home: &Path) -> u16 {
    let path = lmstudio_home(home).join(".internal").join("http-server-config.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("port")?.as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p > 0)
        .unwrap_or(DEFAULT_PORT)
}

/// What the server said: every model it knows, and whether the answer says which are loaded.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Probe {
    pub models: Vec<LocalModel>,
    /// False for a plain /v1/models answer
    pub knows_state: bool,
}

/// `/api/v0/models` → models (loaded first).
pub fn parse_v0(v: &Value) -> Option<Probe> {
    let data = v.get("data")?.as_array()?;
    let mut models: Vec<LocalModel> = data
        .iter()
        .filter_map(|m| {
            Some(LocalModel {
                id: m.get("id")?.as_str()?.to_string(),
                kind: m.get("type").and_then(Value::as_str).unwrap_or("llm").to_string(),
                loaded: m.get("state").and_then(Value::as_str) == Some("loaded"),
                quant: m.get("quantization").and_then(Value::as_str).map(str::to_string),
                context: m
                    .get("loaded_context_length")
                    .and_then(Value::as_u64)
                    .or_else(|| m.get("max_context_length").and_then(Value::as_u64)),
            })
        })
        .collect();
    models.sort_by_key(|m| !m.loaded);
    Some(Probe { models, knows_state: true })
}

/// `/v1/models` → ids only.
pub fn parse_v1(v: &Value) -> Option<Probe> {
    let data = v.get("data")?.as_array()?;
    let models = data
        .iter()
        .filter_map(|m| Some(LocalModel { id: m.get("id")?.as_str()?.to_string(), kind: "llm".into(), ..Default::default() }))
        .collect();
    Some(Probe { models, knows_state: false })
}

/// Asks the server on 127.0.0.1:`port`; None when nothing answers (not running). Never through a
/// proxy, and quick to give up, so a stopped server costs nothing.
pub async fn probe_lmstudio(port: u16) -> Option<Probe> {
    let http = reqwest::Client::builder().no_proxy().timeout(Duration::from_millis(1500)).build().ok()?;
    let get = |path: &str| {
        let url = format!("http://127.0.0.1:{port}{path}");
        let http = http.clone();
        async move {
            let r = http.get(url).send().await.ok()?;
            if !r.status().is_success() {
                return None;
            }
            r.json::<Value>().await.ok()
        }
    };
    if let Some(p) = get("/api/v0/models").await.as_ref().and_then(parse_v0) {
        return Some(p);
    }
    get("/v1/models").await.as_ref().and_then(parse_v1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_api_shape() {
        let v: Value = serde_json::from_str(
            r#"{"data":[
              {"id":"qwen3.5-27b-heretic","type":"vlm","state":"not-loaded","quantization":"Q4_K_S","max_context_length":262144,"loaded_context_length":null},
              {"id":"gpt-oss-20b","type":"llm","state":"loaded","quantization":"MXFP4","max_context_length":131072,"loaded_context_length":32768},
              {"id":"text-embedding-nomic-embed-text-v1.5","type":"embeddings","state":"not-loaded","quantization":"Q4_K_M","max_context_length":2048}
            ]}"#,
        )
        .unwrap();
        let p = parse_v0(&v).unwrap();
        assert!(p.knows_state);
        assert_eq!(p.models.len(), 3);
        assert_eq!((p.models[0].id.as_str(), p.models[0].loaded, p.models[0].context), ("gpt-oss-20b", true, Some(32768)), "loaded first");
        assert_eq!(p.models[1].context, Some(262144), "the model's own limit when it isn't loaded");
        assert_eq!(p.models[2].kind, "embeddings");
        let v1 = parse_v1(&serde_json::json!({"data":[{"id":"a"},{"id":"b"}]})).unwrap();
        assert!(!v1.knows_state && v1.models.iter().all(|m| !m.loaded));
        assert!(parse_v0(&serde_json::json!({"error":"nope"})).is_none());
    }

    #[test]
    fn port_from_lm_studios_config() {
        let dir = std::env::temp_dir().join(format!("tr-local-{}", std::process::id()));
        let internal = dir.join(".lmstudio").join(".internal");
        std::fs::create_dir_all(&internal).unwrap();
        assert!(lmstudio_installed(&dir));
        assert_eq!(lmstudio_port(&dir), DEFAULT_PORT, "no config yet");
        std::fs::write(internal.join("http-server-config.json"), r#"{"autoStartOnLaunch":true,"port":41234}"#).unwrap();
        assert_eq!(lmstudio_port(&dir), 41234);
        std::fs::remove_dir_all(&dir).ok();
        assert!(!lmstudio_installed(&dir));
    }

    #[tokio::test]
    async fn nothing_listening_is_not_running() {
        // A port that's almost certainly closed: answered quickly with None.
        let t = std::time::Instant::now();
        assert!(probe_lmstudio(1).await.is_none());
        assert!(t.elapsed() < Duration::from_secs(4));
    }
}
