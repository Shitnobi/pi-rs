//! MCP server configuration and the servers extensions register with
//! `pi.registerMcpServer()`.
//!
//! Port of `src/core/mcp-servers.ts` (packages/coding-agent). The core only
//! validates and stores registrations; connecting them is the MCP extension's
//! job via the `mcp_servers_change` event.
//!
//! TS line → Rust item map:
//! - L8–19  `toolPatternRegExp` / `createToolNameMatcher` → [`tool_pattern_regexp`],
//!   [`ToolPattern`], [`create_tool_name_matcher`]
//! - L28–30 resource tool name constants → [`LIST_MCP_RESOURCES_TOOL`] etc.
//! - L39–43 `isMcpToolName` → [`is_mcp_tool_name`]
//! - L52–57 `McpExposure` / `MCP_EXPOSURES` / aliases → [`McpExposure`]
//! - L80–150 config interfaces → [`McpServerConfig`] (field-preserving)
//! - L131–134 `isLoopbackRedirectUri` → [`is_loopback_redirect_uri`]
//! - L155–158 `mcpNamespace` → [`mcp_namespace`]
//! - L221–231 `resolveExposureAlias(es)` → `resolve_exposure_alias(es)`
//! - L234–245 `getMcpToolExposure` → [`get_mcp_tool_exposure`]
//! - L248–306 `validateMcpServerConfig` → [`validate_mcp_server_config`]
//! - L308–345 `RegisteredMcpServer` / `McpServerRegistry` → [`RegisteredMcpServer`],
//!   [`McpServerRegistry`]
//!
//! Deviations from the plan (TS wins): the config is modeled as the validated
//! JSON object (insertion-ordered `serde_json::Map`) plus the branch marker
//! `kind`, because the TS validator returns the raw record with exposure
//! aliases resolved — unknown fields and key order survive validation and are
//! observable in `McpServerRegistry::list`. Typed structs could not preserve
//! them.

use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashSet;

/// `LIST_MCP_RESOURCES_TOOL` (mcp-servers.ts L28).
pub const LIST_MCP_RESOURCES_TOOL: &str = "list_mcp_resources";
/// `LIST_MCP_RESOURCE_TEMPLATES_TOOL` (mcp-servers.ts L29).
pub const LIST_MCP_RESOURCE_TEMPLATES_TOOL: &str = "list_mcp_resource_templates";
/// `READ_MCP_RESOURCE_TOOL` (mcp-servers.ts L30).
pub const READ_MCP_RESOURCE_TOOL: &str = "read_mcp_resource";

const MCP_RESOURCE_TOOLS: [&str; 3] = [
    LIST_MCP_RESOURCES_TOOL,
    LIST_MCP_RESOURCE_TEMPLATES_TOOL,
    READ_MCP_RESOURCE_TOOL,
];

/// Whether a tool comes from MCP: a server tool (`mcp__<server>__<tool>`) or a
/// resource tool. Port of `isMcpToolName` (L39).
pub fn is_mcp_tool_name(name: &str) -> bool {
    name.starts_with("mcp__") || MCP_RESOURCE_TOOLS.contains(&name)
}

/// How a server's tools reach the model and codemode scripts (L52).
///
/// - `codemode`: callable from codemode scripts but neither declared to the
///   model nor listed in the codemode description. `codemode-deferred` is an
///   accepted alias.
/// - `deferred`: not declared to the model until `tool_search` loads them.
/// - `direct`: declared to the model like any other tool.
/// - `hidden`: registered but unreachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpExposure {
    Codemode,
    Deferred,
    Direct,
    Hidden,
}

const MCP_EXPOSURES: [McpExposure; 4] = [
    McpExposure::Codemode,
    McpExposure::Deferred,
    McpExposure::Direct,
    McpExposure::Hidden,
];

/// Older exposure names, accepted in configs and replaced by their current
/// name when validated (`MCP_EXPOSURE_ALIASES`, L59).
const MCP_EXPOSURE_ALIASES: [(&str, McpExposure); 1] = [("codemode-deferred", McpExposure::Codemode)];

impl McpExposure {
    pub fn as_str(&self) -> &'static str {
        match self {
            McpExposure::Codemode => "codemode",
            McpExposure::Deferred => "deferred",
            McpExposure::Direct => "direct",
            McpExposure::Hidden => "hidden",
        }
    }

    /// Parse an exposure string; named `parse` to avoid clippy's `FromStr`
    /// confusion (this returns `Option`, not `Result`).
    pub fn parse(s: &str) -> Option<McpExposure> {
        MCP_EXPOSURES.iter().copied().find(|e| e.as_str() == s)
    }
}

impl Serialize for McpExposure {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// `MCP_EXPOSURES.map(...).join(", ")` — the quoted list used in error
/// messages.
fn exposures_list() -> String {
    MCP_EXPOSURES
        .iter()
        .map(|e| format!("\"{}\"", e.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

// ============================================================================
// URL parsing (WHATWG subset)
// ============================================================================

const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// The parts of a WHATWG URL this module needs. `protocol` includes the
/// trailing colon (`"http:"`), `hostname` is lowercased (bracketed for IPv6
/// literals), `port` is the raw digits (empty when absent), `search`/`hash`
/// include their leading `?`/`#` and are empty when absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    pub protocol: String,
    pub hostname: String,
    pub port: String,
    pub pathname: String,
    pub search: String,
    pub hash: String,
}

fn is_special_scheme(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "ws" | "wss" | "ftp" | "file")
}

/// Hosts that must be non-empty (everything special except `file:`).
fn requires_host(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "ws" | "wss" | "ftp")
}

/// Percent-decode; invalid `%` sequences make the host invalid (WHATWG).
fn percent_decode(s: &str) -> Option<String> {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = bytes.get(i + 1).and_then(|&b| hex(b))?;
            let lo = bytes.get(i + 2).and_then(|&b| hex(b))?;
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Split `rest` into (authority, remainder-including-delimiter). The authority
/// ends at the first `/`, `?` or `#` (also `\` for special schemes).
fn split_authority(rest: &str, special: bool) -> (&str, &str) {
    let end = rest
        .find(|c: char| c == '/' || c == '?' || c == '#' || (special && c == '\\'))
        .unwrap_or(rest.len());
    (&rest[..end], &rest[end..])
}

/// Split a post-authority remainder into (path, search, hash). Special-scheme
/// paths normalize `\` to `/` and empty paths to `/`.
fn split_path_query_fragment(remainder: &str, special: bool) -> (String, String, String) {
    let (before_frag, frag) = match remainder.find('#') {
        Some(i) => (&remainder[..i], &remainder[i + 1..]),
        None => (remainder, ""),
    };
    let (before_query, query) = match before_frag.find('?') {
        Some(i) => (&before_frag[..i], &before_frag[i + 1..]),
        None => (before_frag, ""),
    };
    let mut path = before_query.to_string();
    if special {
        path = path.replace('\\', "/");
        if path.is_empty() {
            path.push('/');
        }
    }
    // WHATWG: a trailing `?` with nothing before `#`/end yields no query, same
    // for a trailing `#`.
    let search = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query)
    };
    let hash = if frag.is_empty() {
        String::new()
    } else {
        format!("#{}", frag)
    };
    (path, search, hash)
}

/// Parse `host[:port]` (userinfo already excluded by the caller's split on the
/// last `@`… actually on the *last* `@` before the authority end, see
/// `parse_url`). Returns (hostname, port).
fn parse_host_port(hostport: &str, host_required: bool) -> Option<(String, String)> {
    // Strip userinfo: everything before the last `@`.
    let hostport = match hostport.rfind('@') {
        Some(i) => &hostport[i + 1..],
        None => hostport,
    };
    let (host_raw, port_raw): (&str, Option<&str>) = if let Some(after_bracket) = hostport.strip_prefix('[') {
        let close = after_bracket.find(']')?;
        let host = &hostport[..close + 2]; // includes brackets
        let after = &hostport[close + 2..];
        let port = if after.is_empty() { None } else { Some(after.strip_prefix(':')?) };
        (host, port)
    } else {
        match hostport.find(':') {
            Some(i) => (&hostport[..i], Some(&hostport[i + 1..])),
            None => (hostport, None),
        }
    };
    let port = match port_raw {
        None => String::new(),
        Some(p) => {
            if !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            if !p.is_empty() && p.parse::<u32>().ok()? > 65535 {
                return None;
            }
            p.to_string()
        }
    };
    let hostname = if host_raw.starts_with('[') {
        // IPv6 literal: brackets kept, lowercased; contents must look like an
        // IPv6 address (hex digits, `:`, `.`) — a lenient subset of WHATWG.
        let inner = &host_raw[1..host_raw.len() - 1];
        if inner.is_empty()
            || !inner.contains(':')
            || !inner.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        {
            return None;
        }
        host_raw.to_ascii_lowercase()
    } else {
        let decoded = percent_decode(host_raw)?;
        if decoded
            .chars()
            .any(|c| matches!(c, '\0' | '\t' | '\n' | '\r' | ' ' | '#' | '/' | ':' | '<' | '>' | '?' | '@' | '[' | '\\' | ']' | '^' | '|'))
        {
            return None;
        }
        if decoded.is_empty() && host_required {
            return None;
        }
        decoded.to_ascii_lowercase()
    };
    Some((hostname, port))
}

/// `URL.canParse` / `new URL(value)` for the inputs this module sees: server
/// `url`s (http/https), redirect URIs (http on loopback), resource URIs
/// (opaque or `scheme://host/path`) and metadata URLs. A pragmatic WHATWG
/// subset — see the module notes in `docs/plans/impl-notes-mcp-pure.md` for
/// the known deviations (IDNA/punycode, path dot-segment normalization,
/// opaque-path edge cases).
pub fn parse_url(input: &str) -> Option<ParsedUrl> {
    // Strip leading/trailing C0-control-or-space, then remove all tab, LF and
    // CR characters anywhere (WHATWG input preprocessing).
    let trimmed = input.trim_matches(|c: char| (c as u32) <= 0x20);
    let cleaned: String = trimmed
        .chars()
        .filter(|&c| c != '\t' && c != '\n' && c != '\r')
        .collect();
    if cleaned.is_empty() {
        return None;
    }

    // Scheme: ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"
    let colon = cleaned.find(':')?;
    let scheme_raw = &cleaned[..colon];
    let mut scheme_chars = scheme_raw.chars();
    let first = scheme_chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !scheme_chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    let scheme = scheme_raw.to_ascii_lowercase();
    let protocol = format!("{}:", scheme);
    let rest = &cleaned[colon + 1..];
    let special = is_special_scheme(&scheme);

    let (hostport, remainder, path_special) = if scheme == "file" {
        match rest.strip_prefix("//") {
            Some(r) => {
                let (authority, remainder) = split_authority(r, true);
                (authority, remainder, true)
            }
            None => {
                // `file:x` → path `/x` with an empty host.
                let (path, search, hash) = split_path_query_fragment(rest, false);
                let path = if path.starts_with('/') { path } else { format!("/{}", path) };
                return Some(ParsedUrl {
                    protocol,
                    hostname: String::new(),
                    port: String::new(),
                    pathname: path,
                    search,
                    hash,
                });
            }
        }
    } else if special {
        // Special schemes skip any number of leading slashes/backslashes
        // before the authority (`http:example.com`, `http:///path` both work).
        let r = rest.trim_start_matches(['/', '\\']);
        let (authority, remainder) = split_authority(r, true);
        (authority, remainder, true)
    } else if let Some(r) = rest.strip_prefix("//") {
        let (authority, remainder) = split_authority(r, false);
        (authority, remainder, false)
    } else {
        // Opaque path (`mailto:a@b`, `c:\x`).
        let (path, search, hash) = split_path_query_fragment(rest, false);
        return Some(ParsedUrl {
            protocol,
            hostname: String::new(),
            port: String::new(),
            pathname: path,
            search,
            hash,
        });
    };

    let (hostname, port) = parse_host_port(hostport, requires_host(&scheme))?;
    let (pathname, search, hash) = split_path_query_fragment(remainder, path_special);
    Some(ParsedUrl { protocol, hostname, port, pathname, search, hash })
}

/// Whether a redirect URI can be served by pi's loopback callback server
/// (L131): an `http` URI on `localhost`, `127.0.0.1` or `[::1]` without query
/// or fragment.
pub fn is_loopback_redirect_uri(value: &str) -> bool {
    match parse_url(value) {
        Some(url) => {
            url.protocol == "http:"
                && LOOPBACK_HOSTS.contains(&url.hostname.as_str())
                && url.search.is_empty()
                && url.hash.is_empty()
        }
        None => false,
    }
}

// ============================================================================
// Tool-name patterns
// ============================================================================

/// The JS regex source for a tool-name pattern where `*` matches any
/// characters: each `*`-separated part is regex-escaped
/// (`/[.+?^${}()|[\]\\]/g`), parts are joined with `.*`, and the whole thing
/// is anchored. Port of `toolPatternRegExp` (L8). Matching is done by
/// [`ToolPattern`] without a regex engine.
pub fn tool_pattern_regexp(pattern: &str) -> String {
    let escaped: Vec<String> = pattern
        .split('*')
        .map(|part| {
            let mut s = String::new();
            for c in part.chars() {
                if matches!(c, '.' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\') {
                    s.push('\\');
                }
                s.push(c);
            }
            s
        })
        .collect();
    format!("^{}$", escaped.join(".*"))
}

/// A compiled tool-name pattern (the `^L0.*L1…$` shape produced by
/// [`tool_pattern_regexp`]), matched without a regex engine: the name must
/// start with the first literal, contain the middle literals in order, and
/// end with the last. The gaps stand in for JS `.*`, which matches any code
/// unit *except* line terminators (`\n`, `\r`, `\u{2028}`, `\u{2029}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPattern {
    /// The JS regex source (with anchors), kept for parity/debugging.
    pub source: String,
    parts: Vec<String>,
}

fn gap_ok(s: &str) -> bool {
    !s.chars().any(|c| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
}

impl ToolPattern {
    pub fn new(pattern: &str) -> ToolPattern {
        ToolPattern {
            source: tool_pattern_regexp(pattern),
            parts: pattern.split('*').map(str::to_string).collect(),
        }
    }

    pub fn matches(&self, name: &str) -> bool {
        let parts = &self.parts;
        if parts.len() == 1 {
            // No `*`: the regex is `^literal$`.
            return name == parts[0];
        }
        if !name.starts_with(parts[0].as_str()) {
            return false;
        }
        let mut rest = &name[parts[0].len()..];
        for part in &parts[1..parts.len() - 1] {
            match rest.find(part.as_str()) {
                Some(i) => {
                    // Any later occurrence's gap is a superset, so a
                    // terminator here rules out every occurrence.
                    if !gap_ok(&rest[..i]) {
                        return false;
                    }
                    rest = &rest[i + part.len()..];
                }
                None => return false,
            }
        }
        let last = parts[parts.len() - 1].as_str();
        if !rest.ends_with(last) {
            return false;
        }
        gap_ok(&rest[..rest.len() - last.len()])
    }
}

/// Whether a tool name matches any of the entries, each an exact name or a
/// `*` pattern. Port of `createToolNameMatcher` (L21) — the TS returns a
/// closure; Rust gets a matcher value with a `matches` method.
#[derive(Debug, Clone, Default)]
pub struct ToolNameMatcher {
    names: HashSet<String>,
    patterns: Vec<ToolPattern>,
}

impl ToolNameMatcher {
    pub fn matches(&self, name: &str) -> bool {
        self.names.contains(name) || self.patterns.iter().any(|p| p.matches(name))
    }
}

pub fn create_tool_name_matcher(entries: &[String]) -> ToolNameMatcher {
    let mut names = HashSet::new();
    let mut patterns = Vec::new();
    for entry in entries {
        if entry.contains('*') {
            patterns.push(ToolPattern::new(entry));
        } else {
            names.insert(entry.clone());
        }
    }
    ToolNameMatcher { names, patterns }
}

// ============================================================================
// Server configuration
// ============================================================================

/// Which validation branch accepted the config (`url` → Http, `command` →
/// Stdio). The TS returns the same record either way; the branch is only
/// observable through which checks ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerKind {
    Stdio,
    Http,
}

/// A validated `mcpServers` entry: the raw JSON object with exposure aliases
/// resolved, plus the branch it validated as. Key order and unknown fields
/// are preserved, like the TS spread-copy.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerConfig {
    pub kind: McpServerKind,
    pub fields: Map<String, Value>,
}

impl Serialize for McpServerConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.fields.serialize(serializer)
    }
}

fn value_as_exposure(value: &Value) -> Option<McpExposure> {
    value.as_str().and_then(McpExposure::parse)
}

impl McpServerConfig {
    /// `config.exposure` (validated configs always parse).
    pub fn exposure(&self) -> Option<McpExposure> {
        self.fields.get("exposure").and_then(value_as_exposure)
    }

    /// `config.toolExposure`.
    pub fn tool_exposure(&self) -> Option<&Map<String, Value>> {
        self.fields.get("toolExposure").and_then(Value::as_object)
    }

    /// `config.url` for Http configs.
    pub fn url(&self) -> Option<&str> {
        self.fields.get("url").and_then(Value::as_str)
    }

    /// `config.command` for Stdio configs.
    pub fn command(&self) -> Option<&str> {
        self.fields.get("command").and_then(Value::as_str)
    }
}

/// Namespace of a server's tools: `mcp__<server>` with `-` replaced by `_`,
/// like the tool names. Port of `mcpNamespace` (L155).
pub fn mcp_namespace(server: &str) -> String {
    format!("mcp__{}", server.replace('-', "_"))
}

/// Exposure of one tool of a server: its `toolExposure` entry, else the
/// server's `exposure`. Port of `getMcpToolExposure` (L234). Exact entries win
/// over patterns; among patterns the first match in object order wins; the
/// default is `codemode`.
pub fn get_mcp_tool_exposure(config: &McpServerConfig, tool_name: &str) -> McpExposure {
    if let Some(overrides) = config.tool_exposure() {
        if let Some(exact) = overrides.get(tool_name) {
            if let Some(exposure) = value_as_exposure(exact) {
                return exposure;
            }
        }
        for (pattern, exposure) in overrides {
            if pattern.contains('*') && ToolPattern::new(pattern).matches(tool_name) {
                if let Some(exposure) = value_as_exposure(exposure) {
                    return exposure;
                }
            }
        }
    }
    config.exposure().unwrap_or(McpExposure::Codemode)
}

// ============================================================================
// Validation
// ============================================================================

fn is_string_record(value: &Value) -> bool {
    match value.as_object() {
        Some(map) => map.values().all(Value::is_string),
        None => false,
    }
}

fn is_exposure(value: &Value) -> bool {
    value_as_exposure(value).is_some()
}

/// JS `String.prototype.trim` (used by the `clientName` check).
fn js_trim(s: &str) -> bool {
    !s.chars().all(crate::js_string::is_js_whitespace_or_terminator)
}

fn is_valid_server_name(name: &str) -> bool {
    // /^[A-Za-z0-9_-]+$/
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The exposure an alias stands for; other values are returned unchanged
/// (`resolveExposureAlias`, L224).
fn resolve_exposure_alias(value: &Value) -> Value {
    if let Value::String(s) = value {
        if let Some(&(_, replacement)) = MCP_EXPOSURE_ALIASES.iter().find(|(alias, _)| alias == s) {
            return Value::String(replacement.as_str().to_string());
        }
    }
    value.clone()
}

/// A copy of the server entry with exposure aliases replaced by their current
/// names (`resolveExposureAliases`, L221). Key order is preserved.
fn resolve_exposure_aliases(value: &Map<String, Value>) -> Map<String, Value> {
    let mut resolved = value.clone();
    if resolved.contains_key("exposure") {
        let replacement = resolve_exposure_alias(resolved.get("exposure").unwrap());
        resolved.insert("exposure".to_string(), replacement);
    }
    if let Some(Value::Object(tool_exposure)) = resolved.get("toolExposure") {
        let mapped: Map<String, Value> = tool_exposure
            .iter()
            .map(|(tool, entry)| (tool.clone(), resolve_exposure_alias(entry)))
            .collect();
        resolved.insert("toolExposure".to_string(), Value::Object(mapped));
    }
    resolved
}

/// JS `Number(string)` for the digit strings we feed it (url ports).
fn js_string_to_number(s: &str) -> f64 {
    s.parse::<f64>().unwrap_or(f64::NAN)
}

/// `validateOAuth` (L167–209) — returns the error message, or `None` when the
/// oauth section is valid (or absent).
fn validate_oauth(value: Option<&Value>) -> Option<String> {
    let value = value?;
    let obj = match value.as_object() {
        Some(obj) => obj,
        None => return Some("oauth must be an object".to_string()),
    };
    if let Some(client_id) = obj.get("clientId") {
        if !client_id.is_string() {
            return Some("oauth.clientId must be a string".to_string());
        }
    }
    if let Some(client_secret) = obj.get("clientSecret") {
        if !client_secret.is_string() {
            return Some("oauth.clientSecret must be a string".to_string());
        }
    }
    let port = obj.get("callbackPort");
    if let Some(p) = port {
        // typeof number && Number.isInteger && 1..=65535
        let ok = p
            .as_f64()
            .map(|n| n.fract() == 0.0 && (1.0..=65535.0).contains(&n))
            .unwrap_or(false);
        if !ok {
            return Some("oauth.callbackPort must be a port number".to_string());
        }
    }
    if let Some(callback_url) = obj.get("callbackUrl") {
        let url = callback_url.as_str().and_then(|s| {
            if is_loopback_redirect_uri(s) {
                parse_url(s)
            } else {
                None
            }
        });
        let Some(url) = url else {
            return Some("oauth.callbackUrl must be an http URI on localhost, 127.0.0.1, or [::1] without query or fragment".to_string());
        };
        if !url.port.is_empty() {
            if let Some(port_value) = port.and_then(Value::as_f64) {
                if js_string_to_number(&url.port) != port_value {
                    return Some("oauth.callbackUrl and oauth.callbackPort name different ports".to_string());
                }
            }
        }
    }
    if let Some(scope) = obj.get("scope") {
        if !scope.is_string() {
            return Some("oauth.scope must be a string".to_string());
        }
    }
    if let Some(client_name) = obj.get("clientName") {
        // typeof string && trim() truthy
        let ok = client_name.as_str().map(js_trim).unwrap_or(false);
        if !ok {
            return Some("oauth.clientName must be a non-empty string".to_string());
        }
    }
    if let Some(registration) = obj.get("clientRegistration") {
        if registration.as_str() != Some("dcr") {
            if registration.as_str() != Some("cimd") {
                return Some("oauth.clientRegistration must be \"dcr\" or \"cimd\"".to_string());
            }
            // `!== undefined` — a JSON null counts as present.
            if obj.contains_key("clientId") || obj.contains_key("clientName") {
                return Some(
                    "oauth.clientRegistration \"cimd\" cannot be combined with oauth.clientId or oauth.clientName"
                        .to_string(),
                );
            }
            let callback = obj.get("callbackUrl").and_then(Value::as_str).and_then(parse_url);
            if let Some(callback) = callback {
                if callback.hostname == "[::1]" || callback.pathname != "/callback" {
                    return Some("oauth.clientRegistration \"cimd\" requires oauth.callbackUrl on localhost or 127.0.0.1 with path /callback".to_string());
                }
            }
        }
    }
    if let Some(metadata_url) = obj.get("authServerMetadataUrl") {
        let ok = metadata_url
            .as_str()
            .and_then(parse_url)
            .map(|url| {
                url.protocol == "https:"
                    || (url.protocol == "http:" && LOOPBACK_HOSTS.contains(&url.hostname.as_str()))
            })
            .unwrap_or(false);
        if !ok {
            return Some(
                "oauth.authServerMetadataUrl must be an https URL, or http on localhost, 127.0.0.1, or [::1]"
                    .to_string(),
            );
        }
    }
    None
}

/// Validate one server entry of the `mcpServers` shape. Returns a copy of the
/// config with exposure aliases resolved, or an error message — the TS
/// returns `McpServerConfig | string`, mirrored here as
/// `Result<McpServerConfig, String>`. Port of `validateMcpServerConfig`
/// (L248).
pub fn validate_mcp_server_config(name: &str, raw: &Value) -> Result<McpServerConfig, String> {
    if !is_valid_server_name(name) {
        return Err(format!("invalid server name \"{}\" (use letters, digits, \"_\" and \"-\")", name));
    }
    let Some(raw_map) = raw.as_object() else {
        return Err(format!("server \"{}\" must be an object", name));
    };
    let value = resolve_exposure_aliases(raw_map);
    let type_of = value.get("type");
    let exposures = exposures_list();
    if let Some(exposure) = value.get("exposure") {
        if !is_exposure(exposure) {
            return Err(format!("server \"{}\": exposure must be one of {}", name, exposures));
        }
    }
    if let Some(tool_exposure) = value.get("toolExposure") {
        let Some(map) = tool_exposure.as_object() else {
            return Err(format!("server \"{}\": toolExposure must map tool names to exposures", name));
        };
        for (tool, entry) in map {
            if !is_exposure(entry) {
                return Err(format!("server \"{}\": toolExposure \"{}\" must be one of {}", name, tool, exposures));
            }
        }
    }
    if let Some(enabled) = value.get("enabled") {
        if !enabled.is_boolean() {
            return Err(format!("server \"{}\": enabled must be a boolean", name));
        }
    }
    if let Some(description) = value.get("description") {
        if !description.is_string() {
            return Err(format!("server \"{}\": description must be a string", name));
        }
    }
    if let Some(timeout) = value.get("timeout") {
        // typeof number && timeout > 0 (NaN, 0 and negatives rejected)
        let ok = timeout.as_f64().map(|t| t > 0.0).unwrap_or(false);
        if !ok {
            return Err(format!("server \"{}\": timeout must be a positive number of seconds", name));
        }
    }
    if matches!(type_of, Some(Value::String(s)) if s == "sse") {
        return Err(format!(
            "server \"{}\": legacy SSE transport is not supported; use the streamable HTTP URL",
            name
        ));
    }

    let url_is_string = matches!(value.get("url"), Some(Value::String(_)));
    let type_ok_for_http = type_of.is_none()
        || matches!(type_of, Some(Value::String(s)) if s == "http" || s == "streamable-http");
    if url_is_string && type_ok_for_http {
        let url_str = value.get("url").and_then(Value::as_str).unwrap_or_default();
        let parsed = parse_url(url_str).filter(|url| url.protocol == "http:" || url.protocol == "https:");
        if parsed.is_none() {
            return Err(format!("server \"{}\": url must be an http or https URL", name));
        }
        if let Some(headers) = value.get("headers") {
            if !is_string_record(headers) {
                return Err(format!("server \"{}\": headers must map names to strings", name));
            }
        }
        if let Some(oauth_error) = validate_oauth(value.get("oauth")) {
            return Err(format!("server \"{}\": {}", name, oauth_error));
        }
        if let Some(auth) = value.get("auth") {
            let provider_ok = auth
                .as_object()
                .and_then(|a| a.get("provider"))
                .map(|p| p.is_string() && p.as_str() != Some(""))
                .unwrap_or(false);
            if !provider_ok {
                return Err(format!("server \"{}\": auth.provider must be a provider name", name));
            }
            let url = parse_url(url_str).unwrap();
            if url.protocol != "https:" && !LOOPBACK_HOSTS.contains(&url.hostname.as_str()) {
                return Err(format!(
                    "server \"{}\": auth requires an https URL, or http on localhost, 127.0.0.1, or [::1]",
                    name
                ));
            }
        }
        return Ok(McpServerConfig { kind: McpServerKind::Http, fields: value });
    }

    let command_is_string = matches!(value.get("command"), Some(Value::String(_)));
    let type_ok_for_stdio = type_of.is_none() || matches!(type_of, Some(Value::String(s)) if s == "stdio");
    if command_is_string && type_ok_for_stdio {
        if let Some(args) = value.get("args") {
            let ok = args.as_array().map(|a| a.iter().all(Value::is_string)).unwrap_or(false);
            if !ok {
                return Err(format!("server \"{}\": args must be an array of strings", name));
            }
        }
        if let Some(env) = value.get("env") {
            if !is_string_record(env) {
                return Err(format!("server \"{}\": env must map names to strings", name));
            }
        }
        if let Some(cwd) = value.get("cwd") {
            if !cwd.is_string() {
                return Err(format!("server \"{}\": cwd must be a string", name));
            }
        }
        return Ok(McpServerConfig { kind: McpServerKind::Stdio, fields: value });
    }
    Err(format!(
        "server \"{}\" needs either \"command\" (stdio) or \"url\" (streamable HTTP)",
        name
    ))
}

// ============================================================================
// Registry
// ============================================================================

/// A server an extension registered with `pi.registerMcpServer()` (L308).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredMcpServer {
    pub name: String,
    pub config: McpServerConfig,
    /// Path of the extension that registered the server.
    pub extension_path: String,
}

/// Servers registered by the extensions of one runtime (L316). A JS `Map`
/// keyed by name: re-registering keeps the original position; `list()` hands
/// out deep clones in registration order.
#[derive(Default)]
pub struct McpServerRegistry {
    servers: Vec<RegisteredMcpServer>,
    // Invoked after every mutation with the post-mutation server list; the
    // list is passed (not read back) so listeners never re-enter the
    // registry's lock (the JS original reads `list()` on the unlocked
    // single-threaded registry).
    change_listener: Option<Box<dyn Fn(&[RegisteredMcpServer]) + Send + Sync>>,
}

impl McpServerRegistry {
    pub fn new() -> McpServerRegistry {
        McpServerRegistry::default()
    }

    /// Register or replace a server. The caller checks ownership.
    pub fn register(&mut self, server: RegisteredMcpServer) {
        if let Some(existing) = self.servers.iter_mut().find(|s| s.name == server.name) {
            // Map.set semantics: the key keeps its original position.
            *existing = server;
        } else {
            self.servers.push(server);
        }
        if let Some(listener) = &self.change_listener {
            listener(&self.servers);
        }
    }

    /// Remove a server registered by `extension_path`. Servers of other
    /// extensions are left alone — and cause no change event.
    pub fn unregister(&mut self, name: &str, extension_path: &str) {
        let owned = self
            .servers
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.extension_path == extension_path)
            .unwrap_or(false);
        if !owned {
            return;
        }
        self.servers.retain(|s| s.name != name);
        if let Some(listener) = &self.change_listener {
            listener(&self.servers);
        }
    }

    pub fn get(&self, name: &str) -> Option<&RegisteredMcpServer> {
        self.servers.iter().find(|s| s.name == name)
    }

    /// Deep clones of the registered servers, in registration order.
    pub fn list(&self) -> Vec<RegisteredMcpServer> {
        self.servers.clone()
    }

    /// Called after every change. The runner sets it when it binds, to emit
    /// `mcp_servers_change`.
    pub fn set_change_listener(&mut self, listener: Option<Box<dyn Fn(&[RegisteredMcpServer]) + Send + Sync>>) {
        self.change_listener = listener;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn matcher(entries: &[&str]) -> ToolNameMatcher {
        create_tool_name_matcher(&entries.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn tool_pattern_regexp_escapes_like_js() {
        assert_eq!(tool_pattern_regexp("read_*"), "^read_.*$");
        assert_eq!(tool_pattern_regexp("a.b+c?d"), "^a\\.b\\+c\\?d$");
        assert_eq!(tool_pattern_regexp("^$({[|]})\\"), "^\\^\\$\\(\\{\\[\\|\\]\\}\\)\\\\$");
        assert_eq!(tool_pattern_regexp("a*b**c"), "^a.*b.*.*c$");
        assert_eq!(tool_pattern_regexp(""), "^$");
        assert_eq!(tool_pattern_regexp("*"), "^.*$");
    }

    #[test]
    fn pattern_matching_semantics() {
        let p = |pattern: &str| ToolPattern::new(pattern);
        assert!(p("list_*").matches("list_dirs"));
        assert!(!p("list_*").matches("mylist_dirs"));
        assert!(p("*").matches("anything"));
        assert!(p("").matches(""));
        assert!(!p("").matches("x"));
        assert!(p("a*b").matches("axxb"));
        assert!(p("a*b**c").matches("a-b-c"));
        // `.` in patterns is a literal, not a wildcard.
        assert!(p("a.b").matches("a.b"));
        assert!(!p("a.b").matches("axb"));
        // JS `.` does not match line terminators, so the wildcard gaps may not
        // contain them.
        assert!(!p("a*b").matches("a\nb"));
        assert!(!p("a*b").matches("a\u{2028}b"));
        assert!(p("a*b").matches("a\tb"));
        // `.*` from a leading `*` matches any prefix, so `*a` matches "ba".
        assert!(p("*a").matches("ba"));
        assert!(p("*a").matches("xa"));
        assert!(p("a*a").matches("aa"));
        assert!(!p("a*a").matches("a"));
    }

    #[test]
    fn matcher_exact_and_patterns() {
        let m = matcher(&["read_file", "list_*", "mcp__x__*"]);
        assert!(m.matches("read_file"));
        assert!(!m.matches("read_files"));
        assert!(m.matches("list_mcp_resources"));
        assert!(m.matches("mcp__x__anything"));
        assert!(!m.matches("grep"));
        // Empty entries match the empty name only.
        let empty = matcher(&[""]);
        assert!(empty.matches(""));
        assert!(!empty.matches("x"));
    }

    #[test]
    fn is_mcp_tool_name_cases() {
        assert!(is_mcp_tool_name("mcp__srv__tool"));
        assert!(is_mcp_tool_name("mcp__"));
        assert!(is_mcp_tool_name(LIST_MCP_RESOURCES_TOOL));
        assert!(is_mcp_tool_name(LIST_MCP_RESOURCE_TEMPLATES_TOOL));
        assert!(is_mcp_tool_name(READ_MCP_RESOURCE_TOOL));
        assert!(!is_mcp_tool_name("read_file"));
        assert!(!is_mcp_tool_name("mcp_"));
        assert!(!is_mcp_tool_name(""));
    }

    #[test]
    #[allow(clippy::type_complexity)] // table of WHATWG URL fields, one tuple per case
    fn parse_url_matches_whatwg_for_scope() {
        let cases: &[(&str, Option<(&str, &str, &str, &str, &str, &str)>)] = &[
            // input, (protocol, hostname, port, pathname, search, hash)
            ("http://x:0", Some(("http:", "x", "0", "/", "", ""))),
            ("http:example.com", Some(("http:", "example.com", "", "/", "", ""))),
            ("ui://a/b.txt", Some(("ui:", "a", "", "/b.txt", "", ""))),
            (
                "HTTP://LOCALHOST:8080/CALLBACK?x",
                Some(("http:", "localhost", "8080", "/CALLBACK", "?x", "")),
            ),
            ("http://[::1]:8080/p", Some(("http:", "[::1]", "8080", "/p", "", ""))),
            ("http://localhost/a", Some(("http:", "localhost", "", "/a", "", ""))),
            ("http://", None),
            ("http://user@host/p", Some(("http:", "host", "", "/p", "", ""))),
            ("http://user:pw@host/p", Some(("http:", "host", "", "/p", "", ""))),
            ("http://host:99999/p", None),
            ("http://host:65536/x", None),
            ("http://host:65535/x", Some(("http:", "host", "65535", "/x", "", ""))),
            ("not a url", None),
            ("", None),
            ("http://[::1]/x", Some(("http:", "[::1]", "", "/x", "", ""))),
            ("HTTP://[::1]:9/x", Some(("http:", "[::1]", "9", "/x", "", ""))),
            ("http://%6cocalhost/x", Some(("http:", "localhost", "", "/x", "", ""))),
            ("http://a%b/x", None),
            ("http://a b/", None),
            ("http:///path", Some(("http:", "path", "", "/", "", ""))),
            ("http:/x", Some(("http:", "x", "", "/", "", ""))),
            ("http://?q", None),
            ("http://host:/", Some(("http:", "host", "", "/", "", ""))),
            ("http://host:80x/", None),
            ("http://[::1/x", None),
            ("file:///x.txt", Some(("file:", "", "", "/x.txt", "", ""))),
            ("file://host/p", Some(("file:", "host", "", "/p", "", ""))),
            ("file:/x", Some(("file:", "", "", "/x", "", ""))),
            ("ui://", Some(("ui:", "", "", "", "", ""))),
            ("ui:opaque.txt", Some(("ui:", "", "", "opaque.txt", "", ""))),
            ("mailto:a@b", Some(("mailto:", "", "", "a@b", "", ""))),
            ("C:\\path\\file.txt", Some(("c:", "", "", "\\path\\file.txt", "", ""))),
            ("http://x/p#", Some(("http:", "x", "", "/p", "", ""))),
            ("http://x/p#?", Some(("http:", "x", "", "/p", "", "#?"))),
            ("http://x/p?", Some(("http:", "x", "", "/p", "", ""))),
            ("http://x/p?a#", Some(("http:", "x", "", "/p", "?a", ""))),
            ("http://x/p?#f", Some(("http:", "x", "", "/p", "", "#f"))),
            ("http://x/p#f?q", Some(("http:", "x", "", "/p", "", "#f?q"))),
            ("http://localhost./x", Some(("http:", "localhost.", "", "/x", "", ""))),
            (
                "https://user:pw@localhost:99/cb#f",
                Some(("https:", "localhost", "99", "/cb", "", "#f")),
            ),
            ("http://a\\b/", Some(("http:", "a", "", "/b/", "", ""))),
        ];
        for (input, expected) in cases {
            let parsed = parse_url(input);
            match (parsed, expected) {
                (None, None) => {}
                (Some(u), Some((protocol, hostname, port, pathname, search, hash))) => {
                    assert_eq!((u.protocol.as_str(), u.hostname.as_str(), u.port.as_str(), u.pathname.as_str(), u.search.as_str(), u.hash.as_str()), (*protocol, *hostname, *port, *pathname, *search, *hash), "input {:?}", input);
                }
                (parsed, expected) => panic!("parse_url({:?}) = {:?}, expected {:?}", input, parsed, expected),
            }
        }
    }

    #[test]
    fn loopback_redirect_uri_cases() {
        assert!(is_loopback_redirect_uri("http://localhost:8080/callback"));
        assert!(is_loopback_redirect_uri("http://127.0.0.1:9/x"));
        assert!(is_loopback_redirect_uri("http://[::1]/x"));
        assert!(is_loopback_redirect_uri("HTTP://LOCALHOST/x"));
        // Trailing `?` or `#` with nothing after yields no query/fragment.
        assert!(is_loopback_redirect_uri("http://localhost:8080/callback?"));
        assert!(is_loopback_redirect_uri("http://localhost:8080/callback#"));
        assert!(!is_loopback_redirect_uri("http://localhost:8080/callback?x"));
        assert!(!is_loopback_redirect_uri("http://localhost:80/callback#frag"));
        assert!(!is_loopback_redirect_uri("https://localhost/x"));
        assert!(!is_loopback_redirect_uri("http://example.com/cb"));
        assert!(!is_loopback_redirect_uri("http://localhost./x"));
        assert!(!is_loopback_redirect_uri("not a url"));
        assert!(!is_loopback_redirect_uri(""));
    }

    #[test]
    fn namespace() {
        assert_eq!(mcp_namespace("my-server"), "mcp__my_server");
        assert_eq!(mcp_namespace("srv"), "mcp__srv");
        assert_eq!(mcp_namespace("a--b"), "mcp__a__b");
        // `-` and `_` namespaces collide, like the TS.
        assert_eq!(mcp_namespace("a-b"), mcp_namespace("a_b"));
    }

    fn http_config(raw: Value) -> McpServerConfig {
        let config = validate_mcp_server_config("srv", &raw).unwrap();
        assert_eq!(config.kind, McpServerKind::Http);
        config
    }

    #[test]
    fn tool_exposure_resolution() {
        let config = http_config(json!({
            "url": "http://x",
            "exposure": "hidden",
            "toolExposure": { "b*": "direct", "a": "deferred" }
        }));
        assert_eq!(get_mcp_tool_exposure(&config, "a"), McpExposure::Deferred);
        // `b*` anchors at the start: "abc" does not match, falls to exposure.
        assert_eq!(get_mcp_tool_exposure(&config, "abc"), McpExposure::Hidden);
        assert_eq!(get_mcp_tool_exposure(&config, "bcd"), McpExposure::Direct);
        assert_eq!(get_mcp_tool_exposure(&config, "zzz"), McpExposure::Hidden);
        // First matching pattern in object order wins.
        let config = http_config(json!({
            "url": "http://x",
            "toolExposure": { "a*": "direct", "*z": "hidden" }
        }));
        assert_eq!(get_mcp_tool_exposure(&config, "az"), McpExposure::Direct);
        // Default is codemode.
        let config = http_config(json!({ "url": "http://x" }));
        assert_eq!(get_mcp_tool_exposure(&config, "t"), McpExposure::Codemode);
        let config = http_config(json!({ "url": "http://x", "exposure": "direct" }));
        assert_eq!(get_mcp_tool_exposure(&config, "t"), McpExposure::Direct);
    }

    #[test]
    fn validate_ok_configs_preserve_fields_and_order() {
        let config = validate_mcp_server_config(
            "srv",
            &json!({ "command": "npx", "args": ["x"], "env": {"A": "1"}, "exposure": "codemode-deferred" }),
        )
        .unwrap();
        assert_eq!(config.kind, McpServerKind::Stdio);
        assert_eq!(serde_json::to_string(&config).unwrap(),
            r#"{"command":"npx","args":["x"],"env":{"A":"1"},"exposure":"codemode"}"#);

        // Aliases resolve inside toolExposure too, keeping key order.
        let config = validate_mcp_server_config(
            "srv",
            &json!({ "url": "http://x", "toolExposure": {"a": "codemode-deferred"} }),
        )
        .unwrap();
        assert_eq!(config.kind, McpServerKind::Http);
        assert_eq!(
            serde_json::to_string(&config).unwrap(),
            r#"{"url":"http://x","toolExposure":{"a":"codemode"}}"#
        );

        // streamable-http is an alias of http; unknown fields survive.
        let config = validate_mcp_server_config(
            "srv",
            &json!({ "type": "streamable-http", "url": "http://x", "extra": 1 }),
        )
        .unwrap();
        assert_eq!(config.kind, McpServerKind::Http);
        assert_eq!(serde_json::to_string(&config).unwrap(), r#"{"type":"streamable-http","url":"http://x","extra":1}"#);

        // A url wins over command when both are present.
        let config = validate_mcp_server_config("srv", &json!({ "url": "http://x", "command": "y" })).unwrap();
        assert_eq!(config.kind, McpServerKind::Http);

        // url: null falls through to the stdio branch.
        let config = validate_mcp_server_config("srv", &json!({ "url": null, "command": "x" })).unwrap();
        assert_eq!(config.kind, McpServerKind::Stdio);

        // auth over plain loopback http is fine.
        validate_mcp_server_config("srv", &json!({ "url": "http://127.0.0.1:9", "auth": {"provider": "p"} })).unwrap();
    }

    #[test]
    fn validate_error_strings() {
        let cases: &[(&str, Value, &str)] = &[
            ("sr v", json!({"command": "x"}), "invalid server name \"sr v\" (use letters, digits, \"_\" and \"-\")"),
            ("", json!({"command": "x"}), "invalid server name \"\" (use letters, digits, \"_\" and \"-\")"),
            ("srv", json!("hi"), "server \"srv\" must be an object"),
            ("srv", json!(null), "server \"srv\" must be an object"),
            ("srv", json!([1]), "server \"srv\" must be an object"),
            ("srv", json!({"url": "http://x", "exposure": "bad"}),
                "server \"srv\": exposure must be one of \"codemode\", \"deferred\", \"direct\", \"hidden\""),
            ("srv", json!({"url": "http://x", "exposure": null}),
                "server \"srv\": exposure must be one of \"codemode\", \"deferred\", \"direct\", \"hidden\""),
            ("srv", json!({"url": "http://x", "toolExposure": "nope"}),
                "server \"srv\": toolExposure must map tool names to exposures"),
            ("srv", json!({"url": "http://x", "toolExposure": {"t": "bad"}}),
                "server \"srv\": toolExposure \"t\" must be one of \"codemode\", \"deferred\", \"direct\", \"hidden\""),
            ("srv", json!({"url": "http://x", "enabled": null}), "server \"srv\": enabled must be a boolean"),
            ("srv", json!({"url": "http://x", "enabled": 1}), "server \"srv\": enabled must be a boolean"),
            ("srv", json!({"url": "http://x", "description": 5}), "server \"srv\": description must be a string"),
            ("srv", json!({"command": "x", "timeout": 0}),
                "server \"srv\": timeout must be a positive number of seconds"),
            ("srv", json!({"command": "x", "timeout": -1}),
                "server \"srv\": timeout must be a positive number of seconds"),
            ("srv", json!({"command": "x", "timeout": "60"}),
                "server \"srv\": timeout must be a positive number of seconds"),
            ("srv", json!({"command": "x", "timeout": null}),
                "server \"srv\": timeout must be a positive number of seconds"),
            ("srv", json!({"type": "sse", "url": "http://x"}),
                "server \"srv\": legacy SSE transport is not supported; use the streamable HTTP URL"),
            ("srv", json!({"url": "ftp://x"}), "server \"srv\": url must be an http or https URL"),
            ("srv", json!({"url": "http://x:99999"}), "server \"srv\": url must be an http or https URL"),
            ("srv", json!({"url": "not a url"}), "server \"srv\": url must be an http or https URL"),
            ("srv", json!({"url": "http://x", "headers": {"a": 1}}),
                "server \"srv\": headers must map names to strings"),
            ("srv", json!({"url": "http://x", "oauth": null}), "server \"srv\": oauth must be an object"),
            ("srv", json!({"url": "http://x", "oauth": {"callbackPort": 0}}),
                "server \"srv\": oauth.callbackPort must be a port number"),
            ("srv", json!({"url": "http://x", "oauth": {"callbackPort": 80.5}}),
                "server \"srv\": oauth.callbackPort must be a port number"),
            ("srv", json!({"url": "http://x", "oauth": {"callbackPort": 65536}}),
                "server \"srv\": oauth.callbackPort must be a port number"),
            ("srv", json!({"url": "http://x", "oauth": {"callbackPort": "80"}}),
                "server \"srv\": oauth.callbackPort must be a port number"),
            ("srv", json!({"url": "http://x", "oauth": {"callbackUrl": "http://example.com/cb"}}),
                "server \"srv\": oauth.callbackUrl must be an http URI on localhost, 127.0.0.1, or [::1] without query or fragment"),
            ("srv", json!({"url": "http://x", "oauth": {"callbackUrl": "https://localhost/cb"}}),
                "server \"srv\": oauth.callbackUrl must be an http URI on localhost, 127.0.0.1, or [::1] without query or fragment"),
            ("srv", json!({"url": "http://x", "oauth": {"callbackPort": 8081, "callbackUrl": "http://localhost:8080/callback"}}),
                "server \"srv\": oauth.callbackUrl and oauth.callbackPort name different ports"),
            ("srv", json!({"url": "http://x", "oauth": {"scope": 1}}), "server \"srv\": oauth.scope must be a string"),
            ("srv", json!({"url": "http://x", "oauth": {"clientName": " "}}),
                "server \"srv\": oauth.clientName must be a non-empty string"),
            ("srv", json!({"url": "http://x", "oauth": {"clientRegistration": "nope"}}),
                "server \"srv\": oauth.clientRegistration must be \"dcr\" or \"cimd\""),
            ("srv", json!({"url": "http://x", "oauth": {"clientRegistration": "cimd", "clientId": "a"}}),
                "server \"srv\": oauth.clientRegistration \"cimd\" cannot be combined with oauth.clientId or oauth.clientName"),
            ("srv", json!({"url": "http://x", "oauth": {"clientRegistration": "cimd", "callbackUrl": "http://localhost:8080/nope"}}),
                "server \"srv\": oauth.clientRegistration \"cimd\" requires oauth.callbackUrl on localhost or 127.0.0.1 with path /callback"),
            ("srv", json!({"url": "http://x", "oauth": {"clientRegistration": "cimd", "callbackUrl": "http://[::1]:9/callback"}}),
                "server \"srv\": oauth.clientRegistration \"cimd\" requires oauth.callbackUrl on localhost or 127.0.0.1 with path /callback"),
            ("srv", json!({"url": "http://x", "oauth": {"authServerMetadataUrl": "http://x/.well-known"}}),
                "server \"srv\": oauth.authServerMetadataUrl must be an https URL, or http on localhost, 127.0.0.1, or [::1]"),
            ("srv", json!({"url": "http://x", "oauth": {"authServerMetadataUrl": "garbage"}}),
                "server \"srv\": oauth.authServerMetadataUrl must be an https URL, or http on localhost, 127.0.0.1, or [::1]"),
            ("srv", json!({"url": "http://example.com", "auth": {"provider": "anthropic"}}),
                "server \"srv\": auth requires an https URL, or http on localhost, 127.0.0.1, or [::1]"),
            ("srv", json!({"url": "http://x", "auth": {"provider": ""}}),
                "server \"srv\": auth.provider must be a provider name"),
            ("srv", json!({"url": "http://x", "auth": {"provider": 5}}),
                "server \"srv\": auth.provider must be a provider name"),
            ("srv", json!({"url": "http://x", "auth": "nope"}),
                "server \"srv\": auth.provider must be a provider name"),
            ("srv", json!({"command": "x", "args": [1]}), "server \"srv\": args must be an array of strings"),
            ("srv", json!({"command": "x", "args": "no"}), "server \"srv\": args must be an array of strings"),
            ("srv", json!({"command": "x", "env": {"A": 1}}), "server \"srv\": env must map names to strings"),
            ("srv", json!({"command": "x", "cwd": 3}), "server \"srv\": cwd must be a string"),
            ("srv", json!({"foo": 1}),
                "server \"srv\" needs either \"command\" (stdio) or \"url\" (streamable HTTP)"),
            ("srv", json!({"type": "weird", "url": "http://x"}),
                "server \"srv\" needs either \"command\" (stdio) or \"url\" (streamable HTTP)"),
            ("srv", json!({"type": "stdio", "url": "http://x"}),
                "server \"srv\" needs either \"command\" (stdio) or \"url\" (streamable HTTP)"),
            ("srv", json!({"type": "http", "command": "x"}),
                "server \"srv\" needs either \"command\" (stdio) or \"url\" (streamable HTTP)"),
        ];
        for (name, raw, expected) in cases {
            assert_eq!(
                validate_mcp_server_config(name, raw).unwrap_err(),
                *expected,
                "case: {} {:?}",
                name,
                raw
            );
        }
    }

    #[test]
    fn validate_oauth_ok_paths() {
        // Loopback http metadata URLs are allowed.
        validate_mcp_server_config(
            "srv",
            &json!({"url": "http://x", "oauth": {"authServerMetadataUrl": "http://127.0.0.1/.well-known"}}),
        )
        .unwrap();
        // cimd on the default callback path.
        validate_mcp_server_config(
            "srv",
            &json!({"url": "http://x", "oauth": {"clientRegistration": "cimd", "callbackUrl": "http://localhost:8080/callback"}}),
        )
        .unwrap();
        // dcr with client id.
        validate_mcp_server_config(
            "srv",
            &json!({"url": "http://x", "oauth": {"clientRegistration": "dcr", "clientId": "abc"}}),
        )
        .unwrap();
        // Port match.
        validate_mcp_server_config(
            "srv",
            &json!({"url": "http://x", "oauth": {"callbackPort": 8080, "callbackUrl": "http://localhost:8080/callback"}}),
        )
        .unwrap();
        // https URLs pass the auth check.
        validate_mcp_server_config("srv", &json!({"url": "https://example.com", "auth": {"provider": "p"}})).unwrap();
    }

    #[test]
    fn registry_semantics() {
        let mut registry = McpServerRegistry::new();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0usize));
        let counter = calls.clone();
        registry.set_change_listener(Some(Box::new(move |_servers| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })));

        let config = |url: &str| validate_mcp_server_config("a", &json!({ "url": url })).unwrap();
        registry.register(RegisteredMcpServer {
            name: "a".to_string(),
            config: config("http://1"),
            extension_path: "/x".to_string(),
        });
        registry.register(RegisteredMcpServer {
            name: "b".to_string(),
            config: config("http://2"),
            extension_path: "/x".to_string(),
        });
        // Re-registering replaces in place, keeping the original position.
        registry.register(RegisteredMcpServer {
            name: "a".to_string(),
            config: config("http://3"),
            extension_path: "/y".to_string(),
        });
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        let listed = registry.list();
        assert_eq!(
            listed
                .iter()
                .map(|s| (s.name.as_str(), s.config.url().unwrap(), s.extension_path.as_str()))
                .collect::<Vec<_>>(),
            vec![("a", "http://3", "/y"), ("b", "http://2", "/x")]
        );
        // list() hands out deep clones.
        let mut mutated = registry.list();
        mutated[0].config.fields.insert("url".to_string(), json!("http://mutated"));
        assert_eq!(registry.get("a").unwrap().config.url().unwrap(), "http://3");
        // Unregister with the wrong owner is a no-op (no change event).
        registry.unregister("a", "/wrong");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert_eq!(registry.list().len(), 2);
        // Unregistering an unknown server is also a no-op.
        registry.unregister("nope", "/x");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        // Right owner removes and fires the listener.
        registry.unregister("a", "/y");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 4);
        assert_eq!(registry.list().iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["b"]);
        assert!(registry.get("a").is_none());
        // The listener can be cleared.
        registry.set_change_listener(None);
        registry.unregister("b", "/x");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 4);
    }
}
