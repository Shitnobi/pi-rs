p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()
anchor='''	/// `hasHandlers(eventType)`.'''
new='''	/// `createToolContext(toolCallId, signal)` — the guarded `tools` getter
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

''' + anchor
assert anchor in s; s=s.replace(anchor,new,1)

# views appended before the ExtensionHost bridge section
anchor2='''// ============================================================================
// ExtensionHost bridge — the real runner behind the agent-session seam
// ============================================================================'''
views='''/// `ExtensionToolContext` — `createContext()` + `tools`/`executeTool()`.
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

''' + anchor2
assert anchor2 in s; s=s.replace(anchor2,views,1)

# shared assert helper
anchor3='''	fn ctx_read(&self, field: &str) -> Result<Value, String> {'''
new3='''	fn assert_active_shared(&self) -> Result<(), String> {
		match self.shared.stale_message.lock().unwrap().clone() {
			Some(message) => Err(message),
			None => Ok(()),
		}
	}

''' + anchor3
assert anchor3 in s; s=s.replace(anchor3,new3,1)
open(p,'w').write(s)
print("item 8 views")
