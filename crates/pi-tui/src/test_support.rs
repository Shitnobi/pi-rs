//! Shared test support: a crate-wide lock serializing tests that mutate
//! process-global state (the kitty-protocol flag, env overrides).
//!
//! `cargo test` runs test functions in parallel across the whole binary,
//! while the upstream TS tests run sequentially under node:test — every
//! module whose tests flip `set_kitty_protocol_active` or env overrides
//! must hold this lock for the duration of those tests.

#![cfg(test)]

use std::sync::{Mutex, MutexGuard};

static GLOBAL_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the crate-wide global-state test lock.
pub fn global_test_lock() -> MutexGuard<'static, ()> {
    GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
