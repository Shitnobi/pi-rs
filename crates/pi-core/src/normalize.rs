//! Unicode normalization helpers.
//!
//! Ports `normalizeForFuzzyMatch` from `src/core/tools/edit-diff.ts` and the
//! NFD variant lookup from `src/core/tools/path-utils.ts`.

use unicode_normalization::UnicodeNormalization;

/// `String.prototype.normalize("NFKC")`
pub fn nfkc_str(s: &str) -> String {
    s.nfkc().collect()
}

/// `String.prototype.normalize("NFD")`
pub fn nfd_str(s: &str) -> String {
    s.nfd().collect()
}

/// Normalize text for fuzzy matching. Port of `normalizeForFuzzyMatch`
/// (edit-diff.ts). Progressive transformations, applied in the same order:
/// - NFKC normalization
/// - Strip trailing whitespace from each line (JS `trimEnd`)
/// - Normalize smart quotes to ASCII equivalents
/// - Normalize Unicode dashes/hyphens to ASCII hyphen
/// - Normalize special Unicode spaces to regular space
pub fn normalize_for_fuzzy_match(text: &str) -> String {
    let nfkc_text = nfkc_str(text);

    // Strip trailing whitespace per line (split on \n first, as the original).
    let trimmed: Vec<String> = nfkc_text
        .split('\n')
        .map(|line| crate::js_string::js_trim_end(line).to_string())
        .collect();
    let mut out = trimmed.join("\n");

    // Char-level replacements (disjoint character classes, one pass is fine).
    out = out
        .chars()
        .map(|c| match c {
            // Smart single quotes → '
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            // Smart double quotes → "
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            // Various dashes/hyphens → -
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            // Special spaces → regular space
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect();

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_quotes_and_dashes() {
        assert_eq!(normalize_for_fuzzy_match("\u{201C}hi\u{201D}"), "\"hi\"");
        assert_eq!(normalize_for_fuzzy_match("a\u{2013}b"), "a-b");
        assert_eq!(normalize_for_fuzzy_match("a\u{00A0}b"), "a b");
    }

    #[test]
    fn trailing_whitespace_stripped_per_line() {
        assert_eq!(normalize_for_fuzzy_match("a  \nb\t\n"), "a\nb\n".to_string());
        // trailing newline is preserved (it separates lines, not trailing ws)
        assert!(normalize_for_fuzzy_match("a\n").ends_with('\n'));
    }

    #[test]
    fn nfkc_compat() {
        // NFKC folds compatibility characters such as ﬁ ligature.
        assert_eq!(nfkc_str("\u{FB01}"), "fi");
    }
}
