//! The `Component` foundation — port of the component surface of
//! `packages/tui/src/tui.ts` (Component interface, mouse event types,
//! `dispatchMouseEvent`, `retargetMouseEvent`, `Container`).
//!
//! The full renderer (`TuiBase`/`TUI`) lands in `tui.rs` with tranche T5;
//! this module holds only what components and the layout solver need.
//!
//! ## Port conventions (crate-wide — read before porting components)
//!
//! - **Shared components**: TS passes components as shared mutable object
//!   references. The Rust port uses [`ComponentRef`] =
//!   `Rc<RefCell<dyn Component>>` everywhere a TS signature says
//!   `Component`. Child lists, dispatch targets, and focus targets all hold
//!   `ComponentRef` clones (identity via `Rc::ptr_eq`).
//! - **`render` takes `&mut self`**: the TS components mutate render caches
//!   inside `render()` (e.g. `Text.cachedLines`, `Container.mouseLayout`),
//!   so the honest Rust translation is `&mut self`.
//! - **Optional-method presence**: TS checks `component.handleInput` /
//!   `typeof component[LAYOUT_NODE] === "function"` for method presence.
//!   Rust default methods are always "present", so presence is modeled
//!   explicitly by [`Component::handles_input`] (default `false`) and
//!   [`Component::layout_node`] (default `None`).
//! - **Mouse numerics are `i64`** (JS number semantics): the TS
//!   `Container.handleMouse` checks `event.y < 0`, so coordinates cannot be
//!   `usize`.
//! - **Result-flag fields are `Option<bool>`**: TS optional booleans
//!   (`handled?: boolean`) are `undefined`-or-`true`/`false`; helpers like
//!   [`TuiMouseEventResult::is_handled`] treat `None` as falsy, exactly as
//!   JS does.

use std::cell::RefCell;
use std::rc::Rc;

use crate::layout_node::LayoutNode;

// =============================================================================
// Mouse event types (tui.ts:23-79)
// =============================================================================

/// `TuiMouseEventType` — "press" | "release" | "move" | "drag" | "click" |
/// "wheel".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiMouseEventType {
    Press,
    Release,
    Move,
    Drag,
    Click,
    Wheel,
}

impl TuiMouseEventType {
    /// Serialization name matching the TS string literal.
    pub fn as_str(&self) -> &'static str {
        match self {
            TuiMouseEventType::Press => "press",
            TuiMouseEventType::Release => "release",
            TuiMouseEventType::Move => "move",
            TuiMouseEventType::Drag => "drag",
            TuiMouseEventType::Click => "click",
            TuiMouseEventType::Wheel => "wheel",
        }
    }
}

/// `TuiMouseButton` — "left" | "middle" | "right" | "none".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiMouseButton {
    Left,
    Middle,
    Right,
    None,
}

impl TuiMouseButton {
    /// Serialization name matching the TS string literal.
    pub fn as_str(&self) -> &'static str {
        match self {
            TuiMouseButton::Left => "left",
            TuiMouseButton::Middle => "middle",
            TuiMouseButton::Right => "right",
            TuiMouseButton::None => "none",
        }
    }
}

/// Normalized cell-based mouse event. Coordinates are zero-based (tui.ts:29).
#[derive(Debug, Clone, PartialEq)]
pub struct TuiMouseEvent {
    pub event_type: TuiMouseEventType,
    pub button: TuiMouseButton,
    /// Coordinates local to the receiving component.
    pub x: i64,
    pub y: i64,
    /// Absolute terminal coordinates.
    pub screen_x: i64,
    pub screen_y: i64,
    /// Current component bounds.
    pub width: i64,
    pub height: i64,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    /// Logical lines. Negative values scroll up.
    pub wheel_delta: Option<i64>,
    /// Consecutive click count when type is click.
    pub click_count: Option<i64>,
}

/// Component-level mouse result (tui.ts:46-58). All flags optional; `None`
/// models the TS `undefined`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TuiMouseEventResult {
    /// Stop propagation and suppress renderer-level fallback behavior.
    pub handled: Option<bool>,
    /// Route subsequent drag/release events to this component. Implies handled.
    pub capture: Option<bool>,
    /// Give keyboard focus to this component. Implies handled.
    pub focus: Option<bool>,
    /// Explicitly request or suppress a render. Move and release default to
    /// false; press, click, drag, and wheel default to true.
    pub render: Option<bool>,
}

impl TuiMouseEventResult {
    /// `Some(TuiMouseEventResult { handled: Some(true), ..Default::default() })`
    pub fn handled() -> Self {
        TuiMouseEventResult {
            handled: Some(true),
            ..Default::default()
        }
    }

    /// `Some(TuiMouseEventResult { capture: Some(true), ..Default::default() })`
    pub fn capture() -> Self {
        TuiMouseEventResult {
            capture: Some(true),
            ..Default::default()
        }
    }

    /// `Some(TuiMouseEventResult { focus: Some(true), ..Default::default() })`
    pub fn focus() -> Self {
        TuiMouseEventResult {
            focus: Some(true),
            ..Default::default()
        }
    }

    /// JS truthiness of the `handled` flag.
    pub fn is_handled(&self) -> bool {
        self.handled.unwrap_or(false)
    }

    /// JS truthiness of the `capture` flag.
    pub fn is_capture(&self) -> bool {
        self.capture.unwrap_or(false)
    }

    /// JS truthiness of the `focus` flag.
    pub fn is_focus(&self) -> bool {
        self.focus.unwrap_or(false)
    }
}

/// The union a component's `handleMouse` can return: plain flags, or a
/// forwarded [`TuiMouseDispatchResult`] (the TS `dispatchMouseEvent`
/// distinguishes them with `"target" in result`).
#[derive(Debug, Clone)]
pub enum ComponentMouseResult {
    /// `TuiMouseEventResult`
    Result(TuiMouseEventResult),
    /// The component forwarded the event to a child it hosts.
    Forwarded(TuiMouseDispatchResult),
}

/// Internal target metadata used by containers and alternate-screen
/// dispatch (tui.ts:61-67).
#[derive(Clone)]
pub struct TuiMouseDispatchTarget {
    pub component: ComponentRef,
    pub origin_x: i64,
    pub origin_y: i64,
    pub width: i64,
    pub height: i64,
}

impl std::fmt::Debug for TuiMouseDispatchTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TuiMouseDispatchTarget")
            .field("origin_x", &self.origin_x)
            .field("origin_y", &self.origin_y)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// Result of dispatching to a concrete component (tui.ts:70-78). `handled`
/// is always `true` on values produced by [`dispatch_mouse_event`].
#[derive(Clone)]
pub struct TuiMouseDispatchResult {
    pub handled: bool,
    pub capture: Option<bool>,
    pub focus: Option<bool>,
    pub render: Option<bool>,
    pub target: TuiMouseDispatchTarget,
    /// Keyboard focus target, which may be a delegating parent container.
    pub focus_target: Option<ComponentRef>,
}

impl TuiMouseDispatchResult {
    /// JS truthiness of the `focus` flag.
    pub fn is_focus(&self) -> bool {
        self.focus.unwrap_or(false)
    }

    /// JS truthiness of the `capture` flag.
    pub fn is_capture(&self) -> bool {
        self.capture.unwrap_or(false)
    }
}

impl std::fmt::Debug for TuiMouseDispatchResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TuiMouseDispatchResult")
            .field("handled", &self.handled)
            .field("capture", &self.capture)
            .field("focus", &self.focus)
            .field("render", &self.render)
            .field("target", &self.target)
            .field("has_focus_target", &self.focus_target.is_some())
            .finish()
    }
}

/// Dispatch an event to a component and retain the exact target and
/// coordinate transform. Containers use this when forwarding events to
/// nested children (tui.ts:81-105).
pub fn dispatch_mouse_event(
    component: &ComponentRef,
    event: &TuiMouseEvent,
) -> Option<TuiMouseDispatchResult> {
    let result = component
        .borrow_mut()
        .handle_mouse(component, event);
    let result = match result? {
        ComponentMouseResult::Forwarded(forwarded) => {
            // The component forwarded the event to a child it hosts. Like a
            // delegating container, it routes keys to that child itself, so
            // it keeps keyboard focus. Focusing the child directly would
            // leave focus on a detached component once the host removes it,
            // e.g. a closed settings submenu.
            if forwarded.is_focus() && component.borrow().handles_input() {
                let mut with_target = forwarded;
                with_target.focus_target = Some(component.clone());
                return Some(with_target);
            }
            return Some(forwarded);
        }
        ComponentMouseResult::Result(result) => result,
    };
    if !result.is_handled() && !result.is_capture() && !result.is_focus() {
        return None;
    }
    Some(TuiMouseDispatchResult {
        handled: true,
        capture: result.capture,
        focus: result.focus,
        render: result.render,
        focus_target: if result.is_focus() {
            Some(component.clone())
        } else {
            None
        },
        target: TuiMouseDispatchTarget {
            component: component.clone(),
            origin_x: event.screen_x - event.x,
            origin_y: event.screen_y - event.y,
            width: event.width,
            height: event.height,
        },
    })
}

/// Recreate local coordinates for a previously dispatched mouse target
/// (tui.ts:107-117).
pub fn retarget_mouse_event(event: &TuiMouseEvent, target: &TuiMouseDispatchTarget) -> TuiMouseEvent {
    let mut retargeted = event.clone();
    retargeted.x = event.screen_x - target.origin_x;
    retargeted.y = event.screen_y - target.origin_y;
    retargeted.width = target.width;
    retargeted.height = target.height;
    retargeted
}

// =============================================================================
// Component (tui.ts:117-143)
// =============================================================================

/// A shared, mutable component reference (the TS `Component` object
/// reference).
pub type ComponentRef = Rc<RefCell<dyn Component>>;

/// Component interface — all components must implement this (tui.ts:117).
///
/// The TS optional members map as follows:
/// - `handleInput?(data)` → [`handle_input`] (default no-op) +
///   [`handles_input`] (presence, default `false`)
/// - `handleMouse?(event)` → [`handle_mouse`] (default `None`)
/// - `wantsKeyRelease?` → [`wants_key_release`] (default `false`)
/// - `invalidate()` is required in TS but many components leave it to
///   `Container`; the Rust default is a no-op
/// - `[LAYOUT_NODE]()` → [`layout_node`] (default `None`)
pub trait Component {
    /// Render the component to lines for the given viewport width.
    fn render(&mut self, width: usize) -> Vec<String>;

    /// Optional handler for keyboard input when component has focus.
    fn handle_input(&mut self, _data: &str) {}

    /// TS checks `component.handleInput` for method *presence*; Rust default
    /// methods are always present, so components implementing
    /// [`handle_input`] override this to return `true`.
    fn handles_input(&self) -> bool {
        false
    }

    /// Optional normalized mouse handler. `self_ref` is the shared reference
    /// to this component (the TS `this`) — needed because forwarding
    /// containers address themselves as focus targets.
    fn handle_mouse(
        &mut self,
        _self_ref: &ComponentRef,
        _event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        None
    }

    /// If true, component receives key release events (Kitty protocol).
    /// Default is false - release events are filtered out.
    fn wants_key_release(&self) -> bool {
        false
    }

    /// Invalidate any cached rendering state. Called when theme changes or
    /// when component needs to re-render from scratch.
    fn invalidate(&mut self) {}

    /// The `[LAYOUT_NODE]` capability (layout-node.ts): components
    /// participating in the flex layout solver return their layout node.
    /// The returned node owns cloned entries; `ComponentRef` identity is
    /// preserved.
    fn layout_node(&mut self) -> Option<LayoutNode> {
        None
    }

    /// The TS `Focusable` type guard `"focused" in component`
    /// (tui.ts:186-188). Components implementing [`Focusable`] override
    /// this to expose their focus state; the T5 renderer's `setFocus`
    /// toggles `focused` through it. Default `None` = "not focusable".
    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        None
    }

    /// TS `root instanceof Container` (tui.ts:695): expose the `Container`
    /// child list (cloned) for the overlay focus machinery's
    /// `containsComponent` walk. Only [`Container`] (and T5 screens, which
    /// own one) return `Some`. Default `None` = "not a Container".
    ///
    /// Returns a cloned `Vec` because screens own their `Container` behind
    /// an `Rc<RefCell<...>>` (the alt screen's implicit document shares it),
    /// so a borrowed slice cannot outlive `&self`.
    fn container_children(&self) -> Option<Vec<ComponentRef>> {
        None
    }
}

// =============================================================================
// TUI host abstraction (tui.ts `TUI` interface — editor.ts usage surface)
// =============================================================================

/// The slice of the TS `TUI` interface that components consume before the
/// T5 renderer lands. `Editor` (editor.ts:402,414,537,1963,2389,2408,2413)
/// uses exactly two members: `requestRender()` and `terminal.rows`.
///
/// `TuiMainScreen` implements this when the renderer is ported (T5); tests
/// use the `FakeTui` double in `test_support`.
pub trait TuiHost {
    /// TS `tui.requestRender()` — schedule a re-render of the screen.
    fn request_render(&mut self);

    /// TS `tui.terminal.rows` — current terminal height in rows.
    fn terminal_rows(&self) -> usize;
}

/// `Focusable` (tui.ts:189-197): components that can receive focus and
/// display a hardware cursor. When focused, the component emits
/// `CURSOR_MARKER` at the cursor position in its render output; the TUI
/// finds the marker and positions the hardware cursor there.
///
/// The TS type guard is `"focused" in component`; the Rust bridge is
/// [`Component::as_focusable`] (T5): `is_focusable(component)` ⇔
/// `component.as_focusable().is_some()`, and `setFocus` toggles the flag
/// through it.
pub trait Focusable {    /// Set by TUI when focus changes. Component should emit CURSOR_MARKER
    /// when true.
    fn focused(&self) -> bool;
    fn set_focused(&mut self, focused: bool);
}

// =============================================================================
// Cursor markers + overlay compositing (tui.ts:194-207, 419-459)
// =============================================================================

/// Emitted by focused components at the cursor position. The TUI finds and
/// strips this marker, then positions the hardware cursor there
/// (tui.ts:196).
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

/// Fake cursor markers - zero-width APC sequences around the text drawn as
/// the cursor. See `resolveFakeCursors()` (T5).
pub const FAKE_CURSOR_START: &str = "\x1b_pi:fc\x07";
pub const FAKE_CURSOR_END: &str = "\x1b_pi:/fc\x07";

/// Wrap text in fake cursor markers (tui.ts:205).
pub fn render_fake_cursor(text: &str) -> String {
    format!("{FAKE_CURSOR_START}{text}{FAKE_CURSOR_END}")
}

/// Port of `isImageLine` (terminal-image.ts:207-214). Promoted to
/// [`crate::terminal_image`] in T5; re-exported here for the existing
/// call sites.
pub use crate::terminal_image::is_image_line;

/// `SEGMENT_RESET` (tui.ts:421).
const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

/// Composite overlay content into a terminal line at a fixed column
/// (tui.ts:428-459).
pub fn composite_tui_line(
    base_line: &str,
    overlay_line: &str,
    start_col: usize,
    overlay_width: usize,
    total_width: usize,
) -> String {
    if is_image_line(base_line) {
        return base_line.to_string();
    }

    let after_start = start_col + overlay_width;
    let base = crate::utils::extract_segments(
        base_line,
        start_col,
        after_start,
        total_width.saturating_sub(after_start),
        true,
    );
    let overlay = crate::utils::slice_with_width(overlay_line, 0, overlay_width, true);
    let before_pad = start_col.saturating_sub(base.before_width);
    let overlay_pad = overlay_width.saturating_sub(overlay.width);
    let actual_before_width = start_col.max(base.before_width);
    let actual_overlay_width = overlay_width.max(overlay.width);
    // `Math.max(0, totalWidth - actualBeforeWidth - actualOverlayWidth)`
    let after_target = total_width.saturating_sub(actual_before_width + actual_overlay_width);
    let after_pad = after_target.saturating_sub(base.after_width);
    let result = format!(
        "{}{}{}{}{}{}{}{}",
        base.before,
        " ".repeat(before_pad),
        SEGMENT_RESET,
        overlay.text,
        " ".repeat(overlay_pad),
        SEGMENT_RESET,
        base.after,
        " ".repeat(after_pad),
    );

    if crate::utils::visible_width(&result) <= total_width {
        result
    } else {
        crate::utils::slice_by_column(&result, 0, total_width, true)
    }
}

// =============================================================================
// Container (tui.ts:358-418)
// =============================================================================/// A component that contains other components (tui.ts:358).
///
/// Renders children stacked vertically; forwards mouse events to the child
/// under the cursor using the render-height layout captured by the last
/// [`Container::render`].
#[derive(Default)]
pub struct Container {
    pub children: Vec<ComponentRef>,
    mouse_layout: Option<ContainerMouseLayout>,
}

#[derive(Clone)]
struct ContainerMouseLayout {
    width: usize,
    children: Vec<(ComponentRef, i64)>,
}

impl Container {
    pub fn new() -> Self {
        Container::default()
    }

    pub fn add_child(&mut self, component: ComponentRef) {
        self.children.push(component);
    }

    pub fn remove_child(&mut self, component: &ComponentRef) {
        if let Some(index) = self.children.iter().position(|c| Rc::ptr_eq(c, component)) {
            self.children.remove(index);
        }
    }

    pub fn clear(&mut self) {
        self.children.clear();
    }

    /// The cached mouse-layout child list (component + rendered height) for
    /// the given width, rebuilding it when the width changed since the last
    /// render (tui.ts:386-388).
    fn mouse_children(&mut self, width: usize) -> Vec<(ComponentRef, i64)> {
        if let Some(layout) = &self.mouse_layout {
            if layout.width == width {
                return layout.children.clone();
            }
        }
        let children: Vec<(ComponentRef, i64)> = self
            .children
            .iter()
            .map(|component| {
                let height = component.borrow_mut().render(width).len() as i64;
                (component.clone(), height)
            })
            .collect();
        self.mouse_layout = Some(ContainerMouseLayout { width, children: children.clone() });
        children
    }
}

impl Component for Container {
    /// A `Container` exposes its children
    /// (tui.ts:695 `instanceof Container`).
    fn container_children(&self) -> Option<Vec<ComponentRef>> {
        Some(self.children.clone())
    }

    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut mouse_children: Vec<(ComponentRef, i64)> = Vec::new();
        for child in &self.children {
            let child_lines = child.borrow_mut().render(width);
            mouse_children.push((child.clone(), child_lines.len() as i64));
            lines.extend(child_lines);
        }
        self.mouse_layout = Some(ContainerMouseLayout {
            width,
            children: mouse_children,
        });
        lines
    }

    // TS `Container` defines NO `handleInput` (tui.ts:358-418), so
    // `handles_input()` stays `false` (trait default): a plain Container
    // never claims keyboard focus — `focus_target` remains the child that
    // requested it (tui.ts:397's `(this as Component).handleInput` is
    // always `undefined` here).

    fn invalidate(&mut self) {
        for child in &self.children {
            child.borrow_mut().invalidate();
        }
    }

    fn handle_mouse(
        &mut self,
        self_ref: &ComponentRef,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        if event.y < 0 || event.y >= event.height {
            return None;
        }
        let mouse_children = self.mouse_children(event.width.max(0) as usize);
        let mut child_y: i64 = 0;
        for (child, child_height) in mouse_children {
            if event.y >= child_y && event.y < child_y + child_height {
                let mut local = event.clone();
                local.y = event.y - child_y;
                local.height = child_height;
                let result = dispatch_mouse_event(&child, &local);
                return match result {
                    // `if (result?.focus && (this as Component).handleInput)`
                    // → `{ ...result, focusTarget: this }` (tui.ts:397). For a
                    // plain Container `handleInput` is undefined, so the
                    // retarget never fires and the child's focus target is
                    // kept; a Container subclass that handles input opts in
                    // via `handles_input()`.
                    Some(dispatch) if dispatch.is_focus() && self.handles_input() => {
                        let mut with_target = dispatch;
                        with_target.focus_target = Some(self_ref.clone());
                        Some(ComponentMouseResult::Forwarded(with_target))
                    }
                    Some(dispatch) => Some(ComponentMouseResult::Forwarded(dispatch)),
                    None => None,
                };
            }
            child_y += child_height;
        }
        None
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// Minimal leaf component for routing tests.
    struct Leaf {
        lines: Vec<String>,
        input_count: Cell<usize>,
    }

    impl Component for Leaf {
        fn render(&mut self, _width: usize) -> Vec<String> {
            self.lines.clone()
        }

        fn handle_input(&mut self, _data: &str) {
            self.input_count.set(self.input_count.get() + 1);
        }

        fn handles_input(&self) -> bool {
            true
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
    }

    fn leaf(lines: &[&str]) -> ComponentRef {
        Rc::new(RefCell::new(Leaf {
            lines: lines.iter().map(|s| s.to_string()).collect(),
            input_count: Cell::new(0),
        }))
    }

    fn event(y: i64, height: i64) -> TuiMouseEvent {
        TuiMouseEvent {
            event_type: TuiMouseEventType::Press,
            button: TuiMouseButton::Left,
            x: 0,
            y,
            screen_x: 5,
            screen_y: y + 10,
            width: 20,
            height,
            shift: false,
            alt: false,
            ctrl: false,
            wheel_delta: None,
            click_count: None,
        }
    }

    #[test]
    fn container_renders_children_stacked_and_captures_layout() {
        let mut container = Container::new();
        container.add_child(leaf(&["a1", "a2"]));
        container.add_child(leaf(&["b1"]));
        let lines = container.render(10);
        assert_eq!(lines, vec!["a1", "a2", "b1"]);
        // mouse layout cached at width 10: heights 2 and 1
        let children = container.mouse_children(10);
        assert_eq!(children[0].1, 2);
        assert_eq!(children[1].1, 1);
    }

    #[test]
    fn container_routes_mouse_to_child_under_cursor() {
        let mut container = Container::new();
        container.add_child(leaf(&["a1", "a2"]));
        container.add_child(leaf(&["b1"]));
        container.render(10);
        let container_ref: ComponentRef = Rc::new(RefCell::new(container));
        // y=2 lands on the second child (childY 0..2 first, 2..3 second)
        let result = container_ref
            .borrow_mut()
            .handle_mouse(&container_ref, &event(2, 3));
        match result {
            Some(ComponentMouseResult::Forwarded(dispatch)) => {
                assert!(dispatch.is_capture());
                // Child-local event: y=2-childY(2)=0, screen_y=12 → origin 12.
                assert_eq!(dispatch.target.origin_y, 12);
            }
            other => panic!("expected forwarded dispatch, got none: {}", other.is_none()),
        }
        // y=4 is past every child
        assert!(container_ref
            .borrow_mut()
            .handle_mouse(&container_ref, &event(4, 5))
            .is_none());
        // negative y is rejected before routing (tui.ts:384)
        assert!(container_ref
            .borrow_mut()
            .handle_mouse(&container_ref, &event(-1, 5))
            .is_none());
    }

    /// TS `Container` has no `handleInput` (tui.ts:358-418), so a child that
    /// requests focus keeps it — the container never retargets to itself
    /// (tui.ts:397 `(this as Component).handleInput` is undefined).
    /// Regression test for the T3 redteam F1 finding.
    #[test]
    fn container_does_not_steal_focus_from_focusable_child() {
        struct FocusLeaf;

        impl Component for FocusLeaf {
            fn render(&mut self, _width: usize) -> Vec<String> {
                vec!["leaf".to_string()]
            }
            fn handle_mouse(
                &mut self,
                _self_ref: &ComponentRef,
                _event: &TuiMouseEvent,
            ) -> Option<ComponentMouseResult> {
                let mut result = TuiMouseEventResult::capture();
                result.focus = Some(true);
                Some(ComponentMouseResult::Result(result))
            }
        }

        let child: ComponentRef = Rc::new(RefCell::new(FocusLeaf));
        let mut container = Container::new();
        container.add_child(child.clone());
        let container_ref: ComponentRef = Rc::new(RefCell::new(container));

        let result = container_ref
            .borrow_mut()
            .handle_mouse(&container_ref, &event(0, 1));
        match result {
            Some(ComponentMouseResult::Forwarded(dispatch)) => {
                assert!(dispatch.is_focus());
                let target = dispatch.focus_target.as_ref().expect("focus target");
                assert!(
                    Rc::ptr_eq(target, &child),
                    "focus target must be the child, not the container"
                );
            }
            other => panic!("expected forwarded dispatch, got none: {}", other.is_none()),
        }
    }

    #[test]
    fn container_mouse_layout_rebuilds_on_width_change() {
        let mut container = Container::new();
        container.add_child(leaf(&["a1"]));
        container.render(10);
        let before = container.mouse_children(10);
        // Different width: children re-rendered (fresh layout captured).
        let after = container.mouse_children(12);
        assert_eq!(after[0].1, 1);
        assert!(Rc::ptr_eq(&before[0].0, &after[0].0));
    }

    #[test]
    fn dispatch_requires_a_truthy_flag() {
        let target = leaf(&["x"]);
        // Plain flags with every field None: falsy → None (tui.ts:91).
        let none_result = dispatch_mouse_event(
            &target,
            &TuiMouseEvent {
                event_type: TuiMouseEventType::Move,
                button: TuiMouseButton::None,
                x: 0,
                y: 1,
                screen_x: 0,
                screen_y: 1,
                width: 1,
                height: 1,
                shift: false,
                alt: false,
                ctrl: false,
                wheel_delta: None,
                click_count: None,
            },
        );
        assert!(none_result.is_none());
    }

    #[test]
    fn dispatch_sets_target_and_origin_from_screen_coordinates() {
        let target = leaf(&["x"]);
        let result = dispatch_mouse_event(&target, &event(0, 1)).unwrap();
        assert!(result.handled);
        assert!(result.is_capture());
        // origin = screen - local (tui.ts:99-100)
        assert_eq!(result.target.origin_x, 5);
        assert_eq!(result.target.origin_y, 10);
        // capture without focus → no focus_target
        assert!(result.focus_target.is_none());
    }

    #[test]
    fn retarget_recomputes_local_coordinates() {
        let target = leaf(&["x"]);
        let dispatch = dispatch_mouse_event(&target, &event(0, 1)).unwrap();
        let original = event(0, 1);
        let retargeted = retarget_mouse_event(&original, &dispatch.target);
        assert_eq!(retargeted.x, original.screen_x - dispatch.target.origin_x);
        assert_eq!(retargeted.y, original.screen_y - dispatch.target.origin_y);
        assert_eq!(retargeted.width, dispatch.target.width);
        assert_eq!(retargeted.height, dispatch.target.height);
    }

    #[test]
    fn remove_child_uses_reference_identity() {
        let mut container = Container::new();
        let a = leaf(&["a"]);
        let b = leaf(&["b"]);
        container.add_child(a.clone());
        container.add_child(b.clone());
        // An equal-looking but distinct component is not removed.
        let impostor = leaf(&["a"]);
        container.remove_child(&impostor);
        assert_eq!(container.children.len(), 2);
        container.remove_child(&a);
        assert_eq!(container.children.len(), 1);
        assert!(Rc::ptr_eq(&container.children[0], &b));
        container.clear();
        assert!(container.children.is_empty());
    }
}
