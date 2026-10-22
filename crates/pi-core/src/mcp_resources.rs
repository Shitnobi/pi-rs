//! MCP resources, through the tools Codex and opencode use:
//! `list_mcp_resources`, `list_mcp_resource_templates`, and
//! `read_mcp_resource` — the pure parts of
//! `src/extensions/mcp/resources.ts` (packages/coding-agent).
//!
//! Listings are JSON, as in Codex: `{ server?, resources: [{ server,
//! ...resource }], nextCursor? }`. With a `server`, one page is listed and
//! `cursor` continues it; without, every page of every server. MCP App
//! resources (`ui://` URIs and `profile=mcp-app` HTML) are left out, since
//! they are user interfaces for hosts that render them, and so are icons.
//! Read resources become text and images for the model; binary resources
//! are saved to temp files. Scripts get the JSON payloads.
//!
//! TS line → Rust item map:
//! - L44–46  `isMcpAppResource` → [`is_mcp_app_resource`]
//! - L49–52  `listed` → [`listed`]
//! - L54–133 schema constants → [`create_mcp_resource_tool_definitions`]
//! - L135–142 `stringArgument` → [`string_argument`]
//! - L144–147 `errorMessage` → folded into the server seam (`Err` carries the
//!   JS-visible message)
//! - L149–162 `jsonResult` → `json_result`
//! - L168–237 `createMcpResourceToolDefinitions` / `list` / execute →
//!   [`create_mcp_resource_tool_definitions`], [`McpResourceToolDefinition::execute`]
//!
//! JS-semantics notes:
//! - The all-servers listing sorts with `localeCompare`; see
//!   [`crate::js_string::js_locale_compare`].
//! - `nextCursor` is emitted when it is *anything but* `undefined` — an
//!   explicit JSON `null` survives into the payload. [`ListPage`] models that
//!   with `Option<Value>`.
//! - Payload text is `JSON.stringify` of an insertion-ordered object;
//!   `serde_json` with `preserve_order` reproduces it byte for byte.
//! - The TS module calls `limitMcpContent` / `toModelContent` with their
//!   default temp-file saver; pi-core is pure, so the caller injects the
//!   saver ([`McpResourceToolOptions::save_output`]).

use crate::js_string::{js_locale_compare, js_trim, is_js_whitespace_or_terminator};
use crate::mcp_servers::{
    McpExposure, LIST_MCP_RESOURCE_TEMPLATES_TOOL, LIST_MCP_RESOURCES_TOOL, READ_MCP_RESOURCE_TOOL,
};
use crate::mcp_tools::{
    limit_mcp_content, to_model_content, to_tool_exposure, AgentToolResult, ContentBlock,
    ConvertMcpResultOptions, LlmContent, McpOutputSaver, McpRequestOptions, McpToolDetails,
    ToolAnnotations, ToolExposure,
};
use serde_json::{json, Map, Value};

/// A connected server that offers resources (`McpResourceServer`). Sync per
/// the G8 all-sync decision; `Err` carries the JS-visible error message
/// (`error.message` for `Error` rejections, `String(reason)` otherwise).
pub trait McpResourceServer {
    fn name(&self) -> &str;
    fn timeout_ms(&self) -> u64;
    fn resources_page(&self, cursor: Option<&str>, options: &McpRequestOptions) -> Result<ListPage, String>;
    fn resource_templates_page(
        &self,
        cursor: Option<&str>,
        options: &McpRequestOptions,
    ) -> Result<ListPage, String>;
    fn all_resources(&self, options: &McpRequestOptions) -> Result<Vec<Value>, String>;
    fn all_resource_templates(&self, options: &McpRequestOptions) -> Result<Vec<Value>, String>;
    fn read_resource(&self, uri: &str, options: &McpRequestOptions) -> Result<ReadResourceResult, String>;
}

/// One page of a listing (`ListResourcesResult` /
/// `ListResourceTemplatesResult`): the raw resource/template objects plus
/// the continuation cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct ListPage {
    pub items: Vec<Value>,
    /// `None` = absent (omitted from the payload); `Some(Value::Null)` is an
    /// explicit JSON `null`, which the TS `!== undefined` check still emits.
    pub next_cursor: Option<Value>,
}

/// `ReadResourceResult` as a raw JSON object (`{ contents }`), like
/// [`crate::mcp_tools::CallToolResult`].
#[derive(Debug, Clone, PartialEq)]
pub struct ReadResourceResult {
    pub fields: Map<String, Value>,
}

impl ReadResourceResult {
    pub fn new(contents: Value) -> ReadResourceResult {
        let mut fields = Map::new();
        fields.insert("contents".to_string(), contents);
        ReadResourceResult { fields }
    }
}

/// Options of [`create_mcp_resource_tool_definitions`]. `servers` returns the
/// servers whose resources the tools reach, at call time; `save_output`
/// stands in for the TS default temp-file saver.
pub struct McpResourceToolOptions<'a> {
    pub exposure: McpExposure,
    pub servers: &'a (dyn Fn() -> Vec<&'a dyn McpResourceServer> + 'a),
    pub save_output: McpOutputSaver<'a>,
}

/// MCP App user interfaces, which only hosts that render them can use.
/// Port of `isMcpAppResource` (L44): `ui://` URIs and
/// `/;\s*profile\s*=\s*"?mcp-app"?/i` mime types.
pub fn is_mcp_app_resource(item: &Value) -> bool {
    let object = item.as_object();
    let str_field = |key: &str| object.and_then(|map| map.get(key)).and_then(Value::as_str);
    let uri = str_field("uri").or_else(|| str_field("uriTemplate")).unwrap_or_default();
    if uri.starts_with("ui://") {
        return true;
    }
    let mime_type = str_field("mimeType").unwrap_or_default();
    mime_profile_mcp_app(mime_type)
}

/// `/;\s*profile\s*=\s*"?mcp-app"?/i` — case-insensitive (the pattern is
/// pure ASCII, so ASCII folding suffices for the values this catalog uses),
/// `\s` in ECMAScript WhiteSpace+LineTerminator terms.
fn mime_profile_mcp_app(mime_type: &str) -> bool {
    let chars: Vec<char> = mime_type.chars().map(|c| c.to_ascii_lowercase()).collect();
    let skip_ws = |i: &mut usize| {
        while *i < chars.len() && is_js_whitespace_or_terminator(chars[*i]) {
            *i += 1;
        }
    };
    for (start, &c) in chars.iter().enumerate() {
        if c != ';' {
            continue;
        }
        let mut i = start + 1;
        skip_ws(&mut i);
        if !matches_literal(&chars, i, "profile") {
            continue;
        }
        i += "profile".len();
        skip_ws(&mut i);
        if i >= chars.len() || chars[i] != '=' {
            continue;
        }
        i += 1;
        skip_ws(&mut i);
        if chars.get(i) == Some(&'"') {
            i += 1;
        }
        if !matches_literal(&chars, i, "mcp-app") {
            continue;
        }
        // The trailing `"?` is optional, so nothing more needs matching.
        return true;
    }
    false
}

fn matches_literal(chars: &[char], at: usize, literal: &str) -> bool {
    chars.len() - at >= literal.len()
        && chars[at..at + literal.len()].iter().copied().zip(literal.chars()).all(|(a, b)| a == b)
}

/// A listed resource or template without `_meta` and icons, tagged with its
/// server (`listed`, L49). Like the TS object spread, a later `server` key in
/// the item overrides the tag while keeping the first position.
pub fn listed(server: &str, item: &Value) -> Value {
    let mut out = Map::new();
    out.insert("server".to_string(), Value::String(server.to_string()));
    if let Value::Object(map) = item {
        for (key, value) in map {
            if key != "_meta" && key != "icons" {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(out)
}

/// `stringArgument` (L135): a trimmed non-empty string, `None` for
/// absent/null/blank, `Err` for non-strings.
fn string_argument(params: &Value, key: &str) -> Result<Option<String>, String> {
    // `params?.[key]` — indexing a non-object yields `undefined`, not a throw.
    let Some(value) = params.get(key) else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    let Some(text) = value.as_str() else { return Err(format!("{} must be a string", key)) };
    let trimmed = js_trim(text);
    Ok(if trimmed.is_empty() { None } else { Some(trimmed.to_string()) })
}

/// `jsonResult` (L149): the payload as one text block, limited.
fn json_result(
    tool: &str,
    server: Option<&str>,
    payload: &Value,
    save_output: McpOutputSaver,
) -> Result<AgentToolResult<McpToolDetails>, String> {
    let text = serde_json::to_string(payload).unwrap_or_default();
    let limited = limit_mcp_content(&[LlmContent::Text { text, text_signature: None }], save_output);
    Ok(AgentToolResult {
        content: limited.content,
        details: McpToolDetails {
            server: server.unwrap_or_default().to_string(),
            tool: tool.to_string(),
            full_output_path: limited.full_output_path,
        },
        structured_content: payload.clone(),
        is_error: None,
    })
}

fn find_server<'a>(
    name: &str,
    servers: &[&'a dyn McpResourceServer],
) -> Result<&'a dyn McpResourceServer, String> {
    if let Some(server) = servers.iter().copied().find(|candidate| candidate.name() == name) {
        return Ok(server);
    }
    let available = servers.iter().map(|candidate| candidate.name()).collect::<Vec<_>>().join(", ");
    Err(format!(
        "MCP server \"{}\" has no resources{}",
        name,
        if available.is_empty() { String::new() } else { format!(". Servers with resources: {}", available) }
    ))
}

fn visible(item: &Value) -> bool {
    !is_mcp_app_resource(item)
}

/// Which listing a `list` run produces (drives the payload key and the
/// page/all server methods).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpResourceToolKind {
    ListResources,
    ListResourceTemplates,
    ReadResource,
}

/// One page of one server, or every page of every server (`list`, L186). The
/// payload key is `resources` or `resourceTemplates`.
fn list(params: &Value, kind: McpResourceToolKind, options: &McpResourceToolOptions) -> Result<Value, String> {
    let key = match kind {
        McpResourceToolKind::ListResources => "resources",
        _ => "resourceTemplates",
    };
    let server_name = string_argument(params, "server")?;
    let cursor = string_argument(params, "cursor")?;
    if let Some(name) = server_name.as_deref() {
        let servers = (options.servers)();
        let server = find_server(name, &servers)?;
        let request = McpRequestOptions { timeout_ms: Some(server.timeout_ms()) };
        let result = match kind {
            McpResourceToolKind::ListResources => server.resources_page(cursor.as_deref(), &request),
            _ => server.resource_templates_page(cursor.as_deref(), &request),
        }?;
        let mut payload = Map::new();
        payload.insert("server".to_string(), Value::String(server.name().to_string()));
        let items = result
            .items
            .iter()
            .filter(|item| visible(item))
            .map(|item| listed(server.name(), item))
            .collect::<Vec<_>>();
        payload.insert(key.to_string(), Value::Array(items));
        if let Some(next_cursor) = result.next_cursor {
            payload.insert("nextCursor".to_string(), next_cursor);
        }
        return Ok(Value::Object(payload));
    }
    if cursor.is_some() {
        return Err("cursor can only be used when a server is specified".to_string());
    }
    let mut servers = (options.servers)();
    servers.sort_by(|a, b| js_locale_compare(a.name(), b.name()));
    let mut items: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    for server in &servers {
        let request = McpRequestOptions { timeout_ms: Some(server.timeout_ms()) };
        let result = match kind {
            McpResourceToolKind::ListResources => server.all_resources(&request),
            _ => server.all_resource_templates(&request),
        };
        match result {
            Ok(values) => {
                items.extend(values.iter().filter(|item| visible(item)).map(|item| listed(server.name(), item)))
            }
            Err(message) => errors.push(json!({ "server": server.name(), "error": message })),
        }
    }
    let mut payload = Map::new();
    payload.insert(key.to_string(), Value::Array(items));
    if !errors.is_empty() {
        payload.insert("errors".to_string(), Value::Array(errors));
    }
    Ok(Value::Object(payload))
}

/// `${value}` of a JSON value — the template-literal stringification of the
/// read-content URIs. Objects and arrays stringify like `String(v)` for the
/// shapes this catalog uses (no nested arrays / null elements inside arrays).
fn js_template_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Object(_) => "[object Object]".to_string(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                Value::String(s) => s.clone(),
                other => js_template_string(other),
            })
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// `read_mcp_resource` (readResource execute, L216). Several contents (for
/// example a directory) are labeled with their URIs.
fn read_resource(
    params: &Value,
    options: &McpResourceToolOptions,
) -> Result<AgentToolResult<McpToolDetails>, String> {
    let server_name = string_argument(params, "server")?;
    let uri = string_argument(params, "uri")?;
    let Some(server_name) = server_name else { return Err("server must be provided".to_string()) };
    let Some(uri) = uri else { return Err("uri must be provided".to_string()) };
    let servers = (options.servers)();
    let server = find_server(&server_name, &servers)?;
    let request = McpRequestOptions { timeout_ms: Some(server.timeout_ms()) };
    let result = server.read_resource(&uri, &request)?;
    // `result.contents.flatMap` — V8 error messages for protocol violations.
    let contents = match result.fields.get("contents") {
        None => return Err("Cannot read properties of undefined (reading 'flatMap')".to_string()),
        Some(Value::Null) => return Err("Cannot read properties of null (reading 'flatMap')".to_string()),
        Some(Value::Array(items)) => items.clone(),
        Some(_) => return Err("result.contents.flatMap is not a function".to_string()),
    };
    let mut blocks: Vec<ContentBlock> = Vec::new();
    for content in &contents {
        if contents.len() > 1 {
            let text = content
                .as_object()
                .and_then(|map| map.get("uri"))
                .map(js_template_string)
                .unwrap_or_else(|| "undefined".to_string());
            let mut fields = Map::new();
            fields.insert("type".to_string(), json!("text"));
            fields.insert("text".to_string(), Value::String(format!("{}:", text)));
            blocks.push(ContentBlock::from(fields));
        }
        let mut fields = Map::new();
        fields.insert("type".to_string(), json!("resource"));
        fields.insert("resource".to_string(), content.clone());
        blocks.push(ContentBlock::from(fields));
    }
    let converted = to_model_content(
        server.name(),
        &blocks,
        &ConvertMcpResultOptions { save_output: options.save_output, readable_resources: false },
    )?;
    let model_content = if converted.is_empty() {
        vec![LlmContent::Text { text: format!("Resource {} is empty.", uri), text_signature: None }]
    } else {
        converted
    };
    let limited = limit_mcp_content(&model_content, options.save_output);
    let contents_out = contents
        .iter()
        .map(|content| match content {
            Value::Object(map) => {
                let mut copy = map.clone();
                copy.shift_remove("_meta");
                Value::Object(copy)
            }
            // `({ _meta, ...rest }) => rest` of a non-object yields `{}`.
            _ => Value::Object(Map::new()),
        })
        .collect::<Vec<_>>();
    Ok(AgentToolResult {
        content: limited.content,
        details: McpToolDetails {
            server: server.name().to_string(),
            tool: READ_MCP_RESOURCE_TOOL.to_string(),
            full_output_path: limited.full_output_path,
        },
        structured_content: json!({ "server": server.name(), "uri": uri, "contents": contents_out }),
        is_error: None,
    })
}

/// One of the three resource tools (`ToolDefinition` as far as this module
/// builds it). [`McpResourceToolDefinition::execute`] is the tool's execute.
pub struct McpResourceToolDefinition<'a> {
    pub name: &'static str,
    pub label: String,
    pub description: String,
    pub parameters: Value,
    pub output_schema: Value,
    pub exposure: ToolExposure,
    pub annotations: ToolAnnotations,
    kind: McpResourceToolKind,
    options: &'a McpResourceToolOptions<'a>,
}

impl McpResourceToolDefinition<'_> {
    pub fn kind(&self) -> McpResourceToolKind {
        self.kind
    }

    /// The tool's `execute(_toolCallId, params, signal)`; the abort signal is
    /// a transport concern and stays with the server implementation.
    pub fn execute(&self, _tool_call_id: &str, params: &Value) -> Result<AgentToolResult<McpToolDetails>, String> {
        match self.kind {
            McpResourceToolKind::ListResources => {
                let payload = list(params, McpResourceToolKind::ListResources, self.options)?;
                json_result(
                    LIST_MCP_RESOURCES_TOOL,
                    string_argument(params, "server")?.as_deref(),
                    &payload,
                    self.options.save_output,
                )
            }
            McpResourceToolKind::ListResourceTemplates => {
                let payload = list(params, McpResourceToolKind::ListResourceTemplates, self.options)?;
                json_result(
                    LIST_MCP_RESOURCE_TEMPLATES_TOOL,
                    string_argument(params, "server")?.as_deref(),
                    &payload,
                    self.options.save_output,
                )
            }
            McpResourceToolKind::ReadResource => read_resource(params, self.options),
        }
    }
}

fn string_property(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

/// The three resource tools (`createMcpResourceToolDefinitions`, L168), in
/// the TS order: list resources, list templates, read resource.
pub fn create_mcp_resource_tool_definitions<'a>(
    options: &'a McpResourceToolOptions<'a>,
) -> Vec<McpResourceToolDefinition<'a>> {
    let server_filter =
        string_property("MCP server name. Omit to list every server with resources.");
    let cursor = string_property(
        "Opaque cursor from a previous call with the same server; omit for the first page.",
    );
    let list_parameters = json!({
        "type": "object",
        "properties": { "server": server_filter, "cursor": cursor },
        "additionalProperties": false,
    });
    let read_parameters = json!({
        "type": "object",
        "properties": {
            "server": string_property("MCP server name exactly as configured. Must match the 'server' field returned by list_mcp_resources."),
            "uri": string_property("Resource URI to read. Must be one of the URIs returned by list_mcp_resources."),
        },
        "required": ["server", "uri"],
        "additionalProperties": false,
    });
    let optional_string = || json!({ "type": "string" });
    let listing_errors = || json!({
        "type": "array",
        "description": "Servers that could not be listed",
        "items": {
            "type": "object",
            "properties": { "server": { "type": "string" }, "error": { "type": "string" } },
            "required": ["server", "error"],
        },
    });
    let list_output_schema = json!({
        "type": "object",
        "properties": {
            "server": optional_string(),
            "resources": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "server": { "type": "string" },
                        "uri": { "type": "string" },
                        "name": { "type": "string" },
                        "title": optional_string(),
                        "description": optional_string(),
                        "mimeType": optional_string(),
                        "size": { "type": "number" },
                    },
                    "required": ["server", "uri", "name"],
                },
            },
            "nextCursor": optional_string(),
            "errors": listing_errors(),
        },
        "required": ["resources"],
    });
    let list_templates_output_schema = json!({
        "type": "object",
        "properties": {
            "server": optional_string(),
            "resourceTemplates": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "server": { "type": "string" },
                        "uriTemplate": { "type": "string", "description": "RFC 6570 URI template" },
                        "name": { "type": "string" },
                        "title": optional_string(),
                        "description": optional_string(),
                        "mimeType": optional_string(),
                    },
                    "required": ["server", "uriTemplate", "name"],
                },
            },
            "nextCursor": optional_string(),
            "errors": listing_errors(),
        },
        "required": ["resourceTemplates"],
    });
    let read_output_schema = json!({
        "type": "object",
        "properties": {
            "server": { "type": "string" },
            "uri": { "type": "string" },
            "contents": {
                "type": "array",
                "items": {
                    "anyOf": [
                        {
                            "type": "object",
                            "properties": { "uri": { "type": "string" }, "mimeType": optional_string(), "text": { "type": "string" } },
                            "required": ["uri", "text"],
                        },
                        {
                            "type": "object",
                            "properties": {
                                "uri": { "type": "string" },
                                "mimeType": optional_string(),
                                "blob": { "type": "string", "description": "base64" },
                            },
                            "required": ["uri", "blob"],
                        },
                    ],
                },
            },
        },
        "required": ["server", "uri", "contents"],
    });

    let exposure = to_tool_exposure(options.exposure);
    let read_only = ToolAnnotations { read_only_hint: Some(true), ..ToolAnnotations::default() };
    let make = |name: &'static str,
                description: &str,
                parameters: Value,
                output_schema: Value,
                kind: McpResourceToolKind| McpResourceToolDefinition {
        name,
        label: name.to_string(),
        description: description.to_string(),
        parameters,
        output_schema,
        exposure,
        annotations: read_only,
        kind,
        options,
    };
    vec![
        make(
            LIST_MCP_RESOURCES_TOOL,
            "Lists resources provided by MCP servers. Resources allow servers to share data that provides context to language models, such as files, database schemas, or application-specific information. Prefer resources over web search when possible.",
            list_parameters.clone(),
            list_output_schema,
            McpResourceToolKind::ListResources,
        ),
        make(
            LIST_MCP_RESOURCE_TEMPLATES_TOOL,
            "Lists resource templates provided by MCP servers. Parameterized resource templates allow servers to share data that takes parameters and provides context to language models, such as files, database schemas, or application-specific information. Prefer resource templates over web search when possible.",
            list_parameters,
            list_templates_output_schema,
            McpResourceToolKind::ListResourceTemplates,
        ),
        make(
            READ_MCP_RESOURCE_TOOL,
            "Read a specific resource from an MCP server given the server name and resource URI.",
            read_parameters,
            read_output_schema,
            McpResourceToolKind::ReadResource,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_app_mime_matches() {
        assert!(mime_profile_mcp_app("text/html; profile=mcp-app"));
        assert!(mime_profile_mcp_app("text/html;\tPROFILE = \"mcp-app\""));
        assert!(mime_profile_mcp_app("x;profile=mcp-app\""));
        assert!(mime_profile_mcp_app("x;profile=\"mcp-app"));
        assert!(!mime_profile_mcp_app("text/html; profile=web-app"));
        assert!(!mime_profile_mcp_app("text/html;profileX=mcp-app"));
        assert!(!mime_profile_mcp_app("profile=mcp-app"));
    }

    #[test]
    fn listed_strips_meta_and_icons() {
        let item = json!({ "uri": "u", "name": "n", "icons": [1], "_meta": { "a": 1 } });
        assert_eq!(listed("srv", &item), json!({ "server": "srv", "uri": "u", "name": "n" }));
        // A `server` key in the item overrides the tag, keeping its position.
        let override_item = json!({ "server": "other", "uri": "u" });
        assert_eq!(listed("srv", &override_item), json!({ "server": "other", "uri": "u" }));
    }
}
