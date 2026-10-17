//! Port of `packages/tui/src/components/h-stack.ts` — `HStack`, the
//! horizontal stack container.

use crate::component::{composite_tui_line, Component, ComponentMouseResult, ComponentRef, TuiMouseEvent};
use crate::layout_node::{LayoutNode, LayoutViewport, StackAlign, StackLayoutEntry, StackType};
use crate::utils::visible_width;

use crate::components::stack::{
    allocate_stack_sizes, visible_stack_entries, Stack, StackChild, StackEntryOptions,
    StackOptions,
};

/// `HStack` (h-stack.ts:9-50): renders the visible entries side by side,
/// compositing each child's lines into shared output rows with
/// `composite_tui_line` at its allocated column. Column widths come from
/// `allocateStackSizes` against the viewport width; rows are aligned
/// `start`/`center`/`end` (`stretch` behaves as `start`).
pub struct HStack {
    pub(crate) stack: Stack,
}

impl HStack {
    /// `new HStack(children = [], options = {})` (h-stack.ts:14-16).
    pub fn new(children: Vec<StackChild>, options: StackOptions) -> Self {
        HStack {
            stack: Stack::new(children, options, StackType::HStack),
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

    pub fn align(&self) -> StackAlign {
        self.stack.align()
    }
}

impl Component for HStack {
    /// `render(width)` (h-stack.ts:18-50).
    fn render(&mut self, width: usize) -> Vec<String> {
        // TS: const safeWidth = Math.max(1, width);
        let safe_width = (width as i64).max(1) as usize;
        let viewport = LayoutViewport {
            width: safe_width as i64,
            height: crate::components::stack::MAX_SAFE_INTEGER,
        };
        let entries = visible_stack_entries(&self.stack.entries, &viewport);
        if entries.is_empty() {
            return Vec::new();
        }

        // Intrinsic width of each entry: widest rendered line at full width.
        let intrinsic_widths: Vec<i64> = entries
            .iter()
            .map(|entry| {
                let lines = entry.component.borrow_mut().render(safe_width);
                lines
                    .iter()
                    .map(|line| visible_width(line) as i64)
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let widths = allocate_stack_sizes(
            &entries,
            &intrinsic_widths,
            Some(safe_width as i64),
            self.stack.gap,
        );

        // Re-render each child at its allocated width (width 0 → no lines).
        let rendered: Vec<Vec<String>> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                if widths[index] == 0 {
                    Vec::new()
                } else {
                    entry.component.borrow_mut().render(widths[index] as usize)
                }
            })
            .collect();

        let height = rendered.iter().map(|lines| lines.len()).max().unwrap_or(0);
        let mut result: Vec<String> = vec![String::new(); height];
        let mut x: i64 = 0;
        for index in 0..rendered.len() {
            let lines = &rendered[index];
            let child_width = widths[index];
            let mut offset: i64 = 0;
            match self.stack.align {
                // TS: Math.floor((height - lines.length) / 2)
                // Math.floor((height - lines.length) / 2) (h-stack.ts:40) —
                // floor semantics via div_euclid.
                StackAlign::Center => {
                    offset = (height as i64 - lines.len() as i64).div_euclid(2)
                }
                StackAlign::End => offset = height as i64 - lines.len() as i64,
                StackAlign::Stretch | StackAlign::Start => {}
            }
            for (row, line) in lines.iter().enumerate() {
                let target = row as i64 + offset;
                if target < 0 || target >= height as i64 {
                    continue;
                }
                result[target as usize] = composite_tui_line(
                    &result[target as usize],
                    line,
                    x as usize,
                    child_width as usize,
                    safe_width,
                );
            }
            x += child_width + self.stack.gap;
        }
        result
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
    use crate::layout_node::Basis;
    use crate::utils::strip_terminal_sequences;

    /// Test-only leaf: renders fixed lines regardless of width.
    struct Leaf {
        lines: Vec<String>,
    }

    impl Component for Leaf {
        fn render(&mut self, _width: usize) -> Vec<String> {
            self.lines.clone()
        }
    }

    fn leaf(lines: &[&str]) -> ComponentRef {
        Rc::new(RefCell::new(Leaf {
            lines: lines.iter().map(|s| s.to_string()).collect(),
        }))
    }

    fn plain(lines: Vec<&str>) -> Vec<StackChild> {
        lines
            .into_iter()
            .map(|l| StackChild::Component(leaf(&[l])))
            .collect()
    }

    #[test]
    fn h_stack_empty_renders_no_lines() {
        let mut hstack = HStack::new(Vec::new(), StackOptions::default());
        assert!(hstack.render(10).is_empty());
    }

    #[test]
    fn h_stack_hidden_entries_are_excluded() {
        let mut hstack = HStack::new(Vec::new(), StackOptions::default());
        hstack.add_child_with(
            leaf(&["hidden"]),
            StackEntryOptions {
                visible: Some(Rc::new(|viewport: &LayoutViewport| viewport.width >= 10)),
                ..Default::default()
            },
        );
        hstack.add_child(leaf(&["shown"]));
        // Only the visible entry renders.
        assert_eq!(strip_terminal_sequences(&hstack.render(5)[0]), "shown");
        // At width 10 the intrinsic widths 6+5 = 11 exceed the content size,
        // so `shrink ?? 1` shrinks the first entry by 1 → widths [5, 5] and
        // the first child is sliced to "hidde".
        let lines = hstack.render(10);
        assert_eq!(lines.len(), 1);
        assert_eq!(strip_terminal_sequences(&lines[0]), "hiddeshown");
    }

    #[test]
    fn h_stack_composites_children_horizontally_with_gap() {
        let mut hstack = HStack::new(
            plain(vec!["ab", "c"]),
            StackOptions {
                gap: Some(1.0),
                align: None,
            },
        );
        let lines = hstack.render(10);
        assert_eq!(lines.len(), 1);
        // "ab" at column 0 (width 2), gap column, "c" at column 3, padded
        // to the viewport width.
        assert_eq!(strip_terminal_sequences(&lines[0]), "ab c      ");
        assert_eq!(visible_width(&lines[0]), 10);
    }

    #[test]
    fn h_stack_grow_fills_available_width() {
        let mut hstack = HStack::new(Vec::new(), StackOptions::default());
        hstack.add_child_with(
            leaf(&["a"]),
            StackEntryOptions {
                grow: Some(1.0),
                ..Default::default()
            },
        );
        hstack.add_child_with(
            leaf(&["b"]),
            StackEntryOptions {
                grow: Some(1.0),
                ..Default::default()
            },
        );
        let lines = hstack.render(10);
        assert_eq!(lines.len(), 1);
        // Intrinsic widths 1/1, amount 8: pass 1 gives 4 then 2 (remaining
        // shrinks mid-pass), pass 2 gives 1 and 1 → widths [6, 4].
        assert_eq!(strip_terminal_sequences(&lines[0]), "a     b   ");
        assert_eq!(visible_width(&lines[0]), 10);
    }

    #[test]
    fn h_stack_shrinks_to_available_width() {
        let mut hstack = HStack::new(Vec::new(), StackOptions::default());
        hstack.add_child_with(
            leaf(&["aaaa"]),
            StackEntryOptions {
                shrink: Some(1.0),
                ..Default::default()
            },
        );
        hstack.add_child_with(
            leaf(&["bbbb"]),
            StackEntryOptions {
                shrink: Some(1.0),
                ..Default::default()
            },
        );
        // Intrinsic 4/4 = 8, available 6 → shrink 2 weighted by size →
        // [3, 3].
        let lines = hstack.render(6);
        assert_eq!(lines.len(), 1);
        let stripped = strip_terminal_sequences(&lines[0]);
        // First child rendered at width 3 keeps "aaa"; second starts at
        // column 3.
        assert!(stripped.starts_with("aaab"));
        assert_eq!(visible_width(&lines[0]), 6);
    }

    #[test]
    fn h_stack_zero_width_entry_renders_no_lines() {
        let mut hstack = HStack::new(Vec::new(), StackOptions::default());
        hstack.add_child_with(
            leaf(&["a"]),
            StackEntryOptions {
                basis: Some(Basis::Points(0.0)),
                ..Default::default()
            },
        );
        // widths[0] == 0 → rendered as no lines → height 0 → no output.
        assert!(hstack.render(10).is_empty());
    }

    #[test]
    fn h_stack_align_end_offsets_short_children_down() {
        let mut hstack = HStack::new(
            vec![
                StackChild::Component(leaf(&["a"])),
                StackChild::Component(leaf(&["b1", "b2"])),
            ],
            StackOptions {
                gap: None,
                align: Some(StackAlign::End),
            },
        );
        let lines = hstack.render(10);
        assert_eq!(lines.len(), 2);
        let row0 = strip_terminal_sequences(&lines[0]);
        let row1 = strip_terminal_sequences(&lines[1]);
        // The 1-line child is pushed to the bottom row; the 2-line child
        // starts at column 1 (after the width-1 first child).
        assert!(row0.starts_with(" b1"));
        assert!(row1.starts_with("ab2"));
    }

    #[test]
    fn h_stack_align_center_offsets_short_children() {
        let mut hstack = HStack::new(
            vec![
                StackChild::Component(leaf(&["a"])),
                StackChild::Component(leaf(&["b1", "b2", "b3"])),
            ],
            StackOptions {
                gap: None,
                align: Some(StackAlign::Center),
            },
        );
        let lines = hstack.render(10);
        assert_eq!(lines.len(), 3);
        // floor((3 - 1) / 2) = 1 → the single-line child lands on row 1.
        assert!(strip_terminal_sequences(&lines[0]).starts_with(" b1"));
        assert!(strip_terminal_sequences(&lines[1]).starts_with("ab2"));
        assert!(strip_terminal_sequences(&lines[2]).starts_with(" b3"));
    }

    #[test]
    fn h_stack_width_zero_clamps_to_one() {
        let mut hstack = HStack::new(plain(vec!["ab"]), StackOptions::default());
        let lines = hstack.render(0);
        assert_eq!(lines.len(), 1);
        assert_eq!(visible_width(&lines[0]), 1);
    }

    #[test]
    fn h_stack_exposes_layout_node_capability() {
        let a = leaf(&["a"]);
        let mut hstack = HStack::new(
            vec![StackChild::Component(a.clone())],
            StackOptions {
                gap: Some(1.0),
                align: Some(StackAlign::Center),
            },
        );
        let LayoutNode::Stack(node) = hstack.layout_node().unwrap() else {
            panic!("expected stack layout node");
        };
        assert_eq!(node.layout_type, StackType::HStack);
        assert_eq!(node.gap, 1);
        assert_eq!(node.align, StackAlign::Center);
        assert!(Rc::ptr_eq(&node.entries[0].component, &a));
    }

    #[test]
    fn h_stack_remove_child_updates_entries_and_render() {
        let a = leaf(&["a"]);
        let b = leaf(&["b"]);
        let mut hstack = HStack::new(Vec::new(), StackOptions::default());
        hstack.add_child(a.clone());
        hstack.add_child(b);
        assert_eq!(hstack.render(10).len(), 1);
        hstack.remove_child(&a);
        assert_eq!(hstack.entries().len(), 1);
        assert!(strip_terminal_sequences(&hstack.render(10)[0]).starts_with("b"));
        hstack.clear();
        assert!(hstack.render(10).is_empty());
    }
}
