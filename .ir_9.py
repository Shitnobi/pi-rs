p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()
anchor='''	/// `createToolContext(toolCallId, signal)`'''
new='''	/// `emitUserBash(event)` — the first non-undefined handler result must
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

''' + anchor
assert anchor in s; s=s.replace(anchor,new,1)

# validator near js_truthy
anchor2='''/// JS `typeof` for the flag-default error message.'''
validator='''/// `isUserBashEventResult(value)` — exactly one of `operations`/`result`;
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

''' + anchor2
assert anchor2 in s; s=s.replace(anchor2,validator,1)
open(p,'w').write(s)
print("item 9")
