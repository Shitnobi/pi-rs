# Migrating this project to a new machine

Everything needed lives in this repo **plus** a clone of the original pi
TypeScript repo. Steps below make a fresh machine fully operational.

## 1. Toolchain

- **Rust** (developed/tested on 1.97.x)
- **Node.js 24** — required by the parity TS harness: it imports the original
  repo's TUI TypeScript source directly via Node 24 type-stripping
  (`PI_TUI_SRC`).

## 2. Clone this repo

```bash
git clone https://github.com/Shitnobi/pi-rs.git
cd pi-rs && cargo build && cargo test
```

## 3. Clone the original pi repo

The parity harness defaults expect the original repo at `/tmp/pi-repo`
(override with `PI_DIST` / `PI_TUI_SRC` / `PI_CLI` env vars if you put it
elsewhere):

```bash
git clone https://github.com/earendil-works/pi /tmp/pi-repo
cd /tmp/pi-repo
git checkout 698a2d2   # commit the parity suite was developed against
```

## 4. Apply the lexMarkdown test-seam patch

`parity/ts-repo/lexmarkdown-seam.patch` adds a `lexMarkdown()` export to
`packages/tui/src/components/markdown.ts` in the original repo. It is a
**test seam only** (unused by the renderer) that the Rust port's
marked-lexer differential tests call. It is uncommitted local state on the
original clone — this patch file is the only durable copy.

```bash
cd /tmp/pi-repo
git apply /path/to/pi-rs/parity/ts-repo/lexmarkdown-seam.patch
```

## 5. Build the original repo

The TS harness runs the **built dist** (`PI_DIST` defaults to
`/tmp/pi-repo/packages/coding-agent/dist/index.js`), which is not in git:

```bash
cd /tmp/pi-repo
npm install
npm run build          # or the packages/coding-agent build script
```

## 6. Run the parity suite

```bash
cd /path/to/pi-rs/parity
node run-parity.mjs
```

## Notes

- `~/.pi/agent/bin` (fd, rg) is auto-downloaded by the agent — no action needed.
- This repo's push remote is the `Shitnobi/pi-rs` fork; `Cargo.toml`'s
  `repository` field points at `earendil-works/pi-rs`.
