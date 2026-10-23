p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()

old='''/// The `ExtensionActions` slice bound by `bindCore`.
pub trait RuntimeActions: Send + Sync {
	fn get_active_tools(&self) -> Vec<String>;
	fn get_thinking_level(&self) -> Value;
	fn refresh_tools(&self);
}'''
new='''/// The full `ExtensionActions` set bound by `bindCore` (`runtime.<action>`).
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
}'''
assert old in s; s=s.replace(old,new)

old='''	/// `runtime.getActiveTools()` — pre-bind stub throws.
	pub fn get_active_tools(&self) -> Result<Vec<String>, String> {'''
new='''	/// The loader's pre-bind stub: every action except `refreshTools` and
	/// `setModel` throws this exact message.
	pub const NOT_INITIALIZED: &'static str =
		"Extension runtime not initialized. Action methods cannot be called during extension loading.";
	/// `setModel`'s pre-bind stub rejects with the shorter message.
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
			None => Err(NOT_INITIALIZED_SET_MODEL.to_string()),
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
	pub fn get_active_tools(&self) -> Result<Vec<String>, String> {'''
assert old in s; s=s.replace(old,new)

# The old NOT_INITIALIZED const (module-level) — check for duplicates
open(p,'w').write(s)
print("trait+runtime: done")
