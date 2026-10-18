//! Port of `packages/tui/src/components/loader.ts` — `Loader`: a spinning
//! indicator + message component.
//!
//! Divergences from TS (both are the "clock seam" mandated by the port
//! plan — no threads, no sleeps):
//! - TS `Loader extends Text`; Rust uses composition (`text: Text`) since
//!   the crate has no class inheritance. `render` prepends the same blank
//!   line (loader.ts:38-40).
//! - `setInterval`/`clearInterval` (loader.ts:60-65) are replaced by an
//!   explicit [`Loader::tick`] method the runtime owner calls once per
//!   `interval_ms` (same pattern as `StdinBuffer::fire_timeout`).
//!   `interval_active` models `intervalId !== null`.
//! - `ui: TUI | null` is used only for `ui.requestRender()` in
//!   `updateDisplay` (loader.ts:101-103); modeled as an injectable
//!   `Option<Box<dyn Fn()>>` callback.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::component::Component;
use crate::components::text::Text;

/// `LoaderIndicatorOptions` (loader.ts:9-14).
#[derive(Default, Clone, Debug)]
pub struct LoaderIndicatorOptions {
    /// Animation frames. Use an empty vec to hide the indicator.
    pub frames: Option<Vec<String>>,
    /// Frame interval in milliseconds for animated indicators.
    pub interval_ms: Option<u64>,
}

/// `DEFAULT_FRAMES` (loader.ts:16).
const DEFAULT_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// `DEFAULT_INTERVAL_MS` (loader.ts:17).
const DEFAULT_INTERVAL_MS: u64 = 80;

/// Port of `Loader` (loader.ts:19-106) — a loader component that updates
/// with an optional spinning animation.
pub struct Loader {
    text: Text,
    frames: Vec<String>,
    interval_ms: u64,
    current_frame: usize,
    /// Models `intervalId: NodeJS.Timeout | null` (null ⇒ inactive).
    interval_active: bool,
    /// Virtual-clock deadline of the next animation tick (`setInterval`
    /// next-fire time; `None` while stopped) — see [`pump_loader_intervals_until`].
    next_tick_at: Option<f64>,
    /// The `TUI` seam: called instead of `this.ui.requestRender()`.
    request_render: Option<Box<dyn Fn()>>,
    render_indicator_verbatim: bool,
    spinner_color_fn: Box<dyn Fn(&str) -> String>,
    message_color_fn: Box<dyn Fn(&str) -> String>,
    message: String,
}

impl Loader {
    /// `constructor(ui, spinnerColorFn, messageColorFn, message =
    /// "Loading...", indicator?)` (loader.ts:33-46). The TS `ui: TUI`
    /// argument maps to the `request_render` callback (the only member of
    /// `TUI` this component touches).
    pub fn new(
        request_render: Option<Box<dyn Fn()>>,
        spinner_color_fn: Box<dyn Fn(&str) -> String>,
        message_color_fn: Box<dyn Fn(&str) -> String>,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        let mut loader = Loader {
            // `super("", 1, 0)` (loader.ts:44).
            text: Text::new("", 1, 0, None),
            frames: DEFAULT_FRAMES.iter().map(|f| f.to_string()).collect(),
            interval_ms: DEFAULT_INTERVAL_MS,
            current_frame: 0,
            interval_active: false,
            next_tick_at: None,
            request_render,
            render_indicator_verbatim: false,
            spinner_color_fn,
            message_color_fn,
            message: message.into(),
        };
        loader.set_indicator(indicator);
        loader
    }

    /// `render(width)` override (loader.ts:38-40) — a blank line, then the
    /// `Text` render.
    fn render_lines(&mut self, width: usize) -> Vec<String> {
        let mut lines = vec![String::new()];
        lines.extend(self.text.render(width));
        lines
    }

    /// `start()` (loader.ts:48-51).
    pub fn start(&mut self) {
        self.update_display();
        self.restart_animation();
    }

    /// `stop()` (loader.ts:53-58).
    pub fn stop(&mut self) {
        self.interval_active = false;
        self.next_tick_at = None;
    }

    /// The clock seam replacing the `setInterval` callback (loader.ts:61-64).
    /// The runtime owner calls this once per [`Loader::interval_ms`] while
    /// the loader is running; it advances the frame and updates the display.
    pub fn tick(&mut self) {
        // The interval only exists while active and with >1 frames
        // (loader.ts:59-66).
        if !self.interval_active || self.frames.len() <= 1 {
            return;
        }
        self.current_frame = (self.current_frame + 1) % self.frames.len();
        self.update_display();
    }

    /// The configured frame interval, so the runtime owner knows how often
    /// to call [`Loader::tick`].
    pub fn interval_ms(&self) -> u64 {
        self.interval_ms
    }

    /// Re-arm the virtual-clock deadline after an external fire
    /// ([`CancellableLoader`](crate::components::cancellable_loader::CancellableLoader)
    /// delegation).
    pub(crate) fn rearm_next_tick_at(&mut self, at: f64) {
        self.next_tick_at = Some(at);
    }

    /// `setMessage(message)` (loader.ts:68-71).
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.update_display();
    }

    /// The current message (TS field access).
    pub fn message(&self) -> &str {
        &self.message
    }

    /// `setIndicator(indicator?)` (loader.ts:77-83).
    pub fn set_indicator(&mut self, indicator: Option<LoaderIndicatorOptions>) {
        self.render_indicator_verbatim = indicator.is_some();
        self.frames = match indicator.as_ref().and_then(|i| i.frames.clone()) {
            Some(frames) => frames,
            None => DEFAULT_FRAMES.iter().map(|f| f.to_string()).collect(),
        };
        // `indicator?.intervalMs && indicator.intervalMs > 0 ? ... : DEFAULT`
        self.interval_ms = indicator
            .as_ref()
            .and_then(|i| i.interval_ms)
            .filter(|ms| *ms > 0)
            .unwrap_or(DEFAULT_INTERVAL_MS);
        self.current_frame = 0;
        self.start();
    }

    /// `restartAnimation()` (loader.ts:59-66) — starts the clock. With ≤1
    /// frames there is nothing to animate.
    fn restart_animation(&mut self) {
        self.stop();
        if self.frames.len() <= 1 {
            return;
        }
        self.interval_active = true;
        // TS `setInterval` semantics: the next fire is one period from the
        // arm instant (the virtual-clock event loop fires it at that
        // deadline, repeatedly, `deadline + period` per fire).
        self.next_tick_at = Some(crate::clock::now_ms() + self.interval_ms as f64);
    }

    /// The virtual-clock deadline of the animation interval, if armed
    /// ([`next_loader_deadline`] / [`pump_loader_intervals_until`]).
    pub(crate) fn loader_deadline(&self) -> Option<f64> {
        if self.interval_active && self.frames.len() > 1 {
            self.next_tick_at
        } else {
            None
        }
    }

    /// `getRenderedIndicator()` (loader.ts:68/85-88; `protected` in TS).
    pub fn get_rendered_indicator(&self) -> String {
        let frame = self.frames.get(self.current_frame).map(String::as_str).unwrap_or("");
        if self.render_indicator_verbatim {
            frame.to_string()
        } else {
            (self.spinner_color_fn)(frame)
        }
    }

    /// `updateDisplay()` (loader.ts:90-104) — rebuilds the wrapped `Text`
    /// content and requests a render.
    fn update_display(&mut self) {
        let rendered_frame = self.get_rendered_indicator();
        let indicator = if !rendered_frame.is_empty() {
            format!("{rendered_frame} ")
        } else {
            String::new()
        };
        let message = (self.message_color_fn)(&self.message);
        self.text.set_text(format!("{indicator}{message}"));
        if let Some(request_render) = &self.request_render {
            request_render();
        }
    }
}

impl Component for Loader {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_lines(width)
    }

    /// `invalidate()` override (loader.ts:73-76).
    fn invalidate(&mut self) {
        self.text.invalidate();
        self.update_display();
    }
}

// =============================================================================
// Virtual-clock loader interval registry
//
// TS `Loader.restartAnimation` arms a real `setInterval`, so the animation
// advances whenever the (virtual) event loop runs — independent of any
// `requestRender`. The Rust clock seam keeps `Loader::tick` explicit for
// the component-level harness, but the screen harness (tui-main/tui-alt)
// must fire loader ticks from `advance_clock` exactly like the TS event
// loop. Live loaders register here (weakly); the clock pump consults the
// registry for the earliest deadline and fires due ticks.
// =============================================================================

/// Virtual-clock surface of an animating loader ([`Loader`] and
/// [`CancellableLoader`](crate::components::cancellable_loader::CancellableLoader)).
pub trait LoaderClock {
    /// The next animation deadline, if the interval is armed.
    fn loader_deadline(&self) -> Option<f64>;
    /// Fire one animation tick at `deadline` and re-arm at
    /// `deadline + period` (TS `setInterval` semantics — large clock jumps
    /// fire each missed beat in order).
    fn fire_loader_tick_at(&mut self, deadline: f64);
}

impl LoaderClock for Loader {
    fn loader_deadline(&self) -> Option<f64> {
        Loader::loader_deadline(self)
    }
    fn fire_loader_tick_at(&mut self, deadline: f64) {
        self.tick();
        self.next_tick_at = Some(deadline + self.interval_ms as f64);
    }
}

thread_local! {
    /// Weak registry of live loaders created by the screen harness.
    static LOADER_CLOCKS: RefCell<Vec<Weak<RefCell<dyn LoaderClock>>>> =
        RefCell::new(Vec::new());
}

/// Register a live loader with the virtual-clock pump (screen harness).
pub fn register_loader<T: LoaderClock + 'static>(loader: &Rc<RefCell<T>>) {
    let clock: Rc<RefCell<dyn LoaderClock>> = loader.clone();
    LOADER_CLOCKS.with(|registry| registry.borrow_mut().push(Rc::downgrade(&clock)));
}

/// Collect the live loaders (dropping dead weak refs from the registry).
fn live_loaders() -> Vec<Rc<RefCell<dyn LoaderClock>>> {
    LOADER_CLOCKS.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.retain(|weak| weak.strong_count() > 0);
        registry.iter().filter_map(|weak| weak.upgrade()).collect()
    })
}

/// The earliest armed loader deadline, if any.
pub fn next_loader_deadline() -> Option<f64> {
    live_loaders()
        .iter()
        .filter_map(|loader| loader.borrow().loader_deadline())
        .reduce(f64::min)
}

/// Fire every loader tick due at or before `bound`. Returns whether
/// anything fired.
pub fn pump_loader_intervals_until(bound: f64) -> bool {
    let mut fired = false;
    for loader in live_loaders() {
        loop {
            let deadline = {
                let borrowed = loader.borrow();
                match borrowed.loader_deadline() {
                    Some(deadline) if deadline <= bound => deadline,
                    _ => break,
                }
            };
            loader.borrow_mut().fire_loader_tick_at(deadline);
            fired = true;
        }
    }
    fired
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::visible_width;
    use std::cell::Cell;
    use std::rc::Rc;

    // Upstream has no dedicated loader test file (only the manual
    // chat-simple.ts demo uses it); faithful tests per the port plan.

    fn spinner_fn() -> Box<dyn Fn(&str) -> String> {
        Box::new(|s: &str| format!("[{s}]") as String)
    }

    fn message_fn() -> Box<dyn Fn(&str) -> String> {
        Box::new(|s: &str| format!("({s})") as String)
    }

    fn render_counting_loader() -> (Loader, Rc<Cell<usize>>) {
        let count = Rc::new(Cell::new(0));
        let counter = count.clone();
        let loader = Loader::new(
            Some(Box::new(move || counter.set(counter.get() + 1))),
            spinner_fn(),
            message_fn(),
            "Working...",
            None,
        );
        (loader, count)
    }

    #[test]
    fn renders_blank_line_then_styled_message() {
        let mut loader = Loader::new(None, spinner_fn(), message_fn(), "Working...", None);
        let lines = loader.render(20);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "");
        // Text child has paddingX 1: " [⠋] (Working...)" padded to 20.
        assert!(lines[1].starts_with(" [⠋] (Working...)"));
        assert_eq!(visible_width(&lines[1]), 20);
    }

    #[test]
    fn construction_starts_display_and_requests_render() {
        // start() → updateDisplay() fires the requestRender seam
        // (loader.ts:48-50, 101-103).
        let (loader, count) = render_counting_loader();
        assert_eq!(count.get(), 1);
        assert_eq!(loader.message(), "Working...");
    }

    #[test]
    fn tick_advances_the_frame() {
        let (mut loader, _) = render_counting_loader();
        assert_eq!(loader.get_rendered_indicator(), "[⠋]");
        loader.tick();
        assert_eq!(loader.get_rendered_indicator(), "[⠙]");
        loader.tick();
        assert_eq!(loader.get_rendered_indicator(), "[⠹]");
    }

    #[test]
    fn tick_wraps_around_the_frame_list() {
        let (mut loader, _) = render_counting_loader();
        for _ in 0..DEFAULT_FRAMES.len() {
            loader.tick();
        }
        // A full cycle returns to the second frame (0 + 10) % 10 = 0.
        assert_eq!(loader.get_rendered_indicator(), "[⠋]");
    }

    #[test]
    fn stop_pauses_ticking() {
        let (mut loader, _) = render_counting_loader();
        loader.stop();
        let before = loader.get_rendered_indicator();
        loader.tick();
        assert_eq!(loader.get_rendered_indicator(), before);
        // Restarting resumes from the current frame.
        loader.start();
        loader.tick();
        assert_ne!(loader.get_rendered_indicator(), before);
    }

    #[test]
    fn single_frame_indicators_do_not_animate() {
        // frames.length <= 1 ⇒ restartAnimation leaves the clock stopped
        // (loader.ts:60-62).
        let mut loader = Loader::new(
            None,
            spinner_fn(),
            message_fn(),
            "Working...",
            Some(LoaderIndicatorOptions {
                frames: Some(vec!["*".to_string()]),
                interval_ms: Some(50),
            }),
        );
        assert_eq!(loader.get_rendered_indicator(), "*");
        loader.tick();
        assert_eq!(loader.get_rendered_indicator(), "*");
    }

    #[test]
    fn custom_indicator_renders_verbatim() {
        // An explicitly provided indicator bypasses the spinner color fn
        // (renderIndicatorVerbatim, loader.ts:78).
        let mut loader = Loader::new(
            None,
            spinner_fn(),
            message_fn(),
            "Working...",
            Some(LoaderIndicatorOptions {
                frames: Some(vec!["x".to_string(), "y".to_string()]),
                interval_ms: Some(10),
            }),
        );
        assert_eq!(loader.get_rendered_indicator(), "x");
        loader.tick();
        assert_eq!(loader.get_rendered_indicator(), "y");
        // Without an indicator, the spinner color fn wraps the frame.
        loader.set_indicator(None);
        assert_eq!(loader.get_rendered_indicator(), "[⠋]");
    }

    #[test]
    fn empty_frame_list_hides_the_indicator() {
        let mut loader = Loader::new(
            None,
            spinner_fn(),
            message_fn(),
            "Working...",
            Some(LoaderIndicatorOptions {
                frames: Some(Vec::new()),
                interval_ms: None,
            }),
        );
        // renderedFrame.length === 0 ⇒ no indicator, no trailing space
        // (loader.ts:92-94).
        let lines = loader.render(20);
        assert!(lines[1].starts_with(" (Working...)"));
    }

    #[test]
    fn set_indicator_resets_the_frame_and_interval() {
        let (mut loader, _) = render_counting_loader();
        loader.tick();
        loader.tick();
        assert_eq!(loader.current_frame_position(), 2);
        loader.set_indicator(Some(LoaderIndicatorOptions {
            frames: Some(vec!["a".to_string(), "b".to_string(), "c".to_string()]),
            interval_ms: Some(120),
        }));
        assert_eq!(loader.current_frame_position(), 0);
        assert_eq!(loader.get_rendered_indicator(), "a");
        assert_eq!(loader.interval_ms(), 120);
    }

    #[test]
    fn non_positive_interval_falls_back_to_default() {
        let loader = Loader::new(
            None,
            spinner_fn(),
            message_fn(),
            "Working...",
            Some(LoaderIndicatorOptions {
                frames: None,
                interval_ms: Some(0),
            }),
        );
        assert_eq!(loader.interval_ms(), DEFAULT_INTERVAL_MS);
    }

    #[test]
    fn set_message_updates_the_display() {
        let (mut loader, count) = render_counting_loader();
        loader.set_message("Done");
        let lines = loader.render(20);
        assert!(lines[1].contains("(Done)"));
        assert!(!lines[1].contains("Working"));
        // setMessage → updateDisplay → requestRender.
        assert_eq!(count.get(), 2);
    }

    #[test]
    fn invalidate_rebuilds_the_display() {
        let (mut loader, count) = render_counting_loader();
        loader.tick();
        loader.invalidate();
        // invalidate → super.invalidate() + updateDisplay() → requestRender.
        assert_eq!(count.get(), 3);
        let lines = loader.render(20);
        assert!(lines[1].contains("(Working...)"));
    }
}

/// Test-only access to the current frame index (the TS field
/// `currentFrame` is private).
#[cfg(test)]
impl Loader {
    fn current_frame_position(&self) -> usize {
        self.current_frame
    }
}
