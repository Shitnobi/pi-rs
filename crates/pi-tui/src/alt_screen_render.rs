//! Port of the render half of `packages/tui/src/tui-alt-screen.ts`
//! (~1440-1786) — **T5 Wave B, task D-B3**.
//!
//! Scope (tui-alt-screen.ts method inventory):
//! - Search highlighting: `applySearchTextHighlight`,
//!   `applySearchHighlights` (the `searchMatchStyle`/
//!   `searchCurrentMatchStyle` passes over the layout lines).
//! - Compositing: `compositeScrollToEndIndicator`, `compositeFlashes`,
//!   plus the inherited `compositeOverlays` call.
//! - The `doRender` pipeline (tui-alt-screen.ts:1679+):
//!   `renderLayoutFrame(root, width, height)` (with the `refreshSearch`
//!   re-layout path) → `resolveFakeCursors` with the OSC133 zone-prefix
//!   strip ([`crate::tui_alt_screen::strip_osc133_zone_prefix`]) →
//!   `applySearchHighlights` → `compositeScrollToEndIndicator` →
//!   `compositeOverlays` → slice to height → `applySelection` →
//!   `compositeFlashes` → `extractCursorPosition` → `applyLineResets` +
//!   `sliceByColumn` overflow truncation → terminal write (full vs
//!   differential) → `positionHardwareCursor`.
//! - Kitty image cache: `prepareKittyScreen`, `deleteKittyImages`, the
//!   offscreen cache eviction honoring
//!   [`crate::tui_alt_screen::MAX_CACHED_OFFSCREEN_KITTY_IMAGES`] /
//!   `_TRANSMISSION_BYTES` / `_DECODED_BYTES` (the
//!   `uploaded_kitty_images` map on the screen).
//! - `resetRenderState` (tui-alt-screen.ts:484-488).
//!
//! The screen struct, constructor and hook wiring are coordinator-owned
//! in [`crate::tui_alt_screen`]; the kitty cache entry type
//! ([`crate::tui_alt_screen::CachedKittyImage`]) is fixed. Do not edit
//! them without coordinator review.
//!
//! ## Ground truth
//!
//! The TS source of record is `/tmp/pi-repo/packages/tui/src/
//! tui-alt-screen.ts`; verify ambiguous expectations against compiled
//! node probes from `/tmp/pi-repo/packages/tui/dist/` before encoding
//! them in tests.

use crate::layout::render_layout_frame;
use crate::tui::park_render_request;
use crate::tui_alt_screen::TuiAltScreen;

/// `doRender()` (tui-alt-screen.ts:1679+) — skeleton implementation:
/// lays out the root (the explicit `layoutRoot` or the implicit scroll
/// view), records the frame and writes it as a full redraw.
///
/// TODO(D-B3): replace with the full TS pipeline — fake-cursor resolution
/// with the OSC133 zone-prefix strip, search highlights, scroll-to-end
/// indicator, overlay compositing, selection, flashes, cursor extraction,
/// line resets, `sliceByColumn` truncation and the differential write
/// path with hardware-cursor positioning and the kitty image
/// prepare/evict steps.
pub(crate) fn do_render(screen: &mut TuiAltScreen) {
    let width = screen.base.terminal.borrow().columns() as i64;
    let height = screen.base.terminal.borrow().rows() as i64;
    let tui_id = screen.base.tui_id;
    let root = screen
        .layout_root
        .clone()
        .unwrap_or_else(|| screen.implicit_scroll_view.clone());
    let layout = render_layout_frame(&root, width, height, move || park_render_request(tui_id));
    screen.current_layout = Some(layout.clone());
    let mut lines = layout.lines;
    lines.truncate(height.max(0) as usize);
    screen.previous_screen_width = width;
    screen.previous_screen_height = height;
    screen.last_document = lines.clone();
    screen.previous_screen = lines;
    // Full redraw only — the differential path is D-B3 scope.
    let mut output = String::from("\x1b[H");
    for (index, line) in screen.previous_screen.iter().enumerate() {
        if index > 0 {
            output.push_str("\r\n");
        }
        output.push_str(line);
    }
    screen.base.terminal.borrow_mut().write(&output);
}

/// `resetRenderState()` (tui-alt-screen.ts:484-488).
pub(crate) fn reset_render_state(screen: &mut TuiAltScreen) {
    screen.previous_screen.clear();
    screen.previous_screen_width = 0;
    screen.previous_screen_height = 0;
    screen.current_layout = None;
}
