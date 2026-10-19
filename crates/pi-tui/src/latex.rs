//! Port of `packages/tui/src/latex.ts` — Unicode-text LaTeX rendering.
//! Ground truth: /tmp/pi-repo/packages/tui/src/latex.ts (1506 lines).
//!
//! Public seam (matches `export function renderLatex(source, options = {})`):
//! `render_latex(source, &RenderLatexOptions)` → `Option<String>`
//! (TS `string | undefined`).
//!
//! Porting notes:
//! - JS string indexing is UTF-16-code-unit based, and the TS lexer reads the
//!   source by the unit (`this.source[i]`), so an astral-plane character
//!   (e.g. emoji) in a single-unit position splits into its two surrogate
//!   halves. Rust `str`/`char` cannot represent a lone surrogate, so this port
//!   expands astral chars into surrogate-sentinel pairs at every parser entry
//!   (`js_units`) and decodes them at the boundaries — see the sentinel
//!   section below. `Vec<char>` parser sources are therefore UTF-16
//!   code-unit-indexed exactly like TS.
//! - JS `\s` / `trim()` whitespace differs from Rust's `char::is_whitespace`
//!   (JS omits U+0085, includes U+FEFF), so `js_is_whitespace` +
//!   `JS_WS_CLASS` enumerate the exact ECMAScript set.

use std::collections::HashMap;
use std::sync::OnceLock;

use fancy_regex::Regex;

use crate::utils::visible_width;

// ---------------------------------------------------------------------------
// JS UTF-16 lone-surrogate sentinels
// ---------------------------------------------------------------------------
//
// JS strings are UTF-16 code-unit sequences and the TS lexer indexes them by
// the unit (`this.source[i]`, `.slice()`, non-`u` regexes). A single-unit read
// of an astral-plane char (U+10000..U+10FFFF, e.g. emoji) captures only its
// high surrogate, leaving the low surrogate in the stream — the render result
// then contains lone surrogates (e.g. `x^😀` display → " \ud83d\nx\ude00").
//
// Rust `char` cannot hold a lone surrogate, so this port encodes UTF-16 units
// as follows:
//
// - `js_units` expands every astral char of a parser source into a pair of
//   sentinel chars (high surrogate → U+F1000..U+F13FF, low surrogate →
//   U+F1400..U+F17FF; plane-15 PUA, far above the port's own
//   U+F0000..U+F0005 layout markers). Parser `position`s are then unit
//   indices exactly like TS, and single-unit reads split astral chars
//   automatically. Applied at EVERY parser entry — TS `renderNested`
//   re-parses strings by unit too.
// - `js_code_points` emulates `Array.from`/`[...str]`: adjacent sentinel
//   pairs coalesce back to the real astral char, lone sentinels pass
//   through.
// - `visible_width` (utils) coalesces adjacent pairs before measuring
//   (matching `Intl.Segmenter` joining) and treats lone sentinels as zero
//   width (matching `\p{Surrogate}` in the TS zero-width regex).
// - The parity harness decodes final output JSON via
//   `decode_surrogate_sentinels`: pairs coalesce to the real char, lone
//   sentinels escape as `\uXXXX` exactly like `JSON.stringify`.
//
// Known limitation (accepted): input containing literal U+F1000..U+F17FF
// plane-15 PUA chars would be misdecoded as surrogate halves. Real pi usage
// never produces those code points; TS's own latex markers stop at U+F0005.

/// High surrogates (U+D800..U+DBFF) encode as U+F1000..U+F13FF.
const SENTINEL_HIGH_BASE: u32 = 0xF1000;
/// Low surrogates (U+DC00..U+DFFF) encode as U+F1400..U+F17FF.
const SENTINEL_LOW_BASE: u32 = 0xF1400;

/// Encode one UTF-16 code unit as its sentinel char.
pub(crate) fn surrogate_sentinel(unit: u16) -> char {
    let cp = if (0xD800..=0xDBFF).contains(&unit) {
        SENTINEL_HIGH_BASE + u32::from(unit) - 0xD800
    } else {
        SENTINEL_LOW_BASE + u32::from(unit) - 0xDC00
    };
    char::from_u32(cp).expect("surrogate sentinel is a valid scalar")
}

/// Decode a sentinel char back to its UTF-16 code unit.
pub(crate) fn sentinel_surrogate_unit(c: char) -> Option<u16> {
    sentinel_surrogate_unit_cp(c as u32)
}

/// `sentinel_surrogate_unit` on a raw code point.
pub(crate) fn sentinel_surrogate_unit_cp(cp: u32) -> Option<u16> {
    if (SENTINEL_HIGH_BASE..SENTINEL_HIGH_BASE + 0x400).contains(&cp) {
        Some((0xD800 + cp - SENTINEL_HIGH_BASE) as u16)
    } else if (SENTINEL_LOW_BASE..SENTINEL_LOW_BASE + 0x400).contains(&cp) {
        Some((0xDC00 + cp - SENTINEL_LOW_BASE) as u16)
    } else {
        None
    }
}

/// True for either sentinel half (a lone-surrogate stand-in).
pub(crate) fn is_surrogate_sentinel(c: char) -> bool {
    sentinel_surrogate_unit(c).is_some()
}

/// `Intl.Segmenter` surrogate-pair joining: a high sentinel directly followed
/// by a low sentinel is the real astral code point.
fn sentinel_pair_char(high: char, low: char) -> Option<char> {
    let h = sentinel_surrogate_unit(high)?;
    let l = sentinel_surrogate_unit(low)?;
    if (0xD800..=0xDBFF).contains(&h) && (0xDC00..=0xDFFF).contains(&l) {
        char::from_u32(0x10000 + (u32::from(h) - 0xD800) * 0x400 + u32::from(l) - 0xDC00)
    } else {
        None
    }
}

/// Expand a string into UTF-16 code units, encoding astral chars as sentinel
/// pairs. The parser's `source` is then unit-indexed like `this.source` in TS.
/// Sentinel chars already in the input pass through untouched (nested
/// re-parses receive unit strings, never re-expanded).
fn js_units(s: &str) -> Vec<char> {
    let mut units: Vec<char> = Vec::with_capacity(s.chars().count());
    for c in s.chars() {
        let cp = c as u32;
        if cp >= 0x10000 && !is_surrogate_sentinel(c) {
            let v = cp - 0x10000;
            units.push(surrogate_sentinel((0xD800 + (v >> 10)) as u16));
            units.push(surrogate_sentinel((0xDC00 + (v & 0x3FF)) as u16));
        } else {
            units.push(c);
        }
    }
    units
}

/// Coalesce sentinel pairs for code-point-semantics boundaries (u-flag
/// regexes like `[\p{L}\p{N}]`, final output). Lossless for output: pairs
/// become the real astral char, lone sentinels pass through.
fn with_coalesced_pairs(value: &str) -> String {
    coalesce_surrogate_sentinels(value).unwrap_or_else(|| value.to_string())
}

/// JS `Array.from(str)` / `[...str]`: code points — adjacent sentinel pairs
/// coalesce back to the real astral char, lone sentinels pass through.
fn js_code_points(s: &str) -> Vec<char> {
    let mut points: Vec<char> = Vec::with_capacity(s.chars().count());
    let mut iter = s.chars().peekable();
    while let Some(c) = iter.next() {
        if let Some(&next) = iter.peek() {
            if let Some(real) = sentinel_pair_char(c, next) {
                points.push(real);
                iter.next();
                continue;
            }
        }
        points.push(c);
    }
    points
}

/// Coalesce adjacent sentinel pairs into the real astral chars; `None` when
/// the string contains no sentinels (fast path). Used by the width path to
/// mirror `Intl.Segmenter` pair joining.
pub(crate) fn coalesce_surrogate_sentinels(s: &str) -> Option<String> {
    if !s.chars().any(is_surrogate_sentinel) {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    let mut iter = s.chars().peekable();
    while let Some(c) = iter.next() {
        if let Some(&next) = iter.peek() {
            if let Some(real) = sentinel_pair_char(c, next) {
                out.push(real);
                iter.next();
                continue;
            }
        }
        out.push(c);
    }
    Some(out)
}

/// Harness-boundary decode for final output: adjacent sentinel pairs coalesce
/// to the real astral char; lone sentinels escape as `\uXXXX` text exactly
/// like `JSON.stringify` of a lone surrogate.
pub fn decode_surrogate_sentinels(s: &str) -> String {
    if !s.chars().any(is_surrogate_sentinel) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut iter = s.chars().peekable();
    while let Some(c) = iter.next() {
        if let Some(&next) = iter.peek() {
            if let Some(real) = sentinel_pair_char(c, next) {
                out.push(real);
                iter.next();
                continue;
            }
        }
        match sentinel_surrogate_unit(c) {
            Some(unit) => out.push_str(&format!("\\u{unit:04x}")),
            None => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tables (TS lines 2–580)
// ---------------------------------------------------------------------------

/// TS `SYMBOLS` (latex.ts:3).
fn symbols() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        [
            ("alpha", "α"),
            ("beta", "β"),
            ("gamma", "γ"),
            ("delta", "δ"),
            ("epsilon", "ϵ"),
            ("varepsilon", "ε"),
            ("zeta", "ζ"),
            ("eta", "η"),
            ("theta", "θ"),
            ("vartheta", "ϑ"),
            ("iota", "ι"),
            ("kappa", "κ"),
            ("varkappa", "ϰ"),
            ("lambda", "λ"),
            ("mu", "μ"),
            ("nu", "ν"),
            ("xi", "ξ"),
            ("pi", "π"),
            ("varpi", "ϖ"),
            ("rho", "ρ"),
            ("varrho", "ϱ"),
            ("sigma", "σ"),
            ("varsigma", "ς"),
            ("tau", "τ"),
            ("upsilon", "υ"),
            ("phi", "ϕ"),
            ("varphi", "φ"),
            ("chi", "χ"),
            ("psi", "ψ"),
            ("omega", "ω"),
            ("Gamma", "Γ"),
            ("Delta", "Δ"),
            ("Theta", "Θ"),
            ("Lambda", "Λ"),
            ("Xi", "Ξ"),
            ("Pi", "Π"),
            ("Sigma", "Σ"),
            ("Upsilon", "Υ"),
            ("Phi", "Φ"),
            ("Psi", "Ψ"),
            ("Omega", "Ω"),
            ("pm", "±"),
            ("mp", "∓"),
            ("times", "×"),
            ("div", "÷"),
            ("cdot", "·"),
            ("ast", "∗"),
            ("star", "⋆"),
            ("circ", "∘"),
            ("bullet", "•"),
            ("oplus", "⊕"),
            ("ominus", "⊖"),
            ("otimes", "⊗"),
            ("oslash", "⊘"),
            ("odot", "⊙"),
            ("bigcirc", "○"),
            ("dagger", "†"),
            ("ddagger", "‡"),
            ("amalg", "⨿"),
            ("uplus", "⊎"),
            ("sqcap", "⊓"),
            ("sqcup", "⊔"),
            ("bowtie", "⋈"),
            ("Join", "⋈"),
            ("ltimes", "⋉"),
            ("rtimes", "⋊"),
            ("leftouterjoin", "⟕"),
            ("rightouterjoin", "⟖"),
            ("fullouterjoin", "⟗"),
            ("triangleleft", "◁"),
            ("triangleright", "▷"),
            ("wr", "≀"),
            ("cap", "∩"),
            ("cup", "∪"),
            ("bigcap", "⋂"),
            ("bigcup", "⋃"),
            ("bigwedge", "⋀"),
            ("bigvee", "⋁"),
            ("bigsqcup", "⨆"),
            ("biguplus", "⨄"),
            ("bigoplus", "⨁"),
            ("bigotimes", "⨂"),
            ("bigodot", "⨀"),
            ("setminus", "∖"),
            ("in", "∈"),
            ("notin", "∉"),
            ("ni", "∋"),
            ("subset", "⊂"),
            ("supset", "⊃"),
            ("subseteq", "⊆"),
            ("supseteq", "⊇"),
            ("sqsubset", "⊏"),
            ("sqsupset", "⊐"),
            ("sqsubseteq", "⊑"),
            ("sqsupseteq", "⊒"),
            ("prec", "≺"),
            ("preceq", "≼"),
            ("succ", "≻"),
            ("succeq", "≽"),
            ("ll", "≪"),
            ("gg", "≫"),
            ("le", "≤"),
            ("leq", "≤"),
            ("leqslant", "≤"),
            ("ge", "≥"),
            ("geq", "≥"),
            ("geqslant", "≥"),
            ("ne", "≠"),
            ("neq", "≠"),
            ("equiv", "≡"),
            ("approx", "≈"),
            ("sim", "∼"),
            ("simeq", "≃"),
            ("cong", "≅"),
            ("asymp", "≍"),
            ("doteq", "≐"),
            ("propto", "∝"),
            ("parallel", "∥"),
            ("perp", "⊥"),
            ("mid", "∣"),
            ("vdash", "⊢"),
            ("dashv", "⊣"),
            ("models", "⊨"),
            ("Vdash", "⊩"),
            ("Vvdash", "⊪"),
            ("nvdash", "⊬"),
            ("nvDash", "⊭"),
            ("forall", "∀"),
            ("exists", "∃"),
            ("nexists", "∄"),
            ("neg", "¬"),
            ("land", "∧"),
            ("wedge", "∧"),
            ("lor", "∨"),
            ("vee", "∨"),
            ("to", "→"),
            ("rightarrow", "→"),
            ("longrightarrow", "→"),
            ("leftarrow", "←"),
            ("longleftarrow", "←"),
            ("gets", "←"),
            ("leftrightarrow", "↔"),
            ("longleftrightarrow", "↔"),
            ("hookleftarrow", "↩"),
            ("hookrightarrow", "↪"),
            ("twoheadleftarrow", "↞"),
            ("twoheadrightarrow", "↠"),
            ("leftharpoonup", "↼"),
            ("leftharpoondown", "↽"),
            ("rightharpoonup", "⇀"),
            ("rightharpoondown", "⇁"),
            ("rightleftharpoons", "⇌"),
            ("leftrightharpoons", "⇋"),
            ("nearrow", "↗"),
            ("searrow", "↘"),
            ("swarrow", "↙"),
            ("nwarrow", "↖"),
            ("rightsquigarrow", "⇝"),
            ("leadsto", "⇝"),
            ("Rightarrow", "⇒"),
            ("Longrightarrow", "⇒"),
            ("Leftarrow", "⇐"),
            ("Longleftarrow", "⇐"),
            ("Leftrightarrow", "⇔"),
            ("Longleftrightarrow", "⇔"),
            ("implies", "⇒"),
            ("iff", "⇔"),
            ("mapsto", "↦"),
            ("longmapsto", "↦"),
            ("uparrow", "↑"),
            ("downarrow", "↓"),
            ("partial", "∂"),
            ("nabla", "∇"),
            ("int", "∫"),
            ("iint", "∬"),
            ("iiint", "∭"),
            ("oint", "∮"),
            ("sum", "∑"),
            ("prod", "∏"),
            ("coprod", "∐"),
            ("infty", "∞"),
            ("emptyset", "∅"),
            ("varnothing", "∅"),
            ("angle", "∠"),
            ("therefore", "∴"),
            ("because", "∵"),
            ("aleph", "ℵ"),
            ("beth", "ℶ"),
            ("gimel", "ℷ"),
            ("daleth", "ℸ"),
            ("top", "⊤"),
            ("bot", "⊥"),
            ("triangle", "△"),
            ("square", "□"),
            ("lozenge", "◊"),
            ("checkmark", "✓"),
            ("complement", "∁"),
            ("wp", "℘"),
            ("prime", "′"),
            ("ldots", "…"),
            ("dots", "…"),
            ("cdots", "⋯"),
            ("vdots", "⋮"),
            ("ddots", "⋱"),
            ("ell", "ℓ"),
            ("hbar", "ℏ"),
            ("Im", "ℑ"),
            ("Re", "ℜ"),
            ("langle", "⟨"),
            ("rangle", "⟩"),
            ("vert", "|"),
            ("lvert", "|"),
            ("rvert", "|"),
            ("Vert", "‖"),
            ("lVert", "‖"),
            ("rVert", "‖"),
            ("lbrace", "{"),
            ("rbrace", "}"),
            ("backslash", "\\"),
            ("lfloor", "⌊"),
            ("rfloor", "⌋"),
            ("lceil", "⌈"),
            ("rceil", "⌉"),
            ("colon", ":"),
        ]
        .into_iter()
        .collect()
    })
}

/// TS `NAMED_OPERATORS` (latex.ts:230).
const NAMED_OPERATORS: &[&str] = &[
    "arccos",
    "arcsin",
    "arctan",
    "arg",
    "cos",
    "cosh",
    "cot",
    "coth",
    "csc",
    "deg",
    "det",
    "dim",
    "exp",
    "gcd",
    "hom",
    "inf",
    "ker",
    "lg",
    "lim",
    "liminf",
    "limsup",
    "ln",
    "log",
    "max",
    "min",
    "Pr",
    "sec",
    "sin",
    "sinh",
    "sup",
    "tan",
    "tanh",
];

/// TS `LIMIT_OPERATORS` (latex.ts:265).
const LIMIT_OPERATORS: &[&str] = &[
    "argmax",
    "argmin",
    "inf",
    "injlim",
    "lim",
    "liminf",
    "limsup",
    "max",
    "min",
    "projlim",
    "sup",
];

/// TS `DISPLAY_LIMIT_SYMBOLS` (latex.ts:279).
const DISPLAY_LIMIT_SYMBOLS: &[&str] = &[
    "bigcap",
    "bigcup",
    "bigodot",
    "bigoplus",
    "bigotimes",
    "bigsqcup",
    "biguplus",
    "bigvee",
    "bigwedge",
    "coprod",
    "int",
    "iint",
    "iiint",
    "oint",
    "prod",
    "sum",
];

/// TS `RELATION_COMMANDS` (latex.ts:298).
const RELATION_COMMANDS: &[&str] = &[
    "Leftarrow",
    "Leftrightarrow",
    "Longleftarrow",
    "Longleftrightarrow",
    "Longrightarrow",
    "Rightarrow",
    "Join",
    "Vdash",
    "Vvdash",
    "approx",
    "asymp",
    "bowtie",
    "cong",
    "dashv",
    "fullouterjoin",
    "doteq",
    "downarrow",
    "equiv",
    "ge",
    "geq",
    "geqslant",
    "gets",
    "gg",
    "hookleftarrow",
    "hookrightarrow",
    "iff",
    "implies",
    "in",
    "leadsto",
    "le",
    "leftarrow",
    "leftharpoondown",
    "leftharpoonup",
    "leftrightarrow",
    "leftrightharpoons",
    "leftouterjoin",
    "leq",
    "leqslant",
    "ll",
    "longleftarrow",
    "longleftrightarrow",
    "longmapsto",
    "longrightarrow",
    "ltimes",
    "mapsto",
    "mid",
    "models",
    "ne",
    "nearrow",
    "neq",
    "ni",
    "notin",
    "nvdash",
    "nvDash",
    "nwarrow",
    "parallel",
    "perp",
    "prec",
    "preceq",
    "propto",
    "rightharpoondown",
    "rightharpoonup",
    "rightleftharpoons",
    "rightouterjoin",
    "rightarrow",
    "rightsquigarrow",
    "rtimes",
    "searrow",
    "sim",
    "simeq",
    "sqsubset",
    "sqsubseteq",
    "sqsupset",
    "sqsupseteq",
    "subset",
    "subseteq",
    "succ",
    "succeq",
    "supset",
    "supseteq",
    "swarrow",
    "to",
    "triangleleft",
    "triangleright",
    "twoheadleftarrow",
    "twoheadrightarrow",
    "uparrow",
    "vdash",
];

/// TS `NEGATED_SYMBOLS` (latex.ts:389).
fn negated_symbols() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        [
            ("<", "≮"),
            (">", "≯"),
            ("=", "≠"),
            ("∈", "∉"),
            ("∋", "∌"),
            ("∣", "∤"),
            ("∥", "∦"),
            ("∼", "≁"),
            ("≃", "≄"),
            ("≅", "≇"),
            ("≈", "≉"),
            ("≡", "≢"),
            ("≤", "≰"),
            ("≥", "≱"),
            ("≺", "⊀"),
            ("≻", "⊁"),
            ("⊂", "⊄"),
            ("⊃", "⊅"),
            ("⊆", "⊈"),
            ("⊇", "⊉"),
            ("⊢", "⊬"),
            ("⊨", "⊭"),
            ("↔", "↮"),
            ("←", "↚"),
            ("→", "↛"),
            ("⇒", "⇏"),
            ("⇐", "⇍"),
            ("⇔", "⇎"),
            ("≼", "⋠"),
            ("≽", "⋡"),
        ]
        .into_iter()
        .collect()
    })
}

/// TS `BLACKBOARD` (latex.ts:422) — keyed by single characters.
fn blackboard(c: char) -> Option<char> {
    Some(match c {
        'C' => 'ℂ',
        'H' => 'ℍ',
        'N' => 'ℕ',
        'P' => 'ℙ',
        'Q' => 'ℚ',
        'R' => 'ℝ',
        'Z' => 'ℤ',
        _ => return None,
    })
}

/// TS `SUPERSCRIPTS` (latex.ts:432) — keyed by single characters.
fn superscript_char(c: char) -> Option<char> {
    Some(match c {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'a' => 'ᵃ',
        'b' => 'ᵇ',
        'c' => 'ᶜ',
        'd' => 'ᵈ',
        'e' => 'ᵉ',
        'f' => 'ᶠ',
        'g' => 'ᵍ',
        'h' => 'ʰ',
        'i' => 'ⁱ',
        'j' => 'ʲ',
        'k' => 'ᵏ',
        'l' => 'ˡ',
        'm' => 'ᵐ',
        'n' => 'ⁿ',
        'o' => 'ᵒ',
        'p' => 'ᵖ',
        'r' => 'ʳ',
        's' => 'ˢ',
        't' => 'ᵗ',
        'u' => 'ᵘ',
        'v' => 'ᵛ',
        'w' => 'ʷ',
        'x' => 'ˣ',
        'y' => 'ʸ',
        'z' => 'ᶻ',
        _ => return None,
    })
}

/// TS `SUBSCRIPTS` (latex.ts:475) — keyed by single characters.
fn subscript_char(c: char) -> Option<char> {
    Some(match c {
        '0' => '₀',
        '1' => '₁',
        '2' => '₂',
        '3' => '₃',
        '4' => '₄',
        '5' => '₅',
        '6' => '₆',
        '7' => '₇',
        '8' => '₈',
        '9' => '₉',
        '+' => '₊',
        '-' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        'a' => 'ₐ',
        'e' => 'ₑ',
        'h' => 'ₕ',
        'i' => 'ᵢ',
        'j' => 'ⱼ',
        'k' => 'ₖ',
        'l' => 'ₗ',
        'm' => 'ₘ',
        'n' => 'ₙ',
        'o' => 'ₒ',
        'p' => 'ₚ',
        'r' => 'ᵣ',
        's' => 'ₛ',
        't' => 'ₜ',
        'u' => 'ᵤ',
        'v' => 'ᵥ',
        'x' => 'ₓ',
        _ => return None,
    })
}

/// TS `SPACING_COMMANDS` (latex.ts:510).
const SPACING_COMMANDS: &[&str] = &[
    ",", ":", ";", " ", ">", "enspace", "enskip", "medspace", "quad", "qquad", "thickspace",
    "thinspace",
];

/// TS `NEGATIVE_SPACING_COMMANDS` (latex.ts:524).
const NEGATIVE_SPACING_COMMANDS: &[&str] = &["!", "negmedspace", "negthickspace", "negthinspace"];

/// TS `NEGATIVE_SPACE` (latex.ts:525).
const NEGATIVE_SPACE: &str = "\u{0}";

/// TS `FONT_SWITCH_COMMANDS` (latex.ts:526).
const FONT_SWITCH_COMMANDS: &[&str] = &["bf", "cal", "it", "rm", "sf", "sl", "tt"];

/// TS `IGNORED_COMMANDS` (latex.ts:527).
const IGNORED_COMMANDS: &[&str] = &[
    "displaystyle",
    "limits",
    "nolimits",
    "scriptstyle",
    "scriptscriptstyle",
    "textstyle",
];

/// TS `SIZE_COMMANDS` (latex.ts:535).
const SIZE_COMMANDS: &[&str] = &[
    "big", "Big", "bigg", "Bigg", "bigl", "Bigl", "biggl", "Biggl", "bigr", "Bigr", "biggr",
    "Biggr",
];

/// TS `PLAIN_WRAPPERS` (latex.ts:549).
const PLAIN_WRAPPERS: &[&str] = &[
    "emph",
    "mathcal",
    "mathbf",
    "mathfrak",
    "mathit",
    "mathrm",
    "mathnormal",
    "mathscr",
    "mathsf",
    "mathtt",
    "mathup",
    "mbox",
    "overbrace",
    "pmb",
    "smash",
    "substack",
    "text",
    "textbf",
    "textit",
    "textmd",
    "textnormal",
    "textrm",
    "textsc",
    "textsf",
    "textsl",
    "texttt",
    "textup",
    "underbrace",
    "bm",
    "boldsymbol",
];

/// TS `ACCENTS` (latex.ts:581).
fn accents(command: &str) -> Option<&'static str> {
    Some(match command {
        "acute" => "\u{301}",
        "bar" => "\u{305}",
        "breve" => "\u{306}",
        "check" => "\u{30c}",
        "ddot" => "\u{308}",
        "dot" => "\u{307}",
        "grave" => "\u{300}",
        "hat" => "\u{302}",
        "mathring" => "\u{30a}",
        "overleftarrow" => "\u{20d6}",
        "overleftrightarrow" => "\u{20e1}",
        "overline" => "\u{305}",
        "overrightarrow" => "\u{20d7}",
        "tilde" => "\u{303}",
        "underline" => "\u{332}",
        "vec" => "\u{20d7}",
        "widehat" => "\u{302}",
        "widetilde" => "\u{303}",
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// JS string-semantics helpers
// ---------------------------------------------------------------------------

/// ECMAScript `\s` character class (WhiteSpace ∪ LineTerminator). Note: this
/// deliberately excludes U+0085, which Rust's `char::is_whitespace` includes.
fn js_is_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{b}'
            | '\u{c}'
            | '\r'
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

/// The ECMAScript `\s` set as a regex character class (for the TS patterns
/// that used `/\s/`).
const JS_WS_CLASS: &str =
    "[\\t\\n\\x0B\\x0C\\r \\x{A0}\\x{1680}\\x{2000}-\\x{200A}\\x{2028}\\x{2029}\\x{202F}\\x{205F}\\x{3000}\\x{FEFF}]";

/// JS `String.prototype.trim()`.
fn js_trim(s: &str) -> &str {
    s.trim_matches(js_is_whitespace)
}

/// JS `String.prototype.trimStart()`.
fn js_trim_start(s: &str) -> &str {
    s.trim_start_matches(js_is_whitespace)
}

/// JS `String.prototype.trimEnd()`.
fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(js_is_whitespace)
}

/// `" ".repeat(count)` (JS throws on negative; callers guarantee ≥ 0 here, so
/// we saturate and note the assumption).
fn repeat_spaces(count: usize) -> String {
    " ".repeat(count)
}

/// `s.startsWith(/\s/)` in TS (`/^\s/.test(sliced)`).
fn starts_with_js_whitespace(s: &str) -> bool {
    s.chars().next().is_some_and(js_is_whitespace)
}

/// `/\s$/.test(sliced)` in TS.
fn ends_with_js_whitespace(s: &str) -> bool {
    s.chars().last().is_some_and(js_is_whitespace)
}

/// JS `/[A-Z*∗]/.test(value)` (latex.ts parseScripts, ~line 1000).
fn has_upper_star(c: char) -> bool {
    c.is_ascii_uppercase() || c == '*' || c == '∗'
}

// ---------------------------------------------------------------------------
// Regexes (TS inline patterns)
// ---------------------------------------------------------------------------

fn static_regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("latex.rs: static regex must compile")
}

/// TS `NAMED_OPERATOR_LEFT_SPACING_PATTERN` (latex.ts:651).
fn named_operator_left_spacing_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| static_regex(r"(?<=[\p{L}\p{N})\]}\x{F0001}])\x{F0004}"))
}

/// TS `NAMED_OPERATOR_RIGHT_SPACING_PATTERN` (latex.ts:652).
fn named_operator_right_spacing_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| static_regex(r"\x{F0005}(?=[\p{L}\p{N}√\x{F0000}])"))
}

/// TS `LAYOUT_MARKER_PATTERN` (latex.ts:700).
fn layout_marker_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| static_regex(r"\x{F0000}(\d+)\x{F0001}"))
}

/// TS `TRAILING_LAYOUT_MARKER_PATTERN` (latex.ts:701).
fn trailing_layout_marker_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| static_regex(r"\x{F0000}(\d+)\x{F0001}$"))
}

/// TS `/[ \t]+/g` (normalizeOutput, latex.ts:662).
fn spaces_run_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| static_regex("[ \\t]+"))
}

/// TS `/\s*([=+-])\s*/g` (normalizeScriptValue, latex.ts:615).
fn script_squeeze_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| {
        static_regex(&format!("{}*([=+-]){}*", JS_WS_CLASS, JS_WS_CLASS))
    })
}

/// TS `/^[\p{L}\p{N}.]+$/u` (formatFraction / formatRoot).
fn simple_text_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| static_regex(r"^[\p{L}\p{N}.]+$"))
}

/// TS `/^[\p{N}.]+$/u` (formatFraction).
fn simple_number_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| static_regex(r"^[\p{N}.]+$"))
}

/// TS `/^\\(limits|nolimits)(?![A-Za-z])/` (parseOperator, latex.ts:1216).
fn limits_modifier_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| static_regex(r"^\\(limits|nolimits)(?![A-Za-z])"))
}

/// TS `/\\\\(?:\[[^\]\n]*\])?/` (splitEnvironmentRows, latex.ts:1342).
fn row_split_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| static_regex(r"\\\\(?:\[[^\]\n]*\])?"))
}

/// TS `/^\s*\{[^}]*\}/` (parseEnvironment, latex.ts:1386 & 1413).
fn brace_prefix_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| static_regex(&format!("^{}*\\{{[^}}]*\\}}", JS_WS_CLASS)))
}

/// TS `/,\s*$/` (renderCases, latex.ts:1437).
fn trailing_comma_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| static_regex(&format!(",{}*$", JS_WS_CLASS)))
}

/// TS `/^(?:if|when|for|otherwise)\b/i` (renderCases, latex.ts:1441).
/// `\b` after a letter is equivalent to `(?![A-Za-z0-9_])` (JS `\w` is ASCII).
fn condition_prefix_pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| static_regex("(?i)^(?:if|when|for|otherwise)(?![A-Za-z0-9_])"))
}

/// Replace every match of `pattern` (JS `g` flag) with `replacement`.
/// Replacement is inserted literally (no `$` expansion), matching the constant
/// string replacements used in latex.ts.
fn replace_all_literal(value: &str, pattern: &Regex, replacement: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut last = 0;
    for m in pattern.find_iter(value) {
        let m = m.expect("latex regex match");
        result.push_str(&value[last..m.start()]);
        result.push_str(replacement);
        last = m.end();
    }
    result.push_str(&value[last..]);
    result
}

/// Replace the first match of `pattern` (no `g` flag) with `replacement`.
fn replace_first_literal(value: &str, pattern: &Regex, replacement: &str) -> String {
    match pattern.find(value).expect("latex regex match") {
        Some(m) => {
            let mut result = String::with_capacity(value.len());
            result.push_str(&value[..m.start()]);
            result.push_str(replacement);
            result.push_str(&value[m.end()..]);
            result
        }
        None => value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers (TS lines 602–647)
// ---------------------------------------------------------------------------

/// TS `replaceCharacters` (latex.ts:602) — returns None when any character is
/// missing from the table.
fn replace_characters(value: &str, replacements: fn(char) -> Option<char>) -> Option<String> {
    // TS `Array.from(value, fn)` — iterates code points (sentinel pairs join).
    let mut result = String::new();
    for c in js_code_points(value) {
        result.push(replacements(c)?);
    }
    Some(result)
}

/// TS `normalizeScriptValue` (latex.ts:614):
/// `value.trim().replace(/\s*([=+-])\s*/g, "$1")`.
fn normalize_script_value(value: &str) -> String {
    let trimmed = js_trim(value);
    let pattern = script_squeeze_pattern();
    let mut result = String::with_capacity(trimmed.len());
    let mut last = 0;
    for caps in pattern.captures_iter(trimmed) {
        let caps = caps.expect("latex regex match");
        let m = caps.get(0).expect("script squeeze group 0");
        result.push_str(&trimmed[last..m.start()]);
        // Replacement "$1": the captured [=+-].
        result.push_str(caps.get(1).map(|g| g.as_str()).unwrap_or(""));
        last = m.end();
    }
    result.push_str(&trimmed[last..]);
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptKind {
    Sub,
    Sup,
}

/// TS `formatUnicodeScript` (latex.ts:618).
fn format_unicode_script(value: &str, kind: ScriptKind) -> Option<String> {
    let normalized = normalize_script_value(value);
    match kind {
        ScriptKind::Sub => replace_characters(&normalized, subscript_char),
        ScriptKind::Sup => replace_characters(&normalized, superscript_char),
    }
}

/// TS `formatScript` (latex.ts:622).
fn format_script(value: &str, kind: ScriptKind) -> String {
    let value = normalize_script_value(value);
    if let Some(unicode) = format_unicode_script(&value, kind) {
        return unicode;
    }
    let prefix = match kind {
        ScriptKind::Sub => "_",
        ScriptKind::Sup => "^",
    };
    // `/^[A-Za-z]+$/` — at least one ASCII letter, all ASCII letters.
    let all_ascii_letters =
        !value.is_empty() && value.chars().all(|c| c.is_ascii_alphabetic());
    if js_code_points(&value).len() == 1 || (kind == ScriptKind::Sub && all_ascii_letters) {
        return format!("{prefix}{value}");
    }
    format!("{prefix}({value})")
}

/// TS `formatFraction` (latex.ts:636).
fn format_fraction(numerator: &str, denominator: &str) -> String {
    // `/^[\p{L}\p{N}.]+$/u` and `/^[\p{N}.]+$/u` match code points
    // (sentinel pairs join).
    let numerator = with_coalesced_pairs(numerator);
    let denominator = with_coalesced_pairs(denominator);
    let numerator = js_trim(&numerator);
    let denominator = js_trim(&denominator);
    let simple_numerator = simple_text_pattern()
        .is_match(numerator)
        .expect("latex regex match");
    let simple_denominator = simple_number_pattern()
        .is_match(denominator)
        .expect("latex regex match")
        || js_code_points(denominator).len() == 1;
    let num = if simple_numerator {
        numerator.to_string()
    } else {
        format!("({numerator})")
    };
    let den = if simple_denominator {
        denominator.to_string()
    } else {
        format!("({denominator})")
    };
    format!("{num}/{den}")
}

/// TS `formatRoot` (latex.ts:644).
fn format_root(value: &str, symbol: &str) -> String {
    // `/^[\p{L}\p{N}.]+$/u` matches code points (sentinel pairs join).
    let value = with_coalesced_pairs(value);
    let value = js_trim(&value);
    if simple_text_pattern()
        .is_match(value)
        .expect("latex regex match")
    {
        format!("{symbol}{value}")
    } else {
        format!("{symbol}({value})")
    }
}

// ---------------------------------------------------------------------------
// Private-use markers (TS lines 649–705)
// ---------------------------------------------------------------------------

/// TS `NAMED_OPERATOR_START` (latex.ts:649).
const NAMED_OPERATOR_START: &str = "\u{f0004}";
/// TS `NAMED_OPERATOR_END` (latex.ts:650).
const NAMED_OPERATOR_END: &str = "\u{f0005}";
/// TS `LAYOUT_MARKER_START` (latex.ts:699).
const LAYOUT_MARKER_START: &str = "\u{f0000}";
/// TS `LAYOUT_MARKER_END` (latex.ts:700).
const LAYOUT_MARKER_END: &str = "\u{f0001}";
/// TS `PROTECTED_SPACE` (latex.ts:702).
const PROTECTED_SPACE: &str = "\u{f0002}";

/// TS `normalizeOutput` (latex.ts:654).
fn normalize_output(value: &str) -> String {
    // u-flag `\p{L}\p{N}` lookarounds match code points — feed them the
    // pair-coalesced string (lossless: see `with_coalesced_pairs`).
    let value = with_coalesced_pairs(value);
    let value = replace_all_literal(&value, named_operator_left_spacing_pattern(), " ");
    let value = value.replace(NAMED_OPERATOR_START, "");
    let value = replace_all_literal(&value, named_operator_right_spacing_pattern(), " ");
    let value = value.replace(NAMED_OPERATOR_END, "");
    let lines: Vec<String> = value
        .split('\n')
        .map(|line| {
            js_trim(&replace_all_literal(line, spaces_run_pattern(), " ")).to_string()
        })
        .collect();
    let total = lines.len();
    let kept: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(index, line)| !line.is_empty() || (*index > 0 && *index + 1 < total))
        .map(|(_, line)| line.as_str())
        .collect();
    js_trim(&kept.join("\n")).to_string()
}

// ---------------------------------------------------------------------------
// Layout types & rendering (TS lines 672–836)
// ---------------------------------------------------------------------------

/// TS `FractionNode` (latex.ts:672).
/// TS `OperatorNode` (latex.ts:678).
/// TS `ScriptNode` (latex.ts:684).
/// TS `MatrixNode` (latex.ts:690) — the TS `LayoutNode` union (latex.ts:694).
#[derive(Debug, Clone)]
enum LayoutNode {
    /// TS `{ type: "fraction", numerator, denominator }`.
    Fraction {
        numerator: String,
        denominator: String,
    },
    /// TS `{ type: "operator", operator, lower?, upper? }`.
    Operator {
        operator: String,
        lower: Option<String>,
        upper: Option<String>,
    },
    /// TS `{ type: "script", lower?, upper? }`.
    Script {
        lower: Option<String>,
        upper: Option<String>,
    },
    /// TS `{ type: "matrix", lines, baseline }`.
    Matrix {
        lines: Vec<String>,
        baseline: usize,
    },
}

/// TS `Layout` (latex.ts:696).
#[derive(Debug, Clone)]
struct Layout {
    lines: Vec<String>,
    width: usize,
    baseline: usize,
}

/// TS `padLayoutLine` (latex.ts:706).
fn pad_layout_line(line: &str, width: usize, centered: bool) -> String {
    let padding = width.saturating_sub(visible_width(line));
    let left = if centered { padding / 2 } else { 0 };
    format!(
        "{}{line}{}",
        repeat_spaces(left),
        repeat_spaces(padding - left)
    )
}

/// TS `joinLayouts` (latex.ts:712).
fn join_layouts(layouts: &[Layout]) -> Layout {
    if layouts.is_empty() {
        return Layout {
            lines: vec![String::new()],
            width: 0,
            baseline: 0,
        };
    }
    let baseline = layouts.iter().map(|l| l.baseline).max().unwrap_or(0);
    let below = layouts
        .iter()
        .map(|l| l.lines.len() as isize - l.baseline as isize - 1)
        .max()
        .unwrap_or(0);
    let mut lines: Vec<String> = Vec::new();
    for row in 0..=(baseline as isize + below) {
        let mut line = String::new();
        for layout in layouts {
            let source_row = row - baseline as isize + layout.baseline as isize;
            if source_row >= 0 && (source_row as usize) < layout.lines.len() {
                line.push_str(&pad_layout_line(
                    &layout.lines[source_row as usize],
                    layout.width,
                    false,
                ));
            } else {
                line.push_str(&repeat_spaces(layout.width));
            }
        }
        lines.push(js_trim_end(&line).to_string());
    }
    Layout {
        lines,
        width: layouts.iter().map(|l| l.width).sum(),
        baseline,
    }
}

/// TS `renderLayout` (latex.ts:737).
fn render_layout(source: &str, nodes: &[LayoutNode]) -> Layout {
    let mut rendered_lines: Vec<String> = Vec::new();
    let mut first_baseline = 0usize;
    for source_line in source.split('\n') {
        let mut layouts: Vec<Layout> = Vec::new();
        let mut position = 0usize; // byte offset into source_line
        let mut previous_node: Option<&LayoutNode> = None;
        for caps in layout_marker_pattern()
            .captures_iter(source_line)
        {
            let caps = caps.expect("latex regex match");
            let whole = caps.get(0).expect("layout marker group 0");
            let index = whole.start();
            let node_index: usize = caps
                .get(1)
                .and_then(|g| g.as_str().parse().ok())
                .unwrap_or(usize::MAX);
            let Some(node) = nodes.get(node_index) else {
                continue;
            };
            if index > position {
                let sliced = &source_line[position..index];
                let trimmed = js_trim_end(if previous_node.is_some() {
                    js_trim_start(sliced)
                } else {
                    sliced
                });
                let preserve_leading_space =
                    matches!(previous_node, Some(LayoutNode::Matrix { .. }))
                        && starts_with_js_whitespace(sliced);
                let preserve_trailing_space =
                    matches!(node, LayoutNode::Matrix { .. }) && ends_with_js_whitespace(sliced);
                let text = if !trimmed.is_empty() {
                    format!(
                        "{}{trimmed}{}",
                        if preserve_leading_space { " " } else { "" },
                        if preserve_trailing_space { " " } else { "" }
                    )
                } else if preserve_leading_space || preserve_trailing_space {
                    " ".to_string()
                } else {
                    String::new()
                };
                let width = visible_width(&text);
                layouts.push(Layout {
                    lines: vec![text],
                    width,
                    baseline: 0,
                });
            }
            match node {
                LayoutNode::Fraction {
                    numerator,
                    denominator,
                } => {
                    let numerator = render_layout(numerator, nodes);
                    let denominator = render_layout(denominator, nodes);
                    let content_width = numerator
                        .width
                        .max(denominator.width)
                        .max(1);
                    let width = content_width + 2;
                    let mut lines: Vec<String> = Vec::new();
                    for line in &numerator.lines {
                        lines.push(pad_layout_line(line, width, true));
                    }
                    lines.push(format!(" {} ", "─".repeat(content_width)));
                    for line in &denominator.lines {
                        lines.push(pad_layout_line(line, width, true));
                    }
                    layouts.push(Layout {
                        lines,
                        width,
                        baseline: numerator.lines.len(),
                    });
                }
                LayoutNode::Operator {
                    operator,
                    lower,
                    upper,
                } => {
                    let content_width = visible_width(operator)
                        .max(lower.as_ref().map_or(0, |l| visible_width(l)))
                        .max(upper.as_ref().map_or(0, |l| visible_width(l)));
                    let mut lines: Vec<String> = Vec::new();
                    if let Some(upper) = upper {
                        lines.push(format!("{} ", pad_layout_line(upper, content_width, true)));
                    }
                    lines.push(format!(
                        "{} ",
                        pad_layout_line(operator, content_width, true)
                    ));
                    if let Some(lower) = lower {
                        lines.push(format!("{} ", pad_layout_line(lower, content_width, true)));
                    }
                    layouts.push(Layout {
                        lines,
                        width: content_width + 1,
                        baseline: if upper.is_none() { 0 } else { 1 },
                    });
                }
                LayoutNode::Script { lower, upper } => {
                    let upper = upper.as_ref().map(|u| render_layout(u, nodes));
                    let lower = lower.as_ref().map(|l| render_layout(l, nodes));
                    let width = upper.as_ref().map_or(0, |l| l.width)
                        .max(lower.as_ref().map_or(0, |l| l.width));
                    let mut lines: Vec<String> = Vec::new();
                    if let Some(upper) = &upper {
                        for line in &upper.lines {
                            lines.push(pad_layout_line(line, width, false));
                        }
                    }
                    lines.push(repeat_spaces(width));
                    if let Some(lower) = &lower {
                        for line in &lower.lines {
                            lines.push(pad_layout_line(line, width, false));
                        }
                    }
                    layouts.push(Layout {
                        lines,
                        width,
                        baseline: upper.map_or(0, |l| l.lines.len()),
                    });
                }
                LayoutNode::Matrix { lines, baseline } => {
                    let width = lines.iter().map(|l| visible_width(l)).max().unwrap_or(0);
                    layouts.push(Layout {
                        lines: lines.iter().map(|l| pad_layout_line(l, width, false)).collect(),
                        width,
                        baseline: *baseline,
                    });
                }
            }
            position = whole.end();
            previous_node = Some(node);
        }
        if position < source_line.len() {
            let sliced = &source_line[position..];
            let trimmed = if previous_node.is_some() {
                js_trim_start(sliced)
            } else {
                sliced
            };
            let text = if matches!(previous_node, Some(LayoutNode::Matrix { .. }))
                && starts_with_js_whitespace(sliced)
            {
                format!(" {trimmed}")
            } else {
                trimmed.to_string()
            };
            let width = visible_width(&text);
            layouts.push(Layout {
                lines: vec![text],
                width,
                baseline: 0,
            });
        }
        let line_layout = join_layouts(&layouts);
        if rendered_lines.is_empty() {
            first_baseline = line_layout.baseline;
        }
        rendered_lines.extend(line_layout.lines);
    }
    let width = rendered_lines
        .iter()
        .map(|l| visible_width(l))
        .max()
        .unwrap_or(0);
    Layout {
        lines: rendered_lines,
        width,
        baseline: first_baseline,
    }
}

// ---------------------------------------------------------------------------
// Parser (TS `class LatexParser`, latex.ts:838–1478)
// ---------------------------------------------------------------------------

/// TS `inlineLowerStyle: "bracket" | "script"` (parseOperator).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineLowerStyle {
    Bracket,
    Script,
}

/// TS `class LatexParser` (latex.ts:838).
struct LatexParser<'n> {
    /// `private readonly source` — UTF-16 code units (astral chars expanded
    /// to surrogate-sentinel pairs by `js_units`), so `position` is a unit
    /// index exactly like TS `this.source`.
    source: Vec<char>,
    /// `private readonly layoutNodes` — shared across nested parsers.
    layout_nodes: &'n mut Vec<LayoutNode>,
    /// `private readonly display`.
    display: bool,
    /// `private position = 0`.
    position: usize,
    /// `private supported = true`.
    supported: bool,
    /// `private stackFractions = true`.
    stack_fractions: bool,
    /// `private scriptDepth = 0`.
    script_depth: usize,
}

impl<'n> LatexParser<'n> {
    fn new(
        source: Vec<char>,
        layout_nodes: &'n mut Vec<LayoutNode>,
        display: bool,
    ) -> LatexParser<'n> {
        LatexParser {
            source,
            layout_nodes,
            display,
            position: 0,
            supported: true,
            stack_fractions: true,
            script_depth: 0,
        }
    }

    /// `this.source[this.position]` — `undefined` past the end.
    fn char_at(&self, position: usize) -> Option<char> {
        self.source.get(position).copied()
    }

    /// TS `render()` (latex.ts:856).
    fn render(mut self) -> Option<String> {
        let rendered = self.parse_sequence(None);
        if !self.supported || self.position != self.source.len() {
            return None;
        }
        Some(normalize_output(&rendered))
    }

    /// TS `parseSequence(endCharacter?)` (latex.ts:862).
    fn parse_sequence(&mut self, end_character: Option<char>) -> String {
        let mut result = String::new();
        while self.position < self.source.len() {
            let character = self.source[self.position];
            if let Some(end) = end_character {
                if character == end {
                    self.position += 1;
                    return result;
                }
            }

            if character == '}' {
                self.supported = false;
                return result;
            }

            if character == '{' {
                self.position += 1;
                result.push_str(&self.parse_sequence(Some('}')));
                continue;
            }

            if character == '\\' {
                let command = self.parse_command();
                if command == NEGATIVE_SPACE {
                    result = js_trim_end(&result).to_string();
                    if let Some(stripped) = result.strip_suffix(NAMED_OPERATOR_END) {
                        result = stripped.to_string();
                    }
                } else {
                    result.push_str(&command);
                }
                continue;
            }

            if character == '^' || character == '_' {
                self.position += 1;
                result = js_trim_end(&result).to_string();
                let script = self.parse_scripts(character);
                if let Some(stripped) = result.strip_suffix(NAMED_OPERATOR_END) {
                    result = format!("{stripped}{script}{NAMED_OPERATOR_END}");
                } else {
                    result.push_str(&script);
                }
                continue;
            }

            if js_is_whitespace(character) {
                result.push_str(&self.parse_whitespace());
                continue;
            }

            if character == '=' || character == '<' || character == '>' {
                result = format!("{} {character} ", js_trim_end(&result));
                self.position += 1;
                continue;
            }

            if character == '&' {
                self.position += 1;
                continue;
            }

            if character == '~' {
                self.position += 1;
                result.push(' ');
                continue;
            }

            if character == '.' {
                let mut handled = false;
                if let Some(caps) = trailing_layout_marker_pattern()
                    .captures(&result)
                    .expect("latex regex match")
                {
                    let node_index: usize = caps
                        .get(1)
                        .and_then(|g| g.as_str().parse().ok())
                        .unwrap_or(usize::MAX);
                    if let Some(LayoutNode::Matrix { lines, .. }) =
                        self.layout_nodes.get_mut(node_index)
                    {
                        if let Some(last_line) = lines.last_mut() {
                            last_line.push(character);
                        }
                        handled = true;
                    }
                }
                if handled {
                    self.position += 1;
                    continue;
                }
            }

            result.push(character);
            self.position += 1;
        }

        if end_character.is_some() {
            self.supported = false;
        }
        result
    }

    /// TS `parseScripts(initialMarker)` (latex.ts:953).
    fn parse_scripts(&mut self, initial_marker: char) -> String {
        let mut scripts_sub: Option<String> = None;
        let mut scripts_sup: Option<String> = None;
        let mut order: Vec<char> = Vec::new(); // '_' | '^'

        // TS `parse` closure (latex.ts:959):
        // `scripts[kind] = parseRequiredArgument(false)` under scriptDepth++.
        let mut parse = |parser: &mut Self, marker: char| {
            parser.script_depth += 1;
            let value = parser.parse_required_argument(false);
            parser.script_depth -= 1;
            if marker == '_' {
                scripts_sub = Some(value);
            } else {
                scripts_sup = Some(value);
            }
            order.push(marker);
        };

        parse(self, initial_marker);
        let mut next_position = self.position;
        while next_position < self.source.len()
            && js_is_whitespace(self.source[next_position])
        {
            next_position += 1;
        }
        let next_marker = self.char_at(next_position);
        if let Some(next_marker) = next_marker {
            if (next_marker == '^' || next_marker == '_') && next_marker != initial_marker {
                self.position = next_position + 1;
                parse(self, next_marker);
            }
        }

        let sub_unicode = scripts_sub.as_deref().map(format_unicode_script_sub);
        let sup_unicode = scripts_sup.as_deref().map(format_unicode_script_sup);

        // TS `canUseLayout` (latex.ts:986).
        let can_use_layout = ![scripts_sub.as_deref(), scripts_sup.as_deref()]
            .iter()
            .any(|&value| match value {
                Some(value) => {
                    value.contains('/')
                        || (!value.contains(LAYOUT_MARKER_START)
                            && js_code_points(value).len() > 1
                            && !value.chars().any(has_upper_star))
                }
                None => false,
            });
        let needs_layout = self.display
            && can_use_layout
            && (self.script_depth > 0
                || matches!(sub_unicode, Some(None))
                || matches!(sup_unicode, Some(None)));
        if !needs_layout {
            return order
                .iter()
                .map(|&kind| {
                    if kind == '_' {
                        sub_unicode
                            .clone()
                            .flatten()
                            .unwrap_or_else(|| format_script(scripts_sub.as_deref().unwrap_or(""), ScriptKind::Sub))
                    } else {
                        sup_unicode
                            .clone()
                            .flatten()
                            .unwrap_or_else(|| format_script(scripts_sup.as_deref().unwrap_or(""), ScriptKind::Sup))
                    }
                })
                .collect::<String>();
        }

        let index = self.layout_nodes.len();
        self.layout_nodes.push(LayoutNode::Script {
            lower: scripts_sub.as_deref().map(normalize_output),
            upper: scripts_sup.as_deref().map(normalize_output),
        });
        format!("{LAYOUT_MARKER_START}{index}{LAYOUT_MARKER_END}")
    }

    /// TS `parseWhitespace()` (latex.ts:1030).
    fn parse_whitespace(&mut self) -> String {
        while self.position < self.source.len() && js_is_whitespace(self.source[self.position]) {
            self.position += 1;
        }
        " ".to_string()
    }

    /// TS `parseCommand()` (latex.ts:1035).
    fn parse_command(&mut self) -> String {
        self.position += 1;
        if self.position >= self.source.len() {
            self.supported = false;
            return String::new();
        }

        let mut command = String::new();
        let first = self.source[self.position];
        if first == '\n' || first == '\r' {
            self.position += 1;
            if first == '\r' && self.char_at(self.position) == Some('\n') {
                self.position += 1;
            }
            return " ".to_string();
        }
        if first.is_ascii_alphabetic() {
            let start = self.position;
            while self.position < self.source.len()
                && self.source[self.position].is_ascii_alphabetic()
            {
                self.position += 1;
            }
            command = self.source[start..self.position].iter().collect();
        } else {
            command.push(first);
            self.position += 1;
        }

        if command == "\\" {
            return "\n".to_string();
        }
        if SPACING_COMMANDS.contains(&command.as_str()) {
            return " ".to_string();
        }
        if NEGATIVE_SPACING_COMMANDS.contains(&command.as_str()) {
            return NEGATIVE_SPACE.to_string();
        }
        if FONT_SWITCH_COMMANDS.contains(&command.as_str()) {
            while self.position < self.source.len()
                && js_is_whitespace(self.source[self.position])
            {
                self.position += 1;
            }
            return String::new();
        }
        if IGNORED_COMMANDS.contains(&command.as_str()) {
            return String::new();
        }
        if matches!(command.as_str(), "{" | "}" | "$" | "%" | "#" | "_" | "&") {
            return command;
        }
        if command == "|" {
            return "‖".to_string();
        }
        if command == "not" {
            let value = js_trim(&self.parse_required_argument(false)).to_string();
            if let Some(negated) = negated_symbols().get(value.as_str()) {
                return format!(" {negated} ");
            }
            let characters: Vec<char> = js_code_points(&value);
            if characters.is_empty() {
                self.supported = false;
                return String::new();
            }
            let rest: String = characters[1..].iter().collect();
            return format!(" {}\u{338}{rest} ", characters[0]);
        }
        if LIMIT_OPERATORS.contains(&command.as_str()) {
            return self.parse_operator(&command, InlineLowerStyle::Bracket, true, true);
        }

        if let Some(symbol) = symbols().get(command.as_str()) {
            if DISPLAY_LIMIT_SYMBOLS.contains(&command.as_str()) {
                return self.parse_operator(symbol, InlineLowerStyle::Script, true, false);
            }
            return if command == "cdot"
                || command == "times"
                || RELATION_COMMANDS.contains(&command.as_str())
            {
                format!(" {symbol} ")
            } else {
                symbol.to_string()
            };
        }
        if NAMED_OPERATORS.contains(&command.as_str()) {
            return format!("{NAMED_OPERATOR_START}{command}{NAMED_OPERATOR_END}");
        }
        if SIZE_COMMANDS.contains(&command.as_str()) {
            return String::new();
        }
        if command == "left" || command == "middle" || command == "right" {
            if self.char_at(self.position) == Some('.') {
                self.position += 1;
            }
            return String::new();
        }
        if command == "frac" || command == "dfrac" || command == "tfrac" {
            let should_stack = self.display && self.stack_fractions && command != "tfrac";
            let numerator = self.parse_required_argument(!should_stack);
            let denominator = self.parse_required_argument(!should_stack);
            if should_stack {
                let index = self.layout_nodes.len();
                self.layout_nodes.push(LayoutNode::Fraction {
                    numerator: normalize_output(&numerator),
                    denominator: normalize_output(&denominator),
                });
                return format!("{LAYOUT_MARKER_START}{index}{LAYOUT_MARKER_END}");
            }
            return format_fraction(&numerator, &denominator);
        }
        if command == "sqrt" {
            let degree = self.parse_optional_argument().map(|d| js_trim(&d).to_string());
            let value = self.parse_required_argument(true);
            return match degree.as_deref() {
                None | Some("2") => format_root(&value, "√"),
                Some("3") => format_root(&value, "∛"),
                Some("4") => format_root(&value, "∜"),
                Some(degree) => {
                    format!("{}{}", format_script(degree, ScriptKind::Sup), format_root(&value, "√"))
                }
            };
        }
        if command == "boxed" || command == "fbox" {
            return format!("[{}]", js_trim(&self.parse_required_argument(true)));
        }
        if command == "binom" || command == "dbinom" || command == "tbinom" {
            let first = self.parse_required_argument(true);
            let second = self.parse_required_argument(true);
            return format!("({first} choose {second})");
        }
        if let Some(accent) = accents(&command) {
            let value = self.parse_required_argument(true);
            return if js_code_points(&value).len() == 1 {
                format!("{value}{accent}")
            } else {
                format!("{command}({value})")
            };
        }
        if command == "mathbb" {
            let value = self.parse_required_argument(true);
            return js_code_points(&value)
                .into_iter()
                .map(|c| blackboard(c).unwrap_or(c))
                .collect();
        }
        if command == "operatorname" {
            let starred = self.char_at(self.position) == Some('*');
            if starred {
                self.position += 1;
            }
            let operator = js_trim(&normalize_output(&self.parse_required_argument(true))).to_string();
            return self.parse_operator(&operator, InlineLowerStyle::Bracket, starred, true);
        }
        if command == "mod" || command == "bmod" {
            return " mod ".to_string();
        }
        if command == "pmod" || command == "pod" {
            let value = js_trim(&self.parse_required_argument(true)).to_string();
            return if command == "pmod" {
                format!(" (mod {value})")
            } else {
                format!(" ({value})")
            };
        }
        if command == "overset" || command == "stackrel" {
            let upper = self.parse_required_argument(true);
            let value = js_trim(&self.parse_required_argument(true)).to_string();
            return format!("{value}{}", format_script(&upper, ScriptKind::Sup));
        }
        if command == "underset" {
            let lower = self.parse_required_argument(true);
            let value = js_trim(&self.parse_required_argument(true)).to_string();
            return format!("{value}{}", format_script(&lower, ScriptKind::Sub));
        }
        if PLAIN_WRAPPERS.contains(&command.as_str()) {
            let value = self.parse_required_argument(true);
            return if command.starts_with("text") || command == "mbox" {
                value
            } else {
                js_trim(&value).to_string()
            };
        }
        if command == "begin" {
            return self.parse_environment();
        }
        if command == "end" {
            self.supported = false;
            return String::new();
        }

        self.supported = false;
        format!("\\{command}")
    }

    /// TS `parseOperator` (latex.ts:1204).
    fn parse_operator(
        &mut self,
        operator: &str,
        inline_lower_style: InlineLowerStyle,
        display_limits: bool,
        spaced: bool,
    ) -> String {
        let mut use_display_limits = display_limits;
        let mut modifier_position = self.position;
        while modifier_position < self.source.len()
            && matches!(self.source[modifier_position], ' ' | '\t')
        {
            modifier_position += 1;
        }
        let rest: String = self.source[modifier_position..].iter().collect();
        if let Some(caps) = limits_modifier_pattern()
            .captures(&rest)
            .expect("latex regex match")
        {
            use_display_limits = caps.get(1).map(|g| g.as_str()) == Some("limits");
            self.position = modifier_position
                + caps
                    .get(0)
                    .expect("limits modifier group 0")
                    .as_str()
                    .len();
        }

        let mut lower: Option<String> = None;
        let mut upper: Option<String> = None;
        loop {
            let mut script_position = self.position;
            while script_position < self.source.len()
                && matches!(self.source[script_position], ' ' | '\t')
            {
                script_position += 1;
            }
            let kind = self.char_at(script_position);
            if kind != Some('_') && kind != Some('^') {
                break;
            }
            self.position = script_position + 1;
            let value = normalize_output(&self.parse_required_argument(false))
                .replace(' ', "");
            if kind == Some('_') {
                if lower.is_some() {
                    self.supported = false;
                }
                lower = Some(value);
            } else {
                if upper.is_some() {
                    self.supported = false;
                }
                upper = Some(value);
            }
        }

        if self.display && use_display_limits && (lower.is_some() || upper.is_some()) {
            let index = self.layout_nodes.len();
            self.layout_nodes.push(LayoutNode::Operator {
                operator: operator.to_string(),
                lower,
                upper,
            });
            return format!("{LAYOUT_MARKER_START}{index}{LAYOUT_MARKER_END}");
        }

        let mut rendered = operator.to_string();
        if let Some(lower) = &lower {
            match inline_lower_style {
                InlineLowerStyle::Bracket => rendered.push_str(&format!("[{lower}]")),
                InlineLowerStyle::Script => {
                    rendered.push_str(&format_script(lower, ScriptKind::Sub))
                }
            }
        }
        if let Some(upper) = &upper {
            rendered.push_str(&format_script(upper, ScriptKind::Sup));
        }
        if spaced {
            format!(" {rendered} ")
        } else {
            rendered
        }
    }

    /// TS `parseRequiredArgument(stackFractions = true)` (latex.ts:1260).
    fn parse_required_argument(&mut self, stack_fractions: bool) -> String {
        let previous_stack_fractions = self.stack_fractions;
        self.stack_fractions = previous_stack_fractions && stack_fractions;
        let value = self.parse_required_argument_value();
        self.stack_fractions = previous_stack_fractions;
        value
    }

    /// TS `parseRequiredArgumentValue()` (latex.ts:1268).
    fn parse_required_argument_value(&mut self) -> String {
        while self.position < self.source.len()
            && js_is_whitespace(self.source[self.position])
        {
            self.position += 1;
        }
        if self.position >= self.source.len() {
            self.supported = false;
            return String::new();
        }
        match self.source[self.position] {
            '{' => {
                self.position += 1;
                self.parse_sequence(Some('}'))
            }
            '\\' => self.parse_command(),
            c => {
                self.position += 1;
                c.to_string()
            }
        }
    }

    /// TS `parseOptionalArgument()` (latex.ts:1284).
    fn parse_optional_argument(&mut self) -> Option<String> {
        while self.position < self.source.len()
            && matches!(self.source[self.position], ' ' | '\t')
        {
            self.position += 1;
        }
        if self.char_at(self.position) != Some('[') {
            return None;
        }
        let end = self.source[(self.position + 1)..]
            .iter()
            .position(|&c| c == ']')
            .map(|offset| offset + self.position + 1);
        let Some(end) = end else {
            self.supported = false;
            return None;
        };
        let value: String = self.source[(self.position + 1)..end].iter().collect();
        self.position = end + 1;
        Some(self.render_nested(&value, true))
    }

    /// TS `readRawGroup()` (latex.ts:1297).
    fn read_raw_group(&mut self) -> Option<String> {
        while self.position < self.source.len()
            && matches!(self.source[self.position], ' ' | '\t')
        {
            self.position += 1;
        }
        if self.char_at(self.position) != Some('{') {
            self.supported = false;
            return None;
        }

        self.position += 1;
        let start = self.position;
        let mut depth = 1;
        while self.position < self.source.len() {
            let character = self.source[self.position];
            if character == '\\' {
                self.position += 2;
                continue;
            }
            if character == '{' {
                depth += 1;
            }
            if character == '}' {
                depth -= 1;
            }
            if depth == 0 {
                let value: String = self.source[start..self.position].iter().collect();
                self.position += 1;
                return Some(value);
            }
            self.position += 1;
        }
        self.supported = false;
        None
    }

    /// TS `parseEnvironment()` (latex.ts:1347).
    fn parse_environment(&mut self) -> String {
        let environment = self.read_raw_group();
        let Some(environment) = environment.filter(|e| !e.is_empty()) else {
            return String::new();
        };
        let end_marker: Vec<char> = format!("\\end{{{environment}}}")
            .chars()
            .collect();
        let end = index_of_chars(&self.source, &end_marker, self.position);
        let Some(end) = end else {
            self.supported = false;
            return String::new();
        };
        let body: String = self.source[self.position..end].iter().collect();
        self.position = end + end_marker.len();

        if matches!(
            environment.as_str(),
            "equation" | "equation*" | "displaymath"
        ) {
            return js_trim(&self.render_nested(&body, true)).to_string();
        }

        if matches!(
            environment.as_str(),
            "aligned"
                | "align"
                | "align*"
                | "alignedat"
                | "alignat"
                | "alignat*"
                | "gather"
                | "gathered"
                | "multline"
                | "multline*"
                | "split"
        ) {
            let aligned_at =
                matches!(environment.as_str(), "alignedat" | "alignat" | "alignat*");
            let aligned_body = if aligned_at {
                replace_first_literal(&body, brace_prefix_pattern(), "")
            } else {
                body
            };
            let mut rendered_rows: Vec<String> = Vec::new();
            for row in split_environment_rows(&aligned_body) {
                let cells: Vec<&str> = row.split('&').collect();
                let source = if aligned_at {
                    let pair_count = cells.len().div_ceil(2);
                    (0..pair_count)
                        .map(|index| {
                            cells[(index * 2)..((index * 2 + 2).min(cells.len()))].join("")
                        })
                        .collect::<Vec<String>>()
                        .join(" ")
                } else {
                    cells.join("")
                };
                let rendered = js_trim(&self.render_nested(&source, true)).to_string();
                if !rendered.is_empty() {
                    rendered_rows.push(rendered);
                }
            }
            return rendered_rows.join("\n");
        }

        if matches!(environment.as_str(), "cases" | "cases*") {
            return self.render_cases(&body);
        }

        if matches!(
            environment.as_str(),
            "array"
                | "matrix"
                | "smallmatrix"
                | "pmatrix"
                | "bmatrix"
                | "Bmatrix"
                | "vmatrix"
                | "Vmatrix"
        ) {
            let matrix_body = if environment == "array" {
                replace_first_literal(&body, brace_prefix_pattern(), "")
            } else {
                body
            };
            return self.render_matrix(&environment, &matrix_body);
        }

        self.supported = false;
        body
    }

    /// TS `renderCases(body)` (latex.ts:1430).
    fn render_cases(&mut self, body: &str) -> String {
        let mut rows: Vec<Vec<String>> = Vec::new();
        for row in split_environment_rows(body) {
            let mut cells: Vec<String> = Vec::new();
            for cell in row.split('&') {
                cells.push(js_trim(&self.render_nested(cell, false)).to_string());
            }
            if cells.iter().any(|c| !c.is_empty()) {
                rows.push(cells);
            }
        }
        let value_width = rows
            .iter()
            .map(|row| {
                visible_width(&strip_trailing_comma(row.first().map(|s| s.as_str()).unwrap_or("")))
            })
            .max()
            .unwrap_or(0);
        let mut contents: Vec<String> = Vec::new();
        for row in &rows {
            let value = strip_trailing_comma(row.first().map(|s| s.as_str()).unwrap_or(""));
            let condition = row.get(1).cloned().unwrap_or_default();
            if condition.is_empty() {
                contents.push(value.to_string());
                continue;
            }
            let condition_prefix = if condition_prefix_pattern()
                .is_match(&condition)
                .expect("latex regex match")
            {
                " "
            } else {
                " if "
            };
            // TS `PROTECTED_SPACE.repeat(valueWidth - visibleWidth(value))` —
            // the difference is always ≥ 0 because valueWidth is the max over
            // the same computation.
            contents.push(format!(
                "{value}{}{condition_prefix}{condition}",
                PROTECTED_SPACE.repeat(value_width.saturating_sub(visible_width(&value)))
            ));
        }
        if contents.len() <= 1 {
            return if contents.is_empty() {
                String::new()
            } else {
                format!("⎧ {}", contents[0])
            };
        }

        let middle = contents.len() / 2;
        let mut visual_rows: Vec<Option<String>> = contents.into_iter().map(Some).collect();
        if visual_rows.len() % 2 == 0 {
            visual_rows.insert(middle, None);
        }
        let lines: Vec<String> = visual_rows
            .iter()
            .enumerate()
            .map(|(index, content)| {
                let delimiter = if index == 0 {
                    "⎧"
                } else if index == visual_rows.len() - 1 {
                    "⎩"
                } else {
                    "⎨"
                };
                match content {
                    None => delimiter.to_string(),
                    Some(content) => format!("{delimiter} {content}"),
                }
            })
            .collect();
        let index = self.layout_nodes.len();
        self.layout_nodes.push(LayoutNode::Matrix { lines, baseline: middle });
        format!("{LAYOUT_MARKER_START}{index}{LAYOUT_MARKER_END}")
    }

    /// TS `renderMatrix(environment, body)` (latex.ts:1467).
    fn render_matrix(&mut self, environment: &str, body: &str) -> String {
        let mut matrix: Vec<Vec<String>> = Vec::new();
        for row in split_environment_rows(body) {
            let mut cells: Vec<String> = Vec::new();
            for cell in row.split('&') {
                cells.push(js_trim(&self.render_nested(cell, false)).to_string());
            }
            if cells.iter().any(|c| !c.is_empty()) {
                matrix.push(cells);
            }
        }
        let column_count = matrix.iter().map(|row| row.len()).max().unwrap_or(0);
        let column_widths: Vec<usize> = (0..column_count)
            .map(|column| {
                matrix
                    .iter()
                    .map(|row| visible_width(row.get(column).map(|s| s.as_str()).unwrap_or("")))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let rows: Vec<String> = matrix
            .iter()
            .map(|row| {
                (0..column_count)
                    .map(|column| {
                        let cell = row.get(column).map(|s| s.as_str()).unwrap_or("");
                        // TS `Math.max(0, (columnWidths[column] ?? 0) - visibleWidth(cell))`.
                        format!(
                            "{cell}{}",
                            PROTECTED_SPACE
                                .repeat(column_widths[column].saturating_sub(visible_width(cell)))
                        )
                    })
                    .collect::<Vec<String>>()
                    .join(" │ ")
            })
            .collect();

        let lines: Vec<String> = if matches!(environment, "array" | "matrix" | "smallmatrix") {
            rows
        } else {
            let delimiter: [&str; 6] = match environment {
                "pmatrix" => ["⎛", "⎞", "⎜", "⎟", "⎝", "⎠"],
                "bmatrix" => ["⎡", "⎤", "⎢", "⎥", "⎣", "⎦"],
                "Bmatrix" => ["⎧", "⎫", "⎨", "⎬", "⎩", "⎭"],
                "vmatrix" => ["│", "│", "│", "│", "│", "│"],
                "Vmatrix" => ["║", "║", "║", "║", "║", "║"],
                _ => {
                    self.supported = false;
                    return rows.join("\n");
                }
            };
            rows.iter()
                .enumerate()
                .map(|(index, row)| {
                    let left = if index == 0 {
                        delimiter[0]
                    } else if index == rows.len() - 1 {
                        delimiter[4]
                    } else {
                        delimiter[2]
                    };
                    let right = if index == 0 {
                        delimiter[1]
                    } else if index == rows.len() - 1 {
                        delimiter[5]
                    } else {
                        delimiter[3]
                    };
                    format!("{left} {row} {right}")
                })
                .collect()
        };

        if lines.len() <= 1 {
            return lines.into_iter().next().unwrap_or_default();
        }
        let index = self.layout_nodes.len();
        self.layout_nodes.push(LayoutNode::Matrix { lines, baseline: 0 });
        format!("{LAYOUT_MARKER_START}{index}{LAYOUT_MARKER_END}")
    }

    /// TS `renderNested(source, stackFractions = true)` (latex.ts:1470).
    fn render_nested(&mut self, source: &str, stack_fractions: bool) -> String {
        // TS re-parses nested strings by UTF-16 unit too (`new LatexParser(source)`).
        let nested =
            LatexParser::new(js_units(source), self.layout_nodes, self.display && stack_fractions);
        match nested.render() {
            Some(rendered) => rendered,
            None => {
                self.supported = false;
                source.to_string()
            }
        }
    }
}

/// `formatUnicodeScript(value, "sub")` helper used by `parseScripts`.
fn format_unicode_script_sub(value: &str) -> Option<String> {
    format_unicode_script(value, ScriptKind::Sub)
}

/// `formatUnicodeScript(value, "sup")` helper used by `parseScripts`.
fn format_unicode_script_sup(value: &str) -> Option<String> {
    format_unicode_script(value, ScriptKind::Sup)
}

/// TS `splitEnvironmentRows(body)` (latex.ts:1342):
/// `body.split(/\\\\(?:\[[^\]\n]*\])?/)`.
fn split_environment_rows(body: &str) -> Vec<String> {
    row_split_pattern()
        .split(body)
        .map(|s| s.expect("latex regex match").to_string())
        .collect()
}

/// TS `value.replace(/,\s*$/, "")` (renderCases, latex.ts:1437/1439).
fn strip_trailing_comma(value: &str) -> String {
    replace_first_literal(value, trailing_comma_pattern(), "")
}

/// `String.prototype.indexOf` over char slices.
fn index_of_chars(haystack: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(haystack.len()));
    }
    if haystack.len() < needle.len() || from > haystack.len() - needle.len() {
        return None;
    }
    (from..=haystack.len() - needle.len())
        .find(|&i| haystack[i..i + needle.len()] == *needle)
}

// ---------------------------------------------------------------------------
// Public API (TS latex.ts:1479–1506)
// ---------------------------------------------------------------------------

/// TS `RenderLatexOptions` (`{ display?: boolean }`).
#[derive(Debug, Clone, Default)]
pub struct RenderLatexOptions {
    pub display: Option<bool>,
}

/// TS `renderLatex(source, options = {})` → `string | undefined`
/// (latex.ts:1488). Renders a basic LaTeX math expression as
/// terminal-friendly Unicode text; returns `None` for unsupported or
/// malformed syntax.
pub fn render_latex(source: &str, options: &RenderLatexOptions) -> Option<String> {
    let mut layout_nodes: Vec<LayoutNode> = Vec::new();
    let chars: Vec<char> = js_units(source);
    let rendered = {
        let parser = LatexParser::new(chars, &mut layout_nodes, options.display == Some(true));
        parser.render()?
    };
    if layout_nodes.is_empty() {
        return Some(rendered.replace(PROTECTED_SPACE, " "));
    }
    let lines = render_layout(&rendered, &layout_nodes).lines;
    // TS: `Math.min(...lines.filter(line => line.trim())
    //     .map(line => line.length - line.trimStart().length))` — `Infinity`
    // (i.e. slice-everything-to-empty) when no non-blank lines remain.
    let indentation = lines
        .iter()
        .filter(|line| !js_trim(line).is_empty())
        .map(|line| line.chars().take_while(|c| js_is_whitespace(*c)).count())
        .min();
    let joined = lines
        .iter()
        .map(|line| {
            // TS `line.slice(indentation)` — skipping beyond the end yields "".
            let sliced: String = match indentation {
                Some(indentation) => line.chars().skip(indentation).collect(),
                None => String::new(),
            };
            js_trim_end(&sliced).to_string()
        })
        .collect::<Vec<String>>()
        .join("\n");
    Some(js_trim_end(&joined).replace(PROTECTED_SPACE, " "))
}


// ---------------------------------------------------------------------------
// Tests — every expected value below was captured verbatim from the TypeScript
// ground truth: `node /tmp/probe-latex.mjs '<src>' [display]`, which imports
// /tmp/pi-repo/packages/tui/src/latex.ts directly (/tmp/probe-batch.mjs is the
// batch script that produced the probe log). Do not hand-edit expectations;
// re-run the probe instead.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn check(src: &str, display: bool, expected: Option<&str>) {
        let options = if display {
            RenderLatexOptions {
                display: Some(true),
            }
        } else {
            RenderLatexOptions::default()
        };
        assert_eq!(
            render_latex(src, &options).as_deref(),
            expected,
            "mismatch for src={src:?} display={display}"
        );
    }

    #[test]
    fn simple_symbols() {
        check(r#"\alpha"#, false, Some("α"));
        check(r#"\alpha + \beta = \gamma"#, false, Some("α + β = γ"));
    }

    #[test]
    fn scripts() {
        check(r#"x^2"#, false, Some("x²"));
        check(r#"x^{10}"#, false, Some("x¹⁰"));
        check(r#"x^{ab}"#, false, Some("xᵃᵇ"));
        check(r#"x^a"#, false, Some("xᵃ"));
        check(r#"x_1 + x_2 = 2x_3"#, false, Some("x₁ + x₂ = 2x₃"));
        check(r#"x_{i,j}"#, false, Some("x_(i,j)"));
        check(r#"x_{ij}"#, false, Some("xᵢⱼ"));
        check(r#"x^2_i"#, false, Some("x²ᵢ"));
        check(r#"x_i^2"#, false, Some("xᵢ²"));
    }

    #[test]
    fn fractions() {
        check(r#"\frac{a}{b}"#, false, Some("a/b"));
        check(r#"\frac{a+b}{c+d}"#, false, Some("(a+b)/(c+d)"));
        check(r#"\frac{1}{2} + \frac{3}{4}"#, false, Some("1/2 + 3/4"));
        check(r#"\frac{a}{b}"#, true, Some("a\n─\nb"));
        check(r#"\frac{a+b}{c+d}"#, true, Some("a+b\n───\nc+d"));
        check(r#"\dfrac{a}{b}"#, true, Some("a\n─\nb"));
        check(r#"\tfrac{a}{b}"#, true, Some("a/b"));
    }

    #[test]
    fn roots() {
        check(r#"\sqrt{x}"#, false, Some("√x"));
        check(r#"\sqrt{x+1}"#, false, Some("√(x+1)"));
        check(r#"\sqrt[3]{x}"#, false, Some("∛x"));
        check(r#"\sqrt[4]{x+1}"#, false, Some("∜(x+1)"));
        check(r#"\sqrt[n]{x}"#, false, Some("ⁿ√x"));
    }

    #[test]
    fn operators_and_limits() {
        check(r#"\sum_{i=1}^{n} i"#, false, Some("∑ᵢ₌₁ⁿ i"));
        check(r#"\sum_{i=1}^{n} i"#, true, Some(" n\n ∑  i\ni=1"));
        check(r#"\sum\limits_{i=1}^{n} i"#, false, Some("∑ᵢ₌₁ⁿ i"));
        check(r#"\sum\nolimits_{i=1}^{n} i"#, true, Some("∑ᵢ₌₁ⁿ i"));
        check(r#"\lim_{x \to 0} f(x)"#, false, Some("lim[x→0] f(x)"));
        check(r#"\lim_{x \to 0} f(x)"#, true, Some("lim f(x)\nx→0"));
        check(r#"\lim\limits_{x \to 0} f(x)"#, false, Some("lim[x→0] f(x)"));
        check(r#"\int_0^1 x \, dx"#, true, Some("1\n∫ x dx\n0"));
        check(r#"\prod_{i=1}^{n} a_i"#, true, Some(" n\n ∏  aᵢ\ni=1"));
    }

    #[test]
    fn relations_and_negated_symbols() {
        check(r#"a < b"#, false, Some("a < b"));
        check(r#"a \le b"#, false, Some("a ≤ b"));
        check(r#"a \ne b"#, false, Some("a ≠ b"));
        check(r#"x \mapsto y"#, false, Some("x ↦ y"));
        check(r#"a \notin b"#, false, Some("a ∉ b"));
        check(r#"\not="#, false, Some("≠"));
        check(r#"\not<"#, false, Some("≮"));
        check(r#"\not\in"#, false, Some("∉"));
        check(r#"\not{xy}"#, false, Some("x̸y"));
    }

    #[test]
    fn blackboard() {
        check(r#"\mathbb{R}"#, false, Some("ℝ"));
        check(r#"\mathbb{Q}^n"#, false, Some("ℚⁿ"));
        check(r#"\mathbb{abc}"#, false, Some("abc"));
    }

    #[test]
    fn accents() {
        check(r#"\hat{x}"#, false, Some("x̂"));
        check(r#"\hat{xy}"#, false, Some("hat(xy)"));
        check(r#"\vec{a} \cdot \vec{b}"#, false, Some("a⃗ · b⃗"));
        check(r#"\bar{x}"#, false, Some("x̅"));
        check(r#"\tilde{y}"#, false, Some("ỹ"));
        check(r#"\widehat{xy}"#, false, Some("widehat(xy)"));
    }

    #[test]
    fn spacing_commands() {
        check(r#"a \, b"#, false, Some("a b"));
        check(r#"a \quad b"#, false, Some("a b"));
        check(r#"a \! b"#, false, Some("a b"));
        check(r#"a \; b \: b"#, false, Some("a b b"));
    }

    #[test]
    fn matrices_and_cases() {
        check(r#"\begin{pmatrix} a & b \\ c & d \end{pmatrix}"#, false, Some("⎛ a │ b ⎞\n⎝ c │ d ⎠"));
        check(r#"\begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}"#, true, Some("⎛ 1 │ 2 ⎞\n⎝ 3 │ 4 ⎠"));
        check(r#"\begin{bmatrix} 1 & 2 \\ 3 & 4 \end{bmatrix}"#, false, Some("⎡ 1 │ 2 ⎤\n⎣ 3 │ 4 ⎦"));
        check(r#"\begin{cases} x & x > 0 \\ 0 & \text{otherwise} \end{cases}"#, false, Some("⎧ x if x > 0\n⎨\n⎩ 0 otherwise"));
        check(r#"\begin{cases} x & \text{if } x > 0 \\ -x & \text{if } x < 0 \end{cases}"#, true, Some("⎧ x  if x > 0\n⎨\n⎩ -x if x < 0"));
        check(r#"\begin{matrix} 1 & 2 \\ 3 & 4 \end{matrix}"#, false, Some("1 │ 2\n3 │ 4"));
        check(r#"\begin{array}{cc} 1 & 2 \\ 3 & 4 \end{array}"#, false, Some("1 │ 2\n3 │ 4"));
    }

    #[test]
    fn grouping_and_left_right() {
        check(r#"{a + b}"#, false, Some("a + b"));
        check(r#"x^{a + b}"#, false, Some("xᵃ⁺ᵇ"));
        check(r#"\left( x \right)"#, false, Some("( x )"));
        check(r#"\left( x + y \right)^2"#, false, Some("( x + y )²"));
    }

    #[test]
    fn named_operator_wrappers() {
        check(r#"\sin x + \cos y"#, false, Some("sin x + cos y"));
        check(r#"\max_{x} f(x)"#, true, Some("max f(x)\n x"));
        check(r#"\log_2 n"#, false, Some("log₂ n"));
        check(r#"\operatorname{sgn}(x)"#, false, Some("sgn (x)"));
        check(r#"\gcd(a, b)"#, false, Some("gcd(a, b)"));
        check(r#"\text{hello world}"#, false, Some("hello world"));
        check(r#"\mathbf{v}"#, false, Some("v"));
        check(r#"\mathcal{F}"#, false, Some("F"));
    }

    #[test]
    fn linebreaks_and_aligned() {
        check(r#"a \\ b"#, false, Some("a\nb"));
        check(r#"\alpha\\beta"#, false, Some("α\nbeta"));
        check(r#"\begin{aligned} a &= b \\ c &= d \end{aligned}"#, false, Some("a = b\nc = d"));
        check(r#"\begin{align} a &= b \\ c &= d \end{align}"#, false, Some("a = b\nc = d"));
        check(r#"\begin{equation} x = 1 \end{equation}"#, false, Some("x = 1"));
    }

    #[test]
    fn unknown_and_invalid() {
        check(r#"\begin{unknown} x \end{unknown}"#, false, None);
        check(r#"\unknowncmd"#, false, None);
        check(r#"\unknowncmd{x}"#, false, None);
        check(r#"\frac{a}{"#, false, None);
        check(r#"{"#, false, None);
        check(r#"}"#, false, None);
        check(r#"x }"#, false, None);
        check(r#"\end{foo}"#, false, None);
    }

    #[test]
    fn escapes_and_misc_commands() {
        check(r#"a % b"#, false, Some("a % b"));
        check(r#"\% x"#, false, Some("% x"));
        check(r#"\$5"#, false, Some("$5"));
        check(r#"\{ x \}"#, false, Some("{ x }"));
        check(r#"\|"#, false, Some("‖"));
        check(r#"\vert"#, false, Some("|"));
        check(r#"a \bmod b"#, false, Some("a mod b"));
        check(r#"a \pmod{n}"#, false, Some("a (mod n)"));
        check(r#"\overset{!}{=}"#, false, Some("=^!"));
        check(r#"\underset{x}{=}"#, false, Some("=ₓ"));
        check(r#"\binom{n}{k}"#, false, Some("(n choose k)"));
        check(r#"\boxed{x}"#, false, Some("[x]"));
    }

    #[test]
    fn nested_layouts() {
        check(r#"\sqrt{\frac{a}{b}}"#, false, Some("√(a/b)"));
        check(r#"\frac{\sqrt{a}}{b}"#, true, Some("√a\n──\nb"));
        check(r#"x^{y^z}"#, false, Some("x^(yᶻ)"));
        check(r#"x^{y^z}"#, true, Some("  z\n y\nx"));
        check(r#"\sum_{k=0}^{n} \binom{n}{k}"#, true, Some(" n\n ∑  (n choose k)\nk=0"));
        check(r#"\liminf_{n} x"#, true, Some("liminf x\n  n"));
        check(r#"f'(x)"#, false, Some("f'(x)"));
        check(r#"\begin{cases} 1 \\ 2 \\ 3 \end{cases}"#, false, Some("⎧ 1\n⎨ 2\n⎩ 3"));
        check(r#"\begin{cases} 1 \\ 2 \\ 3 \\ 4 \end{cases}"#, false, Some("⎧ 1\n⎨ 2\n⎨\n⎨ 3\n⎩ 4"));
        check(r#"\begin{matrix} 1 \\ 2 \end{matrix}."#, false, Some("1\n2."));
    }

    #[test]
    fn misc_inline() {
        check(r#"e^{i\pi}"#, false, Some("e^(iπ)"));
        check(r#"\mathbb{R}^{n \times m}"#, false, Some("ℝ^(n × m)"));
        check(r#"\sum"#, false, Some("∑"));
        check(r#"\sum"#, true, Some("∑"));
        check(r#"\sum_{i}"#, true, Some("∑\ni"));
        check(r#"  x  "#, false, Some("x"));
        check(r#"x ~ y"#, false, Some("x y"));
        check(r#"a
b"#, false, Some("a b"));
    }

    #[test]
    fn more_matrices_and_linebreaks() {
        check(r#"\\"#, false, Some(""));
        check(r#"x\\ y"#, false, Some("x\ny"));
        check(r#"\begin{smallmatrix} 1 & 2 \\ 3 & 4 \end{smallmatrix}"#, false, Some("1 │ 2\n3 │ 4"));
        check(r#"\begin{Vmatrix} 1 & 2 \\ 3 & 4 \end{Vmatrix}"#, false, Some("║ 1 │ 2 ║\n║ 3 │ 4 ║"));
        check(r#"\begin{vmatrix} 1 & 2 \\ 3 & 4 \end{vmatrix}"#, false, Some("│ 1 │ 2 │\n│ 3 │ 4 │"));
        check(r#"\begin{Bmatrix} 1 & 2 \\ 3 & 4 \end{Bmatrix}"#, false, Some("⎧ 1 │ 2 ⎫\n⎩ 3 │ 4 ⎭"));
    }

    #[test]
    fn display_limit_symbols() {
        check(r#"\int"#, true, Some("∫"));
        check(r#"\iint"#, true, Some("∬"));
        check(r#"\bigcap_{i} A"#, true, Some("⋂ A\ni"));
    }

    #[test]
    fn style_size_and_delimiters() {
        check(r#"\displaystyle\sum_{i=1}^n i"#, false, Some("∑ᵢ₌₁ⁿ i"));
        check(r#"\textstyle x"#, false, Some("x"));
        check(r#"\big( x \big)"#, false, Some("( x )"));
        check(r#"\sf x + \bf y"#, false, Some("x + y"));
        check(r#"\left[x\right["#, false, Some("[x["));
        check(r#"\sqrt[2]{x}"#, false, Some("√x"));
    }

    #[test]
    fn over_under_braces_and_negations() {
        check(r#"\underbrace{x}_{y}"#, false, Some("x_y"));
        check(r#"\overbrace{x}^{y}"#, false, Some("xʸ"));
        check(r#"\not\le"#, false, Some("≰"));
        check(r#"\not\subset"#, false, Some("⊄"));
        check(r#"a \implies b"#, false, Some("a ⇒ b"));
    }

    #[test]
    fn simple_symbols_misc() {
        check(r#"\aleph_0"#, false, Some("ℵ₀"));
        check(r#"\hbar"#, false, Some("ℏ"));
        check(r#"x''"#, false, Some("x''"));
        check(r#"\Re z + \Im w"#, false, Some("ℜ z + ℑ w"));
        check(r#"\emptyset"#, false, Some("∅"));
        check(r#"\infty"#, false, Some("∞"));
        check(r#"\partial_t"#, false, Some("∂ₜ"));
        check(r#"\nabla f"#, false, Some("∇ f"));
        check(r#"\therefore x"#, false, Some("∴ x"));
        check(r#"\because y"#, false, Some("∵ y"));
        check(r#"\to"#, false, Some("→"));
        check(r#"\gets"#, false, Some("←"));
    }

    #[test]
    fn environments_and_advanced() {
        check(r#"\begin{alignedat}{2} a &=1 & b &=2 \end{alignedat}"#, false, Some("a = 1 b = 2"));
        check(r#"\begin{gather} a \\ b \end{gather}"#, false, Some("a\nb"));
        check(r#"\begin{multline} a \\ b \end{multline}"#, false, Some("a\nb"));
        check(r#"\begin{split} a \\ b \end{split}"#, false, Some("a\nb"));
        check(r#"\begin{displaymath} a=b \end{displaymath}"#, false, Some("a = b"));
        check(r#"\substack{a \\ b}"#, false, Some("a\nb"));
        check(r#"\stackrel{\text{def}}{=}"#, false, Some("=ᵈᵉᶠ"));
        check(r#"\sqrt \alpha"#, false, Some("√α"));
        check(r#"\frac 1 2"#, false, Some("1/2"));
        check(r#"\hat x"#, false, Some("x̂"));
        check(r#"\mathbb R"#, false, Some("ℝ"));
        check(r#"\operatorname*{foo}(x)"#, false, Some("foo (x)"));
        check(r#"\operatorname{foo}_{i}"#, true, Some("foo[i]"));
        check(r#"\lim_{n \to \infty} \frac{1}{n} = 0"#, true, Some("     1\nlim  ─ = 0\nn→∞  n"));
        check(r#"\int_{0}^{1} x^2 \, dx = \frac{1}{3}"#, true, Some("1         1\n∫ x² dx = ─\n0         3"));
        check(r#"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}"#, true, Some("    -b ± √(b² - 4ac)\nx = ────────────────\n           2a"));
    }

}


