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
