//! JavaScript string semantics helpers.
//!
//! The original implementation is TypeScript running on V8. Several behaviors
//! (number formatting, `trimEnd`, UTF-16 length) have exact V8 semantics that
//! Rust does not share by default; they are reproduced here so that observable
//! output (tool text, error messages) matches the original byte for byte.

/// JS `String.prototype.trimEnd()`: strips trailing chars that are
/// ECMAScript WhiteSpace or LineTerminator.
/// (Unicode White_Space plus U+FEFF; LineTerminators are \n, \r, U+2028, U+2029.)
pub fn js_trim_end(s: &str) -> &str {
    let mut end = s.len();
    while end > 0 {
        // Walk back one char.
        let mut start = end - 1;
        while start > 0 && !s.is_char_boundary(start) {
            start -= 1;
        }
        let c = s[start..].chars().next().unwrap();
        if is_js_whitespace_or_terminator(c) {
            end = start;
        } else {
            break;
        }
    }
    &s[..end]
}

pub fn is_js_whitespace_or_terminator(c: char) -> bool {
    matches!(
        c,
        '\u{0009}' | '\u{000A}' | '\u{000B}' | '\u{000C}' | '\u{000D}' | '\u{0020}' | '\u{00A0}'
            | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}'
            | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// JS `Number.prototype.toFixed(1)` for the non-negative finite values we use
/// (byte counts / KiB, exact binary fractions). ECMAScript picks the integer
/// `n` such that `n / 10 - x` is closest to zero, ties to the larger `n`.
pub fn js_to_fixed_1(x: f64) -> String {
    debug_assert!(x.is_finite() && x >= 0.0);
    let n = (x * 10.0 + 0.5).floor();
    let whole = (n / 10.0).floor() as i64;
    let frac = (n - whole as f64 * 10.0) as i64;
    format!("{}.{}", whole, frac)
}

/// JS `Number.prototype.toFixed(2)` for the non-negative finite values we use
/// (image resize scale factors). Same ECMAScript rounding rule as
/// `toFixed(1)`: pick the integer `n` such that `n / 100 - x` is closest to
/// zero, ties to the larger `n`.
pub fn js_to_fixed_2(x: f64) -> String {
    debug_assert!(x.is_finite() && x >= 0.0);
    let n = (x * 100.0 + 0.5).floor();
    let whole = (n / 100.0).floor() as i64;
    let frac = (n - whole as f64 * 100.0) as i64;
    format!("{}.{:02}", whole, frac)
}

/// JS `String.prototype.length` (UTF-16 code units).
pub fn js_length(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_end_matches_js() {
        assert_eq!(js_trim_end("a  "), "a");
        assert_eq!(js_trim_end("a\u{00A0}\t"), "a");
        assert_eq!(js_trim_end("a\n"), "a");
        assert_eq!(js_trim_end("  a  "), "  a");
        assert_eq!(js_trim_end(""), "");
    }

    #[test]
    fn to_fixed_1_matches_js() {
        assert_eq!(js_to_fixed_1(0.0), "0.0");
        assert_eq!(js_to_fixed_1(1.0), "1.0");
        assert_eq!(js_to_fixed_1(0.9990234375), "1.0"); // 1023/1024
        assert_eq!(js_to_fixed_1(49.95), "50.0"); // 49.95 is 49.950000...28 in binary
        assert_eq!(js_to_fixed_1(1.25), "1.3"); // ties to larger
        assert_eq!(js_to_fixed_1(10.04), "10.0");
    }

    #[test]
    fn to_fixed_2_matches_js() {
        assert_eq!(js_to_fixed_2(0.0), "0.00");
        assert_eq!(js_to_fixed_2(1.25), "1.25");
        assert_eq!(js_to_fixed_2(1.005), "1.00"); // 1.005 is 1.00499... in binary
        assert_eq!(js_to_fixed_2(2.5), "2.50");
        assert_eq!(js_to_fixed_2(2500.0 / 2000.0), "1.25");
        assert_eq!(js_to_fixed_2(0.285), "0.28"); // 0.285 is 0.28499... in binary
        assert_eq!(js_to_fixed_2(3.748126), "3.75");
    }

    #[test]
    fn utf16_length() {
        assert_eq!(js_length("abc"), 3);
        assert_eq!(js_length("héllo"), 5);
        assert_eq!(js_length("𝐀"), 2); // surrogate pair
    }
}
