//! Parity-harness entry for the `mcp-resources` protocol:
//! `pi_core::mcp_resources` (TS `extensions/mcp/resources.ts`) — the three
//! resource tools over scripted fake servers. TS mirror: parity/ts-harness.js
//! `mcp-resources` entry (same fake-server scripting, same call log).
//!
//! args:
//! ```json
//! {
//!   "fn": "definitions" | "execute",
//!   "exposure": "codemode" | "deferred" | "direct" | "hidden",
//!   // execute only:
//!   "tool": "list_mcp_resources" | "list_mcp_resource_templates"
//!         | "read_mcp_resource",
//!   "params": { "server"?, "cursor"?, "uri"? },
//!   "servers": [
//!     {
//!       "name": str, "timeoutMs"?: number (default 10000),
//!       "resourcesPages": [ { "resources": [...], "nextCursor"?: str|null }
//!                         | { "error": str } | { "reject": value } ],
//!       "templatesPages": [ { "resourceTemplates": [...], "nextCursor"? }
//!                          | { "error" } | { "reject" } ],
//!       "allResources":    { "items": [...] } | { "error" } | { "reject" },
//!       "allResourceTemplates": same,
//!       "readResource":    { "contents": [...] } | { "error" } | { "reject" }
//!     }
//!   ]
//! }
//! ```
//!
//! Page entries are consumed one per call; an exhausted (or unscripted) page
//! queue fails with `<method> not scripted for server "<name>"` on both
//! sides. `error` rejects with an `Error` (its message), `reject` with the
//! raw value (`String(value)` semantics). Absent `all*` / `readResource`
//! specs resolve empty. Result: `{ result, calls }` or `{ error, calls }`,
//! where `calls` is the shared, insertion-ordered log of server-method
//! invocations (`{ server, method, cursor?/uri?, timeoutMs }`).
//!
//! The output saver is a deterministic fake on both sides — the TS module's
//! default temp-file saver is real I/O, so the catalog never triggers it
//! (no >20KB listings, no non-image binary reads).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use serde_json::{json, Map, Value};

use pi_core::mcp_resources::{
    create_mcp_resource_tool_definitions, ListPage, McpResourceServer, McpResourceToolOptions,
    ReadResourceResult,
};
use pi_core::mcp_servers::McpExposure;
use pi_core::mcp_tools::{McpRequestOptions, OutputData};

/// JS `String(value)` for the rejection values this catalog uses.
fn js_string_value(value: &Value) -> String {
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
                other => js_string_value(other),
            })
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// A scripted fake server (TS mirror: `mcrFakeServer` in ts-harness.js).
struct ScriptedServer {
    name: String,
    timeout_ms: u64,
    resources_pages: RefCell<VecDeque<Value>>,
    templates_pages: RefCell<VecDeque<Value>>,
    all_resources: Option<Value>,
    all_templates: Option<Value>,
    read: Option<Value>,
    log: Rc<RefCell<Vec<Value>>>,
}

impl ScriptedServer {
    fn new(spec: &Value, log: &Rc<RefCell<Vec<Value>>>) -> Result<ScriptedServer, String> {
        let name = spec["name"].as_str().ok_or("missing server name")?.to_string();
        let timeout_ms = spec["timeoutMs"].as_u64().unwrap_or(10000);
        let queue = |key: &str| {
            spec[key]
                .as_array()
                .cloned()
                .map(VecDeque::from)
                .unwrap_or_default()
        };
        Ok(ScriptedServer {
            name,
            timeout_ms,
            resources_pages: RefCell::new(queue("resourcesPages")),
            templates_pages: RefCell::new(queue("templatesPages")),
            all_resources: spec.get("allResources").cloned(),
            all_templates: spec.get("allResourceTemplates").cloned(),
            read: spec.get("readResource").cloned(),
            log: Rc::clone(log),
        })
    }

    fn record(&self, method: &str, extra: Option<(&str, &str)>) {
        let mut entry = Map::new();
        entry.insert("server".to_string(), json!(self.name));
        entry.insert("method".to_string(), json!(method));
        if let Some((key, value)) = extra {
            entry.insert(key.to_string(), json!(value));
        }
        entry.insert("timeoutMs".to_string(), json!(self.timeout_ms));
        self.log.borrow_mut().push(Value::Object(entry));
    }

    fn take_page(&self, method: &str, items_key: &str, queue: &mut VecDeque<Value>) -> Result<ListPage, String> {
        let entry = queue
            .pop_front()
            .ok_or_else(|| format!("{} not scripted for server \"{}\"", method, self.name))?;
        if let Some(error) = entry.get("error") {
            return Err(error.as_str().map(str::to_string).unwrap_or_else(|| js_string_value(error)));
        }
        if let Some(reject) = entry.get("reject") {
            return Err(js_string_value(reject));
        }
        let items = entry.get(items_key).and_then(Value::as_array).cloned().unwrap_or_default();
        let next_cursor = entry.get("nextCursor").cloned();
        Ok(ListPage { items, next_cursor })
    }

    fn apply_outcome(&self, outcome: &Option<Value>) -> Result<Vec<Value>, String> {
        let Some(spec) = outcome else { return Ok(Vec::new()) };
        if let Some(error) = spec.get("error") {
            return Err(error.as_str().map(str::to_string).unwrap_or_else(|| js_string_value(error)));
        }
        if let Some(reject) = spec.get("reject") {
            return Err(js_string_value(reject));
        }
        Ok(spec.get("items").and_then(Value::as_array).cloned().unwrap_or_default())
    }
}

impl McpResourceServer for ScriptedServer {
    fn name(&self) -> &str {
        &self.name
    }

    fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    fn resources_page(&self, cursor: Option<&str>, _options: &McpRequestOptions) -> Result<ListPage, String> {
        self.record("resourcesPage", cursor.map(|cursor| ("cursor", cursor)));
        let mut queue = self.resources_pages.borrow_mut();
        self.take_page("resourcesPage", "resources", &mut queue)
    }

    fn resource_templates_page(
        &self,
        cursor: Option<&str>,
        _options: &McpRequestOptions,
    ) -> Result<ListPage, String> {
        self.record("resourceTemplatesPage", cursor.map(|cursor| ("cursor", cursor)));
        let mut queue = self.templates_pages.borrow_mut();
        self.take_page("resourceTemplatesPage", "resourceTemplates", &mut queue)
    }

    fn all_resources(&self, _options: &McpRequestOptions) -> Result<Vec<Value>, String> {
        self.record("allResources", None);
        self.apply_outcome(&self.all_resources)
    }

    fn all_resource_templates(&self, _options: &McpRequestOptions) -> Result<Vec<Value>, String> {
        self.record("allResourceTemplates", None);
        self.apply_outcome(&self.all_templates)
    }

    fn read_resource(&self, uri: &str, _options: &McpRequestOptions) -> Result<ReadResourceResult, String> {
        self.record("readResource", Some(("uri", uri)));
        let Some(spec) = self.read.as_ref() else {
            return Ok(ReadResourceResult::new(json!([])));
        };
        if let Some(error) = spec.get("error") {
            return Err(error.as_str().map(str::to_string).unwrap_or_else(|| js_string_value(error)));
        }
        if let Some(reject) = spec.get("reject") {
            return Err(js_string_value(reject));
        }
        Ok(ReadResourceResult::new(spec.get("contents").cloned().unwrap_or(json!([]))))
    }
}

fn parse_exposure(input: &Value) -> Result<McpExposure, String> {
    let exposure = input.get("exposure").and_then(Value::as_str).unwrap_or("direct");
    McpExposure::parse(exposure).ok_or_else(|| format!("unknown exposure: {}", exposure))
}

pub fn run(input: &Value) -> Result<Value, String> {
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    // Deterministic fake output saver, mirrored by the TS harness (never
    // triggered by the catalog — the TS side's real default writes random
    // temp files).
    let fake_saver = |_data: OutputData, extension: &str| -> Result<String, String> {
        Ok(format!("/tmp/pi-mcp-out{}", extension))
    };
    match fn_name {
        "definitions" => {
            let exposure = parse_exposure(input)?;
            let no_servers = || Vec::<&dyn McpResourceServer>::new();
            let options = McpResourceToolOptions { exposure, servers: &no_servers, save_output: &fake_saver };
            let definitions = create_mcp_resource_tool_definitions(&options)
                .iter()
                .map(|definition| {
                    Ok(json!({
                        "name": definition.name,
                        "label": definition.label,
                        "description": definition.description,
                        "parameters": definition.parameters,
                        "outputSchema": definition.output_schema,
                        "exposure": definition.exposure.as_str(),
                        "annotations": serde_json::to_value(definition.annotations)
                            .map_err(|e| e.to_string())?,
                    }))
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(json!({ "definitions": definitions }))
        }
        "execute" => {
            let exposure = parse_exposure(input)?;
            let tool_name = input["tool"].as_str().ok_or("missing tool")?;
            let params = input.get("params").cloned().unwrap_or(Value::Null);
            let specs = input["servers"].as_array().cloned().unwrap_or_default();
            let log = Rc::new(RefCell::new(Vec::new()));
            let servers = specs
                .iter()
                .map(|spec| ScriptedServer::new(spec, &log))
                .collect::<Result<Vec<_>, String>>()?;
            let refs: Vec<&dyn McpResourceServer> =
                servers.iter().map(|server| server as &dyn McpResourceServer).collect();
            let get_servers = || refs.clone();
            let options = McpResourceToolOptions { exposure, servers: &get_servers, save_output: &fake_saver };
            let definitions = create_mcp_resource_tool_definitions(&options);
            let definition = definitions
                .iter()
                .find(|definition| definition.name == tool_name)
                .ok_or_else(|| format!("unknown tool: {}", tool_name))?;
            let outcome = definition.execute("parity-call", &params);
            let calls = std::mem::take(&mut *log.borrow_mut());
            match outcome {
                Ok(result) => Ok(json!({
                    "result": serde_json::to_value(&result).map_err(|e| e.to_string())?,
                    "calls": calls,
                })),
                Err(message) => Ok(json!({ "error": message, "calls": calls })),
            }
        }
        _ => Err(format!("unknown mcp-resources fn: {fn_name}")),
    }
}
