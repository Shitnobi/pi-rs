p='crates/pi-cli/src/extension_runner_harness.rs'
s=open(p).read()
old='''impl ContextActions for FakeContext {
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
}'''
new='''impl ContextActions for FakeContext {
	fn is_idle(&self) -> bool {
		self.idle
	}
	fn is_project_trusted(&self) -> bool {
		self.trusted
	}
	fn get_model(&self) -> Option<Value> {
		self.model.clone()
	}
	fn get_scoped_models(&self) -> Vec<Value> {
		Vec::new()
	}
	fn get_signal(&self) -> Option<Value> {
		None
	}
	fn abort(&self) {
		self.log.lock().unwrap().push("contextActions.abort".to_string());
	}
	fn has_pending_messages(&self) -> bool {
		false
	}
	fn shutdown(&self) {
		self.log.lock().unwrap().push("contextActions.shutdown".to_string());
	}
	fn get_context_usage(&self) -> Option<Value> {
		None
	}
	fn compact(&self, _options: Value) -> Result<(), String> {
		Ok(())
	}
	fn get_system_prompt(&self) -> String {
		String::new()
	}
}'''
assert old in s; s=s.replace(old,new)
open(p,'w').write(s)
print("FakeContext extended")
