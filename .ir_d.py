p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()
anchor='''	pub fn emit_error(&self, error: &Value) {'''
assert anchor in s
new='''	/// Host injection for the default `docs` prompt section paths.
	pub fn set_doc_paths(&self, paths: DocPaths) {
		*self.shared.doc_paths.lock().unwrap() = paths;
	}

	fn doc_paths(&self) -> DocPaths {
		self.shared.doc_paths.lock().unwrap().clone()
	}

	/// `emitBeforeAgentStart(trigger, systemPromptOptions)` — the leading
	/// normalize may throw (the emitter rejects; V8 error text); handler
	/// errors are reported. Returns `(messages, systemPromptOptions)`.
	pub async fn emit_before_agent_start(
		&self,
		trigger: Value,
		system_prompt_options: Value,
	) -> Result<(Vec<Value>, Value), String> {
		let mut current_options = normalize_options_raw(&system_prompt_options)?;
		let paths = self.doc_paths();
		let mut messages: Vec<Value> = Vec::new();
		for (extension_path, handlers) in self.snapshot_handlers("before_agent_start") {
			for (_, handler) in handlers {
				// `ctx.getSystemPrompt()` = assertActive + render(live options).
				let render_shared = Arc::clone(&self.shared);
				let render_options = current_options.clone();
				let render_paths = paths.clone();
				let renderer: Arc<dyn Fn() -> Result<String, String> + Send + Sync> =
					Arc::new(move || {
						if let Some(message) = render_shared.stale_message.lock().unwrap().clone() {
							return Err(message);
						}
						build_system_prompt(&render_options, &render_paths)
					});
				let ctx = CtxView::BeforeAgentStart {
					shared: Arc::clone(&self.shared),
					renderer,
				};

				// `{...trigger, type, get systemPrompt(), systemPromptOptions:
				// currentOptions}` — fresh per handler. TS renders the getter
				// lazily; options change only between handlers, so the value is
				// materialized here (a render error leaves the key absent where
				// TS throws at read — plan divergence list).
				let mut event = trigger.clone();
				if let Some(object) = event.as_object_mut() {
					object.insert("type".to_string(), json!("before_agent_start"));
					object.insert("systemPromptOptions".to_string(), current_options.clone());
					if let Ok(prompt) = build_system_prompt(&current_options, &paths) {
						object.insert("systemPrompt".to_string(), json!(prompt));
					}
				}

				match handler(event, ctx).await {
					Ok(HandlerOutcome { event: returned_event, result }) => {
						// In TS the event's options object IS `currentOptions`:
						// in-place edits are observable in the return value.
						if let Some(options) = returned_event.get("systemPromptOptions") {
							if options != &current_options {
								current_options = options.clone();
							}
						}
						if let Some(result) = result {
							if js_truthy(&result) {
								if let Some(message) = result.get("message") {
									if js_truthy(message) {
										messages.push(message.clone());
									}
								}
								// `result.systemPrompt !== undefined` — present
								// null reassigns (probe-pinned).
								if let Some(prompt) = result.get("systemPrompt") {
									if let Some(object) = current_options.as_object_mut() {
										object.insert("forceSystemPrompt".to_string(), prompt.clone());
									}
								}
							}
						}
					}
					Err(error) => {
						self.emit_error(&json!({
							"extensionPath": extension_path,
							"event": "before_agent_start",
							"error": error,
						}));
					}
				}
			}
		}
		Ok((messages, current_options))
	}

''' + anchor
s=s.replace(anchor,new,1)
open(p,'w').write(s)
print("step D done")
