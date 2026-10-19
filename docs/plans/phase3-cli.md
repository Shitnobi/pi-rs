# Phase 3 — CLI/TUI Application Layer Port Plan

Source of truth: `/tmp/pi-repo/packages/coding-agent/src/{cli/,modes/,main.ts,cli.ts,config.ts,rpc-entry.ts,migrations.ts,index.ts}`
Target crate: `crates/pi-cli` (binary `pi-cli`, already hosts the parity harness tool).

This is the **final consumer layer**: argument parsing → config/migration bootstrap → runtime construction (agent-session, providers, extensions) → one of three run modes (interactive TUI, print/JSON, RPC).

---

## 1. Inventory

### Top-level files

| File | LOC | Entry point | Responsibility | Build tooling or runtime? |
|---|---|---|---|---|
| `src/cli.ts` | 6 | `bin: pi` (node) | `setupCli(); main(process.argv.slice(2))` | Runtime (thin shim) |
| `src/rpc-entry.ts` | 13 | `exports["./rpc-entry"]` | sets `process.title = "pi-rpc"`, env flags, `configureHttpDispatcher()`, then `main(["--mode","rpc", ...])` | Runtime (thin shim) |
| `src/main.ts` | 1000 | — | `main(args, options?)` orchestrator: auth/package/config/mcp subcommand dispatch, arg validation, migrations, session manager creation, project trust, runtime factory, mode dispatch | Runtime — the core of this phase |
| `src/config.ts` | 656 | — | App identity constants, install-method detection, self-update commands, asset paths, agent-dir path resolution | Runtime (mostly); self-update/package-manager detection is NPM-specific (partial port) |
| `src/migrations.ts` | 315 | — | One-time startup migrations over `~/.pi/agent` and project `.pi/` | Runtime — parity-testable NOW |
| `src/index.ts` | 497 | `main:` | SDK public re-exports (barrel) | Runtime, but mostly re-exports — port as `pub use` façade only where useful |
| `src/package-manager-cli.ts` | ~1,090 | — | `pi install/remove/uninstall/update/list/config` commands: downloads extension packages from pi.dev installer API, self-update spawning npm/pnpm/yarn/bun, managed install dirs | Runtime but **packaging-ecosystem-specific**; port late / partially (see §6 step 10) |

### `src/bun/` (61 LOC total) — **BUILD/PACKAGING TOOLING → SKIP**

| File | LOC | Notes |
|---|---|---|
| `bun/cli.ts` | 4 | Bun-compiled-binary entrypoint shim (sandbox-env + runtime-setup + `../cli.ts`) |
| `bun/sandbox-env-setup.ts` | 4 | calls `restoreSandboxEnv()` |
| `bun/restore-sandbox-env.ts` | 36 | workaround for Bun issue #27802 (`process.env` empty in sandboxes; reads `/proc/self/environ`) |
| `bun/runtime-setup.ts` | 12 | Bun-specific: `registerBunOAuthFlows()`, bedrock module, embedded QuickJS wasm path |
| `bun/quickjs-wasm.d.ts` | 5 | type shim for wasm import |

All of this exists only to make `bun build --compile` binaries work (see `package.json` `build:binary` script). Rust has no Bun; **do not port**. The Node entry shims (`cli.ts`, `rpc-entry.ts`) map to the two Rust `[[bin]]` entries (`pi-cli` and a `pi-rpc` bin or a `--mode rpc` dispatch — the latter matches `rpc-entry.ts` which literally calls `main(["--mode","rpc",...])`).

### `src/cli/` (1,458 LOC) — runtime, port

| File | LOC | Key exports |
|---|---|---|
| `args.ts` | 479 | `parseArgs(args: string[]): Args`, `printHelp(extensionFlags?: ExtensionFlag[])`, `normalizeSessionName`, `isValidThinkingLevel`, `type Mode = "text"\|"json"\|"rpc"`, `interface Args` (~50 fields) |
| `startup-ui.ts` | 253 | `createStartupTui`, `startStartupTui`, `shouldRunFirstTimeSetup`, `showStartupSelector<T>`, `showStartupInput`, `showFirstTimeSetup` — pre-runtime TUI selectors |
| `auth-command.ts` | 126 | `parseAuthCommand`, `validateAuthCommandArgs`, `printAuthCommandHelp`, `AuthCommandError` — `pi auth` subcommand parsing |
| `list-models.ts` | 115 | `listModels(modelRuntime, searchPattern, signal)` — needs ModelRuntime (providers) |
| `credential-print.ts` | 90 | `resolveCredentialForPrint` — needs ModelRuntime/auth |
| `file-processor.ts` | 88 | `processFileArguments(fileArgs, opts)` → `{text, images}` — reads `@files`, images via image-resize worker |
| `auth-check.ts` | 73 | `checkProviderAuth`, `getProviderCredential`, `createAuthCheckModelRuntime` — needs providers |
| `project-trust.ts` | 62 | `createProjectTrustContext(...)` → `ProjectTrustContext` UI adapter (select/confirm/input/notify) |
| `config-selector.ts` | 61 | `selectConfig(options)` — TUI for `pi config` (uses pi-tui `TuiMainScreen`, `ProcessTerminal`) |
| `session-picker.ts` | 55 | `selectSession(...)` — TUI for `--resume` (uses `SessionSelectorComponent`) |
| `initial-message.ts` | 43 | `buildInitialMessage({parsed, fileText, fileImages, stdinContent})` — pure |
| `setup.ts` | 13 | `setupCli()`: process title, `PI_CODING_AGENT=1`, `AI_AGENT=pi`, silence warnings, http dispatcher |

### `src/modes/` (24,024 LOC) — runtime, port (bulk is deferred behind the agent layer)

| Path | LOC | Responsibility |
|---|---|---|
| `modes/index.ts` | 16 | barrel: re-exports the three modes |
| `modes/json-event.ts` | 61 | `toJsonEvent(event)` — strips cumulative `partial` snapshots from wire events (JSON/RPC protocol shape) |
| `modes/print-mode.ts` | 173 | `runPrintMode(runtime, opts): Promise<number>` — single-shot text/JSON output, signal handlers, raw-stdout backpressure |
| `modes/interactive/` | **21,927** | `interactive-mode.ts` **7,127 LOC** + `components/` (~13.9k: footer, model-selector 421, settings-selector 1009, session-selector 1045, config-selector 951, tree-selector 1428, tool-execution 408, easter-egg-3d 1292, …) + `theme/` (~2.2k: theme.ts 1086, system-theme.ts 655, theme-schema, theme-tokens, theme-controller) + tui-renderer, chat-viewport, external-editor, bug-report, session-share, program-status-reporter, model-search, model-catalog-refresh |
| `modes/rpc/` | 1,797 | `rpc-mode.ts` 819 (JSON stdin/stdout protocol loop), `rpc-client.ts` 617, `rpc-types.ts` 303, `jsonl.ts` 58 (line reader/serializer) |

**Important scoping note:** `modes/interactive/components/*` are *app-level* TUI components. The already-ported `pi-tui` crate is the *library* (editor, alt-screen, main-screen, markdown, keys — 1262/1262 parity). The app components here sit **on top of** pi-tui and are part of this phase, not the library port.

---

## 2. `config.ts` deep-dive (port EARLY — everything imports it)

### 2.1 What it is (and is not)

`config.ts` is **not** the user settings file. It is app identity + filesystem layout + package-manager detection. It has **no user-editable config parsing of its own** — it reads the *package's own* `package.json` for identity:

```ts
// src/config.ts:~437-460 (App Config section)
interface PackageJson {
    name?: string; version?: string;
    piConfig?: { name?: string; configDir?: string };
}
const piConfigName: string | undefined = pkg.piConfig?.name;
export const PACKAGE_NAME: string = pkg.name || "@earendil-works/pi-coding-agent";
export const APP_NAME: string = piConfigName || "pi";
export const APP_TITLE: string = piConfigName ? APP_NAME : "π";
export const CONFIG_DIR_NAME: string = pkg.piConfig?.configDir || ".pi";
export const VERSION: string = pkg.version || "0.0.0";

export const ENV_AGENT_DIR = `${APP_NAME.toUpperCase()}_CODING_AGENT_DIR`;   // PI_CODING_AGENT_DIR
export const ENV_SESSION_DIR = `${APP_NAME.toUpperCase()}_CODING_AGENT_SESSION_DIR`;
```

In Rust: hardcode these as consts in `pi-cli/src/config.rs` (`APP_NAME="pi"`, `CONFIG_DIR_NAME=".pi"`, `VERSION` from `env!("CARGO_PKG_VERSION")` or a build-time constant kept in parity with the TS version). Read via `std::env::var` behind a trait for testability (port convention: I/O behind traits — use an `EnvProvider`/`FsProvider` or take `&Env` params in tests).

### 2.2 Resolution order for the agent directory

```ts
export function getAgentDir(): string {
    const envDir = process.env[ENV_AGENT_DIR];
    if (envDir) return expandTildePath(envDir);          // 1. env override (PI_CODING_AGENT_DIR)
    return join(homedir(), CONFIG_DIR_NAME, "agent");    // 2. ~/.pi/agent
}
```

`expandTildePath(path)` = `normalizePath(path)` (from `utils/paths.ts`, already in pi-core `paths.rs`). Derived paths (all under `getAgentDir()` unless noted):

- `getModelsPath()` → `models.json`; `getAuthPath()` → `auth.json`; `getSettingsPath()` → `settings.json`
- `getToolsDir()` → `tools/`; `getBinDir()` → `bin/` (fd/rg); `getPromptsDir()` → `prompts/`
- `getSessionsDir()` → `sessions/`; `getCustomThemesDir()` → `themes/`
- `getDebugLogPath()` → `pi-debug.log`

Package dir: `getPackageDir()` honors `PI_PACKAGE_DIR` env override, else `dirname(process.execPath)` for bun-binary, else `findNodePackageDir(__dirname)` (walk up to nearest `package.json`). In Rust this becomes: `PI_PACKAGE_DIR` override, else compile-time `CARGO_MANIFEST_DIR`-derived or the executable dir. Asset paths built on it: `getThemesDir()`, `getExportTemplateDir()`, `getInteractiveAssetsDir()`, `getReadmePath()`, `getDocsPath()`, `getExamplesPath()`, `getChangelogPath()`, `getBundledInteractiveAssetPath(name)`. In Rust these assets should move to `include_str!`/`include_bytes!` (themes JSON, export-html templates, logo PNGs) with accessor functions returning `&'static str`/`Cow<'static, [u8]>`.

Other env knobs:
- `PI_SHARE_VIEWER_URL` (default `https://pi.dev/session/`) → `getShareViewerUrl(gistId)` = `${baseUrl}#${gistId}`
- `PI_OFFLINE`, `PI_SKIP_VERSION_CHECK`, `PI_STARTUP_BENCHMARK`, `PI_TELEMETRY` — truthy check is `isTruthyEnvFlag` in main.ts (`"1" | "true" | "yes"`, case-insensitive).

### 2.3 Install-method detection & self-update (lines ~20–360)

`detectInstallMethod(): InstallMethod` (`"bun-binary" | "npm" | "pnpm" | "yarn" | "bun" | "unknown"`), `getSelfUpdateCommand`, `getSelfUpdateUnavailableInstruction`, `getUpdateInstruction`, `isManagedByGlobalPackageManager`, `detectInstallChange`. This is ~340 LOC of npm/pnpm/yarn/bun global-install path heuristics consumed only by `pi update` (package-manager-cli.ts) and deprecation text.

**Rust decision:** port the *surface* (an `InstallMethod` enum with a `detect()` that returns `Unknown`/`Cargo` and the `getUpdateInstruction()` message strings for parity), but the npm-specific path-walking heuristics (`getInferredNpmInstall`, `getGlobalPackageRoots` spawning `npm root -g`, etc.) can be a **reduced stub** behind the same trait — they are meaningless for a Rust binary distribution. Parity-test only the pure message formatting. `detectInstallChange()` (re-read package.json, compare `version !== VERSION`) is trivially portable if we keep a `VERSION` const.

**Skip entirely:** `isBunBinary`/`isBunRuntime`/`isBundledNode` detection, QuickJS wasm path plumbing (`setEmbeddedQuickJSWasmPath`, `getQuickJSWasmPath`, `resolveCodemodeWorkerSpecifier`, `getCodemodeWorkerSpecifier`) — codemode/QuickJS is an extension-system concern.

### 2.4 User settings format (`core/settings-manager.ts`, 1,443 LOC — consumed by the CLI, lives in core)

- **Format: strict JSON with BOM stripping.** `JSON.parse(stripBom(content))` (settings-manager.ts:380, 606). No comments, no YAML. BOM handling already exists in pi-core (`text.rs` — verify `strip_bom` exists; add if not).
- **Two scopes merged:** global `~/.pi/agent/settings.json` (path from `getAgentDir()`) and project `<cwd>/.pi/settings.json` (path = `join(cwd, CONFIG_DIR_NAME, "settings.json")`, settings-manager.ts:319-320). Project scope returns `{}` when the project is not trusted (`projectTrusted=false`).
- **Migration-on-load** `migrateSettings()` (settings-manager.ts:~408-463): `queueMode → steeringMode`; `websockets: bool → transport: "websocket"|"sse"`; legacy `skills` object `{enableSkillCommands, customDirectories}` → array + hoisted `enableSkillCommands`; `retry.maxDelayMs → retry.provider.maxRetryDelayMs`. Exact semantics must be mirrored (in-place key moves, only when target key absent).
- Locking via `proper-lockfile` and `SettingsStorage` abstraction (in-memory + file). Rust: `fs2`/`fs4` advisory lock or a faithful `SettingsStorage` trait with `with_lock`.
- Env does **not** override individual settings; env vars only select directories (§2.2) and provider API keys (pi-ai layer).

### 2.5 `main.ts` env/TTY resolution affecting config

`resolveAppMode(parsed, stdinIsTTY, stdoutIsTTY)` (main.ts:~117): `rpc` if `--mode rpc`; `json` if `--mode json`; `print` if `--print` or either stream is not a TTY; else `interactive`. Session dir resolution order (main.ts:~718-722):

```
parsed.sessionDir (normalized)  ??  env PI_CODING_AGENT_SESSION_DIR (tilde-expanded)  ??  settingsManager.getSessionDir()
```

---

## 3. Dependency graph: app layer → unported modules

Interfaces the CLI consumes (from `main.ts` imports, lines 1–73):

**From `core/` (agent-session layer — Phase 2 output):**
- `createAgentSessionRuntime` / `CreateAgentSessionRuntimeFactory` (`core/agent-session-runtime.ts`) — the object everything downstream consumes: `{ services, session, modelFallbackMessage, dispose() }` where `services = { settingsManager, modelRuntime, resourceLoader }`.
- `createAgentSessionServices`, `createAgentSessionFromServices`, `AgentSessionRuntimeDiagnostic` (`core/agent-session-services.ts`, `core/sdk.ts`) — `CreateAgentSessionOptions`.
- `SessionManager` (`core/session-manager.ts`): `inMemory`, `open`, `forkFrom`, `findById`, `list`, `listAll`, `buildSessionContext`, `getCwd`, `appendSessionInfo`, `assertValidSessionId` — **partially already in pi-session (`session_manager.rs`); extend, don't recreate.**
- `SettingsManager` (see §2.4): `create(cwd, agentDir, opts)`, `getGlobalSettings`, `getTheme`, `applyOverrides`, `getSessionDir`, `getEnabledModels`, `getTerminalCapabilityOverrides`, `getShowHardwareCursor`, `getClearOnShrink`, `getDefaultProjectTrust`.
- `ModelRuntime` (`core/model-runtime.ts`) + `resolveCliModel`/`resolveModelScope` (`core/model-resolver.ts`) — **providers layer**; needed by list-models, auth-check, runtime factory.
- `AuthStorage`/`ReadOnlyAuthStorage` (`core/auth-storage.ts`) — reads/writes `auth.json` with 0600 perms.
- `ProjectTrustStore`/`resolveProjectTrusted`/`hasTrustRequiringProjectResources` (`core/trust-manager.ts`, `core/project-trust.ts`).
- Resource loader / extensions (`DefaultResourceLoader`, `builtInExtensions`, `InlineExtension`, `ExtensionFlag`), MCP command (`loadMcpCommand`), `exportFromFile` (HTML export), `output-guard` (`takeOverStdout`/`writeRawStdout`/backpressure), `http-dispatcher`, `timings`, `settings-diagnostics`, `session-cwd`.

**From `@earendil-works/pi-ai` (providers):** `modelsAreEqual`, `ImageContent`, `Usage` (json-event.ts).

**From `@earendil-works/pi-agent-core`:** `ThinkingLevel`, `AgentMessage` — map to pi-session/pi-core types.

**From `@earendil-works/pi-tui` (ALREADY PORTED):** `ProcessTerminal`, `TuiMainScreen`, `setKeybindings`, `setCapabilityOverrides`, `Markdown` — the CLI's TUI selectors (session-picker, config-selector, startup-ui) and InteractiveMode's render tree consume these directly from pi-tui. This is the main seam to the already-ported stack: `pi-cli` should depend on `pi-tui` (Cargo.toml already does).

**Modes' consumption:**
- `runPrintMode(runtimeHost: AgentSessionRuntime, options: PrintModeOptions): Promise<number>` — needs full runtime; also `output-guard` raw stdout.
- `runRpcMode(runtime)` — runtime + extension UI bridge + theme.
- `new InteractiveMode(runtime, {migratedProviders, startupDiagnostics, modelFallbackMessage, autoTrustOnReloadCwd, initialMessage, initialImages, initialMessages, verbose, tuiMode, initialThemeSetting})` then `.init()`/`.run()`/`.stop()`.

---

## 4. Parity-testable NOW vs. later

**Testable now (no agent/providers/extensions needed):**

| Unit | What to parity-test | Harness approach |
|---|---|---|
| `cli/args.ts` → `parseArgs` | full flag matrix, `--` handling, `@file` args, `--print` value-grab quirk (`-p` followed by non-`-`/non-`@` arg becomes a message), unknown `--flag[=value]` capture into `unknownFlags` Map (extension flags), `--thinking` levels (warning vs error), `--mode`/`--tui-mode` validation, diagnostics array | feed identical argv arrays through `ts-harness.js` and Rust `parse_args`; compare JSON-serialized `Args` (Map → object) |
| `normalizeSessionName`, `isValidThinkingLevel` | trivial cases | same |
| `printHelp` | exact output text (with/without extension flags) | byte-compare (strip ANSI or run both through same chalk level) |
| `cli/initial-message.ts` → `buildInitialMessage` | stdin+fileText+messages combination, `messages.shift()` mutation | pure function — JSON compare |
| `cli/auth-command.ts` parsing (`parseAuthCommand`, `validateAuthCommandArgs`, help text) | argv matrix | pure — JSON compare |
| `config.ts` path functions | `getAgentDir` etc. under fake `HOME`/`PI_CODING_AGENT_DIR`/`PI_PACKAGE_DIR` env, `getShareViewerUrl` with/without `PI_SHARE_VIEWER_URL` | harness sets env, calls both, compares paths |
| `config.ts` `getSelfUpdateCommandForMethod` message formatting | display strings per method (after stubbing install detection) | string compare |
| `migrations.ts` → `runMigrations(cwd)` | seed fixture dirs (legacy `oauth.json`, `settings.json` w/ `apiKeys`, stray `*.jsonl` in agent root, `commands/` dirs, `tools/fd`), run both TS and Rust in identical temp dirs, compare resulting file trees + return values | fs-fixture harness (tempdir per side) |
| `settings-manager.ts` JSON load + `migrateSettings` | old-format settings → migrated output; BOM handling; project-untrusted → `{}` | fixture JSON in, JSON out |
| `modes/json-event.ts` → `toJsonEvent` | given `AgentSessionEvent` fixtures (toolcall_start partial stripping, message_update usage projection) | depends on pi-session event types being exposed; if not yet, port the shape structs in pi-cli now and re-home later |
| `main.ts` pure helpers | `resolveAppMode` truth table, `isTruthyEnvFlag`, `validateForkFlags`/`validateSessionIdFlags` conflict messages, `toPrintOutputMode` | pure — JSON compare |
| `bun/` | — | skipped entirely |

**Testable only after settings/trust layer (Phase 2 mid):** `SettingsManager` full merge behavior, project-trust context, `session-picker`/`config-selector`/`startup-ui` (TUI-interactive — parity via component-state snapshots where the pi-tui harness pattern applies).

**Testable only after agent layer (Phase 2 end):** `print-mode` end-to-end (`pi -p` output), `json-event` against real streamed events, `runRpcMode` protocol (JSONL round-trips), `main()` orchestration, `list-models`, `auth-check`/`credential-print` (providers).

**Last:** `interactive-mode.ts` + components (needs everything), `package-manager-cli.ts` (needs network/installer API — recommend stubbing the installer API behind a trait and parity-testing only CLI arg handling + settings mutation).

---

## 5. Proposed Rust placement

```
crates/pi-cli/
  src/
    main.rs                 # main(args) orchestrator (port of main.ts); also keeps harness tool
    bin/pi-rpc.rs            # (optional) port of rpc-entry.ts — or just dispatch --mode rpc in main
    config.rs                # config.ts: consts, env, path resolution, asset accessors, install-method stub
    cli/
      mod.rs
      args.rs                # Args struct, parse_args, print_help, normalize_session_name
      initial_message.rs     # build_initial_message
      auth_command.rs        # parse/validate/help for `pi auth`
      file_processor.rs      # process_file_arguments (image resize via pi-photon)
      setup.rs               # setup_cli (env flags; http dispatcher stub until ported)
      auth_check.rs          # after providers
      credential_print.rs    # after providers
      list_models.rs         # after providers
      project_trust.rs       # after trust layer
      session_picker.rs      # TUI selector over pi-tui
      config_selector.rs     # TUI selector over pi-tui
      startup_ui.rs          # startup selectors, first-time setup
    migrations.rs            # run_migrations(cwd)
    modes/
      mod.rs
      json_event.rs          # to_json_event
      print_mode.rs          # run_print_mode
      rpc/                   # rpc_mode.rs, rpc_types.rs, rpc_client.rs, jsonl.rs
      interactive/           # interactive_mode.rs + components/ + theme/
```

**Interaction with pi-tui (already ported):**
- Startup selectors use `TuiMainScreen` + `ProcessTerminal` directly (`cli/config-selector.ts:16`, `cli/session-picker.ts:4`) — call the ported pi-tui equivalents; `setKeybindings`, `setCapabilityOverrides` are already ported library functions.
- `modes/interactive/theme/theme.ts` (`initTheme`, `stopThemeWatcher`, `validateThemeJson`) is app-level and lives in pi-cli; it loads theme JSON files (bundled via `include_str!` + user `~/.pi/agent/themes/`).
- `output-guard` (raw stdout takeover) needs a Rust equivalent: wrap `std::io::stdout` behind a lock/buffered writer with backpressure-aware `write_raw_stdout` — put in pi-cli (or pi-core io module) with a trait for testing.

**index.ts (SDK barrel):** do not port as a 1:1 file; instead expose the chosen public API from each crate's `lib.rs`. If a `pi` SDK facade crate is wanted later, add `crates/pi-sdk`.

---

## 6. Step-by-step porting checklist (ordered; LOC estimates are Rust-side, typically ~0.8–1.2× TS)

### Stage A — testable now (do first)

1. **`config.rs`** (~250 LOC + tests): consts (`APP_NAME`, `APP_TITLE`, `CONFIG_DIR_NAME`, `PACKAGE_NAME`, `VERSION`), env-var names, `get_agent_dir` + all derived path fns behind an `EnvFs` trait, `PI_PACKAGE_DIR`, `get_share_viewer_url`, asset accessors via `include_str!`. Add parity harness entries for path resolution + share URL. *Est. 2 days.*
2. **`cli/args.rs`** (~450 LOC incl. help text): `Args` struct (`Option<…>` fields, `messages: Vec<String>`, `file_args: Vec<String>`, `unknown_flags: BTreeMap<String, FlagValue>`, `diagnostics: Vec<Diagnostic>`), `parse_args` with the exact ordering quirks (`--` terminator, `-p` message grab, `@file`, `--flag=v`/`--flag v` unknown capture), `print_help` (embed full help text; keep env-var block). **This is the highest-value parity target** — dozens of harness cases. *Est. 3 days.*
3. **`cli/initial_message.rs`** (~40 LOC) + `main.rs` pure helpers (`resolve_app_mode`, `is_truthy_env_flag`, flag-conflict validators): extract from main.ts now so main.rs later is assembly-only. *Est. 1 day.*
4. **`cli/auth_command.rs`** (~150 LOC): parse/validate/help for `pi auth print-api-key|print-bearer-token|check` (pure parts only; credential resolution stubs behind trait). *Est. 1 day.*
5. **`migrations.rs`** (~300 LOC): `migrate_auth_to_auth_json` (oauth.json + settings.apiKeys → auth.json, 0600 mode, `.migrated` rename), `migrate_sessions_from_agent_root` (first-line header parse, `--{cwd-encoded}--` dir naming — reuses pi-session encoding), `migrate_commands_to_prompts`, `migrate_keybindings_config_file` (depends on core/keybindings `migrateKeybindingsConfig` — check pi-tui keys port; if absent, port that function here or in pi-session), `migrate_tools_to_bin`, `check_deprecated_extension_dirs`, `run_migrations(cwd)`, `show_deprecation_warnings` (prompt part behind a trait). Fixture-based parity harness (tempdir seeds → compare trees). *Est. 3 days.*
6. **`settings` JSON parse + `migrate_settings`** (~200 LOC, in pi-session or pi-cli): strict JSON + BOM strip, global/project merge skeleton, the four key migrations. Even before the full SettingsManager, this unblocks migrations and config parsing parity. *Est. 2 days.*
7. **`modes/json_event.rs`** (~100 LOC): event structs for the wire shape (or re-export from pi-session once events land) + `to_json_event` with `partial`-stripping semantics. *Est. 1 day.*

### Stage B — after settings/trust/session layer

8. **`cli/file_processor.rs`** (~150 LOC): `@file` reading, text/image split, image resize via `pi-photon` (port of image-resize-worker path). **`cli/startup_ui.rs`, `session_picker.rs`, `config_selector.rs`, `project_trust.rs`** (~500 LOC combined, minus the big selector components which arrive with Stage D): startup TUI creation, `should_run_first_time_setup` (checks `getSettingsPath()` absence), selector/input primitives over pi-tui. *Est. 4 days.*
9. **`modes/print_mode.rs` + output-guard** (~250 LOC): raw stdout writer, signal handlers (SIGTERM/SIGHUP exit codes 143/129), text/json output paths. *Est. 2 days.*

### Stage C — after agent-session + providers

10. **`main.rs` full orchestration** (~700 LOC Rust): subcommand dispatch (auth → package/config/mcp → parse → version/export → mode resolution → migrations → session manager → trust → runtime factory → mode dispatch), `createSessionManager` incl. `resolveSessionPath` (path/local/global/not_found), `prepareInitialMessage`, timing points. Reuse pi-session `SessionManager`. **`cli/auth_check.rs`, `credential_print.rs`, `list_models.rs`** (~250 LOC). **`modes/rpc/`** (~900 LOC): jsonl reader, rpc-types (serde), rpc-mode loop, rpc-client. Parity: JSONL round-trip protocol tests. *Est. 8–10 days.*
11. **`package-manager-cli.ts` decision point** (~400 LOC Rust for the arg/settings surface): port `handleConfigCommand` (TUI config selector) and `pi list`; implement `install/remove/update` behind an `InstallerApi` trait with the pi.dev HTTP API ported faithfully but network mocking in tests. Self-update for a Rust binary = download release artifact (replaces the npm heuristic block). *Est. 4 days; can be deferred post-MVP.*

### Stage D — interactive mode (last, largest)

12. **`modes/interactive/theme/`** (~1,200 LOC Rust): theme loading/validation/watcher, bundled theme JSONs via `include_str!`, system-theme (terminal capability queries via pi-tui). *Est. 4 days.*
13. **`modes/interactive/components/`** (~12,000 LOC Rust): port in dependency order — messages (user/assistant/custom/tool-execution/bash-execution/diff), selectors (model, settings, session, config, tree-selector), footer, editors, misc (auth-url, countdown, pi-logo, mermaid, …). Each component is independently parity-testable against the pi-tui render harness. *Est. 3–5 weeks.*
14. **`modes/interactive/interactive_mode.ts` → `interactive_mode.rs`** (~6,500 LOC Rust): the app loop — keybindings, slash commands, editor integration, model cycling, session share, bug-report, external editor, model catalog refresh, program status reporter. *Est. 2–3 weeks.*

### Running totals

| Stage | Rust LOC est. | Dependency |
|---|---|---|
| A (items 1–7) | ~1,500 | none — start immediately |
| B (items 8–9) | ~1,150 | settings/trust/session |
| C (item 10–11) | ~2,250 | agent-session, providers |
| D (items 12–14) | ~19,700 | everything |

### Parity harness additions (Stage A)

Add to `parity/ts-harness.js` + `pi-cli` harness tool:
- `parseArgs` — ~60 argv vectors covering every flag, quirk, and diagnostic;
- `buildInitialMessage`, `normalizeSessionName`, `resolveAppMode`, `isTruthyEnvFlag`;
- `parseAuthCommand`/`validateAuthCommandArgs` matrices;
- `config` paths + `getShareViewerUrl` under env overrides;
- `runMigrations` fixture-tree comparison (seed dirs committed under `parity/fixtures/migrations/`);
- `migrateSettings` fixtures (legacy settings JSON in/out);
- `toJsonEvent` event fixtures;
- `printHelp` text compare.

---

## Key risks / notes

- **`interactive-mode.ts` is 7,127 LOC** and imports ~40 core modules; it cannot start before Phase 2 completes. Do not attempt partial ports of it in Stage A/B.
- **Exact string parity matters** for help text, diagnostics, and migration messages (chalk colors) — keep a shared ANSI-stripped compare in the harness.
- **`Args.unknownFlags` is a `Map` with insertion order** — JSON-serialize as ordered object for parity; use `IndexMap` in Rust.
- **`migrations` writes files with mode 0600** (`writeFileSync(..., { mode: 0o600 })`) — mirror with `std::os::unix::fs::OpenOptionsExt`.
- **`printHelp` env-var block** is ~90 lines listing every provider API key env var — port verbatim (it's user-facing documentation).
- **`rpc-entry.ts` sets env vars before `main`** (`PI_CODING_AGENT=true`, `AI_AGENT=pi`) — replicate in the Rust bin or at the top of `main()` when `--mode rpc` comes from the rpc entry.
