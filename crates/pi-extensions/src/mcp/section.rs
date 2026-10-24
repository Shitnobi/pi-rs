//! `extensions/mcp/index.ts` — the `mcp_servers` system-prompt section
//! (`renderServersSection`, L156–L255) plus the exposure predicates it
//! shares with the rest of the extension.
//!
//! Everything here is pure: a snapshot of the enabled/config state in,
//! the section text (or `None`) out. The JS quirks that matter are the
//! UTF-16 `length`/`slice` in the size budget and truncation, `trim` /
//! `trimEnd`, `localeCompare` sorting, and the description → instructions
//! fallback.

use pi_core::js_string::{js_length, js_locale_compare, js_trim, js_trim_end};
use pi_core::mcp_servers::McpExposure;
use pi_session::mcp_config::McpServerEntry;

/// Name of the system prompt section that lists the servers whose tools are
/// not declared (`MCP_SERVERS_SECTION`, L156).
pub const MCP_SERVERS_SECTION: &str = "mcp_servers";

/// Characters of a server description in the section, as Codex allows for
/// deferred namespaces (`MAX_SERVER_DESCRIPTION_CHARS`, L158).
pub const MAX_SERVER_DESCRIPTION_CHARS: usize = 250;

/// Characters of the whole section. Descriptions shrink to fit; when the
/// server lines alone do not fit, the last servers are left out and counted
/// in a closing line (`MAX_SERVERS_SECTION_CHARS`, L163).
pub const MAX_SERVERS_SECTION_CHARS: usize = 4096;

/// What the section needs of a server (`McpServerListing`, L179).
#[derive(Debug, Clone)]
pub struct McpServerListing<'a> {
    pub entry: &'a McpServerEntry,
    /// `connection?.instructions` — the connected server's instructions.
    pub instructions: Option<&'a str>,
}

/// `server.entry.config.enabled !== false` (`isEnabled`, L134).
pub fn is_enabled(entry: &McpServerEntry) -> bool {
    entry.config.fields.get("enabled") != Some(&serde_json::Value::Bool(false))
}

/// `entry.config.exposure ?? "codemode"` (`exposureOf`, L138).
pub fn exposure_of(entry: &McpServerEntry) -> McpExposure {
    entry.config.exposure().unwrap_or(McpExposure::Codemode)
}

/// `new Set([exposureOf(entry), ...Object.values(toolExposure ?? {})])`
/// (`configuredExposures`, L142) — insertion order, deduplicated.
pub fn configured_exposures(entry: &McpServerEntry) -> Vec<McpExposure> {
    let mut exposures = vec![exposure_of(entry)];
    if let Some(tools) = entry.config.tool_exposure() {
        for value in tools.values() {
            if let Some(exposure) = value.as_str().and_then(McpExposure::parse) {
                if !exposures.contains(&exposure) {
                    exposures.push(exposure);
                }
            }
        }
    }
    exposures
}

/// `hasDirectTools` (L147): some of the server's tools are declared.
pub fn has_direct_tools(entry: &McpServerEntry) -> bool {
    configured_exposures(entry).contains(&McpExposure::Direct)
}

/// `hasIndirectTools` (L152): some tools are reached through codemode or
/// tool_search.
pub fn has_indirect_tools(entry: &McpServerEntry) -> bool {
    let exposures = configured_exposures(entry);
    exposures.contains(&McpExposure::Codemode) || exposures.contains(&McpExposure::Deferred)
}

/// `text.split("\n", 1)[0] ?? ""` (`firstLine`, L129) — keeps a trailing
/// `\r`; callers trim.
fn first_line(text: &str) -> &str {
    match text.find('\n') {
        Some(index) => &text[..index],
        None => text,
    }
}

/// JS `String.prototype.slice(0, max)` for a non-negative `max`, counted in
/// UTF-16 code units. A surrogate pair is never split (V8 would produce a
/// lone surrogate, which cannot round-trip through JSON).
fn utf16_prefix(text: &str, max: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in text.chars() {
        let len = c.len_utf16();
        if units + len > max {
            break;
        }
        units += len;
        out.push(c);
    }
    out
}

/// `truncate(text, max)` (L171): `max <= 1` yields `""`; otherwise the
/// prefix keeps a `…` at exactly `max` UTF-16 units.
fn truncate(text: &str, max: usize) -> String {
    if js_length(text) <= max {
        return text.to_string();
    }
    if max <= 1 {
        return String::new();
    }
    format!("{}…", js_trim_end(&utf16_prefix(text, max - 1)))
}

/// First line of the configured description, or of the server instructions
/// once connected (`serverSummary`, L190).
pub fn server_summary(listing: &McpServerListing<'_>) -> String {
    let description = listing
        .entry
        .config
        .fields
        .get("description")
        .and_then(|value| value.as_str());
    let text = match description {
        Some(description) => {
            let trimmed = js_trim(description);
            if !trimmed.is_empty() {
                trimmed
            } else {
                listing.instructions.unwrap_or("")
            }
        }
        None => listing.instructions.unwrap_or(""),
    };
    js_trim(first_line(text)).to_string()
}

/// The section's first line. It explains only the ways of reaching tools
/// that the listed servers use (`serversSectionIntro`, L168).
fn servers_section_intro(reaches_codemode: bool, reaches_tool_search: bool) -> String {
    let mut intro = "MCP servers whose tools are not declared to you.".to_string();
    if reaches_codemode {
        intro.push_str(" Call the tools of `codemode` servers from codemode scripts.");
    }
    if reaches_tool_search {
        intro.push_str(" Load the tools of `tool_search` servers with `tool_search`.");
    }
    intro
}

/// `omitted(count)` (L201).
fn omitted(count: usize) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }
    vec![format!(
        "- … {count} more server{}; find their tools with searchTools()",
        if count == 1 { "" } else { "s" }
    )]
}

/// The `mcp_servers` section: every enabled server with codemode or deferred
/// tools, with how its tools are reached and a one-line summary
/// (`renderServersSection`, L206). `None` when there are no such servers.
pub fn render_servers_section(servers: &[McpServerListing<'_>]) -> Option<String> {
    let mut listed: Vec<&McpServerListing<'_>> = servers
        .iter()
        .filter(|server| is_enabled(server.entry) && has_indirect_tools(server.entry))
        .collect();
    if listed.is_empty() {
        return None;
    }
    listed.sort_by(|a, b| js_locale_compare(&a.entry.name, &b.entry.name));

    let reaches: Vec<bool> = listed
        .iter()
        .map(|server| configured_exposures(server.entry).contains(&McpExposure::Codemode))
        .collect();
    let intro = servers_section_intro(reaches.contains(&true), reaches.contains(&false));
    let heads: Vec<String> = listed
        .iter()
        .zip(&reaches)
        .map(|(server, codemode)| {
            let reach = if *codemode { "codemode" } else { "tool_search" };
            format!("- {} ({reach})", pi_core::mcp_servers::mcp_namespace(&server.entry.name))
        })
        .collect();

    // `size(kept)`: UTF-16 length of the intro, the first `kept` server lines
    // without descriptions, and the omission line.
    let size = |kept: usize| -> usize {
        let mut parts: Vec<String> = vec![intro.clone()];
        parts.extend(heads[..kept].iter().cloned());
        parts.extend(omitted(listed.len() - kept));
        js_length(&parts.join("\n"))
    };

    let mut kept = listed.len();
    while kept > 0 && size(kept) > MAX_SERVERS_SECTION_CHARS {
        kept -= 1;
    }
    // Each description also takes a ": " separator.
    let per_server = if kept == 0 {
        0
    } else {
        let room = MAX_SERVERS_SECTION_CHARS.saturating_sub(size(kept)) / kept;
        MAX_SERVER_DESCRIPTION_CHARS.min(room.saturating_sub(2))
    };
    let lines: Vec<String> = listed[..kept]
        .iter()
        .zip(&heads)
        .map(|(server, head)| {
            let summary = if per_server > 0 {
                truncate(&server_summary(server), per_server)
            } else {
                String::new()
            };
            if summary.is_empty() {
                head.clone()
            } else {
                format!("{head}: {summary}")
            }
        })
        .collect();

    let mut parts: Vec<String> = vec![intro];
    parts.extend(lines);
    parts.extend(omitted(listed.len() - kept));
    Some(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map, Value};

    fn entry(name: &str, config: Value) -> McpServerEntry {
        let fields: Map<String, Value> = config.as_object().unwrap().clone();
        McpServerEntry {
            name: name.to_string(),
            config: pi_core::mcp_servers::McpServerConfig {
                kind: pi_core::mcp_servers::McpServerKind::Stdio,
                fields,
            },
            source: "mcp.json".to_string(),
            scope: None,
            r#override: None,
        }
    }

    #[test]
    fn no_servers_renders_nothing() {
        assert_eq!(render_servers_section(&[]), None);
        let direct = entry("a", json!({ "command": "x", "exposure": "direct" }));
        assert_eq!(render_servers_section(&[McpServerListing { entry: &direct, instructions: None }]), None);
        let disabled = entry("a", json!({ "command": "x", "enabled": false }));
        assert_eq!(render_servers_section(&[McpServerListing { entry: &disabled, instructions: None }]), None);
    }

    #[test]
    fn lists_indirect_servers_sorted_with_descriptions() {
        let b = entry(
            "beta",
            json!({ "command": "x", "description": "  Second\nmore " }),
        );
        let a = entry(
            "alpha",
            json!({ "command": "x", "exposure": "deferred", "description": "First" }),
        );
        let section = render_servers_section(&[
            McpServerListing { entry: &b, instructions: None },
            McpServerListing { entry: &a, instructions: None },
        ])
        .unwrap();
        assert_eq!(
            section,
            "MCP servers whose tools are not declared to you. Call the tools of `codemode` servers from codemode scripts. Load the tools of `tool_search` servers with `tool_search`.\n- mcp__alpha (tool_search): First\n- mcp__beta (codemode): Second"
        );
    }

    #[test]
    fn instructions_are_the_fallback() {
        let a = entry("a", json!({ "command": "x" }));
        let with_instructions = render_servers_section(&[McpServerListing {
            entry: &a,
            instructions: Some("  Instr\nsecond "),
        }])
        .unwrap();
        assert!(with_instructions.ends_with("\n- mcp__a (codemode): Instr"));
        let empty_description = entry("a", json!({ "command": "x", "description": "   " }));
        let rendered = render_servers_section(&[McpServerListing {
            entry: &empty_description,
            instructions: Some("Instr"),
        }])
        .unwrap();
        assert!(rendered.ends_with("\n- mcp__a (codemode): Instr"));
    }

    #[test]
    fn over_budget_omits_servers() {
        // 4097 chars of head room: each long name line is > 4096/1.
        let long = "z".repeat(4200);
        let a = entry(&long, json!({ "command": "x" }));
        let b = entry("b", json!({ "command": "x" }));
        let section = render_servers_section(&[
            McpServerListing { entry: &a, instructions: None },
            McpServerListing { entry: &b, instructions: None },
        ])
        .unwrap();
        assert_eq!(
            section,
            "MCP servers whose tools are not declared to you. Call the tools of `codemode` servers from codemode scripts.\n- mcp__b (codemode)\n- … 1 more server; find their tools with searchTools()"
        );
        assert!(js_length(&section) <= MAX_SERVERS_SECTION_CHARS);
    }

    #[test]
    fn all_servers_omitted_still_renders_intro() {
        let names: Vec<McpServerEntry> = (0..3)
            .map(|i| entry(&format!("{i}{}", "z".repeat(4000)), json!({ "command": "x" })))
            .collect();
        let listings: Vec<McpServerListing<'_>> =
            names.iter().map(|entry| McpServerListing { entry, instructions: None }).collect();
        let section = render_servers_section(&listings).unwrap();
        assert_eq!(
            section,
            "MCP servers whose tools are not declared to you. Call the tools of `codemode` servers from codemode scripts.\n- … 3 more servers; find their tools with searchTools()"
        );
        assert!(js_length(&section) <= MAX_SERVERS_SECTION_CHARS);
    }

    #[test]
    fn description_truncation_is_utf16_aware() {
        let description = format!("{}🔥tail", "d".repeat(300));
        let a = entry("a", json!({ "command": "x", "description": description }));
        let section = render_servers_section(&[McpServerListing { entry: &a, instructions: None }]).unwrap();
        let line = section.rsplit('\n').next().unwrap();
        let summary = line.split_once(": ").unwrap().1;
        assert!(summary.ends_with('…'));
        assert!(js_length(summary) <= MAX_SERVER_DESCRIPTION_CHARS);
    }

    #[test]
    fn configured_exposures_dedup_and_order() {
        let a = entry(
            "a",
            json!({
                "command": "x",
                "exposure": "deferred",
                "toolExposure": { "x": "deferred", "y": "direct", "z": "codemode" }
            }),
        );
        assert_eq!(
            configured_exposures(&a),
            vec![McpExposure::Deferred, McpExposure::Direct, McpExposure::Codemode]
        );
        assert!(has_direct_tools(&a));
        assert!(has_indirect_tools(&a));
    }

    #[test]
    fn single_server_omission_pluralizes() {
        assert_eq!(
            omitted(1),
            vec!["- … 1 more server; find their tools with searchTools()".to_string()]
        );
        assert_eq!(omitted(2).len(), 1);
        assert!(omitted(2)[0].contains("2 more servers"));
    }
}
