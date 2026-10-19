//! CLI argument parsing and help output (port of `src/cli/cli.ts` surface).

pub mod args;

/// JS `String.prototype.trim()` (ECMAScript WhiteSpace + LineTerminator).
pub fn js_trim(s: &str) -> &str {
    let start = s
        .char_indices()
        .find(|(_, c)| !pi_core::js_string::is_js_whitespace_or_terminator(*c))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let trimmed = &s[start..];
    let end = trimmed
        .char_indices()
        .rev()
        .find(|(_, c)| !pi_core::js_string::is_js_whitespace_or_terminator(*c))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    &trimmed[..end]
}
