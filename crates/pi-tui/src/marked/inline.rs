//! Inline tokenizer methods (marked Tokenizer, marked-pretty.js lines
//! ~1084-1333), the inline dispatch loop (`Lexer.inlineTokens`, lines
//! ~1559-1672), `Lexer.linkInText` (lines ~1536-1558), the latex inline
//! extension (`tokenizeInlineLatex`), and the StrictStrikethroughTokenizer
//! del() override — all from /tmp/marked-pretty.js and
//! /tmp/pi-repo/packages/tui/src/components/markdown.ts.
//!
//! PORT OWNERSHIP: inline.rs is filled in by the inline-tokenizer port; do
//! not edit signatures without updating mod.rs/lexer.rs/block.rs.
//!
//! JS-semantics notes (deviations are all non-BMP/astral edge cases; the
//! differential corpus is BMP-only apart from one plain-text emoji):
//! - JS string lengths / regex match indices are UTF-16 code units. This
//!   port uses CHARACTER (code point) counts everywhere; the two agree for
//!   all BMP input. They can diverge when an astral char sits inside an
//!   emphasis delimiter run, before a closing delimiter in the masked src,
//!   or when a mask replacement length is computed (JS `"a".repeat(len)`
//!   uses UTF-16 length, we use char count).
//! - The shared `Token` model (mod.rs) has no `escaped` (inlineText) or
//!   `inLink`/`inRawBlock` (inline html) fields; those JSON fields are
//!   emitted by the TS side but cannot be represented here (see port
//!   report). `tag()`/`inline_text()` fill every field that exists.

use std::collections::HashMap;
use std::sync::OnceLock;

use fancy_regex::Regex;

use super::helpers::{
    find_closing_bracket, js_match_all, js_replace, js_trim,
    latex_find_closing_delimiter, latex_looks_like_pending_dollar_math, output_link, rtrim,
    StrCaptures,
};
use super::{new_token_list, Lexer, LinkDef, Token, TokenList};

// ---------------------------------------------------------------------------
// Local JS-semantics helpers (helpers.rs versions of these are private)
// ---------------------------------------------------------------------------

/// JS `String.prototype.slice(start, end)` over char indices (negative
/// counts from the end, both clamped to [0, len], start >= end -> "").
fn js_slice(text: &str, start: i64, end: i64) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len() as i64;
    let norm = |v: i64| {
        if v < 0 {
            (len + v).clamp(0, len)
        } else {
            v.clamp(0, len)
        }
    };
    let s = norm(start);
    let e = norm(end);
    if s >= e {
        String::new()
    } else {
        chars[s as usize..e as usize].iter().collect()
    }
}

/// JS `text.substring(0, n)` (n >= 0, clamped): the first `n` characters.
fn first_n_chars(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// JS `e.substring(raw.length)` after a tokenizer match: drop the first
/// `raw.chars().count()` characters. Equivalent to the JS UTF-16 arithmetic
/// whenever `raw` is a prefix of `e` (always true for marked tokenizers).
fn cut_raw(e: &str, raw: &str) -> String {
    let n = raw.chars().count();
    match e.char_indices().nth(n) {
        Some((i, _)) => e[i..].to_string(),
        None => String::new(),
    }
}

/// Byte index of the start of the character after `pos` (helpers.rs keeps a
/// private copy; this is the same logic).
fn char_boundary_after(text: &str, pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    let mut i = pos + 1;
    while i < text.len() && !text.is_char_boundary(i) {
        i += 1;
    }
    i
}

// JS `\s` / JS `.` semantics are shared with the marked rule tables — see
// the constants and translator in `rules.rs` (`JS_WS_SET` / `JS_DOT_SET`).
use super::rules::{JS_DOT_SET as JS_DOT_CLASS, JS_WS_SET as JS_WS_CLASS};

/// pi-tui's STRICT_STRIKETHROUGH_REGEX (markdown.ts line 5), ASCII-\s and
/// JS-dot translated:
/// /^(~~)(?=[^\s~])((?:\\.|[^\\])*?(?:\\.|[^\s~\\]))\1(?=[^~]|$)/
fn strict_del_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let ws = JS_WS_CLASS;
        let dot = JS_DOT_CLASS;
        let pattern = format!(
            r"^(~~)(?=[^{ws}~])((?:\\{dot}|[^\\])*?(?:\\{dot}|[^{ws}~\\]))\1(?=[^~]|$)"
        );
        Regex::new(&pattern).expect("strict strikethrough regex")
    })
}

/// markdown.ts guard `/^\$\s/` — a `$` opener may not be followed by JS
/// whitespace.
fn latex_dollar_start_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"^\$[{ws}]", ws = JS_WS_CLASS)).expect("latex $-start regex")
    })
}

/// markdown.ts guard `/\s$/`.
fn latex_trailing_ws_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"[{ws}]$", ws = JS_WS_CLASS)).expect("latex trailing-ws regex")
    })
}

/// markdown.ts guard `/^\d/` (JS \d is ASCII-only).
fn latex_leading_digit_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[0-9]").expect("latex leading-digit regex"))
}

/// markdown.ts guard
/// `/^[A-Z_][A-Z0-9_]*(?:[^A-Za-z0-9_\s])?$/`.
fn latex_upper_token_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"^[A-Z_][A-Z0-9_]*(?:[^A-Za-z0-9{ws}])?$",
            ws = JS_WS_CLASS
        ))
        .expect("latex upper-token regex")
    })
}

/// markdown.ts guard `/^[A-Za-z_][A-Za-z0-9_]*/`.
fn latex_ident_start_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*").expect("latex ident-start regex")
    })
}

/// First non-empty capture group among `groups` (JS `s[1] || s[2] || ...`:
/// undefined and "" are both falsy).
fn first_nonempty_group<'c>(caps: &'c StrCaptures<'_>, groups: &[usize]) -> Option<&'c str> {
    groups
        .iter()
        .find_map(|&i| caps.get(i).map(|m| m.as_str()))
        .filter(|s| !s.is_empty())
}

/// Group `i` participates and is non-empty (JS truthiness of `s[i]`).
fn group_truthy(caps: &StrCaptures<'_>, i: usize) -> bool {
    caps.get(i).map(|m| !m.as_str().is_empty()).unwrap_or(false)
}

impl Lexer {
    /// `Tokenizer.escape(src)` (marked-pretty.js ~1084).
    pub(crate) fn escape(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.inline.escape.captures(src).ok().flatten()?;
        let mut token = Token::new("escape", caps.get(0)?.as_str());
        token.text = Some(caps.get(1)?.as_str().to_string());
        Some(token)
    }

    /// `Tokenizer.tag(src)` (~1088).
    pub(crate) fn tag(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.inline.tag.captures(src).ok().flatten()?;
        let t0 = caps.get(0)?.as_str();
        // inLink dance: `!inLink && startATag.test ? inLink = true
        //               : inLink && endATag.test && (inLink = false)`
        if !self.state.in_link && self.rules.other.startATag.is_match(t0).unwrap() {
            self.state.in_link = true;
        } else if self.state.in_link && self.rules.other.endATag.is_match(t0).unwrap() {
            self.state.in_link = false;
        }
        // inRawBlock dance (pre/code/kbd/script tags).
        if !self.state.in_raw_block && self.rules.other.startPreScriptTag.is_match(t0).unwrap() {
            self.state.in_raw_block = true;
        } else if self.state.in_raw_block
            && self.rules.other.endPreScriptTag.is_match(t0).unwrap()
        {
            self.state.in_raw_block = false;
        }
        let mut token = Token::new("html", t0);
        token.text = Some(t0.to_string());
        token.block = Some(false);
        // JS reads the state AFTER the dance above.
        token.in_link = Some(self.state.in_link);
        token.in_raw_block = Some(self.state.in_raw_block);
        Some(token)
    }

    /// `Tokenizer.link(src)` (~1113).
    pub(crate) fn link(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.inline.link.captures(src).ok().flatten()?;
        let rules = self.rules;
        let t0_full = caps.get(0)?.as_str();
        let t1 = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let t2_orig = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let t3_orig: Option<&str> = caps.get(3).map(|m| m.as_str());

        let n = js_trim(t2_orig).to_string();
        let mut t2 = t2_orig.to_string();
        let mut t0 = t0_full.to_string();
        let mut cut_paren = false; // whether t[2]/t[0]/t[3] got rewritten below

        if rules.other.startAngleBracket.is_match(&n).unwrap() {
            if !rules.other.endAngleBracket.is_match(&n).unwrap() {
                return None;
            }
            // i = $(n.slice(0, -1), "\\") — rtrim of the trailing backslashes
            let stripped = rtrim(&js_slice(&n, 0, -1), '\\', false);
            if (n.chars().count() - stripped.chars().count()) % 2 == 0 {
                return None;
            }
        } else {
            let close = find_closing_bracket(&t2.chars().collect::<Vec<char>>(), '(', ')');
            if close == -2 {
                return None;
            }
            if close > -1 {
                // u = (image ? 5 : 4) + t[1].length + i — all char counts
                // (JS .length is UTF-16; identical for BMP).
                let u = (if t0_full.starts_with('!') { 5i64 } else { 4i64 })
                    + t1.chars().count() as i64
                    + close;
                t2 = first_n_chars(&t2, close as usize);
                t0 = js_trim(&first_n_chars(t0_full, u.max(0) as usize)).to_string();
                cut_paren = true; // t[3] = ""
            }
        }

        let mut s = t2;
        // non-pedantic: r = t[3] ? t[3].slice(1, -1) : ""
        let mut r = String::new();
        if !cut_paren {
            if let Some(t3) = t3_orig.filter(|x| !x.is_empty()) {
                r = js_slice(t3, 1, -1);
            }
        }
        s = js_trim(&s).to_string();
        if rules.other.startAngleBracket.is_match(&s).unwrap() {
            // non-pedantic branch: s = s.slice(1, -1)
            s = js_slice(&s, 1, -1);
        }
        // href = s && s.replace(anyPunctuation, "$1") (anyPunctuation is /gu)
        // title = r && r.replace(anyPunctuation, "$1")
        let strip_punct = |text: &str| -> String {
            js_replace(&rules.inline.anyPunctuation, text, |c| {
                c.get(1).map(|g| g.as_str().to_string()).unwrap_or_default()
            })
        };
        let href = if s.is_empty() {
            String::new()
        } else {
            strip_punct(&s)
        };
        let title = if r.is_empty() {
            None
        } else {
            Some(strip_punct(&r))
        };
        let def = LinkDef { href, title };
        output_link(&caps, &def, &t0, self, &rules.other.outputLinkReplace)
    }

    /// `Tokenizer.reflink(src, links)` (~1159).
    pub(crate) fn reflink(
        &mut self,
        src: &str,
        links: &HashMap<String, LinkDef>,
    ) -> Option<Token> {
        let rules = self.rules;
        let (caps, from_reflink) = match rules.inline.reflink.captures(src).ok().flatten() {
            Some(c) => (c, true),
            None => match rules.inline.nolink.captures(src).ok().flatten() {
                Some(c) => (c, false),
                None => return None,
            },
        };
        // s = (n[2] || n[1]).replace(multipleSpaceGlobal, " ")
        let label_raw = if from_reflink {
            first_nonempty_group(&caps, &[2]).unwrap_or_else(|| {
                caps.get(1).map(|m| m.as_str()).unwrap_or("")
            })
        } else {
            caps.get(1).map(|m| m.as_str()).unwrap_or("")
        };
        let s = js_replace(&rules.other.multipleSpaceGlobal, label_raw, |_| " ".to_string());
        // r = links[s.toLowerCase()] — Rust to_lowercase matches JS simple
        // lowercase for ASCII (full Unicode mapping has rare divergences,
        // e.g. some expansion cases; corpus is unaffected).
        let def = links.get(&s.to_lowercase());
        match def {
            None => {
                // { type: "text", raw: i, text: i } with i = n[0].charAt(0)
                let first = caps
                    .get(0)?
                    .as_str()
                    .chars()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let mut token = Token::new("text", first.clone());
                token.text = Some(first);
                Some(token)
            }
            Some(def) => output_link(
                &caps,
                def,
                caps.get(0)?.as_str(),
                self,
                &rules.other.outputLinkReplace,
            ),
        }
    }

    /// `Tokenizer.emStrong(src, maskedSrc, prevChar)` (~1174).
    pub(crate) fn em_strong(
        &mut self,
        src: &str,
        masked_src: &str,
        prev_char: &str,
    ) -> Option<Token> {
        let rules = self.rules;
        let caps = rules.inline.emStrongLDelim.captures(src).ok().flatten()?;
        // return if (!s[1] && !s[2] && !s[3] && !s[4]) — no capture groups
        if !(group_truthy(&caps, 1)
            || group_truthy(&caps, 2)
            || group_truthy(&caps, 3)
            || group_truthy(&caps, 4))
        {
            return None;
        }
        // return if (s[4] && prevChar.match(unicodeAlphaNumeric))
        if group_truthy(&caps, 4) && rules.other.unicodeAlphaNumeric.is_match(prev_char).unwrap() {
            return None;
        }
        // if (!(s[1] || s[3] || "") || !prevChar || punctuation.exec(prevChar))
        let punct_prev = rules.inline.punctuation.is_match(prev_char).unwrap();
        if (!group_truthy(&caps, 1) && !group_truthy(&caps, 3))
            || prev_char.is_empty()
            || punct_prev
        {
            let s0 = caps.get(0)?.as_str();
            let i: i64 = s0.chars().count() as i64 - 1; // [...s[0]].length - 1
            let mut a = i;
            let mut p: i64 = 0;
            let c = s0.chars().next()?;
            let h = prev_char == c.to_string(); // h = prevChar === c
            let d = if c == '*' {
                &rules.inline.emStrongRDelimAst
            } else {
                &rules.inline.emStrongRDelimUnd
            };
            // t = maskedSrc.slice(-1 * src.length + i): the last
            // (src.len - i) characters (masks preserve length, so char
            // positions in the masked suffix line up with src).
            let src_chars = src.chars().count() as i64;
            let masked_chars = masked_src.chars().count() as i64;
            let take = src_chars - i; // always >= 1 (i <= src_chars - 1)
            let t: String = if take >= masked_chars {
                masked_src.to_string()
            } else if take <= 0 {
                String::new()
            } else {
                masked_src
                    .chars()
                    .skip((masked_chars - take) as usize)
                    .collect()
            };
            // rdelim loop — the JS regex is /g(u), so exec advances past
            // each match (all alternatives are non-empty; no empty matches).
            let mut pos = 0usize;
            loop {
                let mcaps = match d.captures_from_pos(t.as_str(), pos) {
                    Ok(Some(c)) => c,
                    _ => break,
                };
                let m0 = mcaps.get(0)?;
                let (ms, me) = (m0.start(), m0.end());
                pos = me; // d.lastIndex advance (global exec)
                let o = first_nonempty_group(&mcaps, &[1, 2, 3, 4, 5, 6]);
                let Some(o) = o else { continue };
                let mut u: i64 = o.chars().count() as i64; // [...o].length
                if group_truthy(&mcaps, 3) || group_truthy(&mcaps, 4) {
                    a += u;
                    continue;
                } else if group_truthy(&mcaps, 5) || group_truthy(&mcaps, 6) {
                    if i % 3 != 0 && (i + u) % 3 == 0 {
                        p += u;
                        continue;
                    }
                    if h {
                        break;
                    }
                }
                a -= u;
                if a > 0 {
                    continue;
                }
                u = u.min(u + a + p);
                // O = [...s[0]][0].length — UTF-16 length of the first code
                // point of the rdelim match; 1 in char space (equivalent).
                let big_o: i64 = 1;
                // s.index is a byte offset into `t`; convert to char index.
                let s_index: i64 = t[..ms].chars().count() as i64;
                let g_len = i + s_index + big_o + u;
                let g = if g_len <= 0 {
                    String::new()
                } else {
                    first_n_chars(src, g_len as usize)
                };
                if i.min(u) % 2 != 0 {
                    // em: g.slice(1, -1)
                    let e_text = js_slice(&g, 1, -1);
                    let mut token = Token::new("em", g);
                    token.text = Some(e_text.clone());
                    token.tokens = Some(self.inline_tokens_new(&e_text));
                    return Some(token);
                }
                // strong: g.slice(2, -2)
                let w = js_slice(&g, 2, -2);
                let mut token = Token::new("strong", g);
                token.text = Some(w.clone());
                token.tokens = Some(self.inline_tokens_new(&w));
                return Some(token);
            }
        }
        None
    }

    /// `Tokenizer.codespan(src)` (~1232).
    pub(crate) fn codespan(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.inline.code.captures(src).ok().flatten()?;
        let t2 = caps.get(2)?.as_str();
        let mut n = js_replace(&self.rules.other.newLineCharGlobal, t2, |_| " ".to_string());
        let has_non_space = self.rules.other.nonSpaceChar.is_match(&n).unwrap();
        let spaces_both_ends = self.rules.other.startingSpaceChar.is_match(&n).unwrap()
            && self.rules.other.endingSpaceChar.is_match(&n).unwrap();
        if has_non_space && spaces_both_ends {
            n = js_slice(&n, 1, -1); // n.substring(1, n.length - 1)
        }
        let mut token = Token::new("codespan", caps.get(0)?.as_str());
        token.text = Some(n);
        Some(token)
    }

    /// `Tokenizer.br(src)` (~1246).
    pub(crate) fn br(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.inline.br.captures(src).ok().flatten()?;
        Some(Token::new("br", caps.get(0)?.as_str()))
    }

    /// `Tokenizer.del(src, maskedSrc, prevChar)` (~1250) — OVERRIDDEN by
    /// pi-tui's StrictStrikethroughTokenizer (markdown.ts lines 7-21): match
    /// STRICT_STRIKETHROUGH_REGEX, text = match[2], tokens =
    /// lexer.inlineTokens(text). The maskedSrc/prevChar args are ignored by
    /// the override (kept for dispatch fidelity).
    pub(crate) fn del(&mut self, src: &str, masked_src: &str, prev_char: &str) -> Option<Token> {
        let _ = (masked_src, prev_char);
        let caps = strict_del_regex().captures(src).ok().flatten()?;
        let text = caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
        let mut token = Token::new("del", caps.get(0)?.as_str());
        token.text = Some(text.clone());
        token.tokens = Some(self.inline_tokens_new(&text));
        Some(token)
    }

    /// `Tokenizer.autolink(src)` (~1286).
    pub(crate) fn autolink(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.inline.autolink.captures(src).ok().flatten()?;
        let t1 = caps.get(1)?.as_str();
        let (text, href) = if caps.get(2).map(|m| m.as_str()) == Some("@") {
            (t1.to_string(), format!("mailto:{}", t1))
        } else {
            (t1.to_string(), t1.to_string())
        };
        let mut token = Token::new("link", caps.get(0)?.as_str());
        token.text = Some(text.clone());
        token.href = Some(href);
        // tokens: [{ type: "text", raw: n, text: n }]
        let inner = new_token_list();
        let mut it = Token::new("text", text.clone());
        it.text = Some(text);
        inner.borrow_mut().push(it);
        token.tokens = Some(inner);
        Some(token)
    }

    /// `Tokenizer.url(src)` (~1304).
    pub(crate) fn url(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.inline.url.captures(src).ok().flatten()?;
        let full = caps.get(0)?.as_str();
        let t0_original = full;
        let mut t0 = full.to_string();
        let (text, href) = if caps.get(2).map(|m| m.as_str()) == Some("@") {
            (t0.clone(), format!("mailto:{}", t0))
        } else {
            // do { r = t[0]; t[0] = _backpedal.exec(t[0])?.[0] ?? "" }
            // while (r !== t[0])
            loop {
                let r0 = t0.clone();
                let next = self
                    .rules
                    .inline
                    ._backpedal
                    .captures(&t0)
                    .ok()
                    .flatten()
                    .and_then(|c| c.get(0).map(|g| g.as_str().to_string()))
                    .unwrap_or_default();
                t0 = next;
                if r0 == t0 {
                    break;
                }
            }
            let href = if caps.get(1).map(|m| m.as_str()) == Some("www.") {
                format!("http://{}", t0)
            } else {
                t0.clone()
            };
            (t0.clone(), href)
        };
        let _ = t0_original;
        let mut token = Token::new("link", text.clone());
        token.text = Some(text.clone());
        token.href = Some(href);
        let inner = new_token_list();
        let mut it = Token::new("text", text.clone());
        it.text = Some(text);
        inner.borrow_mut().push(it);
        token.tokens = Some(inner);
        Some(token)
    }

    /// `Tokenizer.inlineText(src)` (~1326).
    pub(crate) fn inline_text(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.inline.text.captures(src).ok().flatten()?;
        let raw = caps.get(0)?.as_str().to_string();
        // let n = this.lexer.state.inRawBlock — `escaped: n` on the token.
        let mut token = Token::new("text", raw.clone());
        token.text = Some(raw);
        token.escaped = Some(self.state.in_raw_block);
        Some(token)
    }

    /// `Lexer.linkInText(src)` (marked-pretty.js ~1536-1558).
    pub(crate) fn link_in_text(&self, src: &str) -> bool {
        if !src.contains('[') {
            return false;
        }
        let link_re = &self.rules.inline.link;
        for (offset, caps) in js_match_all(&self.rules.inline.blockSkip, src) {
            let full = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            if link_re.is_match(full).unwrap() {
                // e.charAt(n.index - 1) !== "!" — charAt out of range gives
                // "" (which is !== "!"), so a match at offset 0 counts.
                let prev = src[..offset].chars().last();
                if prev != Some('!') {
                    return true;
                }
            }
        }
        for (_offset, caps) in js_match_all(&self.rules.inline.reflinkSearch, src) {
            let s = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            let chars: Vec<char> = s.chars().collect();
            let r: i64 = chars
                .iter()
                .rposition(|&ch| ch == '[')
                .map(|x| x as i64)
                .unwrap_or(-1); // s.lastIndexOf("[")
            // !(s.charAt(0) === "!" || !hasOwn(links, s.slice(r + 1, -1)))
            let label = js_slice(s, r + 1, -1);
            let cond1 = !(chars.first() == Some(&'!') || !self.links.contains_key(&label));
            // !(r > 1 && this.linkInText(s.slice(1, r - 1)))
            let cond2 = if r > 1 {
                let inner = js_slice(s, 1, r - 1);
                !self.link_in_text(&inner)
            } else {
                true
            };
            if cond1 && cond2 {
                return true;
            }
        }
        false
    }

    /// The reflinkSearch mask replacer `u` from `Lexer.inlineTokens`
    /// (marked-pretty.js ~1568-1578), factored out so it can recurse
    /// (`c.replace(o, u)` inside the replacer).
    fn reflink_mask_replacer(&self, a: &str) -> String {
        let chars: Vec<char> = a.chars().collect();
        let p: i64 = chars
            .iter()
            .rposition(|&ch| ch == '[')
            .map(|x| x as i64)
            .unwrap_or(-1); // a.lastIndexOf("[")
        let len = chars.len() as i64;
        // if (!hasOwn(links, a.slice(p + 1, -1))) return a;
        let label = js_slice(a, p + 1, -1);
        if !self.links.contains_key(&label) {
            return a.to_string();
        }
        // if (p > 1 && a.charAt(0) !== "!") { ... }
        if p > 1 && chars.first() != Some(&'!') {
            let c = js_slice(a, 1, p - 1); // a.slice(1, p - 1)
            if self.link_in_text(&c) {
                // "[" + c.replace(o, u) + "][" + "a".repeat(a.length - p - 2) + "]"
                let c_masked = self.mask_reflink_search(&c);
                let repeat = (len - p - 2).max(0) as usize;
                return format!("[{}][{}]", c_masked, "a".repeat(repeat));
            }
        }
        // "[" + "a".repeat(a.length - 2) + "]"
        let repeat = (len - 2).max(0) as usize;
        format!("[{}]", "a".repeat(repeat))
    }

    /// `n = n.replace(reflinkSearch, u)` from `Lexer.inlineTokens`
    /// (~1566-1580). The `this.tokens.links &&` guard is always truthy (the
    /// links object always exists), so the caller only checks for "[".
    fn mask_reflink_search(&self, text: &str) -> String {
        let re = &self.rules.inline.reflinkSearch;
        js_replace(re, text, |caps| {
            let a = caps.get(0).map(|m| m.as_str()).unwrap_or("");
            self.reflink_mask_replacer(a)
        })
    }

    /// `Lexer.inlineTokens(src, tokens)` (marked-pretty.js ~1559-1672) —
    /// the inline dispatch loop: masked-src preprocessing, extension-first
    /// dispatch (latex), escape → tag → link → reflink → emStrong →
    /// codespan → br → del → autolink → url → startInline-bounded
    /// inlineText, prevChar tracking, infinite-loop guard.
    pub(crate) fn inline_tokens(&mut self, src: &str, tokens: &TokenList) {
        // this.tokenizer.lexer = this — no-op (tokenizer methods live on self).
        let mut e = src.to_string();
        let mut n = e.clone(); // masked src
        if e.contains('[') {
            n = self.mask_reflink_search(&n);
        }
        // anyPunctuation mask: "+".repeat(o.length) — the full match is
        // backslash + one punct/symbol char (2 chars; JS counts UTF-16
        // units, we count chars — identical for BMP).
        n = js_replace(&self.rules.inline.anyPunctuation, &n, |c| {
            let m = c.get(0).map(|g| g.as_str()).unwrap_or("");
            "+".repeat(m.chars().count())
        });
        // blockSkip mask. DEAD-CODE DISCOVERY (probed in Node): the JS
        // replacer `(o, u, a) => { let p = a ? a.length : 0; ... }` receives
        // `a` = the ALWAYS-EMPTY capture group `()` inside blockSkip, never
        // the match offset — so p is always 0 and the effective replacement
        // is always "[" + "a" * (full_match_len - 2) + "]".
        n = js_replace(&self.rules.inline.blockSkip, &n, |c| {
            let m = c.get(0).map(|g| g.as_str()).unwrap_or("");
            let len = m.chars().count();
            format!("[{}]", "a".repeat(len.saturating_sub(2)))
        });
        // options.hooks?.emStrongMask — hooks are undefined in our config.

        // reflink dispatch needs &self.links while self is mutably
        // borrowed; links are read-only during inline tokenization (defs
        // only register in the block phase), so clone once up front.
        let links = self.links.clone();

        let mut prev_captured = false; // s (prevCaptured)
        let mut prev_char = String::new(); // r (prevChar)
        let mut min_len = usize::MAX; // i (min src length)

        while !e.is_empty() {
            // Infinite-loop guard: JS compares .length (UTF-16); we compare
            // char counts (identical for BMP).
            let e_len = e.chars().count();
            if e_len < min_len {
                min_len = e_len;
            } else {
                self.infinite_loop_error(e.chars().next().map(|c| c as u32).unwrap_or(0));
                break;
            }
            if !prev_captured {
                prev_char.clear(); // s || (r = "")
            }
            prev_captured = false; // s = !1

            // extensions.inline (latex) — dispatched FIRST.
            if let Some(o) = self.ext_latex_inline(&e) {
                e = cut_raw(&e, &o.raw);
                tokens.borrow_mut().push(o);
                continue;
            }
            if let Some(o) = self.escape(&e) {
                e = cut_raw(&e, &o.raw);
                tokens.borrow_mut().push(o);
                continue;
            }
            if let Some(o) = self.tag(&e) {
                e = cut_raw(&e, &o.raw);
                tokens.borrow_mut().push(o);
                continue;
            }
            if let Some(o) = self.link(&e) {
                e = cut_raw(&e, &o.raw);
                tokens.borrow_mut().push(o);
                continue;
            }
            if let Some(o) = self.reflink(&e, &links) {
                e = cut_raw(&e, &o.raw);
                // text-token merging into the previous text token
                // (o.type === "text" && a?.type === "text").
                let mut tl = tokens.borrow_mut();
                let last_is_text = tl.last().map(|a| a.r#type == "text").unwrap_or(false);
                if o.r#type == "text" && last_is_text {
                    let a = tl.last_mut().expect("checked last_is_text");
                    a.raw.push_str(&o.raw);
                    // a.text += o.text — text tokens always carry text in
                    // practice; JS `undefined += x` would yield "undefinedx".
                    a.text = Some(format!(
                        "{}{}",
                        a.text.clone().unwrap_or_default(),
                        o.text.clone().unwrap_or_default()
                    ));
                } else {
                    tl.push(o);
                }
                continue;
            }
            if let Some(o) = self.em_strong(&e, &n, &prev_char) {
                e = cut_raw(&e, &o.raw);
                tokens.borrow_mut().push(o);
                continue;
            }
            if let Some(o) = self.codespan(&e) {
                e = cut_raw(&e, &o.raw);
                tokens.borrow_mut().push(o);
                continue;
            }
            if let Some(o) = self.br(&e) {
                e = cut_raw(&e, &o.raw);
                tokens.borrow_mut().push(o);
                continue;
            }
            if let Some(o) = self.del(&e, &n, &prev_char) {
                e = cut_raw(&e, &o.raw);
                tokens.borrow_mut().push(o);
                continue;
            }
            if let Some(o) = self.autolink(&e) {
                e = cut_raw(&e, &o.raw);
                tokens.borrow_mut().push(o);
                continue;
            }
            if !self.state.in_link {
                if let Some(o) = self.url(&e) {
                    e = cut_raw(&e, &o.raw);
                    tokens.borrow_mut().push(o);
                    continue;
                }
            }
            // startInline-bounded inlineText (~1647-1656): the latex start
            // fn is called on e minus its first CHARACTER and returns a byte
            // index into that slice; u = e.substring(0, a + 1) in JS =
            // first char + p[..a] here.
            let text_src = {
                let first_len = char_boundary_after(&e, 0);
                let p = &e[first_len..]; // e.slice(1)
                match self.ext_latex_inline_start(p) {
                    Some(a) => e[..first_len + a].to_string(),
                    None => e.clone(),
                }
            };
            if let Some(o) = self.inline_text(&text_src) {
                e = cut_raw(&e, &o.raw);
                // o.raw.slice(-1) !== "_" && (r = o.raw.slice(-1))
                let last = o.raw.chars().last();
                if last != Some('_') {
                    prev_char = last.map(|ch| ch.to_string()).unwrap_or_default();
                }
                prev_captured = true;
                // a?.type === "text" ? merge : push (no type check on o —
                // inlineText always produces type "text").
                let mut tl = tokens.borrow_mut();
                let last_is_text = tl.last().map(|a| a.r#type == "text").unwrap_or(false);
                if last_is_text {
                    let a = tl.last_mut().expect("checked last_is_text");
                    a.raw.push_str(&o.raw);
                    a.text = Some(format!(
                        "{}{}",
                        a.text.clone().unwrap_or_default(),
                        o.text.clone().unwrap_or_default()
                    ));
                } else {
                    tl.push(o);
                }
                continue;
            }
            // `if (e) { infiniteLoopError; break; }` — e is non-empty here
            // (loop condition), so this always fires.
            self.infinite_loop_error(e.chars().next().map(|c| c as u32).unwrap_or(0));
            break;
        }
    }

    /// `Lexer.inlineTokens(src)` with a fresh target array (the dispatch
    /// form's default `t = []`, used by emStrong/del/outputLink recursion).
    pub(crate) fn inline_tokens_new(&mut self, src: &str) -> TokenList {
        let arr = new_token_list();
        self.inline_tokens(src, &arr);
        arr
    }

    /// The latex inline tokenizer extension (markdown.ts
    /// `tokenizeInlineLatex`, lines ~36-84), dispatched FIRST in the inline
    /// loop. Handles $$..$$, \(..\), \[..\], and single $ with the
    /// not-actually-math guards; unclosed math becomes a pending token.
    pub(crate) fn ext_latex_inline(&mut self, src: &str) -> Option<Token> {
        let (opening, closing) = if src.starts_with("$$") {
            ("$$", "$$")
        } else if src.starts_with("\\(") {
            ("\\(", "\\)")
        } else if src.starts_with("\\[") {
            ("\\[", "\\]")
        } else if src.starts_with('$') && !latex_dollar_start_regex().is_match(src).unwrap() {
            ("$", "$")
        } else {
            return None;
        };

        // findClosingDelimiter — byte index (helpers); the JS UTF-16 index
        // points at the same character, so the slices below match.
        let closing_index = latex_find_closing_delimiter(src, closing, opening.len());
        if closing_index >= 0 && opening == "$" {
            let inner = &src[opening.len()..closing_index as usize];
            let after = &src[(closing_index as usize + 1).min(src.len())..];
            if latex_trailing_ws_regex().is_match(inner).unwrap()
                || latex_leading_digit_regex().is_match(after).unwrap()
                || (latex_upper_token_regex().is_match(inner).unwrap()
                    && latex_ident_start_regex().is_match(after).unwrap())
                || inner.contains('`')
            {
                return None;
            }
        }

        if closing_index < 0 {
            let pending_source = &src[opening.len()..];
            if opening.starts_with('\\') || latex_looks_like_pending_dollar_math(pending_source) {
                let mut token = Token::new("latex", src);
                token.text = Some(pending_source.to_string());
                token.pending = Some(true);
                return Some(token);
            }
            return None;
        }

        let text = &src[opening.len()..closing_index as usize];
        if text.is_empty() || text.contains('\n') {
            return None;
        }
        let raw = &src[..closing_index as usize + closing.len()];
        let mut token = Token::new("latex", raw);
        token.text = Some(text.to_string());
        Some(token)
    }

    /// The latex extension `start` fn (markdown.ts lines ~104-109): min of
    /// indexOf("$"), indexOf("\\("), indexOf("\\[") — None when all absent.
    /// Returns a BYTE index into `src` (callers slice with it directly).
    pub(crate) fn ext_latex_inline_start(&self, src: &str) -> Option<usize> {
        let mut min: Option<usize> = None;
        for needle in ["$", "\\(", "\\["] {
            if let Some(i) = src.find(needle) {
                min = Some(min.map_or(i, |m: usize| m.min(i)));
            }
        }
        min
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn lex_inline(src: &str) -> String {
        let mut lexer = Lexer::new();
        let list = new_token_list();
        lexer.inline_tokens(src, &list);
        let json = super::super::tokens_to_json(&list.borrow());
        serde_json::to_string(&json).unwrap()
    }

    fn lex_inline_with_links(src: &str, links: &[(&str, &str)]) -> String {
        let mut lexer = Lexer::new();
        for (k, href) in links {
            lexer
                .links
                .insert(k.to_string(), LinkDef { href: href.to_string(), title: None });
        }
        let list = new_token_list();
        lexer.inline_tokens(src, &list);
        let json = super::super::tokens_to_json(&list.borrow());
        serde_json::to_string(&json).unwrap()
    }

    /// Canonical comparison: parse both sides as `serde_json::Value` —
    /// `serde_json::Map` equality is key-order-insensitive, matching the
    /// parity runner's canonical() semantics.
    fn assert_parity(src: &str, expected_ts_json: &str) {
        let got: Value = serde_json::from_str(&lex_inline(src)).unwrap();
        let want: Value = serde_json::from_str(expected_ts_json).unwrap();
        assert_eq!(
            got, want,
            "\nsrc: {:?}\ngot:  {}\nwant: {}",
            src,
            lex_inline(src),
            expected_ts_json
        );
    }

    // Expected outputs captured from the TS harness
    // (parity/ts-harness.js, tool tui-markdown-lex) on 2026-10-18.

    #[test]
    fn emphasis_interplay() {
        assert_parity(
            "*a* **b** ***c*** a*b*c a**b**c",
            r#"[{"type":"em","raw":"*a*","text":"a","tokens":[{"type":"text","raw":"a","text":"a","escaped":false}]},{"type":"text","raw":" ","text":" ","escaped":false},{"type":"strong","raw":"**b**","text":"b","tokens":[{"type":"text","raw":"b","text":"b","escaped":false}]},{"type":"text","raw":" ","text":" ","escaped":false},{"type":"em","raw":"***c***","text":"**c**","tokens":[{"type":"strong","raw":"**c**","text":"c","tokens":[{"type":"text","raw":"c","text":"c","escaped":false}]}]},{"type":"text","raw":" a","text":" a","escaped":false},{"type":"em","raw":"*b*","text":"b","tokens":[{"type":"text","raw":"b","text":"b","escaped":false}]},{"type":"text","raw":"c a","text":"c a","escaped":false},{"type":"strong","raw":"**b**","text":"b","tokens":[{"type":"text","raw":"b","text":"b","escaped":false}]},{"type":"text","raw":"c","text":"c","escaped":false}]"#,
        );
    }

    #[test]
    fn underscore_intraword() {
        assert_parity(
            "snake_case_word and *a_b* _c_",
            r#"[{"type":"text","raw":"snake_case_word and ","text":"snake_case_word and ","escaped":false},{"type":"em","raw":"*a_b*","text":"a_b","tokens":[{"type":"text","raw":"a_b","text":"a_b","escaped":false}]},{"type":"text","raw":" ","text":" ","escaped":false},{"type":"em","raw":"_c_","text":"c","tokens":[{"type":"text","raw":"c","text":"c","escaped":false}]}]"#,
        );
    }

    #[test]
    fn links_images_and_reflink_merge() {
        // [bar][baz] has no link def -> reflink text token merges with the
        // surrounding inlineText runs.
        assert_parity(
            "[foo](/url \"t\") and [bar][baz] and ![img](i.png)",
            r#"[{"type":"link","raw":"[foo](/url \"t\")","href":"/url","title":"t","text":"foo","tokens":[{"type":"text","raw":"foo","text":"foo","escaped":false}]},{"type":"text","raw":" and [bar][baz] and ","text":" and [bar][baz] and ","escaped":false},{"type":"image","raw":"![img](i.png)","href":"i.png","title":null,"text":"img","tokens":[{"type":"text","raw":"img","text":"img","escaped":false}]}]"#,
        );
    }

    #[test]
    fn reflink_with_definition() {
        // Mirrors TS: "[foo]\n\n[foo]: /f" (paragraph tokens only).
        let got: Value = serde_json::from_str(&lex_inline_with_links("[foo]", &[("foo", "/f")])).unwrap();
        let want: Value = serde_json::from_str(
            r#"[{"type":"link","raw":"[foo]","href":"/f","title":null,"text":"foo","tokens":[{"type":"text","raw":"foo","text":"foo","escaped":false}]}]"#,
        )
        .unwrap();
        assert_eq!(got, want);
    }

    #[test]
    fn autolink_and_url() {
        assert_parity(
            "foo@bar.com http://x.com/a?b=1. www.x.com",
            r#"[{"type":"link","raw":"foo@bar.com","text":"foo@bar.com","href":"mailto:foo@bar.com","tokens":[{"type":"text","raw":"foo@bar.com","text":"foo@bar.com"}]},{"type":"text","raw":" ","text":" ","escaped":false},{"type":"link","raw":"http://x.com/a?b=1","text":"http://x.com/a?b=1","href":"http://x.com/a?b=1","tokens":[{"type":"text","raw":"http://x.com/a?b=1","text":"http://x.com/a?b=1"}]},{"type":"text","raw":". ","text":". ","escaped":false},{"type":"link","raw":"www.x.com","text":"www.x.com","href":"http://www.x.com","tokens":[{"type":"text","raw":"www.x.com","text":"www.x.com"}]}]"#,
        );
    }

    #[test]
    fn codespan_escape_tag() {
        // Single backslashes: each \* is a plain escape token, no emphasis.
        assert_parity(
            "`` a `` `x` \\*not\\* <b>x</b>",
            r#"[{"type":"codespan","raw":"`` a ``","text":"a"},{"type":"text","raw":" ","text":" ","escaped":false},{"type":"codespan","raw":"`x`","text":"x"},{"type":"text","raw":" ","text":" ","escaped":false},{"type":"escape","raw":"\\*","text":"*"},{"type":"text","raw":"not","text":"not","escaped":false},{"type":"escape","raw":"\\*","text":"*"},{"type":"text","raw":" ","text":" ","escaped":false},{"type":"html","raw":"<b>","inLink":false,"inRawBlock":false,"block":false,"text":"<b>"},{"type":"text","raw":"x","text":"x","escaped":false},{"type":"html","raw":"</b>","inLink":false,"inRawBlock":false,"block":false,"text":"</b>"}]"#,
        );
    }

    #[test]
    fn em_with_masked_escapes() {
        // Double backslashes: the first \\ pair becomes an escape, and the
        // remaining *...\\* run wins emphasis — this only works because the
        // anyPunctuation (\\x -> ++) and blockSkip masks are applied to the
        // emStrong source before the right-delim scan.
        assert_parity(
            "`` a `` `x` \\\\*not\\\\* <b>x</b>",
            r#"[{"type":"codespan","raw":"`` a ``","text":"a"},{"type":"text","raw":" ","text":" ","escaped":false},{"type":"codespan","raw":"`x`","text":"x"},{"type":"text","raw":" ","text":" ","escaped":false},{"type":"escape","raw":"\\\\","text":"\\"},{"type":"em","raw":"*not\\\\*","text":"not\\\\","tokens":[{"type":"text","raw":"not","text":"not","escaped":false},{"type":"escape","raw":"\\\\","text":"\\"}]},{"type":"text","raw":" ","text":" ","escaped":false},{"type":"html","raw":"<b>","inLink":false,"inRawBlock":false,"block":false,"text":"<b>"},{"type":"text","raw":"x","text":"x","escaped":false},{"type":"html","raw":"</b>","inLink":false,"inRawBlock":false,"block":false,"text":"</b>"}]"#,
        );
    }

    #[test]
    fn tag_raw_block_state() {
        // <pre> flips inRawBlock; the trailing text token inside would carry
        // escaped:true on the TS side (unrepresentable here).
        assert_parity(
            "a <pre>b</pre> `c` d",
            r#"[{"type":"text","raw":"a ","text":"a ","escaped":false},{"type":"html","raw":"<pre>","inLink":false,"inRawBlock":true,"block":false,"text":"<pre>"},{"type":"text","raw":"b","text":"b","escaped":true},{"type":"html","raw":"</pre>","inLink":false,"inRawBlock":false,"block":false,"text":"</pre>"},{"type":"text","raw":" ","text":" ","escaped":false},{"type":"codespan","raw":"`c`","text":"c"},{"type":"text","raw":" d","text":" d","escaped":false}]"#,
        );
    }

    #[test]
    fn strict_strikethrough() {
        assert_parity(
            "a ~~b~~ c ~d~ ~~ e~~f~~",
            r#"[{"type":"text","raw":"a ","text":"a ","escaped":false},{"type":"del","raw":"~~b~~","text":"b","tokens":[{"type":"text","raw":"b","text":"b","escaped":false}]},{"type":"text","raw":" c ~d~ ~~ e","text":" c ~d~ ~~ e","escaped":false},{"type":"del","raw":"~~f~~","text":"f","tokens":[{"type":"text","raw":"f","text":"f","escaped":false}]}]"#,
        );
    }

    #[test]
    fn latex_inline_forms() {
        assert_parity(
            "$x^2$ and $$a$$ and \\(b\\) and \\[c\\] and $5 and 5$",
            r#"[{"type":"latex","raw":"$x^2$","text":"x^2"},{"type":"text","raw":" and ","text":" and ","escaped":false},{"type":"latex","raw":"$$a$$","text":"a"},{"type":"text","raw":" and ","text":" and ","escaped":false},{"type":"latex","raw":"\\(b\\)","text":"b"},{"type":"text","raw":" and ","text":" and ","escaped":false},{"type":"latex","raw":"\\[c\\]","text":"c"},{"type":"text","raw":" and ","text":" and ","escaped":false},{"type":"latex","raw":"$5 and 5$","text":"5 and 5"}]"#,
        );
    }

    #[test]
    fn latex_pending_and_guards() {
        // "$5" is rejected (trailing space before the closing $), then the
        // unclosed "$x_1 unclosed" becomes a pending token.
        assert_parity(
            "price $5 and $x_1 unclosed",
            r#"[{"type":"text","raw":"price $5 and ","text":"price $5 and ","escaped":false},{"type":"latex","raw":"$x_1 unclosed","text":"x_1 unclosed","pending":true}]"#,
        );
        // /^\$\s/ guard: a "$ " opener is not math.
        assert_parity(
            "$ 5$ tail",
            r#"[{"type":"text","raw":"$ 5$ tail","text":"$ 5$ tail","escaped":false}]"#,
        );
        // Trailing space before the closing $ -> not math (codespan wins).
        assert_parity(
            "$a `b` c$ tail",
            r#"[{"type":"text","raw":"$a ","text":"$a ","escaped":false},{"type":"codespan","raw":"`b`","text":"b"},{"type":"text","raw":" c$ tail","text":" c$ tail","escaped":false}]"#,
        );
        // UPPERCASE_ token followed by an identifier -> not math.
        assert_parity(
            "$FOO$bar tail",
            r#"[{"type":"text","raw":"$FOO$bar tail","text":"$FOO$bar tail","escaped":false}]"#,
        );
        // Digit right after the closing $ -> not math.
        assert_parity(
            "a $5$5 b",
            r#"[{"type":"text","raw":"a $5$5 b","text":"a $5$5 b","escaped":false}]"#,
        );
        // Positive: space after the closing $ and non-uppercase inner are
        // fine.
        assert_parity(
            "a $5$ b",
            r#"[{"type":"text","raw":"a ","text":"a ","escaped":false},{"type":"latex","raw":"$5$","text":"5"},{"type":"text","raw":" b","text":" b","escaped":false}]"#,
        );
        assert_parity(
            "$USD 100$ tail",
            r#"[{"type":"latex","raw":"$USD 100$","text":"USD 100"},{"type":"text","raw":" tail","text":" tail","escaped":false}]"#,
        );
    }

    #[test]
    fn em_inside_link_label() {
        // The label goes through outputLink -> inlineTokens recursion, and
        // the * in the label exercises the blockSkip mask on the outer pass.
        assert_parity(
            "[*em*](u) tail",
            r#"[{"type":"link","raw":"[*em*](u)","href":"u","title":null,"text":"*em*","tokens":[{"type":"em","raw":"*em*","text":"em","tokens":[{"type":"text","raw":"em","text":"em","escaped":false}]}]},{"type":"text","raw":" tail","text":" tail","escaped":false}]"#,
        );
    }

    #[test]
    fn link_in_text_detection() {
        let mut lexer = Lexer::new();
        assert!(!lexer.link_in_text("plain text"));
        assert!(lexer.link_in_text("[a](b)"));
        assert!(lexer.link_in_text("x [a](b) y"));
        // An image is exempt ([! ...] guard).
        assert!(!lexer.link_in_text("![a](b)"));
        // Unknown reference label -> not a link.
        assert!(!lexer.link_in_text("[a][b]"));
        lexer.links.insert(
            "b".to_string(),
            LinkDef { href: "/b".to_string(), title: None },
        );
        assert!(lexer.link_in_text("[a][b]"));
    }

    #[test]
    fn latex_start_fn() {
        let lexer = Lexer::new();
        assert_eq!(lexer.ext_latex_inline_start("abc$def"), Some(3));
        assert_eq!(lexer.ext_latex_inline_start("a\\(b"), Some(1));
        assert_eq!(lexer.ext_latex_inline_start("a\\[b"), Some(1));
        assert_eq!(lexer.ext_latex_inline_start("no math here"), None);
        // min of the three
        assert_eq!(lexer.ext_latex_inline_start("\\($x"), Some(0));
    }

    #[test]
    fn js_slice_helpers() {
        assert_eq!(js_slice("hello", 1, -1), "ell");
        assert_eq!(js_slice("hi", 2, -1), "");
        assert_eq!(js_slice("hi", 1, -2), "");
        assert_eq!(js_slice("abc", 0, -1), "ab");
        assert_eq!(first_n_chars("abc", 2), "ab");
        assert_eq!(first_n_chars("abc", 10), "abc");
        assert_eq!(cut_raw("abc", "ab"), "c");
        assert_eq!(cut_raw("abc", "abc"), "");
    }
}
