//! `extensions/mcp/index.ts` — the built-in MCP extension's state machine
//! (`createMcpExtension`, L279–L1353), minus the parts that need real
//! connections, OAuth, or the TUI manager.
//!
//! This slice ports the session lifecycle and everything that can run
//! without a server connection:
//!
//! - `session_start`: load `mcp.json` through the [`McpExtensionOptions`]
//!   seam, merge extension-registered servers (config wins by namespace),
//!   reset the session state, activate codemode/tool_search, and report
//!   startup problems.
//! - `mcp_servers_change`: re-merge registrations; servers that were
//!   unregistered or re-registered with a new config are dropped (and their
//!   tools hidden — tool registration lands with the connection slice).
//! - `before_agent_start`: render/remove the `mcp_servers` prompt section.
//! - `tool_call`: flush queued startups; route by `reachesTools`, resource
//!   tool names, or tool owner (the synchronous waits are already settled).
//! - `turn_start` / `session_shutdown`.
//! - `/mcp` command: non-TUI status and argument completions.
//!
//! Connections live in the state machine as `Arc<Mutex<McpServerConnection>>`
//! (they are `Send`; see the plan §3.3). Their `onTools`/`onChange` hooks only
//! enqueue snapshots into a shared queue, which the state machine drains at
//! the next deterministic boundary (`settle`, a command, a prompt, shutdown):
//! a callback cannot borrow the extension that owns the connection.
//!
//! Tool registration (`registerTools`/`hideTools`/resource exposure sync) is
//! modeled as a [`McpToolRegistration`] handed to [`McpHost::register_tool`];
//! the runner adapter (10f) turns it into the real tool (execute/renderers).
//!
//! Deferred to the next slices (see `docs/plans/phase4-tranche7-mcp-extension.md`):
//! `/mcp` actions (10e), the TUI manager/sign-in, and the runner adapter (10f).

use std::sync::{Arc, Mutex};

use std::collections::HashMap;

use pi_core::js_string::js_trim;
use pi_core::mcp_resources::{create_mcp_resource_tool_definitions, McpResourceToolOptions};
use pi_core::mcp_servers::{
    get_mcp_tool_exposure, mcp_namespace, McpExposure, RegisteredMcpServer,
    LIST_MCP_RESOURCE_TEMPLATES_TOOL, LIST_MCP_RESOURCES_TOOL, READ_MCP_RESOURCE_TOOL,
};
use pi_core::mcp_tools::{
    create_mcp_result_schema, create_mcp_tool_name, to_parameters, to_tool_annotations,
    to_tool_exposure, McpTool, ToolAnnotations, ToolExposure,
};
use pi_session::mcp_config::{LoadedMcpConfig, McpConfigScope, McpServerEntry};
use serde_json::Value;

use super::runtime::{
    McpConnectionSnapshot, McpCredentialStore, McpServerConnection, McpServerConnectionOptions,
    ServerState, TransportFactory,
};
use super::section::{configured_exposures, exposure_of, has_direct_tools, is_enabled, render_servers_section, McpServerListing, MCP_SERVERS_SECTION};
use super::transport::SleepFn;
use super::status::{
    argument_completions, describe_state, format_status, McpCompletion, McpConnectionStatus,
    McpServerSnapshot, MCP_USAGE,
};

/// Tool names the discovery activation looks for (`CODEMODE_TOOL_NAME`,
/// `TOOL_SEARCH_TOOL_NAME`).
pub const CODEMODE_TOOL_NAME: &str = "codemode";
pub const TOOL_SEARCH_TOOL_NAME: &str = "tool_search";

/// `DEFAULT_STARTUP_WAIT_MS` (L99).
pub const DEFAULT_STARTUP_WAIT_MS: u64 = 10_000;

/// The host APIs the extension calls: `pi.*` methods and `ctx.ui`/`ctx` reads.
/// The runner adapter (later slice) and the parity harness both implement it.
pub trait McpHost: Send + Sync {
    /// `pi.getMcpServers()`.
    fn registered_servers(&self) -> Vec<RegisteredMcpServer>;
    /// `ctx.cwd`.
    fn cwd(&self) -> String;
    /// `getAgentDir()`.
    fn agent_dir(&self) -> String;
    /// `ctx.isProjectTrusted()`.
    fn is_project_trusted(&self) -> bool;
    /// `pi.getAllTools()` — each entry has at least `name`, plus
    /// `reachesTools` when the tool reaches other tools.
    fn all_tools(&self) -> Vec<Value>;
    /// `pi.getActiveTools()`.
    fn active_tools(&self) -> Vec<String>;
    /// `pi.setActiveTools(names)`.
    fn set_active_tools(&self, names: Vec<String>);
    /// `ctx.ui.notify(message, level)`.
    fn notify(&self, message: &str, level: &str);
    /// `ctx.ui.select(title, items)` — the interactive picker used when a
    /// `/mcp` subcommand's server is ambiguous.
    fn select(&self, title: &str, items: &[String]) -> Option<String>;
    /// `pi.registerTool(definition)`.
    fn register_tool(&self, definition: &McpToolRegistration);
    /// `ctx.hasUI`.
    fn has_ui(&self) -> bool;
}

/// The `loadConfig` seam (`McpExtensionOptions.loadConfig`).
pub type LoadConfigFn = Arc<dyn Fn(&dyn McpHost) -> LoadedMcpConfig + Send + Sync>;

/// One registered MCP tool (`createMcpToolDefinition`, tools.ts L312), as
/// far as the extension builds it: the pure definition fields plus the
/// execution context the runner adapter needs (`server`, `tool`, timeout).
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolRegistration {
    pub name: String,
    pub label: String,
    pub description: String,
    pub parameters: Value,
    pub output_schema: Value,
    pub exposure: ToolExposure,
    /// `ToolNamespace` — absent for the resource tools.
    pub namespace: Option<Value>,
    pub annotations: Option<ToolAnnotations>,
    /// The server the tool came from (`connection.entry.name`).
    pub server: String,
    /// The server's tool name.
    pub tool: String,
    pub timeout_ms: u64,
    /// One of the resource tools (`list_mcp_resources`, …) instead of a
    /// server tool.
    pub is_resource: bool,
}

impl McpToolRegistration {
    /// `{ ...definition, exposure: "hidden" }` for a withdrawn tool.
    pub fn hidden(&self) -> McpToolRegistration {
        let mut definition = self.clone();
        definition.exposure = ToolExposure::Hidden;
        definition
    }

    /// The definition as `pi.registerTool` sees it, in the TS object's key
    /// order (renderers/execute are not JSON).
    pub fn to_json(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("name".to_string(), Value::String(self.name.clone()));
        object.insert("label".to_string(), Value::String(self.label.clone()));
        object.insert(
            "description".to_string(),
            Value::String(self.description.clone()),
        );
        object.insert("parameters".to_string(), self.parameters.clone());
        object.insert("outputSchema".to_string(), self.output_schema.clone());
        object.insert(
            "exposure".to_string(),
            Value::String(self.exposure.as_str().to_string()),
        );
        if let Some(namespace) = &self.namespace {
            object.insert("namespace".to_string(), namespace.clone());
        }
        if let Some(annotations) = &self.annotations {
            object.insert(
                "annotations".to_string(),
                serde_json::to_value(annotations).unwrap_or(Value::Null),
            );
        }
        Value::Object(object)
    }
}

/// A connection callback, forwarded out of the connection (`onTools` /
/// `onChange`). The state machine drains these at its next boundary.
#[derive(Debug, Clone)]
pub enum ConnectionEvent {
    /// `onTools(connection)`: the server connected or refreshed its tools.
    Tools(McpConnectionSnapshot),
    /// `onChange(connection)`: the connection state changed.
    Changed(McpConnectionSnapshot),
}

/// `McpExtensionOptions` (L69) — the subset this slice needs.
#[derive(Clone)]
pub struct McpExtensionOptions {
    /// `loadConfig`: defaults to `loadMcpConfig({agentDir, cwd, projectTrusted})`
    /// in the real adapter; the harness injects the result directly.
    pub load_config: LoadConfigFn,
    /// `startupWaitMs ?? 10_000`.
    pub startup_wait_ms: u64,
    /// `options.createTransport` (the TS option is one function shared by
    /// every connection). `None` leaves connections unstarted; the runner
    /// adapter supplies the default stdio/HTTP transport.
    pub create_transport: Option<Arc<Mutex<TransportFactory>>>,
    /// Timers (connect retries, request timeouts).
    pub sleep: SleepFn,
}

impl Default for McpExtensionOptions {
    fn default() -> Self {
        McpExtensionOptions {
            load_config: Arc::new(|_| LoadedMcpConfig::default()),
            startup_wait_ms: DEFAULT_STARTUP_WAIT_MS,
            create_transport: None,
            sleep: Arc::new(|ms| std::thread::sleep(std::time::Duration::from_millis(ms))),
        }
    }
}

/// A configured server (`McpServer`, L108).
#[derive(Clone)]
pub struct McpServerState {
    pub entry: McpServerEntry,
    /// `registeredConfig` for extension-registered servers.
    pub registered_config: Option<String>,
    /// Result of the last `/mcp` action that failed.
    pub message: Option<String>,
    /// Display state, synced from the connection's snapshots.
    pub connection: Option<McpConnectionStatus>,
    /// The live connection, once `startConnection` created it.
    handle: Option<Arc<Mutex<McpServerConnection>>>,
    /// `server.ready` exists (the TS attempt started; the port connects
    /// inline, so an existing handle is settled unless `pending_start`
    /// still names it).
    ready: bool,
}

impl McpServerState {
    fn snapshot(&self) -> McpServerSnapshot {
        McpServerSnapshot {
            entry: self.entry.clone(),
            connection: self.connection.clone(),
            message: self.message.clone(),
        }
    }
}

/// `McpExtension` — the closure state of `createMcpExtension`.
pub struct McpExtension {
    options: McpExtensionOptions,
    servers: Vec<McpServerState>,
    /// Servers from `mcp.json`, which take precedence over registered ones.
    configured_entries: Vec<McpServerEntry>,
    config_errors: Vec<String>,
    /// The trusted project's `mcp.json`, where `/mcp` saves overrides.
    project_config: Option<String>,
    /// Registered servers that `mcp.json` overrides, shown in `/mcp`.
    overridden: Vec<String>,
    auto_enable_codemode: bool,
    warned_unreachable: bool,
    /// Whether a prompt already waited for startup connections.
    waited_for_startup: bool,
    /// Server names whose `/mcp` action is running (the TS `backgroundActions`
    /// lives in the connection slice).
    session_cwd: String,
    /// Lifetime of the current session (`session.aborted` after shutdown).
    aborted: bool,
    /// Snapshot queue filled by the connection callbacks.
    connection_events: Arc<Mutex<Vec<ConnectionEvent>>>,
    /// Enabled servers whose `startConnection` the TS launches in the
    /// background from `session_start`; the port connects them at the next
    /// boundary (`settle`, a command, a prompt, shutdown).
    pending_start: Vec<String>,
    /// pi tool name → `<server>\0<tool>` it was assigned to.
    tool_owners: HashMap<String, String>,
    /// Tool names currently offered by each server.
    server_tools: HashMap<String, Vec<String>>,
    /// Last definition registered under each tool name.
    definitions: HashMap<String, McpToolRegistration>,
    /// Exposure the resource tools were last registered with.
    resource_tools_exposure: Option<McpExposure>,
}

impl McpExtension {
    pub fn new(options: McpExtensionOptions) -> McpExtension {
        McpExtension {
            options,
            servers: Vec::new(),
            configured_entries: Vec::new(),
            config_errors: Vec::new(),
            project_config: None,
            overridden: Vec::new(),
            auto_enable_codemode: true,
            warned_unreachable: false,
            waited_for_startup: false,
            session_cwd: String::new(),
            aborted: true,
            connection_events: Arc::new(Mutex::new(Vec::new())),
            pending_start: Vec::new(),
            tool_owners: HashMap::new(),
            server_tools: HashMap::new(),
            definitions: HashMap::new(),
            resource_tools_exposure: None,
        }
    }

    /// Configured servers, in `servers` order (`McpServer[]`).
    pub fn servers(&self) -> &[McpServerState] {
        &self.servers
    }

    pub fn config_errors(&self) -> &[String] {
        &self.config_errors
    }

    pub fn overridden(&self) -> &[String] {
        &self.overridden
    }

    pub fn project_config(&self) -> Option<&str> {
        self.project_config.as_deref()
    }

    pub fn auto_enable_codemode(&self) -> bool {
        self.auto_enable_codemode
    }

    pub fn is_aborted(&self) -> bool {
        self.aborted
    }

    /// `findServer(name)`.
    pub fn find_server(&self, name: &str) -> Option<&McpServerState> {
        self.servers.iter().find(|server| server.entry.name == name)
    }

    /// `registeredServers()` (L341): registered servers except names
    /// `mcp.json` defines, which take precedence.
    fn registered_servers(&self, host: &dyn McpHost) -> (Vec<McpServerState>, Vec<String>) {
        let mut registered = Vec::new();
        let mut overridden = Vec::new();
        for server in host.registered_servers() {
            let configured = self.configured_entries.iter().find(|entry| {
                mcp_namespace(&entry.name) == mcp_namespace(&server.name)
            });
            if let Some(configured) = configured {
                overridden.push(format!(
                    "\"{}\" registered by {} is overridden by \"{}\" in {}",
                    server.name, server.extension_path, configured.name, configured.source
                ));
                continue;
            }
            let registered_config = serde_json::to_string(&server.config).ok();
            registered.push(McpServerState {
                entry: McpServerEntry {
                    name: server.name,
                    config: server.config,
                    source: server.extension_path,
                    scope: Some(McpConfigScope::Extension),
                    r#override: None,
                },
                registered_config,
                message: None,
                connection: None,
                handle: None,
                ready: false,
            });
        }
        (registered, overridden)
    }

    /// `pi.on("session_start")` (L1114).
    pub fn session_start(&mut self, host: &dyn McpHost) {
        let loaded = (self.options.load_config)(host);
        self.config_errors = loaded.errors;
        self.project_config = loaded.project_config;
        self.auto_enable_codemode = loaded.auto_enable_codemode.unwrap_or(true);
        self.warned_unreachable = false;
        self.waited_for_startup = false;
        self.session_cwd = host.cwd();
        self.aborted = false;
        self.configured_entries = loaded.servers;
        let (registered, overridden) = self.registered_servers(host);
        self.overridden = overridden;
        self.servers = self
            .configured_entries
            .iter()
            .cloned()
            .map(|entry| McpServerState {
                entry,
                registered_config: None,
                message: None,
                connection: None,
                handle: None,
                ready: false,
            })
            .chain(registered)
            .collect();
        // Codemode or tool_search is activated from the config: the first
        // prompt does not wait for servers whose tools are not declared.
        self.ensure_discovery_active(host);
        self.connection_events.lock().unwrap().clear();
        self.pending_start.clear();
        // The TS starts the connections in the background (after one event
        // loop turn) and reports problems once they settle. The port queues
        // them and connects at the next boundary: `settle`, a command, or a
        // prompt. With no enabled server the report happens now.
        let enabled: Vec<String> = self
            .servers
            .iter()
            .filter(|server| is_enabled(&server.entry))
            .map(|server| server.entry.name.clone())
            .collect();
        if enabled.is_empty() {
            self.report_problems(host, None);
        } else if self.options.create_transport.is_some() {
            self.pending_start = enabled;
        }
    }

    /// Connect the servers `session_start` queued and report problems. The
    /// harness calls this after `sessionStart`; commands and prompts call it
    /// themselves (they `await pending` in the TS). A no-op without queued
    /// attempts, so it never adds observable lines on its own.
    pub fn settle(&mut self, host: &dyn McpHost) {
        self.flush_pending(host);
    }

    fn flush_pending(&mut self, host: &dyn McpHost) {
        if self.pending_start.is_empty() {
            return;
        }
        let names = std::mem::take(&mut self.pending_start);
        if self.aborted {
            return;
        }
        for name in names {
            if let Some(index) = self
                .servers
                .iter()
                .position(|server| server.entry.name == name)
            {
                if is_enabled(&self.servers[index].entry) {
                    self.start_connection(index);
                }
            }
        }
        self.drain_connection_events(host);
        if !self.aborted {
            self.report_problems(host, None);
        }
    }

    /// `createConnection` + `getClient()` for the server at `index`
    /// (`startConnection`, L560). The connection's hooks only enqueue
    /// snapshots; the caller drains them.
    fn start_connection(&mut self, index: usize) {
        let Some(factory) = self.options.create_transport.clone() else {
            return;
        };
        if self.servers[index].handle.is_some() {
            return;
        }
        let entry = self.servers[index].entry.clone();
        let cwd = self.session_cwd.clone();
        let events = Arc::clone(&self.connection_events);
        let tools_events = Arc::clone(&events);
        let on_tools = Box::new(move |snapshot: &McpConnectionSnapshot| {
            tools_events
                .lock()
                .unwrap()
                .push(ConnectionEvent::Tools(snapshot.clone()));
        });
        let change_events = Arc::clone(&events);
        let on_change = Box::new(move |snapshot: &McpConnectionSnapshot| {
            change_events
                .lock()
                .unwrap()
                .push(ConnectionEvent::Changed(snapshot.clone()));
        });
        let create_transport: TransportFactory = Box::new(move |entry, cwd, auth| {
            (factory.lock().unwrap())(entry, cwd, auth)
        });
        let mut connection = McpServerConnection::new(McpServerConnectionOptions {
            entry: super::runtime::McpServerEntry {
                name: entry.name.clone(),
                config: entry.config,
            },
            cwd,
            create_transport,
            credentials: Box::new(()) as Box<dyn McpCredentialStore>,
            on_tools,
            on_change: Some(on_change),
            log: None,
            sleep: Arc::clone(&self.options.sleep),
        });
        let _ = connection.connect();
        self.servers[index].handle = Some(Arc::new(Mutex::new(connection)));
        self.servers[index].ready = true;
    }

    /// Apply a connection snapshot to the server's display state.
    fn apply_snapshot(&mut self, snapshot: &McpConnectionSnapshot) {
        let Some(server) = self
            .servers
            .iter_mut()
            .find(|server| server.entry.name == snapshot.name)
        else {
            return;
        };
        server.connection = Some(McpConnectionStatus {
            state: snapshot.state,
            tools: snapshot.tools.len(),
            has_resources: snapshot.has_resources,
            error: snapshot.error.clone(),
            oauth_url: snapshot.oauth_url.clone(),
        });
    }

    /// Drain the snapshot queue: update the display state and register the
    /// tools of servers that just connected.
    fn drain_connection_events(&mut self, host: &dyn McpHost) {
        let events: Vec<ConnectionEvent> = std::mem::take(&mut *self.connection_events.lock().unwrap());
        for event in events {
            match event {
                ConnectionEvent::Tools(snapshot) => {
                    self.apply_snapshot(&snapshot);
                    self.register_tools(host, &snapshot);
                }
                ConnectionEvent::Changed(snapshot) => self.apply_snapshot(&snapshot),
            }
        }
    }

    /// `registerTools(connection)` (L387).
    fn register_tools(&mut self, host: &dyn McpHost, snapshot: &McpConnectionSnapshot) {
        let server = snapshot.name.clone();
        let Some(index) = self
            .servers
            .iter()
            .position(|state| state.entry.name == server)
        else {
            return;
        };
        let entry = self.servers[index].entry.clone();
        let description = entry
            .config
            .fields
            .get("description")
            .and_then(Value::as_str)
            .map(js_trim)
            .filter(|description| !description.is_empty())
            .map(str::to_string);
        let mut namespace = serde_json::Map::new();
        namespace.insert(
            "name".to_string(),
            Value::String(mcp_namespace(&server).to_string()),
        );
        namespace.insert("aliases".to_string(), serde_json::json!([server]));
        if let Some(description) = &description {
            namespace.insert(
                "description".to_string(),
                Value::String(description.clone()),
            );
        }
        if let Some(instructions) = snapshot
            .instructions
            .as_deref()
            .filter(|instructions| !instructions.is_empty())
        {
            namespace.insert(
                "instructions".to_string(),
                Value::String(instructions.to_string()),
            );
        }
        let namespace = Value::Object(namespace);
        let previous: Vec<String> = self
            .server_tools
            .get(&server)
            .cloned()
            .unwrap_or_default();
        let mut current: Vec<String> = Vec::new();
        // Like Codex, all tools whose names sanitize to the same name get the
        // hash suffix, so which one keeps the plain name does not depend on
        // the order of the list.
        let mut unique: Vec<&str> = Vec::new();
        for tool in &snapshot.tools {
            if let Some(name) = tool.get("name").and_then(Value::as_str) {
                if !unique.contains(&name) {
                    unique.push(name);
                }
            }
        }
        let plain: Vec<String> = unique
            .iter()
            .map(|tool| create_mcp_tool_name(&server, tool, |_| false))
            .collect();
        let owners = self.tool_owners.clone();
        for tool_value in &snapshot.tools {
            let Some(tool_name) = tool_value.get("name").and_then(Value::as_str) else {
                continue;
            };
            let owner = format!("{server}\0{tool_name}");
            let is_taken = |candidate: &str| {
                let existing = owners.get(candidate);
                (existing.is_some() && existing != Some(&owner))
                    || current.iter().any(|name| name == candidate)
                    || plain.iter().filter(|name| name.as_str() == candidate).count() > 1
            };
            let name = create_mcp_tool_name(&server, tool_name, is_taken);
            self.tool_owners.insert(name.clone(), owner);
            current.push(name.clone());
            let tool = McpTool::from(
                tool_value
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            );
            let title = tool
                .title()
                .map(str::to_string)
                .or_else(|| {
                    tool.annotations()
                        .and_then(|annotations| annotations.get("title"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            let description = tool
                .description()
                .map(js_trim)
                .filter(|description| !description.is_empty())
                .map(str::to_string)
                .or(title)
                .unwrap_or_else(|| format!("MCP tool {tool_name} from server {server}"));
            let definition = McpToolRegistration {
                name: name.clone(),
                label: format!("{server}/{tool_name}"),
                description,
                parameters: to_parameters(tool.input_schema()),
                output_schema: create_mcp_result_schema(tool.output_schema()),
                exposure: to_tool_exposure(get_mcp_tool_exposure(&entry.config, tool_name)),
                namespace: Some(namespace.clone()),
                annotations: to_tool_annotations(&tool),
                server: server.clone(),
                tool: tool_name.to_string(),
                timeout_ms: snapshot.timeout_ms,
                is_resource: false,
            };
            self.definitions.insert(name.clone(), definition.clone());
            host.register_tool(&definition);
        }
        self.server_tools.insert(server.clone(), current.clone());
        // Tools cannot be unregistered, so tools the server dropped are
        // re-registered as hidden.
        for name in previous {
            if !current.contains(&name) {
                if let Some(definition) = self.definitions.get(&name) {
                    let hidden = definition.hidden();
                    host.register_tool(&hidden);
                }
            }
        }
        self.sync_resource_tools(host);
    }

    /// `hideTools(server)` (L448).
    fn hide_tools(&mut self, host: &dyn McpHost, server: &str) {
        for name in self.server_tools.get(server).cloned().unwrap_or_default() {
            if let Some(definition) = self.definitions.get(&name) {
                let hidden = definition.hidden();
                host.register_tool(&hidden);
            }
        }
        self.server_tools.insert(server.to_string(), Vec::new());
        self.sync_resource_tools(host);
    }

    /// Enabled servers with resources whose exposure is not `hidden`.
    fn servers_with_resources(&self) -> Vec<&McpServerState> {
        self.servers
            .iter()
            .filter(|server| {
                server
                    .connection
                    .as_ref()
                    .is_some_and(|connection| connection.has_resources)
                    && is_enabled(&server.entry)
                    && exposure_of(&server.entry) != McpExposure::Hidden
            })
            .collect()
    }

    /// `syncResourceTools` (L458): register the resource tools with the
    /// widest exposure of the servers they reach.
    fn sync_resource_tools(&mut self, host: &dyn McpHost) {
        let exposures: Vec<McpExposure> = self
            .servers_with_resources()
            .iter()
            .map(|server| exposure_of(&server.entry))
            .collect();
        let next = [McpExposure::Direct, McpExposure::Codemode, McpExposure::Deferred]
            .into_iter()
            .find(|candidate| exposures.contains(candidate))
            .unwrap_or(McpExposure::Hidden);
        if self.resource_tools_exposure == Some(next)
            || (self.resource_tools_exposure.is_none() && next == McpExposure::Hidden)
        {
            return;
        }
        let was_direct = self.resource_tools_exposure == Some(McpExposure::Direct);
        self.resource_tools_exposure = Some(next);
        fn no_servers<'a>() -> Vec<&'a dyn pi_core::mcp_resources::McpResourceServer> {
            Vec::new()
        }
        let save_output = |_: pi_core::mcp_tools::OutputData, _: &str| {
            Ok::<String, String>("/tmp/pi-mcp-out".to_string())
        };
        let options = McpResourceToolOptions {
            exposure: next,
            servers: &no_servers,
            save_output: &save_output,
        };
        let definitions = create_mcp_resource_tool_definitions(&options);
        let mut names: Vec<String> = Vec::new();
        for definition in &definitions {
            names.push(definition.name.to_string());
            host.register_tool(&McpToolRegistration {
                name: definition.name.to_string(),
                label: definition.label.clone(),
                description: definition.description.clone(),
                parameters: definition.parameters.clone(),
                output_schema: definition.output_schema.clone(),
                exposure: definition.exposure,
                namespace: None,
                annotations: Some(definition.annotations),
                server: "*".to_string(),
                tool: definition.name.to_string(),
                timeout_ms: 0,
                is_resource: true,
            });
        }
        if was_direct {
            let active: Vec<String> = host
                .active_tools()
                .into_iter()
                .filter(|name| !names.contains(name))
                .collect();
            host.set_active_tools(active);
        }
    }

    /// `pi.on("before_agent_start")` (L1178): inject or remove the
    /// `mcp_servers` section on the event's `systemPromptOptions.sections`.
    pub fn before_agent_start(&mut self, host: &dyn McpHost, event: &mut Value) {
        // `await waitForDirectServers(ctx)`: the port connects inline, so
        // flushing the queued attempts settles everything the wait covers.
        self.flush_pending(host);
        self.waited_for_startup = true;
        let instructions: Vec<Option<String>> = self
            .servers
            .iter()
            .map(|server| {
                server.handle.as_ref().and_then(|handle| {
                    let snapshot = handle.lock().unwrap().snapshot();
                    snapshot.instructions
                })
            })
            .collect();
        let listings: Vec<McpServerListing<'_>> = self
            .servers
            .iter()
            .zip(&instructions)
            .map(|(server, instructions)| McpServerListing {
                entry: &server.entry,
                instructions: instructions.as_deref(),
            })
            .collect();
        let section = render_servers_section(&listings);
        let Some(options) = event.get_mut("systemPromptOptions").and_then(Value::as_object_mut) else {
            return;
        };
        let Some(sections) = options.get_mut("sections").and_then(Value::as_object_mut) else {
            return;
        };
        match section {
            Some(section) => {
                sections.insert(MCP_SERVERS_SECTION.to_string(), Value::String(section));
            }
            None => {
                sections.shift_remove(MCP_SERVERS_SECTION);
            }
        }
    }

    /// `pi.on("tool_call")` (L1191): wait for the servers the tool may
    /// reach. The port connects inline, so flushing the queued startups is
    /// the wait itself; the routing decides which servers the TS would wait
    /// for (and so which tools are registered before the call runs).
    pub fn tool_call(&mut self, host: &dyn McpHost, event: &Value) {
        self.flush_pending(host);
        let tool_name = match event.get("toolName").and_then(Value::as_str) {
            Some(name) => name,
            None => return,
        };
        let tools = host.all_tools();
        let Some(tool) = tools
            .iter()
            .find(|tool| tool.get("name").and_then(Value::as_str) == Some(tool_name))
        else {
            return;
        };
        // Readiness can be pending while the display state is still
        // connected; here every handle exists only after `connect()`.
        let ready: Vec<&McpServerState> = self
            .servers
            .iter()
            .filter(|server| is_enabled(&server.entry) && server.handle.is_some())
            .collect();
        if ready.is_empty() {
            return;
        }
        let waiting: Vec<&str> = if let Some(reach) = tool.get("reachesTools") {
            ready
                .iter()
                .filter(|server| reaches_server(reach, &server.entry.name))
                .map(|server| server.entry.name.as_str())
                .collect()
        } else if is_resource_tool(tool_name) {
            ready.iter().map(|server| server.entry.name.as_str()).collect()
        } else {
            let owner = self
                .tool_owners
                .get(tool_name)
                .and_then(|owner| owner.split('\0').next())
                .unwrap_or_default();
            ready
                .iter()
                .filter(|server| server.entry.name == owner)
                .map(|server| server.entry.name.as_str())
                .collect()
        };
        // Synchronous connections: a `ready` for each of these is already
        // settled, so `waitForServers` returns immediately.
        let _ = waiting;
    }

    /// `pi.on("turn_start")` (L1212): retry servers that got sign-ins
    /// elsewhere. Without connections/sign-ins this is a no-op.
    pub fn turn_start(&mut self) {}

    /// `pi.on("mcp_servers_change")` (L1217).
    pub fn mcp_servers_change(&mut self, host: &dyn McpHost) {
        if self.aborted {
            return;
        }
        let (registered, overridden) = self.registered_servers(host);
        self.overridden = overridden;
        let next: Vec<(String, Option<String>)> = registered
            .iter()
            .map(|server| (server.entry.name.clone(), server.registered_config.clone()))
            .collect();
        // Unregistered servers and re-registered ones with a new config are
        // dropped; the latter come back below.
        let removed: Vec<String> = self
            .servers
            .iter()
            .filter(|server| server.entry.scope == Some(McpConfigScope::Extension))
            .filter(|server| {
                next.iter()
                    .find(|(name, _)| name == &server.entry.name)
                    .map(|(_, config)| config.clone())
                    != Some(server.registered_config.clone())
            })
            .map(|server| server.entry.name.clone())
            .collect();
        let removed_servers: Vec<McpServerState> = self
            .servers
            .iter()
            .filter(|server| removed.contains(&server.entry.name))
            .cloned()
            .collect();
        self.servers.retain(|server| !removed.contains(&server.entry.name));
        for name in &removed {
            self.hide_tools(host, name);
        }
        let added: Vec<McpServerState> = registered
            .into_iter()
            .filter(|server| self.find_server(&server.entry.name).is_none())
            .collect();
        let added_names: Vec<String> = added
            .iter()
            .map(|server| server.entry.name.clone())
            .collect();
        self.servers.extend(added);
        self.ensure_discovery_active(host);
        // Close the dropped connections, then connect the added enabled
        // servers (the TS awaits both).
        for server in &removed_servers {
            if let Some(handle) = &server.handle {
                handle.lock().unwrap().close();
            }
        }
        self.drain_connection_events(host);
        let connecting: Vec<usize> = self
            .servers
            .iter()
            .enumerate()
            .filter(|(_, server)| {
                added_names.contains(&server.entry.name) && is_enabled(&server.entry)
            })
            .map(|(index, _)| index)
            .collect();
        if self.aborted || connecting.is_empty() {
            return;
        }
        for index in connecting {
            self.start_connection(index);
        }
        self.drain_connection_events(host);
        if !self.aborted {
            self.report_problems(host, Some(&added_names));
        }
    }

    /// `pi.on("session_shutdown")` (L1252).
    pub fn session_shutdown(&mut self, host: &dyn McpHost) {
        let _ = host;
        self.aborted = true;
        // A queued attempt is dropped: the TS aborts the session signal.
        self.pending_start.clear();
        let handles: Vec<Arc<Mutex<McpServerConnection>>> = self
            .servers
            .iter()
            .filter_map(|server| server.handle.clone())
            .collect();
        self.servers.clear();
        self.connection_events.lock().unwrap().clear();
        for handle in handles {
            handle.lock().unwrap().close();
        }
    }

    /// `ensureDiscoveryActive` (L500): activate codemode/tool_search from the
    /// configured exposures, and warn once when neither can be reached.
    pub fn ensure_discovery_active(&mut self, host: &dyn McpHost) {
        let mut exposures: Vec<McpExposure> = Vec::new();
        for server in &self.servers {
            if is_enabled(&server.entry) {
                for exposure in configured_exposures(&server.entry) {
                    if !exposures.contains(&exposure) {
                        exposures.push(exposure);
                    }
                }
            }
        }
        let needs_codemode = exposures.contains(&McpExposure::Codemode);
        let needs_tool_search = exposures.contains(&McpExposure::Deferred);
        if !needs_codemode && !needs_tool_search {
            return;
        }
        // Only a tool of these names that declares `reachesTools` can reach
        // MCP tools; another extension's same-named tool without it is never
        // activated.
        let tools = host.all_tools();
        let has_reaching_tool = |name: &str| {
            tools.iter().any(|tool| {
                tool.get("name").and_then(Value::as_str) == Some(name) && js_truthy(tool.get("reachesTools"))
            })
        };
        let has_codemode = has_reaching_tool(CODEMODE_TOOL_NAME);
        let has_tool_search = has_reaching_tool(TOOL_SEARCH_TOOL_NAME);
        let active = host.active_tools();
        let mut activate: Vec<String> = Vec::new();
        if needs_codemode && has_codemode && self.auto_enable_codemode && !active.contains(&CODEMODE_TOOL_NAME.to_string()) {
            activate.push(CODEMODE_TOOL_NAME.to_string());
        }
        if needs_tool_search && has_tool_search && !active.contains(&TOOL_SEARCH_TOOL_NAME.to_string()) {
            activate.push(TOOL_SEARCH_TOOL_NAME.to_string());
        }
        if !activate.is_empty() {
            let mut next = active.clone();
            next.extend(activate.iter().cloned());
            host.set_active_tools(next);
        }
        let mut reachable = active;
        reachable.extend(activate);
        if has_codemode && reachable.iter().any(|name| name == CODEMODE_TOOL_NAME) {
            return;
        }
        if has_tool_search && reachable.iter().any(|name| name == TOOL_SEARCH_TOOL_NAME) {
            return;
        }
        if self.warned_unreachable {
            return;
        }
        self.warned_unreachable = true;
        let reason = if needs_codemode && has_codemode && !self.auto_enable_codemode {
            " (autoEnableCodemode is false)"
        } else {
            ""
        };
        host.notify(
            &format!(
                "MCP tools are only reachable from the codemode or tool_search tool, but neither is active{reason}; they cannot be called."
            ),
            "warning",
        );
    }

    /// `reportProblems` (L609). `only` limits the report to servers that
    /// connected later (10d); without connections only config errors show.
    fn report_problems(&mut self, host: &dyn McpHost, only: Option<&[String]>) {
        let mut lines: Vec<String> = if only.is_none() {
            self.config_errors
                .iter()
                .map(|error| format!("config: {error}"))
                .collect()
        } else {
            Vec::new()
        };
        for server in &self.servers {
            if let Some(only) = only {
                if !only.iter().any(|name| name == &server.entry.name) {
                    continue;
                }
            }
            let Some(connection) = &server.connection else {
                continue;
            };
            if matches!(
                connection.state,
                ServerState::NeedsAuth | ServerState::Failed
            ) {
                lines.push(format!(
                    "{}: {}",
                    server.entry.name,
                    describe_state(&server.snapshot(), true)
                ));
            }
        }
        if lines.is_empty() {
            return;
        }
        let body = lines
            .iter()
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        host.notify(
            &format!("MCP servers need attention:\n{body}\nRun /mcp to fix."),
            "warning",
        );
    }

    /// `/mcp` with no action in non-TUI mode: wait for startup, then print
    /// the plain status (`formatStatus`, L1050).
    pub fn status_text(&self, host: &dyn McpHost) -> String {
        let servers: Vec<McpServerSnapshot> = self.servers.iter().map(McpServerState::snapshot).collect();
        format_status(&servers, &self.config_errors, &self.overridden, &host.agent_dir())
    }

    /// `registerCommand("mcp").getArgumentCompletions` (L1265).
    pub fn argument_completions(&self, prefix: &str) -> Option<Vec<McpCompletion>> {
        let servers: Vec<McpServerSnapshot> = self.servers.iter().map(McpServerState::snapshot).collect();
        argument_completions(prefix, &servers)
    }

    /// The usage line `notify`ed for extra arguments / unknown actions.
    pub fn usage(&self) -> &'static str {
        MCP_USAGE
    }

    /// The `/mcp` command handler (`registerCommand("mcp").handler`, L1288),
    /// covering the non-TUI paths that do not need connections or OAuth:
    /// status, argument validation, and the empty-candidate picker messages.
    /// Sign-in/logout/reconnect of real servers and the TUI manager are
    /// deferred to the next slices.
    pub fn handle_command(&mut self, args: &str, host: &dyn McpHost) {
        let parts = js_split_whitespace_trim(args);
        let action = parts.first().cloned();
        let name = parts.get(1).cloned();
        let extra = parts.len().saturating_sub(2);
        let Some(action) = action else {
            // `ctx.mode === "tui"` opens the manager; otherwise
            // `await pending` and the status.
            self.flush_pending(host);
            host.notify(&self.status_text(host), "info");
            return;
        };
        if extra > 0 {
            // The TS checks the extra arguments before awaiting `pending`.
            host.notify(MCP_USAGE, "warning");
            return;
        }
        self.flush_pending(host);
        match action.as_str() {
            "login" => {
                let picked = self.pick_server(
                    name.as_deref(),
                    host,
                    &|server| uses_oauth(server),
                    &|_| false,
                    "No enabled MCP server uses OAuth. Only HTTP servers without an Authorization header do.",
                );
                if let Some(name) = picked {
                    self.login_command(&name, host);
                }
            }
            "logout" => {
                let picked = self.pick_server(
                    name.as_deref(),
                    host,
                    &|server| uses_oauth(server),
                    &|_| false,
                    "No enabled MCP server uses OAuth. Only HTTP servers without an Authorization header do.",
                );
                if let Some(name) = picked {
                    self.sign_out(&name, host);
                }
            }
            "reconnect" => {
                let picked = self.pick_server(
                    name.as_deref(),
                    host,
                    &|server| server.handle.is_some(),
                    &|server| {
                        matches!(
                            server.connection.as_ref().map(|connection| connection.state),
                            Some(ServerState::Failed) | Some(ServerState::Disconnected)
                        )
                    },
                    "No enabled MCP server to reconnect.",
                );
                let Some(name) = picked else {
                    return;
                };
                match self.reconnect(host, &name) {
                    Some(failure) => host.notify(&failure, "error"),
                    None => {
                        self.ensure_discovery_active(host);
                        let Some(server) = self.find_server(&name) else {
                            return;
                        };
                        let state = describe_state(&server.snapshot(), true);
                        host.notify(
                            &format!("Reconnected to MCP server \"{name}\" ({state})."),
                            "info",
                        );
                    }
                }
            }
            _ => host.notify(MCP_USAGE, "warning"),
        }
    }

    /// `reconnect(server)` (L665): reconnect a live connection. Returns the
    /// JS-visible error message on failure.
    fn reconnect(&mut self, host: &dyn McpHost, name: &str) -> Option<String> {
        let handle = self
            .find_server(name)
            .and_then(|server| server.handle.clone());
        let Some(handle) = handle else {
            return Some(format!("MCP server \"{name}\" is disabled."));
        };
        let result = handle.lock().unwrap().reconnect();
        self.drain_connection_events(host);
        result.err().map(|error| error.message())
    }

    /// `loginCommand` (L1087), non-TUI paths: without a UI the sign-in
    /// cannot start. Interactive/OAuth flows are deferred with the manager.
    fn login_command(&mut self, name: &str, host: &dyn McpHost) {
        if !host.has_ui() {
            host.notify(
                &format!("Signing in to MCP server \"{name}\" requires interactive mode."),
                "error",
            );
        }
        // With a UI, `ctx.mode === "tui"` opens the manager and the other
        // branch runs the OAuth flow (`signIn`), both deferred to the
        // OAuth/TUI slices.
    }

    /// `signOut(server)` (L655) with no credential store: the flow only
    /// applies to OAuth servers, and nothing was stored.
    fn sign_out(&mut self, name: &str, host: &dyn McpHost) {
        let is_oauth = self.find_server(name).is_some_and(uses_oauth);
        if !is_oauth {
            return;
        }
        host.notify(
            &format!("No stored credentials for MCP server \"{name}\"."),
            "info",
        );
    }

    /// `pickServer` (L1070): resolve a subcommand's server, asking when the
    /// name is omitted and ambiguous.
    fn pick_server(
        &self,
        name: Option<&str>,
        host: &dyn McpHost,
        eligible: &dyn Fn(&McpServerState) -> bool,
        preferred: &dyn Fn(&McpServerState) -> bool,
        none: &str,
    ) -> Option<String> {
        if let Some(name) = name {
            let Some(server) = self.find_server(name) else {
                host.notify(&format!("No MCP server named \"{name}\"."), "error");
                return None;
            };
            if !eligible(server) {
                host.notify(none, "error");
                return None;
            }
            return Some(server.entry.name.clone());
        }
        let candidates: Vec<&McpServerState> = self.servers.iter().filter(|server| eligible(server)).collect();
        if candidates.is_empty() {
            host.notify(none, "info");
            return None;
        }
        let preferred: Vec<&McpServerState> = candidates.iter().copied().filter(|server| preferred(server)).collect();
        if candidates.len() == 1 {
            return Some(candidates[0].entry.name.clone());
        }
        if preferred.len() == 1 {
            return Some(preferred[0].entry.name.clone());
        }
        let names: Vec<String> = candidates.iter().map(|server| server.entry.name.clone()).collect();
        let choice = host.select("MCP server", &names)?;
        candidates
            .iter()
            .find(|server| server.entry.name == choice)
            .map(|server| server.entry.name.clone())
    }

    /// `exposureOf` for one configured server (manager/status helper).
    pub fn exposure_of(&self, name: &str) -> Option<McpExposure> {
        self.find_server(name).map(|server| exposure_of(&server.entry))
    }

    /// Whether some server has direct tools, which makes the first prompt
    /// wait (`hasDirectTools`).
    pub fn has_direct_tools(&self) -> bool {
        self.servers
            .iter()
            .any(|server| is_enabled(&server.entry) && has_direct_tools(&server.entry))
    }

    /// `servers` as `McpServerSnapshot`s, for the status/manager helpers.
    pub fn snapshots(&self) -> Vec<McpServerSnapshot> {
        self.servers.iter().map(McpServerState::snapshot).collect()
    }

    /// Trimmed first line helper used by the manager texts (`firstLine`).
    pub fn first_line(text: &str) -> &str {
        let line = match text.find('\n') {
            Some(index) => &text[..index],
            None => text,
        };
        js_trim(line)
    }
}

/// `args.trim().split(/\s+/).filter(Boolean)` — the `/mcp` command's
/// argument parsing.
fn js_split_whitespace_trim(text: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_space = true;
    for c in text.trim().chars() {
        if pi_core::js_string::is_js_whitespace_or_terminator(c) {
            if !in_space {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
                in_space = true;
            }
        } else {
            in_space = false;
            current.push(c);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// `RESOURCE_TOOL_NAMES.has(name)`.
fn is_resource_tool(name: &str) -> bool {
    [
        LIST_MCP_RESOURCES_TOOL,
        LIST_MCP_RESOURCE_TEMPLATES_TOOL,
        READ_MCP_RESOURCE_TOOL,
    ]
    .contains(&name)
}

/// `reachesServer(reach, server)` (L224): `"all"`, or a word containing the
/// server's namespace. The harness models a JavaScript `reachesTools`
/// function as `"all"` or an array of words.
fn reaches_server(reach: &Value, server: &str) -> bool {
    match reach {
        Value::String(reach) => reach == "all",
        Value::Array(words) => {
            let namespace = mcp_namespace(server);
            words
                .iter()
                .filter_map(Value::as_str)
                .any(|word| word.contains(&namespace))
        }
        // A truthy non-array function result (`reachesTools` modeled as a
        // boolean in fixtures) reaches every tool.
        Value::Bool(true) => true,
        _ => false,
    }
}

fn uses_oauth(server: &McpServerState) -> bool {
    server
        .connection
        .as_ref()
        .is_some_and(|connection| connection.oauth_url.is_some())
}

/// JS truthiness for JSON values (`null`/`false`/`0`/`""` falsy).
fn js_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(number)) => number.as_f64().map(|number| number != 0.0).unwrap_or(true),
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}
