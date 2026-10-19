# Implementation Notes: MCP Pure-Logic Tranche (Phase 3)

Port of the pure, non-I/O MCP surface from the TS original
(`/tmp/pi-repo/packages/coding-agent/src`):

- `core/mcp-servers.ts` → `crates/pi-core/src/mcp_servers.rs`
- `extensions/mcp/tools.ts` (pure parts) → `crates/pi-core/src/mcp_tools.rs`
- `extensions/tool-search/tool.ts` → `crates/pi-core/src/tool_search.rs`

Status: all pi-core tests green (117), clippy clean on the new modules,
parity green (21 new `mcp-*` cases + 5 `tool-search` cases + extended
`math/basic` log vectors, full suite re-verified).

## 1. The fdlibm `Math.log` finding (`js_math.rs`)

BM25 idf in `tool_search.rs` computes `ln((N - n + 0.5) / (n + 0.5)) + 1`.
The TS implementation calls `Math.log`; Rust used `f64::ln` (system libm).
For ~0.7% of inputs the two disagree by exactly 1 ulp, and the resulting
BM25 scores flow into model-visible JSON, so parity broke:

```
bm25_scores_match_ts: left: 1.3988109860808993 right: 1.3988109860808995
```

### Root cause

V8 does **not** use the system libm for `Math.log`; it uses its own port of
fdlibm's `__ieee754_log` (from `e_log.c`). Neither side is always correctly
rounded:

- `Math.log(1.6)` = `0x3fde148a1a2726ce` — JS is correctly rounded here
- `Math.log(2.6666666666666665)` = `0x3fef62f40794a7ba` — here Rust's libm
  is correctly rounded and V8 is off by 1 ulp

That asymmetry proves V8 has its own implementation. The fix follows the
existing precedent in `pi-tui/src/fdlibm.rs` (atan2/sin/cos/cbrt/hypot):
port fdlibm `e_log.c` line by line as `pi_core::js_math::js_math_log`.

Port details: constants `LN2_HI`, `LN2_LO`, `TWO54`, `LG1..LG7`; `hx/lx`
bit manipulation to split `x` into halves; subnormal scale-up via `TWO54`;
near-1 path for `|f| < 2^-20`; the `s = f / (2 + f)` polynomial with the
`i/j` rounding-direction selection.

### Validation methodology

1. Generated 20,022 input vectors from node (random bit patterns across the
   full f64 range + edge cases), recording input and `Math.log` output as hex
   bits (`/tmp/logtest/gen.js`).
2. Batch-compared against the Rust port (`main2.rs`): **0/20022 mismatches**.
3. Special cases verified separately on both runtimes: `log(±0) = -∞`,
   `log(negative) = NaN`, `log(+∞) = +∞`, `log(NaN) = NaN`, subnormal
   scale-up path (`5e-324`, `2^-1023 * 1.5`).
4. To decide *which side is right* on a disagreement (needed to confirm the
   divergence was real and not a port bug): BigInt fixed-point `lnRatio(p,q)`
   reference computation.

The parity `math/basic` case now carries 10 log ops with known
V8-vs-libm divergence vectors (e.g. `1.6`, `1+(3-1+0.5)/(1+0.5)`,
`1.0000000000000002`, `0.9999999999999999`, subnormals) so any future
regression to libm `ln` is caught immediately.

## 2. JS semantics that differ from intuition (verified against dist)

Test failures here were **wrong test expectations**, not implementation bugs
— each was verified against the TS dist with
`node -e '...' 2>/dev/null` before touching either side:

- **Stemmer**: `stem('buses') = 'buse'`. Only `ies` and
  `ches|shes|sses|xes|zes` endings drop 2 characters; a plain trailing `s`
  drops exactly 1. So `tokenize("buses uses classes")` →
  `["buse", "use", "class"]`.
- **Tool search documents**: `createToolSearchDocument` includes the tool
  `name` **twice** in the text (raw + with `_` replaced by spaces). A
  minimal tool therefore yields text `"t t"`, plus `"t t d ns"` with
  description/schema fragments.
- **CONTENT_BLOCK_SCHEMA**: the block `type` property is a plain
  `{"enum": ["text","image","audio","resource_link","resource"]}`; the
  surrounding `type` keys are plain string literals (`"object"`, `"array"`)
  — **not** typebox-wrapped `{"type": [...]}`. My initial port over-shaped
  this; the fix removed the helper entirely.
- **resource_link → model content**: `blockToContent` returns exactly ONE
  text block — `[Resource ${uri} "${title ?? name}"${details}${description}${read}]`
  — and never falls through to `toLlmContent`.

## 3. Protocol-violation error parity (`convert_mcp_result`)

TS `convertMcpResult` dereferences `result.content` without guards, so
protocol-violating inputs **throw** with V8 error messages. Faithful parity
required reproducing those exact strings. Verified against the dist:

| `result.content` | TS throws | Rust |
|---|---|---|
| key absent | `Cannot read properties of undefined (reading 'length')` | `Err(...)` |
| `null` | `Cannot read properties of null (reading 'length')` | `Err(...)` |
| non-empty string | `blocks.map is not a function` | `Err(...)` |
| `""` / number / etc. | `(result.content ?? []).map is not a function` | `Err(...)` |

(`""` hits the `??`-default's `.map`, hence the different message; a
non-empty string passes the truthiness check on `content` and dies later in
`blockToContent`.) The Rust signature is therefore
`Result<AgentToolResult<McpToolDetails>, String>` with these guards
up front; covered by the `convert_mcp_result_protocol_violations` unit test
and the `mcp-tools/convert-result-error-*` parity cases.

## 4. WHATWG URL subset deviations (`mcp_servers.rs`)

`validateMcpServerConfig` and `isLoopbackRedirectUri` need URL parsing. The
Rust side uses a hand-rolled subset of WHATWG URL semantics instead of the
`url` crate; the deviations and rationale are documented in the module docs
of `mcp_servers.rs` (module-level `//!` header). The parity harness keeps
the TS side honest by using a **real** `new URL()` in `ts-harness.js` for
`parseUrl`, and the `mcp-servers/parse-url` case exercises the shapes the
validator depends on. Full WHATWG compliance is out of scope until the
networked MCP transport tranche lands (plan: `phase3-mcp-extensions.md`).

## 5. Parity harness design for this tranche

- New module entries in `parity/ts-harness.js`: `mcp-servers`
  (`dist/core/mcp-servers.js`), `mcp-tools` (`dist/extensions/mcp/tools.js`),
  `tool-search` (`dist/extensions/tool-search/tool.js`).
- Rust side: three new harness functions in `pi-cli/src/main.rs`
  (`run_mcp_servers_harness`, `run_mcp_tools_harness`,
  `run_tool_search_harness`) dispatched by module name.
- `toModelContent` takes an output saver; both sides use a deterministic
  fake — `async (_data, extension) => "/tmp/pi-mcp-out" + extension` — so
  saved-artifact paths compare equal without touching the filesystem.
- `convertMcpResult` in TS is sync-returning but can throw; the harness
  case shape carries both result and thrown-message so the Rust `Err`
  side compares against the caught V8 message.

**Operational note**: `parity/run-parity.mjs` prefers
`target/release/pi-cli`, so any Rust change requires
`cargo build --release` before re-running parity — a stale release binary
manifests as `unknown tool: <module>` and phantom failures.

## 6. Red-team / regression notes

- 5 initial test failures decomposed as: 3 wrong test expectations (§2),
  1 implementation bug (schema shape, §2), 1 genuine libm divergence (§1).
  The triage discipline — verify the expectation against the TS dist with
  node before changing either side — caught all three expectation errors
  and prevented "fixing" correct Rust code.
- Clippy: the new modules are clean (`cargo clippy -p pi-core --all-targets`
  emits zero warnings). Pre-existing `js_index_of`/`js_trim_start` helpers
  flagged in an earlier survey turned out to live in and be used by
  `pi-tui`, not `pi-core`.
