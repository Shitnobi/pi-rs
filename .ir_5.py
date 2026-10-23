p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()

# --- trait ---
anchor='''/// The `ExtensionCommandContextActions` bound by `bindCommandContext`'''
trait_def='''/// The `ExtensionUIContext` seam. The real TUI is out of scope: every
/// method routes through one Value-typed [`UiContext::invoke`], which the
/// host backs with its fake-UI script. `Err` models a throw; `Value::Null`
/// models `undefined`.
pub trait UiContext: Send + Sync {
	/// `ui.<method>(...args)` — `args` is the positional argument list.
	fn invoke(&self, method: &str, args: &[Value]) -> Result<Value, String>;
}

''' + anchor
assert anchor in s; s=s.replace(anchor,trait_def,1)

# --- shared fields ---
old='''	command_actions: Mutex<Option<Arc<dyn CommandContextActions>>>,'''
new='''	command_actions: Mutex<Option<Arc<dyn CommandContextActions>>>,
	/// `setUIContext` target; `None` = the no-op context.
	ui_context: Mutex<Option<Arc<dyn UiContext>>>,
	/// `uiPromptDepth` (outermost prompt wins) and the active prompt.
	ui_prompt_depth: Mutex<i64>,
	active_ui_prompt: Mutex<Option<(String, Option<String>)>>,
	/// `ui_prompt_start`/`ui_prompt_end` events deferred through
	/// `queueMicrotask`; the host drains and emits them.
	ui_prompt_queue: Mutex<Vec<Value>>,'''
assert old in s; s=s.replace(old,new)

old='''				command_actions: Mutex::new(None),'''
new='''				command_actions: Mutex::new(None),
				ui_context: Mutex::new(None),
				ui_prompt_depth: Mutex::new(0),
				active_ui_prompt: Mutex::new(None),
				ui_prompt_queue: Mutex::new(Vec::new()),'''
assert old in s; s=s.replace(old,new)

# --- methods: insert before bind_command_context ---
anchor2='''	/// `bindCommandContext(actions?)` — `None` resets every handler to the'''
new_methods='''	/// `setUIContext(uiContext?, mode = "print")` — `None` restores the
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

''' + anchor2
assert anchor2 in s; s=s.replace(anchor2,new_methods,1)
open(p,'w').write(s)
print("item 5/6 state+wrapper")
