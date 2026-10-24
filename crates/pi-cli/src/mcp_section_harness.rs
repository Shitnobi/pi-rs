//! Parity-harness entry for the `mcp-section` protocol:
//! `pi_extensions::mcp::section` (TS `extensions/mcp/index.ts`
//! `renderServersSection`). TS mirror: parity/ts-harness.js `mcp-section`.
//!
//! args:
//! ```json
//! {
//!   "servers": [
//!     { "name": str, "config": json, "source"?: str,
//!       "scope"?: "global" | "project" | "extension", "override"?: str,
//!       "instructions"?: str }
//!   ]
//! }
//! ```
//!
//! `config` is run through `validate_mcp_server_config`, like every entry
//! the extension renders (loaded configs and extension registrations are
//! validated). Result: `{ "result": "<section>" | null }`.

use serde_json::{json, Value};

use pi_core::mcp_servers::validate_mcp_server_config;
use pi_extensions::mcp::section::{render_servers_section, McpServerListing};
use pi_session::mcp_config::{McpConfigScope, McpServerEntry};

fn parse_scope(value: Option<&Value>) -> Option<McpConfigScope> {
    match value.and_then(Value::as_str) {
        Some("global") => Some(McpConfigScope::Global),
        Some("project") => Some(McpConfigScope::Project),
        Some("extension") => Some(McpConfigScope::Extension),
        _ => None,
    }
}

pub fn run(input: &Value) -> Result<Value, String> {
    let servers = input.get("servers").and_then(Value::as_array).ok_or("missing servers")?;
    let mut entries: Vec<McpServerEntry> = Vec::with_capacity(servers.len());
    let mut instructions: Vec<Option<String>> = Vec::with_capacity(servers.len());
    for server in servers {
        let name = server.get("name").and_then(Value::as_str).ok_or("server missing name")?;
        let raw = server.get("config").ok_or("server missing config")?;
        let config = validate_mcp_server_config(name, raw)?;
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
        instructions.push(server.get("instructions").and_then(Value::as_str).map(str::to_string));
    }
    let listings: Vec<McpServerListing<'_>> = entries
        .iter()
        .zip(&instructions)
        .map(|(entry, instructions)| McpServerListing {
            entry,
            instructions: instructions.as_deref(),
        })
        .collect();
    Ok(json!({ "result": render_servers_section(&listings) }))
}
