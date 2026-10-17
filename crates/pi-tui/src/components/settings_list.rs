//! Port of `packages/tui/src/components/settings-list.ts` — the
//! `SettingsList` component: a settings menu with label/value rows, value
//! cycling, submenus, an optional fuzzy search `Input`, and keybinding
//! navigation (`tui.select.*`).
//!
//! Divergences from TS:
//! - `onChange` / `onCancel` (required constructor callbacks) are
//!   `Box<dyn Fn>` values.
//! - The theme is a struct of `Box<dyn Fn>` style hooks (`cursor` stays a
//!   plain `String`, as in TS).
//! - `submenu?: (currentValue, done) => Component` is
//!   [`Option<SubmenuFactory>`] on [`SettingItem`]. The TS `done` callback
//!   closes over `this` (the list); in Rust the factory receives a
//!   [`SubmenuDone`] handle that buffers calls, and the list applies them
//!   right after the delegated input/mouse event returns — observably
//!   equivalent, since `done` is only ever invoked synchronously from the
//!   submenu component's own event handlers. The TS closure also captures
//!   the originating item; the Rust port records its id
//!   (`submenu_item_id`) instead.
//! - `filteredItems` holds indices into `items` (TS shares object
//!   references); `fuzzyFilter` order (best-first) is preserved.
//! - The search `Input` is hosted as a [`ComponentRef`]
//!   (`Rc<RefCell<Input>>`).

use std::cell::RefCell;
use std::rc::Rc;

use crate::component::{
    Component, ComponentMouseResult, ComponentRef, TuiMouseButton, TuiMouseEvent,
    TuiMouseEventResult, TuiMouseEventType,
};
use crate::components::input::Input;
use crate::fuzzy::fuzzy_filter;
use crate::keybindings::get_keybindings;
use crate::utils::{truncate_to_width, truncate_to_width_default, visible_width, wrap_text_with_ansi};

/// A single buffered `done(selectedValue?, options?)` invocation.
#[derive(Debug, Clone)]
struct SubmenuDoneCall {
    selected_value: Option<String>,
    navigate_to: Option<String>,
}

/// The `done` callback handed to a submenu factory
/// (settings-list.ts:18-21). The submenu component's own callbacks invoke
/// this handle; the owning [`SettingsList`] applies the buffered calls
/// after the delegated event returns (synchronously from the caller's
/// perspective, matching the TS).
#[derive(Clone, Default)]
pub struct SubmenuDone {
    pending: Rc<RefCell<Vec<SubmenuDoneCall>>>,
}

impl SubmenuDone {
    /// `done(selectedValue?, options?: { navigateTo? })`.
    pub fn call(&self, selected_value: Option<&str>, navigate_to: Option<&str>) {
        self.pending.borrow_mut().push(SubmenuDoneCall {
            selected_value: selected_value.map(|value| value.to_string()),
            navigate_to: navigate_to.map(|value| value.to_string()),
        });
    }

    fn take_pending(&self) -> Vec<SubmenuDoneCall> {
        std::mem::take(&mut *self.pending.borrow_mut())
    }
}

/// `submenu?: (currentValue: string, done) => Component`
/// (settings-list.ts:16-21).
pub type SubmenuFactory = Box<dyn Fn(&str, &SubmenuDone) -> ComponentRef>;

/// `SettingItem` (settings-list.ts:5-21).
pub struct SettingItem {
    /// Unique identifier for this setting.
    pub id: String,
    /// Display label (left side).
    pub label: String,
    /// Optional description shown when selected.
    pub description: Option<String>,
    /// Current value to display (right side).
    pub current_value: String,
    /// If provided, Enter/Space cycles through these values.
    pub values: Option<Vec<String>>,
    /// If provided, Enter opens this submenu. Receives the current value
    /// and a `done` callback (which accepts an optional selectedValue and
    /// an optional `navigateTo` id).
    pub submenu: Option<SubmenuFactory>,
}

impl SettingItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>, current_value: impl Into<String>) -> Self {
        SettingItem {
            id: id.into(),
            label: label.into(),
            description: None,
            current_value: current_value.into(),
            values: None,
            submenu: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_values(mut self, values: Vec<String>) -> Self {
        self.values = Some(values);
        self
    }

    pub fn with_submenu(mut self, submenu: SubmenuFactory) -> Self {
        self.submenu = Some(submenu);
        self
    }
}

/// `SettingsListTheme` (settings-list.ts:23-29).
pub struct SettingsListTheme {
    pub label: Box<dyn Fn(&str, bool) -> String>,
    pub value: Box<dyn Fn(&str, bool) -> String>,
    pub description: Box<dyn Fn(&str) -> String>,
    pub cursor: String,
    pub hint: Box<dyn Fn(&str) -> String>,
}

impl Default for SettingsListTheme {
    fn default() -> Self {
        SettingsListTheme {
            label: Box::new(|text, _| text.to_string()),
            value: Box::new(|text, _| text.to_string()),
            description: Box::new(|text| text.to_string()),
            cursor: "> ".to_string(),
            hint: Box::new(|text| text.to_string()),
        }
    }
}

/// `SettingsListOptions` (settings-list.ts:31-33).
#[derive(Default)]
pub struct SettingsListOptions {
    pub enable_search: Option<bool>,
}

/// `SettingsList` (settings-list.ts:35-311).
pub struct SettingsList {
    items: Vec<SettingItem>,
    /// Indices into `items`, in fuzzy-filter (best-first) order.
    filtered_items: Vec<usize>,
    theme: SettingsListTheme,
    selected_index: usize,
    mouse_pressed_index: Option<usize>,
    max_visible: usize,
    on_change: Box<dyn Fn(&str, &str)>,
    on_cancel: Box<dyn Fn()>,
    search_input: Option<Rc<RefCell<Input>>>,
    search_enabled: bool,

    // Submenu state
    submenu_component: Option<ComponentRef>,
    submenu_done: Option<SubmenuDone>,
    submenu_item_index: Option<usize>,
    /// The TS `done` closure captures the originating item; we record its
    /// id so buffered calls can still address it.
    submenu_item_id: Option<String>,
    navigate_after_close: Option<String>,
}

impl SettingsList {
    /// `constructor` with default options (settings-list.ts:53-70).
    pub fn new(
        items: Vec<SettingItem>,
        max_visible: usize,
        theme: SettingsListTheme,
        on_change: Box<dyn Fn(&str, &str)>,
        on_cancel: Box<dyn Fn()>,
    ) -> Self {
        SettingsList::with_options(items, max_visible, theme, on_change, on_cancel, SettingsListOptions::default())
    }

    /// `constructor` with explicit options (settings-list.ts:53-70).
    pub fn with_options(
        items: Vec<SettingItem>,
        max_visible: usize,
        theme: SettingsListTheme,
        on_change: Box<dyn Fn(&str, &str)>,
        on_cancel: Box<dyn Fn()>,
        options: SettingsListOptions,
    ) -> Self {
        let filtered_items = (0..items.len()).collect();
        let search_enabled = options.enable_search.unwrap_or(false);
        let search_input = if search_enabled {
            Some(Rc::new(RefCell::new(Input::new())))
        } else {
            None
        };
        SettingsList {
            items,
            filtered_items,
            theme,
            selected_index: 0,
            mouse_pressed_index: None,
            max_visible,
            on_change,
            on_cancel,
            search_input,
            search_enabled,
            submenu_component: None,
            submenu_done: None,
            submenu_item_index: None,
            submenu_item_id: None,
            navigate_after_close: None,
        }
    }

    /// `updateValue(id, newValue)` (settings-list.ts:72-77).
    pub fn update_value(&mut self, id: &str, new_value: &str) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.current_value = new_value.to_string();
        }
    }

    /// `selectItem(id)` (settings-list.ts:79-85) — move selection to the
    /// item with the given id (no-op if not found). `selectedIndex` is a
    /// position in the filtered list (TS `findIndex` over `filteredItems`),
    /// NOT an item index.
    pub fn select_item(&mut self, id: &str) {
        let indices: Vec<usize> = if self.search_enabled {
            self.filtered_items.clone()
        } else {
            (0..self.items.len()).collect()
        };
        if let Some(position) = indices
            .iter()
            .position(|&index| self.items[index].id == id)
        {
            self.selected_index = position;
        }
    }

    /// `renderMainList` (settings-list.ts:100-161).
    fn render_main_list(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();

        if self.search_enabled {
            if let Some(search_input) = self.search_input.clone() {
                lines.extend(search_input.borrow_mut().render(width));
                lines.push(String::new());
            }
        }

        if self.items.is_empty() {
            lines.push((self.theme.hint)("  No settings available"));
            if self.search_enabled {
                self.add_hint_line(&mut lines, width);
            }
            return lines;
        }

        let display_items = self.display_indices();
        if display_items.is_empty() {
            lines.push(truncate_to_width_default(&(self.theme.hint)("  No matching settings"), width));
            self.add_hint_line(&mut lines, width);
            return lines;
        }

        // Calculate visible range with scrolling
        let (start_index, end_index) = self.visible_range(display_items.len());

        // Calculate max label width for alignment
        let max_label_width = self
            .items
            .iter()
            .map(|item| visible_width(&item.label))
            .max()
            .unwrap_or(0)
            .min(36);

        // Render visible items
        for i in start_index..end_index {
            let index = match display_items.get(i) {
                Some(&index) => index,
                None => continue,
            };
            let item = &self.items[index];

            let is_selected = i == self.selected_index;
            let prefix = if is_selected {
                self.theme.cursor.clone()
            } else {
                "  ".to_string()
            };
            let prefix_width = visible_width(&prefix);

            // Pad label to align values
            let label_padded = format!(
                "{}{}",
                item.label,
                " ".repeat(max_label_width.saturating_sub(visible_width(&item.label)))
            );
            let label_text = (self.theme.label)(&label_padded, is_selected);

            // Calculate space for value
            let separator = "  ";
            let used_width = prefix_width + max_label_width + visible_width(separator);
            let value_max_width = width.saturating_sub(used_width + 2);

            let value_text = (self.theme.value)(
                &truncate_to_width(&item.current_value, value_max_width, "", false),
                is_selected,
            );

            lines.push(truncate_to_width_default(
                &format!("{prefix}{label_text}{separator}{value_text}"),
                width,
            ));
        }

        // Add scroll indicator if needed
        if start_index > 0 || end_index < display_items.len() {
            let scroll_text = format!("  ({}/{})", self.selected_index + 1, display_items.len());
            lines.push((self.theme.hint)(&truncate_to_width(
                &scroll_text,
                width.saturating_sub(2),
                "",
                false,
            )));
        }

        // Add description for selected item (TS `if (selectedItem?.description)`
        // — settings-list.ts:165 — JS truthiness: "" is falsy).
        if let Some(&index) = display_items.get(self.selected_index) {
            if let Some(description) = self
                .items[index]
                .description
                .clone()
                .filter(|description| !description.is_empty())
            {
                lines.push(String::new());
                let wrapped_desc = wrap_text_with_ansi(&description, width.saturating_sub(4));
                for line in wrapped_desc {
                    lines.push((self.theme.description)(&format!("  {line}")));
                }
            }
        }

        // Add hint
        self.add_hint_line(&mut lines, width);

        lines
    }

    /// `getDisplayItems()` (settings-list.ts:234-236) — indices into
    /// `items` for the currently displayed list.
    fn display_indices(&self) -> Vec<usize> {
        if self.search_enabled {
            self.filtered_items.clone()
        } else {
            (0..self.items.len()).collect()
        }
    }

    /// `getVisibleRange(displayItems)` (settings-list.ts:238-245).
    fn visible_range(&self, display_len: usize) -> (usize, usize) {
        let len = display_len as i64;
        let start_index = 0.max((self.selected_index as i64 - self.max_visible as i64 / 2)
            .min(len - self.max_visible as i64));
        let start_index = start_index.max(0) as usize;
        let end_index = (start_index + self.max_visible).min(display_len);
        (start_index, end_index)
    }

    /// `activateItem()` (settings-list.ts:247-274).
    fn activate_item(&mut self) {
        let display_items = self.display_indices();
        let Some(&item_index) = display_items.get(self.selected_index) else {
            return;
        };

        if self.items[item_index].submenu.is_some() {
            // Open submenu, passing current value so it can pre-select correctly
            self.submenu_item_index = Some(self.selected_index);
            self.submenu_item_id = Some(self.items[item_index].id.clone());
            let current_value = self.items[item_index].current_value.clone();
            let done = SubmenuDone::default();
            let factory = self.items[item_index].submenu.as_ref().expect("checked above");
            let component = factory(&current_value, &done);
            self.submenu_done = Some(done);
            self.submenu_component = Some(component);
        } else if let Some(values) = self.items[item_index].values.clone() {
            if !values.is_empty() {
                // Cycle through values. `indexOf` returns -1 when the
                // current value is absent, making nextIndex 0.
                let current_index = values
                    .iter()
                    .position(|value| *value == self.items[item_index].current_value);
                let next_index = match current_index {
                    Some(index) => (index + 1) % values.len(),
                    None => 0,
                };
                let new_value = values[next_index].clone();
                self.items[item_index].current_value = new_value.clone();
                let id = self.items[item_index].id.clone();
                (self.on_change)(&id, &new_value);
            }
        }
    }

    /// `closeSubmenu()` (settings-list.ts:276-291).
    fn close_submenu(&mut self) {
        self.submenu_component = None;
        self.submenu_done = None;
        if let Some(id) = self.navigate_after_close.take() {
            self.submenu_item_index = None;
            self.select_item(&id);
            // Open the target item's submenu automatically
            self.activate_item();
        } else if let Some(index) = self.submenu_item_index.take() {
            // Restore selection to the item that opened the submenu
            self.selected_index = index;
        }
    }

    /// Applies buffered `done` calls from the active submenu
    /// (see the module docs for the divergence from the TS closure).
    fn drain_submenu_done(&mut self) {
        loop {
            let pending = match self.submenu_done.as_ref() {
                Some(done) => done.take_pending(),
                None => break,
            };
            if pending.is_empty() {
                break;
            }
            for call in pending {
                let item_id = self.submenu_item_id.clone();
                if let (Some(selected_value), Some(item_id)) = (call.selected_value, item_id) {
                    if let Some(item) = self.items.iter_mut().find(|item| item.id == item_id) {
                        item.current_value = selected_value.clone();
                        let on_change = &self.on_change;
                        on_change(&item_id, &selected_value);
                    }
                }
                if let Some(navigate_to) = call.navigate_to {
                    self.navigate_after_close = Some(navigate_to);
                }
                self.close_submenu();
            }
        }
    }

    /// `applyFilter(query)` (settings-list.ts:293-296).
    fn apply_filter(&mut self, query: &str) {
        // fuzzyFilter over (index, label) pairs, preserving best-first order.
        let labeled: Vec<(usize, String)> = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| (index, item.label.clone()))
            .collect();
        let filtered = fuzzy_filter(&labeled, query, |entry| entry.1.as_str());
        self.filtered_items = filtered.iter().map(|entry| entry.0).collect();
        self.selected_index = 0;
    }

    /// `addHintLine(lines, width)` (settings-list.ts:298-311).
    fn add_hint_line(&self, lines: &mut Vec<String>, width: usize) {
        lines.push(String::new());
        let hint_text = if self.search_enabled {
            "  Type to search · Enter/Space to change · Esc to cancel"
        } else {
            "  Enter/Space to change · Esc to cancel"
        };
        lines.push(truncate_to_width_default(&(self.theme.hint)(hint_text), width));
    }
}

impl Component for SettingsList {
    /// `render(width)` (settings-list.ts:91-98) — submenu renders instead
    /// of the main list while active.
    fn render(&mut self, width: usize) -> Vec<String> {
        if let Some(submenu) = self.submenu_component.clone() {
            return submenu.borrow_mut().render(width);
        }

        self.render_main_list(width)
    }

    /// `handleMouse(event)` (settings-list.ts:163-201).
    fn handle_mouse(
        &mut self,
        _self_ref: &ComponentRef,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        if let Some(submenu) = self.submenu_component.clone() {
            let result = submenu.borrow_mut().handle_mouse(&submenu, event);
            self.drain_submenu_done();
            return match result {
                Some(ComponentMouseResult::Result(mut result)) => {
                    result.focus = Some(true);
                    Some(ComponentMouseResult::Result(result))
                }
                Some(ComponentMouseResult::Forwarded(mut forwarded)) => {
                    forwarded.focus = Some(true);
                    Some(ComponentMouseResult::Forwarded(forwarded))
                }
                None => None,
            };
        }

        if self.search_enabled {
            if event.y == 0 {
                if let Some(search_input) = self.search_input.clone() {
                    let input_ref: ComponentRef = search_input.clone();
                    let result = search_input.borrow_mut().handle_mouse(&input_ref, event);
                    return match result {
                        Some(ComponentMouseResult::Result(mut result)) => {
                            result.focus = Some(true);
                            Some(ComponentMouseResult::Result(result))
                        }
                        Some(ComponentMouseResult::Forwarded(mut forwarded)) => {
                            forwarded.focus = Some(true);
                            Some(ComponentMouseResult::Forwarded(forwarded))
                        }
                        None => None,
                    };
                }
            }
            if event.y == 1 {
                return None;
            }
        }

        let display_items = self.display_indices();
        if display_items.is_empty() {
            return None;
        }
        if event.event_type == TuiMouseEventType::Wheel {
            // JS truthiness: `event.wheelDelta` must be present and nonzero
            if let Some(raw_delta) = event.wheel_delta {
                if raw_delta != 0 {
                    let delta: i64 = if raw_delta < 0 { -1 } else { 1 };
                    let previous_index = self.selected_index;
                    let len = display_items.len() as i64;
                    self.selected_index =
                        (0.max((len - 1).min(self.selected_index as i64 + delta))) as usize;
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

        let row_offset: i64 = if self.search_enabled { 2 } else { 0 };
        let (start_index, end_index) = self.visible_range(display_items.len());
        let item_index = start_index as i64 + event.y - row_offset;
        if item_index < start_index as i64 || item_index >= end_index as i64 {
            return None;
        }
        let item_index = item_index as usize;
        if event.event_type == TuiMouseEventType::Press {
            self.mouse_pressed_index = Some(item_index);
            self.selected_index = item_index;
            return Some(ComponentMouseResult::Result(TuiMouseEventResult {
                handled: Some(true),
                focus: Some(true),
                ..Default::default()
            }));
        }
        // Click
        self.selected_index = self.mouse_pressed_index.take().unwrap_or(item_index);
        self.activate_item();
        Some(ComponentMouseResult::Result(TuiMouseEventResult {
            handled: Some(true),
            ..Default::default()
        }))
    }

    /// `handleInput(data)` (settings-list.ts:203-232).
    fn handle_input(&mut self, data: &str) {
        // If submenu is active, delegate all input to it. The submenu's
        // onCancel (triggered by escape) will call done() which closes it.
        if let Some(submenu) = self.submenu_component.clone() {
            submenu.borrow_mut().handle_input(data);
            self.drain_submenu_done();
            return;
        }

        // Main list input handling
        let kb = get_keybindings();
        let display_len = self.display_indices().len();
        if kb.matches(data, "tui.select.up") {
            if display_len == 0 {
                return;
            }
            self.selected_index = if self.selected_index == 0 {
                display_len - 1
            } else {
                self.selected_index - 1
            };
        } else if kb.matches(data, "tui.select.down") {
            if display_len == 0 {
                return;
            }
            self.selected_index = if self.selected_index == display_len - 1 {
                0
            } else {
                self.selected_index + 1
            };
        } else if kb.matches(data, "tui.select.confirm")
            || (data == " " && (!self.search_enabled || self.search_value_is_empty()))
        {
            self.activate_item();
        } else if kb.matches(data, "tui.select.cancel") {
            (self.on_cancel)();
        } else if self.search_enabled {
            if let Some(search_input) = self.search_input.clone() {
                search_input.borrow_mut().handle_input(data);
                let value = search_input.borrow().get_value().to_string();
                self.apply_filter(&value);
            }
        }
    }

    fn handles_input(&self) -> bool {
        true
    }

    /// `invalidate()` (settings-list.ts:87-89).
    fn invalidate(&mut self) {
        if let Some(submenu) = self.submenu_component.clone() {
            submenu.borrow_mut().invalidate();
        }
    }
}

impl SettingsList {
    /// `this.searchInput?.getValue().length === 0` — `false` when there is
    /// no search input (JS optional-chain semantics).
    fn search_value_is_empty(&self) -> bool {
        self.search_input
            .as_ref()
            .map(|input| input.borrow().get_value().is_empty())
            .unwrap_or(false)
    }
}

// =============================================================================
// Tests — port of test/settings-list.test.ts
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme() -> SettingsListTheme {
        SettingsListTheme::default()
    }

    fn items() -> Vec<SettingItem> {
        vec![SettingItem::new("tui-mode", "TUI mode", "regular")
            .with_values(vec!["regular".to_string(), "fullscreen".to_string()])]
    }

    /// "includes spaces in an active search instead of changing the
    /// selected setting" (settings-list.test.ts:21-41).
    #[test]
    fn includes_spaces_in_an_active_search_instead_of_changing_the_selected_setting() {
        let changes: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded = changes.clone();
        let mut list = SettingsList::with_options(
            items(),
            10,
            test_theme(),
            Box::new(move |id, value| {
                recorded.borrow_mut().push((id.to_string(), value.to_string()));
            }),
            Box::new(|| {}),
            SettingsListOptions {
                enable_search: Some(true),
            },
        );

        for character in ["T", "U", "I", " ", "m", "o", "d", "e"] {
            list.handle_input(character);
        }

        assert!(changes.borrow().is_empty());
        assert!(list.render(80)[0].contains("TUI mode"));

        list.handle_input("\r");
        assert_eq!(
            *changes.borrow(),
            vec![("tui-mode".to_string(), "fullscreen".to_string())]
        );
    }

    /// "keeps Space as a change shortcut before a search query is entered"
    /// (settings-list.test.ts:43-58).
    #[test]
    fn keeps_space_as_a_change_shortcut_before_a_search_query_is_entered() {
        let changes: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded = changes.clone();
        let mut list = SettingsList::with_options(
            items(),
            10,
            test_theme(),
            Box::new(move |id, value| {
                recorded.borrow_mut().push((id.to_string(), value.to_string()));
            }),
            Box::new(|| {}),
            SettingsListOptions {
                enable_search: Some(true),
            },
        );

        list.handle_input(" ");

        assert_eq!(
            *changes.borrow(),
            vec![("tui-mode".to_string(), "fullscreen".to_string())]
        );
    }

    /// Empty-string description is JS-falsy (settings-list.ts:165
    /// `if (selectedItem?.description)`) — no description block is
    /// rendered. T3 redteam lists-F2 regression test.
    #[test]
    fn empty_description_is_falsy_and_renders_no_description_block() {
        let with_empty = vec![SettingItem::new("a", "a", "v1").with_description("")];
        let without = vec![SettingItem::new("a", "a", "v1")];

        let empty_changes: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded = empty_changes.clone();
        let mut list_empty = SettingsList::with_options(
            with_empty,
            10,
            test_theme(),
            Box::new(move |id, value| {
                recorded.borrow_mut().push((id.to_string(), value.to_string()));
            }),
            Box::new(|| {}),
            Default::default(),
        );

        let changes2: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded2 = changes2.clone();
        let mut list_none = SettingsList::with_options(
            without,
            10,
            test_theme(),
            Box::new(move |id, value| {
                recorded2.borrow_mut().push((id.to_string(), value.to_string()));
            }),
            Box::new(|| {}),
            Default::default(),
        );

        assert_eq!(list_empty.render(80), list_none.render(80));
    }

    /// `selectItem(id)` positions the selection in the FILTERED list
    /// (settings-list.ts:85 `findIndex` over `filteredItems`), so after a
    /// search that reorders rows the correct setting is activated. T3
    /// redteam lists-F3 regression test.
    #[test]
    fn select_item_positions_within_the_filtered_list() {
        let changes: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded = changes.clone();
        let mut list = SettingsList::with_options(
            vec![
                SettingItem::new("ab", "ab", "ab-1")
                    .with_values(vec!["ab-1".to_string(), "ab-2".to_string()]),
                SettingItem::new("ba", "ba", "ba-1")
                    .with_values(vec!["ba-1".to_string(), "ba-2".to_string()]),
            ],
            10,
            test_theme(),
            Box::new(move |id, value| {
                recorded.borrow_mut().push((id.to_string(), value.to_string()));
            }),
            Box::new(|| {}),
            SettingsListOptions {
                enable_search: Some(true),
            },
        );

        // Search "b": fuzzy filter reorders to [ba, ab].
        list.handle_input("b");
        assert!(list.render(80).iter().any(|line| line.contains("ba")));

        // Select the second filtered row (item "ab") and activate it.
        list.select_item("ab");
        list.handle_input("\r");
        assert_eq!(
            *changes.borrow(),
            vec![("ab".to_string(), "ab-2".to_string())]
        );
    }
}
