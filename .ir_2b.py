p='crates/pi-cli/src/extension_runner_harness.rs'
s=open(p).read()
old='''impl RuntimeActions for FakeActions {
	fn get_active_tools(&self) -> Vec<String> {
		self.tools.clone()
	}
	fn get_thinking_level(&self) -> Value {
		self.thinking.clone()
	}
	fn refresh_tools(&self) {
		self.log.lock().unwrap().push("actions.refreshTools".to_string());
	}
}'''
new='''impl RuntimeActions for FakeActions {
	fn send_message(&self, message: Value, options: Value) -> Result<(), String> {
		self.record("sendMessage", &json!([message, options]))
	}
	fn send_user_message(&self, content: Value, options: Value) -> Result<(), String> {
		self.record("sendUserMessage", &json!([content, options]))
	}
	fn append_entry(&self, custom_type: &str, data: Value) -> Result<(), String> {
		self.record("appendEntry", &json!([custom_type, data]))
	}
	fn set_session_name(&self, name: &str) -> Result<(), String> {
		self.record("setSessionName", &json!([name]))
	}
	fn get_session_name(&self) -> Result<Value, String> {
		self.record_value("getSessionName", &json!([]))
	}
	fn set_label(&self, entry_id: &str, label: Value) -> Result<(), String> {
		self.record("setLabel", &json!([entry_id, label]))
	}
	fn get_active_tools(&self) -> Vec<String> {
		self.tools.clone()
	}
	fn get_all_tools(&self) -> Result<Value, String> {
		self.record_value("getAllTools", &json!([]))
	}
	fn get_settings(&self) -> Result<Value, String> {
		self.record_value("getSettings", &json!([]))
	}
	fn set_active_tools(&self, names: &[String]) -> Result<(), String> {
		self.record("setActiveTools", &json!([names]))
	}
	fn refresh_tools(&self) {
		self.log.lock().unwrap().push("actions.refreshTools".to_string());
	}
	fn get_commands(&self) -> Result<Value, String> {
		self.record_value("getCommands", &json!([]))
	}
	fn set_model(&self, model: Value) -> Result<Value, String> {
		self.record("setModel", &json!([model]))?;
		Ok(json!(true))
	}
	fn get_thinking_level(&self) -> Value {
		self.thinking.clone()
	}
	fn set_thinking_level(&self, level: Value) -> Result<(), String> {
		self.record("setThinkingLevel", &json!([level]))
	}
}'''
assert old in s; s=s.replace(old,new)
open(p,'w').write(s)
print("FakeActions extended")
