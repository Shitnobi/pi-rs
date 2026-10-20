//! A `minimatch` 10.x-compatible glob matcher (subset).
//!
//! Port for `model-resolver.ts` scope patterns (`--models` globs), which call
//! `minimatch(text, pattern, { nocase: true })` from the `minimatch` npm
//! package (probed version: 10.2.6, see `/tmp/pi-repo`). All behavior below
//! was probed against the real package (unit tests replicate the probed
//! true/false table) rather than reimplemented from memory.
//!
//! Supported:
//! - whole-string match (`*`/`?` never cross `/`)
//! - `**` as a whole path segment crosses any number of segments (a trailing
//!   `/**` requires at least one segment; a bare `*`-only segment never
//!   matches an empty segment — both probed)
//! - dotfile rule: a segment whose pattern opens with a wildcard (`*`, `?`,
//!   multi-member class) cannot match text starting with `.` at all
//!   (minimatch's `(?!\.)` prefix); one or two literal dots followed by a
//!   wildcard cannot match exactly `.` / `..` (traversal guard); an extglob
//!   body opening with a wildcard cannot consume a leading `.`; `**` skips
//!   dot-leading segments
//! - `[...]` classes with ranges, `[!...]`/`[^...]` negation, a leading `]`
//!   member and `\` escapes; an inverted range (`[c-a]`) matches nothing
//! - brace expansion `{a,b}` incl. nesting, and `{m..n}` numeric /
//!   single-char ranges
//! - extglob `+(...)`, `*(...)`, `?(...)`, `@(...)` with `|` alternatives
//! - `\` escapes, `nocase` (per-char Unicode lowercase folding), leading `!`
//!   negation
//!
//! Documented divergences from minimatch 10.2.6:
//! - `!(...)` uses "any run not exactly equal to one alternative" semantics
//!   (minimatch compiles a per-position negative lookahead); agrees with all
//!   probed cases but may differ on exotic patterns. Parity cases avoid `!()`.
//!   A leading `!(` is eaten by minimatch's negation handling (pattern
//!   negation, not extglob) — reproduced here by the leading-`!` rule.
//! - `{a..b..step}` ranges are not expanded (treated literally).
//! - Empty path segments are not normalized: `a//b` vs `a/b` is `false` here
//!   (minimatch 10 says `true`); a pattern `**/a` does not match text `a/`
//!   (minimatch's trailing-empty-segment affordance). Parity ids carry no
//!   trailing/duplicated slashes.
//! - `nocase` folds via per-char `to_lowercase` (JS `toLowerCase` semantics);
//!   regex `i`-flag oddities (`ſ` ↔ `s`, Kelvin sign) differ.
//! - POSIX classes (`[:alpha:]`) are not recognized (parsed as ordinary
//!   classes); minimatch compiles them to `\p{...}`. Parity cases avoid them.

// ============================================================================
// Entry point
// ============================================================================

/// Whole-string glob match, mirroring `minimatch(text, pattern, { nocase })`.
pub fn minimatch(text: &str, pattern: &str, nocase: bool) -> bool {
	// A leading `!` negates the pattern (parity cases avoid these).
	if let Some(rest) = pattern.strip_prefix('!') {
		return !minimatch(text, rest, nocase);
	}
	brace_expand(pattern).iter().any(|expanded| match_expanded(text, expanded, nocase))
}

fn match_expanded(text: &str, pattern: &str, nocase: bool) -> bool {
	let tsegs = split_segments(text);
	let psegs: Vec<Seg> = split_segments(pattern)
		.into_iter()
		.map(|seg| parse_seg(&seg, nocase))
		.collect();
	match_seg_list(&psegs, 0, &tsegs, 0, nocase)
}

/// JS `s.split(/\/+/)`: runs of slashes coalesce (minimatch's
/// `slashSplit` on POSIX); the empty string yields a single empty segment.
fn split_segments(s: &str) -> Vec<String> {
	if s.is_empty() {
		return vec![String::new()];
	}
	let mut out: Vec<String> = Vec::new();
	let mut cur = String::new();
	let mut chars = s.chars().peekable();
	while let Some(c) = chars.next() {
		if c == '/' {
			while chars.peek() == Some(&'/') {
				chars.next();
			}
			out.push(std::mem::take(&mut cur));
		} else {
			cur.push(c);
		}
	}
	out.push(cur);
	out
}

// ============================================================================
// Pattern AST
// ============================================================================

#[derive(Debug, Clone)]
enum Seg {
	/// A segment consisting only of `*`s (length ≥ 2): globstar.
	Globstar,
	Tokens(Vec<Token>),
}

#[derive(Debug, Clone)]
enum Token {
	Literal(char),
	/// `?` — one char (never `/`, which cannot occur inside a segment).
	AnyChar,
	/// `*` — a run of non-`/` chars (consecutive `*`s collapse: `a**b` = `a*b`).
	Star,
	Class(ClassSpec),
	Ext(ExtSpec),
}

#[derive(Debug, Clone)]
struct ClassSpec {
	negated: bool,
	members: Vec<char>,
	ranges: Vec<(char, char)>,
	/// Inverted range (`[c-a]`): the class matches nothing.
	empty: bool,
	/// The class is a single positive literal character (minimatch compiles
	/// `[x]` / `[x-x]` to a plain literal, so it does not open the segment
	/// with a `[`-wildcard and never triggers the dot guards).
	single_literal: Option<char>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtKind {
	/// `+(...)` — one or more repetitions.
	Plus,
	/// `*(...)` — zero or more repetitions.
	Star,
	/// `?(...)` — zero or one repetition.
	Question,
	/// `@(...)` — exactly one alternative.
	At,
	/// `!(...)` — any run not exactly equal to an alternative (approximate).
	Bang,
}

#[derive(Debug, Clone)]
struct ExtSpec {
	kind: ExtKind,
	/// Alternatives (each a token list), split on top-level `|`.
	alts: Vec<Vec<Token>>,
	/// Dot-guard flag: extglobs whose first alternative does not open with a
	/// literal `.` (or a `.`-opening class) cannot consume a leading `.`.
	guarded: bool,
}

fn parse_seg(seg: &str, nocase: bool) -> Seg {
	// Only an exact `**` segment is a globstar (minimatch returns GLOBSTAR
	// for exactly `**`; `***` and longer runs coalesce to a single `*`,
	// probed: `a/b` vs `***` is false).
	if seg == "**" {
		return Seg::Globstar;
	}
	Seg::Tokens(parse_tokens(seg, nocase))
}

/// A token whose minimatch regex source opens with `[` (wildcard or
/// multi-member class), meaning a segment starting with it gets the
/// "must not start with `.`" dot guard.
fn opens_with_wildcard_src(token: &Token) -> bool {
	match token {
		Token::Star | Token::AnyChar => true,
		// A single positive literal class (`[x]`, `[x-x]`) compiles to a
		// plain literal, so it does not trigger the guard.
		Token::Class(class) => class.single_literal.is_none(),
		_ => false,
	}
}

/// Segment-level dot guard (`(?!\.)` prefix): a segment whose pattern opens
/// with a wildcard (regex source starting with `[`) cannot match text that
/// starts with `.` at all — regardless of which token would consume the dot
/// (probed: `*.txt` vs `.txt` is false).
fn seg_dot_guard(tokens: &[Token]) -> bool {
	tokens.first().is_some_and(opens_with_wildcard_src)
}

/// Segment-level traversal guard (`(?!(?:^|/)\.\.?(?:$|/))` prefix):
/// a pattern of one or two literal dots followed by a `[`-opening wildcard
/// cannot match a segment that is exactly `.` or `..` (probed: `.*` vs `..`
/// is false, but `.*` vs `...` and `.[.]` vs `..` are true).
fn seg_trav_guard(tokens: &[Token]) -> bool {
	// A leading `.`-regex-source unit: a literal dot or a single-literal
	// `.`-class (`[.]` compiles to `\.`, probed: `[.]**?(a)` vs `.` is
	// false via the traversal guard).
	fn is_dot_src(token: &Token) -> bool {
		match token {
			Token::Literal('.') => true,
			Token::Class(class) => class.single_literal == Some('.'),
			_ => false,
		}
	}
	let dots = tokens.iter().take_while(|token| is_dot_src(token)).count();
	(dots == 1 || dots == 2) && tokens.get(dots).is_some_and(opens_with_wildcard_src)
}

fn parse_tokens(s: &str, nocase: bool) -> Vec<Token> {
	let chars: Vec<char> = s.chars().collect();
	let mut tokens = Vec::new();
	let mut i = 0;
	while i < chars.len() {
		let c = chars[i];
		let next_is_paren = i + 1 < chars.len() && chars[i + 1] == '(';
		match c {
			'\\' => {
				if i + 1 < chars.len() {
					tokens.push(Token::Literal(chars[i + 1]));
					i += 2;
				} else {
					tokens.push(Token::Literal('\\'));
					i += 1;
				}
			}
			_ if next_is_paren && matches!(c, '+' | '*' | '?' | '@' | '!') => {
				match parse_ext(&chars, i, nocase) {
					Some((ext, next)) => {
						tokens.push(Token::Ext(ext));
						i = next;
					}
					None => {
						tokens.push(Token::Literal(c));
						i += 1;
					}
				}
			}
			'*' => {
				// Consecutive `*`s coalesce (`a**b` = `a*b`), but a `*` that
				// directly precedes `(` opens an extglob (`***(a)` is
				// `*` + `*(a)` in minimatch's scanner, not a star run).
				let mut run_end = i;
				while run_end < chars.len() && chars[run_end] == '*' {
					run_end += 1;
				}
				if run_end < chars.len() && chars[run_end] == '(' {
					run_end -= 1;
				}
				if run_end > i {
					tokens.push(Token::Star);
				}
				i = run_end;
			}
			'?' => {
				tokens.push(Token::AnyChar);
				i += 1;
			}
			'[' => match parse_class(&chars, i) {
				Some((class, next)) => {
					tokens.push(Token::Class(class));
					i = next;
				}
				None => {
					tokens.push(Token::Literal('['));
					i += 1;
				}
			},
			other => {
				tokens.push(Token::Literal(other));
				i += 1;
			}
		}
	}
	if nocase {
		// Fold class members/ranges so nocase class matching is symmetric,
		// like a regex `i` flag (probed: `[A-C]` matches `b` and `[a-c]`
		// matches `B`; `[!a]` does not match `A` — the folded members are
		// excluded before negation is applied).
		for token in &mut tokens {
			if let Token::Class(class) = token {
				let mut extra_members: Vec<char> = Vec::new();
				for &m in &class.members {
					for v in case_variants(m) {
						if !class.members.contains(&v) && !extra_members.contains(&v) {
							extra_members.push(v);
						}
					}
				}
				let mut extra_ranges: Vec<(char, char)> = Vec::new();
				for &(a, b) in &class.ranges {
					for (fa, fb) in case_variants(a).into_iter().zip(case_variants(b)) {
						if !class.ranges.contains(&(fa, fb)) && !extra_ranges.contains(&(fa, fb)) {
							extra_ranges.push((fa, fb));
						}
					}
				}
				class.members.extend(extra_members);
				class.ranges.extend(extra_ranges);
			}
		}
	}
	tokens
}

/// Single-char case variants of `c` (itself plus one-char lowercase and
/// uppercase folds, when they exist).
fn case_variants(c: char) -> Vec<char> {
	let mut out = vec![c];
	if let Some(lower) = fold_char(c) {
		if !out.contains(&lower) {
			out.push(lower);
		}
	}
	let mut upper = c.to_uppercase();
	if let (Some(u), None) = (upper.next(), upper.next()) {
		if !out.contains(&u) {
			out.push(u);
		}
	}
	out
}

/// Parse a `[...]` class starting at `chars[start] == '['`.
/// Returns `None` when the class is unclosed (the `[` stays a literal).
fn parse_class(chars: &[char], start: usize) -> Option<(ClassSpec, usize)> {
	let mut i = start + 1;
	let negated = i < chars.len() && (chars[i] == '!' || chars[i] == '^');
	if negated {
		i += 1;
	}
	let mut members: Vec<char> = Vec::new();
	let mut ranges: Vec<(char, char)> = Vec::new();
	let mut empty = false;
	// A `]` immediately after `[` or `[!` is a literal member (`a[]]b`).
	if i < chars.len() && chars[i] == ']' {
		members.push(']');
		i += 1;
	}
	while i < chars.len() && chars[i] != ']' {
		let c = if chars[i] == '\\' && i + 1 < chars.len() {
			i += 1;
			chars[i]
		} else {
			chars[i]
		};
		// Range `a-z`: a `-` between two non-`]` chars; a `-` at the end of
		// the class is a literal member (`[a-]`).
		if i + 2 < chars.len() && chars[i + 1] == '-' && chars[i + 2] != ']' {
			let end = if chars[i + 2] == '\\' && i + 3 < chars.len() {
				chars[i + 3]
			} else {
				chars[i + 2]
			};
			if c > end {
				// Inverted range (`[c-a]`) matches nothing (probed).
				empty = true;
			} else {
				ranges.push((c, end));
			}
			i += if chars[i + 2] == '\\' { 4 } else { 3 };
			continue;
		}
		members.push(c);
		i += 1;
	}
	if i >= chars.len() {
		return None; // unclosed
	}
	let spec = ClassSpec {
		negated,
		single_literal: if !negated && !empty && ranges.is_empty() && members.len() == 1 {
			Some(members[0])
		} else if !negated && !empty && members.is_empty() && ranges.len() == 1 && ranges[0].0 == ranges[0].1 {
			Some(ranges[0].0)
		} else {
			None
		},
		members,
		ranges,
		empty,
	};
	Some((spec, i + 1))
}

impl ClassSpec {
	fn matches(&self, c: char) -> bool {
		let hit = if self.empty {
			false
		} else {
			self.members.contains(&c) || self.ranges.iter().any(|(a, b)| *a <= c && c <= *b)
		};
		if self.negated {
			!hit
		} else {
			hit
		}
	}
}

/// Parse an extglob group starting at `chars[start]` (one of `+?*@!`
/// followed by `(`). Returns `None` when the group is unclosed.
fn parse_ext(chars: &[char], start: usize, nocase: bool) -> Option<(ExtSpec, usize)> {
	let kind = match chars[start] {
		'+' => ExtKind::Plus,
		'*' => ExtKind::Star,
		'?' => ExtKind::Question,
		'@' => ExtKind::At,
		_ => ExtKind::Bang,
	};
	let mut alts: Vec<Vec<char>> = vec![Vec::new()];
	let mut depth = 1;
	let mut j = start + 2; // past '('
	let mut closed = None;
	while j < chars.len() {
		match chars[j] {
			'\\' => {
				if j + 1 < chars.len() {
					alts.last_mut().unwrap().push(chars[j]);
					alts.last_mut().unwrap().push(chars[j + 1]);
				}
				j += 2;
				continue;
			}
			'(' => {
				depth += 1;
				alts.last_mut().unwrap().push('(');
			}
			')' => {
				depth -= 1;
				if depth == 0 {
					closed = Some(j);
					break;
				}
				alts.last_mut().unwrap().push(')');
			}
			'|' if depth == 1 => alts.push(Vec::new()),
			c => alts.last_mut().unwrap().push(c),
		}
		j += 1;
	}
	let close = closed?;
	let alts: Vec<Vec<Token>> = alts
		.into_iter()
		.map(|alt| parse_tokens(&alt.into_iter().collect::<String>(), nocase))
		.collect();
	// The extglob body carries its own dot guard when it opens with a
	// wildcard; minimatch leaves it off when the first alternative opens
	// with a literal `.` or a single-literal `.`-class (probed: `*(.a)`,
	// `@(.x)`, `?([.])x` match leading-dot text; `*(?)` does not). A `!(...)`
	// group is always guarded: its compiled close puts `(?!\.)` outside the
	// body (probed: `!(.x)y` does not match `.y`).
	let guarded = kind == ExtKind::Bang
		|| !alts
			.first()
			.and_then(|alt| alt.first())
			.is_some_and(|token| match token {
				Token::Literal('.') => true,
				Token::Class(class) => class.single_literal == Some('.'),
				_ => false,
			});
	Some((ExtSpec { kind, alts, guarded }, close + 1))
}

// ============================================================================
// Matching
// ============================================================================

fn match_seg_list(psegs: &[Seg], pi: usize, tsegs: &[String], ti: usize, nocase: bool) -> bool {
	if pi == psegs.len() {
		// Pattern exhausted: an exact end, or a single trailing empty text
		// segment (minimatch's `a/*` matches `a/b/` affordance).
		return ti == tsegs.len() || (ti + 1 == tsegs.len() && tsegs[ti].is_empty());
	}
	match &psegs[pi] {
		Seg::Globstar => {
			// A trailing `/**` requires at least one remaining segment
			// (`a/**` vs `a` is false, vs `a/` is true — both probed); a
			// non-trailing `**` may match zero segments (`a/**/b` vs `a/b`).
			// Consumed segments must not start with `.` (dot rule).
			let min = if pi + 1 == psegs.len() { 1 } else { 0 };
			for k in min..=(tsegs.len().saturating_sub(ti)) {
				if tsegs[ti..ti + k].iter().all(|s| !s.starts_with('.'))
					&& match_seg_list(psegs, pi + 1, tsegs, ti + k, nocase)
				{
					return true;
				}
			}
			false
		}
		Seg::Tokens(tokens) => {
			if ti >= tsegs.len() {
				return false;
			}
			let seg: &str = &tsegs[ti];
			let chars: Vec<char> = seg.chars().collect();
			// A bare `*`-only segment never matches an empty segment
			// (`a/*` vs `a/` is false — probed; `a*` vs `a` is true).
			if chars.is_empty() && tokens.len() == 1 && matches!(tokens[0], Token::Star) {
				return false;
			}
			// Segment-level guards (see `seg_dot_guard` / `seg_trav_guard`).
			if seg_dot_guard(tokens) && chars.first() == Some(&'.') {
				return false;
			}
			if seg_trav_guard(tokens) && (seg == "." || seg == "..") {
				return false;
			}
			let matched = Matcher { chars: &chars, nocase }.ends_of(tokens, 0, true).contains(&chars.len());
			matched && match_seg_list(psegs, pi + 1, tsegs, ti + 1, nocase)
		}
	}
}

struct Matcher<'a> {
	chars: &'a [char],
	nocase: bool,
}

impl<'a> Matcher<'a> {
	fn eq(&self, a: char, b: char) -> bool {
		if a == b {
			return true;
		}
		self.nocase && fold_char(a).is_some_and(|fa| fold_char(b) == Some(fa))
	}

	/// All end positions where `tokens` matches a prefix of `chars` starting
	/// at `ci`. `seg_first` marks that `tokens[0]` is the first token of a
	/// path segment at `ci == 0`, enabling the extglob dot guard: a guarded
	/// extglob body (wildcard-opening) may not consume a leading `.` —
	/// matching zero chars is fine (probed: `*(a).b` vs `.b` is true,
	/// `*(?)` vs `.a` is false).
	fn ends_of(&self, tokens: &[Token], ci: usize, seg_first: bool) -> Vec<usize> {
		let Some((token, rest)) = tokens.split_first() else {
			return vec![ci];
		};
		let guard = seg_first && ci == 0;
		let mut ends: Vec<usize> = Vec::new();
		match token {
			Token::Literal(c) => {
				if ci < self.chars.len() && self.eq(self.chars[ci], *c) {
					ends = self.ends_of(rest, ci + 1, false);
				}
			}
			Token::AnyChar => {
				if ci < self.chars.len() {
					ends = self.ends_of(rest, ci + 1, false);
				}
			}
			Token::Star => {
				for k in 0..=(self.chars.len() - ci) {
					ends.extend(self.ends_of(rest, ci + k, false));
				}
			}
			Token::Class(class) => {
				if ci < self.chars.len() && class.matches(self.chars[ci]) {
					ends = self.ends_of(rest, ci + 1, false);
				}
			}
			Token::Ext(ext) => {
				let mut ext_ends = self.ext_ends(ext, ci);
				if guard && ext.guarded && ci < self.chars.len() && self.chars[ci] == '.' {
					ext_ends.retain(|&e| e == ci); // zero-width only
				}
				for e in ext_ends {
					ends.extend(self.ends_of(rest, e, false));
				}
			}
		}
		ends
	}

	/// End positions of an extglob group matching from `ci`.
	fn ext_ends(&self, ext: &ExtSpec, ci: usize) -> Vec<usize> {
		let alt_ends = |from: usize| -> Vec<usize> {
			let mut out = Vec::new();
			for alt in &ext.alts {
				out.extend(self.ends_of(alt, from, false));
			}
			out
		};
		match ext.kind {
			ExtKind::At => alt_ends(ci),
			ExtKind::Question => {
				let mut out = vec![ci];
				out.extend(alt_ends(ci));
				out
			}
			ExtKind::Star | ExtKind::Plus => {
				// Closure over one-or-more repetitions; `*` adds the
				// zero-repetition start, `+` does not.
				let mut reached: Vec<usize> = if ext.kind == ExtKind::Star {
					vec![ci]
				} else {
					alt_ends(ci)
				};
				loop {
					let mut grew = false;
					let mut next = reached.clone();
					for &p in &reached {
						for e in alt_ends(p) {
							if !next.contains(&e) {
								next.push(e);
								grew = true;
							}
						}
					}
					reached = next;
					if !grew {
						break;
					}
				}
				reached
			}
			ExtKind::Bang => {
				// Approximate: any run not exactly equal to one alternative
				// (documented divergence from minimatch's lookahead form).
				let excluded = alt_ends(ci);
				(ci..=self.chars.len()).filter(|p| !excluded.contains(p)).collect()
			}
		}
	}
}

/// Per-char lowercase fold (single char only; multi-char mappings like
/// `'İ'` are skipped). Mirrors JS `toLowerCase` for the BMP cases the
/// matcher cares about.
fn fold_char(c: char) -> Option<char> {
	let mut it = c.to_lowercase();
	let first = it.next()?;
	if it.next().is_none() {
		Some(first)
	} else {
		None
	}
}

// ============================================================================
// Brace expansion
// ============================================================================

/// Expand `{a,b}` / `{m..n}` groups (nested, textual — like the
/// `brace-expansion` package minimatch uses). Groups without a top-level
/// comma or valid range stay literal.
fn brace_expand(pattern: &str) -> Vec<String> {
	match find_group(pattern) {
		None => vec![pattern.to_string()],
		Some((prefix, alts, suffix)) => {
			let suffix_exp = brace_expand(&suffix);
			let mut out = Vec::new();
			for alt in &alts {
				let mid = format!("{}{}", prefix, alt);
				for m in brace_expand(&mid) {
					for s in &suffix_exp {
						out.push(format!("{}{}", m, s));
					}
				}
			}
			out
		}
	}
}

/// Find the first expandable brace group: `(prefix, alternatives, suffix)`.
fn find_group(s: &str) -> Option<(String, Vec<String>, String)> {
	let chars: Vec<char> = s.chars().collect();
	let mut i = 0;
	while i < chars.len() {
		match chars[i] {
			'\\' => {
				i += 2;
			}
			'{' => {
				let mut depth = 1;
				let mut j = i + 1;
				let mut closed = None;
				while j < chars.len() {
					match chars[j] {
						'\\' => {
							j += 2;
							continue;
						}
						'{' => depth += 1,
						'}' => {
							depth -= 1;
							if depth == 0 {
								closed = Some(j);
								break;
							}
						}
						_ => {}
					}
					j += 1;
				}
				if let Some(close) = closed {
					let inner: String = chars[i + 1..close].iter().collect();
					if let Some(alts) = group_alternatives(&inner) {
						let prefix: String = chars[..i].iter().collect();
						let suffix: String = chars[close + 1..].iter().collect();
						return Some((prefix, alts, suffix));
					}
				}
				i += 1;
			}
			_ => i += 1,
		}
	}
	None
}

/// Alternatives of a brace group's content: top-level comma split, or a
/// `{m..n}` numeric / single-char range. `None` when not expandable.
fn group_alternatives(inner: &str) -> Option<Vec<String>> {
	let chars: Vec<char> = inner.chars().collect();
	let mut parts: Vec<String> = Vec::new();
	let mut cur = String::new();
	let mut depth = 0;
	let mut comma = false;
	let mut i = 0;
	while i < chars.len() {
		match chars[i] {
			'\\' => {
				if i + 1 < chars.len() {
					cur.push(chars[i]);
					cur.push(chars[i + 1]);
				}
				i += 2;
				continue;
			}
			'{' => {
				depth += 1;
				cur.push('{');
			}
			'}' => {
				depth -= 1;
				cur.push('}');
			}
			',' if depth == 0 => {
				comma = true;
				parts.push(std::mem::take(&mut cur));
			}
			c => cur.push(c),
		}
		i += 1;
	}
	if comma {
		parts.push(cur);
		return Some(parts);
	}
	// Range `{m..n}` (numeric or single char); step syntax unsupported.
	// Descending ranges render reversed (`{3..1}` → 3,2,1).
	let dot = inner.find("..")?;
	let (a, b) = inner.split_at(dot);
	let b = &b[2..];
	if let (Ok(m), Ok(n)) = (a.parse::<i64>(), b.parse::<i64>()) {
		let mut out = Vec::new();
		if m <= n {
			for v in m..=n {
				out.push(v.to_string());
			}
		} else {
			for v in n..=m {
				out.insert(0, v.to_string());
			}
		}
		return Some(out);
	}
	let a_chars: Vec<char> = a.chars().collect();
	let b_chars: Vec<char> = b.chars().collect();
	if a_chars.len() == 1 && b_chars.len() == 1 {
		let (m, n) = (a_chars[0], b_chars[0]);
		let mut out = Vec::new();
		if m <= n {
			for v in m..=n {
				out.push(v.to_string());
			}
		} else {
			for v in n..=m {
				out.insert(0, v.to_string());
			}
		}
		return Some(out);
	}
	None
}

// ============================================================================
// Tests (probed against minimatch 10.2.6 via the coding-agent package)
// ============================================================================

#[cfg(test)]
mod tests {
	use super::{brace_expand, minimatch};

	/// (text, pattern, expected) with `nocase: true` — every row probed
	/// against the real `minimatch` package in
	/// `/tmp/pi-repo/packages/coding-agent`.
	#[test]
	fn ground_truth_nocase() {
		let cases: &[(&str, &str, bool)] = &[
			// spec ground truth
			("a/b", "*", false),
			("a/b", "*/*", true),
			("anthropic/claude-sonnet-4-5", "anthropic/*", true),
			("anthropic/claude-sonnet", "*sonnet*", false),
			("claude-sonnet", "*sonnet*", true),
			("myprov/bracketed-model[1m]", "myprov/bracketed-model[1m]", false),
			("openai/gpt-5", "{anthropic,openai}/*", true),
			("anthropic/x", "{anthropic,openai}/*", true),
			("openrouter/moonshotai/kimi-k2.6", "**/kimi*", true),
			("abc", "**", true),
			("a/b/c", "**", true),
			("abc", "a+(b)c", true),
			// stars and questions
			("abbc", "a+(b)c", true),
			("ac", "a+(b)c", false),
			("abc", "a*(b)c", true),
			("abc", "a?(b)c", true),
			("ac", "a?(b)c", true),
			("abc", "a@(b)c", true),
			("a/b/c", "a/**", true),
			("a/b", "a/**", true),
			("a", "a/**", false),
			("a/", "a/**", true),
			("a/b/", "a/**", true),
			("x/a/b", "**/a", false),
			("x/a", "**/a", true),
			("a", "**/a", true),
			("a/b", "**/a", false),
			("AbC", "abc", true),
			("anthropic/claude", "ANTHROPIC/*", true),
			("a/b", "a**b", false),
			("ab", "a**b", true),
			("ab", "a*b", true),
			("a.b", "a?b", true),
			("a/b", "a?b", false),
			("ABC", "?", false),
			("A/B", "?", false),
			("abc", "ab?", true),
			("abc", "a??", true),
			("x/y", "[xy]/*", true),
			("x/y", "[x-y]/*", true),
			("x/y/z", "*/**", true),
			("x", "*/**", false),
			("a/b/c", "*/b/*", true),
			("a/b/c/d", "**/c/**", true),
			("a/b/c", "**/c", true),
			("a/b/c", "a/**/**/c", true),
			("a/b/c", "a/**/c", true),
			("foo/bar", "foo/**/bar", true),
			("foo/bar", "foo/**bar", true),
			("claude-3-5-sonnet-20241022", "claude-*-sonnet-*", true),
			("a", "a*", true),
			("a", "*", true),
			("", "*", false),
			("", "**", true),
			("x", "*x", true),
			("", "a*", false),
			("a/", "a/*", false),
			// classes
			("a.txt", "a[.]txt", true),
			("a/txt", "a[.]txt", false),
			("b", "[!a]", true),
			("a", "[!a]", false),
			("b", "[^a]", true),
			("a", "[a-c]", true),
			("B", "[a-c]", true),
			("b", "[A-C]", true),
			("a", "[A-Z]", true),
			("A", "[a-z]", true),
			("d", "[a-c]", false),
			("-", "[a-c]", false),
			("a]b", "a[]]b", true),
			("x", "[]", false),
			("a-b", "a[!c]b", true),
			("acb", "a[!c]b", false),
			("-", "[-a]", true),
			("a", "[-a]", true),
			("a", "[c-a]", false),
			("c", "[c-a]", false),
			("-", "[c-a]", false),
			("a/b", "[a]/[b]", true),
			("ab", "[a][b]", true),
			("ABC", "[abc]*", true),
			("a.b", "a[.]b", true),
			("axb", "a[.]b", false),
			// braces
			("ac", "{a,b}c", true),
			("abc", "{a,b}c", false),
			("a1", "a{1,2}", true),
			("a,b", "{a,b}", false),
			("a", "{foo,bar}", false),
			("foo", "{foo,bar}", true),
			("baz", "{foo,bar}", false),
			("b", "{*,b}", true),
			("c", "{a,{b,c}}", true),
			("{a,c}", "{a,{b,c}}", false),
			("acdf", "a{b,c{d,e}}f", true),
			("acef", "a{b,c{d,e}}f", true),
			("abf", "a{b,c{d,e}}f", true),
			("x2y", "x{1..3}y", true),
			("2", "{1..3}", true),
			("a,b", "a,b", true),
			("{a", "{a", true),
			("a}", "a}", true),
			("{x}", "{x}", true),
			("ab", "a{b}", false),
			// dotfile rule
			(".hidden", "*", false),
			("a/.hidden", "a/*", false),
			(".hidden", ".hidden", true),
			(".hidden", ".*", true),
			("a/.b", "a/.*", true),
			(".x", "**", false),
			("a/.b", "**", false),
			("a/.b", "a/**", false),
			(".x", "**/.x", true),
			("x/.y", "x/**/.y", true),
			("x/.y/z", "**/.y/**", true),
			("a/.b.c", "**/*.c", false),
			(".b.c", "*.c", false),
			(".txt", "*.txt", false),
			(".a.txt", "*.txt", false),
			("a.txt", "*.txt", true),
			("file.TXT", "*.txt", true),
			(".x", "?x", false),
			("a/.h", "a/?", false),
			(".x", "[.]x", true),
			(".xy", "[.x]y", false),
			(".a", "*(a)", false),
			(".a", "+(a)", false),
			(".a", "?(a)", false),
			(".x", "@(.x)", true),
			(".a", "*(.a)", true),
			(".x", "?([.])x", true),
			(".x", "*([.])x", true),
			(".x", "@([.]|a)x", true),
			(".x", "[!q]x", false),
			(".x", "[!x]", false),
			(".b", "*(a).b", true),
			(".b", "?(a).b", true),
			(".a", "*(*)", false),
			("a/./b", "a/**", false),
			("a/./b", "**", false),
			("p/..", "p/*", false),
			(".", "a", false),
			("..", ".*", false),
			("x", "**/*", true),
			(".a", "**/*", false),
			("a/.b", "**/*", false),
			("a/b/.c/d", "**/*", false),
			// extglob
			("bc", "@(a|b)c", true),
			("ac", "@(a|b)c", true),
			("ab", "@(a|b)c", false),
			("", "?(a|b)", true),
			("ab", "?(a|b)", false),
			("ab", "+(a|b)", true),
			("abab", "+(a|b)", true),
			("a", "+(a|b)", true),
			("abcd", "+(ab|cd)", true),
			("cdcdcd", "+(ab|cd)", true),
			("a+(b)c", "a+(b)c", false),
			("axb", "a@(?)b", true),
			("ab", "a@(?)b", false),
			("a", "@(*)", true),
			("ab", "@(a)+b", false),
			("abb", "@(a)+b", false),
			("ac", "a*(b)*c", true),
			("ab", "a*(b)*c", false),
			("ab", "ab", true),
			// `!()` extglob (approximate semantics — probed cases)
			("xx", "!(a)", true),
			("ac", "!(a)c", true),
			("bc", "!(a)c", true),
			("abc", "a!(b)c", false),
			("ac", "a!(b)c", true),
			// literals and escapes
			("p1/m", "p1/m", true),
			("/a", "/a", true),
			("a", "/a", false),
			("a/", "a/", true),
			("", "", true),
			("a?c", r"a\?c", true),
			("[a]", r"\[a\]", true),
			("!", "!a", true),
			("a!", "a!", true),
			// dot / traversal guards (segment-level, probed)
			(".a", "*a", false),
			("..", "?.", false),
			(".ab", "*ab", false),
			(".a.b", "*.b", false),
			(".tx", "?t", false),
			(".txt", "????", false),
			("..", ".*", false),
			(".", ".*", false),
			("...", ".*", true),
			("..x", ".*", true),
			("x..", ".*", false),
			(".b", "[.]*", true),
			("a/.", "a/.*", false),
			(".", "?", false),
			("..", "??", false),
			(".x", "!(x)", true),
			(".a", "!(.)", true),
			(".a", "@(!(x))", false),
			(".ab", "!(a)*", true),
			(".a", "a!(x)", false),
			("a.x", "a!(x)", true),
			("a/b", "a/./b", false),
			("a/b", "a/../b", false),
			("a/b", "./a/b", false),
			("a/b", "a/b/", false),
			("a/b", "a/b/.", false),
			("a/b", "a/b/..", false),
			("ab", "***", true),
			("a/b", "***", false),
			("a/b/c", "a/***/c", true),
			("a", "**?", true),
			("a/b", "**?", false),
			("x", "?*", true),
			("ab", "*?", true),
			("", "*?", false),
		];
		for (i, (text, pattern, expected)) in cases.iter().enumerate() {
			assert_eq!(
				minimatch(text, pattern, true),
				*expected,
				"case {i}: minimatch({text:?}, {pattern:?}, nocase)"
			);
		}
	}

	#[test]
	fn fuzz_vs_real_minimatch() {
		// Differential fixture: 8174 random text/pattern pairs probed against
		// the real minimatch 10.2.6 package (nocase). Cases containing `\`
		// or `!(` are excluded — documented divergences (see module docs):
		// POSIX minimatch applies literal-suffix fast tests to `?`-only path
		// portions, so a `\` in such a portion is not unescaped, while the
		// regex path unescapes it; `!()` compiles to a negative lookahead we
		// approximate. The pi-rs model-resolver call sites never produce
		// patterns with backslashes or `!(`.
		let cases: Vec<(String, String)> =
			serde_json::from_str(include_str!("minimatch_cases.json")).unwrap();
		let expected: Vec<bool> =
			serde_json::from_str(include_str!("minimatch_expected.json")).unwrap();
		let mut mismatches: Vec<String> = Vec::new();
		for ((t, p), exp) in cases.iter().zip(&expected) {
			let got = minimatch(t, p, true);
			if got != *exp {
				mismatches.push(format!("minimatch({t:?}, {p:?}) = {got}, expected {exp}"));
			}
		}
		assert!(mismatches.is_empty(), "{} mismatches:\n{}", mismatches.len(), mismatches.join("\n"));
	}

	#[test]
	fn ground_truth_case_sensitive() {
		let cases: &[(&str, &str, bool)] = &[
			("AbC", "abc", false),
			("anthropic/claude", "ANTHROPIC/*", false),
			("b", "[A-C]", false),
			("B", "[a-c]", false),
			("abc", "abc", true),
		];
		for (text, pattern, expected) in cases {
			assert_eq!(
				minimatch(text, pattern, false),
				*expected,
				"minimatch({text:?}, {pattern:?}, case-sensitive)"
			);
		}
	}

	#[test]
	fn brace_expansion_expands_and_leaves_literals() {
		assert_eq!(brace_expand("{a,b}"), vec!["a", "b"]);
		assert_eq!(brace_expand("a{b,c{d,e}}f"), vec!["abf", "acdf", "acef"]);
		assert_eq!(brace_expand("{1..3}"), vec!["1", "2", "3"]);
		assert_eq!(brace_expand("{3..1}"), vec!["3", "2", "1"]);
		assert_eq!(brace_expand("{x}"), vec!["{x}"]);
		assert_eq!(brace_expand("{a"), vec!["{a"]);
	}
}
