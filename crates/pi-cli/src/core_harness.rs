//! Parity-harness entries for the landed core-module wave:
//! `event-bus`, `timings`, `slash-commands`, `settings-defaults`,
//! `settings-schema`.
//!
//! Wire protocol (TS mirror contract in parity/ts-harness.js): JSON args on
//! stdin, `{"result": ...}` on stdout; per-case exceptions surface in-band as
//! `{"error": msg}` (thrown on the TS side, `Err` here) with identical
//! messages, so protocol-misuse cases compare equal.
//!
//! Determinism: no ambient time/randomness reaches these modules — the
//! event-bus error sink, the timings clock, and the timings console stream are
//! all injected/captured on both sides.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

pub fn run(tool: &str, input: &Value) -> Result<Value, String> {
    match tool {
        "event-bus" => run_event_bus_harness(input),
        "timings" => run_timings_harness(input),
        "slash-commands" => run_slash_commands_harness(input),
        "settings-defaults" => run_settings_defaults_harness(input),
        "settings-schema" => run_settings_schema_harness(input),
        "settings-manager" => run_settings_manager_harness(input),
        "prompt-templates" => run_prompt_templates_harness(input),
        "skills" => run_skills_harness(input),
        "sess-manager" => run_sess_manager_harness(input),
        "nested-tool-calls" => run_nested_tool_calls_harness(input),
        "pi-manifest" => run_pi_manifest_harness(input),
        "defaults" => run_defaults_harness(input),
        "output-guard" => run_output_guard_harness(input),
        other => Err(format!("core_harness: unknown tool {other:?}")),
    }
}

// ============================================================================
// event-bus
// ============================================================================

/// `event-bus` harness: script-driven `pi_core::event_bus` (TS
/// `core/event-bus.js`). args `{ ops: [...] }`, result
/// `{ log: [...], errors: [...] }`.
///
/// Ops (both sides identical):
/// - `{op:"on"|"once", channel, handler: <id>, throws?: <string>,
///    unsub?: [<id>...], clear?: true}` — register numbered handler. When
///   invoked the handler: (once: unsubscribe itself first, Node `once`
///   semantics), push `"<channel>:<JSON data>"` to the log, unsubscribe the
///   `unsub` handlers, optionally `clear()` the bus, then throw `throws` (a
///   string, so the captured report renders identically to the TS
///   `console.error(template, err)` join).
/// - `{op:"off", channel, handler?: <id>}` — unsubscribe one handler, or all
///   live handlers on `channel` when `handler` is absent.
/// - `{op:"emit", channel, data?: <json>}` — `bus.emit(channel, data ?? null)`
///   (the TS bus signature is single-data; multi-argument emitter emits are
///   not part of the ported surface).
/// - `{op:"clear"}` — `bus.clear()`.
///
/// Not ported (pi-core API gap, see report): `once` as a bus primitive (the
/// harness emulates it via self-unsubscribe), `off(channel)` without the
/// stored unsubscribe handle (emulated via per-handler unsubscribes), and
/// `listenerCount` (no observable probe through the bus interface).
fn run_event_bus_harness(input: &Value) -> Result<Value, String> {
    use pi_core::event_bus::{create_event_bus_with_sink, Subscription};

    struct Entry {
        channel: String,
        sub: Option<Subscription>,
    }
    type Live = Arc<Mutex<HashMap<u64, Entry>>>;

    fn off_id(live: &Live, id: u64) {
        let sub = live.lock().unwrap().get_mut(&id).and_then(|e| e.sub.take());
        if let Some(sub) = sub {
            sub.unsubscribe();
        }
    }

    let ops = input["ops"].as_array().ok_or("missing ops")?;
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_errors = Arc::clone(&errors);
    let bus = create_event_bus_with_sink(Arc::new(move |message: &str| {
        sink_errors.lock().unwrap().push(message.to_string());
    }));
    let live: Live = Arc::new(Mutex::new(HashMap::new()));

    for op in ops {
        let kind = op["op"].as_str().ok_or("missing op")?;
        match kind {
            "on" | "once" => {
                let channel = op["channel"].as_str().ok_or("missing channel")?.to_string();
                let id = op["handler"].as_u64().ok_or("missing handler")?;
                if live.lock().unwrap().contains_key(&id) {
                    return Err(format!("handler {id} already registered"));
                }
                if let Some(v) = op.get("throws") {
                    if !v.is_string() {
                        return Err("throws must be a string".to_string());
                    }
                }
                if let Some(v) = op.get("clear") {
                    if v.as_bool().is_none() {
                        return Err("clear must be a boolean".to_string());
                    }
                }
                let throws = op["throws"].as_str().map(str::to_string);
                let unsub: Vec<u64> = match op.get("unsub") {
                    None => Vec::new(),
                    Some(v) => v
                        .as_array()
                        .ok_or("unsub must be an array")?
                        .iter()
                        .map(|v| v.as_u64().ok_or("unsub entries must be ids"))
                        .collect::<Result<_, _>>()?,
                };
                let clear = op["clear"].as_bool().unwrap_or(false);
                let once = kind == "once";
                let handler_log = Arc::clone(&log);
                let handler_live = Arc::clone(&live);
                let handler_bus = bus.clone();
                let handler_channel = channel.clone();
                let sub = bus.on(&channel, move |data: &Value| {
                    if once {
                        off_id(&handler_live, id);
                    }
                    let rendered = serde_json::to_string(data).unwrap_or_else(|_| "null".to_string());
                    handler_log.lock().unwrap().push(format!("{handler_channel}:{rendered}"));
                    for target in &unsub {
                        off_id(&handler_live, *target);
                    }
                    if clear {
                        handler_bus.clear();
                    }
                    match &throws {
                        Some(message) => Err(message.clone()),
                        None => Ok(()),
                    }
                });
                live.lock().unwrap().insert(id, Entry { channel, sub: Some(sub) });
            }
            "off" => {
                let channel = op["channel"].as_str().ok_or("missing channel")?;
                match op.get("handler") {
                    None => {
                        let targets: Vec<u64> = live
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|(_, e)| e.channel == channel && e.sub.is_some())
                            .map(|(&id, _)| id)
                            .collect();
                        for id in targets {
                            off_id(&live, id);
                        }
                    }
                    Some(v) => {
                        let id = v.as_u64().ok_or("handler must be an id")?;
                        off_id(&live, id);
                    }
                }
            }
            "emit" => {
                let channel = op["channel"].as_str().ok_or("missing channel")?;
                let data = op.get("data").cloned().unwrap_or(Value::Null);
                bus.emit(channel, &data);
            }
            "clear" => bus.clear(),
            other => return Err(format!("unknown op: {other}")),
        }
    }

    let log = std::mem::take(&mut *log.lock().unwrap());
    let errors = std::mem::take(&mut *errors.lock().unwrap());
    Ok(json!({ "result": { "log": log, "errors": errors } }))
}

// ============================================================================
// timings
// ============================================================================

/// `timings` harness: script-driven `pi_core::timings` (TS `core/timings.js`)
/// with an injected clock. args `{ enabled?: bool, steps: [...] }`, result
/// `{ outputs: [<print output>...] }`.
///
/// The clock starts at 1000 and only moves on explicit steps (both sides):
/// - `{step:"reset", namespace?}` — `resetTimings` (default namespace "main")
/// - `{step:"time", label, namespace?}` — `time`
/// - `{step:"advance", ms}` — clock += ms (negative allowed)
/// - `{step:"set", ms}` — clock = ms
/// - `{step:"print"}` — run `printTimings` and capture the exact bytes the
///   TS `console.error` stream would have written (each call contributes
///   `message + "\n"`).
///
/// TS side: `PI_TIMING` is set before a cache-busting dynamic import (ENABLED
/// is captured at module load) and `Date.now` is monkeypatched to the same
/// injected clock.
fn run_timings_harness(input: &Value) -> Result<Value, String> {
    use pi_core::timings::{Timings, TimingLabel};

    fn parse_namespace(step: &Value) -> Result<TimingLabel, String> {
        match step["namespace"].as_str() {
            None | Some("main") => Ok(TimingLabel::Main),
            Some("extensions") => Ok(TimingLabel::Extensions),
            Some(other) => Err(format!("unknown namespace: {other}")),
        }
    }

    let steps: Vec<&Value> = match input.get("steps") {
        None => Vec::new(),
        Some(v) => v.as_array().ok_or("missing steps")?.iter().collect(),
    };
    let enabled = match input.get("enabled") {
        None => true,
        Some(v) => v.as_bool().ok_or("enabled must be a boolean")?,
    };
    let now = Arc::new(Mutex::new(1000i64));
    let clock_now = Arc::clone(&now);
    let mut timings = Timings::with_clock(
        enabled,
        Arc::new(move || *clock_now.lock().unwrap()),
    );
    let mut outputs: Vec<String> = Vec::new();

    for step in steps {
        let kind = step["step"].as_str();
        match kind {
            Some("reset") => timings.reset_timings(parse_namespace(step)?),
            Some("time") => {
                let label = step["label"].as_str().ok_or("missing label")?;
                timings.time(label, parse_namespace(step)?);
            }
            Some("advance") => {
                let ms = step["ms"].as_i64().ok_or("missing ms")?;
                *now.lock().unwrap() += ms;
            }
            Some("set") => {
                let ms = step["ms"].as_i64().ok_or("missing ms")?;
                *now.lock().unwrap() = ms;
            }
            Some("print") => outputs.push(timings.print_timings()),
            Some(other) => return Err(format!("unknown step: {other}")),
            None => return Err("unknown step: undefined".to_string()),
        }
    }

    Ok(json!({ "result": { "outputs": outputs } }))
}

// ============================================================================
// slash-commands
// ============================================================================

/// `slash-commands` harness (TS `core/slash-commands.js`).
///
/// - `{op:"list", app?}` → `{ commands: [...] }` — the 24 builtins. `app` is
///   accepted but ignored: the TS module bakes `APP_NAME` from package.json
///   at module load ("pi" in the shipped package) and it is not injectable,
///   so the Rust side always passes "pi" (the ground-truth package name).
/// - `{op:"roundtrip", kind:"builtin"|"info", value}` → `{ value }` — serde
///   round-trip through the ported types (the TS side returns the value
///   as-is), checking the camelCase field surface and optional-field
///   omission. Only schema-valid values (no unknown or null optional fields)
///   are comparable: the TS module performs no runtime validation.
fn run_slash_commands_harness(input: &Value) -> Result<Value, String> {
    use pi_core::slash_commands::{BuiltinSlashCommand, SlashCommandInfo, builtin_slash_commands};

    let op = input["op"].as_str().ok_or("missing op")?;
    match op {
        "list" => {
            let commands = builtin_slash_commands("pi");
            let commands = serde_json::to_value(&commands).map_err(|e| e.to_string())?;
            Ok(json!({ "result": { "commands": commands } }))
        }
        "roundtrip" => {
            let kind = input["kind"].as_str().ok_or("missing kind")?;
            let value = input.get("value").cloned().ok_or("missing value")?;
            let value = match kind {
                "builtin" => {
                    let parsed: BuiltinSlashCommand =
                        serde_json::from_value(value).map_err(|e| e.to_string())?;
                    serde_json::to_value(&parsed).map_err(|e| e.to_string())?
                }
                "info" => {
                    let parsed: SlashCommandInfo =
                        serde_json::from_value(value).map_err(|e| e.to_string())?;
                    serde_json::to_value(&parsed).map_err(|e| e.to_string())?
                }
                other => return Err(format!("unknown kind: {other}")),
            };
            Ok(json!({ "result": { "value": value } }))
        }
        other => Err(format!("unknown op: {other}")),
    }
}

// ============================================================================
// settings-defaults
// ============================================================================

/// `settings-defaults` harness (TS `core/settings-defaults.js`).
/// `{}` → `{ defaults: <fresh SETTINGS_DEFAULTS> }`;
/// `{calls: N}` → `{ defaults: [<N fresh copies>] }` (N must be a
/// non-negative integer; the TS side returns the same frozen object each
/// call, the Rust side a fresh `Value`).
fn run_settings_defaults_harness(input: &Value) -> Result<Value, String> {
    use pi_core::settings_defaults::settings_defaults;

    match input.get("calls") {
        None => Ok(json!({ "result": { "defaults": settings_defaults() } })),
        Some(v) => {
            let calls = v.as_u64().ok_or("calls must be a non-negative integer")?;
            let defaults: Vec<Value> = (0..calls).map(|_| settings_defaults()).collect();
            Ok(json!({ "result": { "defaults": defaults } }))
        }
    }
}

// ============================================================================
// settings-schema
// ============================================================================

/// `settings-schema` harness (TS `core/settings-schema.js`).
/// `{settings: <json>}` → `{ errors: [{path, instancePath, message}...] }`
/// from `pi_providers::settings_schema::validate_settings`.
///
/// TS mirror: `Compile(SettingsSchema).Errors(value)` mapped through the
/// `formatValidationPath` helper from `core/model-config.ts` (dotted path,
/// `required` errors point at the first missing property, empty base is
/// `root`) — the same rendering the Rust `format_path()` implements.
fn run_settings_schema_harness(input: &Value) -> Result<Value, String> {
    let settings = input.get("settings").ok_or("missing settings")?.clone();
    let errors: Vec<Value> = pi_providers::settings_schema::validate_settings(&settings)
        .iter()
        .map(|e| {
            json!({
                "path": e.format_path(),
                "instancePath": e.instance_path(),
                "message": e.message(),
            })
        })
        .collect();
    Ok(json!({ "result": { "errors": errors } }))
}

// ============================================================================
// settings-manager
// ============================================================================

/// `settings-manager` harness: script-driven `pi_core::settings_manager`
/// (TS `core/settings-manager.js`). args `{ mode, storage?, settings?,
/// options?, ops }`, result `{ out: [...] }` — one entry per op.
///
/// Modes: `"memory"` (in-memory storage preloaded from `storage` strings),
/// `"files"` (per-side temp dirs seeded from `storage`; error paths are
/// normalized to `<agentDir>`/`<cwd>` so both sides compare equal),
/// `"inMemory"` (the `inMemory` factory over `settings`).
///
/// Determinism: `options.env` replaces the ambient env (VISUAL/EDITOR,
/// PI_CLEAR_ON_SHRINK, PI_HARDWARE_CURSOR, HOME for `~`), `options.platform`
/// monkeypatches `process.platform`, and `crypto.randomUUID` is patched to a
/// counter (or `options.uuids`) before the TS module's cache-busting import.
///
/// Ops (identical on both sides):
/// - `{op:"get"|"set", method, args?}` — call a camelCase method; the return
///   value is recorded (`undefined` → null); methods that throw record
///   `{thrown: "<message>"}`.
/// - `{op:"flush"}` / `{op:"reload"}` — the only awaits (the TS writeQueue
///   advances solely here, mirroring the Rust pending-write deque).
/// - `{op:"drainErrors"}` → `[{scope, path?, message}]` (paths normalized).
/// - `{op:"setProjectTrusted", args:[bool]}`, `{op:"applyOverrides", args:[v]}`.
/// - `{op:"storage"}` → both scopes' persisted strings (byte-level: pins key
///   order and 2-space indentation).
/// - `{op:"external", args:{scope, content}}` — write/replace/delete a
///   settings file (or in-memory slot) behind the manager's back.
/// - `{op:"pure", fn, args?}` — free functions `applyToolModifiers`,
///   `getToolListError`, `isToolModifier` and `DEFAULT_TOOL_NAMES`.
///
/// Unreachable TS behaviors (documented port divergences, avoided in cases):
/// non-object `inMemory` inputs, `applyOverrides(null)`, raw-passthrough
/// getters fed non-string values, `"[]"`-shaped settings files, non-array
/// `packages`/`extensions`/... values, and non-string `npmCommand` entries.
fn run_settings_manager_harness(input: &Value) -> Result<Value, String> {
    use pi_core::settings_manager::{
        apply_tool_modifiers, get_tool_list_error, is_tool_modifier, SettingsManager,
        SettingsManagerOptions, SettingsStorage, SettingsScope, UuidGen, DEFAULT_TOOL_NAMES,
    };

    static DIR_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// Mirrors TS `InMemorySettingsStorage`, with a shared snapshot handle so
    /// the `storage` op can read the persisted strings after `from_storage`
    /// takes ownership of the boxed trait object.
    #[derive(Clone, Default)]
    struct SharedMemoryStorage {
        state: Arc<Mutex<(Option<String>, Option<String>)>>,
    }
    impl SettingsStorage for SharedMemoryStorage {
        fn with_lock(
            &mut self,
            scope: SettingsScope,
            f: &mut dyn FnMut(Option<&str>) -> Option<String>,
        ) -> Result<(), String> {
            let mut state = self.state.lock().unwrap();
            let current = match scope {
                SettingsScope::Global => state.0.clone(),
                SettingsScope::Project => state.1.clone(),
            };
            let next = f(current.as_deref());
            if let Some(next) = next {
                match scope {
                    SettingsScope::Global => state.0 = Some(next),
                    SettingsScope::Project => state.1 = Some(next),
                }
            }
            Ok(())
        }
    }

    // ---- structural validation (mirrors the TS entry byte-exactly) ----
    let mode = input["mode"].as_str().ok_or("missing mode")?;
    if mode != "memory" && mode != "files" && mode != "inMemory" {
        return Err(format!("unknown settings-manager mode: {mode}"));
    }
    let storage_in = input.get("storage").cloned().unwrap_or(Value::Null);
    let storage_global = match storage_in.get("global") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("storage.global must be a string or null".to_string()),
    };
    let storage_project = match storage_in.get("project") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("storage.project must be a string or null".to_string()),
    };
    let options_in = input.get("options").cloned().unwrap_or_else(|| json!({}));
    let project_trusted = match options_in.get("projectTrusted") {
        None => None,
        Some(v) => Some(v.as_bool().ok_or("options.projectTrusted must be a boolean")?),
    };
    let mut env: HashMap<String, String> = HashMap::new();
    match options_in.get("env") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (k, v) in map {
                env.insert(
                    k.clone(),
                    v.as_str().ok_or("options.env values must be strings")?.to_string(),
                );
            }
        }
        Some(_) => return Err("options.env must be an object".to_string()),
    }
    let platform = match options_in.get("platform") {
        None => None,
        Some(v) => Some(v.as_str().ok_or("options.platform must be a string")?.to_string()),
    };
    let uuids: Vec<String> = match options_in.get("uuids") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => {
            let mut list = Vec::new();
            for item in items {
                list.push(
                    item.as_str().ok_or("options.uuids must be an array of strings")?.to_string(),
                );
            }
            list
        }
        Some(_) => return Err("options.uuids must be an array of strings".to_string()),
    };
    let ops: Vec<Value> = match input.get("ops") {
        Some(Value::Array(items)) => items.clone(),
        _ => return Err("missing ops".to_string()),
    };

    // Deterministic uuid generator: `options.uuids` in order (last entry
    // repeats once exhausted), or a `uuid-N` counter.
    let uuid_state = Arc::new(Mutex::new(0usize));
    let uuid_list = Arc::new(uuids);
    let uuid_gen: UuidGen = {
        let state = Arc::clone(&uuid_state);
        let list = Arc::clone(&uuid_list);
        Arc::new(move || {
            let i = {
                let mut s = state.lock().unwrap();
                let v = *s;
                *s += 1;
                v
            };
            if list.is_empty() {
                format!("uuid-{}", i + 1)
            } else {
                list[i.min(list.len() - 1)].clone()
            }
        })
    };
    let options = SettingsManagerOptions {
        project_trusted,
        env,
        home_dir: None,
        platform,
        uuid_gen: Some(uuid_gen),
    };

    // ---- manager construction ----
    let memory: Option<SharedMemoryStorage>;
    let mut temp_dirs: Option<(std::path::PathBuf, std::path::PathBuf)> = None;
    let mut manager = match mode {
        "memory" => {
            let mut storage = SharedMemoryStorage::default();
            if let Some(g) = &storage_global {
                let _ = storage.with_lock(SettingsScope::Global, &mut |_| Some(g.clone()));
            }
            if let Some(p) = &storage_project {
                let _ = storage.with_lock(SettingsScope::Project, &mut |_| Some(p.clone()));
            }
            memory = Some(storage.clone());
            SettingsManager::from_storage(Box::new(storage), options)
        }
        "inMemory" => {
            memory = None;
            let settings = input.get("settings").cloned().unwrap_or_else(|| json!({}));
            SettingsManager::in_memory(&settings, options)
        }
        _ => {
            memory = None;
            let seq = DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let base = std::env::temp_dir();
            let cwd = base.join(format!("pi-parity-sm-cwd-{}-{}", std::process::id(), seq));
            let agent_dir = base.join(format!("pi-parity-sm-agent-{}-{}", std::process::id(), seq));
            std::fs::create_dir_all(&cwd).map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&agent_dir).map_err(|e| e.to_string())?;
            if let Some(g) = &storage_global {
                std::fs::write(agent_dir.join("settings.json"), g).map_err(|e| e.to_string())?;
            }
            if let Some(p) = &storage_project {
                std::fs::create_dir_all(cwd.join(".pi")).map_err(|e| e.to_string())?;
                std::fs::write(cwd.join(".pi").join("settings.json"), p)
                    .map_err(|e| e.to_string())?;
            }
            let cwd_s = cwd.to_string_lossy().into_owned();
            let agent_s = agent_dir.to_string_lossy().into_owned();
            temp_dirs = Some((cwd, agent_dir));
            SettingsManager::create(&cwd_s, &agent_s, options)
        }
    };

    let (cwd_s, agent_s) = match &temp_dirs {
        Some((cwd, agent_dir)) => (
            cwd.to_string_lossy().into_owned(),
            agent_dir.to_string_lossy().into_owned(),
        ),
        None => (String::new(), String::new()),
    };
    let norm_path = |p: &str| -> String {
        let mut s = p.to_string();
        if !agent_s.is_empty() {
            s = s.replace(&agent_s, "<agentDir>");
        }
        if !cwd_s.is_empty() {
            s = s.replace(&cwd_s, "<cwd>");
        }
        s
    };
    let global_path = |temp_dirs: &Option<(std::path::PathBuf, std::path::PathBuf)>| {
        temp_dirs.as_ref().map(|( _, agent_dir)| agent_dir.join("settings.json"))
    };
    let project_path = |temp_dirs: &Option<(std::path::PathBuf, std::path::PathBuf)>| {
        temp_dirs.as_ref().map(|(cwd, _)| cwd.join(".pi").join("settings.json"))
    };

    // ---- op loop (per-op exceptions recorded in-band as {thrown}) ----
    let mut out: Vec<Value> = Vec::new();
    for op in &ops {
        let entry: Result<Value, String> = (|| {
            let kind = op["op"].as_str().ok_or("missing op")?;
            match kind {
                "get" | "set" => {
                    let method = op["method"].as_str().ok_or("missing method")?;
                    let args: Vec<Value> = match op.get("args") {
                        Some(Value::Array(items)) => items.clone(),
                        _ => Vec::new(),
                    };
                    sm_call_method(&mut manager, method, &args)
                }
                "flush" => {
                    manager.flush();
                    Ok(Value::Null)
                }
                "reload" => {
                    manager.reload();
                    Ok(Value::Null)
                }
                "drainErrors" => {
                    let errors: Vec<Value> = manager
                        .drain_errors()
                        .iter()
                        .map(|e| sm_error_value(e, &norm_path))
                        .collect();
                    Ok(Value::Array(errors))
                }
                "setProjectTrusted" => {
                    let args = op_arg_array(op)?;
                    sm_check_args("setProjectTrusted", &args, &["b"])?;
                    manager.set_project_trusted(args[0].as_bool().unwrap_or(false));
                    Ok(Value::Null)
                }
                "applyOverrides" => {
                    let args = op_arg_array(op)?;
                    sm_check_args("applyOverrides", &args, &["v"])?;
                    manager.apply_overrides(&args[0]);
                    Ok(Value::Null)
                }
                "storage" => {
                    if mode == "inMemory" {
                        return Err("storage op is unavailable in inMemory mode".to_string());
                    }
                    let read = |path: Option<std::path::PathBuf>| -> Value {
                        path.and_then(|p| std::fs::read_to_string(p).ok())
                            .map(Value::String)
                            .unwrap_or(Value::Null)
                    };
                    Ok(match (&memory, mode) {
                        (Some(mem), "memory") => {
                            let state = mem.state.lock().unwrap();
                            json!({
                                "global": state.0.clone().map(Value::String).unwrap_or(Value::Null),
                                "project": state.1.clone().map(Value::String).unwrap_or(Value::Null),
                            })
                        }
                        _ => json!({
                            "global": read(global_path(&temp_dirs)),
                            "project": read(project_path(&temp_dirs)),
                        }),
                    })
                }
                "external" => {
                    if mode == "inMemory" {
                        return Err("external op is unavailable in inMemory mode".to_string());
                    }
                    let args = op_arg_array(op)?;
                    sm_check_args("external", &args, &["v"])?;
                    let spec = args[0].as_object().ok_or("missing external spec")?;
                    let scope = spec.get("scope").and_then(Value::as_str).ok_or("missing scope")?;
                    let scope = match scope {
                        "global" => SettingsScope::Global,
                        "project" => SettingsScope::Project,
                        other => {
                            return Err(format!("settings-manager: external op scope must be global or project, got {other}"))
                        }
                    };
                    let content = match spec.get("content") {
                        None | Some(Value::Null) => None,
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(_) => {
                            return Err("settings-manager: external content must be a string or null".to_string())
                        }
                    };
                    if let Some(mem) = &memory {
                        let mut state = mem.state.lock().unwrap();
                        match scope {
                            SettingsScope::Global => state.0 = content,
                            SettingsScope::Project => state.1 = content,
                        }
                    } else {
                        let path = match scope {
                            SettingsScope::Global => global_path(&temp_dirs).unwrap(),
                            SettingsScope::Project => project_path(&temp_dirs).unwrap(),
                        };
                        match content {
                            Some(c) => {
                                if let Some(dir) = path.parent() {
                                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                                }
                                std::fs::write(&path, c).map_err(|e| e.to_string())?;
                            }
                            None => {
                                let _ = std::fs::remove_file(&path);
                            }
                        }
                    }
                    Ok(Value::Null)
                }
                "pure" => {
                    let fname = op["fn"].as_str().ok_or("missing fn")?;
                    let args: Vec<Value> = match op.get("args") {
                        Some(Value::Array(items)) => items.clone(),
                        _ => Vec::new(),
                    };
                    match fname {
                        "applyToolModifiers" => {
                            sm_check_args("applyToolModifiers", &args, &["sa", "sa"])?;
                            let base: Vec<String> = str_vec(&args[0]);
                            let entries: Vec<String> = str_vec(&args[1]);
                            Ok(json!(apply_tool_modifiers(&base, &entries)))
                        }
                        "getToolListError" => {
                            sm_check_args("getToolListError", &args, &["sa"])?;
                            let entries: Vec<String> = str_vec(&args[0]);
                            Ok(match get_tool_list_error(&entries) {
                                Some(e) => Value::String(e),
                                None => Value::Null,
                            })
                        }
                        "isToolModifier" => {
                            sm_check_args("isToolModifier", &args, &["v"])?;
                            Ok(Value::Bool(is_tool_modifier(&args[0])))
                        }
                        "DEFAULT_TOOL_NAMES" => {
                            sm_check_args("DEFAULT_TOOL_NAMES", &args, &[])?;
                            Ok(json!(DEFAULT_TOOL_NAMES.to_vec()))
                        }
                        other => Err(format!("unknown settings-manager pure fn: {other}")),
                    }
                }
                other => Err(format!("unknown settings-manager op: {other}")),
            }
        })();
        out.push(match entry {
            Ok(v) => v,
            Err(msg) => json!({ "thrown": msg }),
        });
    }

    if let Some((cwd, agent_dir)) = &temp_dirs {
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(agent_dir);
    }
    Ok(json!({ "result": { "out": out } }))
}

fn op_arg_array(op: &Value) -> Result<Vec<Value>, String> {
    match op.get("args") {
        Some(Value::Array(items)) => Ok(items.clone()),
        _ => Err("missing args".to_string()),
    }
}

fn str_vec(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `{scope, path?, message}` — the JSON rendering of a drained
/// [`pi_core::settings_manager::SettingsError`] (TS mirrors this by mapping
/// `e.error.message` and the optional `e.path`).
fn sm_error_value(e: &pi_core::settings_manager::SettingsError, norm_path: &dyn Fn(&str) -> String) -> Value {
    let mut out = serde_json::Map::new();
    out.insert(
        "scope".to_string(),
        Value::String(
            match e.scope {
                pi_core::settings_manager::SettingsScope::Global => "global",
                pi_core::settings_manager::SettingsScope::Project => "project",
            }
            .to_string(),
        ),
    );
    if let Some(path) = &e.path {
        if !path.is_empty() {
            out.insert("path".to_string(), Value::String(norm_path(path)));
        }
    }
    out.insert("message".to_string(), Value::String(e.message.clone()));
    Value::Object(out)
}

/// Argument-count/type validation with TS-identical messages. Tags: `s`
/// string, `b` boolean, `n` number, `s?` string-or-null, `sa` array of
/// strings, `sa?` that or null, `va` array, `v` any, `m?` model-or-null,
/// `w` `"auto"` or number.
fn sm_check_args(method: &str, args: &[Value], tags: &[&str]) -> Result<(), String> {
    if args.len() != tags.len() {
        return Err(format!(
            "settings-manager: {method} expects {} argument(s), got {}",
            tags.len(),
            args.len()
        ));
    }
    for (i, tag) in tags.iter().enumerate() {
        let a = &args[i];
        let ok = match *tag {
            "s" => a.is_string(),
            "b" => a.is_boolean(),
            "n" => a.is_number(),
            "s?" => a.is_string() || a.is_null(),
            "sa" => a.as_array().is_some_and(|x| x.iter().all(Value::is_string)),
            "sa?" => a.is_null() || a.as_array().is_some_and(|x| x.iter().all(Value::is_string)),
            "va" => a.is_array(),
            "m?" => a.is_null()
                || a.as_object().is_some_and(|o| {
                    o.get("provider").is_some_and(Value::is_string) && o.get("id").is_some_and(Value::is_string)
                }),
            "w" => a.as_str() == Some("auto") || a.is_number(),
            _ => true,
        };
        if !ok {
            let what = match *tag {
                "s" => "a string",
                "b" => "a boolean",
                "n" => "a number",
                "s?" => "a string or null",
                "sa" => "an array of strings",
                "sa?" => "an array of strings or null",
                "va" => "an array",
                "m?" => "a model or null",
                "w" => "\"auto\" or a number",
                _ => "a value",
            };
            return Err(format!("settings-manager: {method} argument {i} must be {what}"));
        }
    }
    Ok(())
}

fn sm_opt_str(v: Option<String>) -> Value {
    v.map(Value::String).unwrap_or(Value::Null)
}

fn sm_opt_vec(v: Option<Vec<String>>) -> Value {
    v.map(|list| Value::Array(list.into_iter().map(Value::String).collect()))
        .unwrap_or(Value::Null)
}

/// Method dispatch: camelCase TS method names over the ported snake_case
/// API. Getters return their JSON rendering (`undefined` → null); setters
/// return null; `Result` methods surface `Err` as the op's `{thrown}`.
fn sm_call_method(
    manager: &mut pi_core::settings_manager::SettingsManager,
    method: &str,
    args: &[Value],
) -> Result<Value, String> {
    use pi_core::settings_manager::{
        ModelRef, WheelScrollLines,
    };
    // Typed extraction (safe after sm_check_args).
    let s = |i: usize| args.get(i).and_then(Value::as_str).unwrap_or_default().to_string();
    let b = |i: usize| args.get(i).and_then(Value::as_bool).unwrap_or(false);
    let n = |i: usize| args.get(i).and_then(Value::as_f64).unwrap_or(0.0);
    let os = |i: usize| args.get(i).and_then(Value::as_str).map(str::to_string);
    let sa = |i: usize| str_vec(args.get(i).unwrap_or(&Value::Null));
    let va = |i: usize| args.get(i).and_then(Value::as_array).cloned().unwrap_or_default();
    let model = |i: usize| -> Option<(String, String)> {
        args.get(i).and_then(Value::as_object).map(|o| {
            (
                o.get("provider").and_then(Value::as_str).unwrap_or_default().to_string(),
                o.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
            )
        })
    };

    macro_rules! ck {
        ($tags:expr) => {
            sm_check_args(method, args, &$tags)?
        };
    }
    match method {
        "flush" => {
            ck!([]);
            manager.flush();
            Ok(Value::Null)
        }
        "reload" => {
            ck!([]);
            manager.reload();
            Ok(Value::Null)
        }
        // ---- core state ----
        "getSettings" => {
            ck!([]);
            Ok(manager.get_settings())
        }
        "getGlobalSettings" => {
            ck!([]);
            Ok(manager.get_global_settings())
        }
        "getProjectSettings" => {
            ck!([]);
            Ok(manager.get_project_settings())
        }
        "isProjectTrusted" => {
            ck!([]);
            Ok(Value::Bool(manager.is_project_trusted()))
        }
        "getLastChangelogVersion" => {
            ck!([]);
            Ok(sm_opt_str(manager.get_last_changelog_version()))
        }
        "setLastChangelogVersion" => {
            ck!(["s"]);
            manager.set_last_changelog_version(&s(0));
            Ok(Value::Null)
        }
        "getSessionDir" => {
            ck!([]);
            Ok(sm_opt_str(manager.get_session_dir()))
        }
        "getDefaultProvider" => {
            ck!([]);
            Ok(sm_opt_str(manager.get_default_provider()))
        }
        "getDefaultModel" => {
            ck!([]);
            Ok(sm_opt_str(manager.get_default_model()))
        }
        "setDefaultProvider" => {
            ck!(["s"]);
            manager.set_default_provider(&s(0));
            Ok(Value::Null)
        }
        "setDefaultModel" => {
            ck!(["s"]);
            manager.set_default_model(&s(0));
            Ok(Value::Null)
        }
        "setDefaultModelAndProvider" => {
            ck!(["s", "s"]);
            manager.set_default_model_and_provider(&s(0), &s(1));
            Ok(Value::Null)
        }
        "getSteeringMode" => {
            ck!([]);
            Ok(Value::String(manager.get_steering_mode()))
        }
        "setSteeringMode" => {
            ck!(["s"]);
            manager.set_steering_mode(&s(0));
            Ok(Value::Null)
        }
        "getFollowUpMode" => {
            ck!([]);
            Ok(Value::String(manager.get_follow_up_mode()))
        }
        "setFollowUpMode" => {
            ck!(["s"]);
            manager.set_follow_up_mode(&s(0));
            Ok(Value::Null)
        }
        "getThemeSetting" => {
            ck!([]);
            Ok(sm_opt_str(manager.get_theme_setting()))
        }
        "getTheme" => {
            ck!([]);
            Ok(sm_opt_str(manager.get_theme()))
        }
        "setTheme" => {
            ck!(["s"]);
            manager.set_theme(&s(0));
            Ok(Value::Null)
        }
        "getDefaultThinkingLevel" => {
            ck!([]);
            Ok(sm_opt_str(manager.get_default_thinking_level()))
        }
        "setDefaultThinkingLevel" => {
            ck!(["s"]);
            manager.set_default_thinking_level(&s(0));
            Ok(Value::Null)
        }
        "getModelThinkingLevel" => {
            ck!(["s", "s"]);
            Ok(sm_opt_str(manager.get_model_thinking_level(&s(0), &s(1))))
        }
        "getAllModelThinkingLevels" => {
            ck!([]);
            Ok(manager.get_all_model_thinking_levels())
        }
        "setModelThinkingLevel" => {
            ck!(["s", "s", "s"]);
            manager.set_model_thinking_level(&s(0), &s(1), &s(2));
            Ok(Value::Null)
        }
        "removeModelThinkingLevel" => {
            ck!(["s", "s"]);
            manager.remove_model_thinking_level(&s(0), &s(1));
            Ok(Value::Null)
        }
        "getTransport" => {
            ck!([]);
            Ok(Value::String(manager.get_transport()))
        }
        "setTransport" => {
            ck!(["s"]);
            manager.set_transport(&s(0));
            Ok(Value::Null)
        }
        "getSessionShareDestination" => {
            ck!([]);
            Ok(Value::String(manager.get_session_share_destination()))
        }
        "setSessionShareDestination" => {
            ck!(["s"]);
            manager.set_session_share_destination(&s(0));
            Ok(Value::Null)
        }
        "getCompactionEnabled" => {
            ck!([]);
            Ok(Value::Bool(manager.get_compaction_enabled()))
        }
        "setCompactionEnabled" => {
            ck!(["b"]);
            manager.set_compaction_enabled(b(0));
            Ok(Value::Null)
        }
        "getCompactionReserveTokens" => {
            ck!(["m?"]);
            let m = model(0);
            let r = m.as_ref().map(|(p, id)| ModelRef { provider: p, id });
            Ok(Value::from(manager.get_compaction_reserve_tokens(r)?))
        }
        "getCompactionKeepRecentTokens" => {
            ck!(["m?"]);
            let m = model(0);
            let r = m.as_ref().map(|(p, id)| ModelRef { provider: p, id });
            Ok(Value::from(manager.get_compaction_keep_recent_tokens(r)?))
        }
        "getCompactionSettings" => {
            ck!(["m?"]);
            let m = model(0);
            let r = m.as_ref().map(|(p, id)| ModelRef { provider: p, id });
            let settings = manager.get_compaction_settings(r)?;
            Ok(json!({
                "enabled": settings.enabled,
                "reserveTokens": settings.reserve_tokens,
                "keepRecentTokens": settings.keep_recent_tokens,
            }))
        }
        "getBranchSummarySettings" => {
            ck!([]);
            let s = manager.get_branch_summary_settings();
            Ok(json!({ "reserveTokens": s.reserve_tokens, "skipPrompt": s.skip_prompt }))
        }
        "getBranchSummarySkipPrompt" => {
            ck!([]);
            Ok(Value::Bool(manager.get_branch_summary_skip_prompt()))
        }
        "getRetryEnabled" => {
            ck!([]);
            Ok(Value::Bool(manager.get_retry_enabled()))
        }
        "setRetryEnabled" => {
            ck!(["b"]);
            manager.set_retry_enabled(b(0));
            Ok(Value::Null)
        }
        "getRetrySettings" => {
            ck!([]);
            let s = manager.get_retry_settings();
            Ok(json!({
                "enabled": s.enabled,
                "maxRetries": s.max_retries,
                "baseDelayMs": s.base_delay_ms,
                "maxAgentDelayMs": s.max_agent_delay_ms,
            }))
        }
        "getHttpIdleTimeoutMs" => {
            ck!([]);
            Ok(Value::from(manager.get_http_idle_timeout_ms()?))
        }
        "setHttpIdleTimeoutMs" => {
            ck!(["n"]);
            manager.set_http_idle_timeout_ms(n(0))?;
            Ok(Value::Null)
        }
        "getCacheWarmingMode" => {
            ck!([]);
            Ok(Value::String(manager.get_cache_warming_mode()))
        }
        "setCacheWarmingMode" => {
            ck!(["s"]);
            manager.set_cache_warming_mode(&s(0));
            Ok(Value::Null)
        }
        "getProviderRetrySettings" => {
            ck!([]);
            let s = manager.get_provider_retry_settings();
            let mut out = serde_json::Map::new();
            if let Some(v) = s.timeout_ms {
                out.insert("timeoutMs".to_string(), Value::from(v));
            }
            if let Some(v) = s.max_retries {
                out.insert("maxRetries".to_string(), Value::from(v));
            }
            out.insert("maxRetryDelayMs".to_string(), Value::from(s.max_retry_delay_ms));
            Ok(Value::Object(out))
        }
        "getWebSocketConnectTimeoutMs" => {
            ck!([]);
            Ok(manager
                .get_websocket_connect_timeout_ms()?
                .map(Value::from)
                .unwrap_or(Value::Null))
        }
        "getHideThinkingBlock" => {
            ck!([]);
            Ok(Value::Bool(manager.get_hide_thinking_block()))
        }
        "setHideThinkingBlock" => {
            ck!(["b"]);
            manager.set_hide_thinking_block(b(0));
            Ok(Value::Null)
        }
        "getShowCacheMissNotices" => {
            ck!([]);
            Ok(Value::Bool(manager.get_show_cache_miss_notices()))
        }
        "setShowCacheMissNotices" => {
            ck!(["b"]);
            manager.set_show_cache_miss_notices(b(0));
            Ok(Value::Null)
        }
        "getExternalEditorCommand" => {
            ck!([]);
            Ok(Value::String(manager.get_external_editor_command()))
        }
        "getShellPath" => {
            ck!([]);
            Ok(sm_opt_str(manager.get_shell_path()))
        }
        "setShellPath" => {
            ck!(["s?"]);
            manager.set_shell_path(os(0).as_deref());
            Ok(Value::Null)
        }
        "getQuietStartup" => {
            ck!([]);
            Ok(manager.get_quiet_startup())
        }
        "setQuietStartup" => {
            ck!(["v"]);
            manager.set_quiet_startup(args.first().unwrap_or(&Value::Null));
            Ok(Value::Null)
        }
        "getDefaultProjectTrust" => {
            ck!([]);
            Ok(Value::String(manager.get_default_project_trust()))
        }
        "setDefaultProjectTrust" => {
            ck!(["s"]);
            manager.set_default_project_trust(&s(0));
            Ok(Value::Null)
        }
        "getShellCommandPrefix" => {
            ck!([]);
            Ok(sm_opt_str(manager.get_shell_command_prefix()))
        }
        "setShellCommandPrefix" => {
            ck!(["s?"]);
            manager.set_shell_command_prefix(os(0).as_deref());
            Ok(Value::Null)
        }
        "getNpmCommand" => {
            ck!([]);
            Ok(sm_opt_vec(manager.get_npm_command()))
        }
        "setNpmCommand" => {
            ck!(["sa?"]);
            let cmd = sa(0);
            manager.set_npm_command(if args.first().and_then(Value::as_array).is_some() {
                Some(cmd.as_slice())
            } else {
                None
            });
            Ok(Value::Null)
        }
        "getCollapseChangelog" => {
            ck!([]);
            Ok(Value::Bool(manager.get_collapse_changelog()))
        }
        "setCollapseChangelog" => {
            ck!(["b"]);
            manager.set_collapse_changelog(b(0));
            Ok(Value::Null)
        }
        "getEnableInstallTelemetry" => {
            ck!([]);
            Ok(Value::Bool(manager.get_enable_install_telemetry()))
        }
        "setEnableInstallTelemetry" => {
            ck!(["b"]);
            manager.set_enable_install_telemetry(b(0));
            Ok(Value::Null)
        }
        "getEnableAnalytics" => {
            ck!([]);
            Ok(Value::Bool(manager.get_enable_analytics()))
        }
        "getTrackingId" => {
            ck!([]);
            Ok(sm_opt_str(manager.get_tracking_id()))
        }
        "setEnableAnalytics" => {
            ck!(["b"]);
            manager.set_enable_analytics(b(0));
            Ok(Value::Null)
        }
        "getOrCreateDeviceId" => {
            ck!([]);
            Ok(Value::String(manager.get_or_create_device_id()))
        }
        "getPackages" => {
            ck!([]);
            Ok(Value::Array(manager.get_packages()))
        }
        "setPackages" => {
            ck!(["va"]);
            manager.set_packages(&va(0));
            Ok(Value::Null)
        }
        "setProjectPackages" => {
            ck!(["va"]);
            manager.set_project_packages(&va(0))?;
            Ok(Value::Null)
        }
        "getExtensionPaths" => {
            ck!([]);
            Ok(Value::Array(manager.get_extension_paths()))
        }
        "setExtensionPaths" => {
            ck!(["va"]);
            manager.set_extension_paths(&va(0));
            Ok(Value::Null)
        }
        "setProjectExtensionPaths" => {
            ck!(["va"]);
            manager.set_project_extension_paths(&va(0))?;
            Ok(Value::Null)
        }
        "getSkillPaths" => {
            ck!([]);
            Ok(Value::Array(manager.get_skill_paths()))
        }
        "setSkillPaths" => {
            ck!(["va"]);
            manager.set_skill_paths(&va(0));
            Ok(Value::Null)
        }
        "setProjectSkillPaths" => {
            ck!(["va"]);
            manager.set_project_skill_paths(&va(0))?;
            Ok(Value::Null)
        }
        "getPromptTemplatePaths" => {
            ck!([]);
            Ok(Value::Array(manager.get_prompt_template_paths()))
        }
        "setPromptTemplatePaths" => {
            ck!(["va"]);
            manager.set_prompt_template_paths(&va(0));
            Ok(Value::Null)
        }
        "setProjectPromptTemplatePaths" => {
            ck!(["va"]);
            manager.set_project_prompt_template_paths(&va(0))?;
            Ok(Value::Null)
        }
        "getThemePaths" => {
            ck!([]);
            Ok(Value::Array(manager.get_theme_paths()))
        }
        "setThemePaths" => {
            ck!(["va"]);
            manager.set_theme_paths(&va(0));
            Ok(Value::Null)
        }
        "setProjectThemePaths" => {
            ck!(["va"]);
            manager.set_project_theme_paths(&va(0))?;
            Ok(Value::Null)
        }
        "getEnableSkillCommands" => {
            ck!([]);
            Ok(Value::Bool(manager.get_enable_skill_commands()))
        }
        "setEnableSkillCommands" => {
            ck!(["b"]);
            manager.set_enable_skill_commands(b(0));
            Ok(Value::Null)
        }
        "getThinkingBudgets" => {
            ck!([]);
            Ok(manager.get_thinking_budgets().unwrap_or(Value::Null))
        }
        "getTerminalCapabilityOverrides" => {
            ck!([]);
            Ok(manager.get_terminal_capability_overrides())
        }
        "getShowImages" => {
            ck!([]);
            Ok(Value::Bool(manager.get_show_images()))
        }
        "setShowImages" => {
            ck!(["b"]);
            manager.set_show_images(b(0));
            Ok(Value::Null)
        }
        "getImageWidthCells" => {
            ck!([]);
            Ok(Value::from(manager.get_image_width_cells()))
        }
        "setImageWidthCells" => {
            ck!(["n"]);
            manager.set_image_width_cells(n(0));
            Ok(Value::Null)
        }
        "getClearOnShrink" => {
            ck!([]);
            Ok(Value::Bool(manager.get_clear_on_shrink()))
        }
        "setClearOnShrink" => {
            ck!(["b"]);
            manager.set_clear_on_shrink(b(0));
            Ok(Value::Null)
        }
        "getShowTerminalProgress" => {
            ck!([]);
            Ok(Value::Bool(manager.get_show_terminal_progress()))
        }
        "setShowTerminalProgress" => {
            ck!(["b"]);
            manager.set_show_terminal_progress(b(0));
            Ok(Value::Null)
        }
        "getTuiMode" => {
            ck!([]);
            Ok(Value::String(manager.get_tui_mode()))
        }
        "setTuiMode" => {
            ck!(["s"]);
            manager.set_tui_mode(&s(0));
            Ok(Value::Null)
        }
        "getFullscreenExitOutput" => {
            ck!([]);
            Ok(Value::String(manager.get_fullscreen_exit_output()))
        }
        "setFullscreenExitOutput" => {
            ck!(["s"]);
            manager.set_fullscreen_exit_output(&s(0));
            Ok(Value::Null)
        }
        "getFullscreenScrollbar" => {
            ck!([]);
            Ok(Value::String(manager.get_fullscreen_scrollbar()))
        }
        "setFullscreenScrollbar" => {
            ck!(["s"]);
            manager.set_fullscreen_scrollbar(&s(0));
            Ok(Value::Null)
        }
        "getFullscreenCopyOnSelect" => {
            ck!([]);
            Ok(Value::Bool(manager.get_fullscreen_copy_on_select()))
        }
        "setFullscreenCopyOnSelect" => {
            ck!(["b"]);
            manager.set_fullscreen_copy_on_select(b(0));
            Ok(Value::Null)
        }
        "getFullscreenWheelScrollLines" => {
            ck!([]);
            Ok(manager.get_fullscreen_wheel_scroll_lines())
        }
        "setFullscreenWheelScrollLines" => {
            ck!(["w"]);
            let lines = if args.first().and_then(Value::as_str) == Some("auto") {
                WheelScrollLines::Auto
            } else {
                WheelScrollLines::Lines(n(0))
            };
            manager.set_fullscreen_wheel_scroll_lines(lines);
            Ok(Value::Null)
        }
        "getImageAutoResize" => {
            ck!([]);
            Ok(Value::Bool(manager.get_image_auto_resize()))
        }
        "setImageAutoResize" => {
            ck!(["b"]);
            manager.set_image_auto_resize(b(0));
            Ok(Value::Null)
        }
        "getBlockImages" => {
            ck!([]);
            Ok(Value::Bool(manager.get_block_images()))
        }
        "setBlockImages" => {
            ck!(["b"]);
            manager.set_block_images(b(0));
            Ok(Value::Null)
        }
        "getEnabledModels" => {
            ck!([]);
            Ok(manager.get_enabled_models().unwrap_or(Value::Null))
        }
        "setEnabledModels" => {
            ck!(["sa?"]);
            let patterns = sa(0);
            manager.set_enabled_models(if args.first().and_then(Value::as_array).is_some() {
                Some(patterns.as_slice())
            } else {
                None
            });
            Ok(Value::Null)
        }
        "getDefaultTools" => {
            ck!([]);
            Ok(manager
                .get_default_tools()
                .map(|tools| Value::Array(tools.into_iter().map(Value::String).collect()))
                .unwrap_or(Value::Null))
        }
        "getDoubleEscapeAction" => {
            ck!([]);
            Ok(Value::String(manager.get_double_escape_action()))
        }
        "setDoubleEscapeAction" => {
            ck!(["s"]);
            manager.set_double_escape_action(&s(0));
            Ok(Value::Null)
        }
        "getTreeFilterMode" => {
            ck!([]);
            Ok(Value::String(manager.get_tree_filter_mode()))
        }
        "setTreeFilterMode" => {
            ck!(["s"]);
            manager.set_tree_filter_mode(&s(0));
            Ok(Value::Null)
        }
        "getShowHardwareCursor" => {
            ck!([]);
            Ok(Value::Bool(manager.get_show_hardware_cursor()))
        }
        "setShowHardwareCursor" => {
            ck!(["b"]);
            manager.set_show_hardware_cursor(b(0));
            Ok(Value::Null)
        }
        "getEditorPaddingX" => {
            ck!([]);
            Ok(Value::from(manager.get_editor_padding_x()))
        }
        "setEditorPaddingX" => {
            ck!(["n"]);
            manager.set_editor_padding_x(n(0));
            Ok(Value::Null)
        }
        "getOutputPad" => {
            ck!([]);
            Ok(Value::from(manager.get_output_pad()))
        }
        "setOutputPad" => {
            ck!(["n"]);
            manager.set_output_pad(n(0));
            Ok(Value::Null)
        }
        "getAutocompleteMaxVisible" => {
            ck!([]);
            Ok(Value::from(manager.get_autocomplete_max_visible()))
        }
        "setAutocompleteMaxVisible" => {
            ck!(["n"]);
            manager.set_autocomplete_max_visible(n(0));
            Ok(Value::Null)
        }
        "getCodeBlockIndent" => {
            ck!([]);
            Ok(Value::String(manager.get_code_block_indent()))
        }
        "getMermaidRenderingMode" => {
            ck!([]);
            Ok(Value::String(manager.get_mermaid_rendering_mode()))
        }
        "setMermaidRenderingMode" => {
            ck!(["s"]);
            manager.set_mermaid_rendering_mode(&s(0));
            Ok(Value::Null)
        }
        "getWarnings" => {
            ck!([]);
            Ok(manager.get_warnings())
        }
        "setWarnings" => {
            ck!(["v"]);
            manager.set_warnings(args.first().unwrap_or(&Value::Null));
            Ok(Value::Null)
        }
        other => Err(format!("unknown settings-manager method: {other}")),
    }
}

// ============================================================================
// prompt-templates / skills (shared per-case fixture helpers)
// ============================================================================

/// Validate the `files` map shape (`{relpath: content | {symlink: target}}`).
/// Byte-identical error messages with the TS entries.
fn validate_files(files: Option<&Value>) -> Result<(), String> {
    let files = files.unwrap_or(&Value::Null);
    if !matches!(files, Value::Null | Value::Object(_)) {
        return Err("files must be an object".to_string());
    }
    if let Value::Object(map) = files {
        for v in map.values() {
            let ok = matches!(v, Value::String(_))
                || matches!(v, Value::Object(o)
                    if o.len() == 1 && o.get("symlink").is_some_and(|s| s.as_str().is_some()));
            if !ok {
                return Err("files values must be a string or {symlink}".to_string());
            }
        }
    }
    Ok(())
}

/// Materialize the pre-validated `files` map under a fresh temp dir.
/// Mirrors the TS `mkdtempSync` + `writeFileSync`/`symlinkSync` fixture; the
/// absolute base never leaks into results (see `normalize_base`).
fn create_fixture(files: Option<&Value>, prefix: &str) -> Result<std::path::PathBuf, String> {
    static DIR_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let files = files.unwrap_or(&Value::Null);
    let seq = DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let base = std::env::temp_dir()
        .join(format!("pi-parity-{prefix}-{}-{}", std::process::id(), seq));
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    if let Value::Object(map) = files {
        for (rel, v) in map {
            let target = base.join(rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            match v {
                Value::String(content) => {
                    std::fs::write(&target, content).map_err(|e| e.to_string())?
                }
                Value::Object(_) => {
                    let link = v.get("symlink").and_then(|s| s.as_str()).unwrap_or("");
                    std::os::unix::fs::symlink(link, &target).map_err(|e| e.to_string())?;
                }
                _ => unreachable!("validated by validate_files"),
            }
        }
    }
    Ok(base)
}

/// Node-ish `path.join(base, rel)` for harness-level roots (cases use clean
/// relative subpaths; `""` / `"."` mean the base itself).
fn join_rel(base: &str, rel: &str) -> String {
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() || rel == "." {
        return base.to_string();
    }
    format!("{}/{}", base.trim_end_matches('/'), rel)
}

/// Rewrite every occurrence of the per-case temp `base` in `value` to
/// `<base>` so results from the two sides compare equal.
fn normalize_base(value: &mut Value, base: &str) {
    match value {
        Value::String(s) => {
            if s.contains(base) {
                *s = s.replace(base, "<base>");
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_base(item, base);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                normalize_base(item, base);
            }
        }
        _ => {}
    }
}

/// Optional string field (`null`/absent → `None`) with a shared type error.
fn opt_string_field(obj: &Value, key: &str, err: &str) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(err.to_string()),
    }
}

fn opt_strings_field(obj: &Value, key: &str, err: &str) -> Result<Option<Vec<String>>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(
                    item.as_str()
                        .ok_or_else(|| err.to_string())?
                        .to_string(),
                );
            }
            Ok(Some(out))
        }
        Some(_) => Err(err.to_string()),
    }
}

fn opt_bool_field(obj: &Value, key: &str, err: &str) -> Result<Option<bool>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(err.to_string()),
    }
}

/// JS truthiness for `disableModelInvocation`-style flags.
fn js_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n
            .as_f64()
            .map(|x| x != 0.0 && !x.is_nan())
            .unwrap_or(true),
        Some(_) => true,
    }
}

/// Sort a serialized templates/skills/diagnostics array by two string keys —
/// readdir iteration order differs between the two sides, so multi-file
/// directories must never be asserted order-sensitively.
fn sort_by_keys(items: &mut [Value], a: &str, b: &str) {
    items.sort_by(|x, y| {
        let key = |v: &Value| {
            (
                v[a].as_str().unwrap_or("").to_string(),
                v[b].as_str().unwrap_or("").to_string(),
            )
        };
        key(x).cmp(&key(y))
    });
}

/// Serialize + normalize + sort a load result (shared by the `load` and
/// `loadFromDir` ops of both tools).
fn fixture_result(
    primary: Vec<Value>,
    diagnostics: Vec<Value>,
    base: &str,
    primary_keys: (&str, &str),
) -> Value {
    let mut primary = primary;
    let mut diagnostics = diagnostics;
    for v in primary.iter_mut().chain(diagnostics.iter_mut()) {
        normalize_base(v, base);
    }
    sort_by_keys(&mut primary, primary_keys.0, primary_keys.1);
    sort_by_keys(&mut diagnostics, "path", "message");
    json!({
        "templates_or_skills": primary,
        "diagnostics": diagnostics,
    })
}

// ============================================================================
// prompt-templates
// ============================================================================

/// `prompt-templates` harness: `pi_core::prompt_templates` (TS
/// `core/prompt-templates.js`). args `{ op: ... }`:
/// - `{op:"parseCommandArgs", argsString}` → `Vec<String>`
/// - `{op:"substituteArgs", content, args?}` → substituted string
/// - `{op:"expandPromptTemplate", text, templates?}` → expanded string
///   (entries need `name`; only name/content reach the module)
/// - `{op:"load", files?, options?}` → `{templates_or_skills, diagnostics}`
///   with `files` materialized under a per-case temp dir (`<base>`-normalized
///   paths, `(filePath, name)` / `(path, message)` sort). `options.cwd` /
///   `agentDir` / `homeDir` are relative roots joined to the base (homeDir
///   defaults to `"home"` and backs `~` expansion; the TS side injects it as
///   `$HOME`); `promptPaths` entries are passed verbatim and resolve against
///   `cwd` like the module does. `configDirName` is fixed to the dist value
///   `".pi"` on both sides (TS reads it from package.json at import time).
fn run_prompt_templates_harness(input: &Value) -> Result<Value, String> {
    use pi_core::prompt_templates::{
        expand_prompt_template, load_prompt_templates, parse_command_args, substitute_args,
        LoadPromptTemplatesOptions, PromptTemplate, SourceInfo, SourceOrigin, SourceScope,
    };

    let op = input["op"].as_str().ok_or("missing op")?;
    match op {
        "parseCommandArgs" => {
            let args_string = input["argsString"].as_str().ok_or("missing argsString")?;
            Ok(json!({ "result": parse_command_args(args_string) }))
        }
        "substituteArgs" => {
            let content = input["content"]
                .as_str()
                .ok_or("content must be a string")?;
            let args: Vec<String> = match input.get("args") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .ok_or_else(|| "args must be an array of strings".to_string())
                    })
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err("args must be an array of strings".to_string()),
            };
            Ok(json!({ "result": substitute_args(content, &args) }))
        }
        "expandPromptTemplate" => {
            let text = input["text"].as_str().ok_or("text must be a string")?;
            let templates_in = match input.get("templates") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items.clone(),
                Some(_) => return Err("templates must be an array".to_string()),
            };
            let mut templates = Vec::with_capacity(templates_in.len());
            for t in &templates_in {
                let obj = t
                    .as_object()
                    .ok_or("templates entries must be objects")?;
                let name = obj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or("templates entries must have a string name")?;
                for (key, err) in [
                    ("content", "templates entry content must be a string"),
                    ("description", "templates entry description must be a string"),
                ] {
                    if let Some(v) = obj.get(key) {
                        if !v.is_string() {
                            return Err(err.to_string());
                        }
                    }
                }
                templates.push(PromptTemplate {
                    name: name.to_string(),
                    description: obj
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    argument_hint: None,
                    content: obj
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    source_info: SourceInfo {
                        path: String::new(),
                        source: "local".to_string(),
                        scope: SourceScope::Temporary,
                        origin: SourceOrigin::TopLevel,
                        base_dir: None,
                    },
                    file_path: String::new(),
                });
            }
            Ok(json!({ "result": expand_prompt_template(text, &templates) }))
        }
        "load" => {
            validate_files(input.get("files"))?;
            let options_in = match input.get("options") {
                None | Some(Value::Null) => json!({}),
                Some(v @ Value::Object(_)) => v.clone(),
                Some(_) => return Err("options must be an object".to_string()),
            };
            let cwd_rel = opt_string_field(&options_in, "cwd", "options.cwd must be a string")?
                .unwrap_or_default();
            let agent_rel =
                opt_string_field(&options_in, "agentDir", "options.agentDir must be a string")?
                    .unwrap_or_default();
            let home_rel = opt_string_field(
                &options_in,
                "homeDir",
                "options.homeDir must be a string",
            )?
            .unwrap_or_else(|| "home".to_string());
            let prompt_paths = opt_strings_field(
                &options_in,
                "promptPaths",
                "options.promptPaths must be an array of strings",
            )?
            .unwrap_or_default();
            let include_defaults = opt_bool_field(
                &options_in,
                "includeDefaults",
                "options.includeDefaults must be a boolean",
            )?
            .unwrap_or(false);

            let base = create_fixture(input.get("files"), "pt")?;
            let base_s = base.to_string_lossy().into_owned();
            let options = LoadPromptTemplatesOptions {
                cwd: join_rel(&base_s, &cwd_rel),
                agent_dir: join_rel(&base_s, &agent_rel),
                prompt_paths,
                include_defaults,
                config_dir_name: ".pi".to_string(),
                home_dir: Some(join_rel(&base_s, &home_rel)),
            };
            let result = load_prompt_templates(&options);
            let templates: Vec<Value> = result
                .templates
                .iter()
                .map(|t| serde_json::to_value(t).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            let diagnostics: Vec<Value> = result
                .diagnostics
                .iter()
                .map(|d| serde_json::to_value(d).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            let _ = std::fs::remove_dir_all(&base);
            Ok(json!({ "result": fixture_result(templates, diagnostics, &base_s, ("filePath", "name")) }))
        }
        other => Err(format!("unknown prompt-templates op: {other}")),
    }
}

// ============================================================================
// skills
// ============================================================================

/// `skills` harness: `pi_core::skills` (TS `core/skills.js`). args
/// `{ op: ... }`:
/// - `{op:"loadFromDir", files?, source, dir?}` — `dir` is a relative
///   subpath under the per-case base (default: the base itself; cases use a
///   named subdir because the name fallback for root files is the parent
///   directory name, which would leak the per-side temp basename).
/// - `{op:"load", files?, options?}` — like the prompt-templates `load` op
///   but with `skillPaths`; collisions and canonical-path dedupe are
///   exercised through multiple paths / symlinks.
/// - `{op:"formatForPrompt", skills?, fileReadTool?}` — the formatted
///   string; `fileReadTool` maps `"read"`/`"bash"` and anything else to the
///   indirect variant (mirroring the TS else-branch).
fn run_skills_harness(input: &Value) -> Result<Value, String> {
    use pi_core::prompt_templates::{SourceInfo, SourceOrigin, SourceScope};
    use pi_core::skills::{
        format_skills_for_prompt, load_skills, load_skills_from_dir, FileReadTool,
        LoadSkillsOptions, Skill,
    };

    let op = input["op"].as_str().ok_or("missing op")?;
    match op {
        "loadFromDir" => {
            validate_files(input.get("files"))?;
            let source = input["source"].as_str().ok_or("missing source")?;
            let dir_rel = opt_string_field(input, "dir", "dir must be a string")?
                .unwrap_or_default();
            let base = create_fixture(input.get("files"), "sk")?;
            let base_s = base.to_string_lossy().into_owned();
            let dir = join_rel(&base_s, &dir_rel);
            let result = load_skills_from_dir(&dir, source);
            let skills: Vec<Value> = result
                .skills
                .iter()
                .map(|s| serde_json::to_value(s).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            let diagnostics: Vec<Value> = result
                .diagnostics
                .iter()
                .map(|d| serde_json::to_value(d).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            let _ = std::fs::remove_dir_all(&base);
            Ok(json!({ "result": fixture_result(skills, diagnostics, &base_s, ("name", "filePath")) }))
        }
        "load" => {
            validate_files(input.get("files"))?;
            let options_in = match input.get("options") {
                None | Some(Value::Null) => json!({}),
                Some(v @ Value::Object(_)) => v.clone(),
                Some(_) => return Err("options must be an object".to_string()),
            };
            let cwd_rel = opt_string_field(&options_in, "cwd", "options.cwd must be a string")?
                .unwrap_or_default();
            let agent_rel =
                opt_string_field(&options_in, "agentDir", "options.agentDir must be a string")?
                    .unwrap_or_default();
            let home_rel = opt_string_field(
                &options_in,
                "homeDir",
                "options.homeDir must be a string",
            )?
            .unwrap_or_else(|| "home".to_string());
            let skill_paths = opt_strings_field(
                &options_in,
                "skillPaths",
                "options.skillPaths must be an array of strings",
            )?
            .unwrap_or_default();
            let include_defaults = opt_bool_field(
                &options_in,
                "includeDefaults",
                "options.includeDefaults must be a boolean",
            )?
            .unwrap_or(false);

            let base = create_fixture(input.get("files"), "sk")?;
            let base_s = base.to_string_lossy().into_owned();
            let options = LoadSkillsOptions {
                cwd: join_rel(&base_s, &cwd_rel),
                agent_dir: join_rel(&base_s, &agent_rel),
                skill_paths,
                include_defaults,
                config_dir_name: ".pi".to_string(),
                home_dir: Some(join_rel(&base_s, &home_rel)),
            };
            let result = load_skills(&options);
            let skills: Vec<Value> = result
                .skills
                .iter()
                .map(|s| serde_json::to_value(s).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            let diagnostics: Vec<Value> = result
                .diagnostics
                .iter()
                .map(|d| serde_json::to_value(d).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            let _ = std::fs::remove_dir_all(&base);
            Ok(json!({ "result": fixture_result(skills, diagnostics, &base_s, ("name", "filePath")) }))
        }
        "formatForPrompt" => {
            let skills_in = match input.get("skills") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items.clone(),
                Some(_) => return Err("skills must be an array".to_string()),
            };
            let mut skills = Vec::with_capacity(skills_in.len());
            for s in &skills_in {
                let obj = s.as_object().ok_or("skills entries must be objects")?;
                let field = |key: &str| {
                    obj.get(key).and_then(|v| v.as_str()).ok_or_else(|| {
                        "skills entries must have string name, description and filePath"
                            .to_string()
                    })
                };
                let name = field("name")?;
                let description = field("description")?;
                let file_path = field("filePath")?;
                skills.push(Skill {
                    name: name.to_string(),
                    description: description.to_string(),
                    file_path: file_path.to_string(),
                    base_dir: String::new(),
                    source_info: SourceInfo {
                        path: file_path.to_string(),
                        source: "local".to_string(),
                        scope: SourceScope::Temporary,
                        origin: SourceOrigin::TopLevel,
                        base_dir: None,
                    },
                    disable_model_invocation: js_truthy(obj.get("disableModelInvocation")),
                });
            }
            let tool = match input.get("fileReadTool") {
                None | Some(Value::Null) => FileReadTool::Read,
                Some(Value::String(s)) => match s.as_str() {
                    "read" => FileReadTool::Read,
                    "bash" => FileReadTool::Bash,
                    _ => FileReadTool::Indirect,
                },
                Some(_) => return Err("fileReadTool must be a string".to_string()),
            };
            Ok(json!({ "result": format_skills_for_prompt(&skills, tool) }))
        }
        other => Err(format!("unknown skills op: {other}")),
    }
}

// ============================================================================
// sess-manager
// ============================================================================

/// Rewrite dynamically generated substrings so results from the two sides
/// (real `uuidv7` / `randomUUID` / `new Date` on TS, real OS randomness /
/// clock on Rust — neither side's generators are injectable through the
/// public API) compare equal: full UUIDs → `<uuid>`, ISO timestamps →
/// `<ts>`, filename-form timestamps → `<tsf>`, strings that are exactly
/// 8 lowercase hex chars (the `generateId` shape) → `<id8>`, and quoted
/// 8-hex tokens embedded in raw JSONL strings (readFile) → `"<id8>"`.
fn sess_rewrite_string(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() == 8 && b.iter().all(|&c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
        return "<id8>".to_string();
    }
    let is_d = |c: u8| c.is_ascii_digit();
    let is_h = |c: u8| c.is_ascii_digit() || (b'a'..=b'f').contains(&c);
    let mut out = String::new();
    let mut i = 0usize;
    while i < b.len() {
        let rest = &b[i..];
        // uuid: 8-4-4-4-12 hex groups
        if rest.len() >= 36
            && rest[..8].iter().all(|&c| is_h(c))
            && rest[8] == b'-'
            && rest[9..13].iter().all(|&c| is_h(c))
            && rest[13] == b'-'
            && rest[14..18].iter().all(|&c| is_h(c))
            && rest[18] == b'-'
            && rest[19..23].iter().all(|&c| is_h(c))
            && rest[23] == b'-'
            && rest[24..36].iter().all(|&c| is_h(c))
        {
            out.push_str("<uuid>");
            i += 36;
            continue;
        }
        // ISO timestamp: YYYY-MM-DDTHH:MM:SS.mmmZ
        if rest.len() >= 24
            && rest[..4].iter().all(|&c| is_d(c))
            && rest[4] == b'-'
            && rest[5..7].iter().all(|&c| is_d(c))
            && rest[7] == b'-'
            && rest[8..10].iter().all(|&c| is_d(c))
            && rest[10] == b'T'
            && rest[11..13].iter().all(|&c| is_d(c))
            && rest[13] == b':'
            && rest[14..16].iter().all(|&c| is_d(c))
            && rest[16] == b':'
            && rest[17..19].iter().all(|&c| is_d(c))
            && rest[19] == b'.'
            && rest[20..23].iter().all(|&c| is_d(c))
            && rest[23] == b'Z'
        {
            out.push_str("<ts>");
            i += 24;
            continue;
        }
        // filename timestamp: YYYY-MM-DDTHH-MM-SS-mmmZ
        if rest.len() >= 24
            && rest[..4].iter().all(|&c| is_d(c))
            && rest[4] == b'-'
            && rest[5..7].iter().all(|&c| is_d(c))
            && rest[7] == b'-'
            && rest[8..10].iter().all(|&c| is_d(c))
            && rest[10] == b'T'
            && rest[11..13].iter().all(|&c| is_d(c))
            && rest[13] == b'-'
            && rest[14..16].iter().all(|&c| is_d(c))
            && rest[16] == b'-'
            && rest[17..19].iter().all(|&c| is_d(c))
            && rest[19] == b'-'
            && rest[20..23].iter().all(|&c| is_d(c))
            && rest[23] == b'Z'
        {
            out.push_str("<tsf>");
            i += 24;
            continue;
        }
        // quoted 8-hex id: "deadbeef" (generated entry ids embedded in raw
        // JSONL strings returned by readFile). Quote delimiters cannot occur
        // inside JSON string content (escaped there), so this only matches
        // structural values.
        if rest.len() >= 10
            && rest[0] == b'"'
            && rest[1..9].iter().all(|&c| is_h(c))
            && rest[9] == b'"'
        {
            out.push_str("\"<id8>\"");
            i += 10;
            continue;
        }
        let ch_len = s[i..].chars().next().map(char::len_utf8).unwrap_or(1);
        out.push_str(&s[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Deep-normalize a result: `<base>` for the per-case fixture dir, the
/// dynamic-id/timestamp rewrites, and epoch-ms numbers under a `timestamp`
/// key (`new Date(ts).getTime()` in compaction `systemMessage`) → `<ms>`.
fn sess_normalize(value: &mut Value, base: &str) {
    match value {
        Value::String(s) => {
            let replaced = s.replace(base, "<base>");
            // getDefaultSessionDir encodes the cwd into a safe directory name
            // (slashes → dashes), which escapes the plain <base> rewrite.
            let mangled_base = base.trim_start_matches('/').replace(['/', '\\', ':'], "-");
            let replaced = replaced.replace(&mangled_base, "<base>");
            // Ambient cwd: `open` without cwdOverride falls back to
            // AMBIENT_CWD ("/") here and process.cwd() on the TS side; each
            // side normalizes its own ambient value to <cwd>. The substring
            // form covers the raw JSONL string returned by readFile.
            let replaced = if replaced == "/" {
                "<cwd>".to_string()
            } else {
                replaced.replace("\"cwd\":\"/\"", "\"cwd\":\"<cwd>\"")
            };
            *s = sess_rewrite_string(&replaced);
        }
        Value::Array(items) => {
            for item in items {
                sess_normalize(item, base);
            }
        }
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if k == "timestamp" {
                    if let Some(n) = v.as_f64() {
                        if (1e12..1e13).contains(&n) {
                            *v = Value::String("<ms>".to_string());
                            continue;
                        }
                    }
                }
                sess_normalize(v, base);
            }
        }
        _ => {}
    }
}

/// `sess-manager` harness: `pi_core::session_manager` (TS
/// `core/session-manager.js`). args `{ op, files?, mtimes?, ... }`; `files`
/// is materialized under a per-case temp base and `mtimes` sets explicit
/// file mtimes (epoch ms) so `findMostRecentSession` ordering is
/// deterministic — readdir order is never asserted. All results are
/// `<base>`-normalized and dynamic ids/timestamps rewritten (see
/// [`sess_normalize`]): neither side can inject `uuidv7` / `randomUUID` /
/// `new Date`, so ambient values are normalized on BOTH sides.
///
/// Ops:
/// - pure: `parseSessionEntries` {content}, `migrateSessionEntries`
///   {entries} (mutated in place), `getLatestCompactionEntry` {entries},
///   `sessionEntryToContextMessages` {entry}, `buildContextEntries` /
///   `buildSessionProjection` / `buildSessionContext` {entries, leafId?}
///   (`leafId` absent → last entry, `null` → root), `assertValidSessionId`
///   {id} (throws in-band), `getDefaultSessionDir` {cwd, agentDir}.
/// - fs: `loadEntriesFromFile` {files, filePath}, `findMostRecentSession`
///   {files, mtimes, sessionDir, cwd?}.
/// - `manager` {factory, ..., ops}: construct a SessionManager (`create` /
///   `open` / `continueRecent` / `inMemory` / `forkFrom`, all paths relative
///   to the base) and run method sub-ops; per-op errors are in-band
///   `{thrown}`. `readFile`/`listDir` observe the persisted bytes (byte-level
///   JSONL compare after normalization), `usesDefaultSessionDir` compares
///   against `getDefaultSessionDir(cwd, agentDir)` with an explicit
///   `agentDir` (the TS ambient default is not observable).
fn run_sess_manager_harness(input: &Value) -> Result<Value, String> {
    use pi_core::session_manager as sm;
    use pi_core::session_manager::{Generators, LeafId};

    let op = input["op"].as_str().ok_or("missing op")?;
    validate_files(input.get("files"))?;
    // TS does `fs.utimesSync(rel(p), new Date(ms), new Date(ms))`; for
    // non-integer values `new Date(ms)` is an Invalid Date and utimesSync is
    // a silent no-op, so non-integer entries are skipped rather than errored.
    let mtimes: Vec<(String, i64)> = match input.get("mtimes") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(map)) => map
            .iter()
            .filter_map(|(k, v)| v.as_i64().map(|ms| (k.clone(), ms)))
            .collect(),
        Some(_) => return Err("mtimes must be an object".to_string()),
    };
    let base = create_fixture(input.get("files"), "sess")?;
    let base_s = base.to_string_lossy().into_owned();
    for (rel, ms) in &mtimes {
        let path = join_rel(&base_s, rel);
        let f = std::fs::OpenOptions::new().write(true).open(&path).map_err(|e| e.to_string())?;
        f.set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(*ms as u64)),
        )
        .map_err(|e| e.to_string())?;
    }
    let gen = Generators::default();
    let result: Result<Value, String> = (|| {
        let rel = |p: &str| join_rel(&base_s, p);
        fn leaf(v: Option<&Value>) -> LeafId<'static> {
            match v {
                None | Some(Value::Null) => LeafId::Root,
                Some(Value::String(s)) => LeafId::Id(s.to_string().leak()),
                Some(_) => panic!("leafId must be a string or null"),
            }
        }
        fn leaf_unset() -> LeafId<'static> { LeafId::Unset }
        let messages_value = |msgs: &[pi_core::messages::AgentMessage]| -> Value {
            serde_json::to_value(msgs).map_err(|e| e.to_string()).unwrap()
        };
        let projection_value = |p: &sm::SessionProjection| -> Value {
            json!({
                "entries": p.entries.iter().map(|e| json!({
                    "sourceEntry": e.source_entry,
                    "messages": e.messages,
                })).collect::<Vec<_>>(),
                "messages": p.messages,
                "thinkingLevel": p.thinking_level,
                "model": p.model,
            })
        };
        let context_value = |c: &sm::SessionContext| -> Value {
            json!({
                "messages": c.messages,
                "thinkingLevel": c.thinking_level,
                "model": c.model,
            })
        };
        match op {
            "parseSessionEntries" => {
                let content = input["content"].as_str().ok_or("missing content")?;
                Ok(json!({ "entries": sm::parse_session_entries(content) }))
            }
            "migrateSessionEntries" => {
                let mut entries = sess_entries(input)?;
                sm::migrate_session_entries(&mut entries);
                Ok(json!({ "entries": entries }))
            }
            "getLatestCompactionEntry" => {
                let entries = sess_entries(input)?;
                Ok(json!({ "entry": sm::get_latest_compaction_entry(&entries) }))
            }
            "sessionEntryToContextMessages" => {
                let entry = input.get("entry").filter(|v| !v.is_null()).cloned().ok_or("missing entry")?;
                Ok(json!({ "messages": messages_value(&sm::session_entry_to_context_messages(&entry)) }))
            }
            "buildContextEntries" => {
                let entries = sess_entries(input)?;
                let l = if input.get("leafId").is_none() { leaf_unset() } else { leaf(input.get("leafId")) };
                Ok(json!({ "entries": sm::build_context_entries(&entries, l, None) }))
            }
            "buildSessionProjection" => {
                let entries = sess_entries(input)?;
                let l = if input.get("leafId").is_none() { leaf_unset() } else { leaf(input.get("leafId")) };
                Ok(projection_value(&sm::build_session_projection(&entries, l, None)))
            }
            "buildSessionContext" => {
                let entries = sess_entries(input)?;
                let l = if input.get("leafId").is_none() { leaf_unset() } else { leaf(input.get("leafId")) };
                Ok(context_value(&sm::build_session_context(&entries, l, None)))
            }
            "assertValidSessionId" => {
                let id = input["id"].as_str().ok_or("missing id")?;
                sm::assert_valid_session_id(id)?;
                Ok(json!({ "valid": true }))
            }
            "getDefaultSessionDir" => {
                let cwd = rel(input["cwd"].as_str().ok_or("missing cwd")?);
                let agent_dir = rel(input["agentDir"].as_str().ok_or("missing agentDir")?);
                Ok(json!({ "dir": sm::get_default_session_dir(&cwd, &agent_dir) }))
            }
            "loadEntriesFromFile" => {
                let file_path = input["filePath"].as_str().ok_or("missing filePath")?;
                Ok(json!({ "entries": sm::load_entries_from_file(&rel(file_path)) }))
            }
            "findMostRecentSession" => {
                let session_dir = input["sessionDir"].as_str().ok_or("missing sessionDir")?;
                let cwd = input.get("cwd").and_then(Value::as_str);
                Ok(json!({ "path": sm::find_most_recent_session(&rel(session_dir), cwd) }))
            }
            "manager" => sess_manager_flow(input, &base_s, &gen),
            other => Err(format!("unknown sess-manager op: {other}")),
        }
    })();
    let _ = std::fs::remove_dir_all(&base);
    // Factory-level errors (e.g. the "not a valid pi session" path) embed
    // the per-case temp base; normalize them like results so both harnesses
    // compare equal ({error: <normalized>} on both sides).
    let mut value = match result {
        Ok(v) => json!({ "result": v }),
        Err(msg) => json!({ "error": msg }),
    };
    sess_normalize(&mut value, &base_s);
    Ok(value)
}

fn sess_entries(input: &Value) -> Result<Vec<Value>, String> {
    input["entries"].as_array().cloned().ok_or("missing entries".to_string())
}

/// The `manager` sub-harness: construction + scripted method calls.
fn sess_manager_flow(input: &Value, base_s: &str, gen: &pi_core::session_manager::Generators) -> Result<Value, String> {
    use pi_core::session_manager::{NewSessionOptions, SessionManager};

    let rel = |p: &str| join_rel(base_s, p);
    let new_options = |v: &Value| -> Option<NewSessionOptions> {
        let id = v.get("id").and_then(Value::as_str).map(str::to_string);
        let parent = v.get("parentSession").and_then(Value::as_str).map(str::to_string);
        if id.is_none() && parent.is_none() {
            None
        } else {
            Some(NewSessionOptions { id, parent_session: parent })
        }
    };
    let factory = input["factory"].as_str().unwrap_or("create");
    let mut mgr: SessionManager = match factory {
        "create" => SessionManager::create(
            &rel(input["cwd"].as_str().unwrap_or("proj")),
            Some(&rel(input["sessionDir"].as_str().unwrap_or("sess"))),
            new_options(input).as_ref(),
            gen,
        )?,
        "open" => SessionManager::open(
            &rel(input["path"].as_str().ok_or("missing path")?),
            input["sessionDir"].as_str().map(rel).as_deref(),
            input["cwdOverride"].as_str().map(rel).as_deref(),
            gen,
        )?,
        "continueRecent" => SessionManager::continue_recent(
            &rel(input["cwd"].as_str().unwrap_or("proj")),
            Some(&rel(input["sessionDir"].as_str().unwrap_or("sess"))),
            base_s,
            gen,
        )?,
        "inMemory" => SessionManager::in_memory(
            &rel(input["cwd"].as_str().unwrap_or("proj")),
            new_options(input).as_ref(),
            input.get("entries").and_then(Value::as_array).cloned().unwrap_or_default(),
            gen,
        )?,
        "forkFrom" => SessionManager::fork_from(
            &rel(input["sourcePath"].as_str().ok_or("missing sourcePath")?),
            &rel(input["targetCwd"].as_str().unwrap_or("proj")),
            Some(&rel(input["sessionDir"].as_str().unwrap_or("sess"))),
            new_options(input).as_ref(),
            gen,
        )?,
        other => return Err(format!("unknown sess-manager factory: {other}")),
    };
    let ops: Vec<Value> = input["ops"].as_array().cloned().ok_or("missing ops")?;
    let mut out: Vec<Value> = Vec::new();
    for sop in &ops {
        let entry: Result<Value, String> = (|| {
            let kind = sop["op"].as_str().ok_or("missing op")?;
            let opt_str = |key: &str| sop[key].as_str();
            match kind {
                "getCwd" => Ok(json!(mgr.get_cwd())),
                "getSessionDir" => Ok(json!(mgr.get_session_dir())),
                "getSessionId" => Ok(json!(mgr.get_session_id())),
                "getSessionFile" => Ok(mgr.get_session_file().map(|s| json!(s)).unwrap_or(Value::Null)),
                "getLeafId" => Ok(mgr.get_leaf_id().map(|s| json!(s)).unwrap_or(Value::Null)),
                "getLeafEntry" => Ok(mgr.get_leaf_entry().cloned().unwrap_or(Value::Null)),
                "getEntry" => {
                    let id = opt_str("id").ok_or("missing id")?;
                    Ok(mgr.get_entry(id).cloned().unwrap_or(Value::Null))
                }
                "getChildren" => {
                    let id = opt_str("id").ok_or("missing id")?;
                    Ok(json!(mgr.get_children(id)))
                }
                "getLabel" => {
                    let id = opt_str("id").ok_or("missing id")?;
                    Ok(mgr.get_label(id).map(|s| json!(s)).unwrap_or(Value::Null))
                }
                "getEntryCount" => Ok(json!(mgr.get_entry_count())),
                "isPersisted" => Ok(json!(mgr.is_persisted())),
                "getSessionName" => Ok(mgr.get_session_name().map(|s| json!(s)).unwrap_or(Value::Null)),
                "getHeader" => Ok(mgr.get_header().cloned().unwrap_or(Value::Null)),
                "getEntries" => Ok(json!(mgr.get_entries())),
                "getTree" => Ok(serde_json::to_value(mgr.get_tree()).map_err(|e| e.to_string())?),
                "getBranch" => Ok(json!(mgr.get_branch(opt_str("fromId")))),
                "buildContextEntries" => Ok(json!(mgr.build_context_entries())),
                "buildSessionProjection" => {
                    let p = mgr.build_session_projection();
                    Ok(json!({
                        "entries": p.entries.iter().map(|e| json!({
                            "sourceEntry": e.source_entry,
                            "messages": e.messages,
                        })).collect::<Vec<_>>(),
                        "messages": p.messages,
                        "thinkingLevel": p.thinking_level,
                        "model": p.model,
                    }))
                }
                "buildSessionContext" => {
                    let c = mgr.build_session_context();
                    Ok(json!({
                        "messages": c.messages,
                        "thinkingLevel": c.thinking_level,
                        "model": c.model,
                    }))
                }
                "newSession" => {
                    let opts = new_options(sop);
                    Ok(mgr.new_session(opts.as_ref())?.map(|s| json!(s)).unwrap_or(Value::Null))
                }
                "setSessionFile" => {
                    let path = opt_str("path").ok_or("missing path")?;
                    mgr.set_session_file(&rel(path))?;
                    Ok(Value::Null)
                }
                "appendMessage" => {
                    let message = sop.get("message").filter(|v| !v.is_null()).cloned().ok_or("missing message")?;
                    Ok(json!(mgr.append_message(message)))
                }
                "appendThinkingLevelChange" => {
                    let level = opt_str("thinkingLevel").ok_or("missing thinkingLevel")?;
                    Ok(json!(mgr.append_thinking_level_change(level)))
                }
                "appendModelChange" => {
                    let provider = opt_str("provider").ok_or("missing provider")?;
                    let model_id = opt_str("modelId").ok_or("missing modelId")?;
                    Ok(json!(mgr.append_model_change(provider, model_id)))
                }
                "appendUsage" => {
                    let kind = opt_str("kind").ok_or("missing kind")?;
                    let provider = opt_str("provider").ok_or("missing provider")?;
                    let model = opt_str("model").ok_or("missing model")?;
                    let usage = sop.get("usage").filter(|v| !v.is_null()).cloned().ok_or("missing usage")?;
                    let note = opt_str("note");
                    Ok(mgr.append_usage(kind, provider, model, usage, note))
                }
                "appendCompaction" => {
                    let summary = opt_str("summary").ok_or("missing summary")?;
                    let first_kept = opt_str("firstKeptEntryId");
                    let tokens = sop["tokensBefore"].as_i64().ok_or("missing tokensBefore")?;
                    let details = sop.get("details").filter(|v| !v.is_null()).cloned();
                    let from_hook = sop.get("fromHook").and_then(Value::as_bool);
                    let usage = sop.get("usage").filter(|v| !v.is_null()).cloned();
                    Ok(json!(mgr.append_compaction(summary, first_kept, tokens, details, from_hook, usage)))
                }
                "appendCustomEntry" => {
                    let custom_type = opt_str("customType").ok_or("missing customType")?;
                    let data = sop.get("data").filter(|v| !v.is_null()).cloned();
                    Ok(json!(mgr.append_custom_entry(custom_type, data)))
                }
                "appendSessionInfo" => {
                    let name = opt_str("name").ok_or("missing name")?;
                    Ok(json!(mgr.append_session_info(name)))
                }
                "appendCustomMessageEntry" => {
                    let custom_type = opt_str("customType").ok_or("missing customType")?;
                    let content = sop.get("content").filter(|v| !v.is_null()).cloned().ok_or("missing content")?;
                    let display = sop.get("display").and_then(Value::as_bool);
                    let details = sop.get("details").filter(|v| !v.is_null()).cloned();
                    Ok(json!(mgr.append_custom_message_entry(custom_type, content, display, details)))
                }
                "appendContextEdit" => {
                    let target = opt_str("targetId").ok_or("missing targetId")?;
                    let replacement = sop.get("replacement").filter(|v| !v.is_null()).cloned();
                    Ok(json!(mgr.append_context_edit(target, replacement)?))
                }
                "appendLabelChange" => {
                    let target = opt_str("targetId").ok_or("missing targetId")?;
                    let label = opt_str("label");
                    Ok(json!(mgr.append_label_change(target, label)?))
                }
                "branch" => {
                    let id = opt_str("id").ok_or("missing id")?;
                    mgr.branch(id)?;
                    Ok(Value::Null)
                }
                "resetLeaf" => {
                    mgr.reset_leaf();
                    Ok(Value::Null)
                }
                "branchWithSummary" => {
                    let from = opt_str("branchFromId");
                    let summary = opt_str("summary").ok_or("missing summary")?;
                    let details = sop.get("details").filter(|v| !v.is_null()).cloned();
                    let from_hook = sop.get("fromHook").and_then(Value::as_bool);
                    let usage = sop.get("usage").filter(|v| !v.is_null()).cloned();
                    Ok(json!(mgr.branch_with_summary(from, summary, details, from_hook, usage)?))
                }
                "createBranchedSession" => {
                    let leaf = opt_str("leafId").ok_or("missing leafId")?;
                    Ok(mgr.create_branched_session(leaf)?.map(|s| json!(s)).unwrap_or(Value::Null))
                }
                "readFile" => {
                    let path = match opt_str("path") {
                        Some(p) => rel(p),
                        None => mgr.get_session_file().ok_or("no session file")?.to_string(),
                    };
                    Ok(std::fs::read_to_string(&path).ok().map(Value::String).unwrap_or(Value::Null))
                }
                "listDir" => {
                    let dir = match opt_str("dir") {
                        Some(d) => rel(d),
                        None => mgr.get_session_dir().to_string(),
                    };
                    let read = std::fs::read_dir(&dir).map_err(|e| {
                        if e.kind() == std::io::ErrorKind::NotFound {
                            // Node's fs.readdirSync throws this exact message.
                            format!("ENOENT: no such file or directory, scandir '{}'", dir)
                        } else {
                            e.to_string()
                        }
                    })?;
                    let mut names: Vec<String> = read
                        .filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect();
                    names.sort();
                    Ok(json!(names))
                }
                "usesDefaultSessionDir" => {
                    let agent_dir = rel(opt_str("agentDir").unwrap_or("agent"));
                    Ok(json!(mgr.get_session_dir() == pi_core::session_manager::get_default_session_dir(mgr.get_cwd(), &agent_dir)))
                }
                other => Err(format!("unknown sess-manager op: {other}")),
            }
        })();
        out.push(match entry {
            Ok(v) => v,
            Err(msg) => json!({ "thrown": msg }),
        });
    }
    Ok(json!({ "out": out }))
}

// ============================================================================
// nested-tool-calls / pi-manifest / output-guard / defaults
// ============================================================================

/// `nested-tool-calls` harness: script-driven
/// `pi_core::nested_tool_calls::NestedCallRecorder` (TS
/// `core/nested-tool-calls.js`). args `{ ops: [...] }` → `{ out: [...] }`.
/// `start` returns a record handle (its index; `null` when dropped past
/// `maxCalls`), `finish` takes it back. `durationMs` is zeroed on both
/// sides (real `performance.now()` / `Instant` differ); `addUsage` inputs
/// must be complete `Usage` objects (the TS arithmetic is NaN on missing
/// fields).
fn run_nested_tool_calls_harness(input: &Value) -> Result<Value, String> {
    use pi_core::ai::Usage;
    use pi_core::nested_tool_calls::{AgentToolCall, NestedCallRecorder, NESTED_CALL_LIMITS};

    let ops: Vec<Value> = input["ops"].as_array().cloned().ok_or("missing ops")?;
    let mut rec = NestedCallRecorder::new();
    let mut out: Vec<Value> = Vec::new();
    for op in &ops {
        let entry: Result<Value, String> = (|| {
            let kind = op["op"].as_str().ok_or("missing op")?;
            match kind {
                "start" => {
                    let id = op["id"].as_str().ok_or("missing id")?;
                    let name = op["name"].as_str().ok_or("missing name")?;
                    let arguments = op.get("arguments").cloned().unwrap_or(Value::Null);
                    let call = AgentToolCall::new(id, name, arguments);
                    Ok(match rec.start(&call) {
                        Some(h) => json!(h),
                        None => Value::Null,
                    })
                }
                "finish" => {
                    let record = op.get("record").and_then(Value::as_u64).map(|h| h as usize);
                    let is_error = op["isError"].as_bool().unwrap_or(false);
                    let error_text = op["errorText"].as_str().unwrap_or("");
                    rec.finish(record, is_error, error_text);
                    Ok(Value::Null)
                }
                "addUsage" => {
                    let usage = op.get("usage").filter(|v| !v.is_null()).ok_or("missing usage")?;
                    rec.add_usage(Usage::from_value(usage));
                    Ok(Value::Null)
                }
                "totalUsage" => Ok(rec.total_usage().map(|u| u.to_value()).unwrap_or(Value::Null)),
                "snapshot" => {
                    let mut snap = rec
                        .snapshot()
                        .map(|s| serde_json::to_value(s).map_err(|e| e.to_string()))
                        .transpose()?
                        .unwrap_or(Value::Null);
                    ntc_zero_durations(&mut snap);
                    Ok(snap)
                }
                "limits" => Ok(json!({
                    "maxCalls": NESTED_CALL_LIMITS.max_calls,
                    "maxArgumentBytesPerCall": NESTED_CALL_LIMITS.max_argument_bytes_per_call,
                    "maxArgumentBytesTotal": NESTED_CALL_LIMITS.max_argument_bytes_total,
                    "maxErrorChars": NESTED_CALL_LIMITS.max_error_chars,
                })),
                other => Err(format!("unknown nested-tool-calls op: {other}")),
            }
        })();
        out.push(match entry {
            Ok(v) => v,
            Err(msg) => json!({ "thrown": msg }),
        });
    }
    Ok(json!({ "result": { "out": out } }))
}

/// Zero every `durationMs` (timing-dependent) in a snapshot value.
fn ntc_zero_durations(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if k == "durationMs" && !v.is_null() {
                    *v = json!(0);
                } else {
                    ntc_zero_durations(v);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                ntc_zero_durations(item);
            }
        }
        _ => {}
    }
}

/// `pi-manifest` harness: `pi_core::pi_manifest::read_pi_manifest` (TS
/// `core/pi-manifest.js`, which reads from a file path; the TS entry writes
/// `content` to a temp `package.json` first, the Rust port takes content).
fn run_pi_manifest_harness(input: &Value) -> Result<Value, String> {
    use pi_core::pi_manifest::read_pi_manifest;
    let op = input["op"].as_str().ok_or("missing op")?;
    match op {
        "read" => {
            let content = input["content"].as_str().ok_or("missing content")?;
            Ok(json!({ "result": read_pi_manifest(content) }))
        }
        other => Err(format!("unknown pi-manifest op: {other}")),
    }
}

/// `defaults` harness: `pi_core::defaults` constants (TS
/// `core/defaults.js` — `DEFAULT_THINKING_LEVEL` / `THINKING_LEVEL_OPTIONS`).
fn run_defaults_harness(input: &Value) -> Result<Value, String> {
    use pi_core::defaults::{DEFAULT_THINKING_LEVEL, THINKING_LEVEL_OPTIONS};
    let op = input["op"].as_str().ok_or("missing op")?;
    match op {
        "constants" => Ok(json!({
            "result": {
                "defaultThinkingLevel": DEFAULT_THINKING_LEVEL.as_str(),
                "options": THINKING_LEVEL_OPTIONS.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
            }
        })),
        other => Err(format!("unknown defaults op: {other}")),
    }
}

/// `output-guard` harness: script-driven `pi_core::output_guard::OutputGuard`
/// (TS `core/output-guard.js`, which is process-global). The stdout write is
/// a scripted recorder (`stdoutScript`: per-call `null` or
/// `{code, message}` errors) shared by both entries; writes are recorded per
/// attempt. `writeRawStdout` failures are stashed and surface at
/// `waitForRawStdoutBackpressure` / `flushRawStdout`, mirroring the TS
/// promise-tail error propagation (a non-retryable raw failure kills the
/// Node process there; `process.exit` is neutralized on the TS side).
fn run_output_guard_harness(input: &Value) -> Result<Value, String> {
    use pi_core::output_guard::{raw_write, OutputGuard};
    use std::cell::RefCell;
    use std::rc::Rc;

    let ops: Vec<Value> = input["ops"].as_array().cloned().ok_or("missing ops")?;
    let script_in = |key: &str| -> Result<Vec<Option<(String, String)>>, String> {
        match input.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| match v {
                    Value::Null => Ok(None),
                    Value::Object(o) => {
                        let code = o.get("code").and_then(Value::as_str).ok_or("stdoutScript entries need a code")?;
                        let message = o.get("message").and_then(Value::as_str).ok_or("stdoutScript entries need a message")?;
                        Ok(Some((code.to_string(), message.to_string())))
                    }
                    _ => Err("stdoutScript entries must be null or {code, message}".to_string()),
                })
                .collect(),
            Some(_) => Err("stdoutScript must be an array".to_string()),
        }
    };
    let stdout_script = script_in("stdoutScript")?;
    let stderr_script = script_in("stderrScript")?;

    type Log = Rc<RefCell<Vec<String>>>;
    type Script = Rc<RefCell<Vec<Option<(String, String)>>>>;
    let mk_writer = |log: Log, script: Script| {
        raw_write(move |text| {
            log.borrow_mut().push(text.to_string());
            let step = {
                let mut s = script.borrow_mut();
                if s.is_empty() {
                    None
                } else {
                    s.remove(0)
                }
            };
            match step {
                Some((code, message)) => Err(pi_core::output_guard::RawWriteError::new(Some(&code), message)),
                None => Ok(()),
            }
        })
    };
    let stdout_log: Log = Rc::new(RefCell::new(Vec::new()));
    let stderr_log: Log = Rc::new(RefCell::new(Vec::new()));
    let stdout_write = mk_writer(Rc::clone(&stdout_log), Rc::new(RefCell::new(stdout_script)));
    let stderr_write = mk_writer(Rc::clone(&stderr_log), Rc::new(RefCell::new(stderr_script)));
    let mut guard = OutputGuard::new(stdout_write, stderr_write);
    let mut pending_error: Option<String> = None;
    // The TS chains raw writes onto a promise tail: queued chunks flush on a
    // microtask after the synchronous ops, or when waitFor/flush awaits the
    // tail. Emulate with a queue drained at waitFor/flush and at the end
    // (the TS entry's final `await waitForRawStdoutBackpressure()`).
    let mut raw_queue: Vec<String> = Vec::new();
    let drain_raw = |guard: &mut OutputGuard, queue: &mut Vec<String>, pending_error: &mut Option<String>| {
        while pending_error.is_none() {
            let Some(text) = queue.first().cloned() else { break };
            queue.remove(0);
            if let Err(e) = guard.write_raw_stdout(&text) {
                *pending_error = Some(e.message);
            }
        }
    };

    let mut out: Vec<Value> = Vec::new();
    for op in &ops {
        let entry: Result<Value, String> = (|| {
            let kind = op["op"].as_str().ok_or("missing op")?;
            match kind {
                "takeOverStdout" => {
                    guard.take_over_stdout();
                    Ok(Value::Null)
                }
                "restoreStdout" => {
                    guard.restore_stdout();
                    Ok(Value::Null)
                }
                "isStdoutTakenOver" => Ok(json!(guard.is_stdout_taken_over())),
                "writeStdout" => {
                    let text = op["text"].as_str().ok_or("missing text")?;
                    guard.write_stdout(text).map_err(|e| e.message)?;
                    Ok(Value::Null)
                }
                "writeRawStdout" => {
                    let text = op["text"].as_str().ok_or("missing text")?;
                    // Empty text returns before chaining; a dead (rejected)
                    // tail swallows later chunks entirely.
                    if pending_error.is_none() && !text.is_empty() {
                        raw_queue.push(text.to_string());
                    }
                    Ok(Value::Null)
                }
                "flushRawStdout" => {
                    drain_raw(&mut guard, &mut raw_queue, &mut pending_error);
                    if let Some(msg) = &pending_error {
                        return Err(msg.clone());
                    }
                    guard.flush_raw_stdout().map_err(|e| e.message)?;
                    Ok(Value::Null)
                }
                "waitForRawStdoutBackpressure" => {
                    drain_raw(&mut guard, &mut raw_queue, &mut pending_error);
                    if let Some(msg) = &pending_error {
                        return Err(msg.clone());
                    }
                    guard.wait_for_raw_stdout_backpressure();
                    Ok(Value::Null)
                }
                other => Err(format!("unknown output-guard op: {other}")),
            }
        })();
        out.push(match entry {
            Ok(v) => v,
            Err(msg) => json!({ "thrown": msg }),
        });
    }
    drain_raw(&mut guard, &mut raw_queue, &mut pending_error);
    Ok(json!({
        "result": {
            "out": out,
            "stdoutWrites": *stdout_log.borrow(),
            "stderrWrites": *stderr_log.borrow(),
        }
    }))
}
