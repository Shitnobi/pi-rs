//! Port of `components/alt-screen-flash.ts` (51 LOC) — transient messages
//! composited by the alternate-screen renderer.
//!
//! The TS `setTimeout`/`clearTimeout` become per-entry deadlines over the
//! crate's virtual clock ([`crate::clock`]); the owner fires them with
//! [`AltScreenFlashContainer::tick`].

use crate::clock::now_ms;
use crate::utils::truncate_to_width;

const DEFAULT_DURATION_MS: f64 = 1000.0;

struct FlashEntry {
    message: String,
    /// Virtual-clock deadline (ms) at which the entry expires.
    expires_at: f64,
}

/// Transient messages composited by the alternate-screen renderer.
pub struct AltScreenFlashContainer {
    entries: Vec<FlashEntry>,
    /// `requestRender` callback (TS `() => this.requestRender()` closure).
    request_render: Option<Box<dyn FnMut()>>,
}

impl Default for AltScreenFlashContainer {
    fn default() -> Self {
        Self::new(None)
    }
}

impl AltScreenFlashContainer {
    pub fn new(request_render: Option<Box<dyn FnMut()>>) -> Self {
        AltScreenFlashContainer { entries: Vec::new(), request_render }
    }

    /// `flash(message, durationMs = 1000)` — queue a transient message.
    /// The entry expires after `duration_ms` of virtual-clock time; the
    /// owner must pump [`tick`] for the expiry (and the re-render) to fire.
    pub fn flash(&mut self, message: &str, duration_ms: Option<f64>) {
        let duration = duration_ms.unwrap_or(DEFAULT_DURATION_MS).max(0.0);
        self.entries.push(FlashEntry { message: message.to_string(), expires_at: now_ms() + duration });
        if let Some(cb) = self.request_render.as_mut() {
            cb();
        }
    }

    /// Fire expired entries (the TS `setTimeout` callbacks). Returns `true`
    /// when at least one entry expired (the TS callback also requests a
    /// render in that case).
    pub fn tick(&mut self) -> bool {
        let now = now_ms();
        let before = self.entries.len();
        self.entries.retain(|entry| entry.expires_at > now);
        let expired = self.entries.len() != before;
        if expired {
            if let Some(cb) = self.request_render.as_mut() {
                cb();
            }
        }
        expired
    }

    /// `dispose()` — drop every entry and cancel the pending timers.
    pub fn dispose(&mut self) {
        self.entries.clear();
    }

    /// Whether any entries are live (port-only introspection for tests).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `render(width)` — reverse-video lines, one per live entry.
    pub fn render(&mut self, width: usize) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| {
                let message = truncate_to_width(&format!(" {} ", entry.message), width, "", false);
                format!("\x1b[7m{message}\x1b[27m")
            })
            .collect()
    }

    /// `invalidate()` — no cached state.
    pub fn invalidate(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::advance_ms;
    use std::cell::Cell;

    #[test]
    fn flash_renders_reverse_video_and_expires() {
        crate::clock::reset_for_test();
        let renders = std::rc::Rc::new(Cell::new(0));
        let flash_renders = renders.clone();
        let mut flashes = AltScreenFlashContainer::new(Some(Box::new(move || {
            flash_renders.set(flash_renders.get() + 1)
        })));
        flashes.flash("Copied!", None);
        assert_eq!(renders.get(), 1);
        assert_eq!(flashes.render(20), vec!["\x1b[7m Copied! \x1b[27m".to_string()]);

        advance_ms(999.0);
        assert!(!flashes.tick());
        assert_eq!(flashes.render(20).len(), 1, "default duration is 1000ms");

        advance_ms(2.0);
        assert!(flashes.tick());
        assert!(flashes.is_empty());
        assert_eq!(renders.get(), 2, "expiry requests a render");
        assert!(!flashes.tick(), "no further expiries");
    }

    #[test]
    fn flash_duration_is_virtual_clock_time() {
        crate::clock::reset_for_test();
        let mut flashes = AltScreenFlashContainer::new(None);
        flashes.flash("msg", Some(100.0));
        advance_ms(50.0);
        assert!(!flashes.tick());
        assert_eq!(flashes.render(10).len(), 1);
        advance_ms(50.0);
        assert!(flashes.tick());
        assert!(flashes.is_empty());
    }

    #[test]
    fn dispose_drops_entries() {
        let mut flashes = AltScreenFlashContainer::new(None);
        flashes.flash("a", Some(10.0));
        flashes.flash("b", Some(10.0));
        flashes.dispose();
        assert!(flashes.is_empty());
        // After dispose, no expiry fires (timers cleared in TS too).
        advance_ms(100.0);
        assert!(!flashes.tick());
    }

    #[test]
    fn render_truncates_to_width() {
        let mut flashes = AltScreenFlashContainer::new(None);
        flashes.flash("hello world", None);
        let lines = flashes.render(6);
        assert_eq!(lines, vec!["\x1b[7m hello\x1b[0m\x1b[27m".to_string()]);
    }

    #[test]
    fn entries_render_in_order() {
        let mut flashes = AltScreenFlashContainer::new(None);
        flashes.flash("first", Some(10.0));
        flashes.flash("second", Some(20.0));
        let lines = flashes.render(20);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("first"));
        assert!(lines[1].contains("second"));
        advance_ms(10.0);
        flashes.tick();
        let lines = flashes.render(20);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("second"));
    }
}
