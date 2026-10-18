//! Block tokenizer methods (marked Tokenizer, marked-pretty.js lines
//! ~609-1083) plus the latexBlock extension. Ground truth for the port:
//! /tmp/marked-pretty.js and
//! /tmp/pi-repo/packages/tui/src/components/markdown.ts (tokenizeBlockLatex,
//! LATEX_MARKDOWN_EXTENSIONS block start fn).
//!
//! PORT OWNERSHIP: block.rs is filled in by the block-tokenizer port; do not
//! edit signatures without updating mod.rs/lexer.rs/inline.rs.

use super::{Lexer, Token};

impl Lexer {
    /// `Tokenizer.space(src)` (marked-pretty.js ~612).
    pub(crate) fn space(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// `Tokenizer.code(src)` (~616).
    pub(crate) fn code(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// `Tokenizer.fences(src)` (~624).
    pub(crate) fn fences(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// `Tokenizer.heading(src)` (~639).
    pub(crate) fn heading(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// `Tokenizer.hr(src)` (~663).
    pub(crate) fn hr(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// `Tokenizer.blockquote(src)` (~675).
    pub(crate) fn blockquote(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// `Tokenizer.list(src)` (~759).
    pub(crate) fn list(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// `Tokenizer.html(src)` (~950).
    pub(crate) fn html(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// `Tokenizer.def(src)` (~963).
    pub(crate) fn def(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// `Tokenizer.table(src)` (~992).
    pub(crate) fn table(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// `Tokenizer.lheading(src)` (~1040).
    pub(crate) fn lheading(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// `Tokenizer.paragraph(src)` (~1057).
    pub(crate) fn paragraph(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// `Tokenizer.text(src)` — the BLOCK text tokenizer (~1074).
    pub(crate) fn text(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// The latexBlock tokenizer extension (markdown.ts `tokenizeBlockLatex`),
    /// dispatched FIRST in the block loop (before `space`).
    pub(crate) fn ext_latex_block(&mut self, src: &str) -> Option<Token> {
        let _ = src;
        todo!("block tokenizer port")
    }

    /// The latexBlock extension `start` fn (markdown.ts), used as the
    /// `extensions.startBlock` bound: returns Some(index) where indexing is
    /// into the sliced source the way marked does it (see lexer.rs).
    pub(crate) fn ext_latex_block_start(&self, src: &str) -> Option<usize> {
        let _ = src;
        todo!("block tokenizer port")
    }
}
