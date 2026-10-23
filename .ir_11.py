p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()

# helper functions near collect_paths
anchor='''/// `if (result?.x?.length) out.push(...result.x.map(path => ({path,'''
helpers='''/// `sameMessages(left, right)` — TS compares element *identity*; the port
/// compares JSON values (documented divergence for in-place message
/// mutation).
fn same_messages(left: &[Value], right: &[Value]) -> bool {
	left.len() == right.len() && left.iter().zip(right).all(|(a, b)| a == b)
}

/// `getCurrentSystemMessage(messages)` — replay every system message into
/// one leading message (content joined `"\\n\\n"`, section patches by name,
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
			None => return Err("content.filter is not a function".to_string()),
			Some(value) => content_text_raw(value, "")?,
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
	result.insert("content".to_string(), json!(content.join("\\n\\n")));
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

''' + anchor
assert anchor in s; s=s.replace(anchor,helpers,1)

# emit_context before create_tool_context
anchor2='''	/// `createToolContext(toolCallId, signal)`'''
new='''	/// `emitContext(messages)` — two phases: `context` handlers see the
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
				let outcome = (|| async {
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
				})()
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
				let outcome = (|| async {
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
				})()
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

''' + anchor2
assert anchor2 in s; s=s.replace(anchor2,new,1)

# imports: content_text_raw
s=s.replace('''use super::system_prompt::{build_system_prompt, normalize_options_raw, DocPaths};''','''use super::system_prompt::{build_system_prompt, content_text_raw, normalize_options_raw, DocPaths};''')
open(p,'w').write(s)
print("item 11")
