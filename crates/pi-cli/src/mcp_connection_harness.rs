//! Parity-harness entry for the `mcp-connection` protocol:
//! `pi_extensions::mcp::runtime` (TS `extensions/mcp/runtime.ts` +
//! pi-mcp `client.ts`) — `McpServerConnection` over scripted fake
//! transports. TS mirror: parity/ts-harness.js `mcp-connection` entry.
//!
//! args:
//! ```json
//! {
//!   "entry": { "name": "srv", "config": { "url": "…" | "command": "…" } },
//!   "cwd"?: "/tmp/pi-parity-mcc",
//!   "logPath"?: "logs/srv.log",
//!   "steps": [
//!     { "op": "callTool", "name": "t", "args"?: {}, "options"?: {"timeoutMs": 5} },
//!     { "op": "readResource", "uri": "u", "options"?: {…} },
//!     { "op": "resourcesPage", "cursor"?: "c", "options"?: {…} },
//!     { "op": "resourceTemplatesPage", "cursor"?: "c", "options"?: {…} },
//!     { "op": "allResources", "options"?: {…} },
//!     { "op": "allResourceTemplates", "options"?: {…} },
//!     { "op": "reconnect" }, { "op": "signOut" }, { "op": "close" },
//!     { "op": "snapshot" },
//!     { "op": "emit", "message"?: {} | "error"?: "text" | "close"?: true },
//!     { …, "nowMs"?: 0 }
//!   ],
//!   "transports": [ { "startSteps": […], "sendSteps": [[…], …] }, … ]
//! }
//! ```
//!
//! Transport steps: `{ "message": json } | { "error": "text" } |
//! { "close": true } | { "fail": { "kind": "http" | "authRequired" |
//! "sessionExpired" | "oauthRequired" | "typeError" | "error" | "closed",
//! "status"?, "message"?, "body"? } }`. Each connection attempt consumes
//! the next transport script; each `send` pops one send-step batch (a
//! missing/empty batch succeeds silently).
//!
//! Result: `{ "results": [...], "markers": [[...], ...], "sent": [...],
//! "files": { rel: content } }` — per-step op outcome (request payload,
//! `{ "error": message }` on rejection, null for void ops / snapshot
//! objects for `snapshot`), per-step `onTools`/`onChange` markers, all
//! sent messages across transports (creation order), and the log file
//! contents (only with `logPath`).
//!
//! Harness constraints (violations diverge from TS; see
//! docs/plans/phase4-tranche5-mcp-connection.md): replies arrive in the
//! same send's batch or the case tests timeouts with small `timeoutMs`;
//! incoming server requests ride in batches without response events;
//! no emits while disconnected.

use std::sync::{Arc, Mutex};
use std::collections::VecDeque;

use serde_json::{json, Map, Value};

use pi_core::mcp_servers::{McpServerConfig, McpServerKind};
use pi_extensions::mcp::client::{ClientEvent, McpErrorKind, RequestOptions};
use pi_extensions::mcp::log::McpServerLog;
use pi_extensions::mcp::runtime::{
    McpAuthProvider, McpCredentialStore, McpLogSink, McpServerConnection, McpServerConnectionOptions,
    McpServerEntry,
};
use pi_extensions::mcp::transport::{FakeStep, FakeTransport, McpTransport, SendOptions};

const DEFAULT_CWD: &str = "/tmp/pi-parity-mcc";

/// A `Box<dyn McpTransport>` over a shared `FakeTransport`, so the harness
/// keeps handles to the transports the factory created (for `sent`).
struct SharedTransport(Arc<Mutex<FakeTransport>>);

impl McpTransport for SharedTransport {
    fn start(&mut self) -> Result<(), McpErrorKind> {
        self.0.lock().unwrap().start()
    }
    fn send(&mut self, message: &Value, options: SendOptions) -> Result<(), McpErrorKind> {
        self.0.lock().unwrap().send(message, options)
    }
    fn close(&mut self) -> Result<(), McpErrorKind> {
        self.0.lock().unwrap().close()
    }
    fn on_message(&mut self, listener: pi_extensions::mcp::transport::MessageListener) -> usize {
        self.0.lock().unwrap().on_message(listener)
    }
    fn on_error(&mut self, listener: pi_extensions::mcp::transport::ErrorListener) -> usize {
        self.0.lock().unwrap().on_error(listener)
    }
    fn on_close(&mut self, listener: pi_extensions::mcp::transport::CloseListener) -> usize {
        self.0.lock().unwrap().on_close(listener)
    }
    fn off_message(&mut self, id: usize) {
        self.0.lock().unwrap().off_message(id)
    }
    fn off_error(&mut self, id: usize) {
        self.0.lock().unwrap().off_error(id)
    }
    fn off_close(&mut self, id: usize) {
        self.0.lock().unwrap().off_close(id)
    }
    fn set_protocol_version(&mut self, version: &str) {
        self.0.lock().unwrap().set_protocol_version(version)
    }
}

/// `makeError(spec)` — the TS error classes for Fail steps.
fn parse_fail(spec: &Value) -> Result<McpErrorKind, String> {
    let obj = spec
        .as_object()
        .ok_or_else(|| "fail must be an object".to_string())?;
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "fail must have a kind".to_string())?;
    let message = |key: &str| {
        obj.get(key)
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .unwrap_or_default()
    };
    match kind {
        "http" => Ok(McpErrorKind::Http {
            status: obj
                .get("status")
                .and_then(Value::as_u64)
                .ok_or_else(|| "http fail must have a status".to_string())?,
            message: message("message"),
            body: message("body"),
            auth_required: false,
            session_expired: false,
        }),
        "authRequired" => Ok(McpErrorKind::Http {
            status: 401,
            message: "MCP server requires authentication".to_string(),
            body: message("body"),
            auth_required: true,
            session_expired: false,
        }),
        "sessionExpired" => Ok(McpErrorKind::Http {
            status: 404,
            message: "MCP session expired".to_string(),
            body: message("body"),
            auth_required: false,
            session_expired: true,
        }),
        "oauthRequired" => Ok(McpErrorKind::OauthAuthorizationRequired),
        "typeError" => Ok(McpErrorKind::Type { message: message("message") }),
        "error" => Ok(McpErrorKind::Other { message: message("message") }),
        "closed" => Ok(McpErrorKind::ConnectionClosed {
            message: match obj.get("message") {
                Some(Value::String(s)) => s.clone(),
                _ => pi_extensions::mcp::transport::MCP_CONNECTION_CLOSED_MESSAGE.to_string(),
            },
        }),
        other => Err(format!("unknown fail kind: {}", other)),
    }
}

/// One transport step: message / error / close / fail.
fn parse_step(step: &Value) -> Result<FakeStep, String> {
    let obj = step
        .as_object()
        .ok_or_else(|| "transport steps must be objects".to_string())?;
    if let Some(message) = obj.get("message") {
        return Ok(FakeStep::Message(message.clone()));
    }
    if let Some(text) = obj.get("error") {
        return Ok(FakeStep::Error(
            text.as_str().unwrap_or_default().to_string(),
        ));
    }
    if obj.get("close") == Some(&Value::Bool(true)) {
        return Ok(FakeStep::Close);
    }
    if let Some(fail) = obj.get("fail") {
        return Ok(FakeStep::Fail(parse_fail(fail)?));
    }
    Err("transport steps must be message/error/close/fail".to_string())
}

/// One transport script: `startSteps` (flat) + `sendSteps` (one batch per
/// send).
fn parse_transport(script: &Value) -> Result<(Vec<FakeStep>, Vec<Vec<FakeStep>>), String> {
    let obj = script
        .as_object()
        .ok_or_else(|| "transports entries must be objects".to_string())?;
    let start_steps = match obj.get("startSteps") {
        None => Vec::new(),
        Some(Value::Array(steps)) => {
            let mut out = Vec::new();
            for step in steps {
                out.push(parse_step(step)?);
            }
            out
        }
        Some(_) => return Err("startSteps must be an array".to_string()),
    };
    let send_batches = match obj.get("sendSteps") {
        None => Vec::new(),
        Some(Value::Array(batches)) => {
            let mut out = Vec::new();
            for batch in batches {
                let batch = batch
                    .as_array()
                    .ok_or_else(|| "sendSteps entries must be arrays".to_string())?;
                let mut steps = Vec::new();
                for step in batch {
                    steps.push(parse_step(step)?);
                }
                out.push(steps);
            }
            out
        }
        Some(_) => return Err("sendSteps must be an array".to_string()),
    };
    Ok((start_steps, send_batches))
}

/// `McpRequestOptions` from a step's `options`.
fn parse_options(step: &Value) -> Result<RequestOptions, String> {
    match step.get("options") {
        None | Some(Value::Null) => Ok(RequestOptions::default()),
        Some(options) => {
            let obj = options
                .as_object()
                .ok_or_else(|| "options must be an object".to_string())?;
            let timeout_ms = match obj.get("timeoutMs") {
                None | Some(Value::Null) => None,
                Some(v) => Some(
                    v.as_u64()
                        .ok_or_else(|| "options.timeoutMs must be an integer".to_string())?,
                ),
            };
            Ok(RequestOptions { timeout_ms })
        }
    }
}

/// The log sink: `McpServerLog` over the shared in-memory fs with the
/// step-pinned clock.
struct HarnessLogSink {
    fs: Arc<crate::mcp_log_harness::MemoryMcpLogFs>,
    logger: McpServerLog,
    now_ms: Arc<Mutex<i64>>,
}

impl McpLogSink for HarnessLogSink {
    fn write(&mut self, server: &str, params: Option<&Value>) {
        let now = *self.now_ms.lock().unwrap();
        self.logger.write(&*self.fs, server, params, now);
    }
}

/// Reads back the log file (relative key, like the mcp-log harness).
fn log_file_contents(fs: &crate::mcp_log_harness::MemoryMcpLogFs, rel: &str) -> Option<Value> {
    let base = format!("<base>/{}", rel.trim_start_matches('/'));
    fs.files
        .lock()
        .unwrap()
        .get(&base)
        .map(|bytes| Value::String(String::from_utf8_lossy(bytes).into_owned()))
}

pub fn run_mcp_connection_harness(input: &Value) -> Result<Value, String> {
    let entry = input
        .get("entry")
        .and_then(Value::as_object)
        .ok_or_else(|| "missing entry".to_string())?;
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "entry.name must be a string".to_string())?
        .to_string();
    let config = entry
        .get("config")
        .and_then(Value::as_object)
        .ok_or_else(|| "entry.config must be an object".to_string())?;
    // The connection only checks `"url" in config`; derive the kind the
    // same way instead of running full config validation.
    let kind = if config.contains_key("url") {
        McpServerKind::Http
    } else {
        McpServerKind::Stdio
    };
    let server_entry = McpServerEntry {
        name,
        config: McpServerConfig { kind, fields: config.clone() },
    };
    let cwd = match input.get("cwd") {
        None | Some(Value::Null) => DEFAULT_CWD.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => return Err("cwd must be a string".to_string()),
    };
    let log_path = match input.get("logPath") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("logPath must be a string".to_string()),
    };
    let steps = match input.get("steps") {
        Some(Value::Array(steps)) => steps.clone(),
        _ => return Err("missing steps".to_string()),
    };
    let transports_arg = match input.get("transports") {
        Some(Value::Array(transports)) => transports.clone(),
        _ => return Err("missing transports".to_string()),
    };
    let mut scripts = VecDeque::new();
    for script in &transports_arg {
        let (start_steps, send_batches) = parse_transport(script)?;
        scripts.push_back((start_steps, send_batches));
    }

    // Shared harness state.
    let transports: Arc<Mutex<Vec<Arc<Mutex<FakeTransport>>>>> = Arc::new(Mutex::new(Vec::new()));
    let markers: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let fs = Arc::new(crate::mcp_log_harness::MemoryMcpLogFs::default());
    let now_ms: Arc<Mutex<i64>> = Arc::new(Mutex::new(0));

    let factory_transports = transports.clone();
    let factory_scripts = Arc::new(Mutex::new(scripts));
    let factory = move |_entry: &McpServerEntry,
                        _cwd: &str,
                        _auth: Option<&McpAuthProvider>|
          -> Box<dyn McpTransport> {
        // A missing script behaves like the TS `scripts.shift()` returning
        // undefined: a transport whose sends all succeed silently.
        let (start_steps, send_batches) = factory_scripts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default();
        let mut transport = FakeTransport::new();
        transport.start_steps = start_steps;
        transport.send_steps = VecDeque::from(send_batches);
        let shared = Arc::new(Mutex::new(transport));
        factory_transports.lock().unwrap().push(shared.clone());
        Box::new(SharedTransport(shared))
    };

    let log: Option<Box<dyn McpLogSink>> = log_path.as_ref().map(|rel| {
        Box::new(HarnessLogSink {
            fs: fs.clone(),
            logger: McpServerLog::new(format!(
                "<base>/{}",
                rel.trim_start_matches('/')
            )),
            now_ms: now_ms.clone(),
        }) as Box<dyn McpLogSink>
    });

    let on_tools = {
        let markers = markers.clone();
        move |_: &pi_extensions::mcp::runtime::McpConnectionSnapshot| {
            markers.lock().unwrap().push("onTools")
        }
    };
    let on_change = {
        let markers = markers.clone();
        move |_: &pi_extensions::mcp::runtime::McpConnectionSnapshot| {
            markers.lock().unwrap().push("onChange")
        }
    };
    // Real sleeping: request timeouts and connect retries advance real
    // time on both sides (TS timers).
    let sleep: pi_extensions::mcp::transport::SleepFn =
        std::sync::Arc::new(|ms| std::thread::sleep(std::time::Duration::from_millis(ms)));

    let mut connection = McpServerConnection::new(McpServerConnectionOptions {
        entry: server_entry,
        cwd,
        create_transport: Box::new(factory),
        credentials: Box::new(()) as Box<dyn McpCredentialStore>,
        on_tools: Box::new(on_tools),
        on_change: Some(Box::new(on_change)),
        log,
        sleep,
    });

    let mut results: Vec<Value> = Vec::new();
    let mut markers_per_step: Vec<Vec<Value>> = Vec::new();
    let take_markers = |markers: &Arc<Mutex<Vec<&'static str>>>| -> Vec<Value> {
        markers
            .lock()
            .unwrap()
            .drain(..)
            .map(|m| Value::String(m.to_string()))
            .collect()
    };
    for step in &steps {
        let obj = step
            .as_object()
            .ok_or_else(|| "steps entries must be objects".to_string())?;
        let op = obj
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing op".to_string())?;
        *now_ms.lock().unwrap() = match obj.get("nowMs") {
            None => 0,
            Some(v) => v
                .as_i64()
                .ok_or_else(|| "nowMs must be an integer".to_string())?,
        };
        let options = parse_options(step)?;
        // Step-local failures (unknown op, missing fields, bad emit args)
        // become the step's `{ error }` outcome and the run continues, like
        // the TS harness's per-step try/catch.
        let outcome: Value = match op {
            "callTool" => {
                let Some(name) = obj
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                else {
                    results.push(json!({ "error": "missing name" }));
                    markers_per_step.push(take_markers(&markers));
                    continue;
                };
                let args = obj.get("args").filter(|v| !v.is_null()).cloned();
                match connection.call_tool(&name, args, &options) {
                    Ok(value) => value,
                    Err(error) => json!({ "error": error.message() }),
                }
            }
            "readResource" => {
                let Some(uri) = obj
                    .get("uri")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                else {
                    results.push(json!({ "error": "missing uri" }));
                    markers_per_step.push(take_markers(&markers));
                    continue;
                };
                match connection.read_resource(&uri, &options) {
                    Ok(value) => value,
                    Err(error) => json!({ "error": error.message() }),
                }
            }
            "resourcesPage" => {
                let cursor = obj
                    .get("cursor")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                match connection.resources_page(cursor.as_deref(), &options) {
                    Ok(value) => value,
                    Err(error) => json!({ "error": error.message() }),
                }
            }
            "resourceTemplatesPage" => {
                let cursor = obj
                    .get("cursor")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                match connection.resource_templates_page(cursor.as_deref(), &options) {
                    Ok(value) => value,
                    Err(error) => json!({ "error": error.message() }),
                }
            }
            "allResources" => match connection.all_resources(&options) {
                Ok(value) => Value::Array(value),
                Err(error) => json!({ "error": error.message() }),
            },
            "allResourceTemplates" => match connection.all_resource_templates(&options) {
                Ok(value) => Value::Array(value),
                Err(error) => json!({ "error": error.message() }),
            },
            "reconnect" => match connection.reconnect() {
                Ok(()) => Value::Null,
                Err(error) => json!({ "error": error.message() }),
            },
            "signOut" => {
                connection.sign_out();
                Value::Null
            }
            "close" => {
                connection.close();
                Value::Null
            }
            "snapshot" => json!({
                "state": connection.state.as_str(),
                "error": connection.error,
                "tools": connection.tools,
                "hasResources": connection.has_resources,
                "instructions": connection.instructions,
                "challenge": connection.challenge,
            }),
            "emit" => {
                if let Some(message) = obj.get("message") {
                    connection.deliver_external(ClientEvent::Message(message.clone()));
                } else if let Some(text) = obj.get("error") {
                    connection.deliver_external(ClientEvent::Error(
                        text.as_str().unwrap_or_default().to_string(),
                    ));
                } else if obj.get("close") == Some(&Value::Bool(true)) {
                    connection.deliver_external(ClientEvent::Close);
                } else {
                    results.push(json!({ "error": "emit must have message, error, or close" }));
                    markers_per_step.push(take_markers(&markers));
                    continue;
                }
                Value::Null
            }
            other => {
                results.push(json!({ "error": format!("unknown mcp-connection op: {}", other) }));
                markers_per_step.push(take_markers(&markers));
                continue;
            }
        };
        results.push(outcome);
        markers_per_step.push(take_markers(&markers));
    }

    let sent: Vec<Value> = transports
        .lock()
        .unwrap()
        .iter()
        .flat_map(|t| t.lock().unwrap().sent.clone())
        .collect();
    let mut files = Map::new();
    if let Some(rel) = &log_path {
        if let Some(content) = log_file_contents(&fs, rel) {
            files.insert(rel.trim_start_matches('/').to_string(), content);
        }
    }
    Ok(json!({ "results": results, "markers": markers_per_step, "sent": sent, "files": files }))
}
