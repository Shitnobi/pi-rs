//! `AgentSession` facade + config (agent-session.ts `class AgentSession`,
//! construction slice). The event-handling slice lives in
//! [`super::event_handler`]; event shapes in [`super::events`].
//!
//! Tranche 8 ports only what the events slice needs; tool registry, model
//! runtime, cache warmer, and resource loader arrive in tranches 9–19
//! (marked `TODO(tranche N)`).

pub mod events;
pub mod event_handler;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use pi_agent::types::BoxFuture;
use pi_agent::{Agent, Clock};
use pi_core::session_manager::SessionManager;
use pi_core::settings_manager::RetrySettings;

pub use events::{MsgId, SessionAgentEvent};

/// TS `settingsManager.getRetrySettings()` seam — the settings the events
/// slice reads. The full settings manager lands with the prompt pipeline
/// (tranche 10+).
pub trait SessionSettingsStore: Send + Sync {
    fn retry_settings(&self) -> RetrySettings;
}

/// Stand-in for `ExtensionRunner` (plan §4 `ExtensionHost`): the extension
/// emission surface the events slice uses. The real runner is Phase 4+;
/// parity tests inject a recording fake.
pub trait ExtensionHost: Send + Sync {
    /// TS `hasHandlers(event)`.
    fn has_handlers(&self, event: &str) -> bool;
    /// TS `emit(event)` — awaited before public listeners.
    fn emit<'a>(&'a self, event: Value) -> BoxFuture<'a, ()>;
    /// TS `emitMessageEnd(event)` — may return a replacement message.
    fn emit_message_end<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>>;
    /// TS `emitError(error)`.
    fn emit_error(&self, error: Value);
    /// TS `invalidate(message)`.
    fn invalidate(&self, message: &str);
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
}

/// A deferred `agent_settled` action (TS `_deferredSettledActions` holds
/// async closures re-invoking `prompt`/`sendCustomMessage`). Those callers
/// land with the run pipeline (tranche 11); until then the harness drives
/// the deterministic `Log` stand-in (plan §2 trap 8).
#[derive(Debug, Clone)]
pub enum DeferredSettledAction {
    Log(String),
}

/// TS `AgentSessionEventListener`.
pub type SessionEventListener = Box<dyn FnMut(&Value) + Send>;

/// The mutable half of `AgentSession` (TS private fields).
struct SessionState {
    /// TS `_eventListeners`.
    listeners: Vec<(usize, SessionEventListener)>,
    /// TS `_isAgentRunActive`.
    is_agent_run_active: bool,
    /// TS `_agentRunAbortRequested`.
    agent_run_abort_requested: bool,
    /// TS `_steeringMessages` — pending steering texts for UI display.
    steering_messages: Vec<String>,
    /// TS `_followUpMessages`.
    follow_up_messages: Vec<String>,
    /// TS `_pendingNextTurnMessages`.
    pending_next_turn_messages: Vec<Value>,
    /// TS `_pendingCustomMessages` — context-only custom messages queued
    /// during a run, flushed once the current turn's tool results are in.
    pending_custom_messages: Vec<Value>,
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
    /// TS `_deferredSettledActions`.
    deferred_settled_actions: Vec<DeferredSettledAction>,
    /// Harness-visible log of executed deferred `Log` actions.
    deferred_log: Vec<String>,
    /// TS `_isEmittingAgentSettled`.
    is_emitting_agent_settled: bool,
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
    /// Idle-wait resolvers (TS `_idleWaitPromise`/`_resolveIdleWait`
    /// single-slot promise; multiple waiters each get a slot).
    idle_resolvers: Mutex<Vec<tokio::sync::oneshot::Sender<()>>>,
    next_listener_id: usize,
    /// TS `_unsubscribeAgent` — the agent→session subscription is wired
    /// with the run pipeline. TODO(tranche 11).
    _unsubscribe_agent: Option<usize>,
}

impl AgentSession {
    /// TS `constructor(config)`. Installs no agent hooks yet (tranches
    /// 9–11); the event handler is driven through
    /// [`AgentSession::handle_agent_event`].
    pub fn new(config: AgentSessionConfig) -> AgentSession {
        AgentSession {
            agent: config.agent,
            session_manager: config.session_manager,
            settings: config.settings,
            extension_host: config.extension_host,
            nested_tool_calls: config.nested_tool_calls,
            clock: config.clock,
            cwd: config.cwd,
            state: SessionState {
                listeners: Vec::new(),
                is_agent_run_active: false,
                agent_run_abort_requested: false,
                steering_messages: Vec::new(),
                follow_up_messages: Vec::new(),
                pending_next_turn_messages: Vec::new(),
                pending_custom_messages: Vec::new(),
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
                deferred_settled_actions: Vec::new(),
                deferred_log: Vec::new(),
                is_emitting_agent_settled: false,
            },
            idle_resolvers: Mutex::new(Vec::new()),
            next_listener_id: 0,
            _unsubscribe_agent: None,
        }
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
        self.state.listeners.push((id, listener));
        id
    }

    /// The TS returned unsubscribe function.
    pub fn unsubscribe(&mut self, id: usize) {
        self.state.listeners.retain(|(i, _)| *i != id);
    }

    /// TS `_emit` — notify every listener in subscription order.
    fn emit(&mut self, event: Value) {
        for (_, listener) in &mut self.state.listeners {
            listener(&event);
        }
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
        self.state.is_agent_run_active
    }

    /// TS `get isIdle` (`!_isAgentRunActive && !isCompacting`;
    /// compaction controllers are tranche 13).
    pub fn is_idle(&self) -> bool {
        !self.state.is_agent_run_active
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

    /// TS `_queueSteer(text)` — internal, already expanded.
    pub fn queue_steer_internal(&mut self, text: &str) {
        self.state.steering_messages.push(text.to_string());
        self.emit_queue_update();
        let message = json!({
            "role": "user",
            "content": [{ "type": "text", "text": text }],
            "timestamp": pi_core::ai::js_number(self.clock.date_now_ms()),
        });
        self.agent.steer(message);
    }

    /// TS `_queueFollowUp(text)`.
    pub fn queue_follow_up_internal(&mut self, text: &str) {
        self.state.follow_up_messages.push(text.to_string());
        self.emit_queue_update();
        let message = json!({
            "role": "user",
            "content": [{ "type": "text", "text": text }],
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
    pub fn send_custom_message(
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
                // TS defers a re-invocation of sendCustomMessage; the
                // deferred-action carriers arrive with tranche 11.
                return Err("sendCustomMessage deferred re-queueing requires the run pipeline (tranche 11)".to_string());
            }
            return Err("sendCustomMessage triggerTurn requires the run pipeline (tranche 11)".to_string());
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
        // TODO(tranche 12): this.abortRetry();
        // TODO(tranche 13): this.abortCompaction();
        // TODO(tranche 16): this.abortBranchSummary();
        // TODO(tranche 15): this.abortBash();
        self.agent.abort();
        self.extension_host.invalidate(
            "This extension ctx is stale after session replacement or reload. Do not use a captured pi or command ctx after ctx.newSession(), ctx.fork(), ctx.switchSession(), or ctx.reload(). For newSession, fork, and switchSession, move post-replacement work into withSession and use the ctx passed to withSession. For reload, do not use the old ctx after await ctx.reload().",
        );
        // TODO(tranche 11): this._disconnectFromAgent();
        self.state.listeners.clear();
        // TODO(tranche 13): cache warmer cleanup.
        // TODO(tranche 18): cleanupSessionResources(this.sessionId).
    }

    // ========================================================================
    // Harness seams (TS parity harness reaches privates via any-cast; the
    // Rust mirror of that access lives here, documented per field).
    // ========================================================================

    /// Harness seam: TS any-cast `_isAgentRunActive = value`.
    pub fn set_agent_run_active(&mut self, value: bool) {
        self.state.is_agent_run_active = value;
    }

    /// Harness seam: TS any-cast `_retryAttempt = value`.
    pub fn set_retry_attempt(&mut self, value: f64) {
        self.state.retry_attempt = value;
    }

    /// Harness seam: TS `abort()` sets `_agentRunAbortRequested` (the rest
    /// of abort() is tranche 11+).
    pub fn request_abort(&mut self) {
        self.state.agent_run_abort_requested = true;
    }

    /// Harness seam: TS any-cast `_deferredSettledActions.push(...)`.
    pub fn push_deferred_log_action(&mut self, tag: &str) {
        self.state.deferred_settled_actions.push(DeferredSettledAction::Log(tag.to_string()));
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
}
