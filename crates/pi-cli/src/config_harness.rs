//! Parity-harness entries for the config/resolution wave tools:
//! `provider-attribution`, `model-config`.
//!
//! Each tool takes JSON args with a `fn` discriminator and emits
//! `{ result: ... }` on stdout (errors surface as `{"error": msg}` via
//! the `Err` path in `main.rs`). The ported logic lives in the
//! `pi-providers` crate; this file only adapts the wire protocol.

use serde_json::{json, Map, Value};

use pi_providers::model_config::ModelConfig;
use pi_providers::pi_ai::model::Model;
use pi_providers::provider_attribution::{merge_provider_attribution_headers, ProviderHeaders};

/// Dispatch a config/resolution-wave harness tool.
pub fn run(tool: &str, input: &Value) -> Result<Value, String> {
    match tool {
        "provider-attribution" => run_provider_attribution(input),
        "model-config" => run_model_config(input),
        other => Err(format!("unknown config tool: {}", other)),
    }
}

// ============================================================================
// provider-attribution
// ============================================================================

fn run_provider_attribution(input: &Value) -> Result<Value, String> {
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "mergeProviderAttributionHeaders" => {
            let model_value = input.get("model").ok_or("missing model")?;
            let map = model_value
                .as_object()
                .ok_or("model must be an object")?
                .clone();
            let model = Model::from_map(map).map_err(|e| format!("invalid model: {}", e))?;
            let telemetry_enabled = input["telemetryEnabled"]
                .as_bool()
                .ok_or("missing telemetryEnabled")?;
            // Absent (undefined) and null both mean "no session id".
            let session_id = match input.get("sessionId") {
                None | Some(Value::Null) => None,
                Some(Value::String(session_id)) => Some(session_id.as_str()),
                Some(_) => return Err("sessionId must be a string".to_string()),
            };
            let header_sources = parse_header_sources(input.get("headerSources"))?;
            let merged = merge_provider_attribution_headers(
                &model,
                telemetry_enabled,
                session_id,
                &header_sources,
            );
            Ok(json!({ "result": headers_to_value(merged) }))
        }
        other => Err(format!("unknown provider-attribution fn: {}", other)),
    }
}

/// `headerSources`: array of header objects with `null` → `undefined`
/// (skipped) entries; absent → empty.
fn parse_header_sources(
    value: Option<&Value>,
) -> Result<Vec<Option<ProviderHeaders>>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut sources = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::Null => sources.push(None),
                    Value::Object(map) => {
                        let mut headers = ProviderHeaders::new();
                        for (key, value) in map {
                            let value = value.as_str().ok_or_else(|| {
                                format!("non-string header value for {}", key)
                            })?;
                            headers.insert(key.clone(), value.to_string());
                        }
                        sources.push(Some(headers));
                    }
                    _ => {
                        return Err("headerSources entries must be objects or null".to_string())
                    }
                }
            }
            Ok(sources)
        }
        Some(_) => Err("headerSources must be an array".to_string()),
    }
}

/// Merged headers as a JSON object (insertion order observable), or null
/// for the TS `undefined` result.
fn headers_to_value(headers: Option<ProviderHeaders>) -> Value {
    match headers {
        Some(headers) => {
            let mut map = Map::new();
            for (key, value) in headers {
                map.insert(key, Value::String(value));
            }
            Value::Object(map)
        }
        None => Value::Null,
    }
}

// ============================================================================
// model-config
// ============================================================================

fn run_model_config(input: &Value) -> Result<Value, String> {
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "load" => {
            let path = optional_str(input.get("path"), "path")?;
            let content = optional_str(input.get("content"), "content")?;
            let io_error = optional_str(input.get("ioError"), "ioError")?;
            let config = ModelConfig::from_load(path, content, io_error);
            let provider_ids = config.get_provider_ids();
            let mut providers = Map::new();
            for provider_id in &provider_ids {
                let provider = config
                    .get_provider(provider_id)
                    .cloned()
                    .unwrap_or(Value::Null);
                providers.insert((*provider_id).to_string(), provider);
            }
            Ok(json!({
                "result": {
                    "providers": Value::Object(providers),
                    "providerIds": provider_ids,
                    "error": config.get_error(),
                }
            }))
        }
        other => Err(format!("unknown model-config fn: {}", other)),
    }
}

/// An optional string argument: absent or null → `None`.
fn optional_str<'a>(
    value: Option<&'a Value>,
    name: &str,
) -> Result<Option<&'a str>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(format!("{} must be a string", name)),
    }
}
