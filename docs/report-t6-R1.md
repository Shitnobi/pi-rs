# T6 Redteam R1 — Markdown / marked port

Date: 2026-10-18. Reviewer: coordinator (recovered from subagent 20261019_1,
which hit its turn limit before writing this report; findings reconstructed
from its scratch batteries under /tmp/pi-parity/redteam/ and re-verified).

## Methodology

Adversarial differential review of the markdown port (tui-marked lexer +
renderer in crates/pi-tui/src) against the TS original
(packages/tui/src/markdown, built on `marked`). Batteries drove both engines
through the parity harness boundary:

- TS: `node parity/ts-harness.js` with `{"tool":"tui-markdown-lex" |
  "tui-markdown-render", "args":{...}}` on stdin
- Rust: `pi-cli harness tool <tool> --cwd <dir>` with the same args

1. **Batch battery** (`batch.mjs`, 38 cases): adversarial inputs across every
   lexer state — mixed emphasis, nested fences, autolink edge forms, HTML
   attributes, table alignment rows, reference links, task list markers,
   hard breaks, entity refs, CJK-mixed emphasis.
2. **Directed astral probe** (`md-astral-probe.mjs`, 59 cases): astral-plane
   characters (emoji U+1F600, math alphanumerics U+1D400, CJK ext-B
   U+20000, ZWJ clusters, flag tag-sequences, skin-tone modifiers, keycaps)
   threaded through every block/inline structure: headings, setext,
   lheading, emphasis, codespans, link text/URL, autolinks, HTML tags and
   attributes, HTML blocks, table cells/heads, list items, task markers,
   blockquotes, fences (lang + body), escapes, reflinks, strikethrough,
   hardbreaks, plus latex-block / latex-inline passthrough into render.
3. **Baseline re-run**: full markdown parity filter.

## Findings

### 1. Astral / UTF-16 divergence — NOT PRESENT in the markdown port

The R2 latex review established that TS code indexing strings by UTF-16 code
unit (`source[i]`) splits astral characters into surrogate halves (see
docs/report-t6-R2.md). The directed 59-case astral probe was designed to
expose the same class of bug in the markdown port: **all 59 cases MATCH**.

Root cause analysis of why marked is immune: the ported lexer rules are
class-based (`\p{L}`, `\p{N}`, `[^...]` character classes) or literal-string
based; they never measure or slice a rule match at single-code-unit
granularity in a way that can split a surrogate pair. Both engines therefore
tokenize identical astral spans, and byte-identical output follows. No fix
required.

### 2. Batch battery — 0 mismatches

38/38 cases pass once the battery's Rust invocation is corrected (see
"Recovery notes"). No markdown divergence found.

### 3. Static list parity

Markdown-relevant static data (delimiter sets, entity handling, whitespace
classes) was previously reconciled in the T6 marked regex/trim fix (commit
6291d39: JS `\s`/`trim` semantics via JS_WS_SET/JS_DOT_SET +
compile-time translate_js_pattern). R1 found no further static drift.

## Differential coverage

- batch.mjs: 38/38 PASS
- md-astral-probe.mjs: 59/59 PASS
- markdown parity filter (baseline-md.log): 268/268 PASS
- markdown-lex parity suite now 187 cases (33 astral regression cases added)
- markdown-render parity suite now 127 cases (12 astral regression cases
  added — includes astral through latex-block/latex-inline)

## Recovery notes (subagent failure)

Subagent 20261019_1 exhausted its turn budget after identifying the astral
root cause (in the latex engine it was also probing) but before writing its
report. Its scratch battery batch.mjs had a Rust-harness invocation bug
(`["harness", tool, ...]` missing the `tool` subcommand keyword → every RS
response was `{"error":"missing tui-markdown-render name"}`). Fixed to
`["harness", "tool", tool, "--cwd", WORKDIR]`; after the fix the battery
passes 38/38, confirming the earlier "failures" were harness wiring, not
port divergences. The battery and probe are preserved in
scripts/redteam/probe-md-astral.mjs and
scripts/redteam/redteam-md-batch.mjs for re-runs.

## Verdict

No confirmed divergences in the markdown/marked port. Regression coverage
extended by 45 parity cases (33 lex + 12 render) locking in astral-plane
behavior on both engines.
