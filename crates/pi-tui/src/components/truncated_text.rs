//! Port of `packages/tui/src/components/truncated-text.ts` — `TruncatedText`:
//! like `Text` but truncates to a single line fitting the viewport width
//! instead of wrapping.

use crate::component::Component;
use crate::utils::{truncate_to_width_default, visible_width};

/// Port of `TruncatedText` (truncated-text.ts:6-58).
pub struct TruncatedText {
    text: String,
    padding_x: usize,
    padding_y: usize,
}

impl TruncatedText {
    /// `constructor(text, paddingX = 0, paddingY = 0)`
    /// (truncated-text.ts:11-15).
    pub fn new(text: impl Into<String>, padding_x: usize, padding_y: usize) -> Self {
        TruncatedText {
            text: text.into(),
            padding_x,
            padding_y,
        }
    }

    /// The wrapped text (TS field access `component.text`).
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl Component for TruncatedText {
    /// `render(width)` (truncated-text.ts:21-57).
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();

        // Empty line padded to width (truncated-text.ts:24).
        let empty_line = " ".repeat(width);

        // Add vertical padding above (truncated-text.ts:26-28).
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        // Calculate available width after horizontal padding
        // (truncated-text.ts:30-31). i64 math so `width - paddingX * 2`
        // clamps through `Math.max(1, ...)` exactly like JS.
        let available_width =
            ((width as i64) - 2 * (self.padding_x as i64)).max(1) as usize;

        // Take only the first line (stop at newline)
        // (truncated-text.ts:33-38).
        let single_line_text = match self.text.find('\n') {
            Some(newline_index) => &self.text[..newline_index],
            None => self.text.as_str(),
        };

        // Truncate text if needed (accounting for ANSI codes)
        // (truncated-text.ts:40).
        let display_text = truncate_to_width_default(single_line_text, available_width);

        // Add horizontal padding (truncated-text.ts:42-44).
        let left_padding = " ".repeat(self.padding_x);
        let right_padding = " ".repeat(self.padding_x);
        let line_with_padding = format!("{left_padding}{display_text}{right_padding}");

        // Pad line to exactly width characters (truncated-text.ts:46-49).
        let line_visible_width = visible_width(&line_with_padding);
        let padding_needed = width.saturating_sub(line_visible_width);
        let final_line = format!("{line_with_padding}{}", " ".repeat(padding_needed));

        result.push(final_line);

        // Add vertical padding below (truncated-text.ts:51-53).
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        result
    }

    /// `invalidate()` (truncated-text.ts:17-19) — no cached state to
    /// invalidate currently.
    fn invalidate(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::strip_terminal_sequences;

    // Ported from upstream test/truncated-text.test.ts. The upstream suite
    // builds colors with `new Chalk({ level: 3 })` (truecolor); the escape
    // sequences below are the equivalent truecolor SGR codes.
    fn red(text: &str) -> String {
        format!("\x1b[38;2;255;0;0m{text}\x1b[39m")
    }

    fn blue(text: &str) -> String {
        format!("\x1b[38;2;0;0;255m{text}\x1b[39m")
    }

    // describe("TruncatedText component")

    // it("pads output lines to exactly match width")
    #[test]
    fn pads_output_lines_to_exactly_match_width() {
        let mut text = TruncatedText::new("Hello world", 1, 0);
        let lines = text.render(50);

        // Should have exactly one content line (no vertical padding)
        assert_eq!(lines.len(), 1);

        // Line should be exactly 50 visible characters
        let visible_len = visible_width(&lines[0]);
        assert_eq!(visible_len, 50);
    }

    // it("pads output with vertical padding lines to width")
    #[test]
    fn pads_output_with_vertical_padding_lines_to_width() {
        let mut text = TruncatedText::new("Hello", 0, 2);
        let lines = text.render(40);

        // Should have 2 padding lines + 1 content line + 2 padding lines = 5 total
        assert_eq!(lines.len(), 5);

        // All lines should be exactly 40 characters
        for line in &lines {
            assert_eq!(visible_width(line), 40);
        }
    }

    // it("truncates long text and pads to width")
    #[test]
    fn truncates_long_text_and_pads_to_width() {
        let long_text =
            "This is a very long piece of text that will definitely exceed the available width";
        let mut text = TruncatedText::new(long_text, 1, 0);
        let lines = text.render(30);

        assert_eq!(lines.len(), 1);

        // Should be exactly 30 characters
        assert_eq!(visible_width(&lines[0]), 30);

        // Should contain ellipsis
        let stripped = strip_terminal_sequences(&lines[0]);
        assert!(stripped.contains("..."));
    }

    // it("preserves ANSI codes in output and pads correctly")
    #[test]
    fn preserves_ansi_codes_in_output_and_pads_correctly() {
        let styled_text = format!("{} {}", red("Hello"), blue("world"));
        let mut text = TruncatedText::new(styled_text, 1, 0);
        let lines = text.render(40);

        assert_eq!(lines.len(), 1);

        // Should be exactly 40 visible characters (ANSI codes don't count)
        assert_eq!(visible_width(&lines[0]), 40);

        // Should preserve the color codes
        assert!(lines[0].contains("\x1b["));
    }

    // it("truncates styled text and adds reset code before ellipsis")
    #[test]
    fn truncates_styled_text_and_adds_reset_code_before_ellipsis() {
        let long_styled_text = red("This is a very long red text that will be truncated");
        let mut text = TruncatedText::new(long_styled_text, 1, 0);
        let lines = text.render(20);

        assert_eq!(lines.len(), 1);

        // Should be exactly 20 visible characters
        assert_eq!(visible_width(&lines[0]), 20);

        // Should contain reset code before ellipsis
        assert!(lines[0].contains("\x1b[0m..."));
    }

    // it("handles text that fits exactly")
    #[test]
    fn handles_text_that_fits_exactly() {
        // With paddingX=1, available width is 30-2=28
        // "Hello world" is 11 chars, fits comfortably
        let mut text = TruncatedText::new("Hello world", 1, 0);
        let lines = text.render(30);

        assert_eq!(lines.len(), 1);
        assert_eq!(visible_width(&lines[0]), 30);

        // Should NOT contain ellipsis
        let stripped = strip_terminal_sequences(&lines[0]);
        assert!(!stripped.contains("..."));
    }

    // it("handles empty text")
    #[test]
    fn handles_empty_text() {
        let mut text = TruncatedText::new("", 1, 0);
        let lines = text.render(30);

        assert_eq!(lines.len(), 1);
        assert_eq!(visible_width(&lines[0]), 30);
    }

    // it("stops at newline and only shows first line")
    #[test]
    fn stops_at_newline_and_only_shows_first_line() {
        let multiline_text = "First line\nSecond line\nThird line";
        let mut text = TruncatedText::new(multiline_text, 1, 0);
        let lines = text.render(40);

        assert_eq!(lines.len(), 1);
        assert_eq!(visible_width(&lines[0]), 40);

        // Should only contain "First line"
        let stripped = js_trim_stripped(&lines[0]);
        assert!(stripped.contains("First line"));
        assert!(!stripped.contains("Second line"));
        assert!(!stripped.contains("Third line"));
    }

    // it("truncates first line even with newlines in text")
    #[test]
    fn truncates_first_line_even_with_newlines_in_text() {
        let long_multiline_text =
            "This is a very long first line that needs truncation\nSecond line";
        let mut text = TruncatedText::new(long_multiline_text, 1, 0);
        let lines = text.render(25);

        assert_eq!(lines.len(), 1);
        assert_eq!(visible_width(&lines[0]), 25);

        // Should contain ellipsis and not second line
        let stripped = strip_terminal_sequences(&lines[0]);
        assert!(stripped.contains("..."));
        assert!(!stripped.contains("Second line"));
    }

    /// The upstream `.replace(...).trim()` chain.
    fn js_trim_stripped(line: &str) -> String {
        crate::utils::js_trim(&strip_terminal_sequences(line)).to_string()
    }
}
