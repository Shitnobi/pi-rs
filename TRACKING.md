# pi-rs Port Tracking

Progress tracker for the Rust port of
[pi's coding-agent package](https://github.com/earendil-works/pi/tree/main/packages/coding-agent).
See [PARITY.md](PARITY.md) for the parity-test methodology.

## Phase 1 — Built-in tools ✅ (complete)

All seven built-in tools from `src/core/tools/` are ported and pass the
64-case differential parity suite (result payload + filesystem effects):

| Tool     | Rust module             | Notes |
|----------|-------------------------|-------|
| `read`   | `pi-tools/src/read.rs`  | text + image paths (see gap G1) |
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

## Phase 2 — Image processing (pending)

- G1: `read` on image files. The original resizes/converts via the photon
  WASM module (`image-process.ts`) before base64-encoding. The port
  currently returns the **raw** image bytes with the identical result shape
  (text note + image content block + `structuredContent`). Plan: use an
  image crate (e.g. `image`) to replicate photon's resize/convert pipeline
  and re-verify with image parity cases.

## Phase 3 — Core runtime (not started)

Upstream `src/core/` modules not yet ported, roughly in dependency order:

- [ ] `bash-executor.ts` / `output-accumulator.ts` (streaming bash session)
- [ ] `exec.ts`, `messages.ts` (message/event model)
- [ ] `mcp-servers.ts`, `extensions/` (extension + MCP loading)
- [ ] `agent-session*.ts` (agent loop, tool-call orchestration)
- [ ] `provider-*`, `model-*`, `http-dispatcher.ts` (LLM providers)
- [ ] `compaction/`, `nested-tool-calls.ts`, `file-mutation-queue.ts`
- [ ] CLI/TUI layer

## Gaps register

| ID | Area | Description | Phase |
|----|------|-------------|-------|
| G1 | read/images | photon WASM resize/convert not ported; raw bytes returned, same result shape | 2 |
| G2 | parity | racy output orderings (bash stdout/stderr interleave, rg multi-file order) are normalized by `sortLines` rather than matched | n/a (inherent) |
| G3 | parity | spill file contents not compared byte-for-byte (paths normalized); mode (0600) and name format verified | 1 |
| G4 | packaging | `powershell.ts` (Windows shell support) intentionally not ported; Linux/macOS only for now | 3+ |

## Parity suite status

- 64/64 cases pass (stable across repeated runs)
- 46/46 Rust unit tests pass
- `cargo clippy --all-targets` clean

## Changelog

- Phase 1 complete: all built-in tools ported, parity suite at 64 cases,
  flaky cases (fd `--max-results` subsets, rg multi-file ordering, bash
  pipe interleave) made deterministic/normalized.
