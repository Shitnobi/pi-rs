//! Hand-port of marked v18.0.11's Lexer + Tokenizer (the subset configured by
//! pi-tui's `markdownParser`): default options `gfm: true, breaks: false,
//! pedantic: false, silent: false`, the `StrictStrikethroughTokenizer` del()
//! override, and the two LATEX_MARKDOWN_EXTENSIONS (block + inline).
//!
//! Ground truth: /tmp/pi-repo/node_modules/marked/lib/marked.esm.js
//! (prettified copy at /tmp/marked-pretty.js) and
//! /tmp/pi-repo/packages/tui/src/components/markdown.ts.
//!
//! Architecture notes (JS -> Rust):
//! - marked's `inlineQueue` entries hold `(src, tokens-array)` references into
//!   the token tree; we mirror that object graph with `Rc<RefCell<Vec<Token>>>`
//!   (`TokenList`) so queue pushes/mutations (including the list tokenizer's
//!   backward `inlineQueue[h].src` rewrite) translate 1:1.
//! - Tokenizer methods live on `Lexer` (TS splits them into a Tokenizer class
//!   only for extensibility); the two pi-tui extensions and the strict-del
//!   override are hardcoded dispatch points in `lexer.rs` / `inline.rs`.
//! - The rules were dumped post-`k()`-chain from Node (see `rules.rs`).
//!
//! Differential gate: parity/waved/markdown-lex-cases.mjs (124 cases, tool
//! `tui-markdown-lex`, fn `lex`).

mod block;
mod helpers;
mod inline;
mod lexer;
mod rules;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{Map, Value};

pub use rules::Rules;

/// A mutable, shared token array — the Rust analogue of a JS `Token[]` that
/// the inline queue and nested block tokenization write into.
pub type TokenList = Rc<RefCell<Vec<Token>>>;

pub fn new_token_list() -> TokenList {
    Rc::new(RefCell::new(Vec::new()))
}

/// `start` on list tokens is either a number (ordered start) or `""`.
#[derive(Debug, Clone, PartialEq)]
pub enum NumOrStr {
    Num(i64),
    Str(String),
}

/// Tri-state for `title`: `None` = field absent (TS `undefined`), `Some(None)`
/// = JSON `null` (TS `title: null` from `e.title || null`), `Some(Some(s))` =
/// string.
pub type OptStr = Option<String>;/// A marked token. One struct with optional fields: `JSON.stringify` on the
/// TS side drops `undefined` fields, and `Token::to_json` skips `None` fields
/// to match that contract exactly.
#[derive(Debug, Clone, Default)]
pub struct Token {
    pub r#type: String,
    pub raw: String,
    pub text: Option<String>,
    pub lang: Option<String>,
    pub depth: Option<i64>,
    pub ordered: Option<bool>,
    pub start: Option<NumOrStr>,
    pub loose: Option<bool>,
    pub task: Option<bool>,
    pub checked: Option<bool>,
    pub items: Option<Vec<Token>>,
    pub header: Option<Vec<TableCell>>,
    pub rows: Option<Vec<Vec<TableCell>>>,
    pub align: Option<Vec<Option<String>>>,
    pub tokens: Option<TokenList>,
    pub href: Option<String>,
    pub title: Option<OptStr>,
    pub tag: Option<String>,
    pub pre: Option<bool>,
    pub block: Option<bool>,
    pub pending: Option<bool>,
    /// inlineText tokens: `escaped: this.lexer.state.inRawBlock`.
    pub escaped: Option<bool>,
    /// inline html (tag) tokens: `inLink` / `inRawBlock` post-state-dance.
    pub in_link: Option<bool>,
    pub in_raw_block: Option<bool>,
    /// Indented code blocks: `codeBlockStyle: "indented"`.
    pub code_block_style: Option<String>,
}

/// A table cell — marked v18 table cells are `{text, tokens, header, align}`
/// objects (no type/raw), unlike every other token shape.
#[derive(Debug, Clone)]
pub struct TableCell {
    pub text: String,
    pub tokens: TokenList,
    pub header: bool,
    /// `"left" | "center" | "right"` or `None` (JSON `null`).
    pub align: Option<String>,
}

impl TableCell {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("text".into(), Value::String(self.text.clone()));
        m.insert(
            "tokens".into(),
            Value::Array(self.tokens.borrow().iter().map(Token::to_json).collect()),
        );
        m.insert("header".into(), Value::Bool(self.header));
        m.insert(
            "align".into(),
            match &self.align {
                Some(s) => Value::String(s.clone()),
                None => Value::Null,
            },
        );
        Value::Object(m)
    }

    pub fn deep_clone(&self) -> TableCell {
        TableCell {
            text: self.text.clone(),
            tokens: new_token_list_with(
                self.tokens.borrow().iter().map(Token::deep_clone).collect(),
            ),
            header: self.header,
            align: self.align.clone(),
        }
    }
}

impl Token {
    pub fn new(r#type: &str, raw: impl Into<String>) -> Token {
        Token {
            r#type: r#type.to_string(),
            raw: raw.into(),
            ..Token::default()
        }
    }

    /// Serialize to JSON matching `JSON.stringify(token)`: `type`/`raw` are
    /// always present, every other field only when `Some` (with
    /// `Some(None)`-style titles emitting `null`).
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("type".into(), Value::String(self.r#type.clone()));
        m.insert("raw".into(), Value::String(self.raw.clone()));
        if let Some(v) = &self.text {
            m.insert("text".into(), Value::String(v.clone()));
        }
        if let Some(v) = &self.lang {
            m.insert("lang".into(), Value::String(v.clone()));
        }
        if let Some(v) = self.depth {
            m.insert("depth".into(), Value::Number(v.into()));
        }
        if let Some(v) = self.ordered {
            m.insert("ordered".into(), Value::Bool(v));
        }
        if let Some(v) = &self.start {
            m.insert(
                "start".into(),
                match v {
                    NumOrStr::Num(n) => Value::Number((*n).into()),
                    NumOrStr::Str(s) => Value::String(s.clone()),
                },
            );
        }
        if let Some(v) = self.loose {
            m.insert("loose".into(), Value::Bool(v));
        }
        if let Some(v) = self.task {
            m.insert("task".into(), Value::Bool(v));
        }
        if let Some(v) = self.checked {
            m.insert("checked".into(), Value::Bool(v));
        }
        if let Some(v) = &self.items {
            m.insert("items".into(), Value::Array(v.iter().map(Self::to_json).collect()));
        }
        if let Some(v) = &self.header {
            m.insert(
                "header".into(),
                Value::Array(v.iter().map(TableCell::to_json).collect()),
            );
        }
        if let Some(v) = &self.rows {
            m.insert(
                "rows".into(),
                Value::Array(
                    v.iter()
                        .map(|row| Value::Array(row.iter().map(TableCell::to_json).collect()))
                        .collect(),
                ),
            );
        }
        if let Some(v) = &self.align {
            m.insert(
                "align".into(),
                Value::Array(
                    v.iter()
                        .map(|a| match a {
                            Some(s) => Value::String(s.clone()),
                            None => Value::Null,
                        })
                        .collect(),
                ),
            );
        }
        if let Some(v) = &self.tokens {
            m.insert(
                "tokens".into(),
                Value::Array(v.borrow().iter().map(Self::to_json).collect()),
            );
        }
        if let Some(v) = &self.href {
            m.insert("href".into(), Value::String(v.clone()));
        }
        if let Some(v) = &self.title {
            m.insert(
                "title".into(),
                match v {
                    None => Value::Null,
                    Some(s) => Value::String(s.clone()),
                },
            );
        }
        if let Some(v) = &self.tag {
            m.insert("tag".into(), Value::String(v.clone()));
        }
        if let Some(v) = self.pre {
            m.insert("pre".into(), Value::Bool(v));
        }
        if let Some(v) = self.block {
            m.insert("block".into(), Value::Bool(v));
        }
        if let Some(v) = self.pending {
            m.insert("pending".into(), Value::Bool(v));
        }
        if let Some(v) = self.escaped {
            m.insert("escaped".into(), Value::Bool(v));
        }
        if let Some(v) = self.in_link {
            m.insert("inLink".into(), Value::Bool(v));
        }
        if let Some(v) = self.in_raw_block {
            m.insert("inRawBlock".into(), Value::Bool(v));
        }
        if let Some(v) = &self.code_block_style {
            m.insert("codeBlockStyle".into(), Value::String(v.clone()));
        }
        Value::Object(m)
    }

    /// Deep clone (a derived Clone would share the `Rc` token lists).
    pub fn deep_clone(&self) -> Token {
        let mut t = self.clone_shallow_owned();
        if let Some(items) = &self.items {
            t.items = Some(items.iter().map(Self::deep_clone).collect());
        }
        if let Some(header) = &self.header {
            t.header = Some(header.iter().map(TableCell::deep_clone).collect());
        }
        if let Some(rows) = &self.rows {
            t.rows = Some(
                rows.iter()
                    .map(|row| row.iter().map(TableCell::deep_clone).collect())
                    .collect(),
            );
        }
        if let Some(list) = &self.tokens {
            t.tokens = Some(new_token_list_with(
                list.borrow().iter().map(Self::deep_clone).collect(),
            ));
        }
        t
    }

    fn clone_shallow_owned(&self) -> Token {
        Token {
            r#type: self.r#type.clone(),
            raw: self.raw.clone(),
            text: self.text.clone(),
            lang: self.lang.clone(),
            depth: self.depth,
            ordered: self.ordered,
            start: self.start.clone(),
            loose: self.loose,
            task: self.task,
            checked: self.checked,
            items: None,
            header: None,
            rows: None,
            align: self.align.clone(),
            tokens: None,
            href: self.href.clone(),
            title: self.title.clone(),
            tag: self.tag.clone(),
            pre: self.pre,
            block: self.block,
            pending: self.pending,
            escaped: self.escaped,
            in_link: self.in_link,
            in_raw_block: self.in_raw_block,
            code_block_style: self.code_block_style.clone(),
        }
    }
}

fn new_token_list_with(tokens: Vec<Token>) -> TokenList {
    Rc::new(RefCell::new(tokens))
}

pub fn tokens_to_json(tokens: &[Token]) -> Value {
    Value::Array(tokens.iter().map(Token::to_json).collect())
}

/// A link definition (`tokens.links[tag]` in marked). Not serialized — the
/// TS harness's `JSON.stringify` drops the `.links` property on the array.
#[derive(Debug, Clone)]
pub struct LinkDef {
    pub href: String,
    pub title: OptStr,
}

#[derive(Debug, Clone)]
pub struct LexerState {
    pub in_link: bool,
    pub in_raw_block: bool,
    pub link_emitted: bool,
    pub top: bool,
}

#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub src: String,
    pub tokens: TokenList,
}

pub struct Lexer {
    pub tokens: TokenList,
    pub links: HashMap<String, LinkDef>,
    pub state: LexerState,
    pub inline_queue: Vec<QueueEntry>,
    pub rules: &'static Rules,
}

impl Lexer {
    pub fn new() -> Lexer {
        Lexer {
            tokens: new_token_list(),
            links: HashMap::new(),
            state: LexerState {
                in_link: false,
                in_raw_block: false,
                link_emitted: false,
                top: true,
            },
            inline_queue: Vec::new(),
            rules: rules::rules(),
        }
    }

    /// `Lexer.lex(src)`: normalize line endings, run block tokenization, then
    /// drain the inline queue in push order.
    pub fn lex(&mut self, src: &str) -> TokenList {
        let src = src.replace("\r\n", "\n").replace('\r', "\n");
        let root = self.tokens.clone();
        self.block_tokens(&src, &root, false);
        let mut i = 0;
        while i < self.inline_queue.len() {
            let entry_src = self.inline_queue[i].src.clone();
            let entry_tokens = self.inline_queue[i].tokens.clone();
            self.inline_tokens(&entry_src, &entry_tokens);
            i += 1;
        }
        self.inline_queue.clear();
        root
    }

    /// `Lexer.inline(src)` (no target passed by callers): create the target
    /// array, queue it for the drain phase, and return the handle.
    pub fn inline(&mut self, src: &str) -> TokenList {
        let arr = new_token_list();
        self.inline_queue.push(QueueEntry {
            src: src.to_string(),
            tokens: arr.clone(),
        });
        arr
    }
}

/// pi-tui's `lexMarkdown` test seam (components/markdown.ts): tab
/// normalization to three spaces, marked lexing, then
/// `trimPartialClosingFences` on the (cloned) result.
pub fn lex_markdown(source: &str) -> Vec<Token> {
    let normalized = source.replace('\t', "   ");
    let mut lexer = Lexer::new();
    let root = lexer.lex(&normalized);
    let mut out: Vec<Token> = root.borrow().iter().map(Token::deep_clone).collect();
    trim_partial_closing_fences(&mut out);
    out
}

/// Port of `trimPartialClosingFences` (markdown.ts): recurse into the last
/// list item / blockquote, and trim streamed partial closing fences off the
/// final code token.
pub fn trim_partial_closing_fences(tokens: &mut [Token]) {
    let Some(token) = tokens.last_mut() else { return };
    match token.r#type.as_str() {
        "list" => {
            let mut recurse = false;
            if let Some(items) = token.items.as_mut() {
                if let Some(last_item) = items.last_mut() {
                    if let Some(tl) = last_item.tokens.as_mut() {
                        trim_partial_closing_fences(&mut tl.borrow_mut());
                        recurse = true;
                    }
                }
            }
            let _ = recurse;
        }
        "blockquote" => {
            if let Some(tl) = token.tokens.as_ref() {
                trim_partial_closing_fences(&mut tl.borrow_mut());
            }
        }
        "code" => {
            trim_partial_closing_fence_code(token);
        }
        _ => {}
    }
}

fn trim_partial_closing_fence_code(token: &mut Token) {
    // marker = /^(`{3,}|~{3,})/ on token.raw; lastLine = raw.split("\n").pop()
    static MARKER: std::sync::OnceLock<fancy_regex::Regex> = std::sync::OnceLock::new();
    let marker_re = MARKER.get_or_init(|| {
        fancy_regex::Regex::new("^(`{3,}|~{3,})").expect("fence marker regex")
    });
    let marker = marker_re
        .captures(&token.raw)
        .ok()
        .flatten()
        .and_then(|c| c.get(1).map(|g| g.as_str().to_string()));
    let last_line = token.raw.split('\n').next_back().map(|s| s.to_string());

    let (Some(marker), Some(last_line)) = (marker, last_line) else {
        return;
    };
    if last_line.is_empty() {
        return; // JS `!lastLine` early return
    }
    let marker_first = marker.chars().next().unwrap().to_string();
    if last_line.chars().count() >= marker.chars().count()
        || last_line != marker_first.repeat(last_line.chars().count())
    {
        return;
    }
    // token.text = text.slice(0, -lastLine.length).replace(/\n$/, "")
    if let Some(text) = token.text.as_mut() {
        let cut = text
            .char_indices()
            .nth(text.chars().count().saturating_sub(last_line.chars().count()))
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        let mut t = text[..cut].to_string();
        if t.ends_with('\n') {
            t.pop();
        }
        *text = t;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_str(t: &Token) -> String {
        serde_json::to_string(&t.to_json()).unwrap()
    }

    #[test]
    fn token_json_omits_none_fields() {
        let t = Token::new("hr", "---\n");
        assert_eq!(json_str(&t), r#"{"type":"hr","raw":"---\n"}"#);
    }

    #[test]
    fn token_json_null_title() {
        let mut t = Token::new("link", "[a](b)");
        t.href = Some("b".into());
        t.title = Some(None);
        t.text = Some("a".into());
        assert_eq!(
            json_str(&t),
            r#"{"type":"link","raw":"[a](b)","text":"a","href":"b","title":null}"#
        );
    }

    #[test]
    fn token_json_nested_tokens() {
        let mut t = Token::new("paragraph", "hi");
        t.text = Some("hi".into());
        let inner = new_token_list();
        inner.borrow_mut().push(Token::new("text", "hi"));
        t.tokens = Some(inner);
        assert_eq!(
            json_str(&t),
            r#"{"type":"paragraph","raw":"hi","text":"hi","tokens":[{"type":"text","raw":"hi"}]}"#
        );
    }

    #[test]
    fn trim_partial_fence_trims_streamed_marker() {
        // streamed fence: raw ends mid-marker, no trailing newline
        let mut t = Token::new("code", "```\ncode\n``");
        t.text = Some("code\n``".into());
        trim_partial_closing_fence_code(&mut t);
        assert_eq!(t.text.as_deref(), Some("code"));
    }

    #[test]
    fn trim_partial_fence_keeps_complete_marker() {
        let mut t = Token::new("code", "```\ncode\n```");
        t.text = Some("code\n".into());
        trim_partial_closing_fence_code(&mut t);
        assert_eq!(t.text.as_deref(), Some("code\n"));
    }

    #[test]
    fn lex_markdown_smoke() {
        // Full lexer port lands with the block/inline/lexer modules; this
        // only exercises the entry plumbing once those are implemented.
        let tokens = lex_markdown("# Hello\n\nWorld **bold**\n");
        assert!(!tokens.is_empty());
    }
}
