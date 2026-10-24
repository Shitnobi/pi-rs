//! Parity-harness entry for the `mcp-log` protocol:
//! `pi_extensions::mcp::log` (TS `extensions/mcp/log.ts`) —
//! `formatMcpLogMessage` and `McpServerLog` append/rotation semantics over
//! an in-memory file map. TS mirror: parity/ts-harness.js `mcp-log` entry
//! (mkdtemp base, real fs, `new Date()` pinned through a swapped global
//! Date constructor, files read back relative to the base).
//!
//! args:
//! ```json
//! {
//!   "files": { "<rel path>": "<content>" | { "unit": str, "count": int }, ... },
//!   "steps": [
//!     { "op": "format", "server": str, "params"?: json, "nowMs"?: int },
//!     { "op": "write", "path": rel, "server": str, "params"?: json,
//!       "nowMs"?: int, "fresh"?: bool },
//!     { "op": "renameFile", "from": rel, "to": rel }
//!   ]
//! }
//! ```
//!
//! `{ unit, count }` seeds expand to `unit` repeated `count` times so
//! rotation cases can cross the 5MB threshold without megabyte payloads.
//! `write` reuses one `McpServerLog` per path (the cached-size state that
//! drives rotation); `fresh: true` uses a brand-new instance for that one
//! write, modeling a second process. Result:
//! `{ results: [{ "line": str } | null], files: { rel: content } }`. The
//! in-memory root is the literal `<base>`, matching the TS harness's path
//! normalization.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use pi_extensions::mcp::log::{McpLogFs, McpServerLog};

const BASE: &str = "<base>";

/// In-memory [`McpLogFs`] driven by the case's `files` map. Shared by the
/// `mcp-log` and `mcp-connection` harnesses.
#[derive(Default)]
pub struct MemoryMcpLogFs {
    pub files: std::sync::Mutex<BTreeMap<String, Vec<u8>>>,
}

impl McpLogFs for MemoryMcpLogFs {
    fn mkdir_all(&self, dir: &str) -> Result<(), String> {
        // Fail when an ancestor of `dir` exists as a file (ENOTDIR), like
        // `mkdirSync(dir, { recursive: true })` on the real fs.
        if dir.is_empty() || dir == "." {
            return Ok(());
        }
        let mut prefix = String::new();
        for part in dir.split('/') {
            if part.is_empty() {
                continue;
            }
            if !prefix.is_empty() || dir.starts_with('/') {
                prefix.push('/');
            }
            prefix.push_str(part);
            if self.files.lock().unwrap().contains_key(&prefix) {
                return Err(format!("ENOTDIR: not a directory, mkdir '{}'", dir));
            }
        }
        Ok(())
    }
    fn size(&self, path: &str) -> Result<u64, String> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .map(|b| b.len() as u64)
            .ok_or_else(|| format!("ENOENT: no such file or directory, stat '{}'", path))
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        let mut files = self.files.lock().unwrap();
        match files.remove(from) {
            Some(content) => {
                files.insert(to.to_string(), content);
                Ok(())
            }
            None => Err(format!("ENOENT: no such file or directory, rename '{}'", from)),
        }
    }
    fn append(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        self.files
            .lock()
            .unwrap()
            .entry(path.to_string())
            .or_default()
            .extend_from_slice(bytes);
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

/// `optStr(step, key, msg)` — missing or null ⇒ `None`.
fn opt_str_field(obj: &Map<String, Value>, key: &str, msg: &str) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(msg.to_string()),
    }
}

/// `intNow(step)` — missing ⇒ 0, anything non-integer throws.
fn now_ms_field(obj: &Map<String, Value>) -> Result<i64, String> {
    match obj.get("nowMs") {
        None => Ok(0),
        Some(v) => v
            .as_i64()
            .ok_or_else(|| "nowMs must be an integer".to_string()),
    }
}

/// Expand the `files` map: strings stay, `{ unit, count }` repeats.
fn seed_files(input: &Value) -> Result<Vec<(String, Vec<u8>)>, String> {
    let map = match input {
        Value::Null => return Ok(Vec::new()),
        Value::Object(map) => map,
        _ => return Err("files must be an object".to_string()),
    };
    let mut out = Vec::new();
    for (rel, content) in map {
        match content {
            Value::String(s) => out.push((rel.clone(), s.as_bytes().to_vec())),
            Value::Object(spec) => {
                let unit = match spec.get("unit") {
                    Some(Value::String(s)) => s.clone(),
                    _ => return Err("files values must be a string or {unit, count}".to_string()),
                };
                let count = match spec.get("count") {
                    Some(v) => v
                        .as_u64()
                        .ok_or_else(|| "files values must be a string or {unit, count}".to_string())?,
                    None => return Err("files values must be a string or {unit, count}".to_string()),
                };
                let mut bytes = Vec::with_capacity(unit.len() * count as usize);
                for _ in 0..count {
                    bytes.extend_from_slice(unit.as_bytes());
                }
                out.push((rel.clone(), bytes));
            }
            _ => return Err("files values must be a string or {unit, count}".to_string()),
        }
    }
    Ok(out)
}

pub fn run_mcp_log_harness(input: &Value) -> Result<Value, String> {
    let files = seed_files(input.get("files").unwrap_or(&Value::Null))?;
    let steps = match input.get("steps") {
        Some(Value::Array(steps)) => steps.clone(),
        _ => return Err("missing steps".to_string()),
    };
    let fs = MemoryMcpLogFs::default();
    for (rel, content) in files {
        fs.files.lock().unwrap().insert(join_base(&rel), content);
    }
    let mut loggers: BTreeMap<String, McpServerLog> = BTreeMap::new();
    let mut results: Vec<Value> = Vec::new();
    for step in &steps {
        let obj = step
            .as_object()
            .ok_or_else(|| "steps entries must be objects".to_string())?;
        let op = obj
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing op".to_string())?;
        match op {
            "format" => {
                let server = opt_str_field(obj, "server", "server must be a string")?
                    .ok_or_else(|| "missing server".to_string())?;
                let now_ms = now_ms_field(obj)?;
                let line = pi_extensions::mcp::log::format_mcp_log_message(
                    &server,
                    obj.get("params"),
                    now_ms,
                );
                results.push(json!({ "line": line }));
            }
            "write" => {
                let path = opt_str_field(obj, "path", "path must be a string")?
                    .ok_or_else(|| "missing path".to_string())?;
                let server = opt_str_field(obj, "server", "server must be a string")?
                    .ok_or_else(|| "missing server".to_string())?;
                let fresh = match obj.get("fresh") {
                    None => false,
                    Some(Value::Bool(b)) => *b,
                    Some(_) => return Err("fresh must be a boolean".to_string()),
                };
                let now_ms = now_ms_field(obj)?;
                let params = obj.get("params");
                let full = join_base(&path);
                if fresh {
                    // A second process: a brand-new instance for this write.
                    let mut logger = McpServerLog::new(full);
                    logger.write(&fs, &server, params, now_ms);
                } else {
                    let logger = loggers
                        .entry(full.clone())
                        .or_insert_with(|| McpServerLog::new(full.clone()));
                    logger.write(&fs, &server, params, now_ms);
                }
                results.push(Value::Null);
            }
            "renameFile" => {
                let from = opt_str_field(obj, "from", "from must be a string")?
                    .ok_or_else(|| "missing from".to_string())?;
                let to = opt_str_field(obj, "to", "to must be a string")?
                    .ok_or_else(|| "missing to".to_string())?;
                fs.rename(&join_base(&from), &join_base(&to))?;
                results.push(Value::Null);
            }
            other => return Err(format!("unknown mcp-log op: {}", other)),
        }
    }
    let mut out_files = Map::new();
    for (path, bytes) in fs.files.lock().unwrap().iter() {
        let rel = path
            .strip_prefix(&format!("{}/", BASE))
            .unwrap_or(path)
            .to_string();
        out_files.insert(rel, json!(String::from_utf8_lossy(bytes)));
    }
    Ok(json!({ "results": results, "files": out_files }))
}
