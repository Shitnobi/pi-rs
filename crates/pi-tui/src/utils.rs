//! Port of `packages/tui/src/utils.ts` (width engine slice).
//!
//! Byte-level ANSI scanning is safe on UTF-8: multi-byte sequences never
//! contain ASCII bytes, and every escape-sequence character this parser
//! consumes is ASCII. Where the original uses UTF-16 `String.length`
//! semantics, that is preserved explicitly (see [`utf16_len`]).

use crate::latex::{coalesce_surrogate_sentinels, is_surrogate_sentinel};
use crate::unicode::{
    east_asian_width, grapheme_segments, is_control, is_cjk_script, is_default_ignorable,
    is_format, is_mark, is_punctuation, is_rgi_emoji, is_spacing_mark, is_surrogate,
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
        let hit = b[pos + 2..]
            .iter()
            .position(|&c| matches!(c, 0x6d | 0x47 | 0x4b | 0x48 | 0x4a));
        return match hit {
            Some(j) => j + 3,
            None => 0,
        };
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
    is_default_ignorable(cp)
        || is_control(cp)
        || is_format(cp)
        || is_mark(cp)
        || is_surrogate(cp)
        || crate::latex::sentinel_surrogate_unit_cp(cp).is_some()
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
            || is_surrogate_sentinel(c)
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

    // JS UTF-16 surrogate-sentinel handling (see latex.rs): adjacent sentinel
    // pairs coalesce into the real astral char (Intl.Segmenter joins them);
    // lone sentinels stay and measure zero-width (lone surrogates match
    // `\p{Surrogate}` in the TS zero-width regex).
    if let Some(coalesced) = coalesce_surrogate_sentinels(&clean) {
        clean = coalesced;
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

// --- JS string helpers --------------------------------------------------------

/// ECMAScript `\s` / `trim` whitespace: WhiteSpace ∪ LineTerminator.
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}'
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

fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_whitespace)
}

pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// JS `str.split(/\r\n|\r|\n/)`.
fn split_js_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let (mut start, mut i) = (0usize, 0usize);
    while i < b.len() {
        match b[i] {
            b'\r' => {
                out.push(&text[start..i]);
                i += 1;
                if b.get(i) == Some(&b'\n') {
                    i += 1;
                }
                start = i;
            }
            b'\n' => {
                out.push(&text[start..i]);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    out.push(&text[start..]);
    out
}

fn is_printable_ascii(s: &str) -> bool {
    s.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

// --- character classification (regexes from utils.ts) --------------------------

/// `cjkBreakRegex.test(segment)` — true if any code point carries a CJK
/// script extension.
fn cjk_break_test(segment: &str) -> bool {
    segment.chars().any(|c| is_cjk_script(c as u32))
}

/// `PUNCTUATION_REGEX` — the ASCII punctuation character class.
pub fn is_punctuation_char(c: char) -> bool {
    matches!(
        c,
        '(' | ')'
            | '{' | '}'
            | '[' | ']'
            | '<' | '>'
            | '.'
            | ','
            | ';'
            | ':'
            | '\''
            | '"'
            | '!'
            | '?'
            | '+'
            | '-'
            | '='
            | '*'
            | '/'
            | '\\'
            | '|'
            | '&'
            | '%'
            | '^'
            | '$'
            | '#'
            | '@'
            | '~'
            | '`'
    )
}

/// `/\s/.test(char)`
pub fn is_whitespace_char(c: char) -> bool {
    is_js_whitespace(c)
}

/// The explicit extra punctuation set from `cjkPunctuationRegex`.
const CJK_PUNCTUATION_EXTRA: &[char] = &[
    '，', '．', '：', '；', '！', '？', '（', '）', '［', '］', '｛', '｝', '“', '”', '‘', '’', '…',
    '—',
];

/// `cjkPunctuationRegex.test(char)`:
/// `(?:(?=\p{Punctuation})cjkBreak | [，．：；！？（）［］｛｝“”‘’…—])`
pub fn is_cjk_punctuation_char(c: char) -> bool {
    let cp = c as u32;
    (is_punctuation(cp) && is_cjk_script(cp)) || CJK_PUNCTUATION_EXTRA.contains(&c)
}

// --- OSC 8 hyperlinks -----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc8Terminator {
    /// `"\x07"`
    Bel,
    /// `"\x1b\\"`
    St,
}

impl Osc8Terminator {
    fn as_str(self) -> &'static str {
        match self {
            Self::Bel => "\x07",
            Self::St => "\x1b\\",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActiveHyperlink {
    params: String,
    url: String,
    terminator: Osc8Terminator,
}

/// Port of `parseOsc8Hyperlink`. `None` = `undefined` (not an OSC 8 code or
/// no `;` separator — leave state untouched), `Some(None)` = `null` (a close
/// sequence), `Some(Some(..))` = an open hyperlink.
fn parse_osc8_hyperlink(ansi_code: &str) -> Option<Option<ActiveHyperlink>> {
    let body = ansi_code.strip_prefix("\x1b]8;")?;
    let terminator = if ansi_code.ends_with('\x07') {
        Osc8Terminator::Bel
    } else {
        Osc8Terminator::St
    };
    let body = body.strip_suffix(terminator.as_str()).unwrap_or("");
    let (params, url) = body.split_once(';')?;
    if url.is_empty() {
        return Some(None);
    }
    Some(Some(ActiveHyperlink {
        params: params.to_string(),
        url: url.to_string(),
        terminator,
    }))
}

fn format_osc8_hyperlink(hyperlink: &ActiveHyperlink) -> String {
    format!(
        "\x1b]8;{};{}{}",
        hyperlink.params,
        hyperlink.url,
        hyperlink.terminator.as_str()
    )
}

fn format_osc8_close(terminator: Osc8Terminator) -> String {
    format!("\x1b]8;;{}", terminator.as_str())
}

/// Port of `getActiveOsc8Close` — the close sequence matching the terminator
/// of the hyperlink still open at the end of `prefix` ("" when none).
pub fn get_active_osc8_close(prefix: &str) -> String {
    if !prefix.contains("\x1b]8;") {
        return String::new();
    }
    let mut active: Option<ActiveHyperlink> = None;
    let mut i = 0usize;
    while i < prefix.len() {
        if let Some(code) = extract_ansi_code(prefix, i) {
            if let Some(hyperlink) = parse_osc8_hyperlink(code) {
                active = hyperlink;
            }
            i += code.len();
        } else {
            i += 1;
        }
    }
    active
        .map(|h| format_osc8_close(h.terminator))
        .unwrap_or_default()
}

// --- ANSI SGR tracker -------------------------------------------------------------

/// Port of `AnsiCodeTracker` — tracks active SGR codes (and the OSC 8
/// hyperlink) so styling can be preserved across line breaks.
#[derive(Debug, Default)]
pub struct AnsiCodeTracker {
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    blink: bool,
    inverse: bool,
    hidden: bool,
    strikethrough: bool,
    /// Full code like `"31"` or `"38;5;240"`.
    fg_color: Option<String>,
    /// Full code like `"41"` or `"48;5;240"`.
    bg_color: Option<String>,
    active_hyperlink: Option<ActiveHyperlink>,
}

impl AnsiCodeTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process(&mut self, ansi_code: &str) {
        // OSC 8 hyperlink open/close. Preserve the original terminator because
        // some terminals only make BEL-terminated links clickable.
        if let Some(hyperlink) = parse_osc8_hyperlink(ansi_code) {
            self.active_hyperlink = hyperlink;
            return;
        }

        if !ansi_code.ends_with('m') {
            return;
        }

        // /\x1b\[([\d;]*)m/ — the parameter body must be digits/semicolons.
        let Some(body) = ansi_code.strip_prefix("\x1b[").and_then(|b| b.strip_suffix('m')) else {
            return;
        };
        if !body.bytes().all(|b| b.is_ascii_digit() || b == b';') {
            return;
        }
        let params = body;

        if params.is_empty() || params == "0" {
            // Full reset
            self.reset();
            return;
        }

        let parts: Vec<&str> = params.split(';').collect();
        let mut i = 0usize;
        while i < parts.len() {
            // Number.parseInt(parts[i], 10) — NaN (empty) matches nothing.
            let code = parts[i].parse::<i64>().ok();

            // 256-color / RGB codes consume multiple parameters. Note: the
            // stored code uses the raw parameter strings, like the original.
            if code == Some(38) || code == Some(48) {
                if parts.get(i + 1).map(|s| s.as_bytes()) == Some(b"5") && i + 2 < parts.len() {
                    let color_code = format!("{};{};{}", parts[i], parts[i + 1], parts[i + 2]);
                    if code == Some(38) {
                        self.fg_color = Some(color_code);
                    } else {
                        self.bg_color = Some(color_code);
                    }
                    i += 3;
                    continue;
                } else if parts.get(i + 1).map(|s| s.as_bytes()) == Some(b"2") && i + 4 < parts.len()
                {
                    let color_code = format!(
                        "{};{};{};{};{}",
                        parts[i],
                        parts[i + 1],
                        parts[i + 2],
                        parts[i + 3],
                        parts[i + 4]
                    );
                    if code == Some(38) {
                        self.fg_color = Some(color_code);
                    } else {
                        self.bg_color = Some(color_code);
                    }
                    i += 5;
                    continue;
                }
            }

            match code {
                Some(0) => self.reset(),
                Some(1) => self.bold = true,
                Some(2) => self.dim = true,
                Some(3) => self.italic = true,
                Some(4) => self.underline = true,
                Some(5) => self.blink = true,
                Some(7) => self.inverse = true,
                Some(8) => self.hidden = true,
                Some(9) => self.strikethrough = true,
                Some(21) => self.bold = false, // Some terminals
                Some(22) => {
                    self.bold = false;
                    self.dim = false;
                }
                Some(23) => self.italic = false,
                Some(24) => self.underline = false,
                Some(25) => self.blink = false,
                Some(27) => self.inverse = false,
                Some(28) => self.hidden = false,
                Some(29) => self.strikethrough = false,
                Some(39) => self.fg_color = None, // Default fg
                Some(49) => self.bg_color = None, // Default bg
                // Standard foreground colors 30-37, 90-97 (stored as the
                // parsed number, like the original's String(code))
                Some(c) if (30..=37).contains(&c) || (90..=97).contains(&c) => {
                    self.fg_color = Some(c.to_string());
                }
                // Standard background colors 40-47, 100-107
                Some(c) if (40..=47).contains(&c) || (100..=107).contains(&c) => {
                    self.bg_color = Some(c.to_string());
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn reset(&mut self) {
        self.bold = false;
        self.dim = false;
        self.italic = false;
        self.underline = false;
        self.blink = false;
        self.inverse = false;
        self.hidden = false;
        self.strikethrough = false;
        self.fg_color = None;
        self.bg_color = None;
        // SGR reset does not affect OSC 8 hyperlink state
    }

    /// Clear all state for reuse.
    pub fn clear(&mut self) {
        self.reset();
        self.active_hyperlink = None;
    }

    pub fn get_active_codes(&self) -> String {
        let mut codes: Vec<String> = Vec::new();
        if self.bold {
            codes.push("1".into());
        }
        if self.dim {
            codes.push("2".into());
        }
        if self.italic {
            codes.push("3".into());
        }
        if self.underline {
            codes.push("4".into());
        }
        if self.blink {
            codes.push("5".into());
        }
        if self.inverse {
            codes.push("7".into());
        }
        if self.hidden {
            codes.push("8".into());
        }
        if self.strikethrough {
            codes.push("9".into());
        }
        if let Some(fg) = &self.fg_color {
            codes.push(fg.clone());
        }
        if let Some(bg) = &self.bg_color {
            codes.push(bg.clone());
        }

        let mut result = if codes.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        };
        if let Some(hyperlink) = &self.active_hyperlink {
            result.push_str(&format_osc8_hyperlink(hyperlink));
        }
        result
    }

    pub fn get_active_background_code(&self) -> String {
        self.bg_color
            .as_ref()
            .map(|bg| format!("\x1b[{bg}m"))
            .unwrap_or_default()
    }

    pub fn has_active_codes(&self) -> bool {
        self.bold
            || self.dim
            || self.italic
            || self.underline
            || self.blink
            || self.inverse
            || self.hidden
            || self.strikethrough
            || self.fg_color.is_some()
            || self.bg_color.is_some()
            || self.active_hyperlink.is_some()
    }

    /// Reset codes for attributes that need to be turned off at line end.
    /// Underline must be closed to prevent bleeding into padding; active
    /// OSC 8 hyperlinks must be closed and re-opened on the next line.
    pub fn get_line_end_reset(&self) -> String {
        let mut result = String::new();
        if self.underline {
            result.push_str("\x1b[24m"); // Underline off only
        }
        if let Some(hyperlink) = &self.active_hyperlink {
            result.push_str(&format_osc8_close(hyperlink.terminator));
        }
        result
    }
}

fn update_tracker_from_text(text: &str, tracker: &mut AnsiCodeTracker) {
    let b = text.as_bytes();
    let mut i = position_of(b, 0x1b, 0);
    while let Some(pos) = i {
        let length = ansi_code_length(text, pos);
        if length > 0 {
            tracker.process(&text[pos..pos + length]);
            i = position_of(b, 0x1b, pos + length);
        } else {
            i = position_of(b, 0x1b, pos + 1);
        }
    }
}

/// Port of `getActiveBackgroundAnsi`.
pub fn get_active_background_ansi(text: &str) -> String {
    let mut tracker = AnsiCodeTracker::new();
    update_tracker_from_text(text, &mut tracker);
    tracker.get_active_background_code()
}

// --- cell ranges and links ---------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphemeCellRange {
    pub start: usize,
    pub end: usize,
}

/// Return the terminal-cell range occupied by the grapheme at a visible column.
pub fn get_grapheme_cell_range(line: &str, column: usize) -> Option<GraphemeCellRange> {
    let mut current_col = 0usize;
    let mut i = 0usize;
    while i < line.len() {
        if let Some(code) = extract_ansi_code(line, i) {
            i += code.len();
            continue;
        }
        let mut text_end = i;
        while text_end < line.len() && ansi_code_length(line, text_end) == 0 {
            text_end += 1;
        }
        for segment in grapheme_segments(&line[i..text_end]) {
            let width = grapheme_width(segment);
            if width > 0 && column >= current_col && column < current_col + width {
                return Some(GraphemeCellRange {
                    start: current_col,
                    end: current_col + width,
                });
            }
            current_col += width;
        }
        i = text_end;
    }
    None
}

/// `/^\x1b\]8;[^;]*;([^\x07\x1b]*)(?:\x07|\x1b\\)$/` — the URL of an OSC 8
/// open sequence, or `None` for non-opens / empty URLs.
fn osc8_open_url(code: &str) -> Option<String> {
    let body = code.strip_prefix("\x1b]8;")?;
    let terminator = if code.ends_with('\x07') { "\x07" } else { "\x1b\\" };
    let body = body.strip_suffix(terminator)?;
    let (_params, url) = body.split_once(';')?;
    if url.is_empty() {
        None
    } else {
        Some(url.to_string())
    }
}

/// Return the OSC 8 hyperlink covering a visible terminal column.
pub fn get_osc8_link_at_column(line: &str, column: usize) -> Option<String> {
    let mut active_url: Option<String> = None;
    let mut current_col = 0usize;
    let mut i = 0usize;
    while i < line.len() {
        if let Some(code) = extract_ansi_code(line, i) {
            if let Some(url) = osc8_open_url(code) {
                active_url = Some(url);
            }
            i += code.len();
            continue;
        }
        let mut text_end = i;
        while text_end < line.len() && ansi_code_length(line, text_end) == 0 {
            text_end += 1;
        }
        for segment in grapheme_segments(&line[i..text_end]) {
            let width = if segment == "\t" {
                3
            } else {
                grapheme_width(segment)
            };
            if column >= current_col && column < current_col + width {
                return active_url;
            }
            current_col += width;
        }
        i = text_end;
    }
    None
}

/// Port of `normalizeTerminalOutput` — decompose precomposed Thai/Lao AM
/// vowels (same cell width, avoids stale-cell repaint artifacts) and expand
/// visible tabs to the fixed 3-column layout width, leaving tabs inside
/// terminal control sequences untouched.
pub fn normalize_terminal_output(s: &str) -> String {
    let normalized = if s.contains('\u{e33}') || s.contains('\u{eb3}') {
        let mut out = String::with_capacity(s.len() + 4);
        for ch in s.chars() {
            match ch {
                '\u{e33}' => out.push_str("\u{e4d}\u{e32}"),
                '\u{eb3}' => out.push_str("\u{ecd}\u{eb2}"),
                _ => out.push(ch),
            }
        }
        out
    } else {
        s.to_string()
    };
    if !normalized.contains('\t') {
        return normalized;
    }

    let mut result = String::new();
    let mut i = 0usize;
    while i < normalized.len() {
        if let Some(code) = extract_ansi_code(&normalized, i) {
            result.push_str(code);
            i += code.len();
            continue;
        }
        let ch = normalized[i..].chars().next().unwrap();
        if ch == '\t' {
            result.push_str("   ");
        } else {
            result.push(ch);
        }
        i += ch.len_utf8();
    }
    result
}

// --- truncation ---------------------------------------------------------------------

fn truncate_fragment_to_width(text: &str, max_width: usize) -> (String, usize) {
    if max_width == 0 || text.is_empty() {
        return (String::new(), 0);
    }

    if is_printable_ascii(text) {
        let clipped = &text[..max_width.min(text.len())];
        return (clipped.to_string(), clipped.len());
    }

    let has_ansi = text.contains('\x1b');
    let has_tabs = text.contains('\t');
    if !has_ansi && !has_tabs {
        let mut result = String::new();
        let mut width = 0usize;
        for segment in grapheme_segments(text) {
            let w = grapheme_width(segment);
            if width + w > max_width {
                break;
            }
            result.push_str(segment);
            width += w;
        }
        return (result, width);
    }

    let b = text.as_bytes();
    let mut result = String::new();
    let mut width = 0usize;
    let mut i = 0usize;
    let mut pending_ansi = String::new();

    while i < text.len() {
        if let Some(code) = extract_ansi_code(text, i) {
            pending_ansi.push_str(code);
            i += code.len();
            continue;
        }

        if b[i] == b'\t' {
            if width + 3 > max_width {
                break;
            }
            if !pending_ansi.is_empty() {
                result.push_str(&pending_ansi);
                pending_ansi.clear();
            }
            result.push('\t');
            width += 3;
            i += 1;
            continue;
        }

        let mut end = i;
        while end < text.len() && b[end] != b'\t' {
            if ansi_code_length(text, end) > 0 {
                break;
            }
            end += 1;
        }

        for segment in grapheme_segments(&text[i..end]) {
            let w = grapheme_width(segment);
            if width + w > max_width {
                return (result, width);
            }
            if !pending_ansi.is_empty() {
                result.push_str(&pending_ansi);
                pending_ansi.clear();
            }
            result.push_str(segment);
            width += w;
        }
        i = end;
    }

    (result, width)
}

fn finalize_truncated_result(
    prefix: &str,
    prefix_width: usize,
    ellipsis: &str,
    ellipsis_width: usize,
    max_width: usize,
    pad: bool,
) -> String {
    let reset = "\x1b[0m";
    let hyperlink_close = get_active_osc8_close(prefix);
    let total_width = prefix_width + ellipsis_width;
    let mut result = if !ellipsis.is_empty() {
        format!("{prefix}{hyperlink_close}{reset}{ellipsis}{reset}")
    } else {
        format!("{prefix}{hyperlink_close}{reset}")
    };

    if pad {
        result.push_str(&" ".repeat(max_width.saturating_sub(total_width)));
    }
    result
}

/// Port of `truncateToWidth` — truncate text to a maximum visible width,
/// adding an ellipsis if needed (ANSI codes don't count toward width).
pub fn truncate_to_width(text: &str, max_width: usize, ellipsis: &str, pad: bool) -> String {
    if max_width == 0 {
        return String::new();
    }

    if text.is_empty() {
        return if pad {
            " ".repeat(max_width)
        } else {
            String::new()
        };
    }

    let ellipsis_width = visible_width(ellipsis);
    if ellipsis_width >= max_width {
        let text_width = visible_width(text);
        if text_width <= max_width {
            return if pad {
                format!("{text}{}", " ".repeat(max_width - text_width))
            } else {
                text.to_string()
            };
        }

        let (clipped_text, clipped_width) = truncate_fragment_to_width(ellipsis, max_width);
        if clipped_width == 0 {
            return if pad {
                " ".repeat(max_width)
            } else {
                String::new()
            };
        }
        return finalize_truncated_result("", 0, &clipped_text, clipped_width, max_width, pad);
    }

    if is_printable_ascii(text) {
        if text.len() <= max_width {
            return if pad {
                format!("{text}{}", " ".repeat(max_width - text.len()))
            } else {
                text.to_string()
            };
        }
        let target_width = max_width - ellipsis_width;
        let prefix = &text[..target_width];
        return finalize_truncated_result(prefix, target_width, ellipsis, ellipsis_width, max_width, pad);
    }

    let target_width = max_width - ellipsis_width;
    let mut result = String::new();
    let mut pending_ansi = String::new();
    let mut visible_so_far = 0usize;
    let mut kept_width = 0usize;
    let mut keep_contiguous_prefix = true;
    let mut overflowed = false;
    let exhausted_input;
    let has_ansi = text.contains('\x1b');
    let has_tabs = text.contains('\t');
    let b = text.as_bytes();

    if !has_ansi && !has_tabs {
        for segment in grapheme_segments(text) {
            let width = grapheme_width(segment);
            if keep_contiguous_prefix && kept_width + width <= target_width {
                result.push_str(segment);
                kept_width += width;
            } else {
                keep_contiguous_prefix = false;
            }
            visible_so_far += width;
            if visible_so_far > max_width {
                overflowed = true;
                break;
            }
        }
        exhausted_input = !overflowed;
    } else {
        let mut i = 0usize;
        while i < text.len() {
            if let Some(code) = extract_ansi_code(text, i) {
                pending_ansi.push_str(code);
                i += code.len();
                continue;
            }

            if b[i] == b'\t' {
                if keep_contiguous_prefix && kept_width + 3 <= target_width {
                    if !pending_ansi.is_empty() {
                        result.push_str(&pending_ansi);
                        pending_ansi.clear();
                    }
                    result.push('\t');
                    kept_width += 3;
                } else {
                    keep_contiguous_prefix = false;
                    pending_ansi.clear();
                }
                visible_so_far += 3;
                if visible_so_far > max_width {
                    overflowed = true;
                    break;
                }
                i += 1;
                continue;
            }

            let mut end = i;
            while end < text.len() && b[end] != b'\t' {
                if ansi_code_length(text, end) > 0 {
                    break;
                }
                end += 1;
            }

            for segment in grapheme_segments(&text[i..end]) {
                let width = grapheme_width(segment);
                if keep_contiguous_prefix && kept_width + width <= target_width {
                    if !pending_ansi.is_empty() {
                        result.push_str(&pending_ansi);
                        pending_ansi.clear();
                    }
                    result.push_str(segment);
                    kept_width += width;
                } else {
                    keep_contiguous_prefix = false;
                    pending_ansi.clear();
                }

                visible_so_far += width;
                if visible_so_far > max_width {
                    overflowed = true;
                    break;
                }
            }
            if overflowed {
                break;
            }
            i = end;
        }
        exhausted_input = i >= text.len();
    }

    if !overflowed && exhausted_input {
        return if pad {
            format!("{text}{}", " ".repeat(max_width.saturating_sub(visible_so_far)))
        } else {
            text.to_string()
        };
    }

    finalize_truncated_result(&result, kept_width, ellipsis, ellipsis_width, max_width, pad)
}

/// `truncateToWidth(text, maxWidth)` with the default ellipsis `"..."`, no pad.
pub fn truncate_to_width_default(text: &str, max_width: usize) -> String {
    truncate_to_width(text, max_width, "...", false)
}

// --- slicing -------------------------------------------------------------------------

/// Port of `sliceByColumn`.
pub fn slice_by_column(line: &str, start_col: usize, length: usize, strict: bool) -> String {
    slice_with_width(line, start_col, length, strict).text
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SliceResult {
    pub text: String,
    pub width: usize,
}

/// Port of `sliceWithWidth` — extract a range of visible columns, handling
/// ANSI codes and wide chars. With `strict`, exclude wide chars at the
/// boundary that would extend past the range.
pub fn slice_with_width(line: &str, start_col: usize, length: usize, strict: bool) -> SliceResult {
    if length == 0 {
        return SliceResult::default();
    }
    let end_col = start_col + length;
    let mut result = String::new();
    let mut result_width = 0usize;
    let mut current_col = 0usize;
    let mut i = 0usize;
    let mut pending_ansi = String::new();

    while i < line.len() {
        if let Some(code) = extract_ansi_code(line, i) {
            if current_col >= start_col && current_col < end_col {
                // Keep original order: codes from before the range must
                // precede codes at the boundary.
                result.push_str(&pending_ansi);
                result.push_str(code);
                pending_ansi.clear();
            } else if current_col < start_col {
                pending_ansi.push_str(code);
            }
            i += code.len();
            continue;
        }

        let mut text_end = i;
        while text_end < line.len() && ansi_code_length(line, text_end) == 0 {
            text_end += 1;
        }

        for segment in grapheme_segments(&line[i..text_end]) {
            let w = grapheme_width(segment);
            let in_range = current_col >= start_col && current_col < end_col;
            let fits = !strict || current_col + w <= end_col;
            if in_range && fits {
                if !pending_ansi.is_empty() {
                    result.push_str(&pending_ansi);
                    pending_ansi.clear();
                }
                result.push_str(segment);
                result_width += w;
            }
            current_col += w;
            if current_col >= end_col {
                break;
            }
        }
        i = text_end;
        if current_col >= end_col {
            break;
        }
    }
    SliceResult {
        text: result,
        width: result_width,
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ExtractedSegments {
    pub before: String,
    pub before_width: usize,
    pub after: String,
    pub after_width: usize,
}

/// Port of `extractSegments` — extract "before" and "after" segments from a
/// line in a single pass (used for overlay compositing). "after" inherits
/// the styling active at `after_start`.
pub fn extract_segments(
    line: &str,
    before_end: usize,
    after_start: usize,
    after_len: usize,
    strict_after: bool,
) -> ExtractedSegments {
    let mut out = ExtractedSegments::default();
    let mut current_col = 0usize;
    let mut i = 0usize;
    let mut pending_ansi_before = String::new();
    let mut after_started = false;
    let after_end = after_start + after_len;

    // Track styling state so "after" inherits styling from before the overlay.
    // (The original pools one instance and clears it; a fresh tracker here is
    // observably identical.)
    let mut tracker = AnsiCodeTracker::new();

    while i < line.len() {
        if let Some(code) = extract_ansi_code(line, i) {
            tracker.process(code);
            if current_col < before_end {
                pending_ansi_before.push_str(code);
            } else if current_col >= after_start
                && current_col < after_end
                && (after_started || code.starts_with("\x1b_"))
            {
                // SGR codes are only included after "after" started (styling
                // already prepended). Zero-width APC markers are not tracked
                // styling, so keep them at the boundary too.
                out.after.push_str(code);
            }
            i += code.len();
            continue;
        }

        let mut text_end = i;
        while text_end < line.len() && ansi_code_length(line, text_end) == 0 {
            text_end += 1;
        }

        for segment in grapheme_segments(&line[i..text_end]) {
            let w = grapheme_width(segment);

            if current_col < before_end && current_col + w <= before_end {
                if !pending_ansi_before.is_empty() {
                    out.before.push_str(&pending_ansi_before);
                    pending_ansi_before.clear();
                }
                out.before.push_str(segment);
                out.before_width += w;
            } else if current_col >= after_start && current_col < after_end {
                let fits = !strict_after || current_col + w <= after_end;
                if fits {
                    // On first "after" grapheme, prepend inherited styling.
                    if !after_started {
                        out.after.push_str(&tracker.get_active_codes());
                        after_started = true;
                    }
                    out.after.push_str(segment);
                    out.after_width += w;
                }
            }

            current_col += w;
            // Early exit: done with "before" only, or done with both segments.
            let done = if after_len == 0 {
                current_col >= before_end
            } else {
                current_col >= after_end
            };
            if done {
                break;
            }
        }
        i = text_end;
        let done = if after_len == 0 {
            current_col >= before_end
        } else {
            current_col >= after_end
        };
        if done {
            break;
        }
    }

    out
}

// --- tokenization and word wrapping -----------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Space,
    Word,
}

fn flush_token(tokens: &mut Vec<String>, current: &mut String, current_kind: &mut Option<TokenKind>) {
    if current.is_empty() {
        return;
    }
    tokens.push(std::mem::take(current));
    *current_kind = None;
}

/// Port of `splitIntoTokensWithAnsi` — split text into words while keeping
/// ANSI codes attached; CJK graphemes become their own tokens.
fn split_into_tokens_with_ansi(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut pending_ansi = String::new(); // codes waiting for next visible content
    let mut current_kind: Option<TokenKind> = None;
    let mut i = 0usize;
    let b = text.as_bytes();

    while i < text.len() {
        if let Some(code) = extract_ansi_code(text, i) {
            pending_ansi.push_str(code);
            i += code.len();
            continue;
        }

        // Visible text runs up to the next valid escape sequence.
        let mut end = position_of(b, 0x1b, i + 1);
        while let Some(e) = end {
            if ansi_code_length(text, e) == 0 {
                end = position_of(b, 0x1b, e + 1);
            } else {
                break;
            }
        }
        let end = end.unwrap_or(text.len());

        let chunk = &text[i..end];
        // Printable ASCII characters are single graphemes, so skip the
        // segmenter for them.
        let ascii = is_printable_ascii(chunk);
        let segments: Vec<&str> = if ascii {
            (0..chunk.len()).map(|k| &chunk[k..k + 1]).collect()
        } else {
            grapheme_segments(chunk).into_iter().collect()
        };
        for segment in segments {
            let segment_is_space = segment == " ";
            if !ascii && !segment_is_space && cjk_break_test(segment) {
                flush_token(&mut tokens, &mut current, &mut current_kind);
                let mut token = String::new();
                token.push_str(&pending_ansi);
                token.push_str(segment);
                pending_ansi.clear();
                tokens.push(token);
                continue;
            }

            let segment_kind = if segment_is_space {
                TokenKind::Space
            } else {
                TokenKind::Word
            };
            if !current.is_empty() && current_kind != Some(segment_kind) {
                flush_token(&mut tokens, &mut current, &mut current_kind);
            }

            // Attach any pending ANSI codes to this visible character.
            if !pending_ansi.is_empty() {
                current.push_str(&pending_ansi);
                pending_ansi.clear();
            }

            current_kind = Some(segment_kind);
            current.push_str(segment);
        }

        i = end;
    }

    // Handle any remaining pending ANSI codes (attach to last token).
    if !pending_ansi.is_empty() {
        if !current.is_empty() {
            current.push_str(&pending_ansi);
        } else if let Some(last) = tokens.last_mut() {
            last.push_str(&pending_ansi);
        } else {
            current = pending_ansi;
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Port of `breakLongWord` — break an over-long token at grapheme
/// boundaries, preserving ANSI state.
fn break_long_word(word: &str, width: usize, tracker: &mut AnsiCodeTracker) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current_line = tracker.get_active_codes();
    let mut current_width = 0usize;

    // First, separate ANSI codes from visible content.
    #[derive(Debug)]
    enum SegKind {
        Ansi,
        Grapheme,
    }
    let mut segments: Vec<(SegKind, String)> = Vec::new();
    let mut i = 0usize;
    while i < word.len() {
        if let Some(code) = extract_ansi_code(word, i) {
            segments.push((SegKind::Ansi, code.to_string()));
            i += code.len();
        } else {
            let mut end = i;
            while end < word.len() {
                if ansi_code_length(word, end) > 0 {
                    break;
                }
                end += 1;
            }
            for seg in grapheme_segments(&word[i..end]) {
                segments.push((SegKind::Grapheme, seg.to_string()));
            }
            i = end;
        }
    }

    for (kind, value) in segments {
        if matches!(kind, SegKind::Ansi) {
            current_line.push_str(&value);
            tracker.process(&value);
            continue;
        }

        let grapheme = value.as_str();
        // Skip empty graphemes to avoid issues with string-width calculation.
        if grapheme.is_empty() {
            continue;
        }

        let grapheme_width = visible_width(grapheme);

        if current_width + grapheme_width > width {
            // Add specific reset for underline only (preserves background).
            let line_end_reset = tracker.get_line_end_reset();
            if !line_end_reset.is_empty() {
                current_line.push_str(&line_end_reset);
            }
            lines.push(std::mem::take(&mut current_line));
            current_line = tracker.get_active_codes();
            current_width = 0;
        }

        current_line.push_str(grapheme);
        current_width += grapheme_width;
    }

    if !current_line.is_empty() {
        // No reset at end of final segment - caller handles continuation.
        lines.push(current_line);
    }

    if !lines.is_empty() {
        lines
    } else {
        vec![String::new()]
    }
}

fn wrap_single_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    let visible_length = visible_width(line);
    if visible_length <= width {
        return vec![line.to_string()];
    }

    let mut wrapped: Vec<String> = Vec::new();
    let mut tracker = AnsiCodeTracker::new();
    let tokens = split_into_tokens_with_ansi(line);

    let mut current_line = String::new();
    let mut current_visible_length = 0usize;

    for token in tokens {
        let token_visible_length = visible_width(&token);
        let is_whitespace = js_trim(&token).is_empty();

        // Token itself is too long - break it character by character.
        if token_visible_length > width && !is_whitespace {
            if !current_line.is_empty() {
                // Add specific reset for underline only (preserves background).
                let line_end_reset = tracker.get_line_end_reset();
                if !line_end_reset.is_empty() {
                    current_line.push_str(&line_end_reset);
                }
                wrapped.push(std::mem::take(&mut current_line));
                // (JS also resets currentVisibleLength to 0 here; the value
                // is immediately recomputed from the broken token below.)
            }

            // Break long token - break_long_word handles its own resets.
            let broken = break_long_word(&token, width, &mut tracker);
            for piece in &broken[..broken.len() - 1] {
                wrapped.push(piece.clone());
            }
            current_line = broken.last().cloned().unwrap_or_default();
            current_visible_length = visible_width(&current_line);
            continue;
        }

        // Check if adding this token would exceed width.
        let total_needed = current_visible_length + token_visible_length;

        if total_needed > width && current_visible_length > 0 {
            // Trim trailing whitespace, then add underline reset (not full
            // reset, to preserve background).
            let mut line_to_wrap = js_trim_end(&current_line).to_string();
            let line_end_reset = tracker.get_line_end_reset();
            if !line_end_reset.is_empty() {
                line_to_wrap.push_str(&line_end_reset);
            }
            wrapped.push(line_to_wrap);
            if is_whitespace {
                // Don't start new line with whitespace.
                current_line = tracker.get_active_codes();
                current_visible_length = 0;
            } else {
                current_line = format!("{}{token}", tracker.get_active_codes());
                current_visible_length = token_visible_length;
            }
        } else {
            // Add to current line.
            current_line.push_str(&token);
            current_visible_length += token_visible_length;
        }

        update_tracker_from_text(&token, &mut tracker);
    }

    if !current_line.is_empty() {
        // No reset at end of final line - let caller handle it.
        wrapped.push(current_line);
    }

    // Trailing whitespace can cause lines to exceed the requested width.
    if !wrapped.is_empty() {
        wrapped
            .into_iter()
            .map(|l| js_trim_end(&l).to_string())
            .collect()
    } else {
        vec![String::new()]
    }
}

/// Port of `wrapTextWithAnsi` — word wrapping with ANSI codes preserved.
/// Only word wrapping: no padding, no background colors; each line is at
/// most `width` visible columns; active ANSI codes carry across breaks.
pub fn wrap_text_with_ansi(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    // Handle newlines by processing each line separately; track ANSI state
    // across lines so styles carry over after literal newlines.
    let input_lines = split_js_lines(text);
    let mut result: Vec<String> = Vec::new();
    let mut tracker = AnsiCodeTracker::new();

    for input_line in input_lines {
        // Prepend active ANSI codes from previous lines (except first line).
        let prefix = if !result.is_empty() {
            tracker.get_active_codes()
        } else {
            String::new()
        };
        let line = format!("{prefix}{input_line}");
        for wrapped_line in wrap_single_line(&line, width) {
            result.push(wrapped_line);
        }
        // Update tracker with codes from this line for next iteration.
        update_tracker_from_text(input_line, &mut tracker);
    }

    if !result.is_empty() {
        result
    } else {
        vec![String::new()]
    }
}

// --- background application ---------------------------------------------------------

/// Port of `applyBackgroundToLine` — apply a background color function to a
/// line padded to the full width.
pub fn apply_background_to_line(line: &str, width: usize, bg_fn: impl Fn(&str) -> String) -> String {
    let visible_len = visible_width(line);
    let padding_needed = width.saturating_sub(visible_len);
    let with_padding = format!("{line}{}", " ".repeat(padding_needed));
    bg_fn(&with_padding)
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

    // --- ported from upstream test/truncate-to-width.test.ts ---

    #[test]
    fn truncate_keeps_output_within_width_for_very_large_unicode_input() {
        let text = "🙂界".repeat(100_000);
        let truncated = truncate_to_width(&text, 40, "…", false);
        assert!(visible_width(&truncated) <= 40);
        assert!(truncated.ends_with("…\x1b[0m"));
    }

    #[test]
    fn truncate_preserves_ansi_styling_and_resets_around_ellipsis() {
        let text = format!("\x1b[31m{}\x1b[0m", "hello ".repeat(1000));
        let truncated = truncate_to_width(&text, 20, "…", false);
        assert!(visible_width(&truncated) <= 20);
        assert!(truncated.contains("\x1b[31m"));
        assert!(truncated.ends_with("\x1b[0m…\x1b[0m"));
    }

    #[test]
    fn truncate_closes_a_bel_terminated_osc8_link() {
        let open = "\x1b]8;;https://example.com\x07";
        let close = "\x1b]8;;\x07";
        let text = format!("{open}some-longer-label-here{close}");
        assert_eq!(
            truncate_to_width(&text, 15, "...", false),
            format!("{open}some-longer-{close}\x1b[0m...\x1b[0m")
        );
    }

    #[test]
    fn truncate_handles_malformed_ansi_escape_prefixes() {
        let text = format!("abc\x1bnot-ansi {}", "🙂".repeat(1000));
        let truncated = truncate_to_width(&text, 20, "…", false);
        assert!(visible_width(&truncated) <= 20);
    }

    #[test]
    fn truncate_clips_wide_ellipsis_safely() {
        assert_eq!(truncate_to_width("abcdef", 1, "🙂", false), "");
        assert_eq!(
            truncate_to_width("abcdef", 2, "🙂", false),
            "\x1b[0m🙂\x1b[0m"
        );
        assert!(visible_width(&truncate_to_width("abcdef", 2, "🙂", false)) <= 2);
    }

    #[test]
    fn truncate_returns_original_text_when_it_fits_even_with_wide_ellipsis() {
        assert_eq!(truncate_to_width("a", 2, "🙂", false), "a");
        assert_eq!(truncate_to_width("界", 2, "🙂", false), "界");
    }

    #[test]
    fn truncate_pads_output_to_requested_width() {
        let truncated = truncate_to_width("🙂界🙂界🙂界", 8, "…", true);
        assert_eq!(visible_width(&truncated), 8);
    }

    #[test]
    fn truncate_adds_trailing_reset_without_ellipsis() {
        let text = format!("\x1b[31m{}", "hello".repeat(100));
        let truncated = truncate_to_width(&text, 10, "", false);
        assert!(visible_width(&truncated) <= 10);
        assert!(truncated.ends_with("\x1b[0m"));
    }

    #[test]
    fn truncate_keeps_contiguous_prefix_instead_of_skipping_wide_graphemes() {
        let truncated = truncate_to_width("🙂\t界 \x1b_abc\x07", 7, "…", true);
        assert_eq!(truncated, "🙂\t\x1b[0m…\x1b[0m ");
    }

    #[test]
    fn visible_width_counts_indic_conjunct_spacing_code_points() {
        assert_eq!(visible_width("र्क"), 2);
        assert_eq!(visible_width("नेटवर्क"), 5);
        assert_eq!(visible_width("सर्वाधिकार सुरक्षित। ऑर्डर पर क्लिक करें"), 33);
        assert_eq!(visible_width("র্ক"), 2);
        assert_eq!(visible_width("ર્ક"), 2);
        assert_eq!(visible_width("ର୍କ"), 2);
        assert_eq!(visible_width("ర్క"), 2);
        assert_eq!(visible_width("ര്‍ക"), 2);
    }

    #[test]
    fn visible_width_keeps_ordinary_combining_marks_zero_width() {
        assert_eq!(visible_width("e\u{301}"), 1);
        assert_eq!(visible_width("čřžůú"), 5);
        assert_eq!(visible_width("שָׁ"), 1);
        assert_eq!(visible_width("بّ"), 1);
        assert_eq!(visible_width("རྐ"), 1);
        assert_eq!(visible_width("ᜠ᜴"), 1);
        assert_eq!(visible_width("가〮"), 2);
        assert_eq!(visible_width("가〯"), 2);
    }

    #[test]
    fn visible_width_keeps_cjk_and_japanese_accounting() {
        assert_eq!(visible_width("网络"), 4);
        assert_eq!(visible_width("ネットワーク"), 12);
        assert_eq!(visible_width("が"), 2);
        assert_eq!(visible_width("か\u{3099}"), 2);
    }

    #[test]
    fn visible_width_counts_myanmar_marks_that_terminals_allocate_cells_for() {
        assert_eq!(visible_width("ကာ"), 2);
        assert_eq!(visible_width("ကေ"), 2);
        assert_eq!(visible_width("က်"), 2);
        assert_eq!(visible_width("ကျ"), 2);
        assert_eq!(visible_width("ကြ"), 2);
        assert_eq!(visible_width("ကဳ"), 2);
        assert_eq!(visible_width("ကဴ"), 2);
        assert_eq!(visible_width("ကဵ"), 2);
        assert_eq!(visible_width("ကး"), 2);
        assert_eq!(visible_width("ကို"), 1);
        assert_eq!(visible_width("က္"), 1);
    }

    #[test]
    fn visible_width_keeps_thai_and_lao_am_clusters_at_normal_cell_width() {
        assert_eq!(visible_width("ำ"), 1);
        assert_eq!(visible_width("ຳ"), 1);
        assert_eq!(visible_width("กำ"), 2);
        assert_eq!(visible_width("ກຳ"), 2);
    }

    #[test]
    fn normalize_terminal_output_decomposes_thai_and_lao_am_only() {
        assert_eq!(normalize_terminal_output("ำ"), "ํา");
        assert_eq!(normalize_terminal_output("ຳ"), "ໍາ");
        assert_eq!(visible_width(&normalize_terminal_output("ำabc")), visible_width("ำabc"));
        assert_eq!(visible_width(&normalize_terminal_output("ຳabc")), visible_width("ຳabc"));
    }

    // --- ported from upstream test/wrap-ansi.test.ts ---

    #[test]
    fn wrap_should_not_apply_underline_style_before_the_styled_text() {
        let underline_on = "\x1b[4m";
        let underline_off = "\x1b[24m";
        let url = "https://example.com/very/long/path/that/will/wrap";
        let text = format!("read this thread {underline_on}{url}{underline_off}");

        let wrapped = wrap_text_with_ansi(&text, 40);
        assert_eq!(wrapped[0], "read this thread");
        assert!(wrapped[1].starts_with(underline_on));
        assert!(wrapped[1].contains("https://"));
    }

    #[test]
    fn wrap_should_not_have_whitespace_before_underline_reset_code() {
        let underline_on = "\x1b[4m";
        let underline_off = "\x1b[24m";
        let text = format!("{underline_on}underlined text here {underline_off}more");

        let wrapped = wrap_text_with_ansi(&text, 18);
        assert!(!wrapped[0].contains(&format!(" {underline_off}")));
    }

    #[test]
    fn wrap_should_not_bleed_underline_to_padding() {
        let underline_on = "\x1b[4m";
        let underline_off = "\x1b[24m";
        let url = "https://example.com/very/long/path/that/will/definitely/wrap";
        let text = format!("prefix {underline_on}{url}{underline_off} suffix");

        let wrapped = wrap_text_with_ansi(&text, 30);
        for line in wrapped.iter().take(wrapped.len() - 1).skip(1) {
            if line.contains(underline_on) {
                assert!(line.ends_with(underline_off));
                assert!(!line.ends_with("\x1b[0m"));
            }
        }
    }

    #[test]
    fn wrap_preserves_background_color_across_lines_without_full_reset() {
        let bg_blue = "\x1b[44m";
        let reset = "\x1b[0m";
        let text = format!("{bg_blue}hello world this is blue background text{reset}");

        let wrapped = wrap_text_with_ansi(&text, 15);
        for line in &wrapped {
            assert!(line.contains(bg_blue));
        }
        for line in wrapped.iter().take(wrapped.len() - 1) {
            assert!(!line.ends_with("\x1b[0m"));
        }
    }

    #[test]
    fn wrap_resets_underline_but_preserves_background() {
        let underline_on = "\x1b[4m";
        let underline_off = "\x1b[24m";
        let reset = "\x1b[0m";
        let text = format!(
            "\x1b[41mprefix {underline_on}UNDERLINED_CONTENT_THAT_WRAPS{underline_off} suffix{reset}"
        );

        let wrapped = wrap_text_with_ansi(&text, 20);
        for line in &wrapped {
            let has_bg_color =
                line.contains("[41m") || line.contains(";41m") || line.contains("[41;");
            assert!(has_bg_color);
        }
        for line in wrapped.iter().take(wrapped.len() - 1) {
            if (line.contains("[4m") || line.contains("[4;") || line.contains(";4m"))
                && !line.contains(underline_off)
            {
                assert!(line.ends_with(underline_off));
                assert!(!line.ends_with("\x1b[0m"));
            }
        }
    }

    #[test]
    fn wrap_handles_lf_crlf_and_cr_line_endings() {
        assert_eq!(
            wrap_text_with_ansi("first\nsecond\r\nthird\rfourth", 80),
            vec!["first", "second", "third", "fourth"]
        );
    }

    #[test]
    fn wrap_preserves_ansi_state_across_crlf_and_cr() {
        let red = "\x1b[31m";
        let reset = "\x1b[0m";
        assert_eq!(
            wrap_text_with_ansi(&format!("{red}first\r\nsecond\rthird{reset}"), 80),
            vec![
                format!("{red}first"),
                format!("{red}second"),
                format!("{red}third{reset}"),
            ]
        );
    }

    #[test]
    fn wrap_wraps_plain_text_correctly() {
        let wrapped = wrap_text_with_ansi("hello world this is a test", 10);
        assert!(wrapped.len() > 1);
        for line in &wrapped {
            assert!(visible_width(line) <= 10);
        }
    }

    #[test]
    fn wrap_breaks_cjk_runs_at_grapheme_boundaries() {
        let text = "This is an example 中文汉字测试段落内容中文汉字测试段落内容.";
        let wrapped = wrap_text_with_ansi(text, 40);
        assert_eq!(
            wrapped,
            vec!["This is an example 中文汉字测试段落内容", "中文汉字测试段落内容."]
        );
        for line in &wrapped {
            assert!(visible_width(line) <= 40);
        }
    }

    #[test]
    fn wrap_preserves_color_codes_when_wrapping_cjk_runs() {
        let red = "\x1b[31m";
        let reset = "\x1b[0m";
        let text =
            format!("{red}This is an example 中文汉字测试段落内容中文汉字测试段落内容.{reset}");
        let wrapped = wrap_text_with_ansi(&text, 40);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0], format!("{red}This is an example 中文汉字测试段落内容"));
        assert_eq!(wrapped[1], format!("{red}中文汉字测试段落内容.{reset}"));
        for line in &wrapped {
            assert!(visible_width(line) <= 40);
        }
    }

    #[test]
    fn wrap_ignores_osc133_semantic_markers_in_visible_width() {
        assert_eq!(visible_width("\x1b]133;A\x07hello\x1b]133;B\x07"), 5);
    }

    #[test]
    fn wrap_ignores_st_terminated_osc_sequences_in_visible_width() {
        assert_eq!(visible_width("\x1b]133;A\x1b\\hello\x1b]133;B\x1b\\"), 5);
    }

    #[test]
    fn wrap_treats_isolated_regional_indicators_as_width_2() {
        assert_eq!(visible_width("🇨"), 2);
        assert_eq!(visible_width("🇨🇳"), 2);
    }

    #[test]
    fn wrap_truncates_trailing_whitespace_that_exceeds_width() {
        let wrapped = wrap_text_with_ansi("  ", 1);
        assert!(visible_width(&wrapped[0]) <= 1);
    }

    #[test]
    fn wrap_preserves_color_codes_across_wraps() {
        let red = "\x1b[31m";
        let reset = "\x1b[0m";
        let text = format!("{red}hello world this is red{reset}");

        let wrapped = wrap_text_with_ansi(&text, 10);
        for line in wrapped.iter().skip(1) {
            assert!(line.starts_with(red));
        }
        for line in wrapped.iter().take(wrapped.len() - 1) {
            assert!(!line.ends_with("\x1b[0m"));
        }
    }

    #[test]
    fn wrap_reemits_osc8_open_at_start_of_continuation_lines() {
        let url = "https://example.com";
        let input = format!("\x1b]8;;{url}\x1b\\0123456789\x1b]8;;\x1b\\");
        let lines = wrap_text_with_ansi(&input, 6);
        for line in &lines {
            let stripped = strip_terminal_sequences(line);
            if !js_trim(&stripped).is_empty() {
                assert!(
                    line.starts_with(&format!("\x1b]8;;{url}\x1b\\"))
                        || line.contains(&format!("\x1b]8;;{url}\x1b\\")),
                    "Line \"{line}\" has visible text but no OSC 8 re-open"
                );
            }
        }
    }

    #[test]
    fn wrap_closes_osc8_before_each_line_break() {
        let url = "https://example.com";
        let input = format!("\x1b]8;;{url}\x1b\\0123456789\x1b]8;;\x1b\\");
        let lines = wrap_text_with_ansi(&input, 6);
        for line in lines.iter().take(lines.len() - 1) {
            if line.contains(&format!("\x1b]8;;{url}\x1b\\")) {
                assert!(
                    line.ends_with("\x1b]8;;\x1b\\"),
                    "Non-final line \"{line}\" is inside a hyperlink but does not close it"
                );
            }
        }
    }

    #[test]
    fn wrap_preserves_bel_terminators_when_wrapping_oauth_hyperlinks() {
        let url = format!("https://example.com/oauth/{}", "a".repeat(32));
        let input = format!("\x1b]8;;{url}\x07{url}\x1b]8;;\x07");
        let lines = wrap_text_with_ansi(&input, 20);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line.contains(&format!("\x1b]8;;{url}\x07")));
            assert!(!line.contains(&format!("\x1b]8;;{url}\x1b\\")));
        }
        for line in lines.iter().take(lines.len() - 1) {
            assert!(line.ends_with("\x1b]8;;\x07"));
        }
    }

    #[test]
    fn wrap_does_not_emit_osc8_outside_the_hyperlink() {
        let url = "https://example.com";
        let input = format!("before \x1b]8;;{url}\x1b\\link\x1b]8;;\x1b\\ after");
        let lines = wrap_text_with_ansi(&input, 80);
        assert_eq!(lines.len(), 1);
        // exactly one open and one close
        assert_eq!(lines[0].matches("\x1b]8;;https:").count(), 1);
        assert_eq!(lines[0].matches("\x1b]8;;\x1b\\").count(), 1);
    }

    // --- ported from upstream test/tab-width.test.ts (utils parts) ---

    #[test]
    fn slice_helper_widths_consistent_with_visible_width() {
        let text = "out 192M\t.pi/skill-tests/results-ha";
        let slice = slice_with_width(text, 0, 10, true);
        assert_eq!(slice.text, "out 192M");
        assert_eq!(slice.width, 8);
        assert_eq!(visible_width(&slice.text), slice.width);
    }

    #[test]
    fn overlay_segment_widths_consistent_with_visible_width() {
        let text = "out 192M\t.pi/skill-tests/results-ha";
        let segments = extract_segments(text, 10, 13, 10, true);
        assert_eq!(segments.before, "out 192M");
        assert_eq!(segments.before_width, 8);
        assert_eq!(visible_width(&segments.before), segments.before_width);

        let tab_fits = extract_segments(text, 11, 13, 10, true);
        assert_eq!(tab_fits.before, "out 192M\t");
        assert_eq!(tab_fits.before_width, 11);
        assert_eq!(visible_width(&tab_fits.before), tab_fits.before_width);
    }

    #[test]
    fn tabs_inside_terminal_control_sequences_stay_byte_identical() {
        let control_sequences = [
            "\x1b]8;;https://example.test/a\tb\x07",
            "\x1b]0;window\ttitle\x1b\\",
            "\x1b_payload\tdata\x1b\\",
        ];
        for control_sequence in control_sequences {
            assert_eq!(
                normalize_terminal_output(&format!("{control_sequence}label\ttext")),
                format!("{control_sequence}label   text")
            );
        }
    }

    // --- ported from upstream test/regression-slice-by-column-ansi-order.test.ts ---

    #[test]
    fn slice_by_column_keeps_reset_at_slice_start_after_earlier_style_codes() {
        let line = "\x1b[32mfoo\x1b[39m bar";
        assert_eq!(slice_by_column(line, 3, 4, true), "\x1b[32m\x1b[39m bar");
    }

    #[test]
    fn slice_by_column_does_not_leak_color_after_highlighted_token() {
        let line = "Another \x1b[35malpha\x1b[39m line with \x1b[35mbeta\x1b[39m later.";
        let after = slice_by_column(line, 13, 100, true);
        assert_eq!(after, "\x1b[35m\x1b[39m line with \x1b[35mbeta\x1b[39m later.");
    }
}
