//! Port of `packages/tui/src/components/mouse-region.ts` — `MouseRegion`,
//! a wrapper that adds mouse handling to an existing component without
//! changing its rendering.
//!
//! ## Modeling notes
//!
//! - TS `MouseRegion implements Component` (it does **not** extend
//!   `Container`): it defines only `render`, `handleMouse`, and
//!   `invalidate`. Optional TS members it leaves undefined (`handleInput`,
//!   `[LAYOUT_NODE]`, `wantsKeyRelease`) map to the Rust defaults
//!   (`handles_input() == false`, `layout_node() == None`,
//!   `wants_key_release() == false`).
//! - The wrapped child is a shared [`ComponentRef`]; `render` and
//!   `invalidate` delegate to it exactly as TS does.
//! - `handleMouse` (mouse-region.ts:25-28) first dispatches to the child
//!   through [`dispatch_mouse_event`]; only when the child produces no
//!   dispatch result does the region's own handler run
//!   (`childResult ?? this.onMouse(event)` — the `??` fires on
//!   `undefined`, i.e. when the child returned nothing or an all-falsy
//!   result). The handler result is returned as-is (a plain
//!   `TuiMouseEventResult`), never wrapped in a dispatch result.

use crate::component::{
    dispatch_mouse_event, Component, ComponentMouseResult, ComponentRef, TuiMouseEvent,
    TuiMouseEventResult,
};

/// `MouseRegionHandler` (mouse-region.ts:9): receives the event only when
/// the wrapped child did not handle it. Returning `None` models the TS
/// `undefined` (event unhandled by the region as well).
pub type MouseRegionHandler = Box<dyn Fn(&TuiMouseEvent) -> Option<TuiMouseEventResult>>;

/// `MouseRegion` (mouse-region.ts:11-33) — adds mouse handling to an
/// existing component without changing its rendering.
pub struct MouseRegion {
    child: ComponentRef,
    on_mouse: MouseRegionHandler,
}

impl MouseRegion {
    /// `constructor(child, onMouse)` (mouse-region.ts:16-19).
    pub fn new(child: ComponentRef, on_mouse: MouseRegionHandler) -> Self {
        MouseRegion { child, on_mouse }
    }

    /// The wrapped component (TS `private readonly child`).
    pub fn child(&self) -> &ComponentRef {
        &self.child
    }
}

impl Component for MouseRegion {
    /// `render(width)` (mouse-region.ts:21-23) — the child's output,
    /// unchanged.
    fn render(&mut self, width: usize) -> Vec<String> {
        self.child.borrow_mut().render(width)
    }

    /// `handleMouse(event)` (mouse-region.ts:25-28): dispatch to the child
    /// first; fall back to the region's handler only when the child did
    /// not produce a dispatch result.
    fn handle_mouse(
        &mut self,
        _self_ref: &ComponentRef,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        let child_result = dispatch_mouse_event(&self.child, event);
        match child_result {
            Some(dispatch) => Some(ComponentMouseResult::Forwarded(dispatch)),
            None => (self.on_mouse)(event).map(ComponentMouseResult::Result),
        }
    }

    /// `invalidate()` (mouse-region.ts:30-32) — delegates to the child.
    fn invalidate(&mut self) {
        self.child.borrow_mut().invalidate();
    }
}

// =============================================================================
// Tests
// =============================================================================
//
// Upstream coverage: `test/mouse-components.test.ts` contains **no**
// MouseRegion tests (every test there exercises Input / SelectList /
// SettingsList / Editor / TuiAltScreen — see the report). The MouseRegion
// behaviors upstream are exercised in `test/tui-alt-screen.test.ts`
// ("applies runtime wheel line count updates", "dispatches clicks to
// nested mouse regions without breaking drag selection", "lets
// mouse-aware components consume wheel events before viewport scrolling"),
// which is renderer-driven (tranche T5). The tests below port those
// behaviors at the component level.

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use crate::component::{TuiMouseButton, TuiMouseEventType};

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
    fn adds_mouse_handling_without_changing_rendering() {
        // mouse-region.ts:11 — the child's render output passes through.
        let child_ref: ComponentRef = leaf(&["wheel target"]);
        let mut region = MouseRegion::new(
            child_ref.clone(),
            Box::new(|_| None),
        );
        assert_eq!(region.render(20), vec!["wheel target"]);
        assert!(Rc::ptr_eq(region.child(), &child_ref));
    }

    #[test]
    fn child_dispatch_takes_precedence_over_the_region_handler() {
        // mouse-region.ts:26-27 — `childResult ?? onMouse(event)`: the
        // child's dispatch result wins when the child handles the event.
        let child_ref: ComponentRef = leaf(&["a"]);
        let called = Rc::new(Cell::new(false));
        let called_flag = called.clone();
        let region_ref: ComponentRef = Rc::new(RefCell::new(MouseRegion::new(
            child_ref.clone(),
            Box::new(move |_| {
                called_flag.set(true);
                Some(TuiMouseEventResult::handled())
            }),
        )));
        let result = region_ref
            .borrow_mut()
            .handle_mouse(&region_ref, &mouse(TuiMouseEventType::Press, 0, 0, 10, 1));
        match result {
            Some(ComponentMouseResult::Forwarded(dispatch)) => {
                assert!(dispatch.handled);
                assert!(dispatch.is_capture());
                // The dispatch target is the child, not the region.
                assert!(Rc::ptr_eq(&dispatch.target.component, &child_ref));
            }
            _ => panic!("expected forwarded child dispatch"),
        }
        assert!(!called.get(), "region handler must not run when the child handles");
    }

    #[test]
    fn falls_back_to_the_region_handler_when_the_child_does_not_handle() {
        let child = leaf(&["a", "b"]);
        let region_ref: ComponentRef = Rc::new(RefCell::new(MouseRegion::new(
            child,
            Box::new(|event| {
                assert_eq!(event.y, 1);
                Some(TuiMouseEventResult::handled())
            }),
        )));
        // Child captures only at y == 0; y == 1 falls through to the
        // region handler, whose plain result is returned as-is.
        let result = region_ref
            .borrow_mut()
            .handle_mouse(&region_ref, &mouse(TuiMouseEventType::Press, 0, 1, 10, 2));
        match result {
            Some(ComponentMouseResult::Result(result)) => {
                assert!(result.is_handled());
                assert!(!result.is_capture());
            }
            _ => panic!("expected plain handler result"),
        }
    }

    #[test]
    fn returns_none_when_neither_child_nor_handler_handles() {
        let child = leaf(&["a", "b"]);
        let region_ref: ComponentRef = Rc::new(RefCell::new(MouseRegion::new(
            child,
            Box::new(|_| None),
        )));
        assert!(region_ref
            .borrow_mut()
            .handle_mouse(&region_ref, &mouse(TuiMouseEventType::Move, 0, 1, 10, 2))
            .is_none());
    }

    #[test]
    fn region_handler_consumes_wheel_events_before_viewport_scrolling() {
        // tui-alt-screen.test.ts "lets mouse-aware components consume wheel
        // events before viewport scrolling": a wheel event the handler
        // claims (`{ handled: true }`) suppresses renderer fallback.
        let child = leaf(&["line 1", "line 2", "line 3"]);
        let wheel_events = Rc::new(Cell::new(0));
        let counter = wheel_events.clone();
        let region_ref: ComponentRef = Rc::new(RefCell::new(MouseRegion::new(
            child,
            Box::new(move |event| {
                if event.event_type != TuiMouseEventType::Wheel {
                    return None;
                }
                counter.set(counter.get() + 1);
                Some(TuiMouseEventResult::handled())
            }),
        )));
        let mut event = mouse(TuiMouseEventType::Wheel, 1, 1, 20, 3);
        event.wheel_delta = Some(-3);
        match region_ref.borrow_mut().handle_mouse(&region_ref, &event) {
            Some(ComponentMouseResult::Result(result)) => assert!(result.is_handled()),
            _ => panic!("expected handled wheel result"),
        }
        assert_eq!(wheel_events.get(), 1);
    }

    #[test]
    fn region_handler_receives_the_wheel_delta() {
        // tui-alt-screen.test.ts "applies runtime wheel line count
        // updates": the handler observes `event.wheelDelta` (negative
        // scrolls up) before deciding.
        let child = leaf(&["wheel target"]);
        let deltas = Rc::new(RefCell::new(Vec::new()));
        let sink = deltas.clone();
        let region_ref: ComponentRef = Rc::new(RefCell::new(MouseRegion::new(
            child,
            Box::new(move |event| {
                if event.event_type != TuiMouseEventType::Wheel {
                    return None;
                }
                sink.borrow_mut().push(event.wheel_delta);
                Some(TuiMouseEventResult::handled())
            }),
        )));
        for delta in [-3, 2, -10] {
            let mut event = mouse(TuiMouseEventType::Wheel, 1, 1, 20, 1);
            event.wheel_delta = Some(delta);
            region_ref.borrow_mut().handle_mouse(&region_ref, &event);
        }
        assert_eq!(*deltas.borrow(), vec![Some(-3), Some(2), Some(-10)]);
    }

    #[test]
    fn dispatches_clicks_to_nested_mouse_regions() {
        // tui-alt-screen.test.ts "dispatches clicks to nested mouse regions
        // without breaking drag selection": the click portion — clicks
        // reach the region handler when the child stays passive.
        let child = leaf(&["clickable", "selectable"]);
        let clicks = Rc::new(Cell::new(0));
        let counter = clicks.clone();
        let region_ref: ComponentRef = Rc::new(RefCell::new(MouseRegion::new(
            child,
            Box::new(move |event| {
                if event.event_type != TuiMouseEventType::Click {
                    return None;
                }
                counter.set(counter.get() + 1);
                Some(TuiMouseEventResult::handled())
            }),
        )));
        let click = mouse(TuiMouseEventType::Click, 2, 1, 20, 2);
        assert_eq!(click.click_count, Some(1));
        match region_ref.borrow_mut().handle_mouse(&region_ref, &click) {
            Some(ComponentMouseResult::Result(result)) => assert!(result.is_handled()),
            _ => panic!("expected handled click result"),
        }
        assert_eq!(clicks.get(), 1);
    }

    #[test]
    fn invalidate_propagates_to_the_child() {
        // mouse-region.ts:30-32.
        let child = leaf(&["a"]);
        let mut region = MouseRegion::new(child.clone(), Box::new(|_| None));
        region.invalidate();
        assert!(child.borrow().invalidated.get());
    }
}
