//! Virtual clock for the T5 renderer (no TS counterpart — port machinery).
//!
//! The TS renderer reads wall-clock time (`performance.now`, `Date.now`) and
//! schedules with `setTimeout`/`setInterval`/`process.nextTick`. The Rust port
//! is pure-std: every T5 module reads [`now_ms`] instead, and the runtime
//! owner (tests, the parity harness, or the real app loop) advances the clock
//! explicitly with [`advance_ms`]. Timers become deadline state that the
//! owners fire when [`now_ms`] passes the deadline — see
//! `TuiBase`'s render-scheduling pump and `AltScreenFlashContainer`'s
//! per-entry deadlines for the pattern.
//!
//! The clock is **thread-local**: `cargo test` runs test functions on
//! parallel threads, so each test owns an independent clock (the lesson from
//! the global-keybindings race in T4).

use std::cell::Cell;

thread_local! {
    static CLOCK_MS: Cell<f64> = const { Cell::new(0.0) };
}

/// `performance.now()` / `Date.now()` — milliseconds since the thread's
/// clock origin (epoch 0 at thread start / after [`reset_for_test`]).
pub fn now_ms() -> f64 {
    CLOCK_MS.with(|c| c.get())
}

/// Advance the clock by `ms` milliseconds. Fires nothing by itself — owners
/// poll deadlines after advancing.
pub fn advance_ms(ms: f64) {
    CLOCK_MS.with(|c| c.set(c.get() + ms.max(0.0)));
}

/// Overwrite the clock (test setup / reset).
pub fn set_now_ms(ms: f64) {
    CLOCK_MS.with(|c| c.set(ms));
}

/// Reset to the origin. Call in test setup when a fresh timeline is needed.
pub fn reset_for_test() {
    CLOCK_MS.with(|c| c.set(0.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_advances_and_resets() {
        reset_for_test();
        assert_eq!(now_ms(), 0.0);
        advance_ms(16.0);
        assert_eq!(now_ms(), 16.0);
        advance_ms(-5.0); // clamped: time never flows backwards
        assert_eq!(now_ms(), 16.0);
        set_now_ms(1000.5);
        assert_eq!(now_ms(), 1000.5);
        reset_for_test();
        assert_eq!(now_ms(), 0.0);
    }
}
