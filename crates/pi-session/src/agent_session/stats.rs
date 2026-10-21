//! TS `AgentSession` stats & exports slice (`agent-session.ts`
//! `getSessionStats` ~4213, `getContextUsage` ~4267, `exportToJsonl`
//! ~4344, `summarizeForBugReport` ~4352; `session-export.ts`
//! `serializeSessionBranch` / `exportSessionToJsonl`; `bug-report.ts`
//! `generateBugReportSummary`). `exportToHtml` is deferred (plan §5).
//!
//! **Documented divergences** (shared with the compaction slice's
//! `_getSummarizationRequestAuth` port): no virtual-model routing and no
//! `streamSimple` branch in the summarization auth — the auth seam returns
//! the bare model when `getAuth` yields nothing.
//!
//! `exportSessionToJsonl` reads `process.cwd()` in TS; the Rust port takes
//! the process cwd as a parameter (harnesses pass the harness cwd,
//! production callers pass `std::env::current_dir()`).

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use pi_core::ai::{content_text, js_number, Usage};
use pi_core::compaction::{calculate_context_tokens, estimate_projected_context_tokens};
use pi_core::session_manager::{
    get_latest_compaction_entry, iso_string, CURRENT_SESSION_VERSION,
};
use pi_core::usage_totals::{add_usage_to_totals, create_usage_totals};

use super::AgentSession;

const BUG_SUMMARY_SYSTEM_PROMPT: &str = "You are helping a user file a bug report about pi, the coding agent they are talking to. You will be shown the conversation transcript. Write a report for the pi developers describing what the user was doing and what went wrong.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the report.";

const BUG_SUMMARY_INSTRUCTIONS: &str = "Write the bug report in Markdown with these sections:

## What the user was doing
One short paragraph.

## What went wrong
Concrete description of the failure: wrong output, errors, hangs, tool failures, unexpected behavior. Quote error messages and tool output verbatim where they exist.

## Steps to reproduce
Numbered list, as specific as the transcript allows.

## Relevant details
Tool calls involved, files touched, model behavior, anything else that helps a developer reproduce or locate the problem.

Do not include file contents, secrets, or credentials from the transcript; refer to files by path only. Keep the report factual and concise.";

/// `selectMessages`: the suffix of `messages` fitting `token_budget`
/// (always keeping the last message).
fn select_messages(messages: &[Value], token_budget: f64) -> Vec<Value> {
    let mut selected: Vec<Value> = Vec::new();
    let mut tokens = 0.0f64;
    for message in messages.iter().rev() {
        let next = pi_core::compaction::estimate_tokens(message);
        if !selected.is_empty() && tokens + next > token_budget {
            break;
        }
        selected.push(message.clone());
        tokens += next;
    }
    selected.reverse();
    selected
}

/// `resolvePath(input, baseDir)` for the POSIX slice the export exercises
/// (absolute or plain relative inputs; lexical `path.resolve`).
fn resolve_path(input: &str, base_dir: &str) -> String {
    let absolute = input.starts_with('/');
    let mut stack: Vec<&str> = Vec::new();
    if !absolute {
        stack.extend(base_dir.split('/').filter(|part| !part.is_empty()));
    }
    for part in input.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            _ => stack.push(part),
        }
    }
    format!("/{}", stack.join("/"))
}

impl AgentSession {
    /// TS `getSessionStats` — aggregates over ALL session entries
    /// (including compacted-away history). `Err` models the TS throw on
    /// malformed entries (missing `usage`/`message` fields).
    pub fn get_session_stats(&self) -> Result<Value, String> {
        let mut user_messages = 0.0f64;
        let mut assistant_messages = 0.0f64;
        let mut tool_results = 0.0f64;
        let mut total_messages = 0.0f64;
        let mut tool_calls = 0.0f64;
        let mut totals = create_usage_totals();

        for entry in self.session_manager.get_entries() {
            let entry_type = entry.get("type").and_then(Value::as_str).unwrap_or_default();
            match entry_type {
                "usage" => {
                    let usage = entry
                        .get("usage")
                        .filter(|usage| !usage.is_null())
                        .ok_or_else(|| "Cannot read properties of undefined (reading 'input')".to_string())?;
                    add_usage_to_totals(&mut totals, &Usage::from_value(usage));
                }
                "branch_summary" | "compaction" => {
                    if let Some(usage) = entry.get("usage").filter(|usage| !usage.is_null()) {
                        add_usage_to_totals(&mut totals, &Usage::from_value(usage));
                    }
                }
                _ => {}
            }
            if entry_type != "message" {
                continue;
            }
            total_messages += 1.0;
            let message = entry
                .get("message")
                .filter(|message| !message.is_null())
                .ok_or_else(|| "Cannot read properties of undefined (reading 'role')".to_string())?;
            let role = message.get("role").and_then(Value::as_str).unwrap_or_default();
            match role {
                "user" => user_messages += 1.0,
                "toolResult" => {
                    tool_results += 1.0;
                    if let Some(usage) = message.get("usage").filter(|usage| !usage.is_null()) {
                        add_usage_to_totals(&mut totals, &Usage::from_value(usage));
                    }
                }
                "assistant" => {
                    assistant_messages += 1.0;
                    if let Some(content) = message.get("content").and_then(Value::as_array) {
                        tool_calls +=
                            content.iter().filter(|block| block.get("type").and_then(Value::as_str) == Some("toolCall")).count() as f64;
                    }
                    // TS `addUsageToTotals(totals, assistantMsg.usage)` —
                    // the usage field is required on assistant messages.
                    let usage = message
                        .get("usage")
                        .filter(|usage| !usage.is_null())
                        .ok_or_else(|| "Cannot read properties of undefined (reading 'input')".to_string())?;
                    add_usage_to_totals(&mut totals, &Usage::from_value(usage));
                }
                _ => {}
            }
        }

        let mut stats = Map::new();
        if let Some(session_file) = self.session_manager.get_session_file() {
            stats.insert("sessionFile".to_string(), json!(session_file));
        }
        stats.insert("sessionId".to_string(), json!(self.session_manager.get_session_id()));
        stats.insert("userMessages".to_string(), js_number(user_messages));
        stats.insert("assistantMessages".to_string(), js_number(assistant_messages));
        stats.insert("toolCalls".to_string(), js_number(tool_calls));
        stats.insert("toolResults".to_string(), js_number(tool_results));
        stats.insert("totalMessages".to_string(), js_number(total_messages));
        stats.insert(
            "tokens".to_string(),
            json!({
                "input": js_number(totals.input),
                "output": js_number(totals.output),
                "cacheRead": js_number(totals.cache_read),
                "cacheWrite": js_number(totals.cache_write),
                "total": js_number(totals.input + totals.output + totals.cache_read + totals.cache_write),
            }),
        );
        stats.insert("cost".to_string(), js_number(totals.cost));
        if let Some(context_usage) = self.get_context_usage()? {
            stats.insert("contextUsage".to_string(), context_usage);
        }
        Ok(Value::Object(stats))
    }

    /// TS `getContextUsage` — `None` models the TS `undefined` return.
    pub fn get_context_usage(&self) -> Result<Option<Value>, String> {
        let Some(model) = self.limits_model() else {
            return Ok(None);
        };

        let context_window = model.get("contextWindow").and_then(Value::as_f64).unwrap_or(0.0);
        if context_window <= 0.0 {
            return Ok(None);
        }

        // After compaction, the last assistant usage reflects the
        // pre-compaction context size; usage is only trusted from an
        // assistant that responded after the latest compaction.
        let projection = self.session_manager.build_session_projection();
        let branch = self.session_manager.get_branch(None);
        let latest_compaction = get_latest_compaction_entry(&branch);

        if let Some(compaction) = latest_compaction {
            let mut projected_assistants: HashSet<String> = HashSet::new();
            for entry in &projection.entries {
                for message in &entry.messages {
                    let message = serde_json::to_value(message).unwrap_or(Value::Null);
                    if message.get("role").and_then(Value::as_str) != Some("assistant") {
                        continue;
                    }
                    let stop_reason = message.get("stopReason").and_then(Value::as_str);
                    if stop_reason == Some("aborted") || stop_reason == Some("error") {
                        continue;
                    }
                    // TS `calculateContextTokens(message.usage)` throws on a
                    // missing usage before the `> 0` check.
                    let usage = match message.get("usage").filter(|usage| !usage.is_null()) {
                        Some(usage) => usage,
                        None => {
                            return Err(format!(
                                "Cannot read properties of {} (reading 'totalTokens')",
                                if message.get("usage").is_some() { "null" } else { "undefined" }
                            ))
                        }
                    };
                    if calculate_context_tokens(usage) > 0.0 {
                        if let Some(id) = entry.source_entry.get("id").and_then(Value::as_str) {
                            projected_assistants.insert(id.to_string());
                        }
                        break;
                    }
                }
            }
            let compaction_index = branch
                .iter()
                .position(|entry| entry.get("id").and_then(Value::as_str) == compaction.get("id").and_then(Value::as_str));
            let has_post_compaction_usage = compaction_index.is_some_and(|index| {
                branch[index + 1..]
                    .iter()
                    .any(|entry| entry.get("id").and_then(Value::as_str).is_some_and(|id| projected_assistants.contains(id)))
            });
            if !has_post_compaction_usage {
                return Ok(Some(json!({
                    "tokens": null,
                    "contextWindow": js_number(context_window),
                    "percent": null,
                })));
            }
        }

        let estimate = estimate_projected_context_tokens(&projection, &branch);
        let percent = (estimate.tokens / context_window) * 100.0;
        Ok(Some(json!({
            "tokens": js_number(estimate.tokens),
            "contextWindow": js_number(context_window),
            "percent": js_number(percent),
        })))
    }

    /// TS `routedModel` getter — under a virtual selection, the physical
    /// model of the latest successful response.
    pub fn routed_model(&self) -> Option<(Value, Option<String>)> {
        let model = self.agent.model();
        if model.is_null() || !pi_providers::virtual_models::is_virtual_model(
            model.get("api").and_then(Value::as_str).unwrap_or_default(),
        ) {
            return None;
        }
        let messages = self.agent.messages();
        let latest = pi_providers::virtual_models::find_latest_response(&messages)?.clone();
        let physical = self.physical_model_lookup.as_ref().and_then(|lookup| {
            lookup(
                latest.get("provider").and_then(Value::as_str).unwrap_or_default(),
                latest.get("model").and_then(Value::as_str).unwrap_or_default(),
            )
        })?;
        let thinking_level = latest
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .map(str::to_string);
        Some((physical, thinking_level))
    }

    /// TS `_limitsModel` — the model whose limits apply to the conversation.
    pub fn limits_model(&self) -> Option<Value> {
        self.routed_model().map(|(model, _)| model).or_else(|| {
            let model = self.agent.model();
            if model.is_null() {
                None
            } else {
                Some(model)
            }
        })
    }

    /// TS `exportToJsonl` — writes the current branch as JSONL. The
    /// process cwd is a parameter (TS reads `process.cwd()`).
    pub fn export_to_jsonl(&self, output_path: Option<&str>, process_cwd: &str) -> String {
        let default_name = format!(
            "session-{}.jsonl",
            iso_string(self.clock.date_now_ms() as i64).replace([':', '.'], "-")
        );
        let file_path = resolve_path(output_path.unwrap_or(&default_name), process_cwd);
        let content = self.serialize_session_branch();
        if let Some(parent) = std::path::Path::new(&file_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&file_path, content);
        file_path
    }

    /// TS `serializeSessionBranch` — the session header followed by the
    /// branch entries with re-chained `parentId`s.
    pub fn serialize_session_branch(&self) -> String {
        let timestamp = iso_string(self.clock.date_now_ms() as i64);
        let mut lines: Vec<String> = Vec::new();
        lines.push(
            json!({
                "type": "session",
                "version": CURRENT_SESSION_VERSION,
                "id": self.session_manager.get_session_id(),
                "timestamp": timestamp,
                "cwd": self.session_manager.get_cwd(),
            })
            .to_string(),
        );
        let mut parent_id: Option<String> = None;
        for entry in self.session_manager.get_branch(None) {
            let mut object = entry.as_object().cloned().unwrap_or_default();
            match &parent_id {
                Some(parent) => {
                    object.insert("parentId".to_string(), json!(parent));
                }
                None => {
                    object.insert("parentId".to_string(), Value::Null);
                }
            }
            parent_id = entry.get("id").and_then(Value::as_str).map(str::to_string);
            lines.push(Value::Object(object).to_string());
        }
        format!("{}\n", lines.join("\n"))
    }

    /// TS `summarizeForBugReport` — ask the session model to describe what
    /// went wrong. `hint` is the optional user hint.
    pub async fn summarize_for_bug_report(
        &self,
        hint: Option<&str>,
        signal: &pi_agent::host::AbortHandle,
    ) -> Result<String, String> {
        let model = self.agent.model();
        if model.is_null() {
            return Err("No model selected".to_string());
        }
        let auth = self.get_summarization_request_auth(&model);
        self.generate_bug_report_summary(&auth, hint, signal).await
    }

    /// TS `generateBugReportSummary` (bug-report.ts) — the seam the plan
    /// defers the rest of bug-report.ts behind; this driver is the full
    /// function (message selection, prompt build, retry-wrapped call,
    /// response checks).
    async fn generate_bug_report_summary(
        &self,
        auth: &super::compaction::SummarizerAuth,
        hint: Option<&str>,
        signal: &pi_agent::host::AbortHandle,
    ) -> Result<String, String> {
        let model = &auth.model;
        let context_window = match model.get("contextWindow").and_then(Value::as_f64) {
            Some(window) if window > 0.0 => window,
            _ => 128_000.0,
        };
        let messages = self.agent.messages();
        let selected = select_messages(&messages, (context_window * 0.6).floor());
        let hint_text = hint.map(str::trim).filter(|text| !text.is_empty());
        let mut parts: Vec<String> = Vec::new();
        if selected.len() < messages.len() {
            parts.push(format!(
                "Note: only the last {} of {} messages are shown.",
                selected.len(),
                messages.len()
            ));
        }
        let llm_selected = super::compaction::to_agent_messages(&selected);
        let conversation = pi_core::compaction_utils::serialize_conversation(
            &pi_core::messages::convert_to_llm(&llm_selected),
        );
        parts.push(format!("<conversation>\n{}\n</conversation>", conversation));
        if let Some(hint) = hint_text {
            parts.push(format!("<user-report>\n{}\n</user-report>", hint));
        }
        parts.push(BUG_SUMMARY_INSTRUCTIONS.to_string());
        let prompt = parts.join("\n\n");

        // `requestOptions` (minus the non-JSON `signal`).
        let max_tokens = match model.get("maxTokens").and_then(Value::as_f64) {
            Some(max) if max > 0.0 => max.min(4096.0),
            _ => 4096.0,
        };
        let mut options = Map::new();
        options.insert("maxTokens".to_string(), js_number(max_tokens));
        if let Some(api_key) = &auth.api_key {
            options.insert("apiKey".to_string(), json!(api_key));
        }
        if let Some(headers) = &auth.headers {
            options.insert("headers".to_string(), headers.clone());
        }
        if let Some(env) = &auth.env {
            options.insert("env".to_string(), env.clone());
        }
        options.insert("sessionId".to_string(), json!(self.session_manager.get_session_id()));
        if model.get("reasoning").and_then(Value::as_bool) == Some(true)
            && !auth.thinking_level.is_empty()
            && auth.thinking_level != "off"
        {
            options.insert("reasoning".to_string(), json!(auth.thinking_level));
        }
        options.insert("cacheRetention".to_string(), json!("none"));

        let context = json!({
            "messages": [
                { "role": "system", "content": BUG_SUMMARY_SYSTEM_PROMPT, "timestamp": 0 },
                { "role": "user", "content": [{ "type": "text", "text": prompt }], "timestamp": js_number(self.clock.date_now_ms()) },
            ],
        });

        // `completeSummarization` without retry callbacks: the
        // retry-wrapped call (`retryAssistantCall`).
        let retry = self.settings.retry_settings();
        let max_attempts = if retry.enabled { retry.max_retries } else { 0.0 };
        let mut attempt = 0.0f64;
        let stream_fn = self.agent.stream_fn();
        let response = loop {
            let output = (stream_fn)(&auth.model, &context, &Value::Object(options.clone())).await?;
            let response = output.result;
            if response.is_null() {
                return Err("Cannot read properties of null (reading 'stopReason')".to_string());
            }
            let stop_reason = response.get("stopReason").and_then(Value::as_str);
            if stop_reason != Some("error") || attempt >= max_attempts || !is_retryable(&response) {
                break response;
            }
            attempt += 1.0;
            let delay_ms = super::retry::retry_delay_ms(
                retry.base_delay_ms,
                Some(super::retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS),
                attempt,
            );
            if self.sleeper.sleep(delay_ms as u64, signal).await.is_err() {
                let mut aborted = response.as_object().cloned().unwrap_or_default();
                aborted.remove("errorMessage");
                aborted.insert("stopReason".to_string(), json!("aborted"));
                break Value::Object(aborted);
            }
        };

        if response.get("stopReason").and_then(Value::as_str) == Some("aborted") {
            return Err("Bug report summary was cancelled".to_string());
        }
        if let Some(failure) =
            pi_core::compaction::get_summarization_failure(&response, "Bug report summary")
        {
            return Err(failure);
        }
        if response
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks.iter().any(|block| block.get("type").and_then(Value::as_str) == Some("toolCall"))
            })
        {
            return Err("Bug report summary attempted to call a tool".to_string());
        }
        let content = response
            .get("content")
            .filter(|content| !content.is_null())
            .ok_or_else(|| "Cannot read properties of undefined (reading 'some')".to_string())?;
        let text = content_text(content, "").trim().to_string();
        if text.is_empty() {
            return Err("Bug report summary was empty".to_string());
        }
        Ok(text)
    }
}

/// `isRetryableAssistantError` for the callback-free retry loop.
fn is_retryable(response: &Value) -> bool {
    crate::retry_error::is_retryable_assistant_error(response)
}
