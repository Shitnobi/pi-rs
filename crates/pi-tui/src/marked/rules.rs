//! Generated rule tables for the marked v18.0.11 port.
//!
//! The static sources below are the FINAL compiled regex sources dumped
//! directly from marked running under Node (default options: gfm=true,
//! breaks=false, pedantic=false — exactly the pi-tui markdownParser config),
//! after marked's internal `k()`/`I()` replace-chain builders ran. That
//! sidesteps porting those builders entirely.
//!
//! Divergence guard: JS (non-`u`, non-`s`) regex semantics differ from
//! fancy-regex in four ways, all handled by `translate_js_pattern` at
//! compile time (see that function for details):
//!   - `\s`/`\S` are Unicode-aware in JS but the exact char set differs
//!     from Rust's `\s` (JS includes U+FEFF, excludes U+0085);
//!   - `.` excludes all four JS line terminators (\n \r U+2028 U+2029);
//!   - `\d`/`\w`/`\D`/`\W`/`\b` are ASCII-only in JS;
//!   - `m`-flag `^`/`$` treat \r U+2028 U+2029 as line terminators too.
//!
//! DO NOT hand-edit the static tables.

use std::sync::OnceLock;

use fancy_regex::{Regex, RegexBuilder};

/// JS `\s` char set (without brackets). JS `\s` is Unicode-aware even
/// without the `u` flag, and its set is NOT the same as Rust's `\s`
/// (White_Space): JS includes U+FEFF (BOM) and excludes U+0085 (NEL).
/// Expanding explicitly avoids both divergences.
pub(crate) const JS_WS_SET: &str =
    r"\t\n\x0b\f\r \u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}";

/// JS `.` (no `s` flag) — any char except the four JS line terminators.
/// Rust's `.` only excludes `\n`.
pub(crate) const JS_DOT_SET: &str = r"[^\n\r\u{2028}\u{2029}]";

/// JS `m`-flag line terminators (for `^`/`$`); Rust's multi-line mode only
/// recognizes `\n`.
const JS_LINE_TERM_SET: &str = r"\n\r\u{2028}\u{2029}";

/// Translate a JS regex source (non-`u`, non-`s` flags) into a
/// fancy-regex-compatible source with exact JS semantics:
///
/// - `\s`/`\S`  -> explicit JS whitespace set (Unicode-aware in JS, and the
///                 set differs from Rust's `\s` on U+FEFF / U+0085).
/// - `.`        -> any char except \n \r U+2028 U+2029 (JS line terminators).
/// - `\d`/`\w`/`\D`/`\W` -> ASCII classes (Unicode-aware in Rust).
/// - `\b`       -> contextually expanded ASCII word boundary (JS `\b` is
///                 ASCII-only; fancy-regex rejects `(?-u:...)` in unicode
///                 mode). If the next char is an ASCII word char,
///                 `\b` == `(?<![0-9A-Za-z_])`; otherwise
///                 `\b` == `(?<=[0-9A-Za-z_])`. (marked only uses `\b_`.)
/// - `[\s\S]`   -> kept verbatim: it matches every char under both engines.
/// - inside a character class `\d`/`\w`/`\s` are expanded to their explicit
///   sets (class unions are position-independent, so this is safe);
///   `\D`/`\W`/`\S` inside a class would need complement splicing and are
///   rejected (marked never uses them there, except the `[\s\S]` idiom).
pub(crate) fn translate_js_pattern(src: &str, multi_line: bool) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut in_class = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let n = chars[i + 1];
            match n {
                'd' | 'D' | 'w' | 'W' => {
                    if in_class {
                        match n {
                            'd' => out.push_str("0-9"),
                            'w' => out.push_str("0-9A-Za-z_"),
                            _ => panic!("unsupported \\{n} inside character class"),
                        }
                    } else {
                        match n {
                            'd' => out.push_str("[0-9]"),
                            'D' => out.push_str("[^0-9]"),
                            'w' => out.push_str("[0-9A-Za-z_]"),
                            'W' => out.push_str("[^0-9A-Za-z_]"),
                            _ => unreachable!(),
                        }
                    }
                }
                's' | 'S' => {
                    if in_class {
                        // `[\s\S]` == "any char" under both engines — keep it.
                        let any_char_idiom = n == 's'
                            && chars.get(i + 2) == Some(&'\\')
                            && chars.get(i + 3) == Some(&'S')
                            && chars.get(i + 4) == Some(&']');
                        if any_char_idiom {
                            out.push_str(r"\s\S");
                            i += 2; // consume both escapes; loop adds the rest
                        } else if n == 's' {
                            out.push_str(JS_WS_SET);
                        } else {
                            panic!("unsupported \\S inside character class");
                        }
                    } else if n == 's' {
                        out.push('[');
                        out.push_str(JS_WS_SET);
                        out.push(']');
                    } else {
                        out.push_str("[^");
                        out.push_str(JS_WS_SET);
                        out.push(']');
                    }
                }
                'b' | 'B' => {
                    if in_class {
                        // JS `\b` inside a class is backspace (U+0008).
                        out.push_str(r"\x08");
                    } else {
                        let next = chars.get(i + 2).copied();
                        let next_is_word = next
                            .map(|c| c.is_ascii_alphanumeric() || c == '_')
                            .unwrap_or(false);
                        if n == 'b' {
                            if next_is_word {
                                out.push_str("(?<![0-9A-Za-z_])");
                            } else {
                                out.push_str("(?<=[0-9A-Za-z_])");
                            }
                        } else if next_is_word {
                            out.push_str("(?<![0-9A-Za-z_])|(?=[0-9A-Za-z_])");
                        } else {
                            out.push_str(
                                "(?<=[0-9A-Za-z_])(?![0-9A-Za-z_])|(?<![0-9A-Za-z_])(?![0-9A-Za-z_])",
                            );
                        }
                    }
                }
                _ => {
                    out.push('\\');
                    out.push(n);
                }
            }
            i += 2;
            continue;
        }
        if c == '[' {
            in_class = true;
            out.push(c);
            i += 1;
            if i < chars.len() && chars[i] == '^' {
                out.push('^');
                i += 1;
            }
            if i < chars.len() && chars[i] == ']' {
                out.push(']');
                i += 1;
            }
            continue;
        }
        if c == ']' && in_class {
            in_class = false;
            out.push(c);
            i += 1;
            continue;
        }
        if !in_class && c == '.' {
            out.push_str(JS_DOT_SET);
            i += 1;
            continue;
        }
        if !in_class && multi_line && (c == '^' || c == '$') {
            // JS m-flag line terminators include \r U+2028 U+2029; Rust's
            // multi-line mode only knows \n. Expand the anchors explicitly
            // and leave multi_line OFF on the builder.
            if c == '^' {
                out.push_str(r"(?:\A|(?<=[");
                out.push_str(JS_LINE_TERM_SET);
                out.push_str("]))");
            } else {
                out.push_str(r"(?:(?=[");
                out.push_str(JS_LINE_TERM_SET);
                out.push_str(r"])|\z)");
            }
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn rx(source: &str, case_insensitive: bool, multi_line: bool) -> Regex {
    build_regex(source, case_insensitive, multi_line)
}

pub struct BlockRules {
    pub blockquote: Regex,
    pub code: Regex,
    pub def: Regex,
    pub fences: Regex,
    pub heading: Regex,
    pub hr: Regex,
    pub html: Regex,
    pub lheading: Regex,
    pub list: Regex,
    pub newline: Regex,
    pub paragraph: Regex,
    pub table: Regex,
    pub text: Regex,
}

pub struct InlineRules {
    pub _backpedal: Regex,
    pub anyPunctuation: Regex,
    pub autolink: Regex,
    pub blockSkip: Regex,
    pub br: Regex,
    pub code: Regex,
    pub del: Regex,
    pub delLDelim: Regex,
    pub delRDelim: Regex,
    pub emStrongLDelim: Regex,
    pub emStrongRDelimAst: Regex,
    pub emStrongRDelimUnd: Regex,
    pub escape: Regex,
    pub link: Regex,
    pub nolink: Regex,
    pub punctuation: Regex,
    pub reflink: Regex,
    pub reflinkSearch: Regex,
    pub tag: Regex,
    pub text: Regex,
    pub url: Regex,
}

pub struct OtherRules {
    pub codeRemoveIndent: Regex,
    pub outputLinkReplace: Regex,
    pub indentCodeCompensation: Regex,
    pub beginningSpace: Regex,
    pub endingHash: Regex,
    pub startingSpaceChar: Regex,
    pub endingSpaceChar: Regex,
    pub nonSpaceChar: Regex,
    pub newLineCharGlobal: Regex,
    pub tabCharGlobal: Regex,
    pub multipleSpaceGlobal: Regex,
    pub blankLine: Regex,
    pub doubleBlankLine: Regex,
    pub blockquoteStart: Regex,
    pub blockquoteSetextReplace: Regex,
    pub blockquoteSetextReplace2: Regex,
    pub listReplaceNesting: Regex,
    pub listIsTask: Regex,
    pub listReplaceTask: Regex,
    pub listTaskCheckbox: Regex,
    pub anyLine: Regex,
    pub hrefBrackets: Regex,
    pub tableDelimiter: Regex,
    pub tableAlignChars: Regex,
    pub tableRowBlankLine: Regex,
    pub tableAlignRight: Regex,
    pub tableAlignCenter: Regex,
    pub tableAlignLeft: Regex,
    pub startATag: Regex,
    pub endATag: Regex,
    pub startPreScriptTag: Regex,
    pub endPreScriptTag: Regex,
    pub startAngleBracket: Regex,
    pub endAngleBracket: Regex,
    pub pedanticHrefTitle: Regex,
    pub unicodeAlphaNumeric: Regex,
    pub escapeTest: Regex,
    pub escapeReplace: Regex,
    pub escapeTestNoEncode: Regex,
    pub escapeReplaceNoEncode: Regex,
    pub caret: Regex,
    pub percentDecode: Regex,
    pub findPipe: Regex,
    pub splitPipe: Regex,
    pub slashPipe: Regex,
    pub carriageReturn: Regex,
    pub spaceLine: Regex,
    pub notSpaceStart: Regex,
    pub endingNewline: Regex,
}

pub struct Rules {
    pub block: BlockRules,
    pub inline: InlineRules,
    pub other: OtherRules,
}

impl Rules {
    /// Build all static rule tables (compiles ~100 regexes).
    pub fn new() -> Rules {
        Rules {
            block: BlockRules {
            blockquote: rx("^( {0,3}> ?(([^\\n]+(?:\\n(?! {0,3}((?:-[\\t ]*){3,}|(?:_[ \\t]*){3,}|(?:\\*[ \\t]*){3,})(?:\\n+|$)| {0,3}#{1,6}(?:\\s|$)| {0,3}>| {0,3}(?:`{3,}(?=[^`\\n]*(?:\\n|$))|~~~)[^\\n]*(?:\\n|$)| {0,3}(?:[*+-]|\\d{1,9}[.)])(?:[ \\t]|\\n|$)|<\\/?(?:address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|meta|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul)(?: +|\\n|\\/?>)|<(?:script|pre|style|textarea|!--)|[ \\t]+\\n)[^\\n]+)*)|[^\\n]*)(?:\\n|$))+", false, false),
            code: rx("^((?: {4}| {0,3}\\t)[^\\n]+(?:\\n(?:[ \\t]*(?:\\n|$))*)?)+", false, false),
            def: rx("^ {0,3}\\[((?!\\s*\\])(?:\\\\[\\s\\S]|[^\\[\\]\\\\])+)\\]: *(?:\\n[ \\t]*)?([^<\\s][^\\s]*|<.*?>)(?:(?: +(?:\\n[ \\t]*)?| *\\n[ \\t]*)((?:\"(?:\\\\\"?|[^\"\\\\])*\"|'[^'\\n]*(?:\\n[^'\\n]+)*\\n?'|\\([^()]*\\))))? *(?:\\n+|$)", false, false),
            fences: rx("^ {0,3}(`{3,}(?=[^`\\n]*(?:\\n|$))|~{3,})([^\\n]*)(?:\\n|$)(?:|([\\s\\S]*?)(?:\\n|$))(?: {0,3}\\1[~`]* *(?=\\n|$)|$)", false, false),
            heading: rx("^ {0,3}(#{1,6})(?=\\s|$)(.*)(?:\\n+|$)", false, false),
            hr: rx("^ {0,3}((?:-[\\t ]*){3,}|(?:_[ \\t]*){3,}|(?:\\*[ \\t]*){3,})(?:\\n+|$)", false, false),
            html: rx("^ {0,3}(?:<(script|pre|style|textarea)[\\s>][\\s\\S]*?(?:<\\/\\1>[^\\n]*\\n*|$)|<!--(?:-?>|[\\s\\S]*?(?:-->|$))[^\\n]*(\\n+|$)|<\\?[\\s\\S]*?(?:\\?>[^\\n]*\\n*|$)|<![A-Z][\\s\\S]*?(?:>[^\\n]*\\n*|$)|<!\\[CDATA\\[[\\s\\S]*?(?:\\]\\]>[^\\n]*\\n*|$)|<\\/?(address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|meta|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul)(?: +|\\n|\\/?>)[\\s\\S]*?(?:(?:\\n[ \t]*)+\\n|$)|<(?!script|pre|style|textarea)([a-z][\\w-]*)(?: +[a-zA-Z:_][\\w.:-]*(?: *= *\"[^\"\\n]*\"| *= *'[^'\\n]*'| *= *[^\\s\"'=<>`]+)?)*? *\\/?>(?=[ \\t]*(?:\\n|$))[\\s\\S]*?(?:(?:\\n[ \t]*)+\\n|$)|<\\/(?!script|pre|style|textarea)[a-z][\\w-]*\\s*>(?=[ \\t]*(?:\\n|$))[\\s\\S]*?(?:(?:\\n[ \t]*)+\\n|$))", true, false),
            lheading: rx("^(?! {0,3}(?:[*+-]|\\d{1,9}[.)]) |(?: {4}| {0,3}\\t)| {0,3}(?:`{3,}|~{3,})| {0,3}>| {0,3}#{1,6}(?:\\s|$)| {0,3}<[^\\n>]+>\\n| {0,3}\\|?(?:[:\\- ]*\\|)+[\\:\\- ]*\\n)((?:.|\\n(?!\\s*?\\n| {0,3}(?:[*+-]|\\d{1,9}[.)]) |(?: {4}| {0,3}\\t)| {0,3}(?:`{3,}|~{3,})| {0,3}>| {0,3}#{1,6}(?:\\s|$)| {0,3}<[^\\n>]+>\\n| {0,3}\\|?(?:[:\\- ]*\\|)+[\\:\\- ]*\\n))+?)\\n {0,3}(=+|-+) *(?:\\n+|$)", false, false),
            list: rx("^( {0,3}(?:[*+-]|\\d{1,9}[.)]))([ \\t][^\\n]*?)?(?:\\n|$)", false, false),
            newline: rx("^(?:[ \\t]*(?:\\n|$))+", false, false),
            paragraph: rx("^([^\\n]+(?:\\n(?! {0,3}((?:-[\\t ]*){3,}|(?:_[ \\t]*){3,}|(?:\\*[ \\t]*){3,})(?:\\n+|$)| {0,3}#{1,6}(?:\\s|$)| {0,3}>| {0,3}(?:`{3,}(?=[^`\\n]*(?:\\n|$))|~~~)[^\\n]*(?:\\n|$)| {0,3}(?:[*+-]|1[.)])[ \\t]+[^ \\t\\n]|<\\/?(?:address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|meta|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul)(?: +|\\n|\\/?>)|<(?:script|pre|style|textarea|!--)| *([^\\n ].*)\\n {0,3}((?:\\| *)?:?-+:? *(?:\\| *:?-+:? *)*(?:\\| *)?)(?:\\n((?:(?! *\\n| {0,3}((?:-[\\t ]*){3,}|(?:_[ \\t]*){3,}|(?:\\*[ \\t]*){3,})(?:\\n+|$)| {0,3}#{1,6}(?:\\s|$)| {0,3}>|(?: {4}| {0,3}\t)[^\\n]| {0,3}(?:`{3,}(?=[^`\\n]*(?:\\n|$))|~~~)[^\\n]*(?:\\n|$)| {0,3}(?:[*+-]|1[.)])[ \\t]|<\\/?(?:address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|meta|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul)(?: +|\\n|\\/?>)|<(?:script|pre|style|textarea|!--)).*(?:\\n|$))*)\\n*|$)|[ \\t]+\\n)[^\\n]+)*)", false, false),
            table: rx("^ *([^\\n ].*)\\n {0,3}((?:\\| *)?:?-+:? *(?:\\| *:?-+:? *)*(?:\\| *)?)(?:\\n((?:(?! *\\n| {0,3}((?:-[\\t ]*){3,}|(?:_[ \\t]*){3,}|(?:\\*[ \\t]*){3,})(?:\\n+|$)| {0,3}#{1,6}(?:\\s|$)| {0,3}>|(?: {4}| {0,3}\t)[^\\n]| {0,3}(?:`{3,}(?=[^`\\n]*(?:\\n|$))|~~~)[^\\n]*(?:\\n|$)| {0,3}(?:[*+-]|1[.)])[ \\t]|<\\/?(?:address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|meta|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul)(?: +|\\n|\\/?>)|<(?:script|pre|style|textarea|!--)).*(?:\\n|$))*)\\n*|$)", false, false),
            text: rx("^[^\\n]+", false, false),
            },
            inline: InlineRules {
            _backpedal: rx("(?:[^?!.,:;*_'\"~()&]+|\\([^)]*\\)|&(?![a-zA-Z0-9]+;$)|[?!.,:;*_'\"~)]+(?!$))+", false, false),
            anyPunctuation: rx("\\\\([\\p{P}\\p{S}])", false, false),
            autolink: rx("^<([a-zA-Z][a-zA-Z0-9+.-]{1,31}:[^\\s\\x00-\\x1f<>]*|[a-zA-Z0-9.!#$%&'*+/=?_`{|}~-]+(@)[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)+(?![-_]))>", false, false),
            blockSkip: rx("\\[(?:[^\\[\\]`]|(?<a>`+)[^`]+\\k<a>(?!`))*?\\]\\((?:\\\\[\\s\\S]|[^\\\\\\(\\)]|\\((?:\\\\[\\s\\S]|[^\\\\\\(\\)])*\\))*\\)|(?<!`)()(?<b>`+)[^`]+\\k<b>(?!`)|<(?! )[^<>]*?>", false, false),
            br: rx("^( {2,}|\\\\)\\n(?!\\s*$)", false, false),
            code: rx("^(`+)([^`]|[^`][\\s\\S]*?[^`])\\1(?!`)", false, false),
            del: rx("^(~~?)(?=[^\\s~])((?:\\\\[\\s\\S]|[^\\\\])*?(?:\\\\[\\s\\S]|[^\\s~\\\\]))\\1(?=[^~]|$)", false, false),
            delLDelim: rx("^~~?(?:((?!~)[\\p{P}\\p{S}])|[^\\s~])", false, false),
            delRDelim: rx("^[^~]+(?=[^~])|(?!~)[\\p{P}\\p{S}](~~?)(?=[\\s]|$)|[^\\s\\p{P}\\p{S}](~~?)(?!~)(?=[\\s\\p{P}\\p{S}]|$)|(?!~)[\\s\\p{P}\\p{S}](~~?)(?=[^\\s\\p{P}\\p{S}])|[\\s](~~?)(?!~)(?=[\\p{P}\\p{S}])|(?!~)[\\p{P}\\p{S}](~~?)(?!~)(?=[\\p{P}\\p{S}])|[^\\s\\p{P}\\p{S}](~~?)(?=[^\\s\\p{P}\\p{S}])", false, false),
            emStrongLDelim: rx("^(?:\\*+(?:((?!\\*)(?!~)[\\p{P}\\p{S}])|([^\\s*]))?)|^_+(?:((?!_)(?!~)[\\p{P}\\p{S}])|([^\\s_]))?", false, false),
            emStrongRDelimAst: rx("^[^_*]*?__[^_*]*?\\*[^_*]*?(?=__)|[^*]+(?=[^*])|(?!\\*)(?!~)[\\p{P}\\p{S}](\\*+)(?=[\\s]|$)|(?:[^\\s\\p{P}\\p{S}]|~)(\\*+)(?!\\*)(?=(?!~)[\\s\\p{P}\\p{S}]|$)|(?!\\*)(?!~)[\\s\\p{P}\\p{S}](\\*+)(?=(?:[^\\s\\p{P}\\p{S}]|~))|[\\s](\\*+)(?!\\*)(?=(?!~)[\\p{P}\\p{S}])|(?!\\*)(?!~)[\\p{P}\\p{S}](\\*+)(?!\\*)(?=(?!~)[\\p{P}\\p{S}])|(?:[^\\s\\p{P}\\p{S}]|~)(\\*+)(?=(?:[^\\s\\p{P}\\p{S}]|~))", false, false),
            emStrongRDelimUnd: rx("^[^_*]*?\\*\\*[^_*]*?_[^_*]*?(?=\\*\\*)|[^_]+(?=[^_])|(?!_)[\\p{P}\\p{S}](_+)(?=[\\s]|$)|[^\\s\\p{P}\\p{S}](_+)(?!_)(?=[\\s\\p{P}\\p{S}]|$)|(?!_)[\\s\\p{P}\\p{S}](_+)(?=[^\\s\\p{P}\\p{S}])|[\\s](_+)(?!_)(?=[\\p{P}\\p{S}])|(?!_)[\\p{P}\\p{S}](_+)(?!_)(?=[\\p{P}\\p{S}])", false, false),
            escape: rx("^\\\\([!\"#$%&'()*+,\\-./:;<=>?@\\[\\]\\\\^_`{|}~])", false, false),
            link: rx("^!?\\[((?:\\[(?:\\\\[\\s\\S]|[^\\[\\]\\\\])*\\]|\\\\[\\s\\S]|`+(?!`)[^`]*?`+(?!`)|``+(?=\\])|[^\\[\\]\\\\`])*?)\\]\\(\\s*(<(?:\\\\.|[^\\n<>\\\\])+>|[^ \\t\\n\\x00-\\x1f]+|(?=\\)))(?:(?:[ \\t]+(?:\\n[ \\t]*)?|\\n[ \\t]*)(\"(?:\\\\\"?|[^\"\\\\])*\"|'(?:\\\\'?|[^'\\\\])*'|\\((?:\\\\\\)?|[^)\\\\])*\\)))?\\s*\\)", false, false),
            nolink: rx("^!?\\[((?!\\s*\\])(?:\\\\[\\s\\S]|[^\\[\\]\\\\])+)\\](?:\\[\\])?", false, false),
            punctuation: rx("^((?![*_])[\\s\\p{P}\\p{S}])", false, false),
            reflink: rx("^!?\\[((?:\\[(?:\\\\[\\s\\S]|[^\\[\\]\\\\])*\\]|\\\\[\\s\\S]|`+(?!`)[^`]*?`+(?!`)|``+(?=\\])|[^\\[\\]\\\\`])*?)\\]\\[((?!\\s*\\])(?:\\\\[\\s\\S]|[^\\[\\]\\\\])+)\\]", false, false),
            reflinkSearch: rx("!?\\[((?:\\[(?:\\\\[\\s\\S]|[^\\[\\]\\\\])*\\]|\\\\[\\s\\S]|`+(?!`)[^`]*?`+(?!`)|``+(?=\\])|[^\\[\\]\\\\`])*?)\\]\\[((?!\\s*\\])(?:\\\\[\\s\\S]|[^\\[\\]\\\\])+)\\]|!?\\[((?!\\s*\\])(?:\\\\[\\s\\S]|[^\\[\\]\\\\])+)\\](?:\\[\\])?(?!\\()", false, false),
            tag: rx("^<!--(?:-?>|[\\s\\S]*?-->)|^<\\/[a-zA-Z][\\w:-]*\\s*>|^<[a-zA-Z][\\w-]*(?:\\s+[a-zA-Z:_][\\w.:-]*(?:\\s*=\\s*\"[^\"]*\"|\\s*=\\s*'[^']*'|\\s*=\\s*[^\\s\"'=<>`]+)?)*?\\s*\\/?>|^<\\?[\\s\\S]*?\\?>|^<![a-zA-Z]+\\s[\\s\\S]*?>|^<!\\[CDATA\\[[\\s\\S]*?\\]\\]>", false, false),
            text: rx("^(`+|~+|[^`~])(?:(?=[`~])|(?= {2,}\\n)|(?=[a-zA-Z0-9.!#$%&'*+\\/=?_`{\\|}~-]+@)|[\\s\\S]*?(?:(?=[\\\\<!\\[`*~_]|\\b_|[hH][tT][tT][pP][sS]?|[fF][tT][pP]:\\/\\/|www\\.|$)|[^ ](?= {2,}\\n)|[^a-zA-Z0-9.!#$%&'*+\\/=?_`{\\|}~-](?=[a-zA-Z0-9.!#$%&'*+\\/=?_`{\\|}~-]+@)))", false, false),
            url: rx("^((?:[hH][tT][tT][pP][sS]?|[fF][tT][pP]):\\/\\/|www\\.)(?:[a-zA-Z0-9\\-]+\\.?)+[^\\s<]*|^[A-Za-z0-9._+-]+(@)[a-zA-Z0-9-_]+(?:\\.[a-zA-Z0-9-_]*[a-zA-Z0-9])+(?![-_])", false, false),
            },
            other: OtherRules {
            codeRemoveIndent: rx("^(?: {1,4}| {0,3}\\t)", false, true),
            outputLinkReplace: rx("\\\\([\\[\\]])", false, false),
            indentCodeCompensation: rx("^(\\s+)(?:```)", false, false),
            beginningSpace: rx("^\\s+", false, false),
            endingHash: rx("#$", false, false),
            startingSpaceChar: rx("^ ", false, false),
            endingSpaceChar: rx(" $", false, false),
            nonSpaceChar: rx("[^ ]", false, false),
            newLineCharGlobal: rx("\\n", false, false),
            tabCharGlobal: rx("\\t", false, false),
            multipleSpaceGlobal: rx("\\s+", false, false),
            blankLine: rx("^[ \\t]*$", false, false),
            doubleBlankLine: rx("\\n[ \\t]*\\n[ \\t]*$", false, false),
            blockquoteStart: rx("^ {0,3}>", false, false),
            blockquoteSetextReplace: rx("\\n {0,3}((?:=+|-+) *)(?=\\n|$)", false, false),
            blockquoteSetextReplace2: rx("^ {0,3}>[ \\t]?", false, true),
            listReplaceNesting: rx("^ {1,4}(?=( {4})*[^ ])", false, false),
            listIsTask: rx("^\\[[ xX]\\] +\\S", false, false),
            listReplaceTask: rx("^\\[[ xX]\\] +", false, false),
            listTaskCheckbox: rx("\\[[ xX]\\]", false, false),
            anyLine: rx("\\n.*\\n", false, false),
            hrefBrackets: rx("^<(.*)>$", false, false),
            tableDelimiter: rx("[:|]", false, false),
            tableAlignChars: rx("^\\||\\| *$", false, false),
            tableRowBlankLine: rx("\\n[ \\t]*$", false, false),
            tableAlignRight: rx("^ *-+: *$", false, false),
            tableAlignCenter: rx("^ *:-+: *$", false, false),
            tableAlignLeft: rx("^ *:-+ *$", false, false),
            startATag: rx("^<a ", true, false),
            endATag: rx("^<\\/a>", true, false),
            startPreScriptTag: rx("^<(pre|code|kbd|script)(\\s|>)", true, false),
            endPreScriptTag: rx("^<\\/(pre|code|kbd|script)(\\s|>)", true, false),
            startAngleBracket: rx("^<", false, false),
            endAngleBracket: rx(">$", false, false),
            pedanticHrefTitle: rx("^([^'\"]*[^\\s])\\s+(['\"])(.*)\\2", false, false),
            unicodeAlphaNumeric: rx("[\\p{L}\\p{N}]", false, false),
            escapeTest: rx("[&<>\"']", false, false),
            escapeReplace: rx("[&<>\"']", false, false),
            escapeTestNoEncode: rx("[<>\"']|&(?!(#\\d{1,7}|#[Xx][a-fA-F0-9]{1,6}|\\w+);)", false, false),
            escapeReplaceNoEncode: rx("[<>\"']|&(?!(#\\d{1,7}|#[Xx][a-fA-F0-9]{1,6}|\\w+);)", false, false),
            caret: rx("(^|[^\\[])\\^", false, false),
            percentDecode: rx("%25", false, false),
            findPipe: rx("\\|", false, false),
            splitPipe: rx(" \\|", false, false),
            slashPipe: rx("\\\\\\|", false, false),
            carriageReturn: rx("\\r\\n|\\r", false, false),
            spaceLine: rx("^ +$", false, true),
            notSpaceStart: rx("^\\S*", false, false),
            endingNewline: rx("\\n$", false, false),
            },
        }
    }

    /// marked's `other.listItemRegex(marker)` — marker is an already-escaped
    /// regex fragment (e.g. `*`, `+`, `-`, or `d{1,9})`) built by the list
    /// tokenizer; the fragment follows JS regex syntax.
    pub fn list_item_regex(&self, marker: &str) -> Regex {
        let pattern = format!(r"^( {{0,3}}{})((?:[\t ][^\n]*)?(?:\n|$))", marker);
        build_dynamic(&pattern, false, false)
    }

    /// marked's `other.nextBulletRegex(cut)` — internal n is clamped to 0..=3.
    pub fn next_bullet_regex(&self, cut: usize) -> Regex {
        let n = cut.saturating_sub(1).min(3);
        let pattern =
            format!(r"^ {{0,{n}}}(?:[*+-]|\d{{1,9}}[.)])((?:[ \t][^\n]*)?(?:\n|$))");
        build_dynamic(&pattern, false, false)
    }

    pub fn hr_regex(&self, cut: usize) -> Regex {
        let n = cut.saturating_sub(1).min(3);
        let pattern =
            format!(r"^ {{0,{n}}}((?:- *){{3,}}|(?:_ *){{3,}}|(?:\* *){{3,}})(?:\n+|$)");
        build_dynamic(&pattern, false, false)
    }

    pub fn fences_begin_regex(&self, cut: usize) -> Regex {
        let n = cut.saturating_sub(1).min(3);
        let pattern = format!(r"^ {{0,{n}}}(?:```|~~~)");
        build_dynamic(&pattern, false, false)
    }

    pub fn heading_begin_regex(&self, cut: usize) -> Regex {
        let n = cut.saturating_sub(1).min(3);
        let pattern = format!(r"^ {{0,{n}}}#");
        build_dynamic(&pattern, false, false)
    }

    pub fn html_begin_regex(&self, cut: usize) -> Regex {
        let n = cut.saturating_sub(1).min(3);
        let pattern = format!(r"^ {{0,{n}}}<(?:[a-z].*>|!--)");
        build_dynamic(&pattern, true, false)
    }

    pub fn blockquote_begin_regex(&self, cut: usize) -> Regex {
        let n = cut.saturating_sub(1).min(3);
        let pattern = format!(r"^ {{0,{n}}}>");
        build_dynamic(&pattern, false, false)
    }
}

fn build_dynamic(pattern: &str, case_insensitive: bool, multi_line: bool) -> Regex {
    build_regex(pattern, case_insensitive, multi_line)
}

fn build_regex(pattern: &str, case_insensitive: bool, multi_line: bool) -> Regex {
    let translated = translate_js_pattern(pattern, multi_line);
    RegexBuilder::new(&translated)
        .case_insensitive(case_insensitive)
        // JS m-flag anchors (incl. \r/U+2028/U+2029 terminators) are expanded
        // explicitly by the translator; multi_line stays off.
        .multi_line(false)
        .build()
        .unwrap_or_else(|e| panic!("marked rule failed to compile: {pattern} ({e})"))
}

static RULES: OnceLock<Rules> = OnceLock::new();

/// The process-wide rule tables (gfm=true config shared with the TS side).
pub fn rules() -> &'static Rules {
    RULES.get_or_init(Rules::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_static_rules_compile() {
        let _ = rules();
    }

    #[test]
    fn dynamic_rules_match_bullets() {
        let r = rules();
        assert!(r.next_bullet_regex(2).is_match("* item\n").unwrap());
        assert!(r.hr_regex(2).is_match("---\n").unwrap());
        assert!(r.fences_begin_regex(3).is_match("  ```").unwrap());
        assert!(r.list_item_regex(r"\*").is_match("* hello\n").unwrap());
    }

    #[test]
    fn ascii_classes_stay_ascii() {
        // JS \d must not match an Arabic-Indic digit.
        let re = Regex::new(&translate_js_pattern(r"\d", false)).unwrap();
        assert!(re.is_match("5").unwrap());
        assert!(!re.is_match("\u{0663}").unwrap());
        // JS \w must not match a non-ASCII letter.
        let re = Regex::new(&translate_js_pattern(r"\w", false)).unwrap();
        assert!(!re.is_match("\u{00e9}").unwrap());
        // ... including inside character classes.
        let re = Regex::new(&translate_js_pattern(r"[\w:-]", false)).unwrap();
        assert!(!re.is_match("\u{00e9}").unwrap());
    }

    #[test]
    fn js_dot_excludes_all_line_terminators() {
        let re = Regex::new(&translate_js_pattern(r"a.b", false)).unwrap();
        assert!(re.is_match("axb").unwrap());
        for term in ["\n", "\r", "\u{2028}", "\u{2029}"] {
            assert!(!re.is_match(&format!("a{term}b")).unwrap(), "dot matched {term:?}");
        }
    }

    #[test]
    fn js_s_is_unicode_aware_js_set() {
        // U+00A0 and U+FEFF are JS whitespace but not Rust \s whitespace.
        let re = Regex::new(&translate_js_pattern(r"\s", false)).unwrap();
        assert!(re.is_match("\u{feff}").unwrap());
        assert!(re.is_match("\u{a0}").unwrap());
        // U+0085 (NEL) is Rust \s but NOT JS \s.
        assert!(!re.is_match("\u{85}").unwrap());
        // Same set inside a (negated) class.
        let re = Regex::new(&translate_js_pattern(r"[^\s]", false)).unwrap();
        assert!(!re.is_match("\u{a0}").unwrap());
        assert!(re.is_match("\u{85}").unwrap());
    }

    #[test]
    fn any_char_idiom_matches_everything() {
        let re = Regex::new(&translate_js_pattern(r"a[\s\S]b", false)).unwrap();
        for term in ["\n", "\r", "\u{2028}", "\u{2029}", "\u{85}", "\u{feff}", "x"] {
            assert!(
                re.is_match(&format!("a{term}b")).unwrap(),
                "[\\s\\S] failed on {term:?}"
            );
        }
    }

    #[test]
    fn multiline_anchors_use_js_line_terminators() {
        let re = Regex::new(&translate_js_pattern(r"^x$", true)).unwrap();
        assert!(re.is_match("a\nx\nb").unwrap());
        // \r / U+2028 / U+2029 are line terminators in JS m-mode.
        assert!(re.is_match("a\rx\rb").unwrap());
        assert!(re.is_match("a\u{2028}x\u{2028}b").unwrap());
        assert!(re.is_match("a\u{2029}x\u{2029}b").unwrap());
    }
}
