//! Port of `packages/tui/src/components/markdown.ts` — the Markdown
//! component (renderer). Ground truth:
//! /tmp/pi-repo/packages/tui/src/components/markdown.ts (1038 lines).
//!
//! Tokenization comes from [`crate::marked::lex_markdown`] (the TS
//! `markdownParser.lexer` + `trimPartialClosingFences` pipeline, already
//! ported); LaTeX rendering from [`crate::latex::render_latex`]; image-line
//! handling from [`crate::terminal_image`].
//!
//! ## Divergences from TS (all behavior-preserving)
//! - **WeakRef token cache → strong cache**: TS holds the parsed token tree
//!   in a `WeakRef` so long transcripts can reclaim it; Rust holds
//!   `cached_tokens: Option<(String, Vec<Token>)>` strongly. Memory
//!   management only — the cache key (`normalizedText`) and hit/miss
//!   behavior are identical.
//! - **`defaultStylePrefix` laziness**: TS computes the default style prefix
//!   on first inline render and caches it on the instance. The Rust render
//!   pipeline is `&self` (theme closures are shared), so the prefix is
//!   precomputed once at the top of `render()` when a default text style
//!   exists. The cached value is identical (a pure function of the
//!   immutable theme + default style).
//! - **`flattenLines(result)`** (markdown.ts:384) is a TS no-op
//!   (`for (const line of lines) Number(line)`) and is omitted.
//! - **List-marker regexes hand-coded**: `getOrderedListMarker` /
//!   `getUnorderedListMarker` (markdown.ts:766-774) use
//!   `^(?: {0,3})(\d{1,9}[.)])[ \t]+` and
//!   `^(?: {0,3})([-+*])(?:[ \t]+|(?=\r?\n|$))`; the port matches them with
//!   char scans (no regex dependency). The `(?: {0,3})` prefix is
//!   deterministic (backtracking cannot help: a space can never start a
//!   digit/bullet match), so the scans are equivalent.
//! - **Table growth arithmetic**: TS `Math.floor((weight / totalWeight) *
//!   remaining)` uses float64 division; the port uses exact integer
//!   arithmetic `(weight * remaining) / total_weight` (i64). Same floor
//!   result for all realistic magnitudes.
//! - **Malformed tables** (rows longer than the header) index
//!   `columnWidths[colIdx]` with `undefined` in TS (NaN arithmetic); the
//!   port treats the missing width as 0. Unreachable for well-formed
//!   markdown.
//! - Theme closures: `Box<dyn Fn(&str) -> String>` per the crate convention
//!   (see components/box_component.rs).

use crate::component::Component;
use crate::latex::{render_latex, RenderLatexOptions};
use crate::marked::{lex_markdown, Token, TokenList};
use crate::terminal_image::{get_capabilities, hyperlink, is_image_line};
use crate::utils::{
    apply_background_to_line, is_js_whitespace, js_trim, visible_width, wrap_text_with_ansi,
};

// =============================================================================
// Public option/style types (markdown.ts:194-247)
// =============================================================================

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

/// TS `InlineStyleContext` (markdown.ts:244-247): `{applyText, stylePrefix}`.
/// The three contexts the renderer builds (default, heading, blockquote) are
/// modeled as an enum; `applyText`/`stylePrefix` dispatch through
/// [`Markdown::context_apply_text`] / [`Markdown::context_style_prefix`].
#[derive(Clone)]
enum StyleContext {
    /// Default context (markdown.ts:470-475): `applyText = applyDefaultStyle`,
    /// `stylePrefix = getDefaultStylePrefix()`.
    Default,
    /// Heading context (markdown.ts:493-503): level 1 wraps
    /// `heading(bold(underline(t)))`, others `heading(bold(t))`.
    Heading { level: i64, prefix: String },
    /// Blockquote context (markdown.ts:596-599): `applyText` is the identity,
    /// `stylePrefix` is the quote style prefix.
    Quote { prefix: String },
}

// =============================================================================
// Markdown component (markdown.ts:249-1038)
// =============================================================================

/// TS `class Markdown implements Component` (markdown.ts:249+).
pub struct Markdown {
    text: String,
    padding_x: usize,
    padding_y: usize,
    default_text_style: Option<DefaultTextStyle>,
    theme: MarkdownTheme,
    options: MarkdownOptions,
    default_style_prefix: Option<String>,

    // Cache for rendered output (markdown.ts:258-261).
    cached_text: Option<String>,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<String>>,
    /// TS `cachedTokens?: WeakRef<{source, tokens}>` (markdown.ts:265) —
    /// strong cache in Rust (see module-header divergence note).
    cached_tokens: Option<(String, Vec<Token>)>,
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
            cached_tokens: None,
        }
    }

    /// TS `setText(text)` (markdown.ts:283-286).
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.invalidate();
    }

    /// TS `invalidate()` (markdown.ts:288-292). Note: like TS, this does NOT
    /// clear `defaultStylePrefix` or the token cache.
    pub fn invalidate(&mut self) {
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }

    // -------------------------------------------------------------------------
    // Default text style helpers (markdown.ts:394-475)
    // -------------------------------------------------------------------------

    /// TS `applyDefaultStyle` (markdown.ts:400-427). Background color is NOT
    /// applied here — it is applied at the padding stage.
    fn apply_default_style(&self, text: &str) -> String {
        let Some(style) = &self.default_text_style else {
            return text.to_string();
        };

        let mut styled = text.to_string();

        if let Some(color) = &style.color {
            styled = color(&styled);
        }
        if style.bold == Some(true) {
            styled = (self.theme.bold)(&styled);
        }
        if style.italic == Some(true) {
            styled = (self.theme.italic)(&styled);
        }
        if style.strikethrough == Some(true) {
            styled = (self.theme.strikethrough)(&styled);
        }
        if style.underline == Some(true) {
            styled = (self.theme.underline)(&styled);
        }

        styled
    }

    /// TS `getDefaultStylePrefix` body (markdown.ts:438-460), computed eagerly
    /// at render start (laziness divergence, see module header).
    fn compute_default_style_prefix(&self) -> String {
        let Some(style) = &self.default_text_style else {
            return String::new();
        };

        let sentinel = "\u{0}";
        let mut styled = sentinel.to_string();

        if let Some(color) = &style.color {
            styled = color(&styled);
        }
        if style.bold == Some(true) {
            styled = (self.theme.bold)(&styled);
        }
        if style.italic == Some(true) {
            styled = (self.theme.italic)(&styled);
        }
        if style.strikethrough == Some(true) {
            styled = (self.theme.strikethrough)(&styled);
        }
        if style.underline == Some(true) {
            styled = (self.theme.underline)(&styled);
        }

        match styled.find(sentinel) {
            Some(index) => styled[..index].to_string(),
            None => String::new(),
        }
    }

    /// TS `getStylePrefix(styleFn)` (markdown.ts:463-468).
    fn get_style_prefix<F: Fn(&str) -> String>(&self, style_fn: F) -> String {
        let sentinel = "\u{0}";
        let styled = style_fn(sentinel);
        match styled.find(sentinel) {
            Some(index) => styled[..index].to_string(),
            None => String::new(),
        }
    }

    /// TS `headingStyleFn` (markdown.ts:493-498).
    fn heading_style(&self, level: i64, text: &str) -> String {
        if level == 1 {
            (self.theme.heading)(&(self.theme.bold)(&(self.theme.underline)(text)))
        } else {
            (self.theme.heading)(&(self.theme.bold)(text))
        }
    }

    /// `InlineStyleContext.applyText` dispatch.
    fn context_apply_text(&self, context: &StyleContext, text: &str) -> String {
        match context {
            StyleContext::Default => self.apply_default_style(text),
            StyleContext::Heading { level, .. } => self.heading_style(*level, text),
            // Blockquote children render with the default message style
            // disabled (markdown.ts:597).
            StyleContext::Quote { .. } => text.to_string(),
        }
    }

    /// `InlineStyleContext.stylePrefix` dispatch.
    fn context_style_prefix(&self, context: &StyleContext) -> String {
        match context {
            StyleContext::Default => {
                self.default_style_prefix.clone().unwrap_or_default()
            }
            StyleContext::Heading { prefix, .. } | StyleContext::Quote { prefix } => {
                prefix.clone()
            }
        }
    }

    // -------------------------------------------------------------------------
    // renderToken (markdown.ts:477-655)
    // -------------------------------------------------------------------------

    /// TS `renderToken` (markdown.ts:477-655).
    fn render_token(
        &self,
        token: &Token,
        width: usize,
        next_token_type: Option<&str>,
        style_context: Option<&StyleContext>,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();

        match token.r#type.as_str() {
            // markdown.ts:486-511
            "heading" => {
                let heading_level = token.depth.unwrap_or(0);
                let heading_prefix = format!("{} ", "#".repeat(heading_level.max(0) as usize));

                // Heading-specific style context so inline tokens restore
                // heading styling after their own ANSI resets.
                let heading_style_context = StyleContext::Heading {
                    level: heading_level,
                    prefix: self.get_style_prefix(|t| self.heading_style(heading_level, t)),
                };

                let heading_text =
                    self.render_inline_tokens_opt(token.tokens.as_ref(), Some(&heading_style_context));
                let styled_heading = if heading_level >= 3 {
                    format!(
                        "{}{}",
                        self.heading_style(heading_level, &heading_prefix),
                        heading_text
                    )
                } else {
                    heading_text
                };
                lines.push(styled_heading);
                if next_token_type.is_some() && next_token_type != Some("space") {
                    lines.push(String::new()); // spacing after headings
                }
            }

            // markdown.ts:514-522
            "paragraph" => {
                let paragraph_text =
                    self.render_inline_tokens_opt(token.tokens.as_ref(), style_context);
                lines.push(paragraph_text);
                // No spacing if next token is space or list.
                if next_token_type.is_some()
                    && next_token_type != Some("list")
                    && next_token_type != Some("space")
                {
                    lines.push(String::new());
                }
            }

            // markdown.ts:524-526
            "text" => {
                let single = vec![token.clone()];
                lines.push(self.render_inline_tokens(&single, style_context));
            }

            // markdown.ts:528-541
            "latexBlock" => {
                let rendered = if token.pending != Some(true)
                    && self.options.render_latex != Some(false)
                {
                    render_latex(
                        token.text.as_deref().unwrap_or(""),
                        &RenderLatexOptions {
                            display: Some(true),
                        },
                    )
                    .unwrap_or_else(|| js_trim(&token.raw).to_string())
                } else {
                    js_trim(&token.raw).to_string()
                };
                for line in rendered.split('\n') {
                    lines.push(self.apply_default_style(line));
                }
                if next_token_type.is_some() && next_token_type != Some("space") {
                    lines.push(String::new());
                }
            }

            // markdown.ts:543-563
            "code" => {
                let indent = self
                    .theme
                    .code_block_indent
                    .clone()
                    .unwrap_or_else(|| "  ".to_string());
                lines.push((self.theme.code_block_border)(&format!(
                    "```{}",
                    token.lang.as_deref().unwrap_or("")
                )));
                if let Some(highlight_code) = &self.theme.highlight_code {
                    let highlighted_lines =
                        highlight_code(token.text.as_deref().unwrap_or(""), token.lang.as_deref());
                    for hl_line in highlighted_lines {
                        lines.push(format!("{indent}{hl_line}"));
                    }
                } else {
                    // Split code by newlines and style each line.
                    for code_line in token.text.as_deref().unwrap_or("").split('\n') {
                        lines.push(format!("{indent}{}", (self.theme.code_block)(code_line)));
                    }
                }
                lines.push((self.theme.code_block_border)("```"));
                if next_token_type.is_some() && next_token_type != Some("space") {
                    lines.push(String::new()); // spacing after code blocks
                }
            }

            // markdown.ts:565-571
            "list" => {
                let list_lines = self.render_list(token, 0, width, style_context);
                lines.extend(list_lines);
                // Spacing after lists is handled by the space token.
            }

            // markdown.ts:573-577
            "table" => {
                let table_lines =
                    self.render_table(token, width, next_token_type, style_context);
                lines.extend(table_lines);
            }

            // markdown.ts:579-626
            "blockquote" => {
                let quote_style_prefix =
                    self.get_style_prefix(|t| (self.theme.quote)(&(self.theme.italic)(t)));
                let apply_quote_style = |line: &str| -> String {
                    if quote_style_prefix.is_empty() {
                        (self.theme.quote)(&(self.theme.italic)(line))
                    } else {
                        let line_with_reapplied_style = line.replace(
                            "\x1b[0m",
                            &format!("\x1b[0m{quote_style_prefix}"),
                        );
                        (self.theme.quote)(&(self.theme.italic)(&line_with_reapplied_style))
                    }
                };

                // Available width for quote content (subtract "│ " = 2 chars).
                let quote_content_width = ((width as i64) - 2).max(1) as usize;

                // Blockquotes contain block-level tokens, so render children
                // with renderToken(); default message style does not apply.
                let quote_inline_style_context = StyleContext::Quote {
                    prefix: quote_style_prefix.clone(),
                };
                let empty_quote_tokens = crate::marked::new_token_list();
                let quote_tokens: &TokenList = token.tokens.as_ref().unwrap_or(&empty_quote_tokens);
                let borrowed_quote_tokens = quote_tokens.borrow();
                let mut rendered_quote_lines: Vec<String> = Vec::new();
                for i in 0..borrowed_quote_tokens.len() {
                    let next_quote_token_type =
                        borrowed_quote_tokens.get(i + 1).map(|t| t.r#type.as_str());
                    rendered_quote_lines.extend(self.render_token(
                        &borrowed_quote_tokens[i],
                        quote_content_width,
                        next_quote_token_type,
                        Some(&quote_inline_style_context),
                    ));
                }
                drop(borrowed_quote_tokens);

                // Avoid an extra empty quote line before outer spacing.
                while rendered_quote_lines.last().is_some_and(|l| l.is_empty()) {
                    rendered_quote_lines.pop();
                }

                for quote_line in rendered_quote_lines {
                    let styled_line = apply_quote_style(&quote_line);
                    let wrapped_lines = wrap_text_with_ansi(&styled_line, quote_content_width);
                    for wrapped_line in wrapped_lines {
                        lines.push(format!("{}{}", (self.theme.quote_border)("│ "), wrapped_line));
                    }
                }
                if next_token_type.is_some() && next_token_type != Some("space") {
                    lines.push(String::new()); // spacing after blockquotes
                }
            }

            // markdown.ts:628-633
            "hr" => {
                lines.push((self.theme.hr)(&"─".repeat(width.min(80))));
                if next_token_type.is_some() && next_token_type != Some("space") {
                    lines.push(String::new()); // spacing after horizontal rules
                }
            }

            // markdown.ts:635-640
            "html" => {
                // Render HTML as plain text (escaped for terminal).
                lines.push(self.apply_default_style(js_trim(&token.raw)));
            }

            // markdown.ts:642-645
            "space" => {
                // Space tokens represent blank lines in markdown.
                lines.push(String::new());
            }

            // markdown.ts:647-651
            _ => {
                if let Some(text) = &token.text {
                    lines.push(text.clone());
                }
            }
        }

        lines
    }

    // -------------------------------------------------------------------------
    // renderInlineTokens (markdown.ts:657-764)
    // -------------------------------------------------------------------------

    /// TS `renderInlineTokens(tokens, styleContext?)` (markdown.ts:657-764)
    /// for `Option<&TokenList>` token lists (`token.tokens || []`).
    fn render_inline_tokens_opt(
        &self,
        tokens: Option<&TokenList>,
        style_context: Option<&StyleContext>,
    ) -> String {
        match tokens {
            Some(list) => {
                let borrowed = list.borrow();
                self.render_inline_tokens(&borrowed, style_context)
            }
            None => self.render_inline_tokens(&[], style_context),
        }
    }

    /// TS `renderInlineTokens` (markdown.ts:657-764).
    fn render_inline_tokens(
        &self,
        tokens: &[Token],
        style_context: Option<&StyleContext>,
    ) -> String {
        let default_context;
        let resolved = match style_context {
            Some(context) => context,
            None => {
                default_context = StyleContext::Default;
                &default_context
            }
        };

        let style_prefix = self.context_style_prefix(resolved);
        let apply_text_with_newlines = |text: &str| -> String {
            text.split('\n')
                .map(|segment| self.context_apply_text(resolved, segment))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let mut result = String::new();

        for token in tokens {
            match token.r#type.as_str() {
                // markdown.ts:668-676
                "latex" => {
                    let rendered = if token.pending != Some(true)
                        && self.options.render_latex != Some(false)
                    {
                        render_latex(token.text.as_deref().unwrap_or(""), &RenderLatexOptions::default())
                            .unwrap_or_else(|| token.raw.clone())
                    } else {
                        token.raw.clone()
                    };
                    result.push_str(&apply_text_with_newlines(&rendered));
                }

                // markdown.ts:678-680
                "escape" => {
                    let text = if self.options.preserve_backslash_escapes == Some(true) {
                        token.raw.clone()
                    } else {
                        token.text.clone().unwrap_or_default()
                    };
                    result.push_str(&apply_text_with_newlines(&text));
                }

                // markdown.ts:682-689
                "text" => {
                    // Text tokens in list items can have nested tokens.
                    let has_nested = token
                        .tokens
                        .as_ref()
                        .is_some_and(|tl| !tl.borrow().is_empty());
                    if has_nested {
                        result.push_str(&self.render_inline_tokens_opt(
                            token.tokens.as_ref(),
                            Some(resolved),
                        ));
                    } else {
                        result.push_str(&apply_text_with_newlines(
                            token.text.as_deref().unwrap_or(""),
                        ));
                    }
                }

                // markdown.ts:691-694
                "paragraph" => {
                    result.push_str(&self.render_inline_tokens_opt(
                        token.tokens.as_ref(),
                        Some(resolved),
                    ));
                }

                // markdown.ts:696-700
                "strong" => {
                    let bold_content =
                        self.render_inline_tokens_opt(token.tokens.as_ref(), Some(resolved));
                    result.push_str(&format!(
                        "{}{}",
                        (self.theme.bold)(&bold_content),
                        style_prefix
                    ));
                }

                // markdown.ts:702-706
                "em" => {
                    let italic_content =
                        self.render_inline_tokens_opt(token.tokens.as_ref(), Some(resolved));
                    result.push_str(&format!(
                        "{}{}",
                        (self.theme.italic)(&italic_content),
                        style_prefix
                    ));
                }

                // markdown.ts:708-710
                "codespan" => {
                    result.push_str(&format!(
                        "{}{}",
                        (self.theme.code)(token.text.as_deref().unwrap_or("")),
                        style_prefix
                    ));
                }

                // markdown.ts:712-732
                "link" => {
                    let link_text =
                        self.render_inline_tokens_opt(token.tokens.as_ref(), Some(resolved));
                    let styled_link = (self.theme.link)(&(self.theme.underline)(&link_text));
                    if get_capabilities().hyperlinks {
                        // OSC 8: render as a clickable hyperlink.
                        result.push_str(&format!(
                            "{}{}",
                            hyperlink(&styled_link, token.href.as_deref().unwrap_or("")),
                            style_prefix
                        ));
                    } else {
                        // Fallback: print URL in parentheses when the text
                        // differs from href (mailto: prefix stripped for the
                        // comparison).
                        let href = token.href.as_deref().unwrap_or("");
                        let href_for_comparison = href.strip_prefix("mailto:").unwrap_or(href);
                        if token.text.as_deref() == Some(href)
                            || token.text.as_deref() == Some(href_for_comparison)
                        {
                            result.push_str(&format!("{styled_link}{style_prefix}"));
                        } else {
                            result.push_str(&format!(
                                "{}{}{}",
                                styled_link,
                                (self.theme.link_url)(&format!(" ({href})")),
                                style_prefix
                            ));
                        }
                    }
                }

                // markdown.ts:734-736
                "br" => {
                    result.push('\n');
                }

                // markdown.ts:738-742
                "del" => {
                    let del_content =
                        self.render_inline_tokens_opt(token.tokens.as_ref(), Some(resolved));
                    result.push_str(&format!(
                        "{}{}",
                        (self.theme.strikethrough)(&del_content),
                        style_prefix
                    ));
                }

                // markdown.ts:744-749
                "html" => {
                    // Render inline HTML as plain text.
                    result.push_str(&apply_text_with_newlines(&token.raw));
                }

                // markdown.ts:751-755
                _ => {
                    if let Some(text) = token.text.as_deref() {
                        result.push_str(&apply_text_with_newlines(text));
                    }
                }
            }
        }

        // markdown.ts:759-761
        while !style_prefix.is_empty() && result.ends_with(&style_prefix) {
            result.truncate(result.len() - style_prefix.len());
        }

        result
    }

    // -------------------------------------------------------------------------
    // Lists (markdown.ts:766-829)
    // -------------------------------------------------------------------------

    /// TS `getOrderedListMarker` (markdown.ts:766-769):
    /// `/^(?: {0,3})(\d{1,9}[.)])[ \t]+/` on `item.raw`.
    fn get_ordered_list_marker(&self, item: &Token) -> Option<String> {
        let chars: Vec<char> = item.raw.chars().collect();
        let mut pos = 0;
        // `(?: {0,3})` — up to three leading spaces.
        while pos < chars.len() && chars[pos] == ' ' && pos < 3 {
            pos += 1;
        }
        // `\d{1,9}[.)]`
        let digit_start = pos;
        while pos < chars.len() && chars[pos].is_ascii_digit() && pos - digit_start < 9 {
            pos += 1;
        }
        if pos == digit_start {
            return None;
        }
        if pos >= chars.len() || (chars[pos] != '.' && chars[pos] != ')') {
            return None;
        }
        let marker: String = chars[digit_start..=pos].iter().collect();
        pos += 1;
        // `[ \t]+`
        let sep_start = pos;
        while pos < chars.len() && (chars[pos] == ' ' || chars[pos] == '\t') {
            pos += 1;
        }
        if pos == sep_start {
            return None;
        }
        Some(format!("{marker} "))
    }

    /// TS `getUnorderedListMarker` (markdown.ts:771-774):
    /// `/^(?: {0,3})([-+*])(?:[ \t]+|(?=\r?\n|$))/` on `item.raw`.
    fn get_unordered_list_marker(&self, item: &Token) -> Option<String> {
        let chars: Vec<char> = item.raw.chars().collect();
        let mut pos = 0;
        // `(?: {0,3})`
        while pos < chars.len() && chars[pos] == ' ' && pos < 3 {
            pos += 1;
        }
        // `([-+*])`
        if pos >= chars.len() {
            return None;
        }
        let bullet = chars[pos];
        if bullet != '-' && bullet != '+' && bullet != '*' {
            return None;
        }
        pos += 1;
        // `(?:[ \t]+|(?=\r?\n|$))`
        if pos < chars.len() && (chars[pos] == ' ' || chars[pos] == '\t') {
            return Some(format!("{bullet} "));
        }
        if pos >= chars.len() {
            return Some(format!("{bullet} "));
        }
        if chars[pos] == '\n' {
            return Some(format!("{bullet} "));
        }
        if chars[pos] == '\r' && chars.get(pos + 1) == Some(&'\n') {
            return Some(format!("{bullet} "));
        }
        None
    }

    /// TS `renderList` (markdown.ts:779-829) — nested list rendering.
    fn render_list(
        &self,
        token: &Token,
        depth: usize,
        width: usize,
        style_context: Option<&StyleContext>,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let indent = "    ".repeat(depth);
        // List `start` (defaults to 1 for ordered lists).
        let start_number = match &token.start {
            Some(crate::marked::NumOrStr::Num(n)) => *n,
            _ => 1,
        };

        let items: &[Token] = token.items.as_deref().unwrap_or(&[]);
        for (i, item) in items.iter().enumerate() {
            let is_last_item = i == items.len() - 1;
            let bullet = if token.ordered == Some(true) {
                if self.options.preserve_ordered_list_markers == Some(true) {
                    self.get_ordered_list_marker(item)
                        .unwrap_or_else(|| format!("{}. ", start_number + i as i64))
                } else {
                    format!("{}. ", start_number + i as i64)
                }
            } else if self.options.preserve_ordered_list_markers == Some(true) {
                self.get_unordered_list_marker(item).unwrap_or_else(|| "- ".to_string())
            } else {
                "- ".to_string()
            };
            let task_marker = if item.task == Some(true) {
                format!("[{}] ", if item.checked == Some(true) { "x" } else { " " })
            } else {
                String::new()
            };
            let marker = format!("{bullet}{task_marker}");
            let first_prefix = format!("{indent}{}", (self.theme.list_bullet)(&marker));
            let continuation_prefix = format!("{indent}{}", " ".repeat(visible_width(&marker)));
            let item_width =
                ((width as i64) - (visible_width(&first_prefix) as i64)).max(1) as usize;
            let mut rendered_any_line = false;

            let empty_item_tokens = crate::marked::new_token_list();
            let item_tokens: &TokenList = item.tokens.as_ref().unwrap_or(&empty_item_tokens);
            let borrowed = item_tokens.borrow();
            for item_token in borrowed.iter() {
                if item_token.r#type == "list" {
                    lines.extend(self.render_list(item_token, depth + 1, width, style_context));
                    rendered_any_line = true;
                    continue;
                }

                let item_lines = self.render_token(item_token, item_width, None, style_context);
                for line in item_lines {
                    for wrapped_line in wrap_text_with_ansi(&line, item_width) {
                        let line_prefix = if rendered_any_line {
                            &continuation_prefix
                        } else {
                            &first_prefix
                        };
                        lines.push(format!("{line_prefix}{wrapped_line}"));
                        rendered_any_line = true;
                    }
                }
            }
            drop(borrowed);

            if !rendered_any_line {
                lines.push(first_prefix);
            }

            if token.loose == Some(true) && !is_last_item {
                lines.push(String::new());
            }
        }

        lines
    }

    // -------------------------------------------------------------------------
    // Tables (markdown.ts:831-1037)
    // -------------------------------------------------------------------------

    /// TS `getLongestWordWidth` (markdown.ts:834-844).
    fn get_longest_word_width(&self, text: &str, max_width: Option<usize>) -> usize {
        let mut longest = 0;
        for word in text.split(is_js_whitespace).filter(|word| !word.is_empty()) {
            longest = longest.max(visible_width(word));
        }
        match max_width {
            None => longest,
            Some(max_width) => longest.min(max_width),
        }
    }

    /// TS `wrapCellText` (markdown.ts:852-859).
    fn wrap_cell_text(&self, text: &str, max_width: usize, style_prefix: &str) -> Vec<String> {
        let lines = wrap_text_with_ansi(text, max_width.max(1));
        let count = lines.len();
        lines
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                // Reset text styles after each non-final fragment, then
                // restore the surrounding style.
                let style_reset = if count > 0 && index < count - 1 {
                    "\x1b[22;23;24;25;27;28;29;39m"
                } else {
                    ""
                };
                format!("{line}{style_reset}{style_prefix}")
            })
            .collect()
    }

    /// TS `renderTable` (markdown.ts:865-1037) — width-aware cell wrapping.
    fn render_table(
        &self,
        token: &Token,
        available_width: usize,
        next_token_type: Option<&str>,
        style_context: Option<&StyleContext>,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let header: &[crate::marked::TableCell] = token.header.as_deref().unwrap_or(&[]);
        let num_cols = header.len();

        if num_cols == 0 {
            return lines;
        }

        // Border overhead: "│ " + (n-1) * " │ " + " │" = 3n + 1.
        let border_overhead: i64 = (3 * num_cols + 1) as i64;
        let available_for_cells: i64 = available_width as i64 - border_overhead;
        if available_for_cells < num_cols as i64 {
            // Too narrow — fall back to raw markdown.
            let mut fallback_lines = if token.raw.is_empty() {
                Vec::new()
            } else {
                wrap_text_with_ansi(&token.raw, available_width)
            };
            if next_token_type.is_some() && next_token_type != Some("space") {
                fallback_lines.push(String::new());
            }
            return fallback_lines;
        }

        let max_unbroken_word_width = 30usize;

        // Natural column widths + minimum (longest-word) widths.
        let mut natural_widths: Vec<i64> = vec![0; num_cols];
        let mut min_word_widths: Vec<i64> = vec![0; num_cols];
        for i in 0..num_cols {
            let header_text =
                self.render_inline_tokens_list(&header[i].tokens, style_context);
            natural_widths[i] = visible_width(&header_text) as i64;
            min_word_widths[i] = 1.max(self.get_longest_word_width(
                &header_text,
                Some(max_unbroken_word_width),
            ) as i64);
        }
        let rows: &[Vec<crate::marked::TableCell>] = token.rows.as_deref().unwrap_or(&[]);
        for row in rows {
            for i in 0..row.len() {
                let cell_text = self.render_inline_tokens_list(&row[i].tokens, style_context);
                if i < num_cols {
                    natural_widths[i] = natural_widths[i].max(visible_width(&cell_text) as i64);
                    min_word_widths[i] = min_word_widths[i]
                        .max(1)
                        .max(self.get_longest_word_width(&cell_text, Some(max_unbroken_word_width)) as i64);
                }
            }
        }

        let mut min_column_widths = min_word_widths.clone();
        let mut min_cells_width: i64 = min_column_widths.iter().sum();

        if min_cells_width > available_for_cells {
            min_column_widths = vec![1; num_cols];
            let remaining = available_for_cells - num_cols as i64;

            if remaining > 0 {
                let total_weight: i64 = min_word_widths
                    .iter()
                    .map(|width| (*width - 1).max(0))
                    .sum();
                let growth: Vec<i64> = min_word_widths
                    .iter()
                    .map(|width| {
                        let weight = (*width - 1).max(0);
                        if total_weight > 0 {
                            (weight * remaining) / total_weight
                        } else {
                            0
                        }
                    })
                    .collect();

                for i in 0..num_cols {
                    min_column_widths[i] += growth[i];
                }

                let allocated: i64 = growth.iter().sum();
                let mut leftover = remaining - allocated;
                let mut i = 0;
                while leftover > 0 && i < num_cols {
                    min_column_widths[i] += 1;
                    leftover -= 1;
                    i += 1;
                }
            }

            min_cells_width = min_column_widths.iter().sum();
        }

        // Column widths that fit within the available width.
        let total_natural_width: i64 = natural_widths.iter().sum::<i64>() + border_overhead;
        let column_widths: Vec<i64>;

        if total_natural_width <= available_width as i64 {
            // Everything fits naturally.
            column_widths = (0..num_cols)
                .map(|i| natural_widths[i].max(min_column_widths[i]))
                .collect();
        } else {
            // Shrink columns to fit.
            let total_grow_potential: i64 = (0..num_cols)
                .map(|i| (natural_widths[i] - min_column_widths[i]).max(0))
                .sum();
            let extra_width = (available_for_cells - min_cells_width).max(0);
            let mut widths: Vec<i64> = (0..num_cols)
                .map(|i| {
                    let natural_width = natural_widths[i];
                    let min_width = min_column_widths[i];
                    let min_width_delta = (natural_width - min_width).max(0);
                    let grow = if total_grow_potential > 0 {
                        (min_width_delta * extra_width) / total_grow_potential
                    } else {
                        0
                    };
                    min_width + grow
                })
                .collect();

            // Adjust for rounding errors — distribute remaining space.
            let allocated: i64 = widths.iter().sum();
            let mut remaining = available_for_cells - allocated;
            while remaining > 0 {
                let mut grew = false;
                let mut i = 0;
                while i < num_cols && remaining > 0 {
                    if widths[i] < natural_widths[i] {
                        widths[i] += 1;
                        remaining -= 1;
                        grew = true;
                    }
                    i += 1;
                }
                if !grew {
                    break;
                }
            }
            column_widths = widths;
        }

        // Top border.
        let top_border_cells: Vec<String> =
            column_widths.iter().map(|w| "─".repeat(*w as usize)).collect();
        lines.push(format!("┌─{}─┐", top_border_cells.join("─┬─")));

        // Header with wrapping.
        let table_style_prefix: String = match style_context {
            Some(context) => self.context_style_prefix(context),
            None => String::new(),
        };
        let header_cell_lines: Vec<Vec<String>> = (0..num_cols)
            .map(|i| {
                let text = self.render_inline_tokens_list(&header[i].tokens, style_context);
                self.wrap_cell_text(&text, column_widths[i] as usize, &table_style_prefix)
            })
            .collect();
        let header_line_count = header_cell_lines.iter().map(|c| c.len()).max().unwrap_or(0);

        for line_idx in 0..header_line_count {
            let row_parts: Vec<String> = (0..num_cols)
                .map(|col_idx| {
                    let text = header_cell_lines[col_idx]
                        .get(line_idx)
                        .cloned()
                        .unwrap_or_default();
                    let padded = format!(
                        "{text}{}",
                        " ".repeat(
                            (column_widths[col_idx] - visible_width(&text) as i64).max(0) as usize
                        )
                    );
                    (self.theme.bold)(&padded)
                })
                .collect();
            lines.push(format!("│ {} │", row_parts.join(" │ ")));
        }

        // Separator.
        let separator_cells: Vec<String> =
            column_widths.iter().map(|w| "─".repeat(*w as usize)).collect();
        let separator_line = format!("├─{}─┤", separator_cells.join("─┼─"));
        lines.push(separator_line.clone());

        // Rows with wrapping.
        for (row_index, row) in rows.iter().enumerate() {
            let row_cell_lines: Vec<Vec<String>> = (0..row.len())
                .map(|i| {
                    let text = self.render_inline_tokens_list(&row[i].tokens, style_context);
                    let col_width = column_widths.get(i).copied().unwrap_or(0);
                    self.wrap_cell_text(&text, col_width as usize, &table_style_prefix)
                })
                .collect();
            let row_line_count = row_cell_lines.iter().map(|c| c.len()).max().unwrap_or(0);

            for line_idx in 0..row_line_count {
                let row_parts: Vec<String> = (0..row.len())
                    .map(|col_idx| {
                        let text = row_cell_lines[col_idx]
                            .get(line_idx)
                            .cloned()
                            .unwrap_or_default();
                        let col_width = column_widths.get(col_idx).copied().unwrap_or(0);
                        format!(
                            "{text}{}",
                            " ".repeat(
                                (col_width - visible_width(&text) as i64).max(0) as usize
                            )
                        )
                    })
                    .collect();
                lines.push(format!("│ {} │", row_parts.join(" │ ")));
            }

            if row_index < rows.len() - 1 {
                lines.push(separator_line.clone());
            }
        }

        // Bottom border.
        let bottom_border_cells: Vec<String> =
            column_widths.iter().map(|w| "─".repeat(*w as usize)).collect();
        lines.push(format!("└─{}─┘", bottom_border_cells.join("─┴─")));

        if next_token_type.is_some() && next_token_type != Some("space") {
            lines.push(String::new()); // spacing after table
        }
        lines
    }

    /// `renderInlineTokens(cell.tokens || [], styleContext)` for table cells
    /// (their `tokens` is always present).
    fn render_inline_tokens_list(
        &self,
        tokens: &TokenList,
        style_context: Option<&StyleContext>,
    ) -> String {
        let borrowed = tokens.borrow();
        self.render_inline_tokens(&borrowed, style_context)
    }
}

// =============================================================================
// render (markdown.ts:294-392)
// =============================================================================

impl Component for Markdown {
    fn render(&mut self, width: usize) -> Vec<String> {
        // Check cache (markdown.ts:296-298).
        if let (Some(lines), Some(cached_text), Some(cached_width)) =
            (&self.cached_lines, &self.cached_text, self.cached_width)
        {
            if *cached_text == self.text && cached_width == width {
                return lines.clone();
            }
        }

        // Available width for content (subtract horizontal padding).
        let content_width = ((width as i64) - 2 * (self.padding_x as i64)).max(1) as usize;
        let text = match &self.options.transform {
            Some(transform) => transform(&self.text, content_width),
            None => self.text.clone(),
        };

        // Don't render anything if there's no actual text (markdown.ts:304-312).
        if js_trim(&text).is_empty() {
            let result: Vec<String> = Vec::new();
            self.cached_text = Some(self.text.clone());
            self.cached_width = Some(width);
            self.cached_lines = Some(result);
            return Vec::new();
        }

        // Replace tabs with 3 spaces (markdown.ts:315).
        let normalized_text = text.replace('\t', "   ");

        // Precompute the default style prefix (TS computes it lazily inside
        // inline rendering; see module-header divergence note).
        if self.default_text_style.is_some() && self.default_style_prefix.is_none() {
            self.default_style_prefix = Some(self.compute_default_style_prefix());
        }

        // Parse markdown to tokens (markdown.ts:318-324) — the token cache is
        // keyed on the normalized source and survives theme/width changes.
        let tokens = match self.cached_tokens.take() {
            Some((source, tokens)) if source == normalized_text => tokens,
            _ => lex_markdown(&normalized_text),
        };

        // Convert tokens to styled terminal output (markdown.ts:329-336).
        let mut rendered_lines: Vec<String> = Vec::new();
        for i in 0..tokens.len() {
            let next_token_type = tokens.get(i + 1).map(|t| t.r#type.as_str());
            let token_lines = self.render_token(&tokens[i], content_width, next_token_type, None);
            rendered_lines.extend(token_lines);
        }

        // Wrap lines (NO padding, NO background yet) (markdown.ts:339-348).
        let mut wrapped_lines: Vec<String> = Vec::new();
        for line in rendered_lines {
            if is_image_line(&line) {
                wrapped_lines.push(line);
            } else {
                wrapped_lines.extend(wrap_text_with_ansi(&line, content_width));
            }
        }

        // Add margins and background to each wrapped line (markdown.ts:351-372).
        let left_margin = " ".repeat(self.padding_x);
        let right_margin = " ".repeat(self.padding_x);
        let bg_fn = self
            .default_text_style
            .as_ref()
            .and_then(|style| style.bg_color.as_ref());
        let mut content_lines: Vec<String> = Vec::new();

        for line in wrapped_lines {
            if is_image_line(&line) {
                content_lines.push(line);
                continue;
            }

            let line_with_margins = format!("{left_margin}{line}{right_margin}");

            match bg_fn {
                Some(bg) => {
                    content_lines.push(apply_background_to_line(&line_with_margins, width, |t| {
                        bg(t)
                    }));
                }
                None => {
                    // No background — just pad to width.
                    let visible_len = visible_width(&line_with_margins);
                    let padding_needed = width.saturating_sub(visible_len);
                    content_lines.push(format!("{line_with_margins}{}", " ".repeat(padding_needed)));
                }
            }
        }

        // Add top/bottom padding (empty lines) (markdown.ts:374-380).
        let empty_line = " ".repeat(width);
        let mut empty_lines: Vec<String> = Vec::new();
        for _ in 0..self.padding_y {
            let line = match bg_fn {
                Some(bg) => apply_background_to_line(&empty_line, width, |t| bg(t)),
                None => empty_line.clone(),
            };
            empty_lines.push(line);
        }

        // Combine top padding, content, and bottom padding (markdown.ts:382-384).
        // (`flattenLines(result)` is a TS no-op — omitted.)
        let mut result = empty_lines.clone();
        result.extend(content_lines);
        result.extend(empty_lines);

        // Update cache (markdown.ts:386-389).
        self.cached_text = Some(self.text.clone());
        self.cached_width = Some(width);
        self.cached_tokens = Some((normalized_text, tokens));
        self.cached_lines = Some(result.clone());

        // markdown.ts:391
        if result.is_empty() {
            vec![String::new()]
        } else {
            result
        }
    }
}

// =============================================================================
// Tests — expectations captured from the TS implementation
// (/tmp/pi-repo/packages/tui/src/components/markdown.ts) via a probe harness
// with the marker theme below (see task notes). `set_test_capabilities`
// pins the thread-local capability cache (libtest reuses threads).
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_image::{set_capabilities, TerminalCapabilities};

    /// Probe theme (mirrors /tmp/probe-md.mjs).
    fn probe_theme() -> MarkdownTheme {
        MarkdownTheme {
            heading: Box::new(|t| format!("#<{t}>")),
            link: Box::new(|t| format!("L<{t}>")),
            link_url: Box::new(|t| format!("U<{t}>")),
            code: Box::new(|t| format!("C<{t}>")),
            code_block: Box::new(|t| format!("CB<{t}>")),
            code_block_border: Box::new(|t| format!("B<{t}>")),
            quote: Box::new(|t| format!("Q<{t}>")),
            quote_border: Box::new(|_| "|".to_string()),
            hr: Box::new(|t| format!("-{t}-")),
            list_bullet: Box::new(|t| format!("*{t}")),
            bold: Box::new(|t| format!("**{t}**")),
            italic: Box::new(|t| format!("_{t}_")),
            strikethrough: Box::new(|t| format!("~{t}~")),
            underline: Box::new(|t| format!("_<{t}>_")),
            highlight_code: None,
            code_block_indent: None,
        }
    }

    fn set_test_capabilities(hyperlinks: bool) {
        set_capabilities(TerminalCapabilities {
            images: None,
            true_color: false,
            hyperlinks,
        });
    }

    fn render_case(text: &str, padding_x: usize, padding_y: usize, width: usize) -> Vec<String> {
        set_test_capabilities(false);
        let mut component = Markdown::new(text, padding_x, padding_y, probe_theme(), None, None);
        component.render(width)
    }

    fn render_case_opts(text: &str, width: usize, options: MarkdownOptions) -> Vec<String> {
        set_test_capabilities(false);
        let mut component = Markdown::new(text, 0, 0, probe_theme(), None, Some(options));
        component.render(width)
    }

    fn render_case_styled(text: &str, width: usize) -> Vec<String> {
        set_test_capabilities(false);
        let mut component = Markdown::new(
            text,
            0,
            0,
            probe_theme(),
            Some(DefaultTextStyle {
                color: Some(Box::new(|t| format!("C<{t}>"))),
                bold: Some(true),
                ..Default::default()
            }),
            None,
        );
        component.render(width)
    }

    fn render_case_hl(text: &str, width: usize) -> Vec<String> {
        set_test_capabilities(false);
        let theme = MarkdownTheme {
            highlight_code: Some(Box::new(|code: &str, lang: Option<&str>| {
                vec![
                    format!(
                        "H<{}:{}>",
                        lang.unwrap_or("null"),
                        code.split('\n').next().unwrap_or("")
                    ),
                    "H<second>".to_string(),
                ]
            })),
            code_block_indent: Some(">>".to_string()),
            ..probe_theme()
        };
        let mut component = Markdown::new(text, 0, 0, theme, None, None);
        component.render(width)
    }

    // Cache + setText invalidation (Rust-observable behavior of
    // markdown.ts:258-292, 296-298, 386-391).
    #[test]
    fn cache_hit_and_invalidation() {
        set_test_capabilities(false);
        let mut component = Markdown::new("# Hello", 0, 0, probe_theme(), None, None);
        let first = component.render(40);
        // Same text + width: cache hit returns the same lines.
        let second = component.render(40);
        assert_eq!(first, second);
        // Different width re-renders.
        let wider = component.render(60);
        assert_ne!(first, wider);
        // set_text invalidates and re-renders.
        component.set_text("## Changed");
        let changed = component.render(40);
        assert_eq!(changed, vec!["#<**Changed**>                          ".to_string()]);
        // set_text to the same value still re-renders (lines cache cleared)
        // and exercises the surviving token cache (same normalized source).
        component.set_text("# Hello");
        assert_eq!(component.render(40), first);
    }

    #[test]
    fn token_cache_survives_invalidate() {
        set_test_capabilities(false);
        let mut component = Markdown::new("- a\n- b", 0, 0, probe_theme(), None, None);
        let first = component.render(40);
        component.invalidate();
        // Token cache survives invalidation: same source re-lexes to the
        // same stream, so the re-render is identical.
        let second = component.render(40);
        assert_eq!(first, second);
    }

    #[test]
    fn heading_h1() {
        assert_eq!(
            render_case("# Hello", 0, 0, 40),
        vec![
        "#<**_<Hello>_**>                        ",
        ]
        );
    }

    #[test]
    fn heading_h2() {
        assert_eq!(
            render_case("## Sub", 0, 0, 40),
        vec![
        "#<**Sub**>                              ",
        ]
        );
    }

    #[test]
    fn heading_h3_prefix_styled() {
        assert_eq!(
            render_case("### Level three", 0, 0, 40),
        vec![
        "#<**### **>#<**Level three**>           ",
        ]
        );
    }

    #[test]
    fn multi_block_spacing() {
        assert_eq!(
            render_case("# Title\n\nBody paragraph here.\n\nAnother one.", 0, 0, 40),
        vec![
        "#<**_<Title>_**>                        ",
        "                                        ",
        "Body paragraph here.                    ",
        "                                        ",
        "Another one.                            ",
        ]
        );
    }

    #[test]
    fn paragraph_emphasis_codespan_link() {
        set_test_capabilities(false);
        assert_eq!(
            render_case("Some **bold** and *ital* and `code` and [link](https://example.com) text", 0, 0, 40),
        vec![
        "Some **bold** and _ital_ and C<code> and",
        "L<_<link>_>U< (https://example.com)>    ",
        "text                                    ",
        ]
        );
    }

    #[test]
    fn links_mixed() {
        set_test_capabilities(false);
        assert_eq!(
            render_case("See [pi](https://github.com/pi) and <https://example.com> and mail [a@b.com](mailto:a@b.com)", 0, 0, 40),
        vec![
        "See L<_<pi>_>U< (https://github.com/pi)>",
        "and L<_<https://example.com>_> and mail ",
        "L<_<a@b.com>_>                          ",
        ]
        );
    }

    #[test]
    fn code_fence_with_lang() {
        assert_eq!(
            render_case("```rust\nfn main() {}\n```", 0, 0, 40),
        vec![
        "B<```rust>                              ",
        "  CB<fn main() {}>                      ",
        "B<```>                                  ",
        ]
        );
    }

    #[test]
    fn code_fence_plain() {
        assert_eq!(
            render_case("```\nplain\n```", 0, 0, 40),
        vec![
        "B<```>                                  ",
        "  CB<plain>                             ",
        "B<```>                                  ",
        ]
        );
    }

    #[test]
    fn indented_code() {
        assert_eq!(
            render_case("    indented code\n    line two", 0, 0, 40),
        vec![
        "B<```>                                  ",
        "  CB<indented code>                     ",
        "  CB<line two>                          ",
        "B<```>                                  ",
        ]
        );
    }

    #[test]
    fn blockquote() {
        assert_eq!(
            render_case("> quoted text\n> more quote", 0, 0, 40),
        vec![
        "|Q<_quoted text                         ",
        "|more quote_>                           ",
        ]
        );
    }

    #[test]
    fn nested_blockquote() {
        assert_eq!(
            render_case("> outer\n> > nested\n> back", 0, 0, 40),
        vec![
        "|Q<_outer_>                             ",
        "|Q<__>                                  ",
        "|Q<_|Q<_nested_>                        ",
        "|Q<_|back_>_>                           ",
        ]
        );
    }

    #[test]
    fn list_in_blockquote() {
        assert_eq!(
            render_case("> - a\n> - b", 0, 0, 40),
        vec![
        "|Q<_*- a_>                              ",
        "|Q<_*- b_>                              ",
        ]
        );
    }

    #[test]
    fn nested_list() {
        assert_eq!(
            render_case("- one\n- two\n  - nested a\n  - nested b\n- three", 0, 0, 40),
        vec![
        "*- one                                  ",
        "*- two                                  ",
        "    *- nested a                         ",
        "    *- nested b                         ",
        "*- three                                ",
        ]
        );
    }

    #[test]
    fn loose_list() {
        assert_eq!(
            render_case("- one\n\n- two", 0, 0, 40),
        vec![
        "*- one                                  ",
        "                                        ",
        "*- two                                  ",
        ]
        );
    }

    #[test]
    fn task_list() {
        assert_eq!(
            render_case("- [ ] todo\n- [x] done", 0, 0, 40),
        vec![
        "*- [ ] todo                             ",
        "*- [x] done                             ",
        ]
        );
    }

    #[test]
    fn ordered_list() {
        assert_eq!(
            render_case("3. three\n4. four", 0, 0, 40),
        vec![
        "*3. three                               ",
        "*4. four                                ",
        ]
        );
    }

    #[test]
    fn table_with_alignment() {
        assert_eq!(
            render_case("| Left | Right |\n|:-----|-----:|\n| a | b |", 0, 0, 40),
        vec![
        "┌──────┬───────┐                        ",
        "│ **Left** │ **Right** │                ",
        "├──────┼───────┤                        ",
        "│ a    │ b     │                        ",
        "└──────┴───────┘                        ",
        ]
        );
    }

    #[test]
    fn hr() {
        assert_eq!(
            render_case("---", 0, 0, 40),
        vec![
        "-───────────────────────────────────────",
        "─-                                      ",
        ]
        );
    }

    #[test]
    fn html_block() {
        assert_eq!(
            render_case("<div>hello</div>", 0, 0, 40),
        vec![
        "<div>hello</div>                        ",
        ]
        );
    }

    #[test]
    fn inline_html() {
        assert_eq!(
            render_case("<span>inline</span> text", 0, 0, 40),
        vec![
        "<span>inline</span> text                ",
        ]
        );
    }

    #[test]
    fn latex_block() {
        assert_eq!(
            render_case("$$x^2 + y$$", 0, 0, 40),
        vec![
        "x² + y                                  ",
        ]
        );
    }

    #[test]
    fn latex_inline() {
        assert_eq!(
            render_case("Math $x^2$ here", 0, 0, 40),
        vec![
        "Math x² here                            ",
        ]
        );
    }

    #[test]
    fn empty_text() {
        assert_eq!(
            render_case("", 0, 0, 40),
        Vec::<String>::new()
        );
    }

    #[test]
    fn whitespace_only_text() {
        assert_eq!(
            render_case("   ", 0, 0, 40),
        Vec::<String>::new()
        );
    }

    #[test]
    fn padding() {
        assert_eq!(
            render_case("padded text", 2, 1, 20),
        vec![
        "                    ",
        "  padded text       ",
        "                    ",
        ]
        );
    }

    #[test]
    fn heading_code_spacing() {
        assert_eq!(
            render_case("# Head\n\n```js\ncode()\n```", 0, 0, 40),
        vec![
        "#<**_<Head>_**>                         ",
        "                                        ",
        "B<```js>                                ",
        "  CB<code()>                            ",
        "B<```>                                  ",
        ]
        );
    }

    #[test]
    fn paragraph_code_space_token() {
        assert_eq!(
            render_case("para\n\n```\nc\n```\n", 0, 0, 40),
        vec![
        "para                                    ",
        "                                        ",
        "B<```>                                  ",
        "  CB<c>                                 ",
        "B<```>                                  ",
        ]
        );
    }

    #[test]
    fn escape_preserved() {
        assert_eq!(
            render_case_opts(
                "a \\* b",
                40,
                MarkdownOptions { preserve_backslash_escapes: Some(true), ..Default::default() },
            ),
        vec![
        "a \\* b                                  ",
        ]
        );
    }

    #[test]
    fn ordered_list_markers_preserved() {
        assert_eq!(
            render_case_opts(
                "5. five\n6. six",
                40,
                MarkdownOptions { preserve_ordered_list_markers: Some(true), ..Default::default() },
            ),
        vec![
        "*5. five                                ",
        "*6. six                                 ",
        ]
        );
    }

    #[test]
    fn ordered_list_paren_markers_preserved() {
        assert_eq!(
            render_case_opts(
                "1) paren\n2) style",
                40,
                MarkdownOptions { preserve_ordered_list_markers: Some(true), ..Default::default() },
            ),
        vec![
        "*1) paren                               ",
        "*2) style                               ",
        ]
        );
    }

    #[test]
    fn render_latex_disabled_inline() {
        assert_eq!(
            render_case_opts(
                "Math $x^2$ here",
                40,
                MarkdownOptions { render_latex: Some(false), ..Default::default() },
            ),
        vec![
        "Math $x^2$ here                         ",
        ]
        );
    }

    #[test]
    fn transform_option() {
        assert_eq!(
            render_case_opts(
                "some text",
                40,
                MarkdownOptions {
                    transform: Some(Box::new(|s: &str, w: usize| {
                        format!("{}|w{}", s.to_uppercase(), w)
                    })),
                    ..Default::default()
                },
            ),
        vec![
        "SOME TEXT|w40                           ",
        ]
        );
    }

    #[test]
    fn default_text_style_heading() {
        assert_eq!(
            render_case_styled("# Styled", 40),
        vec![
        "#<**_<Styled>_**>                       ",
        ]
        );
    }

    #[test]
    fn default_text_style_mixed_inline() {
        assert_eq!(
            render_case_styled("plain **mix** `c`", 40),
        vec![
        "**C<plain >******C<mix>******C<**C<     ",
        ">**C<c>                                 ",
        ]
        );
    }

    #[test]
    fn highlight_code_and_indent() {
        assert_eq!(
            render_case_hl("```py\na\nb\n```", 40),
        vec![
        "B<```py>                                ",
        ">>H<py:a>                               ",
        ">>H<second>                             ",
        "B<```>                                  ",
        ]
        );
    }

    #[test]
    fn link_hyperlink_osc8() {
        set_test_capabilities(true);
        let mut component =
            Markdown::new("go [here](https://example.com) now", 0, 0, probe_theme(), None, None);
        assert_eq!(
            component.render(40),
        vec![
        "go \u{001b}]8;;https://example.com\u{001b}\\L<_<here>_>\u{001b}]8;;\u{001b}\\ now                      ",
        ]
        );
    }

    #[test]
    fn link_hyperlink_fallback() {
        set_test_capabilities(false);
        assert_eq!(
            render_case("go [here](https://example.com) now", 0, 0, 40),
        vec![
        "go L<_<here>_>U< (https://example.com)> ",
        "now                                     ",
        ]
        );
    }

}
