//! Parity-harness entry for the `agent-session-run` protocol: the run
//! orchestration slice of `pi_session::AgentSession` (TS
//! `dist/core/agent-session.js` — `_startRun`/`_prepareRun`/
//! `_handlePostAgentRun`/`_runBeforeSettleBoundary`, boundary drafts, and
//! `abort()`), driven end-to-end over a real `Agent` with a scripted
//! provider. TS mirror: parity/ts-harness.js `agent-session-run` entry.
//!
//! args:
//! ```json
//! {
//!   "nowMs":  number,
//!   "config": { "settings"?: { "retry"?: {...}, "imageAutoResize"? },
//!               "modelAuth"?: { "mode": "ok"|"noKey"|"oauth" },
//!               "model"?: object|null, "entries": [...],
//!               "systemPrompt": string,          // ALWAYS set (custom
//!                                              // preamble; the default
//!              "customTools"?: [ToolDefinitionJson], // sections render
//!              "modelLookup"?: [ModelJson] },
//!   "tools":  { "<key>": { "name", "label"?, "description", "parameters"? } },
//!   "extensionHandlers": {
//!     "before_agent_start"?: [ { "message"?, "systemPrompt"?,
//!                                "selectedTools"?, "setActiveTools"?, "abort"? } ],
//!     "turn_end"?:          [ { "entries"?, "continue"?, "abort"? } ],
//!     "agent_before_settle"?: [ { "entries"?, "continue"?, "abort"? } ],
//!     "agent_settled"?:     { "prompt"?: string }   // fires once on the
//!   },                                                 // first settled
//!   "script": [ { "op": "prompt", "text", "options"? } |
//!               { "op": "abort" } | { "op": "waitForIdle" } ],
//!   "providerTurns": [ { "events"?: [...], "result": {...} } ]
//! }
//! ```
//!
//! Result: `{ results, sessionEvents, entries, idleTimeline,
//! extensionEvents, extensionErrors, promptsSentToAgent }`.
//!
//! **Documented divergences (harness side):** (1) the recorded
//! `before_agent_start` events omit the real runner's `systemPrompt`
//! getter (the rendered prompt); (2) `ctx.setActiveTools` requests are
//! applied after the emission (library trait docs); (3) the deferred
//! `agent_settled` prompt is pushed onto the shared deferred-actions slot
//! (`deferred_prompt_tap`) where the TS fake fires `void session.prompt`.

use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use pi_agent::types::BoxFuture;
use pi_agent::{Agent, AgentOptions, AgentTool, AgentToolResult, Clock, FixedClock};
use pi_core::session_manager::{Generators, SessionManager};
use pi_core::settings_manager::RetrySettings;
use pi_session::agent_session::tools::ToolDefinition;
use pi_session::agent_session::{
    AgentSession, AgentSessionConfig, ExtensionHost, ModelAuthStore, SessionSettingsStore,
};

const HARNESS_CWD: &str = "/proj";
const DEFAULT_DOCS_PATH: &str = "/pi-docs";

struct RunSettings {
    retry: RetrySettings,
    image_auto_resize: bool,
}

impl SessionSettingsStore for RunSettings {
    fn retry_settings(&self) -> RetrySettings {
        self.retry.clone()
    }
    fn image_auto_resize(&self) -> bool {
        self.image_auto_resize
    }
}

fn provider_login_help(docs_path: &str) -> String {
    [
        "Use /login to log into a provider via OAuth or API key. See:".to_string(),
        format!("  {}/providers.md", docs_path),
        format!("  {}/models.md", docs_path),
    ]
    .join("\n")
}

struct FixedModelAuth {
    mode: String,
    docs_path: String,
}

impl ModelAuthStore for FixedModelAuth {
    fn has_configured_auth(&self, _provider: &str) -> bool {
        self.mode == "ok"
    }
    fn check_auth(&self, _provider: &str) -> bool {
        self.mode == "ok"
    }
    fn is_using_oauth(&self, _provider: &str) -> bool {
        self.mode == "oauth"
    }
    fn no_model_selected_message(&self) -> String {
        format!(
            "No model selected.\n\n{}\n\nThen use /model to select a model.",
            provider_login_help(&self.docs_path)
        )
    }
    fn no_api_key_message(&self, provider: &str) -> String {
        let display = if provider == "unknown" { "the selected model" } else { provider };
        format!("No API key found for {}.\n\n{}", display, provider_login_help(&self.docs_path))
    }
}

/// Base-tool stand-in (TS: a plain AgentTool whose execute throws).
struct BaseJsonTool {
    spec: Value,
}

impl AgentTool for BaseJsonTool {
    fn name(&self) -> &str {
        self.spec.get("name").and_then(Value::as_str).unwrap_or("")
    }
    fn description(&self) -> &str {
        self.spec.get("description").and_then(Value::as_str).unwrap_or("")
    }
    fn label(&self) -> &str {
        self.spec.get("label").and_then(Value::as_str).unwrap_or("")
    }
    fn parameters(&self) -> &Value {
        self.spec.get("parameters").unwrap_or(&Value::Null)
    }
    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _args: &'a Value,
        _signal: Option<&'a pi_agent::host::AbortHandle>,
        _on_update: Option<&'a pi_agent::AgentToolUpdateCallback<'a>>,
    ) -> BoxFuture<'a, Result<AgentToolResult, String>> {
        Box::pin(async {
            Err("agent-session-run harness tool execute must never be called".to_string())
        })
    }
}

/// Recording fake `_extensionRunner` with scripted boundary handlers.
struct RunExtensionHost {
    events: Mutex<Vec<Value>>,
    errors: Mutex<Vec<Value>>,
    before_agent_start: Vec<Value>,
    turn_end: Vec<Value>,
    agent_before_settle: Vec<Value>,
    /// `{ prompt?: string }` — fired once on the first `agent_settled`.
    agent_settled: Option<Value>,
    settled_fired: Mutex<bool>,
    /// Invocation counts per boundary event type (for `once: true` specs).
    boundary_calls: Mutex<Vec<(String, usize)>>,
    abort_tap: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    deferred_prompt_tap: Mutex<Option<Arc<dyn Fn(&str) + Send + Sync>>>,
}

impl RunExtensionHost {
    fn record(&self, event: Value) {
        self.events.lock().expect("extension log").push(event);
    }
}

impl ExtensionHost for RunExtensionHost {
    fn has_handlers(&self, _event: &str) -> bool {
        false
    }
    fn emit<'a>(&'a self, event: Value) -> BoxFuture<'a, ()> {
        let is_settled = event.get("type").and_then(Value::as_str) == Some("agent_settled");
        self.record(event);
        if is_settled {
            let mut fired = self.settled_fired.lock().expect("settled fired");
            if !*fired {
                *fired = true;
                let prompt = self
                    .agent_settled
                    .as_ref()
                    .and_then(|spec| spec.get("prompt"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(prompt) = prompt {
                    if let Some(tap) = self.deferred_prompt_tap.lock().expect("tap").clone() {
                        tap(&prompt);
                    }
                }
            }
        }
        Box::pin(async {})
    }
    fn emit_message_end<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>> {
        self.record(event);
        Box::pin(async { None })
    }
    fn emit_error(&self, error: Value) {
        self.errors.lock().expect("extension errors").push(error);
    }
    fn invalidate(&self, _message: &str) {}
    fn get_all_registered_tools(&self) -> Vec<pi_session::agent_session::tools::RegisteredTool> {
        Vec::new()
    }
    fn boundary_handlers(&self, event_type: &str) -> usize {
        match event_type {
            "turn_end" => self.turn_end.len(),
            "agent_before_settle" => self.agent_before_settle.len(),
            _ => 0,
        }
    }
    fn emit_boundary_handler<'a>(&'a self, index: usize, event: Value) -> BoxFuture<'a, Option<Value>> {
        let script = match event.get("type").and_then(Value::as_str) {
            Some("turn_end") => &self.turn_end,
            Some("agent_before_settle") => &self.agent_before_settle,
            _ => &self.before_agent_start,
        };
        // `once: true` specs model a stateful handler that only acts on the
        // first boundary emission (a real extension can keep state; the
        // static spec list cannot).
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or_default().to_string();
        let call = {
            let mut calls = self.boundary_calls.lock().expect("boundary calls");
            let entry = calls.iter_mut().find(|(k, _)| k == &event_type);
            match entry {
                Some((_, count)) => {
                    *count += 1;
                    *count
                }
                None => {
                    calls.push((event_type.clone(), 1));
                    1
                }
            }
        };
        let spec = script.get(index).cloned();
        if spec
            .as_ref()
            .and_then(|s| s.get("once"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && call > 1
        {
            return Box::pin(async { None });
        }
        self.record(event.clone());
        if let Some(spec) = &spec {
            if spec.get("abort").and_then(Value::as_bool) == Some(true) {
                if let Some(tap) = self.abort_tap.lock().expect("tap").clone() {
                    tap();
                }
            }
        }
        let mut result = Map::new();
        if let Some(spec) = &spec {
            if let Some(entries) = spec.get("entries") {
                result.insert("entries".to_string(), entries.clone());
            }
            if let Some(continue_flag) = spec.get("continue") {
                result.insert("continue".to_string(), continue_flag.clone());
            }
        }
        Box::pin(async move {
            if result.is_empty() {
                None
            } else {
                Some(Value::Object(result))
            }
        })
    }
    fn emit_before_agent_start<'a>(
        &'a self,
        trigger: Value,
        options: Value,
    ) -> BoxFuture<'a, (Vec<Value>, Value, Vec<Vec<String>>)> {
        let mut current = options;
        let mut messages: Vec<Value> = Vec::new();
        let mut requests: Vec<Vec<String>> = Vec::new();
        for spec in &self.before_agent_start {
            // Recorded event: `{...trigger, type, systemPromptOptions}` —
            // the real runner's `systemPrompt` getter is not recorded.
            let mut event = trigger.as_object().cloned().unwrap_or_default();
            event.insert("type".to_string(), json!("before_agent_start"));
            event.insert("systemPromptOptions".to_string(), current.clone());
            self.record(Value::Object(event));
            if spec.get("abort").and_then(Value::as_bool) == Some(true) {
                if let Some(tap) = self.abort_tap.lock().expect("tap").clone() {
                    tap();
                }
            }
            if let Some(names) = spec.get("setActiveTools").and_then(Value::as_array) {
                requests.push(names.iter().filter_map(Value::as_str).map(String::from).collect());
            }
            if let Some(selected) = spec.get("selectedTools") {
                if let Some(object) = current.as_object_mut() {
                    object.insert("selectedTools".to_string(), selected.clone());
                }
            }
            if let Some(system_prompt) = spec.get("systemPrompt") {
                if let Some(object) = current.as_object_mut() {
                    object.insert("forceSystemPrompt".to_string(), system_prompt.clone());
                }
            }
            if let Some(message) = spec.get("message") {
                messages.push(message.clone());
            }
        }
        Box::pin(async move { (messages, current, requests) })
    }
}

pub fn run_agent_session_run_harness(args: &Value) -> Result<Value, String> {
    // Validation order mirrors the TS entry exactly.
    let now_ms = args.get("nowMs").and_then(Value::as_f64).ok_or_else(|| "missing nowMs".to_string())?;
    let config = args.get("config").cloned().unwrap_or(json!({}));
    if !config.is_object() {
        return Err("invalid config".to_string());
    }
    let settings_in = config.get("settings").cloned().unwrap_or(json!({}));
    if !settings_in.is_object() {
        return Err("invalid settings".to_string());
    }
    let retry_in = settings_in.get("retry").cloned().unwrap_or(json!({}));
    if !retry_in.is_object() {
        return Err("invalid settings".to_string());
    }
    if let Some(flag) = settings_in.get("imageAutoResize") {
        if !flag.is_boolean() {
            return Err("invalid imageAutoResize".to_string());
        }
    }
    let model_auth_in = config.get("modelAuth").cloned().unwrap_or(json!({}));
    if !model_auth_in.is_object() {
        return Err("invalid modelAuth".to_string());
    }
    let auth_mode = model_auth_in.get("mode").and_then(Value::as_str).unwrap_or("ok").to_string();
    if !matches!(auth_mode.as_str(), "ok" | "noKey" | "oauth") {
        return Err("invalid modelAuth".to_string());
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
    let base_tools = args
        .get("tools")
        .filter(|v| v.is_object())
        .ok_or_else(|| "missing tools".to_string())?
        .as_object()
        .expect("tools object")
        .clone();
    let custom_tools_in = config.get("customTools").cloned().unwrap_or(json!([]));
    if !custom_tools_in.is_array() {
        return Err("invalid customTools".to_string());
    }
    let model_lookup_in = config.get("modelLookup").cloned().unwrap_or(json!([]));
    if !model_lookup_in.is_array() {
        return Err("invalid modelLookup".to_string());
    }
    let handlers_in = args.get("extensionHandlers").cloned().unwrap_or(json!({}));
    if !handlers_in.is_object() {
        return Err("invalid extensionHandlers".to_string());
    }
    let handler_list = |key: &str| -> Result<Vec<Value>, String> {
        match handlers_in.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(list)) => {
                for spec in list {
                    if !spec.is_object() {
                        return Err(format!("invalid {}", key));
                    }
                }
                Ok(list.clone())
            }
            _ => Err(format!("invalid {}", key)),
        }
    };
    let before_agent_start = handler_list("before_agent_start")?;
    let turn_end = handler_list("turn_end")?;
    let agent_before_settle = handler_list("agent_before_settle")?;
    let agent_settled = match handlers_in.get("agent_settled") {
        None | Some(Value::Null) => None,
        Some(spec) if spec.is_object() => Some(spec.clone()),
        Some(_) => return Err("invalid agent_settled".to_string()),
    };
    let script = args
        .get("script")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing script".to_string())?
        .clone();
    let provider_turns = args
        .get("providerTurns")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing providerTurns".to_string())?
        .clone();

    let retry = RetrySettings {
        enabled: retry_in.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        max_retries: retry_in.get("maxRetries").and_then(Value::as_f64).unwrap_or(3.0),
        base_delay_ms: 2000.0,
        max_agent_delay_ms: 300000.0,
    };
    let image_auto_resize =
        settings_in.get("imageAutoResize").and_then(Value::as_bool).unwrap_or(true);

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
    let uuid_v7 = {
        let counter = Arc::clone(&uuid_counter);
        Arc::new(move || {
            let mut n = counter.lock().expect("uuid counter");
            *n += 1;
            format!("uid{:05}", n)
        })
    };
    let now_gen = Arc::new(move || now_ms_i);
    let gen = Generators::with_gens(uuid_v7, uuid_v4, now_gen);

    let session_manager = SessionManager::in_memory(HARNESS_CWD, None, entries, &gen)?;
    let projected = session_manager
        .build_session_projection()
        .messages
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect::<Vec<Value>>();

    let prompts_sent: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let provider_cursor = Arc::new(Mutex::new(0usize));
    let stream_fn: Arc<pi_agent::LoopStreamFn> = {
        let turns = provider_turns;
        let cursor = Arc::clone(&provider_cursor);
        let sink = Arc::clone(&prompts_sent);
        Arc::new(move |_model: &Value, context: &Value, _options: &Value| {
            let turns = turns.clone();
            let cursor = Arc::clone(&cursor);
            let sink = Arc::clone(&sink);
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
                sink.lock()
                    .expect("prompts sent")
                    .push(context.get("messages").cloned().unwrap_or(Value::Array(Vec::new())));
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

    let host = Arc::new(RunExtensionHost {
        events: Mutex::new(Vec::new()),
        errors: Mutex::new(Vec::new()),
        before_agent_start,
        turn_end,
        agent_before_settle,
        agent_settled,
        settled_fired: Mutex::new(false),
        boundary_calls: Mutex::new(Vec::new()),
        abort_tap: Mutex::new(None),
        deferred_prompt_tap: Mutex::new(None),
    });

    let base_override: Vec<(String, Arc<dyn AgentTool>)> = base_tools
        .iter()
        .map(|(key, spec)| {
            if !spec.is_object() {
                return Err("tools entries must be objects".to_string());
            }
            // `parameters: spec.parameters ?? {}` (TS harness).
            let mut spec = spec.clone();
            if spec.get("parameters").map(Value::is_null).unwrap_or(true) {
                spec.as_object_mut()
                    .expect("tools entries are objects")
                    .insert("parameters".to_string(), json!({}));
            }
            Ok((key.clone(), Arc::new(BaseJsonTool { spec }) as Arc<dyn AgentTool>))
        })
        .collect::<Result<_, String>>()?;
    let custom_tools: Vec<ToolDefinition> = custom_tools_in
        .as_array()
        .expect("customTools array")
        .iter()
        .map(|t| {
            serde_json::from_value(t.clone()).map_err(|_| "invalid customTools".to_string())
        })
        .collect::<Result<_, String>>()?;
    let model_lookup_models = model_lookup_in.clone();
    let model_lookup: Option<
        Arc<dyn Fn(&str, &str) -> Option<Value> + Send + Sync>,
    > = if model_lookup_models.is_null() || model_lookup_models.as_array().is_none_or(|a| a.is_empty()) {
        None
    } else {
        Some(Arc::new(move |provider: &str, id: &str| {
            model_lookup_models.as_array().and_then(|models| {
                models
                    .iter()
                    .find(|m| {
                        m.get("provider").and_then(Value::as_str) == Some(provider)
                            && m.get("id").and_then(Value::as_str) == Some(id)
                    })
                    .cloned()
            })
        }))
    };

    let mut session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager,
        settings: Arc::new(RunSettings { retry, image_auto_resize }),
        extension_host: Arc::clone(&host) as Arc<dyn ExtensionHost>,
        clock: Arc::clone(&clock),
        nested_tool_calls: None,
        cwd: HARNESS_CWD.to_string(),
        custom_tools,
        initial_active_tool_names: None,
        uses_default_tools: false,
        default_tool_modifiers: Vec::new(),
        allowed_tool_names: None,
        excluded_tool_names: None,
        base_tools_override: Some(base_override),
        model_auth: Some(Arc::new(FixedModelAuth {
            mode: auth_mode,
            docs_path: DEFAULT_DOCS_PATH.to_string(),
        })),
        skills: Vec::new(),
        prompt_templates: Vec::new(),
        system_prompt: Some(system_prompt),
        model_lookup,
        sleeper: None,
    });

    // Install the session-reaching taps (TS: the fake runner closes over
    // `session`).
    *host.abort_tap.lock().expect("tap") = Some(session.abort_tap());
    *host.deferred_prompt_tap.lock().expect("tap") = Some(session.deferred_prompt_tap());

    let session_events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let idle_timeline: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = Arc::clone(&session_events);
        let timeline = Arc::clone(&idle_timeline);
        session.subscribe(Box::new(move |event: &Value| {
            if event.get("type").and_then(Value::as_str) == Some("agent_settled") {
                timeline.lock().expect("idle timeline").push(json!({
                    "kind": "settled",
                    "aborted": event.get("aborted").and_then(Value::as_bool).unwrap_or(false),
                }));
            }
            sink.lock().expect("session events").push(event.clone());
        }));
    }

    pi_agent::run_current_thread(async {
        let mut results: Vec<Value> = Vec::with_capacity(script.len());
        for op in &script {
            let outcome = run_op(&mut session, op, &idle_timeline).await;
            results.push(match outcome {
                Ok(value) => value,
                Err(message) => json!({ "thrown": message }),
            });
        }

        Ok(json!({
            "result": {
                "results": results,
                "sessionEvents": session_events.lock().expect("session events").clone(),
                "entries": session.session_manager.get_entries(),
                "idleTimeline": idle_timeline.lock().expect("idle timeline").clone(),
                "extensionEvents": host.events.lock().expect("extension log").clone(),
                "extensionErrors": host.errors.lock().expect("extension errors").clone(),
                "promptsSentToAgent": prompts_sent.lock().expect("prompts sent").clone(),
            }
        }))
    })
}

/// Run one script op; `Err` becomes the op's `{thrown}` entry.
async fn run_op(
    session: &mut AgentSession,
    op: &Value,
    idle_timeline: &Arc<Mutex<Vec<Value>>>,
) -> Result<Value, String> {
    let obj = op.as_object().ok_or_else(|| "ops entries must be objects".to_string())?;
    let name = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing op".to_string())?;
    match name {
        "prompt" => {
            let text = obj
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing text".to_string())?;
            let options = obj.get("options").cloned().unwrap_or(json!({}));
            if !options.is_object() {
                return Err("invalid options".to_string());
            }
            if let Some(flag) = options.get("expandPromptTemplates") {
                if !flag.is_boolean() {
                    return Err("invalid expandPromptTemplates".to_string());
                }
            }
            if let Some(source) = options.get("source") {
                if !source.is_null() && !source.is_string() {
                    return Err("invalid source".to_string());
                }
            }
            let prompt_options = pi_session::agent_session::prompt::PromptOptions {
                expand_prompt_templates: options.get("expandPromptTemplates").and_then(Value::as_bool),
                images: match options.get("images") {
                    None | Some(Value::Null) => None,
                    Some(Value::Array(images)) => Some(images.clone()),
                    Some(_) => return Err("invalid images".to_string()),
                },
                streaming_behavior: options.get("streamingBehavior").and_then(Value::as_str).map(str::to_string),
                source: options.get("source").and_then(Value::as_str).map(str::to_string),
                preflight_result: None,
            };
            session.prompt(&text, prompt_options).await?;
            Ok(Value::Null)
        }
        "abort" => {
            session.abort().await;
            Ok(Value::Null)
        }
        "waitForIdle" => {
            session.wait_for_idle().await;
            idle_timeline
                .lock()
                .expect("idle timeline")
                .push(json!({ "kind": "waitIdle" }));
            Ok(Value::Null)
        }
        other => Err(format!("unknown agent-session-run op: {}", other)),
    }
}
