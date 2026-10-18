# T5 Redteam R1 — Overlay / focus state machine

Date: 2026-10-18. Reviewer: coordinator. Method: line-by-line TS (tui.ts)
vs Rust (crates/pi-tui/src/tui_overlay.rs, tui.rs) comparison of every
overlay/focus entry point; TS source treated as ground truth; differential
coverage cross-checked against parity/wavec/interaction-cases.mjs.

## Scope reviewed

| TS (tui.ts) | Rust | Verdict |
|---|---|---|
| setFocus / setFocusInternal (598-666) | TuiBase::set_focus / set_focus_internal (tui_overlay.rs:58-169) | MATCH |
| clearOverlayFocusRestore[For] (668-676) | clear_overlay_focus_restore[_for] (172-184) | MATCH |
| resolveBlockedOverlayFocusResume (678-682) | resolve_blocked_overlay_focus_resume (186-199) | MATCH |
| getVisibleOverlayFocusRestore (684-690) | get_visible_overlay_focus_restore (201-215) | MATCH |
| isOverlayFocusAncestor (692-703) | is_overlay_focus_ancestor (217-239) | MATCH (visited-set walk) |
| retargetOverlayPreFocus (705-711) | retarget_overlay_pre_focus (241-255) | MATCH |
| isComponentMounted / containsComponent (713-721) | is_component_mounted / contains_component | MATCH |
| showOverlay (729-776) | show_overlay (336-372) | MATCH (preFocus snapshot, nonCapturing+visible gate, hideTerminalCursor, requestRender) |
| handle.hide() (779-795) | hide_overlay_entry/_at_index (374-408) | MATCH (clear-restore-for, retarget, splice-at-index, focus restore to topmost-visible ?? preFocus, empty-stack cursor hide) |
| handle.setHidden() (796-817) | set_overlay_hidden (410-444) | MATCH (incl. focusOrder re-bump on unhide) |
| handle.focus() / unfocus() (819-846) | focus_overlay_entry / unfocus_overlay_entry (451-513) | MATCH — unfocus uses the RAW overlayFocusRestore (not the visible-filtered state), matching TS; blocked+focusedBy branch, fallbackTarget = topVisible≠entry ? topVisible : preFocus |
| handle.isFocused() / getBounds() | is_overlay_entry_focused / overlay_entry_bounds (515-533) | MATCH |
| hideOverlay() (845-860) | hide_overlay_top (374-381) | MATCH |
| isOverlayVisible (906-913) | is_overlay_visible[_index] (267-290) | MATCH (hidden → false; options.visible(cols,rows) callback; else true) |
| getTopmostVisibleOverlay (915-923) | get_topmost_visible_overlay (292-309) | MATCH (skips nonCapturing + invisible; max focusOrder) |
| hasOverlay / isOverlayFocused | has_visible_overlay | MATCH |
| resolveMouseFocusTarget (879-887) | resolve_mouse_focus_target (536-549) | MATCH (reverse stack walk, visible + containsComponent) |
| dispatchMouseToOverlay (889-905) | dispatch_mouse_to_overlay (551-583) | MATCH (reverse renderedOverlayLayouts, bounds hit test, local retarget, focus→focusTarget=overlay component) |
| compositeOverlays (1344-1424) | composite_overlays (746-829) | MATCH — height=0 pre-pass for width/maxHeight, fake-cursor resolve, maxHeight slice, bounds record, focusOrder-sorted, workingHeight = max(lines, termHeight, minLinesNeeded) (no maxLinesRendered), viewportStart, defensive width truncation (tui.ts:1409-1412 has the identical "defensive" truncate — NOT a Rust invention) |
| handleInput overlay redirect (1085-1131) | tui.rs handle_input tail (945-1010) | MATCH — focused-overlay visibility re-check (topVisible ?? preFocus w/ preserve), eligible/blocked resume resolution, focused-component dispatch with key-release filter (wantsKeyRelease), requestImmediateRender |

## Detailed findings

1. **setFocusInternal null-branch else-if structure** — initially
   suspected a divergence (Rust nests the policy==Clear check inside the
   Blocked arm). Traced both truth tables: identical. TS
   `blocked && blockedBy===previous ? resume : (policy===clear ? clear : noop)`
   == Rust's `if Blocked { if match {resume} else if Clear {clear} } else if Clear {clear}`.
   No fix needed.
2. **unfocus pending-restore predicate** — TS uses the raw
   `this.overlayFocusRestore` (not `getVisibleOverlayFocusRestore()`);
   Rust clones `self.overlay_focus_restore` directly. Correct.
3. **hideTerminalCursor stopped-gate** — TS `if (!this.stopped)
   this.terminal.hideCursor()`; Rust identical (tui.rs:645-650).
4. **focusOrder bump points** — showOverlay (push), setHidden(false)
   re-show, focus(). All three present in Rust with the same
   `++focusOrderCounter` pre-increment semantics.
5. **No unported TS surface found.** All OverlayHandle methods (hide,
   setHidden, isHidden, focus, unfocus, isFocused, getBounds) have Rust
   id-dispatched counterparts.

## Differential coverage

interaction-cases.mjs exercises this machine end-to-end on both
implementations with byte-identical write streams: nc-input-routes-to-
editor, nc-hide-topmost-keeps-capture, nc-capture-hide-restores-editor,
nc-overlay-restore-chain (11 frames of nested hide/restore), nc-base-
steal-restore, nc-setfocus-null-drops-input, plus 12 overlay geometry
cases and 2 style-leak cases. All 665/665 parity cases pass.

## Verdict

**No divergences found. R1 clean.** No code changes made.
