//! Port of `packages/coding-agent/src/core/settings-manager.ts`.
//!
//! `Settings` is modeled as a raw `serde_json::Map<String, Value>` (with
//! `preserve_order`) because settings files are NOT validated: malformed values
//! replace instead of throwing, and getters apply the TS fallback logic
//! (`?? default`, `typeof` checks, clamps) against the raw `Value`.
//!
//! Known divergences from TS (documented where behavior is unobservable in the
//! TS unit tests):
//! - `create()` takes `agent_dir` as a required argument (the TS default reads
//!   `getAgentDir()` from env/home; ambient reads are forbidden here).
//! - Ambient `process.env` reads (`VISUAL`/`EDITOR`, `PI_CLEAR_ON_SHRINK`,
//!   `PI_HARDWARE_CURSOR`) and `randomUUID()` are injected via
//!   [`SettingsManagerOptions`]; `process.platform` is injected as `platform`.
//! - Getters whose TS counterpart passes malformed raw values through
//!   (`x ?? default` with a non-boolean/non-number value) coerce to the default
//!   instead; serde_json values cannot be `undefined` and Rust return types are
//!   concrete.
//! - The async TS `writeQueue` is a synchronous FIFO drained only by
//!   `flush()`/`reload()`.
//! - Non-finite JSON numbers (NaN/Infinity) cannot exist in a `serde_json`
//!   `Value`; the TS runtime-only paths are exercised through the shared f64
//!   validator in tests.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::paths::{normalize_path, resolve_path, PathInputOptions};
use crate::settings_defaults::settings_defaults;
use crate::text::strip_bom;

/// `CONFIG_DIR_NAME` from `config.ts` (`pkg.piConfig?.configDir || ".pi"`).
pub const CONFIG_DIR_NAME: &str = ".pi";

/// `DEFAULT_HTTP_IDLE_TIMEOUT_MS` from `http-dispatcher.ts` (300_000).
pub const DEFAULT_HTTP_IDLE_TIMEOUT_MS: f64 = 300_000.0;

/// `CACHE_WARMING_MODES` — cache-warming profiles ("idle" also warms between runs).
pub const CACHE_WARMING_MODES: [&str; 3] = ["off", "streaming", "idle"];

/// Tools enabled at startup when `defaultTools` does not change them.
pub const DEFAULT_TOOL_NAMES: [&str; 4] = ["read", "bash", "edit", "write"];

const LOCK_MAX_ATTEMPTS: u32 = 10;
const LOCK_DELAY_MS: u64 = 20;
/// proper-lockfile default `stale` threshold (10s).
const LOCK_STALE_MS: u128 = 10_000;

// ============================================================================
// JS helpers (private; duplicated from mcp_tools.rs because these are private
// there — same situation as mcp_tools duplicating pi-cli::migrations)
// ============================================================================

/// JS `Number.prototype.toString()` for a finite f64.
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

/// JS `String(x)` for a `Value` (error-message formatting). Strings as-is,
/// numbers JS-formatted, objects → "[object Object]", arrays → comma-joined,
/// null → "null". `undefined` ("undefined") is unreachable from JSON.
fn js_string_value(v: &Value) -> String {
	match v {
		Value::Null => "null".to_string(),
		Value::Bool(b) => b.to_string(),
		Value::Number(n) => {
			if let Some(i) = n.as_i64() {
				i.to_string()
			} else if let Some(u) = n.as_u64() {
				u.to_string()
			} else {
				js_f64_to_string(n.as_f64().unwrap_or(0.0))
			}
		}
		Value::String(s) => s.clone(),
		Value::Array(items) => items
			.iter()
			.map(|x| match x {
				Value::Null => String::new(),
				Value::Object(_) => "[object Object]".to_string(),
				other => js_string_value(other),
			})
			.collect::<Vec<_>>()
			.join(","),
		Value::Object(_) => "[object Object]".to_string(),
	}
}

/// `JSON.stringify(value, null, 2)` for a parsed-JSON value — byte-identical
/// for objects, arrays, strings, booleans, null and JS numbers.
fn js_stringify_pretty(value: &Value) -> String {
	fn go(value: &Value, depth: usize) -> String {
		let indent = "  ".repeat(depth);
		let inner = "  ".repeat(depth + 1);
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
					s.push_str(&go(v, depth + 1));
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
					s.push_str(&go(v, depth + 1));
					if i + 1 < n {
						s.push(',');
					}
					s.push('\n');
				}
				s.push_str(&indent);
				s.push(']');
				s
			}
			Value::Number(num) => {
				if let Some(i) = num.as_i64() {
					i.to_string()
				} else if let Some(u) = num.as_u64() {
					u.to_string()
				} else {
					js_f64_to_string(num.as_f64().unwrap_or(0.0))
				}
			}
			other => serde_json::to_string(other).unwrap_or_default(),
		}
	}
	go(value, 0)
}

// ============================================================================
// Node-compatible JSON.parse (load path only)
// ============================================================================

/// `JSON.parse` with Node 24 (V8) error messages. Public so other modules
/// with collected `${path}: ${message}` errors (e.g. pi-session's
/// mcp_config) can reuse the exact message strings. Coverage of the observed
/// message shapes (probed under Node 24):
/// - "Unexpected end of JSON input" (truncated values, `[`/`,` at value
///   position, keyword literals cut short, whitespace-only input)
/// - `Unexpected token 'X', "<input>" is not valid JSON` (bad value token,
///   keyword-literal mismatch)
/// - "Expected property name or '}' in JSON at position N (line L column C)"
/// - "Expected double-quoted property name in JSON at position N ..."
/// - "Expected ':' after property name in JSON at position N ..."
/// - "Expected ',' or '}' after property value in JSON at position N ..."
/// - "Expected ',' or ']' after array element in JSON at position N ..."
/// - "Unexpected non-whitespace character after JSON at position N ..."
/// - "Unterminated string" / "Bad control character in string literal" /
///   "Bad escaped character" / "Bad Unicode escape"
/// - "No number after minus sign" / "Unexpected number" (leading zero) /
///   "Unterminated fractional number" / "Exponent part is missing a number"
///
/// Not covered: `"NaN" is not valid JSON` special form (only for the whole
/// input), lone-surrogate escapes (Rust strings cannot hold them; U+FFFD is
/// substituted), and position units beyond ASCII (V8 counts UTF-16 units,
/// mirrored here).
pub fn parse_json_like(input: &str) -> Result<Value, String> {
	struct Parser<'a> {
		chars: Vec<char>,
		/// UTF-16 position of `chars[i]`.
		u: usize,
		i: usize,
		len: usize,
		input: &'a str,
	}

	impl<'a> Parser<'a> {
		fn new(input: &'a str) -> Self {
			Parser {
				chars: input.chars().collect(),
				u: 0,
				i: 0,
				len: input.chars().map(|c| c.len_utf16()).sum(),
				input,
			}
		}
		fn eof(&self) -> bool {
			self.i >= self.chars.len()
		}
		fn peek(&self) -> char {
			self.chars[self.i]
		}
		fn bump(&mut self) -> char {
			let c = self.chars[self.i];
			self.i += 1;
			self.u += c.len_utf16();
			c
		}
		fn skip_ws(&mut self) {
			while !self.eof() && matches!(self.peek(), ' ' | '\t' | '\n' | '\r') {
				self.bump();
			}
		}
		/// Position used in "Expected ..." messages: the UTF-16 index of the
		/// offending character, or the input length at EOF.
		fn pos(&self) -> usize {
			if self.eof() {
				self.len
			} else {
				self.u
			}
		}
		fn err_at(&self, pos: usize, msg: &str) -> String {
			let mut line = 1usize;
			let mut col = 1usize;
			let mut u = 0usize;
			for c in self.input.chars() {
				if u >= pos {
					break;
				}
				if c == '\n' {
					line += 1;
					col = 1;
				} else {
					col += 1;
				}
				u += c.len_utf16();
			}
			format!("{} in JSON at position {} (line {} column {})", msg, pos, line, col)
		}
		fn err_at_bare(&self, pos: usize, msg: &str) -> String {
			let mut line = 1usize;
			let mut col = 1usize;
			let mut u = 0usize;
			for c in self.input.chars() {
				if u >= pos {
					break;
				}
				if c == '\n' {
					line += 1;
					col = 1;
				} else {
					col += 1;
				}
				u += c.len_utf16();
			}
			format!("{} at position {} (line {} column {})", msg, pos, line, col)
		}
		fn unexpected_token(&self, c: char) -> String {
			// V8 embeds the (possibly truncated) input RAW between double
			// quotes — no escaping — and truncates inputs longer than 20
			// UTF-16 code units to the first 10 code units followed by
			// `"...` (probed under Node 24). Lone surrogates at the cut
			// become U+FFFD (Rust strings cannot hold them; documented gap).
			let units: Vec<u16> = self.input.encode_utf16().collect();
			if units.len() > 20 {
				let snippet = String::from_utf16_lossy(&units[..10]);
				format!("Unexpected token '{}', \"{}\"... is not valid JSON", c, snippet)
			} else {
				format!("Unexpected token '{}', \"{}\" is not valid JSON", c, self.input)
			}
		}

		fn parse(&mut self) -> Result<Value, String> {
			let v = self.parse_value()?;
			self.skip_ws();
			if !self.eof() {
				let pos = self.pos();
				return Err(self.err_at_bare(pos, "Unexpected non-whitespace character after JSON"));
			}
			Ok(v)
		}

		fn parse_value(&mut self) -> Result<Value, String> {
			self.skip_ws();
			if self.eof() {
				return Err("Unexpected end of JSON input".to_string());
			}
			match self.peek() {
				'{' => self.parse_object(),
				'[' => self.parse_array(),
				'"' => {
					self.bump();
					Ok(Value::String(self.parse_string_body()?))
				}
				't' => self.parse_literal("true", Value::Bool(true)),
				'f' => self.parse_literal("false", Value::Bool(false)),
				'n' => self.parse_literal("null", Value::Null),
				'-' | '0'..='9' => self.parse_number(),
				c => Err(self.unexpected_token(c)),
			}
		}

		fn parse_literal(&mut self, word: &str, v: Value) -> Result<Value, String> {
			for expected in word.chars() {
				if self.eof() {
					return Err("Unexpected end of JSON input".to_string());
				}
				let c = self.bump();
				if c != expected {
					return Err(self.unexpected_token(c));
				}
			}
			Ok(v)
		}

		/// Parses the string body after the opening quote was consumed.
		fn parse_string_body(&mut self) -> Result<String, String> {
			let mut out = String::new();
			loop {
				if self.eof() {
					return Err(self.err_at(self.len, "Unterminated string"));
				}
				let pos = self.pos();
				let c = self.bump();
				match c {
					'"' => return Ok(out),
					'\\' => {
						if self.eof() {
							return Err(self.err_at(self.len, "Unterminated string"));
						}
						let epos = self.pos();
						let e = self.bump();
						match e {
							'"' => out.push('"'),
							'\\' => out.push('\\'),
							'/' => out.push('/'),
							'b' => out.push('\u{8}'),
							'f' => out.push('\u{c}'),
							'n' => out.push('\n'),
							'r' => out.push('\r'),
							't' => out.push('\t'),
							'u' => {
								let mut code: u32 = 0;
								for _ in 0..4 {
									if self.eof() {
										return Err(self.err_at(self.len, "Unterminated string"));
									}
									let hpos = self.pos();
									let h = self.bump();
									match h.to_digit(16) {
										Some(d) => code = code * 16 + d,
										None => {
											return Err(self.err_at(hpos, "Bad Unicode escape"));
										}
									}
								}
								// Lone surrogates cannot be represented in a
								// Rust string; substitute U+FFFD (TS keeps them).
								out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
							}
							_ => return Err(self.err_at(epos, "Bad escaped character")),
						}
					}
					c if (c as u32) < 0x20 => {
						return Err(self.err_at(pos, "Bad control character in string literal"));
					}
					c => out.push(c),
				}
			}
		}

		fn parse_number(&mut self) -> Result<Value, String> {
			let mut text = String::new();
			if self.peek() == '-' {
				text.push(self.bump());
				if self.eof() || !self.peek().is_ascii_digit() {
					let pos = self.pos();
					return Err(self.err_at(pos, "No number after minus sign"));
				}
			}
			if self.peek() == '0' {
				text.push(self.bump());
				if !self.eof() && self.peek().is_ascii_digit() {
					let pos = self.pos();
					return Err(self.err_at(pos, "Unexpected number"));
				}
			} else {
				while !self.eof() && self.peek().is_ascii_digit() {
					text.push(self.bump());
				}
			}
			if !self.eof() && self.peek() == '.' {
				text.push(self.bump());
				if self.eof() || !self.peek().is_ascii_digit() {
					let pos = self.pos();
					return Err(self.err_at(pos, "Unterminated fractional number"));
				}
				while !self.eof() && self.peek().is_ascii_digit() {
					text.push(self.bump());
				}
			}
			if !self.eof() && (self.peek() == 'e' || self.peek() == 'E') {
				text.push(self.bump());
				if !self.eof() && (self.peek() == '+' || self.peek() == '-') {
					text.push(self.bump());
				}
				if self.eof() || !self.peek().is_ascii_digit() {
					let pos = self.pos();
					return Err(self.err_at(pos, "Exponent part is missing a number"));
				}
				while !self.eof() && self.peek().is_ascii_digit() {
					text.push(self.bump());
				}
			}
			let f: f64 = text.parse().unwrap_or(f64::NAN);
			if f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_992.0 {
				Ok(Value::from(f as i64))
			} else {
				serde_json::Number::from_f64(f).map(Value::Number).ok_or_else(|| "Unexpected end of JSON input".to_string())
			}
		}

		fn parse_object(&mut self) -> Result<Value, String> {
			self.bump(); // '{'
			let mut map = Map::new();
			self.skip_ws();
			if self.eof() {
				return Err(self.err_at(self.len, "Expected property name or '}'"));
			}
			if self.peek() == '}' {
				self.bump();
				return Ok(Value::Object(map));
			}
			let mut first = true;
			loop {
				self.skip_ws();
				if self.eof() || self.peek() != '"' {
					let pos = self.pos();
					let msg = if first {
						"Expected property name or '}'"
					} else {
						"Expected double-quoted property name"
					};
					return Err(self.err_at(pos, msg));
				}
				self.bump();
				let key = self.parse_string_body()?;
				self.skip_ws();
				if self.eof() || self.peek() != ':' {
					let pos = self.pos();
					return Err(self.err_at(pos, "Expected ':' after property name"));
				}
				self.bump();
				let value = self.parse_value()?;
				map.insert(key, value);
				self.skip_ws();
				if self.eof() {
					return Err(self.err_at(self.len, "Expected ',' or '}' after property value"));
				}
				match self.peek() {
					',' => {
						self.bump();
						first = false;
					}
					'}' => {
						self.bump();
						return Ok(Value::Object(map));
					}
					_ => {
						let pos = self.pos();
						return Err(self.err_at(pos, "Expected ',' or '}' after property value"));
					}
				}
			}
		}

		fn parse_array(&mut self) -> Result<Value, String> {
			self.bump(); // '['
			let mut items = Vec::new();
			self.skip_ws();
			if self.eof() {
				return Err("Unexpected end of JSON input".to_string());
			}
			if self.peek() == ']' {
				self.bump();
				return Ok(Value::Array(items));
			}
			loop {
				items.push(self.parse_value()?);
				self.skip_ws();
				if self.eof() {
					return Err(self.err_at(self.len, "Expected ',' or ']' after array element"));
				}
				match self.peek() {
					',' => {
						self.bump();
					}
					']' => {
						self.bump();
						return Ok(Value::Array(items));
					}
					_ => {
						let pos = self.pos();
						return Err(self.err_at(pos, "Expected ',' or ']' after array element"));
					}
				}
			}
		}
	}

	Parser::new(input).parse()
}

// ============================================================================
// Pure helpers (top of the TS file)
// ============================================================================

/// Whether a tool selection entry is a `+name` or `-name` modifier.
pub fn is_tool_modifier(entry: &Value) -> bool {
	entry
		.as_str()
		.is_some_and(|s| s.starts_with('+') || s.starts_with('-'))
}

/// Validate a tool list from `--tools` or the SDK `tools` option: either an
/// allowlist of plain names/patterns, or only `+name`/`-name` entries with
/// exact names. Returns the problem, or `None` when the list is valid.
pub fn get_tool_list_error(entries: &[String]) -> Option<String> {
	let modifiers: Vec<&String> = entries.iter().filter(|e| is_tool_modifier(&Value::String((*e).clone()))).collect();
	if modifiers.is_empty() {
		return None;
	}
	if modifiers.len() < entries.len() {
		return Some("tool names cannot be mixed with +name or -name entries".to_string());
	}
	if let Some(pattern) = modifiers.into_iter().find(|e| e.contains('*')) {
		return Some(format!("+name and -name entries take exact tool names, not patterns: {}", pattern));
	}
	None
}

/// Apply the `+name` and `-name` entries of `entries` to `base` in order.
pub fn apply_tool_modifiers(base: &[String], entries: &[String]) -> Vec<String> {
	let mut tools = base.to_vec();
	for entry in entries {
		if !is_tool_modifier(&Value::String(entry.clone())) {
			continue;
		}
		let name = &entry[1..];
		let index = tools.iter().position(|t| t == name);
		if entry.starts_with('+') && index.is_none() && !name.is_empty() {
			tools.push(name.to_string());
		} else if entry.starts_with('-') {
			if let Some(i) = index {
				tools.remove(i);
			}
		}
	}
	tools
}

/// Merge `defaultTools` of two settings layers: a plain list replaces the
/// inherited one; a list of only `+name`/`-name` entries is appended.
fn merge_default_tools(base: Option<&Value>, overrides: Option<&Value>) -> Option<Value> {
	let overrides = overrides?;
	let base_arr = base.and_then(Value::as_array);
	let ov_arr = overrides.as_array();
	match (base_arr, ov_arr) {
		(Some(b), Some(o)) if o.iter().all(is_tool_modifier) => {
			let mut merged = b.clone();
			merged.extend(o.iter().cloned());
			Some(Value::Array(merged))
		}
		_ => Some(overrides.clone()),
	}
}

/// Resolve a merged `defaultTools` list: plain names replace
/// `DEFAULT_TOOL_NAMES`, then `+name`/`-name` modify the selection in order.
fn resolve_default_tools(entries: &[Value]) -> Vec<String> {
	let plain: Vec<String> = entries
		.iter()
		.filter(|e| !is_tool_modifier(e))
		.filter_map(|e| e.as_str())
		.map(str::to_string)
		.collect();
	let base = if !plain.is_empty() || entries.is_empty() {
		plain
	} else {
		DEFAULT_TOOL_NAMES.iter().map(|s| s.to_string()).collect()
	};
	let entry_strings: Vec<String> = entries.iter().filter_map(|e| e.as_str()).map(str::to_string).collect();
	apply_tool_modifiers(&base, &entry_strings)
}

fn is_mergeable(value: &Value) -> bool {
	value.is_object()
}

/// `{...base}` spread then iterate override keys in order; recursive merge for
/// plain objects on both sides, otherwise the override wins (arrays replace).
/// Key order: base keys first, then new override keys appended.
fn deep_merge_objects(base: &Map<String, Value>, overrides: &Map<String, Value>) -> Map<String, Value> {
	let mut result = base.clone();
	for (key, override_value) in overrides {
		// TS skips `undefined` override values; JSON values are never undefined.
		let merged = match base.get(key) {
			Some(base_value)
				if is_mergeable(base_value) && is_mergeable(override_value) =>
			{
				Value::Object(deep_merge_objects(
					base_value.as_object().unwrap(),
					override_value.as_object().unwrap(),
				))
			}
			_ => override_value.clone(),
		};
		result.insert(key.clone(), merged);
	}
	result
}

/// Deep merge settings: project/overrides take precedence, nested objects merge
/// recursively, `defaultTools` merges via [`merge_default_tools`].
fn deep_merge_settings(base: &Map<String, Value>, overrides: &Map<String, Value>) -> Map<String, Value> {
	let mut merged = deep_merge_objects(base, overrides);
	if let Some(tools) = merge_default_tools(base.get("defaultTools"), overrides.get("defaultTools")) {
		merged.insert("defaultTools".to_string(), tools);
	}
	merged
}

/// `Number(trimmed)`-style JS number coercion (hex, decimal, exponent,
/// Infinity); `None` for NaN.
fn js_parse_number(s: &str) -> Option<f64> {
	let t = s.trim();
	if t.is_empty() {
		return Some(0.0);
	}
	if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
		return u64::from_str_radix(hex, 16).ok().map(|v| v as f64);
	}
	match t {
		"Infinity" | "+Infinity" => return Some(f64::INFINITY),
		"-Infinity" => return Some(f64::NEG_INFINITY),
		_ => {}
	}
	// Rust's f64 parser accepts forms JS rejects ("inf", "nan").
	if !t.chars().all(|c| c.is_ascii_digit() || matches!(c, 'e' | 'E' | '+' | '-' | '.')) {
		return None;
	}
	t.parse::<f64>().ok()
}

/// Port of `parseHttpIdleTimeoutMs` from `http-dispatcher.ts` (lines 19–33):
/// `"disabled"` → 0, empty/blank → None, non-finite/negative/NaN → None,
/// otherwise `Math.floor(value)`.
fn parse_http_idle_timeout_ms(value: &Value) -> Option<f64> {
	if let Value::String(s) = value {
		let trimmed = s.trim();
		if trimmed.eq_ignore_ascii_case("disabled") {
			return Some(0.0);
		}
		if trimmed.is_empty() {
			return None;
		}
		let n = js_parse_number(trimmed)?;
		return parse_http_idle_timeout_ms(&Value::Number(serde_json::Number::from_f64(n)?));
	}
	match value.as_f64() {
		Some(n) if n.is_finite() && n >= 0.0 => Some(n.floor()),
		_ => None,
	}
}

fn parse_timeout_setting(value: Option<&Value>, setting_name: &str) -> Result<Option<f64>, String> {
	match value {
		None => Ok(None),
		Some(v) => match parse_http_idle_timeout_ms(v) {
			Some(ms) => Ok(Some(ms)),
			None => Err(format!("Invalid {} setting: {}", setting_name, js_string_value(v))),
		},
	}
}

fn is_safe_nonneg_integer(v: &Value) -> bool {
	match v.as_f64() {
		Some(f) => f.is_finite() && f.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(&f),
		None => false,
	}
}

/// Shared validator for compaction token settings; also used directly with
/// non-finite f64s in tests (serde_json values cannot hold NaN/Infinity).
#[cfg(test)]
fn validate_token_f64(f: f64) -> bool {
	f.is_finite() && f.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(&f)
}

// ============================================================================
// Storage abstraction
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsScope {
	Global,
	Project,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingsError {
	pub scope: SettingsScope,
	pub path: Option<String>,
	pub message: String,
}

fn to_settings_error(scope: SettingsScope, message: &str, path: Option<&str>) -> SettingsError {
	SettingsError {
		scope,
		// TS spreads `path` only when truthy (empty string omitted).
		path: path.filter(|p| !p.is_empty()).map(str::to_string),
		message: message.to_string(),
	}
}

pub trait SettingsStorage {
	/// TS `withLock` can throw (fs/lock errors propagate to the caller); this
	/// port returns the error message instead.
	fn with_lock(
		&mut self,
		scope: SettingsScope,
		f: &mut dyn FnMut(Option<&str>) -> Option<String>,
	) -> Result<(), String>;
}

/// `DEFAULT_COMPACTION_TOKEN_SETTINGS` (from `SETTINGS_DEFAULTS.compaction`).
fn default_compaction_token_setting(field: &str) -> f64 {
	settings_defaults()
		.get("compaction")
		.and_then(|c| c.get(field))
		.and_then(Value::as_f64)
		.unwrap_or(0.0)
}

/// Random v4 UUID for `randomUUID()` (no new deps): entropy from per-call
/// `RandomState` hashers, version 4 and RFC 4122 variant bits set.
fn random_uuid_v4() -> String {
	use std::collections::hash_map::RandomState;
	use std::hash::{BuildHasher, Hasher};
	let mut bytes = [0u8; 16];
	for (i, chunk) in bytes.chunks_mut(8).enumerate() {
		let mut h = RandomState::new().build_hasher();
		h.write_usize(i.wrapping_mul(0x9E37_79B9_7F4A_7C15));
		let v = h.finish().to_le_bytes();
		chunk.copy_from_slice(&v[..chunk.len()]);
	}
	bytes[6] = (bytes[6] & 0x0f) | 0x40;
	bytes[8] = (bytes[8] & 0x3f) | 0x80;
	let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
	format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// In-process lock guard: removes the sibling lockfile directory on drop
/// (proper-lockfile creates/removes a directory at `<path>.lock`).
struct LockGuard {
	path: PathBuf,
}

impl Drop for LockGuard {
	fn drop(&mut self) {
		let _ = std::fs::remove_dir(&self.path);
	}
}

pub struct FileSettingsStorage {
	global_settings_path: PathBuf,
	project_settings_path: PathBuf,
}

impl FileSettingsStorage {
	pub fn new(cwd: &str, agent_dir: &str) -> Self {
		// TS resolvePath(x) resolves against process.cwd(); callers here pass
		// absolute paths, so resolving against "." is equivalent.
		let resolved_cwd = resolve_path(cwd, ".", &PathInputOptions::default());
		let resolved_agent_dir = resolve_path(agent_dir, ".", &PathInputOptions::default());
		FileSettingsStorage {
			global_settings_path: Path::new(&resolved_agent_dir).join("settings.json"),
			project_settings_path: Path::new(&resolved_cwd).join(CONFIG_DIR_NAME).join("settings.json"),
		}
	}

	fn acquire_lock_sync_with_retry(&self, path: &Path) -> Result<LockGuard, String> {
		let mut lock_path = path.as_os_str().to_owned();
		lock_path.push(".lock");
		let lock_path = PathBuf::from(lock_path);
		let mut last_error = String::new();
		for attempt in 1..=LOCK_MAX_ATTEMPTS {
			match std::fs::create_dir(&lock_path) {
				Ok(()) => return Ok(LockGuard { path: lock_path }),
				Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
					// proper-lockfile removes locks older than `stale` (10s).
					let stale = std::fs::metadata(&lock_path)
						.and_then(|m| m.modified())
						.ok()
						.and_then(|t| t.elapsed().ok())
						.is_some_and(|d| d.as_millis() > LOCK_STALE_MS);
					if stale {
						let _ = std::fs::remove_dir(&lock_path);
						last_error = "Lock file is already being held".to_string();
						continue;
					}
					if attempt == LOCK_MAX_ATTEMPTS {
						return Err("Lock file is already being held".to_string());
					}
					last_error = "Lock file is already being held".to_string();
					std::thread::sleep(Duration::from_millis(LOCK_DELAY_MS));
				}
				Err(e) => return Err(e.to_string()),
			}
		}
		Err(last_error)
	}
}

impl SettingsStorage for FileSettingsStorage {
	fn with_lock(
		&mut self,
		scope: SettingsScope,
		f: &mut dyn FnMut(Option<&str>) -> Option<String>,
	) -> Result<(), String> {
		let path = match scope {
			SettingsScope::Global => &self.global_settings_path,
			SettingsScope::Project => &self.project_settings_path,
		};
		let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();

		let mut release: Option<LockGuard> = None;
		let outcome: Result<(), String> = (|| {
			let file_exists = path.exists();
			if file_exists {
				release = Some(self.acquire_lock_sync_with_retry(path)?);
			}
			let current = if file_exists {
				Some(std::fs::read_to_string(path).map_err(|e| e.to_string())?)
			} else {
				None
			};
			let next = f(current.as_deref());
			if let Some(next) = next {
				if !dir.exists() {
					std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
				}
				if release.is_none() {
					release = Some(self.acquire_lock_sync_with_retry(path)?);
				}
				std::fs::write(path, next).map_err(|e| e.to_string())?;
			}
			Ok(())
		})();
		drop(release); // `finally`
		outcome
	}
}

#[derive(Default)]
pub struct InMemorySettingsStorage {
	global: Option<String>,
	project: Option<String>,
}

impl InMemorySettingsStorage {
	pub fn new() -> Self {
		Self::default()
	}
}

impl SettingsStorage for InMemorySettingsStorage {
	fn with_lock(
		&mut self,
		scope: SettingsScope,
		f: &mut dyn FnMut(Option<&str>) -> Option<String>,
	) -> Result<(), String> {
		let current = match scope {
			SettingsScope::Global => self.global.clone(),
			SettingsScope::Project => self.project.clone(),
		};
		let next = f(current.as_deref());
		if let Some(next) = next {
			match scope {
				SettingsScope::Global => self.global = Some(next),
				SettingsScope::Project => self.project = Some(next),
			}
		}
		Ok(())
	}
}

// ============================================================================
// SettingsManager
// ============================================================================

pub type UuidGen = Arc<dyn Fn() -> String + Send + Sync>;

#[derive(Clone, Default)]
pub struct SettingsManagerOptions {
	pub project_trusted: Option<bool>,
	/// Injected `process.env` (VISUAL/EDITOR, PI_CLEAR_ON_SHRINK,
	/// PI_HARDWARE_CURSOR; HOME backs `~` expansion).
	pub env: HashMap<String, String>,
	/// Overrides the env-map HOME for `~` expansion.
	pub home_dir: Option<String>,
	/// `process.platform` ("win32" chooses "notepad"); defaults to "linux".
	pub platform: Option<String>,
	/// `randomUUID()`; defaults to a real random v4 generator.
	pub uuid_gen: Option<UuidGen>,
}

/// `Pick<Model, "provider" | "id">` for compaction model overrides.
#[derive(Debug, Clone, Copy)]
pub struct ModelRef<'a> {
	pub provider: &'a str,
	pub id: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompactionSettings {
	pub enabled: bool,
	pub reserve_tokens: f64,
	pub keep_recent_tokens: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrySettings {
	pub enabled: bool,
	pub max_retries: f64,
	pub base_delay_ms: f64,
	pub max_agent_delay_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderRetrySettings {
	pub timeout_ms: Option<f64>,
	pub max_retries: Option<f64>,
	pub max_retry_delay_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BranchSummarySettings {
	pub reserve_tokens: f64,
	pub skip_prompt: bool,
}

/// `WheelScrollLines` = number of lines or "auto".
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelScrollLines {
	Auto,
	Lines(f64),
}

#[derive(Default)]
struct SettingsPaths {
	global: Option<String>,
	project: Option<String>,
}

struct PendingWrite {
	scope: SettingsScope,
	snapshot: Map<String, Value>,
	modified_fields: Vec<String>,
	modified_nested: Vec<(String, Vec<String>)>,
}

pub struct SettingsManager {
	storage: Box<dyn SettingsStorage>,
	global_settings: Map<String, Value>,
	project_settings: Map<String, Value>,
	settings: Map<String, Value>,
	project_trusted: bool,
	modified_fields: Vec<String>,
	modified_nested_fields: Vec<(String, Vec<String>)>,
	modified_project_fields: Vec<String>,
	modified_project_nested_fields: Vec<(String, Vec<String>)>,
	global_settings_load_error: Option<String>,
	project_settings_load_error: Option<String>,
	pending_writes: VecDeque<PendingWrite>,
	errors: Vec<SettingsError>,
	settings_paths: SettingsPaths,
	env: HashMap<String, String>,
	home_dir: Option<String>,
	platform: String,
	uuid_gen: UuidGen,
}

/// Migrate old settings formats in place. `Map::remove` is `swap_remove` on
/// preserve_order maps, so `shift_remove` is used at every order-observable
/// deletion.
fn migrate_settings(settings: &mut Map<String, Value>) {
	// queueMode -> steeringMode
	if settings.contains_key("queueMode") && !settings.contains_key("steeringMode") {
		let v = settings.get("queueMode").cloned().unwrap_or(Value::Null);
		settings.insert("steeringMode".to_string(), v);
		settings.shift_remove("queueMode");
	}

	// legacy websockets boolean -> transport enum
	if !settings.contains_key("transport")
		&& settings.get("websockets").is_some_and(Value::is_boolean)
	{
		let websocket = settings.get("websockets") == Some(&Value::Bool(true));
		settings.insert(
			"transport".to_string(),
			Value::String(if websocket { "websocket" } else { "sse" }.to_string()),
		);
		settings.shift_remove("websockets");
	}

	// old skills object format -> array format
	if let Some(Value::Object(skills_settings)) = settings.get("skills").cloned() {
		if skills_settings.contains_key("enableSkillCommands") && !settings.contains_key("enableSkillCommands") {
			settings.insert(
				"enableSkillCommands".to_string(),
				skills_settings.get("enableSkillCommands").cloned().unwrap_or(Value::Null),
			);
		}
		let custom = skills_settings.get("customDirectories").cloned();
		if matches!(&custom, Some(Value::Array(a)) if !a.is_empty()) {
			// Assignment to an existing key keeps its position.
			settings.insert("skills".to_string(), custom.unwrap());
		} else {
			settings.shift_remove("skills");
		}
	}

	// retry.maxDelayMs -> retry.provider.maxRetryDelayMs
	if let Some(Value::Object(retry_value)) = settings.get("retry").cloned() {
		let provider_settings = match retry_value.get("provider") {
			Some(Value::Object(o)) => Some(o.clone()),
			_ => None,
		};
		let provider_max_missing = provider_settings
			.as_ref()
			.and_then(|p| p.get("maxRetryDelayMs"))
			.is_none_or(Value::is_null);
		let max_delay = retry_value.get("maxDelayMs").cloned();
		let mut retry_value = retry_value;
		if matches!(&max_delay, Some(v) if v.is_number()) && provider_max_missing {
			let mut new_provider = provider_settings.clone().unwrap_or_default();
			new_provider.insert("maxRetryDelayMs".to_string(), max_delay.clone().unwrap());
			retry_value.insert("provider".to_string(), Value::Object(new_provider));
		}
		retry_value.shift_remove("maxDelayMs");
		settings.insert("retry".to_string(), Value::Object(retry_value));
	}
}

fn value_to_settings_map(value: Value) -> Result<Map<String, Value>, String> {
	if value.is_object() {
		Ok(value.as_object().cloned().unwrap_or_default())
	} else if value.is_array() {
		// TS keeps the array as `Settings`; every observable use treats it as
		// empty (spreading an array yields no keys).
		Ok(Map::new())
	} else {
		Err(format!(
			"Cannot use 'in' operator to search for 'queueMode' in {}",
			js_string_value(&value)
		))
	}
}

impl SettingsManager {
	#[allow(clippy::too_many_arguments)]
	fn new(
		storage: Box<dyn SettingsStorage>,
		initial_global: Map<String, Value>,
		initial_project: Map<String, Value>,
		global_load_error: Option<String>,
		project_load_error: Option<String>,
		initial_errors: Vec<SettingsError>,
		project_trusted: bool,
		settings_paths: SettingsPaths,
		options: &SettingsManagerOptions,
	) -> Self {
		let settings = deep_merge_settings(&initial_global, &initial_project);
		SettingsManager {
			storage,
			global_settings: initial_global,
			project_settings: initial_project,
			settings,
			project_trusted,
			modified_fields: Vec::new(),
			modified_nested_fields: Vec::new(),
			modified_project_fields: Vec::new(),
			modified_project_nested_fields: Vec::new(),
			global_settings_load_error: global_load_error,
			project_settings_load_error: project_load_error,
			pending_writes: VecDeque::new(),
			errors: initial_errors,
			settings_paths,
			env: options.env.clone(),
			home_dir: options
				.home_dir
				.clone()
				.or_else(|| options.env.get("HOME").cloned()),
			platform: options.platform.clone().unwrap_or_else(|| "linux".to_string()),
			uuid_gen: options.uuid_gen.clone().unwrap_or_else(|| Arc::new(random_uuid_v4)),
		}
	}

	/// Create a SettingsManager that loads from files. The TS default
	/// `agentDir = getAgentDir()` (env/home read) is intentionally dropped.
	pub fn create(cwd: &str, agent_dir: &str, options: SettingsManagerOptions) -> SettingsManager {
		let resolved_cwd = resolve_path(cwd, ".", &PathInputOptions::default());
		let resolved_agent_dir = resolve_path(agent_dir, ".", &PathInputOptions::default());
		let storage = FileSettingsStorage::new(cwd, agent_dir);
		SettingsManager::from_storage_with_paths(
			Box::new(storage),
			options,
			SettingsPaths {
				global: Some(Path::new(&resolved_agent_dir).join("settings.json").to_string_lossy().into_owned()),
				project: Some(
					Path::new(&resolved_cwd)
						.join(CONFIG_DIR_NAME)
						.join("settings.json")
						.to_string_lossy()
						.into_owned(),
				),
			},
		)
	}

	/// Create a SettingsManager from an arbitrary storage backend.
	pub fn from_storage(storage: Box<dyn SettingsStorage>, options: SettingsManagerOptions) -> SettingsManager {
		SettingsManager::from_storage_with_paths(storage, options, SettingsPaths::default())
	}

	fn from_storage_with_paths(
		mut storage: Box<dyn SettingsStorage>,
		options: SettingsManagerOptions,
		settings_paths: SettingsPaths,
	) -> SettingsManager {
		let project_trusted = options.project_trusted.unwrap_or(true);
		let global_load = SettingsManager::try_load_from_storage(storage.as_mut(), SettingsScope::Global, true);
		let project_load = SettingsManager::try_load_from_storage(storage.as_mut(), SettingsScope::Project, project_trusted);
		let mut initial_errors = Vec::new();
		if let Err(e) = &global_load {
			initial_errors.push(to_settings_error(SettingsScope::Global, e, settings_paths.global.as_deref()));
		}
		if let Err(e) = &project_load {
			initial_errors.push(to_settings_error(SettingsScope::Project, e, settings_paths.project.as_deref()));
		}
		let (global_settings, global_err) = match global_load {
			Ok(m) => (m, None),
			Err(e) => (Map::new(), Some(e)),
		};
		let (project_settings, project_err) = match project_load {
			Ok(m) => (m, None),
			Err(e) => (Map::new(), Some(e)),
		};
		SettingsManager::new(
			storage,
			global_settings,
			project_settings,
			global_err,
			project_err,
			initial_errors,
			project_trusted,
			settings_paths,
			&options,
		)
	}

	/// Create an in-memory SettingsManager (no file I/O).
	pub fn in_memory(settings: &Value, options: SettingsManagerOptions) -> SettingsManager {
		let mut storage = InMemorySettingsStorage::new();
		let mut initial = settings.as_object().cloned().unwrap_or_default();
		migrate_settings(&mut initial);
		let payload = js_stringify_pretty(&Value::Object(initial));
		let _ = storage.with_lock(SettingsScope::Global, &mut |_| Some(payload.clone()));
		SettingsManager::from_storage(Box::new(storage), options)
	}

	fn load_from_storage(
		storage: &mut dyn SettingsStorage,
		scope: SettingsScope,
		project_trusted: bool,
	) -> Result<Map<String, Value>, String> {
		if scope == SettingsScope::Project && !project_trusted {
			return Ok(Map::new());
		}
		let mut content: Option<String> = None;
		storage
			.with_lock(scope, &mut |current| {
				content = current.map(str::to_string);
				None
			})?;
		let content = match content {
			Some(c) if !c.is_empty() => c,
			_ => return Ok(Map::new()),
		};
		let value = parse_json_like(strip_bom(&content))?;
		let mut map = value_to_settings_map(value)?;
		migrate_settings(&mut map);
		Ok(map)
	}

	fn try_load_from_storage(
		storage: &mut dyn SettingsStorage,
		scope: SettingsScope,
		project_trusted: bool,
	) -> Result<Map<String, Value>, String> {
		SettingsManager::load_from_storage(storage, scope, project_trusted)
	}

	// ------------------------------------------------------------------
	// Core state
	// ------------------------------------------------------------------

	/// A copy of the effective settings: global and project merged, with overrides.
	pub fn get_settings(&self) -> Value {
		Value::Object(self.settings.clone())
	}

	pub fn get_global_settings(&self) -> Value {
		Value::Object(self.global_settings.clone())
	}

	pub fn get_project_settings(&self) -> Value {
		Value::Object(self.project_settings.clone())
	}

	pub fn is_project_trusted(&self) -> bool {
		self.project_trusted
	}

	pub fn set_project_trusted(&mut self, trusted: bool) {
		if self.project_trusted == trusted {
			return;
		}
		self.project_trusted = trusted;
		self.modified_project_fields.clear();
		self.modified_project_nested_fields.clear();

		if !trusted {
			self.project_settings = Map::new();
			self.project_settings_load_error = None;
			self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
			return;
		}

		let project_load =
			SettingsManager::try_load_from_storage(self.storage.as_mut(), SettingsScope::Project, trusted);
		match project_load {
			Ok(settings) => {
				self.project_settings = settings;
				self.project_settings_load_error = None;
			}
			Err(e) => {
				self.project_settings_load_error = Some(e.clone());
				self.record_error(SettingsScope::Project, &e);
			}
		}
		self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
	}

	pub fn reload(&mut self) {
		self.flush(); // `await this.writeQueue`

		let global_load = SettingsManager::try_load_from_storage(self.storage.as_mut(), SettingsScope::Global, true);
		match global_load {
			Ok(settings) => {
				self.global_settings = settings;
				self.global_settings_load_error = None;
			}
			Err(e) => {
				self.global_settings_load_error = Some(e.clone());
				self.record_error(SettingsScope::Global, &e);
			}
		}

		self.modified_fields.clear();
		self.modified_nested_fields.clear();
		self.modified_project_fields.clear();
		self.modified_project_nested_fields.clear();

		let project_load =
			SettingsManager::try_load_from_storage(self.storage.as_mut(), SettingsScope::Project, self.project_trusted);
		match project_load {
			Ok(settings) => {
				self.project_settings = settings;
				self.project_settings_load_error = None;
			}
			Err(e) => {
				self.project_settings_load_error = Some(e.clone());
				self.record_error(SettingsScope::Project, &e);
			}
		}

		self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
	}

	/// Apply additional overrides on top of current settings.
	pub fn apply_overrides(&mut self, overrides: &Value) {
		if let Some(overrides) = overrides.as_object() {
			self.settings = deep_merge_settings(&self.settings, overrides);
		}
	}

	fn mark_modified(&mut self, field: &str, nested_key: Option<&str>) {
		if !self.modified_fields.iter().any(|f| f == field) {
			self.modified_fields.push(field.to_string());
		}
		let Some(nested_key) = nested_key else { return };
		if !self.modified_nested_fields.iter().any(|(k, _)| k == field) {
			self.modified_nested_fields.push((field.to_string(), Vec::new()));
		}
		if let Some(entry) = self.modified_nested_fields.iter_mut().find(|(k, _)| k == field) {
			if !entry.1.iter().any(|k| k == nested_key) {
				entry.1.push(nested_key.to_string());
			}
		}
	}

	fn mark_project_modified(&mut self, field: &str, nested_key: Option<&str>) {
		if !self.modified_project_fields.iter().any(|f| f == field) {
			self.modified_project_fields.push(field.to_string());
		}
		let Some(nested_key) = nested_key else { return };
		if !self.modified_project_nested_fields.iter().any(|(k, _)| k == field) {
			self.modified_project_nested_fields.push((field.to_string(), Vec::new()));
		}
		if let Some(entry) = self.modified_project_nested_fields.iter_mut().find(|(k, _)| k == field) {
			if !entry.1.iter().any(|k| k == nested_key) {
				entry.1.push(nested_key.to_string());
			}
		}
	}

	fn assert_project_trusted_for_write(&self) -> Result<(), String> {
		if !self.project_trusted {
			return Err("Project is not trusted; refusing to write project settings".to_string());
		}
		Ok(())
	}

	fn record_error(&mut self, scope: SettingsScope, message: &str) {
		let path = match scope {
			SettingsScope::Global => self.settings_paths.global.as_deref(),
			SettingsScope::Project => self.settings_paths.project.as_deref(),
		};
		self.errors.push(to_settings_error(scope, message, path));
	}

	fn clear_modified_scope(&mut self, scope: SettingsScope) {
		if scope == SettingsScope::Global {
			self.modified_fields.clear();
			self.modified_nested_fields.clear();
		} else {
			self.modified_project_fields.clear();
			self.modified_project_nested_fields.clear();
		}
	}

	fn clone_nested(source: &[(String, Vec<String>)]) -> Vec<(String, Vec<String>)> {
		source.to_vec()
	}

	fn persist_scoped_settings(
		&mut self,
		scope: SettingsScope,
		snapshot: &Map<String, Value>,
		modified_fields: &[String],
		modified_nested: &[(String, Vec<String>)],
	) -> Result<(), String> {
		let mut parse_error: Option<String> = None;
		let result = self.storage.with_lock(scope, &mut |current| {
			if parse_error.is_some() {
				return None;
			}
			let current_file: Map<String, Value> = match current {
				Some(c) => match parse_json_like(strip_bom(c)) {
					Ok(v) => match value_to_settings_map(v) {
						Ok(mut m) => {
							migrate_settings(&mut m);
							m
						}
						Err(msg) => {
							parse_error = Some(msg);
							return None;
						}
					},
					Err(msg) => {
						parse_error = Some(msg);
						return None;
					}
				},
				None => Map::new(),
			};
			let mut merged = current_file.clone();
			for field in modified_fields {
				let value = snapshot.get(field);
				let nested_modified = modified_nested.iter().find(|(k, _)| k == field).map(|(_, v)| v);
				match (nested_modified, value) {
					(Some(nested_keys), Some(v)) if !v.is_null() && (v.is_object() || v.is_array()) => {
						let mut merged_nested = current_file
							.get(field)
							.and_then(|f| f.as_object())
							.cloned()
							.unwrap_or_default();
						for nested_key in nested_keys {
							match v.get(nested_key) {
								Some(x) => {
									merged_nested.insert(nested_key.clone(), x.clone());
								}
								None => {
									merged_nested.shift_remove(nested_key);
								}
							}
						}
						merged.insert(field.clone(), Value::Object(merged_nested));
					}
					(_, value) => match value {
						Some(v) => {
							merged.insert(field.clone(), v.clone());
						}
						// TS assigns `undefined`, which JSON.stringify omits.
						None => {
							merged.shift_remove(field);
						}
					},
				}
			}
			Some(js_stringify_pretty(&Value::Object(merged)))
		});
		result?;
		if let Some(msg) = parse_error {
			return Err(msg);
		}
		Ok(())
	}

	fn save(&mut self) {
		self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);

		if self.global_settings_load_error.is_some() {
			return;
		}

		let snapshot = self.global_settings.clone();
		let modified_fields = self.modified_fields.clone();
		let modified_nested = SettingsManager::clone_nested(&self.modified_nested_fields);
		self.pending_writes.push_back(PendingWrite {
			scope: SettingsScope::Global,
			snapshot,
			modified_fields,
			modified_nested,
		});
	}

	fn save_project_settings(&mut self, settings: Map<String, Value>) -> Result<(), String> {
		self.assert_project_trusted_for_write()?;
		self.project_settings = settings;
		self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);

		if self.project_settings_load_error.is_some() {
			return Ok(());
		}

		let snapshot = self.project_settings.clone();
		let modified_fields = self.modified_project_fields.clone();
		let modified_nested = SettingsManager::clone_nested(&self.modified_project_nested_fields);
		self.pending_writes.push_back(PendingWrite {
			scope: SettingsScope::Project,
			snapshot,
			modified_fields,
			modified_nested,
		});
		Ok(())
	}

	fn update_project_settings(
		&mut self,
		field: &str,
		update: impl FnOnce(&mut Map<String, Value>),
	) -> Result<(), String> {
		self.assert_project_trusted_for_write()?;
		let mut project_settings = self.project_settings.clone();
		update(&mut project_settings);
		self.mark_project_modified(field, None);
		self.save_project_settings(project_settings)
	}

	/// Drain the pending write queue (TS `await this.writeQueue`).
	pub fn flush(&mut self) {
		while let Some(task) = self.pending_writes.pop_front() {
			if task.scope == SettingsScope::Project && self.assert_project_trusted_for_write().is_err() {
				// `.catch` records the error but the chain continues.
				self.record_error(
					SettingsScope::Project,
					"Project is not trusted; refusing to write project settings",
				);
				continue;
			}
			match self.persist_scoped_settings(
				task.scope,
				&task.snapshot,
				&task.modified_fields,
				&task.modified_nested,
			) {
				Ok(()) => self.clear_modified_scope(task.scope),
				Err(msg) => self.record_error(task.scope, &msg),
			}
		}
	}

	pub fn drain_errors(&mut self) -> Vec<SettingsError> {
		std::mem::take(&mut self.errors)
	}

	// ------------------------------------------------------------------
	// Field helpers
	// ------------------------------------------------------------------

	fn field(&self, key: &str) -> Option<&Value> {
		self.settings.get(key)
	}

	fn nested_field(&self, outer: &str, inner: &str) -> Option<&Value> {
		self.settings.get(outer).and_then(|o| o.get(inner))
	}

	fn default_of(&self, path: &[&str]) -> Value {
		let mut v = settings_defaults();
		for key in path {
			v = v.get(key).cloned().unwrap_or(Value::Null);
		}
		v
	}

	fn default_str(&self, path: &[&str]) -> String {
		self.default_of(path).as_str().unwrap_or_default().to_string()
	}

	fn default_bool(&self, path: &[&str]) -> bool {
		self.default_of(path).as_bool().unwrap_or(false)
	}

	fn default_num(&self, path: &[&str]) -> f64 {
		self.default_of(path).as_f64().unwrap_or(0.0)
	}

	fn bool_field(&self, key: &str, default_path: &[&str]) -> bool {
		self.field(key).and_then(Value::as_bool).unwrap_or_else(|| self.default_bool(default_path))
	}

	fn num_field(&self, key: &str, default_path: &[&str]) -> f64 {
		self.field(key).and_then(Value::as_f64).unwrap_or_else(|| self.default_num(default_path))
	}

	fn nested_bool_field(&self, outer: &str, inner: &str, default_path: &[&str]) -> bool {
		self.nested_field(outer, inner)
			.and_then(Value::as_bool)
			.unwrap_or_else(|| self.default_bool(default_path))
	}

	fn env_get(&self, key: &str) -> Option<String> {
		self.env.get(key).filter(|v| !v.is_empty()).cloned()
	}

	fn normalize_setting_path(&self, input: &str) -> String {
		normalize_path(
			input,
			&PathInputOptions {
				home_dir: self.home_dir.clone(),
				..PathInputOptions::default()
			},
		)
	}

	fn set_global_field(&mut self, field: &str, value: Value) {
		self.global_settings.insert(field.to_string(), value);
		self.mark_modified(field, None);
		self.save();
	}

	/// TS `if (!this.globalSettings.x) this.globalSettings.x = {}` then assign
	/// the nested key.
	fn set_global_nested(&mut self, field: &str, nested_key: &str, value: Value) {
		let entry = self
			.global_settings
			.entry(field.to_string())
			.or_insert_with(|| Value::Object(Map::new()));
		if !entry.is_object() {
			*entry = Value::Object(Map::new());
		}
		entry
			.as_object_mut()
			.unwrap()
			.insert(nested_key.to_string(), value);
		self.mark_modified(field, Some(nested_key));
		self.save();
	}

	// ------------------------------------------------------------------
	// Getters / setters
	// ------------------------------------------------------------------

	pub fn get_last_changelog_version(&self) -> Option<String> {
		self.field("lastChangelogVersion").and_then(Value::as_str).map(str::to_string)
	}

	pub fn set_last_changelog_version(&mut self, version: &str) {
		self.set_global_field("lastChangelogVersion", Value::String(version.to_string()));
	}

	pub fn get_session_dir(&self) -> Option<String> {
		self.field("sessionDir").and_then(Value::as_str).map(|s| self.normalize_setting_path(s))
	}

	pub fn get_default_provider(&self) -> Option<String> {
		self.field("defaultProvider").and_then(Value::as_str).map(str::to_string)
	}

	pub fn get_default_model(&self) -> Option<String> {
		self.field("defaultModel").and_then(Value::as_str).map(str::to_string)
	}

	pub fn set_default_provider(&mut self, provider: &str) {
		self.set_global_field("defaultProvider", Value::String(provider.to_string()));
	}

	pub fn set_default_model(&mut self, model_id: &str) {
		self.set_global_field("defaultModel", Value::String(model_id.to_string()));
	}

	pub fn set_default_model_and_provider(&mut self, provider: &str, model_id: &str) {
		self.global_settings
			.insert("defaultProvider".to_string(), Value::String(provider.to_string()));
		self.global_settings
			.insert("defaultModel".to_string(), Value::String(model_id.to_string()));
		self.mark_modified("defaultProvider", None);
		self.mark_modified("defaultModel", None);
		self.save();
	}

	pub fn get_steering_mode(&self) -> String {
		match self.field("steeringMode").and_then(Value::as_str).filter(|s| !s.is_empty()) {
			Some(m) => m.to_string(),
			None => self.default_str(&["steeringMode"]),
		}
	}

	pub fn set_steering_mode(&mut self, mode: &str) {
		self.set_global_field("steeringMode", Value::String(mode.to_string()));
	}

	pub fn get_follow_up_mode(&self) -> String {
		match self.field("followUpMode").and_then(Value::as_str).filter(|s| !s.is_empty()) {
			Some(m) => m.to_string(),
			None => self.default_str(&["followUpMode"]),
		}
	}

	pub fn set_follow_up_mode(&mut self, mode: &str) {
		self.set_global_field("followUpMode", Value::String(mode.to_string()));
	}

	pub fn get_theme_setting(&self) -> Option<String> {
		self.field("theme").and_then(Value::as_str).map(str::to_string)
	}

	pub fn get_theme(&self) -> Option<String> {
		self.get_theme_setting().filter(|t| !t.contains('/'))
	}

	pub fn set_theme(&mut self, theme: &str) {
		self.set_global_field("theme", Value::String(theme.to_string()));
	}

	pub fn get_default_thinking_level(&self) -> Option<String> {
		self.field("defaultThinkingLevel").and_then(Value::as_str).map(str::to_string)
	}

	pub fn set_default_thinking_level(&mut self, level: &str) {
		self.set_global_field("defaultThinkingLevel", Value::String(level.to_string()));
	}

	pub fn get_model_thinking_level(&self, provider: &str, model_id: &str) -> Option<String> {
		self.field("modelThinkingLevels")
			.and_then(|m| m.get(format!("{}/{}", provider, model_id)))
			.and_then(Value::as_str)
			.map(str::to_string)
	}

	pub fn get_all_model_thinking_levels(&self) -> Value {
		self.field("modelThinkingLevels").cloned().unwrap_or_else(|| Value::Object(Map::new()))
	}

	pub fn set_model_thinking_level(&mut self, provider: &str, model_id: &str, level: &str) {
		let key = format!("{}/{}", provider, model_id);
		let entry = self
			.global_settings
			.entry("modelThinkingLevels".to_string())
			.or_insert_with(|| Value::Object(Map::new()));
		if !entry.is_object() {
			*entry = Value::Object(Map::new());
		}
		entry
			.as_object_mut()
			.unwrap()
			.insert(key, Value::String(level.to_string()));
		self.mark_modified("modelThinkingLevels", None);
		self.save();
	}

	pub fn remove_model_thinking_level(&mut self, provider: &str, model_id: &str) {
		let key = format!("{}/{}", provider, model_id);
		let Some(Value::Object(levels)) = self.global_settings.get_mut("modelThinkingLevels") else {
			return;
		};
		levels.shift_remove(&key);
		if levels.is_empty() {
			self.global_settings.shift_remove("modelThinkingLevels");
		}
		self.mark_modified("modelThinkingLevels", None);
		self.save();
	}

	pub fn get_transport(&self) -> String {
		match self.field("transport").and_then(Value::as_str) {
			Some(t) => t.to_string(),
			None => self.default_str(&["transport"]),
		}
	}

	pub fn set_transport(&mut self, transport: &str) {
		self.set_global_field("transport", Value::String(transport.to_string()));
	}

	pub fn get_session_share_destination(&self) -> String {
		match self.field("sessionShareDestination").and_then(Value::as_str) {
			Some(d) if d == "gist" || d == "radius" => d.to_string(),
			_ => self.default_str(&["sessionShareDestination"]),
		}
	}

	pub fn set_session_share_destination(&mut self, destination: &str) {
		self.set_global_field("sessionShareDestination", Value::String(destination.to_string()));
	}

	pub fn get_compaction_enabled(&self) -> bool {
		self.nested_bool_field("compaction", "enabled", &["compaction", "enabled"])
	}

	pub fn set_compaction_enabled(&mut self, enabled: bool) {
		self.set_global_nested("compaction", "enabled", Value::Bool(enabled));
	}

	fn get_compaction_token_setting(&self, field: &str, model: Option<ModelRef<'_>>) -> Result<f64, String> {
		let compaction = self.field("compaction");
		let ordinary = compaction.and_then(|c| c.get(field));
		if ordinary.is_some_and(|v| !is_safe_nonneg_integer(v)) {
			return Err(format!(
				"Invalid compaction.{} setting: {}. Expected a non-negative safe integer.",
				field,
				js_string_value(ordinary.unwrap())
			));
		}

		let model_key = model.map(|m| format!("{}/{}", m.provider, m.id));
		let entry = model_key
			.as_deref()
			.and_then(|k| compaction.and_then(|c| c.get("modelOverrides")).and_then(|mo| mo.get(k)));
		if entry.is_some_and(|e| !is_mergeable(e)) {
			return Err(format!(
				"Invalid compaction.modelOverrides[\"{}\"] setting: {}. Expected an object.",
				model_key.as_deref().unwrap_or_default(),
				js_string_value(entry.unwrap())
			));
		}
		let override_value = entry.and_then(|e| e.get(field));
		if override_value.is_some_and(|v| !is_safe_nonneg_integer(v)) {
			return Err(format!(
				"Invalid compaction.modelOverrides[\"{}\"].{} setting: {}. Expected a non-negative safe integer.",
				model_key.as_deref().unwrap_or_default(),
				field,
				js_string_value(override_value.unwrap())
			));
		}
		Ok(override_value
			.and_then(|v| v.as_f64())
			.or_else(|| ordinary.and_then(|v| v.as_f64()))
			.unwrap_or_else(|| default_compaction_token_setting(field)))
	}

	pub fn get_compaction_reserve_tokens(&self, model: Option<ModelRef<'_>>) -> Result<f64, String> {
		self.get_compaction_token_setting("reserveTokens", model)
	}

	pub fn get_compaction_keep_recent_tokens(&self, model: Option<ModelRef<'_>>) -> Result<f64, String> {
		self.get_compaction_token_setting("keepRecentTokens", model)
	}

	/// Resolve each token setting through model override, ordinary setting, then
	/// built-in default.
	pub fn get_compaction_settings(&self, model: Option<ModelRef<'_>>) -> Result<CompactionSettings, String> {
		Ok(CompactionSettings {
			enabled: self.get_compaction_enabled(),
			reserve_tokens: self.get_compaction_reserve_tokens(model)?,
			keep_recent_tokens: self.get_compaction_keep_recent_tokens(model)?,
		})
	}

	pub fn get_branch_summary_settings(&self) -> BranchSummarySettings {
		BranchSummarySettings {
			reserve_tokens: self
				.nested_field("branchSummary", "reserveTokens")
				.and_then(Value::as_f64)
				.unwrap_or_else(|| self.default_num(&["branchSummary", "reserveTokens"])),
			skip_prompt: self.nested_bool_field("branchSummary", "skipPrompt", &["branchSummary", "skipPrompt"]),
		}
	}

	pub fn get_branch_summary_skip_prompt(&self) -> bool {
		self.nested_bool_field("branchSummary", "skipPrompt", &["branchSummary", "skipPrompt"])
	}

	pub fn get_retry_enabled(&self) -> bool {
		self.nested_bool_field("retry", "enabled", &["retry", "enabled"])
	}

	pub fn set_retry_enabled(&mut self, enabled: bool) {
		self.set_global_nested("retry", "enabled", Value::Bool(enabled));
	}

	pub fn get_retry_settings(&self) -> RetrySettings {
		RetrySettings {
			enabled: self.get_retry_enabled(),
			max_retries: self
				.nested_field("retry", "maxRetries")
				.and_then(Value::as_f64)
				.unwrap_or_else(|| self.default_num(&["retry", "maxRetries"])),
			base_delay_ms: self
				.nested_field("retry", "baseDelayMs")
				.and_then(Value::as_f64)
				.unwrap_or_else(|| self.default_num(&["retry", "baseDelayMs"])),
			max_agent_delay_ms: self
				.nested_field("retry", "maxAgentDelayMs")
				.and_then(Value::as_f64)
				.unwrap_or_else(|| self.default_num(&["retry", "maxAgentDelayMs"])),
		}
	}

	pub fn get_http_idle_timeout_ms(&self) -> Result<f64, String> {
		Ok(parse_timeout_setting(self.field("httpIdleTimeoutMs"), "httpIdleTimeoutMs")?
			.unwrap_or(DEFAULT_HTTP_IDLE_TIMEOUT_MS))
	}

	pub fn set_http_idle_timeout_ms(&mut self, timeout_ms: f64) -> Result<(), String> {
		if !timeout_ms.is_finite() || timeout_ms < 0.0 {
			return Err(format!(
				"Invalid httpIdleTimeoutMs setting: {}",
				js_f64_to_string_runtime(timeout_ms)
			));
		}
		self.set_global_field("httpIdleTimeoutMs", Value::from(timeout_ms.floor() as i64));
		Ok(())
	}

	/// Read from global settings only because warming costs money.
	pub fn get_cache_warming_mode(&self) -> String {
		let mode = self.global_settings.get("cacheWarming").and_then(Value::as_str);
		match mode {
			Some(m) if CACHE_WARMING_MODES.contains(&m) => m.to_string(),
			_ => self.default_str(&["cacheWarming"]),
		}
	}

	pub fn set_cache_warming_mode(&mut self, mode: &str) {
		self.set_global_field("cacheWarming", Value::String(mode.to_string()));
	}

	pub fn get_provider_retry_settings(&self) -> ProviderRetrySettings {
		let provider = self.nested_field("retry", "provider");
		ProviderRetrySettings {
			timeout_ms: provider.and_then(|p| p.get("timeoutMs")).and_then(Value::as_f64),
			max_retries: provider.and_then(|p| p.get("maxRetries")).and_then(Value::as_f64),
			max_retry_delay_ms: provider
				.and_then(|p| p.get("maxRetryDelayMs"))
				.and_then(Value::as_f64)
				.unwrap_or_else(|| self.default_num(&["retry", "provider", "maxRetryDelayMs"])),
		}
	}

	pub fn get_websocket_connect_timeout_ms(&self) -> Result<Option<f64>, String> {
		parse_timeout_setting(self.field("websocketConnectTimeoutMs"), "websocketConnectTimeoutMs")
	}

	pub fn get_hide_thinking_block(&self) -> bool {
		self.bool_field("hideThinkingBlock", &["hideThinkingBlock"])
	}

	pub fn set_hide_thinking_block(&mut self, hide: bool) {
		self.set_global_field("hideThinkingBlock", Value::Bool(hide));
	}

	pub fn get_show_cache_miss_notices(&self) -> bool {
		self.bool_field("showCacheMissNotices", &["showCacheMissNotices"])
	}

	pub fn set_show_cache_miss_notices(&mut self, show: bool) {
		self.set_global_field("showCacheMissNotices", Value::Bool(show));
	}

	pub fn get_external_editor_command(&self) -> String {
		if let Some(configured) = self.field("externalEditor").and_then(Value::as_str) {
			if !configured.trim().is_empty() {
				return configured.to_string();
			}
		}
		if let Some(environment_editor) = self
			.env_get("VISUAL")
			.or_else(|| self.env_get("EDITOR"))
		{
			return environment_editor;
		}
		if self.platform == "win32" {
			"notepad".to_string()
		} else {
			"nano".to_string()
		}
	}

	pub fn get_shell_path(&self) -> Option<String> {
		self.field("shellPath").and_then(Value::as_str).map(|s| self.normalize_setting_path(s))
	}

	pub fn set_shell_path(&mut self, path: Option<&str>) {
		// TS assigns the raw value; JSON null persists as `"shellPath": null`
		// (undefined is not expressible through JSON, so None maps to null).
		match path {
			Some(p) => self.set_global_field("shellPath", Value::String(p.to_string())),
			None => self.set_global_field("shellPath", Value::Null),
		}
	}

	pub fn get_quiet_startup(&self) -> Value {
		match self.field("quietStartup") {
			Some(Value::Bool(true)) => Value::Bool(true),
			Some(Value::String(s)) if s == "header" => Value::String(s.clone()),
			_ => self.default_of(&["quietStartup"]),
		}
	}

	pub fn set_quiet_startup(&mut self, quiet: &Value) {
		self.set_global_field("quietStartup", quiet.clone());
	}

	pub fn get_default_project_trust(&self) -> String {
		match self.global_settings.get("defaultProjectTrust").and_then(Value::as_str) {
			Some(v) if v == "always" || v == "never" => v.to_string(),
			_ => self.default_str(&["defaultProjectTrust"]),
		}
	}

	pub fn set_default_project_trust(&mut self, default_project_trust: &str) {
		self.set_global_field("defaultProjectTrust", Value::String(default_project_trust.to_string()));
	}

	pub fn get_shell_command_prefix(&self) -> Option<String> {
		self.field("shellCommandPrefix").and_then(Value::as_str).map(str::to_string)
	}

	pub fn set_shell_command_prefix(&mut self, prefix: Option<&str>) {
		// TS assigns the raw value; see set_shell_path re None → null.
		match prefix {
			Some(p) => self.set_global_field("shellCommandPrefix", Value::String(p.to_string())),
			None => self.set_global_field("shellCommandPrefix", Value::Null),
		}
	}

	pub fn get_npm_command(&self) -> Option<Vec<String>> {
		self.field("npmCommand").and_then(Value::as_array).map(|a| {
			a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
		})
	}

	pub fn set_npm_command(&mut self, command: Option<&[String]>) {
		// TS: `command ? [...command] : undefined` — null/undefined both
		// collapse to undefined (JSON.stringify omits the key).
		match command {
			Some(c) => self.set_global_field("npmCommand", Value::Array(c.iter().map(|s| Value::String(s.clone())).collect())),
			None => {
				self.global_settings.shift_remove("npmCommand");
				self.mark_modified("npmCommand", None);
				self.save();
			}
		}
	}

	pub fn get_collapse_changelog(&self) -> bool {
		self.bool_field("collapseChangelog", &["collapseChangelog"])
	}

	pub fn set_collapse_changelog(&mut self, collapse: bool) {
		self.set_global_field("collapseChangelog", Value::Bool(collapse));
	}

	pub fn get_enable_install_telemetry(&self) -> bool {
		self.bool_field("enableInstallTelemetry", &["enableInstallTelemetry"])
	}

	pub fn set_enable_install_telemetry(&mut self, enabled: bool) {
		self.set_global_field("enableInstallTelemetry", Value::Bool(enabled));
	}

	pub fn get_enable_analytics(&self) -> bool {
		self.bool_field("enableAnalytics", &["enableAnalytics"])
	}

	pub fn get_tracking_id(&self) -> Option<String> {
		self.field("trackingId").and_then(Value::as_str).map(str::to_string)
	}

	/// Set the analytics opt-in preference; generates a tracking identifier on
	/// first opt-in.
	pub fn set_enable_analytics(&mut self, enabled: bool) {
		self.global_settings.insert("enableAnalytics".to_string(), Value::Bool(enabled));
		self.mark_modified("enableAnalytics", None);
		if enabled && self.global_settings.get("trackingId").and_then(Value::as_str).is_none() {
			let id = (self.uuid_gen)();
			self.global_settings.insert("trackingId".to_string(), Value::String(id));
			self.mark_modified("trackingId", None);
		}
		self.save();
	}

	/// Stable ID of this installation. Project settings are ignored so a
	/// committed project settings file cannot give every clone the same ID.
	pub fn get_or_create_device_id(&mut self) -> String {
		if self.global_settings.get("deviceId").and_then(Value::as_str).is_none() {
			let id = (self.uuid_gen)();
			self.global_settings.insert("deviceId".to_string(), Value::String(id));
			self.mark_modified("deviceId", None);
			self.save();
		}
		self.global_settings
			.get("deviceId")
			.and_then(Value::as_str)
			.unwrap_or_default()
			.to_string()
	}

	pub fn get_packages(&self) -> Vec<Value> {
		self.field("packages").and_then(Value::as_array).cloned().unwrap_or_default()
	}

	pub fn set_packages(&mut self, packages: &[Value]) {
		self.set_global_field("packages", Value::Array(packages.to_vec()));
	}

	pub fn set_project_packages(&mut self, packages: &[Value]) -> Result<(), String> {
		let p = packages.to_vec();
		self.update_project_settings("packages", |settings| {
			settings.insert("packages".to_string(), Value::Array(p.clone()));
		})
	}

	pub fn get_extension_paths(&self) -> Vec<Value> {
		self.field("extensions").and_then(Value::as_array).cloned().unwrap_or_default()
	}

	pub fn set_extension_paths(&mut self, paths: &[Value]) {
		self.set_global_field("extensions", Value::Array(paths.to_vec()));
	}

	pub fn set_project_extension_paths(&mut self, paths: &[Value]) -> Result<(), String> {
		let p = paths.to_vec();
		self.update_project_settings("extensions", |settings| {
			settings.insert("extensions".to_string(), Value::Array(p.clone()));
		})
	}

	pub fn get_skill_paths(&self) -> Vec<Value> {
		self.field("skills").and_then(Value::as_array).cloned().unwrap_or_default()
	}

	pub fn set_skill_paths(&mut self, paths: &[Value]) {
		self.set_global_field("skills", Value::Array(paths.to_vec()));
	}

	pub fn set_project_skill_paths(&mut self, paths: &[Value]) -> Result<(), String> {
		let p = paths.to_vec();
		self.update_project_settings("skills", |settings| {
			settings.insert("skills".to_string(), Value::Array(p.clone()));
		})
	}

	pub fn get_prompt_template_paths(&self) -> Vec<Value> {
		self.field("prompts").and_then(Value::as_array).cloned().unwrap_or_default()
	}

	pub fn set_prompt_template_paths(&mut self, paths: &[Value]) {
		self.set_global_field("prompts", Value::Array(paths.to_vec()));
	}

	pub fn set_project_prompt_template_paths(&mut self, paths: &[Value]) -> Result<(), String> {
		let p = paths.to_vec();
		self.update_project_settings("prompts", |settings| {
			settings.insert("prompts".to_string(), Value::Array(p.clone()));
		})
	}

	pub fn get_theme_paths(&self) -> Vec<Value> {
		self.field("themes").and_then(Value::as_array).cloned().unwrap_or_default()
	}

	pub fn set_theme_paths(&mut self, paths: &[Value]) {
		self.set_global_field("themes", Value::Array(paths.to_vec()));
	}

	pub fn set_project_theme_paths(&mut self, paths: &[Value]) -> Result<(), String> {
		let p = paths.to_vec();
		self.update_project_settings("themes", |settings| {
			settings.insert("themes".to_string(), Value::Array(p.clone()));
		})
	}

	pub fn get_enable_skill_commands(&self) -> bool {
		self.bool_field("enableSkillCommands", &["enableSkillCommands"])
	}

	pub fn set_enable_skill_commands(&mut self, enabled: bool) {
		self.set_global_field("enableSkillCommands", Value::Bool(enabled));
	}

	pub fn get_thinking_budgets(&self) -> Option<Value> {
		self.field("thinkingBudgets").cloned()
	}

	pub fn get_terminal_capability_overrides(&self) -> Value {
		let terminal = self.field("terminal");
		let images = terminal.and_then(|t| t.get("images"));
		let mut out = Map::new();
		match images {
			Some(Value::String(s)) if s == "kitty" || s == "iterm2" => {
				out.insert("images".to_string(), Value::String(s.clone()));
			}
			Some(Value::Bool(false)) => {
				out.insert("images".to_string(), Value::Null);
			}
			_ => {}
		}
		if let Some(Value::Bool(b)) = terminal.and_then(|t| t.get("trueColor")) {
			out.insert("trueColor".to_string(), Value::Bool(*b));
		}
		if let Some(Value::Bool(b)) = terminal.and_then(|t| t.get("hyperlinks")) {
			out.insert("hyperlinks".to_string(), Value::Bool(*b));
		}
		Value::Object(out)
	}

	pub fn get_show_images(&self) -> bool {
		self.nested_bool_field("terminal", "showImages", &["terminal", "showImages"])
	}

	pub fn set_show_images(&mut self, show: bool) {
		self.set_global_nested("terminal", "showImages", Value::Bool(show));
	}

	pub fn get_image_width_cells(&self) -> f64 {
		match self.nested_field("terminal", "imageWidthCells").and_then(Value::as_f64) {
			Some(width) if width.is_finite() => width.floor().max(1.0),
			_ => self.default_num(&["terminal", "imageWidthCells"]),
		}
	}

	pub fn set_image_width_cells(&mut self, width: f64) {
		let clamped = width.floor().max(1.0);
		self.set_global_nested("terminal", "imageWidthCells", Value::from(clamped as i64));
	}

	pub fn get_clear_on_shrink(&self) -> bool {
		// Settings takes precedence, then env var, then default false.
		match self.nested_field("terminal", "clearOnShrink") {
			Some(v) => v.as_bool().unwrap_or(false),
			None => {
				if self.env.get("PI_CLEAR_ON_SHRINK").map(String::as_str) == Some("1") {
					return true;
				}
				self.default_bool(&["terminal", "clearOnShrink"])
			}
		}
	}

	pub fn set_clear_on_shrink(&mut self, enabled: bool) {
		self.set_global_nested("terminal", "clearOnShrink", Value::Bool(enabled));
	}

	pub fn get_show_terminal_progress(&self) -> bool {
		self.nested_bool_field("terminal", "showTerminalProgress", &["terminal", "showTerminalProgress"])
	}

	pub fn set_show_terminal_progress(&mut self, enabled: bool) {
		self.set_global_nested("terminal", "showTerminalProgress", Value::Bool(enabled));
	}

	pub fn get_tui_mode(&self) -> String {
		if self.field("tuiMode").and_then(Value::as_str) == Some("regular") {
			"regular".to_string()
		} else {
			self.default_str(&["tuiMode"])
		}
	}

	pub fn set_tui_mode(&mut self, mode: &str) {
		self.set_global_field("tuiMode", Value::String(mode.to_string()));
	}

	pub fn get_fullscreen_exit_output(&self) -> String {
		if self.field("fullscreenExitOutput").and_then(Value::as_str) == Some("resume-hint") {
			"resume-hint".to_string()
		} else {
			self.default_str(&["fullscreenExitOutput"])
		}
	}

	pub fn set_fullscreen_exit_output(&mut self, output: &str) {
		self.set_global_field("fullscreenExitOutput", Value::String(output.to_string()));
	}

	pub fn get_fullscreen_scrollbar(&self) -> String {
		match self.field("fullscreenScrollbar").and_then(Value::as_str) {
			Some(m) if m == "always" || m == "hidden" => m.to_string(),
			_ => self.default_str(&["fullscreenScrollbar"]),
		}
	}

	pub fn set_fullscreen_scrollbar(&mut self, mode: &str) {
		self.set_global_field("fullscreenScrollbar", Value::String(mode.to_string()));
	}

	pub fn get_fullscreen_copy_on_select(&self) -> bool {
		self.bool_field("fullscreenCopyOnSelect", &["fullscreenCopyOnSelect"])
	}

	pub fn set_fullscreen_copy_on_select(&mut self, enabled: bool) {
		self.set_global_field("fullscreenCopyOnSelect", Value::Bool(enabled));
	}

	pub fn get_fullscreen_wheel_scroll_lines(&self) -> Value {
		match self.field("fullscreenWheelScrollLines").and_then(Value::as_f64) {
			Some(lines) if lines.is_finite() => {
				Value::from(lines.floor().clamp(1.0, 100.0) as i64)
			}
			_ => self.default_of(&["fullscreenWheelScrollLines"]),
		}
	}

	pub fn set_fullscreen_wheel_scroll_lines(&mut self, lines: WheelScrollLines) {
		let value = match lines {
			WheelScrollLines::Auto => Value::String("auto".to_string()),
			WheelScrollLines::Lines(n) => Value::from(n.floor().clamp(1.0, 100.0) as i64),
		};
		self.set_global_field("fullscreenWheelScrollLines", value);
	}

	pub fn get_image_auto_resize(&self) -> bool {
		self.nested_bool_field("images", "autoResize", &["images", "autoResize"])
	}

	pub fn set_image_auto_resize(&mut self, enabled: bool) {
		self.set_global_nested("images", "autoResize", Value::Bool(enabled));
	}

	pub fn get_block_images(&self) -> bool {
		self.nested_bool_field("images", "blockImages", &["images", "blockImages"])
	}

	pub fn set_block_images(&mut self, blocked: bool) {
		self.set_global_nested("images", "blockImages", Value::Bool(blocked));
	}

	pub fn get_enabled_models(&self) -> Option<Value> {
		self.field("enabledModels").cloned()
	}

	/// The resolved `defaultTools` selection, or `None` when no settings layer
	/// sets it.
	pub fn get_default_tools(&self) -> Option<Vec<String>> {
		let tools = self.field("defaultTools")?;
		let entries: Vec<Value> = match tools {
			Value::Array(a) => a.iter().filter(|t| t.is_string()).cloned().collect(),
			_ => Vec::new(),
		};
		Some(resolve_default_tools(&entries))
	}

	pub fn set_enabled_models(&mut self, patterns: Option<&[String]>) {
		// TS assigns the raw value; see set_shell_path re None → null.
		match patterns {
			Some(p) => self.set_global_field(
				"enabledModels",
				Value::Array(p.iter().map(|s| Value::String(s.clone())).collect()),
			),
			None => self.set_global_field("enabledModels", Value::Null),
		}
	}

	pub fn get_double_escape_action(&self) -> String {
		match self.field("doubleEscapeAction").and_then(Value::as_str) {
			Some(a) => a.to_string(),
			None => self.default_str(&["doubleEscapeAction"]),
		}
	}

	pub fn set_double_escape_action(&mut self, action: &str) {
		self.set_global_field("doubleEscapeAction", Value::String(action.to_string()));
	}

	pub fn get_tree_filter_mode(&self) -> String {
		const VALID: [&str; 5] = ["default", "no-tools", "user-only", "labeled-only", "all"];
		match self.field("treeFilterMode").and_then(Value::as_str) {
			Some(m) if !m.is_empty() && VALID.contains(&m) => m.to_string(),
			_ => self.default_str(&["treeFilterMode"]),
		}
	}

	pub fn set_tree_filter_mode(&mut self, mode: &str) {
		self.set_global_field("treeFilterMode", Value::String(mode.to_string()));
	}

	pub fn get_show_hardware_cursor(&self) -> bool {
		self.field("showHardwareCursor")
			.and_then(Value::as_bool)
			.unwrap_or_else(|| self.env.get("PI_HARDWARE_CURSOR").map(String::as_str) == Some("1"))
	}

	pub fn set_show_hardware_cursor(&mut self, enabled: bool) {
		self.set_global_field("showHardwareCursor", Value::Bool(enabled));
	}

	pub fn get_editor_padding_x(&self) -> f64 {
		self.num_field("editorPaddingX", &["editorPaddingX"])
	}

	pub fn set_editor_padding_x(&mut self, padding: f64) {
		let clamped = padding.floor().clamp(0.0, 3.0);
		self.set_global_field("editorPaddingX", Value::from(clamped as i64));
	}

	pub fn get_output_pad(&self) -> f64 {
		if self.field("outputPad").and_then(Value::as_f64) == Some(0.0) {
			0.0
		} else {
			self.default_num(&["outputPad"])
		}
	}

	pub fn set_output_pad(&mut self, padding: f64) {
		// TS stores the raw value (no floor); `js_stringify_pretty` renders
		// whole f64s without a trailing ".0", so persisted bytes match.
		self.set_global_field("outputPad", Value::from(padding));
	}

	pub fn get_autocomplete_max_visible(&self) -> f64 {
		self.num_field("autocompleteMaxVisible", &["autocompleteMaxVisible"])
	}

	pub fn set_autocomplete_max_visible(&mut self, max_visible: f64) {
		let clamped = max_visible.floor().clamp(3.0, 20.0);
		self.set_global_field("autocompleteMaxVisible", Value::from(clamped as i64));
	}

	pub fn get_code_block_indent(&self) -> String {
		self.nested_field("markdown", "codeBlockIndent")
			.and_then(Value::as_str)
			.map(str::to_string)
			.unwrap_or_else(|| self.default_str(&["markdown", "codeBlockIndent"]))
	}

	pub fn get_mermaid_rendering_mode(&self) -> String {
		match self.nested_field("markdown", "mermaid").and_then(Value::as_str) {
			Some(m) if m == "off" || m == "final" => m.to_string(),
			_ => self.default_str(&["markdown", "mermaid"]),
		}
	}

	pub fn set_mermaid_rendering_mode(&mut self, mode: &str) {
		self.set_global_nested("markdown", "mermaid", Value::String(mode.to_string()));
	}

	pub fn get_warnings(&self) -> Value {
		Value::Object(self.field("warnings").map(js_object_spread).unwrap_or_default())
	}

	pub fn set_warnings(&mut self, warnings: &Value) {
		let copy = js_object_spread(warnings);
		self.set_global_field("warnings", Value::Object(copy));
	}
}

/// JS object-spread `{ ...value }` semantics for the raw (unvalidated)
/// settings values: strings and arrays spread to index keys (UTF-16 code
/// units for strings), null/numbers/booleans spread to `{}`, and objects
/// shallow-copy. Mirrors TS `{ ...x }` on unvalidated input.
fn js_object_spread(value: &Value) -> serde_json::Map<String, Value> {
	let mut out = serde_json::Map::new();
	match value {
		Value::Object(o) => out = o.clone(),
		Value::Array(a) => {
			for (i, v) in a.iter().enumerate() {
				out.insert(i.to_string(), v.clone());
			}
		}
		Value::String(s) => {
			for (i, unit) in s.encode_utf16().enumerate() {
				out.insert(
					i.to_string(),
					Value::String(String::from_utf16_lossy(&[unit])),
				);
			}
		}
		_ => {}
	}
	out
}

/// JS `String(number)` for the `Invalid ... setting: ${String(value)}` message
/// (includes NaN/Infinity, unlike the JSON-stringify variant).
fn js_f64_to_string_runtime(v: f64) -> String {
	if v.is_nan() {
		"NaN".to_string()
	} else if v == f64::INFINITY {
		"Infinity".to_string()
	} else if v == f64::NEG_INFINITY {
		"-Infinity".to_string()
	} else {
		js_f64_to_string(v)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn scratch(tag: &str) -> PathBuf {
		use std::sync::atomic::{AtomicUsize, Ordering};
		static COUNTER: AtomicUsize = AtomicUsize::new(0);
		let n = COUNTER.fetch_add(1, Ordering::SeqCst);
		let dir = std::env::temp_dir().join(format!(
			"pi-core-settings-{}-{}-{}",
			std::process::id(),
			tag,
			n
		));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(dir.join("agent")).unwrap();
		std::fs::create_dir_all(dir.join("project").join(".pi")).unwrap();
		dir
	}

	fn write(path: &Path, content: &str) {
		std::fs::write(path, content).unwrap();
	}

	fn read_json(path: &Path) -> Value {
		parse_json_like(strip_bom(&std::fs::read_to_string(path).unwrap())).unwrap()
	}

	fn opts() -> SettingsManagerOptions {
		SettingsManagerOptions {
			uuid_gen: Some(Arc::new(|| "11111111-1111-4111-8111-111111111111".to_string())),
			..Default::default()
		}
	}

	fn det_uuid() -> UuidGen {
		Arc::new(|| "11111111-1111-4111-8111-111111111111".to_string())
	}

	fn env_opts(env: &[(&str, &str)], platform: Option<&str>) -> SettingsManagerOptions {
		SettingsManagerOptions {
			env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
			platform: platform.map(str::to_string),
			uuid_gen: Some(det_uuid()),
			..Default::default()
		}
	}

	fn create(cwd: &Path, agent: &Path, options: SettingsManagerOptions) -> SettingsManager {
		SettingsManager::create(
			&cwd.to_string_lossy(),
			&agent.to_string_lossy(),
			options,
		)
	}

	fn in_mem(settings: Value) -> SettingsManager {
		SettingsManager::in_memory(&settings, opts())
	}

	fn model<'a>(provider: &'a str, id: &'a str) -> Option<ModelRef<'a>> {
		Some(ModelRef { provider, id })
	}

	const MODEL_KEY: &str = "provider/family/model";
	fn mref() -> Option<ModelRef<'static>> {
		model("provider", "family/model")
	}
	fn defaults_compaction() -> CompactionSettings {
		CompactionSettings { enabled: true, reserve_tokens: 16384.0, keep_recent_tokens: 20000.0 }
	}

	// ---- settings-manager.test.ts -------------------------------------

	#[test]
	fn preserves_enabled_models_when_changing_thinking_level() {
		let dir = scratch("a1");
		let settings_path = dir.join("agent").join("settings.json");
		write(&settings_path, r#"{"theme":"dark","defaultModel":"claude-sonnet"}"#);
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		let mut current = read_json(&settings_path);
		current["enabledModels"] = json!(["claude-opus-4-5", "gpt-5.2-codex"]);
		write(&settings_path, &js_stringify_pretty(&current));
		manager.set_default_thinking_level("high");
		manager.flush();
		let saved = read_json(&settings_path);
		assert_eq!(saved["enabledModels"], json!(["claude-opus-4-5", "gpt-5.2-codex"]));
		assert_eq!(saved["defaultThinkingLevel"], json!("high"));
		assert_eq!(saved["theme"], json!("dark"));
		assert_eq!(saved["defaultModel"], json!("claude-sonnet"));
	}

	#[test]
	fn preserves_custom_settings_when_changing_theme() {
		let dir = scratch("a2");
		let settings_path = dir.join("agent").join("settings.json");
		write(&settings_path, r#"{"defaultModel":"claude-sonnet"}"#);
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		let mut current = read_json(&settings_path);
		current["shellPath"] = json!("/bin/zsh");
		current["extensions"] = json!(["/path/to/extension.ts"]);
		write(&settings_path, &js_stringify_pretty(&current));
		manager.set_theme("light");
		manager.flush();
		let saved = read_json(&settings_path);
		assert_eq!(saved["shellPath"], json!("/bin/zsh"));
		assert_eq!(saved["extensions"], json!(["/path/to/extension.ts"]));
		assert_eq!(saved["theme"], json!("light"));
	}

	#[test]
	fn in_memory_changes_override_file_changes_for_same_key() {
		let dir = scratch("a3");
		let settings_path = dir.join("agent").join("settings.json");
		write(&settings_path, r#"{"theme":"dark"}"#);
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		let mut current = read_json(&settings_path);
		current["defaultThinkingLevel"] = json!("low");
		write(&settings_path, &js_stringify_pretty(&current));
		manager.set_default_thinking_level("high");
		manager.flush();
		assert_eq!(read_json(&settings_path)["defaultThinkingLevel"], json!("high"));
	}

	#[test]
	fn creates_one_global_device_id_and_reuses_it() {
		let dir = scratch("a4");
		let settings_path = dir.join("agent").join("settings.json");
		write(&settings_path, r#"{"theme":"dark"}"#);
		write(&dir.join("project").join(".pi").join("settings.json"), r#"{"deviceId":"project-device"}"#);
		let mut first = create(&dir.join("project"), &dir.join("agent"), SettingsManagerOptions::default());
		let device_id = first.get_or_create_device_id();
		first.flush();
		assert!(device_id.len() == 36
			&& device_id.as_bytes()[14] == b'4'
			&& device_id.as_bytes()[19] >= b'8'
			&& device_id.as_bytes()[19] <= b'b'
			&& device_id.chars().filter(|c| *c == '-').count() == 4
			&& device_id
				.chars()
				.all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
		assert_eq!(first.get_or_create_device_id(), device_id);
		assert_eq!(
			create(&dir.join("project"), &dir.join("agent"), SettingsManagerOptions::default())
				.get_or_create_device_id(),
			device_id
		);
		assert_eq!(read_json(&settings_path), json!({"theme": "dark", "deviceId": device_id}));
	}

	#[test]
	fn keeps_local_only_extensions_in_extensions_array() {
		let dir = scratch("a5");
		write(
			&dir.join("agent").join("settings.json"),
			r#"{"extensions":["/local/ext.ts","./relative/ext.ts"]}"#,
		);
		let manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(manager.get_packages(), Vec::<Value>::new());
		assert_eq!(manager.get_extension_paths(), vec![json!("/local/ext.ts"), json!("./relative/ext.ts")]);
	}

	#[test]
	fn handles_packages_with_filtering_objects() {
		let dir = scratch("a6");
		write(
			&dir.join("agent").join("settings.json"),
			r#"{"packages":["npm:simple-pkg",{"source":"npm:shitty-extensions","extensions":["extensions/oracle.ts"],"skills":[]}]}"#,
		);
		let manager = create(&dir.join("project"), &dir.join("agent"), opts());
		let packages = manager.get_packages();
		assert_eq!(packages.len(), 2);
		assert_eq!(packages[0], json!("npm:simple-pkg"));
		assert_eq!(
			packages[1],
			json!({"source":"npm:shitty-extensions","extensions":["extensions/oracle.ts"],"skills":[]})
		);
	}

	#[test]
	fn reloads_global_settings_from_disk() {
		let dir = scratch("a7");
		let settings_path = dir.join("agent").join("settings.json");
		write(&settings_path, r#"{"theme":"dark","extensions":["/before.ts"]}"#);
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		write(
			&settings_path,
			r#"{"theme":"light","extensions":["/after.ts"],"defaultModel":"claude-sonnet"}"#,
		);
		manager.reload();
		assert_eq!(manager.get_theme().as_deref(), Some("light"));
		assert_eq!(manager.get_extension_paths(), vec![json!("/after.ts")]);
		assert_eq!(manager.get_default_model().as_deref(), Some("claude-sonnet"));
	}

	#[test]
	fn keeps_previous_settings_and_reports_path_when_invalid() {
		let dir = scratch("a8");
		let settings_path = dir.join("agent").join("settings.json");
		write(&settings_path, r#"{"theme":"dark"}"#);
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		write(&settings_path, "{ invalid json");
		manager.reload();
		assert_eq!(manager.get_theme().as_deref(), Some("dark"));
		let errors = manager.drain_errors();
		assert_eq!(errors.len(), 1);
		assert_eq!(errors[0].scope, SettingsScope::Global);
		assert_eq!(errors[0].path.as_deref(), Some(settings_path.to_string_lossy().as_ref()));
	}

	#[test]
	fn stores_slash_separated_theme_settings() {
		let dir = scratch("a9");
		let settings_path = dir.join("agent").join("settings.json");
		write(&settings_path, r#"{"theme":"light/dark"}"#);
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(manager.get_theme(), None);
		assert_eq!(manager.get_theme_setting().as_deref(), Some("light/dark"));
		manager.set_theme("solarized-light/tokyo-night");
		manager.flush();
		assert_eq!(
			read_json(&settings_path)["theme"],
			json!("solarized-light/tokyo-night")
		);
	}

	#[test]
	fn collects_and_clears_load_errors_via_drain_errors() {
		let dir = scratch("a10");
		let global_path = dir.join("agent").join("settings.json");
		let project_path = dir.join("project").join(".pi").join("settings.json");
		write(&global_path, "{ invalid global json");
		write(&project_path, "{ invalid project json");
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		let errors = manager.drain_errors();
		assert_eq!(errors.len(), 2);
		assert_eq!(errors[0].scope, SettingsScope::Global);
		assert_eq!(errors[0].path.as_deref(), Some(global_path.to_string_lossy().as_ref()));
		assert_eq!(errors[1].scope, SettingsScope::Project);
		assert_eq!(errors[1].path.as_deref(), Some(project_path.to_string_lossy().as_ref()));
		assert_eq!(manager.drain_errors(), Vec::<SettingsError>::new());
	}

	#[test]
	fn skips_project_settings_when_project_is_not_trusted() {
		let dir = scratch("a11");
		write(&dir.join("agent").join("settings.json"), r#"{"theme":"global"}"#);
		write(&dir.join("project").join(".pi").join("settings.json"), r#"{"theme":"project"}"#);
		let manager = create(
			&dir.join("project"),
			&dir.join("agent"),
			SettingsManagerOptions { project_trusted: Some(false), ..opts() },
		);
		assert!(!manager.is_project_trusted());
		assert_eq!(manager.get_theme().as_deref(), Some("global"));
		assert_eq!(manager.get_project_settings(), json!({}));
	}

	#[test]
	fn reloads_project_settings_after_trust_changes_to_true() {
		let dir = scratch("a12");
		write(&dir.join("agent").join("settings.json"), r#"{"theme":"global"}"#);
		write(&dir.join("project").join(".pi").join("settings.json"), r#"{"theme":"project"}"#);
		let mut manager = create(
			&dir.join("project"),
			&dir.join("agent"),
			SettingsManagerOptions { project_trusted: Some(false), ..opts() },
		);
		manager.set_project_trusted(true);
		assert!(manager.is_project_trusted());
		assert_eq!(manager.get_theme().as_deref(), Some("project"));
	}

	#[test]
	fn fails_project_settings_writes_when_project_is_not_trusted() {
		let dir = scratch("a13");
		let project_path = dir.join("project").join(".pi").join("settings.json");
		write(&project_path, r#"{"packages":["npm:existing"]}"#);
		let mut manager = create(
			&dir.join("project"),
			&dir.join("agent"),
			SettingsManagerOptions { project_trusted: Some(false), ..opts() },
		);
		assert_eq!(
			manager.set_project_packages(&[json!("npm:new")]),
			Err("Project is not trusted; refusing to write project settings".to_string())
		);
		manager.flush();
		assert_eq!(manager.get_project_settings(), json!({}));
		assert_eq!(read_json(&project_path), json!({"packages": ["npm:existing"]}));
	}

	#[test]
	fn reads_default_project_trust_from_global_settings_only() {
		let dir = scratch("a14");
		write(&dir.join("agent").join("settings.json"), r#"{"defaultProjectTrust":"always"}"#);
		write(&dir.join("project").join(".pi").join("settings.json"), r#"{"defaultProjectTrust":"never"}"#);
		let manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(manager.get_default_project_trust(), "always");
	}

	#[test]
	fn defaults_invalid_project_trust_settings_to_ask() {
		let dir = scratch("a15");
		write(&dir.join("agent").join("settings.json"), r#"{"defaultProjectTrust":"sometimes"}"#);
		let manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(manager.get_default_project_trust(), "ask");
	}

	#[test]
	fn does_not_create_pi_folder_when_only_reading() {
		let dir = scratch("a16");
		write(&dir.join("agent").join("settings.json"), r#"{"theme":"dark"}"#);
		std::fs::remove_dir_all(dir.join("project").join(".pi")).unwrap();
		let manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert!(!dir.join("project").join(".pi").exists());
		assert_eq!(manager.get_theme().as_deref(), Some("dark"));
	}

	#[test]
	fn creates_pi_folder_when_writing_project_settings() {
		let dir = scratch("a17");
		write(&dir.join("agent").join("settings.json"), r#"{"theme":"dark"}"#);
		std::fs::remove_dir_all(dir.join("project").join(".pi")).unwrap();
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert!(!dir.join("project").join(".pi").exists());
		manager.set_project_packages(&[json!({"source": "npm:test-pkg"})]).unwrap();
		manager.flush();
		assert!(dir.join("project").join(".pi").exists());
		assert!(dir.join("project").join(".pi").join("settings.json").exists());
	}

	#[test]
	fn maps_explicit_values_and_omits_auto_values() {
		let get_overrides = |terminal: Value| SettingsManager::in_memory(
			&json!({ "terminal": terminal }),
			opts(),
		)
		.get_terminal_capability_overrides();
		assert_eq!(
			get_overrides(json!({"images": false, "trueColor": false, "hyperlinks": false})),
			json!({"images": null, "trueColor": false, "hyperlinks": false})
		);
		assert_eq!(
			get_overrides(json!({"images": "kitty", "trueColor": true, "hyperlinks": true})),
			json!({"images": "kitty", "trueColor": true, "hyperlinks": true})
		);
		assert_eq!(get_overrides(json!({"images": "auto", "trueColor": "auto", "hyperlinks": "auto"})), json!({}));
	}

	#[test]
	fn defaults_and_overrides_agent_retry_delay_cap() {
		assert_eq!(
			in_mem(json!({})).get_retry_settings(),
			RetrySettings { enabled: true, max_retries: 3.0, base_delay_ms: 2000.0, max_agent_delay_ms: 60000.0 }
		);
		assert_eq!(
			in_mem(json!({"retry": {"enabled": true, "maxRetries": 10, "baseDelayMs": 500, "maxAgentDelayMs": 5000}}))
				.get_retry_settings(),
			RetrySettings { enabled: true, max_retries: 10.0, base_delay_ms: 500.0, max_agent_delay_ms: 5000.0 }
		);
	}

	#[test]
	fn http_idle_timeout_defaults_to_5_minutes() {
		let dir = scratch("a18");
		let manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(manager.get_http_idle_timeout_ms().unwrap(), DEFAULT_HTTP_IDLE_TIMEOUT_MS);
	}

	#[test]
	fn http_idle_timeout_uses_merged_settings() {
		let dir = scratch("a19");
		write(&dir.join("agent").join("settings.json"), r#"{"httpIdleTimeoutMs":300000}"#);
		write(&dir.join("project").join(".pi").join("settings.json"), r#"{"httpIdleTimeoutMs":0}"#);
		let manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(manager.get_http_idle_timeout_ms().unwrap(), 0.0);
	}

	#[test]
	fn http_idle_timeout_rejects_invalid_values() {
		let dir = scratch("a20");
		write(&dir.join("agent").join("settings.json"), r#"{"httpIdleTimeoutMs":-1}"#);
		let manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(
			manager.get_http_idle_timeout_ms(),
			Err("Invalid httpIdleTimeoutMs setting: -1".to_string())
		);
	}

	#[test]
	fn cache_warming_defaults_to_streaming_and_ignores_project_settings() {
		let dir = scratch("a21");
		assert_eq!(create(&dir.join("project"), &dir.join("agent"), opts()).get_cache_warming_mode(), "streaming");
		write(&dir.join("project").join(".pi").join("settings.json"), r#"{"cacheWarming":"idle"}"#);
		assert_eq!(create(&dir.join("project"), &dir.join("agent"), opts()).get_cache_warming_mode(), "streaming");
		write(&dir.join("agent").join("settings.json"), r#"{"cacheWarming":"idle"}"#);
		assert_eq!(create(&dir.join("project"), &dir.join("agent"), opts()).get_cache_warming_mode(), "idle");
		write(&dir.join("agent").join("settings.json"), r#"{"cacheWarming":"bogus"}"#);
		assert_eq!(create(&dir.join("project"), &dir.join("agent"), opts()).get_cache_warming_mode(), "streaming");
	}

	#[test]
	fn cache_warming_persists_the_mode_globally() {
		let dir = scratch("a22");
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		manager.set_cache_warming_mode("off");
		manager.flush();
		assert_eq!(
			create(&dir.join("project"), &dir.join("agent"), opts()).get_cache_warming_mode(),
			"off"
		);
		assert_eq!(read_json(&dir.join("agent").join("settings.json")), json!({"cacheWarming": "off"}));
	}

	#[test]
	fn resolves_editor_commands_by_precedence() {
		let opts = env_opts(&[("VISUAL", "vim"), ("EDITOR", "nano")], None);
		assert_eq!(
			SettingsManager::in_memory(&json!({"externalEditor": "code --wait"}), opts.clone())
				.get_external_editor_command(),
			"code --wait"
		);
		assert_eq!(
			SettingsManager::in_memory(&json!({}), opts).get_external_editor_command(),
			"vim"
		);
		assert_eq!(
			SettingsManager::in_memory(&json!({}), env_opts(&[("EDITOR", "emacs")], None))
				.get_external_editor_command(),
			"emacs"
		);
	}

	#[test]
	fn editor_falls_back_to_platform_defaults() {
		for (platform, expected) in [("win32", "notepad"), ("darwin", "nano"), ("linux", "nano")] {
			assert_eq!(
				SettingsManager::in_memory(&json!({}), env_opts(&[], Some(platform)))
					.get_external_editor_command(),
				expected
			);
		}
	}

	#[test]
	fn tui_mode_defaults_to_fullscreen_and_persists_regular_mode() {
		let dir = scratch("a23");
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(manager.get_tui_mode(), "fullscreen");
		manager.set_tui_mode("regular");
		manager.flush();
		assert_eq!(manager.get_tui_mode(), "regular");
		assert_eq!(read_json(&dir.join("agent").join("settings.json"))["tuiMode"], json!("regular"));
	}

	#[test]
	fn tui_mode_falls_back_for_unsupported_values() {
		let dir = scratch("a24");
		write(&dir.join("agent").join("settings.json"), r#"{"tuiMode":"other"}"#);
		assert_eq!(create(&dir.join("project"), &dir.join("agent"), opts()).get_tui_mode(), "fullscreen");
	}

	#[test]
	fn tui_mode_does_not_recognize_old_ui_mode_setting() {
		let dir = scratch("a25");
		write(&dir.join("agent").join("settings.json"), r#"{"uiMode":"regular"}"#);
		assert_eq!(create(&dir.join("project"), &dir.join("agent"), opts()).get_tui_mode(), "fullscreen");
	}

	#[test]
	fn validates_and_persists_fullscreen_settings() {
		let dir = scratch("a26");
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(manager.get_fullscreen_exit_output(), "transcript");
		assert_eq!(manager.get_fullscreen_scrollbar(), "auto");
		assert!(manager.get_fullscreen_copy_on_select());
		manager.set_fullscreen_exit_output("resume-hint");
		manager.set_fullscreen_scrollbar("hidden");
		manager.set_fullscreen_copy_on_select(false);
		manager.flush();
		let saved = read_json(&dir.join("agent").join("settings.json"));
		assert_eq!(saved["fullscreenExitOutput"], json!("resume-hint"));
		assert_eq!(saved["fullscreenScrollbar"], json!("hidden"));
		assert_eq!(saved["fullscreenCopyOnSelect"], json!(false));
		write(
			&dir.join("agent").join("settings.json"),
			r#"{"fullscreenExitOutput":"nothing","fullscreenScrollbar":"sometimes"}"#,
		);
		let reloaded = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(reloaded.get_fullscreen_exit_output(), "transcript");
		assert_eq!(reloaded.get_fullscreen_scrollbar(), "auto");
		assert!(reloaded.get_fullscreen_copy_on_select());
	}

	#[test]
	fn persists_fullscreen_wheel_scroll_lines() {
		let dir = scratch("a27");
		let settings_path = dir.join("agent").join("settings.json");
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(manager.get_fullscreen_wheel_scroll_lines(), json!("auto"));
		manager.set_fullscreen_wheel_scroll_lines(WheelScrollLines::Lines(3.0));
		manager.flush();
		assert_eq!(read_json(&settings_path)["fullscreenWheelScrollLines"], json!(3));
		for (value, expected) in [
			(json!(7.9), json!(7)),
			(json!(0), json!(1)),
			(json!(1000), json!(100)),
			(json!("fast"), json!("auto")),
			(json!(null), json!("auto")),
		] {
			write(&settings_path, &format!(r#"{{"fullscreenWheelScrollLines":{}}}"#, value));
			assert_eq!(
				create(&dir.join("project"), &dir.join("agent"), opts()).get_fullscreen_wheel_scroll_lines(),
				expected
			);
		}
	}

	#[test]
	fn output_pad_defaults_to_1_and_persists_binary_values() {
		let dir = scratch("a28");
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(manager.get_output_pad(), 1.0);
		manager.set_output_pad(0.0);
		manager.flush();
		assert_eq!(manager.get_output_pad(), 0.0);
		assert_eq!(read_json(&dir.join("agent").join("settings.json"))["outputPad"], json!(0));
	}

	#[test]
	fn output_pad_treats_unsupported_values_as_default() {
		let dir = scratch("a29");
		write(&dir.join("agent").join("settings.json"), r#"{"outputPad":2}"#);
		assert_eq!(create(&dir.join("project"), &dir.join("agent"), opts()).get_output_pad(), 1.0);
	}

	#[test]
	fn mermaid_defaults_to_streaming_and_persists_modes() {
		let dir = scratch("a30");
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(manager.get_mermaid_rendering_mode(), "streaming");
		manager.set_mermaid_rendering_mode("final");
		manager.flush();
		assert_eq!(manager.get_mermaid_rendering_mode(), "final");
		assert_eq!(read_json(&dir.join("agent").join("settings.json"))["markdown"]["mermaid"], json!("final"));
	}

	#[test]
	fn mermaid_falls_back_to_streaming_for_unsupported_values() {
		let dir = scratch("a31");
		write(&dir.join("agent").join("settings.json"), r#"{"markdown":{"mermaid":"sometimes"}}"#);
		assert_eq!(create(&dir.join("project"), &dir.join("agent"), opts()).get_mermaid_rendering_mode(), "streaming");
	}

	#[test]
	fn loads_shell_command_prefix_from_settings() {
		let dir = scratch("a32");
		write(
			&dir.join("agent").join("settings.json"),
			r#"{"shellCommandPrefix":"shopt -s expand_aliases"}"#,
		);
		assert_eq!(
			create(&dir.join("project"), &dir.join("agent"), opts()).get_shell_command_prefix(),
			Some("shopt -s expand_aliases".to_string())
		);
	}

	#[test]
	fn shell_command_prefix_returns_none_when_not_set() {
		let dir = scratch("a33");
		write(&dir.join("agent").join("settings.json"), r#"{"theme":"dark"}"#);
		assert_eq!(create(&dir.join("project"), &dir.join("agent"), opts()).get_shell_command_prefix(), None);
	}

	#[test]
	fn preserves_shell_command_prefix_when_saving_unrelated_settings() {
		let dir = scratch("a34");
		let settings_path = dir.join("agent").join("settings.json");
		write(&settings_path, r#"{"shellCommandPrefix":"shopt -s expand_aliases"}"#);
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		manager.set_theme("light");
		manager.flush();
		let saved = read_json(&settings_path);
		assert_eq!(saved["shellCommandPrefix"], json!("shopt -s expand_aliases"));
		assert_eq!(saved["theme"], json!("light"));
	}

	#[test]
	fn default_tools_loads_global_and_lets_project_replace() {
		let dir = scratch("a35");
		write(&dir.join("agent").join("settings.json"), r#"{"defaultTools":["read","bash"]}"#);
		assert_eq!(
			create(&dir.join("project"), &dir.join("agent"), opts()).get_default_tools(),
			Some(vec!["read".to_string(), "bash".to_string()])
		);
		write(&dir.join("project").join(".pi").join("settings.json"), r#"{"defaultTools":["grep"]}"#);
		assert_eq!(
			create(&dir.join("project"), &dir.join("agent"), opts()).get_default_tools(),
			Some(vec!["grep".to_string()])
		);
	}

	#[test]
	fn default_tools_preserves_empty_tool_list() {
		assert_eq!(in_mem(json!({"defaultTools": []})).get_default_tools(), Some(Vec::<String>::new()));
		assert_eq!(in_mem(json!({})).get_default_tools(), None);
	}

	#[test]
	fn default_tools_applies_modifiers_to_default_selection() {
		assert_eq!(
			in_mem(json!({"defaultTools": ["+codemode", "-write"]})).get_default_tools(),
			Some(vec!["read".to_string(), "bash".to_string(), "edit".to_string(), "codemode".to_string()])
		);
		assert_eq!(
			in_mem(json!({"defaultTools": ["read", "+grep", "+read"]})).get_default_tools(),
			Some(vec!["read".to_string(), "grep".to_string()])
		);
	}

	#[test]
	fn default_tools_layers_project_modifiers_on_global_selection() {
		let dir = scratch("a36");
		write(
			&dir.join("agent").join("settings.json"),
			r#"{"defaultTools":["read","bash","+codemode"]}"#,
		);
		write(
			&dir.join("project").join(".pi").join("settings.json"),
			r#"{"defaultTools":["-codemode","+tool_search"]}"#,
		);
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(
			manager.get_default_tools(),
			Some(vec!["read".to_string(), "bash".to_string(), "tool_search".to_string()])
		);
		manager.apply_overrides(&json!({"defaultTools": ["+codemode"]}));
		assert_eq!(
			manager.get_default_tools(),
			Some(vec![
				"read".to_string(),
				"bash".to_string(),
				"tool_search".to_string(),
				"codemode".to_string()
			])
		);
	}

	#[test]
	fn default_tools_project_modifiers_on_builtin_defaults() {
		let dir = scratch("a37");
		write(&dir.join("project").join(".pi").join("settings.json"), r#"{"defaultTools":["+codemode"]}"#);
		assert_eq!(
			create(&dir.join("project"), &dir.join("agent"), opts()).get_default_tools(),
			Some(vec![
				"read".to_string(),
				"bash".to_string(),
				"edit".to_string(),
				"write".to_string(),
				"codemode".to_string()
			])
		);
	}

	fn home_opts() -> SettingsManagerOptions {
		SettingsManagerOptions { home_dir: Some("/home/testuser".to_string()), ..opts() }
	}

	#[test]
	fn session_dir_returns_none_when_not_set() {
		let dir = scratch("a38");
		write(&dir.join("agent").join("settings.json"), r#"{"theme":"dark"}"#);
		assert_eq!(create(&dir.join("project"), &dir.join("agent"), home_opts()).get_session_dir(), None);
	}

	#[test]
	fn session_dir_returns_global_value() {
		let dir = scratch("a39");
		write(&dir.join("agent").join("settings.json"), r#"{"sessionDir":"/tmp/sessions"}"#);
		assert_eq!(
			create(&dir.join("project"), &dir.join("agent"), home_opts()).get_session_dir(),
			Some("/tmp/sessions".to_string())
		);
	}

	#[test]
	fn session_dir_project_overrides_global() {
		let dir = scratch("a40");
		write(&dir.join("agent").join("settings.json"), r#"{"sessionDir":"/global/sessions"}"#);
		write(&dir.join("project").join(".pi").join("settings.json"), r#"{"sessionDir":"./sessions"}"#);
		assert_eq!(
			create(&dir.join("project"), &dir.join("agent"), home_opts()).get_session_dir(),
			Some("./sessions".to_string())
		);
	}

	#[test]
	fn session_dir_expands_tilde() {
		let dir = scratch("a41");
		write(&dir.join("agent").join("settings.json"), r#"{"sessionDir":"~/sessions"}"#);
		assert_eq!(
			create(&dir.join("project"), &dir.join("agent"), home_opts()).get_session_dir(),
			Some("/home/testuser/sessions".to_string())
		);
	}

	#[test]
	fn shell_path_returns_none_when_not_set() {
		let dir = scratch("a42");
		write(&dir.join("agent").join("settings.json"), r#"{"theme":"dark"}"#);
		assert_eq!(create(&dir.join("project"), &dir.join("agent"), home_opts()).get_shell_path(), None);
	}

	#[test]
	fn shell_path_absolute_unchanged() {
		let dir = scratch("a43");
		write(&dir.join("agent").join("settings.json"), r#"{"shellPath":"/bin/zsh"}"#);
		assert_eq!(
			create(&dir.join("project"), &dir.join("agent"), home_opts()).get_shell_path(),
			Some("/bin/zsh".to_string())
		);
	}

	#[test]
	fn shell_path_expands_tilde() {
		let dir = scratch("a44");
		write(
			&dir.join("agent").join("settings.json"),
			r#"{"shellPath":"~/.local/bin/agent-shell-sandbox"}"#,
		);
		assert_eq!(
			create(&dir.join("project"), &dir.join("agent"), home_opts()).get_shell_path(),
			Some("/home/testuser/.local/bin/agent-shell-sandbox".to_string())
		);
	}

	#[test]
	fn shell_path_expands_bare_tilde() {
		let dir = scratch("a45");
		write(&dir.join("agent").join("settings.json"), r#"{"shellPath":"~"}"#);
		assert_eq!(
			create(&dir.join("project"), &dir.join("agent"), home_opts()).get_shell_path(),
			Some("/home/testuser".to_string())
		);
	}

	// ---- settings-manager-bug.test.ts ----------------------------------

	#[test]
	fn preserves_file_changes_to_packages_array() {
		let dir = scratch("b1");
		let settings_path = dir.join("agent").join("settings.json");
		write(&settings_path, r#"{"theme":"dark","packages":["npm:pi-mcp-adapter"]}"#);
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		assert_eq!(manager.get_packages(), vec![json!("npm:pi-mcp-adapter")]);
		let mut current = read_json(&settings_path);
		current["packages"] = json!([]);
		write(&settings_path, &js_stringify_pretty(&current));
		manager.set_theme("light");
		manager.flush();
		let saved = read_json(&settings_path);
		assert_eq!(saved["packages"], json!([]));
		assert_eq!(saved["theme"], json!("light"));
	}

	#[test]
	fn preserves_file_changes_to_extensions_array() {
		let dir = scratch("b2");
		let settings_path = dir.join("agent").join("settings.json");
		write(&settings_path, r#"{"theme":"dark","extensions":["/old/extension.ts"]}"#);
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		let mut current = read_json(&settings_path);
		current["extensions"] = json!(["/new/extension.ts"]);
		write(&settings_path, &js_stringify_pretty(&current));
		manager.set_default_thinking_level("high");
		manager.flush();
		assert_eq!(read_json(&settings_path)["extensions"], json!(["/new/extension.ts"]));
	}

	#[test]
	fn preserves_external_project_settings_changes() {
		let dir = scratch("b3");
		let project_path = dir.join("project").join(".pi").join("settings.json");
		write(
			&project_path,
			r#"{"extensions":["./old-extension.ts"],"prompts":["./old-prompt.md"]}"#,
		);
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		let mut current = read_json(&project_path);
		current["prompts"] = json!(["./new-prompt.md"]);
		write(&project_path, &js_stringify_pretty(&current));
		manager
			.set_project_extension_paths(&[json!("./updated-extension.ts")])
			.unwrap();
		manager.flush();
		let saved = read_json(&project_path);
		assert_eq!(saved["prompts"], json!(["./new-prompt.md"]));
		assert_eq!(saved["extensions"], json!(["./updated-extension.ts"]));
	}

	#[test]
	fn in_memory_project_changes_override_external_changes() {
		let dir = scratch("b4");
		let project_path = dir.join("project").join(".pi").join("settings.json");
		write(&project_path, r#"{"extensions":["./initial-extension.ts"]}"#);
		let mut manager = create(&dir.join("project"), &dir.join("agent"), opts());
		let mut current = read_json(&project_path);
		current["extensions"] = json!(["./external-extension.ts"]);
		write(&project_path, &js_stringify_pretty(&current));
		manager
			.set_project_extension_paths(&[json!("./in-memory-extension.ts")])
			.unwrap();
		manager.flush();
		assert_eq!(read_json(&project_path)["extensions"], json!(["./in-memory-extension.ts"]));
	}

	// ---- settings-manager-compaction.test.ts ---------------------------

	fn mem_storage_with(global: &str, project: &str) -> Box<InMemorySettingsStorage> {
		let mut storage = InMemorySettingsStorage::new();
		let g = global.to_string();
		storage
			.with_lock(SettingsScope::Global, &mut |_| Some(g.clone()))
			.unwrap();
		let p = project.to_string();
		storage
			.with_lock(SettingsScope::Project, &mut |_| Some(p.clone()))
			.unwrap();
		Box::new(storage)
	}

	fn from_storage(storage: Box<InMemorySettingsStorage>) -> SettingsManager {
		SettingsManager::from_storage(storage, opts())
	}

	#[test]
	fn compaction_uses_defaults_without_compaction_settings() {
		let manager = in_mem(json!({}));
		assert_eq!(manager.get_compaction_settings(None).unwrap(), defaults_compaction());
		assert_eq!(manager.get_compaction_settings(mref()).unwrap(), defaults_compaction());
	}

	#[test]
	fn compaction_resolves_each_field_independently() {
		let mut manager = in_mem(json!({"compaction": {
			"reserveTokens": 8192,
			"keepRecentTokens": 10000,
			"modelOverrides": { "provider/family/model": { "reserveTokens": 400000 } }
		}}));
		assert_eq!(
			manager.get_compaction_settings(mref()).unwrap(),
			CompactionSettings { enabled: true, reserve_tokens: 400000.0, keep_recent_tokens: 10000.0 }
		);
		assert_eq!(manager.get_compaction_reserve_tokens(mref()).unwrap(), 400000.0);
		assert_eq!(manager.get_compaction_keep_recent_tokens(mref()).unwrap(), 10000.0);
		assert_eq!(
			manager.get_compaction_settings(None).unwrap(),
			CompactionSettings { enabled: true, reserve_tokens: 8192.0, keep_recent_tokens: 10000.0 }
		);
		manager.apply_overrides(&json!({"compaction": {
			"modelOverrides": { "provider/family/model": { "keepRecentTokens": 30000 } }
		}}));
		assert_eq!(manager.get_compaction_keep_recent_tokens(mref()).unwrap(), 30000.0);
		assert_eq!(manager.get_compaction_reserve_tokens(mref()).unwrap(), 400000.0);
	}

	#[test]
	fn compaction_falls_back_to_builtin_defaults_for_missing_fields() {
		let manager = in_mem(json!({"compaction": {
			"modelOverrides": { "provider/family/model": { "keepRecentTokens": 1024 } }
		}}));
		assert_eq!(
			manager.get_compaction_settings(mref()).unwrap(),
			CompactionSettings { enabled: true, reserve_tokens: 16384.0, keep_recent_tokens: 1024.0 }
		);
	}

	#[test]
	fn compaction_matches_exact_provider_model_ids() {
		let manager = in_mem(json!({"compaction": {
			"modelOverrides": {
				"provider/family/model": { "reserveTokens": 400000 },
				"provider/*": { "reserveTokens": 1 },
				"family/model": { "reserveTokens": 2 }
			}
		}}));
		assert_eq!(manager.get_compaction_reserve_tokens(mref()).unwrap(), 400000.0);
		for other in [
			model("other", "family/model"),
			model("provider", "other"),
			model("provider", "family/Model"),
		] {
			assert_eq!(manager.get_compaction_settings(other).unwrap(), defaults_compaction());
		}
	}

	#[test]
	fn compaction_merges_project_model_overrides_per_field() {
		let manager = from_storage(mem_storage_with(
			r#"{"compaction":{"reserveTokens":8192,"modelOverrides":{
				"provider/family/model":{"reserveTokens":400000,"keepRecentTokens":30000},
				"provider/other":{"keepRecentTokens":4096}}}}"#,
			r#"{"compaction":{"reserveTokens":1024,"modelOverrides":{
				"provider/family/model":{"keepRecentTokens":2000}}}}"#,
		));
		assert_eq!(
			manager.get_compaction_settings(mref()).unwrap(),
			CompactionSettings { enabled: true, reserve_tokens: 400000.0, keep_recent_tokens: 2000.0 }
		);
		assert_eq!(
			manager.get_compaction_settings(model("provider", "other")).unwrap(),
			CompactionSettings { enabled: true, reserve_tokens: 1024.0, keep_recent_tokens: 4096.0 }
		);
		let mut manager = manager;
		manager.reload();
		assert_eq!(manager.get_compaction_keep_recent_tokens(mref()).unwrap(), 2000.0);
		manager.set_project_trusted(false);
		assert_eq!(manager.get_compaction_keep_recent_tokens(mref()).unwrap(), 30000.0);
	}

	#[test]
	fn compaction_keeps_enabled_global_when_saving_toggle() {
		let mut manager = from_storage(mem_storage_with(
			r#"{"compaction":{"modelOverrides":{
				"provider/family/model":{"enabled":false,"reserveTokens":400000}}}}"#,
			"{}",
		));
		assert!(manager.get_compaction_settings(mref()).unwrap().enabled);
		manager.set_compaction_enabled(false);
		manager.flush();
		manager.reload();
		assert_eq!(
			manager.get_compaction_settings(mref()).unwrap(),
			CompactionSettings { enabled: false, reserve_tokens: 400000.0, keep_recent_tokens: 20000.0 }
		);
	}

	#[test]
	fn compaction_reports_invalid_model_override_token_values() {
		for field in ["reserveTokens", "keepRecentTokens"] {
			for value in [
				json!(null),
				json!(-1),
				json!(1.5),
				json!("400000"),
				json!(true),
				json!({}),
				json!([]),
				json!(9007199254740992i64),
			] {
				let manager = from_storage(mem_storage_with(
					&format!(r#"{{"compaction":{{"modelOverrides":{{"{}":{{"{}":{}}}}}}}}}"#, MODEL_KEY, field, value),
					"{}",
				));
				let expected = format!(
					"Invalid compaction.modelOverrides[\"{}\"].{} setting: {}. Expected a non-negative safe integer.",
					MODEL_KEY,
					field,
					js_string_value(&value)
				);
				assert_eq!(manager.get_compaction_settings(mref()), Err(expected));
				assert_eq!(manager.get_compaction_settings(None).unwrap(), defaults_compaction());
				assert_eq!(
					manager.get_compaction_settings(model("other", "family/model")).unwrap(),
					defaults_compaction()
				);
			}
		}
	}

	#[test]
	fn compaction_reports_non_finite_runtime_values() {
		// Divergence: serde_json values cannot hold NaN/Infinity, so the TS
		// runtime-only path is exercised through the shared f64 validator
		// (same message construction as the Value path).
		for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
			assert!(!validate_token_f64(value));
			for field in ["reserveTokens", "keepRecentTokens"] {
				let expected = format!(
					"Invalid compaction.modelOverrides[\"{}\"].{} setting: {}. Expected a non-negative safe integer.",
					MODEL_KEY,
					field,
					js_f64_to_string_runtime(value)
				);
				assert!(expected.contains(&js_f64_to_string_runtime(value)));
			}
		}
	}

	#[test]
	fn compaction_reports_invalid_ordinary_values() {
		for field in ["reserveTokens", "keepRecentTokens"] {
			for value in [
				json!(null),
				json!(-1),
				json!(1.5),
				json!("400000"),
				json!(true),
				json!({}),
				json!([]),
				json!(9007199254740992i64),
			] {
				let manager = from_storage(mem_storage_with(
					&format!(
						r#"{{"compaction":{{"{}":{},"modelOverrides":{{"{}":{{"{}":4096}}}}}}}}"#,
						field, value, MODEL_KEY, field
					),
					"{}",
				));
				let expected = format!(
					"Invalid compaction.{} setting: {}. Expected a non-negative safe integer.",
					field,
					js_string_value(&value)
				);
				assert_eq!(manager.get_compaction_settings(None), Err(expected.clone()));
				assert_eq!(manager.get_compaction_settings(mref()), Err(expected));
			}
		}
	}

	#[test]
	fn compaction_reports_non_finite_ordinary_runtime_values() {
		for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
			assert!(!validate_token_f64(value));
		}
	}

	#[test]
	fn compaction_reports_malformed_model_entries() {
		for entry in [json!(null), json!(false), json!(42), json!("invalid"), json!([])] {
			let manager = from_storage(mem_storage_with(
				&format!(r#"{{"compaction":{{"modelOverrides":{{"{}":{}}}}}}}"#, MODEL_KEY, entry),
				"{}",
			));
			assert_eq!(
				manager.get_compaction_settings(mref()),
				Err(format!(
					"Invalid compaction.modelOverrides[\"{}\"] setting: {}. Expected an object.",
					MODEL_KEY,
					js_string_value(&entry)
				))
			);
		}
	}

	#[test]
	fn compaction_accepts_zero_in_ordinary_settings_and_model_overrides() {
		let mut manager = in_mem(json!({"compaction": {"reserveTokens": 0, "keepRecentTokens": 0}}));
		assert_eq!(
			manager.get_compaction_settings(mref()).unwrap(),
			CompactionSettings { enabled: true, reserve_tokens: 0.0, keep_recent_tokens: 0.0 }
		);
		manager.apply_overrides(&json!({"compaction": {
			"reserveTokens": 1000,
			"keepRecentTokens": 1000,
			"modelOverrides": { "provider/family/model": { "reserveTokens": 0, "keepRecentTokens": 0 } }
		}}));
		assert_eq!(
			manager.get_compaction_settings(mref()).unwrap(),
			CompactionSettings { enabled: true, reserve_tokens: 0.0, keep_recent_tokens: 0.0 }
		);
	}

	// ---- helper coverage (JSON.parse messages, stringify parity) --------

	#[test]
	fn parse_json_like_matches_node_error_messages() {
		for (input, expected) in [
			("{bad}", "Expected property name or '}' in JSON at position 1 (line 1 column 2)"),
			("{ invalid json", "Expected property name or '}' in JSON at position 2 (line 1 column 3)"),
			("{\"a\":}", "Unexpected token '}', \"{\"a\":}\" is not valid JSON"),
			(&(String::from_utf8(vec![b'x'; 20]).unwrap() + "}"), "Unexpected token 'x', \"xxxxxxxxxx\"... is not valid JSON"),
			(&(String::from_utf8(vec![b'x'; 19]).unwrap() + "}"), "Unexpected token 'x', \"xxxxxxxxxxxxxxxxxxx}\" is not valid JSON"),
			("{\"a\":1,}", "Expected double-quoted property name in JSON at position 7 (line 1 column 8)"),
			("[1,2", "Expected ',' or ']' after array element in JSON at position 4 (line 1 column 5)"),
			("{\"a\":1} garbage", "Unexpected non-whitespace character after JSON at position 8 (line 1 column 9)"),
			("not json", "Unexpected token 'o', \"not json\" is not valid JSON"),
			("", "Unexpected end of JSON input"),
			("[", "Unexpected end of JSON input"),
			("{\"a\":", "Unexpected end of JSON input"),
			("{\"a\"", "Expected ':' after property name in JSON at position 4 (line 1 column 5)"),
			("{\"a\":1", "Expected ',' or '}' after property value in JSON at position 6 (line 1 column 7)"),
			("-", "No number after minus sign in JSON at position 1 (line 1 column 2)"),
			("01", "Unexpected number in JSON at position 1 (line 1 column 2)"),
			("1.", "Unterminated fractional number in JSON at position 2 (line 1 column 3)"),
			("1e", "Exponent part is missing a number in JSON at position 2 (line 1 column 3)"),
			("\"abc", "Unterminated string in JSON at position 4 (line 1 column 5)"),
			("{\"a\":1}\n\njunk", "Unexpected non-whitespace character after JSON at position 9 (line 3 column 1)"),
		] {
			assert_eq!(parse_json_like(input).unwrap_err(), expected, "input: {:?}", input);
		}
	}

	#[test]
	fn stringify_pretty_matches_json_stringify() {
		let value = parse_json_like(
			r#"{"a":1,"b":"x","c":[1,2],"d":{"e":true},"f":[],"g":{},"h":1.5,"i":null}"#,
		)
		.unwrap();
		let expected = "{\n  \"a\": 1,\n  \"b\": \"x\",\n  \"c\": [\n    1,\n    2\n  ],\n  \"d\": {\n    \"e\": true\n  },\n  \"f\": [],\n  \"g\": {},\n  \"h\": 1.5,\n  \"i\": null\n}";
		assert_eq!(js_stringify_pretty(&value), expected);
	}
}
