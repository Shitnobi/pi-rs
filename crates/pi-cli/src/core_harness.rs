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
