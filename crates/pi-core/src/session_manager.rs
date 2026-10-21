//! Session management: append-only conversation trees stored as JSONL files.
//! Port of `src/core/session-manager.ts`.
//!
//! Session entries are raw tagged JSONL objects (`type`, `id`, `parentId`,
//! plus per-type fields), carried as `serde_json::Value` throughout so that
//! key order and unknown fields round-trip byte-exactly through the files.
//!
//! Injection points (divergence from TS, which uses ambient `uuidv7()`,
//! `randomUUID()` and `new Date()`): every constructor takes a
//! [`Generators`] handle, and `agent_dir`/`cwd` are explicit arguments
//! wherever TS fell back to ambient config (`getAgentDir()`, `process.cwd()`).
//! The async `list`/`listAll`/`buildSessionInfo` pipeline is ported
//! synchronously; `AbortSignal` support is not replicated.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::messages::{
    create_branch_summary_message, create_compaction_summary_message, create_custom_message,
    parse_date_millis, AgentMessage, CustomContent,
};
use crate::paths::{normalize_path, resolve_path, PathInputOptions};

/// Mirrors TS `CURRENT_SESSION_VERSION`.
pub const CURRENT_SESSION_VERSION: i64 = 3;

/// Bounded synchronous header discovery limit (TS `MAX_SESSION_HEADER_SCAN_BYTES`).
pub const MAX_SESSION_HEADER_SCAN_BYTES: usize = 1024 * 1024;

/// Stand-in for TS `process.cwd()` (ambient cwd is not readable in this port).
const AMBIENT_CWD: &str = "/";

/// TS `APP_NAME` (used in the "not a valid pi session" error).
const APP_NAME: &str = "pi";

// ============================================================================
// Ordered map (JS Map semantics)
// ============================================================================

/// Insertion-ordered map mirroring JS `Map` iteration order: re-inserting an
/// existing key keeps its original position, `remove` shifts later entries
/// down (unlike `serde_json`'s `swap_remove`).
#[derive(Debug, Clone)]
pub struct OrderedMap<V> {
    entries: Vec<(String, V)>,
    index: HashMap<String, usize>,
}

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        OrderedMap { entries: Vec::new(), index: HashMap::new() }
    }
}

impl<V> OrderedMap<V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.index.get(key).map(|&i| &self.entries[i].1)
    }

    pub fn has(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    pub fn set(&mut self, key: &str, value: V) {
        match self.index.get(key) {
            Some(&i) => self.entries[i].1 = value,
            None => {
                self.index.insert(key.to_string(), self.entries.len());
                self.entries.push((key.to_string(), value));
            }
        }
    }

    pub fn remove(&mut self, key: &str) {
        if let Some(i) = self.index.remove(key) {
            self.entries.remove(i);
            for idx in self.index.values_mut() {
                if *idx > i {
                    *idx -= 1;
                }
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
    }
}

/// Entry index type used by the projection functions (TS `Map<string, SessionEntry>`).
pub type EntryIndex = OrderedMap<Value>;

// ============================================================================
// Injected id/time generators
// ============================================================================

pub type UuidGen = Arc<dyn Fn() -> String + Send + Sync>;
pub type NowMsGen = Arc<dyn Fn() -> i64 + Send + Sync>;

fn os_random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(0x9E37_79B9_7F4A_7C15);
    hasher.finish()
}

fn format_uuid(bytes: &[u8; 16]) -> String {
    let hex = |b: u8| format!("{:02x}", b);
    format!(
        "{}-{}-{}-{}-{}",
        hex(bytes[0]) + &hex(bytes[1]) + &hex(bytes[2]) + &hex(bytes[3]),
        hex(bytes[4]) + &hex(bytes[5]),
        hex(bytes[6]) + &hex(bytes[7]),
        hex(bytes[8]) + &hex(bytes[9]),
        hex(bytes[10])
            + &hex(bytes[11])
            + &hex(bytes[12])
            + &hex(bytes[13])
            + &hex(bytes[14])
            + &hex(bytes[15])
    )
}

fn random_uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&os_random_u64().to_be_bytes());
    bytes[8..].copy_from_slice(&os_random_u64().to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format_uuid(&bytes)
}

fn uuid_v7_from_ms(ms: i64) -> String {
    let a = os_random_u64();
    let b = os_random_u64();
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&(ms as u64).to_be_bytes()[2..]);
    bytes[6] = ((a >> 8) as u8 & 0x0f) | 0x70;
    bytes[7] = (a >> 16) as u8;
    bytes[8] = (b as u8 & 0x3f) | 0x80;
    bytes[9..].copy_from_slice(&(b >> 8).to_be_bytes()[..7]);
    format_uuid(&bytes)
}

fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Injected replacements for TS's ambient `uuidv7()`, `randomUUID()` and
/// `new Date()`. Default generators are real (OS randomness / system clock).
#[derive(Clone)]
pub struct Generators {
    uuid_v7: UuidGen,
    uuid_v4: UuidGen,
    now_ms: NowMsGen,
}

impl Default for Generators {
    fn default() -> Self {
        Generators {
            uuid_v7: Arc::new(|| uuid_v7_from_ms(system_now_ms())),
            uuid_v4: Arc::new(random_uuid_v4),
            now_ms: Arc::new(system_now_ms),
        }
    }
}

impl Generators {
    pub fn new() -> Self {
        Self::default()
    }

    /// Harness/tests constructor with fully-injected generators (the
    /// default fields are real OS randomness / system clock).
    pub fn with_gens(uuid_v7: UuidGen, uuid_v4: UuidGen, now_ms: NowMsGen) -> Self {
        Generators { uuid_v7, uuid_v4, now_ms }
    }

    fn now_iso(&self) -> String {
        iso_string((self.now_ms)())
    }
}

// ============================================================================
// JS helpers
// ============================================================================

fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

pub(crate) fn js_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(_)) => true,
    }
}

/// `new Date(ms).toISOString()`.
pub fn iso_string(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y,
        m,
        d,
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60,
        millis
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Node `path.join` + normalize (unix flavor).
fn node_join(a: &str, b: &str) -> String {
    let joined = if a.is_empty() {
        b.to_string()
    } else if a.ends_with('/') {
        format!("{}{}", a, b)
    } else {
        format!("{}/{}", a, b)
    };
    normalize_posix(&joined)
}

fn normalize_posix(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if let Some(last) = out.last() {
                    if *last != ".." {
                        out.pop();
                    }
                } else if !absolute {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    format!("{}{}", if absolute { "/" } else { "" }, out.join("/"))
}

/// TS `resolvePath(input)` (base `process.cwd()` → `AMBIENT_CWD`).
pub fn resolve_ambient(input: &str) -> String {
    resolve_path(input, AMBIENT_CWD, &PathInputOptions::default())
}

fn path_opts() -> PathInputOptions {
    PathInputOptions::default()
}

// ============================================================================
// Entry accessors
// ============================================================================

/// A session entry (or header): tagged JSONL object. `type` is `"session"`
/// for headers, otherwise one of the entry types.
pub type FileEntry = Value;
/// A non-header session entry.
pub type SessionEntry = Value;

fn entry_type(v: &Value) -> &str {
    v.get("type").and_then(Value::as_str).unwrap_or("")
}

fn entry_id(v: &Value) -> &str {
    v.get("id").and_then(Value::as_str).unwrap_or("")
}

fn entry_parent_id(v: &Value) -> Option<&str> {
    v.get("parentId").and_then(Value::as_str)
}

/// Truthy parentId (null / missing / "" all stop the walk in TS).
fn truthy_parent_id(v: &Value) -> Option<&str> {
    entry_parent_id(v).filter(|p| !p.is_empty())
}

/// TS `leafId?: string | null`: unset (undefined → last entry), root (null →
/// empty path), or an explicit entry id.
#[derive(Debug, Clone, Copy)]
pub enum LeafId<'a> {
    Unset,
    Root,
    Id(&'a str),
}

impl<'a> From<Option<&'a str>> for LeafId<'a> {
    fn from(value: Option<&'a str>) -> Self {
        match value {
            Some(id) => LeafId::Id(id),
            None => LeafId::Root,
        }
    }
}

// ============================================================================
// Session id validation
// ============================================================================

/// `assertValidSessionId` — byte-exact error message.
pub fn assert_valid_session_id(id: &str) -> Result<(), String> {
    let bytes = id.as_bytes();
    let valid = !bytes.is_empty()
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && (bytes.len() == 1
            || bytes[1..bytes.len() - 1]
                .iter()
                .all(|&b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-'));
    if valid {
        Ok(())
    } else {
        Err("Session id must be non-empty, contain only alphanumeric characters, '-', '_', and '.', and start and end with an alphanumeric character".to_string())
    }
}

fn create_session_id(gen: &Generators) -> String {
    (gen.uuid_v7)()
}

/// Generate a unique short ID (8 hex chars, collision-checked).
fn generate_id(has: &dyn Fn(&str) -> bool, uuid_v4: &UuidGen) -> String {
    for _ in 0..100 {
        let id = (uuid_v4)()[..8].to_string();
        if !has(&id) {
            return id;
        }
    }
    (uuid_v4)()
}

// ============================================================================
// Migration
// ============================================================================

fn migrate_v1_to_v2(entries: &mut [Value], uuid_v4: &UuidGen) {
    let mut prev_id: Option<String> = None;
    for i in 0..entries.len() {
        if entry_type(&entries[i]) == "session" {
            if let Some(o) = entries[i].as_object_mut() {
                o.insert("version".to_string(), json!(2));
            }
            continue;
        }
        let id = generate_id(&|_| false, uuid_v4);
        if let Some(o) = entries[i].as_object_mut() {
            o.insert("id".to_string(), json!(id.clone()));
            o.insert(
                "parentId".to_string(),
                prev_id.clone().map(Value::String).unwrap_or(Value::Null),
            );
        }
        prev_id = Some(id);

        if entry_type(&entries[i]) == "compaction" {
            let target_id: Option<String> = entries[i]
                .get("firstKeptEntryIndex")
                .and_then(Value::as_f64)
                .and_then(|idx| {
                    entries
                        .get(idx as usize)
                        .filter(|t| entry_type(t) != "session")
                        .and_then(|t| t.get("id").and_then(Value::as_str).map(str::to_string))
                });
            if let Some(o) = entries[i].as_object_mut() {
                if o.get("firstKeptEntryIndex").and_then(Value::as_f64).is_some() {
                    if let Some(tid) = target_id {
                        o.insert("firstKeptEntryId".to_string(), json!(tid));
                    }
                    // `undefined` values are omitted by JSON.stringify.
                    o.shift_remove("firstKeptEntryIndex");
                }
            }
        }
    }
}

fn migrate_v2_to_v3(entries: &mut [Value]) {
    for entry in entries.iter_mut() {
        if entry_type(entry) == "session" {
            if let Some(o) = entry.as_object_mut() {
                o.insert("version".to_string(), json!(3));
            }
            continue;
        }
        if entry_type(entry) == "message" {
            if let Some(message) = entry.get_mut("message").and_then(Value::as_object_mut) {
                if message.get("role").and_then(Value::as_str) == Some("hookMessage") {
                    message.insert("role".to_string(), json!("custom"));
                }
            }
        }
    }
}

fn migrate_to_current_version(entries: &mut [Value], uuid_v4: &UuidGen) -> bool {
    // TS: `const version = header?.version ?? 1;` followed by relational
    // compares. Missing header / absent or null version → 1. Numbers pass
    // through. Strings coerce like JS Number(s): numeric strings parse,
    // everything else → NaN. Other JSON types (objects, arrays, bools as
    // handled here) → NaN. NaN fails every comparison on both sides, so no
    // migration step runs and true is returned (matching TS).
    let header = entries.iter().find(|e| entry_type(e) == "session");
    let version: f64 = match header.and_then(|h| h.get("version")) {
        None | Some(Value::Null) => 1.0,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => s.trim().parse::<f64>().unwrap_or(f64::NAN),
        Some(_) => f64::NAN,
    };
    if version >= CURRENT_SESSION_VERSION as f64 {
        return false;
    }
    if version < 2.0 {
        migrate_v1_to_v2(entries, uuid_v4);
    }
    if version < 3.0 {
        migrate_v2_to_v3(entries);
    }
    true
}

/// Exported for testing. Mutates entries in place.
pub fn migrate_session_entries(entries: &mut [Value]) {
    let uuid_v4: UuidGen = Arc::new(random_uuid_v4);
    migrate_to_current_version(entries, &uuid_v4);
}

// ============================================================================
// Parsing
// ============================================================================

/// `parseSessionEntries` — exported for compaction tests.
pub fn parse_session_entries(content: &str) -> Vec<Value> {
    js_trim(content)
        .split('\n')
        .filter_map(parse_session_entry_line)
        .collect()
}

fn parse_session_entry_line(line: &str) -> Option<Value> {
    if js_trim(line).is_empty() {
        return None;
    }
    serde_json::from_str(line).ok()
}

pub fn get_latest_compaction_entry(entries: &[Value]) -> Option<&Value> {
    entries
        .iter()
        .rev()
        .find(|e| entry_type(e) == "compaction")
}

// ============================================================================
// Projection
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    /// `undefined` on the TS side serializes to omission; absent fields
    /// (e.g. an assistant message without provider/model) stay `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

/// Tree node for `get_tree()` — defensive copy of the session structure.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionTreeNode {
    pub entry: SessionEntry,
    pub children: Vec<SessionTreeNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_timestamp: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectedSessionEntry {
    /// Raw append-only entry that owns this projected contribution.
    pub source_entry: SessionEntry,
    /// Model-visible messages after context edits.
    pub messages: Vec<AgentMessage>,
}

#[derive(Debug, Clone)]
pub struct SessionProjection {
    pub entries: Vec<ProjectedSessionEntry>,
    pub messages: Vec<AgentMessage>,
    pub thinking_level: String,
    pub model: Option<ModelRef>,
}

#[derive(Debug, Clone)]
pub struct SessionContext {
    pub messages: Vec<AgentMessage>,
    pub thinking_level: String,
    pub model: Option<ModelRef>,
}

fn build_entry_index(entries: &[Value]) -> EntryIndex {
    let mut index = EntryIndex::default();
    for entry in entries {
        if let Some(id) = entry.get("id").and_then(Value::as_str) {
            index.set(id, entry.clone());
        }
    }
    index
}

fn build_session_path(
    entries: &[Value],
    leaf_id: LeafId<'_>,
    by_id: Option<&EntryIndex>,
) -> Vec<Value> {
    let owned;
    let index: &EntryIndex = match by_id {
        Some(map) => map,
        None => {
            owned = build_entry_index(entries);
            &owned
        }
    };
    if matches!(leaf_id, LeafId::Root) {
        return Vec::new();
    }
    let mut current: Option<Value> = match leaf_id {
        LeafId::Id(id) if !id.is_empty() => index.get(id).or_else(|| entries.last()).cloned(),
        _ => entries.last().cloned(),
    };
    let mut path = Vec::new();
    while let Some(entry) = current {
        let parent = truthy_parent_id(&entry).and_then(|p| index.get(p).cloned());
        path.push(entry);
        current = parent;
    }
    path.reverse();
    path
}

fn get_session_context_settings(path: &[Value]) -> (String, Option<ModelRef>) {
    let mut thinking_level = "off".to_string();
    let mut model: Option<ModelRef> = None;
    for entry in path {
        match entry_type(entry) {
            "thinking_level_change" => {
                thinking_level = entry
                    .get("thinkingLevel")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
            }
            "model_change" => {
                model = Some(ModelRef {
                    provider: entry.get("provider").and_then(Value::as_str).map(str::to_string),
                    model_id: entry.get("modelId").and_then(Value::as_str).map(str::to_string),
                });
            }
            "message" => {
                let message = entry.get("message");
                if message.and_then(|m| m.get("role")).and_then(Value::as_str) == Some("assistant") {
                    // TS reads `message.provider` / `message.model` directly;
                    // missing fields are `undefined` and drop out of the
                    // serialized shape (leaving `model: {}`).
                    model = Some(ModelRef {
                        provider: message
                            .and_then(|m| m.get("provider"))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        model_id: message
                            .and_then(|m| m.get("model"))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    });
                }
            }
            _ => {}
        }
    }
    (thinking_level, model)
}

/// `sessionEntryToContextMessages` — project one entry into LLM messages.
pub fn session_entry_to_context_messages(entry: &Value) -> Vec<AgentMessage> {
    match entry_type(entry) {
        "message" => {
            let mut message = entry.get("message").cloned().unwrap_or(Value::Null);
            let role = message.get("role").and_then(Value::as_str).unwrap_or("").to_string();
            let content_null = message.get("content").is_none_or(Value::is_null);
            if let Some(obj) = message.as_object_mut() {
                if role == "system" && content_null {
                    obj.insert("content".to_string(), json!(""));
                } else if matches!(role.as_str(), "user" | "assistant" | "toolResult") && content_null {
                    obj.insert("content".to_string(), json!([]));
                }
            }
            vec![AgentMessage::Base(message)]
        }
        "custom_message" => {
            let content_value = entry.get("content").filter(|c| !c.is_null()).cloned();
            let content: CustomContent = content_value
                .and_then(|c| serde_json::from_value(c).ok())
                .unwrap_or(CustomContent::Blocks(Vec::new()));
            vec![AgentMessage::Custom(create_custom_message(
                entry.get("customType").and_then(Value::as_str).unwrap_or_default(),
                content,
                entry.get("display").and_then(Value::as_bool),
                entry.get("details").cloned(),
                entry.get("timestamp").and_then(Value::as_str).unwrap_or_default(),
            ))]
        }
        "branch_summary" => {
            let summary = entry.get("summary");
            if js_truthy(summary) {
                vec![AgentMessage::BranchSummary(create_branch_summary_message(
                    summary.and_then(Value::as_str).unwrap_or_default(),
                    entry.get("fromId").and_then(Value::as_str),
                    entry.get("timestamp").and_then(Value::as_str).unwrap_or_default(),
                ))]
            } else {
                Vec::new()
            }
        }
        "compaction" => {
            let summary = create_compaction_summary_message(
                entry.get("summary").and_then(Value::as_str).unwrap_or_default(),
                entry.get("tokensBefore").and_then(Value::as_u64).unwrap_or(0),
                entry.get("timestamp").and_then(Value::as_str).unwrap_or_default(),
            );
            match entry.get("systemMessage").filter(|s| js_truthy(Some(s))) {
                Some(system_message) => {
                    vec![AgentMessage::Base(system_message.clone()), AgentMessage::CompactionSummary(summary)]
                }
                None => vec![AgentMessage::CompactionSummary(summary)],
            }
        }
        _ => Vec::new(),
    }
}

/// `buildContextEntries` — active, compaction-aware entry list.
pub fn build_context_entries(
    entries: &[Value],
    leaf_id: LeafId<'_>,
    by_id: Option<&EntryIndex>,
) -> Vec<Value> {
    let path = build_session_path(entries, leaf_id, by_id);
    let compaction = path.iter().rev().find(|e| entry_type(e) == "compaction");
    let Some(compaction) = compaction else {
        return path;
    };
    let compaction_id = entry_id(compaction).to_string();
    let compaction_idx = match path.iter().position(|e| entry_id(e) == compaction_id) {
        Some(i) => i,
        None => return path,
    };
    let first_kept_id = compaction
        .get("firstKeptEntryId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut context_entries = vec![compaction.clone()];
    let mut found_first_kept = false;
    for entry in &path[..compaction_idx] {
        if entry_id(entry) == first_kept_id {
            found_first_kept = true;
        }
        let is_system_message = entry_type(entry) == "message"
            && entry
                .get("message")
                .and_then(|m| m.get("role"))
                .and_then(Value::as_str)
                == Some("system");
        if found_first_kept && !is_system_message {
            context_entries.push(entry.clone());
        }
    }
    context_entries.extend(path[compaction_idx + 1..].iter().cloned());
    context_entries
}

fn project_context_entry(entry: &Value, edit: Option<&Value>) -> Vec<AgentMessage> {
    let messages = session_entry_to_context_messages(entry);
    let Some(edit) = edit else {
        return messages;
    };
    let replacement = edit.get("replacement");
    if replacement.is_none() || replacement == Some(&Value::Null) {
        return Vec::new();
    }
    let content_value = replacement.and_then(|r| r.get("content")).cloned();
    messages
        .into_iter()
        .map(|m| {
            let mut value = serde_json::to_value(&m).expect("message serializes");
            let role = value.get("role").and_then(Value::as_str).unwrap_or("").to_string();
            if !matches!(role.as_str(), "user" | "assistant" | "toolResult" | "custom") {
                return m;
            }
            let is_string = content_value.as_ref().map(Value::is_string).unwrap_or(false);
            let content = if (role == "assistant" || role == "toolResult") && is_string {
                json!([{ "type": "text", "text": content_value.as_ref().and_then(Value::as_str).unwrap_or("") }])
            } else {
                content_value.clone().unwrap_or(Value::Null)
            };
            if let Some(obj) = value.as_object_mut() {
                obj.insert("content".to_string(), content);
            }
            serde_json::from_value(value).expect("message round-trips")
        })
        .collect()
}

/// `buildSessionProjection` — provenance-preserving, compaction-aware context.
pub fn build_session_projection(
    entries: &[Value],
    leaf_id: LeafId<'_>,
    by_id: Option<&EntryIndex>,
) -> SessionProjection {
    let path = build_session_path(entries, leaf_id, by_id);
    let (thinking_level, model) = get_session_context_settings(&path);
    let context_entries = build_context_entries(entries, leaf_id, by_id);
    let mut edits: HashMap<String, &Value> = HashMap::new();
    for entry in &context_entries {
        if entry_type(entry) == "context_edit" {
            if let Some(target) = entry.get("targetId").and_then(Value::as_str) {
                edits.insert(target.to_string(), entry);
            }
        }
    }
    let mut projected = Vec::with_capacity(context_entries.len());
    let mut messages = Vec::new();
    for (index, source) in context_entries.iter().enumerate() {
        let msgs = if entry_type(source) == "compaction" && index > 0 {
            Vec::new()
        } else {
            project_context_entry(source, edits.get(entry_id(source)).copied())
        };
        messages.extend(msgs.iter().cloned());
        projected.push(ProjectedSessionEntry {
            source_entry: source.clone(),
            messages: msgs,
        });
    }
    SessionProjection { entries: projected, messages, thinking_level, model }
}

/// `buildSessionContext` — finalized model context.
pub fn build_session_context(
    entries: &[Value],
    leaf_id: LeafId<'_>,
    by_id: Option<&EntryIndex>,
) -> SessionContext {
    let projection = build_session_projection(entries, leaf_id, by_id);
    SessionContext {
        messages: projection.messages,
        thinking_level: projection.thinking_level,
        model: projection.model,
    }
}

/// `getCurrentSystemMessage` from pi-ai (replayed leading system message).
fn get_current_system_message(messages: &[AgentMessage]) -> Option<Value> {
    let mut content: Vec<String> = Vec::new();
    let mut sections: OrderedMap<Value> = OrderedMap::default();
    let mut tools: OrderedMap<Value> = OrderedMap::default();
    let mut timestamp: Option<f64> = None;
    for message in messages {
        let value = serde_json::to_value(message).expect("message serializes");
        if value.get("role").and_then(Value::as_str) != Some("system") {
            continue;
        }
        if timestamp.is_none() {
            if let Some(t) = value.get("timestamp").and_then(Value::as_f64) {
                timestamp = Some(t);
            }
        }
        let content_val = value.get("content");
        let text = match content_val {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(blocks)) => blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .map(|b| b.get("text").and_then(Value::as_str).unwrap_or_default().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        if !text.is_empty() {
            content.push(text);
        }
        if let Some(Value::Object(map)) = value.get("sections") {
            for (name, val) in map {
                if val.is_null() {
                    sections.remove(name);
                } else {
                    sections.set(name, val.clone());
                }
            }
        }
        if let Some(Value::Array(removed)) = value.get("toolsRemoved") {
            for tool in removed {
                if let Some(name) = tool.get("name").and_then(Value::as_str) {
                    tools.remove(name);
                }
            }
        }
        if let Some(Value::Array(added)) = value.get("toolsAdded") {
            for tool in added {
                if let Some(name) = tool.get("name").and_then(Value::as_str) {
                    tools.set(name, tool.clone());
                }
            }
        }
    }
    if timestamp.is_none() && tools.is_empty() {
        return None;
    }
    let mut map = Map::new();
    map.insert("role".to_string(), json!("system"));
    map.insert("content".to_string(), json!(content.join("\n\n")));
    if !sections.is_empty() {
        let mut sm = Map::new();
        for (k, v) in sections.iter() {
            sm.insert(k.to_string(), v.clone());
        }
        map.insert("sections".to_string(), Value::Object(sm));
    }
    if !tools.is_empty() {
        map.insert("toolsAdded".to_string(), Value::Array(tools.iter().map(|(_, v)| v.clone()).collect()));
    }
    let ts = timestamp.unwrap_or(0.0);
    let ts = if ts.fract() == 0.0 { json!(ts as i64) } else { json!(ts) };
    map.insert("timestamp".to_string(), ts);
    Some(Value::Object(map))
}

// ============================================================================
// Default session directory
// ============================================================================

fn get_default_session_dir_path(cwd: &str, agent_dir: &str) -> String {
    let resolved_cwd = resolve_ambient(cwd);
    let resolved_agent_dir = resolve_ambient(agent_dir);
    let stripped = resolved_cwd
        .strip_prefix('/')
        .or_else(|| resolved_cwd.strip_prefix('\\'))
        .unwrap_or(&resolved_cwd);
    let safe: String = stripped
        .chars()
        .map(|c| if c == '/' || c == '\\' || c == ':' { '-' } else { c })
        .collect();
    node_join(&node_join(&resolved_agent_dir, "sessions"), &format!("--{}--", safe))
}

/// `getDefaultSessionDir(cwd, agentDir)` — TS defaults `agentDir` to
/// `getAgentDir()`; it is a required argument here.
pub fn get_default_session_dir(cwd: &str, agent_dir: &str) -> String {
    let session_dir = get_default_session_dir_path(cwd, agent_dir);
    if !Path::new(&session_dir).exists() {
        std::fs::create_dir_all(&session_dir).expect("create default session dir");
    }
    session_dir
}

// ============================================================================
// File loading
// ============================================================================

/// `loadEntriesFromFile` — exported for testing. Reads the file (splitting
/// only on `\n`, mirroring the TS readSync loop), validates the header, and
/// appends a trailing newline to unterminated valid files.
pub fn load_entries_from_file(file_path: &str) -> Vec<Value> {
    let resolved = normalize_path(file_path, &path_opts());
    if !Path::new(&resolved).exists() {
        return Vec::new();
    }
    let bytes = std::fs::read(&resolved).expect("read session file");
    let content = String::from_utf8_lossy(&bytes);
    let mut entries: Vec<Value> = Vec::new();
    let mut start = 0usize;
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            if let Some(entry) = parse_session_entry_line(&content[start..i]) {
                entries.push(entry);
            }
            start = i + 1;
        }
    }
    let pending = &content[start..];
    if let Some(entry) = parse_session_entry_line(pending) {
        entries.push(entry);
    }

    if entries.is_empty() {
        return entries;
    }
    let header = &entries[0];
    if entry_type(header) != "session" || !header.get("id").is_some_and(Value::is_string) {
        return Vec::new();
    }
    if !pending.is_empty() {
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(&resolved) {
            let _ = file.write_all(b"\n");
        }
    }
    entries
}

enum ReadHeaderError {
    ScanLimit,
    Io(String),
}

/// `parseSessionHeaderCandidate`: `None` keeps scanning, `Some(None)` is a
/// parsed non-header entry, `Some(Some(header))` is the header.
fn parse_session_header_candidate(line: &str) -> Option<Option<Value>> {
    if js_trim(line).is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(line) {
        Err(_) => None,
        Ok(entry) => {
            if entry_type(&entry) != "session" || !entry.get("id").is_some_and(Value::is_string) {
                Some(None)
            } else {
                Some(Some(entry))
            }
        }
    }
}

fn read_session_header(file_path: &str) -> Result<Option<Value>, ReadHeaderError> {
    let bytes = std::fs::read(file_path).map_err(|e| ReadHeaderError::Io(e.to_string()))?;
    let scan_len = bytes.len().min(MAX_SESSION_HEADER_SCAN_BYTES);
    let text = String::from_utf8_lossy(&bytes[..scan_len]);
    let mut start = 0usize;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            if let Some(candidate) = parse_session_header_candidate(&text[start..i]) {
                return Ok(candidate);
            }
            start = i + 1;
        }
    }
    if bytes.len() <= MAX_SESSION_HEADER_SCAN_BYTES {
        Ok(parse_session_header_candidate(&text[start..]).unwrap_or(None))
    } else {
        Err(ReadHeaderError::ScanLimit)
    }
}

fn read_session_header_for_discovery(file_path: &str) -> Option<Value> {
    read_session_header(file_path).ok().flatten()
}

fn get_session_header_cwd(header: &Value) -> Option<&str> {
    header.get("cwd").and_then(Value::as_str)
}

fn session_cwd_matches(cwd: Option<&str>, resolved_cwd: &str) -> bool {
    match cwd {
        Some(c) if !c.is_empty() => resolve_ambient(c) == resolved_cwd,
        _ => false,
    }
}

/// `findMostRecentSession` — exported for testing. Ties on mtime keep
/// `read_dir` order (TS relies on the stability of `Array.prototype.sort`
/// over `readdirSync` order; both are filesystem-dependent).
pub fn find_most_recent_session(session_dir: &str, cwd: Option<&str>) -> Option<String> {
    let resolved_session_dir = normalize_path(session_dir, &path_opts());
    let resolved_cwd = cwd.map(resolve_ambient);
    let read_dir = std::fs::read_dir(&resolved_session_dir).ok()?;
    let mut files: Vec<(String, f64)> = Vec::new();
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".jsonl") {
            continue;
        }
        let path = node_join(&resolved_session_dir, &name);
        let metadata = std::fs::metadata(&path).ok()?; // statSync failure aborts discovery (TS try/catch)
        files.push((path, mtime_ms(&metadata)));
    }
    files.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    for (path, _) in files {
        if let Some(header) = read_session_header_for_discovery(&path) {
            let matches = resolved_cwd
                .as_deref()
                .is_none_or(|cwd| session_cwd_matches(get_session_header_cwd(&header), cwd));
            if matches {
                return Some(path);
            }
        }
    }
    None
}

fn mtime_ms(metadata: &std::fs::Metadata) -> f64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

// ============================================================================
// SessionInfo
// ============================================================================

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub path: String,
    pub id: String,
    /// Working directory where the session was started.
    pub cwd: String,
    /// User-defined display name from session_info entries.
    pub name: Option<String>,
    /// Path to the parent session, if forked.
    pub parent_session_path: Option<String>,
    /// `new Date(header.timestamp)` in Unix millis; `None` for invalid dates.
    pub created: Option<i64>,
    /// Last activity time in Unix millis.
    pub modified: i64,
    pub message_count: usize,
    pub first_message: String,
    pub all_messages_text: String,
}

fn is_message_with_content(message: &Value) -> bool {
    message.get("role").is_some_and(Value::is_string) && message.get("content").is_some()
}

fn extract_text_content(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .map(|b| b.get("text").and_then(Value::as_str).unwrap_or_default().to_string())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn get_message_activity_time(entry: &Value) -> Option<f64> {
    let message = entry.get("message")?;
    if !is_message_with_content(message) {
        return None;
    }
    let role = message.get("role").and_then(Value::as_str).unwrap_or("");
    if role != "user" && role != "assistant" {
        return None;
    }
    if let Some(t) = message.get("timestamp").and_then(Value::as_f64) {
        return Some(t);
    }
    entry
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_date_millis)
        .map(|t| t as f64)
}

/// Node `readline` line splitting (with `crlfDelay: Infinity`): `\r\n` is a
/// single terminator; bare `\n` and `\r` each terminate a line.
fn split_readline_lines(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                lines.push(&s[start..i]);
                i += 1;
                start = i;
            }
            b'\r' => {
                let end = if i + 1 < bytes.len() && bytes[i + 1] == b'\n' { i + 1 } else { i };
                lines.push(&s[start..i]);
                i = end + 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// `buildSessionInfo` (sync port).
fn build_session_info(file_path: &str) -> Option<SessionInfo> {
    let metadata = std::fs::metadata(file_path).ok()?;
    let bytes = std::fs::read(file_path).ok()?;
    let content = String::from_utf8_lossy(&bytes);

    let mut header: Option<Value> = None;
    let mut message_count = 0usize;
    let mut first_message = String::new();
    let mut all_messages: Vec<String> = Vec::new();
    let mut name: Option<String> = None;
    let mut last_activity_time: Option<f64> = None;

    for line in split_readline_lines(&content) {
        let Some(entry) = parse_session_entry_line(line) else {
            continue;
        };
        if header.is_none() {
            if entry_type(&entry) != "session" {
                return None;
            }
            header = Some(entry);
            continue;
        }
        if entry_type(&entry) == "session_info" {
            let trimmed = entry.get("name").and_then(Value::as_str).map(js_trim);
            name = trimmed.filter(|n| !n.is_empty()).map(str::to_string);
        }
        if entry_type(&entry) != "message" {
            continue;
        }
        message_count += 1;
        if let Some(activity_time) = get_message_activity_time(&entry) {
            last_activity_time = Some(last_activity_time.map_or(activity_time, |prev| {
                prev.max(activity_time)
            }));
        }
        let message = entry.get("message").cloned().unwrap_or(Value::Null);
        if !is_message_with_content(&message) {
            continue;
        }
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if role != "user" && role != "assistant" {
            continue;
        }
        let text = extract_text_content(&message);
        if text.is_empty() {
            continue;
        }
        all_messages.push(text.clone());
        if first_message.is_empty() && role == "user" {
            first_message = text;
        }
    }

    let header = header?;
    let cwd = header.get("cwd").and_then(Value::as_str).unwrap_or_default().to_string();
    let parent_session_path = header
        .get("parentSession")
        .and_then(Value::as_str)
        .map(str::to_string);
    let header_time = header
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_date_millis);
    let modified = match last_activity_time {
        Some(t) if t > 0.0 => t as i64,
        _ => header_time.unwrap_or_else(|| mtime_ms(&metadata) as i64),
    };
    Some(SessionInfo {
        path: file_path.to_string(),
        id: header.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
        cwd,
        name,
        parent_session_path,
        created: header
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_date_millis),
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

type SessionListProgress<'a> = Option<&'a mut dyn FnMut(usize, usize, Option<&[SessionInfo]>)>;

fn sort_session_infos(sessions: &mut [SessionInfo]) {
    sessions.sort_by_key(|session| std::cmp::Reverse(session.modified));
}

fn list_sessions_from_dir(
    dir: &str,
    mut on_progress: SessionListProgress,
) -> Vec<SessionInfo> {
    if !Path::new(dir).exists() {
        return Vec::new();
    }
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read_dir
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".jsonl"))
        .collect();
    // TS sorts with `b.localeCompare(a)`; locale collation is approximated
    // with byte-wise comparison (only affects tie ordering).
    names.sort_by(|a, b| b.cmp(a));
    let total = names.len();
    let mut sessions = Vec::new();
    let mut partial: Vec<SessionInfo> = Vec::new();
    let mut loaded = 0usize;
    for name in names {
        let path = node_join(dir, &name);
        if let Some(info) = build_session_info(&path) {
            partial.push(info.clone());
            sessions.push(info);
        }
        loaded += 1;
        let publish = loaded == 1 || loaded.is_multiple_of(10) || loaded == total;
        if let Some(progress) = on_progress.as_deref_mut() {
            let snapshot: Vec<SessionInfo> = partial.clone();
            if publish {
                let mut sorted = snapshot;
                sort_session_infos(&mut sorted);
                progress(loaded, total, Some(&sorted));
            } else {
                progress(loaded, total, None);
            }
        }
    }
    sessions
}

// ============================================================================
// SessionManager
// ============================================================================

#[derive(Debug, Clone, Default)]
pub struct NewSessionOptions {
    pub id: Option<String>,
    pub parent_session: Option<String>,
}

/// Manages conversation sessions as append-only trees stored in JSONL files.
pub struct SessionManager {
    session_id: String,
    session_file: Option<String>,
    session_dir: String,
    cwd: String,
    persist: bool,
    flushed: bool,
    file_entries: Vec<FileEntry>,
    by_id: EntryIndex,
    labels_by_id: OrderedMap<String>,
    label_timestamps_by_id: OrderedMap<String>,
    leaf_id: Option<String>,
    gen: Generators,
}

fn stringify_entry(entry: &Value) -> String {
    serde_json::to_string(entry).expect("entry serializes")
}

fn entry_base(entry_type: &str, id: &str, parent_id: Option<&str>, timestamp: &str) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("type".to_string(), json!(entry_type));
    map.insert("id".to_string(), json!(id));
    map.insert(
        "parentId".to_string(),
        parent_id.map(|p| json!(p)).unwrap_or(Value::Null),
    );
    map.insert("timestamp".to_string(), json!(timestamp));
    map
}

#[allow(clippy::too_many_arguments)]
impl SessionManager {
    fn new_inner(
        cwd: &str,
        session_dir: &str,
        session_file: Option<&str>,
        persist: bool,
        options: Option<&NewSessionOptions>,
        preloaded_file_entries: Option<Vec<FileEntry>>,
        gen: &Generators,
    ) -> Result<SessionManager, String> {
        let mut manager = SessionManager {
            session_id: String::new(),
            session_file: None,
            session_dir: normalize_path(session_dir, &path_opts()),
            cwd: resolve_ambient(cwd),
            persist,
            flushed: false,
            file_entries: Vec::new(),
            by_id: EntryIndex::default(),
            labels_by_id: OrderedMap::default(),
            label_timestamps_by_id: OrderedMap::default(),
            leaf_id: None,
            gen: gen.clone(),
        };
        if manager.persist && !manager.session_dir.is_empty() && !Path::new(&manager.session_dir).exists() {
            std::fs::create_dir_all(&manager.session_dir).expect("create session dir");
        }
        if let Some(file) = session_file {
            manager.set_session_file_internal(file, preloaded_file_entries)?;
        } else if preloaded_file_entries.as_ref().is_some_and(|e| !e.is_empty()) {
            manager.load_entries(preloaded_file_entries.unwrap_or_default(), options)?;
        } else {
            manager.new_session(options)?;
        }
        Ok(manager)
    }

    /// `static create(cwd, sessionDir?, options?)`.
    pub fn create(
        cwd: &str,
        session_dir: Option<&str>,
        options: Option<&NewSessionOptions>,
        gen: &Generators,
    ) -> Result<SessionManager, String> {
        let dir = session_dir
            .map(|d| normalize_path(d, &path_opts()))
            .unwrap_or_else(|| get_default_session_dir(cwd, "/"));
        SessionManager::new_inner(cwd, &dir, None, true, options, None, gen)
    }

    /// `static open(path, sessionDir?, cwdOverride?)`.
    pub fn open(
        path: &str,
        session_dir: Option<&str>,
        cwd_override: Option<&str>,
        gen: &Generators,
    ) -> Result<SessionManager, String> {
        let resolved_path = resolve_ambient(path);
        let mut header: Option<Value> = None;
        let mut preloaded_file_entries: Option<Vec<FileEntry>> = None;
        if cwd_override.is_none() && Path::new(&resolved_path).exists() {
            match read_session_header(&resolved_path) {
                Ok(h) => header = h,
                Err(ReadHeaderError::Io(message)) => return Err(message),
                Err(ReadHeaderError::ScanLimit) => {
                    let entries = load_entries_from_file(&resolved_path);
                    header = entries
                        .first()
                        .filter(|e| entry_type(e) == "session")
                        .cloned();
                    preloaded_file_entries = Some(entries);
                }
            }
        }
        let header_cwd = header.as_ref().and_then(get_session_header_cwd);
        let cwd = cwd_override.or(header_cwd).unwrap_or(AMBIENT_CWD);
        let dir = session_dir
            .map(|d| normalize_path(d, &path_opts()))
            .unwrap_or_else(|| resolve_path("..", &resolved_path, &path_opts()));
        SessionManager::new_inner(cwd, &dir, Some(&resolved_path), true, None, preloaded_file_entries, gen)
    }

    /// `static continueRecent(cwd, sessionDir?)`.
    pub fn continue_recent(
        cwd: &str,
        session_dir: Option<&str>,
        agent_dir: &str,
        gen: &Generators,
    ) -> Result<SessionManager, String> {
        let dir = session_dir
            .map(|d| normalize_path(d, &path_opts()))
            .unwrap_or_else(|| get_default_session_dir(cwd, agent_dir));
        let filter_cwd = session_dir.is_some() && dir != get_default_session_dir_path(cwd, agent_dir);
        let most_recent = find_most_recent_session(&dir, if filter_cwd { Some(cwd) } else { None });
        match most_recent {
            Some(file) => SessionManager::new_inner(cwd, &dir, Some(&file), true, None, None, gen),
            None => SessionManager::new_inner(cwd, &dir, None, true, None, None, gen),
        }
    }

    /// `static inMemory(cwd = process.cwd(), options?, entries?)`.
    pub fn in_memory(
        cwd: &str,
        options: Option<&NewSessionOptions>,
        entries: Vec<FileEntry>,
        gen: &Generators,
    ) -> Result<SessionManager, String> {
        SessionManager::new_inner(cwd, "", None, false, options, Some(entries), gen)
    }

    /// `static forkFrom(sourcePath, targetCwd, sessionDir?, options?)`.
    pub fn fork_from(
        source_path: &str,
        target_cwd: &str,
        session_dir: Option<&str>,
        options: Option<&NewSessionOptions>,
        gen: &Generators,
    ) -> Result<SessionManager, String> {
        let resolved_source_path = resolve_ambient(source_path);
        let resolved_target_cwd = resolve_ambient(target_cwd);
        let source_entries = load_entries_from_file(&resolved_source_path);
        if source_entries.is_empty() {
            return Err(format!(
                "Cannot fork: source session file is empty or invalid: {}",
                resolved_source_path
            ));
        }
        let source_header = source_entries.iter().find(|e| entry_type(e) == "session");
        let Some(source_header) = source_header else {
            return Err(format!(
                "Cannot fork: source session has no header: {}",
                resolved_source_path
            ));
        };
        let parent_session = source_header.get("parentSession").cloned();

        let dir = session_dir
            .map(|d| normalize_path(d, &path_opts()))
            .unwrap_or_else(|| get_default_session_dir(&resolved_target_cwd, "/"));
        if !Path::new(&dir).exists() {
            std::fs::create_dir_all(&dir).expect("create fork session dir");
        }

        if let Some(opts) = options {
            if let Some(id) = &opts.id {
                assert_valid_session_id(id)?;
            }
        }
        let new_session_id = options
            .and_then(|o| o.id.clone())
            .unwrap_or_else(|| create_session_id(gen));
        let timestamp = gen.now_iso();
        let file_timestamp = timestamp.replace([':', '.'], "-");
        let new_session_file = node_join(&dir, &format!("{}_{}.jsonl", file_timestamp, new_session_id));

        let mut header_map = Map::new();
        header_map.insert("type".to_string(), json!("session"));
        header_map.insert("version".to_string(), json!(CURRENT_SESSION_VERSION));
        header_map.insert("id".to_string(), json!(new_session_id));
        header_map.insert("timestamp".to_string(), json!(timestamp));
        header_map.insert("cwd".to_string(), json!(resolved_target_cwd));
        header_map.insert("parentSession".to_string(), json!(resolved_source_path));
        let header = Value::Object(header_map);

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&new_session_file)
            .expect("create fork session file");
        writeln!(file, "{}", stringify_entry(&header)).expect("write fork header");
        drop(file);

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&new_session_file)
            .expect("open fork session file");
        for entry in &source_entries {
            if entry_type(entry) != "session" {
                writeln!(file, "{}", stringify_entry(entry)).expect("write fork entry");
            }
        }
        drop(file);
        let _ = parent_session;

        SessionManager::new_inner(&resolved_target_cwd, &dir, Some(&new_session_file), true, None, None, gen)
    }

    /// `static findById(cwd, id, sessionDir?)`.
    pub fn find_by_id(
        cwd: &str,
        id: &str,
        session_dir: Option<&str>,
        agent_dir: &str,
    ) -> Option<String> {
        let dir = session_dir
            .map(|d| normalize_path(d, &path_opts()))
            .unwrap_or_else(|| get_default_session_dir(cwd, agent_dir));
        let filter_cwd = session_dir.is_some() && dir != get_default_session_dir_path(cwd, agent_dir);
        let resolved_cwd = resolve_ambient(cwd);
        let read_dir = std::fs::read_dir(&dir).ok()?;
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".jsonl") {
                continue;
            }
            let path = node_join(&dir, &name);
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

    /// `static list(cwd, sessionDir?, onProgress?, signal?)` (sync port).
    pub fn list(
        cwd: &str,
        session_dir: Option<&str>,
        agent_dir: &str,
        on_progress: SessionListProgress,
    ) -> Vec<SessionInfo> {
        let dir = session_dir
            .map(|d| normalize_path(d, &path_opts()))
            .unwrap_or_else(|| get_default_session_dir(cwd, agent_dir));
        let filter_cwd = session_dir.is_some() && dir != get_default_session_dir_path(cwd, agent_dir);
        let resolved_cwd = resolve_ambient(cwd);
        let include = |session: &SessionInfo| {
            !filter_cwd || session_cwd_matches(Some(session.cwd.as_str()), &resolved_cwd)
        };
        let mut sessions = match on_progress {
            Some(progress) => {
                let mut wrapped =
                    move |loaded: usize, total: usize, partial: Option<&[SessionInfo]>| {
                        let filtered: Option<Vec<SessionInfo>> = partial
                            .map(|p| p.iter().filter(|s| include(s)).cloned().collect());
                        progress(loaded, total, filtered.as_deref());
                    };
                list_sessions_from_dir(&dir, Some(&mut wrapped))
            }
            None => list_sessions_from_dir(&dir, None),
        };
        sessions.retain(include);
        sort_session_infos(&mut sessions);
        sessions
    }

    /// `static listAll(sessionDir?, onProgress?, signal?)` (sync port). When
    /// `session_dir` is `None`, `agent_dir` supplies the sessions root that TS
    /// read from ambient config (`getSessionsDir()`).
    pub fn list_all(
        session_dir: Option<&str>,
        agent_dir: &str,
        mut on_progress: SessionListProgress,
    ) -> Vec<SessionInfo> {
        if let Some(custom_dir) = session_dir {
            let custom_dir = normalize_path(custom_dir, &path_opts());
            let mut sessions = match on_progress {
                Some(progress) => {
                    let mut passthrough =
                        |loaded: usize, total: usize, partial: Option<&[SessionInfo]>| {
                            progress(loaded, total, partial);
                        };
                    list_sessions_from_dir(&custom_dir, Some(&mut passthrough))
                }
                None => list_sessions_from_dir(&custom_dir, None),
            };
            sort_session_infos(&mut sessions);
            return sessions;
        }

        let sessions_dir = node_join(&resolve_ambient(agent_dir), "sessions");
        if !Path::new(&sessions_dir).exists() {
            return Vec::new();
        }
        let Ok(read_dir) = std::fs::read_dir(&sessions_dir) else {
            return Vec::new();
        };
        let mut dirs: Vec<String> = Vec::new();
        for entry in read_dir.flatten() {
            let file_type = entry.file_type().ok();
            let is_dir_or_link = file_type.as_ref().map(|ft| ft.is_dir() || ft.is_symlink()).unwrap_or(false);
            if is_dir_or_link {
                dirs.push(node_join(&sessions_dir, &entry.file_name().to_string_lossy()));
            }
        }
        let mut all_files: Vec<String> = Vec::new();
        for dir in &dirs {
            if let Ok(read_dir) = std::fs::read_dir(dir) {
                for entry in read_dir.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".jsonl") {
                        all_files.push(node_join(dir, &name));
                    }
                }
            }
        }
        let mut candidates: Vec<(String, Option<f64>)> = all_files
            .into_iter()
            .map(|path| {
                let mtime = std::fs::metadata(&path).ok().map(|m| mtime_ms(&m));
                (path, mtime)
            })
            .collect();
        candidates.sort_by(|a, b| {
            let am = a.1.unwrap_or(f64::NEG_INFINITY);
            let bm = b.1.unwrap_or(f64::NEG_INFINITY);
            let base = bm.partial_cmp(&am).unwrap_or(std::cmp::Ordering::Equal);
            fn basename(p: &str) -> &str {
                p.rsplit('/').next().unwrap_or(p)
            }
            base.then_with(|| basename(&b.0).cmp(basename(&a.0)))
        });

        let total_files = candidates.len();
        let mut loaded = 0usize;
        let mut first_candidate_loaded = false;
        let mut results: Vec<Option<SessionInfo>> = Vec::new();
        let mut partial: Vec<SessionInfo> = Vec::new();
        for (index, (path, _)) in candidates.iter().enumerate() {
            let info = build_session_info(path);
            if index == 0 {
                first_candidate_loaded = true;
            }
            if let Some(info) = &info {
                partial.push(info.clone());
            }
            results.push(info);
            loaded += 1;
            let publish = first_candidate_loaded
                && (index == 0 || loaded.is_multiple_of(100) || loaded == total_files);
            if let Some(progress) = on_progress.as_deref_mut() {
                if publish {
                    let mut sorted = partial.clone();
                    sort_session_infos(&mut sorted);
                    progress(loaded, total_files, Some(&sorted));
                } else {
                    progress(loaded, total_files, None);
                }
            }
        }
        let mut sessions: Vec<SessionInfo> = results.into_iter().flatten().collect();
        sort_session_infos(&mut sessions);
        sessions
    }

    // ========================================================================
    // Session lifecycle
    // ========================================================================

    /// `setSessionFile`.
    pub fn set_session_file(&mut self, session_file: &str) -> Result<(), String> {
        self.set_session_file_internal(session_file, None)
    }

    fn set_session_file_internal(
        &mut self,
        session_file: &str,
        preloaded_file_entries: Option<Vec<FileEntry>>,
    ) -> Result<(), String> {
        let resolved = resolve_ambient(session_file);
        self.session_file = Some(resolved.clone());
        if Path::new(&resolved).exists() {
            let entries = preloaded_file_entries.unwrap_or_else(|| load_entries_from_file(&resolved));
            if entries.is_empty() {
                let explicit_path = resolved.clone();
                let size = std::fs::metadata(&explicit_path)
                    .map(|m| m.len())
                    .expect("stat session file");
                if size > 0 {
                    return Err(format!("Session file is not a valid {} session: {}", APP_NAME, explicit_path));
                }
                self.new_session(None)?;
                self.session_file = Some(explicit_path.clone());
                self.rewrite_file();
                self.flushed = true;
                return Ok(());
            }
            self.load_entries(entries, None)?;
            self.flushed = true;
        } else {
            let explicit_path = resolved.clone();
            self.new_session(None)?;
            self.session_file = Some(explicit_path); // preserve explicit path from --session flag
        }
        Ok(())
    }

    /// `newSession(options?)`. Returns the new session file path (persisting
    /// sessions) or `None`.
    pub fn new_session(&mut self, options: Option<&NewSessionOptions>) -> Result<Option<String>, String> {
        if let Some(opts) = options {
            if let Some(id) = &opts.id {
                assert_valid_session_id(id)?;
            }
        }
        self.session_id = options
            .and_then(|o| o.id.clone())
            .unwrap_or_else(|| create_session_id(&self.gen));
        let timestamp = self.gen.now_iso();
        let mut header = Map::new();
        header.insert("type".to_string(), json!("session"));
        header.insert("version".to_string(), json!(CURRENT_SESSION_VERSION));
        header.insert("id".to_string(), json!(self.session_id.clone()));
        header.insert("timestamp".to_string(), json!(timestamp.clone()));
        header.insert("cwd".to_string(), json!(self.cwd.clone()));
        if let Some(parent) = options.and_then(|o| o.parent_session.clone()) {
            header.insert("parentSession".to_string(), json!(parent));
        }
        self.file_entries = vec![Value::Object(header)];
        self.by_id.clear();
        self.labels_by_id.clear();
        self.label_timestamps_by_id.clear();
        self.leaf_id = None;
        self.flushed = false;

        if self.persist {
            let file_timestamp = timestamp.replace([':', '.'], "-");
            self.session_file = Some(node_join(
                &self.session_dir,
                &format!("{}_{}.jsonl", file_timestamp, self.session_id),
            ));
        }
        Ok(self.session_file.clone())
    }

    fn load_entries(&mut self, entries: Vec<FileEntry>, options: Option<&NewSessionOptions>) -> Result<(), String> {
        let has_header = entries.iter().any(|e| entry_type(e) == "session");
        if has_header {
            self.session_id = entries
                .iter()
                .find(|e| entry_type(e) == "session")
                .and_then(|h| h.get("id").and_then(Value::as_str))
                .unwrap_or_default()
                .to_string();
            self.file_entries = entries;
            let uuid_v4 = self.gen.uuid_v4.clone();
            if migrate_to_current_version(&mut self.file_entries, &uuid_v4) {
                self.rewrite_file();
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
        self.labels_by_id.clear();
        self.label_timestamps_by_id.clear();
        self.leaf_id = None;
        for entry in &self.file_entries {
            if entry_type(entry) == "session" {
                continue;
            }
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            self.by_id.set(id, entry.clone());
            self.leaf_id = Some(id.to_string());
            if entry_type(entry) == "label" {
                let target_id = entry.get("targetId").and_then(Value::as_str).unwrap_or_default();
                let label = entry.get("label");
                if js_truthy(label) {
                    self.labels_by_id.set(target_id, label.and_then(Value::as_str).unwrap_or_default().to_string());
                    self.label_timestamps_by_id.set(
                        target_id,
                        entry.get("timestamp").and_then(Value::as_str).unwrap_or_default().to_string(),
                    );
                } else {
                    self.labels_by_id.remove(target_id);
                    self.label_timestamps_by_id.remove(target_id);
                }
            }
        }
    }

    fn rewrite_file(&self) {
        let Some(file) = &self.session_file else {
            return;
        };
        if !self.persist {
            return;
        }
        let mut handle = std::fs::File::create(file).expect("rewrite session file");
        for entry in &self.file_entries {
            writeln!(handle, "{}", stringify_entry(entry)).expect("write session entry");
        }
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

    pub fn uses_default_session_dir(&self, agent_dir: &str) -> bool {
        self.session_dir == get_default_session_dir_path(&self.cwd, agent_dir)
    }

    pub fn get_session_id(&self) -> &str {
        &self.session_id
    }

    pub fn get_session_file(&self) -> Option<&str> {
        self.session_file.as_deref()
    }

    fn has_conversation(&self) -> bool {
        self.file_entries.iter().any(|e| {
            entry_type(e) == "message"
                && matches!(
                    e.get("message").and_then(|m| m.get("role")).and_then(Value::as_str),
                    Some("user") | Some("assistant")
                )
        })
    }

    fn persist_entry(&mut self, entry: &Value) {
        if !self.persist {
            return;
        }
        let Some(file) = self.session_file.clone() else {
            return;
        };
        if !self.flushed {
            if !self.has_conversation() {
                return;
            }
            let mut handle = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
                .expect("create session file");
            for e in &self.file_entries {
                writeln!(handle, "{}", stringify_entry(e)).expect("write session entry");
            }
            self.flushed = true;
        } else {
            let mut handle = std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .expect("open session file");
            writeln!(handle, "{}", stringify_entry(entry)).expect("append session entry");
        }
    }

    fn append_entry(&mut self, entry: Value) {
        let id = entry.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        self.file_entries.push(entry.clone());
        self.by_id.set(&id, entry.clone());
        self.leaf_id = Some(id);
        self.persist_entry(&entry);
    }

    // ========================================================================
    // Append operations
    // ========================================================================

    pub fn append_message(&mut self, message: Value) -> String {
        let id = generate_id(&|k| self.by_id.has(k), &self.gen.uuid_v4.clone());
        let mut map = entry_base("message", &id, self.leaf_id.as_deref(), &self.gen.now_iso());
        map.insert("message".to_string(), message);
        let entry = Value::Object(map);
        self.append_entry(entry);
        id
    }

    pub fn append_thinking_level_change(&mut self, thinking_level: &str) -> String {
        let id = generate_id(&|k| self.by_id.has(k), &self.gen.uuid_v4.clone());
        let mut map = entry_base("thinking_level_change", &id, self.leaf_id.as_deref(), &self.gen.now_iso());
        map.insert("thinkingLevel".to_string(), json!(thinking_level));
        let entry = Value::Object(map);
        self.append_entry(entry);
        id
    }

    pub fn append_model_change(&mut self, provider: &str, model_id: &str) -> String {
        let id = generate_id(&|k| self.by_id.has(k), &self.gen.uuid_v4.clone());
        let mut map = entry_base("model_change", &id, self.leaf_id.as_deref(), &self.gen.now_iso());
        map.insert("provider".to_string(), json!(provider));
        map.insert("modelId".to_string(), json!(model_id));
        let entry = Value::Object(map);
        self.append_entry(entry);
        id
    }

    pub fn append_usage(
        &mut self,
        kind: &str,
        provider: &str,
        model: &str,
        usage: Value,
        note: Option<&str>,
    ) -> Value {
        let id = generate_id(&|k| self.by_id.has(k), &self.gen.uuid_v4.clone());
        let mut map = entry_base("usage", &id, self.leaf_id.as_deref(), &self.gen.now_iso());
        map.insert("kind".to_string(), json!(kind));
        map.insert("provider".to_string(), json!(provider));
        map.insert("model".to_string(), json!(model));
        map.insert("usage".to_string(), usage);
        if let Some(note) = note.filter(|n| !n.is_empty()) {
            map.insert("note".to_string(), json!(note));
        }
        let entry = Value::Object(map);
        self.append_entry(entry.clone());
        entry
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: Option<&str>,
        tokens_before: i64,
        details: Option<Value>,
        from_hook: Option<bool>,
        usage: Option<Value>,
    ) -> String {
        let timestamp = self.gen.now_iso();
        let system_message =
            get_current_system_message(&self.build_session_projection().messages);
        let id = generate_id(&|k| self.by_id.has(k), &self.gen.uuid_v4.clone());
        let mut map = entry_base("compaction", &id, self.leaf_id.as_deref(), &timestamp);
        map.insert("summary".to_string(), json!(summary));
        map.insert(
            "firstKeptEntryId".to_string(),
            json!(first_kept_entry_id.unwrap_or(&id)),
        );
        map.insert("tokensBefore".to_string(), json!(tokens_before));
        if let Some(details) = details {
            map.insert("details".to_string(), details);
        }
        if let Some(usage) = usage {
            map.insert("usage".to_string(), usage);
        }
        if let Some(from_hook) = from_hook {
            map.insert("fromHook".to_string(), json!(from_hook));
        }
        if let Some(mut system_message) = system_message {
            let ts = parse_date_millis(&timestamp);
            if let Some(obj) = system_message.as_object_mut() {
                obj.insert(
                    "timestamp".to_string(),
                    ts.map(|t| json!(t)).unwrap_or(Value::Null),
                );
            }
            map.insert("systemMessage".to_string(), system_message);
        }
        let entry = Value::Object(map);
        self.append_entry(entry);
        id
    }

    pub fn append_custom_entry(&mut self, custom_type: &str, data: Option<Value>) -> String {
        let id = generate_id(&|k| self.by_id.has(k), &self.gen.uuid_v4.clone());
        let mut map = Map::new();
        map.insert("type".to_string(), json!("custom"));
        map.insert("customType".to_string(), json!(custom_type));
        if let Some(data) = data {
            map.insert("data".to_string(), data);
        }
        map.insert("id".to_string(), json!(id.clone()));
        map.insert(
            "parentId".to_string(),
            self.leaf_id.as_deref().map(|p| json!(p)).unwrap_or(Value::Null),
        );
        map.insert("timestamp".to_string(), json!(self.gen.now_iso()));
        let entry = Value::Object(map);
        self.append_entry(entry);
        id
    }

    pub fn append_session_info(&mut self, name: &str) -> String {
        let sanitized = replace_crlf(name);
        let id = generate_id(&|k| self.by_id.has(k), &self.gen.uuid_v4.clone());
        let mut map = entry_base("session_info", &id, self.leaf_id.as_deref(), &self.gen.now_iso());
        map.insert("name".to_string(), json!(sanitized));
        let entry = Value::Object(map);
        self.append_entry(entry);
        id
    }

    pub fn get_session_name(&self) -> Option<String> {
        for entry in self.file_entries.iter().rev() {
            if entry_type(entry) == "session_info" {
                return entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(js_trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_string);
            }
        }
        None
    }

    pub fn append_custom_message_entry(
        &mut self,
        custom_type: &str,
        content: Value,
        display: Option<bool>,
        details: Option<Value>,
    ) -> String {
        let id = generate_id(&|k| self.by_id.has(k), &self.gen.uuid_v4.clone());
        let mut map = Map::new();
        map.insert("type".to_string(), json!("custom_message"));
        map.insert("customType".to_string(), json!(custom_type));
        map.insert("content".to_string(), content);
        // TS: `display` is `undefined` when the caller omits it, and
        // `JSON.stringify` drops the key.
        if let Some(display) = display {
            map.insert("display".to_string(), json!(display));
        }
        if let Some(details) = details {
            map.insert("details".to_string(), details);
        }
        map.insert("id".to_string(), json!(id.clone()));
        map.insert(
            "parentId".to_string(),
            self.leaf_id.as_deref().map(|p| json!(p)).unwrap_or(Value::Null),
        );
        map.insert("timestamp".to_string(), json!(self.gen.now_iso()));
        let entry = Value::Object(map);
        self.append_entry(entry);
        id
    }

    /// `appendContextEdit(targetId, replacement)` — `replacement` is `None`
    /// (null) or `Some({"content": string | array})`.
    pub fn append_context_edit(&mut self, target_id: &str, replacement: Option<Value>) -> Result<String, String> {
        let valid = match &replacement {
            None => true,
            Some(r) => {
                r.is_object()
                    && r.get("content")
                        .is_some_and(|c| c.is_string() || c.is_array())
            }
        };
        if !valid {
            return Err("Context edit replacement must be null or contain string/array content".to_string());
        }
        let target = self
            .by_id
            .get(target_id)
            .cloned()
            .ok_or_else(|| format!("Entry {} not found", target_id))?;
        if !self.get_branch(None).iter().any(|e| entry_id(e) == target_id) {
            return Err(format!("Entry {} is not on the active branch", target_id));
        }
        let editable = entry_type(&target) == "custom_message"
            || (entry_type(&target) == "message"
                && matches!(
                    target.get("message").and_then(|m| m.get("role")).and_then(Value::as_str),
                    Some("user") | Some("assistant") | Some("toolResult")
                ));
        if !editable {
            return Err(format!("Entry {} does not contribute editable model content", target_id));
        }
        let target_role = if entry_type(&target) == "message" {
            target
                .get("message")
                .and_then(|m| m.get("role"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        } else {
            "custom".to_string()
        };
        let normalized = match &replacement {
            Some(r) if target_role == "assistant" || target_role == "toolResult" => {
                match r.get("content").and_then(Value::as_str) {
                    Some(text) => Some(json!({ "content": [{ "type": "text", "text": text }] })),
                    None => replacement.clone(),
                }
            }
            _ => replacement.clone(),
        };

        let id = generate_id(&|k| self.by_id.has(k), &self.gen.uuid_v4.clone());
        let mut map = entry_base("context_edit", &id, self.leaf_id.as_deref(), &self.gen.now_iso());
        map.insert("targetId".to_string(), json!(target_id));
        map.insert("replacement".to_string(), normalized.unwrap_or(Value::Null));
        let entry = Value::Object(map);
        self.append_entry(entry);
        Ok(id)
    }

    // ========================================================================
    // Tree traversal
    // ========================================================================

    pub fn get_leaf_id(&self) -> Option<&str> {
        self.leaf_id.as_deref()
    }

    pub fn get_leaf_entry(&self) -> Option<&Value> {
        self.leaf_id.as_deref().and_then(|id| self.by_id.get(id))
    }

    pub fn get_entry(&self, id: &str) -> Option<&Value> {
        self.by_id.get(id)
    }

    /// All direct children of an entry, in insertion order.
    pub fn get_children(&self, parent_id: &str) -> Vec<Value> {
        self.by_id
            .iter()
            .filter(|(_, entry)| entry.get("parentId") == Some(&Value::String(parent_id.to_string())))
            .map(|(_, entry)| entry.clone())
            .collect()
    }

    pub fn get_label(&self, id: &str) -> Option<&str> {
        self.labels_by_id.get(id).map(String::as_str)
    }

    /// `appendLabelChange(targetId, label)` — `None` clears the label.
    pub fn append_label_change(&mut self, target_id: &str, label: Option<&str>) -> Result<String, String> {
        if !self.by_id.has(target_id) {
            return Err(format!("Entry {} not found", target_id));
        }
        let id = generate_id(&|k| self.by_id.has(k), &self.gen.uuid_v4.clone());
        let mut map = entry_base("label", &id, self.leaf_id.as_deref(), &self.gen.now_iso());
        map.insert("targetId".to_string(), json!(target_id));
        if let Some(label) = label {
            map.insert("label".to_string(), json!(label));
        }
        let entry = Value::Object(map);
        let timestamp = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.append_entry(entry);
        if let Some(label) = label {
            self.labels_by_id.set(target_id, label.to_string());
            self.label_timestamps_by_id.set(target_id, timestamp);
        } else {
            self.labels_by_id.remove(target_id);
            self.label_timestamps_by_id.remove(target_id);
        }
        Ok(id)
    }

    /// Walk from entry to root, returning entries in path order.
    pub fn get_branch(&self, from_id: Option<&str>) -> Vec<Value> {
        let start_id = from_id.or(self.leaf_id.as_deref());
        let mut path = Vec::new();
        let mut current = start_id.and_then(|id| self.by_id.get(id));
        while let Some(entry) = current {
            path.push(entry.clone());
            current = truthy_parent_id(entry).and_then(|p| self.by_id.get(p));
        }
        path.reverse();
        path
    }

    pub fn build_context_entries(&self) -> Vec<Value> {
        build_context_entries(&self.get_entries(), self.leaf_id.as_deref().into(), Some(&self.by_id))
    }

    pub fn build_session_projection(&self) -> SessionProjection {
        build_session_projection(&self.get_entries(), self.leaf_id.as_deref().into(), Some(&self.by_id))
    }

    pub fn build_session_context(&self) -> SessionContext {
        let projection = self.build_session_projection();
        SessionContext {
            messages: projection.messages,
            thinking_level: projection.thinking_level,
            model: projection.model,
        }
    }

    /// The manager's generators — shared with preview managers so entry
    /// ids draw from the same sequence (TS: one global uuid source).
    pub fn generators(&self) -> &Generators {
        &self.gen
    }

    pub fn get_header(&self) -> Option<&Value> {
        self.file_entries.iter().find(|e| entry_type(e) == "session")
    }

    pub fn get_entry_count(&self) -> usize {
        self.by_id.len()
    }

    /// All session entries (excludes header). Returns a copy.
    pub fn get_entries(&self) -> Vec<Value> {
        self.file_entries
            .iter()
            .filter(|e| entry_type(e) != "session")
            .cloned()
            .collect()
    }

    pub fn get_tree(&self) -> Vec<SessionTreeNode> {
        let entries = self.get_entries();
        let nodes: Vec<SessionTreeNode> = entries
            .iter()
            .map(|entry| SessionTreeNode {
                entry: entry.clone(),
                children: Vec::new(),
                label: self.labels_by_id.get(entry_id(entry)).cloned(),
                label_timestamp: self.label_timestamps_by_id.get(entry_id(entry)).cloned(),
            })
            .collect();
        let mut index: HashMap<&str, usize> = HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            if let Some(id) = entry.get("id").and_then(Value::as_str) {
                index.insert(id, i);
            }
        }
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); entries.len()];
        let mut roots: Vec<usize> = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            let parent_id = entry.get("parentId");
            let id = entry.get("id").and_then(Value::as_str).unwrap_or_default();
            let is_root = parent_id == Some(&Value::Null) || parent_id.and_then(Value::as_str) == Some(id);
            let parent_idx = parent_id
                .and_then(Value::as_str)
                .and_then(|p| index.get(p).copied());
            match (is_root, parent_idx) {
                (true, _) | (false, None) => roots.push(i),
                (false, Some(p)) => children[p].push(i),
            }
        }
        // Sort children by timestamp (oldest first), iteratively.
        let mut stack: Vec<usize> = roots.clone();
        while let Some(node) = stack.pop() {
            children[node].sort_by(|&a, &b| {
                let ta = entries[a].get("timestamp").and_then(Value::as_str).and_then(parse_date_millis);
                let tb = entries[b].get("timestamp").and_then(Value::as_str).and_then(parse_date_millis);
                match (ta, tb) {
                    (Some(ta), Some(tb)) => ta.cmp(&tb),
                    _ => std::cmp::Ordering::Equal,
                }
            });
            stack.extend(children[node].iter().copied());
        }
        fn assemble(idx: usize, nodes: &mut Vec<Option<SessionTreeNode>>, children: &[Vec<usize>]) -> SessionTreeNode {
            let mut node = nodes[idx].take().expect("node not yet assembled");
            node.children = children[idx]
                .iter()
                .map(|&c| assemble(c, nodes, children))
                .collect();
            node
        }
        let mut slots: Vec<Option<SessionTreeNode>> = nodes.into_iter().map(Some).collect();
        roots
            .into_iter()
            .map(|r| assemble(r, &mut slots, &children))
            .collect()
    }

    // ========================================================================
    // Branching
    // ========================================================================

    pub fn branch(&mut self, branch_from_id: &str) -> Result<(), String> {
        if !self.by_id.has(branch_from_id) {
            return Err(format!("Entry {} not found", branch_from_id));
        }
        self.leaf_id = Some(branch_from_id.to_string());
        Ok(())
    }

    pub fn reset_leaf(&mut self) {
        self.leaf_id = None;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn branch_with_summary(
        &mut self,
        branch_from_id: Option<&str>,
        summary: &str,
        details: Option<Value>,
        from_hook: Option<bool>,
        usage: Option<Value>,
    ) -> Result<String, String> {
        if let Some(id) = branch_from_id {
            if !self.by_id.has(id) {
                return Err(format!("Entry {} not found", id));
            }
        }
        let from_id = self.leaf_id.clone().unwrap_or_else(|| "root".to_string());
        self.leaf_id = branch_from_id.map(str::to_string);
        let id = generate_id(&|k| self.by_id.has(k), &self.gen.uuid_v4.clone());
        let mut map = Map::new();
        map.insert("type".to_string(), json!("branch_summary"));
        map.insert("id".to_string(), json!(id.clone()));
        map.insert(
            "parentId".to_string(),
            branch_from_id.map(|p| json!(p)).unwrap_or(Value::Null),
        );
        map.insert("timestamp".to_string(), json!(self.gen.now_iso()));
        map.insert("fromId".to_string(), json!(from_id));
        map.insert("summary".to_string(), json!(summary));
        if let Some(details) = details {
            map.insert("details".to_string(), details);
        }
        if let Some(usage) = usage {
            map.insert("usage".to_string(), usage);
        }
        if let Some(from_hook) = from_hook {
            map.insert("fromHook".to_string(), json!(from_hook));
        }
        let entry = Value::Object(map);
        self.append_entry(entry);
        Ok(id)
    }

    /// `createBranchedSession(leafId)` — returns the new session file path
    /// (persisting sessions) or `None`.
    pub fn create_branched_session(&mut self, leaf_id: &str) -> Result<Option<String>, String> {
        let previous_session_file = self.session_file.clone();
        let path = self.get_branch(Some(leaf_id));
        if path.is_empty() {
            return Err(format!("Entry {} not found", leaf_id));
        }

        let mut path_without_labels: Vec<Value> = Vec::new();
        let mut replacement_by_label_id: HashMap<String, String> = HashMap::new();
        let mut pending_label_ids: Vec<String> = Vec::new();
        let mut path_parent_id: Option<String> = None;
        for entry in &path {
            if entry_type(entry) == "label" {
                pending_label_ids.push(entry_id(entry).to_string());
                continue;
            }
            for label_id in &pending_label_ids {
                replacement_by_label_id.insert(label_id.clone(), entry_id(entry).to_string());
            }
            pending_label_ids.clear();
            let mut clone = entry.clone();
            if let Some(obj) = clone.as_object_mut() {
                obj.insert(
                    "parentId".to_string(),
                    path_parent_id.clone().map(Value::String).unwrap_or(Value::Null),
                );
                if entry_type(entry) == "compaction" {
                    let first_kept = entry
                        .get("firstKeptEntryId")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let replaced = if first_kept == entry_id(entry) {
                        first_kept.to_string()
                    } else {
                        replacement_by_label_id
                            .get(first_kept)
                            .cloned()
                            .unwrap_or_else(|| first_kept.to_string())
                    };
                    obj.insert("firstKeptEntryId".to_string(), json!(replaced));
                }
            }
            path_without_labels.push(clone);
            path_parent_id = Some(entry_id(entry).to_string());
        }

        let new_session_id = create_session_id(&self.gen);
        let timestamp = self.gen.now_iso();
        let file_timestamp = timestamp.replace([':', '.'], "-");
        let new_session_file = node_join(
            &self.session_dir,
            &format!("{}_{}.jsonl", file_timestamp, new_session_id),
        );

        let mut header = Map::new();
        header.insert("type".to_string(), json!("session"));
        header.insert("version".to_string(), json!(CURRENT_SESSION_VERSION));
        header.insert("id".to_string(), json!(new_session_id.clone()));
        header.insert("timestamp".to_string(), json!(timestamp));
        header.insert("cwd".to_string(), json!(self.cwd.clone()));
        if self.persist {
            if let Some(previous) = &previous_session_file {
                header.insert("parentSession".to_string(), json!(previous));
            }
        }
        let header = Value::Object(header);

        let mut path_entry_ids: Vec<String> = path_without_labels.iter().map(|e| entry_id(e).to_string()).collect();
        let mut labels_to_write: Vec<(String, String, String)> = Vec::new();
        for (target_id, label) in self.labels_by_id.iter() {
            if path_entry_ids.iter().any(|id| id == target_id) {
                labels_to_write.push((
                    target_id.to_string(),
                    label.clone(),
                    self.label_timestamps_by_id
                        .get(target_id)
                        .cloned()
                        .unwrap_or_default(),
                ));
            }
        }

        if self.persist {
            let last_entry_id = path_without_labels.last().map(|e| entry_id(e).to_string());
            let mut parent_id = last_entry_id;
            let mut label_entries: Vec<Value> = Vec::new();
            for (target_id, label, label_timestamp) in &labels_to_write {
                let id = generate_id(
                    &|k| path_entry_ids.iter().any(|e| e == k),
                    &self.gen.uuid_v4.clone(),
                );
                let mut map = entry_base("label", &id, parent_id.as_deref(), label_timestamp);
                map.insert("targetId".to_string(), json!(target_id));
                map.insert("label".to_string(), json!(label));
                path_entry_ids.push(id.clone());
                label_entries.push(Value::Object(map));
                parent_id = Some(id);
            }

            let mut file_entries = vec![header];
            file_entries.extend(path_without_labels.clone());
            file_entries.extend(label_entries);
            self.file_entries = file_entries;
            self.session_id = new_session_id;
            self.session_file = Some(new_session_file.clone());
            self.build_index();

            if self.has_conversation() {
                self.rewrite_file();
                self.flushed = true;
            } else {
                self.flushed = false;
            }
            return Ok(Some(new_session_file));
        }

        // In-memory mode: replace current session with the path + labels.
        let mut label_entries: Vec<Value> = Vec::new();
        let mut parent_id = path_without_labels.last().map(|e| entry_id(e).to_string());
        for (target_id, label, label_timestamp) in &labels_to_write {
            let mut known: Vec<String> = path_entry_ids.clone();
            known.extend(label_entries.iter().map(|e| entry_id(e).to_string()));
            let id = generate_id(&|k| known.iter().any(|e| e == k), &self.gen.uuid_v4.clone());
            let mut map = entry_base("label", &id, parent_id.as_deref(), label_timestamp);
            map.insert("targetId".to_string(), json!(target_id));
            map.insert("label".to_string(), json!(label));
            label_entries.push(Value::Object(map));
            parent_id = Some(id);
        }
        let mut file_entries = vec![header];
        file_entries.extend(path_without_labels);
        file_entries.extend(label_entries);
        self.file_entries = file_entries;
        self.session_id = new_session_id;
        self.build_index();
        Ok(None)
    }
}

fn replace_crlf(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut in_run = false;
    for c in name.chars() {
        if c == '\r' || c == '\n' {
            if !in_run {
                out.push(' ');
            }
            in_run = true;
        } else {
            out.push(c);
            in_run = false;
        }
    }
    js_trim(&out).to_string()
}

// ============================================================================
// Tests — mirror test/session-manager/*.test.ts
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::AgentMessage;
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    // ---- helpers ----------------------------------------------------------

    /// Deterministic generators: `uuid_v4` yields unique 8-char ids,
    /// `uuid_v7` yields unique session ids, `now_ms` increments by 1s from
    /// 2023-11-14T22:13:20.000Z.
    fn det_gen() -> Generators {
        let session_counter = Arc::new(AtomicUsize::new(1));
        let id_counter = Arc::new(AtomicUsize::new(1));
        let now_counter = Arc::new(AtomicI64::new(1_700_000_000_000));
        Generators {
            uuid_v7: Arc::new(move || {
                format!("sess{:04}", session_counter.fetch_add(1, Ordering::SeqCst))
            }),
            uuid_v4: Arc::new(move || {
                format!("id{:06x}", id_counter.fetch_add(1, Ordering::SeqCst))
            }),
            now_ms: Arc::new(move || now_counter.fetch_add(1000, Ordering::SeqCst)),
        }
    }

    fn user_msg(text: &str) -> Value {
        json!({ "role": "user", "content": text, "timestamp": 1700000000000i64 })
    }

    fn assistant_msg(text: &str) -> Value {
        json!({
            "role": "assistant",
            "content": text,
            "timestamp": 1700000000001i64,
            "usage": { "input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0 },
            "cost": { "input": 0.01, "output": 0.02 },
            "stopReason": "stop"
        })
    }

    /// Entry with explicit id/parentId (for in-memory preloads).
    fn msg_entry(id: &str, parent: Option<&str>, role: &str, content: Value) -> Value {
        json!({
            "type": "message",
            "id": id,
            "parentId": parent,
            "timestamp": "2024-01-01T00:00:00.000Z",
            "message": { "role": role, "content": content, "timestamp": 1704067200000i64 }
        })
    }

    fn header_entry(id: &str, version: i64, cwd: &str) -> Value {
        json!({
            "type": "session",
            "version": version,
            "id": id,
            "timestamp": "2024-01-01T00:00:00.000Z",
            "cwd": cwd
        })
    }

    fn ctx_roles(ctx: &SessionContext) -> Vec<String> {
        role_strings(&ctx.messages)
    }

    fn role_strings(messages: &[AgentMessage]) -> Vec<String> {
        messages
            .iter()
            .map(|m| match m {
                AgentMessage::Base(v) => {
                    v.get("role").and_then(Value::as_str).unwrap_or("").to_string()
                }
                AgentMessage::Custom(_) => "custom".to_string(),
                AgentMessage::BranchSummary(_) => "branch_summary".to_string(),
                AgentMessage::CompactionSummary(_) => "compaction_summary".to_string(),
                AgentMessage::BashExecution(_) => "bash_execution".to_string(),
            })
            .collect()
    }

    fn base_content(m: &AgentMessage) -> Value {
        match m {
            AgentMessage::Base(v) => v.get("content").cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }

    fn temp_dir(tag: &str) -> String {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "pi-core-session-manager-{}-{}-{}",
            std::process::id(),
            tag,
            n
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir.to_str().unwrap().to_string()
    }

    fn write_lines(path: &str, lines: &[&str], terminate: bool) {
        use std::io::Write;
        let mut f = std::fs::File::create(path).expect("create file");
        for line in lines {
            writeln!(f, "{}", line).expect("write line");
        }
        if !terminate {
            // Drop the trailing newline from the last line.
            let content = std::fs::read(path).expect("read back");
            let mut trimmed = content;
            while trimmed.last() == Some(&b'\n') {
                trimmed.pop();
            }
            std::fs::write(path, trimmed).expect("strip trailing newline");
        }
    }

    fn read_lines(path: &str) -> Vec<String> {
        std::fs::read_to_string(path)
            .expect("read file")
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn header_line(id: &str, cwd: &str) -> String {
        header_entry(id, 3, cwd).to_string()
    }

    fn entry_types(entries: &[Value]) -> Vec<String> {
        entries.iter().map(|e| entry_type(e).to_string()).collect()
    }

    // ---- assert_valid_session_id (custom-session-id.test.ts) -------------

    #[test]
    fn valid_session_ids_accepted() {
        for id in ["abc", "a-b_c.d1", "A1", "x", "0", "a.b-c_d"] {
            assert_valid_session_id(id).unwrap_or_else(|e| panic!("{}: {}", id, e));
        }
    }

    #[test]
    fn invalid_session_ids_rejected() {
        let expected = "Session id must be non-empty, contain only alphanumeric characters, '-', '_', and '.', and start and end with an alphanumeric character";
        for id in ["", "-abc", "abc-", "ab cd", "a$b", ".abc", "abc.", "a--b "] {
            assert_eq!(assert_valid_session_id(id), Err(expected.to_string()));
        }
    }

    #[test]
    fn create_with_custom_session_id_uses_id_in_header_and_filename() {
        let dir = temp_dir("custom-id");
        let options = NewSessionOptions { id: Some("my.custom-id_1".to_string()), parent_session: None };
        let manager =
            SessionManager::create("/test", Some(&dir), Some(&options), &det_gen()).unwrap();
        assert_eq!(manager.get_session_id(), "my.custom-id_1");
        let file = manager.get_session_file().unwrap();
        assert!(file.ends_with("/2023-11-14T22-13-20-000Z_my.custom-id_1.jsonl"), "{}", file);
        let header = manager.get_header().unwrap();
        assert_eq!(header.get("id").and_then(Value::as_str), Some("my.custom-id_1"));
        assert_eq!(header.get("version").and_then(Value::as_i64), Some(3));
        assert_eq!(header.get("cwd").and_then(Value::as_str), Some("/test"));
    }

    #[test]
    fn create_with_invalid_custom_session_id_errors() {
        let dir = temp_dir("custom-id-bad");
        let options = NewSessionOptions { id: Some("-bad-".to_string()), parent_session: None };
        let err = SessionManager::create("/test", Some(&dir), Some(&options), &det_gen())
            .err()
            .expect("invalid session id must error");
        assert!(err.starts_with("Session id must be non-empty"));
    }

    #[test]
    fn default_session_id_comes_from_uuid_v7() {
        let dir = temp_dir("default-id");
        let manager = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        assert_eq!(manager.get_session_id(), "sess0001");
        assert!(manager.get_session_file().unwrap().ends_with("_sess0001.jsonl"));
    }

    #[test]
    fn new_session_options_parent_session_recorded() {
        let dir = temp_dir("parent-session");
        let options = NewSessionOptions {
            id: None,
            parent_session: Some("/prev/session.jsonl".to_string()),
        };
        let manager =
            SessionManager::create("/test", Some(&dir), Some(&options), &det_gen()).unwrap();
        assert_eq!(
            manager.get_header().unwrap().get("parentSession").and_then(Value::as_str),
            Some("/prev/session.jsonl")
        );
    }

    // ---- in_memory / load_entries (load-entries.test.ts) ------------------

    #[test]
    fn in_memory_adopts_entries_verbatim() {
        let entries = vec![
            msg_entry("e1", None, "user", json!("one")),
            msg_entry("e2", Some("e1"), "assistant", json!("two")),
        ];
        let manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        assert_eq!(manager.get_entry_count(), 2);
        assert_eq!(manager.get_leaf_id(), Some("e2"));
        assert!(manager.get_entry("e1").is_some());
        assert_eq!(ctx_roles(&manager.build_session_context()), ["user", "assistant"]);
    }

    #[test]
    fn in_memory_with_header_uses_header_session_id() {
        let entries = vec![
            header_entry("hdr", 3, "/test"),
            msg_entry("e1", None, "user", json!("one")),
        ];
        let manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        assert_eq!(manager.get_session_id(), "hdr");
        assert_eq!(manager.get_entry_count(), 1);
    }

    #[test]
    fn in_memory_without_header_gets_new_session() {
        let entries = vec![msg_entry("e1", None, "user", json!("one"))];
        let manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        assert_eq!(manager.get_session_id(), "sess0001");
        // Entries are adopted as-is (no migration without a header).
        assert_eq!(manager.get_leaf_id(), Some("e1"));
    }

    #[test]
    fn in_memory_with_empty_entries_starts_empty() {
        let mut manager = SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        assert_eq!(manager.get_entry_count(), 0);
        assert_eq!(manager.get_leaf_id(), None);
        let id = manager.append_message(user_msg("hi"));
        assert_eq!(manager.get_leaf_id(), Some(id.as_str()));
    }

    #[test]
    fn in_memory_stays_off_filesystem() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        assert!(!manager.is_persisted());
        manager.append_message(user_msg("hi"));
        manager.append_message(assistant_msg("hello"));
        assert!(manager.get_session_file().is_none());
    }

    #[test]
    fn in_memory_continues_leaf_from_last_entry() {
        let entries = vec![
            msg_entry("e1", None, "user", json!("one")),
            msg_entry("e2", Some("e1"), "assistant", json!("two")),
        ];
        let mut manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        let id = manager.append_message(user_msg("three"));
        let entry = manager.get_entry(&id).unwrap();
        assert_eq!(entry.get("parentId").and_then(Value::as_str), Some("e2"));
    }

    #[test]
    fn in_memory_avoids_id_collisions_with_preloaded_entries() {
        let entries = vec![msg_entry("id000001", None, "user", json!("one"))];
        let mut manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        let id = manager.append_message(user_msg("two"));
        assert_ne!(id, "id000001");
        assert_eq!(id, "id000002");
    }

    #[test]
    fn in_memory_migrates_v1_entries_on_restore() {
        let v1 = json!({
            "type": "message",
            "timestamp": "2024-01-01T00:00:00.000Z",
            "message": { "role": "user", "content": "one", "timestamp": 1 }
        });
        let entries = vec![header_entry("h1", 1, "/test"), v1];
        let manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        // Header migrated to current version.
        assert_eq!(manager.get_header().unwrap().get("version").and_then(Value::as_i64), Some(3));
        // Entry got an id and a null parent.
        let entries = manager.get_entries();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].get("id").and_then(Value::as_str).is_some_and(|i| !i.is_empty()));
        assert!(entries[0].get("parentId").is_some_and(Value::is_null));
    }

    // ---- migration (migration.test.ts) ------------------------------------

    #[test]
    fn migrate_v1_adds_ids_and_parent_chains() {
        let mut entries = vec![
            header_entry("h1", 1, "/test"),
            json!({ "type": "message", "message": { "role": "user", "content": "one" } }),
            json!({ "type": "message", "message": { "role": "assistant", "content": "two" } }),
        ];
        migrate_session_entries(&mut entries);
        assert_eq!(entries[0].get("version").and_then(Value::as_i64), Some(3));
        let id1 = entries[1].get("id").and_then(Value::as_str).unwrap().to_string();
        let id2 = entries[2].get("id").and_then(Value::as_str).unwrap().to_string();
        assert!(!id1.is_empty() && !id2.is_empty() && id1 != id2);
        assert!(entries[1].get("parentId").is_some_and(Value::is_null));
        assert_eq!(entries[2].get("parentId").and_then(Value::as_str), Some(id1.as_str()));
    }

    #[test]
    fn migrate_v1_compaction_first_kept_index_becomes_id() {
        let mut entries = vec![
            header_entry("h1", 1, "/test"),
            json!({ "type": "message", "message": { "role": "user", "content": "one" } }),
            json!({ "type": "message", "message": { "role": "assistant", "content": "two" } }),
            json!({ "type": "compaction", "summary": "s", "firstKeptEntryIndex": 1, "tokensBefore": 10 }),
        ];
        migrate_session_entries(&mut entries);
        let kept_id = entries[1].get("id").and_then(Value::as_str).unwrap().to_string();
        let compaction = &entries[3];
        assert_eq!(compaction.get("firstKeptEntryId").and_then(Value::as_str), Some(kept_id.as_str()));
        assert!(compaction.get("firstKeptEntryIndex").is_none());
    }

    #[test]
    fn migrate_v2_hook_message_role_becomes_custom() {
        let mut entries = vec![
            header_entry("h1", 2, "/test"),
            json!({
                "type": "message",
                "id": "e1",
                "parentId": null,
                "message": { "role": "hookMessage", "content": "hook!" }
            }),
        ];
        migrate_session_entries(&mut entries);
        assert_eq!(entries[0].get("version").and_then(Value::as_i64), Some(3));
        assert_eq!(
            entries[1]["message"]["role"].as_str(),
            Some("custom"),
            "hookMessage role should be rewritten to custom"
        );
    }

    #[test]
    fn migrate_non_numeric_version_is_no_op() {
        let mut entries = vec![
            json!({ "type": "session", "version": "abc", "id": "h1", "cwd": "/test" }),
            json!({ "type": "message", "message": { "role": "user", "content": "one" } }),
        ];
        let before = entries.clone();
        migrate_session_entries(&mut entries);
        assert_eq!(entries, before, "non-numeric version must not trigger migration");
    }

    #[test]
    fn migrate_current_version_is_no_op() {
        let mut entries = vec![
            header_entry("h1", 3, "/test"),
            msg_entry("e1", None, "user", json!("one")),
        ];
        let before = entries.clone();
        migrate_session_entries(&mut entries);
        assert_eq!(entries, before);
    }

    #[test]
    fn migrate_no_header_treated_as_v1() {
        // TS: `header?.version ?? 1` — no session header means v1, so the
        // v1→v2 step (id/parentId assignment) runs even without a header.
        let mut entries = vec![
            json!({ "type": "message", "message": { "role": "user", "content": "one" } }),
        ];
        migrate_session_entries(&mut entries);
        assert!(entries[0].get("id").and_then(Value::as_str).is_some_and(|i| !i.is_empty()));
        assert!(entries[0].get("parentId").is_some_and(Value::is_null));
    }

    #[test]
    fn migrate_null_or_absent_version_is_v1() {
        for header in [
            json!({ "type": "session", "id": "h1", "cwd": "/test" }),
            json!({ "type": "session", "version": null, "id": "h1", "cwd": "/test" }),
        ] {
            let mut entries = vec![
                header,
                json!({ "type": "message", "message": { "role": "user", "content": "one" } }),
            ];
            migrate_session_entries(&mut entries);
            assert_eq!(entries[0].get("version").and_then(Value::as_i64), Some(3));
            assert!(entries[1].get("id").and_then(Value::as_str).is_some_and(|i| !i.is_empty()));
        }
    }

    #[test]
    fn migrate_numeric_string_version_coerces() {
        // JS relational ops coerce: "2" >= 3 is false, "2" < 2 is false,
        // "2" < 3 is true — so only the v2→v3 step runs (no ids assigned).
        let mut entries = vec![
            json!({ "type": "session", "version": "2", "id": "h1", "cwd": "/test" }),
            json!({ "type": "message", "message": { "role": "hookMessage", "content": "hm" } }),
        ];
        migrate_session_entries(&mut entries);
        assert_eq!(entries[0].get("version").and_then(Value::as_i64), Some(3));
        assert_eq!(entries[1]["message"]["role"].as_str(), Some("custom"));
        assert!(entries[1].get("id").is_none(), "v1→v2 must be skipped for coerced version 2");
    }

    // ---- append operations / projection (build-context.test.ts) -----------

    #[test]
    fn context_messages_for_user_and_assistant() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        manager.append_message(user_msg("one"));
        manager.append_message(assistant_msg("two"));
        assert_eq!(ctx_roles(&manager.build_session_context()), ["user", "assistant"]);
    }

    #[test]
    fn system_message_null_content_normalized_to_empty_string() {
        let entries = vec![msg_entry("e1", None, "system", Value::Null)];
        let manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        let ctx = manager.build_session_context();
        assert_eq!(ctx_roles(&ctx), ["system"]);
        assert_eq!(base_content(&ctx.messages[0]), json!(""));
    }

    #[test]
    fn user_null_content_normalized_to_empty_array() {
        let entries = vec![msg_entry("e1", None, "user", Value::Null)];
        let manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        let ctx = manager.build_session_context();
        assert_eq!(base_content(&ctx.messages[0]), json!([]));
    }

    #[test]
    fn assistant_null_content_normalized_to_empty_array() {
        let entries = vec![msg_entry("e1", None, "assistant", Value::Null)];
        let manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        let ctx = manager.build_session_context();
        assert_eq!(base_content(&ctx.messages[0]), json!([]));
    }

    #[test]
    fn thinking_level_defaults_to_off_and_tracks_changes() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        assert_eq!(manager.build_session_context().thinking_level, "off");
        manager.append_thinking_level_change("high");
        assert_eq!(manager.build_session_context().thinking_level, "high");
        manager.append_thinking_level_change("medium");
        assert_eq!(manager.build_session_context().thinking_level, "medium");
    }

    #[test]
    fn model_change_sets_context_model() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        assert!(manager.build_session_context().model.is_none());
        manager.append_model_change("anthropic", "claude-3");
        let model = manager.build_session_context().model.unwrap();
        assert_eq!(model.provider.as_deref(), Some("anthropic"));
        assert_eq!(model.model_id.as_deref(), Some("claude-3"));
    }

    #[test]
    fn assistant_message_without_model_fields_yields_empty_model() {
        // TS sets `model = { provider: message.provider, modelId: message.model }`
        // for any assistant message; missing fields are undefined and drop out
        // of JSON serialization, leaving `model: {}`.
        let entries = vec![msg_entry("e1", None, "assistant", json!("hi"))];
        let manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        let model = manager.build_session_context().model.unwrap();
        let value = serde_json::to_value(&model).unwrap();
        assert_eq!(value, json!({}));
    }

    #[test]
    fn compaction_keeps_entries_from_first_kept() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        manager.append_message(user_msg("one"));
        let a1 = manager.append_message(assistant_msg("two"));
        manager.append_compaction("summary text", Some(&a1), 100, None, None, None);
        manager.append_message(user_msg("three"));
        assert_eq!(
            ctx_roles(&manager.build_session_context()),
            ["compaction_summary", "assistant", "user"]
        );
    }

    #[test]
    fn compaction_with_self_reference_keeps_only_post_compaction() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        manager.append_message(user_msg("one"));
        let c = manager.append_compaction("summary", None, 50, None, None, None);
        // firstKeptEntryId defaults to the compaction's own id.
        assert_eq!(
            manager.get_entry(&c).unwrap().get("firstKeptEntryId").and_then(Value::as_str),
            Some(c.as_str())
        );
        manager.append_message(user_msg("two"));
        assert_eq!(
            ctx_roles(&manager.build_session_context()),
            ["compaction_summary", "user"]
        );
    }

    #[test]
    fn compaction_captures_system_message() {
        let entries = vec![
            msg_entry("s1", None, "system", json!("You are pi.")),
            msg_entry("u1", Some("s1"), "user", json!("hi")),
        ];
        let mut manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        let c = manager.append_compaction("summary", Some("u1"), 10, None, None, None);
        let compaction = manager.get_entry(&c).unwrap();
        assert!(compaction.get("systemMessage").is_some(), "systemMessage must be captured");
        assert_eq!(
            ctx_roles(&manager.build_session_context()),
            ["system", "compaction_summary", "user"]
        );
    }

    #[test]
    fn branch_limits_context_to_path() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        manager.append_message(assistant_msg("two"));
        manager.branch(&u1).unwrap();
        manager.append_message(user_msg("three"));
        assert_eq!(ctx_roles(&manager.build_session_context()), ["user", "user"]);
    }

    #[test]
    fn branch_errors_on_unknown_entry() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        assert_eq!(manager.branch("nope").unwrap_err(), "Entry nope not found");
    }

    #[test]
    fn branch_with_summary_records_from_id_and_resets_leaf() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let a1 = manager.append_message(assistant_msg("two"));
        let s = manager.branch_with_summary(None, "sum", None, None, None).unwrap();
        let summary = manager.get_entry(&s).unwrap();
        assert_eq!(summary.get("type").and_then(Value::as_str), Some("branch_summary"));
        assert_eq!(summary.get("fromId").and_then(Value::as_str), Some(a1.as_str()));
        assert!(summary.get("parentId").is_some_and(Value::is_null));
        // Leaf now points at the summary entry, which chains to root.
        assert_eq!(manager.get_leaf_id(), Some(s.as_str()));
        assert_eq!(ctx_roles(&manager.build_session_context()), ["branch_summary"]);
        assert_eq!(manager.get_branch(None).len(), 1);
        // Branching from an explicit id records the old leaf as fromId.
        let s2 = manager.branch_with_summary(Some(&u1), "sum2", None, None, None).unwrap();
        assert_eq!(
            manager.get_entry(&s2).unwrap().get("fromId").and_then(Value::as_str),
            Some(s.as_str())
        );
        assert_eq!(
            manager.get_entry(&s2).unwrap().get("parentId").and_then(Value::as_str),
            Some(u1.as_str())
        );
    }

    #[test]
    fn branch_with_summary_errors_on_unknown_entry() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        assert_eq!(
            manager.branch_with_summary(Some("nope"), "s", None, None, None).unwrap_err(),
            "Entry nope not found"
        );
    }

    #[test]
    fn orphaned_entries_are_excluded_from_context() {
        let entries = vec![
            msg_entry("e1", None, "user", json!("one")),
            msg_entry("e2", Some("ghost"), "assistant", json!("two")),
        ];
        let manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        // Walk stops at the orphaned parent: only e2 is reachable.
        assert_eq!(ctx_roles(&manager.build_session_context()), ["assistant"]);
        assert_eq!(manager.get_branch(None).len(), 1);
    }

    #[test]
    fn unknown_leaf_id_falls_back_to_last_entry() {
        let entries = vec![
            msg_entry("e1", None, "user", json!("one")),
            msg_entry("e2", Some("e1"), "assistant", json!("two")),
        ];
        let unknown = build_session_context(&entries, LeafId::Id("missing"), None);
        let last = build_session_context(&entries, LeafId::Unset, None);
        assert_eq!(role_strings(&unknown.messages), role_strings(&last.messages));
    }

    #[test]
    fn root_leaf_id_yields_empty_context() {
        let entries = vec![msg_entry("e1", None, "user", json!("one"))];
        let ctx = build_session_context(&entries, LeafId::Root, None);
        assert!(ctx.messages.is_empty());
    }

    #[test]
    fn reset_leaf_returns_to_last_entry() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        manager.append_message(assistant_msg("two"));
        manager.reset_leaf();
        assert_eq!(manager.get_leaf_id(), None);
        // TS buildSessionPath: leafId === null → empty path → empty context.
        assert!(ctx_roles(&manager.build_session_context()).is_empty());
        manager.branch(&u1).unwrap();
        assert_eq!(manager.get_leaf_id(), Some(u1.as_str()));
    }

    #[test]
    fn custom_message_entry_contributes_custom_message() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        manager.append_message(user_msg("one"));
        let id = manager.append_custom_message_entry("note", json!("a note"), Some(true), None);
        let entry = manager.get_entry(&id).unwrap();
        assert_eq!(entry.get("type").and_then(Value::as_str), Some("custom_message"));
        assert_eq!(entry.get("customType").and_then(Value::as_str), Some("note"));
        assert_eq!(entry.get("display").and_then(Value::as_bool), Some(true));
        assert_eq!(
            ctx_roles(&manager.build_session_context()),
            ["user", "custom"]
        );
    }

    #[test]
    fn custom_entry_contributes_no_messages() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        manager.append_message(user_msg("one"));
        let id = manager.append_custom_entry("progress", Some(json!({ "pct": 50 })));
        let entry = manager.get_entry(&id).unwrap();
        assert_eq!(entry.get("type").and_then(Value::as_str), Some("custom"));
        assert_eq!(entry.get("customType").and_then(Value::as_str), Some("progress"));
        assert_eq!(entry.get("data").and_then(|d| d.get("pct")).and_then(Value::as_i64), Some(50));
        assert_eq!(ctx_roles(&manager.build_session_context()), ["user"]);
    }

    #[test]
    fn usage_entry_shape() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let usage = json!({ "input": 10, "output": 20 });
        let entry = manager.append_usage("turn", "anthropic", "claude-3", usage.clone(), None);
        assert_eq!(entry.get("type").and_then(Value::as_str), Some("usage"));
        assert_eq!(entry.get("kind").and_then(Value::as_str), Some("turn"));
        assert_eq!(entry.get("provider").and_then(Value::as_str), Some("anthropic"));
        assert_eq!(entry.get("model").and_then(Value::as_str), Some("claude-3"));
        assert_eq!(entry.get("usage"), Some(&usage));
        assert!(entry.get("note").is_none(), "empty note must be omitted");
        let entry = manager.append_usage("turn", "anthropic", "claude-3", usage, Some("note!"));
        assert_eq!(entry.get("note").and_then(Value::as_str), Some("note!"));
        // Usage entries contribute no context messages.
        assert!(manager.build_session_context().messages.is_empty());
    }

    #[test]
    fn context_edit_replaces_user_content() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("original"));
        manager
            .append_context_edit(&u1, Some(json!({ "content": "edited" })))
            .unwrap();
        let ctx = manager.build_session_context();
        assert_eq!(ctx_roles(&ctx), ["user"]);
        assert_eq!(base_content(&ctx.messages[0]), json!("edited"));
    }

    #[test]
    fn context_edit_normalizes_string_content_for_assistant() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let a1 = manager.append_message(assistant_msg("original"));
        manager
            .append_context_edit(&a1, Some(json!({ "content": "edited" })))
            .unwrap();
        let ctx = manager.build_session_context();
        assert_eq!(
            base_content(&ctx.messages[0]),
            json!([{ "type": "text", "text": "edited" }])
        );
    }

    #[test]
    fn context_edit_null_replacement_omits_message() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("original"));
        manager.append_message(assistant_msg("keep"));
        manager.append_context_edit(&u1, None).unwrap();
        assert_eq!(ctx_roles(&manager.build_session_context()), ["assistant"]);
    }

    #[test]
    fn context_edit_rejects_invalid_replacement() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("original"));
        let err = manager
            .append_context_edit(&u1, Some(json!({ "content": 42 })))
            .unwrap_err();
        assert_eq!(
            err,
            "Context edit replacement must be null or contain string/array content"
        );
    }

    #[test]
    fn context_edit_rejects_off_branch_target() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let a1 = manager.append_message(assistant_msg("two"));
        manager.branch(&u1).unwrap();
        let err = manager.append_context_edit(&a1, Some(json!({ "content": "x" }))).unwrap_err();
        assert_eq!(err, format!("Entry {} is not on the active branch", a1));
    }

    #[test]
    fn context_edit_rejects_non_editable_target() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        manager.append_message(user_msg("one"));
        let t = manager.append_thinking_level_change("high");
        let err = manager.append_context_edit(&t, Some(json!({ "content": "x" }))).unwrap_err();
        assert_eq!(
            err,
            format!("Entry {} does not contribute editable model content", t)
        );
        let err = manager.append_context_edit("nope", None).unwrap_err();
        assert_eq!(err, "Entry nope not found");
    }

    // ---- labels (labels.test.ts) -------------------------------------------

    #[test]
    fn label_set_and_get() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        manager.append_message(assistant_msg("two"));
        let l = manager.append_label_change(&u1, Some("checkpoint")).unwrap();
        assert_eq!(manager.get_label(&u1), Some("checkpoint"));
        let entry = manager.get_entry(&l).unwrap();
        assert_eq!(entry.get("type").and_then(Value::as_str), Some("label"));
        assert_eq!(entry.get("targetId").and_then(Value::as_str), Some(u1.as_str()));
        assert_eq!(entry.get("label").and_then(Value::as_str), Some("checkpoint"));
    }

    #[test]
    fn label_clear() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        manager.append_label_change(&u1, Some("checkpoint")).unwrap();
        manager.append_label_change(&u1, None).unwrap();
        assert_eq!(manager.get_label(&u1), None);
    }

    #[test]
    fn label_last_write_wins() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        manager.append_label_change(&u1, Some("first")).unwrap();
        manager.append_label_change(&u1, Some("second")).unwrap();
        assert_eq!(manager.get_label(&u1), Some("second"));
    }

    #[test]
    fn label_on_nonexistent_entry_errors() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        assert_eq!(
            manager.append_label_change("nope", Some("x")).unwrap_err(),
            "Entry nope not found"
        );
    }

    #[test]
    fn label_appears_in_tree() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        manager.append_label_change(&u1, Some("mark")).unwrap();
        let tree = manager.get_tree();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].label.as_deref(), Some("mark"));
        assert!(tree[0].label_timestamp.is_some());
    }

    #[test]
    fn labels_do_not_contribute_context_messages() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        manager.append_label_change(&u1, Some("mark")).unwrap();
        assert_eq!(ctx_roles(&manager.build_session_context()), ["user"]);
    }

    #[test]
    fn create_branched_session_preserves_on_path_labels() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let a1 = manager.append_message(assistant_msg("two"));
        manager.append_label_change(&u1, Some("keep-me")).unwrap();
        manager.branch(&u1).unwrap();
        let u2 = manager.append_message(user_msg("three"));
        manager.create_branched_session(&u2).unwrap();
        assert_eq!(manager.get_label(&u1), Some("keep-me"), "on-path label preserved");
        assert!(manager.get_label(&a1).is_none(), "off-path label dropped");
    }

    #[test]
    fn label_rewired_across_branch_boundary() {
        // Label entries are dropped from the copied path; the label survives
        // via re-emitted label entries targeting the path entries.
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let l = manager.append_label_change(&u1, Some("mark")).unwrap();
        let a1 = manager.append_message(assistant_msg("two"));
        manager.create_branched_session(&a1).unwrap();
        // The original label entry is not part of the new session.
        assert!(manager.get_entry(&l).is_none());
        // But the label itself is re-attached to the target.
        assert_eq!(manager.get_label(&u1), Some("mark"));
        let tree = manager.get_tree();
        assert_eq!(tree[0].label.as_deref(), Some("mark"));
    }

    // ---- tree traversal (tree-traversal.test.ts) ---------------------------

    #[test]
    fn appended_entries_chain_parent_ids() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let a1 = manager.append_message(assistant_msg("two"));
        let u2 = manager.append_message(user_msg("three"));
        assert!(manager.get_entry(&u1).unwrap().get("parentId").is_some_and(Value::is_null));
        assert_eq!(
            manager.get_entry(&a1).unwrap().get("parentId").and_then(Value::as_str),
            Some(u1.as_str())
        );
        assert_eq!(
            manager.get_entry(&u2).unwrap().get("parentId").and_then(Value::as_str),
            Some(a1.as_str())
        );
        assert_eq!(manager.get_branch(None).len(), 3);
    }

    #[test]
    fn get_branch_from_explicit_id() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let a1 = manager.append_message(assistant_msg("two"));
        manager.append_message(user_msg("three"));
        let branch = manager.get_branch(Some(&a1));
        assert_eq!(branch.len(), 2);
        assert_eq!(entry_id(&branch[0]), u1);
        assert_eq!(entry_id(&branch[1]), a1);
    }

    #[test]
    fn get_children_after_branching() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let a1 = manager.append_message(assistant_msg("two"));
        manager.branch(&u1).unwrap();
        let u2 = manager.append_message(user_msg("three"));
        let children = manager.get_children(&u1);
        assert_eq!(children.len(), 2);
        let ids: Vec<&str> = children.iter().map(entry_id).collect();
        // Children ordered by timestamp: a1 appended before u2.
        assert_eq!(ids, [a1.as_str(), u2.as_str()]);
        assert!(manager.get_children(&a1).is_empty());
        assert!(manager.get_children("nope").is_empty());
    }

    #[test]
    fn get_tree_linear_chain() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let a1 = manager.append_message(assistant_msg("two"));
        let u2 = manager.append_message(user_msg("three"));
        let tree = manager.get_tree();
        assert_eq!(tree.len(), 1);
        assert_eq!(entry_id(&tree[0].entry), u1);
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(entry_id(&tree[0].children[0].entry), a1);
        assert_eq!(tree[0].children[0].children.len(), 1);
        assert_eq!(entry_id(&tree[0].children[0].children[0].entry), u2);
    }

    #[test]
    fn get_tree_branched() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let a1 = manager.append_message(assistant_msg("two"));
        manager.branch(&u1).unwrap();
        let u2 = manager.append_message(user_msg("three"));
        let tree = manager.get_tree();
        assert_eq!(tree.len(), 1);
        assert_eq!(entry_id(&tree[0].entry), u1);
        assert_eq!(tree[0].children.len(), 2);
        let child_ids: Vec<&str> = tree[0].children.iter().map(|n| entry_id(&n.entry)).collect();
        assert!(child_ids.contains(&a1.as_str()));
        assert!(child_ids.contains(&u2.as_str()));
    }

    #[test]
    fn get_tree_multi_root() {
        let entries = vec![
            msg_entry("e1", None, "user", json!("one")),
            msg_entry("e2", None, "user", json!("two")),
        ];
        let manager = SessionManager::in_memory("/test", None, entries, &det_gen()).unwrap();
        let tree = manager.get_tree();
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn get_leaf_entry_and_get_entry() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let a1 = manager.append_message(assistant_msg("two"));
        assert_eq!(entry_id(manager.get_leaf_entry().unwrap()), a1);
        assert_eq!(entry_id(manager.get_entry(&u1).unwrap()), u1);
        assert!(manager.get_entry("nope").is_none());
    }

    #[test]
    fn create_branched_session_errors_on_nonexistent_leaf() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        manager.append_message(user_msg("one"));
        assert_eq!(
            manager.create_branched_session("nope").unwrap_err(),
            "Entry nope not found"
        );
    }

    #[test]
    fn create_branched_session_extracts_path_in_memory() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let a1 = manager.append_message(assistant_msg("two"));
        manager.branch(&u1).unwrap();
        let u2 = manager.append_message(user_msg("three"));
        let result = manager.create_branched_session(&u2).unwrap();
        assert!(result.is_none(), "in-memory sessions return no file path");
        assert_eq!(ctx_roles(&manager.build_session_context()), ["user", "user"]);
        // New session id and header.
        assert_ne!(manager.get_session_id(), "sess0001");
        assert!(manager.get_entry(&u1).is_some());
        assert!(manager.get_entry(&u2).is_some());
        assert!(manager.get_entry(&a1).is_none(), "off-path entry dropped");
    }

    #[test]
    fn create_branched_session_persists_file_with_path() {
        let dir = temp_dir("branched-persist");
        let mut manager = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let a1 = manager.append_message(assistant_msg("two"));
        manager.branch(&u1).unwrap();
        let u2 = manager.append_message(user_msg("three"));
        let new_file = manager.create_branched_session(&u2).unwrap().unwrap();
        assert!(new_file.starts_with(&dir), "{}", new_file);
        assert!(Path::new(&new_file).exists(), "file written immediately at a user message");
        let lines = read_lines(&new_file);
        assert_eq!(lines.len(), 3, "header + two path entries");
        let entries: Vec<Value> = lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(entry_types(&entries), ["session", "message", "message"]);
        assert_eq!(
            entries[2].get("parentId").and_then(Value::as_str),
            Some(u1.as_str()),
            "path re-chained"
        );
        let _ = a1;
    }

    #[test]
    fn create_branched_session_defers_write_without_conversation() {
        let dir = temp_dir("branched-defer");
        let mut manager = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        let t1 = manager.append_thinking_level_change("high");
        let new_file = manager.create_branched_session(&t1).unwrap().unwrap();
        assert!(!Path::new(&new_file).exists(), "no file until a conversation starts");
    }

    #[test]
    fn usage_entries_roundtrip_through_file() {
        let dir = temp_dir("usage-roundtrip");
        let mut manager = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        manager.append_message(user_msg("one"));
        manager.append_usage("turn", "anthropic", "claude-3", json!({ "input": 1, "output": 1 }), None);
        let leaf = manager.get_leaf_id().unwrap().to_string();
        let new_file = manager.create_branched_session(&leaf).unwrap().unwrap();
        let reloaded = SessionManager::open(&new_file, None, None, &det_gen()).unwrap();
        let all_entries = reloaded.get_entries();
        let usage_entries: Vec<&Value> = all_entries
            .iter()
            .filter(|e| entry_type(e) == "usage")
            .collect();
        assert_eq!(usage_entries.len(), 1);
        assert_eq!(usage_entries[0].get("kind").and_then(Value::as_str), Some("turn"));
    }

    // ---- file operations (file-operations.test.ts, save-entry.test.ts) ----

    #[test]
    fn load_entries_from_nonexistent_file_is_empty() {
        assert!(load_entries_from_file("/nonexistent/dir/session.jsonl").is_empty());
    }

    #[test]
    fn load_entries_from_empty_file_is_empty() {
        let dir = temp_dir("load-empty");
        let file = format!("{}/empty.jsonl", dir);
        std::fs::write(&file, "").unwrap();
        assert!(load_entries_from_file(&file).is_empty());
    }

    #[test]
    fn load_entries_without_header_is_empty_and_file_unmodified() {
        let dir = temp_dir("load-no-header");
        let file = format!("{}/no-header.jsonl", dir);
        let content = format!(
            "{}\n{}\n",
            msg_entry("e1", None, "user", json!("one")),
            msg_entry("e2", Some("e1"), "assistant", json!("two"))
        );
        std::fs::write(&file, &content).unwrap();
        assert!(load_entries_from_file(&file).is_empty());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), content, "file untouched");
    }

    #[test]
    fn load_entries_skips_malformed_lines() {
        let dir = temp_dir("load-malformed");
        let file = format!("{}/mixed.jsonl", dir);
        write_lines(
            &file,
            &[
                &header_line("h1", "/test"),
                "this is not json",
                &msg_entry("e1", None, "user", json!("one")).to_string(),
                "{\"type\":",
                "",
            ],
            true,
        );
        let entries = load_entries_from_file(&file);
        assert_eq!(entry_types(&entries), ["session", "message"]);
    }

    #[test]
    fn load_entries_appends_trailing_newline_to_unterminated_file() {
        let dir = temp_dir("load-unterminated");
        let file = format!("{}/unterminated.jsonl", dir);
        write_lines(
            &file,
            &[&header_line("h1", "/test"), &msg_entry("e1", None, "user", json!("one")).to_string()],
            false,
        );
        let entries = load_entries_from_file(&file);
        assert_eq!(entry_types(&entries), ["session", "message"]);
        assert!(
            std::fs::read_to_string(&file).unwrap().ends_with('\n'),
            "trailing newline appended"
        );
    }

    #[test]
    fn load_entries_tolerates_leading_blank_and_malformed_lines() {
        let dir = temp_dir("load-leading");
        let file = format!("{}/leading.jsonl", dir);
        write_lines(
            &file,
            &["   ", "{oops", &header_line("h1", "/test"), &msg_entry("e1", None, "user", json!("one")).to_string()],
            true,
        );
        let entries = load_entries_from_file(&file);
        assert_eq!(entry_types(&entries), ["session", "message"]);
    }

    #[test]
    fn header_must_have_string_id() {
        let dir = temp_dir("load-header-id");
        let file = format!("{}/bad-header.jsonl", dir);
        write_lines(&file, &[&json!({ "type": "session", "version": 3 }).to_string()], true);
        assert!(load_entries_from_file(&file).is_empty());
    }

    #[test]
    fn find_most_recent_session_empty_dir() {
        let dir = temp_dir("recent-empty");
        assert_eq!(find_most_recent_session(&dir, None), None);
    }

    #[test]
    fn find_most_recent_session_nonexistent_dir() {
        assert_eq!(find_most_recent_session("/nonexistent/dir", None), None);
    }

    #[test]
    fn find_most_recent_session_ignores_non_jsonl() {
        let dir = temp_dir("recent-nonjsonl");
        std::fs::write(format!("{}/notes.txt", dir), header_line("h1", "/test")).unwrap();
        assert_eq!(find_most_recent_session(&dir, None), None);
    }

    #[test]
    fn find_most_recent_session_ignores_invalid_header() {
        let dir = temp_dir("recent-invalid");
        std::fs::write(format!("{}/a.jsonl", dir), "not a session\n").unwrap();
        assert_eq!(find_most_recent_session(&dir, None), None);
    }

    #[test]
    fn find_most_recent_session_single_file() {
        let dir = temp_dir("recent-single");
        let file = format!("{}/a.jsonl", dir);
        std::fs::write(&file, format!("{}\n", header_line("h1", "/test"))).unwrap();
        let found = find_most_recent_session(&dir, None).unwrap();
        assert!(found.ends_with("/a.jsonl"), "{}", found);
    }

    #[test]
    fn find_most_recent_session_picks_most_recent_mtime() {
        let dir = temp_dir("recent-mtime");
        let older = format!("{}/older.jsonl", dir);
        let newer = format!("{}/newer.jsonl", dir);
        std::fs::write(&older, format!("{}\n", header_line("h1", "/test"))).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&newer, format!("{}\n", header_line("h2", "/test"))).unwrap();
        let found = find_most_recent_session(&dir, None).unwrap();
        assert!(found.ends_with("/newer.jsonl"), "{}", found);
    }

    #[test]
    fn find_most_recent_session_skips_invalid_then_valid() {
        let dir = temp_dir("recent-skip-invalid");
        let invalid = format!("{}/a-invalid.jsonl", dir);
        let valid = format!("{}/b-valid.jsonl", dir);
        std::fs::write(&invalid, "garbage\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&valid, format!("{}\n", header_line("h2", "/test"))).unwrap();
        let found = find_most_recent_session(&dir, None).unwrap();
        assert!(found.ends_with("/b-valid.jsonl"), "{}", found);
    }

    #[test]
    fn find_most_recent_session_cwd_filter() {
        let dir = temp_dir("recent-cwd");
        let file = format!("{}/a.jsonl", dir);
        std::fs::write(&file, format!("{}\n", header_line("h1", "/elsewhere"))).unwrap();
        assert_eq!(find_most_recent_session(&dir, Some("/proj")), None);
        assert_eq!(
            find_most_recent_session(&dir, Some("/elsewhere")).unwrap(),
            normalize_path(&file, &path_opts())
        );
    }

    #[test]
    fn set_session_file_empty_file_starts_new_session() {
        let dir = temp_dir("setfile-empty");
        let file = format!("{}/blank.jsonl", dir);
        std::fs::write(&file, "").unwrap();
        let mut manager = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        manager.set_session_file(&file).unwrap();
        assert_eq!(manager.get_session_file(), Some(file.as_str()));
        let lines = read_lines(&file);
        assert_eq!(lines.len(), 1, "only the header is rewritten");
        let header: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(header.get("type").and_then(Value::as_str), Some("session"));
    }

    #[test]
    fn set_session_file_invalid_nonempty_errors() {
        let dir = temp_dir("setfile-invalid");
        let file = format!("{}/corrupt.jsonl", dir);
        std::fs::write(&file, "definitely not a session\n").unwrap();
        let mut manager = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        let err = manager.set_session_file(&file).unwrap_err();
        assert_eq!(err, format!("Session file is not a valid pi session: {}", file));
    }

    #[test]
    fn set_session_file_nonexistent_preserves_explicit_path() {
        let dir = temp_dir("setfile-new");
        let file = format!("{}/fresh.jsonl", dir);
        let mut manager = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        manager.set_session_file(&file).unwrap();
        assert_eq!(manager.get_session_file(), Some(file.as_str()));
        assert!(!Path::new(&file).exists(), "not written until a conversation starts");
        let id = manager.append_message(user_msg("one"));
        assert!(Path::new(&file).exists());
        assert_eq!(read_lines(&file).len(), 2);
        let _ = id;
    }

    #[test]
    fn session_file_not_created_for_setup_only_entries() {
        let dir = temp_dir("lazy-setup");
        let mut manager = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        let file = manager.get_session_file().unwrap().to_string();
        assert!(!Path::new(&file).exists(), "no file right after create");
        manager.append_thinking_level_change("high");
        manager.append_model_change("anthropic", "claude-3");
        manager.append_custom_entry("progress", Some(json!({ "pct": 1 })));
        manager.append_session_info("setup");
        assert!(!Path::new(&file).exists(), "no file before first user/assistant message");
    }

    #[test]
    fn session_file_created_on_first_conversation_message() {
        let dir = temp_dir("lazy-first-message");
        let mut manager = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        let file = manager.get_session_file().unwrap().to_string();
        manager.append_custom_entry("progress", None);
        manager.append_message(user_msg("hello"));
        assert!(Path::new(&file).exists());
        let lines = read_lines(&file);
        assert_eq!(lines.len(), 3, "header + first entry, earlier setup entry included in rewrite");
        let entries: Vec<Value> = lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(entry_types(&entries), ["session", "custom", "message"]);
        // Subsequent appends append single lines.
        manager.append_message(assistant_msg("hi"));
        assert_eq!(read_lines(&file).len(), 4);
    }

    #[test]
    fn open_roundtrips_persisted_session() {
        let dir = temp_dir("open-roundtrip");
        let mut manager = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        // The session file is created lazily on the first conversation message
        // (TS `_persist`), so flush one before reopening.
        manager.append_message(user_msg("hello"));
        let id = manager.get_session_id().to_string();
        let file = manager.get_session_file().unwrap().to_string();
        drop(manager);
        let reopened = SessionManager::open(&file, None, None, &det_gen()).unwrap();
        assert_eq!(reopened.get_session_id(), id);
        assert_eq!(reopened.get_session_file(), Some(file.as_str()));
        assert_eq!(reopened.get_entry_count(), 1);
        // cwd comes from the header.
        assert_eq!(reopened.get_cwd(), "/test");
    }

    #[test]
    fn open_with_entries_and_append() {
        let dir = temp_dir("open-append");
        let mut manager = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        let u1 = manager.append_message(user_msg("one"));
        let file = manager.get_session_file().unwrap().to_string();
        drop(manager);
        let mut reopened = SessionManager::open(&file, None, None, &det_gen()).unwrap();
        assert_eq!(reopened.get_entry_count(), 1);
        let a1 = reopened.append_message(assistant_msg("two"));
        assert_eq!(
            reopened.get_entry(&a1).unwrap().get("parentId").and_then(Value::as_str),
            Some(u1.as_str())
        );
        assert_eq!(read_lines(&file).len(), 3);
    }

    #[test]
    fn session_info_name_sanitized_and_last_wins() {
        let mut manager =
            SessionManager::in_memory("/test", None, Vec::new(), &det_gen()).unwrap();
        assert_eq!(manager.get_session_name(), None);
        manager.append_session_info("first");
        assert_eq!(manager.get_session_name(), Some("first".to_string()));
        manager.append_session_info("line1\nline2\r\nline3");
        assert_eq!(manager.get_session_name(), Some("line1 line2 line3".to_string()));
        manager.append_session_info("   ");
        assert_eq!(manager.get_session_name(), None, "whitespace-only name is ignored");
    }

    #[test]
    fn session_dir_scoping() {
        let dir = temp_dir("scoping");
        let manager = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        assert_eq!(manager.get_session_dir(), dir);
        assert_eq!(manager.get_cwd(), "/test");
        assert!(manager.get_session_file().unwrap().starts_with(&dir));
        assert!(!manager.uses_default_session_dir("/some/agent/dir"));
    }

    #[test]
    fn continue_recent_opens_most_recent_session() {
        let dir = temp_dir("continue-recent");
        let mut first = SessionManager::create("/test", Some(&dir), None, &det_gen()).unwrap();
        let id = first.get_session_id().to_string();
        first.append_message(user_msg("hello"));
        let second = SessionManager::continue_recent("/test", Some(&dir), "/agent", &det_gen()).unwrap();
        assert_eq!(second.get_session_id(), id);
    }

    #[test]
    fn continue_recent_with_no_sessions_starts_new() {
        let dir = temp_dir("continue-new");
        let manager = SessionManager::continue_recent("/test", Some(&dir), "/agent", &det_gen()).unwrap();
        assert_eq!(manager.get_session_id(), "sess0001");
        assert!(manager.get_entry_count() == 0);
    }

    #[test]
    fn fork_from_invalid_source_errors() {
        let dir = temp_dir("fork-invalid");
        let missing = format!("{}/missing.jsonl", dir);
        let err = SessionManager::fork_from(&missing, "/test", Some(&dir), None, &det_gen())
            .err()
            .expect("fork from missing file must error");
        assert_eq!(
            err,
            format!("Cannot fork: source session file is empty or invalid: {}", missing)
        );
    }

    #[test]
    fn fork_from_copies_entries_to_new_session() {
        let source_dir = temp_dir("fork-source");
        let target_dir = temp_dir("fork-target");
        let mut source =
            SessionManager::create("/source", Some(&source_dir), None, &det_gen()).unwrap();
        source.append_message(user_msg("one"));
        source.append_message(assistant_msg("two"));
        let source_file = source.get_session_file().unwrap().to_string();
        drop(source);

        let mut forked =
            SessionManager::fork_from(&source_file, "/target", Some(&target_dir), None, &det_gen())
                .unwrap();
        assert_eq!(forked.get_entry_count(), 2);
        assert_eq!(ctx_roles(&forked.build_session_context()), ["user", "assistant"]);
        assert_eq!(forked.get_cwd(), "/target");
        let new_file = forked.get_session_file().unwrap().to_string();
        assert!(new_file.starts_with(&target_dir), "{}", new_file);
        assert_ne!(new_file, source_file);
        // Continuing the fork appends to the new file.
        forked.append_message(user_msg("three"));
        assert!(Path::new(&new_file).exists());
        assert_eq!(read_lines(&new_file).len(), 4);
    }

    #[test]
    fn find_by_id_locates_session() {
        let dir = temp_dir("find-by-id");
        let options = NewSessionOptions { id: Some("findme1".to_string()), parent_session: None };
        let mut manager =
            SessionManager::create("/test", Some(&dir), Some(&options), &det_gen()).unwrap();
        manager.append_message(user_msg("hello"));
        let found = SessionManager::find_by_id("/test", "findme1", Some(&dir), "/agent").unwrap();
        assert!(found.ends_with("_findme1.jsonl"), "{}", found);
        assert_eq!(SessionManager::find_by_id("/test", "other", Some(&dir), "/agent"), None);
    }

    #[test]
    fn list_returns_session_infos() {
        let dir = temp_dir("list");
        let options = NewSessionOptions { id: Some("listed1".to_string()), parent_session: None };
        let mut manager =
            SessionManager::create("/test", Some(&dir), Some(&options), &det_gen()).unwrap();
        manager.append_message(user_msg("hello there"));
        manager.append_session_info("my session");
        let sessions = SessionManager::list("/test", Some(&dir), "/agent", None);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "listed1");
        assert_eq!(sessions[0].name.as_deref(), Some("my session"));
        assert_eq!(sessions[0].message_count, 1);
        assert_eq!(sessions[0].cwd, "/test");
        assert!(sessions[0].first_message.contains("hello there"));
    }

    // ---- ordered map semantics ---------------------------------------------

    #[test]
    fn ordered_map_reinsert_keeps_position() {
        let mut map: OrderedMap<i32> = OrderedMap::new();
        map.set("a", 1);
        map.set("b", 2);
        map.set("c", 3);
        map.set("a", 10);
        let keys: Vec<&str> = map.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["a", "b", "c"]);
        assert_eq!(map.get("a"), Some(&10));
    }

    #[test]
    fn ordered_map_remove_shifts_later_entries() {
        let mut map: OrderedMap<i32> = OrderedMap::new();
        map.set("a", 1);
        map.set("b", 2);
        map.set("c", 3);
        map.remove("a");
        let keys: Vec<&str> = map.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["b", "c"], "JS Map.delete shifts, unlike swap_remove");
        assert_eq!(map.len(), 2);
    }

    // ---- misc free functions ------------------------------------------------

    #[test]
    fn parse_session_entries_splits_and_skips_blanks() {
        let content = format!(
            "  \n{}\n\n{}\n",
            header_line("h1", "/test"),
            msg_entry("e1", None, "user", json!("one"))
        );
        let entries = parse_session_entries(&content);
        assert_eq!(entry_types(&entries), ["session", "message"]);
        assert!(parse_session_entries("   ").is_empty());
    }

    #[test]
    fn get_latest_compaction_entry_finds_last() {
        let entries = vec![
            msg_entry("e1", None, "user", json!("one")),
            json!({ "type": "compaction", "id": "c1", "parentId": "e1", "summary": "a" }),
            msg_entry("e2", Some("c1"), "user", json!("two")),
            json!({ "type": "compaction", "id": "c2", "parentId": "e2", "summary": "b" }),
        ];
        assert_eq!(
            get_latest_compaction_entry(&entries).and_then(|e| e.get("id")).and_then(Value::as_str),
            Some("c2")
        );
        assert_eq!(
            get_latest_compaction_entry(&entries[..2]).and_then(|e| e.get("id")).and_then(Value::as_str),
            Some("c1")
        );
        assert!(get_latest_compaction_entry(&entries[..1]).is_none());
    }

    #[test]
    fn get_default_session_dir_lives_under_agent_dir() {
        let agent = temp_dir("default-session-dir");
        let dir = get_default_session_dir("/test", &agent);
        assert!(dir.starts_with(&format!("{}/", normalize_path(&agent, &path_opts()))), "{}", dir);
        assert!(dir.contains("sessions"), "{}", dir);
    }

    #[test]
    fn current_version_constant() {
        assert_eq!(CURRENT_SESSION_VERSION, 3);
        assert_eq!(MAX_SESSION_HEADER_SCAN_BYTES, 1024 * 1024);
    }
}

impl Generators {
    /// The uuidv7 generator (shared; the compaction driver draws session
    /// routing ids from the same counter as entry ids).
    pub fn uuid_v7_gen(&self) -> UuidGen {
        Arc::clone(&self.uuid_v7)
    }
}
