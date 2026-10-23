p='crates/pi-cli/src/extension_runner_harness.rs'
s=open(p).read()

# CaseInner field
s=s.replace('''	next_error_id: std::sync::atomic::AtomicU64,
}''','''	next_error_id: std::sync::atomic::AtomicU64,
	engine: Mutex<Engine>,
}

/// Microtask engine (plan Addendum B): handler bodies (`Body`) and emit-loop
/// continuations (`Loop`) for fire-and-forget `mcp_servers_change` emits,
/// plus `Mark` steps pushed by awaited continuations (the load op).
#[derive(Default)]
struct Engine {
	queue: std::collections::VecDeque<Step>,
	jobs: Vec<Job>,
}

enum Step {
	Body(usize, usize),
	Loop(usize, usize),
	Mark(String),
}

struct Job {
	event: Value,
	handlers: Vec<(String, ExtensionHandler)>,
}

/// Turn the runner's deferred mcp jobs into engine jobs (handler snapshots
/// were taken at push time) and enqueue each emit start.
fn push_mcp_jobs(case: &Arc<CaseInner>) {
	let Some(runner) = case.runner.lock().unwrap().clone() else {
		return;
	};
	let jobs = runner.drain_mcp_jobs();
	let mut engine = case.engine.lock().unwrap();
	for job in jobs {
		let index = engine.jobs.len();
		let handlers: Vec<(String, ExtensionHandler)> = job
			.handlers
			.into_iter()
			.flat_map(|(path, handlers)| {
				handlers.into_iter().map(move |handler| (path.clone(), handler))
			})
			.collect();
		engine.jobs.push(Job { event: job.event, handlers });
		engine.queue.push_back(Step::Body(index, 0));
	}
}

/// Drain the microtask queue to exhaustion (the TS `setImmediate` settle).
async fn drain_engine(case: &Arc<CaseInner>) -> Result<(), String> {
	loop {
		let step = { case.engine.lock().unwrap().queue.pop_front() };
		let Some(step) = step else {
			break;
		};
		match step {
			Step::Mark(line) => case.log.lock().unwrap().push(line),
			Step::Body(job, index) => {
				let (event, path, handler) = {
					let engine = case.engine.lock().unwrap();
					let job = &engine.jobs[job];
					(
						job.event.clone(),
						job.handlers[index].0.clone(),
						job.handlers[index].1.clone(),
					)
				};
				let runner = case.runner.lock().unwrap().clone().ok_or("runner not created")?;
				if let Err(error) = handler(event.clone(), runner.ctx()).await {
					runner.emit_error(&json!({
						"extensionPath": path,
						"event": event.get("type").cloned().unwrap_or(Value::Null),
						"error": error,
					}));
				}
				case.engine.lock().unwrap().queue.push_back(Step::Loop(job, index));
			}
			Step::Loop(job, index) => {
				let len = case.engine.lock().unwrap().jobs[job].handlers.len();
				if index + 1 < len {
					case.engine.lock().unwrap().queue.push_back(Step::Body(job, index + 1));
				}
			}
		}
	}
	Ok(())
}''')

# init the field
s=s.replace('''		next_error_id: std::sync::atomic::AtomicU64::new(0),
	});''','''		next_error_id: std::sync::atomic::AtomicU64::new(0),
		engine: Mutex::new(Engine::default()),
	});''')

# load op: push jobs then a Mark
old='''				match result {
					Ok(extension) => {
						case.log.lock().unwrap().push(format!("load:{}:ok", path));
						case.extensions.lock().unwrap().push(extension);
					}
					Err(message) => case.log.lock().unwrap().push(format!("load:{}:thrown:{}", path, message)),
				}'''
new='''				match result {
					Ok(extension) => {
						// The load-resolution continuation: mcp jobs opened at
						// commit land BEFORE the mark (Addendum B, Trace 2).
						push_mcp_jobs(&case);
						case.engine.lock().unwrap().queue.push_back(Step::Mark(format!("load:{}:ok", path)));
						case.extensions.lock().unwrap().push(extension);
					}
					Err(message) => {
						push_mcp_jobs(&case);
						case.engine
							.lock()
							.unwrap()
							.queue
							.push_back(Step::Mark(format!("load:{}:thrown:{}", path, message)));
					}
				}'''
assert old in s; s=s.replace(old,new)

# per-op settle: engine instead of full drain
old='''		// Drain queued `mcp_servers_change` emits (the TS settles the void
		// emit on a microtask before the next op).
		if let Some(runner) = case.runner.lock().unwrap().clone() {
			runner.drain_mcp_events().await;
		}'''
new='''		// Settle the microtask queue (the TS `setImmediate` per op): pending
		// mcp emit starts interleave handler-by-handler, and op marks fire in
		// push order.
		push_mcp_jobs(&case);
		drain_engine(&case).await?;'''
assert old in s; s=s.replace(old,new)
open(p,'w').write(s)
print("harness engine")
