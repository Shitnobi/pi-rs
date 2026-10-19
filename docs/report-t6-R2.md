# T6 Redteam R2 — Latex renderer port

Date: 2026-10-18. Reviewer: coordinator (recovered from subagent 20261019_2,
which hit its turn limit before writing this report; findings reconstructed
from its scratch batteries under /tmp/pi-parity/redteam/ and re-verified).

## Methodology

Adversarial differential review of the latex renderer
(crates/pi-tui/src/latex.rs + the visible_width path in utils.rs) against the
TS original (packages/tui/src/latex.ts + its width utils). Both engines
driven through the parity harness boundary:

- TS: `node parity/ts-harness.js` with `{"tool":"tui-latex","args":
  {"fn":"render","source":...,"display":...}}` on stdin
- Rust: `pi-cli harness tool tui-latex --cwd <dir>` with `{"fn":...}`

Batteries:

1. **Static table sweep** (`r2-sweep.mjs`, 936 cases): extracted every static
   table from both sources (SYMBOLS, NEGATED_SYMBOLS, ACCENTS, BLACKBOARD,
   SUPERSCRIPTS, SUBSCRIPTS, NAMED_OPERATORS, LIMIT_OPERATORS,
   DISPLAY_LIMIT_SYMBOLS, RELATION_COMMANDS, SPACING_COMMANDS,
   NEGATIVE_SPACING_COMMANDS, FONT_SWITCH_COMMANDS, IGNORED_COMMANDS,
   SIZE_COMMANDS, PLAIN_WRAPPERS), diffed them, and rendered every entry in
   both plain and display modes with surrounding context (`a \cmd b`).
2. **Structural battery** (`r2-battery2.mjs`, 149 cases): nesting, matrices,
   cases environments, fraction/script stacking, delimiter sizing, roots
   with degrees, named-operator spacing, text mode, pmod.
3. **Deterministic fuzz** (`r2-fuzz.mjs`, 600 cases): seeded-PRNG
   compositions of commands, symbols, scripts, braces, and raw text.
4. **Astral repros** (`r2-repro-astral.mjs` → superseded by
   `astral-cases.mjs`, 32 cases).

## Findings

### 1. CONFIRMED DIVERGENCE — astral-plane characters split into UTF-16 surrogate halves

All fuzz mismatches shared one root cause: the TS latex lexer is **UTF-16
code-unit indexed** (`this.source[this.position]`, `.slice(position)`,
non-`u` regexes). When an astral-plane character (U+10000..U+10FFFF — emoji,
CJK ext-B, math alphanumerics) appears in a single-unit argument position
(e.g. the unbraced argument of `^`, `_`, `\sqrt`, `\hat`, `\vec`,
`\overline`), TS consumes only the **high surrogate**; the **low surrogate**
is left in the stream and renders as a separate character. TS render output
then contains lone surrogates (e.g. `x^😀` display → `" \ud83d\nx\ude00"`).

Rust `char` cannot hold a lone surrogate, so the faithful port required an
encoding scheme. 14 of 32 official astral repro cases mismatched pre-fix
(scripts, sqrt variants, matrix/cases column-width padding,
format_root/format_fraction regex boundaries, double scripts).

**Fix** (crates/pi-tui/src/latex.rs, utils.rs, pi-cli/src/main.rs):

- **Sentinel-pair encoding**: high surrogates U+D800..U+DBFF →
  U+F1000..U+F13FF, low surrogates U+DC00..U+DFFF → U+F1400..U+F17FF
  (plane-15 PUA, deliberately above TS latex's own private markers
  U+F0000..U+F0005: LAYOUT_MARKER_START/END, PROTECTED_SPACE,
  NAMED_OPERATOR_START/END).
- `js_units(s)` applied at **every parser entry** (`render_latex` +
  `render_nested` — TS re-parses nested strings by unit); astral chars
  become sentinel pairs; existing sentinel chars pass through without
  re-expansion (a double-encode bug found and fixed during validation).
- Three boundary treatments where TS semantics differ from unit indexing:
  1. **Code-point semantics** (JS `Array.from`/spread sites:
     replace_characters, format_script, format_fraction, parse_scripts,
     parse_command not/mathbb/accents; and `u`-flag regexes
     `[\p{L}\p{N}...]+` in normalize_output/format_root/format_fraction and
     NAMED_OPERATOR spacing lookarounds): pairs coalesce via
     `js_code_points` / `with_coalesced_pairs`.
  2. **Width path** (visible_width in utils.rs): pairs coalesce after ANSI
     stripping (Intl.Segmenter grapheme joining — emoji pairs render width
     2, lone sentinels width 0, matching TS `\p{Surrogate}` in
     zeroWidthRegex/leadingNonPrintingRegex/nonPrintingCharRegex).
  3. **Harness output boundary** (main.rs): `decode_surrogate_sentinels`
     coalesces pairs back to the real character and renders lone sentinels
     as `\uXXXX` text — exactly what `JSON.stringify` produces for a lone
     surrogate, so parity JSON compares equal to TS.

**Known accepted limitation** (documented in latex.rs): input containing
literal U+F1000..U+F17FF plane-15 PUA characters would be misdecoded as
surrogate halves. Real pi usage never produces those code points, and TS
latex's own markers stop at U+F0005.

### 2. DISMISSED — staticReport SPACING_COMMANDS discrepancy

The static sweep reported `onlyTs:[""]`, `onlyRs:[","]` for
SPACING_COMMANDS. Manual inspection shows both lists are identical:
TS latex.ts:510-523 `(",", ":", ";", " ", ">",
"enspace", "enskip", "medspace", "quad", "qquad", "thickspace", "thinspace")` == Rust SPACING_COMMANDS
(latex.rs:745-748). The report entry is an **extraction artifact**: the
sweep's `tsSet()` splits the TS array region on commas, and the array's
first element is the string literal `","` — the split mangles it into a
spurious empty-string key and destroys the real `,` member. The behavioral
sweep (r2-sweep line 147) iterated the mangled set and therefore never
tested `a \, b`; a dedicated spacing probe (`spacing-probe.mjs`, all 12
SPACING + 4 NEGATIVE_SPACING commands × {inline, display} = 32 cases)
confirms **32/32 MATCH**. Dismissed.

## Differential coverage (post-fix)

| Battery | Result |
|---|---|
| astral-cases.mjs (official repro battery, 32 cases) | 32/32 PASS (was 14 mismatches pre-fix) |
| r2-sweep.mjs (936 cases) | 936/936 PASS |
| r2-battery2.mjs (149 cases) | 149/149 PASS |
| r2-fuzz.mjs (600 deterministic fuzz cases) | 600/600 PASS |
| spacing-probe.mjs (32 cases) | 32/32 PASS |
| tui-latex parity filter baseline | 125/125 PASS |
| latex parity suite (now 158 cases, +34 astral regression) | PASS |
| astral through markdown-render latex-block/inline | PASS (covered by R1 probe + 12 render cases) |

## Recovery notes (subagent failure)

Subagent 20261019_2 exhausted its turn budget after building the three
batteries and the fuzz root-cause analysis, but before writing its report.
Its scratch artifacts (r2-sweep.mjs, r2-battery2.mjs, r2-fuzz.mjs,
r2-results.json) were re-run directly: sweep/battery2/fuzz all reported 0
mismatches, but the fuzz battery's 7 original mismatches (pre-fix) had been
observed against the pre-sentinel binary. The root cause was confirmed by
re-running the 32-case astral battery against a rebuilt binary after the
sentinel fix: 32/32. Batteries preserved under scripts/redteam/ for re-runs
(probe-latex-astral, redteam-latex-sweep, redteam-latex-battery2,
redteam-latex-fuzz, probe-spacing-cmds — all re-verified from a fresh
checkout of this commit).

## Verdict

One confirmed divergence class (astral/surrogate handling), root-caused,
fixed with sentinel-pair encoding, and locked in with 34 latex parity
regression cases + the 32-case astral battery + 33/12 markdown
lex/render astral cases exercising latex through markdown. All static
tables verified byte-equal (the single report entry was an audit-script
artifact, dismissed with a dedicated probe).
