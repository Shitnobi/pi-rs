# Parity Testing Methodology

This document describes the differential (TS vs Rust) parity framework in
`parity/`, used to verify that the Rust port behaves identically to the
original TypeScript implementation of the pi coding agent.

## Design

Parity is **differential**: the same input is executed against both
implementations and the outputs (and resulting filesystem state) are compared
for equality. Neither side is treated as a golden snapshot; the original TS
build is the ground truth.

```
            ┌────────────────┐
 cases.mjs ─▶│ run-parity.mjs │
            └───┬────────┬───┘
        fresh   │        │   fresh
        fixture │        │   fixture
        dir ts/ │        │  dir rs/
                ▼        ▼
        ts-harness.js   pi-cli harness tool <name>
        (node, imports   (rust binary)
         built dist)
                │        │
                └───┬────┘
              canonical JSON equality
              + filesystem snapshot equality
```

### Sides

- **TS side** (`parity/ts-harness.js`): a small Node.js script that imports
  the original *built* distribution (`dist/index.js` in
  `packages/coding-agent`) and exposes its tool factories
  (`createReadTool`, `createWriteTool`, `createEditTool`, `createLsTool`,
  `createGrepTool`, `createFindTool`, `createBashTool`). For each request it
  reads `{tool, args}` JSON from stdin, applies the tool's
  `prepareArguments(args)`, then calls `tool.execute('parity-call',
  prepared)` and prints the result JSON on stdout. The dist path can be
  overridden with the `PI_DIST` environment variable.

- **Rust side**: `pi-cli harness tool <name> --cwd <dir>` reads the same JSON
  from stdin and prints the `ToolOutput` JSON on stdout. Any tool error is
  printed as `{"error": "<message>"}` with exit code 1.

### Module-level entries

Tools are the main surface, but some ported functionality lives in plain
modules rather than `AgentTool` factories. Both harnesses expose matching
module-level entries under the same `tool` dispatch, so they get identical
differential coverage:

| `tool` name | Original module (under `dist/`) | Rust module |
|---|---|---|
| `bash-executor` | `core/bash-executor.js` (`executeBashWithOperations`, driven with `createLocalBashOperations` from `core/tools/bash.js`) | `pi_tools::bash_executor` |
| `output-accumulator` | `core/tools/output-accumulator.js` (`OutputAccumulator`: append / finish / snapshot / closeTempFile / readFullOutput / getLastLineBytes) | `pi_tools::output_accumulator` |
| `messages` | `core/messages.js` (`bashExecutionToText`, `convertToLlm`, `create*Message`) | `pi_core::messages` |
| `session-manager` | `core/session-manager.js` (id validation, migrations, parsing, projection, context building, session-dir defaults, file loading, most-recent lookup) | `pi_session::session_manager` |
| `compaction-utils` | `core/compaction/utils.js` (file-op tracking, `computeFileLists`, `formatFileOperations`, `serializeConversation`, `SUMMARIZATION_SYSTEM_PROMPT`) | `pi_session::compaction_utils` |
| `compaction` | `core/compaction/compaction.js` (settings, token estimation, `shouldCompact`, cut points, `getSummarizationFailure`, `prepareCompaction`) | `pi_session::compaction` |
| `nested-calls` | `core/nested-tool-calls.js` (`NESTED_CALL_LIMITS`, `NestedCallRecorder` driven by an op script) | `pi_session::nested_tool_calls` |

For these entries `args` is module-specific JSON — e.g. base64-encoded
`chunks` for the accumulator and `{fn, ...}` for messages — and each entry
mirrors the call sequence the real agent performs.

### Case model (`parity/cases.mjs`)

```js
{
  name: "read/offset-limit",          // unique id, used by --filter
  tool: "read",                        // tool name
  args: { path: "a.txt", offset: 2, limit: 3 },
  setup: { "a.txt": "line1\nline2\n" }, // files written into a fresh fixture dir
  sortLines: true,                      // optional: order-insensitive compare
}
```

Setup values can be a string (UTF-8 file), `{ base64: "..." }`, or
`{ fixture: "name" }` (binary file copied from `parity/fixtures/` — used for
the image cases).

Each case gets a **fresh fixture directory per side** (`parity/out/ts/<case>`
and `parity/out/rs/<case>`). After execution both directories are snapshotted
(file list + contents) and compared, so cases verify *filesystem effects*
(writes, edits, spill files) in addition to result payloads.

### Comparison rules

1. **Canonical JSON**: object keys are sorted recursively before comparison,
   so key order never matters.
2. **Normalization** of values that are legitimately volatile:
   - the absolute fixture directory path → `<DIR>`
   - temp spill-file paths (`/tmp/pi-bash-<16 hex>.log`,
     `/tmp/pi-output-<16 hex>.log`) → `<SPILL>`
   - `wall_time_seconds` → `0`
3. **`sortLines: true`** (per case): sorts the text lines of every content
   block and of bash `structuredContent.output`. Used when output ordering is
   inherently racy:
   - `bash` combined stdout/stderr — both implementations attach one handler
     to each pipe; cross-pipe arrival order is scheduler-dependent.
   - `grep` with matches in multiple files — `rg`'s multi-file output order
     is nondeterministic (verified empirically; the same rg invocation
     returns different file orders across runs).
   - `find` cases are constructed so that `fd --max-results` subsets are
     deterministic by construction (exactly `limit` matching files), rather
     than sorted, because fd's early-stop subset selection is arbitrary.
4. **`normalizeSessionIds: true`** (per case): rewrites randomly generated
   8-hex-char session-entry ids on the linkage keys (`id`, `parentId`,
   `firstKeptEntryId`) to `<ID0>`, `<ID1>`, … in first-appearance order.
   Used for code paths that generate random ids (the v1→v2 migration):
   sequential token substitution preserves the tree shape, so the two sides
   are compared graph-isomorphically. Fixture ids deliberately avoid the
   8-hex shape so they are never rewritten.
5. **Wall-clock values in op scripts**: the `nested-calls` recorder harness
   normalizes each recorded call's `durationMs` to `0` on both sides.

## Running

```bash
# prerequisites: node on PATH; the original repo checked out at /tmp/pi-repo
# (or set PI_DIST=/path/to/pi/packages/coding-agent/dist/index.js)
node parity/run-parity.mjs                    # all cases
node parity/run-parity.mjs --filter read/     # substring filter
node parity/run-parity.mjs --verbose          # show diffs on failure
```

Exit code is non-zero on any failure. Output artifacts live in
`parity/out/` (gitignored).

## Fidelity techniques used during porting

Several components were verified against **runtime ground truth**, not just
by reading source:

- **jsdiff port** (`pi-core/src/jsdiff.rs`): the exact
  `createTwoFilesPatch` output of the real `jsdiff@8.0.4` package was
  generated under Node and used to fix test expectations — this caught
  hunk-header format (`@@ -1,1 +1,1 @@`, not `@@ -1 +1 @@`) and the
  placement of `\ No newline at end of file` markers.
- **Node.js semantics** are ported explicitly, not approximated:
  `String.length` in UTF-16 code units (`js_string.rs`), `trimEnd`,
  `toFixed(1)`, `path.resolve/join/relative/dirname`, `fs` error message
  formats (`node_fs.rs`), and `TextDecoder` streaming decode (BOM strip,
  incomplete multi-byte sequence holdback) in the bash full-output reader.
- **External binaries**: `grep`/`find` shell out to `rg`/`fd` with
  byte-identical argument vectors. The binaries are found on `PATH`, falling
  back to `~/.pi/agent/bin` (where the original auto-downloads them).
- **Bash process management**: children are spawned in their own process
  group (like the original's detached spawn) and killed with
  `kill(-pgid, SIGKILL)` so descendants don't outlive the timeout.
- **`waitForChildProcess` grace semantics**: after the shell exits, the
  100 ms grace timer is re-armed on every stdout/stderr chunk, so a
  detached descendant still writing to the inherited pipes keeps the
  reader alive; the Rust port reproduces this with pump threads feeding
  an event loop over an mpsc channel.
- **`new Date(ts).getTime()` semantics** (`messages.rs`): timestamps are
  parsed with a strict ISO-8601 parser (Howard Hinnant days-from-civil),
  verified differentially against Node for fraction edge cases
  (`.5Z` → 500 ms, `.1234Z` → 123 ms truncation). Pre-epoch dates are
  negative milliseconds; invalid dates become `null`, matching
  `JSON.stringify(NaN)`.
- **Photon wasm under wasmtime** (`pi-photon`): the original's image
  pipeline runs the `@silvia-odwyer/photon-node@0.3.4` wasm module under
  V8. Rather than reimplementing (or natively recompiling) photon, the Rust
  port executes the **same wasm binary** under wasmtime with a hand-bound
  wasm-bindgen ABI, so float rounding, PNG/JPEG encoders, and resize
  behavior are identical *by construction*. Ground truth for
  `pi-photon/tests/parity.rs` was generated by running the real
  photon-node under Node (decode, resize, PNG/JPEG encodes at five
  qualities across five input formats) and the wasmtime execution is
  asserted byte-identical. Notable ABI subtlety: wasm-bindgen **transfers
  ownership** of constructor input buffers to the wasm side, so the host
  must never free them (freeing corrupts the wasm heap).

## Current coverage

103 cases across `read` (22, incl. 9 image cases), `write` (5), `edit` (16),
`ls` (5), `grep` (10), `find` (5), `bash` (10), `bash-executor` (8),
`output-accumulator` (9), `messages` (13). See `parity/cases.mjs` for
the full list. Adding a case is a single entry in `cases.mjs`.

The Rust side runs `target/release/pi-cli` when present (image cases with
multi-MB payloads are impractically slow in a debug wasmtime build);
override the binary with the `PI_CLI` environment variable.

## Known limitations

- Parity compares tool behavior only. Agent-loop, provider, and extension
  layers are not covered yet (they don't exist in the port yet).
- The image fixtures exercise the main pipeline paths (resize, convert,
  EXIF, corruption, animated-PNG fallthrough), but the full combinatorics
  of formats/dimensions are covered by `pi-photon`'s byte-parity tests
  rather than differential cases.
