//! Port of `packages/tui/src/components/v-stack.ts` — `VStack`, the
//! vertical stack container.

use crate::component::{Component, ComponentMouseResult, ComponentRef, TuiMouseEvent};
use crate::layout_node::{LayoutNode, LayoutViewport, StackLayoutEntry, StackType};

use crate::components::stack::{allocate_stack_sizes, visible_stack_entries, Stack};
// TS re-export (v-stack.ts:38): `export type { StackChild, StackEntry,
// StackEntryOptions, StackOptions } from "./stack.ts"`.
pub use crate::components::stack::{StackChild, StackEntryOptions, StackOptions};

/// `VStack` (v-stack.ts:10-36): renders the visible entries vertically,
/// separated by `gap` rows, each child padded/truncated to its allocated
/// size (with `availableSize` undefined, sizes are the clamped intrinsic
/// line counts or bases).
pub struct VStack {
    pub(crate) stack: Stack,
}

impl VStack {
    /// `new VStack(children = [], options = {})` (v-stack.ts:14-16).
    pub fn new(children: Vec<StackChild>, options: StackOptions) -> Self {
        VStack {
            stack: Stack::new(children, options, StackType::VStack),
        }
    }

    /// `addChild(component)` with default options.
    pub fn add_child(&mut self, component: ComponentRef) {
        self.stack.add_child(component);
    }

    /// `addChild(component, options)`.
    pub fn add_child_with(&mut self, component: ComponentRef, options: StackEntryOptions) {
        self.stack.add_child_with(component, options);
    }

    /// `removeChild(component)` (identity-based).
    pub fn remove_child(&mut self, component: &ComponentRef) {
        self.stack.remove_child(component);
    }

    /// `clear()`.
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    /// The layout entries.
    pub fn entries(&self) -> &[StackLayoutEntry] {
        self.stack.entries()
    }

    pub fn gap(&self) -> i64 {
        self.stack.gap()
    }
}

impl Component for VStack {
    /// `render(width)` (v-stack.ts:18-36).
    fn render(&mut self, width: usize) -> Vec<String> {
        // TS: viewport = { width: Math.max(1, width), height: MAX_SAFE }
        let viewport = LayoutViewport {
            width: (width as i64).max(1),
            height: crate::components::stack::MAX_SAFE_INTEGER,
        };
        let entries = visible_stack_entries(&self.stack.entries, &viewport);
        let rendered: Vec<Vec<String>> = entries
            .iter()
            .map(|entry| entry.component.borrow_mut().render(viewport.width as usize))
            .collect();
        let intrinsic_heights: Vec<i64> = rendered.iter().map(|lines| lines.len() as i64).collect();
        let sizes = allocate_stack_sizes(&entries, &intrinsic_heights, None, self.stack.gap);

        let mut lines: Vec<String> = Vec::new();
        for index in 0..entries.len() {
            if index > 0 {
                for _ in 0..self.stack.gap.max(0) {
                    lines.push(String::new());
                }
            }
            // TS: rendered[index].slice(0, sizes[index]) — truncate to the
            // allocated size.
            let child_lines = &rendered[index];
            let allocated = sizes[index].max(0) as usize;
            let take = allocated.min(child_lines.len());
            lines.extend(child_lines[..take].iter().cloned());
            // Pad up to the allocated size (e.g. minSize above the intrinsic
            // line count).
            for _ in child_lines.len()..allocated {
                lines.push(String::new());
            }
        }
        lines
    }

    fn handles_input(&self) -> bool {
        self.stack.handles_input()
    }

    fn invalidate(&mut self) {
        self.stack.invalidate();
    }

    fn handle_mouse(
        &mut self,
        self_ref: &ComponentRef,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        self.stack.handle_mouse(self_ref, event)
    }

    /// `[LAYOUT_NODE]()` — inherited from `Stack` (stack.ts:75-78).
    fn layout_node(&mut self) -> Option<LayoutNode> {
        Some(self.stack.layout_node())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::component::{
        ComponentMouseResult, TuiMouseButton, TuiMouseEvent, TuiMouseEventResult,
        TuiMouseEventType,
    };
    use crate::layout_node::StackAlign;

    /// Test-only leaf: renders fixed lines regardless of width; captures
    /// mouse events at local y == 0.
    struct Leaf {
        lines: Vec<String>,
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
    }

    fn leaf(lines: &[&str]) -> ComponentRef {
        Rc::new(RefCell::new(Leaf {
            lines: lines.iter().map(|s| s.to_string()).collect(),
        }))
    }

    fn event(y: i64, height: i64) -> TuiMouseEvent {
        TuiMouseEvent {
            event_type: TuiMouseEventType::Press,
            button: TuiMouseButton::Left,
            x: 0,
            y,
            screen_x: 3,
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
    fn v_stack_renders_children_vertically_with_gap() {
        let mut vstack = VStack::new(
            vec![
                StackChild::Component(leaf(&["a"])),
                StackChild::Component(leaf(&["b1", "b2"])),
            ],
            StackOptions {
                gap: Some(1.0),
                align: None,
            },
        );
        let lines = vstack.render(10);
        assert_eq!(lines, vec!["a", "", "b1", "b2"]);
    }

    #[test]
    fn v_stack_renders_without_gap_by_default() {
        let mut vstack = VStack::new(
            vec![StackChild::Component(leaf(&["a", "a2"])), StackChild::Component(leaf(&["b"]))],
            StackOptions::default(),
        );
        let lines = vstack.render(10);
        assert_eq!(lines, vec!["a", "a2", "b"]);
    }

    #[test]
    fn v_stack_pads_to_allocated_size() {
        // availableSize is undefined → sizes are clamped intrinsic heights;
        // a minSize above the line count pads with empty lines.
        let mut vstack = VStack::new(Vec::new(), StackOptions::default());
        vstack.add_child_with(
            leaf(&["a"]),
            StackEntryOptions {
                min_size: Some(3.0),
                ..Default::default()
            },
        );
        let lines = vstack.render(10);
        assert_eq!(lines, vec!["a", "", ""]);
    }

    #[test]
    fn v_stack_truncates_to_allocated_size() {
        // A fixed basis below the intrinsic line count truncates the child.
        let mut vstack = VStack::new(Vec::new(), StackOptions::default());
        vstack.add_child_with(
            leaf(&["a", "b", "c"]),
            StackEntryOptions {
                basis: Some(crate::layout_node::Basis::Points(2.0)),
                ..Default::default()
            },
        );
        let lines = vstack.render(10);
        assert_eq!(lines, vec!["a", "b"]);
    }

    #[test]
    fn v_stack_filters_entries_by_visibility() {
        let mut vstack = VStack::new(Vec::new(), StackOptions::default());
        vstack.add_child_with(
            leaf(&["wide"]),
            StackEntryOptions {
                visible: Some(Rc::new(|viewport: &LayoutViewport| viewport.width >= 10)),
                ..Default::default()
            },
        );
        vstack.add_child(leaf(&["always"]));
        assert_eq!(vstack.render(5), vec!["always"]);
        assert_eq!(vstack.render(10), vec!["wide", "always"]);
    }

    #[test]
    fn v_stack_width_zero_clamps_viewport_to_one() {
        let mut vstack = VStack::new(
            vec![StackChild::Component(leaf(&["a"]))],
            StackOptions::default(),
        );
        // The viewport width floors at 1 (TS Math.max(1, width)).
        assert_eq!(vstack.render(0), vec!["a"]);
    }

    #[test]
    fn v_stack_empty_renders_no_lines() {
        let mut vstack = VStack::new(Vec::new(), StackOptions::default());
        assert!(vstack.render(10).is_empty());
    }

    #[test]
    fn v_stack_mouse_routing_delegates_to_container() {
        // TS Stack inherits Container.handleMouse: children are stacked by
        // their render heights (gap rows are not part of the mouse layout —
        // the upstream quirk is preserved).
        let mut vstack = VStack::new(
            vec![
                StackChild::Component(leaf(&["a1", "a2"])),
                StackChild::Component(leaf(&["b"])),
            ],
            StackOptions::default(),
        );
        vstack.render(10);
        let vstack_ref: ComponentRef = Rc::new(RefCell::new(vstack));
        // y=2 lands on the second child (heights 2 then 1), local y=0.
        let result = vstack_ref
            .borrow_mut()
            .handle_mouse(&vstack_ref, &event(2, 3));
        match result {
            Some(ComponentMouseResult::Forwarded(dispatch)) => {
                assert!(dispatch.is_capture());
                // origin = screen_y(12) - child-local y(0) = 12.
                assert_eq!(dispatch.target.origin_y, 12);
            }
            _ => panic!("expected forwarded dispatch"),
        }
        // Past every child → None.
        assert!(vstack_ref
            .borrow_mut()
            .handle_mouse(&vstack_ref, &event(3, 4))
            .is_none());
    }

    #[test]
    fn v_stack_exposes_layout_node_capability() {
        let a = leaf(&["a"]);
        let mut vstack = VStack::new(
            vec![StackChild::Component(a.clone())],
            StackOptions {
                gap: Some(2.0),
                align: Some(StackAlign::Start),
            },
        );
        let LayoutNode::Stack(node) = vstack.layout_node().unwrap() else {
            panic!("expected stack layout node");
        };
        assert_eq!(node.layout_type, StackType::VStack);
        assert_eq!(node.gap, 2);
        assert_eq!(node.align, StackAlign::Start);
        assert!(Rc::ptr_eq(&node.entries[0].component, &a));
    }

    #[test]
    fn v_stack_remove_child_updates_entries_and_render() {
        let a = leaf(&["a"]);
        let b = leaf(&["b"]);
        let mut vstack = VStack::new(Vec::new(), StackOptions::default());
        vstack.add_child(a.clone());
        vstack.add_child(b);
        assert_eq!(vstack.render(10), vec!["a", "b"]);
        vstack.remove_child(&a);
        assert_eq!(vstack.entries().len(), 1);
        assert_eq!(vstack.render(10), vec!["b"]);
        vstack.clear();
        assert!(vstack.render(10).is_empty());
    }
}
