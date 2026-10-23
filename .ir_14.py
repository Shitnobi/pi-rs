p='crates/pi-session/src/agent_session/extension_runner.rs'
s=open(p).read()

# job type + queue
s=s.replace('''	mcp_event_queue: Mutex<Vec<Value>>,''','''	mcp_event_queue: Mutex<Vec<McpEventJob>>,''')

s=s.replace('''/// TS `ExtensionRunner` — the tranche-1 core slice.''','''/// One deferred `mcp_servers_change` emit: the event plus the handler
/// snapshot taken at push time (a handler registered later in the same op
/// does not observe the mutation).
pub struct McpEventJob {
	pub event: Value,
	pub handlers: Vec<(String, Vec<ExtensionHandler>)>,
}

/// TS `ExtensionRunner` — the tranche-1 core slice.''')

s=s.replace('''				shared
					.mcp_event_queue
					.lock()
					.unwrap()
					.push(json!({ "type": "mcp_servers_change", "servers": event_servers }));''','''				let runner = ExtensionRunner { shared: Arc::clone(&shared) };
				let job = McpEventJob {
					event: json!({ "type": "mcp_servers_change", "servers": event_servers }),
					handlers: runner.snapshot_handlers("mcp_servers_change"),
				};
				shared.mcp_event_queue.lock().unwrap().push(job);''')

# drain_mcp_events over jobs (full emit, compat) + drain_mcp_jobs
old = s[s.index('	pub async fn drain_mcp_events(&self) {'):]
old = old[:old.index('\n\t}\n')+4]
new = '''	pub async fn drain_mcp_events(&self) {
		for job in self.drain_mcp_jobs() {
			self.emit(job.event).await;
		}
	}

	/// Hand the deferred jobs to the host (the parity harness schedules the
	/// handler microtasks itself — see the plan's Addendum B).
	pub fn drain_mcp_jobs(&self) -> Vec<McpEventJob> {
		std::mem::take(&mut *self.shared.mcp_event_queue.lock().unwrap())
	}

	/// The `createContext()` view (used by the host for stepped emits).
	pub fn ctx(&self) -> CtxView {
		CtxView::Runner(Arc::clone(&self.shared))
	}
'''
s=s.replace(old,new)
open(p,'w').write(s)
print("runner jobs")
