//! Port of `packages/tui/src/components/box.ts` — `Box`: a container that
//! applies padding and an optional background to all children and forwards
//! mouse events to the child under the cursor.
//!
//! Divergences from TS:
//! - `bgFn?: (text: string) => string` is `Option<Box<dyn Fn(&str) -> String>>`
//!   (reusing [`crate::components::text::CustomBgFn`]).
//! - The upstream `flattenLines(result)` call is omitted (TS no-op).
//! - Children are [`ComponentRef`]s (shared mutable references) per the
//!   crate-wide port convention.
//! - The TS cache identity trick ("children usually return the same string
//!   objects every frame, so the cache check is a cheap identity comparison")
//!   becomes a value comparison of the child lines — semantically identical.

use std::rc::Rc;

use crate::component::{
    dispatch_mouse_event, Component, ComponentMouseResult, ComponentRef, TuiMouseEvent,
};
use crate::components::text::CustomBgFn;
use crate::utils::visible_width;

/// `RenderCache` (box.ts:8-13).
struct RenderCache {
    child_lines: Vec<String>,
    width: usize,
    bg_sample: Option<String>,
    lines: Vec<String>,
}

/// Child list entry for mouse routing: component + rendered height
/// (box.ts:15, inlined shape `{ component, height }`).
type MouseChild = (ComponentRef, i64);

/// Port of `Box` (box.ts:17-172) — a container that applies padding and
/// background to all children.
///
/// Named `TuiBox` to avoid colliding with `std::box::Box` (which the
/// component's own API uses for `Box<dyn Fn>` background callbacks).
pub struct TuiBox {
    /// `children: Component[] = []` (box.ts:19) — public field.
    pub children: Vec<ComponentRef>,
    padding_x: usize,
    padding_y: usize,
    bg_fn: Option<CustomBgFn>,

    // Cache for rendered output (box.ts:22-23).
    cache: Option<RenderCache>,
    mouse_layout: Option<(usize, Vec<MouseChild>)>,
}

impl TuiBox {
    /// `constructor(paddingX = 1, paddingY = 1, bgFn?)` (box.ts:25-29).
    pub fn new(padding_x: usize, padding_y: usize, bg_fn: Option<CustomBgFn>) -> Self {
        TuiBox {
            children: Vec::new(),
            padding_x,
            padding_y,
            bg_fn,
            cache: None,
            mouse_layout: None,
        }
    }

    /// `addChild(component)` (box.ts:31-34).
    pub fn add_child(&mut self, component: ComponentRef) {
        self.children.push(component);
        self.invalidate_cache();
    }

    /// `removeChild(component)` (box.ts:36-41) — reference identity via
    /// `Rc::ptr_eq`.
    pub fn remove_child(&mut self, component: &ComponentRef) {
        if let Some(index) = self.children.iter().position(|c| Rc::ptr_eq(c, component)) {
            self.children.remove(index);
            self.invalidate_cache();
        }
    }

    /// `clear()` (box.ts:43-46).
    pub fn clear(&mut self) {
        self.children.clear();
        self.invalidate_cache();
    }

    /// `setBgFn(bgFn?)` (box.ts:48-50) — deliberately does NOT invalidate;
    /// bgFn changes are detected by sampling output.
    pub fn set_bg_fn(&mut self, bg_fn: Option<CustomBgFn>) {
        self.bg_fn = bg_fn;
    }

    /// `setPaddingX(paddingX)` (box.ts:52-55).
    pub fn set_padding_x(&mut self, padding_x: usize) {
        self.padding_x = padding_x;
        self.invalidate_cache();
    }

    /// `invalidateCache()` (box.ts:57-59).
    fn invalidate_cache(&mut self) {
        self.cache = None;
    }

    /// `matchCache(width, childLines, bgSample)` (box.ts:61-69).
    fn match_cache(&self, width: usize, child_lines: &[String], bg_sample: Option<&str>) -> bool {
        match &self.cache {
            Some(cache) => {
                cache.width == width
                    && cache.bg_sample.as_deref() == bg_sample
                    && cache.child_lines.len() == child_lines.len()
                    && cache
                        .child_lines
                        .iter()
                        .zip(child_lines.iter())
                        .all(|(a, b)| a == b)
            }
            None => false,
        }
    }

    /// `applyBg(line, width)` (box.ts:164-171) — pad to width, then apply
    /// the background function.
    fn apply_bg(line: &str, width: usize, bg_fn: &Option<CustomBgFn>) -> String {
        let vis_len = visible_width(line);
        let pad_needed = width.saturating_sub(vis_len);
        let padded = format!("{line}{}", " ".repeat(pad_needed));
        // Already padded to width, so apply the background directly instead
        // of measuring the line again.
        match bg_fn {
            Some(bg_fn) => bg_fn(&padded),
            None => padded,
        }
    }
}

impl Component for TuiBox {
    /// `render(width)` (box.ts:90-160).
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.children.is_empty() {
            return Vec::new();
        }

        // `Math.max(1, width - this.paddingX * 2)` — i64 math so narrow
        // widths clamp through exactly like JS.
        let content_width =
            ((width as i64) - 2 * (self.padding_x as i64)).max(1) as usize;
        let left_pad = " ".repeat(self.padding_x);

        // Render all children. Keep the child lines unpadded: children
        // usually return the same strings every frame, so the cache check
        // below is cheap (box.ts:99-110).
        let mut child_lines: Vec<String> = Vec::new();
        let mut mouse_children: Vec<MouseChild> = Vec::new();
        for child in &self.children {
            let lines = child.borrow_mut().render(content_width);
            mouse_children.push((child.clone(), lines.len() as i64));
            child_lines.extend(lines);
        }
        self.mouse_layout = Some((content_width, mouse_children));

        if child_lines.is_empty() {
            return Vec::new();
        }

        // Check if bgFn output changed by sampling (box.ts:115).
        let bg_sample = self.bg_fn.as_ref().map(|bg_fn| bg_fn("test"));

        // Check cache validity (box.ts:117-120).
        if self.match_cache(width, &child_lines, bg_sample.as_deref()) {
            // `return this.cache!.lines`
            return self.cache.as_ref().expect("match_cache implies cache").lines.clone();
        }

        // Apply background and padding (box.ts:122-144).
        let mut result: Vec<String> = Vec::new();

        // Top padding
        for _ in 0..self.padding_y {
            result.push(Self::apply_bg("", width, &self.bg_fn));
        }

        // Content
        for line in &child_lines {
            result.push(Self::apply_bg(&format!("{left_pad}{line}"), width, &self.bg_fn));
        }

        // Bottom padding
        for _ in 0..self.padding_y {
            result.push(Self::apply_bg("", width, &self.bg_fn));
        }

        // Update cache (box.ts:147-149). `flattenLines` is a TS no-op.
        self.cache = Some(RenderCache {
            child_lines,
            width,
            bg_sample,
            lines: result.clone(),
        });

        result
    }

    /// `invalidate()` (box.ts:84-88) — clears the render cache and
    /// invalidates all children.
    fn invalidate(&mut self) {
        self.invalidate_cache();
        for child in &self.children {
            child.borrow_mut().invalidate();
        }
    }

    /// `handleMouse(event)` (box.ts:71-82).
    fn handle_mouse(
        &mut self,
        _self_ref: &ComponentRef,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        let content_width_i = (event.width - 2 * (self.padding_x as i64)).max(1);
        let content_y = event.y - self.padding_y as i64;
        let content_x = event.x - self.padding_x as i64;
        if content_y < 0 || content_x < 0 || content_x >= content_width_i {
            return None;
        }

        let mouse_children: Vec<MouseChild> = match &self.mouse_layout {
            Some((layout_width, children)) if *layout_width == content_width_i as usize => {
                children.clone()
            }
            // The freshly built list is NOT stored (box.ts:77) — only
            // `render` updates `mouseLayout`.
            _ => self
                .children
                .iter()
                .map(|component| {
                    let height =
                        component.borrow_mut().render(content_width_i as usize).len() as i64;
                    (component.clone(), height)
                })
                .collect(),
        };
        let mut child_y: i64 = 0;
        for (child, child_height) in mouse_children {
            if content_y >= child_y && content_y < child_y + child_height {
                let mut local = event.clone();
                local.x = content_x;
                local.y = content_y - child_y;
                local.width = content_width_i;
                local.height = child_height;
                return dispatch_mouse_event(&child, &local)
                    .map(ComponentMouseResult::Forwarded);
            }
            child_y += child_height;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::TuiMouseEventResult;
    use crate::components::text::Text;
    use std::cell::Cell;
    use std::cell::RefCell;

    /// Leaf component that records renders, invalidations, and mouse hits.
    struct Probe {
        render_count: Cell<usize>,
        invalidate_count: Cell<usize>,
        mouse_y: Cell<i64>,
    }

    impl Probe {
        fn new() -> Rc<RefCell<Probe>> {
            Rc::new(RefCell::new(Probe {
                render_count: Cell::new(0),
                invalidate_count: Cell::new(0),
                mouse_y: Cell::new(-1000),
            }))
        }
    }

    impl Component for Probe {
        fn render(&mut self, _width: usize) -> Vec<String> {
            self.render_count.set(self.render_count.get() + 1);
            vec!["probe".to_string()]
        }

        fn invalidate(&mut self) {
            self.invalidate_count.set(self.invalidate_count.get() + 1);
        }

        fn handle_mouse(
            &mut self,
            _self_ref: &ComponentRef,
            event: &TuiMouseEvent,
        ) -> Option<ComponentMouseResult> {
            self.mouse_y.set(event.y);
            Some(ComponentMouseResult::Result(TuiMouseEventResult::handled()))
        }
    }

    fn probe_ref(probe: &Rc<RefCell<Probe>>) -> ComponentRef {
        probe.clone()
    }

    fn event(x: i64, y: i64, width: i64, height: i64) -> TuiMouseEvent {
        TuiMouseEvent {
            event_type: crate::component::TuiMouseEventType::Press,
            button: crate::component::TuiMouseButton::Left,
            x,
            y,
            screen_x: x + 100,
            screen_y: y + 100,
            width,
            height,
            shift: false,
            alt: false,
            ctrl: false,
            wheel_delta: None,
            click_count: None,
        }
    }

    // Upstream has no dedicated box test file (mouse-components.test.ts
    // covers the editor); faithful tests per the port plan.

    #[test]
    fn renders_children_with_padding_and_background() {
        let mut b = TuiBox::new(1, 1, None);
        let child: ComponentRef = Rc::new(RefCell::new(Text::new("hi", 0, 0, None)));
        b.add_child(child);
        let lines = b.render(10);
        // top pad + content + bottom pad
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], " ".repeat(10));
        assert_eq!(lines[2], " ".repeat(10));
        // left pad + "hi" (child pads its own content to 8) + one more
        // space padding the 9-wide line to 10.
        assert_eq!(lines[1], " hi       ");
        assert_eq!(visible_width(&lines[1]), 10);
    }

    #[test]
    fn applies_background_function_to_all_lines() {
        let mut b = TuiBox::new(1, 0, Some(Box::new(|t: &str| format!("[{t}]") as String)));
        let child: ComponentRef = Rc::new(RefCell::new(Text::new("hi", 0, 0, None)));
        b.add_child(child);
        let lines = b.render(10);
        assert_eq!(lines.len(), 1);
        // " hi      " padded to 10, then wrapped by bgFn
        assert_eq!(lines[0], "[ hi       ]");
    }

    #[test]
    fn returns_empty_when_no_children() {
        let mut b = TuiBox::new(1, 1, None);
        assert!(b.render(10).is_empty());
    }

    #[test]
    fn returns_empty_when_children_render_nothing() {
        let empty: ComponentRef = Rc::new(RefCell::new(Text::new("", 0, 0, None)));
        let mut b = TuiBox::new(1, 1, None);
        b.add_child(empty);
        assert!(b.render(10).is_empty());
    }

    #[test]
    fn reuses_cache_when_children_and_sample_unchanged() {
        let probe = Probe::new();
        let mut b = TuiBox::new(1, 1, None);
        b.add_child(probe_ref(&probe));
        let bg_calls = Rc::new(Cell::new(0));
        let counter = bg_calls.clone();
        b.set_bg_fn(Some(Box::new(move |t: &str| {
            counter.set(counter.get() + 1);
            format!("bg({t})")
        })));
        let first = b.render(10);
        // 1 sample + 3 applyBg calls on the cache miss.
        assert_eq!(bg_calls.get(), 4);
        let second = b.render(10);
        assert_eq!(first, second);
        // Cache hit: only the bg sample ran ("test"); children are still
        // re-rendered every Box render (box.ts:103 renders before the cache
        // check), but no applyBg calls happened.
        assert_eq!(bg_calls.get(), 5);
        assert_eq!(probe.borrow().render_count.get(), 2);
    }

    #[test]
    fn detects_bg_fn_change_via_sample() {
        let mut b = TuiBox::new(0, 0, Some(Box::new(|t: &str| format!("one({t})") as String)));
        let child: ComponentRef = Rc::new(RefCell::new(Text::new("hi", 0, 0, None)));
        b.add_child(child);
        let first = b.render(10);
        assert_eq!(first, vec!["one(hi        )".to_string()]);
        // setBgFn does not invalidate, but the sample differs → re-render.
        b.set_bg_fn(Some(Box::new(|t: &str| format!("two({t})") as String)));
        let second = b.render(10);
        assert_eq!(second, vec!["two(hi        )".to_string()]);
    }

    #[test]
    fn child_content_change_bypasses_cache() {
        let child = Rc::new(RefCell::new(Text::new("one", 0, 0, None)));
        let mut b = TuiBox::new(0, 0, None);
        b.add_child(child.clone());
        assert_eq!(b.render(10), vec!["one       ".to_string()]);
        child.borrow_mut().set_text("two");
        assert_eq!(b.render(10), vec!["two       ".to_string()]);
    }

    #[test]
    fn set_padding_x_invalidates_cache() {
        let child: ComponentRef = Rc::new(RefCell::new(Text::new("hi", 0, 0, None)));
        let mut b = TuiBox::new(0, 0, None);
        b.add_child(child);
        assert_eq!(b.render(10), vec!["hi        ".to_string()]);
        b.set_padding_x(3);
        assert_eq!(b.render(10), vec!["   hi     ".to_string()]);
    }

    #[test]
    fn invalidate_clears_cache_and_invalidates_children() {
        let probe = Probe::new();
        let mut b = TuiBox::new(1, 1, None);
        b.add_child(probe_ref(&probe));
        b.render(10);
        assert_eq!(probe.borrow().invalidate_count.get(), 0);
        b.invalidate();
        assert_eq!(probe.borrow().invalidate_count.get(), 1);
        // Still renders correctly after invalidation.
        assert_eq!(b.render(10).len(), 3);
    }

    #[test]
    fn remove_child_uses_reference_identity() {
        let a: ComponentRef = Rc::new(RefCell::new(Text::new("a", 0, 0, None)));
        let b_child: ComponentRef = Rc::new(RefCell::new(Text::new("b", 0, 0, None)));
        let mut b = TuiBox::new(0, 0, None);
        b.add_child(a.clone());
        b.add_child(b_child.clone());
        // An equal-looking but distinct component is not removed.
        let impostor: ComponentRef = Rc::new(RefCell::new(Text::new("a", 0, 0, None)));
        b.remove_child(&impostor);
        assert_eq!(b.children.len(), 2);
        b.remove_child(&a);
        assert_eq!(b.children.len(), 1);
        assert!(Rc::ptr_eq(&b.children[0], &b_child));
        b.clear();
        assert!(b.children.is_empty());
        assert!(b.render(10).is_empty());
    }

    #[test]
    fn forwards_mouse_to_child_under_cursor() {
        let probe = Probe::new();
        let mut b = TuiBox::new(1, 1, None);
        b.add_child(probe_ref(&probe));
        b.render(10);
        let b_ref: ComponentRef = Rc::new(RefCell::new(b));
        // Event lands inside the content area: y=1 skips top padding,
        // x=2 skips left padding. width=10 → contentWidth=8.
        let result = b_ref.borrow_mut().handle_mouse(&b_ref, &event(2, 1, 10, 3));
        match result {
            Some(ComponentMouseResult::Forwarded(dispatch)) => {
                assert!(dispatch.handled);
                // Local event forwarded to the child: x = contentX = 1,
                // y = contentY - childY = 0, width = contentWidth = 8.
                // origin = screen - local of the child-local event:
                // (102 - 1, 101 - 0).
                assert_eq!(dispatch.target.origin_x, 101);
                assert_eq!(dispatch.target.origin_y, 101);
                assert_eq!(dispatch.target.width, 8);
                assert_eq!(dispatch.target.height, 1);
            }
            _ => panic!("expected forwarded dispatch"),
        }
        // Child received child-local coordinates.
        assert_eq!(probe.borrow().mouse_y.get(), 0);
    }

    #[test]
    fn mouse_in_padding_or_outside_content_returns_none() {
        let probe = Probe::new();
        let mut b = TuiBox::new(1, 1, None);
        b.add_child(probe_ref(&probe));
        b.render(10);
        let b_ref: ComponentRef = Rc::new(RefCell::new(b));
        // y=0 is inside the top padding (contentY = -1).
        assert!(b_ref.borrow_mut().handle_mouse(&b_ref, &event(2, 0, 10, 3)).is_none());
        // x=0 is inside the left padding (contentX = -1).
        assert!(b_ref.borrow_mut().handle_mouse(&b_ref, &event(0, 1, 10, 3)).is_none());
        // y=2 is past the single content line.
        assert!(b_ref.borrow_mut().handle_mouse(&b_ref, &event(2, 2, 10, 3)).is_none());
        assert_eq!(probe.borrow().mouse_y.get(), -1000);
    }
}
