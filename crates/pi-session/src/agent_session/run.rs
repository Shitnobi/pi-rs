//! TS `AgentSession` run orchestration (`agent-session.ts` ~1830–2005):
//! `_startRun`, `_prepareRun` (before_agent_start + prompt/tool-loadout
//! system patch), `_handlePostAgentRun`, `_runBeforeSettleBoundary`, the
//! boundary draft/context helpers (~947–1038), `_recordSelection`,
//! `_finishCancelledRetry`, and `abort()`.
//!
//! **Documented divergences:** (1) the turn_end boundary dispatches at
//! TurnEnd event-pump time (post-loop); a `continue` from a turn_end
//! handler re-enters the agent loop through
//! [`AgentSession::resume_turn_end_continuation`] (a resume run that emits
//! `turn_start` but no `agent_start`, and whose run-final `agent_end` is
//! suppressed while a continuation is pending — see
//! `handle_agent_event`). (2) `_pendingBashMessages`
//! flush calls are tranche-15 TODOs (the queue itself is unported, so the
//! flush is a no-op). (3) `_runBeforeSettleBoundary`'s emitBoundary loop
//! lives here, driven by [`ExtensionHost::boundary_handlers`] /
//! [`ExtensionHost::emit_boundary_handler`] (the real runner's loop is
//! Phase 4+; the TS parity fake mirrors it exactly).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use pi_core::ai::js_number;
use pi_core::compaction::estimate_projected_context_tokens;
use pi_core::session_manager::SessionManager;
use pi_providers::virtual_models::{get_branch_selection, is_virtual_model};

use super::prompt::RunTrigger;
use super::AgentSession;

impl AgentSession {
    /// TS `_startRun(trigger, onStarted?)`.
    pub async fn start_run(
        &mut self,
        trigger: RunTrigger,
        on_started: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<(), String> {
        self.state.is_agent_run_active.store(true, Ordering::SeqCst);
        self.state.agent_run_abort_requested.store(false, Ordering::SeqCst);
        let messages = match self.prepare_run(&trigger).await {
            Ok(messages) => messages,
            Err(error) => {
                self.state.is_agent_run_active.store(false, Ordering::SeqCst);
                // TODO(tranche 15): `_flushPendingBashMessages()`.
                self.flush_pending_custom_messages();
                self.resolve_idle_wait_if_idle();
                return Err(error);
            }
        };
        // TODO(tranche 15): `_flushPendingBashMessages()`.
        self.flush_pending_custom_messages();
        if let Some(callback) = &on_started {
            callback();
        }
        // Compaction before the prompt may have scheduled a retry; the new
        // prompt replaces it.
        self.state.failed_response = None;
        self.record_selection();
        // The run records the loadout in the transcript; restored tools
        // that did not register by now are dropped.
        self.state.tools.pending_tool_names.clear();
        let result = self.drive_run(messages).await;
        // TS `finally`.
        if self.state.agent_run_abort_requested.load(Ordering::SeqCst) {
            self.finish_cancelled_retry();
        }
        self.state.failed_response = None;
        self.state.tools.run_system_prompt_options = None;
        // TODO(tranche 15): `_flushPendingBashMessages()`.
        self.flush_pending_custom_messages();
        self.emit_agent_settled().await?;
        result
    }

    /// The `_startRun` try body: run the agent, then re-dispatch undelivered
    /// queue entries (`agent.continue()` while messages remain queued).
    pub(super) async fn drive_run(&mut self, messages: Vec<Value>) -> Result<(), String> {
        let outcome = {
            let mut bridge_rx = self.take_bridge_rx();
            let result = async {
            // TS installs `agent.transformContext` at construction and reads
            // the forced prompt / hidden declarations live at request time;
            // the Rust agent takes the projection as a per-run hook.
            self.install_request_projection();
            // An abort requested while the run was starting (e.g. during
            // before_agent_start) reaches the agent before its first request
            // (TS re-aborts on the agent_start event, which the Rust session
            // only pumps after the run).
            if self.state.agent_run_abort_requested.load(Ordering::SeqCst) {
                self.agent.abort_next_run();
            }
            self.state.turn_end_boundary_continue = false;
            let prompt_run = self.agent.prompt(messages)?;
            run_with_bridge(self, prompt_run, &mut bridge_rx).await?;
            self.pump_agent_events().await;
            self.resume_turn_end_continuation_with_bridge(&mut bridge_rx).await;
            while !self.state.agent_run_abort_requested.load(Ordering::SeqCst) {
                if self.handle_post_agent_run().await? {
                    if self.state.agent_run_abort_requested.load(Ordering::SeqCst) {
                        break;
                    }
                    self.state.turn_end_boundary_continue = false;
                    let run = self.agent.continue_run()?;
                    run_with_bridge(self, run, &mut bridge_rx).await?;
                    self.pump_agent_events().await;
                    self.resume_turn_end_continuation_with_bridge(&mut bridge_rx).await;
                    continue;
                }
                let boundary_continue = self.run_before_settle_boundary().await;
                if self.state.agent_run_abort_requested.load(Ordering::SeqCst)
                    || !boundary_continue
                {
                    break;
                }
                if self.state.agent_run_abort_requested.load(Ordering::SeqCst) {
                    break;
                }
                self.state.turn_end_boundary_continue = false;
                let run = self.agent.continue_run()?;
                run_with_bridge(self, run, &mut bridge_rx).await?;
                self.pump_agent_events().await;
                self.resume_turn_end_continuation_with_bridge(&mut bridge_rx).await;
            }
            Ok(())
            }
            .await;
            self.restore_bridge_rx(bridge_rx);
            result
        };
        self.pump_agent_events().await;
        outcome
    }

    /// Take the projection-bridge receiver for the duration of a run (the
    /// hook's sender is installed once at construction and never closes).
    fn take_bridge_rx(&mut self) -> Option<tokio::sync::mpsc::UnboundedReceiver<super::model::ProjectionCall>> {
        self.bridge_rx.lock().expect("bridge receiver").take()
    }

    fn restore_bridge_rx(&mut self, rx: Option<tokio::sync::mpsc::UnboundedReceiver<super::model::ProjectionCall>>) {
        *self.bridge_rx.lock().expect("bridge receiver") = rx;
    }

    /// `run_with_bridge` for the resume runs of turn_end continuations.
    /// The receiver is passed through (it is not `Clone`), so the session
    /// keeps servicing forwarded hook calls across the whole drive loop.
    async fn resume_turn_end_continuation_with_bridge(
        &mut self,
        bridge_rx: &mut Option<tokio::sync::mpsc::UnboundedReceiver<super::model::ProjectionCall>>,
    ) {
        while self.state.turn_end_boundary_continue {
            self.state.turn_end_boundary_continue = false;
            if self.state.agent_run_abort_requested.load(Ordering::SeqCst) {
                self.agent.abort_next_run();
            }
            let run = match self.agent.resume_run() {
                Ok(run) => run,
                Err(_) => break,
            };
            if run_with_bridge(self, run, bridge_rx).await.is_err() {
                break;
            }
            self.pump_agent_events().await;
        }
    }

    /// Install the session's request projections on the agent for this run
    /// (TS `agent.transformContext`).
    fn install_request_projection(&mut self) {
        let forced = self
            .state
            .tools
            .run_system_prompt_options
            .as_ref()
            .and_then(|o| o.get("forceSystemPrompt"))
            .filter(|v| !v.is_null())
            .and_then(Value::as_str)
            .map(String::from);
        let hidden = self.state.tools.hidden_declarations.clone();
        if forced.is_none() && hidden.is_empty() {
            self.agent.set_context_transform(None);
            return;
        }
        let clock = Arc::clone(&self.clock);
        self.agent.set_context_transform(Some(pi_agent::TransformContext::Custom(Arc::new(
            move |messages: &[Value]| {
                super::tools::apply_request_projections(
                    messages.to_vec(),
                    &hidden,
                    forced.as_deref(),
                    clock.date_now_ms(),
                )
            },
        ))));
    }

    /// TS `_prepareRun(trigger)`.
    pub(super) async fn prepare_run(&mut self, trigger: &RunTrigger) -> Result<Vec<Value>, String> {
        // Check if we need to compact before sending (catches aborted responses).
        // The starting message is sent below, so do not call agent.continue() here.
        if let Some(last_assistant) = self.find_last_assistant_message() {
            self.check_compaction(last_assistant, false, &[]).await?;
        }

        let selected_tools_before: Vec<Value> = self
            .state
            .tools
            .base_system_prompt_options
            .selected_tools
            .iter()
            .map(|name| json!(name))
            .collect();
        let base_options = self.base_system_prompt_options_value();
        let trigger_json = trigger.to_json();
        let host = Arc::clone(&self.extension_host);
        let (handler_messages, mut options, set_active_tools_requests) =
            host.emit_before_agent_start(trigger_json, base_options).await;
        // TS `ctx.setActiveTools(names)` — handlers call it mid-emission;
        // applied here (documented divergence, see the trait docs).
        for names in set_active_tools_requests {
            self.set_active_tools_by_name(names);
        }
        // Handlers may edit selectedTools or call setActiveTools() (the
        // live loadout). An explicit edit wins; otherwise the live loadout
        // is authoritative.
        let selected_after: Vec<Value> =
            options.get("selectedTools").and_then(Value::as_array).cloned().unwrap_or_default();
        let handler_edited = selected_after.len() != selected_tools_before.len()
            || selected_after
                .iter()
                .zip(selected_tools_before.iter())
                .any(|(a, b)| a != b);
        if !handler_edited {
            let names: Vec<Value> =
                self.get_active_tool_names().iter().map(|n| json!(n)).collect();
            if let Some(object) = options.as_object_mut() {
                object.insert("selectedTools".to_string(), Value::Array(names));
            }
        }
        let mut messages: Vec<Value> = Vec::new();
        match trigger {
            RunTrigger::Custom { message } => messages.push(message.clone()),
            RunTrigger::User { prompt, images } => {
                // Normalize images after before_agent_start so
                // extension-driven model selection determines the profile.
                let (normalized_images, hints) =
                    self.normalize_prompt_images(images.as_deref()).await;
                let text = if hints.is_empty() {
                    prompt.clone()
                } else {
                    format!("{}\n\n{}", prompt, hints.join("\n"))
                };
                let mut content = vec![json!({ "type": "text", "text": text })];
                content.extend(normalized_images);
                messages.push(json!({
                    "role": "user",
                    "content": content,
                    "timestamp": js_number(self.clock.date_now_ms()),
                }));
            }
        }
        messages.append(&mut self.state.pending_next_turn_messages);
        for msg in &handler_messages {
            let mut object = Map::new();
            object.insert("role".to_string(), json!("custom"));
            object.insert(
                "customType".to_string(),
                msg.get("customType").cloned().unwrap_or(Value::Null),
            );
            // Untyped extensions can pass null/missing content; normalize.
            let content = match msg.get("content") {
                Some(content) if !content.is_null() => content.clone(),
                _ => json!([]),
            };
            object.insert("content".to_string(), content);
            if let Some(display) = msg.get("display") {
                if !display.is_null() {
                    object.insert("display".to_string(), display.clone());
                }
            }
            if let Some(details) = msg.get("details") {
                if !details.is_null() {
                    object.insert("details".to_string(), details.clone());
                }
            }
            object.insert("timestamp".to_string(), js_number(self.clock.date_now_ms()));
            messages.push(Value::Object(object));
        }
        let update_message = self.prepare_prompt_and_tool_loadout(&mut options);
        self.state.tools.run_system_prompt_options = Some(options);
        if let Some(update) = update_message {
            messages.insert(0, update);
        }
        Ok(messages)
    }

    /// TS `_handlePostAgentRun()`. `Err` models a throw (the
    /// `_omitRecoveryAttempt` projected-message guard); the TS throw
    /// propagates out of `prompt()` after the run's finally.
    pub(super) async fn handle_post_agent_run(&mut self) -> Result<bool, String> {
        let message = self.state.last_assistant_message.take();
        let tool_results = std::mem::take(&mut self.state.last_assistant_tool_results);
        if self.state.agent_run_abort_requested.load(Ordering::SeqCst) {
            self.finish_cancelled_retry();
            return Ok(false);
        }
        let Some(message) = message else {
            return Ok(self.agent.has_queued_messages());
        };
        if self.is_retryable_error(&self.state.arena[message])
            && self.prepare_retry(message).await?
        {
            if self.state.agent_run_abort_requested.load(Ordering::SeqCst) {
                self.finish_cancelled_retry();
            }
            self.state.failed_response = Some(message);
            return Ok(!self.state.agent_run_abort_requested.load(Ordering::SeqCst));
        }
        if self.state.agent_run_abort_requested.load(Ordering::SeqCst) {
            self.finish_cancelled_retry();
            return Ok(false);
        }

        if self.state.arena[message].get("stopReason").and_then(Value::as_str) == Some("error")
            && self.state.retry_attempt > 0.0
        {
            let attempt = self.state.retry_attempt;
            let final_error = self.state.arena[message]
                .get("errorMessage")
                .and_then(Value::as_str)
                .map(str::to_string);
            self.emit(super::events::auto_retry_end_event(
                false,
                attempt,
                final_error.as_deref(),
            ));
            self.state.retry_attempt = 0.0;
        }

        if self.check_compaction(message, true, &tool_results).await? {
            return Ok(!self.state.agent_run_abort_requested.load(Ordering::SeqCst));
        }

        // The low-level loop drains both queues before agent_end. Messages queued by
        // agent_end handlers require a fresh run before pre-settlement handlers fire.
        Ok(!self.state.agent_run_abort_requested.load(Ordering::SeqCst)
            && self.agent.has_queued_messages())
    }

    /// TS `_runBeforeSettleBoundary()`.
    pub(super) async fn run_before_settle_boundary(&mut self) -> bool {
        if self.extension_host.boundary_handlers("agent_before_settle") == 0 {
            return self.agent.has_queued_messages();
        }
        self.state.is_before_settle.store(true, Ordering::SeqCst);
        self.state.abort_during_before_settle.store(false, Ordering::SeqCst);
        let outcome: Result<bool, String> = async {
            let mut event = Map::new();
            event.insert("type".to_string(), json!("agent_before_settle"));
            event.insert(
                "outcome".to_string(),
                json!(self.state.last_activity_outcome.clone()),
            );
            let base_event = Value::Object(event);
            let result = self.emit_boundary(base_event, "agent_before_settle").await;
            self.commit_boundary_drafts(&result.entries)?;
            self.flush_pending_custom_messages();
            let final_context = self.build_boundary_context(&[], "agent_before_settle")?;
            if self.state.abort_during_before_settle.load(Ordering::SeqCst) {
                return Ok(false);
            }
            let should_continue = result.continue_ || self.agent.has_queued_messages();
            if should_continue && !final_context.can_continue {
                if result.continue_ {
                    self.report_invalid_boundary_continuation("agent_before_settle");
                }
                return Ok(false);
            }
            Ok(should_continue)
        }
        .await;
        self.state.is_before_settle.store(false, Ordering::SeqCst);
        outcome.unwrap_or(false)
    }

    /// TS `ExtensionRunner.emitBoundary(baseEvent, buildContext)` — the
    /// handler loop threading `entries`/`continue` and rebuilding the
    /// context after each handler; invalid drafts invalidate the result.
    pub(super) async fn emit_boundary(
        &mut self,
        base_event: Value,
        event_type: &str,
    ) -> BoundaryDispatchResult {
        let host = Arc::clone(&self.extension_host);
        let count = host.boundary_handlers(event_type);
        let mut entries: Vec<Value> = Vec::new();
        let mut should_continue = false;
        let mut valid = true;
        let mut context = self.build_boundary_context(&entries, event_type);
        for index in 0..count {
            let context_json = match &context {
                Ok(context) => context.to_json(),
                Err(_) => Value::Null,
            };
            let mut event = base_event.as_object().cloned().unwrap_or_default();
            event.insert("entries".to_string(), json!(entries));
            event.insert("continue".to_string(), json!(should_continue));
            event.insert("context".to_string(), context_json);
            let handler_result =
                host.emit_boundary_handler(index, Value::Object(event)).await;
            if let Some(result) = handler_result {
                if let Some(next) = result.get("entries").filter(|e| !e.is_null()) {
                    entries = next.as_array().cloned().unwrap_or_default();
                }
                if let Some(continue_flag) = result.get("continue").and_then(Value::as_bool) {
                    should_continue = continue_flag;
                }
            }
            match self.build_boundary_context(&entries, event_type) {
                Ok(next) => {
                    valid = true;
                    context = Ok(next);
                }
                Err(error) => {
                    valid = false;
                    self.extension_host.emit_error(json!({
                        "extensionPath": "<boundary>",
                        "event": event_type,
                        "error": format!("Invalid boundary entries: {}", error),
                    }));
                }
            }
        }
        if valid {
            BoundaryDispatchResult { entries, continue_: should_continue }
        } else {
            BoundaryDispatchResult { entries: Vec::new(), continue_: false }
        }
    }

    /// TS `_applyBoundaryDrafts(manager, drafts)` — `Err` models a throw.
    fn apply_boundary_drafts(
        manager: &mut SessionManager,
        drafts: &[Value],
    ) -> Result<Vec<Value>, String> {
        let mut appended: Vec<Value> = Vec::new();
        for draft in drafts {
            let id = match draft.get("type").and_then(Value::as_str).unwrap_or_default() {
                "custom" => manager.append_custom_entry(
                    draft.get("customType").and_then(Value::as_str).unwrap_or_default(),
                    draft.get("data").filter(|d| !d.is_null()).cloned(),
                ),
                "custom_message" => manager.append_custom_message_entry(
                    draft.get("customType").and_then(Value::as_str).unwrap_or_default(),
                    draft.get("content").cloned().unwrap_or(Value::Null),
                    draft.get("display").and_then(Value::as_bool),
                    draft.get("details").filter(|d| !d.is_null()).cloned(),
                ),
                "context_edit" => manager.append_context_edit(
                    draft.get("targetId").and_then(Value::as_str).unwrap_or_default(),
                    draft.get("replacement").filter(|r| !r.is_null()).cloned(),
                )?,
                "compaction" => {
                    let tokens_before = estimate_projected_context_tokens(
                        &manager.build_session_projection(),
                        &manager.get_branch(None),
                    )
                    .tokens;
                    manager.append_compaction(
                        draft.get("summary").and_then(Value::as_str).unwrap_or_default(),
                        draft.get("firstKeptEntryId").and_then(Value::as_str),
                        tokens_before as i64,
                        draft.get("details").filter(|d| !d.is_null()).cloned(),
                        Some(true),
                        draft.get("usage").filter(|u| !u.is_null()).cloned(),
                    )
                }
                // TS has no default arm: unknown types leave `entryId`
                // undefined and the draft is silently skipped.
                _ => continue,
            };
            if let Some(entry) = manager.get_entry(&id) {
                appended.push(entry.clone());
            }
        }
        Ok(appended)
    }

    /// TS `_createBoundaryPreviewManager(drafts)`.
    fn create_boundary_preview_manager(
        &self,
        drafts: &[Value],
    ) -> Result<SessionManager, String> {
        let header = self
            .session_manager
            .get_header()
            .cloned()
            .ok_or_else(|| "Session header is missing".to_string())?;
        let mut entries = vec![header];
        entries.extend(self.session_manager.get_branch(None));
        let gen = self.session_manager.generators().clone();
        let mut manager = SessionManager::in_memory(&self.cwd, None, entries, &gen)?;
        Self::apply_boundary_drafts(&mut manager, drafts)?;
        Ok(manager)
    }

    /// TS `_getPendingBoundaryMessages()`.
    fn get_pending_boundary_messages(&self) -> Vec<Value> {
        let mut messages = self.agent.peek_queued_messages();
        messages.extend(self.state.pending_custom_messages.iter().cloned());
        messages
    }

    /// TS `_buildBoundaryContext(drafts, boundary)`.
    pub(super) fn build_boundary_context(
        &self,
        drafts: &[Value],
        boundary: &str,
    ) -> Result<BoundaryContext, String> {
        let manager = self.create_boundary_preview_manager(drafts)?;
        let projection = manager.build_session_projection();
        let pending_messages = self.get_pending_boundary_messages();
        let context_messages: Vec<Value> = projection
            .messages
            .iter()
            .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
            .collect();
        // TS `_buildBoundaryContext` uses the session-side `convertToLlm`
        // (core/messages.ts): custom/bashExecution/branchSummary/
        // compactionSummary roles become user messages — unlike the agent
        // loop's `defaultConvertToLlm`, which drops them.
        let llm_messages = pi_core::messages::convert_to_llm(&projection.messages);
        let final_role = llm_messages
            .last()
            .and_then(|m| m.get("role").and_then(Value::as_str))
            .map(String::from);
        let has_non_system_context =
            llm_messages.iter().any(|m| m.get("role").and_then(Value::as_str) != Some("system"));
        let context_can_continue = has_non_system_context && final_role.as_deref() != Some("assistant");
        let pending_custom_context = !self.state.pending_custom_messages.is_empty();
        let has_queued = self.agent.has_queued_messages();
        let can_continue = context_can_continue
            || pending_custom_context
            || if boundary == "turn_end" {
                has_queued
            } else {
                final_role.as_deref() == Some("assistant") && has_queued
            };
        Ok(BoundaryContext {
            context_entries: projection
                .entries
                .iter()
                .map(|e| {
                    json!({ "sourceEntry": e.source_entry, "messages": e.messages })
                })
                .collect(),
            context_messages,
            llm_messages,
            pending_messages,
            can_continue,
        })
    }

    /// TS `_commitBoundaryDrafts(drafts)`.
    pub(super) fn commit_boundary_drafts(&mut self, drafts: &[Value]) -> Result<(), String> {
        let appended = {
            let mut manager = std::mem::replace(
                &mut self.session_manager,
                SessionManager::in_memory("", None, Vec::new(), &Default::default())?,
            );
            let result = Self::apply_boundary_drafts(&mut manager, drafts);
            self.session_manager = manager;
            result?
        };
        self.refresh_finalized_context();
        for entry in appended {
            self.emit(json!({ "type": "entry_appended", "entry": entry }));
        }
        Ok(())
    }

    /// TS `_reportInvalidBoundaryContinuation(event)`.
    pub(super) fn report_invalid_boundary_continuation(&self, event: &str) {
        self.extension_host.emit_error(json!({
            "extensionPath": "<boundary>",
            "event": event,
            "error": format!("{} requested continuation without runnable model context", event),
        }));
    }

    /// TS `_recordSelection()` — records the model selection on the
    /// current branch when the branch implies another one (see the TS
    /// doc comment; `_modelRuntime.getModel` is the injected lookup).
    fn record_selection(&mut self) {
        let model = self.agent.model();
        if model.is_null() {
            return;
        }
        let Some(lookup) = self.model_lookup.clone() else { return };
        let branch = self.session_manager.get_branch(None);
        let Some(recorded) = get_branch_selection(&branch, |provider, id| {
            lookup(provider, id).and_then(|value| {
                serde_json::from_value::<pi_providers::pi_ai::model::Model>(value).ok()
            })
        })
        else {
            return;
        };
        let current_provider = model.get("provider").and_then(Value::as_str).unwrap_or_default();
        let current_id = model.get("id").and_then(Value::as_str).unwrap_or_default();
        if recorded.0 == current_provider && recorded.1 == current_id {
            return;
        }
        let recorded_model = lookup(&recorded.0, &recorded.1);
        let current_virtual =
            is_virtual_model(model.get("api").and_then(Value::as_str).unwrap_or_default());
        let recorded_virtual = recorded_model
            .as_ref()
            .map(|m| is_virtual_model(m.get("api").and_then(Value::as_str).unwrap_or_default()))
            .unwrap_or(false);
        if !current_virtual && !recorded_virtual {
            return;
        }
        self.session_manager.append_model_change(current_provider, current_id);
    }

    /// TS `_finishCancelledRetry()`.
    pub(super) fn finish_cancelled_retry(&mut self) {
        if self.state.retry_attempt == 0.0 {
            return;
        }
        let attempt = self.state.retry_attempt;
        self.state.retry_attempt = 0.0;
        self.emit(super::events::auto_retry_end_event(false, attempt, Some("Retry cancelled")));
    }

    /// TS `abort()` — abort the current operation and wait for idle.
    /// Retry/compaction/branch-summary/bash aborts arrive with their
    /// tranches (TODOs below).
    pub async fn abort(&mut self) {
        if self.state.is_agent_run_active.load(Ordering::SeqCst) {
            self.state.agent_run_abort_requested.store(true, Ordering::SeqCst);
        }
        self.abort_retry();
        self.abort_compaction();
        // TODO(tranche 16): this.abortBranchSummary();
        // TODO(tranche 15): this.abortBash();
        if self.state.is_before_settle.load(Ordering::SeqCst) {
            self.state.abort_during_before_settle.store(true, Ordering::SeqCst);
        }
        self.agent.abort();
        self.wait_for_idle().await;
    }
}

/// Await `run` while servicing forwarded request-hook calls (the run
/// future parks inside the hook while each call is served — see the
/// `model` module docs).
async fn run_with_bridge(
    session: &mut AgentSession,
    mut run: pi_agent::types::BoxFuture<'static, Result<(), String>>,
    receiver: &mut Option<tokio::sync::mpsc::UnboundedReceiver<super::model::ProjectionCall>>,
) -> Result<(), String> {
    loop {
        let received = async {
            match receiver.as_mut() {
                Some(rx) => rx.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            outcome = &mut run => return outcome,
            received = received => {
                match received {
                    None => *receiver = None,
                    Some((request, reply)) => {
                        let response = session.service_projection(request).await;
                        let _ = reply.send(response);
                    }
                }
            }
        }
    }
}

/// TS `BoundaryDispatchResult` (the slice the session consumes).
pub(super) struct BoundaryDispatchResult {
    pub entries: Vec<Value>,
    pub continue_: bool,
}

/// TS `BoundaryContextPreview`.
pub(super) struct BoundaryContext {
    pub context_entries: Vec<Value>,
    pub context_messages: Vec<Value>,
    pub llm_messages: Vec<Value>,
    pub pending_messages: Vec<Value>,
    pub can_continue: bool,
}

impl BoundaryContext {
    /// The JSON projection handlers receive (key order per the TS object).
    pub fn to_json(&self) -> Value {
        json!({
            "contextEntries": self.context_entries,
            "contextMessages": self.context_messages,
            "llmMessages": self.llm_messages,
            "pendingMessages": self.pending_messages,
            "canContinue": self.can_continue,
        })
    }
}

impl RunTrigger {
    /// TS `BeforeAgentStartTrigger` JSON (`{...trigger}` spread order).
    pub fn to_json(&self) -> Value {
        match self {
            RunTrigger::User { prompt, images } => {
                let mut object = Map::new();
                object.insert("messageType".to_string(), json!("user"));
                object.insert("prompt".to_string(), json!(prompt));
                if let Some(images) = images {
                    object.insert("images".to_string(), json!(images));
                }
                Value::Object(object)
            }
            RunTrigger::Custom { message } => json!({
                "messageType": "custom",
                "message": message,
            }),
        }
    }
}
