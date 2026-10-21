//! Parity-harness entry for the `agent-session-prompt` protocol: the
//! prompt pipeline slice of `pi_session::AgentSession` (TS
//! `dist/core/agent-session.js` — `prompt`, `steer`/`followUp`,
//! `sendUserMessage`, extension-command dispatch, skill/template
//! expansion, image normalization seam, and the reduced `_startRun` run
//! driver). TS mirror: parity/ts-harness.js `agent-session-prompt` entry.
//!
//! args:
//! ```json
//! {
//!   "nowMs":  number,              // pinned Date.now / clock
//!   "config": { "settings": { "retry"?: {...}, "imageAutoResize"? },
//!               "modelAuth"?: { "mode": "ok"|"noKey"|"oauth" },
//!               "docsPath"?: string, "entries": [...],
//!               "model"?: object|null,
//!               "steeringMode"?, "followUpMode"? },
//!   "commands":      { "<name>": { "mode": "ok"|"throw", "errorText"? } },
//!   "inputHandler"?: { "mode": "continue"|"handled"|"transform",
//!                      "text"?, "images"? },
//!   "templates":     [ PromptTemplateJson ],
//!   "skills":        [ { "name", "filePath", "baseDir", "content",
//!                        "readFails"? } ],
//!   "script":        [ { "op": ... } ],
//!   "providerTurns": [ { "events"?: [...], "result": {...} } ]
//! }
//! ```
//!
//! The TS side runs the REAL AgentSession with the real `Agent` over a
//! scripted provider (the `agent` entry's providerTurns scripting); the
//! `_extensionRunner` and `_resourceLoader` are swapped for fakes after
//! construction. `docsPath`-bearing model/auth error messages are
//! normalized to `<docs>` on both sides (TS embeds the real
//! `getDocsPath()`). Both sides run the real `_preparePromptAndToolLoadout`
//! (system-patch diff): the fake loader's `getSystemPrompt` is
//! `config.systemPrompt` (cases pin a non-null customPrompt so the
//! sections are deterministic), and the fake `emitBeforeAgentStart`
//! returns `{ systemPromptOptions: clone, messages: [] }` (no handler
//! messages). The TS harness writes real skill files; the Rust side reads
//! the injected `content` directly (`readFails` stands in for ENOENT).
//!
//! Ops and their logged results (`results`): per-op errors surface as
//! `{ "thrown": msg }` (docs paths normalized).
//! - `{op:"prompt", text, options?: {expandPromptTemplates?, images?,
//!        streamingBehavior?, source?, preflightTag?}}` → `null`
//! - `{op:"steer"|"followUp", text, images?, source?}` → `"queued"|"handled"`
//! - `{op:"sendCustomMessage", customType, content, display, details?,
//!        triggerTurn?, deliverAs?}` → `null`
//! - `{op:"sendUserMessage", content, deliverAs?, expandPromptTemplates?}` → `null`
//! - `{op:"setStreaming", value}` → `null` (any-cast `_isAgentRunActive`)
//!
//! Result:
//! ```json
//! { "results", "dispositions", "sessionEvents", "extensionEvents",
//!   "extensionErrors", "commandCalls", "inputEvents", "queueState",
//!   "promptsSentToAgent", "entries" }
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pi_agent::types::BoxFuture;
use pi_agent::{Agent, AgentOptions, Clock, FixedClock};
use pi_core::prompt_templates::PromptTemplate;
use pi_core::session_manager::{Generators, SessionManager};
use pi_core::settings_manager::RetrySettings;
use pi_session::agent_session::prompt::{InputAction, PromptOptions, SkillSpec};
use pi_session::agent_session::{
    AgentSession, AgentSessionConfig, ExtensionHost, ModelAuthStore, SessionSettingsStore,
};

const HARNESS_CWD: &str = "/proj";
/// Default stand-in for the TS `getDocsPath()` (normalized to `<docs>`
/// before comparison; the literal value is unobservable).
const DEFAULT_DOCS_PATH: &str = "/pi-docs";

// ============================================================================
// Fakes (TS harness swaps the same seams post-construction)
// ============================================================================

struct PromptSettings {
    retry: RetrySettings,
    image_auto_resize: bool,
}

impl SessionSettingsStore for PromptSettings {
    fn retry_settings(&self) -> RetrySettings {
        self.retry.clone()
    }
    fn image_auto_resize(&self) -> bool {
        self.image_auto_resize
    }
}

/// TS `auth-guidance.ts` over an injected docs path.
fn provider_login_help(docs_path: &str) -> String {
    [
        "Use /login to log into a provider via OAuth or API key. See:".to_string(),
        format!("  {}/providers.md", docs_path),
        format!("  {}/models.md", docs_path),
    ]
    .join("\n")
}

/// TS `formatNoModelSelectedMessage` / `formatNoApiKeyFoundMessage` over
/// the harness `modelAuth` mode (the `_modelRuntime` auth surface).
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

/// Recording stand-in for `_extensionRunner`: commands, input handlers,
/// and the extension event/error logs.
struct PromptExtensionHost {
    events: Mutex<Vec<Value>>,
    errors: Mutex<Vec<Value>>,
    command_calls: Mutex<Vec<Value>>,
    input_events: Mutex<Vec<Value>>,
    /// name → (mode, errorText).
    commands: HashMap<String, (String, String)>,
    /// `{ mode, text?, images? }` — `None` = no input handlers.
    input_handler: Option<Value>,
}

impl PromptExtensionHost {
    fn input_action(&self) -> InputAction {
        let Some(handler) = &self.input_handler else { return InputAction::Continue };
        match handler.get("mode").and_then(Value::as_str).unwrap_or("continue") {
            "handled" => InputAction::Handled,
            "transform" => InputAction::Transform {
                text: handler.get("text").and_then(Value::as_str).map(str::to_string),
                images: handler.get("images").and_then(Value::as_array).cloned(),
            },
            _ => InputAction::Continue,
        }
    }
}

impl ExtensionHost for PromptExtensionHost {
    fn has_handlers(&self, event: &str) -> bool {
        event == "input" && self.input_handler.is_some()
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
    fn get_command(&self, name: &str) -> bool {
        self.commands.contains_key(name)
    }
    fn execute_command<'a>(&'a self, name: &'a str, args: &'a str) -> BoxFuture<'a, Result<(), String>> {
        self.command_calls
            .lock()
            .expect("command calls")
            .push(json!({ "name": name, "args": args }));
        let (mode, error_text) = self.commands.get(name).cloned().unwrap_or_default();
        let result = if mode == "throw" { Err(error_text) } else { Ok(()) };
        Box::pin(async move { result })
    }
    fn emit_input<'a>(&'a self, event: Value) -> BoxFuture<'a, InputAction> {
        self.input_events.lock().expect("input events").push(event);
        let action = self.input_action();
        Box::pin(async move { action })
    }
    fn invalidate(&self, _message: &str) {}
}

// ============================================================================
// Harness entry
// ============================================================================

pub fn run_agent_session_prompt_harness(args: &Value) -> Result<Value, String> {
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
    let auth_mode = model_auth_in
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("ok")
        .to_string();
    if !matches!(auth_mode.as_str(), "ok" | "noKey" | "oauth") {
        return Err("invalid modelAuth".to_string());
    }
    let docs_path = config
        .get("docsPath")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_DOCS_PATH)
        .to_string();
    let system_prompt = match config.get("systemPrompt") {
        None | Some(Value::Null) => None,
        Some(Value::String(prompt)) => Some(prompt.clone()),
        Some(_) => return Err("invalid systemPrompt".to_string()),
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
    let mode_field = |key: &str| -> Result<Option<String>, String> {
        match config.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(mode)) => Ok(Some(mode.clone())),
            _ => Err(format!("invalid {}", key)),
        }
    };
    let steering_mode = mode_field("steeringMode")?;
    let follow_up_mode = mode_field("followUpMode")?;
    if let Some(mode) = &steering_mode {
        if pi_agent::types::QueueMode::parse(mode).is_none() {
            return Err("invalid steeringMode".to_string());
        }
    }
    if let Some(mode) = &follow_up_mode {
        if pi_agent::types::QueueMode::parse(mode).is_none() {
            return Err("invalid followUpMode".to_string());
        }
    }
    let commands_in = args.get("commands").cloned().unwrap_or(json!({}));
    if !commands_in.is_object() {
        return Err("invalid commands".to_string());
    }
    let mut commands: HashMap<String, (String, String)> = HashMap::new();
    for (name, spec) in commands_in.as_object().expect("commands object") {
        let obj = spec.as_object().ok_or_else(|| "invalid commands".to_string())?;
        let mode = obj.get("mode").and_then(Value::as_str).unwrap_or("ok").to_string();
        if !matches!(mode.as_str(), "ok" | "throw") {
            return Err("invalid commands".to_string());
        }
        let error_text = obj.get("errorText").and_then(Value::as_str).unwrap_or("").to_string();
        commands.insert(name.clone(), (mode, error_text));
    }
    let input_handler = match args.get("inputHandler") {
        None | Some(Value::Null) => None,
        Some(handler) if handler.is_object() => {
            let mode = handler.get("mode").and_then(Value::as_str).unwrap_or("continue");
            if !matches!(mode, "continue" | "handled" | "transform") {
                return Err("invalid inputHandler".to_string());
            }
            Some(handler.clone())
        }
        _ => return Err("invalid inputHandler".to_string()),
    };
    let templates_in = args.get("templates").cloned().unwrap_or(json!([]));
    if !templates_in.is_array() {
        return Err("invalid templates".to_string());
    }
    let templates: Vec<PromptTemplate> = templates_in
        .as_array()
        .expect("templates array")
        .iter()
        .map(|t| serde_json::from_value(t.clone()).map_err(|_| "invalid templates".to_string()))
        .collect::<Result<_, String>>()?;
    let skills_in = args.get("skills").cloned().unwrap_or(json!([]));
    if !skills_in.is_array() {
        return Err("invalid skills".to_string());
    }
    let skills: Vec<SkillSpec> = skills_in
        .as_array()
        .expect("skills array")
        .iter()
        .map(|s| {
            let obj = s.as_object().ok_or_else(|| "invalid skills".to_string())?;
            Ok(SkillSpec {
                name: obj.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                file_path: obj.get("filePath").and_then(Value::as_str).unwrap_or("").to_string(),
                base_dir: obj.get("baseDir").and_then(Value::as_str).unwrap_or("").to_string(),
                content: obj.get("content").and_then(Value::as_str).unwrap_or("").to_string(),
                read_fails: obj.get("readFails").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect::<Result<_, String>>()?;
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
    // Agent state is seeded from the session projection (the app wires the
    // same objects; the TS harness passes them via initialState).
    let projected = session_manager
        .build_session_projection()
        .messages
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect::<Vec<Value>>();

    // Fake provider popping providerTurns[i] per stream call; each call's
    // llm context is recorded as promptsSentToAgent (mirrors the TS fake).
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
        // `null` models the "no model selected" validation path (TS pins
        // `agent.state.model = undefined` post-construction).
        model: config.get("model").filter(|m| !m.is_null()).cloned().unwrap_or(Value::Null),
        thinking_level: "off".to_string(),
        steering_mode: steering_mode
            .as_deref()
            .and_then(pi_agent::types::QueueMode::parse)
            .unwrap_or_default(),
        follow_up_mode: follow_up_mode
            .as_deref()
            .and_then(pi_agent::types::QueueMode::parse)
            .unwrap_or_default(),
        stream_fn,
        tool_execution: Default::default(),
        clock: Arc::clone(&clock),
    });

    let extension_host = Arc::new(PromptExtensionHost {
        events: Mutex::new(Vec::new()),
        errors: Mutex::new(Vec::new()),
        command_calls: Mutex::new(Vec::new()),
        input_events: Mutex::new(Vec::new()),
        commands,
        input_handler,
    });

    let mut session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager,
        settings: Arc::new(PromptSettings { retry, image_auto_resize }),
        extension_host: Arc::clone(&extension_host) as Arc<dyn ExtensionHost>,
        clock: Arc::clone(&clock),
        nested_tool_calls: None,
        cwd: HARNESS_CWD.to_string(),
        custom_tools: Vec::new(),
        initial_active_tool_names: None,
        uses_default_tools: false,
        default_tool_modifiers: Vec::new(),
        allowed_tool_names: None,
        excluded_tool_names: None,
        base_tools_override: None,
        model_auth: Some(Arc::new(FixedModelAuth {
            mode: auth_mode,
            docs_path: docs_path.clone(),
        })),
        skills,
        prompt_templates: templates,
        system_prompt,
        model_lookup: None,
        scoped_models: Vec::new(),
        model_routes: None,
        physical_model_lookup: None,
        sleeper: None,
    });

    // Public listener collector (the TS side deep-clones each event at
    // collection time).
    let session_events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = Arc::clone(&session_events);
        session.subscribe(Box::new(move |event: &Value| {
            sink.lock().expect("session events").push(event.clone());
        }));
    }

    let dispositions: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));

    pi_agent::run_current_thread(async {
        let mut results: Vec<Value> = Vec::with_capacity(script.len());
        let normalize = |message: &str| message.replace(&docs_path, "<docs>");
        for op in &script {
            let outcome = run_op(&mut session, op, &dispositions).await;
            results.push(match outcome {
                Ok(value) => value,
                Err(message) => json!({ "thrown": normalize(&message) }),
            });
        }

        let queue_state = json!({
            "steering": session.steering_messages(),
            "followUp": session.follow_up_messages(),
            "isIdle": session.is_idle(),
            "pendingCustomMessages": session.pending_custom_messages(),
        });

        Ok(json!({
            "result": {
                "results": results,
                "dispositions": dispositions.lock().expect("dispositions").clone(),
                "sessionEvents": session_events.lock().expect("session events").clone(),
                "extensionEvents": extension_host.events.lock().expect("extension log").clone(),
                "extensionErrors": extension_host.errors.lock().expect("extension errors").clone(),
                "commandCalls": extension_host.command_calls.lock().expect("command calls").clone(),
                "inputEvents": extension_host.input_events.lock().expect("input events").clone(),
                "queueState": queue_state,
                "promptsSentToAgent": prompts_sent.lock().expect("prompts sent").clone(),
                "entries": session.session_manager.get_entries(),
            }
        }))
    })
}

/// Run one script op; `Err` becomes the op's `{thrown}` entry.
async fn run_op(
    session: &mut AgentSession,
    op: &Value,
    dispositions: &Arc<Mutex<Vec<Value>>>,
) -> Result<Value, String> {
    let obj = op
        .as_object()
        .ok_or_else(|| "ops entries must be objects".to_string())?;
    let name = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing op".to_string())?;
    let str_field = |key: &str| -> Result<String, String> {
        obj.get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("missing {}", key))
    };
    let images_field = |options: &Value| -> Result<Option<Vec<Value>>, String> {
        match options.get("images") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(images)) => Ok(Some(images.clone())),
            _ => Err("invalid images".to_string()),
        }
    };
    match name {
        "prompt" => {
            let text = str_field("text")?;
            let options = obj.get("options").cloned().unwrap_or(json!({}));
            if !options.is_object() {
                return Err("invalid options".to_string());
            }
            let preflight = options.get("preflightTag").and_then(Value::as_str).map(|tag| {
                let sink = Arc::clone(dispositions);
                let tag = tag.to_string();
                Arc::new(move |disposition: &str| {
                    sink.lock()
                        .expect("dispositions")
                        .push(json!({ "tag": tag, "disposition": disposition }));
                }) as Arc<dyn Fn(&str) + Send + Sync>
            });
            if let Some(flag) = options.get("expandPromptTemplates") {
                if !flag.is_boolean() {
                    return Err("invalid expandPromptTemplates".to_string());
                }
            }
            if let Some(behavior) = options.get("streamingBehavior") {
                if !behavior.is_null() && !behavior.is_string() {
                    return Err("invalid streamingBehavior".to_string());
                }
            }
            if let Some(source) = options.get("source") {
                if !source.is_null() && !source.is_string() {
                    return Err("invalid source".to_string());
                }
            }
            let prompt_options = PromptOptions {
                expand_prompt_templates: options
                    .get("expandPromptTemplates")
                    .and_then(Value::as_bool),
                images: images_field(&options)?,
                streaming_behavior: options
                    .get("streamingBehavior")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                source: options.get("source").and_then(Value::as_str).map(str::to_string),
                preflight_result: preflight,
            };
            session.prompt(&text, prompt_options).await?;
            Ok(Value::Null)
        }
        "steer" | "followUp" => {
            let text = str_field("text")?;
            let images = images_field(op)?;
            let source = match obj.get("source") {
                None | Some(Value::Null) => None,
                Some(Value::String(source)) => Some(source.clone()),
                _ => return Err("invalid source".to_string()),
            };
            let disposition = if name == "steer" {
                session.steer(&text, images.as_deref(), source.as_deref()).await?
            } else {
                session.follow_up(&text, images.as_deref(), source.as_deref()).await?
            };
            Ok(Value::String(disposition.to_string()))
        }
        "sendCustomMessage" => {
            let custom_type = str_field("customType")?;
            let content = obj
                .get("content")
                .cloned()
                .ok_or_else(|| "missing content".to_string())?;
            let display = obj
                .get("display")
                .and_then(Value::as_bool)
                .ok_or_else(|| "missing display".to_string())?;
            let details = obj.get("details").cloned();
            let trigger_turn = match obj.get("triggerTurn") {
                Some(Value::Null) | None => None,
                Some(v) => Some(v.as_bool().ok_or_else(|| "invalid triggerTurn".to_string())?),
            };
            let deliver_as = match obj.get("deliverAs") {
                Some(Value::Null) | None => None,
                Some(v) => {
                    Some(v.as_str().ok_or_else(|| "invalid deliverAs".to_string())?.to_string())
                }
            };
            session
                .send_custom_message(
                    &custom_type,
                    content,
                    display,
                    details,
                    trigger_turn,
                    deliver_as.as_deref(),
                )
                .await?;
            Ok(Value::Null)
        }
        "sendUserMessage" => {
            let content = obj
                .get("content")
                .cloned()
                .ok_or_else(|| "missing content".to_string())?;
            if !content.is_string() && !content.is_array() {
                return Err("invalid content".to_string());
            }
            let deliver_as = match obj.get("deliverAs") {
                Some(Value::Null) | None => None,
                Some(v) => {
                    Some(v.as_str().ok_or_else(|| "invalid deliverAs".to_string())?.to_string())
                }
            };
            let expand = match obj.get("expandPromptTemplates") {
                Some(Value::Null) | None => None,
                Some(v) => Some(v.as_bool().ok_or_else(|| "invalid expandPromptTemplates".to_string())?),
            };
            session.send_user_message(&content, deliver_as.as_deref(), expand).await?;
            Ok(Value::Null)
        }
        "setStreaming" => {
            let flag = obj
                .get("value")
                .and_then(Value::as_bool)
                .ok_or_else(|| "missing value".to_string())?;
            session.set_agent_run_active(flag);
            Ok(Value::Null)
        }
        other => Err(format!("unknown agent-session-prompt op: {}", other)),
    }
}
