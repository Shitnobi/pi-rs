# pi-rs — Rust port of the pi coding agent

A faithful Rust reimplementation of the [pi coding agent](https://github.com/earendil-works/pi)
(`packages/coding-agent`), focused on **behavioral parity** with the original
TypeScript implementation.

## Status

**Phase 1 complete: built-in tools.** All seven built-in file/shell tools
(`read`, `write`, `edit`, `ls`, `grep`, `find`, `bash`) are ported and pass a
64-case differential parity suite against the original TS implementation
(see [PARITY.md](PARITY.md)).

Upstream layers (agent loop, providers, extensions, TUI) are not yet ported —
see [TRACKING.md](TRACKING.md) for the roadmap and known gaps.

## Layout

```
crates/
  pi-core/   Pure logic: jsdiff port, edit-diff/truncation, Unicode
             normalization, JS string semantics, path handling
  pi-tools/  The built-in tools, plus Node.js fs/path semantics shims
  pi-cli/    CLI entry point (`harness tool|pure` subcommands used by parity)
parity/      Differential test harness (Node.js) driving both implementations
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
