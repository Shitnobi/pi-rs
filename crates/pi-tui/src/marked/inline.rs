//! Inline tokenizer methods (marked Tokenizer, marked-pretty.js lines
//! ~1084-1333), the inline dispatch loop (`Lexer.inlineTokens`), the latex
//! inline extension, and the StrictStrikethroughTokenizer del() override.
//! Ground truth: /tmp/marked-pretty.js and
//! /tmp/pi-repo/packages/tui/src/components/markdown.ts.
//!
//! PORT OWNERSHIP: inline.rs is filled in by the inline-tokenizer port; do
//! not edit signatures without updating mod.rs/lexer.rs/block.rs.

use std::collections::HashMap;

use super::{Lexer, LinkDef, Token, TokenList};

impl Lexer {
    /// `Tokenizer.escape(src)` (~1084).
    pub(crate) fn escape(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("inline tokenizer port")
    }

    /// `Tokenizer.tag(src)` (~1088).
    pub(crate) fn tag(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("inline tokenizer port")
    }

    /// `Tokenizer.link(src)` (~1113).
    pub(crate) fn link(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("inline tokenizer port")
    }

    /// `Tokenizer.reflink(src, links)` (~1159).
    pub(crate) fn reflink(
        &mut self,
        src: &str,
        links: &HashMap<String, LinkDef>,
    ) -> Option<Token> {
        let _ = (src, links);
        todo!("inline tokenizer port")
    }

    /// `Tokenizer.emStrong(src, maskedSrc, prevChar)` (~1174).
    pub(crate) fn em_strong(
        &mut self,
        src: &str,
        masked_src: &str,
        prev_char: &str,
    ) -> Option<Token> {
        let _ = (src, masked_src, prev_char);
        todo!("inline tokenizer port")
    }

    /// `Tokenizer.codespan(src)` (~1232).
    pub(crate) fn codespan(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("inline tokenizer port")
    }

    /// `Tokenizer.br(src)` (~1246).
    pub(crate) fn br(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("inline tokenizer port")
    }

    /// `Tokenizer.del(src, maskedSrc, prevChar)` (~1250) — OVERRIDDEN by
    /// pi-tui's StrictStrikethroughTokenizer: match
    /// STRICT_STRIKETHROUGH_REGEX, text = match[2], tokens =
    /// lexer.inlineTokens(text). The maskedSrc/prevChar args are ignored by
    /// the override (keep the signature for dispatch fidelity).
    pub(crate) fn del(&mut self, src: &str, masked_src: &str, prev_char: &str) -> Option<Token> {
        let _ = (src, masked_src, prev_char);
        todo!("inline tokenizer port")
    }

    /// `Tokenizer.autolink(src)` (~1286).
    pub(crate) fn autolink(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("inline tokenizer port")
    }

    /// `Tokenizer.url(src)` (~1304).
    pub(crate) fn url(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("inline tokenizer port")
    }

    /// `Tokenizer.inlineText(src)` (~1326).
    pub(crate) fn inline_text(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("inline tokenizer port")
    }

    /// `Lexer.linkInText(src)`.
    pub(crate) fn link_in_text(&self, src: &str) -> bool {
        let _ = src;
        todo!("inline tokenizer port")
    }

    /// `Lexer.inlineTokens(src, tokens)` — the inline dispatch loop
    /// (masked-src preprocessing, extension-first dispatch, escape → tag →
    /// link → reflink → emStrong → codespan → br → del → autolink → url →
    /// inlineText order, prevChar tracking, infinite-loop guard).
    pub(crate) fn inline_tokens(&mut self, src: &str, tokens: &TokenList) {
        let _ = (src, tokens);
        todo!("inline tokenizer port")
    }

    /// `Lexer.inlineTokens(src)` with a fresh target array (used by
    /// emStrong/del/outputLink recursion).
    pub(crate) fn inline_tokens_new(&mut self, src: &str) -> TokenList {
        let _ = src;
        todo!("inline tokenizer port")
    }

    /// The latex inline tokenizer extension (markdown.ts
    /// `tokenizeInlineLatex`), dispatched FIRST in the inline loop.
    pub(crate) fn ext_latex_inline(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("inline tokenizer port")
    }

    /// The latex extension `start` fn (markdown.ts): min of indexOf("$"),
    /// indexOf("\\("), indexOf("\\[") — None when all absent.
    pub(crate) fn ext_latex_inline_start(&self, src: &str) -> Option<usize> {
        let _ = src;
        todo!("inline tokenizer port")
    }
}
