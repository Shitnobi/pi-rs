//! `extension-runner` parity harness: drives the pi-session
//! `extension_runner` port (loader + runner core slice) with a script of
//! ops, mirroring parity/ts-harness.js's "extension-runner" entry 1:1.
//!
//! Extensions load from scripted factories (`loadExtensionFromFactory` with
//! a shared `ExtensionRuntime` + event bus). Handler/factory scripts are
//! mirrored on the TS side; only the seams outside the ported slice
//! (runtime actions, context actions, provider hooks, model registry) are
//! fakes, recording identical log lines on both sides.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pi_core::event_bus::{create_event_bus_with_sink, EventBusController};
use pi_session::agent_session::extension_runner::*;
use serde_json::{json, Value};

type Log = Arc<Mutex<Vec<String>>>;

fn jstr(value: &Value) -> String {
	serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

fn str_list(spec: &Value, key: &str) -> Result<Vec<String>, String> {
	match spec.get(key) {
		None => Ok(Vec::new()),
		Some(Value::Array(list)) => list
			.iter()
			.map(|v| v.as_str().map(str::to_string).ok_or_else(|| format!("{key} entries must be strings")))
			.collect(),
		Some(_) => Err(format!("{key} must be an array")),
	}
}

struct CaseInner {
	log: Log,
	warns: Arc<Mutex<Vec<String>>>,
	bus: Arc<EventBusController>,
	runtime: ExtensionRuntime,
	extensions: Mutex<Vec<Arc<Mutex<Extension>>>>,
	captured: Mutex<HashMap<String, ExtensionApi>>,
	unsubs: Mutex<Vec<HandlerToken>>,
	runner: Mutex<Option<ExtensionRunner>>,
	next_error_id: std::sync::atomic::AtomicU64,
}

fn make_handler(spec: &Value, log: &Log) -> Result<ExtensionHandler, String> {
	if spec.get("id").and_then(Value::as_u64).is_none() {
		return Err("missing handler id".to_string());
	}
	let id = spec["id"].as_u64().unwrap();
	let returns = spec.get("returns").cloned();
	let throws = spec.get("throws").and_then(Value::as_str).map(str::to_string);
	let reads = str_list(spec, "reads")?;
	let calls = str_list(spec, "calls")?;
	let log = Arc::clone(log);
	Ok(Arc::new(move |event: Value, ctx: CtxView| {
		let log = Arc::clone(&log);
		let returns = returns.clone();
		let throws = throws.clone();
		let reads = reads.clone();
		let calls = calls.clone();
		Box::pin(async move {
			let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
			log.lock().unwrap().push(format!("h{}:{}", id, event_type));
			for field in &reads {
				match ctx.read(field) {
					Ok(value) => log.lock().unwrap().push(format!("h{}.{}={}", id, field, jstr(&value))),
					Err(message) => log.lock().unwrap().push(format!("h{}.{}!{}", id, field, message)),
				}
			}
			for method in &calls {
				match ctx.call(method) {
					Ok(_) => log.lock().unwrap().push(format!("h{}.{}:ok", id, method)),
					Err(message) => log.lock().unwrap().push(format!("h{}.{}!{}", id, method, message)),
				}
			}
			if let Some(message) = throws {
				return Err(message);
			}
			Ok(returns)
		})
	}))
}

fn make_bus_handler(spec: &Value, log: &Log) -> Result<pi_core::event_bus::EventHandler, String> {
	if spec.get("id").and_then(Value::as_u64).is_none() {
		return Err("missing bus handler id".to_string());
	}
	let id = spec["id"].as_u64().unwrap();
	let throws = spec.get("throws").and_then(Value::as_str).map(str::to_string);
	let log = Arc::clone(log);
	Ok(Arc::new(move |data: &Value| {
		log.lock().unwrap().push(format!("bus:{}:{}", id, jstr(data)));
		if let Some(message) = throws.as_deref() {
			return Err(message.to_string());
		}
		Ok(())
	}))
}

/// Run one factory/api script step sequence. `Err` models a throw (the load
/// fails, or the api-op wrapper records it).
fn run_script(api: &ExtensionApi, script: &[Value], case: &CaseInner) -> Result<(), String> {
	for step in script {
		let kind = step.get("step").and_then(Value::as_str).ok_or("missing step")?;
		let str_arg = |key: &str| -> Result<String, String> {
			step.get(key).and_then(Value::as_str).map(str::to_string).ok_or_else(|| format!("missing {key}"))
		};
		match kind {
			"on" => {
				let event = str_arg("event")?;
				let spec = step.get("handler").ok_or("missing handler")?;
				let token = api.on(&event, make_handler(spec, &case.log)?)?;
				case.unsubs.lock().unwrap().push(token);
			}
			"off" => {
				let index = step.get("index").and_then(Value::as_u64).ok_or("missing index")? as usize;
				let unsubs = case.unsubs.lock().unwrap();
				let token = unsubs.get(index).ok_or("bad off index")?;
				token.unsubscribe();
			}
			"registerFlag" => api.register_flag(&str_arg("name")?, step.get("options").unwrap_or(&json!({})))?,
			"registerTool" => api.register_tool(step.get("tool").ok_or("missing tool")?)?,
			"registerCommand" => {
				let name = str_arg("name")?;
				let options = step.get("options").cloned().unwrap_or(json!({}));
				let handler = options.get("handler").and_then(Value::as_bool).filter(|v| *v).map(|_| {
					Arc::new(|_args: String, _ctx: CtxView| {
						Box::pin(async { Ok(()) }) as pi_agent::types::BoxFuture<'static, Result<(), String>>
					}) as CommandHandler
				});
				api.register_command(&name, &options, handler)?
			}
			"registerShortcut" => api.register_shortcut(&str_arg("shortcut")?, step.get("options").unwrap_or(&json!({})))?,
			"registerProvider" => {
				let name = str_arg("name")?;
				let config = step.get("config").ok_or("missing config")?;
				api.register_provider(&name, Some(config))?
			}
			"registerProviderNative" => api.register_native_provider(step.get("provider").ok_or("missing provider")?)?,
			"unregisterProvider" => api.unregister_provider(&str_arg("name")?)?,
			"registerVirtualModel" => api.register_virtual_model(step.get("definition").ok_or("missing definition")?)?,
			"unregisterVirtualModel" => {
				api.unregister_virtual_model(&str_arg("provider")?, &str_arg("id")?)?
			}
			"registerMcpServer" => api.register_mcp_server(&str_arg("name")?, step.get("config").ok_or("missing config")?)?,
			"unregisterMcpServer" => api.unregister_mcp_server(&str_arg("name")?)?,
			"getMcpServers" => {
				let servers = api.get_mcp_servers()?;
				case.log.lock().unwrap().push(format!("api.getMcpServers:{}", jstr(&json!(servers))));
			}
			"eventsEmit" => {
				let channel = str_arg("channel")?;
				let data = step.get("data").cloned().unwrap_or(Value::Null);
				api.events_emit(&channel, &data)?;
			}
			"eventsOn" => {
				let channel = str_arg("channel")?;
				let spec = step.get("handler").ok_or("missing handler")?;
				api.events_on(&channel, make_bus_handler(spec, &case.log)?)?;
			}
			"getFlag" => {
				let value = api.get_flag(&str_arg("name")?)?;
				case.log.lock().unwrap().push(format!("api.getFlag:{}", jstr(&value.unwrap_or(Value::Null))));
			}
			"getActiveTools" => {
				let tools = api.get_active_tools()?;
				case.log.lock().unwrap().push(format!("api.getActiveTools:{}", jstr(&json!(tools))));
			}
			"captureApi" => {
				let key = str_arg("key")?;
				case.captured.lock().unwrap().insert(key, api.clone());
			}
			"throw" => return Err(str_arg("message")?),
			"try" => {
				let inner = step.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
				if let Err(message) = run_script(api, &inner, case) {
					case.log.lock().unwrap().push(format!("caught:{}", message));
				}
			}
			other => return Err(format!("unknown step: {}", other)),
		}
	}
	Ok(())
}

struct FakeRegistry {
	log: Log,
	throw_on: Vec<String>,
	message: String,
}

impl FakeRegistry {
	fn record(&self, method: &str, args: &Value) -> Result<(), String> {
		if self.throw_on.iter().any(|m| m == method) {
			return Err(self.message.clone());
		}
		self.log.lock().unwrap().push(format!("mr.{}:{}", method, jstr(args)));
		Ok(())
	}
}

impl ModelRegistrySeam for FakeRegistry {
	fn register_provider(&self, name: &str, config: &Value) -> Result<(), String> {
		self.record("registerProvider", &json!([name, config]))
	}
	fn register_native_provider(&self, provider: &Value) -> Result<(), String> {
		// TS bindCore routes native providers through registerProvider.
		self.record("registerProvider", &json!([provider]))
	}
	fn register_virtual_model(&self, definition: &Value) -> Result<(), String> {
		self.record("registerVirtualModel", &json!([definition]))
	}
	fn unregister_provider(&self, name: &str) -> Result<(), String> {
		self.record("unregisterProvider", &json!([name]))
	}
	fn unregister_virtual_model(&self, provider: &str, id: &str) -> Result<(), String> {
		self.record("unregisterVirtualModel", &json!([provider, id]))
	}
}

struct FakeActions {
	tools: Vec<String>,
	thinking: Value,
	log: Log,
}

impl RuntimeActions for FakeActions {
	fn get_active_tools(&self) -> Vec<String> {
		self.tools.clone()
	}
	fn get_thinking_level(&self) -> Value {
		self.thinking.clone()
	}
	fn refresh_tools(&self) {
		self.log.lock().unwrap().push("actions.refreshTools".to_string());
	}
}

struct FakeContext {
	idle: bool,
	trusted: bool,
	model: Option<Value>,
	log: Log,
}

impl ContextActions for FakeContext {
	fn is_idle(&self) -> bool {
		self.idle
	}
	fn is_project_trusted(&self) -> bool {
		self.trusted
	}
	fn get_model(&self) -> Option<Value> {
		self.model.clone()
	}
	fn shutdown(&self) {
		self.log.lock().unwrap().push("contextActions.shutdown".to_string());
	}
}

fn pa_hook(
	log: &Log,
	method: &'static str,
	mode: &Value,
	args: Value,
) -> Result<(), String> {
	if mode.as_str() == Some("throw") {
		return Err("pa boom".to_string());
	}
	log.lock().unwrap().push(format!("pa.{}:{}", method, jstr(&args)));
	Ok(())
}

fn flag_value(flag: &ExtensionFlag) -> Value {
	let mut object = serde_json::Map::new();
	object.insert("name".to_string(), json!(flag.name));
	object.insert("extensionPath".to_string(), json!(flag.extension_path));
	if let Some(description) = &flag.description {
		object.insert("description".to_string(), json!(description));
	}
	if !flag.flag_type.is_empty() {
		object.insert("type".to_string(), json!(flag.flag_type));
	}
	if let Some(default) = &flag.default {
		object.insert("default".to_string(), json!(default));
	}
	Value::Object(object)
}

fn shortcut_value(shortcut: &ExtensionShortcut) -> Value {
	let mut object = serde_json::Map::new();
	object.insert("shortcut".to_string(), json!(shortcut.shortcut));
	object.insert("extensionPath".to_string(), json!(shortcut.extension_path));
	if let Some(description) = &shortcut.description {
		object.insert("description".to_string(), json!(description));
	}
	Value::Object(object)
}

fn command_value(command: &(String, String, Option<String>, Value)) -> Value {
	let (name, invocation_name, description, source_info) = command;
	let mut object = serde_json::Map::new();
	object.insert("name".to_string(), json!(name));
	object.insert("invocationName".to_string(), json!(invocation_name));
	if let Some(description) = description {
		object.insert("description".to_string(), json!(description));
	}
	object.insert("sourceInfo".to_string(), source_info.clone());
	Value::Object(object)
}

pub fn run(input: &Value) -> Result<Value, String> {
	let ops = input.get("ops").and_then(Value::as_array).ok_or("missing ops")?;
	let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|e| e.to_string())?;
	runtime.block_on(run_async(ops))
}

async fn run_async(ops: &[Value]) -> Result<Value, String> {
	let log: Log = Arc::new(Mutex::new(Vec::new()));
	let bus = {
		let log = Arc::clone(&log);
		Arc::new(create_event_bus_with_sink(Arc::new(move |message: &str| {
			log.lock().unwrap().push(message.to_string());
		})))
	};
	let case = Arc::new(CaseInner {
		log: Arc::clone(&log),
		warns: Arc::new(Mutex::new(Vec::new())),
		bus: Arc::clone(&bus),
		runtime: ExtensionRuntime::new(),
		extensions: Mutex::new(Vec::new()),
		captured: Mutex::new(HashMap::new()),
		unsubs: Mutex::new(Vec::new()),
		runner: Mutex::new(None),
		next_error_id: std::sync::atomic::AtomicU64::new(0),
	});
	for op in ops {
		let kind = op.get("op").and_then(Value::as_str).ok_or("missing op")?;
		match kind {
			"load" => {
				let path = op.get("path").and_then(Value::as_str).ok_or("missing path")?;
				let script = op.get("script").and_then(Value::as_array).cloned().ok_or("missing script")?;
				let case_for_factory = Arc::clone(&case);
				let factory: FactoryFn = Arc::new(move |pi: &ExtensionApi| {
					let pi = pi.clone();
					let case = Arc::clone(&case_for_factory);
					let script = script.clone();
					Box::pin(async move { run_script(&pi, &script, &case) })
				});
				let result =
					load_extension_from_factory(factory, "/proj", Some(Arc::clone(&case.bus)), &case.runtime, path).await;
				match result {
					Ok(extension) => {
						case.log.lock().unwrap().push(format!("load:{}:ok", path));
						case.extensions.lock().unwrap().push(extension);
					}
					Err(message) => case.log.lock().unwrap().push(format!("load:{}:thrown:{}", path, message)),
				}
			}
			"createRunner" => {
				let throw_on = str_list(op, "throwOn").unwrap_or_default();
				let message = op
					.get("throwMsg")
					.and_then(Value::as_str)
					.unwrap_or("mr boom")
					.to_string();
				let registry = Arc::new(FakeRegistry { log: Arc::clone(&case.log), throw_on, message });
				let extensions = case.extensions.lock().unwrap().clone();
				let runner = ExtensionRunner::new(extensions, case.runtime.clone(), "/proj", Some(registry));
				*case.runner.lock().unwrap() = Some(runner);
				case.log.lock().unwrap().push("runner:created".to_string());
			}
			"bindCore" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let actions_cfg = op.get("actions").cloned().unwrap_or(json!({}));
				let tools = str_list(&actions_cfg, "activeTools").unwrap_or_default();
				let actions = Arc::new(FakeActions {
					tools,
					thinking: actions_cfg.get("thinkingLevel").cloned().unwrap_or(Value::Null),
					log: Arc::clone(&case.log),
				});
				let ctx_cfg = op.get("contextActions").cloned().unwrap_or(json!({}));
				let context = Arc::new(FakeContext {
					idle: ctx_cfg.get("isIdle").and_then(Value::as_bool).unwrap_or(false),
					trusted: ctx_cfg.get("isProjectTrusted").and_then(Value::as_bool).unwrap_or(false),
					model: ctx_cfg.get("model").filter(|v| !v.is_null()).cloned(),
					log: Arc::clone(&case.log),
				});
				let mut hooks = ProviderHooks::default();
				if let Some(cfg) = op.get("providerActions").and_then(Value::as_object) {
					let log = Arc::clone(&case.log);
					if let Some(mode) = cfg.get("registerProvider") {
						let mode = mode.clone();
						hooks.register_provider = Some(Arc::new(move |name, config| {
							pa_hook(&log, "registerProvider", &mode, json!([name, config]))
						}));
					}
					if let Some(mode) = cfg.get("registerNativeProvider") {
						let mode = mode.clone();
						let log = Arc::clone(&case.log);
						hooks.register_native_provider = Some(Arc::new(move |provider| {
							pa_hook(&log, "registerNativeProvider", &mode, json!([provider]))
						}));
					}
					if let Some(mode) = cfg.get("unregisterProvider") {
						let mode = mode.clone();
						let log2 = Arc::clone(&case.log);
						hooks.unregister_provider = Some(Arc::new(move |name| {
							pa_hook(&log2, "unregisterProvider", &mode, json!([name]))
						}));
					}
					if let Some(mode) = cfg.get("registerVirtualModel") {
						let mode = mode.clone();
						let log = Arc::clone(&case.log);
						hooks.register_virtual_model = Some(Arc::new(move |definition| {
							pa_hook(&log, "registerVirtualModel", &mode, json!([definition]))
						}));
					}
					if let Some(mode) = cfg.get("unregisterVirtualModel") {
						let mode = mode.clone();
						let log2 = Arc::clone(&case.log);
						hooks.unregister_virtual_model = Some(Arc::new(move |provider, id| {
							pa_hook(&log2, "unregisterVirtualModel", &mode, json!([provider, id]))
						}));
					}
				}
				runner.bind_core(actions, context, hooks);
			}
			"onError" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let id = case.next_error_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
				let log = Arc::clone(&case.log);
				runner.on_error(Arc::new(move |error: &Value| {
					let stripped = json!({
						"extensionPath": error.get("extensionPath").cloned().unwrap_or(Value::Null),
						"event": error.get("event").cloned().unwrap_or(Value::Null),
						"error": error.get("error").cloned().unwrap_or(Value::Null),
					});
					log.lock().unwrap().push(format!("err{}:{}", id, jstr(&stripped)));
				}));
			}
			"removeErrorListener" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let id = op.get("id").and_then(Value::as_u64).ok_or("missing id")?;
				runner.remove_error_listener(id);
			}
			"hasHandlers" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let event = op.get("event").and_then(Value::as_str).ok_or("missing event")?;
				let has = runner.has_handlers(event);
				case.log.lock().unwrap().push(format!("hasHandlers:{}:{}", event, has));
			}
			"emit" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let event = op.get("event").ok_or("missing event")?.clone();
				let result = runner.emit(event).await;
				case.log.lock().unwrap().push(format!("emit:return:{}", jstr(&json!(result))));
			}
			"emitMessageEnd" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let event = op.get("event").ok_or("missing event")?.clone();
				let result = runner.emit_message_end(event).await;
				case.log.lock().unwrap().push(format!("emitMessageEnd:return:{}", jstr(&json!(result))));
			}
			"getFlags" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let entries: Vec<Value> = runner
					.get_flags()
					.iter()
					.map(|flag| json!([flag.name, flag_value(flag)]))
					.collect();
				case.log.lock().unwrap().push(format!("flags:{}", jstr(&Value::Array(entries))));
			}
			"setFlagValue" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let name = op.get("name").and_then(Value::as_str).ok_or("missing name")?;
				let value = op.get("value").cloned().unwrap_or(Value::Null);
				runner.set_flag_value(name, value);
			}
			"getFlagValues" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let entries: Vec<Value> = runner
					.get_flag_values()
					.iter()
					.map(|(name, value)| json!([name, value]))
					.collect();
				case.log.lock().unwrap().push(format!("flagValues:{}", jstr(&Value::Array(entries))));
			}
			"getRegisteredCommands" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let commands: Vec<Value> = runner.get_registered_commands().iter().map(command_value).collect();
				case.log.lock().unwrap().push(format!("commands:{}", jstr(&Value::Array(commands))));
			}
			"getCommand" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let name = op.get("name").and_then(Value::as_str).ok_or("missing name")?;
				let found = runner.get_command(name).map(|c| command_value(&c));
				case.log.lock().unwrap().push(format!("command:{}", jstr(&found.unwrap_or(Value::Null))));
			}
			"getShortcuts" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let keybindings = op.get("keybindings").cloned().unwrap_or(json!({}));
				let shortcuts = runner.get_shortcuts(&keybindings);
				let entries: Vec<Value> = shortcuts
					.iter()
					.map(|(key, shortcut)| json!([key, shortcut_value(shortcut)]))
					.collect();
				case.log.lock().unwrap().push(format!("shortcuts:{}", jstr(&Value::Array(entries))));
				let diagnostics = runner.get_shortcut_diagnostics();
				case.log.lock().unwrap().push(format!("shortcutDiag:{}", jstr(&Value::Array(diagnostics.clone()))));
				// TS warns per diagnostic when there is no UI context.
				if !runner.has_ui() {
					for diagnostic in &diagnostics {
						if let Some(message) = diagnostic.get("message").and_then(Value::as_str) {
							case.warns.lock().unwrap().push(message.to_string());
						}
					}
				}
			}
			"reportUnhandledMcpServers" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				runner.report_unhandled_mcp_servers();
			}
			"invalidate" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let message = op.get("message").and_then(Value::as_str);
				runner.invalidate(message);
			}
			"getActiveTools" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				match runner.get_active_tools() {
					Ok(tools) => {
						case.log.lock().unwrap().push(format!("runner.getActiveTools:{}", jstr(&json!(tools))))
					}
					Err(message) => {
						case.log.lock().unwrap().push(format!("runner.getActiveTools!{}", message))
					}
				}
			}
			"setUIContext" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let ui = op.get("ui").and_then(Value::as_bool).unwrap_or(false);
				let mode = op.get("mode").and_then(Value::as_str).unwrap_or("print");
				runner.set_ui_context(ui, mode);
			}
			"hasUI" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				case.log.lock().unwrap().push(format!("hasUI:{}", runner.has_ui()));
			}
			"shutdown" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				runner.shutdown();
			}
			"emitSessionShutdown" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let event = op.get("event").ok_or("missing event")?.clone();
				let emitted = emit_session_shutdown_event(&runner, event).await;
				case.log.lock().unwrap().push(format!("sessionShutdown:{}", emitted));
			}
			"emitProjectTrust" => {
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				let event = op.get("event").ok_or("missing event")?.clone();
				let ctx = op.get("ctx").cloned().unwrap_or(json!({}));
				let (result, errors) = emit_project_trust_event(&runner, event, ctx).await;
				// TS jstr({ result: r.result, ... }) omits the key when the
				// result is undefined; mirror that (Value::Null serializes).
				let mut payload = serde_json::Map::new();
				if let Some(result) = result {
					payload.insert("result".to_string(), result);
				}
				payload.insert("errors".to_string(), Value::Array(errors));
				case.log.lock().unwrap().push(format!("projectTrust:{}", jstr(&Value::Object(payload))));
			}
			"api" => {
				let key = op.get("key").and_then(Value::as_str).ok_or("missing key")?;
				let steps = op.get("steps").and_then(Value::as_array).cloned().ok_or("missing steps")?;
				let api = case
					.captured
					.lock()
					.unwrap()
					.get(key)
					.cloned()
					.ok_or_else(|| format!("unknown api key: {}", key))?;
				for step in steps {
					let kind = step.get("step").and_then(Value::as_str).unwrap_or("");
					if let Err(message) = run_script(&api, std::slice::from_ref(&step), &case) {
						case.log.lock().unwrap().push(format!("api:{}:{}!{}", key, kind, message));
					}
				}
			}
			"busEmit" => {
				let channel = op.get("channel").and_then(Value::as_str).ok_or("missing channel")?;
				let data = op.get("data").cloned().unwrap_or(Value::Null);
				case.bus.emit(channel, &data);
			}
			"mcpList" => {
				let servers = case.runtime.mcp_servers().lock().unwrap().list();
				case.log.lock().unwrap().push(format!("mcpServers:{}", jstr(&json!(servers))));
			}
			other => return Err(format!("unknown op: {}", other)),
		}
		// Drain queued `mcp_servers_change` emits (the TS settles the void
		// emit on a microtask before the next op).
		if let Some(runner) = case.runner.lock().unwrap().clone() {
			runner.drain_mcp_events().await;
		}
	}
	Ok(json!({
		"result": {
			"log": *case.log.lock().unwrap(),
			"warns": *case.warns.lock().unwrap(),
		}
	}))
}
