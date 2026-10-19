//! Shared helpers ported from marked v18.0.11 (marked-pretty.js lines
//! ~489-604) plus the latex helpers from components/markdown.ts.
//!
//! JS-semantics notes:
//! - `js_replace` / `js_replace_once` / `js_match_all` mimic JS
//!   `String.prototype.replace` / `matchAll` (empty-match advance rules).
//! - `js_search` returns a CHAR index like JS `String.search` (callers do
//!   arithmetic on it, so byte indices would diverge on non-ASCII input).

use fancy_regex::Regex;

/// fancy-regex captures over `&str`.
pub(crate) type StrCaptures<'t> = fancy_regex::Captures<'t, str>;

use super::{Lexer, LinkDef, Token, TokenList};

// ---------------------------------------------------------------------------
// JS string/regex semantics helpers
// ---------------------------------------------------------------------------

pub(crate) fn next_char_boundary(text: &str, pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    let mut i = pos + 1;
    while i < text.len() && !text.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// JS `e = e.substring(raw.length)` — the block-loop advance by the token
/// raw's UTF-16 unit count. marked does NOT guarantee the reconstructed
/// raw is a prefix of the source: the blockquote tokenizer reassembles its
/// raw from line sections and can EXCEED the source (e.g. raw ">*\n\n<"
/// from src ">*\n<" — fuzz/reg-markdown-002 / reg-markdown-003). JS
/// `String.prototype.substring` clamps out-of-range indices and returns ""
/// instead of throwing; the previous byte-slice `&src[raw_len..]` panicked.
/// Skip exactly `units` UTF-16 code units from the start of `src`, clamped
/// at the end (a raw longer than the source consumes everything). A unit
/// count landing mid-astral-char (JS can split surrogate pairs, yielding a
/// lone surrogate) rounds up to the next char boundary — &str cannot hold
/// lone surrogates, and the parity corpus never reaches that case.
pub(crate) fn js_substring_skip(src: &str, units: usize) -> &str {
    let mut skipped = 0usize;
    for (off, ch) in src.char_indices() {
        if skipped >= units {
            return &src[off..];
        }
        skipped += ch.len_utf16();
    }
    ""
}

/// JS `text.replace(/.../g, f)` — replaces every match; after an empty match
/// the scan advances one char (JS semantics).
pub(crate) fn js_replace<F: FnMut(&StrCaptures<'_>) -> String>(
    re: &Regex,
    text: &str,
    mut f: F,
) -> String {
    let mut out = String::new();
    let mut last = 0usize;
    let mut pos = 0usize;
    while pos <= text.len() {
        let caps = match re.captures_from_pos(text, pos) {
            Ok(Some(c)) => c,
            _ => break,
        };
        let m = caps.get(0).expect("match 0");
        let (s, e) = (m.start(), m.end());
        if s < pos {
            break; // defensive: fancy-regex should not match before pos
        }
        out.push_str(&text[last..s]);
        out.push_str(&f(&caps));
        last = e;
        if e == s {
            let next = next_char_boundary(text, e);
            if next == e {
                break; // empty match at end of text: JS replace stops here
            }
            pos = next;
        } else {
            pos = e;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// JS `text.replace(/.../, f)` — replaces the first match only.
pub(crate) fn js_replace_once<F: FnMut(&StrCaptures<'_>) -> String>(
    re: &Regex,
    text: &str,
    mut f: F,
) -> String {
    match re.captures(text) {
        Ok(Some(caps)) => {
            let m = caps.get(0).expect("match 0");
            let mut out = String::with_capacity(text.len());
            out.push_str(&text[..m.start()]);
            out.push_str(&f(&caps));
            out.push_str(&text[m.end()..]);
            out
        }
        _ => text.to_string(),
    }
}

/// JS `text.matchAll(/.../g)` — all non-overlapping matches with their
/// indices; empty matches advance one char.
pub(crate) fn js_match_all<'t>(re: &Regex, text: &'t str) -> Vec<(usize, StrCaptures<'t>)> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos <= text.len() {
        let caps = match re.captures_from_pos(text, pos) {
            Ok(Some(c)) => c,
            _ => break,
        };
        let m = caps.get(0).expect("match 0");
        let (s, e) = (m.start(), m.end());
        if s < pos {
            break;
        }
        let is_empty = e == s;
        out.push((s, caps));
        if is_empty {
            let next = next_char_boundary(text, e);
            if next == e {
                break;
            }
            pos = next;
        } else {
            pos = e;
        }
    }
    out
}

/// JS `text.search(re)` — CHAR index of the first match, or -1.
pub(crate) fn js_search(re: &Regex, text: &str) -> i64 {
    match re.find(text) {
        Ok(Some(m)) => text[..m.start()].chars().count() as i64,
        _ => -1,
    }
}

/// JS `str.split(sep, 1)[0]` for a string separator.
pub(crate) fn js_split_first_line(text: &str) -> &str {
    match text.find('\n') {
        Some(i) => &text[..i],
        None => text,
    }
}

/// JS `str.indexOf(needle)` as a byte index (use with `&text[..i]`), or -1.
pub(crate) fn js_index_of(text: &str, needle: &str) -> i64 {
    match text.find(needle) {
        Some(i) => i as i64,
        None => -1,
    }
}

// ---------------------------------------------------------------------------
// marked helpers (marked-pretty.js ~489-604)
// ---------------------------------------------------------------------------

/// `$` (rtrim): strip trailing `c` chars; when `keep` is set, strip trailing
/// chars that are NOT `c`.
/// True for the chars JS `String.prototype.trim{,Start,End}` removes —
/// exactly WhiteSpace + LineTerminator, i.e. the same set `\s` matches.
/// Rust's `char::is_whitespace` (White_Space) differs on U+0085 (Rust
/// trims it, JS keeps it) and U+FEFF (JS trims it, Rust keeps it).
pub(crate) fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0b}'
            | '\u{0c}'
            | '\r'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// JS `String.prototype.trim`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// JS `String.prototype.trimStart`.
pub(crate) fn js_trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_whitespace)
}

/// JS `String.prototype.trimEnd`.
pub(crate) fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_whitespace)
}

pub(crate) fn rtrim(text: &str, c: char, keep: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n == 0 {
        return String::new();
    }
    let mut s = 0usize;
    while s < n {
        let r = chars[n - s - 1];
        if r == c && !keep {
            s += 1;
        } else if r != c && keep {
            s += 1;
        } else {
            break;
        }
    }
    chars[..n - s].iter().collect()
}

/// `te`: drop trailing blank lines (returns input unchanged when only a
/// couple of lines would remain — see marked source).
pub(crate) fn trim_trailing_blank_lines(text: &str, blank_line: &Regex) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut t = lines.len() as i64 - 1;
    while t >= 0 && blank_line.is_match(lines[t as usize]).unwrap_or(false) {
        t -= 1;
    }
    if lines.len() as i64 - t <= 2 {
        text.to_string()
    } else {
        lines[..(t + 1) as usize].join("\n")
    }
}

/// `me` (splitSpaces): expand tabs to 4-column stops starting at column
/// `col`.
pub(crate) fn split_spaces(text: &str, col: usize) -> String {
    let mut t = col;
    let mut out = String::new();
    for ch in text.chars() {
        if ch == '\t' {
            let r = 4 - (t % 4);
            for _ in 0..r {
                out.push(' ');
            }
            t += r;
        } else {
            out.push(ch);
            t += 1;
        }
    }
    out
}

/// `ee` (splitCells): split a table row on unescaped pipes, trimming the
/// outer edges and padding/truncating to `max` columns.
pub(crate) fn split_cells(
    text: &str,
    max: Option<usize>,
    find_pipe: &Regex,
    slash_pipe: &Regex,
) -> Vec<String> {
    // Pass 1: rewrite pipes — escaped \| stays, unescaped | becomes " |".
    let chars: Vec<char> = text.chars().collect();
    let mut pass1 = String::new();
    let _ = find_pipe; // scanning below replaces the regex walk 1:1
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '|' {
            let mut backslashes = 0;
            let mut a = i as i64 - 1;
            while a >= 0 && chars[a as usize] == '\\' {
                backslashes += 1;
                a -= 1;
            }
            let escaped = backslashes % 2 == 1;
            if escaped {
                pass1.push('|');
            } else {
                pass1.push_str(" |");
            }
        } else {
            pass1.push(ch);
        }
    }
    // Pass 2: split on " |" (JS split(/ \|/)).
    let mut cells: Vec<String> = pass1.split(" |").map(|s| s.to_string()).collect();
    // Drop leading cell when blank; drop trailing cell when blank.
    if js_trim(&cells[0]).is_empty() {
        cells.remove(0);
    }
    if !cells.is_empty() && cells.last().map(|s| js_trim(s).is_empty()).unwrap_or(false) {
        cells.pop();
    }
    if let Some(max) = max {
        if cells.len() > max {
            cells.truncate(max);
        } else {
            while cells.len() < max {
                cells.push(String::new());
            }
        }
    }
    // Trim each cell and unescape \| (slashPipe = /\\\|/g).
    for cell in cells.iter_mut() {
        let trimmed = js_trim(cell).to_string();
        *cell = js_replace(slash_pipe, &trimmed, |_| "|".to_string());
    }
    cells
}

/// `fe` (findClosingBracket): find the index of the matching `close` for the
/// leading `open` (returns -1 when absent, -2 when unbalanced-open).
/// Operates on a char slice; the returned index is a char index.
pub(crate) fn find_closing_bracket(chars: &[char], open: char, close: char) -> i64 {
    if !chars.contains(&close) {
        return -1;
    }
    let mut level = 0i64;
    let mut n = 0usize;
    while n < chars.len() {
        let c = chars[n];
        if c == '\\' {
            n += 1;
        } else if c == open {
            level += 1;
        } else if c == close {
            level -= 1;
            if level < 0 {
                return n as i64;
            }
        }
        n += 1;
    }
    if level > 0 {
        -2
    } else {
        -1
    }
}

/// `xe` (outputLink): build the link/image token for an inline match, with
/// the nested-link suppression state dance. `m[1]` is the label text,
/// `m[0]` starts with "!" for images.
pub(crate) fn output_link(
    m: &StrCaptures<'_>,
    def: &LinkDef,
    raw: &str,
    lexer: &mut Lexer,
    output_link_replace: &Regex,
) -> Option<Token> {
    let href = def.href.clone();
    let title = match &def.title {
        Some(t) if !t.is_empty() => Some(t.clone()),
        _ => None,
    };
    let label = m.get(1).map(|g| g.as_str()).unwrap_or("");
    let label = js_replace(output_link_replace, label, |c| {
        c.get(1).map(|g| g.as_str().to_string()).unwrap_or_default()
    });
    let is_image = m.get(0).map(|g| g.as_str()).unwrap_or("").starts_with('!');

    lexer.state.in_link = true;
    let saved_link_emitted = lexer.state.link_emitted;
    let saved_in_raw_block = lexer.state.in_raw_block;
    lexer.state.link_emitted = false;
    let inner: TokenList = lexer.inline_tokens_new(&label);
    let inner_emitted = lexer.state.link_emitted;
    lexer.state.link_emitted = saved_link_emitted;
    lexer.state.in_link = false;

    if !is_image && inner_emitted {
        lexer.state.in_raw_block = saved_in_raw_block;
        return None;
    }
    if !is_image {
        lexer.state.link_emitted = true;
    }

    let mut token = Token::new(if is_image { "image" } else { "link" }, raw);
    token.href = Some(href);
    token.title = Some(title);
    token.text = Some(label);
    token.tokens = Some(inner);
    Some(token)
}

/// `kt` (indentCodeCompensation): strip the fence indent from each line of
/// an indented code block's text.
pub(crate) fn indent_code_compensation(
    raw: &str,
    text: &str,
    indent_code_compensation: &Regex,
    beginning_space: &Regex,
) -> String {
    let m = indent_code_compensation
        .captures(raw)
        .ok()
        .flatten()
        .and_then(|c| c.get(1).map(|g| g.as_str().to_string()));
    let Some(indent) = m else {
        return text.to_string();
    };
    text.split('\n')
        .map(|line| {
            let lead = beginning_space
                .captures(line)
                .ok()
                .flatten()
                .and_then(|c| c.get(0).map(|g| g.as_str().to_string()));
            match lead {
                None => line.to_string(),
                Some(lead) => {
                    if lead.chars().count() >= indent.chars().count() {
                        let skip = indent.chars().count();
                        line.chars().skip(skip).collect()
                    } else {
                        line.to_string()
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Latex helpers (components/markdown.ts)
// ---------------------------------------------------------------------------

/// `findClosingDelimiter(source, closing, start)` — byte index or -1.
pub(crate) fn latex_find_closing_delimiter(source: &str, closing: &str, start: usize) -> i64 {
    let mut index = index_of_from(source, closing, start);
    while index >= 0 {
        if is_escaped_at_byte(source, index as usize) {
            index = index_of_from(source, closing, index as usize + closing.len());
        } else {
            return index;
        }
    }
    -1
}

fn index_of_from(source: &str, needle: &str, from: usize) -> i64 {
    let start = from.min(source.len());
    match source[start..].find(needle) {
        Some(i) => (start + i) as i64,
        None => -1,
    }
}

fn is_escaped_at_byte(source: &str, byte_index: usize) -> bool {
    // count backslashes immediately before byte_index
    let prefix = &source[..byte_index];
    let mut backslashes = 0usize;
    for c in prefix.chars().rev() {
        if c == '\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 1
}

pub(crate) fn latex_looks_like_pending_dollar_math(source: &str) -> bool {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"\\[A-Za-z]+|[_^=+*/<>()\[\]|±≤≥≠≈∈→⇒∞∫∑√-]",
        )
        .expect("pending dollar math regex")
    });
    re.is_match(source).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn re(p: &str) -> Regex {
        Regex::new(p).unwrap()
    }

    #[test]
    fn rtrim_basics() {
        assert_eq!(rtrim("abc##", '#', false), "abc");
        assert_eq!(rtrim("abc", '#', false), "abc");
        assert_eq!(rtrim("", '#', false), "");
        assert_eq!(rtrim("abc", 'c', false), "ab");
    }

    #[test]
    fn split_spaces_tabs() {
        assert_eq!(split_spaces("\ta", 3), " a");
        assert_eq!(split_spaces("\ta", 0), "    a");
        assert_eq!(split_spaces("x\ty", 0), "x   y");
    }

    #[test]
    fn find_closing_bracket_cases() {
        assert_eq!(find_closing_bracket(&"ab]x".chars().collect::<Vec<_>>(), '[', ']'), 2);
        assert_eq!(find_closing_bracket(&"ab".chars().collect::<Vec<_>>(), '[', ']'), -1);
        assert_eq!(find_closing_bracket(&"[a[b]c".chars().collect::<Vec<_>>(), '[', ']'), -2);
        assert_eq!(find_closing_bracket(&"a[b]c".chars().collect::<Vec<_>>(), '[', ']'), -1);
    }

    #[test]
    fn split_cells_basics() {
        let fp = re(r"\|");
        let sp = re(r"\\\|");
        assert_eq!(
            split_cells("| a | b |", None, &fp, &sp),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(split_cells("a|b", None, &fp, &sp), vec!["a", "b"]);
        assert_eq!(
            split_cells(r"a\|b|c", None, &fp, &sp),
            vec!["a|b".to_string(), "c".to_string()]
        );
        assert_eq!(
            split_cells("a|b|c", Some(2), &fp, &sp),
            vec!["a", "b"]
        );
    }

    #[test]
    fn js_replace_empty_matches() {
        let re = re("x*");
        assert_eq!(js_replace(&re, "ab", |_| "-".to_string()), "-a-b-");
        assert_eq!(js_replace(&re, "", |_| "-".to_string()), "-");
    }

    #[test]
    fn js_replace_once_first_only() {
        let re = re("a");
        assert_eq!(js_replace_once(&re, "aaa", |_| "b".to_string()), "baa");
    }

    #[test]
    fn latex_helpers() {
        assert!(latex_find_closing_delimiter("a$b$c", "$", 2) >= 0);
        assert_eq!(latex_find_closing_delimiter("a\\$b$c", "$", 2), 4);
        assert!(latex_looks_like_pending_dollar_math("x_1"));
        assert!(!latex_looks_like_pending_dollar_math("plain text"));
    }

    #[test]
    fn blank_line_trim() {
        let blank = re(r"^[ \t]*$");
        assert_eq!(trim_trailing_blank_lines("a\n\n\n", &blank), "a");
        assert_eq!(trim_trailing_blank_lines("a\nb\n\n\n\n", &blank), "a\nb");
    }
}
