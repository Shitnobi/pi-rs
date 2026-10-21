//! Port of `class Agent` from `packages/agent/src/agent.ts`: the stateful
//! wrapper around the low-level agent loop.
//!
//! `Agent` owns the current transcript, emits lifecycle events (state
//! reduction first, then listeners awaited in subscription order with the
//! run's abort signal), and exposes the queueing APIs for steering and
//! follow-up messages (reusing [`crate::agent_surface::AgentQueues`]).
//!
//! Threading model (plan §3 G8, current-thread runtime): the run entry
//! points mirror the JS synchronous prefix exactly — `prompt`/`continue`
//! perform their guard checks and install the active run (isStreaming on,
//! streaming/error cleared) synchronously, then return the remainder of
//! `runWithLifecycle` as an owned future the caller drives. Everything the
//! run touches is `Arc`/`Mutex` interior-mutability state, so a pending
//! run future coexists with script ops the way an un-awaited JS promise
//! coexists with synchronous driver code.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};
use tokio::sync::Notify;

use pi_core::ai::{get_current_system_message, js_number};
use pi_providers::pi_ai::transcript::{
    create_initial_system_message, get_current_system_prompt,
};

use crate::agent_surface::AgentQueues;
use crate::host::{AbortHandle, Clock};
use crate::r#loop::{
    run_agent_loop, run_agent_loop_continue, run_agent_loop_resume, tool_declarations,
    AgentEventSink, LoopConfig, TransformContext,
    LoopStreamFn, MessagePollHook,
};
use crate::types::{AgentEvent, AgentTool, BoxFuture, QueueMode, ToolExecutionMode};

/// TS `EMPTY_USAGE` (exact JSON / key order).
pub fn empty_usage() -> Value {
    json!({
        "input": 0,
        "output": 0,
        "cacheRead": 0,
        "cacheWrite": 0,
        "totalTokens": 0,
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
    })
}

/// TS `DEFAULT_MODEL` (exact JSON / key order — same model JSON as the
/// agent-loop parity entry).
pub fn default_model() -> Value {
    json!({
        "id": "unknown",
        "name": "unknown",
        "api": "unknown",
        "provider": "unknown",
        "baseUrl": "",
        "reasoning": false,
        "input": [],
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 0,
        "maxTokens": 0,
    })
}

/// A subscribed lifecycle listener (TS
/// `(event: AgentEvent, signal: AbortSignal) => Promise<void> | void`).
/// The signal is the active run's abort handle.
pub type AgentListener =
    dyn Fn(AgentEvent, AbortHandle) -> BoxFuture<'static, ()> + Send + Sync;

/// The remainder of a started run (`runWithLifecycle` after the
/// synchronous prefix). `Err` models a rejection (only reachable when
/// `handleRunFailure` itself fails — see TS `runWithLifecycle`).
pub type AgentRun = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Options for constructing an [`Agent`] (TS `AgentOptions` — the fields
/// observable through the parity harness).
pub struct AgentOptions {
    /// TS `initialState.messages` (default `[]`).
    pub messages: Vec<Value>,
    /// TS `initialState.tools` — the executable tools (default `[]`).
    pub tools: Vec<Arc<dyn AgentTool>>,
    /// TS `initialState.systemPrompt`.
    pub system_prompt: Option<String>,
    /// TS `initialState.model` (default `DEFAULT_MODEL`).
    pub model: Value,
    /// TS `initialState.thinkingLevel` (default `"off"`).
    pub thinking_level: String,
    /// TS `steeringMode` (default `"one-at-a-time"`).
    pub steering_mode: QueueMode,
    /// TS `followUpMode` (default `"one-at-a-time"`).
    pub follow_up_mode: QueueMode,
    /// TS `streamFn`.
    pub stream_fn: Arc<LoopStreamFn>,
    /// TS `toolExecution` (default `"parallel"`).
    pub tool_execution: ToolExecutionMode,
    /// The `Date.now`/`performance.now` seam.
    pub clock: Arc<dyn Clock>,
}

impl AgentOptions {
    /// Fill in the TS defaults (`createMutableAgentState` + constructor).
    pub fn with_defaults(stream_fn: Arc<LoopStreamFn>, clock: Arc<dyn Clock>) -> Self {
        AgentOptions {
            messages: Vec::new(),
            tools: Vec::new(),
            system_prompt: None,
            model: default_model(),
            thinking_level: "off".to_string(),
            steering_mode: QueueMode::default(),
            follow_up_mode: QueueMode::default(),
            stream_fn,
            tool_execution: ToolExecutionMode::default(),
            clock,
        }
    }
}

/// TS `MutableAgentState` — the mutable half of `Agent#state`.
struct MutableState {
    messages: Vec<Value>,
    tools: Vec<Arc<dyn AgentTool>>,
    model: Value,
    thinking_level: String,
    is_streaming: bool,
    streaming_message: Option<Value>,
    /// Set contents in insertion order (TS `Set<string>`).
    pending_tool_calls: Vec<String>,
    error_message: Option<String>,
}

/// TS `ActiveRun` — the run's abort controller (the promise half is the
/// [`Notify`] in [`AgentInner`]).
struct ActiveRun {
    abort: AbortHandle,
}

struct AgentInner {
    state: Mutex<MutableState>,
    listeners: Mutex<Vec<(usize, Arc<AgentListener>)>>,
    queues: Mutex<AgentQueues>,
    active_run: Mutex<Option<ActiveRun>>,
    /// Resolved by `finishRun` (`activeRun.resolve()`), waited on by
    /// `waitForIdle`.
    idle: Notify,
    stream_fn: Arc<LoopStreamFn>,
    tool_execution: ToolExecutionMode,
    clock: Arc<dyn Clock>,
    next_listener_id: AtomicUsize,
    /// TS has no equivalent: `abort()` with no active run is a no-op there,
    /// because the session re-aborts on the `agent_start` event before the
    /// first request. The Rust session pumps events after the run, so it
    /// arms the next run's signal instead (`abort_next_run`).
    pending_abort: AtomicBool,
    /// TS `agent.transformContext` — installed per run by the session
    /// (forced-prompt projection).
    context_transform: Mutex<Option<TransformContext>>,
}

/// Stateful wrapper around the low-level agent loop (TS `class Agent`).
///
/// Clone-cheap handle over shared state, so pending run futures, tools,
/// and listeners can all observe the same agent.
#[derive(Clone)]
pub struct Agent {
    inner: Arc<AgentInner>,
}

enum RunKind {
    /// `runAgentLoop(messages, snapshot, …)` — a fresh prompt.
    Prompt { messages: Vec<Value> },
    /// `runAgentLoopContinue(snapshot, …)`.
    Continue,
    /// An in-loop continuation (TS `finishTurn` returning `continue`):
    /// `turn_start` but no `agent_start`, no entry guards.
    Resume,
}

/// Emit sink forwarding loop events into `Agent#processEvents` (TS
/// `(event) => this.processEvents(event)`).
struct EventForwarder {
    agent: Agent,
}

impl AgentEventSink for EventForwarder {
    fn emit<'a>(&'a self, event: AgentEvent) -> BoxFuture<'a, ()> {
        let agent = self.agent.clone();
        Box::pin(async move { agent.process_events(event).await })
    }
}

impl Agent {
    /// TS constructor + `createMutableAgentState`: `messages[0]` not a
    /// system message → unshift `createInitialSystemMessage(systemPrompt,
    /// tools.map(toToolDeclaration))`.
    pub fn new(options: AgentOptions) -> Agent {
        let mut messages = options.messages;
        let declarations = tool_declarations(&options.tools);
        let leading_system =
            messages.first().and_then(|m| m.get("role").and_then(Value::as_str)) == Some("system");
        if !leading_system {
            if let Some(initial) =
                create_initial_system_message(options.system_prompt.as_deref(), Some(&declarations))
            {
                messages.insert(0, initial);
            }
        }
        Agent {
            inner: Arc::new(AgentInner {
                state: Mutex::new(MutableState {
                    messages,
                    tools: options.tools,
                    model: options.model,
                    thinking_level: options.thinking_level,
                    is_streaming: false,
                    streaming_message: None,
                    pending_tool_calls: Vec::new(),
                    error_message: None,
                }),
                listeners: Mutex::new(Vec::new()),
                queues: Mutex::new(AgentQueues::new(
                    options.steering_mode,
                    options.follow_up_mode,
                )),
                active_run: Mutex::new(None),
                idle: Notify::new(),
                stream_fn: options.stream_fn,
                tool_execution: options.tool_execution,
                clock: options.clock,
                next_listener_id: AtomicUsize::new(0),
                pending_abort: AtomicBool::new(false),
                context_transform: Mutex::new(None),
            }),
        }
    }

    // ========================================================================
    // Subscriptions
    // ========================================================================

    /// TS `subscribe(listener)` — returns the unsubscribe token.
    pub fn subscribe(&self, listener: Arc<AgentListener>) -> usize {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::SeqCst);
        self.inner.listeners.lock().expect("agent listeners").push((id, listener));
        id
    }

    /// Remove a subscription (the TS returned unsubscribe function).
    pub fn unsubscribe(&self, id: usize) {
        self.inner.listeners.lock().expect("agent listeners").retain(|(i, _)| *i != id);
    }

    // ========================================================================
    // State
    // ========================================================================

    /// TS `state.messages` getter (a defensive copy).
    pub fn messages(&self) -> Vec<Value> {
        self.inner.state.lock().expect("agent state").messages.clone()
    }

    /// TS `state.messages = next` (copy-on-assign — the input is owned).
    pub fn set_messages(&self, messages: Vec<Value>) {
        self.inner.state.lock().expect("agent state").messages = messages;
    }

    /// TS `state.tools` getter.
    pub fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.inner.state.lock().expect("agent state").tools.clone()
    }

    /// TS `state.tools = next` (copy-on-assign).
    pub fn set_tools(&self, tools: Vec<Arc<dyn AgentTool>>) {
        self.inner.state.lock().expect("agent state").tools = tools;
    }

    /// TS `state.systemPrompt` getter — `getCurrentSystemPrompt(messages)`.
    pub fn system_prompt(&self) -> String {
        let messages = self.messages();
        get_current_system_prompt(&messages)
    }

    /// TS `state.isStreaming`.
    pub fn is_streaming(&self) -> bool {
        self.inner.state.lock().expect("agent state").is_streaming
    }

    /// TS `state.model` getter.
    pub fn model(&self) -> Value {
        self.inner.state.lock().expect("agent state").model.clone()
    }

    /// The harness state snapshot (getState + the tool `snapshot` step):
    /// exact JSON shape / key order from the shared protocol spec.
    pub fn state_snapshot(&self) -> Value {
        let state = self.inner.state.lock().expect("agent state");
        let queues = self.inner.queues.lock().expect("agent queues");
        json!({
            "messages": state.messages,
            "isStreaming": state.is_streaming,
            "streamingMessage": state.streaming_message.clone().unwrap_or(Value::Null),
            "pendingToolCalls": state.pending_tool_calls,
            "errorMessage": state.error_message.clone().map(Value::String).unwrap_or(Value::Null),
            "model": state.model,
            "thinkingLevel": state.thinking_level,
            "toolNames": state.tools.iter().map(|t| t.name().to_string()).collect::<Vec<_>>(),
            "steeringMode": queues.steering_mode().as_str(),
            "followUpMode": queues.follow_up_mode().as_str(),
        })
    }

    // ========================================================================
    // Queues
    // ========================================================================

    /// TS `steer(message)`.
    pub fn steer(&self, message: Value) {
        self.inner.queues.lock().expect("agent queues").steer(message);
    }

    /// TS `followUp(message)`.
    pub fn follow_up(&self, message: Value) {
        self.inner.queues.lock().expect("agent queues").follow_up(message);
    }

    /// TS `clearSteeringQueue()`.
    pub fn clear_steering_queue(&self) {
        self.inner.queues.lock().expect("agent queues").clear_steering_queue();
    }

    /// TS `clearFollowUpQueue()`.
    pub fn clear_follow_up_queue(&self) {
        self.inner.queues.lock().expect("agent queues").clear_follow_up_queue();
    }

    /// TS `clearAllQueues()`.
    pub fn clear_all_queues(&self) {
        self.inner.queues.lock().expect("agent queues").clear_all_queues();
    }

    /// TS `hasQueuedMessages()`.
    pub fn has_queued_messages(&self) -> bool {
        self.inner.queues.lock().expect("agent queues").has_queued_messages()
    }

    /// TS `peekQueuedMessages()`.
    pub fn peek_queued_messages(&self) -> Vec<Value> {
        self.inner.queues.lock().expect("agent queues").peek_queued_messages()
    }

    /// TS `steeringMode` getter.
    pub fn steering_mode(&self) -> QueueMode {
        self.inner.queues.lock().expect("agent queues").steering_mode()
    }

    /// TS `steeringMode` setter.
    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.inner.queues.lock().expect("agent queues").set_steering_mode(mode);
    }

    /// TS `followUpMode` getter.
    pub fn follow_up_mode(&self) -> QueueMode {
        self.inner.queues.lock().expect("agent queues").follow_up_mode()
    }

    /// TS `followUpMode` setter.
    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.inner.queues.lock().expect("agent queues").set_follow_up_mode(mode);
    }

    fn drain_steering(&self) -> Vec<Value> {
        self.inner.queues.lock().expect("agent queues").drain_steering()
    }

    fn drain_follow_up(&self) -> Vec<Value> {
        self.inner.queues.lock().expect("agent queues").drain_follow_up()
    }

    // ========================================================================
    // Run lifecycle
    // ========================================================================

    fn has_active_run(&self) -> bool {
        self.inner.active_run.lock().expect("active run").is_some()
    }

    /// TS `get signal()` — the active run's abort handle, if any.
    pub fn signal(&self) -> Option<AbortHandle> {
        self.inner
            .active_run
            .lock()
            .expect("active run")
            .as_ref()
            .map(|run| run.abort.clone())
    }

    /// TS `abort()` — abort the current run, if one is active.
    pub fn abort(&self) {
        if let Some(run) = self.inner.active_run.lock().expect("active run").as_ref() {
            run.abort.abort();
        }
    }

    /// Arm the next run's abort signal (see `AgentInner::pending_abort`).
    pub fn abort_next_run(&self) {
        self.inner.pending_abort.store(true, Ordering::SeqCst);
    }

    /// TS `agent.transformContext = fn` — the session installs its
    /// forced-prompt projection per run.
    pub fn set_context_transform(&self, transform: Option<TransformContext>) {
        *self.inner.context_transform.lock().expect("context transform") = transform;
    }

    /// TS `waitForIdle()` — resolves after `agent_end` listeners settle
    /// (`finishRun` resolves the run promise).
    pub async fn wait_for_idle(&self) {
        loop {
            let mut notified = std::pin::pin!(self.inner.idle.notified());
            notified.as_mut().enable();
            if !self.has_active_run() {
                return;
            }
            notified.await;
        }
    }

    /// TS `reset()` — clear conversation state and queues while retaining
    /// the replayed prompt/tool baseline.
    pub fn reset(&self) -> Result<(), String> {
        if self.has_active_run() {
            return Err(
                "Agent is already processing. Wait for completion before resetting.".to_string(),
            );
        }
        let baseline = {
            let state = self.inner.state.lock().expect("agent state");
            get_current_system_message(&state.messages)
        };
        {
            let mut state = self.inner.state.lock().expect("agent state");
            state.messages = baseline.map(|message| vec![message]).unwrap_or_default();
            state.is_streaming = false;
            state.streaming_message = None;
            state.pending_tool_calls.clear();
            state.error_message = None;
        }
        let mut queues = self.inner.queues.lock().expect("agent queues");
        queues.clear_follow_up_queue();
        queues.clear_steering_queue();
        Ok(())
    }

    /// TS `prompt(input)` — start a new prompt from a batch of messages.
    /// Guard errors are byte-exact; the returned future is the rest of
    /// `runWithLifecycle`.
    pub fn prompt(&self, messages: Vec<Value>) -> Result<AgentRun, String> {
        if self.has_active_run() {
            return Err("Agent is already processing a prompt. Use steer() or followUp() to queue messages, or wait for completion.".to_string());
        }
        Ok(self.start_run(RunKind::Prompt { messages }, false))
    }

    /// TS `normalizePromptInput(input)` for the string form — a user
    /// message with `timestamp: Date.now()`.
    pub fn normalize_prompt_input(&self, input: &str) -> Value {
        json!({
            "role": "user",
            "content": [{ "type": "text", "text": input }],
            "timestamp": js_number(self.inner.clock.date_now_ms()),
        })
    }

    /// TS `continue()` — continue from the current transcript. The last
    /// message must be a user or tool-result message (or a queue drain).
    pub fn continue_run(&self) -> Result<AgentRun, String> {
        if self.has_active_run() {
            return Err(
                "Agent is already processing. Wait for completion before continuing.".to_string(),
            );
        }
        let (last_role, all_system) = {
            let state = self.inner.state.lock().expect("agent state");
            let all_system = state.messages.iter().all(|m| m["role"] == json!("system"));
            let last_role = state
                .messages
                .last()
                .and_then(|m| m.get("role").and_then(Value::as_str))
                .map(str::to_string);
            (last_role, all_system)
        };
        if last_role.is_none() || all_system {
            return Err("No messages to continue from".to_string());
        }
        if last_role.as_deref() == Some("assistant") {
            let queued_steering = self.drain_steering();
            if !queued_steering.is_empty() {
                return Ok(self.start_run(RunKind::Prompt { messages: queued_steering }, true));
            }
            let queued_follow_ups = self.drain_follow_up();
            if !queued_follow_ups.is_empty() {
                return Ok(self.start_run(RunKind::Prompt { messages: queued_follow_ups }, false));
            }
            return Err("Cannot continue from message role: assistant".to_string());
        }
        Ok(self.start_run(RunKind::Continue, false))
    }

    /// An in-loop continuation (TS `finishTurn` returning `continue`): no
    /// `agent_start`, no entry guards, no queue drain.
    pub fn resume_run(&self) -> Result<AgentRun, String> {
        if self.has_active_run() {
            return Err(
                "Agent is already processing. Wait for completion before continuing.".to_string(),
            );
        }
        Ok(self.start_run(RunKind::Resume, true))
    }

    /// TS `runWithLifecycle` — the synchronous prefix (install active run,
    /// isStreaming on, streaming/error cleared) plus the remainder as an
    /// owned future: executor → `handleRunFailure` on error → `finishRun`.
    fn start_run(&self, kind: RunKind, skip_initial_steering_poll: bool) -> AgentRun {
        {
            let mut state = self.inner.state.lock().expect("agent state");
            state.is_streaming = true;
            state.streaming_message = None;
            state.error_message = None;
        }
        let abort = AbortHandle::new();
        if self.inner.pending_abort.swap(false, Ordering::SeqCst) {
            abort.abort();
        }
        *self.inner.active_run.lock().expect("active run") =
            Some(ActiveRun { abort: abort.clone() });

        let config = self.create_loop_config(skip_initial_steering_poll);
        let context = self.create_context_snapshot();
        let stream_fn = Arc::clone(&self.inner.stream_fn);
        let agent = self.clone();
        Box::pin(async move {
            let sink = EventForwarder { agent: agent.clone() };
            let result = match kind {
                RunKind::Prompt { messages } => {
                    run_agent_loop(&messages, context, &config, &sink, Some(&abort), stream_fn.as_ref())
                        .await
                        .map(|_| ())
                }
                RunKind::Continue => {
                    run_agent_loop_continue(context, &config, &sink, Some(&abort), stream_fn.as_ref())
                        .await
                        .map(|_| ())
                }
                RunKind::Resume => {
                    run_agent_loop_resume(context, &config, &sink, Some(&abort), stream_fn.as_ref())
                        .await
                        .map(|_| ())
                }
            };
            let outcome = match result {
                Ok(()) => Ok(()),
                Err(message) => agent.handle_run_failure(&message, abort.is_aborted()).await,
            };
            agent.finish_run();
            outcome
        })
    }

    /// TS `createContextSnapshot()`.
    fn create_context_snapshot(&self) -> crate::types::AgentContext {
        let state = self.inner.state.lock().expect("agent state");
        crate::types::AgentContext {
            messages: state.messages.clone(),
            tools: Some(state.tools.clone()),
        }
    }

    /// TS `createLoopConfig(options)` — reasoning mapping, steering poll
    /// with `skipInitialSteeringPoll`, follow-up poll, default
    /// `convertToLlm`, default tool execution.
    fn create_loop_config(&self, skip_initial_steering_poll: bool) -> LoopConfig {
        let skip = Arc::new(Mutex::new(skip_initial_steering_poll));
        let steering_agent = self.clone();
        let get_steering_messages: Arc<MessagePollHook> = Arc::new(move || {
            let agent = steering_agent.clone();
            let skip = Arc::clone(&skip);
            Box::pin(async move {
                let mut skip = skip.lock().expect("steering skip flag");
                if *skip {
                    *skip = false;
                    return Vec::new();
                }
                agent.drain_steering()
            })
        });
        let follow_up_agent = self.clone();
        let get_follow_up_messages: Arc<MessagePollHook> = Arc::new(move || {
            let agent = follow_up_agent.clone();
            Box::pin(async move { agent.drain_follow_up() })
        });
        let state = self.inner.state.lock().expect("agent state");
        LoopConfig {
            model: state.model.clone(),
            reasoning: if state.thinking_level == "off" {
                None
            } else {
                Some(state.thinking_level.clone())
            },
            tool_execution: self.inner.tool_execution,
            get_steering_messages: Some(get_steering_messages),
            get_follow_up_messages: Some(get_follow_up_messages),
            clock: Arc::clone(&self.inner.clock),
            transform_context: self.inner.context_transform.lock().expect("context transform").clone(),
            ..LoopConfig::default()
        }
    }

    /// TS `handleRunFailure(error, aborted)` — emit the synthetic
    /// assistant error message (EMPTY_USAGE) as message_start/message_end/
    /// turn_end(toolResults [])/agent_end with `messages: [failureMessage]`.
    /// Model fields absent from `state.model` are omitted (TS
    /// `JSON.stringify` drops `undefined`).
    async fn handle_run_failure(&self, message: &str, aborted: bool) -> Result<(), String> {
        let model = self.inner.state.lock().expect("agent state").model.clone();
        let mut failure = Map::new();
        failure.insert("role".to_string(), json!("assistant"));
        failure.insert("content".to_string(), json!([{ "type": "text", "text": "" }]));
        for (key, source) in [("api", "api"), ("provider", "provider"), ("model", "id")] {
            if let Some(value) = model.get(source) {
                failure.insert(key.to_string(), value.clone());
            }
        }
        failure.insert("usage".to_string(), empty_usage());
        failure.insert(
            "stopReason".to_string(),
            json!(if aborted { "aborted" } else { "error" }),
        );
        failure.insert("errorMessage".to_string(), json!(message));
        failure.insert(
            "timestamp".to_string(),
            js_number(self.inner.clock.date_now_ms()),
        );
        let failure = Value::Object(failure);

        self.process_events(AgentEvent::MessageStart { message: failure.clone() }).await;
        self.process_events(AgentEvent::MessageEnd { message: failure.clone() }).await;
        self.process_events(AgentEvent::TurnEnd { message: failure.clone(), tool_results: Vec::new() })
            .await;
        self.process_events(AgentEvent::AgentEnd { messages: vec![failure] }).await;
        Ok(())
    }

    /// TS `finishRun()`.
    fn finish_run(&self) {
        {
            let mut state = self.inner.state.lock().expect("agent state");
            state.is_streaming = false;
            state.streaming_message = None;
            state.pending_tool_calls.clear();
        }
        *self.inner.active_run.lock().expect("active run") = None;
        self.inner.idle.notify_waiters();
    }

    /// TS `processEvents(event)` — reduce internal state for a loop event,
    /// then await listeners in order with the active run's signal. The
    /// no-active-run case is unreachable through the loop (the run installs
    /// its `ActiveRun` before the executor starts and clears it only in
    /// `finishRun`), matching the TS throw site.
    pub async fn process_events(&self, event: AgentEvent) {
        match &event {
            AgentEvent::MessageStart { message } | AgentEvent::MessageUpdate { message, .. } => {
                self.inner.state.lock().expect("agent state").streaming_message =
                    Some(message.clone());
            }
            AgentEvent::MessageEnd { message } => {
                let mut state = self.inner.state.lock().expect("agent state");
                state.streaming_message = None;
                state.messages.push(message.clone());
            }
            AgentEvent::ToolExecutionStart { tool_call_id, .. } => {
                let mut state = self.inner.state.lock().expect("agent state");
                if !state.pending_tool_calls.contains(tool_call_id) {
                    state.pending_tool_calls.push(tool_call_id.clone());
                }
            }
            AgentEvent::ToolExecutionEnd { tool_call_id, .. } => {
                self.inner
                    .state
                    .lock()
                    .expect("agent state")
                    .pending_tool_calls
                    .retain(|id| id != tool_call_id);
            }
            AgentEvent::TurnEnd { message, .. } => {
                let is_error = message.get("role") == Some(&json!("assistant"))
                    && message
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty());
                if is_error {
                    let mut state = self.inner.state.lock().expect("agent state");
                    state.error_message = message
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
            AgentEvent::AgentEnd { .. } => {
                self.inner.state.lock().expect("agent state").streaming_message = None;
            }
            AgentEvent::AgentStart
            | AgentEvent::TurnStart
            | AgentEvent::ToolExecutionUpdate { .. } => {}
        }

        let signal = self
            .inner
            .active_run
            .lock()
            .expect("active run")
            .as_ref()
            .map(|run| run.abort.clone())
            .expect("Agent listener invoked outside active run");
        let listeners: Vec<Arc<AgentListener>> = self
            .inner
            .listeners
            .lock()
            .expect("agent listeners")
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect();
        for listener in listeners {
            listener(event.clone(), signal.clone()).await;
        }
    }
}

// ============================================================================
// Tests — mirror agent.test.ts behaviors observable through this port
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::FixedClock;
    use crate::provider::StreamOutput;
    use crate::types::BoxFuture;
    use serde_json::json;
    use std::sync::Mutex as StdMutex;

    fn user(text: &str) -> Value {
        json!({ "role": "user", "content": [{ "type": "text", "text": text }], "timestamp": 1 })
    }

    fn assistant(stop_reason: &str) -> Value {
        json!({ "role": "assistant", "content": [], "stopReason": stop_reason, "timestamp": 1 })
    }

    /// Fake provider popping scripted turns; exhaustion throws.
    fn scripted_provider(turns: Vec<Value>) -> Arc<LoopStreamFn> {
        let turns = Arc::new(StdMutex::new(turns));
        Arc::new(move |_model: &Value, _context: &Value, _options: &Value| {
            let turns = Arc::clone(&turns);
            Box::pin(async move {
                let mut queue = turns.lock().unwrap();
                if queue.is_empty() {
                    return Err("provider script exhausted".to_string());
                }
                let turn = queue.remove(0);
                Ok(StreamOutput {
                    events: turn
                        .get("events")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                        .into(),
                    result: turn.get("result").cloned().unwrap_or(Value::Null),
                })
            }) as BoxFuture<'static, Result<StreamOutput, String>>
        })
    }

    fn agent_with(turns: Vec<Value>) -> Agent {
        Agent::new(AgentOptions::with_defaults(
            scripted_provider(turns),
            Arc::new(FixedClock::new(vec![42.0], vec![])),
        ))
    }

    fn recording(agent: &Agent) -> Arc<StdMutex<Vec<Value>>> {
        let events = Arc::new(StdMutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        agent.subscribe(Arc::new(move |event: AgentEvent, _signal: AbortHandle| {
            sink.lock().unwrap().push(event.to_value());
            Box::pin(async {}) as BoxFuture<'static, ()>
        }));
        events
    }

    fn done_turn(result: Value) -> Value {
        json!({ "events": [{ "type": "done", "reason": "stop" }], "result": result })
    }

    #[test]
    fn prompt_happy_path_appends_messages_and_clears_runtime_state() {
        let agent = agent_with(vec![done_turn(assistant("stop"))]);
        let events = recording(&agent);
        let run = agent.prompt(vec![user("hi")]).expect("prompt starts");
        crate::run_current_thread(async {
            run.await.expect("run completes");
            agent.wait_for_idle().await;
        });
        let state = agent.state_snapshot();
        // createMutableAgentState unshifted the (empty) system baseline? No —
        // no systemPrompt and no tools → no initial message.
        assert_eq!(state["messages"].as_array().unwrap().len(), 2);
        assert_eq!(state["messages"][0]["role"], json!("user"));
        assert_eq!(state["messages"][1]["role"], json!("assistant"));
        assert_eq!(state["isStreaming"], json!(false));
        assert_eq!(state["streamingMessage"], Value::Null);
        assert_eq!(state["errorMessage"], Value::Null);
        let types: Vec<String> = events
            .lock()
            .unwrap()
            .iter()
            .map(|e| e["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            types,
            ["agent_start", "turn_start", "message_start", "message_end", "message_start",
             "message_end", "turn_end", "agent_end"]
        );
    }

    #[test]
    fn initial_state_unshifts_system_message_with_tool_declarations() {
        struct NoopTool;
        impl AgentTool for NoopTool {
            fn name(&self) -> &str { "noop" }
            fn description(&self) -> &str { "does nothing" }
            fn label(&self) -> &str { "noop" }
            fn parameters(&self) -> &Value { &Value::Null }
            fn execute<'a>(
                &'a self,
                _id: &'a str,
                _args: &'a Value,
                _signal: Option<&'a AbortHandle>,
                _on_update: Option<&'a crate::types::AgentToolUpdateCallback<'a>>,
            ) -> BoxFuture<'a, Result<crate::types::AgentToolResult, String>> {
                Box::pin(async { Ok(crate::types::AgentToolResult::default()) })
            }
        }
        let agent = Agent::new(AgentOptions {
            tools: vec![Arc::new(NoopTool)],
            system_prompt: Some("be helpful".to_string()),
            ..AgentOptions::with_defaults(
                scripted_provider(vec![]),
                Arc::new(FixedClock::new(vec![1.0], vec![])),
            )
        });
        let state = agent.state_snapshot();
        assert_eq!(state["messages"][0]["role"], json!("system"));
        assert_eq!(state["messages"][0]["content"], json!("be helpful"));
        assert_eq!(state["messages"][0]["toolsAdded"][0]["name"], json!("noop"));
        assert_eq!(state["messages"][0]["timestamp"], json!(0));
        assert_eq!(state["toolNames"], json!(["noop"]));
    }

    #[test]
    fn reentrant_prompt_throws_byte_exact_error() {
        let agent = agent_with(vec![done_turn(assistant("stop")), done_turn(assistant("stop"))]);
        let first = agent.prompt(vec![user("hi")]).expect("first prompt starts");
        let err = match agent.prompt(vec![user("again")]) {
            Ok(_) => panic!("second prompt rejected"),
            Err(err) => err,
        };
        assert_eq!(
            err,
            "Agent is already processing a prompt. Use steer() or followUp() to queue messages, or wait for completion."
        );
        assert!(agent.is_streaming());
        crate::run_current_thread(async {
            first.await.expect("run completes");
            agent.wait_for_idle().await;
        });
        assert!(!agent.is_streaming());
    }

    #[test]
    fn continue_guards() {
        let agent = agent_with(vec![]);
        let err = match agent.continue_run() {
            Ok(_) => panic!("guard expected"),
            Err(err) => err,
        };
        assert_eq!(err, "No messages to continue from");

        let agent = agent_with(vec![done_turn(assistant("stop"))]);
        crate::run_current_thread(async {
            agent.prompt(vec![user("hi")]).unwrap().await.unwrap();
        });
        let err = match agent.continue_run() {
            Ok(_) => panic!("guard expected"),
            Err(err) => err,
        };
        assert_eq!(err, "Cannot continue from message role: assistant");
    }

    #[test]
    fn provider_exhaustion_produces_synthetic_error_message() {
        let agent = agent_with(vec![]);
        let events = recording(&agent);
        crate::run_current_thread(async {
            agent.prompt(vec![user("hi")]).unwrap().await.unwrap();
            agent.wait_for_idle().await;
        });
        let state = agent.state_snapshot();
        assert_eq!(state["errorMessage"], json!("provider script exhausted"));
        let messages = state["messages"].as_array().unwrap();
        let failure = messages.last().unwrap();
        assert_eq!(failure["role"], json!("assistant"));
        assert_eq!(failure["stopReason"], json!("error"));
        assert_eq!(failure["errorMessage"], json!("provider script exhausted"));
        assert_eq!(failure["usage"], empty_usage());
        assert_eq!(failure["api"], json!("unknown"));
        assert_eq!(failure["timestamp"], json!(42));
        let types: Vec<String> = events
            .lock()
            .unwrap()
            .iter()
            .map(|e| e["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            types,
            ["agent_start", "turn_start", "message_start", "message_end", "message_start",
             "message_end", "turn_end", "agent_end"]
        );
        // agent_end carried only the failure message.
        let end = events.lock().unwrap().last().unwrap().clone();
        assert_eq!(end["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn abort_during_pending_run_yields_aborted_stop_reason() {
        let agent = agent_with(vec![]);
        let run = agent.prompt(vec![user("hi")]).expect("prompt starts");
        agent.abort();
        crate::run_current_thread(async {
            run.await.expect("run completes");
        });
        let state = agent.state_snapshot();
        assert_eq!(state["messages"].as_array().unwrap().last().unwrap()["stopReason"], json!("aborted"));
    }

    #[test]
    fn reset_reduces_to_system_baseline_and_clears_queues() {
        let agent = agent_with(vec![done_turn(assistant("stop"))]);
        crate::run_current_thread(async {
            agent.prompt(vec![user("hi")]).unwrap().await.unwrap();
        });
        agent.steer(user("queued"));
        agent.follow_up(user("later"));
        agent.reset().expect("reset when idle");
        let state = agent.state_snapshot();
        // No system baseline existed (no systemPrompt/tools) → empty.
        assert_eq!(state["messages"].as_array().unwrap().len(), 0);
        assert!(!agent.has_queued_messages());

        let agent = Agent::new(AgentOptions {
            system_prompt: Some("base".to_string()),
            ..AgentOptions::with_defaults(
                scripted_provider(vec![done_turn(assistant("stop"))]),
                Arc::new(FixedClock::new(vec![1.0], vec![])),
            )
        });
        crate::run_current_thread(async {
            agent.prompt(vec![user("hi")]).unwrap().await.unwrap();
        });
        agent.reset().unwrap();
        let state = agent.state_snapshot();
        assert_eq!(state["messages"].as_array().unwrap().len(), 1);
        assert_eq!(state["messages"][0]["role"], json!("system"));
        assert_eq!(state["messages"][0]["content"], json!("base"));
    }

    #[test]
    fn steer_before_prompt_is_drained_by_first_poll() {
        let agent = agent_with(vec![done_turn(assistant("stop"))]);
        let steer = json!({ "role": "user", "content": [{ "type": "text", "text": "steered" }], "timestamp": 7 });
        agent.steer(steer.clone());
        crate::run_current_thread(async {
            agent.prompt(vec![user("hi")]).unwrap().await.unwrap();
        });
        let messages = agent.messages();
        let texts: Vec<&str> = messages
            .iter()
            .filter(|m| m["role"] == json!("user"))
            .filter_map(|m| m["content"][0]["text"].as_str())
            .collect();
        assert_eq!(texts, ["hi", "steered"]);
    }

    #[test]
    fn continue_from_assistant_with_follow_up_runs_follow_ups() {
        let agent = agent_with(vec![done_turn(assistant("stop")), done_turn(assistant("stop"))]);
        crate::run_current_thread(async {
            agent.prompt(vec![user("hi")]).unwrap().await.unwrap();
        });
        agent.follow_up(user("and then?"));
        crate::run_current_thread(async {
            agent.continue_run().unwrap().await.unwrap();
        });
        let messages = agent.messages();
        let texts: Vec<&str> = messages
            .iter()
            .filter(|m| m["role"] == json!("user"))
            .filter_map(|m| m["content"][0]["text"].as_str())
            .collect();
        assert_eq!(texts, ["hi", "and then?"]);
    }

    #[test]
    fn queue_surface_round_trip() {
        let agent = agent_with(vec![]);
        agent.steer(user("a"));
        agent.steer(user("b"));
        assert!(agent.has_queued_messages());
        assert_eq!(agent.peek_queued_messages().len(), 1);
        agent.set_steering_mode(QueueMode::All);
        assert_eq!(agent.peek_queued_messages().len(), 2);
        assert_eq!(agent.steering_mode(), QueueMode::All);
        agent.clear_all_queues();
        assert!(!agent.has_queued_messages());
        assert_eq!(agent.system_prompt(), "");
    }
}
