//! Adapts MCP tools to pi tool definitions — the pure parts of
//! `src/extensions/mcp/tools.ts` (packages/coding-agent).
//!
//! Results map onto pi's model-facing content (text and images). Text over
//! 20KB keeps its start and end with the middle cut out, like Codex does, and
//! the full text is saved to a temp file the model can read. Binary resources
//! other than images are saved to temp files too, and resource links name the
//! `read_mcp_resource` tool. Codemode scripts receive the whole
//! `CallToolResult` without `_meta` (`content` blocks as sent by the server,
//! `structuredContent`, `isError`), never truncated. MCP errors (`isError`)
//! are error results for the model, but scripts still resolve to the result.
//!
//! TS line → Rust item map:
//! - L44–48  `toToolExposure` → [`to_tool_exposure`]
//! - L51     `MCP_OUTPUT_MAX_BYTES` → const
//! - L57–62  `McpToolDetails` → [`McpToolDetails`]
//! - L68–70  `McpOutputSaver` / `saveToTempFile` → [`McpOutputSaver`] (the
//!   default temp-file saver is I/O and lives behind the caller)
//! - L74–76  `McpToolCaller` → [`McpToolCaller`]
//! - L84–92  `createMcpToolName` → [`create_mcp_tool_name`]
//! - L94–97  `textOf` → `text_of`
//! - L100–112 `CONTENT_BLOCK_SCHEMA` / `toLlmContent` (pi-mcp) → const /
//!   [`to_llm_content`]
//! - L114–160 `relocateSchema` / `createMcpResultSchema` → [`relocate_schema`],
//!   [`create_mcp_result_schema`]
//! - L162–180 `limitMcpContent` → [`limit_mcp_content`]
//! - L182–192 `ConvertMcpResultOptions` → [`ConvertMcpResultOptions`]
//! - L194–204 `extensionOf` / `isTextMimeType` → `extension_of` /
//!   `is_text_mime_type`
//! - L206–222 `blockToContent` → `block_to_content`
//! - L224–230 `toModelContent` → [`to_model_content`]
//! - L232–263 `convertMcpResult` → [`convert_mcp_result`]
//! - L265–277 `toParameters` → [`to_parameters`]
//! - L279–288 `toToolAnnotations` → [`to_tool_annotations`]
//!
//! MCP SDK types (`Tool`, `CallToolResult`, `ContentBlock` from
//! packages/mcp/src/protocol) are modeled as insertion-ordered JSON maps with
//! typed accessors, so `_meta` stripping and key order survive round trips —
//! see the module notes.

use crate::mcp_servers::{parse_url, McpExposure, READ_MCP_RESOURCE_TOOL};
use crate::messages::TextOrImage;
use crate::truncate::truncate_middle;
use serde::Serialize;
use serde_json::{json, Map, Value};

/// `MAX_TOOL_NAME_LENGTH`: provider tool names are limited to 64 characters
/// of `[A-Za-z0-9_-]`.
pub const MAX_TOOL_NAME_LENGTH: usize = 64;
/// Model-facing text of an MCP result beyond this is cut in the middle.
pub const MCP_OUTPUT_MAX_BYTES: u64 = 20 * 1024;

// ============================================================================
// Tool exposure
// ============================================================================

/// `ToolExposure` (core/extensions/types.ts L516): how the model reaches a
/// tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExposure {
    Direct,
    ModelOnly,
    Codemode,
    Deferred,
    Hidden,
}

impl ToolExposure {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolExposure::Direct => "direct",
            ToolExposure::ModelOnly => "model-only",
            ToolExposure::Codemode => "codemode",
            ToolExposure::Deferred => "deferred",
            ToolExposure::Hidden => "hidden",
        }
    }
}

impl Serialize for ToolExposure {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Tool exposure of an MCP exposure (L44): `codemode` and `deferred` both
/// leave tools out of the codemode description; they differ only in which
/// tool the MCP extension activates to reach them.
pub fn to_tool_exposure(exposure: McpExposure) -> ToolExposure {
    match exposure {
        McpExposure::Codemode => ToolExposure::Deferred,
        McpExposure::Deferred => ToolExposure::Deferred,
        McpExposure::Direct => ToolExposure::Direct,
        McpExposure::Hidden => ToolExposure::Hidden,
    }
}

// ============================================================================
// MCP SDK types (packages/mcp/src/protocol)
// ============================================================================

/// `Tool` (protocol/types.ts L76). Kept as the raw JSON object with typed
/// accessors so unknown fields and key order survive.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    pub fields: Map<String, Value>,
}

impl McpTool {
    pub fn name(&self) -> &str {
        self.fields.get("name").and_then(Value::as_str).unwrap_or_default()
    }

    pub fn title(&self) -> Option<&str> {
        self.fields.get("title").and_then(Value::as_str)
    }

    pub fn description(&self) -> Option<&str> {
        self.fields.get("description").and_then(Value::as_str)
    }

    /// `inputSchema: Record<string, unknown>`.
    pub fn input_schema(&self) -> &Value {
        self.fields.get("inputSchema").unwrap_or(&Value::Null)
    }

    /// `outputSchema?: Record<string, unknown>`.
    pub fn output_schema(&self) -> Option<&Value> {
        self.fields.get("outputSchema")
    }

    /// `annotations?: ToolAnnotations`.
    pub fn annotations(&self) -> Option<&Map<String, Value>> {
        self.fields.get("annotations").and_then(Value::as_object)
    }
}

impl From<Map<String, Value>> for McpTool {
    fn from(fields: Map<String, Value>) -> McpTool {
        McpTool { fields }
    }
}

/// One block of `CallToolResult.content` (`ContentBlock`, protocol/content.ts
/// L58). Raw JSON object with typed accessors, like [`McpTool`].
#[derive(Debug, Clone, PartialEq)]
pub struct ContentBlock {
    pub fields: Map<String, Value>,
}

impl ContentBlock {
    /// `type` (never absent in practice; unknown types fall through
    /// `toLlmContent`'s default branch).
    pub fn block_type(&self) -> &str {
        self.fields.get("type").and_then(Value::as_str).unwrap_or_default()
    }

    pub fn text(&self) -> Option<&str> {
        self.fields.get("text").and_then(Value::as_str)
    }

    pub fn data(&self) -> Option<&str> {
        self.fields.get("data").and_then(Value::as_str)
    }

    pub fn mime_type(&self) -> Option<&str> {
        self.fields.get("mimeType").and_then(Value::as_str)
    }

    pub fn uri(&self) -> Option<&str> {
        self.fields.get("uri").and_then(Value::as_str)
    }

    pub fn name(&self) -> Option<&str> {
        self.fields.get("name").and_then(Value::as_str)
    }

    pub fn title(&self) -> Option<&str> {
        self.fields.get("title").and_then(Value::as_str)
    }

    pub fn description(&self) -> Option<&str> {
        self.fields.get("description").and_then(Value::as_str)
    }

    /// `size?: number`.
    pub fn size(&self) -> Option<f64> {
        self.fields.get("size").and_then(Value::as_f64)
    }

    /// `resource?: TextResourceContents | BlobResourceContents`.
    pub fn resource(&self) -> Option<&Map<String, Value>> {
        self.fields.get("resource").and_then(Value::as_object)
    }
}

impl From<Map<String, Value>> for ContentBlock {
    fn from(fields: Map<String, Value>) -> ContentBlock {
        ContentBlock { fields }
    }
}

/// `CallToolResult` (protocol/content.ts L74). Raw JSON object with typed
/// accessors; `script_result` produces the `_meta`-stripped copy scripts see.
#[derive(Debug, Clone, PartialEq)]
pub struct CallToolResult {
    pub fields: Map<String, Value>,
}

impl CallToolResult {
    pub fn content(&self) -> Vec<ContentBlock> {
        match self.fields.get("content") {
            Some(Value::Array(blocks)) => blocks
                .iter()
                .filter_map(|block| block.as_object().cloned())
                .map(ContentBlock::from)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// `structuredContent?: unknown` — `None` only when the key is absent (a
    /// JSON `null` is present and defined).
    pub fn structured_content(&self) -> Option<&Value> {
        self.fields.get("structuredContent")
    }

    pub fn is_error(&self) -> bool {
        self.fields.get("isError").and_then(Value::as_bool).unwrap_or(false)
    }

    /// `{ _meta, ...result }` → `result`: the script-visible structured
    /// result, key order preserved, `_meta` dropped.
    pub fn script_result(&self) -> Value {
        let mut copy = self.fields.clone();
        copy.shift_remove("_meta"); // JS delete preserves order
        Value::Object(copy)
    }
}

impl From<Map<String, Value>> for CallToolResult {
    fn from(fields: Map<String, Value>) -> CallToolResult {
        CallToolResult { fields }
    }
}

/// Model-facing content: pi-ai `TextContent | ImageContent`, reused from
/// `crate::messages` (same serde shape: `{"type":"text","text":…}` /
/// `{"type":"image","data":…,"mimeType":…}`).
pub type LlmContent = TextOrImage;

// ============================================================================
// McpToolCaller seam
// ============================================================================

/// `McpRequestOptions` as far as the pure layer sees it (the abort signal and
/// progress callback are transport concerns).
#[derive(Debug, Clone, Default)]
pub struct McpRequestOptions {
    pub timeout_ms: Option<u64>,
}

/// `McpToolCaller` (L74): the seam the MCP connection implements. Sync per
/// the G8 all-sync decision; the error is a message string.
pub trait McpToolCaller {
    fn call_tool(
        &self,
        name: &str,
        args: &Map<String, Value>,
        options: &McpRequestOptions,
    ) -> Result<CallToolResult, String>;
}

// ============================================================================
// Tool names
// ============================================================================

/// `mcp__<server>__<tool>`, sanitized and shortened with a hash suffix when
/// too long. Like Codex, everything but `[A-Za-z0-9_]` becomes `_`, so the
/// name is also the identifier codemode scripts call it by. `isTaken` reports
/// names used by a different MCP tool: sanitizing can map two tools to one
/// name (`a-b` and `a_b`), which then get the hash suffix. Port of
/// `createMcpToolName` (L84).
///
/// The JS replace/slice run on UTF-16 code units, so a non-BMP character
/// becomes *two* underscores; the sanitizer replicates that.
pub fn create_mcp_tool_name(server: &str, tool: &str, is_taken: impl Fn(&str) -> bool) -> String {
    let mut name = String::new();
    for c in format!("mcp__{}__{}", server, tool).chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            name.push(c);
        } else {
            // One `_` per UTF-16 code unit, like the JS regex replace.
            for _ in 0..c.len_utf16() {
                name.push('_');
            }
        }
    }
    // The name is pure ASCII after sanitizing, so byte length is UTF-16
    // length.
    if name.len() <= MAX_TOOL_NAME_LENGTH && !is_taken(&name) {
        return name;
    }
    let hash = sha256_hex(format!("{}\0{}", server, tool).as_bytes());
    let hash = &hash[..8];
    // JS `String.prototype.slice` clamps out-of-range indices.
    let keep = name.len().min(MAX_TOOL_NAME_LENGTH - hash.len() - 1);
    format!("{}_{}", &name[..keep], hash)
}

// ============================================================================
// Schemas
// ============================================================================

fn text_of(content: &[LlmContent]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            LlmContent::Text { text, .. } => Some(text.as_str()),
            LlmContent::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `CONTENT_BLOCK_SCHEMA` (L100): MCP content blocks as one compact type
/// with the fields of text and image blocks, which scripts read most
/// (`image(block)` takes an image block as is). Rendered into every MCP
/// tool's declaration.
fn content_block_schema() -> Value {
    let mut properties = Map::new();
    properties.insert(
        "type".to_string(),
        json!({ "enum": ["text", "image", "audio", "resource_link", "resource"] }),
    );
    properties.insert("text".to_string(), json!({ "type": "string" }));
    properties.insert("data".to_string(), json!({ "type": "string" }));
    properties.insert("mimeType".to_string(), json!({ "type": "string" }));
    let mut block = Map::new();
    block.insert("type".to_string(), Value::String("object".to_string()));
    block.insert("properties".to_string(), Value::Object(properties));
    block.insert("required".to_string(), Value::Array(vec![Value::String("type".to_string())]));
    block.insert("additionalProperties".to_string(), Value::Bool(true));
    Value::Object(block)
}

/// Keywords whose values are data, so a `$ref` key in them is not a reference.
const SCHEMA_DATA_KEYWORDS: [&str; 4] = ["const", "enum", "default", "examples"];
/// Keywords whose values map names to schemas, so their keys are names, not
/// keywords.
const SCHEMA_MAP_KEYWORDS: [&str; 6] = [
    "properties",
    "patternProperties",
    "dependentSchemas",
    "dependencies",
    "$defs",
    "definitions",
];

/// `schema` as a subschema at the JSON pointer `pointer`
/// (`#/properties/structuredContent`) of another schema: its local references
/// (`#`, `#/$defs/Item`) are rewritten to point below `pointer`, so they
/// resolve to the same subschemas as before. Port of `relocateSchema` (L135).
fn relocate_value(schema: &Value, pointer: &str) -> Value {
    match schema {
        Value::Array(items) => Value::Array(items.iter().map(|item| relocate_value(item, pointer)).collect()),
        Value::Object(map) => {
            let mut relocated = Map::new();
            for (key, value) in map {
                if key == "$ref" {
                    if let Value::String(reference) = value {
                        if reference == "#" || reference.starts_with("#/") {
                            relocated.insert(
                                key.clone(),
                                Value::String(format!("{}{}", pointer, &reference[1..])),
                            );
                            continue;
                        }
                    }
                    relocated.insert(key.clone(), value.clone());
                } else if SCHEMA_DATA_KEYWORDS.contains(&key.as_str()) {
                    relocated.insert(key.clone(), value.clone());
                } else if SCHEMA_MAP_KEYWORDS.contains(&key.as_str()) {
                    match value {
                        Value::Object(entries) => {
                            let mapped: Map<String, Value> = entries
                                .iter()
                                .map(|(name, subschema)| (name.clone(), relocate_value(subschema, pointer)))
                                .collect();
                            relocated.insert(key.clone(), Value::Object(mapped));
                        }
                        _ => {
                            relocated.insert(key.clone(), relocate_value(value, pointer));
                        }
                    }
                } else {
                    relocated.insert(key.clone(), relocate_value(value, pointer));
                }
            }
            Value::Object(relocated)
        }
        other => other.clone(),
    }
}

/// `relocateSchema` — exposed for parity/tests with the TS signature shape.
pub fn relocate_schema(schema: &Value, pointer: &str) -> Value {
    relocate_value(schema, pointer)
}

/// Output schema of every MCP tool: the `CallToolResult` scripts receive
/// (without `_meta`, which [`convert_mcp_result`] drops), with the tool's own
/// output schema as `structuredContent`. Port of `createMcpResultSchema`
/// (L159).
pub fn create_mcp_result_schema(structured_content_schema: Option<&Value>) -> Value {
    let mut properties = Map::new();
    let mut content = Map::new();
    content.insert("type".to_string(), Value::String("array".to_string()));
    content.insert("items".to_string(), content_block_schema());
    properties.insert("content".to_string(), Value::Object(content));
    properties.insert(
        "structuredContent".to_string(),
        // Servers may send structured content without declaring a schema for
        // it, and it may be any JSON value.
        match structured_content_schema {
            Some(schema) => relocate_value(schema, "#/properties/structuredContent"),
            None => Value::Object(Map::new()),
        },
    );
    properties.insert("isError".to_string(), json!({ "type": "boolean" }));
    let mut schema = Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    schema.insert("properties".to_string(), Value::Object(properties));
    schema.insert("required".to_string(), Value::Array(vec![Value::String("content".to_string())]));
    // Fields newer protocol versions add.
    schema.insert("additionalProperties".to_string(), Value::Bool(true));
    Value::Object(schema)
}

/// Tool input schemas must be objects. MCP servers may omit `type`, and some
/// providers reject object schemas without `properties`. Port of
/// `toParameters` (L292): `{...schema, type: schema.type ?? "object",
/// ...(schema.properties === undefined ? {properties: {}} : {})}` — key order
/// preserved, `type`/`properties` appended when absent (a `null` type is
/// replaced in place, per `??`).
pub fn to_parameters(schema: &Value) -> Value {
    let mut map = schema.as_object().cloned().unwrap_or_default();
    let type_is_null = matches!(map.get("type"), Some(Value::Null));
    if !map.contains_key("type") || type_is_null {
        map.insert("type".to_string(), Value::String("object".to_string()));
    }
    if !map.contains_key("properties") {
        map.insert("properties".to_string(), Value::Object(Map::new()));
    }
    Value::Object(map)
}

/// The boolean hints of an MCP tool's annotations (`readOnlyHint`,
/// `destructiveHint`, `idempotentHint`, `openWorldHint`, in that order), or
/// `None` when it has none. Port of `toToolAnnotations` (L279).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct ToolAnnotations {
    #[serde(rename = "readOnlyHint", skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(rename = "destructiveHint", skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(rename = "idempotentHint", skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(rename = "openWorldHint", skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

pub fn to_tool_annotations(tool: &McpTool) -> Option<ToolAnnotations> {
    let annotations = tool.annotations();
    let hint = |key: &str| -> Option<bool> {
        let value = annotations?.get(key)?;
        value.as_bool()
    };
    let result = ToolAnnotations {
        read_only_hint: hint("readOnlyHint"),
        destructive_hint: hint("destructiveHint"),
        idempotent_hint: hint("idempotentHint"),
        open_world_hint: hint("openWorldHint"),
    };
    if result.read_only_hint.is_some()
        || result.destructive_hint.is_some()
        || result.idempotent_hint.is_some()
        || result.open_world_hint.is_some()
    {
        Some(result)
    } else {
        None
    }
}

// ============================================================================
// Output limiting
// ============================================================================

/// Data handed to [`McpOutputSaver`]: the full text of a truncated result, or
/// the decoded bytes of a binary resource. TS: `string | Uint8Array`.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputData {
    Text(String),
    Bytes(Vec<u8>),
}

/// Saves the full text of a truncated result, or a binary resource, and
/// returns the file path. `extension` includes the dot, for example `.txt`.
/// The TS default (`saveToTempFile`) writes a real temp file — I/O this pure
/// module cannot do, so callers (and the parity harness) supply the saver;
/// `Err` carries the error message like `error.message` in the TS catch.
pub type McpOutputSaver<'a> = &'a dyn Fn(OutputData, &str) -> Result<String, String>;

/// The result of [`limit_mcp_content`].
#[derive(Debug, Clone, PartialEq)]
pub struct LimitedMcpContent {
    pub content: Vec<LlmContent>,
    /// Temp file with the full text output, when the text was truncated (and
    /// saving succeeded).
    pub full_output_path: Option<String>,
}

/// Keep model-facing text within [`MCP_OUTPUT_MAX_BYTES`]. Longer text becomes
/// one text block in Codex's truncation format, followed by the path of the
/// file with the full text; images follow it. Port of `limitMcpContent`
/// (L180).
pub fn limit_mcp_content(content: &[LlmContent], save_output: McpOutputSaver) -> LimitedMcpContent {
    let combined = text_of(content);
    let truncation = truncate_middle(&combined, MCP_OUTPUT_MAX_BYTES);
    if !truncation.truncated {
        return LimitedMcpContent { content: content.to_vec(), full_output_path: None };
    }
    let (full_output_path, where_) = match save_output(OutputData::Text(combined), ".txt") {
        Ok(path) => (
            Some(path.clone()),
            format!("[Full output: {} (read it with offset/limit)]", path),
        ),
        Err(message) => (None, format!("[Could not save the full output: {}]", message)),
    };
    let tokens = (truncation.total_bytes as f64 / 4.0).ceil();
    let text = format!(
        "Warning: truncated output (original token count: {})\nTotal output lines: {}\n\n{}\n\n{}",
        js_f64_to_string(tokens),
        truncation.total_lines,
        truncation.content,
        where_
    );
    let mut limited = vec![LlmContent::Text { text, text_signature: None }];
    limited.extend(
        content
            .iter()
            .filter(|block| matches!(block, LlmContent::Image { .. }))
            .cloned(),
    );
    LimitedMcpContent { content: limited, full_output_path }
}

// ============================================================================
// Result conversion
// ============================================================================

/// Options of [`to_model_content`] / [`convert_mcp_result`] (L182). The TS
/// `saveOutput` defaults to a temp-file writer; here the saver is required
/// (see [`McpOutputSaver`]).
pub struct ConvertMcpResultOptions<'a> {
    pub save_output: McpOutputSaver<'a>,
    /// Whether the server's resources can be read with `read_mcp_resource`,
    /// which resource links then name.
    pub readable_resources: bool,
}

impl<'a> ConvertMcpResultOptions<'a> {
    pub fn new(save_output: McpOutputSaver<'a>) -> ConvertMcpResultOptions<'a> {
        ConvertMcpResultOptions { save_output, readable_resources: false }
    }
}

/// File extension for a saved binary resource: the one its URI ends in, else
/// `.bin` (L194). `/\.[A-Za-z0-9]{1,8}$/` on the URI's pathname (or the raw
/// URI when it does not parse).
fn extension_of(uri: &str) -> String {
    let path = match parse_url(uri) {
        Some(url) => url.pathname,
        None => uri.to_string(),
    };
    let bytes = path.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'.' {
            continue;
        }
        let suffix = &path[i + 1..];
        let len = suffix.len();
        if (1..=8).contains(&len) && suffix.bytes().all(|c| c.is_ascii_alphanumeric()) {
            return path[i..].to_string();
        }
    }
    ".bin".to_string()
}

/// Blobs of these types are shown as text (L198).
fn is_text_mime_type(mime_type: Option<&str>) -> bool {
    let Some(mime_type) = mime_type else { return false };
    // mimeType.split(";", 1)[0].trim().toLowerCase()
    let base = mime_type.split(';').next().unwrap_or("").trim().to_lowercase();
    base.starts_with("text/") || base == "application/json" || base.ends_with("+json") || base.ends_with("+xml")
}

/// `formatSize` (core/tools/truncate.ts) for a JS number.
fn format_size_number(bytes: f64) -> String {
    if bytes < 1024.0 {
        format!("{}B", js_f64_to_string(bytes))
    } else if bytes < 1024.0 * 1024.0 {
        format!("{}KB", crate::js_string::js_to_fixed_1(bytes / 1024.0))
    } else {
        format!("{}MB", crate::js_string::js_to_fixed_1(bytes / (1024.0 * 1024.0)))
    }
}

/// `blockToLlmContent` from the MCP SDK (protocol/content.ts L69): one block
/// as text or image content.
fn block_to_llm_content(block: &ContentBlock) -> LlmContent {
    match block.block_type() {
        "text" => LlmContent::Text { text: block.text().unwrap_or_default().to_string(), text_signature: None },
        "image" => LlmContent::Image {
            data: block.data().unwrap_or_default().to_string(),
            mime_type: block.mime_type().unwrap_or_default().to_string(),
        },
        "audio" => LlmContent::Text {
            text: format!("[audio {} omitted]", block.mime_type().unwrap_or("undefined")),
            text_signature: None,
        },
        "resource_link" => LlmContent::Text {
            text: format!("{}: {}", block.name().unwrap_or("undefined"), block.uri().unwrap_or("undefined")),
            text_signature: None,
        },
        "resource" => {
            let resource = block.resource();
            let get = |key: &str| resource.and_then(|r| r.get(key));
            if resource.map(|r| r.contains_key("text")).unwrap_or(false) {
                LlmContent::Text {
                    text: get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
                    text_signature: None,
                }
            } else if block_is_image_resource(resource) {
                LlmContent::Image {
                    data: get("blob").and_then(Value::as_str).unwrap_or_default().to_string(),
                    mime_type: get("mimeType").and_then(Value::as_str).unwrap_or_default().to_string(),
                }
            } else {
                LlmContent::Text {
                    text: format!(
                        "[binary resource {} ({}) omitted]",
                        get("uri").and_then(Value::as_str).unwrap_or("undefined"),
                        get("mimeType").and_then(Value::as_str).unwrap_or("unknown type")
                    ),
                    text_signature: None,
                }
            }
        }
        other => LlmContent::Text {
            text: format!("[unsupported MCP content {}]", other),
            text_signature: None,
        },
    }
}

fn block_is_image_resource(resource: Option<&Map<String, Value>>) -> bool {
    resource
        .and_then(|r| r.get("mimeType"))
        .and_then(Value::as_str)
        .map(|m| m.starts_with("image/"))
        .unwrap_or(false)
}

/// `toLlmContent` from the MCP SDK: a tool result as text and image content
/// for a model. A result without content blocks but with `structuredContent`
/// becomes its JSON.
pub fn to_llm_content(result: &CallToolResult) -> Vec<LlmContent> {
    let mut content: Vec<LlmContent> = result.content().iter().map(block_to_llm_content).collect();
    if content.is_empty() {
        if let Some(structured) = result.structured_content() {
            content.push(LlmContent::Text {
                text: js_stringify_pretty(structured),
                text_signature: None,
            });
        }
    }
    content
}

/// Model-facing content of one block of `server`'s result
/// (`blockToContent`, L206).
/// The right-hand operand of JS `key in value` in V8 error messages:
/// `String(value)` for the non-object shapes (`"blob" in 42`).
fn js_in_operand(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Object(_) => "[object Object]".to_string(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => js_in_operand(other),
            })
            .collect::<Vec<_>>()
            .join(","),
    }
}

fn block_to_content(server: &str, block: &ContentBlock, options: &ConvertMcpResultOptions) -> Result<Vec<LlmContent>, String> {
    if block.block_type() == "resource_link" {
        let mut details: Vec<String> = Vec::new();
        if let Some(mime_type) = block.mime_type() {
            details.push(mime_type.to_string());
        }
        if let Some(size) = block.size() {
            details.push(format_size_number(size));
        }
        let read = if options.readable_resources {
            format!(". Read it with {} (server \"{}\")", READ_MCP_RESOURCE_TOOL, server)
        } else {
            String::new()
        };
        let description = match block.description() {
            Some(d) if !d.is_empty() => format!(": {}", d),
            _ => String::new(),
        };
        return Ok(vec![LlmContent::Text {
            text: format!(
                "[Resource {} \"{}\"{}{}{}]",
                block.uri().unwrap_or("undefined"),
                block.title().or(block.name()).unwrap_or("undefined"),
                if details.is_empty() { String::new() } else { format!(" ({})", details.join(", ")) },
                description,
                read
            ),
            text_signature: None,
        }]);
    }
    if block.block_type() == "resource" {
        // `"blob" in block.resource` throws (V8) for anything but an object —
        // including an absent `resource` field (`in undefined`).
        let resource = match block.fields.get("resource") {
            None => {
                return Err("Cannot use 'in' operator to search for 'blob' in undefined".to_string())
            }
            Some(Value::Object(map)) => map,
            Some(other) => {
                return Err(format!(
                    "Cannot use 'in' operator to search for 'blob' in {}",
                    js_in_operand(other)
                ))
            }
        };
        let has_blob = resource.contains_key("blob");
        let is_image = block_is_image_resource(Some(resource));
        if has_blob && !is_image {
            let get = |key: &str| resource.get(key);
            let uri = get("uri").and_then(Value::as_str).unwrap_or("undefined");
            let mime_type = get("mimeType").and_then(Value::as_str);
            let blob = get("blob").and_then(Value::as_str).unwrap_or_default();
            let data = base64_decode(blob);
            if is_text_mime_type(mime_type) {
                return Ok(vec![LlmContent::Text {
                    text: String::from_utf8_lossy(&data).into_owned(),
                    text_signature: None,
        }]);
            }
            let kind = format!(
                "{}, {}",
                mime_type.unwrap_or("unknown type"),
                format_size_number(data.len() as f64)
            );
            return Ok(match (options.save_output)(OutputData::Bytes(data), &extension_of(uri)) {
                Ok(path) => vec![LlmContent::Text {
                    text: format!("[Binary resource {} ({}) saved to {}]", uri, kind, path),
                    text_signature: None,
                }],
                Err(reason) => vec![LlmContent::Text {
                    text: format!("[Binary resource {} ({}) could not be saved: {}]", uri, kind, reason),
                    text_signature: None,
                }],
            });
        }
    }
    Ok(vec![block_to_llm_content(block)])
}

/// Model-facing content of `server`'s content blocks, before the output limit
/// (`toModelContent`, L224).
pub fn to_model_content(
    server: &str,
    blocks: &[ContentBlock],
    options: &ConvertMcpResultOptions,
) -> Result<Vec<LlmContent>, String> {
    let mut content = Vec::new();
    for block in blocks {
        content.extend(block_to_content(server, block, options)?);
    }
    Ok(content)
}

/// `McpToolDetails` (L57).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct McpToolDetails {
    pub server: String,
    pub tool: String,
    /// Temp file with the full text output, when the model-facing text was
    /// truncated.
    #[serde(rename = "fullOutputPath", skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
}

/// `AgentToolResult` (pi-agent-core types.ts L424) as far as the MCP layer
/// builds it: final or partial result produced by a tool. The `usage` /
/// `prompt` fields exist in the TS but are never set by `convertMcpResult`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentToolResult<D = Value> {
    /// Text or image content returned to the model.
    pub content: Vec<LlmContent>,
    /// Arbitrary structured details for logs or UI rendering.
    pub details: D,
    /// Machine-readable result matching the tool's `outputSchema`, for
    /// programmatic callers. Not sent to the model; `content` remains the
    /// model-facing result.
    #[serde(rename = "structuredContent")]
    pub structured_content: Value,
    /// Report a failure without throwing.
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// Convert an MCP result. `isError` results become error results that keep
/// the structured result. Port of `convertMcpResult` (L232).
pub fn convert_mcp_result(
    server: &str,
    tool: &str,
    result: &CallToolResult,
    options: &ConvertMcpResultOptions,
) -> Result<AgentToolResult<McpToolDetails>, String> {
    // `result.content.length > 0` in the TS throws for protocol-violating
    // results before any conversion runs; `toLlmContent`'s
    // `(result.content ?? []).map` throws for the values that fall through.
    // The messages are V8's, verified against the dist, so tool errors
    // compare equal with the TS.
    match result.fields.get("content") {
        None => {
            return Err("Cannot read properties of undefined (reading 'length')".to_string())
        }
        Some(Value::Null) => {
            return Err("Cannot read properties of null (reading 'length')".to_string())
        }
        Some(Value::Array(_)) => {}
        // A non-empty string has `.length > 0`, so it reaches
        // `toModelContent`, whose `blocks.map` throws.
        Some(Value::String(s)) if !s.is_empty() => {
            return Err("blocks.map is not a function".to_string())
        }
        Some(_) => return Err("(result.content ?? []).map is not a function".to_string()),
    }
    let blocks = result.content();
    // Without content blocks, toLlmContent falls back to the structured
    // content as JSON.
    let mut converted = if !blocks.is_empty() {
        to_model_content(server, &blocks, options)?
    } else {
        to_llm_content(result)
    };
    if result.is_error() && text_of(&converted).is_empty() {
        converted.push(LlmContent::Text {
            text: format!("MCP tool {}/{} returned an error", server, tool),
            text_signature: None,
        });
    }
    let limited = limit_mcp_content(&converted, options.save_output);
    Ok(AgentToolResult {
        content: limited.content,
        details: McpToolDetails {
            server: server.to_string(),
            tool: tool.to_string(),
            full_output_path: limited.full_output_path,
        },
        structured_content: result.script_result(),
        is_error: if result.is_error() { Some(true) } else { None },
    })
}

// ============================================================================
// JS helpers (private)
// ============================================================================

/// JS `Number.prototype.toString()` for a finite f64 (JSON.stringify /
/// template-literal path). Same implementation as `pi-cli::migrations` —
/// duplicated here because pi-core cannot depend on pi-cli; the orchestrator
/// may want to lift it into `js_string.rs`.
fn js_f64_to_string(v: f64) -> String {
    if !v.is_finite() {
        return "null".to_string();
    }
    if v == 0.0 {
        return "0".to_string();
    }
    let a = v.abs();
    if (1e-6..1e21).contains(&a) {
        format!("{}", v)
    } else {
        let mut s = format!("{:e}", v);
        if let Some(pos) = s.find('e') {
            if !s[pos + 1..].starts_with('-') {
                s.insert(pos + 1, '+');
            }
        }
        s
    }
}

/// `JSON.stringify(value, null, 2)` for a parsed-JSON value — byte-identical
/// output for objects, arrays, strings, booleans, null and JS numbers. Same
/// implementation as `pi-cli::migrations::js_stringify_pretty` (duplicated;
/// see [`js_f64_to_string`]).
fn js_stringify_pretty(value: &Value) -> String {
    fn go(value: &Value, depth: usize) -> String {
        let indent = "  ".repeat(depth);
        let inner = "  ".repeat(depth + 1);
        match value {
            Value::Object(map) => {
                if map.is_empty() {
                    return "{}".to_string();
                }
                let mut s = String::from("{\n");
                let n = map.len();
                for (i, (k, v)) in map.iter().enumerate() {
                    s.push_str(&inner);
                    s.push_str(&serde_json::to_string(&Value::String(k.clone())).unwrap());
                    s.push_str(": ");
                    s.push_str(&go(v, depth + 1));
                    if i + 1 < n {
                        s.push(',');
                    }
                    s.push('\n');
                }
                s.push_str(&indent);
                s.push('}');
                s
            }
            Value::Array(items) => {
                if items.is_empty() {
                    return "[]".to_string();
                }
                let mut s = String::from("[\n");
                let n = items.len();
                for (i, v) in items.iter().enumerate() {
                    s.push_str(&inner);
                    s.push_str(&go(v, depth + 1));
                    if i + 1 < n {
                        s.push(',');
                    }
                    s.push('\n');
                }
                s.push_str(&indent);
                s.push(']');
                s
            }
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    i.to_string()
                } else if let Some(u) = n.as_u64() {
                    u.to_string()
                } else {
                    js_f64_to_string(n.as_f64().unwrap_or(0.0))
                }
            }
            other => serde_json::to_string(other).unwrap_or_default(),
        }
    }
    go(value, 0)
}

/// Lenient base64 decode, like `Buffer.from(blob, "base64")`: non-alphabet
/// characters are skipped, padding is optional.
fn base64_decode(input: &str) -> Vec<u8> {
    fn value(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let digits: Vec<u8> = input.bytes().filter_map(value).collect();
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for chunk in digits.chunks(4) {
        match chunk.len() {
            4 => {
                out.push((chunk[0] << 2) | (chunk[1] >> 4));
                out.push((chunk[1] << 4) | (chunk[2] >> 2));
                out.push((chunk[2] << 6) | chunk[3]);
            }
            3 => {
                out.push((chunk[0] << 2) | (chunk[1] >> 4));
                out.push((chunk[1] << 4) | (chunk[2] >> 2));
            }
            2 => {
                out.push((chunk[0] << 2) | (chunk[1] >> 4));
            }
            _ => {}
        }
    }
    out
}

/// SHA-256 (FIPS 180-4), hex-encoded — `createHash("sha256").update(...).
/// digest("hex")`. Implemented here because pi-core's dependencies are fixed.
fn sha256_hex(message: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (message.len() as u64) * 8;
    let mut data = message.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());
    for block in data.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|word| format!("{:08x}", word)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn saver_ok() -> impl Fn(OutputData, &str) -> Result<String, String> {
        |data: OutputData, extension: &str| {
            let _ = data;
            Ok(format!("/tmp/pi-mcp/x{}", extension))
        }
    }

    fn saver_err() -> impl Fn(OutputData, &str) -> Result<String, String> {
        |data: OutputData, _extension: &str| {
            let _ = data;
            Err("disk full".to_string())
        }
    }

    fn text(text: &str) -> LlmContent {
        LlmContent::Text { text: text.to_string(), text_signature: None }
    }

    fn image(data: &str) -> LlmContent {
        LlmContent::Image { data: data.to_string(), mime_type: "image/png".to_string() }
    }

    #[test]
    fn to_tool_exposure_mapping() {
        assert_eq!(to_tool_exposure(McpExposure::Codemode), ToolExposure::Deferred);
        assert_eq!(to_tool_exposure(McpExposure::Deferred), ToolExposure::Deferred);
        assert_eq!(to_tool_exposure(McpExposure::Direct), ToolExposure::Direct);
        assert_eq!(to_tool_exposure(McpExposure::Hidden), ToolExposure::Hidden);
    }

    #[test]
    fn tool_names() {
        // Verified against the TS dist.
        assert_eq!(create_mcp_tool_name("srv", "read_file", |_| false), "mcp__srv__read_file");
        assert_eq!(create_mcp_tool_name("s", "t", |_| true), "mcp__s__t_12dce344");
        // 80-char tool name: 64 chars with an 8-hex suffix.
        let name = create_mcp_tool_name("s", &"x".repeat(80), |_| false);
        assert_eq!(name.len(), 64);
        assert_eq!(name, "mcp__s__xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx_cbf4205d");
        // Non-BMP characters become one underscore per UTF-16 code unit.
        assert_eq!(create_mcp_tool_name("srv", "\u{1F600}tool", |_| false), "mcp__srv____tool");
        // Non-ASCII single-unit characters become one underscore.
        assert_eq!(create_mcp_tool_name("srv", "café", |_| false), "mcp__srv__caf_");
        // A taken name gets the hash suffix; the hash is over server\0tool.
        let taken = create_mcp_tool_name("srv", "read_file", |n| n == "mcp__srv__read_file");
        assert_eq!(taken, "mcp__srv__read_file_145837d4");
    }

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b"s\0t"),
            "12dce344856d5b2d37cc4945201f16fc9512e61d917421d0575f301d68390ca9"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn result_schema_shape_and_refs() {
        // Verified against the TS dist output.
        let schema = create_mcp_result_schema(Some(&json!({
            "type": "object",
            "properties": {"a": {"$ref": "#/$defs/X"}},
            "$defs": {"X": {"type": "string"}}
        })));
        assert_eq!(
            serde_json::to_string(&schema).unwrap(),
            r##"{"type":"object","properties":{"content":{"type":"array","items":{"type":"object","properties":{"type":{"enum":["text","image","audio","resource_link","resource"]},"text":{"type":"string"},"data":{"type":"string"},"mimeType":{"type":"string"}},"required":["type"],"additionalProperties":true}},"structuredContent":{"type":"object","properties":{"a":{"$ref":"#/properties/structuredContent/$defs/X"}},"$defs":{"X":{"type":"string"}}},"isError":{"type":"boolean"}},"required":["content"],"additionalProperties":true}"##
        );
        // No structured schema → `structuredContent: {}`.
        let schema = create_mcp_result_schema(None);
        assert_eq!(
            schema["properties"]["structuredContent"],
            json!({})
        );
    }

    #[test]
    fn relocate_schema_data_and_map_keywords() {
        // $refs under data keywords are left alone.
        let schema = json!({"enum": [{"$ref": "#/x"}], "const": "#/y", "default": {"$ref": "#/z"}});
        assert_eq!(relocate_schema(&schema, "#/p"), schema);
        // Map keywords relocate their values, not their keys.
        let schema = json!({"properties": {"a": {"$ref": "#"}}, "items": {"$ref": "#/$defs/T"}, "$defs": {"T": {}}});
        let relocated = relocate_schema(&schema, "#/p");
        assert_eq!(relocated["properties"]["a"]["$ref"], json!("#/p"));
        assert_eq!(relocated["items"]["$ref"], json!("#/p/$defs/T"));
        // Arrays recurse; non-# refs pass through.
        let schema = json!([{"$ref": "http://x"}, {"$ref": "#/a"}]);
        let relocated = relocate_schema(&schema, "#/p");
        assert_eq!(relocated[0]["$ref"], json!("http://x"));
        assert_eq!(relocated[1]["$ref"], json!("#/p/a"));
    }

    #[test]
    fn parameters_defaults() {
        assert_eq!(
            to_parameters(&json!({})),
            json!({"type": "object", "properties": {}})
        );
        // Existing keys keep their position and value.
        assert_eq!(
            to_parameters(&json!({"properties": {"a": {}}, "type": "object", "extra": 1})),
            json!({"properties": {"a": {}}, "type": "object", "extra": 1})
        );
        // A null type is replaced in place (`??`), null properties kept.
        assert_eq!(
            to_parameters(&json!({"type": null, "properties": null})),
            json!({"type": "object", "properties": null})
        );
        // Non-object schemas spread to nothing.
        assert_eq!(to_parameters(&json!("x")), json!({"type": "object", "properties": {}}));
    }

    #[test]
    fn annotations() {
        let tool = |annotations: Value| McpTool {
            fields: json!({"name": "t", "annotations": annotations}).as_object().unwrap().clone(),
        };
        // Only boolean hints are taken, in hint order.
        let result = to_tool_annotations(&tool(json!({
            "title": "T", "readOnlyHint": true, "openWorldHint": false, "destructiveHint": "yes"
        })))
        .unwrap();
        assert_eq!(serde_json::to_string(&result).unwrap(), r#"{"readOnlyHint":true,"openWorldHint":false}"#);
        // Non-boolean-only annotations → undefined.
        assert!(to_tool_annotations(&tool(json!({"title": "T"}))).is_none());
        assert!(to_tool_annotations(&tool(json!({}))).is_none());
        // No annotations key → undefined.
        assert!(to_tool_annotations(&McpTool { fields: json!({"name": "t"}).as_object().unwrap().clone() }).is_none());
        // All four hints in order.
        let result = to_tool_annotations(&tool(json!({
            "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": true
        })))
        .unwrap();
        assert_eq!(
            serde_json::to_string(&result).unwrap(),
            r#"{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":true,"openWorldHint":true}"#
        );
    }

    #[test]
    fn limit_mcp_content_small() {
        let content = vec![text("hello"), image("zz")];
        let limited = limit_mcp_content(&content, &|_, _| panic!("should not save"));
        assert_eq!(limited.content, content);
        assert_eq!(limited.full_output_path, None);
    }

    #[test]
    fn limit_mcp_content_truncates_and_saves() {
        // Verified against the TS dist.
        let big = "a".repeat(30000);
        let content = vec![text(&big), image("zz")];
        let limited = limit_mcp_content(&content, &|_, ext| {
            assert_eq!(ext, ".txt");
            Ok("/tmp/pi-mcp/x.txt".to_string())
        });
        assert_eq!(limited.content.len(), 2);
        let out = match &limited.content[0] {
            LlmContent::Text { text, .. } => text.clone(),
            _ => panic!("expected text"),
        };
        assert!(out.starts_with("Warning: truncated output (original token count: 7500)\nTotal output lines: 1\n\n"));
        assert!(out.ends_with("\n\n[Full output: /tmp/pi-mcp/x.txt (read it with offset/limit)]"));
        assert!(out.contains(" chars truncated…"));
        assert_eq!(limited.content[1], image("zz"));
        assert_eq!(limited.full_output_path, Some("/tmp/pi-mcp/x.txt".to_string()));
        // The image block comes after the warning text; text blocks are gone.
        assert!(!matches!(limited.content[0], LlmContent::Image { .. }));
    }

    #[test]
    fn limit_mcp_content_save_failure() {
        // Verified against the TS dist.
        let big = "a".repeat(30000);
        let limited = limit_mcp_content(&[text(&big)], &|_, _| Err("disk full".to_string()));
        let out = match &limited.content[0] {
            LlmContent::Text { text, .. } => text.clone(),
            _ => panic!("expected text"),
        };
        assert!(out.starts_with("Warning: truncated output (original token count: 7500)\n"));
        assert!(out.ends_with("\n\n[Could not save the full output: disk full]"));
        assert_eq!(limited.full_output_path, None);
    }

    #[test]
    fn extension_of_uris() {
        assert_eq!(extension_of("file://x/data.bin"), ".bin");
        assert_eq!(extension_of("file://x/a.tar.gz"), ".gz");
        assert_eq!(extension_of("file://x/a."), ".bin");
        assert_eq!(extension_of("file://x/.abcdefghi"), ".bin"); // 9 chars
        assert_eq!(extension_of("file://x/.abcdefg"), ".abcdefg"); // 7 chars
        assert_eq!(extension_of("not a url.txt"), ".txt");
        assert_eq!(extension_of("ui://x/y.png"), ".png");
        assert_eq!(extension_of("noextension"), ".bin");
    }

    #[test]
    fn text_mime_types() {
        assert!(is_text_mime_type(Some("text/plain")));
        assert!(is_text_mime_type(Some("text/plain;charset=utf-8")));
        assert!(is_text_mime_type(Some("APPLICATION/JSON")));
        assert!(is_text_mime_type(Some("application/vnd.api+json")));
        assert!(is_text_mime_type(Some("image/svg+xml")));
        assert!(!is_text_mime_type(Some("image/png")));
        assert!(!is_text_mime_type(Some("application/octet-stream")));
        assert!(!is_text_mime_type(None));
    }

    #[test]
    fn model_content_resource_link() {
        // Verified against the TS dist.
        let block = ContentBlock {
            fields: json!({
                "type": "resource_link",
                "uri": "ui://x/y.png",
                "name": "res",
                "title": "T",
                "description": "d",
                "mimeType": "image/png",
                "size": 2048
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let saver = saver_ok();
        let options = ConvertMcpResultOptions { save_output: &saver, readable_resources: true };
        let content = to_model_content("srv", &[block], &options).unwrap();
        assert_eq!(
            content,
            vec![text("[Resource ui://x/y.png \"T\" (image/png, 2.0KB): d. Read it with read_mcp_resource (server \"srv\")]")]
        );
        // Without readable resources, no read hint; without a title the name
        // is used; falsy (empty) descriptions are dropped.
        let block = ContentBlock {
            fields: json!({"type": "resource_link", "uri": "u", "name": "n", "description": ""})
                .as_object()
                .unwrap()
                .clone(),
        };
        let options = ConvertMcpResultOptions { save_output: &saver, readable_resources: false };
        let content = to_model_content("srv", &[block], &options);
        // `blockToContent` returns exactly one text block for a
        // `resource_link` and never falls through to `toLlmContent`; an
        // empty description is falsy and drops out, and without
        // `readableResources` there is no read hint.
        assert_eq!(content.unwrap(), vec![text("[Resource u \"n\"]")]);
    }

    #[test]
    fn model_content_binary_resource() {
        // Verified against the TS dist (error path).
        let block = ContentBlock {
            fields: json!({
                "type": "resource",
                "resource": {"uri": "file://x/data.bin", "mimeType": "application/octet-stream", "blob": "aGk="}
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let saver = saver_err();
        let options = ConvertMcpResultOptions { save_output: &saver, readable_resources: false };
        let content = to_model_content("srv", &[block], &options).unwrap();
        assert_eq!(
            content,
            vec![text("[Binary resource file://x/data.bin (application/octet-stream, 2B) could not be saved: disk full]")]
        );
        // Success path.
        let block = ContentBlock {
            fields: json!({
                "type": "resource",
                "resource": {"uri": "file://x/data.bin", "mimeType": "application/octet-stream", "blob": "aGk="}
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let saver = saver_ok();
        let options = ConvertMcpResultOptions { save_output: &saver, readable_resources: false };
        let content = to_model_content("srv", &[block], &options).unwrap();
        assert_eq!(
            content,
            vec![text("[Binary resource file://x/data.bin (application/octet-stream, 2B) saved to /tmp/pi-mcp/x.bin]")]
        );
    }

    #[test]
    fn model_content_text_and_image_resources() {
        // Verified against the TS dist.
        let block = ContentBlock {
            fields: json!({
                "type": "resource",
                "resource": {"uri": "file://x/a.txt", "mimeType": "text/plain;charset=utf-8", "blob": "aMOpbGxv"}
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let saver = saver_ok();
        let options = ConvertMcpResultOptions { save_output: &saver, readable_resources: false };
        assert_eq!(to_model_content("srv", &[block], &options).unwrap(), vec![text("héllo")]);
        // Embedded image resources become images.
        let block = ContentBlock {
            fields: json!({
                "type": "resource",
                "resource": {"uri": "file://x/a.png", "mimeType": "image/png", "blob": "QUJD"}
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        assert_eq!(
            to_model_content("srv", &[block], &options).unwrap(),
            vec![LlmContent::Image { data: "QUJD".to_string(), mime_type: "image/png".to_string() }]
        );
    }

    #[test]
    fn llm_content_fallbacks() {
        // toLlmContent: audio, resource links, unsupported types.
        let block = |value: Value| ContentBlock { fields: value.as_object().unwrap().clone() };
        assert_eq!(block_to_llm_content(&block(json!({"type": "audio", "mimeType": "audio/wav"}))), text("[audio audio/wav omitted]"));
        assert_eq!(
            block_to_llm_content(&block(json!({"type": "resource_link", "name": "n", "uri": "u"}))),
            text("n: u")
        );
        assert_eq!(block_to_llm_content(&block(json!({"type": "weird"}))), text("[unsupported MCP content weird]"));
        assert_eq!(block_to_llm_content(&block(json!({"type": "text", "text": "hi"}))), text("hi"));
        // Binary non-image resource without a saver → the omitted placeholder.
        assert_eq!(
            block_to_llm_content(&block(json!({"type": "resource", "resource": {"uri": "u", "blob": "aGk="}}))),
            text("[binary resource u (unknown type) omitted]")
        );
        // toLlmContent on a result without content → JSON.stringify(structuredContent, null, 2).
        let result = CallToolResult { fields: json!({"content": [], "structuredContent": [1, null, "s"]}).as_object().unwrap().clone() };
        assert_eq!(to_llm_content(&result), vec![text("[\n  1,\n  null,\n  \"s\"\n]")]);
        let result = CallToolResult { fields: json!({"content": [], "structuredContent": null}).as_object().unwrap().clone() };
        assert_eq!(to_llm_content(&result), vec![text("null")]);
        // No content and no structuredContent → nothing.
        let result = CallToolResult { fields: json!({"content": []}).as_object().unwrap().clone() };
        assert!(to_llm_content(&result).is_empty());
    }

    #[test]
    fn convert_mcp_result_shapes() {
        // Verified against the TS dist.
        let saver = saver_ok();
        let options = ConvertMcpResultOptions { save_output: &saver, readable_resources: true };
        let result = CallToolResult {
            fields: json!({
                "content": [{"type": "resource_link", "uri": "ui://x/y.png", "name": "res", "title": "T", "description": "d", "mimeType": "image/png", "size": 2048}],
                "structuredContent": {"a": 1},
                "_meta": {"z": 1}
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let converted = convert_mcp_result("srv", "t", &result, &options).unwrap();
        assert_eq!(
            serde_json::to_string(&converted).unwrap(),
            r#"{"content":[{"type":"text","text":"[Resource ui://x/y.png \"T\" (image/png, 2.0KB): d. Read it with read_mcp_resource (server \"srv\")]"}],"details":{"server":"srv","tool":"t"},"structuredContent":{"content":[{"type":"resource_link","uri":"ui://x/y.png","name":"res","title":"T","description":"d","mimeType":"image/png","size":2048}],"structuredContent":{"a":1}}}"#
        );

        // Empty content falls back to the structured content JSON.
        let options = ConvertMcpResultOptions { save_output: &saver, readable_resources: false };
        let result = CallToolResult {
            fields: json!({"content": [], "structuredContent": [1, null, "s"]}).as_object().unwrap().clone(),
        };
        let converted = convert_mcp_result("srv", "t", &result, &options).unwrap();
        assert_eq!(
            serde_json::to_string(&converted).unwrap(),
            r#"{"content":[{"type":"text","text":"[\n  1,\n  null,\n  \"s\"\n]"}],"details":{"server":"srv","tool":"t"},"structuredContent":{"content":[],"structuredContent":[1,null,"s"]}}"#
        );

        // isError results keep the structured result and gain isError: true.
        let result = CallToolResult {
            fields: json!({"content": [{"type": "text", "text": "boom"}], "isError": true}).as_object().unwrap().clone(),
        };
        let converted = convert_mcp_result("srv", "t", &result, &options).unwrap();
        assert_eq!(
            serde_json::to_string(&converted).unwrap(),
            r#"{"content":[{"type":"text","text":"boom"}],"details":{"server":"srv","tool":"t"},"structuredContent":{"content":[{"type":"text","text":"boom"}],"isError":true},"isError":true}"#
        );

        // isError with content that has no text gets the fallback message
        // appended (an *absent* content key throws — see below).
        let result = CallToolResult {
            fields: json!({"content": [], "isError": true}).as_object().unwrap().clone(),
        };
        let converted = convert_mcp_result("srv", "t", &result, &options).unwrap();
        assert_eq!(converted.content, vec![text("MCP tool srv/t returned an error")]);
        assert_eq!(converted.is_error, Some(true));
    }

    #[test]
    fn convert_mcp_result_protocol_violations() {
        // `result.content.length` throws for absent/null content, and the
        // non-array values that pass the check throw in toLlmContent's
        // `(result.content ?? []).map` or toModelContent's `blocks.map`.
        // Messages verified against the TS dist.
        let saver = saver_ok();
        let options = ConvertMcpResultOptions::new(&saver);
        for (fields, message) in [
            (json!({"isError": true, "structuredContent": {}}), "Cannot read properties of undefined (reading 'length')"),
            (json!({"content": null}), "Cannot read properties of null (reading 'length')"),
            (json!({"content": "abc"}), "blocks.map is not a function"),
            (json!({"content": ""}), "(result.content ?? []).map is not a function"),
            (json!({"content": 5}), "(result.content ?? []).map is not a function"),
        ] {
            let result = CallToolResult { fields: fields.as_object().unwrap().clone() };
            assert_eq!(convert_mcp_result("srv", "t", &result, &options), Err(message.to_string()));
        }
    }

    #[test]
    fn convert_mcp_result_truncation_via_details() {
        let big = "b".repeat(25000);
        let result = CallToolResult {
            fields: json!({"content": [{"type": "text", "text": big}]}).as_object().unwrap().clone(),
        };
        let saver = saver_ok();
        let options = ConvertMcpResultOptions { save_output: &saver, readable_resources: false };
        let converted = convert_mcp_result("srv", "t", &result, &options).unwrap();
        assert_eq!(converted.details.full_output_path, Some("/tmp/pi-mcp/x.txt".to_string()));
        assert_eq!(converted.content.len(), 1);
        assert!(matches!(&converted.content[0], LlmContent::Text { text, .. } if text.starts_with("Warning: truncated output (original token count: 6250)")));
        assert_eq!(converted.is_error, None);
    }

    #[test]
    fn base64_decode_is_lenient() {
        assert_eq!(base64_decode("aGk="), b"hi");
        assert_eq!(base64_decode("aGk"), b"hi");
        assert_eq!(base64_decode("a G k ="), b"hi"); // skips whitespace
        assert_eq!(base64_decode("!!!"), b"");
        assert_eq!(base64_decode(""), b"");
        assert_eq!(base64_decode("QUJD"), b"ABC");
        assert_eq!(base64_decode("YUJD"), b"aBC".to_vec());
    }

    #[test]
    fn js_stringify_pretty_matches_js() {
        assert_eq!(js_stringify_pretty(&json!({"a": 1, "b": [1, 2], "c": {"d": null}})), "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2\n  ],\n  \"c\": {\n    \"d\": null\n  }\n}");
        assert_eq!(js_stringify_pretty(&json!({})), "{}");
        assert_eq!(js_stringify_pretty(&json!([])), "[]");
        assert_eq!(js_stringify_pretty(&json!(null)), "null");
        assert_eq!(js_stringify_pretty(&json!(1.5)), "1.5");
        assert_eq!(js_stringify_pretty(&json!(-0.0)), "0");
    }

    #[test]
    fn mcp_tool_accessors() {
        let tool = McpTool {
            fields: json!({
                "name": "t",
                "title": "Title",
                "description": "d",
                "inputSchema": {"type": "object"},
                "outputSchema": {"type": "object"},
                "annotations": {"readOnlyHint": true}
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        assert_eq!(tool.name(), "t");
        assert_eq!(tool.title(), Some("Title"));
        assert_eq!(tool.description(), Some("d"));
        assert_eq!(tool.input_schema(), &json!({"type": "object"}));
        assert_eq!(tool.output_schema(), Some(&json!({"type": "object"})));
        assert!(tool.annotations().is_some());
        // Missing optional fields.
        let tool = McpTool { fields: json!({"name": "t"}).as_object().unwrap().clone() };
        assert_eq!(tool.name(), "t");
        assert_eq!(tool.title(), None);
        assert_eq!(tool.input_schema(), &Value::Null);
        assert_eq!(tool.output_schema(), None);
        assert!(tool.annotations().is_none());
    }

    #[test]
    fn call_tool_result_accessors() {
        let result = CallToolResult {
            fields: json!({
                "content": [{"type": "text", "text": "hi"}, "not-an-object"],
                "structuredContent": {"a": 1},
                "isError": true,
                "_meta": {"z": 1}
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        assert_eq!(result.content().len(), 1); // non-objects are skipped
        assert_eq!(result.structured_content(), Some(&json!({"a": 1})));
        assert!(result.is_error());
        // script_result keeps key order and drops _meta.
        assert_eq!(
            serde_json::to_string(&result.script_result()).unwrap(),
            r#"{"content":[{"type":"text","text":"hi"},"not-an-object"],"structuredContent":{"a":1},"isError":true}"#
        );
        // Defaults.
        let result = CallToolResult { fields: Map::new() };
        assert!(result.content().is_empty());
        assert_eq!(result.structured_content(), None);
        assert!(!result.is_error());
        assert_eq!(result.script_result(), json!({}));
    }

    #[test]
    fn mcp_tool_caller_trait_object() {
        struct FakeCaller;
        impl McpToolCaller for FakeCaller {
            fn call_tool(
                &self,
                name: &str,
                _args: &Map<String, Value>,
                _options: &McpRequestOptions,
            ) -> Result<CallToolResult, String> {
                if name == "boom" {
                    Err("connection closed".to_string())
                } else {
                    Ok(CallToolResult {
                        fields: json!({"content": [{"type": "text", "text": "ok"}]}).as_object().unwrap().clone(),
                    })
                }
            }
        }
        let caller = FakeCaller;
        let result = caller.call_tool("t", &Map::new(), &McpRequestOptions { timeout_ms: Some(1000) }).unwrap();
        assert_eq!(result.content().len(), 1);
        assert_eq!(
            caller.call_tool("boom", &Map::new(), &McpRequestOptions::default()).unwrap_err(),
            "connection closed"
        );
    }
}
