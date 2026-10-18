//! Port of the `TuiBase` core of `packages/tui/src/tui.ts` (506-1517).
//!
//! Split across files in this port:
//! - `tui.rs` (this file): TUI types, [`TuiBase`] (focus, input listeners,
//!   render scheduling, terminal-input dispatch, terminal color queries,
//!   cursor-marker helpers), the [`TuiSurface`] trait + the
//!   [`impl_tui_surface!`] macro shared by both screens.
//! - `tui_overlay.rs`: the overlay stack, overlay focus-restore machinery,
//!   overlay layout resolution and compositing.
//! - `tui_main_screen.rs` / `tui_alt_screen.rs` (+ `alt_screen_*`): the
//!   concrete screens.
//!
//! ## TS inheritance → Rust composition
//!
//! TS `TuiBase extends Container`, and `TuiMainScreen`/`TuiAltScreen` extend
//! `TuiBase`. Rust: each screen owns a `base: TuiBase` (which owns a
//! [`Container`]); screen-specific overrides ("protected" hook methods in
//! TS) are [`TuiSurface`] hook methods; the shared `TUI` interface surface
//! is generated for each screen by the [`impl_tui_surface!`] macro.
//!
//! ## Timers / microtasks (see T5_PLAN §4-6)
//!
//! TS `process.nextTick`, `setTimeout` and promise resolution become
//! explicit pump state over the crate's virtual clock ([`crate::clock`]):
//! - `request_render` only flips flags and queues microtasks, exactly like
//!   TS; nothing renders synchronously.
//! - [`TuiSurface::run_pending_ticks`] drains the microtask queue (TS
//!   nextTick drain); the immediate-render microtask executes `do_render`.
//! - [`TuiSurface::advance_clock`] advances the clock, drains microtasks and
//!   fires due render timers.
//! - `queryTerminalColors`'s promise becomes delivery callbacks fired at
//!   reply time (or timeout, via the clock pump).

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::rc::{Rc, Weak};

use crate::clock::now_ms;
use crate::component::{
    Component, ComponentRef, Container, TuiMouseEvent, CURSOR_MARKER,
    FAKE_CURSOR_END, FAKE_CURSOR_START,
};
use crate::keys::{is_key_release, matches_key};
use crate::terminal::{ResizeHandler, Terminal, InputHandler};
use crate::terminal_colors::{
    parse_osc_color_response, parse_terminal_color_scheme_report, OscColorTarget, TerminalColorScheme,
    TerminalColors,
};

// =============================================================================
// Types (tui.ts:457-503)
// =============================================================================

/// `TuiMode` — "regular" | "fullscreen" (tui.ts:457).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiMode {
    Regular,
    Fullscreen,
}

/// `TuiStopOptions` (tui.ts:459-462).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TuiStopOptions {
    /// Leave renderer output in place for another TUI taking over the same
    /// terminal.
    pub preserve_screen: bool,
}

/// `TuiInputListenerResult` (tui.ts:144): `{ consume?, data? } | undefined`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TuiInputListenerResult {
    pub consume: Option<bool>,
    pub data: Option<String>,
}

/// Result of draining one queued microtask (port machinery — see module
/// docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextTickAction {
    /// A queued `process.nextTick` requested an immediate `doRender()`.
    DoRender,
    /// The microtask only adjusted scheduling state.
    None,
}

/// The kind of queued microtask (TS `process.nextTick` callbacks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NextTickKind {
    /// `requestRender()` → `nextTick(() => this.scheduleRender())`.
    ScheduleRender,
    /// `requestImmediateRender()` → `nextTick(() => { …doRender() })`.
    ImmediateRender,
}

// =============================================================================
// Overlay types (tui.ts:214-356) — state lives in TuiBase; the stack
// operations are implemented in tui_overlay.rs.
// =============================================================================

/// `OverlayAnchor` (tui.ts:214-227).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayAnchor {
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    TopCenter,
    BottomCenter,
    LeftCenter,
    RightCenter,
}

impl OverlayAnchor {
    /// TS string literal for each anchor.
    pub fn as_str(&self) -> &'static str {
        match self {
            OverlayAnchor::Center => "center",
            OverlayAnchor::TopLeft => "top-left",
            OverlayAnchor::TopRight => "top-right",
            OverlayAnchor::BottomLeft => "bottom-left",
            OverlayAnchor::BottomRight => "bottom-right",
            OverlayAnchor::TopCenter => "top-center",
            OverlayAnchor::BottomCenter => "bottom-center",
            OverlayAnchor::LeftCenter => "left-center",
            OverlayAnchor::RightCenter => "right-center",
        }
    }
}

/// `OverlayMargin` (tui.ts:228-233).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OverlayMargin {
    pub top: Option<i64>,
    pub right: Option<i64>,
    pub bottom: Option<i64>,
    pub left: Option<i64>,
}

/// `SizeValue` — `number | \`${number}%\`` (tui.ts:236). Invalid percent
/// strings are kept verbatim; [`parse_size_value`] rejects them like the TS
/// regex does.
#[derive(Debug, Clone, PartialEq)]
pub enum SizeValue {
    Number(f64),
    Percent(String),
}

/// `OverlayOptions` (tui.ts:254-291).
#[derive(Clone, Default)]
pub struct OverlayOptions {
    /// Width in columns, or percentage of terminal width.
    pub width: Option<SizeValue>,
    /// Minimum width in columns.
    pub min_width: Option<i64>,
    /// Maximum height in rows, or percentage of terminal height.
    pub max_height: Option<SizeValue>,
    /// Anchor point for positioning (default: center).
    pub anchor: Option<OverlayAnchor>,
    /// Horizontal offset from anchor position (positive = right).
    pub offset_x: Option<i64>,
    /// Vertical offset from anchor position (positive = down).
    pub offset_y: Option<i64>,
    /// Row position: absolute number, or percentage.
    pub row: Option<SizeValue>,
    /// Column position: absolute number, or percentage.
    pub col: Option<SizeValue>,
    /// Margin from terminal edges (number applies to all sides).
    pub margin: Option<OverlayMarginOrNumber>,
    /// Overlay is only rendered while this returns true.
    pub visible: Option<Rc<dyn Fn(usize, usize) -> bool>>,
    /// If true, don't capture keyboard focus when shown.
    pub non_capturing: bool,
}

impl std::fmt::Debug for OverlayOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayOptions")
            .field("width", &self.width)
            .field("min_width", &self.min_width)
            .field("max_height", &self.max_height)
            .field("anchor", &self.anchor)
            .field("offset_x", &self.offset_x)
            .field("offset_y", &self.offset_y)
            .field("row", &self.row)
            .field("col", &self.col)
            .field("margin", &self.margin)
            .field("has_visible_cb", &self.visible.is_some())
            .field("non_capturing", &self.non_capturing)
            .finish()
    }
}

/// `margin?: OverlayMargin | number`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayMarginOrNumber {
    Margin(OverlayMargin),
    Number(i64),
}

/// `OverlayUnfocusOptions` (tui.ts:293-296).
#[derive(Clone)]
pub struct OverlayUnfocusOptions {
    /// Explicit target to focus after releasing this overlay.
    pub target: Option<ComponentRef>,
}

/// `OverlayBounds` (tui.ts:299-304).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayBounds {
    pub row: i64,
    pub col: i64,
    pub width: i64,
    pub height: i64,
}

/// `OverlayHandle` (tui.ts:309-324) — dispatches into the owning screen.
/// TS closes over the stack entry; the Rust handle identifies the entry by
/// its `entry_id` and upgrades a weak screen reference.
#[derive(Clone)]
pub struct OverlayHandle {
    surface: Weak<RefCell<dyn TuiSurface>>,
    entry_id: u64,
}

impl OverlayHandle {
    pub(crate) fn new(surface: Weak<RefCell<dyn TuiSurface>>, entry_id: u64) -> Self {
        OverlayHandle { surface, entry_id }
    }

    fn with_surface<R>(&self, f: impl FnOnce(&mut dyn TuiSurface) -> R) -> Option<R> {
        let surface = self.surface.upgrade()?;
        let mut borrowed = surface.borrow_mut();
        Some(f(&mut *borrowed))
    }

    /// Permanently remove the overlay (cannot be shown again).
    pub fn hide(&self) {
        let _ = self.with_surface(|s| s.hide_overlay_entry(self.entry_id));
    }

    /// Temporarily hide or show the overlay.
    pub fn set_hidden(&self, hidden: bool) {
        let _ = self.with_surface(|s| s.set_overlay_hidden(self.entry_id, hidden));
    }

    /// Check if overlay is temporarily hidden.
    pub fn is_hidden(&self) -> bool {
        self.with_surface(|s| s.is_overlay_hidden(self.entry_id)).unwrap_or(true)
    }

    /// Focus this overlay and bring it to the visual front.
    pub fn focus(&self) {
        let _ = self.with_surface(|s| s.focus_overlay(self.entry_id));
    }

    /// Release focus to the next visible capturing overlay or previous
    /// target, or to an explicit target when provided.
    pub fn unfocus(&self, options: Option<OverlayUnfocusOptions>) {
        let _ = self.with_surface(|s| s.unfocus_overlay(self.entry_id, options));
    }

    /// Check if this overlay currently has focus.
    pub fn is_focused(&self) -> bool {
        self.with_surface(|s| s.is_overlay_focused(self.entry_id)).unwrap_or(false)
    }

    /// The most recent rendered bounds for a visible overlay.
    pub fn get_bounds(&self) -> Option<OverlayBounds> {
        self.with_surface(|s| s.overlay_bounds(self.entry_id)).flatten()
    }
}

/// `OverlayStackEntry` (tui.ts:326-333) + port `entry_id` identity.
#[derive(Clone)]
pub(crate) struct OverlayStackEntry {
    pub(crate) entry_id: u64,
    pub(crate) component: ComponentRef,
    pub(crate) options: Option<OverlayOptions>,
    pub(crate) pre_focus: Option<ComponentRef>,
    pub(crate) hidden: bool,
    pub(crate) focus_order: u64,
    pub(crate) bounds: Option<OverlayBounds>,
}

/// `RenderedOverlayLayout` (tui.ts:335-341). Holds the entry's component
/// (the TS layout holds the entry object reference, keeping it alive even
/// if the stack mutates before the next render).
#[derive(Clone)]
pub(crate) struct RenderedOverlayLayout {
    pub(crate) entry_id: u64,
    pub(crate) component: ComponentRef,
    pub(crate) row: i64,
    pub(crate) col: i64,
    pub(crate) width: i64,
    pub(crate) height: i64,
}

/// `OverlayFocusRestorePolicy` (tui.ts:353).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverlayFocusRestorePolicy {
    Clear,
    Preserve,
}

/// `OverlayFocusRestoreState` (tui.ts:343-352), one enum instead of the TS
/// discriminated union.
#[derive(Clone)]
pub(crate) enum OverlayFocusRestoreState {
    Inactive,
    /// `{ status: "eligible"; overlay }`.
    Eligible { entry_id: u64 },
    /// `{ status: "blocked"; overlay; blockedBy; resume }`.
    Blocked {
        entry_id: u64,
        blocked_by: Option<ComponentRef>,
        /// `{ status: "restore-overlay" } | { status: "focus-target"; target }`.
        resume: BlockedResume,
    },
}

/// `OverlayBlockedFocusResume` (tui.ts:343).
#[derive(Clone)]
pub(crate) enum BlockedResume {
    RestoreOverlay,
    FocusTarget(Option<ComponentRef>),
}

// =============================================================================
// Terminal color query state (tui.ts:146-170)
// =============================================================================

pub(crate) const TERMINAL_PALETTE_SIZE: usize = 16;
/// OSC 10 and 11 plus OSC 4 for every palette color.
const TERMINAL_COLOR_REPLY_COUNT: usize = 2 + TERMINAL_PALETTE_SIZE;
/// Default colors, palette colors 0-15, and a trailing DA1 request.
pub(crate) const TERMINAL_COLOR_QUERY: &str = "\x1b]10;?\x07\x1b]11;?\x07\x1b]4;0;?\x07\x1b]4;1;?\x07\x1b]4;2;?\x07\x1b]4;3;?\x07\x1b]4;4;?\x07\x1b]4;5;?\x07\x1b]4;6;?\x07\x1b]4;7;?\x07\x1b]4;8;?\x07\x1b]4;9;?\x07\x1b]4;10;?\x07\x1b]4;11;?\x07\x1b]4;12;?\x07\x1b]4;13;?\x07\x1b]4;14;?\x07\x1b]4;15;?\x07\x1b[c";

/// Delivery callback for [`TuiSurface::query_terminal_colors`] (the TS
/// promise resolve / `onLateReply`).
pub type ColorQueryCallback = Box<dyn FnMut(TerminalColors)>;

pub(crate) struct PendingTerminalColorQuery {
    pub(crate) foreground: Option<crate::terminal_colors::RgbColor>,
    pub(crate) background: Option<crate::terminal_colors::RgbColor>,
    pub(crate) palette: Vec<Option<crate::terminal_colors::RgbColor>>,
    /// Targets that already replied, so duplicates do not count twice.
    pub(crate) replied: HashSet<String>,
    /// The promise's resolve until the timeout, then `on_late_reply`; unset
    /// once completed. Later replies are ignored.
    pub(crate) deliver: Option<ColorQueryCallback>,
    /// `onLateReply` — takes over `deliver` when the timeout fires.
    pub(crate) deliver_on_timeout: Option<ColorQueryCallback>,
    /// Virtual-clock deadline of the timeout (None once fired/completed).
    pub(crate) timer: Option<f64>,
}

// =============================================================================
// TuiSurface — the TS `TUI` interface + screen hooks
// =============================================================================

/// The shared `TUI` interface (tui.ts:464-493) plus the TS "protected" hook
/// surface, implemented for each concrete screen by [`impl_tui_surface!`].
///
/// Object-safe: [`OverlayHandle`] dispatches through
/// `Weak<RefCell<dyn TuiSurface>>`, and screens coerce from their concrete
/// `Rc<RefCell<Screen>>`.
pub trait TuiSurface: Component {
    // ----- screen hooks (TS protected abstract/overridable) -----

    /// `abstract doRender()` (tui.ts:583).
    fn do_render(&mut self);
    /// `protected resetRenderState()` (tui.ts:585).
    fn reset_render_state_hook(&mut self);
    /// `protected beforeTerminalStart()` (tui.ts:587).
    fn before_terminal_start(&mut self);
    /// `protected afterTerminalStart()` (tui.ts:589).
    fn after_terminal_start(&mut self);
    /// `protected beforeTerminalStop(options)` (tui.ts:591).
    fn before_terminal_stop(&mut self, options: &TuiStopOptions);
    /// `protected afterTerminalStop(options)` (tui.ts:593).
    fn after_terminal_stop(&mut self, options: &TuiStopOptions);
    /// `protected getMountedRoots()` (tui.ts:717) — default `children`.
    fn mounted_roots(&self) -> Vec<ComponentRef>;
    /// `abstract readonly mode` (tui.ts:507).
    fn mode(&self) -> TuiMode;
    /// The alt screen's implicit input listener (its constructor calls
    /// `addInputListener((data) => this.handleViewportInput(data))`);
    /// main screens never install one.
    fn handle_viewport_input(&mut self, _data: &str) -> Option<TuiInputListenerResult> {
        None
    }
    /// `render(width)` — TuiBase inherits `Container.render`; the alt screen
    /// overrides it for `layoutRoot`.
    fn render_children(&mut self, width: usize) -> Vec<String>;
    /// `Container.handleMouse` — TuiBase inherits it; the alt screen's
    /// implicit document forwards to it.
    fn handle_mouse_base(
        &mut self,
        self_ref: &ComponentRef,
        event: &TuiMouseEvent,
    ) -> Option<crate::component::ComponentMouseResult>;

    // ----- shared surface (macro-generated bodies) -----

    fn base(&self) -> &TuiBase;
    fn base_mut(&mut self) -> &mut TuiBase;
    fn tui_id(&self) -> usize;

    fn request_render(&mut self, force: bool);
    fn render_now(&mut self, force: bool);
    /// Drain queued microtasks (TS `process.nextTick`); may execute
    /// `do_render` for immediate renders.
    fn run_pending_ticks(&mut self);
    /// Advance the virtual clock `ms`, drain microtasks, fire due render
    /// timers and terminal-color-query timeouts.
    fn advance_clock(&mut self, ms: f64);
    fn start(&mut self);
    fn stop(&mut self, options: &TuiStopOptions);
    fn handle_terminal_input(&mut self, data: &str);

    fn get_focused_component(&self) -> Option<ComponentRef>;
    fn set_focus(&mut self, component: Option<ComponentRef>);
    fn show_overlay(&mut self, component: ComponentRef, options: OverlayOptions) -> OverlayHandle;
    fn hide_overlay(&mut self);
    fn hide_overlay_entry(&mut self, entry_id: u64);
    fn set_overlay_hidden(&mut self, entry_id: u64, hidden: bool);
    fn is_overlay_hidden(&self, entry_id: u64) -> bool;
    fn focus_overlay(&mut self, entry_id: u64);
    fn unfocus_overlay(&mut self, entry_id: u64, options: Option<OverlayUnfocusOptions>);
    fn is_overlay_focused(&self, entry_id: u64) -> bool;
    fn overlay_bounds(&self, entry_id: u64) -> Option<OverlayBounds>;
    fn has_overlay(&self) -> bool;

    fn get_show_hardware_cursor(&self) -> bool;
    fn set_show_hardware_cursor(&mut self, enabled: bool);
    fn get_clear_on_shrink(&self) -> bool;
    fn set_clear_on_shrink(&mut self, enabled: bool);
    fn full_redraws(&self) -> u64;

    fn add_input_listener(&mut self, listener: Box<dyn FnMut(&str) -> Option<TuiInputListenerResult>>) -> usize;
    fn remove_input_listener(&mut self, listener_id: usize);
    fn on_terminal_color_scheme_change(
        &mut self,
        listener: Box<dyn FnMut(TerminalColorScheme)>,
    ) -> usize;
    fn set_terminal_color_scheme_notifications(&mut self, enabled: bool);
    /// `queryTerminalColors({timeoutMs, onLateReply})` — the TS promise
    /// resolve becomes `on_result`; delivery happens at DA1/full-palette
    /// reply, or at the timeout via the clock pump (then `on_late_reply`
    /// takes over).
    fn query_terminal_colors(
        &mut self,
        timeout_ms: u64,
        on_result: ColorQueryCallback,
        on_late_reply: Option<ColorQueryCallback>,
    );
    fn set_on_debug(&mut self, callback: Option<Box<dyn FnMut()>>);
}

// =============================================================================
// TuiBase
// =============================================================================

/// `MIN_RENDER_INTERVAL_MS` (tui.ts:518).
const MIN_RENDER_INTERVAL_MS: f64 = 16.0;

/// Input-listener entry: a plain closure, or the screen's own viewport
/// listener (installed by the alt-screen constructor before the `Rc`
/// exists — dispatched through the weak self reference).
pub(crate) enum InputListenerEntry {
    Callback(Box<dyn FnMut(&str) -> Option<TuiInputListenerResult>>),
    /// The alt screen's `(data) => this.handleViewportInput(data)`.
    SelfViewport,
}

/// TS `TuiBase extends Container` — the shared renderer state (tui.ts:506).
pub struct TuiBase {
    pub(crate) terminal: Rc<RefCell<dyn Terminal>>,
    /// The `Container` state TS inherits (`children`, `mouseLayout`).
    pub(crate) container: Container,
    /// Weak self reference (`Rc<RefCell<dyn TuiSurface>>`), installed by
    /// [`TuiBase::install_self`] after the screen's `Rc` is created.
    pub(crate) self_weak: RefCell<Option<Weak<RefCell<dyn TuiSurface>>>>,
    /// Per-instance identity (monotonic per thread).
    pub(crate) tui_id: usize,
    /// `getMountedRoots()` (tui.ts:717-719): default is `container.children`;
    /// the alt screen overrides to `layoutRoot` and re-syncs this field on
    /// every layout change.
    pub(crate) mounted_roots: Vec<ComponentRef>,
    pub(crate) focused_component: Option<ComponentRef>,
    pub(crate) input_listeners: Vec<(usize, InputListenerEntry)>,
    next_input_listener_id: usize,
    /// Global callback for the debug key (Shift+Ctrl+D).
    pub(crate) on_debug: Option<Box<dyn FnMut()>>,
    render_requested: bool,
    /// Microtask queue (TS `process.nextTick`), FIFO.
    next_tick_queue: VecDeque<NextTickKind>,
    immediate_render_scheduled: bool,
    /// `renderTimer` — setTimeout deadline on the virtual clock.
    render_timer: Option<f64>,
    last_render_at: f64,
    show_hardware_cursor: bool,
    clear_on_shrink: bool,
    pub(crate) full_redraw_count: u64,
    pub(crate) stopped: bool,
    /// Color queries waiting for their DA1 reply, oldest first.
    pub(crate) pending_color_queries: Vec<PendingTerminalColorQuery>,
    color_scheme_listeners: Vec<(usize, Box<dyn FnMut(TerminalColorScheme)>)>,
    next_color_scheme_listener_id: usize,
    pub(crate) color_scheme_notifications_enabled: bool,
    // Overlay stack (tui.ts:534-541). Operations in tui_overlay.rs.
    pub(crate) focus_order_counter: u64,
    pub(crate) next_overlay_entry_id: u64,
    pub(crate) overlay_stack: Vec<OverlayStackEntry>,
    pub(crate) rendered_overlay_layouts: Vec<RenderedOverlayLayout>,
    pub(crate) overlay_focus_restore: OverlayFocusRestoreState,
}

thread_local! {
    static NEXT_TUI_ID: std::cell::Cell<usize> = const { std::cell::Cell::new(1) };
}

impl TuiBase {
    /// `constructor(terminal, showHardwareCursor?, logDirectory?)` (tui.ts:543).
    /// `logDirectory` is env-gated debug output — see T5_PLAN divergences.
    pub(crate) fn new(terminal: Rc<RefCell<dyn Terminal>>, show_hardware_cursor: Option<bool>) -> Self {
        let tui_id = NEXT_TUI_ID.with(|c| {
            let id = c.get();
            c.set(id + 1);
            id
        });
        TuiBase {
            terminal,
            container: Container::new(),
            self_weak: RefCell::new(None),
            tui_id,
            mounted_roots: Vec::new(),
            focused_component: None,
            input_listeners: Vec::new(),
            next_input_listener_id: 0,
            on_debug: None,
            render_requested: false,
            next_tick_queue: VecDeque::new(),
            immediate_render_scheduled: false,
            render_timer: None,
            last_render_at: 0.0,
            show_hardware_cursor: show_hardware_cursor.unwrap_or(false),
            clear_on_shrink: false,
            full_redraw_count: 0,
            stopped: false,
            pending_color_queries: Vec::new(),
            color_scheme_listeners: Vec::new(),
            next_color_scheme_listener_id: 0,
            color_scheme_notifications_enabled: false,
            focus_order_counter: 0,
            next_overlay_entry_id: 0,
            overlay_stack: Vec::new(),
            rendered_overlay_layouts: Vec::new(),
            overlay_focus_restore: OverlayFocusRestoreState::Inactive,
        }
    }

    /// Record the weak self reference once the screen's `Rc` exists.
    pub(crate) fn install_self(&mut self, surface: Weak<RefCell<dyn TuiSurface>>) {
        *self.self_weak.borrow_mut() = Some(surface);
    }

    pub(crate) fn self_weak(&self) -> Option<Weak<RefCell<dyn TuiSurface>>> {
        self.self_weak.borrow().clone()
    }

    // ----- simple getters/setters (tui.ts:596-637) -----

    pub(crate) fn get_show_hardware_cursor(&self) -> bool {
        self.show_hardware_cursor
    }

    pub(crate) fn set_show_hardware_cursor(&mut self, enabled: bool) {
        if self.show_hardware_cursor == enabled {
            return;
        }
        self.show_hardware_cursor = enabled;
        if !enabled {
            self.hide_terminal_cursor();
        }
        self.request_render_state(false);
    }

    pub(crate) fn get_clear_on_shrink(&self) -> bool {
        self.clear_on_shrink
    }

    pub(crate) fn set_clear_on_shrink(&mut self, enabled: bool) {
        self.clear_on_shrink = enabled;
    }

    pub(crate) fn get_focused_component(&self) -> Option<ComponentRef> {
        self.focused_component.clone()
    }

    /// `hideTerminalCursor()` (tui.ts:845) — after stop() the shell owns the
    /// cursor and it must stay visible.
    pub(crate) fn hide_terminal_cursor(&mut self) {
        if !self.stopped {
            self.terminal.borrow_mut().hide_cursor();
        }
    }

    // ----- input listeners (tui.ts:940-955) -----

    pub(crate) fn add_input_listener(
        &mut self,
        listener: Box<dyn FnMut(&str) -> Option<TuiInputListenerResult>>,
    ) -> usize {
        let id = self.next_input_listener_id;
        self.next_input_listener_id += 1;
        self.input_listeners.push((id, InputListenerEntry::Callback(listener)));
        id
    }

    /// The alt-screen constructor's self-listener — installed before the
    /// `Rc` exists, so it is dispatched through the screen's
    /// `handle_viewport_input` hook (see the macro's listener loop).
    pub(crate) fn add_viewport_input_listener(&mut self) -> usize {
        let id = self.next_input_listener_id;
        self.next_input_listener_id += 1;
        self.input_listeners.push((id, InputListenerEntry::SelfViewport));
        id
    }

    pub(crate) fn remove_input_listener(&mut self, listener_id: usize) {
        self.input_listeners.retain(|(id, _)| *id != listener_id);
    }

    // ----- terminal color scheme listeners (tui.ts:957-972) -----

    pub(crate) fn on_terminal_color_scheme_change(
        &mut self,
        listener: Box<dyn FnMut(TerminalColorScheme)>,
    ) -> usize {
        let id = self.next_color_scheme_listener_id;
        self.next_color_scheme_listener_id += 1;
        self.color_scheme_listeners.push((id, listener));
        id
    }

    pub(crate) fn set_terminal_color_scheme_notifications(&mut self, enabled: bool) {
        if self.color_scheme_notifications_enabled == enabled {
            return;
        }
        self.color_scheme_notifications_enabled = enabled;
        if !self.stopped {
            self.terminal
                .borrow_mut()
                .write(if enabled { "\x1b[?2031h" } else { "\x1b[?2031l" });
        }
    }

    // ----- lifecycle (tui.ts:924-938, 974-997) -----

    /// `queryCellSize()` (tui.ts:971-983) — only when the terminal supports
    /// images (cell size is only used for image rendering).
    pub(crate) fn query_cell_size(&mut self) {
        if !crate::terminal_image::get_capabilities().images.is_some() {
            return;
        }
        // Query terminal for cell size in pixels: CSI 16 t
        self.terminal.borrow_mut().write("\x1b[16t");
    }

    /// The terminal-facing part of `start()` (tui.ts:924-937): registers
    /// input/resize handlers that dispatch into the screen, hides the
    /// cursor, and requests the first render.
    pub(crate) fn start_terminal(&mut self) {
        self.stopped = false;
        let weak_input = self.self_weak();
        let on_input: InputHandler = Rc::new(RefCell::new(move |data: String| {
            if let Some(surface) = weak_input.as_ref().and_then(|w| w.upgrade()) {
                {
                    let mut s = surface.borrow_mut();
                    s.handle_terminal_input(&data);
                }
                // The immediate-render microtask queued by input handling
                // must run before the next input event (TS: process.nextTick
                // runs after the current event).
                surface.borrow_mut().run_pending_ticks();
            }
        }));
        let weak_resize = self.self_weak();
        let on_resize: ResizeHandler = Rc::new(RefCell::new(move || {
            if let Some(surface) = weak_resize.as_ref().and_then(|w| w.upgrade()) {
                {
                    let mut s = surface.borrow_mut();
                    s.request_render(false);
                }
                surface.borrow_mut().run_pending_ticks();
            }
        }));
        self.terminal.borrow_mut().start(on_input, on_resize);
        self.terminal.borrow_mut().hide_cursor();
        if self.color_scheme_notifications_enabled {
            self.terminal.borrow_mut().write("\x1b[?2031h");
        }
        self.query_cell_size();
        self.request_render_state(false);
    }

    /// The base part of `stop()` (tui.ts:985-997) — hook calls happen at
    /// the screen level.
    pub(crate) fn stop_terminal(&mut self) {
        // The terminal-facing tail of `stop()` (tui.ts:985-997); the
        // screen-level macro method calls the hooks around this.
        self.stopped = true;
        self.cancel_render_timer();
        if self.color_scheme_notifications_enabled {
            self.terminal.borrow_mut().write("\x1b[?2031l");
        }
        self.terminal.borrow_mut().show_cursor();
        self.terminal.borrow_mut().stop();
    }

    // ----- render scheduling (tui.ts:999-1053) -----

    /// State-only `requestRender(force)` (tui.ts:1016): sets flags and
    /// queues microtasks; `force` resetRenderState happens at the screen
    /// level before this call.
    pub(crate) fn request_render_state(&mut self, force: bool) {
        if force {
            // requestImmediateRender() minus the hook
            self.cancel_render_timer();
            self.render_requested = true;
            if !self.immediate_render_scheduled {
                self.immediate_render_scheduled = true;
                self.next_tick_queue.push_back(NextTickKind::ImmediateRender);
            }
            return;
        }
        if self.render_requested {
            return;
        }
        self.render_requested = true;
        self.next_tick_queue.push_back(NextTickKind::ScheduleRender);
    }

    /// State-only `renderNow(force)` prologue (tui.ts:999-1005).
    pub(crate) fn render_now_begin(&mut self) {
        self.render_requested = false;
        self.cancel_render_timer();
        self.last_render_at = now_ms();
    }

    pub(crate) fn cancel_render_timer(&mut self) {
        self.render_timer = None;
    }

    /// `scheduleRender()` (tui.ts:1035-1053) — state only; the timer fires
    /// in `advance_clock`.
    fn schedule_render_state(&mut self) {
        if self.stopped || self.render_timer.is_some() || !self.render_requested {
            return;
        }
        let elapsed = now_ms() - self.last_render_at;
        let delay = (MIN_RENDER_INTERVAL_MS - elapsed).max(0.0);
        self.render_timer = Some(now_ms() + delay);
    }

    /// Drain one queued microtask (TS `process.nextTick` FIFO). Returns
    /// whether an immediate `do_render` must execute.
    pub(crate) fn drain_one_next_tick(&mut self) -> Option<NextTickAction> {
        let kind = self.next_tick_queue.pop_front()?;
        match kind {
            NextTickKind::ScheduleRender => {
                self.schedule_render_state();
                Some(NextTickAction::None)
            }
            NextTickKind::ImmediateRender => {
                self.immediate_render_scheduled = false;
                if self.stopped || !self.render_requested {
                    return Some(NextTickAction::None);
                }
                // A previously queued scheduleRender() can create a timer
                // before this callback runs. User input must preempt that
                // throttled frame.
                self.cancel_render_timer();
                self.render_requested = false;
                self.last_render_at = now_ms();
                Some(NextTickAction::DoRender)
            }
        }
    }

    pub(crate) fn has_pending_ticks(&self) -> bool {
        !self.next_tick_queue.is_empty()
    }

    /// Whether the render timer is due at the current clock (consumes the
    /// timer and the `renderRequested` flag exactly like the TS callback).
    pub(crate) fn fire_due_render_timer(&mut self) -> bool {
        let Some(deadline) = self.render_timer else { return false };
        if now_ms() < deadline {
            return false;
        }
        self.render_timer = None;
        if self.stopped || !self.render_requested {
            return false;
        }
        self.render_requested = false;
        self.last_render_at = now_ms();
        true
    }

    /// The tail of the TS timer callback: `if (this.renderRequested)
    /// this.scheduleRender()` (direct call, not a microtask).
    pub(crate) fn rearm_after_timer_render(&mut self) {
        if self.render_requested {
            self.schedule_render_state();
        }
    }

    // ----- terminal input (tui.ts:1055-1131) -----

    /// The color-response prologue of `handleTerminalInput` (tui.ts:1057-
    /// 1062): terminal color responses and color-scheme reports are
    /// consumed before listeners run.
    pub(crate) fn consume_color_input(&mut self, data: &str) -> bool {
        if self.consume_terminal_color_response(data) {
            return true;
        }
        self.consume_terminal_color_scheme_report(data)
    }

    pub(crate) fn has_input_listeners(&self) -> bool {
        !self.input_listeners.is_empty()
    }

    /// The listener-free tail of `handleTerminalInput` (tui.ts:1097-1131).
    pub(crate) fn forward_input_after_listeners(&mut self, data: &str) {
        // Consume terminal cell size responses without blocking unrelated
        // input.
        if self.consume_cell_size_response(data) {
            return;
        }

        // Global debug key handler (Shift+Ctrl+D)
        if matches_key(data, "shift+ctrl+d") {
            if let Some(cb) = self.on_debug.as_mut() {
                cb();
            }
            return;
        }

        // If focused component is an overlay, verify it's still visible.
        let focused = self.focused_component.clone();
        if let Some(focused) = &focused {
            if let Some(entry_id) = self
                .overlay_stack
                .iter()
                .find(|e| Rc::ptr_eq(&e.component, focused))
                .map(|e| e.entry_id)
            {
                if !self.is_overlay_visible(entry_id) {
                    // Focused overlay is no longer visible; redirect.
                    let top_visible = self.get_topmost_visible_overlay();
                    match top_visible {
                        Some(top) => {
                            let component = self.overlay_entry_component(top);
                            self.set_focus_internal(Some(component), OverlayFocusRestorePolicy::Clear);
                        }
                        None => {
                            let pre_focus = self.overlay_entry_pre_focus(entry_id);
                            self.set_focus_internal(pre_focus, OverlayFocusRestorePolicy::Preserve);
                        }
                    }
                }
            }
        }

        let focused = self.focused_component.clone();
        let focus_is_overlay = focused
            .as_ref()
            .map(|f| self.overlay_stack.iter().any(|e| Rc::ptr_eq(&e.component, f)))
            .unwrap_or(false);
        if !focus_is_overlay {
            match self.get_visible_overlay_focus_restore() {
                OverlayFocusRestoreState::Eligible { entry_id } => {
                    let component = self.overlay_entry_component(entry_id);
                    self.set_focus_internal(Some(component), OverlayFocusRestorePolicy::Clear);
                }
                OverlayFocusRestoreState::Blocked { entry_id, blocked_by, resume } => {
                    // `restoreState.blockedBy !== this.focusedComponent`
                    let blocked_is_focused = match (&focused, &blocked_by) {
                        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                        (None, None) => true,
                        _ => false,
                    };
                    if !blocked_is_focused {
                        match resume {
                            BlockedResume::RestoreOverlay => {
                                let component = self.overlay_entry_component(entry_id);
                                self.set_focus_internal(
                                    Some(component),
                                    OverlayFocusRestorePolicy::Clear,
                                );
                            }
                            BlockedResume::FocusTarget(target) => {
                                self.clear_overlay_focus_restore();
                                self.set_focus_internal(
                                    target,
                                    OverlayFocusRestorePolicy::Clear,
                                );
                            }
                        }
                    }
                }
                OverlayFocusRestoreState::Inactive => {}
            }
        }

        // Pass input to focused component (including Ctrl+C)
        if let Some(focused) = &self.focused_component {
            let handles = focused.borrow().handles_input();
            // Filter out key release events unless component opts in
            let wants_release = focused.borrow().wants_key_release();
            if is_key_release(data) && !wants_release {
                return;
            }
            if handles_input_gate(handles) {
                focused.borrow_mut().handle_input(data);
                // Keyboard input is latency-sensitive — immediate render.
                self.request_render_state(true);
            }
        }
    }

    /// `consumeTerminalColorResponse(data)` (tui.ts:1133-1165).
    fn consume_terminal_color_response(&mut self, data: &str) -> bool {
        let Some(query) = self.pending_color_queries.first_mut() else {
            return false;
        };
        // DEVICE_ATTRIBUTES_RESPONSE_PATTERN: /^\x1b\[\?[\d;]*c$/
        if is_device_attributes_response(data) {
            let mut query = self.pending_color_queries.remove(0);
            self.complete_terminal_color_query(&mut query);
            return true;
        }

        let Some((target, rgb)) = parse_osc_color_response(data) else {
            return false;
        };
        let key = match target {
            OscColorTarget::Foreground => "foreground".to_string(),
            OscColorTarget::Background => "background".to_string(),
            OscColorTarget::Index(i) => i.to_string(),
        };
        if query.deliver.is_none() || query.replied.contains(&key) {
            return true;
        }
        query.replied.insert(key);
        match target {
            OscColorTarget::Foreground => query.foreground = rgb,
            OscColorTarget::Background => query.background = rgb,
            OscColorTarget::Index(i) => {
                if (i as usize) < TERMINAL_PALETTE_SIZE {
                    query.palette[i as usize] = rgb;
                }
            }
        }
        if query.replied.len() == TERMINAL_COLOR_REPLY_COUNT {
            let mut query = self.pending_color_queries.remove(0);
            self.complete_terminal_color_query(&mut query);
        }
        true
    }

    /// `terminalColorQueryResult(query)` (tui.ts:1167-1170).
    fn terminal_color_query_result(query: &PendingTerminalColorQuery) -> TerminalColors {
        let palette = if query.palette.iter().all(|c| c.is_some()) {
            Some(
                query
                    .palette
                    .iter()
                    .map(|c| c.expect("all present"))
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        };
        TerminalColors { foreground: query.foreground, background: query.background, palette }
    }

    /// `completeTerminalColorQuery(query)` (tui.ts:1172-1177). The query is
    /// removed from the pending list by the caller when appropriate (DA1).
    fn complete_terminal_color_query(&mut self, query: &mut PendingTerminalColorQuery) {
        let deliver = query.deliver.take();
        query.timer = None;
        if let Some(mut deliver) = deliver {
            deliver(Self::terminal_color_query_result(query));
        }
    }

    /// `consumeTerminalColorSchemeReport(data)` (tui.ts:1179-1189).
    fn consume_terminal_color_scheme_report(&mut self, data: &str) -> bool {
        let Some(scheme) = parse_terminal_color_scheme_report(data) else {
            return false;
        };
        for (_, listener) in self.color_scheme_listeners.iter_mut() {
            listener(scheme);
        }
        true
    }

    /// `consumeCellSizeResponse(data)` (tui.ts:1191-1213) —
    /// `/^\x1b\[6;(\d+);(\d+)t$/`.
    fn consume_cell_size_response(&mut self, data: &str) -> bool {
        let Some((height_px, width_px)) = parse_cell_size_response(data) else {
            return false;
        };
        if height_px <= 0 || width_px <= 0 {
            return true;
        }
        crate::terminal_image::set_cell_dimensions(width_px as u32, height_px as u32);
        // Invalidate all components so images re-render with correct
        // dimensions. Runs through the screen hook surface.
        if let Some(surface) = self.self_weak.borrow().as_ref().and_then(|w| w.upgrade()) {
            {
                let mut s = surface.borrow_mut();
                s.invalidate();
                // requestRender() — state only; drained by the caller's pump.
                s.base_mut().request_render_state(false);
            }
        }
        true
    }

    /// `queryTerminalColors` (tui.ts:1470-1515) — promise → callbacks.
    pub(crate) fn query_terminal_colors_state(
        &mut self,
        timeout_ms: u64,
        on_result: ColorQueryCallback,
        on_late_reply: Option<ColorQueryCallback>,
    ) {
        let query = PendingTerminalColorQuery {
            foreground: None,
            background: None,
            palette: vec![None; TERMINAL_PALETTE_SIZE],
            replied: HashSet::new(),
            deliver: Some(on_result),
            deliver_on_timeout: on_late_reply,
            timer: Some(now_ms() + timeout_ms.max(0) as f64),
        };
        self.pending_color_queries.push(query);
        self.terminal.borrow_mut().write(TERMINAL_COLOR_QUERY);
    }

    /// Fire due color-query timeouts (the TS `setTimeout` callbacks) —
    /// called from the clock pump.
    pub(crate) fn fire_due_color_query_timeouts(&mut self) {
        let now = now_ms();
        for query in self.pending_color_queries.iter_mut() {
            let Some(deadline) = query.timer else { continue };
            if now < deadline {
                continue;
            }
            query.timer = None;
            // `query.deliver = onLateReply; resolve(result)`.
            if let Some(mut deliver) = query.deliver.take() {
                let result = Self::terminal_color_query_result(query);
                query.deliver = query.deliver_on_timeout.take();
                deliver(result);
            } else {
                query.deliver = query.deliver_on_timeout.take();
            }
        }
    }

    // ----- cursor markers (tui.ts:1417-1462) -----

    /// `extractCursorPosition(lines, height)` (tui.ts:1419-1445) — finds
    /// CURSOR_MARKER in the bottom `height` lines, strips it, and returns
    /// the visual cursor position.
    pub(crate) fn extract_cursor_position(lines: &mut [String], height: usize) -> Option<(usize, usize)> {
        let viewport_top = lines.len().saturating_sub(height);
        for row in (viewport_top..lines.len()).rev() {
            if let Some(marker_index) = lines[row].find(CURSOR_MARKER) {
                let before_marker = &lines[row][..marker_index];
                let col = crate::utils::visible_width(before_marker);
                let line = lines[row].clone();
                let mut stripped = String::with_capacity(line.len());
                stripped.push_str(&line[..marker_index]);
                stripped.push_str(&line[marker_index + CURSOR_MARKER.len()..]);
                lines[row] = stripped;
                return Some((row, col));
            }
        }
        None
    }

    /// `resolveFakeCursors(lines)` (tui.ts:1447-1461) — fake cursor markers
    /// to reverse video; when the hardware cursor is shown, a fake cursor
    /// directly after CURSOR_MARKER is dropped.
    pub(crate) fn resolve_fake_cursors(&self, lines: Vec<String>) -> Vec<String> {
        lines
            .into_iter()
            .map(|line| {
                if !line.contains(FAKE_CURSOR_START) {
                    return line;
                }
                let resolved = if self.show_hardware_cursor {
                    replace_focused_fake_cursor(&line)
                } else {
                    line
                };
                resolved
                    .replace(FAKE_CURSOR_START, "\x1b[7m")
                    .replace(FAKE_CURSOR_END, "\x1b[27m")
            })
            .collect()
    }

    /// `applyLineResets(lines)` (tui.ts:1464-1472).
    pub(crate) fn apply_line_resets(&self, lines: Vec<String>) -> Vec<String> {
        const RESET: &str = "\x1b[0m\x1b]8;;\x07";
        lines
            .into_iter()
            .map(|line| {
                if crate::component::is_image_line(&line) {
                    line
                } else {
                    crate::utils::normalize_terminal_output(&line) + RESET
                }
            })
            .collect()
    }
}

/// `if (this.focusedComponent?.handleInput)` — presence gate (tui.ts:1128).
fn handles_input_gate(handles: bool) -> bool {
    handles
}

/// `DEVICE_ATTRIBUTES_RESPONSE_PATTERN` (tui.ts:172):
/// `/^\x1b\[\?[\d;]*c$/`.
fn is_device_attributes_response(data: &str) -> bool {
    let Some(rest) = data.strip_prefix("\x1b[?") else { return false };
    let Some(body) = rest.strip_suffix('c') else { return false };
    body.bytes().all(|b| b.is_ascii_digit() || b == b';')
}

/// `/^\x1b\[6;(\d+);(\d+)t$/` (tui.ts:1194).
fn parse_cell_size_response(data: &str) -> Option<(i64, i64)> {
    let rest = data.strip_prefix("\x1b[6;")?;
    let rest = rest.strip_suffix('t')?;
    let mut parts = rest.split(';');
    let height = parts.next()?;
    let width = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if height.is_empty() || width.is_empty() || !height.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !width.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((height.parse().ok()?, width.parse().ok()?))
}

/// `FOCUSED_FAKE_CURSOR` replacement (tui.ts:202):
/// `/(\x1b_pi:c\x07)\x1b_pi:fc\x07(.*?)(?:\x1b_pi:\/fc\x07|$)/s` → `$1$2`.
/// One non-greedy match: the first `CURSOR_MARKER` followed by
/// `FAKE_CURSOR_START`, keeping the text up to the first `FAKE_CURSOR_END`
/// (or end of line) and dropping the markers.
fn replace_focused_fake_cursor(line: &str) -> String {
    let pattern = format!("{CURSOR_MARKER}{FAKE_CURSOR_START}");
    let Some(start) = line.find(&pattern) else { return line.to_string() };
    let text_start = start + pattern.len();
    let rest = &line[text_start..];
    let (text, consumed) = match rest.find(FAKE_CURSOR_END) {
        Some(end) => (&rest[..end], end + FAKE_CURSOR_END.len()),
        None => (rest, rest.len()),
    };
    // Regex `.*?` with the `s` flag matches any chars (incl. newlines)
    // non-greedily. Replace the whole match with `$1$2` (marker + text);
    // text after the fake cursor end is NOT part of the match and stays.
    format!("{}{}{}{}", &line[..start], CURSOR_MARKER, text, &line[text_start + consumed..])
}

// The query struct's late-reply slot (declared separately to keep the
// struct definition above readable).

// =============================================================================
// impl_tui_surface! — generate the shared TUI surface for a screen
// =============================================================================

/// Generate the [`TuiSurface`], [`crate::component::TuiHost`] and
/// [`Component`] delegation for a screen struct that owns a public `base:
/// TuiBase` field. The screen must also implement the hook methods
/// (`do_render`, `reset_render_state_hook`, …) — the macro references them.
#[macro_export]
macro_rules! impl_tui_surface {
    ($screen:ident) => {
        impl $screen {
            /// Record the weak self reference after creating the `Rc`.
            /// `new_rc`-style constructors call this. The concrete
            /// `Rc<RefCell<$screen>>` coerces to `Rc<RefCell<dyn TuiSurface>>`
            /// once, and the weak form is handed to the base.
            pub fn install_self(self_rc: &::std::rc::Rc<::std::cell::RefCell<$screen>>) {
                let surface: ::std::rc::Rc<::std::cell::RefCell<dyn $crate::tui::TuiSurface>> =
                    self_rc.clone();
                self_rc
                    .borrow_mut()
                    .base
                    .install_self(::std::rc::Rc::downgrade(&surface));
            }

            /// `addChild` (inherited from `Container` via `TuiBase`,
            /// tui.ts:363-365) — keeps `mountedRoots` in sync.
            pub fn add_child(&mut self, component: $crate::component::ComponentRef) {
                self.base.container.add_child(component.clone());
                self.base.mounted_roots.push(component);
            }

            /// `removeChild` (tui.ts:367-373) — keeps `mountedRoots` in sync.
            pub fn remove_child(&mut self, component: &$crate::component::ComponentRef) {
                self.base.container.remove_child(component);
                self.base
                    .mounted_roots
                    .retain(|c| !::std::rc::Rc::ptr_eq(c, component));
            }

            /// `getMountedRoots()` (tui.ts:717-719): `mountedRoots` is
            /// synced by `add_child`/`remove_child` (main screen, where it
            /// equals the container children) and by the alt screen's
            /// layout-root updates.
            pub fn mounted_roots(&self) -> Vec<$crate::component::ComponentRef> {
                self.base.mounted_roots.clone()
            }

            /// `requestRender(force)` (tui.rs port of tui.ts:1016-1028):
            /// hook + state; microtasks drain at op boundaries.
            pub fn request_render(&mut self, force: bool) {
                if force {
                    self.reset_render_state_hook();
                }
                self.base.request_render_state(force);
            }

            /// `renderNow(force)` (tui.ts:999-1005) — synchronous.
            pub fn render_now(&mut self, force: bool) {
                if force {
                    self.reset_render_state_hook();
                }
                self.base.render_now_begin();
                self.do_render();
            }

            /// Drain queued microtasks (TS `process.nextTick`).
            pub fn run_pending_ticks(&mut self) {
                while let Some(action) = self.base.drain_one_next_tick() {
                    if action == $crate::tui::NextTickAction::DoRender {
                        self.do_render();
                    }
                }
            }

            /// Advance the virtual clock, drain microtasks, fire due render
            /// timers and color-query timeouts.
            pub fn advance_clock(&mut self, ms: f64) {
                $crate::clock::advance_ms(ms);
                self.run_pending_ticks();
                while self.base.fire_due_render_timer() {
                    self.do_render();
                    self.base.rearm_after_timer_render();
                    self.run_pending_ticks();
                }
                self.base.fire_due_color_query_timeouts();
            }

            /// `start()` (tui.ts:924-937).
            pub fn start(&mut self) {
                self.before_terminal_start();
                self.base.start_terminal();
                self.after_terminal_start();
            }

            /// `stop(options)` (tui.ts:985-997).
            pub fn stop(&mut self, options: &$crate::tui::TuiStopOptions) {
                self.base.stopped = true;
                self.base.cancel_render_timer();
                if self.base.color_scheme_notifications_enabled {
                    self.base.terminal.borrow_mut().write("\x1b[?2031l");
                }
                self.before_terminal_stop(options);
                self.base.terminal.borrow_mut().show_cursor();
                self.base.terminal.borrow_mut().stop();
                self.after_terminal_stop(options);
            }

            /// `handleTerminalInput(data)` (tui.ts:1055-1131). The listener
            /// loop lives here (not on `TuiBase`) because the alt screen's
            /// self-listener dispatches to `handle_viewport_input` on the
            /// screen, which is already mutably borrowed through the base.
            pub fn handle_terminal_input(&mut self, data: &str) {
                if self.base.consume_color_input(data) {
                    return;
                }
                let mut data = data.to_string();
                if self.base.has_input_listeners() {
                    let mut current = data;
                    let mut index = 0;
                    while index < self.base.input_listeners.len() {
                        let listener_id = self.base.input_listeners[index].0;
                        // TS iterates the listener Set in insertion order;
                        // dispatching a listener can add or remove entries.
                        // Advancing only when the entry at `index` is still
                        // the one just dispatched mirrors Set semantics for
                        // self-removal and tail-removal.
                        let is_viewport = matches!(
                            self.base.input_listeners[index].1,
                            $crate::tui::InputListenerEntry::SelfViewport
                        );
                        let result = if is_viewport {
                            self.handle_viewport_input(&current)
                        } else {
                            match &mut self.base.input_listeners[index].1 {
                                $crate::tui::InputListenerEntry::Callback(cb) => cb(&current),
                                $crate::tui::InputListenerEntry::SelfViewport => unreachable!(),
                            }
                        };
                        if let Some(result) = result {
                            if result.consume == Some(true) {
                                return;
                            }
                            if let Some(next) = result.data {
                                current = next;
                            }
                        }
                        if index >= self.base.input_listeners.len()
                            || self.base.input_listeners[index].0 != listener_id
                        {
                            // The dispatched listener removed itself (or an
                            // earlier entry); the next listener shifted to
                            // `index` — stay put.
                        } else {
                            index += 1;
                        }
                    }
                    if current.is_empty() {
                        return;
                    }
                    data = current;
                }
                self.base.forward_input_after_listeners(&data);
            }
        }

        impl $crate::component::Component for $screen {
            fn render(&mut self, width: usize) -> Vec<String> {
                self.render_children(width)
            }

            fn handle_input(&mut self, _data: &str) {}

            fn handle_mouse(
                &mut self,
                self_ref: &$crate::component::ComponentRef,
                event: &$crate::component::TuiMouseEvent,
            ) -> Option<$crate::component::ComponentMouseResult> {
                self.handle_mouse_base(self_ref, event)
            }

            fn invalidate(&mut self) {
                // TuiBase.invalidate (tui.ts:916-919)
                for root in self.mounted_roots() {
                    root.borrow_mut().invalidate();
                }
                let overlays: Vec<$crate::component::ComponentRef> = self
                    .base
                    .overlay_stack
                    .iter()
                    .map(|e| e.component.clone())
                    .collect();
                for overlay in overlays {
                    overlay.borrow_mut().invalidate();
                }
            }

            /// The screen owns a `Container` (TS: screens extend
            /// `TuiBase extends Container`).
            fn as_container(&self) -> Option<&$crate::component::Container> {
                Some(&self.base.container)
            }
        }

        impl $crate::tui::TuiSurface for $screen {
            fn do_render(&mut self) {
                self.do_render()
            }
            fn reset_render_state_hook(&mut self) {
                self.reset_render_state_hook()
            }
            fn before_terminal_start(&mut self) {
                self.before_terminal_start()
            }
            fn after_terminal_start(&mut self) {
                self.after_terminal_start()
            }
            fn before_terminal_stop(&mut self, options: &$crate::tui::TuiStopOptions) {
                self.before_terminal_stop(options)
            }
            fn after_terminal_stop(&mut self, options: &$crate::tui::TuiStopOptions) {
                self.after_terminal_stop(options)
            }
            fn mounted_roots(&self) -> Vec<$crate::component::ComponentRef> {
                self.mounted_roots()
            }
            fn mode(&self) -> $crate::tui::TuiMode {
                self.mode()
            }
            fn handle_viewport_input(
                &mut self,
                data: &str,
            ) -> Option<$crate::tui::TuiInputListenerResult> {
                self.handle_viewport_input(data)
            }
            fn render_children(&mut self, width: usize) -> Vec<String> {
                self.render_children(width)
            }
            fn handle_mouse_base(
                &mut self,
                self_ref: &$crate::component::ComponentRef,
                event: &$crate::component::TuiMouseEvent,
            ) -> Option<$crate::component::ComponentMouseResult> {
                self.handle_mouse_base(self_ref, event)
            }

            fn base(&self) -> &$crate::tui::TuiBase {
                &self.base
            }
            fn base_mut(&mut self) -> &mut $crate::tui::TuiBase {
                &mut self.base
            }
            fn tui_id(&self) -> usize {
                self.base.tui_id
            }

            fn request_render(&mut self, force: bool) {
                self.request_render(force)
            }
            fn render_now(&mut self, force: bool) {
                self.render_now(force)
            }
            fn run_pending_ticks(&mut self) {
                self.run_pending_ticks()
            }
            fn advance_clock(&mut self, ms: f64) {
                self.advance_clock(ms)
            }
            fn start(&mut self) {
                self.start()
            }
            fn stop(&mut self, options: &$crate::tui::TuiStopOptions) {
                self.stop(options)
            }
            fn handle_terminal_input(&mut self, data: &str) {
                self.handle_terminal_input(data)
            }

            fn get_focused_component(&self) -> Option<$crate::component::ComponentRef> {
                self.base.get_focused_component()
            }
            fn set_focus(&mut self, component: Option<$crate::component::ComponentRef>) {
                self.base.set_focus_internal(component, $crate::tui::OverlayFocusRestorePolicy::Clear);
            }
            fn show_overlay(
                &mut self,
                component: $crate::component::ComponentRef,
                options: $crate::tui::OverlayOptions,
            ) -> $crate::tui::OverlayHandle {
                self.base.show_overlay(component, Some(options))
            }
            fn hide_overlay(&mut self) {
                self.base.hide_overlay_top()
            }
            fn hide_overlay_entry(&mut self, entry_id: u64) {
                self.base.hide_overlay_entry(entry_id)
            }
            fn set_overlay_hidden(&mut self, entry_id: u64, hidden: bool) {
                self.base.set_overlay_hidden(entry_id, hidden)
            }
            fn is_overlay_hidden(&self, entry_id: u64) -> bool {
                self.base.is_overlay_entry_hidden(entry_id)
            }
            fn focus_overlay(&mut self, entry_id: u64) {
                self.base.focus_overlay_entry(entry_id)
            }
            fn unfocus_overlay(
                &mut self,
                entry_id: u64,
                options: Option<$crate::tui::OverlayUnfocusOptions>,
            ) {
                self.base.unfocus_overlay_entry(entry_id, options)
            }
            fn is_overlay_focused(&self, entry_id: u64) -> bool {
                self.base.is_overlay_entry_focused(entry_id)
            }
            fn overlay_bounds(&self, entry_id: u64) -> Option<$crate::tui::OverlayBounds> {
                self.base.overlay_entry_bounds(entry_id)
            }
            fn has_overlay(&self) -> bool {
                self.base.has_visible_overlay()
            }

            fn get_show_hardware_cursor(&self) -> bool {
                self.base.get_show_hardware_cursor()
            }
            fn set_show_hardware_cursor(&mut self, enabled: bool) {
                self.base.set_show_hardware_cursor(enabled)
            }
            fn get_clear_on_shrink(&self) -> bool {
                self.base.get_clear_on_shrink()
            }
            fn set_clear_on_shrink(&mut self, enabled: bool) {
                self.base.set_clear_on_shrink(enabled)
            }
            fn full_redraws(&self) -> u64 {
                self.base.full_redraw_count
            }

            fn add_input_listener(
                &mut self,
                listener: Box<dyn FnMut(&str) -> Option<$crate::tui::TuiInputListenerResult>>,
            ) -> usize {
                self.base.add_input_listener(listener)
            }
            fn remove_input_listener(&mut self, listener_id: usize) {
                self.base.remove_input_listener(listener_id)
            }
            fn on_terminal_color_scheme_change(
                &mut self,
                listener: Box<dyn FnMut($crate::terminal_colors::TerminalColorScheme)>,
            ) -> usize {
                self.base.on_terminal_color_scheme_change(listener)
            }
            fn set_terminal_color_scheme_notifications(&mut self, enabled: bool) {
                self.base.set_terminal_color_scheme_notifications(enabled)
            }
            fn query_terminal_colors(
                &mut self,
                timeout_ms: u64,
                on_result: $crate::tui::ColorQueryCallback,
                on_late_reply: Option<$crate::tui::ColorQueryCallback>,
            ) {
                self.base.query_terminal_colors_state(timeout_ms, on_result, on_late_reply)
            }
            fn set_on_debug(&mut self, callback: Option<Box<dyn FnMut()>>) {
                self.base.on_debug = callback;
            }
        }

        impl $crate::component::TuiHost for $screen {
            fn request_render(&mut self) {
                $crate::tui::TuiSurface::request_render(self, false);
            }
            fn terminal_rows(&self) -> usize {
                self.base.terminal.borrow().rows()
            }
        }
    };
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_attributes_response_pattern() {
        assert!(is_device_attributes_response("\x1b[?c"));
        assert!(is_device_attributes_response("\x1b[?1;2c"));
        assert!(is_device_attributes_response("\x1b[?62;4;6c"));
        assert!(!is_device_attributes_response("\x1b[?1;2;3"));
        assert!(!is_device_attributes_response("x\x1b[?c"));
        assert!(!is_device_attributes_response("\x1b[?ac"));
    }

    #[test]
    fn cell_size_response_parse() {
        assert_eq!(parse_cell_size_response("\x1b[6;28;14t"), Some((28, 14)));
        assert_eq!(parse_cell_size_response("\x1b[6;0;14t"), Some((0, 14)));
        assert_eq!(parse_cell_size_response("\x1b[6;28t"), None);
        assert_eq!(parse_cell_size_response("\x1b[6;28;14;9t"), None);
        assert_eq!(parse_cell_size_response("hello"), None);
    }

    #[test]
    fn focused_fake_cursor_replacement() {
        // Marker + fake cursor + text + end → marker + text.
        let line = format!("ab{CURSOR_MARKER}{}xy{}cd", FAKE_CURSOR_START, FAKE_CURSOR_END);
        assert_eq!(replace_focused_fake_cursor(&line), format!("ab{CURSOR_MARKER}xycd"));
        // Truncated end (no FAKE_CURSOR_END): `$` alternative.
        let line = format!("ab{CURSOR_MARKER}{}xy", FAKE_CURSOR_START);
        assert_eq!(replace_focused_fake_cursor(&line), format!("ab{CURSOR_MARKER}xy"));
        // No fake cursor after the marker: no match, line unchanged.
        let line = format!("ab{CURSOR_MARKER}xy");
        assert_eq!(replace_focused_fake_cursor(&line), line);
        // Marker must directly precede the fake cursor start.
        let line = format!("{}q{FAKE_CURSOR_START}xy{FAKE_CURSOR_END}", CURSOR_MARKER);
        assert_eq!(replace_focused_fake_cursor(&line), line);
    }

    #[test]
    fn terminal_color_query_string() {
        // 10, 11, then OSC 4 for 0-15, then DA1.
        assert!(TERMINAL_COLOR_QUERY.starts_with("\x1b]10;?\x07\x1b]11;?\x07"));
        assert!(TERMINAL_COLOR_QUERY.ends_with("\x1b]4;15;?\x07\x1b[c"));
        assert_eq!(TERMINAL_COLOR_QUERY.matches("\x1b]4;").count(), 16);
    }
}
