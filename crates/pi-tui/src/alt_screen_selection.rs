//! Port of the text-selection half of
//! `packages/tui/src/tui-alt-screen.ts` (~1100-1462) — **T5 Wave B,
//! task D-B2**.
//!
//! Scope (tui-alt-screen.ts method inventory):
//! - Selection points and ranges: `getScrollSelectionPoint`,
//!   `getSelectionPoint`, `getSelectionSourceLine`, `getWordSelection`
//!   (word selection with
//!   [`crate::tui_alt_screen::TERMINAL_WORD_SELECTION_JOINERS`]),
//!   `getLineSelection`, `updateSelectionFocus`, `getClickCount`.
//! - Auto-scroll: `updateSelectionAutoScroll`, `autoScrollSelection`,
//!   `stopSelectionAutoScroll` (the `NodeJS.Timeout` becomes a
//!   virtual-clock deadline in `selection_auto_scroll_timer`, fired from
//!   the clock pump).
//! - Event handling: `handleSelectionMouseEvent`, `clearTextSelection`,
//!   `clearComponentMouseGesture` interplay.
//! - Bounds and text: `getSelectionBounds`, `getSelectionColumns`,
//!   `getActiveSelectionText`, `hasActiveSelection`,
//!   `resetTextSelection`, `copyActiveSelectionToClipboard`.
//! - Copy paths: `copySelectionToClipboard`, `copyTextToClipboard`
//!   (the configured [`crate::tui_alt_screen::CopySelectionFn`], the
//!   OSC 52 fallback write, and the
//!   [`crate::tui_alt_screen::COPY_ERROR_FLASH_DURATION_MS`] error flash).
//! - Highlighting: `applySelectionHighlight`, `applySelection` (the
//!   line/column clipping onto rendered frames — consumed by the D-B3
//!   doRender pipeline).
//!
//! The selection state types ([`crate::tui_alt_screen::SelectionPoint`],
//! `SelectionRange`, `SelectionGranularity`, `ClickTarget`) and the
//! screen fields are coordinator-owned — do not edit them without
//! coordinator review.
//!
//! ## Ground truth
//!
//! The TS source of record is `/tmp/pi-repo/packages/tui/src/
//! tui-alt-screen.ts`; verify ambiguous expectations against compiled
//! node probes from `/tmp/pi-repo/packages/tui/dist/` before encoding
//! them in tests.

use crate::tui_alt_screen::TuiAltScreen;

/// `getActiveSelectionText()` (tui-alt-screen.ts) — TODO(D-B2): the
/// selection bounds → source lines → column-accurate text join.
/// `None` when there is no active (non-empty) selection.
pub(crate) fn get_active_selection_text(_screen: &TuiAltScreen) -> Option<String> {
    None
}

/// `handleSelectionMouseEvent(event)` (tui-alt-screen.ts:1321) —
/// TODO(D-B2): press/drag/release state machine (anchor/focus/granularity
/// multi-click, drag pointer, auto-scroll arming). Called by **D-B1**'s
/// `handleMouseEvent` pipeline.
pub(crate) fn handle_selection_mouse_event(
    _screen: &mut TuiAltScreen,
    _event: &crate::tui_alt_screen::SgrMouseEvent,
) {
}

/// `clearTextSelection()` (tui-alt-screen.ts:890) — TODO(D-B2).
pub(crate) fn clear_text_selection(_screen: &mut TuiAltScreen) {}

/// `resetTextSelection()` (tui-alt-screen.ts:314-317) — drops the
/// selection and the multi-click history.
pub(crate) fn reset_text_selection(screen: &mut TuiAltScreen) {
    clear_text_selection(screen);
    screen.last_click = None;
}

/// `applySelection(screen, layout = this.currentLayout)` (tui-alt-screen.
/// ts:1588) — TODO(D-B2): map the selection bounds onto the rendered
/// frame and restyle the covered spans. Called by **D-B3**'s doRender
/// pipeline.
pub(crate) fn apply_selection(
    _screen: &mut TuiAltScreen,
    screen_lines: Vec<String>,
    _layout: Option<&crate::layout::LayoutFrame>,
) -> Vec<String> {
    screen_lines
}

/// `copyActiveSelectionToClipboard()` (tui-alt-screen.ts:309-312) —
/// TODO(D-B2): `false` when there is no active selection; otherwise the
/// configured copy path (OSC 52 fallback, error flash on failure).
pub(crate) fn copy_active_selection_to_clipboard(_screen: &mut TuiAltScreen) -> bool {
    false
}
