//! Parity-harness entries for the pi-providers module-level tools:
//! `config-value`, `http-dispatcher`, `auth-guidance`, `models-store`,
//! `runtime-credentials`, `virtual-models`.
//!
//! Each tool takes JSON args on stdin with a `fn` discriminator and emits
//! `{ result: ... }` on stdout (errors surface as `{"error": msg}` via the
//! `Err` path in `main.rs`). The ported logic itself lives in the
//! `pi-providers` crate; this file only adapts the wire protocol to those
//! APIs.

use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use pi_providers::auth_guidance;
use pi_providers::config_value::{
    self, CommandRunner, ConfigValueResolver, Env,
};
use pi_providers::error::JsError;
use pi_providers::http_config;
use pi_providers::models_store::{FileModelsStore, InMemoryCodingAgentModelsStore};
use pi_providers::pi_ai::credential::{
    AuthOperationOptions, Credential, CredentialInfo, CredentialStore,
};
use pi_providers::pi_ai::model::{Model, ModelInputModality, ModelThinkingLevel};
use pi_providers::pi_ai::models_store::{ModelsStore, ModelsStoreEntry};
use pi_providers::runtime_credentials::RuntimeCredentials;
use pi_providers::virtual_models::{self, VirtualModelDefinition};

/// Dispatch a pi-providers module-level harness tool.
pub fn run(tool: &str, input: &Value) -> Result<Value, String> {
    match tool {
        "config-value" => run_config_value(input),
        "http-dispatcher" => run_http_dispatcher(input),
        "auth-guidance" => run_auth_guidance(input),
        "models-store" => run_models_store(input),
        "runtime-credentials" => run_runtime_credentials(input),
        "virtual-models" => run_virtual_models(input),
        other => Err(format!("unknown providers tool: {}", other)),
    }
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Parse the optional `env` argument: an object of string → string, or
/// absent/null for no injected environment.
fn parse_env(input: &Value) -> Result<Option<Env>, String> {
    match input.get("env") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(map)) => {
            let mut env = BTreeMap::new();
            for (key, value) in map {
                let value = value
                    .as_str()
                    .ok_or_else(|| format!("non-string env value for {}", key))?;
                env.insert(key.clone(), value.to_string());
            }
            Ok(Some(env))
        }
        Some(_) => Err("env must be an object".to_string()),
    }
}

/// An object argument as an ordered list of key/value string pairs
/// (`serde_json` is built with `preserve_order`, so insertion order is
/// kept, like a JS object literal).
fn string_pairs(value: &Value) -> Option<Vec<(String, String)>> {
    let map = value.as_object()?;
    let mut pairs = Vec::with_capacity(map.len());
    for (k, v) in map {
        pairs.push((k.clone(), v.as_str()?.to_string()));
    }
    Some(pairs)
}

// ============================================================================
// config-value
// ============================================================================

/// Real shell execution for `!command` config values, mirroring the TS
/// `executeWithDefaultShell` (`execSync` with a 10 000 ms timeout):
/// `/bin/sh -c <command>`, stdout trimmed, empty → `None`, spawn error /
/// nonzero exit / timeout → `None`.
struct ShellCommandRunner;

impl CommandRunner for ShellCommandRunner {
    fn run(&self, command: &str) -> Option<String> {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_millis(10_000);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return None;
                    }
                    break;
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return None,
            }
        }
        let mut stdout = String::new();
        child
            .stdout
            .take()?
            .read_to_string(&mut stdout)
            .ok()?;
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

fn headers_to_value(resolved: Option<Vec<(String, String)>>) -> Value {
    match resolved {
        Some(pairs) => {
            let mut map = Map::new();
            for (key, value) in pairs {
                map.insert(key, Value::String(value));
            }
            Value::Object(map)
        }
        None => Value::Null,
    }
}

fn run_config_value(input: &Value) -> Result<Value, String> {
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    let env = parse_env(input)?;
    match fn_name {
        "getConfigValueEnvVarName" => {
            let config = input["config"].as_str().ok_or("missing config")?;
            Ok(json!({ "result": config_value::get_config_value_env_var_name(config) }))
        }
        "getConfigValueEnvVarNames" => {
            let config = input["config"].as_str().ok_or("missing config")?;
            Ok(json!({ "result": config_value::get_config_value_env_var_names(config) }))
        }
        "getMissingConfigValueEnvVarNames" => {
            let config = input["config"].as_str().ok_or("missing config")?;
            Ok(json!({
                "result": config_value::get_missing_config_value_env_var_names(config, env.as_ref())
            }))
        }
        "isCommandConfigValue" => {
            let config = input["config"].as_str().ok_or("missing config")?;
            Ok(json!({ "result": config_value::is_command_config_value(config) }))
        }
        "isConfigValueConfigured" => {
            let config = input["config"].as_str().ok_or("missing config")?;
            Ok(json!({ "result": config_value::is_config_value_configured(config, env.as_ref()) }))
        }
        "resolveConfigValue" => {
            let config = input["config"].as_str().ok_or("missing config")?;
            // Cached resolution (TS module-level commandResultCache).
            let mut resolver = ConfigValueResolver::new(&ShellCommandRunner);
            Ok(json!({ "result": resolver.resolve_config_value(config, env.as_ref()) }))
        }
        "resolveConfigValueOrThrow" => {
            let config = input["config"].as_str().ok_or("missing config")?;
            let description = input["description"].as_str().ok_or("missing description")?;
            match config_value::resolve_config_value_or_throw(
                config,
                description,
                env.as_ref(),
                &ShellCommandRunner,
            ) {
                Ok(value) => Ok(json!({ "result": value })),
                Err(error) => Err(error.to_string()),
            }
        }
        "resolveHeaders" => {
            let headers = match input.get("headers") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    string_pairs(value).ok_or("headers must be an object of strings")?,
                ),
            };
            let mut resolver = ConfigValueResolver::new(&ShellCommandRunner);
            let resolved = resolver.resolve_headers(headers.as_deref(), env.as_ref());
            Ok(json!({ "result": headers_to_value(resolved) }))
        }
        "resolveHeadersOrThrow" => {
            let headers = match input.get("headers") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    string_pairs(value).ok_or("headers must be an object of strings")?,
                ),
            };
            let description = input["description"].as_str().ok_or("missing description")?;
            match config_value::resolve_headers_or_throw(
                headers.as_deref(),
                description,
                env.as_ref(),
                &ShellCommandRunner,
            ) {
                Ok(resolved) => Ok(json!({ "result": headers_to_value(resolved) })),
                Err(error) => Err(error.to_string()),
            }
        }
        other => Err(format!("unknown config-value fn: {}", other)),
    }
}

// ============================================================================
// http-dispatcher
// ============================================================================

fn run_http_dispatcher(input: &Value) -> Result<Value, String> {
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "parseHttpIdleTimeoutMs" => {
            let value = input.get("value").cloned().unwrap_or(Value::Null);
            Ok(json!({ "result": http_config::parse_http_idle_timeout_ms(&value) }))
        }
        "formatHttpIdleTimeoutMs" => {
            let timeout_ms = input["timeoutMs"]
                .as_f64()
                .ok_or("missing timeoutMs")?;
            Ok(json!({ "result": http_config::format_http_idle_timeout_ms(timeout_ms) }))
        }
        other => Err(format!("unknown http-dispatcher fn: {}", other)),
    }
}

// ============================================================================
// auth-guidance
// ============================================================================

fn run_auth_guidance(input: &Value) -> Result<Value, String> {
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    let docs_path = input["docsPath"].as_str().ok_or("missing docsPath")?;
    // The messages embed the docs path; both harnesses normalize it to the
    // literal "<docs>" so outputs compare equal across environments.
    let normalize = |message: String| message.replace(docs_path, "<docs>");
    match fn_name {
        "getProviderLoginHelp" => {
            Ok(json!({ "result": normalize(auth_guidance::get_provider_login_help(docs_path)) }))
        }
        "formatNoModelsAvailableMessage" => Ok(json!({
            "result": normalize(auth_guidance::format_no_models_available_message(docs_path))
        })),
        "formatNoModelSelectedMessage" => Ok(json!({
            "result": normalize(auth_guidance::format_no_model_selected_message(docs_path))
        })),
        "formatNoApiKeyFoundMessage" => {
            let provider = input["provider"].as_str().ok_or("missing provider")?;
            Ok(json!({
                "result": normalize(auth_guidance::format_no_api_key_found_message(docs_path, provider))
            }))
        }
        other => Err(format!("unknown auth-guidance fn: {}", other)),
    }
}

// ============================================================================
// models-store
// ============================================================================

/// Drive `write`/`read`/`delete` steps against a models store, collecting
/// read results (entry JSON or null) in order.
fn drive_models_steps(
    store: &mut dyn ModelsStore,
    steps: &[Value],
) -> Result<Vec<Value>, String> {
    let mut reads = Vec::new();
    for step in steps {
        let op = step["op"].as_str().ok_or("missing step op")?;
        let provider_id = step["providerId"].as_str().ok_or("missing step providerId")?;
        match op {
            "write" => {
                let entry = ModelsStoreEntry::from_value(step.get("entry").unwrap_or(&Value::Null))
                    .ok_or("invalid step entry")?;
                store.write(provider_id, &entry, None).map_err(|e| e.message)?;
            }
            "read" => {
                let entry = store.read(provider_id, None).map_err(|e| e.message)?;
                reads.push(match entry {
                    Some(entry) => entry.to_value(),
                    None => Value::Null,
                });
            }
            "delete" => {
                store.delete(provider_id, None).map_err(|e| e.message)?;
            }
            other => return Err(format!("unknown models-store step op: {}", other)),
        }
    }
    Ok(reads)
}

fn run_models_store(input: &Value) -> Result<Value, String> {
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    let steps = input["steps"].as_array().ok_or("missing steps")?;
    match fn_name {
        "inMemory" => {
            let mut store = InMemoryCodingAgentModelsStore::new();
            let reads = drive_models_steps(&mut store, steps)?;
            Ok(json!({ "result": { "reads": reads } }))
        }
        "file" => {
            let path = input["path"].as_str().ok_or("missing path")?;
            // Per-case temp dir: temp/pi-parity-models-store/<pid>/<path>.
            let dir = std::env::temp_dir()
                .join("pi-parity-models-store")
                .join(std::process::id().to_string());
            let abs = dir.join(path);
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            if let Some(initial) = input.get("initialContent") {
                if let Some(content) = initial.as_str() {
                    std::fs::write(&abs, content).map_err(|e| e.to_string())?;
                }
            }
            // The TS FileAuthStorageBackend pre-creates the file ("{}") at
            // construction (ensureFileExists); the Rust port creates it
            // lazily (documented divergence). Mirror the TS side effect
            // here so the final-content comparison matches.
            if !abs.exists() {
                std::fs::write(&abs, "{}").map_err(|e| e.to_string())?;
            }
            let mut store = FileModelsStore::new(abs.to_string_lossy().into_owned());
            let reads = drive_models_steps(&mut store, steps)?;
            let content = std::fs::read_to_string(&abs).ok();
            // Best-effort cleanup of the per-case temp dir.
            let _ = std::fs::remove_dir_all(&dir);
            Ok(json!({ "result": { "reads": reads, "content": content } }))
        }
        other => Err(format!("unknown models-store fn: {}", other)),
    }
}

// ============================================================================
// runtime-credentials
// ============================================================================

/// Insertion-ordered credential-store fake mirroring the TS
/// `InMemoryCredentialStore` (`Map` semantics: `set` replaces in place or
/// appends; `modify` resolves with the post-write credential or the
/// current one).
struct FakeCredentialStore {
    entries: Vec<(String, Credential)>,
}

impl CredentialStore for FakeCredentialStore {
    fn read(
        &self,
        provider_id: &str,
        _options: Option<&AuthOperationOptions>,
    ) -> Result<Option<Credential>, JsError> {
        Ok(self
            .entries
            .iter()
            .find(|(id, _)| id == provider_id)
            .map(|(_, credential)| credential.clone()))
    }

    fn list(
        &self,
        _options: Option<&AuthOperationOptions>,
    ) -> Result<Vec<CredentialInfo>, JsError> {
        Ok(self
            .entries
            .iter()
            .map(|(id, credential)| CredentialInfo {
                provider_id: id.clone(),
                credential_type: credential.credential_type(),
            })
            .collect())
    }

    fn modify(
        &mut self,
        provider_id: &str,
        f: &mut dyn FnMut(Option<&Credential>) -> Result<Option<Credential>, JsError>,
        _options: Option<&AuthOperationOptions>,
    ) -> Result<Option<Credential>, JsError> {
        let index = self.entries.iter().position(|(id, _)| id == provider_id);
        let current = index.map(|i| self.entries[i].1.clone());
        let next = f(current.as_ref())?;
        match next {
            Some(next) => {
                match index {
                    Some(i) => self.entries[i].1 = next.clone(),
                    None => self.entries.push((provider_id.to_string(), next.clone())),
                }
                Ok(Some(next))
            }
            None => Ok(current),
        }
    }

    fn delete(
        &mut self,
        provider_id: &str,
        _options: Option<&AuthOperationOptions>,
    ) -> Result<(), JsError> {
        self.entries.retain(|(id, _)| id != provider_id);
        Ok(())
    }
}

fn credential_to_value(credential: Option<Credential>) -> Value {
    match credential {
        Some(credential) => credential.to_value(),
        None => Value::Null,
    }
}

fn run_runtime_credentials(input: &Value) -> Result<Value, String> {
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    if fn_name != "overlay" {
        return Err(format!("unknown runtime-credentials fn: {}", fn_name));
    }
    let backing = input["backing"].as_array().ok_or("missing backing")?;
    let mut fake = FakeCredentialStore { entries: Vec::new() };
    for item in backing {
        let provider_id = item["providerId"].as_str().ok_or("missing backing providerId")?;
        let credential = item
            .get("credential")
            .and_then(Value::as_object)
            .ok_or("missing backing credential")?;
        let credential = Credential::from_map(credential.clone())
            .map_err(|e| format!("invalid backing credential: {}", e))?;
        fake.entries.push((provider_id.to_string(), credential));
    }

    let mut rc = RuntimeCredentials::new(Box::new(fake));
    let steps = input["steps"].as_array().ok_or("missing steps")?;
    let mut has_results = Vec::new();
    let mut reads = Vec::new();
    let mut lists = Vec::new();
    let mut modifies = Vec::new();
    for step in steps {
        let op = step["op"].as_str().ok_or("missing step op")?;
        // `list` is the only step without a providerId.
        let provider_id = match op {
            "list" => "",
            _ => step["providerId"].as_str().ok_or("missing step providerId")?,
        };
        match op {
            "set" => {
                let key = step["key"].as_str().ok_or("missing step key")?;
                rc.set_runtime_api_key(provider_id, key);
            }
            "remove" => {
                rc.remove_runtime_api_key(provider_id);
            }
            "has" => {
                has_results.push(rc.has_runtime_api_key(provider_id));
            }
            "read" => {
                let credential = rc.read(provider_id, None).map_err(|e| e.message)?;
                reads.push(credential_to_value(credential));
            }
            "list" => {
                let entries = rc.list(None).map_err(|e| e.message)?;
                lists.push(Value::Array(
                    entries
                        .into_iter()
                        .map(|entry| {
                            json!({ "providerId": entry.provider_id, "type": entry.credential_type })
                        })
                        .collect(),
                ));
            }
            "delete" => {
                rc.delete(provider_id, None).map_err(|e| e.message)?;
            }
            "modify" => {
                // The closure returns a new credential when the step has a
                // `set` payload, else leaves the entry unchanged (the
                // modify then resolves with the current credential).
                let set = match step.get("set") {
                    Some(value) if !value.is_null() => Some(
                        Credential::from_map(
                            value.as_object().ok_or("invalid step set")?.clone(),
                        )
                        .map_err(|e| format!("invalid step set: {}", e))?,
                    ),
                    _ => None,
                };
                let resolved = rc
                    .modify(
                        provider_id,
                        &mut |_current: Option<&Credential>| -> Result<Option<Credential>, JsError> {
                            Ok(set.clone())
                        },
                        None,
                    )
                    .map_err(|e| e.message)?;
                modifies.push(credential_to_value(resolved));
            }
            other => return Err(format!("unknown runtime-credentials step op: {}", other)),
        }
    }
    Ok(json!({
        "result": { "has": has_results, "reads": reads, "lists": lists, "modifies": modifies }
    }))
}

// ============================================================================
// virtual-models
// ============================================================================

fn run_virtual_models(input: &Value) -> Result<Value, String> {
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "isVirtualModel" => {
            let model = input.get("model").ok_or("missing model")?;
            let api = model.get("api").and_then(Value::as_str).unwrap_or("");
            Ok(json!({ "result": virtual_models::is_virtual_model(api) }))
        }
        "findLatestResponse" => {
            let messages = input["messages"].as_array().ok_or("missing messages")?;
            Ok(json!({
                "result": virtual_models::find_latest_response(messages).cloned()
            }))
        }
        "getBranchSelection" => {
            let branch = input["branch"].as_array().ok_or("missing branch")?;
            let models = input["models"].as_array().ok_or("missing models")?;
            // The TS harness's getModel: find the catalog entry by
            // provider + id. TS returns the raw object whatever its shape;
            // here required Model fields are defaulted so parsing succeeds
            // (only `api` is observable through this call path).
            let get_model = |provider: &str, model_id: &str| -> Option<Model> {
                let raw = models.iter().find(|m| {
                    m.get("provider").and_then(Value::as_str) == Some(provider)
                        && m.get("id").and_then(Value::as_str) == Some(model_id)
                })?;
                let mut map = raw.as_object()?.clone();
                let defaults: [(&str, Value); 10] = [
                    ("id", json!(model_id)),
                    ("name", json!("")),
                    ("api", json!("")),
                    ("provider", json!(provider)),
                    ("baseUrl", json!("")),
                    ("input", json!(["text"])),
                    ("cost", json!({ "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 })),
                    ("reasoning", json!(false)),
                    ("contextWindow", json!(0)),
                    ("maxTokens", json!(0)),
                ];
                for (key, default) in defaults {
                    map.entry(key.to_string()).or_insert_with(|| default.clone());
                }
                Model::from_map(map).ok()
            };
            let selection = virtual_models::get_branch_selection(branch, get_model);
            Ok(json!({
                "result": selection.map(|(provider, model_id)| {
                    json!({ "provider": provider, "modelId": model_id })
                })
            }))
        }
        "getVirtualModelState" => {
            let branch = input["branch"].as_array().ok_or("missing branch")?;
            let provider = input["provider"].as_str().ok_or("missing provider")?;
            let model_id = input["modelId"].as_str().ok_or("missing modelId")?;
            Ok(json!({
                "result": virtual_models::get_virtual_model_state(branch, provider, model_id)
            }))
        }
        "createVirtualModel" => {
            let definition = input
                .get("definition")
                .and_then(Value::as_object)
                .ok_or("missing definition")?;
            let provider = definition.get("provider").and_then(Value::as_str)
                .ok_or("missing definition provider")?;
            let id = definition.get("id").and_then(Value::as_str)
                .ok_or("missing definition id")?;
            let name = definition.get("name").and_then(Value::as_str)
                .ok_or("missing definition name")?;
            let thinking_levels = match definition.get("thinkingLevels") {
                None | Some(Value::Null) => None,
                Some(Value::Array(levels)) => Some(
                    levels
                        .iter()
                        .map(|level| {
                            level
                                .as_str()
                                .and_then(ModelThinkingLevel::parse)
                                .ok_or("invalid thinkingLevels entry")
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                Some(_) => return Err("thinkingLevels must be an array".to_string()),
            };
            let number_field = |key: &str| -> Result<Option<f64>, String> {
                match definition.get(key) {
                    None | Some(Value::Null) => Ok(None),
                    Some(value) => value.as_f64().map(Some).ok_or_else(|| {
                        format!("definition {} must be a number", key)
                    }),
                }
            };
            let context_window = number_field("contextWindow")?;
            let max_tokens = number_field("maxTokens")?;
            let input_modalities = match definition.get("input") {
                None | Some(Value::Null) => None,
                Some(Value::Array(entries)) => Some(
                    entries
                        .iter()
                        .map(|entry| {
                            serde_json::from_value::<ModelInputModality>(entry.clone())
                                .map_err(|_| "invalid input entry")
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                Some(_) => return Err("definition input must be an array".to_string()),
            };
            let definition = VirtualModelDefinition {
                provider: provider.to_string(),
                id: id.to_string(),
                name: name.to_string(),
                thinking_levels,
                context_window,
                max_tokens,
                input: input_modalities,
                // Not reachable through createVirtualModel.
                route: Box::new(|_request| Err(JsError::new("virtual model route unavailable"))),
            };
            let model = virtual_models::create_virtual_model(&definition);
            Ok(json!({ "result": { "model": model.to_value() } }))
        }
        other => Err(format!("unknown virtual-models fn: {}", other)),
    }
}
