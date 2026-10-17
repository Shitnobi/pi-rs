//! Port of `packages/tui/src/components/select-list.ts` — the `SelectList`
//! component: a filterable list with scrolling, selection highlight,
//! two-column (value + description) rendering, and keybinding-driven
//! navigation (`tui.select.*`).
//!
//! Divergences from TS:
//! - `onSelect` / `onCancel` / `onSelectionChange` optional callbacks are
//!   [`Option<Box<dyn Fn>>`] fields (assignable after construction).
//! - The theme is a struct of `Box<dyn Fn(&str) -> String>` style hooks.
//! - `selectedIndex` is `i64` (JS number semantics): navigating up in an
//!   empty filtered list produces `-1`, exactly like
//!   `this.filteredItems.length - 1` in JS.
//! - `truncatePrimary` receives an owned
//!   [`SelectListTruncatePrimaryContext`] (the TS passes an object literal
//!   by reference; clones are the honest Rust translation).

use crate::component::{
    Component, ComponentMouseResult, ComponentRef, TuiMouseButton, TuiMouseEvent,
    TuiMouseEventResult, TuiMouseEventType,
};
use crate::keybindings::get_keybindings;
use crate::utils::{truncate_to_width, visible_width};

/// `DEFAULT_PRIMARY_COLUMN_WIDTH` (select-list.ts:1).
const DEFAULT_PRIMARY_COLUMN_WIDTH: usize = 32;
/// `PRIMARY_COLUMN_GAP` (select-list.ts:2).
const PRIMARY_COLUMN_GAP: usize = 2;
/// `MIN_DESCRIPTION_WIDTH` (select-list.ts:3).
const MIN_DESCRIPTION_WIDTH: usize = 10;

/// `normalizeToSingleLine` (select-list.ts:5) — collapse `[\r\n]+` runs to
/// single spaces, then JS-trim.
fn normalize_to_single_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_newline_run = false;
    for c in text.chars() {
        if c == '\r' || c == '\n' {
            if !in_newline_run {
                out.push(' ');
            }
            in_newline_run = true;
        } else {
            out.push(c);
            in_newline_run = false;
        }
    }
    crate::utils::js_trim(&out).to_string()
}

/// `clamp` (select-list.ts:4) — `Math.max(min, Math.min(value, max))`.
fn clamp(value: usize, min: usize, max: usize) -> usize {
    value.min(max).max(min)
}

/// `SelectItem` (select-list.ts:7-11).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SelectItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

impl SelectItem {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        SelectItem {
            value: value.into(),
            label: label.into(),
            description: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// `SelectListTheme` (select-list.ts:13-19) — style hooks. Note that
/// upstream `selectedPrefix` is part of the theme interface but the render
/// path hardcodes the `"→ "` prefix; kept for interface parity.
pub struct SelectListTheme {
    pub selected_prefix: Box<dyn Fn(&str) -> String>,
    pub selected_text: Box<dyn Fn(&str) -> String>,
    pub description: Box<dyn Fn(&str) -> String>,
    pub scroll_info: Box<dyn Fn(&str) -> String>,
    pub no_match: Box<dyn Fn(&str) -> String>,
}

impl Default for SelectListTheme {
    fn default() -> Self {
        SelectListTheme {
            selected_prefix: Box::new(|t| t.to_string()),
            selected_text: Box::new(|t| t.to_string()),
            description: Box::new(|t| t.to_string()),
            scroll_info: Box::new(|t| t.to_string()),
            no_match: Box::new(|t| t.to_string()),
        }
    }
}

/// `SelectListTruncatePrimaryContext` (select-list.ts:21-27). Owned copies
/// of the TS object-literal fields.
pub struct SelectListTruncatePrimaryContext {
    pub text: String,
    pub max_width: usize,
    pub column_width: usize,
    pub item: SelectItem,
    pub is_selected: bool,
}

/// `SelectListLayoutOptions` (select-list.ts:29-33).
#[derive(Default)]
pub struct SelectListLayoutOptions {
    pub min_primary_column_width: Option<usize>,
    pub max_primary_column_width: Option<usize>,
    /// `truncatePrimary?: (context) => string`
    pub truncate_primary: Option<Box<dyn Fn(&SelectListTruncatePrimaryContext) -> String>>,
}

/// `SelectList` (select-list.ts:35-280).
pub struct SelectList {
    items: Vec<SelectItem>,
    filtered_items: Vec<SelectItem>,
    /// JS number semantics: can become `-1` when navigating an empty list.
    selected_index: i64,
    mouse_pressed_index: Option<i64>,
    max_visible: usize,
    theme: SelectListTheme,
    layout: SelectListLayoutOptions,
    /// `onSelect?: (item: SelectItem) => void` (select-list.ts:42).
    pub on_select: Option<Box<dyn Fn(&SelectItem)>>,
    /// `onCancel?: () => void` (select-list.ts:43).
    pub on_cancel: Option<Box<dyn Fn()>>,
    /// `onSelectionChange?: (item: SelectItem) => void` (select-list.ts:44).
    pub on_selection_change: Option<Box<dyn Fn(&SelectItem)>>,
}

impl SelectList {
    /// `constructor(items, maxVisible, theme, layout = {})` with the default
    /// empty layout (select-list.ts:47-54).
    pub fn new(items: Vec<SelectItem>, max_visible: usize, theme: SelectListTheme) -> Self {
        SelectList::with_layout(items, max_visible, theme, SelectListLayoutOptions::default())
    }

    /// `constructor` with explicit layout options (select-list.ts:47-54).
    pub fn with_layout(
        items: Vec<SelectItem>,
        max_visible: usize,
        theme: SelectListTheme,
        layout: SelectListLayoutOptions,
    ) -> Self {
        let filtered_items = items.clone();
        SelectList {
            items,
            filtered_items,
            selected_index: 0,
            mouse_pressed_index: None,
            max_visible,
            theme,
            layout,
            on_select: None,
            on_cancel: None,
            on_selection_change: None,
        }
    }

    /// `setFilter(filter)` (select-list.ts:56-60) — prefix match on
    /// `item.value`, case-insensitive; resets selection.
    pub fn set_filter(&mut self, filter: &str) {
        let filter_lower = filter.to_lowercase();
        self.filtered_items = self
            .items
            .iter()
            .filter(|item| item.value.to_lowercase().starts_with(&filter_lower))
            .cloned()
            .collect();
        // Reset selection when filter changes
        self.selected_index = 0;
    }

    /// `setSelectedIndex(index)` (select-list.ts:62-64) — clamped to the
    /// filtered list.
    pub fn set_selected_index(&mut self, index: i64) {
        self.selected_index = 0.max(index.min(self.filtered_items.len() as i64 - 1));
    }

    /// `getSelectedItem(): SelectItem | null` (select-list.ts:277-280).
    pub fn get_selected_item(&self) -> Option<&SelectItem> {
        self.item_at(self.selected_index)
    }

    /// `getVisibleRange()` (select-list.ts:167-176) — the window is centered
    /// on the selection. Returns `(startIndex, endIndex)` as JS numbers.
    fn visible_range(&self) -> (i64, i64) {
        let len = self.filtered_items.len() as i64;
        let start_index = 0.max(
            (self.selected_index - (self.max_visible as i64 / 2)).min(len - self.max_visible as i64),
        );
        let end_index = (start_index + self.max_visible as i64).min(len);
        (start_index, end_index)
    }

    /// `renderItem` (select-list.ts:178-222).
    fn render_item(
        &self,
        item: &SelectItem,
        is_selected: bool,
        width: usize,
        description_single_line: Option<&str>,
        primary_column_width: usize,
    ) -> String {
        let prefix = if is_selected { "→ " } else { "  " };
        let prefix_width = visible_width(prefix);

        if let Some(description) = description_single_line {
            if width > 40 {
                // Math.min(columnWidth, width - prefixWidth - 4), floored at 1
                let effective_primary_column_width =
                    1.max(primary_column_width.min(width.saturating_sub(prefix_width + 4)));
                let max_primary_width = 1.max(effective_primary_column_width.saturating_sub(PRIMARY_COLUMN_GAP));
                let truncated_value =
                    self.truncate_primary_value(item, is_selected, max_primary_width, effective_primary_column_width);
                let truncated_value_width = visible_width(&truncated_value);
                let spacing = " ".repeat(1.max(effective_primary_column_width.saturating_sub(truncated_value_width)));
                let description_start = prefix_width + truncated_value_width + spacing.len();
                // -2 for safety; may go negative in JS, so compute signed
                let remaining_width = width as isize - description_start as isize - 2;

                if remaining_width > MIN_DESCRIPTION_WIDTH as isize {
                    let truncated_desc =
                        truncate_to_width(description, remaining_width as usize, "", false);
                    if is_selected {
                        let combined = format!("{prefix}{truncated_value}{spacing}{truncated_desc}");
                        return (self.theme.selected_text)(&combined);
                    }

                    let desc_text = (self.theme.description)(&format!("{spacing}{truncated_desc}"));
                    return format!("{prefix}{truncated_value}{desc_text}");
                }
            }
        }

        let max_width = width.saturating_sub(prefix_width + 2);
        let truncated_value = self.truncate_primary_value(item, is_selected, max_width, max_width);
        if is_selected {
            let combined = format!("{prefix}{truncated_value}");
            return (self.theme.selected_text)(&combined);
        }

        format!("{prefix}{truncated_value}")
    }

    /// `getPrimaryColumnWidth()` (select-list.ts:224-233).
    fn get_primary_column_width(&self) -> usize {
        let (min, max) = self.primary_column_bounds();
        let widest_primary = self
            .filtered_items
            .iter()
            .map(|item| visible_width(&self.display_value(item)) + PRIMARY_COLUMN_GAP)
            .max()
            .unwrap_or(0);
        clamp(widest_primary, min, max)
    }

    /// `getPrimaryColumnBounds()` (select-list.ts:235-248).
    fn primary_column_bounds(&self) -> (usize, usize) {
        let raw_min = self
            .layout
            .min_primary_column_width
            .or(self.layout.max_primary_column_width)
            .unwrap_or(DEFAULT_PRIMARY_COLUMN_WIDTH);
        let raw_max = self
            .layout
            .max_primary_column_width
            .or(self.layout.min_primary_column_width)
            .unwrap_or(DEFAULT_PRIMARY_COLUMN_WIDTH);

        (
            1.max(raw_min.min(raw_max)),
            1.max(raw_min.max(raw_max)),
        )
    }

    /// `truncatePrimary` (select-list.ts:250-264) — custom hook with a final
    /// safety truncation.
    fn truncate_primary_value(
        &self,
        item: &SelectItem,
        is_selected: bool,
        max_width: usize,
        column_width: usize,
    ) -> String {
        let display_value = self.display_value(item);
        let truncated_value = match &self.layout.truncate_primary {
            Some(hook) => hook(&SelectListTruncatePrimaryContext {
                text: display_value.clone(),
                max_width,
                column_width,
                item: item.clone(),
                is_selected,
            }),
            None => truncate_to_width(&display_value, max_width, "", false),
        };
        truncate_to_width(&truncated_value, max_width, "", false)
    }

    /// `getDisplayValue` (select-list.ts:266-268).
    fn display_value(&self, item: &SelectItem) -> String {
        if item.label.is_empty() {
            item.value.clone()
        } else {
            item.label.clone()
        }
    }

    /// `notifySelectionChange` (select-list.ts:270-275).
    fn notify_selection_change(&mut self) {
        if let (Some(item), Some(callback)) = (
            self.item_at(self.selected_index),
            self.on_selection_change.as_ref(),
        ) {
            callback(item);
        }
    }

    /// JS `this.filteredItems[index]` — `undefined` for out-of-range or
    /// negative indices.
    fn item_at(&self, index: i64) -> Option<&SelectItem> {
        usize::try_from(index).ok().and_then(|i| self.filtered_items.get(i))
    }
}

impl Component for SelectList {
    /// `render(width)` (select-list.ts:70-100).
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();

        // If no items match filter, show message
        if self.filtered_items.is_empty() {
            lines.push((self.theme.no_match)("  No matching commands"));
            return lines;
        }

        let primary_column_width = self.get_primary_column_width();

        // Calculate visible range with scrolling
        let (start_index, end_index) = self.visible_range();

        // Render visible items
        for i in start_index..end_index {
            let item = match self.item_at(i) {
                Some(item) => item,
                None => continue,
            };

            let is_selected = i == self.selected_index;
            // TS `item.description ? ... : undefined` (select-list.ts:95) —
            // JS truthiness: an empty-string description is falsy.
            let description_single_line = item
                .description
                .as_ref()
                .filter(|description| !description.is_empty())
                .map(|description| normalize_to_single_line(description));
            lines.push(self.render_item(
                item,
                is_selected,
                width,
                description_single_line.as_deref(),
                primary_column_width,
            ));
        }

        // Add scroll indicators if needed
        if start_index > 0 || (end_index as usize) < self.filtered_items.len() {
            let scroll_text = format!("  ({}/{})", self.selected_index + 1, self.filtered_items.len());
            // Truncate if too long for terminal
            lines.push((self.theme.scroll_info)(&truncate_to_width(
                &scroll_text,
                width.saturating_sub(2),
                "",
                false,
            )));
        }

        lines
    }

    /// `handleMouse(event)` (select-list.ts:102-138).
    fn handle_mouse(
        &mut self,
        _self_ref: &ComponentRef,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        if self.filtered_items.is_empty() {
            return None;
        }
        if event.event_type == TuiMouseEventType::Wheel {
            // JS truthiness: `event.wheelDelta` must be present and nonzero
            if let Some(raw_delta) = event.wheel_delta {
                if raw_delta != 0 {
                    let delta: i64 = if raw_delta < 0 { -1 } else { 1 };
                    let previous_index = self.selected_index;
                    let len = self.filtered_items.len() as i64;
                    self.selected_index = 0.max((len - 1).min(self.selected_index + delta));
                    if self.selected_index != previous_index {
                        self.notify_selection_change();
                    }
                    let changed = self.selected_index != previous_index;
                    return Some(ComponentMouseResult::Result(TuiMouseEventResult {
                        handled: Some(true),
                        render: Some(changed),
                        ..Default::default()
                    }));
                }
            }
        }
        // Hover must not change selection: the visible range is centered on it.
        if event.button != TuiMouseButton::Left {
            return None;
        }
        if event.event_type != TuiMouseEventType::Press && event.event_type != TuiMouseEventType::Click {
            return None;
        }
        let (start_index, end_index) = self.visible_range();
        let item_index = start_index + event.y;
        if item_index < start_index || item_index >= end_index {
            return None;
        }

        if event.event_type == TuiMouseEventType::Press {
            self.mouse_pressed_index = Some(item_index);
            if self.selected_index != item_index {
                self.selected_index = item_index;
                self.notify_selection_change();
            }
            return Some(ComponentMouseResult::Result(TuiMouseEventResult {
                handled: Some(true),
                focus: Some(true),
                ..Default::default()
            }));
        }
        // Click
        let clicked_index = self.mouse_pressed_index.unwrap_or(item_index);
        self.mouse_pressed_index = None;
        let changed = self.selected_index != clicked_index;
        self.selected_index = clicked_index;
        if changed {
            self.notify_selection_change();
        }
        if let (Some(item), Some(callback)) =
            (self.item_at(self.selected_index), self.on_select.as_ref())
        {
            callback(item);
        }
        Some(ComponentMouseResult::Result(TuiMouseEventResult {
            handled: Some(true),
            ..Default::default()
        }))
    }

    /// `handleInput(keyData)` (select-list.ts:140-165).
    fn handle_input(&mut self, data: &str) {
        let kb = get_keybindings();
        let len = self.filtered_items.len() as i64;
        // Up arrow - wrap to bottom when at top
        if kb.matches(data, "tui.select.up") {
            self.selected_index = if self.selected_index == 0 {
                len - 1
            } else {
                self.selected_index - 1
            };
            self.notify_selection_change();
        }
        // Down arrow - wrap to top when at bottom
        else if kb.matches(data, "tui.select.down") {
            self.selected_index = if self.selected_index == len - 1 {
                0
            } else {
                self.selected_index + 1
            };
            self.notify_selection_change();
        }
        // Enter
        else if kb.matches(data, "tui.select.confirm") {
            if let (Some(item), Some(callback)) =
                (self.item_at(self.selected_index), self.on_select.as_ref())
            {
                callback(item);
            }
        }
        // Escape or Ctrl+C
        else if kb.matches(data, "tui.select.cancel") {
            if let Some(callback) = self.on_cancel.as_ref() {
                callback();
            }
        }
    }

    fn handles_input(&self) -> bool {
        true
    }

    /// `invalidate()` (select-list.ts:66-68) — no cached state.
    fn invalidate(&mut self) {}
}

// =============================================================================
// Tests — port of test/select-list.test.ts
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme() -> SelectListTheme {
        SelectListTheme::default()
    }

    /// `visibleIndexOf` helper (select-list.test.ts:12-16).
    fn visible_index_of(line: &str, text: &str) -> usize {
        let index = line.find(text).expect("text expected in line");
        visible_width(&line[..index])
    }

    /// JS `String.prototype.indexOf` — UTF-16 code-unit index in the raw
    /// string (ANSI escapes included).
    fn utf16_index_of(line: &str, text: &str) -> usize {
        let index = line.find(text).expect("text expected in line");
        line[..index].encode_utf16().count()
    }

    /// "normalizes multiline descriptions to single line"
    /// (select-list.test.ts:19-33).
    #[test]
    fn normalizes_multiline_descriptions_to_single_line() {
        let items = vec![SelectItem::new("test", "test")
            .with_description("Line one\nLine two\nLine three")];

        let mut list = SelectList::new(items, 5, test_theme());
        let rendered = list.render(100);

        assert!(!rendered.is_empty());
        assert!(!rendered[0].contains('\n'));
        assert!(rendered[0].contains("Line one Line two Line three"));
    }

    /// "keeps descriptions aligned when the primary text is truncated"
    /// (select-list.test.ts:35-54).
    #[test]
    fn keeps_descriptions_aligned_when_the_primary_text_is_truncated() {
        let items = vec![
            SelectItem::new("short", "short").with_description("short description"),
            SelectItem::new(
                "very-long-command-name-that-needs-truncation",
                "very-long-command-name-that-needs-truncation",
            )
            .with_description("long description"),
        ];

        let mut list = SelectList::new(items, 5, test_theme());
        let rendered = list.render(80);

        assert_eq!(
            visible_index_of(&rendered[0], "short description"),
            visible_index_of(&rendered[1], "long description")
        );
    }

    /// "uses the configured minimum primary column width"
    /// (select-list.test.ts:56-72).
    #[test]
    fn uses_the_configured_minimum_primary_column_width() {
        let items = vec![
            SelectItem::new("a", "a").with_description("first"),
            SelectItem::new("bb", "bb").with_description("second"),
        ];

        let mut list = SelectList::with_layout(
            items,
            5,
            test_theme(),
            SelectListLayoutOptions {
                min_primary_column_width: Some(12),
                max_primary_column_width: Some(20),
                ..Default::default()
            },
        );
        let rendered = list.render(80);

        assert_eq!(utf16_index_of(&rendered[0], "first"), 14);
        assert_eq!(utf16_index_of(&rendered[1], "second"), 14);
    }

    /// "uses the configured maximum primary column width"
    /// (select-list.test.ts:74-93).
    #[test]
    fn uses_the_configured_maximum_primary_column_width() {
        let items = vec![
            SelectItem::new(
                "very-long-command-name-that-needs-truncation",
                "very-long-command-name-that-needs-truncation",
            )
            .with_description("first"),
            SelectItem::new("short", "short").with_description("second"),
        ];

        let mut list = SelectList::with_layout(
            items,
            5,
            test_theme(),
            SelectListLayoutOptions {
                min_primary_column_width: Some(12),
                max_primary_column_width: Some(20),
                ..Default::default()
            },
        );
        let rendered = list.render(80);

        assert_eq!(visible_index_of(&rendered[0], "first"), 22);
        assert_eq!(visible_index_of(&rendered[1], "second"), 22);
    }

    /// "allows overriding primary truncation while preserving description
    /// alignment" (select-list.test.ts:95-124).
    #[test]
    fn allows_overriding_primary_truncation_while_preserving_description_alignment() {
        let items = vec![
            SelectItem::new(
                "very-long-command-name-that-needs-truncation",
                "very-long-command-name-that-needs-truncation",
            )
            .with_description("first"),
            SelectItem::new("short", "short").with_description("second"),
        ];

        let mut list = SelectList::with_layout(
            items,
            5,
            test_theme(),
            SelectListLayoutOptions {
                min_primary_column_width: Some(12),
                max_primary_column_width: Some(12),
                truncate_primary: Some(Box::new(|context: &SelectListTruncatePrimaryContext| {
                    let text = &context.text;
                    // JS `text.length` (UTF-16 code units) and `text.slice`
                    if crate::utils::utf16_len(text) <= context.max_width {
                        return text.clone();
                    }
                    let units: Vec<u16> = text.encode_utf16().collect();
                    let end = context.max_width.saturating_sub(1).min(units.len());
                    let mut truncated = String::from_utf16_lossy(&units[..end]);
                    truncated.push('…');
                    truncated
                })),
                ..Default::default()
            },
        );
        let rendered = list.render(80);

        assert!(rendered[0].contains("…"));
        assert_eq!(
            visible_index_of(&rendered[0], "first"),
            visible_index_of(&rendered[1], "second")
        );
    }

    /// Empty-string description is JS-falsy (select-list.ts:95
    /// `item.description ?`) — the plain render branch is used, no
    /// description column. T3 redteam lists-F1 regression test.
    #[test]
    fn empty_description_is_falsy_and_renders_the_plain_branch() {
        let items = vec![SelectItem::new("x".repeat(40), "x".repeat(40))
            .with_description("")];

        let mut list = SelectList::new(items, 5, test_theme());
        let rendered = list.render(80);

        assert_eq!(rendered.len(), 1);
        // Plain branch: marker + label truncated to width - 4, no ANSI
        // reset/description padding.
        assert_eq!(rendered[0], format!("→ {}", "x".repeat(40)));
        assert!(!rendered[0].contains("\x1b"));
    }
}

