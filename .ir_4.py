p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()

old='''/// The `ExtensionContextActions` slice bound by `bindCore`.
pub trait ContextActions: Send + Sync {
	fn is_idle(&self) -> bool;
	fn is_project_trusted(&self) -> bool;
	fn get_model(&self) -> Option<Value>;
	fn shutdown(&self);
}'''
new='''/// The full `ExtensionContextActions` bound by `bindCore` (`required except
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
}'''
assert old in s; s=s.replace(old,new)

# shared field
old='''	context_actions: Mutex<Option<Arc<dyn ContextActions>>>,'''
new='''	context_actions: Mutex<Option<Arc<dyn ContextActions>>>,
	command_actions: Mutex<Option<Arc<dyn CommandContextActions>>>,'''
assert old in s; s=s.replace(old,new)

old='''				context_actions: Mutex::new(None),'''
new='''				context_actions: Mutex::new(None),
				command_actions: Mutex::new(None),'''
assert old in s; s=s.replace(old,new)

# bind_command_context after bind_core (anchor on the fn end is awkward; insert
# right before bind_core's doc comment instead)
anchor='''	pub fn bind_core(
		&self,'''
new_fn='''	/// `bindCommandContext(actions?)` — `None` resets every handler to the
	/// TS defaults (`{ cancelled: false }` / no-op).
	pub fn bind_command_context(&self, actions: Option<Arc<dyn CommandContextActions>>) {
		*self.shared.command_actions.lock().unwrap() = actions;
	}

''' + anchor
assert anchor in s; s=s.replace(anchor,new_fn,1)
open(p,'w').write(s)
print("item 4 traits")
