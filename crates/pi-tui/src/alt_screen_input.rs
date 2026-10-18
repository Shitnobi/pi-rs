//! Port of the input/mouse/lifecycle half of
//! `packages/tui/src/tui-alt-screen.ts` — **T5 Wave B, task D-B1**.
//!
//! Scope (tui-alt-screen.ts, method inventory):
//! - Terminal lifecycle: `beforeTerminalStart` (tui-alt-screen.ts:345),
//!   `beforeTerminalStop` (tui-alt-screen.ts:393) and
//!   `afterTerminalStop` (tui-alt-screen.ts:407) — the alt-screen
//!   enter/exit ESC sequences ([`crate::tui_alt_screen`] constants), mouse
//!   mode enable/disable (button-motion under multiplexers), autowrap
//!   disable/restore, iTerm2 capability masking, and the
//!   preserve-screen vs. document-dump exit paths.
//!   (`afterTerminalStart` is the empty `TuiBase` default — tui.ts:558.)
//! - `handleViewportInput` (tui-alt-screen.ts:674): FOCUS_IN/FOCUS_OUT,
//!   wheel events (`parseWheelEvent`/`routeWheel` with
//!   [`crate::tui_alt_screen::ALT_WHEEL_SCROLL_MULTIPLIER`] and
//!   `PAGE_SCROLL_OVERLAP` for the page-up/down keybindings), the SGR
//!   mouse pipeline, search-mode keybindings, and overlay deferral.
//! - The mouse event pipeline: `handleMouseEvent`, `decodeMouseButton`,
//!   `createMouseEvent`, `dispatchMouseToLayout`,
//!   `applyMouseDispatchResult`, `dispatchMouseToTarget`,
//!   `getComponentClickCount`, `clearComponentMouseGesture`,
//!   `handleRightClickPaste`, `handleScrollToEndIndicatorMouseEvent`,
//!   `isMouseSequence`.
//! - Scrollbar interactions: `getScrollbarTargetAt`, `setScrollbarHover`,
//!   `updateScrollbarHover`, `stopScrollbarHover`,
//!   `scrollScrollbarToPointer`, `handleScrollbarMouseEvent`,
//!   `stopScrollbarDrag`.
//! - The scroll API: `scrollBy`, `scrollToTop`, `scrollToBottom`,
//!   `scrollToPrompt` (`viewportTop`/`isFollowingOutput` getters already
//!   live in the coordinator-owned skeleton).
//! - Search control flow: `toggleSearch`, `closeSearch`,
//!   `updateSearchQuery`, `navigateSearch`,
//!   `getSearchNavigationDirectionAt`, `handleSearchMouseEvent`,
//!   `refreshSearch` (the corpus/index machinery lives in
//!   [`crate::alt_screen_search`]).
//!
//! Text selection is **D-B2** (`crate::alt_screen_selection`); the render
//! pipeline and the kitty image cache are **D-B3**
//! (`crate::alt_screen_render`). The screen struct, constructor, option
//! types and hook wiring are coordinator-owned in
//! [`crate::tui_alt_screen`]; behavior is added here as `impl TuiAltScreen`
//! blocks and free functions.
//!
//! ## Port notes (read before touching the query plumbing)
//!
//! - **`updateSearchQuery` re-entrancy**: the TS search component holds an
//!   `onQueryChange` closure calling `this.updateSearchQuery(query)`
//!   synchronously. The closure fires from the focused component's
//!   `handleInput`, which runs while the screen's `RefCell` is mutably
//!   borrowed (the input-dispatch path) — impossible through the `RefCell`.
//!   The port parks the query by `tui_id`
//!   ([`PENDING_SEARCH_QUERIES`]) plus a parked render request, and the
//!   consumers (`navigate_search`, `refresh_search`) drain the parked
//!   query before reading search state — the same observable effect as
//!   the TS synchronous update, because nothing between the keypress and
//!   the next render reads the query.
//! - **Overlay handles**: `OverlayHandle::hide`/`is_focused`/`get_bounds`
//!   re-borrow the owning screen (`Weak<RefCell<dyn TuiSurface>>`), which
//!   panics while our caller already holds the borrow. The port recovers
//!   the overlay `entry_id` from `base.overlay_stack` by component
//!   identity and calls the `TuiSurface` methods directly.
//! - **`stopSelectionAutoScroll`** is selection state (**D-B2**), but the
//!   lifecycle and scrollbar paths must clear it; the fields are
//!   `pub(crate)` on the screen, so this module clears them directly
//!   ([`stop_selection_auto_scroll`]). **D-B2** owns firing the deadline
//!   from the clock pump.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::alt_screen_search::{
    get_alt_screen_search_match_key, AltScreenSearchComponent, AltScreenSearchIndex,
    NavigationButtonStyle, OnQueryChange,
};
use crate::clock::now_ms;
use crate::component::{
    dispatch_mouse_event, retarget_mouse_event, Component, ComponentRef, CURSOR_MARKER,
    TuiMouseButton, TuiMouseDispatchResult, TuiMouseDispatchTarget, TuiMouseEvent,
    TuiMouseEventType,
};
use crate::components::scroll_view::ScrollViewScrollToOptions;
use crate::keybindings::get_keybindings;
use crate::keys::is_key_release;
use crate::layout::{
    get_layout_boxes_at, get_scroll_view_box, get_scroll_views_at, get_scrollbar_geometry,
    LayoutFrame, ScrollbarGeometry, ScrollViewRef,
};
use crate::layout_node::Overscroll;
use crate::terminal_image::{self, ImageProtocol, TerminalCapabilities};
use crate::tui::{
    park_render_request, OverlayAnchor, OverlayBounds, OverlayMarginOrNumber, OverlayOptions,
    SizeValue, TuiInputListenerResult, TuiStopOptions, TuiSurface,
};
use crate::tui_alt_screen::{
    ActiveSearch, SearchSelectionMode, SelectionGranularity, SgrMouseEvent, TuiAltScreen,
    WheelEvent, LastComponentClick, ScrollbarDragState,
    ALT_WHEEL_SCROLL_MULTIPLIER, BEGIN_SYNCHRONIZED_OUTPUT, DISABLE_AUTOWRAP, DISABLE_MOUSE,
    DOUBLE_CLICK_INTERVAL_MS, ENABLE_ALL_MOTION_MOUSE, ENABLE_AUTOWRAP,
    ENABLE_BUTTON_MOTION_MOUSE, END_SYNCHRONIZED_OUTPUT, ENTER_ALT_SCREEN, EXIT_ALT_SCREEN,
    FOCUS_IN, FOCUS_OUT, PAGE_SCROLL_OVERLAP, starts_with_osc133_prompt_start,
    strip_osc133_zone_prefix,
};
use crate::utils::{slice_by_column, visible_width};

// =============================================================================
// Parked search queries (see module docs)
// =============================================================================

thread_local! {
    /// Queries typed into the search component while the screen's `RefCell`
    /// was mutably borrowed, keyed by `tui_id`. Last write wins, exactly
    /// like the TS synchronous `query` field assignment.
    static PENDING_SEARCH_QUERIES: RefCell<HashMap<usize, String>> =
        RefCell::new(HashMap::new());
}

/// Park a query update for the screen with this `tui_id` (the
/// `onQueryChange` closure body — TS applies it synchronously, see module
/// docs).
fn park_search_query(tui_id: usize, query: String) {
    PENDING_SEARCH_QUERIES.with(|map| {
        map.borrow_mut().insert(tui_id, query);
    });
}

/// Drop a parked query (a direct `update_search_query` call supersedes any
/// parked closure update).
fn clear_pending_search_query(tui_id: usize) {
    PENDING_SEARCH_QUERIES.with(|map| {
        map.borrow_mut().remove(&tui_id);
    });
}

/// Apply a query parked by the search component's `onQueryChange` closure,
/// if any. Called before any read of the active search state.
fn apply_pending_search_query(screen: &mut TuiAltScreen) {
    let pending = PENDING_SEARCH_QUERIES
        .with(|map| map.borrow_mut().remove(&screen.base.tui_id));
    if let Some(query) = pending {
        screen.update_search_query(&query);
    }
}

// =============================================================================
// SGR mouse / wheel sequence parsing (tui-alt-screen.ts:1010-1052)
// =============================================================================

/// `/^(\d+)$/` — one or more ASCII digits (regex `\d+`).
fn parse_decimal(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>().ok()
}

/// `parseSgrMouseEvent(data)` (tui-alt-screen.ts:1010-1018):
/// `/^\x1b\[<(\d+);(\d+);(\d+)([Mm])$/`. Coordinates are 1-based in the
/// sequence and 0-based in the event.
pub(crate) fn parse_sgr_mouse_event(data: &str) -> Option<SgrMouseEvent> {
    let rest = data.strip_prefix("\x1b[<")?;
    let final_char = rest.chars().next_back()?;
    let params = &rest[..rest.len() - final_char.len_utf8()];
    let mut parts = params.split(';');
    let button = parse_decimal(parts.next()?)?;
    let x = parse_decimal(parts.next()?)?;
    let y = parse_decimal(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    let release = match final_char {
        'm' => true,
        'M' => false,
        _ => return None,
    };
    Some(SgrMouseEvent {
        button,
        x: x - 1,
        y: y - 1,
        release,
    })
}

/// `parseWheelEvent(data)` (tui-alt-screen.ts:1024-1052): the SGR form plus
/// the legacy 6-byte `\x1b[M` encoding. Button codes with bit 6 set (64)
/// are wheel events; the low two bits pick the direction (0 = up, 1 =
/// down); anything else is not a wheel event.
pub(crate) fn parse_wheel_event(data: &str) -> Option<WheelEvent> {
    if data.starts_with("\x1b[<") {
        let sgr = parse_sgr_mouse_event(data)?;
        let button = sgr.button;
        if (button & 64) == 0 {
            return None;
        }
        let direction = button & 3;
        if direction != 0 && direction != 1 {
            return None;
        }
        return Some(WheelEvent {
            direction: if direction == 0 { -1 } else { 1 },
            x: sgr.x,
            y: sgr.y,
            button,
        });
    }
    let bytes = data.as_bytes();
    if data.len() == 6 && bytes.starts_with(b"\x1b[M") {
        let button = bytes[3] as i64 - 32;
        if (button & 64) == 0 {
            return None;
        }
        let direction = button & 3;
        if direction != 0 && direction != 1 {
            return None;
        }
        return Some(WheelEvent {
            direction: if direction == 0 { -1 } else { 1 },
            x: bytes[4] as i64 - 33,
            y: bytes[5] as i64 - 33,
            button,
        });
    }
    None
}

/// `isMouseSequence(data)` (tui-alt-screen.ts:1637-1639):
/// `/^\x1b\[<\d+;\d+;\d+[Mm]$/` or the 6-byte legacy `\x1b[M` prefix.
fn is_mouse_sequence(data: &str) -> bool {
    parse_sgr_mouse_event(data).is_some() || (data.len() == 6 && data.starts_with("\x1b[M"))
}

/// `decodeMouseButton(button)` (tui-alt-screen.ts:757-766).
fn decode_mouse_button(button: i64) -> TuiMouseButton {
    match button & 3 {
        0 => TuiMouseButton::Left,
        1 => TuiMouseButton::Middle,
        2 => TuiMouseButton::Right,
        _ => TuiMouseButton::None,
    }
}

// =============================================================================
// Selection-state helpers owned by D-B2 but cleared from this half
// =============================================================================

/// `stopSelectionAutoScroll()` (tui-alt-screen.ts:1287-1293) — the state
/// reset only. The timer is a virtual-clock deadline (**D-B2** fires it
/// from the clock pump); clearing it here disarms the pump, exactly like
/// TS `clearInterval`.
fn stop_selection_auto_scroll(screen: &mut TuiAltScreen) {
    screen.selection_auto_scroll_timer = None;
    screen.selection_auto_scroll_direction = 0;
    screen.selection_drag_pointer = None;
}

/// The search overlay's `entry_id`, recovered from the overlay stack by
/// component identity (the `OverlayHandle` methods re-borrow the screen —
/// see module docs).
fn active_search_entry_id(screen: &TuiAltScreen) -> Option<u64> {
    let search = screen.active_search.as_ref()?;
    let component: ComponentRef = search.component.clone();
    screen
        .base
        .overlay_stack
        .iter()
        .find(|entry| Rc::ptr_eq(&entry.component, &component))
        .map(|entry| entry.entry_id)
}

/// `this.activeSearch?.overlay?.getBounds()` without re-borrowing the
/// screen.
fn active_search_overlay_bounds(screen: &TuiAltScreen) -> Option<OverlayBounds> {
    let entry_id = active_search_entry_id(screen)?;
    screen.base.overlay_entry_bounds(entry_id)
}

/// `isOverlayFocused()` (tui.ts:855-859): the focused component is a
/// visible overlay.
fn is_overlay_focused(screen: &TuiAltScreen) -> bool {
    screen.base.overlay_stack.iter().any(|entry| {
        screen
            .base
            .focused_component
            .as_ref()
            .is_some_and(|focused| Rc::ptr_eq(focused, &entry.component))
            && screen.base.is_overlay_visible(entry.entry_id)
    })
}

// =============================================================================
// Lifecycle (tui-alt-screen.ts:345-440)
// =============================================================================

/// `beforeTerminalStart()` (tui-alt-screen.ts:345-392).
pub(crate) fn before_terminal_start(screen: &mut TuiAltScreen) {
    stop_selection_auto_scroll(screen);
    screen.selection_press_active = false;
    screen.stop_scrollbar_hover();
    screen.stop_scrollbar_drag();
    screen.flashes.dispose();
    screen.alt_screen_active = true;
    let capabilities = terminal_image::get_capabilities();
    // `this.imageProtocol = capabilities.images` (tui-alt-screen.ts:354).
    screen.image_protocol = capabilities.images;
    screen.uploaded_kitty_images.clear();
    if capabilities.images == Some(ImageProtocol::Iterm2) {
        screen.saved_capabilities = Some(capabilities);
        terminal_image::set_capabilities(TerminalCapabilities {
            images: None,
            true_color: capabilities.true_color,
            hyperlinks: capabilities.hyperlinks,
        });
        // `this.invalidate()` (the TuiBase override: mounted roots +
        // overlay stack).
        Component::invalidate(screen);
    }
    screen.last_document.clear();
    screen.selection_anchor = None;
    screen.selection_focus = None;
    screen.selection_granularity = SelectionGranularity::Character;
    screen.selection_initial_range = None;
    screen.last_click = None;
    screen.pressed_url = None;
    screen.selection_dragged = false;
    screen.clear_component_mouse_gesture();
    screen.last_component_click = None;
    crate::alt_screen_render::reset_render_state(screen);
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    // Multiplexers can lag when every pointer movement is forwarded.
    // Button-motion tracking preserves clicks, wheel events, selections,
    // and scrollbar dragging (tui-alt-screen.ts:383-384).
    let mouse_sequence = if std::env::var_os("TMUX").is_some()
        || std::env::var_os("ZELLIJ").is_some()
        || std::env::var_os("STY").is_some()
        || term.starts_with("tmux")
        || term.starts_with("screen")
    {
        ENABLE_BUTTON_MOTION_MOUSE
    } else {
        ENABLE_ALL_MOTION_MOUSE
    };
    let mut sequence = String::new();
    sequence.push_str(ENTER_ALT_SCREEN);
    sequence.push_str(DISABLE_AUTOWRAP);
    if screen.mouse_enabled {
        sequence.push_str(mouse_sequence);
    }
    sequence.push_str("\x1b[2J\x1b[H\x1b[?25l");
    screen.base.terminal.borrow_mut().write(&sequence);
}

/// `afterTerminalStart()` — the `TuiBase` default is empty (tui.ts:558)
/// and `TuiAltScreen` does not override it.
pub(crate) fn after_terminal_start(_screen: &mut TuiAltScreen) {}

/// `deleteKittyImages()` (tui-alt-screen.ts:429-431).
fn delete_kitty_images(screen: &TuiAltScreen) -> &'static str {
    if screen.image_protocol == Some(ImageProtocol::Kitty) {
        terminal_image::delete_all_kitty_images()
    } else {
        ""
    }
}

/// `beforeTerminalStop(options)` (tui-alt-screen.ts:393-405).
pub(crate) fn before_terminal_stop(screen: &mut TuiAltScreen, _options: &TuiStopOptions) {
    screen.close_search();
    stop_selection_auto_scroll(screen);
    screen.selection_press_active = false;
    screen.stop_scrollbar_hover();
    screen.stop_scrollbar_drag();
    screen.clear_component_mouse_gesture();
    screen.flashes.dispose();
    if !screen.alt_screen_active {
        return;
    }
    let mut sequence = String::from(BEGIN_SYNCHRONIZED_OUTPUT);
    sequence.push_str(delete_kitty_images(screen));
    if screen.mouse_enabled {
        sequence.push_str(DISABLE_MOUSE);
    }
    sequence.push_str(ENABLE_AUTOWRAP);
    sequence.push_str(END_SYNCHRONIZED_OUTPUT);
    screen.base.terminal.borrow_mut().write(&sequence);
    screen.uploaded_kitty_images.clear();
}

/// `afterTerminalStop(options)` (tui-alt-screen.ts:407-428).
pub(crate) fn after_terminal_stop(screen: &mut TuiAltScreen, options: &TuiStopOptions) {
    if !screen.alt_screen_active {
        return;
    }
    screen.alt_screen_active = false;
    if options.preserve_screen {
        let mut sequence = String::from(BEGIN_SYNCHRONIZED_OUTPUT);
        sequence.push_str(EXIT_ALT_SCREEN);
        sequence.push_str("\x1b[?25h");
        sequence.push_str(END_SYNCHRONIZED_OUTPUT);
        screen.base.terminal.borrow_mut().write(&sequence);
    } else {
        let width = (screen.base.terminal.borrow().columns() as i64).max(1) as usize;
        // `this.resolveFakeCursors(this.render(width))` → strip OSC133 zone
        // prefixes → drop CURSOR_MARKER → `applyLineResets` → overflow
        // truncation (tui-alt-screen.ts:415-419).
        let rendered = TuiSurface::render_children(screen, width);
        let document_lines: Vec<String> = rendered
            .into_iter()
            .map(|line| strip_osc133_zone_prefix(&line).to_string())
            .map(|line| line.replace(CURSOR_MARKER, ""))
            .collect();
        let reset_lines = screen.base.apply_line_resets(document_lines);
        screen.last_document = reset_lines
            .into_iter()
            .map(|line| {
                if terminal_image::is_image_line(&line) || visible_width(&line) <= width {
                    line
                } else {
                    slice_by_column(&line, 0, width, true)
                }
            })
            .collect();
        let mut buffer = String::from(BEGIN_SYNCHRONIZED_OUTPUT);
        buffer.push_str(EXIT_ALT_SCREEN);
        buffer.push_str(DISABLE_AUTOWRAP);
        for (row, line) in screen.last_document.iter().enumerate() {
            if row > 0 {
                buffer.push_str("\r\n");
            }
            buffer.push_str("\r\x1b[2K");
            buffer.push_str(line);
        }
        buffer.push_str("\x1b[0m");
        buffer.push_str(ENABLE_AUTOWRAP);
        buffer.push_str("\r\n\x1b[?25h");
        buffer.push_str(END_SYNCHRONIZED_OUTPUT);
        screen.base.terminal.borrow_mut().write(&buffer);
    }
    if let Some(saved) = screen.saved_capabilities.take() {
        terminal_image::set_capabilities(saved);
    }
}

// =============================================================================
// handleViewportInput (tui-alt-screen.ts:674-755)
// =============================================================================

/// `(data) => this.handleViewportInput(data)` (tui-alt-screen.ts:282).
pub(crate) fn handle_viewport_input(
    screen: &mut TuiAltScreen,
    data: &str,
) -> Option<TuiInputListenerResult> {
    screen.viewport_input(data)
}

/// `refreshSearch(layout)` (tui-alt-screen.ts:595-663) — re-runs the active
/// search against the freshly rendered layout. Returns whether the
/// selected match reveal moved the viewport (the caller re-reads the
/// scroll position; a `true` also implies the search UI state changed).
pub(crate) fn refresh_search(screen: &mut TuiAltScreen, layout: &LayoutFrame) -> bool {
    screen.refresh_search_impl(layout)
}

impl TuiAltScreen {
    /// `handleViewportInput(data)` (tui-alt-screen.ts:674-755).
    fn viewport_input(&mut self, data: &str) -> Option<TuiInputListenerResult> {
        let consume = || TuiInputListenerResult {
            consume: Some(true),
            data: None,
        };

        if data == FOCUS_OUT {
            // (tui-alt-screen.ts:678-702)
            let had_active_selection = self.selection_press_active;
            // TS reads `getSelectionBounds() !== undefined` (D-B2);
            // `getActiveSelectionText() !== undefined` carries the same
            // "non-empty selection" truth here (both are undefined for an
            // empty/degenerate selection).
            let had_non_empty_active_selection = had_active_selection
                && crate::alt_screen_selection::get_active_selection_text(self).is_some();
            self.selection_press_active = false;
            stop_selection_auto_scroll(self);
            self.stop_scrollbar_hover();
            let hovered_cleared = match &mut self.active_search {
                Some(search) => search
                    .component
                    .borrow_mut()
                    .set_hovered_navigation_direction(None),
                None => false,
            };
            if hovered_cleared {
                self.request_render(false);
            }
            self.stop_scrollbar_drag();
            self.pressed_url = None;
            self.selection_dragged = false;
            self.clear_component_mouse_gesture();
            self.last_component_click = None;
            if had_active_selection {
                self.selection_anchor = None;
                self.selection_focus = None;
                self.selection_granularity = SelectionGranularity::Character;
                self.selection_initial_range = None;
                if had_non_empty_active_selection {
                    self.request_render(false);
                }
            }
            self.last_click = None;
            return Some(consume());
        }
        if data == FOCUS_IN {
            return Some(consume());
        }

        let wheel_event = parse_wheel_event(data);
        if let Some(wheel_event) = wheel_event {
            // (tui-alt-screen.ts:706-720)
            let lines = self.wheel_scroll.next(wheel_event.direction, now_ms());
            // SGR mouse button codes use bit 3 (value 8) for the Alt
            // modifier; Alt+wheel moves five times as far.
            let wheel_delta = wheel_event.direction
                * if (wheel_event.button & 8) != 0 {
                    (lines * ALT_WHEEL_SCROLL_MULTIPLIER as f64) as i64
                } else {
                    lines as i64
                };
            let event = self.create_mouse_event(
                TuiMouseEventType::Wheel,
                wheel_event.button,
                wheel_event.x,
                wheel_event.y,
                Some(wheel_delta),
                None,
            );
            let overlay = self.base.dispatch_mouse_to_overlay(&event);
            let result = match overlay {
                Some(result) => result,
                None => self.dispatch_mouse_to_layout(&event),
            };
            if let Some(result) = result {
                if self.apply_mouse_dispatch_result(&event, result) {
                    self.request_render(false);
                }
                return Some(consume());
            }
            self.route_wheel(&wheel_event, wheel_delta);
            return Some(consume());
        }
        if let Some(mouse_event) = parse_sgr_mouse_event(data) {
            self.handle_mouse_event(mouse_event);
            return Some(consume());
        }
        if is_mouse_sequence(data) {
            return Some(consume());
        }

        // (tui-alt-screen.ts:727-754) — keyboard bindings.
        let keybindings = get_keybindings();
        let is_release = is_key_release(data);
        if keybindings.matches(data, "tui.altScreen.search") {
            if !is_release {
                self.toggle_search();
            }
            return Some(consume());
        }
        if self
            .active_search
            .as_ref()
            .and_then(|_| active_search_entry_id(self))
            .map(|entry_id| self.base.is_overlay_entry_focused(entry_id))
            .unwrap_or(false)
        {
            if keybindings.matches(data, "tui.altScreen.searchNext") {
                if !is_release {
                    self.navigate_search(1);
                }
                return Some(consume());
            }
            if keybindings.matches(data, "tui.altScreen.searchPrevious") {
                if !is_release {
                    self.navigate_search(-1);
                }
                return Some(consume());
            }
            if keybindings.matches(data, "tui.altScreen.searchClose") {
                if !is_release {
                    self.close_search();
                }
                return Some(consume());
            }
        }
        if self.should_defer_viewport_input_to_overlay() {
            return None;
        }
        let viewport_height = self.get_primary_scroll_view().borrow().viewport_height();
        if keybindings.matches(data, "tui.altScreen.pageUp") {
            if !is_release {
                self.scroll_by(-(viewport_height - PAGE_SCROLL_OVERLAP).max(1));
            }
            return Some(consume());
        }
        if keybindings.matches(data, "tui.altScreen.pageDown") {
            if !is_release {
                self.scroll_by((viewport_height - PAGE_SCROLL_OVERLAP).max(1));
            }
            return Some(consume());
        }
        if keybindings.matches(data, "tui.altScreen.halfPageUp") {
            if !is_release {
                self.scroll_by(-(viewport_height / 2).max(1));
            }
            return Some(consume());
        }
        if keybindings.matches(data, "tui.altScreen.halfPageDown") {
            if !is_release {
                self.scroll_by((viewport_height / 2).max(1));
            }
            return Some(consume());
        }
        if keybindings.matches(data, "tui.altScreen.lineUp") {
            if !is_release {
                self.scroll_by(-1);
            }
            return Some(consume());
        }
        if keybindings.matches(data, "tui.altScreen.lineDown") {
            if !is_release {
                self.scroll_by(1);
            }
            return Some(consume());
        }
        if keybindings.matches(data, "tui.altScreen.previousPrompt") {
            if !is_release {
                self.scroll_to_prompt(-1);
            }
            return Some(consume());
        }
        if keybindings.matches(data, "tui.altScreen.nextPrompt") {
            if !is_release {
                self.scroll_to_prompt(1);
            }
            return Some(consume());
        }
        if keybindings.matches(data, "tui.altScreen.top") {
            if !is_release {
                self.scroll_to_top();
            }
            return Some(consume());
        }
        if keybindings.matches(data, "tui.altScreen.bottom") {
            if !is_release {
                self.scroll_to_bottom();
            }
            return Some(consume());
        }
        None
    }

    /// `shouldDeferViewportInputToOverlay()` (tui-alt-screen.ts:670-672).
    fn should_defer_viewport_input_to_overlay(&self) -> bool {
        let search_overlay_focused = self
            .active_search
            .as_ref()
            .and_then(|_| active_search_entry_id(self))
            .map(|entry_id| self.base.is_overlay_entry_focused(entry_id))
            .unwrap_or(false);
        is_overlay_focused(self) && !search_overlay_focused
    }

    // -------------------------------------------------------------------------
    // Mouse pipeline (tui-alt-screen.ts:757-1010)
    // -------------------------------------------------------------------------

    /// `createMouseEvent(type, button, x, y, extra)` (tui-alt-screen.ts:768-787).
    fn create_mouse_event(
        &self,
        event_type: TuiMouseEventType,
        button: i64,
        x: i64,
        y: i64,
        wheel_delta: Option<i64>,
        click_count: Option<i64>,
    ) -> TuiMouseEvent {
        let columns = self.base.terminal.borrow().columns() as i64;
        let rows = self.base.terminal.borrow().rows() as i64;
        TuiMouseEvent {
            event_type,
            button: if event_type == TuiMouseEventType::Wheel {
                TuiMouseButton::None
            } else {
                decode_mouse_button(button)
            },
            x,
            y,
            screen_x: x,
            screen_y: y,
            width: columns.max(1),
            height: rows.max(1),
            shift: (button & 4) != 0,
            alt: (button & 8) != 0,
            ctrl: (button & 16) != 0,
            wheel_delta,
            click_count,
        }
    }

    /// `dispatchMouseToLayout(event)` (tui-alt-screen.ts:799-818): walk the
    /// visual hit path (deepest first) and dispatch to the first component
    /// that handles the event.
    fn dispatch_mouse_to_layout(&self, event: &TuiMouseEvent) -> Option<TuiMouseDispatchResult> {
        let layout = self.current_layout.as_ref()?;
        let boxes = get_layout_boxes_at(layout, event.screen_x, event.screen_y);
        let mut visited: Vec<ComponentRef> = Vec::new();
        for box_ in boxes {
            if visited.iter().any(|seen| Rc::ptr_eq(seen, &box_.component)) {
                continue;
            }
            // TS skips layout-participating components whose `handleMouse`
            // is the inherited `Container.prototype.handleMouse` (their
            // children appear in the hit path themselves). The Rust proxy:
            // a component exposing a `Container` child list (the
            // `Container.prototype.handleMouse` forwarders) with a layout
            // node — `ScrollView` overrides its mouse routing and exposes
            // no container list, so it is dispatched like in TS.
            let skip = {
                let mut component = box_.component.borrow_mut();
                component.layout_node().is_some() && component.container_children().is_some()
            };
            if skip {
                continue;
            }
            visited.push(box_.component.clone());
            let mut local = event.clone();
            local.x = event.screen_x - box_.rect.x;
            local.y = event.screen_y - box_.rect.y;
            local.width = box_.rect.width;
            local.height = box_.rect.height;
            if let Some(result) = dispatch_mouse_event(&box_.component, &local) {
                return Some(result);
            }
        }
        None
    }

    /// `applyMouseDispatchResult(event, result)` (tui-alt-screen.ts:820-833).
    fn apply_mouse_dispatch_result(
        &mut self,
        event: &TuiMouseEvent,
        result: TuiMouseDispatchResult,
    ) -> bool {
        let focus_component = result
            .focus_target
            .clone()
            .unwrap_or_else(|| result.target.component.clone());
        let focus_target = self.base.resolve_mouse_focus_target(&focus_component);
        let focus_changed = result.is_focus()
            && !self
                .base
                .get_focused_component()
                .is_some_and(|focused| Rc::ptr_eq(&focused, &focus_target));
        let capture_target = if result.is_capture() {
            Some(result.target.clone())
        } else {
            None
        };
        if result.is_focus() {
            TuiSurface::set_focus(self, Some(focus_target));
        }
        if let Some(target) = capture_target {
            self.mouse_capture = Some(target);
        }
        match result.render {
            Some(render) => render,
            None => {
                focus_changed
                    || matches!(
                        event.event_type,
                        TuiMouseEventType::Press
                            | TuiMouseEventType::Click
                            | TuiMouseEventType::Drag
                            | TuiMouseEventType::Wheel
                    )
            }
        }
    }

    /// `dispatchMouseToTarget(event, target)` (tui-alt-screen.ts:835-839).
    fn dispatch_mouse_to_target(
        &self,
        event: &TuiMouseEvent,
        target: &TuiMouseDispatchTarget,
    ) -> Option<TuiMouseDispatchResult> {
        dispatch_mouse_event(&target.component, &retarget_mouse_event(event, target))
    }

    /// `getComponentClickCount(target, x, y)` (tui-alt-screen.ts:841-854).
    fn get_component_click_count(
        &mut self,
        target: &TuiMouseDispatchTarget,
        x: i64,
        y: i64,
    ) -> i64 {
        let now = now_ms();
        let previous = self.last_component_click.take();
        let count = match previous {
            Some(ref previous)
                if now - previous.timestamp <= DOUBLE_CLICK_INTERVAL_MS
                    && Rc::ptr_eq(&previous.component, &target.component)
                    && previous.x == x
                    && previous.y == y =>
            {
                (previous.count % 3) + 1
            }
            _ => 1,
        };
        self.last_component_click = Some(LastComponentClick {
            timestamp: now,
            count,
            component: target.component.clone(),
            x,
            y,
        });
        count
    }

    /// `clearComponentMouseGesture()` (tui-alt-screen.ts:668-673).
    fn clear_component_mouse_gesture(&mut self) {
        self.mouse_capture = None;
        self.mouse_press_target = None;
        self.mouse_press_point = None;
        self.mouse_press_moved = false;
    }

    /// `handleMouseEvent(raw)` (tui-alt-screen.ts:889-976) — the SGR mouse
    /// state machine: capture routing, click synthesis, overlay dispatch,
    /// scrollbar and indicator hit testing, layout dispatch, right-click
    /// paste, and finally text selection (**D-B2**).
    fn handle_mouse_event(&mut self, raw: SgrMouseEvent) {
        let is_motion = (raw.button & 32) != 0;
        let event_type = if raw.release {
            TuiMouseEventType::Release
        } else if is_motion {
            if decode_mouse_button(raw.button) == TuiMouseButton::None {
                TuiMouseEventType::Move
            } else {
                TuiMouseEventType::Drag
            }
        } else {
            TuiMouseEventType::Press
        };
        let event = self.create_mouse_event(event_type, raw.button, raw.x, raw.y, None, None);

        // Capture / press-target routing (tui-alt-screen.ts:899-928).
        if self.mouse_capture.is_some() || self.mouse_press_target.is_some() {
            let target = self
                .mouse_capture
                .clone()
                .or_else(|| self.mouse_press_target.clone())
                .expect("one of mouseCapture/mousePressTarget is set");
            if let Some((press_x, press_y)) = self.mouse_press_point {
                if raw.x != press_x || raw.y != press_y {
                    self.mouse_press_moved = true;
                    self.last_component_click = None;
                }
            }
            let mut render = false;
            if let Some(target_result) = self.dispatch_mouse_to_target(&event, &target) {
                render = self.apply_mouse_dispatch_result(&event, target_result);
            }
            if raw.release {
                // `!mousePressMoved && mousePressPoint?.x === raw.x &&
                //  mousePressPoint.y === raw.y` — the point must exist and
                // match (the TS `?.` short-circuits the second access).
                let click_in_place = !self.mouse_press_moved
                    && self
                        .mouse_press_point
                        .is_some_and(|(press_x, press_y)| press_x == raw.x && press_y == raw.y);
                if click_in_place {
                    let click_count = self.get_component_click_count(&target, raw.x, raw.y);
                    let click_event = self.create_mouse_event(
                        TuiMouseEventType::Click,
                        raw.button,
                        raw.x,
                        raw.y,
                        None,
                        Some(click_count),
                    );
                    if let Some(click_result) = self.dispatch_mouse_to_target(&click_event, &target)
                    {
                        render = self.apply_mouse_dispatch_result(&click_event, click_result)
                            || render;
                    }
                }
                self.clear_component_mouse_gesture();
            }
            if render {
                self.request_render(false);
            }
            return;
        }

        if self.handle_search_mouse_event(&raw) {
            return;
        }

        let overlay = self.base.dispatch_mouse_to_overlay(&event);
        let overlay_hit = overlay.is_some();
        if !overlay_hit {
            if self.handle_scroll_to_end_indicator_mouse_event(&raw) {
                return;
            }
            let scrollbar_handled = self.handle_scrollbar_mouse_event(&raw);
            if self.scrollbar_drag.is_none() {
                self.update_scrollbar_hover(raw.x, raw.y);
            }
            if scrollbar_handled {
                return;
            }
        } else {
            self.stop_scrollbar_hover();
        }

        let result = match overlay {
            Some(result) => result,
            None => self.dispatch_mouse_to_layout(&event),
        };
        if let Some(result) = result {
            let press_target = result.target.clone();
            let render = self.apply_mouse_dispatch_result(&event, result);
            if event_type == TuiMouseEventType::Press {
                crate::alt_screen_selection::clear_text_selection(self);
                self.mouse_press_target = Some(press_target);
                self.mouse_press_point = Some((raw.x, raw.y));
                self.mouse_press_moved = false;
            }
            if render {
                self.request_render(false);
            }
            return;
        }

        if self.handle_right_click_paste(&raw) {
            return;
        }
        crate::alt_screen_selection::handle_selection_mouse_event(self, &raw);
    }

    /// `handleRightClickPaste(event)` (tui-alt-screen.ts:1054-1068) — the
    /// Windows-only unmodified secondary-button paste.
    fn handle_right_click_paste(&mut self, event: &SgrMouseEvent) -> bool {
        let term_program_vscode = std::env::var("TERM_PROGRAM")
            .unwrap_or_default()
            .to_lowercase()
            == "vscode";
        if self.on_right_click_paste.is_none()
            || !cfg!(windows)
            || term_program_vscode
            || event.release
            || event.button != 2
        {
            return false;
        }
        // Clipboard paste is best-effort (the TS try/catch has no Rust
        // counterpart; the callback is not expected to panic).
        if let Some(paste) = &self.on_right_click_paste {
            paste();
        }
        true
    }

    /// `handleScrollToEndIndicatorMouseEvent(event)`
    /// (tui-alt-screen.ts:1070-1077).
    fn handle_scroll_to_end_indicator_mouse_event(&mut self, event: &SgrMouseEvent) -> bool {
        let Some(rect) = self.scroll_to_end_indicator_rect else {
            return false;
        };
        if event.release || (event.button & 32) != 0 || (event.button & 3) != 0 {
            return false;
        }
        if event.y != rect.row || event.x < rect.column || event.x >= rect.column + rect.width {
            return false;
        }
        self.scroll_to_bottom();
        true
    }

    // -------------------------------------------------------------------------
    // Wheel routing (tui-alt-screen.ts:996-1008)
    // -------------------------------------------------------------------------

    /// `routeWheel(event, delta)` (tui-alt-screen.ts:996-1008): chained
    /// overscroll through the scroll views under the pointer, then the
    /// primary scroll view for the remainder.
    fn route_wheel(&mut self, event: &WheelEvent, delta: i64) {
        let mut remaining = delta;
        let mut seen: Vec<ScrollViewRef> = Vec::new();
        if let Some(layout) = self.current_layout.as_ref() {
            for scroll_view in get_scroll_views_at(layout, event.x, event.y) {
                seen.push(scroll_view.clone());
                remaining = scroll_view.borrow_mut().scroll_by(remaining);
                if remaining == 0 || scroll_view.borrow().overscroll() == Overscroll::Contain {
                    break;
                }
            }
        }
        let primary = self.get_primary_scroll_view();
        if remaining != 0 && !seen.iter().any(|seen| Rc::ptr_eq(seen, &primary)) {
            primary.borrow_mut().scroll_by(remaining);
        }
        self.update_scrollbar_hover(event.x, event.y);
        self.request_render(false);
    }

    // -------------------------------------------------------------------------
    // Scrollbar interactions (tui-alt-screen.ts:1079-1178)
    // -------------------------------------------------------------------------

    /// `getScrollbarTargetAt(x, y, includeHiddenAuto)` (tui-alt-screen.ts:1079-1094).
    fn get_scrollbar_target_at(
        &self,
        x: i64,
        y: i64,
        include_hidden_auto: bool,
    ) -> Option<(ScrollViewRef, ScrollbarGeometry)> {
        if TuiSurface::has_overlay(self) || self.current_layout.is_none() {
            return None;
        }
        let layout = self.current_layout.as_ref().expect("checked above");
        for scroll_view in get_scroll_views_at(layout, x, y) {
            let geometry = get_scroll_view_box(layout, &scroll_view)
                .and_then(|box_| get_scrollbar_geometry(box_, include_hidden_auto));
            if let Some(geometry) = geometry {
                if x == geometry.column
                    && y >= geometry.track_top
                    && y < geometry.track_top + geometry.track_height
                {
                    return Some((scroll_view, geometry));
                }
            }
        }
        None
    }

    /// `setScrollbarHover(scrollView)` (tui-alt-screen.ts:1096-1101).
    fn set_scrollbar_hover(&mut self, scroll_view: Option<ScrollViewRef>) {
        let unchanged = match (&self.scrollbar_hover, &scroll_view) {
            (Some(current), Some(next)) => Rc::ptr_eq(current, next),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return;
        }
        if let Some(previous) = self.scrollbar_hover.take() {
            previous.borrow_mut().set_scrollbar_active(false);
        }
        self.scrollbar_hover = scroll_view;
        if let Some(current) = &self.scrollbar_hover {
            current.borrow_mut().set_scrollbar_active(true);
        }
    }

    /// `updateScrollbarHover(x, y)` (tui-alt-screen.ts:1103-1105).
    fn update_scrollbar_hover(&mut self, x: i64, y: i64) {
        let target = self
            .get_scrollbar_target_at(x, y, true)
            .map(|(scroll_view, _)| scroll_view);
        self.set_scrollbar_hover(target);
    }

    /// `stopScrollbarHover()` (tui-alt-screen.ts:1107-1109).
    fn stop_scrollbar_hover(&mut self) {
        self.set_scrollbar_hover(None);
    }

    /// `scrollScrollbarToPointer(scrollView, geometry, pointerY, grabOffset)`
    /// (tui-alt-screen.ts:1111-1118).
    fn scroll_scrollbar_to_pointer(
        &self,
        scroll_view: &ScrollViewRef,
        geometry: &ScrollbarGeometry,
        pointer_y: i64,
        grab_offset: i64,
    ) {
        let max_thumb_offset = geometry.track_height - geometry.thumb_height;
        // `Math.max(0, Math.min(maxThumbOffset, pointerY - trackTop - grabOffset))`
        // — min first, then clamp at zero (maxThumbOffset is never negative
        // in practice; the order keeps TS parity even if it were).
        let thumb_offset = (pointer_y - geometry.track_top - grab_offset)
            .min(max_thumb_offset)
            .max(0);
        let scroll_top = if max_thumb_offset == 0 {
            0
        } else {
            ((thumb_offset as f64 / max_thumb_offset as f64)
                * geometry.max_scroll_top as f64)
                .round() as i64
        };
        scroll_view
            .borrow_mut()
            .scroll_to(scroll_top, ScrollViewScrollToOptions::default());
    }

    /// `handleScrollbarMouseEvent(event)` (tui-alt-screen.ts:1120-1169).
    fn handle_scrollbar_mouse_event(&mut self, event: &SgrMouseEvent) -> bool {
        if let Some(drag) = self.scrollbar_drag.clone() {
            if event.release {
                self.stop_scrollbar_drag();
                return true;
            }
            let geometry = self
                .current_layout
                .as_ref()
                .and_then(|layout| get_scroll_view_box(layout, &drag.scroll_view))
                .and_then(|box_| get_scrollbar_geometry(box_, false));
            if let Some(geometry) = geometry {
                self.scroll_scrollbar_to_pointer(
                    &drag.scroll_view,
                    &geometry,
                    event.y,
                    drag.grab_offset,
                );
            }
            return true;
        }

        if event.release || (event.button & 32) != 0 || (event.button & 3) != 0 {
            return false;
        }
        let Some((scroll_view, geometry)) = self.get_scrollbar_target_at(event.x, event.y, false)
        else {
            return false;
        };
        stop_selection_auto_scroll(self);
        self.selection_press_active = false;
        self.selection_anchor = None;
        self.selection_focus = None;
        self.selection_granularity = SelectionGranularity::Character;
        self.selection_initial_range = None;
        self.last_click = None;
        self.pressed_url = None;
        self.selection_dragged = false;
        self.set_scrollbar_hover(Some(scroll_view.clone()));
        let on_thumb = event.y >= geometry.thumb_top
            && event.y < geometry.thumb_top + geometry.thumb_height;
        let grab_offset = if on_thumb {
            event.y - geometry.thumb_top
        } else {
            geometry.thumb_height / 2
        };
        if !on_thumb {
            self.scroll_scrollbar_to_pointer(&scroll_view, &geometry, event.y, grab_offset);
        }
        self.scrollbar_drag = Some(ScrollbarDragState {
            scroll_view,
            grab_offset,
        });
        true
    }

    /// `stopScrollbarDrag()` (tui-alt-screen.ts:1171-1173).
    fn stop_scrollbar_drag(&mut self) {
        self.scrollbar_drag = None;
    }

    // -------------------------------------------------------------------------
    // Scroll API (tui-alt-screen.ts:490-509)
    // -------------------------------------------------------------------------

    /// `scrollBy(lines)` (tui-alt-screen.ts:490-493).
    pub fn scroll_by(&mut self, lines: i64) {
        self.get_primary_scroll_view().borrow_mut().scroll_by(lines);
        self.request_render(false);
    }

    /// `scrollToTop()` (tui-alt-screen.ts:495-498).
    pub fn scroll_to_top(&mut self) {
        self.get_primary_scroll_view().borrow_mut().scroll_to_start();
        self.request_render(false);
    }

    /// `scrollToBottom()` (tui-alt-screen.ts:500-503).
    pub fn scroll_to_bottom(&mut self) {
        self.get_primary_scroll_view().borrow_mut().scroll_to_end();
        self.request_render(false);
    }

    /// `scrollToPrompt(direction)` (tui-alt-screen.ts:505-520): jump to the
    /// previous (`-1`) or next (`1`) OSC 133 prompt-start marker in the
    /// primary scroll view's content.
    pub fn scroll_to_prompt(&mut self, direction: i64) {
        let Some(layout) = self.current_layout.clone() else {
            return;
        };
        let scroll_view = self.get_primary_scroll_view();
        let lines = get_scroll_view_box(&layout, &scroll_view)
            .and_then(|box_| box_.scroll_content_lines.clone());
        let Some(lines) = lines else {
            return;
        };
        let mut row = scroll_view.borrow().scroll_top() + direction;
        while row >= 0 && row < lines.len() as i64 {
            if starts_with_osc133_prompt_start(lines.get(row as usize).map(String::as_str).unwrap_or("")) {
                scroll_view
                    .borrow_mut()
                    .scroll_to(row, ScrollViewScrollToOptions::default());
                self.request_render(false);
                return;
            }
            row += direction;
        }
    }

    // -------------------------------------------------------------------------
    // Search control flow (tui-alt-screen.ts:522-663)
    // -------------------------------------------------------------------------

    /// `toggleSearch()` (tui-alt-screen.ts:522-544).
    pub(crate) fn toggle_search(&mut self) {
        if self.active_search.is_some() {
            self.close_search();
            return;
        }
        let tui_id = self.base.tui_id;
        let navigation_style = self.search_navigation_button_style.clone();
        // `(query) => this.updateSearchQuery(query)` — applied via the
        // parked-query bridge (see module docs).
        let on_query_change: OnQueryChange = Box::new(move |query: &str| {
            park_search_query(tui_id, query.to_string());
            park_render_request(tui_id);
        });
        let navigation_button_style: NavigationButtonStyle =
            Box::new(move |text: &str, hovered: bool| navigation_style(text, hovered));
        let component = Rc::new(RefCell::new(
            AltScreenSearchComponent::with_navigation_button_style(
                on_query_change,
                navigation_button_style,
            ),
        ));
        let anchor_row = self.get_primary_scroll_view().borrow().scroll_top();
        let search = ActiveSearch {
            component: component.clone(),
            index: AltScreenSearchIndex::new(),
            overlay: None,
            query: String::new(),
            matches: Vec::new(),
            selected_index: -1,
            selected_key: None,
            anchor_row,
            selection_mode: SearchSelectionMode::Query,
        };
        self.active_search = Some(search);
        let component_ref: ComponentRef = component;
        let overlay = TuiSurface::show_overlay(
            self,
            component_ref,
            OverlayOptions {
                anchor: Some(OverlayAnchor::TopRight),
                width: Some(SizeValue::Percent("40%".to_string())),
                min_width: Some(32),
                margin: Some(OverlayMarginOrNumber::Number(1)),
                ..OverlayOptions::default()
            },
        );
        self.active_search.as_mut().expect("just set").overlay = Some(overlay);
    }

    /// `closeSearch()` (tui-alt-screen.ts:546-552).
    pub(crate) fn close_search(&mut self) {
        let Some(search) = self.active_search.take() else {
            return;
        };
        // `search.overlay?.hide()` — via the entry id to avoid the handle's
        // re-entrant screen borrow (see module docs).
        let component: ComponentRef = search.component.clone();
        let entry_id = self
            .base
            .overlay_stack
            .iter()
            .find(|entry| Rc::ptr_eq(&entry.component, &component))
            .map(|entry| entry.entry_id);
        if let Some(entry_id) = entry_id {
            TuiSurface::hide_overlay_entry(self, entry_id);
        }
        self.request_render(false);
    }

    /// `updateSearchQuery(query)` (tui-alt-screen.ts:554-565).
    pub(crate) fn update_search_query(&mut self, query: &str) {
        // A direct call supersedes any query parked by the component's
        // `onQueryChange` closure.
        clear_pending_search_query(self.base.tui_id);
        let primary_top = self.get_primary_scroll_view().borrow().scroll_top();
        let Some(search) = self.active_search.as_mut() else {
            return;
        };
        if query == search.query {
            return;
        }
        // `search.anchorRow = selected?.segments[0]?.row ?? scrollTop`.
        // JS `search.matches[search.selectedIndex]` with a negative index
        // is `undefined` — it must NOT fall back to `matches[0]`.
        let anchor_row = if search.selected_index >= 0 {
            search
                .matches
                .get(search.selected_index as usize)
                .and_then(|selected| selected.segments.first())
                .map(|segment| segment.row as i64)
        } else {
            None
        }
        .unwrap_or(primary_top);
        search.anchor_row = anchor_row;
        search.query = query.to_string();
        search.selection_mode = SearchSelectionMode::Query;
        search.component.borrow_mut().set_result(-1, 0);
        self.request_render(false);
    }

    /// `navigateSearch(direction)` (tui-alt-screen.ts:567-572).
    pub(crate) fn navigate_search(&mut self, direction: i64) {
        // Drain any query typed since the last read (TS applied it
        // synchronously via the closure).
        apply_pending_search_query(self);
        let Some(search) = self.active_search.as_mut() else {
            return;
        };
        if search.query.is_empty() {
            return;
        }
        search.selection_mode = if direction < 0 {
            SearchSelectionMode::Previous
        } else {
            SearchSelectionMode::Next
        };
        self.request_render(false);
    }

    /// `getSearchNavigationDirectionAt(x, y)` (tui-alt-screen.ts:574-583).
    fn get_search_navigation_direction_at(&self, x: i64, y: i64) -> Option<i64> {
        let search = self.active_search.as_ref()?;
        let bounds = active_search_overlay_bounds(self)?;
        if x < bounds.col
            || x >= bounds.col + bounds.width
            || y < bounds.row
            || y >= bounds.row + bounds.height
        {
            return None;
        }
        search
            .component
            .borrow()
            .get_navigation_direction_at(y - bounds.row, x - bounds.col)
    }

    /// `handleSearchMouseEvent(event)` (tui-alt-screen.ts:585-595).
    fn handle_search_mouse_event(&mut self, event: &SgrMouseEvent) -> bool {
        if self.active_search.is_none() {
            return false;
        }
        let direction = self.get_search_navigation_direction_at(event.x, event.y);
        let hovered_changed = self
            .active_search
            .as_mut()
            .map(|search| {
                search
                    .component
                    .borrow_mut()
                    .set_hovered_navigation_direction(direction)
            })
            .unwrap_or(false);
        if hovered_changed {
            self.request_render(false);
        }
        let Some(direction) = direction else {
            return false;
        };
        if event.release || (event.button & 32) != 0 || (event.button & 3) != 0 {
            return false;
        }
        self.navigate_search(direction);
        true
    }

    /// `refreshSearch(layout)` (tui-alt-screen.ts:595-663).
    fn refresh_search_impl(&mut self, layout: &LayoutFrame) -> bool {
        apply_pending_search_query(self);
        let scroll_view = layout
            .primary_scroll_view
            .clone()
            .unwrap_or_else(|| self.implicit_scroll_view_state.clone());
        let lines = get_scroll_view_box(layout, &scroll_view)
            .and_then(|box_| box_.scroll_content_lines.clone());
        let has_box = get_scroll_view_box(layout, &scroll_view).is_some();
        let Some(search) = self.active_search.as_mut() else {
            return false;
        };
        if lines.is_none() || search.query.trim().is_empty() {
            search.matches.clear();
            search.selected_index = -1;
            search.selected_key = None;
            search.selection_mode = SearchSelectionMode::Retain;
            search.component.borrow_mut().set_result(-1, 0);
            return false;
        }
        let lines = lines.expect("checked above");

        let should_reveal_selection = search.selection_mode != SearchSelectionMode::Retain;
        let result = search.index.search(&lines, &search.query);
        let matches = result.matches;
        search.matches = matches.clone();
        if !result.changed && search.selection_mode == SearchSelectionMode::Retain {
            return false;
        }

        let exact_index = if result.changed {
            match &search.selected_key {
                Some(key) => matches
                    .iter()
                    .position(|candidate| &get_alt_screen_search_match_key(candidate) == key)
                    .map(|index| index as i64)
                    .unwrap_or(-1),
                None => -1,
            }
        } else {
            search.selected_index
        };
        let mut selected_index: i64 = -1;
        if !matches.is_empty() {
            selected_index = match search.selection_mode {
                SearchSelectionMode::Query => {
                    // Binary search for the first match at or after the
                    // anchor row (tui-alt-screen.ts:623-633).
                    let mut low: i64 = 0;
                    let mut high: i64 = matches.len() as i64;
                    while low < high {
                        let middle = low + (high - low) / 2;
                        let row = matches[middle as usize]
                            .segments
                            .first()
                            .map(|segment| segment.row as i64)
                            .unwrap_or(0);
                        if row < search.anchor_row {
                            low = middle + 1;
                        } else {
                            high = middle;
                        }
                    }
                    if low < matches.len() as i64 {
                        low
                    } else {
                        0
                    }
                }
                SearchSelectionMode::Next => {
                    let base_index = if exact_index >= 0 {
                        exact_index
                    } else {
                        search.selected_index.min(matches.len() as i64 - 1)
                    };
                    if base_index < 0 {
                        0
                    } else {
                        (base_index + 1) % matches.len() as i64
                    }
                }
                SearchSelectionMode::Previous => {
                    let base_index = if exact_index >= 0 {
                        exact_index
                    } else {
                        search.selected_index.min(matches.len() as i64 - 1)
                    };
                    if base_index < 0 {
                        matches.len() as i64 - 1
                    } else {
                        (base_index - 1 + matches.len() as i64) % matches.len() as i64
                    }
                }
                SearchSelectionMode::Retain => {
                    if exact_index >= 0 {
                        exact_index
                    } else {
                        search.selected_index.clamp(0, matches.len() as i64 - 1)
                    }
                }
            };
        }

        search.selected_index = selected_index;
        search.selected_key = if selected_index >= 0 {
            Some(get_alt_screen_search_match_key(
                &matches[selected_index as usize],
            ))
        } else {
            None
        };
        search.selection_mode = SearchSelectionMode::Retain;
        search
            .component
            .borrow_mut()
            .set_result(selected_index, matches.len() as i64);
        if !should_reveal_selection {
            return false;
        }

        // Reveal the selected match (tui-alt-screen.ts:650-663). JS
        // `matches[selectedIndex]` with a negative index is `undefined`.
        let selected = if selected_index >= 0 {
            matches.get(selected_index as usize)
        } else {
            None
        };
        let first_segment = selected.and_then(|selected| selected.segments.first());
        let last_segment = selected.and_then(|selected| selected.segments.last());
        let viewport_height = scroll_view.borrow().viewport_height();
        if !has_box || first_segment.is_none() || last_segment.is_none() || viewport_height <= 0 {
            return false;
        }
        let first_segment = first_segment.expect("checked above");
        let last_segment = last_segment.expect("checked above");
        let before = scroll_view.borrow().scroll_top();
        let visible_bottom = before + viewport_height - 1;
        let mut target = before;
        if (first_segment.row as i64) < before || (last_segment.row as i64) > visible_bottom {
            target = first_segment.row as i64 - viewport_height / 3;
        }
        scroll_view
            .borrow_mut()
            .scroll_to(target, ScrollViewScrollToOptions { disable_follow: true });
        let scrolled = scroll_view.borrow().scroll_top() != before;
        scrolled
    }

    /// Test/harness seam: the scroll-to-end indicator hit rect (set by the
    /// D-B3 compositing pass; consumed by
    /// [`Self::handle_scroll_to_end_indicator_mouse_event`]).
    #[cfg(test)]
    pub(crate) fn set_scroll_to_end_indicator_rect(
        &mut self,
        rect: Option<crate::tui_alt_screen::ScrollToEndIndicatorRect>,
    ) {
        self.scroll_to_end_indicator_rect = rect;
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{ComponentMouseResult, TuiMouseEventResult};
    use crate::keybindings::{
        set_keybindings, KeybindingsConfig, KeybindingsManager, tui_keybinding_definitions,
    };
    use crate::terminal::{InputHandler, ProgramStatus, ResizeHandler, Terminal};
    use crate::test_support::global_test_lock;
    use crate::tui_alt_screen::{ScrollToEndIndicatorRect, TuiAltScreenOptions};
    use std::cell::Cell;

    // -------------------------------------------------------------------------
    // Test doubles
    // -------------------------------------------------------------------------

    /// Recording terminal double (pattern from tui_overlay.rs /
    /// tui_alt_screen.rs tests).
    struct RecordingTerminal {
        cols: Cell<usize>,
        rows: Cell<usize>,
        log: Rc<RefCell<Vec<String>>>,
    }

    impl Terminal for RecordingTerminal {
        fn start(&mut self, _on_input: InputHandler, _on_resize: ResizeHandler) {}
        fn stop(&mut self) {}
        fn drain_input(&mut self, _max_ms: u64, _idle_ms: u64) {}
        fn write(&mut self, data: &str) {
            self.log.borrow_mut().push(data.to_string());
        }
        fn columns(&self) -> usize {
            self.cols.get()
        }
        fn rows(&self) -> usize {
            self.rows.get()
        }
        fn kitty_protocol_active(&self) -> bool {
            false
        }
        fn move_by(&mut self, _lines: i32) {}
        fn hide_cursor(&mut self) {}
        fn show_cursor(&mut self) {}
        fn clear_line(&mut self) {}
        fn clear_from_cursor(&mut self) {}
        fn clear_screen(&mut self) {}
        fn set_title(&mut self, _title: &str) {}
        fn set_progress(&mut self, _active: bool) {}
        fn set_program_status(&mut self, _status: ProgramStatus) {}
    }

    /// A plain multi-line document component.
    struct Lines {
        lines: Vec<String>,
    }

    impl Component for Lines {
        fn render(&mut self, _width: usize) -> Vec<String> {
            self.lines.clone()
        }
    }

    /// A mouse-capturing leaf that records every dispatched event.
    struct ClickSink {
        events: Rc<RefCell<Vec<(String, Option<i64>)>>>,
    }

    impl Component for ClickSink {
        fn render(&mut self, width: usize) -> Vec<String> {
            vec!["sink".to_string(); 1.max(width)]
                .into_iter()
                .map(|_| "sink".to_string())
                .collect()
        }
        fn handle_mouse(
            &mut self,
            _self_ref: &ComponentRef,
            event: &TuiMouseEvent,
        ) -> Option<ComponentMouseResult> {
            self.events
                .borrow_mut()
                .push((event.event_type.as_str().to_string(), event.click_count));
            Some(ComponentMouseResult::Result(TuiMouseEventResult {
                handled: Some(true),
                ..TuiMouseEventResult::default()
            }))
        }
    }

    fn new_screen(
        cols: usize,
        rows: usize,
        options: TuiAltScreenOptions,
    ) -> (Rc<RefCell<TuiAltScreen>>, Rc<RefCell<Vec<String>>>) {
        let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let terminal: Rc<RefCell<dyn Terminal>> = Rc::new(RefCell::new(RecordingTerminal {
            cols: Cell::new(cols),
            rows: Cell::new(rows),
            log: log.clone(),
        }));
        let screen = TuiAltScreen::new(terminal, None, None, options);
        screen.borrow_mut().alt_screen_active = true;
        (screen, log)
    }

    fn plain_screen(cols: usize, rows: usize) -> (Rc<RefCell<TuiAltScreen>>, Rc<RefCell<Vec<String>>>) {
        new_screen(cols, rows, TuiAltScreenOptions::default())
    }

    /// A screen whose implicit scroll view holds `count` content lines.
    fn tall_screen(cols: usize, rows: usize, count: usize) -> Rc<RefCell<TuiAltScreen>> {
        let (screen, _log) = plain_screen(cols, rows);
        let lines: Vec<String> = (0..count).map(|index| format!("line {index}")).collect();
        screen
            .borrow_mut()
            .add_child(Rc::new(RefCell::new(Lines { lines })));
        screen.borrow_mut().render_now(true);
        screen
    }

    /// Scoped environment override: restores the previous values on drop.
    /// Must be used under `global_test_lock()`.
    struct EnvRestore {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    fn set_env(vars: &[(&'static str, Option<&str>)]) -> EnvRestore {
        let saved = vars
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in vars {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        EnvRestore { saved }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn feed(screen: &Rc<RefCell<TuiAltScreen>>, data: &str) -> Option<TuiInputListenerResult> {
        handle_viewport_input(&mut screen.borrow_mut(), data)
    }

    // -------------------------------------------------------------------------
    // SGR mouse / wheel parsing
    // -------------------------------------------------------------------------

    #[test]
    fn parse_sgr_mouse_event_round_trips() {
        // `\x1b[<0;10;5M` — left press at column 9, row 4 (1-based input).
        let event = parse_sgr_mouse_event("\x1b[<0;10;5M").expect("parses");
        assert_eq!(
            event,
            SgrMouseEvent { button: 0, x: 9, y: 4, release: false }
        );
        // Release uses a lowercase final byte.
        let event = parse_sgr_mouse_event("\x1b[<0;10;5m").expect("parses");
        assert!(event.release);
        // Motion + modifiers: button 35 = 32 (motion) | 3 (no button).
        let event = parse_sgr_mouse_event("\x1b[<35;1;1M").expect("parses");
        assert_eq!(event.button, 35);
        assert_eq!((event.x, event.y), (0, 0));
        // Malformed: wrong prefix, trailing junk, missing params,
        // non-digit params, other final bytes, empty params.
        assert!(parse_sgr_mouse_event("\x1b[>0;1;1M").is_none());
        assert!(parse_sgr_mouse_event("\x1b[<0;1;1MX").is_none());
        assert!(parse_sgr_mouse_event("\x1b[<0;1M").is_none());
        assert!(parse_sgr_mouse_event("\x1b[<0;1;1;1M").is_none());
        assert!(parse_sgr_mouse_event("\x1b[<0;a;1M").is_none());
        assert!(parse_sgr_mouse_event("\x1b[<;;M").is_none());
        assert!(parse_sgr_mouse_event("\x1b[<0;1;1A").is_none());
        assert!(parse_sgr_mouse_event("").is_none());
        assert!(parse_sgr_mouse_event("plain").is_none());
        // Coordinate 0 is preserved as -1 (TS `parseInt(...) - 1`).
        let event = parse_sgr_mouse_event("\x1b[<0;0;0M").expect("parses");
        assert_eq!((event.x, event.y), (-1, -1));
    }

    #[test]
    fn parse_wheel_event_decodes_sgr_and_legacy_forms() {
        // SGR wheel up: button 64.
        let event = parse_wheel_event("\x1b[<64;10;5M").expect("parses");
        assert_eq!(
            event,
            WheelEvent { direction: -1, x: 9, y: 4, button: 64 }
        );
        // SGR wheel down: button 65.
        let event = parse_wheel_event("\x1b[<65;1;1m").expect("parses");
        assert_eq!(event.direction, 1);
        assert_eq!(event.button, 65);
        // Alt+wheel up: button 72 = 64 | 8.
        let event = parse_wheel_event("\x1b[<72;1;1M").expect("parses");
        assert_eq!(event.button, 72);
        assert_eq!(event.direction, -1);
        // Non-wheel buttons (no bit 6) and non-vertical wheels are rejected.
        assert!(parse_wheel_event("\x1b[<0;1;1M").is_none());
        assert!(parse_wheel_event("\x1b[<66;1;1M").is_none()); // 64 | 2
        // Legacy 6-byte form: `\x1b[M` + (32 + 64) + (33 + x) + (33 + y).
        let event = parse_wheel_event("\x1b[M`##").expect("parses");
        assert_eq!(event.direction, -1);
        assert_eq!((event.x, event.y), (2, 2));
        assert_eq!(event.button, 64);
        // Legacy wheel down: 32 + 65 = 0x61 ('a').
        let event = parse_wheel_event("\x1b[Ma##").expect("parses");
        assert_eq!(event.direction, 1);
        // Legacy non-wheel press: 32 + 0 = space.
        assert!(parse_wheel_event("\x1b[M ##").is_none());
        // Wrong length / wrong prefix.
        assert!(parse_wheel_event("\x1b[M`#").is_none());
        assert!(parse_wheel_event("\x1b[N`##").is_none());
        assert!(parse_wheel_event("x").is_none());
    }

    #[test]
    fn is_mouse_sequence_accepts_known_forms() {
        assert!(is_mouse_sequence("\x1b[<0;1;1M"));
        assert!(is_mouse_sequence("\x1b[<0;1;1m"));
        assert!(is_mouse_sequence("\x1b[M`##"));
        assert!(is_mouse_sequence("\x1b[M ##"));
        assert!(!is_mouse_sequence("\x1b[<0;1;1A"));
        assert!(!is_mouse_sequence("\x1b[5~"));
        assert!(!is_mouse_sequence("q"));
        assert!(!is_mouse_sequence("\x1b[M`###"));
    }

    #[test]
    fn decode_mouse_button_table() {
        assert_eq!(decode_mouse_button(0), TuiMouseButton::Left);
        assert_eq!(decode_mouse_button(1), TuiMouseButton::Middle);
        assert_eq!(decode_mouse_button(2), TuiMouseButton::Right);
        assert_eq!(decode_mouse_button(3), TuiMouseButton::None);
        // Modifier bits (shift=4, alt=8, ctrl=16) do not change the button.
        assert_eq!(decode_mouse_button(4), TuiMouseButton::Left);
        assert_eq!(decode_mouse_button(12), TuiMouseButton::Left);
        assert_eq!(decode_mouse_button(16 | 1), TuiMouseButton::Middle);
        // Motion (32) still decodes through the low two bits: 32 & 3 == 0.
        assert_eq!(decode_mouse_button(32), TuiMouseButton::Left);
        assert_eq!(decode_mouse_button(33), TuiMouseButton::Middle);
        // Wheel bits (64) are outside the low two bits.
        assert_eq!(decode_mouse_button(64), TuiMouseButton::Left);
    }

    // -------------------------------------------------------------------------
    // Wheel routing / scroll keybindings
    // -------------------------------------------------------------------------

    #[test]
    fn wheel_events_scroll_the_primary_view() {
        let _guard = global_test_lock();
        let screen = tall_screen(20, 10, 50);
        // Following end: scroll top = 40.
        assert_eq!(screen.borrow().viewport_top(), 40);
        // Wheel up (button 64) at (5, 3): 1 line.
        let result = feed(&screen, "\x1b[<64;6;4M");
        assert!(result.expect("consumed").consume == Some(true));
        assert_eq!(screen.borrow().viewport_top(), 39);
        // Alt+wheel up (button 72 = 64 | 8): 5 lines.
        feed(&screen, "\x1b[<72;6;4M");
        assert_eq!(screen.borrow().viewport_top(), 34);
        // Wheel down (button 65) at the middle: clamped at the end.
        for _ in 0..10 {
            feed(&screen, "\x1b[<65;6;4M");
        }
        assert_eq!(screen.borrow().viewport_top(), 40);
    }

    #[test]
    fn page_keybindings_use_the_overlap_math() {
        let _guard = global_test_lock();
        let screen = tall_screen(20, 10, 50);
        assert_eq!(screen.borrow().viewport_top(), 40);
        // pageUp: -(viewportHeight - PAGE_SCROLL_OVERLAP) = -(10 - 4).
        let result = feed(&screen, "\x1b[5~");
        assert!(result.expect("consumed").consume == Some(true));
        assert_eq!(screen.borrow().viewport_top(), 34);
        // pageDown from the middle: +6, clamped to the end.
        feed(&screen, "\x1b[6~");
        assert_eq!(screen.borrow().viewport_top(), 40);
        // At the top edge, pageUp clamps to 0.
        screen.borrow_mut().scroll_to_top();
        feed(&screen, "\x1b[5~");
        assert_eq!(screen.borrow().viewport_top(), 0);
        // Tiny viewport: max(1, height - overlap) keeps one line.
        let small = tall_screen(20, 4, 50);
        feed(&small, "\x1b[5~");
        assert_eq!(small.borrow().viewport_top(), 45);
    }

    #[test]
    fn line_and_top_bottom_keybindings() {
        let _guard = global_test_lock();
        let screen = tall_screen(20, 10, 50);
        feed(&screen, "\x1b[<64;6;4M"); // wheel up 1 → 39
        assert_eq!(screen.borrow().viewport_top(), 39);
        screen.borrow_mut().scroll_to_top();
        assert_eq!(screen.borrow().viewport_top(), 0);
        screen.borrow_mut().scroll_to_bottom();
        assert_eq!(screen.borrow().viewport_top(), 40);
        assert!(screen.borrow().is_following_output());
        screen.borrow_mut().scroll_to_top();
        assert!(!screen.borrow().is_following_output());
    }

    #[test]
    fn scroll_to_prompt_finds_osc133_markers() {
        let _guard = global_test_lock();
        let (screen, _log) = plain_screen(20, 10);
        let mut lines: Vec<String> = Vec::new();
        for index in 0..30 {
            if index == 5 || index == 22 {
                lines.push(format!("\x1b]133;A\x07prompt {index}"));
            } else {
                lines.push(format!("output {index}"));
            }
        }
        screen
            .borrow_mut()
            .add_child(Rc::new(RefCell::new(Lines { lines })));
        screen.borrow_mut().render_now(true);
        // From the end (top 20): previous prompt is row 5.
        screen.borrow_mut().scroll_to_prompt(-1);
        assert_eq!(screen.borrow().viewport_top(), 5);
        // Next prompt forward from row 5 is row 22, but scrollTo clamps to
        // maxScrollTop = 30 - 10 = 20 (TS probe: viewportTop 20).
        screen.borrow_mut().scroll_to_prompt(1);
        assert_eq!(screen.borrow().viewport_top(), 20);
        // From the bottom (top 20) the next forward prompt also clamps to 20.
        screen.borrow_mut().scroll_to_bottom();
        assert_eq!(screen.borrow().viewport_top(), 20);
        screen.borrow_mut().scroll_to_prompt(1);
        assert_eq!(screen.borrow().viewport_top(), 20);
        // No marker past the last one: no move.
        screen.borrow_mut().scroll_to_prompt(1);
        assert_eq!(screen.borrow().viewport_top(), 20);
        // No marker above the first one either.
        screen.borrow_mut().scroll_to_top();
        assert_eq!(screen.borrow().viewport_top(), 0);
        screen.borrow_mut().scroll_to_prompt(-1);
        assert_eq!(screen.borrow().viewport_top(), 0);
    }

    // -------------------------------------------------------------------------
    // Lifecycle ESC sequences
    // -------------------------------------------------------------------------

    #[test]
    fn before_terminal_start_writes_the_enter_sequence() {
        let _guard = global_test_lock();
        let _env = set_env(&[
            ("TERM", Some("xterm-256color")),
            ("TMUX", None),
            ("ZELLIJ", None),
            ("STY", None),
        ]);
        let (screen, log) = plain_screen(20, 5);
        before_terminal_start(&mut screen.borrow_mut());
        assert_eq!(
            log.borrow().last().map(String::as_str),
            Some(concat!(
                "\x1b[?1049h",
                "\x1b[?7l",
                "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1004h\x1b[?1006h",
                "\x1b[2J\x1b[H\x1b[?25l",
            ))
        );
        assert!(screen.borrow().alt_screen_active);
    }

    #[test]
    fn before_terminal_start_uses_button_motion_under_multiplexers() {
        let _guard = global_test_lock();
        let _env = set_env(&[
            ("TERM", Some("xterm")),
            ("TMUX", Some("")),
            ("ZELLIJ", None),
            ("STY", None),
        ]);
        let (screen, log) = plain_screen(20, 5);
        before_terminal_start(&mut screen.borrow_mut());
        let written = log.borrow().last().cloned().expect("written");
        assert!(written.contains(ENABLE_BUTTON_MOTION_MOUSE));
        assert!(!written.contains("\x1b[?1003h"));
        // TERM starting with "screen" also selects button-motion.
        let _env = set_env(&[
            ("TERM", Some("screen-256color")),
            ("TMUX", None),
            ("ZELLIJ", None),
            ("STY", None),
        ]);
        let (screen, log) = plain_screen(20, 5);
        before_terminal_start(&mut screen.borrow_mut());
        assert!(log.borrow().last().expect("written").contains(ENABLE_BUTTON_MOTION_MOUSE));
    }

    #[test]
    fn before_terminal_start_skips_mouse_mode_when_disabled() {
        let _guard = global_test_lock();
        let _env = set_env(&[
            ("TERM", Some("xterm")),
            ("TMUX", None),
            ("ZELLIJ", None),
            ("STY", None),
        ]);
        let (screen, log) = new_screen(20, 5, TuiAltScreenOptions { mouse: Some(false), ..Default::default() });
        before_terminal_start(&mut screen.borrow_mut());
        let written = log.borrow().last().cloned().expect("written");
        assert!(!written.contains("\x1b[?1000h"));
        assert!(written.contains(ENTER_ALT_SCREEN));
    }

    #[test]
    fn before_terminal_stop_writes_the_exit_sequence_and_clears_state() {
        let _guard = global_test_lock();
        let previous = terminal_image::get_capabilities();
        terminal_image::set_capabilities(TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            ..previous
        });
        let (screen, log) = plain_screen(20, 5);
        {
            let mut borrowed = screen.borrow_mut();
            before_terminal_start(&mut borrowed);
            borrowed.uploaded_kitty_images.insert(
                7,
                crate::tui_alt_screen::CachedKittyImage {
                    transmission_generation: 1,
                    transmission_bytes: 10,
                    estimated_decoded_bytes: 20,
                },
            );
        }
        log.borrow_mut().clear();
        before_terminal_stop(&mut screen.borrow_mut(), &TuiStopOptions::default());
        // kitty protocol → the all-images deletion escape is included.
        assert_eq!(
            log.borrow().last().map(String::as_str),
            Some(concat!(
                "\x1b[?2026h",
                "\x1b_Ga=d,d=I,q=2\x1b\\",
                "\x1b[?1006l\x1b[?1004l\x1b[?1003l\x1b[?1002l\x1b[?1000l",
                "\x1b[?7h",
                "\x1b[?2026l",
            ))
        );
        assert!(screen.borrow().uploaded_kitty_images.is_empty());
        terminal_image::set_capabilities(previous);
    }

    #[test]
    fn before_terminal_stop_is_a_noop_when_alt_screen_inactive() {
        let _guard = global_test_lock();
        let (screen, log) = plain_screen(20, 5);
        screen.borrow_mut().alt_screen_active = false;
        before_terminal_stop(&mut screen.borrow_mut(), &TuiStopOptions::default());
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn after_terminal_stop_preserve_screen_writes_the_short_exit() {
        let _guard = global_test_lock();
        let (screen, log) = plain_screen(20, 5);
        {
            let mut borrowed = screen.borrow_mut();
            before_terminal_start(&mut borrowed);
        }
        log.borrow_mut().clear();
        after_terminal_stop(
            &mut screen.borrow_mut(),
            &TuiStopOptions { preserve_screen: true },
        );
        assert_eq!(
            log.borrow().last().map(String::as_str),
            Some("\x1b[?2026h\x1b[?1049l\x1b[?25h\x1b[?2026l")
        );
        assert!(!screen.borrow().alt_screen_active);
    }

    #[test]
    fn after_terminal_stop_dumps_the_document_without_preserve() {
        let _guard = global_test_lock();
        let (screen, log) = plain_screen(20, 3);
        screen
            .borrow_mut()
            .add_child(Rc::new(RefCell::new(Lines { lines: vec!["hello".to_string()] })));
        {
            let mut borrowed = screen.borrow_mut();
            before_terminal_start(&mut borrowed);
        }
        log.borrow_mut().clear();
        after_terminal_stop(&mut screen.borrow_mut(), &TuiStopOptions::default());
        let written = log.borrow().last().expect("written").clone();
        let expected_line = "hello\x1b[0m\x1b]8;;\x07";
        assert_eq!(
            written,
            format!(
                "\x1b[?2026h\x1b[?1049l\x1b[?7l\r\x1b[2K{expected_line}\x1b[0m\x1b[?7h\r\n\x1b[?25h\x1b[?2026l"
            )
        );
        // The document survives for the next TUI (getLastDocument).
        assert_eq!(screen.borrow().last_document, vec![expected_line.to_string()]);
    }

    #[test]
    fn iterm2_capabilities_are_masked_at_start_and_restored_at_stop() {
        let _guard = global_test_lock();
        let previous = terminal_image::get_capabilities();
        terminal_image::set_capabilities(TerminalCapabilities {
            images: Some(ImageProtocol::Iterm2),
            ..previous
        });
        let (screen, _log) = plain_screen(20, 5);
        {
            let mut borrowed = screen.borrow_mut();
            before_terminal_start(&mut borrowed);
            // iTerm2 image support is masked for the alt screen.
            assert_eq!(terminal_image::get_capabilities().images, None);
            assert!(borrowed.saved_capabilities.is_some());
        }
        before_terminal_stop(&mut screen.borrow_mut(), &TuiStopOptions::default());
        after_terminal_stop(
            &mut screen.borrow_mut(),
            &TuiStopOptions { preserve_screen: true },
        );
        // Restored after the exit sequence is written.
        assert_eq!(
            terminal_image::get_capabilities().images,
            Some(ImageProtocol::Iterm2)
        );
        assert!(screen.borrow().saved_capabilities.is_none());
        terminal_image::set_capabilities(previous);
    }

    #[test]
    fn after_terminal_start_is_a_noop() {
        let _guard = global_test_lock();
        let (screen, log) = plain_screen(20, 5);
        after_terminal_start(&mut screen.borrow_mut());
        assert!(log.borrow().is_empty());
    }

    // -------------------------------------------------------------------------
    // FOCUS_IN / FOCUS_OUT
    // -------------------------------------------------------------------------

    #[test]
    fn focus_events_are_consumed_and_focus_out_resets_gestures() {
        let _guard = global_test_lock();
        let screen = tall_screen(20, 10, 50);
        assert!(feed(&screen, FOCUS_IN).expect("consumed").consume == Some(true));
        {
            let mut borrowed = screen.borrow_mut();
            borrowed.selection_press_active = true;
            borrowed.selection_anchor = Some(crate::tui_alt_screen::SelectionPoint {
                row: 1,
                col: 1,
                scroll_view: None,
                boundary: false,
            });
            borrowed.selection_focus = Some(crate::tui_alt_screen::SelectionPoint {
                row: 2,
                col: 2,
                scroll_view: None,
                boundary: false,
            });
            borrowed.mouse_press_moved = true;
            borrowed.selection_dragged = true;
        }
        let result = feed(&screen, FOCUS_OUT);
        assert!(result.expect("consumed").consume == Some(true));
        let borrowed = screen.borrow();
        assert!(!borrowed.selection_press_active);
        assert!(borrowed.selection_anchor.is_none());
        assert!(borrowed.selection_focus.is_none());
        assert_eq!(borrowed.selection_granularity, SelectionGranularity::Character);
        assert!(borrowed.selection_initial_range.is_none());
        assert!(borrowed.last_click.is_none());
        assert!(!borrowed.selection_dragged);
        assert!(borrowed.mouse_press_target.is_none());
        assert!(borrowed.mouse_capture.is_none());
    }

    #[test]
    fn unknown_mouse_sequences_are_consumed_without_side_effects() {
        let _guard = global_test_lock();
        let screen = tall_screen(20, 10, 50);
        // A legacy motion sequence we do not decode: consumed, no scroll.
        let before = screen.borrow().viewport_top();
        assert!(feed(&screen, "\x1b[M ##").expect("consumed").consume == Some(true));
        assert_eq!(screen.borrow().viewport_top(), before);
    }

    // -------------------------------------------------------------------------
    // Component click counts
    // -------------------------------------------------------------------------

    #[test]
    fn component_clicks_cycle_the_multi_click_count() {
        let _guard = global_test_lock();
        crate::clock::reset_for_test();
        let (screen, _log) = plain_screen(20, 10);
        let events: Rc<RefCell<Vec<(String, Option<i64>)>>> = Rc::new(RefCell::new(Vec::new()));
        let sink: ComponentRef = Rc::new(RefCell::new(ClickSink { events: events.clone() }));
        screen.borrow_mut().set_layout_root(Some(sink.clone()));
        screen.borrow_mut().render_now(true);

        let press = "\x1b[<0;5;5M";
        let release = "\x1b[<0;5;5m";
        feed(&screen, press);
        feed(&screen, release);
        feed(&screen, press);
        feed(&screen, release);
        feed(&screen, press);
        feed(&screen, release);
        let clicks: Vec<Option<i64>> = events
            .borrow()
            .iter()
            .filter(|(kind, _)| kind == "click")
            .map(|(_, count)| *count)
            .collect();
        assert_eq!(clicks, vec![Some(1), Some(2), Some(3)]);
        // Moving the pointer between press and release suppresses the click.
        feed(&screen, "\x1b[<0;5;5M");
        feed(&screen, "\x1b[<32;6;5M"); // motion with left button → drag
        feed(&screen, "\x1b[<0;6;5m");
        let clicks: Vec<Option<i64>> = events
            .borrow()
            .iter()
            .filter(|(kind, _)| kind == "click")
            .map(|(_, count)| *count)
            .collect();
        assert_eq!(clicks, vec![Some(1), Some(2), Some(3)]);
        // After the double-click interval the count restarts at 1.
        crate::clock::set_now_ms(crate::clock::now_ms() + 600.0);
        feed(&screen, press);
        feed(&screen, release);
        let clicks: Vec<Option<i64>> = events
            .borrow()
            .iter()
            .filter(|(kind, _)| kind == "click")
            .map(|(_, count)| *count)
            .collect();
        assert_eq!(clicks, vec![Some(1), Some(2), Some(3), Some(1)]);
        // Press/release/drag/click dispatch order for the gesture above.
        let kinds: Vec<String> = events.borrow().iter().map(|(kind, _)| kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                "press", "release", "click", "press", "release", "click", "press", "release",
                "click", "press", "drag", "release", "press", "release", "click"
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
        );
    }

    // -------------------------------------------------------------------------
    // Scrollbar interactions
    // -------------------------------------------------------------------------

    /// A screen with a primary, always-visible-scrollbar scroll view as the
    /// layout root — mirrors the upstream "jumps to a scrollbar track
    /// position" test setup (tui-alt-screen.test.ts:481-507).
    fn scrollbar_screen(cols: usize, rows: usize, count: usize) -> Rc<RefCell<TuiAltScreen>> {
        let (screen, _log) = plain_screen(cols, rows);
        let lines: Vec<String> = (0..count).map(|index| format!("line {index}")).collect();
        let scroll_view = crate::components::scroll_view::ScrollView::new(
            Rc::new(RefCell::new(Lines { lines })) as ComponentRef,
            crate::components::scroll_view::ScrollViewOptions {
                primary: true,
                scrollbar: Some(crate::layout::ScrollViewScrollbar::Always),
                ..Default::default()
            },
        );
        screen
            .borrow_mut()
            .set_layout_root(Some(Rc::new(RefCell::new(scroll_view)) as ComponentRef));
        screen.borrow_mut().render_now(true);
        screen
    }

    #[test]
    fn scrollbar_press_jumps_and_drag_tracks_the_pointer() {
        let _guard = global_test_lock();
        // Layout: 10×10, content 50 lines. Scrollbar column 9; track rows
        // 0..10; thumbHeight = max(2, round(10*10/50)) = 2; maxScrollTop =
        // 40; maxThumbTop = 8. No follow: starts at scrollTop 0.
        let screen = scrollbar_screen(10, 10, 50);
        assert_eq!(screen.borrow().viewport_top(), 0);
        // Press on the track below the thumb (row 5): jumps so the thumb
        // centers on the pointer (thumbOffset = 5 - 0 - 1 = 4 → scrollTop
        // round(4/8 * 40) = 20) and starts a drag.
        feed(&screen, "\x1b[<0;10;6M"); // press at (9, 5)
        assert_eq!(screen.borrow().viewport_top(), 20);
        assert!(screen.borrow().scrollbar_drag.is_some());
        // Drag to row 9: thumbOffset = 9 - 1 = 8 → scrollTop = 40.
        feed(&screen, "\x1b[<32;10;10M"); // motion with left button
        assert_eq!(screen.borrow().viewport_top(), 40);
        // Release ends the drag.
        feed(&screen, "\x1b[<0;10;10m");
        assert!(screen.borrow().scrollbar_drag.is_none());
        assert_eq!(screen.borrow().viewport_top(), 40);
    }

    #[test]
    fn scrollbar_press_on_the_thumb_keeps_the_grab_offset() {
        let _guard = global_test_lock();
        let screen = scrollbar_screen(10, 10, 50);
        // Press directly on the thumb (rows 0..2 at scrollTop 0).
        feed(&screen, "\x1b[<0;10;2M"); // press at (9, 1) — on the thumb
        // No jump: the grab offset preserves the position.
        assert_eq!(screen.borrow().viewport_top(), 0);
        let drag = screen.borrow().scrollbar_drag.clone().expect("dragging");
        assert_eq!(drag.grab_offset, 1);
        // Drag to row 5: thumbOffset = 5 - 0 - 1 = 4 → scrollTop = 20.
        feed(&screen, "\x1b[<32;10;6M");
        assert_eq!(screen.borrow().viewport_top(), 20);
        // Drag past the end (row 9): thumbOffset 8 clamps → scrollTop 40.
        feed(&screen, "\x1b[<32;10;10M");
        assert_eq!(screen.borrow().viewport_top(), 40);
        feed(&screen, "\x1b[<0;10;10m");
        assert!(screen.borrow().scrollbar_drag.is_none());
    }

    #[test]
    fn scrollbar_interactions_clear_selection_state() {
        let _guard = global_test_lock();
        let screen = scrollbar_screen(10, 10, 50);
        {
            let mut borrowed = screen.borrow_mut();
            borrowed.selection_press_active = true;
            borrowed.selection_dragged = true;
        }
        feed(&screen, "\x1b[<0;10;6M");
        let borrowed = screen.borrow();
        assert!(!borrowed.selection_press_active);
        assert!(!borrowed.selection_dragged);
        assert!(borrowed.selection_anchor.is_none());
        assert!(borrowed.last_click.is_none());
    }

    // -------------------------------------------------------------------------
    // Scroll-to-end indicator
    // -------------------------------------------------------------------------

    #[test]
    fn scroll_to_end_indicator_press_jumps_to_the_bottom() {
        let _guard = global_test_lock();
        let screen = tall_screen(20, 10, 50);
        screen
            .borrow_mut()
            .set_scroll_to_end_indicator_rect(Some(ScrollToEndIndicatorRect {
                row: 9,
                column: 8,
                width: 4,
            }));
        screen.borrow_mut().scroll_to_top();
        assert_eq!(screen.borrow().viewport_top(), 0);
        // Press inside the rect (x in [8, 12), y == 9).
        feed(&screen, "\x1b[<0;10;10M");
        assert_eq!(screen.borrow().viewport_top(), 40);
        // Release and motion events inside the rect are ignored.
        screen.borrow_mut().scroll_to_top();
        feed(&screen, "\x1b[<0;10;10m");
        feed(&screen, "\x1b[<32;10;10M");
        assert_eq!(screen.borrow().viewport_top(), 0);
        // A press outside the rect does not jump (falls through to the
        // selection stub).
        feed(&screen, "\x1b[<0;5;10M");
        assert_eq!(screen.borrow().viewport_top(), 0);
    }

    // -------------------------------------------------------------------------
    // Search control flow
    // -------------------------------------------------------------------------

    #[test]
    fn toggle_search_opens_and_closes_the_overlay() {
        let _guard = global_test_lock();
        let screen = tall_screen(40, 10, 30);
        assert!(screen.borrow().active_search.is_none());
        screen.borrow_mut().toggle_search();
        {
            let borrowed = screen.borrow();
            let search = borrowed.active_search.as_ref().expect("search");
            assert_eq!(search.query, "");
            assert_eq!(search.selected_index, -1);
            assert_eq!(search.selection_mode, SearchSelectionMode::Query);
            assert_eq!(search.anchor_row, borrowed.viewport_top());
            assert_eq!(borrowed.base.overlay_stack.len(), 1);
            // The overlay captured keyboard focus.
            assert!(borrowed
                .base
                .focused_component
                .as_ref()
                .is_some_and(|focused| Rc::ptr_eq(
                    focused,
                    &borrowed.base.overlay_stack[0].component
                )));
        }
        // Toggling again closes it.
        screen.borrow_mut().toggle_search();
        assert!(screen.borrow().active_search.is_none());
        assert!(screen.borrow().base.overlay_stack.is_empty());
    }

    #[test]
    fn update_search_query_resets_selection_and_sets_the_anchor() {
        let _guard = global_test_lock();
        let screen = tall_screen(40, 10, 30);
        screen.borrow_mut().toggle_search();
        screen.borrow_mut().update_search_query("needle");
        {
            let borrowed = screen.borrow();
            let search = borrowed.active_search.as_ref().expect("search");
            assert_eq!(search.query, "needle");
            assert_eq!(search.selection_mode, SearchSelectionMode::Query);
            assert_eq!(search.anchor_row, 20); // scrollTop while following end
        }
        // Setting the same query is a no-op (selectionMode untouched).
        screen.borrow_mut().navigate_search(1);
        screen.borrow_mut().update_search_query("needle");
        assert_eq!(
            screen
                .borrow()
                .active_search
                .as_ref()
                .expect("search")
                .selection_mode,
            SearchSelectionMode::Next
        );
    }

    #[test]
    fn navigate_search_requires_a_query() {
        let _guard = global_test_lock();
        let screen = tall_screen(40, 10, 30);
        screen.borrow_mut().toggle_search();
        screen.borrow_mut().navigate_search(1);
        assert_eq!(
            screen
                .borrow()
                .active_search
                .as_ref()
                .expect("search")
                .selection_mode,
            SearchSelectionMode::Query
        );
        screen.borrow_mut().update_search_query("x");
        screen.borrow_mut().navigate_search(1);
        assert_eq!(
            screen
                .borrow()
                .active_search
                .as_ref()
                .expect("search")
                .selection_mode,
            SearchSelectionMode::Next
        );
        screen.borrow_mut().navigate_search(-1);
        assert_eq!(
            screen
                .borrow()
                .active_search
                .as_ref()
                .expect("search")
                .selection_mode,
            SearchSelectionMode::Previous
        );
    }

    #[test]
    fn refresh_search_finds_matches_reveals_and_returns_whether_it_scrolled() {
        let _guard = global_test_lock();
        let (screen, _log) = plain_screen(40, 10);
        let lines: Vec<String> = (0..100)
            .map(|index| {
                if index % 10 == 3 {
                    format!("needle {index}")
                } else {
                    format!("hay {index}")
                }
            })
            .collect();
        screen
            .borrow_mut()
            .add_child(Rc::new(RefCell::new(Lines { lines })));
        screen.borrow_mut().render_now(true);
        assert_eq!(screen.borrow().viewport_top(), 90);

        // No active search → false.
        let layout = screen.borrow().current_layout.clone().expect("layout");
        assert!(!refresh_search(&mut screen.borrow_mut(), &layout));

        screen.borrow_mut().toggle_search();
        screen.borrow_mut().update_search_query("needle");
        screen.borrow_mut().navigate_search(1); // selectionMode: Next
        let scrolled = refresh_search(&mut screen.borrow_mut(), &layout);
        {
            let borrowed = screen.borrow();
            let search = borrowed.active_search.as_ref().expect("search");
            assert_eq!(search.matches.len(), 10);
            // Next from selectedIndex -1 → 0 (tui-alt-screen.ts:639-641).
            assert_eq!(search.selected_index, 0);
            assert_eq!(search.selection_mode, SearchSelectionMode::Retain);
            assert!(search.selected_key.is_some());
            // The reveal scrolled the viewport to the first match (row 3):
            // target = 3 - floor(10 / 3) = 0.
            assert_eq!(borrowed.viewport_top(), 0);
        }
        assert!(scrolled);

        // Already revealed → false, selection preserved by key.
        let scrolled = refresh_search(&mut screen.borrow_mut(), &layout);
        assert!(!scrolled);
        assert_eq!(
            screen
                .borrow()
                .active_search
                .as_ref()
                .expect("search")
                .selected_index,
            0
        );

        // Navigate backwards: previous from index 0 wraps to the last match
        // (row 93) and reveals it.
        screen.borrow_mut().navigate_search(-1);
        let layout = screen.borrow().current_layout.clone().expect("layout");
        let scrolled = refresh_search(&mut screen.borrow_mut(), &layout);
        assert_eq!(
            screen
                .borrow()
                .active_search
                .as_ref()
                .expect("search")
                .selected_index,
            9
        );
        // target = 93 - 3 = 90.
        assert_eq!(screen.borrow().viewport_top(), 90);
        assert!(scrolled);
    }

    #[test]
    fn refresh_search_clears_state_for_blank_queries() {
        let _guard = global_test_lock();
        let screen = tall_screen(40, 10, 30);
        screen.borrow_mut().toggle_search();
        screen.borrow_mut().update_search_query("   ");
        let layout = screen.borrow().current_layout.clone().expect("layout");
        assert!(!refresh_search(&mut screen.borrow_mut(), &layout));
        let borrowed = screen.borrow();
        let search = borrowed.active_search.as_ref().expect("search");
        assert!(search.matches.is_empty());
        assert_eq!(search.selected_index, -1);
        assert_eq!(search.selection_mode, SearchSelectionMode::Retain);
    }

    #[test]
    fn search_keybindings_toggle_navigate_and_close() {
        let _guard = global_test_lock();
        // Bind the search toggles to deterministic sequences for this test.
        set_keybindings(KeybindingsManager::new(
            tui_keybinding_definitions(),
            KeybindingsConfig {
                tui_alt_screen_search: Some(vec!["ctrl+f".to_string()]),
                tui_alt_screen_search_next: Some(vec!["ctrl+n".to_string()]),
                tui_alt_screen_search_close: Some(vec!["escape".to_string()]),
                ..KeybindingsConfig::default()
            },
        ));
        let screen = tall_screen(40, 10, 30);
        // ctrl+f opens the search; the overlay captures focus.
        feed(&screen, "\x06");
        assert!(screen.borrow().active_search.is_some());
        let search_overlay_focused = active_search_entry_id(&screen.borrow())
            .map(|entry_id| screen.borrow().base.is_overlay_entry_focused(entry_id))
            .unwrap_or(false);
        assert!(search_overlay_focused);
        // While the search overlay is focused, searchNext navigates.
        screen.borrow_mut().update_search_query("line");
        feed(&screen, "\x0e"); // ctrl+n
        assert_eq!(
            screen
                .borrow()
                .active_search
                .as_ref()
                .expect("search")
                .selection_mode,
            SearchSelectionMode::Next
        );
        // Escape closes the search.
        feed(&screen, "\x1b");
        assert!(screen.borrow().active_search.is_none());
        // ctrl+f toggles open again, and closes on the second press.
        feed(&screen, "\x06");
        assert!(screen.borrow().active_search.is_some());
        feed(&screen, "\x06");
        assert!(screen.borrow().active_search.is_none());
        // Restore the default global keybindings.
        set_keybindings(KeybindingsManager::new(
            tui_keybinding_definitions(),
            KeybindingsConfig::default(),
        ));
    }

    #[test]
    fn typed_query_updates_arrive_through_the_parked_closure_bridge() {
        let _guard = global_test_lock();
        let screen = tall_screen(40, 10, 30);
        screen.borrow_mut().toggle_search();
        // The search overlay is focused; typing goes through the base's
        // forward-to-focused-component path, which parks the query.
        screen.borrow_mut().handle_terminal_input("n");
        screen.borrow_mut().handle_terminal_input("e");
        // Nothing has drained the parked queries yet through the render
        // path in this test, but navigate_search drains them first.
        assert_eq!(
            screen
                .borrow()
                .active_search
                .as_ref()
                .expect("search")
                .query,
            ""
        );
        screen.borrow_mut().navigate_search(1);
        assert_eq!(
            screen
                .borrow()
                .active_search
                .as_ref()
                .expect("search")
                .query,
            "ne"
        );
    }

    // -------------------------------------------------------------------------

    #[test]
    #[cfg(not(windows))]
    fn right_click_paste_is_disabled_off_windows() {
        let _guard = global_test_lock();
        let (screen, _log) = new_screen(
            20,
            5,
            TuiAltScreenOptions {
                on_right_click_paste: Some(Rc::new(|| panic!("must not paste off Windows"))),
                ..Default::default()
            },
        );
        // A right press: falls through to the selection stub (no panic,
        // no paste).
        feed(&screen, "\x1b[<2;5;3M");
        feed(&screen, "\x1b[<2;5;3m");
    }

}
