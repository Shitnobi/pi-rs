//! Extension runner core (Phase 4, tranche 1): port of
//! `core/extensions/loader.ts` (factory-based loading + the `ExtensionAPI`
//! registration/commit/discard lifecycle) and the core slice of
//! `core/extensions/runner.ts` (`ExtensionRunner`: constructor, `bindCore`
//! pending-registration flush, `hasHandlers`, the generic `emit` loop,
//! `emitMessageEnd`, `emitError`/`onError`, flags/tools/commands/shortcuts
//! queries, `reportUnhandledMcpServers`, `invalidate`).
//!
//! Divergences from the TS (documented, observably preserved):
//! - jiti module loading is out of scope: extensions load from `FactoryFn`
//!   closures (`loadExtensionFromFactory`); path discovery is a later
//!   tranche.
//! - Handlers receive their own event copy per invocation (TS passes the
//!   same live object) and hand it back in [`HandlerOutcome`]; emitters
//!   read in-place edits back, so mutation semantics match the TS. The
//!   generic `emit` still ignores event edits (no TS emitter reads them).
//! - `ExtensionError.stack` is never populated (the harness records errors
//!   without it).
//! - The `mcp_servers_change` listener queues its emit; the host drains it
//!   (the TS fires it with `void this.emit(...)`, settled by the harness).
//! - `getShortcuts` console.warn side channel: the TS warns per diagnostic
//!   when `!hasUI()`; the port records diagnostics only (the parity harness
//!   derives the warn stream from them).
//! - `providerActions` mirrors the TS per-method optionality
//!   ([`ProviderHooks`]); a missing hook falls through to the model
//!   registry seam.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pi_agent::types::BoxFuture;
use pi_core::event_bus::{EventBusController, Subscription};
use pi_core::mcp_servers::{
	mcp_namespace, validate_mcp_server_config, McpServerRegistry, RegisteredMcpServer,
};
use serde_json::{json, Map, Value};

use super::exec::{exec_command, ExecResult};
use super::system_prompt::{build_system_prompt, content_text_raw, normalize_options_raw, reorder_options_raw, DocPaths};
use super::ExtensionHost;

/// TS default `invalidate()` message (loader.ts L192 / runner.ts L729).
pub const DEFAULT_STALE_MESSAGE: &str = "This extension ctx is stale after session replacement or reload. Do not use a captured pi or command ctx after ctx.newSession(), ctx.fork(), ctx.switchSession(), or ctx.reload(). For newSession, fork, and switchSession, move post-replacement work into withSession and use the ctx passed to withSession. For reload, do not use the old ctx after await ctx.reload().";
/// Thrown by the pre-`bindCore` action stubs (loader.ts L161).
const NOT_INITIALIZED: &str =
	"Extension runtime not initialized. Action methods cannot be called during extension loading.";

/// `RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS` (runner.ts L84).
const RESERVED_KEYBINDINGS: [&str; 17] = [
	"app.interrupt",
	"app.clear",
	"app.exit",
	"app.suspend",
	"app.thinking.cycle",
	"app.model.cycleForward",
	"app.model.cycleBackward",
	"app.model.select",
	"app.tools.expand",
	"app.thinking.toggle",
	"app.editor.external",
	"app.message.copy",
	"app.message.followUp",
	"tui.input.submit",
	"tui.select.confirm",
	"tui.select.cancel",
	"tui.input.copy",
];

/// What a handler hands back: its event (handlers own the copy, so
/// in-place edits — `event.messages`, `event.headers`,
/// `event.systemPromptOptions` — are read back by the emitter, exactly
/// like TS's aliased event object) plus the `handlerResult`
/// (`Ok(None)` = undefined; `Err` = throw).
pub struct HandlerOutcome {
	pub event: Value,
	pub result: Option<Value>,
}

/// One event handler: `(event, ctx) -> Ok(HandlerOutcome) |
/// Err(thrown)`.
pub type ExtensionHandler = Arc<
	dyn Fn(Value, CtxView) -> BoxFuture<'static, Result<HandlerOutcome, String>> + Send + Sync,
>;

/// A command handler (`(args, ctx) -> Result<(), String>`); execution is a
/// later tranche, the type keeps registrations 1:1.
pub type CommandHandler = Arc<dyn Fn(String, CtxView) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// An extension factory (`(pi) => void | Promise<void>`); `Err` models a
/// throw.
pub type FactoryFn = Arc<dyn Fn(&ExtensionApi) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

type ErrorListener = Arc<dyn Fn(&Value) + Send + Sync>;

/// JS truthiness for JSON values (`null`/`false`/`0`/`""` falsy).
fn js_truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(b) => *b,
		Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
		Value::String(s) => !s.is_empty(),
		_ => true,
	}
}

/// `currentOptions.forceSystemPrompt = value` with the TS literal key order
/// (position 2, after `customPrompt`).
fn set_force_system_prompt(options: &mut Value, value: Value) {
	if let Some(object) = options.as_object_mut() {
		if object.contains_key("forceSystemPrompt") {
			object.insert("forceSystemPrompt".to_string(), value);
			return;
		}
		let mut rebuilt = Map::new();
		for (key, current) in object.iter() {
			if key == "selectedTools" {
				rebuilt.insert("forceSystemPrompt".to_string(), value.clone());
			}
			rebuilt.insert(key.clone(), current.clone());
		}
		if !rebuilt.contains_key("forceSystemPrompt") {
			rebuilt.insert("forceSystemPrompt".to_string(), value);
		}
		*object = rebuilt;
	}
}

/// `entries` rebuild hook handed to `emitBoundary`.
pub type BoundaryBuildContext =
	Arc<dyn Fn(Vec<Value>) -> BoxFuture<'static, Result<Value, String>> + Send + Sync>;

fn set_field(object: &mut Value, key: &str, value: Value) {
	if let Some(map) = object.as_object_mut() {
		map.insert(key.to_string(), value);
	}
}

/// `sameMessages(left, right)` — TS compares element *identity*; the port
/// compares JSON values (documented divergence for in-place message
/// mutation).
fn same_messages(left: &[Value], right: &[Value]) -> bool {
	left.len() == right.len() && left.iter().zip(right).all(|(a, b)| a == b)
}

/// `getCurrentSystemMessage(messages)` — replay every system message into
/// one leading message (content joined `"\n\n"`, section patches by name,
/// tools resolved by name). `undefined` when no system message carries a
/// timestamp and no tools were added.
fn get_current_system_message(messages: &[Value]) -> Result<Option<Value>, String> {
	let mut content: Vec<String> = Vec::new();
	let mut sections: Vec<(String, String)> = Vec::new();
	let mut timestamp: Option<Value> = None;
	for message in messages {
		if message.get("role").and_then(Value::as_str) != Some("system") {
			continue;
		}
		if timestamp.is_none() {
			if let Some(value) = message.get("timestamp") {
				if !value.is_null() {
					timestamp = Some(value.clone());
				}
			}
		}
		let text = match message.get("content") {
			None => {
				return Err("Cannot read properties of undefined (reading 'filter')".to_string())
			}
			Some(value) => content_text_raw(value, "\n")?,
		};
		if !text.is_empty() {
			content.push(text);
		}
		if let Some(sections_value) = message.get("sections") {
			if !sections_value.is_null() {
				let entries: Vec<(String, Value)> = match sections_value {
					Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
					Value::String(s) => s
						.char_indices()
						.map(|(i, c)| (i.to_string(), json!(c.to_string())))
						.collect(),
					_ => Vec::new(),
				};
				for (name, value) in entries {
					if value.is_null() {
						sections.retain(|(existing, _)| existing != &name);
					} else if let Some(slot) = sections.iter_mut().find(|(existing, _)| existing == &name) {
						slot.1 = value.as_str().unwrap_or_default().to_string();
					} else {
						sections.push((name, value.as_str().unwrap_or_default().to_string()));
					}
				}
			}
		}
	}
	// `getCurrentTools`: replay toolsRemoved/toolsAdded deltas in order.
	let mut tools: Vec<(String, Value)> = Vec::new();
	for message in messages {
		if message.get("role").and_then(Value::as_str) != Some("system") {
			continue;
		}
		for (key, remove) in [("toolsRemoved", true), ("toolsAdded", false)] {
			let Some(delta) = message.get(key) else { continue };
			if delta.is_null() {
				continue;
			}
			let Some(delta) = delta.as_array() else {
				return Err(format!("message.{key} is not iterable"));
			};
			for tool in delta {
				let name = tool.get("name").and_then(Value::as_str).unwrap_or("").to_string();
				if remove {
					tools.retain(|(existing, _)| existing != &name);
				} else if let Some(slot) = tools.iter_mut().find(|(existing, _)| existing == &name) {
					slot.1 = tool.clone();
				} else {
					tools.push((name, tool.clone()));
				}
			}
		}
	}
	if timestamp.is_none() && tools.is_empty() {
		return Ok(None);
	}
	let mut result = Map::new();
	result.insert("role".to_string(), json!("system"));
	result.insert("content".to_string(), json!(content.join("\n\n")));
	if !sections.is_empty() {
		let mut map = Map::new();
		for (name, value) in sections {
			map.insert(name, json!(value));
		}
		result.insert("sections".to_string(), Value::Object(map));
	}
	if !tools.is_empty() {
		result.insert("toolsAdded".to_string(), json!(tools.into_iter().map(|(_, tool)| tool).collect::<Vec<_>>()));
	}
	result.insert("timestamp".to_string(), timestamp.unwrap_or(json!(0)));
	Ok(Some(Value::Object(result)))
}

/// `restoreSystemMessages(current, visible, returned)` — an unchanged
/// conversation keeps every system message; a changed one gets the replayed
/// head prepended.
fn restore_system_messages(
	current: &[Value],
	visible: &[Value],
	returned: &[Value],
) -> Result<Vec<Value>, String> {
	if same_messages(returned, visible) {
		return Ok(current.to_vec());
	}
	match get_current_system_message(current)? {
		Some(head) => {
			let mut result = vec![head];
			result.extend_from_slice(returned);
			Ok(result)
		}
		None => Ok(returned.to_vec()),
	}
}

/// `if (result?.x?.length) out.push(...result.x.map(path => ({path,
/// extensionPath})))`.
fn collect_paths(result: &Value, key: &str, extension_path: &str, out: &mut Vec<Value>) {
	if let Some(Value::Array(paths)) = result.get(key) {
		if !paths.is_empty() {
			for path in paths {
				out.push(json!({"path": path, "extensionPath": extension_path}));
			}
		}
	}
}

/// `isUserBashEventResult(value)` — exactly one of `operations`/`result`;
/// `operations.exec` is a function in TS, modeled here by key presence;
/// `result` needs a string `output`, a **present** `exitCode`
/// (number/undefined), boolean `cancelled`/`truncated`, and an optional
/// string `fullOutputPath`.
pub fn is_user_bash_event_result(value: &Value) -> bool {
	let Some(object) = value.as_object() else {
		return false;
	};
	let has_operations = object.contains_key("operations");
	let has_result = object.contains_key("result");
	if has_operations == has_result {
		return false;
	}
	if has_operations {
		let Some(operations) = object.get("operations").and_then(Value::as_object) else {
			return false;
		};
		return operations.contains_key("exec");
	}
	let Some(result) = object.get("result").and_then(Value::as_object) else {
		return false;
	};
	if !matches!(result.get("output"), Some(Value::String(_))) {
		return false;
	}
	if !result.contains_key("exitCode") {
		return false;
	}
	if !matches!(result.get("exitCode"), Some(Value::Number(_)) | Some(Value::Null)) {
		return false;
	}
	if !matches!(result.get("cancelled"), Some(Value::Bool(_))) {
		return false;
	}
	if !matches!(result.get("truncated"), Some(Value::Bool(_))) {
		return false;
	}
	matches!(result.get("fullOutputPath"), None | Some(Value::String(_)))
}

/// JS `typeof` for the flag-default error message.
fn js_typeof(value: &Value) -> &'static str {
	match value {
		Value::Null => "object",
		Value::Bool(_) => "boolean",
		Value::Number(_) => "number",
		Value::String(_) => "string",
		_ => "object",
	}
}

// ============================================================================
// CtxView — the lazy `ExtensionContext` getter slice
// ============================================================================

/// The context handed to handlers. `Runner` mirrors `createContext()`
/// (guarded getters resolved at read time); `Raw` is a plain value (the
/// `project_trust` ctx).
#[derive(Clone)]
pub enum CtxView {
	Runner(Arc<RunnerShared>),
	Raw(Value),
	/// `emitBeforeAgentStart`'s ctx: the `createContext()` descriptors plus
	/// the injected `getSystemPrompt` getter (`assertActive` + lazy render
	/// against the live options).
	BeforeAgentStart {
		shared: Arc<RunnerShared>,
		renderer: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
	},
}

/// `createContext()` members that are functions (`typeof v === "function"`).
pub fn is_context_method(field: &str) -> bool {
	matches!(
		field,
		"isIdle"
			| "isProjectTrusted"
			| "abort"
			| "hasPendingMessages"
			| "shutdown"
			| "getContextUsage"
			| "compact"
			| "getSystemPrompt"
	)
}

impl CtxView {
	/// Harness read semantics: `typeof v === "function" ? v() : v` — method
	/// members are CALLED, getters are read; `Raw` contexts are plain values.
	pub fn read_field(&self, field: &str) -> Result<Value, String> {
		match self {
			CtxView::Raw(value) => Ok(value.get(field).cloned().unwrap_or(Value::Null)),
			_ => {
				if is_context_method(field) {
					self.call(field)
				} else {
					self.read(field)
				}
			}
		}
	}

	/// Read one context field, mirroring the TS guarded getter/method.
	/// `Err` = the getter threw (stale ctx, unbound runtime action).
	pub fn read(&self, field: &str) -> Result<Value, String> {
		match self {
			CtxView::Raw(value) => Ok(value.get(field).cloned().unwrap_or(Value::Null)),
			CtxView::Runner(shared) => ExtensionRunner { shared: Arc::clone(shared) }.ctx_read(field),
			// `getSystemPrompt` is a function property: reading it yields a
			// function (JSON-undefined); the harness calls it via `call`.
			CtxView::BeforeAgentStart { shared, .. } => {
				if field == "getSystemPrompt" {
					return Ok(Value::Null);
				}
				ExtensionRunner { shared: Arc::clone(shared) }.ctx_read(field)
			}
		}
	}

	/// Invoke one context method (TS `ctx.shutdown()`); `Err` = the guarded
	/// call threw (stale ctx).
	pub fn call(&self, method: &str) -> Result<Value, String> {
		self.call_args(method, &[])
	}

	/// [`CtxView::call`] with positional arguments (`ctx.compact(options)`).
	pub fn call_args(&self, method: &str, args: &[Value]) -> Result<Value, String> {
		match self {
			CtxView::Raw(_) => Err("not a function".to_string()),
			CtxView::Runner(shared) => {
				ExtensionRunner { shared: Arc::clone(shared) }.ctx_call_args(method, args)
			}
			CtxView::BeforeAgentStart { shared, renderer } => {
				if method == "getSystemPrompt" {
					if let Some(message) = shared.stale_message.lock().unwrap().clone() {
						return Err(message);
					}
					return renderer().map(|prompt| json!(prompt));
				}
				ExtensionRunner { shared: Arc::clone(shared) }.ctx_call_args(method, args)
			}
		}
	}
}

// ============================================================================
// Runtime actions seams (bindCore inputs)
// ============================================================================

/// The full `ExtensionActions` set bound by `bindCore` (`runtime.<action>`).
/// `Err` models a throw from a sync action or a rejected promise
/// (`setModel`); pre-bind calls fail with the loader's stub messages.
pub trait RuntimeActions: Send + Sync {
	fn send_message(&self, message: Value, options: Value) -> Result<(), String>;
	fn send_user_message(&self, content: Value, options: Value) -> Result<(), String>;
	fn append_entry(&self, custom_type: &str, data: Value) -> Result<(), String>;
	fn set_session_name(&self, name: &str) -> Result<(), String>;
	/// `string | undefined` — `Value::Null` stands for undefined.
	fn get_session_name(&self) -> Result<Value, String>;
	fn set_label(&self, entry_id: &str, label: Value) -> Result<(), String>;
	fn get_active_tools(&self) -> Vec<String>;
	fn get_all_tools(&self) -> Result<Value, String>;
	fn get_settings(&self) -> Result<Value, String>;
	fn set_active_tools(&self, names: &[String]) -> Result<(), String>;
	fn refresh_tools(&self);
	fn get_commands(&self) -> Result<Value, String>;
	/// `Promise<boolean>` — `Err` is the rejected-promise path.
	fn set_model(&self, model: Value) -> Result<Value, String>;
	fn get_thinking_level(&self) -> Value;
	fn set_thinking_level(&self, level: Value) -> Result<(), String>;
}

/// The full `ExtensionContextActions` bound by `bindCore` (`required except
/// the three optional ones, which default to the TS class-field fallbacks).
pub trait ContextActions: Send + Sync {
	fn get_model(&self) -> Option<Value>;
	fn get_scoped_models(&self) -> Vec<Value>;
	fn is_idle(&self) -> bool;
	fn is_project_trusted(&self) -> bool;
	/// `AbortSignal | undefined` — opaque to the port.
	fn get_signal(&self) -> Option<Value>;
	fn abort(&self);
	fn has_pending_messages(&self) -> bool;
	fn shutdown(&self);
	fn get_context_usage(&self) -> Option<Value>;
	fn compact(&self, options: Value) -> Result<(), String>;
	fn get_system_prompt(&self) -> String;
	/// Optional: absent → `normalizeBuildSystemPromptOptions({cwd})`.
	fn get_system_prompt_options(&self) -> Option<Value> {
		None
	}
	/// Optional: absent → nested tool calls are unavailable.
	fn execute_tool(
		&self,
		caller_id: &str,
		name: &str,
		args: Value,
		options: Value,
	) -> Option<Result<Value, String>> {
		let _ = (caller_id, name, args, options);
		None
	}
	/// Optional: absent → the callable-tool list is empty.
	fn get_callable_tools(&self) -> Option<Vec<Value>> {
		None
	}
}

/// The `ExtensionUIContext` seam. The real TUI is out of scope: every
/// method routes through one Value-typed [`UiContext::invoke`], which the
/// host backs with its fake-UI script. `Err` models a throw; `Value::Null`
/// models `undefined`.
pub trait UiContext: Send + Sync {
	/// `ui.<method>(...args)` — `args` is the positional argument list.
	fn invoke(&self, method: &str, args: &[Value]) -> Result<Value, String>;
}

/// The `ExtensionCommandContextActions` bound by `bindCommandContext`
/// (interactive command execution is a later tranche; the seam keeps the
/// future-based signatures).
pub trait CommandContextActions: Send + Sync {
	fn wait_for_idle(&self) -> BoxFuture<'static, ()>;
	/// `Promise<{ cancelled: boolean }>`.
	fn new_session(&self, options: Value) -> BoxFuture<'static, Value>;
	fn fork(&self, entry_id: String, options: Value) -> BoxFuture<'static, Value>;
	fn navigate_tree(&self, target_id: String, options: Value) -> BoxFuture<'static, Value>;
	fn switch_session(&self, session_path: String, options: Value) -> BoxFuture<'static, Value>;
	fn reload(&self) -> BoxFuture<'static, ()>;
}

/// Provider registration hook aliases: `Err` models a throw (TS propagates
/// hook and registry errors to the extension API caller).
pub type RegisterProviderHook = Arc<dyn for<'a, 'b> Fn(&'a str, &'b Value) -> Result<(), String> + Send + Sync>;
pub type ProviderValueHook = Arc<dyn for<'a> Fn(&'a Value) -> Result<(), String> + Send + Sync>;
pub type ProviderNameHook = Arc<dyn for<'a> Fn(&'a str) -> Result<(), String> + Send + Sync>;
pub type UnregisterVirtualModelHook = Arc<dyn for<'a, 'b> Fn(&'a str, &'b str) -> Result<(), String> + Send + Sync>;

/// `bindCore`'s optional `providerActions` — one hook per method, mirroring
/// the TS per-method optionality: a missing hook falls through to the model
/// registry.
#[derive(Default)]
pub struct ProviderHooks {
	pub register_provider: Option<RegisterProviderHook>,
	pub register_native_provider: Option<ProviderValueHook>,
	pub unregister_provider: Option<ProviderNameHook>,
	pub register_virtual_model: Option<ProviderValueHook>,
	pub unregister_virtual_model: Option<UnregisterVirtualModelHook>,
}

/// The `ModelRegistry` registration seam (the `bindCore` fallback).
pub trait ModelRegistrySeam: Send + Sync {
	fn register_provider(&self, name: &str, config: &Value) -> Result<(), String>;
	fn register_native_provider(&self, provider: &Value) -> Result<(), String>;
	fn unregister_provider(&self, name: &str) -> Result<(), String>;
	fn register_virtual_model(&self, definition: &Value) -> Result<(), String>;
	fn unregister_virtual_model(&self, provider: &str, id: &str) -> Result<(), String>;
}

struct ProviderSink {
	hooks: ProviderHooks,
	model_registry: Option<Arc<dyn ModelRegistrySeam>>,
}

impl ProviderSink {
	fn register_provider(&self, name: &str, config: &Value) -> Result<(), String> {
		if let Some(hook) = &self.hooks.register_provider {
			return hook(name, config);
		}
		match &self.model_registry {
			Some(registry) => registry.register_provider(name, config),
			None => Ok(()),
		}
	}
	fn register_native_provider(&self, provider: &Value) -> Result<(), String> {
		if let Some(hook) = &self.hooks.register_native_provider {
			return hook(provider);
		}
		match &self.model_registry {
			Some(registry) => registry.register_native_provider(provider),
			None => Ok(()),
		}
	}
	fn register_virtual_model(&self, definition: &Value) -> Result<(), String> {
		if let Some(hook) = &self.hooks.register_virtual_model {
			return hook(definition);
		}
		match &self.model_registry {
			Some(registry) => registry.register_virtual_model(definition),
			None => Ok(()),
		}
	}
	fn unregister_provider(&self, name: &str) -> Result<(), String> {
		if let Some(hook) = &self.hooks.unregister_provider {
			return hook(name);
		}
		match &self.model_registry {
			Some(registry) => registry.unregister_provider(name),
			None => Ok(()),
		}
	}
	fn unregister_virtual_model(&self, provider: &str, id: &str) -> Result<(), String> {
		if let Some(hook) = &self.hooks.unregister_virtual_model {
			return hook(provider, id);
		}
		match &self.model_registry {
			Some(registry) => registry.unregister_virtual_model(provider, id),
			None => Ok(()),
		}
	}
}

// ============================================================================
// ExtensionRuntime (loader.ts createExtensionRuntime)
// ============================================================================

struct RuntimeInner {
	/// JS `Map<string, boolean|string>` (insertion order, last write wins).
	flag_values: Vec<(String, Value)>,
	pending_providers: Vec<(String, Value, String)>,
	pending_native: Vec<(Value, String)>,
	pending_virtual: Vec<(Value, String)>,
	stale_message: Option<String>,
	/// Event-bus subscriptions retained until invalidation
	/// (`trackEventBusSubscription`); `None` = already unsubscribed.
	bus_subscriptions: Vec<Option<Subscription>>,
	actions: Option<Arc<dyn RuntimeActions>>,
	/// Set by `bindCore`: registrations take effect immediately.
	provider_sink: Option<ProviderSink>,
}

/// The shared runtime created by the loader (`createExtensionRuntime()`).
#[derive(Clone)]
pub struct ExtensionRuntime {
	inner: Arc<Mutex<RuntimeInner>>,
	mcp_servers: Arc<Mutex<McpServerRegistry>>,
}

impl Default for ExtensionRuntime {
	fn default() -> Self {
		Self::new()
	}
}

impl ExtensionRuntime {
	pub fn new() -> ExtensionRuntime {
		ExtensionRuntime {
			inner: Arc::new(Mutex::new(RuntimeInner {
				flag_values: Vec::new(),
				pending_providers: Vec::new(),
				pending_native: Vec::new(),
				pending_virtual: Vec::new(),
				stale_message: None,
				bus_subscriptions: Vec::new(),
				actions: None,
				provider_sink: None,
			})),
			mcp_servers: Arc::new(Mutex::new(McpServerRegistry::new())),
		}
	}

	fn set_flag_value(&self, name: &str, value: Value) {
		let mut inner = self.inner.lock().unwrap();
		if let Some(slot) = inner.flag_values.iter_mut().find(|(k, _)| k == name) {
			slot.1 = value;
		} else {
			inner.flag_values.push((name.to_string(), value));
		}
	}

	fn has_flag_value(&self, name: &str) -> bool {
		self.inner.lock().unwrap().flag_values.iter().any(|(k, _)| k == name)
	}

	/// `runtime.assertActive()` — `Err` carries the stale message.
	pub fn assert_active(&self) -> Result<(), String> {
		match self.inner.lock().unwrap().stale_message.clone() {
			Some(message) => Err(message),
			None => Ok(()),
		}
	}

	/// `runtime.invalidate(message?)` — first call wins; drops tracked
	/// event-bus subscriptions.
	pub fn invalidate(&self, message: Option<&str>) {
		let mut inner = self.inner.lock().unwrap();
		if inner.stale_message.is_some() {
			return;
		}
		inner.stale_message = Some(message.unwrap_or(DEFAULT_STALE_MESSAGE).to_string());
		for subscription in inner.bus_subscriptions.iter_mut() {
			if let Some(subscription) = subscription.take() {
				subscription.unsubscribe();
			}
		}
	}

	/// `runtime.trackEventBusSubscription(unsubscribe)`.
	pub fn track_event_bus_subscription(&self, subscription: Subscription) {
		self.inner.lock().unwrap().bus_subscriptions.push(Some(subscription));
	}

	/// Number of tracked subscriptions (the discard-range start).
	pub fn tracked_count(&self) -> usize {
		self.inner.lock().unwrap().bus_subscriptions.len()
	}

	/// Unsubscribe every subscription tracked at or after `start` (the
	/// loading-range discard in `createExtensionAPI.discard`).
	pub fn unsubscribe_from(&self, start: usize) {
		let mut inner = self.inner.lock().unwrap();
		for subscription in inner.bus_subscriptions.iter_mut().skip(start) {
			if let Some(subscription) = subscription.take() {
				subscription.unsubscribe();
			}
		}
	}

	/// Pre-`bindCore` `registerProvider` — queues the registration.
	pub fn register_provider(&self, name: &str, config: &Value, extension_path: &str) -> Result<(), String> {
		let mut inner = self.inner.lock().unwrap();
		if let Some(sink) = &inner.provider_sink {
			let sink_register = sink.register_provider(name, config);
			drop(inner);
			return sink_register;
		}
		inner
			.pending_providers
			.push((name.to_string(), config.clone(), extension_path.to_string()));
		Ok(())
	}

	pub fn register_native_provider(&self, provider: &Value, extension_path: &str) -> Result<(), String> {
		let mut inner = self.inner.lock().unwrap();
		if let Some(sink) = &inner.provider_sink {
			let result = sink.register_native_provider(provider);
			drop(inner);
			return result;
		}
		inner.pending_native.push((provider.clone(), extension_path.to_string()));
		Ok(())
	}

	/// `unregisterProvider(name)` — filters both pending queues (pre-bind) or
	/// calls through (post-bind, propagating registry/hook throws).
	pub fn unregister_provider(&self, name: &str) -> Result<(), String> {
		let bound = self.inner.lock().unwrap().provider_sink.is_some();
		if bound {
			let inner = self.inner.lock().unwrap();
			if let Some(sink) = &inner.provider_sink {
				return sink.unregister_provider(name);
			}
			return Ok(());
		}
		let mut inner = self.inner.lock().unwrap();
		inner.pending_providers.retain(|(n, _, _)| n != name);
		inner
			.pending_native
			.retain(|(p, _)| p.get("id").and_then(Value::as_str) != Some(name));
		Ok(())
	}

	pub fn register_virtual_model(&self, definition: &Value, extension_path: &str) -> Result<(), String> {
		let mut inner = self.inner.lock().unwrap();
		if let Some(sink) = &inner.provider_sink {
			let result = sink.register_virtual_model(definition);
			drop(inner);
			return result;
		}
		inner
			.pending_virtual
			.push((definition.clone(), extension_path.to_string()));
		Ok(())
	}

	pub fn unregister_virtual_model(&self, provider: &str, id: &str) -> Result<(), String> {
		if self.inner.lock().unwrap().provider_sink.is_some() {
			let inner = self.inner.lock().unwrap();
			if let Some(sink) = &inner.provider_sink {
				return sink.unregister_virtual_model(provider, id);
			}
			return Ok(());
		}
		let mut inner = self.inner.lock().unwrap();
		inner
			.pending_virtual
			.retain(|(d, _)| d.get("provider").and_then(Value::as_str) != Some(provider) || d.get("id").and_then(Value::as_str) != Some(id));
		Ok(())
	}

	/// `setModel`'s pre-bind stub rejects with this shorter message.
	pub const NOT_INITIALIZED_SET_MODEL: &'static str = "Extension runtime not initialized";

	fn actions(&self) -> Option<Arc<dyn RuntimeActions>> {
		self.inner.lock().unwrap().actions.clone()
	}

	/// `runtime.sendMessage(message, options)` — pre-bind stub throws.
	pub fn send_message(&self, message: Value, options: Value) -> Result<(), String> {
		match self.actions() {
			Some(actions) => actions.send_message(message, options),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.sendUserMessage(content, options)` — pre-bind stub throws.
	pub fn send_user_message(&self, content: Value, options: Value) -> Result<(), String> {
		match self.actions() {
			Some(actions) => actions.send_user_message(content, options),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.appendEntry(customType, data)` — pre-bind stub throws.
	pub fn append_entry(&self, custom_type: &str, data: Value) -> Result<(), String> {
		match self.actions() {
			Some(actions) => actions.append_entry(custom_type, data),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.setSessionName(name)` — pre-bind stub throws.
	pub fn set_session_name(&self, name: &str) -> Result<(), String> {
		match self.actions() {
			Some(actions) => actions.set_session_name(name),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.getSessionName()` — pre-bind stub throws.
	pub fn get_session_name(&self) -> Result<Value, String> {
		match self.actions() {
			Some(actions) => actions.get_session_name(),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.setLabel(entryId, label)` — pre-bind stub throws.
	pub fn set_label(&self, entry_id: &str, label: Value) -> Result<(), String> {
		match self.actions() {
			Some(actions) => actions.set_label(entry_id, label),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.getAllTools()` — pre-bind stub throws.
	pub fn get_all_tools(&self) -> Result<Value, String> {
		match self.actions() {
			Some(actions) => actions.get_all_tools(),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.getSettings()` — pre-bind stub throws.
	pub fn get_settings(&self) -> Result<Value, String> {
		match self.actions() {
			Some(actions) => actions.get_settings(),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.setActiveTools(names)` — pre-bind stub throws.
	pub fn set_active_tools(&self, names: &[String]) -> Result<(), String> {
		match self.actions() {
			Some(actions) => actions.set_active_tools(names),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.getCommands()` — pre-bind stub throws.
	pub fn get_commands(&self) -> Result<Value, String> {
		match self.actions() {
			Some(actions) => actions.get_commands(),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.setModel(model)` — pre-bind returns a rejected promise
	/// ("Extension runtime not initialized", no suffix).
	pub fn set_model(&self, model: Value) -> Result<Value, String> {
		match self.actions() {
			Some(actions) => actions.set_model(model),
			None => Err(Self::NOT_INITIALIZED_SET_MODEL.to_string()),
		}
	}

	/// `runtime.setThinkingLevel(level)` — pre-bind stub throws.
	pub fn set_thinking_level(&self, level: Value) -> Result<(), String> {
		match self.actions() {
			Some(actions) => actions.set_thinking_level(level),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.getActiveTools()` — pre-bind stub throws.
	pub fn get_active_tools(&self) -> Result<Vec<String>, String> {
		let actions = self.inner.lock().unwrap().actions.clone();
		match actions {
			Some(actions) => Ok(actions.get_active_tools()),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.getThinkingLevel()` — pre-bind stub throws.
	pub fn get_thinking_level(&self) -> Result<Value, String> {
		let actions = self.inner.lock().unwrap().actions.clone();
		match actions {
			Some(actions) => Ok(actions.get_thinking_level()),
			None => Err(NOT_INITIALIZED.to_string()),
		}
	}

	/// `runtime.refreshTools()` — no-op pre-bind.
	pub fn refresh_tools(&self) {
		if let Some(actions) = self.inner.lock().unwrap().actions.clone() {
			actions.refresh_tools();
		}
	}

	pub fn mcp_servers(&self) -> Arc<Mutex<McpServerRegistry>> {
		Arc::clone(&self.mcp_servers)
	}

	pub fn flag_values(&self) -> Vec<(String, Value)> {
		self.inner.lock().unwrap().flag_values.clone()
	}

	pub fn pending_provider_registrations(&self) -> Vec<(String, Value, String)> {
		self.inner.lock().unwrap().pending_providers.clone()
	}

	pub fn pending_native_provider_registrations(&self) -> Vec<(Value, String)> {
		self.inner.lock().unwrap().pending_native.clone()
	}

	pub fn pending_virtual_model_registrations(&self) -> Vec<(Value, String)> {
		self.inner.lock().unwrap().pending_virtual.clone()
	}
}

// ============================================================================
// Pending runtime changes (applyRuntimeChange)
// ============================================================================

enum PendingChange {
	RegisterProvider { name: String, config: Value, extension_path: String },
	RegisterNativeProvider { provider: Value, extension_path: String },
	UnregisterProvider { name: String },
	RegisterVirtualModel { definition: Value, extension_path: String },
	UnregisterVirtualModel { provider: String, id: String },
	RegisterMcpServer { server: RegisteredMcpServer },
	UnregisterMcpServer { name: String, extension_path: String },
}

impl PendingChange {
	fn apply(&self, runtime: &ExtensionRuntime) -> Result<(), String> {
		match self {
			PendingChange::RegisterProvider { name, config, extension_path } => {
				runtime.register_provider(name, config, extension_path)
			}
			PendingChange::RegisterNativeProvider { provider, extension_path } => {
				runtime.register_native_provider(provider, extension_path)
			}
			PendingChange::UnregisterProvider { name } => runtime.unregister_provider(name),
			PendingChange::RegisterVirtualModel { definition, extension_path } => {
				runtime.register_virtual_model(definition, extension_path)
			}
			PendingChange::UnregisterVirtualModel { provider, id } => {
				runtime.unregister_virtual_model(provider, id)
			}
			PendingChange::RegisterMcpServer { server } => {
				runtime.mcp_servers().lock().unwrap().register(server.clone());
				Ok(())
			}
			PendingChange::UnregisterMcpServer { name, extension_path } => {
				runtime.mcp_servers().lock().unwrap().unregister(name, extension_path);
				Ok(())
			}
		}
	}
}

// ============================================================================
// Extension + registrations
// ============================================================================

/// TS `RegisteredCommand` (handler kept for later tranches).
pub struct RegisteredCommand {
	pub name: String,
	pub source_info: Value,
	pub description: Option<String>,
	pub handler: Option<CommandHandler>,
}

/// TS `ExtensionFlag`.
#[derive(Clone)]
pub struct ExtensionFlag {
	pub name: String,
	pub description: Option<String>,
	pub flag_type: String,
	pub default: Option<Value>,
	pub extension_path: String,
}

/// TS `ExtensionShortcut` (handler invocation is a later tranche).
#[derive(Clone)]
pub struct ExtensionShortcut {
	pub shortcut: String,
	pub description: Option<String>,
	pub extension_path: String,
}

/// TS `Extension` — one loaded extension's registrations.
pub struct Extension {
	pub path: String,
	pub resolved_path: String,
	pub hidden: bool,
	pub replaceable: bool,
	pub source_info: Value,
	/// event type -> handlers in registration order.
	pub handlers: Vec<(String, Vec<(u64, ExtensionHandler)>)>,
	/// tool name -> (definition, sourceInfo).
	pub tools: Vec<(String, Value, Value)>,
	pub message_renderers: Vec<String>,
	pub entry_renderers: Vec<String>,
	pub markdown_transformer: bool,
	pub tool_renderers: usize,
	pub commands: Vec<(String, RegisteredCommand)>,
	pub flags: Vec<(String, ExtensionFlag)>,
	pub shortcuts: Vec<(String, ExtensionShortcut)>,
}

fn synthetic_source(path: &str) -> String {
	if let Some(rest) = path.strip_prefix("builtin:") {
		let _ = rest;
		"builtin".to_string()
	} else if path.starts_with('<') && path.ends_with('>') {
		let inner = &path[1..path.len() - 1];
		let prefix = inner.split(':').next().unwrap_or("");
		if prefix.is_empty() {
			"temporary".to_string()
		} else {
			prefix.to_string()
		}
	} else {
		"local".to_string()
	}
}

fn is_synthetic_path(path: &str) -> bool {
	path.starts_with("builtin:") || path.starts_with('<')
}

/// TS `createSyntheticSourceInfo` projection (key order matches).
fn source_info_value(path: &str, resolved_path: &str) -> Value {
	let mut object = Map::new();
	object.insert("path".to_string(), json!(path));
	object.insert("source".to_string(), json!(synthetic_source(path)));
	object.insert("scope".to_string(), json!("temporary"));
	object.insert("origin".to_string(), json!("top-level"));
	if !is_synthetic_path(path) {
		let base_dir = std::path::Path::new(resolved_path)
			.parent()
			.map(|p| p.to_string_lossy().to_string())
			.unwrap_or_default();
		object.insert("baseDir".to_string(), json!(base_dir));
	}
	Value::Object(object)
}

// ============================================================================
// ExtensionApi (loader.ts createExtensionAPI)
// ============================================================================

struct LoadingState {
	/// "loading" | "active" | "failed".
	state: String,
	pending_flag_values: Vec<(String, Value)>,
	pending_changes: Vec<PendingChange>,
	/// `runtime.tracked_count()` at API creation.
	bus_start: usize,
	next_handler_id: u64,
}

/// The `pi` API handed to a factory. Cloning shares the loading state.
#[derive(Clone)]
pub struct ExtensionApi {
	extension: Arc<Mutex<Extension>>,
	runtime: ExtensionRuntime,
	loading: Arc<Mutex<LoadingState>>,
	bus: Option<Arc<EventBusController>>,
	/// The loader's resolved cwd — `pi.exec`'s `options?.cwd ?? cwd`.
	cwd: String,
}

/// Unsubscribe handle for `pi.on` (the returned function).
pub struct HandlerToken {
	extension: Arc<Mutex<Extension>>,
	event: String,
	id: u64,
}

impl HandlerToken {
	/// The `pi.on` unsubscribe function.
	pub fn unsubscribe(&self) {
		let mut extension = self.extension.lock().unwrap();
		if let Some((_, handlers)) = extension.handlers.iter_mut().find(|(e, _)| e == &self.event) {
			if let Some(index) = handlers.iter().position(|(id, _)| *id == self.id) {
				handlers.remove(index);
			}
		}
	}
}

impl ExtensionApi {
	fn assert_active(&self) -> Result<(), String> {
		let extension_path = self.extension.lock().unwrap().path.clone();
		let state = self.loading.lock().unwrap().state.clone();
		if state == "failed" {
			return Err(format!(
				"Extension \"{}\" failed to load and its API is no longer active.",
				extension_path
			));
		}
		self.runtime.assert_active()
	}

	fn apply_runtime_change(&self, change: PendingChange) -> Result<(), String> {
		let mut loading = self.loading.lock().unwrap();
		if loading.state == "loading" {
			loading.pending_changes.push(change);
			Ok(())
		} else {
			drop(loading);
			change.apply(&self.runtime)
		}
	}

	// ---- Action methods (`assertActive()` then runtime delegation) -----

	/// `pi.sendMessage(message, options?)`.
	pub fn send_message(&self, message: Value, options: Value) -> Result<(), String> {
		self.assert_active()?;
		self.runtime.send_message(message, options)
	}

	/// `pi.sendUserMessage(content, options?)`.
	pub fn send_user_message(&self, content: Value, options: Value) -> Result<(), String> {
		self.assert_active()?;
		self.runtime.send_user_message(content, options)
	}

	/// `pi.appendEntry(customType, data?)`.
	pub fn append_entry(&self, custom_type: &str, data: Value) -> Result<(), String> {
		self.assert_active()?;
		self.runtime.append_entry(custom_type, data)
	}

	/// `pi.setSessionName(name)`.
	pub fn set_session_name(&self, name: &str) -> Result<(), String> {
		self.assert_active()?;
		self.runtime.set_session_name(name)
	}

	/// `pi.getSessionName()` (`undefined` → `Value::Null`).
	pub fn get_session_name(&self) -> Result<Value, String> {
		self.assert_active()?;
		self.runtime.get_session_name()
	}

	/// `pi.setLabel(entryId, label)`.
	pub fn set_label(&self, entry_id: &str, label: Value) -> Result<(), String> {
		self.assert_active()?;
		self.runtime.set_label(entry_id, label)
	}

	/// `pi.getActiveTools()`.
	pub fn get_active_tools(&self) -> Result<Vec<String>, String> {
		self.assert_active()?;
		self.runtime.get_active_tools()
	}

	/// `pi.getAllTools()`.
	pub fn get_all_tools(&self) -> Result<Value, String> {
		self.assert_active()?;
		self.runtime.get_all_tools()
	}

	/// `pi.getSettings()`.
	pub fn get_settings(&self) -> Result<Value, String> {
		self.assert_active()?;
		self.runtime.get_settings()
	}

	/// `pi.setActiveTools(toolNames)`.
	pub fn set_active_tools(&self, names: &[String]) -> Result<(), String> {
		self.assert_active()?;
		self.runtime.set_active_tools(names)
	}

	/// `pi.refreshTools()`.
	pub fn refresh_tools(&self) {
		self.runtime.refresh_tools();
	}

	/// `pi.getCommands()`.
	pub fn get_commands(&self) -> Result<Value, String> {
		self.assert_active()?;
		self.runtime.get_commands()
	}

	/// `pi.setModel(model)` — `Err` models the rejected promise.
	pub fn set_model(&self, model: Value) -> Result<Value, String> {
		self.assert_active()?;
		self.runtime.set_model(model)
	}

	/// `pi.getThinkingLevel()`.
	pub fn get_thinking_level(&self) -> Result<Value, String> {
		self.assert_active()?;
		self.runtime.get_thinking_level()
	}

	/// `pi.setThinkingLevel(level)`.
	pub fn set_thinking_level(&self, level: Value) -> Result<(), String> {
		self.assert_active()?;
		self.runtime.set_thinking_level(level)
	}

	/// `pi.exec(command, args, options?)` — `options.cwd ?? loadingCwd`;
	/// argument validation and process semantics live in [`exec_command`].
	pub async fn exec(
		&self,
		command: Value,
		args: Value,
		options: Value,
	) -> Result<ExecResult, String> {
		self.assert_active()?;
		let cwd = options
			.get("cwd")
			.and_then(Value::as_str)
			.map(str::to_string)
			.unwrap_or_else(|| self.cwd.clone());
		exec_command(&command, &args, Some(&cwd), &options, None).await
	}

	/// `pi.on(event, handler)` — returns the unsubscribe token.
	pub fn on(&self, event: &str, handler: ExtensionHandler) -> Result<HandlerToken, String> {
		self.assert_active()?;
		let mut loading = self.loading.lock().unwrap();
		let id = loading.next_handler_id;
		loading.next_handler_id += 1;
		drop(loading);
		let mut extension = self.extension.lock().unwrap();
		let token = HandlerToken {
			extension: Arc::clone(&self.extension),
			event: event.to_string(),
			id,
		};
		if let Some((_, handlers)) = extension.handlers.iter_mut().find(|(e, _)| e == event) {
			handlers.push((id, handler));
		} else {
			extension.handlers.push((event.to_string(), vec![(id, handler)]));
		}
		Ok(token)
	}

	/// `pi.registerTool(tool)`.
	pub fn register_tool(&self, tool: &Value) -> Result<(), String> {
		self.assert_active()?;
		let extension_path = self.extension.lock().unwrap().path.clone();
		let name = tool.get("name").and_then(Value::as_str).unwrap_or("").to_string();
		let valid = matches!(tool.get("parameters"), Some(Value::Object(_)));
		if !valid {
			return Err(format!(
				"Tool \"{}\" registered by extension \"{}\" must define an object parameter schema.",
				name, extension_path
			));
		}
		let source_info = self.extension.lock().unwrap().source_info.clone();
		{
			let mut extension = self.extension.lock().unwrap();
			let entry = (name.clone(), tool.clone(), source_info);
			if let Some(slot) = extension.tools.iter_mut().find(|(n, _, _)| *n == name) {
				*slot = entry;
			} else {
				extension.tools.push(entry);
			}
		}
		self.runtime.refresh_tools();
		Ok(())
	}

	/// `pi.registerCommand(name, options)`.
	pub fn register_command(&self, name: &str, options: &Value, handler: Option<CommandHandler>) -> Result<(), String> {
		self.assert_active()?;
		let extension_path = self.extension.lock().unwrap().path.clone();
		if name.is_empty() {
			return Err(format!(
				"Command registered by extension \"{}\" must have a non-empty string name. Use pi.registerCommand(\"name\", {{ description, handler }}).",
				extension_path
			));
		}
		if handler.is_none() {
			return Err(format!(
				"Command \"/{}\" registered by extension \"{}\" must define handler().",
				name, extension_path
			));
		}
		let source_info = self.extension.lock().unwrap().source_info.clone();
		let description = options
			.get("description")
			.and_then(Value::as_str)
			.map(|s| s.to_string());
		let command = RegisteredCommand { name: name.to_string(), source_info, description, handler };
		let mut extension = self.extension.lock().unwrap();
		if let Some(slot) = extension.commands.iter_mut().find(|(n, _)| n == name) {
			slot.1 = command;
		} else {
			extension.commands.push((name.to_string(), command));
		}
		Ok(())
	}

	/// `pi.registerShortcut(shortcut, options)`.
	pub fn register_shortcut(&self, shortcut: &str, options: &Value) -> Result<(), String> {
		self.assert_active()?;
		let extension_path = self.extension.lock().unwrap().path.clone();
		let description = options
			.get("description")
			.and_then(Value::as_str)
			.map(|s| s.to_string());
		let entry = ExtensionShortcut {
			shortcut: shortcut.to_string(),
			description,
			extension_path,
		};
		let mut extension = self.extension.lock().unwrap();
		if let Some(slot) = extension.shortcuts.iter_mut().find(|(k, _)| k == shortcut) {
			slot.1 = entry;
		} else {
			extension.shortcuts.push((shortcut.to_string(), entry));
		}
		Ok(())
	}

	/// `pi.registerFlag(name, options)`.
	pub fn register_flag(&self, name: &str, options: &Value) -> Result<(), String> {
		self.assert_active()?;
		let flag_type = options.get("type").and_then(Value::as_str).unwrap_or("").to_string();
		let type_display = if flag_type.is_empty() { "undefined" } else { flag_type.as_str() };
		let default = options.get("default").cloned();
		if let Some(default) = &default {
			// TS: `typeof options.default !== options.type` (undefined type
			// never matches).
			if js_typeof(default) != type_display {
				return Err(format!(
					"Invalid default for flag \"{}\": expected {}, got {}",
					name,
					type_display,
					js_typeof(default)
				));
			}
		}
		let extension_path = self.extension.lock().unwrap().path.clone();
		let description = options
			.get("description")
			.and_then(Value::as_str)
			.map(|s| s.to_string());
		let flag = ExtensionFlag {
			name: name.to_string(),
			description,
			flag_type,
			default: default.clone(),
			extension_path,
		};
		{
			let mut extension = self.extension.lock().unwrap();
			if let Some(slot) = extension.flags.iter_mut().find(|(n, _)| n == name) {
				slot.1 = flag;
			} else {
				extension.flags.push((name.to_string(), flag));
			}
		}
		if let Some(default) = default {
			if !self.runtime.has_flag_value(name) {
				let mut loading = self.loading.lock().unwrap();
				if loading.state == "loading" {
					if !loading.pending_flag_values.iter().any(|(n, _)| n == name) {
						loading.pending_flag_values.push((name.to_string(), default));
					}
				} else {
					drop(loading);
					self.runtime.set_flag_value(name, default);
				}
			}
		}
		Ok(())
	}

	pub fn register_message_renderer(&self, custom_type: &str) -> Result<(), String> {
		self.assert_active()?;
		let mut extension = self.extension.lock().unwrap();
		if !extension.message_renderers.iter().any(|t| t == custom_type) {
			extension.message_renderers.push(custom_type.to_string());
		}
		Ok(())
	}

	pub fn register_entry_renderer(&self, custom_type: &str) -> Result<(), String> {
		self.assert_active()?;
		let mut extension = self.extension.lock().unwrap();
		if !extension.entry_renderers.iter().any(|t| t == custom_type) {
			extension.entry_renderers.push(custom_type.to_string());
		}
		Ok(())
	}

	pub fn register_markdown_transformer(&self) -> Result<(), String> {
		self.assert_active()?;
		self.extension.lock().unwrap().markdown_transformer = true;
		Ok(())
	}

	pub fn register_tool_renderer(&self) -> Result<(), String> {
		self.assert_active()?;
		self.extension.lock().unwrap().tool_renderers += 1;
		Ok(())
	}

	/// `pi.getFlag(name)` — pending defaults are visible during loading.
	pub fn get_flag(&self, name: &str) -> Result<Option<Value>, String> {
		self.assert_active()?;
		let registered = self.extension.lock().unwrap().flags.iter().any(|(n, _)| n == name);
		if !registered {
			return Ok(None);
		}
		if let Some((_, value)) = self.runtime.flag_values().into_iter().find(|(n, _)| n == name) {
			return Ok(Some(value));
		}
		let loading = self.loading.lock().unwrap();
		Ok(loading
			.pending_flag_values
			.iter()
			.find(|(n, _)| n == name)
			.map(|(_, v)| v.clone()))
	}

	/// `pi.registerProvider(name, config)`.
	pub fn register_provider(&self, name: &str, config: Option<&Value>) -> Result<(), String> {
		self.assert_active()?;
		let config = match config {
			Some(config) => config,
			None => return Err("Provider config is required when registering by name".to_string()),
		};
		let extension_path = self.extension.lock().unwrap().path.clone();
		self.apply_runtime_change(PendingChange::RegisterProvider {
			name: name.to_string(),
			config: config.clone(),
			extension_path,
		})
	}

	/// `pi.registerProvider(provider)`.
	pub fn register_native_provider(&self, provider: &Value) -> Result<(), String> {
		self.assert_active()?;
		let extension_path = self.extension.lock().unwrap().path.clone();
		self.apply_runtime_change(PendingChange::RegisterNativeProvider {
			provider: provider.clone(),
			extension_path,
		})
	}

	pub fn unregister_provider(&self, name: &str) -> Result<(), String> {
		self.assert_active()?;
		self.apply_runtime_change(PendingChange::UnregisterProvider { name: name.to_string() })
	}

	pub fn register_virtual_model(&self, definition: &Value) -> Result<(), String> {
		self.assert_active()?;
		let extension_path = self.extension.lock().unwrap().path.clone();
		self.apply_runtime_change(PendingChange::RegisterVirtualModel {
			definition: definition.clone(),
			extension_path,
		})
	}

	pub fn unregister_virtual_model(&self, provider: &str, id: &str) -> Result<(), String> {
		self.assert_active()?;
		self.apply_runtime_change(PendingChange::UnregisterVirtualModel {
			provider: provider.to_string(),
			id: id.to_string(),
		})
	}

	/// `pi.registerMcpServer(name, config)`.
	pub fn register_mcp_server(&self, name: &str, config: &Value) -> Result<(), String> {
		self.assert_active()?;
		let extension_path = self.extension.lock().unwrap().path.clone();
		let validated = validate_mcp_server_config(name, config).map_err(|error| {
			format!("Invalid MCP server registered by extension \"{}\": {}", extension_path, error)
		})?;
		let owner = self
			.runtime
			.mcp_servers()
			.lock()
			.unwrap()
			.get(name)
			.map(|server| server.extension_path.clone());
		if let Some(owner) = owner.filter(|owner| owner != &extension_path) {
			return Err(format!("MCP server \"{}\" is already registered by extension \"{}\"", name, owner));
		}
		let clash = self
			.runtime
			.mcp_servers()
			.lock()
			.unwrap()
			.list()
			.into_iter()
			.find(|server| server.name != name && mcp_namespace(&server.name) == mcp_namespace(name))
			.map(|server| server.name);
		if let Some(clash) = clash {
			return Err(format!("MCP server \"{}\" conflicts with registered server \"{}\"", name, clash));
		}
		self.apply_runtime_change(PendingChange::RegisterMcpServer {
			server: RegisteredMcpServer {
				name: name.to_string(),
				config: validated,
				extension_path,
			},
		})
	}

	pub fn unregister_mcp_server(&self, name: &str) -> Result<(), String> {
		self.assert_active()?;
		let extension_path = self.extension.lock().unwrap().path.clone();
		self.apply_runtime_change(PendingChange::UnregisterMcpServer {
			name: name.to_string(),
			extension_path,
		})
	}

	pub fn get_mcp_servers(&self) -> Result<Vec<RegisteredMcpServer>, String> {
		self.assert_active()?;
		Ok(self.runtime.mcp_servers().lock().unwrap().list())
	}

	/// `pi.events.emit(channel, data)`.
	pub fn events_emit(&self, channel: &str, data: &Value) -> Result<(), String> {
		self.assert_active()?;
		match &self.bus {
			Some(bus) => {
				bus.emit(channel, data);
				Ok(())
			}
			None => Ok(()),
		}
	}

	/// `pi.events.on(channel, handler)` — tracked until invalidation.
	pub fn events_on(&self, channel: &str, handler: pi_core::event_bus::EventHandler) -> Result<(), String> {
		self.assert_active()?;
		let subscription = match &self.bus {
			Some(bus) => bus.on(channel, move |data| handler(data)),
			None => return Ok(()),
		};
		self.runtime.track_event_bus_subscription(subscription);
		Ok(())
	}

	/// `commit()` — flush pending flags and runtime changes.
	pub fn commit(&self) -> Result<(), String> {
		let mut loading = self.loading.lock().unwrap();
		if loading.state != "loading" {
			return Ok(());
		}
		// TS asserts before flushing; a stale runtime leaves the API in
		// `loading` so `discard()` still runs.
		self.runtime.assert_active()?;
		let pending_flags = std::mem::take(&mut loading.pending_flag_values);
		let pending_changes = std::mem::take(&mut loading.pending_changes);
		loading.state = "active".to_string();
		drop(loading);
		for (name, value) in pending_flags {
			if !self.runtime.has_flag_value(&name) {
				self.runtime.set_flag_value(&name, value);
			}
		}
		for change in &pending_changes {
			change.apply(&self.runtime)?;
		}
		Ok(())
	}

	/// `discard()` — failed state; loading bus subscriptions are dropped.
	pub fn discard(&self) {
		let mut loading = self.loading.lock().unwrap();
		if loading.state != "loading" {
			return;
		}
		loading.state = "failed".to_string();
		let bus_start = loading.bus_start;
		loading.pending_flag_values.clear();
		loading.pending_changes.clear();
		drop(loading);
		self.runtime.unsubscribe_from(bus_start);
	}
}

/// TS `loadExtensionFromFactory(factory, cwd, eventBus, runtime,
/// extensionPath = "<inline>")` — `Err` rethrows the factory (or commit)
/// error after discarding. Returns the live extension handle: the runner
/// and any captured API share it, so post-load `pi.on`/unsubscribes are
/// visible to both (TS object reference semantics).
pub async fn load_extension_from_factory(
	factory: FactoryFn,
	cwd: &str,
	bus: Option<Arc<EventBusController>>,
	runtime: &ExtensionRuntime,
	extension_path: &str,
) -> Result<Arc<Mutex<Extension>>, String> {
	let mut extension = Extension {
		path: extension_path.to_string(),
		resolved_path: extension_path.to_string(),
		hidden: false,
		replaceable: false,
		source_info: source_info_value(extension_path, extension_path),
		handlers: Vec::new(),
		tools: Vec::new(),
		message_renderers: Vec::new(),
		entry_renderers: Vec::new(),
		markdown_transformer: false,
		tool_renderers: 0,
		commands: Vec::new(),
		flags: Vec::new(),
		shortcuts: Vec::new(),
	};
	if let Some(name) = extension_path.strip_prefix("<inline:") {
		let _ = name;
	}
	extension.replaceable = extension_path.starts_with("<inline:");
	let api = ExtensionApi {
		extension: Arc::new(Mutex::new(extension)),
		runtime: runtime.clone(),
		loading: Arc::new(Mutex::new(LoadingState {
			state: "loading".to_string(),
			pending_flag_values: Vec::new(),
			pending_changes: Vec::new(),
			bus_start: runtime.tracked_count(),
			next_handler_id: 0,
		})),
		bus,
		cwd: cwd.to_string(),
	};
	match factory(&api).await {
		Ok(()) => match api.commit() {
			// A commit failure discards like the TS `initializeExtension`
			// catch block.
			Ok(()) => Ok(Arc::clone(&api.extension)),
			Err(error) => {
				api.discard();
				Err(error)
			}
		},
		Err(error) => {
			api.discard();
			Err(error)
		}
	}
}

// ============================================================================
// ExtensionRunner (runner.ts core slice)
// ============================================================================

pub struct RunnerShared {
	pub(crate) extensions: Mutex<Vec<Arc<Mutex<Extension>>>>,
	runtime: ExtensionRuntime,
	cwd: String,
	mode: Mutex<String>,
	error_listeners: Mutex<Vec<(u64, ErrorListener)>>,
	next_listener_id: AtomicU64,
	reported_mcp_servers: Mutex<HashSet<String>>,
	stale_message: Mutex<Option<String>>,
	context_actions: Mutex<Option<Arc<dyn ContextActions>>>,
	command_actions: Mutex<Option<Arc<dyn CommandContextActions>>>,
	/// `setUIContext` target; `None` = the no-op context.
	ui_context: Mutex<Option<Arc<dyn UiContext>>>,
	/// `uiPromptDepth` (outermost prompt wins) and the active prompt.
	ui_prompt_depth: Mutex<i64>,
	active_ui_prompt: Mutex<Option<(String, Option<String>)>>,
	/// `ui_prompt_start`/`ui_prompt_end` events deferred through
	/// `queueMicrotask`; the host drains and emits them.
	ui_prompt_queue: Mutex<Vec<Value>>,
	/// `mcp_servers_change` events queued by the registry listener; drained
	/// by [`ExtensionRunner::drain_mcp_events`].
	mcp_event_queue: Mutex<Vec<McpEventJob>>,
	shortcut_diagnostics: Mutex<Vec<Value>>,
	model_registry: Option<Arc<dyn ModelRegistrySeam>>,
	/// Host paths for the default `docs` prompt section (TS resolves them
	/// from the package install; see [`ExtensionRunner::set_doc_paths`]).
	doc_paths: Mutex<DocPaths>,
}

/// One deferred `mcp_servers_change` emit: the event plus the handler
/// snapshot taken at push time (a handler registered later in the same op
/// does not observe the mutation).
pub struct McpEventJob {
	pub event: Value,
	pub handlers: Vec<(String, Vec<ExtensionHandler>)>,
}

/// TS `ExtensionRunner` — the tranche-1 core slice.
#[derive(Clone)]
pub struct ExtensionRunner {
	shared: Arc<RunnerShared>,
}

impl ExtensionRunner {
	/// `new ExtensionRunner(extensions, runtime, cwd, sessionManager,
	/// modelRegistry)`. `session_manager` is not consumed by this slice.
	pub fn new(
		extensions: Vec<Arc<Mutex<Extension>>>,
		runtime: ExtensionRuntime,
		cwd: &str,
		model_registry: Option<Arc<dyn ModelRegistrySeam>>,
	) -> ExtensionRunner {
		ExtensionRunner {
			shared: Arc::new(RunnerShared {
				extensions: Mutex::new(extensions),
				runtime,
				cwd: cwd.to_string(),
				mode: Mutex::new("print".to_string()),
				error_listeners: Mutex::new(Vec::new()),
				next_listener_id: AtomicU64::new(0),
				reported_mcp_servers: Mutex::new(HashSet::new()),
				stale_message: Mutex::new(None),
				context_actions: Mutex::new(None),
				command_actions: Mutex::new(None),
				ui_context: Mutex::new(None),
				ui_prompt_depth: Mutex::new(0),
				active_ui_prompt: Mutex::new(None),
				ui_prompt_queue: Mutex::new(Vec::new()),
				mcp_event_queue: Mutex::new(Vec::new()),
				shortcut_diagnostics: Mutex::new(Vec::new()),
				model_registry,
				doc_paths: Mutex::new(DocPaths::default()),
			}),
		}
	}

	pub fn runtime(&self) -> &ExtensionRuntime {
		&self.shared.runtime
	}

	fn snapshot_handlers(&self, event_type: &str) -> Vec<(String, Vec<(u64, ExtensionHandler)>)> {
		self.shared
			.extensions
			.lock()
			.unwrap()
			.iter()
			.map(|ext| {
				let ext = ext.lock().unwrap();
				(
					ext.path.clone(),
					ext.handlers
						.iter()
						.find(|(e, _)| e == event_type)
						.map(|(_, handlers)| handlers.clone())
						.unwrap_or_default(),
				)
			})
			.collect()
	}

	/// `bindCore(actions, contextActions, providerActions?)`.
	/// `setUIContext(uiContext?, mode = "print")` — `None` restores the
	/// no-op context.
	pub fn set_ui_context(&self, ui: Option<Arc<dyn UiContext>>, mode: &str) {
		*self.shared.ui_context.lock().unwrap() = ui;
		*self.shared.mode.lock().unwrap() = mode.to_string();
	}

	/// `getUIContext()` — the (wrapped) context handle; `None` = no-op.
	pub fn get_ui_context(&self) -> Option<Arc<dyn UiContext>> {
		self.shared.ui_context.lock().unwrap().clone()
	}

	/// `hasUI()` — `uiContext !== noOpUIContext`.
	pub fn has_ui(&self) -> bool {
		self.shared.ui_context.lock().unwrap().is_some()
	}

	/// Drain the deferred `ui_prompt_*` events (TS `queueMicrotask(... emit)`).
	pub fn drain_ui_prompt_events(&self) -> Vec<Value> {
		std::mem::take(&mut *self.shared.ui_prompt_queue.lock().unwrap())
	}

	fn emit_ui_prompt_event(&self, event: Value) {
		self.shared.ui_prompt_queue.lock().unwrap().push(event);
	}

	/// `withUIPrompt(kind, title, run)` — the outermost prompt emits
	/// `ui_prompt_start`; the depth counter discards nested prompts and the
	/// end event carries the ACTIVE prompt (whose kind/title can differ when
	/// a nested prompt opened first... they cannot: nesting does not
	/// overwrite). `finish` clamps the depth at zero (double-guard).
	async fn with_ui_prompt<T>(
		&self,
		kind: &str,
		title: Option<&str>,
		run: impl std::future::Future<Output = Result<T, String>>,
	) -> Result<T, String> {
		let outer = {
			let mut depth = self.shared.ui_prompt_depth.lock().unwrap();
			let outer = *depth == 0;
			*depth += 1;
			outer
		};
		if outer {
			*self.shared.active_ui_prompt.lock().unwrap() =
				Some((kind.to_string(), title.map(str::to_string)));
			let mut event = json!({"type": "ui_prompt_start", "reason": "ui_prompt", "kind": kind});
			if let Some(title) = title {
				event["title"] = json!(title);
			}
			self.emit_ui_prompt_event(event);
		}
		let result = run.await;
		{
			let mut depth = self.shared.ui_prompt_depth.lock().unwrap();
			*depth -= 1;
			if *depth <= 0 {
				*depth = 0;
				let prompt = self
					.shared
					.active_ui_prompt
					.lock()
					.unwrap()
					.clone()
					.unwrap_or_else(|| (kind.to_string(), title.map(str::to_string)));
				*self.shared.active_ui_prompt.lock().unwrap() = None;
				let mut event = json!({"type": "ui_prompt_end", "reason": "ui_prompt", "kind": prompt.0});
				if let Some(title) = prompt.1 {
					event["title"] = json!(title);
				}
				self.emit_ui_prompt_event(event);
			}
		}
		result
	}

	async fn ui_invoke(&self, method: &str, args: &[Value]) -> Result<Value, String> {
		let ui = self.shared.ui_context.lock().unwrap().clone();
		match ui {
			Some(ui) => ui.invoke(method, args),
			None => {
				// noOpUIContext: async prompts resolve undefined/false, sync
				// setters no-op, getEditorText = "", themes = [], setTheme =
				// {success:false,error:"UI not available"}.
				Ok(match method {
					"confirm" => json!(false),
					"getEditorText" => json!(""),
					"getAllThemes" => json!([]),
					"setTheme" => json!({"success": false, "error": "UI not available"}),
					"getToolsExpanded" => json!(false),
					_ => Value::Null,
				})
			}
		}
	}

	/// `withUIPrompt`-wrapped `ui.<kind>(...args)` (`select`/`confirm`/
	/// `input`/`editor`/`custom`); `Null` = undefined.
	pub async fn ui_prompt(
		&self,
		kind: &str,
		title: Option<&str>,
		args: Vec<Value>,
	) -> Result<Value, String> {
		// `setUIContext(undefined)` leaves the noOp context UNWRAPPED: prompts
		// resolve without ui_prompt_* events.
		if self.get_ui_context().is_none() {
			return self.ui_invoke(kind, &args).await;
		}
		self.with_ui_prompt(kind, title, async { self.ui_invoke(kind, &args).await })
			.await
	}

	/// `ctx.ui.select(title, options, opts?)`.
	pub async fn ui_select(
		&self,
		title: &str,
		options: &[String],
		opts: Value,
	) -> Result<Value, String> {
		self.ui_prompt("select", Some(title), vec![json!(title), json!(options), opts])
			.await
	}

	/// `ctx.ui.confirm(title, message, opts?)`.
	pub async fn ui_confirm(
		&self,
		title: &str,
		message: &str,
		opts: Value,
	) -> Result<Value, String> {
		self.ui_prompt("confirm", Some(title), vec![json!(title), json!(message), opts])
			.await
	}

	/// `ctx.ui.input(title, placeholder?, opts?)`.
	pub async fn ui_input(
		&self,
		title: &str,
		placeholder: Option<&str>,
		opts: Value,
	) -> Result<Value, String> {
		self.ui_prompt(
			"input",
			Some(title),
			vec![json!(title), placeholder.map(|p| json!(p)).unwrap_or(Value::Null), opts],
		)
		.await
	}

	/// `ctx.ui.editor(title, prefill?)`.
	pub async fn ui_editor(
		&self,
		title: &str,
		prefill: Option<&str>,
	) -> Result<Value, String> {
		self.ui_prompt(
			"editor",
			Some(title),
			vec![json!(title), prefill.map(|p| json!(p)).unwrap_or(Value::Null)],
		)
		.await
	}

	/// `ctx.ui.custom(factory, options?)` — the factory is not modelable;
	/// `args` carries the harness-shaped call.
	pub async fn ui_custom(&self, args: Vec<Value>) -> Result<Value, String> {
		self.ui_prompt("custom", None, args).await
	}

	/// `bindCommandContext(actions?)` — `None` resets every handler to the
	/// TS defaults (`{ cancelled: false }` / no-op).
	pub fn bind_command_context(&self, actions: Option<Arc<dyn CommandContextActions>>) {
		*self.shared.command_actions.lock().unwrap() = actions;
	}

	pub fn bind_core(
		&self,
		actions: Arc<dyn RuntimeActions>,
		context_actions: Arc<dyn ContextActions>,
		provider_hooks: ProviderHooks,
	) {
		{
			let mut inner = self.shared.runtime.inner.lock().unwrap();
			inner.actions = Some(actions);
			inner.provider_sink = Some(ProviderSink {
				hooks: provider_hooks,
				model_registry: self.shared.model_registry.clone(),
			});
		}
		*self.shared.context_actions.lock().unwrap() = Some(context_actions);
		// The MCP change listener: queue the `mcp_servers_change` emit (the
		// host drains it) and report unhandled servers synchronously.
		let shared = Arc::clone(&self.shared);
		self.shared
			.runtime
			.mcp_servers()
			.lock()
			.unwrap()
			.set_change_listener(Some(Box::new(move |servers: &[RegisteredMcpServer]| {
				let event_servers = servers
					.iter()
					.map(|server| serde_json::to_value(server).unwrap_or(Value::Null))
					.collect::<Vec<_>>();
				let runner = ExtensionRunner { shared: Arc::clone(&shared) };
				let job = McpEventJob {
					event: json!({ "type": "mcp_servers_change", "servers": event_servers }),
					handlers: runner
						.snapshot_handlers("mcp_servers_change")
						.into_iter()
						.map(|(path, handlers)| (path, handlers.into_iter().map(|(_, handler)| handler).collect()))
						.collect(),
				};
				shared.mcp_event_queue.lock().unwrap().push(job);
				// The listener runs with the registry lock held (it fires
				// inside register/unregister), so the unhandled-server report
				// consumes the passed list instead of re-locking.
				ExtensionRunner { shared: Arc::clone(&shared) }.report_unhandled_from(servers);
			})));
		// Flush provider registrations queued during extension loading
		// (per-item error capture; the loop never aborts).
		let pending_providers = self.shared.runtime.pending_provider_registrations();
		for (name, config, extension_path) in pending_providers {
			if let Err(error) = self.shared.runtime.register_provider(&name, &config, &extension_path) {
				self.emit_error(&json!({
					"extensionPath": extension_path,
					"event": "register_provider",
					"error": error,
				}));
			}
		}
		self.shared.runtime.inner.lock().unwrap().pending_providers.clear();
		let pending_native = self.shared.runtime.pending_native_provider_registrations();
		for (provider, extension_path) in pending_native {
			if let Err(error) = self.shared.runtime.register_native_provider(&provider, &extension_path) {
				self.emit_error(&json!({
					"extensionPath": extension_path,
					"event": "register_provider",
					"error": error,
				}));
			}
		}
		self.shared.runtime.inner.lock().unwrap().pending_native.clear();
		let pending_virtual = self.shared.runtime.pending_virtual_model_registrations();
		for (definition, extension_path) in pending_virtual {
			if let Err(error) = self.shared.runtime.register_virtual_model(&definition, &extension_path) {
				self.emit_error(&json!({
					"extensionPath": extension_path,
					"event": "register_virtual_model",
					"error": error,
				}));
			}
		}
		self.shared.runtime.inner.lock().unwrap().pending_virtual.clear();
	}

	/// Drain queued `mcp_servers_change` emits (the TS fires them with `void
	/// this.emit(...)`; the harness settles symmetrically).
	pub async fn drain_mcp_events(&self) {
		for job in self.drain_mcp_jobs() {
			self.emit(job.event).await;
		}
	}

	/// Hand the deferred jobs to the host (the parity harness schedules the
	/// handler microtasks itself — see the plan's Addendum B).
	pub fn drain_mcp_jobs(&self) -> Vec<McpEventJob> {
		std::mem::take(&mut *self.shared.mcp_event_queue.lock().unwrap())
	}

	/// The `createContext()` view (used by the host for stepped emits).
	pub fn ctx(&self) -> CtxView {
		CtxView::Runner(Arc::clone(&self.shared))
	}

	fn assert_active_shared(&self) -> Result<(), String> {
		match self.shared.stale_message.lock().unwrap().clone() {
			Some(message) => Err(message),
			None => Ok(()),
		}
	}

	fn ctx_read(&self, field: &str) -> Result<Value, String> {
		if let Some(message) = self.shared.stale_message.lock().unwrap().clone() {
			return Err(message);
		}
		let actions = self.shared.context_actions.lock().unwrap().clone();
		match field {
			// Non-modelable objects (`ui`/`sessionManager`/`modelRegistry`
			// carry getters and functions); the harness reaches `ui`
			// through dedicated runner calls, never by reading it.
			"ui" | "sessionManager" | "modelRegistry" => Ok(Value::Null),
			"mode" => Ok(json!(*self.shared.mode.lock().unwrap())),
			"hasUI" => Ok(json!(self.has_ui())),
			"cwd" => Ok(json!(self.shared.cwd)),
			"model" => Ok(actions
				.as_ref()
				.and_then(|actions| actions.get_model())
				.unwrap_or(Value::Null)),
			"scopedModels" => Ok(json!(actions
				.as_ref()
				.map(|actions| actions.get_scoped_models())
				.unwrap_or_default())),
			// `getThinkingLevel()` is the loader stub pre-bind (throws).
			"thinkingLevel" => self.shared.runtime.get_thinking_level(),
			"signal" => Ok(actions
				.as_ref()
				.and_then(|actions| actions.get_signal())
				.unwrap_or(Value::Null)),
			_ => Ok(Value::Null),
		}
	}

	/// Guarded context method invocation (`ctx.shutdown()`, `ctx.compact()`).
	pub fn ctx_call_args(&self, method: &str, args: &[Value]) -> Result<Value, String> {
		if let Some(message) = self.shared.stale_message.lock().unwrap().clone() {
			return Err(message);
		}
		let actions = self.shared.context_actions.lock().unwrap().clone();
		match method {
			"isIdle" => Ok(json!(actions
				.as_ref()
				.map(|actions| actions.is_idle())
				.unwrap_or(true))),
			"isProjectTrusted" => Ok(json!(actions
				.as_ref()
				.map(|actions| actions.is_project_trusted())
				.unwrap_or(true))),
			"abort" => {
				if let Some(actions) = &actions {
					actions.abort();
				}
				Ok(Value::Null)
			}
			"hasPendingMessages" => Ok(json!(actions
				.as_ref()
				.map(|actions| actions.has_pending_messages())
				.unwrap_or(false))),
			"shutdown" => {
				if let Some(actions) = &actions {
					actions.shutdown();
				}
				Ok(Value::Null)
			}
			"getContextUsage" => Ok(actions
				.as_ref()
				.and_then(|actions| actions.get_context_usage())
				.unwrap_or(Value::Null)),
			"compact" => {
				if let Some(actions) = &actions {
					actions.compact(args.first().cloned().unwrap_or(Value::Null))?;
				}
				Ok(Value::Null)
			}
			"getSystemPrompt" => Ok(json!(actions
				.as_ref()
				.map(|actions| actions.get_system_prompt())
				.unwrap_or_default())),
			_ => Ok(Value::Null),
		}
	}

	/// `emitUserBash(event)` — the first non-undefined handler result must
	/// validate as exactly one of `{operations}` or `{result}`; a handler
	/// throw or an invalid result is reported and **rethrown**.
	pub async fn emit_user_bash(&self, event: Value) -> Result<Option<Value>, String> {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		for (extension_path, handlers) in self.snapshot_handlers("user_bash") {
			for (_, handler) in handlers {
				match handler(event.clone(), ctx.clone()).await {
					Ok(HandlerOutcome { result: Some(result), .. }) => {
						if !is_user_bash_event_result(&result) {
							let message = "Invalid user_bash handler result: return undefined for local execution or exactly one valid { operations } or { result } object";
							self.emit_error(&json!({
								"extensionPath": extension_path,
								"event": "user_bash",
								"error": message,
							}));
							return Err(message.to_string());
						}
						return Ok(Some(result));
					}
					Ok(HandlerOutcome { result: None, .. }) => {}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "user_bash",
							"error": error,
						}));
						return Err(error);
					}
				}
			}
		}
		Ok(None)
	}

	/// `emitCacheWarmingDecision(event)` — the event's own action unless a
	/// handler overrides it (`result.action !== undefined`); last wins.
	pub async fn emit_cache_warming_decision(&self, event: Value) -> Value {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let event_type = event.get("type").and_then(Value::as_str).unwrap_or("").to_string();
		let mut action = event.get("action").cloned().unwrap_or(Value::Null);
		for (extension_path, handlers) in self.snapshot_handlers(&event_type) {
			for (_, handler) in handlers {
				match handler(event.clone(), ctx.clone()).await {
					Ok(HandlerOutcome { result: Some(result), .. }) => {
						// `result?.action !== undefined` — present null overrides.
						if let Some(next) = result.get("action") {
							action = next.clone();
						}
					}
					Ok(_) => {}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": event_type,
							"error": error,
						}));
					}
				}
			}
		}
		action
	}

	/// `emitToolResult(event)` — the single `currentEvent` is patched in
	/// order; replacing `content` without `structuredContent` DELETES the
	/// stale structured content; the return carries only present fields.
	pub async fn emit_tool_result(&self, event: Value) -> Result<Option<Value>, String> {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let mut current_event = event.clone();
		if !current_event.is_object() {
			current_event = json!({});
		}
		let mut modified = false;
		for (extension_path, handlers) in self.snapshot_handlers("tool_result") {
			for (_, handler) in handlers {
				match handler(current_event.clone(), ctx.clone()).await {
					Ok(HandlerOutcome { event: returned_event, result }) => {
						// TS passes the SAME `currentEvent` object to every
						// handler: in-place edits are visible downstream and in
						// the merged result even when the handler returns
						// nothing.
						current_event = returned_event;
						let Some(handler_result) = result else {
							continue;
						};
						if !js_truthy(&handler_result) {
							continue;
						}
						if let Some(content) = handler_result.get("content") {
							set_field(&mut current_event, "content", content.clone());
							if handler_result.get("structuredContent").is_none() {
								if let Some(object) = current_event.as_object_mut() {
									object.remove("structuredContent");
								}
							}
							modified = true;
						}
						if let Some(details) = handler_result.get("details") {
							set_field(&mut current_event, "details", details.clone());
							modified = true;
						}
						if let Some(structured) = handler_result.get("structuredContent") {
							set_field(&mut current_event, "structuredContent", structured.clone());
							modified = true;
						}
						if let Some(is_error) = handler_result.get("isError") {
							set_field(&mut current_event, "isError", is_error.clone());
							modified = true;
						}
						if let Some(usage) = handler_result.get("usage") {
							set_field(&mut current_event, "usage", usage.clone());
							modified = true;
						}
					}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "tool_result",
							"error": error,
						}));
					}
				}
			}
		}
		if !modified {
			return Ok(None);
		}
		let mut result = Map::new();
		for key in ["content", "details", "structuredContent", "isError", "usage"] {
			if let Some(value) = current_event.get(key) {
				result.insert(key.to_string(), value.clone());
			}
		}
		Ok(Some(Value::Object(result)))
	}

	/// `emitToolCall(event)` — truthy results accumulate (last wins), a
	/// truthy `block` short-circuits; handler throws are NOT caught.
	pub async fn emit_tool_call(&self, event: Value) -> Result<Option<Value>, String> {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let mut current_event = event;
		let mut result: Option<Value> = None;
		for (_, handlers) in self.snapshot_handlers("tool_call") {
			for (_, handler) in handlers {
				let outcome = handler(current_event.clone(), ctx.clone()).await?;
				current_event = outcome.event;
				if let Some(handler_result) = outcome.result {
					if js_truthy(&handler_result) {
						if handler_result.get("block").map(js_truthy).unwrap_or(false) {
							return Ok(Some(handler_result));
						}
						result = Some(handler_result);
					}
				}
			}
		}
		Ok(result)
	}

	/// `emitBeforeProviderRequest(payload)` — any non-undefined result
	/// replaces the payload (last wins).
	pub async fn emit_before_provider_request(&self, payload: Value) -> Value {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let mut current = payload;
		for (extension_path, handlers) in self.snapshot_handlers("before_provider_request") {
			for (_, handler) in handlers {
				let event = json!({"type": "before_provider_request", "payload": current.clone()});
				match handler(event, ctx.clone()).await {
					Ok(HandlerOutcome { event: returned, result }) => {
						// `event.payload` aliases `currentPayload`: in-place
						// edits persist; a non-undefined result replaces it.
						if let Some(payload) = returned.get("payload") {
							current = payload.clone();
						}
						if let Some(result) = result {
							current = result;
						}
					}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "before_provider_request",
							"error": error,
						}));
					}
				}
			}
		}
		current
	}

	/// `emitBeforeProviderHeaders(headers)` — handlers mutate `headers` in
	/// place (return value ignored); mutations accumulate.
	pub async fn emit_before_provider_headers(&self, headers: Value) -> Value {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let mut current = headers;
		for (extension_path, handlers) in self.snapshot_handlers("before_provider_headers") {
			for (_, handler) in handlers {
				let event = json!({"type": "before_provider_headers", "headers": current.clone()});
				match handler(event, ctx.clone()).await {
					Ok(HandlerOutcome { event: returned, .. }) => {
						if let Some(next) = returned.get("headers") {
							current = next.clone();
						}
					}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "before_provider_headers",
							"error": error,
						}));
					}
				}
			}
		}
		current
	}

	/// `emitResourcesDiscover(cwd, reason)` — collects each handler's
	/// non-empty path lists tagged with the extension path.
	pub async fn emit_resources_discover(&self, cwd: &str, reason: Value) -> Value {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let mut skill_paths: Vec<Value> = Vec::new();
		let mut prompt_paths: Vec<Value> = Vec::new();
		let mut theme_paths: Vec<Value> = Vec::new();
		for (extension_path, handlers) in self.snapshot_handlers("resources_discover") {
			for (_, handler) in handlers {
				let event = json!({"type": "resources_discover", "cwd": cwd, "reason": reason.clone()});
				match handler(event, ctx.clone()).await {
					Ok(HandlerOutcome { result: Some(result), .. }) => {
						collect_paths(&result, "skillPaths", &extension_path, &mut skill_paths);
						collect_paths(&result, "promptPaths", &extension_path, &mut prompt_paths);
						collect_paths(&result, "themePaths", &extension_path, &mut theme_paths);
					}
					Ok(_) => {}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "resources_discover",
							"error": error,
						}));
					}
				}
			}
		}
		json!({"skillPaths": skill_paths, "promptPaths": prompt_paths, "themePaths": theme_paths})
	}

	/// `emitInput(text, images, source, streamingBehavior)` — transforms
	/// chain, `handled` short-circuits; the final result is `transform` only
	/// when something changed.
	pub async fn emit_input(
		&self,
		text: &str,
		images: Option<Value>,
		source: Value,
		streaming_behavior: Option<Value>,
	) -> Value {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let original_text = json!(text);
		let original_images = images.clone();
		let mut current_text: Option<Value> = Some(original_text.clone());
		let mut current_images: Option<Value> = original_images.clone();
		for (extension_path, handlers) in self.snapshot_handlers("input") {
			for (_, handler) in handlers {
				let mut event = json!({"type": "input", "source": source});
				if let Some(value) = &current_text {
					event["text"] = value.clone();
				}
				if let Some(value) = &current_images {
					event["images"] = value.clone();
				}
				if let Some(behavior) = &streaming_behavior {
					event["streamingBehavior"] = behavior.clone();
				}
				match handler(event, ctx.clone()).await {
					Ok(HandlerOutcome { result: Some(result), .. }) => {
						match result.get("action").and_then(Value::as_str) {
							Some("handled") => return result,
							Some("transform") => {
								// `currentText = result.text` (undefined stays
								// undefined → the key disappears on jstr).
								current_text = result.get("text").cloned();
								if let Some(images) = result.get("images") {
									if !images.is_null() {
										current_images = Some(images.clone());
									}
								}
							}
							_ => {}
						}
					}
					Ok(_) => {}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "input",
							"error": error,
						}));
					}
				}
			}
		}
		if current_text != Some(original_text) || current_images != original_images {
			let mut result = Map::new();
			result.insert("action".to_string(), json!("transform"));
			if let Some(text) = current_text {
				result.insert("text".to_string(), text);
			}
			if let Some(images) = current_images {
				result.insert("images".to_string(), images);
			}
			Value::Object(result)
		} else {
			json!({"action": "continue"})
		}
	}

	/// `emitBoundary(baseEvent, buildContext)` — the host rebuild hook runs
	/// once up front (a throw escapes) and after every handler; a failing
	/// rebuild marks the batch invalid (`Invalid boundary entries: …`).
	pub async fn emit_boundary(
		&self,
		base_event: Value,
		build_context: BoundaryBuildContext,
	) -> Result<Value, String> {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let event_type = base_event.get("type").and_then(Value::as_str).unwrap_or("").to_string();
		let mut entries: Vec<Value> = Vec::new();
		let mut should_continue = false;
		let mut context = build_context(entries.clone()).await?;
		let mut valid = true;
		for (extension_path, handlers) in self.snapshot_handlers(&event_type) {
			for (_, handler) in handlers {
				let mut event = base_event.clone();
				if let Some(object) = event.as_object_mut() {
					object.insert("entries".to_string(), json!(entries));
					object.insert("continue".to_string(), json!(should_continue));
					object.insert("context".to_string(), context.clone());
				}
				match handler(event, ctx.clone()).await {
					Ok(HandlerOutcome { result: Some(result), .. }) => {
						if let Some(next) = result.get("entries") {
							entries = next.as_array().cloned().unwrap_or_default();
						}
						if let Some(next) = result.get("continue") {
							should_continue = js_truthy(next);
						}
					}
					Ok(_) => {}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": event_type,
							"error": error,
						}));
					}
				}
				match build_context(entries.clone()).await {
					Ok(next) => {
						context = next;
						valid = true;
					}
					Err(message) => {
						valid = false;
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": event_type,
							"error": format!("Invalid boundary entries: {}", message),
						}));
					}
				}
			}
		}
		if valid {
			Ok(json!({"entries": entries, "continue": should_continue, "context": context, "valid": true}))
		} else {
			Ok(json!({"entries": [], "continue": false, "context": context, "valid": false}))
		}
	}

	/// `emitContext(messages)` — two phases: `context` handlers see the
	/// conversation without system messages (with `restoreSystemMessages`
	/// re-attaching the replay head), then `context_with_system` handlers
	/// see the full transcript; losing a leading system message is reported
	/// but honored.
	pub async fn emit_context(&self, messages: Value) -> Result<Value, String> {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let mut current: Vec<Value> = match messages {
			Value::Array(array) => array,
			_ => Vec::new(),
		};

		for (extension_path, handlers) in self.snapshot_handlers("context") {
			for (_, handler) in handlers {
				let outcome: Result<(), String> = async {
					let visible: Vec<Value> = current
						.iter()
						.filter(|message| message.get("role").and_then(Value::as_str) != Some("system"))
						.cloned()
						.collect();
					let snapshot = visible.clone();
					let event = json!({"type": "context", "messages": visible.clone()});
					let outcome = handler(event, ctx.clone()).await?;
					// In-place edits of `event.messages` are the visible array.
					let visible_after: Vec<Value> = outcome
						.event
						.get("messages")
						.and_then(Value::as_array)
						.cloned()
						.unwrap_or(visible);
					// `handlerResult?.messages ?? (same ? undefined : visible)`.
					let returned = match outcome
						.result
						.as_ref()
						.and_then(|result| result.get("messages"))
					{
						Some(value) if !value.is_null() => Some(value.clone()),
						_ => {
							if same_messages(&visible_after, &snapshot) {
								None
							} else {
								Some(json!(visible_after))
							}
						}
					};
					let Some(returned) = returned else { return Ok::<(), String>(()) };
					// `if (!returned) continue` — jstr-model falsy values.
					if !js_truthy(&returned) {
						return Ok(());
					}
					let Some(returned) = returned.as_array() else {
						return Err("returned is not iterable".to_string());
					};
					current = restore_system_messages(&current, &snapshot, returned)?;
					Ok(())
				}
				.await;
				if let Err(error) = outcome {
					self.emit_error(&json!({
						"extensionPath": extension_path,
						"event": "context",
						"error": error,
					}));
				}
			}
		}

		for (extension_path, handlers) in self.snapshot_handlers("context_with_system") {
			for (_, handler) in handlers {
				let outcome: Result<(), String> = async {
					let had_leading = current
						.first()
						.and_then(|message| message.get("role"))
						.and_then(Value::as_str)
						== Some("system");
					let event = json!({"type": "context_with_system", "messages": current.clone()});
					let outcome = handler(event, ctx.clone()).await?;
					current = match outcome.result.as_ref().and_then(|result| result.get("messages")) {
						Some(value) if !value.is_null() => value
							.as_array()
							.cloned()
							.unwrap_or_else(|| current.clone()),
						_ => outcome
							.event
							.get("messages")
							.and_then(Value::as_array)
							.cloned()
							.unwrap_or_else(|| current.clone()),
					};
					let leading_now = current
						.first()
						.and_then(|message| message.get("role"))
						.and_then(Value::as_str)
						== Some("system");
					if had_leading && !leading_now {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "context_with_system",
							"error": "Handler removed the leading system message; the request has no prompt or initial tool declarations. Keep it at index 0 or replace a dropped prefix with getCurrentSystemMessage().",
						}));
					}
					Ok::<(), String>(())
				}
				.await;
				if let Err(error) = outcome {
					self.emit_error(&json!({
						"extensionPath": extension_path,
						"event": "context_with_system",
						"error": error,
					}));
				}
			}
		}

		Ok(json!(current))
	}

	/// `createToolContext(toolCallId, signal)` — the guarded `tools` getter
	/// plus `executeTool()` (nested-call fallback when the context actions
	/// do not bind it).
	pub fn create_tool_context(
		&self,
		tool_call_id: &str,
		signal: Value,
	) -> ToolContextView {
		ToolContextView {
			runner: self.clone(),
			tool_call_id: tool_call_id.to_string(),
			signal,
		}
	}

	/// `createCommandContext()` — `createContext()` plus the command-only
	/// members, all guarded by `assertActive`.
	pub fn create_command_context(&self) -> CommandContextView {
		CommandContextView { runner: self.clone() }
	}

	/// `hasHandlers(eventType)`.
	pub fn has_handlers(&self, event_type: &str) -> bool {
		self.snapshot_handlers(event_type).iter().any(|(_, handlers)| !handlers.is_empty())
	}

	/// The generic `emit(event)` — session_before_* results accumulate and
	/// `cancel` short-circuits; handler errors go to `emitError`.
	pub async fn emit(&self, event: Value) -> Option<Value> {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let event_type = event.get("type").and_then(Value::as_str).unwrap_or("").to_string();
		let is_session_before = matches!(
			event_type.as_str(),
			"session_before_switch" | "session_before_fork" | "session_before_compact" | "session_before_tree"
		);
		let mut result: Option<Value> = None;
		for (extension_path, handlers) in self.snapshot_handlers(&event_type) {
			for (_, handler) in handlers {
				match handler(event.clone(), ctx.clone()).await {
					Ok(HandlerOutcome { result: handler_result, .. }) => {
						if is_session_before && handler_result.as_ref().is_some_and(js_truthy) {
							result = handler_result;
							if result.as_ref().and_then(|r| r.get("cancel")) == Some(&Value::Bool(true)) {
								return result;
							}
						}
					}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": event_type,
							"error": error,
						}));
					}
				}
			}
		}
		result
	}

	/// `emitMessageEnd(event)` — last same-role replacement wins.
	pub async fn emit_message_end(&self, event: Value) -> Option<Value> {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let mut current_message = event.get("message").cloned().unwrap_or(Value::Null);
		let mut modified = false;
		for (extension_path, handlers) in self.snapshot_handlers("message_end") {
			for (_, handler) in handlers {
				let mut current_event = event.clone();
				if let Some(object) = current_event.as_object_mut() {
					object.insert("message".to_string(), current_message.clone());
				}
				match handler(current_event, ctx.clone()).await {
					Ok(HandlerOutcome { event: returned, result: handler_result }) => {
						// `event.message` aliases `currentMessage`: in-place
						// edits persist even when the handler returns nothing.
						let in_place = returned.get("message").cloned().unwrap_or_else(|| current_message.clone());
						let replacement = handler_result
							.as_ref()
							.and_then(|result| result.get("message"))
							.filter(|message| js_truthy(message))
							.cloned();
						let Some(replacement) = replacement else {
							current_message = in_place;
							continue;
						};
						if replacement.get("role") != current_message.get("role") {
							current_message = in_place;
							self.emit_error(&json!({
								"extensionPath": extension_path,
								"event": "message_end",
								"error": "message_end handlers must return a message with the same role",
							}));
							continue;
						}
						current_message = replacement;
						modified = true;
					}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "message_end",
							"error": error,
						}));
					}
				}
			}
		}
		if modified {
			Some(current_message)
		} else {
			None
		}
	}

	/// `onError(listener)` — returns the removal id.
	pub fn on_error(&self, listener: ErrorListener) -> u64 {
		let id = self.shared.next_listener_id.fetch_add(1, Ordering::SeqCst);
		self.shared.error_listeners.lock().unwrap().push((id, listener));
		id
	}

	pub fn remove_error_listener(&self, id: u64) {
		self.shared.error_listeners.lock().unwrap().retain(|(i, _)| *i != id);
	}

	/// `emitError(error)`.
	/// Host injection for the default `docs` prompt section paths.
	pub fn set_doc_paths(&self, paths: DocPaths) {
		*self.shared.doc_paths.lock().unwrap() = paths;
	}

	fn doc_paths(&self) -> DocPaths {
		self.shared.doc_paths.lock().unwrap().clone()
	}

	/// `emitBeforeAgentStart(trigger, systemPromptOptions)` — the leading
	/// normalize may throw (the emitter rejects; V8 error text); handler
	/// errors are reported. Returns `(messages, systemPromptOptions)`.
	pub async fn emit_before_agent_start(
		&self,
		trigger: Value,
		system_prompt_options: Value,
	) -> Result<(Vec<Value>, Value), String> {
		let mut current_options = normalize_options_raw(&system_prompt_options)?;
		let paths = self.doc_paths();
		let mut messages: Vec<Value> = Vec::new();
		for (extension_path, handlers) in self.snapshot_handlers("before_agent_start") {
			for (_, handler) in handlers {
				// `ctx.getSystemPrompt()` = assertActive + render(live options).
				let render_shared = Arc::clone(&self.shared);
				let render_options = current_options.clone();
				let render_paths = paths.clone();
				let renderer: Arc<dyn Fn() -> Result<String, String> + Send + Sync> =
					Arc::new(move || {
						if let Some(message) = render_shared.stale_message.lock().unwrap().clone() {
							return Err(message);
						}
						build_system_prompt(&render_options, &render_paths)
					});
				let ctx = CtxView::BeforeAgentStart {
					shared: Arc::clone(&self.shared),
					renderer,
				};

				// `{...trigger, type, get systemPrompt(), systemPromptOptions:
				// currentOptions}` — fresh per handler. TS renders the getter
				// lazily; options change only between handlers, so the value is
				// materialized here (a render error leaves the key absent where
				// TS throws at read — plan divergence list).
				let mut event = trigger.clone();
				if let Some(object) = event.as_object_mut() {
					object.insert("type".to_string(), json!("before_agent_start"));
					object.insert("systemPromptOptions".to_string(), current_options.clone());
					if let Ok(prompt) = build_system_prompt(&current_options, &paths) {
						object.insert("systemPrompt".to_string(), json!(prompt));
					}
				}

				match handler(event, ctx).await {
					Ok(HandlerOutcome { event: returned_event, result }) => {
						// In TS the event's options object IS `currentOptions`:
						// in-place edits are observable in the return value.
						if let Some(options) = returned_event.get("systemPromptOptions") {
							if options != &current_options {
								current_options = options.clone();
								// TS keys keep their literal slot even when their value
								// was undefined; re-insert newly-assigned ones in place.
								reorder_options_raw(&mut current_options);
							}
						}
						if let Some(result) = result {
							if js_truthy(&result) {
								if let Some(message) = result.get("message") {
									if js_truthy(message) {
										messages.push(message.clone());
									}
								}
								// `result.systemPrompt !== undefined` — present
								// null reassigns (probe-pinned).
								if let Some(prompt) = result.get("systemPrompt") {
									// Assignment into the live object: an EXISTING key
									// keeps its literal position (2nd); an absent one
									// is inserted there (TS object order).
									set_force_system_prompt(&mut current_options, prompt.clone());
								}
							}
						}
					}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "before_agent_start",
							"error": error,
						}));
					}
				}
			}
		}
		Ok((messages, current_options))
	}

	pub fn emit_error(&self, error: &Value) {
		for (_, listener) in self.shared.error_listeners.lock().unwrap().iter() {
			listener(error);
		}
	}

	/// `reportUnhandledMcpServers()`.
	pub fn report_unhandled_mcp_servers(&self) {
		if self.has_handlers("mcp_servers_change") {
			return;
		}
		let servers = self.shared.runtime.mcp_servers().lock().unwrap().list();
		self.report_unhandled_from(&servers);
	}

	/// The report over an already-snapshotted server list (used by the MCP
	/// change listener, which runs with the registry lock held).
	fn report_unhandled_from(&self, servers: &[RegisteredMcpServer]) {
		if self.has_handlers("mcp_servers_change") {
			return;
		}
		for server in servers {
			let mut reported = self.shared.reported_mcp_servers.lock().unwrap();
			if reported.contains(&server.name) {
				continue;
			}
			reported.insert(server.name.clone());
			drop(reported);
			self.emit_error(&json!({
				"extensionPath": server.extension_path,
				"event": "register_mcp_server",
				"error": format!("MCP server \"{}\" is registered, but no loaded extension connects MCP servers; another extension may have replaced the built-in MCP support", server.name),
			}));
		}
	}

	/// `getExtensionPaths()`.
	pub fn get_extension_paths(&self) -> Vec<String> {
		self.shared.extensions.lock().unwrap().iter().map(|ext| ext.lock().unwrap().path.clone()).collect()
	}

	/// `getAllRegisteredTools()` — first registration per name wins.
	pub fn get_all_registered_tools(&self) -> Vec<(Value, Value)> {
		let mut tools: Vec<(String, Value, Value)> = Vec::new();
		for extension_handle in self.shared.extensions.lock().unwrap().iter() {
			let extension = extension_handle.lock().unwrap();
			for (name, definition, source_info) in &extension.tools {
				if !tools.iter().any(|(n, _, _)| n == name) {
					tools.push((name.clone(), definition.clone(), source_info.clone()));
				}
			}
		}
		tools.into_iter().map(|(_, definition, source_info)| (definition, source_info)).collect()
	}

	/// `getToolDefinition(toolName)`.
	pub fn get_tool_definition(&self, tool_name: &str) -> Option<Value> {
		let extensions = self.shared.extensions.lock().unwrap();
		extensions
			.iter()
			.find_map(|ext| {
				let ext = ext.lock().unwrap();
				ext.tools.iter().find(|(name, _, _)| name == tool_name).map(|(_, definition, _)| definition.clone())
			})
			.or(None)
	}

	/// `getFlags()` — first registration per name wins.
	pub fn get_flags(&self) -> Vec<ExtensionFlag> {
		let mut names: Vec<String> = Vec::new();
		let mut flags: Vec<ExtensionFlag> = Vec::new();
		for extension_handle in self.shared.extensions.lock().unwrap().iter() {
			let extension = extension_handle.lock().unwrap();
			for (name, flag) in &extension.flags {
				if !names.contains(name) {
					names.push(name.clone());
					flags.push(flag.clone());
				}
			}
		}
		flags
	}

	/// `setFlagValue(name, value)`.
	pub fn set_flag_value(&self, name: &str, value: Value) {
		self.shared.runtime.set_flag_value(name, value);
	}

	/// `getFlagValues()`.
	pub fn get_flag_values(&self) -> Vec<(String, Value)> {
		self.shared.runtime.flag_values()
	}

	/// `resolveRegisteredCommands()` — duplicate names get
	/// `name:occurrence` invocation names.
	fn resolve_registered_commands(&self) -> Vec<(String, String, Option<String>, Value)> {
		let mut commands: Vec<(String, Option<String>, Value)> = Vec::new();
		let mut counts: Vec<(String, usize)> = Vec::new();
		for extension_handle in self.shared.extensions.lock().unwrap().iter() {
			let extension = extension_handle.lock().unwrap();
			for (name, command) in &extension.commands {
				commands.push((command.name.clone(), command.description.clone(), command.source_info.clone()));
				match counts.iter_mut().find(|(n, _)| n == name) {
					Some((_, count)) => *count += 1,
					None => counts.push((command.name.clone(), 1)),
				}
			}
		}
		let mut seen: Vec<(String, usize)> = Vec::new();
		let mut taken: Vec<String> = Vec::new();
		commands
			.into_iter()
			.map(|(name, description, source_info)| {
				let occurrence = match seen.iter_mut().find(|(n, _)| n == &name) {
					Some((_, count)) => {
						*count += 1;
						*count
					}
					None => {
						seen.push((name.clone(), 1));
						1
					}
				};
				let total = counts.iter().find(|(n, _)| n == &name).map(|(_, c)| *c).unwrap_or(0);
				let mut invocation_name =
					if total > 1 { format!("{}:{}", name, occurrence) } else { name.clone() };
				if taken.contains(&invocation_name) {
					let mut suffix = occurrence;
					loop {
						suffix += 1;
						invocation_name = format!("{}:{}", name, suffix);
						if !taken.contains(&invocation_name) {
							break;
						}
					}
				}
				taken.push(invocation_name.clone());
				(name, invocation_name, description, source_info)
			})
			.collect()
	}

	/// `getRegisteredCommands()`.
	pub fn get_registered_commands(&self) -> Vec<(String, String, Option<String>, Value)> {
		self.resolve_registered_commands()
	}

	/// `getCommand(name)` — matched by invocation name.
	pub fn get_command(&self, name: &str) -> Option<(String, String, Option<String>, Value)> {
		self.resolve_registered_commands().into_iter().find(|(_, invocation, _, _)| invocation == name)
	}

	/// `getShortcuts(resolvedKeybindings)` — reserved-keybinding conflict
	/// logic; diagnostics recorded for `getShortcutDiagnostics()`.
	pub fn get_shortcuts(&self, resolved_keybindings: &Value) -> Vec<(String, ExtensionShortcut)> {
		*self.shared.shortcut_diagnostics.lock().unwrap() = Vec::new();
		// buildBuiltinKeybindings
		let mut builtin: Vec<(String, String, bool)> = Vec::new();
		if let Some(object) = resolved_keybindings.as_object() {
			for (keybinding, keys) in object {
				let key_list: Vec<Value> = match keys {
					Value::Array(list) => list.clone(),
					value => vec![value.clone()],
				};
				let restrict_override = RESERVED_KEYBINDINGS.contains(&keybinding.as_str());
				for key in key_list {
					let Some(key) = key.as_str() else { continue };
					let normalized_key = key.to_lowercase();
					if let Some((_, _, existing_restrict)) =
						builtin.iter_mut().find(|(k, _, _)| *k == normalized_key)
					{
						if *existing_restrict && !restrict_override {
							continue;
						}
						*existing_restrict = restrict_override;
					} else {
						builtin.push((normalized_key, keybinding.clone(), restrict_override));
					}
				}
			}
		}
		let mut shortcuts: Vec<(String, ExtensionShortcut)> = Vec::new();
		for extension_handle in self.shared.extensions.lock().unwrap().iter() {
			let extension = extension_handle.lock().unwrap();
			for (key, shortcut) in &extension.shortcuts {
				let normalized_key = key.to_lowercase();
				let builtin_entry = builtin.iter().find(|(k, _, _)| *k == normalized_key);
				if let Some((_, _keybinding, true)) = builtin_entry {
					self.push_shortcut_diagnostic(
						format!(
							"Extension shortcut '{}' from {} conflicts with built-in shortcut. Skipping.",
							key, shortcut.extension_path
						),
						&shortcut.extension_path,
					);
					continue;
				}
				if let Some((_, keybinding, false)) = builtin_entry {
					self.push_shortcut_diagnostic(
						format!(
							"Extension shortcut conflict: '{}' is built-in shortcut for {} and {}. Using {}.",
							key, keybinding, shortcut.extension_path, shortcut.extension_path
						),
						&shortcut.extension_path,
					);
				}
				if let Some((_, existing)) = shortcuts.iter().find(|(k, _)| *k == normalized_key) {
					self.push_shortcut_diagnostic(
						format!(
							"Extension shortcut conflict: '{}' registered by both {} and {}. Using {}.",
							key, existing.extension_path, shortcut.extension_path, shortcut.extension_path
						),
						&shortcut.extension_path,
					);
				}
				match shortcuts.iter_mut().find(|(k, _)| *k == normalized_key) {
					Some(slot) => slot.1 = shortcut.clone(),
					None => shortcuts.push((normalized_key, shortcut.clone())),
				}
			}
		}
		shortcuts
	}

	fn push_shortcut_diagnostic(&self, message: String, path: &str) {
		self.shared
			.shortcut_diagnostics
			.lock()
			.unwrap()
			.push(json!({ "type": "warning", "message": message, "path": path }));
	}

	/// `getShortcutDiagnostics()`.
	pub fn get_shortcut_diagnostics(&self) -> Vec<Value> {
		self.shared.shortcut_diagnostics.lock().unwrap().clone()
	}

	/// `invalidate(message?)` — first call wins.
	pub fn invalidate(&self, message: Option<&str>) {
		let mut stale = self.shared.stale_message.lock().unwrap();
		if stale.is_some() {
			return;
		}
		*stale = Some(message.unwrap_or(DEFAULT_STALE_MESSAGE).to_string());
		drop(stale);
		self.shared.runtime.invalidate(message);
	}

	/// `getActiveTools()` — `Err` is the stale/unbound throw.
	pub fn get_active_tools(&self) -> Result<Vec<String>, String> {
		if let Some(message) = self.shared.stale_message.lock().unwrap().clone() {
			return Err(message);
		}
		self.shared.runtime.get_active_tools()
	}


	/// `shutdown()`.
	pub fn shutdown(&self) {
		if let Some(actions) = self.shared.context_actions.lock().unwrap().clone() {
			actions.shutdown();
		}
	}
}

/// TS `emitSessionShutdownEvent(extensionRunner, event)`.
pub async fn emit_session_shutdown_event(runner: &ExtensionRunner, event: Value) -> bool {
	if runner.has_handlers("session_shutdown") {
		runner.emit(event).await;
		return true;
	}
	false
}

/// TS `emitProjectTrustEvent(extensionsResult, event, ctx)` — the first
/// non-`undecided` handler wins.
pub async fn emit_project_trust_event(
	runner: &ExtensionRunner,
	event: Value,
	ctx: Value,
) -> (Option<Value>, Vec<Value>) {
	let mut errors: Vec<Value> = Vec::new();
	let event_type = event.get("type").and_then(Value::as_str).unwrap_or("").to_string();
	let ctx = CtxView::Raw(ctx);
	for (extension_path, handlers) in runner.snapshot_handlers("project_trust") {
		for (_, handler) in handlers {
			match handler(event.clone(), ctx.clone()).await {
				Ok(HandlerOutcome { result: handler_result, .. }) => {
					// TS reads `handlerResult.trusted` inside the try: an
					// undefined/null result throws a TypeError that is
					// recorded as a handler error.
					let undecided = match handler_result.as_ref() {
						Some(result) if !result.is_null() => {
							result.get("trusted").and_then(Value::as_str) == Some("undecided")
						}
						_ => {
							errors.push(json!({
								"extensionPath": extension_path,
								"event": event_type,
								"error": "Cannot read properties of undefined (reading 'trusted')",
							}));
							continue;
						}
					};
					if undecided {
						continue;
					}
					return (handler_result, errors);
				}
				Err(error) => {
					errors.push(json!({
						"extensionPath": extension_path,
						"event": event_type,
						"error": error,
					}));
				}
			}
		}
	}
	(None, errors)
}

/// `ExtensionToolContext` — `createContext()` + `tools`/`executeTool()`.
#[derive(Clone)]
pub struct ToolContextView {
	runner: ExtensionRunner,
	tool_call_id: String,
	/// The tool call's default signal (an opaque value).
	signal: Value,
}

impl ToolContextView {
	/// `ctx.tools` (guarded getter; absent → `[]`).
	pub fn tools(&self) -> Result<Vec<Value>, String> {
		self.runner.assert_active_shared()?;
		Ok(self
			.runner
			.shared
			.context_actions
			.lock()
			.unwrap()
			.as_ref()
			.and_then(|actions| actions.get_callable_tools())
			.unwrap_or_default())
	}

	/// `ctx.executeTool(name, args, options?)` — the unbound fallback is
	/// the TS canned "Nested tool calls are not available in this context"
	/// result; the bound path merges `signal: options.signal ?? signal`.
	pub async fn execute_tool(
		&self,
		name: &str,
		args: Value,
		options: Value,
	) -> Result<Value, String> {
		self.runner.assert_active_shared()?;
		let action = self
			.runner
			.shared
			.context_actions
			.lock()
			.unwrap()
			.clone();
		let Some(actions) = action else {
			return Ok(nested_call_fallback(&self.tool_call_id, name));
		};
		let mut merged = options.clone();
		if merged.get("signal").is_none() || merged.get("signal") == Some(&Value::Null) {
			if let Some(object) = merged.as_object_mut() {
				if !self.signal.is_null() {
					object.insert("signal".to_string(), self.signal.clone());
				}
			}
		}
		match actions.execute_tool(&self.tool_call_id, name, args, merged) {
			Some(result) => result,
			None => Ok(nested_call_fallback(&self.tool_call_id, name)),
		}
	}
}

/// The exact `executeTool()` fallback object.
pub fn nested_call_fallback(tool_call_id: &str, name: &str) -> Value {
	json!({
		"toolCall": {"type": "toolCall", "id": format!("{}/0", tool_call_id), "name": name, "arguments": {}},
		"result": {
			"content": [{"type": "text", "text": "Nested tool calls are not available in this context"}],
			"details": {}
		},
		"isError": true
	})
}

/// `ExtensionCommandContext` — the command-only members bound through
/// `bindCommandContext`; `None` yields the TS default handlers.
#[derive(Clone)]
pub struct CommandContextView {
	runner: ExtensionRunner,
}

impl CommandContextView {
	fn actions(&self) -> Option<Arc<dyn CommandContextActions>> {
		self.runner.shared.command_actions.lock().unwrap().clone()
	}

	fn assert_active(&self) -> Result<(), String> {
		self.runner.assert_active_shared()
	}

	/// `ctx.getSystemPromptOptions()` (command contexts only).
	pub fn get_system_prompt_options(&self) -> Result<Value, String> {
		self.assert_active()?;
		Ok(self
			.runner
			.shared
			.context_actions
			.lock()
			.unwrap()
			.as_ref()
			.and_then(|actions| actions.get_system_prompt_options())
			.unwrap_or_else(|| {
				normalize_options_raw(&json!({"cwd": self.runner.shared.cwd}))
					.unwrap_or_else(|_| json!({}))
			}))
	}

	/// `ctx.waitForIdle()`.
	pub async fn wait_for_idle(&self) -> Result<(), String> {
		self.assert_active()?;
		match self.actions() {
			Some(actions) => {
				actions.wait_for_idle().await;
				Ok(())
			}
			None => Ok(()),
		}
	}

	/// `ctx.newSession(options?)` → `{ cancelled: boolean }`.
	pub async fn new_session(&self, options: Value) -> Result<Value, String> {
		self.assert_active()?;
		match self.actions() {
			Some(actions) => Ok(actions.new_session(options).await),
			None => Ok(json!({"cancelled": false})),
		}
	}

	/// `ctx.fork(entryId, options?)`.
	pub async fn fork(&self, entry_id: &str, options: Value) -> Result<Value, String> {
		self.assert_active()?;
		match self.actions() {
			Some(actions) => Ok(actions.fork(entry_id.to_string(), options).await),
			None => Ok(json!({"cancelled": false})),
		}
	}

	/// `ctx.navigateTree(targetId, options?)`.
	pub async fn navigate_tree(&self, target_id: &str, options: Value) -> Result<Value, String> {
		self.assert_active()?;
		match self.actions() {
			Some(actions) => Ok(actions.navigate_tree(target_id.to_string(), options).await),
			None => Ok(json!({"cancelled": false})),
		}
	}

	/// `ctx.switchSession(sessionPath, options?)`.
	pub async fn switch_session(&self, session_path: &str, options: Value) -> Result<Value, String> {
		self.assert_active()?;
		match self.actions() {
			Some(actions) => Ok(actions.switch_session(session_path.to_string(), options).await),
			None => Ok(json!({"cancelled": false})),
		}
	}

	/// `ctx.reload()`.
	pub async fn reload(&self) -> Result<(), String> {
		self.assert_active()?;
		if let Some(actions) = self.actions() {
			actions.reload().await;
		}
		Ok(())
	}
}

// ============================================================================
// ExtensionHost bridge — the real runner behind the agent-session seam
// ============================================================================

impl ExtensionHost for ExtensionRunner {
	fn has_handlers(&self, event: &str) -> bool {
		ExtensionRunner::has_handlers(self, event)
	}
	fn emit_session_before_switch<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>> {
		Box::pin(async move { self.emit(event).await })
	}
	fn emit_session_before_fork<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>> {
		Box::pin(async move { self.emit(event).await })
	}
	fn emit<'a>(&'a self, event: Value) -> BoxFuture<'a, ()> {
		Box::pin(async move {
			self.emit(event).await;
		})
	}
	fn emit_message_end<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>> {
		Box::pin(async move { self.emit_message_end(event).await })
	}
	fn emit_error(&self, error: Value) {
		ExtensionRunner::emit_error(self, &error);
	}
	fn invalidate(&self, message: &str) {
		ExtensionRunner::invalidate(self, Some(message));
	}
	fn get_command(&self, name: &str) -> bool {
		ExtensionRunner::get_command(self, name).is_some()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn flags_commit_and_discard() {
		let runtime = ExtensionRuntime::new();
		let ok = Arc::new(|_pi: &ExtensionApi| Box::pin(async { Ok(()) }) as BoxFuture<'static, Result<(), String>>);
		let extension =
			load_extension_from_factory(ok, "/proj", None, &runtime, "<inline:a>").await.unwrap();
		assert_eq!(extension.lock().unwrap().path, "<inline:a>");

		let spec = Arc::new(|pi: &ExtensionApi| {
			let pi = pi.clone();
			Box::pin(async move {
				pi.register_flag("verbose", &json!({"type": "boolean", "default": true})).unwrap();
				Err("boom".to_string())
			}) as BoxFuture<'static, Result<(), String>>
		});
		let failed = load_extension_from_factory(spec, "/proj", None, &runtime, "<inline:b>").await;
		assert_eq!(failed.err(), Some("boom".to_string()));
		assert!(runtime.flag_values().is_empty());
	}

	async fn runner_with_handlers(
		event: &'static str,
		handlers: Vec<ExtensionHandler>,
	) -> ExtensionRunner {
		let runtime = ExtensionRuntime::new();
		let factory: FactoryFn = Arc::new(move |pi: &ExtensionApi| {
			let pi = pi.clone();
			let handlers: Vec<ExtensionHandler> = handlers.clone();
			Box::pin(async move {
				for handler in handlers {
					pi.on(event, handler).unwrap();
				}
				Ok(())
			})
		});
		let extension = load_extension_from_factory(factory, "/proj", None, &runtime, "<inline>")
			.await
			.unwrap();
		ExtensionRunner::new(vec![extension], runtime, "/proj", None)
	}

	#[tokio::test]
	async fn before_agent_start_merges_message_and_force() {
		let handler: ExtensionHandler = Arc::new(|_event, _ctx| {
			Box::pin(async {
				Ok(HandlerOutcome {
					event: json!({}),
					result: Some(json!({
						"message": {"role": "user", "content": [{"type": "text", "text": "m"}]},
						"systemPrompt": "X",
					})),
				})
			})
		});
		let runner = runner_with_handlers("before_agent_start", vec![handler]).await;
		let (messages, options) = runner
			.emit_before_agent_start(json!({"prompt": "p"}), json!({"cwd": "/p", "customPrompt": "CP"}))
			.await
			.unwrap();
		assert_eq!(messages, vec![json!({"role": "user", "content": [{"type": "text", "text": "m"}]})]);
		assert_eq!(options["forceSystemPrompt"], json!("X"));
		assert_eq!(options["customPrompt"], json!("CP"));
	}

	#[tokio::test]
	async fn before_agent_start_null_reassigns_and_truthy_gate() {
		// `result.systemPrompt !== undefined` → null RESETS; a falsey result
		// and a falsey message are skipped.
		let handler: ExtensionHandler = Arc::new(|_event, _ctx| {
			Box::pin(async {
				Ok(HandlerOutcome {
					event: json!({}),
					result: Some(json!({"systemPrompt": null, "message": null})),
				})
			})
		});
		let runner = runner_with_handlers("before_agent_start", vec![handler]).await;
		let (messages, options) =
			runner.emit_before_agent_start(json!({}), json!({"cwd": "/p"})).await.unwrap();
		assert!(messages.is_empty());
		assert_eq!(options["forceSystemPrompt"], Value::Null);
		assert!(options.as_object().unwrap().contains_key("forceSystemPrompt"));
	}

	#[tokio::test]
	async fn before_agent_start_live_options_object() {
		// TS: `event.systemPromptOptions` is the live `currentOptions` ref.
		let handler: ExtensionHandler = Arc::new(|mut event, _ctx| {
			Box::pin(async move {
				event["systemPromptOptions"]["customPrompt"] = json!("NEW");
				Ok(HandlerOutcome { event, result: None })
			})
		});
		let runner = runner_with_handlers("before_agent_start", vec![handler]).await;
		let (_, options) =
			runner.emit_before_agent_start(json!({}), json!({"cwd": "/p"})).await.unwrap();
		assert_eq!(options["customPrompt"], json!("NEW"));
	}

	#[tokio::test]
	async fn before_agent_start_normalize_throws_out() {
		let runner = runner_with_handlers("before_agent_start", vec![]).await;
		let err = runner
			.emit_before_agent_start(json!({}), json!({"selectedTools": 5}))
			.await
			.unwrap_err();
		assert_eq!(
			err,
			"(input.selectedTools ?? [\"read\",\"bash\",\"edit\",\"write\"]) is not iterable"
		);
	}

	#[tokio::test]
	async fn before_agent_start_render_surfaces_and_handler_errors() {
		// forceSystemPrompt: null → both the event getter and ctx render throw;
		// the handler observes the ctx error, and its own throw is reported.
		let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
		let seen_handler = Arc::clone(&seen);
		let handler: ExtensionHandler = Arc::new(move |event, ctx| {
			let seen = Arc::clone(&seen_handler);
			Box::pin(async move {
				seen.lock().unwrap().push(format!("event:{}", event.get("systemPrompt").is_some()));
				match ctx.call("getSystemPrompt") {
					Ok(value) => seen.lock().unwrap().push(format!("call:{}", value)),
					Err(message) => seen.lock().unwrap().push(format!("call!{}", message)),
				}
				Err("boom".to_string())
			})
		});
		let runner = runner_with_handlers("before_agent_start", vec![handler]).await;
		let errors: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
		let errors_listener = Arc::clone(&errors);
		runner.on_error(Arc::new(move |error| {
			errors_listener.lock().unwrap().push(error.clone());
		}));
		let (messages, options) = runner
			.emit_before_agent_start(json!({}), json!({"cwd": "/p", "forceSystemPrompt": null}))
			.await
			.unwrap();
		assert!(messages.is_empty());
		assert_eq!(options["forceSystemPrompt"], Value::Null);
		// event.systemPrompt omitted (render error) — TS throws at read.
		let seen = seen.lock().unwrap().clone();
		assert_eq!(seen[0], "event:false");
		assert_eq!(seen[1], "call!Cannot read properties of null (reading 'filter')");
		let errors = errors.lock().unwrap().clone();
		assert_eq!(errors.len(), 1);
		assert_eq!(errors[0]["event"], json!("before_agent_start"));
		assert_eq!(errors[0]["error"], json!("boom"));
	}

	#[tokio::test]
	async fn before_agent_start_event_and_ctx_render() {
		let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
		let seen_handler = Arc::clone(&seen);
		let handler: ExtensionHandler = Arc::new(move |event, ctx| {
			let seen = Arc::clone(&seen_handler);
			Box::pin(async move {
				let prompt = event["systemPrompt"].as_str().unwrap_or("<none>").to_string();
				seen.lock().unwrap().push(format!("event:{prompt}"));
				let via_ctx = ctx.call("getSystemPrompt").unwrap();
				seen.lock().unwrap().push(format!("ctx:{}", via_ctx.as_str().unwrap_or("")));
				Ok(HandlerOutcome { event, result: None })
			})
		});
		let runner = runner_with_handlers("before_agent_start", vec![handler]).await;
		runner.set_doc_paths(DocPaths {
			readme: "R".to_string(),
			docs: "D".to_string(),
			examples: "E".to_string(),
		});
		let (_, options) = runner
			.emit_before_agent_start(json!({"prompt": "p"}), json!({"cwd": "/p"}))
			.await
			.unwrap();
		assert!(options["customPrompt"].is_null());
		let seen = seen.lock().unwrap().clone();
		// default cluster renders (no customPrompt) with the injected paths.
		assert!(seen[0].starts_with("event:You are an expert coding assistant"));
		assert!(seen[0].contains("- Main documentation: R"), "{}", seen[0]);
		assert!(seen[0].contains("<cwd>\n/p\n</cwd>"));
		assert_eq!(seen[1], format!("ctx:{}", &seen[0]["event:".len()..]));
	}

	#[tokio::test]
	async fn api_actions_pre_bind_and_exec() {
		let runtime = ExtensionRuntime::new();
		let captured: Arc<Mutex<Option<ExtensionApi>>> = Arc::new(Mutex::new(None));
		let captured_factory = Arc::clone(&captured);
		let results: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
		let results_factory = Arc::clone(&results);
		let factory: FactoryFn = Arc::new(move |pi: &ExtensionApi| {
			*captured_factory.lock().unwrap() = Some(pi.clone());
			let results = Arc::clone(&results_factory);
			let pi = pi.clone();
			Box::pin(async move {
				// pre-bind: assertActive passes (loading ≠ failed), the runtime
				// stub throws.
				match pi.send_message(json!({"customType": "t"}), json!({})) {
					Ok(()) => results.lock().unwrap().push("ok".to_string()),
					Err(message) => results.lock().unwrap().push(format!("!{message}")),
				}
				// pi.exec runs during loading with the loader cwd.
				let exec = pi.exec(json!("echo"), json!(["hi"]), json!({})).await.unwrap();
				results.lock().unwrap().push(format!("exec:{}:{}", exec.stdout, exec.code));
				Ok(())
			})
		});
		let _extension =
			load_extension_from_factory(factory, "/", None, &runtime, "<inline>").await.unwrap();
		let results = results.lock().unwrap().clone();
		assert_eq!(results[0], format!("!{NOT_INITIALIZED}"));
		assert_eq!(results[1], "exec:hi\n:0");
		// A stale runtime fails assertActive with the stale message.
		let api = captured.lock().unwrap().clone().unwrap();
		runtime.invalidate(None);
		assert_eq!(api.send_message(json!({}), json!({})).unwrap_err(), DEFAULT_STALE_MESSAGE);
	}

	fn handler_returning(result: Value) -> ExtensionHandler {
		Arc::new(move |event, _ctx| {
			let result = result.clone();
			Box::pin(async move { Ok(HandlerOutcome { event, result: Some(result) }) })
		})
	}

	fn handler_undefined() -> ExtensionHandler {
		Arc::new(|event, _ctx| Box::pin(async move { Ok(HandlerOutcome { event, result: None }) }))
	}

	#[tokio::test]
	async fn emit_cache_warming_decision_overrides() {
		let runner = runner_with_handlers(
			"cache_warming_decision",
			vec![handler_undefined(), handler_returning(json!({"action": "skip"}))],
		)
		.await;
		let event = json!({"type": "cache_warming_decision", "action": "warm"});
		assert_eq!(runner.emit_cache_warming_decision(event).await, json!("skip"));

		// present null overrides; a result without the key keeps the action.
		let runner = runner_with_handlers(
			"cache_warming_decision",
			vec![handler_returning(json!({})), handler_returning(json!({"action": null}))],
		)
		.await;
		let event = json!({"type": "cache_warming_decision", "action": "warm"});
		assert_eq!(runner.emit_cache_warming_decision(event).await, Value::Null);
	}

	#[tokio::test]
	async fn emit_tool_result_patches_and_deletes_structured() {
		let runner = runner_with_handlers(
			"tool_result",
			vec![handler_returning(json!({"content": [{"type": "text", "text": "new"}]}))],
		)
		.await;
		let event = json!({
			"type": "tool_result",
			"toolCallId": "c1",
			"content": [{"type": "text", "text": "old"}],
			"details": {"d": 1},
			"structuredContent": {"s": 1},
			"isError": false,
			"usage": {"tokens": 1},
		});
		let result = runner.emit_tool_result(event).await.unwrap().unwrap();
		// content replaced; structuredContent deleted (not replaced together);
		// the return carries only present fields, in TS key order.
		assert_eq!(
			result,
			json!({
				"content": [{"type": "text", "text": "new"}],
				"details": {"d": 1},
				"isError": false,
				"usage": {"tokens": 1},
			})
		);
		assert_eq!(
			result.as_object().unwrap().keys().cloned().collect::<Vec<_>>(),
			vec!["content", "details", "isError", "usage"]
		);

		// no modifications → None; falsy results are ignored.
		let runner = runner_with_handlers(
			"tool_result",
			vec![handler_returning(Value::Null), handler_undefined()],
		)
		.await;
		let event = json!({"type": "tool_result", "content": [], "details": {}});
		assert_eq!(runner.emit_tool_result(event).await.unwrap(), None);

		// replacing structuredContent explicitly keeps the key.
		let runner = runner_with_handlers(
			"tool_result",
			vec![handler_returning(json!({"content": [], "structuredContent": {"s": 2}}))],
		)
		.await;
		let event = json!({"type": "tool_result", "content": [], "structuredContent": {"s": 1}});
		let result = runner.emit_tool_result(event).await.unwrap().unwrap();
		assert_eq!(result["structuredContent"], json!({"s": 2}));
	}

	#[tokio::test]
	async fn emit_tool_call_truthy_block_and_rethrow() {
		let runner = runner_with_handlers(
			"tool_call",
			vec![
				handler_returning(json!({"block": false, "reason": "a"})),
				handler_returning(json!({"block": true, "reason": "b"})),
				handler_returning(json!({"block": false, "reason": "c"})),
			],
		)
		.await;
		let event = json!({"type": "tool_call", "toolName": "read"});
		assert_eq!(
			runner.emit_tool_call(event).await.unwrap(),
			Some(json!({"block": true, "reason": "b"}))
		);
		// last truthy wins when nothing blocks.
		let runner = runner_with_handlers(
			"tool_call",
			vec![
				handler_returning(json!({"reason": "a"})),
				handler_returning(json!({"reason": "b"})),
			],
		)
		.await;
		let event = json!({"type": "tool_call"});
		assert_eq!(
			runner.emit_tool_call(event).await.unwrap(),
			Some(json!({"reason": "b"}))
		);
		// throws propagate with NO emitError.
		let thrower: ExtensionHandler =
			Arc::new(|_event, _ctx| Box::pin(async { Err("tc boom".to_string()) }));
		let runner = runner_with_handlers("tool_call", vec![thrower]).await;
		let errors: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
		let listener = Arc::clone(&errors);
		runner.on_error(Arc::new(move |error| listener.lock().unwrap().push(error.clone())));
		assert_eq!(runner.emit_tool_call(json!({"type": "tool_call"})).await.unwrap_err(), "tc boom");
		assert!(errors.lock().unwrap().is_empty());
	}

	#[tokio::test]
	async fn emit_provider_hooks_and_resources() {
		let runner = runner_with_handlers(
			"before_provider_request",
			vec![handler_returning(Value::Null), handler_returning(json!({"x": 1}))],
		)
		.await;
		assert_eq!(
			runner.emit_before_provider_request(json!({"p": 0})).await,
			json!({"x": 1})
		);

		// headers: in-place mutation read back, return value ignored.
		let mutator: ExtensionHandler = Arc::new(|mut event, _ctx| {
			Box::pin(async move {
				event["headers"]["authorization"] = json!("token");
				Ok(HandlerOutcome { event, result: Some(json!({"ignored": true})) })
			})
		});
		let runner = runner_with_handlers("before_provider_headers", vec![mutator]).await;
		assert_eq!(
			runner.emit_before_provider_headers(json!({"accept": "a"})).await,
			json!({"accept": "a", "authorization": "token"})
		);

		// resources_discover: non-empty lists tagged with the extension path
		// (`<inline>` here); empty lists contribute nothing.
		let runner = runner_with_handlers(
			"resources_discover",
			vec![
				handler_returning(json!({"skillPaths": ["/s1"], "promptPaths": [], "themePaths": ["/t1"]})),
				handler_returning(json!({"skillPaths": ["/s2"]})),
			],
		)
		.await;
		assert_eq!(
			runner.emit_resources_discover("/proj", json!("startup")).await,
			json!({
				"skillPaths": [
					{"path": "/s1", "extensionPath": "<inline>"},
					{"path": "/s2", "extensionPath": "<inline>"}
				],
				"promptPaths": [],
				"themePaths": [{"path": "/t1", "extensionPath": "<inline>"}]
			})
		);
	}

	#[tokio::test]
	async fn emit_input_transform_chain_and_handled() {
		let transform: ExtensionHandler = Arc::new(|event, _ctx| {
			Box::pin(async move {
				let text = event["text"].as_str().unwrap_or("").to_string();
				Ok(HandlerOutcome {
					event,
					result: Some(json!({"action": "transform", "text": format!("{text}!")})),
				})
			})
		});
		let runner = runner_with_handlers("input", vec![transform]).await;
		assert_eq!(
			runner.emit_input("hi", None, json!("user"), None).await,
			json!({"action": "transform", "text": "hi!"})
		);
		// nothing changed → continue.
		let runner = runner_with_handlers("input", vec![handler_undefined()]).await;
		assert_eq!(
			runner.emit_input("hi", Some(json!([{"type": "image"}])), json!("user"), None).await,
			json!({"action": "continue"})
		);
		// handled short-circuits and returns the handler result as-is.
		let runner = runner_with_handlers(
			"input",
			vec![handler_returning(json!({"action": "handled", "text": "x"}))],
		)
		.await;
		assert_eq!(
			runner.emit_input("hi", None, json!("user"), None).await,
			json!({"action": "handled", "text": "x"})
		);
	}

	#[tokio::test]
	async fn emit_boundary_entries_context_validity() {
		// buildContext echoes the entries it receives.
		let build: BoundaryBuildContext = Arc::new(|entries: Vec<Value>| {
			Box::pin(async move { Ok(json!({"count": entries.len()})) })
		});
		let runner = runner_with_handlers(
			"turn_end",
			vec![
				handler_returning(json!({"entries": [{"k": 1}], "continue": true})),
				handler_returning(json!({"entries": [{"k": 1}, {"k": 2}]})),
			],
		)
		.await;
		let result = runner
			.emit_boundary(json!({"type": "turn_end", "turn": 3}), build)
			.await
			.unwrap();
		assert_eq!(
			result,
			json!({
				"entries": [{"k": 1}, {"k": 2}],
				"continue": true,
				"context": {"count": 2},
				"valid": true
			})
		);
		assert_eq!(
			result.as_object().unwrap().keys().cloned().collect::<Vec<_>>(),
			vec!["entries", "continue", "context", "valid"]
		);

		// a failing rebuild marks invalid; the error is reported per handler.
		let build: BoundaryBuildContext = Arc::new(|entries: Vec<Value>| {
			Box::pin(async move {
				if entries.len() > 1 {
					Err("bad draft".to_string())
				} else {
					Ok(json!({"count": entries.len()}))
				}
			})
		});
		let runner = runner_with_handlers(
			"turn_end",
			vec![handler_returning(json!({"entries": [{"k": 1}, {"k": 2}]}))],
		)
		.await;
		let errors: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
		let listener = Arc::clone(&errors);
		runner.on_error(Arc::new(move |error| listener.lock().unwrap().push(error.clone())));
		let result = runner.emit_boundary(json!({"type": "turn_end"}), build).await.unwrap();
		assert_eq!(result, json!({"entries": [], "continue": false, "context": {"count": 0}, "valid": false}));
		let errors = errors.lock().unwrap().clone();
		assert_eq!(errors[0]["error"], json!("Invalid boundary entries: bad draft"));

		// the initial buildContext throw escapes (no catch).
		let build: BoundaryBuildContext =
			Arc::new(|_entries| Box::pin(async { Err("init boom".to_string()) }));
		let runner = runner_with_handlers("turn_end", vec![]).await;
		assert_eq!(
			runner.emit_boundary(json!({"type": "turn_end"}), build).await.unwrap_err(),
			"init boom"
		);
	}

	#[tokio::test]
	async fn emit_context_restores_system_head() {
		let original = json!([
			{"role": "system", "content": "HEAD", "timestamp": 7},
			{"role": "user", "content": [{"type": "text", "text": "hi"}]}
		]);
		// unchanged (undefined result) keeps the transcript as-is.
		let runner = runner_with_handlers("context", vec![handler_undefined()]).await;
		assert_eq!(runner.emit_context(original.clone()).await.unwrap(), original);

		// a replaced list gets the replayed head prepended.
		let runner = runner_with_handlers(
			"context",
			vec![handler_returning(json!({"messages": [{"role": "user", "content": "new"}]}))],
		)
		.await;
		assert_eq!(
			runner.emit_context(original.clone()).await.unwrap(),
			json!([
				{"role": "system", "content": "HEAD", "timestamp": 7},
				{"role": "user", "content": "new"}
			])
		);

		// an in-place edit (no return) is detected against the snapshot.
		let pusher: ExtensionHandler = Arc::new(|mut event, _ctx| {
			Box::pin(async move {
				if let Some(messages) = event["messages"].as_array_mut() {
					messages.push(json!({"role": "assistant", "content": "added"}));
				}
				Ok(HandlerOutcome { event, result: None })
			})
		});
		let runner = runner_with_handlers("context", vec![pusher]).await;
		assert_eq!(
			runner.emit_context(original).await.unwrap(),
			json!([
				{"role": "system", "content": "HEAD", "timestamp": 7},
				{"role": "user", "content": [{"type": "text", "text": "hi"}]},
				{"role": "assistant", "content": "added"}
			])
		);
	}

	#[test]
	fn current_system_message_replays_parts() {
		// contentText joins multi-block content with "\n"; sections patch by
		// name (null deletes); tools resolve by name in order.
		let messages = json!([
			{"role": "system", "content": [{"type": "text", "text": "A"}, {"type": "text", "text": "B"}], "timestamp": 1},
			{"role": "user", "content": "u"},
			{"role": "system", "content": "C", "sections": {"x": "1", "y": "2"}, "toolsAdded": [{"name": "read", "v": 1}]},
			{"role": "system", "content": "", "sections": {"x": null}, "toolsAdded": [{"name": "bash", "v": 2}], "toolsRemoved": [{"name": "read"}]}
		]);
		let head = get_current_system_message(messages.as_array().unwrap()).unwrap().unwrap();
		assert_eq!(
			head,
			json!({
				"role": "system",
				"content": "A\nB\n\nC",
				"sections": {"y": "2"},
				"toolsAdded": [{"name": "bash", "v": 2}],
				"timestamp": 1
			})
		);
		assert_eq!(
			head.as_object().unwrap().keys().cloned().collect::<Vec<_>>(),
			vec!["role", "content", "sections", "toolsAdded", "timestamp"]
		);
		// no timestamps and no tools → undefined, even with content.
		let none = get_current_system_message(&[json!({"role": "system", "content": "x"})])
			.unwrap();
		assert_eq!(none, None);
		// tools alone suffice.
		let tools_only = get_current_system_message(&[json!({
			"role": "system",
			"content": "",
			"toolsAdded": [{"name": "t"}]
		})])
		.unwrap()
		.unwrap();
		assert_eq!(tools_only["timestamp"], json!(0));
	}

	#[tokio::test]
	async fn emit_context_with_system_reports_lost_head() {
		let original = json!([
			{"role": "system", "content": "S", "timestamp": 1},
			{"role": "user", "content": "u"}
		]);
		let runner = runner_with_handlers(
			"context_with_system",
			vec![handler_returning(json!({"messages": [{"role": "user", "content": "only"}]}))],
		)
		.await;
		let errors: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
		let listener = Arc::clone(&errors);
		runner.on_error(Arc::new(move |error| listener.lock().unwrap().push(error.clone())));
		assert_eq!(
			runner.emit_context(original).await.unwrap(),
			json!([{"role": "user", "content": "only"}])
		);
		let errors = errors.lock().unwrap().clone();
		assert_eq!(errors.len(), 1);
		assert!(errors[0]["error"].as_str().unwrap().starts_with("Handler removed the leading system message;"));
		// keeping the head is silent.
		let runner = runner_with_handlers("context_with_system", vec![handler_undefined()]).await;
		assert!(runner
			.emit_context(json!([{"role": "system", "content": "S", "timestamp": 1}]))
			.await
			.is_ok());
	}

	#[test]
	fn user_bash_result_validator() {
		let valid_result = json!({
			"result": {"output": "o", "exitCode": 0, "cancelled": false, "truncated": false}
		});
		assert!(is_user_bash_event_result(&valid_result));
		// exitCode present-but-undefined (Null in the jstr model) is allowed.
		assert!(is_user_bash_event_result(&json!({
			"result": {"output": "o", "exitCode": null, "cancelled": true, "truncated": false}
		})));
		assert!(is_user_bash_event_result(&json!({
			"result": {"output": "o", "exitCode": 0, "cancelled": false, "truncated": false, "fullOutputPath": "/f"}
		})));
		assert!(is_user_bash_event_result(&json!({"operations": {"exec": "fn"}})));
		// invalid: both / neither
		assert!(!is_user_bash_event_result(&json!({})));
		assert!(!is_user_bash_event_result(&json!({"operations": {"exec": "fn"}, "result": {}})));
		assert!(!is_user_bash_event_result(&json!(5)));
		assert!(!is_user_bash_event_result(&Value::Null));
		// invalid operations/result shapes
		assert!(!is_user_bash_event_result(&json!({"operations": {}})));
		assert!(!is_user_bash_event_result(&json!({"operations": 5})));
		assert!(!is_user_bash_event_result(&json!({"result": {"output": 5, "exitCode": 0, "cancelled": false, "truncated": false}})));
		assert!(!is_user_bash_event_result(&json!({"result": {"exitCode": 0, "cancelled": false, "truncated": false}})));
		assert!(!is_user_bash_event_result(&json!({"result": {"output": "o", "cancelled": false, "truncated": false}})));
		assert!(!is_user_bash_event_result(&json!({"result": {"output": "o", "exitCode": "0", "cancelled": false, "truncated": false}})));
		assert!(!is_user_bash_event_result(&json!({"result": {"output": "o", "exitCode": 0, "cancelled": false}})));
		assert!(!is_user_bash_event_result(&json!({"result": {"output": "o", "exitCode": 0, "cancelled": false, "truncated": false, "fullOutputPath": 5}})));
	}

	#[tokio::test]
	async fn emit_user_bash_first_result_wins_and_rethrows() {
		// handler 1 returns undefined, handler 2 a valid result → returned.
		let undefined_handler: ExtensionHandler = Arc::new(|event, _ctx| {
			Box::pin(async move { Ok(HandlerOutcome { event, result: None }) })
		});
		let valid_handler: ExtensionHandler = Arc::new(|event, _ctx| {
			Box::pin(async move {
				Ok(HandlerOutcome {
					event,
					result: Some(json!({"operations": {"exec": "fn"}})),
				})
			})
		});
		let runner = runner_with_handlers("user_bash", vec![undefined_handler, valid_handler]).await;
		let result = runner.emit_user_bash(json!({"command": "ls"})).await.unwrap();
		assert_eq!(result, Some(json!({"operations": {"exec": "fn"}})));

		// invalid result: reported + rethrown.
		let invalid_handler: ExtensionHandler = Arc::new(|event, _ctx| {
			Box::pin(async move { Ok(HandlerOutcome { event, result: Some(json!({"nope": 1})) }) })
		});
		let runner = runner_with_handlers("user_bash", vec![invalid_handler]).await;
		let errors: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
		let errors_listener = Arc::clone(&errors);
		runner.on_error(Arc::new(move |error| {
			errors_listener.lock().unwrap().push(error.clone());
		}));
		let error = runner.emit_user_bash(json!({})).await.unwrap_err();
		assert!(error.starts_with("Invalid user_bash handler result:"));
		assert_eq!(errors.lock().unwrap().len(), 1);
		assert!(!matches!(runner.emit_user_bash(json!({})).await, Ok(Some(_))));
	}

	#[tokio::test]
	async fn tool_and_command_contexts() {
		let runtime = ExtensionRuntime::new();
		let runner = ExtensionRunner::new(vec![], runtime, "/proj", None);
		// unbound: tools = [], executeTool = the canned fallback.
		let tool = runner.create_tool_context("call-1", json!("sig"));
		assert!(tool.tools().unwrap().is_empty());
		let fallback = tool.execute_tool("read", json!({"p": 1}), json!({})).await.unwrap();
		assert_eq!(
			fallback,
			json!({
				"toolCall": {"type": "toolCall", "id": "call-1/0", "name": "read", "arguments": {}},
				"result": {
					"content": [{"type": "text", "text": "Nested tool calls are not available in this context"}],
					"details": {}
				},
				"isError": true
			})
		);
		// command context defaults.
		let command = runner.create_command_context();
		assert_eq!(command.new_session(json!({})).await.unwrap(), json!({"cancelled": false}));
		assert_eq!(command.fork("e", json!({})).await.unwrap(), json!({"cancelled": false}));
		assert_eq!(
			command.navigate_tree("t", json!({})).await.unwrap(),
			json!({"cancelled": false})
		);
		assert_eq!(
			command.switch_session("/s", json!({})).await.unwrap(),
			json!({"cancelled": false})
		);
		command.wait_for_idle().await.unwrap();
		command.reload().await.unwrap();
		assert_eq!(command.get_system_prompt_options().unwrap()["cwd"], json!("/proj"));
		// stale guard.
		runner.invalidate(Some("stale"));
		assert_eq!(tool.tools().unwrap_err(), "stale");
		assert_eq!(command.new_session(json!({})).await.unwrap_err(), "stale");
	}

	struct NoActions;

	impl RuntimeActions for NoActions {
		fn send_message(&self, _message: Value, _options: Value) -> Result<(), String> { Ok(()) }
		fn send_user_message(&self, _content: Value, _options: Value) -> Result<(), String> { Ok(()) }
		fn append_entry(&self, _custom_type: &str, _data: Value) -> Result<(), String> { Ok(()) }
		fn set_session_name(&self, _name: &str) -> Result<(), String> { Ok(()) }
		fn get_session_name(&self) -> Result<Value, String> { Ok(Value::Null) }
		fn set_label(&self, _entry_id: &str, _label: Value) -> Result<(), String> { Ok(()) }
		fn get_active_tools(&self) -> Vec<String> { vec!["read".to_string()] }
		fn get_all_tools(&self) -> Result<Value, String> { Ok(json!([])) }
		fn get_settings(&self) -> Result<Value, String> { Ok(json!({})) }
		fn set_active_tools(&self, _names: &[String]) -> Result<(), String> { Ok(()) }
		fn refresh_tools(&self) {}
		fn get_commands(&self) -> Result<Value, String> { Ok(json!([])) }
		fn set_model(&self, _model: Value) -> Result<Value, String> { Ok(json!(true)) }
		fn get_thinking_level(&self) -> Value { json!("high") }
		fn set_thinking_level(&self, _level: Value) -> Result<(), String> { Ok(()) }
	}

	struct TestContextActions;

	impl ContextActions for TestContextActions {
		fn get_model(&self) -> Option<Value> { Some(json!({"id": "m"})) }
		fn get_scoped_models(&self) -> Vec<Value> { vec![json!("s")] }
		fn is_idle(&self) -> bool { false }
		fn is_project_trusted(&self) -> bool { false }
		fn get_signal(&self) -> Option<Value> { Some(json!("sig")) }
		fn abort(&self) {}
		fn has_pending_messages(&self) -> bool { true }
		fn shutdown(&self) {}
		fn get_context_usage(&self) -> Option<Value> { Some(json!({"tokens": 5})) }
		fn compact(&self, _options: Value) -> Result<(), String> { Ok(()) }
		fn get_system_prompt(&self) -> String { "SYS".to_string() }
	}

	#[tokio::test]
	async fn ctx_reads_defaults_and_bound() {
		let runtime = ExtensionRuntime::new();
		let runner = ExtensionRunner::new(vec![], runtime, "/proj", None);
		// createContext class-field defaults (pre-bind); the `isIdle`/
		// `getSystemPrompt`-style members are METHODS (ctx_call).
		assert_eq!(runner.ctx_call_args("isIdle", &[]).unwrap(), json!(true));
		assert_eq!(runner.ctx_call_args("isProjectTrusted", &[]).unwrap(), json!(true));
		assert_eq!(runner.ctx_call_args("hasPendingMessages", &[]).unwrap(), json!(false));
		assert_eq!(runner.ctx_call_args("getContextUsage", &[]).unwrap(), Value::Null);
		assert_eq!(runner.ctx_call_args("getSystemPrompt", &[]).unwrap(), json!(""));
		assert_eq!(runner.ctx_read("signal").unwrap(), Value::Null);
		assert_eq!(runner.ctx_read("scopedModels").unwrap(), json!([]));
		assert_eq!(runner.ctx_read("model").unwrap(), Value::Null);
		assert_eq!(runner.ctx_read("cwd").unwrap(), json!("/proj"));
		assert_eq!(runner.ctx_read("mode").unwrap(), json!("print"));
		assert_eq!(runner.ctx_read("hasUI").unwrap(), json!(false));
		// reading a method (function) is a stand-in: Null.
		assert_eq!(runner.ctx_read("nope").unwrap(), Value::Null);
		// thinkingLevel = the loader stub pre-bind.
		assert_eq!(runner.ctx_read("thinkingLevel").unwrap_err(), NOT_INITIALIZED);
		// `getSystemPromptOptions` is command-context-only (TS createContext
		// has no such member; covered by the command-context test).

		// post-bind delegation.
		runner.bind_core(
			Arc::new(NoActions),
			Arc::new(TestContextActions),
			ProviderHooks::default(),
		);
		assert_eq!(runner.ctx_call_args("isIdle", &[]).unwrap(), json!(false));
		assert_eq!(runner.ctx_call_args("isProjectTrusted", &[]).unwrap(), json!(false));
		assert_eq!(runner.ctx_call_args("hasPendingMessages", &[]).unwrap(), json!(true));
		assert_eq!(runner.ctx_call_args("getContextUsage", &[]).unwrap(), json!({"tokens": 5}));
		assert_eq!(runner.ctx_call_args("getSystemPrompt", &[]).unwrap(), json!("SYS"));
		assert_eq!(runner.ctx_read("signal").unwrap(), json!("sig"));
		assert_eq!(runner.ctx_read("scopedModels").unwrap(), json!(["s"]));
		assert_eq!(runner.ctx_read("model").unwrap(), json!({"id": "m"}));
		assert_eq!(runner.ctx_read("thinkingLevel").unwrap(), json!("high"));
		assert_eq!(runner.ctx_call_args("compact", &[json!({"x": 1})]).unwrap(), Value::Null);
	}

	struct TestUi {
		answer: Result<Value, String>,
		calls: Arc<Mutex<Vec<String>>>,
	}

	impl UiContext for TestUi {
		fn invoke(&self, method: &str, _args: &[Value]) -> Result<Value, String> {
			self.calls.lock().unwrap().push(method.to_string());
			self.answer.clone()
		}
	}

	#[tokio::test]
	async fn ui_prompts_emit_start_end_and_depth() {
		let runtime = ExtensionRuntime::new();
		let runner = ExtensionRunner::new(vec![], runtime, "/proj", None);
		// no-op context: prompts resolve undefined/false, no events.
		assert_eq!(runner.ui_select("t", &["a".to_string()], json!({})).await.unwrap(), Value::Null);
		assert_eq!(runner.ui_confirm("t", "m", json!({})).await.unwrap(), json!(false));
		assert!(runner.drain_ui_prompt_events().is_empty());

		let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
		runner.set_ui_context(
			Some(Arc::new(TestUi { answer: Ok(json!("picked")), calls: Arc::clone(&calls) })),
			"tui",
		);
		assert!(runner.has_ui());
		let value = runner.ui_select("Pick", &["a".to_string(), "b".to_string()], json!({})).await.unwrap();
		assert_eq!(value, json!("picked"));
		let events = runner.drain_ui_prompt_events();
		assert_eq!(
			events,
			vec![
				json!({"type": "ui_prompt_start", "reason": "ui_prompt", "kind": "select", "title": "Pick"}),
				json!({"type": "ui_prompt_end", "reason": "ui_prompt", "kind": "select", "title": "Pick"}),
			]
		);
		// nested prompt: depth suppresses the inner events (finish clamps).
		let nested = runner.with_ui_prompt("select", Some("Outer"), async {
			runner.with_ui_prompt("select", Some("Inner"), async { Ok::<(), String>(()) }).await
		});
		nested.await.unwrap();
		let events = runner.drain_ui_prompt_events();
		assert_eq!(events.len(), 2);
		assert_eq!(events[0]["title"], json!("Outer"));
		assert_eq!(events[1]["title"], json!("Outer"));
		// a throwing prompt still emits the end event.
		runner.set_ui_context(
			Some(Arc::new(TestUi { answer: Err("ui boom".to_string()), calls })),
			"tui",
		);
		assert_eq!(runner.ui_input("t", None, json!({})).await.unwrap_err(), "ui boom");
		let events = runner.drain_ui_prompt_events();
		assert_eq!(events.len(), 2);
		assert_eq!(events[0]["kind"], json!("input"));
		// custom: no title key.
		runner.set_ui_context(
			Some(Arc::new(TestUi { answer: Ok(json!(1)), calls: Arc::new(Mutex::new(Vec::new())) })),
			"print",
		);
		runner.ui_custom(vec![json!({"overlay": true})]).await.unwrap();
		let events = runner.drain_ui_prompt_events();
		assert!(events[0].get("title").is_none());
	}

	#[tokio::test]
	async fn pre_bind_action_stubs() {
		// loader.ts createExtensionRuntime: every action throws the stub
		// message except refreshTools (no-op) and setModel (rejected with the
		// short message).
		let runtime = ExtensionRuntime::new();
		let stub = NOT_INITIALIZED.to_string();
		assert_eq!(runtime.send_message(json!({}), json!({})).unwrap_err(), stub);
		assert_eq!(runtime.send_user_message(json!("hi"), json!({})).unwrap_err(), stub);
		assert_eq!(runtime.append_entry("t", json!(1)).unwrap_err(), stub);
		assert_eq!(runtime.set_session_name("n").unwrap_err(), stub);
		assert_eq!(runtime.get_session_name().unwrap_err(), stub);
		assert_eq!(runtime.set_label("e", json!("l")).unwrap_err(), stub);
		assert_eq!(runtime.get_active_tools().unwrap_err(), stub);
		assert_eq!(runtime.get_all_tools().unwrap_err(), stub);
		assert_eq!(runtime.get_settings().unwrap_err(), stub);
		assert_eq!(runtime.set_active_tools(&["read".to_string()]).unwrap_err(), stub);
		assert_eq!(runtime.get_commands().unwrap_err(), stub);
		assert_eq!(runtime.get_thinking_level().unwrap_err(), stub);
		assert_eq!(runtime.set_thinking_level(json!("high")).unwrap_err(), stub);
		// no-ops / rejected-promise variant
		runtime.refresh_tools();
		assert_eq!(
			runtime.set_model(json!("m")).unwrap_err(),
			"Extension runtime not initialized"
		);
	}

	#[tokio::test]
	async fn emit_cancel_short_circuits() {
		let runtime = ExtensionRuntime::new();
		let factory: FactoryFn = Arc::new(|pi: &ExtensionApi| {
			let pi = pi.clone();
			Box::pin(async move {
				let handler: ExtensionHandler = Arc::new(|event, _ctx| {
					Box::pin(async move { Ok(HandlerOutcome { event, result: Some(json!({"cancel": true})) }) })
				});
				pi.on("session_before_switch", handler).unwrap();
				Ok(())
			})
		});
		let extension = load_extension_from_factory(factory, "/proj", None, &runtime, "<inline>").await.unwrap();
		let runner = ExtensionRunner::new(vec![extension], runtime, "/proj", None);
		assert!(runner.has_handlers("session_before_switch"));
		let result = runner.emit(json!({"type": "session_before_switch", "sessionPath": "/x"})).await;
		assert_eq!(result, Some(json!({"cancel": true})));
	}
}
