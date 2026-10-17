# T3 (layout + components) — WIND-DOWN / RESUME RECORD

Written 2026-10-17 when pausing T3 mid-flight. Read this + TRACKING.md +
docs/TUI_PORT_PLAN.md before resuming.

## Current state (uncommitted → committed as WIP on this commit)

- pi-tui: **573 passed, 6 failed, 1 ignored**. Parity: **434/434 green**.
- All T3 modules LANDED (fleet-built via subagent delegates):

| File | LOC | Status |
|---|---|---|
| component.rs (foundation: Component trait, ComponentRef=Rc<RefCell<dyn Component>>, mouse types, Container, dispatch, CURSOR_MARKER/FAKE_CURSOR, composite_tui_line, is_image_line) | ~750 | done, tests green |
| layout_node.rs (LayoutNode/StackLayoutEntry/ScrollLayoutState types) | 101 | done |
| layout.rs (flex solver + LayoutFrame tree; B1 delegate died with 2 compile errors, fixed by coordinator) | 2144 | tests green |
| components/stack.rs + h_stack.rs + v_stack.rs (allocate_stack_sizes/visible_stack_entries API contract with layout.rs) | 782+393+335 | tests green |
| components/text, truncated_text, spacer, box_component, loader, cancellable_loader | 341+248+94+499+399+226 | tests green |
| components/mouse_region.rs, scroll_view.rs | 333+1331 | tests green |
| components/input.rs (Input + Focusable; API surface documented in /tmp/report-t3-input.md) | 1443 | tests green |
| components/select_list.rs, settings_list.rs | 669+723 | 1 test failing |
| components/autocomplete.rs | 2795 | 5 tests failing |

Foundation design decisions (binding for T4/T5 — see component.rs doc header):
- `render(&mut self, width: usize) -> Vec<String>` (TS mutates caches in render).
- `handle_mouse(&mut self, self_ref: &ComponentRef, event, ...)` — self_ref is
  the TS `this` (containers set focus_target = self).
- `handles_input() -> bool` models TS optional-method PRESENCE.
- Mouse numerics i64 (Container checks `y < 0`); result flags Option<bool>.
- `layout_node()` default None models the `[LAYOUT_NODE]` symbol capability.

## The 6 failing tests (delegates died mid-fix; diagnosis in progress)

**autocomplete ×5** (all return None where TS returns Some — unwrap panic at
autocomplete.rs:2083 in test 1):
1. `completes_quoted_cjk_attachments_after_prose_without_losing_path_segments_or_quotes`
2. `quotes_paths_containing_whitespace_or_cjk_punctuation_for_direct_completion`
3. `quotes_paths_containing_whitespace_or_cjk_punctuation_for_at_suggestions`
4. `returns_symlinked_files_without_requiring_type_l`
5. `treats_unquoted_separators_as_boundaries_even_when_a_matching_literal_path_exists`

- 43/48 autocomplete tests pass (incl. many quoted/CJK cases), so the bug is
  narrow. TS references: autocomplete.test.ts:205, 466, 597.
- Hints gathered: TS test at :466 uses `symlinkSync` — check the Rust FakeFs
  supports symlinks. TS test at :599 constructs `CombinedAutocompleteProvider([], baseDir)`
  WITHOUT requireFdPath() — check the Rust test's make_provider flags match.
  cursor_col helpers (`char_col`/`char_index_of`) count CHARS; JS `indexOf`
  returns UTF-16 code units — BMP-only tests coincide, but verify the engine's
  cursor_col unit convention (should be UTF-16 units per JS) before fixing.

**select_list ×1**: `uses_the_configured_minimum_primary_column_width` —
delegate's last message: "JS indexOf returns a UTF-16 code-unit index ('→' is
3 bytes but 1 UTF-16 unit). The test must measure in UTF-16 units." Fix: the
test helper must count UTF-16 units (or the engine column semantics are
UTF-16 — check which side is wrong vs TS).

## Remaining T3 work (in order)

1. Fix the 6 failing tests above (debug via TS probes: node, stderr→/dev/null,
   absolute imports; compare provider outputs on the same fixture setup).
2. Full `cargo test -p pi-tui --lib` + `cargo test --workspace` green.
3. Add `tui-components` parity group (render-line differential): construct
   components by type+params from case JSON in the pi-cli harness, render at
   widths, compare Vec<String> lines vs TS. Cover Text/TruncatedText/Spacer/
   Box/Stacks/Loader/Input/ScrollView/SelectList/SettingsList incl. stateful
   scripts (handle_input sequences → re-render) for Input/SelectList. Follow
   the tui-keys/tui-stdin-buffer conventions in parity/cases.mjs +
   ts-harness.js + crates/pi-cli/src/main.rs.
4. T3 redteams (adversarial fidelity audit per component + render probes),
   fix findings (fleet pattern: builder delegates own single files; redteams
   read-only, reports to /tmp/report-*.md).
5. Update TRACKING.md changelog (T3 COMPLETE entry), commit + push.

## Fleet operational notes (reuse for T4–T6)

- Delegates reliably DO the work but the result channel often fails
  ("Other"/max-turns). ALWAYS: (a) instruct them to write reports
  incrementally to /tmp/report-<task>.md, (b) verify via the working tree +
  test runs, not the return message, (c) fix their leftover compile errors
  yourself when the file owner is dead.
- Strict file ownership per delegate; coordinator owns lib.rs/mod.rs
  registrations + shared foundations + API contracts between parallel
  delegates (e.g. stack.rs ↔ layout.rs contract worked well).
- Wave structure that worked: shared-foundation prep (coordinator) →
  independent builders in parallel → dependent builders next wave →
  redteams → fixer delegate for redteam findings → coordinator final gate
  (full tests + parity + TRACKING.md + commit/push).
- Pending T4+: editor.rs (2473 LOC, biggest), editor-component.ts (74);
  T5 tui.rs renderer + alt-screen; T6 markdown/latex/terminal-image;
  T7 batter. Settings/config flows need the keybindings global-manager
  rebind pattern noted in TRACKING.md (Arc immutability divergence).
