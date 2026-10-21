# pi-rs Port Tracking

# pi-rs Port Tracking

## ⚠️ Current checkpoint (2026-10-20) — resume here

**State: facade wave COMPLETE and parity-green (1760/1760).**
Landed on main through 0f55321:

- `pi_ai/{models_error,model_operations,transcript,lazy,models_impl}.rs`
  (commits 07c008d, cad9e79): pi-ai prerequisites — ModelsError,
  assert/image/classifier error results, normalizeContext, lazyStream
  semantics, and ModelsImpl/createModels implementing MutableModels
  (refresh generations/supersede, publication chains, getAuth precedence,
  login/logout, refresh with publication.update invocation). merge_headers
  pub for reuse.
- `model_runtime.rs` (~2800 LOC) + `model_registry.rs` (commits c5e979f):
  full ModelRuntime/ModelRegistry ports — create() with injected
  env/offline/catalog hooks, composition layer, snapshot machine with
  availability seqs, credential op queue + CredentialSynchronizationError,
  getProviderAuthStatus precedence, prepareRequest merging, virtual-model
  registration/routing, register/unregister (TS void-refresh deliberately
  NOT run inline — synchronous-facade observable contract; harness settle
  step emulates it), getError ordering.
- Parity harness: `runtime_harness.rs` + ts-harness entries + 85 cases
  (54 mr-*, 31 reg-*) — commit 0f55321. builtin-catalog.json refreshed
  from pi-ai dist data (42 providers, 1731 entries).
- **Full parity: 1760/1760.** cargo test --workspace green.
  Clippy at baseline (~349 warnings).

**Next steps:** remaining core modules per roadmap (agent-session,
session-manager, extensions, tools/*), each wave parity-green
via new harness entries.

**Checkpoint update (2026-10-20, core-modules waves):**
- `pi-core` module wave (9aed24d): event_bus, timings, slash_commands,
  settings_defaults. `settings_schema.rs` in pi-providers (adecf45),
  `prompt_templates.rs` (4bf7b40), `skills.rs` (c11d060).
- **settings-manager.ts ported** (e8724c9): `pi-core/src/settings_manager.rs`
  (~3500 LOC, 69 tests). Settings = raw serde_json Values; StorageStorage
  trait + File/InMemory backends (proper-lockfile semantics); async TS
  writeQueue → sync VecDeque drained by flush()/reload(); env/platform/
  uuid injected via SettingsManagerOptions; create() requires agent_dir.
- **Core-module parity wave** (1bc7b9d): `core_harness.rs` + 155 cases
  (event-bus 34, timings 21, slash-commands 18, settings-defaults 5,
  settings-schema 77). Full parity 1915/1915 at that commit.
- **settings-manager parity wave** (37a06cd): 121 sm-* cases; differential
  found + fixed 4 genuine divergences (raw-null persistence in
  setShellPath/setShellCommandPrefix/setEnabledModels, V8 snippet
  raw-embed + >20-unit truncation in parse_json_like, JS object-spread
  semantics in set/getWarnings, raw-f64 outputPad). ts-harness installs a
  startup pass-through dispatcher on require('crypto').randomUUID — the
  crypto ESM namespace snapshots named exports, per-case patches are
  invisible (probed; see top of ts-harness.js).
- Parity harness still pending for: prompt-templates, skills (delegate
  notes: per-case temp dirs, don't assert multi-template directory order).
- Remaining queue: agent-session (tokio per G8), session-manager,
  extensions, tools/*, diagnostics; then the shared typebox-walker refactor
  (model_config + settings_schema).

**Resume note (2026-10-19 23:59 — partial provider_composer work in tree):**
`crates/pi-providers/src/provider_composer.rs` + `pi_ai/compat.rs` (new) +
`pi_ai/{credential,mod,provider}.rs` edits are UNCOMMITTED and DO NOT BUILD
(~26 errors: private ModelType import in pi_ai::models, fn-arg mismatches
~line 655+). Finish the port vs
/tmp/pi-repo/packages/coding-agent/src/core/provider-composer.ts, then
composer_harness.rs, TS harness entries + ~120 parity cases for
auth-storage/remote-catalog/provider-composer, then the facade wave
(model-runtime, model-registry + harness + ~80 cases), full parity,
workspace tests, clippy, checkpoint, push. Trap: serde_json preserve_order
Map::remove is swap_remove — use shift_remove for JS-delete semantics.

**Resume note (2026-10-19 23:24, wound down):** survey of the auth/composer
wave just started, nothing written yet. Correct file names/sizes:
`core/auth-storage.ts` (507 LOC), `core/provider-composer.ts` (732 LOC),
`core/remote-catalog-provider.ts` (NOT remote-catalog.ts). Useful scratch
assets on this machine (outside the repo): `/home/shinobi/wave2-spec.md`
(wire-protocol template), `/home/shinobi/compat-probe.json` (probed
typebox schema), `/home/shinobi/mr-diff.mjs` (per-tool differential runner;
usage: `node /home/shinobi/mr-diff.mjs <tool>` — sends bare args JSON to
the Rust side, wrapped {tool,args} to the TS side; RUST_BIN env overrides
the binary). Workflow that worked for this wave: probe TS ground truth
first → write spec → parallel delegates on git worktrees with pre-wired
main.rs dispatch → per-tool differential vs ts-harness → merge → full
parity → workspace tests + clippy → TRACKING checkpoint → push.

## Previous checkpoint (2026-10-19, later) — superseded

**State: providers foundation wave COMPLETE and parity-green.** The 3 E0308s
in `models_store.rs` are fixed (the `with_lock` closures now pass `&mut` to
match the `&mut dyn FnMut` trait parameter; test helper return type corrected
to `FileModelsStore<FakeBackendHandle>`; one `Arc` clone fix). The wave's
parity harness entries are landed:

- New module-level harness tools (TS side `parity/ts-harness.js`, Rust side
  `crates/pi-cli/src/providers_harness.rs`, wired via `run_tool_input`):
  `config-value` (9 fns, real `/bin/sh` command execution mirroring TS
  `execSync`), `http-dispatcher` (parse/format), `auth-guidance` (docs-path
  normalized to `<docs>` on both sides), `models-store` (inMemory + file
  variants over per-case temp dirs), `runtime-credentials` (overlay over an
  insertion-ordered fake mirroring pi-ai `InMemoryCredentialStore` Map
  semantics), `virtual-models` (5 fns).
- 102 new parity cases (`parity/cases.mjs`): config-value 34,
  http-dispatcher 18, auth-guidance 5, models-store 13, runtime-credentials
  13, virtual-models 19. **Full parity: 1390/1390.**
- `cargo test --workspace`: 1517 passed / 0 failed. Clippy: 0 errors.

Documented divergences (see comments in cases.mjs / harness):
- `models_store::parse_stored` rejects non-object JSON roots; the TS parse is
  an unchecked cast (null root throws "Cannot read properties of null",
  array roots read string keys as undefined). Not parity-tested.
- `FileAuthStorageBackend` creates the file lazily; TS pre-creates `"{}"` at
  construction (mode 0o600). The harness mirrors the TS side effect so the
  final-content comparison matches.
- `pi_ai::InMemoryCredentialStore` uses BTreeMap (sorted); the harness fake
  is insertion-ordered to match the TS Map. (If that store becomes
  parity-visible later, switch it to insertion order per plan trap #20.)

**Next steps (in order):**
1. Config/resolution wave: `model_config.rs`, `model_resolver.rs`,
   `provider_attribution.rs` (currently 1-line stubs) — port per
   `docs/plans/phase3-providers.md` steps 4/6/9, parity-green via new
   module-level harness entries.
2. Auth/composer wave: full `auth_storage` (AuthStorage/ReadOnly/
   InMemoryAuthStorageBackend — only the backend trait is ported),
   `provider_composer`, `remote_catalog`.
3. Facade wave: `model_runtime`, `model_registry`.
Each wave parity-green via new harness entries, then batter targets +
regressions.

## Previous checkpoint (2026-10-19, earlier) — superseded

**State: mid-tranche, code does NOT compile.** The providers foundation wave
(commit `8f8b0e4`, uncommitted agent work batch-committed as WIP) is partially
landed in `crates/pi-providers/`: `auth_guidance.rs`, `auth_storage.rs`,
`config_value.rs`, `http_config.rs`, `models_store.rs`,
`runtime_credentials.rs`, `virtual_models.rs`, and the `pi_ai/` modules
(`js`, `credential`, `macros`, `model`, `models`, `models_store`,
`provider`), plus `error.rs` and `crates/pi-tools/src/node_fs.rs` changes.

Note: `pi-tui` has ~58 pre-existing warnings (known, see T5 notes); only
`pi-providers` errors block the build.

Progress tracker for the Rust port of
[pi's coding-agent package](https://github.com/earendil-works/pi/tree/main/packages/coding-agent).
See [PARITY.md](PARITY.md) for the parity-test methodology.

## Phase 1 — Built-in tools ✅ (complete)

All seven built-in tools from `src/core/tools/` are ported and pass the
differential parity suite (result payload + filesystem effects):

| Tool     | Rust module             | Notes |
|----------|-------------------------|-------|
| `read`   | `pi-tools/src/read.rs`  | text path; image path completed in Phase 2 |
| `write`  | `pi-tools/src/write.rs` | |
| `edit`   | `pi-tools/src/edit.rs`  | uses `pi-core` edit-diff/jsdiff |
| `ls`     | `pi-tools/src/ls.rs`    | |
| `grep`   | `pi-tools/src/grep.rs`  | shells out to `rg` |
| `find`   | `pi-tools/src/find.rs`  | shells out to `fd` |
| `bash`   | `pi-tools/src/bash.rs`  | process-group kill, spill files, full-output reader |

Supporting ports in `pi-core` (verified by 46 unit tests, several of them
ground-truthed against the real JS libraries):

- `jsdiff.rs` — exact port of `jsdiff@8.0.4` structured diff +
  `createTwoFilesPatch` unified-patch output (verified by executing the real
  package under Node).
- `edit_diff.rs` — the edit tool's diff/patch rendering, including
  fuzzy-match normalization (smart quotes, trailing whitespace).
- `truncate.rs` — `truncateTail` / `truncateLines` with byte+line caps.
- `normalize.rs` — NFKC/NFD Unicode normalization used by edit fuzzy match.
- `js_string.rs` — JS `String.length` (UTF-16 code units), `trimEnd`,
  `toFixed(1)`.
- `paths.rs` — macOS screenshot filename handling (narrow no-break space,
  AM/PM regex), workspace path resolution.
- `text.rs`, `pi-tools/node_fs.rs` — Node.js `fs` error messages, access
  checks, stat semantics.

Infra: `pi-cli harness tool <name>` (JSON stdin → stdout) drives the Rust
side of the parity harness.

## Phase 2 — Image processing ✅ (complete)

`read` on image files now runs the exact same pixel pipeline as the
original, byte-identically:

- New crate `crates/pi-photon`: executes the **exact wasm binary** shipped
  with `@silvia-odwyer/photon-node@0.3.4` (the module the original loads
  under Node/V8) under **wasmtime**, with the wasm-bindgen ABI
  (`__wbindgen_malloc/free`, `__wbg_photonimage_free`, multivalue returns)
  bound by hand. A native recompile of photon-rs was tried first and
  **rejected**: LLVM vectorizes the resize float accumulation differently
  on x86-64 than on wasm32, so outputs diverge in the last bytes; running
  the identical wasm module makes float behavior deterministic and
  byte-identical by construction. Byte parity against ground truth
  generated by the real photon-node under Node is covered by
  `pi-photon/tests/parity.rs` (decode → PNG roundtrip, resize, JPEG encode
  at q80/85/70/55/40 across PNG/JPEG/GIF/WebP/BMP inputs).
- `pi-tools/src/image.rs`: full port of `image-process.ts`,
  `image-resize-core.ts`, `image-convert.ts`, and `exif-orientation.ts` —
  mime whitelist + PNG conversion, default resize options
  (2000×2000, 4.5 MB base64 cap, quality steps 80/85/70/55/40 with PNG+JPEG
  candidate encodings and ×0.75 progressive shrinking), EXIF orientation
  (JPEG APP1 / WebP chunk → TIFF IFD tag 0x0112, orientations 1–8), and
  the `toFixed(2)` dimension note.
- 9 new parity cases (`read/image-*`) with binary fixtures
  (`parity/fixtures/`): PNG passthrough/resize, JPEG resize, BMP→PNG
  conversion, GIF/WebP passthrough, corrupt PNG, EXIF orientation 6, and
  an animated PNG that falls through to the text path.

## Phase 3 — Core runtime (in progress)

First tranche complete: the process-runtime layer, streaming bash execution,
and the custom-message model.

- [x] `shell.rs` (pi-tools) — port of `utils/shell.ts` + `utils/child-process.ts`:
  shell config resolution (custom path → `/bin/bash` → `which bash` → `sh -c`
  fallback), `getShellEnv()` (`~/.pi/agent/bin` PATH prepend, `PI_CODING_AGENT_DIR`
  override), process-group spawn + `killProcessTree`, and `waitForChildProcess`
  with the 100 ms exit-grace re-arm semantics (a descendant still writing to the
  pipes keeps us reading). `BashOperations` trait + `createLocalBashOperations`
  (resolveTimeoutMs validation, cwd access check, `timeout:<raw>` /
  `aborted` error strings, 128+signal exit-code fallback), plus `execCommand`
  (`exec.ts`) with per-chunk Buffer.toString decoding (BOM preserved).
- [x] `bash_executor.rs` — port of `bash-executor.ts` `executeBashWithOperations`:
  rolling output window in UTF-16 code units (`js_length`), spill to a
  `pi-bash-*.log` temp file past 50 KB, `splitIncompleteAnsiSuffix` pending-ANSI
  carry across chunks, `sanitizeBinaryOutput(stripAnsi(...)).replace(\r, "")`,
  partial output on abort, `truncateTail` on the full output.
- [x] `output_accumulator.rs` / `output_files.rs` (registered + parity-tested).
- [x] `bash.rs` rebuilt on the new layers (replacing the Phase-1 monolith):
  same observable behavior, parity suite stayed green through the refactor.
- [x] `messages.rs` (pi-core) — port of `messages.ts`: `BashExecutionMessage`,
  `CustomMessage`, `BranchSummaryMessage`, `CompactionSummaryMessage`,
  `AgentMessage` tagged serde roundtrip, `bashExecutionToText`, `convertToLlm`
  (custom roles → user messages, `excludeFromContext` filtering),
  `create*Message` factories with `new Date(ts).getTime()` semantics
  (ISO-8601 parse with fraction truncation to ms, negative pre-epoch values,
  invalid dates → `null` like `JSON.stringify(NaN)`).
- [x] Harness + parity: new module-level harness entries (`bash-executor`,
  `output-accumulator`, `messages`) on both sides; the TS harness now drives
  the original dist per-module files directly. 30 new parity cases.

Second tranche complete: session persistence, compaction, and nested-call
recording.

- [x] `session_manager.rs` (new `pi-session` crate) — full port of
  `session-manager.ts`: append-only JSONL session trees (`CURRENT_SESSION_VERSION=3`),
  `LeafSpec` tri-state leaf selection, entry projection
  (`sessionEntryToContextMessages`, `buildContextEntries`,
  `buildSessionProjection` with context-edit replay and compaction
  boundaries), v1→v2/v2→v3 migrations, 4 KB header scan with 1 MB limit,
  `forkFrom` `wx` semantics, `createBranchedSession` label re-chaining,
  `_persist` deferral until `hasConversation()`, and
  `getDefaultSessionDir`/`loadEntriesFromFile`/`findMostRecentSession`.
  Supporting pi-core additions: `ai.rs` (`uuidv7`, `js_number`, `Usage`,
  `contentText`, `getCurrentSystemMessage` with TS `??=` timestamp
  semantics) and `usage_totals.rs` (`combineUsage`).
- [x] `compaction_utils.rs` (pi-session) — port of `compaction/utils.ts`:
  `FileOperations` (insertion-ordered sets), `extractFileOpsFromMessage`
  (assistant tool calls + tool-result `nestedCalls`), `computeFileLists`,
  `formatFileOperations`, `serializeConversation` (tool results truncated at
  `TOOL_RESULT_MAX_CHARS=2000`, thinking omitted),
  `SUMMARIZATION_SYSTEM_PROMPT`. JS `.length` semantics are UTF-16 code
  units (`js_str_len`), and `Object.entries` argument formatting matches
  `JSON.stringify` output.
- [x] `compaction.rs` (pi-session) — port of the pure logic in
  `compaction.ts`: `DEFAULT_COMPACTION_SETTINGS`,
  `calculateContextTokens` (JS `||` + `undefined + number → NaN`
  semantics: a usage missing components is invalid, exactly like TS),
  `getLastAssistantUsage` (skips aborted/error/zero),
  `estimateTokens` (chars/4 per role, `ESTIMATED_IMAGE_CHARS=4800`;
  assistant string content estimates to 0 because TS `for..of` iterates
  chars), `estimateContextTokens`/`estimateProjectedContextTokens`
  (usage invalidation by later context edits/compaction), `shouldCompact`,
  `findCutPoint`/`findTurnStartIndex` (backwards budget walk, cut-point
  selection, metadata backtracking, split-turn detection), the projected
  variants with the recovery-omission-suffix rule, `getSummarizationFailure`,
  and `prepareCompaction` (full `CompactionPreparation` JSON form). The
  LLM-dependent tail (`compact`, `generateSummary*`, `completeSummarization`,
  `generateTurnPrefixSummary`) is deferred until the provider layer exists
  (G7).
- [x] `nested_tool_calls.rs` (pi-session) — port of `NestedCallRecorder`
  from `nested-tool-calls.ts`: `NESTED_CALL_LIMITS`
  (256 calls / 8 KB per-call / 32 KB total / 500 error chars), record key
  growth matching TS object-literal order, byte-budgeted argument retention,
  `combineUsage` usage sums, and `snapshot` completion semantics.
  `NestedToolCallRunner` is deferred until the agent tool pipeline exists
  (G8).
- [x] Harness + parity: new module-level harness entries (`session-manager`,
  `compaction-utils`, `compaction`, `nested-calls`) on both sides; the
  nested-calls entry drives an op script (start/finish/addUsage/totalUsage/
  snapshot) against a fresh recorder, with `durationMs` normalized to 0 on
  both sides (wall clock). New `normalizeSessionIds` case option rewrites
  randomly generated 8-hex-char linkage ids (`id`/`parentId`/
  `firstKeptEntryId`) to `<ID0>`, `<ID1>`, … in first-appearance order, so
  the v1→v2 migration (which generates random ids) can be compared
  graph-isomorphically. 136 new parity cases (62 session-manager,
  18 compaction-utils, 44 compaction, 12 nested-calls).

Remaining, roughly in dependency order:

- [ ] `file-mutation-queue.ts` (deferred with the async runtime decision — G8)
- [ ] `mcp-servers.ts`, `extensions/` (extension + MCP loading)
- [ ] `agent-session*.ts` (agent loop, tool-call orchestration)
- [ ] `provider-*`, `model-*`, `http-dispatcher.ts` (LLM providers)
- [ ] CLI/TUI layer

## Gaps register

| ID | Area | Description | Phase |
|----|------|-------------|-------|
| ~~G1~~ | read/images | **Resolved in Phase 2** — photon wasm pipeline ported byte-identically (see above) | ~~2~~ |
| G2 | parity | racy output orderings (bash stdout/stderr interleave, rg multi-file order) are normalized by `sortLines` rather than matched | n/a (inherent) |
| G3 | parity | spill file contents not compared byte-for-byte (paths normalized); mode (0600) and name format verified | 1 |
| G4 | packaging | `powershell.ts` (Windows shell support) intentionally not ported; Linux/macOS only for now | 3+ |
| G5 | messages | `parseDateMillis` accepts only strict ISO-8601 (session-file format); Node's `new Date()` also parses looser formats ("March 5, 2024"). Unparseable strings → `null` on both sides, so this only affects inputs Node would accept | 3 |
| G6 | messages | `CustomMessage.details` cannot represent an explicit JSON `null` (Rust `Option` omits the key); TS emits `"details": null` when passed null. Callers pass `undefined`/absent in practice | 3 |
| G7 | compaction | LLM-dependent summarization functions (`compact`, `generateSummaryWithUsage`, `completeSummarization`, `generateTurnPrefixSummary` and the summarization prompts) are deferred until the provider/agent layer exists; all pure logic (estimation, cut points, `prepareCompaction`, `getSummarizationFailure`) is ported and parity-tested | 3+ |
| G8 | async | `NestedToolCallRunner` (drives the agent tool pipeline) and `file-mutation-queue.ts` (global registration queue + per-realpath mutation queues) need an async runtime and the agent session layer; deferred with the agent-loop tranche. `NestedCallRecorder` — the persisted half — is ported | 3+ |
## Parity suite status

- 1262/1262 cases pass (stable across repeated runs; includes the built-in
  tools, core-runtime modules, and the TUI groups: `tui-utils`,
  `tui-colors`, `math`, `tui-word-navigation`, `tui-keys`,
  `tui-stdin-buffer`, `tui-components`, `tui-editor`, `tui-main`,
  `tui-alt`, `tui-terminal-colors`, `tui-terminal-image`,
  `tui-markdown-lex`, `tui-latex`, `tui-markdown-render`; 79 astral/
  surrogate regression cases added by the T6 redteam — see
  docs/report-t6-R1.md and docs/report-t6-R2.md — plus 58 fuzz
  regressions auto-written by the T7 batter — see the T7 section)
- 1228 Rust unit tests pass (pi-core, pi-tools, pi-session, pi-cli, pi-tui;
  plus fuzz corpora: ansi 2010 cases, decoder 800 cases, pi-photon
  byte-parity, segmenter corpus 1208 cases)
- `cargo clippy --all-targets`: 0 errors, ~160 warnings. The warnings are
  predominantly intentional: ~56 non_snake_case mirrors of TS field names
  (e.g. markdown token fields) and small style lints in faithful-port code;
  the fdlibm constants/idioms are `allow`-annotated with justification.

## Changelog

- T7 complete: differential fuzz batter (parity/fuzz/ — seeded JS
  generators, batched both-harness execution with crash fallback, ddmin
  shrinker, auto-written permanent regressions aggregated into the main
  parity suite). First runs found 5 divergence classes (14 regressions):
  marked prevChar UTF-16-unit semantics, the block-loop substring-clamp
  panic, AnsiCodeTracker unanchored-SGR extraction on oversized CSI
  tokens, OSC-8 close not clearing the active link, and a GB11
  double-ZWJ lookback bug in the custom segmenter — all fixed. Batter
  clean at scale (unicode 21k, keys 96k sub-cases, stdin 40k sub-cases,
  latex 16k, markdown 16k, all 0 divergences); parity 1218/1218;
  1307 tests; clippy 0 errors.
- Phase 1 complete: all built-in tools ported, parity suite at 64 cases,
  flaky cases (fd `--max-results` subsets, rg multi-file ordering, bash
  pipe interleave) made deterministic/normalized.
- Phase 2 complete: image processing ported with byte-identical output by
  executing the exact photon wasm binary under wasmtime (`pi-photon`);
  parity suite at 73 cases; binary fixture support in the harness.

- Phase 3 first tranche: process-runtime layer (`shell.rs`: shell config,
  env, process groups, `waitForChildProcess` grace semantics),
  `bash-executor.rs` (streaming, ANSI/binary sanitization, spill files),
  `messages.rs` (custom message model + `convertToLlm`), and the bash tool
  rebuilt on the new layers. Module-level harness entries
  (`bash-executor`, `output-accumulator`, `messages`) drive the original
  dist modules directly for differential coverage; parity suite at 103
  cases.
- Phase 3 second tranche: new `pi-session` crate with the full
  `session-manager.ts` port (JSONL session trees, projection, migrations,
  forking/branching) plus `ai.rs`/`usage_totals.rs` support in pi-core;
  compaction pure logic (`compaction_utils.rs`, `compaction.rs`) and the
  `NestedCallRecorder` (`nested_tool_calls.rs`). Four new module-level
  harness entries and a `normalizeSessionIds` comparison option for
  random-id-generating code paths; parity suite at 239 cases. LLM-dependent
  summarization and the nested-call runner are deferred to later tranches
  (G7/G8).
- TUI tranche 1 (first slice): new `pi-tui` crate with the Unicode
  foundation for the width engine — `scripts/gen-unicode-tables.mjs`
  regenerates `unicode/tables.rs` from the **live Node/V8 runtime**
  (regex property scans over all codepoints, `get-east-asian-width`
  package data, V8 `RGI_Emoji` string set: 3796 sequences) plus UCD 16.0
  Grapheme_Cluster_Break and InCB data (matching Node 24's ICU vintage).
  `unicode/mod.rs` implements UAX #29 GB3–GB999 including GB9c (Indic
  conjunct breaks), validated by a 1208-case golden corpus recorded from
  the live `Intl.Segmenter` (`scripts/gen-segmenter-corpus.mjs`) — ICU is
  the ground truth and diverges from the published UCD expectations in
  exactly one recorded case (U+2701 ZWJ U+2701). `utils.rs` ports the
  width-engine slice of `utils.ts` (ANSI/OSC/APC parsing, terminal
  spacing-mark set, grapheme widths, `visibleWidth`) with the upstream
  visible-width tests ported. 11 unit tests pass. See
  `docs/TUI_PORT_PLAN.md` for the full tranche plan (T1–T7).
- TUI tranche 1b: rest of `utils.ts` (~1400 LOC) ported — OSC 8
  hyperlink parsing/formatting (BEL and ST terminators, tri-state
  undefined/closed/open), `AnsiCodeTracker` (full SGR attribute state
  with raw parameter strings for 38/48 colors, hyperlink state
  surviving SGR resets, line-end reset closing underline + hyperlink),
  `truncateToWidth` (contiguous-prefix ANSI/tabs handling, wide
  ellipsis clipping, padding), `sliceByColumn`/`sliceWithWidth`
  (pending-ANSI ordering preserved), `extractSegments`
  (style-inheritance via tracker, APC markers), `wrapTextWithAnsi`
  with CJK token breaking via new Script_Extensions tables
  (Han/Hiragana/Katakana/Hangul/Bopomofo + Punctuation added to the
  generator, now 34 statics), `normalizeTerminalOutput` (Thai/Lao AM
  decomposition, tab expansion), `getGraphemeCellRange`,
  `getOsc8LinkAtColumn`, `getActiveBackgroundAnsi`. 50 pi-tui unit
  tests (upstream truncate-to-width, wrap-ansi, tab-width,
  slice-by-column-ansi-order tests ported). New `tui-utils` parity
  harness entry on both sides (TS side dynamic-imports the TS source
  via Node 24 type-stripping, `PI_TUI_SRC` env) with 53 cases. Suite:
  292/292 parity, 225 Rust unit tests, clippy clean.
- TUI tranche 1c (float parity): the last 4 parity failures fixed; suite is
  389/389. Two root causes, both interesting:
  1. **serde_json float parsing**: without the `float_roundtrip` feature,
     serde_json's fast parser is up to 1 ULP off on shortest-round-trip
     decimals, silently corrupting parity inputs. Enabled workspace-wide —
     this was corrupting roughly 10% of full-mantissa doubles.
  2. **V8 vs glibc libm**: Node's `Math.atan2/sin/cos/cbrt` come from V8's
     fdlibm port (`src/base/ieee754.cc`, V8 13.6.233 = Node 24's V8) and
     `Math.hypot` from the `FastMathHypot` Torque builtin; glibc's libm (Rust
     std) differs in the last 1-2 ULPs. New `pi-tui/src/fdlibm.rs` ports
     atan/atan2/sin/cos/cbrt (incl. `__ieee754_rem_pio2`/`__kernel_rem_pio2`
     argument reduction and the 66-entry 2/pi table) plus the 2-arg Torque
     hypot; `oklab.rs`/`colors.rs` now call these instead of std. Verified
     bit-exact against Node over 300k random ops (`parity/math-diff.mjs`,
     kept as a permanent differential float hunt: `node parity/math-diff.mjs
     [count]`). `Math.pow`/`sqrt` verified empirically identical to Rust std
     (0/250k mismatches) — no port needed. New batch `math` parity entry on
     both sides reports raw f64 bit patterns as hex; 2 parity cases lock in
     the fdlibm port (basic values + large-argument reduction paths).
  Also fixed `tui-colors/styleText` harness divergence: TS passes
  `options.fg`/`bg` to `foregroundAnsi` unparsed, so a truthy non-Color
  value hits `colorToRgb` → falls through the `switch (color.kind)` → the
  `{ r, g, b }` destructuring throws V8's `Cannot read properties of
  undefined (reading 'r')`; the Rust harness now reproduces that exactly
  (incl. JS truthiness: `""`/`0`/`false` are skipped by `fg && ...`).
  Known model divergence, not yet exercised by tests: TS `Color` is
  structural, so a raw `{kind: "indexed", index: 300}` reaches
  `colorAnsi`'s indexed fast path and emits `38;5;300m` without throwing,
  while the Rust `Color::Indexed { index: u8 }` validates at construction.
  Tranche 1b remainder (fuzzy, kill-ring, undo-stack, program-status,
  oklab, colors, terminal-colors) all green: 92 pi-tui unit tests.
- TUI tranche 1d (word-navigation / ICU word segmentation): COMPLETE.
  `pi-tui/src/unicode/word_break.rs` (~1900 lines) executes Node's actual ICU
  78.3 machinery offline: word.brk RBBI forward state table (58 states,
  8-bit rows), UText, dictionary-engine dispatch (Thai/Lao/Khmer/Burmese SEA
  + CJK Viterbi with NFKC fragment normalization, katakana-cost heuristic,
  inputMap supplementary conversion), BytesTrie/UCharsTrie walks, and the
  DictionaryCache (rbbi_cache.cpp) with process-global factory semantics.
  Binary data is generated offline by `scripts/gen-word-break-data.mjs` into
  `word_break_data.rs` + `data/*.bin`. `word_navigation.rs` operates at the
  UTF-16 level (exact JS `String.slice` semantics — cursors may split
  surrogate pairs). Validation stack:
  - `scripts/probe-word-seg.mjs` validates the JS reference prototype
    (`scripts/lib/word-segmenter.mjs`) vs live Intl.Segmenter; `--emit-fixture`
    writes `data/word-seg-fixture.bin` (10099 cases, UTF-16 units so lone
    surrogates survive) only when the probe is 0-fail.
  - in-crate fixture test replays all 10099 cases through both the u16-level
    and &str paths.
  - `tui-word-navigation` parity harness (both sides) + 20 cases: 409/409.
  - `scripts/batter-word-seg.mjs` — large-scale differential batter with
    MATCHED trajectories (see below). Seeds 777/888/999 × (20000 warm seg +
    1500 cold seg + 1500 cold nav sweeps + 1500 warm nav sweeps) all green.
  - 112 pi-tui unit tests.
  Notable bug fixed en route: BytesTrie/UCharsTrie `branch_next` binary
  search must increment pos in the comparison (`u[pos++]` in JS) before
  jump/skip-delta — otherwise wide trie branches never match.
- TUI tranche 2 (input): COMPLETE. `keys.rs` (~2450 lines, all 61 upstream
  tests; JS number semantics — i64 codepoints, i32 modifiers with wrapping
  ops; 5 TS regexes hand-ported to greedy parsers), `stdin_buffer.rs`
  (~1700 lines, 86 tests — 76 upstream + 4 astral/WTF-16 lock-ins; Vec<u16>
  internals because TS splits astral chars into per-surrogate data events;
  timers → `PendingTimeout` + `fire_timeout()`), `keybindings.rs` (47
  bindings, conflict detection, global manager — SHIP verdict, no fixes),
  `terminal.rs` (~1800 lines, 27 tests; kitty negotiation state machine with
  injectable clock/I/O; hand-declared termios externs, pure std),
  `native_modifiers.rs`/`native_module_path.rs`/`native_platform.rs`
  (behavior port per gaps T3/T4: Linux clipboard via wl-paste/wl-copy/
  xclip/xsel with timeout + 50 MB cap + pipe-hang/EPIPE hardening; win32/
  darwin cleanly unavailable). Validation: 335 pi-tui unit tests + parity
  groups `tui-keys` (378 sub-cases) and `tui-stdin-buffer` (12 entries,
  chunk scripts + fire-timeout steps vs mocked-timer TS) — 434/434 parity.
  Fleet-built: parallel builder + redteam subagents; redteam findings fixed
  (empty-string SSH env falsy, fractional escape-timeout ceil, native
  modifier seam wired, env-dimension floor, clipboard writer pipe-hang,
  Wayland-only sessions, empty DISPLAY gates). Reports in /tmp/report-*.md.
- **ICU engine-factory trajectory semantics** (matters for any differential
  test of word segmentation): ICU's `ICULanguageBreakFactory` engine cache is
  process-global and warms lazily on the first *dictionary dispatch*. A
  leading Script=Common CJ-set char (U+FF70 ｰ, U+FF9E, U+FF9F, U+30FC) merges
  with the following word while the factory is cold (loadEngineFor returns
  null for Common → UnhandledEngine rejects the whole Common script → the CJ
  engine range starts after it), but splits once Cjk is cached (the CJ set
  explicitly covers those code points). Subtleties that make trajectories
  diverge innocently:
  1. `DictionaryCache::populate` no-ops on ranges of length ≤ 1, so
     single-char dictionary prefixes (e.g. "饩" or "ｰ") NEVER dispatch an
     engine and never warm the factory — live ICU behaves the same.
  2. Each `segment()` call gets a fresh instance engine stack (per-iterator),
     but the factory cache persists across calls in-process.
  3. JSON transport can't carry lone surrogates, so cursor sweeps that split
     surrogate pairs can't be shipped as sliced prefixes — send the full text
     + cursors and slice in the harness.
  The batter therefore compares matched trajectories on both sides: warm
  (one pi-cli `batchNav` process warmed by the same corpus sequence as the
  shared Intl.Segmenter) and cold (fresh pi-cli process vs
  `scripts/navref-cold.mjs`, a fresh node whose first segmentation is the
  input). `batchNav` was added to the `tui-word-navigation` pi-cli harness
  entry for the warm phase. The original 3 batter "failures" were all
  trajectory artifacts of comparing cold Rust vs warm live — the Rust port
  was already bit-exact (verified against true-fresh live ICU and the
  prototype on minimal repros).

- TUI tranche 3 (layout + components): COMPLETE. Foundation
  `component.rs` (~800 lines: `Component` trait,
  `ComponentRef = Rc<RefCell<dyn Component>>`, mouse types with i64
  numerics and `Option<bool>` JS-truthiness flags,
  `ComponentMouseResult::{Result, Forwarded}` ≡ TS `"target" in result`,
  `dispatch_mouse_event`/`retarget_mouse_event`, `Container`, `Focusable`,
  `CURSOR_MARKER`/fake-cursor/composite_tui_line) and `layout_node.rs`,
  then `layout.rs` (~2150 lines: full flex solver, LayoutBox tree,
  scrollbar painting, kitty-image crop w/ [T6] markers, hit-testing) and
  all 15 components: stack/h-stack/v-stack (API contract
  `visible_stack_entries`/`allocate_stack_sizes`), text, truncated-text,
  spacer, box, loader (timer → deadline + `tick()`), cancellable-loader,
  mouse-region, scroll-view (~1330 lines), input (~1450 lines),
  select-list, settings-list, autocomplete (~2900 lines with injectable
  FsSeam/CommandRunner seams). Validation: 584 pi-tui tests (583 pass,
  1 ignored), 757 workspace tests, and a new `tui-components` parity
  group (13 cases / ~128 sub-cases: render-line differential at multiple
  widths for every component, stateful scripts — input key sequences,
  select movement, loader ticks, mouse records — with identical
  theme/callback presets on both sides) — 447/447 parity. Fleet-built
  (parallel builders + redteams; reports in /tmp/report-*.md). Notable
  fixes en route: (a) the 6 wind-down test failures were all
  test-harness bugs, not engine bugs — the FakeFs must be
  interior-mutable with clones sharing state (TS reads a live
  filesystem, so files created after provider construction must be
  visible), `add_parents` must not register file/link paths as
  directories (a symlink-to-file then got a spurious trailing `/` in the
  emulated fd output), and one select-list test measured byte indices
  where TS `indexOf` returns UTF-16 code units; (b) redteam findings
  fixed: `Container::handles_input()` must be `false` (TS `Container`
  defines no `handleInput`, so it never steals keyboard focus from a
  focusable child — tui.ts:397), `locale_compare` upgraded from raw
  codepoint ordering to an en-US ICU approximation (case/diacritic-
  insensitive primary weights with ligature expansions +
  lowercase-first tertiary tiebreak; sign-verified against node on a
  20-pair battery), SelectList/SettingsList empty-string description
  JS-truthiness ("" is falsy → no description column/block), and
  SettingsList `select_item` must position within the FILTERED list
  (TS `findIndex` over `filteredItems`), not by item index; (c) layout
  tidy-ups: primary scroll-view tracking skips unregistered views
  instead of clobbering with `None`, h-stack center offset uses
  `div_euclid` (Math.floor semantics). Documented divergences (all
  unreachable via public APIs): `distribute()` floors fractional
  grow/shrink capacities (Stack normalizes sizes to integers),
  MAX_SAFE_INTEGER stand-in `i64::MAX/2`, `LayoutBox.parent` is
  write-only upstream, autocomplete `cursor_col` uses char (Unicode
  scalar) indices vs TS UTF-16 code units (no astral-plane engine input
  identified).

## T4 (editor) — COMPLETE

- Full port of editor.ts (2473 lines) + editor-component.ts across
  editor.rs (coordinator foundation: types/struct/ctor/accessors/sync
  autocomplete surface/token pattern matcher/UTF-16 helpers),
  editor_word_wrap.rs [D2], editor_text.rs [E2], editor_cursor.rs [E3],
  editor_history.rs [E4], editor_render.rs [E5], editor_input.rs [E6],
  editor_autocomplete.rs [D1b: the TS promise chain as an explicit state
  machine — thread-local per-editor continuation maps; host surface
  run_pending_autocomplete_requests() = flushAutocomplete,
  tick_autocomplete_debounce(ms) = mock-timer tick], and
  editor_component.rs (extension-surface trait).
- Fleet model that worked: coordinator writes the struct + types +
  binding-signature todo!() stubs, then ONE FILE PER DELEGATE in
  parallel; delegates write reports incrementally to /tmp (all result
  channels failed; all work landed). Fleet reports preserved in
  docs/t4-reports/.
- Parity: `pi-cli harness tool tui-editor` (run_tui_editor_harness)
  mirrors ts-harness.js per docs/T4_PLAN.md §6 → 447 + 118 = 565/565.
- pi-tui: 831 tests + 1 ignored; workspace 1005 passed / 0 failed;
  zero warnings.
- Redteams (3 scopes: text-ops/input, render/layout/mouse,
  history/kill/undo/autocomplete; reports in docs/t4-reports/):
  text-ops and render scopes verified faithful; the autocomplete-scope
  redteam found 5 issues, all fixed with regression tests (commit
  056b0c1): cancel_autocomplete_request now aborts the in-flight
  continuation at cancel time (cancelled requests could previously
  resurrect on flush and even auto-apply completions); pipeline slots
  are per-editor (IN_FLIGHT/DEBOUNCE_ELAPSED keyed by editor identity,
  matching the TS per-instance autocompleteRequestTask/setTimeout);
  removed the dead autocomplete_aborted field.
- Documented divergences / faithfully replicated upstream quirks:
  - wordWrapLine recurses infinitely when a single grapheme's visible
    width exceeds maxWidth (e.g. "你" at width 1) — TS stack-overflows
    identically; unreachable by upstream tests (widths >= 8).
  - Forward-deleting a paste marker leaves a stale pastes-registry
    entry in both implementations (upstream quirk, ported faithfully).
  - The positive shouldSubmitOnBackslashEnter path (plain Enter with
    remapped tui.input.submit = shift+enter) is covered by the parity
    differential only — unit-testing it requires mutating the
    process-global keybindings, which races cargo test's parallel
    test threads (see editor_input.rs note). The negative path
    (Alt+Enter) is unit-tested.
  - The async pipeline aborts eagerly at cancel time (TS parity) but
    the provider call happens synchronously at request start (Rust's
    sync provider trait) rather than after a microtask hop —
    unobservable through the public surface.

## T5 — TUI alt-screen (COMPLETE; last updated 2026-10-18)

- Waves A+B complete and committed (latest: 91d7970).
  - Wave A (c15deac, 683f3a2 skeletons): tui_main_screen.rs (D-A2),
    alt_screen_search.rs (D-A3), TuiAltScreen core/shared container/
    park-render bridge, cross-module seams.
  - Wave B (91d7970, ~6.2k LOC): alt_screen_input.rs (D-B1: SGR mouse/
    wheel parsing, viewport input pipeline, scrollbar interactions,
    scroll-to-prompt, scroll-to-end indicator, lifecycle ESC),
    alt_screen_selection.rs (D-B2: selection state machine — character/
    word/line granularity, multi-click, applySelection/Highlight, active
    text, copy-on-select, virtual-clock auto-scroll), alt_screen_render.rs
    (D-B3: doRender differential writes, line resets, OSC133 strip +
    fake-cursor resolution, kitty image cache, indicator/flash
    compositing). Worktrees wt-B1/B2/B3 + branches removed after merge.
- Status: pi-tui 1000 tests green / 0 warnings (lib + release);
  parity 665/665; cargo build --release clean.
- Real TS bugs found & fixed during integration (probe-verified):
  - tui_overlay.rs clamp order: TS does `max(marginTop, min(row,
    termHeight - marginBottom - effectiveHeight))` — inner min FIRST
    (tui.ts:1303-1306). Rust had max-then-min, shifting oversized
    overlays up one row (found via do_render_height_slice test + probe).
  - Selection auto-scroll interval (TS setInterval 50ms) is modeled as a
    virtual-clock deadline; pump wired into advance_clock via new
    pump_screen_timers hook on impl_tui_surface! (all 3 surfaces
    implement it; alt screen pumps selection auto-scroll).
- Key TS ground-truth facts re-learned via node probes (keep
  /tmp/probe-sel.mjs, probe-scrollbar.mjs, probe-render.mjs,
  probe-prompt.mjs patterns):
  - TuiAltScreen's implicit ScrollView uses scrollbar:'hidden' — scrollbar
    tests need an explicit layout-root ScrollView with scrollbar:'always'
    (upstream tui-alt-screen.test.ts:481-507); transient 'auto'
    scrollbars only appear after markScrollbarActivity.
  - doRender differential writes ONLY changed rows (changedRows map).
  - scrollToPrompt's scrollTo clamps to maxScrollTop (contentHeight -
    viewportHeight): a "next prompt" below the max scroll is unreachable.
  - TS overlay height-slice keeps the LAST termHeight rows of the padded
    working frame.
- Test-helper invariants: set alt_screen_active=true in new_screen;
  SGR input coords 1-based in feed(); Text pads every line to width;
  drag clears lastClick/pressedUrl; second press within
  DOUBLE_CLICK_INTERVAL_MS (500ms virtual clock) = word selection;
  document_frame() strips the applyLineResets SEGMENT_RESET suffix to
  reconstruct the pre-reset frame applySelection receives.

### Remaining T5 work (resume here)
1. ~~Wave C~~ **COMPLETE** (commits d452785, 187510c, 1872c4d):
   - Harness 'tui-main'/'tui-alt' both sides (script ops: addChild,
     input, terminalReply, setFocus, showOverlay/hideOverlay,
     requestRender/renderNow, resize, tickMs, start, stop; alt-only
     setLayoutRoot, scrollBy, flash). Frames split at \x1b[?2026h.
   - 100 cases authored in parity/wavec/ (shared builders in
     wavec-helpers.mjs): main-cases.mjs (32, ports of tui-render/
     tui-shrink/tui-cell-size-input), alt-cases.mjs (43, ports of
     tui-alt-screen behaviors), interaction-cases.mjs (25, ports of
     wheel-scroll/tui-overlay-style-leak/overlay-options/
     overlay-non-capturing). All validated against TS before merge.
   - Final: parity 665/665; pi-tui tests 1000/1000.
   - Real Rust bugs found & fixed by the cases (all TS-probed first):
     - Input/Editor lacked Component::as_focusable → setFocus never set
       the focused flag → missing hardware-cursor writes.
     - consume_cell_size_response re-borrowed the surface during input
       dispatch (RefCell panic) → deferred via cell_size_invalidate_
       pending drained in run_pending_ticks.
     - Loader/CancellableLoader setInterval never fired (TS arms a real
       interval in restartAnimation) → LoaderClock registry pumped by
       advance_clock's early tier.
     - HStack/VStack lacked container_children → dispatchMouseToLayout
       skip rule (inherited Container.handleMouse) wrongly dispatched
       clicks through stacks.
     - Scroll-view requestRenderCallback seam (scroll-view.ts:192) was
       dropped in the port → setScrollbarActive/hide-timer expiry
       renders went nowhere; now parked in a thread-local and drained
       into request_render in TuiBase::drain_parked_render_request
       (fixes scrollbar reveal/hide frames).
     - pump_screen_timers_until didn't report selection auto-scroll
       fires → advance_clock's defensive break stopped the interval
       after one tick (selection-autoscroll-edge off-by-one).
   - Debug pattern kept: node /tmp/casediff.mjs <tool> <case-substring>
     [show] runs one case on both sides and prints field diffs.
2. ~~Redteams R1-R3~~ **COMPLETE** (committed with the R3 hardening;
   reports in docs/report-t5-R{1,2,3}.md): all three subsystems clean.
   R3 hardened one unreachable seam (JS `matches[-1] === undefined` vs
   Rust `.max(0)` fallback in update_search_query + reveal lookup).
   T5 is now DONE (harness + 100 cases + redteams; 665/665 parity,
   1000/1000 tests).
3. After T5: T6 markdown/latex/terminal-image remainder + marked-lex
   decision; T7 batter framework; ~123 pre-existing pi-tui clippy
   warnings or correct TRACKING.md claims.

## T6 — Media: terminal-image, marked, markdown, latex (COMPLETE; last updated 2026-10-18)

- **terminal-image remainder** (062c9e3): `encodeKitty`, `encodeITerm2`,
  image-dimension parsers (PNG/GIF/WebP), `calculateImage*`,
  `renderImage`, `hyperlink`, `imageFallback` — the T5 slice had covered
  capabilities/detection/crop only. 67 parity cases
  (`tui-terminal-image`).
- **Marked decision resolved → option (a), hand-port** (a336c30, be941aa,
  86b4fd5): the full `marked` lexer/tokenizer was hand-ported to
  `pi-tui/src/marked.rs` — rules generated from a live Node regex dump,
  JS-semantics helpers, and the token model exactly as `markdown.ts`
  consumes it. Validated by the `tui-markdown-lex` differential entry:
  124 cases, TS-validated before merge; full suite 856/856 at the time.
- **Image component** (b6d9d19): `components/image.rs` port with the
  registered seams; latex/markdown stubs at that point.
- **latex renderer** (54fdf12): full port of `latex.ts` (1506 LOC) →
  `pi-tui/src/latex.rs` — the Unicode-text math renderer (symbols,
  scripts, fractions, sqrt, matrices, accents, …).
- **markdown renderer** (8340a70): full port of `components/markdown.ts`
  (1038 LOC) → `pi-tui/src/components/markdown.rs` — block layout over
  the marked token stream, theme callbacks, options
  (`preserveOrderedListMarkers`, `preserveBackslashEscapes`,
  `renderLatex`), inline styling.
- **Wave D parity cases** (this commit): `parity/waved/latex-cases.mjs`
  (125 cases; `tui-latex` — drawn from latex.test.ts plus the
  CommonMark/LaTeX shapes markdown relies on) and
  `parity/waved/markdown-render-cases.mjs` (96 cases;
  `tui-markdown-render` — headings, emphasis, links, code, quotes,
  lists, tables, HTML, streaming/partial-fence shapes, options matrix,
  under a deterministic marker theme identical on both sides). Harness
  entries wired on both sides (`pi-cli harness tool tui-latex|
  tui-markdown-render`, TS dynamic-imports the sources). Final:
  **parity 1125/1125, 1307 Rust tests, clippy 0 errors.**
- Clippy error fixes (pre-existing, surfaced by rustc 1.97 lints):
  fdlibm.rs now `allow`s `approx_constant`/`excessive_precision`/`eq_op`
  with justification (verbatim fdlibm literals and the `x - x` NaN
  idiom); `tui_main_screen.rs` `total_lines <= 0` → `== 0` (usize —
  behavior-identical). The remaining ~190 warnings are documented in
  the parity-status section above (mostly intentional TS-field-name
  mirrors); the "clippy clean" claim was corrected rather than
  mechanically renaming ported fields.
- Remaining after T6: T7 batter framework (seeded generators + shrinker,
  see TUI_PORT_PLAN.md); agent-session layer (Phase 3 continuation:
  `file-mutation-queue`, MCP/extensions, `agent-session*`, providers).

### T6 redteam R1/R2 (COMPLETE — see docs/report-t6-R1.md, docs/report-t6-R2.md)

Status as of 2026-10-18 21:00 — both redteam reviews completed
(coordinator re-ran them after the delegate turn-limit failures; findings
recovered from scratch batteries and re-verified), confirmed divergences
fixed, full validation green.

1. **[FIXED] JS regex `.` / `\s` / m-flag-anchor semantics in marked
   rules + JS `trim()` semantics.** All three divergence classes were
   fixed in one pass:
   - `rules.rs`: statics are now regenerated RAW from marked v18
     (`scripts/dump-marked-rules.mjs` + `scripts/gen-marked-rules.mjs`)
     and translated at compile time by the rewritten
     `translate_js_pattern(src, multi_line)`:
     - `.` (outside classes) → `[^\n\r\u{2028}\u{2029}]` (JS dot);
     - `\s`/`\S` → explicit JS whitespace set (Unicode-aware; includes
       U+FEFF, excludes U+0085 — Rust `\s` differs on both), also
       inside character classes;
     - `\d`/`\w` (+`\D`/`\W`) → ASCII classes, now ALSO inside
       character classes (e.g. `[\w:-]` in the tag rule);
     - `[\s\S]` idiom kept verbatim (== "any char" in both engines);
     - m-flag `^`/`$` → explicit JS line-terminator anchors (JS m-mode
       treats `\r` U+2028 U+2029 as terminators; Rust only `\n`);
       builder multi_line stays OFF.
   - `helpers.rs`: added `is_js_whitespace` + `js_trim`/`js_trim_start`/
     `js_trim_end` (JS trim removes WhiteSpace+LineTerminator = the
     `\s` set; Rust `str::trim` differs on U+0085/U+FEFF). Replaced
     every marked-port `.trim()`/`.trim_end()` that mirrors a JS
     `String.trim*` call (fences lang, heading text, list marker/items,
     lheading, table row/cells, latex block text, link href/title,
     splitCells).
   - `inline.rs`: local JS_WS/JS_DOT consts now re-exported from
     `rules.rs` (single source of truth).
   - Verified: 143/143 whitespace probe cases match
     (`scripts/probe-ws-parity.mjs`, 13 ws chars x 11 block shapes),
     including the original repros (`# aa\u2028bb`, `#\u00a0foo`,
     BOM fence langs, `a\ufeffb\n===` setext, NEL cases).
   - Regression cases added: 30 lex + 19 render (ls/ps/nbsp/bom/nel
     shapes) in parity/waved/markdown-lex-cases.mjs and
     markdown-render-cases.mjs.
   Full suite after fix: 1125/1125 parity, 1307 Rust tests, clippy 0
   errors. Note: `cargo fmt -p pi-tui` was run by mistake once and
   reverted wholesale — pi-tui is deliberately NOT rustfmt-formatted.

2. **[DONE — no divergence found] markdown/marked redteam (R1).** The
   delegate (20261019_1) hit its turn limit after identifying the astral
   root cause (in the latex engine); its 38-case batch battery had a
   harness-invocation bug (`harness tool` subcommand missing) that made
   every Rust response an error — after fixing the wiring, 38/38 PASS.
   A directed 59-case astral probe (emoji/math-alphanumeric/CJK-ext-B/
   ZWJ/flag/skin-tone/keycap through every block+inline structure,
   including latex-block/inline passthrough): **59/59 MATCH** — the
   marked port is immune to the UTF-16-unit class of bug because its
   rules are class/literal-based, never slicing at code-unit
   granularity. Report: docs/report-t6-R1.md.
3. **[DONE — one divergence class found and FIXED] latex redteam (R2).**
   Delegate 20261019_2 built three batteries (936-case static-table
   sweep, 149-case structural battery, 600-case deterministic fuzz)
   before hitting its turn limit; re-run directly, all 0 mismatches
   except the astral class. **Root cause:** the TS latex lexer is
   UTF-16 code-unit indexed — an astral char in a single-unit argument
   position (`x^😀`, `\sqrt 𐀀`, `\hat😀`, …) yields only the high
   surrogate, leaving a lone low surrogate in the output (TS renders
   `x^😀` display as `" \ud83d\nx\ude00"`). Rust `char` cannot hold a
   lone surrogate, so 14/32 official astral repros mismatched.
   **Fix (this commit):** sentinel-pair encoding in latex.rs (high
   surrogates U+D800..U+DBFF → U+F1000..U+F13FF, low U+DC00..U+DFFF →
   U+F1400..U+F17FF — plane-15 PUA above TS latex's own U+F0000..U+F0005
   markers; applied via `js_units` at every parser entry including
   render_nested re-parses, existing sentinels pass through), with
   three boundary treatments: code-point semantics sites (JS Array.from
   equivalents + u-flag regexes) coalesce pairs
   (`js_code_points`/`with_coalesced_pairs`); the width path
   (visible_width in utils.rs) coalesces pairs (Intl.Segmenter joining)
   and treats lone sentinels as zero-width (TS \p{Surrogate}); the
   harness output boundary (pi-cli main.rs) decodes sentinels so parity
   JSON matches JSON.stringify of lone surrogates. Known accepted
   limitation (documented in latex.rs): literal U+F1000..U+F17FF input
   would be misdecoded — real pi usage never produces those code
   points. Report: docs/report-t6-R2.md.
   - Static sweep artifact dismissed: `SPACING_COMMANDS onlyTs:[""]
     onlyRs:[","]` is an audit-script bug (naive comma-split mangles the
     array's first element — the string `","`); lists verified identical
     (latex.ts:510-523 == latex.rs:745-748), and a dedicated 32-case
     spacing probe (`scripts/redteam/probe-spacing-cmds.mjs`) covering
     the `\,` case the mangled set dropped confirms 32/32 MATCH.
   - All batteries preserved as re-runnable scripts in scripts/redteam/
     (probe-latex-astral, probe-md-astral, probe-spacing-cmds,
     redteam-md-batch, redteam-latex-sweep, redteam-latex-battery2,
     redteam-latex-fuzz) and re-verified from this commit.
   - Regression cases added: +34 latex (parity/waved/latex-cases.mjs,
     now 158), +33 markdown-lex (now 187), +12 markdown-render (now
     127) — astral shapes through scripts/sqrt/fracs/matrices/accents/
     mathbb, markdown blocks, and latex-block/inline passthrough.
4. **Full validation green (this commit):** parity **1204/1204** (was
   1125), `cargo test --workspace` 1307 passed / 0 failed, clippy
   (pi-tui, pi-cli, --release) 0 errors.

## T7 — Differential fuzz batter (COMPLETE; last updated 2026-10-19)

Seeded differential fuzzing framework per docs/TUI_PORT_PLAN.md: JS-side
generators produce argument payloads; both harnesses replay them in batch;
divergences are ddmin-minimized and auto-written as permanent parity
regressions.

### Framework (all under parity/fuzz/)

- **lib.mjs** — mulberry32 seeded PRNG (case generation is JS-only; Rust
  just replays the emitted args — no cross-language PRNG needed),
  unicode corpus samplers (POOLS, weighted 50% toward risky pools:
  combining marks, ZWJ, sentinels, Cn, Hangul jamo, line/para
  separators), ANSI-dense line sampler, byte/UTF-16-unit string
  samplers, canonical JSON compare, batched `runBatch` (crash-hardened:
  on a harness crash it replays one-by-one and reports the crashing case
  as a divergence), `checkCase`, ddmin delta-debugging shrinker, and
  `writeRegression` (auto-writes parity/regressions/reg-<target>-NNN.mjs).
- **run-batter.mjs** — orchestrator. Targets: `unicode` (tui-utils, 13
  fns, default 3000), `keys` (tui-keys, 1200 payloads × 40 sub-cases),
  `stdin` (tui-stdin-buffer, 800 payloads × 25 sub-cases with
  string/bytes/u16-unit chunks incl. lone surrogates), `latex`
  (tui-latex, 2000), `markdown` (tui-markdown-lex, 2000). CLI:
  `--target all|csv`, `--count`, `--seed`, `--budget` (default 900s),
  `--no-shrink`, `--list`. Exit 1 on divergence. Seed derivation:
  `rng(seed * 7919 + name.length * 104729 + name.charCodeAt(0))`.
- **Batter expansion (2026-10-19)** — four TUI-layer targets:
  `components` (tui-components native batch, 1200 payloads × ~20
  sub-cases: spec-tree generators over text/box/stack/scroll-view/
  editor/loader/select-list/mouse-region + scripts of inputs/ticks/
  renders/scroll/mouse at random widths), `editor` (tui-editor, 1500:
  full editor lifecycle — options, history, setText, insertTextAtCursor,
  pastes gated to render widths ≥ 24, key scripts and renders; narrow
  mode 2–23 exercises the width-exceeds paths), `main` (tui-main, 800)
  and `alt` (tui-alt, 800): screen scripts (addChild/showOverlay/
  setFocus/input/terminalReply/resize/renderNow/tickMs/start/stop +
  alt-only setLayoutRoot/scrollBy/flash) over the component generators,
  with per-target structured minimizer (`minimizeDeep`: del-elem /
  del-key / num→0|1 / code-point-safe string halving; PROTECTED_KEYS /
  FROZEN_KEYS keep harness-identity fields and reject lone-surrogate
  candidates; screen shrink guards keep setFocus refs registered and
  script-op required fields present).- **Batch harness modes** — `pi-cli harness tool-batch <tool> --cwd`
  (stdin JSON array of arg objects → array of results, errors in-band)
  and `node parity/ts-harness.js <cwd> --batch` (stdin array of
  {tool, args}); both amortize process startup over thousands of cases.
- **Regression aggregation** — parity/regressions/index.mjs scans
  reg-*.mjs and flattens into the main parity suite via cases.mjs, so
  every fuzz find becomes a permanent parity case.

### Divergences found and fixed (14 permanent regressions)

1. **marked prevChar UTF-16-unit semantics** (fuzz/reg-markdown-001,
   "🇺_+_") — marked v18's emStrong gate uses `o.raw.slice(-1)`, the last
   UTF-16 UNIT: for a token ending in an astral char that is the lone
   LOW surrogate (unassigned Cn — matches none of punctSpace /
   unicodeAlphaNumeric and never equals the `*`/`_` delimiter). Rust had
   used the full char (So → \p{S} → passed the punctuation gate → wrong
   em token). Fix: marked/inline.rs maps an astral last char to
   `crate::latex::surrogate_sentinel(low)` (U+F1400..U+F17FF, Co — also
   matches none of those classes).
2. **Lexer substring-clamp panic** (fuzz/reg-markdown-002 ">*\n<",
   reg-markdown-003 ">-\n^") — marked's blockquote tokenizer reconstructs
   its raw from line sections and can EXCEED the source (raw ">*\n\n<"
   from src ">*\n<" — 7 UTF-16 units from 5); the block loop advances
   via JS `e.substring(raw.length)`, which CLAMPS out-of-range indices
   to "" instead of throwing, while the Rust byte-slice
   `&src[raw_len..]` panicked ("start byte index 96 is out of bounds
   for string of length 95"). Fix: helpers.rs `js_substring_skip(src,
   units)` skips the raw's UTF-16-unit count clamped at the end;
   replaced at all 16 block-advance sites in lexer.rs (also switched
   the advance amount from raw BYTES to UTF-16 units, the faithful JS
   semantics; a unit count landing mid-astral-char rounds up to the
   next char boundary — &str cannot hold the lone surrogate JS would
   produce, unreachable in practice).
3. **AnsiCodeTracker oversized-token SGR extraction** (fuzz/reg-unicode-
   004..011, 013 — wrapTextWithAnsi / getActiveBackgroundAnsi /
   extractSegments) — `ansiCodeLength`'s CSI scan swallows everything
   up to the first m/G/K/H/J, so one "code" token can span visible text
   and later escape sequences (e.g. "\x1b[?2026h…\x1b[1;38;2;10;20;30m").
   TS extracts the params via the UNANCHORED regex
   `ansiCode.match(/\x1b\[([\d;]*)m/)` and still finds the embedded
   well-formed SGR; the Rust port used anchored strip_prefix/suffix,
   which rejected the whole token and silently dropped the state update
   (missing line prefixes / active background). Fix: utils.rs
   `sgr_params()` — first-match search for "\x1b[" + maximal digit/;
   run + "m".
4. **OSC-8 close does not clear the active link** (fuzz/reg-unicode-012,
   getOsc8LinkAtColumn) — TS's regex `/^\x1b\]8;[^;]*;([^\x07\x1b]*)
   (?:\x07|\x1b\\)$/` MATCHES the close sequence with an empty group,
   and `hyperlink[1] || undefined` then CLEARS activeUrl; the Rust
   `osc8_open_url` returned None for the close and left the stale URL
   active. Fix: utils.rs `osc8_regex_exec()` — three-valued (no-match /
   clear / set) faithful port of the regex incl. its backtracking
   semantics (params may not contain ';', URL may not contain BEL/ESC).
5. **GB11 ZWJ-chain lookback** (fuzz/reg-unicode-014, getGraphemeCellRange
   — "⌛ ZWJ ZWJ ⌚ ᆨ U+20000 U+20000 ° TAB") — the custom UAX #29 segmenter kept
   "⌛ ZWJ ZWJ ⌚" as ONE cluster; ICU/V8 splits ["⌛ ZWJ ZWJ", "⌚"]:
   GB11's lookbehind is ExtPict Extend* ZWJ, and a ZWJ is neither
   Extend nor ExtPict, so a SECOND consecutive ZWJ does not qualify.
   The state machine set `zwj_ok = extpict_run` on a ZWJ without
   resetting `extpict_run`, letting the second ZWJ inherit the first
   one's qualification. Fix: unicode/mod.rs resets `extpict_run` after
   consuming a ZWJ. The 1208-case live-ICU segmenter corpus still
   passes unchanged.

### Batter-expansion divergences and fixes (2026-10-19)

44 permanent regressions from the components/editor/main/alt targets
(reg-components-015..034, reg-editor-036, reg-main-036..058,
reg-alt-001..002), plus harness/framework hardening:

1. **Nested loader ticks** (reg-components series) — the components
   harness 'ticks' step fired only the top-level built loader; a
   loader nested inside a box never advanced (ts "⠙" vs rs "⠋").
   Fix: `register_loader()` in the harness + new
   `loader::tick_all_live_loaders()` (fires one synchronous tick on
   every live ARMED loader, modeling the TS event loop firing all
   armed `setInterval` callbacks); TS harness setInterval patch moved
   from single-slot to a Map of all armed callbacks.
2. **Stack-entry visible contract** (components + screen builders) —
   TS maps `entry.visible === "always" ? () => true : () => false`:
   ANY other present value means never-visible. Rust matched only
   `"never"`. Fix: `_ if entry.get("visible").is_some() => false`.
3. **mouse-region recording gate** — the Rust components harness
   attached the recording handler to EVERY mouse-region; TS only
   records when `onMouse === "record"`. Fix: conditional handler.
4. **BOM history entries** (reg-editor-036) — JS `String.trim`
   treats U+FEFF as whitespace, Rust `str::trim` does not, so a
   BOM-only entry was rejected by TS `add_to_history` but kept by
   Rust. Fix: `editor_history::add_to_history` uses
   `utils::js_trim`.
5. **ScrollView input presence** (reg-main-046..053) — TS `ScrollView`
   defines no `handleInput` method, so the
   `if (this.focusedComponent?.handleInput)` gate in
   `handleTerminalInput` DROPS keyboard input to a focused
   scroll-view (no `requestImmediateRender` either — the render
   timer stays pending, nothing is written). Rust's
   `ScrollView::handles_input()` wrongly returned `true` (a stale
   comment claimed the Container foundation modeled presence as
   true; it models `false`), so Rust dispatched the input and
   rendered an empty synchronized-output frame. Fix:
   `handles_input() == false`, aligning the gate and both
   mouse-forward focus-target sites.
6. **Alt-screen stop fake-cursor resolution** (reg-alt-001) — TS
   `afterTerminalStop` renders the final document through
   `resolveFakeCursors` (fake-cursor markers → reverse video
   `\x1b[7m/\x1b[27m`); the Rust port skipped the call and wrote
   the raw `\x1b_pi:fc\x07` markers. Fix: `resolve_fake_cursors`
   applied in `alt_screen_input::after_terminal_stop`.
7. **ScrollView container traversal** (reg-alt-002) — TS
   `ScrollView extends Container`, so `containsComponent`'s
   `instanceof Container` recursion walks the inherited child list
   (the constructor's `this.children.push(child)`); the Rust
   `ScrollView` exposed no `container_children`, so
   `isComponentMounted` returned false for anything inside a
   scroll-view and `setFocusInternal`'s blocked-overlay-restore
   resolution fired early (focus jumped to the overlay). Fix:
   `ScrollView::container_children` returns the inherited list
   (also aligns the alt-screen mouse-dispatch skip, whose TS check
   is `handleMouse === Container.prototype.handleMouse` — true for
   the inherited ScrollView method).

Framework/harness hardening driven by the same runs:

- **wordWrapLine upstream recursion** — a single grapheme wider than
  the wrap width recurses `wordWrapLine(grapheme, maxWidth)` with
  IDENTICAL arguments (editor.ts:188-205): infinite recursion in
  BOTH implementations. TS surfaces it as an in-band RangeError;
  Rust aborts the process. Untestable class (faithful port,
  not fixed): `runBatch` reconciles TS "Maximum call stack size
  exceeded" + Rust stack-overflow abort as `skipped` (ok, no
  regression recorded); the editor generator gates pastes to
  render widths ≥ 24 (emoji ZWJ clusters remain skip-class).
- **writeRegression numbering** — was `existing.length + 1`, which
  collides with surviving files after deletions (10 editor
  regressions were collapsed into one file by this bug). Now
  max existing sequence + 1.
- **Panic → in-band error** — `run_tool_input_catching_panics`
  (catch_unwind around every harness case, single and batch modes):
  a Rust panic becomes an in-band `{"error": <message>}` exactly like
  a TS exception, e.g. the "Rendered line N exceeds terminal width"
  crash both sides now report in-band with the debug-log path
  redacted on both sides (`errMsg()` in ts-harness.js / truncate in
  main.rs). run-parity.mjs now prefers parsed stdout over the
  exit-status/stderr fallback.
- **Harness contract alignment for missing op fields** — the
  minimizer deleting required fields used to expose CONTRACT gaps:
  input/terminalReply `data` (TS crash vs Rust ""-default — TS now
  defaults `?? ""`), tickMs `n` (Rust errored; TS's `clock +=
  undefined` = NaN semantics now modeled via `advance_clock(NAN)`),
  resize `cols`/`rows` and scrollBy `lines` (both harnesses now
  throw the same in-band "X requires Y" errors). The minimizer also
  gained a screenOpsValid guard so shrunk cases keep required fields
  and stay generator-reachable.

### Validation (this tranche)

- Batter, final binary: unicode seeds 1–5 + all-targets seeds 1–2 →
  21,000 cases, 0 divergences; keys 96,000 sub-cases, 0 divergences;
  stdin 40,000 sub-cases, 0 divergences; latex seeds 1–8 → 16,000
  cases, 0 divergences; markdown seeds 1–8 → 16,000 cases, 0
  divergences. (all-targets runs hit the 900s budget before
  latex/markdown; those targets were run separately at 8 seeds.)
- Batter expansion, final binary: components/editor/main/alt seeds
  1–3 → **81,300 cases, 0 divergences** (components 24,000/seed,
  editor 1,500/seed, main 800/seed, alt 800/seed; the wordWrapLine
  stack-overflow class reconciled as skipped).
- Full parity: **1262/1262** (1204 prior + 58 fuzz regressions: 14
  earlier [markdown 3, unicode 11] + 44 from the expansion [components
  18, editor 1, main 23, alt 2]).
- `cargo test --workspace`: 1228 passed / 0 failed.
- `cargo clippy --workspace --all-targets`: 0 errors.

### Phase 3 roadmap: remaining layers (planned 2026-10-19)

Port plans researched and written by parallel subagents (see `docs/plans/`):
`phase3-providers.md` (provider/model/auth layer, ~4,400 LOC TS → new `pi-providers`
crate), `phase3-mcp-extensions.md` (MCP + extensions subsystem; phase-3 scope is the
pure logic: `mcp-servers.ts`, mcp tools/resources, tool-search), `phase3-agent-session.md`
(agent loop, sliced into parity-green tranches), `phase3-cli.md` (CLI/TUI app layer,
`config.ts` + Stage-A pieces first).

- [x] **G8 async-runtime decision**: tokio with `new_current_thread()` runtime for the
  agent-session layer; providers and MCP parity layers stay fully synchronous (pure
  logic + sync traits with scripted fakes). Rationale and layer scoping in
  `docs/decisions/G8-async-runtime.md`.
- [x] `crates/pi-providers` scaffolded (Cargo.toml, module stubs, workspace member).
- [~] Providers tranche: **IN PROGRESS — see "Current checkpoint" at top.**
      Foundation wave COMPLETE and parity-green (compiles, 1517 tests,
      1390/1390 parity incl. 102 new module-level cases for config-value,
      http-dispatcher, auth-guidance, models-store, runtime-credentials,
      virtual-models). Remaining:
      config/resolution (model_config, model_resolver, provider_attribution)
      → auth/composer (provider_composer, remote_catalog) → facades
      (model_runtime, model_registry) — each wave parity-green via new
      module-level harness entries, then batter targets + regressions.
- [x] MCP pure-logic tranche: `pi-core/src/mcp_servers.rs`, `mcp_tools.rs`,
      `tool_search.rs`. All pi-core tests green, clippy clean, parity green
      (21 `mcp-*` + 5 `tool-search` cases + V8-vs-libm log divergence vectors
      in `math/basic`). Implementation notes: `docs/plans/impl-notes-mcp-pure.md`
      (fdlibm `js_math_log` port, protocol-violation V8 error parity, WHATWG
      URL subset deviations, fake-saver harness design).
- [x] CLI Stage A: `pi-cli/src/config.rs`, `migrations.rs`, `cli/args.rs`
      (commit `626e091`).
- [x] Session-manager + small-modules wave (commits `7bddbaa`, `171de2b`):
      `session_manager.rs` (JSONL sessions, streaming load, migrate,
      projections), `nested_tool_calls.rs`, `pi_manifest.rs`,
      `output_guard.rs`, `defaults.rs`, `diagnostics.rs`. Parity: 261 new
      cases — `session-manager` (62 fn-style) + `sess-manager` (129
      op-style/manager) + nested-tool-calls (28) + pi-manifest (22) +
      output-guard (17) + defaults (3), all fail=0. Divergence fixes in
      `171de2b`: pi-core migrate gate (`header?.version ?? 1` + JS
      relational coercion — no-header migrates, string "2" runs v2→v3);
      quoted-8-hex id normalization inside readFile JSONL strings; fn-style
      migrate id normalization (dist imports randomUUID from "crypto" as
      ESM — the crypto dispatcher override is invisible to it, ESM
      namespace snapshot); ambient cwd → `<cwd>` (TS process.cwd() vs Rust
      AMBIENT_CWD "/"); listDir ENOENT Node-style message; ModelRef
      Option fields (assistant message without provider/model → model {});
      mtimes non-integer silently skipped (Node utimesSync with Invalid
      Date is a no-op). NOTE: two parallel session-manager ports exist —
      crates/pi-session (older, used by the `session-manager` fn-style
      harness) and crates/pi-core (new wave, used by `sess-manager`);
      reconcile/dedupe before the agent-session wave touches sessions.
      ENV NOTE: /tmp gets wiped by a cleaner mid-session — the TS
      reference now lives durably at /home/shinobi/pi-repo with a
      /tmp/pi-repo symlink (recreate with `ln -sfn /home/shinobi/pi-repo
      /tmp/pi-repo`); write long-running outputs under /home/shinobi.
- [x] builtin-catalog.json refresh (2026-10-20, later): the TS reference
      was rebuilt (fresh clone + npm run build) and packages/ai's build
      regenerated provider data with `compat.supportsStrictMode` etc. that
      the vendored Rust catalog (1713 entries) lacked — full parity showed
      mr-models-all / reg-getall / reg-register-virtual failures. Fixed by
      regenerating `builtin-catalog.json` from the current dist data via
      new `scripts/gen-builtin-catalog.mjs` (1731 models, 42 providers,
      entry-for-entry equal to TS; provider order taken from
      providers/all.js import order — NOT filename sort, because '-' < '.'
      would put google-vertex before google). model-runtime 54/54 and
      model-registry 31/31 green again.
- [~] Agent-session tranches (tokio per plan §5.5 / phase3-agent-session.md
      §3 — new_current_thread runtime, no tokio::spawn inside the loop):
      **tranches 0–5 COMPLETE and parity-green.**
  - Tranche 0 (307f858): pi-agent crate skeleton + `agent-queue`
    harness entry (steer/followUp queue surface).
  - Tranches 1–2 (2398a5c): message conversion (`declare_tool_changes`,
    `default_convert_to_llm`) + tool-call pipeline (`run_tool_call` with
    before/after hooks).
  - Tranches 3–4 (f5b19b3): `stream_assistant.rs` (streamAssistantResponse
    partial-message replacement, thinkingLevel last key) + `loop.rs`
    (runAgentLoop sequential/parallel, steering re-poll, explicit
    continuation, length-stopReason failure).
  - Tranche 5 (f3f7753): full `Agent` class port (`agent.rs`, ~39 KB) —
    copy-on-assign state, queues, subscribe, prompt/continue/steer/
    followUp/abort/waitForIdle/reset, runWithLifecycle +
    handleRunFailure, createLoopConfig, context snapshots. 25 `ag-*`
    parity cases. Full parity **2491/2491** (parity-full13.txt).
  - JS-semantics traps recorded in commit messages: first-position key
    order, undefined-omitted JSON, first-position spread order,
    Promise.all sequential join, f64 rounding for durationMs.
- [x] **Session-manager port reconciled — pi-core is canonical; the
      pi-session crate is deleted.** Duplicate history: pi-session
      (Oct 16, 3ae4404) ported session-manager.ts + compaction + a
      partial nested-calls recorder (Value-based, ambient std::env
      reads); the Oct-20 core wave (7bddbaa/171de2b) re-ported
      session-manager.ts + the FULL nested-tool-calls module (runner
      included) into pi-core with Generators injection and no ambient
      state — more complete, so it wins.
      Compaction + compaction_utils moved into pi-core, adapted to the
      typed pi-core SessionProjection (Value-form helpers
      projected_entry_values / projection_messages_values /
      entry_context_messages keep the algorithm code unchanged);
      node_join/node_dirname moved to pi-core::paths (private 2-arg
      node_join renamed node_join_2); js_truthy made pub(crate);
      pi-agent's estimate_tokens now resolves through pi-core; pi-agent
      and pi-cli dropped their pi-session deps; crate removed from the
      workspace. Superseded harness entries session-manager (62 cases)
      and nested-calls (12) deleted — covered by sess-manager (129) and
      nested-tool-calls (28); compaction (44) and compaction-utils (18)
      entries now drive the pi-core modules. Tranche 7's runner is
      thereby already ported; only agent-loop integration remains.
- [x] **Tranche 6 — file mutation queue (src/core/tools/file-mutation-queue.ts)
      ported to crates/pi-tools/src/file_mutation_queue.rs.** Keyed by
      realpath of the target file (missing-path errors fall back to the
      resolved path, like TS's realpath-miss branch). Rust uses a tokio
      AsyncMutex per queue key in a process-global map instead of the
      TS realpath-keyed promise chains; the TS registration queue is
      unnecessary here because the key is computed synchronously before
      the map lock (documented divergence). Map entries are removed when
      the last holder releases (Arc ptr_eq check). Wiring the queue
      into write/edit belongs to the tool-definition layer (tranche 9).
      Parity harness `file-mutation-queue` (7 cases, suite total 2424)
      with deterministic gate/release concurrency: ops run as futures
      with awaitGate steps, release rules fire once their afterStarted
      ops begin; TS spins gates with `setImmediate` macrotask yields
      (a Promise.resolve microtask spin starves pending realpath I/O),
      Rust round-robins NoopWake polling with per-future done flags
      (re-polling completed async fns panics). Harness setup
      (realFiles/symlinks) is idempotent on both sides (rm before
      symlink) so stale scratch state can't produce EEXIST. Full parity
      2424/2424.
- [x] **Tranche 8 — AgentSession event handling.** New crate
      `crates/pi-session` (recreated; deps pi-core + pi-agent + serde)
      with the `agent_session/` module tree from the plan: mod.rs
      (AgentSession facade + config, SessionSettingsStore /
      ExtensionHost / NestedCallsStore traits, SessionState, message
      arena, subscribe/_emit, isIdle/waitForIdle, queue steer/follow-up,
      sendCustomMessage, dispose, harness seams), events.rs
      (SessionAgentEvent — AgentEvent with messages replaced by arena
      ids — + to_value), event_handler.rs (_handleAgentEvent port:
      abort-at-agent_start, nested-calls takeRecord merge +
      combineUsage, steering-then-followUp queue removal + queue_update,
      extension-emit-then-public-emit, agent_end willRetry enrichment,
      message_end persistence mapping, assistant bookkeeping +
      auto_retry_end, turn_end flush; _willRetryAfterAgentEnd,
      _findPersistedMessageEntryId, _flushPendingCustomMessages,
      _emitAgentSettled + deferred actions, _emitExtensionEvent).
      retry_error.rs ports isContextOverflow + isRetryableAssistantError
      (overflow.ts + retry.ts pattern lists). pi-agent gained
      Agent::model(); pi-core Generators::with_gens for injected
      uuid/clock in tests. WeakMap-identity emulation: insertion-ordered
      Vec<Value> arena keyed by MsgId + entry_message_ids side-table;
      the harness feeds a shared message pool + messageIndex refs so
      reference-compare semantics stay observable on both sides.
      Parity harness `agent-session-events` (11 cases; suite total
      2435): real AgentSession on the TS side (recording fake extension
      runner, SettingsManager.inMemory, __piParityUuidOverride pinned
      after construction) vs the Rust facade; Date pinned via Date.now
      override + PinnedDate class so new Date().toISOString() matches
      the fixed clock. Full parity 2435/2435.
- [x] Tranche 9 — AgentSession tool registry & loadout (tools.rs,
      ~1071 LOC in crates/pi-session/src/agent_session/): ToolExposure
      algebra (_isAllowedTool/_allowlistFiltersMcp, _isActivatable,
      _isDeclarable, _isActivatedOnRegistration), _applyToolLoadout
      with prepareLoadout hooks (spec-based PrepareLoadoutSpec since
      ExtensionRunner is Phase 4+; hook throw → emitError), _refreshToolRegistry
      (base + extension + custom merges over ordered Vec maps preserving
      TS Map insertion order), _rebuildSystemPrompt (NormalizedSystemPromptOptions),
      _restoreToolsFromTranscript, hidden-declarations + forced-prompt
      projections (stateful AgentSession::transform_context), constructor
      _buildRuntime slice (createAllToolDefinitions unported →
      baseToolsOverride required; default active [read,bash,edit,write]).
      ExtensionHost trait gained get_all_registered_tools(). Parity
      harness `agent-session-tools` (11 cases; suite 2446/2446):
      both harnesses gate the fake extension host behind an `installed`
      flag AND gate emit_error recording the same way — construction-time
      prepareLoadout throws hit the real (empty) runner in TS and are
      invisible; the Rust fake must mirror that or double-record.
- [x] **Tranche 10 — prompt pipeline.** crates/pi-session/
      agent_session/prompt.rs (~700 LOC): prompt (extension-command
      dispatch, input interception, skill/template expansion,
      streaming→steer/followUp queueing, model/auth validation),
      steer/followUp/_queueUserInput, sendUserMessage, image
      normalization seam (processImage seam; cases pin
      autoResize:false), reduced run driver (_startRun/_prepareRun/
      _handlePostAgentRun/_runBeforeSettleBoundary with queue
      re-dispatch), and the agent→session event pump (AgentEvent →
      arena SessionAgentEvent; FIFO drain — one arena slot per
      streaming message; nestedCalls/usage in-place-mutation carry at
      message_end). mod.rs: ModelAuthStore trait, ExtensionHost gains
      get_command/execute_command/emit_input, DeferredSettledAction
      Prompt/SendCustomMessage carriers executed by _emitAgentSettled.
      Parity harness `agent-session-prompt` (14 cases; suite
      2460/2460). Key fixes found in differential: preflight callback
      must read the normalized options object (not op.options —
      undefined without options); TS AgentSession gets
      baseToolsOverride:{} so createAllToolDefinitions (unported) never
      declares builtin tools the reduced Rust prepare_run can't patch;
      skill-name slice must skip the "/skill:" prefix (TS slice(7)).
- [x] **Tranche 11 — run orchestration.** Two parts: 22d3c24
      (library) + 7ea8764 (harness). crates/pi-session/
      agent_session/run.rs (~560 LOC): full _startRun
      (recordSelection, pendingToolNames lifecycle),
      drive_run, _prepareRun (emitBeforeAgentStart + handler
      messages + prepare_prompt_and_tool_loadout system patch),
      _handlePostAgentRun (+_finishCancelledRetry),
      _runBeforeSettleBoundary, boundary emit loop,
      _applyBoundaryDrafts/_createBoundaryPreviewManager/
      _buildBoundaryContext/_commitBoundaryDrafts (turn-end
      boundary drafts with preview-manager uuid/clock sharing),
      abort. system_prompt.rs (~300 LOC): normalize/
      buildSystemPromptSections/diffSystemPromptSections port.
      mod.rs: is_agent_run_active/agent_run_abort_requested as
      Arc<AtomicBool> + abort_tap; ExtensionHost gains
      boundary_handlers/emit_boundary. Parity harness
      `agent-session-run` (12 cases; suite 2472/2472).
      Key fixes found in differential: (a) _buildBoundaryContext
      must use the session-side pi_core::messages::convert_to_llm
      (maps custom/bashExecution/branchSummary → user), NOT the
      agent's default_convert_to_llm (drops custom roles) —
      otherwise continuation reports "without runnable model
      context"; (b) TS keeps the turn loop alive mid-run across a
      finishTurn-continue, so no agent_end fires between continued
      turns — Rust suppresses AgentEnd while
      turn_end_boundary_continue is pending (resumed run owns the
      final agent_end); (c) <cwd> section content was JSON-quoted
      (Value Display quotes strings) — unwrap Value::String;
      (d) build_rules takes declaredTools not selectedTools;
      (e) CustomMessage.display is Option<bool> with
      skip_serializing_if (TS omits undefined display).
      Divergence note: TS system prompt default docs section
      renders host paths — parity cases always pin customPrompt.
- [x] **Tranche 12 — auto-retry with backoff.** crates/pi-session/
      agent_session/retry.rs (~170 LOC): retry_delay_ms (JS f64
      semantics: baseDelayMs * 2^(attempt-1), safe-integer clamp,
      maxAgentDelayMs default 60s cap), prepare_retry (enabled gate,
      attempt increment/decrement-at-max, auto_retry_start with
      errorMessage || "Unknown error", abortable sleep in the shared
      retry_abort slot, rejection → finish_cancelled_retry),
      omit_recovery_attempt (appendContextEdit(targetId, null) +
      entry_appended + refresh_finalized_context; unresolved-projected
      throw), abort_retry/is_retrying/abort_retry_tap. mod.rs:
      Sleeper config injection (None = TokioSleeper; harness
      RecordingSleeper records sleepLog and resolves or holds),
      failed_response: Option<MsgId> bookkeeping, abort_tap also
      aborts the retry handle. run.rs: handle_post_agent_run
      rewritten to full TS _handlePostAgentRun parity (retryable-error
      routing, auto_retry_end failure emission + attempt reset).
      Parity harness `agent-session-retry` (12 cases; suite
      2484/2484): TS side patches globalThis.setTimeout to record
      sleepLog [{ms}] with immediate/pending modes; cases cover
      success/multi-error-then-success/exhausted/disabled,
      non-retryable + empty-error-message classification, backoff
      caps (maxAgentDelayMs and safe-integer 1e300 →
      9007199254740991), abort + abortRetry mid-sleep. Deferred:
      context-overflow routing (goes through _checkCompaction's
      overflow-recovery path — tranche 13; case noted in cases.mjs
      for re-adding there); full _installAgentRequestProjection
      virtual-model "retry" reason (tranche 14).
- [x] **Tranche 13 — compaction.** crates/pi-session/
      agent_session/compaction.rs (new): the async compact()
      driver (split-turn vs plain summarization via the injected
      streamFn seam, summary + formatFileOperations append,
      "First kept entry has no UUID" throw), _checkCompaction
      decision tree (overflow-retry/overflow-complete/threshold
      with stale pre-compaction usage checks, projection
      bookkeeping, _modelForMessage same-model gate,
      is_recoverable_length port), _runAutoCompaction +
      manual compact() (session_before_compact extension
      cancel/custom-result, appendCompaction, savedCompactionEntry
      lookup, session_compact emit, byte-exact compaction_end
      error messages, _emitSessionCompactFailed, abort
      controllers + abortCompaction), _overflowRecoveryAttempted
      one-shot + resets in event handling, pre-prompt
      _checkCompaction(lastAssistant, false) in prepare_run.
      Parity harness `agent-session-compaction` (16 cases; suite
      2500/2500): scenario-driven (overflow-retry,
      overflow-complete, threshold, manual, already-compacted,
      too-small), summarizer driven by the same providerTurns
      script, sleepMode immediate/pending for abort-mid-compaction.
      Re-added the tranche-12-deferred context-overflow-not-retried
      case. Harness determinism fixes: summarizer uuid_v7 never
      consults the parity uuid pin (TS uses random uuidv7) →
      constant "summarization-session"; abortCompaction taps
      without holding the session mutex (TS abortCompaction is
      callable mid-run); session_before_compact event embeds
      preparation with fileOps serialized as JS Sets ({}).
- [x] **Tranche 14 — model & thinking.** crates/pi-session/
      agent_session/model.rs (~690 LOC): setModel (checkAuth
      gate "No API key for p/id", thinking-level switch,
      appendModelChange, persist path + scoped enabledModels
      update), cycleModel/_cycleScopedModel/_cycleAvailableModel
      (snapshot filtering, modelsAreEqual index −1→0, wrap,
      ModelCycleResult isScoped), setThinkingLevel (clamp,
      no-change skip, persist stores requested level,
      thinking_level_changed + thinking_level_select),
      cycleThinkingLevel, getAvailableThinkingLevels/
      supportsThinking, _getThinkingLevelForModelSwitch
      (explicit > per-model > global default),
      ModelRouteStore trait, and the full
      _installAgentRequestProjection (virtual-model routing:
      lastResponse findLastIndex, userTurn detection,
      getVirtualModelState, reason retry|user|continuation,
      VIRTUAL_MODEL_STATE_ENTRY + entry_appended,
      threshold-compaction re-prepare, routed model/
      thinkingLevel). pi-agent: AgentEvent::SessionEvent
      variant, set_model/set_thinking_level/set_prepare_request,
      PrepareRequestHook → Result (router throw rejects the
      run). ARCHITECTURE: TS's prepareRequest closure captures
      `this`; the Rust hook is 'static while the run driver
      holds &mut self — the hook forwards session accesses
      (TakeFailed, Canonical, AppendVirtualModelState,
      ExceedsThreshold, RunThresholdCompaction) over an mpsc
      bridge serviced by the parked run driver (run_with_bridge
      select-loop) for 1:1 timing; Canonical first pumps the
      agent event queue so the projection includes earlier
      in-run messages and clears turn_end_boundary_continue.
      Parity harness `agent-session-model` (15 cases; suite
      2515/2515): fake modelRuntime (snapshot from args.models,
      scripted resolveModel, configurable checkAuth), settings
      store recording writes. Divergences: abort signal not
      threaded to resolveModel; state-entry events flow
      through the agent event queue — observably identical in
      all cases.
- [ ] Remaining agent-session tranches (phase3-agent-session.md):
      AgentSession slices 15–17 (bash,
      tree, stats) and 18–19 (runtime, services+event-bus),
      extending crates/pi-session.
