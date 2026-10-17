//! Port of `packages/tui/src/components/scroll-view.ts` — `ScrollView`, the
//! single-child vertical scroll container that feeds the flex layout
//! solver its scroll state.
//!
//! ## Modeling notes
//!
//! - TS `ScrollView extends Container` (scroll-view.ts:22) with exactly one
//!   child pushed into the inherited child list; `addChild`/`removeChild`/
//!   `clear` throw. The Rust model splits the TS object in two, mirroring
//!   the landed layout solver's expectations (`layout.rs`):
//!   - [`ScrollView`] is the [`crate::component::Component`] half (render,
//!     inherited `Container` mouse routing, invalidate);
//!   - [`ScrollViewStateData`] is the shared state half — the TS object as
//!     the layout solver sees it. It implements both
//!     [`crate::layout_node::ScrollLayoutState`] and
//!     [`crate::layout::ScrollViewState`] (the scrollbar-bearing surface
//!     `layout.ts` reads), and is handed to the solver through
//!     [`crate::layout::scroll_layout_node`], which registers the extended
//!     view so `get_scroll_views_at`/`get_scroll_view_box` can recover it
//!     from the frozen `ScrollLayoutNode.state` field.
//! - **`requestRender` seam** (scroll-view.ts:36, 97): TS stores the
//!   `requestRender` callback passed to `updateLayout` and invokes it from
//!   every state mutation. A `&mut dyn FnMut()` cannot be retained past
//!   the call, so the port models it as an injectable callback
//!   ([`ScrollView::set_request_render_callback`], invoked at every TS
//!   `this.requestRenderCallback?.()` call site) plus a
//!   [`ScrollViewStateData::render_requested`] flag that records requests
//!   even when no callback is installed.
//! - **Transient scrollbar timer** (scroll-view.ts:126-149): TS arms a
//!   `setTimeout(hideDelayMs)` to hide an `auto` scrollbar. There are no
//!   threads or an event loop here, so the armed timer is the
//!   [`ScrollViewStateData::hide_timer_armed`] flag and
//!   [`ScrollView::expire_scrollbar_hide_timer`] fires it explicitly —
//!   the same modeling the landed `layout.rs` tests use.
//! - **`render` does not apply the scroll offset**: TS `ScrollView.render`
//!   (scroll-view.ts:203-207) renders the child at the content width and
//!   reserves the scrollbar column; the scrollTop line slicing happens in
//!   the layout solver (`layout.rs`), which owns the viewport. Parity
//!   preserved.
//! - `ScrollViewScrollbar` is defined in [`crate::layout`] (it landed with
//!   the solver, which reads it off the scroll state); it is re-exported
//!   here to mirror the TS export from scroll-view.ts:4.

use std::cell::RefCell;
use std::rc::Rc;

use crate::component::{
    Component, ComponentMouseResult, ComponentRef, Container, TuiMouseEvent,
};
use crate::layout::{scroll_layout_node, ScrollViewScrollbar};
use crate::layout_node::{LayoutNode, Overscroll, ScrollLayoutState};

// TS re-export (scroll-view.ts:4): the enum itself lives in `layout.rs`.
pub use crate::layout::ScrollViewState as ScrollViewStateSurface;

/// `axis?: "vertical"` (scroll-view.ts:7). The TS runtime guard
/// (`Unsupported ScrollView axis: ...`, scroll-view.ts:43-45) rejects any
/// non-vertical axis; with a single-variant enum the invalid states are
/// unrepresentable, so the throw is encoded in the type system instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollViewAxis {
    Vertical,
}

/// `follow?: "none" | "end"` (scroll-view.ts:8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollViewFollow {
    #[default]
    None,
    End,
}

/// `ScrollViewOptions` (scroll-view.ts:6-15). `None` fields model the TS
/// `undefined` and fall back to the constructor defaults.
pub struct ScrollViewOptions {
    pub axis: Option<ScrollViewAxis>,
    pub follow: Option<ScrollViewFollow>,
    pub primary: bool,
    pub overscroll: Option<Overscroll>,
    pub scrollbar: Option<ScrollViewScrollbar>,
    /// `scrollbarTrackStyle?: (text: string) => string`.
    pub scrollbar_track_style: Option<Rc<dyn Fn(&str) -> String>>,
    /// `scrollbarThumbStyle?: (text: string) => string`.
    pub scrollbar_thumb_style: Option<Rc<dyn Fn(&str) -> String>>,
    /// `scrollbarHideDelayMs?: number` — normalized by
    /// `max(0, floor(value ?? 1000))` (scroll-view.ts:55).
    pub scrollbar_hide_delay_ms: Option<f64>,
}

impl Default for ScrollViewOptions {
    fn default() -> Self {
        ScrollViewOptions {
            axis: None,
            follow: None,
            primary: false,
            overscroll: None,
            scrollbar: None,
            scrollbar_track_style: None,
            scrollbar_thumb_style: None,
            scrollbar_hide_delay_ms: None,
        }
    }
}

/// `ScrollViewScrollToOptions` (scroll-view.ts:17-20).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScrollViewScrollToOptions {
    /// Keep follow-end disabled even when the target is the current
    /// content end (`disableFollow`, scroll-view.ts:19).
    pub disable_follow: bool,
}

// =============================================================================
// ScrollViewStateData — the shared state half
// =============================================================================

/// The shared state half of the TS `ScrollView` object: every field and
/// method of scroll-view.ts except the `Component` surface. Held behind an
/// `Rc<RefCell<...>>` so the layout solver can address it through
/// [`crate::layout_node::ScrollLayoutState`] while callers keep the
/// concrete [`ScrollView`] component.
pub struct ScrollViewStateData {
    child: ComponentRef,
    /// The inherited `Container` child list (TS `this.children.push`,
    /// scroll-view.ts:47) used by the inherited mouse routing.
    pub(crate) container: Container,
    /// `readonly followEnd` (scroll-view.ts:24).
    follow_end: bool,
    /// `private followingEnd` (scroll-view.ts:34).
    following_end: bool,
    /// `readonly primary` (scroll-view.ts:25).
    primary: bool,
    /// `readonly overscroll` (scroll-view.ts:26).
    overscroll: Overscroll,
    /// `readonly scrollbarTrackStyle` (scroll-view.ts:27).
    track_style: Option<Rc<dyn Fn(&str) -> String>>,
    /// `readonly scrollbarThumbStyle` (scroll-view.ts:28).
    thumb_style: Option<Rc<dyn Fn(&str) -> String>>,
    /// `private readonly scrollbarHideDelayMs` (scroll-view.ts:30).
    hide_delay_ms: i64,
    /// `private currentScrollbar` (scroll-view.ts:29).
    scrollbar: ScrollViewScrollbar,
    /// `private currentScrollTop` (scroll-view.ts:31).
    scroll_top: i64,
    /// `private contentHeight` (scroll-view.ts:32).
    content_height: i64,
    /// `private currentViewportHeight` (scroll-view.ts:33).
    viewport_height: i64,
    /// `private followSuppressedAtEnd` (scroll-view.ts:35).
    follow_suppressed_at_end: bool,
    /// `private requestRenderCallback` (scroll-view.ts:36) — see the
    /// module notes: injectable callback + recorded flag.
    request_render_callback: Option<Rc<dyn Fn()>>,
    render_requested: bool,
    /// `private transientScrollbarVisible` (scroll-view.ts:37).
    transient_scrollbar_visible: bool,
    /// `private scrollbarActive` (scroll-view.ts:38).
    scrollbar_active: bool,
    /// `private scrollbarHideTimer` (scroll-view.ts:39) — modeled as the
    /// armed flag (see module notes).
    hide_timer_armed: bool,
}

impl ScrollViewStateData {
    /// The constructor half operating on the state (scroll-view.ts:41-56).
    /// The `super()` + `this.children.push(component)` lines build the
    /// inherited container child list.
    pub(crate) fn new(child: ComponentRef, options: ScrollViewOptions) -> Self {
        // scroll-view.ts:43-45: `axis !== undefined && axis !== "vertical"`
        // throws. `ScrollViewAxis` has only `Vertical`, so the guard is
        // satisfied by construction.
        let follow_end = options.follow.unwrap_or(ScrollViewFollow::None) == ScrollViewFollow::End;
        let mut container = Container::new();
        container.add_child(child.clone());
        ScrollViewStateData {
            child,
            container,
            follow_end,
            following_end: follow_end,
            primary: options.primary,
            overscroll: options.overscroll.unwrap_or(Overscroll::Chain),
            track_style: options.scrollbar_track_style,
            thumb_style: options.scrollbar_thumb_style,
            hide_delay_ms: options
                .scrollbar_hide_delay_ms
                .unwrap_or(1000.0)
                .floor()
                .max(0.0) as i64,
            scrollbar: options.scrollbar.unwrap_or(ScrollViewScrollbar::Hidden),
            scroll_top: 0,
            content_height: 0,
            viewport_height: 0,
            follow_suppressed_at_end: false,
            request_render_callback: None,
            render_requested: false,
            transient_scrollbar_visible: false,
            scrollbar_active: false,
            hide_timer_armed: false,
        }
    }

    /// `get scrollTop()` (scroll-view.ts:58-60).
    pub fn scroll_top(&self) -> i64 {
        self.scroll_top
    }

    /// `get isFollowingEnd()` (scroll-view.ts:62-64).
    pub fn is_following_end(&self) -> bool {
        self.following_end
    }

    /// `get viewportHeight()` (scroll-view.ts:66-68).
    pub fn viewport_height(&self) -> i64 {
        self.viewport_height
    }

    /// `get scrollbar()` (scroll-view.ts:70-72).
    pub fn scrollbar(&self) -> ScrollViewScrollbar {
        self.scrollbar
    }

    /// `get isScrollbarVisible()` (scroll-view.ts:74-80).
    pub fn is_scrollbar_visible(&self) -> bool {
        if self.scrollbar == ScrollViewScrollbar::Always {
            return self.viewport_height > 0;
        }
        self.scrollbar == ScrollViewScrollbar::Auto
            && self.content_height > self.viewport_height
            && self.transient_scrollbar_visible
    }

    /// `get isScrollbarActive()` (scroll-view.ts:82-84).
    pub fn is_scrollbar_active(&self) -> bool {
        self.scrollbar_active
    }

    /// `readonly followEnd` (scroll-view.ts:24).
    pub fn follow_end(&self) -> bool {
        self.follow_end
    }

    /// `readonly primary` (scroll-view.ts:25).
    pub fn primary(&self) -> bool {
        self.primary
    }

    /// `readonly overscroll` (scroll-view.ts:26).
    pub fn overscroll(&self) -> Overscroll {
        self.overscroll
    }

    /// The single child (TS `private readonly child`, scroll-view.ts:23).
    pub fn child(&self) -> &ComponentRef {
        &self.child
    }

    /// `readonly scrollbarTrackStyle` (scroll-view.ts:27) with the default
    /// from scroll-view.ts:53.
    pub fn scrollbar_track_style(&self, text: &str) -> String {
        match &self.track_style {
            Some(style) => style(text),
            None => format!("\x1b[90m{text}\x1b[39m"),
        }
    }

    /// `readonly scrollbarThumbStyle` (scroll-view.ts:28) with the default
    /// from scroll-view.ts:54.
    pub fn scrollbar_thumb_style(&self, text: &str) -> String {
        match &self.thumb_style {
            Some(style) => style(text),
            None => format!("\x1b[37m{text}\x1b[39m"),
        }
    }

    /// `private markScrollbarActivity()` (scroll-view.ts:126-138): reveals
    /// the `auto` scrollbar and (re)arms the hide timer when the scrollbar
    /// is not being dragged.
    fn mark_scrollbar_activity(&mut self) {
        if self.scrollbar != ScrollViewScrollbar::Auto
            || self.content_height <= self.viewport_height
        {
            return;
        }
        self.transient_scrollbar_visible = true;
        // `clearTimeout(this.scrollbarHideTimer)` (scroll-view.ts:130-133).
        self.hide_timer_armed = false;
        if self.scrollbar_active {
            return;
        }
        // `setTimeout(...)` + `unref()` (scroll-view.ts:135-141).
        self.hide_timer_armed = true;
    }

    /// `private hideTransientScrollbar()` (scroll-view.ts:140-146).
    fn hide_transient_scrollbar(&mut self) {
        self.transient_scrollbar_visible = false;
        if !self.hide_timer_armed {
            return;
        }
        self.hide_timer_armed = false;
    }

    /// [divergence] Fires the modeled hide timer
    /// (`scrollbarHideTimer` callback, scroll-view.ts:136-139): hides the
    /// transient scrollbar and requests a render. Upstream waits out
    /// `scrollbarHideDelayMs`; there is no event loop here.
    pub fn expire_scrollbar_hide_timer(&mut self) {
        if self.hide_timer_armed {
            self.hide_timer_armed = false;
            self.transient_scrollbar_visible = false;
            self.request_render();
        }
    }

    /// `setScrollbar(scrollbar)` (scroll-view.ts:86-93).
    pub fn set_scrollbar(&mut self, scrollbar: ScrollViewScrollbar) {
        if scrollbar == self.scrollbar {
            return;
        }
        self.scrollbar = scrollbar;
        if scrollbar != ScrollViewScrollbar::Auto {
            self.hide_transient_scrollbar();
        } else if self.scrollbar_active {
            self.mark_scrollbar_activity();
        }
        self.request_render();
    }

    /// `setScrollbarActive(active)` (scroll-view.ts:108-113).
    pub fn set_scrollbar_active(&mut self, active: bool) {
        if active == self.scrollbar_active {
            return;
        }
        self.scrollbar_active = active;
        self.mark_scrollbar_activity();
        self.request_render();
    }

    /// `getContentWidth(width)` (scroll-view.ts:95-97): an `always`
    /// scrollbar reserves one content column.
    pub fn get_content_width(&self, width: i64) -> i64 {
        if self.scrollbar == ScrollViewScrollbar::Always && width > 1 {
            width - 1
        } else {
            width
        }
    }

    /// `scrollTo(scrollTop, options)` (scroll-view.ts:115-131). Clamps to
    /// `[0, maxScrollTop]` and manages the follow-end latch. (TS rejects
    /// non-finite input by keeping the current position; `i64` is always
    /// finite, so that branch is unrepresentable.)
    pub fn scroll_to(&mut self, scroll_top: i64, options: ScrollViewScrollToOptions) {
        let requested = scroll_top;
        let max_scroll_top = (self.content_height - self.viewport_height).max(0);
        let next = requested.clamp(0, max_scroll_top);
        let next_follow_suppressed_at_end = options.disable_follow && next == max_scroll_top;
        let next_following_end =
            !next_follow_suppressed_at_end && self.follow_end && next == max_scroll_top;
        if next == self.scroll_top
            && next_following_end == self.following_end
            && next_follow_suppressed_at_end == self.follow_suppressed_at_end
        {
            return;
        }
        let moved = next != self.scroll_top;
        self.scroll_top = next;
        self.following_end = next_following_end;
        self.follow_suppressed_at_end = next_follow_suppressed_at_end;
        if moved {
            self.mark_scrollbar_activity();
        }
        self.request_render();
    }

    /// `scrollBy(lines)` (scroll-view.ts:133-147). Returns the unused
    /// delta (`requested - moved`) so an outer scroll view can chain the
    /// overscroll.
    pub fn scroll_by(&mut self, lines: i64) -> i64 {
        let requested = lines;
        if requested == 0 {
            return 0;
        }
        let max_scroll_top = (self.content_height - self.viewport_height).max(0);
        let start = if self.following_end {
            max_scroll_top
        } else {
            self.scroll_top
        };
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
            self.request_render();
        }
        requested - moved
    }

    /// `scrollToStart()` (scroll-view.ts:149-158).
    pub fn scroll_to_start(&mut self) {
        let changed = self.scroll_top != 0
            || self.following_end != (self.follow_end && self.content_height <= self.viewport_height);
        self.scroll_top = 0;
        self.following_end = self.follow_end && self.content_height <= self.viewport_height;
        self.follow_suppressed_at_end = false;
        if changed {
            self.mark_scrollbar_activity();
            self.request_render();
        }
    }

    /// `scrollToEnd()` (scroll-view.ts:160-169).
    pub fn scroll_to_end(&mut self) {
        let next = (self.content_height - self.viewport_height).max(0);
        let changed = self.scroll_top != next || self.following_end != self.follow_end;
        self.scroll_top = next;
        self.following_end = self.follow_end;
        self.follow_suppressed_at_end = false;
        if changed {
            self.mark_scrollbar_activity();
            self.request_render();
        }
    }

    /// `updateLayout(contentHeight, viewportHeight, requestRender)`
    /// (scroll-view.ts:171-187): normalizes the heights, pins/clamps the
    /// scroll position, and re-latches follow-end. TS additionally stores
    /// the `requestRender` callback; see the module notes for the seam
    /// divergence — use [`ScrollView::set_request_render_callback`].
    pub fn update_layout(&mut self, content_height: i64, viewport_height: i64) {
        self.content_height = content_height.max(0);
        self.viewport_height = viewport_height.max(0);
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

    /// [divergence] The `requestRenderCallback` seam: installs the callback
    /// invoked at every TS `this.requestRenderCallback?.()` call site.
    /// TS receives this callback from `updateLayout`; a `&mut dyn FnMut()`
    /// cannot be retained past that call, so it is injected explicitly.
    pub fn set_request_render_callback(&mut self, callback: Option<Rc<dyn Fn()>>) {
        self.request_render_callback = callback;
    }

    /// Whether a render has been requested since the flag was last
    /// cleared (models the presence of `requestRenderCallback` even when
    /// no callback is installed).
    pub fn render_requested(&self) -> bool {
        self.render_requested
    }

    /// Clears the [`Self::render_requested`] flag.
    pub fn clear_render_requested(&mut self) {
        self.render_requested = false;
    }

    /// `this.requestRenderCallback?.()` — records the request and invokes
    /// the injectable callback. The callback runs while the state is
    /// borrowed; it must not re-enter the same scroll view synchronously
    /// (upstream callbacks only schedule an async renderer tick).
    fn request_render(&mut self) {
        self.render_requested = true;
        let callback = self.request_render_callback.clone();
        if let Some(callback) = callback {
            callback();
        }
    }
}

impl ScrollLayoutState for ScrollViewStateData {
    /// `ScrollLayoutState.scrollTop` (layout-node.ts:34).
    fn scroll_top(&self) -> i64 {
        ScrollViewStateData::scroll_top(self)
    }

    /// `ScrollLayoutState.primary` (layout-node.ts:35).
    fn primary(&self) -> bool {
        ScrollViewStateData::primary(self)
    }

    /// `ScrollLayoutState.overscroll` (layout-node.ts:36).
    fn overscroll(&self) -> Overscroll {
        ScrollViewStateData::overscroll(self)
    }

    /// `ScrollLayoutState.viewportHeight` (layout-node.ts:37).
    fn viewport_height(&self) -> i64 {
        ScrollViewStateData::viewport_height(self)
    }

    /// `ScrollLayoutState.getContentWidth` (layout-node.ts:38).
    fn get_content_width(&self, width: i64) -> i64 {
        ScrollViewStateData::get_content_width(self, width)
    }

    /// `ScrollLayoutState.updateLayout` (layout-node.ts:39-40). The
    /// `requestRender` argument is consumed by the seam (see module
    /// notes); the TS math is otherwise exact.
    fn update_layout(
        &mut self,
        content_height: i64,
        viewport_height: i64,
        _request_render: &mut dyn FnMut(),
    ) {
        ScrollViewStateData::update_layout(self, content_height, viewport_height);
    }
}

impl crate::layout::ScrollViewState for ScrollViewStateData {
    /// `layout.ts` reads `scrollView.scrollbar` (layout.ts:154-161).
    fn scrollbar(&self) -> ScrollViewScrollbar {
        ScrollViewStateData::scrollbar(self)
    }

    fn is_scrollbar_visible(&self) -> bool {
        ScrollViewStateData::is_scrollbar_visible(self)
    }

    fn is_scrollbar_active(&self) -> bool {
        ScrollViewStateData::is_scrollbar_active(self)
    }

    fn scrollbar_track_style(&self, text: &str) -> String {
        ScrollViewStateData::scrollbar_track_style(self, text)
    }

    fn scrollbar_thumb_style(&self, text: &str) -> String {
        ScrollViewStateData::scrollbar_thumb_style(self, text)
    }
}

// =============================================================================
// ScrollView — the Component half
// =============================================================================

/// `ScrollView` (scroll-view.ts:22-209) — a single-child vertical scroll
/// container. The TS public API lives here; all state mutations delegate
/// to the shared [`ScrollViewStateData`] (see module notes).
pub struct ScrollView {
    state: Rc<RefCell<ScrollViewStateData>>,
}

impl ScrollView {
    /// `constructor(component, options = {})` (scroll-view.ts:41-56).
    pub fn new(component: ComponentRef, options: ScrollViewOptions) -> Self {
        ScrollView {
            state: Rc::new(RefCell::new(ScrollViewStateData::new(component, options))),
        }
    }

    /// The shared state half (the TS `ScrollView` object as the layout
    /// solver addresses it).
    pub fn state(&self) -> Rc<RefCell<ScrollViewStateData>> {
        Rc::clone(&self.state)
    }

    /// The single child (TS `private readonly child`).
    pub fn child(&self) -> ComponentRef {
        self.state.borrow().child().clone()
    }

    /// `get scrollTop()` (scroll-view.ts:58-60).
    pub fn scroll_top(&self) -> i64 {
        self.state.borrow().scroll_top()
    }

    /// `get isFollowingEnd()` (scroll-view.ts:62-64).
    pub fn is_following_end(&self) -> bool {
        self.state.borrow().is_following_end()
    }

    /// `get viewportHeight()` (scroll-view.ts:66-68).
    pub fn viewport_height(&self) -> i64 {
        self.state.borrow().viewport_height()
    }

    /// `get scrollbar()` (scroll-view.ts:70-72).
    pub fn scrollbar(&self) -> ScrollViewScrollbar {
        self.state.borrow().scrollbar()
    }

    /// `get isScrollbarVisible()` (scroll-view.ts:74-80).
    pub fn is_scrollbar_visible(&self) -> bool {
        self.state.borrow().is_scrollbar_visible()
    }

    /// `get isScrollbarActive()` (scroll-view.ts:82-84).
    pub fn is_scrollbar_active(&self) -> bool {
        self.state.borrow().is_scrollbar_active()
    }

    /// `readonly followEnd` (scroll-view.ts:24).
    pub fn follow_end(&self) -> bool {
        self.state.borrow().follow_end()
    }

    /// `readonly primary` (scroll-view.ts:25).
    pub fn primary(&self) -> bool {
        self.state.borrow().primary()
    }

    /// `readonly overscroll` (scroll-view.ts:26).
    pub fn overscroll(&self) -> Overscroll {
        self.state.borrow().overscroll()
    }

    /// `setScrollbar(scrollbar)` (scroll-view.ts:86-93).
    pub fn set_scrollbar(&self, scrollbar: ScrollViewScrollbar) {
        self.state.borrow_mut().set_scrollbar(scrollbar);
    }

    /// `setScrollbarActive(active)` (scroll-view.ts:108-113).
    pub fn set_scrollbar_active(&self, active: bool) {
        self.state.borrow_mut().set_scrollbar_active(active);
    }

    /// `getContentWidth(width)` (scroll-view.ts:95-97).
    pub fn get_content_width(&self, width: i64) -> i64 {
        self.state.borrow().get_content_width(width)
    }

    /// `scrollTo(scrollTop, options = {})` (scroll-view.ts:115-131).
    pub fn scroll_to(&self, scroll_top: i64, options: ScrollViewScrollToOptions) {
        self.state.borrow_mut().scroll_to(scroll_top, options);
    }

    /// `scrollBy(lines)` (scroll-view.ts:133-147) — returns the unused
    /// delta for overscroll chaining.
    pub fn scroll_by(&self, lines: i64) -> i64 {
        self.state.borrow_mut().scroll_by(lines)
    }

    /// `scrollToStart()` (scroll-view.ts:149-158).
    pub fn scroll_to_start(&self) {
        self.state.borrow_mut().scroll_to_start();
    }

    /// `scrollToEnd()` (scroll-view.ts:160-169).
    pub fn scroll_to_end(&self) {
        self.state.borrow_mut().scroll_to_end();
    }

    /// `updateLayout(contentHeight, viewportHeight, requestRender)`
    /// (scroll-view.ts:171-187). The `requestRender` argument matches the
    /// TS signature; storing it is impossible (see module notes) — install
    /// the persistent callback via [`Self::set_request_render_callback`].
    pub fn update_layout(
        &self,
        content_height: i64,
        viewport_height: i64,
        request_render: &mut dyn FnMut(),
    ) {
        ScrollLayoutState::update_layout(
            &mut *self.state.borrow_mut(),
            content_height,
            viewport_height,
            request_render,
        );
    }

    /// [divergence] The injectable `requestRender` seam (see module notes).
    pub fn set_request_render_callback(&self, callback: Option<Rc<dyn Fn()>>) {
        self.state
            .borrow_mut()
            .set_request_render_callback(callback);
    }

    /// [divergence] Fires the modeled scrollbar hide timer (see module
    /// notes).
    pub fn expire_scrollbar_hide_timer(&self) {
        self.state.borrow_mut().expire_scrollbar_hide_timer();
    }

    /// Whether a render has been requested since the flag was last
    /// cleared.
    pub fn render_requested(&self) -> bool {
        self.state.borrow().render_requested()
    }

    /// Clears the render-requested flag.
    pub fn clear_render_requested(&mut self) {
        self.state.borrow_mut().clear_render_requested();
    }

    /// `addChild` (scroll-view.ts:189-191) — always throws.
    pub fn add_child(&mut self, _component: ComponentRef) {
        panic!("ScrollView has exactly one child");
    }

    /// `removeChild` (scroll-view.ts:193-195) — always throws.
    pub fn remove_child(&mut self, _component: &ComponentRef) {
        panic!("ScrollView child cannot be removed");
    }

    /// `clear` (scroll-view.ts:197-199) — always throws.
    pub fn clear(&mut self) {
        panic!("ScrollView child cannot be cleared");
    }
}

impl Component for ScrollView {
    /// `render(width)` (scroll-view.ts:203-207): the child rendered at the
    /// content width; with a reserved scrollbar column every line gains a
    /// trailing space. The scrollTop offset is applied by the layout
    /// solver, not here (parity with TS).
    fn render(&mut self, width: usize) -> Vec<String> {
        let state = self.state.borrow();
        let content_width = state.get_content_width(width as i64);
        let lines = state
            .child
            .borrow_mut()
            .render(content_width.max(0) as usize);
        if content_width == width as i64 {
            lines
        } else {
            lines.into_iter().map(|line| format!("{line} ")).collect()
        }
    }

    /// TS `ScrollView` inherits `Container`'s (absent) `handleInput`
    /// presence; the Rust `Container` foundation models that presence as
    /// `true` (see `component.rs`), inherited unchanged like `VStack`.
    fn handles_input(&self) -> bool {
        true
    }

    /// `invalidate()` — inherited from `Container` (tui.ts:376-380):
    /// invalidates the child list.
    fn invalidate(&mut self) {
        let mut state = self.state.borrow_mut();
        Component::invalidate(&mut state.container);
    }

    /// `handleMouse` — inherited unchanged from `Container`
    /// (tui.ts:382-405): routes to the child under the cursor using the
    /// render-height layout captured by the last `Container.render` (the
    /// TS `ScrollView.render` override never populates that cache, so the
    /// first mouse event rebuilds it at `event.width` — quirk preserved).
    fn handle_mouse(
        &mut self,
        self_ref: &ComponentRef,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        let mut state = self.state.borrow_mut();
        Component::handle_mouse(&mut state.container, self_ref, event)
    }

    /// `[LAYOUT_NODE]()` (scroll-view.ts:209-211):
    /// `{ type: "scroll", component: this.child, state: this }`. Built
    /// through [`crate::layout::scroll_layout_node`], which registers the
    /// extended (scrollbar-bearing) state view for the solver.
    fn layout_node(&mut self) -> Option<LayoutNode> {
        let child = self.state.borrow().child().clone();
        Some(scroll_layout_node(child, Rc::clone(&self.state)))
    }
}

// =============================================================================
// Tests
// =============================================================================
//
// Upstream coverage: `test/mouse-components.test.ts` contains **no**
// ScrollView tests (every test there exercises Input / SelectList /
// SettingsList / Editor / TuiAltScreen — see the report). The ScrollView
// behaviors upstream are exercised in `test/layout.test.ts` (ported with
// the solver against its own local fixture in `layout.rs`) and
// `test/tui-alt-screen.test.ts` (renderer-driven, tranche T5). The tests
// below port the scroll-view.ts state machine against this component.

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use crate::component::{TuiMouseButton, TuiMouseEventResult, TuiMouseEventType};
    use crate::layout_node::ScrollLayoutState as _;

    /// Test-only leaf: renders fixed lines; captures mouse events at
    /// local y == 0; counts invalidate() calls.
    struct Leaf {
        lines: Vec<String>,
        invalidated: Cell<bool>,
    }

    impl Component for Leaf {
        fn render(&mut self, _width: usize) -> Vec<String> {
            self.lines.clone()
        }

        fn handle_mouse(
            &mut self,
            _self_ref: &ComponentRef,
            event: &TuiMouseEvent,
        ) -> Option<ComponentMouseResult> {
            if event.y == 0 {
                Some(ComponentMouseResult::Result(TuiMouseEventResult::capture()))
            } else {
                None
            }
        }

        fn invalidate(&mut self) {
            self.invalidated.set(true);
        }
    }

    fn leaf(lines: &[&str]) -> Rc<RefCell<Leaf>> {
        Rc::new(RefCell::new(Leaf {
            lines: lines.iter().map(|s| s.to_string()).collect(),
            invalidated: Cell::new(false),
        }))
    }

    /// The `mouse(type, x, y, width, height)` helper from
    /// mouse-components.test.ts:12-25.
    fn mouse(
        event_type: TuiMouseEventType,
        x: i64,
        y: i64,
        width: i64,
        height: i64,
    ) -> TuiMouseEvent {
        TuiMouseEvent {
            event_type,
            button: TuiMouseButton::Left,
            x,
            y,
            screen_x: x,
            screen_y: y,
            width,
            height,
            shift: false,
            alt: false,
            ctrl: false,
            wheel_delta: None,
            click_count: if event_type == TuiMouseEventType::Click {
                Some(1)
            } else {
                None
            },
        }
    }

    #[test]
    fn constructor_applies_option_defaults() {
        // scroll-view.ts:48-55: follow "none", primary false, overscroll
        // "chain", scrollbar "hidden", default styles, 1000ms hide delay.
        let sv = ScrollView::new(leaf(&["a"]), ScrollViewOptions::default());
        assert!(!sv.follow_end());
        assert!(!sv.is_following_end());
        assert!(!sv.primary());
        assert_eq!(sv.overscroll(), Overscroll::Chain);
        assert_eq!(sv.scrollbar(), ScrollViewScrollbar::Hidden);
        assert_eq!(sv.scroll_top(), 0);
        assert_eq!(sv.viewport_height(), 0);
        assert!(!sv.is_scrollbar_visible());
        assert_eq!(
            sv.state().borrow().scrollbar_track_style("x"),
            "\x1b[90mx\x1b[39m"
        );
        assert_eq!(
            sv.state().borrow().scrollbar_thumb_style("x"),
            "\x1b[37mx\x1b[39m"
        );
    }

    #[test]
    fn vertical_axis_is_accepted() {
        // scroll-view.ts:43-45 only rejects non-vertical axes.
        let mut sv = ScrollView::new(
            leaf(&["a"]),
            ScrollViewOptions {
                axis: Some(ScrollViewAxis::Vertical),
                ..Default::default()
            },
        );
        assert_eq!(sv.render(5), vec!["a"]);
    }

    #[test]
    fn renders_the_child_at_full_width_by_default() {
        // scroll-view.ts:203-207 with a hidden scrollbar: contentWidth ==
        // width, no padding.
        let mut sv = ScrollView::new(leaf(&["a", "b"]), ScrollViewOptions::default());
        assert_eq!(sv.get_content_width(10), 10);
        assert_eq!(sv.render(10), vec!["a", "b"]);
    }

    #[test]
    fn always_scrollbar_reserves_one_content_column() {
        // scroll-view.ts:95-97 + 203-207: `always` narrows the content by
        // one column and pads every line with a trailing space (including
        // empty ones).
        let mut sv = ScrollView::new(
            leaf(&["a", "", "c"]),
            ScrollViewOptions {
                scrollbar: Some(ScrollViewScrollbar::Always),
                ..Default::default()
            },
        );
        assert_eq!(sv.get_content_width(10), 9);
        assert_eq!(sv.render(10), vec!["a ", " ", "c "]);
        // width <= 1: no column reserved, no padding.
        assert_eq!(sv.get_content_width(1), 1);
        assert_eq!(sv.render(1), vec!["a", "", "c"]);
        // `always` is visible as soon as the viewport is positive
        // (scroll-view.ts:76-77).
        sv.update_layout(2, 2, &mut || {});
        assert!(sv.is_scrollbar_visible());
    }

    #[test]
    fn always_scrollbar_hidden_without_a_viewport() {
        let sv = ScrollView::new(
            leaf(&["a"]),
            ScrollViewOptions {
                scrollbar: Some(ScrollViewScrollbar::Always),
                ..Default::default()
            },
        );
        // viewportHeight == 0 (scroll-view.ts:76-77).
        assert!(!sv.is_scrollbar_visible());
    }

    #[test]
    fn update_layout_pins_follow_end_to_max_scroll_top() {
        // scroll-view.ts:171-187: following views sit at maxScrollTop.
        let sv = ScrollView::new(
            leaf(&["1", "2", "3", "4", "5", "6"]),
            ScrollViewOptions {
                follow: Some(ScrollViewFollow::End),
                ..Default::default()
            },
        );
        sv.update_layout(6, 2, &mut || {});
        assert_eq!(sv.scroll_top(), 4);
        assert!(sv.is_following_end());
        // Content grows: still pinned to the new end.
        sv.update_layout(8, 2, &mut || {});
        assert_eq!(sv.scroll_top(), 6);
        assert!(sv.is_following_end());
        // Content shrinks below the viewport: clamped to 0.
        sv.update_layout(1, 2, &mut || {});
        assert_eq!(sv.scroll_top(), 0);
        assert!(sv.is_following_end());
    }

    #[test]
    fn update_layout_clamps_scroll_top_when_content_shrinks() {
        // scroll-view.ts:180: non-following views clamp into range.
        let sv = ScrollView::new(leaf(&["a"]), ScrollViewOptions::default());
        sv.update_layout(10, 4, &mut || {});
        sv.scroll_to(6, ScrollViewScrollToOptions::default());
        assert_eq!(sv.scroll_top(), 6);
        sv.update_layout(6, 4, &mut || {});
        assert_eq!(sv.scroll_top(), 2);
    }

    #[test]
    fn scroll_to_clamps_to_the_scrollable_range() {
        let sv = ScrollView::new(leaf(&["a"]), ScrollViewOptions::default());
        sv.update_layout(6, 2, &mut || {});
        sv.scroll_to(100, ScrollViewScrollToOptions::default());
        assert_eq!(sv.scroll_top(), 4);
        sv.scroll_to(-5, ScrollViewScrollToOptions::default());
        assert_eq!(sv.scroll_top(), 0);
    }

    #[test]
    fn scroll_to_manages_the_follow_end_latch() {
        let sv = ScrollView::new(
            leaf(&["a"]),
            ScrollViewOptions {
                follow: Some(ScrollViewFollow::End),
                ..Default::default()
            },
        );
        sv.update_layout(10, 4, &mut || {});
        assert!(sv.is_following_end());
        // Scroll away from the end: latch released.
        sv.scroll_to(3, ScrollViewScrollToOptions::default());
        assert_eq!(sv.scroll_top(), 3);
        assert!(!sv.is_following_end());
        // Scroll back to the end: latch re-engaged.
        sv.scroll_to(6, ScrollViewScrollToOptions::default());
        assert!(sv.is_following_end());
    }

    #[test]
    fn scroll_to_disable_follow_suppresses_following_at_end() {
        // scroll-view.ts:17-20 + 122-127: `disableFollow` keeps follow-end
        // off even when the target is the current content end, and
        // updateLayout honors the suppression until the view scrolls
        // below the end.
        let sv = ScrollView::new(
            leaf(&["a"]),
            ScrollViewOptions {
                follow: Some(ScrollViewFollow::End),
                ..Default::default()
            },
        );
        sv.update_layout(10, 4, &mut || {});
        assert!(sv.is_following_end());
        sv.scroll_to(
            6,
            ScrollViewScrollToOptions {
                disable_follow: true,
            },
        );
        assert_eq!(sv.scroll_top(), 6);
        assert!(!sv.is_following_end());
        // Same layout pass: still at the end, still suppressed.
        sv.update_layout(10, 4, &mut || {});
        assert_eq!(sv.scroll_top(), 6);
        assert!(!sv.is_following_end());
        // Content grows past the current position: suppression cleared,
        // but the view is no longer at the end, so following stays off.
        sv.update_layout(12, 4, &mut || {});
        assert_eq!(sv.scroll_top(), 6);
        assert!(!sv.is_following_end());
        // Reaching the (new) end re-engages the latch.
        sv.scroll_to(8, ScrollViewScrollToOptions::default());
        assert!(sv.is_following_end());
    }

    #[test]
    fn scroll_by_returns_unused_delta_for_overscroll_chaining() {
        // scroll-view.ts:147: `return requested - moved` feeds chained
        // overscroll ("chain" views hand the remainder to the parent).
        let sv = ScrollView::new(leaf(&["a"]), ScrollViewOptions::default());
        sv.update_layout(6, 2, &mut || {});
        // Only 4 of the requested 5 lines are available.
        assert_eq!(sv.scroll_by(5), 1);
        assert_eq!(sv.scroll_top(), 4);
        // Negative remainder when clamped at the top.
        assert_eq!(sv.scroll_by(-6), -2);
        assert_eq!(sv.scroll_top(), 0);
    }

    #[test]
    fn scroll_by_starts_from_the_end_while_following() {
        // scroll-view.ts:138-140: a following view scrolls from
        // maxScrollTop even if scrollTop already disagrees.
        let sv = ScrollView::new(
            leaf(&["a"]),
            ScrollViewOptions {
                follow: Some(ScrollViewFollow::End),
                ..Default::default()
            },
        );
        sv.update_layout(6, 2, &mut || {});
        assert_eq!(sv.scroll_top(), 4);
        assert_eq!(sv.scroll_by(-1), 0);
        assert_eq!(sv.scroll_top(), 3);
        assert!(!sv.is_following_end());
        // From mid-scroll, upward scroll_by lands exactly at the end and
        // re-engages the latch.
        assert_eq!(sv.scroll_by(3), 2);
        assert_eq!(sv.scroll_top(), 4);
        assert!(sv.is_following_end());
    }

    #[test]
    fn scroll_by_zero_is_a_no_op() {
        // scroll-view.ts:135-137.
        let mut sv = ScrollView::new(leaf(&["a"]), ScrollViewOptions::default());
        sv.update_layout(6, 2, &mut || {});
        sv.clear_render_requested();
        assert_eq!(sv.scroll_by(0), 0);
        assert_eq!(sv.scroll_top(), 0);
        assert!(!sv.render_requested());
    }

    #[test]
    fn scroll_to_start_and_end_manage_follow_state() {
        // scroll-view.ts:149-169.
        let sv = ScrollView::new(
            leaf(&["a"]),
            ScrollViewOptions {
                follow: Some(ScrollViewFollow::End),
                ..Default::default()
            },
        );
        sv.update_layout(10, 4, &mut || {});
        sv.scroll_to_start();
        assert_eq!(sv.scroll_top(), 0);
        // contentHeight (10) > viewportHeight (4): no longer following.
        assert!(!sv.is_following_end());
        sv.scroll_to_end();
        assert_eq!(sv.scroll_top(), 6);
        assert!(sv.is_following_end());
        // scrollToEnd with content that fits: following stays true at 0.
        sv.update_layout(3, 4, &mut || {});
        sv.scroll_to_start();
        assert_eq!(sv.scroll_top(), 0);
        assert!(sv.is_following_end());
    }

    #[test]
    fn auto_scrollbar_appears_on_activity_and_hides_after_the_delay() {
        // scroll-view.ts:74-80 + 126-149.
        let sv = ScrollView::new(
            leaf(&["a"]),
            ScrollViewOptions {
                scrollbar: Some(ScrollViewScrollbar::Auto),
                ..Default::default()
            },
        );
        sv.update_layout(10, 4, &mut || {});
        assert!(!sv.is_scrollbar_visible());
        sv.scroll_by(1);
        assert!(sv.is_scrollbar_visible());
        // [divergence] the TS waits out scrollbarHideDelayMs; the port
        // expires the modeled timer explicitly.
        sv.expire_scrollbar_hide_timer();
        assert!(!sv.is_scrollbar_visible());
        // Content that fits hides the transient scrollbar outright
        // (scroll-view.ts:186).
        sv.scroll_by(1);
        sv.update_layout(3, 4, &mut || {});
        assert!(!sv.is_scrollbar_visible());
        // No overflow → activity does not reveal it again.
        sv.scroll_to(2, ScrollViewScrollToOptions::default());
        assert!(!sv.is_scrollbar_visible());
    }

    #[test]
    fn set_scrollbar_hides_the_transient_scrollbar_when_leaving_auto() {
        // scroll-view.ts:88-92.
        let sv = ScrollView::new(
            leaf(&["a"]),
            ScrollViewOptions {
                scrollbar: Some(ScrollViewScrollbar::Auto),
                ..Default::default()
            },
        );
        sv.update_layout(10, 4, &mut || {});
        sv.scroll_by(1);
        assert!(sv.is_scrollbar_visible());
        sv.set_scrollbar(ScrollViewScrollbar::Always);
        // Transient state cleared; `always` is visible on its own.
        assert!(sv.is_scrollbar_visible());
        // Returning to auto: the transient scrollbar stays hidden until
        // new activity.
        sv.set_scrollbar(ScrollViewScrollbar::Auto);
        assert!(!sv.is_scrollbar_visible());
    }

    #[test]
    fn set_scrollbar_active_tracks_drag_state() {
        // scroll-view.ts:108-113 + markScrollbarActivity: while active
        // (dragging), the scrollbar stays visible with no hide timer.
        let sv = ScrollView::new(
            leaf(&["a"]),
            ScrollViewOptions {
                scrollbar: Some(ScrollViewScrollbar::Auto),
                ..Default::default()
            },
        );
        sv.update_layout(10, 4, &mut || {});
        sv.set_scrollbar_active(true);
        assert!(sv.is_scrollbar_active());
        assert!(sv.is_scrollbar_visible());
        // Expiring does nothing while active (no timer armed).
        sv.expire_scrollbar_hide_timer();
        assert!(sv.is_scrollbar_visible());
        // Releasing re-arms the timer.
        sv.set_scrollbar_active(false);
        assert!(!sv.is_scrollbar_active());
        assert!(sv.is_scrollbar_visible());
        sv.expire_scrollbar_hide_timer();
        assert!(!sv.is_scrollbar_visible());
    }

    #[test]
    fn mutation_no_ops_do_not_request_renders() {
        // scroll-view.ts:89 (same scrollbar), 122-126 (unchanged scroll
        // state), 160-166 (unchanged end state).
        let mut sv = ScrollView::new(leaf(&["a"]), ScrollViewOptions::default());
        sv.update_layout(6, 2, &mut || {});
        sv.clear_render_requested();
        sv.set_scrollbar(ScrollViewScrollbar::Hidden);
        sv.scroll_to(0, ScrollViewScrollToOptions::default());
        assert!(!sv.render_requested());
        // A real mutation requests one.
        sv.scroll_to_end();
        assert!(sv.render_requested());
        sv.clear_render_requested();
        // Unchanged end state and unchanged target: no request.
        sv.scroll_to_end();
        sv.scroll_to(4, ScrollViewScrollToOptions::default());
        assert!(!sv.render_requested());
        sv.scroll_by(-1);
        assert!(sv.render_requested());
    }

    #[test]
    fn request_render_callback_seam_is_invoked() {
        // [divergence] The TS requestRenderCallback arrives via
        // updateLayout and fires on every mutation; the port injects it
        // explicitly (a &mut dyn FnMut() cannot be retained).
        let mut sv = ScrollView::new(leaf(&["a"]), ScrollViewOptions::default());
        let calls = Rc::new(Cell::new(0));
        let counter = calls.clone();
        sv.set_request_render_callback(Some(Rc::new(move || {
            counter.set(counter.get() + 1);
        })));
        sv.update_layout(6, 2, &mut || {});
        sv.scroll_by(1);
        assert_eq!(calls.get(), 1);
        assert!(sv.render_requested());
        // The modeled hide-timer expiry also requests a render
        // (scroll-view.ts:136-139) — no timer is armed here, so no call.
        sv.expire_scrollbar_hide_timer();
        assert_eq!(calls.get(), 1);
        sv.clear_render_requested();
        assert!(!sv.render_requested());
    }

    #[test]
    fn layout_node_exposes_the_child_and_registered_state() {
        // scroll-view.ts:209-211: `{ type: "scroll", component: this.child,
        // state: this }` — built through layout::scroll_layout_node so the
        // solver can recover the extended state view.
        let child_ref: ComponentRef = leaf(&["a"]);
        let mut sv = ScrollView::new(
            child_ref.clone(),
            ScrollViewOptions {
                follow: Some(ScrollViewFollow::End),
                ..Default::default()
            },
        );
        let LayoutNode::Scroll(node) = sv.layout_node().unwrap() else {
            panic!("expected scroll layout node");
        };
        assert!(Rc::ptr_eq(&node.component, &child_ref));
        // State identity: a layout pass through the frozen node state
        // (follow-end pinned to maxScrollTop) is observed on the component.
        node.state.borrow_mut().update_layout(6, 2, &mut || {});
        assert_eq!(sv.scroll_top(), 4);
        sv.scroll_by(-1);
        assert_eq!(sv.scroll_top(), 3);
        // The state coerces to the solver's extended ScrollViewRef view.
        let extended: crate::layout::ScrollViewRef = sv.state();
        assert_eq!(extended.borrow().scrollbar(), ScrollViewScrollbar::Hidden);
    }

    #[test]
    fn handle_mouse_routes_to_the_child_like_container() {
        // TS ScrollView inherits Container.handleMouse (tui.ts:382-405).
        // The render override never populates the container's mouse
        // layout, so the first mouse event rebuilds it at event.width.
        let child = leaf(&["a", "b", "c"]);
        let child_ref: ComponentRef = child.clone();
        let sv = ScrollView::new(child_ref.clone(), ScrollViewOptions::default());
        let sv_ref: ComponentRef = Rc::new(RefCell::new(sv));
        // y == 0 lands on the child (local y == 0, where it captures).
        let result = sv_ref
            .borrow_mut()
            .handle_mouse(&sv_ref, &mouse(TuiMouseEventType::Press, 1, 0, 10, 3));
        match result {
            Some(ComponentMouseResult::Forwarded(dispatch)) => {
                assert!(dispatch.handled);
                assert!(dispatch.is_capture());
                assert!(Rc::ptr_eq(&dispatch.target.component, &child_ref));
            }
            _ => panic!("expected forwarded child dispatch"),
        }
        // Past the child height: no routing.
        assert!(sv_ref
            .borrow_mut()
            .handle_mouse(&sv_ref, &mouse(TuiMouseEventType::Press, 1, 3, 10, 3))
            .is_none());
        // Negative y is rejected before routing (tui.ts:384).
        assert!(sv_ref
            .borrow_mut()
            .handle_mouse(&sv_ref, &mouse(TuiMouseEventType::Press, 1, -1, 10, 3))
            .is_none());
    }

    #[test]
    fn invalidate_propagates_to_the_child() {
        let child = leaf(&["a"]);
        let mut sv = ScrollView::new(child.clone(), ScrollViewOptions::default());
        sv.invalidate();
        assert!(child.borrow().invalidated.get());
    }

    #[test]
    #[should_panic(expected = "ScrollView has exactly one child")]
    fn add_child_panics() {
        // scroll-view.ts:189-191.
        let mut sv = ScrollView::new(leaf(&["a"]), ScrollViewOptions::default());
        sv.add_child(leaf(&["b"]));
    }

    #[test]
    #[should_panic(expected = "ScrollView child cannot be removed")]
    fn remove_child_panics() {
        // scroll-view.ts:193-195.
        let child_ref: ComponentRef = leaf(&["a"]);
        let mut sv = ScrollView::new(child_ref.clone(), ScrollViewOptions::default());
        sv.remove_child(&child_ref);
    }

    #[test]
    #[should_panic(expected = "ScrollView child cannot be cleared")]
    fn clear_panics() {
        // scroll-view.ts:197-199.
        let mut sv = ScrollView::new(leaf(&["a"]), ScrollViewOptions::default());
        sv.clear();
    }
}
