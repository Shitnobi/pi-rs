//! Parity-harness entry for the `agent-session-tools` protocol: the tool
//! registry & loadout slice of `pi_session::AgentSession` (TS
//! `agent-session.ts` `_refreshToolRegistry`, `_applyToolLoadout`,
//! exposure/allowlist algebra, `_rebuildSystemPrompt` glue,
//! `_restoreToolsFromTranscript`, the hidden-declarations and
//! forced-prompt projections, and the constructor's `_buildRuntime`
//! slice). TS mirror: parity/ts-harness.js `agent-session-tools` entry.
//!
//! args:
//! ```json
//! {
//!   "nowMs":  number,            // pinned Date.now / clock
//!   "config": { "usesDefaultTools"?, "defaultToolModifiers"?,
//!               "allowedToolNames"?, "excludedToolNames"?,
//!               "initialActiveToolNames"? },
//!   "baseTools":       { "<key>": { "name", "label"?, "description", "parameters"? } },
//!                      // → baseToolsOverride (the unported builtin
//!                      //   definitions; required, may be empty)
//!   "customTools":     [ ToolDefinitionJson ],
//!   "extensionTools":  [ { "definition": ToolDefinitionJson, "sourceInfo": SourceInfoJson } ],
//!                      // visible to the registry only AFTER construction
//!                      //   (the TS constructor builds a real, empty
//!                      //   ExtensionRunner; the fake is swapped after)
//!   "entries":  [...],           // session entries, header FIRST
//!   "messages": [...],           // pool referenced by transformContext
//!   "ops": [ { "op": ... }, ... ]
//! }
//! ```
//!
//! ToolDefinitionJson: `{ name, description, label?, parameters?,
//! promptSnippet?, promptGuidelines?, exposure?, namespace?, annotations?,
//! reachesTools?, defaultActive?, prepareLoadout? }` where `prepareLoadout`
//! is `{ "kind": "changes", "descriptions"?, "hiddenDeclarations"? }` or
//! `{ "kind": "throw", "message" }`.
//!
//! Ops and their logged results (`results`):
//! - `{op:"refreshRegistry", activeToolNames?, includeAllExtensionTools?}` → `null`
//! - `{op:"setActiveTools", names}` → `null`
//! - `{op:"restoreFromTranscript", modifiers?}` → `null`
//! - `{op:"transformContext", messageIndices}` → transformed messages
//! - `{op:"setForcedPrompt", text|null}` → `null` (any-cast `_runSystemPromptOptions`)
//! - `{op:"getAllTools"}` → ToolInfo[]
//! - `{op:"getCallableToolNames"}` → string[]
//!
//! Result:
//! ```json
//! { "activeTools", "declaredTools", "hiddenDeclarations",
//!   "systemPromptOptions", "results", "extensionErrors" }
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pi_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture};
use pi_agent::{Agent, AgentOptions, Clock, FixedClock};
use pi_core::session_manager::{Generators, SessionManager};
use pi_core::settings_manager::RetrySettings;
use pi_session::agent_session::tools::{
    RegisteredTool, ToolDefinition,
};
use pi_session::agent_session::{
    AgentSession, AgentSessionConfig, ExtensionHost, SessionSettingsStore,
};

const HARNESS_CWD: &str = "/proj";

// ============================================================================
// Fakes
// ============================================================================

struct FixedSettings {
    retry: RetrySettings,
}

impl SessionSettingsStore for FixedSettings {
    fn retry_settings(&self) -> RetrySettings {
        self.retry.clone()
    }
}

/// Recording stand-in for `_extensionRunner`: extension tools become
/// visible only once `installed` (the TS constructor runs a real, empty
/// runner and the fake is swapped in afterwards).
struct ToolsExtensionHost {
    extension_tools: Vec<RegisteredTool>,
    installed: AtomicBool,
    errors: Mutex<Vec<Value>>,
}

impl ExtensionHost for ToolsExtensionHost {
    fn has_handlers(&self, _event: &str) -> bool {
        false
    }
    fn emit<'a>(&'a self, _event: Value) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
    fn emit_message_end<'a>(&'a self, _event: Value) -> BoxFuture<'a, Option<Value>> {
        Box::pin(async { None })
    }
    fn emit_error(&self, error: Value) {
        if self.installed.load(Ordering::SeqCst) {
            self.errors.lock().expect("extension errors").push(error);
        }
    }
    fn invalidate(&self, _message: &str) {}
    fn get_all_registered_tools(&self) -> Vec<RegisteredTool> {
        if self.installed.load(Ordering::SeqCst) {
            self.extension_tools.clone()
        } else {
            Vec::new()
        }
    }
}

/// Base-tool stand-in (TS: a plain AgentTool object whose execute throws).
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
        _on_update: Option<&'a AgentToolUpdateCallback<'a>>,
    ) -> BoxFuture<'a, Result<AgentToolResult, String>> {
        Box::pin(async {
            Err("agent-session-tools harness tool execute must never be called".to_string())
        })
    }
}

// ============================================================================
// Parsing (mirrors the TS buildDefinition validation)
// ============================================================================

const EXPOSURES: [&str; 5] = ["direct", "model-only", "codemode", "deferred", "hidden"];

fn parse_tool_definition(value: &Value) -> Result<ToolDefinition, String> {
    if !value.is_object() {
        return Err("customTools entries must be objects".to_string());
    }
    if let Some(exposure) = value.get("exposure") {
        match exposure.as_str() {
            Some(e) if EXPOSURES.contains(&e) => {}
            _ => return Err("invalid exposure".to_string()),
        }
    }
    if let Some(prepare) = value.get("prepareLoadout") {
        let ok = prepare.get("kind").and_then(Value::as_str) == Some("changes")
            || prepare.get("kind").and_then(Value::as_str) == Some("throw");
        if !ok {
            return Err("invalid prepareLoadout".to_string());
        }
    }
    serde_json::from_value::<ToolDefinition>(value.clone())
        .map_err(|_| "invalid tool definition".to_string())
}

fn parse_source_info(value: &Value) -> Result<pi_core::prompt_templates::SourceInfo, String> {
    serde_json::from_value(value.clone()).map_err(|_| "invalid sourceInfo".to_string())
}

// ============================================================================
// Harness entry
// ============================================================================

pub fn run_agent_session_tools_harness(args: &Value) -> Result<Value, String> {
    // Validation order mirrors the TS entry exactly.
    let now_ms = args.get("nowMs").and_then(Value::as_f64).ok_or_else(|| "missing nowMs".to_string())?;
    let config = args.get("config").cloned().unwrap_or(json!({}));
    if !config.is_object() {
        return Err("invalid config".to_string());
    }
    let base_tools = args
        .get("baseTools")
        .filter(|v| v.is_object())
        .ok_or_else(|| "missing baseTools".to_string())?
        .as_object()
        .expect("baseTools object")
        .clone();
    let custom_tools_in = args.get("customTools").cloned().unwrap_or(json!([]));
    if !custom_tools_in.is_array() {
        return Err("invalid customTools".to_string());
    }
    let extension_tools_in = args.get("extensionTools").cloned().unwrap_or(json!([]));
    if !extension_tools_in.is_array() {
        return Err("invalid extensionTools".to_string());
    }
    let entries = args
        .get("entries")
        .and_then(Value::as_array)
        .filter(|e| !e.is_empty())
        .filter(|e| e.first().and_then(|h| h.get("type")).and_then(Value::as_str) == Some("session"))
        .ok_or_else(|| "missing entries".to_string())?
        .clone();
    let messages = args.get("messages").cloned().unwrap_or(json!([]));
    if !messages.is_array() {
        return Err("invalid messages".to_string());
    }
    let messages = messages.as_array().expect("messages array").clone();
    let ops = args
        .get("ops")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing ops".to_string())?
        .clone();

    let base_override: Vec<(String, Arc<dyn AgentTool>)> = base_tools
        .iter()
        .map(|(key, spec)| {
            if !spec.is_object() {
                return Err("baseTools entries must be objects".to_string());
            }
            Ok((key.clone(), Arc::new(BaseJsonTool { spec: spec.clone() }) as Arc<dyn AgentTool>))
        })
        .collect::<Result<_, String>>()?;
    let custom_tools: Vec<ToolDefinition> = custom_tools_in
        .as_array()
        .expect("customTools array")
        .iter()
        .map(parse_tool_definition)
        .collect::<Result<_, String>>()?;
    let extension_tools: Vec<RegisteredTool> = extension_tools_in
        .as_array()
        .expect("extensionTools array")
        .iter()
        .map(|t| {
            if !t.is_object() {
                return Err("extensionTools entries must be objects".to_string());
            }
            let definition = t
                .get("definition")
                .ok_or_else(|| "missing definition".to_string())?;
            let source_info = t
                .get("sourceInfo")
                .ok_or_else(|| "missing sourceInfo".to_string())?;
            Ok(RegisteredTool {
                definition: parse_tool_definition(definition)?,
                source_info: parse_source_info(source_info)?,
            })
        })
        .collect::<Result<_, String>>()?;

    let config_obj = config.as_object().expect("config object");
    let uses_default_tools =
        config_obj.get("usesDefaultTools").and_then(Value::as_bool).unwrap_or(false);
    let default_tool_modifiers: Vec<String> = config_obj
        .get("defaultToolModifiers")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default();
    let str_list = |key: &str| -> Option<Vec<String>> {
        config_obj.get(key).and_then(Value::as_array).map(|a| {
            a.iter().filter_map(Value::as_str).map(String::from).collect()
        })
    };
    let initial_active_tool_names = str_list("initialActiveToolNames");
    let allowed_tool_names = str_list("allowedToolNames");
    let excluded_tool_names = str_list("excludedToolNames");

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
    let uuid_v7 = Arc::clone(&uuid_v4);
    let now_gen = Arc::new(move || now_ms_i);
    let gen = Generators::with_gens(uuid_v7, uuid_v4, now_gen);

    let stream_fn: Arc<pi_agent::LoopStreamFn> = Arc::new(|_model, _context, _options| {
        Box::pin(async { Err("agent-session-tools harness streamFn must never be called".to_string()) })
    });
    let agent = Agent::new(AgentOptions {
        messages: Vec::new(),
        tools: Vec::new(),
        system_prompt: None,
        model: pi_agent::default_model(),
        thinking_level: "off".to_string(),
        steering_mode: Default::default(),
        follow_up_mode: Default::default(),
        stream_fn,
        tool_execution: Default::default(),
        clock: Arc::clone(&clock),
    });

    let session_manager = SessionManager::in_memory(HARNESS_CWD, None, entries, &gen)?;
    let extension_host = Arc::new(ToolsExtensionHost {
        extension_tools,
        installed: AtomicBool::new(false),
        errors: Mutex::new(Vec::new()),
    });

    let mut session = AgentSession::new(AgentSessionConfig {
        model_auth: None,
        skills: Vec::new(),
        prompt_templates: Vec::new(),
        agent,
        session_manager,
        settings: Arc::new(FixedSettings {
            retry: RetrySettings {
                enabled: true,
                max_retries: 3.0,
                base_delay_ms: 2000.0,
                max_agent_delay_ms: 300000.0,
            },
        }),
        extension_host: Arc::clone(&extension_host) as Arc<dyn ExtensionHost>,
        clock: Arc::clone(&clock),
        nested_tool_calls: None,
        cwd: HARNESS_CWD.to_string(),
        custom_tools,
        initial_active_tool_names: initial_active_tool_names.clone(),
        uses_default_tools,
        default_tool_modifiers: default_tool_modifiers.clone(),
        allowed_tool_names,
        excluded_tool_names,
        base_tools_override: Some(base_override),
    });

    // The fake runner is installed after construction (the TS constructor
    // refreshes against a real, empty ExtensionRunner).
    extension_host.installed.store(true, Ordering::SeqCst);

    pi_agent::run_current_thread(async {
        let mut results: Vec<Value> = Vec::with_capacity(ops.len());
        for op in &ops {
            let obj = op
                .as_object()
                .ok_or_else(|| "ops entries must be objects".to_string())?;
            let name = obj
                .get("op")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing op".to_string())?;
            let mut value = Value::Null;
            match name {
                "refreshRegistry" => {
                    let active = match obj.get("activeToolNames") {
                        Some(Value::Null) | None => None,
                        Some(v) => Some(
                            v.as_array()
                                .ok_or_else(|| "invalid activeToolNames".to_string())?
                                .iter()
                                .filter_map(Value::as_str)
                                .map(String::from)
                                .collect::<Vec<String>>(),
                        ),
                    };
                    let include_all = obj
                        .get("includeAllExtensionTools")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    session.refresh_tool_registry(active, include_all);
                }
                "setActiveTools" => {
                    let names = obj
                        .get("names")
                        .and_then(Value::as_array)
                        .ok_or_else(|| "missing names".to_string())?
                        .iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect();
                    session.set_active_tools_by_name(names);
                }
                "restoreFromTranscript" => {
                    let modifiers: Vec<String> = obj
                        .get("modifiers")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                        .unwrap_or_default();
                    session.restore_tools_from_transcript(&modifiers);
                }
                "transformContext" => {
                    let indices = obj
                        .get("messageIndices")
                        .and_then(Value::as_array)
                        .ok_or_else(|| "missing messageIndices".to_string())?;
                    let mut input: Vec<Value> = Vec::with_capacity(indices.len());
                    for index in indices {
                        let i = index.as_f64().filter(|i| *i >= 0.0).ok_or_else(|| {
                            "invalid messageIndex".to_string()
                        })? as usize;
                        if i >= messages.len() {
                            return Err("invalid messageIndex".to_string());
                        }
                        input.push(messages[i].clone());
                    }
                    value = Value::Array(session.transform_context(input).await);
                }
                "setForcedPrompt" => {
                    let text = match obj.get("text") {
                        Some(Value::Null) | None => None,
                        Some(v) => Some(
                            v.as_str()
                                .ok_or_else(|| "invalid text".to_string())?
                                .to_string(),
                        ),
                    };
                    session.set_forced_prompt(text);
                }
                "getAllTools" => {
                    value = Value::Array(session.get_all_tools());
                }
                "getCallableToolNames" => {
                    value = Value::Array(
                        session.get_callable_tool_names().into_iter().map(Value::String).collect(),
                    );
                }
                other => return Err(format!("unknown agent-session-tools op: {}", other)),
            }
            results.push(value);
        }

        let declared_tools: Vec<Value> = session
            .agent
            .tools()
            .iter()
            .map(|t| {
                json!({
                    "name": t.name(),
                    "description": t.description(),
                    "parameters": t.parameters(),
                })
            })
            .collect();

        Ok(json!({
            "result": {
                "activeTools": session.get_active_tool_names(),
                "declaredTools": declared_tools,
                "hiddenDeclarations": session.hidden_declarations(),
                "systemPromptOptions": session.system_prompt_options(),
                "results": results,
                "extensionErrors": extension_host.errors.lock().expect("extension errors").clone(),
            }
        }))
    })
}
