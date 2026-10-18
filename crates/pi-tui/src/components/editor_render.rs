//! Rendering: `render` (editor.ts:521-631), `layoutText`
//! (editor.ts:998-1084), and mouse hit-testing `handleMouse`
//! (editor.ts:633-694).
//!
//! **Owner: T4 delegate E5.**

use std::cell::RefCell;
use std::rc::Rc;

use crate::component::{
    render_fake_cursor, Component, ComponentMouseResult, ComponentRef, TuiMouseButton,
    TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType, CURSOR_MARKER,
};
use crate::utils::{utf16_len, visible_width};

use super::editor::{slice_utf16, Editor, LayoutLine};
use super::editor_word_wrap::word_wrap_line;

impl Editor {
    /// `render(width)` (editor.ts:521-631) — the inherent method behind
    /// `Component::render`. Emits borders, visible wrapped lines with the
    /// fake cursor (`render_fake_cursor`) + hardware cursor marker when
    /// focused, and the autocomplete list when active.
    pub(crate) fn render_editor(&mut self, width: usize) -> Vec<String> {
        // editor.ts:523-526
        let max_padding = (((width as i64) - 1) / 2).max(0) as usize;
        let padding_x = self.padding_x.min(max_padding);
        let content_width = ((width as i64) - (padding_x as i64) * 2).max(1) as usize;

        // Layout width: with padding the cursor can overflow into it,
        // without padding we reserve 1 column for the cursor
        // (editor.ts:528-531).
        let layout_width = if padding_x > 0 {
            content_width
        } else {
            content_width.saturating_sub(1)
        }
        .max(1);

        // Store for cursor navigation (must match wrapping width)
        // (editor.ts:533-534).
        self.last_width = layout_width;

        // Layout the text (editor.ts:536-537).
        let layout_lines = self.layout_text(layout_width);

        // Calculate max visible lines: 30% of terminal height, minimum 5
        // lines (editor.ts:539-541).
        let terminal_rows = self.tui.borrow().terminal_rows();
        let max_visible_lines = 5.max((terminal_rows as f64 * 0.3).floor() as usize);

        // Find the cursor line index in layoutLines (editor.ts:543-545).
        let cursor_line_index = layout_lines
            .iter()
            .position(|line| line.has_cursor)
            .unwrap_or(0);

        // Adjust scroll offset to keep cursor visible (editor.ts:547-552).
        if cursor_line_index < self.scroll_offset {
            self.scroll_offset = cursor_line_index;
        } else if cursor_line_index >= self.scroll_offset + max_visible_lines {
            self.scroll_offset = cursor_line_index + 1 - max_visible_lines;
        }

        // Clamp scroll offset to valid range (editor.ts:554-556).
        let max_scroll_offset = layout_lines.len().saturating_sub(max_visible_lines);
        self.scroll_offset = self.scroll_offset.min(max_scroll_offset);

        // Get visible lines slice (editor.ts:558-560).
        let visible_end = (self.scroll_offset + max_visible_lines).min(layout_lines.len());
        let visible_lines = &layout_lines[self.scroll_offset..visible_end];
        self.rendered_visible_line_count = visible_lines.len();

        let mut result: Vec<String> = Vec::new();
        let left_padding = " ".repeat(padding_x);
        let right_padding = left_padding.clone();

        // Render top border (with scroll indicator if scrolled down)
        // (editor.ts:566-567).
        result.push(self.render_top_border(width, self.scroll_offset));

        // Render each visible layout line (editor.ts:569-609).
        // Emit hardware cursor marker when focused so TUI can position the
        // hardware cursor for IME candidate-window placement even while
        // autocomplete (e.g. slash-command menu) is visible.
        let emit_cursor_marker = self.focused;

        for layout_line in visible_lines {
            let mut display_text = layout_line.text.clone();
            let mut line_visible_width = visible_width(&layout_line.text);
            let mut cursor_in_padding = false;

            // Add cursor if this line has it (editor.ts:578-602)
            if layout_line.has_cursor {
                if let Some(cursor_pos) = layout_line.cursor_pos {
                    let before = slice_utf16(&layout_line.text, 0, cursor_pos);
                    let text_len = utf16_len(&layout_line.text);

                    // Hardware cursor marker (zero-width, emitted before fake
                    // cursor for IME positioning)
                    let marker = if emit_cursor_marker {
                        CURSOR_MARKER
                    } else {
                        ""
                    };

                    if cursor_pos < text_len {
                        // Cursor is on a character (grapheme) - replace it
                        // with highlighted version. Get the first grapheme
                        // from 'after' (editor.ts:586-592).
                        let after = slice_utf16(&layout_line.text, cursor_pos, text_len);
                        let first_grapheme = self
                            .segment_graphemes(&after)
                            .first()
                            .map(|g| g.segment.clone())
                            .unwrap_or_default();
                        let rest_after =
                            slice_utf16(&after, utf16_len(&first_grapheme), utf16_len(&after));
                        let cursor = render_fake_cursor(&first_grapheme);
                        display_text = format!("{before}{marker}{cursor}{rest_after}");
                        // lineVisibleWidth stays the same - we're replacing,
                        // not adding
                    } else {
                        // Cursor is at the end - add highlighted space
                        // (editor.ts:594-602).
                        let cursor = render_fake_cursor(" ");
                        display_text = format!("{before}{marker}{cursor}");
                        line_visible_width += 1;
                        // If cursor overflows content width into the
                        // padding, flag it
                        if line_visible_width > content_width && padding_x > 0 {
                            cursor_in_padding = true;
                        }
                    }
                }
            }

            // Calculate padding based on actual visible width
            // (editor.ts:604-606).
            let padding = " ".repeat(content_width.saturating_sub(line_visible_width));
            let line_right_padding = if cursor_in_padding {
                right_padding.chars().skip(1).collect::<String>()
            } else {
                right_padding.clone()
            };

            // Render the line (no side borders, just horizontal lines above
            // and below) (editor.ts:608-609).
            result.push(format!(
                "{left_padding}{display_text}{padding}{line_right_padding}"
            ));
        }

        // Render bottom border (with scroll indicator if more content below)
        // (editor.ts:611-613).
        let lines_below = layout_lines
            .len()
            .saturating_sub(self.scroll_offset + visible_lines.len());
        result.push(self.render_bottom_border(width, lines_below));

        // Add autocomplete list if active (editor.ts:615-630).
        self.rendered_autocomplete_height = 0;
        if self.autocomplete_state.is_some() {
            if let Some(list) = self.autocomplete_list.as_mut() {
                let autocomplete_result = list.render(content_width);
                let extra: Vec<String> = autocomplete_result
                    .into_iter()
                    .map(|line| {
                        let line_width = visible_width(&line);
                        let line_padding = " ".repeat(content_width.saturating_sub(line_width));
                        format!("{left_padding}{line}{line_padding}{right_padding}")
                    })
                    .collect();
                self.rendered_autocomplete_height = extra.len();
                result.extend(extra);
            }
        }

        result
    }

    /// `handleMouse(event)` (editor.ts:633-694) — the inherent method behind
    /// `Component::handle_mouse`. Click positions the cursor via the visual
    /// line map; autocomplete rows forward to the owned `SelectList`.
    pub(crate) fn handle_mouse_impl(
        &mut self,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        // Autocomplete rows forward to the owned SelectList (editor.ts:634-651).
        let autocomplete_start_row = self.rendered_visible_line_count as i64 + 2;
        if self.autocomplete_state.is_some()
            && self.autocomplete_list.is_some()
            && event.y >= autocomplete_start_row
            && event.y < autocomplete_start_row + self.rendered_autocomplete_height as i64
        {
            let max_padding = (((event.width) - 1) / 2).max(0) as usize;
            let padding_x = self.padding_x.min(max_padding);
            let content_width = ((event.width) - (padding_x as i64) * 2).max(1) as usize;
            let mut forwarded = event.clone();
            forwarded.x = event.x - padding_x as i64;
            forwarded.y = event.y - autocomplete_start_row;
            forwarded.width = content_width as i64;
            forwarded.height = self.rendered_autocomplete_height as i64;
            return self
                .forward_mouse_to_autocomplete_list(&forwarded)
                .map(ComponentMouseResult::Result);
        }

        // Leave press/drag/release unhandled so the renderer's screen-level
        // text selection can run over the editor rows (drag to select,
        // release to copy). The renderer synthesizes a click when press and
        // release land on the same cell without movement, which is the
        // gesture that positions the cursor (editor.ts:653-656).
        if event.event_type != TuiMouseEventType::Click || event.button != TuiMouseButton::Left {
            return None;
        }
        if event.y <= 0 || event.y > self.rendered_visible_line_count as i64 {
            return Some(ComponentMouseResult::Result(TuiMouseEventResult {
                handled: Some(true),
                focus: Some(true),
                ..Default::default()
            }));
        }

        // Click-to-position via the visual line map (editor.ts:658-680).
        let visual_lines = self.build_visual_line_map(self.last_width);
        let visual_line_index = (self.scroll_offset as i64 + event.y - 1) as usize;
        let Some(visual_line) = visual_lines.get(visual_line_index) else {
            return Some(ComponentMouseResult::Result(TuiMouseEventResult {
                handled: Some(true),
                focus: Some(true),
                ..Default::default()
            }));
        };
        let visual_line = visual_line.clone();
        let logical_line = self
            .state
            .lines
            .get(visual_line.logical_line)
            .cloned()
            .unwrap_or_default();
        let chunk_end = visual_line.start_col + visual_line.length;
        let chunk = slice_utf16(&logical_line, visual_line.start_col, chunk_end);
        let max_padding = (((event.width) - 1) / 2).max(0) as usize;
        let padding_x = self.padding_x.min(max_padding);
        let target_column = (event.x - padding_x as i64).max(0);
        let mut visible_column: i64 = 0;
        let mut target_index = utf16_len(&chunk);
        let mut last_grapheme_index = 0usize;
        for grapheme in self.segment_graphemes(&chunk) {
            let next_column = visible_column + visible_width(&grapheme.segment) as i64;
            last_grapheme_index = grapheme.index;
            if target_column < next_column {
                target_index = grapheme.index;
                break;
            }
            visible_column = next_column;
        }
        let is_last_segment = visual_line_index + 1 == visual_lines.len()
            || visual_lines
                .get(visual_line_index + 1)
                .map(|next| next.logical_line)
                != Some(visual_line.logical_line);
        if !is_last_segment && target_index == utf16_len(&chunk) && !chunk.is_empty() {
            target_index = last_grapheme_index;
        }

        // editor.ts:682-690
        self.state.cursor_line = visual_line.logical_line;
        self.set_cursor_col(visual_line.start_col + target_index);
        self.last_action = None;
        self.exit_history_browsing();
        if self.autocomplete_state.is_some() {
            self.update_autocomplete();
        }
        Some(ComponentMouseResult::Result(TuiMouseEventResult {
            handled: Some(true),
            focus: Some(true),
            ..Default::default()
        }))
    }

    /// `this.autocompleteList.handleMouse?.({ ...event, x, y, width, height })`
    /// followed by `result ? { ...result, focus: true } : undefined`
    /// (editor.ts:642-651). The owned `SelectList` is hosted by value, so it
    /// is temporarily wrapped in a `ComponentRef` to satisfy the trait's
    /// self-reference parameter (the `SelectList` impl ignores it, but the
    /// genuine reference is passed).
    fn forward_mouse_to_autocomplete_list(
        &mut self,
        event: &TuiMouseEvent,
    ) -> Option<TuiMouseEventResult> {
        let list = self.autocomplete_list.take()?;
        let typed = Rc::new(RefCell::new(list));
        let result = {
            let untyped: ComponentRef = typed.clone();
            let result = untyped.borrow_mut().handle_mouse(&untyped, event);
            result
        };
        let list = match Rc::try_unwrap(typed) {
            Ok(cell) => cell.into_inner(),
            Err(_) => unreachable!("SelectList self-ref must not be retained"),
        };
        self.autocomplete_list = Some(list);

        match result {
            Some(ComponentMouseResult::Result(mut result)) => {
                result.focus = Some(true);
                Some(result)
            }
            // Defensive: the SelectList never forwards, but if it did, keep
            // the TS spread semantics (`{ ...result, focus: true }`).
            Some(ComponentMouseResult::Forwarded(dispatch)) => Some(TuiMouseEventResult {
                handled: Some(true),
                capture: dispatch.capture,
                focus: Some(true),
                render: dispatch.render,
            }),
            None => None,
        }
    }

    /// `layoutText(contentWidth)` (editor.ts:998-1084): word-wrap every
    /// logical line (`word_wrap_line` with marker-aware graphemes) and
    /// assign cursor positions to the layout lines.
    pub(crate) fn layout_text(&mut self, content_width: usize) -> Vec<LayoutLine> {
        let mut layout_lines: Vec<LayoutLine> = Vec::new();

        // Empty editor (editor.ts:1002-1009)
        if self.state.lines.is_empty()
            || (self.state.lines.len() == 1 && self.state.lines[0].is_empty())
        {
            layout_lines.push(LayoutLine {
                text: String::new(),
                has_cursor: true,
                cursor_pos: Some(0),
            });
            return layout_lines;
        }

        // Process each logical line (editor.ts:1011-1082)
        for i in 0..self.state.lines.len() {
            let line = self.state.lines[i].clone();
            let is_current_line = i == self.state.cursor_line;
            let line_visible_width = visible_width(&line);

            if line_visible_width <= content_width {
                // Line fits in one layout line (editor.ts:1017-1028)
                if is_current_line {
                    layout_lines.push(LayoutLine {
                        text: line,
                        has_cursor: true,
                        cursor_pos: Some(self.state.cursor_col),
                    });
                } else {
                    layout_lines.push(LayoutLine {
                        text: line,
                        has_cursor: false,
                        cursor_pos: None,
                    });
                }
            } else {
                // Line needs wrapping - use word-aware wrapping
                // (editor.ts:1030-1032)
                let chunks =
                    word_wrap_line(&line, content_width, Some(&self.segment_graphemes(&line)));
                let chunk_count = chunks.len();

                for (chunk_index, chunk) in chunks.into_iter().enumerate() {
                    let cursor_pos = self.state.cursor_col;
                    let is_last_chunk = chunk_index + 1 == chunk_count;

                    // Determine if cursor is in this chunk
                    // For word-wrapped chunks, we need to handle the case
                    // where cursor might be in trimmed whitespace at end of
                    // chunk (editor.ts:1040-1062)
                    let mut has_cursor_in_chunk = false;
                    let mut adjusted_cursor_pos = 0usize;

                    if is_current_line {
                        if is_last_chunk {
                            // Last chunk: cursor belongs here if >= startIndex
                            has_cursor_in_chunk = cursor_pos >= chunk.start_index;
                            adjusted_cursor_pos = cursor_pos.saturating_sub(chunk.start_index);
                        } else {
                            // Non-last chunk: cursor belongs here if in range
                            // [startIndex, endIndex). But we need to handle
                            // the visual position in the trimmed text.
                            has_cursor_in_chunk =
                                cursor_pos >= chunk.start_index && cursor_pos < chunk.end_index;
                            if has_cursor_in_chunk {
                                adjusted_cursor_pos = cursor_pos - chunk.start_index;
                                // Clamp to text length (in case cursor was in
                                // trimmed whitespace)
                                let chunk_text_len = utf16_len(&chunk.text);
                                if adjusted_cursor_pos > chunk_text_len {
                                    adjusted_cursor_pos = chunk_text_len;
                                }
                            }
                        }
                    }

                    if has_cursor_in_chunk {
                        layout_lines.push(LayoutLine {
                            text: chunk.text,
                            has_cursor: true,
                            cursor_pos: Some(adjusted_cursor_pos),
                        });
                    } else {
                        layout_lines.push(LayoutLine {
                            text: chunk.text,
                            has_cursor: false,
                            cursor_pos: None,
                        });
                    }
                }
            }
        }

        layout_lines
    }
}

// =============================================================================
// Ported upstream tests (test/editor.test.ts)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::FAKE_CURSOR_START;
    use crate::test_support::create_test_tui_sized;
    use crate::utils::strip_terminal_sequences;

    use super::super::editor::{EditorOptions, EditorTheme};

    // describe("Scroll indicators")

    /// "centers scroll indicators on wide borders" (editor.test.ts:707-720).
    #[test]
    fn centers_scroll_indicators_on_wide_borders() {
        let width = 40;
        let mut editor = Editor::new(create_test_tui_sized(width, 24), EditorTheme::default());
        let text = (0..20)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        editor.set_text(&text);

        editor.render(width);
        for _ in 0..10 {
            editor.handle_input("\x1b[A");
        }

        let lines = editor.render(width);
        assert_eq!(
            strip_terminal_sequences(&lines[0]),
            format!("{} ↑ 9 more {}", "─".repeat(15), "─".repeat(15))
        );
        assert_eq!(
            strip_terminal_sequences(lines.last().unwrap()),
            format!("{} ↓ 4 more {}", "─".repeat(15), "─".repeat(15))
        );
    }

    /// "keeps truncated scroll indicators within width and preserves their
    /// color (issue #6962)" (editor.test.ts:722-744).
    #[test]
    fn keeps_truncated_scroll_indicators_within_width_and_preserves_color() {
        let width = 10;
        let border_color = |text: &str| format!("\x1b[35m{text}\x1b[39m");
        let theme = EditorTheme {
            border_color: Box::new(border_color),
            ..EditorTheme::default()
        };
        let mut editor = Editor::new(create_test_tui_sized(width, 24), theme);
        let text = (0..20)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        editor.set_text(&text);

        // Render once to initialize wrapping, then move the cursor so
        // content remains above and below the viewport.
        editor.render(width);
        for _ in 0..10 {
            editor.handle_input("\x1b[A");
        }

        let lines = editor.render(width);
        let top_border = lines[0].clone();
        let bottom_border = lines.last().unwrap().clone();

        let top_stripped = strip_terminal_sequences(&top_border);
        let bottom_stripped = strip_terminal_sequences(&bottom_border);
        assert!(
            top_stripped.starts_with("─── ↑"),
            "unexpected top border: {top_stripped:?}"
        );
        assert!(
            bottom_stripped.starts_with("─── ↓"),
            "unexpected bottom border: {bottom_stripped:?}"
        );
        assert_eq!(top_border, border_color(&top_stripped));
        assert_eq!(bottom_border, border_color(&bottom_stripped));
        for line in &lines {
            assert_eq!(
                visible_width(line),
                width,
                "line exceeds width {width}: {line:?}"
            );
        }
    }

    // describe("Grapheme-aware text wrapping")

    /// "wraps lines correctly when text contains wide emojis"
    /// (editor.test.ts:747-760).
    #[test]
    fn wraps_lines_correctly_when_text_contains_wide_emojis() {
        let mut editor = Editor::new(create_test_tui_sized(80, 24), EditorTheme::default());
        let width = 20;

        // ✅ is 2 columns wide, so "Hello ✅ World" is 14 columns
        editor.set_text("Hello ✅ World");
        let lines = editor.render(width);

        // All content lines (between borders) should fit within width
        for i in 1..lines.len() - 1 {
            let line_width = visible_width(&lines[i]);
            assert_eq!(
                line_width, width,
                "Line {i} has width {line_width}, expected {width}"
            );
        }
    }

    /// "wraps long text with emojis at correct positions"
    /// (editor.test.ts:762-775).
    #[test]
    fn wraps_long_text_with_emojis_at_correct_positions() {
        let mut editor = Editor::new(create_test_tui_sized(80, 24), EditorTheme::default());
        let width = 10;

        // Each ✅ is 2 columns. "✅✅✅✅✅" = 10 columns, fits exactly
        // "✅✅✅✅✅✅" = 12 columns, needs wrap
        editor.set_text("✅✅✅✅✅✅");
        let lines = editor.render(width);

        // Should have 2 content lines (plus 2 border lines)
        // First line: 5 emojis (10 cols), second line: 1 emoji (2 cols) +
        // padding
        for i in 1..lines.len() - 1 {
            let line_width = visible_width(&lines[i]);
            assert_eq!(
                line_width, width,
                "Line {i} has width {line_width}, expected {width}"
            );
        }
    }

    /// "renders isolated Thai and Lao AM clusters without width drift"
    /// (editor.test.ts:777-787).
    #[test]
    fn renders_isolated_thai_and_lao_am_clusters_without_width_drift() {
        for text in ["ำabc", "ຳabc"] {
            let mut editor = Editor::new(create_test_tui_sized(80, 24), EditorTheme::default());
            let width = 8;
            editor.set_text(text);

            for line in editor.render(width) {
                assert_eq!(
                    visible_width(&line),
                    width,
                    "line width drift for {text:?}: {line}"
                );
            }
        }
    }

    /// "wraps CJK characters correctly (each is 2 columns wide)"
    /// (editor.test.ts:789-808).
    #[test]
    fn wraps_cjk_characters_correctly() {
        let mut editor = Editor::new(create_test_tui_sized(80, 24), EditorTheme::default());
        let width = 10 + 1; // +1 col reserved for cursor

        // Each CJK char is 2 columns. "日本語テスト" = 6 chars = 12 columns
        editor.set_text("日本語テスト");
        let lines = editor.render(width);

        for i in 1..lines.len() - 1 {
            let line_width = visible_width(&lines[i]);
            assert_eq!(
                line_width, width,
                "Line {i} has width {line_width}, expected {width}"
            );
        }

        // Verify content split correctly
        let content_lines: Vec<String> = lines[1..lines.len() - 1]
            .iter()
            .map(|l| strip_terminal_sequences(l).trim().to_string())
            .collect();
        assert_eq!(content_lines.len(), 2);
        assert_eq!(content_lines[0], "日本語テス"); // 5 chars = 10 columns
        assert_eq!(content_lines[1], "ト"); // 1 char = 2 columns (+ padding)
    }

    /// "handles mixed ASCII and wide characters in wrapping"
    /// (editor.test.ts:810-823).
    #[test]
    fn handles_mixed_ascii_and_wide_characters_in_wrapping() {
        let mut editor = Editor::new(create_test_tui_sized(80, 24), EditorTheme::default());
        let width = 15 + 1; // +1 col reserved for cursor

        // "Test ✅ OK 日本" = 4 + 1 + 2 + 1 + 2 + 1 + 4 = 15 columns (fits
        // in width-1=15)
        editor.set_text("Test ✅ OK 日本");
        let lines = editor.render(width);

        // Should fit in one content line
        let content_lines = &lines[1..lines.len() - 1];
        assert_eq!(content_lines.len(), 1);

        let line_width = visible_width(&content_lines[0]);
        assert_eq!(line_width, width);
    }

    /// "renders cursor correctly on wide characters" (editor.test.ts:825-837).
    #[test]
    fn renders_cursor_correctly_on_wide_characters() {
        let mut editor = Editor::new(create_test_tui_sized(80, 24), EditorTheme::default());
        let width = 20;

        editor.set_text("A✅B");
        // Cursor should be at end (after B)
        let lines = editor.render(width);

        // The cursor (reverse video space) should be visible
        let content_line = &lines[1];
        assert!(
            content_line.contains(FAKE_CURSOR_START),
            "Should have fake cursor"
        );

        // Line should still be correct width
        assert_eq!(visible_width(content_line), width);
    }

    /// "does not exceed terminal width with emoji at wrap boundary"
    /// (editor.test.ts:839-851).
    #[test]
    fn does_not_exceed_terminal_width_with_emoji_at_wrap_boundary() {
        let mut editor = Editor::new(create_test_tui_sized(80, 24), EditorTheme::default());
        let width = 11;

        // "0123456789✅" = 10 ASCII + 2-wide emoji = 12 columns
        // Should wrap before the emoji since it would exceed width
        editor.set_text("0123456789✅");
        let lines = editor.render(width);

        for i in 1..lines.len() - 1 {
            let line_width = visible_width(&lines[i]);
            assert!(
                line_width <= width,
                "Line {i} has width {line_width}, exceeds max {width}"
            );
        }
    }

    /// "shows cursor at end of line before wrap, wraps on next char"
    /// (editor.test.ts:853-877).
    #[test]
    fn shows_cursor_at_end_of_line_before_wrap_wraps_on_next_char() {
        let width = 10;
        for padding_x in [0.0f64, 1.0] {
            let mut editor = Editor::with_options(
                create_test_tui_sized(width + padding_x as usize, 24),
                EditorTheme::default(),
                EditorOptions {
                    padding_x: Some(padding_x),
                    autocomplete_max_visible: None,
                },
            );

            // Type 9 chars → fills layoutWidth exactly, cursor at end on
            // same line
            for ch in "aaaaaaaaa".chars() {
                editor.handle_input(&ch.to_string());
            }
            let lines = editor.render(width + padding_x as usize);
            let content_lines = &lines[1..lines.len() - 1];
            assert_eq!(
                content_lines.len(),
                1,
                "Should be 1 content line before wrap"
            );
            assert!(
                content_lines[0].ends_with(&render_fake_cursor(" ")),
                "Cursor should be at end of line"
            );

            // Type 1 more → text wraps to second line
            editor.handle_input("a");
            let lines = editor.render(width + padding_x as usize);
            let content_lines = &lines[1..lines.len() - 1];
            assert_eq!(content_lines.len(), 2, "Should wrap to 2 content lines");
        }
    }

    // describe("Word wrapping")

    /// "wraps at word boundaries instead of mid-word" (editor.test.ts:880-899).
    #[test]
    fn wraps_at_word_boundaries_instead_of_mid_word() {
        let mut editor = Editor::new(create_test_tui_sized(80, 24), EditorTheme::default());
        let width = 40;

        editor.set_text("Hello world this is a test of word wrapping functionality");
        let lines = editor.render(width);

        // Get content lines (between borders)
        let content_lines: Vec<String> = lines[1..lines.len() - 1]
            .iter()
            .map(|l| strip_terminal_sequences(l).trim().to_string())
            .collect();

        // Should NOT break mid-word
        // Line 1 should end with a complete word
        assert!(
            !content_lines[0].ends_with('-'),
            "Line should not end with hyphen (mid-word break)"
        );

        // Each content line should be complete words
        for line in &content_lines {
            // Words at end of line should be complete (no partial words)
            let last_char = line.chars().last();
            let ok = match last_char {
                None => true,
                Some(c) => {
                    // JS /[\w.,!?;:]/
                    c.is_ascii_alphanumeric() || c == '_' || ".,!?;:".contains(c)
                }
            };
            assert!(ok, "Line ends unexpectedly with: {:?}", line.chars().last());
        }
    }

    /// "does not start lines with leading whitespace after word wrap"
    /// (editor.test.ts:901-919).
    #[test]
    fn does_not_start_lines_with_leading_whitespace_after_word_wrap() {
        let mut editor = Editor::new(create_test_tui_sized(80, 24), EditorTheme::default());
        let width = 20;

        editor.set_text("Word1 Word2 Word3 Word4 Word5 Word6");
        let lines = editor.render(width);

        // Get content lines (between borders)
        let content_lines = &lines[1..lines.len() - 1];

        // No line should start with whitespace (except for padding at the
        // end)
        for (i, raw) in content_lines.iter().enumerate() {
            let line = strip_terminal_sequences(raw);
            let trimmed_start = line.trim_start();
            // The line should either be all padding or start with a word
            // character
            if !trimmed_start.is_empty() {
                // JS /^\s+\S/.test(line.trimEnd())
                let trimmed_end = line.trim_end();
                let starts_with_whitespace = trimmed_end
                    .chars()
                    .next()
                    .is_some_and(crate::utils::is_js_whitespace);
                let rest = trimmed_end.trim_start();
                assert!(
                    !starts_with_whitespace || rest.is_empty(),
                    "Line {i} starts with unexpected whitespace before content"
                );
            }
        }
    }

    /// "breaks long words (URLs) at character level" (editor.test.ts:921-933).
    #[test]
    fn breaks_long_words_urls_at_character_level() {
        let mut editor = Editor::new(create_test_tui_sized(80, 24), EditorTheme::default());
        let width = 30;

        editor.set_text("Check https://example.com/very/long/path/that/exceeds/width here");
        let lines = editor.render(width);

        // All lines should fit within width
        for i in 1..lines.len() - 1 {
            let line_width = visible_width(&lines[i]);
            assert_eq!(
                line_width, width,
                "Line {i} has width {line_width}, expected {width}"
            );
        }
    }

    /// "preserves multiple spaces within words on same line"
    /// (editor.test.ts:935-946).
    #[test]
    fn preserves_multiple_spaces_within_words_on_same_line() {
        let mut editor = Editor::new(create_test_tui_sized(80, 24), EditorTheme::default());
        let width = 50;

        editor.set_text("Word1   Word2    Word3");
        let lines = editor.render(width);

        let content_line = strip_terminal_sequences(&lines[1]).trim().to_string();
        // Multiple spaces should be preserved
        assert!(
            content_line.contains("Word1   Word2"),
            "Multiple spaces should be preserved"
        );
    }
}
