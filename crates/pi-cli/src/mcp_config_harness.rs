//! Parity-harness entry for the `mcp-config` protocol:
//! `pi_session::mcp_config` (TS `extensions/mcp/config.ts`) — load / update /
//! add / remove over an in-memory file map. TS mirror: parity/ts-harness.js
//! `mcp-config` entry (mkdtemp base, real fs, `<base>`-normalized results,
//! files read back relative to the base).
//!
//! args:
//! ```json
//! {
//!   "files": { "<rel path>": "<content>", ... },
//!   "steps": [
//!     { "op": "load", "agentDir"?: rel, "cwd"?: rel, "projectTrusted"?: bool },
//!     { "op": "update", "path": rel, "name": str,
//!       "patch": { "enabled"?: bool, "exposure"?: str }, "override"?: bool },
//!     { "op": "add", "path": rel, "name": str, "config": obj },
//!     { "op": "remove", "path": rel, "name": str }
//!   ]
//! }
//! ```
//!
//! Result: `{ results: [loadResult | true/false/null | { "error": msg }],
//! files: { rel: content } }`. The in-memory root is the literal `<base>`,
//! which is what the TS harness's path normalization produces, so paths in
//! error messages and entries compare equal.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use pi_session::mcp_config::{
    add_mcp_server_config, load_mcp_config, remove_mcp_server_config, update_mcp_server_config,
    LoadMcpConfigOptions, McpConfigFs, McpServerConfigPatch,
};

const BASE: &str = "<base>";

/// In-memory [`McpConfigFs`] driven by the case's `files` map.
#[derive(Default)]
struct MemoryMcpConfigFs {
    files: std::cell::RefCell<BTreeMap<String, String>>,
}

impl McpConfigFs for MemoryMcpConfigFs {
    fn exists(&self, path: &str) -> bool {
        self.files.borrow().contains_key(path)
    }
    fn read(&self, path: &str) -> Result<String, String> {
        self.files
            .borrow()
            .get(path)
            .cloned()
            .ok_or_else(|| format!("ENOENT: no such file or directory, open '{}'", path))
    }
    fn write(&self, path: &str, contents: &str) -> Result<(), String> {
        self.files.borrow_mut().insert(path.to_string(), contents.to_string());
        Ok(())
    }
    fn mkdir_all(&self, _dir: &str) -> Result<(), String> {
        Ok(())
    }
}

/// `parityJoinRel(base, rel)` — leading slashes trimmed, ""/"." → base.
fn join_base(rel: &str) -> String {
    let trimmed = rel.trim_start_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        return BASE.to_string();
    }
    format!("{}/{}", BASE.trim_end_matches('/'), trimmed)
}

fn opt_str_field(obj: &Map<String, Value>, key: &str, msg: &str) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(msg.to_string()),
    }
}

fn opt_bool_field(obj: &Map<String, Value>, key: &str, msg: &str) -> Result<Option<bool>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(msg.to_string()),
    }
}

fn entry_json(entry: &pi_session::mcp_config::McpServerEntry) -> Value {
    let mut out = Map::new();
    out.insert("name".to_string(), json!(entry.name));
    out.insert(
        "config".to_string(),
        serde_json::to_value(&entry.config).unwrap_or(Value::Null),
    );
    out.insert("source".to_string(), json!(entry.source));
    out.insert(
        "scope".to_string(),
        json!(entry.scope.map(|s| s.as_str())),
    );
    if let Some(path) = &entry.r#override {
        out.insert("override".to_string(), json!(path));
    }
    Value::Object(out)
}

pub fn run_mcp_config_harness(input: &Value) -> Result<Value, String> {
    let files = match input.get("files") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(map)) => {
            let mut out = Vec::new();
            for (rel, content) in map {
                let content = content
                    .as_str()
                    .ok_or("files values must be a string or {symlink}")?;
                out.push((rel.clone(), content.to_string()));
            }
            out
        }
        Some(_) => return Err("files must be an object".to_string()),
    };
    let steps = match input.get("steps") {
        Some(Value::Array(items)) => items.clone(),
        _ => return Err("missing steps".to_string()),
    };
    let fs = MemoryMcpConfigFs::default();
    for (rel, content) in files {
        fs.files.borrow_mut().insert(join_base(&rel), content);
    }

    let mut results = Vec::with_capacity(steps.len());
    for step in &steps {
        let step = step
            .as_object()
            .ok_or("steps entries must be objects".to_string())?;
        let op = step
            .get("op")
            .and_then(Value::as_str)
            .ok_or("missing op".to_string())?;
        let result = match op {
            "load" => {
                let agent_dir = opt_str_field(step, "agentDir", "agentDir must be a string")?
                    .unwrap_or_else(|| "agent".to_string());
                let cwd = opt_str_field(step, "cwd", "cwd must be a string")?
                    .unwrap_or_else(|| "cwd".to_string());
                let project_trusted =
                    opt_bool_field(step, "projectTrusted", "projectTrusted must be a boolean")?
                        .unwrap_or(false);
                let loaded = load_mcp_config(
                    &fs,
                    &LoadMcpConfigOptions {
                        agent_dir: &join_base(&agent_dir),
                        cwd: &join_base(&cwd),
                        project_trusted,
                        config_dir_name: ".pi",
                    },
                );
                let mut out = Map::new();
                out.insert(
                    "servers".to_string(),
                    Value::Array(loaded.servers.iter().map(entry_json).collect()),
                );
                if let Some(flag) = loaded.auto_enable_codemode {
                    out.insert("autoEnableCodemode".to_string(), json!(flag));
                }
                out.insert("errors".to_string(), json!(loaded.errors));
                if let Some(path) = loaded.project_config {
                    out.insert("projectConfig".to_string(), json!(path));
                }
                Value::Object(out)
            }
            "update" => {
                let path = opt_str_field(step, "path", "path must be a string")?
                    .ok_or("missing path".to_string())?;
                let name = opt_str_field(step, "name", "name must be a string")?
                    .ok_or("missing name".to_string())?;
                let patch_in = match step.get("patch") {
                    None | Some(Value::Null) => json!({}),
                    Some(v @ Value::Object(_)) => v.clone(),
                    Some(_) => return Err("patch must be an object".to_string()),
                };
                let patch_obj = patch_in.as_object().expect("checked");
                let patch = McpServerConfigPatch {
                    enabled: opt_bool_field(patch_obj, "enabled", "patch.enabled must be a boolean")?,
                    exposure: opt_str_field(patch_obj, "exposure", "patch.exposure must be a string")?,
                };
                let add_as_override =
                    opt_bool_field(step, "override", "override must be a boolean")?.unwrap_or(false);
                match update_mcp_server_config(
                    &fs,
                    &join_base(&path),
                    &name,
                    &patch,
                    add_as_override,
                ) {
                    Ok(()) => Value::Null,
                    Err(message) => json!({ "error": message }),
                }
            }
            "add" => {
                let path = opt_str_field(step, "path", "path must be a string")?
                    .ok_or("missing path".to_string())?;
                let name = opt_str_field(step, "name", "name must be a string")?
                    .ok_or("missing name".to_string())?;
                let config = step.get("config").ok_or("missing config")?;
                match add_mcp_server_config(&fs, &join_base(&path), &name, config) {
                    Ok(replaced) => json!(replaced),
                    Err(message) => json!({ "error": message }),
                }
            }
            "remove" => {
                let path = opt_str_field(step, "path", "path must be a string")?
                    .ok_or("missing path".to_string())?;
                let name = opt_str_field(step, "name", "name must be a string")?
                    .ok_or("missing name".to_string())?;
                match remove_mcp_server_config(&fs, &join_base(&path), &name) {
                    Ok(removed) => json!(removed),
                    Err(message) => json!({ "error": message }),
                }
            }
            other => return Err(format!("unknown mcp-config op: {}", other)),
        };
        results.push(result);
    }

    let mut out_files = Map::new();
    for (path, content) in fs.files.borrow().iter() {
        let rel = path.strip_prefix(&format!("{}/", BASE)).unwrap_or(path);
        out_files.insert(rel.to_string(), Value::String(content.clone()));
    }
    Ok(json!({ "results": results, "files": out_files }))
}
