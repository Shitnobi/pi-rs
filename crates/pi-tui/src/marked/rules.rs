//! Generated rule tables for the marked v18.0.11 port.
//!
//! The static sources below are the FINAL compiled regex sources dumped
//! directly from marked running under Node (default options: gfm=true,
//! breaks=false, pedantic=false — exactly the pi-tui markdownParser config),
//! after marked's internal `k()`/`I()` replace-chain builders ran. That
//! sidesteps porting those builders entirely.
//!
//! Divergence guard: JS regex \d/\w/\s/\b (without the `u` flag) are
//! ASCII-only, while Rust's are Unicode-aware by default. Sources are
//! translated to explicit ASCII classes when the escape appears OUTSIDE a
//! character class ([0-9], [0-9A-Za-z_], [ \t\n\r\f\x0b], (?-u:\b)).
//! Inside classes the escapes are kept as-is: marked only uses them in
//! [\s\S]-style unions, which are equivalent under either semantics.
//!
//! DO NOT hand-edit the static tables.

use std::sync::OnceLock;

use fancy_regex::{Regex, RegexBuilder};

/// Translate a JS (non-`u`-flag) regex source into a Rust-compatible source
/// with ASCII \d/\w/\s/\b semantics outside character classes.
pub(crate) fn js_pattern_to_ascii(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut in_class = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let n = chars[i + 1];
            if in_class {
                out.push('\\');
                out.push(n);
            } else {
                match n {
                    'd' => out.push_str("[0-9]"),
                    'D' => out.push_str("[^0-9]"),
                    'w' => out.push_str("[0-9A-Za-z_]"),
                    'W' => out.push_str("[^0-9A-Za-z_]"),
                    's' => out.push_str("[ \\t\\n\\r\\f\\x0b]"),
                    'S' => out.push_str("[^ \\t\\n\\r\\f\\x0b]"),
                    // fancy-regex rejects `(?-u:...)` under unicode mode,
                    // so JS \b is expanded contextually: if the next char is
                    // an ASCII word char, \b == (?<![0-9A-Za-z_]); otherwise
                    // \b == (?<=[0-9A-Za-z_]). (marked only uses `\b_`.)
                    'b' | 'B' => {
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
                    _ => {
                        out.push('\\');
                        out.push(n);
                    }
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
        out.push(c);
        i += 1;
    }
    out
}

fn rx(source: &str, case_insensitive: bool, multi_line: bool) -> Regex {
    RegexBuilder::new(source)
        .case_insensitive(case_insensitive)
        .multi_line(multi_line)
        .build()
        .unwrap_or_else(|e| panic!("static marked rule failed to compile: {source} ({e})"))
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
            blockquote: rx("^( {0,3}> ?(([^\\n]+(?:\\n(?! {0,3}((?:-[\\t ]*){3,}|(?:_[ \\t]*){3,}|(?:\\*[ \\t]*){3,})(?:\\n+|$)| {0,3}#{1,6}(?:[ \\t\\n\\r\\f\\x0b]|$)| {0,3}>| {0,3}(?:`{3,}(?=[^`\\n]*(?:\\n|$))|~~~)[^\\n]*(?:\\n|$)| {0,3}(?:[*+-]|[0-9]{1,9}[.)])(?:[ \\t]|\\n|$)|<\\/?(?:address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|meta|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul)(?: +|\\n|\\/?>)|<(?:script|pre|style|textarea|!--)|[ \\t]+\\n)[^\\n]+)*)|[^\\n]*)(?:\\n|$))+", false, false),
            code: rx("^((?: {4}| {0,3}\\t)[^\\n]+(?:\\n(?:[ \\t]*(?:\\n|$))*)?)+", false, false),
            def: rx("^ {0,3}\\[((?![ \\t\\n\\r\\f\\x0b]*\\])(?:\\\\[\\s\\S]|[^\\[\\]\\\\])+)\\]: *(?:\\n[ \\t]*)?([^<\\s][^\\s]*|<.*?>)(?:(?: +(?:\\n[ \\t]*)?| *\\n[ \\t]*)((?:\"(?:\\\\\"?|[^\"\\\\])*\"|'[^'\\n]*(?:\\n[^'\\n]+)*\\n?'|\\([^()]*\\))))? *(?:\\n+|$)", false, false),
            fences: rx("^ {0,3}(`{3,}(?=[^`\\n]*(?:\\n|$))|~{3,})([^\\n]*)(?:\\n|$)(?:|([\\s\\S]*?)(?:\\n|$))(?: {0,3}\\1[~`]* *(?=\\n|$)|$)", false, false),
            heading: rx("^ {0,3}(#{1,6})(?=[ \\t\\n\\r\\f\\x0b]|$)(.*)(?:\\n+|$)", false, false),
            hr: rx("^ {0,3}((?:-[\\t ]*){3,}|(?:_[ \\t]*){3,}|(?:\\*[ \\t]*){3,})(?:\\n+|$)", false, false),
            html: rx("^ {0,3}(?:<(script|pre|style|textarea)[\\s>][\\s\\S]*?(?:<\\/\\1>[^\\n]*\\n*|$)|<!--(?:-?>|[\\s\\S]*?(?:-->|$))[^\\n]*(\\n+|$)|<\\?[\\s\\S]*?(?:\\?>[^\\n]*\\n*|$)|<![A-Z][\\s\\S]*?(?:>[^\\n]*\\n*|$)|<!\\[CDATA\\[[\\s\\S]*?(?:\\]\\]>[^\\n]*\\n*|$)|<\\/?(address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|meta|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul)(?: +|\\n|\\/?>)[\\s\\S]*?(?:(?:\\n[ \t]*)+\\n|$)|<(?!script|pre|style|textarea)([a-z][\\w-]*)(?: +[a-zA-Z:_][\\w.:-]*(?: *= *\"[^\"\\n]*\"| *= *'[^'\\n]*'| *= *[^\\s\"'=<>`]+)?)*? *\\/?>(?=[ \\t]*(?:\\n|$))[\\s\\S]*?(?:(?:\\n[ \t]*)+\\n|$)|<\\/(?!script|pre|style|textarea)[a-z][\\w-]*[ \\t\\n\\r\\f\\x0b]*>(?=[ \\t]*(?:\\n|$))[\\s\\S]*?(?:(?:\\n[ \t]*)+\\n|$))", true, false),
            lheading: rx("^(?! {0,3}(?:[*+-]|[0-9]{1,9}[.)]) |(?: {4}| {0,3}\\t)| {0,3}(?:`{3,}|~{3,})| {0,3}>| {0,3}#{1,6}(?:[ \\t\\n\\r\\f\\x0b]|$)| {0,3}<[^\\n>]+>\\n| {0,3}\\|?(?:[:\\- ]*\\|)+[\\:\\- ]*\\n)((?:.|\\n(?![ \\t\\n\\r\\f\\x0b]*?\\n| {0,3}(?:[*+-]|[0-9]{1,9}[.)]) |(?: {4}| {0,3}\\t)| {0,3}(?:`{3,}|~{3,})| {0,3}>| {0,3}#{1,6}(?:[ \\t\\n\\r\\f\\x0b]|$)| {0,3}<[^\\n>]+>\\n| {0,3}\\|?(?:[:\\- ]*\\|)+[\\:\\- ]*\\n))+?)\\n {0,3}(=+|-+) *(?:\\n+|$)", false, false),
            list: rx("^( {0,3}(?:[*+-]|[0-9]{1,9}[.)]))([ \\t][^\\n]*?)?(?:\\n|$)", false, false),
            newline: rx("^(?:[ \\t]*(?:\\n|$))+", false, false),
            paragraph: rx("^([^\\n]+(?:\\n(?! {0,3}((?:-[\\t ]*){3,}|(?:_[ \\t]*){3,}|(?:\\*[ \\t]*){3,})(?:\\n+|$)| {0,3}#{1,6}(?:[ \\t\\n\\r\\f\\x0b]|$)| {0,3}>| {0,3}(?:`{3,}(?=[^`\\n]*(?:\\n|$))|~~~)[^\\n]*(?:\\n|$)| {0,3}(?:[*+-]|1[.)])[ \\t]+[^ \\t\\n]|<\\/?(?:address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|meta|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul)(?: +|\\n|\\/?>)|<(?:script|pre|style|textarea|!--)| *([^\\n ].*)\\n {0,3}((?:\\| *)?:?-+:? *(?:\\| *:?-+:? *)*(?:\\| *)?)(?:\\n((?:(?! *\\n| {0,3}((?:-[\\t ]*){3,}|(?:_[ \\t]*){3,}|(?:\\*[ \\t]*){3,})(?:\\n+|$)| {0,3}#{1,6}(?:[ \\t\\n\\r\\f\\x0b]|$)| {0,3}>|(?: {4}| {0,3}\t)[^\\n]| {0,3}(?:`{3,}(?=[^`\\n]*(?:\\n|$))|~~~)[^\\n]*(?:\\n|$)| {0,3}(?:[*+-]|1[.)])[ \\t]|<\\/?(?:address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|meta|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul)(?: +|\\n|\\/?>)|<(?:script|pre|style|textarea|!--)).*(?:\\n|$))*)\\n*|$)|[ \\t]+\\n)[^\\n]+)*)", false, false),
            table: rx("^ *([^\\n ].*)\\n {0,3}((?:\\| *)?:?-+:? *(?:\\| *:?-+:? *)*(?:\\| *)?)(?:\\n((?:(?! *\\n| {0,3}((?:-[\\t ]*){3,}|(?:_[ \\t]*){3,}|(?:\\*[ \\t]*){3,})(?:\\n+|$)| {0,3}#{1,6}(?:[ \\t\\n\\r\\f\\x0b]|$)| {0,3}>|(?: {4}| {0,3}\t)[^\\n]| {0,3}(?:`{3,}(?=[^`\\n]*(?:\\n|$))|~~~)[^\\n]*(?:\\n|$)| {0,3}(?:[*+-]|1[.)])[ \\t]|<\\/?(?:address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|meta|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul)(?: +|\\n|\\/?>)|<(?:script|pre|style|textarea|!--)).*(?:\\n|$))*)\\n*|$)", false, false),
            text: rx("^[^\\n]+", false, false),
            },
            inline: InlineRules {
            _backpedal: rx("(?:[^?!.,:;*_'\"~()&]+|\\([^)]*\\)|&(?![a-zA-Z0-9]+;$)|[?!.,:;*_'\"~)]+(?!$))+", false, false),
            anyPunctuation: rx("\\\\([\\p{P}\\p{S}])", false, false),
            autolink: rx("^<([a-zA-Z][a-zA-Z0-9+.-]{1,31}:[^\\s\\x00-\\x1f<>]*|[a-zA-Z0-9.!#$%&'*+/=?_`{|}~-]+(@)[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)+(?![-_]))>", false, false),
            blockSkip: rx("\\[(?:[^\\[\\]`]|(?<a>`+)[^`]+\\k<a>(?!`))*?\\]\\((?:\\\\[\\s\\S]|[^\\\\\\(\\)]|\\((?:\\\\[\\s\\S]|[^\\\\\\(\\)])*\\))*\\)|(?<!`)()(?<b>`+)[^`]+\\k<b>(?!`)|<(?! )[^<>]*?>", false, false),
            br: rx("^( {2,}|\\\\)\\n(?![ \\t\\n\\r\\f\\x0b]*$)", false, false),
            code: rx("^(`+)([^`]|[^`][\\s\\S]*?[^`])\\1(?!`)", false, false),
            del: rx("^(~~?)(?=[^\\s~])((?:\\\\[\\s\\S]|[^\\\\])*?(?:\\\\[\\s\\S]|[^\\s~\\\\]))\\1(?=[^~]|$)", false, false),
            delLDelim: rx("^~~?(?:((?!~)[\\p{P}\\p{S}])|[^\\s~])", false, false),
            delRDelim: rx("^[^~]+(?=[^~])|(?!~)[\\p{P}\\p{S}](~~?)(?=[\\s]|$)|[^\\s\\p{P}\\p{S}](~~?)(?!~)(?=[\\s\\p{P}\\p{S}]|$)|(?!~)[\\s\\p{P}\\p{S}](~~?)(?=[^\\s\\p{P}\\p{S}])|[\\s](~~?)(?!~)(?=[\\p{P}\\p{S}])|(?!~)[\\p{P}\\p{S}](~~?)(?!~)(?=[\\p{P}\\p{S}])|[^\\s\\p{P}\\p{S}](~~?)(?=[^\\s\\p{P}\\p{S}])", false, false),
            emStrongLDelim: rx("^(?:\\*+(?:((?!\\*)(?!~)[\\p{P}\\p{S}])|([^\\s*]))?)|^_+(?:((?!_)(?!~)[\\p{P}\\p{S}])|([^\\s_]))?", false, false),
            emStrongRDelimAst: rx("^[^_*]*?__[^_*]*?\\*[^_*]*?(?=__)|[^*]+(?=[^*])|(?!\\*)(?!~)[\\p{P}\\p{S}](\\*+)(?=[\\s]|$)|(?:[^\\s\\p{P}\\p{S}]|~)(\\*+)(?!\\*)(?=(?!~)[\\s\\p{P}\\p{S}]|$)|(?!\\*)(?!~)[\\s\\p{P}\\p{S}](\\*+)(?=(?:[^\\s\\p{P}\\p{S}]|~))|[\\s](\\*+)(?!\\*)(?=(?!~)[\\p{P}\\p{S}])|(?!\\*)(?!~)[\\p{P}\\p{S}](\\*+)(?!\\*)(?=(?!~)[\\p{P}\\p{S}])|(?:[^\\s\\p{P}\\p{S}]|~)(\\*+)(?=(?:[^\\s\\p{P}\\p{S}]|~))", false, false),
            emStrongRDelimUnd: rx("^[^_*]*?\\*\\*[^_*]*?_[^_*]*?(?=\\*\\*)|[^_]+(?=[^_])|(?!_)[\\p{P}\\p{S}](_+)(?=[\\s]|$)|[^\\s\\p{P}\\p{S}](_+)(?!_)(?=[\\s\\p{P}\\p{S}]|$)|(?!_)[\\s\\p{P}\\p{S}](_+)(?=[^\\s\\p{P}\\p{S}])|[\\s](_+)(?!_)(?=[\\p{P}\\p{S}])|(?!_)[\\p{P}\\p{S}](_+)(?!_)(?=[\\p{P}\\p{S}])", false, false),
            escape: rx("^\\\\([!\"#$%&'()*+,\\-./:;<=>?@\\[\\]\\\\^_`{|}~])", false, false),
            link: rx("^!?\\[((?:\\[(?:\\\\[\\s\\S]|[^\\[\\]\\\\])*\\]|\\\\[\\s\\S]|`+(?!`)[^`]*?`+(?!`)|``+(?=\\])|[^\\[\\]\\\\`])*?)\\]\\([ \\t\\n\\r\\f\\x0b]*(<(?:\\\\.|[^\\n<>\\\\])+>|[^ \\t\\n\\x00-\\x1f]+|(?=\\)))(?:(?:[ \\t]+(?:\\n[ \\t]*)?|\\n[ \\t]*)(\"(?:\\\\\"?|[^\"\\\\])*\"|'(?:\\\\'?|[^'\\\\])*'|\\((?:\\\\\\)?|[^)\\\\])*\\)))?[ \\t\\n\\r\\f\\x0b]*\\)", false, false),
            nolink: rx("^!?\\[((?![ \\t\\n\\r\\f\\x0b]*\\])(?:\\\\[\\s\\S]|[^\\[\\]\\\\])+)\\](?:\\[\\])?", false, false),
            punctuation: rx("^((?![*_])[\\s\\p{P}\\p{S}])", false, false),
            reflink: rx("^!?\\[((?:\\[(?:\\\\[\\s\\S]|[^\\[\\]\\\\])*\\]|\\\\[\\s\\S]|`+(?!`)[^`]*?`+(?!`)|``+(?=\\])|[^\\[\\]\\\\`])*?)\\]\\[((?![ \\t\\n\\r\\f\\x0b]*\\])(?:\\\\[\\s\\S]|[^\\[\\]\\\\])+)\\]", false, false),
            reflinkSearch: rx("!?\\[((?:\\[(?:\\\\[\\s\\S]|[^\\[\\]\\\\])*\\]|\\\\[\\s\\S]|`+(?!`)[^`]*?`+(?!`)|``+(?=\\])|[^\\[\\]\\\\`])*?)\\]\\[((?![ \\t\\n\\r\\f\\x0b]*\\])(?:\\\\[\\s\\S]|[^\\[\\]\\\\])+)\\]|!?\\[((?![ \\t\\n\\r\\f\\x0b]*\\])(?:\\\\[\\s\\S]|[^\\[\\]\\\\])+)\\](?:\\[\\])?(?!\\()", false, false),
            tag: rx("^<!--(?:-?>|[\\s\\S]*?-->)|^<\\/[a-zA-Z][\\w:-]*[ \\t\\n\\r\\f\\x0b]*>|^<[a-zA-Z][\\w-]*(?:[ \\t\\n\\r\\f\\x0b]+[a-zA-Z:_][\\w.:-]*(?:[ \\t\\n\\r\\f\\x0b]*=[ \\t\\n\\r\\f\\x0b]*\"[^\"]*\"|[ \\t\\n\\r\\f\\x0b]*=[ \\t\\n\\r\\f\\x0b]*'[^']*'|[ \\t\\n\\r\\f\\x0b]*=[ \\t\\n\\r\\f\\x0b]*[^\\s\"'=<>`]+)?)*?[ \\t\\n\\r\\f\\x0b]*\\/?>|^<\\?[\\s\\S]*?\\?>|^<![a-zA-Z]+[ \\t\\n\\r\\f\\x0b][\\s\\S]*?>|^<!\\[CDATA\\[[\\s\\S]*?\\]\\]>", false, false),
            text: rx("^(`+|~+|[^`~])(?:(?=[`~])|(?= {2,}\\n)|(?=[a-zA-Z0-9.!#$%&'*+\\/=?_`{\\|}~-]+@)|[\\s\\S]*?(?:(?=[\\\\<!\\[`*~_]|(?<![0-9A-Za-z_])_|[hH][tT][tT][pP][sS]?|[fF][tT][pP]:\\/\\/|www\\.|$)|[^ ](?= {2,}\\n)|[^a-zA-Z0-9.!#$%&'*+\\/=?_`{\\|}~-](?=[a-zA-Z0-9.!#$%&'*+\\/=?_`{\\|}~-]+@)))", false, false),
            url: rx("^((?:[hH][tT][tT][pP][sS]?|[fF][tT][pP]):\\/\\/|www\\.)(?:[a-zA-Z0-9\\-]+\\.?)+[^\\s<]*|^[A-Za-z0-9._+-]+(@)[a-zA-Z0-9-_]+(?:\\.[a-zA-Z0-9-_]*[a-zA-Z0-9])+(?![-_])", false, false),
            },
            other: OtherRules {
            codeRemoveIndent: rx("^(?: {1,4}| {0,3}\\t)", false, true),
            outputLinkReplace: rx("\\\\([\\[\\]])", false, false),
            indentCodeCompensation: rx("^([ \\t\\n\\r\\f\\x0b]+)(?:```)", false, false),
            beginningSpace: rx("^[ \\t\\n\\r\\f\\x0b]+", false, false),
            endingHash: rx("#$", false, false),
            startingSpaceChar: rx("^ ", false, false),
            endingSpaceChar: rx(" $", false, false),
            nonSpaceChar: rx("[^ ]", false, false),
            newLineCharGlobal: rx("\\n", false, false),
            tabCharGlobal: rx("\\t", false, false),
            multipleSpaceGlobal: rx("[ \\t\\n\\r\\f\\x0b]+", false, false),
            blankLine: rx("^[ \\t]*$", false, false),
            doubleBlankLine: rx("\\n[ \\t]*\\n[ \\t]*$", false, false),
            blockquoteStart: rx("^ {0,3}>", false, false),
            blockquoteSetextReplace: rx("\\n {0,3}((?:=+|-+) *)(?=\\n|$)", false, false),
            blockquoteSetextReplace2: rx("^ {0,3}>[ \\t]?", false, true),
            listReplaceNesting: rx("^ {1,4}(?=( {4})*[^ ])", false, false),
            listIsTask: rx("^\\[[ xX]\\] +[^ \\t\\n\\r\\f\\x0b]", false, false),
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
            startPreScriptTag: rx("^<(pre|code|kbd|script)([ \\t\\n\\r\\f\\x0b]|>)", true, false),
            endPreScriptTag: rx("^<\\/(pre|code|kbd|script)([ \\t\\n\\r\\f\\x0b]|>)", true, false),
            startAngleBracket: rx("^<", false, false),
            endAngleBracket: rx(">$", false, false),
            pedanticHrefTitle: rx("^([^'\"]*[^\\s])[ \\t\\n\\r\\f\\x0b]+(['\"])(.*)\\2", false, false),
            unicodeAlphaNumeric: rx("[\\p{L}\\p{N}]", false, false),
            escapeTest: rx("[&<>\"']", false, false),
            escapeReplace: rx("[&<>\"']", false, false),
            escapeTestNoEncode: rx("[<>\"']|&(?!(#[0-9]{1,7}|#[Xx][a-fA-F0-9]{1,6}|[0-9A-Za-z_]+);)", false, false),
            escapeReplaceNoEncode: rx("[<>\"']|&(?!(#[0-9]{1,7}|#[Xx][a-fA-F0-9]{1,6}|[0-9A-Za-z_]+);)", false, false),
            caret: rx("(^|[^\\[])\\^", false, false),
            percentDecode: rx("%25", false, false),
            findPipe: rx("\\|", false, false),
            splitPipe: rx(" \\|", false, false),
            slashPipe: rx("\\\\\\|", false, false),
            carriageReturn: rx("\\r\\n|\\r", false, false),
            spaceLine: rx("^ +$", false, true),
            notSpaceStart: rx("^[^ \\t\\n\\r\\f\\x0b]*", false, false),
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
    let translated = js_pattern_to_ascii(pattern);
    RegexBuilder::new(&translated)
        .case_insensitive(case_insensitive)
        .multi_line(multi_line)
        .build()
        .unwrap_or_else(|e| panic!("dynamic marked rule failed to compile: {pattern} ({e})"))
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
        let re = Regex::new(&js_pattern_to_ascii(r"\d")).unwrap();
        assert!(re.is_match("5").unwrap());
        assert!(!re.is_match("\u{0663}").unwrap());
    }
}
