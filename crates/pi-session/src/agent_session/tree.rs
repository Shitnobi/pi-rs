//! TS tree-navigation slice (agent-session.ts ~3994–4215 +
//! compaction/branch-summarization.ts `generateBranchSummary`):
//! `navigateTree` (target resolution, validation, branch switching via the
//! session manager, agent-state rebuild from the new projection, tool
//! restore, branch-summary generation with the `session_before_tree` /
//! `session_tree` extension events), `abortBranchSummary`, and
//! `getUserMessagesForForking`.
//!
//! **Documented divergences:** the summarizer retry callbacks buffer into
//! the shared summarization-event queue the session flushes when the driver
//! call completes (TS emits them synchronously — the recorded order is
//! identical; see the `compaction` module docs). The `_getSummarizationRequestAuth`
//! divergences documented there apply to the branch summary call as well,
//! except that the abort-signal rejection is mirrored: a signal aborted
//! before the auth call rejects `navigateTree` with the abort error (TS:
//! `getAuth` rejects, the helper's catch rethrows).

use std::sync::Arc;

use serde_json::{json, Map, Value};

use pi_agent::host::AbortHandle;
use pi_core::ai::content_text;
use pi_core::compaction::{
    collect_entries_for_branch_summary, prepare_branch_entries, BRANCH_SUMMARY_PREAMBLE,
    BRANCH_SUMMARY_PROMPT,
};
use pi_core::compaction_utils::{
    compute_file_lists, format_file_operations, serialize_conversation, SUMMARIZATION_SYSTEM_PROMPT,
};

use super::compaction::{run_summarizer_raw, SummarizerCtx};
use super::AgentSession;

/// TS `navigateTree` options
/// (`{ summarize?, customInstructions?, replaceInstructions?, label? }`).
#[derive(Default)]
pub struct NavigateTreeOptions {
    pub summarize: Option<bool>,
    pub custom_instructions: Option<String>,
    pub replace_instructions: Option<bool>,
    pub label: Option<String>,
}

/// TS `generateBranchSummary` result (the slice `navigateTree` consumes).
enum BranchSummaryOutcome {
    Aborted,
    Error(String),
    Done {
        summary: String,
        usage: Option<Value>,
        read_files: Vec<String>,
        modified_files: Vec<String>,
    },
}

/// TS `generateBranchSummary` (branch-summarization.ts) — the branch-summary
/// driver over the shared summarizer call.
async fn generate_branch_summary(
    ctx: &SummarizerCtx,
    entries: &[Value],
) -> BranchSummaryOutcome {
    // Token budget = context window minus reserved space for prompt + response.
    let context_window =
        ctx.model.get("contextWindow").and_then(Value::as_f64).unwrap_or(128000.0);
    let token_budget = context_window - ctx.reserve_tokens;

    let preparation = prepare_branch_entries(entries, token_budget);
    if preparation.messages.is_empty() {
        return BranchSummaryOutcome::Done {
            summary: "No content to summarize".to_string(),
            usage: None,
            read_files: Vec::new(),
            modified_files: Vec::new(),
        };
    }

    // Transform to LLM-compatible messages, then serialize to text
    // (serialization prevents the model from continuing the conversation).
    let llm_messages = pi_core::messages::convert_to_llm(&preparation.messages);
    let conversation_text = serialize_conversation(&llm_messages);

    let instructions = match (&ctx.custom_instructions, ctx.replace_instructions) {
        (Some(custom), true) => custom.clone(),
        (Some(custom), false) => {
            format!("{}\n\nAdditional focus: {}", BRANCH_SUMMARY_PROMPT, custom)
        }
        (None, _) => BRANCH_SUMMARY_PROMPT.to_string(),
    };
    let prompt_text =
        format!("<conversation>\n{}\n</conversation>\n\n{}", conversation_text, instructions);

    let context = json!({
        "messages": [
            { "role": "system", "content": SUMMARIZATION_SYSTEM_PROMPT, "timestamp": 0 },
            { "role": "user", "content": [{ "type": "text", "text": prompt_text }], "timestamp": pi_core::ai::js_number(ctx.now_ms as f64) },
        ],
    });
    let response = match run_summarizer_raw(ctx, &context).await {
        Ok(response) => response,
        Err(error) => return BranchSummaryOutcome::Error(error),
    };

    if response.get("stopReason").and_then(Value::as_str) == Some("aborted") {
        return BranchSummaryOutcome::Aborted;
    }
    if let Some(failure) =
        pi_core::compaction::get_summarization_failure(&response, "Branch summarization")
    {
        return BranchSummaryOutcome::Error(failure);
    }
    if response
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks.iter().any(|block| block.get("type").and_then(Value::as_str) == Some("toolCall"))
        })
    {
        return BranchSummaryOutcome::Error(
            "Branch summarization attempted to call a tool".to_string(),
        );
    }

    let mut summary = format!(
        "{}{}",
        BRANCH_SUMMARY_PREAMBLE,
        content_text(response.get("content").unwrap_or(&Value::Null), "")
    );
    let lists = compute_file_lists(&preparation.file_ops);
    summary.push_str(&format_file_operations(&lists.read_files, &lists.modified_files));
    BranchSummaryOutcome::Done {
        summary: if summary.is_empty() { "No summary generated".to_string() } else { summary },
        usage: response.get("usage").filter(|u| !u.is_null()).cloned(),
        read_files: lists.read_files,
        modified_files: lists.modified_files,
    }
}

impl AgentSession {
    /// TS `navigateTree(targetId, options?)` — returns the TS result object
    /// (`{ editorText?, cancelled, aborted?, summaryEntry? }`); `Err` models
    /// a thrown error.
    pub async fn navigate_tree(
        &mut self,
        target_id: &str,
        options: NavigateTreeOptions,
    ) -> Result<Value, String> {
        if self.is_streaming() {
            return Err(
                "Wait for the current response to finish before navigating the session tree."
                    .to_string(),
            );
        }
        if self.is_compacting() {
            return Err(
                "Wait for the current compaction or tree navigation to finish before navigating the session tree."
                    .to_string(),
            );
        }

        let old_leaf_id = self.session_manager.get_leaf_id().map(str::to_string);

        // No-op if already at target.
        if Some(target_id) == old_leaf_id.as_deref() {
            return Ok(json!({ "cancelled": false }));
        }

        // Model required for summarization.
        let model = self.agent.model();
        if options.summarize == Some(true) && model.is_null() {
            return Err("No model available for summarization".to_string());
        }

        if self.session_manager.get_entry(target_id).is_none() {
            return Err(format!("Entry {} not found", target_id));
        }

        // Collect entries to summarize (from old leaf to common ancestor).
        let collected =
            collect_entries_for_branch_summary(&self.session_manager, old_leaf_id.as_deref(), target_id);
        let entries_to_summarize = collected.entries;

        // Prepare event data - mutable so extensions can override.
        let mut custom_instructions = options.custom_instructions.clone();
        let mut replace_instructions = options.replace_instructions;
        let mut label = options.label.clone();

        // TS `TreePreparation` — undefined-valued fields are omitted by JSON
        // serialization.
        let mut preparation = Map::new();
        preparation.insert("targetId".to_string(), json!(target_id));
        preparation.insert("oldLeafId".to_string(), json!(old_leaf_id));
        preparation.insert("commonAncestorId".to_string(), json!(collected.common_ancestor_id));
        preparation.insert("entriesToSummarize".to_string(), json!(entries_to_summarize));
        preparation.insert("userWantsSummary".to_string(), json!(options.summarize.unwrap_or(false)));
        if let Some(instructions) = &custom_instructions {
            preparation.insert("customInstructions".to_string(), json!(instructions));
        }
        if let Some(replace) = replace_instructions {
            preparation.insert("replaceInstructions".to_string(), json!(replace));
        }
        if let Some(label) = &label {
            preparation.insert("label".to_string(), json!(label));
        }
        let preparation = Value::Object(preparation);

        // Set up abort controller for summarization.
        let handle = AbortHandle::new();
        *self.state.branch_summary_abort.lock().expect("branch summary abort") = Some(handle.clone());

        let outcome = self
            .navigate_tree_inner(
                target_id,
                &old_leaf_id,
                &entries_to_summarize,
                &options,
                &model,
                &preparation,
                &mut custom_instructions,
                &mut replace_instructions,
                &mut label,
                &handle,
            )
            .await;
        // TS finally: clear the controller + resolve idle waiters.
        *self.state.branch_summary_abort.lock().expect("branch summary abort") = None;
        self.resolve_idle_wait_if_idle();
        outcome
    }

    /// The `try` body of TS `navigateTree` (after the controller is set).
    #[allow(clippy::too_many_arguments)]
    async fn navigate_tree_inner(
        &mut self,
        target_id: &str,
        old_leaf_id: &Option<String>,
        entries_to_summarize: &[Value],
        options: &NavigateTreeOptions,
        model: &Value,
        preparation: &Value,
        custom_instructions: &mut Option<String>,
        replace_instructions: &mut Option<bool>,
        label: &mut Option<String>,
        handle: &AbortHandle,
    ) -> Result<Value, String> {
        let mut extension_summary: Option<Value> = None;
        let mut from_extension = false;

        // Emit session_before_tree event.
        if self.extension_host.has_handlers("session_before_tree") {
            // TS embeds the raw AbortSignal, which JSON-serializes to `{}`.
            let event = json!({
                "type": "session_before_tree",
                "preparation": preparation,
                "signal": {},
            });
            let host = Arc::clone(&self.extension_host);
            let result = host
                .emit_session_before_tree(event)
                .await
                .unwrap_or(Value::Null);

            if result.get("cancel").and_then(Value::as_bool) == Some(true) {
                return Ok(json!({ "cancelled": true }));
            }
            if let Some(summary) = result.get("summary").filter(|s| !s.is_null()) {
                if options.summarize == Some(true) {
                    extension_summary = Some(summary.clone());
                    from_extension = true;
                }
            }
            if let Some(instructions) = result.get("customInstructions") {
                if !instructions.is_null() {
                    *custom_instructions = instructions.as_str().map(str::to_string);
                }
            }
            if let Some(replace) = result.get("replaceInstructions") {
                if !replace.is_null() {
                    *replace_instructions = replace.as_bool();
                }
            }
            if let Some(result_label) = result.get("label") {
                if !result_label.is_null() {
                    *label = result_label.as_str().map(str::to_string);
                }
            }
        }

        // Run default summarizer if needed.
        let mut summary_text: Option<String> = None;
        let mut summary_details: Option<Value> = None;
        let mut summary_usage: Option<Value> = None;
        if options.summarize == Some(true) && !entries_to_summarize.is_empty() && extension_summary.is_none()
        {
            // TS awaits `_getSummarizationRequestAuth(model, signal)` here:
            // the real modelRuntime's `getAuth` races credential resolution
            // against the signal and rejects on an aborted one, and the
            // helper's catch rethrows when the signal is aborted — so an
            // aborted branch summary rejects `navigateTree` before the
            // summarizer call.
            if handle.is_aborted() {
                return Err("This operation was aborted".to_string());
            }
            let request = self.get_summarization_request_auth(model);
            let branch_settings = self.settings.branch_summary_settings();
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
                attempt_start_source: json!({ "source": "branchSummary" }),
                signal: handle.clone(),
                custom_instructions: custom_instructions.clone(),
                replace_instructions: replace_instructions.unwrap_or(false),
                reserve_tokens: branch_settings.reserve_tokens,
                include_reasoning: false,
            };
            let result = generate_branch_summary(&ctx, entries_to_summarize).await;
            self.flush_summarization_events();
            match result {
                BranchSummaryOutcome::Aborted => {
                    return Ok(json!({ "cancelled": true, "aborted": true }));
                }
                BranchSummaryOutcome::Error(error) => return Err(error),
                BranchSummaryOutcome::Done { summary, usage, read_files, modified_files } => {
                    summary_text = Some(summary);
                    summary_usage = usage;
                    summary_details = Some(json!({
                        "readFiles": read_files,
                        "modifiedFiles": modified_files,
                    }));
                }
            }
        } else if let Some(extension) = extension_summary {
            summary_text = extension.get("summary").and_then(Value::as_str).map(str::to_string);
            summary_details = extension.get("details").filter(|d| !d.is_null()).cloned();
            summary_usage = extension.get("usage").filter(|u| !u.is_null()).cloned();
        }

        // Determine the new leaf position based on target type.
        let target_entry = self
            .session_manager
            .get_entry(target_id)
            .cloned()
            .expect("target entry checked");
        let (new_leaf_id, editor_text): (Option<String>, Option<String>) =
            match target_entry.get("type").and_then(Value::as_str) {
                Some("message")
                    if target_entry["message"]["role"].as_str() == Some("user") =>
                {
                    // User message: leaf = parent (null if root), text goes to editor.
                    (
                        target_entry.get("parentId").and_then(Value::as_str).map(str::to_string),
                        Some(content_text(&target_entry["message"]["content"], "")),
                    )
                }
                Some("custom_message") => (
                    target_entry.get("parentId").and_then(Value::as_str).map(str::to_string),
                    Some(content_text(&target_entry["content"], "")),
                ),
                _ => (Some(target_id.to_string()), None),
            };

        // Switch leaf (with or without summary). The summary is attached at
        // the navigation target position (newLeafId), not the old branch.
        let mut summary_entry: Option<Value> = None;
        if let Some(summary) = &summary_text {
            let summary_id = self.session_manager.branch_with_summary(
                new_leaf_id.as_deref(),
                summary,
                summary_details.clone(),
                if from_extension { Some(true) } else { Some(false) },
                summary_usage.clone(),
            )?;
            summary_entry = self.session_manager.get_entry(&summary_id).cloned();

            // Attach label to the summary entry.
            if let Some(label) = &label {
                self.session_manager.append_label_change(&summary_id, Some(label))?;
            }
        } else if new_leaf_id.is_none() {
            // No summary, navigating to root - reset leaf.
            self.session_manager.reset_leaf();
        } else {
            // No summary, navigating to non-root.
            self.session_manager.branch(new_leaf_id.as_deref().expect("checked non-root"))?;
        }

        // Attach label to target entry when not summarizing.
        if label.is_some() && summary_text.is_none() {
            self.session_manager.append_label_change(target_id, label.as_deref())?;
        }

        // Update finalized context from the canonical session projection.
        self.refresh_finalized_context();
        self.restore_tools_from_transcript(&[]);

        // Emit session_tree event (undefined-valued fields omitted).
        let mut event = Map::new();
        event.insert("type".to_string(), json!("session_tree"));
        event.insert("newLeafId".to_string(), json!(self.session_manager.get_leaf_id()));
        event.insert("oldLeafId".to_string(), json!(old_leaf_id));
        if let Some(summary_entry) = &summary_entry {
            event.insert("summaryEntry".to_string(), summary_entry.clone());
        }
        if summary_text.is_some() {
            event.insert("fromExtension".to_string(), json!(from_extension));
        }
        let host = Arc::clone(&self.extension_host);
        host.emit(Value::Object(event)).await;

        let mut result = Map::new();
        if let Some(editor_text) = editor_text {
            result.insert("editorText".to_string(), json!(editor_text));
        }
        result.insert("cancelled".to_string(), json!(false));
        if let Some(summary_entry) = summary_entry {
            result.insert("summaryEntry".to_string(), summary_entry);
        }
        Ok(Value::Object(result))
    }

    /// TS `abortBranchSummary()` — cancel in-progress branch summarization.
    pub fn abort_branch_summary(&self) {
        if let Some(handle) =
            self.state.branch_summary_abort.lock().expect("branch summary abort").as_ref()
        {
            handle.abort();
        }
    }

    /// Harness seam: the shared abort tap for extension fakes that call
    /// `session.abortBranchSummary()` mid-hook.
    pub fn branch_summary_abort_tap(&self) -> Arc<dyn Fn() + Send + Sync> {
        let slot = Arc::clone(&self.state.branch_summary_abort);
        Arc::new(move || {
            if let Some(handle) = slot.lock().expect("branch summary abort").as_ref() {
                handle.abort();
            }
        })
    }

    /// TS `getUserMessagesForForking()` — all user messages with text, for
    /// the fork selector.
    pub fn get_user_messages_for_forking(&self) -> Vec<Value> {
        let mut result = Vec::new();
        for entry in self.session_manager.get_entries() {
            if entry.get("type").and_then(Value::as_str) != Some("message") {
                continue;
            }
            if entry["message"]["role"].as_str() != Some("user") {
                continue;
            }
            let text = content_text(&entry["message"]["content"], "");
            if !text.is_empty() {
                result.push(json!({ "entryId": entry["id"], "text": text }));
            }
        }
        result
    }
}
