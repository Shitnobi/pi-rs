//! Keyboard input handling for terminal applications.
//!
//! Rust port of `packages/tui/src/keys.ts` (1:1 behavior).
//!
//! Supports both legacy terminal sequences and Kitty keyboard protocol.
//! See: https://sw.kovidgoyal.net/kitty/keyboard-protocol/
//!
//! API:
//! - [`matches_key`] — check if input matches a key identifier
//! - [`parse_key`] — parse input and return the key identifier
//! - [`Key`] — helper for creating typed key identifiers
//! - [`set_kitty_protocol_active`] / [`is_kitty_protocol_active`] — global
//!   Kitty protocol state
//! - [`decode_kitty_printable`] / [`decode_printable_key`] — decode CSI-u /
//!   modifyOtherKeys sequences into printable characters
//!
//! Number semantics: the TS original uses JS numbers; codepoints are `i64`
//! (arrow/functional sentinels are negative) and modifiers are `i32` so the
//! bitwise `& ~LOCK_MASK` arithmetic matches JS int32 semantics (including
//! pathological `;0u` inputs that produce a `-1` modifier).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

// =============================================================================
// Global Kitty Protocol State
// =============================================================================

static KITTY_PROTOCOL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set the global Kitty keyboard protocol state.
/// Called by ProcessTerminal after detecting protocol support.
pub fn set_kitty_protocol_active(active: bool) {
    KITTY_PROTOCOL_ACTIVE.store(active, Ordering::SeqCst);
}

/// Query whether Kitty keyboard protocol is currently active.
pub fn is_kitty_protocol_active() -> bool {
    KITTY_PROTOCOL_ACTIVE.load(Ordering::SeqCst)
}

// Test-only environment overrides so env-dependent behavior can be exercised
// without racing parallel test threads (mirrors the TS `withEnv` helpers).
thread_local! {
    static ENV_OVERRIDES: RefCell<HashMap<String, Option<String>>> = RefCell::new(HashMap::new());
}

pub(crate) fn env_get(name: &str) -> Option<String> {
    ENV_OVERRIDES.with(|overrides| {
        if let Some(value) = overrides.borrow().get(name) {
            return value.clone();
        }
        std::env::var(name).ok()
    })
}

#[cfg(test)]
pub(crate) fn with_env_vars<F, R>(vars: &[(&str, Option<&str>)], f: F) -> R
where
    F: FnOnce() -> R,
{
    let saved: Vec<(String, Option<String>)> = ENV_OVERRIDES.with(|overrides| {
        let mut overrides = overrides.borrow_mut();
        vars.iter()
            .map(|(name, value)| ((*name).to_string(), overrides.insert((*name).to_string(), value.map(|v| v.to_string())).flatten()))
            .collect()
    });
    let result = f();
    ENV_OVERRIDES.with(|overrides| {
        let mut overrides = overrides.borrow_mut();
        for (name, previous) in saved {
            match previous {
                Some(value) => {
                    overrides.insert(name, Some(value));
                }
                None => {
                    overrides.remove(&name);
                }
            }
        }
    });
    result
}

// =============================================================================
// Constants
// =============================================================================

pub const MODIFIERS_SHIFT: i32 = 1;
pub const MODIFIERS_ALT: i32 = 2;
pub const MODIFIERS_CTRL: i32 = 4;
pub const MODIFIERS_SUPER: i32 = 8;

/// Caps Lock + Num Lock
pub const LOCK_MASK: i32 = 64 + 128;

pub const CODEPOINT_ESCAPE: i64 = 27;
pub const CODEPOINT_TAB: i64 = 9;
pub const CODEPOINT_ENTER: i64 = 13;
pub const CODEPOINT_SPACE: i64 = 32;
pub const CODEPOINT_BACKSPACE: i64 = 127;
/// Numpad Enter (Kitty protocol)
pub const CODEPOINT_KP_ENTER: i64 = 57414;

pub const ARROW_CODEPOINT_UP: i64 = -1;
pub const ARROW_CODEPOINT_DOWN: i64 = -2;
pub const ARROW_CODEPOINT_RIGHT: i64 = -3;
pub const ARROW_CODEPOINT_LEFT: i64 = -4;

pub const FUNCTIONAL_CODEPOINT_DELETE: i64 = -10;
pub const FUNCTIONAL_CODEPOINT_INSERT: i64 = -11;
pub const FUNCTIONAL_CODEPOINT_PAGE_UP: i64 = -12;
pub const FUNCTIONAL_CODEPOINT_PAGE_DOWN: i64 = -13;
pub const FUNCTIONAL_CODEPOINT_HOME: i64 = -14;
pub const FUNCTIONAL_CODEPOINT_END: i64 = -15;

/// `String.fromCharCode` semantics (JS `ToUint16`).
fn js_from_char_code(code: i64) -> char {
    // JS `String.fromCharCode` (ToUint16). Lone-surrogate values cannot occur
    // in Rust `char`; they are only ever used for symbol-set membership tests
    // (where they never match) or after ASCII range checks, so the replacement
    // character is behaviorally equivalent.
    char::from_u32((code & 0xFFFF) as u32).unwrap_or('\u{FFFD}')
}

/// UTF-16 code-unit length of a Rust string (JS `string.length`).
fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

fn is_symbol_key(c: char) -> bool {
    matches!(
        c,
        '`' | '-'
            | '='
            | '['
            | ']'
            | '\\'
            | ';'
            | '\''
            | ','
            | '.'
            | '/'
            | '!'
            | '@'
            | '#'
            | '$'
            | '%'
            | '^'
            | '&'
            | '*'
            | '('
            | ')'
            | '_'
            | '+'
            | '|'
            | '~'
            | '{'
            | '}'
            | ':'
            | '<'
            | '>'
            | '?'
    )
}

fn is_shifted_symbol_key(c: char) -> bool {
    matches!(
        c,
        '~' | '!'
            | '@'
            | '#'
            | '$'
            | '%'
            | '^'
            | '&'
            | '*'
            | '('
            | ')'
            | '_'
            | '+'
            | '|'
            | '{'
            | '}'
            | ':'
            | '<'
            | '>'
            | '?'
    )
}

const MODIFIER_NAMES: [&str; 4] = ["shift", "alt", "ctrl", "super"];

/// Kitty keypad functional key equivalents (KP_* codepoints → logical keys).
fn kitty_functional_key_equivalent(codepoint: i64) -> Option<i64> {
    match codepoint {
        57399 => Some(48),   // KP_0 -> 0
        57400 => Some(49),   // KP_1 -> 1
        57401 => Some(50),   // KP_2 -> 2
        57402 => Some(51),   // KP_3 -> 3
        57403 => Some(52),   // KP_4 -> 4
        57404 => Some(53),   // KP_5 -> 5
        57405 => Some(54),   // KP_6 -> 6
        57406 => Some(55),   // KP_7 -> 7
        57407 => Some(56),   // KP_8 -> 8
        57408 => Some(57),   // KP_9 -> 9
        57409 => Some(46),   // KP_DECIMAL -> .
        57410 => Some(47),   // KP_DIVIDE -> /
        57411 => Some(42),   // KP_MULTIPLY -> *
        57412 => Some(45),   // KP_SUBTRACT -> -
        57413 => Some(43),   // KP_ADD -> +
        57415 => Some(61),   // KP_EQUAL -> =
        57416 => Some(44),   // KP_SEPARATOR -> ,
        57417 => Some(ARROW_CODEPOINT_LEFT),
        57418 => Some(ARROW_CODEPOINT_RIGHT),
        57419 => Some(ARROW_CODEPOINT_UP),
        57420 => Some(ARROW_CODEPOINT_DOWN),
        57421 => Some(FUNCTIONAL_CODEPOINT_PAGE_UP),
        57422 => Some(FUNCTIONAL_CODEPOINT_PAGE_DOWN),
        57423 => Some(FUNCTIONAL_CODEPOINT_HOME),
        57424 => Some(FUNCTIONAL_CODEPOINT_END),
        57425 => Some(FUNCTIONAL_CODEPOINT_INSERT),
        57426 => Some(FUNCTIONAL_CODEPOINT_DELETE),
        _ => None,
    }
}

fn normalize_kitty_functional_codepoint(codepoint: i64) -> i64 {
    kitty_functional_key_equivalent(codepoint).unwrap_or(codepoint)
}

fn normalize_shifted_letter_identity_codepoint(codepoint: i64, modifier: i32) -> i64 {
    let effective_modifier = modifier & !LOCK_MASK;
    if (effective_modifier & MODIFIERS_SHIFT) != 0 && (65..=90).contains(&codepoint) {
        return codepoint + 32;
    }
    codepoint
}

/// Which symbol set [`get_logical_shifted_symbol_candidate`] may draw from.
#[derive(Clone, Copy, PartialEq)]
enum SymbolSet {
    Symbols,
    ShiftedSymbols,
}

impl SymbolSet {
    fn contains(self, c: char) -> bool {
        match self {
            SymbolSet::Symbols => is_symbol_key(c),
            SymbolSet::ShiftedSymbols => is_shifted_symbol_key(c),
        }
    }
}

/// Kitty reports symbols such as "+" as Shift plus the physical "=" key.
/// Bindings name the produced symbol, so consume that implicit Shift while
/// preserving Ctrl, Alt, and Super.
fn get_logical_shifted_symbol_candidate(
    codepoint: Option<i64>,
    modifier: i32,
    allowed_symbols: SymbolSet,
) -> Option<(i64, i32)> {
    let codepoint = codepoint?;
    if (modifier & MODIFIERS_SHIFT) == 0 {
        return None;
    }
    let normalized_codepoint = normalize_kitty_functional_codepoint(codepoint);
    if !allowed_symbols.contains(js_from_char_code(normalized_codepoint)) {
        return None;
    }
    Some((normalized_codepoint, modifier & !MODIFIERS_SHIFT))
}

// =============================================================================
// Legacy Sequences
// =============================================================================

fn legacy_key_sequences(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "up" => Some(&["\x1b[A", "\x1bOA"]),
        "down" => Some(&["\x1b[B", "\x1bOB"]),
        "right" => Some(&["\x1b[C", "\x1bOC"]),
        "left" => Some(&["\x1b[D", "\x1bOD"]),
        "home" => Some(&["\x1b[H", "\x1bOH", "\x1b[1~", "\x1b[7~"]),
        "end" => Some(&["\x1b[F", "\x1bOF", "\x1b[4~", "\x1b[8~"]),
        "insert" => Some(&["\x1b[2~"]),
        "delete" => Some(&["\x1b[3~"]),
        "pageUp" => Some(&["\x1b[5~", "\x1b[[5~"]),
        "pageDown" => Some(&["\x1b[6~", "\x1b[[6~"]),
        "clear" => Some(&["\x1b[E", "\x1bOE"]),
        "f1" => Some(&["\x1bOP", "\x1b[11~", "\x1b[[A"]),
        "f2" => Some(&["\x1bOQ", "\x1b[12~", "\x1b[[B"]),
        "f3" => Some(&["\x1bOR", "\x1b[13~", "\x1b[[C"]),
        "f4" => Some(&["\x1bOS", "\x1b[14~", "\x1b[[D"]),
        "f5" => Some(&["\x1b[15~", "\x1b[[E"]),
        "f6" => Some(&["\x1b[17~"]),
        "f7" => Some(&["\x1b[18~"]),
        "f8" => Some(&["\x1b[19~"]),
        "f9" => Some(&["\x1b[20~"]),
        "f10" => Some(&["\x1b[21~"]),
        "f11" => Some(&["\x1b[23~"]),
        "f12" => Some(&["\x1b[24~"]),
        _ => None,
    }
}

fn legacy_shift_sequences(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "up" => Some(&["\x1b[a"]),
        "down" => Some(&["\x1b[b"]),
        "right" => Some(&["\x1b[c"]),
        "left" => Some(&["\x1b[d"]),
        "clear" => Some(&["\x1b[e"]),
        "insert" => Some(&["\x1b[2$"]),
        "delete" => Some(&["\x1b[3$"]),
        "pageUp" => Some(&["\x1b[5$"]),
        "pageDown" => Some(&["\x1b[6$"]),
        "home" => Some(&["\x1b[7$"]),
        "end" => Some(&["\x1b[8$"]),
        _ => None,
    }
}

fn legacy_ctrl_sequences(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "up" => Some(&["\x1bOa"]),
        "down" => Some(&["\x1bOb"]),
        "right" => Some(&["\x1bOc"]),
        "left" => Some(&["\x1bOd"]),
        "clear" => Some(&["\x1bOe"]),
        "insert" => Some(&["\x1b[2^"]),
        "delete" => Some(&["\x1b[3^"]),
        "pageUp" => Some(&["\x1b[5^"]),
        "pageDown" => Some(&["\x1b[6^"]),
        "home" => Some(&["\x1b[7^"]),
        "end" => Some(&["\x1b[8^"]),
        _ => None,
    }
}

fn legacy_sequence_key_id(sequence: &str) -> Option<&'static str> {
    match sequence {
        "\x1bOA" => Some("up"),
        "\x1bOB" => Some("down"),
        "\x1bOC" => Some("right"),
        "\x1bOD" => Some("left"),
        "\x1bOH" => Some("home"),
        "\x1bOF" => Some("end"),
        "\x1b[E" => Some("clear"),
        "\x1bOE" => Some("clear"),
        "\x1bOe" => Some("ctrl+clear"),
        "\x1b[e" => Some("shift+clear"),
        "\x1b[2~" => Some("insert"),
        "\x1b[2$" => Some("shift+insert"),
        "\x1b[2^" => Some("ctrl+insert"),
        "\x1b[3$" => Some("shift+delete"),
        "\x1b[3^" => Some("ctrl+delete"),
        "\x1b[[5~" => Some("pageUp"),
        "\x1b[[6~" => Some("pageDown"),
        "\x1b[a" => Some("shift+up"),
        "\x1b[b" => Some("shift+down"),
        "\x1b[c" => Some("shift+right"),
        "\x1b[d" => Some("shift+left"),
        "\x1bOa" => Some("ctrl+up"),
        "\x1bOb" => Some("ctrl+down"),
        "\x1bOc" => Some("ctrl+right"),
        "\x1bOd" => Some("ctrl+left"),
        "\x1b[5$" => Some("shift+pageUp"),
        "\x1b[6$" => Some("shift+pageDown"),
        "\x1b[7$" => Some("shift+home"),
        "\x1b[8$" => Some("shift+end"),
        "\x1b[5^" => Some("ctrl+pageUp"),
        "\x1b[6^" => Some("ctrl+pageDown"),
        "\x1b[7^" => Some("ctrl+home"),
        "\x1b[8^" => Some("ctrl+end"),
        "\x1bOP" => Some("f1"),
        "\x1bOQ" => Some("f2"),
        "\x1bOR" => Some("f3"),
        "\x1bOS" => Some("f4"),
        "\x1b[11~" => Some("f1"),
        "\x1b[12~" => Some("f2"),
        "\x1b[13~" => Some("f3"),
        "\x1b[14~" => Some("f4"),
        "\x1b[[A" => Some("f1"),
        "\x1b[[B" => Some("f2"),
        "\x1b[[C" => Some("f3"),
        "\x1b[[D" => Some("f4"),
        "\x1b[[E" => Some("f5"),
        "\x1b[15~" => Some("f5"),
        "\x1b[17~" => Some("f6"),
        "\x1b[18~" => Some("f7"),
        "\x1b[19~" => Some("f8"),
        "\x1b[20~" => Some("f9"),
        "\x1b[21~" => Some("f10"),
        "\x1b[23~" => Some("f11"),
        "\x1b[24~" => Some("f12"),
        "\x1bb" => Some("alt+left"),
        "\x1bf" => Some("alt+right"),
        "\x1bp" => Some("alt+up"),
        "\x1bn" => Some("alt+down"),
        _ => None,
    }
}

fn matches_legacy_sequence(data: &str, sequences: &[&str]) -> bool {
    sequences.contains(&data)
}

fn matches_legacy_modifier_sequence(data: &str, key: &str, modifier: i32) -> bool {
    if modifier == MODIFIERS_SHIFT {
        return matches_legacy_sequence(data, legacy_shift_sequences(key).unwrap_or(&[]));
    }
    if modifier == MODIFIERS_CTRL {
        return matches_legacy_sequence(data, legacy_ctrl_sequences(key).unwrap_or(&[]));
    }
    false
}

// =============================================================================
// Kitty Protocol Parsing
// =============================================================================

/// Event types from Kitty keyboard protocol (flag 2).
/// 1 = key press, 2 = key repeat, 3 = key release
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventType {
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedKittySequence {
    pub codepoint: i64,
    /// Shifted version of the key (when shift is pressed)
    pub shifted_key: Option<i64>,
    /// Key in standard PC-101 layout (for non-Latin layouts)
    pub base_layout_key: Option<i64>,
    pub modifier: i32,
    /// Kept for parity with the TS struct (stored globally, never read back).
    #[allow(dead_code)]
    pub event_type: KeyEventType,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedModifyOtherKeysSequence {
    pub codepoint: i64,
    pub modifier: i32,
}

/// Store the last parsed event type (mirrors the TS module state; the TS
/// variable is written by `parseKittySequence` but never read back).
static LAST_EVENT_TYPE: AtomicU8 = AtomicU8::new(0);

fn store_last_event_type(event_type: KeyEventType) {
    let value = match event_type {
        KeyEventType::Press => 0,
        KeyEventType::Repeat => 1,
        KeyEventType::Release => 2,
    };
    LAST_EVENT_TYPE.store(value, Ordering::SeqCst);
}

/// Check if the last parsed key event was a key release.
/// Only meaningful when Kitty keyboard protocol with flag 2 is active.
pub fn is_key_release(data: &str) -> bool {
    // Don't treat bracketed paste content as key release, even if it contains
    // patterns like ":3F" (e.g., bluetooth MAC addresses like "90:62:3F:A5").
    // Terminal.ts re-wraps paste content with bracketed paste markers before
    // passing to TUI, so pasted data will always contain \x1b[200~.
    if data.contains("\x1b[200~") {
        return false;
    }

    // Quick check: release events with flag 2 contain ":3"
    // Format: \x1b[<codepoint>;<modifier>:3u
    data.contains(":3u")
        || data.contains(":3~")
        || data.contains(":3A")
        || data.contains(":3B")
        || data.contains(":3C")
        || data.contains(":3D")
        || data.contains(":3H")
        || data.contains(":3F")
}

/// Check if the last parsed key event was a key repeat.
/// Only meaningful when Kitty keyboard protocol with flag 2 is active.
pub fn is_key_repeat(data: &str) -> bool {
    // Don't treat bracketed paste content as key repeat, even if it contains
    // patterns like ":2F". See isKeyRelease() for details.
    if data.contains("\x1b[200~") {
        return false;
    }

    data.contains(":2u")
        || data.contains(":2~")
        || data.contains(":2A")
        || data.contains(":2B")
        || data.contains(":2C")
        || data.contains(":2D")
        || data.contains(":2H")
        || data.contains(":2F")
}

fn parse_event_type(event_type_str: Option<u64>) -> KeyEventType {
    let Some(event_type) = event_type_str else {
        return KeyEventType::Press;
    };
    match event_type {
        2 => KeyEventType::Repeat,
        3 => KeyEventType::Release,
        _ => KeyEventType::Press,
    }
}

/// One or more ASCII digits, parsed with saturating arithmetic
/// (JS `parseInt` produces an IEEE double; saturating u64 is equivalent for
/// every value that fits and clamps beyond 2^64).
fn parse_digits(bytes: &[u8], i: &mut usize) -> Option<u64> {
    let start = *i;
    let mut value: u64 = 0;
    while let Some(&c) = bytes.get(*i) {
        if c.is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add((c - b'0') as u64);
            *i += 1;
        } else {
            break;
        }
    }
    if *i == start {
        None
    } else {
        Some(value)
    }
}

/// Zero or more ASCII digits (may be empty).
fn parse_digits_star(bytes: &[u8], i: &mut usize) -> u64 {
    let mut value: u64 = 0;
    while let Some(&c) = bytes.get(*i) {
        if c.is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add((c - b'0') as u64);
            *i += 1;
        } else {
            break;
        }
    }
    value
}

/// Core of the CSI-u pattern `^\x1b\[(\d+)(?::(\d*))?(?::(\d+))?(?:;(\d+))?(?::(\d+))?u$`.
///
/// The regex's greedy quantifiers with backtracking reduce to in-order greedy
/// matching with fail-through restore (each optional group starts with a
/// distinct prefix character, so no shorter match can enable a longer one).
#[derive(Debug, Default)]
struct CsiUParsed {
    codepoint: u64,
    /// The first `:` group matched (its digit value may be empty → shifted None)
    shifted_present: bool,
    shifted_value: u64,
    base: Option<u64>,
    mod_present: bool,
    mod_value: u64,
    event: Option<u64>,
}

fn match_csi_u(data: &str) -> Option<CsiUParsed> {
    let bytes = data.as_bytes();
    if bytes.len() < 4 || bytes[0] != 0x1b || bytes[1] != b'[' {
        return None;
    }
    if *bytes.last().expect("len checked") != b'u' {
        return None;
    }
    let mut i = 2usize;
    let codepoint = parse_digits(bytes, &mut i)?;
    let mut parsed = CsiUParsed {
        codepoint,
        ..Default::default()
    };
    // (?::(\d*))? — ':' followed by zero or more digits
    if bytes.get(i) == Some(&b':') {
        i += 1;
        parsed.shifted_present = true;
        parsed.shifted_value = parse_digits_star(bytes, &mut i);
    }
    // (?::(\d+))? — ':' followed by one or more digits
    if bytes.get(i) == Some(&b':') {
        let save = i;
        i += 1;
        match parse_digits(bytes, &mut i) {
            Some(value) => parsed.base = Some(value),
            None => i = save,
        }
    }
    // (?:;(\d+))? — ';' followed by one or more digits
    if bytes.get(i) == Some(&b';') {
        let save = i;
        i += 1;
        match parse_digits(bytes, &mut i) {
            Some(value) => {
                parsed.mod_present = true;
                parsed.mod_value = value;
            }
            None => i = save,
        }
    }
    // (?::(\d+))? — ':' followed by one or more digits
    if bytes.get(i) == Some(&b':') {
        let save = i;
        i += 1;
        match parse_digits(bytes, &mut i) {
            Some(value) => parsed.event = Some(value),
            None => i = save,
        }
    }
    if i != bytes.len() - 1 {
        return None;
    }
    Some(parsed)
}

fn parse_kitty_sequence(data: &str) -> Option<ParsedKittySequence> {
    // CSI u format with alternate keys (flag 4):
    // \x1b[<codepoint>u
    // \x1b[<codepoint>;<mod>u
    // \x1b[<codepoint>;<mod>:<event>u
    // \x1b[<codepoint>:<shifted>;<mod>u
    // \x1b[<codepoint>:<shifted>:<base>;<mod>u
    // \x1b[<codepoint>::<base>;<mod>u (no shifted key, only base)
    //
    // With flag 2, event type is appended after modifier colon: 1=press, 2=repeat, 3=release
    // With flag 4, alternate keys are appended after codepoint with colons
    if let Some(csi) = match_csi_u(data) {
        // JS: `csiUMatch[2] && csiUMatch[2].length > 0` — an empty digit run
        // (the `::base` form) yields undefined.
        let shifted_key = if csi.shifted_present && shifted_digits_nonempty(data) {
            Some(csi.shifted_value as i64)
        } else {
            None
        };
        let mod_value = if csi.mod_present { csi.mod_value as i32 } else { 1 };
        let event_type = parse_event_type(csi.event);
        store_last_event_type(event_type);
        return Some(ParsedKittySequence {
            codepoint: csi.codepoint as i64,
            shifted_key,
            base_layout_key: csi.base.map(|v| v as i64),
            modifier: mod_value.wrapping_sub(1),
            event_type,
        });
    }

    // Arrow keys with modifier: \x1b[1;<mod>A/B/C/D or \x1b[1;<mod>:<event>A/B/C/D
    if let Some((mod_value, event, letter)) = match_arrow_sequence(data) {
        let event_type = parse_event_type(event);
        let codepoint = match letter {
            b'A' => ARROW_CODEPOINT_UP,
            b'B' => ARROW_CODEPOINT_DOWN,
            b'C' => ARROW_CODEPOINT_RIGHT,
            b'D' => ARROW_CODEPOINT_LEFT,
            _ => return None,
        };
        store_last_event_type(event_type);
        return Some(ParsedKittySequence {
            codepoint,
            shifted_key: None,
            base_layout_key: None,
            modifier: (mod_value as i32).wrapping_sub(1),
            event_type,
        });
    }

    // Functional keys: \x1b[<num>~ or \x1b[<num>;<mod>~ or \x1b[<num>;<mod>:<event>~
    if let Some((key_num, mod_value, event)) = match_func_sequence(data) {
        let codepoint = match key_num {
            2 => Some(FUNCTIONAL_CODEPOINT_INSERT),
            3 => Some(FUNCTIONAL_CODEPOINT_DELETE),
            5 => Some(FUNCTIONAL_CODEPOINT_PAGE_UP),
            6 => Some(FUNCTIONAL_CODEPOINT_PAGE_DOWN),
            7 => Some(FUNCTIONAL_CODEPOINT_HOME),
            8 => Some(FUNCTIONAL_CODEPOINT_END),
            _ => None,
        };
        if let Some(codepoint) = codepoint {
            let event_type = parse_event_type(event);
            store_last_event_type(event_type);
            return Some(ParsedKittySequence {
                codepoint,
                shifted_key: None,
                base_layout_key: None,
                modifier: (mod_value as i32).wrapping_sub(1),
                event_type,
            });
        }
    }

    // Home/End with modifier: \x1b[1;<mod>H/F or \x1b[1;<mod>:<event>H/F
    if let Some((mod_value, event, letter)) = match_home_end_sequence(data) {
        let event_type = parse_event_type(event);
        let codepoint = if letter == b'H' { FUNCTIONAL_CODEPOINT_HOME } else { FUNCTIONAL_CODEPOINT_END };
        store_last_event_type(event_type);
        return Some(ParsedKittySequence {
            codepoint,
            shifted_key: None,
            base_layout_key: None,
            modifier: (mod_value as i32).wrapping_sub(1),
            event_type,
        });
    }

    None
}

/// Whether the shifted-key digit run in a CSI-u sequence is non-empty
/// (the `:` group matched with at least one digit).
fn shifted_digits_nonempty(data: &str) -> bool {
    let bytes = data.as_bytes();
    let mut i = 2usize;
    while let Some(&c) = bytes.get(i) {
        if c.is_ascii_digit() {
            i += 1;
        } else {
            break;
        }
    }
    if bytes.get(i) == Some(&b':') {
        i += 1;
        return bytes.get(i).is_some_and(|c| c.is_ascii_digit());
    }
    false
}

/// `^\x1b\[1;(\d+)(?::(\d+))?([ABCD])$`
fn match_arrow_sequence(data: &str) -> Option<(u64, Option<u64>, u8)> {
    let bytes = data.as_bytes();
    if bytes.len() < 6 || &bytes[..3] != b"\x1b[1" {
        return None;
    }
    if bytes[3] != b';' {
        return None;
    }
    let mut i = 4usize;
    let mod_value = parse_digits(bytes, &mut i)?;
    let mut event = None;
    if bytes.get(i) == Some(&b':') {
        let save = i;
        i += 1;
        match parse_digits(bytes, &mut i) {
            Some(value) => event = Some(value),
            None => i = save,
        }
    }
    let letter = *bytes.get(i)?;
    if !matches!(letter, b'A' | b'B' | b'C' | b'D') {
        return None;
    }
    if i + 1 != bytes.len() {
        return None;
    }
    Some((mod_value, event, letter))
}

/// `^\x1b\[(\d+)(?:;(\d+))?(?::(\d+))?~$`
fn match_func_sequence(data: &str) -> Option<(u64, u64, Option<u64>)> {
    let bytes = data.as_bytes();
    if bytes.len() < 4 || bytes[0] != 0x1b || bytes[1] != b'[' {
        return None;
    }
    if *bytes.last().expect("len checked") != b'~' {
        return None;
    }
    let mut i = 2usize;
    let key_num = parse_digits(bytes, &mut i)?;
    let mut mod_value = 1u64;
    if bytes.get(i) == Some(&b';') {
        let save = i;
        i += 1;
        match parse_digits(bytes, &mut i) {
            Some(value) => mod_value = value,
            None => i = save,
        }
    }
    let mut event = None;
    if bytes.get(i) == Some(&b':') {
        let save = i;
        i += 1;
        match parse_digits(bytes, &mut i) {
            Some(value) => event = Some(value),
            None => i = save,
        }
    }
    if i != bytes.len() - 1 {
        return None;
    }
    Some((key_num, mod_value, event))
}

/// `^\x1b\[1;(\d+)(?::(\d+))?([HF])$`
fn match_home_end_sequence(data: &str) -> Option<(u64, Option<u64>, u8)> {
    let bytes = data.as_bytes();
    if bytes.len() < 6 || &bytes[..3] != b"\x1b[1" {
        return None;
    }
    if bytes[3] != b';' {
        return None;
    }
    let mut i = 4usize;
    let mod_value = parse_digits(bytes, &mut i)?;
    let mut event = None;
    if bytes.get(i) == Some(&b':') {
        let save = i;
        i += 1;
        match parse_digits(bytes, &mut i) {
            Some(value) => event = Some(value),
            None => i = save,
        }
    }
    let letter = *bytes.get(i)?;
    if !matches!(letter, b'H' | b'F') {
        return None;
    }
    if i + 1 != bytes.len() {
        return None;
    }
    Some((mod_value, event, letter))
}

fn matches_kitty_sequence(data: &str, expected_codepoint: i64, expected_modifier: i32) -> bool {
    let Some(parsed) = parse_kitty_sequence(data) else {
        return false;
    };
    let actual_mod = parsed.modifier & !LOCK_MASK;
    let expected_mod = expected_modifier & !LOCK_MASK;

    let normalized_codepoint = normalize_shifted_letter_identity_codepoint(
        normalize_kitty_functional_codepoint(parsed.codepoint),
        parsed.modifier,
    );
    let normalized_expected_codepoint = normalize_shifted_letter_identity_codepoint(
        normalize_kitty_functional_codepoint(expected_codepoint),
        expected_modifier,
    );

    // Physical match: primary codepoint and every reported modifier match.
    if actual_mod == expected_mod && normalized_codepoint == normalized_expected_codepoint {
        return true;
    }

    // Logical match: Kitty reports symbols such as "+" as Shift plus the
    // physical "=" key. Bindings name the produced symbol, so consume that
    // implicit Shift while preserving Ctrl, Alt, and Super. The physical
    // "shift+=" spelling remains valid through the match above.
    if let Some((codepoint, modifier)) =
        get_logical_shifted_symbol_candidate(parsed.shifted_key, actual_mod, SymbolSet::Symbols)
    {
        if (modifier & !LOCK_MASK) == expected_mod
            && codepoint == normalize_kitty_functional_codepoint(expected_codepoint)
        {
            return true;
        }
    }

    if actual_mod != expected_mod {
        return false;
    }

    // Alternate match: use base layout key for non-Latin keyboard layouts.
    // This allows Ctrl+С (Cyrillic) to match Ctrl+c (Latin) when terminal
    // reports the base layout key (the key in standard PC-101 layout).
    //
    // Only fall back to base layout key when the codepoint is NOT already a
    // recognized Latin letter (a-z) or symbol (e.g., /, -, [, ;, etc.).
    // When the codepoint is a recognized key, it is authoritative regardless
    // of physical key position. This prevents remapped layouts (Dvorak,
    // Colemak, xremap, etc.) from causing false matches: both letters and
    // symbols move to different physical positions, so Ctrl+K could falsely
    // match Ctrl+V (letter remapping) and Ctrl+/ could falsely match Ctrl+[
    // (symbol remapping) if the base layout key were always considered.
    if parsed.base_layout_key.is_some_and(|base| base == expected_codepoint) {
        let cp = normalized_codepoint;
        let is_latin_letter = (97..=122).contains(&cp); // a-z
        let is_known_symbol = is_symbol_key(js_from_char_code(cp));
        if !is_latin_letter && !is_known_symbol {
            return true;
        }
    }

    false
}

fn parse_modify_other_keys_sequence(data: &str) -> Option<ParsedModifyOtherKeysSequence> {
    // ^\x1b\[27;(\d+);(\d+)~$
    let bytes = data.as_bytes();
    if bytes.len() < 8 || &bytes[..4] != b"\x1b[27" {
        return None;
    }
    if bytes[4] != b';' {
        return None;
    }
    let mut i = 5usize;
    let mod_value = parse_digits(bytes, &mut i)?;
    if bytes.get(i) != Some(&b';') {
        return None;
    }
    i += 1;
    let codepoint = parse_digits(bytes, &mut i)?;
    if i != bytes.len() - 1 || *bytes.last().expect("len checked") != b'~' {
        return None;
    }
    Some(ParsedModifyOtherKeysSequence {
        codepoint: codepoint as i64,
        modifier: (mod_value as i32).wrapping_sub(1),
    })
}

/// Match xterm modifyOtherKeys format: CSI 27 ; modifiers ; keycode ~
/// This is used by terminals when Kitty protocol is not enabled.
/// Modifier values are 1-indexed: 2=shift, 3=alt, 5=ctrl, etc.
fn matches_modify_other_keys(data: &str, expected_keycode: i64, expected_modifier: i32) -> bool {
    let Some(parsed) = parse_modify_other_keys_sequence(data) else {
        return false;
    };
    parsed.codepoint == expected_keycode && parsed.modifier == expected_modifier
}

/// Whether this session is a local Windows Terminal session
/// (`WT_SESSION` set, not over SSH).
pub fn is_windows_terminal_session() -> bool {
    is_windows_terminal_session_with(|name| env_get(name))
}

/// Injectable form of [`is_windows_terminal_session`] (mirrors the env reads
/// of the TS original: `Boolean(WT_SESSION)` — a non-empty value — and no
/// `SSH_CONNECTION`/`SSH_CLIENT`/`SSH_TTY` present).
pub fn is_windows_terminal_session_with(getenv: impl Fn(&str) -> Option<String>) -> bool {
    let wt_session = getenv("WT_SESSION");
    let wt_session_nonempty = wt_session.as_deref().is_some_and(|v| !v.is_empty());
    wt_session_nonempty
        && getenv("SSH_CONNECTION").is_none()
        && getenv("SSH_CLIENT").is_none()
        && getenv("SSH_TTY").is_none()
}

/// Raw 0x08 (BS) is ambiguous in legacy terminals.
///
/// - Windows Terminal uses it for Ctrl+Backspace.
/// - Some legacy terminals and tmux setups send it for plain Backspace.
///
/// Prefer explicit Kitty / CSI-u / modifyOtherKeys sequences whenever they are
/// available. Fall back to a Windows Terminal heuristic only for raw BS bytes.
fn matches_raw_backspace(data: &str, expected_modifier: i32) -> bool {
    if data == "\x7f" {
        return expected_modifier == 0;
    }
    if data != "\x08" {
        return false;
    }
    if is_windows_terminal_session() {
        expected_modifier == MODIFIERS_CTRL
    } else {
        expected_modifier == 0
    }
}

// =============================================================================
// Generic Key Matching
// =============================================================================

/// Get the control character for a key.
/// Uses the universal formula: code & 0x1f (mask to lower 5 bits)
///
/// Works for:
/// - Letters a-z → 1-26
/// - Symbols [\]_ → 27, 28, 29, 31
/// - Also maps - to same as _ (same physical key on US keyboards)
fn raw_ctrl_char(key: char) -> Option<char> {
    let char = key.to_ascii_lowercase();
    let code = char as u32;
    if (97..=122).contains(&code) || char == '[' || char == '\\' || char == ']' || char == '_' {
        return Some((code & 0x1f) as u8 as char);
    }
    // Handle - as _ (same physical key on US keyboards)
    if char == '-' {
        return Some('\x1f'); // Same as Ctrl+_
    }
    None
}

fn is_digit_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    bytes.len() == 1 && bytes[0].is_ascii_digit()
}

fn matches_printable_modify_other_keys(data: &str, expected_keycode: i64, expected_modifier: i32) -> bool {
    let Some(parsed) = parse_modify_other_keys_sequence(data) else {
        return false;
    };
    let physical_match = parsed.modifier == expected_modifier
        && normalize_shifted_letter_identity_codepoint(parsed.codepoint, parsed.modifier)
            == normalize_shifted_letter_identity_codepoint(expected_keycode, expected_modifier);
    if physical_match {
        return true;
    }
    match get_logical_shifted_symbol_candidate(Some(parsed.codepoint), parsed.modifier, SymbolSet::ShiftedSymbols) {
        Some((codepoint, modifier)) => modifier == expected_modifier && codepoint == expected_keycode,
        None => false,
    }
}

fn format_key_name_with_modifiers(key_name: &str, modifier: i32) -> Option<String> {
    let mut mods: Vec<&str> = Vec::new();
    let effective_mod = modifier & !LOCK_MASK;
    let supported_modifier_mask = MODIFIERS_SHIFT | MODIFIERS_CTRL | MODIFIERS_ALT | MODIFIERS_SUPER;
    if (effective_mod & !supported_modifier_mask) != 0 {
        return None;
    }
    if (effective_mod & MODIFIERS_SHIFT) != 0 {
        mods.push("shift");
    }
    if (effective_mod & MODIFIERS_CTRL) != 0 {
        mods.push("ctrl");
    }
    if (effective_mod & MODIFIERS_ALT) != 0 {
        mods.push("alt");
    }
    if (effective_mod & MODIFIERS_SUPER) != 0 {
        mods.push("super");
    }
    if mods.is_empty() {
        Some(key_name.to_string())
    } else {
        Some(format!("{}+{}", mods.join("+"), key_name))
    }
}

struct ParsedKeyId {
    key: String,
    ctrl: bool,
    shift: bool,
    alt: bool,
    super_modifier: bool,
}

fn parse_key_id(key_id: &str) -> Option<ParsedKeyId> {
    let mut key = key_id.to_lowercase();
    let mut shift = false;
    let mut alt = false;
    let mut ctrl = false;
    let mut super_modifier = false;
    loop {
        let modifier = MODIFIER_NAMES.iter().find(|name| key.starts_with(&format!("{}+", name)));
        let Some(modifier) = modifier else { break };
        let already = match *modifier {
            "shift" => shift,
            "alt" => alt,
            "ctrl" => ctrl,
            "super" => super_modifier,
            _ => unreachable!(),
        };
        if already {
            return None;
        }
        match *modifier {
            "shift" => shift = true,
            "alt" => alt = true,
            "ctrl" => ctrl = true,
            "super" => super_modifier = true,
            _ => unreachable!(),
        }
        key = key[modifier.len() + 1..].to_string();
    }
    if key.is_empty() || (key.contains('+') && key != "+") {
        return None;
    }
    Some(ParsedKeyId { key, ctrl, shift, alt, super_modifier })
}

/// Match input data against a key identifier string.
///
/// Supported key identifiers:
/// - Single keys: "escape", "tab", "enter", "backspace", "delete", "home", "end", "space"
/// - Arrow keys: "up", "down", "left", "right"
/// - Ctrl combinations: "ctrl+c", "ctrl+z", etc.
/// - Shift combinations: "shift+tab", "shift+enter"
/// - Alt combinations: "alt+enter", "alt+backspace"
/// - Super combinations: "super+k", "super+enter"
/// - Combined modifiers: "shift+ctrl+p", "ctrl+alt+x", "ctrl+super+k"
pub fn matches_key(data: &str, key_id: &str) -> bool {
    let Some(parsed) = parse_key_id(key_id) else {
        return false;
    };

    let ParsedKeyId { key, ctrl, shift, alt, super_modifier } = parsed;
    let mut modifier = 0;
    if shift {
        modifier |= MODIFIERS_SHIFT;
    }
    if alt {
        modifier |= MODIFIERS_ALT;
    }
    if ctrl {
        modifier |= MODIFIERS_CTRL;
    }
    if super_modifier {
        modifier |= MODIFIERS_SUPER;
    }

    let kitty = is_kitty_protocol_active();

    let special_result: Option<bool> = match key.as_str() {
        "escape" | "esc" => {
            if modifier != 0 {
                Some(false)
            } else {
                Some(
                    data == "\x1b"
                        || matches_kitty_sequence(data, CODEPOINT_ESCAPE, 0)
                        || matches_modify_other_keys(data, CODEPOINT_ESCAPE, 0),
                )
            }
        }
        "space" => {
            if !kitty {
                if modifier == MODIFIERS_CTRL && data == "\x00" {
                    return true;
                }
                if modifier == MODIFIERS_ALT && data == "\x1b " {
                    return true;
                }
            }
            if modifier == 0 {
                Some(
                    data == " "
                        || matches_kitty_sequence(data, CODEPOINT_SPACE, 0)
                        || matches_modify_other_keys(data, CODEPOINT_SPACE, 0),
                )
            } else {
                Some(
                    matches_kitty_sequence(data, CODEPOINT_SPACE, modifier)
                        || matches_modify_other_keys(data, CODEPOINT_SPACE, modifier),
                )
            }
        }
        "tab" => {
            if modifier == MODIFIERS_SHIFT {
                Some(
                    data == "\x1b[Z"
                        || matches_kitty_sequence(data, CODEPOINT_TAB, MODIFIERS_SHIFT)
                        || matches_modify_other_keys(data, CODEPOINT_TAB, MODIFIERS_SHIFT),
                )
            } else if modifier == 0 {
                Some(data == "\t" || matches_kitty_sequence(data, CODEPOINT_TAB, 0))
            } else {
                Some(
                    matches_kitty_sequence(data, CODEPOINT_TAB, modifier)
                        || matches_modify_other_keys(data, CODEPOINT_TAB, modifier),
                )
            }
        }
        "enter" | "return" => Some(match_enter(data, modifier, kitty)),
        "backspace" => Some(match_backspace(data, modifier)),
        "insert" => Some(match_functional(data, "insert", FUNCTIONAL_CODEPOINT_INSERT, modifier)),
        "delete" => Some(match_functional(data, "delete", FUNCTIONAL_CODEPOINT_DELETE, modifier)),
        "clear" => {
            if modifier == 0 {
                Some(matches_legacy_sequence(data, legacy_key_sequences("clear").unwrap_or(&[])))
            } else {
                Some(matches_legacy_modifier_sequence(data, "clear", modifier))
            }
        }
        "home" => Some(match_functional(data, "home", FUNCTIONAL_CODEPOINT_HOME, modifier)),
        "end" => Some(match_functional(data, "end", FUNCTIONAL_CODEPOINT_END, modifier)),
        "pageup" => Some(match_functional(data, "pageUp", FUNCTIONAL_CODEPOINT_PAGE_UP, modifier)),
        "pagedown" => Some(match_functional(data, "pageDown", FUNCTIONAL_CODEPOINT_PAGE_DOWN, modifier)),
        "up" => Some(match_arrow(data, "up", ARROW_CODEPOINT_UP, modifier, kitty, "\x1bp")),
        "down" => Some(match_arrow(data, "down", ARROW_CODEPOINT_DOWN, modifier, kitty, "\x1bn")),
        "left" => Some(match_arrow_horizontal(data, "left", ARROW_CODEPOINT_LEFT, modifier, kitty, b'D', 'B', "\x1bb")),
        "right" => Some(match_arrow_horizontal(data, "right", ARROW_CODEPOINT_RIGHT, modifier, kitty, b'C', 'F', "\x1bf")),
        "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12" => {
            if modifier != 0 {
                Some(false)
            } else {
                Some(matches_legacy_sequence(data, legacy_key_sequences(&key).unwrap_or(&[])))
            }
        }
        _ => None,
    };
    if let Some(result) = special_result {
        return result;
    }

    // Handle single letter/digit keys and symbols
    let mut key_chars = key.chars();
    let single = key_chars.next();
    if single.is_some() && key_chars.next().is_none() {
        if let Some(key_char) = single {
            let is_letter = key_char.is_ascii_lowercase();
            let is_digit = is_digit_key(&key);
            if is_letter || is_digit || is_symbol_key(key_char) {
                return match_printable_key(data, key_char, &key, modifier, kitty);
            }
        }
    }

    false
}

fn match_enter(data: &str, modifier: i32, kitty: bool) -> bool {
    if modifier == MODIFIERS_SHIFT {
        // CSI u sequences (standard Kitty protocol)
        if matches_kitty_sequence(data, CODEPOINT_ENTER, MODIFIERS_SHIFT)
            || matches_kitty_sequence(data, CODEPOINT_KP_ENTER, MODIFIERS_SHIFT)
        {
            return true;
        }
        // xterm modifyOtherKeys format (fallback when Kitty protocol not enabled)
        if matches_modify_other_keys(data, CODEPOINT_ENTER, MODIFIERS_SHIFT) {
            return true;
        }
        // When Kitty protocol is active, legacy sequences are custom terminal mappings
        // \x1b\r = Kitty's "map shift+enter send_text all \e\r"
        // \n = Ghostty's "keybind = shift+enter=text:\n"
        if kitty {
            return data == "\x1b\r" || data == "\n";
        }
        return false;
    }
    if modifier == MODIFIERS_ALT {
        // CSI u sequences (standard Kitty protocol)
        if matches_kitty_sequence(data, CODEPOINT_ENTER, MODIFIERS_ALT)
            || matches_kitty_sequence(data, CODEPOINT_KP_ENTER, MODIFIERS_ALT)
        {
            return true;
        }
        // xterm modifyOtherKeys format (fallback when Kitty protocol not enabled)
        if matches_modify_other_keys(data, CODEPOINT_ENTER, MODIFIERS_ALT) {
            return true;
        }
        // \x1b\r is alt+enter only in legacy mode (no Kitty protocol)
        // When Kitty protocol is active, alt+enter comes as CSI u sequence
        if !kitty {
            return data == "\x1b\r";
        }
        return false;
    }
    if modifier == 0 {
        return data == "\r"
            || (!kitty && data == "\n")
            || data == "\x1bOM" // SS3 M (numpad enter in some terminals)
            || matches_kitty_sequence(data, CODEPOINT_ENTER, 0)
            || matches_kitty_sequence(data, CODEPOINT_KP_ENTER, 0);
    }
    matches_kitty_sequence(data, CODEPOINT_ENTER, modifier)
        || matches_kitty_sequence(data, CODEPOINT_KP_ENTER, modifier)
        || matches_modify_other_keys(data, CODEPOINT_ENTER, modifier)
}

fn match_backspace(data: &str, modifier: i32) -> bool {
    if modifier == MODIFIERS_ALT {
        if data == "\x1b\x7f" || data == "\x1b\x08" {
            return true;
        }
        return matches_kitty_sequence(data, CODEPOINT_BACKSPACE, MODIFIERS_ALT)
            || matches_modify_other_keys(data, CODEPOINT_BACKSPACE, MODIFIERS_ALT);
    }
    if modifier == MODIFIERS_CTRL {
        // Legacy raw 0x08 is ambiguous: it can be Ctrl+Backspace on Windows
        // Terminal or plain Backspace on other terminals, while also
        // overlapping with Ctrl+H.
        if matches_raw_backspace(data, MODIFIERS_CTRL) {
            return true;
        }
        return matches_kitty_sequence(data, CODEPOINT_BACKSPACE, MODIFIERS_CTRL)
            || matches_modify_other_keys(data, CODEPOINT_BACKSPACE, MODIFIERS_CTRL);
    }
    if modifier == 0 {
        return matches_raw_backspace(data, 0)
            || matches_kitty_sequence(data, CODEPOINT_BACKSPACE, 0)
            || matches_modify_other_keys(data, CODEPOINT_BACKSPACE, 0);
    }
    matches_kitty_sequence(data, CODEPOINT_BACKSPACE, modifier)
        || matches_modify_other_keys(data, CODEPOINT_BACKSPACE, modifier)
}

fn match_functional(data: &str, legacy_key: &str, codepoint: i64, modifier: i32) -> bool {
    if modifier == 0 {
        return matches_legacy_sequence(data, legacy_key_sequences(legacy_key).unwrap_or(&[]))
            || matches_kitty_sequence(data, codepoint, 0);
    }
    if matches_legacy_modifier_sequence(data, legacy_key, modifier) {
        return true;
    }
    matches_kitty_sequence(data, codepoint, modifier)
}

fn match_arrow(data: &str, legacy_key: &str, codepoint: i64, modifier: i32, kitty: bool, alt_sequence: &str) -> bool {
    let _ = kitty;
    if modifier == MODIFIERS_ALT {
        return data == alt_sequence || matches_kitty_sequence(data, codepoint, MODIFIERS_ALT);
    }
    if modifier == 0 {
        return matches_legacy_sequence(data, legacy_key_sequences(legacy_key).unwrap_or(&[]))
            || matches_kitty_sequence(data, codepoint, 0);
    }
    if matches_legacy_modifier_sequence(data, legacy_key, modifier) {
        return true;
    }
    matches_kitty_sequence(data, codepoint, modifier)
}

#[allow(clippy::too_many_arguments)]
fn match_arrow_horizontal(
    data: &str,
    legacy_key: &str,
    codepoint: i64,
    modifier: i32,
    kitty: bool,
    csi_letter: u8,
    legacy_meta_char: char,
    meta_sequence: &str,
) -> bool {
    let _ = (csi_letter, legacy_meta_char);
    if modifier == MODIFIERS_ALT {
        return data == format!("\x1b[1;3{}", csi_letter as char)
            || (!kitty && data == format!("\x1b{}", legacy_meta_char))
            || data == meta_sequence
            || matches_kitty_sequence(data, codepoint, MODIFIERS_ALT);
    }
    if modifier == MODIFIERS_CTRL {
        return data == format!("\x1b[1;5{}", csi_letter as char)
            || matches_legacy_modifier_sequence(data, legacy_key, MODIFIERS_CTRL)
            || matches_kitty_sequence(data, codepoint, MODIFIERS_CTRL);
    }
    if modifier == 0 {
        return matches_legacy_sequence(data, legacy_key_sequences(legacy_key).unwrap_or(&[]))
            || matches_kitty_sequence(data, codepoint, 0);
    }
    if matches_legacy_modifier_sequence(data, legacy_key, modifier) {
        return true;
    }
    matches_kitty_sequence(data, codepoint, modifier)
}

fn match_printable_key(data: &str, key_char: char, key: &str, modifier: i32, kitty: bool) -> bool {
    let codepoint = key_char as i64;
    let raw_ctrl = raw_ctrl_char(key_char);
    let is_letter = key_char.is_ascii_lowercase();
    let is_digit = is_digit_key(key);

    if modifier == MODIFIERS_CTRL + MODIFIERS_ALT && !kitty && raw_ctrl.is_some() {
        // Legacy: ctrl+alt+key is ESC followed by the control character.
        // If that legacy form does not match, continue so CSI-u and
        // modifyOtherKeys sequences from tmux can still be recognized.
        if data == format!("\x1b{}", raw_ctrl.expect("checked")) {
            return true;
        }
    }

    if modifier == MODIFIERS_ALT && !kitty && (is_letter || is_digit || is_symbol_key(key_char)) {
        // Legacy: alt+printable key is ESC followed by the key
        if data == format!("\x1b{}", key) {
            return true;
        }
    }

    if modifier == MODIFIERS_CTRL {
        // Legacy: ctrl+key sends the control character
        if let Some(raw_ctrl) = raw_ctrl {
            if data == raw_ctrl.to_string() {
                return true;
            }
        }
        return matches_kitty_sequence(data, codepoint, MODIFIERS_CTRL)
            || matches_printable_modify_other_keys(data, codepoint, MODIFIERS_CTRL);
    }

    if modifier == MODIFIERS_SHIFT + MODIFIERS_CTRL {
        return matches_kitty_sequence(data, codepoint, MODIFIERS_SHIFT + MODIFIERS_CTRL)
            || matches_printable_modify_other_keys(data, codepoint, MODIFIERS_SHIFT + MODIFIERS_CTRL);
    }

    if modifier == MODIFIERS_SHIFT {
        // Legacy: shift+letter produces uppercase
        if is_letter && data == key.to_uppercase() {
            return true;
        }
        return matches_kitty_sequence(data, codepoint, MODIFIERS_SHIFT)
            || matches_printable_modify_other_keys(data, codepoint, MODIFIERS_SHIFT);
    }

    if modifier != 0 {
        return matches_kitty_sequence(data, codepoint, modifier)
            || matches_printable_modify_other_keys(data, codepoint, modifier);
    }

    // Check both raw char and Kitty sequence (needed for release events)
    data == key
        || matches_kitty_sequence(data, codepoint, 0)
        || matches_printable_modify_other_keys(data, codepoint, 0)
}

// =============================================================================
// parseKey
// =============================================================================

fn format_parsed_key(codepoint: i64, modifier: i32, base_layout_key: Option<i64>) -> Option<String> {
    let normalized_codepoint = normalize_kitty_functional_codepoint(codepoint);
    let identity_codepoint = normalize_shifted_letter_identity_codepoint(normalized_codepoint, modifier);

    // Use base layout key only when codepoint is not a recognized Latin
    // letter (a-z), digit (0-9), or symbol (/, -, [, ;, etc.). For those,
    // the codepoint is authoritative regardless of physical key position.
    // This prevents remapped layouts (Dvorak, Colemak, xremap, etc.) from
    // reporting the wrong key name based on the QWERTY physical position.
    let is_latin_letter = (97..=122).contains(&identity_codepoint); // a-z
    let is_digit = (48..=57).contains(&identity_codepoint); // 0-9
    let is_known_symbol = is_symbol_key(js_from_char_code(identity_codepoint));
    let effective_codepoint = if is_latin_letter || is_digit || is_known_symbol {
        identity_codepoint
    } else {
        base_layout_key.unwrap_or(identity_codepoint)
    };

    let key_name: Option<String> = if effective_codepoint == CODEPOINT_ESCAPE {
        Some("escape".to_string())
    } else if effective_codepoint == CODEPOINT_TAB {
        Some("tab".to_string())
    } else if effective_codepoint == CODEPOINT_ENTER || effective_codepoint == CODEPOINT_KP_ENTER {
        Some("enter".to_string())
    } else if effective_codepoint == CODEPOINT_SPACE {
        Some("space".to_string())
    } else if effective_codepoint == CODEPOINT_BACKSPACE {
        Some("backspace".to_string())
    } else if effective_codepoint == FUNCTIONAL_CODEPOINT_DELETE {
        Some("delete".to_string())
    } else if effective_codepoint == FUNCTIONAL_CODEPOINT_INSERT {
        Some("insert".to_string())
    } else if effective_codepoint == FUNCTIONAL_CODEPOINT_HOME {
        Some("home".to_string())
    } else if effective_codepoint == FUNCTIONAL_CODEPOINT_END {
        Some("end".to_string())
    } else if effective_codepoint == FUNCTIONAL_CODEPOINT_PAGE_UP {
        Some("pageUp".to_string())
    } else if effective_codepoint == FUNCTIONAL_CODEPOINT_PAGE_DOWN {
        Some("pageDown".to_string())
    } else if effective_codepoint == ARROW_CODEPOINT_UP {
        Some("up".to_string())
    } else if effective_codepoint == ARROW_CODEPOINT_DOWN {
        Some("down".to_string())
    } else if effective_codepoint == ARROW_CODEPOINT_LEFT {
        Some("left".to_string())
    } else if effective_codepoint == ARROW_CODEPOINT_RIGHT {
        Some("right".to_string())
    } else if (48..=57).contains(&effective_codepoint) {
        Some(js_from_char_code(effective_codepoint).to_string())
    } else if (97..=122).contains(&effective_codepoint) {
        Some(js_from_char_code(effective_codepoint).to_string())
    } else if is_symbol_key(js_from_char_code(effective_codepoint)) {
        Some(js_from_char_code(effective_codepoint).to_string())
    } else {
        None
    };

    let key_name = key_name?;
    format_key_name_with_modifiers(&key_name, modifier)
}

/// Parse input data and return the key identifier if recognized.
pub fn parse_key(data: &str) -> Option<String> {
    let kitty = is_kitty_protocol_active();

    if let Some(kitty_parsed) = parse_kitty_sequence(data) {
        let logical_shifted_symbol =
            get_logical_shifted_symbol_candidate(kitty_parsed.shifted_key, kitty_parsed.modifier, SymbolSet::Symbols);
        let (codepoint, modifier) = match logical_shifted_symbol {
            Some((codepoint, modifier)) => (codepoint, modifier),
            None => (kitty_parsed.codepoint, kitty_parsed.modifier),
        };
        return format_parsed_key(codepoint, modifier, kitty_parsed.base_layout_key);
    }

    if let Some(modify_other_keys) = parse_modify_other_keys_sequence(data) {
        let logical_shifted_symbol = get_logical_shifted_symbol_candidate(
            Some(modify_other_keys.codepoint),
            modify_other_keys.modifier,
            SymbolSet::ShiftedSymbols,
        );
        let (codepoint, modifier) = match logical_shifted_symbol {
            Some((codepoint, modifier)) => (codepoint, modifier),
            None => (modify_other_keys.codepoint, modify_other_keys.modifier),
        };
        return format_parsed_key(codepoint, modifier, None);
    }

    // Mode-aware legacy sequences
    // When Kitty protocol is active, ambiguous sequences are interpreted as custom terminal mappings:
    // - \x1b\r = shift+enter (Kitty mapping), not alt+enter
    // - \n = shift+enter (Ghostty mapping)
    if kitty && (data == "\x1b\r" || data == "\n") {
        return Some("shift+enter".to_string());
    }

    if let Some(legacy_sequence_key_id) = legacy_sequence_key_id(data) {
        return Some(legacy_sequence_key_id.to_string());
    }

    // Legacy sequences (used when Kitty protocol is not active, or for unambiguous sequences)
    if data == "\x1b" {
        return Some("escape".to_string());
    }
    if data == "\x1c" {
        return Some("ctrl+\\".to_string());
    }
    if data == "\x1d" {
        return Some("ctrl+]".to_string());
    }
    if data == "\x1f" {
        return Some("ctrl+-".to_string());
    }
    if data == "\x1b\x1b" {
        return Some("ctrl+alt+[".to_string());
    }
    if data == "\x1b\x1c" {
        return Some("ctrl+alt+\\".to_string());
    }
    if data == "\x1b\x1d" {
        return Some("ctrl+alt+]".to_string());
    }
    if data == "\x1b\x1f" {
        return Some("ctrl+alt+-".to_string());
    }
    if data == "\t" {
        return Some("tab".to_string());
    }
    if data == "\r" || (!kitty && data == "\n") || data == "\x1bOM" {
        return Some("enter".to_string());
    }
    if data == "\x00" {
        return Some("ctrl+space".to_string());
    }
    if data == " " {
        return Some("space".to_string());
    }
    if data == "\x7f" {
        return Some("backspace".to_string());
    }
    if data == "\x08" {
        return Some(if is_windows_terminal_session() { "ctrl+backspace" } else { "backspace" }.to_string());
    }
    if data == "\x1b[Z" {
        return Some("shift+tab".to_string());
    }
    if !kitty && data == "\x1b\r" {
        return Some("alt+enter".to_string());
    }
    if !kitty && data == "\x1b " {
        return Some("alt+space".to_string());
    }
    if data == "\x1b\x7f" || data == "\x1b\x08" {
        return Some("alt+backspace".to_string());
    }
    if !kitty && data == "\x1bB" {
        return Some("alt+left".to_string());
    }
    if !kitty && data == "\x1bF" {
        return Some("alt+right".to_string());
    }
    if !kitty && utf16_len(data) == 2 && data.starts_with('\x1b') {
        let code = data.encode_utf16().nth(1).expect("utf16_len == 2") as i64;
        if (1..=26).contains(&code) {
            return Some(format!("ctrl+alt+{}", js_from_char_code(code + 96)));
        }
        // Legacy alt+letter/digit/symbol (ESC followed by the key)
        let key = js_from_char_code(code);
        if (97..=122).contains(&code) || (48..=57).contains(&code) || is_symbol_key(key) {
            return Some(format!("alt+{}", key));
        }
    }
    if data == "\x1b[A" {
        return Some("up".to_string());
    }
    if data == "\x1b[B" {
        return Some("down".to_string());
    }
    if data == "\x1b[C" {
        return Some("right".to_string());
    }
    if data == "\x1b[D" {
        return Some("left".to_string());
    }
    if data == "\x1b[H" || data == "\x1bOH" {
        return Some("home".to_string());
    }
    if data == "\x1b[F" || data == "\x1bOF" {
        return Some("end".to_string());
    }
    if data == "\x1b[3~" {
        return Some("delete".to_string());
    }
    if data == "\x1b[5~" {
        return Some("pageUp".to_string());
    }
    if data == "\x1b[6~" {
        return Some("pageDown".to_string());
    }

    // Raw Ctrl+letter
    if utf16_len(data) == 1 {
        let code = data.encode_utf16().next().expect("utf16_len == 1") as i64;
        if (1..=26).contains(&code) {
            return Some(format!("ctrl+{}", js_from_char_code(code + 96)));
        }
        if (32..=126).contains(&code) {
            return Some(js_from_char_code(code).to_string());
        }
    }

    None
}

// =============================================================================
// Kitty CSI-u Printable Decoding
// =============================================================================

const KITTY_PRINTABLE_ALLOWED_MODIFIERS: i32 = MODIFIERS_SHIFT | LOCK_MASK;

/// `String.fromCodePoint` semantics — throws (→ None) for values above
/// 0x10FFFF and for surrogate code points.
fn js_from_code_point(codepoint: i64) -> Option<char> {
    if !(0..=0x10FFFF).contains(&codepoint) {
        return None;
    }
    let c = char::from_u32(codepoint as u32)?;
    // Rust's char type cannot represent lone surrogates; the TS original
    // throws a RangeError for them, so reject.
    let _ = c;
    if (0xD800..=0xDFFF).contains(&codepoint) {
        return None;
    }
    char::from_u32(codepoint as u32)
}

/// Decode a Kitty CSI-u sequence into a printable character, if applicable.
///
/// When Kitty keyboard protocol flag 1 (disambiguate) is active, terminals
/// send CSI-u sequences for all keys, including plain printable characters.
/// This function extracts the printable character from such sequences.
///
/// Only accepts plain or Shift-modified keys. Rejects Ctrl, Alt, and
/// unsupported modifier combinations (those are handled by keybinding
/// matching instead). Prefers the shifted keycode when Shift is held and a
/// shifted key is reported.
pub fn decode_kitty_printable(data: &str) -> Option<String> {
    let csi = match_csi_u(data)?;
    let codepoint = csi.codepoint as i64;
    let shifted_key = if csi.shifted_present && shifted_digits_nonempty(data) {
        Some(csi.shifted_value as i64)
    } else {
        None
    };
    let mod_value = if csi.mod_present { csi.mod_value as i32 } else { 1 };
    // Modifiers are 1-indexed in CSI-u; normalize to our bitmask.
    let modifier = mod_value.wrapping_sub(1);

    // Only accept printable CSI-u input for plain or Shift-modified text keys.
    // Reject unsupported modifier bits (e.g. Super/Meta) to avoid inserting
    // characters from modifier-only terminal events.
    if (modifier & !KITTY_PRINTABLE_ALLOWED_MODIFIERS) != 0 {
        return None;
    }
    if (modifier & (MODIFIERS_ALT | MODIFIERS_CTRL)) != 0 {
        return None;
    }

    // Prefer the shifted keycode when Shift is held.
    let mut effective_codepoint = codepoint;
    if (modifier & MODIFIERS_SHIFT) != 0 {
        if let Some(shifted_key) = shifted_key {
            effective_codepoint = shifted_key;
        }
    }
    effective_codepoint = normalize_kitty_functional_codepoint(effective_codepoint);
    // Drop control characters or invalid codepoints.
    if effective_codepoint < 32 {
        return None;
    }

    js_from_code_point(effective_codepoint).map(|c| c.to_string())
}

fn decode_modify_other_keys_printable(data: &str) -> Option<String> {
    let parsed = parse_modify_other_keys_sequence(data)?;
    let modifier = parsed.modifier & !LOCK_MASK;
    if (modifier & !MODIFIERS_SHIFT) != 0 {
        return None;
    }
    if parsed.codepoint < 32 {
        return None;
    }
    js_from_code_point(parsed.codepoint).map(|c| c.to_string())
}

pub fn decode_printable_key(data: &str) -> Option<String> {
    decode_kitty_printable(data).or_else(|| decode_modify_other_keys_printable(data))
}

// =============================================================================
// Key Helper
// =============================================================================

/// Helper for creating typed key identifiers (Rust port of the TS `Key`
/// object; runtime values are the same strings).
pub struct Key;

impl Key {
    // Special keys
    pub const ESCAPE: &'static str = "escape";
    pub const ESC: &'static str = "esc";
    pub const ENTER: &'static str = "enter";
    pub const RETURN: &'static str = "return";
    pub const TAB: &'static str = "tab";
    pub const SPACE: &'static str = "space";
    pub const BACKSPACE: &'static str = "backspace";
    pub const DELETE: &'static str = "delete";
    pub const INSERT: &'static str = "insert";
    pub const CLEAR: &'static str = "clear";
    pub const HOME: &'static str = "home";
    pub const END: &'static str = "end";
    pub const PAGE_UP: &'static str = "pageUp";
    pub const PAGE_DOWN: &'static str = "pageDown";
    pub const UP: &'static str = "up";
    pub const DOWN: &'static str = "down";
    pub const LEFT: &'static str = "left";
    pub const RIGHT: &'static str = "right";
    pub const F1: &'static str = "f1";
    pub const F2: &'static str = "f2";
    pub const F3: &'static str = "f3";
    pub const F4: &'static str = "f4";
    pub const F5: &'static str = "f5";
    pub const F6: &'static str = "f6";
    pub const F7: &'static str = "f7";
    pub const F8: &'static str = "f8";
    pub const F9: &'static str = "f9";
    pub const F10: &'static str = "f10";
    pub const F11: &'static str = "f11";
    pub const F12: &'static str = "f12";

    // Symbol keys
    pub const BACKTICK: &'static str = "`";
    pub const HYPHEN: &'static str = "-";
    pub const EQUALS: &'static str = "=";
    pub const LEFTBRACKET: &'static str = "[";
    pub const RIGHTBRACKET: &'static str = "]";
    pub const BACKSLASH: &'static str = "\\";
    pub const SEMICOLON: &'static str = ";";
    pub const QUOTE: &'static str = "'";
    pub const COMMA: &'static str = ",";
    pub const PERIOD: &'static str = ".";
    pub const SLASH: &'static str = "/";
    pub const EXCLAMATION: &'static str = "!";
    pub const AT: &'static str = "@";
    pub const HASH: &'static str = "#";
    pub const DOLLAR: &'static str = "$";
    pub const PERCENT: &'static str = "%";
    pub const CARET: &'static str = "^";
    pub const AMPERSAND: &'static str = "&";
    pub const ASTERISK: &'static str = "*";
    pub const LEFTPAREN: &'static str = "(";
    pub const RIGHTPAREN: &'static str = ")";
    pub const UNDERSCORE: &'static str = "_";
    pub const PLUS: &'static str = "+";
    pub const PIPE: &'static str = "|";
    pub const TILDE: &'static str = "~";
    pub const LEFTBRACE: &'static str = "{";
    pub const RIGHTBRACE: &'static str = "}";
    pub const COLON: &'static str = ":";
    pub const LESSTHAN: &'static str = "<";
    pub const GREATERTHAN: &'static str = ">";
    pub const QUESTION: &'static str = "?";

    // Single modifiers
    pub fn ctrl(key: &str) -> String {
        format!("ctrl+{key}")
    }
    pub fn shift(key: &str) -> String {
        format!("shift+{key}")
    }
    pub fn alt(key: &str) -> String {
        format!("alt+{key}")
    }
    pub fn super_key(key: &str) -> String {
        format!("super+{key}")
    }

    // Combined modifiers
    pub fn ctrl_shift(key: &str) -> String {
        format!("ctrl+shift+{key}")
    }
    pub fn shift_ctrl(key: &str) -> String {
        format!("shift+ctrl+{key}")
    }
    pub fn ctrl_alt(key: &str) -> String {
        format!("ctrl+alt+{key}")
    }
    pub fn alt_ctrl(key: &str) -> String {
        format!("alt+ctrl+{key}")
    }
    pub fn shift_alt(key: &str) -> String {
        format!("shift+alt+{key}")
    }
    pub fn alt_shift(key: &str) -> String {
        format!("alt+shift+{key}")
    }
    pub fn ctrl_super(key: &str) -> String {
        format!("ctrl+super+{key}")
    }
    pub fn super_ctrl(key: &str) -> String {
        format!("super+ctrl+{key}")
    }
    pub fn shift_super(key: &str) -> String {
        format!("shift+super+{key}")
    }
    pub fn super_shift(key: &str) -> String {
        format!("super+shift+{key}")
    }
    pub fn alt_super(key: &str) -> String {
        format!("alt+super+{key}")
    }
    pub fn super_alt(key: &str) -> String {
        format!("super+alt+{key}")
    }

    // Triple modifiers
    pub fn ctrl_shift_alt(key: &str) -> String {
        format!("ctrl+shift+alt+{key}")
    }
    pub fn ctrl_shift_super(key: &str) -> String {
        format!("ctrl+shift+super+{key}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that touch the global Kitty protocol flag
    /// (the TS suite runs sequentially; cargo test does not). Uses the
    /// crate-wide shared lock so other modules' global-state tests
    /// (stdin_buffer, terminal, ...) are serialized against these too.
    fn with_kitty<F: FnOnce()>(active: bool, f: F) {
        let _guard = crate::test_support::global_test_lock();
        set_kitty_protocol_active(active);
        f();
        set_kitty_protocol_active(false);
    }

    // ------------------------------------------------------------------
    // matchesKey — Kitty protocol with alternate keys (non-Latin layouts)
    // ------------------------------------------------------------------

    #[test]
    fn matches_ctrl_c_cyrillic_with_base_layout_key() {
        with_kitty(true, || {
            // Cyrillic 'с' = codepoint 1089, Latin 'c' = 99
            // Format: CSI 1089::99;5u (codepoint::base;modifier with ctrl=4, +1=5)
            assert!(matches_key("\x1b[1089::99;5u", "ctrl+c"));
        });
    }

    #[test]
    fn matches_ctrl_d_cyrillic_with_base_layout_key() {
        with_kitty(true, || {
            // Cyrillic 'в' = codepoint 1074, Latin 'd' = 100
            assert!(matches_key("\x1b[1074::100;5u", "ctrl+d"));
        });
    }

    #[test]
    fn matches_ctrl_z_cyrillic_with_base_layout_key() {
        with_kitty(true, || {
            // Cyrillic 'я' = codepoint 1103, Latin 'z' = 122
            assert!(matches_key("\x1b[1103::122;5u", "ctrl+z"));
        });
    }

    #[test]
    fn matches_ctrl_shift_p_with_base_layout_key() {
        with_kitty(true, || {
            // Cyrillic 'з' = codepoint 1079, Latin 'p' = 112
            // ctrl=4, shift=1, +1 = 6
            assert!(matches_key("\x1b[1079::112;6u", "ctrl+shift+p"));
        });
    }

    #[test]
    fn still_matches_direct_codepoint_when_no_base_layout_key() {
        with_kitty(true, || {
            // Latin ctrl+c without base layout key (terminal doesn't support flag 4)
            assert!(matches_key("\x1b[99;5u", "ctrl+c"));
        });
    }

    #[test]
    fn matches_super_modified_kitty_bindings_including_combined_modifiers() {
        with_kitty(true, || {
            assert!(matches_key("\x1b[107;9u", "super+k"));
            assert!(matches_key("\x1b[13;9u", "super+enter"));
            assert!(matches_key("\x1b[107;13u", &Key::ctrl_super("k")));
            assert!(matches_key("\x1b[107;13u", "ctrl+super+k"));
            assert!(matches_key("\x1b[107;14u", "ctrl+shift+super+k"));
            assert!(!matches_key("\x1b[107;13u", "super+k"));
            assert_eq!(parse_key("\x1b[107;9u").as_deref(), Some("super+k"));
            assert_eq!(parse_key("\x1b[13;9u").as_deref(), Some("super+enter"));
            assert_eq!(parse_key("\x1b[107;13u").as_deref(), Some("ctrl+super+k"));
            assert_eq!(parse_key("\x1b[107;14u").as_deref(), Some("shift+ctrl+super+k"));
        });
    }

    #[test]
    fn matches_digit_bindings_via_kitty_csi_u() {
        with_kitty(true, || {
            assert!(matches_key("\x1b[49u", "1"));
            assert!(matches_key("\x1b[49;5u", "ctrl+1"));
            assert!(!matches_key("\x1b[49;5u", "ctrl+2"));
            assert_eq!(parse_key("\x1b[49u").as_deref(), Some("1"));
            assert_eq!(parse_key("\x1b[49;5u").as_deref(), Some("ctrl+1"));
        });
    }

    #[test]
    fn normalizes_kitty_keypad_functional_keys() {
        with_kitty(true, || {
            assert!(matches_key("\x1b[57400u", "1"));
            assert!(matches_key("\x1b[57410u", "/"));
            assert!(matches_key("\x1b[57417u", "left"));
            assert!(matches_key("\x1b[57426u", "delete"));
            assert_eq!(parse_key("\x1b[57399u").as_deref(), Some("0"));
            assert_eq!(parse_key("\x1b[57409u").as_deref(), Some("."));
            assert_eq!(parse_key("\x1b[57413u").as_deref(), Some("+"));
            assert_eq!(parse_key("\x1b[57416u").as_deref(), Some(","));
            assert_eq!(parse_key("\x1b[57417u").as_deref(), Some("left"));
            assert_eq!(parse_key("\x1b[57418u").as_deref(), Some("right"));
            assert_eq!(parse_key("\x1b[57419u").as_deref(), Some("up"));
            assert_eq!(parse_key("\x1b[57420u").as_deref(), Some("down"));
            assert_eq!(parse_key("\x1b[57421u").as_deref(), Some("pageUp"));
            assert_eq!(parse_key("\x1b[57422u").as_deref(), Some("pageDown"));
            assert_eq!(parse_key("\x1b[57423u").as_deref(), Some("home"));
            assert_eq!(parse_key("\x1b[57424u").as_deref(), Some("end"));
            assert_eq!(parse_key("\x1b[57425u").as_deref(), Some("insert"));
            assert_eq!(parse_key("\x1b[57426u").as_deref(), Some("delete"));
        });
    }

    #[test]
    fn matches_shifted_symbols_by_their_produced_character() {
        with_kitty(true, || {
            assert!(matches_key("\x1b[61:43;2u", "+"));
            assert!(matches_key("\x1b[61:43;2u", "shift+="));
            assert_eq!(parse_key("\x1b[61:43;2u").as_deref(), Some("+"));
            assert!(matches_key("\x1b[61:43;6u", "ctrl++"));
            assert_eq!(parse_key("\x1b[61:43;6u").as_deref(), Some("ctrl++"));
        });
    }

    #[test]
    fn handles_shifted_key_in_format() {
        with_kitty(true, || {
            // Latin 'c' with shifted 'C' (67) and base 'c' (99)
            let shifted_key = "\x1b[99:67:99;2u"; // shift modifier = 1, +1 = 2
            assert!(matches_key(shifted_key, "shift+c"));
        });
    }

    #[test]
    fn handles_event_type_in_format() {
        with_kitty(true, || {
            // Cyrillic ctrl+c release event (event type 3)
            let release_event = "\x1b[1089::99;5:3u";
            assert!(matches_key(release_event, "ctrl+c"));
        });
    }

    #[test]
    fn handles_full_format_with_shifted_base_and_event_type() {
        with_kitty(true, || {
            // Cyrillic 'С' (shifted) with base 'c', Ctrl+Shift pressed, repeat event
            // Cyrillic 'с' = 1089, Cyrillic 'С' = 1057, Latin 'c' = 99
            // ctrl=4, shift=1, +1 = 6, repeat event = 2
            let full_format = "\x1b[1089:1057:99;6:2u";
            assert!(matches_key(full_format, "ctrl+shift+c"));
        });
    }

    #[test]
    fn prefers_codepoint_for_latin_letters_even_when_base_layout_differs() {
        with_kitty(true, || {
            // Dvorak Ctrl+K reports codepoint 'k' (107) and base layout 'v' (118)
            let dvorak_ctrl_k = "\x1b[107::118;5u";
            assert!(matches_key(dvorak_ctrl_k, "ctrl+k"));
            assert!(!matches_key(dvorak_ctrl_k, "ctrl+v"));
        });
    }

    #[test]
    fn prefers_codepoint_for_symbol_keys_even_when_base_layout_differs() {
        with_kitty(true, || {
            // Dvorak Ctrl+/ reports codepoint '/' (47) and base layout '[' (91)
            let dvorak_ctrl_slash = "\x1b[47::91;5u";
            assert!(matches_key(dvorak_ctrl_slash, "ctrl+/"));
            assert!(!matches_key(dvorak_ctrl_slash, "ctrl+["));
        });
    }

    #[test]
    fn does_not_match_wrong_key_even_with_base_layout() {
        with_kitty(true, || {
            // Cyrillic ctrl+с with base 'c' should NOT match ctrl+d
            assert!(!matches_key("\x1b[1089::99;5u", "ctrl+d"));
        });
    }

    #[test]
    fn does_not_match_wrong_modifiers_even_with_base_layout() {
        with_kitty(true, || {
            // Cyrillic ctrl+с should NOT match ctrl+shift+c
            assert!(!matches_key("\x1b[1089::99;5u", "ctrl+shift+c"));
        });
    }

    // ------------------------------------------------------------------
    // matchesKey — modifyOtherKeys matching
    // ------------------------------------------------------------------

    #[test]
    fn matches_xterm_modify_other_keys_ctrl_c() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[27;5;99~", "ctrl+c"));
            assert_eq!(parse_key("\x1b[27;5;99~").as_deref(), Some("ctrl+c"));
        });
    }

    #[test]
    fn matches_xterm_modify_other_keys_ctrl_d() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[27;5;100~", "ctrl+d"));
            assert_eq!(parse_key("\x1b[27;5;100~").as_deref(), Some("ctrl+d"));
        });
    }

    #[test]
    fn matches_xterm_modify_other_keys_ctrl_z() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[27;5;122~", "ctrl+z"));
            assert_eq!(parse_key("\x1b[27;5;122~").as_deref(), Some("ctrl+z"));
        });
    }

    #[test]
    fn matches_xterm_modify_other_keys_enter_variants() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[27;5;13~", "ctrl+enter"));
            assert!(matches_key("\x1b[27;2;13~", "shift+enter"));
            assert!(matches_key("\x1b[27;3;13~", "alt+enter"));
            assert_eq!(parse_key("\x1b[27;5;13~").as_deref(), Some("ctrl+enter"));
            assert_eq!(parse_key("\x1b[27;2;13~").as_deref(), Some("shift+enter"));
            assert_eq!(parse_key("\x1b[27;3;13~").as_deref(), Some("alt+enter"));
        });
    }

    #[test]
    fn matches_xterm_modify_other_keys_tab_variants() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[27;2;9~", "shift+tab"));
            assert!(matches_key("\x1b[27;5;9~", "ctrl+tab"));
            assert!(matches_key("\x1b[27;3;9~", "alt+tab"));
            assert_eq!(parse_key("\x1b[27;2;9~").as_deref(), Some("shift+tab"));
            assert_eq!(parse_key("\x1b[27;5;9~").as_deref(), Some("ctrl+tab"));
            assert_eq!(parse_key("\x1b[27;3;9~").as_deref(), Some("alt+tab"));
        });
    }

    #[test]
    fn matches_xterm_modify_other_keys_backspace_variants() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[27;1;127~", "backspace"));
            assert!(matches_key("\x1b[27;5;127~", "ctrl+backspace"));
            assert!(matches_key("\x1b[27;3;127~", "alt+backspace"));
            assert_eq!(parse_key("\x1b[27;1;127~").as_deref(), Some("backspace"));
            assert_eq!(parse_key("\x1b[27;5;127~").as_deref(), Some("ctrl+backspace"));
            assert_eq!(parse_key("\x1b[27;3;127~").as_deref(), Some("alt+backspace"));
        });
    }

    #[test]
    fn matches_xterm_modify_other_keys_escape() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[27;1;27~", "escape"));
            assert_eq!(parse_key("\x1b[27;1;27~").as_deref(), Some("escape"));
        });
    }

    #[test]
    fn matches_xterm_modify_other_keys_space_variants() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[27;1;32~", "space"));
            assert!(matches_key("\x1b[27;5;32~", "ctrl+space"));
            assert_eq!(parse_key("\x1b[27;1;32~").as_deref(), Some("space"));
            assert_eq!(parse_key("\x1b[27;5;32~").as_deref(), Some("ctrl+space"));
        });
    }

    #[test]
    fn matches_xterm_modify_other_keys_symbol_combos() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[27;5;47~", "ctrl+/"));
            assert_eq!(parse_key("\x1b[27;5;47~").as_deref(), Some("ctrl+/"));
            assert!(matches_key("\x1b[27;6;43~", "ctrl++"));
            assert_eq!(parse_key("\x1b[27;6;43~").as_deref(), Some("ctrl++"));
        });
    }

    #[test]
    fn matches_xterm_modify_other_keys_digit_combos() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[27;5;49~", "ctrl+1"));
            assert!(matches_key("\x1b[27;2;49~", "shift+1"));
            assert_eq!(parse_key("\x1b[27;5;49~").as_deref(), Some("ctrl+1"));
            assert_eq!(parse_key("\x1b[27;2;49~").as_deref(), Some("shift+1"));
        });
    }

    #[test]
    fn matches_xterm_modify_other_keys_shifted_uppercase_letters() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[27;2;69~", "shift+e"));
            assert!(matches_key("\x1b[27;6;69~", "ctrl+shift+e"));
            assert_eq!(parse_key("\x1b[27;2;69~").as_deref(), Some("shift+e"));
            assert_eq!(parse_key("\x1b[27;6;69~").as_deref(), Some("shift+ctrl+e"));
        });
    }

    #[test]
    fn matches_ctrl_alt_letter_via_csi_u_when_kitty_inactive() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[104;7u", "ctrl+alt+h"));
            assert_eq!(parse_key("\x1b[104;7u").as_deref(), Some("ctrl+alt+h"));
        });
    }

    #[test]
    fn matches_ctrl_alt_letter_via_xterm_modify_other_keys() {
        with_kitty(false, || {
            assert!(matches_key("\x1b[27;7;104~", "ctrl+alt+h"));
            assert_eq!(parse_key("\x1b[27;7;104~").as_deref(), Some("ctrl+alt+h"));
        });
    }

    // ------------------------------------------------------------------
    // matchesKey — Legacy key matching
    // ------------------------------------------------------------------

    #[test]
    fn matches_legacy_ctrl_c() {
        with_kitty(false, || {
            // Ctrl+c sends ASCII 3 (ETX)
            assert!(matches_key("\x03", "ctrl+c"));
        });
    }

    #[test]
    fn matches_legacy_ctrl_d() {
        with_kitty(false, || {
            // Ctrl+d sends ASCII 4 (EOT)
            assert!(matches_key("\x04", "ctrl+d"));
        });
    }

    #[test]
    fn matches_escape_key() {
        assert!(matches_key("\x1b", "escape"));
    }

    #[test]
    fn matches_legacy_linefeed_as_enter() {
        with_kitty(false, || {
            assert!(matches_key("\n", "enter"));
            assert_eq!(parse_key("\n").as_deref(), Some("enter"));
        });
    }

    #[test]
    fn treats_linefeed_as_shift_enter_when_kitty_active() {
        with_kitty(true, || {
            assert!(matches_key("\n", "shift+enter"));
            assert!(!matches_key("\n", "enter"));
            assert_eq!(parse_key("\n").as_deref(), Some("shift+enter"));
        });
    }

    #[test]
    fn parses_ctrl_space() {
        with_kitty(false, || {
            assert!(matches_key("\x00", "ctrl+space"));
            assert_eq!(parse_key("\x00").as_deref(), Some("ctrl+space"));
        });
    }

    #[test]
    fn matches_legacy_ctrl_symbol() {
        with_kitty(false, || {
            // Ctrl+\ sends ASCII 28 (File Separator) in legacy terminals
            assert!(matches_key("\x1c", "ctrl+\\"));
            assert_eq!(parse_key("\x1c").as_deref(), Some("ctrl+\\"));
            // Ctrl+] sends ASCII 29 (Group Separator) in legacy terminals
            assert!(matches_key("\x1d", "ctrl+]"));
            assert_eq!(parse_key("\x1d").as_deref(), Some("ctrl+]"));
            // Ctrl+_ sends ASCII 31 (Unit Separator) in legacy terminals
            // Ctrl+- is on the same physical key on US keyboards
            assert!(matches_key("\x1f", "ctrl+_"));
            assert!(matches_key("\x1f", "ctrl+-"));
            assert_eq!(parse_key("\x1f").as_deref(), Some("ctrl+-"));
        });
    }

    #[test]
    fn matches_legacy_ctrl_alt_symbol() {
        with_kitty(false, || {
            // Ctrl+Alt+[ sends ESC followed by ESC (Ctrl+[ = ESC)
            assert!(matches_key("\x1b\x1b", "ctrl+alt+["));
            assert_eq!(parse_key("\x1b\x1b").as_deref(), Some("ctrl+alt+["));
            // Ctrl+Alt+\ sends ESC followed by ASCII 28
            assert!(matches_key("\x1b\x1c", "ctrl+alt+\\"));
            assert_eq!(parse_key("\x1b\x1c").as_deref(), Some("ctrl+alt+\\"));
            // Ctrl+Alt+] sends ESC followed by ASCII 29
            assert!(matches_key("\x1b\x1d", "ctrl+alt+]"));
            assert_eq!(parse_key("\x1b\x1d").as_deref(), Some("ctrl+alt+]"));
            // Ctrl+_ sends ASCII 31 (Unit Separator) in legacy terminals
            // Ctrl+- is on the same physical key on US keyboards
            assert!(matches_key("\x1b\x1f", "ctrl+alt+_"));
            assert!(matches_key("\x1b\x1f", "ctrl+alt+-"));
            assert_eq!(parse_key("\x1b\x1f").as_deref(), Some("ctrl+alt+-"));
        });
    }

    #[test]
    fn treats_raw_0x08_as_plain_backspace_outside_windows_terminal() {
        with_kitty(false, || {
            with_env_vars(&[("WT_SESSION", None)], || {
                assert!(matches_key("\x7f", "backspace"));
                assert!(!matches_key("\x7f", "ctrl+backspace"));
                assert_eq!(parse_key("\x7f").as_deref(), Some("backspace"));
                assert!(matches_key("\x08", "backspace"));
                assert!(!matches_key("\x08", "ctrl+backspace"));
                assert_eq!(parse_key("\x08").as_deref(), Some("backspace"));
                assert!(matches_key("\x08", "ctrl+h"));
            });
        });
    }

    #[test]
    fn treats_raw_0x08_as_ctrl_backspace_in_local_windows_terminal() {
        with_kitty(false, || {
            with_env_vars(
                &[
                    ("WT_SESSION", Some("test-session")),
                    ("SSH_CONNECTION", None),
                    ("SSH_CLIENT", None),
                    ("SSH_TTY", None),
                ],
                || {
                    assert!(matches_key("\x08", "ctrl+backspace"));
                    assert!(!matches_key("\x08", "backspace"));
                    assert_eq!(parse_key("\x08").as_deref(), Some("ctrl+backspace"));
                    assert!(matches_key("\x08", "ctrl+h"));
                },
            );
        });
    }

    #[test]
    fn treats_raw_0x08_as_plain_backspace_in_windows_terminal_over_ssh() {
        with_kitty(false, || {
            with_env_vars(
                &[
                    ("WT_SESSION", Some("test-session")),
                    ("SSH_CONNECTION", Some("1 2 3 4")),
                    ("SSH_CLIENT", Some("1 2 3")),
                    ("SSH_TTY", Some("/dev/pts/1")),
                ],
                || {
                    assert!(!matches_key("\x08", "ctrl+backspace"));
                    assert!(matches_key("\x08", "backspace"));
                    assert_eq!(parse_key("\x08").as_deref(), Some("backspace"));
                    assert!(matches_key("\x08", "ctrl+h"));
                },
            );
        });
    }

    #[test]
    fn parses_legacy_alt_prefixed_sequences_when_kitty_inactive() {
        with_kitty(false, || {
            assert!(matches_key("\x1b ", "alt+space"));
            assert_eq!(parse_key("\x1b ").as_deref(), Some("alt+space"));
            assert!(matches_key("\x1b\x08", "alt+backspace"));
            assert_eq!(parse_key("\x1b\x08").as_deref(), Some("alt+backspace"));
            assert!(matches_key("\x1b\x03", "ctrl+alt+c"));
            assert_eq!(parse_key("\x1b\x03").as_deref(), Some("ctrl+alt+c"));
            assert!(matches_key("\x1bB", "alt+left"));
            assert_eq!(parse_key("\x1bB").as_deref(), Some("alt+left"));
            assert!(matches_key("\x1bF", "alt+right"));
            assert_eq!(parse_key("\x1bF").as_deref(), Some("alt+right"));
            assert!(matches_key("\x1ba", "alt+a"));
            assert_eq!(parse_key("\x1ba").as_deref(), Some("alt+a"));
            assert!(matches_key("\x1b1", "alt+1"));
            assert_eq!(parse_key("\x1b1").as_deref(), Some("alt+1"));
            assert!(matches_key("\x1b,", "alt+,"));
            assert_eq!(parse_key("\x1b,").as_deref(), Some("alt+,"));
            assert!(matches_key("\x1b.", "alt+."));
            assert_eq!(parse_key("\x1b.").as_deref(), Some("alt+."));
            assert!(matches_key("\x1by", "alt+y"));
            assert_eq!(parse_key("\x1by").as_deref(), Some("alt+y"));
            assert!(matches_key("\x1bz", "alt+z"));
            assert_eq!(parse_key("\x1bz").as_deref(), Some("alt+z"));

            set_kitty_protocol_active(true);
            assert!(!matches_key("\x1b ", "alt+space"));
            assert_eq!(parse_key("\x1b "), None);
            assert!(matches_key("\x1b\x08", "alt+backspace"));
            assert_eq!(parse_key("\x1b\x08").as_deref(), Some("alt+backspace"));
            assert!(!matches_key("\x1b\x03", "ctrl+alt+c"));
            assert_eq!(parse_key("\x1b\x03"), None);
            assert!(!matches_key("\x1bB", "alt+left"));
            assert_eq!(parse_key("\x1bB"), None);
            assert!(!matches_key("\x1bF", "alt+right"));
            assert_eq!(parse_key("\x1bF"), None);
            assert!(!matches_key("\x1ba", "alt+a"));
            assert_eq!(parse_key("\x1ba"), None);
            assert!(!matches_key("\x1b1", "alt+1"));
            assert_eq!(parse_key("\x1b1"), None);
            assert!(!matches_key("\x1b,", "alt+,"));
            assert_eq!(parse_key("\x1b,"), None);
            assert!(!matches_key("\x1b.", "alt+."));
            assert_eq!(parse_key("\x1b."), None);
            assert!(!matches_key("\x1by", "alt+y"));
            assert_eq!(parse_key("\x1by"), None);
        });
    }

    #[test]
    fn matches_arrow_keys() {
        assert!(matches_key("\x1b[A", "up"));
        assert!(matches_key("\x1b[B", "down"));
        assert!(matches_key("\x1b[C", "right"));
        assert!(matches_key("\x1b[D", "left"));
    }

    #[test]
    fn matches_ss3_arrows_and_home_end() {
        assert!(matches_key("\x1bOA", "up"));
        assert!(matches_key("\x1bOB", "down"));
        assert!(matches_key("\x1bOC", "right"));
        assert!(matches_key("\x1bOD", "left"));
        assert!(matches_key("\x1bOH", "home"));
        assert!(matches_key("\x1bOF", "end"));
    }

    #[test]
    fn matches_xterm_ctrl_modified_viewport_navigation() {
        assert!(matches_key("\x1b[1;5H", "ctrl+home"));
        assert!(matches_key("\x1b[1;5F", "ctrl+end"));
        assert!(matches_key("\x1b[5;5~", "ctrl+pageUp"));
        assert!(matches_key("\x1b[6;5~", "ctrl+pageDown"));
        assert_eq!(parse_key("\x1b[1;5H").as_deref(), Some("ctrl+home"));
        assert_eq!(parse_key("\x1b[1;5F").as_deref(), Some("ctrl+end"));
        assert_eq!(parse_key("\x1b[5;5~").as_deref(), Some("ctrl+pageUp"));
        assert_eq!(parse_key("\x1b[6;5~").as_deref(), Some("ctrl+pageDown"));
    }

    #[test]
    fn matches_legacy_function_keys_and_clear() {
        assert!(matches_key("\x1bOP", "f1"));
        assert!(matches_key("\x1b[24~", "f12"));
        assert!(matches_key("\x1b[E", "clear"));
    }

    #[test]
    fn matches_alt_arrows() {
        assert!(matches_key("\x1bp", "alt+up"));
        assert!(!matches_key("\x1bp", "up"));
    }

    #[test]
    fn matches_rxvt_modifier_sequences() {
        assert!(matches_key("\x1b[a", "shift+up"));
        assert!(matches_key("\x1bOa", "ctrl+up"));
        assert!(matches_key("\x1b[2$", "shift+insert"));
        assert!(matches_key("\x1b[2^", "ctrl+insert"));
        assert!(matches_key("\x1b[7$", "shift+home"));
    }

    // ------------------------------------------------------------------
    // decodeKittyPrintable
    // ------------------------------------------------------------------

    #[test]
    fn decodes_kitty_keypad_functional_keys_to_printable_characters() {
        assert_eq!(decode_kitty_printable("\x1b[57399u").as_deref(), Some("0"));
        assert_eq!(decode_kitty_printable("\x1b[57400u").as_deref(), Some("1"));
        assert_eq!(decode_kitty_printable("\x1b[57409u").as_deref(), Some("."));
        assert_eq!(decode_kitty_printable("\x1b[57410u").as_deref(), Some("/"));
        assert_eq!(decode_kitty_printable("\x1b[57411u").as_deref(), Some("*"));
        assert_eq!(decode_kitty_printable("\x1b[57412u").as_deref(), Some("-"));
        assert_eq!(decode_kitty_printable("\x1b[57413u").as_deref(), Some("+"));
        assert_eq!(decode_kitty_printable("\x1b[57415u").as_deref(), Some("="));
        assert_eq!(decode_kitty_printable("\x1b[57416u").as_deref(), Some(","));
        assert_eq!(decode_kitty_printable("\x1b[57417u"), None);
    }

    // ------------------------------------------------------------------
    // decodePrintableKey
    // ------------------------------------------------------------------

    #[test]
    fn decodes_printable_xterm_modify_other_keys_sequences() {
        assert_eq!(decode_printable_key("\x1b[27;2;69~").as_deref(), Some("E"));
        assert_eq!(decode_printable_key("\x1b[27;2;196~").as_deref(), Some("Ä"));
        assert_eq!(decode_printable_key("\x1b[27;2;32~").as_deref(), Some(" "));
        assert_eq!(decode_printable_key("\x1b[27;2;13~"), None);
        assert_eq!(decode_printable_key("\x1b[27;6;69~"), None);
    }

    // ------------------------------------------------------------------
    // parseKey — Kitty protocol with alternate keys
    // ------------------------------------------------------------------

    #[test]
    fn parse_returns_latin_key_name_when_base_layout_key_present() {
        with_kitty(true, || {
            // Cyrillic ctrl+с with base layout 'c'
            assert_eq!(parse_key("\x1b[1089::99;5u").as_deref(), Some("ctrl+c"));
        });
    }

    #[test]
    fn parse_prefers_codepoint_for_latin_letters_when_base_layout_differs() {
        with_kitty(true, || {
            // Dvorak Ctrl+K reports codepoint 'k' (107) and base layout 'v' (118)
            assert_eq!(parse_key("\x1b[107::118;5u").as_deref(), Some("ctrl+k"));
        });
    }

    #[test]
    fn parse_prefers_codepoint_for_symbol_keys_when_base_layout_differs() {
        with_kitty(true, || {
            // Dvorak Ctrl+/ reports codepoint '/' (47) and base layout '[' (91)
            assert_eq!(parse_key("\x1b[47::91;5u").as_deref(), Some("ctrl+/"));
        });
    }

    #[test]
    fn parse_returns_key_name_from_codepoint_when_no_base_layout() {
        with_kitty(true, || {
            assert_eq!(parse_key("\x1b[99;5u").as_deref(), Some("ctrl+c"));
        });
    }

    #[test]
    fn parse_parses_shifted_uppercase_csi_u_letters_as_shift_letter() {
        with_kitty(true, || {
            assert!(matches_key("\x1b[69;2u", "shift+e"));
            assert_eq!(parse_key("\x1b[69;2u").as_deref(), Some("shift+e"));
        });
    }

    #[test]
    fn parse_ignores_kitty_csi_u_with_unsupported_modifiers() {
        with_kitty(true, || {
            assert_eq!(parse_key("\x1b[99;17u"), None);
        });
    }

    // ------------------------------------------------------------------
    // parseKey — Legacy key parsing
    // ------------------------------------------------------------------

    #[test]
    fn parse_parses_legacy_ctrl_letter() {
        with_kitty(false, || {
            assert_eq!(parse_key("\x03").as_deref(), Some("ctrl+c"));
            assert_eq!(parse_key("\x04").as_deref(), Some("ctrl+d"));
        });
    }

    #[test]
    fn parse_parses_special_keys() {
        with_kitty(false, || {
            assert_eq!(parse_key("\x1b").as_deref(), Some("escape"));
        assert_eq!(parse_key("\t").as_deref(), Some("tab"));
        assert_eq!(parse_key("\r").as_deref(), Some("enter"));
        assert_eq!(parse_key("\n").as_deref(), Some("enter"));
        assert_eq!(parse_key("\x00").as_deref(), Some("ctrl+space"));
        assert_eq!(parse_key(" ").as_deref(), Some("space"));
        assert_eq!(parse_key("1").as_deref(), Some("1"));
        assert!(matches_key("1", "1"));
        assert_eq!(parse_key("+").as_deref(), Some("+"));
        assert!(matches_key("+", "+"));
        });
    }

    #[test]
    fn parse_parses_arrow_keys() {
        assert_eq!(parse_key("\x1b[A").as_deref(), Some("up"));
        assert_eq!(parse_key("\x1b[B").as_deref(), Some("down"));
        assert_eq!(parse_key("\x1b[C").as_deref(), Some("right"));
        assert_eq!(parse_key("\x1b[D").as_deref(), Some("left"));
    }

    #[test]
    fn parse_parses_ss3_arrows_and_home_end() {
        assert_eq!(parse_key("\x1bOA").as_deref(), Some("up"));
        assert_eq!(parse_key("\x1bOB").as_deref(), Some("down"));
        assert_eq!(parse_key("\x1bOC").as_deref(), Some("right"));
        assert_eq!(parse_key("\x1bOD").as_deref(), Some("left"));
        assert_eq!(parse_key("\x1bOH").as_deref(), Some("home"));
        assert_eq!(parse_key("\x1bOF").as_deref(), Some("end"));
    }

    #[test]
    fn parse_parses_legacy_function_and_modifier_sequences() {
        assert_eq!(parse_key("\x1bOP").as_deref(), Some("f1"));
        assert_eq!(parse_key("\x1b[24~").as_deref(), Some("f12"));
        assert_eq!(parse_key("\x1b[E").as_deref(), Some("clear"));
        assert_eq!(parse_key("\x1b[2^").as_deref(), Some("ctrl+insert"));
        assert_eq!(parse_key("\x1bp").as_deref(), Some("alt+up"));
    }

    #[test]
    fn parse_parses_double_bracket_page_up() {
        assert_eq!(parse_key("\x1b[[5~").as_deref(), Some("pageUp"));
    }
}
