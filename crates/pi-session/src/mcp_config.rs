//! MCP server configuration files (`mcp.json`).
//!
//! Port of `src/extensions/mcp/config.ts` (packages/coding-agent). Servers
//! are read from `mcp.json` in the agent directory and, for trusted
//! projects, from `<project>/.pi/mcp.json`. Project entries replace global
//! entries with the same name; a project entry without `command`, `url` or
//! `type` overrides only `enabled`, `exposure` and `toolExposure` of the
//! global server (the rest is kept, including credentials).
//!
//! TS line → Rust item map:
//! - L84–89 `OVERRIDE_KEYS` / `isOverride` → [`OVERRIDE_KEYS`],
//!   [`is_override_value`]
//! - L91–93 `isRecord` → `Value::is_object`
//! - L95–152 `readConfigFile` → [`read_config_file`]
//! - L159–174 `loadMcpConfig` → [`load_mcp_config`]
//! - L183–204 `updateMcpServerConfig` → [`update_mcp_server_config`]
//! - L211–221 `addMcpServerConfig` → [`add_mcp_server_config`]
//! - L224–232 `removeMcpServerConfig` → [`remove_mcp_server_config`]
//! - L239–242 `editMcpServers` → [`edit_mcp_servers`]
//!
//! All filesystem access is behind [`McpConfigFs`] so tests and the parity
//! harness can drive an in-memory file map; [`RealMcpConfigFs`] is the thin
//! `std::fs` adapter. Validation and the namespace check reuse
//! `pi_core::mcp_servers` (already ported). JSON is parsed with
//! `pi_core::settings_manager::parse_json_like` (Node-compatible `JSON.parse`
//! error messages) and written with a JS-compatible `JSON.stringify(value,
//! null, indent)` ([`js_stringify_gap`]), preserving key insertion order
//! (`serde_json` `preserve_order`).

use pi_core::mcp_servers::{mcp_namespace, validate_mcp_server_config, McpServerConfig};
use pi_core::settings_manager::parse_json_like;
use serde_json::{Map, Value};

/// Keys a project override may set (`OVERRIDE_KEYS`, L84).
pub const OVERRIDE_KEYS: [&str; 3] = ["enabled", "exposure", "toolExposure"];

/// Filesystem operations the config loader needs, so tests and the parity
/// harness can substitute an in-memory map.
pub trait McpConfigFs {
    /// `existsSync(path)`.
    fn exists(&self, path: &str) -> bool;
    /// `readFileSync(path, "utf8")` — the error string is the io error's
    /// message (parity runs never hit this; `exists` guards the reads).
    fn read(&self, path: &str) -> Result<String, String>;
    /// `writeFileSync(path, contents)`.
    fn write(&self, path: &str, contents: &str) -> Result<(), String>;
    /// `mkdirSync(dir, { recursive: true })`.
    fn mkdir_all(&self, dir: &str) -> Result<(), String>;
}

/// The real filesystem (`node:fs` adapter).
pub struct RealMcpConfigFs;

impl McpConfigFs for RealMcpConfigFs {
    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }
    fn read(&self, path: &str) -> Result<String, String> {
        std::fs::read_to_string(path).map_err(|e| e.to_string())
    }
    fn write(&self, path: &str, contents: &str) -> Result<(), String> {
        std::fs::write(path, contents).map_err(|e| e.to_string())
    }
    fn mkdir_all(&self, dir: &str) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())
    }
}

/// Which `mcp.json` (or extension registration) defined a server (`scope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigScope {
    Global,
    Project,
    Extension,
}

impl McpConfigScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            McpConfigScope::Global => "global",
            McpConfigScope::Project => "project",
            McpConfigScope::Extension => "extension",
        }
    }
}

/// One entry of the loaded config (`McpServerEntry`, L61).
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerEntry {
    pub name: String,
    pub config: McpServerConfig,
    /// Config file that defined the entry, or the path of the extension that
    /// registered it.
    pub source: String,
    /// The global or the project `mcp.json`, or `extension` for
    /// `pi.registerMcpServer()` servers.
    pub scope: Option<McpConfigScope>,
    /// Project `mcp.json` with an override of this global server's
    /// `enabled`, `exposure` or `toolExposure` (TS `override`).
    pub r#override: Option<String>,
}

/// `loadMcpConfig`'s return value (`LoadedMcpConfig`, L76). Fields the TS
/// omits when `undefined` are `Option`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LoadedMcpConfig {
    pub servers: Vec<McpServerEntry>,
    /// Activate the codemode tool when `codemode` servers connect
    /// (default true — `None` when no file set it).
    pub auto_enable_codemode: Option<bool>,
    pub errors: Vec<String>,
    /// The project `mcp.json` when the project is trusted, where `/mcp`
    /// saves project overrides.
    pub project_config: Option<String>,
}

/// Options of [`load_mcp_config`] (`loadMcpConfig`, L159).
pub struct LoadMcpConfigOptions<'a> {
    pub agent_dir: &'a str,
    pub cwd: &'a str,
    pub project_trusted: bool,
    /// `CONFIG_DIR_NAME` — `pkg.piConfig?.configDir || ".pi"`.
    pub config_dir_name: &'a str,
}

/// `McpConfigState` (L95): a `Map` with insertion order — `set` on an
/// existing name updates in place, keeping the position.
#[derive(Default)]
struct McpConfigState {
    servers: Vec<(String, McpServerEntry)>,
    auto_enable_codemode: Option<bool>,
    errors: Vec<String>,
}

impl McpConfigState {
    fn get(&self, name: &str) -> Option<&McpServerEntry> {
        self.servers.iter().find(|(n, _)| n == name).map(|(_, e)| e)
    }

    fn set(&mut self, name: &str, entry: McpServerEntry) {
        match self.servers.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = entry,
            None => self.servers.push((name.to_string(), entry)),
        }
    }
}

/// Whether an entry overrides a server defined elsewhere instead of defining
/// one: no `command`, `url` or `type` key (`isOverride`, L87). JSON cannot
/// hold `undefined`, so `command: null` makes it a definition.
fn is_override_value(value: &Value) -> bool {
    match value.as_object() {
        Some(map) => {
            !map.contains_key("command") && !map.contains_key("url") && !map.contains_key("type")
        }
        None => false,
    }
}

/// JS truthiness for JSON values (`config.auth` check).
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::Bool(true) => true,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `path.join` for the two- and three-segment joins this module does.
fn join_path(a: &str, b: &str) -> String {
    if a.is_empty() {
        return b.to_string();
    }
    if a.ends_with('/') {
        format!("{}{}", a, b)
    } else {
        format!("{}/{}", a, b)
    }
}

/// `dirname(path)`.
fn parent_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => path[..i].to_string(),
        None => ".".to_string(),
    }
}

/// Read an `mcp.json` and merge it into `state` (`readConfigFile`, L98).
/// Validation errors are collected byte-exact; a bad file never aborts the
/// load. (Module-private like the TS `readConfigFile`.)
fn read_config_file(
    fs: &dyn McpConfigFs,
    path: &str,
    scope: McpConfigScope,
    state: &mut McpConfigState,
) {
    if !fs.exists(path) {
        return;
    }
    // TS wraps the read and the parse in one try: either error becomes
    // `${path}: ${message}`.
    let parsed = match fs.read(path).and_then(|text| parse_json_like(&text)) {
        Ok(v) => v,
        Err(msg) => {
            state.errors.push(format!("{}: {}", path, msg));
            return;
        }
    };
    let Some(obj) = parsed.as_object() else {
        state
            .errors
            .push(format!("{}: expected an object with an \"mcpServers\" object", path));
        return;
    };
    if let Some(servers) = obj.get("mcpServers") {
        if !servers.is_object() {
            state
                .errors
                .push(format!("{}: expected an object with an \"mcpServers\" object", path));
            return;
        }
    }
    match obj.get("autoEnableCodemode") {
        Some(Value::Bool(b)) => state.auto_enable_codemode = Some(*b),
        Some(_) => state
            .errors
            .push(format!("{}: autoEnableCodemode must be a boolean", path)),
        None => {}
    }
    let empty = Map::new();
    // `parsed.mcpServers ?? {}` — `null` was rejected above, so only an
    // absent key falls back to the empty map.
    let servers_map = match obj.get("mcpServers") {
        Some(Value::Object(map)) => map,
        _ => &empty,
    };
    for (name, value) in servers_map {
        if scope == McpConfigScope::Project && value.is_object() && is_override_value(value) {
            let value_map = value.as_object().expect("checked is_object");
            let base = state.get(name);
            let extra = value_map
                .keys()
                .any(|key| !OVERRIDE_KEYS.contains(&key.as_str()));
            match base {
                None => state.errors.push(format!(
                    "{}: server \"{}\" needs \"command\" or \"url\", or a global server to override",
                    path, name
                )),
                Some(_) if extra => state.errors.push(format!(
                    "{}: server \"{}\": an override can only set {}",
                    path,
                    name,
                    OVERRIDE_KEYS.join(", ")
                )),
                Some(base) => {
                    // `{ ...base.config, ...value }`: first-position keys
                    // kept, new keys appended (preserve_order `insert`).
                    let mut merged = base.config.fields.clone();
                    for (key, entry) in value_map {
                        merged.insert(key.clone(), entry.clone());
                    }
                    match validate_mcp_server_config(name, &Value::Object(merged)) {
                        Err(msg) => state.errors.push(format!("{}: {}", path, msg)),
                        Ok(config) => {
                            // `{ ...base, config, override: path }` — name,
                            // source and scope are kept from the base entry.
                            let mut entry = base.clone();
                            entry.config = config;
                            entry.r#override = Some(path.to_string());
                            state.set(name, entry);
                        }
                    }
                }
            }
            continue;
        }
        let config = match validate_mcp_server_config(name, value) {
            Ok(config) => config,
            Err(msg) => {
                state.errors.push(format!("{}: {}", path, msg));
                continue;
            }
        };
        // Names that differ only in `-` and `_` would share a namespace;
        // the first existing entry (map order) wins.
        let clash = state
            .servers
            .iter()
            .find(|(other, _)| other.as_str() != name && mcp_namespace(other) == mcp_namespace(name))
            .map(|(other, _)| other.clone());
        if let Some(clash) = clash {
            state
                .errors
                .push(format!("{}: server \"{}\" conflicts with \"{}\"", path, name, clash));
            continue;
        }
        // `"url" in config && config.auth` — the key may be present (even
        // null) on stdio configs, and `auth` must be truthy.
        if scope == McpConfigScope::Project
            && config.fields.contains_key("url")
            && config.fields.get("auth").is_some_and(js_truthy)
        {
            state.errors.push(format!(
                "{}: server \"{}\": auth is only allowed in the global mcp.json",
                path, name
            ));
            continue;
        }
        state.set(
            name,
            McpServerEntry {
                name: name.clone(),
                config,
                source: path.to_string(),
                scope: Some(scope),
                r#override: None,
            },
        );
    }
}

/// Load global and (when trusted) project MCP configuration
/// (`loadMcpConfig`, L159). Disabled servers are included with
/// `enabled: false`, so they can be enabled again.
pub fn load_mcp_config(fs: &dyn McpConfigFs, options: &LoadMcpConfigOptions) -> LoadedMcpConfig {
    let mut state = McpConfigState::default();
    read_config_file(
        fs,
        &join_path(options.agent_dir, "mcp.json"),
        McpConfigScope::Global,
        &mut state,
    );
    let project_config = if options.project_trusted {
        Some(join_path(&join_path(options.cwd, options.config_dir_name), "mcp.json"))
    } else {
        None
    };
    if let Some(path) = &project_config {
        read_config_file(fs, path, McpConfigScope::Project, &mut state);
    }
    LoadedMcpConfig {
        servers: state.servers.into_iter().map(|(_, entry)| entry).collect(),
        auto_enable_codemode: state.auto_enable_codemode,
        errors: state.errors,
        project_config,
    }
}

/// Settings `/mcp` changes (`McpServerConfigPatch`, L178): `enabled: true`
/// and `exposure: "codemode"` are the defaults and remove the key. The
/// exposure is kept as the raw string so parity cases can exercise the exact
/// TS comparison (`patch.exposure === "codemode"`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct McpServerConfigPatch {
    pub enabled: Option<bool>,
    pub exposure: Option<String>,
}

/// Change one server's settings in the `mcp.json` that defines or overrides
/// it (`updateMcpServerConfig`, L183). With `add_as_override`, a missing
/// entry is added as an override. Overrides keep default values, since they
/// replace the global server's. Other content is kept; the file is rewritten
/// with its indentation.
pub fn update_mcp_server_config(
    fs: &dyn McpConfigFs,
    path: &str,
    name: &str,
    patch: &McpServerConfigPatch,
    add_as_override: bool,
) -> Result<(), String> {
    edit_mcp_servers(fs, path, |parsed| {
        // `let server = servers?.[name]` — `servers` is `parsed.mcpServers`
        // when it is a record, else undefined (a non-record was rejected).
        let server = match parsed.get("mcpServers") {
            Some(Value::Object(map)) => map.get(name).cloned(),
            _ => None,
        };
        let existed = server.is_some();
        if !existed && !add_as_override {
            return Err(format!("{} does not define MCP server \"{}\"", path, name));
        }
        if !existed {
            // `parsed.mcpServers = { ...servers, [name]: server }` — the
            // spread keeps existing keys in order and appends the new one.
            let mut new_map = match parsed.get("mcpServers") {
                Some(Value::Object(map)) => map.clone(),
                _ => Map::new(),
            };
            new_map.insert(name.to_string(), Value::Object(Map::new()));
            parsed
                .as_object_mut()
                .expect("edit_mcp_servers checked is_object")
                .insert("mcpServers".to_string(), Value::Object(new_map));
        }
        let Some(entry) = parsed
            .get_mut("mcpServers")
            .and_then(Value::as_object_mut)
            .and_then(|map| map.get_mut(name))
            .and_then(Value::as_object_mut)
        else {
            return Err(format!("{} does not define MCP server \"{}\"", path, name));
        };
        let keep_defaults = is_override_value(&Value::Object(entry.clone()));
        if let Some(enabled) = patch.enabled {
            if enabled && !keep_defaults {
                entry.shift_remove("enabled");
            } else {
                entry.insert("enabled".to_string(), Value::Bool(enabled));
            }
        }
        if let Some(exposure) = &patch.exposure {
            if exposure == "codemode" && !keep_defaults {
                entry.shift_remove("exposure");
            } else {
                entry.insert("exposure".to_string(), Value::String(exposure.clone()));
            }
        }
        Ok(true)
    })
}

/// Add a server to an `mcp.json`, creating the file when missing
/// (`addMcpServerConfig`, L211). An existing entry with the same name is
/// replaced (position kept). Returns true when an entry was replaced. The
/// config is stored as given — the TS does not validate it here.
pub fn add_mcp_server_config(
    fs: &dyn McpConfigFs,
    path: &str,
    name: &str,
    config: &Value,
) -> Result<bool, String> {
    let mut replaced = false;
    edit_mcp_servers(fs, path, |parsed| {
        // `const target = servers ?? {}` — a fresh map when `mcpServers` is
        // absent; `parsed.mcpServers = target` re-assigns it either way.
        if !matches!(parsed.get("mcpServers"), Some(Value::Object(_))) {
            parsed
                .as_object_mut()
                .expect("edit_mcp_servers checked is_object")
                .insert("mcpServers".to_string(), Value::Object(Map::new()));
        }
        let map = parsed
            .get_mut("mcpServers")
            .and_then(Value::as_object_mut)
            .expect("just inserted");
        replaced = map.contains_key(name);
        map.insert(name.to_string(), config.clone());
        Ok(true)
    })?;
    Ok(replaced)
}

/// Remove a server from an `mcp.json` (`removeMcpServerConfig`, L224).
/// Returns false when the file does not define it (and writes nothing).
pub fn remove_mcp_server_config(fs: &dyn McpConfigFs, path: &str, name: &str) -> Result<bool, String> {
    if !fs.exists(path) {
        return Ok(false);
    }
    let mut removed = false;
    edit_mcp_servers(fs, path, |parsed| {
        if let Some(map) = parsed.get_mut("mcpServers").and_then(Value::as_object_mut) {
            if map.shift_remove(name).is_some() {
                removed = true;
                return Ok(true);
            }
        }
        Ok(false)
    })?;
    Ok(removed)
}

/// Read an `mcp.json` (an empty config when missing), let `edit` change its
/// `mcpServers`, and write it back with its indentation when `edit` returns
/// true (`editMcpServers`, L239). Other content is kept. Parse errors throw
/// with the raw `JSON.parse` message (no path prefix), unlike the load path.
fn edit_mcp_servers(
    fs: &dyn McpConfigFs,
    path: &str,
    edit: impl FnOnce(&mut Value) -> Result<bool, String>,
) -> Result<(), String> {
    let text = if fs.exists(path) {
        Some(fs.read(path)?)
    } else {
        None
    };
    let mut parsed = match &text {
        Some(text) => parse_json_like(text)?,
        None => Value::Object(Map::new()),
    };
    let ok = parsed.is_object()
        && match parsed.get("mcpServers") {
            None => true,
            Some(servers) => servers.is_object(),
        };
    if !ok {
        return Err(format!("{}: expected an object with an \"mcpServers\" object", path));
    }
    if !edit(&mut parsed)? {
        return Ok(());
    }
    // `(text && /^([ \t]+)\S/m.exec(text)?.[1]) || "  "` — the first line
    // that starts with a run of spaces/tabs followed by a non-whitespace
    // character contributes that run; anything else falls back to two
    // spaces. An empty (but existing) file is falsy in JS.
    let indent = sniff_indent(text.as_deref());
    fs.mkdir_all(&parent_dir(path))?;
    fs.write(path, &format!("{}\n", js_stringify_gap(&parsed, &indent)))?;
    Ok(())
}

/// The indent-sniffing regex above.
fn sniff_indent(text: Option<&str>) -> String {
    let Some(text) = text else {
        return "  ".to_string();
    };
    if text.is_empty() {
        return "  ".to_string();
    }
    for line in text.split('\n') {
        let run = line.len() - line.trim_start_matches([' ', '\t']).len();
        if run > 0 {
            // `\S` — any character that is not JS whitespace.
            let rest = &line[run..];
            if rest
                .chars()
                .next()
                .is_some_and(|c| !pi_core::js_string::is_js_whitespace_or_terminator(c))
            {
                return line[..run].to_string();
            }
        }
    }
    "  ".to_string()
}

// ============================================================================
// JS-compatible JSON.stringify(value, null, gap)
// ============================================================================

/// `JSON.stringify(value)` for one f64 — shortest round-trip like V8
/// (same implementation as `pi_core::mcp_tools::js_f64_to_string`,
/// duplicated like the other copies in this repo).
fn js_f64_to_string(v: f64) -> String {
    if !v.is_finite() {
        return "null".to_string();
    }
    if v == 0.0 {
        return "0".to_string();
    }
    let a = v.abs();
    if (1e-6..1e21).contains(&a) {
        format!("{}", v)
    } else {
        let mut s = format!("{:e}", v);
        if let Some(pos) = s.find('e') {
            if !s[pos + 1..].starts_with('-') {
                s.insert(pos + 1, '+');
            }
        }
        s
    }
}

/// `JSON.stringify(value, null, gap)` for a parsed-JSON value —
/// byte-identical output for objects, arrays, strings, booleans, null and
/// JS numbers. The gap string is truncated to 10 characters like the spec.
fn js_stringify_gap(value: &Value, gap: &str) -> String {
    let gap: String = gap.chars().take(10).collect();
    fn go(value: &Value, gap: &str, depth: usize) -> String {
        let indent = gap.repeat(depth);
        let inner = gap.repeat(depth + 1);
        match value {
            Value::Object(map) => {
                if map.is_empty() {
                    return "{}".to_string();
                }
                let mut s = String::from("{\n");
                let n = map.len();
                for (i, (k, v)) in map.iter().enumerate() {
                    s.push_str(&inner);
                    s.push_str(&serde_json::to_string(&Value::String(k.clone())).unwrap());
                    s.push_str(": ");
                    s.push_str(&go(v, gap, depth + 1));
                    if i + 1 < n {
                        s.push(',');
                    }
                    s.push('\n');
                }
                s.push_str(&indent);
                s.push('}');
                s
            }
            Value::Array(items) => {
                if items.is_empty() {
                    return "[]".to_string();
                }
                let mut s = String::from("[\n");
                let n = items.len();
                for (i, v) in items.iter().enumerate() {
                    s.push_str(&inner);
                    s.push_str(&go(v, gap, depth + 1));
                    if i + 1 < n {
                        s.push(',');
                    }
                    s.push('\n');
                }
                s.push_str(&indent);
                s.push(']');
                s
            }
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    i.to_string()
                } else if let Some(u) = n.as_u64() {
                    u.to_string()
                } else {
                    js_f64_to_string(n.as_f64().unwrap_or(0.0))
                }
            }
            other => serde_json::to_string(other).unwrap_or_default(),
        }
    }
    go(value, &gap, 0)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// In-memory [`McpConfigFs`] (the parity harness drives the same shape).
    #[derive(Default)]
    struct MemoryFs {
        files: std::cell::RefCell<std::collections::BTreeMap<String, String>>,
    }

    impl McpConfigFs for MemoryFs {
        fn exists(&self, path: &str) -> bool {
            self.files.borrow().contains_key(path)
        }
        fn read(&self, path: &str) -> Result<String, String> {
            self.files.borrow().get(path).cloned().ok_or_else(|| "not found".to_string())
        }
        fn write(&self, path: &str, contents: &str) -> Result<(), String> {
            self.files.borrow_mut().insert(path.to_string(), contents.to_string());
            Ok(())
        }
        fn mkdir_all(&self, _dir: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn mem(files: &[(&str, &str)]) -> MemoryFs {
        let fs = MemoryFs::default();
        for (path, content) in files {
            fs.files.borrow_mut().insert((*path).to_string(), (*content).to_string());
        }
        fs
    }

    #[test]
    fn load_merges_global_and_project() {
        let fs = mem(&[
            (
                "/agent/mcp.json",
                r#"{"mcpServers":{"fs":{"command":"npx"},"docs":{"url":"https://x/mcp","auth":{"provider":"p"}}}}"#,
            ),
            (
                "/proj/.pi/mcp.json",
                r#"{"mcpServers":{"fs":{"enabled":false},"own":{"command":"bin"}}}"#,
            ),
        ]);
        let loaded = load_mcp_config(
            &fs,
            &LoadMcpConfigOptions {
                agent_dir: "/agent",
                cwd: "/proj",
                project_trusted: true,
                config_dir_name: ".pi",
            },
        );
        assert!(loaded.errors.is_empty());
        assert_eq!(loaded.project_config.as_deref(), Some("/proj/.pi/mcp.json"));
        assert_eq!(
            loaded.servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["fs", "docs", "own"]
        );
        // The override keeps the global source/scope and merges the config.
        let fs_entry = &loaded.servers[0];
        assert_eq!(fs_entry.source, "/agent/mcp.json");
        assert_eq!(fs_entry.scope, Some(McpConfigScope::Global));
        assert_eq!(fs_entry.r#override.as_deref(), Some("/proj/.pi/mcp.json"));
        assert_eq!(fs_entry.config.fields.get("command"), Some(&json!("npx")));
        assert_eq!(fs_entry.config.fields.get("enabled"), Some(&json!(false)));
        // Project replacement.
        let own = &loaded.servers[2];
        assert_eq!(own.source, "/proj/.pi/mcp.json");
        assert_eq!(own.scope, Some(McpConfigScope::Project));
    }

    #[test]
    fn override_without_base_is_an_error() {
        let fs = mem(&[("/proj/.pi/mcp.json", r#"{"mcpServers":{"x":{"enabled":false}}}"#)]);
        let loaded = load_mcp_config(
            &fs,
            &LoadMcpConfigOptions {
                agent_dir: "/agent",
                cwd: "/proj",
                project_trusted: true,
                config_dir_name: ".pi",
            },
        );
        assert_eq!(
            loaded.errors,
            vec!["/proj/.pi/mcp.json: server \"x\" needs \"command\" or \"url\", or a global server to override"]
        );
    }

    #[test]
    fn auth_in_project_is_rejected() {
        let fs = mem(&[(
            "/proj/.pi/mcp.json",
            r#"{"mcpServers":{"x":{"url":"https://x/mcp","auth":{"provider":"p"}}}}"#,
        )]);
        let loaded = load_mcp_config(
            &fs,
            &LoadMcpConfigOptions {
                agent_dir: "/agent",
                cwd: "/proj",
                project_trusted: true,
                config_dir_name: ".pi",
            },
        );
        assert_eq!(
            loaded.errors,
            vec!["/proj/.pi/mcp.json: server \"x\": auth is only allowed in the global mcp.json"]
        );
    }

    #[test]
    fn update_removes_default_values() {
        let fs = mem(&[(
            "/a/mcp.json",
            "{\n  \"other\": true,\n  \"mcpServers\": {\n    \"x\": {\n      \"command\": \"bin\",\n      \"enabled\": false\n    }\n  }\n}\n",
        )]);
        update_mcp_server_config(
            &fs,
            "/a/mcp.json",
            "x",
            &McpServerConfigPatch { enabled: Some(true), exposure: Some("codemode".to_string()) },
            false,
        )
        .unwrap();
        assert_eq!(
            fs.files.borrow().get("/a/mcp.json").unwrap(),
            "{\n  \"other\": true,\n  \"mcpServers\": {\n    \"x\": {\n      \"command\": \"bin\"\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn update_sniffs_indent_and_creates_override() {
        let fs = mem(&[("/a/mcp.json", "{\n\t\"mcpServers\": {\n\t\t\"x\": {\"command\": \"bin\"}\n\t}\n}\n")]);
        update_mcp_server_config(
            &fs,
            "/a/mcp.json",
            "y",
            &McpServerConfigPatch { enabled: Some(false), exposure: None },
            true,
        )
        .unwrap();
        assert_eq!(
            fs.files.borrow().get("/a/mcp.json").unwrap(),
            "{\n\t\"mcpServers\": {\n\t\t\"x\": {\n\t\t\t\"command\": \"bin\"\n\t\t},\n\t\t\"y\": {\n\t\t\t\"enabled\": false\n\t\t}\n\t}\n}\n"
        );
    }

    #[test]
    fn update_missing_without_override_throws() {
        let fs = mem(&[("/a/mcp.json", "{}")]);
        let err = update_mcp_server_config(
            &fs,
            "/a/mcp.json",
            "x",
            &McpServerConfigPatch::default(),
            false,
        )
        .unwrap_err();
        assert_eq!(err, "/a/mcp.json does not define MCP server \"x\"");
    }

    #[test]
    fn add_and_remove_round_trip() {
        let fs = MemoryFs::default();
        assert!(!add_mcp_server_config(&fs, "/a/mcp.json", "x", &json!({"command": "bin"})).unwrap());
        assert_eq!(
            fs.files.borrow().get("/a/mcp.json").unwrap(),
            "{\n  \"mcpServers\": {\n    \"x\": {\n      \"command\": \"bin\"\n    }\n  }\n}\n"
        );
        assert!(add_mcp_server_config(&fs, "/a/mcp.json", "x", &json!({"url": "https://x"})).unwrap());
        assert!(remove_mcp_server_config(&fs, "/a/mcp.json", "x").unwrap());
        // Removing the last server leaves an empty mcpServers object.
        assert_eq!(
            fs.files.borrow().get("/a/mcp.json").unwrap(),
            "{\n  \"mcpServers\": {}\n}\n"
        );
        assert!(!remove_mcp_server_config(&fs, "/a/mcp.json", "x").unwrap());
        assert!(!remove_mcp_server_config(&fs, "/missing/mcp.json", "x").unwrap());
    }

    #[test]
    fn real_fs_round_trip() {
        // The std::fs adapter, exercised on a scratch dir (never /tmp — the
        // parity environment wipes it mid-run).
        let base = std::env::current_dir()
            .expect("cwd")
            .join(format!("mcp-config-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let path = base.join("nested").join("mcp.json");
        let path_str = path.to_string_lossy().to_string();
        let fs = RealMcpConfigFs;
        assert!(!add_mcp_server_config(&fs, &path_str, "x", &json!({"command": "bin"})).unwrap());
        let agent_dir = base.join("nested").to_string_lossy().to_string();
        let loaded = load_mcp_config(
            &fs,
            &LoadMcpConfigOptions {
                agent_dir: agent_dir.as_str(),
                cwd: "/nowhere",
                project_trusted: false,
                config_dir_name: ".pi",
            },
        );
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers[0].name, "x");
        assert!(remove_mcp_server_config(&fs, &path_str, "x").unwrap());
        let _ = std::fs::remove_dir_all(&base);
    }
}
