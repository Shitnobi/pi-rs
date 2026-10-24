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
//! - `turn_start` / `tool_call` / `session_shutdown`: the parts that do not
//!   need a live connection yet.
//! - `/mcp` command: non-TUI status and argument completions.
//!
//! Deferred to the next slices (see `docs/plans/phase4-tranche7-mcp-extension.md`):
//! connections + tool registration, the TUI manager/sign-in, and the runner
//! adapter. The state machine is intentionally a plain `&mut self` object:
//! [`McpServerConnection`](super::runtime::McpServerConnection) is `!Send`
//! (it holds `Rc`/`RefCell`), while `ExtensionRunner` handlers must be
//! `Send + Sync`; the adapter design is recorded in the plan.

use std::sync::Arc;

use pi_core::js_string::js_trim;
use pi_core::mcp_servers::{mcp_namespace, McpExposure, RegisteredMcpServer};
use pi_session::mcp_config::{LoadedMcpConfig, McpConfigScope, McpServerEntry};
use serde_json::Value;

use super::section::{configured_exposures, exposure_of, has_direct_tools, is_enabled, render_servers_section, McpServerListing, MCP_SERVERS_SECTION};
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
}

/// The `loadConfig` seam (`McpExtensionOptions.loadConfig`).
pub type LoadConfigFn = Arc<dyn Fn(&dyn McpHost) -> LoadedMcpConfig + Send + Sync>;

/// `McpExtensionOptions` (L69) — the subset this slice needs.
#[derive(Clone)]
pub struct McpExtensionOptions {
    /// `loadConfig`: defaults to `loadMcpConfig({agentDir, cwd, projectTrusted})`
    /// in the real adapter; the harness injects the result directly.
    pub load_config: LoadConfigFn,
    /// `startupWaitMs ?? 10_000` — used once direct-server waits are wired.
    pub startup_wait_ms: u64,
}

impl Default for McpExtensionOptions {
    fn default() -> Self {
        McpExtensionOptions {
            load_config: Arc::new(|_| LoadedMcpConfig::default()),
            startup_wait_ms: DEFAULT_STARTUP_WAIT_MS,
        }
    }
}

/// A configured server (`McpServer`, L108). Connections, attempts, and the
/// closing/ready promises belong to the connection slice; `None` here always
/// means "no connection yet", which is also the TS state before
/// `startConnection` runs.
#[derive(Debug, Clone)]
pub struct McpServerState {
    pub entry: McpServerEntry,
    /// `registeredConfig` for extension-registered servers.
    pub registered_config: Option<String>,
    /// Result of the last `/mcp` action that failed.
    pub message: Option<String>,
    /// Connection status once the connection slice lands.
    pub connection: Option<McpConnectionStatus>,
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
            })
            .chain(registered)
            .collect();
        // Codemode or tool_search is activated from the config: the first
        // prompt does not wait for servers whose tools are not declared.
        self.ensure_discovery_active(host);
        // Connections start here in the full port (10d); the TS reports
        // startup problems once those attempts settle, so with enabled
        // servers the report is deferred. With none it happens now.
        let has_enabled = self.servers.iter().any(|server| is_enabled(&server.entry));
        if !has_enabled {
            self.report_problems(host, None);
        }
    }

    /// `pi.on("before_agent_start")` (L1178): inject or remove the
    /// `mcp_servers` section on the event's `systemPromptOptions.sections`.
    pub fn before_agent_start(&mut self, host: &dyn McpHost, event: &mut Value) {
        // `await waitForDirectServers(ctx)` — without connections there is
        // nothing to wait for; the flag still flips like the TS.
        if !self.waited_for_startup {
            self.waited_for_startup = true;
            // 10d: wait for servers with direct tools, bounded by
            // `startup_wait_ms`, and notify when still connecting.
            let _ = &self.options.startup_wait_ms;
        }
        let listings: Vec<McpServerListing<'_>> = self
            .servers
            .iter()
            .map(|server| McpServerListing {
                entry: &server.entry,
                instructions: None,
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
        let _ = host;
    }

    /// `pi.on("tool_call")` (L1191): wait for the servers the tool may reach.
    /// Without connections every `ready` is absent, so this returns
    /// immediately; the routing logic (reachesTools/owner/resource tools)
    /// lands with tool registration.
    pub fn tool_call(&mut self, host: &dyn McpHost, event: &Value) {
        let _ = host;
        let _ = event;
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
        self.servers.retain(|server| !removed.contains(&server.entry.name));
        // 10d: `hideTools` for removed servers.
        let added: Vec<McpServerState> = registered
            .into_iter()
            .filter(|server| self.find_server(&server.entry.name).is_none())
            .collect();
        self.servers.extend(added);
        self.ensure_discovery_active(host);
        // 10d: close removed connections, start added enabled servers, and
        // report problems for them once their attempts settle.
    }

    /// `pi.on("session_shutdown")` (L1252).
    pub fn session_shutdown(&mut self, host: &dyn McpHost) {
        let _ = host;
        self.aborted = true;
        // 10d: close every connection after the tracked background actions.
        self.servers.clear();
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
                super::runtime::ServerState::NeedsAuth | super::runtime::ServerState::Failed
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
            // `ctx.mode === "tui"` opens the manager; otherwise the status.
            host.notify(&self.status_text(host), "info");
            return;
        };
        if extra > 0 {
            host.notify(MCP_USAGE, "warning");
            return;
        }
        match action.as_str() {
            "login" | "logout" => {
                let picked = self.pick_server(
                    name.as_deref(),
                    host,
                    &|server| uses_oauth(server),
                    &|_| false,
                    "No enabled MCP server uses OAuth. Only HTTP servers without an Authorization header do.",
                );
                // The sign-in/sign-out flow needs the connection slice.
                let _ = picked;
            }
            "reconnect" => {
                let picked = self.pick_server(
                    name.as_deref(),
                    host,
                    &|server| server.connection.is_some(),
                    &|server| {
                        matches!(
                            server.connection.as_ref().map(|connection| connection.state),
                            Some(super::runtime::ServerState::Failed)
                                | Some(super::runtime::ServerState::Disconnected)
                        )
                    },
                    "No enabled MCP server to reconnect.",
                );
                let _ = picked;
            }
            _ => host.notify(MCP_USAGE, "warning"),
        }
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
