//! `Lexer.blockTokens` — the block dispatch loop (marked-pretty.js lines
//! 1345–1530) and `infiniteLoopError`. Ground truth: /tmp/marked-pretty.js.
//!
//! Config in effect: gfm=true, breaks=false, pedantic=false, silent=false,
//! no hooks, extensions = LATEX_MARKDOWN_EXTENSIONS only (block: latexBlock,
//! whose `start` fn is the only startBlock entry).

use super::helpers::{js_substring_skip, next_char_boundary};
use super::{Lexer, LinkDef, TokenList};

impl Lexer {
    /// `Lexer.blockTokens(src, tokens = [], n = false)` (marked-pretty.js
    /// 1345–1530). `in_cont` is the JS third argument `n` (continuation
    /// flag, default false) — it gates the paragraph merge path and is
    /// updated to `i.length !== e.length` after each successful paragraph.
    pub(crate) fn block_tokens(&mut self, src: &str, target: &TokenList, mut in_cont: bool) {
        let mut src = src;

        // JS `s = 1/0` min-length tracker (UTF-16 length; we track chars).
        let mut min_len = usize::MAX;

        while !src.is_empty() {
            let src_chars = src.chars().count();
            if src_chars < min_len {
                min_len = src_chars;
            } else {
                // JS e.charCodeAt(0): UTF-16 unit of the first char. For BMP
                // text this equals the code point; non-BMP leads differ but
                // the parity corpus never reaches this guard.
                let code = src.chars().next().map(|c| c as u32).unwrap_or(0);
                self.infinite_loop_error(code);
                break;
            }

            // extensions.block: the latexBlock tokenizer runs FIRST.
            if let Some(tok) = self.ext_latex_block(src) {
                let raw_units = tok.raw.encode_utf16().count();
                src = js_substring_skip(src, raw_units);
                target.borrow_mut().push(tok);
                continue;
            }

            // space: a single-char (newline) match merges into the previous
            // token's raw instead of pushing a space token.
            if let Some(tok) = self.space(src) {
                let raw_units = tok.raw.encode_utf16().count();
                let single = raw_units == 1;
                let mut tg = target.borrow_mut();
                if single && !tg.is_empty() {
                    // JS `o.raw += "\n"` — the raw is exactly "\n".
                    tg.last_mut().unwrap().raw.push('\n');
                } else {
                    tg.push(tok);
                }
                drop(tg);
                src = js_substring_skip(src, raw_units);
                continue;
            }

            // code: paragraph/text continuation appends raw+text and
            // rewrites the LAST inline-queue entry's src.
            if let Some(tok) = self.code(src) {
                let raw_units = tok.raw.encode_utf16().count();
                src = js_substring_skip(src, raw_units);
                let mut tg = target.borrow_mut();
                let last_is_pt = tg
                    .last()
                    .map(|o| o.r#type == "paragraph" || o.r#type == "text")
                    .unwrap_or(false);
                if last_is_pt {
                    let o = tg.last_mut().unwrap();
                    if !o.raw.ends_with('\n') {
                        o.raw.push('\n');
                    }
                    o.raw.push_str(&tok.raw);
                    // JS `o.text += "\n" + r.text` (r.text for code tokens).
                    let r_text = tok.text.clone().unwrap_or_default();
                    let o_text = o.text.get_or_insert_with(String::new);
                    o_text.push('\n');
                    o_text.push_str(&r_text);
                    let new_src = o_text.clone();
                    drop(tg);
                    if let Some(q) = self.inline_queue.last_mut() {
                        q.src = new_src;
                    }
                } else {
                    tg.push(tok);
                }
                continue;
            }

            if let Some(tok) = self.fences(src) {
                let raw_units = tok.raw.encode_utf16().count();
                src = js_substring_skip(src, raw_units);
                target.borrow_mut().push(tok);
                continue;
            }
            if let Some(tok) = self.heading(src) {
                let raw_units = tok.raw.encode_utf16().count();
                src = js_substring_skip(src, raw_units);
                target.borrow_mut().push(tok);
                continue;
            }
            if let Some(tok) = self.hr(src) {
                let raw_units = tok.raw.encode_utf16().count();
                src = js_substring_skip(src, raw_units);
                target.borrow_mut().push(tok);
                continue;
            }
            if let Some(tok) = self.blockquote(src) {
                let raw_units = tok.raw.encode_utf16().count();
                src = js_substring_skip(src, raw_units);
                target.borrow_mut().push(tok);
                continue;
            }
            if let Some(tok) = self.list(src) {
                let raw_units = tok.raw.encode_utf16().count();
                src = js_substring_skip(src, raw_units);
                target.borrow_mut().push(tok);
                continue;
            }
            if let Some(tok) = self.html(src) {
                let raw_units = tok.raw.encode_utf16().count();
                src = js_substring_skip(src, raw_units);
                target.borrow_mut().push(tok);
                continue;
            }

            // def: continuation first; otherwise register in the links
            // registry (first definition wins) and push.
            if let Some(tok) = self.def(src) {
                let raw_units = tok.raw.encode_utf16().count();
                src = js_substring_skip(src, raw_units);
                let mut tg = target.borrow_mut();
                let last_is_pt = tg
                    .last()
                    .map(|o| o.r#type == "paragraph" || o.r#type == "text")
                    .unwrap_or(false);
                if last_is_pt {
                    let o = tg.last_mut().unwrap();
                    if !o.raw.ends_with('\n') {
                        o.raw.push('\n');
                    }
                    o.raw.push_str(&tok.raw);
                    // JS `o.text += "\n" + r.raw` (def uses raw, not text).
                    let r_raw = tok.raw.clone();
                    let o_text = o.text.get_or_insert_with(String::new);
                    o_text.push('\n');
                    o_text.push_str(&r_raw);
                    let new_src = o_text.clone();
                    drop(tg);
                    if let Some(q) = self.inline_queue.last_mut() {
                        q.src = new_src;
                    }
                } else {
                    let tag = tok.tag.clone().unwrap_or_default();
                    if !self.links.contains_key(&tag) {
                        let href = tok.href.clone().unwrap_or_default();
                        let title = tok.title.clone().flatten();
                        self.links.insert(tag, LinkDef { href, title });
                        tg.push(tok);
                    }
                }
                continue;
            }

            if let Some(tok) = self.table(src) {
                let raw_units = tok.raw.encode_utf16().count();
                src = js_substring_skip(src, raw_units);
                target.borrow_mut().push(tok);
                continue;
            }
            if let Some(tok) = self.lheading(src) {
                let raw_units = tok.raw.encode_utf16().count();
                src = js_substring_skip(src, raw_units);
                target.borrow_mut().push(tok);
                continue;
            }

            // startBlock-bounded paragraph source: latexBlock's start fn is
            // called with `e.slice(1)` and returns a byte index `a` into it;
            // the bounded source is the first char of e plus u[..a]
            // (JS `i = e.substring(0, o + 1)`).
            let first_char_len = next_char_boundary(src, 0);
            let u = &src[first_char_len..];
            let i: &str = match self.ext_latex_block_start(u) {
                Some(a) => &src[..first_char_len + a],
                None => src,
            };

            if self.state.top {
                if let Some(tok) = self.paragraph(i) {
                    let raw_units = tok.raw.encode_utf16().count();
                    let mut tg = target.borrow_mut();
                    let last_is_paragraph =
                        tg.last().map(|o| o.r#type == "paragraph").unwrap_or(false);
                    if in_cont && last_is_paragraph {
                        let o = tg.last_mut().unwrap();
                        if !o.raw.ends_with('\n') {
                            o.raw.push('\n');
                        }
                        o.raw.push_str(&tok.raw);
                        let r_text = tok.text.clone().unwrap_or_default();
                        let o_text = o.text.get_or_insert_with(String::new);
                        o_text.push('\n');
                        o_text.push_str(&r_text);
                        let new_src = o_text.clone();
                        drop(tg);
                        // JS: pop the queue entry pushed by this paragraph's
                        // `inline()` call, then rewrite the new last entry.
                        self.inline_queue.pop();
                        if let Some(q) = self.inline_queue.last_mut() {
                            q.src = new_src;
                        }
                    } else {
                        tg.push(tok);
                        drop(tg);
                    }
                    // JS `n = i.length !== e.length` (before e advances).
                    in_cont = i.len() != src.len();
                    src = js_substring_skip(src, raw_units);
                    continue;
                }
            }

            // text: merge into a previous text token (popping this token's
            // queue entry and rewriting the previous one).
            if let Some(tok) = self.text(src) {
                let raw_units = tok.raw.encode_utf16().count();
                src = js_substring_skip(src, raw_units);
                let mut tg = target.borrow_mut();
                let last_is_text = tg.last().map(|o| o.r#type == "text").unwrap_or(false);
                if last_is_text {
                    let o = tg.last_mut().unwrap();
                    if !o.raw.ends_with('\n') {
                        o.raw.push('\n');
                    }
                    o.raw.push_str(&tok.raw);
                    let r_text = tok.text.clone().unwrap_or_default();
                    let o_text = o.text.get_or_insert_with(String::new);
                    o_text.push('\n');
                    o_text.push_str(&r_text);
                    let new_src = o_text.clone();
                    drop(tg);
                    self.inline_queue.pop();
                    if let Some(q) = self.inline_queue.last_mut() {
                        q.src = new_src;
                    }
                } else {
                    tg.push(tok);
                }
                continue;
            }

            // `if (e)` — always true here; marked throws (silent=false).
            let code = src.chars().next().map(|c| c as u32).unwrap_or(0);
            self.infinite_loop_error(code);
            break;
        }

        // JS `return (this.state.top = !0), t`.
        self.state.top = true;
    }

    /// `Lexer.infiniteLoopError(charCode)` — options.silent is false in our
    /// config, so marked THROWS; mirror that with a panic (the parity corpus
    /// never reaches it).
    pub(crate) fn infinite_loop_error(&self, char_code: u32) {
        panic!("Infinite loop on byte: {}", char_code);
    }
}
