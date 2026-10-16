//! ANSI escape-sequence handling. Port of `src/utils/ansi.ts`
//! (derived from ansi-regex / strip-ansi).
//!
//! The original drives two regexes:
//! - `ansiRegex`: `(?:ESC\][\s\S]*?ST)|CSI_START CSI_FINAL` (global) — used
//!   by `stripAnsi` to remove complete sequences.
//! - `unfinishedAnsiAtEndRegex`: `(?:ESC\]consumable*|CSI_START)$` — used by
//!   `splitIncompleteAnsiSuffix` to hold back a trailing unfinished sequence
//!   while streaming.
//!
//! Both are implemented here as hand-rolled scanners that replicate the regex
//! engines' greedy/backtracking behavior exactly (including that a digit is
//! a valid CSI final byte, so e.g. `ESC [ 1` is a complete sequence, and that
//! `]` is a CSI intermediate, so an unterminated OSC can still match CSI).

/// BEL, ESC \\ and 0x9C are the valid string terminators (ST).
const ESC: char = '\u{1b}';
const CSI8: char = '\u{9b}';
const BEL: char = '\u{7}';
/// 0x9C — the 8-bit string terminator (ST).
const ST8: char = '\u{9c}';

/// Longest unfinished sequence held back while streaming.
const MAX_PENDING_ANSI_LENGTH: usize = 256;

fn is_csi_intermediate(c: char) -> bool {
    // `[[\]()#;?]` — note: no backslash member (the source's `\\]` is an
    // escaped `]`, not a backslash + bracket).
    matches!(c, '[' | ']' | '(' | ')' | '#' | ';' | '?')
}

/// `CSI_FINAL` = `[\dA-PR-TZcf-nq-uy=><~]`.
fn is_csi_final(c: char) -> bool {
    c.is_ascii_digit()
        || matches!(
            c,
            'A'..='P' | 'R'..='T' | 'Z' | 'c'..='n' | 'q'..='u' | 'y' | '=' | '>' | '<' | '~'
        )
}

/// Length of the greedy `params` match (`\d{1,4}(?:[;:]\d{0,4})*`) at `p`.
/// Returns 0 when `chars[p]` is not a digit.
fn greedy_params_len(chars: &[char], p: usize) -> usize {
    let n = chars.len();
    if p >= n || !chars[p].is_ascii_digit() {
        return 0;
    }
    let mut i = p;
    let mut digits = 0;
    while digits < 4 && i < n && chars[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    loop {
        // One repetition of `[;:]\d{0,4}`.
        if i < n && (chars[i] == ';' || chars[i] == ':') {
            i += 1;
            let mut digits = 0;
            while digits < 4 && i < n && chars[i].is_ascii_digit() {
                i += 1;
                digits += 1;
            }
        } else {
            return i - p;
        }
    }
}

/// Match a CSI sequence starting at `i` (an ESC or 8-bit CSI introducer),
/// replicating the regex's greedy-then-backtrack order:
/// intermediates greedy (longest first), then params variants (longest
/// first, then absent), then one final byte. Returns the match end.
fn match_csi(chars: &[char], i: usize) -> Option<usize> {
    let n = chars.len();
    let q = i + 1;
    let mut max_inter = 0;
    while q + max_inter < n && is_csi_intermediate(chars[q + max_inter]) {
        max_inter += 1;
    }
    for inter in (0..=max_inter).rev() {
        let p = q + inter;
        if p < n {
            if chars[p].is_ascii_digit() {
                let g = greedy_params_len(chars, p);
                for l in (1..=g).rev() {
                    if p + l < n && is_csi_final(chars[p + l]) {
                        return Some(p + l + 1);
                    }
                }
            }
            if is_csi_final(chars[p]) {
                return Some(p + 1);
            }
        }
    }
    None
}

/// Match an OSC sequence `ESC ] ... ST` starting at `i` (non-greedy: the
/// first ST wins). Returns the match end.
fn match_osc(chars: &[char], i: usize) -> Option<usize> {
    let n = chars.len();
    if chars[i] != ESC || i + 1 >= n || chars[i + 1] != ']' {
        return None;
    }
    let mut j = i + 2;
    while j < n {
        match chars[j] {
            BEL | ST8 => return Some(j + 1),
            ESC if j + 1 < n && chars[j + 1] == '\\' => return Some(j + 2),
            _ => j += 1,
        }
    }
    None
}

fn match_ansi(chars: &[char], i: usize) -> Option<usize> {
    // The regex alternation tries the OSC branch first, then CSI.
    match_osc(chars, i).or_else(|| match_csi(chars, i))
}

/// `stripAnsi(value)`: remove all complete ANSI sequences.
pub fn strip_ansi(value: &str) -> String {
    if !value.contains(ESC) && !value.contains(CSI8) {
        return value.to_string();
    }
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == ESC || c == CSI8 {
            if let Some(end) = match_ansi(&chars, i) {
                i = end;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// `splitIncompleteAnsiSuffix(value)`: split streamed text into a part that
/// is safe to pass to `stripAnsi` now and a trailing unfinished escape
/// sequence to prepend to the next chunk.
pub fn split_incomplete_ansi_suffix(value: &str) -> (String, String) {
    if !value.contains(ESC) && !value.contains(CSI8) {
        return (value.to_string(), String::new());
    }
    // The original applies the regex to the last 256 UTF-16 code units.
    let chars: Vec<char> = value.chars().collect();
    let total_units: usize = value.chars().map(|c| c.len_utf16()).sum();
    let window_start_units = total_units.saturating_sub(MAX_PENDING_ANSI_LENGTH);
    let mut window_start = 0;
    let mut units = 0;
    for (idx, &c) in chars.iter().enumerate() {
        if units >= window_start_units {
            window_start = idx;
            break;
        }
        units += c.len_utf16();
    }
    let window = &chars[window_start..];
    if let Some(p) = find_unfinished(window) {
        let split_at = window_start + p;
        (chars[..split_at].iter().collect(), chars[split_at..].iter().collect())
    } else {
        (value.to_string(), String::new())
    }
}

/// Leftmost position in `window` where the `unfinishedAnsiAtEndRegex`
/// matches (the match must extend to the end of the window).
fn find_unfinished(window: &[char]) -> Option<usize> {
    let n = window.len();
    for p in 0..n {
        let c = window[p];
        if c == ESC && p + 1 < n && window[p + 1] == ']' && osc_unfinished_reaches_end(window, p + 2) {
            return Some(p);
        }
        if (c == ESC || c == CSI8) && csi_reaches_end(window, p) {
            return Some(p);
        }
    }
    None
}

/// OSC branch: `ESC ]` followed by only consumable chars up to the end.
/// Consumable = not BEL, not 0x9C, and an ESC must not be followed by `\`
/// (an ESC at the very end is consumable: nothing follows it). Note that
/// 0x9B (the 8-bit CSI introducer) IS consumable inside an unfinished OSC.
fn osc_unfinished_reaches_end(window: &[char], from: usize) -> bool {
    let n = window.len();
    let mut j = from;
    while j < n {
        let c = window[j];
        if c == BEL || c == ST8 {
            return false;
        }
        if c == ESC && j + 1 < n && window[j + 1] == '\\' {
            return false;
        }
        j += 1;
    }
    true
}

/// CSI branch: a `CSI_START` match that extends exactly to the end of the
/// window (introducer + intermediates* + params?, no final byte).
fn csi_reaches_end(window: &[char], p: usize) -> bool {
    let n = window.len();
    let mut q = p + 1;
    while q < n && is_csi_intermediate(window[q]) {
        q += 1;
    }
    if q == n {
        return true;
    }
    if window[q].is_ascii_digit() {
        // A params prefix of any length 1..=greedy is a valid parse, so the
        // remainder is fully consumable iff its length is <= the greedy
        // params match.
        return n - q <= greedy_params_len(window, q);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_basic() {
        assert_eq!(strip_ansi("a\u{1b}[31mred\u{1b}[0mb"), "aredb");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}text"), "text");
        assert_eq!(strip_ansi("\u{1b}]0;t\u{1b}\\x"), "x");
        assert_eq!(strip_ansi("\u{1b}[?25h\u{1b}(B\u{1b}[38;5;196mX"), "X");
        assert_eq!(strip_ansi("\u{9b}31mX"), "X");
        // Unterminated OSC: the CSI fallback matches "ESC]0;t" ('t' is a
        // final byte), leaving "itle".
        assert_eq!(strip_ansi("a\u{1b}]0;title"), "aitle");
    }

    #[test]
    fn strip_csi_backtracking() {
        // A digit is a valid final byte: "ESC [ 1" is complete.
        assert_eq!(strip_ansi("\u{1b}[1"), "");
        // ']' is a CSI intermediate: "ESC ] 0 ; m" matches CSI when the OSC
        // has no terminator.
        assert_eq!(strip_ansi("\u{1b}]0;m"), "");
        // 't' is in the final class: "ESC t" is a complete sequence.
        assert_eq!(strip_ansi("plain\u{1b}text"), "plainext");
        // Intermediates then params then final.
        assert_eq!(strip_ansi("\u{1b}[;5"), "");
        assert_eq!(strip_ansi("\u{1b}[12345x"), "x");
    }

    #[test]
    fn split_basic() {
        let (complete, pending) = split_incomplete_ansi_suffix("abc\u{1b}[3");
        assert_eq!((complete.as_str(), pending.as_str()), ("abc", "\u{1b}[3"));
        let (complete, pending) = split_incomplete_ansi_suffix("x\u{1b}]0;tit");
        assert_eq!((complete.as_str(), pending.as_str()), ("x", "\u{1b}]0;tit"));
        let (complete, pending) = split_incomplete_ansi_suffix("abc\u{1b}[31m");
        assert_eq!((complete.as_str(), pending.as_str()), ("abc\u{1b}[31m", ""));
        // An ESC inside an unfinished OSC is consumable unless followed by \.
        let (complete, pending) = split_incomplete_ansi_suffix("\u{1b}]a\u{1b}[3");
        assert_eq!((complete.as_str(), pending.as_str()), ("", "\u{1b}]a\u{1b}[3"));
        // A trailing "ESC \" is NOT pending: it is not a CSI_START
        // (backslash is not an intermediate byte) and the OSC branch cannot
        // consume it. A bare trailing ESC is pending.
        let (complete, pending) = split_incomplete_ansi_suffix("\u{1b}]a\u{1b}\\");
        assert_eq!((complete.as_str(), pending.as_str()), ("\u{1b}]a\u{1b}\\", ""));
        let (complete, pending) = split_incomplete_ansi_suffix("x\u{1b}");
        assert_eq!((complete.as_str(), pending.as_str()), ("x", "\u{1b}"));
    }

    #[test]
    fn split_window_limit() {
        // Sequences longer than 256 units are processed as-is.
        let long = format!("x{}{}", "\u{1b}]0;", "a".repeat(300));
        let (complete, pending) = split_incomplete_ansi_suffix(&long);
        assert_eq!(pending, "");
        assert_eq!(complete, long);
    }
}
