//! Port of `packages/tui/src/components/markdown.ts` — the Markdown
//! component (renderer). Ground truth:
//! /tmp/pi-repo/packages/tui/src/components/markdown.ts (1038 lines).
//!
//! Tokenization comes from [`crate::marked::lex_markdown`]; LaTeX rendering
//! from [`crate::latex::render_latex`]; image-line handling from
//! [`crate::terminal_image`].
//!
//! Conventions (see components/box_component.rs): TS closure fields become
//! `Option<Box<dyn Fn(..) -> ..>>`; the TS WeakRef token cache becomes a
//! strong cache (memory management only, no behavior change) — document any
//! further divergence in the module header.

use crate::component::Component;
use crate::latex::RenderLatexOptions;

/// TS `DefaultTextStyle` (markdown.ts:194-210) — all optional.
#[derive(Default)]
pub struct DefaultTextStyle {
    pub color: Option<Box<dyn Fn(&str) -> String>>,
    pub bg_color: Option<Box<dyn Fn(&str) -> String>>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub strikethrough: Option<bool>,
    pub underline: Option<bool>,
}

/// TS `MarkdownTheme` (markdown.ts:213-231).
pub struct MarkdownTheme {
    pub heading: Box<dyn Fn(&str) -> String>,
    pub link: Box<dyn Fn(&str) -> String>,
    pub link_url: Box<dyn Fn(&str) -> String>,
    pub code: Box<dyn Fn(&str) -> String>,
    pub code_block: Box<dyn Fn(&str) -> String>,
    pub code_block_border: Box<dyn Fn(&str) -> String>,
    pub quote: Box<dyn Fn(&str) -> String>,
    pub quote_border: Box<dyn Fn(&str) -> String>,
    pub hr: Box<dyn Fn(&str) -> String>,
    pub list_bullet: Box<dyn Fn(&str) -> String>,
    pub bold: Box<dyn Fn(&str) -> String>,
    pub italic: Box<dyn Fn(&str) -> String>,
    pub strikethrough: Box<dyn Fn(&str) -> String>,
    pub underline: Box<dyn Fn(&str) -> String>,
    pub highlight_code: Option<Box<dyn Fn(&str, Option<&str>) -> Vec<String>>>,
    pub code_block_indent: Option<String>,
}

/// TS `MarkdownOptions` (markdown.ts:233-247) — all optional.
#[derive(Default)]
pub struct MarkdownOptions {
    pub preserve_ordered_list_markers: Option<bool>,
    pub preserve_backslash_escapes: Option<bool>,
    pub transform: Option<Box<dyn Fn(&str, usize) -> String>>,
    pub render_latex: Option<bool>,
}

/// TS `class Markdown implements Component` (markdown.ts:249+).
pub struct Markdown {
    text: String,
    padding_x: usize,
    padding_y: usize,
    default_text_style: Option<DefaultTextStyle>,
    theme: MarkdownTheme,
    options: MarkdownOptions,
    default_style_prefix: Option<String>,
    cached_text: Option<String>,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<String>>,
}

impl Markdown {
    pub fn new(
        text: impl Into<String>,
        padding_x: usize,
        padding_y: usize,
        theme: MarkdownTheme,
        default_text_style: Option<DefaultTextStyle>,
        options: Option<MarkdownOptions>,
    ) -> Markdown {
        Markdown {
            text: text.into(),
            padding_x,
            padding_y,
            default_text_style,
            theme,
            // TS `options ? { ...options } : {}`.
            options: options.unwrap_or_default(),
            default_style_prefix: None,
            cached_text: None,
            cached_width: None,
            cached_lines: None,
        }
    }

    /// TS `setText(text)`.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.invalidate();
    }

    /// TS `invalidate()`.
    pub fn invalidate(&mut self) {
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }
}

impl Component for Markdown {
    fn render(&mut self, width: usize) -> Vec<String> {
        let _ = RenderLatexOptions::default(); // seam check (remove when wiring)
        let _ = width;
        todo!("markdown renderer port")
    }
}
