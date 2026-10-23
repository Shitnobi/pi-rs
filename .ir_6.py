p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()
anchor='''	/// `bindCommandContext(actions?)` — `None` resets every handler to the'''
new='''	/// `withUIPrompt`-wrapped `ui.<kind>(...args)` (`select`/`confirm`/
	/// `input`/`editor`/`custom`); `Null` = undefined.
	pub async fn ui_prompt(
		&self,
		kind: &str,
		title: Option<&str>,
		args: Vec<Value>,
	) -> Result<Value, String> {
		self.with_ui_prompt(kind, title, async { self.ui_invoke(kind, &args).await })
			.await
	}

	/// `ctx.ui.select(title, options, opts?)`.
	pub async fn ui_select(
		&self,
		title: &str,
		options: &[String],
		opts: Value,
	) -> Result<Value, String> {
		self.ui_prompt("select", Some(title), vec![json!(title), json!(options), opts])
			.await
	}

	/// `ctx.ui.confirm(title, message, opts?)`.
	pub async fn ui_confirm(
		&self,
		title: &str,
		message: &str,
		opts: Value,
	) -> Result<Value, String> {
		self.ui_prompt("confirm", Some(title), vec![json!(title), json!(message), opts])
			.await
	}

	/// `ctx.ui.input(title, placeholder?, opts?)`.
	pub async fn ui_input(
		&self,
		title: &str,
		placeholder: Option<&str>,
		opts: Value,
	) -> Result<Value, String> {
		self.ui_prompt(
			"input",
			Some(title),
			vec![json!(title), placeholder.map(|p| json!(p)).unwrap_or(Value::Null), opts],
		)
		.await
	}

	/// `ctx.ui.editor(title, prefill?)`.
	pub async fn ui_editor(
		&self,
		title: &str,
		prefill: Option<&str>,
	) -> Result<Value, String> {
		self.ui_prompt(
			"editor",
			Some(title),
			vec![json!(title), prefill.map(|p| json!(p)).unwrap_or(Value::Null)],
		)
		.await
	}

	/// `ctx.ui.custom(factory, options?)` — the factory is not modelable;
	/// `args` carries the harness-shaped call.
	pub async fn ui_custom(&self, args: Vec<Value>) -> Result<Value, String> {
		self.ui_prompt("custom", None, args).await
	}

''' + anchor
assert anchor in s; s=s.replace(anchor,new,1)
open(p,'w').write(s)
print("item 6 methods")
