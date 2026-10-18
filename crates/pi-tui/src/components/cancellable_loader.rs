//! Port of `packages/tui/src/components/cancellable-loader.ts` —
//! `CancellableLoader`: a [`Loader`] that can be cancelled with Escape.
//!
//! Divergences from TS:
//! - TS `CancellableLoader extends Loader`; Rust uses composition
//!   (`loader: Loader`) with delegating methods, since the crate has no
//!   class inheritance.
//! - `AbortController`/`AbortSignal` have no std equivalent; the signal is
//!   modeled by the `aborted` flag ([`CancellableLoader::aborted`]).
//! - `onAbort?: () => void` is a settable `Option<Box<dyn FnMut()>>` field.

use crate::component::Component;
use crate::components::loader::{Loader, LoaderIndicatorOptions};
use crate::keybindings::get_keybindings;

/// Port of `CancellableLoader` (cancellable-loader.ts:16-42) — a loader
/// that can be cancelled with Escape.
///
/// ```text
/// const loader = new CancellableLoader(tui, cyan, dim, "Working...");
/// loader.onAbort = () => done(null);
/// doWork(loader.signal).then(done);
/// ```
pub struct CancellableLoader {
    /// The wrapped `Loader` (TS inheritance).
    pub loader: Loader,
    /// Models the `AbortController`'s aborted state.
    aborted: bool,
    /// Called when user presses Escape (TS public field `onAbort`).
    pub on_abort: Option<Box<dyn FnMut()>>,
}

impl CancellableLoader {
    /// `constructor` — same signature as [`Loader::new`] (the TS class
    /// inherits the Loader constructor).
    pub fn new(
        request_render: Option<Box<dyn Fn()>>,
        spinner_color_fn: Box<dyn Fn(&str) -> String>,
        message_color_fn: Box<dyn Fn(&str) -> String>,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        CancellableLoader {
            loader: Loader::new(
                request_render,
                spinner_color_fn,
                message_color_fn,
                message,
                indicator,
            ),
            aborted: false,
            on_abort: None,
        }
    }

    /// `get aborted(): boolean` — whether the loader was aborted (the
    /// `AbortSignal`'s state).
    pub fn aborted(&self) -> bool {
        self.aborted
    }

    /// `handleInput(data)` (cancellable-loader.ts:31-37) — abort on the
    /// `tui.select.cancel` binding.
    pub fn handle_input_data(&mut self, data: &str) {
        let kb = get_keybindings();
        if kb.matches(data, "tui.select.cancel") {
            self.aborted = true;
            if let Some(on_abort) = &mut self.on_abort {
                on_abort();
            }
        }
    }

    /// `dispose()` (cancellable-loader.ts:39-41).
    pub fn dispose(&mut self) {
        self.loader.stop();
    }

    // --- Loader delegation (TS inheritance surface) -----------------------

    /// Delegates to [`Loader::start`].
    pub fn start(&mut self) {
        self.loader.start();
    }

    /// Delegates to [`Loader::stop`].
    pub fn stop(&mut self) {
        self.loader.stop();
    }

    /// Delegates to [`Loader::tick`] (the clock seam).
    pub fn tick(&mut self) {
        self.loader.tick();
    }

    /// Delegates to [`Loader::set_message`].
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.loader.set_message(message);
    }

    /// Delegates to [`Loader::set_indicator`].
    pub fn set_indicator(&mut self, indicator: Option<LoaderIndicatorOptions>) {
        self.loader.set_indicator(indicator);
    }
}

impl Component for CancellableLoader {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.loader.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input_data(data);
    }

    fn handles_input(&self) -> bool {
        // TS defines `handleInput`, so the method is present.
        true
    }

    fn invalidate(&mut self) {
        self.loader.invalidate();
    }
}

impl crate::components::loader::LoaderClock for CancellableLoader {
    fn loader_deadline(&self) -> Option<f64> {
        self.loader.loader_deadline()
    }
    fn fire_loader_tick_at(&mut self, deadline: f64) {
        let interval_ms = self.loader.interval_ms();
        self.loader.tick();
        self.loader.rearm_next_tick_at(deadline + interval_ms as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::visible_width;
    use std::cell::Cell;
    use std::rc::Rc;

    // Upstream has no dedicated cancellable-loader test file; faithful
    // tests per the port plan. `get_keybindings().matches` consults the
    // global kitty-protocol flag, so these tests hold the crate-wide lock.

    fn make() -> (CancellableLoader, Rc<Cell<usize>>) {
        let count = Rc::new(Cell::new(0));
        let counter = count.clone();
        let loader = CancellableLoader::new(
            Some(Box::new(move || counter.set(counter.get() + 1))),
            Box::new(|s: &str| format!("[{s}]") as String),
            Box::new(|s: &str| format!("({s})") as String),
            "Working...",
            None,
        );
        (loader, count)
    }

    #[test]
    fn escape_aborts_and_fires_on_abort() {
        let _guard = crate::test_support::global_test_lock();
        let (mut loader, _) = make();
        let fired = Rc::new(Cell::new(false));
        let flag = fired.clone();
        loader.on_abort = Some(Box::new(move || flag.set(true)));
        assert!(!loader.aborted());
        // "tui.select.cancel" defaults to escape/ctrl+c (keybindings.ts).
        loader.handle_input_data("\x1b");
        assert!(loader.aborted());
        assert!(fired.get());
        // Aborting again still fires the callback (AbortController.abort is
        // idempotent, but onAbort fires per matching keypress per TS).
        loader.handle_input_data("\x1b");
        assert!(loader.aborted());
    }

    #[test]
    fn ctrl_c_also_cancels() {
        let _guard = crate::test_support::global_test_lock();
        let (mut loader, _) = make();
        loader.handle_input_data("\x03");
        assert!(loader.aborted());
    }

    #[test]
    fn other_input_does_not_abort() {
        let _guard = crate::test_support::global_test_lock();
        let (mut loader, _) = make();
        let fired = Rc::new(Cell::new(false));
        let flag = fired.clone();
        loader.on_abort = Some(Box::new(move || flag.set(true)));
        loader.handle_input_data("a");
        loader.handle_input_data("\r");
        assert!(!loader.aborted());
        assert!(!fired.get());
    }

    #[test]
    fn renders_like_the_wrapped_loader() {
        let (mut loader, _) = make();
        let lines = loader.render(20);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "");
        assert!(lines[1].starts_with(" [⠋] (Working...)"));
        assert_eq!(visible_width(&lines[1]), 20);
    }

    #[test]
    fn forwards_loader_operations() {
        let (mut loader, _) = make();
        loader.set_message("Done");
        assert!(loader.render(20)[1].contains("(Done)"));
        loader.tick();
        loader.stop();
        // dispose() stops the animation clock.
        loader.dispose();
        let before = loader.render(20)[1].clone();
        loader.tick();
        assert_eq!(loader.render(20)[1], before);
    }

    #[test]
    fn component_trait_routes_input_and_invalidate() {
        let _guard = crate::test_support::global_test_lock();
        let (mut loader, count) = make();
        use crate::component::Component;
        assert!(loader.handles_input());
        loader.handle_input("\x1b");
        assert!(loader.aborted());
        // invalidate() rebuilds the display (requestRender fires).
        let before = count.get();
        loader.invalidate();
        assert_eq!(count.get(), before + 1);
    }
}
