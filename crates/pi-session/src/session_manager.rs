//! Session manager: append-only conversation trees stored as JSONL files.
//!
//! Port of `src/core/session-manager.ts`. Entries are heterogeneous JSON
//! objects (extensions can attach arbitrary data), so they are represented
//! as `serde_json::Value` throughout and discriminated by their `type`
//! field. Object key order is preserved (serde_json `preserve_order`),
//! matching `JSON.stringify` in the original so session files are
//! byte-identical.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use pi_core::ai;
use pi_core::messages::parse_date_millis;
use pi_core::paths::{normalize_path, resolve_path, PathInputOptions};
use pi_core::text_decoder::StreamingDecoder;
use serde_json::{json, Map, Value};

use crate::config::{node_dirname, node_join, APP_NAME};

pub const CURRENT_SESSION_VERSION: i64 = 3;

const SESSION_READ_BUFFER_SIZE: usize = 1024 * 1024;
const SESSION_HEADER_READ_BUFFER_SIZE: usize = 4096;
/// Bound synchronous header discovery while allowing large cwd and custom
/// metadata fields.
const MAX_SESSION_HEADER_SCAN_BYTES: usize = 1024 * 1024;

/// Options for [`SessionManager::new_session`].
#[derive(Debug, Clone, Default)]
pub struct NewSessionOptions {
    pub id: Option<String>,
    pub parent_session: Option<String>,
}

/// The `leafId?: string | null` parameter of the pure tree functions:
/// `undefined` (absent), `null` (before any entries), or an explicit leaf.
#[derive(Debug, Clone)]
pub enum LeafSpec {
    /// `leafId` undefined — default to the last entry.
    Latest,
    /// `leafId` null — empty path.
    Root,
    /// An explicit leaf entry id.
    At(String),
}

pub fn leaf_spec_from_value(value: Option<&Value>) -> LeafSpec {
    match value {
        None => LeafSpec::Latest,
        Some(Value::Null) => LeafSpec::Root,
        Some(v) => LeafSpec::At(v.as_str().unwrap_or("").to_string()),
    }
}

// ============================================================================
// Small JS-semantics helpers
// ============================================================================

/// JavaScript truthiness for JSON values.
pub(crate) fn js_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

/// `new Date(x).getTime()` for JSON values: numbers are millis, strings are
/// parsed (strict ISO-8601 like the rest of the port), `null` is epoch 0,
/// anything else is an invalid date (`None`).
fn js_date_get_time(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(n)) => n.as_f64().map(|f| f as i64),
        Some(Value::String(s)) => parse_date_millis(s),
        Some(Value::Null) => Some(0),
        _ => None,
    }
}

/// The process working directory (used where the original calls
/// `resolvePath(input)` with the default `process.cwd()` base).
pub fn process_cwd() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `resolvePath(input)` with the default base directory.
pub fn resolve_from_process(input: &str) -> String {
    resolve_path(input, &process_cwd(), &PathInputOptions::default())
}

// ============================================================================
// Ids
// ============================================================================

/// `createSessionId()` — a uuidv7.
pub fn create_session_id() -> Result<String, String> {
    ai::uuidv7(None)
}

/// `assertValidSessionId`.
pub fn assert_valid_session_id(id: &str) -> Result<(), String> {
    let valid = !id.is_empty()
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id.chars().last().is_some_and(|c| c.is_ascii_alphanumeric())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return Err(
            "Session id must be non-empty, contain only alphanumeric characters, '-', '_', \
             and '.', and start and end with an alphanumeric character"
                .to_string(),
        );
    }
    Ok(())
}

/// Generate a unique short ID (8 hex chars, collision-checked).
pub fn generate_id(exists: &impl Fn(&str) -> bool) -> String {
    for _ in 0..100 {
        let id = ai::random_uuid()[..8].to_string();
        if !exists(&id) {
            return id;
        }
    }
    // Fallback to full UUID if somehow we have collisions
    ai::random_uuid()
}

fn now_iso() -> String {
    ai::date_to_iso_string(ai::date_now_millis())
}

// ============================================================================
// Migrations
// ============================================================================

/// Migrate v1 → v2: add id/parentId tree structure. Mutates in place.
fn migrate_v1_to_v2(entries: &mut [Value]) {
    let mut ids: HashSet<String> = HashSet::new();
    let mut prev_id: Option<String> = None;

    for i in 0..entries.len() {
        if entries[i]["type"] == "session" {
            if let Some(m) = entries[i].as_object_mut() {
                m.insert("version".to_string(), json!(2));
            }
            continue;
        }

        let id = generate_id(&|c| ids.contains(c));
        ids.insert(id.clone());
        if let Some(m) = entries[i].as_object_mut() {
            m.insert("id".to_string(), json!(id.clone()));
            m.insert(
                "parentId".to_string(),
                prev_id.clone().map(Value::String).unwrap_or(Value::Null),
            );
        }
        prev_id = Some(id);

        // Convert firstKeptEntryIndex to firstKeptEntryId for compaction.
        if entries[i]["type"] == "compaction" {
            if let Some(index) = entries[i].get("firstKeptEntryIndex").and_then(Value::as_u64) {
                let index = index as usize;
                let target_id = entries
                    .get(index)
                    .filter(|t| t["type"] != "session")
                    .and_then(|t| t.get("id").cloned())
                    .filter(|v| v.is_string());
                if let Some(m) = entries[i].as_object_mut() {
                    if let Some(target_id) = target_id {
                        m.insert("firstKeptEntryId".to_string(), target_id);
                    }
                    m.remove("firstKeptEntryIndex");
                }
            }
        }
    }
}

/// Migrate v2 → v3: rename hookMessage role to custom. Mutates in place.
fn migrate_v2_to_v3(entries: &mut [Value]) {
    for entry in entries.iter_mut() {
        if entry["type"] == "session" {
            if let Some(m) = entry.as_object_mut() {
                m.insert("version".to_string(), json!(3));
            }
            continue;
        }
        if entry["type"] == "message" && entry["message"]["role"] == "hookMessage" {
            if let Some(message) = entry.get_mut("message").and_then(Value::as_object_mut) {
                message.insert("role".to_string(), json!("custom"));
            }
        }
    }
}

/// Run all necessary migrations to bring entries to current version.
/// Mutates entries in place. Returns true if any migration was applied.
fn migrate_to_current_version(entries: &mut [Value]) -> bool {
    let header_version = entries
        .iter()
        .find(|e| e["type"] == "session")
        .and_then(|h| h.get("version"))
        .and_then(|v| match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        })
        .unwrap_or(1.0);

    if header_version >= CURRENT_SESSION_VERSION as f64 {
        return false;
    }

    if header_version < 2.0 {
        migrate_v1_to_v2(entries);
    }
    if header_version < 3.0 {
        migrate_v2_to_v3(entries);
    }

    true
}

/// Exported for testing: bring entries to the current session version.
pub fn migrate_session_entries(entries: &mut [Value]) {
    migrate_to_current_version(entries);
}

// ============================================================================
// Parsing
// ============================================================================

/// Parse one JSONL line; blank and malformed lines yield `None`.
fn parse_session_entry_line(line: &str) -> Option<Value> {
    if line.trim().is_empty() {
        return None;
    }
    serde_json::from_str(line).ok()
}

/// `parseSessionEntries`.
pub fn parse_session_entries(content: &str) -> Vec<Value> {
    let mut entries = Vec::new();
    for line in content.trim().split('\n') {
        if let Some(entry) = parse_session_entry_line(line) {
            entries.push(entry);
        }
    }
    entries
}

/// `getLatestCompactionEntry`.
pub fn get_latest_compaction_entry(entries: &[Value]) -> Option<Value> {
    entries.iter().rev().find(|e| e["type"] == "compaction").cloned()
}

// ============================================================================
// Tree traversal (pure functions)
// ============================================================================

fn build_entry_index(entries: &[Value]) -> HashMap<String, usize> {
    let mut index = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        if let Some(id) = entry.get("id").and_then(Value::as_str) {
            index.insert(id.to_string(), i);
        }
    }
    index
}

/// `buildSessionPath`: walk the parent chain from a leaf to the root.
pub fn build_session_path(entries: &[Value], leaf: &LeafSpec) -> Vec<Value> {
    if matches!(leaf, LeafSpec::Root) {
        return Vec::new();
    }
    let index = build_entry_index(entries);
    let leaf_pos = match leaf {
        LeafSpec::At(id) => index.get(id).copied(),
        _ => None,
    }
    .or_else(|| (!entries.is_empty()).then(|| entries.len() - 1));
    let Some(mut current) = leaf_pos else {
        return Vec::new();
    };

    let mut path = Vec::new();
    loop {
        path.push(entries[current].clone());
        let next = entries[current]
            .get("parentId")
            .and_then(Value::as_str)
            .filter(|pid| !pid.is_empty())
            .and_then(|pid| index.get(pid).copied());
        match next {
            Some(i) => current = i,
            None => break,
        }
    }
    path.reverse();
    path
}

fn get_session_context_settings(path: &[Value]) -> (String, Option<Value>) {
    let mut thinking_level = "off".to_string();
    let mut model: Option<Value> = None;

    for entry in path {
        match entry["type"].as_str() {
            Some("thinking_level_change") => {
                thinking_level = entry["thinkingLevel"].as_str().unwrap_or_default().to_string();
            }
            Some("model_change") => {
                let mut m = Map::new();
                if let Some(p) = entry.get("provider") {
                    m.insert("provider".to_string(), p.clone());
                }
                if let Some(mi) = entry.get("modelId") {
                    m.insert("modelId".to_string(), mi.clone());
                }
                model = Some(Value::Object(m));
            }
            Some("message") if entry["message"]["role"] == "assistant" => {
                let message = &entry["message"];
                let mut m = Map::new();
                if let Some(p) = message.get("provider") {
                    m.insert("provider".to_string(), p.clone());
                }
                if let Some(mi) = message.get("model") {
                    m.insert("modelId".to_string(), mi.clone());
                }
                model = Some(Value::Object(m));
            }
            _ => {}
        }
    }

    (thinking_level, model)
}

/// `sessionEntryToContextMessages`: project one session entry into
/// LLM/runtime messages.
pub fn session_entry_to_context_messages(entry: &Value) -> Vec<Value> {
    match entry["type"].as_str() {
        Some("message") => {
            let message = entry.get("message").cloned().unwrap_or(Value::Null);
            if !message.is_object() {
                return vec![message];
            }
            let role = message["role"].as_str().unwrap_or("");
            let content_null = matches!(message.get("content"), None | Some(Value::Null));
            if content_null {
                // Session files are parsed without validation; old versions,
                // forks, or hand-edited files can contain messages with
                // null/missing content.
                if role == "system" {
                    let mut m = message;
                    m["content"] = json!("");
                    return vec![m];
                }
                if matches!(role, "user" | "assistant" | "toolResult") {
                    let mut m = message;
                    m["content"] = json!([]);
                    return vec![m];
                }
            }
            vec![message]
        }
        Some("custom_message") => {
            // createCustomMessage(customType, content ?? [], display, details, timestamp)
            let mut m = Map::new();
            m.insert("role".to_string(), json!("custom"));
            if let Some(ct) = entry.get("customType") {
                m.insert("customType".to_string(), ct.clone());
            }
            let content = match entry.get("content") {
                None | Some(Value::Null) => json!([]),
                Some(c) => c.clone(),
            };
            m.insert("content".to_string(), content);
            if let Some(d) = entry.get("display") {
                m.insert("display".to_string(), d.clone());
            }
            if let Some(d) = entry.get("details") {
                m.insert("details".to_string(), d.clone());
            }
            let ts = js_date_get_time(entry.get("timestamp"));
            m.insert("timestamp".to_string(), ts.map(|t| json!(t)).unwrap_or(Value::Null));
            vec![Value::Object(m)]
        }
        Some("branch_summary") if js_truthy(entry.get("summary")) => {
            // createBranchSummaryMessage(summary, fromId, timestamp)
            let mut m = Map::new();
            m.insert("role".to_string(), json!("branchSummary"));
            if let Some(s) = entry.get("summary") {
                m.insert("summary".to_string(), s.clone());
            }
            if let Some(f) = entry.get("fromId") {
                m.insert("fromId".to_string(), f.clone());
            }
            let ts = js_date_get_time(entry.get("timestamp"));
            m.insert("timestamp".to_string(), ts.map(|t| json!(t)).unwrap_or(Value::Null));
            vec![Value::Object(m)]
        }
        Some("compaction") => {
            // createCompactionSummaryMessage(summary, tokensBefore, timestamp)
            let mut summary_message = Map::new();
            summary_message.insert("role".to_string(), json!("compactionSummary"));
            if let Some(s) = entry.get("summary") {
                summary_message.insert("summary".to_string(), s.clone());
            }
            if let Some(t) = entry.get("tokensBefore") {
                summary_message.insert("tokensBefore".to_string(), t.clone());
            }
            let ts = js_date_get_time(entry.get("timestamp"));
            summary_message.insert(
                "timestamp".to_string(),
                ts.map(|t| json!(t)).unwrap_or(Value::Null),
            );
            let mut out = Vec::new();
            if js_truthy(entry.get("systemMessage")) {
                out.push(entry["systemMessage"].clone());
            }
            out.push(Value::Object(summary_message));
            out
        }
        _ => Vec::new(),
    }
}

/// `buildContextEntries`: the active, compaction-aware session entry list.
pub fn build_context_entries(entries: &[Value], leaf: &LeafSpec) -> Vec<Value> {
    let path = build_session_path(entries, leaf);
    let mut compaction: Option<&Value> = None;
    for entry in &path {
        if entry["type"] == "compaction" {
            compaction = Some(entry);
        }
    }

    let Some(compaction) = compaction else {
        return path;
    };

    let compaction_idx = match path
        .iter()
        .position(|entry| entry.get("id") == compaction.get("id"))
    {
        Some(idx) => idx,
        None => return path,
    };

    let first_kept_id = compaction.get("firstKeptEntryId");
    let mut context_entries = vec![compaction.clone()];
    let mut found_first_kept = false;
    for entry in &path[..compaction_idx] {
        if entry.get("id") == first_kept_id {
            found_first_kept = true;
        }
        if found_first_kept && !(entry["type"] == "message" && entry["message"]["role"] == "system") {
            context_entries.push(entry.clone());
        }
    }
    context_entries.extend(path[compaction_idx + 1..].iter().cloned());
    context_entries
}

/// `projectContextEntry`: apply one append-only context edit.
fn project_context_entry(entry: &Value, edit: Option<&Value>) -> Vec<Value> {
    let messages = session_entry_to_context_messages(entry);
    let Some(edit) = edit else {
        return messages;
    };
    if matches!(edit.get("replacement"), Some(Value::Null)) {
        return Vec::new();
    }
    let content = edit.get("replacement").and_then(|r| r.get("content"));
    messages
        .into_iter()
        .map(|mut message| {
            let role = message["role"].as_str().unwrap_or("").to_string();
            if !matches!(role.as_str(), "user" | "assistant" | "toolResult" | "custom") {
                return message;
            }
            if (role == "assistant" || role == "toolResult")
                && content.is_some_and(|c| c.is_string())
            {
                message["content"] = json!([{ "type": "text", "text": content.unwrap() }]);
            } else if let Some(c) = content {
                message["content"] = c.clone();
            } else if let Some(m) = message.as_object_mut() {
                // `{ ...message, content: undefined }` — the key is dropped.
                m.remove("content");
            }
            message
        })
        .collect()
}

/// `buildSessionProjection` result.
#[derive(Debug, Clone)]
pub struct SessionProjection {
    pub entries: Vec<Value>,
    pub messages: Vec<Value>,
    pub thinking_level: String,
    pub model: Option<Value>,
}

impl SessionProjection {
    pub fn to_value(&self) -> Value {
        let entries: Vec<Value> = self
            .entries
            .iter()
            .map(|p| json!({ "sourceEntry": p["sourceEntry"], "messages": p["messages"] }))
            .collect();
        json!({
            "entries": entries,
            "messages": self.messages,
            "thinkingLevel": self.thinking_level,
            "model": self.model.clone().unwrap_or(Value::Null),
        })
    }
}

/// `buildSessionProjection`: provenance-preserving, compaction-aware model
/// context.
pub fn build_session_projection(entries: &[Value], leaf: &LeafSpec) -> SessionProjection {
    let path = build_session_path(entries, leaf);
    let (thinking_level, model) = get_session_context_settings(&path);
    let context_entries = build_context_entries(entries, leaf);
    let mut edits: HashMap<String, &Value> = HashMap::new();
    for entry in &context_entries {
        if entry["type"] == "context_edit" {
            if let Some(target_id) = entry.get("targetId").and_then(Value::as_str) {
                edits.insert(target_id.to_string(), entry);
            }
        }
    }
    let mut projected_entries = Vec::with_capacity(context_entries.len());
    let mut messages = Vec::new();
    for (index, source_entry) in context_entries.iter().enumerate() {
        let entry_messages = if source_entry["type"] == "compaction" && index > 0 {
            // buildContextEntries() may retain an older compaction entry
            // because its raw ID lies inside the newest retained range. Only
            // the newest compaction at index zero contributes a checkpoint
            // and summary.
            Vec::new()
        } else {
            let edit = source_entry
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| edits.get(id).copied());
            project_context_entry(source_entry, edit)
        };
        messages.extend(entry_messages.iter().cloned());
        projected_entries.push(json!({
            "sourceEntry": source_entry,
            "messages": entry_messages,
        }));
    }
    SessionProjection {
        entries: projected_entries,
        messages,
        thinking_level,
        model,
    }
}

/// `buildSessionContext`: the finalized model context.
pub fn build_session_context(entries: &[Value], leaf: &LeafSpec) -> Value {
    let projection = build_session_projection(entries, leaf);
    json!({
        "messages": projection.messages,
        "thinkingLevel": projection.thinking_level,
        "model": projection.model.unwrap_or(Value::Null),
    })
}

// ============================================================================
// Session directory
// ============================================================================

/// `getDefaultSessionDirPath`: encode cwd into a safe directory name under
/// `<agentDir>/sessions/`.
pub fn get_default_session_dir_path(cwd: &str, agent_dir: &str) -> String {
    let resolved_cwd = resolve_from_process(cwd);
    let resolved_agent_dir = resolve_from_process(agent_dir);
    let stripped = resolved_cwd
        .strip_prefix(|c| c == '/' || c == '\\')
        .unwrap_or(&resolved_cwd);
    let safe_path = format!(
        "--{}--",
        stripped.replace(['/', '\\', ':'], "-")
    );
    node_join(&resolved_agent_dir, &["sessions", &safe_path])
}

/// `getDefaultSessionDir`.
pub fn get_default_session_dir(cwd: &str, agent_dir: Option<&str>) -> Result<String, String> {
    let agent_dir = agent_dir
        .map(str::to_string)
        .unwrap_or_else(crate::config::get_agent_dir);
    let session_dir = get_default_session_dir_path(cwd, &agent_dir);
    if !Path::new(&session_dir).exists() {
        fs::create_dir_all(&session_dir).map_err(|e| e.to_string())?;
    }
    Ok(session_dir)
}

// ============================================================================
// File loading
// ============================================================================

/// `loadEntriesFromFile`: stream-parse a JSONL session file (1MB chunks with
/// incremental UTF-8 decoding), validating the header and repairing a
/// missing trailing newline.
pub fn load_entries_from_file(file_path: &str) -> Result<Vec<Value>, String> {
    let resolved_file_path = normalize_path(file_path, &PathInputOptions::default());
    if !Path::new(&resolved_file_path).exists() {
        return Ok(Vec::new());
    }

    let mut file = File::open(&resolved_file_path).map_err(|e| e.to_string())?;
    let mut entries: Vec<Value> = Vec::new();
    let mut pending = String::new();
    let mut decoder = StreamingDecoder::new();
    let mut buffer = vec![0u8; SESSION_READ_BUFFER_SIZE];

    loop {
        let bytes_read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if bytes_read == 0 {
            break;
        }
        pending.push_str(&decoder.decode(&buffer[..bytes_read]));
        let mut line_start = 0;
        while let Some(offset) = pending[line_start..].find('\n') {
            let line = &pending[line_start..line_start + offset];
            if let Some(entry) = parse_session_entry_line(line) {
                entries.push(entry);
            }
            line_start += offset + 1;
        }
        pending.drain(..line_start);
    }

    pending.push_str(&decoder.finish());
    let had_pending = !pending.is_empty();
    if let Some(final_entry) = parse_session_entry_line(&pending) {
        entries.push(final_entry);
    }

    // Validate session header before repairing the file.
    if entries.is_empty() {
        return Ok(entries);
    }
    let header = &entries[0];
    if header["type"] != "session" || !header.get("id").is_some_and(|v| v.is_string()) {
        return Ok(Vec::new());
    }

    if had_pending {
        let mut f = OpenOptions::new()
            .append(true)
            .open(&resolved_file_path)
            .map_err(|e| e.to_string())?;
        f.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    Ok(entries)
}

/// Inspect a physical line while searching for the first parsed session
/// entry. `None` keeps scanning, `Some(None)` is a parsed non-header entry,
/// `Some(Some(header))` is the header.
fn parse_session_header_candidate(line: &str) -> Option<Option<Value>> {
    if line.trim().is_empty() {
        return None;
    }
    let entry = parse_session_entry_line(line)?;
    if entry["type"] != "session" || !entry.get("id").is_some_and(|v| v.is_string()) {
        return Some(None);
    }
    Some(Some(entry))
}

/// `readSessionHeader`: bounded synchronous header discovery (4KB reads,
/// 1MB scan limit). Errors with the scan-limit message when the header is
/// too deep.
pub fn read_session_header(file_path: &str) -> Result<Option<Value>, String> {
    let mut file = File::open(file_path).map_err(|e| e.to_string())?;
    let mut decoder = StreamingDecoder::new();
    let mut buffer = vec![0u8; SESSION_HEADER_READ_BUFFER_SIZE];
    let mut line_buf = String::new();
    let mut scanned_bytes = 0usize;

    while scanned_bytes < MAX_SESSION_HEADER_SCAN_BYTES {
        let read_length = buffer.len().min(MAX_SESSION_HEADER_SCAN_BYTES - scanned_bytes);
        let bytes_read = file.read(&mut buffer[..read_length]).map_err(|e| e.to_string())?;
        if bytes_read == 0 {
            line_buf.push_str(&decoder.finish());
            return Ok(parse_session_header_candidate(&line_buf).flatten());
        }
        scanned_bytes += bytes_read;

        let chunk = decoder.decode(&buffer[..bytes_read]);
        let mut line_start = 0;
        while let Some(offset) = chunk[line_start..].find('\n') {
            line_buf.push_str(&chunk[line_start..line_start + offset]);
            if let Some(result) = parse_session_header_candidate(&line_buf) {
                return Ok(result);
            }
            line_buf.clear();
            line_start += offset + 1;
        }
        line_buf.push_str(&chunk[line_start..]);
    }

    // Probe for EOF so a final header without a newline is allowed when it
    // ends exactly at the scan limit.
    let mut probe = [0u8; 1];
    if file.read(&mut probe).map_err(|e| e.to_string())? == 0 {
        line_buf.push_str(&decoder.finish());
        return Ok(parse_session_header_candidate(&line_buf).flatten());
    }
    Err(format!(
        "Session header exceeds {}-byte scan limit: {}",
        MAX_SESSION_HEADER_SCAN_BYTES, file_path
    ))
}

/// `readSessionHeaderForDiscovery`: best-effort header read.
fn read_session_header_for_discovery(file_path: &str) -> Option<Value> {
    read_session_header(file_path).ok().flatten()
}

fn get_session_header_cwd(header: &Value) -> Option<&str> {
    header.get("cwd").and_then(Value::as_str)
}

fn session_cwd_matches(cwd: Option<&str>, resolved_cwd: &str) -> bool {
    match cwd {
        Some(cwd) if !cwd.is_empty() => resolve_from_process(cwd) == resolved_cwd,
        _ => false,
    }
}

/// `findMostRecentSession`.
pub fn find_most_recent_session(session_dir: &str, cwd: Option<&str>) -> Option<String> {
    let resolved_session_dir = normalize_path(session_dir, &PathInputOptions::default());
    let resolved_cwd = cwd.map(resolve_from_process);
    let inner = || -> Option<String> {
        let mut files: Vec<(String, f64)> = Vec::new();
        for name in fs::read_dir(&resolved_session_dir).ok()?.flatten() {
            let name = name.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".jsonl") {
                continue;
            }
            let path = node_join(&resolved_session_dir, &[&name]);
            let mtime = stat_mtime_ms(&path)?;
            files.push((path, mtime));
        }
        files.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (path, _) in files {
            let header = read_session_header_for_discovery(&path);
            if let Some(header) = header {
                let matches = match &resolved_cwd {
                    Some(resolved) => session_cwd_matches(get_session_header_cwd(&header), resolved),
                    None => true,
                };
                if matches {
                    return Some(path);
                }
            }
        }
        None
    };
    inner()
}

fn stat_mtime_ms(path: &str) -> Option<f64> {
    let md = fs::metadata(path).ok()?;
    let mt = md.modified().ok()?;
    let d = mt.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_secs() as f64 * 1000.0 + d.subsec_nanos() as f64 / 1e6)
}

// ============================================================================
// Session listing
// ============================================================================

/// `SessionInfo`.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub path: String,
    pub id: String,
    pub cwd: String,
    pub name: Option<String>,
    pub parent_session_path: Option<String>,
    /// `Date.getTime()` millis (NaN for invalid dates is kept as `None`).
    pub created: Option<i64>,
    pub modified: Option<i64>,
    pub message_count: u64,
    pub first_message: String,
    pub all_messages_text: String,
}

impl SessionInfo {
    /// JSON shape of the original (dates serialize via `toISOString`,
    /// invalid dates as null).
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("path".to_string(), json!(self.path));
        m.insert("id".to_string(), json!(self.id));
        m.insert("cwd".to_string(), json!(self.cwd));
        if let Some(name) = &self.name {
            m.insert("name".to_string(), json!(name));
        }
        if let Some(parent) = &self.parent_session_path {
            m.insert("parentSessionPath".to_string(), json!(parent));
        }
        m.insert(
            "created".to_string(),
            self.created
                .map(|t| json!(ai::date_to_iso_string(t as f64)))
                .unwrap_or(Value::Null),
        );
        m.insert(
            "modified".to_string(),
            self.modified
                .map(|t| json!(ai::date_to_iso_string(t as f64)))
                .unwrap_or(Value::Null),
        );
        m.insert("messageCount".to_string(), json!(self.message_count));
        m.insert("firstMessage".to_string(), json!(self.first_message));
        m.insert("allMessagesText".to_string(), json!(self.all_messages_text));
        Value::Object(m)
    }
}

fn is_message_with_content(message: &Value) -> bool {
    message.is_object()
        && message.get("role").is_some_and(|r| r.is_string())
        && message.get("content").is_some()
}

fn extract_text_content(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .map(|b| b.get("text").and_then(Value::as_str).unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn get_message_activity_time(entry: &Value) -> Option<f64> {
    let message = &entry["message"];
    if !is_message_with_content(message) {
        return None;
    }
    let role = message["role"].as_str();
    if !matches!(role, Some("user") | Some("assistant")) {
        return None;
    }
    if let Some(t) = message.get("timestamp").and_then(Value::as_f64) {
        return Some(t);
    }
    js_date_get_time(entry.get("timestamp")).map(|t| t as f64)
}

/// `buildSessionInfo` (synchronous port).
fn build_session_info(file_path: &str, file_mtime_ms: Option<f64>) -> Option<SessionInfo> {
    let stats_mtime = file_mtime_ms.or_else(|| stat_mtime_ms(file_path))?;
    let bytes = fs::read(file_path).ok()?;
    let content = String::from_utf8_lossy(&bytes);

    let mut header: Option<Value> = None;
    let mut message_count = 0u64;
    let mut first_message = String::new();
    let mut all_messages: Vec<String> = Vec::new();
    let mut name: Option<String> = None;
    let mut last_activity_time: Option<f64> = None;

    for line in content.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some(entry) = parse_session_entry_line(line) else {
            continue;
        };

        if header.is_none() {
            if entry["type"] != "session" {
                return None;
            }
            header = Some(entry);
            continue;
        }

        // Extract session name (use latest, including explicit clears).
        if entry["type"] == "session_info" {
            name = entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        }

        if entry["type"] != "message" {
            continue;
        }
        message_count += 1;

        if let Some(activity_time) = get_message_activity_time(&entry) {
            last_activity_time = Some(last_activity_time.map_or(activity_time, |l| l.max(activity_time)));
        }

        let message = &entry["message"];
        if !is_message_with_content(message) {
            continue;
        }
        if !matches!(message["role"].as_str(), Some("user") | Some("assistant")) {
            continue;
        }

        let text_content = extract_text_content(message);
        if text_content.is_empty() {
            continue;
        }

        all_messages.push(text_content.clone());
        if first_message.is_empty() && message["role"] == "user" {
            first_message = text_content;
        }
    }

    let header = header?;
    let cwd = header.get("cwd").and_then(Value::as_str).unwrap_or("").to_string();
    let parent_session_path = header.get("parentSession").and_then(Value::as_str).map(str::to_string);
    let header_time = js_date_get_time(header.get("timestamp"));
    let modified = match last_activity_time {
        Some(last) if last > 0.0 => Some(last as i64),
        _ => header_time.or(Some(stats_mtime as i64)),
    };

    Some(SessionInfo {
        path: file_path.to_string(),
        id: header.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
        cwd,
        name,
        parent_session_path,
        created: header_time,
        modified,
        message_count,
        first_message: if first_message.is_empty() {
            "(no messages)".to_string()
        } else {
            first_message
        },
        all_messages_text: all_messages.join(" "),
    })
}

fn sort_session_infos(sessions: &mut [SessionInfo]) {
    sessions.sort_by(|a, b| {
        let am = a.modified.map(|t| t as f64).unwrap_or(f64::NAN);
        let bm = b.modified.map(|t| t as f64).unwrap_or(f64::NAN);
        bm.partial_cmp(&am).unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// `listSessionsFromDir` (synchronous port; no progress callbacks).
pub fn list_sessions_from_dir(dir: &str) -> Vec<SessionInfo> {
    if !Path::new(dir).exists() {
        return Vec::new();
    }
    let Ok(dir_entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = dir_entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".jsonl"))
        .collect();
    // `.sort((a, b) => b.localeCompare(a))` — descending byte order.
    names.sort_by(|a, b| b.cmp(a));
    names
        .iter()
        .map(|name| node_join(dir, &[name]))
        .filter_map(|path| build_session_info(&path, None))
        .collect()
}

// ============================================================================
// SessionManager
// ============================================================================

#[derive(Debug, Clone)]
struct LabelSlot {
    target_id: String,
    label: String,
    timestamp: String,
}

/// Manages conversation sessions as append-only trees stored in JSONL files.
pub struct SessionManager {
    session_id: String,
    session_file: Option<String>,
    session_dir: String,
    cwd: String,
    persist: bool,
    flushed: bool,
    file_entries: Vec<Value>,
    by_id: HashMap<String, usize>,
    /// Insertion-ordered label map (targetId, label, timestamp).
    labels: Vec<LabelSlot>,
    leaf_id: Option<String>,
}

fn write_entry_lines(file: &mut File, entries: &[Value]) -> Result<(), String> {
    for entry in entries {
        let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        file.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl SessionManager {
    fn new(
        cwd: &str,
        session_dir: &str,
        session_file: Option<&str>,
        persist: bool,
        new_session_options: Option<&NewSessionOptions>,
        preloaded_file_entries: Option<Vec<Value>>,
    ) -> Result<Self, String> {
        let cwd = resolve_from_process(cwd);
        let session_dir = normalize_path(session_dir, &PathInputOptions::default());
        let mut mgr = SessionManager {
            session_id: String::new(),
            session_file: None,
            session_dir,
            cwd,
            persist,
            flushed: false,
            file_entries: Vec::new(),
            by_id: HashMap::new(),
            labels: Vec::new(),
            leaf_id: None,
        };
        if mgr.persist && !mgr.session_dir.is_empty() && !Path::new(&mgr.session_dir).exists() {
            fs::create_dir_all(&mgr.session_dir).map_err(|e| e.to_string())?;
        }

        if let Some(session_file) = session_file {
            mgr.set_session_file_internal(session_file, preloaded_file_entries)?;
        } else if preloaded_file_entries.as_ref().is_some_and(|e| !e.is_empty()) {
            mgr.load_entries_internal(preloaded_file_entries.unwrap_or_default(), new_session_options)?;
        } else {
            mgr.new_session(new_session_options)?;
        }
        Ok(mgr)
    }

    /// Switch to a different session file (used for resume and branching).
    pub fn set_session_file(&mut self, session_file: &str) -> Result<(), String> {
        self.set_session_file_internal(session_file, None)
    }

    fn set_session_file_internal(
        &mut self,
        session_file: &str,
        preloaded_file_entries: Option<Vec<Value>>,
    ) -> Result<(), String> {
        let resolved = resolve_from_process(session_file);
        self.session_file = Some(resolved.clone());
        if Path::new(&resolved).exists() {
            let entries = match preloaded_file_entries {
                Some(entries) => entries,
                None => load_entries_from_file(&resolved)?,
            };

            // If file was empty, initialize it with a valid session header.
            // If it was non-empty but did not parse as a pi session, fail
            // without modifying it.
            if entries.is_empty() {
                let explicit_path = resolved.clone();
                let size = fs::metadata(&explicit_path)
                    .map_err(|e| e.to_string())?
                    .len();
                if size > 0 {
                    return Err(format!(
                        "Session file is not a valid {} session: {}",
                        APP_NAME, explicit_path
                    ));
                }
                self.new_session(None)?;
                self.session_file = Some(explicit_path);
                self.rewrite_file()?;
                self.flushed = true;
                return Ok(());
            }

            self.load_entries_internal(entries, None)?;
            self.flushed = true;
        } else {
            let explicit_path = resolved.clone();
            self.new_session(None)?;
            self.session_file = Some(explicit_path); // preserve explicit path from --session flag
        }
        Ok(())
    }

    /// Start a new session. Returns the session file path when persisting.
    pub fn new_session(&mut self, options: Option<&NewSessionOptions>) -> Result<Option<String>, String> {
        if let Some(id) = options.and_then(|o| o.id.as_deref()) {
            assert_valid_session_id(id)?;
        }
        self.session_id = match options.and_then(|o| o.id.clone()) {
            Some(id) => id,
            None => create_session_id()?,
        };
        let timestamp = now_iso();
        let mut header = Map::new();
        header.insert("type".to_string(), json!("session"));
        header.insert("version".to_string(), json!(CURRENT_SESSION_VERSION));
        header.insert("id".to_string(), json!(self.session_id));
        header.insert("timestamp".to_string(), json!(timestamp));
        header.insert("cwd".to_string(), json!(self.cwd));
        if let Some(parent) = options.and_then(|o| o.parent_session.as_deref()) {
            header.insert("parentSession".to_string(), json!(parent));
        }
        self.file_entries = vec![Value::Object(header)];
        self.by_id.clear();
        self.labels.clear();
        self.leaf_id = None;
        self.flushed = false;

        if self.persist {
            let file_timestamp = timestamp.replace([':', '.'], "-");
            self.session_file = Some(node_join(
                &self.session_dir,
                &[&format!("{}_{}.jsonl", file_timestamp, self.session_id)],
            ));
        }
        Ok(self.session_file.clone())
    }

    fn load_entries_internal(
        &mut self,
        entries: Vec<Value>,
        options: Option<&NewSessionOptions>,
    ) -> Result<(), String> {
        let header = entries.iter().find(|e| e["type"] == "session").cloned();

        if let Some(header) = header {
            self.session_id = header
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            self.file_entries = entries;
            if migrate_to_current_version(&mut self.file_entries) {
                self.rewrite_file()?;
            }
        } else {
            self.new_session(options)?;
            self.file_entries.extend(entries);
        }

        self.build_index();
        Ok(())
    }

    fn build_index(&mut self) {
        self.by_id.clear();
        self.labels.clear();
        self.leaf_id = None;
        for (i, entry) in self.file_entries.iter().enumerate() {
            if entry["type"] == "session" {
                continue;
            }
            if let Some(id) = entry.get("id").and_then(Value::as_str) {
                self.by_id.insert(id.to_string(), i);
                self.leaf_id = Some(id.to_string());
            }
            if entry["type"] == "label" {
                let target_id = entry["targetId"].as_str().unwrap_or_default().to_string();
                if js_truthy(entry.get("label")) {
                    let slot = LabelSlot {
                        target_id: target_id.clone(),
                        label: entry["label"].as_str().unwrap_or_default().to_string(),
                        timestamp: entry["timestamp"].as_str().unwrap_or_default().to_string(),
                    };
                    set_label_slot(&mut self.labels, slot);
                } else {
                    self.labels.retain(|s| s.target_id != target_id);
                }
            }
        }
    }

    fn rewrite_file(&mut self) -> Result<(), String> {
        if !self.persist {
            return Ok(());
        }
        let Some(path) = self.session_file.clone() else {
            return Ok(());
        };
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let mut file = file;
        write_entry_lines(&mut file, &self.file_entries)
    }

    pub fn is_persisted(&self) -> bool {
        self.persist
    }

    pub fn get_cwd(&self) -> &str {
        &self.cwd
    }

    pub fn get_session_dir(&self) -> &str {
        &self.session_dir
    }

    pub fn uses_default_session_dir(&self) -> bool {
        self.session_dir == get_default_session_dir_path(&self.cwd, &crate::config::get_agent_dir())
    }

    pub fn get_session_id(&self) -> &str {
        &self.session_id
    }

    pub fn get_session_file(&self) -> Option<&str> {
        self.session_file.as_deref()
    }

    /// A new session file is created only once the session contains a user
    /// or assistant message.
    fn has_conversation(&self) -> bool {
        self.file_entries.iter().any(|e| {
            e["type"] == "message"
                && matches!(e["message"]["role"].as_str(), Some("user") | Some("assistant"))
        })
    }

    fn persist_entry(&mut self, entry: &Value) -> Result<(), String> {
        if !self.persist {
            return Ok(());
        }
        let Some(path) = self.session_file.clone() else {
            return Ok(());
        };

        if !self.flushed {
            if !self.has_conversation() {
                return Ok(());
            }
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| e.to_string())?;
            let mut file = file;
            write_entry_lines(&mut file, &self.file_entries)?;
            self.flushed = true;
        } else {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&path)
                .map_err(|e| e.to_string())?;
            let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
            file.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
            file.write_all(b"\n").map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn append_entry(&mut self, entry: Value) -> Result<(), String> {
        self.file_entries.push(entry.clone());
        if let Some(id) = entry.get("id").and_then(Value::as_str) {
            self.by_id.insert(id.to_string(), self.file_entries.len() - 1);
            self.leaf_id = Some(id.to_string());
        }
        self.persist_entry(&entry)?;
        Ok(())
    }

    /// Append a message as child of current leaf, then advance leaf.
    pub fn append_message(&mut self, message: Value) -> Result<String, String> {
        let id = generate_id(&|c| self.by_id.contains_key(c));
        let mut m = Map::new();
        m.insert("type".to_string(), json!("message"));
        m.insert("id".to_string(), json!(id));
        m.insert(
            "parentId".to_string(),
            self.leaf_id.clone().map(Value::String).unwrap_or(Value::Null),
        );
        m.insert("timestamp".to_string(), json!(now_iso()));
        m.insert("message".to_string(), message);
        self.append_entry(Value::Object(m))?;
        Ok(id)
    }

    /// Append a thinking level change.
    pub fn append_thinking_level_change(&mut self, thinking_level: &str) -> Result<String, String> {
        let id = generate_id(&|c| self.by_id.contains_key(c));
        let mut m = Map::new();
        m.insert("type".to_string(), json!("thinking_level_change"));
        m.insert("id".to_string(), json!(id));
        m.insert(
            "parentId".to_string(),
            self.leaf_id.clone().map(Value::String).unwrap_or(Value::Null),
        );
        m.insert("timestamp".to_string(), json!(now_iso()));
        m.insert("thinkingLevel".to_string(), json!(thinking_level));
        self.append_entry(Value::Object(m))?;
        Ok(id)
    }

    /// Append a model change.
    pub fn append_model_change(&mut self, provider: &str, model_id: &str) -> Result<String, String> {
        let id = generate_id(&|c| self.by_id.contains_key(c));
        let mut m = Map::new();
        m.insert("type".to_string(), json!("model_change"));
        m.insert("id".to_string(), json!(id));
        m.insert(
            "parentId".to_string(),
            self.leaf_id.clone().map(Value::String).unwrap_or(Value::Null),
        );
        m.insert("timestamp".to_string(), json!(now_iso()));
        m.insert("provider".to_string(), json!(provider));
        m.insert("modelId".to_string(), json!(model_id));
        self.append_entry(Value::Object(m))?;
        Ok(id)
    }

    /// Append model-attributed usage that does not participate in LLM
    /// context. Returns the appended entry.
    pub fn append_usage(
        &mut self,
        kind: &str,
        provider: &str,
        model: &str,
        usage: Value,
        note: Option<&str>,
    ) -> Result<Value, String> {
        let id = generate_id(&|c| self.by_id.contains_key(c));
        let mut m = Map::new();
        m.insert("type".to_string(), json!("usage"));
        m.insert("id".to_string(), json!(id));
        m.insert(
            "parentId".to_string(),
            self.leaf_id.clone().map(Value::String).unwrap_or(Value::Null),
        );
        m.insert("timestamp".to_string(), json!(now_iso()));
        m.insert("kind".to_string(), json!(kind));
        m.insert("provider".to_string(), json!(provider));
        m.insert("model".to_string(), json!(model));
        m.insert("usage".to_string(), usage);
        if let Some(note) = note.filter(|n| !n.is_empty()) {
            m.insert("note".to_string(), json!(note));
        }
        let entry = Value::Object(m);
        self.append_entry(entry.clone())?;
        Ok(entry)
    }

    /// Append a compaction summary.
    #[allow(clippy::too_many_arguments)]
    pub fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: Option<&str>,
        tokens_before: &Value,
        details: Option<Value>,
        from_hook: Option<bool>,
        usage: Option<Value>,
    ) -> Result<String, String> {
        let timestamp = now_iso();
        let system_message = ai::get_current_system_message(&self.build_session_projection().messages);
        let id = generate_id(&|c| self.by_id.contains_key(c));
        let mut m = Map::new();
        m.insert("type".to_string(), json!("compaction"));
        m.insert("id".to_string(), json!(id));
        m.insert(
            "parentId".to_string(),
            self.leaf_id.clone().map(Value::String).unwrap_or(Value::Null),
        );
        m.insert("timestamp".to_string(), json!(timestamp));
        m.insert("summary".to_string(), json!(summary));
        m.insert(
            "firstKeptEntryId".to_string(),
            json!(first_kept_entry_id.unwrap_or(&id)),
        );
        m.insert("tokensBefore".to_string(), tokens_before.clone());
        if let Some(details) = details {
            m.insert("details".to_string(), details);
        }
        if let Some(usage) = usage {
            m.insert("usage".to_string(), usage);
        }
        if let Some(from_hook) = from_hook {
            m.insert("fromHook".to_string(), json!(from_hook));
        }
        if let Some(mut system_message) = system_message {
            // `{ ...systemMessage, timestamp: new Date(timestamp).getTime() }`
            if let Some(sm) = system_message.as_object_mut() {
                let millis = parse_date_millis(&timestamp).unwrap_or(0);
                sm.insert("timestamp".to_string(), json!(millis));
            }
            m.insert("systemMessage".to_string(), system_message);
        }
        self.append_entry(Value::Object(m))?;
        Ok(id)
    }

    /// Append a custom entry (for extensions).
    pub fn append_custom_entry(&mut self, custom_type: &str, data: Option<Value>) -> Result<String, String> {
        let id = generate_id(&|c| self.by_id.contains_key(c));
        let mut m = Map::new();
        m.insert("type".to_string(), json!("custom"));
        m.insert("id".to_string(), json!(id));
        m.insert(
            "parentId".to_string(),
            self.leaf_id.clone().map(Value::String).unwrap_or(Value::Null),
        );
        m.insert("timestamp".to_string(), json!(now_iso()));
        m.insert("customType".to_string(), json!(custom_type));
        if let Some(data) = data {
            m.insert("data".to_string(), data);
        }
        self.append_entry(Value::Object(m))?;
        Ok(id)
    }

    /// Append a session info entry (e.g., display name).
    pub fn append_session_info(&mut self, name: &str) -> Result<String, String> {
        // `name.replace(/[\r\n]+/g, " ").trim()`: each run of \r/\n becomes
        // one space; interior spacing is otherwise preserved.
        let sanitized_name = {
            let mut out = String::with_capacity(name.len());
            let mut in_run = false;
            for c in name.chars() {
                if c == '\r' || c == '\n' {
                    if !in_run {
                        out.push(' ');
                        in_run = true;
                    }
                } else {
                    out.push(c);
                    in_run = false;
                }
            }
            out.trim().to_string()
        };
        let id = generate_id(&|c| self.by_id.contains_key(c));
        let mut m = Map::new();
        m.insert("type".to_string(), json!("session_info"));
        m.insert("id".to_string(), json!(id));
        m.insert(
            "parentId".to_string(),
            self.leaf_id.clone().map(Value::String).unwrap_or(Value::Null),
        );
        m.insert("timestamp".to_string(), json!(now_iso()));
        m.insert("name".to_string(), json!(sanitized_name));
        self.append_entry(Value::Object(m))?;
        Ok(id)
    }

    /// The current session name from the latest session_info entry, if any.
    pub fn get_session_name(&self) -> Option<String> {
        for entry in self.file_entries.iter().rev() {
            if entry["type"] == "session_info" {
                return entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
            }
        }
        None
    }

    /// Append a custom message entry (for extensions) that participates in
    /// LLM context.
    pub fn append_custom_message_entry(
        &mut self,
        custom_type: &str,
        content: Value,
        display: bool,
        details: Option<Value>,
    ) -> Result<String, String> {
        let id = generate_id(&|c| self.by_id.contains_key(c));
        let mut m = Map::new();
        m.insert("type".to_string(), json!("custom_message"));
        m.insert("id".to_string(), json!(id));
        m.insert(
            "parentId".to_string(),
            self.leaf_id.clone().map(Value::String).unwrap_or(Value::Null),
        );
        m.insert("timestamp".to_string(), json!(now_iso()));
        m.insert("customType".to_string(), json!(custom_type));
        m.insert("content".to_string(), content);
        m.insert("display".to_string(), json!(display));
        if let Some(details) = details {
            m.insert("details".to_string(), details);
        }
        self.append_entry(Value::Object(m))?;
        Ok(id)
    }

    /// Append a branch-local edit to an earlier model-visible entry.
    pub fn append_context_edit(&mut self, target_id: &str, replacement: Option<Value>) -> Result<String, String> {
        if let Some(r) = &replacement {
            let content_ok = r
                .get("content")
                .is_some_and(|c| c.is_string() || c.is_array());
            if !r.is_object() || r.get("content").is_none() || !content_ok {
                return Err(
                    "Context edit replacement must be null or contain string/array content".to_string()
                );
            }
        }
        if !self.by_id.contains_key(target_id) {
            return Err(format!("Entry {} not found", target_id));
        }
        let on_branch = self
            .branch_path_indices(None)
            .iter()
            .any(|&i| self.file_entries[i].get("id").and_then(Value::as_str) == Some(target_id));
        if !on_branch {
            return Err(format!("Entry {} is not on the active branch", target_id));
        }
        let target = &self.file_entries[self.by_id[target_id]];
        let target_type = target["type"].as_str().unwrap_or("");
        let editable = target_type == "custom_message"
            || (target_type == "message"
                && matches!(
                    target["message"]["role"].as_str(),
                    Some("user") | Some("assistant") | Some("toolResult")
                ));
        if !editable {
            return Err(format!("Entry {} does not contribute editable model content", target_id));
        }
        let target_role = if target_type == "message" {
            target["message"]["role"].as_str().unwrap_or("").to_string()
        } else {
            "custom".to_string()
        };
        let normalized_replacement = match (&replacement, target_role.as_str()) {
            (Some(r), "assistant" | "toolResult")
                if r.get("content").is_some_and(|c| c.is_string()) =>
            {
                let text = r["content"].as_str().unwrap_or("");
                json!({ "content": [{ "type": "text", "text": text }] })
            }
            _ => replacement.unwrap_or(Value::Null),
        };
        let id = generate_id(&|c| self.by_id.contains_key(c));
        let mut m = Map::new();
        m.insert("type".to_string(), json!("context_edit"));
        m.insert("id".to_string(), json!(id));
        m.insert(
            "parentId".to_string(),
            self.leaf_id.clone().map(Value::String).unwrap_or(Value::Null),
        );
        m.insert("timestamp".to_string(), json!(now_iso()));
        m.insert("targetId".to_string(), json!(target_id));
        m.insert("replacement".to_string(), normalized_replacement);
        self.append_entry(Value::Object(m))?;
        Ok(id)
    }

    // =========================================================================
    // Tree traversal
    // =========================================================================

    pub fn get_leaf_id(&self) -> Option<&str> {
        self.leaf_id.as_deref()
    }

    pub fn get_leaf_entry(&self) -> Option<&Value> {
        self.leaf_id
            .as_deref()
            .and_then(|id| self.by_id.get(id))
            .map(|&i| &self.file_entries[i])
    }

    pub fn get_entry(&self, id: &str) -> Option<&Value> {
        self.by_id.get(id).map(|&i| &self.file_entries[i])
    }

    /// All direct children of an entry, in file order.
    pub fn get_children(&self, parent_id: &str) -> Vec<&Value> {
        self.file_entries
            .iter()
            .filter(|e| e["type"] != "session" && e["parentId"].as_str() == Some(parent_id))
            .collect()
    }

    /// The label for an entry, if any.
    pub fn get_label(&self, id: &str) -> Option<&str> {
        self.labels
            .iter()
            .find(|s| s.target_id == id)
            .map(|s| s.label.as_str())
    }

    /// Set or clear a label on an entry.
    pub fn append_label_change(&mut self, target_id: &str, label: Option<&str>) -> Result<String, String> {
        if !self.by_id.contains_key(target_id) {
            return Err(format!("Entry {} not found", target_id));
        }
        let id = generate_id(&|c| self.by_id.contains_key(c));
        let timestamp = now_iso();
        let mut m = Map::new();
        m.insert("type".to_string(), json!("label"));
        m.insert("id".to_string(), json!(id));
        m.insert(
            "parentId".to_string(),
            self.leaf_id.clone().map(Value::String).unwrap_or(Value::Null),
        );
        m.insert("timestamp".to_string(), json!(timestamp));
        m.insert("targetId".to_string(), json!(target_id));
        if let Some(label) = label {
            m.insert("label".to_string(), json!(label));
        }
        let entry = Value::Object(m);
        let label_truthy = label.is_some_and(|l| !l.is_empty());
        self.append_entry(entry)?;
        if label_truthy {
            let slot = LabelSlot {
                target_id: target_id.to_string(),
                label: label.unwrap_or_default().to_string(),
                timestamp,
            };
            set_label_slot(&mut self.labels, slot);
        } else {
            self.labels.retain(|s| s.target_id != target_id);
        }
        Ok(id)
    }

    fn branch_path_indices(&self, from_id: Option<&str>) -> Vec<usize> {
        let start = from_id.or(self.leaf_id.as_deref());
        let mut current = start.and_then(|id| self.by_id.get(id)).copied();
        let mut path = Vec::new();
        while let Some(i) = current {
            path.push(i);
            current = self.file_entries[i]
                .get("parentId")
                .and_then(Value::as_str)
                .filter(|pid| !pid.is_empty())
                .and_then(|pid| self.by_id.get(pid))
                .copied();
        }
        path.reverse();
        path
    }

    /// Walk from entry to root, returning all entries in path order.
    pub fn get_branch(&self, from_id: Option<&str>) -> Vec<Value> {
        self.branch_path_indices(from_id)
            .iter()
            .map(|&i| self.file_entries[i].clone())
            .collect()
    }

    /// The active, compaction-aware entry list for context/rendering.
    pub fn build_context_entries(&self) -> Vec<Value> {
        let leaf = self.leaf_spec();
        build_context_entries(&self.get_entries(), &leaf)
    }

    fn leaf_spec(&self) -> LeafSpec {
        match &self.leaf_id {
            None => LeafSpec::Root,
            Some(id) => LeafSpec::At(id.clone()),
        }
    }

    /// The session projection (what gets sent to the LLM).
    pub fn build_session_projection(&self) -> SessionProjection {
        let leaf = self.leaf_spec();
        build_session_projection(&self.get_entries(), &leaf)
    }

    pub fn build_session_context(&self) -> Value {
        let leaf = self.leaf_spec();
        build_session_context(&self.get_entries(), &leaf)
    }

    /// Session header.
    pub fn get_header(&self) -> Option<&Value> {
        self.file_entries.iter().find(|e| e["type"] == "session")
    }

    /// Number of session entries (excludes header).
    pub fn get_entry_count(&self) -> usize {
        self.by_id.len()
    }

    /// All session entries (excludes header).
    pub fn get_entries(&self) -> Vec<Value> {
        self.file_entries
            .iter()
            .filter(|e| e["type"] != "session")
            .cloned()
            .collect()
    }

    /// The session as a tree structure.
    pub fn get_tree(&self) -> Value {
        let entries = self.get_entries();
        let n = entries.len();
        let mut node_index: HashMap<String, usize> = HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            if let Some(id) = entry.get("id").and_then(Value::as_str) {
                node_index.insert(id.to_string(), i);
            }
        }

        // Link children by index (TS shares node references; indices avoid the
        // stale-clone problem of embedding nodes inside their parents early).
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut roots: Vec<usize> = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            let parent_id = entry.get("parentId").and_then(Value::as_str);
            let entry_id = entry.get("id").and_then(Value::as_str);
            // Roots: null/missing parentId, self-referencing parentId, or an
            // orphaned chain (parent not found).
            let parent_idx = parent_id.and_then(|pid| {
                if Some(pid) == entry_id {
                    None
                } else {
                    node_index.get(pid).copied()
                }
            });
            match parent_idx {
                Some(parent_idx) => children[parent_idx].push(i),
                None => roots.push(i),
            }
        }

        // Sort children by timestamp (oldest first), iteratively.
        let mut stack: Vec<usize> = roots.clone();
        while let Some(node_idx) = stack.pop() {
            children[node_idx].sort_by(|&a, &b| {
                let ta = js_date_get_time(entries[a].get("timestamp")).unwrap_or(0);
                let tb = js_date_get_time(entries[b].get("timestamp")).unwrap_or(0);
                ta.cmp(&tb)
            });
            stack.extend(children[node_idx].iter().copied());
        }

        // Materialize bottom-up with an explicit stack (no recursion, so deep
        // trees cannot overflow the call stack).
        enum Frame {
            Enter(usize),
            Exit(usize),
        }
        let mut built: Vec<Option<Value>> = vec![None; n];
        // 0 = unvisited, 1 = in progress, 2 = done. Guards against cycles in
        // malformed files (TS would build a cyclic object graph instead).
        let mut state = vec![0u8; n];
        let mut frames: Vec<Frame> = roots.iter().rev().map(|&r| Frame::Enter(r)).collect();
        while let Some(frame) = frames.pop() {
            match frame {
                Frame::Enter(i) => {
                    if state[i] != 0 {
                        continue;
                    }
                    state[i] = 1;
                    frames.push(Frame::Exit(i));
                    for &c in children[i].iter().rev() {
                        frames.push(Frame::Enter(c));
                    }
                }
                Frame::Exit(i) => {
                    state[i] = 2;
                    let node_children: Vec<Value> = children[i]
                        .iter()
                        .filter_map(|&c| built[c].clone())
                        .collect();
                    let mut node = Map::new();
                    node.insert("entry".to_string(), entries[i].clone());
                    node.insert("children".to_string(), Value::Array(node_children));
                    let entry_id = entries[i]["id"].as_str().unwrap_or_default();
                    if let Some(label) = self.get_label(entry_id) {
                        node.insert("label".to_string(), json!(label));
                        if let Some(slot) =
                            self.labels.iter().find(|s| s.target_id == entry_id)
                        {
                            node.insert("labelTimestamp".to_string(), json!(slot.timestamp));
                        }
                    }
                    built[i] = Some(Value::Object(node));
                }
            }
        }

        Value::Array(
            roots.into_iter()
                .map(|i| built[i].clone().expect("root built"))
                .collect(),
        )
    }
    // =========================================================================
    // Branching
    // =========================================================================

    /// Start a new branch from an earlier entry.
    pub fn branch(&mut self, branch_from_id: &str) -> Result<(), String> {
        if !self.by_id.contains_key(branch_from_id) {
            return Err(format!("Entry {} not found", branch_from_id));
        }
        self.leaf_id = Some(branch_from_id.to_string());
        Ok(())
    }

    /// Reset the leaf pointer to null (before any entries).
    pub fn reset_leaf(&mut self) {
        self.leaf_id = None;
    }

    /// Start a new branch with a summary of the abandoned path.
    pub fn branch_with_summary(
        &mut self,
        branch_from_id: Option<&str>,
        summary: &str,
        details: Option<Value>,
        from_hook: Option<bool>,
        usage: Option<Value>,
    ) -> Result<String, String> {
        if let Some(id) = branch_from_id {
            if !self.by_id.contains_key(id) {
                return Err(format!("Entry {} not found", id));
            }
        }
        let from_id = self.leaf_id.clone().unwrap_or_else(|| "root".to_string());
        self.leaf_id = branch_from_id.map(str::to_string);
        let id = generate_id(&|c| self.by_id.contains_key(c));
        let mut m = Map::new();
        m.insert("type".to_string(), json!("branch_summary"));
        m.insert("id".to_string(), json!(id));
        m.insert(
            "parentId".to_string(),
            branch_from_id
                .map(|s| Value::String(s.to_string()))
                .unwrap_or(Value::Null),
        );
        m.insert("timestamp".to_string(), json!(now_iso()));
        m.insert("fromId".to_string(), json!(from_id));
        m.insert("summary".to_string(), json!(summary));
        if let Some(details) = details {
            m.insert("details".to_string(), details);
        }
        if let Some(usage) = usage {
            m.insert("usage".to_string(), usage);
        }
        if let Some(from_hook) = from_hook {
            m.insert("fromHook".to_string(), json!(from_hook));
        }
        self.append_entry(Value::Object(m))?;
        Ok(id)
    }

    /// Create a new session file containing only the path from root to the
    /// specified leaf. Returns the new session file path when persisting.
    pub fn create_branched_session(&mut self, leaf_id: &str) -> Result<Option<String>, String> {
        let previous_session_file = self.session_file.clone();
        let path = self.get_branch(Some(leaf_id));
        if path.is_empty() {
            return Err(format!("Entry {} not found", leaf_id));
        }

        // Filter out LabelEntry from path - we'll recreate them from the
        // resolved map. Because labels are real tree entries, later entries
        // can be children of labels; removing labels requires re-chaining the
        // retained path to avoid orphaned subtrees.
        let mut path_without_labels: Vec<Value> = Vec::new();
        let mut replacement_by_label_id: HashMap<String, String> = HashMap::new();
        let mut pending_label_ids: Vec<String> = Vec::new();
        let mut path_parent_id: Option<String> = None;
        for entry in &path {
            if entry["type"] == "label" {
                pending_label_ids.push(entry["id"].as_str().unwrap_or_default().to_string());
                continue;
            }
            let entry_id = entry["id"].as_str().unwrap_or_default().to_string();
            for label_id in &pending_label_ids {
                replacement_by_label_id.insert(label_id.clone(), entry_id.clone());
            }
            pending_label_ids.clear();
            let mut cloned = entry.clone();
            if let Some(m) = cloned.as_object_mut() {
                m.insert(
                    "parentId".to_string(),
                    path_parent_id.clone().map(Value::String).unwrap_or(Value::Null),
                );
                if entry["type"] == "compaction" {
                    let first_kept = entry.get("firstKeptEntryId").cloned();
                    let mapped = first_kept
                        .as_ref()
                        .and_then(Value::as_str)
                        .and_then(|fk| {
                            if fk == entry_id {
                                None // self-reference keeps its own id
                            } else {
                                replacement_by_label_id.get(fk).cloned()
                            }
                        });
                    match mapped {
                        Some(new_id) => {
                            m.insert("firstKeptEntryId".to_string(), json!(new_id));
                        }
                        None => {
                            if let Some(fk) = first_kept {
                                m.insert("firstKeptEntryId".to_string(), fk);
                            }
                        }
                    }
                }
            }
            path_without_labels.push(cloned);
            path_parent_id = Some(entry_id);
        }

        let new_session_id = create_session_id()?;
        let timestamp = now_iso();
        let file_timestamp = timestamp.replace([':', '.'], "-");
        let new_session_file = node_join(
            &self.session_dir,
            &[&format!("{}_{}.jsonl", file_timestamp, new_session_id)],
        );

        let mut header = Map::new();
        header.insert("type".to_string(), json!("session"));
        header.insert("version".to_string(), json!(CURRENT_SESSION_VERSION));
        header.insert("id".to_string(), json!(new_session_id));
        header.insert("timestamp".to_string(), json!(timestamp));
        header.insert("cwd".to_string(), json!(self.cwd));
        if self.persist {
            if let Some(previous) = previous_session_file {
                header.insert("parentSession".to_string(), json!(previous));
            }
        }
        let header = Value::Object(header);

        // Collect labels for entries in the path.
        let mut path_entry_ids: HashSet<String> = path_without_labels
            .iter()
            .filter_map(|e| e["id"].as_str().map(str::to_string))
            .collect();
        let labels_to_write: Vec<LabelSlot> = self
            .labels
            .iter()
            .filter(|s| path_entry_ids.contains(&s.target_id))
            .cloned()
            .collect();

        if self.persist {
            let last_entry_id = path_without_labels
                .last()
                .and_then(|e| e["id"].as_str())
                .map(str::to_string);
            let mut parent_id = last_entry_id;
            let mut label_entries: Vec<Value> = Vec::new();
            for slot in &labels_to_write {
                let label_id = generate_id(&|c| path_entry_ids.contains(c));
                path_entry_ids.insert(label_id.clone());
                let mut m = Map::new();
                m.insert("type".to_string(), json!("label"));
                m.insert("id".to_string(), json!(label_id));
                m.insert(
                    "parentId".to_string(),
                    parent_id.clone().map(Value::String).unwrap_or(Value::Null),
                );
                m.insert("timestamp".to_string(), json!(slot.timestamp));
                m.insert("targetId".to_string(), json!(slot.target_id));
                m.insert("label".to_string(), json!(slot.label));
                parent_id = Some(label_id);
                label_entries.push(Value::Object(m));
            }

            self.file_entries = vec![header]
                .into_iter()
                .chain(path_without_labels)
                .chain(label_entries)
                .collect();
            self.session_id = new_session_id;
            self.session_file = Some(new_session_file.clone());
            self.build_index();

            // Use the same rule as _persist(): write now if the branched
            // path already has a conversation, otherwise let _persist()
            // create the file later.
            if self.has_conversation() {
                self.rewrite_file()?;
                self.flushed = true;
            } else {
                self.flushed = false;
            }

            return Ok(Some(new_session_file));
        }

        // In-memory mode: replace current session with the path + labels
        let mut label_ids: HashSet<String> = HashSet::new();
        let last_entry_id = path_without_labels
            .last()
            .and_then(|e| e["id"].as_str())
            .map(str::to_string);
        let mut parent_id = last_entry_id;
        let mut label_entries: Vec<Value> = Vec::new();
        for slot in &labels_to_write {
            let label_id = generate_id(&|c| path_entry_ids.contains(c) || label_ids.contains(c));
            label_ids.insert(label_id.clone());
            let mut m = Map::new();
            m.insert("type".to_string(), json!("label"));
            m.insert("id".to_string(), json!(label_id));
            m.insert(
                "parentId".to_string(),
                parent_id.clone().map(Value::String).unwrap_or(Value::Null),
            );
            m.insert("timestamp".to_string(), json!(slot.timestamp));
            m.insert("targetId".to_string(), json!(slot.target_id));
            m.insert("label".to_string(), json!(slot.label));
            parent_id = Some(label_id);
            label_entries.push(Value::Object(m));
        }
        self.file_entries = vec![header]
            .into_iter()
            .chain(path_without_labels)
            .chain(label_entries)
            .collect();
        self.session_id = new_session_id;
        self.build_index();
        Ok(None)
    }

    // =========================================================================
    // Static constructors
    // =========================================================================

    /// Create a new session.
    pub fn create(
        cwd: &str,
        session_dir: Option<&str>,
        options: Option<&NewSessionOptions>,
    ) -> Result<Self, String> {
        let dir = match session_dir.filter(|d| !d.is_empty()) {
            Some(dir) => normalize_path(dir, &PathInputOptions::default()),
            None => get_default_session_dir(cwd, None)?,
        };
        SessionManager::new(cwd, &dir, None, true, options, None)
    }

    /// Open a specific session file.
    pub fn open(
        path: &str,
        session_dir: Option<&str>,
        cwd_override: Option<&str>,
    ) -> Result<Self, String> {
        let resolved_path = resolve_from_process(path);
        let mut header: Option<Value> = None;
        let mut preloaded_file_entries: Option<Vec<Value>> = None;
        if cwd_override.is_none() && Path::new(&resolved_path).exists() {
            match read_session_header(&resolved_path) {
                Ok(h) => header = h,
                Err(message) => {
                    if !message.starts_with("Session header exceeds") {
                        return Err(message);
                    }
                    // The bounded scan is only a discovery optimization. A
                    // full load remains authoritative for legacy files with
                    // very large headers or prefixes.
                    let entries = load_entries_from_file(&resolved_path)?;
                    header = entries
                        .first()
                        .filter(|e| e["type"] == "session")
                        .cloned();
                    preloaded_file_entries = Some(entries);
                }
            }
        }
        let cwd = cwd_override
            .map(str::to_string)
            .or_else(|| {
                header
                    .as_ref()
                    .and_then(|h| get_session_header_cwd(h).map(str::to_string))
            })
            .unwrap_or_else(process_cwd);
        // If no sessionDir provided, derive from file's parent directory
        let dir = match session_dir.filter(|d| !d.is_empty()) {
            Some(dir) => normalize_path(dir, &PathInputOptions::default()),
            None => node_dirname(&resolved_path),
        };
        SessionManager::new(&cwd, &dir, Some(&resolved_path), true, None, preloaded_file_entries)
    }

    /// Continue the most recent session, or create new if none.
    pub fn continue_recent(cwd: &str, session_dir: Option<&str>) -> Result<Self, String> {
        let dir = match session_dir.filter(|d| !d.is_empty()) {
            Some(dir) => normalize_path(dir, &PathInputOptions::default()),
            None => get_default_session_dir(cwd, None)?,
        };
        let filter_cwd = session_dir.is_some()
            && dir != get_default_session_dir_path(cwd, &crate::config::get_agent_dir());
        let most_recent = find_most_recent_session(&dir, if filter_cwd { Some(cwd) } else { None });
        match most_recent {
            Some(path) => SessionManager::new(cwd, &dir, Some(&path), true, None, None),
            None => SessionManager::new(cwd, &dir, None, true, None, None),
        }
    }

    /// Create an in-memory session (no file persistence).
    pub fn in_memory(
        cwd: Option<&str>,
        options: Option<&NewSessionOptions>,
        entries: Option<Vec<Value>>,
    ) -> Result<Self, String> {
        let cwd = cwd.map(str::to_string).unwrap_or_else(process_cwd);
        SessionManager::new(&cwd, "", None, false, options, entries)
    }

    /// Fork a session from another project directory into the current
    /// project.
    pub fn fork_from(
        source_path: &str,
        target_cwd: &str,
        session_dir: Option<&str>,
        options: Option<&NewSessionOptions>,
    ) -> Result<Self, String> {
        let resolved_source_path = resolve_from_process(source_path);
        let resolved_target_cwd = resolve_from_process(target_cwd);
        let source_entries = load_entries_from_file(&resolved_source_path)?;
        if source_entries.is_empty() {
            return Err(format!(
                "Cannot fork: source session file is empty or invalid: {}",
                resolved_source_path
            ));
        }

        let _source_header = source_entries
            .iter()
            .find(|e| e["type"] == "session")
            .cloned()
            .ok_or_else(|| {
                format!(
                    "Cannot fork: source session has no header: {}",
                    resolved_source_path
                )
            })?;

        let dir = match session_dir.filter(|d| !d.is_empty()) {
            Some(dir) => normalize_path(dir, &PathInputOptions::default()),
            None => get_default_session_dir(&resolved_target_cwd, None)?,
        };
        if !Path::new(&dir).exists() {
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        }

        // Create new session file with new ID but forked content
        if let Some(id) = options.and_then(|o| o.id.as_deref()) {
            assert_valid_session_id(id)?;
        }
        let new_session_id = match options.and_then(|o| o.id.clone()) {
            Some(id) => id,
            None => create_session_id()?,
        };
        let timestamp = now_iso();
        let file_timestamp = timestamp.replace([':', '.'], "-");
        let new_session_file = node_join(&dir, &[&format!("{}_{}.jsonl", file_timestamp, new_session_id)]);

        // Write new header pointing to source as parent, with updated cwd
        let mut header = Map::new();
        header.insert("type".to_string(), json!("session"));
        header.insert("version".to_string(), json!(CURRENT_SESSION_VERSION));
        header.insert("id".to_string(), json!(new_session_id));
        header.insert("timestamp".to_string(), json!(timestamp));
        header.insert("cwd".to_string(), json!(resolved_target_cwd));
        header.insert("parentSession".to_string(), json!(resolved_source_path));

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&new_session_file)
            .map_err(|e| e.to_string())?;
        write_entry_lines(&mut file, &[Value::Object(header)])?;

        // Copy all non-header entries from source
        let non_header: Vec<&Value> = source_entries.iter().filter(|e| e["type"] != "session").collect();
        for entry in non_header {
            let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
            file.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
            file.write_all(b"\n").map_err(|e| e.to_string())?;
        }
        drop(file);

        SessionManager::new(&resolved_target_cwd, &dir, Some(&new_session_file), true, None, None)
    }

    /// Find an exact session ID without loading transcript bodies.
    pub fn find_by_id(cwd: &str, id: &str, session_dir: Option<&str>) -> Option<String> {
        let dir = match session_dir.filter(|d| !d.is_empty()) {
            Some(dir) => normalize_path(dir, &PathInputOptions::default()),
            None => get_default_session_dir(cwd, None).ok()?,
        };
        let filter_cwd = session_dir.is_some()
            && dir != get_default_session_dir_path(cwd, &crate::config::get_agent_dir());
        let resolved_cwd = resolve_from_process(cwd);

        for entry in fs::read_dir(&dir).ok()?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".jsonl") {
                continue;
            }
            let path = node_join(&dir, &[&name]);
            let Some(header) = read_session_header_for_discovery(&path) else {
                continue;
            };
            if header.get("id").and_then(Value::as_str) != Some(id) {
                continue;
            }
            if filter_cwd && !session_cwd_matches(get_session_header_cwd(&header), &resolved_cwd) {
                continue;
            }
            return Some(path);
        }
        None
    }

    /// List all sessions for a directory.
    pub fn list(cwd: &str, session_dir: Option<&str>) -> Vec<SessionInfo> {
        let dir = match session_dir.filter(|d| !d.is_empty()) {
            Some(dir) => normalize_path(dir, &PathInputOptions::default()),
            None => match get_default_session_dir(cwd, None) {
                Ok(dir) => dir,
                Err(_) => return Vec::new(),
            },
        };
        let filter_cwd = session_dir.is_some()
            && dir != get_default_session_dir_path(cwd, &crate::config::get_agent_dir());
        let resolved_cwd = resolve_from_process(cwd);
        let include_session =
            |session: &SessionInfo| !filter_cwd || session_cwd_matches(Some(&session.cwd), &resolved_cwd);
        let mut sessions: Vec<SessionInfo> = list_sessions_from_dir(&dir)
            .into_iter()
            .filter(include_session)
            .collect();
        sort_session_infos(&mut sessions);
        sessions
    }

    /// List all sessions across all project directories, or within one
    /// custom session directory.
    pub fn list_all(session_dir: Option<&str>) -> Vec<SessionInfo> {
        if let Some(custom_session_dir) = session_dir.filter(|d| !d.is_empty()) {
            let dir = normalize_path(custom_session_dir, &PathInputOptions::default());
            let mut sessions = list_sessions_from_dir(&dir);
            sort_session_infos(&mut sessions);
            return sessions;
        }

        let sessions_dir = crate::config::get_sessions_dir();
        if !Path::new(&sessions_dir).exists() {
            return Vec::new();
        }

        let Ok(entries) = fs::read_dir(&sessions_dir) else {
            return Vec::new();
        };
        let dirs: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir() || t.is_symlink()).unwrap_or(false))
            .map(|e| node_join(&sessions_dir, &[&e.file_name().to_string_lossy()]))
            .collect();

        let mut all_files: Vec<String> = Vec::new();
        for dir in &dirs {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".jsonl") {
                        all_files.push(node_join(dir, &[&name]));
                    }
                }
            }
        }

        let mut candidates: Vec<(String, Option<f64>)> = all_files
            .into_iter()
            .map(|path| {
                let mtime = stat_mtime_ms(&path);
                (path, mtime)
            })
            .collect();
        candidates.sort_by(|a, b| {
            let am = a.1.unwrap_or(f64::NEG_INFINITY);
            let bm = b.1.unwrap_or(f64::NEG_INFINITY);
            bm.partial_cmp(&am)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let an = node_basename(&a.0);
                    let bn = node_basename(&b.0);
                    bn.cmp(&an)
                })
        });

        let mut sessions: Vec<SessionInfo> = candidates
            .iter()
            .filter_map(|(path, mtime)| build_session_info(path, *mtime))
            .collect();
        sort_session_infos(&mut sessions);
        sessions
    }
}

fn node_basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(idx) => trimmed[idx + 1..].to_string(),
        None => trimmed.to_string(),
    }
}

fn set_label_slot(labels: &mut Vec<LabelSlot>, slot: LabelSlot) {
    if let Some(existing) = labels.iter_mut().find(|s| s.target_id == slot.target_id) {
        existing.label = slot.label;
        existing.timestamp = slot.timestamp;
    } else {
        labels.push(slot);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "pi-session-test-{}-{}-{}",
            tag,
            std::process::id(),
            ai::random_uuid()[..8].to_lowercase()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    fn user_msg(text: &str) -> Value {
        json!({
            "role": "user",
            "content": [{ "type": "text", "text": text }],
            "timestamp": "2026-01-01T00:00:01.000Z",
        })
    }

    fn assistant_msg(text: &str) -> Value {
        json!({
            "role": "assistant",
            "content": [{ "type": "text", "text": text }],
            "provider": "anthropic",
            "model": "claude-sonnet-4",
            "timestamp": "2026-01-01T00:00:02.000Z",
        })
    }

    #[test]
    fn in_memory_append_and_context() {
        let mut mgr = SessionManager::in_memory(Some("/tmp/proj"), None, None).unwrap();
        assert_eq!(mgr.get_entry_count(), 0);
        assert_eq!(mgr.get_leaf_id(), None);
        assert!(mgr.get_session_file().is_none());

        let e1 = mgr.append_message(user_msg("hello")).unwrap();
        let e2 = mgr.append_message(assistant_msg("hi")).unwrap();
        assert_eq!(mgr.get_entry_count(), 2);
        assert_eq!(mgr.get_leaf_id().unwrap(), e2);

        let entries = mgr.get_entries();
        assert_eq!(entries[0]["parentId"], Value::Null);
        assert_eq!(entries[1]["parentId"], json!(e1));

        let ctx = mgr.build_session_context();
        assert_eq!(ctx["messages"].as_array().unwrap().len(), 2);
        assert_eq!(ctx["thinkingLevel"], json!("off"));
        assert_eq!(ctx["model"]["provider"], json!("anthropic"));
        assert_eq!(ctx["model"]["modelId"], json!("claude-sonnet-4"));
    }

    #[test]
    fn persist_defers_file_until_conversation() {
        let dir = temp_dir("persist");
        let mut mgr = SessionManager::create("/tmp/proj", Some(&dir), None).unwrap();
        let file = mgr.get_session_file().unwrap().to_string();
        assert!(file.ends_with(".jsonl"));
        assert!(!Path::new(&file).exists(), "file must not exist before a conversation");

        // Non-conversation entries do not create the file.
        mgr.append_thinking_level_change("high").unwrap();
        mgr.append_session_info("my session").unwrap();
        assert!(!Path::new(&file).exists());

        // First user message flushes header + all pending entries.
        mgr.append_message(user_msg("hello")).unwrap();
        assert!(Path::new(&file).exists());
        let content = fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = content.trim_end_matches('\n').split('\n').collect();
        assert_eq!(lines.len(), 4, "header + 3 entries");

        let header: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(header["type"], json!("session"));
        assert_eq!(header["version"], json!(3));
        assert_eq!(header["cwd"], json!("/tmp/proj"));

        // Subsequent entries append line by line.
        mgr.append_message(assistant_msg("hi")).unwrap();
        let content = fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = content.trim_end_matches('\n').split('\n').collect();
        assert_eq!(lines.len(), 5);

        // Reopen: same entries, session name, and leaf.
        let reopened = SessionManager::open(&file, None, None).unwrap();
        assert_eq!(reopened.get_entry_count(), 4);
        assert_eq!(reopened.get_session_name(), Some("my session".to_string()));
        assert_eq!(reopened.get_leaf_id(), mgr.get_leaf_id());
        assert_eq!(reopened.get_session_id(), mgr.get_session_id());
    }

    #[test]
    fn open_migrates_v1_and_rewrites() {
        let dir = temp_dir("migrate");
        let file = node_join(&dir, &["v1.jsonl"]);
        let v1_lines = [
            json!({ "type": "session", "version": 1, "id": "01920000-0000-7000-8000-000000000001", "timestamp": "2026-01-01T00:00:00.000Z", "cwd": "/tmp/proj" }),
            json!({ "type": "message", "timestamp": "2026-01-01T00:00:01.000Z", "message": { "role": "hookMessage", "content": "hm", "timestamp": "2026-01-01T00:00:01.000Z" } }),
            json!({ "type": "message", "timestamp": "2026-01-01T00:00:02.000Z", "message": user_msg("hello") }),
        ];
        let content = v1_lines
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&file, content).unwrap();

        let mgr = SessionManager::open(&file, None, None).unwrap();
        // Migration rewrote the file to the current version.
        let rewritten = fs::read_to_string(&file).unwrap();
        let first: Value = serde_json::from_str(rewritten.lines().next().unwrap()).unwrap();
        assert_eq!(first["version"], json!(3));
        assert_eq!(mgr.get_entry_count(), 2);
        // hookMessage role was migrated to custom.
        let entries = mgr.get_entries();
        assert_eq!(entries[0]["message"]["role"], json!("custom"));
        // Ids were generated and chained.
        assert!(entries[0]["id"].as_str().unwrap().len() == 8);
        assert_eq!(entries[1]["parentId"], entries[0]["id"]);
        assert_eq!(mgr.get_leaf_id(), entries[1]["id"].as_str());
    }

    #[test]
    fn open_empty_file_initializes_header() {
        let dir = temp_dir("empty");
        let file = node_join(&dir, &["empty.jsonl"]);
        fs::write(&file, "").unwrap();
        let mgr = SessionManager::open(&file, None, None).unwrap();
        assert_eq!(mgr.get_entry_count(), 0);
        let content = fs::read_to_string(&file).unwrap();
        let header: Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(header["type"], json!("session"));
    }

    #[test]
    fn open_invalid_nonempty_file_errors() {
        let dir = temp_dir("invalid");
        let file = node_join(&dir, &["bad.jsonl"]);
        fs::write(&file, "this is not a session\n").unwrap();
        let err = match SessionManager::open(&file, None, None) {
            Err(e) => e,
            Ok(_) => panic!("expected open to fail"),
        };
        assert!(err.contains("not a valid"), "unexpected error: {}", err);
    }

    #[test]
    fn branch_changes_context_path() {
        let mut mgr = SessionManager::in_memory(Some("/tmp/proj"), None, None).unwrap();
        let e1 = mgr.append_message(user_msg("q1")).unwrap();
        let e2 = mgr.append_message(assistant_msg("a1")).unwrap();
        let _e3 = mgr.append_message(user_msg("q2")).unwrap();

        // Branch from e2: context follows the new leaf, excluding e3.
        mgr.branch(&e2).unwrap();
        assert_eq!(mgr.get_leaf_id().unwrap(), e2);
        let ctx = mgr.build_context_entries();
        assert_eq!(ctx.len(), 2);
        assert_eq!(ctx[0]["id"], json!(e1));
        assert_eq!(ctx[1]["id"], json!(e2));

        let e4 = mgr.append_message(user_msg("other q2")).unwrap();
        let ctx = mgr.build_context_entries();
        assert_eq!(ctx.len(), 3);
        assert_eq!(ctx[2]["id"], json!(e4));
        assert_eq!(mgr.get_children(&e2).len(), 2);

        // reset_leaf clears the pointer: the next append creates a new root.
        mgr.reset_leaf();
        assert_eq!(mgr.get_leaf_id(), None);
        let r1 = mgr.append_message(user_msg("fresh root")).unwrap();
        assert_eq!(mgr.get_entry(&r1).unwrap()["parentId"], Value::Null);
    }

    #[test]
    fn branch_with_summary_appends_entry() {
        let mut mgr = SessionManager::in_memory(Some("/tmp/proj"), None, None).unwrap();
        let e1 = mgr.append_message(user_msg("q1")).unwrap();
        let e2 = mgr.append_message(user_msg("q2")).unwrap();

        let id = mgr
            .branch_with_summary(Some(&e1), "abandoned q2 path", None, None, None)
            .unwrap();
        let entry = mgr.get_entry(&id).unwrap();
        assert_eq!(entry["type"], json!("branch_summary"));
        assert_eq!(entry["fromId"], json!(e2));
        assert_eq!(entry["parentId"], json!(e1));
        // _appendEntry advances the leaf to the new summary entry.
        assert_eq!(mgr.get_leaf_id().unwrap(), id);

        // The summary shows up in the model context, after the kept path.
        let ctx = mgr.build_session_context();
        let messages = ctx["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], json!("user"));
        assert_eq!(messages[1]["role"], json!("branchSummary"));
        assert_eq!(messages[1]["summary"], json!("abandoned q2 path"));
    }

    #[test]
    fn labels_set_clear_and_query() {
        let mut mgr = SessionManager::in_memory(Some("/tmp/proj"), None, None).unwrap();
        let e1 = mgr.append_message(user_msg("q1")).unwrap();
        let e2 = mgr.append_message(user_msg("q2")).unwrap();

        assert_eq!(mgr.get_label(&e1), None);
        mgr.append_label_change(&e1, Some("good")).unwrap();
        assert_eq!(mgr.get_label(&e1), Some("good"));

        // Update the label (entry inserted, slot updated in place).
        mgr.append_label_change(&e1, Some("great")).unwrap();
        assert_eq!(mgr.get_label(&e1), Some("great"));

        // Clearing removes the label.
        mgr.append_label_change(&e1, Some("")).unwrap();
        assert_eq!(mgr.get_label(&e1), None);

        // Unknown target errors.
        assert!(mgr.append_label_change("nope", Some("x")).is_err());

        // Branch from the leaf: e1, e2, then the three appended label entries.
        assert_eq!(mgr.get_branch(None).len(), 5);
        let _ = e2;
    }

    #[test]
    fn context_edit_validates_and_projects() {
        let mut mgr = SessionManager::in_memory(Some("/tmp/proj"), None, None).unwrap();
        let e1 = mgr.append_message(user_msg("q1")).unwrap();

        // Unknown target errors.
        assert!(mgr.append_context_edit("nope", Some(json!({"role": "user", "content": "x"}))).is_err());
        // Off-branch target errors after branching.
        let e2 = mgr.append_message(assistant_msg("a1")).unwrap();

        mgr.append_context_edit(&e1, Some(json!({"role": "user", "content": "edited"})))
            .unwrap();
        let ctx = mgr.build_session_context();
        let messages = ctx["messages"].as_array().unwrap();
        assert_eq!(messages[0]["content"], json!("edited"));

        // Null replacement removes the message from context.
        mgr.append_context_edit(&e1, None).unwrap();
        let ctx = mgr.build_session_context();
        assert_eq!(ctx["messages"].as_array().unwrap().len(), 1); // only assistant

        // String replacement on an assistant message becomes a text block.
        mgr.append_context_edit(&e2, Some(json!({"role": "assistant", "content": "new text"})))
            .unwrap();
        let ctx = mgr.build_session_context();
        let messages = ctx["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], json!("assistant"));
        assert_eq!(
            messages[0]["content"],
            json!([{ "type": "text", "text": "new text" }])
        );
    }

    #[test]
    fn append_compaction_captures_system_message() {
        let mut mgr = SessionManager::in_memory(Some("/tmp/proj"), None, None).unwrap();
        // A system message followed by a conversation.
        mgr.append_message(json!({
            "role": "system",
            "content": "You are pi.",
            "timestamp": "2026-01-01T00:00:00.000Z",
        }))
        .unwrap();
        let e1 = mgr.append_message(user_msg("q1")).unwrap();
        let e2 = mgr.append_message(assistant_msg("a1")).unwrap();

        let id = mgr
            .append_compaction("summarized", Some(&e1), &json!(1000), None, None, None)
            .unwrap();
        let entry = mgr.get_entry(&id).unwrap();
        assert_eq!(entry["type"], json!("compaction"));
        assert_eq!(entry["firstKeptEntryId"], json!(e1));
        assert_eq!(entry["parentId"], json!(e2));
        // The system message was captured with a numeric (rewritten) timestamp.
        let system_message = &entry["systemMessage"];
        assert_eq!(system_message["role"], json!("system"));
        assert_eq!(system_message["content"], json!("You are pi."));
        assert!(system_message["timestamp"].is_number());

        // Context: compaction first, then kept entries from firstKeptEntryId.
        let ctx = mgr.build_session_context();
        let messages = ctx["messages"].as_array().unwrap();
        // systemMessage + compactionSummary + e1 + e2 (system message before
        // firstKept is dropped).
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], json!("system"));
        assert_eq!(messages[1]["role"], json!("compactionSummary"));
        assert_eq!(messages[2]["role"], json!("user"));
    }

    #[test]
    fn usage_and_model_changes() {
        let mut mgr = SessionManager::in_memory(Some("/tmp/proj"), None, None).unwrap();
        mgr.append_message(user_msg("q1")).unwrap();
        let usage = json!({ "input": 100, "output": 50, "cacheRead": 10, "cacheWrite": 5, "cost": { "input": 0.001, "output": 0.002, "total": 0.003 } });
        mgr.append_usage("main", "anthropic", "claude-sonnet-4", usage, None)
            .unwrap();
        // Every usage entry is recorded, even a zeroed one.
        mgr.append_usage(
            "main",
            "anthropic",
            "claude-sonnet-4",
            json!({ "input": 0, "output": 0, "cost": { "input": 0, "output": 0, "total": 0 } }),
            None,
        )
        .unwrap();
        assert_eq!(mgr.get_entry_count(), 3);

        mgr.append_model_change("openai", "gpt-5").unwrap();
        mgr.append_thinking_level_change("high").unwrap();
        let ctx = mgr.build_session_context();
        assert_eq!(ctx["thinkingLevel"], json!("high"));
        assert_eq!(ctx["model"], json!({ "provider": "openai", "modelId": "gpt-5" }));
    }

    #[test]
    fn create_branched_session_rechains_labels() {
        let dir = temp_dir("branched");
        let mut mgr = SessionManager::create("/tmp/proj", Some(&dir), None).unwrap();
        let e1 = mgr.append_message(user_msg("q1")).unwrap();
        let e2 = mgr.append_message(assistant_msg("a1")).unwrap();
        let e3 = mgr.append_message(user_msg("q2")).unwrap();
        mgr.append_label_change(&e2, Some("checkpoint")).unwrap();
        let old_file = mgr.get_session_file().unwrap().to_string();
        let old_session_id = mgr.get_session_id().to_string();

        // Branch the session from the labeled entry.
        let new_file = mgr.create_branched_session(&e2).unwrap().unwrap();
        assert!(Path::new(&new_file).exists());
        assert_ne!(new_file, old_file);
        assert_eq!(mgr.get_session_file().unwrap(), new_file);

        let header: Value = serde_json::from_str(
            fs::read_to_string(&new_file).unwrap().lines().next().unwrap(),
        )
        .unwrap();
        assert_eq!(header["type"], json!("session"));
        assert_ne!(header["id"], json!(old_session_id));

        // Reopened branched session: path is e1, e2 (+ label entry), label intact.
        let branched = SessionManager::open(&new_file, None, None).unwrap();
        assert_eq!(branched.get_label(&e2), Some("checkpoint"));
        let ctx = branched.build_context_entries();
        assert!(ctx.iter().any(|e| e["id"] == json!(e1)));
        assert!(ctx.iter().any(|e| e["id"] == json!(e2)));
        assert!(!ctx.iter().any(|e| e["id"] == json!(e3)));
    }

    #[test]
    fn fork_from_copies_entries_to_new_file() {
        let dir1 = temp_dir("fork-src");
        let dir2 = temp_dir("fork-dst");
        let mut src = SessionManager::create("/tmp/src-proj", Some(&dir1), None).unwrap();
        let e1 = src.append_message(user_msg("q1")).unwrap();
        src.append_message(assistant_msg("a1")).unwrap();
        let src_file = src.get_session_file().unwrap().to_string();

        let forked = SessionManager::fork_from(&src_file, "/tmp/dst-proj", Some(&dir2), None).unwrap();
        let new_file = forked.get_session_file().unwrap();
        assert!(Path::new(new_file).exists());
        assert_ne!(new_file, src_file);
        assert_eq!(forked.get_cwd(), "/tmp/dst-proj");
        assert_eq!(forked.get_entry_count(), 2);

        let content = fs::read_to_string(new_file).unwrap();
        let mut lines = content.lines();
        let header: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(header["cwd"], json!("/tmp/dst-proj"));
        assert_eq!(header["parentSession"], json!(src_file));
        let first_entry: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(first_entry["id"], json!(e1));

        // Forking into an existing file fails (wx semantics).
        let again = SessionManager::fork_from(&src_file, "/tmp/dst-proj", Some(&dir2), None);
        // A new session id means a new file name, so this succeeds; instead
        // verify the second fork produced a different file.
        let forked2 = again.unwrap();
        assert_ne!(forked2.get_session_file().unwrap(), new_file);
    }

    #[test]
    fn find_by_id_and_continue_recent() {
        let dir = temp_dir("find");
        let mut mgr = SessionManager::create("/tmp/proj", Some(&dir), None).unwrap();
        mgr.append_message(user_msg("q1")).unwrap();
        let file = mgr.get_session_file().unwrap().to_string();
        let id = mgr.get_session_id().to_string();

        let found = SessionManager::find_by_id("/tmp/proj", &id, Some(&dir)).unwrap();
        assert_eq!(found, file);

        // continueRecent opens the most recent session.
        let continued = SessionManager::continue_recent("/tmp/proj", Some(&dir)).unwrap();
        assert_eq!(continued.get_session_file().unwrap(), file);
        assert_eq!(continued.get_entry_count(), 1);

        // A different (empty) dir creates a new session.
        let dir2 = temp_dir("find2");
        let fresh = SessionManager::continue_recent("/tmp/proj", Some(&dir2)).unwrap();
        assert_ne!(fresh.get_session_file().unwrap(), file);
    }

    #[test]
    fn tree_structure_and_projection() {
        let mut mgr = SessionManager::in_memory(Some("/tmp/proj"), None, None).unwrap();
        let e1 = mgr.append_message(user_msg("q1")).unwrap();
        let e2 = mgr.append_message(assistant_msg("a1")).unwrap();
        let _e3 = mgr.append_message(user_msg("q2")).unwrap();
        mgr.branch(&e2).unwrap();
        let e4 = mgr.append_message(user_msg("other q2")).unwrap();

        let tree = mgr.get_tree();
        let roots = tree.as_array().unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0]["entry"]["id"], json!(e1));
        let children = roots[0]["children"].as_array().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["entry"]["id"], json!(e2));
        let grandchildren = children[0]["children"].as_array().unwrap();
        assert_eq!(grandchildren.len(), 2);

        let projection = mgr.build_session_projection();
        assert_eq!(projection.entries.len(), 3);
        assert_eq!(projection.entries[2]["sourceEntry"]["id"], json!(e4));
    }

    #[test]
    fn session_info_sanitization_and_name() {
        let mut mgr = SessionManager::in_memory(Some("/tmp/proj"), None, None).unwrap();
        assert_eq!(mgr.get_session_name(), None);
        mgr.append_session_info("line1\r\nline2  padded  ").unwrap();
        assert_eq!(mgr.get_session_name(), Some("line1 line2  padded".to_string()));
        // Clearing the name with an empty string.
        mgr.append_session_info("").unwrap();
        assert_eq!(mgr.get_session_name(), None);
    }

    #[test]
    fn custom_entries_and_messages() {
        let mut mgr = SessionManager::in_memory(Some("/tmp/proj"), None, None).unwrap();
        let id = mgr
            .append_custom_entry("mystate", Some(json!({ "k": 1 })))
            .unwrap();
        let entry = mgr.get_entry(&id).unwrap();
        assert_eq!(entry["type"], json!("custom"));
        assert_eq!(entry["customType"], json!("mystate"));
        assert_eq!(entry["data"], json!({ "k": 1 }));
        // Custom entries stay on the branch but contribute no messages.
        assert_eq!(mgr.build_context_entries().len(), 1);
        assert_eq!(mgr.build_session_context()["messages"].as_array().unwrap().len(), 0);

        let mid = mgr
            .append_custom_message_entry(
                "hook",
                json!([{ "type": "text", "text": "note" }]),
                true,
                None,
            )
            .unwrap();
        let entry = mgr.get_entry(&mid).unwrap();
        assert_eq!(entry["type"], json!("custom_message"));
        let ctx = mgr.build_session_context();
        let messages = ctx["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], json!("custom"));
        assert_eq!(messages[0]["customType"], json!("hook"));
        assert_eq!(messages[0]["display"], json!(true));
    }

    #[test]
    fn generate_id_avoids_collisions() {
        let taken: HashSet<String> = ["deadbeef".to_string()].into_iter().collect();
        let id = generate_id(&|c| taken.contains(c));
        assert_ne!(id, "deadbeef");
        assert!(id.len() == 8 || id.len() == 36);
    }

    #[test]
    fn default_session_dir_path_encoding() {
        let path = get_default_session_dir_path("/home/user/my proj", "/home/user/.pi/agent");
        assert_eq!(path, "/home/user/.pi/agent/sessions/--home-user-my proj--");
        let path = get_default_session_dir_path("/a/b:c", "/agent");
        assert_eq!(path, "/agent/sessions/--a-b-c--");
    }
}
