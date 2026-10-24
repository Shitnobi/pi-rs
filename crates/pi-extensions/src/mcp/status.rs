//! `extensions/mcp/index.ts` — server descriptions, ordering, plain-text
//! status, and `/mcp` argument completions (L257–L310, L1050–L1100,
//! L1260–L1300).
//!
//! The TUI manager (`ui.ts`) is not ported: this module keeps the pure
//! formatting/completion logic that the manager and the non-TUI `/mcp`
//! command share. `localeCompare` ordering is approximated like the rest of
//! the port (see [`pi_core::js_string::js_locale_compare`] — ASCII server
//! names only; mixed-case/accents are a documented divergence).

use pi_core::js_string::js_locale_compare;
use pi_session::mcp_config::McpServerEntry;

use super::runtime::ServerState;
use super::section::{exposure_of, is_enabled};

/// The part of an `McpServerConnection` these helpers read.
#[derive(Debug, Clone, PartialEq)]
pub struct McpConnectionStatus {
    pub state: ServerState,
    pub tools: usize,
    pub has_resources: bool,
    pub error: Option<String>,
    /// Set for HTTP servers without an `Authorization` header.
    pub oauth_url: Option<String>,
}

/// One server's state, as the manager/status view sees it (`McpServer`).
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerSnapshot {
    pub entry: McpServerEntry,
    pub connection: Option<McpConnectionStatus>,
    /// Result of the last `/mcp` action that failed (`server.message`).
    pub message: Option<String>,
}

/// `MCP_USAGE` (L292).
pub const MCP_USAGE: &str =
    "Usage: /mcp, /mcp login [server], /mcp logout [server], /mcp reconnect [server]";

/// `firstLine` (L129).
fn first_line(text: &str) -> &str {
    match text.find('\n') {
        Some(index) => &text[..index],
        None => text,
    }
}

/// Short state for lists and the startup report (`describeState`, L257).
/// `withError` appends the first line of a failure.
pub fn describe_state(server: &McpServerSnapshot, with_error: bool) -> String {
    if !is_enabled(&server.entry) {
        return "disabled".to_string();
    }
    let Some(connection) = &server.connection else {
        return "starting".to_string();
    };
    match connection.state {
        ServerState::NeedsAuth => "needs sign-in".to_string(),
        ServerState::Failed => {
            if with_error {
                format!("failed: {}", first_line(connection.error.as_deref().unwrap_or("unknown error")))
            } else {
                "failed".to_string()
            }
        }
        ServerState::Connected => format!(
            "connected · {} tool{}{}",
            connection.tools,
            if connection.tools == 1 { "" } else { "s" },
            if connection.has_resources { " · resources" } else { "" }
        ),
        ServerState::Connecting => "connecting…".to_string(),
        other => other.as_str().to_string(),
    }
}

/// Servers that need the user first (`attentionRank`, L279): lower sorts
/// first.
pub fn attention_rank(server: &McpServerSnapshot) -> u8 {
    if !is_enabled(&server.entry) {
        return 5;
    }
    match server.connection.as_ref().map(|connection| connection.state) {
        Some(ServerState::NeedsAuth) => 0,
        Some(ServerState::Failed) => 1,
        Some(ServerState::Disconnected) => 2,
        Some(ServerState::Connected) => 4,
        _ => 3,
    }
}

/// Servers as the manager lists them: attention first, then name
/// (`serversMenu`'s item order).
pub fn sort_for_menu(servers: &mut [McpServerSnapshot]) {
    servers.sort_by(|a, b| {
        attention_rank(a)
            .cmp(&attention_rank(b))
            .then_with(|| js_locale_compare(&a.entry.name, &b.entry.name))
    });
}

/// How the server is reached (`describeTransport`, L288).
pub fn describe_transport(entry: &McpServerEntry) -> String {
    if let Some(url) = entry.config.url() {
        return url.to_string();
    }
    let mut parts = vec![entry.config.command().unwrap_or("").to_string()];
    if let Some(args) = entry.config.fields.get("args").and_then(|value| value.as_array()) {
        parts.extend(
            args.iter()
                .filter_map(|arg| arg.as_str().map(str::to_string))
                .collect::<Vec<_>>(),
        );
    }
    parts.join(" ")
}

fn uses_oauth(server: &McpServerSnapshot) -> bool {
    server.connection.as_ref().is_some_and(|connection| connection.oauth_url.is_some())
}

/// `formatStatus` (L1050): the non-TUI `/mcp` output.
///
/// `agent_dir` is `getAgentDir()`; the TS resolves it against `mcp.json`
/// (`path.resolve(agentDir, "mcp.json")`), which on Unix is the join.
pub fn format_status(
    servers: &[McpServerSnapshot],
    config_errors: &[String],
    overridden: &[String],
    agent_dir: &str,
) -> String {
    if servers.is_empty() && config_errors.is_empty() && overridden.is_empty() {
        return format!(
            "No MCP servers configured. Add them to {} or .pi/mcp.json.",
            std::path::Path::new(agent_dir).join("mcp.json").display()
        );
    }
    let mut lines: Vec<String> = servers
        .iter()
        .map(|server| {
            let name = &server.entry.name;
            let exposure = exposure_of(&server.entry);
            let connection = server.connection.as_ref();
            if connection.is_some_and(|connection| connection.state == ServerState::NeedsAuth) {
                return format!(
                    "{name}: needs sign-in, run /mcp login {name} ({})",
                    exposure.as_str()
                );
            }
            let tools = match connection {
                Some(connection) if connection.state == ServerState::Connected => {
                    format!(", {} tools", connection.tools)
                }
                _ => String::new(),
            };
            let state = if !is_enabled(&server.entry) {
                "disabled".to_string()
            } else if connection.is_some_and(|connection| connection.state == ServerState::Disconnected) {
                "disconnected, reconnects on next call".to_string()
            } else {
                match connection {
                    Some(connection) => connection.state.as_str().to_string(),
                    None => "starting".to_string(),
                }
            };
            let error = match connection {
                Some(connection)
                    if connection.error.as_deref().is_some_and(|error| !error.is_empty())
                        && connection.state != ServerState::Connected =>
                {
                    let error = connection.error.as_deref().unwrap_or("");
                    format!("\n    {}", error.replace('\n', "\n    "))
                }
                _ => String::new(),
            };
            format!("{name}: {state}{tools} ({}){error}", exposure.as_str())
        })
        .collect();
    lines.extend(config_errors.iter().map(|error| format!("config error: {error}")));
    lines.extend(overridden.iter().map(|line| format!("overridden: {line}")));
    lines.join("\n")
}

/// One completion item from `getArgumentCompletions` (L1264).
#[derive(Debug, Clone, PartialEq)]
pub struct McpCompletion {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

impl McpCompletion {
    fn new(value: impl Into<String>, label: impl Into<String>, description: Option<String>) -> Self {
        McpCompletion { value: value.into(), label: label.into(), description }
    }
}

/// `String.prototype.split(/\s+/)` for the trimmed prefix — JS whitespace
/// runs collapse, leading/trailing runs produce empty strings.
fn js_split_whitespace(text: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_space = false;
    for c in text.chars() {
        if pi_core::js_string::is_js_whitespace_or_terminator(c) {
            if !in_space {
                parts.push(std::mem::take(&mut current));
                in_space = true;
            }
        } else {
            in_space = false;
            current.push(c);
        }
    }
    parts.push(current);
    parts
}

/// `/mcp` argument completions (`getArgumentCompletions`, L1265).
pub fn argument_completions(prefix: &str, servers: &[McpServerSnapshot]) -> Option<Vec<McpCompletion>> {
    let trimmed = prefix.trim_start();
    let parts = js_split_whitespace(trimmed);
    let action = parts.first().cloned().unwrap_or_default();
    let server = parts.get(1).cloned();
    let rest = parts.len().saturating_sub(2);
    if rest > 0 {
        return None;
    }
    if server.is_none() {
        let items: Vec<McpCompletion> = ["login", "logout", "reconnect"]
            .iter()
            .filter(|item| item.starts_with(&action))
            .map(|item| McpCompletion::new(format!("{item} "), *item, None))
            .collect();
        return if items.is_empty() { None } else { Some(items) };
    }
    if action != "login" && action != "logout" && action != "reconnect" {
        return None;
    }
    let server = server.unwrap_or_default();
    let items: Vec<McpCompletion> = servers
        .iter()
        .filter(|candidate| {
            if action == "reconnect" {
                candidate.connection.is_some()
            } else {
                uses_oauth(candidate)
            }
        })
        .filter(|candidate| candidate.entry.name.starts_with(&server))
        .map(|candidate| {
            McpCompletion::new(
                format!("{action} {}", candidate.entry.name),
                candidate.entry.name.clone(),
                Some(describe_state(candidate, true)),
            )
        })
        .collect();
    if items.is_empty() {
        return None;
    }
    // TS preserves the `servers` order (no sort here).
    Some(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_core::mcp_servers::{McpServerConfig, McpServerKind};
    use serde_json::{json, Map, Value};

    fn entry(name: &str, config: Value) -> McpServerEntry {
        let fields: Map<String, Value> = config.as_object().unwrap().clone();
        McpServerEntry {
            name: name.to_string(),
            config: McpServerConfig { kind: McpServerKind::Stdio, fields },
            source: "mcp.json".to_string(),
            scope: None,
            r#override: None,
        }
    }

    fn server(name: &str, config: Value, state: Option<ServerState>) -> McpServerSnapshot {
        McpServerSnapshot {
            entry: entry(name, config),
            connection: state.map(|state| McpConnectionStatus {
                state,
                tools: 2,
                has_resources: false,
                error: None,
                oauth_url: None,
            }),
            message: None,
        }
    }

    #[test]
    fn describe_states() {
        let starting = server("a", json!({ "command": "x" }), None);
        assert_eq!(describe_state(&starting, true), "starting");
        let disabled = server("a", json!({ "command": "x", "enabled": false }), Some(ServerState::Connected));
        assert_eq!(describe_state(&disabled, true), "disabled");
        let auth = server("a", json!({ "command": "x" }), Some(ServerState::NeedsAuth));
        assert_eq!(describe_state(&auth, true), "needs sign-in");
        let mut failed = server("a", json!({ "command": "x" }), Some(ServerState::Failed));
        failed.connection.as_mut().unwrap().error = Some("boom\nstack".to_string());
        assert_eq!(describe_state(&failed, true), "failed: boom");
        assert_eq!(describe_state(&failed, false), "failed");
        let connecting = server("a", json!({ "command": "x" }), Some(ServerState::Connecting));
        assert_eq!(describe_state(&connecting, true), "connecting…");
        let mut one = server("a", json!({ "command": "x" }), Some(ServerState::Connected));
        one.connection.as_mut().unwrap().tools = 1;
        assert_eq!(describe_state(&one, true), "connected · 1 tool");
        let mut resources = server("a", json!({ "command": "x" }), Some(ServerState::Connected));
        resources.connection.as_mut().unwrap().has_resources = true;
        assert_eq!(describe_state(&resources, true), "connected · 2 tools · resources");
    }

    #[test]
    fn status_lines_and_first_run_message() {
        let empty = format_status(&[], &[], &[], "/home/u/.pi");
        assert_eq!(
            empty,
            "No MCP servers configured. Add them to /home/u/.pi/mcp.json or .pi/mcp.json."
        );
        let mut failed = server("bad", json!({ "command": "x" }), Some(ServerState::Failed));
        failed.connection.as_mut().unwrap().error = Some("one\ntwo".to_string());
        let servers = vec![
            failed,
            server("off", json!({ "command": "x", "enabled": false }), Some(ServerState::Connected)),
            server("gone", json!({ "command": "x" }), Some(ServerState::Disconnected)),
            server("ok", json!({ "command": "x", "exposure": "deferred" }), Some(ServerState::Connected)),
            server("auth", json!({ "url": "https://x" }), Some(ServerState::NeedsAuth)),
        ];
        let status = format_status(
            &servers,
            &["bad file".to_string()],
            &["\"a\" overridden".to_string()],
            "/home/u/.pi",
        );
        assert_eq!(
            status,
            "bad: failed (codemode)\n    one\n    two\n\
             off: disabled, 2 tools (codemode)\n\
             gone: disconnected, reconnects on next call (codemode)\n\
             ok: connected, 2 tools (deferred)\n\
             auth: needs sign-in, run /mcp login auth (codemode)\n\
             config error: bad file\n\
             overridden: \"a\" overridden"
        );
        // The tool count is independent of `enabled`/`disconnected` in the TS
        // template: only the state text changes, so a disabled server whose
        // last connection is still `connected` keeps its count.
    }

    #[test]
    fn attention_order() {
        let mut list = vec![
            server("z", json!({ "command": "x" }), Some(ServerState::Connected)),
            server("a", json!({ "command": "x", "enabled": false }), None),
            server("m", json!({ "command": "x" }), Some(ServerState::NeedsAuth)),
            server("b", json!({ "command": "x" }), Some(ServerState::Disconnected)),
            server("c", json!({ "command": "x" }), None),
        ];
        sort_for_menu(&mut list);
        let names: Vec<&str> = list.iter().map(|server| server.entry.name.as_str()).collect();
        assert_eq!(names, vec!["m", "b", "c", "z", "a"]);
    }

    #[test]
    fn transport_text() {
        let stdio = entry("a", json!({ "command": "node", "args": ["s.js", "--x"] }));
        assert_eq!(describe_transport(&stdio), "node s.js --x");
        let url = entry("a", json!({ "url": "https://example.test/mcp" }));
        assert_eq!(describe_transport(&url), "https://example.test/mcp");
    }

    #[test]
    fn completions_action_prefix() {
        let servers = vec![server("alpha", json!({ "command": "x" }), Some(ServerState::Connected))];
        let actions = argument_completions("lo", &servers).unwrap();
        assert_eq!(
            actions,
            vec![
                McpCompletion::new("login ", "login", None),
                McpCompletion::new("logout ", "logout", None),
            ]
        );
        assert_eq!(argument_completions("zzz", &servers), None);
        assert_eq!(argument_completions("login a b", &servers), None);
    }
}
