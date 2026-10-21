//! TS `_handleAgentEvent` and friends (agent-session.ts lines 1104–1397):
//! the shared subscribe/reconnect event handler — abort-at-`agent_start`,
//! nested-call record merge, steering/follow-up bookkeeping, extension +
//! public emission, `message_end` persistence, assistant bookkeeping,
//! `turn_end` flush, retry classification, persisted-entry lookup, and
//! `agent_settled` emission with deferred actions.

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use serde_json::{json, Map, Value};

use pi_core::usage_totals::combine_usage_values;

use super::events::{agent_event_json, MsgId, SessionAgentEvent};
use super::{AgentSession, DeferredSettledAction};

/// `entry.type` of a session entry Value.
fn entry_type_of(entry: &Value) -> &str {
    entry.get("type").and_then(Value::as_str).unwrap_or_default()
}

/// `entry.id` of a session entry Value.
fn entry_id_of(entry: &Value) -> &str {
    entry.get("id").and_then(Value::as_str).unwrap_or_default()
}

fn role_of(message: &Value) -> &str {
    message.get("role").and_then(Value::as_str).unwrap_or_default()
}

fn str_field<'a>(message: &'a Value, key: &str) -> Option<&'a str> {
    message.get(key).and_then(Value::as_str)
}

impl AgentSession {
    /// TS `_handleAgentEvent` (bound arrow field — shared by subscribe and
    /// reconnect).
    pub async fn handle_agent_event(&mut self, event: &SessionAgentEvent) {
        // An abort requested while the run was starting, e.g. during
        // before_agent_start, stops it here. agent_start is awaited before
        // the first request.
        if matches!(event, SessionAgentEvent::AgentStart)
            && self.state.agent_run_abort_requested.load(Ordering::SeqCst)
        {
            self.agent.abort();
        }
        // Record the calls a tool made through ctx.executeTool() and their
        // usage on its result message.
        if let Some(nested) = self.nested_tool_calls.clone() {
            match event {
                SessionAgentEvent::MessageStart { message } if role_of(&self.state.arena[*message]) == "toolResult" => {
                    let id = *message;
                    let tool_call_id =
                        str_field(&self.state.arena[id], "toolCallId").unwrap_or_default().to_string();
                    if let Some(summary) = nested.take_record(&tool_call_id) {
                        let mut message_value = self.state.arena[id].clone();
                        let has_calls = summary.get("calls").is_some_and(|c| !c.is_null());
                        if has_calls {
                            message_value.as_object_mut().expect("toolResult message is an object")
                                .insert("nestedCalls".to_string(), summary["calls"].clone());
                        }
                        if let Some(usage) = summary.get("usage").filter(|u| !u.is_null()) {
                            let combined = match message_value.get("usage") {
                                Some(existing) if !existing.is_null() => {
                                    combine_usage_values(existing, usage)
                                }
                                _ => usage.clone(),
                            };
                            message_value.as_object_mut().expect("toolResult message is an object")
                                .insert("usage".to_string(), combined);
                        }
                        self.sync_message(id, message_value);
                    }
                }
                SessionAgentEvent::AgentEnd { .. } => nested.clear(),
                _ => {}
            }
        }
        // When a user message starts, check if it's from either queue and
        // remove it BEFORE emitting. This ensures the UI sees the updated
        // queue state.
        if let SessionAgentEvent::MessageStart { message } = event {
            if role_of(&self.state.arena[*message]) == "user" {
                self.state.overflow_recovery_attempted = false;
                let message_text =
                    pi_core::ai::content_text(self.state.arena[*message].get("content").unwrap_or(&Value::Null), "");
                if !message_text.is_empty() {
                    // Check steering queue first.
                    if let Some(steering_index) =
                        self.state.steering_messages.iter().position(|text| *text == message_text)
                    {
                        self.state.steering_messages.remove(steering_index);
                        self.emit_queue_update();
                    } else if let Some(follow_up_index) =
                        self.state.follow_up_messages.iter().position(|text| *text == message_text)
                    {
                        self.state.follow_up_messages.remove(follow_up_index);
                        self.emit_queue_update();
                    }
                }
            }
        }

        // Emit to extensions first, then notify public listeners.
        self.emit_extension_event(event).await;
        let public_event = match event {
            SessionAgentEvent::AgentEnd { messages } => {
                let will_retry = self.will_retry_after_agent_end(messages);
                let mut map = Map::new();
                map.insert("type".to_string(), json!("agent_end"));
                map.insert(
                    "messages".to_string(),
                    Value::Array(messages.iter().map(|&id| self.state.arena[id].clone()).collect()),
                );
                map.insert("willRetry".to_string(), Value::Bool(will_retry));
                Value::Object(map)
            }
            _ => event.to_value(&self.state.arena),
        };
        self.emit(public_event);

        // Handle session persistence.
        if let SessionAgentEvent::MessageEnd { message } = event {
            let id = *message;
            let role = role_of(&self.state.arena[id]).to_string();
            let mut entry_id: Option<String> = None;
            // Check if this is a custom message from extensions.
            if role == "custom" {
                let message = &self.state.arena[id];
                // Persist as CustomMessageEntry.
                entry_id = Some(self.session_manager.append_custom_message_entry(
                    str_field(message, "customType").unwrap_or_default(),
                    message.get("content").cloned().unwrap_or(Value::Null),
                    message.get("display").and_then(Value::as_bool).unwrap_or(true),
                    message.get("details").filter(|d| !d.is_null()).cloned(),
                ));
            } else if matches!(role.as_str(), "system" | "user" | "assistant" | "toolResult") {
                // Regular LLM message - persist as SessionMessageEntry.
                entry_id = Some(self.session_manager.append_message(self.state.arena[id].clone()));
            }
            if let Some(entry_id) = entry_id {
                self.state.entry_ids_by_message.insert(id, entry_id.clone());
                self.state.entry_message_ids.insert(entry_id, id);
            }
            // Other message types (bashExecution, compactionSummary,
            // branchSummary) are persisted elsewhere.

            if role == "assistant" {
                self.state.last_assistant_message = Some(id);
                let stop_reason = str_field(&self.state.arena[id], "stopReason");
                if stop_reason != Some("error") && stop_reason != Some("length") {
                    self.state.overflow_recovery_attempted = false;
                }

                // Reset retry counter immediately on successful assistant
                // response. This prevents accumulation across multiple LLM
                // calls within a turn.
                if stop_reason != Some("error") && self.state.retry_attempt > 0.0 {
                    let attempt = self.state.retry_attempt;
                    self.emit(super::events::auto_retry_end_event(true, attempt, None));
                    self.state.retry_attempt = 0.0;
                }
            }
        }

        // A turn ends after its assistant message and every tool result has
        // been appended, so this is the first point in the run where a
        // context-only custom message can be inserted without landing
        // between a tool call and its result. Flushing after the extension
        // and listener dispatch above also picks up messages that turn_end
        // handlers queued.
        if let SessionAgentEvent::TurnEnd { tool_results, .. } = event {
            self.state.last_assistant_tool_results = tool_results.clone();
            self.flush_pending_custom_messages();
        }
    }

    /// TS `_willRetryAfterAgentEnd(event)`.
    fn will_retry_after_agent_end(&mut self, messages: &[MsgId]) -> bool {
        if self.state.agent_run_abort_requested.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        let settings = self.settings.retry_settings();
        if !settings.enabled || self.state.retry_attempt >= settings.max_retries {
            return false;
        }
        for &id in messages.iter().rev() {
            if role_of(&self.state.arena[id]) == "assistant" {
                return self.is_retryable_error(&self.state.arena[id]);
            }
        }
        false
    }

    /// TS `_isRetryableError(message)` — context overflow is handled by
    /// compaction, not retry.
    fn is_retryable_error(&self, message: &Value) -> bool {
        let context_window = self
            .model_for_message(message)
            .or_else(|| {
                let model = self.agent.model();
                if model.is_null() { None } else { Some(model) }
            })
            .and_then(|model| model.get("contextWindow").and_then(Value::as_f64));
        if crate::retry_error::is_context_overflow(message, context_window) {
            return false;
        }
        crate::retry_error::is_retryable_assistant_error(message)
    }

    /// TS `_modelForMessage(message)` — the non-virtual path; virtual-model
    /// routing is tranche 14.
    fn model_for_message(&self, message: &Value) -> Option<Value> {
        let model = self.agent.model();
        if model.is_null() {
            return None;
        }
        let provider_matches = model.get("provider").and_then(Value::as_str)
            == str_field(message, "provider");
        let id_matches = model.get("id").and_then(Value::as_str) == str_field(message, "model");
        if provider_matches && id_matches {
            Some(model)
        } else {
            None
        }
    }

    /// TS `_findPersistedMessageEntryId(message)` — WeakMap cache, branch
    /// scan, then projection scan (identity via arena ids; see
    /// [`super::events`] docs).
    pub fn find_persisted_message_entry_id(&mut self, message: MsgId) -> Option<String> {
        if let Some(mapped) = self.state.entry_ids_by_message.get(&message) {
            return Some(mapped.clone());
        }
        for entry in self.session_manager.get_branch(None).iter().rev() {
            if entry_type_of(entry) == "message"
                && self.state.entry_message_ids.get(entry_id_of(entry)) == Some(&message)
            {
                return Some(entry_id_of(entry).to_string());
            }
        }

        let message_index = self.state.state_message_ids.iter().position(|&m| m == message)?;
        let projection = self.session_manager.build_session_projection();
        let mut projected_index = 0usize;
        for entry in &projection.entries {
            for _ in 0..entry.messages.len() {
                if projected_index == message_index {
                    let id = entry_id_of(&entry.source_entry).to_string();
                    self.state.entry_ids_by_message.insert(message, id.clone());
                    return Some(id);
                }
                projected_index += 1;
            }
        }
        None
    }

    /// TS `_flushPendingCustomMessages` — append custom messages queued
    /// while the agent was running, once the current turn's tool results
    /// are in agent state and session history.
    pub(crate) fn flush_pending_custom_messages(&mut self) {
        if self.state.pending_custom_messages.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.state.pending_custom_messages);
        for app_message in pending {
            self.append_custom_message(app_message);
        }
    }

    /// TS `_appendCustomMessage(appMessage)`.
    pub(crate) fn append_custom_message(&mut self, app_message: Value) {
        self.session_manager.append_custom_message_entry(
            str_field(&app_message, "customType").unwrap_or_default(),
            app_message.get("content").cloned().unwrap_or(Value::Null),
            app_message.get("display").and_then(Value::as_bool).unwrap_or(true),
            app_message.get("details").filter(|d| !d.is_null()).cloned(),
        );
        self.refresh_finalized_context();
        let start = agent_event_json("message_start", &[("message", app_message.clone())]);
        self.emit(start);
        let end = agent_event_json("message_end", &[("message", app_message)]);
        self.emit(end);
    }

    /// TS `_refreshFinalizedContext` — refresh the public finalized
    /// transcript from the canonical session projection.
    pub(super) fn refresh_finalized_context(&mut self) {
        let projection = self.session_manager.build_session_projection();
        // Entries targeted by a context edit project fresh message objects
        // (TS); unedited message entries project the stored message object
        // itself, so the arena id carries over.
        let edited: HashSet<String> = self
            .session_manager
            .get_branch(None)
            .iter()
            .filter(|entry| entry_type_of(entry) == "context_edit")
            .filter_map(|entry| entry.get("targetId").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        let mut new_state_ids = Vec::with_capacity(projection.messages.len());
        for entry in &projection.entries {
            let source_id = entry_id_of(&entry.source_entry).to_string();
            let unedited_id = if edited.contains(&source_id) {
                None
            } else {
                self.state.entry_message_ids.get(&source_id).copied()
            };
            for message in &entry.messages {
                let id = match unedited_id {
                    Some(id) => id,
                    None => self.register_message(serde_json::to_value(message).unwrap_or(Value::Null)),
                };
                self.state.entry_ids_by_message.insert(id, source_id.clone());
                new_state_ids.push(id);
            }
        }
        self.agent.set_messages(projection.messages.iter().map(|m| serde_json::to_value(m).unwrap_or(Value::Null)).collect());
        self.state.state_message_ids = new_state_ids;
    }

    /// TS `_replaceMessageInPlace(target, replacement)` — the arena slot
    /// (and agent state, when present) is overwritten in place so state,
    /// later events, listeners, and persistence stay in sync.
    fn replace_message_in_place(&mut self, target: MsgId, replacement: Value) {
        self.sync_message(target, replacement);
    }

    /// TS `_emitAgentSettled` — cache-warmer hook, extension + public
    /// `agent_settled`, deferred settled actions, idle resolution.
    pub async fn emit_agent_settled(&mut self) -> Result<(), String> {
        // TODO(tranche 13): this._cacheWarmer?.onAgentSettled();
        self.state.is_agent_run_active.store(false, std::sync::atomic::Ordering::SeqCst);
        self.state.is_emitting_agent_settled = true;
        // TS try/finally: the flag resets even when a listener throws.
        let aborted = self.state.agent_run_abort_requested.load(std::sync::atomic::Ordering::SeqCst);
        let settled = super::events::agent_settled_event(aborted);
        self.extension_host.emit(settled).await;
        self.emit(super::events::agent_settled_event(aborted));
        self.state.is_emitting_agent_settled = false;

        let deferred = std::mem::take(&mut self.state.deferred_settled_actions);
        if !deferred.is_empty() {
            let mut failure: Option<String> = None;
            for action in deferred {
                match action {
                    DeferredSettledAction::Log(tag) => {
                        self.state.deferred_log.push(tag);
                    }
                    DeferredSettledAction::Prompt { text, options } => {
                        // Boxed: prompt → start_run → emit_agent_settled is
                        // recursive (TS nests the same way at runtime).
                        let result = Box::pin(self.prompt(&text, options)).await;
                        if let Err(error) = result {
                            failure = Some(error);
                            break;
                        }
                    }
                    DeferredSettledAction::SendCustomMessage {
                        custom_type,
                        content,
                        display,
                        details,
                        trigger_turn,
                        deliver_as,
                    } => {
                        let result = Box::pin(self.send_custom_message(
                            &custom_type,
                            content,
                            display,
                            details,
                            trigger_turn,
                            deliver_as.as_deref(),
                        ))
                        .await;
                        if let Err(error) = result {
                            failure = Some(error);
                            break;
                        }
                    }
                }
            }
            self.resolve_idle_wait_if_idle();
            return match failure {
                Some(error) => Err(error),
                None => Ok(()),
            };
        }
        self.resolve_idle_wait_if_idle();
        Ok(())
    }

    /// TS `_emitExtensionEvent(event)` — map agent events onto extension
    /// events (agent_start resets the turn index; turn_end dispatches the
    /// boundary unless the message was pre-dispatched; message_end may
    /// replace the message in place).
    async fn emit_extension_event(&mut self, event: &SessionAgentEvent) {
        let host = self.extension_host.clone();
        match event {
            SessionAgentEvent::AgentStart => {
                self.state.turn_index = 0.0;
                host.emit(json!({ "type": "agent_start" })).await;
            }
            SessionAgentEvent::AgentEnd { messages } => {
                let messages: Vec<Value> =
                    messages.iter().map(|&id| self.state.arena[id].clone()).collect();
                host.emit(json!({ "type": "agent_end", "messages": messages })).await;
            }
            SessionAgentEvent::TurnStart => {
                let extension_event = json!({
                    "type": "turn_start",
                    "turnIndex": pi_core::ai::js_number(self.state.turn_index),
                    "timestamp": pi_core::ai::js_number(self.clock.date_now_ms()),
                });
                host.emit(extension_event).await;
            }
            SessionAgentEvent::TurnEnd { message, tool_results } => {
                if role_of(&self.state.arena[*message]) == "assistant"
                    && !self.state.boundary_dispatched_messages.remove(message)
                {
                    let _ = self.dispatch_turn_end_boundary(*message, tool_results).await;
                }
                self.state.turn_index += 1.0;
            }
            SessionAgentEvent::MessageStart { message } => {
                let extension_event =
                    agent_event_json("message_start", &[("message", self.state.arena[*message].clone())]);
                host.emit(extension_event).await;
            }
            SessionAgentEvent::MessageUpdate { message, assistant_message_event } => {
                let extension_event = agent_event_json(
                    "message_update",
                    &[
                        ("message", self.state.arena[*message].clone()),
                        ("assistantMessageEvent", assistant_message_event.clone()),
                    ],
                );
                host.emit(extension_event).await;
            }
            SessionAgentEvent::MessageEnd { message } => {
                let extension_event =
                    agent_event_json("message_end", &[("message", self.state.arena[*message].clone())]);
                let replacement = host.emit_message_end(extension_event).await;
                if let Some(replacement) = replacement {
                    // Untyped extension handlers can return messages with
                    // null/missing content; normalize so it never enters
                    // agent state or session history.
                    let role = role_of(&replacement).to_string();
                    let normalized = if matches!(role.as_str(), "user" | "assistant" | "toolResult" | "custom")
                        && replacement.get("content").is_none_or(Value::is_null)
                    {
                        let mut normalized = replacement;
                        normalized
                            .as_object_mut()
                            .expect("replacement message is an object")
                            .insert("content".to_string(), json!([]));
                        normalized
                    } else {
                        replacement
                    };
                    self.replace_message_in_place(*message, normalized);
                }
            }
            SessionAgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
                let extension_event = agent_event_json(
                    "tool_execution_start",
                    &[
                        ("toolCallId", json!(tool_call_id)),
                        ("toolName", json!(tool_name)),
                        ("args", args.clone()),
                    ],
                );
                host.emit(extension_event).await;
            }
            SessionAgentEvent::ToolExecutionUpdate { tool_call_id, tool_name, args, partial_result } => {
                let extension_event = agent_event_json(
                    "tool_execution_update",
                    &[
                        ("toolCallId", json!(tool_call_id)),
                        ("toolName", json!(tool_name)),
                        ("args", args.clone()),
                        ("partialResult", partial_result.clone()),
                    ],
                );
                host.emit(extension_event).await;
            }
            SessionAgentEvent::ToolExecutionEnd { tool_call_id, tool_name, result, is_error, duration_ms } => {
                let mut extension_event = agent_event_json(
                    "tool_execution_end",
                    &[
                        ("toolCallId", json!(tool_call_id)),
                        ("toolName", json!(tool_name)),
                        ("result", result.clone()),
                        ("isError", json!(is_error)),
                    ],
                );
                if let Some(duration_ms) = duration_ms {
                    extension_event
                        .as_object_mut()
                        .expect("extension event is an object")
                        .insert("durationMs".to_string(), pi_core::ai::js_number(*duration_ms));
                }
                host.emit(extension_event).await;
            }
        }
    }

    /// TS `_dispatchTurnEndBoundary(message, toolResults)` — the prologue
    /// the events slice owns (activity outcome + persisted-entry lookup);
    /// the boundary emission and draft commits are tranche 11.
    async fn dispatch_turn_end_boundary(&mut self, message: MsgId, tool_results: &[MsgId]) -> bool {
        self.state.last_activity_outcome = match str_field(&self.state.arena[message], "stopReason") {
            Some("aborted") => "aborted".to_string(),
            Some("error") => "error".to_string(),
            _ => "completed".to_string(),
        };
        let message_entry_id = self.find_persisted_message_entry_id(message);
        if self.extension_host.boundary_handlers("turn_end") == 0 {
            return false;
        }
        let Some(message_entry_id) = message_entry_id else {
            self.extension_host.emit_error(json!({
                "extensionPath": "<boundary>",
                "event": "turn_end",
                "error": "turn_end could not resolve the persisted assistant entry ID",
            }));
            return false;
        };
        let tool_result_entry_ids: Vec<Value> = tool_results
            .iter()
            .filter_map(|&result| self.find_persisted_message_entry_id(result))
            .map(|id| json!(id))
            .collect();
        let mut base = Map::new();
        base.insert("type".to_string(), json!("turn_end"));
        base.insert("turnIndex".to_string(), pi_core::ai::js_number(self.state.turn_index));
        base.insert("message".to_string(), self.state.arena[message].clone());
        base.insert(
            "toolResults".to_string(),
            Value::Array(tool_results.iter().map(|&r| self.state.arena[r].clone()).collect()),
        );
        base.insert("messageEntryId".to_string(), json!(message_entry_id));
        base.insert("toolResultEntryIds".to_string(), Value::Array(tool_result_entry_ids));
        base.insert("outcome".to_string(), json!(self.state.last_activity_outcome.clone()));
        let boundary = self.emit_boundary(Value::Object(base), "turn_end").await;
        // Divergence (module docs): dispatched post-loop, so a turn_end
        // `continue` cannot restart the loop; drafts still commit in order.
        let _ = self.commit_boundary_drafts(&boundary.entries);
        if boundary.continue_
            && !self.build_boundary_context(&[], "turn_end").map(|c| c.can_continue).unwrap_or(false)
        {
            self.report_invalid_boundary_continuation("turn_end");
            return false;
        }
        boundary.continue_
    }
}
