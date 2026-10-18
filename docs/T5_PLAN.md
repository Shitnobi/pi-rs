# T5 Plan — Renderer + screens (`tui.ts`, `tui-main-screen.ts`, `tui-alt-screen.ts`, `alt-screen-search.ts`, `wheel-scroll.ts`)

~4,368 LOC TS + the T5 slice of `terminal-image.ts` (~420 LOC) + `components/alt-screen-flash.ts` (51 LOC).
Upstream tests: tui-render.test.ts (1024), tui-alt-screen.test.ts (2100), wheel-scroll (47),
tui-overlay-style-leak (81), tui-shrink (45), tui-cell-size-input (82).

## Ground rules (inherited)

- **Pure std** — no regex crate (explicit matchers), no threads/timers, no async runtime, no serde.
  All TS timers/`process.nextTick`/promises become **thread-local deadline/continuation state with
  explicit pump methods** (same model as editor D1b: firing STARTS work, flush completes it).
- **Virtual clock** (`src/clock.rs`, coordinator-owned): thread-local `now_ms()` / `advance_ms(n)` /
  `reset_for_test()`. Every `performance.now()` / `Date.now()` in the T5 modules reads it. The parity
  harness drives both sides via script op `tickMs`.
- **Ground truth is TS under Node.** Every numeric type that TS treats as `number` stays `i64`/`f64`
  with JS semantics (NaN, Infinity, truncation) where the original relies on them.
- UTF-16 semantics: mouse/terminal coords are cell indices (plain integers); string indexing follows
  the crate's established utf16 helpers where TS slices strings.

## Module split

| Rust file | TS source | Owner |
|---|---|---|
| `src/clock.rs` | (new, no TS counterpart) | coordinator |
| `src/wheel_scroll.rs` | wheel-scroll.ts (82) | coordinator |
| `src/components/alt_screen_flash.rs` | components/alt-screen-flash.ts (51) | coordinator |
| `src/terminal_image.rs` | terminal-image.ts T5 slice | D-A4 |
| `src/tui.rs` | tui.ts core (types, TuiBase focus/input/scheduling/color-queries/markers) | coordinator skeleton + D-A1 |
| `src/tui_overlay.rs` | tui.ts overlay half (214-356, 692-915, 1215-1462) | D-A1 |
| `src/tui_main_screen.rs` | tui-main-screen.ts (656) | D-A2 |
| `src/alt_screen_search.rs` | alt-screen-search.ts (327) | D-A3 |
| `src/tui_alt_screen.rs` | tui-alt-screen.ts core (struct/lifecycle/input/mouse/scrollbar/search-routing) | coordinator skeleton + D-B1 |
| `src/alt_screen_selection.rs` | tui-alt-screen.ts selection half (~1100-1462) | D-B2 |
| `src/alt_screen_render.rs` | tui-alt-screen.ts doRender half (~1440-1786) | D-B3 |

`terminal_image.rs` T5 slice: `TerminalCapabilities`/`ImageProtocol`, env detection (injectable env
reader), get/set/reset capabilities + overrides (thread-local cache), `is_image_line` (promote from
component.rs), `allocate_image_id`, kitty delete sequences, kitty metadata/placement registry
(promote the private layout.rs helpers), `crop_kitty_image_line`, cell dimensions get/set.
**T6-later:** `encodeKitty`, `encodeITerm2`, image dimension parsers, `calculateImage*`, `renderImage`,
`hyperlink`, `imageFallback`.

## Structural design (the load-bearing decisions)

1. **TuiBase is composition, not inheritance.** `pub struct TuiBase { ...common fields...,
   container: Container }`. Each screen owns `base: TuiBase` plus its own fields.
   Hook methods (`doRender`, `resetRenderState`, `before/afterTerminalStart/Stop`, `getMountedRoots`)
   become the `TuiHooks`-shaped inherent methods on each screen; shared pipeline code lives in
   `tui.rs` as `TuiBase` methods that return pump instructions instead of calling hooks directly.
2. **Screens are shared as `Rc<RefCell<TuiMainScreen>>` / `Rc<RefCell<TuiAltScreen>>`.**
   - `impl Component for TuiMainScreen/TuiAltScreen` (children via `base.container`, alt overrides
     `render` for `layoutRoot`).
   - `TuiSurface` trait (object-safe, the TS `TUI` interface) implemented for both screens **via a
     declarative macro** that forwards to `self.base` + screen pump: `set_focus`, `show_overlay`,
     `hide_overlay`, `has_overlay`, `get/set_show_hardware_cursor`, `get/set_clear_on_shrink`,
     `request_render`, `render_now`, `add/remove_input_listener`, color-scheme APIs, `query_terminal_colors`.
   - `OverlayHandle` = `{ screen: Weak<RefCell<dyn TuiSurface>>, entry_id: u64 }` with inherent
     methods that upgrade + dispatch (`hide`, `set_hidden`, `is_hidden`, `focus`, `unfocus`,
     `is_focused`, `get_bounds`). Entry identity by `entry_id` (not pointer) since entries hold
     `ComponentRef`s and the handle must survive `Rc` clones.
   - `impl TuiHost for TuiMainScreen/TuiAltScreen` (editor surface: `request_render`,
     `terminal_rows`) so screens coerce to the existing `Rc<RefCell<dyn TuiHost>>`.
3. **Focusable bridge** (replaces the planned `Any` downcast): `Component::as_focusable(&mut self)
   -> Option<&mut dyn Focusable>` (default `None`). TS `"focused" in component` ⇔
   `as_focusable().is_some()`. Every Focusable component overrides it. (Editor, Input, ScrollView,
   AltScreenSearchComponent, …)
4. **Render scheduling state machine** (tui.ts:1012-1053):
   - `render_requested: bool`, `next_tick_queued: bool` (process.nextTick → scheduleRender),
     `immediate_scheduled: bool` (process.nextTick → immediate render),
     `render_timer: Option<u64>` (setTimeout deadline, virtual-clock ms), `last_render_at: f64`.
   - Pumps (per screen, macro-generated): `run_pending_ticks()` drains the nextTick queue
     (= TS microtask drain; a no-op after the first harness op boundary), `advance_clock(ms)`
     advances the global clock and fires due timers — each fired render timer executes
     `doRender` through the screen hook. `request_render(force)` stays synchronous-flag-setting,
     exactly like TS.
   - Parity contract: harness op `tickMs: n` = `advance_clock(n)` on Rust; patched
     setTimeout/nextTick drain on TS.
5. **Terminal color query state machine** (tui.ts:1470-1515): `queryTerminalColors` promise →
   thread-local `COLOR_QUERY_CONTINUATIONS: RefCell<HashMap<usize /*tui_id*/, Vec<PendingQuery>>>`
   with `deliver: Option<Box<dyn FnMut(TerminalColors)>>`, `timer` deadline. DA1 reply / full palette
   completes + delivers; timeout (fired by clock pump) delivers partial and re-targets to
   `on_late_reply`. Same observability rules as D1b: the query ESC write happens at call time.
6. **Alt-screen timers** similarly: selection auto-scroll `setInterval(50ms)` →
   `selection_auto_scroll_deadline: Option<u64>` + `tick_auto_scroll()` fired from the clock pump;
   flash timers → per-entry deadlines + `tick_flashes()` on the flash container.

## Parity layer 3 — write-stream differential (Wave C)

- TS harness tool `tui-main` + `tui-alt`: construct the screen with a `RecordingTerminal`
  (implements the TS `Terminal` interface; records `write()` strings, `columns`, `rows`,
  `hideCursor`/`showCursor` counts), scripted ops, canonical JSON out.
- Rust harness `harness tui-main` / `harness tui-alt` mirroring 1:1.
- Script ops: `{type:"addChild", component}` (component described by kind+props: text/truncatedText/
  spacer/input/box/stack/scrollView…), `{type:"input", data}`, `{type:"setFocus", path}`,
  `{type:"showOverlay", component, options}`, `{type:"hideOverlay"}`, `{type:"requestRender", force?}`,
  `{type:"renderNow", force?}`, `{type:"resize", cols, rows}`, `{type:"tickMs", n}`,
  `{type:"start"}`, `{type:"stop", options?}`, `{type:"terminalReply", data}` (feed DA1/OSC replies),
  alt-only: `{type:"setLayoutRoot"}`, `{type:"scrollBy"}`, `{type:"flash"}`.
- Output: `{frames: [base64(write stream per render)], fullRedraws, writes: [ … ], focused: path,
  screenLines?}`. Write stream comparison catches damage tracking, cursor moves, SGR, sync-output.
- TS side clock patching: `performance.now`, `Date.now`, `setTimeout/clearTimeout/setInterval`
  (virtual scheduler fired by `tickMs`), `process.nextTick` (queue drained after each op) — same
  pattern as the editor harness `tickMs`.
- Reuse T4's group naming: `tui-main`, `tui-alt`.

## Fleet

- **Foundation (coordinator)**: clock.rs, wheel_scroll.rs, alt_screen_flash.rs, tui.rs skeleton
  (types + TuiBase struct + TuiSurface macro + pump plumbing + focus/input/color-query methods +
  cursor-marker helpers), tui_alt_screen.rs skeleton (struct + fields + binding-signature stubs),
  component.rs `as_focusable` bridge, mod.rs wiring.
- **Wave A** (parallel, one file per delegate): D-A1 tui_overlay.rs, D-A2 tui_main_screen.rs,
  D-A3 alt_screen_search.rs, D-A4 terminal_image.rs.
- **Wave B** (parallel): D-B1 tui_alt_screen.rs core fill, D-B2 alt_screen_selection.rs,
  D-B3 alt_screen_render.rs.
- **Wave C**: parity harness (coordinator) + case authoring (delegate) — tui-render/tui-alt-screen
  upstream test expectations become parity cases where they encode real behavior.
- **Redteams**: R1 (overlay/focus state machine), R2 (main-screen diff algorithm vs write stream),
  R3 (alt-screen selection + search), after waves A/B. Report-first to /tmp/report-t5-R*.md.

## Divergences (documented as found)

- `allocateImageId` uses `Math.random` in TS; Rust uses an injectable random supplier with a
  deterministic default (clock-seeded xorshift). Parity cases avoid asserting on it.
- FS debug logs (`PI_TUI_DEBUG`, `PI_TUI_DEBUG_REDRAW`, crash log on over-wide lines) are env-gated
  in TS; Rust ports the env-gated behavior with std::fs but the parity harness never enables them.
  The over-wide-line crash: TS throws after writing a crash log; Rust panics (same message) — parity
  cases that trigger it are out of contract.
- `isTermuxSession`, mouse-mode selection (TMUX/ZELLIJ/STY/TERM prefix), WezTerm detection, and
  right-click paste platform gate read `process.env`/`process.platform`; Rust reads the same env
  via an injectable env reader (tests inject; harness injects a fixed empty env on both sides).
