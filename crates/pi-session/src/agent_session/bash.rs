//! Bash-execution slice of `AgentSession` (TS `executeBash` /
//! `recordBashResult` / `abortBash` / `_flushPendingBashMessages`,
//! agent-session.ts ~3860–3965).
//!
//! TS `executeBash` awaits `executeBashWithOperations` (bash-executor.ts),
//! whose `BashOperations.exec` is async; the pi-tools executor entry is
//! synchronous (the bash *tool* seam), so this module defines the async
//! operations seam the session awaits over and reuses the pi-tools
//! executor state + finalize tail for the pipeline (sanitize → spill →
//! rolling buffer → truncate).
//!
//! Concurrency model (plan §2 trap: `&mut self` across awaits): TS callers
//! can hold several `executeBash` promises at once and record their results
//! in completion order. The Rust mirror splits the TS method at its await
//! point: [`AgentSession::start_bash`] runs the synchronous prefix (signal
//! registration, prefix resolution, the emit tap) and returns a `'static`
//! future; [`AgentSession::complete_bash`] is the post-await continuation
//! (`recordBashResult` + the `finally` controller removal).
//! [`AgentSession::execute_bash`] composes the two for single-call callers.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use pi_agent::types::BoxFuture;
use pi_tools::bash_executor::{finalize_bash_execution, BashResult, ExecutorState};
use pi_tools::shell::{
    create_local_bash_operations, AbortSignal, BashOperations, ExecOutcome, ShellExecError,
    ShellExecOptions,
};

use super::AgentSession;

/// TS `BashOperations` as consumed by `executeBashWithOperations` (the
/// session-level seam; `exec` is async there, unlike the bash tool's).
pub trait SessionBashOperations: Send + Sync {
    /// `operations.exec(command, cwd, { onData, signal })` →
    /// `{ exitCode }` (`Err` models a rejection).
    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a str,
        on_data: &'a mut (dyn FnMut(&[u8]) + Send),
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<ExecOutcome, ShellExecError>>;
}

/// `createLocalBashOperations({ shellPath })` adapter. The local shell runs
/// synchronously (the pi-tools seam); production callers on a multithreaded
/// runtime should wrap this in a blocking task — the parity harness always
/// injects scripted operations.
pub struct LocalSessionBashOperations {
    shell_path: Option<String>,
}

impl LocalSessionBashOperations {
    pub fn new(shell_path: Option<&str>) -> Self {
        LocalSessionBashOperations { shell_path: shell_path.map(str::to_string) }
    }
}

impl SessionBashOperations for LocalSessionBashOperations {
    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a str,
        on_data: &'a mut (dyn FnMut(&[u8]) + Send),
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<ExecOutcome, ShellExecError>> {
        let inner = create_local_bash_operations(self.shell_path.as_deref());
        let mut adapted =
            |_kind: pi_tools::shell::StreamKind, data: &[u8]| on_data(data);
        let opts = ShellExecOptions { on_data: Some(&mut adapted), signal, timeout: None, env: None };
        let result = BashOperations::exec(&inner, command, cwd, opts);
        Box::pin(async move { result })
    }
}

/// TS `executeBash` options bag.
#[derive(Default)]
pub struct BashExecutionOptions {
    /// `options.excludeFromContext` (`!!` prefix; `None` = key absent).
    pub exclude_from_context: Option<bool>,
    /// `options.id` — included in `bash_execution_update` events.
    pub id: Option<String>,
    /// `options.operations` — custom operations for remote execution.
    pub operations: Option<Arc<dyn SessionBashOperations>>,
}

/// A started-but-unawaited bash execution (TS: the floating
/// `session.executeBash(...)` promise). Awaiting `future` runs the
/// executor; then feed both to [`AgentSession::complete_bash`].
pub struct PendingBash {
    /// The original command (the prefix-stripped one TS records).
    pub command: String,
    pub exclude_from_context: Option<bool>,
    /// The per-call abort signal (controller-set identity).
    pub signal: AbortSignal,
    /// The executor future (`executeBashWithOperations(...)`).
    pub future: BoxFuture<'static, Result<BashResult, ShellExecError>>,
}

/// The async port of `executeBashWithOperations` over the session seam.
pub async fn execute_bash_with_session_operations(
    command: &str,
    cwd: &str,
    operations: &dyn SessionBashOperations,
    on_chunk: Option<&mut (dyn FnMut(&str) + Send)>,
    signal: Option<&AbortSignal>,
) -> Result<BashResult, ShellExecError> {
    let mut state = ExecutorState::new();
    let mut on_chunk = on_chunk;
    let exec_result = operations
        .exec(command, cwd, &mut |data: &[u8]| {
            // IO errors on the spill file surface as Other in finalize.
            let _ = state.on_data(&mut on_chunk, data);
        }, signal)
        .await;
    finalize_bash_execution(state, on_chunk, exec_result, signal)
}

/// TS `onChunk?: (delta: string) => void` — the per-execution stream
/// callback (mirrors [`crate::agent_session::prompt::PreflightResult`]'s
/// alias for the same shape).
pub type OnChunk = Arc<dyn Fn(&str) + Send + Sync>;

impl AgentSession {
    /// TS `executeBash(command, onChunk?, options?)` — composed form.
    pub async fn execute_bash(
        &mut self,
        command: &str,
        on_chunk: Option<OnChunk>,
        options: BashExecutionOptions,
    ) -> Result<BashResult, String> {
        let pending = self.start_bash(command, on_chunk, options);
        let PendingBash { command, exclude_from_context, signal, future } = pending;
        let result = future.await;
        self.complete_bash(&command, exclude_from_context, &signal, result)
    }

    /// The `executeBash` synchronous prefix: register the per-call abort
    /// signal, resolve `shellCommandPrefix`, and capture the emit tap so
    /// the executor future is `'static` (TS: the promise created by
    /// calling `executeBash`, which runs until the executor's first await).
    pub fn start_bash(
        &mut self,
        command: &str,
        on_chunk: Option<OnChunk>,
        options: BashExecutionOptions,
    ) -> PendingBash {
        let signal = AbortSignal::new();
        self.state.bash_signals.lock().expect("bash abort signals").push(signal.clone());

        // Apply command prefix if configured (e.g. "shopt -s
        // expand_aliases" for alias support).
        let prefix = self.settings.shell_command_prefix();
        let resolved_command = match prefix {
            Some(prefix) if !prefix.is_empty() => format!("{}\n{}", prefix, command),
            _ => command.to_string(),
        };
        let shell_path = self.settings.shell_path();
        let cwd = self.session_manager.get_cwd().to_string();

        // `onChunk: (delta) => { onChunk?.(delta); this._emit({
        // type: "bash_execution_update", id: options?.id, delta }); }`
        let emit_tap = self.emit_tap();
        let id = options.id.clone();
        let mut on_chunk = move |delta: &str| {
            if let Some(callback) = &on_chunk {
                callback(delta);
            }
            let mut event = Map::new();
            event.insert("type".to_string(), json!("bash_execution_update"));
            event.insert("delta".to_string(), json!(delta));
            if let Some(id) = &id {
                event.insert("id".to_string(), json!(id));
            }
            emit_tap(Value::Object(event));
        };

        let operations: Arc<dyn SessionBashOperations> =
            options.operations.unwrap_or_else(|| Arc::new(LocalSessionBashOperations::new(shell_path.as_deref())));

        let future: BoxFuture<'static, Result<BashResult, ShellExecError>> = {
            let command = resolved_command;
            let operations = operations;
            let exec_signal = signal.clone();
            Box::pin(async move {
                execute_bash_with_session_operations(&command, &cwd, operations.as_ref(), Some(&mut on_chunk), Some(&exec_signal)).await
            })
        };

        PendingBash {
            command: command.to_string(),
            exclude_from_context: options.exclude_from_context,
            signal,
            future,
        }
    }

    /// The post-await continuation of `executeBash`: `recordBashResult` on
    /// success, then the `finally` controller removal (TS order). Errors
    /// carry the executor's rejection message.
    pub fn complete_bash(
        &mut self,
        command: &str,
        exclude_from_context: Option<bool>,
        signal: &AbortSignal,
        result: Result<BashResult, ShellExecError>,
    ) -> Result<BashResult, String> {
        let outcome = match result {
            Ok(result) => {
                self.record_bash_result(command, &result, exclude_from_context);
                Ok(result)
            }
            Err(error) => Err(error.message()),
        };
        self.state
            .bash_signals
            .lock()
            .expect("bash abort signals")
            .retain(|s| !AbortSignal::ptr_eq(s, signal));
        outcome
    }

    /// TS `recordBashResult(command, result, options?)` — append the
    /// `bashExecution` message to the session (or queue it while
    /// streaming, flushed at the run checkpoints).
    pub fn record_bash_result(
        &mut self,
        command: &str,
        result: &BashResult,
        exclude_from_context: Option<bool>,
    ) {
        let mut bash_message = Map::new();
        bash_message.insert("role".to_string(), json!("bashExecution"));
        bash_message.insert("command".to_string(), json!(command));
        bash_message.insert("output".to_string(), json!(result.output));
        // `exitCode: undefined` (cancelled) is dropped by JSON
        // serialization, like the TS object literal.
        if let Some(exit_code) = result.exit_code {
            bash_message.insert("exitCode".to_string(), json!(exit_code));
        }
        bash_message.insert("cancelled".to_string(), json!(result.cancelled));
        bash_message.insert("truncated".to_string(), json!(result.truncated));
        if let Some(path) = &result.full_output_path {
            bash_message.insert("fullOutputPath".to_string(), json!(path));
        }
        bash_message.insert(
            "timestamp".to_string(),
            pi_core::ai::js_number(self.clock.date_now_ms()),
        );
        if let Some(exclude) = exclude_from_context {
            bash_message.insert("excludeFromContext".to_string(), json!(exclude));
        }
        let bash_message = Value::Object(bash_message);

        // If agent is streaming, defer adding to avoid breaking
        // tool_use/tool_result ordering — queued for later, flushed on
        // agent_end.
        if self.is_streaming() {
            self.state.pending_bash_messages.push(bash_message);
        } else {
            self.session_manager.append_message(bash_message);
            self.refresh_finalized_context();
        }
    }

    /// TS `abortBash()` — cancel every running bash command.
    pub fn abort_bash(&self) {
        for signal in self.state.bash_signals.lock().expect("bash abort signals").iter() {
            signal.abort();
        }
    }

    /// Shareable `abortBash()` — the TS parity fake operations close over
    /// the session and call `session.abortBash()` mid-execution; the Rust
    /// fake captures this tap instead.
    pub fn bash_abort_tap(&self) -> Arc<dyn Fn() + Send + Sync> {
        let signals = Arc::clone(&self.state.bash_signals);
        Arc::new(move || {
            for signal in signals.lock().expect("bash abort signals").iter() {
                signal.abort();
            }
        })
    }
    /// TS `get isBashRunning`.
    pub fn is_bash_running(&self) -> bool {
        !self.state.bash_signals.lock().expect("bash abort signals").is_empty()
    }

    /// TS `get hasPendingBashMessages`.
    pub fn has_pending_bash_messages(&self) -> bool {
        !self.state.pending_bash_messages.is_empty()
    }

    /// TS `_flushPendingBashMessages()` — pub for the parity harness (the
    /// TS harness any-casts the private).
    pub fn flush_pending_bash_messages(&mut self) {
        if self.state.pending_bash_messages.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.state.pending_bash_messages);
        for bash_message in pending {
            self.session_manager.append_message(bash_message);
        }
        self.refresh_finalized_context();
    }
}
