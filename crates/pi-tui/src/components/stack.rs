//! Port of `packages/tui/src/components/stack.ts` — the abstract `Stack`
//! container shared by [`crate::components::v_stack::VStack`] and
//! [`crate::components::h_stack::HStack`], plus the exported layout helpers
//! [`visible_stack_entries`] and [`allocate_stack_sizes`] that the layout
//! solver (`layout.rs`) codes against.
//!
//! ## Modeling notes
//!
//! - TS `Stack` is an abstract class (`extends Container`, abstract
//!   `layoutType`); the Rust model is a concrete base struct the two stacks
//!   *compose*, parameterized by [`StackType`]. Container behavior
//!   (child list, mouse routing, invalidate) is delegated to an inner
//!   [`Container`].
//! - `StackChild = Component | StackEntry` (the TS type guard
//!   `!("render" in child)`) becomes an explicit [`StackChild`] enum.
//! - `Number.MAX_SAFE_INTEGER` is represented by [`MAX_SAFE_INTEGER`]
//!   (`i64::MAX / 2`): equally "unbounded" for terminal dimensions, while
//!   leaving headroom against overflow when sizes are summed or
//!   differenced.

use std::rc::Rc;

use crate::component::{
    Component, ComponentMouseResult, ComponentRef, Container, TuiMouseEvent,
};
use crate::layout_node::{
    Basis, LayoutNode, LayoutViewport, StackAlign, StackLayoutEntry, StackLayoutNode, StackType,
};

/// Stand-in for JS `Number.MAX_SAFE_INTEGER` in the integer size domain
/// (stack.ts uses the JS constant at lines 57, 106, 111, 120, 131, 145).
pub(crate) const MAX_SAFE_INTEGER: i64 = i64::MAX / 2;

// =============================================================================
// Options / child types (stack.ts:5-22)
// =============================================================================

/// `StackEntryOptions` (stack.ts:7-14). `None` fields model the TS
/// `undefined` (left unset on the pushed layout entry).
pub struct StackEntryOptions {
    /// `basis?: number | "auto"`.
    pub basis: Option<Basis>,
    pub grow: Option<f64>,
    pub shrink: Option<f64>,
    pub min_size: Option<f64>,
    pub max_size: Option<f64>,
    /// `(viewport: LayoutViewport) => boolean` — dynamic visibility.
    pub visible: Option<Rc<dyn Fn(&LayoutViewport) -> bool>>,
}

impl Default for StackEntryOptions {
    fn default() -> Self {
        StackEntryOptions {
            basis: None,
            grow: None,
            shrink: None,
            min_size: None,
            max_size: None,
            visible: None,
        }
    }
}

/// `StackChild = Component | StackEntry` (stack.ts:18-20). The TS type guard
/// `isStackEntry(child)` checks `!("render" in child)`; the Rust model makes
/// the union explicit.
pub enum StackChild {
    /// A plain component (no entry options).
    Component(ComponentRef),
    /// A `StackEntry`: component plus entry options.
    Entry {
        component: ComponentRef,
        options: StackEntryOptions,
    },
}

/// `StackOptions` (stack.ts:22-25).
#[derive(Default)]
pub struct StackOptions {
    pub gap: Option<f64>,
    pub align: Option<StackAlign>,
}

// =============================================================================
// normalizeSize (stack.ts:26-28)
// =============================================================================

/// `normalizeSize(value, fallback)`: non-finite → fallback, else
/// `max(0, floor(value))`.
fn normalize_size(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0).floor()
    } else {
        fallback
    }
}

// =============================================================================
// Stack (stack.ts:31-79)
// =============================================================================

/// `Stack` (stack.ts:31-79) — abstract base of `VStack`/`HStack`.
///
/// Holds the layout `entries` alongside the container child list (TS
/// `addChild` pushes both), the normalized `gap`, and the `align`. The
/// concrete stacks compose this struct and provide `render`.
pub struct Stack {
    pub(crate) container: Container,
    pub(crate) entries: Vec<StackLayoutEntry>,
    pub(crate) gap: i64,
    pub(crate) align: StackAlign,
    /// The TS `abstract readonly layoutType`.
    pub(crate) layout_type: StackType,
}

impl Stack {
    /// Shared constructor (stack.ts:33-45): normalizes gap/align and folds
    /// the mixed child list through [`Stack::add_child_with`].
    pub(crate) fn new(
        children: Vec<StackChild>,
        options: StackOptions,
        layout_type: StackType,
    ) -> Self {
        let mut stack = Stack {
            container: Container::new(),
            entries: Vec::new(),
            gap: normalize_size(options.gap.unwrap_or(0.0), 0.0) as i64,
            align: options.align.unwrap_or(StackAlign::Stretch),
            layout_type,
        };
        for child in children {
            match child {
                StackChild::Component(component) => {
                    stack.add_child_with(component, StackEntryOptions::default());
                }
                StackChild::Entry { component, options } => {
                    stack.add_child_with(component, options);
                }
            }
        }
        stack
    }

    /// `addChild(component, options = {})` (stack.ts:48-61): pushes the
    /// container child *and* the layout entry. Defined numeric options are
    /// normalized (`max(0, floor)`); `basis` and `visible` pass through
    /// as-is; undefined options stay unset (defaults applied later as
    /// `grow ?? 0`, `shrink ?? 1`, `minSize ?? 0`, `maxSize ?? MAX`).
    pub fn add_child_with(&mut self, component: ComponentRef, options: StackEntryOptions) {
        self.container.add_child(component.clone());
        self.entries.push(StackLayoutEntry {
            component,
            basis: options.basis,
            grow: options.grow.map(|v| normalize_size(v, 0.0)),
            shrink: options.shrink.map(|v| normalize_size(v, 1.0)),
            min_size: options.min_size.map(|v| normalize_size(v, 0.0)),
            max_size: options
                .max_size
                .map(|v| normalize_size(v, MAX_SAFE_INTEGER as f64)),
            visible: options.visible,
        });
    }

    /// `addChild(component)` — default (empty) options.
    pub fn add_child(&mut self, component: ComponentRef) {
        self.add_child_with(component, StackEntryOptions::default());
    }

    /// `removeChild(component)` (stack.ts:63-68): removes the container
    /// child and the first layout entry whose component is the same
    /// reference.
    pub fn remove_child(&mut self, component: &ComponentRef) {
        self.container.remove_child(component);
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| Rc::ptr_eq(&entry.component, component))
        {
            self.entries.remove(index);
        }
    }

    /// `clear()` (stack.ts:70-73).
    pub fn clear(&mut self) {
        self.container.clear();
        self.entries.clear();
    }

    /// The layout entries (TS `protected readonly entries`).
    pub fn entries(&self) -> &[StackLayoutEntry] {
        &self.entries
    }

    /// Normalized gap (`max(0, floor)`, TS stack.ts:39).
    pub fn gap(&self) -> i64 {
        self.gap
    }

    pub fn align(&self) -> StackAlign {
        self.align
    }

    /// `[LAYOUT_NODE]()` (stack.ts:75-78): returns the stack layout node
    /// with cloned entries — `ComponentRef` identity preserved.
    pub fn layout_node(&self) -> LayoutNode {
        LayoutNode::Stack(StackLayoutNode {
            layout_type: self.layout_type,
            entries: self.entries.clone(),
            gap: self.gap,
            align: self.align,
        })
    }

    /// Container delegation used by the concrete stacks'
    /// `Component::handle_mouse` impls (Stack inherits `Container`'s mouse
    /// routing in TS).
    pub(crate) fn handle_mouse(
        &mut self,
        self_ref: &ComponentRef,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        self.container.handle_mouse(self_ref, event)
    }

    /// Container delegation for `Component::invalidate`.
    pub(crate) fn invalidate(&mut self) {
        self.container.invalidate();
    }

    /// Container delegation for `Component::handles_input`.
    pub(crate) fn handles_input(&self) -> bool {
        self.container.handles_input()
    }
}

// =============================================================================
// visibleStackEntries (stack.ts:81-87)
// =============================================================================

/// `visibleStackEntries` (stack.ts:82-87): filters entries by their
/// `visible` closure; a missing closure defaults to visible.
pub fn visible_stack_entries(
    entries: &[StackLayoutEntry],
    viewport: &LayoutViewport,
) -> Vec<StackLayoutEntry> {
    entries
        .iter()
        .filter(|entry| entry.visible.as_ref().map_or(true, |visible| visible(viewport)))
        .cloned()
        .collect()
}

// =============================================================================
// clampSize / distribute (stack.ts:89-133)
// =============================================================================

/// `clampSize` (stack.ts:89-93): clamp to `[min, max]` after flooring,
/// with `min = max(0, floor(minSize ?? 0))` and
/// `max = max(min, floor(maxSize ?? MAX_SAFE_INTEGER))`.
fn clamp_size(size: i64, entry: &StackLayoutEntry) -> i64 {
    let min = entry.min_size.unwrap_or(0.0).floor().max(0.0) as i64;
    let max = (entry.max_size.unwrap_or(MAX_SAFE_INTEGER as f64).floor() as i64).max(min);
    size.max(0).min(max).max(min)
}

/// `entry.maxSize ?? MAX_SAFE_INTEGER`, unfloored (the raw comparison in
/// `distribute`, stack.ts:111/120/131).
fn raw_max_size(entry: &StackLayoutEntry) -> f64 {
    entry.max_size.unwrap_or(MAX_SAFE_INTEGER as f64)
}

/// `entry.minSize ?? 0`, unfloored (stack.ts:112/126/132).
fn raw_min_size(entry: &StackLayoutEntry) -> f64 {
    entry.min_size.unwrap_or(0.0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DistributeMode {
    Grow,
    Shrink,
}

/// `distribute` (stack.ts:95-133) — iterative proportional allocation.
///
/// Ported exactly, iteration by iteration:
/// 1. Collect candidates: grow mode wants `grow > 0` with room below
///    `maxSize`; shrink mode wants `shrink > 0` with room above `minSize`.
/// 2. Weights: grow → `grow ?? 0`; shrink → `(shrink ?? 1) * max(1, size)`.
/// 3. Each candidate proposes `max(1, floor(remaining * weight /
///    totalWeight))`, capped by remaining and by its capacity
///    (`maxSize - size` / `size - minSize`); apply, decrement `remaining`.
/// 4. Loop while `remaining > 0` and some progress (`distributed > 0`)
///    was made this pass.
///
/// Divergence: TS capacity arithmetic can carry fractions when a
/// hand-built entry has a non-integer `minSize`/`maxSize` (the JS `min`
/// then yields fractional sizes). This port floors capacities; entries
/// added through [`Stack::add_child_with`] always have integer bounds
/// because `normalizeSize` floors, so the difference is unobservable for
/// stack-built entries.
fn distribute(sizes: &mut [i64], entries: &[StackLayoutEntry], amount: i64, mode: DistributeMode) {
    let mut remaining = amount;
    while remaining > 0 {
        let candidates: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|&(index, entry)| match mode {
                DistributeMode::Grow => {
                    entry.grow.unwrap_or(0.0) > 0.0 && (sizes[index] as f64) < raw_max_size(entry)
                }
                DistributeMode::Shrink => {
                    entry.shrink.unwrap_or(1.0) > 0.0 && (sizes[index] as f64) > raw_min_size(entry)
                }
            })
            .map(|(index, _)| index)
            .collect();
        if candidates.is_empty() {
            return;
        }

        // Weight of a candidate (stack.ts:116-119, 123): grow → `grow ?? 0`;
        // shrink → `(shrink ?? 1) * max(1, size)`. A free function (not a
        // closure) so `sizes` can be mutated inside the loop.
        fn weight_of(
            mode: DistributeMode,
            entries: &[StackLayoutEntry],
            sizes: &[i64],
            index: usize,
        ) -> f64 {
            match mode {
                DistributeMode::Grow => entries[index].grow.unwrap_or(0.0),
                DistributeMode::Shrink => {
                    entries[index].shrink.unwrap_or(1.0) * (sizes[index].max(1) as f64)
                }
            }
        }
        let total_weight: f64 = candidates
            .iter()
            .map(|&index| weight_of(mode, entries, sizes, index))
            .sum();

        let mut distributed: i64 = 0;
        for &index in &candidates {
            if remaining <= 0 {
                break;
            }
            let proposed = ((remaining as f64 * weight_of(mode, entries, sizes, index))
                / total_weight)
                .floor()
                .max(1.0) as i64;
            let capacity = match mode {
                DistributeMode::Grow => {
                    (raw_max_size(&entries[index]) - sizes[index] as f64).floor() as i64
                }
                DistributeMode::Shrink => {
                    (sizes[index] as f64 - raw_min_size(&entries[index])).floor() as i64
                }
            };
            let delta = remaining.min(proposed).min(capacity);
            if delta <= 0 {
                continue;
            }
            match mode {
                DistributeMode::Grow => sizes[index] += delta,
                DistributeMode::Shrink => sizes[index] -= delta,
            }
            remaining -= delta;
            distributed += delta;
        }
        if distributed == 0 {
            return;
        }
    }
}

// =============================================================================
// allocateStackSizes (stack.ts:135-154)
// =============================================================================

/// `allocateStackSizes` (stack.ts:135-154): floor each entry's base size
/// (intrinsic size for missing/`auto` basis, else the basis) through
/// `clampSize`, then grow/shrink toward `availableSize` minus the total
/// gap. `available_size == None` returns the clamped base sizes unchanged.
pub fn allocate_stack_sizes(
    entries: &[StackLayoutEntry],
    intrinsic_sizes: &[i64],
    available_size: Option<i64>,
    gap: i64,
) -> Vec<i64> {
    let mut sizes: Vec<i64> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            // TS: basis undefined-or-auto → intrinsicSizes ?? 0; else the
            // (clamped, floored) basis number.
            let base = match entry.basis {
                None | Some(Basis::Auto) => intrinsic_sizes.get(index).copied().unwrap_or(0),
                Some(Basis::Points(points)) => points.floor().max(0.0) as i64,
            };
            clamp_size(base, entry)
        })
        .collect();

    let Some(available) = available_size else {
        return sizes;
    };

    // TS: Math.max(0, floor(availableSize) - Math.max(0, entries.length - 1) * gap)
    let content_size = (available - (entries.len() as i64 - 1).max(0) * gap).max(0);
    let total: i64 = sizes.iter().sum();
    if total < content_size {
        distribute(&mut sizes, entries, content_size - total, DistributeMode::Grow);
    } else if total > content_size {
        distribute(&mut sizes, entries, total - content_size, DistributeMode::Shrink);
    }
    sizes
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Minimal leaf component (test-only, local to this module).
    struct Fixed {
        lines: Vec<String>,
    }

    impl Component for Fixed {
        fn render(&mut self, _width: usize) -> Vec<String> {
            self.lines.clone()
        }
    }

    fn fixed(lines: &[&str]) -> ComponentRef {
        Rc::new(RefCell::new(Fixed {
            lines: lines.iter().map(|s| s.to_string()).collect(),
        }))
    }

    fn plain_entry(component: ComponentRef) -> StackLayoutEntry {
        StackLayoutEntry {
            component,
            basis: None,
            grow: None,
            shrink: None,
            min_size: None,
            max_size: None,
            visible: None,
        }
    }

    fn entry_with(
        component: ComponentRef,
        basis: Option<Basis>,
        grow: Option<f64>,
        shrink: Option<f64>,
        min_size: Option<f64>,
        max_size: Option<f64>,
    ) -> StackLayoutEntry {
        StackLayoutEntry {
            component,
            basis,
            grow,
            shrink,
            min_size,
            max_size,
            visible: None,
        }
    }

    // --- allocate_stack_sizes -------------------------------------------------

    #[test]
    fn allocate_without_available_returns_clamped_base_sizes() {
        // auto basis → intrinsic; fixed basis → basis; missing intrinsic → 0.
        let a = plain_entry(fixed(&["a"]));
        let b = entry_with(fixed(&["b"]), Some(Basis::Points(10.0)), None, None, None, None);
        let c = plain_entry(fixed(&["c"]));
        let sizes = allocate_stack_sizes(&[a, b, c], &[5, 99], None, 0);
        assert_eq!(sizes, vec![5, 10, 0]);
    }

    #[test]
    fn allocate_clamps_to_min_and_max() {
        let min_only = entry_with(fixed(&["a"]), None, None, None, Some(3.0), None);
        let max_only = entry_with(fixed(&["b"]), Some(Basis::Points(10.0)), None, None, None, Some(4.0));
        let sizes = allocate_stack_sizes(&[min_only, max_only], &[1, 0], None, 0);
        // intrinsic 1 clamped up to minSize 3; basis 10 clamped down to 4.
        assert_eq!(sizes, vec![3, 4]);
    }

    #[test]
    fn allocate_distributes_growth_by_weight() {
        // grow 1:3, bases 2/2, content 22 → amount 18. Note `remaining`
        // decreases *within* a pass, so later proposals in the same pass
        // use the smaller remainder (TS stack.ts:124).
        // Pass 1: floor(18*1/4)=4, then floor(14*3/4)=10 → [6,12], rem 4.
        // Pass 2: floor(4*1/4)=1, then floor(3*3/4)=2 → [7,14], rem 1.
        // Pass 3: proposal clamps to min 1 → first candidate takes 1 → [8,14].
        let a = entry_with(fixed(&["a"]), Some(Basis::Points(2.0)), Some(1.0), None, None, None);
        let b = entry_with(fixed(&["b"]), Some(Basis::Points(2.0)), Some(3.0), None, None, None);
        let sizes = allocate_stack_sizes(&[a, b], &[0, 0], Some(22), 0);
        assert_eq!(sizes, vec![8, 14]);
    }

    #[test]
    fn allocate_growth_respects_max_size() {
        // Both grow 1, first capped at maxSize 3: pass 1 gives [3,5],
        // pass 2 grows only the uncapped entry → [3,7].
        let a = entry_with(fixed(&["a"]), Some(Basis::Points(1.0)), Some(1.0), None, None, Some(3.0));
        let b = entry_with(fixed(&["b"]), Some(Basis::Points(1.0)), Some(1.0), None, None, None);
        let sizes = allocate_stack_sizes(&[a, b], &[0, 0], Some(10), 0);
        assert_eq!(sizes, vec![3, 7]);
    }

    #[test]
    fn allocate_shrinks_by_size_weighted_shrink() {
        // shrink ?? 1; weight = shrink * max(1, size). Bases 10/10, content 15.
        let a = entry_with(fixed(&["a"]), Some(Basis::Points(10.0)), None, Some(1.0), None, None);
        let b = entry_with(fixed(&["b"]), Some(Basis::Points(10.0)), None, Some(1.0), None, None);
        let sizes = allocate_stack_sizes(&[a, b], &[0, 0], Some(15), 0);
        assert_eq!(sizes, vec![7, 8]);
    }

    #[test]
    fn allocate_default_shrink_is_one() {
        // shrink undefined → 1: same distribution as the explicit case.
        let a = entry_with(fixed(&["a"]), Some(Basis::Points(10.0)), None, None, None, None);
        let b = entry_with(fixed(&["b"]), Some(Basis::Points(10.0)), None, None, None, None);
        let sizes = allocate_stack_sizes(&[a, b], &[0, 0], Some(15), 0);
        assert_eq!(sizes, vec![7, 8]);
    }

    #[test]
    fn allocate_default_grow_is_zero_so_surplus_is_ignored() {
        let a = entry_with(fixed(&["a"]), Some(Basis::Points(4.0)), None, None, None, None);
        let b = entry_with(fixed(&["b"]), Some(Basis::Points(4.0)), None, None, None, None);
        let sizes = allocate_stack_sizes(&[a, b], &[0, 0], Some(100), 0);
        assert_eq!(sizes, vec![4, 4]);
    }

    #[test]
    fn allocate_shrink_respects_min_size() {
        // First entry floors at minSize 6; the rest of the shrink lands on
        // the second entry.
        let a = entry_with(fixed(&["a"]), Some(Basis::Points(10.0)), None, Some(1.0), Some(6.0), None);
        let b = entry_with(fixed(&["b"]), Some(Basis::Points(10.0)), None, Some(1.0), None, None);
        let sizes = allocate_stack_sizes(&[a, b], &[0, 0], Some(12), 0);
        assert_eq!(sizes, vec![6, 6]);
    }

    #[test]
    fn allocate_accounts_for_gap_in_content_size() {
        // Three bases of 4; gap 2 removes 2*2 from the available size.
        let entries: Vec<StackLayoutEntry> = (0..3)
            .map(|i| {
                entry_with(
                    fixed(&[if i == 0 { "a" } else { "b" }]),
                    Some(Basis::Points(4.0)),
                    None,
                    Some(1.0),
                    None,
                    None,
                )
            })
            .collect();
        // Without gap: content 12 == total → untouched.
        assert_eq!(allocate_stack_sizes(&entries, &[0, 0, 0], Some(12), 0), vec![4, 4, 4]);
        // With gap 2: content = 12 - 4 = 8 → shrink 4 → [2, 3, 3].
        assert_eq!(allocate_stack_sizes(&entries, &[0, 0, 0], Some(12), 2), vec![2, 3, 3]);
    }

    #[test]
    fn clamp_size_floors_bounds_and_prefers_min_over_smaller_max() {
        // minSize 2.9 → 2; maxSize 5.7 → 5; basis 7.7 → 7 → clamped to 5.
        let entry = entry_with(fixed(&["x"]), Some(Basis::Points(7.7)), None, None, Some(2.9), Some(5.7));
        assert_eq!(clamp_size(7, &entry), 5);
        // max < min → max is raised to min.
        let entry = entry_with(fixed(&["x"]), None, None, None, Some(3.0), Some(1.0));
        assert_eq!(clamp_size(10, &entry), 3);
        // Negative sizes clamp to 0 (or min).
        let entry = plain_entry(fixed(&["x"]));
        assert_eq!(clamp_size(-5, &entry), 0);
    }

    // --- visible_stack_entries -------------------------------------------------

    #[test]
    fn visible_stack_entries_defaults_to_visible_and_filters_by_closure() {
        let hidden = fixed(&["a"]);
        let always = fixed(&["b"]);
        let never = fixed(&["c"]);
        let entries = vec![
            StackLayoutEntry {
                visible: Some(Rc::new(|viewport: &LayoutViewport| viewport.width >= 10)),
                ..plain_entry(hidden.clone())
            },
            plain_entry(always.clone()),
            StackLayoutEntry {
                visible: Some(Rc::new(|_viewport: &LayoutViewport| false)),
                ..plain_entry(never.clone())
            },
        ];

        let narrow = LayoutViewport { width: 5, height: 10 };
        let visible = visible_stack_entries(&entries, &narrow);
        assert_eq!(visible.len(), 1);
        assert!(Rc::ptr_eq(&visible[0].component, &always));

        let wide = LayoutViewport { width: 10, height: 10 };
        let visible = visible_stack_entries(&entries, &wide);
        assert_eq!(visible.len(), 2);
        assert!(Rc::ptr_eq(&visible[0].component, &hidden));
    }

    // --- Stack add/remove/clear -------------------------------------------------

    #[test]
    fn stack_constructor_normalizes_gap_and_options() {
        let stack = Stack::new(
            Vec::new(),
            StackOptions {
                gap: Some(2.9),
                align: Some(StackAlign::Center),
            },
            StackType::VStack,
        );
        assert_eq!(stack.gap(), 2);
        assert_eq!(stack.align(), StackAlign::Center);
        assert!(stack.entries().is_empty());

        let stack = Stack::new(Vec::new(), StackOptions::default(), StackType::HStack);
        assert_eq!(stack.gap(), 0);
        assert_eq!(stack.align(), StackAlign::Stretch);
    }

    #[test]
    fn stack_negative_gap_normalizes_to_zero() {
        let stack = Stack::new(
            Vec::new(),
            StackOptions {
                gap: Some(-3.0),
                align: None,
            },
            StackType::VStack,
        );
        assert_eq!(stack.gap(), 0);
    }

    #[test]
    fn stack_add_child_pushes_container_child_and_normalized_entry() {
        let component = fixed(&["a"]);
        let mut stack = Stack::new(Vec::new(), StackOptions::default(), StackType::VStack);
        stack.add_child_with(
            component.clone(),
            StackEntryOptions {
                basis: Some(Basis::Points(5.5)),
                grow: Some(1.7),
                shrink: Some(0.5),
                min_size: Some(2.9),
                max_size: Some(f64::INFINITY),
                visible: None,
            },
        );
        // Container child list and entry list stay in sync.
        assert_eq!(stack.container.children.len(), 1);
        assert!(Rc::ptr_eq(&stack.container.children[0], &component));
        assert_eq!(stack.entries().len(), 1);
        let entry = &stack.entries()[0];
        assert!(Rc::ptr_eq(&entry.component, &component));
        // basis passes through un-normalized (floored later by clampSize).
        assert_eq!(entry.basis, Some(Basis::Points(5.5)));
        // grow 1.7 floors to 1; shrink 0.5 floors to 0 (TS normalizeSize
        // quirk: fractional shrink collapses to "no shrink").
        assert_eq!(entry.grow, Some(1.0));
        assert_eq!(entry.shrink, Some(0.0));
        assert_eq!(entry.min_size, Some(2.0));
        // Non-finite max falls back to the MAX_SAFE_INTEGER stand-in.
        assert_eq!(entry.max_size, Some(MAX_SAFE_INTEGER as f64));
    }

    #[test]
    fn stack_add_child_with_default_options_leaves_fields_unset() {
        let mut stack = Stack::new(Vec::new(), StackOptions::default(), StackType::VStack);
        stack.add_child(fixed(&["a"]));
        let entry = &stack.entries()[0];
        assert_eq!(entry.basis, None);
        assert_eq!(entry.grow, None);
        assert_eq!(entry.shrink, None);
        assert_eq!(entry.min_size, None);
        assert_eq!(entry.max_size, None);
    }

    #[test]
    fn stack_constructor_accepts_mixed_children() {
        let plain = fixed(&["a"]);
        let with_options = fixed(&["b"]);
        let stack = Stack::new(
            vec![
                StackChild::Component(plain.clone()),
                StackChild::Entry {
                    component: with_options.clone(),
                    options: StackEntryOptions {
                        grow: Some(2.0),
                        ..Default::default()
                    },
                },
            ],
            StackOptions::default(),
            StackType::VStack,
        );
        assert_eq!(stack.container.children.len(), 2);
        assert_eq!(stack.entries().len(), 2);
        assert!(Rc::ptr_eq(&stack.entries()[1].component, &with_options));
        assert_eq!(stack.entries()[1].grow, Some(2.0));
    }

    #[test]
    fn stack_remove_child_and_clear_keep_lists_in_sync() {
        let a = fixed(&["a"]);
        let b = fixed(&["b"]);
        let c = fixed(&["c"]);
        let mut stack = Stack::new(Vec::new(), StackOptions::default(), StackType::VStack);
        stack.add_child(a.clone());
        stack.add_child(b.clone());
        stack.add_child(c.clone());
        assert_eq!(stack.container.children.len(), 3);
        assert_eq!(stack.entries().len(), 3);

        // Identity-based removal: an equal-looking impostor removes nothing.
        let impostor = fixed(&["b"]);
        stack.remove_child(&impostor);
        assert_eq!(stack.container.children.len(), 3);
        assert_eq!(stack.entries().len(), 3);

        stack.remove_child(&b);
        assert_eq!(stack.container.children.len(), 2);
        assert_eq!(stack.entries().len(), 2);
        assert!(Rc::ptr_eq(&stack.entries()[0].component, &a));
        assert!(Rc::ptr_eq(&stack.entries()[1].component, &c));

        stack.clear();
        assert!(stack.container.children.is_empty());
        assert!(stack.entries().is_empty());
    }

    #[test]
    fn stack_layout_node_returns_cloned_entries_with_identity() {
        let a = fixed(&["a"]);
        let mut stack = Stack::new(
            vec![StackChild::Component(a.clone())],
            StackOptions {
                gap: Some(1.0),
                align: Some(StackAlign::End),
            },
            StackType::HStack,
        );
        stack.add_child_with(
            fixed(&["b"]),
            StackEntryOptions {
                grow: Some(2.0),
                ..Default::default()
            },
        );
        let LayoutNode::Stack(node) = stack.layout_node() else {
            panic!("expected stack layout node");
        };
        assert_eq!(node.layout_type, StackType::HStack);
        assert_eq!(node.gap, 1);
        assert_eq!(node.align, StackAlign::End);
        assert_eq!(node.entries.len(), 2);
        assert!(Rc::ptr_eq(&node.entries[0].component, &a));
        assert_eq!(node.entries[1].grow, Some(2.0));
        // The stack keeps its own entries (the node holds clones).
        assert_eq!(stack.entries().len(), 2);
    }
}
