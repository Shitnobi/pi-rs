p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()

old_start = s.index('	fn ctx_read(&self, field: &str) -> Result<Value, String> {')
old_end = s.index('	/// `hasHandlers(eventType)`.')
new = '''	fn ctx_read(&self, field: &str) -> Result<Value, String> {
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
			"getSystemPromptOptions" => Ok(actions
				.as_ref()
				.and_then(|actions| actions.get_system_prompt_options())
				.unwrap_or_else(|| {
					normalize_options_raw(&json!({"cwd": self.shared.cwd}))
						.unwrap_or_else(|_| json!({}))
				})),
			_ => Ok(Value::Null),
		}
	}

	fn ctx_call(&self, method: &str) -> Result<Value, String> {
		self.ctx_call_args(method, &[])
	}

''' + s[old_end:]
s = s[:old_start] + new

# CtxView::call gains args
s=s.replace('''	/// Invoke one context method (TS `ctx.shutdown()`); `Err` = the guarded
	/// call threw (stale ctx).
	pub fn call(&self, method: &str) -> Result<Value, String> {
		match self {
			CtxView::Raw(_) => Err("not a function".to_string()),
			CtxView::Runner(shared) => ExtensionRunner { shared: Arc::clone(shared) }.ctx_call(method),''','''	/// Invoke one context method (TS `ctx.shutdown()`); `Err` = the guarded
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
			}''')
s=s.replace('''				ExtensionRunner { shared: Arc::clone(shared) }.ctx_call(method)
			}
		}
	}
}''','''				ExtensionRunner { shared: Arc::clone(shared) }.ctx_call_args(method, args)
			}
		}
	}
}''')
open(p,'w').write(s)
print("ctx map")
