p='crates/pi-cli/src/extension_runner_harness.rs'
s=open(p).read()
s=s.replace('''	Ok(Arc::new(move |event: Value, ctx: CtxView| {''','''	Ok(Arc::new(move |event: Value, ctx: CtxView| {''')
old='''			if let Some(message) = throws {
				return Err(message);
			}
			Ok(returns)
		})
	}))'''
new='''			if let Some(message) = throws {
				return Err(message);
			}
			// Hand the (possibly mutated) event back: in-place edits model
			// TS handler mutation of the aliased event object.
			Ok(HandlerOutcome { event, result: returns })
		})
	}))'''
assert old in s; s=s.replace(old,new)
s=s.replace("use pi_session::agent_session::extension_runner::{", "use pi_session::agent_session::extension_runner::{HandlerOutcome, ",1)
open(p,'w').write(s)
print("harness step A done")
