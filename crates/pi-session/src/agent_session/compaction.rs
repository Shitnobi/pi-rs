//! TS compaction slice (agent-session.ts ~2747–3274 + compaction.ts
//! `compact()`): the async summarization driver (`compact`,
//! `generateSummaryWithUsage`, `generateTurnPrefixSummary`,
//! `completeSummarization` + `retryAssistantCall`), `_checkCompaction`,
//! `_runAutoCompaction`, manual `compact()`, `_runDefaultCompaction`,
//! `_getSummarizationRequestAuth`, `_summarizationRetryCallbacks`,
//! `_exceedsCompactionThreshold`, `abortCompaction`, and
//! `setAutoCompactionEnabled`/`autoCompactionEnabled`.
//!
//! **Documented divergences:** (1) `_getSummarizationRequestAuth` skips the
//! virtual-model `resolveModel` route (tranche 14) and the
//! `streamFunction === streamSimple` branch (the Rust agent's stream seam is
//! always injected; harness fakes provide no auth, like the TS fakes whose
//! `getAuth` is missing/undefined — the try/catch returns the bare model).
//! (2) `_compactBeforeNextAssistantResponse` (the request-time threshold
//! compaction inside the agent context transform) is not wired; the pre-prompt
//! (`_prepareRun`) and post-run (`_handlePostAgentRun`) checks are. (3) The
//! summarization retry callbacks buffer into a shared queue the session
//! flushes when the driver call completes — TS emits them synchronously, but
//! no other session event interleaves inside the driver, so the recorded
//! order is identical. (4) `retryAssistantCall`'s sleep uses the injected
//! [`Sleeper`] seam (TS `setTimeout`); aborts normalize to a `stopReason
//! "aborted"` result the same way.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use pi_agent::host::{AbortHandle, Sleeper};
use pi_agent::LoopStreamFn;
use pi_core::ai::{content_text, js_number};
use pi_core::compaction::{
    calculate_context_tokens, estimate_context_tokens, estimate_projected_context_tokens,
    prepare_compaction, should_compact, SUMMARIZATION_PROMPT,
    TURN_PREFIX_SUMMARIZATION_PROMPT,
};
use pi_core::compaction_utils::{
    compute_file_lists, format_file_operations, serialize_conversation, FileOperations,
    SUMMARIZATION_SYSTEM_PROMPT,
};
use pi_core::messages::parse_date_millis;
use pi_core::session_manager::get_latest_compaction_entry;
use pi_core::usage_totals::combine_usage_values;

use super::events::MsgId;
use super::retry::retry_delay_ms;
use super::AgentSession;
use crate::retry_error::{is_context_overflow, is_recoverable_length, is_retryable_assistant_error};

fn field_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn role_of(message: &Value) -> &str {
    field_str(message, "role").unwrap_or_default()
}

/// TS `withoutDeletedHeaders` — drop header entries whose value is null.
fn without_deleted_headers(headers: Option<Value>) -> Option<Value> {
    let headers = headers?;
    if !headers.is_object() {
        return Some(headers);
    }
    let object = headers.as_object().expect("headers object");
    let filtered: Map<String, Value> = object
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Some(Value::Object(filtered))
}

/// The summarizer request auth (`_getSummarizationRequestAuth` result).
pub struct SummarizerAuth {
    pub model: Value,
    pub api_key: Option<String>,
    pub headers: Option<Value>,
    pub env: Option<Value>,
    pub thinking_level: String,
}

/// Everything the summarization driver needs; cloned out of the session so
/// no `&mut self` borrow is held across the model call.
pub(crate) struct SummarizerCtx {
    pub model: Value,
    pub api_key: Option<String>,
    pub headers: Option<Value>,
    pub env: Option<Value>,
    pub thinking_level: String,
    pub stream_fn: Arc<LoopStreamFn>,
    pub retry: pi_core::settings_manager::RetrySettings,
    pub events: Arc<std::sync::Mutex<Vec<Value>>>,
    pub uuid_v7: pi_core::session_manager::UuidGen,
    pub now_ms: i64,
    pub sleeper: Arc<dyn Sleeper>,
    /// The `summarization_retry_attempt_start` source fields
    /// (`{source: "compaction", reason}` or `{source: "branchSummary"}`).
    pub attempt_start_source: Value,
    pub signal: AbortHandle,
    /// TS `customInstructions` — the manual-compaction focus text (also the
    /// branch-summary focus text on the tree slice).
    pub custom_instructions: Option<String>,
    /// Branch-summary-only: `replaceInstructions` (prompt build) and the
    /// `reserveTokens` budget. Compaction passes `false` / `0`.
    pub replace_instructions: bool,
    pub reserve_tokens: f64,
    /// Whether the request options include the `reasoning` thinking level
    /// (compaction does; branch summaries pass no thinking level).
    pub include_reasoning: bool,
}

/// `convertToLlm(currentMessages)` — parse the JSON messages back into the
/// typed `AgentMessage` enum the shared converter consumes.
fn to_agent_messages(messages: &[Value]) -> Vec<pi_core::messages::AgentMessage> {
    messages
        .iter()
        .map(|message| {
            serde_json::from_value(message.clone()).expect("session message deserializes")
        })
        .collect()
}

/// `completeSummarization` — the shared retry-wrapped LLM call, returning
/// the raw response message (post-failure checks are the callers').
pub(super) async fn run_summarizer_raw(
    ctx: &SummarizerCtx,
    context: &Value,
) -> Result<Value, String> {
    // `createSummarizationOptions` + the `cacheRetention: "none"` /
    // `sessionId: options.sessionId ?? uuidv7()` wrapper.
    let mut options = Map::new();
    if let Some(api_key) = &ctx.api_key {
        options.insert("apiKey".to_string(), json!(api_key));
    }
    if let Some(headers) = &ctx.headers {
        options.insert("headers".to_string(), headers.clone());
    }
    if let Some(env) = &ctx.env {
        options.insert("env".to_string(), env.clone());
    }
    if ctx.include_reasoning
        && ctx.model.get("reasoning").and_then(Value::as_bool).unwrap_or(false)
        && !ctx.thinking_level.is_empty()
        && ctx.thinking_level != "off"
    {
        options.insert("reasoning".to_string(), json!(ctx.thinking_level));
    }
    options.insert("cacheRetention".to_string(), json!("none"));
    options.insert("sessionId".to_string(), json!((ctx.uuid_v7)()));
    let options = Value::Object(options);

    // `retryAssistantCall(produce, retry, signal, callbacks)`.
    let max_attempts = if ctx.retry.enabled { ctx.retry.max_retries } else { 0.0 };
    let mut attempt = 0.0f64;
    let mut last_retry: Option<(f64, String)> = None;
    let response = loop {
        // `produce()`: `streamFn(model, context, requestOptions).result()`.
        let output = (ctx.stream_fn)(&ctx.model, &context, &options).await?;
        let response = output.result;
        if response.is_null() {
            return Err("Cannot read properties of null (reading 'stopReason')".to_string());
        }
        let stop_reason = field_str(&response, "stopReason");
        if stop_reason == Some("aborted") {
            if let Some((attempt, _)) = &last_retry {
                let attempt = *attempt;
                push_retry_finished(ctx);
                let _ = attempt;
            }
            break response;
        }
        if stop_reason != Some("error") {
            if last_retry.is_some() {
                push_retry_finished(ctx);
            }
            break response;
        }
        if attempt >= max_attempts || !is_retryable_assistant_error(&response) {
            if last_retry.is_some() {
                push_retry_finished(ctx);
            }
            break response;
        }
        attempt += 1.0;
        let error_message = field_str(&response, "errorMessage").unwrap_or_default();
        let error_message =
            if error_message.is_empty() { "Unknown error" } else { error_message }.to_string();
        let delay_ms =
            retry_delay_ms(ctx.retry.base_delay_ms, Some(ctx.retry.max_agent_delay_ms), attempt);
        ctx.events.lock().expect("summarization events").push(json!({
            "type": "summarization_retry_scheduled",
            "attempt": js_number(attempt),
            "maxAttempts": js_number(max_attempts),
            "delayMs": js_number(delay_ms),
            "errorMessage": error_message,
        }));
        // Sleep; aborts normalize to an aborted response.
        if ctx.sleeper.sleep(delay_ms as u64, &ctx.signal).await.is_err() {
            push_retry_finished(ctx);
            let mut aborted = response.as_object().cloned().unwrap_or_default();
            aborted.remove("errorMessage");
            aborted.insert("stopReason".to_string(), json!("aborted"));
            break Value::Object(aborted);
        }
        let mut event = Map::new();
        event.insert("type".to_string(), json!("summarization_retry_attempt_start"));
        if let Some(source) = ctx.attempt_start_source.as_object() {
            for (key, value) in source {
                event.insert(key.clone(), value.clone());
            }
        }
        ctx.events.lock().expect("summarization events").push(Value::Object(event));
        last_retry = Some((attempt, error_message));
    };
    Ok(response)
}

/// `completeSummarization` + the compaction-side failure checks — the
/// retry-wrapped LLM call returning `(text, usage)`.
async fn run_summarizer(
    ctx: &SummarizerCtx,
    prompt_text: &str,
    label: &str,
    tool_call_error: &str,
) -> Result<(String, Value), String> {
    // `buildSummarizationContext` — `normalizeContext({ systemPrompt, messages })`.
    let context = json!({
        "messages": [
            { "role": "system", "content": SUMMARIZATION_SYSTEM_PROMPT, "timestamp": 0 },
            { "role": "user", "content": [{ "type": "text", "text": prompt_text }], "timestamp": js_number(ctx.now_ms as f64) },
        ],
    });
    let response = run_summarizer_raw(ctx, &context).await?;
    if let Some(failure) = pi_core::compaction::get_summarization_failure(&response, label) {
        return Err(failure);
    }
    if response
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|blocks| blocks.iter().any(|block| field_str(block, "type") == Some("toolCall")))
    {
        return Err(tool_call_error.to_string());
    }
    let text = content_text(response.get("content").unwrap_or(&Value::Null), "");
    let usage = response.get("usage").cloned().unwrap_or(Value::Null);
    Ok((text, usage))
}

pub(super) fn push_retry_finished(ctx: &SummarizerCtx) {
    ctx.events.lock().expect("summarization events").push(json!({
        "type": "summarization_retry_finished",
    }));
}

/// `generateSummaryWithUsage` (history summary; update prompt when a
/// previous summary exists).
async fn generate_summary_with_usage(
    ctx: &SummarizerCtx,
    messages: &[Value],
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
) -> Result<(String, Value), String> {
    let mut base_prompt = match previous_summary {
        Some(_) => pi_core::compaction::update_summarization_prompt(),
        None => SUMMARIZATION_PROMPT.to_string(),
    };
    if let Some(custom) = custom_instructions {
        base_prompt = format!("{}\n\nAdditional focus: {}", base_prompt, custom);
    }
    let llm_messages = pi_core::messages::convert_to_llm(&to_agent_messages(messages));
    let conversation_text = serialize_conversation(&llm_messages);
    let mut prompt_text = format!("<conversation>\n{}\n</conversation>\n\n", conversation_text);
    if let Some(previous) = previous_summary {
        prompt_text.push_str(&format!("<previous-summary>\n{}\n</previous-summary>\n\n", previous));
    }
    prompt_text.push_str(&base_prompt);
    run_summarizer(ctx, &prompt_text, "Summarization", "Summarization attempted to call a tool")
        .await
}

/// `generateTurnPrefixSummary` (split-turn prefix checkpoint).
async fn generate_turn_prefix_summary(
    ctx: &SummarizerCtx,
    messages: &[Value],
) -> Result<(String, Value), String> {
    let llm_messages = pi_core::messages::convert_to_llm(&to_agent_messages(messages));
    let conversation_text = serialize_conversation(&llm_messages);
    let prompt_text = format!(
        "# Conversation\n{}\n\n# Instructions\n{}",
        conversation_text, TURN_PREFIX_SUMMARIZATION_PROMPT
    );
    run_summarizer(
        ctx,
        &prompt_text,
        "Turn prefix summarization",
        "Turn prefix summarization attempted to call a tool",
    )
    .await
}

/// TS `compact(preparation, ...)` (compaction.ts) — the driver. Returns the
/// `CompactionResult` JSON (`{summary, firstKeptEntryId, tokensBefore,
/// usage?, details?}`).
async fn compact_driver(ctx: &SummarizerCtx, preparation: &Value) -> Result<Value, String> {
    let first_kept_entry_id = field_str(preparation, "firstKeptEntryId").unwrap_or_default();
    let messages_to_summarize = preparation
        .get("messagesToSummarize")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let turn_prefix_messages = preparation
        .get("turnPrefixMessages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let is_split_turn = preparation.get("isSplitTurn").and_then(Value::as_bool).unwrap_or(false);
    let tokens_before = preparation.get("tokensBefore").and_then(Value::as_f64).unwrap_or(0.0);
    let previous_summary = preparation.get("previousSummary").and_then(Value::as_str);
    let custom_instructions = ctx.custom_instructions.as_deref();
    let file_ops = FileOperations::from_value(preparation.get("fileOps").unwrap_or(&Value::Null));

    let (summary, usage) = if is_split_turn && !turn_prefix_messages.is_empty() {
        let mut history_text = previous_summary.unwrap_or("No prior history.").to_string();
        let mut history_usage: Option<Value> = None;
        if !messages_to_summarize.is_empty() {
            let (text, usage) = generate_summary_with_usage(
                ctx,
                &messages_to_summarize,
                previous_summary,
                custom_instructions,
            )
            .await?;
            history_text = text;
            history_usage = Some(usage);
        }
        let (turn_text, turn_usage) = generate_turn_prefix_summary(ctx, &turn_prefix_messages).await?;
        let summary =
            format!("{}\n\n---\n\n**Turn Context (split turn):**\n\n{}", history_text, turn_text);
        let usage = match history_usage {
            Some(history) if !history.is_null() => combine_usage_values(&history, &turn_usage),
            _ => turn_usage,
        };
        (summary, usage)
    } else {
        generate_summary_with_usage(ctx, &messages_to_summarize, previous_summary, custom_instructions)
            .await?
    };

    let lists = compute_file_lists(&file_ops);
    let summary = format!(
        "{}{}",
        summary,
        format_file_operations(&lists.read_files, &lists.modified_files)
    );

    if first_kept_entry_id.is_empty() {
        return Err("First kept entry has no UUID - session may need migration".to_string());
    }

    let mut result = Map::new();
    result.insert("summary".to_string(), json!(summary));
    result.insert("firstKeptEntryId".to_string(), json!(first_kept_entry_id));
    result.insert("tokensBefore".to_string(), js_number(tokens_before));
    if !usage.is_null() {
        result.insert("usage".to_string(), usage);
    }
    result.insert(
        "details".to_string(),
        pi_core::compaction_utils::file_lists_to_value(&lists),
    );
    Ok(Value::Object(result))
}

impl AgentSession {
    /// TS `_getSummarizationRequestAuth` (see module docs for the
    /// divergences: no virtual-model routing, no streamSimple branch).
    pub(super) fn get_summarization_request_auth(&self, model: &Value) -> SummarizerAuth {
        let thinking_level = self.agent.thinking_level();
        match self.model_auth.as_ref().and_then(|auth| auth.get_auth(model)) {
            None => SummarizerAuth {
                model: model.clone(),
                api_key: None,
                headers: None,
                env: None,
                thinking_level,
            },
            Some(auth) => {
                let request_model = match &auth.base_url {
                    Some(base_url) => {
                        let mut model = model.clone();
                        model
                            .as_object_mut()
                            .map(|object| object.insert("baseUrl".to_string(), json!(base_url)));
                        model
                    }
                    None => model.clone(),
                };
                SummarizerAuth {
                    model: request_model,
                    api_key: auth.api_key.clone(),
                    headers: without_deleted_headers(auth.headers.clone()),
                    env: auth.env.clone(),
                    thinking_level,
                }
            }
        }
    }

    /// Drain the buffered summarization-retry events (see module docs).
    pub(super) fn flush_summarization_events(&mut self) {
        let events: Vec<Value> =
            std::mem::take(self.state.summarization_events.lock().expect("summarization events").as_mut());
        for event in events {
            self.emit(event);
        }
    }

    /// TS `_runDefaultCompaction`.
    async fn run_default_compaction(
        &mut self,
        preparation: &Value,
        model: &Value,
        custom_instructions: Option<&str>,
        signal: &AbortHandle,
        reason: &str,
    ) -> Result<Value, String> {
        let request = self.get_summarization_request_auth(model);
        let ctx = SummarizerCtx {
            model: request.model,
            api_key: request.api_key,
            headers: request.headers,
            env: request.env,
            thinking_level: request.thinking_level,
            stream_fn: self.agent.stream_fn(),
            retry: self.settings.retry_settings(),
            events: Arc::clone(&self.state.summarization_events),
            uuid_v7: self.session_manager.generators().uuid_v7_gen(),
            now_ms: self.clock.date_now_ms() as i64,
            sleeper: Arc::clone(&self.sleeper),
            attempt_start_source: json!({ "source": "compaction", "reason": reason }),
            signal: signal.clone(),
            custom_instructions: custom_instructions.map(str::to_string),
            replace_instructions: false,
            reserve_tokens: 0.0,
            include_reasoning: true,
        };
        let result = compact_driver(&ctx, preparation).await;
        self.flush_summarization_events();
        result
    }

    /// TS `_clearManualCompactionState`.
    fn clear_manual_compaction_state(&mut self) {
        *self.state.compaction_abort.lock().expect("compaction abort") = None;
        self.resolve_idle_wait_if_idle();
    }

    /// TS `_emitSessionCompactFailed` — extension-only emission.
    async fn emit_session_compact_failed(
        &mut self,
        reason: &str,
        error_message: Option<&str>,
        aborted: bool,
        will_retry: bool,
        from_extension: bool,
    ) {
        if !self.extension_host.has_handlers("session_compact_failed") {
            return;
        }
        let mut event = Map::new();
        event.insert("type".to_string(), json!("session_compact_failed"));
        event.insert("reason".to_string(), json!(reason));
        if let Some(message) = error_message {
            event.insert("errorMessage".to_string(), json!(message));
        }
        event.insert("aborted".to_string(), json!(aborted));
        event.insert("willRetry".to_string(), json!(will_retry));
        event.insert("fromExtension".to_string(), json!(from_extension));
        let host = Arc::clone(&self.extension_host);
        host.emit(Value::Object(event)).await;
    }

    /// TS manual `compact(customInstructions?)`.
    pub async fn compact(&mut self, custom_instructions: Option<&str>) -> Result<Value, String> {
        self.abort().await;
        let handle = AbortHandle::new();
        *self.state.compaction_abort.lock().expect("compaction abort") = Some(handle.clone());
        self.emit(json!({ "type": "compaction_start", "reason": "manual" }));
        let mut from_extension = false;
        let mut cancelled_by_extension = false;
        let model = self.agent.model();
        let outcome: Result<Value, String> = async {
            if model.is_null() {
                return Err(self.no_model_selected_message());
            }
            let settings = self.compaction_settings_for(&model);
            let branch = self.session_manager.get_branch(None);
            let Some(preparation) = prepare_compaction(&branch, &settings) else {
                let last_type = branch.last().and_then(|entry| field_str(entry, "type"));
                return Err(if last_type == Some("compaction") {
                    "Already compacted".to_string()
                } else {
                    "Nothing to compact (session too small)".to_string()
                });
            };

            let extension_compaction = self
                .emit_session_before_compact(
                    &preparation,
                    &branch,
                    custom_instructions,
                    "manual",
                    false,
                    &handle,
                    &mut from_extension,
                    &mut cancelled_by_extension,
                )
                .await?;
            let (result, from_ext) = match extension_compaction {
                Some(result) => (result, true),
                None => {
                    let result =
                        self.run_default_compaction(&preparation, &model, custom_instructions, &handle, "manual").await?;
                    (result, false)
                }
            };
            from_extension = from_ext;
            if handle.is_aborted() {
                return Err("Compaction cancelled".to_string());
            }
            Ok(self
                .finish_compaction(&result, &handle, "manual", false, from_extension)
                .await?
                .0)
        }
        .await;
        match outcome {
            Ok(result) => {
                self.clear_manual_compaction_state();
                self.emit(json!({
                    "type": "compaction_end",
                    "reason": "manual",
                    "result": result,
                    "aborted": false,
                    "willRetry": false,
                }));
                Ok(result)
            }
            Err(error) => {
                let aborted = handle.is_aborted() || cancelled_by_extension;
                let error_message =
                    if aborted { None } else { Some(format!("Compaction failed: {}", error)) };
                self.clear_manual_compaction_state();
                let mut event = Map::new();
                event.insert("type".to_string(), json!("compaction_end"));
                event.insert("reason".to_string(), json!("manual"));
                event.insert("aborted".to_string(), json!(aborted));
                event.insert("willRetry".to_string(), json!(false));
                if let Some(message) = &error_message {
                    event.insert("errorMessage".to_string(), json!(message));
                }
                self.emit(Value::Object(event));
                self.emit_session_compact_failed(
                    "manual",
                    error_message.as_deref(),
                    aborted,
                    false,
                    from_extension,
                )
                .await;
                Err(error)
            }
        }
    }

    /// The shared `session_before_compact` extension hook. `Err` models a
    /// throw (`"Compaction cancelled"` for cancels).
    #[allow(clippy::too_many_arguments)]
    async fn emit_session_before_compact(
        &mut self,
        preparation: &Value,
        branch: &[Value],
        custom_instructions: Option<&str>,
        reason: &str,
        will_retry: bool,
        signal: &AbortHandle,
        from_extension: &mut bool,
        cancelled_by_extension: &mut bool,
    ) -> Result<Option<Value>, String> {
        if !self.extension_host.has_handlers("session_before_compact") {
            return Ok(None);
        }
        // TS embeds the raw preparation object into the event; its `fileOps`
        // holds JS `Set`s, which JSON-serialize to `{}` regardless of contents.
        // Mirror that wire shape for the embedded copy.
        let mut preparation_json = preparation.clone();
        if let Some(obj) = preparation_json.as_object_mut() {
            if obj.get("fileOps").is_some_and(|f| f.get("read").is_some()) {
                obj.insert(
                    "fileOps".to_string(),
                    json!({ "read": {}, "written": {}, "edited": {} }),
                );
            }
        }
        let mut event = Map::new();
        event.insert("type".to_string(), json!("session_before_compact"));
        event.insert("preparation".to_string(), preparation_json);
        event.insert("branchEntries".to_string(), json!(branch));
        if let Some(custom) = custom_instructions {
            event.insert("customInstructions".to_string(), json!(custom));
        }
        event.insert("reason".to_string(), json!(reason));
        event.insert("willRetry".to_string(), json!(will_retry));
        event.insert("signal".to_string(), json!({}));
        let _ = signal;
        let host = Arc::clone(&self.extension_host);
        let result = host.emit_session_before_compact(Value::Object(event)).await;
        if result.as_ref().and_then(|r| r.get("cancel")).and_then(Value::as_bool) == Some(true) {
            *cancelled_by_extension = true;
            return Err("Compaction cancelled".to_string());
        }
        if let Some(compaction) =
            result.as_ref().and_then(|r| r.get("compaction")).filter(|c| !c.is_null())
        {
            *from_extension = true;
            return Ok(Some(compaction.clone()));
        }
        Ok(None)
    }

    /// The shared post-summary flow: `appendCompaction`, finalized-context
    /// refresh, `session_compact` emission, and the `CompactionResult`
    /// (with `estimatedTokensAfter`). Returns the result and the saved
    /// compaction entry (already emitted to extensions when found).
    async fn finish_compaction(
        &mut self,
        compact_result: &Value,
        signal: &AbortHandle,
        reason: &str,
        will_retry: bool,
        from_extension: bool,
    ) -> Result<(Value, Option<Value>), String> {
        if signal.is_aborted() {
            return Err("This operation was aborted".to_string());
        }
        let summary = field_str(compact_result, "summary").unwrap_or_default();
        let first_kept_entry_id = field_str(compact_result, "firstKeptEntryId").unwrap_or_default();
        let tokens_before = compact_result.get("tokensBefore").and_then(Value::as_f64).unwrap_or(0.0);
        let details = compact_result.get("details").filter(|d| !d.is_null()).cloned();
        let usage = compact_result.get("usage").filter(|u| !u.is_null()).cloned();
        self.session_manager.append_compaction(
            summary,
            Some(first_kept_entry_id),
            tokens_before as i64,
            details,
            Some(from_extension),
            usage.clone(),
        );
        let new_entries = self.session_manager.get_entries();
        self.refresh_finalized_context();
        let projection = self.session_manager.build_session_projection();
        let projected_messages: Vec<Value> = projection
            .messages
            .iter()
            .map(|message| serde_json::to_value(message).unwrap_or(Value::Null))
            .collect();
        let estimated_tokens_after =
            pi_agent::conversion::estimate_messages_tokens(&projected_messages);
        let saved_compaction_entry = new_entries
            .iter()
            .find(|entry| {
                field_str(entry, "type") == Some("compaction") && entry.get("summary") == Some(&json!(summary))
            })
            .cloned();
        if let Some(saved) = &saved_compaction_entry {
            let host = Arc::clone(&self.extension_host);
            host.emit(json!({
                "type": "session_compact",
                "compactionEntry": saved,
                "fromExtension": from_extension,
                "reason": reason,
                "willRetry": will_retry,
            }))
            .await;
        }
        let mut result = Map::new();
        result.insert("summary".to_string(), json!(summary));
        result.insert("firstKeptEntryId".to_string(), json!(first_kept_entry_id));
        result.insert("tokensBefore".to_string(), js_number(tokens_before));
        result.insert(
            "estimatedTokensAfter".to_string(),
            js_number(estimated_tokens_after as f64),
        );
        if let Some(usage) = usage {
            result.insert("usage".to_string(), usage);
        }
        if let Some(details) = compact_result.get("details").filter(|d| !d.is_null()) {
            result.insert("details".to_string(), details.clone());
        }
        Ok((Value::Object(result), saved_compaction_entry))
    }

    /// TS `abortCompaction()` — cancel in-progress compaction (manual or
    /// auto; branch summarization is tranche 16).
    pub fn abort_compaction(&self) {
        if let Some(handle) = self.state.compaction_abort.lock().expect("compaction abort").as_ref() {
            handle.abort();
        }
        if let Some(handle) = self.state.auto_compaction_abort.lock().expect("auto compaction abort").as_ref() {
            handle.abort();
        }
    }

    /// TS `get isCompacting` (manual + auto compaction controllers and the
    /// branch-summary controller, tranche 16).
    pub fn is_compacting(&self) -> bool {
        self.state.compaction_abort.lock().expect("compaction abort").is_some()
            || self.state.auto_compaction_abort.lock().expect("auto compaction abort").is_some()
            || self.state.branch_summary_abort.lock().expect("branch summary abort").is_some()
    }

    /// Harness seam: the shared abort tap for extension fakes that call
    /// `session.abortCompaction()` mid-hook.
    pub fn compaction_abort_tap(&self) -> Arc<dyn Fn() + Send + Sync> {
        let manual = Arc::clone(&self.state.compaction_abort);
        let auto = Arc::clone(&self.state.auto_compaction_abort);
        Arc::new(move || {
            if let Some(handle) = manual.lock().expect("compaction abort").as_ref() {
                handle.abort();
            }
            if let Some(handle) = auto.lock().expect("auto compaction abort").as_ref() {
                handle.abort();
            }
        })
    }

    /// TS `setAutoCompactionEnabled(enabled)`.
    pub fn set_auto_compaction_enabled(&mut self, enabled: bool) {
        self.settings.set_compaction_enabled(enabled);
    }

    /// TS `get autoCompactionEnabled`.
    pub fn auto_compaction_enabled(&self) -> bool {
        self.compaction_settings_for(&self.agent.model()).enabled
    }

    /// TS `_exceedsCompactionThreshold(model, projection)` (request-time
    /// threshold check; see module docs for wiring).
    pub fn exceeds_compaction_threshold(
        &self,
        model: &Value,
        projection: &pi_core::session_manager::SessionProjection,
    ) -> bool {
        let context_window = model.get("contextWindow").and_then(Value::as_f64).unwrap_or(0.0);
        if context_window <= 0.0 {
            return false;
        }
        let branch = self.session_manager.get_branch(None);
        let settings = self.compaction_settings_for(&self.agent.model());
        should_compact(
            estimate_projected_context_tokens(projection, &branch).tokens,
            context_window,
            &settings,
        )
    }

    /// TS `_runAutoCompaction(reason, willRetry)`.
    pub(super) async fn run_auto_compaction(&mut self, reason: &str, will_retry: bool) -> bool {
        let model = self.agent.model();
        let settings = self.compaction_settings_for(&model);
        let mut started = false;
        let mut from_extension = false;
        let mut cancelled_by_extension = false;
        let handle = AbortHandle::new();
        let outcome: Result<bool, String> = async {
            if model.is_null() {
                return Ok(false);
            }
            let branch = self.session_manager.get_branch(None);
            let Some(preparation) = prepare_compaction(&branch, &settings) else {
                return Ok(false);
            };
            *self.state.auto_compaction_abort.lock().expect("auto compaction abort") =
                Some(handle.clone());
            started = true;
            self.emit(json!({ "type": "compaction_start", "reason": reason }));
            if handle.is_aborted() {
                return Err("This operation was aborted".to_string());
            }
            let extension_compaction = self
                .emit_session_before_compact(
                    &preparation,
                    &branch,
                    None,
                    reason,
                    will_retry,
                    &handle,
                    &mut from_extension,
                    &mut cancelled_by_extension,
                )
                .await?;
            if handle.is_aborted() {
                return Err("This operation was aborted".to_string());
            }
            let (result, from_ext) = match extension_compaction {
                Some(result) => (result, true),
                None => {
                    let result =
                        self.run_default_compaction(&preparation, &model, None, &handle, reason).await?;
                    (result, false)
                }
            };
            from_extension = from_ext;
            let (result, _) =
                self.finish_compaction(&result, &handle, reason, will_retry, from_extension).await?;
            self.emit(json!({
                "type": "compaction_end",
                "reason": reason,
                "result": result,
                "aborted": false,
                "willRetry": will_retry,
            }));
            if will_retry {
                return Ok(true);
            }
            Ok(self.agent.has_queued_messages())
        }
        .await;
        let value = match outcome {
            Ok(value) => value,
            Err(message) => {
                let aborted = handle.is_aborted() || cancelled_by_extension;
                if started {
                    let error_message = if aborted {
                        None
                    } else if reason == "overflow" {
                        Some(format!("Context overflow recovery failed: {}", message))
                    } else {
                        Some(format!("Auto-compaction failed: {}", message))
                    };
                    let mut event = Map::new();
                    event.insert("type".to_string(), json!("compaction_end"));
                    event.insert("reason".to_string(), json!(reason));
                    event.insert("aborted".to_string(), json!(aborted));
                    event.insert("willRetry".to_string(), json!(false));
                    if let Some(message) = &error_message {
                        event.insert("errorMessage".to_string(), json!(message));
                    }
                    self.emit(Value::Object(event));
                    self.emit_session_compact_failed(
                        reason,
                        error_message.as_deref(),
                        aborted,
                        false,
                        from_extension,
                    )
                    .await;
                }
                false
            }
        };
        // TS `finally`: clear the controller and resolve idle waiters.
        *self.state.auto_compaction_abort.lock().expect("auto compaction abort") = None;
        self.resolve_idle_wait_if_idle();
        value
    }

    /// TS `_checkCompaction(assistantMessage, skipAbortedCheck, toolResults)`.
    /// `Err` models a throw (the `_omitRecoveryAttempt` guard).
    pub(super) async fn check_compaction(
        &mut self,
        message: MsgId,
        skip_aborted_check: bool,
        tool_results: &[MsgId],
    ) -> Result<bool, String> {
        let model = self.agent.model();
        let settings = self.compaction_settings_for(&model);
        if !settings.enabled {
            return Ok(false);
        }
        let assistant_message = self.state.arena[message].clone();
        if skip_aborted_check && field_str(&assistant_message, "stopReason") == Some("aborted") {
            return Ok(false);
        }
        let message_model = self.model_for_message(&assistant_message);
        let same_model = message_model.is_some();
        let limits_model = message_model.clone().or_else(|| {
            if model.is_null() { None } else { Some(model.clone()) }
        });
        let context_window = limits_model
            .as_ref()
            .and_then(|model| model.get("contextWindow"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);

        let branch = self.session_manager.get_branch(None);
        let compaction_entry = get_latest_compaction_entry(&branch).cloned();
        let assistant_timestamp = assistant_message.get("timestamp").and_then(Value::as_f64);
        let assistant_is_from_before_compaction = compaction_entry
            .as_ref()
            .and_then(|entry| field_str(entry, "timestamp"))
            .and_then(parse_date_millis)
            .map(|millis| {
                assistant_timestamp.is_some_and(|timestamp| timestamp <= millis as f64)
            })
            .unwrap_or(false);
        if assistant_is_from_before_compaction {
            return Ok(false);
        }

        // Projection bookkeeping.
        let projection = self.session_manager.build_session_projection();
        let assistant_entry_id = self.find_persisted_message_entry_id(message);
        let assistant_is_projected = assistant_entry_id.is_none()
            || projection.entries.iter().any(|entry| {
                entry.source_entry.get("id").and_then(Value::as_str) == assistant_entry_id.as_deref()
                    && entry
                        .messages
                        .iter()
                        .any(|message| role_of(&serde_json::to_value(message).unwrap_or(Value::Null)) == "assistant")
            });
        let assistant_index = assistant_entry_id
            .as_ref()
            .and_then(|id| branch.iter().position(|entry| entry.get("id").and_then(Value::as_str) == Some(id)));
        let entries_after_assistant: &[Value] =
            assistant_index.map(|index| &branch[index + 1..]).unwrap_or(&[]);
        let has_post_assistant_context_edit =
            entries_after_assistant.iter().any(|entry| field_str(entry, "type") == Some("context_edit"));
        let latest_assistant_edit = entries_after_assistant
            .iter()
            .rev()
            .find(|entry| {
                field_str(entry, "type") == Some("context_edit")
                    && entry.get("targetId").and_then(Value::as_str) == assistant_entry_id.as_deref()
            });
        let assistant_retained_for_explicit_recovery = assistant_entry_id.is_none()
            || (!entries_after_assistant.iter().any(|entry| field_str(entry, "type") == Some("compaction"))
                && latest_assistant_edit
                    .map(|entry| {
                        entry.get("replacement").is_none_or(|replacement| !replacement.is_null())
                    })
                    .unwrap_or(true));
        let assistant_usage_matches_projection = assistant_is_projected && !has_post_assistant_context_edit;
        let explicit_overflow = field_str(&assistant_message, "stopReason") == Some("error")
            && is_context_overflow(&assistant_message, None);
        let context_overflow = same_model
            && ((explicit_overflow && assistant_retained_for_explicit_recovery)
                || (assistant_usage_matches_projection
                    && is_context_overflow(&assistant_message, Some(context_window))));
        let recoverable_length = same_model
            && assistant_is_projected
            && is_recoverable_length(
                &assistant_message,
                message_model
                    .as_ref()
                    .and_then(|model| model.get("maxTokens"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
            );
        if context_overflow || recoverable_length {
            let will_retry = field_str(&assistant_message, "stopReason") != Some("stop");
            if !will_retry {
                return Ok(self.run_auto_compaction("overflow", false).await);
            }
            if self.state.overflow_recovery_attempted {
                let error_message = if context_overflow {
                    "Context overflow recovery failed after one compact-and-retry attempt. Try reducing context or switching to a larger-context model."
                } else {
                    "Truncated response recovery failed after one compact-and-retry attempt."
                };
                self.emit(json!({
                    "type": "compaction_end",
                    "reason": "overflow",
                    "aborted": false,
                    "willRetry": false,
                    "errorMessage": error_message,
                }));
                self.emit_session_compact_failed("overflow", Some(error_message), false, false, false)
                    .await;
                return Ok(false);
            }
            self.state.overflow_recovery_attempted = true;
            self.omit_recovery_attempt(message, tool_results)?;
            let retry = self.run_auto_compaction("overflow", will_retry).await;
            if retry {
                self.state.failed_response = Some(message);
            }
            return Ok(retry);
        }

        // Case 3: threshold compaction without retry.
        let has_context_edits = projection
            .entries
            .iter()
            .any(|entry| field_str(&entry.source_entry, "type") == Some("context_edit"));
        let direct_context_tokens = assistant_message
            .get("usage")
            .filter(|usage| !usage.is_null())
            .map(calculate_context_tokens)
            .unwrap_or(0.0);
        let context_tokens = if has_context_edits {
            estimate_projected_context_tokens(&projection, &branch).tokens
        } else if field_str(&assistant_message, "stopReason") == Some("error")
            || direct_context_tokens == 0.0
        {
            let messages = self.agent.messages();
            let estimate = estimate_context_tokens(&messages);
            if let Some(last_usage_index) = estimate.last_usage_index {
                let usage_message = &messages[last_usage_index];
                let stale = compaction_entry
                    .as_ref()
                    .and_then(|entry| field_str(entry, "timestamp"))
                    .and_then(parse_date_millis)
                    .is_some_and(|millis| {
                        role_of(usage_message) == "assistant"
                            && usage_message
                                .get("timestamp")
                                .and_then(Value::as_f64)
                                .is_some_and(|timestamp| timestamp <= millis as f64)
                    });
                if stale {
                    return Ok(false);
                }
            }
            estimate.tokens
        } else {
            direct_context_tokens
        };
        if should_compact(context_tokens, context_window, &settings) {
            return Ok(self.run_auto_compaction("threshold", false).await);
        }
        Ok(false)
    }
}
