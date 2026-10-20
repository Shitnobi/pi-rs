//! Port of `packages/coding-agent/src/core/event-bus.ts`.
//!
//! The TypeScript original wraps a `node:events` EventEmitter:
//! - `on` registers a handler wrapped in an async "safe handler" that swallows
//!   errors and reports them via `console.error(`Event handler error (${channel}):`, err)`.
//! - `on` returns an unsubscribe function (plain `emitter.off`).
//! - `clear` is `removeAllListeners()` across all channels.
//! - `emit` invokes the handlers registered for a channel synchronously, in
//!   registration order, over a snapshot of the listener list (Node's `emit`
//!   copies the list before calling, so handlers unsubscribed *during* an emit
//!   by an earlier handler are still invoked for that emit).
//!
//! Divergence notes (1:1 within Rust's sync model):
//! - Handlers are synchronous `Fn(&Value) -> Result<(), String>`; the `Err`
//!   payload stands in for the JS thrown error and is rendered into the
//!   `console.error` message by the injected error sink (JS renders the error
//!   via `console.error`'s multi-argument inspection; here the caller supplies
//!   the string form).
//! - The error sink is injected instead of calling `console.error` directly;
//!   `create_event_bus()` uses a stderr sink matching the TS default.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// A registered event handler. `Err` mirrors a thrown error inside the TS
/// safe handler; the message is embedded in the
/// `Event handler error (${channel}):` report.
pub type EventHandler = Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;

/// Sink receiving handler-error reports (one call per caught error,
/// message = `Event handler error (${channel}): ${err}`).
pub type ErrorSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
struct EventBusInner {
	next_id: u64,
	/// channel -> handler ids in registration order.
	channels: HashMap<String, Vec<u64>>,
	handlers: HashMap<u64, EventHandler>,
}

/// Port of `EventBusController` (the `EventBus` view is the same object; the
/// alias below mirrors the TS interface split). Cloning shares the underlying
/// listener state, like copying the controller object in TS.
#[derive(Clone)]
pub struct EventBusController {
	inner: Arc<Mutex<EventBusInner>>,
	error_sink: ErrorSink,
}

/// The read/subscribe face of the bus (`emit`/`on`).
pub type EventBus = EventBusController;

/// Unsubscribe handle returned by [`EventBusController::on`], mirroring the
/// function returned by the TS `on`. Idempotent, like `emitter.off`.
pub struct Subscription {
	inner: Arc<Mutex<EventBusInner>>,
	channel: String,
	id: u64,
}

impl Subscription {
	/// Remove this listener. Calling more than once is a no-op.
	pub fn unsubscribe(&self) {
		let mut inner = self.inner.lock().unwrap();
		if let Some(ids) = inner.channels.get_mut(&self.channel) {
			ids.retain(|&id| id != self.id);
		}
		inner.handlers.remove(&self.id);
	}
}

/// Port of `createEventBus()` with the TS default error reporting
/// (`console.error` → stderr).
pub fn create_event_bus() -> EventBusController {
	create_event_bus_with_sink(Arc::new(|message| eprintln!("{message}")))
}

/// `createEventBus()` with an injected error sink (for tests / capture).
pub fn create_event_bus_with_sink(error_sink: ErrorSink) -> EventBusController {
	EventBusController {
		inner: Arc::new(Mutex::new(EventBusInner::default())),
		error_sink,
	}
}

impl EventBusController {
	/// Port of `emit(channel, data)`. Invokes the channel's handlers in
	/// registration order over a snapshot; handler errors are caught and
	/// reported to the error sink, and later handlers still run.
	pub fn emit(&self, channel: &str, data: &Value) {
		let handlers: Vec<EventHandler> = {
			let inner = self.inner.lock().unwrap();
			match inner.channels.get(channel) {
				Some(ids) => ids.iter().filter_map(|id| inner.handlers.get(id).cloned()).collect(),
				None => return,
			}
		};
		for handler in handlers {
			if let Err(err) = handler(data) {
				(self.error_sink)(&format!("Event handler error ({channel}): {err}"));
			}
		}
	}

	/// Port of `on(channel, handler)` — returns the unsubscribe handle.
	pub fn on<F>(&self, channel: &str, handler: F) -> Subscription
	where
		F: Fn(&Value) -> Result<(), String> + Send + Sync + 'static,
	{
		let mut inner = self.inner.lock().unwrap();
		let id = inner.next_id;
		inner.next_id += 1;
		inner.handlers.insert(id, Arc::new(handler));
		inner.channels.entry(channel.to_string()).or_default().push(id);
		Subscription {
			inner: Arc::clone(&self.inner),
			channel: channel.to_string(),
			id,
		}
	}

	/// Port of `clear()` — `removeAllListeners()` across all channels.
	pub fn clear(&self) {
		let mut inner = self.inner.lock().unwrap();
		inner.channels.clear();
		inner.handlers.clear();
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	type Log = Arc<Mutex<Vec<String>>>;

	fn recording_handler(seen: Log) -> impl Fn(&Value) -> Result<(), String> + Send + Sync + 'static {
		move |data: &Value| {
			seen.lock().unwrap().push(serde_json::to_string(data).unwrap());
			Ok(())
		}
	}

	#[test]
	fn emit_delivers_to_registered_handlers_in_order() {
		let bus = create_event_bus();
		let seen: Log = Arc::new(Mutex::new(Vec::new()));
		let s1 = Arc::clone(&seen);
		bus.on("evt", move |data: &Value| {
			s1.lock().unwrap().push(format!("first:{}", data));
			Ok(())
		});
		let s2 = Arc::clone(&seen);
		bus.on("evt", move |data: &Value| {
			s2.lock().unwrap().push(format!("second:{}", data));
			Ok(())
		});

		bus.emit("evt", &json!({"n": 1}));
		assert_eq!(
			seen.lock().unwrap().as_slice(),
			&["first:{\"n\":1}".to_string(), "second:{\"n\":1}".to_string()]
		);
	}

	#[test]
	fn emit_without_listeners_is_a_noop() {
		let bus = create_event_bus();
		bus.emit("nobody-listening", &json!(null));
	}

	#[test]
	fn unsubscribe_stops_delivery_and_is_idempotent() {
		let bus = create_event_bus();
		let seen: Log = Arc::new(Mutex::new(Vec::new()));
		let sub = bus.on("evt", recording_handler(Arc::clone(&seen)));
		bus.emit("evt", &json!(1));
		sub.unsubscribe();
		sub.unsubscribe(); // second call is a no-op, like emitter.off
		bus.emit("evt", &json!(2));
		assert_eq!(seen.lock().unwrap().as_slice(), &["1".to_string()]);
	}

	#[test]
	fn handler_errors_are_reported_and_do_not_stop_other_handlers() {
		let reports: Log = Arc::new(Mutex::new(Vec::new()));
		let sink_reports = Arc::clone(&reports);
		let bus = create_event_bus_with_sink(Arc::new(move |msg: &str| {
			sink_reports.lock().unwrap().push(msg.to_string());
		}));

		let seen: Log = Arc::new(Mutex::new(Vec::new()));
		bus.on("boom", move |_| Err("kaput".to_string()));
		bus.on("boom", recording_handler(Arc::clone(&seen)));

		bus.emit("boom", &json!(42));
		assert_eq!(
			reports.lock().unwrap().as_slice(),
			&["Event handler error (boom): kaput".to_string()]
		);
		assert_eq!(seen.lock().unwrap().as_slice(), &["42".to_string()]);
	}

	#[test]
	fn clear_removes_all_listeners_across_channels() {
		let bus = create_event_bus();
		let seen_a: Log = Arc::new(Mutex::new(Vec::new()));
		let seen_b: Log = Arc::new(Mutex::new(Vec::new()));
		bus.on("a", recording_handler(Arc::clone(&seen_a)));
		bus.on("b", recording_handler(Arc::clone(&seen_b)));
		bus.clear();
		bus.emit("a", &json!(1));
		bus.emit("b", &json!(2));
		assert!(seen_a.lock().unwrap().is_empty());
		assert!(seen_b.lock().unwrap().is_empty());
	}

	#[test]
	fn unsubscribing_another_handler_during_emit_still_invokes_it_this_emit() {
		// Node's emit snapshots the listener list; the TS bus inherits that.
		let bus = create_event_bus();
		let seen: Log = Arc::new(Mutex::new(Vec::new()));
		let target = Arc::new(bus.on("evt", recording_handler(Arc::clone(&seen))));
		let unsubscribe_target = Arc::clone(&target);
		bus.on("evt", move |_data: &Value| {
			unsubscribe_target.unsubscribe();
			Ok(())
		});

		bus.emit("evt", &json!(null));
		assert_eq!(
			seen.lock().unwrap().as_slice(),
			&["null".to_string()],
			"handler unsubscribed during emit must still run this emit"
		);

		bus.emit("evt", &json!(null));
		assert_eq!(seen.lock().unwrap().len(), 1, "and must not run on the next emit");
	}

	#[test]
	fn cloned_controllers_share_state() {
		let bus = create_event_bus();
		let twin = bus.clone();
		let seen: Log = Arc::new(Mutex::new(Vec::new()));
		bus.on("evt", recording_handler(Arc::clone(&seen)));
		twin.emit("evt", &json!("x"));
		assert_eq!(seen.lock().unwrap().as_slice(), &["\"x\"".to_string()]);
		twin.clear();
		bus.emit("evt", &json!("y"));
		assert_eq!(seen.lock().unwrap().len(), 1);
	}
}
