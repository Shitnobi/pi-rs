//! Text helpers ported from `src/utils/text.ts`.

/// Split a leading UTF-8 byte order mark from decoded text.
/// Port of `splitBom` in `src/utils/text.ts`. Returns `(bom, text)`.
pub fn split_bom(content: &str) -> (&str, &str) {
    if let Some(rest) = content.strip_prefix('\u{FEFF}') {
        ("\u{FEFF}", rest)
    } else {
        ("", content)
    }
}

/// Remove a leading UTF-8 byte order mark from decoded text.
pub fn strip_bom(content: &str) -> &str {
    split_bom(content).1
}

/// Sanitize binary output for display/storage. Port of
/// `sanitizeBinaryOutput` in `src/utils/shell.ts`: removes control
/// characters (except tab, newline, carriage return) and the Unicode
/// interlinear annotation characters U+FFF9..U+FFFB.
pub fn sanitize_binary_output(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            let u = c as u32;
            !((0x00..=0x08).contains(&u)
                || u == 0x0B
                || u == 0x0C
                || (0x0E..=0x1F).contains(&u)
                || (0xFFF9..=0xFFFB).contains(&u))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_bom_behaviour() {
        assert_eq!(split_bom("\u{FEFF}abc"), ("\u{FEFF}", "abc"));
        assert_eq!(split_bom("abc"), ("", "abc"));
        assert_eq!(split_bom("a\u{FEFF}b"), ("", "a\u{FEFF}b"));
        assert_eq!(strip_bom("\u{FEFF}abc"), "abc");
    }
}
