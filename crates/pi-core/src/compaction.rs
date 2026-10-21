//! Port of the pure logic in `src/core/compaction/compaction.ts`.
//!
//! Token estimation, cut-point detection, and compaction preparation. The
//! LLM-dependent parts (`compact`, `generateSummary*`, `completeSummarization`)
//! are deferred until the agent pipeline exists; `getSummarizationFailure`
//! and `prepareCompaction` are pure and ported here.

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use crate::ai::{get_current_system_message, js_number};
use crate::session_manager::{build_session_projection, js_truthy, session_entry_to_context_messages, LeafId, SessionProjection};

use crate::compaction_utils::{
    create_file_ops, extract_file_ops_from_message, js_str_len, js_stringify, FileOperations,
};

/// Value form of one projected entry: `{sourceEntry, messages}` — the JSON
/// shape the TS `buildSessionProjection` produces.
fn projected_entry_values(projection: &SessionProjection) -> Vec<Value> {
    projection
        .entries
        .iter()
        .map(|e| json!({ "sourceEntry": e.source_entry, "messages": e.messages }))
        .collect()
}

/// Value form of `sessionEntryToContextMessages` (typed [`AgentMessage`]s
/// serialized back to their JSON wire shape).
fn entry_context_messages(entry: &Value) -> Vec<Value> {
    session_entry_to_context_messages(entry)
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect()
}

/// Value form of the projection's model-visible messages.
fn projection_messages_values(projection: &SessionProjection) -> Vec<Value> {
    projection
        .messages
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect()
}

// ============================================================================
// File Operation Tracking
// ============================================================================

/// `extractFileOperations`: extract file operations from messages and the
/// previous compaction entry's details.
fn extract_file_operations(
    messages: &[Value],
    entries: &[Value],
    prev_compaction_index: i64,
) -> FileOperations {
    let mut file_ops = create_file_ops();

    // Collect from previous compaction's details (if pi-generated).
    if prev_compaction_index >= 0 {
        let prev_compaction = &entries[prev_compaction_index as usize];
        if !js_truthy(prev_compaction.get("fromHook")) {
            if let Some(details) = prev_compaction.get("details").filter(|d| js_truthy(Some(d))) {
                if let Some(read_files) = details["readFiles"].as_array() {
                    for f in read_files.iter().filter_map(Value::as_str) {
                        file_ops.read_push(f);
                    }
                }
                if let Some(modified_files) = details["modifiedFiles"].as_array() {
                    for f in modified_files.iter().filter_map(Value::as_str) {
                        file_ops.edited_push(f);
                    }
                }
            }
        }
    }

    // Extract from tool calls in messages.
    for msg in messages {
        extract_file_ops_from_message(msg, &mut file_ops);
    }

    file_ops
}

// ============================================================================
// Message Extraction
// ============================================================================

/// `getMessagesFromProjectedEntryForCompaction`: extract AgentMessages from a
/// projected entry if it produces one. System messages are prompt state, not
/// conversation; the compaction entry carries their replay.
fn get_messages_from_projected_entry_for_compaction(entry: &Value) -> Vec<Value> {
    if entry["sourceEntry"]["type"] == "compaction" {
        return Vec::new();
    }
    let empty: Vec<Value> = Vec::new();
    entry["messages"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|message| message["role"] != "system")
        .cloned()
        .collect()
}

// ============================================================================
// Types
// ============================================================================

/// `CompactionSettings`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: f64,
    pub keep_recent_tokens: f64,
}

impl CompactionSettings {
    pub fn from_value(value: &Value) -> CompactionSettings {
        CompactionSettings {
            enabled: js_truthy(value.get("enabled")),
            reserve_tokens: value["reserveTokens"].as_f64().unwrap_or(0.0),
            keep_recent_tokens: value["keepRecentTokens"].as_f64().unwrap_or(0.0),
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "reserveTokens": js_number(self.reserve_tokens),
            "keepRecentTokens": js_number(self.keep_recent_tokens),
        })
    }
}

/// `DEFAULT_COMPACTION_SETTINGS`.
pub fn default_compaction_settings() -> CompactionSettings {
    CompactionSettings {
        enabled: true,
        reserve_tokens: 16384.0,
        keep_recent_tokens: 20000.0,
    }
}

// ============================================================================
// Token calculation
// ============================================================================

/// `calculateContextTokens`: total context tokens from usage, using the native
/// `totalTokens` when available (JS `||`: zero/missing falls back to the sum
/// of components). Missing components make the sum `NaN`, exactly like
/// `undefined + number` in JS — callers' `> 0` checks then reject the usage.
pub fn calculate_context_tokens(usage: &Value) -> f64 {
    let total = usage["totalTokens"].as_f64();
    match total {
        Some(t) if t != 0.0 && !t.is_nan() => t,
        _ => {
            let field = |name: &str| usage.get(name).and_then(Value::as_f64);
            match (
                field("input"),
                field("output"),
                field("cacheRead"),
                field("cacheWrite"),
            ) {
                (Some(input), Some(output), Some(cache_read), Some(cache_write)) => {
                    input + output + cache_read + cache_write
                }
                _ => f64::NAN,
            }
        }
    }
}

/// `getAssistantUsage`: usage from an assistant message, skipping aborted,
/// error, and all-zero usage messages.
fn get_assistant_usage(msg: &Value) -> Option<Value> {
    if msg["role"] != "assistant" {
        return None;
    }
    let usage = msg.get("usage")?;
    if !js_truthy(Some(usage)) {
        return None;
    }
    let stop_reason = msg["stopReason"].as_str();
    if stop_reason == Some("aborted") || stop_reason == Some("error") {
        return None;
    }
    if calculate_context_tokens(usage) > 0.0 {
        Some(usage.clone())
    } else {
        None
    }
}

/// `getLastAssistantUsage`: last valid assistant usage from session entries.
pub fn get_last_assistant_usage(entries: &[Value]) -> Option<Value> {
    for entry in entries.iter().rev() {
        if entry["type"] == "message" {
            if let Some(usage) = get_assistant_usage(&entry["message"]) {
                return Some(usage);
            }
        }
    }
    None
}

/// `ContextUsageEstimate`.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextUsageEstimate {
    pub tokens: f64,
    pub usage_tokens: f64,
    pub trailing_tokens: f64,
    pub last_usage_index: Option<usize>,
}

impl ContextUsageEstimate {
    pub fn to_value(&self) -> Value {
        json!({
            "tokens": js_number(self.tokens),
            "usageTokens": js_number(self.usage_tokens),
            "trailingTokens": js_number(self.trailing_tokens),
            "lastUsageIndex": match self.last_usage_index {
                Some(i) => json!(i),
                None => Value::Null,
            },
        })
    }
}

/// `estimateContextTokens`: estimate context tokens from messages, using the
/// last assistant usage when available.
pub fn estimate_context_tokens(messages: &[Value]) -> ContextUsageEstimate {
    let usage_info = get_last_assistant_usage_info(messages);

    let Some((usage, index)) = usage_info else {
        let mut estimated = 0.0;
        for message in messages {
            estimated += estimate_tokens(message);
        }
        return ContextUsageEstimate {
            tokens: estimated,
            usage_tokens: 0.0,
            trailing_tokens: estimated,
            last_usage_index: None,
        };
    };

    let usage_tokens = calculate_context_tokens(&usage);
    let mut trailing_tokens = 0.0;
    for message in messages.iter().skip(index + 1) {
        trailing_tokens += estimate_tokens(message);
    }

    ContextUsageEstimate {
        tokens: usage_tokens + trailing_tokens,
        usage_tokens,
        trailing_tokens,
        last_usage_index: Some(index),
    }
}

fn get_last_assistant_usage_info(messages: &[Value]) -> Option<(Value, usize)> {
    for (i, message) in messages.iter().enumerate().rev() {
        if let Some(usage) = get_assistant_usage(message) {
            return Some((usage, i));
        }
    }
    None
}

/// `estimateProjectedContextTokens`: estimate projected context without
/// trusting usage captured before a later edit or compaction.
pub fn estimate_projected_context_tokens(
    projection: &SessionProjection,
    branch_entries: &[Value],
) -> ContextUsageEstimate {
    let projection_messages = projection_messages_values(projection);
    let projected_entries = projected_entry_values(projection);
    let estimate = estimate_context_tokens(&projection_messages);
    if let Some(last_usage_index) = estimate.last_usage_index {
        let mut projected_message_index = 0usize;
        let mut usage_entry_id: Option<&str> = None;
        for entry in &projected_entries {
            let entry_len = entry["messages"].as_array().map_or(0, Vec::len);
            let next_message_index = projected_message_index + entry_len;
            if last_usage_index < next_message_index {
                usage_entry_id = entry["sourceEntry"]["id"].as_str();
                break;
            }
            projected_message_index = next_message_index;
        }

        let usage_entry_index = match usage_entry_id {
            Some(id) => branch_entries
                .iter()
                .position(|entry| entry["id"].as_str() == Some(id))
                .map_or(-1i64, |i| i as i64),
            None => -1,
        };
        let mut latest_invalidating_entry_index: i64 = -1;
        for (i, entry) in branch_entries.iter().enumerate().rev() {
            if entry["type"] == "context_edit" || entry["type"] == "compaction" {
                latest_invalidating_entry_index = i as i64;
                break;
            }
        }
        if usage_entry_index > latest_invalidating_entry_index {
            return estimate;
        }
    }

    let current_system = get_current_system_message(&projection_messages);
    let mut tokens = current_system
        .as_ref()
        .map_or(0.0, estimate_tokens);
    for message in &projection_messages {
        if message["role"] != "system" {
            tokens += estimate_tokens(message);
        }
    }
    ContextUsageEstimate {
        tokens,
        usage_tokens: 0.0,
        trailing_tokens: tokens,
        last_usage_index: None,
    }
}

/// `shouldCompact`: whether compaction should trigger based on context usage.
pub fn should_compact(context_tokens: f64, context_window: f64, settings: &CompactionSettings) -> bool {
    if !settings.enabled {
        return false;
    }
    context_tokens > context_window - settings.reserve_tokens
}

// ============================================================================
// Cut point detection
// ============================================================================

const ESTIMATED_IMAGE_CHARS: usize = 4800;

fn estimate_text_and_image_content_chars(content: &Value) -> usize {
    match content {
        Value::String(s) => js_str_len(s),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| {
                if block["type"] == "text" {
                    // JS `block.text` truthiness guards empty strings too.
                    block["text"].as_str().map_or(0, js_str_len)
                } else if block["type"] == "image" {
                    ESTIMATED_IMAGE_CHARS
                } else {
                    0
                }
            })
            .sum(),
        _ => 0,
    }
}

/// `estimateTokens`: chars/4 heuristic token estimate for one message.
pub fn estimate_tokens(message: &Value) -> f64 {
    let mut chars: usize = 0;

    match message["role"].as_str() {
        Some("system") => {
            chars = estimate_text_and_image_content_chars(&message["content"]);
            if let Some(sections) = message["sections"].as_object() {
                for section in sections.values() {
                    if let Some(s) = section.as_str() {
                        if !s.is_empty() {
                            chars += js_str_len(s);
                        }
                    }
                }
            }
            let tools_added = message.get("toolsAdded");
            if js_truthy(tools_added) {
                chars += js_str_len(&js_stringify(tools_added.unwrap_or(&Value::Null)));
            }
        }
        Some("user") => {
            chars = estimate_text_and_image_content_chars(&message["content"]);
        }
        Some("assistant") => {
            if let Some(blocks) = message["content"].as_array() {
                for block in blocks {
                    match block["type"].as_str() {
                        Some("text") => chars += block["text"].as_str().map_or(0, js_str_len),
                        Some("thinking") => chars += block["thinking"].as_str().map_or(0, js_str_len),
                        Some("toolCall") => {
                            chars += block["name"].as_str().map_or(0, js_str_len);
                            let arguments = block.get("arguments").unwrap_or(&Value::Null);
                            chars += js_str_len(&js_stringify(arguments));
                        }
                        _ => {}
                    }
                }
            }
        }
        Some("custom") | Some("toolResult") => {
            chars = estimate_text_and_image_content_chars(&message["content"]);
        }
        Some("bashExecution") => {
            chars += message["command"].as_str().map_or(0, js_str_len);
            chars += message["output"].as_str().map_or(0, js_str_len);
        }
        Some("branchSummary") | Some("compactionSummary") => {
            chars = message["summary"].as_str().map_or(0, js_str_len);
        }
        _ => return 0.0,
    }

    chars.div_ceil(4) as f64
}

fn is_cut_point_message(message: &Value) -> bool {
    matches!(
        message["role"].as_str(),
        Some("user")
            | Some("assistant")
            | Some("bashExecution")
            | Some("custom")
            | Some("branchSummary")
            | Some("compactionSummary")
    )
}

fn is_turn_start_message(message: &Value) -> bool {
    matches!(
        message["role"].as_str(),
        Some("user")
            | Some("bashExecution")
            | Some("custom")
            | Some("branchSummary")
            | Some("compactionSummary")
    )
}

fn is_turn_start_entry(entry: &Value) -> bool {
    if entry["type"] == "compaction" {
        return false;
    }
    entry_context_messages(entry)
        .iter()
        .any(is_turn_start_message)
}

/// `findValidCutPoints`: indices of context-visible user-like or assistant
/// entries. Never cut at tool results (they must follow their tool call).
fn find_valid_cut_points(entries: &[Value], start_index: usize, end_index: usize) -> Vec<usize> {
    let mut cut_points: Vec<usize> = Vec::new();
    for (i, entry) in entries.iter().enumerate().take(end_index).skip(start_index) {
        if entry["type"] == "compaction" {
            continue;
        }
        if entry_context_messages(entry).iter().any(is_cut_point_message) {
            cut_points.push(i);
        }
    }
    cut_points
}

/// `findTurnStartIndex`: index of the context-visible user-role entry that
/// starts the turn containing `entry_index`, or -1.
pub fn find_turn_start_index(entries: &[Value], entry_index: usize, start_index: usize) -> i64 {
    for i in (start_index..=entry_index).rev() {
        if is_turn_start_entry(&entries[i]) {
            return i as i64;
        }
    }
    -1
}

/// `CutPointResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct CutPointResult {
    /// Index of first entry to keep.
    pub first_kept_entry_index: usize,
    /// Index of the entry that starts the turn being split, or -1.
    pub turn_start_index: i64,
    /// Whether this cut splits a turn.
    pub is_split_turn: bool,
}

impl CutPointResult {
    pub fn to_value(&self) -> Value {
        json!({
            "firstKeptEntryIndex": self.first_kept_entry_index,
            "turnStartIndex": self.turn_start_index,
            "isSplitTurn": self.is_split_turn,
        })
    }
}

/// `findCutPoint`: find the cut point in session entries that keeps
/// approximately `keep_recent_tokens`. Walks backwards accumulating
/// estimated message sizes; cuts at user OR assistant entries (never tool
/// results); backtracks over adjacent context-invisible metadata entries.
pub fn find_cut_point(
    entries: &[Value],
    start_index: usize,
    end_index: usize,
    keep_recent_tokens: f64,
) -> CutPointResult {
    let cut_points = find_valid_cut_points(entries, start_index, end_index);

    if cut_points.is_empty() {
        return CutPointResult {
            first_kept_entry_index: start_index,
            turn_start_index: -1,
            is_split_turn: false,
        };
    }

    // Walk backwards from newest, accumulating estimated message sizes.
    let mut accumulated_tokens = 0.0;
    let mut cut_index = cut_points[0]; // Default: keep from first message (not header).

    'outer: for i in (start_index..end_index).rev() {
        let entry = &entries[i];
        let mut message_tokens = 0.0;
        for message in entry_context_messages(entry) {
            message_tokens += estimate_tokens(&message);
        }
        if message_tokens == 0.0 {
            continue;
        }
        accumulated_tokens += message_tokens;

        if accumulated_tokens >= keep_recent_tokens {
            // Prefer the closest valid cut point at or after this entry.
            cut_index = cut_points
                .iter()
                .copied()
                .find(|candidate| *candidate >= i)
                .unwrap_or(*cut_points.last().unwrap());
            break 'outer;
        }
    }

    // Scan backwards from cutIndex to include adjacent metadata entries that
    // do not affect context. Stop at compaction boundaries or context-visible
    // entries.
    while cut_index > start_index {
        let prev_entry = &entries[cut_index - 1];
        if prev_entry["type"] == "compaction"
            || !entry_context_messages(prev_entry).is_empty()
        {
            break;
        }
        cut_index -= 1;
    }

    // Determine if this is a split turn.
    let cut_entry = &entries[cut_index];
    let starts_turn = is_turn_start_entry(cut_entry);
    let turn_start_index = if starts_turn {
        -1
    } else {
        find_turn_start_index(entries, cut_index, start_index)
    };

    CutPointResult {
        first_kept_entry_index: cut_index,
        turn_start_index,
        is_split_turn: !starts_turn && turn_start_index != -1,
    }
}

// ============================================================================
// Summarization failure detection
// ============================================================================

/// `getSummarizationFailure`: error message when a summarization response
/// cannot safely be persisted (a length stop contains partial text).
pub fn get_summarization_failure(response: &Value, label: &str) -> Option<String> {
    match response["stopReason"].as_str() {
        Some("error") => Some(format!(
            "{} failed: {}",
            label,
            response["errorMessage"]
                .as_str()
                .filter(|m| !m.is_empty())
                .unwrap_or("Unknown error")
        )),
        Some("length") => Some(format!(
            "{} failed: generation hit the token cap and the summary is incomplete",
            label
        )),
        _ => None,
    }
}

// ============================================================================
// Compaction Preparation (for extensions)
// ============================================================================

fn is_projected_turn_start(entry: &Value) -> bool {
    if entry["sourceEntry"]["type"] == "compaction" {
        return false;
    }
    let empty: Vec<Value> = Vec::new();
    entry["messages"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .any(is_turn_start_message)
}

fn find_projected_turn_start_index(entries: &[Value], entry_index: usize, start_index: usize) -> i64 {
    for i in (start_index..=entry_index).rev() {
        if is_projected_turn_start(&entries[i]) {
            return i as i64;
        }
    }
    -1
}

fn is_intrinsically_visible(entry: &Value) -> bool {
    entry["sourceEntry"]["type"] != "context_edit"
        && !entry_context_messages(&entry["sourceEntry"]).is_empty()
}

fn is_omitted(entry: &Value) -> bool {
    is_intrinsically_visible(entry) && entry["messages"].as_array().map_or(0, Vec::len) == 0
}

fn find_projected_cut_point(
    entries: &[Value],
    start_index: usize,
    end_index: usize,
    keep_recent_tokens: f64,
) -> CutPointResult {
    let mut cut_points: Vec<usize> = Vec::new();
    for (i, entry) in entries.iter().enumerate().take(end_index).skip(start_index) {
        if entry["sourceEntry"]["type"] != "compaction" {
            let empty: Vec<Value> = Vec::new();
            if entry["messages"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .any(is_cut_point_message)
            {
                cut_points.push(i);
            }
        }
    }
    if cut_points.is_empty() {
        return CutPointResult {
            first_kept_entry_index: start_index,
            turn_start_index: -1,
            is_split_turn: false,
        };
    }

    let mut accumulated_tokens = 0.0;
    let mut exceeded_budget = false;
    let mut cut_index = cut_points[0];
    'outer: for i in (start_index..end_index).rev() {
        let empty: Vec<Value> = Vec::new();
        let mut message_tokens = 0.0;
        for message in entries[i]["messages"].as_array().unwrap_or(&empty) {
            message_tokens += estimate_tokens(message);
        }
        if message_tokens == 0.0 {
            continue;
        }
        accumulated_tokens += message_tokens;
        if accumulated_tokens >= keep_recent_tokens {
            exceeded_budget = true;
            cut_index = cut_points
                .iter()
                .copied()
                .find(|candidate| *candidate >= i)
                .unwrap_or(*cut_points.last().unwrap());
            break 'outer;
        }
    }

    // A recovery attempt and its omission edits are context-invisible after
    // the last visible input. Advance only for a closed suffix containing an
    // omitted assistant attempt; arbitrary metadata must not move the cut
    // past unsent input.
    let suffix = &entries[(cut_index + 1).min(entries.len())..end_index.min(entries.len())];
    let omitted_suffix_ids: HashSet<&str> = suffix
        .iter()
        .filter(|entry| is_omitted(entry))
        .filter_map(|entry| entry["sourceEntry"]["id"].as_str())
        .collect();
    let has_external_replacement = suffix.iter().any(|entry| {
        entry["sourceEntry"]["type"] == "context_edit"
            && entry["sourceEntry"]
                .get("replacement")
                .is_some_and(|r| !r.is_null())
            && !omitted_suffix_ids.contains(entry["sourceEntry"]["targetId"].as_str().unwrap_or(""))
    });
    let is_recovery_omission_suffix = exceeded_budget
        && !has_external_replacement
        && suffix.iter().any(|entry| {
            entry["sourceEntry"]["type"] == "message"
                && entry["sourceEntry"]["message"]["role"] == "assistant"
                && is_omitted(entry)
        })
        && suffix
            .iter()
            .all(|entry| entry["sourceEntry"]["type"] != "compaction" && (!is_intrinsically_visible(entry) || is_omitted(entry)));
    if is_recovery_omission_suffix {
        cut_index += 1;
    }

    while cut_index > start_index {
        let previous = &entries[cut_index - 1];
        if previous["sourceEntry"]["type"] == "compaction"
            || previous["messages"].as_array().map_or(0, Vec::len) > 0
        {
            break;
        }
        cut_index -= 1;
    }
    let starts_turn = is_projected_turn_start(&entries[cut_index]);
    let turn_start_index = if starts_turn {
        -1
    } else {
        find_projected_turn_start_index(entries, cut_index, start_index)
    };
    CutPointResult {
        first_kept_entry_index: cut_index,
        turn_start_index,
        is_split_turn: !starts_turn && turn_start_index != -1,
    }
}

/// `prepareCompaction`: pre-compute the compaction cut point, messages to
/// summarize, and file operations. Returns `None` when there is nothing to
/// compact. The result is the JSON form of `CompactionPreparation`.
pub fn prepare_compaction(path_entries: &[Value], settings: &CompactionSettings) -> Option<Value> {
    if let Some(last) = path_entries.last() {
        if last["type"] == "compaction" {
            return None;
        }
    }

    let projection = build_session_projection(path_entries, LeafId::Unset, None);
    let projected_entries = projected_entry_values(&projection);
    let source_entries: Vec<Value> = projected_entries
        .iter()
        .map(|entry| entry["sourceEntry"].clone())
        .collect();
    // The newest compaction is projected first. Older compaction entries can
    // still occur in its retained raw range, but their projected contribution
    // is empty.
    let prev_compaction_index: i64 = projected_entries
        .iter()
        .position(|entry| {
            entry["sourceEntry"]["type"] == "compaction"
                && entry["messages"].as_array().is_some_and(|m| !m.is_empty())
        })
        .map_or(-1, |i| i as i64);

    let mut previous_summary: Option<Value> = None;
    let mut boundary_start = 0usize;
    if prev_compaction_index >= 0 {
        // `summary` missing (undefined) is dropped from the JSON form; an
        // explicit null survives, matching JSON.stringify.
        previous_summary =
            projected_entries[prev_compaction_index as usize]["sourceEntry"].get("summary").cloned();
        // The canonical projection has already selected the previous
        // compaction's retained tail.
        boundary_start = (prev_compaction_index + 1) as usize;
    }
    let boundary_end = projected_entries.len();
    let tokens_before = estimate_projected_context_tokens(&projection, path_entries).tokens;
    let cut_point = find_projected_cut_point(
        &projected_entries,
        boundary_start,
        boundary_end,
        settings.keep_recent_tokens,
    );

    let first_kept_entry = projected_entries
        .get(cut_point.first_kept_entry_index)
        .map(|entry| &entry["sourceEntry"]);
    let first_kept_entry_id = first_kept_entry
        .and_then(|entry| entry.get("id"))
        .and_then(Value::as_str)?;
    let history_end = if cut_point.is_split_turn {
        cut_point.turn_start_index.max(0) as usize
    } else {
        cut_point.first_kept_entry_index
    };

    let mut messages_to_summarize: Vec<Value> = Vec::new();
    for entry in projected_entries
        .get(boundary_start..history_end.min(projected_entries.len()))
        .unwrap_or(&[])
    {
        messages_to_summarize.extend(get_messages_from_projected_entry_for_compaction(entry));
    }
    let mut turn_prefix_messages: Vec<Value> = Vec::new();
    if cut_point.is_split_turn {
        for entry in projected_entries
            .get(
                (cut_point.turn_start_index.max(0) as usize)
                    ..cut_point.first_kept_entry_index.min(projected_entries.len()),
            )
            .unwrap_or(&[])
        {
            turn_prefix_messages.extend(get_messages_from_projected_entry_for_compaction(entry));
        }
    }

    if messages_to_summarize.is_empty() && turn_prefix_messages.is_empty() {
        return None;
    }

    // Extract file operations from edited model-visible messages and the
    // previous compaction.
    let mut file_ops = extract_file_operations(&messages_to_summarize, &source_entries, prev_compaction_index);

    // Also extract file ops from turn prefix if splitting.
    if cut_point.is_split_turn {
        for msg in &turn_prefix_messages {
            extract_file_ops_from_message(msg, &mut file_ops);
        }
    }

    let mut out = Map::new();
    out.insert("firstKeptEntryId".into(), json!(first_kept_entry_id));
    out.insert("messagesToSummarize".into(), json!(messages_to_summarize));
    out.insert("turnPrefixMessages".into(), json!(turn_prefix_messages));
    out.insert("isSplitTurn".into(), json!(cut_point.is_split_turn));
    out.insert("tokensBefore".into(), js_number(tokens_before));
    if let Some(previous_summary) = previous_summary {
        out.insert("previousSummary".into(), previous_summary);
    }
    out.insert("fileOps".into(), file_ops.to_value());
    out.insert("settings".into(), settings.to_value());
    Some(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user_msg(text: &str) -> Value {
        json!({"role": "user", "content": text})
    }

    #[test]
    fn calculate_context_tokens_fallbacks() {
        assert_eq!(
            calculate_context_tokens(&json!({"totalTokens": 100, "input": 1})),
            100.0
        );
        assert_eq!(
            calculate_context_tokens(&json!({"totalTokens": 0, "input": 10, "output": 5, "cacheRead": 1, "cacheWrite": 2})),
            18.0
        );
        // Missing components are NaN in JS (`undefined + number`), which
        // callers reject via `> 0`.
        assert!(calculate_context_tokens(&json!({"input": 10, "output": 5})).is_nan());
    }

    #[test]
    fn default_settings_values() {
        let settings = default_compaction_settings();
        assert!(settings.enabled);
        assert_eq!(settings.reserve_tokens, 16384.0);
        assert_eq!(settings.keep_recent_tokens, 20000.0);
    }

    #[test]
    fn should_compact_thresholds() {
        let settings = default_compaction_settings();
        assert!(!should_compact(50000.0, 100000.0, &settings));
        assert!(should_compact(90000.0, 100000.0, &settings));
        assert!(!should_compact(999999.0, 100000.0, &CompactionSettings { enabled: false, ..settings }));
    }

    #[test]
    fn estimate_tokens_roles() {
        // chars/4, rounded up; images count as 4800 chars.
        assert_eq!(estimate_tokens(&user_msg("12345")), 2.0);
        assert_eq!(
            estimate_tokens(&json!({"role": "user", "content": [{"type": "text", "text": "abcd"}, {"type": "image"}]})),
            1201.0
        );
        assert_eq!(estimate_tokens(&json!({"role": "weird"})), 0.0);
        // system: content + sections + toolsAdded JSON.
        let system = json!({
            "role": "system",
            "content": "12345678",
            "sections": {"a": "1234", "b": null, "c": ""},
            "toolsAdded": [{"name": "read"}]
        });
        // 8 + 4 + 17 (JSON of toolsAdded) = 29 chars -> 8 tokens.
        assert_eq!(estimate_tokens(&system), 8.0);
        // TS `for (const block of content)` iterates a string char-by-char and
        // no char has `.type`, so assistant string content estimates to 0.
        assert_eq!(estimate_tokens(&json!({"role": "assistant", "content": "123456789"})), 0.0);
    }

    #[test]
    fn estimate_context_tokens_uses_last_usage() {
        let estimate = estimate_context_tokens(&[
            user_msg("12345678"),
            json!({
                "role": "assistant", "stopReason": "endTurn",
                "content": [{"type": "text", "text": "1234"}],
                "usage": {"input": 400, "output": 100, "cacheRead": 0, "cacheWrite": 0}
            }),
            user_msg("12345678"),
        ]);
        assert_eq!(estimate.usage_tokens, 500.0);
        assert_eq!(estimate.trailing_tokens, 2.0);
        assert_eq!(estimate.tokens, 502.0);
        assert_eq!(estimate.last_usage_index, Some(1));
    }

    #[test]
    fn get_last_assistant_usage_skips_invalid() {
        let entries = vec![
            json!({"type": "message", "message": {"role": "user", "content": "q"}}),
            json!({"type": "message", "message": {
                "role": "assistant", "stopReason": "aborted",
                "usage": {"input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0}
            }}),
            json!({"type": "message", "message": {
                "role": "assistant", "stopReason": "endTurn",
                "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0}
            }}),
        ];
        let usage = get_last_assistant_usage(&entries).unwrap();
        assert_eq!(usage["input"], json!(1));
    }

    #[test]
    fn find_cut_point_walks_backwards() {
        let entries: Vec<Value> = (0..4)
            .map(|i| {
                json!({
                    "type": "message", "id": format!("e{}", i),
                    "parentId": if i == 0 { Value::Null } else { json!(format!("e{}", i - 1)) },
                    "message": {
                        "role": if i % 2 == 0 { "user" } else { "assistant" },
                        "content": [{"type": "text", "text": "x".repeat(400)}]
                    }
                })
            })
            .collect();
        let cut = find_cut_point(&entries, 0, 4, 150.0);
        assert_eq!(cut.first_kept_entry_index, 2);
        assert_eq!(cut.turn_start_index, -1);
        assert!(!cut.is_split_turn);
    }

    #[test]
    fn find_cut_point_split_turn() {
        let entries = vec![
            json!({"type": "message", "id": "e1", "parentId": null, "message": {"role": "user", "content": [{"type": "text", "text": "123456789"}]}}),
            json!({"type": "message", "id": "e2", "parentId": "e1", "message": {"role": "assistant", "content": [{"type": "text", "text": "b".repeat(1200)}]}}),
            json!({"type": "message", "id": "e3", "parentId": "e2", "message": {"role": "user", "content": [{"type": "text", "text": "c".repeat(100)}]}}),
            json!({"type": "message", "id": "e4", "parentId": "e3", "message": {"role": "user", "content": [{"type": "text", "text": "d".repeat(100)}]}}),
        ];
        // Budget exceeded at the big assistant entry: cut there, splitting
        // the turn that the preceding user message started.
        let cut = find_cut_point(&entries, 0, 4, 150.0);
        assert_eq!(cut.first_kept_entry_index, 1);
        assert_eq!(cut.turn_start_index, 0);
        assert!(cut.is_split_turn);
    }

    #[test]
    fn find_cut_point_no_cut_points() {
        let entries = vec![json!({
            "type": "message", "id": "e1", "parentId": null,
            "message": {"role": "toolResult", "content": "ok"}
        })];
        let cut = find_cut_point(&entries, 0, 1, 10.0);
        assert_eq!(cut.first_kept_entry_index, 0);
        assert_eq!(cut.turn_start_index, -1);
    }

    #[test]
    fn get_summarization_failure_reasons() {
        assert_eq!(
            get_summarization_failure(&json!({"stopReason": "error", "errorMessage": "boom"}), "Summarization"),
            Some("Summarization failed: boom".to_string())
        );
        assert_eq!(
            get_summarization_failure(&json!({"stopReason": "error"}), "Summarization"),
            Some("Summarization failed: Unknown error".to_string())
        );
        assert_eq!(
            get_summarization_failure(&json!({"stopReason": "length"}), "Summarization"),
            Some("Summarization failed: generation hit the token cap and the summary is incomplete".to_string())
        );
        assert_eq!(get_summarization_failure(&json!({"stopReason": "endTurn"}), "Summarization"), None);
    }

    #[test]
    fn prepare_compaction_returns_none_when_last_is_compaction() {
        let entries = vec![json!({"type": "compaction", "id": "e1", "summary": "s"})];
        assert!(prepare_compaction(&entries, &default_compaction_settings()).is_none());
    }

    #[test]
    fn prepare_compaction_basic() {
        let entries = vec![
            json!({"type": "message", "id": "e1", "parentId": null, "message": {"role": "user", "content": "a".repeat(400)}}),
            json!({"type": "message", "id": "e2", "parentId": "e1", "message": {"role": "assistant", "content": [{"type": "text", "text": "b".repeat(400)}]}}),
            json!({"type": "message", "id": "e3", "parentId": "e2", "message": {"role": "user", "content": "c".repeat(400)}}),
            json!({"type": "message", "id": "e4", "parentId": "e3", "message": {"role": "user", "content": "d".repeat(400)}}),
        ];
        let prep = prepare_compaction(&entries, &CompactionSettings {
            enabled: true,
            reserve_tokens: 16384.0,
            keep_recent_tokens: 120.0,
        })
        .unwrap();
        assert_eq!(prep["firstKeptEntryId"], json!("e3"));
        assert_eq!(prep["messagesToSummarize"].as_array().unwrap().len(), 2);
        assert_eq!(prep["turnPrefixMessages"], json!([]));
        assert!(!prep["isSplitTurn"].as_bool().unwrap());
        assert!(prep["tokensBefore"].as_f64().unwrap() > 0.0);
        assert!(prep.get("previousSummary").is_none());
    }

    #[test]
    fn extract_file_operations_includes_prev_compaction_details() {
        let messages = vec![json!({
            "role": "assistant",
            "content": [{"type": "toolCall", "name": "read", "arguments": {"path": "/new"}}]
        })];
        let entries = vec![json!({
            "type": "compaction", "id": "c1",
            "details": {"readFiles": ["/old-r"], "modifiedFiles": ["/old-m"]}
        })];
        let ops = extract_file_operations(&messages, &entries, 0);
        assert_eq!(ops.read, vec!["/old-r".to_string(), "/new".to_string()]);
        assert_eq!(ops.edited, vec!["/old-m".to_string()]);
    }

    #[test]
    fn extract_file_operations_skips_hook_compaction() {
        let entries = vec![json!({
            "type": "compaction", "id": "c1", "fromHook": true,
            "details": {"readFiles": ["/old-r"], "modifiedFiles": []}
        })];
        let ops = extract_file_operations(&[], &entries, 0);
        assert!(ops.read.is_empty());
    }
}
