//! Block tokenizer methods (marked Tokenizer, marked-pretty.js lines
//! ~609-1083) plus the latexBlock extension. Ground truth for the port:
//! /tmp/marked-pretty.js and
//! /tmp/pi-repo/packages/tui/src/components/markdown.ts (tokenizeBlockLatex,
//! LATEX_MARKDOWN_EXTENSIONS block start fn).
//!
//! PORT OWNERSHIP: block.rs is filled in by the block-tokenizer port; do not
//! edit signatures without updating mod.rs/lexer.rs/inline.rs.
//!
//! Porting notes:
//! - Strict 1:1 with the JS. `regex.exec(src)` -> `regex.captures(src).ok()
//!   .flatten()`; `.test()` -> `.is_match(..).unwrap()`.
//! - JS string `.length` arithmetic (slice/substring/repeat) is ported with
//!   CHAR counts (`chars().count()`, `skip_chars`, `drop_last_chars`). JS
//!   uses UTF-16 code units; for ASCII (and all inputs the marked block
//!   tokenizers slice structurally — prefixes/suffixes of already-matched
//!   text) char counts are identical. Noted per use.
//! - Replace flags verified against the minified original:
//!   codeRemoveIndent=/gm, blockquoteSetextReplace=/g,
//!   blockquoteSetextReplace2=/gm, tableAlignChars=/g,
//!   anyPunctuation=/gu, tabCharGlobal=/g, multipleSpaceGlobal=/g are
//!   GLOBAL (js_replace); tableRowBlankLine, listReplaceTask,
//!   hrefBrackets have no g flag (js_replace_once).
//! - MODEL GAPS in mod.rs's `Token` (not fixable from this file): the JS
//!   `code` token carries `codeBlockStyle: "indented"` and table cells are
//!   `{ text, tokens, header, align }` with NO `type`/`raw` — neither is
//!   representable; see `code()` / `table()` below.

use std::sync::OnceLock;

use fancy_regex::Regex;

use super::helpers::{
    indent_code_compensation, js_replace, js_replace_once, js_search, js_split_first_line,
    js_trim, js_trim_end, latex_looks_like_pending_dollar_math, rtrim, split_cells,
    split_spaces, trim_trailing_blank_lines,
};
use super::{Lexer, NumOrStr, TableCell, Token, TokenList, new_token_list};

// ---------------------------------------------------------------------------
// Local JS-string helpers
// ---------------------------------------------------------------------------

/// JS `s.substring(0, s.length - n)` (char-count semantics; JS would clamp a
/// negative length to 0, hence `saturating_sub`).
fn drop_last_chars(s: &str, n: usize) -> String {
    let keep = s.chars().count().saturating_sub(n);
    s.chars().take(keep).collect()
}

/// JS `s.substring(n)` / `s.slice(n)` for n >= 0 (char-count semantics).
fn skip_chars(s: &str, n: usize) -> String {
    s.chars().skip(n).collect()
}

/// JS `$(s, "\n")` — marked's rtrim: strip trailing newlines.
fn rtrim_newlines(s: &str) -> String {
    rtrim(s, '\n', false)
}

/// JS `e.substring(line.length + 1)` where `line` is the first line of `e`:
/// drop the line plus its newline; an absent newline clamps to "".
fn advance_past_line(e: &str, line_byte_len: usize) -> &str {
    let cut = line_byte_len + 1;
    if cut >= e.len() {
        ""
    } else {
        &e[cut..]
    }
}

/// Build a table cell token. MODEL GAP (mod.rs): marked's table cells are
/// `{ text, tokens, header: bool, align: "left"|"center"|"right"|null }`
/// with NO `type`/`raw` fields, but the Rust `Token` always serializes
/// `type`/`raw` and has no per-cell `header`/`align` fields. We set what the
/// model allows; the missing fields need a mod.rs extension.
fn table_cell(text: &str, tokens: TokenList, header: bool, align: Option<String>) -> TableCell {
    TableCell {
        text: text.to_string(),
        tokens,
        header,
        align,
    }
}

impl Lexer {
    /// `Tokenizer.space(src)` (marked-pretty.js ~612).
    pub(crate) fn space(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.newline.captures(src).ok().flatten()?;
        let m = caps.get(0)?;
        if m.as_str().is_empty() {
            return None; // JS `t[0].length > 0` guard
        }
        Some(Token::new("space", m.as_str()))
    }

    /// `Tokenizer.code(src)` (~616).
    pub(crate) fn code(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.code.captures(src).ok().flatten()?;
        // pedantic=false -> n = te(t[0]) (trimTrailingBlankLines)
        let n = trim_trailing_blank_lines(caps.get(0).unwrap().as_str(), &self.rules.other.blankLine);
        // codeRemoveIndent is /gm — strips up to 4 leading spaces (or a tab)
        // from EVERY line.
        let text = js_replace(&self.rules.other.codeRemoveIndent, &n, |_| String::new());
        let mut token = Token::new("code", n);
        token.text = Some(text);
        // JS: `codeBlockStyle: "indented"` — unconditional on this path
        // (fences() tokens have no such field).
        token.code_block_style = Some("indented".to_string());
        Some(token)
    }

    /// `Tokenizer.fences(src)` (~624).
    pub(crate) fn fences(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.fences.captures(src).ok().flatten()?;
        let raw = caps.get(0).unwrap().as_str().to_string();
        // JS `t[3] || ""` — undefined or empty both become "".
        let content = caps
            .get(3)
            .map(|g| g.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("");
        let text = indent_code_compensation(
            &raw,
            content,
            &self.rules.other.indentCodeCompensation,
            &self.rules.other.beginningSpace,
        );
        // JS `lang: t[2] ? t[2].trim().replace(anyPunctuation, "$1") : t[2]`
        // — t[2] is `([^\n]*)` so it always participates, but "" is falsy
        // and yields lang "" (not undefined).
        let lang = match caps.get(2) {
            None => None,
            Some(m) if m.as_str().is_empty() => Some(String::new()),
            Some(m) => {
                let trimmed = js_trim(m.as_str()).to_string();
                Some(js_replace(&self.rules.inline.anyPunctuation, &trimmed, |c| {
                    c.get(1).map(|g| g.as_str().to_string()).unwrap_or_default()
                }))
            }
        };
        let mut token = Token::new("code", raw);
        token.lang = lang;
        token.text = Some(text);
        Some(token)
    }

    /// `Tokenizer.heading(src)` (~639).
    pub(crate) fn heading(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.heading.captures(src).ok().flatten()?;
        let mut text = js_trim(caps.get(2).unwrap().as_str()).to_string();
        if self.rules.other.endingHash.is_match(&text).unwrap() {
            let s = rtrim(&text, '#', false);
            // JS `(pedantic || !s || endingSpaceChar.test(s)) && (n = s.trim())`
            // with pedantic=false.
            if s.is_empty() || self.rules.other.endingSpaceChar.is_match(&s).unwrap() {
                text = js_trim(&s).to_string();
            }
        }
        let raw = rtrim_newlines(caps.get(0).unwrap().as_str());
        // depth: t[1].length — `#{1,6}` is ASCII, char count == length.
        let depth = caps.get(1).unwrap().as_str().chars().count() as i64;
        let mut token = Token::new("heading", raw);
        token.depth = Some(depth);
        token.text = Some(text.clone());
        token.tokens = Some(self.inline(&text));
        Some(token)
    }

    /// `Tokenizer.hr(src)` (~663).
    pub(crate) fn hr(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.hr.captures(src).ok().flatten()?;
        let raw = rtrim_newlines(caps.get(0).unwrap().as_str());
        Some(Token::new("hr", raw))
    }

    /// `Tokenizer.blockquote(src)` (~675-758).
    pub(crate) fn blockquote(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.blockquote.captures(src).ok().flatten()?;
        let mut lines: Vec<String> = rtrim_newlines(caps.get(0).unwrap().as_str())
            .split('\n')
            .map(|s| s.to_string())
            .collect();
        let mut raw = String::new(); // s
        let mut text = String::new(); // r
        let tokens = new_token_list(); // i
        while !lines.is_empty() {
            let mut has_quote = false; // o
            let mut section: Vec<String> = Vec::new(); // u
            let mut a = 0usize;
            while a < lines.len() {
                if self.rules.other.blockquoteStart.is_match(&lines[a]).unwrap() {
                    section.push(lines[a].clone());
                    has_quote = true;
                } else if !has_quote {
                    section.push(lines[a].clone());
                } else {
                    break;
                }
                a += 1;
            }
            lines = lines.split_off(a);
            let raw_section = section.join("\n"); // p
            // Both replaces are on GLOBAL regexes (see file header).
            let processed = js_replace(
                &self.rules.other.blockquoteSetextReplace,
                &raw_section,
                |c| format!("\n    {}", c.get(1).map(|g| g.as_str()).unwrap_or("")),
            );
            let processed = js_replace(
                &self.rules.other.blockquoteSetextReplace2,
                &processed,
                |_| String::new(),
            );
            // s = s ? `${s}\n${p}` : p (empty string stays falsy in JS)
            if !raw.is_empty() {
                raw.push('\n');
            }
            raw.push_str(&raw_section);
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&processed);
            let saved_top = self.state.top;
            self.state.top = true;
            self.block_tokens(&processed, &tokens, true);
            self.state.top = saved_top;
            if lines.is_empty() {
                break;
            }
            let last = tokens.borrow().last().cloned(); // d = i.at(-1)
            match last.as_ref().map(|d| d.r#type.as_str()) {
                Some("code") => break,
                Some("blockquote") => {
                    let last_bq = last.unwrap(); // O
                    let remaining = lines.join("\n"); // g
                    let recursive_raw = format!(
                        "{}\n{}",
                        last_bq.raw,
                        js_replace(
                            &self.rules.other.blockquoteSetextReplace2,
                            &remaining,
                            |_| String::new()
                        )
                    );
                    // w always starts with the nested blockquote's raw
                    // (which starts with ` {0,3}>`), so the blockquote regex
                    // always matches — JS would crash otherwise.
                    let new_bq = self
                        .blockquote(&recursive_raw)
                        .expect("blockquote continuation re-tokenizes as blockquote");
                    let n = tokens.borrow().len();
                    tokens.borrow_mut()[n - 1] = new_bq.clone();
                    raw.push('\n');
                    raw.push_str(&remaining);
                    // r = r.substring(0, r.length - O.text.length) + E.text
                    // (char-count semantics; O.text is always set here, and
                    // JS `undefined` would clamp to a no-op via NaN.)
                    let cut = last_bq.text.as_deref().map(|t| t.chars().count()).unwrap_or(0);
                    text = drop_last_chars(&text, cut);
                    text.push_str(new_bq.text.as_deref().unwrap_or(""));
                    break;
                }
                Some("list") => {
                    let last_list = last.unwrap(); // O === d
                    let combined = format!("{}\n{}", last_list.raw, lines.join("\n")); // g
                    // g starts with the list token's raw (a list marker), so
                    // the list regex always matches — JS would crash
                    // otherwise.
                    let new_list = self
                        .list(&combined)
                        .expect("list continuation re-tokenizes as list");
                    let n = tokens.borrow().len();
                    tokens.borrow_mut()[n - 1] = new_list.clone();
                    // s = s.substring(0, s.length - d.raw.length) + w.raw
                    let cut = last_list.raw.chars().count();
                    raw = drop_last_chars(&raw, cut);
                    raw.push_str(&new_list.raw);
                    // r = r.substring(0, r.length - O.raw.length) + w.raw
                    text = drop_last_chars(&text, cut);
                    text.push_str(&new_list.raw);
                    // n = g.substring(i.at(-1).raw.length).split("\n")
                    // (i.at(-1) is now w; w.raw is a prefix of g.)
                    let rest = skip_chars(&combined, new_list.raw.chars().count());
                    lines = rest.split('\n').map(|s| s.to_string()).collect();
                    continue;
                }
                _ => {}
            }
        }
        let mut token = Token::new("blockquote", raw);
        token.tokens = Some(tokens);
        token.text = Some(text);
        Some(token)
    }

    /// `Tokenizer.list(src)` (~759-949).
    pub(crate) fn list(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.list.captures(src).ok().flatten()?;
        let mut e = src;
        let marker = js_trim(caps.get(1).unwrap().as_str()).to_string(); // n
        let ordered = marker.chars().count() > 1; // s
        let mut loose = false;
        let mut raw = String::new();
        let mut items: Vec<Token> = Vec::new();
        // n = s ? `\d{1,9}\${n.slice(-1)}` : `\${n}` (the pedantic `[*+-]`
        // fallback is dead: pedantic=false). Marker text is ASCII
        // ([*+-] or [0-9]{1,9}[.)]).
        let marker_pattern = if ordered {
            let delim = marker.chars().last().unwrap();
            format!(r"\d{{1,9}}\{delim}")
        } else {
            format!(r"\{marker}")
        };
        let item_re = self.rules.list_item_regex(&marker_pattern);
        let mut saw_double_blank = false; // o
        while !e.is_empty() {
            let mut ends_with_blank_line; // a
            let mut item_text = String::new(); // c
            // if (!(t = i.exec(e)) || this.rules.block.hr.test(e)) break;
            let item_caps = match item_re.captures(e).ok().flatten() {
                Some(c) => c,
                None => break,
            };
            if self.rules.block.hr.is_match(e).unwrap() {
                break;
            }
            let mut item_raw = item_caps.get(0).unwrap().as_str().to_string(); // p
            e = &e[item_caps.get(0).unwrap().end()..];
            let t1 = item_caps.get(1).unwrap().as_str().to_string();
            let t1_len = t1.chars().count();
            let t2 = item_caps.get(2).map(|g| g.as_str()).unwrap_or("");
            let mut line = split_spaces(js_split_first_line(t2), t1_len); // h
            let next_line = js_split_first_line(e).to_string(); // d
            let mut is_blank = js_trim(&line).is_empty(); // O = !h.trim()
            let indent: i64; // g
            if is_blank {
                indent = t1_len as i64 + 1;
            } else {
                let mut g = js_search(&self.rules.other.nonSpaceChar, &line);
                g = if g > 4 { 1 } else { g };
                // The else-branch implies `line` has a non-whitespace char,
                // which nonSpaceChar ([^ ]) always finds, so g >= 0 here.
                item_text = skip_chars(&line, g.max(0) as usize);
                indent = g + t1_len as i64;
            }
            ends_with_blank_line = false;
            if is_blank && self.rules.other.blankLine.is_match(&next_line).unwrap() {
                item_raw.push_str(&next_line);
                item_raw.push('\n');
                e = advance_past_line(e, next_line.len());
                ends_with_blank_line = true;
            }
            if !ends_with_blank_line {
                let next_bullet = self.rules.next_bullet_regex(indent.max(0) as usize);
                let hr_re = self.rules.hr_regex(indent.max(0) as usize);
                let fences_begin = self.rules.fences_begin_regex(indent.max(0) as usize);
                let heading_begin = self.rules.heading_begin_regex(indent.max(0) as usize);
                let html_begin = self.rules.html_begin_regex(indent.max(0) as usize);
                let blockquote_begin = self.rules.blockquote_begin_regex(indent.max(0) as usize);
                while !e.is_empty() {
                    let cur_line = js_split_first_line(e).to_string(); // N (=== d)
                    let d = &cur_line;
                    // D = d.replace(tabCharGlobal, "    ") — global replace.
                    let dd = js_replace(&self.rules.other.tabCharGlobal, d, |_| "    ".to_string());
                    if fences_begin.is_match(d).unwrap()
                        || heading_begin.is_match(d).unwrap()
                        || html_begin.is_match(d).unwrap()
                        || blockquote_begin.is_match(d).unwrap()
                        || next_bullet.is_match(d).unwrap()
                        || hr_re.is_match(d).unwrap()
                    {
                        break;
                    }
                    if js_search(&self.rules.other.nonSpaceChar, &dd) >= indent
                        || js_trim(d).is_empty()
                    {
                        item_text.push('\n');
                        item_text.push_str(&skip_chars(&dd, indent.max(0) as usize));
                    } else {
                        let line_tabbed = js_replace(
                            &self.rules.other.tabCharGlobal,
                            &line,
                            |_| "    ".to_string(),
                        );
                        if is_blank
                            || js_search(&self.rules.other.nonSpaceChar, &line_tabbed) >= 4
                            || fences_begin.is_match(&line).unwrap()
                            || heading_begin.is_match(&line).unwrap()
                            || hr_re.is_match(&line).unwrap()
                        {
                            break;
                        }
                        item_text.push('\n');
                        item_text.push_str(d);
                    }
                    is_blank = js_trim(d).is_empty();
                    item_raw.push_str(&cur_line);
                    item_raw.push('\n');
                    e = advance_past_line(e, cur_line.len());
                    line = skip_chars(&dd, indent.max(0) as usize);
                }
            }
            // r.loose || (o ? (r.loose = true) : doubleBlankLine.test(p) && (o = true))
            if !loose {
                if saw_double_blank {
                    loose = true;
                } else if self.rules.other.doubleBlankLine.is_match(&item_raw).unwrap() {
                    saw_double_blank = true;
                }
            }
            let mut item = Token::new("list_item", item_raw.clone());
            // task: !!gfm && listIsTask.test(c) — gfm=true in our config.
            item.task = Some(self.rules.other.listIsTask.is_match(&item_text).unwrap());
            item.loose = Some(false);
            item.text = Some(item_text);
            item.tokens = Some(new_token_list());
            items.push(item);
            raw.push_str(&item_raw);
        }
        // let u = r.items.at(-1); if (u) { trimEnd both } else return;
        let Some(last_item) = items.last_mut() else {
            return None;
        };
        last_item.raw = js_trim_end(&last_item.raw).to_string();
        if let Some(t) = last_item.text.as_mut() {
            // JS trimEnd; Rust trim_end trims the same practical set.
            *t = js_trim_end(t).to_string();
        }
        let raw = js_trim_end(&raw).to_string();
        for item in items.iter_mut() {
            self.state.top = false;
            let text = item.text.clone().unwrap_or_default();
            let arr = new_token_list();
            self.block_tokens(&text, &arr, false);
            item.tokens = Some(arr);
            if !loose {
                // p = a.tokens.filter(space); c = p.length > 0 && p.some(anyLine)
                let c = item
                    .tokens
                    .as_ref()
                    .unwrap()
                    .borrow()
                    .iter()
                    .filter(|h| h.r#type == "space")
                    .any(|h| self.rules.other.anyLine.is_match(&h.raw).unwrap_or(false));
                loose = c;
            }
        }
        for item in items.iter_mut() {
            if item.task != Some(true) {
                continue; // JS `else a.task && (a.task = false)` — no-op here
            }
            let tokens_list = item.tokens.clone().expect("list item tokens");
            let first_type = tokens_list.borrow().first().map(|t| t.r#type.clone());
            if !matches!(first_type.as_deref(), Some("text") | Some("paragraph")) {
                // a.task && (a.task = false)
                item.task = Some(false);
                continue;
            }
            // a.text / p.raw / p.text replace (listReplaceTask has no g flag)
            if let Some(t) = item.text.as_mut() {
                *t = js_replace_once(&self.rules.other.listReplaceTask, t, |_| String::new());
            }
            {
                let mut tl = tokens_list.borrow_mut();
                let p = &mut tl[0];
                p.raw = js_replace_once(&self.rules.other.listReplaceTask, &p.raw, |_| {
                    String::new()
                });
                if let Some(t) = p.text.as_mut() {
                    *t = js_replace_once(&self.rules.other.listReplaceTask, t, |_| String::new());
                }
            }
            // Backward rewrite of an EARLIER inlineQueue entry's src.
            for h in (0..self.inline_queue.len()).rev() {
                if self.rules.other.listIsTask.is_match(&self.inline_queue[h].src).unwrap() {
                    self.inline_queue[h].src = js_replace_once(
                        &self.rules.other.listReplaceTask,
                        &self.inline_queue[h].src,
                        |_| String::new(),
                    );
                    break;
                }
            }
            let checkbox_match = self
                .rules
                .other
                .listTaskCheckbox
                .captures(&item.raw)
                .ok()
                .flatten();
            if let Some(cc) = checkbox_match {
                let m0 = cc.get(0).unwrap().as_str().to_string();
                let checkbox_raw = format!("{m0} ");
                let checked = m0 != "[ ]";
                item.checked = Some(checked);
                let mut checkbox = Token::new("checkbox", checkbox_raw.clone());
                checkbox.checked = Some(checked);
                if loose {
                    let mut tl = tokens_list.borrow_mut();
                    // a.tokens[0] && ["paragraph","text"].includes(type) &&
                    // "tokens" in tok && tok.tokens (an array is always
                    // truthy in JS when the property is set)
                    let first_is_para_text = tl
                        .first()
                        .map(|t| t.r#type == "paragraph" || t.r#type == "text")
                        .unwrap_or(false)
                        && tl.first().map(|t| t.tokens.is_some()).unwrap_or(false);
                    if first_is_para_text {
                        let p = &mut tl[0];
                        p.raw = format!("{checkbox_raw}{}", p.raw);
                        if let Some(t) = p.text.as_mut() {
                            *t = format!("{checkbox_raw}{t}");
                        }
                        if let Some(pt) = p.tokens.as_ref() {
                            pt.borrow_mut().insert(0, checkbox);
                        }
                    } else {
                        let mut para = Token::new("paragraph", checkbox_raw.clone());
                        para.text = Some(checkbox_raw.clone());
                        let inner = new_token_list();
                        inner.borrow_mut().push(checkbox);
                        para.tokens = Some(inner);
                        tl.insert(0, para);
                    }
                } else {
                    tokens_list.borrow_mut().insert(0, checkbox);
                }
            }
        }
        if loose {
            for item in items.iter_mut() {
                item.loose = Some(true);
                if let Some(tl) = item.tokens.as_ref() {
                    for p in tl.borrow_mut().iter_mut() {
                        if p.r#type == "text" {
                            p.r#type = "paragraph".to_string();
                        }
                    }
                }
            }
        }
        let mut token = Token::new("list", raw);
        token.ordered = Some(ordered);
        token.start = Some(if ordered {
            // JS `+n.slice(0, -1)` — the marker is [0-9]{1,9} + delimiter,
            // so this always parses (JS NaN -> null is unreachable).
            NumOrStr::Num(marker[..marker.len() - 1].parse().unwrap_or(0))
        } else {
            NumOrStr::Str(String::new())
        });
        token.loose = Some(loose);
        token.items = Some(items);
        Some(token)
    }

    /// `Tokenizer.html(src)` (~950).
    pub(crate) fn html(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.html.captures(src).ok().flatten()?;
        let n = trim_trailing_blank_lines(caps.get(0).unwrap().as_str(), &self.rules.other.blankLine);
        let pre = caps
            .get(1)
            .map(|g| matches!(g.as_str(), "pre" | "script" | "style"))
            .unwrap_or(false);
        let mut token = Token::new("html", n.clone());
        token.block = Some(true);
        token.pre = Some(pre);
        token.text = Some(n);
        Some(token)
    }

    /// `Tokenizer.def(src)` (~963). The links registry continuation lives in
    /// lexer.rs; this only builds the token.
    pub(crate) fn def(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.def.captures(src).ok().flatten()?;
        // n = t[1].toLowerCase().replace(multipleSpaceGlobal, " ")
        let tag = js_replace(
            &self.rules.other.multipleSpaceGlobal,
            &caps.get(1).unwrap().as_str().to_lowercase(),
            |_| " ".to_string(),
        );
        // s = t[2] ? t[2].replace(hrefBrackets, "$1").replace(anyPunctuation, "$1") : ""
        let href = match caps.get(2) {
            Some(m) => {
                let s = js_replace_once(&self.rules.other.hrefBrackets, m.as_str(), |c| {
                    c.get(1).map(|g| g.as_str().to_string()).unwrap_or_default()
                });
                js_replace(&self.rules.inline.anyPunctuation, &s, |c| {
                    c.get(1).map(|g| g.as_str().to_string()).unwrap_or_default()
                })
            }
            None => String::new(),
        };
        // r = t[3] ? t[3].substring(1, t[3].length - 1).replace(anyPunctuation, "$1") : t[3]
        // — t[3] is either a quoted string or undefined (never ""), so the
        // title field is either a string or absent.
        let title = caps.get(3).map(|m| {
            let s = m.as_str();
            let n = s.chars().count();
            let inner: String = s.chars().skip(1).take(n.saturating_sub(2)).collect();
            Some(js_replace(&self.rules.inline.anyPunctuation, &inner, |c| {
                c.get(1).map(|g| g.as_str().to_string()).unwrap_or_default()
            }))
        });
        let mut token = Token::new("def", rtrim_newlines(caps.get(0).unwrap().as_str()));
        token.tag = Some(tag);
        token.href = Some(href);
        token.title = title;
        Some(token)
    }

    /// `Tokenizer.table(src)` (~992-1039).
    pub(crate) fn table(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.table.captures(src).ok().flatten()?;
        let t2 = caps.get(2).unwrap().as_str();
        if !self.rules.other.tableDelimiter.is_match(t2).unwrap() {
            return None;
        }
        let header_cells = split_cells(
            caps.get(1).unwrap().as_str(),
            None,
            &self.rules.other.findPipe,
            &self.rules.other.slashPipe,
        );
        // s = t[2].replace(tableAlignChars, "").split("|") — tableAlignChars
        // is global.
        let align_row: Vec<String> =
            js_replace(&self.rules.other.tableAlignChars, t2, |_| String::new())
                .split('|')
                .map(|s| s.to_string())
                .collect();
        // r = t[3]?.trim() ? t[3].replace(tableRowBlankLine, "").split("\n") : []
        // (tableRowBlankLine has no g flag)
        let body_lines: Vec<String> = match caps.get(3).map(|m| m.as_str()) {
            Some(s) if !js_trim(s).is_empty() => js_replace_once(
                &self.rules.other.tableRowBlankLine,
                s,
                |_| String::new(),
            )
            .split('\n')
            .map(|s| s.to_string())
            .collect(),
            _ => Vec::new(),
        };
        if header_cells.len() == align_row.len() {
            let mut align: Vec<Option<String>> = Vec::new();
            for o in &align_row {
                if self.rules.other.tableAlignRight.is_match(o).unwrap() {
                    align.push(Some("right".into()));
                } else if self.rules.other.tableAlignCenter.is_match(o).unwrap() {
                    align.push(Some("center".into()));
                } else if self.rules.other.tableAlignLeft.is_match(o).unwrap() {
                    align.push(Some("left".into()));
                } else {
                    align.push(None);
                }
            }
            let mut header: Vec<TableCell> = Vec::new();
            for o in 0..header_cells.len() {
                header.push(table_cell(
                    &header_cells[o],
                    self.inline(&header_cells[o]),
                    true,
                    align[o].clone(),
                ));
            }
            let mut rows: Vec<Vec<TableCell>> = Vec::new();
            for o in &body_lines {
                let cells = split_cells(
                    o,
                    Some(header.len()),
                    &self.rules.other.findPipe,
                    &self.rules.other.slashPipe,
                );
                let mut row: Vec<TableCell> = Vec::new();
                for (a, u) in cells.iter().enumerate() {
                    row.push(table_cell(u, self.inline(u), false, align[a].clone()));
                }
                rows.push(row);
            }
            let mut token = Token::new("table", rtrim_newlines(caps.get(0).unwrap().as_str()));
            token.header = Some(header);
            token.align = Some(align);
            token.rows = Some(rows);
            Some(token)
        } else {
            None
        }
    }

    /// `Tokenizer.lheading(src)` (~1040).
    pub(crate) fn lheading(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.lheading.captures(src).ok().flatten()?;
        let text = js_trim(caps.get(1).unwrap().as_str()).to_string();
        let depth = if caps.get(2).and_then(|m| m.as_str().chars().next()) == Some('=') {
            1
        } else {
            2
        };
        let mut token = Token::new("heading", rtrim_newlines(caps.get(0).unwrap().as_str()));
        token.depth = Some(depth);
        token.text = Some(text.clone());
        token.tokens = Some(self.inline(&text));
        Some(token)
    }

    /// `Tokenizer.paragraph(src)` (~1057).
    pub(crate) fn paragraph(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.paragraph.captures(src).ok().flatten()?;
        let t1 = caps.get(1).unwrap().as_str();
        let text = if t1.chars().next_back() == Some('\n') {
            // t[1].slice(0, -1) — drop the last char
            drop_last_chars(t1, 1)
        } else {
            t1.to_string()
        };
        let mut token = Token::new("paragraph", caps.get(0).unwrap().as_str().to_string());
        token.text = Some(text.clone());
        token.tokens = Some(self.inline(&text));
        Some(token)
    }

    /// `Tokenizer.text(src)` — the BLOCK text tokenizer (~1074).
    pub(crate) fn text(&mut self, src: &str) -> Option<Token> {
        let caps = self.rules.block.text.captures(src).ok().flatten()?;
        let m0 = caps.get(0).unwrap().as_str();
        let mut token = Token::new("text", m0);
        token.text = Some(m0.to_string());
        token.tokens = Some(self.inline(m0));
        Some(token)
    }

    /// The latexBlock tokenizer extension (markdown.ts `tokenizeBlockLatex`,
    /// ~99-120), dispatched FIRST in the block loop (before `space`).
    pub(crate) fn ext_latex_block(&mut self, src: &str) -> Option<Token> {
        static DOLLAR: OnceLock<Regex> = OnceLock::new();
        static BRACKET: OnceLock<Regex> = OnceLock::new();
        static PENDING_BRACKET: OnceLock<Regex> = OnceLock::new();
        static PENDING_DOLLAR: OnceLock<Regex> = OnceLock::new();
        let dollar_re = DOLLAR.get_or_init(|| {
            Regex::new(r"^ {0,3}\$\$[ \t]*(?:\n)?([\s\S]*?)\$\$[ \t]*(?:\n|$)").unwrap()
        });
        let bracket_re = BRACKET.get_or_init(|| {
            Regex::new(r"^ {0,3}\\\[[ \t]*(?:\n)?([\s\S]*?)\\\][ \t]*(?:\n|$)").unwrap()
        });
        let pending_bracket_re = PENDING_BRACKET
            .get_or_init(|| Regex::new(r"^ {0,3}\\\[[ \t]*(?:\n)?([\s\S]*)$").unwrap());
        let pending_dollar_re = PENDING_DOLLAR
            .get_or_init(|| Regex::new(r"^ {0,3}\$\$[ \t]*(?:\n)?([\s\S]*)$").unwrap());

        let dollar_match = dollar_re.captures(src).ok().flatten();
        if let Some(c) = &dollar_match {
            if let Some(g) = c.get(1).filter(|g| !g.as_str().is_empty()) {
                let mut token = Token::new("latexBlock", c.get(0).unwrap().as_str());
                // JS String.prototype.trim trims WhiteSpace+LineTerminator —
                // the same practical set as Rust str::trim.
                token.text = Some(js_trim(g.as_str()).to_string());
                return Some(token);
            }
        }
        let bracket_match = bracket_re.captures(src).ok().flatten();
        if let Some(c) = &bracket_match {
            if let Some(g) = c.get(1).filter(|g| !g.as_str().is_empty()) {
                let mut token = Token::new("latexBlock", c.get(0).unwrap().as_str());
                token.text = Some(js_trim(g.as_str()).to_string());
                return Some(token);
            }
        }
        if let Some(c) = pending_bracket_re.captures(src).ok().flatten() {
            let mut token = Token::new("latexBlock", c.get(0).unwrap().as_str());
            token.text = Some(c.get(1).map(|g| g.as_str()).unwrap_or("").to_string());
            token.pending = Some(true);
            return Some(token);
        }
        if let Some(c) = pending_dollar_re.captures(src).ok().flatten() {
            if let Some(g) = c.get(1).filter(|g| !g.as_str().is_empty()) {
                if latex_looks_like_pending_dollar_math(g.as_str()) {
                    let mut token = Token::new("latexBlock", c.get(0).unwrap().as_str());
                    token.text = Some(g.as_str().to_string());
                    token.pending = Some(true);
                    return Some(token);
                }
            }
        }
        None
    }

    /// The latexBlock extension `start` fn (markdown.ts), used as the
    /// `extensions.startBlock` bound: returns Some(byte index into `src`).
    /// The dispatcher calls this with the source minus its first character;
    /// the returned index is relative to that sliced source (both the regex
    /// match offset and the +1 after "\n" land on char boundaries).
    pub(crate) fn ext_latex_block_start(&self, src: &str) -> Option<usize> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re =
            RE.get_or_init(|| Regex::new(r"(?:^|\n) {0,3}(?:\$\$|\\\[)").unwrap());
        let m = re.find(src).ok().flatten()?;
        let mut index = m.start();
        if m.as_str().starts_with('\n') {
            index += 1;
        }
        Some(index)
    }
}

#[cfg(test)]
mod tests {
    // Leaf tokenizer tests only — nothing here transitively calls the
    // todo!() dispatch loops (block_tokens lives in lexer.rs). Methods used
    // by these tests only call self.inline(), which is implemented.
    use super::*;

    #[test]
    fn space_basic() {
        let mut lex = Lexer::new();
        let t = lex.space("\n\nfoo").unwrap();
        assert_eq!(t.r#type, "space");
        assert_eq!(t.raw, "\n\n");
        assert!(lex.space("foo").is_none());
        assert!(lex.space("").is_none()); // empty match -> undefined
    }

    #[test]
    fn code_indented() {
        let mut lex = Lexer::new();
        let t = lex.code("    indented code\n").unwrap();
        assert_eq!(t.r#type, "code");
        assert_eq!(t.raw, "    indented code\n");
        assert_eq!(t.text.as_deref(), Some("indented code\n"));
    }

    #[test]
    fn fences_lang_unescape() {
        // Ground truth (ts-harness): lang "js! lang", text "_code_".
        let mut lex = Lexer::new();
        let t = lex.fences("``` js\\! lang\n_code_\n```\n").unwrap();
        assert_eq!(t.r#type, "code");
        // t[0] from the fences regex (the trailing "\n" seen in full-lexer
        // output is added by Lexer.blockTokens' cutSrc logic, not here).
        assert_eq!(t.raw, "``` js\\! lang\n_code_\n```");
        assert_eq!(t.lang.as_deref(), Some("js! lang"));
        assert_eq!(t.text.as_deref(), Some("_code_"));
    }

    #[test]
    fn hr_basic() {
        let mut lex = Lexer::new();
        let t = lex.hr("---\n\n").unwrap();
        assert_eq!(t.r#type, "hr");
        assert_eq!(t.raw, "---");
    }

    #[test]
    fn heading_trailing_hash() {
        let mut lex = Lexer::new();
        let t = lex.heading("## hi ##\n").unwrap();
        assert_eq!(t.depth, Some(2));
        assert_eq!(t.text.as_deref(), Some("hi"));
        assert_eq!(t.raw, "## hi ##");
    }

    #[test]
    fn lheading_depth() {
        let mut lex = Lexer::new();
        let t = lex.lheading("Title\n=====\n").unwrap();
        assert_eq!(t.r#type, "heading");
        assert_eq!(t.depth, Some(1));
        assert_eq!(t.text.as_deref(), Some("Title"));
        assert_eq!(t.raw, "Title\n=====");
        let t2 = lex.lheading("Sub\n---\n").unwrap();
        assert_eq!(t2.depth, Some(2));
    }

    #[test]
    fn def_with_and_without_title() {
        let mut lex = Lexer::new();
        let t = lex.def("[ref]: http://x.com \"T\"\n").unwrap();
        assert_eq!(t.r#type, "def");
        assert_eq!(t.tag.as_deref(), Some("ref"));
        assert_eq!(t.href.as_deref(), Some("http://x.com"));
        assert_eq!(t.title, Some(Some("T".to_string())));
        assert_eq!(t.raw, "[ref]: http://x.com \"T\"");
        let t2 = lex.def("[ref2]: <http://y.com>\n").unwrap();
        assert_eq!(t2.href.as_deref(), Some("http://y.com"));
        assert_eq!(t2.title, None);
    }

    #[test]
    fn paragraph_trailing_newline() {
        let mut lex = Lexer::new();
        let t = lex.paragraph("text\n").unwrap();
        assert_eq!(t.r#type, "paragraph");
        // t[0]/t[1] from the paragraph regex; the trailing "\n" in full-lexer
        // output is the lexer's cutSrc rewrite (lexer.rs), not the tokenizer.
        assert_eq!(t.raw, "text");
        assert_eq!(t.text.as_deref(), Some("text"));
        assert_eq!(lex.inline_queue.len(), 1);
        assert_eq!(lex.inline_queue[0].src, "text");
    }

    #[test]
    fn text_block() {
        let mut lex = Lexer::new();
        let t = lex.text("some text").unwrap();
        assert_eq!(t.r#type, "text");
        assert_eq!(t.raw, "some text");
        assert_eq!(t.text.as_deref(), Some("some text"));
    }

    #[test]
    fn latex_block_complete() {
        let mut lex = Lexer::new();
        let t = lex.ext_latex_block("$$ x^2 $$\n").unwrap();
        assert_eq!(t.r#type, "latexBlock");
        assert_eq!(t.raw, "$$ x^2 $$\n");
        assert_eq!(t.text.as_deref(), Some("x^2"));
        assert_eq!(t.pending, None);
        let t2 = lex.ext_latex_block("\\[ \\frac{1}{2} \\]\n").unwrap();
        assert_eq!(t2.text.as_deref(), Some("\\frac{1}{2}"));
        assert_eq!(t2.pending, None);
    }

    #[test]
    fn latex_block_pending() {
        let mut lex = Lexer::new();
        let t = lex.ext_latex_block("\\[ \\frac{1}{2}\n").unwrap();
        assert_eq!(t.pending, Some(true));
        assert_eq!(t.text.as_deref(), Some("\\frac{1}{2}\n"));
        let t2 = lex.ext_latex_block("$$ x_1\n").unwrap();
        assert_eq!(t2.pending, Some(true));
        assert_eq!(t2.text.as_deref(), Some("x_1\n"));
        // "$$ plain words" is not pending-dollar-math.
        assert!(lex.ext_latex_block("$$ plain words\n").is_none());
    }

    #[test]
    fn latex_block_start() {
        let lex = Lexer::new();
        assert_eq!(lex.ext_latex_block_start("$$x"), Some(0));
        assert_eq!(lex.ext_latex_block_start("hello\n\n  $$x"), Some(7));
        assert_eq!(lex.ext_latex_block_start("plain"), None);
    }
}
