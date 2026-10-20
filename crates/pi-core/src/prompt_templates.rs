//! Prompt templates loaded from markdown files.
//!
//! Port of `packages/coding-agent/src/core/prompt-templates.ts`.
//!
//! # JS-semantics traps honored here
//!
//! - **No ambient state**: all paths (`cwd`, `agent_dir`, `prompt_paths`) and
//!   the config dir name (TS reads `pkg.piConfig.configDir` at import time;
//!   default `".pi"`) are injected. `~` expansion in prompt paths needs
//!   `home_dir` injected; when `None`, `crate::paths` falls back to `$HOME`
//!   (only reachable for `~`-prefixed inputs). Relative `cwd`/`agent_dir`
//!   resolve against `"/"` (TS would use `process.cwd()`).
//! - **Slicing by UTF-16 code units**: the 60-char description truncation
//!   uses JS `.slice(0, 60)` / `.length` semantics.
//! - **`undefined` omitted, not `null`**: `argumentHint`, `baseDir` and the
//!   diagnostic `path` are `Option` and skipped during serialization.
//!
//! # Parity notes (for the harness wave)
//!
//! - `SourceInfo` / `ResourceDiagnostic` are local minimal ports of
//!   `core/source-info.ts` / `core/diagnostics.ts` (not yet ported). When
//!   those modules land, unify the types here with them.
//! - Frontmatter is parsed with a YAML subset (top-level `key: value` scalar
//!   entries, quoted scalars with `'…'`/`"…"` escapes, comment stripping,
//!   null/bool/number detection). The three error shapes the `yaml` package
//!   (eemeli/yaml 2.9.0) produces for realistic malformed frontmatter are
//!   reproduced **byte-exactly**, including the `prettifyError` excerpt and
//!   caret: unterminated quote, nested mapping (`a: b: c`), duplicate keys.
//!   Known divergences: multi-line quoted scalars join with `\n` instead of
//!   YAML folding, block scalars (`>`/`|`), nested maps as values, flow
//!   collections as documents, and error positions/line excerpts are counted
//!   in chars (not UTF-16 units) are not supported.
//! - `readdir` iteration order is the OS order (like `readdirSync`); the
//!   parity harness must not depend on directory order.
//! - fs error diagnostics use Node-style messages
//!   (`ENOENT: no such file or directory, open '<path>'`), mirroring
//!   `pi_tools::node_fs` (kept private here because pi-core is below
//!   pi-tools in the dependency graph).

use serde::{Deserialize, Serialize};

use crate::js_string::{is_js_whitespace_or_terminator, js_length};
use crate::paths::{resolve_path, PathInputOptions};
use crate::text::strip_bom;

/// `SourceScope` from `core/source-info.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceScope {
	User,
	Project,
	Temporary,
}

/// `SourceOrigin` from `core/source-info.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceOrigin {
	Package,
	TopLevel,
}

/// `SourceInfo` from `core/source-info.ts` (minimal local port).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
	pub path: String,
	pub source: String,
	pub scope: SourceScope,
	pub origin: SourceOrigin,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub base_dir: Option<String>,
}

/// The `type` field of `ResourceDiagnostic` from `core/diagnostics.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticType {
	Warning,
	Error,
	Collision,
}

/// `ResourceCollision` from `core/diagnostics.ts` (minimal local port).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceCollision {
	pub resource_type: String,
	pub name: String,
	pub winner_path: String,
	pub loser_path: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub winner_source: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub loser_source: Option<String>,
}

/// `ResourceDiagnostic` from `core/diagnostics.ts` (minimal local port; this
/// module only ever produces `warning` diagnostics with a path, while the
/// skills module also produces `collision` diagnostics).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceDiagnostic {
	#[serde(rename = "type")]
	pub r#type: DiagnosticType,
	pub message: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub path: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub collision: Option<ResourceCollision>,
}

impl ResourceDiagnostic {
	pub(crate) fn warning(message: String, path: Option<String>) -> Self {
		Self {
			r#type: DiagnosticType::Warning,
			message,
			path,
			collision: None,
		}
	}
}

/// A prompt template loaded from a markdown file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTemplate {
	pub name: String,
	pub description: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub argument_hint: Option<String>,
	pub content: String,
	pub source_info: SourceInfo,
	/// Absolute path to the template file.
	pub file_path: String,
}

/// Options for [`load_prompt_templates`].
#[derive(Debug, Clone)]
pub struct LoadPromptTemplatesOptions {
	/// Working directory for project-local templates.
	pub cwd: String,
	/// Agent config directory for global templates.
	pub agent_dir: String,
	/// Explicit prompt template paths (files or directories).
	pub prompt_paths: Vec<String>,
	/// Include default prompt directories.
	pub include_defaults: bool,
	/// `CONFIG_DIR_NAME` from `config.ts` (TS reads it from package.json at
	/// import time; the default distribution value is `".pi"`).
	pub config_dir_name: String,
	/// Injected home directory used for `~` expansion in prompt paths. When
	/// `None`, `crate::paths` falls back to `$HOME` (see module docs).
	pub home_dir: Option<String>,
}

impl Default for LoadPromptTemplatesOptions {
	fn default() -> Self {
		Self {
			cwd: ".".to_string(),
			agent_dir: ".".to_string(),
			prompt_paths: Vec::new(),
			include_defaults: false,
			config_dir_name: ".pi".to_string(),
			home_dir: None,
		}
	}
}

/// Result of [`load_prompt_templates`].
#[derive(Debug, Clone, Default)]
pub struct LoadPromptTemplatesResult {
	pub templates: Vec<PromptTemplate>,
	pub diagnostics: Vec<ResourceDiagnostic>,
}

/// Parse command arguments respecting quoted strings (bash-style).
/// Returns an array of arguments.
///
/// Mirrors TS `parseCommandArgs`: quoted empty strings (`""` / `''`) are kept
/// as arguments, quotes are stripped, and JS `\s` (including U+00A0 and
/// U+FEFF) separates tokens.
pub fn parse_command_args(args_string: &str) -> Vec<String> {
	let mut args: Vec<String> = Vec::new();
	let mut current = String::new();
	// Tracks whether a token has started, so quoted empty strings are kept.
	let mut has_token = false;
	let mut in_quote: Option<char> = None;

	for char in args_string.chars() {
		if let Some(quote) = in_quote {
			if char == quote {
				in_quote = None;
			} else {
				current.push(char);
			}
		} else if char == '"' || char == '\'' {
			in_quote = Some(char);
			has_token = true;
		} else if is_js_whitespace_or_terminator(char) {
			if has_token {
				args.push(std::mem::take(&mut current));
				has_token = false;
			}
		} else {
			current.push(char);
			has_token = true;
		}
	}

	if has_token {
		args.push(current);
	}

	args
}

/// Consume a maximal run of ASCII digits starting at `start`.
/// Returns `(parseInt(runes), end_index)`; `None` when there is no digit.
/// Digit runs that overflow `u64` saturate (JS `parseInt` yields a huge
/// double that never indexes an existing argument).
fn digit_run(chars: &[char], start: usize) -> Option<(u64, usize)> {
	let mut end = start;
	while end < chars.len() && chars[end].is_ascii_digit() {
		end += 1;
	}
	if end == start {
		return None;
	}
	let run: String = chars[start..end].iter().collect();
	let n = run.parse::<u64>().unwrap_or(u64::MAX);
	Some((n, end))
}

/// Whether `chars[at..]` starts with `prefix` (bounds-safe).
fn starts_with_str(chars: &[char], at: usize, prefix: &str) -> bool {
	prefix
		.chars()
		.enumerate()
		.all(|(k, c)| chars.get(at + k) == Some(&c))
}

/// Alternative 1 of the TS substitution regex:
/// `\$\{(\d+|ARGUMENTS|@):-([^}]*)\}` — positional/all-args value with a
/// default used when the value is missing or empty (JS truthiness).
fn match_default_form(
	chars: &[char],
	i: usize,
	args: &[String],
	all_args: &str,
) -> Option<(usize, String)> {
	if !matches!(chars.get(i + 1), Some('{')) {
		return None;
	}
	let j = i + 2;
	// group1: \d+ | ARGUMENTS | @ , each followed by ":-"
	let (value, k) = if matches!(chars.get(j), Some(c) if c.is_ascii_digit()) {
		let (n, after) = digit_run(chars, j)?;
		if !starts_with_str(chars, after, ":-") {
			return None;
		}
		let v = if n >= 1 {
			usize::try_from(n).ok().and_then(|idx| args.get(idx - 1)).cloned()
		} else {
			// parseInt("0") - 1 === -1 → args[-1] → undefined
			None
		};
		(v, after + 2)
	} else if starts_with_str(chars, j, "ARGUMENTS") && starts_with_str(chars, j + 9, ":-") {
		(Some(all_args.to_string()), j + 11)
	} else if matches!(chars.get(j), Some('@')) && starts_with_str(chars, j + 1, ":-") {
		(Some(all_args.to_string()), j + 3)
	} else {
		return None;
	};
	// ([^}]*)\} — default runs to the first '}' (which must exist)
	let brace = k + chars[k..].iter().position(|&c| c == '}')?;
	let default_value: String = chars[k..brace].iter().collect();
	let replacement = match value {
		Some(v) if !v.is_empty() => v,
		_ => default_value,
	};
	Some((brace + 1, replacement))
}

/// Alternative 2 of the TS substitution regex: `\$\{@:(\d+)(?::(\d+))?\}` —
/// bash-style slicing of the argument list (1-indexed, `0` treated as `1`).
fn match_slice_form(chars: &[char], i: usize, args: &[String]) -> Option<(usize, String)> {
	if !matches!(chars.get(i + 1), Some('{'))
		|| !matches!(chars.get(i + 2), Some('@'))
		|| !matches!(chars.get(i + 3), Some(':'))
	{
		return None;
	}
	let (n, after_digits) = digit_run(chars, i + 4)?;
	let (mut length, mut end) = (None, after_digits);
	if matches!(chars.get(after_digits), Some(':')) {
		if let Some((m, e2)) = digit_run(chars, after_digits + 1) {
			length = Some(m);
			end = e2;
		}
	}
	if !matches!(chars.get(end), Some('}')) {
		return None;
	}
	// 0-indexed start; "0" is treated as "1" (bash convention)
	let start: usize = if n == 0 {
		0
	} else {
		usize::try_from(n - 1).unwrap_or(usize::MAX)
	};
	let len = args.len();
	let slice: Vec<&str> = if start >= len {
		Vec::new()
	} else {
		let end_idx = match length {
			Some(l) => start.saturating_add(usize::try_from(l).unwrap_or(usize::MAX)).min(len),
			None => len,
		};
		args[start..end_idx].iter().map(|s| s.as_str()).collect()
	};
	Some((end + 1, slice.join(" ")))
}

/// Alternative 3 of the TS substitution regex: `\$(ARGUMENTS|@|\d+)`.
fn match_simple_form(
	chars: &[char],
	i: usize,
	args: &[String],
	all_args: &str,
) -> Option<(usize, String)> {
	let j = i + 1;
	if matches!(chars.get(j), Some('@')) {
		return Some((j + 1, all_args.to_string()));
	}
	if starts_with_str(chars, j, "ARGUMENTS") {
		return Some((j + 9, all_args.to_string()));
	}
	if let Some((n, end)) = digit_run(chars, j) {
		let replacement = if n >= 1 {
			usize::try_from(n)
				.ok()
				.and_then(|idx| args.get(idx - 1))
				.cloned()
				.unwrap_or_default()
		} else {
			String::new()
		};
		return Some((end, replacement));
	}
	None
}

/// Substitute argument placeholders in template content.
///
/// Hand-rolled scan of the TS regex
/// `/\$\{(\d+|ARGUMENTS|@):-([^}]*)\}|\$\{@:(\d+)(?::(\d+))?\}|\$(ARGUMENTS|@|\d+)/g`
/// (left-to-right, non-overlapping; the crate has no regex dependency).
/// Supports:
/// - `$1`, `$2`, ... for positional args
/// - `$@` and `$ARGUMENTS` for all args
/// - `${N:-default}` for positional arg N with a default when missing/empty
/// - `${@:-default}` and `${ARGUMENTS:-default}` for all args with a default
///   when empty
/// - `${@:N}` for args from Nth onwards (bash-style slicing)
/// - `${@:N:L}` for L args starting from Nth
///
/// Replacement happens on the template string only. Argument and default
/// values containing patterns like `$1`, `$@`, or `$ARGUMENTS` are NOT
/// recursively substituted.
pub fn substitute_args(content: &str, args: &[String]) -> String {
	let all_args = args.join(" ");
	let chars: Vec<char> = content.chars().collect();
	let mut out = String::new();
	let mut i = 0usize;
	while i < chars.len() {
		if chars[i] != '$' {
			out.push(chars[i]);
			i += 1;
			continue;
		}
		let matched = match_default_form(&chars, i, args, &all_args)
			.or_else(|| match_slice_form(&chars, i, args))
			.or_else(|| match_simple_form(&chars, i, args, &all_args));
		if let Some((next, replacement)) = matched {
			out.push_str(&replacement);
			i = next;
		} else {
			out.push('$');
			i += 1;
		}
	}
	out
}

// ---------------------------------------------------------------------------
// Frontmatter (subset of utils/frontmatter.ts + the `yaml` package)
// ---------------------------------------------------------------------------

/// A parsed frontmatter value. `Other` covers everything the TS code treats
/// as "not a string" (null, numbers, sequences, mappings); `Bool` keeps the
/// raw truth value for `disable-model-invocation === true` in skills.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FrontmatterValue {
	Str(String),
	Bool(bool),
	Other,
}

/// JS `String.prototype.trim()` (JS whitespace, incl. U+00A0 / U+FEFF).
pub(crate) fn js_trim(s: &str) -> &str {
	s.trim_matches(is_js_whitespace_or_terminator)
}

/// JS newline normalization: `\r\n` → `\n`, then lone `\r` → `\n`.
fn normalize_newlines(s: &str) -> String {
	s.replace("\r\n", "\n").replace('\r', "\n")
}

/// `extractFrontmatter` + `parseFrontmatter` from `utils/frontmatter.ts`,
/// with a YAML-subset parser standing in for eemeli/yaml (see module docs).
/// `Err` carries the yaml error message (reproduced byte-exactly for the
/// shapes the subset parser detects).
pub(crate) fn parse_frontmatter(content: &str) -> Result<(Vec<(String, FrontmatterValue)>, String), String> {
	let normalized = normalize_newlines(strip_bom(content));
	if !normalized.starts_with("---") {
		return Ok((Vec::new(), normalized));
	}
	// indexOf("\n---", 3)
	let end_index = match normalized[3..].find("\n---") {
		Some(offset) => offset + 3,
		None => return Ok((Vec::new(), normalized)),
	};
	// slice(4, endIndex): empty (falsy in TS) when endIndex < 4
	let yaml_string = if end_index < 4 {
		""
	} else {
		&normalized[4..end_index]
	};
	let body = js_trim(&normalized[end_index + 4..]).to_string();
	if yaml_string.is_empty() {
		return Ok((Vec::new(), body));
	}
	let entries = parse_yaml_subset(yaml_string)?;
	Ok((entries, body))
}

/// Whether a plain scalar resolves to a non-string (null/bool/number) under
/// the YAML 1.2 core schema (approximation; see module docs).
fn is_yaml_non_string_scalar(s: &str) -> bool {
	matches!(
		s,
		"null" | "Null" | "NULL" | "~" | "true" | "True" | "TRUE" | "false" | "False" | "FALSE"
	) || is_yaml_number(s)
}

fn is_yaml_number(s: &str) -> bool {
	if matches!(
		s,
		".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN"
	) {
		return true;
	}
	let signed = s.strip_prefix(['-', '+']).unwrap_or(s);
	if matches!(
		signed,
		".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN"
	) {
		return true;
	}
	if let Some(hex) = signed.strip_prefix("0x") {
		return !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit());
	}
	if let Some(oct) = signed.strip_prefix("0o") {
		return !oct.is_empty() && oct.chars().all(|c| ('0'..='7').contains(&c));
	}
	let mut seen_digit = false;
	let mut seen_dot = false;
	let mut rest = signed.chars().peekable();
	let mut in_exponent = false;
	while let Some(&c) = rest.peek() {
		match c {
			'0'..='9' => {
				seen_digit = true;
				rest.next();
			}
			'.' if !seen_dot && !in_exponent => {
				seen_dot = true;
				rest.next();
			}
			'e' | 'E' if seen_digit && !in_exponent => {
				in_exponent = true;
				rest.next();
				if matches!(rest.peek(), Some('+' | '-')) {
					rest.next();
				}
			}
			_ => break,
		}
	}
	seen_digit && rest.peek().is_none()
}

/// Reproduce eemeli/yaml 2.9.0 `prettifyError` message formatting:
/// `"<message> at line L, column C:\n\n<excerpt>\n<caret>\n"`, including the
/// 80-char line trimming, the previous-line context when the caret points at
/// a line start, and (for this module's error shapes) a single `^`.
fn yaml_error(src: &str, message: &str, line: usize, col: usize) -> String {
	let lines: Vec<&str> = src.split('\n').collect();
	let mut msg = format!("{} at line {}, column {}", message, line, col);
	let mut ci = col.saturating_sub(1);
	let mut line_str: String = lines
		.get(line - 1)
		.copied()
		.unwrap_or("")
		.trim_end_matches(['\n', '\r'])
		.to_string();
	let line_len = line_str.chars().count();
	// Trim to max 80 chars, keeping col position near the middle
	if ci >= 60 && line_len > 80 {
		let trim_start = (ci - 39).min(line_len - 79);
		let chars: Vec<char> = line_str.chars().collect();
		line_str = format!("\u{2026}{}", chars[trim_start..].iter().collect::<String>());
		ci -= trim_start - 1;
	}
	if line_str.chars().count() > 80 {
		let chars: Vec<char> = line_str.chars().collect();
		line_str = format!("{}\u{2026}", chars[..79].iter().collect::<String>());
	}
	// Include previous line in context if pointing at line start
	if line > 1 && line_str.chars().take(ci).all(|c| c == ' ') {
		let prev_raw = lines.get(line - 2).copied().unwrap_or("");
		let prev = if prev_raw.chars().count() > 80 {
			let chars: Vec<char> = prev_raw.chars().collect();
			format!("{}\u{2026}\n", chars[..79].iter().collect::<String>())
		} else {
			format!("{}\n", prev_raw)
		};
		line_str = format!("{}{}", prev, line_str);
	}
	if line_str.chars().any(|c| c != ' ') {
		let pointer = format!("{}^", " ".repeat(ci));
		msg.push_str(&format!(":\n\n{}\n{}\n", line_str, pointer));
	}
	msg
}

/// `Missing closing "quote` / `Missing closing 'quote` — reported at the end
/// of the frontmatter source, like the yaml package scanning to EOF.
fn unterminated_quote_error(src: &str, quote: char) -> String {
	let lines: Vec<&str> = src.split('\n').collect();
	let last = lines.last().copied().unwrap_or("");
	let line = lines.len();
	let col = last.chars().count() + 1;
	yaml_error(src, &format!("Missing closing {}quote", quote), line, col)
}

/// Scan a quoted scalar that opens at `lines[i]` (in `opening_line`, which
/// starts with the quote character). Handles `''` (single) and `\"`/`\\`
/// (double) escapes and continuation onto following lines. Returns the
/// unescaped text and the line index the scalar ends on. `Err(())` when the
/// quote is never closed.
fn scan_quoted(
	lines: &[&str],
	i: usize,
	opening_line: &str,
	quote: char,
) -> Result<(String, usize), ()> {
	let mut text = String::new();
	let mut chars = opening_line.chars();
	// consume the opening quote
	chars.next();
	let mut line_idx = i;
	let mut rest: Vec<char> = chars.collect();
	loop {
		let mut k = 0usize;
		while k < rest.len() {
			let c = rest[k];
			if quote == '"' && c == '\\' && k + 1 < rest.len() {
				match rest[k + 1] {
					'"' => text.push('"'),
					'\\' => text.push('\\'),
					'n' => text.push('\n'),
					't' => text.push('\t'),
					'r' => text.push('\r'),
					other => {
						text.push('\\');
						text.push(other);
					}
				}
				k += 2;
				continue;
			}
			if quote == '\'' && c == '\'' && k + 1 < rest.len() && rest[k + 1] == '\'' {
				text.push('\'');
				k += 2;
				continue;
			}
			if c == quote {
				// closing quote found; trailing content on this line is
				// ignored (yaml errors on non-empty trailing content)
				return Ok((text, line_idx));
			}
			text.push(c);
			k += 1;
		}
		// end of line: the scalar continues on the next line (yaml folds the
		// newline to a space; we join with '\n' — see module docs)
		line_idx += 1;
		if line_idx >= lines.len() {
			return Err(());
		}
		text.push('\n');
		rest = lines[line_idx].chars().collect();
	}
}

/// Parse the top-level `key: value` entries of a frontmatter block. See the
/// module docs for the supported subset and the byte-exact error shapes.
fn parse_yaml_subset(src: &str) -> Result<Vec<(String, FrontmatterValue)>, String> {
	let lines: Vec<&str> = src.split('\n').collect();
	let mut entries: Vec<(String, FrontmatterValue)> = Vec::new();
	let mut i = 0usize;
	while i < lines.len() {
		let raw = lines[i];
		let line_no = i + 1;
		let trimmed = raw.trim();
		if trimmed.is_empty()
			|| trimmed.starts_with('#')
			|| trimmed.starts_with("- ")
			|| trimmed == "-"
		{
			i += 1;
			continue;
		}
		if raw.starts_with([' ', '\t']) {
			// nested content: belongs to the previous key (or a folded
			// multi-line scalar) — not a top-level entry
			i += 1;
			continue;
		}
		// a key separator is a ':' followed by a space or end-of-line
		let colon = raw
			.char_indices()
			.find(|(idx, c)| *c == ':' && raw[*idx + 1..].starts_with([' ', '\t']) || (*c == ':' && *idx + 1 == raw.len()))
			.map(|(idx, _)| idx);
		let Some(colon) = colon else {
			// plain scalar line (no top-level key)
			i += 1;
			continue;
		};
		let key = raw[..colon].trim_end();
		if key.is_empty() || key.starts_with(['#', '[', '{', '&', '*', '!', '|', '>', '\'', '"']) {
			i += 1;
			continue;
		}
		let value_part = &raw[colon + 1..];
		// 1-based column of the first non-space char after the key colon
		let value_start_col = colon + 2 + value_part.chars().take_while(|c| *c == ' ' || *c == '\t').count();
		let after_trim = value_part.trim_start_matches([' ', '\t']);

		let value = if matches!(after_trim.chars().next(), Some('"') | Some('\'')) {
			let quote = after_trim.chars().next().unwrap();
			match scan_quoted(&lines, i, after_trim, quote) {
				Ok((text, end_line)) => {
					i = end_line;
					FrontmatterValue::Str(text)
				}
				Err(()) => return Err(unterminated_quote_error(src, quote)),
			}
		} else {
			// plain scalar: strip a trailing " #" comment
			let no_comment = match after_trim.find(" #") {
				Some(pos) => &after_trim[..pos],
				None => after_trim,
			};
			let no_comment = if no_comment.starts_with('#') { "" } else { no_comment };
			let trimmed_value = js_trim(no_comment);
			if trimmed_value.is_empty() {
				FrontmatterValue::Other // null
			} else if trimmed_value.contains(": ") || trimmed_value.ends_with(':') {
				return Err(yaml_error(
					src,
					"Nested mappings are not allowed in compact mappings",
					line_no,
					value_start_col,
				));
			} else if is_yaml_non_string_scalar(trimmed_value) {
				match trimmed_value {
					"true" | "True" | "TRUE" => FrontmatterValue::Bool(true),
					"false" | "False" | "FALSE" => FrontmatterValue::Bool(false),
					_ => FrontmatterValue::Other,
				}
			} else {
				FrontmatterValue::Str(trimmed_value.to_string())
			}
		};

		if entries.iter().any(|(k, _)| k == key) {
			return Err(yaml_error(src, "Map keys must be unique", line_no, 1));
		}
		entries.push((key.to_string(), value));
		i += 1;
	}
	Ok(entries)
}

pub(crate) fn frontmatter_get_str<'a>(
	entries: &'a [(String, FrontmatterValue)],
	key: &str,
) -> Option<&'a str> {
	entries
		.iter()
		.find(|(k, _)| k == key)
		.and_then(|(_, v)| match v {
			FrontmatterValue::Str(s) => Some(s.as_str()),
			FrontmatterValue::Bool(_) | FrontmatterValue::Other => None,
		})
}

/// JS `str.slice(0, max_units)` by UTF-16 code units (never splits a
/// surrogate pair — see module docs).
fn utf16_prefix(s: &str, max_units: usize) -> String {
	let mut out = String::new();
	let mut units = 0usize;
	for c in s.chars() {
		let len = c.len_utf16();
		if units + len > max_units {
			break;
		}
		out.push(c);
		units += len;
	}
	out
}

// ---------------------------------------------------------------------------
// Filesystem loading
// ---------------------------------------------------------------------------

/// Node `path.posix.basename` (trailing separators ignored).
pub(crate) fn basename(path: &str) -> &str {
	path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// `basename(filePath).replace(/\.md$/, "")` — strip one trailing `.md`.
fn template_name(file_path: &str) -> String {
	basename(file_path).strip_suffix(".md").unwrap_or(basename(file_path)).to_string()
}

/// Node `path.dirname` (posix).
pub(crate) fn node_dirname(path: &str) -> String {
	let trimmed = path.trim_end_matches('/');
	match trimmed.rfind('/') {
		Some(0) => "/".to_string(),
		Some(idx) => trimmed[..idx].to_string(),
		None => ".".to_string(),
	}
}

/// Node `path.join` for two unix segments (mirrors the private helper in
/// `crate::paths`).
pub(crate) fn node_path_join(a: &str, b: &str) -> String {
	if a.is_empty() {
		return b.to_string();
	}
	if b.is_empty() {
		return a.to_string();
	}
	format!("{}/{}", a.trim_end_matches('/'), b.trim_start_matches('/'))
}

/// Node error code for an io error (mirrors `pi_tools::node_fs::node_code`).
pub(crate) fn node_code(err: &std::io::Error) -> &'static str {
	match err.kind() {
		std::io::ErrorKind::NotFound => "ENOENT",
		std::io::ErrorKind::PermissionDenied => "EACCES",
		std::io::ErrorKind::AlreadyExists => "EEXIST",
		std::io::ErrorKind::InvalidInput => "EINVAL",
		_ => match err.raw_os_error() {
			Some(21) => "EISDIR",
			Some(20) => "ENOTDIR",
			_ => "UNKNOWN",
		},
	}
}

/// Full Node-style error message, e.g.
/// `ENOENT: no such file or directory, open '/tmp/x'` (mirrors
/// `pi_tools::node_fs::node_error_message`).
pub(crate) fn node_error_message(syscall: &str, path: &str, err: &std::io::Error) -> String {
	let code = node_code(err);
	let reason = match code {
		"ENOENT" => "no such file or directory",
		"EACCES" => "permission denied",
		"EEXIST" => "file already exists",
		"EISDIR" => "illegal operation on a directory",
		"ENOTDIR" => "not a directory",
		"EINVAL" => "invalid argument",
		_ => "operation not permitted",
	};
	format!("{}: {}, {} '{}'", code, reason, syscall, path)
}

fn load_template_from_file(
	file_path: &str,
	source_info: SourceInfo,
) -> (Option<PromptTemplate>, Vec<ResourceDiagnostic>) {
	let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();
	let raw_content = match std::fs::read(file_path) {
		Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
		Err(err) => {
			diagnostics.push(ResourceDiagnostic::warning(
				node_error_message("open", file_path, &err),
				Some(file_path.to_string()),
			));
			return (None, diagnostics);
		}
	};

	let (frontmatter, body) = match parse_frontmatter(&raw_content) {
		Ok(parsed) => parsed,
		Err(message) => {
			diagnostics.push(ResourceDiagnostic::warning(message, Some(file_path.to_string())));
			return (None, diagnostics);
		}
	};

	let name = template_name(file_path);

	// Get description from frontmatter or first non-empty line
	let mut description = frontmatter_get_str(&frontmatter, "description")
		.map(|s| s.to_string())
		.unwrap_or_default();
	if description.is_empty() {
		if let Some(first_line) = body.split('\n').find(|line| !js_trim(line).is_empty()) {
			// Truncate if too long (UTF-16 code units)
			description = utf16_prefix(first_line, 60);
			if js_length(first_line) > 60 {
				description.push_str("...");
			}
		}
	}

	// `...(argumentHint && { argumentHint })` — omitted when empty
	let argument_hint = frontmatter_get_str(&frontmatter, "argument-hint")
		.filter(|s| !s.is_empty())
		.map(|s| s.to_string());

	(
		Some(PromptTemplate {
			name,
			description,
			argument_hint,
			content: body,
			source_info,
			file_path: file_path.to_string(),
		}),
		diagnostics,
	)
}

/// Scan a directory for `.md` files (non-recursive) and load them as prompt
/// templates. Missing directory → empty result; `readdir` failure → partial
/// result (both mirror the TS try/catch). Iteration order is OS order.
fn load_templates_from_dir(
	dir: &str,
	get_source_info: &dyn Fn(&str) -> SourceInfo,
) -> LoadPromptTemplatesResult {
	let mut templates: Vec<PromptTemplate> = Vec::new();
	let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

	if std::fs::metadata(dir).is_err() {
		return LoadPromptTemplatesResult {
			templates,
			diagnostics,
		};
	}

	let entries = match std::fs::read_dir(dir) {
		Ok(entries) => entries,
		Err(_) => {
			return LoadPromptTemplatesResult {
				templates,
				diagnostics,
			}
		}
	};

	for entry in entries {
		let entry = match entry {
			Ok(entry) => entry,
			Err(_) => {
				return LoadPromptTemplatesResult {
					templates,
					diagnostics,
				}
			}
		};
		let name = entry.file_name().to_string_lossy().into_owned();
		let full_path = node_path_join(dir, &name);

		let file_type = match entry.file_type() {
			Ok(file_type) => file_type,
			Err(_) => continue,
		};
		// For symlinks, check if they point to a file
		let mut is_file = file_type.is_file();
		if file_type.is_symlink() {
			match std::fs::metadata(&full_path) {
				Ok(stats) => is_file = stats.is_file(),
				// Broken symlink, skip it
				Err(_) => continue,
			}
		}

		if is_file && name.ends_with(".md") {
			let (template, result_diagnostics) =
				load_template_from_file(&full_path, get_source_info(&full_path));
			if let Some(template) = template {
				templates.push(template);
			}
			diagnostics.extend(result_diagnostics);
		}
	}

	LoadPromptTemplatesResult {
		templates,
		diagnostics,
	}
}

fn add_result(
	templates: &mut Vec<PromptTemplate>,
	diagnostics: &mut Vec<ResourceDiagnostic>,
	result: LoadPromptTemplatesResult,
) {
	templates.extend(result.templates);
	diagnostics.extend(result.diagnostics);
}

/// Load all prompt templates from:
/// 1. Global: `agentDir/prompts/`
/// 2. Project: `cwd/{CONFIG_DIR_NAME}/prompts/`
/// 3. Explicit prompt paths
pub fn load_prompt_templates(options: &LoadPromptTemplatesOptions) -> LoadPromptTemplatesResult {
	let base = PathInputOptions {
		home_dir: options.home_dir.clone(),
		..PathInputOptions::default()
	};
	// TS `resolvePath(options.cwd)` / `resolvePath(options.agentDir)` resolve
	// relative inputs against process.cwd(); we resolve against "/" (see
	// module docs — callers pass absolute paths).
	let resolved_cwd = resolve_path(&options.cwd, ".", &base);
	let resolved_agent_dir = resolve_path(&options.agent_dir, ".", &base);

	let mut templates: Vec<PromptTemplate> = Vec::new();
	let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

	let global_prompts_dir = node_path_join(&resolved_agent_dir, "prompts");
	// TS: resolve(resolvedCwd, CONFIG_DIR_NAME, "prompts")
	let project_prompts_dir = resolve_path(
		&format!("{}/{}/prompts", resolved_cwd, options.config_dir_name),
		".",
		&base,
	);

	let get_source_info = |resolved_path: &str| -> SourceInfo {
		// TS isUnderPath compares against resolve(root); both roots here are
		// already absolute and normalized.
		if is_under_path(resolved_path, &global_prompts_dir) {
			return create_synthetic_source_info(
				resolved_path,
				Some(SourceScope::User),
				Some(global_prompts_dir.clone()),
			);
		}
		if is_under_path(resolved_path, &project_prompts_dir) {
			return create_synthetic_source_info(
				resolved_path,
				Some(SourceScope::Project),
				Some(project_prompts_dir.clone()),
			);
		}
		let base_dir = if std::fs::metadata(resolved_path)
			.map(|stats| stats.is_dir())
			.unwrap_or(false)
		{
			resolved_path.to_string()
		} else {
			node_dirname(resolved_path)
		};
		create_synthetic_source_info(resolved_path, None, Some(base_dir))
	};

	if options.include_defaults {
		add_result(
			&mut templates,
			&mut diagnostics,
			load_templates_from_dir(&global_prompts_dir, &get_source_info),
		);
		add_result(
			&mut templates,
			&mut diagnostics,
			load_templates_from_dir(&project_prompts_dir, &get_source_info),
		);
	}

	// 3. Load explicit prompt paths
	for raw_path in &options.prompt_paths {
		// TS: resolvePath(rawPath, resolvedCwd, { trim: true })
		let resolved_path = resolve_path(
			raw_path,
			&resolved_cwd,
			&PathInputOptions {
				trim: true,
				..base.clone()
			},
		);
		if std::fs::metadata(&resolved_path).is_err() {
			continue;
		}

		match std::fs::metadata(&resolved_path) {
			Ok(stats) => {
				if stats.is_dir() {
					add_result(
						&mut templates,
						&mut diagnostics,
						load_templates_from_dir(&resolved_path, &get_source_info),
					);
				} else if stats.is_file() && resolved_path.ends_with(".md") {
					let (template, result_diagnostics) =
						load_template_from_file(&resolved_path, get_source_info(&resolved_path));
					if let Some(template) = template {
						templates.push(template);
					}
					diagnostics.extend(result_diagnostics);
				}
			}
			Err(err) => {
				diagnostics.push(ResourceDiagnostic::warning(
					node_error_message("stat", &resolved_path, &err),
					Some(resolved_path.clone()),
				));
			}
		}
	}

	LoadPromptTemplatesResult {
		templates,
		diagnostics,
	}
}

/// TS `isUnderPath(target, root)`: exact match or `target` starts with
/// `root + sep` (root is already resolved).
pub(crate) fn is_under_path(target: &str, root: &str) -> bool {
	if target == root {
		return true;
	}
	let prefix = if root.ends_with('/') {
		root.to_string()
	} else {
		format!("{}/", root)
	};
	target.starts_with(&prefix)
}

/// TS `createSyntheticSourceInfo(path, {source, scope?, baseDir?})` with the
/// module-fixed `source: "local"` and default `origin: "top-level"`.
pub(crate) fn create_synthetic_source_info(
	path: &str,
	scope: Option<SourceScope>,
	base_dir: Option<String>,
) -> SourceInfo {
	SourceInfo {
		path: path.to_string(),
		source: "local".to_string(),
		scope: scope.unwrap_or(SourceScope::Temporary),
		origin: SourceOrigin::TopLevel,
		base_dir,
	}
}

/// Expand a prompt template if it matches a template name.
/// Returns the expanded content or the original text if not a template.
pub fn expand_prompt_template(text: &str, templates: &[PromptTemplate]) -> String {
	if !text.starts_with('/') {
		return text.to_string();
	}

	// TS: text.match(/^\/([^\s]+)(?:\s+([\s\S]*))?$/)
	// Fails only when text is "/" or the char after "/" is JS whitespace.
	let chars: Vec<char> = text.chars().collect();
	if chars.len() < 2 || is_js_whitespace_or_terminator(chars[1]) {
		return text.to_string();
	}
	let mut i = 1;
	while i < chars.len() && !is_js_whitespace_or_terminator(chars[i]) {
		i += 1;
	}
	let template_name: String = chars[1..i].iter().collect();
	// (\s+) greedily consumes the separator; group 2 is everything after it
	let mut j = i;
	while j < chars.len() && is_js_whitespace_or_terminator(chars[j]) {
		j += 1;
	}
	let args_string: String = chars[j..].iter().collect();

	if let Some(template) = templates.iter().find(|t| t.name == template_name) {
		let args = parse_command_args(&args_string);
		return substitute_args(&template.content, &args);
	}

	text.to_string()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn scratch(tag: &str) -> std::path::PathBuf {
		use std::sync::atomic::{AtomicUsize, Ordering};
		static COUNTER: AtomicUsize = AtomicUsize::new(0);
		let n = COUNTER.fetch_add(1, Ordering::SeqCst);
		let dir = std::env::temp_dir().join(format!(
			"pi-core-prompt-templates-{}-{}-{}",
			std::process::id(),
			tag,
			n
		));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).unwrap();
		dir
	}

	fn write(path: &std::path::Path, content: &str) {
		std::fs::write(path, content).unwrap();
	}

	fn opts(
		cwd: &str,
		agent_dir: &str,
		prompt_paths: &[&str],
		include_defaults: bool,
	) -> LoadPromptTemplatesOptions {
		LoadPromptTemplatesOptions {
			cwd: cwd.to_string(),
			agent_dir: agent_dir.to_string(),
			prompt_paths: prompt_paths.iter().map(|s| s.to_string()).collect(),
			include_defaults,
			config_dir_name: ".pi".to_string(),
			home_dir: None,
		}
	}

	fn template(name: &str, content: &str) -> PromptTemplate {
		PromptTemplate {
			name: name.to_string(),
			description: String::new(),
			argument_hint: None,
			content: content.to_string(),
			source_info: SourceInfo {
				path: format!("/x/{}.md", name),
				source: "local".to_string(),
				scope: SourceScope::Temporary,
				origin: SourceOrigin::TopLevel,
				base_dir: None,
			},
			file_path: format!("/x/{}.md", name),
		}
	}

	fn strs(values: &[&str]) -> Vec<String> {
		values.iter().map(|s| s.to_string()).collect()
	}

	// ----- parse_command_args -----

	#[test]
	fn parse_command_args_basic() {
		assert!(parse_command_args("").is_empty());
		assert!(parse_command_args("  ").is_empty());
		assert_eq!(parse_command_args("a b c"), strs(&["a", "b", "c"]));
		assert_eq!(parse_command_args("a    b"), strs(&["a", "b"]));
		assert_eq!(parse_command_args("a "), strs(&["a"]));
		assert_eq!(parse_command_args("  a"), strs(&["a"]));
	}

	#[test]
	fn parse_command_args_quotes() {
		assert_eq!(parse_command_args("a \"b c\" d"), strs(&["a", "b c", "d"]));
		assert_eq!(parse_command_args("--flag 'q r'"), strs(&["--flag", "q r"]));
		assert_eq!(parse_command_args("\"\" x"), strs(&["", "x"]));
		assert_eq!(parse_command_args("'' x"), strs(&["", "x"]));
		assert_eq!(parse_command_args("'a'  'b'"), strs(&["a", "b"]));
		// unterminated quote keeps the token
		assert_eq!(parse_command_args("a \"b c"), strs(&["a", "b c"]));
		assert_eq!(parse_command_args("mix'ed"), strs(&["mixed"]));
	}

	#[test]
	fn parse_command_args_js_whitespace() {
		// JS \s includes U+00A0 and U+FEFF
		assert_eq!(parse_command_args("a\u{00a0}b"), strs(&["a", "b"]));
		assert_eq!(parse_command_args("a\u{feff}b"), strs(&["a", "b"]));
	}

	// ----- substitute_args -----

	#[test]
	fn substitute_args_positional() {
		let args = strs(&["a", "b"]);
		assert_eq!(substitute_args("$1 and $2", &args), "a and b");
		assert_eq!(substitute_args("$3", &args), "");
		assert_eq!(substitute_args("$1$2", &args), "ab");
		let twelve = (0..12).map(|i| format!("a{}", i)).collect::<Vec<_>>();
		assert_eq!(substitute_args("$12", &twelve), "a11");
		assert_eq!(substitute_args("$12", &args), "");
	}

	#[test]
	fn substitute_args_all_args() {
		let args = strs(&["a", "b"]);
		assert_eq!(substitute_args("$@", &args), "a b");
		assert_eq!(substitute_args("$ARGUMENTS", &args), "a b");
		assert_eq!(substitute_args("$ARGUMENTSX", &args), "a bX");
		assert_eq!(substitute_args("$@", &[]), "");
	}

	#[test]
	fn substitute_args_defaults() {
		assert_eq!(substitute_args("${1:-d}", &[]), "d");
		assert_eq!(substitute_args("${1:-d}", &strs(&["x"])), "x");
		// empty string is falsy → default
		assert_eq!(substitute_args("${1:-d}", &strs(&[""])), "d");
		assert_eq!(substitute_args("${ARGUMENTS:-d}", &[]), "d");
		assert_eq!(substitute_args("${ARGUMENTS:-d}", &strs(&["x"])), "x");
		assert_eq!(substitute_args("${@:-d}", &[]), "d");
		assert_eq!(substitute_args("${@:-d}", &strs(&["x"])), "x");
		assert_eq!(substitute_args("${@:-}", &[]), "");
		assert_eq!(substitute_args("${0:-d}", &strs(&["x"])), "d");
		// parseInt overflow → out-of-range index → default
		assert_eq!(
			substitute_args("${99999999999999999999:-d}", &[]),
			"d"
		);
	}

	#[test]
	fn substitute_args_slices() {
		let args = strs(&["a", "b", "c"]);
		assert_eq!(substitute_args("${@:2}", &args), "b c");
		assert_eq!(substitute_args("${@:2:1}", &args), "b");
		assert_eq!(substitute_args("${@:0}", &args), "a b c");
		assert_eq!(substitute_args("${@:1:2}", &args), "a b");
		// clamped like JS Array.prototype.slice
		assert_eq!(substitute_args("${@:2:10}", &args), "b c");
		assert_eq!(substitute_args("${@:5}", &args), "");
		assert_eq!(substitute_args("${@:5:2}", &args), "");
	}

	#[test]
	fn substitute_args_non_matching() {
		let args = strs(&["a"]);
		assert_eq!(substitute_args("${5}", &args), "${5}");
		assert_eq!(substitute_args("${12x:-y}", &args), "${12x:-y}");
		assert_eq!(substitute_args("${@:x}", &args), "${@:x}");
		assert_eq!(substitute_args("${@:1:2:3}", &strs(&["a", "b"])), "${@:1:2:3}");
		assert_eq!(substitute_args("${@:-", &args), "${@:-");
		assert_eq!(substitute_args("100%", &args), "100%");
		// JS \s after $ is not a token start
		assert_eq!(substitute_args("a$\u{00a0}b", &strs(&["x"])), "a$\u{00a0}b");
	}

	#[test]
	fn substitute_args_no_recursion() {
		// defaults and values are not re-substituted
		assert_eq!(substitute_args("${1:-$2}", &strs(&["", "x"])), "$2");
		assert_eq!(substitute_args("$1", &strs(&["$2"])), "$2");
	}

	// ----- expand_prompt_template -----

	#[test]
	fn expand_prompt_template_basic() {
		let templates = [template("commit", "do $1 now")];
		assert_eq!(expand_prompt_template("/commit world", &templates), "do world now");
		assert_eq!(expand_prompt_template("/commit", &templates), "do  now");
		assert_eq!(expand_prompt_template("/commit  a  b", &templates), "do a now");
		assert_eq!(
			expand_prompt_template("plain text", &templates),
			"plain text"
		);
	}

	#[test]
	fn expand_prompt_template_no_match_or_no_name() {
		let templates = [template("commit", "c")];
		assert_eq!(expand_prompt_template("/missing x", &templates), "/missing x");
		assert_eq!(expand_prompt_template("/", &templates), "/");
		assert_eq!(expand_prompt_template("/ x", &templates), "/ x");
		assert_eq!(expand_prompt_template("", &templates), "");
	}

	#[test]
	fn expand_prompt_template_first_match_and_args() {
		let templates = [template("name", "[name]"), template("NAME", "[NAME]")];
		assert_eq!(expand_prompt_template("/NAME", &templates), "[NAME]");
		let quoted = [template("q", "[$1][$2][$@]")];
		assert_eq!(
			expand_prompt_template("/q \"a b\" c", &quoted),
			"[a b][c][a b c]"
		);
	}

	// ----- parse_frontmatter / template file loading -----

	#[test]
	fn frontmatter_basic() {
		let (fm, body) = parse_frontmatter("---\ndescription: hi\n---\nbody text").unwrap();
		assert_eq!(frontmatter_get_str(&fm, "description"), Some("hi"));
		assert_eq!(body, "body text");
	}

	#[test]
	fn frontmatter_no_or_unclosed_or_empty() {
		let (fm, body) = parse_frontmatter("just body").unwrap();
		assert!(fm.is_empty());
		assert_eq!(body, "just body");
		let (fm, body) = parse_frontmatter("---\ndescription: hi\n").unwrap();
		assert!(fm.is_empty());
		assert_eq!(body, "---\ndescription: hi\n");
		// "---\n---\nbody" → empty (falsy) frontmatter string
		let (fm, body) = parse_frontmatter("---\n---\nbody").unwrap();
		assert!(fm.is_empty());
		assert_eq!(body, "body");
	}

	#[test]
	fn frontmatter_bom_and_crlf() {
		let (fm, body) = parse_frontmatter("\u{feff}---\r\ndescription: hi\r\n---\r\nbody").unwrap();
		assert_eq!(frontmatter_get_str(&fm, "description"), Some("hi"));
		assert_eq!(body, "body");
	}

	#[test]
	fn frontmatter_yaml_errors_byte_exact() {
		// unterminated double quote (26-char line → column 27)
		let err = parse_frontmatter("---\ndescription: \"unterminated\n---\nbody").unwrap_err();
		assert_eq!(
			err,
			format!(
				"Missing closing \"quote at line 1, column 27:\n\ndescription: \"unterminated\n{}^\n",
				" ".repeat(26)
			)
		);
		// unterminated single quote, error at end of stream
		let err = parse_frontmatter("---\na: 1\ndescription: 'unterm\nc: 2\n---\nbody").unwrap_err();
		assert_eq!(
			err,
			"Missing closing 'quote at line 3, column 5:\n\nc: 2\n    ^\n"
		);
		// nested mapping
		let err = parse_frontmatter("---\na: b: c\n---\nbody").unwrap_err();
		assert_eq!(
			err,
			"Nested mappings are not allowed in compact mappings at line 1, column 4:\n\na: b: c\n   ^\n"
		);
		// nested mapping with trailing space / trailing colon
		let err = parse_frontmatter("---\na: b: \n---\nbody").unwrap_err();
		assert_eq!(
			err,
			"Nested mappings are not allowed in compact mappings at line 1, column 4:\n\na: b: \n   ^\n"
		);
		let err = parse_frontmatter("---\na: b:\n---\nbody").unwrap_err();
		assert_eq!(
			err,
			"Nested mappings are not allowed in compact mappings at line 1, column 4:\n\na: b:\n   ^\n"
		);
		// quoted values containing ": " are fine
		assert!(parse_frontmatter("---\na: 'b: c'\n---\nbody").is_ok());
		assert!(parse_frontmatter("---\na: \"b: c\"\n---\nbody").is_ok());
		// duplicate keys (previous line included as context)
		let err = parse_frontmatter("---\ndescription: 1\ndescription: 2\n---\nbody").unwrap_err();
		assert_eq!(
			err,
			"Map keys must be unique at line 2, column 1:\n\ndescription: 1\ndescription: 2\n^\n"
		);
	}

	#[test]
	fn frontmatter_scalar_kinds() {
		let (fm, _) = parse_frontmatter(
		 "---\ndescription: hi # comment\nd: 123\nflag: true\nnone: ~\nnil:\nq: \"123\"\nhttp: //x\n---\nb",
		)
		.unwrap();
		assert_eq!(frontmatter_get_str(&fm, "description"), Some("hi"));
		// numbers / bools / null are not strings
		assert_eq!(frontmatter_get_str(&fm, "d"), None);
		assert_eq!(frontmatter_get_str(&fm, "flag"), None);
		assert_eq!(frontmatter_get_str(&fm, "none"), None);
		assert_eq!(frontmatter_get_str(&fm, "nil"), None);
		// quoted "123" is a string
		assert_eq!(frontmatter_get_str(&fm, "q"), Some("123"));
		// "http: //x" — ": " is a key separator; "//x" is a plain scalar
		assert_eq!(frontmatter_get_str(&fm, "http"), Some("//x"));
	}

	#[test]
	fn load_template_description_and_hint() {
		let dir = scratch("desc");
		write(
			&dir.join("t.md"),
			"---\ndescription: my template\nargument-hint: \"<url>\"\n---\ncontent here",
		);
		let result = load_prompt_templates(&opts(
			dir.to_str().unwrap(),
			dir.to_str().unwrap(),
			&[],
			false,
		));
		// no defaults → use the explicit path
		assert!(result.templates.is_empty());
		let result = load_prompt_templates(&opts(
			dir.to_str().unwrap(),
			dir.to_str().unwrap(),
			&["t.md"],
			false,
		));
		assert_eq!(result.templates.len(), 1);
		let t = &result.templates[0];
		assert_eq!(t.name, "t");
		assert_eq!(t.description, "my template");
		assert_eq!(t.argument_hint.as_deref(), Some("<url>"));
		assert_eq!(t.content, "content here");
		assert_eq!(t.file_path, format!("{}/t.md", dir.to_str().unwrap()));
		assert!(result.diagnostics.is_empty());
	}

	#[test]
	fn load_template_description_fallback_and_truncation() {
		let dir = scratch("fallback");
		// no frontmatter → description from first non-empty line, trimmed body
		write(&dir.join("plain.md"), "\n  \nfirst line is description\n\nsecond");
		// frontmatter description: '' (empty string is falsy) → fallback
		write(&dir.join("empty.md"), "---\ndescription: ''\n---\nfrom body");
		// non-string description → fallback
		write(&dir.join("num.md"), "---\ndescription: 42\n---\nnumber body");
		// empty argument-hint → omitted
		write(&dir.join("hint.md"), "---\ndescription: d\nargument-hint: ''\n---\nb");
		// 61 chars → 60 chars + "..."
		let long = "x".repeat(61);
		write(&dir.join("long.md"), &format!("\n{}", long));
		let exact = "y".repeat(60);
		write(&dir.join("exact.md"), &format!("\n{}", exact));

		let result = load_prompt_templates(&opts(
			dir.to_str().unwrap(),
			dir.to_str().unwrap(),
			&["plain.md", "empty.md", "num.md", "hint.md", "long.md", "exact.md"],
			false,
		));
		let by_name = |n: &str| {
			result
				.templates
				.iter()
				.find(|t| t.name == n)
				.unwrap_or_else(|| panic!("missing {}", n))
		};
		assert_eq!(by_name("plain").description, "first line is description");
		// no frontmatter → body is NOT trimmed (TS extractFrontmatter)
		assert_eq!(by_name("plain").content, "\n  \nfirst line is description\n\nsecond");
		assert_eq!(by_name("empty").description, "from body");
		assert_eq!(by_name("num").description, "number body");
		assert_eq!(by_name("hint").argument_hint, None);
		assert_eq!(by_name("long").description, format!("{}...", "x".repeat(60)));
		assert_eq!(by_name("exact").description, exact);
	}

	#[test]
	fn load_template_name_from_basename() {
		let dir = scratch("names");
		write(&dir.join("a.md.md"), "---\ndescription: d\n---\nb");
		write(&dir.join("b.MD"), "b");
		write(&dir.join("c.txt"), "c");
		let result = load_prompt_templates(&opts(
			dir.to_str().unwrap(),
			dir.to_str().unwrap(),
			&["a.md.md", "b.MD", "c.txt"],
			false,
		));
		// one trailing .md stripped; .MD (case) and non-.md explicit files skipped
		assert_eq!(result.templates.len(), 1);
		assert_eq!(result.templates[0].name, "a.md");
	}

	#[test]
	fn load_templates_dirs_defaults_order_and_scope() {
		let root = scratch("defaults");
		let agent_dir = root.join("agent");
		let cwd = root.join("proj");
		std::fs::create_dir_all(agent_dir.join("prompts")).unwrap();
		std::fs::create_dir_all(cwd.join(".pi").join("prompts")).unwrap();
		write(&agent_dir.join("prompts").join("global.md"), "g");
		write(&cwd.join(".pi").join("prompts").join("project.md"), "p");
		let extra = root.join("extra.md");
		write(&extra, "e");

		let result = load_prompt_templates(&opts(
			cwd.to_str().unwrap(),
			agent_dir.to_str().unwrap(),
			&[extra.to_str().unwrap()],
			true,
		));
		assert_eq!(
			result
				.templates
				.iter()
				.map(|t| t.name.as_str())
				.collect::<Vec<_>>(),
			vec!["global", "project", "extra"]
		);
		let global = &result.templates[0];
		assert_eq!(global.source_info.scope, SourceScope::User);
		assert_eq!(
			global.source_info.base_dir.as_deref(),
			Some(agent_dir.join("prompts").to_str().unwrap())
		);
		let project = &result.templates[1];
		assert_eq!(project.source_info.scope, SourceScope::Project);
		assert_eq!(
			project.source_info.base_dir.as_deref(),
			Some(cwd.join(".pi").join("prompts").to_str().unwrap())
		);
		// explicit file outside the default dirs → temporary scope, dirname base
		let extra_t = &result.templates[2];
		assert_eq!(extra_t.source_info.scope, SourceScope::Temporary);
		assert_eq!(
			extra_t.source_info.base_dir.as_deref(),
			Some(root.to_str().unwrap())
		);

		// include_defaults = false → only explicit paths
		let result = load_prompt_templates(&opts(
			cwd.to_str().unwrap(),
			agent_dir.to_str().unwrap(),
			&[extra.to_str().unwrap()],
			false,
		));
		assert_eq!(result.templates.len(), 1);
		assert_eq!(result.templates[0].name, "extra");

		// custom config dir name
		let custom = root.join("custom");
		std::fs::create_dir_all(custom.join("pi").join("prompts")).unwrap();
		write(&custom.join("pi").join("prompts").join("z.md"), "z");
		let mut options = opts(custom.to_str().unwrap(), "/nonexistent", &[], true);
		options.config_dir_name = "pi".to_string();
		let result = load_prompt_templates(&options);
		assert_eq!(result.templates.len(), 1);
		assert_eq!(result.templates[0].name, "z");
		assert_eq!(result.templates[0].source_info.scope, SourceScope::Project);
	}

	#[test]
	fn load_templates_dir_scan_rules() {
		let root = scratch("scan");
		let dir = root.join("prompts");
		std::fs::create_dir_all(dir.join("nested")).unwrap();
		write(&dir.join("one.md"), "---\ndescription: d\n---\nb");
		write(&dir.join("two.txt"), "t");
		write(&dir.join("nested").join("three.md"), "n");

		let result = load_prompt_templates(&opts(
			root.to_str().unwrap(),
			"/nonexistent",
			&[dir.to_str().unwrap()],
			false,
		));
		// non-recursive, .md files only
		assert_eq!(result.templates.len(), 1);
		assert_eq!(result.templates[0].name, "one");

		// explicit directory that does not exist → skipped, no diagnostics
		let result = load_prompt_templates(&opts(
			root.to_str().unwrap(),
			"/nonexistent",
			&["missing.md", "missing-dir"],
			false,
		));
		assert!(result.templates.is_empty());
		assert!(result.diagnostics.is_empty());
	}

	#[cfg(unix)]
	#[test]
	fn load_templates_symlinks() {
		use std::os::unix::fs::symlink;
		let root = scratch("symlinks");
		let dir = root.join("prompts");
		std::fs::create_dir_all(&dir).unwrap();
		write(&root.join("real.md"), "r");
		symlink(root.join("real.md"), dir.join("link.md")).unwrap();
		symlink(root.join("nowhere.md"), dir.join("broken.md")).unwrap();
		symlink(root.join("missing-dir"), dir.join("dir-link")).unwrap();

		let result = load_prompt_templates(&opts(
			root.to_str().unwrap(),
			"/nonexistent",
			&[dir.to_str().unwrap()],
			false,
		));
		assert_eq!(result.templates.len(), 1);
		assert_eq!(result.templates[0].name, "link");
		assert!(result.diagnostics.is_empty());
	}

	#[test]
	fn load_templates_parse_error_diagnostic() {
		let dir = scratch("badyaml");
		write(&dir.join("bad.md"), "---\ndescription: \"unterminated\n---\nbody");
		let result = load_prompt_templates(&opts(
			dir.to_str().unwrap(),
			"/nonexistent",
			&["bad.md"],
			false,
		));
		assert!(result.templates.is_empty());
		assert_eq!(result.diagnostics.len(), 1);
		let d = &result.diagnostics[0];
		assert_eq!(d.r#type, DiagnosticType::Warning);
		assert_eq!(d.path.as_deref(), Some(format!("{}/bad.md", dir.to_str().unwrap()).as_str()));
		assert_eq!(
			d.message,
			format!(
				"Missing closing \"quote at line 1, column 27:\n\ndescription: \"unterminated\n{}^\n",
				" ".repeat(26)
			)
		);
	}

	#[test]
	fn load_templates_prompt_path_resolution() {
		let root = scratch("resolve");
		let cwd = root.join("work");
		std::fs::create_dir_all(&cwd).unwrap();
		write(&cwd.join("rel.md"), "relative");
		// relative prompt path resolves against cwd; trims whitespace
		let result = load_prompt_templates(&opts(
			cwd.to_str().unwrap(),
			"/nonexistent",
			&["  rel.md  "],
			false,
		));
		assert_eq!(result.templates.len(), 1);
		assert_eq!(result.templates[0].name, "rel");
		assert_eq!(result.templates[0].content, "relative");
	}

	#[test]
	fn node_helpers() {
		assert_eq!(basename("/a/b.md"), "b.md");
		assert_eq!(basename("/a/b/"), "b");
		assert_eq!(basename("b.md"), "b.md");
		assert_eq!(template_name("/a/b.md"), "b");
		assert_eq!(template_name("/a/.md"), "");
		assert_eq!(node_dirname("/a/b"), "/a");
		assert_eq!(node_dirname("/a"), "/");
		assert_eq!(node_dirname("a"), ".");
		assert_eq!(node_path_join("/a", "b"), "/a/b");
		assert_eq!(node_path_join("/a/", "b"), "/a/b");
		assert!(is_under_path("/a/b", "/a"));
		assert!(is_under_path("/a", "/a"));
		assert!(!is_under_path("/ab", "/a"));
		assert!(is_under_path("/a", "/"));
	}
}
