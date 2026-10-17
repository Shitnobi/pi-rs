//! The async autocomplete request pipeline (editor.ts:2218-2242,
//! 2324-2442): `getBestAutocompleteMatchIndex`, `createAutocompleteList`,
//! `startAutocompleteRequest`, `runAutocompleteRequest`,
//! `isAutocompleteRequestCurrent`, and `applyAutocompleteSuggestions`.
//!
//! The synchronous guard chain (request scheduling, debounce pattern,
//! cancellation) lives in `editor.rs`. The TS promise chain is modeled as
//! an explicit state machine:
//!
//! - `request_autocomplete` (editor.rs) either arms
//!   `autocomplete_debounce_timer` (the 20ms `setTimeout`) or pushes onto
//!   `autocomplete_request_pending` (the immediately-invoked
//!   `startAutocompleteRequest`).
//! - [`Editor::tick_autocomplete_debounce`] models the timer firing
//!   (`t.mock.timers.tick(N)` / a real sleep): the request STARTS — token
//!   check, per-request `AbortSignal`, snapshot, synchronous provider
//!   call — and the continuation (currency gate + apply) is suspended,
//!   exactly like TS awaiting the provider promise.
//! - [`Editor::run_pending_autocomplete_requests`] models
//!   `flushAutocomplete()` (`await Promise.resolve(); setImmediate` — a
//!   full microtask drain): the suspended continuation runs, and any
//!   immediately-scheduled request starts and completes.
//!
//! The suspended continuation and the mock-timer elapsed time live in
//! thread-locals keyed by an editor identity (`Rc::as_ptr` of its `tui`),
//! since the `Editor` struct fields are frozen by earlier waves. Tests
//! reset the slots through their editor factory.

use std::cell::RefCell;
use std::rc::Rc;

use crate::components::autocomplete::{
    AbortSignal, AutocompleteItem, AutocompleteSuggestions, SuggestionsOptions,
};
use crate::components::select_list::{
    SelectItem, SelectList, SelectListLayoutOptions, SelectListTheme,
};

use super::editor::{
    AutocompleteMode, Editor, PendingAutocompleteRequest, ATTACHMENT_AUTOCOMPLETE_DEBOUNCE_MS,
};

/// The suspended tail of a started request — the Rust model of the TS
/// `await this.autocompleteProvider.getSuggestions(...)` suspension point
/// (editor.ts:2391-2396) plus the `autocompleteAbort` controller
/// (editor.ts:2333-2334).
struct AutocompleteContinuation {
    /// Identity of the owning editor — guards the thread-local slot
    /// against cross-editor leakage when tests run in parallel threads.
    editor_id: usize,
    request_id: u64,
    snapshot_text: String,
    snapshot_line: usize,
    snapshot_col: usize,
    force: bool,
    explicit_tab: bool,
    /// The per-request `AbortController`'s signal, shared with the
    /// provider call (`options.signal`).
    signal: AbortSignal,
    /// The provider's synchronous result — the resolved value of the TS
    /// promise. `None` is TS `null`.
    result: Option<AutocompleteSuggestions>,
}

thread_local! {
    /// `this.autocompleteRequestTask` (the latest suspended continuation).
    /// At most one slot: every new request first cancels the previous one
    /// (editor.ts:2294), so older continuations are always stale.
    static IN_FLIGHT: RefCell<Option<AutocompleteContinuation>> = RefCell::new(None);
    /// Accumulated mock-timer time for the currently armed debounce timer:
    /// `(editor_id, armed start_token, elapsed ms)`. Re-arming (a new
    /// start token) resets the accumulation, like a fresh `setTimeout`.
    static DEBOUNCE_ELAPSED: RefCell<(usize, u64, u64)> = RefCell::new((0, 0, 0));
}

/// Build an owned `SelectListTheme` that forwards every style hook to the
/// editor's shared theme (`Rc<SelectListTheme>`). `SelectListTheme` is not
/// `Clone` (it holds `Box<dyn Fn>` hooks), so the owned list receives
/// forwarding closures that call the same underlying functions — the Rust
/// shape of TS passing the same theme object reference (editor.ts:2241).
fn forwarding_select_list_theme(shared: &Rc<SelectListTheme>) -> SelectListTheme {
    let selected_prefix = shared.clone();
    let selected_text = shared.clone();
    let description = shared.clone();
    let scroll_info = shared.clone();
    let no_match = shared.clone();
    SelectListTheme {
        selected_prefix: Box::new(move |t| (selected_prefix.selected_prefix)(t)),
        selected_text: Box::new(move |t| (selected_text.selected_text)(t)),
        description: Box::new(move |t| (description.description)(t)),
        scroll_info: Box::new(move |t| (scroll_info.scroll_info)(t)),
        no_match: Box::new(move |t| (no_match.no_match)(t)),
    }
}

impl Editor {
    /// Identity of this editor instance for the thread-local pipeline
    /// state (the `tui` `Rc` allocation address — unique among live
    /// editors).
    fn autocomplete_editor_id(&self) -> usize {
        Rc::as_ptr(&self.tui).cast::<()>() as usize
    }

    // -------------------------------------------------------------------------
    // Public flush surface (test / host driven; the TS flushAutocomplete)
    // -------------------------------------------------------------------------

    /// Fire the pending debounced / queued autocomplete request(s) — the
    /// Rust model of the `setTimeout` callback → `startAutocompleteRequest`
    /// promise chain (editor.ts:2297-2301, 2324-2341, 2387-2442).
    ///
    /// `flushAutocomplete()` in the TS tests is a full microtask drain
    /// (`await Promise.resolve(); await setImmediate`), so one call runs a
    /// suspended continuation to completion AND starts + completes any
    /// immediately-scheduled request.
    pub fn run_pending_autocomplete_requests(&mut self) {
        // Older first: a continuation suspended by a debounce-timer fire
        // (editor.ts:2297-2301) precedes anything scheduled after it.
        if let Some((request_id, force, explicit_tab)) = self.peek_in_flight_continuation() {
            self.run_autocomplete_request(request_id, force, explicit_tab);
        }
        while !self.autocomplete_request_pending.is_empty() {
            let request = self.autocomplete_request_pending.remove(0);
            self.start_autocomplete_request(request);
            // Same microtask drain: the freshly started request's
            // continuation also completes within this flush.
            if let Some((request_id, force, explicit_tab)) = self.peek_in_flight_continuation() {
                self.run_autocomplete_request(request_id, force, explicit_tab);
            }
        }
    }

    /// Advance the debounce timer by `ms` milliseconds — the model of the
    /// TS tests' `t.mock.timers.tick(N)` / real `setTimeout` sleeps. When
    /// the armed `autocomplete_debounce_timer` (editor.ts:2297-2301)
    /// expires, the request starts exactly like the TS callback: the timer
    /// field is cleared and `startAutocompleteRequest` runs (up to and
    /// including the provider call), leaving the continuation suspended
    /// for the next [`Editor::run_pending_autocomplete_requests`].
    pub fn tick_autocomplete_debounce(&mut self, ms: u64) {
        let editor_id = self.autocomplete_editor_id();
        let Some(armed) = self.autocomplete_debounce_timer.clone() else {
            return;
        };
        let elapsed = DEBOUNCE_ELAPSED.with(|state| {
            let mut state = state.borrow_mut();
            if state.0 != editor_id || state.1 != armed.start_token {
                *state = (editor_id, armed.start_token, 0);
            }
            state.2 = state.2.saturating_add(ms);
            state.2
        });
        if elapsed >= ATTACHMENT_AUTOCOMPLETE_DEBOUNCE_MS {
            // setTimeout callback: `this.autocompleteDebounceTimer =
            // undefined; void this.startAutocompleteRequest(...)`.
            self.autocomplete_debounce_timer = None;
            self.start_autocomplete_request(armed);
        }
    }

    // -------------------------------------------------------------------------
    // The pipeline (editor.ts:2324-2385, 2387-2442)
    // -------------------------------------------------------------------------

    /// `startAutocompleteRequest(startToken, options)` (editor.ts:2324-2341)
    /// plus the synchronous prefix of `runAutocompleteRequest`
    /// (editor.ts:2387-2396): abort the previous in-flight controller,
    /// validate the token, create the per-request abort signal, snapshot
    /// the editor state, call the (synchronous) provider, and suspend the
    /// continuation.
    fn start_autocomplete_request(&mut self, request: PendingAutocompleteRequest) {
        // `requestAutocomplete` always runs `cancelAutocompleteRequest`
        // first (editor.ts:2294), which aborts the in-flight controller.
        // The sync port aborts lazily at the next request start — the same
        // set of signals ends up aborted, only observably later (only the
        // provider can observe an abort; see editor.test.ts:2614).
        self.abort_in_flight_autocomplete();

        // editor.ts:2330-2332 (after the `await previousTask` hop).
        if request.start_token != self.autocomplete_start_token {
            return;
        }
        let Some(provider) = self.autocomplete_provider.clone() else {
            return;
        };

        // editor.ts:2333-2338.
        let signal = AbortSignal::new();
        self.autocomplete_request_id += 1;
        let request_id = self.autocomplete_request_id;
        let snapshot_text = self.get_text();
        let snapshot_line = self.state.cursor_line;
        let snapshot_col = self.state.cursor_col;

        // editor.ts:2391-2396 — the provider is called with the CURRENT
        // state (not the snapshot). The trait is synchronous, so the
        // resolved value is available immediately; the continuation stays
        // suspended until the flush.
        let options = SuggestionsOptions {
            signal: signal.clone(),
            force: request.force,
        };
        let result = provider.get_suggestions(
            &self.state.lines,
            self.state.cursor_line,
            self.state.cursor_col,
            &options,
        );

        let editor_id = self.autocomplete_editor_id();
        IN_FLIGHT.with(|slot| {
            *slot.borrow_mut() = Some(AutocompleteContinuation {
                editor_id,
                request_id,
                snapshot_text,
                snapshot_line,
                snapshot_col,
                force: request.force,
                explicit_tab: request.explicit_tab,
                signal,
                result,
            });
        });
    }

    /// `runAutocompleteRequest(...)` (editor.ts:2387-2442) — the
    /// continuation after `await getSuggestions(...)`: the currency gate,
    /// then cancel-on-empty, single-suggestion force auto-apply, or
    /// `applyAutocompleteSuggestions`.
    pub(crate) fn run_autocomplete_request(&mut self, request_id: u64, force: bool, explicit_tab: bool) {
        let editor_id = self.autocomplete_editor_id();
        let continuation = IN_FLIGHT.with(|slot| {
            let mut slot = slot.borrow_mut();
            match slot.take() {
                Some(cont) if cont.editor_id == editor_id && cont.request_id == request_id => {
                    Some(cont)
                }
                // Not ours (a stale slot from another editor): put it back.
                other => {
                    *slot = other;
                    None
                }
            }
        });
        let Some(continuation) = continuation else {
            return;
        };

        // editor.ts:2398-2400: the currency gate. (A signal is only ever
        // aborted after the token was invalidated, so the abort flag is
        // implied by the request-id check — kept explicit for parity.)
        if continuation.signal.aborted()
            || !self.is_autocomplete_request_current(
                continuation.request_id,
                &continuation.snapshot_text,
                continuation.snapshot_line,
                continuation.snapshot_col,
            )
        {
            // TS aborted this controller inside `cancelAutocompleteRequest`
            // (editor.ts:2448); the sync port aborts lazily here (or at
            // the next request start, whichever comes first).
            if !continuation.signal.aborted() {
                continuation.signal.abort();
                self.autocomplete_aborted = true;
            }
            return;
        }

        // editor.ts:2402: `this.autocompleteAbort = undefined` (the slot
        // was already taken above).

        // editor.ts:2404-2408: no suggestions → hide and re-render.
        let Some(suggestions) = continuation.result else {
            self.cancel_autocomplete();
            self.tui.borrow_mut().request_render();
            return;
        };
        if suggestions.items.is_empty() {
            self.cancel_autocomplete();
            self.tui.borrow_mut().request_render();
            return;
        }

        // editor.ts:2410-2426: force + explicit Tab with a single
        // suggestion auto-applies without showing the menu.
        if force && explicit_tab && suggestions.items.len() == 1 {
            let item = suggestions.items[0].clone();
            let Some(provider) = self.autocomplete_provider.clone() else {
                return;
            };
            self.push_undo_snapshot();
            self.last_action = None;
            let result = provider.apply_completion(
                &self.state.lines,
                self.state.cursor_line,
                self.state.cursor_col,
                &item,
                &suggestions.prefix,
            );
            self.state.lines = result.lines;
            self.state.cursor_line = result.cursor_line;
            self.set_cursor_col(result.cursor_col);
            if self.on_change.is_some() {
                let text = self.get_text();
                if let Some(on_change) = self.on_change.as_mut() {
                    on_change(&text);
                }
            }
            self.tui.borrow_mut().request_render();
            return;
        }

        // editor.ts:2428-2430.
        let state = if force {
            AutocompleteMode::Force
        } else {
            AutocompleteMode::Regular
        };
        self.apply_autocomplete_suggestions(&suggestions, state);
        self.tui.borrow_mut().request_render();
    }

    /// `isAutocompleteRequestCurrent(...)` (editor.ts:2416-2430).
    pub(crate) fn is_autocomplete_request_current(
        &self,
        request_id: u64,
        snapshot_text: &str,
        snapshot_line: usize,
        snapshot_col: usize,
    ) -> bool {
        request_id == self.autocomplete_request_id
            && self.get_text() == snapshot_text
            && self.state.cursor_line == snapshot_line
            && self.state.cursor_col == snapshot_col
    }

    /// `applyAutocompleteSuggestions(suggestions, state)`
    /// (editor.ts:2432-2442).
    pub(crate) fn apply_autocomplete_suggestions(
        &mut self,
        suggestions: &AutocompleteSuggestions,
        state: AutocompleteMode,
    ) {
        self.autocomplete_prefix = suggestions.prefix.clone();
        self.create_autocomplete_list(&suggestions.prefix, &suggestions.items);

        let best_match_index =
            self.get_best_autocomplete_match_index(&suggestions.items, &suggestions.prefix);
        if best_match_index >= 0 {
            if let Some(list) = self.autocomplete_list.as_mut() {
                list.set_selected_index(best_match_index);
            }
        }

        self.autocomplete_state = Some(state);
    }

    /// `createAutocompleteList(prefix, items)` (editor.ts:2236-2260) —
    /// builds the owned `SelectList` with the editor's select-list theme
    /// and the slash-command column layout for `/`-prefixed completions.
    ///
    /// Divergence: TS wires `list.onSelect` to the completion handler
    /// (editor.ts:2242-2259); the Rust port applies the identical body
    /// from `editor_input.rs`'s Tab / Enter routing instead, because a
    /// stored closure cannot borrow the owning `Editor`. `on_select`
    /// therefore stays unset.
    pub(crate) fn create_autocomplete_list(&mut self, prefix: &str, items: &[AutocompleteItem]) {
        // editor.ts:2240 + 248-251.
        let layout = if prefix.starts_with('/') {
            SelectListLayoutOptions {
                min_primary_column_width: Some(12),
                max_primary_column_width: Some(32),
                truncate_primary: None,
            }
        } else {
            SelectListLayoutOptions::default()
        };

        let select_items: Vec<SelectItem> = items
            .iter()
            .map(|item| SelectItem {
                value: item.value.clone(),
                label: item.label.clone(),
                description: item.description.clone(),
            })
            .collect();

        let theme = forwarding_select_list_theme(&self.select_list_theme);
        let list = SelectList::with_layout(
            select_items,
            self.autocomplete_max_visible,
            theme,
            layout,
        );
        self.autocomplete_list = Some(list);
    }

    /// `getBestAutocompleteMatchIndex(items, prefix)` (editor.ts:2218-2234):
    /// an exact match always wins, otherwise the first value that starts
    /// with the prefix. `-1` when there is no match (or no prefix).
    pub(crate) fn get_best_autocomplete_match_index(
        &self,
        items: &[AutocompleteItem],
        prefix: &str,
    ) -> i64 {
        if prefix.is_empty() {
            return -1;
        }

        let mut first_prefix_index: i64 = -1;
        for (i, item) in items.iter().enumerate() {
            if item.value == prefix {
                return i as i64; // Exact match always wins
            }
            if first_prefix_index == -1 && item.value.starts_with(prefix) {
                first_prefix_index = i as i64;
            }
        }
        first_prefix_index
    }

    // -------------------------------------------------------------------------
    // Thread-local plumbing
    // -------------------------------------------------------------------------

    /// `(requestId, force, explicitTab)` of this editor's suspended
    /// continuation, if any.
    fn peek_in_flight_continuation(&self) -> Option<(u64, bool, bool)> {
        let editor_id = self.autocomplete_editor_id();
        IN_FLIGHT.with(|slot| {
            slot.borrow()
                .as_ref()
                .filter(|cont| cont.editor_id == editor_id)
                .map(|cont| (cont.request_id, cont.force, cont.explicit_tab))
        })
    }

    /// Abort and drop this editor's suspended continuation — the lazy
    /// model of `cancelAutocompleteRequest`'s
    /// `this.autocompleteAbort?.abort()` (editor.ts:2448).
    fn abort_in_flight_autocomplete(&mut self) {
        let editor_id = self.autocomplete_editor_id();
        let stale = IN_FLIGHT.with(|slot| {
            let mut slot = slot.borrow_mut();
            match slot.take() {
                Some(cont) if cont.editor_id == editor_id => Some(cont),
                other => {
                    *slot = other;
                    None
                }
            }
        });
        if let Some(continuation) = stale {
            if !continuation.signal.aborted() {
                continuation.signal.abort();
                self.autocomplete_aborted = true;
            }
        }
    }
}

// =============================================================================
// Tests — the async autocomplete block of editor.test.ts (2096-3079) plus
// the provider-paste test (1820-1834).
// =============================================================================

#[cfg(test)]
mod tests {
    use crate::component::Component;
    use crate::components::autocomplete::{
        AutocompleteItem, AutocompleteProvider, AutocompleteSuggestions, CommandEntry,
        CombinedAutocompleteProvider, CompletionResult, SlashCommand, SuggestionsOptions,
    };
    use crate::components::editor::{js_index_of, slice_utf16, Editor, EditorTheme};
    use crate::test_support::create_test_tui;
    use crate::utils::{strip_terminal_sequences, utf16_len};

    use std::cell::RefCell;
    use std::rc::Rc;

    use super::editor_autocomplete_tests_support as support;

    // -------------------------------------------------------------------------
    // Test helpers
    // -------------------------------------------------------------------------

    /// Reset the thread-local pipeline state, then build a fresh editor —
    /// the port of `new Editor(createTestTUI(), defaultEditorTheme)`.
    /// Every editor in this module is created through this factory so no
    /// suspended continuation leaks between tests sharing a thread.
    fn make_editor() -> Editor {
        support::reset_pipeline_state();
        Editor::new(create_test_tui(), EditorTheme::default())
    }

    /// `flushAutocomplete()` (editor.test.ts:40-43) — one microtask drain.
    fn flush(editor: &mut Editor) {
        editor.run_pending_autocomplete_requests();
    }

    /// Port of the shared `applyCompletion` test helper
    /// (editor.test.ts:20-38): replaces the prefix before the cursor with
    /// `item.value`. UTF-16 code-unit semantics throughout.
    fn apply_completion(
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> CompletionResult {
        let line = lines.get(cursor_line).cloned().unwrap_or_default();
        let prefix_len = utf16_len(prefix);
        let before = slice_utf16(&line, 0, cursor_col.saturating_sub(prefix_len));
        let after = slice_utf16(&line, cursor_col, utf16_len(&line));
        let mut new_lines = lines.to_vec();
        if cursor_line < new_lines.len() {
            new_lines[cursor_line] = format!("{}{}{}", before, item.value, after);
        }
        CompletionResult {
            lines: new_lines,
            cursor_line,
            cursor_col: cursor_col.saturating_sub(prefix_len) + utf16_len(&item.value),
        }
    }

    fn item(value: &str, label: &str) -> AutocompleteItem {
        AutocompleteItem {
            value: value.to_string(),
            label: label.to_string(),
            description: None,
        }
    }

    fn suggestions(items: Vec<AutocompleteItem>, prefix: &str) -> AutocompleteSuggestions {
        AutocompleteSuggestions {
            items,
            prefix: prefix.to_string(),
        }
    }

    /// What the TS mock `getSuggestions` closures see (editor state at
    /// call time).
    struct CallInfo {
        lines: Vec<String>,
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
    }

    impl CallInfo {
        /// `lines[cursorLine]!.slice(0, cursorCol)` — UTF-16 units.
        fn before(&self) -> String {
            slice_utf16(
                self.lines.get(self.cursor_line).map(|s| s.as_str()).unwrap_or(""),
                0,
                self.cursor_col,
            )
        }
    }

    type Responder = Box<dyn Fn(&CallInfo) -> Option<AutocompleteSuggestions>>;
    type Applier = Box<dyn Fn(&[String], usize, usize, &AutocompleteItem, &str) -> CompletionResult>;

    /// Scriptable fake `AutocompleteProvider` (the TS tests' inline mock
    /// object literals). `Rc` + `RefCell` interior mutability lets tests
    /// both script responses by prefix and count `getSuggestions` calls
    /// (the TS `suggestionCalls` / `requests` counters).
    struct FakeProvider {
        state: RefCell<FakeProviderState>,
    }

    struct FakeProviderState {
        trigger_characters: Vec<char>,
        /// Recorded `getSuggestions` calls: (text before cursor, force).
        calls: Vec<(String, bool)>,
        /// Every `AbortSignal` handed to `getSuggestions` — the channel
        /// through which the "aborts active @ autocomplete" test observes
        /// controller aborts (editor.test.ts:2628-2637).
        signals: Vec<crate::components::autocomplete::AbortSignal>,
        responder: Responder,
        applier: Applier,
    }

    impl FakeProvider {
        fn new(responder: impl Fn(&CallInfo) -> Option<AutocompleteSuggestions> + 'static) -> Rc<Self> {
            Self::build(Vec::new(), Box::new(responder), Box::new(apply_completion))
        }

        fn with_triggers(
            trigger_characters: Vec<char>,
            responder: impl Fn(&CallInfo) -> Option<AutocompleteSuggestions> + 'static,
        ) -> Rc<Self> {
            Self::build(trigger_characters, Box::new(responder), Box::new(apply_completion))
        }

        fn with_applier(
            responder: impl Fn(&CallInfo) -> Option<AutocompleteSuggestions> + 'static,
            applier: impl Fn(&[String], usize, usize, &AutocompleteItem, &str) -> CompletionResult + 'static,
        ) -> Rc<Self> {
            Self::build(Vec::new(), Box::new(responder), Box::new(applier))
        }

        fn build(trigger_characters: Vec<char>, responder: Responder, applier: Applier) -> Rc<Self> {
            Rc::new(Self {
                state: RefCell::new(FakeProviderState {
                    trigger_characters,
                    calls: Vec::new(),
                    signals: Vec::new(),
                    responder,
                    applier,
                }),
            })
        }

        fn calls(&self) -> Vec<(String, bool)> {
            self.state.borrow().calls.clone()
        }

        fn call_count(&self) -> usize {
            self.state.borrow().calls.len()
        }

        /// How many captured signals ended up aborted — the TS `aborts`
        /// counter (editor.test.ts:2616-2637).
        fn aborted_signal_count(&self) -> usize {
            self.state.borrow().signals.iter().filter(|s| s.aborted()).count()
        }
    }

    impl AutocompleteProvider for FakeProvider {
        fn trigger_characters(&self) -> Vec<char> {
            self.state.borrow().trigger_characters.clone()
        }

        fn get_suggestions(
            &self,
            lines: &[String],
            cursor_line: usize,
            cursor_col: usize,
            options: &SuggestionsOptions,
        ) -> Option<AutocompleteSuggestions> {
            let info = CallInfo {
                lines: lines.to_vec(),
                cursor_line,
                cursor_col,
                force: options.force,
            };
            let before = info.before();
            let result = {
                let state = self.state.borrow();
                (state.responder)(&info)
            };
            let mut state = self.state.borrow_mut();
            state.calls.push((before, options.force));
            state.signals.push(options.signal.clone());
            result
        }

        fn apply_completion(
            &self,
            lines: &[String],
            cursor_line: usize,
            cursor_col: usize,
            selected: &AutocompleteItem,
            prefix: &str,
        ) -> CompletionResult {
            // The applier never inspects the recorded call state, so a
            // plain borrow is safe (the default applier is a pure fn; the
            // delegated one captures only an `Rc` to the real provider).
            let state = self.state.borrow();
            (state.applier)(lines, cursor_line, cursor_col, selected, prefix)
        }
    }

    /// Temp directory guard (the TS tests' `mkdtempSync`/`t.after(rmSync)`).
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pi-tui-editor-ac-{tag}-{}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }

        fn path(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `/^\/cmd\s+(\S+)$/` — a slash command, whitespace, then a
    /// non-whitespace argument run to the end of the text. Returns the
    /// argument text (the TS `match[1]`).
    fn slash_arg_match<'a>(before: &'a str, command: &str) -> Option<&'a str> {
        let rest = before.strip_prefix(command)?;
        let arg = rest.trim_start();
        if arg.len() == rest.len() {
            return None; // `\s+` requires at least one whitespace
        }
        if arg.is_empty() || arg.chars().any(char::is_whitespace) {
            return None;
        }
        Some(arg)
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2096-2122
    // -------------------------------------------------------------------------

    #[test]
    fn undoes_autocomplete() {
        let mut editor = make_editor();

        let provider = FakeProvider::new(|info| {
            let before = info.before();
            if before == "di" {
                return Some(suggestions(vec![item("dist/", "dist/")], "di"));
            }
            None
        });

        editor.set_autocomplete_provider(provider);

        // Type "di"
        editor.handle_input("d");
        editor.handle_input("i");
        assert_eq!(editor.get_text(), "di");

        // Press Tab to trigger autocomplete
        editor.handle_input("\t");
        flush(&mut editor);
        assert_eq!(editor.get_text(), "dist/");
        assert!(!editor.is_showing_autocomplete());

        // Undo should restore to "di"
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "di");
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2130-2166
    // -------------------------------------------------------------------------

    #[test]
    fn triggers_and_debounces_symbol_completion_after_cjk_punctuation() {
        let mut befores: Vec<String> = vec!["查看，".to_string(), "\u{3000}".to_string()];
        befores.extend("，．：；！？（）［］｛｝“”‘’…—。、「」『』《》【】".chars().map(String::from));
        befores.extend(["(", "see (", "`", "["].map(String::from));

        for before in &befores {
            for trigger in ["@", "#", "$", "-"] {
                let mut editor = make_editor();
                let requests = Rc::new(RefCell::new(Vec::<String>::new()));
                let requests_clone = requests.clone();
                let provider = FakeProvider::with_triggers(
                    vec!['$', '-'],
                    move |info| {
                        requests_clone.borrow_mut().push(info.before());
                        None
                    },
                );
                editor.set_autocomplete_provider(provider);

                editor.set_text(before);
                editor.handle_input(trigger);

                // tick(19) — the 20ms debounce has not expired yet.
                editor.tick_autocomplete_debounce(19);
                flush(&mut editor);
                assert_eq!(*requests.borrow(), Vec::<String>::new(), "{before}{trigger}");

                // tick(1) — debounce expires, the request fires.
                editor.tick_autocomplete_debounce(1);
                flush(&mut editor);
                assert_eq!(*requests.borrow(), vec![format!("{before}{trigger}")]);

                // Typing re-arms a fresh 20ms debounce.
                editor.handle_input("r");
                editor.handle_input("e");
                editor.tick_autocomplete_debounce(19);
                flush(&mut editor);
                assert_eq!(requests.borrow().len(), 1, "{before}{trigger}");

                editor.tick_autocomplete_debounce(1);
                flush(&mut editor);
                assert_eq!(
                    *requests.borrow(),
                    vec![format!("{before}{trigger}"), format!("{before}{trigger}re")],
                    "{before}{trigger}"
                );
            }
        }
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2168-2205
    // -------------------------------------------------------------------------

    #[test]
    fn does_not_auto_trigger_after_cjk_letters_or_for_unprefixed_paths() {
        let texts = [
            "user@example.com",
            "张三@example.com",
            "查看@src",
            "あ@src",
            "カ@src",
            "한@src",
            "ㄅ@src",
            "𠮷@src",
            "か\u{3099}@src",
            "禰\u{e0100}@src",
            "々@src",
            "Ａ@src",
            "文档@备份",
            "prefix#123",
            "foo(@src",
            "问题#123",
            "查看，/path/",
            "查看，./文档/",
            "src/index.ts",
            "./文档/说明.md",
            "文档/说明.md",
            "查看src/index.ts",
        ];

        let mut editor = make_editor();
        let provider = FakeProvider::new(|_| None);
        editor.set_autocomplete_provider(provider.clone());

        for text in texts {
            editor.set_text("");
            for ch in text.chars() {
                editor.handle_input(&ch.to_string());
            }
            editor.tick_autocomplete_debounce(20);
            flush(&mut editor);
            assert_eq!(provider.call_count(), 0, "{text}");
        }
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2207-2227
    // -------------------------------------------------------------------------

    #[test]
    fn requests_path_completion_after_cjk_punctuation_only_on_tab() {
        let mut editor = make_editor();
        let provider = FakeProvider::new(|_| None);
        editor.set_autocomplete_provider(provider.clone());

        let text = "查看，/path/";
        for ch in text.chars() {
            editor.handle_input(&ch.to_string());
        }
        editor.tick_autocomplete_debounce(20);
        flush(&mut editor);
        assert_eq!(provider.calls(), Vec::<(String, bool)>::new());

        editor.handle_input("\t");
        flush(&mut editor);
        assert_eq!(provider.calls(), vec![(text.to_string(), true)]);
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2229-2260
    // -------------------------------------------------------------------------

    #[test]
    fn completes_chinese_path_prefixes_after_whitespace_or_cjk_punctuation_with_tab() {
        let base = TempDir::new("chinese-paths");
        std::fs::create_dir_all(base.0.join("文档")).unwrap();
        std::fs::write(base.0.join("文档").join("说明.md"), "text").unwrap();

        let mut editor = make_editor();
        editor.set_autocomplete_provider(Rc::new(CombinedAutocompleteProvider::new(
            Vec::new(),
            base.path(),
            None,
        )));

        for separator in [" ", "\t", "\u{3000}", "\u{a0}", "，", "。"] {
            editor.set_text(&format!("查看{separator}"));
            let before = editor.get_text();

            editor.handle_input("文");
            editor.handle_input("\t");
            flush(&mut editor);
            assert_eq!(editor.get_text(), format!("{before}文档/"), "{separator:?}");

            editor.handle_input("说");
            editor.handle_input("\t");
            flush(&mut editor);
            assert_eq!(editor.get_text(), format!("{before}文档/说明.md"), "{separator:?}");
            assert_eq!(editor.get_cursor(), (0, utf16_len(&editor.get_text())));
        }
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2262-2298
    // -------------------------------------------------------------------------

    #[test]
    fn ends_unquoted_trigger_and_debounce_contexts_at_whitespace_or_cjk_punctuation() {
        for separator in [" ", "\u{3000}", "，", "。"] {
            for trigger in ["@", "#", "$"] {
                let mut editor = make_editor();
                let requests = Rc::new(RefCell::new(Vec::<String>::new()));
                let prefix = format!("{trigger}src");
                let expected_prefix = prefix.clone();
                let items_prefix = prefix.clone();
                let requests_clone = requests.clone();
                let provider = FakeProvider::with_triggers(
                    vec!['$'],
                    move |info| {
                        let before = info.before();
                        requests_clone.borrow_mut().push(before.clone());
                        if before == expected_prefix {
                            Some(suggestions(
                                vec![item(&format!("{items_prefix}/"), "src/")],
                                &items_prefix,
                            ))
                        } else {
                            None
                        }
                    },
                );
                editor.set_autocomplete_provider(provider);

                editor.set_text(&format!("{trigger}sr"));
                editor.handle_input("c");
                editor.tick_autocomplete_debounce(20);
                flush(&mut editor);
                assert!(editor.is_showing_autocomplete(), "{separator:?} {trigger}");

                editor.handle_input(separator);
                flush(&mut editor);
                assert_eq!(
                    *requests.borrow(),
                    vec![prefix.clone(), format!("{prefix}{separator}")],
                    "{separator:?} {trigger}"
                );
                assert!(!editor.is_showing_autocomplete(), "{separator:?} {trigger}");

                editor.handle_input("文");
                editor.tick_autocomplete_debounce(20);
                flush(&mut editor);
                assert_eq!(
                    *requests.borrow(),
                    vec![prefix.clone(), format!("{prefix}{separator}")],
                    "{separator:?} {trigger}"
                );
            }
        }
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2300-2346
    // -------------------------------------------------------------------------

    #[test]
    fn re_triggers_cjk_path_completion_after_accepting_directories_and_deleting() {
        // The real provider supplies applyCompletion (quoting rules); the
        // scripted provider supplies getSuggestions.
        let real: Rc<CombinedAutocompleteProvider> =
            Rc::new(CombinedAutocompleteProvider::new(Vec::new(), ".", None));

        for directory in ["文档", "我的 文档", "资料，归档"] {
            let quoted = directory != "文档";
            let initial = if quoted {
                format!("@\"{}", slice_utf16(directory, 0, 2))
            } else {
                "@文".to_string()
            };
            let directory_value = if quoted {
                format!("@\"{directory}/\"")
            } else {
                format!("@{directory}/")
            };
            let file_value = if quoted {
                format!("@\"{directory}/说明.md\"")
            } else {
                format!("@{directory}/说明.md")
            };
            let file_prefix = if quoted {
                format!("@\"{directory}/说")
            } else {
                format!("@{directory}/说")
            };

            let mut editor = make_editor();
            let initial_capture = initial.clone();
            let file_prefix_capture = file_prefix.clone();
            let directory_value_capture = directory_value.clone();
            let file_value_capture = file_value.clone();
            let label = format!("{directory}/");
            let provider = FakeProvider::with_applier(
                move |info| {
                    let before = info.before();
                    let at = js_index_of(&before, "@", 0).unwrap_or(0);
                    let prefix = slice_utf16(&before, at, utf16_len(&before));
                    if prefix == initial_capture {
                        return Some(suggestions(
                            vec![item(&directory_value_capture, &label)],
                            &prefix,
                        ));
                    }
                    if prefix == file_prefix_capture {
                        return Some(suggestions(
                            vec![item(&file_value_capture, "说明.md")],
                            &prefix,
                        ));
                    }
                    None
                },
                {
                    let real = real.clone();
                    move |lines, cursor_line, cursor_col, selected, prefix| {
                        real.apply_completion(lines, cursor_line, cursor_col, selected, prefix)
                    }
                },
            );
            editor.set_autocomplete_provider(provider);

            editor.set_text(&format!("查看：{initial}"));
            editor.handle_input("\t");
            flush(&mut editor);
            assert_eq!(editor.get_text(), format!("查看：{directory_value}"), "{directory}");
            assert!(!editor.is_showing_autocomplete(), "{directory}");

            editor.handle_input("说");
            editor.tick_autocomplete_debounce(20);
            flush(&mut editor);
            assert!(editor.is_showing_autocomplete(), "{directory}");

            for deletion in ["\x7f", "\x1b[3~"] {
                editor.handle_input("错");
                editor.tick_autocomplete_debounce(20);
                flush(&mut editor);
                assert!(!editor.is_showing_autocomplete(), "{directory} {deletion:?}");

                if deletion == "\x1b[3~" {
                    editor.handle_input("\x1b[D");
                }
                editor.handle_input(deletion);
                editor.tick_autocomplete_debounce(20);
                flush(&mut editor);
                assert!(editor.is_showing_autocomplete(), "{directory} {deletion:?}");
            }

            editor.handle_input("\t");
            assert_eq!(editor.get_text(), format!("查看：{file_value} "), "{directory}");
            assert_eq!(editor.get_cursor(), (0, utf16_len(&editor.get_text())));
        }
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2348-2387
    // -------------------------------------------------------------------------

    #[test]
    fn auto_applies_single_force_file_suggestion_without_showing_menu() {
        let mut editor = make_editor();

        let provider = FakeProvider::new(|info| {
            if !info.force {
                return None;
            }
            let before = info.before();
            if before == "Work" {
                return Some(suggestions(vec![item("Workspace/", "Workspace/")], "Work"));
            }
            None
        });

        editor.set_autocomplete_provider(provider);

        // Type "Work"
        for ch in "Work".chars() {
            editor.handle_input(&ch.to_string());
        }
        assert_eq!(editor.get_text(), "Work");

        // Press Tab - should auto-apply without showing menu
        editor.handle_input("\t");
        flush(&mut editor);
        assert_eq!(editor.get_text(), "Workspace/");
        assert!(!editor.is_showing_autocomplete());

        // Undo should restore to "Work"
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "Work");
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2389-2431
    // -------------------------------------------------------------------------

    #[test]
    fn shows_menu_when_force_file_has_multiple_suggestions() {
        let mut editor = make_editor();

        let provider = FakeProvider::new(|info| {
            if !info.force {
                return None;
            }
            let before = info.before();
            if before == "src" {
                return Some(suggestions(
                    vec![item("src/", "src/"), item("src.txt", "src.txt")],
                    "src",
                ));
            }
            None
        });

        editor.set_autocomplete_provider(provider);

        // Type "src"
        for ch in "src".chars() {
            editor.handle_input(&ch.to_string());
        }
        assert_eq!(editor.get_text(), "src");

        // Press Tab - should show menu because there are multiple suggestions
        editor.handle_input("\t");
        flush(&mut editor);
        assert_eq!(editor.get_text(), "src");
        assert!(editor.is_showing_autocomplete());

        // Press Tab again to accept first suggestion
        editor.handle_input("\t");
        assert_eq!(editor.get_text(), "src/");
        assert!(!editor.is_showing_autocomplete());
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2433-2486
    // -------------------------------------------------------------------------

    #[test]
    fn keeps_suggestions_open_when_typing_in_force_mode() {
        let mut editor = make_editor();

        let all_files = ["readme.md", "package.json", "src/", "dist/"];

        let provider = FakeProvider::new(move |info| {
            let before = info.before();
            let should_match = info.force || before.contains('/') || before.starts_with('.');
            if !should_match {
                return None;
            }
            let filtered: Vec<AutocompleteItem> = all_files
                .iter()
                .filter(|f| f.to_lowercase().starts_with(&before.to_lowercase()))
                .map(|f| item(f, f))
                .collect();
            if !filtered.is_empty() {
                return Some(suggestions(filtered, &before));
            }
            None
        });

        editor.set_autocomplete_provider(provider);

        // Press Tab on empty prompt - should show all files (force mode)
        editor.handle_input("\t");
        flush(&mut editor);
        assert!(editor.is_showing_autocomplete());

        // Type "r" - should narrow to "readme.md" (force mode keeps
        // suggestions open)
        editor.handle_input("r");
        flush(&mut editor);
        assert_eq!(editor.get_text(), "r");
        assert!(editor.is_showing_autocomplete());

        // Type "e" - should still show "readme.md"
        editor.handle_input("e");
        flush(&mut editor);
        assert_eq!(editor.get_text(), "re");
        assert!(editor.is_showing_autocomplete());

        // Accept with Tab
        editor.handle_input("\t");
        assert_eq!(editor.get_text(), "readme.md");
        assert!(!editor.is_showing_autocomplete());
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2488-2517
    // -------------------------------------------------------------------------

    #[test]
    fn debounces_at_autocomplete_while_typing() {
        let mut editor = make_editor();

        let provider = FakeProvider::new(|info| {
            let before = info.before();
            Some(suggestions(vec![item("@main.ts", "main.ts")], &before))
        });
        editor.set_autocomplete_provider(provider.clone());

        editor.handle_input("@");
        editor.handle_input("m");
        editor.handle_input("a");
        editor.handle_input("i");

        assert_eq!(provider.call_count(), 0);
        assert!(!editor.is_showing_autocomplete());

        // `await new Promise(r => setTimeout(r, 50))` — the 20ms debounce
        // expires and the request starts; flushAutocomplete then drains it.
        editor.tick_autocomplete_debounce(50);
        flush(&mut editor);

        assert_eq!(provider.call_count(), 1);
        assert!(editor.is_showing_autocomplete());
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2519-2560
    // -------------------------------------------------------------------------

    #[test]
    fn re_queries_autocomplete_picker_when_cursor_moves_back_into_command_name() {
        let mut editor = make_editor();

        let provider = FakeProvider::new(|info| {
            let before = info.before();
            if !before.starts_with('/') {
                return None;
            }
            // Past the command name (a space before the cursor): offer
            // arguments.
            if before.contains(' ') {
                let prefix = before[before.find(' ').unwrap() + 1..].to_string();
                return Some(suggestions(
                    vec![item("repo", "repo"), item("message", "message"), item("help", "help")],
                    &prefix,
                ));
            }
            // Inside the command name: offer the command name only.
            Some(suggestions(vec![item("cmd", "cmd")], &before))
        });
        editor.set_autocomplete_provider(provider);

        // Type `/cmd ` so the picker ends up showing the argument list.
        for ch in "/cmd ".chars() {
            editor.handle_input(&ch.to_string());
            flush(&mut editor);
        }
        assert_eq!(editor.get_text(), "/cmd ");
        assert!(editor.is_showing_autocomplete());
        let at_arg = editor
            .render(80)
            .iter()
            .map(|l| strip_terminal_sequences(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(at_arg.contains("repo"), "argument menu should be visible at `/cmd `");

        // Arrow Left back into the command name (`/cmd`).
        editor.handle_input("\x1b[D");
        flush(&mut editor);

        // The picker must have re-queried: the stale argument items are
        // gone (replaced by the command-name suggestion, or closed).
        let after_move = editor
            .render(80)
            .iter()
            .map(|l| strip_terminal_sequences(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !after_move.contains("repo"),
            "stale argument menu must not survive the cursor move"
        );
        assert!(
            !after_move.contains("message"),
            "stale argument menu must not survive the cursor move"
        );
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2562-2582
    // -------------------------------------------------------------------------

    #[test]
    fn debounces_hash_autocomplete_while_typing() {
        let mut editor = make_editor();

        let provider = FakeProvider::new(|info| {
            let before = info.before();
            Some(suggestions(vec![item("#2983", "#2983")], &before))
        });
        editor.set_autocomplete_provider(provider.clone());

        editor.handle_input("#");
        editor.handle_input("2");
        editor.handle_input("9");
        editor.handle_input("8");

        assert_eq!(provider.call_count(), 0);
        assert!(!editor.is_showing_autocomplete());

        editor.tick_autocomplete_debounce(50);
        flush(&mut editor);

        assert_eq!(provider.call_count(), 1);
        assert!(editor.is_showing_autocomplete());
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2584-2587 (c)
    // -------------------------------------------------------------------------

    #[test]
    fn debounces_custom_trigger_characters_autocomplete_while_typing() {
        let mut editor = make_editor();

        let provider = FakeProvider::with_triggers(vec!['$'], |info| {
            let before = info.before();
            Some(suggestions(vec![item("$skill-name", "skill-name")], &before))
        });
        editor.set_autocomplete_provider(provider.clone());

        editor.handle_input("$");
        editor.handle_input("s");
        editor.handle_input("k");

        assert_eq!(provider.call_count(), 0);
        editor.tick_autocomplete_debounce(50);
        flush(&mut editor);

        assert_eq!(provider.call_count(), 1);
        assert!(editor.is_showing_autocomplete());
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2589-2612
    // -------------------------------------------------------------------------

    #[test]
    fn resets_custom_trigger_characters_when_provider_changes() {
        let mut editor = make_editor();

        let first = FakeProvider::with_triggers(
            vec!['$'],
            |_| Some(suggestions(vec![item("$skill-name", "skill-name")], "$")),
        );
        editor.set_autocomplete_provider(first);
        let second = FakeProvider::new(|_| {
            Some(suggestions(vec![item("$skill-name", "skill-name")], "$"))
        });
        editor.set_autocomplete_provider(second.clone());

        editor.handle_input("$");
        editor.handle_input("s");
        editor.tick_autocomplete_debounce(50);
        flush(&mut editor);

        assert_eq!(second.call_count(), 0);
        assert!(!editor.is_showing_autocomplete());
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2614-2645
    // -------------------------------------------------------------------------

    #[test]
    fn aborts_active_at_autocomplete_when_typing_continues() {
        let mut editor = make_editor();

        // The TS provider returns a promise that only resolves on abort;
        // the sync port keeps the request suspended (no flush) and
        // observes the abort through the captured AbortSignal.
        let provider = FakeProvider::new(|_| None);
        editor.set_autocomplete_provider(provider.clone());

        editor.handle_input("@");
        editor.handle_input("m");
        editor.handle_input("a");
        editor.handle_input("i");
        // sleep(250): the debounce fires, the request starts (the provider
        // captures signal 1), and its promise stays pending.
        editor.tick_autocomplete_debounce(250);
        editor.handle_input("n");
        // sleep(50): the re-armed debounce fires — starting the new request
        // aborts the previous in-flight controller (signal 1).
        editor.tick_autocomplete_debounce(50);

        assert_eq!(provider.aborted_signal_count(), 1);
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2647-2676
    // -------------------------------------------------------------------------

    #[test]
    fn hides_autocomplete_when_backspacing_slash_command_to_empty() {
        let mut editor = make_editor();

        let provider = FakeProvider::new(|info| {
            let before = info.before();
            // Only return slash command suggestions when line starts with /
            if let Some(query) = before.strip_prefix('/') {
                let commands = [
                    item("/model", "model"),
                    item("/help", "help"),
                ];
                let filtered: Vec<AutocompleteItem> = commands
                    .into_iter()
                    .filter(|c| c.value.starts_with(query))
                    .collect();
                if !filtered.is_empty() {
                    return Some(suggestions(filtered, &before));
                }
            }
            None
        });
        editor.set_autocomplete_provider(provider);

        // Type "/" - should show slash command suggestions
        editor.handle_input("/");
        flush(&mut editor);
        assert_eq!(editor.get_text(), "/");
        assert!(editor.is_showing_autocomplete());

        // Backspace to delete "/" - should hide autocomplete completely
        editor.handle_input("\x7f");
        flush(&mut editor);
        assert_eq!(editor.get_text(), "");
        assert!(!editor.is_showing_autocomplete());
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2678-2723
    // -------------------------------------------------------------------------

    #[test]
    fn applies_exact_typed_slash_argument_value_on_enter_even_when_first_item_is_highlighted() {
        let mut editor = make_editor();

        let all_arguments = [item("one", "one"), item("two", "two"), item("three", "three")];
        let provider = FakeProvider::new(move |info| {
            let before = info.before();
            if let Some(argument_text) = slash_arg_match(&before, "/argtest") {
                let filtered: Vec<AutocompleteItem> = all_arguments
                    .iter()
                    .filter(|arg| arg.value.starts_with(argument_text))
                    .cloned()
                    .collect();
                if !filtered.is_empty() {
                    return Some(suggestions(filtered, argument_text));
                }
            }
            None
        });
        editor.set_autocomplete_provider(provider);

        // Type "/argtest two"
        for ch in "/argtest two".chars() {
            editor.handle_input(&ch.to_string());
        }
        assert_eq!(editor.get_text(), "/argtest two");
        flush(&mut editor);
        assert!(editor.is_showing_autocomplete());

        // Press Enter - should apply the exact typed value "two", not the
        // first item (the exact match IS the selection, so applying is an
        // identity operation).
        editor.handle_input("\r");
        assert_eq!(editor.get_text(), "/argtest two");
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2725-2795
    // -------------------------------------------------------------------------

    #[test]
    fn selects_first_prefix_match_on_enter_when_typed_arg_is_not_exact_match() {
        let mut editor = make_editor();

        let all_arguments = [item("two", "two"), item("three", "three"), item("twelve", "twelve")];
        let provider = FakeProvider::new(move |info| {
            let before = info.before();
            if let Some(argument_text) = slash_arg_match(&before, "/argtest") {
                let filtered: Vec<AutocompleteItem> = all_arguments
                    .iter()
                    .filter(|arg| arg.value.starts_with(argument_text))
                    .cloned()
                    .collect();
                if !filtered.is_empty() {
                    return Some(suggestions(filtered, argument_text));
                }
            }
            None
        });
        editor.set_autocomplete_provider(provider);

        // Type "/argtest t" - filtered to [two, three, twelve], prefix "t"
        // matches "two" first.
        for ch in "/argtest t".chars() {
            editor.handle_input(&ch.to_string());
        }
        flush(&mut editor);
        assert!(editor.is_showing_autocomplete());

        // Press Enter - "t" prefix matches "two" (first in list), so "two"
        // is applied.
        editor.handle_input("\r");
        assert_eq!(editor.get_text(), "/argtest two");
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2797-2863
    // -------------------------------------------------------------------------

    #[test]
    fn highlights_unique_prefix_match_as_user_types_before_full_exact_match() {
        let mut editor = make_editor();

        let all_arguments = [item("one", "one"), item("two", "two"), item("three", "three")];
        let provider = FakeProvider::new(move |info| {
            let before = info.before();
            if let Some(argument_text) = slash_arg_match(&before, "/argtest") {
                // Return all items - provider does not filter.
                return Some(suggestions(all_arguments.to_vec(), argument_text));
            }
            None
        });
        editor.set_autocomplete_provider(provider);

        // Type "/argtest tw" - "tw" is a prefix of only "two".
        for ch in "/argtest tw".chars() {
            editor.handle_input(&ch.to_string());
        }
        assert_eq!(editor.get_text(), "/argtest tw");
        flush(&mut editor);
        assert!(editor.is_showing_autocomplete());

        // Press Enter - "tw" uniquely matches "two", so "two" is applied.
        editor.handle_input("\r");
        assert_eq!(editor.get_text(), "/argtest two");
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2865-2908
    // -------------------------------------------------------------------------

    #[test]
    fn selects_first_prefix_match_when_multiple_items_match() {
        let mut editor = make_editor();

        let all_arguments = [item("one", "one"), item("two", "two"), item("three", "three")];
        let provider = FakeProvider::new(move |info| {
            let before = info.before();
            if let Some(argument_text) = slash_arg_match(&before, "/argtest") {
                return Some(suggestions(all_arguments.to_vec(), argument_text));
            }
            None
        });
        editor.set_autocomplete_provider(provider);

        // Type "/argtest t" - "t" is a prefix of both "two" and "three".
        for ch in "/argtest t".chars() {
            editor.handle_input(&ch.to_string());
        }
        flush(&mut editor);
        assert!(editor.is_showing_autocomplete());

        // Press Enter - "t" matches "two" first, so "two" is selected.
        editor.handle_input("\r");
        assert_eq!(editor.get_text(), "/argtest two");
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2910-2964
    // -------------------------------------------------------------------------

    #[test]
    fn works_for_builtin_style_command_argument_completion_path_model_like() {
        let mut editor = make_editor();

        let all_models = [
            item("gpt-4o", "gpt-4o"),
            item("gpt-4o-mini", "gpt-4o-mini"),
            item("claude-sonnet", "claude-sonnet"),
        ];
        let provider = FakeProvider::new(move |info| {
            let before = info.before();
            if let Some(model_text) = slash_arg_match(&before, "/model") {
                let filtered: Vec<AutocompleteItem> = all_models
                    .iter()
                    .filter(|m| m.value.starts_with(model_text))
                    .cloned()
                    .collect();
                if !filtered.is_empty() {
                    return Some(suggestions(filtered, model_text));
                }
            }
            None
        });
        editor.set_autocomplete_provider(provider);

        // Type "/model gpt-4o-mini" - exact match for second item in list.
        for ch in "/model gpt-4o-mini".chars() {
            editor.handle_input(&ch.to_string());
        }
        assert_eq!(editor.get_text(), "/model gpt-4o-mini");
        flush(&mut editor);
        assert!(editor.is_showing_autocomplete());

        // Press Enter - the exact typed value is retained (identity apply).
        editor.handle_input("\r");
        assert_eq!(editor.get_text(), "/model gpt-4o-mini");
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:2966-2998
    // -------------------------------------------------------------------------

    #[test]
    fn awaits_async_slash_command_argument_completions() {
        let mut editor = make_editor();

        // The TS `getArgumentCompletions` is async; the Rust port resolves
        // it synchronously through the provider trait.
        let provider = Rc::new(CombinedAutocompleteProvider::new(
            vec![CommandEntry::Slash(SlashCommand {
                name: "load-skills".to_string(),
                description: Some("Load skills".to_string()),
                argument_hint: None,
                get_argument_completions: Some(Box::new(|prefix: &str| {
                    if prefix.starts_with('s') {
                        Some(vec![item("skill-a", "skill-a")])
                    } else {
                        None
                    }
                })),
            })],
            ".",
            None,
        ));
        editor.set_autocomplete_provider(provider);
        editor.set_text("/load-skills ");

        editor.handle_input("s");
        flush(&mut editor);
        assert!(editor.is_showing_autocomplete());

        editor.handle_input("\t");
        assert_eq!(editor.get_text(), "/load-skills skill-a");
        assert!(!editor.is_showing_autocomplete());
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:3000-3022
    // -------------------------------------------------------------------------

    #[test]
    fn ignores_invalid_slash_command_argument_completion_results() {
        let mut editor = make_editor();

        // TS passes a completer whose result is not an array ("not-an-
        // array"); the typed Rust port maps that runtime-invalid return to
        // `None` (no valid completions).
        let provider = Rc::new(CombinedAutocompleteProvider::new(
            vec![CommandEntry::Slash(SlashCommand {
                name: "load-skills".to_string(),
                description: Some("Load skills".to_string()),
                argument_hint: None,
                get_argument_completions: Some(Box::new(|_: &str| None)),
            })],
            ".",
            None,
        ));
        editor.set_autocomplete_provider(provider);
        editor.set_text("/load-skills ");

        editor.handle_input("s");
        flush(&mut editor);
        assert!(!editor.is_showing_autocomplete());
        assert_eq!(editor.get_text(), "/load-skills s");
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:3024-3078
    // -------------------------------------------------------------------------

    #[test]
    fn does_not_show_argument_completions_when_command_has_no_argument_completer() {
        let mut editor = make_editor();

        let provider = Rc::new(CombinedAutocompleteProvider::new(
            vec![
                CommandEntry::Slash(SlashCommand {
                    name: "help".to_string(),
                    description: Some("Show help".to_string()),
                    argument_hint: None,
                    get_argument_completions: None,
                }),
                CommandEntry::Slash(SlashCommand {
                    name: "model".to_string(),
                    description: Some("Switch model".to_string()),
                    argument_hint: None,
                    get_argument_completions: Some(Box::new(|_: &str| {
                        Some(vec![item("claude-opus", "claude-opus")])
                    })),
                }),
            ],
            ".",
            None,
        ));
        editor.set_autocomplete_provider(provider);

        editor.handle_input("/");
        editor.handle_input("h");
        editor.handle_input("e");
        flush(&mut editor);
        assert!(editor.is_showing_autocomplete());

        editor.handle_input("\t");
        assert_eq!(editor.get_text(), "/help ");
        assert!(!editor.is_showing_autocomplete());
    }

    // -------------------------------------------------------------------------
    // editor.test.ts:1820-1834
    // -------------------------------------------------------------------------

    #[test]
    fn does_not_trigger_autocomplete_during_single_line_paste() {
        let mut editor = make_editor();

        let provider = FakeProvider::new(|_| None);
        editor.set_autocomplete_provider(provider.clone());

        editor.handle_input("\x1b[200~look at @node_modules/react/index.js please\x1b[201~");

        assert_eq!(editor.get_text(), "look at @node_modules/react/index.js please");
        assert_eq!(provider.call_count(), 0);
        assert!(!editor.is_showing_autocomplete());
    }
}

/// Test-only access to the thread-local pipeline state (see
/// [`make_editor`](super::tests)). Kept in a dedicated (test-gated) module
/// so the reset helper is reachable from the `tests` module without
/// polluting the module surface.
#[cfg(test)]
pub(crate) mod editor_autocomplete_tests_support {
    /// Clear the suspended continuation and the debounce-timer
    /// accumulation. Called before every editor construction in this
    /// module's tests (cargo test reuses threads across test fns, and the
    /// editor identity is an allocation address that can repeat).
    pub(crate) fn reset_pipeline_state() {
        super::IN_FLIGHT.with(|slot| *slot.borrow_mut() = None);
        super::DEBOUNCE_ELAPSED.with(|state| *state.borrow_mut() = (0, 0, 0));
    }
}
