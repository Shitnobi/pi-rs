p='docs/plans/phase4-tranche6-extension-runner-rest.md'
s=open(p).read()
s=s.replace('- [ ] 1. `exec.rs` (exec.ts + waitForChildProcess subset) + unit tests',
'''- [x] 1. `exec.rs` (exec.ts + waitForChildProcess subset) + unit tests
      **DONE (396a92a)**: probe-pinned extras — signal-kill →
      `{code: 0, killed: true}` (exitCode ?? 0); ENOENT-**and-bad-cwd**
      → RESOLVED `{code: 1, killed, empty-stderr}`; non-string command /
      non-array args → REJECT (exact node messages); **args elements
      string-coerced** ([5] → "5", [null] → "null"); no shell; timeout >
      0 (full JS ToNumber); pre-aborted + ENOENT+pre-aborted =
      killed:true; 29 tests + clippy clean''')
s=s.replace('- [ ] 12. `system_prompt.rs`: `normalize_options_raw` (full JS semantics) +',
'''- [x] 12. `system_prompt.rs`: `normalize_options_raw` (full JS semantics) +''')
i=s.rfind('## Addendum D')
if i==-1: i=s.rfind('Addenda A–D supersede')
j=s.rfind('\n')
s+='''

## Addendum E — sp1–sp8 errata + item-12 completion notes (c0973a7)

**The Addendum A sections were corrected by 8 further probe rounds**
(sp1–sp8, run against the compiled dist live); where Addendum A/E-less
text conflicts with the following, THIS addendum wins:

### normalizeBuildSystemPromptOptions (sp1/sp2 — corrected)

- **`customPrompt` / `forceSystemPrompt` / `cwd` = RAW `input.x`
  reads**: present (ANY value incl. null) → kept raw; missing
  (undefined) → key ABSENT (jstr drops it). `n(5)` = 9 jstr-keys
  (Object.keys sees 12 — undefined-valued; unobservable = documented
  divergence). The Addendum A "cwd: 5 → String(5)" bullet was WRONG.
- `selectedTools`/`hiddenTools`/`promptGuidelines` = `[...(x ?? def)]`
  — non-iterable non-nullish throws the FULL compiled expression text
  (`(input.selectedTools ?? ["read", "bash", "edit", "write"]) is not
  iterable`, NO Symbol-suffix); `"ab"` → `["a","b"]`; elements raw;
  null → default/`[]`.
- `toolSnippets`/`sections` = `{...(x ?? {})}` — **never throws**;
  primitives → `{}`; **`"ab"` → `{"0":"a","1":"b"}` (strings spread to
  char-keys); `["a"]` → `{"0":"a"}` (arrays to index-keys)**.
- `toolGuidelines` = `fromEntries(entries(?? {}).map(([n, g]) => [n,
  ...g]))` — primitive → `{}`; per-VALUE spread: non-iterable g (5,
  null, `{a:1}`, undefined) → **`guidelines is not iterable`** (V8
  names the variable); `{x: "ab"}` → `{x: ["a","b"]}`.
- `appendSystemPrompt` = `x ?? ""` (0/false/5 kept; null → "").
- `contextFiles`/`skills` = `(x ?? []).map(e => ({...e}))` — non-array
  non-nullish → `(input.x ?? []).map is not a function`; elements
  spread (primitives → `{}`, strings → char-keys).
- **Rust pitfall confirmed**: `Value::Null.get()` ≠ throw (matched
  explicitly); serde has no undefined (missing key ≠ present null).

### buildSystemPromptSections (sp4/sp5/sp7 — corrected)

- **cp-gate = the default cluster ONLY**
  (preamble+tools+rules+docs); **addendum / project_context / skills /
  cwd / custom sections all still run** (B4/B5/B6/B7/B8 verbatim).
- `customPrompt` **truthy** → `promptSections.preamble = customPrompt`
  **RAW** (non-strings survive into the map — the gsmt len>0 filter
  drops them); falsy (incl. null/0/"") → defaults.
- `toolSnippets[String(name)]` truthy w/ **raw templates**:
  `{read: 5}` → `- read: 5`; **`{read: []}` → `- read: ` (empty-array
  TRUTHY!)**; 0/""/null → `(none)`; non-declared not listed.
- `appendSystemPrompt` truthy-raw (5 → `<addendum>\n5\n</addendum>`).
- `renderProjectContext` = **raw `${path}`/`${content}`**: missing →
  **"undefined"**, null → **"null"**, 5 → "5", 0 → "0" (sp8 A–F).
- skills: `escapeXml` throws — non-string name/desc/filePath →
  `str.replace is not a function`; **missing filePath → `'replace'`
  of undefined**; null → of-null. dmi:true → no section; no reader
  (`selectedTools: ["x"]`) → no section; **`skills: []` → no
  section**.
- `cwd.replace` throws: 5 → `cwd.replace is not a function`;
  present-null → of-null; **MISSING → `'replace'` of undefined**
  (`bp(n({}))` / `st(msgs)`).
- custom sections **last**, `if (content)` truthy-kept **raw**
  (5 → `<a>\n5\n</a>`; null/0/"" skipped), **overwrite** built.
- `sections = {preamble: raw}` (the ONLY non-templated map entry) +
  others `<name>\n${content}\n</name>` (template-coerced).
- **rules throw**: `rule.trim` (non-string; null → of-'trim');
  guidelines looked up by `String(name)`; pg/tg rules dedup (trim +
  seen); **hidden bash → no bash rule** (declared = selected − hidden).

### buildSystemPromptState / buildSystemPrompt / gsmt (sp3/sp4)

- **`buildSystemPromptState(input)` = 1-arg (the OPTIONS!)** — the
  Addendum-A "st(msgs)" reading was wrong: `input` null →
  `null.forceSystemPrompt` throw; **`forceSystemPrompt !== undefined`
  = KEY-PRESENT** (null/0/false/"" all force) → `{content: fsp}`
  (sections ABSENT); else `{content: "", sections:
  buildSections(input)}`.
- `buildSystemPrompt(input)` = `getSystemMessageText({role: "system",
  ...state, timestamp: 0})` — `getSystemMessageText`/`contentText`
  live in **pi-ai utils/text.js** (NOT system-prompt.ts): gsmt =
  `[ct(content)] + Object.values(sections ?? {})` (null values
  filtered BEFORE push; **raw Values pushed**) → `filter(p => p.length
  > 0)` (**non-strings DROPPED**; empty strings dropped) → `"\n\n"`.
  join. ct = string | array `.filter(b => b.type === "text").map(b =>
  b.text).join(sep)` (**`b.type` on null throws of-'type'**;
  non-objects drop; b.text missing = undefined → join-coerced "") |
  null → `null.filter` throw | other → `content.filter is not a
  function`.
- **forced fsp matrix**: `"F"` → "F"; **null → `'filter'` of null**;
  **0 → `content.filter`**; **`["x"]` → ""** (ct drops non-text
  blocks!). Forced-falsy ≠ unforced (contains_key!).

### emitBeforeAgentStart (runner.js L1119–1161 — verbatim-pinned)

```
async emitBeforeAgentStart(trigger, systemPromptOptions) {
    const currentOptions = normalizeBuildSystemPromptOptions(systemPromptOptions);  // 1st normalize — V8 throws ESCAPE
    const renderCurrentSystemPrompt = () => buildSystemPrompt(currentOptions);
    const ctx = Object.defineProperties({}, Object.getOwnPropertyDescriptors(this.createContext()));
    ctx.getSystemPrompt = () => { this.assertActive(); return renderCurrentSystemPrompt(); };
    const messages = [];
    for (const { ext, handlers } of snapshotEventHandlers(this.extensions, "before_agent_start")) {
        for (const handler of handlers) {
            try {
                const event = { ...trigger, type: "before_agent_start",
                    get systemPrompt() { return renderCurrentSystemPrompt(); },
                    systemPromptOptions: currentOptions };   // LIVE-REF (in-place mutations hit currentOptions)
                const handlerResult = await handler(event, ctx);
                if (handlerResult) {
                    const result = handlerResult;
                    if (result.message) messages.push(result.message);
                    if (result.systemPrompt !== undefined)   // NULL REASSIGNS
                        currentOptions.forceSystemPrompt = result.systemPrompt;
                }
            } catch (err) { this.emitError({extensionPath: ext.path, event: "before_agent_start", error, stack}); }
        }
    }
    return { messages, systemPromptOptions: currentOptions };
}
```

- ctx = **defineProperties-over-fresh-createContext** +
  `getSystemPrompt` = assertActive + **lazy** render (V8 throws from
  the normalize-throw-options + fsp-null/0 render paths surface at the
  event/ctx READS).
- `event.systemPrompt` = **lazy getter** (renders at read).
- `event.systemPromptOptions` = **currentOptions live-ref** —
  in-place event mutations mutate currentOptions (emitBoundary-style
  mutate steps must see it).
- `handlerResult` **truthy** → `result.message` **truthy**-push +
  `result.systemPrompt !== undefined` (**null REASSIGNS**).
- jstr(return) drops the `content`/`sections` distinctions: the op
  log = `{messages, systemPromptOptions: currentOptions}` (jstr =
  the 12/11-key normalize output).
- **Rust model**: `emit_before_agent_start(trigger, options: &Value)`
  → `normalize_options_raw(options)` (Err → the emitter THROWS OUT —
  op log `!...`); `build_system_prompt(&current, &doc_paths)` at the
  reads (ctx.getSystemPrompt + event.systemPrompt — LAZY: the event
  object carries an Arc<Mutex<Option<Result>>>-style render cache?? —
  NO: jstr drops functions; the harness reads systemPrompt → logs
  jstr(render) or !err-message → the event = a proxy w/ a lazy render
  fn; reads throw → caught by the read-try); **`doc_paths` must live
  on the runner** (host paths; the plan's DocPaths plumbing).

### exec.ts (item 1 — DONE 396a92a; probe-pinned)

`execCommand(command, args, cwd, options)`:
- `{stdout, stderr, code, killed}` literal order (both branches);
  stdout/stderr UTF-8-**lossy**; `code = exitCode ?? 0` (**signal-kill
  (ours OR the child's own) → 0**); killed = killProcess ran.
- **spawn errors (missing binary AND missing cwd) → the promise
  RESOLVES** `{stdout: "", stderr: "", code: 1, killed}` (empty
  stderr!; killed = pre-aborted signal) — **NO rejection**.
- **spawn-ARG validation → REJECTION** (exact node v24 messages):
  non-string command → `The "file" argument must be of type string.
  Received type number (5)`; non-array args → `The "args" argument
  must be of type object. Received type number (5)`; **array ELEMENTS
  are string-coerced, not validated** ([5] → "5", [null] → "null",
  [{}] → "[object Object]", [[1],[2,3]] → "1,2,3").
- No shell (`"/bin/sh -c 'echo hi'"` → code 1).
- `options.timeout > 0` (JS `>`; null/""/false → no; "120"/true →
  yes) → setTimeout(killProcess).
- abort: pre-aborted → killProcess immediately (ENOENT +
  pre-aborted → `{code: 1, killed: true}`); mid-run abort →
  `{code: 0, killed: true}`; partial stdout kept.
- `killProcess` = idempotent guard (killed = true + SIGTERM;
  detached 5 s SIGKILL).
- **Divergence**: the 100 ms post-exit pipe-idle grace (detached
  descendants) not ported (reads end at the pipes' EOF).
- Rust: `exec.rs` — `std::process` + reader threads (tokio has no
  "process" feature); `ExecSignal` seam (PreAborted/Live);
  `pi.exec` = assertActive → `execCommand(command, args,
  options?.cwd ?? loadingCwd, options)`.
'''
open(p,'w').write(s)
print("Addendum E + checklist:", len(s.splitlines()), "lines")
