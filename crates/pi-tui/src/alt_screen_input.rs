//! Port of the input/mouse/lifecycle half of
//! `packages/tui/src/tui-alt-screen.ts` — **T5 Wave B, task D-B1**.
//!
//! Scope (tui-alt-screen.ts, method inventory):
//! - Terminal lifecycle: `beforeTerminalStart` (tui-alt-screen.ts:345+),
//!   `beforeTerminalStop`/`afterTerminalStop` — the alt-screen enter/exit
//!   ESC sequences ([`crate::tui_alt_screen`] constants), mouse-mode
//!   enable/disable, autowrap disable, focus in/out, capability save and
//!   kitty/iterm2 image handling at start.
//! - `handleViewportInput` (the constructor's self-listener): decode SGR
//!   mouse sequences, wheel events (`parseWheelEvent`/`routeWheel` with
//!   [`crate::tui_alt_screen::ALT_WHEEL_SCROLL_MULTIPLIER`] and
//!   `PAGE_SCROLL_OVERLAP`), keyboard forwarding, search-mode keys.
//! - The mouse event pipeline: `parseSgrMouseEvent`, `decodeMouseButton`,
//!   `createMouseEvent`, `dispatchMouseToLayout`,
//!   `applyMouseDispatchResult`, `dispatchMouseToTarget`,
//!   `getComponentClickCount`, `isMouseSequence`,
//!   `shouldDeferViewportInputToOverlay`, `clearComponentMouseGesture`.
//! - Scrollbar interactions: `getScrollbarTargetAt`, `setScrollbarHover`,
//!   `updateScrollbarHover`, `stopScrollbarHover`,
//!   `scrollScrollbarToPointer`, `handleScrollbarMouseEvent`,
//!   `stopScrollbarDrag`.
//! - The scroll API: `scrollBy`, `scrollToTop`, `scrollToBottom`,
//!   `scrollToPrompt`, `viewportTop`, `isFollowingOutput`.
//! - Search control flow: `toggleSearch`, `closeSearch`,
//!   `updateSearchQuery`, `navigateSearch`,
//!   `getSearchNavigationDirectionAt`, `handleSearchMouseEvent`,
//!   `refreshSearch` (the corpus/index plumbing already lives in
//!   `alt_screen_search.rs`).
//! - `handleRightClickPaste`, `handleScrollToEndIndicatorMouseEvent`,
//!   `flash`.
//!
//! The screen struct, constructor, option types and the hook wiring are
//! coordinator-owned in [`crate::tui_alt_screen`] — add behavior here as
//! `impl TuiAltScreen` blocks and free functions; do not edit the
//! skeleton's struct fields or constructor without coordinator review.
//!
//! ## Ground truth
//!
//! The TS source of record is `/tmp/pi-repo/packages/tui/src/
//! tui-alt-screen.ts`; compiled probes run under node from
//! `/tmp/pi-repo/packages/tui/dist/`. Verify every ambiguous expectation
//! against a node probe before encoding it in a test (see
//! `docs/T5_PLAN.md`).

use crate::tui::{TuiInputListenerResult, TuiStopOptions};
use crate::tui_alt_screen::TuiAltScreen;

/// `beforeTerminalStart()` (tui-alt-screen.ts:345+) — TODO(D-B1): full
/// body (stop selection auto-scroll, reset gesture state, enter alt
/// screen, capability save, image-protocol masking, mouse/autowrap modes).
pub(crate) fn before_terminal_start(_screen: &mut TuiAltScreen) {}

/// `afterTerminalStart()` — TODO(D-B1).
pub(crate) fn after_terminal_start(_screen: &mut TuiAltScreen) {}

/// `beforeTerminalStop(options)` — TODO(D-B1): the alt-screen exit
/// sequence (synchronized output, mouse disable, autowrap restore).
pub(crate) fn before_terminal_stop(_screen: &mut TuiAltScreen, _options: &TuiStopOptions) {}

/// `afterTerminalStop(options)` — TODO(D-B1): restore capabilities,
/// `altScreenActive = false`.
pub(crate) fn after_terminal_stop(_screen: &mut TuiAltScreen, _options: &TuiStopOptions) {}

/// `(data) => this.handleViewportInput(data)` (tui-alt-screen.ts:282) —
/// TODO(D-B1): the SGR mouse/wheel/keyboard decode and dispatch pipeline.
pub(crate) fn handle_viewport_input(
    _screen: &mut TuiAltScreen,
    _data: &str,
) -> Option<TuiInputListenerResult> {
    None
}

/// `refreshSearch(layout)` (tui-alt-screen.ts:595) — TODO(D-B1): re-run
/// the active search corpus/index against the new layout. Returns whether
/// the caller (the doRender pipeline, **D-B3**) must re-layout because the
/// search UI state changed. Called by
/// [`crate::alt_screen_render::do_render`] before highlighting.
pub(crate) fn refresh_search(
    _screen: &mut TuiAltScreen,
    _layout: &crate::layout::LayoutFrame,
) -> bool {
    false
}
