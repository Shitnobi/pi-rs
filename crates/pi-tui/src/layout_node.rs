//! Port of `packages/tui/src/layout-node.ts` — the layout-node capability
//! types connecting components to the flex layout solver (`layout.rs`).
//!
//! TS exposes the capability through the `LAYOUT_NODE` symbol; the Rust
//! port models it as [`crate::component::Component::layout_node`]
//! (default `None`, mirroring "no such method").

use std::rc::Rc;

use crate::component::ComponentRef;

/// `LayoutViewport` (layout-node.ts:8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutViewport {
    pub width: i64,
    pub height: i64,
}

/// `basis?: number | "auto"` — `Option<Basis>` with `None` meaning unset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Basis {
    Auto,
    /// A JS number (kept as `f64` for exact arithmetic parity).
    Points(f64),
}

/// `StackLayoutEntry` (layout-node.ts:16-24). The component is a shared
/// reference; entries are cloned into layout nodes, preserving identity.
#[derive(Clone)]
pub struct StackLayoutEntry {
    pub component: ComponentRef,
    pub basis: Option<Basis>,
    pub grow: Option<f64>,
    pub shrink: Option<f64>,
    pub min_size: Option<f64>,
    pub max_size: Option<f64>,
    /// `(viewport: LayoutViewport) => boolean` — dynamic visibility.
    pub visible: Option<Rc<dyn Fn(&LayoutViewport) -> bool>>,
}

/// `align: "stretch" | "start" | "center" | "end"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackAlign {
    Stretch,
    Start,
    Center,
    End,
}

/// `StackLayoutNode` (layout-node.ts:26-31).
#[derive(Clone)]
pub struct StackLayoutNode {
    /// `"vstack" | "hstack"`.
    pub layout_type: StackType,
    pub entries: Vec<StackLayoutEntry>,
    pub gap: i64,
    pub align: StackAlign,
}

/// `"vstack" | "hstack"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackType {
    VStack,
    HStack,
}

/// `overscroll: "chain" | "contain"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overscroll {
    Chain,
    Contain,
}

/// `ScrollLayoutState` (layout-node.ts:33-40): the scroll solver's live
/// state, owned by the ScrollView component.
pub trait ScrollLayoutState {
    fn scroll_top(&self) -> i64;
    fn primary(&self) -> bool;
    fn overscroll(&self) -> Overscroll;
    fn viewport_height(&self) -> i64;
    fn get_content_width(&self, width: i64) -> i64;
    /// `updateLayout(contentHeight, viewportHeight, requestRender)`.
    fn update_layout(
        &mut self,
        content_height: i64,
        viewport_height: i64,
        request_render: &mut dyn FnMut(),
    );
}

/// `ScrollLayoutNode` (layout-node.ts:42-46).
pub struct ScrollLayoutNode {
    pub component: ComponentRef,
    pub state: Rc<std::cell::RefCell<dyn ScrollLayoutState>>,
}

/// `LayoutNode = StackLayoutNode | ScrollLayoutNode`.
pub enum LayoutNode {
    Stack(StackLayoutNode),
    Scroll(ScrollLayoutNode),
}
