# T4 / D1b — editor_autocomplete.rs (async pipeline + async test ports)

## Flush / timer semantics findings (the core modeling decision)

TS ground truth (editor.ts:2287-2341, 2324-2385):

- `requestAutocomplete` (sync, already in editor.rs) cancels the previous
  request, then either arms `autocompleteDebounceTimer` (setTimeout, 20ms)
  or calls `startAutocompleteRequest` immediately.
- `startAutocompleteRequest`'s IIFE suspends at `await previousTask` —
  NOTHING runs synchronously, not even the token check.
- The timer callback calls `startAutocompleteRequest`; the provider call
  happens one microtask later (after the `await previousTask` hop).
- The continuation (currency check + apply) runs when the provider promise
  resolves — for sync mock providers that is within the same microtask
  drain.
- `flushAutocomplete()` = `await Promise.resolve(); await setImmediate` —
  a FULL microtask drain. One flush therefore completes a sync-provider
  request end-to-end. **No test in the 2096-3079 block calls
  flushAutocomplete twice in a row** — each `await flushAutocomplete()`
  maps 1:1 to one `run_pending_autocomplete_requests()` call.
- Mocked-timer tests (`t.mock.timers.tick(19/1/20)`) → ported as
  `tick_autocomplete_debounce(ms)` (test-visible, `StdinBuffer::fire_timeout`
  precedent): accumulates elapsed ms against the currently armed timer
  (keyed by its start_token; re-arm resets), fires at ≥ 20ms. Firing
  STARTS the request (provider called, signal captured) but the
  continuation (currency+apply) is deferred to the next flush — mirroring
  the microtask hop.
- Real-sleep tests (`await new Promise(r => setTimeout(r, 50/250))`) map to
  `tick_autocomplete_debounce(N)` (+ flush where the TS test flushes).

## Promise-chain state machine (thread_locals, since editor.rs fields are frozen)

- `IN_FLIGHT: Option<AutocompleteContinuation>` — the suspended
  `await getSuggestions` continuation + the AbortController
  (`this.autocompleteAbort`). Holds: request_id, snapshot (text/line/col),
  force/explicit_tab, the per-request `AbortSignal`, and the provider
  result. Keyed by an editor identity (`Rc::as_ptr(&self.tui)`) to guard
  against cross-test leakage; my test helper also resets it.
- `DEBOUNCE_ELAPSED: (editor_id, start_token, elapsed_ms)`.
- The provider trait is synchronous, so the "pending promise" of
  editor.test.ts:2614-2645 ("aborts active @ autocomplete") is modeled by
  simply NEVER flushing that request; the abort is observed through the
  `AbortSignal` captured by the FakeProvider.
- **Lazy abort**: TS aborts the in-flight controller inside
  `cancelAutocompleteRequest` (editor.rs code I may not change), so the
  port aborts the previous in-flight signal when the NEXT request starts
  (or when a stale continuation is discarded at flush). Same set of
  aborted signals, observably later — only the provider-side test
  (editor.test.ts:2614) observes aborts and it fits (assert happens after
  the follow-up request's timer fires).
- `autocomplete_aborted: bool` (editor.rs) is set true on abort;
  editor.rs's `cancel_autocomplete_request` resets it (models
  `abort(); autocompleteAbort = undefined`).

## Ordering inside `run_pending_autocomplete_requests()`

1. Run the suspended continuation (tick-fired request, older) — currency
   gate, then null/empty → cancel+render; force&&explicitTab&&1 item →
   auto-apply; else `apply_autocomplete_suggestions` + render.
2. Drain `autocomplete_request_pending` (immediate requests): start
   (abort prev signal, token check, snapshot, provider call) and run the
   continuation in the same flush (microtasks drain fully). The queue
   holds at most one entry (every `request_autocomplete` cancels first).

## Notable ports / decisions

- `create_autocomplete_list`: SelectListTheme is not Clone and the Editor
  holds `Rc<SelectListTheme>`, so the list gets a forwarding theme
  (closures capturing Rc clones call the same underlying style fns).
  `on_select` stays unset — the TS onSelect body (apply completion) is
  already ported in editor_input.rs's Tab/Enter routing; a closure cannot
  borrow the owning Editor. Slash-command layout (min 12 / max 32 primary
  column) per editor.ts:248.
- `get_best_autocomplete_match_index` returns i64 (-1 = no match).
- Tests 18-22 (Enter semantics) work because applying an item whose value
  equals the typed prefix is an identity operation — no editor_input
  changes needed.
- Test 24 ("ignores invalid slash command argument completion results"):
  TS's runtime-invalid return ("not-an-array") maps to the typed Rust
  `None` from `GetArgumentCompletions`.
- Test 7 uses a scripted getSuggestions + the REAL
  CombinedAutocompleteProvider's applyCompletion (delegated), base "."
  (apply/shouldTrigger are pure text ops — no FS).
- Test 5 uses a real temp dir (created/cleaned by a Drop guard).

## Test list (editor.test.ts → editor_autocomplete.rs)

| TS lines | name |
|---|---|
| 2096 | undoes_autocomplete |
| 2130 | triggers_and_debounces_symbol_completion_after_cjk_punctuation |
| 2168 | does_not_auto_trigger_after_cjk_letters_or_for_unprefixed_paths |
| 2207 | requests_path_completion_after_cjk_punctuation_only_on_tab |
| 2229 | completes_chinese_path_prefixes_after_whitespace_or_cjk_punctuation_with_tab |
| 2262 | ends_unquoted_trigger_and_debounce_contexts_at_whitespace_or_cjk_punctuation |
| 2300 | re_triggers_cjk_path_completion_after_accepting_directories_and_deleting |
| 2348 | auto_applies_single_force_file_suggestion_without_showing_menu |
| 2389 | shows_menu_when_force_file_has_multiple_suggestions |
| 2433 | keeps_suggestions_open_when_typing_in_force_mode |
| 2488 | debounces_at_autocomplete_while_typing |
| 2519 | re_queries_autocomplete_picker_when_cursor_moves_back_into_command_name |
| 2562 | debounces_hash_autocomplete_while_typing |
| 2584 | debounces_custom_trigger_characters_autocomplete_while_typing |
| 2589(c) | resets_custom_trigger_characters_when_provider_changes |
| 2614 | aborts_active_at_autocomplete_when_typing_continues |
| 2647 | hides_autocomplete_when_backspacing_slash_command_to_empty |
| 2678 | applies_exact_typed_slash_argument_value_on_enter… |
| 2725 | selects_first_prefix_match_on_enter_when_typed_arg_is_not_exact_match |
| 2797 | highlights_unique_prefix_match_as_user_types… |
| 2865 | selects_first_prefix_match_when_multiple_items_match |
| 2910 | works_for_builtin_style_command_argument_completion_path_model_like |
| 2966 | awaits_async_slash_command_argument_completions |
| 3000 | ignores_invalid_slash_command_argument_completion_results |
| 3024 | does_not_show_argument_completions_when_command_has_no_argument_completer |
| 1820 | does_not_trigger_autocomplete_during_single_line_paste |

## Progress log

- [x] Read plan/resume, stub, editor.rs sync surface, TS 2218-2473.
- [x] Read TS test block 2096-3080 + 1820-1834; traced flush/tick semantics.
- [x] Implementation written (this file's impl section).
- [ ] Compile + full test run.
