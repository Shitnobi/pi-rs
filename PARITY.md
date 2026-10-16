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

Each case gets a **fresh fixture directory per side** (`parity/out/ts/<case>`
and `parity/out/rs/<case>`). After execution both directories are snapshotted
(file list + contents) and compared, so cases verify *filesystem effects*
(writes, edits, spill files) in addition to result payloads.

### Comparison rules

1. **Canonical JSON**: object keys are sorted recursively before comparison,
   so key order never matters.
2. **Normalization** of values that are legitimately volatile:
   - the absolute fixture directory path → `<DIR>`
   - bash spill-file paths (`/tmp/pi-bash-<16 hex>.log`) → `<SPILL>`
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

## Current coverage

64 cases across `read` (13), `write` (5), `edit` (16), `ls` (5),
`grep` (10), `find` (5), `bash` (10). See `parity/cases.mjs` for the full
list. Adding a case is a single entry in `cases.mjs`.

## Known limitations

- Image reads: the original resizes/converts images via a WASM module
  (photon) before returning base64. The Rust port returns the raw image with
  the identical result shape; pixel-processing parity is deferred to
  phase 2 (see TRACKING.md).
- Parity compares tool behavior only. Agent-loop, provider, and extension
  layers are not covered yet (they don't exist in the port yet).
