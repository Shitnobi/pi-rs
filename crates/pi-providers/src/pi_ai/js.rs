//! JavaScript semantics helpers shared by the provider layer.
//!
//! These reproduce V8/Node observable behavior that plain Rust does not:
//! `Number(str)` parsing, `String(number)` formatting, `JSON.parse` error
//! messages (V8 wording) and `JSON.stringify(x, null, 2)` output.
//!
//! TS source mapping:
//! - `js_trim` — `String.prototype.trim` (ECMAScript WhiteSpace + LineTerminator)
//! - `js_to_number` — `Number(str)` / ToNumber on strings (V8)
//! - `js_number_to_string` — `Number::toString` (ECMAScript Number::toString spec)
//! - `js_json_parse` — `JSON.parse` incl. V8 error message strings
//! - `js_stringify_pretty` — `JSON.stringify(x, null, 2)`
//!
//! Known divergences (documented in docs/plans/impl-notes-providers-foundation.md):
//! - positions in JSON error messages are counted in `char`s (V8 uses UTF-16
//!   code units); identical for BMP-only input
//! - lone surrogates in `"\uD800"`-style escapes: V8 accepts them, Rust
//!   `String` cannot represent them, so `js_json_parse` errors
//! - numbers beyond f64 range (`1e400`): V8 yields `Infinity`, serde errors

use serde::Deserialize;
use serde_json::{Map, Value};

// ============================================================================
// trim
// ============================================================================

/// JS `String.prototype.trim()`.
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

// ============================================================================
// Number(str)
// ============================================================================

fn parse_radix(digits: &str, radix: u32) -> f64 {
    // JS converts the mathematical value, rounded to nearest f64. Accumulating
    // in u128 is exact up to ~3.4e38; beyond that fall back to f64 accumulation
    // (may differ from correctly-rounded by 1 ulp for extreme inputs).
    // Number("0x") / Number("0b") / Number("0o") are NaN (node-verified).
    if digits.is_empty() {
        return f64::NAN;
    }
    let mut acc: u128 = 0;
    let mut overflow = false;
    for c in digits.chars() {
        let d = match c.to_digit(radix) {
            Some(d) => d as u128,
            None => return f64::NAN,
        };
        match acc.checked_mul(radix as u128).and_then(|v| v.checked_add(d)) {
            Some(v) => acc = v,
            None => {
                overflow = true;
                break;
            }
        }
    }
    if !overflow {
        return acc as f64;
    }
    let mut value = 0.0f64;
    for c in digits.chars() {
        let d = match c.to_digit(radix) {
            Some(d) => d as f64,
            None => return f64::NAN,
        };
        value = value * radix as f64 + d;
    }
    value
}

/// True if `s` (already known non-empty and not inf/nan) is a valid JS decimal
/// literal: `[+-]? ( digits [. digits]? | . digits ) ( [eE] [+-]? digits )?`.
fn is_valid_js_decimal(s: &str) -> bool {
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == '+' || bytes[i] == '-') {
        i += 1;
    }
    let int_digits = count_digits(&bytes, i);
    i += int_digits;
    let mut frac_digits = 0;
    if i < bytes.len() && bytes[i] == '.' {
        i += 1;
        frac_digits = count_digits(&bytes, i);
        i += frac_digits;
    }
    if int_digits == 0 && frac_digits == 0 {
        return false;
    }
    if i < bytes.len() && (bytes[i] == 'e' || bytes[i] == 'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == '+' || bytes[i] == '-') {
            i += 1;
        }
        let exp_digits = count_digits(&bytes, i);
        if exp_digits == 0 {
            return false;
        }
        i += exp_digits;
    }
    i == bytes.len()
}

fn count_digits(chars: &[char], from: usize) -> usize {
    let mut n = 0;
    while from + n < chars.len() && chars[from + n].is_ascii_digit() {
        n += 1;
    }
    n
}

/// JS `Number(value: string)` — ToNumber string conversion.
/// Empty/whitespace-only strings are `0`; invalid strings are `NaN`.
pub fn js_to_number(s: &str) -> f64 {
    let t = js_trim(s);
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    // Rust's parser accepts "inf"/"infinity"/"nan" case-insensitively; JS does not.
    let lower = t.to_ascii_lowercase();
    if lower == "inf" || lower == "infinity" || lower == "nan" {
        return f64::NAN;
    }
    // Radix prefixes carry no sign and no decimal point/exponent.
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return parse_radix(h, 16);
    }
    if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        return parse_radix(b, 2);
    }
    if let Some(o) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        return parse_radix(o, 8);
    }
    if !is_valid_js_decimal(t) {
        return f64::NAN;
    }
    // Rust's f64 parser is correctly rounded like V8's for this grammar,
    // but it rejects "5.e3" style mantissas that JS accepts; normalize those.
    let normalized: String = if t.contains('.') {
        let mut out = String::with_capacity(t.len() + 1);
        let chars: Vec<char> = t.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            out.push(c);
            if c == '.' && i + 1 < chars.len() && !chars[i + 1].is_ascii_digit() {
                out.push('0');
            }
            i += 1;
        }
        out
    } else {
        t.to_string()
    };
    normalized.parse::<f64>().unwrap_or(f64::NAN)
}

// ============================================================================
// String(number)
// ============================================================================

/// JS `Number::toString()` / `` `${x}` `` — shortest round-trip digits with
/// ECMAScript's plain-vs-exponential formatting rules.
pub fn js_number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x == 0.0 {
        return "0".to_string(); // covers -0, which JS prints as "0"
    }
    let neg = x < 0.0;
    let ax = x.abs();
    if ax == f64::INFINITY {
        return if neg { "-Infinity".to_string() } else { "Infinity".to_string() };
    }
    // Shortest round-trip digits via ryu (through serde_json).
    let Some(number) = serde_json::Number::from_f64(ax) else {
        return "NaN".to_string();
    };
    let ryu = serde_json::to_string(&number).unwrap_or_default();
    let (mantissa, exp10) = match ryu.split_once('e') {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (ryu.as_str(), 0),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut digits: String = String::with_capacity(int_part.len() + frac_part.len());
    digits.push_str(int_part);
    digits.push_str(frac_part);
    // value = 0.<digits> x 10^n
    let mut n: i32 = digits.len() as i32 + exp10 - frac_part.len() as i32;
    // Strip insignificant leading/trailing zeros.
    while digits.len() > 1 && digits.starts_with('0') {
        digits.remove(0);
        n -= 1;
    }
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    let k = digits.len() as i32;
    let body = if k <= n && n <= 21 {
        // digits followed by n-k zeros
        let mut s = digits.clone();
        for _ in 0..(n - k) {
            s.push('0');
        }
        s
    } else if 0 < n && n <= 21 {
        // decimal point inside the digits
        let mut s = digits[..n as usize].to_string();
        s.push('.');
        s.push_str(&digits[n as usize..]);
        s
    } else if -6 < n && n <= 0 {
        // 0.000...digits
        let mut s = String::from("0.");
        for _ in 0..(-n) {
            s.push('0');
        }
        s.push_str(&digits);
        s
    } else {
        // exponential: d[.ddd]e±(n-1)
        let mut s = String::new();
        s.push_str(&digits[..1]);
        if k > 1 {
            s.push('.');
            s.push_str(&digits[1..]);
        }
        s.push('e');
        let m = n - 1;
        if m >= 0 {
            s.push('+');
        } else {
            s.push('-');
        }
        s.push_str(&m.abs().to_string());
        s
    };
    if neg {
        format!("-{}", body)
    } else {
        body
    }
}

// ============================================================================
// JSON.parse (V8 error messages)
// ============================================================================

struct JsonScanner {
    chars: Vec<char>,
    /// UTF-16 code-unit offset at each char index (V8 positions).
    utf16: Vec<usize>,
}

impl JsonScanner {
    fn new(input: &str) -> JsonScanner {
        let chars: Vec<char> = input.chars().collect();
        let mut utf16 = Vec::with_capacity(chars.len() + 1);
        let mut acc = 0;
        for c in &chars {
            utf16.push(acc);
            acc += c.len_utf16();
        }
        utf16.push(acc);
        JsonScanner { chars, utf16 }
    }

    fn len(&self) -> usize {
        self.chars.len()
    }

    fn eos(&self) -> String {
        "Unexpected end of JSON input".to_string()
    }

    fn line_col(&self, i: usize) -> (usize, usize) {
        let mut line = 1;
        let mut line_start = 0;
        for j in 0..i {
            if matches!(self.chars[j], '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                line += 1;
                line_start = j + 1;
            }
        }
        (line, self.utf16[i] - self.utf16[line_start] + 1)
    }

    /// Family A error: `<msg> in JSON at position P (line L column C)`.
    fn err_at(&self, i: usize, msg: &str) -> String {
        let (line, col) = self.line_col(i);
        format!("{} in JSON at position {} (line {} column {})", msg, self.utf16[i], line, col)
    }

    /// Family B error: `Unexpected token 'X', "<snippet>"[...] is not valid JSON`.
    /// The snippet is a ±10-char window around the error position; `...`
    /// appears before the opening quote when truncated at the start and after
    /// the closing quote when truncated at the end (V8 quirk).
    fn unexpected_token(&self, i: usize) -> String {
        let len = self.chars.len();
        let start = i.saturating_sub(10);
        let end = (i + 10).min(len);
        let body: String = self.chars[start..end].iter().collect();
        let mut snippet = String::new();
        if start > 0 {
            snippet.push_str("...");
        }
        snippet.push('"');
        snippet.push_str(&body);
        snippet.push('"');
        if end < len {
            snippet.push_str("...");
        }
        format!("Unexpected token '{}', {} is not valid JSON", self.chars[i], snippet)
    }

    fn skip_ws(&self, i: &mut usize) {
        while *i < self.len() && matches!(self.chars[*i], ' ' | '\t' | '\n' | '\r') {
            *i += 1;
        }
    }

    fn scan(&self) -> Result<(), String> {
        let mut i = 0;
        self.skip_ws(&mut i);
        if i >= self.len() {
            return Err(self.eos());
        }
        self.scan_value(&mut i)?;
        self.skip_ws(&mut i);
        if i < self.len() {
            // V8 wording has a single "JSON" here, not err_at's "<msg> in JSON".
            let (line, col) = self.line_col(i);
            return Err(format!(
                "Unexpected non-whitespace character after JSON at position {} (line {} column {})",
                self.utf16[i], line, col
            ));
        }
        Ok(())
    }

    fn scan_value(&self, i: &mut usize) -> Result<(), String> {
        self.skip_ws(i);
        if *i >= self.len() {
            return Err(self.eos());
        }
        match self.chars[*i] {
            '{' => self.scan_object(i),
            '[' => self.scan_array(i),
            '"' => self.scan_string(i),
            '-' | '0'..='9' => self.scan_number(i),
            't' => self.scan_literal(i, "true"),
            'f' => self.scan_literal(i, "false"),
            'n' => self.scan_literal(i, "null"),
            _ => Err(self.unexpected_token(*i)),
        }
    }

    fn scan_literal(&self, i: &mut usize, lit: &str) -> Result<(), String> {
        for (k, lc) in lit.chars().enumerate() {
            let pos = *i + k;
            if pos >= self.len() {
                return Err(self.eos());
            }
            if self.chars[pos] != lc {
                return Err(self.unexpected_token(pos));
            }
        }
        *i += lit.chars().count();
        Ok(())
    }

    fn scan_object(&self, i: &mut usize) -> Result<(), String> {
        *i += 1; // '{'
        let mut first = true;
        loop {
            self.skip_ws(i);
            if *i >= self.len() {
                let msg = if first {
                    "Expected property name or '}'"
                } else {
                    "Expected double-quoted property name"
                };
                return Err(self.err_at(*i, msg));
            }
            if self.chars[*i] == '}' && first {
                *i += 1;
                return Ok(());
            }
            // After a comma, `}` is a trailing comma — V8 demands a property
            // name (node: {"a":1,} → "Expected double-quoted property name
            // ... at position 7").
            let name_msg = if first {
                "Expected property name or '}'"
            } else {
                "Expected double-quoted property name"
            };
            if self.chars[*i] != '"' {
                return Err(self.err_at(*i, name_msg));
            }
            self.scan_string(i)?;
            self.skip_ws(i);
            if *i >= self.len() || self.chars[*i] != ':' {
                return Err(self.err_at(*i, "Expected ':' after property name"));
            }
            *i += 1;
            self.scan_value(i)?;
            self.skip_ws(i);
            if *i >= self.len() {
                return Err(self.err_at(*i, "Expected ',' or '}' after property value"));
            }
            match self.chars[*i] {
                '}' => {
                    *i += 1;
                    return Ok(());
                }
                ',' => {
                    *i += 1;
                    first = false;
                }
                _ => return Err(self.err_at(*i, "Expected ',' or '}' after property value")),
            }
        }
    }

    fn scan_array(&self, i: &mut usize) -> Result<(), String> {
        *i += 1; // '['
        self.skip_ws(i);
        if *i >= self.len() {
            return Err(self.eos());
        }
        if self.chars[*i] == ']' {
            *i += 1;
            return Ok(());
        }
        loop {
            self.scan_value(i)?;
            self.skip_ws(i);
            if *i >= self.len() {
                return Err(self.err_at(*i, "Expected ',' or ']' after array element"));
            }
            match self.chars[*i] {
                ']' => {
                    *i += 1;
                    return Ok(());
                }
                ',' => {
                    *i += 1;
                    self.skip_ws(i);
                    if *i >= self.len() {
                        return Err(self.eos());
                    }
                }
                _ => return Err(self.err_at(*i, "Expected ',' or ']' after array element")),
            }
        }
    }

    fn scan_string(&self, i: &mut usize) -> Result<(), String> {
        *i += 1; // '"'
        loop {
            if *i >= self.len() {
                return Err(self.err_at(*i, "Unterminated string"));
            }
            let c = self.chars[*i];
            if c == '"' {
                *i += 1;
                return Ok(());
            }
            if c == '\\' {
                *i += 1;
                if *i >= self.len() {
                    return Err(self.err_at(*i, "Unterminated string"));
                }
                match self.chars[*i] {
                    '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' => *i += 1,
                    'u' => {
                        *i += 1;
                        for _ in 0..4 {
                            if *i >= self.len() || !self.chars[*i].is_ascii_hexdigit() {
                                return Err(self.err_at(*i, "Bad Unicode escape"));
                            }
                            *i += 1;
                        }
                    }
                    _ => return Err(self.err_at(*i, "Bad escaped character")),
                }
            } else if (c as u32) < 0x20 {
                return Err(self.err_at(*i, "Bad control character in string literal"));
            } else {
                *i += 1;
            }
        }
    }

    fn scan_number(&self, i: &mut usize) -> Result<(), String> {
        if self.chars[*i] == '-' {
            *i += 1;
            if *i >= self.len() || !self.chars[*i].is_ascii_digit() {
                return Err(self.err_at(*i, "No number after minus sign"));
            }
        }
        if self.chars[*i] == '0' {
            *i += 1;
            if *i < self.len() && self.chars[*i].is_ascii_digit() {
                return Err(self.err_at(*i, "Unexpected number"));
            }
        } else {
            while *i < self.len() && self.chars[*i].is_ascii_digit() {
                *i += 1;
            }
        }
        if *i < self.len() && self.chars[*i] == '.' {
            *i += 1;
            if *i >= self.len() || !self.chars[*i].is_ascii_digit() {
                return Err(self.err_at(*i, "Unterminated fractional number"));
            }
            while *i < self.len() && self.chars[*i].is_ascii_digit() {
                *i += 1;
            }
        }
        if *i < self.len() && (self.chars[*i] == 'e' || self.chars[*i] == 'E') {
            *i += 1;
            if *i < self.len() && (self.chars[*i] == '+' || self.chars[*i] == '-') {
                *i += 1;
            }
            if *i >= self.len() || !self.chars[*i].is_ascii_digit() {
                return Err(self.err_at(*i, "Exponent part is missing a number"));
            }
            while *i < self.len() && self.chars[*i].is_ascii_digit() {
                *i += 1;
            }
        }
        Ok(())
    }
}

/// Normalize all JSON numbers to f64-backed `Value`s, matching JS `JSON.parse`
/// (all JS numbers are doubles; integers beyond 2^53 lose precision, exactly
/// like JS).
fn normalize_numbers(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_numbers).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k, normalize_numbers(v));
            }
            Value::Object(out)
        }
        Value::Number(n) => {
            // Route through f64 so serialization and comparison behave like JS.
            Value::Number(serde_json::Number::from_f64(n.as_f64().unwrap_or(0.0)).unwrap_or_else(|| {
                serde_json::Number::from(0)
            }))
        }
        other => other,
    }
}

/// JS `JSON.parse(input)` with V8-compatible error message strings.
/// All numbers are normalized to f64 (JS number semantics).
pub fn js_json_parse(input: &str) -> Result<Value, String> {
    // V8 special-cases these exact inputs.
    if matches!(input, "NaN" | "Infinity" | "undefined") {
        return Err(format!("\"{}\" is not valid JSON", input));
    }
    JsonScanner::new(input).scan()?;
    // The scanner accepted the input; serde should too. Recursion limit is
    // disabled to match V8's effectively unbounded nesting depth.
    let mut de = serde_json::Deserializer::from_str(input);
    de.disable_recursion_limit();
    match Value::deserialize(&mut de) {
        Ok(v) => Ok(normalize_numbers(v)),
        Err(e) => Err(e.to_string()),
    }
}

// ============================================================================
// JSON.stringify(x, null, 2)
// ============================================================================

fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn stringify_pretty(value: &Value, indent: usize, unit: &str) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => js_number_to_string(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => escape_json_string(s),
        Value::Array(items) => {
            if items.is_empty() {
                return "[]".to_string();
            }
            let pad = unit.repeat(indent + 1);
            let body: Vec<String> = items.iter().map(|v| format!("{}{}", pad, stringify_pretty(v, indent + 1, unit))).collect();
            format!("[\n{}\n{}]", body.join(",\n"), unit.repeat(indent))
        }
        Value::Object(map) => {
            if map.is_empty() {
                return "{}".to_string();
            }
            let pad = unit.repeat(indent + 1);
            let body: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}{}: {}", pad, escape_json_string(k), stringify_pretty(v, indent + 1, unit)))
                .collect();
            format!("{{\n{}\n{}}}", body.join(",\n"), unit.repeat(indent))
        }
    }
}

/// JS `JSON.stringify(value, null, 2)` — 2-space pretty printing with JS
/// number formatting and raw non-ASCII output.
pub fn js_stringify_pretty(value: &Value) -> String {
    stringify_pretty(value, 0, "  ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn trim_matches_js() {
        assert_eq!(js_trim("  x\u{00A0}"), "x"); // NBSP is JS WhiteSpace — node-verified
        assert_eq!(js_trim("\u{FEFF}x"), "x");
        assert_eq!(js_trim("x\u{2028}"), "x");
        assert_eq!(js_trim("  "), "");
        assert_eq!(js_trim(""), "");
    }

    #[test]
    fn to_number_matches_js() {
        // Verified against node: Number(s)
        assert_eq!(js_to_number("0x1E"), 30.0);
        assert!(js_to_number("1_000").is_nan());
        assert_eq!(js_to_number("+5"), 5.0);
        assert_eq!(js_to_number("5."), 5.0);
        assert_eq!(js_to_number(".5"), 0.5);
        assert_eq!(js_to_number(""), 0.0);
        assert_eq!(js_to_number("  "), 0.0);
        assert_eq!(js_to_number("0b11"), 3.0);
        assert_eq!(js_to_number("0o7"), 7.0);
        assert_eq!(js_to_number("Infinity"), f64::INFINITY);
        assert_eq!(js_to_number("+Infinity"), f64::INFINITY);
        assert_eq!(js_to_number("-Infinity"), f64::NEG_INFINITY);
        assert_eq!(js_to_number("1e3"), 1000.0);
        assert_eq!(js_to_number("0X10"), 16.0);
        assert!(js_to_number("0x").is_nan());
        assert_eq!(js_to_number(" 5 "), 5.0);
        assert_eq!(js_to_number("-0").to_bits(), (-0.0f64).to_bits());
        assert!(js_to_number("abc").is_nan());
        assert!(js_to_number("5e").is_nan());
        assert!(js_to_number("+0x10").is_nan());
        assert!(js_to_number(".").is_nan());
        assert!(js_to_number("+.").is_nan());
        assert_eq!(js_to_number("1.5e2"), 150.0);
        assert!(js_to_number("０").is_nan()); // fullwidth digit
        assert_eq!(js_to_number("5.e3"), 5000.0);
        assert!(js_to_number("inf").is_nan());
        // "Infinity " trims to "Infinity" — node-verified
        assert!(js_to_number("Infinity ").is_infinite());
    }

    #[test]
    fn number_to_string_matches_js() {
        // Verified against node: String(n)
        assert_eq!(js_number_to_string(1500.0 / 1000.0), "1.5");
        assert_eq!(js_number_to_string(100.0 / 1000.0), "0.1");
        assert_eq!(js_number_to_string(0.0 / 1000.0), "0");
        assert_eq!(js_number_to_string(-0.0), "0");
        assert_eq!(js_number_to_string(300000.0), "300000");
        assert_eq!(js_number_to_string(1.0 / 1000.0), "0.001");
        assert_eq!(js_number_to_string(5e-7), "5e-7");
        assert_eq!(js_number_to_string(0.0005 / 1000.0), "5e-7"); // node-verified
        assert_eq!(js_number_to_string(5e-10), "5e-10");
        assert_eq!(js_number_to_string(1e19), "10000000000000000000");
        assert_eq!(js_number_to_string(1e21), "1e+21");
        assert_eq!(js_number_to_string(1e22 / 1000.0), "10000000000000000000");
        assert_eq!(js_number_to_string(123456789.0 / 1000.0), "123456.789");
        assert_eq!(js_number_to_string(1.005 / 1000.0), "0.0010049999999999998");
        assert_eq!(js_number_to_string(f64::NAN), "NaN");
        assert_eq!(js_number_to_string(f64::INFINITY), "Infinity");
        assert_eq!(js_number_to_string(-f64::INFINITY), "-Infinity");
        assert_eq!(js_number_to_string(-1.5), "-1.5");
        assert_eq!(js_number_to_string(100.0), "100");
    }

    #[test]
    fn json_parse_valid() {
        // All numbers normalize to f64 (JS number semantics), so compare the
        // re-serialized form, not serde_json Value equality (u64 vs f64).
        let v = js_json_parse(" {\"a\": [1, 2.5, null, true]} ").unwrap();
        assert_eq!(js_stringify_pretty(&v), "{\n  \"a\": [\n    1,\n    2.5,\n    null,\n    true\n  ]\n}");
        // JS number semantics: integers are doubles.
        let v = js_json_parse("[1, 1.0, 100000000000000000000]").unwrap();
        assert_eq!(js_stringify_pretty(&v), "[\n  1,\n  1,\n  100000000000000000000\n]");
    }

    #[test]
    fn json_parse_errors_match_v8() {
        // All messages verified against node (V8).
        let cases: Vec<(&str, &str)> = vec![
            ("", "Unexpected end of JSON input"),
            (" ", "Unexpected end of JSON input"),
            ("hello", "Unexpected token 'h', \"hello\" is not valid JSON"),
            ("{", "Expected property name or '}' in JSON at position 1 (line 1 column 2)"),
            ("[", "Unexpected end of JSON input"),
            ("{\"a\"", "Expected ':' after property name in JSON at position 4 (line 1 column 5)"),
            ("{\"a\":1", "Expected ',' or '}' after property value in JSON at position 6 (line 1 column 7)"),
            ("{\"a\":}", "Unexpected token '}', \"{\"a\":}\" is not valid JSON"),
            ("{a:1}", "Expected property name or '}' in JSON at position 1 (line 1 column 2)"),
            ("{\"a\":1,}", "Expected double-quoted property name in JSON at position 7 (line 1 column 8)"),
            ("[1,]", "Unexpected token ']', \"[1,]\" is not valid JSON"),
            ("[1 2]", "Expected ',' or ']' after array element in JSON at position 3 (line 1 column 4)"),
            ("tru", "Unexpected end of JSON input"),
            ("nul", "Unexpected end of JSON input"),
            ("\"abc", "Unterminated string in JSON at position 4 (line 1 column 5)"),
            ("\"a\\qb\"", "Bad escaped character in JSON at position 3 (line 1 column 4)"),
            ("01", "Unexpected number in JSON at position 1 (line 1 column 2)"),
            ("1.", "Unterminated fractional number in JSON at position 2 (line 1 column 3)"),
            (".5", "Unexpected token '.', \".5\" is not valid JSON"),
            ("+1", "Unexpected token '+', \"+1\" is not valid JSON"),
            ("1e", "Exponent part is missing a number in JSON at position 2 (line 1 column 3)"),
            ("-", "No number after minus sign in JSON at position 1 (line 1 column 2)"),
            ("{\"a\":1}x", "Unexpected non-whitespace character after JSON at position 7 (line 1 column 8)"),
            ("[}", "Unexpected token '}', \"[}\" is not valid JSON"),
            ("{]", "Expected property name or '}' in JSON at position 1 (line 1 column 2)"),
            ("NaN", "\"NaN\" is not valid JSON"),
            ("undefined", "\"undefined\" is not valid JSON"),
            ("Infinity", "\"Infinity\" is not valid JSON"),
            ("[1,2", "Expected ',' or ']' after array element in JSON at position 4 (line 1 column 5)"),
            ("{\"a\":[1,{", "Expected property name or '}' in JSON at position 9 (line 1 column 10)"),
            ("\"\\u00\"", "Bad Unicode escape in JSON at position 5 (line 1 column 6)"),
            ("truX", "Unexpected token 'X', \"truX\" is not valid JSON"),
            ("truex", "Unexpected non-whitespace character after JSON at position 4 (line 1 column 5)"),
            ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Unexpected token 'a', \"aaaaaaaaaa\"... is not valid JSON"),
            ("{\"a\":", "Unexpected end of JSON input"),
            ("[1,", "Unexpected end of JSON input"),
            ("-x", "No number after minus sign in JSON at position 1 (line 1 column 2)"),
            ("{\"a\":1,\n}", "Expected double-quoted property name in JSON at position 8 (line 2 column 1)"),
            ("{\n\"a\" 1}", "Expected ':' after property name in JSON at position 6 (line 2 column 5)"),
            ("  \n  x", "Unexpected token 'x', \"  \n  x\" is not valid JSON"),
            ("[1,2,", "Unexpected end of JSON input"),
            ("\"\\u12\"", "Bad Unicode escape in JSON at position 5 (line 1 column 6)"),
            ("{\"a\":undefined}", "Unexpected token 'u', \"{\"a\":undefined}\" is not valid JSON"),
            ("[undefined]", "Unexpected token 'u', \"[undefined]\" is not valid JSON"),
            ("-Infinity", "No number after minus sign in JSON at position 1 (line 1 column 2)"),
            ("1e+", "Exponent part is missing a number in JSON at position 3 (line 1 column 4)"),
            ("{\"a\":1}}", "Unexpected non-whitespace character after JSON at position 7 (line 1 column 8)"),
            ("[[]", "Expected ',' or ']' after array element in JSON at position 3 (line 1 column 4)"),
            ("[,]", "Unexpected token ',', \"[,]\" is not valid JSON"),
            ("{,}", "Expected property name or '}' in JSON at position 1 (line 1 column 2)"),
            ("\"a\nb\"", "Bad control character in string literal in JSON at position 2 (line 1 column 3)"),
            ("1.2.3", "Unexpected non-whitespace character after JSON at position 3 (line 1 column 4)"),
            ("00", "Unexpected number in JSON at position 1 (line 1 column 2)"),
            ("-.5", "No number after minus sign in JSON at position 1 (line 1 column 2)"),
            ("[1", "Expected ',' or ']' after array element in JSON at position 2 (line 1 column 3)"),
            ("{\"a\":1,", "Expected double-quoted property name in JSON at position 7 (line 1 column 8)"),
            ("[1.]", "Unterminated fractional number in JSON at position 3 (line 1 column 4)"),
            ("[1e]", "Exponent part is missing a number in JSON at position 3 (line 1 column 4)"),
            ("\"\\u\"", "Bad Unicode escape in JSON at position 3 (line 1 column 4)"),
            ("\"\\uZZZZ\"", "Bad Unicode escape in JSON at position 3 (line 1 column 4)"),
            ("[\"a\\u12\"]", "Bad Unicode escape in JSON at position 7 (line 1 column 8)"),
            ("{\"a\"x", "Expected ':' after property name in JSON at position 4 (line 1 column 5)"),
            ("[1}]", "Expected ',' or ']' after array element in JSON at position 2 (line 1 column 3)"),
            ("{\"a\":1 2}", "Expected ',' or '}' after property value in JSON at position 7 (line 1 column 8)"),
            ("[00]", "Unexpected number in JSON at position 2 (line 1 column 3)"),
            ("[-]", "No number after minus sign in JSON at position 2 (line 1 column 3)"),
            ("[+1]", "Unexpected token '+', \"[+1]\" is not valid JSON"),
            ("true1", "Unexpected non-whitespace character after JSON at position 4 (line 1 column 5)"),
            ("tx", "Unexpected token 'x', \"tx\" is not valid JSON"),
            ("[NaN]", "Unexpected token 'N', \"[NaN]\" is not valid JSON"),
            ("[1,2,3,4,5,6,7,8,9,10,x]", "Unexpected token 'x', ...\",7,8,9,10,x]\" is not valid JSON"),
            ("hello world this is long", "Unexpected token 'h', \"hello worl\"... is not valid JSON"),
            ("0x1", "Unexpected non-whitespace character after JSON at position 1 (line 1 column 2)"),
            ("--1", "No number after minus sign in JSON at position 1 (line 1 column 2)"),
            ("1E", "Exponent part is missing a number in JSON at position 2 (line 1 column 3)"),
            ("\"\\x\"", "Bad escaped character in JSON at position 2 (line 1 column 3)"),
        ];
        for (input, expected) in cases {
            let got = js_json_parse(input).unwrap_err();
            assert_eq!(got, expected, "input: {:?}", input);
        }
    }

    #[test]
    fn stringify_pretty_matches_js() {
        // Verified against node: JSON.stringify(x, null, 2)
        assert_eq!(js_stringify_pretty(&json!({})), "{}");
        assert_eq!(js_stringify_pretty(&json!([])), "[]");
        assert_eq!(js_stringify_pretty(&json!({"a": 1})), "{\n  \"a\": 1\n}");
        assert_eq!(js_stringify_pretty(&json!([1, 2])), "[\n  1,\n  2\n]");
        assert_eq!(
            js_stringify_pretty(&json!({"a": {"b": [true, null, "x"]}})),
            "{\n  \"a\": {\n    \"b\": [\n      true,\n      null,\n      \"x\"\n    ]\n  }\n}"
        );
        // JS-style number formatting.
        assert_eq!(js_stringify_pretty(&json!({"n": 1.5})), "{\n  \"n\": 1.5\n}");
        // Control-character escaping (V8 uses lowercase hex).
        assert_eq!(js_stringify_pretty(&json!("\u{0001}")), "\"\\u0001\"");
        assert_eq!(js_stringify_pretty(&json!("\u{0008}")), "\"\\b\"");
        // Non-ASCII stays raw.
        assert_eq!(js_stringify_pretty(&json!("héllo")), "\"héllo\"");
    }
}
