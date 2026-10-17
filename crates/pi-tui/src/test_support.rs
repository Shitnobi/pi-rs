//! Shared test support: a crate-wide lock serializing tests that mutate
//! process-global state (the kitty-protocol flag, env overrides).
//!
//! `cargo test` runs test functions in parallel across the whole binary,
//! while the upstream TS tests run sequentially under node:test — every
//! module whose tests flip `set_kitty_protocol_active` or env overrides
//! must hold this lock for the duration of those tests.

#![cfg(test)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};

use crate::component::TuiHost;

static GLOBAL_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the crate-wide global-state test lock.
pub fn global_test_lock() -> MutexGuard<'static, ()> {
    GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Minimal `TUI` test double mirroring the upstream `createTestTUI()`
/// (editor.test.ts:16-18: `new TuiMainScreen(new VirtualTerminal(cols, rows))`).
/// The only `TUI` members `Editor` consumes are `requestRender()` and
/// `terminal.rows`, so this double is observably identical for editor tests.
pub struct FakeTui {
    pub cols: usize,
    pub rows: usize,
    pub render_requests: usize,
}

impl FakeTui {
    pub fn new(cols: usize, rows: usize) -> Self {
        FakeTui {
            cols,
            rows,
            render_requests: 0,
        }
    }
}

impl TuiHost for FakeTui {
    fn request_render(&mut self) {
        self.render_requests += 1;
    }

    fn terminal_rows(&self) -> usize {
        self.rows
    }
}

/// Standard test TUI (`createTestTUI()` — 80×24) as a shared host handle.
pub fn create_test_tui() -> Rc<RefCell<dyn TuiHost>> {
    create_test_tui_sized(80, 24)
}

/// `createTestTUI(cols, rows)` as a shared host handle.
pub fn create_test_tui_sized(cols: usize, rows: usize) -> Rc<RefCell<dyn TuiHost>> {
    Rc::new(RefCell::new(FakeTui::new(cols, rows)))
}
