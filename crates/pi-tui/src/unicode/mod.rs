//! Unicode data and segmentation for the TUI width engine.
//!
//! All tables in [`tables`] are generated from the *live Node/V8 runtime*
//! (`scripts/gen-unicode-tables.mjs`) so the Rust port is 1:1 by construction
//! with the TypeScript implementation's V8 regex property data, the
//! `get-east-asian-width` package tables, V8's `RGI_Emoji` Unicode set, and
//! the UCD 16.0 Grapheme_Cluster_Break and InCB classes that ICU's
//! `Intl.Segmenter` (used by the original via `utils.ts`) implements.

pub mod tables;
pub mod segmenter_corpus;
pub mod word_break;
pub mod word_break_data;

use tables::*;

/// Binary search a sorted, non-overlapping range table.
pub fn in_ranges(ranges: &[(u32, u32)], cp: u32) -> bool {
    let mut lo = 0usize;
    let mut hi = ranges.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (start, end) = ranges[mid];
        if cp < start {
            hi = mid;
        } else if cp > end {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

/// `eastAsianWidth(cp)` from `get-east-asian-width` with default options
/// (`ambiguousAsWide: false`): fullwidth or wide → 2, everything else → 1.
pub fn east_asian_width(cp: u32) -> u32 {
    if in_ranges(EAW_FULLWIDTH, cp) || in_ranges(EAW_WIDE, cp) {
        2
    } else {
        1
    }
}

/// `eastAsianWidthType(cp)` — the raw category (`"F"`, `"H"`, `"W"`, `"Na"`,
/// `"A"`, `"N"`).
pub fn east_asian_width_type(cp: u32) -> &'static str {
    if in_ranges(EAW_FULLWIDTH, cp) {
        "F"
    } else if in_ranges(EAW_HALFWIDTH, cp) {
        "H"
    } else if in_ranges(EAW_WIDE, cp) {
        "W"
    } else if in_ranges(EAW_NARROW, cp) {
        "Na"
    } else if in_ranges(EAW_AMBIGUOUS, cp) {
        "A"
    } else {
        "N"
    }
}

// --- V8 regex property predicates (used to port utils.ts regexes) ----------

/// `\p{Default_Ignorable_Code_Point}`
pub fn is_default_ignorable(cp: u32) -> bool {
    in_ranges(PROP_DEFAULT_IGNORABLE, cp)
}
/// `\p{Control}` (Cc)
pub fn is_control(cp: u32) -> bool {
    in_ranges(PROP_CONTROL, cp)
}
/// `\p{Mark}` (Mn | Mc | Me)
pub fn is_mark(cp: u32) -> bool {
    in_ranges(PROP_MARK, cp)
}
/// `\p{Format}` (Cf)
pub fn is_format(cp: u32) -> bool {
    in_ranges(PROP_FORMAT, cp)
}
/// `\p{Surrogate}` — always false for `char` (no lone surrogates in `&str`),
/// kept for parity with the JS regex where JSON inputs may carry them.
pub fn is_surrogate(_cp: u32) -> bool {
    false
}
/// `\p{Grapheme_Extend}` (DerivedCoreProperties)
pub fn is_grapheme_extend(cp: u32) -> bool {
    in_ranges(PROP_GRAPHEME_EXTEND, cp)
}
/// `\p{Spacing_Mark}`
pub fn is_spacing_mark(cp: u32) -> bool {
    in_ranges(PROP_SPACING_MARK, cp)
}
/// `\p{Extended_Pictographic}`
pub fn is_extended_pictographic(cp: u32) -> bool {
    in_ranges(PROP_EXTENDED_PICTOGRAPHIC, cp)
}
/// `\p{Emoji_Modifier}`
pub fn is_emoji_modifier(cp: u32) -> bool {
    in_ranges(PROP_EMOJI_MODIFIER, cp)
}
/// `\p{Regional_Indicator}`
pub fn is_regional_indicator_property(cp: u32) -> bool {
    in_ranges(PROP_REGIONAL_INDICATOR, cp)
}
/// `\p{Emoji}`
pub fn is_emoji(cp: u32) -> bool {
    in_ranges(PROP_EMOJI, cp)
}

// --- Script_Extensions (utils.ts cjkBreakRegex / cjkPunctuationRegex) --------

/// `\p{Script_Extensions=Han}`
pub fn is_script_han(cp: u32) -> bool {
    in_ranges(SCRIPT_HAN, cp)
}
/// `\p{Script_Extensions=Hiragana}`
pub fn is_script_hiragana(cp: u32) -> bool {
    in_ranges(SCRIPT_HIRAGANA, cp)
}
/// `\p{Script_Extensions=Katakana}`
pub fn is_script_katakana(cp: u32) -> bool {
    in_ranges(SCRIPT_KATAKANA, cp)
}
/// `\p{Script_Extensions=Hangul}`
pub fn is_script_hangul(cp: u32) -> bool {
    in_ranges(SCRIPT_HANGUL, cp)
}
/// `\p{Script_Extensions=Bopomofo}`
pub fn is_script_bopomofo(cp: u32) -> bool {
    in_ranges(SCRIPT_BOPOMOFO, cp)
}
/// The `cjkBreakRegex` character class (any of the five CJK scripts).
pub fn is_cjk_script(cp: u32) -> bool {
    is_script_han(cp)
        || is_script_hiragana(cp)
        || is_script_katakana(cp)
        || is_script_hangul(cp)
        || is_script_bopomofo(cp)
}
/// `\p{Punctuation}` (general category P*)
pub fn is_punctuation(cp: u32) -> bool {
    in_ranges(PROP_PUNCTUATION, cp)
}

/// `/^\p{RGI_Emoji}$/v` — V8's RGI emoji **string** set (ZWJ families, flags,
/// keycaps, tag sequences, skin-tone variants, …). The table only contains
/// sequences the live V8 regex accepted.
pub fn is_rgi_emoji(s: &str) -> bool {
    RGI_EMOJI_SEQUENCES.binary_search(&s).is_ok()
}

// --- grapheme segmentation (UAX #29, matching Intl.Segmenter) ---------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gcb {
    Cr,
    Lf,
    Control,
    Extend,
    Zwj,
    RegionalIndicator,
    Prepend,
    SpacingMark,
    L,
    V,
    T,
    Lv,
    Lvt,
    ExtPict,
    Other,
}

fn gcb_class(cp: u32) -> Gcb {
    match cp {
        0x0d => Gcb::Cr,
        0x0a => Gcb::Lf,
        0x200d => Gcb::Zwj,
        _ => {
            if in_ranges(GCB_PREPEND, cp) {
                Gcb::Prepend
            } else if in_ranges(GCB_CONTROL, cp) {
                Gcb::Control
            } else if in_ranges(GCB_EXTEND, cp) {
                Gcb::Extend
            } else if in_ranges(GCB_SPACING_MARK, cp) {
                Gcb::SpacingMark
            } else if in_ranges(GCB_REGIONAL_INDICATOR, cp) {
                Gcb::RegionalIndicator
            } else if in_ranges(PROP_EXTENDED_PICTOGRAPHIC, cp) {
                Gcb::ExtPict
            } else if is_hangul_l(cp) {
                Gcb::L
            } else if is_hangul_v(cp) {
                Gcb::V
            } else if is_hangul_t(cp) {
                Gcb::T
            } else if (0xac00..=0xd7a3).contains(&cp) {
                // Hangul syllable: LV if the final consonant index is 0.
                if (cp - 0xac00).is_multiple_of(28) {
                    Gcb::Lv
                } else {
                    Gcb::Lvt
                }
            } else {
                Gcb::Other
            }
        }
    }
}

fn is_hangul_l(cp: u32) -> bool {
    (0x1100..=0x115f).contains(&cp) || (0xa960..=0xa97c).contains(&cp)
}
fn is_hangul_v(cp: u32) -> bool {
    (0x1160..=0x11a7).contains(&cp) || (0xd7b0..=0xd7c6).contains(&cp)
}
fn is_hangul_t(cp: u32) -> bool {
    (0x11a8..=0x11ff).contains(&cp) || (0xd7cb..=0xd7fb).contains(&cp)
}

// --- InCB (Indic_Conjunct_Break, Unicode 16.0 / DerivedCoreProperties) ------

fn is_incb_consonant(cp: u32) -> bool {
    in_ranges(INCB_CONSONANT, cp)
}
fn is_incb_linker(cp: u32) -> bool {
    in_ranges(INCB_LINKER, cp)
}
fn is_incb_extend(cp: u32) -> bool {
    in_ranges(INCB_EXTEND, cp)
}

/// Split `s` into extended grapheme clusters, matching the original's
/// `Intl.Segmenter(granularity: "grapheme")` (ICU / UAX #29).
///
/// Implements GB3–GB999 including GB9c (Indic conjunct breaks, Unicode 16.0
/// `InCB` data), matching the live ICU the corpus was recorded from.
pub fn grapheme_segments(s: &str) -> Vec<&str> {
    if s.is_empty() {
        return Vec::new();
    }
    let mut segments = Vec::new();
    let mut start = 0usize;
    // State carried across the boundary decision:
    let mut prev: Option<Gcb> = None;
    let mut ri_run = 0usize; // consecutive Regional_Indicator count ending at prev
    let mut extpict_run = false; // an ExtPict followed only by Extends so far
    let mut zwj_ok = false; // ExtPict Extend* ZWJ matched (GB11 lookback)
    // GB9c lookback (regex semantics: state persists across cluster boundaries
    // because the pattern chars never break among themselves):
    let mut incb_consonant_seen = false;
    let mut incb_linker_seen = false;

    for (idx, ch) in s.char_indices() {
        let cp = ch as u32;
        let class = gcb_class(cp);

        if let Some(p) = prev {
            let brk = !match (p, class) {
                // GB3
                (Gcb::Cr, Gcb::Lf) => true,
                // GB4
                (Gcb::Control | Gcb::Cr | Gcb::Lf, _) => false,
                // GB5
                (_, Gcb::Control | Gcb::Cr | Gcb::Lf) => false,
                // GB6
                (Gcb::L, Gcb::L | Gcb::V | Gcb::Lv | Gcb::Lvt) => true,
                // GB7
                (Gcb::Lv | Gcb::V, Gcb::V | Gcb::T) => true,
                // GB8
                (Gcb::Lvt | Gcb::T, Gcb::T) => true,
                // GB9
                (_, Gcb::Extend | Gcb::Zwj) => true,
                // GB9a
                (_, Gcb::SpacingMark) => true,
                // GB9b
                (Gcb::Prepend, _) => true,
                // GB9c: InCB=Consonant [Extend|Linker]* Linker [Extend|Linker]* ×
                // InCB=Consonant
                (_, _) if incb_linker_seen && is_incb_consonant(cp) => true,
                // GB11: ExtPict Extend* ZWJ × ExtPict
                (Gcb::Zwj, Gcb::ExtPict) => zwj_ok,
                // GB12/13: keep RI pairs together
                (Gcb::RegionalIndicator, Gcb::RegionalIndicator) => ri_run % 2 == 1,
                // GB999
                _ => false,
            };
            if brk {
                segments.push(&s[start..idx]);
                start = idx;
                // boundary resets
                ri_run = 0;
                extpict_run = false;
                zwj_ok = false;
                // (InCB lookback state is deliberately NOT reset here: GB9c is
                // a pure lookback regex over text, not cluster state, and its
                // pattern chars can never be separated by a break anyway.)
                // re-evaluate state for the new cluster start below
            }
        }

        // Update lookback state (after the boundary decision).
        match class {
            Gcb::RegionalIndicator => ri_run += 1,
            _ => ri_run = 0,
        }
        match class {
            Gcb::ExtPict => {
                extpict_run = true;
                zwj_ok = false;
            }
            Gcb::Extend => {
                // keep extpict_run / zwj_ok
            }
            Gcb::Zwj => {
                zwj_ok = extpict_run;
                // A ZWJ is neither Extend nor ExtPict: the GB11 lookback
                // (ExtPict Extend* ZWJ) is broken for a FOLLOWING ZWJ, so
                // "⌛ ZWJ ZWJ ⌚" must split as ["⌛ ZWJ ZWJ", "⌚"] like ICU
                // (fuzz/reg-unicode-014). Without this reset the second ZWJ
                // inherited the first one's qualification and joined the
                // pictographic that follows.
                extpict_run = false;
            }
            _ => {
                extpict_run = false;
                zwj_ok = false;
            }
        }
        if is_incb_consonant(cp) {
            incb_consonant_seen = true;
            incb_linker_seen = false;
        } else if is_incb_linker(cp) {
            if incb_consonant_seen {
                incb_linker_seen = true;
            }
        } else if !is_incb_extend(cp) {
            incb_consonant_seen = false;
            incb_linker_seen = false;
        }

        prev = Some(class);
    }
    segments.push(&s[start..]);
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Replay the golden corpus of the **live Intl.Segmenter** (ICU) —
    /// generated by `scripts/gen-segmenter-corpus.mjs` from the UCD 15.1
    /// GraphemeBreakTest inputs plus targeted extras. ICU is the ground truth
    /// (the TS original uses `Intl.Segmenter`); it diverges from the published
    /// UCD expectations in exactly one known case (U+2701 ZWJ U+2701), which
    /// this corpus records as ICU behaves.
    #[test]
    fn segmenter_corpus_replay() {
        let mut checked = 0usize;
        for (input, expected) in segmenter_corpus::CORPUS {
            let got = grapheme_segments(input);
            assert_eq!(&got, expected, "input: {input:?}");
            checked += 1;
        }
        assert!(checked > 1200, "expected the full corpus, got {checked} cases");
    }

    #[test]
    fn segments_basic() {
        let segs = grapheme_segments("a\u{0301}b😀👨‍👩‍👧‍👦🇺🇸");
        assert_eq!(
            segs,
            vec!["a\u{0301}", "b", "😀", "👨‍👩‍👧‍👦", "🇺🇸"]
        );
    }

    #[test]
    fn hangul_jamo_compose() {
        // L V T sequences stay in one cluster
        let s = "\u{1100}\u{1161}\u{11a8}";
        assert_eq!(grapheme_segments(s), vec![s]);
        // Precomposed LV + T
        let s = "\u{ac00}\u{11a8}";
        assert_eq!(grapheme_segments(s), vec![s]);
        // Precomposed syllable is one cluster
        assert_eq!(grapheme_segments("\u{ac01}"), vec!["\u{ac01}"]);
    }

    #[test]
    fn rgi_emoji_matches_v8_examples() {
        assert!(is_rgi_emoji("😀"));
        assert!(is_rgi_emoji("👨‍👩‍👧‍👦"));
        assert!(is_rgi_emoji("🇺🇸"));
        assert!(is_rgi_emoji("👍🏽"));
        assert!(!is_rgi_emoji("🇺")); // lone regional indicator
        assert!(!is_rgi_emoji("a"));
    }

    #[test]
    fn east_asian_width_basics() {
        assert_eq!(east_asian_width('日' as u32), 2);
        assert_eq!(east_asian_width('a' as u32), 1);
        assert_eq!(east_asian_width('ｱ' as u32), 1); // halfwidth katakana
        assert_eq!(east_asian_width('→' as u32), 1); // ambiguous → narrow by default
    }
}
