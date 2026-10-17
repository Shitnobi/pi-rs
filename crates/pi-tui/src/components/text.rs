//! Port of `packages/tui/src/components/text.ts` — the `Text` component:
//! multi-line text with word wrapping, padding, and an optional background
//! function.
//!
//! Divergence from TS: `customBgFn?: (text: string) => string` is
//! [`Option<CustomBgFn>`] (`Box<dyn Fn(&str) -> String>`), and the upstream
//! `flattenLines(result)` call (utils.ts:896) is omitted — it is a no-op in
//! TS (`Number(line)` is discarded).

use crate::component::Component;
use crate::utils::{apply_background_to_line, js_trim, visible_width, wrap_text_with_ansi};

/// Background callback applied to a full-width padded line
/// (text.ts:9: `customBgFn?: (text: string) => string`).
pub type CustomBgFn = Box<dyn Fn(&str) -> String>;

/// Port of `Text` (text.ts:11-105) — displays multi-line text with word
/// wrapping.
pub struct Text {
    text: String,
    /// Left/right padding (text.ts:13).
    padding_x: usize,
    /// Top/bottom padding (text.ts:14).
    padding_y: usize,
    custom_bg_fn: Option<CustomBgFn>,

    // Cache for rendered output (text.ts:18-20).
    cached_text: Option<String>,
    cached_width: Option<usize>,
    cached_lines: Option<Vec<String>>,
}

impl Text {
    /// `constructor(text = "", paddingX = 1, paddingY = 1, customBgFn?)`
    /// (text.ts:22-28).
    pub fn new(
        text: impl Into<String>,
        padding_x: usize,
        padding_y: usize,
        custom_bg_fn: Option<CustomBgFn>,
    ) -> Self {
        Text {
            text: text.into(),
            padding_x,
            padding_y,
            custom_bg_fn,
            cached_text: None,
            cached_width: None,
            cached_lines: None,
        }
    }

    /// `setText(text)` (text.ts:30-35) — clears the render cache.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }

    /// `setCustomBgFn(customBgFn?)` (text.ts:37-42) — clears the render
    /// cache. Pass `None` to remove the background function.
    pub fn set_custom_bg_fn(&mut self, custom_bg_fn: Option<CustomBgFn>) {
        self.custom_bg_fn = custom_bg_fn;
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }

    /// `setPaddingX(paddingX)` (text.ts:44-47) — invalidates the cache.
    pub fn set_padding_x(&mut self, padding_x: usize) {
        self.padding_x = padding_x;
        self.invalidate();
    }

    /// `setPaddingY(paddingY)` — analog of `setPaddingX` for the vertical
    /// padding. NOTE: upstream `text.ts` has no `setPaddingY` (paddingY is
    /// constructor-only); added per port plan so callers are not forced to
    /// reconstruct. Semantics mirror `setPaddingX` exactly.
    pub fn set_padding_y(&mut self, padding_y: usize) {
        self.padding_y = padding_y;
        self.invalidate();
    }

    /// The current text (TS field access `component.text`).
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl Component for Text {
    /// `render(width)` (text.ts:49-105).
    fn render(&mut self, width: usize) -> Vec<String> {
        // Check cache (text.ts:50-53).
        if let Some(lines) = &self.cached_lines {
            if self.cached_text.as_deref() == Some(self.text.as_str())
                && self.cached_width == Some(width)
            {
                return lines.clone();
            }
        }

        // Don't render anything if there's no actual text (text.ts:55-63).
        // NOTE: this early return skips the `result.length > 0` fallback at
        // the end, so empty text renders zero lines.
        if self.text.is_empty() || js_trim(&self.text).is_empty() {
            let result: Vec<String> = Vec::new();
            self.cached_text = Some(self.text.clone());
            self.cached_width = Some(width);
            self.cached_lines = Some(result.clone());
            return result;
        }

        // Replace tabs with 3 spaces (text.ts:65).
        let normalized_text = self.text.replace('\t', "   ");

        // Reduce margins when necessary so content and padding fit within
        // the available width (text.ts:68-69).
        // `Math.min(paddingX, Math.max(0, Math.floor((width - 1) / 2)))` —
        // i64 math so width 0 clamps to 0 exactly like the JS floor.
        let padding_x = self
            .padding_x
            .min(((width as i64 - 1) / 2).max(0) as usize);
        let content_width = ((width as i64) - 2 * (padding_x as i64)).max(1) as usize;

        // Wrap text (this preserves ANSI codes but does NOT pad)
        // (text.ts:72).
        let wrapped_lines = wrap_text_with_ansi(&normalized_text, content_width);

        // Add margins and background to each line (text.ts:74-93).
        let left_margin = " ".repeat(padding_x);
        let right_margin = " ".repeat(padding_x);
        let mut content_lines: Vec<String> = Vec::new();

        for line in wrapped_lines {
            let line_with_margins = format!("{left_margin}{line}{right_margin}");

            // Apply background if specified (this also pads to full width).
            if let Some(bg_fn) = &self.custom_bg_fn {
                content_lines.push(apply_background_to_line(&line_with_margins, width, |t| {
                    bg_fn(t)
                }));
            } else {
                // No background - just pad to width with spaces.
                let visible_len = visible_width(&line_with_margins);
                let padding_needed = width.saturating_sub(visible_len);
                content_lines.push(format!("{line_with_margins}{}", " ".repeat(padding_needed)));
            }
        }

        // Add top/bottom padding (empty lines) (text.ts:95-101).
        let empty_line = " ".repeat(width);
        let mut empty_lines: Vec<String> = Vec::new();
        for _ in 0..self.padding_y {
            let line = if let Some(bg_fn) = &self.custom_bg_fn {
                apply_background_to_line(&empty_line, width, |t| bg_fn(t))
            } else {
                empty_line.clone()
            };
            empty_lines.push(line);
        }

        let mut result = empty_lines.clone();
        result.extend(content_lines);
        result.extend(empty_lines);
        // `flattenLines(result)` is a TS no-op — omitted.

        // Update cache (text.ts:103-106).
        self.cached_text = Some(self.text.clone());
        self.cached_width = Some(width);
        self.cached_lines = Some(result.clone());

        // text.ts:108 — the fallback is unreachable for non-empty text
        // (`wrapTextWithAnsi` always yields at least one line), but kept
        // for parity.
        if !result.is_empty() {
            result
        } else {
            vec![String::new()]
        }
    }

    /// `invalidate()` (text.ts:110-114) — clears the render cache.
    fn invalidate(&mut self) {
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Upstream has no dedicated text component test file; these are
    // faithful tests written per the port plan (tranche T3).

    #[test]
    fn renders_single_line_with_padding() {
        let mut text = Text::new("Hello", 1, 1, None);
        let lines = text.render(20);
        // 1 blank + 1 content + 1 blank
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], " ".repeat(20));
        assert_eq!(lines[2], " ".repeat(20));
        // " Hello " padded to 20
        assert_eq!(lines[1], format!("{}{}{}", " ", "Hello", " ".repeat(14)));
        assert_eq!(visible_width(&lines[1]), 20);
    }

    #[test]
    fn wraps_long_text_to_multiple_lines() {
        let mut text = Text::new("one two three four", 0, 0, None);
        let lines = text.render(8);
        assert!(lines.len() >= 2);
        for line in &lines {
            assert_eq!(visible_width(line), 8);
        }
        assert!(lines[0].starts_with("one two"));
    }

    #[test]
    fn renders_each_hard_line_separately() {
        let mut text = Text::new("a\nb", 0, 0, None);
        let lines = text.render(5);
        assert_eq!(lines, vec!["a    ".to_string(), "b    ".to_string()]);
    }

    #[test]
    fn replaces_tabs_with_three_spaces() {
        let mut text = Text::new("a\tb", 0, 0, None);
        let lines = text.render(10);
        assert_eq!(lines[0], "a   b     ");
    }

    #[test]
    fn empty_text_renders_no_lines() {
        let mut text = Text::new("", 1, 1, None);
        assert_eq!(text.render(10), Vec::<String>::new());
    }

    #[test]
    fn whitespace_only_text_renders_no_lines() {
        let mut text = Text::new("   \n  ", 1, 1, None);
        assert_eq!(text.render(10), Vec::<String>::new());
    }

    #[test]
    fn applies_custom_background_function_to_all_lines() {
        let bg = |t: &str| format!("|{t}|") as String;
        let mut text = Text::new("hi", 1, 1, Some(Box::new(bg)));
        let lines = text.render(10);
        assert_eq!(lines.len(), 3);
        // Empty padding lines get the background (text.ts:98-99).
        assert_eq!(lines[0], "|          |");
        assert_eq!(lines[2], "|          |");
        // " hi " padded to 10, then wrapped by the background fn.
        assert_eq!(lines[1], "| hi       |");
    }

    #[test]
    fn reduces_padding_when_width_is_too_narrow() {
        // paddingX=5 but width=3: padding clamps to floor((3-1)/2) = 1
        // (text.ts:68).
        let mut text = Text::new("hello", 5, 0, None);
        let lines = text.render(3);
        // contentWidth 1 wraps "hello" one character per line.
        assert_eq!(lines.len(), 5);
        for line in &lines {
            assert_eq!(visible_width(line), 3);
        }
    }

    #[test]
    fn caches_render_by_text_and_width() {
        let mut text = Text::new("Hello", 0, 0, None);
        let first = text.render(10);
        // Same text + width: cached output returned.
        let second = text.render(10);
        assert_eq!(first, second);
        // Different width: re-rendered for the new width.
        let wider = text.render(20);
        assert_eq!(visible_width(&wider[0]), 20);
    }

    #[test]
    fn set_text_updates_output() {
        let mut text = Text::new("Hello", 0, 0, None);
        text.render(10);
        text.set_text("World");
        let lines = text.render(10);
        assert!(lines[0].starts_with("World"));
    }

    #[test]
    fn set_custom_bg_fn_invalidates_cache() {
        let mut text = Text::new("hi", 0, 0, None);
        let plain = text.render(10);
        text.set_custom_bg_fn(Some(Box::new(|t: &str| format!("[{t}]") as String)));
        let styled = text.render(10);
        assert_ne!(plain, styled);
        assert!(styled[0].starts_with('['));
        // Clearing the background restores plain padding.
        text.set_custom_bg_fn(None);
        assert_eq!(text.render(10), plain);
    }

    #[test]
    fn set_padding_x_invalidates_cache() {
        let mut text = Text::new("hi", 0, 0, None);
        assert_eq!(text.render(10), vec!["hi        ".to_string()]);
        text.set_padding_x(2);
        assert_eq!(text.render(10), vec!["  hi      ".to_string()]);
    }

    #[test]
    fn set_padding_y_invalidates_cache() {
        let mut text = Text::new("hi", 0, 0, None);
        assert_eq!(text.render(10).len(), 1);
        text.set_padding_y(2);
        assert_eq!(text.render(10).len(), 5);
    }

    #[test]
    fn invalidate_clears_cache() {
        let mut text = Text::new("hi", 0, 0, None);
        let first = text.render(10);
        text.invalidate();
        assert_eq!(text.render(10), first);
    }

    #[test]
    fn preserves_ansi_codes_when_padding() {
        let styled = "\x1b[31mHello\x1b[39m";
        let mut text = Text::new(styled, 1, 0, None);
        let lines = text.render(20);
        assert_eq!(lines.len(), 1);
        assert_eq!(visible_width(&lines[0]), 20);
        assert!(lines[0].contains("\x1b[31mHello"));
    }
}
