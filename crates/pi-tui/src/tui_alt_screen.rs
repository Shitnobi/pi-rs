//! Port of `packages/tui/src/tui-alt-screen.ts` — the alternate-screen TUI
//! with a scrollable, application-owned viewport.
//!
//! ## File split (T5 Wave B)
//!
//! This file is the **coordinator-owned skeleton**: the constants, the
//! option/state types, the [`TuiAltScreen`] struct, the constructor, the
//! trivial public getters and the [`impl_tui_surface!`] hook wiring. The
//! behavioral halves live in sibling modules, one owner each:
//!
//! - `alt_screen_input.rs` (**D-B1**): terminal lifecycle (the
//!   `beforeTerminalStart/Stop`/`afterTerminalStart/Stop` bodies with all
//!   ESC sequences), `handleViewportInput`, the SGR mouse pipeline
//!   (`parseSgrMouseEvent`/`decodeMouseButton`/`createMouseEvent`/
//!   `dispatchMouseToLayout`/`applyMouseDispatchResult`/
//!   `dispatchMouseToTarget`/`getComponentClickCount`), scrollbar
//!   interactions, `routeWheel`, search toggle/navigate/refresh, and the
//!   scroll API (`scrollBy`/`scrollToTop`/`scrollToBottom`/`scrollToPrompt`).
//! - `alt_screen_selection.rs` (**D-B2**): the text-selection half of
//!   `tui-alt-screen.ts` (~1100-1462) — word/line selection with
//!   `TERMINAL_WORD_SELECTION_JOINERS`, auto-scroll, `getSelectionBounds`/
//!   `getSelectionColumns`, `getActiveSelectionText`,
//!   `applySelectionHighlight`/`applySelection`, the copy paths
//!   (including the OSC 52 fallback).
//! - `alt_screen_render.rs` (**D-B3**): the render half (~1440-1786) —
//!   `applySearchTextHighlight`/`applySearchHighlights`,
//!   `compositeScrollToEndIndicator`, `compositeFlashes`, the `doRender`
//!   pipeline, kitty image prepare/upload/eviction with the
//!   `MAX_CACHED_OFFSCREEN_KITTY_*` budgets, and `resetRenderState`.
//!
//! ## Key structural decisions (fixed — do not change without coordinator)
//!
//! - TS `implicitDocument` (tui-alt-screen.ts:262-268) is functionally the
//!   screen's own `Container` (TS `super.render`/`super.handleMouse` are
//!   `Container` methods, and the invalidate closure matches
//!   `Container.invalidate`). The port shares [`TuiBase::container`] (an
//!   `Rc<RefCell<Container>>`) by coercion: `implicit_document` IS the
//!   container viewed as a `ComponentRef`.
//! - TS closures like `() => this.requestRender()` (flash container, the
//!   layout solver callback) fire while the screen's `RefCell` is
//!   mutably borrowed; the port parks them by `tui_id` (see
//!   [`crate::tui::park_render_request`]) and drains at borrow release.
//! - The implicit scroll view is stored twice over the same allocation:
//!   `implicit_scroll_view` (a `ComponentRef`, the layout root fallback)
//!   and `implicit_scroll_view_state` (a [`ScrollViewRef`], the
//!   `getPrimaryScrollView()` fallback). `Rc` identity comparisons work
//!   because both coercions share the data pointer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::alt_screen_search::{
    AltScreenSearchComponent, AltScreenSearchIndex, AltScreenSearchMatch,
};
use crate::component::{Component, ComponentMouseResult, ComponentRef, TuiMouseEvent};
use crate::components::alt_screen_flash::AltScreenFlashContainer;
use crate::components::scroll_view::{ScrollView, ScrollViewFollow, ScrollViewOptions};
use crate::impl_tui_surface;
use crate::layout::{LayoutFrame, ScrollViewRef};
use crate::terminal::Terminal;
use crate::terminal_image::{ImageProtocol, TerminalCapabilities};
use crate::tui::{
    park_render_request, OverlayHandle, TuiBase, TuiInputListenerResult, TuiMode, TuiStopOptions,
};
use crate::wheel_scroll::{WheelScrollAccelerator, WheelScrollEnv, WheelScrollLines};

// =============================================================================
// Constants (tui-alt-screen.ts:53-77)
// =============================================================================

pub(crate) const ENTER_ALT_SCREEN: &str = "\x1b[?1049h";
pub(crate) const EXIT_ALT_SCREEN: &str = "\x1b[?1049l";
pub(crate) const DISABLE_AUTOWRAP: &str = "\x1b[?7l";
pub(crate) const ENABLE_AUTOWRAP: &str = "\x1b[?7h";
pub(crate) const ENABLE_BUTTON_MOTION_MOUSE: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1004h\x1b[?1006h";
pub(crate) const ENABLE_ALL_MOTION_MOUSE: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1004h\x1b[?1006h";
pub(crate) const DISABLE_MOUSE: &str = "\x1b[?1006l\x1b[?1004l\x1b[?1003l\x1b[?1002l\x1b[?1000l";
pub(crate) const FOCUS_IN: &str = "\x1b[I";
pub(crate) const FOCUS_OUT: &str = "\x1b[O";
pub(crate) const BEGIN_SYNCHRONIZED_OUTPUT: &str = "\x1b[?2026h";
pub(crate) const END_SYNCHRONIZED_OUTPUT: &str = "\x1b[?2026l";
/// `PAGE_SCROLL_OVERLAP` (tui-alt-screen.ts:68).
pub(crate) const PAGE_SCROLL_OVERLAP: i64 = 4;
/// `ALT_WHEEL_SCROLL_MULTIPLIER` (tui-alt-screen.ts:69).
pub(crate) const ALT_WHEEL_SCROLL_MULTIPLIER: i64 = 5;
/// `MAX_CACHED_OFFSCREEN_KITTY_IMAGES` (tui-alt-screen.ts:70).
pub(crate) const MAX_CACHED_OFFSCREEN_KITTY_IMAGES: usize = 16;
/// `MAX_CACHED_OFFSCREEN_KITTY_TRANSMISSION_BYTES` (tui-alt-screen.ts:71).
pub(crate) const MAX_CACHED_OFFSCREEN_KITTY_TRANSMISSION_BYTES: i64 = 32 * 1024 * 1024;
/// `MAX_CACHED_OFFSCREEN_KITTY_DECODED_BYTES` (tui-alt-screen.ts:72).
pub(crate) const MAX_CACHED_OFFSCREEN_KITTY_DECODED_BYTES: i64 = 64 * 1024 * 1024;
/// `DOUBLE_CLICK_INTERVAL_MS` (tui-alt-screen.ts:73).
pub(crate) const DOUBLE_CLICK_INTERVAL_MS: f64 = 500.0;
/// `COPY_ERROR_FLASH_DURATION_MS` (tui-alt-screen.ts:74).
pub(crate) const COPY_ERROR_FLASH_DURATION_MS: f64 = 5000.0;
/// `TERMINAL_WORD_SELECTION_JOINERS` (tui-alt-screen.ts:78-79): `/` and
/// `-` keep paths and kebab-case tokens whole during word selection.
pub(crate) const TERMINAL_WORD_SELECTION_JOINERS: [char; 2] = ['/', '-'];

/// `OSC133_ZONE_PREFIX` (tui-alt-screen.ts:66): the regex
/// `/^(?:\x1b\]133;[ABC](?:\x07|\x1b\\))+/` as an explicit matcher —
/// strips the leading run of shell-integration zone markers. Used by the
/// doRender fake-cursor step (**D-B3**) and selection (**D-B2**).
pub(crate) fn strip_osc133_zone_prefix(line: &str) -> &str {
    let mut rest = line;
    'outer: loop {
        let Some(after) = rest.strip_prefix("\x1b]133;") else { break };
        let Some(command) = after.chars().next() else { break };
        if !matches!(command, 'A' | 'B' | 'C') {
            break;
        }
        let tail = &after[command.len_utf8()..];
        if let Some(tail) = tail.strip_prefix('\x07') {
            rest = tail;
            continue 'outer;
        }
        if let Some(tail) = tail.strip_prefix("\x1b\\") {
            rest = tail;
            continue 'outer;
        }
        break;
    }
    rest
}

/// `OSC133_PROMPT_START` (tui-alt-screen.ts:67):
/// `/^\x1b\]133;A(?:\x07|\x1b\\)/` as an explicit matcher.
pub(crate) fn starts_with_osc133_prompt_start(line: &str) -> bool {
    let Some(after) = line.strip_prefix("\x1b]133;A") else { return false };
    after.starts_with('\x07') || after.starts_with("\x1b\\")
}

// =============================================================================
// Options (tui-alt-screen.ts:157-203)
// =============================================================================

/// `searchMatchStyle?: (text: string) => string` — defaults to
/// `\x1b[4m{text}\x1b[24m` (underline).
pub type SearchMatchStyleFn = Rc<dyn Fn(&str) -> String>;
/// `searchCurrentMatchStyle?: (text: string) => string` — defaults to
/// `\x1b[1;7m{text}\x1b[22;27m` (bold + reverse).
pub type SearchCurrentMatchStyleFn = Rc<dyn Fn(&str) -> String>;
/// `searchNavigationButtonStyle?: (text: string, hovered: boolean) => string`.
pub type SearchNavigationButtonStyleFn = Rc<dyn Fn(&str, bool) -> String>;
/// `scrollToEndIndicator?: () => string`.
pub type ScrollToEndIndicatorFn = Rc<dyn Fn() -> String>;
/// `openUrl?: (url: string) => void`.
pub type OpenUrlFn = Rc<dyn Fn(&str)>;
/// `onRightClickPaste?: () => void`.
pub type OnRightClickPasteFn = Rc<dyn Fn()>;
/// `copySelection?: (text: string) => Promise<boolean | string>` — the TS
/// promise resolves `true` (copied), `false` (generic error flash) or a
/// message string (flashed verbatim). Pure-std port: the callback returns
/// synchronously.
pub type CopySelectionFn = Rc<dyn Fn(&str) -> CopySelectionResult>;

/// Synchronous stand-in for the TS `Promise<boolean | string>` returned by
/// `copySelection`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopySelectionResult {
    /// Resolved `true` — copied successfully.
    Success,
    /// Resolved `false` — show the generic copy-error flash.
    Failure,
    /// Resolved a message string — flash it verbatim.
    Message(String),
}

/// `TuiAltScreenOptions` (tui-alt-screen.ts:157-181).
#[derive(Default, Clone)]
pub struct TuiAltScreenOptions {
    /// `wheelScrollLines?: WheelScrollLines` — default `1`.
    pub wheel_scroll_lines: Option<WheelScrollLines>,
    /// `mouse?: boolean` — default `true`.
    pub mouse: Option<bool>,
    pub search_match_style: Option<SearchMatchStyleFn>,
    pub search_current_match_style: Option<SearchCurrentMatchStyleFn>,
    pub search_navigation_button_style: Option<SearchNavigationButtonStyleFn>,
    pub scroll_to_end_indicator: Option<ScrollToEndIndicatorFn>,
    pub open_url: Option<OpenUrlFn>,
    pub on_right_click_paste: Option<OnRightClickPasteFn>,
    /// `copyOnSelect?: boolean` — default `true`.
    pub copy_on_select: Option<bool>,
    pub copy_selection: Option<CopySelectionFn>,
}

impl TuiAltScreenOptions {
    /// The constructor's `??` defaults, applied once.
    fn apply_defaults(self) -> Self {
        TuiAltScreenOptions {
            wheel_scroll_lines: Some(
                self.wheel_scroll_lines.unwrap_or(WheelScrollLines::Lines(1.0)),
            ),
            mouse: Some(self.mouse.unwrap_or(true)),
            search_match_style: Some(
                self.search_match_style
                    .unwrap_or_else(|| Rc::new(|text: &str| format!("\x1b[4m{text}\x1b[24m"))),
            ),
            search_current_match_style: Some(
                self.search_current_match_style
                    .unwrap_or_else(|| Rc::new(|text: &str| format!("\x1b[1;7m{text}\x1b[22;27m"))),
            ),
            search_navigation_button_style: Some(
                self.search_navigation_button_style
                    .unwrap_or_else(|| Rc::new(|text: &str, _hovered: bool| text.to_string())),
            ),
            scroll_to_end_indicator: self.scroll_to_end_indicator,
            open_url: self.open_url,
            on_right_click_paste: self.on_right_click_paste,
            copy_on_select: Some(self.copy_on_select.unwrap_or(true)),
            copy_selection: self.copy_selection,
        }
    }
}

// =============================================================================
// State types (tui-alt-screen.ts:88-165)
// =============================================================================

/// `CachedKittyImage` (tui-alt-screen.ts:88-92) — offscreen kitty image
/// cache accounting (**D-B3**).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct CachedKittyImage {
    pub transmission_generation: i64,
    pub transmission_bytes: i64,
    pub estimated_decoded_bytes: i64,
}

/// `SelectionPoint` (tui-alt-screen.ts:94-100) — **D-B2**. No
/// `Debug`/`PartialEq` derives: the `scroll_view` handle is a trait
/// object (TS compares points by row/col, scroll views by reference).
#[derive(Clone)]
pub(crate) struct SelectionPoint {
    pub row: i64,
    pub col: i64,
    pub scroll_view: Option<ScrollViewRef>,
    /// Whether this point lies between terminal cells rather than on a
    /// cell (`boundary?: boolean`).
    pub boundary: bool,
}

/// `SelectionRange` (tui-alt-screen.ts:102-105) — **D-B2**.
#[derive(Clone)]
pub(crate) struct SelectionRange {
    pub start: SelectionPoint,
    pub end: SelectionPoint,
}

/// `SelectionGranularity` (tui-alt-screen.ts:107).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionGranularity {
    Character,
    Word,
    Line,
}

/// `ClickTarget` (tui-alt-screen.ts:109-117) — multi-click history
/// (**D-B2**). `timestamp` is virtual-clock ms.
#[derive(Clone)]
pub(crate) struct ClickTarget {
    pub timestamp: f64,
    pub count: i64,
    pub row: i64,
    pub scroll_view: Option<ScrollViewRef>,
    pub word_start: i64,
    pub word_end: i64,
}

/// `SgrMouseEvent` (tui-alt-screen.ts:119-124) — **D-B1**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SgrMouseEvent {
    pub button: i64,
    pub x: i64,
    pub y: i64,
    pub release: bool,
}

/// `WheelEvent` (tui-alt-screen.ts:126-131) — **D-B1**. `direction` is
/// `-1 | 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WheelEvent {
    pub direction: i64,
    pub x: i64,
    pub y: i64,
    pub button: i64,
}

/// `ScrollbarDrag` (tui-alt-screen.ts:132-135) — **D-B1**. No `Debug`:
/// the `scroll_view` handle is a trait object.
#[derive(Clone)]
pub(crate) struct ScrollbarDragState {
    pub scroll_view: ScrollViewRef,
    pub grab_offset: i64,
}

/// `ScrollToEndIndicatorRect` (tui-alt-screen.ts:142-146) — **D-B1** hit
/// rect, composited by **D-B3**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScrollToEndIndicatorRect {
    pub row: i64,
    pub column: i64,
    pub width: i64,
}

/// `SearchSelectionMode` (tui-alt-screen.ts:148).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchSelectionMode {
    Query,
    Retain,
    Next,
    Previous,
}

/// `ActiveSearch` (tui-alt-screen.ts:150-160) — owned by **D-B1**'s
/// toggle/navigate/refresh, consumed by **D-B3**'s highlight pass.
pub(crate) struct ActiveSearch {
    pub component: Rc<RefCell<AltScreenSearchComponent>>,
    pub index: AltScreenSearchIndex,
    pub overlay: Option<OverlayHandle>,
    pub query: String,
    pub matches: Vec<AltScreenSearchMatch>,
    pub selected_index: i64,
    pub selected_key: Option<String>,
    pub anchor_row: i64,
    pub selection_mode: SearchSelectionMode,
}

// =============================================================================
// TuiAltScreen
// =============================================================================

/// Alternate-screen TUI with a scrollable, application-owned viewport
/// (tui-alt-screen.ts:203).
pub struct TuiAltScreen {
    /// Composition root (TS: `extends TuiBase`).
    pub base: TuiBase,
    /// `previousScreen: string[]` — the last frame as written to the
    /// terminal (`getScreenLines()`).
    pub(crate) previous_screen: Vec<String>,
    /// `lastDocument: string[]` — the last un-truncated document lines.
    pub(crate) last_document: Vec<String>,
    pub(crate) previous_screen_width: i64,
    pub(crate) previous_screen_height: i64,
    /// `layoutRoot?: Component`.
    pub(crate) layout_root: Option<ComponentRef>,
    /// `currentLayout?: LayoutFrame`.
    pub(crate) current_layout: Option<LayoutFrame>,
    /// The implicit scroll view as a `ComponentRef` — the doRender layout
    /// root when `layoutRoot` is unset (tui-alt-screen.ts:1683). Shares the
    /// allocation with [`Self::implicit_scroll_view_state`].
    pub(crate) implicit_scroll_view: ComponentRef,
    /// The same scroll view as the layout solver addresses it — the
    /// `getPrimaryScrollView()` fallback (tui-alt-screen.ts:343).
    pub(crate) implicit_scroll_view_state: ScrollViewRef,
    /// `flashes: AltScreenFlashContainer` — its `requestRender` closure
    /// parks by `tui_id` (see module docs).
    pub(crate) flashes: AltScreenFlashContainer,
    /// `altScreenActive: boolean`.
    pub(crate) alt_screen_active: bool,
    /// `imageProtocol: ImageProtocol`.
    /// `imageProtocol: ImageProtocol | null` — `None` is TS `null`.
    pub(crate) image_protocol: Option<ImageProtocol>,
    /// `savedCapabilities?: TerminalCapabilities` — restored on stop when
    /// iTerm2 image support was masked at start.
    pub(crate) saved_capabilities: Option<TerminalCapabilities>,
    /// `uploadedKittyImages: Map<number, CachedKittyImage>` — **D-B3**.
    pub(crate) uploaded_kitty_images: HashMap<i64, CachedKittyImage>,
    // ----- selection state (D-B2) -----
    pub(crate) selection_anchor: Option<SelectionPoint>,
    pub(crate) selection_focus: Option<SelectionPoint>,
    pub(crate) selection_granularity: SelectionGranularity,
    pub(crate) selection_initial_range: Option<SelectionRange>,
    pub(crate) last_click: Option<ClickTarget>,
    pub(crate) selection_drag_pointer: Option<(i64, i64)>,
    /// `-1 | 0 | 1`.
    pub(crate) selection_auto_scroll_direction: i64,
    /// `selectionAutoScrollTimer?: NodeJS.Timeout` — a virtual-clock
    /// deadline (ms); **D-B2** fires it from the clock pump.
    pub(crate) selection_auto_scroll_timer: Option<f64>,
    pub(crate) selection_press_active: bool,
    // ----- scrollbar / indicator state (D-B1) -----
    pub(crate) scrollbar_drag: Option<ScrollbarDragState>,
    pub(crate) scrollbar_hover: Option<ScrollViewRef>,
    pub(crate) scroll_to_end_indicator_rect: Option<ScrollToEndIndicatorRect>,
    // ----- search / mouse gesture state -----
    pub(crate) active_search: Option<ActiveSearch>,
    pub(crate) pressed_url: Option<String>,
    pub(crate) selection_dragged: bool,
    pub(crate) mouse_capture: Option<crate::component::TuiMouseDispatchTarget>,
    pub(crate) mouse_press_target: Option<crate::component::TuiMouseDispatchTarget>,
    pub(crate) mouse_press_point: Option<(i64, i64)>,
    pub(crate) mouse_press_moved: bool,
    /// `lastComponentClick` (tui-alt-screen.ts:247-253) — **D-B1**.
    pub(crate) last_component_click: Option<LastComponentClick>,
    // ----- options ------
    pub(crate) wheel_scroll: WheelScrollAccelerator,
    pub(crate) mouse_enabled: bool,
    pub(crate) search_match_style: SearchMatchStyleFn,
    pub(crate) search_current_match_style: SearchCurrentMatchStyleFn,
    pub(crate) search_navigation_button_style: SearchNavigationButtonStyleFn,
    pub(crate) scroll_to_end_indicator: Option<ScrollToEndIndicatorFn>,
    pub(crate) open_url: Option<OpenUrlFn>,
    pub(crate) on_right_click_paste: Option<OnRightClickPasteFn>,
    pub(crate) copy_on_select: bool,
    pub(crate) copy_selection: Option<CopySelectionFn>,
}

/// `lastComponentClick` (tui-alt-screen.ts:247-253).
pub(crate) struct LastComponentClick {
    pub timestamp: f64,
    pub count: i64,
    pub component: ComponentRef,
    pub x: i64,
    pub y: i64,
}

impl TuiAltScreen {
    /// `constructor(terminal, showHardwareCursor?, logDirectory?,
    /// options)` (tui-alt-screen.ts:255-283). `logDirectory` is env-gated
    /// debug output — see T5_PLAN divergences.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        terminal: Rc<RefCell<dyn Terminal>>,
        show_hardware_cursor: Option<bool>,
        _log_directory: Option<String>,
        options: TuiAltScreenOptions,
    ) -> Rc<RefCell<TuiAltScreen>> {
        let options = options.apply_defaults();
        let base = TuiBase::new(terminal, show_hardware_cursor);
        let tui_id = base.tui_id;

        // implicitDocument = the shared container (see module docs); the
        // scroll view wraps it with `{ follow: "end", primary: true }`.
        let implicit_document: ComponentRef = base.container.clone();
        let scroll_view = Rc::new(RefCell::new(ScrollView::new(
            implicit_document,
            ScrollViewOptions {
                follow: Some(ScrollViewFollow::End),
                primary: true,
                ..ScrollViewOptions::default()
            },
        )));
        let implicit_scroll_view: ComponentRef = scroll_view.clone();
        let implicit_scroll_view_state: ScrollViewRef = scroll_view.borrow().state();

        let flashes =
            AltScreenFlashContainer::new(Some(Box::new(move || park_render_request(tui_id))));

        // TS `new WheelScrollAccelerator(lines)` reads the platform/env
        // itself (wheel-scroll.ts:18-25).
        let wheel_env = WheelScrollEnv {
            is_darwin: cfg!(target_os = "macos"),
            ssh_free: std::env::var_os("SSH_CONNECTION").is_none()
                && std::env::var_os("SSH_CLIENT").is_none()
                && std::env::var_os("SSH_TTY").is_none(),
        };
        let wheel_scroll = WheelScrollAccelerator::new(
            options.wheel_scroll_lines.clone().unwrap_or(WheelScrollLines::Lines(1.0)),
            None,
            &wheel_env,
        );

        let rc = Rc::new(RefCell::new(TuiAltScreen {
            base,
            previous_screen: Vec::new(),
            last_document: Vec::new(),
            previous_screen_width: 0,
            previous_screen_height: 0,
            layout_root: None,
            current_layout: None,
            implicit_scroll_view,
            implicit_scroll_view_state,
            flashes,
            alt_screen_active: false,
            image_protocol: None, // TS `null` — set from capabilities at start
            saved_capabilities: None,
            uploaded_kitty_images: HashMap::new(),
            selection_anchor: None,
            selection_focus: None,
            selection_granularity: SelectionGranularity::Character,
            selection_initial_range: None,
            last_click: None,
            selection_drag_pointer: None,
            selection_auto_scroll_direction: 0,
            selection_auto_scroll_timer: None,
            selection_press_active: false,
            scrollbar_drag: None,
            scrollbar_hover: None,
            scroll_to_end_indicator_rect: None,
            active_search: None,
            pressed_url: None,
            selection_dragged: false,
            mouse_capture: None,
            mouse_press_target: None,
            mouse_press_point: None,
            mouse_press_moved: false,
            last_component_click: None,
            wheel_scroll,
            mouse_enabled: options.mouse.unwrap_or(true),
            search_match_style: options.search_match_style.unwrap(),
            search_current_match_style: options.search_current_match_style.unwrap(),
            search_navigation_button_style: options.search_navigation_button_style.unwrap(),
            scroll_to_end_indicator: options.scroll_to_end_indicator,
            open_url: options.open_url,
            on_right_click_paste: options.on_right_click_paste,
            copy_on_select: options.copy_on_select.unwrap_or(true),
            copy_selection: options.copy_selection,
        }));
        TuiAltScreen::install_self(&rc);
        // `this.addInputListener((data) => this.handleViewportInput(data))`
        // (tui-alt-screen.ts:282) — dispatched through the macro's
        // SelfViewport entry.
        rc.borrow_mut().base.add_viewport_input_listener();
        rc
    }

    // ----- public getters (tui-alt-screen.ts:284-339) -----

    /// `get viewportTop()` (tui-alt-screen.ts:284-286).
    pub fn viewport_top(&self) -> i64 {
        self.get_primary_scroll_view().borrow().scroll_top()
    }

    /// `get isFollowingOutput()` (tui-alt-screen.ts:288-290).
    pub fn is_following_output(&self) -> bool {
        self.get_primary_scroll_view().borrow().is_following_end()
    }

    /// `setWheelScrollLines(lines)` (tui-alt-screen.ts:292-294).
    pub fn set_wheel_scroll_lines(&mut self, lines: WheelScrollLines) {
        self.wheel_scroll.set_lines(lines);
    }

    /// `getCopyOnSelect()` (tui-alt-screen.ts:296-298).
    pub fn get_copy_on_select(&self) -> bool {
        self.copy_on_select
    }

    /// `setCopyOnSelect(enabled)` (tui-alt-screen.ts:300-302).
    pub fn set_copy_on_select(&mut self, enabled: bool) {
        self.copy_on_select = enabled;
    }

    /// `hasActiveSelection()` (tui-alt-screen.ts:305-307) — delegates to
    /// **D-B2**'s `getActiveSelectionText`.
    pub fn has_active_selection(&self) -> bool {
        crate::alt_screen_selection::get_active_selection_text(self).is_some()
    }

    /// `copyActiveSelectionToClipboard()` (tui-alt-screen.ts:309-312) —
    /// delegates to **D-B2**. The TS promise resolves synchronously in the
    /// pure-std port.
    pub fn copy_active_selection_to_clipboard(&mut self) -> bool {
        crate::alt_screen_selection::copy_active_selection_to_clipboard(self)
    }

    /// `resetTextSelection()` (tui-alt-screen.ts:314-317) — delegates to
    /// **D-B2**.
    pub fn reset_text_selection(&mut self) {
        crate::alt_screen_selection::reset_text_selection(self);
    }

    /// `getScreenLines()` (tui-alt-screen.ts:320-322): the lines of the
    /// last rendered frame, one per terminal row, as written.
    pub fn get_screen_lines(&self) -> Vec<String> {
        self.previous_screen.clone()
    }

    /// `setLayoutRoot(component)` (tui-alt-screen.ts:324-328).
    pub fn set_layout_root(&mut self, component: Option<ComponentRef>) {
        let unchanged = match (&self.layout_root, &component) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return;
        }
        self.layout_root = component;
        self.current_layout = None;
        self.refresh_mounted_roots();
        self.request_render(false);
    }

    /// `getPrimaryScrollView()` (tui-alt-screen.ts:343).
    pub(crate) fn get_primary_scroll_view(&self) -> ScrollViewRef {
        self.current_layout
            .as_ref()
            .and_then(|layout| layout.primary_scroll_view.clone())
            .unwrap_or_else(|| self.implicit_scroll_view_state.clone())
    }

    /// `getMountedRoots()` override (tui-alt-screen.ts:337-339):
    /// `layoutRoot ? [layoutRoot] : children`. Re-synced on every layout
    /// change (this helper) and on `add_child`/`remove_child` (macro).
    fn refresh_mounted_roots(&mut self) {
        self.base.mounted_roots = match &self.layout_root {
            Some(root) => vec![root.clone()],
            None => self.base.container.borrow().children.clone(),
        };
    }

    /// `flash(message, durationMs = 1000)` — public passthrough
    /// (`AltScreenFlashContainer.flash`).
    pub fn flash(&mut self, message: &str, duration_ms: Option<f64>) {
        self.flashes.flash(message, duration_ms);
        self.base.drain_parked_render_request();
    }

    // ----- impl_tui_surface! hooks -----

    /// `doRender()` (tui-alt-screen.ts:1679+) — **D-B3**
    /// (`alt_screen_render.rs`).
    fn do_render(&mut self) {
        crate::alt_screen_render::do_render(self);
    }

    /// `resetRenderState()` (tui-alt-screen.ts) — **D-B3**.
    fn reset_render_state_hook(&mut self) {
        crate::alt_screen_render::reset_render_state(self);
    }

    /// `beforeTerminalStart()` (tui-alt-screen.ts:345-...) — **D-B1**
    /// (`alt_screen_input.rs`).
    fn before_terminal_start(&mut self) {
        crate::alt_screen_input::before_terminal_start(self);
    }

    /// `afterTerminalStart()` — **D-B1**.
    fn after_terminal_start(&mut self) {
        crate::alt_screen_input::after_terminal_start(self);
    }

    /// `beforeTerminalStop(options)` — **D-B1**.
    fn before_terminal_stop(&mut self, options: &TuiStopOptions) {
        crate::alt_screen_input::before_terminal_stop(self, options);
    }

    /// `afterTerminalStop(options)` — **D-B1**.
    fn after_terminal_stop(&mut self, options: &TuiStopOptions) {
        crate::alt_screen_input::after_terminal_stop(self, options);
    }

    /// Port machinery (no TS counterpart): `advanceClock` pumps the
    /// selection auto-scroll interval modeled on the virtual clock
    /// (TS: `setInterval(() => this.autoScrollSelection(), 50)`).
    /// The next deadline among the screen-owned timers: the selection
    /// auto-scroll `setInterval` and the flash-expiry `setTimeout`s.
    fn next_screen_timer_deadline(&self) -> Option<f64> {
        [self.selection_auto_scroll_timer, self.flashes.next_deadline()]
            .into_iter()
            .flatten()
            .reduce(f64::min)
    }

    /// Fire the screen-owned timers due at or before `bound`.
    fn pump_screen_timers_until(&mut self, bound: f64) -> bool {
        // TS: the selection auto-scroll `setInterval` callback
        // (tui-alt-screen.ts:1290).
        crate::alt_screen_selection::pump_selection_auto_scroll_until(self, bound);
        // TS: flash-expiry `setTimeout` callbacks remove the entry and
        // request a render (alt-screen-flash.ts:22-29).
        self.flashes.tick_until(bound)
    }

    /// `readonly mode = "fullscreen"`.
    fn mode(&self) -> TuiMode {
        TuiMode::Fullscreen
    }

    /// `render(width)` override (tui-alt-screen.ts:331-333):
    /// `layoutRoot?.render(width) ?? super.render(width)`.
    fn render_children(&mut self, width: usize) -> Vec<String> {
        match &self.layout_root {
            Some(root) => root.borrow_mut().render(width),
            None => Component::render(&mut *self.base.container.borrow_mut(), width),
        }
    }

    /// `handleMouse` — inherited from `Container` (no alt-screen
    /// override; the alt screen's mouse pipeline lives in
    /// `handleViewportInput`, **D-B1**).
    fn handle_mouse_base(
        &mut self,
        self_ref: &ComponentRef,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        Component::handle_mouse(&mut *self.base.container.borrow_mut(), self_ref, event)
    }

    /// `(data) => this.handleViewportInput(data)` (tui-alt-screen.ts:282)
    /// — **D-B1** (`alt_screen_input.rs`).
    fn handle_viewport_input(&mut self, data: &str) -> Option<TuiInputListenerResult> {
        crate::alt_screen_input::handle_viewport_input(self, data)
    }
}

impl_tui_surface!(TuiAltScreen);

// =============================================================================
// Tests (skeleton smoke tests; behavioral parity tests arrive with the
// Wave B delegate modules and the Wave C harness)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::Component;
    use crate::terminal::{InputHandler, ProgramStatus, ResizeHandler, Terminal};
    use crate::test_support::global_test_lock;
    use std::cell::Cell;

    /// Recording terminal double (pattern from tui_overlay.rs tests).
    struct TestTerminal {
        cols: Cell<usize>,
        rows: Cell<usize>,
    }

    impl TestTerminal {
        fn new(cols: usize, rows: usize) -> Self {
            TestTerminal { cols: Cell::new(cols), rows: Cell::new(rows) }
        }
    }

    impl Terminal for TestTerminal {
        fn start(&mut self, _on_input: InputHandler, _on_resize: ResizeHandler) {}
        fn stop(&mut self) {}
        fn drain_input(&mut self, _max_ms: u64, _idle_ms: u64) {}
        fn write(&mut self, _data: &str) {}
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

    /// One-line label component.
    struct Label {
        text: String,
    }

    impl Component for Label {
        fn render(&mut self, width: usize) -> Vec<String> {
            vec![crate::utils::truncate_to_width(&self.text, width, "", false)]
        }
    }

    fn label_component(text: &str) -> ComponentRef {
        Rc::new(RefCell::new(Label { text: text.to_string() }))
    }

    fn new_screen(cols: usize, rows: usize) -> Rc<RefCell<TuiAltScreen>> {
        let terminal: Rc<RefCell<dyn Terminal>> =
            Rc::new(RefCell::new(TestTerminal::new(cols, rows)));
        TuiAltScreen::new(terminal, None, None, TuiAltScreenOptions::default())
    }

    #[test]
    fn osc133_prefix_stripping_matches_the_regex() {
        // /^(?:\x1b\]133;[ABC](?:\x07|\x1b\\))+/
        assert_eq!(strip_osc133_zone_prefix("plain"), "plain");
        assert_eq!(strip_osc133_zone_prefix("\x1b]133;A\x07hello"), "hello");
        assert_eq!(strip_osc133_zone_prefix("\x1b]133;A\x1b\\x"), "x");
        assert_eq!(
            strip_osc133_zone_prefix("\x1b]133;B\x07\x1b]133;C\x1b\\y"),
            "y"
        );
        // Incomplete terminator: no strip.
        assert_eq!(strip_osc133_zone_prefix("\x1b]133;A"), "\x1b]133;A");
        // Unknown zone command: no strip.
        assert_eq!(strip_osc133_zone_prefix("\x1b]133;D\x07z"), "\x1b]133;D\x07z");
    }

    #[test]
    fn osc133_prompt_start_matches_the_regex() {
        assert!(starts_with_osc133_prompt_start("\x1b]133;A\x07$ "));
        assert!(starts_with_osc133_prompt_start("\x1b]133;A\x1b\\$ "));
        assert!(!starts_with_osc133_prompt_start("\x1b]133;B\x07$ "));
        assert!(!starts_with_osc133_prompt_start("\x1b]133;A"));
        assert!(!starts_with_osc133_prompt_start("$ "));
    }

    #[test]
    fn constructor_builds_implicit_scroll_view_over_shared_container() {
        let _guard = global_test_lock();
        let screen = new_screen(20, 5);
        let borrowed = screen.borrow();
        assert_eq!(borrowed.mode(), TuiMode::Fullscreen);
        assert!(!borrowed.alt_screen_active);
        assert!(borrowed.get_screen_lines().is_empty());
        assert!(borrowed.base.container.borrow().children.is_empty());
        // `followingEnd` initializes from `follow: "end"` (scroll-view.ts:49).
        assert!(borrowed.is_following_output());
    }

    #[test]
    fn set_layout_root_resyncs_mounted_roots() {
        let _guard = global_test_lock();
        let screen = new_screen(20, 5);

        let child = label_component("hi");
        screen.borrow_mut().add_child(child.clone());
        assert_eq!(screen.borrow().mounted_roots().len(), 1);

        let root = label_component("root");
        screen.borrow_mut().set_layout_root(Some(root.clone()));
        // getMountedRoots() override: [layoutRoot], not the children.
        let roots = screen.borrow().mounted_roots();
        assert_eq!(roots.len(), 1);
        assert!(Rc::ptr_eq(&roots[0], &root));

        screen.borrow_mut().set_layout_root(None);
        let roots = screen.borrow().mounted_roots();
        assert_eq!(roots.len(), 1);
        assert!(Rc::ptr_eq(&roots[0], &child));

        // Identity no-op: no new render request. render_now clears the
        // request flag first.
        screen.borrow_mut().render_now(true);
        assert!(!screen.borrow().base.render_requested_probe());
        screen.borrow_mut().set_layout_root(None);
        assert!(!screen.borrow().base.render_requested_probe());
    }
}
