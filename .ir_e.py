p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()
anchor='''	#[tokio::test]
	async fn emit_cancel_short_circuits() {'''
assert anchor in s
new='''	fn runner_with_handlers(
		event: &'static str,
		handlers: Vec<ExtensionHandler>,
	) -> ExtensionRunner {
		let runtime = ExtensionRuntime::new();
		let factory: FactoryFn = Arc::new(move |pi: &ExtensionApi| {
			let pi = pi.clone();
			let handlers: Vec<ExtensionHandler> = handlers.clone();
			Box::pin(async move {
				for handler in handlers {
					pi.on(event, handler).unwrap();
				}
				Ok(())
			})
		});
		let extension = tokio::runtime::Handle::current()
			.block_on(async { load_extension_from_factory(factory, "/proj", None, &runtime, "<inline>").await })
			.unwrap();
		ExtensionRunner::new(vec![extension], runtime, "/proj", None)
	}

	#[tokio::test]
	async fn before_agent_start_merges_message_and_force() {
		let handler: ExtensionHandler = Arc::new(|_event, _ctx| {
			Box::pin(async {
				Ok(HandlerOutcome {
					event: json!({}),
					result: Some(json!({
						"message": {"role": "user", "content": [{"type": "text", "text": "m"}]},
						"systemPrompt": "X",
					})),
				})
			})
		});
		let runner = runner_with_handlers("before_agent_start", vec![handler]);
		let (messages, options) = runner
			.emit_before_agent_start(json!({"prompt": "p"}), json!({"cwd": "/p", "customPrompt": "CP"}))
			.await
			.unwrap();
		assert_eq!(messages, vec![json!({"role": "user", "content": [{"type": "text", "text": "m"}]})]);
		assert_eq!(options["forceSystemPrompt"], json!("X"));
		assert_eq!(options["customPrompt"], json!("CP"));
	}

	#[tokio::test]
	async fn before_agent_start_null_reassigns_and_truthy_gate() {
		// `result.systemPrompt !== undefined` → null RESETS; a falsey result
		// and a falsey message are skipped.
		let handler: ExtensionHandler = Arc::new(|_event, _ctx| {
			Box::pin(async {
				Ok(HandlerOutcome {
					event: json!({}),
					result: Some(json!({"systemPrompt": null, "message": null})),
				})
			})
		});
		let runner = runner_with_handlers("before_agent_start", vec![handler]);
		let (messages, options) =
			runner.emit_before_agent_start(json!({}), json!({"cwd": "/p"})).await.unwrap();
		assert!(messages.is_empty());
		assert_eq!(options["forceSystemPrompt"], Value::Null);
		assert!(options.as_object().unwrap().contains_key("forceSystemPrompt"));
	}

	#[tokio::test]
	async fn before_agent_start_live_options_object() {
		// TS: `event.systemPromptOptions` is the live `currentOptions` ref.
		let handler: ExtensionHandler = Arc::new(|mut event, _ctx| {
			Box::pin(async move {
				event["systemPromptOptions"]["customPrompt"] = json!("NEW");
				Ok(HandlerOutcome { event, result: None })
			})
		});
		let runner = runner_with_handlers("before_agent_start", vec![handler]);
		let (_, options) =
			runner.emit_before_agent_start(json!({}), json!({"cwd": "/p"})).await.unwrap();
		assert_eq!(options["customPrompt"], json!("NEW"));
	}

	#[tokio::test]
	async fn before_agent_start_normalize_throws_out() {
		let runner = runner_with_handlers("before_agent_start", vec![]);
		let err = runner
			.emit_before_agent_start(json!({}), json!({"selectedTools": 5}))
			.await
			.unwrap_err();
		assert_eq!(
			err,
			"(input.selectedTools ?? [\\"read\\", \\"bash\\", \\"edit\\", \\"write\\"]) is not iterable"
		);
	}

	#[tokio::test]
	async fn before_agent_start_render_surfaces_and_handler_errors() {
		// forceSystemPrompt: null → both the event getter and ctx render throw;
		// the handler observes the ctx error, and its own throw is reported.
		let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
		let seen_handler = Arc::clone(&seen);
		let handler: ExtensionHandler = Arc::new(move |event, ctx| {
			let seen = Arc::clone(&seen_handler);
			Box::pin(async move {
				seen.lock().unwrap().push(format!("event:{}", event.get("systemPrompt").is_some()));
				match ctx.call("getSystemPrompt") {
					Ok(value) => seen.lock().unwrap().push(format!("call:{}", value)),
					Err(message) => seen.lock().unwrap().push(format!("call!{}", message)),
				}
				Err("boom".to_string())
			})
		});
		let runner = runner_with_handlers("before_agent_start", vec![handler]);
		let errors: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
		let errors_listener = Arc::clone(&errors);
		runner.on_error(Arc::new(move |error| {
			errors_listener.lock().unwrap().push(error.clone());
		}));
		let (messages, options) = runner
			.emit_before_agent_start(json!({}), json!({"cwd": "/p", "forceSystemPrompt": null}))
			.await
			.unwrap();
		assert!(messages.is_empty());
		assert_eq!(options["forceSystemPrompt"], Value::Null);
		// event.systemPrompt omitted (render error) — TS throws at read.
		let seen = seen.lock().unwrap().clone();
		assert_eq!(seen[0], "event:false");
		assert_eq!(seen[1], "call!Cannot read properties of null (reading 'filter')");
		let errors = errors.lock().unwrap().clone();
		assert_eq!(errors.len(), 1);
		assert_eq!(errors[0]["event"], json!("before_agent_start"));
		assert_eq!(errors[0]["error"], json!("boom"));
	}

	#[tokio::test]
	async fn before_agent_start_event_and_ctx_render() {
		let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
		let seen_handler = Arc::clone(&seen);
		let handler: ExtensionHandler = Arc::new(move |event, ctx| {
			let seen = Arc::clone(&seen_handler);
			Box::pin(async move {
				let prompt = event["systemPrompt"].as_str().unwrap_or("<none>").to_string();
				seen.lock().unwrap().push(format!("event:{prompt}"));
				let via_ctx = ctx.call("getSystemPrompt").unwrap();
				seen.lock().unwrap().push(format!("ctx:{}", via_ctx.as_str().unwrap_or("")));
				Ok(HandlerOutcome { event, result: None })
			})
		});
		let runner = runner_with_handlers("before_agent_start", vec![handler]);
		runner.set_doc_paths(DocPaths {
			readme: "R".to_string(),
			docs: "D".to_string(),
			examples: "E".to_string(),
		});
		let (_, options) = runner
			.emit_before_agent_start(json!({"prompt": "p"}), json!({"cwd": "/p", "customPrompt": "CP"}))
			.await
			.unwrap();
		assert_eq!(options["customPrompt"], json!("CP"));
		let seen = seen.lock().unwrap().clone();
		assert_eq!(seen[0], "event:CP\\n\\n<cwd>\\n/p\\n</cwd>");
		assert_eq!(
			seen[1],
			"ctx:CP\\n\\n<cwd>\\n/p\\n</cwd>\\n\\n<docs>\\nPi documentation (read only when the user asks about pi itself, its SDK, extensions, themes, skills, or TUI):\\n- Main documentation: R\\n- Additional docs: D\\n- Examples: E (extensions, custom tools, SDK)"
		);
	}

''' + anchor
s=s.replace(anchor,new,1)
open(p,'w').write(s)
print("step E tests added")
