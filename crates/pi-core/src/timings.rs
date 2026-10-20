//! Port of `packages/coding-agent/src/core/timings.ts` — central timing
//! instrumentation for startup profiling.
//!
//! TS semantics kept 1:1:
//! - `ENABLED` is captured once from `process.env.PI_TIMING === "1"`; here the
//!   flag is injected at construction (the crate never reads `std::env`).
//! - `Date.now()` is injectable so tests are deterministic.
//! - `time()` reads the clock *before* the implicit `resetTimings()` of a
//!   missing namespace, exactly like the TS ordering, so a clock that advances
//!   between the two reads can produce `ms <= 0` — the printer filters those.
//! - `printTimings` output mirrors the `console.error` stream byte-for-byte:
//!   each `console.error(msg)` line contributes `msg + "\n"`, and the final
//!   line's message already ends in `\n` (so the group ends with a blank line).
//!   Here the output is returned as a `String` instead of being written to
//!   stderr.
//! - Namespaces print in first-touch (insertion) order, like the TS `Map`.

use std::time::{SystemTime, UNIX_EPOCH};

/// Port of `TimingLabel = "main" | "extensions"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingLabel {
	Main,
	Extensions,
}

impl TimingLabel {
	pub fn as_str(self) -> &'static str {
		match self {
			TimingLabel::Main => "main",
			TimingLabel::Extensions => "extensions",
		}
	}
}

/// One `{ label, ms }` timing record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingEntry {
	pub label: String,
	pub ms: i64,
}

#[derive(Debug, Clone)]
struct TimingNamespace {
	timings: Vec<TimingEntry>,
	last_time: i64,
}

/// Clock returning `Date.now()`-style epoch milliseconds.
pub type Clock = std::sync::Arc<dyn Fn() -> i64 + Send + Sync>;

/// The ported timing state machine. The TS module keeps a module-level
/// singleton; here the caller owns the instance (e.g. one per process).
#[derive(Clone)]
pub struct Timings {
	enabled: bool,
	now_ms: Clock,
	/// First-touch ordered namespaces (TS `Map` insertion order).
	namespaces: Vec<(TimingLabel, TimingNamespace)>,
}

impl Timings {
	/// Port with the enabled flag injected (`PI_TIMING === "1"` in TS) and the
	/// real wall clock.
	pub fn new(enabled: bool) -> Self {
		Self::with_clock(
			enabled,
			std::sync::Arc::new(|| {
				SystemTime::now()
					.duration_since(UNIX_EPOCH)
					.map(|d| d.as_millis() as i64)
					.unwrap_or(0)
			}),
		)
	}

	/// Constructor with an injected clock, for deterministic tests.
	pub fn with_clock(enabled: bool, now_ms: Clock) -> Self {
		Timings {
			enabled,
			now_ms,
			namespaces: Vec::new(),
		}
	}

	pub fn is_enabled(&self) -> bool {
		self.enabled
	}

	fn namespace_mut(&mut self, namespace: TimingLabel) -> Option<&mut TimingNamespace> {
		self.namespaces.iter_mut().find(|(n, _)| *n == namespace).map(|(_, ns)| ns)
	}

	/// Port of `resetTimings(namespace = "main")`. No-op when disabled.
	/// Re-setting an existing namespace preserves its position (TS `Map.set`).
	pub fn reset_timings(&mut self, namespace: TimingLabel) {
		if !self.enabled {
			return;
		}
		let last_time = (self.now_ms)();
		if let Some((_, ns)) = self.namespaces.iter_mut().find(|(n, _)| *n == namespace) {
			ns.timings.clear();
			ns.last_time = last_time;
		} else {
			self.namespaces.push((namespace, TimingNamespace { timings: Vec::new(), last_time }));
		}
	}

	/// Port of `time(label, namespace = "main")`. No-op when disabled.
	/// A missing namespace is implicitly reset first (which reads the clock a
	/// second time in TS — mirrored here).
	pub fn time(&mut self, label: &str, namespace: TimingLabel) {
		if !self.enabled {
			return;
		}
		let now = (self.now_ms)();
		if !self.namespaces.iter().any(|(n, _)| *n == namespace) {
			self.reset_timings(namespace);
		}
		let ns = self.namespace_mut(namespace).expect("namespace just ensured");
		ns.timings.push(TimingEntry { label: label.to_string(), ms: now - ns.last_time });
		ns.last_time = now;
	}

	/// Port of `printTimings()`. Returns the exact bytes `console.error` would
	/// have written (empty string when disabled or when every group is empty).
	pub fn print_timings(&self) -> String {
		let mut out = String::new();
		if !self.enabled {
			return out;
		}
		for (namespace, ns) in &self.namespaces {
			print_timing_group(&mut out, &format!("Startup Timings: {}", namespace.as_str()), &ns.timings);
		}
		out
	}
}

/// Port of `printTimingGroup(title, timings)`.
fn print_timing_group(out: &mut String, title: &str, timings: &[TimingEntry]) {
	let printable: Vec<&TimingEntry> = timings.iter().filter(|t| t.ms >= 0).collect();
	if printable.is_empty() {
		return;
	}
	out.push_str(&format!("\n--- {title} ---\n"));
	for t in &printable {
		out.push_str(&format!("  {}: {}ms\n", t.label, t.ms));
	}
	let total: i64 = printable.iter().map(|t| t.ms).sum();
	out.push_str(&format!("  TOTAL: {total}ms\n"));
	// console.error(`${"-".repeat(title.length + 8)}\n`): the message ends with
	// "\n" and console.error adds another, producing a blank line after the rule.
	// `title.length` is a UTF-16 code-unit count; titles here are ASCII.
	let rule = "-".repeat(title.chars().count() + 8);
	out.push_str(&rule);
	out.push('\n');
	out.push('\n');
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::{Arc, Mutex};

	/// Clock stepping by `step` ms on every call, starting at `start`.
	fn stepping_clock(start: i64, step: i64) -> (Clock, Arc<Mutex<i64>>) {
		let now = Arc::new(Mutex::new(start));
		let tick = Arc::clone(&now);
		(Arc::new(move || {
			let mut n = tick.lock().unwrap();
			*n += step;
			*n
		}), now)
	}

	#[test]
	fn disabled_timings_are_noops() {
		let mut timings = Timings::with_clock(false, Arc::new(|| 1000));
		timings.reset_timings(TimingLabel::Main);
		timings.time("startup", TimingLabel::Main);
		timings.time("extensions", TimingLabel::Extensions);
		assert!(!timings.is_enabled());
		assert_eq!(timings.print_timings(), "");
	}

	#[test]
	fn time_records_elapsed_ms_since_last_mark() {
		let (clock, _now) = stepping_clock(1000, 10);
		let mut timings = Timings::with_clock(true, clock);
		timings.reset_timings(TimingLabel::Main); // last_time = 1010
		timings.time("a", TimingLabel::Main); // now = 1020, ms = 10
		timings.time("b", TimingLabel::Main); // now = 1030, ms = 10
		let expected = "\n--- Startup Timings: main ---\n  \
			a: 10ms\n  b: 10ms\n  TOTAL: 20ms\n\
			-----------------------------\n\n";
		assert_eq!(timings.print_timings(), expected);
	}

	#[test]
	fn implicit_reset_of_missing_namespace() {
		// TS `time()` reads `now` first, then resets the missing namespace with a
		// fresh `Date.now()`; with an advancing clock the first entry can be <= 0
		// and is filtered from the printed group.
		let (clock, _now) = stepping_clock(1000, 10);
		let mut timings = Timings::with_clock(true, clock);
		timings.time("only", TimingLabel::Extensions); // now = 1010, reset last_time = 1020 → ms = -10
		assert_eq!(
			timings.print_timings(),
			"",
			"group with only non-positive timings is omitted entirely"
		);
	}

	#[test]
	fn namespaces_print_in_first_touch_order() {
		// Constant clock: every timing lands at ms = 0.
		let mut timings = Timings::with_clock(true, Arc::new(|| 1000));
		timings.time("m1", TimingLabel::Main); // touches main first
		timings.time("e1", TimingLabel::Extensions);
		timings.time("m2", TimingLabel::Main);

		let out = timings.print_timings();
		let main_pos = out.find("Startup Timings: main").unwrap();
		let ext_pos = out.find("Startup Timings: extensions").unwrap();
		assert!(main_pos < ext_pos);

		let expected_main = "\n--- Startup Timings: main ---\n  m1: 0ms\n  m2: 0ms\n  TOTAL: 0ms\n\
			-----------------------------\n\n";
		let expected_ext = "\n--- Startup Timings: extensions ---\n  e1: 0ms\n  TOTAL: 0ms\n\
			-----------------------------------\n\n";
		assert_eq!(out, format!("{expected_main}{expected_ext}"));
	}

	#[test]
	fn reset_clears_entries_and_preserves_namespace_position() {
		let mut timings = Timings::with_clock(true, Arc::new(|| 1000));
		timings.time("m1", TimingLabel::Main);
		timings.time("e1", TimingLabel::Extensions);
		timings.reset_timings(TimingLabel::Main);
		timings.time("m2", TimingLabel::Main);

		let out = timings.print_timings();
		let main_pos = out.find("Startup Timings: main").unwrap();
		let ext_pos = out.find("Startup Timings: extensions").unwrap();
		assert!(main_pos < ext_pos, "Map.set keeps the original insertion position");
		assert!(!out.contains("m1:"));
		assert!(out.contains("m2:"));
		assert!(out.contains("e1:"));
	}

	#[test]
	fn negative_timings_are_filtered_but_positives_print() {
		let (clock, now) = stepping_clock(1000, 0);
		let mut timings = Timings::with_clock(true, clock);
		timings.time("a", TimingLabel::Main); // ms = 0
		// Wind the clock backwards to force a negative entry.
		*now.lock().unwrap() = 500;
		timings.time("bad", TimingLabel::Main); // ms < 0
		*now.lock().unwrap() = 600;
		timings.time("c", TimingLabel::Main); // ms > 0

		let out = timings.print_timings();
		assert!(!out.contains("bad:"));
		assert!(out.contains("a: 0ms"));
		assert!(out.contains("c: 100ms"));
	}
}
