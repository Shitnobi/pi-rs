//! Parity-harness entry for the `agent-session-model` protocol: the model
//! & thinking-management slice of `pi_session::AgentSession` (TS `dist/
//! core/agent-session.js` — `setModel`/`cycleModel`/`setThinkingLevel`/
//! `cycleThinkingLevel`/`getAvailableThinkingLevels`/`supportsThinking`)
//! plus the `prepareRequest` projection with virtual-model routing
//! (`_installAgentRequestProjection` → `_modelRuntime.resolveModel`),
//! driven end-to-end over a real `Agent` with a scripted provider. TS
//! mirror: parity/ts-harness.js `agent-session-model` entry.
//!
//! args:
//! ```json
//! {
//!   "nowMs": number,
//!   "config": { "model"?: object|null, "entries": [...],
//!               "systemPrompt": string,
//!               "scopedModels"?: [{ "model": {...}, "thinkingLevel"?: string }],
//!               "settings"?: { "defaultProvider"?: string,
//!                              "defaultModel"?: string,
//!                              "enabledModels"?: [string],
//!                              "defaultThinkingLevel"?: string,
//!                              "modelThinkingLevels"?: { "p/id": string } } },
//!   "models": [...],
//!   "authDenied"?: [string],
//!   "resolveScript": [ { "model": {...}, "thinkingLevel": string,
//!                        "state"?: value } | { "throw": string } ],
//!   "script": [ { "op": "setModel", "modelIndex": number, "persist"?: bool } |
//!               { "op": "cycleModel", "direction"?: "forward"|"backward",
//!                 "persist"?: bool } |
//!               { "op": "setThinkingLevel", "level": string, "persist"?: bool } |
//!               { "op": "cycleThinkingLevel", "persist"?: bool } |
//!               { "op": "prompt", "text": string } |
//!               { "op": "waitForIdle" } ]
//! }
//! ```
//!
//! Result: `{ results, sessionEvents, entries, modelChanges,
//! thinkingLevelChanges, extensionEvents, providerCalls, resolveCalls,
//! settingsEnd }`.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pi_agent::host::{Clock, FixedClock, Sleeper};
use pi_agent::types::BoxFuture;
use pi_agent::{Agent, AgentOptions};
use pi_core::session_manager::{Generators, SessionManager};
use pi_session::agent_session::model::{
    ModelMutationOptions, ModelRouteStore, RouteOutcome, RouteRequest, ScopedModel,
};
use pi_session::agent_session::{
    AgentSession, AgentSessionConfig, ExtensionHost, ModelAuthStore, SessionSettingsStore,
};

const HARNESS_CWD: &str = "/proj";
const DEFAULT_DOCS_PATH: &str = "/pi-docs";

/// The mutable settings slice the model ops read and write (the TS entry
/// uses the real `SettingsManager.inMemory`, whose getters read back the
/// setters' writes; this fake keeps one state cell).
#[derive(Default)]
struct SettingsState {
    default_provider: Option<String>,
    default_model: Option<String>,
    enabled_models: Option<Vec<String>>,
    default_thinking_level: Option<String>,
    model_thinking_levels: Option<serde_json::Map<String, Value>>,
}

struct ModelHarnessSettings {
    state: Mutex<SettingsState>,
}

impl SessionSettingsStore for ModelHarnessSettings {
    fn retry_settings(&self) -> pi_core::settings_manager::RetrySettings {
        // TS `SettingsManager.inMemory({})` retry defaults.
        pi_core::settings_manager::RetrySettings {
            enabled: true,
            max_retries: 3.0,
            base_delay_ms: 2000.0,
            max_agent_delay_ms: pi_session::agent_session::retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS,
        }
    }
    fn set_default_model_and_provider(&self, provider: &str, model_id: &str) {
        let mut state = self.state.lock().expect("settings state");
        state.default_provider = Some(provider.to_string());
        state.default_model = Some(model_id.to_string());
    }
    fn get_enabled_models(&self) -> Option<Vec<String>> {
        self.state.lock().expect("settings state").enabled_models.clone()
    }
    fn set_enabled_models(&self, patterns: Vec<String>) {
        self.state.lock().expect("settings state").enabled_models = Some(patterns);
    }
    fn set_default_thinking_level(&self, level: &str) {
        self.state.lock().expect("settings state").default_thinking_level = Some(level.to_string());
    }
    fn get_model_thinking_level(&self, provider: &str, model_id: &str) -> Option<String> {
        self.state
            .lock()
            .expect("settings state")
            .model_thinking_levels
            .as_ref()?
            .get(&format!("{}/{}", provider, model_id))
            .and_then(Value::as_str)
            .map(str::to_string)
    }
    fn get_default_thinking_level(&self) -> Option<String> {
        self.state.lock().expect("settings state").default_thinking_level.clone()
    }
}

struct ModelHarnessAuth {
    denied: Vec<String>,
}

impl ModelAuthStore for ModelHarnessAuth {
    fn has_configured_auth(&self, _provider: &str) -> bool {
        true
    }
    fn check_auth(&self, provider: &str) -> bool {
        !self.denied.iter().any(|p| p == provider)
    }
    fn is_using_oauth(&self, _provider: &str) -> bool {
        false
    }
    fn no_model_selected_message(&self) -> String {
        "No model selected.".to_string()
    }
    fn no_api_key_message(&self, _provider: &str) -> String {
        format!("No API key found for the provider. See {}/providers.md", DEFAULT_DOCS_PATH)
    }
}

/// Recording fake `_modelRuntime` routing seam: `getAvailableSnapshot`
/// from `args.models`, scripted `resolveModel` (advances per call).
struct ModelHarnessRoutes {
    models: Vec<Value>,
    script: Vec<Value>,
    calls: Mutex<Vec<Value>>,
}

impl ModelRouteStore for ModelHarnessRoutes {
    fn available_snapshot(&self) -> Vec<Value> {
        self.models.clone()
    }
    fn resolve_model(&self, model: &Value, messages: &[Value], request: &RouteRequest) -> Result<RouteOutcome, String> {
        let mut calls = self.calls.lock().expect("resolve calls");
        calls.push(json!({
            "model": model,
            "messages": messages,
            "reason": request.reason,
            "thinkingLevel": request.thinking_level,
            "failed": request.failed,
            "state": request.state,
        }));
        let index = calls.len() - 1;
        let entry = match self.script.get(index) {
            Some(entry) => entry.clone(),
            None => return Err("resolve script exhausted".to_string()),
        };
        if let Some(message) = entry.get("throw").and_then(Value::as_str) {
            return Err(message.to_string());
        }
        Ok(RouteOutcome {
            model: entry.get("model").cloned().unwrap_or(Value::Null),
            thinking_level: entry
                .get("thinkingLevel")
                .and_then(Value::as_str)
                .unwrap_or("off")
                .to_string(),
            state: entry.get("state").filter(|s| !s.is_null()).cloned(),
        })
    }
}

/// Recording fake `_extensionRunner` (no handlers; `emit` records).
struct ModelHarnessHost {
    events: Mutex<Vec<Value>>,
    errors: Mutex<Vec<Value>>,
}

impl ExtensionHost for ModelHarnessHost {
    fn has_handlers(&self, _event: &str) -> bool {
        false
    }
    fn emit<'a>(&'a self, event: Value) -> BoxFuture<'a, ()> {
        self.events.lock().expect("extension log").push(event);
        Box::pin(async {})
    }
    fn emit_message_end<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>> {
        self.events.lock().expect("extension log").push(event);
        Box::pin(async { None })
    }
    fn emit_error(&self, error: Value) {
        self.errors.lock().expect("extension errors").push(error);
    }
    fn invalidate(&self, _message: &str) {}
}

/// The patched-`setTimeout` mirror: fast-forward (no log — the model ops
/// only sleep through the retry backoff, which the TS entry patches the
/// same way without recording).
struct ImmediateSleeper;

impl Sleeper for ImmediateSleeper {
    fn sleep<'a>(&'a self, _ms: u64, signal: &'a pi_agent::host::AbortHandle) -> BoxFuture<'a, Result<(), pi_agent::host::Aborted>> {
        Box::pin(async move {
            if signal.is_aborted() {
                return Err(pi_agent::host::Aborted);
            }
            Ok(())
        })
    }
}

pub fn run_agent_session_model_harness(args: &Value) -> Result<Value, String> {
    // Validation order mirrors the TS entry exactly.
    let now_ms = args.get("nowMs").and_then(Value::as_f64).ok_or_else(|| "missing nowMs".to_string())?;
    let config = args.get("config").cloned().unwrap_or(json!({}));
    if !config.is_object() {
        return Err("invalid config".to_string());
    }
    let system_prompt = match config.get("systemPrompt") {
        Some(Value::String(prompt)) => prompt.clone(),
        _ => return Err("invalid systemPrompt".to_string()),
    };
    let entries = config
        .get("entries")
        .and_then(Value::as_array)
        .filter(|e| !e.is_empty())
        .filter(|e| e.first().and_then(|h| h.get("type")).and_then(Value::as_str) == Some("session"))
        .ok_or_else(|| "missing entries".to_string())?
        .clone();
    if let Some(model) = config.get("model") {
        if !model.is_null() && !model.is_object() {
            return Err("invalid model".to_string());
        }
    }
    let scoped_models_in = config.get("scopedModels").cloned().unwrap_or(json!([]));
    if !scoped_models_in.is_array() {
        return Err("invalid scopedModels".to_string());
    }
    let mut scoped_models: Vec<ScopedModel> = Vec::new();
    for scoped in scoped_models_in.as_array().expect("scopedModels array") {
        if !scoped.is_object() {
            return Err("invalid scopedModels".to_string());
        }
        let model = scoped.get("model").cloned().unwrap_or(Value::Null);
        if !model.is_object() {
            return Err("invalid scopedModels".to_string());
        }
        let thinking_level = match scoped.get("thinkingLevel") {
            None | Some(Value::Null) => None,
            Some(Value::String(level)) => Some(level.clone()),
            _ => return Err("invalid scopedModels".to_string()),
        };
        scoped_models.push(ScopedModel { model, thinking_level });
    }
    let settings_in = config.get("settings").cloned().unwrap_or(json!({}));
    if !settings_in.is_object() {
        return Err("invalid settings".to_string());
    }
    let mut initial = SettingsState::default();
    let string_field = |value: &Value| -> Result<Option<String>, String> {
        match value {
            Value::Null => Ok(None),
            Value::String(text) => Ok(Some(text.clone())),
            _ => Err("invalid settings".to_string()),
        }
    };
    initial.default_provider = string_field(settings_in.get("defaultProvider").unwrap_or(&Value::Null))?;
    initial.default_model = string_field(settings_in.get("defaultModel").unwrap_or(&Value::Null))?;
    match settings_in.get("enabledModels") {
        None | Some(Value::Null) => {}
        Some(Value::Array(patterns)) => {
            let mut parsed = Vec::new();
            for pattern in patterns {
                match pattern.as_str() {
                    Some(pattern) => parsed.push(pattern.to_string()),
                    None => return Err("invalid settings".to_string()),
                }
            }
            initial.enabled_models = Some(parsed);
        }
        _ => return Err("invalid settings".to_string()),
    }
    initial.default_thinking_level = string_field(settings_in.get("defaultThinkingLevel").unwrap_or(&Value::Null))?;
    match settings_in.get("modelThinkingLevels") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            let mut parsed = serde_json::Map::new();
            for (key, level) in map {
                match level.as_str() {
                    Some(level) => {
                        parsed.insert(key.clone(), json!(level));
                    }
                    None => return Err("invalid settings".to_string()),
                }
            }
            initial.model_thinking_levels = Some(parsed);
        }
        _ => return Err("invalid settings".to_string()),
    }
    let models = args
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing models".to_string())?
        .clone();
    if models.iter().any(|model| !model.is_object()) {
        return Err("invalid models".to_string());
    }
    let auth_denied = match args.get("authDenied") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(providers)) => {
            let mut parsed = Vec::new();
            for provider in providers {
                match provider.as_str() {
                    Some(provider) => parsed.push(provider.to_string()),
                    None => return Err("invalid authDenied".to_string()),
                }
            }
            parsed
        }
        _ => return Err("invalid authDenied".to_string()),
    };
    let resolve_script = match args.get("resolveScript") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(script)) => {
            for entry in script {
                if !entry.is_object() {
                    return Err("invalid resolveScript".to_string());
                }
                if entry.get("throw").and_then(Value::as_str).is_some() {
                    continue;
                }
                if !entry.get("model").map(Value::is_object).unwrap_or(false)
                    || entry.get("thinkingLevel").and_then(Value::as_str).is_none()
                {
                    return Err("invalid resolveScript".to_string());
                }
            }
            script.clone()
        }
        _ => return Err("invalid resolveScript".to_string()),
    };
    let script = args
        .get("script")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing script".to_string())?
        .clone();

    let now_ms_i = now_ms as i64;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(vec![now_ms], vec![]));
    let uuid_counter = Arc::new(Mutex::new(0usize));
    let uuid_v4 = {
        let counter = Arc::clone(&uuid_counter);
        Arc::new(move || {
            let mut n = counter.lock().expect("uuid counter");
            *n += 1;
            format!("uid{:05}", n)
        })
    };
    let uuid_v7: pi_core::session_manager::UuidGen = Arc::new(|| "summarization-session".to_string());
    let now_gen = Arc::new(move || now_ms_i);
    let gen = Generators::with_gens(uuid_v7, uuid_v4, now_gen);

    let session_manager = SessionManager::in_memory(HARNESS_CWD, None, entries, &gen)?;
    let projected = session_manager
        .build_session_projection()
        .messages
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect::<Vec<Value>>();

    let provider_calls: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let provider_turns = args
        .get("providerTurns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let provider_cursor = Arc::new(Mutex::new(0usize));
    let stream_fn: Arc<pi_agent::LoopStreamFn> = {
        let turns = provider_turns;
        let cursor = Arc::clone(&provider_cursor);
        let sink = Arc::clone(&provider_calls);
        Arc::new(move |model: &Value, context: &Value, _options: &Value| {
            let turns = turns.clone();
            let cursor = Arc::clone(&cursor);
            let sink = Arc::clone(&sink);
            let model = model.clone();
            let context = context.clone();
            Box::pin(async move {
                let index = {
                    let mut cursor = cursor.lock().expect("provider cursor");
                    let value = *cursor;
                    *cursor += 1;
                    value
                };
                let turn = match turns.get(index) {
                    Some(turn) => turn.clone(),
                    None => return Err("provider script exhausted".to_string()),
                };
                sink.lock().expect("provider calls").push(json!({
                    "model": model,
                    "messages": context.get("messages").cloned().unwrap_or(Value::Array(Vec::new())),
                }));
                Ok(pi_agent::provider::StreamOutput {
                    events: turn
                        .get("events")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                        .into(),
                    result: turn.get("result").cloned().unwrap_or(Value::Null),
                })
            }) as BoxFuture<'static, Result<pi_agent::provider::StreamOutput, String>>
        })
    };

    let agent = Agent::new(AgentOptions {
        messages: projected,
        tools: Vec::new(),
        system_prompt: None,
        model: config.get("model").filter(|m| !m.is_null()).cloned().unwrap_or(Value::Null),
        thinking_level: "off".to_string(),
        steering_mode: Default::default(),
        follow_up_mode: Default::default(),
        stream_fn,
        tool_execution: Default::default(),
        clock: Arc::clone(&clock),
    });

    let host = Arc::new(ModelHarnessHost {
        events: Mutex::new(Vec::new()),
        errors: Mutex::new(Vec::new()),
    });
    let session_events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let routes = Arc::new(ModelHarnessRoutes {
        models,
        script: resolve_script,
        calls: Mutex::new(Vec::new()),
    });
    let settings = Arc::new(ModelHarnessSettings { state: Mutex::new(initial) });

    let mut session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager,
        settings: Arc::clone(&settings) as Arc<dyn SessionSettingsStore>,
        extension_host: Arc::clone(&host) as Arc<dyn ExtensionHost>,
        clock: Arc::clone(&clock),
        nested_tool_calls: None,
        cwd: HARNESS_CWD.to_string(),
        custom_tools: Vec::new(),
        initial_active_tool_names: None,
        uses_default_tools: false,
        default_tool_modifiers: Vec::new(),
        allowed_tool_names: None,
        excluded_tool_names: None,
        base_tools_override: Some(Vec::new()),
        model_auth: Some(Arc::new(ModelHarnessAuth { denied: auth_denied })),
        skills: Vec::new(),
        prompt_templates: Vec::new(),
        system_prompt: Some(system_prompt),
        model_lookup: None,
        scoped_models,
        model_routes: Some(Arc::clone(&routes) as Arc<dyn ModelRouteStore>),
        sleeper: Some(Arc::new(ImmediateSleeper)),
    });
    {
        let sink = Arc::clone(&session_events);
        session.subscribe(Box::new(move |event: &Value| {
            sink.lock().expect("session events").push(event.clone());
        }));
    }

    let models_for_ops = routes.models.clone();
    pi_agent::run_current_thread(async move {
        let mut results: Vec<Value> = Vec::with_capacity(script.len());
        for op in &script {
            let outcome = run_op(&mut session, op, &models_for_ops).await;
            results.push(match outcome {
                Ok(value) => value,
                Err(message) => json!({ "thrown": message }),
            });
        }

        let entries = session.session_manager.get_entries();
        let model_changes: Vec<Value> = entries
            .iter()
            .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("model_change"))
            .cloned()
            .collect();
        let thinking_level_changes: Vec<Value> = entries
            .iter()
            .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("thinking_level_change"))
            .cloned()
            .collect();
        let settings_end = {
            let state = settings.state.lock().expect("settings state");
            json!({
                "defaultProvider": state.default_provider,
                "defaultModel": state.default_model,
                "enabledModels": state.enabled_models,
                "defaultThinkingLevel": state.default_thinking_level,
                "modelThinkingLevels": state.model_thinking_levels.clone().unwrap_or_default(),
            })
        };
        Ok(json!({
            "result": {
                "results": results,
                "sessionEvents": session_events.lock().expect("session events").clone(),
                "entries": entries,
                "modelChanges": model_changes,
                "thinkingLevelChanges": thinking_level_changes,
                "extensionEvents": host.events.lock().expect("extension log").clone(),
                "extensionErrors": host.errors.lock().expect("extension errors").clone(),
                "providerCalls": provider_calls.lock().expect("provider calls").clone(),
                "resolveCalls": routes.calls.lock().expect("resolve calls").clone(),
                "settingsEnd": settings_end,
            }
        }))
    })
}

/// Run one script op; `Err` becomes the op's `{thrown}` entry.
async fn run_op(session: &mut AgentSession, op: &Value, models: &[Value]) -> Result<Value, String> {
    let obj = op.as_object().ok_or_else(|| "ops entries must be objects".to_string())?;
    let name = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing op".to_string())?;
    let persist = |obj: &serde_json::Map<String, Value>| -> Result<bool, String> {
        match obj.get("persist") {
            None | Some(Value::Bool(false)) => Ok(false),
            Some(Value::Bool(true)) => Ok(true),
            _ => Err("invalid persist".to_string()),
        }
    };
    match name {
        "setModel" => {
            let model_index = obj
                .get("modelIndex")
                .and_then(Value::as_u64)
                .ok_or_else(|| "missing modelIndex".to_string())?;
            let model = models
                .get(model_index as usize)
                .cloned()
                .ok_or_else(|| "modelIndex out of range".to_string())?;
            session
                .set_model(&model, ModelMutationOptions { persist: persist(obj)? })
                .await?;
            Ok(Value::Null)
        }
        "cycleModel" => {
            let direction = match obj.get("direction") {
                None => "forward".to_string(),
                Some(Value::String(direction)) if direction == "forward" || direction == "backward" => {
                    direction.clone()
                }
                _ => return Err("invalid direction".to_string()),
            };
            let result = session
                .cycle_model(&direction, ModelMutationOptions { persist: persist(obj)? })
                .await;
            Ok(match result {
                Some(result) => json!({
                    "model": result.model,
                    "thinkingLevel": result.thinking_level,
                    "isScoped": result.is_scoped,
                }),
                None => Value::Null,
            })
        }
        "setThinkingLevel" => {
            let level = obj
                .get("level")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing level".to_string())?;
            session
                .set_thinking_level(level, ModelMutationOptions { persist: persist(obj)? })
                .await;
            Ok(Value::Null)
        }
        "cycleThinkingLevel" => {
            let result = session
                .cycle_thinking_level(ModelMutationOptions { persist: persist(obj)? })
                .await;
            Ok(match result {
                Some(level) => json!(level),
                None => Value::Null,
            })
        }
        "prompt" => {
            let text = obj
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing text".to_string())?;
            session.prompt(text, Default::default()).await?;
            Ok(Value::Null)
        }
        "waitForIdle" => {
            session.wait_for_idle().await;
            Ok(Value::Null)
        }
        other => Err(format!("unknown agent-session-model op: {}", other)),
    }
}
