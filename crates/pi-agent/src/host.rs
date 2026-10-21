//! Host seams the agent layer needs injected (plan §4): clocks, sleeping,
//! and abort handles. No ambient state — everything is passed in, so
//! parity tests are bit-deterministic.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::types::BoxFuture;

// ============================================================================
// Clock
// ============================================================================

/// Wall/monotonic clock seam (plan §4). `date_now_ms` is `Date.now()`,
/// `perf_now_ms` is `performance.now()` (monotonic, used for tool
/// durations).
pub trait Clock: Send + Sync {
    fn date_now_ms(&self) -> f64;
    fn perf_now_ms(&self) -> f64;
}

/// Scripted clock for tests: two independent sequences (`Date.now()` and
/// `performance.now()` calls pop their own sequence). Each call returns the
/// next value; once a sequence is exhausted its last value repeats. An
/// empty sequence yields `0.0`.
#[derive(Debug)]
pub struct FixedClock {
    date_seq: Vec<f64>,
    perf_seq: Vec<f64>,
    date_i: AtomicUsize,
    perf_i: AtomicUsize,
}

impl FixedClock {
    pub fn new(date_seq: Vec<f64>, perf_seq: Vec<f64>) -> Self {
        FixedClock {
            date_seq,
            perf_seq,
            date_i: AtomicUsize::new(0),
            perf_i: AtomicUsize::new(0),
        }
    }

    fn next(seq: &[f64], i: &AtomicUsize) -> f64 {
        let n = i.fetch_add(1, Ordering::SeqCst);
        seq.get(n).or(seq.last()).copied().unwrap_or(0.0)
    }
}

impl Clock for FixedClock {
    fn date_now_ms(&self) -> f64 {
        FixedClock::next(&self.date_seq, &self.date_i)
    }

    fn perf_now_ms(&self) -> f64 {
        FixedClock::next(&self.perf_seq, &self.perf_i)
    }
}

// ============================================================================
// AbortHandle
// ============================================================================

/// The agent's abort signal (TS `AbortSignal` equivalent). A shared flag
/// plus a notify, checked at the exact TS check points; `select!`-style
/// races only inside `Sleeper::sleep` and provider-stream iteration.
#[derive(Debug, Clone, Default)]
pub struct AbortHandle {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

/// Error produced by abortable host operations (sleep, streams).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aborted;

impl std::fmt::Display for Aborted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("aborted")
    }
}

impl std::error::Error for Aborted {}

impl AbortHandle {
    pub fn new() -> Self {
        AbortHandle::default()
    }

    /// TS `AbortController#abort()`.
    pub fn abort(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// TS `AbortSignal#aborted`.
    pub fn is_aborted(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Completes once the handle is aborted. Registration happens before
    /// the flag check (`Notified::enable`), so an abort racing with a new
    /// waiter is never lost.
    pub async fn aborted(&self) {
        let mut notified = std::pin::pin!(self.notify.notified());
        notified.as_mut().enable();
        if self.is_aborted() {
            return;
        }
        notified.await;
    }
}

// ============================================================================
// Sleeper
// ============================================================================

/// Abortable sleep seam (plan §4; retry backoff, bash, compaction). An
/// already-aborted handle rejects immediately, like a JS `AbortSignal`.
pub trait Sleeper: Send + Sync {
    fn sleep<'a>(&'a self, ms: u64, signal: &'a AbortHandle) -> BoxFuture<'a, Result<(), Aborted>>;
}

/// Real sleeper over `tokio::time` (single current-thread runtime, plan §3).
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    fn sleep<'a>(&'a self, ms: u64, signal: &'a AbortHandle) -> BoxFuture<'a, Result<(), Aborted>> {
        Box::pin(async move {
            if signal.is_aborted() {
                return Err(Aborted);
            }
            tokio::select! {
                biased;
                _ = signal.aborted() => Err(Aborted),
                _ = tokio::time::sleep(Duration::from_millis(ms)) => Ok(()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_pops_sequences_independently_and_repeats_last() {
        let clock = FixedClock::new(vec![100.0, 200.0], vec![1.0, 2.0, 3.0]);
        assert_eq!(clock.date_now_ms(), 100.0);
        assert_eq!(clock.perf_now_ms(), 1.0);
        assert_eq!(clock.date_now_ms(), 200.0);
        assert_eq!(clock.perf_now_ms(), 2.0);
        assert_eq!(clock.date_now_ms(), 200.0, "last date value repeats");
        assert_eq!(clock.perf_now_ms(), 3.0);
        assert_eq!(clock.perf_now_ms(), 3.0, "last perf value repeats");
    }

    #[test]
    fn fixed_clock_empty_sequence_yields_zero() {
        let clock = FixedClock::new(vec![], vec![5.0]);
        assert_eq!(clock.date_now_ms(), 0.0);
        assert_eq!(clock.perf_now_ms(), 5.0);
    }

    #[test]
    fn abort_handle_flag_and_waiters() {
        let handle = AbortHandle::new();
        assert!(!handle.is_aborted());
        let waiter = handle.clone();
        // Before abort: the waiter must stay pending (only the timeout arm
        // fires on the current-thread runtime).
        let outcome = crate::run_current_thread(async move {
            tokio::select! {
                _ = waiter.aborted() => "aborted",
                _ = tokio::time::sleep(Duration::from_millis(20)) => "timeout",
            }
        });
        assert_eq!(outcome, "timeout");
        handle.abort();
        assert!(handle.is_aborted());
        // After abort: a waiter resolves immediately.
        crate::run_current_thread(handle.aborted());
    }

    #[test]
    fn aborted_handle_waiter_returns_immediately() {
        let handle = AbortHandle::new();
        handle.abort();
        crate::run_current_thread(handle.aborted());
    }

    #[test]
    fn sleeper_completes_after_ms() {
        let sleeper = TokioSleeper;
        let handle = AbortHandle::new();
        let start = std::time::Instant::now();
        let result = crate::run_current_thread(sleeper.sleep(10, &handle));
        assert_eq!(result, Ok(()));
        assert!(start.elapsed() >= Duration::from_millis(10));
    }

    #[test]
    fn sleeper_rejects_when_aborted_first() {
        let sleeper = TokioSleeper;
        let handle = AbortHandle::new();
        handle.abort();
        let result = crate::run_current_thread(sleeper.sleep(10_000, &handle));
        assert_eq!(result, Err(Aborted));
    }

    #[test]
    fn sleeper_aborts_mid_sleep() {
        let sleeper = TokioSleeper;
        let handle = AbortHandle::new();
        let aborter = handle.clone();
        let jh = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            aborter.abort();
        });
        let result = crate::run_current_thread(sleeper.sleep(60_000, &handle));
        jh.join().expect("aborter thread");
        assert_eq!(result, Err(Aborted));
    }
}
