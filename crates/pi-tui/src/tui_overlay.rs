//! Port of the overlay half of `packages/tui/src/tui.ts` — the overlay
//! stack, the overlay focus-restore state machine, overlay layout
//! resolution and compositing.
//!
//! TS source map:
//! - `setFocus`/`setFocusInternal` + focus-restore machinery — tui.ts:594-724
//! - `showOverlay`/`hideOverlay` + handle semantics — tui.ts:726-860
//! - `resolveMouseFocusTarget`/`dispatchMouseToOverlay` — tui.ts:879-913
//! - `resolveOverlayLayout`/anchors — tui.ts:1215-1318
//! - `compositeOverlays` — tui.ts:1330-1420
//!
//! TS overlay-stack entries are object references; the Rust stack entries
//! carry a monotonic `entry_id` (see [`crate::tui::OverlayHandle`]) and
//! `ComponentRef` identity is `Rc::ptr_eq`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::component::{
    composite_tui_line, dispatch_mouse_event, ComponentRef, TuiMouseDispatchResult,
    TuiMouseEvent,
};
use crate::tui::{
    BlockedResume, OverlayAnchor, OverlayBounds, OverlayFocusRestorePolicy, OverlayFocusRestoreState,
    OverlayHandle, OverlayMarginOrNumber, OverlayOptions, OverlayStackEntry, OverlayUnfocusOptions,
    RenderedOverlayLayout, SizeValue, TuiBase,
};

// =============================================================================
// Focus (tui.ts:594-666)
// =============================================================================

/// `Component | null` identity — TS `===` on object references.
fn components_equal(a: Option<&ComponentRef>, b: Option<&ComponentRef>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// `containsComponent(root, target)` (tui.ts:695-700): identity, then
/// recurse into `Container` children (`instanceof Container` →
/// [`Component::as_container`]).
fn contains_component(root: &ComponentRef, target: &ComponentRef) -> bool {
    if Rc::ptr_eq(root, target) {
        return true;
    }
    let borrowed = root.borrow();
    if let Some(container) = borrowed.as_container() {
        return container.children.iter().any(|child| contains_component(child, target));
    }
    false
}

impl TuiBase {
    /// `setFocus(component)` (tui.ts:597-599) — public entry with the
    /// "clear" restore policy.
    pub(crate) fn set_focus(&mut self, component: Option<ComponentRef>) {
        self.set_focus_internal(component, OverlayFocusRestorePolicy::Clear);
    }

    /// `setFocusInternal({component, overlayFocusRestore})` (tui.ts:601-666).
    pub(crate) fn set_focus_internal(
        &mut self,
        component: Option<ComponentRef>,
        policy: OverlayFocusRestorePolicy,
    ) {
        let previous_focus = self.focused_component.clone();
        let mut next_focus = component;
        // `previousFocus ? overlayStack.find(e => e.component ===
        // previousFocus && isOverlayVisible(e)) : undefined`
        let previous_focused_overlay = previous_focus.as_ref().and_then(|previous| {
            self.overlay_stack
                .iter()
                .find(|entry| Rc::ptr_eq(&entry.component, previous) && self.is_overlay_visible(entry.entry_id))
                .map(|entry| entry.entry_id)
        });
        let next_focus_is_overlay = next_focus
            .as_ref()
            .map(|next| self.overlay_stack.iter().any(|entry| Rc::ptr_eq(&entry.component, next)))
            .unwrap_or(false);
        let restore_state = self.get_visible_overlay_focus_restore();

        if next_focus.is_some() && !next_focus_is_overlay {
            // `restoreState.status === "blocked" &&
            //  restoreState.blockedBy === previousFocus`
            let blocked_matches = match &restore_state {
                OverlayFocusRestoreState::Blocked { blocked_by, .. } => {
                    components_equal(blocked_by.as_ref(), previous_focus.as_ref())
                }
                _ => false,
            };
            if blocked_matches {
                if let OverlayFocusRestoreState::Blocked { entry_id, blocked_by, resume } =
                    &restore_state
                {
                    // `resume.status === "focus-target" ||
                    //  !isComponentMounted(restoreState.blockedBy)`
                    let resume_is_focus_target = matches!(resume, BlockedResume::FocusTarget(_));
                    let blocked_is_mounted = blocked_by
                        .as_ref()
                        .map(|b| self.is_component_mounted(b))
                        .unwrap_or(false);
                    if resume_is_focus_target || !blocked_is_mounted {
                        next_focus =
                            self.resolve_blocked_overlay_focus_resume(*entry_id, resume.clone());
                    } else {
                        self.overlay_focus_restore = OverlayFocusRestoreState::Blocked {
                            entry_id: *entry_id,
                            blocked_by: next_focus.clone(),
                            resume: resume.clone(),
                        };
                    }
                }
            } else if previous_focused_overlay.is_some()
                && !matches!(restore_state, OverlayFocusRestoreState::Inactive)
                && state_entry_id(&restore_state) == previous_focused_overlay
            {
                let overlay_id = previous_focused_overlay.expect("checked above");
                let is_ancestor = next_focus
                    .as_ref()
                    .map(|next| self.is_overlay_focus_ancestor(overlay_id, next))
                    .unwrap_or(false);
                if !is_ancestor {
                    self.overlay_focus_restore = OverlayFocusRestoreState::Blocked {
                        entry_id: overlay_id,
                        blocked_by: next_focus.clone(),
                        resume: BlockedResume::RestoreOverlay,
                    };
                }
            }
        } else if next_focus.is_none() {
            if let OverlayFocusRestoreState::Blocked { entry_id, blocked_by, resume } = &restore_state {
                if components_equal(blocked_by.as_ref(), previous_focus.as_ref()) {
                    next_focus =
                        self.resolve_blocked_overlay_focus_resume(*entry_id, resume.clone());
                } else if policy == OverlayFocusRestorePolicy::Clear {
                    self.clear_overlay_focus_restore();
                }
            } else if policy == OverlayFocusRestorePolicy::Clear {
                self.clear_overlay_focus_restore();
            }
        }

        // `if (isFocusable(this.focusedComponent))
        //     this.focusedComponent.focused = false;`
        if let Some(previous) = &self.focused_component {
            if let Some(focusable) = previous.borrow_mut().as_focusable() {
                focusable.set_focused(false);
            }
        }
        self.focused_component = next_focus.clone();
        if let Some(next) = &next_focus {
            if let Some(focusable) = next.borrow_mut().as_focusable() {
                focusable.set_focused(true);
            }
        }

        // Focusing a visible overlay arms the restore machinery.
        let focused_overlay = next_focus.as_ref().and_then(|next| {
            self.overlay_stack
                .iter()
                .find(|entry| Rc::ptr_eq(&entry.component, next) && self.is_overlay_visible(entry.entry_id))
                .map(|entry| entry.entry_id)
        });
        if let Some(entry_id) = focused_overlay {
            self.overlay_focus_restore = OverlayFocusRestoreState::Eligible { entry_id };
        }
    }

    /// `clearOverlayFocusRestore()` (tui.ts:668-670).
    pub(crate) fn clear_overlay_focus_restore(&mut self) {
        self.overlay_focus_restore = OverlayFocusRestoreState::Inactive;
    }

    /// `clearOverlayFocusRestoreFor(overlay)` (tui.ts:672-676).
    fn clear_overlay_focus_restore_for(&mut self, entry_id: u64) {
        if state_entry_id(&self.overlay_focus_restore) == Some(entry_id)
            && !matches!(self.overlay_focus_restore, OverlayFocusRestoreState::Inactive)
        {
            self.clear_overlay_focus_restore();
        }
    }

    /// `resolveBlockedOverlayFocusResume(restoreState)` (tui.ts:678-682).
    fn resolve_blocked_overlay_focus_resume(
        &mut self,
        entry_id: u64,
        resume: BlockedResume,
    ) -> Option<ComponentRef> {
        match resume {
            BlockedResume::RestoreOverlay => self.overlay_entry_component_opt(entry_id),
            BlockedResume::FocusTarget(target) => {
                self.clear_overlay_focus_restore();
                target
            }
        }
    }

    /// `getVisibleOverlayFocusRestore()` (tui.ts:684-690).
    pub(crate) fn get_visible_overlay_focus_restore(&self) -> OverlayFocusRestoreState {
        let entry_id = match state_entry_id(&self.overlay_focus_restore) {
            Some(entry_id) => entry_id,
            None => return OverlayFocusRestoreState::Inactive,
        };
        match self.entry_index(entry_id) {
            Some(index) if self.is_overlay_visible_index(index) => {
                self.overlay_focus_restore.clone()
            }
            _ => OverlayFocusRestoreState::Inactive,
        }
    }

    /// `isOverlayFocusAncestor(entry, component)` (tui.ts:692-703): walk the
    /// entry's `preFocus` chain (through overlay stack links) with a
    /// visited set.
    fn is_overlay_focus_ancestor(&self, entry_id: u64, component: &ComponentRef) -> bool {
        let Some(index) = self.entry_index(entry_id) else { return false };
        let mut visited: Vec<ComponentRef> = Vec::new();
        let mut current = self.overlay_stack[index].pre_focus.clone();
        while let Some(node) = current {
            if visited.iter().any(|seen| Rc::ptr_eq(seen, &node)) {
                break;
            }
            visited.push(node.clone());
            if Rc::ptr_eq(&node, component) {
                return true;
            }
            current = self
                .overlay_stack
                .iter()
                .find(|overlay| Rc::ptr_eq(&overlay.component, &node))
                .and_then(|overlay| overlay.pre_focus.clone());
        }
        false
    }

    /// `retargetOverlayPreFocus(removed)` (tui.ts:705-711): overlays that
    /// would restore focus to the removed overlay's component now restore
    /// to what the removed overlay would have restored to.
    fn retarget_overlay_pre_focus(&mut self, removed_index: usize) {
        let removed_component = self.overlay_stack[removed_index].component.clone();
        let removed_pre_focus = self.overlay_stack[removed_index].pre_focus.clone();
        for (index, overlay) in self.overlay_stack.iter_mut().enumerate() {
            if index != removed_index {
                if let Some(pre_focus) = &overlay.pre_focus {
                    if Rc::ptr_eq(pre_focus, &removed_component) {
                        overlay.pre_focus = removed_pre_focus.clone();
                    }
                }
            }
        }
    }

    /// `isComponentMounted(component)` (tui.ts:713-715) —
    /// `getMountedRoots().some((child) => containsComponent(child, component))`.
    fn is_component_mounted(&self, component: &ComponentRef) -> bool {
        self.mounted_roots.iter().any(|root| contains_component(root, component))
    }

    // ----- stack lookups -----

    fn entry_index(&self, entry_id: u64) -> Option<usize> {
        self.overlay_stack.iter().position(|entry| entry.entry_id == entry_id)
    }

    fn is_overlay_visible_index(&self, index: usize) -> bool {
        let entry = &self.overlay_stack[index];
        if entry.hidden {
            return false;
        }
        if let Some(options) = &entry.options {
            if let Some(visible) = &options.visible {
                let (columns, rows) = {
                    let terminal = self.terminal.borrow();
                    (terminal.columns(), terminal.rows())
                };
                return visible(columns, rows);
            }
        }
        true
    }

    /// `isOverlayVisible(entry)` (tui.ts:906-913) by entry id. Unknown ids
    /// are not visible.
    pub(crate) fn is_overlay_visible(&self, entry_id: u64) -> bool {
        self.entry_index(entry_id).map(|index| self.is_overlay_visible_index(index)).unwrap_or(false)
    }

    /// `getTopmostVisibleOverlay()` (tui.ts:915-923) — the visual-frontmost
    /// visible capturing overlay's entry id.
    pub(crate) fn get_topmost_visible_overlay(&self) -> Option<u64> {
        let mut topmost: Option<usize> = None;
        for (index, overlay) in self.overlay_stack.iter().enumerate() {
            let non_capturing = overlay.options.as_ref().map(|o| o.non_capturing).unwrap_or(false);
            if non_capturing || !self.is_overlay_visible_index(index) {
                continue;
            }
            let better = match topmost {
                None => true,
                Some(top_index) => overlay.focus_order > self.overlay_stack[top_index].focus_order,
            };
            if better {
                topmost = Some(index);
            }
        }
        topmost.map(|index| self.overlay_stack[index].entry_id)
    }

    /// `hasOverlay()` (tui.ts:868-870).
    pub(crate) fn has_visible_overlay(&self) -> bool {
        self.overlay_stack.iter().any(|entry| self.is_overlay_visible(entry.entry_id))
    }

    /// The entry's component (panics on unknown ids — callers hold live
    /// ids, mirroring TS's closure-captured entries).
    pub(crate) fn overlay_entry_component(&self, entry_id: u64) -> ComponentRef {
        self.overlay_entry_component_opt(entry_id)
            .expect("overlay entry must exist while its id is live")
    }

    fn overlay_entry_component_opt(&self, entry_id: u64) -> Option<ComponentRef> {
        self.entry_index(entry_id).map(|index| self.overlay_stack[index].component.clone())
    }

    pub(crate) fn overlay_entry_pre_focus(&self, entry_id: u64) -> Option<ComponentRef> {
        self.entry_index(entry_id).and_then(|index| self.overlay_stack[index].pre_focus.clone())
    }

    // =============================================================================
    // Overlay stack operations (tui.ts:726-860)
    // =============================================================================

    /// `showOverlay(component, options?)` (tui.ts:726-776). The TS handle
    /// closures become id-dispatched [`OverlayHandle`] methods.
    pub(crate) fn show_overlay(
        &mut self,
        component: ComponentRef,
        options: Option<OverlayOptions>,
    ) -> OverlayHandle {
        let entry_id = self.next_overlay_entry_id;
        self.next_overlay_entry_id += 1;
        self.focus_order_counter += 1;
        let entry = OverlayStackEntry {
            entry_id,
            component: component.clone(),
            options,
            pre_focus: self.focused_component.clone(),
            hidden: false,
            focus_order: self.focus_order_counter,
            bounds: None,
        };
        self.overlay_stack.push(entry);
        // Only focus if the overlay is actually visible.
        let non_capturing = self.overlay_stack.last().expect("just pushed")
            .options
            .as_ref()
            .map(|o| o.non_capturing)
            .unwrap_or(false);
        if !non_capturing && self.is_overlay_visible(entry_id) {
            self.set_focus(Some(component));
        }
        self.hide_terminal_cursor();
        self.request_render_state(false);
        let surface = self
            .self_weak
            .borrow()
            .clone()
            .expect("install_self must run before show_overlay");
        OverlayHandle::new(surface, entry_id)
    }

    /// `hideOverlay()` (tui.ts:845-860) — topmost entry.
    pub(crate) fn hide_overlay_top(&mut self) {
        if self.overlay_stack.is_empty() {
            return;
        }
        let index = self.overlay_stack.len() - 1;
        self.hide_overlay_entry_at_index(index);
    }

    /// The `hide()` handle closure (tui.ts:779-795) / `hideOverlay()` body.
    pub(crate) fn hide_overlay_entry(&mut self, entry_id: u64) {
        let Some(index) = self.entry_index(entry_id) else { return };
        self.hide_overlay_entry_at_index(index);
    }

    fn hide_overlay_entry_at_index(&mut self, index: usize) {
        let component = self.overlay_stack[index].component.clone();
        let pre_focus = self.overlay_stack[index].pre_focus.clone();
        let entry_id = self.overlay_stack[index].entry_id;
        self.clear_overlay_focus_restore_for(entry_id);
        self.retarget_overlay_pre_focus(index);
        self.overlay_stack.remove(index);
        // Restore focus if this overlay had focus.
        if components_equal(self.focused_component.as_ref(), Some(&component)) {
            let target = match self.get_topmost_visible_overlay() {
                Some(top) => Some(self.overlay_entry_component(top)),
                None => pre_focus,
            };
            self.set_focus(target);
        }
        if self.overlay_stack.is_empty() {
            self.hide_terminal_cursor();
        }
        self.request_render_state(false);
    }

    /// The `setHidden(hidden)` handle closure (tui.ts:796-817).
    pub(crate) fn set_overlay_hidden(&mut self, entry_id: u64, hidden: bool) {
        let Some(index) = self.entry_index(entry_id) else { return };
        if self.overlay_stack[index].hidden == hidden {
            return;
        }
        self.overlay_stack[index].hidden = hidden;
        let component = self.overlay_stack[index].component.clone();
        let pre_focus = self.overlay_stack[index].pre_focus.clone();
        let non_capturing = self.overlay_stack[index]
            .options
            .as_ref()
            .map(|o| o.non_capturing)
            .unwrap_or(false);
        if hidden {
            self.clear_overlay_focus_restore_for(entry_id);
            // If this overlay had focus, move focus to the next visible
            // overlay or preFocus.
            if components_equal(self.focused_component.as_ref(), Some(&component)) {
                let target = match self.get_topmost_visible_overlay() {
                    Some(top) => Some(self.overlay_entry_component(top)),
                    None => pre_focus,
                };
                self.set_focus(target);
            }
        } else {
            // Restore focus to this overlay when showing (if visible).
            if !non_capturing && self.is_overlay_visible(entry_id) {
                self.focus_order_counter += 1;
                self.overlay_stack[index].focus_order = self.focus_order_counter;
                self.set_focus(Some(component));
            }
        }
        self.request_render_state(false);
    }

    /// The `isHidden()` handle closure (tui.ts:818).
    pub(crate) fn is_overlay_entry_hidden(&self, entry_id: u64) -> bool {
        self.entry_index(entry_id).map(|index| self.overlay_stack[index].hidden).unwrap_or(true)
    }

    /// The `focus()` handle closure (tui.ts:819-824).
    pub(crate) fn focus_overlay_entry(&mut self, entry_id: u64) {
        let Some(index) = self.entry_index(entry_id) else { return };
        if !self.is_overlay_visible_index(index) {
            return;
        }
        self.focus_order_counter += 1;
        self.overlay_stack[index].focus_order = self.focus_order_counter;
        let component = self.overlay_stack[index].component.clone();
        self.set_focus(Some(component));
        self.request_render_state(false);
    }

    /// The `unfocus(unfocusOptions?)` handle closure (tui.ts:825-846).
    pub(crate) fn unfocus_overlay_entry(
        &mut self,
        entry_id: u64,
        unfocus_options: Option<OverlayUnfocusOptions>,
    ) {
        let Some(index) = self.entry_index(entry_id) else { return };
        let component = self.overlay_stack[index].component.clone();
        let is_focused = components_equal(self.focused_component.as_ref(), Some(&component));
        let restore_state = self.overlay_focus_restore.clone();
        let has_pending_restore =
            state_entry_id(&restore_state) == Some(entry_id)
                && !matches!(restore_state, OverlayFocusRestoreState::Inactive);
        if !is_focused && !has_pending_restore {
            return;
        }
        if let OverlayFocusRestoreState::Blocked { entry_id: blocked_entry, blocked_by, .. } =
            &restore_state
        {
            if *blocked_entry == entry_id
                && components_equal(blocked_by.as_ref(), self.focused_component.as_ref())
            {
                if let Some(options) = unfocus_options {
                    self.overlay_focus_restore = OverlayFocusRestoreState::Blocked {
                        entry_id,
                        blocked_by: blocked_by.clone(),
                        resume: BlockedResume::FocusTarget(options.target.clone()),
                    };
                } else {
                    self.clear_overlay_focus_restore();
                }
                self.request_render_state(false);
                return;
            }
        }
        self.clear_overlay_focus_restore_for(entry_id);
        if is_focused || unfocus_options.is_some() {
            let top_visible = self.get_topmost_visible_overlay();
            let fallback_target = match top_visible {
                Some(top) if top != entry_id => Some(self.overlay_entry_component(top)),
                _ => self.overlay_stack[index].pre_focus.clone(),
            };
            let target = match unfocus_options {
                Some(options) => options.target,
                None => fallback_target,
            };
            self.set_focus(target);
        }
        self.request_render_state(false);
    }

    /// The `isFocused()` handle closure (tui.ts:847).
    pub(crate) fn is_overlay_entry_focused(&self, entry_id: u64) -> bool {
        let Some(component) = self.overlay_entry_component_opt(entry_id) else { return false };
        components_equal(self.focused_component.as_ref(), Some(&component))
    }

    /// The `getBounds()` handle closure (tui.ts:848-853): the most recent
    /// rendered bounds of a visible, still-stacked overlay.
    pub(crate) fn overlay_entry_bounds(&self, entry_id: u64) -> Option<OverlayBounds> {
        let index = self.entry_index(entry_id)?;
        if !self.is_overlay_visible_index(index) {
            return None;
        }
        self.overlay_stack[index].bounds
    }

    // =============================================================================
    // Mouse plumbing (tui.ts:879-913)
    // =============================================================================

    /// `resolveMouseFocusTarget(component)` (tui.ts:879-887): keep overlay
    /// containers as keyboard focus owners when a nested control is clicked.
    pub(crate) fn resolve_mouse_focus_target(&self, component: &ComponentRef) -> ComponentRef {
        for index in (0..self.overlay_stack.len()).rev() {
            if self.is_overlay_visible_index(index)
                && contains_component(&self.overlay_stack[index].component, component)
            {
                return self.overlay_stack[index].component.clone();
            }
        }
        component.clone()
    }

    /// `dispatchMouseToOverlay(event)` (tui.ts:889-905): dispatch to the
    /// visually topmost overlay under the pointer. `None` = no hit;
    /// `Some(None)` = hit with no result; `Some(Some(result))` = hit with a
    /// dispatch result.
    pub(crate) fn dispatch_mouse_to_overlay(
        &mut self,
        event: &TuiMouseEvent,
    ) -> Option<Option<TuiMouseDispatchResult>> {
        for layout in self.rendered_overlay_layouts.iter().rev() {
            if event.screen_x < layout.col
                || event.screen_x >= layout.col + layout.width
                || event.screen_y < layout.row
                || event.screen_y >= layout.row + layout.height
            {
                continue;
            }
            let mut local = event.clone();
            local.x = event.screen_x - layout.col;
            local.y = event.screen_y - layout.row;
            local.width = layout.width;
            local.height = layout.height;
            let result = dispatch_mouse_event(&layout.component, &local);
            let result = result.map(|mut result| {
                if result.is_focus() {
                    result.focus_target = Some(layout.component.clone());
                }
                result
            });
            return Some(result);
        }
        None
    }

    // =============================================================================
    // Layout resolution (tui.ts:1215-1318)
    // =============================================================================

    /// `resolveOverlayLayout(options, overlayHeight, termWidth, termHeight)`
    /// (tui.ts:1215-1289) → `(width, row, col, maxHeight)`.
    fn resolve_overlay_layout(
        &self,
        options: Option<&OverlayOptions>,
        overlay_height: i64,
        term_width: i64,
        term_height: i64,
    ) -> (i64, i64, i64, Option<i64>) {
        let opt = options;

        // Parse margin (clamp to non-negative).
        let (margin_top, margin_right, margin_bottom, margin_left) = match opt.and_then(|o| o.margin)
        {
            Some(OverlayMarginOrNumber::Number(n)) => (n, n, n, n),
            Some(OverlayMarginOrNumber::Margin(m)) => (
                m.top.unwrap_or(0),
                m.right.unwrap_or(0),
                m.bottom.unwrap_or(0),
                m.left.unwrap_or(0),
            ),
            None => (0, 0, 0, 0),
        };
        let margin_top = margin_top.max(0);
        let margin_right = margin_right.max(0);
        let margin_bottom = margin_bottom.max(0);
        let margin_left = margin_left.max(0);

        let avail_width = (term_width - margin_left - margin_right).max(1);
        let avail_height = (term_height - margin_top - margin_bottom).max(1);

        // === Resolve width ===
        let mut width = parse_size_value(opt.and_then(|o| o.width.as_ref()), term_width)
            .unwrap_or(80.min(avail_width));
        if let Some(min_width) = opt.and_then(|o| o.min_width) {
            width = width.max(min_width);
        }
        width = width.max(1).min(avail_width);

        // === Resolve maxHeight ===
        let mut max_height = parse_size_value(opt.and_then(|o| o.max_height.as_ref()), term_height);
        if let Some(mh) = max_height {
            max_height = Some(mh.max(1).min(avail_height));
        }

        let effective_height = max_height.map(|mh| overlay_height.min(mh)).unwrap_or(overlay_height);

        // === Resolve position ===
        let mut row = if let Some(row_value) = opt.and_then(|o| o.row.as_ref()) {
            match row_value {
                SizeValue::Percent(percent) => match parse_percent(percent) {
                    // 0% = top, 100% = bottom (overlay stays within bounds).
                    Some(percent) => {
                        let max_row = (avail_height - effective_height).max(0);
                        margin_top + (max_row as f64 * (percent / 100.0)).floor() as i64
                    }
                    None => self.resolve_anchor_row(
                        OverlayAnchor::Center,
                        effective_height,
                        avail_height,
                        margin_top,
                    ),
                },
                SizeValue::Number(n) => *n as i64,
            }
        } else {
            let anchor = opt.and_then(|o| o.anchor).unwrap_or(OverlayAnchor::Center);
            self.resolve_anchor_row(anchor, effective_height, avail_height, margin_top)
        };

        let mut col = if let Some(col_value) = opt.and_then(|o| o.col.as_ref()) {
            match col_value {
                SizeValue::Percent(percent) => match parse_percent(percent) {
                    Some(percent) => {
                        let max_col = (avail_width - width).max(0);
                        margin_left + (max_col as f64 * (percent / 100.0)).floor() as i64
                    }
                    None => self.resolve_anchor_col(
                        OverlayAnchor::Center,
                        width,
                        avail_width,
                        margin_left,
                    ),
                },
                SizeValue::Number(n) => *n as i64,
            }
        } else {
            let anchor = opt.and_then(|o| o.anchor).unwrap_or(OverlayAnchor::Center);
            self.resolve_anchor_col(anchor, width, avail_width, margin_left)
        };

        // Apply offsets.
        if let Some(offset_y) = opt.and_then(|o| o.offset_y) {
            row += offset_y;
        }
        if let Some(offset_x) = opt.and_then(|o| o.offset_x) {
            col += offset_x;
        }

        // Clamp to terminal bounds (respecting margins).
        row = row.max(margin_top).min(term_height - margin_bottom - effective_height);
        col = col.max(margin_left).min(term_width - margin_right - width);

        (width, row, col, max_height)
    }

    /// `resolveAnchorRow` (tui.ts:1291-1305).
    fn resolve_anchor_row(
        &self,
        anchor: OverlayAnchor,
        height: i64,
        avail_height: i64,
        margin_top: i64,
    ) -> i64 {
        match anchor {
            OverlayAnchor::TopLeft
            | OverlayAnchor::TopCenter
            | OverlayAnchor::TopRight => margin_top,
            OverlayAnchor::BottomLeft
            | OverlayAnchor::BottomCenter
            | OverlayAnchor::BottomRight => margin_top + avail_height - height,
            // `Math.floor((availHeight - height) / 2)` — floor, also for the
            // negative intermediate (i64 division truncates toward zero).
            OverlayAnchor::LeftCenter
            | OverlayAnchor::Center
            | OverlayAnchor::RightCenter => margin_top + (avail_height - height).div_euclid(2),
        }
    }

    /// `resolveAnchorCol` (tui.ts:1307-1318).
    fn resolve_anchor_col(
        &self,
        anchor: OverlayAnchor,
        width: i64,
        avail_width: i64,
        margin_left: i64,
    ) -> i64 {
        match anchor {
            OverlayAnchor::TopLeft
            | OverlayAnchor::LeftCenter
            | OverlayAnchor::BottomLeft => margin_left,
            OverlayAnchor::TopRight
            | OverlayAnchor::RightCenter
            | OverlayAnchor::BottomRight => margin_left + avail_width - width,
            OverlayAnchor::TopCenter
            | OverlayAnchor::Center
            | OverlayAnchor::BottomCenter => margin_left + (avail_width - width).div_euclid(2),
        }
    }

    // =============================================================================
    // Compositing (tui.ts:1330-1420)
    // =============================================================================

    /// `compositeOverlays(lines, termWidth, termHeight)` (tui.ts:1330-1420):
    /// pre-render visible overlays (sorted by `focusOrder`), position them,
    /// pad the frame to at least terminal height and composite.
    pub(crate) fn composite_overlays(
        &mut self,
        mut lines: Vec<String>,
        term_width: usize,
        term_height: usize,
    ) -> Vec<String> {
        if self.overlay_stack.is_empty() {
            self.rendered_overlay_layouts = Vec::new();
            return lines;
        }

        for entry in &mut self.overlay_stack {
            entry.bounds = None;
        }

        // Pre-render all visible overlays and calculate positions.
        let mut rendered: Vec<(usize, Vec<String>, i64, i64, i64)> = Vec::new();
        let mut min_lines_needed = lines.len() as i64;

        let mut visible_indices: Vec<usize> = (0..self.overlay_stack.len())
            .filter(|&index| self.is_overlay_visible_index(index))
            .collect();
        visible_indices.sort_by_key(|&index| self.overlay_stack[index].focus_order);

        for &index in &visible_indices {
            let options = self.overlay_stack[index].options.clone();
            let component = self.overlay_stack[index].component.clone();

            // Layout with height=0 first: width and maxHeight don't depend
            // on the overlay height.
            let (width, _, _, max_height) =
                self.resolve_overlay_layout(options.as_ref(), 0, term_width as i64, term_height as i64);

            let mut overlay_lines =
                self.resolve_fake_cursors(component.borrow_mut().render(width.max(0) as usize));

            if let Some(max_height) = max_height {
                if overlay_lines.len() as i64 > max_height {
                    overlay_lines.truncate(max_height.max(0) as usize);
                }
            }

            let (_, row, col, _) = self.resolve_overlay_layout(
                options.as_ref(),
                overlay_lines.len() as i64,
                term_width as i64,
                term_height as i64,
            );
            self.overlay_stack[index].bounds = Some(OverlayBounds {
                row,
                col,
                width,
                height: overlay_lines.len() as i64,
            });

            min_lines_needed = min_lines_needed.max(row + overlay_lines.len() as i64);
            rendered.push((index, overlay_lines, row, col, width));
        }
        self.rendered_overlay_layouts = rendered
            .iter()
            .map(|(index, overlay_lines, row, col, width)| RenderedOverlayLayout {
                entry_id: self.overlay_stack[*index].entry_id,
                component: self.overlay_stack[*index].component.clone(),
                row: *row,
                col: *col,
                width: *width,
                height: overlay_lines.len() as i64,
            })
            .collect();

        // Pad to at least terminal height so overlays have screen-relative
        // positions (excludes the historical maxLinesRendered high-water
        // mark — see tui.ts:1373-1376).
        let working_height = (lines.len() as i64)
            .max(term_height as i64)
            .max(min_lines_needed) as usize;
        while lines.len() < working_height {
            lines.push(String::new());
        }

        let viewport_start = working_height.saturating_sub(term_height);

        for (_, overlay_lines, row, col, width) in &rendered {
            for (line_index, overlay_line) in overlay_lines.iter().enumerate() {
                let index = viewport_start as i64 + row + line_index as i64;
                if index >= 0 && (index as usize) < lines.len() {
                    // Defensive: truncate the overlay line to the declared
                    // width before compositing.
                    let truncated =
                        if crate::utils::visible_width(overlay_line) > *width as usize {
                            crate::utils::slice_by_column(overlay_line, 0, *width as usize, true)
                        } else {
                            overlay_line.clone()
                        };
                    lines[index as usize] = composite_tui_line(
                        &lines[index as usize],
                        &truncated,
                        (*col).max(0) as usize,
                        (*width).max(0) as usize,
                        term_width,
                    );
                }
            }
        }

        lines
    }
}

/// The overlay a restore state refers to (`Inactive` → `None`).
fn state_entry_id(state: &OverlayFocusRestoreState) -> Option<u64> {
    match state {
        OverlayFocusRestoreState::Inactive => None,
        OverlayFocusRestoreState::Eligible { entry_id }
        | OverlayFocusRestoreState::Blocked { entry_id, .. } => Some(*entry_id),
    }
}

/// `parseSizeValue` (tui.ts:239-248): absolute number, or `%` of the
/// reference size (`Math.floor`); invalid percent strings → `None`.
fn parse_size_value(value: Option<&SizeValue>, reference_size: i64) -> Option<i64> {
    let value = value?;
    match value {
        SizeValue::Number(n) => Some(*n as i64),
        SizeValue::Percent(percent) => {
            let percent = parse_percent(percent)?;
            Some(((reference_size as f64 * percent) / 100.0).floor() as i64)
        }
    }
}

/// `/^(\d+(?:\.\d+)?)%$/` — the numeric part of a valid percent string.
fn parse_percent(value: &str) -> Option<f64> {
    let body = value.strip_suffix('%')?;
    let (int_part, frac_part) = match body.split_once('.') {
        Some((int_part, frac_part)) => (int_part, Some(frac_part)),
        None => (body, None),
    };
    if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if let Some(frac_part) = frac_part {
        if frac_part.is_empty() || !frac_part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    body.parse::<f64>().ok()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::impl_tui_surface;
    use crate::component::{Component, ComponentMouseResult, Container, Focusable, CURSOR_MARKER};
    use crate::terminal::{ProgramStatus, Terminal};
    use crate::tui::{TuiInputListenerResult, TuiMode, TuiStopOptions, TuiSurface};
    use crate::test_support::global_test_lock;

    /// Recording terminal double with a settable size.
    struct TestTerminal {
        cols: usize,
        rows: usize,
        writes: Vec<String>,
        hide_cursor_calls: u32,
        show_cursor_calls: u32,
    }

    impl TestTerminal {
        fn new(cols: usize, rows: usize) -> Self {
            TestTerminal {
                cols,
                rows,
                writes: Vec::new(),
                hide_cursor_calls: 0,
                show_cursor_calls: 0,
            }
        }
    }

    impl Terminal for TestTerminal {
        fn start(
            &mut self,
            _on_input: crate::terminal::InputHandler,
            _on_resize: crate::terminal::ResizeHandler,
        ) {
        }
        fn stop(&mut self) {}
        fn drain_input(&mut self, _max_ms: u64, _idle_ms: u64) {}
        fn write(&mut self, data: &str) {
            self.writes.push(data.to_string());
        }
        fn columns(&self) -> usize {
            self.cols
        }
        fn rows(&self) -> usize {
            self.rows
        }
        fn kitty_protocol_active(&self) -> bool {
            false
        }
        fn move_by(&mut self, _lines: i32) {}
        fn hide_cursor(&mut self) {
            self.hide_cursor_calls += 1;
        }
        fn show_cursor(&mut self) {
            self.show_cursor_calls += 1;
        }
        fn clear_line(&mut self) {}
        fn clear_from_cursor(&mut self) {}
        fn clear_screen(&mut self) {}
        fn set_title(&mut self, _title: &str) {}
        fn set_progress(&mut self, _active: bool) {}
        fn set_program_status(&mut self, _status: ProgramStatus) {}
    }

    /// Minimal screen exercising the macro against `TuiBase` + overlays.
    struct TestScreen {
        base: TuiBase,
        last_frame: Vec<String>,
        renders: u32,
        resets: u32,
        cursor: Option<(usize, usize)>,
    }

    impl TestScreen {
        fn new_rc(cols: usize, rows: usize) -> Rc<RefCell<TestScreen>> {
            let terminal: Rc<RefCell<dyn Terminal>> = Rc::new(RefCell::new(TestTerminal::new(cols, rows)));
            Self::new_rc_with(terminal)
        }

        fn new_rc_with(terminal: Rc<RefCell<dyn Terminal>>) -> Rc<RefCell<TestScreen>> {
            let rc = Rc::new(RefCell::new(TestScreen {
                base: TuiBase::new(terminal, None),
                last_frame: Vec::new(),
                renders: 0,
                resets: 0,
                cursor: None,
            }));
            TestScreen::install_self(&rc);
            rc
        }

        fn do_render(&mut self) {
            let width = self.base.terminal.borrow().columns();
            let height = self.base.terminal.borrow().rows();
            let mut lines = self.render_children(width);
            lines = self.base.composite_overlays(lines, width, height);
            self.cursor = TuiBase::extract_cursor_position(&mut lines, height);
            self.last_frame = lines;
            self.renders += 1;
        }

        fn reset_render_state_hook(&mut self) {
            self.resets += 1;
        }
        fn before_terminal_start(&mut self) {}
        fn after_terminal_start(&mut self) {}
        fn before_terminal_stop(&mut self, _options: &TuiStopOptions) {}
        fn after_terminal_stop(&mut self, _options: &TuiStopOptions) {}
        fn mode(&self) -> TuiMode {
            TuiMode::Regular
        }
        fn render_children(&mut self, width: usize) -> Vec<String> {
            self.base.container.render(width)
        }
        fn handle_mouse_base(
            &mut self,
            _self_ref: &ComponentRef,
            _event: &TuiMouseEvent,
        ) -> Option<ComponentMouseResult> {
            None
        }
        fn handle_viewport_input(&mut self, _data: &str) -> Option<TuiInputListenerResult> {
            None
        }
    }

    impl_tui_surface!(TestScreen);

    /// A focusable, input-handling test component rendering one line.
    struct TestComponent {
        text: String,
        focusable: bool,
        focused: bool,
        inputs: Vec<String>,
    }

    impl TestComponent {
        fn plain(text: &str) -> ComponentRef {
            Rc::new(RefCell::new(TestComponent {
                text: text.to_string(),
                focusable: false,
                focused: false,
                inputs: Vec::new(),
            }))
        }

        fn focusable(text: &str) -> ComponentRef {
            Rc::new(RefCell::new(TestComponent {
                text: text.to_string(),
                focusable: true,
                focused: false,
                inputs: Vec::new(),
            }))
        }

        /// Typed handle for tests that inspect component state directly.
        fn focusable_typed(text: &str) -> Rc<RefCell<TestComponent>> {
            Rc::new(RefCell::new(TestComponent {
                text: text.to_string(),
                focusable: true,
                focused: false,
                inputs: Vec::new(),
            }))
        }
    }

    /// Coerce a typed test component to `ComponentRef` (shares the RefCell).
    fn comp(rc: &Rc<RefCell<TestComponent>>) -> ComponentRef {
        rc.clone()
    }

    impl Component for TestComponent {
        fn render(&mut self, _width: usize) -> Vec<String> {
            let mut line = self.text.clone();
            if self.focused {
                line.push_str(CURSOR_MARKER);
            }
            vec![line]
        }
        fn handles_input(&self) -> bool {
            true
        }
        fn handle_input(&mut self, data: &str) {
            self.inputs.push(data.to_string());
        }
        fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
            if self.focusable {
                Some(self)
            } else {
                None
            }
        }
    }

    impl Focusable for TestComponent {
        fn focused(&self) -> bool {
            self.focused
        }
        fn set_focused(&mut self, focused: bool) {
            self.focused = focused;
        }
    }

    fn focused_flag(component: &ComponentRef) -> bool {
        component.borrow_mut().as_focusable().map(|f| f.focused()).unwrap_or(false)
    }

    #[test]
    fn set_focus_toggles_focusable_flags() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(80, 24);
        let a = TestComponent::focusable("a");
        let b = TestComponent::focusable("b");
        screen.borrow_mut().add_child(a.clone());
        screen.borrow_mut().add_child(b.clone());

        TuiSurface::set_focus(&mut *screen.borrow_mut(), Some(a.clone()));
        assert!(focused_flag(&a));
        assert!(!focused_flag(&b));
        TuiSurface::set_focus(&mut *screen.borrow_mut(), Some(b.clone()));
        assert!(!focused_flag(&a));
        assert!(focused_flag(&b));
        TuiSurface::set_focus(&mut *screen.borrow_mut(), None);
        assert!(!focused_flag(&b));
    }

    #[test]
    fn show_overlay_focuses_and_hide_restores_pre_focus() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(80, 24);
        let editor = TestComponent::focusable("editor");
        screen.borrow_mut().add_child(editor.clone());
        TuiSurface::set_focus(&mut *screen.borrow_mut(), Some(editor.clone()));
        assert!(focused_flag(&editor));

        let overlay = TestComponent::focusable("overlay");
        let handle = TuiSurface::show_overlay(
            &mut *screen.borrow_mut(),
            overlay.clone(),
            OverlayOptions::default(),
        );
        assert!(focused_flag(&overlay));
        assert!(!focused_flag(&editor));
        assert!(handle.is_focused());
        assert!(TuiSurface::has_overlay(&mut *screen.borrow_mut()));

        handle.hide();
        assert!(focused_flag(&editor));
        assert!(!handle.is_focused());
        assert!(!TuiSurface::has_overlay(&mut *screen.borrow_mut()));
    }

    #[test]
    fn set_hidden_moves_focus_back_and_forth() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(80, 24);
        let editor = TestComponent::focusable("editor");
        screen.borrow_mut().add_child(editor.clone());
        TuiSurface::set_focus(&mut *screen.borrow_mut(), Some(editor.clone()));

        let overlay = TestComponent::focusable("ov");
        let handle = TuiSurface::show_overlay(
            &mut *screen.borrow_mut(),
            overlay.clone(),
            OverlayOptions::default(),
        );
        assert!(!handle.is_hidden());

        handle.set_hidden(true);
        assert!(handle.is_hidden());
        assert!(focused_flag(&editor));
        assert!(!TuiSurface::has_overlay(&mut *screen.borrow_mut()));

        handle.set_hidden(false);
        assert!(!handle.is_hidden());
        assert!(focused_flag(&overlay));
        assert!(handle.is_focused());
    }

    #[test]
    fn non_capturing_overlay_does_not_steal_focus() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(80, 24);
        let editor = TestComponent::focusable("editor");
        screen.borrow_mut().add_child(editor.clone());
        TuiSurface::set_focus(&mut *screen.borrow_mut(), Some(editor.clone()));

        let overlay = TestComponent::focusable("ov");
        let options = OverlayOptions { non_capturing: true, ..Default::default() };
        let handle = TuiSurface::show_overlay(&mut *screen.borrow_mut(), overlay.clone(), options);
        assert!(focused_flag(&editor));
        assert!(!handle.is_focused());

        // Focus can be moved onto it explicitly (eligible restore arms).
        handle.focus();
        assert!(handle.is_focused());
        assert!(focused_flag(&overlay));
    }

    #[test]
    fn visible_callback_gates_focus_and_compositing() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(80, 24);
        let editor = TestComponent::focusable("editor");
        screen.borrow_mut().add_child(editor.clone());
        TuiSurface::set_focus(&mut *screen.borrow_mut(), Some(editor.clone()));

        let overlay = TestComponent::focusable("ov");
        let options = OverlayOptions {
            visible: Some(Rc::new(|_cols, _rows| false)),
            ..Default::default()
        };
        let handle = TuiSurface::show_overlay(&mut *screen.borrow_mut(), overlay, options);
        // Not visible → no focus capture, no overlay reported.
        assert!(focused_flag(&editor));
        assert!(!TuiSurface::has_overlay(&mut *screen.borrow_mut()));
        handle.get_bounds(); // None; must not panic.

        // Rendering skips invisible overlays entirely.
        screen.borrow_mut().render_now(true);
        assert_eq!(screen.borrow().last_frame.len(), 24);
        assert!(screen.borrow().last_frame.iter().all(|line| !line.contains("ov")));
    }

    #[test]
    fn composite_overlays_positions_centered_overlay() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(80, 24);
        let overlay = TestComponent::plain("hello");
        TuiSurface::show_overlay(&mut *screen.borrow_mut(), overlay, OverlayOptions::default());

        screen.borrow_mut().render_now(true);
        let frame = screen.borrow();
        // Default width: min(80, 80) = 80 → overlay spans the full row.
        // Centered: row = floor((24-1)/2) = 11.
        assert!(frame.last_frame[11].contains("hello"), "row 11: {:?}", frame.last_frame[11]);
        assert_eq!(frame.cursor, None);
    }

    #[test]
    fn composite_overlays_respects_width_and_extract_cursor() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(40, 10);
        let editor = TestComponent::focusable("ab");
        screen.borrow_mut().add_child(editor.clone());
        TuiSurface::set_focus(&mut *screen.borrow_mut(), Some(editor));

        let options = OverlayOptions {
            width: Some(SizeValue::Number(10.0)),
            anchor: Some(OverlayAnchor::TopLeft),
            ..Default::default()
        };
        let overlay = TestComponent::focusable("XYZ");
        let handle = TuiSurface::show_overlay(&mut *screen.borrow_mut(), overlay, options);
        screen.borrow_mut().render_now(true);

        {
            let frame = screen.borrow();
            // Top-left anchor: row 0, col 0. The overlay's focused marker is
            // extracted from the composited frame.
            assert!(frame.last_frame[0].contains("XYZ"));
            assert_eq!(frame.cursor, Some((0, 3)));
        }
        let bounds = handle.get_bounds().expect("bounds");
        assert_eq!((bounds.row, bounds.col, bounds.width, bounds.height), (0, 0, 10, 1));
    }

    #[test]
    fn composite_overlays_stacks_by_focus_order() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(20, 6);
        let first = TestComponent::plain("AAA");
        let second = TestComponent::plain("BBB");
        TuiSurface::show_overlay(&mut *screen.borrow_mut(), first, OverlayOptions::default());
        // Same anchor → same position; the later handle is on top.
        let handle = TuiSurface::show_overlay(
            &mut *screen.borrow_mut(),
            second,
            OverlayOptions { width: Some(SizeValue::Number(3.0)), ..Default::default() },
        );
        handle.focus();
        screen.borrow_mut().render_now(true);
        let frame = screen.borrow();
        let row = (6 - 1) / 2;
        // "BBB" (width 3, centered at col floor((20-3)/2)=8) covers "AAA"'s
        // first cells (AAA centered at col 8, width 3 → cols 8-10).
        let line = &frame.last_frame[row];
        assert!(line.contains("BBB"));
    }

    #[test]
    fn resolve_overlay_layout_anchors_and_percentages() {
        let _lock = global_test_lock();
        let terminal: Rc<RefCell<dyn Terminal>> = Rc::new(RefCell::new(TestTerminal::new(80, 24)));
        let base = TuiBase::new(terminal, None);

        // Default: width = min(80, avail), centered.
        let (width, row, col, max_height) = base.resolve_overlay_layout(None, 3, 80, 24);
        assert_eq!((width, row, col, max_height), (80, 10, 0, None));

        // Anchors.
        let options = OverlayOptions {
            width: Some(SizeValue::Number(10.0)),
            anchor: Some(OverlayAnchor::BottomRight),
            ..Default::default()
        };
        let (_, row, col, _) = base.resolve_overlay_layout(Some(&options), 4, 80, 24);
        assert_eq!((row, col), (20, 70));

        // Percent width/height + margins + offsets + clamping.
        let options = OverlayOptions {
            width: Some(SizeValue::Percent("50%".to_string())),
            max_height: Some(SizeValue::Percent("50%".to_string())),
            margin: Some(OverlayMarginOrNumber::Number(2)),
            offset_x: Some(1),
            offset_y: Some(1),
            ..Default::default()
        };
        let (width, row, col, max_height) = base.resolve_overlay_layout(Some(&options), 10, 80, 24);
        assert_eq!(width, 40); // floor(80 * 50 / 100)
        assert_eq!(max_height, Some(12)); // floor(24 * 50 / 100), ≤ avail 20
        // avail 76x20, centered + offset: row=2+floor((20-10)/2)+1, col=2+floor((76-40)/2)+1.
        assert_eq!((row, col), (8, 21));

        // Absolute row/col + minWidth.
        let options = OverlayOptions {
            width: Some(SizeValue::Number(5.0)),
            min_width: Some(12),
            row: Some(SizeValue::Number(100.0)),
            col: Some(SizeValue::Number(3.0)),
            ..Default::default()
        };
        let (width, row, col, _) = base.resolve_overlay_layout(Some(&options), 2, 80, 24);
        assert_eq!(width, 12);
        assert_eq!(row, 22); // clamped: 24 - 0 - 2
        assert_eq!(col, 3);

        // Invalid percent strings fall back to center anchors.
        let options = OverlayOptions {
            row: Some(SizeValue::Percent("bogus%".to_string())),
            col: Some(SizeValue::Percent("".to_string())),
            ..Default::default()
        };
        let (_, row, col, _) = base.resolve_overlay_layout(Some(&options), 2, 80, 24);
        assert_eq!((row, col), (11, 0));
    }

    #[test]
    fn focus_restore_blocked_and_resumed() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(80, 24);
        let a = TestComponent::focusable("a");
        let b_typed = TestComponent::focusable_typed("b");
        let b = comp(&b_typed);
        screen.borrow_mut().add_child(a.clone());
        screen.borrow_mut().add_child(b.clone());
        TuiSurface::set_focus(&mut *screen.borrow_mut(), Some(a.clone()));

        // Show an overlay; pre-focus is A.
        let overlay = TestComponent::focusable("ov");
        let handle = TuiSurface::show_overlay(
            &mut *screen.borrow_mut(),
            overlay.clone(),
            OverlayOptions { width: Some(SizeValue::Number(4.0)), ..Default::default() },
        );
        assert!(focused_flag(&overlay));

        // Focusing B (not in the overlay's preFocus chain) blocks restore.
        TuiSurface::set_focus(&mut *screen.borrow_mut(), Some(b.clone()));
        assert!(focused_flag(&b));
        let restore = screen.borrow().base.overlay_focus_restore.clone();
        match restore {
            OverlayFocusRestoreState::Blocked { entry_id, blocked_by, resume } => {
                assert_eq!(entry_id, 0);
                assert!(blocked_by.as_ref().map(|c| Rc::ptr_eq(c, &b)).unwrap_or(false));
                assert!(matches!(resume, BlockedResume::RestoreOverlay));
            }
            _ => panic!("expected blocked restore state"),
        }

        // Input while B is focused and blockedBy === B: no refocus, B gets
        // the input, and an immediate render is queued.
        let renders_before = screen.borrow().renders;
        screen.borrow_mut().handle_terminal_input("x");
        assert_eq!(b_typed.borrow().inputs.last().map(String::as_str), Some("x"));
        screen.borrow_mut().run_pending_ticks();
        assert!(screen.borrow().renders > renders_before);
        assert!(focused_flag(&b));

        // unfocus() with an explicit target while blockedBy === focused
        // retargets the resume to that target without moving focus yet.
        handle.unfocus(Some(OverlayUnfocusOptions { target: Some(a.clone()) }));
        assert!(focused_flag(&b));
        let restore = screen.borrow().base.overlay_focus_restore.clone();
        match restore {
            OverlayFocusRestoreState::Blocked { resume, blocked_by, .. } => {
                assert!(matches!(resume, BlockedResume::FocusTarget(t) if t.as_ref().map(|t| Rc::ptr_eq(t, &a)).unwrap_or(false)));
                assert!(blocked_by.as_ref().map(|c| Rc::ptr_eq(c, &b)).unwrap_or(false));
            }
            _ => panic!("expected blocked retargeted state"),
        }

        // Focusing the now-mounted target resolves through the resume.
        TuiSurface::set_focus(&mut *screen.borrow_mut(), Some(a.clone()));
        assert!(focused_flag(&a));
        assert!(matches!(
            screen.borrow().base.overlay_focus_restore,
            OverlayFocusRestoreState::Inactive
        ));
    }

    #[test]
    fn focused_overlay_hidden_by_visibility_redirects_input_focus() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(80, 24);
        let editor_typed = TestComponent::focusable_typed("editor");
        let editor = comp(&editor_typed);
        screen.borrow_mut().add_child(editor.clone());
        TuiSurface::set_focus(&mut *screen.borrow_mut(), Some(editor.clone()));

        let visible = std::cell::Cell::new(true);
        let visible = Rc::new(visible);
        let visible_flag = visible.clone();
        let overlay = TestComponent::focusable("ov");
        let options = OverlayOptions {
            visible: Some(Rc::new(move |_cols, _rows| visible_flag.get())),
            width: Some(SizeValue::Number(4.0)),
            ..Default::default()
        };
        let _handle =
            TuiSurface::show_overlay(&mut *screen.borrow_mut(), overlay.clone(), options);
        assert!(focused_flag(&overlay));

        // The overlay's visible() flips false (e.g. resize): the next input
        // redirects focus to the pre-focus component and the input reaches
        // it.
        visible.set(false);
        screen.borrow_mut().handle_terminal_input("k");
        assert!(focused_flag(&editor));
        assert_eq!(editor_typed.borrow().inputs.last().map(String::as_str), Some("k"));
    }

    #[test]
    fn dispatch_mouse_to_overlay_hits_topmost_layout() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(80, 24);
        let overlay = TestComponent::plain("ov");
        let options = OverlayOptions {
            width: Some(SizeValue::Number(4.0)),
            anchor: Some(OverlayAnchor::TopLeft),
            ..Default::default()
        };
        TuiSurface::show_overlay(&mut *screen.borrow_mut(), overlay, options);
        screen.borrow_mut().render_now(true);

        let event = TuiMouseEvent {
            event_type: crate::component::TuiMouseEventType::Press,
            button: crate::component::TuiMouseButton::Left,
            x: 1,
            y: 0,
            screen_x: 1,
            screen_y: 0,
            width: 80,
            height: 24,
            shift: false,
            alt: false,
            ctrl: false,
            wheel_delta: None,
            click_count: None,
        };
        // TestComponent has no mouse handler: hit, no result.
        let result = screen.borrow_mut().base.dispatch_mouse_to_overlay(&event);
        assert!(matches!(result, Some(None)));

        // Outside the layout: no hit.
        let mut outside = event.clone();
        outside.screen_y = 23;
        assert!(screen.borrow_mut().base.dispatch_mouse_to_overlay(&outside).is_none());
    }

    #[test]
    fn resolve_mouse_focus_target_prefers_visible_overlay_container() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(80, 24);
        let overlay = TestComponent::plain("ov");
        let nested = TestComponent::plain("nested");
        // Put the nested component inside the overlay via a Container.
        let container = Rc::new(RefCell::new(Container::new()));
        container.borrow_mut().add_child(nested.clone());
        TuiSurface::show_overlay(
            &mut *screen.borrow_mut(),
            container,
            OverlayOptions { width: Some(SizeValue::Number(4.0)), ..Default::default() },
        );

        let target = screen.borrow().base.resolve_mouse_focus_target(&nested);
        assert!(screen
            .borrow()
            .base
            .overlay_stack
            .iter()
            .any(|entry| Rc::ptr_eq(&entry.component, &target)));

        // A plain mounted component resolves to itself.
        let plain = TestComponent::plain("plain");
        let target = screen.borrow().base.resolve_mouse_focus_target(&plain);
        assert!(Rc::ptr_eq(&target, &plain));
    }

    #[test]
    fn hide_overlay_top_restores_and_retargets_pre_focus() {
        let _lock = global_test_lock();
        let screen = TestScreen::new_rc(80, 24);
        let editor = TestComponent::focusable("editor");
        screen.borrow_mut().add_child(editor.clone());
        TuiSurface::set_focus(&mut *screen.borrow_mut(), Some(editor.clone()));

        let first = TestComponent::focusable("one");
        TuiSurface::show_overlay(&mut *screen.borrow_mut(), first.clone(), OverlayOptions::default());
        let second = TestComponent::focusable("two");
        TuiSurface::show_overlay(&mut *screen.borrow_mut(), second.clone(), OverlayOptions::default());
        assert!(focused_flag(&second));

        // Hiding the top overlay focuses the next visible overlay (not the
        // pre-focus), because getTopmostVisibleOverlay wins.
        TuiSurface::hide_overlay(&mut *screen.borrow_mut());
        assert!(focused_flag(&first));
        TuiSurface::hide_overlay(&mut *screen.borrow_mut());
        assert!(focused_flag(&editor));
    }
}
