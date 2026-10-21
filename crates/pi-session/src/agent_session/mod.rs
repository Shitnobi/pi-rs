//! `AgentSession` facade + config (agent-session.ts `class AgentSession`,
//! construction slice). The event-handling slice lives in
//! [`super::event_handler`]; event shapes in [`super::events`].
//!
//! Tranche 8 ports only what the events slice needs; tool registry, model
//! runtime, cache warmer, and resource loader arrive in tranches 9–19
//! (marked `TODO(tranche N)`).

pub mod compaction;
pub mod bash;
pub mod runtime;
pub mod services;
pub mod events;
pub mod model;
pub mod event_handler;
pub mod prompt;
pub mod retry;
pub mod run;
pub mod stats;
pub mod system_prompt;
pub mod tree;
pub mod tools;
pub mod extension_runner;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use pi_agent::types::BoxFuture;
use pi_agent::{Agent, Clock};
use pi_agent::host::AbortHandle;
use pi_core::compaction::{default_compaction_settings, CompactionSettings};
use pi_core::session_manager::SessionManager;
use pi_core::settings_manager::RetrySettings;
pub use events::{MsgId, SessionAgentEvent};

/// TS `settingsManager.getRetrySettings()` seam — the settings the events
/// slice reads. The full settings manager lands with the prompt pipeline
/// (tranche 10+).
pub trait SessionSettingsStore: Send + Sync {
    fn retry_settings(&self) -> RetrySettings;
    /// TS `settingsManager.getImageAutoResize()` (default `true`).
    fn image_auto_resize(&self) -> bool {
        true
    }
    /// TS `settingsManager.getCompactionSettings(model?)`.
    fn compaction_settings(&self, _model: Option<(&str, &str)>) -> CompactionSettings {
        default_compaction_settings()
    }
    /// TS `settingsManager.setCompactionEnabled(enabled)` (default no-op;
    /// the harness stores implement it when a case exercises the setter).
    fn set_compaction_enabled(&self, _enabled: bool) {}
    /// TS `settingsManager.setDefaultModelAndProvider(provider, modelId)`.
    fn set_default_model_and_provider(&self, _provider: &str, _model_id: &str) {}
    /// TS `settingsManager.getEnabledModels()` (`None` = undefined).
    fn get_enabled_models(&self) -> Option<Vec<String>> {
        None
    }
    /// TS `settingsManager.setEnabledModels(patterns)`.
    fn set_enabled_models(&self, _patterns: Vec<String>) {}
    /// TS `settingsManager.setDefaultThinkingLevel(level)`.
    fn set_default_thinking_level(&self, _level: &str) {}
    /// TS `settingsManager.getModelThinkingLevel(provider, modelId)`.
    fn get_model_thinking_level(&self, _provider: &str, _model_id: &str) -> Option<String> {
        None
    }
    /// TS `settingsManager.getDefaultThinkingLevel()`.
    fn get_default_thinking_level(&self) -> Option<String> {
        None
    }
    /// TS `settingsManager.getShellCommandPrefix()` (bash slice).
    fn shell_command_prefix(&self) -> Option<String> {
        None
    }
    /// TS `settingsManager.getShellPath()` (bash slice).
    fn shell_path(&self) -> Option<String> {
        None
    }
    /// TS `settingsManager.getBranchSummarySettings()` (tree slice).
    fn branch_summary_settings(&self) -> pi_core::settings_manager::BranchSummarySettings {
        pi_core::settings_manager::BranchSummarySettings {
            reserve_tokens: 16384.0,
            skip_prompt: false,
        }
    }
}

/// TS `_modelRuntime` auth surface the prompt pipeline validates against
/// (`hasConfiguredAuth` / `checkAuth` / `isUsingOAuth`) plus the
/// docs-path-bearing error messages (`auth-guidance.ts` — the paths make
/// the texts environment-dependent, so the harness injects them).
/// TS `_modelRuntime.getAuth(model)` result (`{ auth: { baseUrl?,
/// apiKey?, headers? }, env? }` — the slice the summarizer consumes).
#[derive(Clone)]
pub struct RequestAuth {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub headers: Option<Value>,
    pub env: Option<Value>,
}

pub trait ModelAuthStore: Send + Sync {
    /// TS `getAuth(model, { signal })` — `None` models an absent result or
    /// a throw (the TS catch returns the bare model).
    fn get_auth(&self, _model: &Value) -> Option<RequestAuth> {
        None
    }
    fn has_configured_auth(&self, provider: &str) -> bool;
    /// TS `(await checkAuth(provider)) !== undefined`.
    fn check_auth(&self, provider: &str) -> bool;
    fn is_using_oauth(&self, provider: &str) -> bool;
    fn no_model_selected_message(&self) -> String;
    fn no_api_key_message(&self, provider: &str) -> String;
}

/// Stand-in for `ExtensionRunner` (plan §4 `ExtensionHost`): the extension
/// emission surface the events slice uses. The real runner is Phase 4+;
/// parity tests inject a recording fake.
pub trait ExtensionHost: Send + Sync {
    /// TS `hasHandlers(event)`.
    fn has_handlers(&self, event: &str) -> bool;
    /// TS `emit({ type: "session_before_switch", ... })` → the
    /// `SessionBeforeSwitchResult` (`{ cancel? }`). Default: no handlers.
    fn emit_session_before_switch<'a>(
        &'a self,
        _event: Value,
    ) -> pi_agent::types::BoxFuture<'a, Option<Value>> {
        Box::pin(async { None })
    }
    /// TS `emit({ type: "session_before_fork", ... })` → the
    /// `SessionBeforeForkResult` (`{ cancel? }`). Default: no handlers.
    fn emit_session_before_fork<'a>(
        &'a self,
        _event: Value,
    ) -> pi_agent::types::BoxFuture<'a, Option<Value>> {
        Box::pin(async { None })
    }
    /// TS `runner.createCommandContext()` — the base of
    /// `createReplacedSessionContext`. Default: empty object.
    fn command_context(&self) -> Value {
        serde_json::json!({})
    }
    /// TS `emit(event)` — awaited before public listeners.
    fn emit<'a>(&'a self, event: Value) -> BoxFuture<'a, ()>;
    /// TS `emitMessageEnd(event)` — may return a replacement message.
    fn emit_message_end<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>>;
    /// TS `emitError(error)`.
    fn emit_error(&self, error: Value);
    /// TS `runner.invalidate(message)` (dispose; default: no-op).
    fn invalidate(&self, _message: &str) {}
    /// TS `runner.getCommand(name) !== undefined` (prompt slice).
    fn get_command(&self, _name: &str) -> bool {
        false
    }
    /// TS `command.handler(args, ctx)` — `Err` models a throw.
    fn execute_command<'a>(
        &'a self,
        _name: &'a str,
        _args: &'a str,
    ) -> pi_agent::types::BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("command not found".to_string()) })
    }
    /// TS `runner.emitInput(event)` → the input-event result.
    fn emit_input<'a>(
        &'a self,
        _event: Value,
    ) -> pi_agent::types::BoxFuture<'a, prompt::InputAction> {
        Box::pin(async { prompt::InputAction::Continue })
    }
    /// TS `runner.getAllRegisteredTools()` — the tools-slice seam (plan §4).
    /// Default: none (the events-slice fakes register no tools).
    fn get_all_registered_tools(&self) -> Vec<tools::RegisteredTool> {
        Vec::new()
    }
    /// TS `runner.emitBeforeAgentStart(trigger, systemPromptOptions)` →
    /// `(handlerMessages, systemPromptOptions, setActiveToolsRequests)`
    /// (the options Value the handlers may have edited,
    /// `forceSystemPrompt` included; the `ctx.setActiveTools(names)`
    /// calls handlers made, applied by `_prepareRun` after the emission —
    /// TS applies them mid-handler, which is unobservable through the
    /// threaded options clone since the live loadout is only read after).
    /// Default: passthrough.
    fn emit_before_agent_start<'a>(
        &'a self,
        _trigger: Value,
        options: Value,
    ) -> pi_agent::types::BoxFuture<'a, (Vec<Value>, Value, Vec<Vec<String>>)> {
        Box::pin(async move { (Vec::new(), options, Vec::new()) })
    }
    /// TS `emit({ type: "session_before_compact", ... })` → the
    /// `SessionBeforeCompactResult` (`{ cancel?, compaction? }`).
    /// Default: no handlers (the result of an emission to zero handlers).
    fn emit_session_before_compact<'a>(
        &'a self,
        _event: Value,
    ) -> pi_agent::types::BoxFuture<'a, Option<Value>> {
        Box::pin(async { None })
    }
    /// TS `emit({ type: "session_before_tree", ... })` → the
    /// `SessionBeforeTreeResult` (`{ cancel?, summary?, customInstructions?,
    /// replaceInstructions?, label? }`). Default: no handlers.
    fn emit_session_before_tree<'a>(
        &'a self,
        _event: Value,
    ) -> pi_agent::types::BoxFuture<'a, Option<Value>> {
        Box::pin(async { None })
    }
    /// TS `snapshotEventHandlers(type).length` for boundary events —
    /// the handler count driving [`AgentSession::emit_boundary`].
    fn boundary_handlers(&self, _event_type: &str) -> usize {
        0
    }
    /// One boundary handler invocation (registration order). `None` =
    /// the handler returned `undefined`; `Some(value)` is the TS
    /// `BoundaryResult` (`{entries?, continue?}`).
    fn emit_boundary_handler<'a>(
        &'a self,
        _index: usize,
        _event: Value,
    ) -> pi_agent::types::BoxFuture<'a, Option<Value>> {
        Box::pin(async { None })
    }
}

/// Stand-in for the `NestedToolCallRunner` field (`_nestedToolCalls`):
/// `takeRecord`/`clear` are all `_handleAgentEvent` uses. The record is
/// passed as its JSON projection (`{ calls?, usage? }`,
/// TS `NestedCallSummary`) because the handler only forwards `calls` onto
/// the message and combines `usage`.
pub trait NestedCallsStore: Send + Sync {
    fn take_record(&self, tool_call_id: &str) -> Option<Value>;
    fn clear(&self);
}

/// TS `AgentSessionConfig` — only the fields the events slice consumes.
/// TODO(tranche 9+): `customTools`, `resourceLoader`, `modelRuntime`,
/// `cacheWarmer`, tool allowlists, `extensionRunnerRef`, …
pub struct AgentSessionConfig {
    pub agent: Agent,
    pub session_manager: SessionManager,
    pub settings: Arc<dyn SessionSettingsStore>,
    pub extension_host: Arc<dyn ExtensionHost>,
    pub clock: Arc<dyn Clock>,
    pub nested_tool_calls: Option<Arc<dyn NestedCallsStore>>,
    pub cwd: String,
    /// TS `customTools` (ToolDefinitions; see [`tools`] for the
    /// prepareLoadout data-spec stand-in).
    pub custom_tools: Vec<tools::ToolDefinition>,
    /// TS `initialActiveToolNames`.
    pub initial_active_tool_names: Option<Vec<String>>,
    /// TS `usesDefaultTools`.
    pub uses_default_tools: bool,
    /// TS `defaultToolModifiers`.
    pub default_tool_modifiers: Vec<String>,
    /// TS `allowedToolNames` (`None` = the flag was absent).
    pub allowed_tool_names: Option<Vec<String>>,
    /// TS `excludedToolNames`.
    pub excluded_tool_names: Option<Vec<String>>,
    /// TS `baseToolsOverride` (key → AgentTool). The default builtin
    /// definitions are unported (see [`tools`] module docs), so production
    /// callers must pass an override until Phase 4+.
    pub base_tools_override: Option<Vec<(String, Arc<dyn pi_agent::AgentTool>)>>,
    /// TS `_modelRuntime` auth seam (prompt slice).
    pub model_auth: Option<Arc<dyn ModelAuthStore>>,
    /// TS `resourceLoader.getSkills()` (prompt slice; Phase 4 loader).
    pub skills: Vec<prompt::SkillSpec>,
    /// TS `resourceLoader.getPrompts().prompts`.
    pub prompt_templates: Vec<pi_core::prompt_templates::PromptTemplate>,
    /// TS `resourceLoader.getSystemPrompt()` (custom prompt; the default
    /// preamble/docs sections render host paths, so parity cases always
    /// pin one — see [`system_prompt`] docs).
    pub system_prompt: Option<String>,
    /// TS `_modelRuntime.getModel(provider, modelId)` — the `_recordSelection`
    /// seam (tranche 14 ports the full runtime).
    pub model_lookup: Option<Arc<dyn Fn(&str, &str) -> Option<Value> + Send + Sync>>,
    /// TS `sleep(delayMs, signal)` seam (plan §3): the retry backoff
    /// sleeper. `None` = the real tokio-time sleeper.
    pub sleeper: Option<Arc<dyn pi_agent::host::Sleeper>>,
    /// TS `config.scopedModels` (`--models` flag).
    pub scoped_models: Vec<model::ScopedModel>,
    /// TS `_modelRuntime` routing seam (model & thinking slice).
    pub model_routes: Option<Arc<dyn model::ModelRouteStore>>,
    /// TS `_modelRuntime.getPhysicalModel(provider, modelId)` — the
    /// `routedModel` getter seam (stats slice).
    pub physical_model_lookup: Option<Arc<dyn Fn(&str, &str) -> Option<Value> + Send + Sync>>,
}

/// A deferred `agent_settled` action (TS `_deferredSettledActions` holds
/// async closures re-invoking `prompt`/`sendCustomMessage`). Those callers
/// land with the run pipeline (tranche 11); until then the harness drives
/// the deterministic `Log` stand-in (plan §2 trap 8).
pub enum DeferredSettledAction {
    Log(String),
    /// A deferred re-invocation of `prompt` (TS pushes a closure).
    Prompt { text: String, options: prompt::PromptOptions },
    /// A deferred re-invocation of `sendCustomMessage`.
    SendCustomMessage {
        custom_type: String,
        content: Value,
        display: bool,
        details: Option<Value>,
        trigger_turn: Option<bool>,
        deliver_as: Option<String>,
    },
}

/// TS `AgentSessionEventListener`. `Sync` is a Rust-side requirement so
/// the session (and its listener list) can move onto other threads — the
/// retry harness holds a prompt run on a spawned task.
pub type SessionEventListener = Box<dyn FnMut(&Value) + Send + Sync>;

/// The mutable half of `AgentSession` (TS private fields).
struct SessionState {
    /// TS `_eventListeners` (shared — the bash slice's `onChunk` callback
    /// emits `bash_execution_update` from a `'static` executor future via
    /// [`AgentSession::emit_tap`]).
    listeners: Arc<Mutex<Vec<(usize, SessionEventListener)>>>,
    /// TS `_isAgentRunActive` (shared — see [`AgentSession::shared`]).
    is_agent_run_active: Arc<AtomicBool>,
    /// TS `_agentRunAbortRequested` (shared).
    agent_run_abort_requested: Arc<AtomicBool>,
    /// TS `_steeringMessages` — pending steering texts for UI display.
    steering_messages: Vec<String>,
    /// TS `_followUpMessages`.
    follow_up_messages: Vec<String>,
    /// TS `_pendingNextTurnMessages`.
    pending_next_turn_messages: Vec<Value>,
    /// TS `_pendingCustomMessages` — context-only custom messages queued
    /// during a run, flushed once the current turn's tool results are in.
    pending_custom_messages: Vec<Value>,
    /// TS `_bashAbortControllers` (shared — `abortBash()` is `&self` so
    /// scripted operations and the abort taps can fire it mid-execution).
    bash_signals: Arc<Mutex<Vec<pi_tools::shell::AbortSignal>>>,
    /// TS `_pendingBashMessages` — bash results queued while streaming,
    /// flushed at the run checkpoints ([`bash`] module).
    pending_bash_messages: Vec<Value>,
    /// TS `_overflowRecoveryAttempted`.
    overflow_recovery_attempted: bool,
    /// TS `_retryAttempt`.
    retry_attempt: f64,
    /// TS `_turnIndex`.
    turn_index: f64,
    /// TS `_entryIdsByMessage` (WeakMap<object, string>) — keyed by arena
    /// id instead of object identity (see [`events`] docs).
    entry_ids_by_message: HashMap<MsgId, String>,
    /// TS `_boundaryDispatchedMessages` (WeakSet) — arena ids.
    boundary_dispatched_messages: HashSet<MsgId>,
    /// entry id → arena id of its `message` (TS: `entry.message === message`
    /// reference comparisons). Populated for seeded and appended message
    /// entries.
    entry_message_ids: HashMap<String, MsgId>,
    /// Insertion-ordered message arena (TS: the message objects themselves).
    arena: Vec<Value>,
    /// Arena ids of `agent.state.messages`, kept in step (TS:
    /// `agent.state.messages.indexOf(message)`).
    state_message_ids: Vec<MsgId>,
    /// TS `_lastAssistantMessage`.
    last_assistant_message: Option<MsgId>,
    /// TS `_lastAssistantToolResults`.
    last_assistant_tool_results: Vec<MsgId>,
    /// TS `_lastActivityOutcome`.
    last_activity_outcome: String,
    /// Whether the last dispatched turn_end boundary requested a
    /// continuation that TS would honor inside the agent loop (a
    /// `finishTurn` `continue` decision on a non-error turn).
    turn_end_boundary_continue: bool,
    /// TS `_deferredSettledActions` (shared — see
    /// [`AgentSession::deferred_prompt_tap`]).
    deferred_settled_actions: Arc<Mutex<Vec<DeferredSettledAction>>>,
    /// Harness-visible log of executed deferred `Log` actions.
    deferred_log: Vec<String>,
    /// TS `_isEmittingAgentSettled`.
    is_emitting_agent_settled: bool,
    /// TS `_isBeforeSettle` (shared — see [`AgentSession::abort_tap`]).
    is_before_settle: Arc<AtomicBool>,
    /// TS `_abortDuringBeforeSettle` (shared).
    abort_during_before_settle: Arc<AtomicBool>,
    /// Tools slice state (TS `_toolRegistry` & friends; [`tools`]).
    tools: tools::ToolsState,
    /// Bridge state: the message currently streaming (TS: the one mutated
    /// object) and this run's completed messages (value → arena id).
    open_message: Option<MsgId>,
    run_message_values: Vec<Value>,
    run_message_ids: Vec<MsgId>,
    /// TS `_failedResponse` — the failed attempt routed through the
    /// request projection (virtual-model routing is tranche 14; the state
    /// bookkeeping is tranche 12).
    failed_response: Option<MsgId>,
    /// TS `_compactionAbortController` (shared —
    /// [`AgentSession::compaction_abort_tap`]).
    compaction_abort: Arc<Mutex<Option<AbortHandle>>>,
    /// TS `_autoCompactionAbortController` (shared).
    auto_compaction_abort: Arc<Mutex<Option<AbortHandle>>>,
    /// TS `_branchSummaryAbortController` (shared —
    /// [`AgentSession::branch_summary_abort_tap`]).
    branch_summary_abort: Arc<Mutex<Option<AbortHandle>>>,
    /// Buffered summarization-retry events (see `compaction` module docs).
    summarization_events: Arc<Mutex<Vec<Value>>>,
    /// TS `_retryAbortController` (shared — the harness fires
    /// `abortRetry()` while a prompt run holds the session; see
    /// [`AgentSession::abort_retry_tap`]). `Some` while a retry sleep is
    /// in flight (`isRetrying`).
    retry_abort: Arc<Mutex<Option<pi_agent::host::AbortHandle>>>,
    /// Harness seam: the runtime-harness fake in-flight auto-retry (the
    /// TS parity harness any-casts `session._retryAbortController = c`
    /// with an abort listener running `_finishCancelledRetry()` — the
    /// parked retry sleep is unreachable from the harness because Rust
    /// blocks concurrent prompt futures). [`AgentSession::abort_retry`]
    /// fires it synchronously, mirroring the TS abort listener.
    pending_retry_listener: Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>,
    /// TS `_scopedModels`.
    scoped_models: Vec<model::ScopedModel>,
    /// The projection-bridge sender the request hook forwards over (see
    /// [`model`] module docs).
    projection_bridge: tokio::sync::mpsc::UnboundedSender<model::ProjectionCall>,
}

/// TS `class AgentSession` — the events slice.
pub struct AgentSession {
    /// TS `agent` (readonly).
    pub agent: Agent,
    /// TS `sessionManager` (readonly).
    pub session_manager: SessionManager,
    /// TS `settingsManager.getRetrySettings()`.
    settings: Arc<dyn SessionSettingsStore>,
    /// TS `_extensionRunner` (read at execution time, like the TS).
    extension_host: Arc<dyn ExtensionHost>,
    /// TS `_nestedToolCalls` (created on the first `ctx.executeTool()` —
    /// tranche 7 wiring; injected here).
    nested_tool_calls: Option<Arc<dyn NestedCallsStore>>,
    clock: Arc<dyn Clock>,
    /// TS `_cwd`.
    pub cwd: String,
    state: SessionState,
    /// TS `_modelRuntime` auth seam (prompt slice).
    model_auth: Option<Arc<dyn ModelAuthStore>>,
    /// TS `resourceLoader.getSkills()` specs (prompt slice).
    skills: Vec<prompt::SkillSpec>,
    /// TS `resourceLoader.getPrompts().prompts`.
    prompt_templates: Vec<pi_core::prompt_templates::PromptTemplate>,
    /// TS `_modelRuntime.getModel` (record_selection seam).
    model_lookup: Option<Arc<dyn Fn(&str, &str) -> Option<Value> + Send + Sync>>,
    /// TS `sleep(delayMs, signal)` seam — the retry backoff sleeper.
    sleeper: Arc<dyn pi_agent::host::Sleeper>,
    /// Host paths the default docs section renders (system_prompt docs).
    doc_paths: system_prompt::DocPaths,
    /// The agent→session event queue (the subscription only pushes;
    /// [`prompt`] pumps it at run checkpoints — see its module docs).
    agent_event_queue: Arc<Mutex<Vec<pi_agent::AgentEvent>>>,
    /// Idle-wait resolvers (TS `_idleWaitPromise`/`_resolveIdleWait`
    /// single-slot promise; multiple waiters each get a slot).
    idle_resolvers: Mutex<Vec<tokio::sync::oneshot::Sender<()>>>,
    next_listener_id: usize,
    /// TS `_unsubscribeAgent` — the agent→session subscription is wired
    /// with the run pipeline. TODO(tranche 11).
    _unsubscribe_agent: Option<usize>,
    /// TS `_modelRuntime` routing seam (model & thinking slice).
    model_routes: Option<Arc<dyn model::ModelRouteStore>>,
    /// TS `_modelRuntime.getPhysicalModel(provider, modelId)` (stats slice).
    physical_model_lookup: Option<Arc<dyn Fn(&str, &str) -> Option<Value> + Send + Sync>>,
    /// The projection-bridge receiver [`AgentSession::run_with_bridge`]
    /// services while an agent run is parked in the request hook.
    bridge_rx: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<model::ProjectionCall>>>,
}

impl AgentSession {
    /// TS `constructor(config)`. Installs no agent hooks yet (tranches
    /// 9–11); the event handler is driven through
    /// [`AgentSession::handle_agent_event`].
    pub fn new(config: AgentSessionConfig) -> AgentSession {
        let tools_state = tools::ToolsState::new(&config);
        let agent_event_queue = Arc::new(Mutex::new(Vec::new()));
        let (projection_bridge, bridge_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut session = AgentSession {
            agent: config.agent,
            session_manager: config.session_manager,
            settings: config.settings,
            extension_host: config.extension_host,
            nested_tool_calls: config.nested_tool_calls,
            clock: config.clock,
            cwd: config.cwd,
            model_auth: config.model_auth,
            skills: config.skills,
            prompt_templates: config.prompt_templates,
            model_lookup: config.model_lookup,
            sleeper: config.sleeper.unwrap_or(Arc::new(pi_agent::host::TokioSleeper)),
            doc_paths: system_prompt::DocPaths::default(),
            agent_event_queue: Arc::clone(&agent_event_queue),
            state: SessionState {
                listeners: Arc::new(Mutex::new(Vec::new())),
                is_agent_run_active: Arc::new(AtomicBool::new(false)),
                agent_run_abort_requested: Arc::new(AtomicBool::new(false)),
                is_before_settle: Arc::new(AtomicBool::new(false)),
                abort_during_before_settle: Arc::new(AtomicBool::new(false)),
                steering_messages: Vec::new(),
                follow_up_messages: Vec::new(),
                pending_next_turn_messages: Vec::new(),
                pending_custom_messages: Vec::new(),
                bash_signals: Arc::new(Mutex::new(Vec::new())),
                pending_bash_messages: Vec::new(),
                overflow_recovery_attempted: false,
                retry_attempt: 0.0,
                turn_index: 0.0,
                entry_ids_by_message: HashMap::new(),
                boundary_dispatched_messages: HashSet::new(),
                entry_message_ids: HashMap::new(),
                arena: Vec::new(),
                state_message_ids: Vec::new(),
                last_assistant_message: None,
                last_assistant_tool_results: Vec::new(),
                last_activity_outcome: "completed".to_string(),
                turn_end_boundary_continue: false,
                deferred_settled_actions: Arc::new(Mutex::new(Vec::new())),
                deferred_log: Vec::new(),
                is_emitting_agent_settled: false,
                tools: tools_state,
                open_message: None,
                run_message_values: Vec::new(),
                run_message_ids: Vec::new(),
                failed_response: None,
                compaction_abort: Arc::new(Mutex::new(None)),
                auto_compaction_abort: Arc::new(Mutex::new(None)),
                branch_summary_abort: Arc::new(Mutex::new(None)),
                summarization_events: Arc::new(Mutex::new(Vec::new())),
                retry_abort: Arc::new(Mutex::new(None)),
                pending_retry_listener: Arc::new(Mutex::new(None)),
                scoped_models: config.scoped_models,
                projection_bridge,
            },
            idle_resolvers: Mutex::new(Vec::new()),
            next_listener_id: 0,
            _unsubscribe_agent: None,
            model_routes: config.model_routes.clone(),
            physical_model_lookup: config.physical_model_lookup.clone(),
            bridge_rx: std::sync::Mutex::new(Some(bridge_rx)),
        };
        // TS constructor: `_installHiddenDeclarationsProjection` +
        // `_installAgentForcedPromptProjection` are stateful here (see
        // `tools::AgentSession::transform_context`), then `_buildRuntime`
        // runs and the default-tools restore follows.
        // TS constructor: `_installAgentRequestProjection()` (chained over
        // any agent-level `prepareRequest`).
        session.install_agent_request_projection();
        session.build_runtime(config.initial_active_tool_names.clone(), true);
        if session.state.tools.uses_default_tools {
            let modifiers = session.state.tools.default_tool_modifiers.clone();
            session.restore_tools_from_transcript(&modifiers);
        }
        // TS constructor: `this._unsubscribeAgent = this.agent.subscribe(
        // this._handleAgentEvent)`. The Rust subscription only queues;
        // `prompt::pump_agent_events` drains it at run checkpoints.
        {
            let queue = Arc::clone(&session.agent_event_queue);
            session.agent.subscribe(Arc::new(move |event, _signal| {
                queue.lock().expect("agent event queue").push(event);
                Box::pin(async {})
            }));
        }
        session
    }

    // ========================================================================
    // Arena (message identity)
    // ========================================================================

    /// Register a message in the identity arena, returning its id (TS: the
    /// object reference itself).
    pub fn register_message(&mut self, message: Value) -> MsgId {
        self.state.arena.push(message);
        self.state.arena.len() - 1
    }

    /// The registered message (TS: the object itself).
    pub fn message(&self, id: MsgId) -> &Value {
        &self.state.arena[id]
    }

    /// Record that seeded entry `entry_id` stores the arena message `id`
    /// (TS: the entry was built around that very object).
    pub fn link_entry_message(&mut self, entry_id: &str, id: MsgId) {
        self.state.entry_message_ids.insert(entry_id.to_string(), id);
    }

    /// Seed `agent.state.messages` with arena ids `ids` (TS constructor
    /// `initialState.messages` — the same objects).
    pub fn seed_state_messages(&mut self, ids: &[MsgId]) {
        let messages = ids.iter().map(|&id| self.state.arena[id].clone()).collect();
        self.agent.set_messages(messages);
        self.state.state_message_ids = ids.to_vec();
    }

    /// Write `message` back to arena slot `id` and, when the slot is in
    /// agent state, to the same position (TS: in-place object mutation is
    /// visible through every reference — plan §2 trap 2).
    fn sync_message(&mut self, id: MsgId, message: Value) {
        self.state.arena[id] = message.clone();
        if let Some(pos) = self.state.state_message_ids.iter().position(|&m| m == id) {
            let mut messages = self.agent.messages();
            if pos < messages.len() {
                messages[pos] = message;
                self.agent.set_messages(messages);
            }
        }
    }

    // ========================================================================
    // Event subscription (TS `subscribe` / `_emit`)
    // ========================================================================

    /// TS `subscribe(listener)` — returns the unsubscribe token.
    pub fn subscribe(&mut self, listener: SessionEventListener) -> usize {
        let id = self.next_listener_id;
        self.next_listener_id += 1;
        self.state.listeners.lock().expect("listeners").push((id, listener));
        id
    }

    /// The TS returned unsubscribe function.
    pub fn unsubscribe(&mut self, id: usize) {
        self.state.listeners.lock().expect("listeners").retain(|(i, _)| *i != id);
    }

    /// TS `_emit` — notify every listener in subscription order.
    fn emit(&mut self, event: Value) {
        for (_, listener) in self.state.listeners.lock().expect("listeners").iter_mut() {
            listener(&event);
        }
    }

    /// Harness/`'static`-future seam for `_emit`: a shareable closure that
    /// notifies every listener (the bash slice's `onChunk` callback emits
    /// `bash_execution_update` from the executor future).
    pub fn emit_tap(&self) -> Arc<dyn Fn(Value) + Send + Sync> {
        let listeners = Arc::clone(&self.state.listeners);
        Arc::new(move |event: Value| {
            for (_, listener) in listeners.lock().expect("listeners").iter_mut() {
                listener(&event);
            }
        })
    }

    /// TS `_emitQueueUpdate`.
    fn emit_queue_update(&mut self) {
        let event = events::queue_update_event(&self.state.steering_messages, &self.state.follow_up_messages);
        self.emit(event);
    }

    // ========================================================================
    // Idle / settled
    // ========================================================================

    /// TS `get isStreaming`.
    pub fn is_streaming(&self) -> bool {
        self.state.is_agent_run_active.load(Ordering::SeqCst)
    }

    /// TS `get isIdle` (`!_isAgentRunActive && !isCompacting`;
    /// compaction controllers are tranche 13).
    pub fn is_idle(&self) -> bool {
        !self.state.is_agent_run_active.load(Ordering::SeqCst) && !self.is_compacting()
    }

    /// TS `waitForIdle()` — a `'static` future so callers can hold it
    /// pending across later `&mut self` operations (TS: a floating
    /// promise).
    pub fn wait_for_idle(&self) -> BoxFuture<'static, ()> {
        if self.is_idle() {
            return Box::pin(async {});
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.idle_resolvers.lock().expect("idle resolvers").push(tx);
        Box::pin(async move {
            let _ = rx.await;
        })
    }

    /// TS `_resolveIdleWaitIfIdle`.
    fn resolve_idle_wait_if_idle(&mut self) {
        if !self.is_idle() {
            return;
        }
        let resolvers = std::mem::take(self.idle_resolvers.get_mut().expect("idle resolvers"));
        for resolver in resolvers {
            let _ = resolver.send(());
        }
    }

    /// TS `get retryAttempt`.
    pub fn retry_attempt(&self) -> f64 {
        self.state.retry_attempt
    }

    // ========================================================================
    // Queueing (TS `_queueSteer` / `_queueFollowUp`)
    // ========================================================================

    /// TS `_queueSteer(text, images?)` — internal, already expanded.
    pub fn queue_steer_internal(&mut self, text: &str, images: Option<&[Value]>) {
        self.state.steering_messages.push(text.to_string());
        self.emit_queue_update();
        let mut content = vec![json!({ "type": "text", "text": text })];
        if let Some(images) = images {
            content.extend(images.iter().cloned());
        }
        let message = json!({
            "role": "user",
            "content": content,
            "timestamp": pi_core::ai::js_number(self.clock.date_now_ms()),
        });
        self.agent.steer(message);
    }

    /// TS `_queueFollowUp(text, images?)`.
    pub fn queue_follow_up_internal(&mut self, text: &str, images: Option<&[Value]>) {
        self.state.follow_up_messages.push(text.to_string());
        self.emit_queue_update();
        let mut content = vec![json!({ "type": "text", "text": text })];
        if let Some(images) = images {
            content.extend(images.iter().cloned());
        }
        let message = json!({
            "role": "user",
            "content": content,
            "timestamp": pi_core::ai::js_number(self.clock.date_now_ms()),
        });
        self.agent.follow_up(message);
    }

    /// TS `clearQueue()` — returns and clears both queues.
    pub fn clear_queue(&mut self) -> (Vec<String>, Vec<String>) {
        let steering = std::mem::take(&mut self.state.steering_messages);
        let follow_up = std::mem::take(&mut self.state.follow_up_messages);
        self.agent.clear_all_queues();
        self.emit_queue_update();
        (steering, follow_up)
    }

    // ========================================================================
    // Custom messages (TS `sendCustomMessage` slice)
    // ========================================================================

    /// TS `sendCustomMessage(message, options)` — the paths the events
    /// slice covers. `triggerTurn: Some(true)` needs the run pipeline
    /// (tranche 11) and is rejected here.
    pub async fn send_custom_message(
        &mut self,
        custom_type: &str,
        content: Value,
        display: bool,
        details: Option<Value>,
        trigger_turn: Option<bool>,
        deliver_as: Option<&str>,
    ) -> Result<(), String> {
        // Untyped extensions can pass null/missing content; normalize at
        // ingestion (TS).
        let content = if content.is_null() { json!([]) } else { content };
        // TS `{ role, customType, content, display, details, timestamp }`
        // with `details: message.details` — `undefined` is omitted by
        // JSON serialization, so a missing `details` never becomes null.
        let mut app_message = Map::new();
        app_message.insert("role".to_string(), json!("custom"));
        app_message.insert("customType".to_string(), json!(custom_type));
        app_message.insert("content".to_string(), content);
        app_message.insert("display".to_string(), json!(display));
        if let Some(details) = details {
            app_message.insert("details".to_string(), details);
        }
        app_message.insert(
            "timestamp".to_string(),
            pi_core::ai::js_number(self.clock.date_now_ms()),
        );
        let app_message = Value::Object(app_message);
        if deliver_as == Some("nextTurn") {
            self.state.pending_next_turn_messages.push(app_message);
        } else if self.is_streaming() && trigger_turn != Some(false) {
            if deliver_as == Some("followUp") {
                self.agent.follow_up(app_message);
            } else {
                self.agent.steer(app_message);
            }
        } else if trigger_turn == Some(true) {
            if self.state.is_emitting_agent_settled {
                self.state.deferred_settled_actions.lock().expect("deferred actions").push(DeferredSettledAction::SendCustomMessage {
                    custom_type: custom_type.to_string(),
                    content: app_message.get("content").cloned().unwrap_or(Value::Null),
                    display,
                    details: app_message.get("details").filter(|d| !d.is_null()).cloned(),
                    trigger_turn,
                    deliver_as: deliver_as.map(str::to_string),
                });
                return Ok(());
            }
            self.start_run(prompt::RunTrigger::Custom { message: app_message }, None).await?;
        } else if self.is_streaming() {
            // Appending now would put the message between an assistant
            // tool call and its result; defer to the end of the turn (TS).
            self.state.pending_custom_messages.push(app_message);
        } else {
            self.append_custom_message(app_message);
        }
        Ok(())
    }

    /// TS `dispose()` — the slice's teardown. Retry/compaction/branch-
    /// summary/bash aborts and resource cleanup arrive with their
    /// tranches; the extension invalidation message is byte-exact.
    pub fn dispose(&mut self) {
        self.abort_retry();
        self.abort_compaction();
        self.abort_branch_summary();
        self.abort_bash();
        self.agent.abort();
        self.extension_host.invalidate(
            "This extension ctx is stale after session replacement or reload. Do not use a captured pi or command ctx after ctx.newSession(), ctx.fork(), ctx.switchSession(), or ctx.reload(). For newSession, fork, and switchSession, move post-replacement work into withSession and use the ctx passed to withSession. For reload, do not use the old ctx after await ctx.reload().",
        );
        // TODO(tranche 11): this._disconnectFromAgent();
        self.state.listeners.lock().expect("listeners").clear();
        // TODO(tranche 13): cache warmer cleanup.
        // TODO(tranche 18): cleanupSessionResources(this.sessionId).
    }

    // ========================================================================
    // Harness seams (TS parity harness reaches privates via any-cast; the
    // Rust mirror of that access lives here, documented per field).
    // ========================================================================

    /// Harness seam: TS any-cast `_isAgentRunActive = value`.
    pub fn set_agent_run_active(&mut self, value: bool) {
        self.state.is_agent_run_active.store(value, Ordering::SeqCst);
    }

    /// Harness seam: TS any-cast `_retryAttempt = value`.
    pub fn set_retry_attempt(&mut self, value: f64) {
        self.state.retry_attempt = value;
    }

    /// Harness seam: TS `abort()` sets `_agentRunAbortRequested` (the rest
    /// of abort() is tranche 11+).
    pub fn request_abort(&mut self) {
        self.state.agent_run_abort_requested.store(true, Ordering::SeqCst);
    }

    /// Harness seam: TS `abort()`'s synchronous half as a shareable
    /// closure — the TS parity harness fires `void session.abort()` from
    /// extension-handler context; the Rust harness fakes reach the same
    /// flags through this tap (retry/compaction/branch-summary/bash
    /// aborts arrive with their tranches).
    pub fn abort_tap(&self) -> Arc<dyn Fn() + Send + Sync> {
        let active = Arc::clone(&self.state.is_agent_run_active);
        let abort_requested = Arc::clone(&self.state.agent_run_abort_requested);
        let is_before_settle = Arc::clone(&self.state.is_before_settle);
        let abort_during_before_settle = Arc::clone(&self.state.abort_during_before_settle);
        let retry_abort = Arc::clone(&self.state.retry_abort);
        let compaction_abort = Arc::clone(&self.state.compaction_abort);
        let auto_compaction_abort = Arc::clone(&self.state.auto_compaction_abort);
        let branch_summary_abort = Arc::clone(&self.state.branch_summary_abort);
        let agent = self.agent.clone();
        Arc::new(move || {
            if active.load(Ordering::SeqCst) {
                abort_requested.store(true, Ordering::SeqCst);
            }
            if let Some(handle) = retry_abort.lock().expect("retry abort slot").as_ref() {
                handle.abort();
            }
            // TS abortCompaction() — part of abort()'s synchronous prefix.
            if let Some(handle) = compaction_abort.lock().expect("compaction abort").as_ref() {
                handle.abort();
            }
            if let Some(handle) = auto_compaction_abort.lock().expect("auto compaction abort").as_ref() {
                handle.abort();
            }
            if let Some(handle) = branch_summary_abort.lock().expect("branch summary abort").as_ref() {
                handle.abort();
            }
            if is_before_settle.load(Ordering::SeqCst) {
                abort_during_before_settle.store(true, Ordering::SeqCst);
            }
            agent.abort();
        })
    }

    /// Harness seam: TS any-cast `_deferredSettledActions.push(...)`.
    pub fn push_deferred_log_action(&mut self, tag: &str) {
        self.state.deferred_settled_actions.lock().expect("deferred actions").push(DeferredSettledAction::Log(tag.to_string()));
    }

    /// Harness seam: the TS parity harness fires `void session.prompt(text)`
    /// from an `agent_settled` listener (a floating promise whose synchronous
    /// prefix pushes the deferred action); the shared deferred-actions slot
    /// is the Rust mirror of that reach-in.
    pub fn deferred_prompt_tap(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        let slot = Arc::clone(&self.state.deferred_settled_actions);
        Arc::new(move |text: &str| {
            slot.lock().expect("deferred actions").push(DeferredSettledAction::Prompt {
                text: text.to_string(),
                options: prompt::PromptOptions::default(),
            });
        })
    }

    /// TS `_extensionRunner` (read at execution time, like the TS).
    pub(crate) fn extension_host_ref(&self) -> &Arc<dyn ExtensionHost> {
        &self.extension_host
    }

    /// TS `createReplacedSessionContext()` — the command-context half; the
    /// session-bound `sendMessage`/`sendUserMessage` are host-side closures
    /// in TS (the runtime port records the context only).
    pub fn create_replaced_session_context(&self) -> Value {
        self.extension_host.command_context()
    }

    /// TS `refreshContext()`.
    pub fn refresh_context(&mut self) {
        let messages = self
            .session_manager
            .build_session_projection()
            .messages
            .iter()
            .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
            .collect();
        self.agent.set_messages(messages);
    }

    // ========================================================================
    // Read-only state access (queueState surface)
    // ========================================================================

    pub fn steering_messages(&self) -> &[String] {
        &self.state.steering_messages
    }

    pub fn follow_up_messages(&self) -> &[String] {
        &self.state.follow_up_messages
    }

    pub fn pending_custom_messages(&self) -> &[Value] {
        &self.state.pending_custom_messages
    }

    /// TS `_lastAssistantToolResults` (the messages).
    pub fn last_assistant_tool_results(&self) -> Vec<Value> {
        self.state.last_assistant_tool_results.iter().map(|&id| self.state.arena[id].clone()).collect()
    }

    /// TS `_lastAssistantMessage`.
    pub fn last_assistant_message(&self) -> Option<Value> {
        self.state.last_assistant_message.map(|id| self.state.arena[id].clone())
    }

    /// TS `_lastActivityOutcome`.
    pub fn last_activity_outcome(&self) -> &str {
        &self.state.last_activity_outcome
    }

    /// TS `_turnIndex`.
    pub fn turn_index(&self) -> f64 {
        self.state.turn_index
    }

    /// Harness seam: the executed deferred-`Log` tags, in run order.
    pub fn deferred_log(&self) -> &[String] {
        &self.state.deferred_log
    }

    /// TS `_findLastAssistantMessage()` — the last assistant message in
    /// agent state (arena id).
    pub fn find_last_assistant_message(&self) -> Option<MsgId> {
        self.state
            .state_message_ids
            .iter()
            .rev()
            .find(|&&id| self.state.arena[id].get("role").and_then(Value::as_str) == Some("assistant"))
            .copied()
    }

    /// `settingsManager.getCompactionSettings(this.model)`.
    pub(crate) fn compaction_settings_for(&self, model: &Value) -> CompactionSettings {
        let key = if model.is_null() { None } else {
            Some((
                model.get("provider").and_then(Value::as_str).unwrap_or_default(),
                model.get("id").and_then(Value::as_str).unwrap_or_default(),
            ))
        };
        self.settings.compaction_settings(key)
    }
}
