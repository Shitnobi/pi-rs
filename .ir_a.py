p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()

# 1. type: handlers hand the (possibly mutated) event back
old='''/// One event handler: `(event, ctx) -> Ok(Some(result)) | Ok(None)
/// (undefined) | Err(thrown)`.
pub type ExtensionHandler =
	Arc<dyn Fn(Value, CtxView) -> BoxFuture<'static, Result<Option<Value>, String>> + Send + Sync>;'''
new='''/// What a handler hands back: its event (handlers own the copy, so
/// in-place edits — `event.messages`, `event.headers`,
/// `event.systemPromptOptions` — are read back by the emitter, exactly
/// like TS's aliased event object) plus the `handlerResult`
/// (`Ok(None)` = undefined; `Err` = throw).
pub struct HandlerOutcome {
	pub event: Value,
	pub result: Option<Value>,
}

/// One event handler: `(event, ctx) -> Ok(HandlerOutcome) |
/// Err(thrown)`.
pub type ExtensionHandler = Arc<
	dyn Fn(Value, CtxView) -> BoxFuture<'static, Result<HandlerOutcome, String>> + Send + Sync,
>;'''
assert old in s; s=s.replace(old,new)

# 2. generic emit call site
old='''				match handler(event.clone(), ctx.clone()).await {
					Ok(handler_result) => {
						if is_session_before && handler_result.as_ref().is_some_and(js_truthy) {
							result = handler_result;'''
new='''				match handler(event.clone(), ctx.clone()).await {
					Ok(HandlerOutcome { result: handler_result, .. }) => {
						if is_session_before && handler_result.as_ref().is_some_and(js_truthy) {
							result = handler_result;'''
assert old in s; s=s.replace(old,new)

# 3. message_end call site
old='''				match handler(current_event, ctx.clone()).await {
					Ok(handler_result) => {
						let replacement = handler_result'''
new='''				match handler(current_event, ctx.clone()).await {
					Ok(HandlerOutcome { result: handler_result, .. }) => {
						let replacement = handler_result'''
assert old in s; s=s.replace(old,new)

# 4. project_trust call site (row-1 result use is `handler_result.as_ref()`)
old='''			match handler(event.clone(), ctx.clone()).await {
				Ok(handler_result) => {
					// TS reads `handlerResult.trusted` inside the try: an'''
new='''			match handler(event.clone(), ctx.clone()).await {
				Ok(HandlerOutcome { result: handler_result, .. }) => {
					// TS reads `handlerResult.trusted` inside the try: an'''
assert old in s; s=s.replace(old,new)

# 5. test closure
old='''				let handler: ExtensionHandler = Arc::new(|_event, _ctx| {
					Box::pin(async { Ok(Some(json!({"cancel": true}))) })
				});'''
new='''				let handler: ExtensionHandler = Arc::new(|event, _ctx| {
					Box::pin(async move { Ok(HandlerOutcome { event, result: Some(json!({"cancel": true})) }) })
				});'''
assert old in s; s=s.replace(old,new)

open(p,'w').write(s)
print("step A done")
