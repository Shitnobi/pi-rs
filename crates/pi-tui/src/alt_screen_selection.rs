//! Port of the text-selection half of
//! `packages/tui/src/tui-alt-screen.ts` (tui-alt-screen.ts:890-919,
//! 1137-1475, 1571-1636) — **T5 Wave B, task D-B2**.
//!
//! Scope (tui-alt-screen.ts method inventory, with line numbers):
//! - Selection points and geometry: `getScrollSelectionPoint` (:1137),
//!   `getSelectionPoint` (:1157), `getSelectionSourceLine` (:1168),
//!   `getWordSelection` (:1176 — word segmentation honoring
//!   [`crate::tui_alt_screen::TERMINAL_WORD_SELECTION_JOINERS`]),
//!   `getLineSelection` (:1213), `getSelectionBounds` (:1401),
//!   `getSelectionColumns` (:1418).
//! - Multi-click + drag state machine: `handleSelectionMouseEvent`
//!   (:1321, called by **D-B1**'s `handleMouseEvent` pipeline through
//!   this module's [`handle_selection_mouse_event`] seam),
//!   `getClickCount` (:1240 — `DOUBLE_CLICK_INTERVAL_MS` on the virtual
//!   clock), `updateSelectionFocus` (:1220 — character/word/line
//!   granularity transitions and the `selectionInitialRange` anchor
//!   flip), `selectionDragged`/`pressedUrl`/`openUrl` interplay.
//! - Auto-scroll: `updateSelectionAutoScroll` (:1266),
//!   `autoScrollSelection` (:1294), `stopSelectionAutoScroll` (:1312).
//!   The TS `setInterval(() => this.autoScrollSelection(), 50)` becomes
//!   the virtual-clock deadline `screen.selection_auto_scroll_timer`;
//!   the clock pump calls [`pump_selection_auto_scroll`] (and may call
//!   [`auto_scroll_selection`] directly).
//! - Text extraction and copy: `getActiveSelectionText` (:1439),
//!   `copySelectionToClipboard` (:1460), `copyTextToClipboard` (:1466 —
//!   the `copySelection` option callback, the OSC 52 fallback write, and
//!   the `COPY_ERROR_FLASH_DURATION_MS` error flash),
//!   `copyActiveSelectionToClipboard` (:310), `copyOnSelect` on release.
//! - Highlighting (consumed by **D-B3**'s doRender pipeline):
//!   `applySelectionHighlight` (:1571) and [`apply_selection`] (:1588 —
//!   maps selection columns onto the rendered frame lines with
//!   column-accurate SGR restyling).
//! - State resets: `clearTextSelection` (:890), `resetTextSelection`
//!   (:317).
//!
//! ## Divergences from TS (see also the method-level doc comments and
//! /tmp/report-t5-B2.md)
//!
//! - TS `Date.now()` → [`crate::clock::now_ms`]; the auto-scroll
//!   `setInterval` becomes a deadline re-armed by
//!   [`pump_selection_auto_scroll`] (interval cadence scheduled from the
//!   firing time).
//! - The TS promise returned by `copySelection` resolves synchronously
//!   ([`crate::tui_alt_screen::CopySelectionResult`]).
//! - The release-click component dispatch (`createMouseEvent` /
//!   `dispatchMouseToOverlay` / `dispatchMouseToLayout` /
//!   `applyMouseDispatchResult`, tui-alt-screen.ts:809-871) is owned by
//!   **D-B1**; until their module lands, this file carries faithful
//!   local ports of the four helpers (see the "D-B1-owned helpers"
//!   section) so the selection release path is behaviorally complete.
//!   The coordinator should reconcile the duplication when D-B1 lands.
//! - `scrollView.scrollBy` cannot be expressed through the
//!   coordinator-owned `ScrollViewState` trait (no scroll mutation); the
//!   interim [`scroll_selection_view_by`] reports "nothing moved", which
//!   makes auto-scroll disarm exactly as TS does at a scroll edge.
//!   Needs coordinator action — see the report.
//!
//! ## Ground truth
//!
//! The TS source of record is `/tmp/pi-repo/packages/tui/src/
//! tui-alt-screen.ts`; verify ambiguous expectations against compiled
//! node probes from `/tmp/pi-repo/packages/tui/dist/` before encoding
//! them in tests.

use std::rc::Rc;

use crate::clock::now_ms;
use crate::component::{
    dispatch_mouse_event, ComponentRef, TuiMouseDispatchResult, TuiMouseButton,
    TuiMouseEvent, TuiMouseEventType,
};
use crate::layout::{
    get_layout_boxes_at, get_scroll_view_box, get_scroll_views_at, LayoutFrame, ScrollViewRef,
};
use crate::terminal_image::is_image_line;
use crate::tui::TuiSurface;
use crate::tui_alt_screen::{
    CopySelectionResult, SgrMouseEvent, SelectionGranularity, SelectionPoint, SelectionRange,
    TuiAltScreen, COPY_ERROR_FLASH_DURATION_MS, DOUBLE_CLICK_INTERVAL_MS,
    TERMINAL_WORD_SELECTION_JOINERS,
};
use crate::unicode::word_break::segment_words;
use crate::utils::{
    extract_ansi_code, get_grapheme_cell_range, get_osc8_link_at_column, is_js_whitespace,
    slice_by_column, strip_terminal_sequences, visible_width,
};

/// The TS auto-scroll `setInterval(..., 50)` cadence
/// (tui-alt-screen.ts:1286). Hardcoded upstream; not a coordinator
/// constant.
const SELECTION_AUTO_SCROLL_INTERVAL_MS: f64 = 50.0;

// =============================================================================
// Small shared helpers
// =============================================================================

/// JS `a === b` on `ScrollView | undefined` object references.
fn same_scroll_view(a: &Option<ScrollViewRef>, b: &Option<ScrollViewRef>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}

/// JS `String.prototype.trimEnd()` — trims the JS whitespace set.
fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_whitespace)
}

/// `Buffer.from(text).toString("base64")` — standard base64 with padding
/// (the OSC 52 clipboard payload, tui-alt-screen.ts:1483).
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 0x3f] as char);
        out.push(TABLE[(n >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

/// The terminal size as `(columns, rows)` (i64, matching the TS `number`
/// usage in clamps).
fn terminal_size(screen: &TuiAltScreen) -> (i64, i64) {
    let terminal = screen.base.terminal.borrow();
    (terminal.columns() as i64, terminal.rows() as i64)
}

// =============================================================================
// D-B1-owned helpers (tui-alt-screen.ts:796-871), ported locally
// =============================================================================
//
// `handleSelectionMouseEvent`'s release path dispatches a synthesized
// click to the overlay/component under the pointer before deciding
// whether to keep the selection (tui-alt-screen.ts:1350-1362). Those
// four helpers are D-B1's scope, but their module is still a stub, so
// faithful local ports live here. **Coordinator action**: when D-B1
// lands `createMouseEvent`/`dispatchMouseToLayout`/
// `applyMouseDispatchResult`, replace these with calls into
// `alt_screen_input.rs` (the behavior must stay identical — these are
// 1:1 ports of the same TS lines).

/// `decodeMouseButton(button)` (tui-alt-screen.ts:796-804).
fn decode_mouse_button(button: i64) -> TuiMouseButton {
    match button & 3 {
        0 => TuiMouseButton::Left,
        1 => TuiMouseButton::Middle,
        2 => TuiMouseButton::Right,
        _ => TuiMouseButton::None,
    }
}

/// `createMouseEvent(type, button, x, y, extra)` (tui-alt-screen.ts:809-
/// 831), restricted to the click shape used by the selection release
/// path (`clickCount`; no wheel delta).
fn create_click_mouse_event(
    screen: &TuiAltScreen,
    button: i64,
    x: i64,
    y: i64,
    click_count: i64,
) -> TuiMouseEvent {
    let (columns, rows) = terminal_size(screen);
    TuiMouseEvent {
        event_type: TuiMouseEventType::Click,
        button: decode_mouse_button(button),
        x,
        y,
        screen_x: x,
        screen_y: y,
        width: columns.max(1),
        height: rows.max(1),
        shift: (button & 4) != 0,
        alt: (button & 8) != 0,
        ctrl: (button & 16) != 0,
        wheel_delta: None,
        click_count: Some(click_count),
    }
}

/// `dispatchMouseToLayout(event)` (tui-alt-screen.ts:833-850).
///
/// TS skips boxes whose component has a `[LAYOUT_NODE]` **and** uses the
/// stock `Container.prototype.handleMouse`. In this codebase the
/// layout-node components (`Stack`/`VStack`/`HStack`/`ScrollView`) all
/// inherit that stock handler, so `layout_node().is_some()` is an exact
/// equivalent of the conjunction.
fn dispatch_mouse_to_layout(
    screen: &mut TuiAltScreen,
    event: &TuiMouseEvent,
) -> Option<TuiMouseDispatchResult> {
    let layout = screen.current_layout.clone()?;
    let mut visited: Vec<ComponentRef> = Vec::new();
    for box_ in get_layout_boxes_at(&layout, event.screen_x, event.screen_y) {
        if visited.iter().any(|c| Rc::ptr_eq(c, &box_.component)) {
            continue;
        }
        if box_.component.borrow_mut().layout_node().is_some() {
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

/// `applyMouseDispatchResult(event, result)` (tui-alt-screen.ts:853-868).
fn apply_mouse_dispatch_result(
    screen: &mut TuiAltScreen,
    event: &TuiMouseEvent,
    result: TuiMouseDispatchResult,
) -> bool {
    let focus_component = result
        .focus_target
        .clone()
        .unwrap_or_else(|| result.target.component.clone());
    let focus_target = screen.base.resolve_mouse_focus_target(&focus_component);
    let focus_changed = result.focus.unwrap_or(false)
        && match screen.base.get_focused_component() {
            Some(focused) => !Rc::ptr_eq(&focused, &focus_target),
            None => true,
        };
    if result.focus.unwrap_or(false) {
        TuiSurface::set_focus(screen, Some(focus_target));
    }
    if result.capture.unwrap_or(false) {
        screen.mouse_capture = Some(result.target);
    }
    result.render.unwrap_or(
        focus_changed
            || matches!(
                event.event_type,
                TuiMouseEventType::Press
                    | TuiMouseEventType::Click
                    | TuiMouseEventType::Drag
                    | TuiMouseEventType::Wheel
            ),
    )
}

// =============================================================================
// Points and geometry (tui-alt-screen.ts:1137-1237)
// =============================================================================

/// `getScrollSelectionPoint(scrollView, x, y)` (tui-alt-screen.ts:1137-
/// 1155): viewport → document coordinate mapping through the layout's
/// scroll-view box, clamped to the visible window and the content.
fn get_scroll_selection_point(
    screen: &TuiAltScreen,
    scroll_view: &ScrollViewRef,
    x: i64,
    y: i64,
) -> Option<SelectionPoint> {
    let layout = screen.current_layout.as_ref()?;
    let box_ = get_scroll_view_box(layout, scroll_view)?;
    if box_.rect.height <= 0 || box_.clip.height <= 0 {
        return None;
    }
    let (_, rows) = terminal_size(screen);
    let visible_top = 0.max(box_.rect.y).max(box_.clip.y);
    let visible_bottom = (rows - 1)
        .min(box_.rect.y + box_.rect.height - 1)
        .min(box_.clip.y + box_.clip.height - 1);
    if visible_bottom < visible_top {
        return None;
    }
    let pointer_row = y.max(visible_top).min(visible_bottom);
    let content_lines = box_
        .scroll_content_lines
        .as_ref()
        .map(|lines| lines.len() as i64)
        .unwrap_or(1);
    let max_content_row = 0.max(content_lines - 1);
    let scroll_top = scroll_view.borrow().scroll_top();
    Some(SelectionPoint {
        row: (scroll_top + pointer_row - box_.rect.y).min(max_content_row).max(0),
        col: (x - box_.rect.x).min(box_.rect.width - 1).max(0),
        scroll_view: Some(Rc::clone(scroll_view)),
        boundary: false,
    })
}

/// `getSelectionPoint(event, scrollView?)` (tui-alt-screen.ts:1157-1166):
/// prefer the scroll-view mapping, fall back to clamped terminal
/// coordinates.
fn get_selection_point(
    screen: &TuiAltScreen,
    event: &SgrMouseEvent,
    scroll_view: Option<&ScrollViewRef>,
) -> SelectionPoint {
    if let Some(scroll_view) = scroll_view {
        if let Some(point) = get_scroll_selection_point(screen, scroll_view, event.x, event.y) {
            return point;
        }
    }
    let (columns, rows) = terminal_size(screen);
    SelectionPoint {
        row: event.y.min(rows - 1).max(0),
        col: event.x.min(columns - 1).max(0),
        scroll_view: None,
        boundary: false,
    }
}

/// `getSelectionSourceLine(point)` (tui-alt-screen.ts:1168-1174): the
/// document line for a point — the scroll view's `scrollContentLines`
/// when the point lives in one, else the last rendered frame.
fn get_selection_source_line(screen: &TuiAltScreen, point: &SelectionPoint) -> String {
    if let Some(scroll_view) = &point.scroll_view {
        if let Some(layout) = screen.current_layout.as_ref() {
            if let Some(box_) = get_scroll_view_box(layout, scroll_view) {
                if let Some(lines) = &box_.scroll_content_lines {
                    return lines
                        .get(point.row.max(0) as usize)
                        .cloned()
                        .unwrap_or_default();
                }
            }
        }
    }
    screen
        .previous_screen
        .get(point.row.max(0) as usize)
        .cloned()
        .unwrap_or_default()
}

/// One `getWordSelection` segment (tui-alt-screen.ts:1178-1186):
/// `[start, end)` visible-column span plus the joiner/selectable flags.
struct WordSegmentColumn {
    start: i64,
    end: i64,
    selectable: bool,
    joiner: bool,
}

/// `getWordSelection(point)` (tui-alt-screen.ts:1176-1211): segment the
/// (sequence-stripped) source line into `Intl.Segmenter` word segments,
/// then coalesce the clicked segment with its neighbors across
/// `/`/`-` joiners so paths and kebab-case tokens stay whole.
fn get_word_selection(
    screen: &TuiAltScreen,
    point: &SelectionPoint,
) -> Option<SelectionRange> {
    let line = strip_terminal_sequences(&get_selection_source_line(screen, point));
    let mut segments: Vec<WordSegmentColumn> = Vec::new();
    let mut start = 0i64;
    for segment in segment_words(&line) {
        let end = start + visible_width(&segment.segment) as i64;
        // `TERMINAL_WORD_SELECTION_JOINERS.has(segment.segment)` — the set
        // holds exactly "/" and "-".
        let joiner = matches!(segment.segment.as_str(), "/" | "-")
            && TERMINAL_WORD_SELECTION_JOINERS.contains(&segment.segment.chars().next().unwrap());
        segments.push(WordSegmentColumn {
            start,
            end,
            selectable: segment.is_word_like || joiner,
            joiner,
        });
        start = end;
    }
    let clicked = segments
        .iter()
        .position(|segment| point.col >= segment.start && point.col < segment.end)?;

    let can_join = |left: &WordSegmentColumn, right: &WordSegmentColumn| {
        left.selectable && right.selectable && (left.joiner || right.joiner)
    };
    let mut selection_start = segments[clicked].start;
    let mut selection_end = segments[clicked].end;
    let mut index = clicked;
    while index > 0 && can_join(&segments[index - 1], &segments[index]) {
        selection_start = segments[index - 1].start;
        index -= 1;
    }
    let mut index = clicked;
    while index + 1 < segments.len() && can_join(&segments[index], &segments[index + 1]) {
        selection_end = segments[index + 1].end;
        index += 1;
    }
    Some(SelectionRange {
        start: SelectionPoint {
            row: point.row,
            col: selection_start,
            scroll_view: point.scroll_view.clone(),
            boundary: false,
        },
        end: SelectionPoint {
            row: point.row,
            col: selection_end,
            scroll_view: point.scroll_view.clone(),
            boundary: true,
        },
    })
}

/// `getLineSelection(point)` (tui-alt-screen.ts:1213-1218): columns 0
/// through the line's visible width (a boundary point).
fn get_line_selection(screen: &TuiAltScreen, point: &SelectionPoint) -> SelectionRange {
    let line = get_selection_source_line(screen, point);
    SelectionRange {
        start: SelectionPoint {
            row: point.row,
            col: 0,
            scroll_view: point.scroll_view.clone(),
            boundary: false,
        },
        end: SelectionPoint {
            row: point.row,
            col: visible_width(&line) as i64,
            scroll_view: point.scroll_view.clone(),
            boundary: true,
        },
    }
}

/// `updateSelectionFocus(point)` (tui-alt-screen.ts:1220-1238): move the
/// focus for a drag. Character granularity (or no initial range) sets the
/// focus directly; word/line granularity re-derives the range at the
/// pointer and flips the anchor to the far side of the initial range
/// when the pointer moves before it.
fn update_selection_focus(screen: &mut TuiAltScreen, point: SelectionPoint) {
    if screen.selection_granularity == SelectionGranularity::Character
        || screen.selection_initial_range.is_none()
    {
        screen.selection_focus = Some(point);
        return;
    }
    let range = if screen.selection_granularity == SelectionGranularity::Word {
        get_word_selection(screen, &point)
    } else {
        Some(get_line_selection(screen, &point))
    };
    let Some(range) = range else { return };
    let initial = screen.selection_initial_range.as_ref().unwrap();
    let target_before_initial = range.start.row < initial.start.row
        || (range.start.row == initial.start.row && range.start.col < initial.start.col);
    if target_before_initial {
        screen.selection_anchor = Some(initial.end.clone());
        screen.selection_focus = Some(range.start);
    } else {
        screen.selection_anchor = Some(initial.start.clone());
        screen.selection_focus = Some(range.end);
    }
}

/// `getClickCount(point, word)` (tui-alt-screen.ts:1240-1256): the
/// multi-click counter over the virtual clock. A repeat requires the
/// same row, scroll view and word extents within
/// `DOUBLE_CLICK_INTERVAL_MS`; the count cycles 1 → 2 → 3 → 1. A press
/// with no word under it clears the history.
fn get_click_count(
    screen: &mut TuiAltScreen,
    point: &SelectionPoint,
    word: Option<&SelectionRange>,
) -> i64 {
    let now = now_ms();
    let previous = screen.last_click.clone();
    let count = match (word, previous) {
        (Some(word), Some(previous))
            if now - previous.timestamp <= DOUBLE_CLICK_INTERVAL_MS
                && previous.row == point.row
                && same_scroll_view(&previous.scroll_view, &point.scroll_view)
                && previous.word_start == word.start.col
                && previous.word_end == word.end.col =>
        {
            (previous.count % 3) + 1
        }
        _ => 1,
    };
    screen.last_click = word.map(|word| crate::tui_alt_screen::ClickTarget {
        timestamp: now,
        count,
        row: point.row,
        scroll_view: point.scroll_view.clone(),
        word_start: word.start.col,
        word_end: word.end.col,
    });
    count
}

// =============================================================================
// Auto-scroll (tui-alt-screen.ts:1266-1319)
// =============================================================================

/// `scrollView.scrollBy(lines)` as used by `autoScrollSelection`
/// (tui-alt-screen.ts:1297) — returns the unused delta.
fn scroll_selection_view_by(_screen: &TuiAltScreen, scroll_view: &ScrollViewRef, lines: i64) -> i64 {
    scroll_view.borrow_mut().scroll_by(lines)
}

/// `updateSelectionAutoScroll(event)` (tui-alt-screen.ts:1266-1292):
/// record the drag pointer and arm the auto-scroll interval when the
/// pointer is held at/beyond the anchor scroll view's visible edges.
fn update_selection_auto_scroll(screen: &mut TuiAltScreen, event: &SgrMouseEvent) {
    let Some(scroll_view) = screen
        .selection_anchor
        .as_ref()
        .and_then(|point| point.scroll_view.clone())
    else {
        stop_selection_auto_scroll(screen);
        return;
    };
    let Some(layout) = screen.current_layout.clone() else {
        stop_selection_auto_scroll(screen);
        return;
    };
    let Some(box_) = get_scroll_view_box(&layout, &scroll_view) else {
        stop_selection_auto_scroll(screen);
        return;
    };
    if box_.rect.height <= 0 || box_.clip.height <= 0 {
        stop_selection_auto_scroll(screen);
        return;
    }
    let (_, rows) = terminal_size(screen);
    let visible_top = 0.max(box_.rect.y).max(box_.clip.y);
    let visible_bottom = (rows - 1)
        .min(box_.rect.y + box_.rect.height - 1)
        .min(box_.clip.y + box_.clip.height - 1);
    screen.selection_drag_pointer = Some((event.x, event.y));
    screen.selection_auto_scroll_direction = if event.y <= visible_top {
        -1
    } else if event.y >= visible_bottom {
        1
    } else {
        0
    };
    if screen.selection_auto_scroll_direction == 0 {
        stop_selection_auto_scroll(screen);
        return;
    }
    if screen.selection_auto_scroll_timer.is_some() {
        return;
    }
    // TS: `setInterval(() => this.autoScrollSelection(), 50)` + `.unref()`.
    screen.selection_auto_scroll_timer = Some(now_ms() + SELECTION_AUTO_SCROLL_INTERVAL_MS);
}

/// `autoScrollSelection()` (tui-alt-screen.ts:1294-1310): one interval
/// tick — scroll the anchor's view in the armed direction, stop when the
/// view cannot scroll further, and extend the selection to the pointer's
/// new content position.
pub(crate) fn auto_scroll_selection(screen: &mut TuiAltScreen) {
    let scroll_view = screen
        .selection_anchor
        .as_ref()
        .and_then(|point| point.scroll_view.clone());
    let pointer = screen.selection_drag_pointer;
    let direction = screen.selection_auto_scroll_direction;
    let (Some(scroll_view), Some(pointer)) = (scroll_view, pointer) else {
        stop_selection_auto_scroll(screen);
        return;
    };
    if direction == 0 {
        stop_selection_auto_scroll(screen);
        return;
    }
    let remaining = scroll_selection_view_by(screen, &scroll_view, direction);
    if remaining == direction {
        stop_selection_auto_scroll(screen);
        return;
    }
    if let Some(point) = get_scroll_selection_point(screen, &scroll_view, pointer.0, pointer.1) {
        update_selection_focus(screen, point);
    }
    screen.request_render(false);
}

/// `stopSelectionAutoScroll()` (tui-alt-screen.ts:1312-1319): clear the
/// interval, direction and drag pointer.
pub(crate) fn stop_selection_auto_scroll(screen: &mut TuiAltScreen) {
    if screen.selection_auto_scroll_timer.is_some() {
        screen.selection_auto_scroll_timer = None;
    }
    screen.selection_auto_scroll_direction = 0;
    screen.selection_drag_pointer = None;
}

/// Clock-pump entry for the auto-scroll interval (port machinery — no TS
/// counterpart; the pump owner is D-B1/coordinator). Fires
/// [`auto_scroll_selection`] while the virtual clock has passed the
/// deadline, re-arming the 50 ms cadence after each fire (the interval
/// survives until `stopSelectionAutoScroll` clears it).
pub(crate) fn pump_selection_auto_scroll_until(
    screen: &mut TuiAltScreen,
    bound: f64,
) -> bool {
    // TS `setInterval` semantics: the callback fires once per period —
    // each re-arm is `deadline + period`, not `now + period`, so a large
    // clock jump fires the callback for every elapsed period.
    let mut fired = false;
    while let Some(deadline) = screen.selection_auto_scroll_timer {
        if bound < deadline {
            break;
        }
        fired = true;
        auto_scroll_selection(screen);
        if screen.selection_auto_scroll_timer.is_some() {
            screen.selection_auto_scroll_timer =
                Some(deadline + SELECTION_AUTO_SCROLL_INTERVAL_MS);
        }
    }
    fired
}

// =============================================================================
// The selection mouse state machine (tui-alt-screen.ts:1321-1399)
// =============================================================================

/// `handleSelectionMouseEvent(event)` (tui-alt-screen.ts:1321-1399) —
/// the press/drag/release state machine driving anchor/focus,
/// multi-click granularity, pressed URLs, auto-scroll and copy-on-select.
/// Called by **D-B1**'s `handleMouseEvent` once components, scrollbar and
/// search have declined the event.
// Wired by D-B1's `handleMouseEvent` (alt_screen_input.rs, still a stub);
// referenced today only from this module's tests.
#[allow(dead_code)]
pub(crate) fn handle_selection_mouse_event(screen: &mut TuiAltScreen, event: &SgrMouseEvent) {
    let button = event.button & 3;
    if button != 0 && !(event.release && button == 3) {
        return;
    }
    let anchor_scroll_view = screen
        .selection_anchor
        .as_ref()
        .and_then(|point| point.scroll_view.clone());
    let point = get_selection_point(screen, event, anchor_scroll_view.as_ref());
    if event.release {
        if !screen.selection_press_active {
            return;
        }
        screen.selection_press_active = false;
        stop_selection_auto_scroll(screen);
        if screen.selection_anchor.is_none() {
            return;
        }
        update_selection_focus(screen, point.clone());
        let is_click = !screen.selection_dragged && {
            let anchor = screen.selection_anchor.as_ref().unwrap();
            same_scroll_view(&anchor.scroll_view, &point.scroll_view)
                && anchor.row == point.row
                && anchor.col == point.col
        };
        let clicked_url = if is_click {
            screen.pressed_url.clone()
        } else {
            None
        };
        screen.pressed_url = None;
        if let Some(url) = clicked_url {
            if let Some(open_url) = screen.open_url.clone() {
                screen.selection_anchor = None;
                screen.selection_focus = None;
                // TS wraps `this.openUrl(clickedUrl)` in try/catch — URL
                // activation is best-effort. The Rust callback must not
                // panic; a panic propagates (documented divergence).
                open_url(&url);
                screen.request_render(false);
                return;
            }
        }
        if is_click {
            let click_count = screen
                .last_click
                .as_ref()
                .map(|click| click.count)
                .unwrap_or(1);
            let click_event =
                create_click_mouse_event(screen, event.button, event.x, event.y, click_count);
            let overlay = screen.base.dispatch_mouse_to_overlay(&click_event);
            let result = match overlay {
                // `overlay.result ?? (overlay.hit ? undefined : dispatch)`
                Some(inner) => inner,
                None => dispatch_mouse_to_layout(screen, &click_event),
            };
            if let Some(result) = result {
                let render = apply_mouse_dispatch_result(screen, &click_event, result);
                clear_text_selection(screen);
                if render {
                    screen.request_render(false);
                }
                return;
            }
        }
        if screen.copy_on_select {
            copy_selection_to_clipboard(screen);
        }
        screen.request_render(false);
        return;
    }
    if (event.button & 32) != 0 {
        // Motion (drag) with a button held.
        if !screen.selection_press_active || screen.selection_anchor.is_none() {
            return;
        }
        screen.selection_dragged = true;
        screen.last_click = None;
        screen.pressed_url = None;
        update_selection_focus(screen, point);
        update_selection_auto_scroll(screen, event);
        screen.request_render(false);
        return;
    }
    // Press.
    stop_selection_auto_scroll(screen);
    screen.selection_press_active = true;
    let scroll_view = if !screen.base.has_visible_overlay() {
        screen
            .current_layout
            .as_ref()
            .and_then(|layout| get_scroll_views_at(layout, event.x, event.y).into_iter().next())
    } else {
        None
    };
    let anchor = get_selection_point(screen, event, scroll_view.as_ref());
    let word = get_word_selection(screen, &anchor);
    let click_count = get_click_count(screen, &anchor, word.as_ref());
    let range = if click_count == 2 {
        word
    } else if click_count == 3 {
        Some(get_line_selection(screen, &anchor))
    } else {
        None
    };
    screen.selection_granularity = if range.is_some() {
        if click_count == 2 {
            SelectionGranularity::Word
        } else {
            SelectionGranularity::Line
        }
    } else {
        SelectionGranularity::Character
    };
    screen.selection_initial_range = range.clone();
    screen.selection_anchor = Some(match &range {
        Some(range) => range.start.clone(),
        None => anchor.clone(),
    });
    screen.selection_focus = Some(match &range {
        Some(range) => range.end.clone(),
        None => anchor.clone(),
    });
    screen.selection_dragged = false;
    screen.pressed_url = if range.is_some() {
        None
    } else {
        let (columns, rows) = terminal_size(screen);
        let row = event.y.min(rows - 1).max(0) as usize;
        let column = event.x.min(columns - 1).max(0) as usize;
        screen
            .previous_screen
            .get(row)
            .and_then(|line| get_osc8_link_at_column(line, column))
    };
    screen.request_render(false);
}

/// `clearTextSelection()` (tui-alt-screen.ts:890-900).
pub(crate) fn clear_text_selection(screen: &mut TuiAltScreen) {
    stop_selection_auto_scroll(screen);
    screen.selection_press_active = false;
    screen.selection_anchor = None;
    screen.selection_focus = None;
    screen.selection_granularity = SelectionGranularity::Character;
    screen.selection_initial_range = None;
    screen.pressed_url = None;
    screen.selection_dragged = false;
}

/// `resetTextSelection()` (tui-alt-screen.ts:317-319) — drops the
/// selection and the multi-click history.
pub(crate) fn reset_text_selection(screen: &mut TuiAltScreen) {
    clear_text_selection(screen);
    screen.last_click = None;
}

// =============================================================================
// Bounds, columns and text (tui-alt-screen.ts:1401-1475)
// =============================================================================

/// `getSelectionBounds()` (tui-alt-screen.ts:1401-1416): the ordered
/// (start ≤ end) anchor/focus pair, or `None` when the selection is
/// empty or spans different scroll views.
fn get_selection_bounds(screen: &TuiAltScreen) -> Option<SelectionRange> {
    let anchor = screen.selection_anchor.as_ref()?;
    let focus = screen.selection_focus.as_ref()?;
    if !same_scroll_view(&anchor.scroll_view, &focus.scroll_view) {
        return None;
    }
    if anchor.row == focus.row && anchor.col == focus.col {
        return None;
    }
    let anchor_before_focus = anchor.row < focus.row
        || (anchor.row == focus.row && anchor.col < focus.col);
    if anchor_before_focus {
        Some(SelectionRange {
            start: anchor.clone(),
            end: focus.clone(),
        })
    } else {
        Some(SelectionRange {
            start: focus.clone(),
            end: anchor.clone(),
        })
    }
}

/// `getSelectionColumns(line, row, selection, minColumn = 0, maxColumn =
/// visibleWidth(line))` (tui-alt-screen.ts:1418-1437): the visible-column
/// span of the selection on one line, snapping non-boundary endpoints to
/// grapheme cell ranges.
fn get_selection_columns(
    line: &str,
    row: i64,
    selection: &SelectionRange,
    min_column: i64,
    max_column: i64,
) -> (i64, i64) {
    let line_width = visible_width(line) as i64;
    let mut start = 0.max(min_column);
    let mut end = line_width.min(max_column);
    if row == selection.start.row {
        start = get_grapheme_cell_range(line, selection.start.col.max(0) as usize)
            .map(|range| range.start as i64)
            .unwrap_or(selection.start.col.min(line_width));
    }
    if row == selection.end.row {
        end = if selection.end.boundary {
            selection.end.col.min(line_width)
        } else {
            get_grapheme_cell_range(line, selection.end.col.max(0) as usize)
                .map(|range| range.end as i64)
                .unwrap_or((selection.end.col + 1).min(line_width))
        };
    }
    (min_column.max(start), max_column.min(end))
}

/// `getActiveSelectionText()` (tui-alt-screen.ts:1439-1460): the selected
/// text, one `trimEnd`-ed line per row joined by newlines. `None` when
/// there is no non-empty selection (or its scroll view is gone).
pub(crate) fn get_active_selection_text(screen: &TuiAltScreen) -> Option<String> {
    let selection = get_selection_bounds(screen)?;
    let source_lines: &[String] = if let Some(scroll_view) = &selection.start.scroll_view {
        let layout = screen.current_layout.as_ref()?;
        let box_ = get_scroll_view_box(layout, scroll_view)?;
        let lines = box_.scroll_content_lines.as_ref()?;
        lines.as_slice()
    } else {
        screen.previous_screen.as_slice()
    };
    let mut lines: Vec<String> = Vec::new();
    let mut row = selection.start.row;
    while row <= selection.end.row {
        let line = source_lines
            .get(row.max(0) as usize)
            .map(|line| line.as_str())
            .unwrap_or("");
        let line_width = visible_width(line) as i64;
        let (start, end) = get_selection_columns(line, row, &selection, 0, line_width);
        let sliced = slice_by_column(
            line,
            start.max(0) as usize,
            (end - start).max(0) as usize,
            true,
        );
        lines.push(js_trim_end(&strip_terminal_sequences(&sliced)).to_string());
        row += 1;
    }
    let text = lines.join("\n");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

// =============================================================================
// Copy paths (tui-alt-screen.ts:310-315, 1462-1486)
// =============================================================================

/// `copySelectionToClipboard()` (tui-alt-screen.ts:1462-1465) — the
/// copy-on-select path. The TS promise resolves synchronously here.
fn copy_selection_to_clipboard(screen: &mut TuiAltScreen) -> bool {
    match get_active_selection_text(screen) {
        Some(text) => copy_text_to_clipboard(screen, &text),
        None => false,
    }
}

/// `copyTextToClipboard(text)` (tui-alt-screen.ts:1466-1486): prefer the
/// injected `copySelection` callback (flashing "Copied!", its message, or
/// the generic error with `COPY_ERROR_FLASH_DURATION_MS`); otherwise
/// write the OSC 52 clipboard sequence and flash "Copied!".
fn copy_text_to_clipboard(screen: &mut TuiAltScreen, text: &str) -> bool {
    if let Some(copy_selection) = screen.copy_selection.clone() {
        let result = copy_selection(text);
        let ok = result == CopySelectionResult::Success;
        match result {
            CopySelectionResult::Success => screen.flash("Copied!", None),
            CopySelectionResult::Failure => {
                screen.flash("Copy failed", Some(COPY_ERROR_FLASH_DURATION_MS));
            }
            CopySelectionResult::Message(message) => {
                screen.flash(&message, Some(COPY_ERROR_FLASH_DURATION_MS));
            }
        }
        return ok;
    }
    let sequence = format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()));
    screen.base.terminal.borrow_mut().write(&sequence);
    screen.flash("Copied!", None);
    true
}

/// `copyActiveSelectionToClipboard()` (tui-alt-screen.ts:310-315) — the
/// public entry. `false` when there is no active selection; otherwise the
/// configured copy path (OSC 52 fallback, error flash on failure).
pub(crate) fn copy_active_selection_to_clipboard(screen: &mut TuiAltScreen) -> bool {
    match get_active_selection_text(screen) {
        Some(text) => copy_text_to_clipboard(screen, &text),
        None => false,
    }
}

// =============================================================================
// Highlighting (tui-alt-screen.ts:1571-1636)
// =============================================================================

/// `applySelectionHighlight(text)` (tui-alt-screen.ts:1571-1586): wrap
/// the text in reverse video, re-applying `\x1b[7m` after every SGR code
/// inside it so the inversion survives mid-selection style changes.
pub(crate) fn apply_selection_highlight(text: &str) -> String {
    let mut result = String::from("\x1b[7m");
    let mut index = 0usize;
    while index < text.len() {
        if let Some(code) = extract_ansi_code(text, index) {
            result.push_str(code);
            if code.ends_with('m') {
                result.push_str("\x1b[7m");
            }
            index += code.len();
        } else {
            // TS walks UTF-16 units; the Rust walk advances whole chars
            // (ANSI codes are ASCII, so the scans agree).
            let ch = text[index..].chars().next().unwrap();
            result.push(ch);
            index += ch.len_utf8();
        }
    }
    result.push_str("\x1b[27m");
    result
}

/// `applySelection(screen, layout = this.currentLayout)`
/// (tui-alt-screen.ts:1588-1636): restyle the rendered frame lines
/// covered by the selection. Scroll-view selections are mapped from
/// content coordinates back to screen coordinates through the layout box
/// and clipped to its visible window; image lines are skipped.
// Wired by D-B3's doRender pipeline (alt_screen_render.rs, still a
// stub); referenced today only from this module's tests.
#[allow(dead_code)]
pub(crate) fn apply_selection(
    screen: &mut TuiAltScreen,
    screen_lines: Vec<String>,
    layout: Option<&LayoutFrame>,
) -> Vec<String> {
    let Some(selection) = get_selection_bounds(screen) else {
        return screen_lines;
    };
    let mut screen_selection = selection.clone();
    let mut min_row = 0i64;
    let mut max_row = screen_lines.len() as i64 - 1;
    let mut min_column = 0i64;
    let (columns, _) = terminal_size(screen);
    let mut max_column = columns;
    if selection.start.scroll_view.is_some() {
        let Some(layout) = layout else {
            return screen_lines;
        };
        let scroll_view = selection.start.scroll_view.clone().unwrap();
        let Some(box_) = get_scroll_view_box(layout, &scroll_view) else {
            return screen_lines;
        };
        min_row = 0.max(box_.rect.y).max(box_.clip.y);
        max_row = (screen_lines.len() as i64 - 1)
            .min(box_.rect.y + box_.rect.height - 1)
            .min(box_.clip.y + box_.clip.height - 1);
        min_column = 0.max(box_.rect.x).max(box_.clip.x);
        max_column = columns
            .min(box_.rect.x + box_.rect.width)
            .min(box_.clip.x + box_.clip.width);
        // TS maps both endpoints through `selection.start.scrollView`'s
        // scrollTop (getSelectionBounds guarantees one shared view).
        let scroll_top = scroll_view.borrow().scroll_top();
        screen_selection.start.row = box_.rect.y + selection.start.row - scroll_top;
        screen_selection.start.col = box_.rect.x + selection.start.col;
        screen_selection.end.row = box_.rect.y + selection.end.row - scroll_top;
        screen_selection.end.col = box_.rect.x + selection.end.col;
    }
    screen_lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let row = index as i64;
            if row < min_row
                || row > max_row
                || row < screen_selection.start.row
                || row > screen_selection.end.row
                || is_image_line(&line)
            {
                return line;
            }
            let line_width = visible_width(&line) as i64;
            let (start, end) =
                get_selection_columns(&line, row, &screen_selection, min_column, max_column);
            if end <= start {
                return line;
            }
            let before = slice_by_column(&line, 0, start.max(0) as usize, true);
            let selected =
                slice_by_column(&line, start.max(0) as usize, (end - start).max(0) as usize, true);
            let after = slice_by_column(
                &line,
                end.max(0) as usize,
                (line_width - end).max(0) as usize,
                true,
            );
            format!("{before}{}{after}", apply_selection_highlight(&selected))
        })
        .collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{advance_ms, reset_for_test};
    use crate::component::Component;
    use crate::components::text::Text;
    use crate::terminal::{InputHandler, ProgramStatus, ResizeHandler, Terminal};
    use crate::test_support::global_test_lock;
    use crate::tui_alt_screen::{
        TuiAltScreenOptions,
    };
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// Recording terminal double (pattern from tui_alt_screen.rs tests).
    struct TestTerminal {
        cols: Cell<usize>,
        rows: Cell<usize>,
        writes: RefCell<Vec<String>>,
    }

    impl TestTerminal {
        fn new(cols: usize, rows: usize) -> Self {
            TestTerminal {
                cols: Cell::new(cols),
                rows: Cell::new(rows),
                writes: RefCell::new(Vec::new()),
            }
        }

        fn writes(&self) -> Vec<String> {
            self.writes.borrow().clone()
        }
    }

    impl Terminal for TestTerminal {
        fn start(&mut self, _on_input: InputHandler, _on_resize: ResizeHandler) {}
        fn stop(&mut self) {}
        fn drain_input(&mut self, _max_ms: u64, _idle_ms: u64) {}
        fn write(&mut self, data: &str) {
            self.writes.borrow_mut().push(data.to_string());
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

    fn text_component(text: &str) -> ComponentRef {
        Rc::new(RefCell::new(Text::new(text, 0, 0, None)))
    }

    /// Raw (unpadded) content lines — unlike `Text`, which pads every
    /// line to the full width, this double emits the lines verbatim so a
    /// row can be truly empty (no word segments under the pointer).
    fn raw_lines(lines: &[&str]) -> ComponentRef {
        Rc::new(RefCell::new(RawLines { lines: lines.iter().map(|l| l.to_string()).collect() }))
    }

    /// Backing store for [`raw_lines`].
    struct RawLines {
        lines: Vec<String>,
    }

    impl Component for RawLines {
        fn render(&mut self, _width: usize) -> Vec<String> {
            self.lines.clone()
        }
    }

    fn new_screen(
        cols: usize,
        rows: usize,
        options: TuiAltScreenOptions,
    ) -> (Rc<RefCell<TuiAltScreen>>, Rc<RefCell<TestTerminal>>) {
        let terminal = Rc::new(RefCell::new(TestTerminal::new(cols, rows)));
        let terminal_ref: Rc<RefCell<dyn Terminal>> = terminal.clone();
        let screen = TuiAltScreen::new(terminal_ref, None, None, options);
        screen.borrow_mut().alt_screen_active = true;
        (screen, terminal)
    }

    fn default_screen(cols: usize, rows: usize) -> Rc<RefCell<TuiAltScreen>> {
        new_screen(cols, rows, TuiAltScreenOptions::default()).0
    }

    fn press(screen: &Rc<RefCell<TuiAltScreen>>, x: i64, y: i64) {
        handle_selection_mouse_event(
            &mut screen.borrow_mut(),
            &SgrMouseEvent { button: 0, x, y, release: false },
        );
    }

    fn release(screen: &Rc<RefCell<TuiAltScreen>>, x: i64, y: i64) {
        handle_selection_mouse_event(
            &mut screen.borrow_mut(),
            &SgrMouseEvent { button: 0, x, y, release: true },
        );
    }

    fn drag(screen: &Rc<RefCell<TuiAltScreen>>, x: i64, y: i64) {
        handle_selection_mouse_event(
            &mut screen.borrow_mut(),
            &SgrMouseEvent { button: 32, x, y, release: false },
        );
    }

    /// The rendered frame after one render pass.
    #[allow(dead_code)]
    fn rendered(screen: &Rc<RefCell<TuiAltScreen>>) -> Vec<String> {
        screen.borrow().previous_screen.clone()
    }

    /// The frame `doRender` hands to `applySelection`: `previousScreen`
    /// minus the `SEGMENT_RESET` suffix `applyLineResets` appends after
    /// selection highlighting (`normalizeTerminalOutput` is the identity
    /// on these ASCII fixtures).
    fn document_frame(screen: &Rc<RefCell<TuiAltScreen>>) -> Vec<String> {
        const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\u{7}";
        screen
            .borrow()
            .previous_screen
            .iter()
            .map(|line| line.strip_suffix(SEGMENT_RESET).unwrap_or(line).to_string())
            .collect()
    }

    fn active_text(screen: &Rc<RefCell<TuiAltScreen>>) -> Option<String> {
        get_active_selection_text(&screen.borrow())
    }

    fn flashes(screen: &Rc<RefCell<TuiAltScreen>>) -> Vec<String> {
        screen.borrow_mut().flashes.render(60)
    }

    // ---- word / line selection geometry -----------------------------------

    #[test]
    fn word_selection_coalesces_slash_and_hyphen_joiners() {
        let _guard = global_test_lock();
        reset_for_test();
        // Upstream: tui-alt-screen.test.ts "coalesces slash and hyphen
        // separated segments for double-click word selection".
        for (line, needle_col) in [
            ("extensions/starline/fixed-editor/compositor.ts", 12),
            ("earendil-works/pi-tui", 10),
        ] {
            let screen = default_screen(80, 1);
            screen.borrow_mut().add_child(text_component(line));
            screen.borrow_mut().render_now(true);

            // Two clicks on the same word inside the double-click window.
            press(&screen, needle_col, 0);
            release(&screen, needle_col, 0);
            press(&screen, needle_col, 0);

            let expected = line;
            assert_eq!(active_text(&screen).as_deref(), Some(expected));
            assert_eq!(screen.borrow().selection_granularity, SelectionGranularity::Word);
        }
    }

    #[test]
    fn word_drag_extends_over_whitespace_segments() {
        let _guard = global_test_lock();
        reset_for_test();
        // Upstream: tui-alt-screen.test.ts "highlights a complete
        // whitespace segment during a word drag".
        let screen = default_screen(20, 1);
        screen.borrow_mut().add_child(text_component("foo  bar"));
        screen.borrow_mut().render_now(true);

        // Double-click on "foo" (0-based col 1), then drag to col 3 (the
        // first space of the gap).
        press(&screen, 1, 0);
        release(&screen, 1, 0);
        press(&screen, 1, 0);
        drag(&screen, 3, 0);

        let frame = document_frame(&screen);
        let layout = screen.borrow().current_layout.clone();
        let lines = apply_selection(&mut screen.borrow_mut(), frame, layout.as_ref());
        assert_eq!(lines[0], "\x1b[7mfoo  \x1b[27mbar            ");
    }

    #[test]
    fn triple_click_selects_the_whole_line() {
        let _guard = global_test_lock();
        reset_for_test();
        // Upstream: tui-alt-screen.test.ts "selects whole words on double
        // click, ... and selects lines on triple click".
        let screen = default_screen(20, 2);
        screen.borrow_mut().add_child(text_component("zero alpha beta\ngamma delta"));
        screen.borrow_mut().render_now(true);

        // Three clicks on the same word ("delta") in the second row.
        for col in [6, 8, 10] {
            press(&screen, col, 1);
            release(&screen, col, 1);
        }
        // The last press armed the line selection.
        assert_eq!(active_text(&screen).as_deref(), Some("gamma delta"));
        assert_eq!(screen.borrow().selection_granularity, SelectionGranularity::Line);
    }

    #[test]
    fn click_count_resets_after_the_double_click_interval() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 1);
        screen.borrow_mut().add_child(text_component("alpha beta"));
        screen.borrow_mut().render_now(true);

        press(&screen, 0, 0);
        release(&screen, 0, 0);
        advance_ms(DOUBLE_CLICK_INTERVAL_MS + 1.0);
        press(&screen, 0, 0);

        // Outside the window: still a single (character) click.
        assert_eq!(screen.borrow().selection_granularity, SelectionGranularity::Character);
        assert!(active_text(&screen).is_none());
        // The history restarts: an immediate third click is a double.
        release(&screen, 0, 0);
        press(&screen, 0, 0);
        assert_eq!(active_text(&screen).as_deref(), Some("alpha"));
    }

    #[test]
    fn click_count_cycles_modulo_three() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 1);
        screen.borrow_mut().add_child(text_component("alpha beta"));
        screen.borrow_mut().render_now(true);

        for expected_count in [1, 2, 3, 1] {
            press(&screen, 0, 0);
            assert_eq!(
                screen.borrow().last_click.as_ref().map(|c| c.count),
                Some(expected_count)
            );
            release(&screen, 0, 0);
        }
    }

    #[test]
    fn press_without_a_word_clears_the_multi_click_history() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 2);
        screen.borrow_mut().add_child(raw_lines(&["alpha", ""]));
        screen.borrow_mut().render_now(true);

        press(&screen, 0, 0);
        release(&screen, 0, 0);
        assert!(screen.borrow().last_click.is_some());
        // Row 1 renders empty: no word under the pointer.
        press(&screen, 0, 1);
        assert!(screen.borrow().last_click.is_none());
        assert_eq!(screen.borrow().selection_granularity, SelectionGranularity::Character);
    }

    #[test]
    fn drag_selection_supports_reversed_ranges() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 2);
        screen.borrow_mut().add_child(text_component("zero alpha beta\ngamma delta"));
        screen.borrow_mut().render_now(true);

        // Drag rightwards...
        press(&screen, 5, 0);
        drag(&screen, 11, 0);
        assert_eq!(active_text(&screen).as_deref(), Some("alpha b"));
        // ...and leftwards past the anchor (reversed range).
        drag(&screen, 1, 0);
        assert_eq!(active_text(&screen).as_deref(), Some("ero a"));

        // Multi-row, reversed.
        press(&screen, 3, 1);
        drag(&screen, 2, 0);
        assert_eq!(active_text(&screen).as_deref(), Some("ro alpha beta\ngamm"));
    }

    #[test]
    fn selection_columns_boundary_and_grapheme_semantics() {
        let _guard = global_test_lock();
        let line = "a界b";
        let selection = SelectionRange {
            start: SelectionPoint { row: 0, col: 0, scroll_view: None, boundary: false },
            end: SelectionPoint { row: 0, col: 2, scroll_view: None, boundary: false },
        };
        // Non-boundary end on the wide char's second cell snaps to the
        // grapheme's cell-range end (col 3).
        assert_eq!(get_selection_columns(line, 0, &selection, 0, 4), (0, 3));
        // Boundary end stops between cells (col 2).
        let boundary = SelectionRange {
            start: selection.start.clone(),
            end: SelectionPoint { row: 0, col: 2, scroll_view: None, boundary: true },
        };
        assert_eq!(get_selection_columns(line, 0, &boundary, 0, 4), (0, 2));
        // Non-boundary start snaps forward to the grapheme start.
        let mid_grapheme = SelectionRange {
            start: SelectionPoint { row: 0, col: 2, scroll_view: None, boundary: false },
            end: SelectionPoint { row: 0, col: 4, scroll_view: None, boundary: true },
        };
        assert_eq!(get_selection_columns(line, 0, &mid_grapheme, 0, 4), (1, 4));
        // Endpoints beyond the line clamp to its width.
        let beyond = SelectionRange {
            start: SelectionPoint { row: 0, col: 0, scroll_view: None, boundary: false },
            end: SelectionPoint { row: 0, col: 99, scroll_view: None, boundary: true },
        };
        assert_eq!(get_selection_columns(line, 0, &beyond, 0, 99), (0, 4));
        // min/max clamps apply after the grapheme snap.
        assert_eq!(get_selection_columns(line, 0, &boundary, 1, 2), (1, 2));
        // Rows outside the selection fall back to the min/max window.
        assert_eq!(get_selection_columns(line, 1, &boundary, 0, 4), (0, 4));
    }

    #[test]
    fn grapheme_boundary_snapping_in_drag_selection() {
        let _guard = global_test_lock();
        reset_for_test();
        // Upstream: tui-alt-screen.test.ts "snaps mouse selection to CJK,
        // emoji, and combining grapheme boundaries".
        let screen = default_screen(20, 1);
        screen.borrow_mut().add_child(text_component("x界🙂y"));
        screen.borrow_mut().render_now(true);

        // Drag from col 0 to col 2 (the second cell of 界): the focus
        // snaps to the grapheme end (col 3), selecting "x界".
        press(&screen, 0, 0);
        drag(&screen, 2, 0);
        assert_eq!(active_text(&screen).as_deref(), Some("x界"));

        // Drag onto the emoji's first cell selects through it.
        drag(&screen, 3, 0);
        assert_eq!(active_text(&screen).as_deref(), Some("x界🙂"));
    }

    #[test]
    fn active_selection_text_trims_trailing_cells_per_line() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 3);
        screen
            .borrow_mut()
            .add_child(text_component("alpha   \nbeta\x1b[31m!  \x1b[0m\ngamma"));
        screen.borrow_mut().render_now(true);

        press(&screen, 0, 0);
        drag(&screen, 4, 2);
        let text = active_text(&screen).unwrap();
        assert_eq!(text, "alpha\nbeta!\ngamma");
        // Lines with only whitespace inside the selection vanish.
        let screen = default_screen(20, 3);
        screen.borrow_mut().add_child(text_component("a   \n    \nb"));
        screen.borrow_mut().render_now(true);
        press(&screen, 0, 0);
        drag(&screen, 0, 1);
        // Rows 0-1 selected; row 1 trims to empty but still joins.
        assert_eq!(active_text(&screen).as_deref(), Some("a\n"));
    }

    // ---- highlighting ------------------------------------------------------

    #[test]
    fn apply_selection_restyles_sgr_lines_column_accurately() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 1);
        screen
            .borrow_mut()
            .add_child(text_component("\x1b[31mred\x1b[0m green \x1b[1mblue\x1b[22m"));
        screen.borrow_mut().render_now(true);

        // Select cols 2..13 ("d green blu").
        press(&screen, 2, 0);
        drag(&screen, 12, 0);
        let frame = document_frame(&screen);
        let layout = screen.borrow().current_layout.clone();
        let lines = apply_selection(&mut screen.borrow_mut(), frame, layout.as_ref());
        // Byte-for-byte the node-probe output (probe-sel.mjs, "sgr"):
        // the selected slice re-opens active SGR state at its start and
        // applySelectionHighlight re-applies [7m after every SGR code.
        assert_eq!(
            lines[0],
            concat!(
                "\x1b[31mre",
                "\x1b[7m\x1b[31m\x1b[7md\x1b[0m\x1b[7m",
                " green ",
                "\x1b[1m\x1b[7mblu\x1b[27m",
                "\x1b[31m\x1b[0m\x1b[1me\x1b[22m",
                "      ",
            )
        );
    }

    #[test]
    fn apply_selection_skips_image_lines_and_out_of_window_rows() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 2);
        screen.borrow_mut().add_child(text_component("alpha\ngamma"));
        screen.borrow_mut().render_now(true);

        // Selection over row 1 only.
        press(&screen, 0, 1);
        drag(&screen, 4, 1);
        let frame = document_frame(&screen);
        let layout = screen.borrow().current_layout.clone();
        let lines = apply_selection(&mut screen.borrow_mut(), frame, layout.as_ref());
        // Byte-for-byte the node-probe output (probe-sel.mjs, "skips"):
        // row 0 passes through (padded), row 1 is highlighted to the full
        // line width.
        assert_eq!(lines[0], "alpha               ");
        assert_eq!(lines[1], "\x1b[7mgamma\x1b[27m               ");

        // An image line is never restyled.
        let image_lines = vec![
            "\x1b_Ga=...;q\x1b\\".to_string(),
            "plain".to_string(),
        ];
        screen.borrow_mut().selection_anchor = Some(SelectionPoint {
            row: 0,
            col: 0,
            scroll_view: None,
            boundary: false,
        });
        screen.borrow_mut().selection_focus = Some(SelectionPoint {
            row: 1,
            col: 5,
            scroll_view: None,
            boundary: true,
        });
        let layout = screen.borrow().current_layout.clone();
        let lines = apply_selection(&mut screen.borrow_mut(), image_lines, layout.as_ref());
        assert_eq!(lines[0], "\x1b_Ga=...;q\x1b\\");
        assert_eq!(lines[1], "\x1b[7mplain\x1b[27m");
    }

    #[test]
    fn apply_selection_maps_scroll_view_coordinates_through_the_layout() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 2);
        screen.borrow_mut().add_child(text_component("one\ntwo"));
        screen.borrow_mut().render_now(true);

        // A scroll-view selection (cols/rows in content space) maps back
        // through the box (identity here: the implicit view fills the
        // screen at scrollTop 0).
        press(&screen, 0, 0);
        drag(&screen, 2, 1);
        assert!(screen.borrow().selection_anchor.as_ref().unwrap().scroll_view.is_some());
        let frame = document_frame(&screen);
        let layout = screen.borrow().current_layout.clone();
        let lines = apply_selection(&mut screen.borrow_mut(), frame, layout.as_ref());
        // Byte-for-byte the node-probe output (probe-sel.mjs, "maps").
        assert_eq!(lines[0], "\x1b[7mone                 \x1b[27m");
        assert_eq!(lines[1], "\x1b[7mtwo\x1b[27m                 ");

        // No layout → unchanged.
        let frame = document_frame(&screen);
        let lines = apply_selection(&mut screen.borrow_mut(), frame.clone(), None);
        assert_eq!(lines, frame);
    }

    #[test]
    fn apply_selection_highlight_reapplies_inverse_after_sgr_codes() {
        // tui-alt-screen.ts:1571-1586.
        assert_eq!(
            apply_selection_highlight("plain"),
            "\x1b[7mplain\x1b[27m"
        );
        assert_eq!(
            apply_selection_highlight("a\x1b[31mb\x1b[0mc"),
            "\x1b[7ma\x1b[31m\x1b[7mb\x1b[0m\x1b[7mc\x1b[27m"
        );
        // Non-SGR escape codes pass through without re-application.
        assert_eq!(
            apply_selection_highlight("a\x1b]8;;u\x07b"),
            "\x1b[7ma\x1b]8;;u\x07b\x1b[27m"
        );
    }

    // ---- copy paths --------------------------------------------------------

    #[test]
    fn copy_with_injected_handler_reports_all_three_outcomes() {
        let _guard = global_test_lock();
        reset_for_test();
        let copied: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = copied.clone();
        let screen = new_screen(
            20,
            1,
            TuiAltScreenOptions {
                copy_selection: Some(Rc::new(move |text| {
                    sink.borrow_mut().push(text.to_string());
                    if text == "ok" {
                        CopySelectionResult::Success
                    } else if text == "no" {
                        CopySelectionResult::Failure
                    } else {
                        CopySelectionResult::Message("Clipboard unavailable".to_string())
                    }
                })),
                ..TuiAltScreenOptions::default()
            },
        )
        .0;
        screen.borrow_mut().add_child(text_component("ok no maybe"));
        screen.borrow_mut().render_now(true);

        fn select_and_copy(screen: &Rc<RefCell<TuiAltScreen>>, start: i64, end: i64) -> bool {
            press(screen, start, 0);
            drag(screen, end, 0);
            copy_active_selection_to_clipboard(&mut screen.borrow_mut())
        }

        // Success: "Copied!" flash, true.
        assert!(select_and_copy(&screen, 0, 1));
        assert_eq!(flashes(&screen).last().unwrap(), "\x1b[7m Copied! \x1b[27m");
        // Failure: generic error flash with COPY_ERROR_FLASH_DURATION_MS.
        assert!(!select_and_copy(&screen, 3, 4));
        assert_eq!(flashes(&screen).last().unwrap(), "\x1b[7m Copy failed \x1b[27m");
        let entry_expires_after_5s = {
            let before = screen.borrow_mut().flashes.render(60).len();
            let _ = before;
            advance_ms(COPY_ERROR_FLASH_DURATION_MS - 1.0);
            let still = !screen.borrow_mut().flashes.render(60).is_empty();
            advance_ms(1.0);
            screen.borrow_mut().flashes.tick();
            let gone = screen.borrow_mut().flashes.render(60).is_empty();
            still && gone
        };
        assert!(entry_expires_after_5s, "error flash must last exactly 5000ms");
        // Message: flashed verbatim with the error duration.
        assert!(!select_and_copy(&screen, 6, 10));
        assert_eq!(
            flashes(&screen).last().unwrap(),
            "\x1b[7m Clipboard unavailable \x1b[27m"
        );
        assert_eq!(
            copied.borrow().as_slice(),
            ["ok".to_string(), "no".to_string(), "maybe".to_string()]
        );
    }

    #[test]
    fn copy_falls_back_to_osc52_and_flashes_copied() {
        let _guard = global_test_lock();
        reset_for_test();
        let (screen, terminal) = new_screen(20, 1, TuiAltScreenOptions::default());
        screen.borrow_mut().add_child(text_component("alpha beta"));
        screen.borrow_mut().render_now(true);

        // No selection → false, no write, no flash.
        let writes_before = terminal.borrow().writes().len();
        assert!(!copy_active_selection_to_clipboard(&mut screen.borrow_mut()));
        assert_eq!(terminal.borrow().writes().len(), writes_before);

        press(&screen, 0, 0);
        drag(&screen, 4, 0);
        assert!(copy_active_selection_to_clipboard(&mut screen.borrow_mut()));
        let expected = format!("\x1b]52;c;{}\x07", base64_encode(b"alpha"));
        assert!(terminal.borrow().writes().iter().any(|w| w.contains(&expected)));
        assert_eq!(flashes(&screen).last().unwrap(), "\x1b[7m Copied! \x1b[27m");
    }

    #[test]
    fn copy_on_select_fires_on_release_and_can_be_disabled() {
        let _guard = global_test_lock();
        reset_for_test();
        let copied: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = copied.clone();
        let screen = new_screen(
            20,
            1,
            TuiAltScreenOptions {
                copy_selection: Some(Rc::new(move |text| {
                    sink.borrow_mut().push(text.to_string());
                    CopySelectionResult::Success
                })),
                ..TuiAltScreenOptions::default()
            },
        )
        .0;
        screen.borrow_mut().add_child(text_component("alpha beta"));
        screen.borrow_mut().render_now(true);

        // Default copyOnSelect = true: a completed drag copies on release.
        press(&screen, 0, 0);
        drag(&screen, 4, 0);
        release(&screen, 4, 0);
        assert_eq!(copied.borrow().as_slice(), ["alpha".to_string()]);

        // A plain click (no drag) does not copy.
        press(&screen, 6, 0);
        release(&screen, 6, 0);
        assert_eq!(copied.borrow().len(), 1);

        screen.borrow_mut().set_copy_on_select(false);
        press(&screen, 0, 0);
        drag(&screen, 4, 0);
        release(&screen, 4, 0);
        assert_eq!(copied.borrow().len(), 1);
        // The selection itself survives (upstream: "leaves selections
        // visible without copying when copyOnSelect is disabled").
        assert_eq!(active_text(&screen).as_deref(), Some("alpha"));
    }

    // ---- pressed URLs ------------------------------------------------------

    #[test]
    fn pressed_url_opens_on_click_release() {
        let _guard = global_test_lock();
        reset_for_test();
        let opened: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = opened.clone();
        let screen = new_screen(
            40,
            1,
            TuiAltScreenOptions {
                open_url: Some(Rc::new(move |url| {
                    sink.borrow_mut().push(url.to_string());
                })),
                // Keep copyOnSelect from flashing on the release path.
                copy_on_select: Some(false),
                ..TuiAltScreenOptions::default()
            },
        )
        .0;
        screen
            .borrow_mut()
            .add_child(text_component("\x1b]8;;https://example.com\x07read this\x1b]8;;\x07"));
        screen.borrow_mut().render_now(true);

        // A single press (no word range) remembers the OSC 8 link.
        press(&screen, 0, 0);
        assert_eq!(
            screen.borrow().pressed_url.as_deref(),
            Some("https://example.com")
        );
        // Releasing over the same cell is a click → the URL opens and the
        // selection is dropped.
        release(&screen, 0, 0);
        assert_eq!(opened.borrow().as_slice(), ["https://example.com".to_string()]);
        assert!(screen.borrow().selection_anchor.is_none());
        assert!(screen.borrow().selection_focus.is_none());

        // A drag cancels the pressed URL (motion path clears it). Advance
        // past the double-click window first: a second press on the same
        // word within it is a double-click (word range → no pressed URL).
        advance_ms(DOUBLE_CLICK_INTERVAL_MS + 100.0);
        press(&screen, 0, 0);
        assert!(screen.borrow().pressed_url.is_some());
        drag(&screen, 9, 0);
        release(&screen, 9, 0);
        assert_eq!(opened.borrow().len(), 1);
    }

    // ---- auto-scroll -------------------------------------------------------

    #[test]
    fn auto_scroll_arms_at_viewport_edges_and_disarms_in_the_middle() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 3);
        screen.borrow_mut().add_child(text_component("a\nb\nc"));
        screen.borrow_mut().render_now(true);

        press(&screen, 0, 1);
        drag(&screen, 0, 0);
        {
            let borrowed = screen.borrow();
            assert_eq!(borrowed.selection_auto_scroll_direction, -1);
            assert_eq!(borrowed.selection_drag_pointer, Some((0, 0)));
            assert_eq!(
                borrowed.selection_auto_scroll_timer,
                Some(now_ms() + SELECTION_AUTO_SCROLL_INTERVAL_MS)
            );
            assert!(borrowed.selection_dragged);
            assert!(borrowed.last_click.is_none());
        }

        // Not due yet: the pump is a no-op before the deadline.
        advance_ms(10.0);
        pump_selection_auto_scroll_until(&mut screen.borrow_mut(), now_ms());
        assert!(screen.borrow().selection_auto_scroll_timer.is_some());

        // At the deadline the pump fires autoScrollSelection. The view
        // cannot scroll (interim scroll seam, see the module docs), so the
        // drag stops itself exactly as TS does at a scroll edge.
        advance_ms(40.0);
        pump_selection_auto_scroll_until(&mut screen.borrow_mut(), now_ms());
        {
            let borrowed = screen.borrow();
            assert!(borrowed.selection_auto_scroll_timer.is_none());
            assert_eq!(borrowed.selection_auto_scroll_direction, 0);
            assert_eq!(borrowed.selection_drag_pointer, None);
            // The press/drag state itself survives.
            assert!(borrowed.selection_press_active);
            assert!(borrowed.selection_dragged);
        }

        // Moving to the middle of the viewport disarms immediately.
        drag(&screen, 0, 0);
        assert!(screen.borrow().selection_auto_scroll_timer.is_some());
        drag(&screen, 5, 1);
        {
            let borrowed = screen.borrow();
            assert_eq!(borrowed.selection_auto_scroll_direction, 0);
            assert!(borrowed.selection_auto_scroll_timer.is_none());
            assert_eq!(borrowed.selection_drag_pointer, None);
        }

        // The bottom edge arms the downward direction.
        drag(&screen, 5, 2);
        assert_eq!(screen.borrow().selection_auto_scroll_direction, 1);
        assert!(screen.borrow().selection_auto_scroll_timer.is_some());
    }

    #[test]
    fn auto_scroll_requires_a_scroll_view_anchor() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 2);
        screen.borrow_mut().add_child(text_component("a\nb"));
        screen.borrow_mut().render_now(true);

        // Without a layout there is no scroll-view anchor: the drag arms
        // nothing and clears any prior auto-scroll state. (The anchor
        // itself must exist for the motion branch to run at all — TS
        // returns early before the auto-scroll update when it is unset.)
        let scroll_view = screen.borrow().implicit_scroll_view_state.clone();
        screen.borrow_mut().current_layout = None;
        screen.borrow_mut().selection_press_active = true;
        screen.borrow_mut().selection_anchor = Some(SelectionPoint {
            row: 0,
            col: 0,
            scroll_view: Some(scroll_view),
            boundary: false,
        });
        screen.borrow_mut().selection_auto_scroll_direction = -1;
        screen.borrow_mut().selection_auto_scroll_timer = Some(1.0);
        drag(&screen, 0, 0);
        {
            let borrowed = screen.borrow();
            assert_eq!(borrowed.selection_auto_scroll_direction, 0);
            assert!(borrowed.selection_auto_scroll_timer.is_none());
        }
    }

    // ---- state resets and guards -------------------------------------------

    #[test]
    fn clear_and_reset_text_selection_drop_all_state() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 1);
        screen.borrow_mut().add_child(text_component("alpha beta"));
        screen.borrow_mut().render_now(true);

        press(&screen, 0, 0);
        // A drag would clear the multi-click history (TS motion branch
        // sets `lastClick = undefined`), so arm the drag flag directly.
        screen.borrow_mut().selection_dragged = true;
        screen.borrow_mut().selection_auto_scroll_direction = 1;
        screen.borrow_mut().selection_auto_scroll_timer = Some(99.0);

        clear_text_selection(&mut screen.borrow_mut());
        {
            let borrowed = screen.borrow();
            assert!(!borrowed.selection_press_active);
            assert!(borrowed.selection_anchor.is_none());
            assert!(borrowed.selection_focus.is_none());
            assert_eq!(borrowed.selection_granularity, SelectionGranularity::Character);
            assert!(borrowed.selection_initial_range.is_none());
            assert!(borrowed.pressed_url.is_none());
            assert!(!borrowed.selection_dragged);
            assert!(borrowed.selection_auto_scroll_timer.is_none());
            assert_eq!(borrowed.selection_auto_scroll_direction, 0);
            // clearTextSelection keeps the multi-click history...
            assert!(borrowed.last_click.is_some());
        }

        reset_text_selection(&mut screen.borrow_mut());
        assert!(screen.borrow().last_click.is_none());
        // Idempotent.
        reset_text_selection(&mut screen.borrow_mut());
        assert!(screen.borrow().last_click.is_none());
    }

    #[test]
    fn non_left_buttons_are_ignored() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 1);
        screen.borrow_mut().add_child(text_component("alpha"));
        screen.borrow_mut().render_now(true);

        for button in [1, 2, 3, 33, 34] {
            handle_selection_mouse_event(
                &mut screen.borrow_mut(),
                &SgrMouseEvent { button, x: 0, y: 0, release: false },
            );
        }
        // Release with button 3 ("none") is allowed through; everything
        // else above left the state untouched.
        let borrowed = screen.borrow();
        assert!(!borrowed.selection_press_active);
        assert!(borrowed.selection_anchor.is_none());
        assert!(borrowed.last_click.is_none());
    }

    #[test]
    fn release_without_press_and_motion_without_press_are_no_ops() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 1);
        screen.borrow_mut().add_child(text_component("alpha"));
        screen.borrow_mut().render_now(true);

        release(&screen, 0, 0);
        drag(&screen, 2, 0);
        let borrowed = screen.borrow();
        assert!(!borrowed.selection_press_active);
        assert!(borrowed.selection_anchor.is_none());
        assert!(!borrowed.selection_dragged);
    }

    #[test]
    fn click_release_without_a_component_handler_keeps_copy_on_select_path() {
        let _guard = global_test_lock();
        reset_for_test();
        // A press+release over a component that does not handle the click
        // falls through to copy-on-select with no selection (no copy) —
        // and over a component that DOES handle it, the selection is
        // cleared instead.
        struct ClickCatcher {
            handled: bool,
        }
        impl Component for ClickCatcher {
            fn render(&mut self, width: usize) -> Vec<String> {
                vec![crate::utils::truncate_to_width("alpha beta", width, "", false)]
            }
            fn handle_mouse(
                &mut self,
                _self_ref: &ComponentRef,
                _event: &TuiMouseEvent,
            ) -> Option<crate::component::ComponentMouseResult> {
                if self.handled {
                    Some(crate::component::ComponentMouseResult::Result(
                        crate::component::TuiMouseEventResult::handled(),
                    ))
                } else {
                    None
                }
            }
        }

        let screen = default_screen(20, 1);
        screen
            .borrow_mut()
            .add_child(Rc::new(RefCell::new(ClickCatcher { handled: false })));
        screen.borrow_mut().render_now(true);

        // Plain single click: no word? "alpha" is a word, clickCount 1 →
        // no range → anchor == focus → no active selection; release
        // dispatches the click, the component declines, nothing is copied.
        press(&screen, 0, 0);
        release(&screen, 0, 0);
        assert!(active_text(&screen).is_none());

        // Now with a drag so the selection is non-empty, then release over
        // the declining component: selection survives, requestRender fired.
        press(&screen, 0, 0);
        drag(&screen, 4, 0);
        release(&screen, 4, 0);
        assert_eq!(active_text(&screen).as_deref(), Some("alpha"));

        // A handling component clears the selection on the click dispatch.
        let screen = default_screen(20, 1);
        screen
            .borrow_mut()
            .add_child(Rc::new(RefCell::new(ClickCatcher { handled: true })));
        screen.borrow_mut().render_now(true);
        press(&screen, 0, 0);
        release(&screen, 0, 0);
        assert!(screen.borrow().selection_anchor.is_none());
    }

    #[test]
    fn get_selection_point_clamps_to_the_terminal() {
        let _guard = global_test_lock();
        reset_for_test();
        let screen = default_screen(20, 3);
        let event = SgrMouseEvent { button: 0, x: 100, y: -5, release: false };
        let point = get_selection_point(&screen.borrow(), &event, None);
        assert_eq!(point.row, 0);
        assert_eq!(point.col, 19);
        assert!(point.scroll_view.is_none());
        assert!(!point.boundary);
    }

    #[test]
    fn click_target_helpers_and_base64_match_the_ts_shapes() {
        let _guard = global_test_lock();
        // Buffer.from("alpha").toString("base64").
        assert_eq!(base64_encode(b"alpha"), "YWxwaGE=");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b""), "");

        // decodeMouseButton (tui-alt-screen.ts:796-804).
        assert_eq!(decode_mouse_button(0), TuiMouseButton::Left);
        assert_eq!(decode_mouse_button(1), TuiMouseButton::Middle);
        assert_eq!(decode_mouse_button(2), TuiMouseButton::Right);
        assert_eq!(decode_mouse_button(3), TuiMouseButton::None);
        assert_eq!(decode_mouse_button(35), TuiMouseButton::None);

        // getClickCount's count cycle uses (count % 3) + 1.
        let count = |previous: i64| (previous % 3) + 1;
        assert_eq!([1, 2, 3].map(count), [2, 3, 1]);
    }


}
