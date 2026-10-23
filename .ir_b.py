p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()

# imports
old='''use super::ExtensionHost;'''
new='''use super::system_prompt::{build_system_prompt, normalize_options_raw, DocPaths};
use super::ExtensionHost;'''
assert old in s; s=s.replace(old,new)

# module doc divergence line
old='''//! - Handlers receive a cloned event snapshot per invocation (TS passes the
//!   same live object; the parity scripts never mutate it) and a `CtxView`
//!   standing in for the lazy `ExtensionContext` getters.'''
new='''//! - Handlers receive their own event copy per invocation (TS passes the
//!   same live object) and hand it back in [`HandlerOutcome`]; emitters
//!   read in-place edits back, so mutation semantics match the TS. The
//!   generic `emit` still ignores event edits (no TS emitter reads them).'''
assert old in s; s=s.replace(old,new)

# CtxView variant
old='''pub enum CtxView {
	Runner(Arc<RunnerShared>),
	Raw(Value),
}'''
new='''pub enum CtxView {
	Runner(Arc<RunnerShared>),
	Raw(Value),
	/// `emitBeforeAgentStart`'s ctx: the `createContext()` descriptors plus
	/// the injected `getSystemPrompt` getter (`assertActive` + lazy render
	/// against the live options).
	BeforeAgentStart {
		shared: Arc<RunnerShared>,
		renderer: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
	},
}'''
assert old in s; s=s.replace(old,new)

# read/call arms
old='''		match self {
			CtxView::Raw(value) => Ok(value.get(field).cloned().unwrap_or(Value::Null)),
			CtxView::Runner(shared) => ExtensionRunner { shared: Arc::clone(shared) }.ctx_read(field),
		}
	}'''
new='''		match self {
			CtxView::Raw(value) => Ok(value.get(field).cloned().unwrap_or(Value::Null)),
			CtxView::Runner(shared) => ExtensionRunner { shared: Arc::clone(shared) }.ctx_read(field),
			// `getSystemPrompt` is a function property: reading it yields a
			// function (JSON-undefined); the harness calls it via `call`.
			CtxView::BeforeAgentStart { shared, .. } => {
				if field == "getSystemPrompt" {
					return Ok(Value::Null);
				}
				ExtensionRunner { shared: Arc::clone(shared) }.ctx_read(field)
			}
		}
	}'''
assert old in s; s=s.replace(old,new)

old='''		match self {
			CtxView::Raw(_) => Err("not a function".to_string()),
			CtxView::Runner(shared) => ExtensionRunner { shared: Arc::clone(shared) }.ctx_call(method),
		}
	}'''
new='''		match self {
			CtxView::Raw(_) => Err("not a function".to_string()),
			CtxView::Runner(shared) => ExtensionRunner { shared: Arc::clone(shared) }.ctx_call(method),
			CtxView::BeforeAgentStart { shared, renderer } => {
				if method == "getSystemPrompt" {
					if let Some(message) = shared.stale_message.lock().unwrap().clone() {
						return Err(message);
					}
					return renderer().map(|prompt| json!(prompt));
				}
				ExtensionRunner { shared: Arc::clone(shared) }.ctx_call(method)
			}
		}
	}'''
assert old in s; s=s.replace(old,new)

# RunnerShared field
old='''	shortcut_diagnostics: Mutex<Vec<Value>>,
	model_registry: Option<Arc<dyn ModelRegistrySeam>>,
}'''
new='''	shortcut_diagnostics: Mutex<Vec<Value>>,
	model_registry: Option<Arc<dyn ModelRegistrySeam>>,
	/// Host paths for the default `docs` prompt section (TS resolves them
	/// from the package install; see [`ExtensionRunner::set_doc_paths`]).
	doc_paths: Mutex<DocPaths>,
}'''
assert old in s; s=s.replace(old,new)

old='''				shortcut_diagnostics: Mutex::new(Vec::new()),
				model_registry,
			}),'''
new='''				shortcut_diagnostics: Mutex::new(Vec::new()),
				model_registry,
				doc_paths: Mutex::new(DocPaths::default()),
			}),'''
assert old in s; s=s.replace(old,new)

open(p,'w').write(s)
print("step B+C done")
