//! Agent skills loaded from `SKILL.md` files and plain markdown files.
//!
//! Port of `packages/coding-agent/src/core/skills.ts`.
//!
//! # JS-semantics traps honored here
//!
//! - **No ambient state**: all paths (`cwd`, `agent_dir`, `skill_paths`) and
//!   the config dir name are injected (TS `getAgentDir()` fallback is
//!   unreachable for the typed options; callers always pass `agent_dir`).
//!   Relative `cwd`/`agent_dir` resolve against `"/"` like
//!   [`crate::prompt_templates`].
//! - **`undefined` omitted, not `null`**: the diagnostic `path` and collision
//!   fields are `Option` and skipped during serialization.
//! - **UTF-16 lengths**: `name.length > 64` / `description.length > 1024`
//!   use JS `.length` semantics via [`crate::js_string::js_length`].
//! - **`||` fallback**: an empty frontmatter `name` falls back to the parent
//!   directory name (empty string is falsy).
//!
//! # Frontmatter
//!
//! Reuses the YAML-subset parser probed for [`crate::prompt_templates`]
//! (commit 4bf7b40); the parser now distinguishes `true`/`false` booleans so
//! `disable-model-invocation === true` can be evaluated exactly.
//!
//! # node-ignore subset
//!
//! `ignore` (npm, 7.0.8) is an external dependency of skills.ts; this module
//! carries a hand-rolled matcher for the `ignores()` surface it uses
//! (relative posix paths, trailing `/` marks directories). It reproduces the
//! observable semantics for gitignore-family patterns:
//!
//! - rule order matters; the last matching rule wins, except that a negation
//!   (`!pattern`) before any positive match is skipped (`checkUnignored:
//!   false`, as `ignores()` invokes the state machine);
//! - matching is **case-insensitive** (`new Ignore()` defaults
//!   `ignorecase: true`), approximated with per-`char` Unicode lowercasing
//!   (avoids the final-sigma context rule; diverges from JS simple folding
//!   for chars like `İ` that lowercase to multiple chars);
//! - `*` matches within one segment, `?` one non-slash char, `[...]` classes
//!   (with `!`/`^` negation, ranges, `\` escapes, POSIX `[:alpha:]` classes)
//!   never match `/`; an unterminated or unknown class matches nothing;
//! - a `/` at the start or middle anchors the pattern to the scan root;
//!   otherwise it matches at any depth;
//! - a trailing `/` restricts the pattern to directories (callers mark
//!   directories with a trailing `/`);
//! - `**` between slashes: leading `**/` matches zero or more segments,
//!   interior `/**/` zero or more, trailing `/**` (and `/**/`) one or more
//!   with the directory restriction — verified against node-ignore 7.0.8;
//! - `ignores()` walks parent directories first: a path under an ignored
//!   directory is ignored and cannot be re-included.
//!
//! Known divergences (documented, out of realistic parity scope): patterns
//! with more than two consecutive leading slashes, brace or `extglob`
//! syntax, non-ASCII case folding edge cases, and invalid (absolute) paths
//! (node-ignore throws `RangeError`, which skills.ts would swallow in its
//! outer `try/catch`; this port never panics).
//!
//! # Harness notes
//!
//! - `readdir` iteration order is the OS order (like `readdirSync`); parity
//!   cases must not depend on directory order.
//! - The ignore matcher is shared across one top-level directory scan (rules
//!   from nested ignore files accumulate as the scan descends), matching the
//!   TS closure sharing.
//! - fs error diagnostics use Node-style messages, mirroring the private
//!   helpers in [`crate::prompt_templates`] (pi-core is below pi-tools).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::js_string::js_length;
use crate::paths::{canonicalize_path, node_relative, resolve_path, PathInputOptions};
use crate::prompt_templates::{
	basename, create_synthetic_source_info, frontmatter_get_str, is_under_path, js_trim,
	node_dirname, node_error_message, node_path_join, parse_frontmatter, DiagnosticType,
	FrontmatterValue, ResourceCollision, ResourceDiagnostic, SourceInfo, SourceOrigin,
	SourceScope,
};

/// Max name length per spec
const MAX_NAME_LENGTH: usize = 64;

/// Max description length per spec
const MAX_DESCRIPTION_LENGTH: usize = 1024;

const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];

/// A skill loaded from a `SKILL.md` or markdown file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
	pub name: String,
	pub description: String,
	pub file_path: String,
	pub base_dir: String,
	pub source_info: SourceInfo,
	pub disable_model_invocation: bool,
}

/// Result of loading skills.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadSkillsResult {
	pub skills: Vec<Skill>,
	pub diagnostics: Vec<ResourceDiagnostic>,
}

/// Options for [`load_skills`].
#[derive(Debug, Clone)]
pub struct LoadSkillsOptions {
	/// Working directory for project-local skills.
	pub cwd: String,
	/// Agent config directory for global skills.
	pub agent_dir: String,
	/// Explicit skill paths (files or directories).
	pub skill_paths: Vec<String>,
	/// Include default skills directories.
	pub include_defaults: bool,
	/// `CONFIG_DIR_NAME` from `config.ts` (default distribution value `".pi"`).
	pub config_dir_name: String,
	/// Injected home directory used for `~` expansion in skill paths. When
	/// `None`, `crate::paths` falls back to `$HOME` (see module docs).
	pub home_dir: Option<String>,
}

impl Default for LoadSkillsOptions {
	fn default() -> Self {
		Self {
			cwd: ".".to_string(),
			agent_dir: ".".to_string(),
			skill_paths: Vec::new(),
			include_defaults: false,
			config_dir_name: ".pi".to_string(),
			home_dir: None,
		}
	}
}

/// The `fileReadTool` argument of `formatSkillsForPrompt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileReadTool {
	#[default]
	Read,
	Bash,
	Indirect,
}

// ---------------------------------------------------------------------------
// node-ignore subset (npm `ignore` 7.0.8, `ignores()` surface only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum ClassItem {
	Ch(char),
	Range(char, char),
}

/// A `[...]` bracket expression. Never matches a path separator, whatever
/// its members say (git `wildmatch.c`).
#[derive(Debug, Clone, PartialEq)]
struct ClassSpec {
	negated: bool,
	items: Vec<ClassItem>,
}

impl ClassSpec {
	fn matches_item(&self, c: char) -> bool {
		self.items.iter().any(|item| match item {
			ClassItem::Ch(x) => *x == c,
			ClassItem::Range(a, b) => (*a..=*b).contains(&c),
		})
	}

	/// Case-insensitive match (`ignorecase: true`): any case variant of the
	/// input char hitting the class counts.
	fn matches_ci(&self, c: char) -> bool {
		let hit = ci_variants(c).iter().any(|&v| self.matches_item(v));
		if self.negated {
			!hit
		} else {
			hit
		}
	}
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
	Lit(char),
	Star,
	Ques,
	Class(ClassSpec),
}

/// One pattern segment: either glob tokens or a slash-bounded `**`.
#[derive(Debug, Clone, PartialEq)]
enum Seg {
	/// `**` bounded by slashes; consumes `min` or more path segments.
	Globstar { min: usize },
	Toks(Vec<Tok>),
}

#[derive(Debug, Clone, PartialEq)]
enum PatternMatcher {
	/// A pattern git gives up on (unterminated bracket expression, unknown
	/// POSIX class): matches nothing.
	Never,
	Segments {
		anchored: bool,
		dir_only: bool,
		segs: Vec<Seg>,
	},
}

#[derive(Debug, Clone, PartialEq)]
struct IgnoreRule {
	negative: bool,
	matcher: PatternMatcher,
}

/// Case variants of a char (itself plus single-char lower/upper foldings).
fn ci_variants(c: char) -> Vec<char> {
	let mut variants = vec![c];
	let lower: String = c.to_lowercase().collect();
	if lower.chars().count() == 1 {
		let l = lower.chars().next().unwrap();
		if l != c {
			variants.push(l);
		}
	}
	let upper: String = c.to_uppercase().collect();
	if upper.chars().count() == 1 {
		let u = upper.chars().next().unwrap();
		if u != c && !variants.contains(&u) {
			variants.push(u);
		}
	}
	variants
}

/// Case-insensitive char equality (JS regex `i` flag approximation).
fn ci_eq(a: char, b: char) -> bool {
	if a == b {
		return true;
	}
	let la: String = a.to_lowercase().collect();
	let lb: String = b.to_lowercase().collect();
	// Multi-char foldings (e.g. `İ`) do not simple-fold in JS either.
	la.chars().count() == 1 && lb.chars().count() == 1 && la == lb
}

/// POSIX bracket classes (git `wildmatch.c`, ASCII-only).
fn posix_class_items(name: &str) -> Option<Vec<ClassItem>> {
	let range = |a: char, b: char| ClassItem::Range(a, b);
	let ch = ClassItem::Ch;
	let items = match name {
		"alnum" => vec![range('0', '9'), range('A', 'Z'), range('a', 'z')],
		"alpha" => vec![range('A', 'Z'), range('a', 'z')],
		"blank" => vec![ch(' '), ch('\t')],
		"cntrl" => vec![range('\u{0}', '\u{1f}'), ch('\u{7f}')],
		"digit" => vec![range('0', '9')],
		"graph" => vec![range('!', '.'), range('0', '~')],
		"lower" => vec![range('a', 'z')],
		"print" => vec![range(' ', '.'), range('0', '~')],
		"punct" => vec![range('!', '.'), range(':', '@'), range('[', '`'), range('{', '~')],
		"space" => vec![ch(' '), ch('\t'), ch('\n'), ch('\r')],
		"upper" => vec![range('A', 'Z')],
		"xdigit" => vec![range('0', '9'), range('A', 'F'), range('a', 'f')],
		_ => return None,
	};
	Some(items)
}

/// Scan the bracket expression starting at `chars[start] == '['`, mirroring
/// the member loop of git's `wildmatch.c` (via node-ignore `scanBracket`).
/// Returns the class and the index of the closing `]`; `None` when the
/// expression is never terminated or names an unknown POSIX class, which
/// makes the whole pattern match nothing.
fn scan_bracket(chars: &[char], start: usize) -> Option<(ClassSpec, usize)> {
	let mut index = start + 1;
	let mut negated = false;
	if index < chars.len() && (chars[index] == '!' || chars[index] == '^') {
		negated = true;
		index += 1;
	}
	let mut items: Vec<ClassItem> = Vec::new();
	// The member a `-` could start a range from; `None` when the previous
	// member cannot open one.
	let mut prev: Option<char> = None;
	loop {
		let c = *chars.get(index)?;
		if c == '\\' {
			let escaped = *chars.get(index + 1)?;
			items.push(ClassItem::Ch(escaped));
			prev = Some(escaped);
			index += 1;
		} else if c == '-'
			&& prev.is_some()
			&& index + 1 < chars.len()
			&& chars[index + 1] != ']'
		{
			index += 1;
			let mut to = chars[index];
			if to == '\\' {
				index += 1;
				to = *chars.get(index)?;
			}
			let from = prev.unwrap();
			// An out-of-order range is dropped; its lower bound stays as a
			// plain member.
			if from <= to {
				items.push(ClassItem::Range(from, to));
			}
			prev = None;
		} else if c == '[' && index + 1 < chars.len() && chars[index + 1] == ':' {
			let name_start = index + 2;
			let mut end = name_start;
			while end < chars.len() && chars[end] != ']' {
				end += 1;
			}
			if end == chars.len() {
				return None;
			}
			if end > name_start && chars[end - 1] == ':' {
				let name: String = chars[name_start..end - 1].iter().collect();
				items.extend(posix_class_items(&name)?);
				prev = None;
				index = end;
			} else {
				// No `:]` to close it: the `[` is a plain member.
				items.push(ClassItem::Ch('['));
				prev = Some('[');
				index = name_start - 2;
			}
		} else {
			items.push(ClassItem::Ch(c));
			prev = Some(c);
		}
		index += 1;
		if index < chars.len() && chars[index] == ']' {
			return Some((ClassSpec { negated, items }, index));
		}
	}
}

/// git trailing-space trim (node-ignore REPLACERS step): trailing unescaped
/// spaces are dropped; a run of `n` backslashes immediately before them
/// keeps `n - 1` backslashes and a single space.
fn trim_trailing_spaces(pattern: &str) -> String {
	let chars: Vec<char> = pattern.chars().collect();
	let mut end = chars.len();
	while end > 0 && chars[end - 1] == ' ' {
		end -= 1;
	}
	if end == chars.len() {
		return pattern.to_string();
	}
	if end > 0 && chars[end - 1] == '\\' {
		let mut run_start = end;
		while run_start > 0 && chars[run_start - 1] == '\\' {
			run_start -= 1;
		}
		let n = end - run_start;
		let mut out: String = chars[..run_start].iter().collect();
		out.push_str(&"\\".repeat(n - 1));
		out.push(' ');
		out
	} else {
		chars[..end].iter().collect()
	}
}

/// node-ignore REPLACERS step `/(\\+?) /g`: a space preceded by `k >= 1`
/// backslashes keeps `k - k % 2` of them (an odd backslash escapes the
/// space).
fn unescape_spaces(pattern: &str) -> String {
	let chars: Vec<char> = pattern.chars().collect();
	let mut out = String::new();
	let mut i = 0usize;
	while i < chars.len() {
		if chars[i] == '\\' {
			let mut run_end = i;
			while run_end < chars.len() && chars[run_end] == '\\' {
				run_end += 1;
			}
			let n = run_end - i;
			if run_end < chars.len() && chars[run_end] == ' ' {
				out.push_str(&"\\".repeat(n - n % 2));
				out.push(' ');
				i = run_end + 1;
			} else {
				out.push_str(&"\\".repeat(n));
				i = run_end;
			}
		} else {
			out.push(chars[i]);
			i += 1;
		}
	}
	out
}

/// node-ignore `checkPattern`: non-empty, not a spaces-only line, no invalid
/// trailing backslash, not a `#` comment.
fn check_pattern(pattern: &str) -> bool {
	if pattern.is_empty() {
		return false;
	}
	let chars: Vec<char> = pattern.chars().collect();
	if chars.iter().all(|&c| c == ' ') {
		return false;
	}
	// /(?:[^\\]|^)\\$/
	if chars.last() == Some(&'\\') && (chars.len() == 1 || chars[chars.len() - 2] != '\\') {
		return false;
	}
	!pattern.starts_with('#')
}

/// Compile one ignore pattern into a rule (node-ignore `createRule` +
/// `makeRegexPrefix`, expressed as a segment matcher instead of a regex).
fn compile_rule(pattern: &str) -> Option<IgnoreRule> {
	if !check_pattern(pattern) {
		return None;
	}
	let mut negative = false;
	let mut body: &str = pattern;
	if let Some(rest) = body.strip_prefix('!') {
		negative = true;
		body = rest;
	}
	// .replace(/^\\!/, '!').replace(/^\\#/, '#')
	if body.starts_with("\\!") || body.starts_with("\\#") {
		body = &body[1..];
	}
	// BOM strip
	let body = body.strip_prefix('\u{feff}').unwrap_or(body);
	let body = trim_trailing_spaces(body);
	let body = unescape_spaces(&body);

	let matcher = compile_matcher(&body);
	Some(IgnoreRule { negative, matcher })
}

fn is_double_star(seg: &[Tok]) -> bool {
	seg.len() == 2 && matches!(seg[0], Tok::Star) && matches!(seg[1], Tok::Star)
}

fn compile_matcher(body: &str) -> PatternMatcher {
	let chars: Vec<char> = body.chars().collect();
	let mut segs: Vec<Vec<Tok>> = vec![Vec::new()];
	let mut i = 0usize;
	while i < chars.len() {
		let c = chars[i];
		match c {
			'\\' => {
				let next = chars.get(i + 1).copied().unwrap_or('\\');
				segs.last_mut().expect("non-empty").push(Tok::Lit(next));
				i += 2;
			}
			'/' => {
				segs.push(Vec::new());
				i += 1;
			}
			'*' => {
				segs.last_mut().expect("non-empty").push(Tok::Star);
				i += 1;
			}
			'?' => {
				segs.last_mut().expect("non-empty").push(Tok::Ques);
				i += 1;
			}
			'[' => match scan_bracket(&chars, i) {
				Some((class, end)) => {
					segs.last_mut().expect("non-empty").push(Tok::Class(class));
					i = end + 1;
				}
				None => return PatternMatcher::Never,
			},
			c => {
				segs.last_mut().expect("non-empty").push(Tok::Lit(c));
				i += 1;
			}
		}
	}
	// A trailing slash restricts the pattern to directories. It also bounds
	// a preceding `**` (`a/**/` and `**/` stay globstars, verified against
	// node-ignore 7.0.8).
	let mut dir_only = false;
	if segs.len() > 1 && segs.last().is_some_and(|s| s.is_empty()) {
		dir_only = true;
		segs.pop();
	}
	let started_with_slash = body.starts_with('/');
	let anchored = started_with_slash || segs.len() > 1;
	// Leading `/` anchors (drop leading empty segments; node-ignore keeps
	// exactly one, diverging only for multi-slash patterns).
	while segs.first().is_some_and(|s| s.is_empty()) {
		segs.remove(0);
	}
	if segs.is_empty() {
		return PatternMatcher::Never;
	}
	let n = segs.len();
	let mut out: Vec<Seg> = Vec::with_capacity(n);
	for (i, seg) in segs.into_iter().enumerate() {
		if is_double_star(&seg) && n > 1 {
			// trailing globstar (last, or sole segment after `**/`): min 1;
			// leading/interior: min 0
			let min = if i == n - 1 { 1 } else { 0 };
			out.push(Seg::Globstar { min });
			continue;
		}
		if is_double_star(&seg) && n == 1 && dir_only {
			// pattern `**/`
			out.push(Seg::Globstar { min: 0 });
			continue;
		}
		// Collapse consecutive `*` (`[^/]*[^/]*` == `[^/]*`).
		let mut toks: Vec<Tok> = Vec::new();
		for tok in seg {
			if matches!(tok, Tok::Star) && matches!(toks.last(), Some(Tok::Star)) {
				continue;
			}
			toks.push(tok);
		}
		out.push(Seg::Toks(toks));
	}
	PatternMatcher::Segments {
		anchored,
		dir_only,
		segs: out,
	}
}

/// Glob match within one path segment (`*` runs, `?`, classes, literals).
fn seg_match(toks: &[Tok], seg: &[char]) -> bool {
	let mut ti = 0usize;
	let mut si = 0usize;
	let mut star: Option<(usize, usize)> = None;
	while si < seg.len() {
		let matched = match toks.get(ti) {
			None => false,
			Some(Tok::Star) => {
				star = Some((ti + 1, si));
				ti += 1;
				continue;
			}
			Some(Tok::Ques) => true,
			Some(Tok::Lit(c)) => ci_eq(*c, seg[si]),
			Some(Tok::Class(class)) => class.matches_ci(seg[si]),
		};
		if matched {
			ti += 1;
			si += 1;
		} else if let Some((tok_idx, str_idx)) = star {
			star = Some((tok_idx, str_idx + 1));
			ti = tok_idx;
			si = str_idx + 1;
		} else {
			return false;
		}
	}
	while matches!(toks.get(ti), Some(Tok::Star)) {
		ti += 1;
	}
	ti == toks.len()
}

fn match_from(
	segs: &[Seg],
	path_segs: &[&str],
	start: usize,
	dir_only: bool,
	is_dir: bool,
) -> bool {
	if segs.is_empty() {
		// Pattern exhausted (e.g. via a globstar): for dir-only patterns the
		// path must end here and be a directory (`a/**/` vs `a/b`).
		return !dir_only || (start == path_segs.len() && is_dir);
	}
	match &segs[0] {
		Seg::Globstar { min } => {
			let mut k = *min;
			while start + k <= path_segs.len() {
				if match_from(&segs[1..], path_segs, start + k, dir_only, is_dir) {
					return true;
				}
				k += 1;
			}
			false
		}
		Seg::Toks(toks) => {
			if start >= path_segs.len() {
				return false;
			}
			let seg_chars: Vec<char> = path_segs[start].chars().collect();
			if !seg_match(toks, &seg_chars) {
				return false;
			}
			let rest = &segs[1..];
			if rest.is_empty() {
				if dir_only {
					// `foo/$`: the path must end here and be a directory.
					start + 1 == path_segs.len() && is_dir
				} else {
					// `foo(?=$|\/)`: end of path or more segments.
					true
				}
			} else {
				match_from(rest, path_segs, start + 1, dir_only, is_dir)
			}
		}
	}
}

impl PatternMatcher {
	fn matches(&self, path: &str) -> bool {
		let (anchored, dir_only, segs) = match self {
			PatternMatcher::Never => return false,
			PatternMatcher::Segments {
				anchored,
				dir_only,
				segs,
			} => (*anchored, *dir_only, segs),
		};
		let is_dir = path.ends_with('/');
		let trimmed = path.strip_suffix('/').unwrap_or(path);
		let path_segs: Vec<&str> = trimmed.split('/').collect();
		if anchored {
			match_from(segs, &path_segs, 0, dir_only, is_dir)
		} else {
			(0..=path_segs.len()).any(|i| match_from(segs, &path_segs, i, dir_only, is_dir))
		}
	}
}

/// The parent directory of a path with its trailing separator, or `""` when
/// the path has none (node-ignore `parentOf`).
fn parent_of(path: &str) -> String {
	if path.starts_with('/') || path.contains("//") {
		let mut slices: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
		slices.pop();
		if slices.is_empty() {
			return String::new();
		}
		return format!("{}/", slices.join("/"));
	}
	let chars: Vec<char> = path.chars().collect();
	if chars.is_empty() {
		return String::new();
	}
	let end = chars.len() - 1;
	let from = if chars[end] == '/' {
		end.saturating_sub(1)
	} else {
		end
	};
	for i in (0..=from).rev() {
		if chars[i] == '/' {
			return chars[..=i].iter().collect();
		}
	}
	String::new()
}

/// A shared, ordered set of ignore rules (node-ignore `Ignore`).
#[derive(Debug, Clone, Default)]
struct IgnoreMatcher {
	rules: Vec<IgnoreRule>,
}

impl IgnoreMatcher {
	/// `add(patterns)` for the array form skills.ts uses.
	fn add(&mut self, patterns: &[String]) {
		for pattern in patterns {
			if let Some(rule) = compile_rule(pattern) {
				self.rules.push(rule);
			}
		}
	}

	/// `ignores(path)`: `path` is a relative posix path, optionally with a
	/// trailing `/` marking a directory.
	fn ignores(&self, path: &str) -> bool {
		let parent = parent_of(path);
		if !parent.is_empty() && self.ignores(&parent) {
			// It is not possible to re-include a file if a parent directory
			// of that file is excluded.
			return true;
		}
		self.test_rules(path)
	}

	/// node-ignore `RuleManager.test` with `checkUnignored: false` (how
	/// `ignores()` invokes it): the last matching rule wins, except that a
	/// negation before any positive match is skipped.
	fn test_rules(&self, path: &str) -> bool {
		let mut ignored = false;
		let mut unignored = false;
		for rule in &self.rules {
			let negative = rule.negative;
			let skip = (unignored == negative && ignored != unignored)
				|| (negative && !ignored && !unignored);
			if !skip && rule.matcher.matches(path) {
				ignored = !negative;
				unignored = negative;
			}
		}
		ignored
	}
}

// ---------------------------------------------------------------------------
// Ignore file handling (skills.ts `addIgnoreRules` / `prefixIgnorePattern`)
// ---------------------------------------------------------------------------

fn to_posix_path(p: &str) -> String {
	// path.sep is "/" on unix; a no-op port kept for 1:1 structure.
	p.to_string()
}

fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
	let trimmed = js_trim(line);
	if trimmed.is_empty() {
		return None;
	}
	if trimmed.starts_with('#') && !trimmed.starts_with("\\#") {
		return None;
	}

	let mut pattern = line;
	let mut negated = false;

	if let Some(rest) = pattern.strip_prefix('!') {
		negated = true;
		pattern = rest;
	} else if pattern.starts_with("\\!") {
		pattern = &pattern[1..];
	}

	if let Some(rest) = pattern.strip_prefix('/') {
		pattern = rest;
	}

	let prefixed = if prefix.is_empty() {
		pattern.to_string()
	} else {
		format!("{}{}", prefix, pattern)
	};
	Some(if negated {
		format!("!{}", prefixed)
	} else {
		prefixed
	})
}

fn add_ignore_rules(ig: &mut IgnoreMatcher, dir: &str, root_dir: &str) {
	let relative_dir = node_relative(root_dir, dir);
	let prefix = if relative_dir.is_empty() {
		String::new()
	} else {
		format!("{}/", to_posix_path(&relative_dir))
	};

	for filename in IGNORE_FILE_NAMES {
		let ignore_path = node_path_join(dir, filename);
		// existsSync + readFileSync in try/catch: any failure skips the file.
		let content = match std::fs::read(&ignore_path) {
			Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
			Err(_) => continue,
		};
		let patterns: Vec<String> = content
			.split("\r\n")
			.flat_map(|line| line.split('\n'))
			.filter_map(|line| prefix_ignore_pattern(line, &prefix))
			.collect();
		if !patterns.is_empty() {
			ig.add(&patterns);
		}
	}
}

// ---------------------------------------------------------------------------
// Validation (Agent Skills spec)
// ---------------------------------------------------------------------------

/// Validate skill name per Agent Skills spec.
/// Returns validation error messages (empty if valid).
fn validate_name(name: &str) -> Vec<String> {
	let mut errors: Vec<String> = Vec::new();

	let length = js_length(name);
	if length > MAX_NAME_LENGTH {
		errors.push(format!(
			"name exceeds {} characters ({})",
			MAX_NAME_LENGTH, length
		));
	}

	if name.is_empty()
		|| !name
			.chars()
			.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
	{
		errors.push(
			"name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"
				.to_string(),
		);
	}

	if name.starts_with('-') || name.ends_with('-') {
		errors.push("name must not start or end with a hyphen".to_string());
	}

	if name.contains("--") {
		errors.push("name must not contain consecutive hyphens".to_string());
	}

	errors
}

/// Validate description per Agent Skills spec.
fn validate_description(description: Option<&str>) -> Vec<String> {
	let mut errors: Vec<String> = Vec::new();

	match description {
		Some(desc) if !js_trim(desc).is_empty() => {
			let length = js_length(desc);
			if length > MAX_DESCRIPTION_LENGTH {
				errors.push(format!(
					"description exceeds {} characters ({})",
					MAX_DESCRIPTION_LENGTH, length
				));
			}
		}
		_ => errors.push("description is required".to_string()),
	}

	errors
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// TS `createSkillSourceInfo`.
fn create_skill_source_info(file_path: &str, base_dir: &str, source: &str) -> SourceInfo {
	match source {
		"user" => create_synthetic_source_info(
			file_path,
			Some(SourceScope::User),
			Some(base_dir.to_string()),
		),
		"project" => create_synthetic_source_info(
			file_path,
			Some(SourceScope::Project),
			Some(base_dir.to_string()),
		),
		"path" => {
			create_synthetic_source_info(file_path, None, Some(base_dir.to_string()))
		}
		_ => SourceInfo {
			path: file_path.to_string(),
			source: source.to_string(),
			scope: SourceScope::Temporary,
			origin: SourceOrigin::TopLevel,
			base_dir: Some(base_dir.to_string()),
		},
	}
}

/// `frontmatter["disable-model-invocation"] === true`.
fn frontmatter_get_bool(entries: &[(String, FrontmatterValue)], key: &str) -> bool {
	entries
		.iter()
		.any(|(k, v)| k == key && matches!(v, FrontmatterValue::Bool(true)))
}

/// TS `loadSkillFromFile`.
fn load_skill_from_file(file_path: &str, source: &str) -> (Option<Skill>, Vec<ResourceDiagnostic>) {
	let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();
	let is_declared_skill = basename(file_path) == "SKILL.md";

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

	let (frontmatter, _body) = match parse_frontmatter(&raw_content) {
		Ok(parsed) => parsed,
		Err(message) => {
			if is_declared_skill {
				diagnostics.push(ResourceDiagnostic::warning(
					message,
					Some(file_path.to_string()),
				));
			}
			return (None, diagnostics);
		}
	};

	let description = frontmatter_get_str(&frontmatter, "description");
	let has_description = description.is_some_and(|d| !js_trim(d).is_empty());
	if !is_declared_skill && !has_description {
		return (None, diagnostics);
	}

	let skill_dir = node_dirname(file_path);
	let parent_dir_name = basename(&skill_dir).to_string();

	// Validate description
	for error in validate_description(description) {
		diagnostics.push(ResourceDiagnostic::warning(
			error,
			Some(file_path.to_string()),
		));
	}

	// Use name from frontmatter, or fall back to parent directory name
	// (JS `||`: an empty string is falsy).
	let name = match frontmatter_get_str(&frontmatter, "name") {
		Some(n) if !n.is_empty() => n.to_string(),
		_ => parent_dir_name,
	};

	// Validate name
	for error in validate_name(&name) {
		diagnostics.push(ResourceDiagnostic::warning(
			error,
			Some(file_path.to_string()),
		));
	}

	// Still load the skill even with warnings, unless description is missing
	// or empty.
	if !has_description {
		return (None, diagnostics);
	}

	(
		Some(Skill {
			name,
			description: description.unwrap_or_default().to_string(),
			file_path: file_path.to_string(),
			base_dir: skill_dir.clone(),
			source_info: create_skill_source_info(file_path, &skill_dir, source),
			disable_model_invocation: frontmatter_get_bool(&frontmatter, "disable-model-invocation"),
		}),
		diagnostics,
	)
}

struct DirEntryInfo {
	name: String,
	is_file: bool,
	is_dir: bool,
	is_symlink: bool,
}

/// `readdirSync(dir, { withFileTypes: true })`; `None` on any error (the TS
/// outer `try/catch` then returns the partial, empty result).
fn collect_entries(dir: &str) -> Option<Vec<DirEntryInfo>> {
	let entries = std::fs::read_dir(dir).ok()?;
	let mut out = Vec::new();
	for entry in entries {
		let entry = entry.ok()?;
		let file_type = entry.file_type().ok()?;
		out.push(DirEntryInfo {
			name: entry.file_name().to_string_lossy().into_owned(),
			is_file: file_type.is_file(),
			is_dir: file_type.is_dir(),
			is_symlink: file_type.is_symlink(),
		});
	}
	Some(out)
}

/// Load skills from a directory (public `loadSkillsFromDir`).
///
/// Discovery rules:
/// - if a directory contains SKILL.md, treat it as a skill root and do not
///   recurse further
/// - otherwise, load direct .md children in the root
/// - recurse into subdirectories to find SKILL.md
pub fn load_skills_from_dir(dir: &str, source: &str) -> LoadSkillsResult {
	let mut ig = IgnoreMatcher::default();
	load_skills_from_dir_internal(dir, source, true, &mut ig, dir)
}

fn load_skills_from_dir_internal(
	dir: &str,
	source: &str,
	include_root_files: bool,
	ig: &mut IgnoreMatcher,
	root_dir: &str,
) -> LoadSkillsResult {
	let mut skills: Vec<Skill> = Vec::new();
	let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();

	if std::fs::metadata(dir).is_err() {
		return LoadSkillsResult {
			skills,
			diagnostics,
		};
	}

	add_ignore_rules(ig, dir, root_dir);

	let Some(entries) = collect_entries(dir) else {
		return LoadSkillsResult {
			skills,
			diagnostics,
		};
	};

	for entry in &entries {
		if entry.name != "SKILL.md" {
			continue;
		}

		let full_path = node_path_join(dir, &entry.name);

		let mut is_file = entry.is_file;
		if entry.is_symlink {
			match std::fs::metadata(&full_path) {
				Ok(stats) => is_file = stats.is_file(),
				Err(_) => continue,
			}
		}

		let rel_path = to_posix_path(&node_relative(root_dir, &full_path));
		if !is_file || ig.ignores(&rel_path) {
			continue;
		}

		let (skill, result_diagnostics) = load_skill_from_file(&full_path, source);
		if let Some(skill) = skill {
			skills.push(skill);
		}
		diagnostics.extend(result_diagnostics);
		// Skill root: do not recurse further.
		return LoadSkillsResult {
			skills,
			diagnostics,
		};
	}

	for entry in &entries {
		if entry.name.starts_with('.') {
			continue;
		}

		// Skip node_modules to avoid scanning dependencies
		if entry.name == "node_modules" {
			continue;
		}

		let full_path = node_path_join(dir, &entry.name);

		// For symlinks, check if they point to a directory and follow them
		let mut is_directory = entry.is_dir;
		let mut is_file = entry.is_file;
		if entry.is_symlink {
			match std::fs::metadata(&full_path) {
				Ok(stats) => {
					is_directory = stats.is_dir();
					is_file = stats.is_file();
				}
				// Broken symlink, skip it
				Err(_) => continue,
			}
		}

		let rel_path = to_posix_path(&node_relative(root_dir, &full_path));
		let ignore_path = if is_directory {
			format!("{}/", rel_path)
		} else {
			rel_path
		};
		if ig.ignores(&ignore_path) {
			continue;
		}

		if is_directory {
			let sub_result =
				load_skills_from_dir_internal(&full_path, source, false, ig, root_dir);
			skills.extend(sub_result.skills);
			diagnostics.extend(sub_result.diagnostics);
			continue;
		}

		if !is_file || !include_root_files || !entry.name.ends_with(".md") {
			continue;
		}

		let (skill, result_diagnostics) = load_skill_from_file(&full_path, source);
		if let Some(skill) = skill {
			skills.push(skill);
		}
		diagnostics.extend(result_diagnostics);
	}

	LoadSkillsResult {
		skills,
		diagnostics,
	}
}

/// TS `addSkills` closure: dedupes by canonical path, records name
/// collisions (first skill wins).
#[allow(clippy::too_many_arguments)]
fn add_skills(
	result: LoadSkillsResult,
	skill_map: &mut Vec<Skill>,
	real_path_set: &mut HashSet<String>,
	all_diagnostics: &mut Vec<ResourceDiagnostic>,
	collision_diagnostics: &mut Vec<ResourceDiagnostic>,
) {
	all_diagnostics.extend(result.diagnostics);
	for skill in result.skills {
		// Resolve symlinks to detect duplicate files
		let real_path = canonicalize_path(&skill.file_path);

		// Skip silently if we've already loaded this exact file (via symlink)
		if real_path_set.contains(&real_path) {
			continue;
		}

		if let Some(existing) = skill_map.iter().find(|s| s.name == skill.name) {
			collision_diagnostics.push(ResourceDiagnostic {
				r#type: DiagnosticType::Collision,
				message: format!("name \"{}\" collision", skill.name),
				path: Some(skill.file_path.clone()),
				collision: Some(ResourceCollision {
					resource_type: "skill".to_string(),
					name: skill.name.clone(),
					winner_path: existing.file_path.clone(),
					loser_path: skill.file_path.clone(),
					winner_source: None,
					loser_source: None,
				}),
			});
		} else {
			skill_map.push(skill);
			real_path_set.insert(real_path);
		}
	}
}

/// Load skills from all configured locations.
/// Returns skills and any validation diagnostics.
pub fn load_skills(options: &LoadSkillsOptions) -> LoadSkillsResult {
	let base = PathInputOptions {
		home_dir: options.home_dir.clone(),
		..PathInputOptions::default()
	};
	let resolved_cwd = resolve_path(&options.cwd, ".", &base);
	let resolved_agent_dir = resolve_path(&options.agent_dir, ".", &base);

	let mut skill_map: Vec<Skill> = Vec::new();
	let mut real_path_set: HashSet<String> = HashSet::new();
	let mut all_diagnostics: Vec<ResourceDiagnostic> = Vec::new();
	let mut collision_diagnostics: Vec<ResourceDiagnostic> = Vec::new();

	if options.include_defaults {
		let user_dir = node_path_join(&resolved_agent_dir, "skills");
		let user_result = load_skills_from_dir(&user_dir, "user");
		add_skills(
			user_result,
			&mut skill_map,
			&mut real_path_set,
			&mut all_diagnostics,
			&mut collision_diagnostics,
		);
		let project_dir = resolve_path(
			&format!("{}/{}/skills", resolved_cwd, options.config_dir_name),
			".",
			&base,
		);
		let project_result = load_skills_from_dir(&project_dir, "project");
		add_skills(
			project_result,
			&mut skill_map,
			&mut real_path_set,
			&mut all_diagnostics,
			&mut collision_diagnostics,
		);
	}

	let user_skills_dir = node_path_join(&resolved_agent_dir, "skills");
	let project_skills_dir = resolve_path(
		&format!("{}/{}/skills", resolved_cwd, options.config_dir_name),
		".",
		&base,
	);

	let get_source = |resolved_path: &str| -> &'static str {
		if !options.include_defaults {
			if is_under_path(resolved_path, &user_skills_dir) {
				return "user";
			}
			if is_under_path(resolved_path, &project_skills_dir) {
				return "project";
			}
		}
		"path"
	};

	for raw_path in &options.skill_paths {
		let resolved_path = resolve_path(
			raw_path,
			&resolved_cwd,
			&PathInputOptions {
				trim: true,
				..base.clone()
			},
		);
		if std::fs::metadata(&resolved_path).is_err() {
			all_diagnostics.push(ResourceDiagnostic::warning(
				"skill path does not exist".to_string(),
				Some(resolved_path),
			));
			continue;
		}

		let stats = match std::fs::metadata(&resolved_path) {
			Ok(stats) => stats,
			Err(err) => {
				all_diagnostics.push(ResourceDiagnostic::warning(
					node_error_message("stat", &resolved_path, &err),
					Some(resolved_path),
				));
				continue;
			}
		};
		let source = get_source(&resolved_path);
		if stats.is_dir() {
			let result = load_skills_from_dir(&resolved_path, source);
			add_skills(
				result,
				&mut skill_map,
				&mut real_path_set,
				&mut all_diagnostics,
				&mut collision_diagnostics,
			);
		} else if stats.is_file() && resolved_path.ends_with(".md") {
			let (skill, result_diagnostics) = load_skill_from_file(&resolved_path, source);
			if let Some(skill) = skill {
				add_skills(
					LoadSkillsResult {
						skills: vec![skill],
						diagnostics: result_diagnostics,
					},
					&mut skill_map,
					&mut real_path_set,
					&mut all_diagnostics,
					&mut collision_diagnostics,
				);
			} else {
				all_diagnostics.extend(result_diagnostics);
			}
		} else {
			all_diagnostics.push(ResourceDiagnostic::warning(
				"skill path is not a markdown file".to_string(),
				Some(resolved_path),
			));
		}
	}

	all_diagnostics.extend(collision_diagnostics);
	LoadSkillsResult {
		skills: skill_map,
		diagnostics: all_diagnostics,
	}
}

// ---------------------------------------------------------------------------
// Prompt formatting
// ---------------------------------------------------------------------------

fn escape_xml(s: &str) -> String {
	s.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
		.replace('"', "&quot;")
		.replace('\'', "&apos;")
}

/// Format skills for inclusion in a system prompt.
/// Uses XML format per Agent Skills standard.
/// See: https://agentskills.io/integrate-skills
///
/// Skills with disableModelInvocation=true are excluded from the prompt
/// (they can only be invoked explicitly via /skill:name commands).
///
/// `file_read_tool` names the tool that loads skill files; `indirect` names
/// none, for a reader that is reachable only through another tool.
pub fn format_skills_for_prompt(skills: &[Skill], file_read_tool: FileReadTool) -> String {
	let visible_skills: Vec<&Skill> = skills
		.iter()
		.filter(|s| !s.disable_model_invocation)
		.collect();

	if visible_skills.is_empty() {
		return String::new();
	}

	let mut lines: Vec<String> = vec![
		"\n\nThe following skills provide specialized instructions for specific tasks."
			.to_string(),
		match file_read_tool {
			FileReadTool::Read => "Use the read tool to load a skill's file when the task matches its description.",
			FileReadTool::Bash => "Use bash to load a skill's file when the task matches its description.",
			FileReadTool::Indirect => "Load a skill's file when the task matches its description.",
		}
		.to_string(),
		"When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string(),
		String::new(),
		"<available_skills>".to_string(),
	];

	for skill in visible_skills {
		lines.push("  <skill>".to_string());
		lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
		lines.push(format!(
			"    <description>{}</description>",
			escape_xml(&skill.description)
		));
		lines.push(format!(
			"    <location>{}</location>",
			escape_xml(&skill.file_path)
		));
		lines.push("  </skill>".to_string());
	}

	lines.push("</available_skills>".to_string());

	lines.join("\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
	use super::*;

	fn ignores(patterns: &[&str], path: &str) -> bool {
		let mut ig = IgnoreMatcher::default();
		ig.add(&patterns.iter().map(|s| s.to_string()).collect::<Vec<_>>());
		ig.ignores(path)
	}

	// ----- node-ignore subset -----

	#[test]
	fn ignore_simple_names() {
		assert!(ignores(&["foo"], "foo"));
		assert!(ignores(&["foo"], "foo/"));
		assert!(ignores(&["foo"], "a/foo"));
		assert!(ignores(&["foo"], "a/foo/b"));
		assert!(ignores(&["foo"], "foo/bar"));
		assert!(!ignores(&["foo"], "tempness"));
		assert!(!ignores(&["foo"], "a/fooX"));
	}

	#[test]
	fn ignore_dir_only() {
		assert!(!ignores(&["foo/"], "foo"));
		assert!(ignores(&["foo/"], "foo/"));
		assert!(ignores(&["foo/"], "a/foo/"));
		// paths underneath an ignored directory are ignored (parent walk)
		assert!(ignores(&["foo/"], "foo/bar"));
		assert!(ignores(&["build/"], "x/build/y"));
		assert!(!ignores(&["build/"], "build"));
	}

	#[test]
	fn ignore_negation() {
		assert!(ignores(&["*.log"], "a.log"));
		assert!(!ignores(&["*.log", "!keep.log"], "keep.log"));
		assert!(ignores(&["*.log", "!keep.log"], "b.log"));
		// negation cannot re-include under an excluded directory
		assert!(ignores(&["sub/", "!sub/keep.md"], "sub/keep.md"));
		assert!(ignores(&["sub/", "!sub/keep.md"], "sub/x/y"));
		// a negation before any positive match is skipped
		assert!(!ignores(&["!a", "b"], "a"));
		assert!(ignores(&["!a", "b"], "b"));
	}

	#[test]
	fn ignore_globstars() {
		assert!(ignores(&["**/temp"], "temp"));
		assert!(ignores(&["**/temp"], "a/temp/"));
		assert!(!ignores(&["**/temp"], "tempness"));
		assert!(ignores(&["a/**/b"], "a/b"));
		assert!(ignores(&["a/**/b"], "a/x/y/b"));
		assert!(!ignores(&["a/**/b"], "x/a/b"));
		assert!(ignores(&["a/**"], "a/x"));
		assert!(ignores(&["a/**"], "a/x/y"));
		assert!(!ignores(&["a/**"], "a"));
		assert!(!ignores(&["a/**"], "a/"));
		// `a/**/` is directory-only with at least one segment (node-ignore
		// 7.0.8 behavior, verified against the package)
		assert!(!ignores(&["a/**/"], "a/b"));
		assert!(ignores(&["a/**/"], "a/b/"));
		assert!(ignores(&["a/**/"], "a/b/c"));
		assert!(!ignores(&["a/**/"], "a/"));
		// `a/*` matches `a/x/y` via the trailing boundary lookahead
		assert!(ignores(&["a/*"], "a/x/y"));
		assert!(!ignores(&["a/*"], "a/"));
		assert!(!ignores(&["a/*"], "a"));
	}

	#[test]
	fn ignore_wildcards_and_classes() {
		assert!(ignores(&["[a-z].txt"], "a.txt"));
		assert!(ignores(&["[a-z].txt"], "A.txt")); // case-insensitive
		assert!(!ignores(&["[a-z].txt"], "1.txt"));
		assert!(ignores(&["?at"], "cat"));
		assert!(!ignores(&["?at"], "at"));
		assert!(!ignores(&["?at"], "Cats"));
		assert!(ignores(&["READ*"], "readme.md"));
		assert!(ignores(&["READ*"], "src/README"));
		assert!(ignores(&["a b"], "a b"));
		assert!(!ignores(&["a b"], "a  b"));
		// classes never match a path separator
		assert!(!ignores(&["a[/.]b"], "a/b"));
		// unterminated class matches nothing
		assert!(!ignores(&["a[bc"], "abc"));
	}

	#[test]
	fn ignore_anchored_and_escaped() {
		// inner slash anchors the pattern to the root
		assert!(ignores(&["a/b"], "a/b"));
		assert!(!ignores(&["a/b"], "x/a/b"));
		// escaped wildcard is literal
		assert!(!ignores(&["a\\*"], "ax"));
		assert!(ignores(&["a\\*"], "a*"));
		// trailing unescaped spaces are trimmed
		assert!(ignores(&["foo  "], "foo"));
		// escaped trailing space is kept
		assert!(!ignores(&["foo\\ "], "foo"));
		assert!(ignores(&["foo\\ "], "foo "));
	}

	#[test]
	fn ignore_check_pattern_filtering() {
		// comments, blanks and lone trailing backslashes are dropped by add()
		assert!(!ignores(&["# comment", "", "a\\"], "a"));
		assert!(!ignores(&["# comment"], "# comment"));
		// `\!` unescapes to a literal `!`-named pattern (not a negation)
		assert!(!ignores(&["\\!important"], "important"));
	}

	#[test]
	fn prefix_ignore_pattern_rules() {
		assert_eq!(prefix_ignore_pattern("build/", "sub/").as_deref(), Some("sub/build/"));
		assert_eq!(prefix_ignore_pattern("!keep.md", "sub/").as_deref(), Some("!sub/keep.md"));
		assert_eq!(prefix_ignore_pattern("\\!x", "").as_deref(), Some("!x"));
		assert_eq!(prefix_ignore_pattern("/abs", "").as_deref(), Some("abs"));
		assert_eq!(prefix_ignore_pattern("  ", "").as_deref(), None);
		assert_eq!(prefix_ignore_pattern("# c", "").as_deref(), None);
		assert_eq!(prefix_ignore_pattern("\\#hash", "").as_deref(), Some("\\#hash"));
	}

	#[test]
	fn parent_of_paths() {
		assert_eq!(parent_of("a/b/c"), "a/b/");
		assert_eq!(parent_of("a/b/"), "a/");
		assert_eq!(parent_of("a/"), "");
		assert_eq!(parent_of("a"), "");
		assert_eq!(parent_of("a//b"), "a/");
	}

	// ----- loading -----

	fn scratch(tag: &str) -> std::path::PathBuf {
		use std::sync::atomic::{AtomicUsize, Ordering};
		static COUNTER: AtomicUsize = AtomicUsize::new(0);
		let n = COUNTER.fetch_add(1, Ordering::SeqCst);
		let dir = std::env::temp_dir().join(format!(
			"pi-core-skills-{}-{}-{}",
			std::process::id(),
			tag,
			n
		));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).unwrap();
		dir
	}

	fn write(path: impl AsRef<std::path::Path>, content: &str) {
		std::fs::write(path, content).unwrap();
	}

	fn opts(cwd: &str, agent_dir: &str, skill_paths: &[&str], include_defaults: bool) -> LoadSkillsOptions {
		LoadSkillsOptions {
			cwd: cwd.to_string(),
			agent_dir: agent_dir.to_string(),
			skill_paths: skill_paths.iter().map(|s| s.to_string()).collect(),
			include_defaults,
			config_dir_name: ".pi".to_string(),
			home_dir: None,
		}
	}

	#[test]
	fn load_skill_from_file_basic() {
		let dir = scratch("basic");
		let skill_dir = dir.join("my-skill");
		std::fs::create_dir_all(&skill_dir).unwrap();
		write(
			skill_dir.join("SKILL.md"),
			"---\nname: my-skill\ndescription: Does things\n---\n# body\n",
		);
		let path = skill_dir.join("SKILL.md").to_string_lossy().into_owned();
		let (skill, diagnostics) = load_skill_from_file(&path, "user");
		assert!(diagnostics.is_empty());
		let skill = skill.unwrap();
		assert_eq!(skill.name, "my-skill");
		assert_eq!(skill.description, "Does things");
		assert_eq!(skill.base_dir, skill_dir.to_string_lossy());
		assert!(!skill.disable_model_invocation);
		assert_eq!(skill.source_info.source, "local");
		assert_eq!(skill.source_info.scope, SourceScope::User);
	}

	#[test]
	fn load_skill_name_fallback_and_flags() {
		let dir = scratch("fallback");
		let skill_dir = dir.join("parent-name");
		std::fs::create_dir_all(&skill_dir).unwrap();
		write(
			skill_dir.join("SKILL.md"),
			"---\ndescription: D\ndisable-model-invocation: true\n---\nbody\n",
		);
		let path = skill_dir.join("SKILL.md").to_string_lossy().into_owned();
		let (skill, diagnostics) = load_skill_from_file(&path, "path");
		assert!(diagnostics.is_empty());
		let skill = skill.unwrap();
		// name falls back to the parent directory name
		assert_eq!(skill.name, "parent-name");
		assert!(skill.disable_model_invocation);
		assert_eq!(skill.source_info.scope, SourceScope::Temporary);
		// non-boolean value does not count as true
		write(skill_dir.join("SKILL.md"), "---\ndescription: D\ndisable-model-invocation: yes\n---\nb\n");
		let (skill, _) = load_skill_from_file(&path, "path");
		assert!(!skill.unwrap().disable_model_invocation);
	}

	#[test]
	fn load_skill_validation_errors() {
		let dir = scratch("validation");
		let skill_dir = dir.join("Bad_Name");
		std::fs::create_dir_all(&skill_dir).unwrap();
		write(
			skill_dir.join("SKILL.md"),
			"---\nname: Bad_Name\ndescription: D\n---\nb\n",
		);
		let path = skill_dir.join("SKILL.md").to_string_lossy().into_owned();
		let (skill, diagnostics) = load_skill_from_file(&path, "user");
		// still loaded, with warnings
		assert!(skill.is_some());
		assert_eq!(diagnostics.len(), 1);
		assert_eq!(
			diagnostics[0].message,
			"name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"
		);

		// long name + bad name
		let long = "a".repeat(65);
		write(
			skill_dir.join("SKILL.md"),
			&format!("---\nname: {long}--x\ndescription: D\n---\nb\n"),
		);
		let (skill, diagnostics) = load_skill_from_file(&path, "user");
		assert!(skill.is_some());
		let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
		assert!(messages.contains(&"name exceeds 64 characters (68)"));
		assert!(messages.contains(&"name must not contain consecutive hyphens"));

		// missing description on a declared skill: warning, no skill
		write(skill_dir.join("SKILL.md"), "---\nname: ok\n---\nb\n");
		let (skill, diagnostics) = load_skill_from_file(&path, "user");
		assert!(skill.is_none());
		assert_eq!(diagnostics.len(), 1);
		assert_eq!(diagnostics[0].message, "description is required");

		// long description
		let long_desc = "d".repeat(1025);
		write(
			skill_dir.join("SKILL.md"),
			&format!("---\nname: ok\ndescription: {long_desc}\n---\nb\n"),
		);
		let (skill, diagnostics) = load_skill_from_file(&path, "user");
		assert!(skill.is_some());
		assert_eq!(
			diagnostics[0].message,
			"description exceeds 1024 characters (1025)"
		);
	}

	#[test]
	fn undeclared_md_without_description_is_silent() {
		let dir = scratch("undeclared");
		write(dir.join("note.md"), "# Just a note\n");
		let path = dir.join("note.md").to_string_lossy().into_owned();
		let (skill, diagnostics) = load_skill_from_file(&path, "path");
		assert!(skill.is_none());
		assert!(diagnostics.is_empty());
		// with a description it loads (name from filename minus nothing —
		// parent dir name, since frontmatter name is absent)
		write(dir.join("note.md"), "---\ndescription: has one\n---\nbody\n");
		let (skill, diagnostics) = load_skill_from_file(&path, "path");
		assert!(diagnostics.is_empty());
		let skill = skill.unwrap();
		// name falls back to the parent directory name
		assert_eq!(skill.name, dir.to_string_lossy().rsplit('/').next().unwrap());
	}

	#[test]
	fn declared_skill_parse_error_diagnostic() {
		let dir = scratch("parseerr");
		let skill_dir = dir.join("broken");
		std::fs::create_dir_all(&skill_dir).unwrap();
		// duplicate keys are a yaml error (probed parser reproduces it)
		write(
			skill_dir.join("SKILL.md"),
			"---\nname: a\nname: b\ndescription: D\n---\nb\n",
		);
		let path = skill_dir.join("SKILL.md").to_string_lossy().into_owned();
		let (skill, diagnostics) = load_skill_from_file(&path, "user");
		assert!(skill.is_none());
		assert_eq!(diagnostics.len(), 1);
		assert!(diagnostics[0].message.contains("Map keys must be unique"));
		// the same malformed file NOT named SKILL.md is silent
		write(dir.join("broken.md"), "---\nname: a\nname: b\n---\nb\n");
		let path2 = dir.join("broken.md").to_string_lossy().into_owned();
		let (skill, diagnostics) = load_skill_from_file(&path2, "user");
		assert!(skill.is_none());
		assert!(diagnostics.is_empty());
	}

	#[test]
	fn dir_scan_discovery_rules() {
		let dir = scratch("scan");
		// root SKILL.md wins and stops recursion
		write(dir.join("SKILL.md"), "---\ndescription: root\n---\nb\n");
		std::fs::create_dir_all(dir.join("nested")).unwrap();
		write(dir.join("nested").join("SKILL.md"), "---\ndescription: nested\n---\nb\n");
		write(dir.join("plain.md"), "---\ndescription: plain\n---\nb\n");
		let result = load_skills_from_dir(&dir.to_string_lossy(), "path");
		assert_eq!(result.skills.len(), 1);
		assert_eq!(result.skills[0].description, "root");

		// without a root SKILL.md: root .md files + nested SKILL.md dirs
		std::fs::remove_file(dir.join("SKILL.md")).unwrap();
		write(dir.join(".hidden.md"), "---\ndescription: h\n---\nb\n");
		std::fs::create_dir_all(dir.join("node_modules")).unwrap();
		write(dir.join("node_modules").join("dep.md"), "---\ndescription: n\n---\nb\n");
		std::fs::create_dir_all(dir.join("deep").join("inner")).unwrap();
		write(
			dir.join("deep").join("inner").join("SKILL.md"),
			"---\ndescription: deep\n---\nb\n",
		);
		write(dir.join("deep").join("notaskill.md"), "---\ndescription: x\n---\nb\n");
		let result = load_skills_from_dir(&dir.to_string_lossy(), "path");
		let descriptions: Vec<&str> = result.skills.iter().map(|s| s.description.as_str()).collect();
		assert!(descriptions.contains(&"plain"));
		assert!(descriptions.contains(&"deep"));
		// dot files and node_modules are skipped; nested non-SKILL .md files
		// are not loaded (includeRootFiles=false when recursing)
		assert!(!descriptions.contains(&"h"));
		assert!(!descriptions.contains(&"n"));
		assert!(!descriptions.contains(&"x"));
	}

	#[test]
	fn dir_scan_respects_ignore_files() {
		let dir = scratch("ignorefiles");
		std::fs::create_dir_all(dir.join("excluded")).unwrap();
		std::fs::create_dir_all(dir.join("kept")).unwrap();
		write(dir.join("excluded").join("SKILL.md"), "---\ndescription: no\n---\nb\n");
		write(dir.join("kept").join("SKILL.md"), "---\ndescription: yes\n---\nb\n");
		write(dir.join(".gitignore"), "excluded/\n");
		let result = load_skills_from_dir(&dir.to_string_lossy(), "path");
		assert_eq!(result.skills.len(), 1);
		assert_eq!(result.skills[0].description, "yes");

		// nested ignore file with dir-relative pattern
		let dir2 = scratch("ignorefiles2");
		std::fs::create_dir_all(dir2.join("sub").join("ex")).unwrap();
		std::fs::create_dir_all(dir2.join("sub").join("ok")).unwrap();
		write(dir2.join("sub").join("ex").join("SKILL.md"), "---\ndescription: no\n---\nb\n");
		write(dir2.join("sub").join("ok").join("SKILL.md"), "---\ndescription: yes\n---\nb\n");
		write(dir2.join("sub").join(".gitignore"), "ex/\n");
		let result = load_skills_from_dir(&dir2.to_string_lossy(), "path");
		assert_eq!(result.skills.len(), 1);
		assert_eq!(result.skills[0].description, "yes");
	}

	#[test]
	fn load_skills_explicit_paths_and_collisions() {
		let dir = scratch("explicit");
		let a = dir.join("a");
		let b = dir.join("b");
		std::fs::create_dir_all(&a).unwrap();
		std::fs::create_dir_all(&b).unwrap();
		write(a.join("SKILL.md"), "---\nname: dup\ndescription: first\n---\nb\n");
		write(b.join("SKILL.md"), "---\nname: dup\ndescription: second\n---\nb\n");
		write(dir.join("notmd.txt"), "x");

		let a_str = a.to_string_lossy().into_owned();
		let b_str = b.to_string_lossy().into_owned();

		// missing path
		let mut result = load_skills(&opts(&dir.to_string_lossy(), "/nonexistent-agent", &["/does/not/exist"], false));
		assert!(result.skills.is_empty());
		assert_eq!(result.diagnostics.len(), 1);
		assert_eq!(result.diagnostics[0].message, "skill path does not exist");

		// not a markdown file
		let notmd = dir.join("notmd.txt").to_string_lossy().into_owned();
		result = load_skills(&opts(&dir.to_string_lossy(), "/nonexistent-agent", &[&notmd], false));
		assert_eq!(result.diagnostics[0].message, "skill path is not a markdown file");

		// collisions: first wins, loser recorded as collision diagnostic
		result = load_skills(&opts(&dir.to_string_lossy(), "/nonexistent-agent", &[&a_str, &b_str], false));
		assert_eq!(result.skills.len(), 1);
		assert_eq!(result.skills[0].description, "first");
		assert_eq!(result.diagnostics.len(), 1);
		let d = &result.diagnostics[0];
		assert_eq!(d.r#type, DiagnosticType::Collision);
		assert_eq!(d.message, "name \"dup\" collision");
		let collision = d.collision.as_ref().unwrap();
		assert_eq!(collision.resource_type, "skill");
		assert_eq!(collision.winner_path, format!("{}/SKILL.md", a_str));
		assert_eq!(collision.loser_path, format!("{}/SKILL.md", b_str));

		// source classification: outside defaults → "path" (source "local",
		// scope temporary)
		assert_eq!(result.skills[0].source_info.source, "local");
		assert_eq!(result.skills[0].source_info.scope, SourceScope::Temporary);
	}

	#[test]
	fn load_skills_defaults_and_sources() {
		let dir = scratch("defaults");
		let agent = dir.join("agent");
		let project = dir.join("project");
		std::fs::create_dir_all(agent.join("skills").join("user-skill")).unwrap();
		std::fs::create_dir_all(project.join(".pi").join("skills").join("proj-skill")).unwrap();
		write(
			agent.join("skills").join("user-skill").join("SKILL.md"),
			"---\ndescription: user skill\n---\nb\n",
		);
		write(
			project.join(".pi").join("skills").join("proj-skill").join("SKILL.md"),
			"---\ndescription: project skill\n---\nb\n",
		);

		let result = load_skills(&opts(
			&project.to_string_lossy(),
			&agent.to_string_lossy(),
			&[],
			true,
		));
		assert_eq!(result.skills.len(), 2);
		let by_desc = |d: &str| {
			result
				.skills
				.iter()
				.find(|s| s.description == d)
				.unwrap()
				.clone()
		};
		let user = by_desc("user skill");
		let proj = by_desc("project skill");
		assert_eq!(user.source_info.scope, SourceScope::User);
		assert_eq!(proj.source_info.scope, SourceScope::Project);
		assert_eq!(user.name, "user-skill");
		assert_eq!(proj.name, "proj-skill");

		// an explicit skill path under the user dir classifies as "user"
		// only when include_defaults is false (TS getSource)
		let user_skill_md = agent
			.join("skills")
			.join("user-skill")
			.join("SKILL.md")
			.to_string_lossy()
			.into_owned();
		let result = load_skills(&opts(
			&project.to_string_lossy(),
			&agent.to_string_lossy(),
			&[&user_skill_md],
			false,
		));
		assert_eq!(result.skills[0].source_info.scope, SourceScope::User);
	}

	#[test]
	fn load_skills_missing_dir_is_empty() {
		let result = load_skills_from_dir("/does/not/exist", "user");
		assert!(result.skills.is_empty());
		assert!(result.diagnostics.is_empty());
	}

	// ----- prompt formatting -----

	fn skill(name: &str, description: &str, disable: bool) -> Skill {
		Skill {
			name: name.to_string(),
			description: description.to_string(),
			file_path: format!("/x/{}/SKILL.md", name),
			base_dir: format!("/x/{}", name),
			source_info: SourceInfo {
				path: format!("/x/{}/SKILL.md", name),
				source: "local".to_string(),
				scope: SourceScope::User,
				origin: SourceOrigin::TopLevel,
				base_dir: None,
			},
			disable_model_invocation: disable,
		}
	}

	#[test]
	fn format_skills_empty() {
		assert_eq!(format_skills_for_prompt(&[], FileReadTool::Read), "");
		assert_eq!(
			format_skills_for_prompt(&[skill("a", "d", true)], FileReadTool::Read),
			""
		);
	}

	#[test]
	fn format_skills_prompt() {
		let skills = vec![
			skill("deploy", "Deploys & <things>", false),
			skill("hidden", "no", true),
		];
		let out = format_skills_for_prompt(&skills, FileReadTool::Read);
		assert!(out.starts_with("\n\nThe following skills provide specialized instructions for specific tasks.\n"));
		assert!(out.contains("Use the read tool to load a skill's file when the task matches its description.\n"));
		assert!(out.contains("When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.\n\n<available_skills>\n"));
		assert!(out.contains("    <name>deploy</name>"));
		assert!(out.contains("    <description>Deploys &amp; &lt;things&gt;</description>"));
		assert!(out.contains("    <location>/x/deploy/SKILL.md</location>"));
		assert!(out.ends_with("  </skill>\n</available_skills>"));
		assert!(!out.contains("hidden"));

		let bash = format_skills_for_prompt(&skills, FileReadTool::Bash);
		assert!(bash.contains("Use bash to load a skill's file when the task matches its description."));
		let indirect = format_skills_for_prompt(&skills, FileReadTool::Indirect);
		assert!(indirect.contains("Load a skill's file when the task matches its description."));
	}

	#[test]
		fn escape_xml_order() {
		assert_eq!(escape_xml("&<>\"'"), "&amp;&lt;&gt;&quot;&apos;");
	}
}
