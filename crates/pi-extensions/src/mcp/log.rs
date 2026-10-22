//! Log messages MCP servers send with `notifications/message`, appended to
//! `mcp.log` in the agent directory (`src/extensions/mcp/log.ts`,
//! packages/coding-agent).
//!
//! Several pi processes may write to the same file, so every message is one
//! synchronous append. The file is rotated to `mcp.log.1` once it grows
//! past [`MAX_LOG_BYTES`]. Write errors are ignored: logging must not break
//! tools.
//!
//! TS line → Rust item map:
//! - L11 `MAX_LOG_BYTES` → [`MAX_LOG_BYTES`]
//! - L13–15 `isRecord` → `serde_json::Value::is_object`
//! - L17–23 `formatData` → [`format_data`]
//! - L26–33 `formatMcpLogMessage` → [`format_mcp_log_message`]
//! - L36–67 `McpServerLog` → [`McpServerLog`]
//!
//! Deviations (determinism only, no behavior change):
//! - `now: Date = new Date()` becomes an explicit `now_ms` argument; the
//!   harness injects fixed clocks. `Date#toISOString()` is
//!   [`pi_core::session_manager::iso_string`].
//! - TS `params: unknown` can be `undefined` (missing key ⇒
//!   `{ data: undefined }` ⇒ text `"undefined"`); Rust models that with
//!   `Option<&Value>`. TS `null` stays `Some(Value::Null)`.
//! - `JSON.stringify(data) ?? String(data)` — a `serde_json::Value` can
//!   never be `undefined`/`BigInt`/cyclic, so the `??`/`catch` fallback is
//!   unreachable; numbers stringify like V8 via
//!   [`pi_core::nested_tool_calls::js_json_stringify`].
//! - Filesystem access is behind [`McpLogFs`] (the [`McpServerLog`] shape
//!   of the `McpConfigFs` pattern in `pi_session::mcp_config`) so tests
//!   and the parity harness drive an in-memory file map;
//!   [`RealMcpLogFs`] is the thin `std::fs` adapter.

use pi_core::nested_tool_calls::js_json_stringify;
use pi_core::session_manager::iso_string;
use serde_json::Value;

/// Rotate `mcp.log` to `mcp.log.1` once it grows past this size (L11).
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Filesystem operations the log writer needs, so tests and the parity
/// harness can substitute an in-memory file map.
pub trait McpLogFs {
    /// `mkdirSync(dir, { recursive: true })` — fails when an ancestor of
    /// `dir` exists as a file.
    fn mkdir_all(&self, dir: &str) -> Result<(), String>;
    /// `statSync(path).size`; the error kind is unchecked (missing ⇒ size 0).
    fn size(&self, path: &str) -> Result<u64, String>;
    /// `renameSync(from, to)`.
    fn rename(&self, from: &str, to: &str) -> Result<(), String>;
    /// `appendFileSync(path, bytes)` — creates the file when missing.
    fn append(&self, path: &str, bytes: &[u8]) -> Result<(), String>;
}

/// The real filesystem (`node:fs` adapter).
pub struct RealMcpLogFs;

impl McpLogFs for RealMcpLogFs {
    fn mkdir_all(&self, dir: &str) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())
    }
    fn size(&self, path: &str) -> Result<u64, String> {
        std::fs::metadata(path).map(|m| m.len()).map_err(|e| e.to_string())
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        std::fs::rename(from, to).map_err(|e| e.to_string())
    }
    fn append(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())
    }
}

/// `formatData` (L17): strings pass through, everything else is
/// `JSON.stringify`. `data` missing (TS `undefined`) is the string
/// `"undefined"`.
fn format_data(data: Option<&Value>) -> String {
    match data {
        Some(Value::String(s)) => s.clone(),
        Some(other) => js_json_stringify(other),
        None => "undefined".to_string(),
    }
}

/// `text.replace(/\r?\n/g, "\n    ")` (L31): continuation lines are
/// indented with four spaces; a lone `\r` is kept.
fn indent_continuations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => out.push_str("\n    "),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    out.push_str("\n    ");
                } else {
                    out.push('\r');
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Format one `notifications/message` from `server` as a log line (L26).
/// `params` is the notification's `params` field; `None` models the TS
/// `undefined` argument. `now_ms` is `now.getTime()`.
///
/// ```text
/// ${now.toISOString()} [${server}] ${level}${logger} ${text}\n
/// ```
pub fn format_mcp_log_message(server: &str, params: Option<&Value>, now_ms: i64) -> String {
    // `const message = isRecord(params) ? params : { data: params }`
    let (level, logger, data): (&str, Option<&str>, Option<&Value>) = match params {
        Some(v) if v.is_object() => (
            v.get("level").and_then(Value::as_str).unwrap_or("info"),
            v.get("logger").and_then(Value::as_str),
            v.get("data"),
        ),
        other => ("info", None, other),
    };
    // `typeof message.logger === "string" && message.logger` — empty string
    // is falsy.
    let logger_prefix = match logger {
        Some(l) if !l.is_empty() => format!(" {}:", l),
        _ => String::new(),
    };
    let text = indent_continuations(&format_data(data));
    format!(
        "{} [{}] {}{} {}\n",
        iso_string(now_ms),
        server,
        level,
        logger_prefix,
        text
    )
}

/// `path.dirname` (unix flavor): no slash ⇒ `.`, slash at 0 ⇒ `/`.
fn node_dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => ".",
    }
}

/// Appends server log messages to one file (L36). Write errors are ignored:
/// logging must not break tools.
pub struct McpServerLog {
    /// Log file path (`readonly path`).
    pub path: String,
    /// Cached file size in bytes; `None` until the first write
    /// (`private size: number | undefined`).
    size: Option<u64>,
}

impl McpServerLog {
    pub fn new(path: impl Into<String>) -> Self {
        McpServerLog {
            path: path.into(),
            size: None,
        }
    }

    /// `write(server, params)` (L41) with the injectable clock. Every call
    /// is one synchronous append; errors are swallowed.
    pub fn write(&mut self, fs: &dyn McpLogFs, server: &str, params: Option<&Value>, now_ms: i64) {
        let line = format_mcp_log_message(server, params, now_ms);
        // `catch { /* Ignore: the log is best effort. */ }`
        let _ = self.write_line(fs, line.as_bytes());
    }

    fn write_line(&mut self, fs: &dyn McpLogFs, bytes: &[u8]) -> Result<(), String> {
        if self.size.is_none() {
            fs.mkdir_all(node_dirname(&self.path))?;
            self.size = Some(Self::current_size(fs, &self.path));
        }
        if let Some(size) = self.size {
            if size > MAX_LOG_BYTES {
                // Another process may have rotated it already; check before
                // renaming.
                if Self::current_size(fs, &self.path) > MAX_LOG_BYTES {
                    fs.rename(&self.path, &format!("{}.1", self.path))?;
                }
                self.size = Some(Self::current_size(fs, &self.path));
            }
        }
        fs.append(&self.path, bytes)?;
        self.size = Some(self.size.unwrap_or(0) + bytes.len() as u64);
        Ok(())
    }

    /// `currentSize` (L62): `statSync(path).size`, 0 when missing.
    fn current_size(fs: &dyn McpLogFs, path: &str) -> u64 {
        fs.size(path).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    /// In-memory [`McpLogFs`]: files are byte vectors keyed by full path.
    #[derive(Default)]
    struct MemoryFs {
        files: std::cell::RefCell<BTreeMap<String, Vec<u8>>>,
    }

    impl MemoryFs {
        fn seed(&self, path: &str, content: &str) {
            self.files
                .borrow_mut()
                .insert(path.to_string(), content.as_bytes().to_vec());
        }
        fn content(&self, path: &str) -> String {
            self.files
                .borrow()
                .get(path)
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default()
        }
    }

    impl McpLogFs for MemoryFs {
        fn mkdir_all(&self, dir: &str) -> Result<(), String> {
            // Fail when an ancestor of `dir` exists as a file (ENOTDIR).
            let mut prefix = String::new();
            for part in dir.split('/') {
                if part.is_empty() {
                    continue;
                }
                if !prefix.is_empty() || dir.starts_with('/') {
                    prefix.push('/');
                }
                prefix.push_str(part);
                if self.files.borrow().contains_key(&prefix) {
                    return Err(format!("ENOTDIR: not a directory, mkdir '{}'", dir));
                }
            }
            Ok(())
        }
        fn size(&self, path: &str) -> Result<u64, String> {
            self.files
                .borrow()
                .get(path)
                .map(|b| b.len() as u64)
                .ok_or_else(|| format!("ENOENT: no such file or directory, stat '{}'", path))
        }
        fn rename(&self, from: &str, to: &str) -> Result<(), String> {
            let mut files = self.files.borrow_mut();
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
                .borrow_mut()
                .entry(path.to_string())
                .or_default()
                .extend_from_slice(bytes);
            Ok(())
        }
    }

    #[test]
    fn format_string_data_passthrough() {
        let line = format_mcp_log_message(
            "fs",
            Some(&json!({ "level": "debug", "logger": "mylog", "data": "hello" })),
            0,
        );
        assert_eq!(line, "1970-01-01T00:00:00.000Z [fs] debug mylog: hello\n");
    }

    #[test]
    fn format_defaults_and_missing_data() {
        assert_eq!(
            format_mcp_log_message("fs", Some(&json!({})), 0),
            "1970-01-01T00:00:00.000Z [fs] info undefined\n"
        );
        assert_eq!(
            format_mcp_log_message("fs", None, 0),
            "1970-01-01T00:00:00.000Z [fs] info undefined\n"
        );
        assert_eq!(
            format_mcp_log_message("fs", Some(&json!(null)), 0),
            "1970-01-01T00:00:00.000Z [fs] info null\n"
        );
    }

    #[test]
    fn format_non_record_params_wrap_as_data() {
        assert_eq!(
            format_mcp_log_message("fs", Some(&json!("plain")), 0),
            "1970-01-01T00:00:00.000Z [fs] info plain\n"
        );
        assert_eq!(
            format_mcp_log_message("fs", Some(&json!(42)), 0),
            "1970-01-01T00:00:00.000Z [fs] info 42\n"
        );
        assert_eq!(
            format_mcp_log_message("fs", Some(&json!([1, "a"])), 0),
            "1970-01-01T00:00:00.000Z [fs] info [1,\"a\"]\n"
        );
    }

    #[test]
    fn format_level_and_logger_edge_values() {
        // Non-string level falls back to "info"; empty level is kept.
        assert_eq!(
            format_mcp_log_message("fs", Some(&json!({ "level": 5, "data": "x" })), 0),
            "1970-01-01T00:00:00.000Z [fs] info x\n"
        );
        assert_eq!(
            format_mcp_log_message("fs", Some(&json!({ "level": "", "data": "x" })), 0),
            "1970-01-01T00:00:00.000Z [fs]  x\n"
        );
        // Empty or non-string logger: no prefix.
        assert_eq!(
            format_mcp_log_message("fs", Some(&json!({ "logger": "", "data": "x" })), 0),
            "1970-01-01T00:00:00.000Z [fs] info x\n"
        );
        assert_eq!(
            format_mcp_log_message("fs", Some(&json!({ "logger": 7, "data": "x" })), 0),
            "1970-01-01T00:00:00.000Z [fs] info x\n"
        );
    }

    #[test]
    fn format_multiline_continuation_indent() {
        assert_eq!(
            format_mcp_log_message("fs", Some(&json!({ "data": "a\nb\r\nc\rd" })), 0),
            "1970-01-01T00:00:00.000Z [fs] info a\n    b\n    c\rd\n"
        );
    }

    #[test]
    fn format_iso_timestamps() {
        assert_eq!(
            format_mcp_log_message("fs", Some(&json!({ "data": "x" })), -1),
            "1969-12-31T23:59:59.999Z [fs] info x\n"
        );
        assert_eq!(
            format_mcp_log_message("fs", Some(&json!({ "data": "x" })), 1767225623123),
            "2026-01-01T00:00:23.123Z [fs] info x\n"
        );
    }

    #[test]
    fn write_appends_lines_and_creates_directories() {
        let fs = MemoryFs::default();
        let mut log = McpServerLog::new("<base>/agent/mcp.log");
        log.write(&fs, "fs", Some(&json!({ "data": "one" })), 1000);
        log.write(&fs, "fs", Some(&json!({ "data": "two" })), 2000);
        assert_eq!(
            fs.content("<base>/agent/mcp.log"),
            "1970-01-01T00:00:01.000Z [fs] info one\n1970-01-01T00:00:02.000Z [fs] info two\n"
        );
    }

    #[test]
    fn write_errors_are_ignored() {
        let fs = MemoryFs::default();
        fs.seed("<base>/afile", "content");
        let mut log = McpServerLog::new("<base>/afile/mcp.log");
        log.write(&fs, "fs", Some(&json!({ "data": "x" })), 0);
        assert!(fs.files.borrow().get("<base>/afile/mcp.log").is_none());
        assert_eq!(fs.content("<base>/afile"), "content");
    }

    #[test]
    fn rotation_at_threshold() {
        let fs = MemoryFs::default();
        let seed = "x".repeat(MAX_LOG_BYTES as usize + 1);
        fs.seed("<base>/mcp.log", &seed);
        let mut log = McpServerLog::new("<base>/mcp.log");
        log.write(&fs, "fs", Some(&json!({ "data": "first" })), 0);
        // Over the threshold on the first write: rotate, then append fresh.
        assert_eq!(fs.content("<base>/mcp.log"), "1970-01-01T00:00:00.000Z [fs] info first\n");
        assert_eq!(fs.content("<base>/mcp.log.1"), seed);
        log.write(&fs, "fs", Some(&json!({ "data": "second" })), 0);
        assert_eq!(
            fs.content("<base>/mcp.log"),
            "1970-01-01T00:00:00.000Z [fs] info first\n1970-01-01T00:00:00.000Z [fs] info second\n"
        );
    }

    #[test]
    fn rotation_skipped_when_another_process_rotated() {
        let fs = MemoryFs::default();
        let seed = "x".repeat(MAX_LOG_BYTES as usize - 10);
        fs.seed("<base>/mcp.log", &seed);
        let mut log = McpServerLog::new("<base>/mcp.log");
        log.write(&fs, "fs", Some(&json!({ "data": "0123456789abc" })), 0);
        // Cached size is now past MAX_LOG_BYTES, but the file was rotated
        // externally: the guard must skip renaming and reset the size.
        let rotated = seed.clone() + "1970-01-01T00:00:00.000Z [fs] info 0123456789abc\n";
        fs.rename("<base>/mcp.log", "<base>/mcp.log.1").unwrap();
        log.write(&fs, "fs", Some(&json!({ "data": "after" })), 0);
        assert_eq!(
            fs.content("<base>/mcp.log"),
            "1970-01-01T00:00:00.000Z [fs] info after\n"
        );
        assert_eq!(fs.content("<base>/mcp.log.1"), rotated);
    }
}
