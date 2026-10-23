p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()

# cwd on the API (pi.exec's fallback)
old='''pub struct ExtensionApi {
	extension: Arc<Mutex<Extension>>,
	runtime: ExtensionRuntime,
	loading: Arc<Mutex<LoadingState>>,
	bus: Option<Arc<EventBusController>>,
}'''
new='''pub struct ExtensionApi {
	extension: Arc<Mutex<Extension>>,
	runtime: ExtensionRuntime,
	loading: Arc<Mutex<LoadingState>>,
	bus: Option<Arc<EventBusController>>,
	/// The loader's resolved cwd — `pi.exec`'s `options?.cwd ?? cwd`.
	cwd: String,
}'''
assert old in s; s=s.replace(old,new)

old='''pub async fn load_extension_from_factory(
	factory: FactoryFn,
	_cwd: &str,'''
new='''pub async fn load_extension_from_factory(
	factory: FactoryFn,
	cwd: &str,'''
assert old in s; s=s.replace(old,new)

old='''			bus_start: runtime.tracked_count(),
			next_handler_id: 0,
		})),
		bus,
	};'''
new='''			bus_start: runtime.tracked_count(),
			next_handler_id: 0,
		})),
		bus,
		cwd: cwd.to_string(),
	};'''
assert old in s; s=s.replace(old,new)

# action methods after assert_active/apply_runtime_change (before pi.on)
anchor='''	/// `pi.on(event, handler)` — returns the unsubscribe token.'''
new_methods='''	// ---- Action methods (`assertActive()` then runtime delegation) -----

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

''' + anchor
assert anchor in s; s=s.replace(anchor,new_methods,1)
open(p,'w').write(s)
print("item 3 methods added")
