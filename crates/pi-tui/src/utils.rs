//! Port of `packages/tui/src/utils.ts` (width engine slice).
//!
//! Byte-level ANSI scanning is safe on UTF-8: multi-byte sequences never
//! contain ASCII bytes, and every escape-sequence character this parser
//! consumes is ASCII. Where the original uses UTF-16 `String.length`
//! semantics, that is preserved explicitly (see [`utf16_len`]).

use crate::unicode::{
    east_asian_width, grapheme_segments, is_control, is_default_ignorable, is_format, is_mark,
    is_rgi_emoji, is_spacing_mark, is_surrogate,
};

/// JS `String.length` (UTF-16 code units) for a `&str`.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| if (c as u32) > 0xffff { 2 } else { 1 }).sum()
}

// --- ANSI escape sequence scanning (byte level) ------------------------------

/// Length in bytes of the ANSI/OSC/APC escape sequence starting at `pos`,
/// or 0 if there is none. Port of `ansiCodeLength`.
pub fn ansi_code_length(s: &str, pos: usize) -> usize {
    let b = s.as_bytes();
    if pos >= b.len() || b[pos] != 0x1b {
        return 0;
    }
    let next = if pos + 1 < b.len() { b[pos + 1] } else { 0 };
    if next == b'[' {
        // CSI: ESC [ ... m/G/K/H/J
        for j in (pos + 2)..b.len() {
            match b[j] {
                0x6d | 0x47 | 0x4b | 0x48 | 0x4a => return j + 1 - pos,
                _ => {}
            }
        }
        return 0;
    }
    if next == b']' || next == b'_' {
        // OSC / APC: ... BEL or ESC \ (ST)
        for j in (pos + 2)..b.len() {
            if b[j] == 0x07 {
                return j + 1 - pos;
            }
            if b[j] == 0x1b && j + 1 < b.len() && b[j + 1] == b'\\' {
                return j + 2 - pos;
            }
        }
        return 0;
    }
    0
}

/// Port of `extractAnsiCode` — the escape sequence starting at `pos`, if any.
pub fn extract_ansi_code(s: &str, pos: usize) -> Option<&str> {
    let len = ansi_code_length(s, pos);
    if len > 0 {
        Some(&s[pos..pos + len])
    } else {
        None
    }
}

/// Port of `asciiVisibleWidth` — width for strings made only of printable
/// ASCII, tabs, and complete escape sequences; `None` (= JS `-1`) otherwise.
fn ascii_visible_width(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut width = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        let code = b[i];
        if (0x20..=0x7e).contains(&code) {
            width += 1;
            i += 1;
        } else if code == 0x09 {
            width += 3;
            i += 1;
        } else if code == 0x1b {
            let len = ansi_code_length(s, i);
            if len == 0 {
                return None;
            }
            i += len;
        } else {
            return None;
        }
    }
    Some(width)
}

/// Port of `stripTerminalSequences` — remove ANSI/OSC/APC sequences, keep text.
pub fn strip_terminal_sequences(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut result = String::new();
    let mut i = 0usize;
    while i < s.len() {
        if let Some(code) = extract_ansi_code(s, i) {
            i += code.len();
        } else {
            let ch = s[i..].chars().next().unwrap();
            result.push(ch);
            i += ch.len_utf8();
        }
    }
    result
}

// --- terminal spacing marks (utils.ts terminalSpacingMarkRegex) --------------

const TERMINAL_SPACING_MARK_REMOVALS: &[u32] = &[0x1734, 0x302e, 0x302f];
const TERMINAL_SPACING_MARK_ADDITIONS: &[u32] = &[
    0x065f, 0x0f7f, 0x102b, 0x102c, 0x1031, 0x1033, 0x1034, 0x1035, 0x1038, 0x103a, 0x103b,
    0x103c, 0x103d, 0x103e,
];

/// `terminalSpacingMarkRegex.test(char)` for a single codepoint.
fn is_terminal_spacing_mark(cp: u32) -> bool {
    (is_spacing_mark(cp) && !TERMINAL_SPACING_MARK_REMOVALS.contains(&cp))
        || TERMINAL_SPACING_MARK_ADDITIONS.contains(&cp)
}

/// `nonPrintingCharRegex` for a single codepoint:
/// `Default_Ignorable | Control | Format | Mark | Surrogate`.
fn is_non_printing(cp: u32) -> bool {
    is_default_ignorable(cp) || is_control(cp) || is_format(cp) || is_mark(cp) || is_surrogate(cp)
}

/// Port of `couldBeEmoji` (cheap pre-filter for the RGI regex).
fn could_be_emoji(segment: &str) -> bool {
    let cp = segment.chars().next().map(|c| c as u32).unwrap_or(0);
    (0x1f000..=0x1fbff).contains(&cp)
        || (0x2300..=0x23ff).contains(&cp)
        || (0x2600..=0x27bf).contains(&cp)
        || (0x2b50..=0x2b55).contains(&cp)
        || segment.contains('\u{fe0f}')
        || utf16_len(segment) > 2
}

/// Port of `graphemeWidth`.
fn grapheme_width(segment: &str) -> usize {
    let mut chars = segment.chars();
    let first = chars.next();
    let single = first.is_some() && chars.next().is_none();

    // Fast path: single UTF-16 code unit (BMP) printable ASCII / tab.
    if single {
        let cp = first.unwrap() as u32;
        if cp <= 0xffff {
            if (0x20..=0x7e).contains(&cp) {
                return 1;
            }
            if cp == 0x09 {
                return 3;
            }
        }
    }

    // Some marks occupy cells even without a base character.
    if segment.chars().all(|c| is_terminal_spacing_mark(c as u32)) {
        return segment.chars().count();
    }

    // Zero-width clusters: Default_Ignorable | Control | Mark | Surrogate.
    if segment.chars().all(|c| {
        is_default_ignorable(c as u32)
            || is_control(c as u32)
            || is_mark(c as u32)
            || is_surrogate(c as u32)
    }) {
        return 0;
    }

    // Emoji check with pre-filter.
    if could_be_emoji(segment) && is_rgi_emoji(segment) {
        return 2;
    }

    // Strip leading non-printing code points.
    let base = segment.trim_start_matches(|c: char| is_non_printing(c as u32));
    let mut base_chars = base.chars();
    let Some(first) = base_chars.next() else {
        return 0;
    };
    let cp = first as u32;

    // Regional indicators render full-width even in isolation.
    if (0x1f1e6..=0x1f1ff).contains(&cp) {
        return 2;
    }

    let mut width = east_asian_width(cp) as usize;
    let mut follows_mark = false;
    for ch in base_chars {
        let c = ch as u32;
        if is_terminal_spacing_mark(c) {
            width += 1;
            follows_mark = false;
        } else if is_mark(c) {
            follows_mark = true;
        } else if !is_non_printing(c) {
            if follows_mark || (0xff00..=0xffef).contains(&c) {
                // halfwidth + fullwidth forms
                width += east_asian_width(c) as usize;
            } else if c == 0x0e33 || c == 0x0eb3 {
                width += 1;
            }
            follows_mark = false;
        }
    }
    width
}

/// Port of `visibleWidth` — the terminal-column width of a string.
pub fn visible_width(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }

    // Fast path: printable ASCII, tabs, and complete ANSI escape sequences.
    if let Some(w) = ascii_visible_width(s) {
        return w;
    }

    // Slow path: normalize into an owned buffer (tabs → 3 spaces, then strip
    // complete ANSI/OSC/APC sequences in one pass), then sum grapheme widths.
    let mut clean = String::with_capacity(s.len());
    if s.contains('\t') {
        for ch in s.chars() {
            if ch == '\t' {
                clean.push_str("   ");
            } else {
                clean.push(ch);
            }
        }
    } else {
        clean.push_str(s);
    }

    if clean.contains('\x1b') {
        let b = clean.as_bytes();
        let mut stripped = String::with_capacity(clean.len());
        let mut copy_from = 0usize;
        let mut escape_index = position_of(b, 0x1b, 0);
        while let Some(i) = escape_index {
            let len = ansi_code_length(&clean, i);
            if len > 0 {
                stripped.push_str(&clean[copy_from..i]);
                let next = i + len;
                copy_from = next;
                escape_index = position_of(b, 0x1b, next);
            } else {
                escape_index = position_of(b, 0x1b, i + 1);
            }
        }
        stripped.push_str(&clean[copy_from..]);
        clean = stripped;
    }

    let mut width = 0usize;
    for segment in grapheme_segments(&clean) {
        width += grapheme_width(segment);
    }
    width
}

fn position_of(bytes: &[u8], needle: u8, from: usize) -> Option<usize> {
    bytes[from..]
        .iter()
        .position(|&b| b == needle)
        .map(|i| i + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from upstream test/visible-width.test.ts
    #[test]
    fn measures_styled_ascii_without_counting_escape_sequences() {
        assert_eq!(visible_width("\x1b[38;5;4mhello\x1b[39m world"), 11);
        assert_eq!(
            visible_width("\x1b]8;;https://example.com\x07link\x1b]8;;\x07"),
            4
        );
        assert_eq!(visible_width("\x1b]133;A\x1b\\prompt"), 6);
        assert_eq!(visible_width("\x1b_pi:c\x07cursor"), 6);
    }

    #[test]
    fn counts_tabs_as_three_columns_in_styled_text() {
        assert_eq!(visible_width("\x1b[1ma\tb\x1b[22m"), 5);
    }

    #[test]
    fn measures_styled_non_ascii_text() {
        assert_eq!(visible_width("\x1b[31m日本\x1b[39m ok"), 7);
        assert_eq!(visible_width("\x1b[31m─→\x1b[39m"), 2);
    }

    #[test]
    fn treats_unterminated_escape_sequences_as_zero_width_control_characters() {
        assert_eq!(visible_width("\x1b[31"), 3);
        assert_eq!(visible_width("a\x1b"), 1);
    }

    #[test]
    fn strip_terminal_sequences_removes_ansi_osc_apc() {
        assert_eq!(
            strip_terminal_sequences("\x1b[1mhi\x1b[22m there"),
            "hi there"
        );
        assert_eq!(
            strip_terminal_sequences("\x1b]8;;https://x\x07link\x1b]8;;\x07"),
            "link"
        );
        assert_eq!(strip_terminal_sequences("plain"), "plain");
    }

    #[test]
    fn emoji_and_wide_clusters() {
        assert_eq!(visible_width("😀"), 2);
        assert_eq!(visible_width("👨‍👩‍👧‍👦"), 2); // ZWJ family is one RGI cluster
        assert_eq!(visible_width("🇺🇸"), 2); // flag pair
        assert_eq!(visible_width("🇺🇸🇺🇸"), 4);
        assert_eq!(visible_width("a😀b"), 4);
        assert_eq!(visible_width("한국어"), 6);
    }
}
