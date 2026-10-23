p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()
anchor='''	/// `createToolContext(toolCallId, signal)`'''
new='''	/// `emitCacheWarmingDecision(event)` — the event's own action unless a
	/// handler overrides it (`result.action !== undefined`); last wins.
	pub async fn emit_cache_warming_decision(&self, event: Value) -> Value {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let event_type = event.get("type").and_then(Value::as_str).unwrap_or("").to_string();
		let mut action = event.get("action").cloned().unwrap_or(Value::Null);
		for (extension_path, handlers) in self.snapshot_handlers(&event_type) {
			for (_, handler) in handlers {
				match handler(event.clone(), ctx.clone()).await {
					Ok(HandlerOutcome { result: Some(result), .. }) => {
						// `result?.action !== undefined` — present null overrides.
						if let Some(next) = result.get("action") {
							action = next.clone();
						}
					}
					Ok(_) => {}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": event_type,
							"error": error,
						}));
					}
				}
			}
		}
		action
	}

	/// `emitToolResult(event)` — the single `currentEvent` is patched in
	/// order; replacing `content` without `structuredContent` DELETES the
	/// stale structured content; the return carries only present fields.
	pub async fn emit_tool_result(&self, event: Value) -> Result<Option<Value>, String> {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let mut current_event = event.clone();
		if !current_event.is_object() {
			current_event = json!({});
		}
		let mut modified = false;
		for (extension_path, handlers) in self.snapshot_handlers("tool_result") {
			for (_, handler) in handlers {
				match handler(current_event.clone(), ctx.clone()).await {
					Ok(HandlerOutcome { result: Some(handler_result), .. })
						if js_truthy(&handler_result) =>
					{
						if let Some(content) = handler_result.get("content") {
							set_field(&mut current_event, "content", content.clone());
							if handler_result.get("structuredContent").is_none() {
								if let Some(object) = current_event.as_object_mut() {
									object.remove("structuredContent");
								}
							}
							modified = true;
						}
						if let Some(details) = handler_result.get("details") {
							set_field(&mut current_event, "details", details.clone());
							modified = true;
						}
						if let Some(structured) = handler_result.get("structuredContent") {
							set_field(&mut current_event, "structuredContent", structured.clone());
							modified = true;
						}
						if let Some(is_error) = handler_result.get("isError") {
							set_field(&mut current_event, "isError", is_error.clone());
							modified = true;
						}
						if let Some(usage) = handler_result.get("usage") {
							set_field(&mut current_event, "usage", usage.clone());
							modified = true;
						}
					}
					Ok(_) => {}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "tool_result",
							"error": error,
						}));
					}
				}
			}
		}
		if !modified {
			return Ok(None);
		}
		let mut result = Map::new();
		for key in ["content", "details", "structuredContent", "isError", "usage"] {
			if let Some(value) = current_event.get(key) {
				result.insert(key.to_string(), value.clone());
			}
		}
		Ok(Some(Value::Object(result)))
	}

	/// `emitToolCall(event)` — truthy results accumulate (last wins), a
	/// truthy `block` short-circuits; handler throws are NOT caught.
	pub async fn emit_tool_call(&self, event: Value) -> Result<Option<Value>, String> {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let mut result: Option<Value> = None;
		for (_, handlers) in self.snapshot_handlers("tool_call") {
			for (_, handler) in handlers {
				let outcome = handler(event.clone(), ctx.clone()).await?;
				if let Some(handler_result) = outcome.result {
					if js_truthy(&handler_result) {
						if handler_result.get("block").map(js_truthy).unwrap_or(false) {
							return Ok(Some(handler_result));
						}
						result = Some(handler_result);
					}
				}
			}
		}
		Ok(result)
	}

	/// `emitBeforeProviderRequest(payload)` — any non-undefined result
	/// replaces the payload (last wins).
	pub async fn emit_before_provider_request(&self, payload: Value) -> Value {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let mut current = payload;
		for (extension_path, handlers) in self.snapshot_handlers("before_provider_request") {
			for (_, handler) in handlers {
				let event = json!({"type": "before_provider_request", "payload": current.clone()});
				match handler(event, ctx.clone()).await {
					Ok(HandlerOutcome { result: Some(result), .. }) => current = result,
					Ok(_) => {}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "before_provider_request",
							"error": error,
						}));
					}
				}
			}
		}
		current
	}

	/// `emitBeforeProviderHeaders(headers)` — handlers mutate `headers` in
	/// place (return value ignored); mutations accumulate.
	pub async fn emit_before_provider_headers(&self, headers: Value) -> Value {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let mut current = headers;
		for (extension_path, handlers) in self.snapshot_handlers("before_provider_headers") {
			for (_, handler) in handlers {
				let event = json!({"type": "before_provider_headers", "headers": current.clone()});
				match handler(event, ctx.clone()).await {
					Ok(HandlerOutcome { event: returned, .. }) => {
						if let Some(next) = returned.get("headers") {
							current = next.clone();
						}
					}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "before_provider_headers",
							"error": error,
						}));
					}
				}
			}
		}
		current
	}

	/// `emitResourcesDiscover(cwd, reason)` — collects each handler's
	/// non-empty path lists tagged with the extension path.
	pub async fn emit_resources_discover(&self, cwd: &str, reason: Value) -> Value {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let mut skill_paths: Vec<Value> = Vec::new();
		let mut prompt_paths: Vec<Value> = Vec::new();
		let mut theme_paths: Vec<Value> = Vec::new();
		for (extension_path, handlers) in self.snapshot_handlers("resources_discover") {
			for (_, handler) in handlers {
				let event = json!({"type": "resources_discover", "cwd": cwd, "reason": reason.clone()});
				match handler(event, ctx.clone()).await {
					Ok(HandlerOutcome { result: Some(result), .. }) => {
						collect_paths(&result, "skillPaths", &extension_path, &mut skill_paths);
						collect_paths(&result, "promptPaths", &extension_path, &mut prompt_paths);
						collect_paths(&result, "themePaths", &extension_path, &mut theme_paths);
					}
					Ok(_) => {}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "resources_discover",
							"error": error,
						}));
					}
				}
			}
		}
		json!({"skillPaths": skill_paths, "promptPaths": prompt_paths, "themePaths": theme_paths})
	}

	/// `emitInput(text, images, source, streamingBehavior)` — transforms
	/// chain, `handled` short-circuits; the final result is `transform` only
	/// when something changed.
	pub async fn emit_input(
		&self,
		text: &str,
		images: Option<Value>,
		source: Value,
		streaming_behavior: Option<Value>,
	) -> Value {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let original_text = text.to_string();
		let original_images = images.clone().unwrap_or(Value::Null);
		let mut current_text = original_text.clone();
		let mut current_images = original_images.clone();
		for (extension_path, handlers) in self.snapshot_handlers("input") {
			for (_, handler) in handlers {
				let mut event = json!({"type": "input", "text": current_text, "images": current_images, "source": source});
				if let Some(behavior) = &streaming_behavior {
					event["streamingBehavior"] = behavior.clone();
				}
				match handler(event, ctx.clone()).await {
					Ok(HandlerOutcome { result: Some(result), .. }) => {
						match result.get("action").and_then(Value::as_str) {
							Some("handled") => return result,
							Some("transform") => {
								current_text = result.get("text").cloned().unwrap_or(Value::Null);
								if let Some(images) = result.get("images") {
									if !images.is_null() {
										current_images = images.clone();
									}
								}
							}
							_ => {}
						}
					}
					Ok(_) => {}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "input",
							"error": error,
						}));
					}
				}
			}
		}
		if current_text != json!(original_text) || current_images != original_images {
			json!({"action": "transform", "text": current_text, "images": current_images})
		} else {
			json!({"action": "continue"})
		}
	}

	/// `emitBoundary(baseEvent, buildContext)` — the host rebuild hook runs
	/// once up front (a throw escapes) and after every handler; a failing
	/// rebuild marks the batch invalid (`Invalid boundary entries: …`).
	pub async fn emit_boundary(
		&self,
		base_event: Value,
		build_context: BoundaryBuildContext,
	) -> Result<Value, String> {
		let ctx = CtxView::Runner(Arc::clone(&self.shared));
		let event_type = base_event.get("type").and_then(Value::as_str).unwrap_or("").to_string();
		let mut entries: Vec<Value> = Vec::new();
		let mut should_continue = false;
		let mut context = build_context(entries.clone()).await?;
		let mut valid = true;
		for (extension_path, handlers) in self.snapshot_handlers(&event_type) {
			for (_, handler) in handlers {
				let mut event = base_event.clone();
				if let Some(object) = event.as_object_mut() {
					object.insert("entries".to_string(), json!(entries));
					object.insert("continue".to_string(), json!(should_continue));
					object.insert("context".to_string(), context.clone());
				}
				match handler(event, ctx.clone()).await {
					Ok(HandlerOutcome { result: Some(result), .. }) => {
						if let Some(next) = result.get("entries") {
							entries = next.as_array().cloned().unwrap_or_default();
						}
						if let Some(next) = result.get("continue") {
							should_continue = js_truthy(next);
						}
					}
					Ok(_) => {}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": event_type,
							"error": error,
						}));
					}
				}
				match build_context(entries.clone()).await {
					Ok(next) => {
						context = next;
						valid = true;
					}
					Err(message) => {
						valid = false;
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": event_type,
							"error": format!("Invalid boundary entries: {}", message),
						}));
					}
				}
			}
		}
		if valid {
			Ok(json!({"entries": entries, "continue": should_continue, "context": context, "valid": true}))
		} else {
			Ok(json!({"entries": [], "continue": false, "context": context, "valid": false}))
		}
	}

''' + anchor
assert anchor in s; s=s.replace(anchor,new,1)

# helpers + BoundaryBuildContext type near js_truthy
anchor2='''/// `isUserBashEventResult(value)`'''
helpers='''/// `entries` rebuild hook handed to `emitBoundary`.
pub type BoundaryBuildContext =
	Arc<dyn Fn(Vec<Value>) -> BoxFuture<'static, Result<Value, String>> + Send + Sync>;

fn set_field(object: &mut Value, key: &str, value: Value) {
	if let Some(map) = object.as_object_mut() {
		map.insert(key.to_string(), value);
	}
}

/// `if (result?.x?.length) out.push(...result.x.map(path => ({path,
/// extensionPath})))`.
fn collect_paths(result: &Value, key: &str, extension_path: &str, out: &mut Vec<Value>) {
	if let Some(Value::Array(paths)) = result.get(key) {
		if !paths.is_empty() {
			for path in paths {
				out.push(json!({"path": path, "extensionPath": extension_path}));
			}
		}
	}
}

''' + anchor2
assert anchor2 in s; s=s.replace(anchor2,helpers,1)
open(p,'w').write(s)
print("item 10")
