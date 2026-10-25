//! Parity-harness entry for the `mcp-extension` protocol: the
//! `createMcpExtension` state machine (`pi_extensions::mcp::extension`).
//! TS mirror: parity/ts-harness.js `mcp-extension` (drives the real
//! `createMcpExtension` factory with a fake `pi` API and fake ctx).
//!
//! args:
//! ```json
//! {
//!   "host": {
//!     "registeredServers": [{ "name": str, "config": json, "extensionPath": str }],
//!     "cwd": str, "agentDir": str, "projectTrusted": bool,
//!     "tools": [{ "name": str, "reachesTools"?: bool }],
//!     "activeTools": [str], "selectChoice"?: str
//!   },
//!   "loadConfig": { "servers": [McpServerEntry], "errors": [str],
//!                   "projectConfig"?: str, "autoEnableCodemode"?: bool },
//!   "transports": [ { "startSteps": [...], "sendSteps": [[...]] } ],
//!   "ops": [
//!     { "op": "sessionStart" },
//!     { "op": "settle" },
//!     { "op": "beforeAgentStart", "event": json },
//!     { "op": "setRegistered", "servers": [...] },
//!     { "op": "serversChange" },
//!     { "op": "turnStart" },
//!     { "op": "toolCall", "event": json },
//!     { "op": "command", "args": str },
//!     { "op": "status" },
//!     { "op": "completions", "prefix": str },
//!     { "op": "shutdown" }
//!   ]
//! }
//! ```
//!
//! Result: `{ "log": [...] }` with `notify:<level>:<message>`,
//! `activeTools:<json>`, and one line per op — byte-identical to the TS
//! harness.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pi_core::mcp_servers::{validate_mcp_server_config, RegisteredMcpServer};
use pi_extensions::mcp::extension::{
    McpExtension, McpExtensionOptions, McpHost, McpToolRegistration,
};
use pi_extensions::mcp::transport::{FakeTransport, McpTransport};
use pi_session::mcp_config::{LoadedMcpConfig, McpConfigScope, McpServerEntry};

#[derive(Default)]
struct HostState {
    registered: Vec<RegisteredMcpServer>,
    cwd: String,
    agent_dir: String,
    project_trusted: bool,
    tools: Vec<Value>,
    active_tools: Vec<String>,
    select_choice: Option<String>,
    has_ui: bool,
    log: Vec<String>,
}

struct HarnessHost(Arc<Mutex<HostState>>);

impl McpHost for HarnessHost {
    fn registered_servers(&self) -> Vec<RegisteredMcpServer> {
        self.0.lock().unwrap().registered.clone()
    }
    fn cwd(&self) -> String {
        self.0.lock().unwrap().cwd.clone()
    }
    fn agent_dir(&self) -> String {
        self.0.lock().unwrap().agent_dir.clone()
    }
    fn is_project_trusted(&self) -> bool {
        self.0.lock().unwrap().project_trusted
    }
    fn all_tools(&self) -> Vec<Value> {
        self.0.lock().unwrap().tools.clone()
    }
    fn active_tools(&self) -> Vec<String> {
        self.0.lock().unwrap().active_tools.clone()
    }
    fn set_active_tools(&self, names: Vec<String>) {
        let mut state = self.0.lock().unwrap();
        state.active_tools = names.clone();
        state.log.push(format!("activeTools:{}", serde_json::to_string(&names).unwrap()));
    }
    fn notify(&self, message: &str, level: &str) {
        self.0
            .lock()
            .unwrap()
            .log
            .push(format!("notify:{level}:{message}"));
    }
    fn has_ui(&self) -> bool {
        self.0.lock().unwrap().has_ui
    }
    fn register_tool(&self, definition: &McpToolRegistration) {
        self.0.lock().unwrap().log.push(format!(
            "registerTool:{}",
            serde_json::to_string(&definition.to_json()).unwrap()
        ));
    }
    fn select(&self, title: &str, items: &[String]) -> Option<String> {
        let mut state = self.0.lock().unwrap();
        state.log.push(format!("select:{title}:{}", serde_json::to_string(items).unwrap()));
        state.select_choice.clone()
    }
}

fn parse_scope(value: Option<&Value>) -> Option<McpConfigScope> {
    match value.and_then(Value::as_str) {
        Some("global") => Some(McpConfigScope::Global),
        Some("project") => Some(McpConfigScope::Project),
        Some("extension") => Some(McpConfigScope::Extension),
        _ => None,
    }
}

fn parse_entries(values: &[Value]) -> Result<Vec<McpServerEntry>, String> {
    let mut entries = Vec::with_capacity(values.len());
    for server in values {
        let name = server.get("name").and_then(Value::as_str).ok_or("server missing name")?;
        let config = validate_mcp_server_config(name, server.get("config").ok_or("server missing config")?)?;
        entries.push(McpServerEntry {
            name: name.to_string(),
            config,
            source: server
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("mcp.json")
                .to_string(),
            scope: parse_scope(server.get("scope")),
            r#override: server.get("override").and_then(Value::as_str).map(str::to_string),
        });
    }
    Ok(entries)
}

fn parse_registered(values: &[Value]) -> Result<Vec<RegisteredMcpServer>, String> {
    let mut servers = Vec::with_capacity(values.len());
    for server in values {
        let name = server.get("name").and_then(Value::as_str).ok_or("registered server missing name")?;
        let config = validate_mcp_server_config(name, server.get("config").ok_or("registered server missing config")?)?;
        servers.push(RegisteredMcpServer {
            name: name.to_string(),
            config,
            extension_path: server
                .get("extensionPath")
                .and_then(Value::as_str)
                .unwrap_or("/ext")
                .to_string(),
        });
    }
    Ok(servers)
}

fn load_config(value: Option<&Value>) -> Result<LoadedMcpConfig, String> {
    let Some(value) = value else {
        return Ok(LoadedMcpConfig::default());
    };
    let servers = value.get("servers").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(LoadedMcpConfig {
        servers: parse_entries(&servers)?,
        auto_enable_codemode: value.get("autoEnableCodemode").and_then(Value::as_bool),
        errors: value
            .get("errors")
            .and_then(Value::as_array)
            .map(|errors| errors.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        project_config: value.get("projectConfig").and_then(Value::as_str).map(str::to_string),
    })
}

pub fn run(input: &Value) -> Result<Value, String> {
    let host_args = input.get("host").cloned().unwrap_or_else(|| json!({}));
    let state = Arc::new(Mutex::new(HostState {
        registered: parse_registered(
            host_args.get("registeredServers").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]),
        )?,
        cwd: host_args.get("cwd").and_then(Value::as_str).unwrap_or("/proj").to_string(),
        agent_dir: host_args.get("agentDir").and_then(Value::as_str).unwrap_or("/home/u/.pi").to_string(),
        project_trusted: host_args.get("projectTrusted").and_then(Value::as_bool).unwrap_or(false),
        tools: host_args
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        active_tools: host_args
            .get("activeTools")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        select_choice: host_args.get("selectChoice").and_then(Value::as_str).map(str::to_string),
        has_ui: host_args.get("hasUI").and_then(Value::as_bool).unwrap_or(false),
        log: Vec::new(),
    }));
    let host = HarnessHost(Arc::clone(&state));
    let loaded = load_config(input.get("loadConfig"))?;
    // One scripted transport per connection the extension creates, like the
    // `mcp-connection` harness (same script schema).
    let mut scripts = VecDeque::new();
    for script in input
        .get("transports")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        scripts.push_back(crate::mcp_connection_harness::parse_transport(script)?);
    }
    let factory_scripts = Arc::new(Mutex::new(scripts));
    let factory = Box::new(
        move |_entry: &pi_extensions::mcp::runtime::McpServerEntry,
              _cwd: &str,
              _auth: Option<&pi_extensions::mcp::runtime::McpAuthProvider>|
              -> Box<dyn McpTransport> {
            let (start_steps, send_batches) = factory_scripts
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_default();
            let mut transport = FakeTransport::new();
            transport.start_steps = start_steps;
            transport.send_steps = VecDeque::from(send_batches);
            Box::new(crate::mcp_connection_harness::SharedTransport(Arc::new(
                Mutex::new(transport),
            )))
        },
    );
    let options = McpExtensionOptions {
        load_config: Arc::new(move |_host: &dyn McpHost| loaded.clone()),
        startup_wait_ms: input.get("startupWaitMs").and_then(Value::as_u64).unwrap_or(10_000),
        create_transport: Some(Arc::new(Mutex::new(factory))),
        sleep: Arc::new(|ms| std::thread::sleep(std::time::Duration::from_millis(ms))),
    };
    let mut extension = McpExtension::new(options);

    let ops = input.get("ops").and_then(Value::as_array).ok_or("missing ops")?;
    for op in ops {
        let name = op.get("op").and_then(Value::as_str).ok_or("op missing name")?;
        let line = match name {
            "sessionStart" => {
                extension.session_start(&host);
                "sessionStart:ok".to_string()
            }
            "settle" => {
                extension.settle(&host);
                "settle:ok".to_string()
            }
            "beforeAgentStart" => {
                let mut event = op.get("event").cloned().unwrap_or(Value::Null);
                extension.before_agent_start(&host, &mut event);
                format!("beforeAgentStart:{}", serde_json::to_string(&event).unwrap())
            }
            "setRegistered" => {
                let servers = op.get("servers").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
                state.lock().unwrap().registered = parse_registered(servers)?;
                "setRegistered:ok".to_string()
            }
            "serversChange" => {
                extension.mcp_servers_change(&host);
                "serversChange:ok".to_string()
            }
            "turnStart" => {
                extension.turn_start();
                "turnStart:ok".to_string()
            }
            "toolCall" => {
                let event = op.get("event").cloned().unwrap_or(Value::Null);
                extension.tool_call(&host, &event);
                "toolCall:ok".to_string()
            }
            "command" => {
                let args = op.get("args").and_then(Value::as_str).unwrap_or("");
                extension.handle_command(args, &host);
                format!("command:{args}:ok")
            }
            "completions" => {
                let prefix = op.get("prefix").and_then(Value::as_str).unwrap_or("");
                let items = extension.argument_completions(prefix).map(|items| {
                    items
                        .into_iter()
                        .map(|item| match item.description {
                            Some(description) => json!({
                                "value": item.value,
                                "label": item.label,
                                "description": description,
                            }),
                            None => json!({ "value": item.value, "label": item.label }),
                        })
                        .collect::<Vec<_>>()
                });
                format!("completions:{}", serde_json::to_string(&items).unwrap())
            }
            "shutdown" => {
                extension.session_shutdown(&host);
                "shutdown:ok".to_string()
            }
            other => return Err(format!("unknown mcp-extension op: {other}")),
        };
        let mut state = state.lock().unwrap();
        // Host-side lines (notify/activeTools/select) happened during the op.
        let host_log = std::mem::take(&mut state.log);
        state.log.extend(host_log);
        state.log.push(line);
    }
    let lines = state.lock().unwrap().log.clone();
    Ok(json!({ "log": lines }))
}
