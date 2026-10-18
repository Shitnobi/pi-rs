//! Port of the render half of `packages/tui/src/tui-alt-screen.ts`
//! (~1440-1786) — **T5 Wave B, task D-B3**.
//!
//! Scope (tui-alt-screen.ts method inventory, with line references):
//! - Search highlighting: `applySearchTextHighlight` (:1488),
//!   `applySearchHighlights` (:1508) — the
//!   `searchMatchStyle`/`searchCurrentMatchStyle` passes over the layout
//!   lines, clipped to the scroll-view box (plus scrollbar column).
//! - Compositing: `compositeScrollToEndIndicator` (:1641, with the
//!   `scrollToEndIndicatorRect` bookkeeping), `compositeFlashes`
//!   (:1665, `AltScreenFlashContainer::render`).
//! - The `doRender` pipeline (:1679-1786):
//!   guard → `renderLayoutFrame` (with the `refreshSearch` re-layout path,
//!   through the [`crate::alt_screen_input::refresh_search`] seam) →
//!   `resolveFakeCursors` over OSC133-stripped lines
//!   ([`crate::tui_alt_screen::strip_osc133_zone_prefix`]) →
//!   `applySearchHighlights` → `compositeScrollToEndIndicator` →
//!   `compositeOverlays` → slice to the LAST `height` lines →
//!   `applySelection` (through the
//!   [`crate::alt_screen_selection::apply_selection`] seam) →
//!   `compositeFlashes` → `extractCursorPosition` → `applyLineResets` +
//!   `sliceByColumn` overflow truncation → full-vs-differential write
//!   decision (`previousScreen` empty / size changed, `changedRows`,
//!   `imageAnchorsNeedRedraw`, the WezTerm env check,
//!   `imageCellsNeedRedraw` with placement rows) → the terminal write
//!   (row addressing, synchronized-output brackets, the WezTerm
//!   kitty-last interleave) → cursor positioning → bookkeeping.
//! - Kitty image cache: `deleteKittyImages` (:430),
//!   `prepareKittyScreen` (:434) with the offscreen cache eviction
//!   honoring [`crate::tui_alt_screen::MAX_CACHED_OFFSCREEN_KITTY_IMAGES`]
//!   / `_TRANSMISSION_BYTES` / `_DECODED_BYTES` over
//!   `uploaded_kitty_images`.
//! - `resetRenderState` (:484-488).
//!
//! The screen struct, constructor and hook wiring are coordinator-owned
//! in [`crate::tui_alt_screen`]; the kitty cache entry type
//! ([`crate::tui_alt_screen::CachedKittyImage`]) is fixed.
//!
//! ## Insertion-order side table (port mechanism)
//!
//! TS `uploadedKittyImages` is a `Map` iterated in **insertion order**;
//! `prepareKittyScreen` refreshes a re-seen image to the end
//! (delete + set), and the eviction loop evicts from the front (oldest
//! first). The coordinator-owned field is a `HashMap` (unordered), so the
//! order is tracked in the thread-local [`KITTY_CACHE_ORDER`] side table
//! keyed by `tui_id` (the same pattern as
//! [`crate::tui::park_render_request`]). The table is reconciled against
//! the live map on every prepare, so lifecycle `clear()`s performed by
//! D-B1 (`beforeTerminalStart`/`beforeTerminalStop`) stay consistent.
//!
//! ## Ground truth
//!
//! The TS source of record is `/tmp/pi-repo/packages/tui/src/
//! tui-alt-screen.ts`; the write sequences below were verified against
//! node probes over `/tmp/pi-repo/packages/tui/dist/` (see
//! `/tmp/probe-b3/*.mjs`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::component::composite_tui_line;
use crate::layout::{get_scroll_view_box, get_scrollbar_geometry, render_layout_frame, LayoutFrame};
use crate::terminal_image::{
    delete_all_kitty_images, delete_all_kitty_placements, delete_kitty_image,
    get_kitty_image_placement, get_kitty_image_placement_rows, is_image_line, ImageProtocol,
};
use crate::tui::{park_render_request, TuiBase};
use crate::tui_alt_screen::{
    strip_osc133_zone_prefix, CachedKittyImage, ScrollToEndIndicatorRect, TuiAltScreen,
    BEGIN_SYNCHRONIZED_OUTPUT, END_SYNCHRONIZED_OUTPUT, MAX_CACHED_OFFSCREEN_KITTY_DECODED_BYTES,
    MAX_CACHED_OFFSCREEN_KITTY_IMAGES, MAX_CACHED_OFFSCREEN_KITTY_TRANSMISSION_BYTES,
};
use crate::utils::{extract_ansi_code, slice_by_column, truncate_to_width, visible_width};

// =============================================================================
// doRender (tui-alt-screen.ts:1679-1786)
// =============================================================================

/// `doRender()` (tui-alt-screen.ts:1679-1786) — the alt-screen render
/// pipeline. Byte-exact against the TS write sequences (verified with node
/// probes over the dist build).
pub(crate) fn do_render(screen: &mut TuiAltScreen) {
    // `if (this.stopped || !this.altScreenActive) return;`
    if screen.base.stopped || !screen.alt_screen_active {
        return;
    }
    let width = (screen.base.terminal.borrow().columns() as i64).max(1);
    let height = (screen.base.terminal.borrow().rows() as i64).max(1);
    let root = screen
        .layout_root
        .clone()
        .unwrap_or_else(|| screen.implicit_scroll_view.clone());
    let tui_id = screen.base.tui_id;
    let mut next_layout = render_layout_frame(&root, width, height, move || park_render_request(tui_id));
    // `if (this.refreshSearch(nextLayout))` — re-layout when the search UI
    // moved the viewport (seam owned by D-B1).
    if crate::alt_screen_input::refresh_search(screen, &next_layout) {
        next_layout = render_layout_frame(&root, width, height, move || park_render_request(tui_id));
    }

    // Resolve fake cursors before highlighting and compositing, which only
    // track SGR codes (tui-alt-screen.ts:1691-1692): strip the OSC133 zone
    // prefix run first, then resolve the fake-cursor markers.
    let stripped: Vec<String> = next_layout
        .lines
        .iter()
        .map(|line| strip_osc133_zone_prefix(line).to_string())
        .collect();
    let mut lines = screen.base.resolve_fake_cursors(stripped);
    lines = apply_search_highlights(screen, lines, &next_layout);
    lines = composite_scroll_to_end_indicator(screen, lines, &next_layout, width);
    lines = screen.base.composite_overlays(lines, width as usize, height as usize);
    // `if (screen.length > height) screen = screen.slice(screen.length - height);`
    if lines.len() > height as usize {
        let overflow = lines.len() - height as usize;
        lines.drain(..overflow);
    }
    lines = crate::alt_screen_selection::apply_selection(screen, lines, Some(&next_layout));
    lines = composite_flashes(screen, lines, width, height);

    let cursor_pos = TuiBase::extract_cursor_position(&mut lines, height as usize);
    lines = screen
        .base
        .apply_line_resets(lines)
        .into_iter()
        .map(|line| {
            if is_image_line(&line) || visible_width(&line) as i64 <= width {
                line
            } else {
                slice_by_column(&line, 0, width as usize, true)
            }
        })
        .collect();

    // ----- full vs differential write decision (tui-alt-screen.ts:1704-1726)
    let full_redraw = screen.previous_screen.is_empty()
        || screen.previous_screen_width != width
        || screen.previous_screen_height != height;
    // `screen.map((line, row) => line !== this.previousScreen[row])` —
    // `undefined !== undefined` is false, so a missing row on both sides is
    // "unchanged".
    let changed_rows: Vec<bool> = lines
        .iter()
        .enumerate()
        .map(|(row, line)| screen.previous_screen.get(row) != Some(line))
        .collect();
    let image_anchors_need_redraw = lines.iter().enumerate().any(|(row, line)| {
        changed_rows.get(row).copied().unwrap_or(false)
            && (is_image_line(line)
                || is_image_line(
                    screen.previous_screen.get(row).map(|s| s.as_str()).unwrap_or(""),
                ))
    });
    let is_wezterm = wezterm_env();
    let image_cells_need_redraw = !image_anchors_need_redraw
        && is_wezterm
        && screen.image_protocol == Some(ImageProtocol::Kitty)
        && changed_rows.iter().any(|changed| *changed)
        && lines.iter().enumerate().any(|(row, line)| {
            // WezTerm erases intersecting Kitty image cells when a later
            // row write touches a covered row.
            let Some(placement_rows) = get_kitty_image_placement_rows(line) else {
                return false;
            };
            let covered_end = row as i64 + placement_rows;
            let mut covered_row = row as i64;
            while covered_row < covered_end {
                if changed_rows.get(covered_row as usize).copied().unwrap_or(false) {
                    return true;
                }
                covered_row += 1;
            }
            false
        });
    let images_need_redraw = image_anchors_need_redraw || image_cells_need_redraw;
    let redraw_images = full_redraw || images_need_redraw;
    let had_uploaded_kitty_images = !screen.uploaded_kitty_images.is_empty();
    let (prepared_lines, evicted_image_deletion) =
        if redraw_images && screen.image_protocol == Some(ImageProtocol::Kitty) {
            prepare_kitty_screen(screen, &lines)
        } else {
            (lines.clone(), String::new())
        };

    // ----- the write itself (tui-alt-screen.ts:1728-1777)
    let mut buffer = String::from(BEGIN_SYNCHRONIZED_OUTPUT);
    if full_redraw {
        screen.base.full_redraw_count += 1;
        let clear_images =
            if screen.image_protocol == Some(ImageProtocol::Kitty) && had_uploaded_kitty_images {
                delete_all_kitty_placements().to_string()
            } else {
                delete_kitty_images(screen)
            };
        buffer.push_str(&clear_images);
        buffer.push_str("\x1b[2J");
    } else if images_need_redraw {
        if screen.image_protocol == Some(ImageProtocol::Iterm2) {
            buffer.push_str("\x1b[2J");
        } else if screen.image_protocol == Some(ImageProtocol::Kitty) {
            buffer.push_str(delete_all_kitty_placements());
        }
    }
    buffer.push_str(&evicted_image_deletion);

    // WezTerm erases intersecting Kitty image cells when a later row write
    // touches a covered row. Draw image placements after every clear and
    // text write so nothing later intersects them; preserve the existing
    // interleaved output for text-only frames and every other terminal.
    let draw_kitty_images_last = redraw_images
        && screen.image_protocol == Some(ImageProtocol::Kitty)
        && lines.iter().any(|line| is_image_line(line))
        && is_wezterm;
    // `if (!fullRedraw && !imagesNeedRedraw && screen[row] === previous[row])
    // continue;` — write every row on a full/image redraw, else only rows
    // whose content differs from the previous frame.
    let writes: Vec<bool> = (0..height as usize)
        .map(|row| {
            full_redraw
                || images_need_redraw
                || lines.get(row).map(|s| s.as_str())
                    != screen.previous_screen.get(row).map(|s| s.as_str())
        })
        .collect();
    let prepared_row =
        |row: usize| prepared_lines.get(row).map(|s| s.as_str()).unwrap_or("");
    if draw_kitty_images_last {
        for row in 0..height as usize {
            if !writes[row] {
                continue;
            }
            buffer.push_str(&format!("\x1b[{};1H\x1b[2K", row + 1));
        }
        for row in 0..height as usize {
            if !writes[row] || is_image_line(prepared_row(row)) {
                continue;
            }
            buffer.push_str(&format!("\x1b[{};1H", row + 1));
            buffer.push_str(prepared_row(row));
        }
        for row in 0..height as usize {
            if !writes[row] || !is_image_line(prepared_row(row)) {
                continue;
            }
            buffer.push_str(&format!("\x1b[{};1H", row + 1));
            buffer.push_str(prepared_row(row));
        }
    } else {
        for row in 0..height as usize {
            if !writes[row] {
                continue;
            }
            buffer.push_str(&format!("\x1b[{};1H\x1b[2K", row + 1));
            buffer.push_str(prepared_row(row));
        }
    }

    // Cursor: position the hardware cursor when the frame carried a
    // CURSOR_MARKER, else keep it hidden (the alt screen owns the cursor).
    if let Some((row, col)) = cursor_pos {
        buffer.push_str(&format!("\x1b[{};{}H", row + 1, width.min(col as i64) + 1));
        buffer.push_str(if screen.base.get_show_hardware_cursor() {
            "\x1b[?25h"
        } else {
            "\x1b[?25l"
        });
    } else {
        buffer.push_str("\x1b[?25l");
    }
    buffer.push_str(END_SYNCHRONIZED_OUTPUT);
    screen.base.terminal.borrow_mut().write(&buffer);

    // ----- bookkeeping (tui-alt-screen.ts:1781-1785)
    screen.previous_screen = lines;
    screen.previous_screen_width = width;
    screen.previous_screen_height = height;
    screen.current_layout = Some(next_layout);
}

/// `Boolean(process.env.WEZTERM_PANE) ||
/// process.env.TERM_PROGRAM?.toLowerCase() === "wezterm"`
/// (tui-alt-screen.ts:1712). An empty `WEZTERM_PANE` string is falsy in
/// JS, so only a non-empty value counts.
fn wezterm_env() -> bool {
    let wezterm_pane =
        std::env::var("WEZTERM_PANE").map(|value| !value.is_empty()).unwrap_or(false);
    let term_program = std::env::var("TERM_PROGRAM")
        .map(|value| value.to_lowercase() == "wezterm")
        .unwrap_or(false);
    wezterm_pane || term_program
}

// =============================================================================
// Search highlighting (tui-alt-screen.ts:1488-1586)
// =============================================================================

/// `applySearchTextHighlight(text, current)` (tui-alt-screen.ts:1488-1507):
/// wrap every plain-text run between ANSI escapes with the match style,
/// leaving the escape codes themselves untouched.
fn apply_search_text_highlight(screen: &TuiAltScreen, text: &str, current: bool) -> String {
    let style = if current {
        screen.search_current_match_style.clone()
    } else {
        screen.search_match_style.clone()
    };
    let mut result = String::new();
    let mut plain_start = 0usize;
    let mut index = 0usize;
    while index < text.len() {
        // JS advances one UTF-16 unit per miss; escape sequences only
        // start at the ASCII `\x1b`, so stepping by chars visits the same
        // match positions.
        let Some(ansi) = extract_ansi_code(text, index) else {
            index += text[index..].chars().next().map_or(1, |c| c.len_utf8());
            continue;
        };
        if index > plain_start {
            result.push_str(&style(&text[plain_start..index]));
        }
        result.push_str(ansi);
        index += ansi.len();
        plain_start = index;
    }
    if plain_start < text.len() {
        result.push_str(&style(&text[plain_start..]));
    }
    result
}

/// `SearchHighlightRange` (tui-alt-screen.ts:162-166).
struct SearchHighlightRange {
    start_col: i64,
    end_col: i64,
    current: bool,
}

/// `applySearchHighlights(screen, layout)` (tui-alt-screen.ts:1508-1586):
/// restyle the search matches of the active search over the primary
/// scroll-view box, clipping to the box rect/clip, the scrollbar column
/// and the terminal width. Ranges apply right-to-left per row.
fn apply_search_highlights(
    screen: &TuiAltScreen,
    screen_lines: Vec<String>,
    layout: &LayoutFrame,
) -> Vec<String> {
    let Some(search) = screen.active_search.as_ref() else {
        return screen_lines;
    };
    if search.selected_index < 0 || search.matches.is_empty() {
        return screen_lines;
    }
    let scroll_view = layout
        .primary_scroll_view
        .clone()
        .unwrap_or_else(|| screen.implicit_scroll_view_state.clone());
    let Some(box_) = get_scroll_view_box(layout, &scroll_view) else {
        return screen_lines;
    };

    let mut ranges_by_row: HashMap<i64, Vec<SearchHighlightRange>> = HashMap::new();
    let scrollbar_column = get_scrollbar_geometry(box_, false).map(|geometry| geometry.column);
    let min_row = 0.max(box_.rect.y).max(box_.clip.y);
    let max_row = (screen_lines.len() as i64)
        .min(box_.rect.y + box_.rect.height)
        .min(box_.clip.y + box_.clip.height);
    let min_column = 0.max(box_.rect.x).max(box_.clip.x);
    // `Math.min(terminal.columns, rect.x + rect.width, clip.x + clip.width,
    // scrollbarColumn ?? Infinity)`.
    let term_columns = screen.base.terminal.borrow().columns() as i64;
    let max_column = term_columns
        .min(box_.rect.x + box_.rect.width)
        .min(box_.clip.x + box_.clip.width)
        .min(scrollbar_column.unwrap_or(i64::MAX));
    let scroll_top = scroll_view.borrow().scroll_top();
    let min_content_row = scroll_top + min_row - box_.rect.y;
    let max_content_row = scroll_top + max_row - box_.rect.y - 1;

    // Binary search for the first match whose last segment row is at or
    // below `minContentRow`.
    let mut low: i64 = 0;
    let mut high: i64 = search.matches.len() as i64;
    while low < high {
        let middle = low + (high - low) / 2;
        let search_match = &search.matches[middle as usize];
        let last_row = search_match
            .segments
            .last()
            .map(|segment| segment.row as i64)
            .unwrap_or(-1);
        if last_row < min_content_row {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    for match_index in low..(search.matches.len() as i64) {
        let search_match = &search.matches[match_index as usize];
        let first_row = search_match
            .segments
            .first()
            .map(|segment| segment.row as i64)
            .unwrap_or(0);
        if first_row > max_content_row {
            break;
        }
        for segment in &search_match.segments {
            let row = box_.rect.y + segment.row as i64 - scroll_top;
            if row < min_row || row >= max_row {
                continue;
            }
            let start_col = min_column.max(box_.rect.x + segment.start_col as i64);
            let end_col = max_column.min(box_.rect.x + segment.end_col as i64);
            if end_col <= start_col {
                continue;
            }
            ranges_by_row.entry(row).or_default().push(SearchHighlightRange {
                start_col,
                end_col,
                current: match_index == search.selected_index,
            });
        }
    }

    let mut result = screen_lines;
    for (row, mut ranges) in ranges_by_row {
        if row < 0 || row as usize >= result.len() {
            continue;
        }
        let mut line = result[row as usize].clone();
        if is_image_line(&line) {
            continue;
        }
        let line_width = visible_width(&line) as i64;
        // Right-to-left so earlier slices stay column-accurate.
        ranges.sort_by(|a, b| b.start_col.cmp(&a.start_col));
        for range in ranges {
            let start_col = range.start_col.min(line_width);
            let end_col = range.end_col.min(line_width);
            if end_col <= start_col {
                continue;
            }
            let before = slice_by_column(&line, 0, start_col as usize, true);
            let highlighted =
                slice_by_column(&line, start_col as usize, (end_col - start_col) as usize, true);
            let after =
                slice_by_column(&line, end_col as usize, (line_width - end_col).max(0) as usize, true);
            line = format!(
                "{}{}{}",
                before,
                apply_search_text_highlight(screen, &highlighted, range.current),
                after
            );
        }
        result[row as usize] = line;
    }
    result
}

// =============================================================================
// Compositing (tui-alt-screen.ts:1641-1677)
// =============================================================================

/// `compositeScrollToEndIndicator(screen, layout, width)`
/// (tui-alt-screen.ts:1641-1663): center the "scroll to end" label on the
/// last clip row of the primary scroll view when it follows the end but is
/// currently scrolled away, and record its hit rect (consumed by D-B1's
/// mouse handling).
fn composite_scroll_to_end_indicator(
    screen: &mut TuiAltScreen,
    screen_lines: Vec<String>,
    layout: &LayoutFrame,
    width: i64,
) -> Vec<String> {
    screen.scroll_to_end_indicator_rect = None;
    let scroll_view = layout
        .primary_scroll_view
        .clone()
        .unwrap_or_else(|| screen.implicit_scroll_view_state.clone());
    let Some(indicator) = screen.scroll_to_end_indicator.clone() else {
        return screen_lines;
    };
    {
        let view = scroll_view.borrow();
        // TS also checks the `followEnd` constructor option here; the
        // shared-state trait does not expose it. `followingEnd` is only
        // ever set true when `followEnd` is set (scroll-view.ts:115-131,
        // 395, 411), so `is_following_end() == false` is the same gate for
        // every follow-end view (the implicit view is constructed with
        // `follow: "end"`). Divergence: a custom primary scroll view with
        // `follow: "none"` scrolled away from the end would show the
        // indicator here but not in TS — see the D-B3 report.
        if view.is_following_end() {
            return screen_lines;
        }
    }
    let Some(box_) = get_scroll_view_box(layout, &scroll_view) else {
        return screen_lines;
    };
    let clip = box_.clip;
    if clip.width <= 0 || clip.height <= 0 {
        return screen_lines;
    }
    let row = clip.y + clip.height - 1;
    if row < 0 || row >= screen_lines.len() as i64 {
        return screen_lines;
    }
    if is_image_line(screen_lines.get(row as usize).map(|s| s.as_str()).unwrap_or("")) {
        return screen_lines;
    }
    let scrollbar_column = get_scrollbar_geometry(box_, false).map(|geometry| geometry.column);
    let label = truncate_to_width(&indicator(), clip.width as usize, "", false);
    let label_width = visible_width(&label) as i64;
    // `Math.floor((clip.width - labelWidth) / 2)` — the difference is
    // non-negative (the label was truncated to `clip.width`).
    let column = clip.x + (clip.width - label_width) / 2;
    let right_edge = scrollbar_column.unwrap_or(clip.x + clip.width);
    let available_width = (right_edge - column).max(0);
    let text = truncate_to_width(&label, available_width as usize, "", false);
    let text_width = visible_width(&text) as i64;
    if text_width == 0 {
        return screen_lines;
    }
    let mut result = screen_lines;
    let base = result[row as usize].clone();
    result[row as usize] = composite_tui_line(
        &base,
        &text,
        column.max(0) as usize,
        text_width as usize,
        width as usize,
    );
    screen.scroll_to_end_indicator_rect =
        Some(ScrollToEndIndicatorRect { row, column, width: text_width });
    result
}

/// `compositeFlashes(screen, width, height)` (tui-alt-screen.ts:1665-1677):
/// right-align the flash container's lines over the top of the frame.
fn composite_flashes(
    screen: &mut TuiAltScreen,
    mut screen_lines: Vec<String>,
    width: i64,
    height: i64,
) -> Vec<String> {
    let flash_lines = screen.flashes.render(width as usize);
    // `.slice(-height)` keeps the LAST `height` entries.
    let flash_lines = if flash_lines.len() > height as usize {
        flash_lines[flash_lines.len() - height as usize..].to_vec()
    } else {
        flash_lines
    };
    if flash_lines.is_empty() {
        return screen_lines;
    }
    while screen_lines.len() < height as usize {
        screen_lines.push(String::new());
    }
    for row in 0..flash_lines.len() {
        let line = &flash_lines[row];
        let flash_width = visible_width(line) as i64;
        if flash_width == 0 {
            continue;
        }
        let base = screen_lines.get(row).cloned().unwrap_or_default();
        screen_lines[row] = composite_tui_line(
            &base,
            line,
            (width - flash_width).max(0) as usize,
            flash_width as usize,
            width as usize,
        );
    }
    screen_lines
}

// =============================================================================
// Kitty image cache (tui-alt-screen.ts:430-482)
// =============================================================================

thread_local! {
    /// Insertion order of `uploadedKittyImages` per screen (`tui_id`) —
    /// see the module docs. JS `Map` iteration order; the field itself is
    /// an unordered `HashMap`.
    static KITTY_CACHE_ORDER: RefCell<HashMap<usize, Vec<i64>>> =
        RefCell::new(HashMap::new());
}

/// Delete + set: refresh `image_id` to the end of the insertion order.
fn kitty_cache_track(tui_id: usize, image_id: i64) {
    KITTY_CACHE_ORDER.with(|table| {
        let mut table = table.borrow_mut();
        let order = table.entry(tui_id).or_default();
        order.retain(|id| *id != image_id);
        order.push(image_id);
    });
}

/// The insertion order restricted to ids still present in the live map
/// (lifecycle `clear()`s by D-B1 drop ids without telling this table).
fn kitty_cache_order_synced(tui_id: usize, map: &HashMap<i64, CachedKittyImage>) -> Vec<i64> {
    KITTY_CACHE_ORDER.with(|table| {
        let mut table = table.borrow_mut();
        let order = table.entry(tui_id).or_default();
        order.retain(|id| map.contains_key(id));
        order.clone()
    })
}

/// `deleteKittyImages()` (tui-alt-screen.ts:430-432): delete every
/// uploaded image (and its data) when the kitty protocol is active.
fn delete_kitty_images(screen: &TuiAltScreen) -> String {
    if screen.image_protocol == Some(ImageProtocol::Kitty) {
        delete_all_kitty_images().to_string()
    } else {
        String::new()
    }
}

/// `prepareKittyScreen(screen)` (tui-alt-screen.ts:434-482): replace image
/// transmission lines whose cached upload still matches with their
/// placement-only command, refresh the cache entries, and evict the
/// oldest offscreen images over the
/// [`crate::tui_alt_screen::MAX_CACHED_OFFSCREEN_KITTY_*`] budgets.
/// Returns the prepared lines plus the deletion sequence for evicted
/// images.
fn prepare_kitty_screen(
    screen: &mut TuiAltScreen,
    screen_lines: &[String],
) -> (Vec<String>, String) {
    let tui_id = screen.base.tui_id;
    let mut visible_image_ids: HashSet<i64> = HashSet::new();
    let mut prepared = Vec::with_capacity(screen_lines.len());
    for line in screen_lines {
        let Some(placement) = get_kitty_image_placement(line) else {
            prepared.push(line.clone());
            continue;
        };
        visible_image_ids.insert(placement.image_id);
        let cached_image = screen.uploaded_kitty_images.get(&placement.image_id).copied();
        let next_cached_image = CachedKittyImage {
            transmission_generation: placement.transmission_generation,
            transmission_bytes: placement.transmission_bytes,
            estimated_decoded_bytes: placement.estimated_decoded_bytes,
        };
        // Delete + set: a re-seen image refreshes to the end of the Map.
        screen.uploaded_kitty_images.remove(&placement.image_id);
        screen.uploaded_kitty_images.insert(placement.image_id, next_cached_image);
        kitty_cache_track(tui_id, placement.image_id);
        prepared.push(
            if cached_image
                .map(|cached| cached.transmission_generation == placement.transmission_generation)
                .unwrap_or(false)
            {
                placement.replacement_line
            } else {
                line.clone()
            },
        );
    }

    let mut cached_offscreen_image_count: i64 = 0;
    let mut cached_offscreen_transmission_bytes: i64 = 0;
    let mut cached_offscreen_decoded_bytes: i64 = 0;
    for (image_id, cached_image) in &screen.uploaded_kitty_images {
        if visible_image_ids.contains(image_id) {
            continue;
        }
        cached_offscreen_image_count += 1;
        cached_offscreen_transmission_bytes += cached_image.transmission_bytes;
        cached_offscreen_decoded_bytes += cached_image.estimated_decoded_bytes;
    }

    let mut evicted_image_deletion = String::new();
    for image_id in kitty_cache_order_synced(tui_id, &screen.uploaded_kitty_images) {
        if cached_offscreen_image_count <= MAX_CACHED_OFFSCREEN_KITTY_IMAGES as i64
            && cached_offscreen_transmission_bytes
                <= MAX_CACHED_OFFSCREEN_KITTY_TRANSMISSION_BYTES
            && cached_offscreen_decoded_bytes <= MAX_CACHED_OFFSCREEN_KITTY_DECODED_BYTES
        {
            break;
        }
        if visible_image_ids.contains(&image_id) {
            continue;
        }
        let Some(cached_image) = screen.uploaded_kitty_images.get(&image_id).copied() else {
            continue;
        };
        evicted_image_deletion.push_str(&delete_kitty_image(image_id));
        screen.uploaded_kitty_images.remove(&image_id);
        cached_offscreen_image_count -= 1;
        cached_offscreen_transmission_bytes -= cached_image.transmission_bytes;
        cached_offscreen_decoded_bytes -= cached_image.estimated_decoded_bytes;
    }
    (prepared, evicted_image_deletion)
}

// =============================================================================
// resetRenderState (tui-alt-screen.ts:484-488)
// =============================================================================

/// `resetRenderState()` (tui-alt-screen.ts:484-488): forget the previous
/// frame so the next render is a full redraw.
pub(crate) fn reset_render_state(screen: &mut TuiAltScreen) {
    screen.previous_screen.clear();
    screen.previous_screen_width = 0;
    screen.previous_screen_height = 0;
    screen.current_layout = None;
}

// =============================================================================
// Tests — write sequences verified against node probes over
// /tmp/pi-repo/packages/tui/dist (see /tmp/probe-b3/*.mjs).
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alt_screen_search::{
        AltScreenSearchComponent, AltScreenSearchIndex, AltScreenSearchMatch,
        AltScreenSearchSegment,
    };
    use crate::component::{Component, ComponentRef, CURSOR_MARKER, FAKE_CURSOR_END, FAKE_CURSOR_START};
    use crate::components::scroll_view::{
        ScrollView, ScrollViewFollow, ScrollViewOptions, ScrollViewScrollToOptions,
    };
    use crate::terminal::{InputHandler, ProgramStatus, ResizeHandler, Terminal};
    use crate::terminal_image as terminal_image_module;
    use crate::test_support::global_test_lock;
    use crate::tui::{OverlayAnchor, OverlayOptions, SizeValue};
    use crate::tui_alt_screen::{
        ActiveSearch, SearchSelectionMode, TuiAltScreenOptions,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    /// The line-reset suffix `applyLineResets` appends to non-image lines.
    const R: &str = "\x1b[0m\x1b]8;;\u{7}";

    /// Recording terminal double (pattern from tui_overlay.rs tests).
    struct RecordingTerminal {
        cols: Cell<usize>,
        rows: Cell<usize>,
        writes: Vec<String>,
    }

    impl RecordingTerminal {
        fn new(cols: usize, rows: usize) -> Self {
            RecordingTerminal { cols: Cell::new(cols), rows: Cell::new(rows), writes: Vec::new() }
        }
    }

    impl Terminal for RecordingTerminal {
        fn start(&mut self, _on_input: InputHandler, _on_resize: ResizeHandler) {}
        fn stop(&mut self) {}
        fn drain_input(&mut self, _max_ms: u64, _idle_ms: u64) {}
        fn write(&mut self, data: &str) {
            self.writes.push(data.to_string());
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

    /// Widget whose render output is exactly `lines` (the probe pattern).
    struct LinesWidget {
        lines: Vec<String>,
    }

    impl Component for LinesWidget {
        fn render(&mut self, _width: usize) -> Vec<String> {
            self.lines.clone()
        }
    }

    fn widget(lines: &[&str]) -> Rc<RefCell<LinesWidget>> {
        Rc::new(RefCell::new(LinesWidget { lines: lines.iter().map(|s| s.to_string()).collect() }))
    }

    fn new_screen(
        cols: usize,
        rows: usize,
        options: TuiAltScreenOptions,
        show_hardware_cursor: Option<bool>,
    ) -> (Rc<RefCell<TuiAltScreen>>, Rc<RefCell<RecordingTerminal>>) {
        let terminal: Rc<RefCell<RecordingTerminal>> =
            Rc::new(RefCell::new(RecordingTerminal::new(cols, rows)));
        let screen = TuiAltScreen::new(
            terminal.clone() as Rc<RefCell<dyn Terminal>>,
            show_hardware_cursor,
            None,
            options,
        );
        screen.borrow_mut().alt_screen_active = true;
        (screen, terminal)
    }

    /// The content written for `row` in a single-pass (non-WezTerm) frame,
    /// up to the next escape.
    fn row_content(buffer: &str, row: usize) -> String {
        let marker = format!("\x1b[{};1H\x1b[2K", row);
        let start = buffer.find(&marker).expect("row address") + marker.len();
        let rest = &buffer[start..];
        let end = rest.find("\x1b[").unwrap_or(rest.len());
        rest[..end].to_string()
    }

    // -------------------------------------------------------------------------
    // doRender: write sequences (probe render1.mjs)
    // -------------------------------------------------------------------------

    #[test]
    fn do_render_full_redraw_write_sequence() {
        let _guard = global_test_lock();
        let (screen, terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Iterm2);
        let child = widget(&["hello"]);
        screen.borrow_mut().add_child(child);

        screen.borrow_mut().render_now(false);
        let expected = format!(
            "{BEGIN}\x1b[2J\
             \x1b[1;1H\x1b[2Khello{R}\
             \x1b[2;1H\x1b[2K{R}\
             \x1b[3;1H\x1b[2K{R}\
             \x1b[?25l{END}",
            BEGIN = BEGIN_SYNCHRONIZED_OUTPUT,
            END = END_SYNCHRONIZED_OUTPUT,
            R = R,
        );
        assert_eq!(terminal.borrow().writes.last().unwrap(), &expected);
        assert_eq!(screen.borrow().base.full_redraw_count, 1);
        assert_eq!(screen.borrow().get_screen_lines().len(), 3);
    }

    #[test]
    fn do_render_differential_writes_only_changed_rows() {
        let _guard = global_test_lock();
        let (screen, terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Iterm2);
        let child = widget(&["hello"]);
        screen.borrow_mut().add_child(child.clone());
        screen.borrow_mut().render_now(false);

        child.borrow_mut().lines = vec!["hello".into(), "second".into()];
        terminal.borrow_mut().writes.clear();
        screen.borrow_mut().render_now(false);
        // Only the changed row is rewritten (TS probe: the row-1 "hello"
        // line matches previousScreen and is skipped by the changedRows
        // diff; tui-alt-screen.ts writes `\x1b[2;1H\x1b[2Ksecond` only).
        let expected = format!(
            "{BEGIN}\
             \x1b[2;1H\x1b[2Ksecond{R}\
             \x1b[?25l{END}",
            BEGIN = BEGIN_SYNCHRONIZED_OUTPUT,
            END = END_SYNCHRONIZED_OUTPUT,
            R = R,
        );
        assert_eq!(terminal.borrow().writes.last().unwrap(), &expected);
        assert_eq!(screen.borrow().base.full_redraw_count, 1);
    }

    #[test]
    fn do_render_unchanged_frame_writes_cursor_hide_only() {
        let _guard = global_test_lock();
        let (screen, terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Iterm2);
        screen.borrow_mut().add_child(widget(&["hello"]));
        screen.borrow_mut().render_now(false);

        terminal.borrow_mut().writes.clear();
        screen.borrow_mut().render_now(false);
        let expected = format!(
            "{BEGIN}\x1b[?25l{END}",
            BEGIN = BEGIN_SYNCHRONIZED_OUTPUT,
            END = END_SYNCHRONIZED_OUTPUT,
        );
        assert_eq!(terminal.borrow().writes.last().unwrap(), &expected);
    }

    #[test]
    fn do_render_resize_forces_full_redraw() {
        let _guard = global_test_lock();
        let (screen, terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Iterm2);
        screen.borrow_mut().add_child(widget(&["hello"]));
        screen.borrow_mut().render_now(false);

        terminal.borrow().set_cols(10);
        terminal.borrow().set_rows(2);
        terminal.borrow_mut().writes.clear();
        screen.borrow_mut().render_now(false);
        let expected = format!(
            "{BEGIN}\x1b[2J\
             \x1b[1;1H\x1b[2Khello{R}\
             \x1b[2;1H\x1b[2K{R}\
             \x1b[?25l{END}",
            BEGIN = BEGIN_SYNCHRONIZED_OUTPUT,
            END = END_SYNCHRONIZED_OUTPUT,
            R = R,
        );
        assert_eq!(terminal.borrow().writes.last().unwrap(), &expected);
        assert_eq!(screen.borrow().base.full_redraw_count, 2);
        assert_eq!(screen.borrow().previous_screen_width, 10);
        assert_eq!(screen.borrow().previous_screen_height, 2);
    }

    impl RecordingTerminal {
        fn set_cols(&self, cols: usize) {
            self.cols.set(cols);
        }
        fn set_rows(&self, rows: usize) {
            self.rows.set(rows);
        }
    }

    #[test]
    fn do_render_guard_requires_active_alt_screen() {
        let _guard = global_test_lock();
        let (screen, terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().alt_screen_active = false;
        screen.borrow_mut().add_child(widget(&["hello"]));
        screen.borrow_mut().render_now(false);
        assert!(terminal.borrow().writes.is_empty());

        screen.borrow_mut().alt_screen_active = true;
        screen.borrow_mut().render_now(false);
        assert_eq!(terminal.borrow().writes.len(), 1);

        // `if (this.stopped ...) return;`
        screen.borrow_mut().base.stopped = true;
        terminal.borrow_mut().writes.clear();
        screen.borrow_mut().render_now(false);
        assert!(terminal.borrow().writes.is_empty());
    }

    #[test]
    fn reset_render_state_forces_next_full_redraw() {
        let _guard = global_test_lock();
        let (screen, terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Iterm2);
        screen.borrow_mut().add_child(widget(&["hello"]));
        screen.borrow_mut().render_now(false);
        assert_eq!(screen.borrow().base.full_redraw_count, 1);

        // `renderNow(true)` → `resetRenderState()` → full redraw next frame.
        screen.borrow_mut().render_now(true);
        assert!(terminal.borrow().last_write().contains("\x1b[2J"));
        assert_eq!(screen.borrow().base.full_redraw_count, 2);
        assert_eq!(screen.borrow().get_screen_lines().len(), 3);
    }

    impl RecordingTerminal {
        fn last_write(&self) -> &str {
            self.writes.last().map(|s| s.as_str()).unwrap_or("")
        }
    }

    // -------------------------------------------------------------------------
    // doRender: height slice keeps the LAST rows (overlays can extend the
    // frame past the terminal height; tui.ts:1373-1376 padding math)
    // -------------------------------------------------------------------------

    #[test]
    fn do_render_height_slice_keeps_last_rows() {
        let _guard = global_test_lock();
        let (screen, terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Iterm2);
        screen.borrow_mut().add_child(widget(&["a", "b", "c"]));

        let overlay = widget(&["O1", "O2", "O3", "O4"]);
        screen.borrow_mut().base.show_overlay(
            overlay as ComponentRef,
            Some(OverlayOptions {
                width: Some(SizeValue::Number(10.0)),
                anchor: Some(OverlayAnchor::TopLeft),
                offset_y: Some(1),
                ..Default::default()
            }),
        );
        screen.borrow_mut().render_now(false);

        let buffer = terminal.borrow().last_write().to_string();
        // TS probe ground truth (20x3, content a/b/c, overlay O1..O4 width
        // 10 topLeft offsetY 1): the working frame is padded past the
        // terminal height and the slice keeps the LAST 3 rows, which are
        // the overlay lines O1, O2, O3 (O4 is past the frame; the content
        // rows a/b/c are sliced away). The write is a full redraw with
        // O1/O2/O3 on terminal rows 1/2/3.
        let lines = screen.borrow().get_screen_lines();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("O1"));
        assert!(lines[1].contains("O2"));
        assert!(lines[2].contains("O3"));
        assert!(!buffer.contains("O4"));
        // The content rows ("a", "b", "c") were sliced away.
        assert!(!buffer.contains("\x1b[2Ka\x1b[0m"));
        assert!(!buffer.contains("\x1b[2Kb\x1b[0m"));
        assert!(!buffer.contains("\x1b[2Kc\x1b[0m"));
        assert!(buffer.contains("\x1b[1;1H\x1b[2K"));
        assert!(buffer.contains("\x1b[3;1H\x1b[2K"));
        assert!(!buffer.contains("\x1b[4;1H"));
    }

    // -------------------------------------------------------------------------
    // doRender: OSC133 strip + fake cursor resolution + hardware cursor
    // -------------------------------------------------------------------------

    #[test]
    fn do_render_strips_osc133_before_fake_cursor_resolution() {
        let _guard = global_test_lock();
        let (screen, terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Iterm2);
        let line = format!("\x1b]133;A\x07{FAKE_CURSOR_START}${FAKE_CURSOR_END} tail");
        screen.borrow_mut().add_child(widget(&[&line]));
        screen.borrow_mut().render_now(false);

        let buffer = terminal.borrow().last_write().to_string();
        // The OSC133 zone prefix is stripped before resolveFakeCursors, so
        // the fake cursor resolves to reverse video at column 0.
        assert!(!buffer.contains("\x1b]133;A"));
        assert!(buffer.contains("\x1b[7m$\x1b[27m"));
        // The raw markers never reach the terminal.
        assert!(!buffer.contains(FAKE_CURSOR_START));
        assert!(!buffer.contains(FAKE_CURSOR_END));
    }

    #[test]
    fn do_render_positions_hardware_cursor_from_marker() {
        let _guard = global_test_lock();
        let (screen, terminal) =
            new_screen(20, 3, TuiAltScreenOptions::default(), Some(true));
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Iterm2);
        let line = format!("ab{CURSOR_MARKER}cd");
        screen.borrow_mut().add_child(widget(&[&line]));
        screen.borrow_mut().render_now(false);

        let buffer = terminal.borrow().last_write().to_string();
        // Marker at visible column 2 of row 0 → `\x1b[1;3H`, then shown.
        assert!(buffer.contains("\x1b[1;3H\x1b[?25h"));
        assert!(!buffer.contains(CURSOR_MARKER));
        assert!(buffer.contains("abcd"));

        // With the hardware cursor hidden, the position is still emitted
        // but the cursor stays off.
        let (screen2, terminal2) =
            new_screen(20, 3, TuiAltScreenOptions::default(), Some(false));
        screen2.borrow_mut().image_protocol = Some(ImageProtocol::Iterm2);
        screen2.borrow_mut().add_child(widget(&[&line]));
        screen2.borrow_mut().render_now(false);
        let buffer2 = terminal2.borrow().last_write().to_string();
        assert!(buffer2.contains("\x1b[1;3H\x1b[?25l"));
    }

    // -------------------------------------------------------------------------
    // Search highlighting (probe render4.mjs)
    // -------------------------------------------------------------------------

    fn search_with_matches(matches: Vec<AltScreenSearchMatch>, selected_index: i64) -> ActiveSearch {
        ActiveSearch {
            component: Rc::new(RefCell::new(AltScreenSearchComponent::new(Box::new(|_q: &str| {})))),
            index: AltScreenSearchIndex::new(),
            overlay: None,
            query: "beta".to_string(),
            matches,
            selected_index,
            selected_key: None,
            anchor_row: 0,
            selection_mode: SearchSelectionMode::Retain,
        }
    }

    fn segment(row: usize, start_col: usize, end_col: usize) -> AltScreenSearchSegment {
        AltScreenSearchSegment { row, start_col, end_col }
    }

    #[test]
    fn apply_search_highlights_matches_the_ts_output() {
        let _guard = global_test_lock();
        let (screen, _terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().add_child(widget(&["alpha beta gamma", "beta beta", "gamma"]));
        screen.borrow_mut().render_now(false);
        let layout = screen.borrow().current_layout.clone().expect("layout");
        let frame = screen.borrow().previous_screen.clone();
        assert_eq!(
            frame,
            vec![
                format!("alpha beta gamma{R}"),
                format!("beta beta{R}"),
                format!("gamma{R}"),
            ],
            "R = {R}",
        );

        screen.borrow_mut().active_search = Some(search_with_matches(
            vec![
                AltScreenSearchMatch { segments: vec![segment(0, 6, 10)] },
                AltScreenSearchMatch {
                    segments: vec![segment(0, 12, 16), segment(1, 0, 4)],
                },
                AltScreenSearchMatch { segments: vec![segment(1, 5, 9)] },
            ],
            1,
        ));
        let highlighted = {
            let borrowed = screen.borrow();
            apply_search_highlights(&borrowed, frame, &layout)
        };
        // Byte-for-byte the node-probe output (render4.mjs): the current
        // match is bold+reverse, the others underlined, applied
        // right-to-left per row.
        assert_eq!(
            highlighted,
            vec![
                "alpha \x1b[4mbeta\x1b[24m g\x1b[1;7mamma".to_string(),
                "\x1b[1;7mbeta\x1b[22;27m \x1b[4mbeta".to_string(),
                format!("gamma{R}"),
            ],
            "R = {R}",
        );
    }

    #[test]
    fn apply_search_highlights_skips_without_selection_or_matches() {
        let _guard = global_test_lock();
        let (screen, _terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().add_child(widget(&["alpha beta"]));
        screen.borrow_mut().render_now(false);
        let layout = screen.borrow().current_layout.clone().expect("layout");
        let frame = screen.borrow().previous_screen.clone();

        // No active search.
        let out = {
            let borrowed = screen.borrow();
            apply_search_highlights(&borrowed, frame.clone(), &layout)
        };
        assert_eq!(out, frame);

        // selectedIndex < 0.
        screen.borrow_mut().active_search =
            Some(search_with_matches(vec![AltScreenSearchMatch { segments: vec![segment(0, 6, 10)] }], -1));
        let out = {
            let borrowed = screen.borrow();
            apply_search_highlights(&borrowed, frame.clone(), &layout)
        };
        assert_eq!(out, frame);

        // Empty matches.
        screen.borrow_mut().active_search = Some(search_with_matches(Vec::new(), 0));
        let out = {
            let borrowed = screen.borrow();
            apply_search_highlights(&borrowed, frame.clone(), &layout)
        };
        assert_eq!(out, frame);
    }

    #[test]
    fn do_render_applies_search_highlights() {
        let _guard = global_test_lock();
        let (screen, terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Iterm2);
        screen.borrow_mut().add_child(widget(&["alpha beta gamma"]));
        screen.borrow_mut().render_now(false);

        screen.borrow_mut().active_search = Some(search_with_matches(
            vec![AltScreenSearchMatch { segments: vec![segment(0, 6, 10)] }],
            0,
        ));
        terminal.borrow_mut().writes.clear();
        screen.borrow_mut().render_now(false);
        let buffer = terminal.borrow().last_write().to_string();
        // Current match (selectedIndex 0): bold + reverse around "beta".
        assert!(buffer.contains("alpha \x1b[1;7mbeta\x1b[22;27m gamma"));
        // The refreshSearch seam (D-B1) currently reports no re-layout;
        // the highlight pass still runs over the single layout pass.
        assert!(screen.borrow().current_layout.is_some());
    }

    // -------------------------------------------------------------------------
    // Scroll-to-end indicator (probe render4.mjs)
    // -------------------------------------------------------------------------

    #[test]
    fn composite_scroll_to_end_indicator_centers_on_last_clip_row() {
        let _guard = global_test_lock();
        let options = TuiAltScreenOptions {
            scroll_to_end_indicator: Some(Rc::new(|| "↓ end".to_string())),
            ..Default::default()
        };
        let (screen, terminal) = new_screen(20, 3, options, None);
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Iterm2);

        // A custom layout root scroll view (follow: end, primary) so the
        // test can drive the follow latch through the shared state.
        let content = widget(&["l0", "l1", "l2", "l3", "l4", "l5", "l6", "l7", "l8", "l9"]);
        let scroll_view = ScrollView::new(
            content as ComponentRef,
            ScrollViewOptions {
                follow: Some(ScrollViewFollow::End),
                primary: true,
                ..Default::default()
            },
        );
        let state = scroll_view.state();
        screen
            .borrow_mut()
            .set_layout_root(Some(Rc::new(RefCell::new(scroll_view)) as ComponentRef));

        // Following the end: no indicator, rect cleared.
        screen.borrow_mut().render_now(false);
        assert!(screen.borrow().scroll_to_end_indicator_rect.is_none());
        assert!(!terminal.borrow().last_write().contains("↓ end"));

        // Scroll away from the end (scrollTop 5 of max 7).
        state.borrow_mut().scroll_to(5, ScrollViewScrollToOptions::default());
        screen.borrow_mut().render_now(false);
        // clip {0,0,20,3}: row = 2, label width 5 → column = floor(15/2) = 7.
        assert_eq!(
            screen.borrow().scroll_to_end_indicator_rect,
            Some(ScrollToEndIndicatorRect { row: 2, column: 7, width: 5 })
        );
        let buffer = terminal.borrow().last_write().to_string();
        let row3 = row_content(&buffer, 3);
        assert!(row3.starts_with("l7"), "row 3 shows l7, got {row3:?}");
        assert!(buffer.contains("↓ end"));
    }

    #[test]
    fn composite_scroll_to_end_indicator_requires_callback() {
        let _guard = global_test_lock();
        let (screen, _terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().add_child(widget(&["a", "b", "c", "d"]));
        screen.borrow_mut().render_now(false);
        let layout = screen.borrow().current_layout.clone().expect("layout");
        let frame = layout.lines.clone();
        // No callback configured → unchanged, rect stays None.
        let out = composite_scroll_to_end_indicator(&mut screen.borrow_mut(), frame.clone(), &layout, 20);
        assert_eq!(out, frame);
        assert!(screen.borrow().scroll_to_end_indicator_rect.is_none());
    }

    // -------------------------------------------------------------------------
    // Flash compositing (probe render4.mjs)
    // -------------------------------------------------------------------------

    #[test]
    fn composite_flashes_right_aligns_over_the_frame() {
        let _guard = global_test_lock();
        crate::clock::reset_for_test();
        let (screen, terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Iterm2);
        screen.borrow_mut().add_child(widget(&["l5", "l6", "l7"]));
        screen.borrow_mut().render_now(false);

        screen.borrow_mut().flash("hi", None);
        terminal.borrow_mut().writes.clear();
        screen.borrow_mut().render_now(false);
        let buffer = terminal.borrow().last_write().to_string();
        // The flash renders as reverse-video " hi " (4 columns) on the
        // first frame row, right-aligned at column 20 - 4 = 16.
        let row1 = row_content(&buffer, 1);
        assert!(row1.starts_with("l5"), "row 1 shows l5, got {row1:?}");
        assert!(row1.len() >= 16, "row 1 padded to the flash column: {row1:?}");
        assert!(buffer.contains("\x1b[7m hi "));
        // Rows 2-3 unchanged → not rewritten in the differential frame.
        assert!(!buffer.contains("\x1b[2;1H"));
    }

    // -------------------------------------------------------------------------
    // Kitty image cache (probes render2.mjs / render3.mjs)
    // -------------------------------------------------------------------------

    fn kitty_metadata(image_id: i64, rows: i64) -> crate::terminal_image::KittyImageMetadata {
        crate::terminal_image::KittyImageMetadata {
            image_id,
            columns: 10,
            rows,
            width_px: 90,
            height_px: 54,
        }
    }

    fn image_line(image_id: i64, rows: i64) -> String {
        format!("img:\x1b_Ga=T,i={image_id},c=10,r={rows},m=0;DATA\x1b\\")
    }

    #[test]
    fn do_render_kitty_full_redraw_then_cached_placement() {
        let _guard = global_test_lock();
        terminal_image_module::reset_for_test();
        let (screen, terminal) = new_screen(20, 4, TuiAltScreenOptions::default(), None);
        // TS: imageProtocol = "kitty" (set here instead of via start()).
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Kitty);
        // imageProtocol stays at the skeleton's Kitty placeholder.
        terminal_image_module::register_kitty_image_metadata(kitty_metadata(9, 3));
        terminal_image_module::register_kitty_image_metadata(kitty_metadata(8, 1));
        let child = widget(&[]);
        child.borrow_mut().lines =
            vec![image_line(9, 3), "r1".into(), "r2".into(), image_line(8, 1)];
        screen.borrow_mut().add_child(child.clone());

        // First render: full redraw, no uploaded images yet →
        // deleteAllKittyImages + 2J, image rows carry the full
        // transmissions (no cached generation yet).
        screen.borrow_mut().render_now(false);
        let first = terminal.borrow().last_write().to_string();
        let expected_first = format!(
            "{BEGIN}{CLEAR}\x1b[2J\
             \x1b[1;1H\x1b[2K{IMG9}\
             \x1b[2;1H\x1b[2Kr1{R}\
             \x1b[3;1H\x1b[2Kr2{R}\
             \x1b[4;1H\x1b[2K{IMG8}\
             \x1b[?25l{END}",
            BEGIN = BEGIN_SYNCHRONIZED_OUTPUT,
            END = END_SYNCHRONIZED_OUTPUT,
            CLEAR = delete_all_kitty_images(),
            IMG9 = image_line(9, 3),
            IMG8 = image_line(8, 1),
            R = R,
        );
        assert_eq!(first, expected_first);
        assert_eq!(screen.borrow().uploaded_kitty_images.len(), 2);

        // No change: nothing rewritten (the write comparison uses the
        // pre-preparation frame).
        terminal.borrow_mut().writes.clear();
        screen.borrow_mut().render_now(false);
        assert_eq!(
            terminal.borrow().last_write(),
            format!("{BEGIN}\x1b[?25l{END}",
                BEGIN = BEGIN_SYNCHRONIZED_OUTPUT, END = END_SYNCHRONIZED_OUTPUT)
        );

        // Replace the second image with text: that row changed and its
        // previous content was an image line → imageAnchorsNeedRedraw →
        // deleteAllKittyPlacements, every row rewritten, and the surviving
        // image's cached generation matches → placement-only line.
        child.borrow_mut().lines =
            vec![image_line(9, 3), "r1".into(), "r2".into(), "CHANGED".into()];
        terminal.borrow_mut().writes.clear();
        screen.borrow_mut().render_now(false);
        let expected_third = format!(
            "{BEGIN}\x1b_Ga=d,d=a,q=2\x1b\\\
             \x1b[1;1H\x1b[2Kimg:\x1b_Ga=p,q=2,i=9,c=10,r=3\x1b\\\
             \x1b[2;1H\x1b[2Kr1{R}\
             \x1b[3;1H\x1b[2Kr2{R}\
             \x1b[4;1H\x1b[2KCHANGED{R}\
             \x1b[?25l{END}",
            BEGIN = BEGIN_SYNCHRONIZED_OUTPUT,
            END = END_SYNCHRONIZED_OUTPUT,
            R = R,
        );
        assert_eq!(terminal.borrow().last_write(), &expected_third);
        // Image 8 went offscreen but stays cached (within budget).
        assert!(screen.borrow().uploaded_kitty_images.contains_key(&8));
        terminal_image_module::reset_for_test();
    }

    #[test]
    fn kitty_eviction_through_do_render_count_budget() {
        let _guard = global_test_lock();
        terminal_image_module::reset_for_test();
        let (screen, terminal) = new_screen(20, 2, TuiAltScreenOptions::default(), None);
        // TS: imageProtocol = "kitty" (set here instead of via start()).
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Kitty);
        let child = widget(&[]);
        screen.borrow_mut().add_child(child.clone());

        // Upload 18 distinct images one frame at a time (each frame change
        // triggers imageAnchorsNeedRedraw → prepareKittyScreen). The
        // offscreen budget is 16, so the 18th upload evicts the oldest.
        for id in 100..118i64 {
            terminal_image_module::register_kitty_image_metadata(kitty_metadata(id, 1));
            child.borrow_mut().lines = vec![image_line(id, 1), format!("x{id}")];
            screen.borrow_mut().render_now(false);
        }
        let last = terminal.borrow().last_write().to_string();
        // Oldest (100) evicted; its delete sequence rides the frame.
        assert!(last.contains(&delete_kitty_image(100)));
        assert!(!last.contains(&delete_kitty_image(101)));
        let map = &screen.borrow().uploaded_kitty_images;
        assert!(!map.contains_key(&100));
        assert!(map.contains_key(&101));
        assert!(map.contains_key(&117));
        assert_eq!(map.len(), 17);
        terminal_image_module::reset_for_test();
    }

    #[test]
    fn kitty_eviction_budgets_and_order() {
        let _guard = global_test_lock();
        terminal_image_module::reset_for_test();
        let (screen, _terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        let tui_id = screen.borrow().base.tui_id;

        // Transmission-byte budget: 3 offscreen images at 20 MB each
        // exceed the 32 MB budget → evict the two oldest, insertion order.
        let mut seeded = screen.borrow_mut();
        for id in 1..=3i64 {
            seeded.uploaded_kitty_images.insert(
                id,
                CachedKittyImage {
                    transmission_generation: 1,
                    transmission_bytes: 20 * 1024 * 1024,
                    estimated_decoded_bytes: 0,
                },
            );
            kitty_cache_track(tui_id, id);
        }
        let (lines, evicted) =
            prepare_kitty_screen(&mut seeded, &["plain".to_string(), String::new()]);
        drop(seeded);
        assert_eq!(lines, vec!["plain".to_string(), String::new()]);
        assert_eq!(evicted, format!("{}{}", delete_kitty_image(1), delete_kitty_image(2)));
        assert_eq!(screen.borrow().uploaded_kitty_images.len(), 1);
        assert!(screen.borrow().uploaded_kitty_images.contains_key(&3));

        // Count budget: 17 small offscreen images → evict exactly the
        // oldest.
        let mut seeded = screen.borrow_mut();
        seeded.uploaded_kitty_images.clear();
        for id in 10..=26i64 {
            seeded.uploaded_kitty_images.insert(
                id,
                CachedKittyImage {
                    transmission_generation: 1,
                    transmission_bytes: 10,
                    estimated_decoded_bytes: 10,
                },
            );
            kitty_cache_track(tui_id, id);
        }
        let (_, evicted) = prepare_kitty_screen(&mut seeded, &["plain".to_string()]);
        drop(seeded);
        assert_eq!(evicted, delete_kitty_image(10));
        assert_eq!(screen.borrow().uploaded_kitty_images.len(), 16);

        // Decoded-byte budget: 2 offscreen images at 40 MB decoded exceed
        // the 64 MB budget → evict the oldest.
        let mut seeded = screen.borrow_mut();
        seeded.uploaded_kitty_images.clear();
        for id in 30..=31i64 {
            seeded.uploaded_kitty_images.insert(
                id,
                CachedKittyImage {
                    transmission_generation: 1,
                    transmission_bytes: 0,
                    estimated_decoded_bytes: 40 * 1024 * 1024,
                },
            );
            kitty_cache_track(tui_id, id);
        }
        let (_, evicted) = prepare_kitty_screen(&mut seeded, &["plain".to_string()]);
        drop(seeded);
        assert_eq!(evicted, delete_kitty_image(30));
        assert_eq!(screen.borrow().uploaded_kitty_images.len(), 1);
        terminal_image_module::reset_for_test();
    }

    #[test]
    fn kitty_eviction_never_touches_visible_images() {
        let _guard = global_test_lock();
        terminal_image_module::reset_for_test();
        let (screen, _terminal) = new_screen(20, 3, TuiAltScreenOptions::default(), None);
        let tui_id = screen.borrow().base.tui_id;
        terminal_image_module::register_kitty_image_metadata(kitty_metadata(2, 1));

        // Image 2 is on-screen (and re-seen, refreshing its cache entry to
        // the end of the insertion order); image 1 is offscreen and over
        // the transmission budget — only 1 is evicted.
        let mut seeded = screen.borrow_mut();
        seeded.uploaded_kitty_images.insert(
            1,
            CachedKittyImage {
                transmission_generation: 1,
                transmission_bytes: 40 * 1024 * 1024,
                estimated_decoded_bytes: 0,
            },
        );
        seeded.uploaded_kitty_images.insert(
            2,
            CachedKittyImage {
                transmission_generation: 0, // stale generation
                transmission_bytes: 0,
                estimated_decoded_bytes: 0,
            },
        );
        kitty_cache_track(tui_id, 1);
        kitty_cache_track(tui_id, 2);
        let visible = image_line(2, 1);
        let (lines, evicted) = prepare_kitty_screen(&mut seeded, &[visible.clone()]);
        drop(seeded);
        assert_eq!(evicted, delete_kitty_image(1));
        // Stale cached generation → the full transmission is kept.
        assert_eq!(lines, vec![visible]);
        let map = &screen.borrow().uploaded_kitty_images;
        assert!(!map.contains_key(&1));
        let cached = map.get(&2).expect("visible image kept");
        assert_eq!(cached.transmission_generation, 1);
        terminal_image_module::reset_for_test();
    }

    // -------------------------------------------------------------------------
    // WezTerm interleave (probe render3.mjs, WEZTERM_PANE set)
    // -------------------------------------------------------------------------

    #[test]
    fn wezterm_env_detection() {
        let _guard = global_test_lock();
        std::env::remove_var("WEZTERM_PANE");
        std::env::remove_var("TERM_PROGRAM");
        assert!(!wezterm_env());
        std::env::set_var("WEZTERM_PANE", "1");
        assert!(wezterm_env());
        // An empty string is falsy in JS (`Boolean("") === false`).
        std::env::set_var("WEZTERM_PANE", "");
        assert!(!wezterm_env());
        std::env::remove_var("WEZTERM_PANE");
        std::env::set_var("TERM_PROGRAM", "WezTerm");
        assert!(wezterm_env());
        std::env::set_var("TERM_PROGRAM", "iTerm.app");
        assert!(!wezterm_env());
        std::env::remove_var("TERM_PROGRAM");
    }

    #[test]
    fn do_render_wezterm_draws_kitty_images_last() {
        let _guard = global_test_lock();
        terminal_image_module::reset_for_test();
        std::env::remove_var("TERM_PROGRAM");
        std::env::set_var("WEZTERM_PANE", "0");
        let (screen, terminal) = new_screen(20, 5, TuiAltScreenOptions::default(), None);
        // TS: imageProtocol = "kitty" (set here instead of via start()).
        screen.borrow_mut().image_protocol = Some(ImageProtocol::Kitty);
        terminal_image_module::register_kitty_image_metadata(kitty_metadata(9, 3));
        let child = widget(&[]);
        child.borrow_mut().lines = vec![image_line(9, 3), "r1".into(), "r2".into(), "r3".into()];
        screen.borrow_mut().add_child(child.clone());

        // Full redraw with an image on WezTerm: clear every row first,
        // then write the text rows, then the image rows.
        screen.borrow_mut().render_now(false);
        let expected_first = format!(
            "{BEGIN}{CLEAR}\x1b[2J\
             \x1b[1;1H\x1b[2K\x1b[2;1H\x1b[2K\x1b[3;1H\x1b[2K\x1b[4;1H\x1b[2K\x1b[5;1H\x1b[2K\
             \x1b[2;1Hr1{R}\x1b[3;1Hr2{R}\x1b[4;1Hr3{R}\x1b[5;1H{R}\
             \x1b[1;1H{IMG9}\
             \x1b[?25l{END}",
            BEGIN = BEGIN_SYNCHRONIZED_OUTPUT,
            END = END_SYNCHRONIZED_OUTPUT,
            CLEAR = delete_all_kitty_images(),
            IMG9 = image_line(9, 3),
            R = R,
        );
        assert_eq!(terminal.borrow().last_write(), &expected_first);

        // Change only a row NOT covered by the 3-row image (rows 0-2):
        // plain differential, no image redraw.
        child.borrow_mut().lines[3] = "CHANGED".into();
        terminal.borrow_mut().writes.clear();
        screen.borrow_mut().render_now(false);
        let expected_second = format!(
            "{BEGIN}\x1b[4;1H\x1b[2KCHANGED{R}\x1b[?25l{END}",
            BEGIN = BEGIN_SYNCHRONIZED_OUTPUT,
            END = END_SYNCHRONIZED_OUTPUT,
            R = R,
        );
        assert_eq!(terminal.borrow().last_write(), &expected_second);

        // Change a COVERED row (row 1): imageCellsNeedRedraw →
        // deleteAllKittyPlacements + the three-pass interleave with the
        // cached placement-only image line.
        child.borrow_mut().lines[1] = "r1x".into();
        terminal.borrow_mut().writes.clear();
        screen.borrow_mut().render_now(false);
        let expected_third = format!(
            "{BEGIN}\x1b_Ga=d,d=a,q=2\x1b\\\
             \x1b[1;1H\x1b[2K\x1b[2;1H\x1b[2K\x1b[3;1H\x1b[2K\x1b[4;1H\x1b[2K\x1b[5;1H\x1b[2K\
             \x1b[2;1Hr1x{R}\x1b[3;1Hr2{R}\x1b[4;1HCHANGED{R}\x1b[5;1H{R}\
             \x1b[1;1Himg:\x1b_Ga=p,q=2,i=9,c=10,r=3\x1b\\\
             \x1b[?25l{END}",
            BEGIN = BEGIN_SYNCHRONIZED_OUTPUT,
            END = END_SYNCHRONIZED_OUTPUT,
            R = R,
        );
        assert_eq!(terminal.borrow().last_write(), &expected_third);
        std::env::remove_var("WEZTERM_PANE");
        terminal_image_module::reset_for_test();
    }
}
