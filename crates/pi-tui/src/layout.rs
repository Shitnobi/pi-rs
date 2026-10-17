//! Port of `packages/tui/src/layout.ts` — the flex layout solver.
//!
//! ## Design decisions (documented divergences from the TS shapes)
//!
//! - **Tree ownership**: the TS `LayoutBox` has a `parent?: LayoutBox`
//!   back-reference, but it is *write-only* — a repo-wide grep of
//!   `packages/tui/src` shows `parent` is only ever assigned (`withParent`,
//!   `childBox.parent = box`, layout.ts:92 and :165) and never read. The
//!   Rust port therefore models the box tree as a plainly-owned tree
//!   (`LayoutBox::children: Vec<LayoutBox>`); the mutation the TS performs
//!   during layout (`translateBox` / `updateClips`) happens before the child
//!   is pushed into its parent, which is observably identical. `withParent`
//!   is consequently not ported. If a later tranche needs parent links, a
//!   parent-index map can be layered on top without changing observable
//!   behavior.
//! - **Numerics**: all sizes/coordinates are `i64` (JS number semantics);
//!   raw TS arithmetic (`Math.round` thumb geometry, kitty pixel math)
//!   stays `f64`.
//! - **Scroll views**: layout.ts reads two surfaces off `ScrollView`:
//!   the layout state (`scrollTop`, `primary`, `getContentWidth`,
//!   `updateLayout` — already modeled by the frozen
//!   [`crate::layout_node::ScrollLayoutState`] trait) and the scrollbar
//!   surface (`scrollbar`, `isScrollbarVisible`, `isScrollbarActive`,
//!   `scrollbarTrackStyle`, `scrollbarThumbStyle`). The foundation trait
//!   cannot be extended, so this module defines
//!   [`ScrollViewState`] (supertrait of `ScrollLayoutState` plus the
//!   scrollbar surface) and bridges the frozen
//!   `ScrollLayoutNode.state: Rc<RefCell<dyn ScrollLayoutState>>` field
//!   with a thread-local registry keyed by the `Rc` data pointer: the
//!   scroll-view component builds its node through [`scroll_layout_node`],
//!   which registers the extended view alongside the coerced base view
//!   (both `Rc` coercions of one allocation share the data pointer).
//!   Entries are held as `Weak`, so dead scroll views are pruned on
//!   lookup. Scroll views that bypass [`scroll_layout_node`] still lay
//!   out (scroll, follow-end, clipping) but paint no scrollbar and are
//!   invisible to `getScrollViewsAt`/`getScrollViewBox`.
//! - **Kitty images** `[T6]`: `getKittyImageMetadata`, `cropKittyImageLine`
//!   and a private metadata registry are ported here (from
//!   terminal-image.ts:326-480) with just enough surface for
//!   `paintBox`'s image cropping; the full terminal-image module lands in
//!   tranche T6 and will supersede these helpers.
//! - **Timers**: none exist in this module (the upstream scrollbar
//!   hide-timer lives in scroll-view.ts; the test port replaces the
//!   upstream `await sleep(30)` with an explicit timer-expiry call on the
//!   test fixture).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::component::{composite_tui_line, is_image_line, ComponentRef, CURSOR_MARKER};
use crate::layout_node::{
    Basis, LayoutNode, LayoutViewport, ScrollLayoutNode, ScrollLayoutState, StackAlign,
    StackLayoutEntry, StackType,
};
use crate::utils::{
    extract_ansi_code, get_active_background_ansi, get_grapheme_cell_range, slice_by_column,
    visible_width,
};

// =============================================================================
// Types (layout.ts:12-49)
// =============================================================================

/// `LayoutRect` (layout.ts:12-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutRect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

/// `LayoutBox` (layout.ts:19-30). The TS `parent?` back-reference is
/// write-only in the upstream codebase and is not modeled (see module docs).
#[derive(Clone)]
pub struct LayoutBox {
    pub component: ComponentRef,
    pub rect: LayoutRect,
    pub clip: LayoutRect,
    pub children: Vec<LayoutBox>,
    /// `lines?: readonly string[]` — the render-cache array for leaf boxes.
    pub lines: Option<Rc<Vec<String>>>,
    /// `lineOffset?: number` (always 0 unless the leaf carries the cursor
    /// marker below its allocated height).
    pub line_offset: i64,
    pub scroll_view: Option<ScrollViewRef>,
    pub scroll_content_lines: Option<Rc<Vec<String>>>,
    pub layer: i64,
}

impl LayoutBox {
    fn empty(component: ComponentRef, rect: LayoutRect, clip: LayoutRect) -> LayoutBox {
        LayoutBox {
            component,
            rect,
            clip,
            children: Vec::new(),
            lines: None,
            line_offset: 0,
            scroll_view: None,
            scroll_content_lines: None,
            layer: 0,
        }
    }
}

/// `LayoutFrame` (layout.ts:32-38).
pub struct LayoutFrame {
    pub root: LayoutBox,
    pub width: i64,
    pub height: i64,
    pub lines: Vec<String>,
    pub primary_scroll_view: Option<ScrollViewRef>,
}

/// `ScrollbarGeometry` (layout.ts:40-47).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarGeometry {
    pub column: i64,
    pub track_top: i64,
    pub track_height: i64,
    pub thumb_top: i64,
    pub thumb_height: i64,
    pub max_scroll_top: i64,
}

/// `ScrollViewScrollbar` — `"hidden" | "auto" | "always"`
/// (scroll-view.ts:5; layout.ts reads it off the scroll state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollViewScrollbar {
    Hidden,
    Auto,
    Always,
}

// =============================================================================
// Scroll-view bridge (see module docs)
// =============================================================================

/// The scrollbar-bearing surface layout.ts reads off `ScrollView`
/// (layout.ts:154-161, 208-233, 246-260). The scroll-view component
/// implements this on the same object it hands to
/// `ScrollLayoutNode.state` and builds its layout node through
/// [`scroll_layout_node`].
pub trait ScrollViewState: ScrollLayoutState {
    fn scrollbar(&self) -> ScrollViewScrollbar;
    fn is_scrollbar_visible(&self) -> bool;
    fn is_scrollbar_active(&self) -> bool;
    fn scrollbar_track_style(&self, text: &str) -> String;
    fn scrollbar_thumb_style(&self, text: &str) -> String;
}

/// Shared handle to a scroll view's state (the TS `ScrollView` object
/// reference; identity via `Rc::ptr_eq`).
pub type ScrollViewRef = Rc<RefCell<dyn ScrollViewState>>;

thread_local! {
    /// data-pointer → weak extended view of the same allocation backing a
    /// `ScrollLayoutNode.state`. Both `Rc` coercions of one concrete
    /// allocation share the data pointer, which is what keys this map.
    static SCROLL_VIEW_STATES: RefCell<HashMap<usize, Weak<RefCell<dyn ScrollViewState>>>> =
        RefCell::new(HashMap::new());
}

fn scroll_state_key<T: ?Sized>(state: &Rc<RefCell<T>>) -> usize {
    Rc::as_ptr(state) as *const () as usize
}

/// Register the extended (scrollbar-bearing) view of a scroll state so the
/// layout solver can recover it from the frozen
/// `ScrollLayoutNode.state: Rc<RefCell<dyn ScrollLayoutState>>` field.
/// Prefer [`scroll_layout_node`], which registers and builds the node in
/// one step.
pub fn register_scroll_view_state(state: &ScrollViewRef) {
    let key = scroll_state_key(state);
    SCROLL_VIEW_STATES.with(|registry| {
        registry
            .borrow_mut()
            .insert(key, Rc::downgrade(state));
    });
}

fn lookup_scroll_view_state(state: &Rc<RefCell<dyn ScrollLayoutState>>) -> Option<ScrollViewRef> {
    let key = scroll_state_key(state);
    SCROLL_VIEW_STATES.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.retain(|_, weak| weak.strong_count() > 0);
        registry.get(&key).and_then(|weak| weak.upgrade())
    })
}

/// Build the `LayoutNode::Scroll` for a scroll-view component: registers the
/// extended state view (see module docs) and coerces the same allocation
/// into the frozen `ScrollLayoutNode.state` field type. The scroll-view port
/// should construct its `[LAYOUT_NODE]` result through this helper.
pub fn scroll_layout_node<S: ScrollViewState + 'static>(
    component: ComponentRef,
    state: Rc<RefCell<S>>,
) -> LayoutNode {
    let extended: ScrollViewRef = state.clone();
    register_scroll_view_state(&extended);
    let base: Rc<RefCell<dyn ScrollLayoutState>> = state;
    LayoutNode::Scroll(ScrollLayoutNode { component, state: base })
}

// =============================================================================
// LayoutContext + measurement (layout.ts:51-84)
// =============================================================================

/// `LayoutContext` (layout.ts:51-56, private). The TS render cache is a
/// `Map<Component, Map<number, string[]>>` keyed by object identity; the
/// Rust port keys by the `Rc` data pointer.
struct LayoutContext {
    viewport: LayoutViewport,
    render_cache: HashMap<usize, HashMap<i64, Rc<Vec<String>>>>,
    request_render: Box<dyn FnMut()>,
    primary_scroll_view: Option<ScrollViewRef>,
}

/// `intersect` (layout.ts:57-64).
fn intersect(a: LayoutRect, b: LayoutRect) -> LayoutRect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    LayoutRect {
        x,
        y,
        width: (right - x).max(0),
        height: (bottom - y).max(0),
    }
}

/// `renderCached` (layout.ts:66-78). Returns the shared cached lines for
/// `(component, width)`; the cache lives for one frame (one
/// `renderLayoutFrame` call).
fn render_cached(context: &mut LayoutContext, component: &ComponentRef, width: i64) -> Rc<Vec<String>> {
    let safe_width = width.max(1);
    let key = Rc::as_ptr(component) as *const () as usize;
    if let Some(widths) = context.render_cache.get(&key) {
        if let Some(lines) = widths.get(&safe_width) {
            return Rc::clone(lines);
        }
    }
    let lines = Rc::new(component.borrow_mut().render(safe_width as usize));
    context
        .render_cache
        .entry(key)
        .or_default()
        .insert(safe_width, Rc::clone(&lines));
    lines
}

/// `measureHeight` (layout.ts:80-82).
fn measure_height(context: &mut LayoutContext, component: &ComponentRef, width: i64) -> i64 {
    render_cached(context, component, width).len() as i64
}

/// `measureWidth` (layout.ts:84-86).
fn measure_width(context: &mut LayoutContext, component: &ComponentRef, width: i64) -> i64 {
    render_cached(context, component, width)
        .iter()
        .map(|line| visible_width(line) as i64)
        .fold(0, i64::max)
}

/// `translateBox` (layout.ts:94-97). `withParent` (layout.ts:88-92) is not
/// ported: the TS `parent` field is write-only (see module docs).
fn translate_box(box_: &mut LayoutBox, delta_y: i64) {
    box_.rect.y += delta_y;
    for child in &mut box_.children {
        translate_box(child, delta_y);
    }
}

/// `updateClips` (layout.ts:99-102).
fn update_clips(box_: &mut LayoutBox, parent_clip: LayoutRect) {
    box_.clip = intersect(parent_clip, box_.rect);
    let clip = box_.clip;
    for child in &mut box_.children {
        update_clips(child, clip);
    }
}

// =============================================================================
// layoutComponent (layout.ts:104-215)
// =============================================================================

/// `layoutComponent` (layout.ts:104-215).
fn layout_component(
    context: &mut LayoutContext,
    component: &ComponentRef,
    x: i64,
    y: i64,
    width: i64,
    height: Option<i64>,
    clip: LayoutRect,
) -> LayoutBox {
    let safe_width = width.max(1);
    let node = component.borrow_mut().layout_node();
    let node = match node {
        None => {
            // No layout node: plain leaf component (layout.ts:107-126).
            let lines = render_cached(context, component, safe_width);
            let allocated_height = match height {
                None => lines.len() as i64,
                Some(h) => h.max(0),
            };
            let mut line_offset = 0i64;
            if lines.len() as i64 > allocated_height && allocated_height > 0 {
                if let Some(cursor_line) = lines.iter().position(|line| line.contains(CURSOR_MARKER)) {
                    if cursor_line as i64 >= allocated_height {
                        line_offset = cursor_line as i64 - allocated_height + 1;
                    }
                }
            }
            let rect = LayoutRect { x, y, width: safe_width, height: allocated_height };
            let mut box_ = LayoutBox::empty(component.clone(), rect, intersect(clip, rect));
            box_.lines = Some(lines);
            box_.line_offset = line_offset;
            box_
        }
        Some(LayoutNode::Scroll(node)) => {
            // Scroll view (layout.ts:128-167).
            let state = Rc::clone(&node.state);
            let previous_scroll_top = state.borrow().scroll_top();
            let content_width = state.borrow().get_content_width(safe_width);
            let mut child_box = layout_component(
                context,
                &node.component,
                x,
                y - previous_scroll_top,
                content_width,
                None,
                clip,
            );
            let content_height = child_box.rect.height;
            let viewport_height = match height {
                None => content_height,
                Some(h) => h.max(0),
            };
            state
                .borrow_mut()
                .update_layout(content_height, viewport_height, &mut *context.request_render);
            translate_box(
                &mut child_box,
                previous_scroll_top - state.borrow().scroll_top(),
            );
            let scroll_view = lookup_scroll_view_state(&state);
            let is_primary = state.borrow().primary();
            if is_primary || context.primary_scroll_view.is_none() {
                context.primary_scroll_view = scroll_view.clone();
            }
            let rect = LayoutRect { x, y, width: safe_width, height: viewport_height };
            let child_clip = intersect(clip, rect);
            let scroll_content_lines = render_cached(context, &node.component, content_width);
            update_clips(&mut child_box, child_clip);
            let mut box_ = LayoutBox::empty(component.clone(), rect, child_clip);
            box_.children = vec![child_box];
            box_.scroll_view = scroll_view;
            box_.scroll_content_lines = Some(scroll_content_lines);
            box_
        }
        Some(LayoutNode::Stack(node)) if node.layout_type == StackType::VStack => {
            // Vertical stack (layout.ts:170-197).
            let entries = crate::components::stack::visible_stack_entries(&node.entries, &context.viewport);
            let gap_total = (entries.len() as i64 - 1).max(0) * node.gap;
            let intrinsic_heights: Vec<i64> = entries
                .iter()
                .map(|entry| match entry.basis {
                    Some(Basis::Points(basis)) => basis.floor() as i64,
                    _ => measure_height(context, &entry.component, safe_width),
                })
                .collect();
            let sizes = crate::components::stack::allocate_stack_sizes(&entries, &intrinsic_heights, height, node.gap);
            let natural_height: i64 = sizes.iter().sum::<i64>() + gap_total;
            let allocated_height = match height {
                None => natural_height,
                Some(h) => h.max(0),
            };
            let rect = LayoutRect { x, y, width: safe_width, height: allocated_height };
            let mut box_ = LayoutBox::empty(component.clone(), rect, intersect(clip, rect));
            let mut child_y = y;
            for index in 0..entries.len() {
                let child = layout_component(
                    context,
                    &entries[index].component,
                    x,
                    child_y,
                    safe_width,
                    Some(sizes[index]),
                    box_.clip,
                );
                box_.children.push(child);
                child_y += sizes[index] + node.gap;
            }
            box_
        }
        Some(LayoutNode::Stack(node)) => {
            // Horizontal stack (layout.ts:199-215).
            let entries = crate::components::stack::visible_stack_entries(&node.entries, &context.viewport);
            let intrinsic_widths: Vec<i64> = entries
                .iter()
                .map(|entry| match entry.basis {
                    Some(Basis::Points(basis)) => basis.floor() as i64,
                    _ => measure_width(context, &entry.component, safe_width),
                })
                .collect();
            let widths = crate::components::stack::allocate_stack_sizes(
                &entries,
                &intrinsic_widths,
                Some(safe_width),
                node.gap,
            );
            let intrinsic_heights: Vec<i64> = entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    measure_height(context, &entry.component, widths[index].max(1))
                })
                .collect();
            let allocated_height = match height {
                None => intrinsic_heights.iter().copied().fold(0, i64::max),
                Some(h) => h.max(0),
            };
            let rect = LayoutRect { x, y, width: safe_width, height: allocated_height };
            let mut box_ = LayoutBox::empty(component.clone(), rect, intersect(clip, rect));
            let mut child_x = x;
            for index in 0..entries.len() {
                let natural_child_height = intrinsic_heights[index];
                let child_height = if node.align == StackAlign::Stretch {
                    allocated_height
                } else {
                    allocated_height.min(natural_child_height)
                };
                let mut child_y = y;
                if node.align == StackAlign::Center {
                    // Math.floor((allocatedHeight - childHeight) / 2)
                    child_y += (allocated_height - child_height).div_euclid(2);
                } else if node.align == StackAlign::End {
                    child_y += allocated_height - child_height;
                }
                let child_width = widths[index];
                if child_width == 0 {
                    // Zero-width children get a synthetic clipped-out box
                    // (layout.ts:211-220) and are never painted.
                    box_.children.push(LayoutBox::empty(
                        Rc::clone(&entries[index].component),
                        LayoutRect { x: child_x, y: child_y, width: 0, height: child_height },
                        LayoutRect { x: child_x, y: child_y, width: 0, height: 0 },
                    ));
                } else {
                    let child = layout_component(
                        context,
                        &entries[index].component,
                        child_x,
                        child_y,
                        child_width,
                        Some(child_height),
                        box_.clip,
                    );
                    box_.children.push(child);
                }
                child_x += child_width + node.gap;
            }
            box_
        }
    };
    node
}

// =============================================================================
// Scrollbar painting (layout.ts:217-262, 264-293)
// =============================================================================

/// `replaceScrollbarCell` (layout.ts:217-246).
fn replace_scrollbar_cell(
    line: &str,
    column: i64,
    total_width: i64,
    replacement: &str,
    preserve_target_background: bool,
) -> String {
    if is_image_line(line) {
        return line.to_string();
    }

    let column = column.max(0) as usize;
    let grapheme_range = get_grapheme_cell_range(line, column);
    let start = grapheme_range.map(|range| range.start).unwrap_or(column);
    let end = grapheme_range.map(|range| range.end).unwrap_or(column + 1);
    let before = slice_by_column(line, 0, start, true);
    let target = slice_by_column(line, start, end - start, true);
    let after_len = (total_width - end as i64).max(0) as usize;
    let after = slice_by_column(line, end, after_len, true);

    let mut target_prefix = String::new();
    let mut target_index = 0usize;
    while target_index < target.len() {
        match extract_ansi_code(&target, target_index) {
            Some(code) => {
                target_prefix.push_str(code);
                target_index += code.len();
            }
            None => break,
        }
    }
    let before_padding = " ".repeat(start.saturating_sub(visible_width(&before)));
    let cell_padding_before = " ".repeat((column as i64 - start as i64).max(0) as usize);
    let cell_padding_after = " ".repeat((end as i64 - column as i64 - 1).max(0) as usize);
    let background = if preserve_target_background {
        get_active_background_ansi(&target_prefix)
    } else {
        String::new()
    };
    let target_style = format!("\x1b[0m\x1b]8;;\x07{background}");
    format!(
        "{before}{before_padding}{target_style}{cell_padding_before}{replacement}{cell_padding_after}{after}"
    )
}

/// `getScrollbarGeometry` (layout.ts:248-273). The TS default parameter
/// `includeHiddenAuto = false` is an explicit argument here.
pub fn get_scrollbar_geometry(box_: &LayoutBox, include_hidden_auto: bool) -> Option<ScrollbarGeometry> {
    let scroll_view = box_.scroll_view.as_ref()?;
    if box_.rect.width <= 0 || box_.rect.height <= 0 {
        return None;
    }
    let state = scroll_view.borrow();

    let content_height = box_
        .children
        .first()
        .map(|child| child.rect.height)
        .unwrap_or_else(|| {
            box_.scroll_content_lines
                .as_ref()
                .map_or(0, |lines| lines.len() as i64)
        });
    let track_height = box_.rect.height;
    let can_reveal_hidden_auto =
        include_hidden_auto && state.scrollbar() == ScrollViewScrollbar::Auto && content_height > track_height;
    if !state.is_scrollbar_visible() && !can_reveal_hidden_auto {
        return None;
    }

    let min_thumb_height = 2.min(track_height);
    let thumb_height = {
        // Math.max(minThumb, Math.min(trackHeight, Math.round(trackHeight * trackHeight / contentHeight)))
        let rounded = (track_height as f64 * track_height as f64 / content_height as f64).round();
        let capped = rounded.min(track_height as f64);
        (min_thumb_height as f64).max(capped) as i64
    };
    let max_scroll_top = (content_height - track_height).max(0);
    let max_thumb_top = track_height - thumb_height;
    let thumb_offset = if max_scroll_top == 0 {
        0
    } else {
        ((state.scroll_top() as f64 / max_scroll_top as f64) * max_thumb_top as f64).round() as i64
    };
    let column = box_.rect.x + box_.rect.width - 1;
    if column < box_.clip.x || column >= box_.clip.x + box_.clip.width {
        return None;
    }

    Some(ScrollbarGeometry {
        column,
        track_top: box_.rect.y,
        track_height,
        thumb_top: box_.rect.y + thumb_offset,
        thumb_height,
        max_scroll_top,
    })
}

/// `paintScrollbar` (layout.ts:275-293).
fn paint_scrollbar(box_: &LayoutBox, screen: &mut Vec<String>, total_width: i64) {
    let geometry = match get_scrollbar_geometry(box_, false) {
        Some(geometry) => geometry,
        None => return,
    };
    let scroll_view = match &box_.scroll_view {
        Some(scroll_view) => scroll_view,
        None => return,
    };
    let state = scroll_view.borrow();

    for offset in 0..geometry.track_height {
        let row = geometry.track_top + offset;
        if row < box_.clip.y
            || row >= box_.clip.y + box_.clip.height
            || row < 0
            || row >= screen.len() as i64
        {
            continue;
        }
        let is_thumb = row >= geometry.thumb_top && row < geometry.thumb_top + geometry.thumb_height;
        let replacement = if is_thumb {
            let glyph = if state.is_scrollbar_active() { "█" } else { "┃" };
            state.scrollbar_thumb_style(glyph)
        } else {
            state.scrollbar_track_style("│")
        };
        let preserve_target_background = state.scrollbar() != ScrollViewScrollbar::Always;
        let row = row as usize;
        // `screen[row] ?? ""` — row is bounds-checked above.
        let base = screen[row].clone();
        screen[row] =
            replace_scrollbar_cell(&base, geometry.column, total_width, &replacement, preserve_target_background);
    }
}

// =============================================================================
// [T6] Kitty image helpers (terminal-image.ts:326-480, minimal surface)
// =============================================================================

/// `KittyImageMetadata` (terminal-image.ts:326-334). [T6]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KittyImageMetadata {
    image_id: i64,
    columns: i64,
    rows: i64,
    width_px: i64,
    height_px: i64,
}

/// [T6] Internal registry entry (terminal-image.ts:336-346). Insertion order
/// is preserved with a `Vec`; the >1000 cap evicts the oldest entry.
struct RegisteredKittyImageMetadata {
    metadata: KittyImageMetadata,
    transmission_generation: i64,
}

thread_local! {
    static KITTY_IMAGE_METADATA: RefCell<Vec<RegisteredKittyImageMetadata>> = const { RefCell::new(Vec::new()) };
    static KITTY_TRANSMISSION_GENERATION: Cell<i64> = const { Cell::new(0) };
}

/// `registerKittyImageMetadata` (terminal-image.ts:349-359). [T6]
fn register_kitty_image_metadata(metadata: KittyImageMetadata) {
    KITTY_TRANSMISSION_GENERATION.with(|generation| generation.set(generation.get() + 1));
    let transmission_generation = KITTY_TRANSMISSION_GENERATION.with(|generation| generation.get());
    KITTY_IMAGE_METADATA.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.retain(|entry| entry.metadata.image_id != metadata.image_id);
        registry.push(RegisteredKittyImageMetadata {
            metadata,
            transmission_generation,
        });
        if registry.len() > 1000 {
            registry.remove(0);
        }
    });
}

/// `/\x1b_G([^;]*);/` — the first Kitty control block in the line, as
/// `(byte start, byte match length, controls)` (terminal-image.ts:380-384,
/// 458-460). [T6]
fn find_kitty_controls(line: &str) -> Option<(usize, usize, &str)> {
    let start = line.find("\x1b_G")?;
    let controls_start = start + "\x1b_G".len();
    let semi = controls_start + line[controls_start..].find(';')?;
    Some((start, semi + 1 - start, &line[controls_start..semi]))
}

/// `getRegisteredKittyImageMetadataFromControls` (terminal-image.ts:361-363):
/// `/(?:^|,)i=(\d+)(?:,|$)/` over the controls. [T6]
fn registered_metadata_from_controls(controls: &str) -> Option<KittyImageMetadata> {
    for part in controls.split(',') {
        if let Some(value) = part.strip_prefix("i=") {
            // The regex requires the digits to run to the ',' or end.
            if value.is_empty() || !value.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let image_id: i64 = value.parse().ok()?;
            return KITTY_IMAGE_METADATA.with(|registry| {
                registry
                    .borrow()
                    .iter()
                    .find(|entry| entry.metadata.image_id == image_id)
                    .map(|entry| entry.metadata)
            });
        }
    }
    None
}

/// `getKittyImageMetadata` (terminal-image.ts:380-388). [T6]
fn get_kitty_image_metadata(line: &str) -> Option<KittyImageMetadata> {
    let (_, _, controls) = find_kitty_controls(line)?;
    registered_metadata_from_controls(controls)
}

/// `cropKittyImageLine` (terminal-image.ts:458-478). [T6]
fn crop_kitty_image_line(line: &str, hidden_rows: i64, visible_rows: i64) -> String {
    let metadata = match get_kitty_image_metadata(line) {
        Some(metadata) => metadata,
        None => return line.to_string(),
    };
    let (start, match_len, controls) = match find_kitty_controls(line) {
        Some(found) => found,
        None => return line.to_string(),
    };
    if hidden_rows < 0 || hidden_rows >= metadata.rows || visible_rows <= 0 {
        return line.to_string();
    }
    let cropped_rows = visible_rows.min(metadata.rows - hidden_rows);
    if hidden_rows == 0 && cropped_rows == metadata.rows {
        return line.to_string();
    }
    let source_y = ((metadata.height_px as f64 * hidden_rows as f64) / metadata.rows as f64).floor();
    let source_end =
        ((metadata.height_px as f64 * (hidden_rows + cropped_rows) as f64) / metadata.rows as f64).ceil();
    let source_height = 1f64.max((metadata.height_px as f64).min(source_end) - source_y);
    let mut rebuilt = controls
        .split(',')
        .filter(|control| {
            !control.starts_with("y=") && !control.starts_with("h=") && !control.starts_with("r=")
        })
        .collect::<Vec<&str>>()
        .join(",");
    if !rebuilt.is_empty() || !controls.is_empty() {
        rebuilt.push(',');
    }
    rebuilt.push_str(&format!(
        "y={},h={},r={}",
        source_y as i64,
        source_height as i64,
        cropped_rows
    ));
    format!(
        "{}\x1b_G{};{}",
        &line[..start],
        rebuilt,
        &line[start + match_len..]
    )
}

// =============================================================================
// paintBox (layout.ts:265-314)
// =============================================================================

/// `OSC133_ZONE_PREFIX` (layout.ts:15): strip the leading run of OSC 133
/// zone markers (`\x1b]133;A|B|C` + BEL/ST).
fn strip_osc133_zone_prefix(line: &str) -> &str {
    let mut rest = line;
    'outer: loop {
        for marker in ["\x1b]133;A", "\x1b]133;B", "\x1b]133;C"] {
            if let Some(after_marker) = rest.strip_prefix(marker) {
                if let Some(after_bel) = after_marker.strip_prefix('\x07') {
                    rest = after_bel;
                    continue 'outer;
                }
                if let Some(after_st) = after_marker.strip_prefix("\x1b\\") {
                    rest = after_st;
                    continue 'outer;
                }
                // Prefix matched but no terminator: the anchored `+` regex
                // cannot match — stop without stripping anything further.
                return rest;
            }
        }
        return rest;
    }
}

/// `paintBox` (layout.ts:265-314).
fn paint_box(box_: &LayoutBox, screen: &mut Vec<String>, total_width: i64) {
    if let Some(lines) = &box_.lines {
        let offset = box_.line_offset;
        let first_row = box_.rect.y.max(box_.clip.y).max(0);
        let last_row = (box_.rect.y + box_.rect.height)
            .min(box_.clip.y + box_.clip.height)
            .min(screen.len() as i64);
        for row in first_row..last_row {
            // Negative indices wrap to huge `usize` values and miss, which
            // mirrors the TS `undefined` hole read (`continue`).
            let source_line = match lines.get((offset + row - box_.rect.y) as usize) {
                Some(line) => line.as_str(),
                None => continue,
            };
            let stripped = strip_osc133_zone_prefix(source_line);
            let line = if let Some(metadata) = get_kitty_image_metadata(stripped) {
                let clip_bottom = (screen.len() as i64).min(box_.clip.y + box_.clip.height);
                let visible_rows = metadata.rows.min(clip_bottom - row);
                if visible_rows < metadata.rows {
                    crop_kitty_image_line(stripped, 0, visible_rows)
                } else {
                    stripped.to_string()
                }
            } else {
                stripped.to_string()
            };
            let row = row as usize;
            // Fast path (layout.ts:291-300): a full-width box painting onto
            // an untouched row reuses the source line. JS `!screen[row]` is
            // false for every string except `""`.
            if box_.rect.x == 0
                && box_.rect.width >= total_width
                && (is_image_line(&line) || screen[row].is_empty())
            {
                screen[row] = line;
            } else {
                let composited = composite_tui_line(
                    &screen[row],
                    &line,
                    box_.rect.x.max(0) as usize,
                    box_.rect.width.max(0) as usize,
                    total_width.max(0) as usize,
                );
                screen[row] = composited;
            }
        }
    }
    for child in &box_.children {
        paint_box(child, screen, total_width);
    }

    // Partially-scrolled Kitty images peeking from above the viewport
    // (layout.ts:302-312).
    if let (Some(scroll_view), Some(scroll_content_lines)) =
        (&box_.scroll_view, &box_.scroll_content_lines)
    {
        let scroll_top = scroll_view.borrow().scroll_top();
        if scroll_top > 0 && box_.rect.height > 0 {
            for image_row in (0..scroll_top).rev() {
                let image_line = scroll_content_lines
                    .get(image_row as usize)
                    .map(|line| line.as_str())
                    .unwrap_or("");
                if let Some(metadata) = get_kitty_image_metadata(image_line) {
                    let hidden_rows = scroll_top - image_row;
                    if hidden_rows < metadata.rows {
                        let visible_rows = box_.rect.height.min(metadata.rows - hidden_rows);
                        let cropped = crop_kitty_image_line(image_line, hidden_rows, visible_rows);
                        if box_.rect.x == 0 && box_.rect.width >= total_width {
                            // Out-of-bounds rows are skipped: the JS array
                            // writes land on exotic indices (negative keys or
                            // past-the-end holes) that never render.
                            if let Ok(row) = usize::try_from(box_.rect.y) {
                                if row < screen.len() {
                                    screen[row] = cropped;
                                }
                            }
                        }
                    }
                    break;
                }
                if !image_line.is_empty() {
                    break;
                }
            }
        }
    }

    paint_scrollbar(box_, screen, total_width);
}

// =============================================================================
// renderLayoutFrame + hit testing (layout.ts:316-357, 371-449)
// =============================================================================

/// `renderLayoutFrame` (layout.ts:316-357).
pub fn render_layout_frame(
    root: &ComponentRef,
    width: i64,
    height: i64,
    request_render: impl FnMut() + 'static,
) -> LayoutFrame {
    let safe_width = width.max(1);
    let safe_height = height.max(1);
    let mut context = LayoutContext {
        viewport: LayoutViewport { width: safe_width, height: safe_height },
        render_cache: HashMap::new(),
        request_render: Box::new(request_render),
        primary_scroll_view: None,
    };
    let root_box = layout_component(
        &mut context,
        root,
        0,
        0,
        safe_width,
        Some(safe_height),
        LayoutRect { x: 0, y: 0, width: safe_width, height: safe_height },
    );
    let mut lines = vec![String::new(); safe_height as usize];
    paint_box(&root_box, &mut lines, safe_width);
    LayoutFrame {
        root: root_box,
        width: safe_width,
        height: safe_height,
        lines,
        primary_scroll_view: context.primary_scroll_view,
    }
}

/// `containsPoint` (layout.ts:359-361).
fn contains_point(rect: &LayoutRect, x: i64, y: i64) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

/// Return the visual hit path from the deepest component to the layout root
/// — `getLayoutBoxesAt` (layout.ts:364-376).
pub fn get_layout_boxes_at(frame: &LayoutFrame, x: i64, y: i64) -> Vec<&LayoutBox> {
    fn visit<'a>(
        box_: &'a LayoutBox,
        depth: i64,
        x: i64,
        y: i64,
        result: &mut Vec<(&'a LayoutBox, i64)>,
    ) {
        if !contains_point(&box_.clip, x, y) {
            return;
        }
        result.push((box_, depth));
        for child in &box_.children {
            visit(child, depth + 1, x, y, result);
        }
    }
    let mut result: Vec<(&LayoutBox, i64)> = Vec::new();
    visit(&frame.root, 0, x, y, &mut result);
    // Stable sort by (layer desc, depth desc), mirroring the TS comparator
    // `b.layer - a.layer || b.depth - a.depth`.
    result.sort_by(|a, b| b.0.layer.cmp(&a.0.layer).then(b.1.cmp(&a.1)));
    result.into_iter().map(|(box_, _)| box_).collect()
}

/// `getScrollViewBox` (layout.ts:378-388).
pub fn get_scroll_view_box<'a>(
    frame: &'a LayoutFrame,
    scroll_view: &ScrollViewRef,
) -> Option<&'a LayoutBox> {
    fn visit<'a>(box_: &'a LayoutBox, scroll_view: &ScrollViewRef) -> Option<&'a LayoutBox> {
        if box_
            .scroll_view
            .as_ref()
            .is_some_and(|state| Rc::ptr_eq(state, scroll_view))
        {
            return Some(box_);
        }
        for child in &box_.children {
            if let Some(found) = visit(child, scroll_view) {
                return Some(found);
            }
        }
        None
    }
    visit(&frame.root, scroll_view)
}

/// `getScrollViewsAt` (layout.ts:390-402).
pub fn get_scroll_views_at(frame: &LayoutFrame, x: i64, y: i64) -> Vec<ScrollViewRef> {
    fn visit(box_: &LayoutBox, depth: i64, x: i64, y: i64, result: &mut Vec<(ScrollViewRef, i64)>) {
        if !contains_point(&box_.clip, x, y) {
            return;
        }
        if let Some(scroll_view) = &box_.scroll_view {
            if contains_point(&box_.rect, x, y) {
                result.push((Rc::clone(scroll_view), depth));
            }
        }
        for child in &box_.children {
            visit(child, depth + 1, x, y, result);
        }
    }
    let mut result: Vec<(ScrollViewRef, i64)> = Vec::new();
    visit(&frame.root, 0, x, y, &mut result);
    result.sort_by(|a, b| b.1.cmp(&a.1));
    result.into_iter().map(|(scroll_view, _)| scroll_view).collect()
}

// =============================================================================
// Tests — port of test/layout.test.ts (367 lines, 15 it() blocks)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::Component;
    use crate::components::stack::{allocate_stack_sizes, visible_stack_entries};
    use crate::layout_node::{LayoutViewport, Overscroll, StackLayoutNode};
    use crate::utils::{
        apply_background_to_line, js_trim, strip_terminal_sequences, wrap_text_with_ansi,
    };
    use std::cell::Cell;

    /// JS `Number.MAX_SAFE_INTEGER` (stack.ts:135).
    const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

    fn sl(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    /// The test file's `visibleLines` helper: strip sequences + trimEnd.
    fn visible_lines(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .map(|line| strip_terminal_sequences(line).trim_end().to_string())
            .collect()
    }

    // --- Test fixtures -------------------------------------------------------
    //
    // The TS tests use Text/VStack/HStack/ScrollView from the sibling
    // components; per the port plan, equivalent local test-only components
    // are used instead.

    /// Local port of `Text` (text.ts) restricted to the behavior the layout
    /// tests rely on (arbitrary padding + optional background fn + cache).
    struct TestText {
        text: String,
        padding_x: usize,
        padding_y: usize,
        bg_fn: Option<Rc<dyn Fn(&str) -> String>>,
        cache: Option<(String, usize, Vec<String>)>,
    }

    impl TestText {
        fn new(text: &str) -> TestText {
            TestText {
                text: text.to_string(),
                padding_x: 0,
                padding_y: 0,
                bg_fn: None,
                cache: None,
            }
        }

        fn with_bg(text: &str, bg_fn: Rc<dyn Fn(&str) -> String>) -> TestText {
            TestText { bg_fn: Some(bg_fn), ..TestText::new(text) }
        }

        fn set_text(&mut self, text: &str) {
            self.text = text.to_string();
            self.cache = None;
        }

        fn render_uncached(&self, width: usize) -> Vec<String> {
            if self.text.is_empty() || js_trim(&self.text).is_empty() {
                return Vec::new();
            }
            let normalized = self.text.replace('\t', "   ");
            let padding_x = self.padding_x.min(((width as i64 - 1) / 2).max(0) as usize);
            let content_width = (width as i64 - 2 * padding_x as i64).max(1) as usize;
            let wrapped = wrap_text_with_ansi(&normalized, content_width);
            let margin = " ".repeat(padding_x);
            let mut content_lines = Vec::new();
            for line in &wrapped {
                let line_with_margins = format!("{margin}{line}{margin}");
                match &self.bg_fn {
                    Some(bg_fn) => {
                        content_lines.push(apply_background_to_line(&line_with_margins, width, |text| bg_fn(text)));
                    }
                    None => {
                        let padding_needed = (width as i64 - visible_width(&line_with_margins) as i64).max(0) as usize;
                        content_lines.push(format!("{line_with_margins}{}", " ".repeat(padding_needed)));
                    }
                }
            }
            let empty_line = " ".repeat(width);
            let mut empty_lines = Vec::new();
            for _ in 0..self.padding_y {
                match &self.bg_fn {
                    Some(bg_fn) => empty_lines.push(apply_background_to_line(&empty_line, width, |text| bg_fn(text))),
                    None => empty_lines.push(empty_line.clone()),
                }
            }
            let mut result = empty_lines.clone();
            result.extend(content_lines);
            result.extend(empty_lines);
            if !result.is_empty() {
                result
            } else {
                vec![String::new()]
            }
        }
    }

    impl Component for TestText {
        fn render(&mut self, width: usize) -> Vec<String> {
            if let Some((text, cached_width, lines)) = &self.cache {
                if *text == self.text && *cached_width == width {
                    return lines.clone();
                }
            }
            let result = self.render_uncached(width);
            self.cache = Some((self.text.clone(), width, result.clone()));
            result
        }

        fn invalidate(&mut self) {
            self.cache = None;
        }
    }

    fn text(text: &str) -> Rc<RefCell<TestText>> {
        Rc::new(RefCell::new(TestText::new(text)))
    }

    fn text_bg(text: &str, bg_fn: Rc<dyn Fn(&str) -> String>) -> Rc<RefCell<TestText>> {
        Rc::new(RefCell::new(TestText::with_bg(text, bg_fn)))
    }

    /// Coerce a concrete component reference into a `ComponentRef` (same
    /// allocation, so identity is preserved).
    fn comp<T: Component + 'static>(component: &Rc<RefCell<T>>) -> ComponentRef {
        let erased: ComponentRef = component.clone();
        erased
    }

    /// `{ render, invalidate }` plain-object content components.
    struct FnComponent {
        render_fn: Rc<dyn Fn(usize) -> Vec<String>>,
    }

    impl Component for FnComponent {
        fn render(&mut self, width: usize) -> Vec<String> {
            (self.render_fn)(width)
        }
    }

    fn fn_component(render_fn: impl Fn(usize) -> Vec<String> + 'static) -> ComponentRef {
        Rc::new(RefCell::new(FnComponent { render_fn: Rc::new(render_fn) }))
    }

    /// TS `Stack.addChild` entry normalization (stack.ts:46-61).
    fn normalize_size(value: f64, fallback: f64) -> f64 {
        if value.is_finite() {
            value.floor().max(0.0)
        } else {
            fallback
        }
    }

    #[derive(Default)]
    struct EntryOpts {
        basis: Option<Basis>,
        grow: Option<f64>,
        shrink: Option<f64>,
        min_size: Option<f64>,
        max_size: Option<f64>,
        visible: Option<Rc<dyn Fn(&LayoutViewport) -> bool>>,
    }

    fn entry_with(component: ComponentRef, opts: EntryOpts) -> StackLayoutEntry {
        StackLayoutEntry {
            component,
            basis: opts.basis,
            grow: opts.grow.map(|value| normalize_size(value, 0.0)),
            shrink: opts.shrink.map(|value| normalize_size(value, 1.0)),
            min_size: opts.min_size.map(|value| normalize_size(value, 0.0)),
            max_size: opts.max_size.map(|value| normalize_size(value, MAX_SAFE_INTEGER)),
            visible: opts.visible,
        }
    }

    fn entry(component: ComponentRef) -> StackLayoutEntry {
        entry_with(component, EntryOpts::default())
    }

    /// Local port of `Stack`/`VStack`/`HStack` (stack.ts, v-stack.ts,
    /// h-stack.ts) — the layout node plus the plain-render behavior the
    /// tests exercise directly (`stack.render(10)`).
    struct TestStack {
        entries: Vec<StackLayoutEntry>,
        gap: i64,
        align: StackAlign,
        layout_type: StackType,
    }

    impl TestStack {
        fn render_v(&mut self, width: usize) -> Vec<String> {
            let viewport = LayoutViewport { width: width.max(1) as i64, height: i64::MAX / 2 };
            let entries = visible_stack_entries(&self.entries, &viewport);
            let rendered: Vec<Vec<String>> = entries
                .iter()
                .map(|entry| entry.component.borrow_mut().render(viewport.width as usize))
                .collect();
            let intrinsic_sizes: Vec<i64> = rendered.iter().map(|lines| lines.len() as i64).collect();
            let sizes = allocate_stack_sizes(&entries, &intrinsic_sizes, None, self.gap);
            let mut lines = Vec::new();
            for index in 0..entries.len() {
                if index > 0 {
                    for _ in 0..self.gap {
                        lines.push(String::new());
                    }
                }
                let child_lines = &rendered[index];
                let size = sizes[index].max(0) as usize;
                for line in child_lines.iter().take(size) {
                    lines.push(line.clone());
                }
                for _ in child_lines.len()..size {
                    lines.push(String::new());
                }
            }
            lines
        }

        fn render_h(&mut self, width: usize) -> Vec<String> {
            let safe_width = width.max(1);
            let viewport = LayoutViewport { width: safe_width as i64, height: i64::MAX / 2 };
            let entries = visible_stack_entries(&self.entries, &viewport);
            if entries.is_empty() {
                return Vec::new();
            }
            let intrinsic_widths: Vec<i64> = entries
                .iter()
                .map(|entry| {
                    let lines = entry.component.borrow_mut().render(safe_width);
                    lines.iter().map(|line| visible_width(line) as i64).fold(0, i64::max)
                })
                .collect();
            let widths = allocate_stack_sizes(&entries, &intrinsic_widths, Some(safe_width as i64), self.gap);
            let rendered: Vec<Vec<String>> = entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    if widths[index] == 0 {
                        Vec::new()
                    } else {
                        entry.component.borrow_mut().render(widths[index].max(0) as usize)
                    }
                })
                .collect();
            let height = rendered.iter().map(|lines| lines.len()).max().unwrap_or(0);
            let mut result = vec![String::new(); height];
            let mut x: i64 = 0;
            for (index, lines) in rendered.iter().enumerate() {
                let child_width = widths[index].max(0) as usize;
                let mut offset: i64 = 0;
                if self.align == StackAlign::Center {
                    offset = (height as i64 - lines.len() as i64).div_euclid(2);
                } else if self.align == StackAlign::End {
                    offset = height as i64 - lines.len() as i64;
                }
                for (row, line) in lines.iter().enumerate() {
                    let target = row as i64 + offset;
                    if target < 0 || target >= height as i64 {
                        continue;
                    }
                    let target = target as usize;
                    result[target] = composite_tui_line(&result[target], line, x.max(0) as usize, child_width, safe_width);
                }
                x += child_width as i64 + self.gap;
            }
            result
        }
    }

    impl Component for TestStack {
        fn render(&mut self, width: usize) -> Vec<String> {
            match self.layout_type {
                StackType::VStack => self.render_v(width),
                StackType::HStack => self.render_h(width),
            }
        }

        fn layout_node(&mut self) -> Option<LayoutNode> {
            Some(LayoutNode::Stack(StackLayoutNode {
                layout_type: self.layout_type,
                entries: self.entries.clone(),
                gap: self.gap,
                align: self.align,
            }))
        }
    }

    fn stack(children: Vec<StackLayoutEntry>, layout_type: StackType, gap: i64, align: StackAlign) -> ComponentRef {
        Rc::new(RefCell::new(TestStack { entries: children, gap, align, layout_type }))
    }

    fn vstack(children: Vec<StackLayoutEntry>) -> ComponentRef {
        stack(children, StackType::VStack, 0, StackAlign::Stretch)
    }

    fn vstack_with_gap(children: Vec<StackLayoutEntry>, gap: i64) -> ComponentRef {
        stack(children, StackType::VStack, gap, StackAlign::Stretch)
    }

    fn hstack(children: Vec<StackLayoutEntry>) -> ComponentRef {
        stack(children, StackType::HStack, 0, StackAlign::Stretch)
    }

    fn hstack_with_align(children: Vec<StackLayoutEntry>, align: StackAlign) -> ComponentRef {
        stack(children, StackType::HStack, 0, align)
    }

    /// Local port of `ScrollView` (scroll-view.ts) as far as the layout
    /// tests exercise it. The upstream `setTimeout`-based transient
    /// scrollbar hiding is modeled by an explicit `hide_timer_armed` flag
    /// that the test expires manually (`expire_scrollbar_hide_timer`),
    /// replacing the upstream `await sleep(hideDelayMs)`; there is no event
    /// loop here. The stored `requestRenderCallback` similarly becomes a
    /// `render_requested` flag.
    struct TestScrollViewState {
        child: ComponentRef,
        follow_end: bool,
        following_end: bool,
        primary: bool,
        overscroll: Overscroll,
        track_style: Option<Rc<dyn Fn(&str) -> String>>,
        thumb_style: Option<Rc<dyn Fn(&str) -> String>>,
        scrollbar: ScrollViewScrollbar,
        hide_delay_ms: i64,
        scroll_top: i64,
        content_height: i64,
        viewport_height: i64,
        follow_suppressed_at_end: bool,
        transient_scrollbar_visible: bool,
        scrollbar_active: bool,
        hide_timer_armed: bool,
        render_requested: bool,
    }

    impl TestScrollViewState {
        fn new(child: ComponentRef, opts: SvOpts) -> TestScrollViewState {
            TestScrollViewState {
                child,
                follow_end: opts.follow_end,
                following_end: opts.follow_end,
                primary: opts.primary,
                overscroll: Overscroll::Chain,
                track_style: opts.track_style,
                thumb_style: opts.thumb_style,
                scrollbar: opts.scrollbar.unwrap_or(ScrollViewScrollbar::Hidden),
                hide_delay_ms: opts.hide_delay_ms.unwrap_or(1000).max(0),
                scroll_top: 0,
                content_height: 0,
                viewport_height: 0,
                follow_suppressed_at_end: false,
                transient_scrollbar_visible: false,
                scrollbar_active: false,
                hide_timer_armed: false,
                render_requested: false,
            }
        }

        fn mark_scrollbar_activity(&mut self) {
            if self.scrollbar != ScrollViewScrollbar::Auto || self.content_height <= self.viewport_height {
                return;
            }
            self.transient_scrollbar_visible = true;
            self.hide_timer_armed = false; // clearTimeout
            if self.scrollbar_active {
                return;
            }
            self.hide_timer_armed = true; // setTimeout(hide, hideDelayMs)
        }

        fn hide_transient_scrollbar(&mut self) {
            self.transient_scrollbar_visible = false;
            self.hide_timer_armed = false;
        }

        /// [divergence] Expires the modeled hide timer (the upstream test
        /// waits out `scrollbarHideDelayMs` instead).
        fn expire_scrollbar_hide_timer(&mut self) {
            if self.hide_timer_armed {
                self.hide_timer_armed = false;
                self.transient_scrollbar_visible = false;
            }
        }

        fn set_scrollbar(&mut self, scrollbar: ScrollViewScrollbar) {
            if scrollbar == self.scrollbar {
                return;
            }
            self.scrollbar = scrollbar;
            if scrollbar != ScrollViewScrollbar::Auto {
                self.hide_transient_scrollbar();
            } else if self.scrollbar_active {
                self.mark_scrollbar_activity();
            }
            self.render_requested = true;
        }

        fn set_scrollbar_active(&mut self, active: bool) {
            if active == self.scrollbar_active {
                return;
            }
            self.scrollbar_active = active;
            self.mark_scrollbar_activity();
            self.render_requested = true;
        }

        fn scroll_by(&mut self, lines: i64) -> i64 {
            let requested = lines;
            if requested == 0 {
                return 0;
            }
            let max_scroll_top = (self.content_height - self.viewport_height).max(0);
            let start = if self.following_end { max_scroll_top } else { self.scroll_top };
            let next = (start + requested).clamp(0, max_scroll_top);
            let moved = next - start;
            let was_following_end = self.following_end;
            self.scroll_top = next;
            self.following_end = self.follow_end && next == max_scroll_top;
            self.follow_suppressed_at_end = false;
            if moved != 0 {
                self.mark_scrollbar_activity();
            }
            if moved != 0 || self.following_end != was_following_end {
                self.render_requested = true;
            }
            requested - moved
        }

        fn scroll_to_start(&mut self) {
            let changed = self.scroll_top != 0
                || self.following_end != (self.follow_end && self.content_height <= self.viewport_height);
            self.scroll_top = 0;
            self.following_end = self.follow_end && self.content_height <= self.viewport_height;
            self.follow_suppressed_at_end = false;
            if changed {
                self.mark_scrollbar_activity();
                self.render_requested = true;
            }
        }

        fn scroll_to_end(&mut self) {
            let next = (self.content_height - self.viewport_height).max(0);
            let changed = self.scroll_top != next || self.following_end != self.follow_end;
            self.scroll_top = next;
            self.following_end = self.follow_end;
            self.follow_suppressed_at_end = false;
            if changed {
                self.mark_scrollbar_activity();
                self.render_requested = true;
            }
        }

        fn is_following_end(&self) -> bool {
            self.following_end
        }
    }

    impl ScrollLayoutState for TestScrollViewState {
        fn scroll_top(&self) -> i64 {
            self.scroll_top
        }

        fn primary(&self) -> bool {
            self.primary
        }

        fn overscroll(&self) -> Overscroll {
            self.overscroll
        }

        fn viewport_height(&self) -> i64 {
            self.viewport_height
        }

        fn get_content_width(&self, width: i64) -> i64 {
            if self.scrollbar == ScrollViewScrollbar::Always && width > 1 {
                width - 1
            } else {
                width
            }
        }

        fn update_layout(&mut self, content_height: i64, viewport_height: i64, _request_render: &mut dyn FnMut()) {
            self.content_height = content_height.max(0);
            self.viewport_height = viewport_height.max(0);
            // The TS stores requestRender here; &mut dyn cannot outlive the
            // call, so the test fixture tracks `render_requested` instead.
            let max_scroll_top = (self.content_height - self.viewport_height).max(0);
            if self.following_end {
                self.scroll_top = max_scroll_top;
            } else {
                self.scroll_top = self.scroll_top.clamp(0, max_scroll_top);
            }
            if self.scroll_top < max_scroll_top {
                self.follow_suppressed_at_end = false;
            }
            if self.follow_end && self.scroll_top == max_scroll_top && !self.follow_suppressed_at_end {
                self.following_end = true;
            }
            if self.content_height <= self.viewport_height {
                self.hide_transient_scrollbar();
            }
        }
    }

    impl ScrollViewState for TestScrollViewState {
        fn scrollbar(&self) -> ScrollViewScrollbar {
            self.scrollbar
        }

        fn is_scrollbar_visible(&self) -> bool {
            if self.scrollbar == ScrollViewScrollbar::Always {
                return self.viewport_height > 0;
            }
            self.scrollbar == ScrollViewScrollbar::Auto
                && self.content_height > self.viewport_height
                && self.transient_scrollbar_visible
        }

        fn is_scrollbar_active(&self) -> bool {
            self.scrollbar_active
        }

        fn scrollbar_track_style(&self, text: &str) -> String {
            match &self.track_style {
                Some(style) => style(text),
                None => format!("\x1b[90m{text}\x1b[39m"),
            }
        }

        fn scrollbar_thumb_style(&self, text: &str) -> String {
            match &self.thumb_style {
                Some(style) => style(text),
                None => format!("\x1b[37m{text}\x1b[39m"),
            }
        }
    }

    struct TestScrollView {
        state: Rc<RefCell<TestScrollViewState>>,
    }

    impl Component for TestScrollView {
        fn render(&mut self, width: usize) -> Vec<String> {
            let content_width = self.state.borrow().get_content_width(width as i64);
            let lines = self.state.borrow().child.borrow_mut().render(content_width as usize);
            if content_width == width as i64 {
                lines
            } else {
                lines.into_iter().map(|line| format!("{line} ")).collect()
            }
        }

        fn layout_node(&mut self) -> Option<LayoutNode> {
            let child = self.state.borrow().child.clone();
            Some(scroll_layout_node(child, Rc::clone(&self.state)))
        }
    }

    #[derive(Default)]
    struct SvOpts {
        follow_end: bool,
        primary: bool,
        scrollbar: Option<ScrollViewScrollbar>,
        track_style: Option<Rc<dyn Fn(&str) -> String>>,
        thumb_style: Option<Rc<dyn Fn(&str) -> String>>,
        hide_delay_ms: Option<i64>,
    }

    fn scroll_view(child: ComponentRef, opts: SvOpts) -> (ComponentRef, Rc<RefCell<TestScrollViewState>>) {
        let state = Rc::new(RefCell::new(TestScrollViewState::new(child, opts)));
        let component: ComponentRef = Rc::new(RefCell::new(TestScrollView { state: Rc::clone(&state) }));
        (component, state)
    }

    /// [T6] `encodeKitty` (terminal-image.ts:226-274), test-only surface.
    fn encode_kitty(base64_data: &str, columns: i64, rows: i64, image_id: Option<i64>, move_cursor: bool) -> String {
        const CHUNK_SIZE: usize = 4096;
        let mut params = vec!["a=T".to_string(), "f=100".to_string(), "q=2".to_string()];
        if !move_cursor {
            params.push("C=1".to_string());
        }
        if columns != 0 {
            params.push(format!("c={columns}"));
        }
        if rows != 0 {
            params.push(format!("r={rows}"));
        }
        if let Some(image_id) = image_id {
            if image_id != 0 {
                params.push(format!("i={image_id}"));
            }
        }
        let params = params.join(",");
        if base64_data.len() <= CHUNK_SIZE {
            return format!("\x1b_G{params};{base64_data}\x1b\\");
        }
        let mut chunks: Vec<String> = Vec::new();
        let mut offset = 0usize;
        let mut is_first = true;
        while offset < base64_data.len() {
            let chunk = &base64_data[offset..(offset + CHUNK_SIZE).min(base64_data.len())];
            let is_last = offset + CHUNK_SIZE >= base64_data.len();
            if is_first {
                chunks.push(format!("\x1b_G{params},m=1;{chunk}\x1b\\"));
                is_first = false;
            } else if is_last {
                chunks.push(format!("\x1b_Gm=0;{chunk}\x1b\\"));
            } else {
                chunks.push(format!("\x1b_Gm=1;{chunk}\x1b\\"));
            }
            offset += CHUNK_SIZE;
        }
        chunks.join("")
    }

    // --- viewport layout (test/layout.test.ts) --------------------------------

    #[test]
    fn allocates_vertical_grow_space_deterministically() {
        let frame = render_layout_frame(
            &vstack(vec![
                entry_with(comp(&text("top")), EntryOpts {
                    basis: Some(Basis::Points(1.0)),
                    shrink: Some(0.0),
                    ..Default::default()
                }),
                entry_with(comp(&text("body")), EntryOpts {
                    basis: Some(Basis::Points(0.0)),
                    grow: Some(1.0),
                    ..Default::default()
                }),
            ]),
            10,
            4,
            || {},
        );

        assert_eq!(
            frame.root.children.iter().map(|child| child.rect.height).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(visible_lines(&frame.lines), sl(&["top", "body", "", ""]));
    }

    #[test]
    fn does_not_render_fixed_basis_scroll_content_during_stack_measurement() {
        let render_count = Rc::new(Cell::new(0));
        let counter = Rc::clone(&render_count);
        let transcript_content = fn_component(move |_width| {
            counter.set(counter.get() + 1);
            sl(&["one", "two", "three"])
        });
        let (transcript, _) = scroll_view(transcript_content, SvOpts::default());
        let root = vstack(vec![
            entry_with(transcript, EntryOpts {
                basis: Some(Basis::Points(0.0)),
                grow: Some(1.0),
                ..Default::default()
            }),
            entry_with(comp(&text("dock")), EntryOpts {
                basis: Some(Basis::Auto),
                ..Default::default()
            }),
        ]);
        render_layout_frame(&root, 10, 3, || {});
        assert_eq!(render_count.get(), 1);
    }

    #[test]
    fn paints_only_clipped_rows_from_very_large_scroll_content() {
        // [divergence] The upstream test uses a 1_000_000_000-entry *sparse*
        // JS array (holes are free); a Rust Vec<String> allocates every
        // element, so the count is reduced to 1_000_000. The behavior under
        // test — only the clipped rows are painted from huge follow-end
        // content — is unchanged.
        let line_count: usize = 1_000_000;
        let mut lines = vec![String::new(); line_count];
        lines[line_count - 4] = "before".to_string();
        lines[line_count - 3] = "visible 1".to_string();
        lines[line_count - 2] = "visible 2".to_string();
        lines[line_count - 1] = "visible 3".to_string();
        let lines = Rc::new(lines);
        let transcript_content = fn_component(move |_width| (*lines).clone());
        let (transcript, _) = scroll_view(
            transcript_content,
            SvOpts { follow_end: true, ..Default::default() },
        );

        let frame = render_layout_frame(&transcript, 10, 3, || {});
        assert_eq!(visible_lines(&frame.lines), sl(&["visible 1", "visible 2", "visible 3"]));
    }

    #[test]
    fn shrinks_entries_to_their_minimum_sizes() {
        let frame = render_layout_frame(
            &vstack(vec![
                entry_with(comp(&text("a1\na2\na3")), EntryOpts {
                    shrink: Some(1.0),
                    min_size: Some(1.0),
                    ..Default::default()
                }),
                entry_with(comp(&text("b1\nb2\nb3")), EntryOpts {
                    shrink: Some(0.0),
                    ..Default::default()
                }),
            ]),
            10,
            4,
            || {},
        );

        assert_eq!(
            frame.root.children.iter().map(|child| child.rect.height).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(visible_lines(&frame.lines), sl(&["a1", "b1", "b2", "b3"]));
    }

    #[test]
    fn includes_nested_minimum_sizes_in_intrinsic_stack_measurement() {
        let dock = vstack(vec![
            entry(comp(&text("top1\ntop2\ntop3"))),
            entry_with(comp(&text("selector")), EntryOpts {
                min_size: Some(3.0),
                ..Default::default()
            }),
            entry(comp(&text("below"))),
            entry_with(comp(&text("footer")), EntryOpts {
                min_size: Some(1.0),
                ..Default::default()
            }),
        ]);
        let frame = render_layout_frame(
            &vstack(vec![
                entry_with(comp(&text("body")), EntryOpts {
                    basis: Some(Basis::Points(0.0)),
                    grow: Some(1.0),
                    min_size: Some(1.0),
                    ..Default::default()
                }),
                entry_with(dock, EntryOpts {
                    basis: Some(Basis::Auto),
                    min_size: Some(1.0),
                    ..Default::default()
                }),
            ]),
            10,
            9,
            || {},
        );

        assert_eq!(
            visible_lines(&frame.lines),
            sl(&["body", "top1", "top2", "top3", "selector", "", "", "below", "footer"])
        );
    }

    #[test]
    fn omits_gaps_around_invisible_entries() {
        let stack = vstack_with_gap(
            vec![
                entry(comp(&text("one"))),
                entry_with(comp(&text("hidden")), EntryOpts {
                    visible: Some(Rc::new(|_viewport| false)),
                    ..Default::default()
                }),
                entry(comp(&text("two"))),
            ],
            1,
        );
        assert_eq!(
            stack
                .borrow_mut()
                .render(10)
                .iter()
                .map(|line| line.trim_end().to_string())
                .collect::<Vec<_>>(),
            sl(&["one", "", "two"])
        );
    }

    #[test]
    fn crops_kitty_images_at_a_scroll_views_lower_boundary() {
        let image_id = 124;
        let image_line = encode_kitty("AAAA", 2, 3, Some(image_id), false);
        register_kitty_image_metadata(KittyImageMetadata {
            image_id,
            columns: 2,
            rows: 3,
            width_px: 100,
            height_px: 100,
        });
        let transcript_content = fn_component(move |_width| {
            vec![
                "one".to_string(),
                "two".to_string(),
                image_line.clone(),
                String::new(),
                String::new(),
            ]
        });
        let (transcript, _) = scroll_view(transcript_content, SvOpts::default());
        let frame = render_layout_frame(
            &vstack(vec![
                entry_with(transcript, EntryOpts {
                    basis: Some(Basis::Points(0.0)),
                    grow: Some(1.0),
                    ..Default::default()
                }),
                entry(comp(&text("dock"))),
            ]),
            20,
            4,
            || {},
        );

        assert!(frame.lines[2].contains("y=0,h=34,r=1"));
    }

    #[test]
    fn composes_horizontal_children_at_allocated_widths() {
        let frame = render_layout_frame(
            &hstack(vec![
                entry_with(comp(&text("left")), EntryOpts {
                    basis: Some(Basis::Points(6.0)),
                    shrink: Some(0.0),
                    ..Default::default()
                }),
                entry_with(comp(&text("right")), EntryOpts {
                    basis: Some(Basis::Points(6.0)),
                    shrink: Some(0.0),
                    ..Default::default()
                }),
            ]),
            12,
            1,
            || {},
        );
        assert_eq!(visible_lines(&frame.lines), sl(&["left  right"]));
    }

    #[test]
    fn does_not_paint_zero_width_horizontal_children() {
        let frame = render_layout_frame(
            &hstack(vec![
                entry_with(comp(&text("hidden")), EntryOpts {
                    basis: Some(Basis::Points(0.0)),
                    shrink: Some(0.0),
                    ..Default::default()
                }),
                entry_with(comp(&text("shown")), EntryOpts {
                    basis: Some(Basis::Points(0.0)),
                    grow: Some(1.0),
                    ..Default::default()
                }),
            ]),
            5,
            1,
            || {},
        );
        assert_eq!(visible_lines(&frame.lines), sl(&["shown"]));
    }

    #[test]
    fn tracks_follow_end_state_and_returns_unused_scroll_delta() {
        let (scroll_view_component, state) = scroll_view(
            comp(&text("1\n2\n3\n4\n5\n6")),
            SvOpts { follow_end: true, primary: true, ..Default::default() },
        );
        render_layout_frame(&scroll_view_component, 10, 3, || {});
        assert_eq!(state.borrow().scroll_top(), 3);
        assert!(state.borrow().is_following_end());

        assert_eq!(state.borrow_mut().scroll_by(-2), 0);
        assert_eq!(state.borrow().scroll_top(), 1);
        assert!(!state.borrow().is_following_end());
        assert_eq!(state.borrow_mut().scroll_by(-3), -2);
        assert_eq!(state.borrow().scroll_top(), 0);
        assert_eq!(state.borrow_mut().scroll_by(10), 7);
        assert_eq!(state.borrow().scroll_top(), 3);
        assert!(state.borrow().is_following_end());
    }

    #[test]
    fn renders_a_proportional_glyph_scrollbar_with_an_expanded_active_thumb() {
        let source_lines = ["abcd界", "abcde2", "abcde3", "abcde4", "abcde5", "abcde6", "abcde7", "abcde8"];
        let content_background = "\x1b[42m";
        let track_color = "\x1b[38;5;2m";
        let thumb_color = "\x1b[38;5;1m";
        let track_style: Rc<dyn Fn(&str) -> String> =
            Rc::new(move |text: &str| format!("{track_color}{text}\x1b[39m"));
        let thumb_style: Rc<dyn Fn(&str) -> String> =
            Rc::new(move |text: &str| format!("{thumb_color}{text}\x1b[39m"));
        let content = comp(&text_bg(
            &source_lines.join("\n"),
            Rc::new(move |text: &str| format!("{content_background}{text}\x1b[49m")),
        ));
        let (scroll_view_component, state) = scroll_view(
            content.clone(),
            SvOpts {
                scrollbar: Some(ScrollViewScrollbar::Auto),
                track_style: Some(Rc::clone(&track_style)),
                thumb_style: Some(Rc::clone(&thumb_style)),
                hide_delay_ms: Some(10),
                ..Default::default()
            },
        );
        let render = || render_layout_frame(&scroll_view_component, 6, 4, || {}).lines;
        let visible = |lines: &[String]| lines.iter().map(|line| strip_terminal_sequences(line)).collect::<Vec<_>>();

        let lines = render();
        assert_eq!(visible(&lines), sl(&source_lines[..4]));

        state.borrow_mut().scroll_by(2);
        let lines = render();
        assert_eq!(visible(&lines), sl(&["abcde│", "abcde┃", "abcde┃", "abcde│"]));
        assert_eq!(
            lines.iter().map(|line| line.contains(track_color)).collect::<Vec<_>>(),
            vec![true, false, false, true]
        );
        assert_eq!(
            lines.iter().map(|line| line.contains(thumb_color)).collect::<Vec<_>>(),
            vec![false, true, true, false]
        );

        state.borrow_mut().set_scrollbar_active(true);
        let lines = render();
        assert_eq!(visible(&lines), sl(&["abcde│", "abcde█", "abcde█", "abcde│"]));
        assert_eq!(
            lines.iter().map(|line| line.contains(thumb_color)).collect::<Vec<_>>(),
            vec![false, true, true, false]
        );
        assert!(lines[1].rfind(content_background).unwrap() < lines[1].rfind(thumb_color).unwrap());

        state.borrow_mut().set_scrollbar_active(false);
        // [divergence] The upstream test awaits the 30ms hide timer; the
        // port expires the modeled timer explicitly (no event loop).
        state.borrow_mut().expire_scrollbar_hide_timer();
        let lines = render();
        assert_eq!(visible(&lines), sl(&source_lines[2..6]));

        state.borrow_mut().scroll_to_end();
        let lines = render();
        assert_eq!(visible(&lines), sl(&["abcde│", "abcde│", "abcde┃", "abcde┃"]));

        state.borrow_mut().scroll_to_start();
        let lines = render();
        assert_eq!(visible(&lines)[0], "abcd ┃");

        let followed_content = text(&source_lines.join("\n"));
        let (followed_component, followed) = scroll_view(
            comp(&followed_content),
            SvOpts {
                follow_end: true,
                scrollbar: Some(ScrollViewScrollbar::Auto),
                track_style: Some(Rc::clone(&track_style)),
                thumb_style: Some(Rc::clone(&thumb_style)),
                ..Default::default()
            },
        );
        render_layout_frame(&followed_component, 6, 4, || {});
        assert_eq!(followed.borrow().scroll_top(), 4);
        followed_content
            .borrow_mut()
            .set_text(&format!("{}\nabcde9", source_lines.join("\n")));
        let growth_frame = render_layout_frame(&followed_component, 6, 4, || {});
        assert_eq!(followed.borrow().scroll_top(), 5);
        assert!(growth_frame
            .lines
            .iter()
            .all(|line| !strip_terminal_sequences(line).contains(['│', '┃'])));

        let fitting_content = text("1\n2");
        let (automatic_component, automatic) = scroll_view(
            comp(&fitting_content),
            SvOpts {
                scrollbar: Some(ScrollViewScrollbar::Auto),
                thumb_style: Some(Rc::clone(&thumb_style)),
                ..Default::default()
            },
        );
        render_layout_frame(&automatic_component, 6, 4, || {});
        automatic.borrow_mut().scroll_by(1);
        assert!(render_layout_frame(&automatic_component, 6, 4, || {})
            .lines
            .iter()
            .all(|line| !strip_terminal_sequences(line).contains(['│', '┃'])));

        let (always_fitting_component, _) = scroll_view(
            comp(&text("1\n2")),
            SvOpts {
                scrollbar: Some(ScrollViewScrollbar::Always),
                thumb_style: Some(Rc::clone(&thumb_style)),
                ..Default::default()
            },
        );
        let always_fitting_frame = render_layout_frame(&always_fitting_component, 6, 4, || {});
        assert_eq!(always_fitting_frame.root.children[0].rect.width, 5);
        assert!(visible(&always_fitting_frame.lines)
            .iter()
            .all(|line| line.ends_with('┃')));

        let (always_overflowing_component, _) = scroll_view(
            content,
            SvOpts {
                scrollbar: Some(ScrollViewScrollbar::Always),
                track_style: Some(Rc::clone(&track_style)),
                thumb_style: Some(Rc::clone(&thumb_style)),
                ..Default::default()
            },
        );
        let always_overflowing_frame = render_layout_frame(&always_overflowing_component, 6, 4, || {});
        assert_eq!(always_overflowing_frame.root.children[0].rect.width, 5);
        assert_eq!(
            visible(&always_overflowing_frame.lines)
                .iter()
                .filter(|line| line.ends_with('┃'))
                .count(),
            2
        );
        assert_eq!(
            visible(&always_overflowing_frame.lines)
                .iter()
                .filter(|line| line.ends_with('│'))
                .count(),
            2
        );
        for line in &always_overflowing_frame.lines {
            let scrollbar_style_index = line
                .rfind(track_color)
                .map(|index| index as i64)
                .unwrap_or(-1)
                .max(line.rfind(thumb_color).map(|index| index as i64).unwrap_or(-1));
            let reserved_column_reset_index = if scrollbar_style_index < 0 {
                -1
            } else {
                line[..scrollbar_style_index as usize]
                    .rfind("\x1b[0m\x1b]8;;\x07")
                    .map(|index| index as i64)
                    .unwrap_or(-1)
            };
            assert!(
                reserved_column_reset_index > line.rfind(content_background).map(|index| index as i64).unwrap_or(-1)
            );
        }

        let thumb_height_for = |content_height: i64| -> usize {
            let sized_content = text(&vec!["x"; content_height as usize].join("\n"));
            let (sized_component, sized) = scroll_view(
                comp(&sized_content),
                SvOpts {
                    scrollbar: Some(ScrollViewScrollbar::Auto),
                    thumb_style: Some(Rc::clone(&thumb_style)),
                    ..Default::default()
                },
            );
            render_layout_frame(&sized_component, 6, 20, || {});
            sized.borrow_mut().scroll_by(1);
            render_layout_frame(&sized_component, 6, 20, || {})
                .lines
                .iter()
                .filter(|line| strip_terminal_sequences(line).ends_with('┃'))
                .count()
        };
        assert_eq!(thumb_height_for(21), 19);
        assert_eq!(thumb_height_for(40), 10);
        assert_eq!(thumb_height_for(100), 4);
        assert_eq!(thumb_height_for(400), 2);
    }

    #[test]
    fn preserves_only_the_underlying_background_beneath_overlay_scrollbar_glyphs() {
        let background = "\x1b[42m";
        let border_foreground = "\x1b[31m";
        let content = fn_component(move |width: usize| {
            (0..8)
                .map(|_| {
                    format!(
                        "{background}{}{border_foreground}│\x1b[39m\x1b[49m",
                        "x".repeat(width - 1)
                    )
                })
                .collect()
        });
        let (scroll_view_component, state) = scroll_view(
            content,
            SvOpts {
                scrollbar: Some(ScrollViewScrollbar::Auto),
                track_style: Some(Rc::new(|text: &str| text.to_string())),
                thumb_style: Some(Rc::new(|text: &str| text.to_string())),
                ..Default::default()
            },
        );
        render_layout_frame(&scroll_view_component, 6, 4, || {});
        state.borrow_mut().scroll_by(1);
        let frame = render_layout_frame(&scroll_view_component, 6, 4, || {});

        assert_eq!(
            frame.lines.iter().map(|line| strip_terminal_sequences(line)).collect::<Vec<_>>(),
            sl(&["xxxxx│", "xxxxx┃", "xxxxx┃", "xxxxx│"])
        );
        for line in &frame.lines {
            assert!(line.contains(background));
            assert!(!line.contains(border_foreground));
            assert!(line.contains(&format!("\x1b[0m\x1b]8;;\x07{background}")));
        }
    }

    #[test]
    fn updates_reserved_scrollbar_layout_at_runtime() {
        let content = text("123456");
        let (scroll_view_component, state) = scroll_view(
            comp(&content),
            SvOpts { scrollbar: Some(ScrollViewScrollbar::Always), ..Default::default() },
        );
        let render = || {
            render_layout_frame(
                &hstack_with_align(vec![entry(Rc::clone(&scroll_view_component))], StackAlign::Start),
                6,
                2,
                || {},
            )
        };
        let always = render();
        assert_eq!(visible_lines(&always.lines), sl(&["12345┃", "6    ┃"]));
        assert_eq!(always.root.children[0].rect.width, 6);
        assert_eq!(always.root.children[0].children[0].rect.width, 5);

        state.borrow_mut().set_scrollbar(ScrollViewScrollbar::Hidden);
        let hidden = render();
        assert_eq!(hidden.root.children[0].children[0].rect.width, 6);
        assert!(!state.borrow().is_scrollbar_visible());
    }

    #[test]
    fn measures_nested_scroll_content_from_constrained_child_geometry() {
        let inner_content = text("1\n2\n3\n4\n5\n6");
        let (inner, inner_state) = scroll_view(comp(&inner_content), SvOpts::default());
        let outer_content = vstack(vec![
            entry_with(inner, EntryOpts {
                basis: Some(Basis::Points(2.0)),
                ..Default::default()
            }),
            entry(comp(&text("tail"))),
        ]);
        let (outer, outer_state) = scroll_view(outer_content, SvOpts::default());
        render_layout_frame(&outer, 10, 2, || {});

        assert_eq!(inner_state.borrow().viewport_height(), 2);
        assert_eq!(outer_state.borrow_mut().scroll_by(10), 9);
        assert_eq!(outer_state.borrow().scroll_top(), 1);
    }

    #[test]
    fn rebuilds_geometry_after_content_changes() {
        let text_component = text("one");
        let root = vstack(vec![entry(comp(&text_component))]);
        let first = render_layout_frame(&root, 10, 4, || {});
        text_component.borrow_mut().set_text("one\ntwo\nthree");
        let second = render_layout_frame(&root, 10, 4, || {});

        assert_eq!(first.root.children[0].lines.as_ref().map(|lines| lines.len()), Some(1));
        assert_eq!(second.root.children[0].lines.as_ref().map(|lines| lines.len()), Some(3));
    }
}
