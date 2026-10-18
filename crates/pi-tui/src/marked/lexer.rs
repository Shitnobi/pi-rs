//! `Lexer.blockTokens` — the block dispatch loop (marked-pretty.js lines
//! ~1345-1480) and `infiniteLoopError`. Ground truth: /tmp/marked-pretty.js.
//!
//! PORT OWNERSHIP: lexer.rs is filled in by the lexer-dispatch port; do not
//! edit signatures without updating mod.rs/block.rs/inline.rs.

use super::{Lexer, TokenList};

impl Lexer {
    /// `Lexer.blockTokens(src, tokens, top)` — the block dispatch loop:
    /// latexBlock extension first, then space (newline merging), code
    /// (paragraph/text continuation + queue src rewrite), fences, heading,
    /// hr, blockquote, list, html, def (links registry), table, lheading,
    /// startBlock-bounded paragraph (with the in-quote continuation path),
    /// text. `in_cont` is the third JS argument (`n`, default false).
    pub(crate) fn block_tokens(&mut self, src: &str, target: &TokenList, in_cont: bool) {
        let _ = (src, target, in_cont);
        todo!("lexer dispatch port")
    }

    /// `Lexer.infiniteLoopError(charCode)` — options.silent is false in our
    /// config, so marked THROWS; mirror that with a panic (the parity corpus
    /// never reaches it).
    pub(crate) fn infinite_loop_error(&self, char_code: u32) {
        panic!("Infinite loop on byte: {}", char_code);
    }
}
