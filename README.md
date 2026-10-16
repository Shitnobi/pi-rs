# pi-rs — Rust port of the pi coding agent

A faithful Rust reimplementation of the [pi coding agent](https://github.com/earendil-works/pi)
(`packages/coding-agent`), focused on **behavioral parity** with the original
TypeScript implementation.

## Status

**Phase 1 complete: built-in tools.** All seven built-in file/shell tools
(`read`, `write`, `edit`, `ls`, `grep`, `find`, `bash`) are ported and pass a
differential parity suite against the original TS implementation.

**Phase 2 complete: image processing.** `read` on images runs the exact
photon wasm binary under wasmtime, producing **byte-identical** resized /
converted output to the original's V8 execution (see [PARITY.md](PARITY.md)).

**Phase 3 in progress: core runtime.** The process-runtime layer (shell
config/env, process groups, `waitForChildProcess` grace semantics), streaming
bash execution (`bash-executor`), `OutputAccumulator`, the custom message
model (`messages.ts`), session persistence (`session-manager.ts` in the new
`pi-session` crate: JSONL session trees, projection, migrations, forking),
compaction pure logic (token estimation, cut points, `prepareCompaction`),
and nested-call recording are ported with differential parity coverage; the
bash tool was rebuilt on these layers with behavior verified unchanged.

The parity suite is now 239 cases, all passing.

Upstream layers (agent loop, providers, extensions, TUI) are not yet ported —
see [TRACKING.md](TRACKING.md) for the roadmap and known gaps.

## Layout

```
crates/
  pi-core/   Pure logic: jsdiff port, edit-diff/truncation, Unicode
             normalization, JS string semantics, path handling, streaming
             UTF-8 decoding, ANSI handling, the custom message model
  pi-tools/  The built-in tools (incl. the image-processing pipeline),
             Node.js fs/path semantics shims, and the process-runtime layer
             (shell config/env, process groups, bash executor,
             output accumulator)
  pi-photon/ Executes the exact @silvia-odwyer/photon-node wasm binary
             under wasmtime (byte-identical image resize/convert)
  pi-session/ Session persistence (JSONL session trees, projection,
             migrations, forking/branching) and the compaction/nested-call
             pure logic
  pi-cli/    CLI entry point (`harness tool|pure` subcommands used by parity)
parity/      Differential test harness (Node.js) driving both implementations,
             with binary image fixtures in parity/fixtures/
```

## Building

```bash
cargo build
cargo test
```

## Parity testing

The parity harness runs the same inputs through the original built TS
distribution and the Rust implementation and compares results and filesystem
effects:

```bash
# requires node on PATH and the original repo at /tmp/pi-repo
# (override with PI_DIST=<path to pi coding-agent dist/index.js>)
node parity/run-parity.mjs [--filter <substring>] [--verbose]
```

See [PARITY.md](PARITY.md) for methodology, case model, and normalization
rules.

## Harness CLI

`pi-cli` currently exposes a machine interface used by the parity harness:

```bash
echo '{"path":"a.txt"}' | pi-cli harness tool read --cwd /some/dir
# prints the ToolOutput JSON on stdout, or {"error": "..."} with exit 1
```

Tools: `read`, `write`, `edit`, `ls`, `grep`, `find`, `bash`.

## License

MIT
