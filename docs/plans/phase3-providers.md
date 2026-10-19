# Phase 3 Port Plan: LLM Provider / Model Layer

Source: `/tmp/pi-repo/packages/coding-agent/src/core/` (TS original, Node 24 semantics).
Target: `/home/shinobi/AI-Trident/pi-rs`.

Scope: every `provider-*`, `model-*`, `auth-*` file in `src/core/` plus the closely coupled
`runtime-credentials.ts` and `virtual-models.ts`. The layer sits between `pi-ai`
(`@earendil-works/pi-ai`, the model/Provider/credential SDK) and `agent-session.ts`.
**The single biggest scoping fact: this layer is a thin-but-critical composition layer over the
`pi-ai` package.** A faithful port requires porting a (minimal) `pi-ai` type surface first
(`Model`, `Provider`, `CredentialStore`, `Models`/`MutableModels`, `AuthResult`, `lazyStream`,
event streams). That is called out in §6.

---

## 1. File-by-file inventory

Line numbers below are **file-local** (as in the source file itself).

### 1.1 `http-dispatcher.ts` — 113 LOC
- **Exports**:
  - `DEFAULT_HTTP_IDLE_TIMEOUT_MS = 300_000` (l4)
  - `HTTP_IDLE_TIMEOUT_CHOICES` (l8–14): `[{label, timeoutMs}] as const`
  - `parseHttpIdleTimeoutMs(value: unknown): number | undefined` (l19)
  - `formatHttpIdleTimeoutMs(timeoutMs: number): string` (l37)
  - `applyHttpProxySettings(httpProxy: string | undefined): void` (l45) — sets `process.env.HTTP_PROXY ??=` / `HTTPS_PROXY ??=`
  - `configureHttpDispatcher(timeoutMs: number = DEFAULT_HTTP_IDLE_TIMEOUT_MS): void` (l81) — installs a global undici `EnvHttpProxyAgent` (`allowH2: false`, `proxyTunnel: true`, `bodyTimeout`/`headersTimeout` = timeout, `autoSelectFamilyAttemptTimeout: 2_000`) and conditionally `undici.install()`.
- **Internal state**: module-level `originalGlobalFetch` / `installedGlobalFetch` (l16–17) tracking deliberate `globalThis.fetch` overrides; helpers `withUndiciErrorListener`, `createUndiciClient`, `createUndiciOriginDispatcher` (Pool with `connections === 1` → single `Client`).
- **Responsibility**: pure parse/format of the idle-timeout setting + global HTTP client configuration (process side effect). Only the parse/format half is parity-testable.

### 1.2 `provider-attribution.ts` — 97 LOC
- **Exports**: `mergeProviderAttributionHeaders(model: Model<Api>, settingsManager: SettingsManager, sessionId: string | undefined, ...headerSources: Array<ProviderHeaders | undefined>): ProviderHeaders | undefined` (l79 file-local; TS l79).
- **Internals**: `matchesHost(baseUrl, expectedHost)` (l11, WHATWG `new URL().hostname`, try/catch → false); host constants (l5–9: openrouter.ai, integrate.api.nvidia.com, api.cloudflare.com, gateway.ai.cloudflare.com, opencode.ai); `isOpenRouterModel` (l19 — `provider === "openrouter" || baseUrl.includes(OPENROUTER_HOST)` — note: `includes` on the **whole URL string**, not hostname!); `isNvidiaNimModel` (l24); `isCloudflareModel` (l28); `getDefaultAttributionHeaders` (l39 — gated on `isInstallTelemetryEnabled(settingsManager)`, returns OpenRouter `HTTP-Referer`/`X-OpenRouter-Title`/`X-OpenRouter-Categories`, NVIDIA `X-BILLING-INVOKE-ORIGIN`, Cloudflare `User-Agent: pi-coding-agent`); `getSessionHeaders` (l70 — opencode/opencode-go/OPENCODE_HOST → `x-opencode-session`, `x-opencode-client: pi`).
- **Merging**: `merged = {...sessionHeaders, ...defaultHeaders}` then `Object.assign(merged, headers)` per source; empty → `undefined`.
- **Dependencies**: `SettingsManager` (only via `isInstallTelemetryEnabled`, `core/telemetry.ts`), pi-ai types.

### 1.3 `model-config.ts` — 175 LOC
- **Responsibility**: one immutable, credential-blind load of `models.json`; typebox schema validation.
- **Exports**:
  - `ModelsConfigSchema` (l84 file-local) — `{$schema?, providers: Record<string, ProviderConfigSchema>}`
  - Types `ModelsJsonModel`, `ModelsJsonModelOverride`, `ModelsJsonProvider` (l90–92) via typebox `Static`
  - `class ModelConfig` (l115): `static async load(modelsJsonPath: string | undefined): Promise<ModelConfig>` (l124), `getProvider(providerId): ModelsJsonProvider | undefined` (l164), `getProviderIds(): readonly string[]` (l168), `getError(): string | undefined` (l172).
- **Internals**: `SamplingParamsSchema`, `SamplingParamsByThinkingLevelSchema` (levels off/minimal/low/medium/high/xhigh/max), `PositiveTokenCountSchema` (`Type.Number({exclusiveMinimum: 0})`), `ModelDefinitionSchema` (l54), `ModelOverrideSchema` (l73), `ProviderConfigSchema` (l89 — incl. `oauth: Type.Literal("radius")`, `authHeader`), compiled validator `validateModelsConfig = Compile(...)` (l89 l.89), `formatValidationPath` (l95 — typebox error path → `a.b.c` dotted; `required` keyword special-case reading `params.requiredProperties[0]`), `deepFreeze` (l108).
- **Load semantics** (l124–142): `normalizePath` → `readFile` (ENOENT → empty config, no error; other IO error → error string `Failed to load models.json: <msg>\n\nFile: <path>`) → `JSON.parse(stripJsonComments(stripBom(content)))` (parse error → `Failed to parse models.json: <msg>\n\nFile: <path>`) → schema check (failure → `Invalid models.json schema:\n  - <path>: <message>\n...` joined by `\n`) → providers map with `deepFreeze(structuredClone(provider))`.
- **State**: instance only (immutable). No module state.

### 1.4 `model-registry.ts` — 244 LOC
- **Responsibility**: synchronous compatibility facade over `ModelRuntime` for extensions. Nearly pure delegation.
- **Exports**: `ResolvedRequestAuth` union (l33–41: `{ok: true; apiKey?; headers?; baseUrl?; env?} | {ok: false; error}`), re-export `ProviderConfigInput`, `clearApiKeyCache`, and `class ModelRegistry` (l48) with:
  - `refresh(options?): Promise<ModelsRefreshResult>` (l56)
  - `getError()`, `getAll()`, `getAvailable()`, `find(provider, modelId)`, `findOfType(type, provider, modelId)`, `hasConfiguredAuth(model)`, `getProviderAuthStatus(provider)`, `getProvider(provider)`
  - `async getApiKeyAndHeaders(model): Promise<ResolvedRequestAuth>` (l89) — **non-trivial**: falls back to `getCompatibilityRequestConfig`; special-cases thrown error message `"authHeader requires a resolved API key"` → `` `No API key found for "${model.provider}"` ``; inspects `error.cause` (l107–109); only spreads `baseUrl` when truthy (l103).
  - `stream`/`streamSimple`/`complete`, `getModelsOfType`, `getAvailableOfType`, `getModelOfType`, `classify`, `generateImages`, `getProviderDisplayName`, `getProviderAuth`, `async getApiKeyForProvider` (swallows errors → undefined), `isUsingOAuth`
  - `registerProvider` overloads (l210–219: `Provider` object or name+`ProviderConfigInput`; by-name without config throws `Provider config is required when registering by name`), `unregisterProvider`, `registerVirtualModel`, `unregisterVirtualModel`, `getRegisteredProviderConfig`, `getRegisteredNativeProvider`, `getRegisteredProviderIds` (set-deduped, extension keys first).
- **State**: none of its own (delegates to runtime).

### 1.5 `model-resolver.ts` — 684 LOC
- **Responsibility**: model reference resolution (`provider/id:thinking`), scope pattern resolution (`--models` globs), CLI model resolution, initial-model selection, session restore. **The most parity-valuable file: almost entirely pure logic over a `Model[]` list.**
- **Exports**:
  - `defaultModelPerProvider: Partial<Record<KnownProvider, string>>` (l20–62) — 41 entries; **insertion order matters** (used by `Object.keys` iteration in `findInitialModel`/`restoreModelFromSession`).
  - `ScopedModel` (l64: `{model, thinkingLevel?}`)
  - `ModelReferenceRequest` (l76: `{text, provider?, thinkingSuffix: "none"|"strict"|"lenient", fuzzy}`)
  - `ModelReferenceResult` (l93): `resolved {model, thinkingLevel?, warning?}` | `ambiguous {candidates, usableCount, thinkingLevel?, warning?}` | `not-found {target?}`
  - `ModelReferenceTarget` (l108: `{provider, providerModels: [Model, ...Model[]], modelText, modelId, thinkingLevel?}`)
  - `resolveModelReference(request, models, usable: (provider) => boolean): ModelReferenceResult` (l144)
  - `findExactModelReferenceMatch(modelReference, models)` (l174)
  - `resolveModelScopeFromModels(patterns: string[], models): ResolveModelScopeResult` (l327)
  - `resolveModelScopeWithDiagnostics` / `resolveModelScope` (l403/l411 — console.warn chalk.yellow side effects)
  - `resolveCliModel(options)` (l445) → `ResolveCliModelResult` (l423)
  - `findInitialModel(options)` (l543) → `InitialModelResult` (l531)
  - `restoreModelFromSession(savedProvider, savedModelId, currentModel, shouldPrintMessages, modelRuntime)` (l615)
- **Key internals**: `splitThinkingSuffix` (l120, `lastIndexOf(":")` + `isValidThinkingLevel`), `splitProviderPrefix` (l127, first `/`, prefix lowercased+trimmed), `resolveText` (l186, recursion with strict/lenient suffix handling — invalid suffix in lenient mode produces warning `` `Invalid thinking level "${suffix}" in pattern "${text}". Using default instead.` ``), `selectReference` (l230 — raw id matches vs named-provider matches tie-breaking with `usable`), `fuzzyMatches` (l262, substring on lowercased id/name), `selectFuzzy` (l269), `bestFuzzyMatch` (l287 — alias preference + `b.id.localeCompare(a.id)` sort), `isAlias` (l292, `/-\d{8}$/`), `buildFallbackModel` (l296).
- **Glob branch** (l350–381): patterns containing `*?[` → `minimatch(\`${provider}/${id}\`, glob, {nocase: true})` OR `minimatch(id, glob, {nocase: true})`, after first trying exact resolution (bracketed ids like `bracketed-model[1m]` are not globs).
- **Console output**: `resolveModelScope` and `restoreModelFromSession` print via `chalk.yellow`/`chalk.dim` when `shouldPrintMessages`.

### 1.6 `provider-composer.ts` — 732 LOC
- **Responsibility**: compose built-in provider + `models.json` config + extension registration into a `Provider`, without reading credentials; auth-method composition (API-key / OAuth wrappers); model header resolution.
- **Exports**:
  - `ExtensionOAuthConfig` (l40), `ProviderChatModelConfig`/`ProviderImageModelConfig`/`ProviderClassifierModelConfig`/`ProviderModelConfig` (l63–88), `ProviderConfigInput` (l91)
  - `AuthStatus` (l110: `{configured: boolean; source?: "stored"|"runtime"|"environment"|"fallback"|"models_json_key"|"models_json_command"; label?}`)
  - `clearApiKeyCache = clearConfigValueCache` (l116)
  - `validateExtensionProvider(providerId, base, modelsConfig, extension): void` (l529) — throws on structural errors
  - `composeModelProvider(providerId, base, modelConfig, extension): Provider` (l542)
  - `resolveConfiguredModelHeaders(model, config, extension, env?)` (l685)
  - `CompatibilityRequestConfig` (l698), `resolveCompatibilityRequestConfig(model, config, extension)` (l703)
  - `configuredRequestAuthStatus(config, extension): AuthStatus | undefined` (l718)
- **Key internals**:
  - `mergeCompat` (l122): spread merge + deep merge for keys `openRouterRouting`, `vercelGatewayRouting`, `chatTemplateKwargs`, `chatTemplateArgs` when either side is an object.
  - `mergeInputLimits` (l144): spread + nested `images`/`images.resize` merge.
  - `mergeSamplingParamsByThinkingLevel` (l164): per-level object merge.
  - `applyModelOverride(model, override)` (l177): field-by-field `??` fallbacks; `cost` merged per-field (`input/output/cacheRead/cacheWrite/tiers`).
  - `modelFromJson(providerId, definition, providerConfig, defaults)` (l210): defaults `reasoning: false`, `input: ["text"]`, `cost: {input:0, output:0, cacheRead:0, cacheWrite:0}`, `contextWindow: 128000`, `maxTokens: 16384`; throws `` `Provider ${providerId}, model ${definition.id}: no "api" specified. Set at provider or model level.` ``, `` `Provider ${providerId}: "baseUrl" is required when defining custom models.` ``, invalid contextWindow/maxTokens guards (l224–228).
  - `findModelDefaults` (l251): id → api → `openai-completions` → first chat model.
  - `findExtensionModelDefaults` (l261), `extensionModelFromDefinition` (l275).
  - `applyModelsJson` (l300): empty-config guard throws `` `Provider ${providerId}: must specify "baseUrl", "headers", "compat", "modelOverrides", or "models".` ``; oauth-without-baseUrl throws; radius oauth keeps model baseUrl (l326: `config.oauth === "radius" ? model.baseUrl : config.baseUrl ?? model.baseUrl`); custom-model upsert by id (replace-or-push, l331–337).
  - `applyExtension` (l341): no models → baseUrl remap only; with models → full replacement.
  - `adaptOAuth` (l353): adapts extension callbacks to pi-ai `OAuthAuth` (notify/prompt passthrough, `type: "oauth"` tagging).
  - `withConfiguredAuth` (l374): merges headers, `authHeader: true` requires apiKey else throws `"authHeader requires a resolved API key"`, adds `Authorization: Bearer <key>`.
  - `composeApiKeyAuth` (l417): the check/resolve closures — credential present → inherited check/resolve; raw configured key (possibly `!command` / `$ENV` template) → `resolveConfigValueOrThrow` with env from `ctx.env(name)`; header env resolution via `configContextEnv` (l403); returns `{auth: withConfiguredAuth(...), env, source}`.
  - `composeOAuthAuth` (l483): wraps `toAuth` with header resolution + `withConfiguredAuth`.
  - `rawModelHeaders` (l508): merges `modelOverrides[id].headers` (chat only) + `models[].headers` (chat only, matched by id) + extension `models[].headers` (matched by type+id); empty → undefined.
  - `composeModelProvider` body (l542–683): `getAllModels()` closure ordering: **extension replacement → oauth.modifyModels projection (chat only, chat-first ordering) → modelOverrides applied last**; eager validation call (l577); provider name precedence `extension.name ?? config.name ?? base.name ?? extension.oauth.name ?? providerId` (l607); `streamWith` lazy dispatch (extension.streamSimple when api matches → base when `supportsBaseApi` → global `getApiProvider(model.api)` else throw `No API provider registered for api: ...`); `refreshModels` closure with `context.publish({update})`; conditional passthrough of `filterModels`, `filterAllModels`, `fetchDeferred`, `cancelDeferred`, `generateImages`, `classify` (with error-result fallbacks `imageErrorResult`/`classifierErrorResult`).
- **Internal state**: closures (`extensionOAuthCredential`, `refreshedExtensionModels`) captured per composed provider; no module-level state.

### 1.7 `model-runtime.ts` — 1041 LOC
- **Responsibility**: the live orchestrator. Owns `MutableModels` (pi-ai), `RuntimeCredentials`, builtin provider catalog, extension/native/virtual registrations, composition error map, availability snapshots with sequence guards, credential operation queue, request preparation (auth + header merge), and all streaming entry points.
- **Exports**: `CreateModelRuntimeOptions` (l109: `{credentials?, authPath?, modelsPath?, modelsStore?, modelsStorePath?, allowModelNetwork?, modelRefreshTimeoutMs?, catalogBaseUrl?, signal?, refreshOnCreate?}`), `ModelRuntimeAuthOverrides` (l127: `{apiKey?, env?, minOAuthValidityMs?}` default 5 min), `CredentialSynchronizationOperation` (l134), `CredentialSynchronizationError` (l137 — message `` `Credential ${operation} committed for ${providerId}, but local synchronization failed` ``, name `"CredentialSynchronizationError"`), `class ModelRuntime implements Models` (l173).
- **Notable internals**:
  - `mergeHeaders(base, override)` (l156): case-insensitive delete-then-set (override casing wins, key moves to end).
  - `static async create` (l217): defaults `modelsPath = join(getAgentDir(), "models.json")`, `modelsStore = FileModelsStore(join(dirname(modelsPath), "models-store.json"))`, builtin providers wrapped with `withRemoteCatalog` (openrouter → `withPersonalizedCatalog`), `modelNetworkEnabled = process.env.PI_OFFLINE === undefined`, radius provider configs from models.json (`baseUrl.replace(/\/v1\/?$/u, "")`, l281), `AbortSignal.any` timeout handling, optional `refreshOnCreate`.
  - Snapshot machinery: `availabilityRefreshSeq`, `availabilityErrorSeq`, `providerAvailabilitySeq`, `runAvailabilityRefresh` (l342, `Promise.all` of getAvailable/checkAuth-per-provider/credentials.list), `queueAvailabilityRefresh` (l373), `refreshProviderAvailability` (l388, per-provider patch using `` `${provider}\0${model.id}` `` keys).
  - `getError()` (l511): config error + per-provider composition errors (`` `Provider "${providerId}": ${error}` ``) + availability error (`` `Availability refresh: ${error}` ``), joined `\n\n`.
  - `getAuth` overloads (l555): model form merges `resolveConfiguredModelHeaders` into resolution headers via `mergeHeaders`.
  - `enqueueCredentialOperation` (l579): per-provider serialized promise chain with "started" gating against abort.
  - `synchronizeCredentialState` (l599): recompose → models.refresh(allowNetwork: false) → refreshProviderAvailability; wraps failures in `CredentialSynchronizationError`.
  - `prepareRequest` (l658): provider lookup (`ModelsError("provider", "Unknown provider: ...")`), auth resolution (`ModelsError("auth", "Provider is not configured: ...")`), `transformHeaders` hook, env merge, baseUrl override.
  - Streaming: `stream`/`complete`/`streamSimple` (virtual-model routing branch l725–743, incl. maxTokens clamp and credential-provider mismatch guard)/`streamDeferred`/`fetchDeferred`/`cancelDeferred`/`generateImages`/`classify` (error-result wrapping).
  - Registration: `registerNativeProvider` (l891), `registerProvider` (l928 — validate-then-merge semantics: re-registration merges defined values over previous, preserving undefined ones), `unregisterProvider` (l951), `registerVirtualModel` (l964 — throws on empty id, physical-model conflict; marks provider provisionally configured with `source: "virtual"`), `unregisterVirtualModel` (l985), `resolveModel` (l1003, virtual routing with clamping + credential check), `getPhysicalModel` (l1037).
  - `refresh` (l848): reload config, radius reconfigure, provider-scoped vs full rebuild, availability refresh, legacy fallback for older pi-ai `refresh()` returning void (l861–866).
- **Internal state**: extensive (see l174–197). Mutated by async passes guarded by sequence numbers — a concurrency state machine.

### 1.8 `auth-storage.ts` — 507 LOC
- **Responsibility**: `CredentialStore` backed by `auth.json` with proper-lockfile locking, revision-based read caching, and in-memory/read-only backends.
- **Exports**:
  - `AuthStorageBackend` interface (l53): `withLock<T>(fn: (current: string | undefined) => LockResult<T>): T` and async variant with `AuthOperationOptions`.
  - `FileAuthStorageBackend` (l61): default path `join(getAgentDir(), "auth.json")`; `ensureParentDir` (mode 0o700), `ensureFileExists` (writes `"{}"` with mode 0o600), `acquireLockSyncWithRetry` (l81: 10 attempts × 20 ms **synchronous busy-wait**, retryable codes `ELOCKED` and win32 `EPERM`), `acquireLockAsync` (l117: 30 s stale deadline, exp backoff `min(10*2^retry, 1000)` ms × `(1 + Math.random())`, EPERM retries ≤ 5), `withLock`/`withLockAsync` (read-modify-write with `next` string, compromised-lock re-checks, abort checks).
  - `ReadOnlyAuthStorage` (l204): one-shot lazy load with strict shape validation (api_key: `key` undefined-or-string, `env` object of strings; oauth: `access`/`refresh` strings + finite-number `expires`) — error strings `Failed to read auth.json: ...`, `Invalid auth.json: expected an object`, `` `Invalid auth.json credential for provider "${providerId}"` ``; `read` resolves api_key values via `resolveConfigValue(key, env)`; `modify`/`delete` throw `"Read-only credential storage cannot modify auth.json"`.
  - `InMemoryAuthStorageBackend` (l293): value + `asyncChain` promise serialization.
  - `AuthStorage` (l328): `create`/`fromStorage`/`inMemory(data)` factories; **module-level shared read state** `sharedAuthFileReadState` (l51) keyed by authPath; `reload()` (sync, preserves last snapshot on error), `readLatestData` (revision check via `getFileRevision`, single-flight reload with `AbortController` + reader counting), `read` (api_key value resolution), `modify` (lock, run fn, merge `{...currentData, [provider]: next}`, write `JSON.stringify(merged, null, 2)`), `delete`, `list` (metadata only).
  - `readStoredCredential(providerId, authPath?)` (l497): one-off sync read, any error → undefined.
- **Traps**: `getFileRevision` = `` `${dev}:${ino}:${size}:${mtimeNs}:${ctimeNs}` `` (bigint stat, `utils/paths.ts` l36); `structuredClone` before returning entries; write format `JSON.stringify(x, null, 2)`; `JSON.parse(stripBom(content))` **without** schema validation in `AuthStorage.parseStorageData` (unlike `ReadOnlyAuthStorage`).

### 1.9 `models-store.ts` — 147 LOC
- **Exports**: `InMemoryCodingAgentModelsStore implements ModelsStore` (l26: map of `structuredClone`d entries, abort checks), `FileModelsStore implements ModelsStore` (l47): default `join(getAgentDir(), "models-store.json")`, reuses `FileAuthStorageBackend` + **module-level `sharedModelsFileReadState`** (l23), same revision/single-flight reload pattern as auth-storage, `read`/`write`/`delete` with `JSON.stringify(current, null, 2)`.
- **Note**: `parse` (l62) is `JSON.parse(stripBom(content))` with **no validation**.

### 1.10 `runtime-credentials.ts` — 52 LOC
- **Exports**: `RuntimeCredentials implements CredentialStore` (l4): overlay map of non-persistent API keys over a backing store; `setRuntimeApiKey`/`removeRuntimeApiKey`/`hasRuntimeApiKey`; `read` returns `{type: "api_key", key: override}` for overrides else delegates; `list` merges override markers (`{providerId, type: "api_key"}`) over backing entries (override wins); `modify`/`delete` delegate, `delete` also clears the override.
- Pure state-machine over an injected store — ideal parity target with a fake `CredentialStore`.

### 1.11 `auth-guidance.ts` — 25 LOC
- **Exports**: `getProviderLoginHelp()` (l6: 4-line string embedding `join(getDocsPath(), "providers.md")` / `models.md`), `formatNoModelsAvailableMessage()` (l14), `formatNoModelSelectedMessage()` (l18), `formatNoApiKeyFoundMessage(provider)` (l18/l19: `"unknown"` → "the selected model"). Pure string formatting given the docs path.

### 1.12 `remote-catalog-provider.ts` — ~250 LOC
- **Exports**: `REMOTE_CATALOG_REFRESH_INTERVAL_MS = 4*60*60*1000` (l16), `REMOTE_CATALOG_MODEL_TYPES = ["chat","image","classifier"]` (l23), `PersonalizeCatalog` type (l98), `withRemoteCatalog(provider, catalogBaseUrl = "https://pi.dev", localGeneratedAt?)` (l104), `withPersonalizedCatalog(...)` (l195).
- **Internals**: `parseCatalog` (l46: accepts array, `{models: [...]}`, or object-of-entries; filters entries with `id`; filters unsupported types; stamps `provider`), `readCatalog` (l65: `Date.parse(last-modified)`, NaN → 0, etag passthrough), `isNewer`, `remoteModels`, `requestCatalog` (l85: URL `/api/models/providers/${encodeURIComponent(providerId)}?types=chat,image,classifier`, headers `accept`, `User-Agent: getPiUserAgent(VERSION)`, optional `if-none-match`, via `fetchWithRetry` with 4 s attempt timeout), `mergeModels` (keyed `` `${getModelType(model)}\0${model.id}` ``, dynamic wins).
- I/O boundary: `fetchWithRetry` (utils/management-http.ts) — the natural `HttpDispatcher` trait seam.

### 1.13 `virtual-models.ts` — ~250 LOC
- **Exports**: `VIRTUAL_MODEL_API = "pi-virtual"` (l29), `VIRTUAL_MODEL_STATE_ENTRY = "pi.virtual-model-state"` (l32), `VirtualModelStateData` (l35), `ModelRouteReason` (l50: user|continuation|retry|direct), `ModelRouteRequest` (l52), `ModelRoute` (l73), `VirtualModelDefinition` (l84, incl. `route(request)` callback), `isVirtualModel(model: {api: string})` (l105), `findLatestResponse(messages)` (l110 — last assistant with `stopReason` not error/aborted), `getBranchSelection(branch, getModel)` (l128 — model_change vs last physical response logic), `getVirtualModelState(branch, provider, modelId)` (l159), `createVirtualModel(definition)` (l170 — `thinkingLevelMap` from `thinkingLevels ?? ["off"]`, `baseUrl: ""`, zero cost/limits, `input ?? ["text","image"]`), `withVirtualModels(providerId, provider, virtualModels)` (l201 — keyless provider synthesis, physical model hiding, filter composition, unrouted-stream errors).
- Depends on `SessionEntry` from `session-manager.ts` (already ported in pi-session).

### 1.14 `model-registry`/`model-runtime` external imports
From pi-ai: `createModels`, `MutableModels`, `lazyStream`, `normalizeContext`, `clampThinkingLevel`, `isModelType`, `getModelType`, `modelsAreEqual`, `ModelsError`, error-result helpers, builtin provider catalog (`providers/all`, incl. `radiusProvider`), `personalizeOpenRouterCatalog`.
From repo: `config.ts` (`getAgentDir`, `getDocsPath`, `VERSION`), `utils/abort.ts` (`operationSignal`, `raceWithAbortSignal`), `utils/paths.ts` (`normalizePath`, `getFileRevision`), `utils/text.ts` (`stripBom`), `utils/json.ts` (`stripJsonComments`), `utils/management-http.ts` (`fetchWithRetry`), `utils/pi-user-agent.ts`, `core/resolve-config-value.ts`, `core/telemetry.ts` (`isInstallTelemetryEnabled`), `cli/args.ts` (`isValidThinkingLevel`), `core/defaults.ts` (`DEFAULT_THINKING_LEVEL`), `session-manager.ts` (types), `chalk`, `minimatch`, `proper-lockfile`.

---

## 2. JS-semantics traps for Rust (exhaustive list)

These are the places where this port's history shows divergences hide. Each entry names the file:line and the required Rust behavior.

1. **`undefined` vs missing vs `null`** (everywhere):
   - `ModelConfig.getError(): string | undefined` — distinguish "no error" from empty string; use `Option<String>`, never `""`.
   - `parseHttpIdleTimeoutMs` returns `undefined` for invalid, `0` for "disabled" — three-valued (`Option<u64>` where `0` is meaningful).
   - `ReadOnlyAuthStorage` validation: `value.key === undefined || typeof value.key === "string"` — **`null` key fails** (`typeof null === "object"`). A JSON `null` must be distinct from absent.
   - `structuredClone(entry) ? ... : undefined` (models-store l121/l130): JSON `null` entry → undefined.
   - `resolution.auth.baseUrl ? {...}` (model-registry l103): falsy `""` also excluded.
   - Spread `{...model, name: override.name ?? model.name}`: when `override.name` is `undefined` **but the key exists**, JSON.stringify drops it — serde round-trip differs unless `skip_serializing_if = "Option::is_none"` is used consistently on every optional field.
2. **Object key insertion order + integer-key reordering**:
   - `Object.entries(parsed.providers)`, `Object.keys(defaultModelPerProvider)`, header merge results — JS objects iterate integer-like keys first (numerically sorted), then insertion order. A provider or model id like `"123"` **reorders**. Use `serde_json` with `preserve_order` (IndexMap) everywhere data crosses JSON, and emulate integer-key-first ordering where `Object.keys` iterates parsed JSON.
   - `mergeHeaders` (model-runtime l156): case-insensitive `delete` then set moves the key to the **end** of insertion order; serialization order changes.
   - `Object.assign(merged, headers)` order affects JSON output of header maps.
3. **`JSON.stringify(x, null, 2)` write format** (auth-storage l468/l480, models-store l131/l138): 2-space indent, `": "` separator, no trailing newline, **empty object prints `{}`**, key order = insertion order. Non-ASCII characters are emitted raw (UTF-8), lone surrogates become `\udXXX` escapes (see the pi-tui UTF-16 helpers / surrogate sentinels). Verify against the existing JS-compatible serializer used by the session layer (`session-manager` port already solved this — reuse it).
4. **`JSON.parse` error message strings** (model-config l131–134, auth-storage l223): Node 24 V8 messages like `Unexpected token 'h', ..."hello" is not valid JSON` are embedded verbatim in `getError()` strings. Parity requires a JS-compatible parse-error formatter (V8-style snippets with `...` and position). Check whether the session-manager port already has one; if not, this is a dedicated small module.
5. **Node fs error message strings** (model-config l134 `Failed to load models.json: ${error.message}`): `ENOENT: no such file or directory, open '<path>'` etc. The `node_fs.rs` module in pi-tools should provide Node-style `ErrnoException` messages; reuse it. ENOENT is handled specially (empty config, no error) — only **other** errors produce strings.
6. **typebox validation error output** (model-config l358–364): `formatValidationPath` reads `error.keyword === "required"` + `params.requiredProperties[0]`, converts `instancePath` `/a/b` → `a.b`, empty → `"root"`, and each error line is `` `  - ${path}: ${error.message}` ``. The **exact typebox error messages** (e.g. `Expected string, received undefined`) must be reproduced; capture them as golden fixtures from the TS harness rather than reimplementing typebox's wording from memory.
7. **`getFileRevision`** (utils/paths.ts l36): `` `${dev}:${ino}:${size}:${mtimeNs}:${ctimeNs}` `` with **bigint** stat. Rust `std::fs::Metadata` exposes secs+nanos; `mtimeNs` is total nanoseconds — compose as `secs * 1e9 + nanos`. dev/ino come from `MetadataExt`. Failure → `undefined` (not an error).
8. **JS `Number` parsing** (http-dispatcher l28 `Number(trimmed)`): accepts `"1e3"`, `"0x1E"` (→ 30), `"+5"`, `"Infinity"` (later rejected by `Number.isFinite`); rejects `"1_000"`, `""` (already guarded). Also `Math.floor` (l34) on e.g. `2.9` → 2. Port a `js_to_number(&str) -> f64` helper.
9. **`String.prototype.trim`** (http-dispatcher l21, applyHttpProxySettings): JS trim removes Unicode whitespace incl. `\u00A0`, `\uFEFF`, `\u2028`. Use the `js_trim` helper in pi-tui utils (or move it to pi-core).
10. **JS number-to-string** (http-dispatcher l42 `` `${timeoutMs / 1000} sec` ``): shortest-round-trip float formatting (e.g. `1500 → "1.5 sec"`, `1501 → "1.501 sec"`, `100 → "0.1 sec"`). Rust `{}` on f64 prints `1.5` but also `0.1` — mostly identical, but exponents differ at extremes (`1e21`). Use a ryu/JS-compatible formatter (see existing `js_number` in pi-core/ai.rs and friends).
11. **`toLowerCase()`** (model-resolver l133/136/237/245/263/265, l471): JS Unicode full lowercase mapping differs from Rust `to_lowercase()` in known cases (`İ` → `i̇` two chars in both, but `Σ` final sigma handling, `K` Kelvin sign). Test non-ASCII ids via harness; use a JS-compatible lowercase helper (check pi-tui utils / pi-core text).
12. **`localeCompare`** (model-resolver l289, l341, l493): ICU collation under Node's default locale. Diverges from Rust byte/codepoint compare for case, accents, digits. Mitigation: keep parity cases ASCII-first, and implement `js_locale_compare` as an explicit, fixture-verified comparator (simple `String::cmp` on UTF-16 code units matches ICU only for ASCII). Document the divergence; pin `LC_ALL` in the TS harness runner to make it deterministic.
13. **minimatch semantics** (model-resolver l374–376): `{nocase: true}`, globstar `**`, brace expansion, character classes, and minimatch's specific `.` handling. Rust `globset` is **not** a drop-in (brace expansion, `nocase` differences, `**` at start). Port or vendor a minimatch-compatible matcher; test against TS harness outputs for tricky patterns (`bracketed-model[1m]`, `*sonnet*`, `provider/*:high`).
14. **URL semantics** (provider-attribution l12 `new URL(baseUrl).hostname`; remote-catalog l86 `new URL(path, base)` + `searchParams.set`): WHATWG URL. Use the `url` crate (closest available) and fixture-test hostname extraction for odd inputs (`"not a url"`, `"https://EXAMPLE.com:443/x"`, IDs in userinfo). Note `isOpenRouterModel` uses `baseUrl.includes(host)` on the raw string (provider-attribution l19) — substring, **not** hostname — do not "fix" this.
15. **`Date.parse`** (remote-catalog l66): JS date parsing of `Last-Modified` HTTP dates (RFC 7231) plus V8 extensions; NaN → 0. Use `pi-core`'s `parse_date_millis` and verify against RFC-date fixtures. OAuth `credential.expires` is a JS epoch-ms number — no Date objects cross the JSON boundary, but `minOAuthValidityMs` arithmetic (default 5 min) is ms-based.
16. **`process.env` presence vs empty** (model-runtime l247 `process.env.PI_OFFLINE === undefined`; telemetry `PI_TELEMETRY`; applyHttpProxySettings `??=`): empty string is **not** undefined. Inject an `Env` map (or trait) so parity cases control it explicitly; never read `std::env` directly in ported functions.
17. **`Promise` error propagation & abort semantics**:
   - `raceWithAbortSignal` (utils/abort.ts l14): rejects with the signal's `reason` (or `AbortError` named error, message `"The operation was aborted"`); the abandoned operation keeps running and its rejection is observed (not unhandled).
   - `enqueueCredentialOperation` (model-runtime l579): per-provider chaining where the tail swallows errors (`operation.catch(() => {})`) and the returned promise races "started" against abort — the operation still runs to completion after abort.
   - `registerProvider`/`unregisterProvider`/`registerVirtualModel` fire `void this.refresh({allowNetwork: false})` — **unawaited background refresh** whose errors vanish. In a sync Rust port this becomes an explicit queued refresh the harness can drive deterministically.
   - `getApiKeyAndHeaders` (model-registry l106–117) inspects `error.cause` — Rust error type needs a cause chain mirroring JS `new Error(msg, {cause})`.
18. **Special-cased error strings** (parity-critical):
   - `"authHeader requires a resolved API key"` → remapped to `` `No API key found for "${model.provider}"` `` (model-registry l113, thrown by provider-composer l382).
   - `CredentialSynchronizationError` message/name fields.
   - `ModelsError("provider"|"auth", ...)` codes.
19. **Regex ports**: `/\/v1\/?$/u` (model-runtime l281 — strip trailing `/v1` or `/v1/`), `/-\d{8}$/` (isAlias), `/^[A-Za-z_][A-Za-z0-9_]*$/` and `/^[A-Za-z_][A-Za-z0-9_]*/` (resolve-config-value env-var names). `\d` in JS regex = `[0-9]` only (no `u`-flag differences here). Port as hand-written string scanners, not regex crates, to avoid Unicode-class divergence.
20. **Set/Map iteration order = insertion order** (model-resolver l253 `new Set(candidates.map(provider))`, model-runtime l287 `providerIds()`, model-registry l241 dedup): determines composition and snapshot order. Use `IndexSet`/`IndexMap` (or Vec+dedup) — **never** `HashSet`/`HashMap`.
21. **`structuredClone` + `deepFreeze`** (model-config l139, models-store, auth-storage): plain-JSON data → `Clone` is fine; note `structuredClone` preserves `undefined`-valued keys (unlike JSON round-trip), which matters for objects never serialized — in practice all parity-visible data is JSON.
22. **Module-level shared mutable state** (auth-storage l51 `sharedAuthFileReadState`, models-store l23, resolve-config-value `commandResultCache`, http-dispatcher fetch globals): the **TS harness runs many cases in one process** (`--batch`), so this state **leaks between cases** on the TS side while a fresh Rust process per case (or fresh state) does not. Parity cases must either (a) construct stores with unique paths, (b) use `AuthStorage::fromStorage`/`inMemory` (no path → no shared state), or (c) replicate the sharing. Document this in the harness runner.
23. **`Math.random()` / wall-clock backoff** (auth-storage l145): non-deterministic by design; keep out of parity scope (InMemory backend covers the state machine). `Date.now()` busy-wait in `acquireLockSyncWithRetry` likewise.
24. **Windows-specific branches** (EPERM retries, platform checks): parity harness runs on Linux; implement the branches but gate tests.
25. **`\0` separator keys** (model-runtime l417 `` `${provider}\0${id}` ``, remote-catalog `mergeModels`): NUL is a valid JS string char; keep as a byte separator in Rust composition keys (provider/model ids can theoretically contain `\0`? — JS strings can; JSON can encode `\u0000`. Harness cases should pin this).
26. **`Object.values(parsed)` / `"models" in value`** (remote-catalog parseCatalog): `in` operator sees prototype chain for JSON data — irrelevant for parsed JSON but relevant if the harness feeds hand-built objects; keep to JSON inputs.
27. **Falsy checks on numbers** (model-config `definition.contextWindow !== undefined && definition.contextWindow <= 0`): `0` is invalid (throws), `undefined` is default — but note `NaN`? JSON can't encode NaN; typebox `exclusiveMinimum: 0` rejects `0` and negatives, and rejects non-numbers. Validation happens **after** these manual guards in `modelFromJson` — order matters for which error message wins.
28. **String `includes` / `indexOf` / `lastIndexOf` / `substring`** on UTF-16: model/provider ids with astral characters (emoji) split differently in UTF-16 vs Rust UTF-8 chars. `lastIndexOf(":")` on an id containing an astral char before the colon is index-consistent in both only if we operate on code units. Use `js_length`/UTF-16-aware helpers (pi-tui utils, surrogate sentinels) for id manipulation; safest is to operate on `Vec<u16>` or use the existing js_string module in pi-core.

---

## 3. Pure-logic vs I/O split, and the mock boundary

### 3.1 Deterministically parity-testable (pure, given injected inputs)

| Unit | Function(s) | Inputs to inject |
|---|---|---|
| http-dispatcher | `parseHttpIdleTimeoutMs`, `formatHttpIdleTimeoutMs` | raw JSON value |
| auth-guidance | all 4 exports | `docsPath` string |
| provider-attribution | `mergeProviderAttributionHeaders` (+ host matchers) | model object, `telemetryEnabled: bool` (replacing `SettingsManager`+env), sessionId, header sources |
| model-config | `ModelConfig::load` **minus fs** — i.e. `parse_and_validate(content: &str, path: &str) -> ModelConfig`, plus `getProvider*`/`getError` | file content string, path label; error strings for ENOENT/IO are produced by a `readFile` trait |
| provider-composer | `mergeCompat`, `mergeInputLimits`, `mergeSamplingParamsByThinkingLevel`, `applyModelOverride`, `modelFromJson`, `findModelDefaults`, `findExtensionModelDefaults`, `extensionModelFromDefinition`, `applyModelsJson`, `applyExtension`, `rawModelHeaders`, `resolveConfiguredModelHeaders`, `resolveCompatibilityRequestConfig`, `configuredRequestAuthStatus`, `validateExtensionProvider`, and the **model-list half** of `composeModelProvider` (`getAllModels()` evaluation) | models.json config object, extension config object, base provider model list |
| provider-composer (semi) | `composeApiKeyAuth.check/resolve`, `composeOAuthAuth.toAuth`, `withConfiguredAuth` | `ctx.env(name)` as an injected `Fn(&str) -> Option<String>`; `resolveConfigValue` with injected env + fake command runner |
| model-resolver | everything except the `modelRuntime`-taking wrappers: `resolveModelReference`, `findExactModelReferenceMatch`, `resolveModelScopeFromModels`, `resolveCliModel` (runtime access reduced to `{getModels(), hasConfiguredAuth()}` snapshot), `findInitialModel`, `restoreModelFromSession` (same reduction) | model list (JSON), `usable` predicate, saved defaults |
| auth-storage | `ReadOnlyAuthStorage::load` validation given content; `AuthStorage` read/modify/delete/list over `InMemoryAuthStorageBackend`; `readStoredCredential` given content+path | backend trait fake, file content |
| models-store | `InMemoryCodingAgentModelsStore`; `FileModelsStore` over a fake backend; `parse` | content strings |
| runtime-credentials | all methods | fake `CredentialStore` |
| virtual-models | `isVirtualModel`, `findLatestResponse`, `getBranchSelection`, `getVirtualModelState`, `createVirtualModel` | message/entry JSON |
| resolve-config-value (dependency) | `parseConfigValue*`, `getConfigValueEnvVarName(s)`, `getMissingConfigValueEnvVarNames`, `isCommandConfigValue`, `isConfigValueConfigured`, `resolveConfigValue` | env map, fake shell runner |
| remote-catalog-provider (semi) | `parseCatalog`, `readCatalog` (given header map + body JSON), `isNewer`, `mergeModels` | canned response values |

### 3.2 I/O-bound (behind traits; parity via fakes)

| Concern | TS location | Proposed trait seam |
|---|---|---|
| HTTP fetch + retry + timeouts | `fetchWithRetry` (utils/management-http.ts), used by remote-catalog; eventually OAuth + streaming | `trait HttpDispatcher { fn fetch(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>; }` with `HttpRequest {url, method, headers, body, timeout_ms}` and `HttpResponse {status, headers, body}` — **sync** for parity; fake impl returns scripted responses keyed by URL (incl. `last-modified`/`etag`). Real impl (reqwest/hyper) comes with the async phase. Mirrors `BashOperations` (pi-tools/shell.rs l528). |
| Filesystem | `readFile`/`writeFileSync`/`existsSync`/`mkdirSync`/stat-for-revision, `proper-lockfile` | `trait FsOperations` — reuse/extend `pi-tools::node_fs`; add `file_revision(path) -> Option<String>` (dev:ino:size:mtimeNs:ctimeNs) and `write_private(path, contents)` (mode 0o600, parent 0o700). Fake impl = in-memory map. |
| Locking | proper-lockfile lock/unlock, stale detection, EPERM retry | `trait LockOperations { fn lock(&self, path) -> Result<LockGuard, LockError>; }` — fake is a mutex + scripted error sequence; a real impl creates `<path>.lock` dirs compatible with proper-lockfile for TS interop. |
| Env | `process.env` reads (PI_OFFLINE, PI_TELEMETRY, HTTP(S)_PROXY writes, resolveConfigValue fallback) | `trait EnvOperations` or plain `&Env` struct passed down; fake = `BTreeMap` (case-sensitive on Linux, matching Node). |
| Shell commands | `resolveConfigValue` `!command` execution (spawnSync 10 s timeout, stdout trim, cached) | `trait CommandRunner { fn run(&self, command: &str, timeout_ms: u64) -> Option<String>; }` — same shape as the existing BashOperations seam; fake returns scripted values including "empty stdout → undefined". |
| Process-global undici config | `configureHttpDispatcher`, `applyHttpProxySettings` | Not parity-tested; ported as builder config for the real HTTP client. |
| Console output | `chalk` warnings in resolveModelScope/restoreModelFromSession | collect `diagnostics` (already returned) — print layer separate; parity harness compares diagnostics, not chalk escapes. |

### 3.3 Not parity-testable (defer)

- Live OAuth login flows (`adaptOAuth` callback plumbing), real streaming (`lazyStream` + SSE), availability refresh against real credentials, undici/proxy installation, background `void refresh()` timing.

---

## 4. Rust module placement and async story

### 4.1 Crate & module layout (recommended)

**New crate `pi-providers`** (the layer is ~4,400 LOC TS and cohesive; keeping it out of pi-core avoids bloating the base crate; it depends on `pi-core` for JS helpers/types and reuses `pi-tools` seams).

```
crates/pi-providers/src/
  lib.rs                  // re-exports
  js_json.rs              // (or reuse pi-core) JSON.stringify(,2)/parse-error helpers if not shared already
  config_value.rs         // resolve-config-value.ts  (~340 LOC Rust)
  http_config.rs          // http-dispatcher.ts parse/format (~90 LOC)
  auth_guidance.rs        // auth-guidance.ts (~40 LOC)
  provider_attribution.rs // provider-attribution.ts (~110 LOC)
  model_config.rs         // model-config.ts: types + validation + ModelConfig (~380 LOC)
  provider_composer.rs    // provider-composer.ts (~850 LOC)
  model_resolver.rs       // model-resolver.ts (~780 LOC)
  auth_storage.rs         // auth-storage.ts: AuthStorageBackend trait + fakes + AuthStorage (~600 LOC)
  models_store.rs         // models-store.ts (~170 LOC)
  runtime_credentials.rs  // runtime-credentials.ts (~70 LOC)
  virtual_models.rs       // virtual-models.ts pure fns (~220 LOC)
  remote_catalog.rs       // remote-catalog-provider.ts parse/merge + HttpDispatcher trait (~300 LOC)
  model_runtime.rs        // model-runtime.ts (phase 3c; ~1,100 LOC)
  model_registry.rs       // model-registry.ts facade (~250 LOC)
  pi_ai/                  // minimal pi-ai type surface (see §6): mod.rs, model.rs, provider.rs,
                          // credential.rs, models.rs (MutableModels), stream.rs (types only)
```

**Shared pi-ai surface** (submodule `pi_ai` or a future `pi-ai` crate): `Model` (as `serde_json::Value`-backed struct or typed struct with `#[serde(flatten)]` extra fields — prefer typed struct + `extra: Map` for unknown fields, mirroring how `messages.rs` round-trips), `AnyModel`, `Provider` (trait-object or struct with boxed closures/`Arc<dyn ...>` for stream/filter hooks), `Credential` (api_key/oauth tagged enum), `CredentialStore` trait, `AuthResult`/`AuthCheck`/`AuthStatus`, `ProviderHeaders = Map<String, String>` (ordered), `modelsAreEqual`, `isModelType`/`getModelType`. The builtin provider catalog (`providers/all`) is **data** — vendor the generated model list as a JSON fixture loaded by the fake/parity path; do not hand-port 40 providers.

### 4.2 Pub API sketch (parity-relevant subset)

```rust
// http_config.rs
pub const DEFAULT_HTTP_IDLE_TIMEOUT_MS: u64;
pub fn parse_http_idle_timeout_ms(value: &Value) -> Option<u64>;        // undefined = invalid
pub fn format_http_idle_timeout_ms(timeout_ms: f64) -> String;

// auth_guidance.rs
pub fn get_provider_login_help(docs_path: &str) -> String;
pub fn format_no_models_available_message(docs_path: &str) -> String;
pub fn format_no_model_selected_message(docs_path: &str) -> String;
pub fn format_no_api_key_found_message(docs_path: &str, provider: &str) -> String;

// provider_attribution.rs
pub fn merge_provider_attribution_headers(
    model: &Model, telemetry_enabled: bool, session_id: Option<&str>,
    header_sources: &[Option<ProviderHeaders>],
) -> Option<ProviderHeaders>;

// model_config.rs
pub struct ModelConfig { /* providers: IndexMap<String, ModelsJsonProvider>, error: Option<String> */ }
impl ModelConfig {
    pub fn from_content(path: &str, content: Option<&str>, io_error: Option<&str>) -> ModelConfig;
    pub fn get_provider(&self, id: &str) -> Option<&ModelsJsonProvider>;
    pub fn get_provider_ids(&self) -> Vec<String>;
    pub fn get_error(&self) -> Option<&str>;
}

// provider_composer.rs
pub fn apply_models_json(provider_id: &str, base_models: &[AnyModel], config: Option<&ModelsJsonProvider>) -> Result<Vec<AnyModel>, JsError>;
pub fn apply_model_override(model: &Model, ovr: &ModelsJsonModelOverride) -> Model;
pub fn compose_model_provider(provider_id: &str, base: Option<&Provider>, config: &ModelConfig, extension: Option<&ProviderConfigInput>) -> Result<Provider, JsError>;
pub fn resolve_configured_model_headers(model: &AnyModel, config: Option<&ModelsJsonProvider>, extension: Option<&ProviderConfigInput>, env: Option<&Env>) -> Result<Option<ProviderHeaders>, JsError>;
pub fn resolve_compatibility_request_config(...) -> CompatibilityRequestConfig;
pub fn configured_request_auth_status(config: Option<&ModelsJsonProvider>, extension: Option<&ProviderConfigInput>) -> Option<AuthStatus>;
pub fn validate_extension_provider(...) -> Result<(), JsError>;

// model_resolver.rs
pub fn resolve_model_reference(request: &ModelReferenceRequest, models: &[Model], usable: &dyn Fn(&str) -> bool) -> ModelReferenceResult;
pub fn find_exact_model_reference_match(reference: &str, models: &[Model]) -> Option<Model>;
pub fn resolve_model_scope_from_models(patterns: &[String], models: &[Model]) -> ResolveModelScopeResult;
pub fn resolve_cli_model(options: &ResolveCliModelOptions) -> ResolveCliModelResult; // options carry model snapshot fns
pub fn find_initial_model(options: &FindInitialModelOptions) -> InitialModelResult;
pub fn restore_model_from_session(...) -> RestoreModelResult; // diagnostics returned, printing separated

// auth_storage.rs
pub trait AuthStorageBackend { fn with_lock(&self, f: &mut dyn FnMut(Option<&str>) -> LockResult) -> Result<Value, JsError>; }
pub struct AuthStorage { .. } // create/from_storage/in_memory; read/modify/delete/list with injected FsOperations
pub struct ReadOnlyAuthStorage { .. }
pub fn read_stored_credential(provider_id: &str, content: Option<&str>) -> Option<Credential>;

// runtime_credentials.rs
pub struct RuntimeCredentials<'a> { store: &'a dyn CredentialStore, overrides: IndexMap<String, String> }

// config_value.rs
pub fn resolve_config_value(config: &str, env: Option<&Env>, runner: &dyn CommandRunner) -> Option<String>;
pub fn get_config_value_env_var_names(config: &str) -> Vec<String>;
pub fn is_command_config_value(config: &str) -> bool;
pub fn is_config_value_configured(config: &str, env: Option<&Env>) -> bool;
```

`JsError` must carry `message: String` and an optional `cause: Box<JsError>` (mirrors `new Error(msg, {cause})` inspection in model-registry).

### 4.3 Async story

**Tokio is NOT required for phase 3 parity.** Recommendation:

1. **Parity layer stays sync.** All composition/resolution/parsing logic (§3.1) is written as plain sync functions. Async-shaped TS constructs map as follows:
   - `Promise`-returning functions whose body is pure (`ModelConfig.load`, `resolveModelScopeWithDiagnostics`, `getApiKeyAndHeaders`) → sync functions taking injected content/env.
   - Concurrency state machines (`availability refresh`, `enqueueCredentialOperation`, single-flight reloads) → **explicit state machines** with an injectable "run pending refreshes" step the harness drives; the sequence-number guards (`availabilityRefreshSeq` etc.) port directly as integers.
   - `AbortSignal` → reuse the existing `AbortSignal(Arc<AtomicBool>)` from pi-tools/shell.rs (l37) plus a reason field; `raceWithAbortSignal` becomes a check-and-return-error helper at the sync boundary.
   - `lazyStream` / `AssistantMessageEventStream` → define **types only** in phase 3 (an enum/iterator design), with `stream`/`complete` methods returning a `Result` in the parity harness driving a scripted provider. Real streaming is phase 4.
2. **`HttpDispatcher` trait is sync** (`fn fetch(...) -> Result<HttpResponse, HttpError>`) for parity. When real networking lands (remote catalog refresh, OAuth device flow, SSE streaming), introduce a *separate* async-capable impl behind a feature flag; the leading candidate remains tokio (per repo conventions: not yet decided). Design the trait now so an async twin can wrap it: keep `HttpRequest`/`HttpResponse` as plain data with no lifetimes borrowed from the trait object.
3. **Background `void refresh()` calls** become a returned `PendingRefresh` handle (or an internal queue) so the deterministic harness can choose to run or drop them — matching TS observable behavior (errors vanish, snapshot eventually updates) only where a test depends on it.

---

## 5. Parity harness entries

Follow existing conventions: TS side `parity/ts-harness.js` `runModule(tool, args, cwd)` with `args.fn` sub-dispatch (see `"messages"` entry); Rust side `crates/pi-cli/src/main.rs` `run_tool` match arm (e.g. `"messages" => run_messages_harness(&input)`) with per-case panics converted to in-band `{error}`; cases in `parity/cases.mjs` as `{name, tool, args}`. All entries below are module-level (not AgentTools). Errors that TS throws must surface as in-band `{error: "<message>"}` — the Rust side returns `Err(String)` from the harness fn.

Proposed tool names and shapes (args in, result out):

### `http-dispatcher`
- `{fn: "parseHttpIdleTimeoutMs", value: <json>}` → `{result: <number|null>}` (undefined → null)
- `{fn: "formatHttpIdleTimeoutMs", timeoutMs: 1500}` → `{result: "1.5 sec"}`

### `auth-guidance`
- `{fn: "getProviderLoginHelp" | "formatNoModelsAvailableMessage" | "formatNoModelSelectedMessage" | "formatNoApiKeyFoundMessage", provider?: "x", docsPath: "/docs"}` → `{result: "<string>"}`
  (TS harness injects `docsPath` by overriding `getDocsPath` via a env-var-driven hook or by parameterizing the module import — simplest: reimplement the join in the harness and pass the composed path; prefer adding a tiny wrapper in ts-harness.js that monkey-patches `config.getDocsPath` before import if needed. Decide at implementation time; the Rust side takes `docs_path` directly.)

### `provider-attribution`
- `{fn: "mergeProviderAttributionHeaders", model: {provider, baseUrl}, telemetryEnabled: true|false, sessionId?: "s1", headerSources: [<headers|null>, ...]}` → `{result: <headers|null>}`
  TS side: call the real function with a stub `settingsManager` (`{getEnableInstallTelemetry: () => args.telemetryEnabled}`) and `process.env.PI_TELEMETRY` scrubbed (`delete process.env.PI_TELEMETRY` before the call to keep `isInstallTelemetryEnabled` deterministic).

### `model-config`
- `{fn: "load", path: "/models.json", content: "<string|null>", ioError: "<node message>|null"}` → `{result: {providers: {<id>: {...}}, providerIds: [...], error: <string|null>}}`
  (harness performs the file access — reading `content` from a fixture dir keyed by `path`, or from an inline string — so both sides see identical bytes; ENOENT modeled as `content: null`.)
- `{fn: "getProvider", id: "x"}` on a loaded config is covered by returning the full map above; keep `load` single-shot per case.

### `provider-composer`
- `{fn: "applyModelsJson", providerId, baseModels: [...], config: <provider-config|null>}` → `{result: {models: [...]}}` or `{error}`
- `{fn: "applyModelOverride", model, override}` → `{result: {model}}`
- `{fn: "modelFromJson", providerId, definition, providerConfig, defaults: <model|null>}` → `{result: {model}}` or `{error}`
- `{fn: "composeModelProvider", providerId, baseProviderModels: [...] | null, baseAuth: "apiKey"|"oauth"|"both"|null, config: <provider-config|null>, extension: <provider-config-input|null>}` → `{result: {models: [...], name, baseUrl, hasApiKey, hasOauth}}` or `{error}` (stream/auth closures are not serialized; only the credential-blind composition surface is compared)
- `{fn: "resolveConfiguredModelHeaders", model, config, extension, env?}` → `{result: <headers|null>}` or `{error}`
- `{fn: "resolveCompatibilityRequestConfig", model, config, extension}` → `{result: {headers, authHeader}}`
- `{fn: "configuredRequestAuthStatus", config, extension}` → `{result: <auth-status|null>}`
- `{fn: "validateExtensionProvider", providerId, baseProviderModels, config, extension}` → `{result: true}` or `{error}`

### `model-resolver`
- `{fn: "resolveModelReference", text, provider?, thinkingSuffix, fuzzy, models: [...], usable: [<provider-ids>]}` → `{result: {status, ...}}` (result is the discriminated union; TS side passes `usable = (p) => args.usable.includes(p)`)
- `{fn: "findExactModelReferenceMatch", modelReference, models}` → `{result: <model|null>}`
- `{fn: "resolveModelScopeFromModels", patterns: [...], models: [...]}` → `{result: {scopedModels, diagnostics}}`
- `{fn: "resolveCliModel", cliProvider?, cliModel?, cliThinking?, inlineApiKey?, models: [...], configuredAuth: [...]}` → `{result: {model?, thinkingLevel?, warning?, error?}}`
- `{fn: "findInitialModel", scopedModels: [...], isContinuing, defaultProvider?, defaultModelId?, defaultThinkingLevel?, modelThinkingLevels?, availableModels: [...], configuredAuth: [...]}` → `{result: {model, thinkingLevel, fallbackMessage}}`
- `{fn: "restoreModelFromSession", savedProvider, savedModelId, currentModel?, models, availableModels, configuredAuth: [...], hasRestoredAuth: true|false}` → `{result: {model, fallbackMessage}}` (printing suppressed; `shouldPrintMessages: false` on TS side)

### `auth-storage`
- `{fn: "readOnlyLoad", content: "<string|null>", readError: "<msg>|null"}` → `{result: {data}}` or `{error: "<message>"}`
- `{fn: "readOnlyRead", content, providerId}` → `{result: <credential|null>}` (with `resolveConfigValue` env: pass `env: {...}`; command values use a `command` fixture runner in the harness)
- `{fn: "storageRoundtrip", steps: [{op: "write"|"modify"|"delete"|"read"|"list", providerId?, entry?, modifyResult?}, ...]}` → `{result: {finalContent: "<json string>", reads: [...], lists: [...]}}` over `AuthStorage::in_memory` / fake backend — TS side uses `AuthStorage.fromStorage(new InMemoryAuthStorageBackend())`
- `{fn: "readStoredCredential", content, providerId}` → `{result: <credential|null>}`

### `models-store`
- `{fn: "roundtrip", steps: [...]}` same shape as `auth-storage/storageRoundtrip` (in-memory store)

### `runtime-credentials`
- `{fn: "overlay", backing: {<providerId>: <credential>}, steps: [{op: "set"|"remove"|"has"|"read"|"list"|"delete", ...}]}` → `{result: {reads: [...], lists: [...]}}`

### `virtual-models`
- `{fn: "isVirtualModel", model}` → `{result: bool}`
- `{fn: "findLatestResponse", messages: [...]}` → `{result: <assistant-message|null>}`
- `{fn: "getBranchSelection", branch: [...], models: [...]}` → `{result: {provider, modelId}|null}`
- `{fn: "getVirtualModelState", branch, provider, modelId}` → `{result: <state|null>}`
- `{fn: "createVirtualModel", definition}` → `{result: {model}}`

### `config-value`
- `{fn: "resolveConfigValue", config, env: {...}, commands: {"<cmd>": "<stdout>"}}` → `{result: <string|null>}`
- `{fn: "getConfigValueEnvVarNames" | "getMissingConfigValueEnvVarNames" | "isCommandConfigValue" | "isConfigValueConfigured", config, env?}` → `{result: ...}`

### `remote-catalog` (parse-only)
- `{fn: "parseCatalog", providerId, value: <json>}` → `{result: {models}}` or `{error}`
- `{fn: "readCatalog", providerId, lastModified: "<http-date>"|null, etag?, body: <json>}` → `{result: {models, lastModified, etag}}`
- `{fn: "mergeModels", baseline: [...], dynamic: [...]}` → `{result: {models}}`

TS-side harness notes: several modules (`provider-composer`, `model-config`, `auth-storage`, `virtual-models`) must be imported from the **dist build** (existing `PI_DIST` mechanism) — verify they are exported from the package index (`src/index.ts` re-exports `core/index.ts`; check and, if missing, import the dist module files directly the way `tui-utils` imports TS source).

---

## 6. Dependencies on other unported modules & porting order

### Dependency graph (provider layer → prerequisites)

| Dependency | Status in pi-rs | Notes |
|---|---|---|
| `resolve-config-value.ts` (233 LOC) | **unported** | Must be ported first; everything in auth/composer depends on it. Needs `getShellConfig` (pi-tools/shell.rs already has `ShellConfig`) and an injectable command runner. |
| `utils/json.ts` `stripJsonComments` | check pi-core (likely unported) | ~40 LOC; port with fixtures (comment stripping semantics: `//`, `/* */`, strings must be preserved). |
| `utils/text.ts` `stripBom` | **ported** (`pi-core/src/text.rs`) | reuse. |
| `utils/paths.ts` `normalizePath`, `getFileRevision` | `normalize_path` ported; `getFileRevision` **unported** | add `file_revision` (bigint stat string). |
| `utils/abort.ts` `operationSignal`, `raceWithAbortSignal` | pi-tools `AbortSignal` exists | port the reason/`AbortError` semantics for error-string parity. |
| `config.ts` `getAgentDir`/`getDocsPath`/`VERSION` | `get_agent_dir` ported (pi-session/config.rs); `getDocsPath` **unported** | add `get_docs_path`/`get_models_path`/`get_auth_path`. |
| `core/telemetry.ts` `isInstallTelemetryEnabled` | **unported** (needs SettingsManager) | For parity, replace with an injected `telemetry_enabled: bool`; port properly with settings-manager phase. |
| `core/settings-manager.ts` | **unported** (46 K) | Only touched via telemetry flag in this layer — defer. |
| `cli/args.ts` `isValidThinkingLevel` | **unported** | tiny (list membership over off/minimal/low/medium/high/xhigh/max). |
| `core/defaults.ts` `DEFAULT_THINKING_LEVEL` | **unported** | one-liner (check value in source). |
| pi-ai type surface + `createModels`/`MutableModels`/`modelsAreEqual`/`isModelType`/`clampThinkingLevel`/`lazyStream`/error-result helpers | **unported** (pi-core/ai.rs has Usage/uuidv7/contentText only) | Port the minimal surface (§4.1). `MutableModels` full behavior (availability filtering, login orchestration) is a later phase; parity cases supply model lists directly. |
| pi-ai builtin provider catalog + radius + openrouter personalization | **unported** | vendor as JSON fixture data for tests; real port is data-entry work. |
| `session-manager.ts` (`SessionEntry`) | **ported** (pi-session) | virtual-models `getBranchSelection` reuses its entry types. |
| `agent-session.ts`, `mcp-servers.ts`, `extensions/` | **unported** | consumers of this layer; not blockers. |
| `utils/management-http.ts` `fetchWithRetry`, `utils/pi-user-agent.ts` | **unported** | behind `HttpDispatcher` trait; defer real impl. |

### Recommended porting order (incremental parity at every step)

1. `config-value` (resolve-config-value) → parity green.
2. `http-dispatcher` parse/format + `auth-guidance` → parity green (trivial, builds harness muscle).
3. `provider-attribution` (needs injected telemetry flag) → parity green.
4. pi-ai minimal type surface (`Model`, `AnyModel`, headers, `modelsAreEqual`, `isModelType`) + `model-config` (needs `stripJsonComments`, typebox-error fixture strategy) → parity green.
5. `provider-composer` pure half (apply*/merge*/modelFromJson/rawModelHeaders/resolve*/configuredRequestAuthStatus/validate) → parity green; then the auth-composition closures with fake env/command runner.
6. `model-resolver` (all pure fns; needs minimatch-compatible glob + `isValidThinkingLevel` + `DEFAULT_THINKING_LEVEL`) → parity green. **Highest case count; budget the most fixtures here.**
7. `auth-storage` + `models-store` + `runtime-credentials` (backend trait + fakes; `getFileRevision` injection) → parity green.
8. `virtual-models` pure fns → parity green.
9. `remote-catalog` parse/merge (+ `HttpDispatcher` trait + fake) → parity green.
10. `model-runtime` + `model-registry` (state machine with injected stores/fixtures; sync refresh steps) → parity green for composition/snapshot/registration behavior.
11. (Phase 4 bridge) real `HttpDispatcher`, real fs/lock backends, streaming types → integration parity, not harness parity.

---

## 7. Step-by-step porting checklist (with LOC estimates)

Rust LOC estimates assume the repo's style (explicit JS-semantics helpers, fixture-heavy tests).

| # | Step | Files touched | Est. LOC (impl) | Est. parity cases |
|---|---|---|---|---|
| 1 | Scaffold `pi-providers` crate; move nothing yet; add to workspace | `Cargo.toml`, `crates/pi-providers/*` | 30 | — |
| 2 | Port `resolve-config-value.ts`: template parser, env-var name fns, `resolveConfigValue` with `CommandRunner` trait (+ fake); port `getShellConfig`-shaped option | `config_value.rs`, `pi-tools/shell.rs` (reuse) | 340 | 25–35 |
| 3 | Port `http-dispatcher.ts` parse/format (+ `js_to_number`, js trim reuse); `auth-guidance.ts`; add `get_docs_path` to pi-session/config.rs | `http_config.rs`, `auth_guidance.rs` | 140 | 15 |
| 4 | Port `provider-attribution.ts` with `telemetry_enabled: bool` injection | `provider_attribution.rs` | 120 | 20 |
| 5 | pi-ai minimal surface: `Model`/`AnyModel`/`ProviderHeaders`/`Credential`/`modelsAreEqual`/`isModelType` (typed structs w/ extra-field round-trip); port `stripJsonComments` | `pi_ai/`, `pi-core` (json helper) | 350 | (covered by later steps) |
| 6 | Port `model-config.ts`: schemas as hand-written validators producing typebox-identical messages (fixture-driven), `ModelConfig::from_content`, error strings | `model_config.rs` | 380 | 30–40 |
| 7 | Port provider-composer pure half: merge fns, `modelFromJson`, `findModelDefaults`, `applyModelsJson`, `applyExtension`, `rawModelHeaders`, `resolve*`, `configuredRequestAuthStatus`, `validateExtensionProvider` | `provider_composer.rs` | 550 | 60–80 |
| 8 | Port provider-composer auth half: `withConfiguredAuth`, `composeApiKeyAuth`/`composeOAuthAuth` closures (env-injecting `AuthContext`), `composeModelProvider` model-list + name/baseUrl surface | `provider_composer.rs` | 300 | 25 |
| 9 | Port `model-resolver.ts` pure fns; vendor minimatch-compatible matcher; `isValidThinkingLevel`, `DEFAULT_THINKING_LEVEL`; diagnostics separated from printing | `model_resolver.rs`, `glob.rs` (vendored) | 900 | 80–120 |
| 10 | Port `auth-storage.ts`: backend trait, InMemory + ReadOnly + AuthStorage state machine (sync refresh model), `readStoredCredential`, `file_revision` helper | `auth_storage.rs`, `pi-tools/node_fs.rs` | 620 | 40–50 |
| 11 | Port `models-store.ts` + `runtime-credentials.ts` | `models_store.rs`, `runtime_credentials.rs` | 240 | 20 |
| 12 | Port `virtual-models.ts` pure fns | `virtual_models.rs` | 230 | 20 |
| 13 | Port `remote-catalog-provider.ts` parse/merge + `HttpDispatcher` trait + fake | `remote_catalog.rs` | 320 | 20 |
| 14 | Port `model-runtime.ts` + `model-registry.ts`: snapshot state machine, composition-error map, registration semantics, `getAuth` header merge, `getApiKeyAndHeaders` incl. cause-chain remap; sync refresh driver | `model_runtime.rs`, `model_registry.rs` | 1,150 | 40–60 |
| 15 | TS harness work: add all `runModule` entries above to `parity/ts-harness.js` (dist imports, settings stub, env scrubbing, in-memory backends); add all `run_*_harness` arms to `pi-cli/main.rs`; add case groups to `parity/cases.mjs` | `parity/*`, `pi-cli/main.rs` | 700 (TS+glue) | — |
| 16 | Docs + divergence log: record localeCompare/minimatch/JSON-error-string divergences and fixture strategy | `docs/` | 100 | — |

**Totals**: ~6,100 LOC Rust implementation, ~400–550 parity cases, plus harness glue. Steps 2–9 deliver the credential-blind core (the bulk of parity value) before any async decision is forced; step 14 can be trimmed if `model-runtime` is deferred behind phase 4's async decision.

### Definition of done (per step)
- `node parity/run-parity.mjs` green for the new group with zero filtered cases.
- Divergences (if any) documented in the case file with a comment naming the JS semantics involved.
- No direct `std::env`/file/HTTP access outside the injected traits.
