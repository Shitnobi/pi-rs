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
