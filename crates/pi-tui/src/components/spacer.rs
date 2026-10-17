//! Port of `packages/tui/src/components/spacer.ts` — `Spacer`: renders a
//! fixed number of empty lines.
//!
//! NOTE: despite the port-plan hint about "basis sizing for stacks", the
//! upstream `Spacer` carries no layout metadata — grep of the TS package
//! shows it is only re-exported from `index.ts` and consumed as a plain
//! component that renders `lines` empty (unpadded) lines. This port follows
//! the TS exactly.

use crate::component::Component;

/// Port of `Spacer` (spacer.ts:6-22).
pub struct Spacer {
    lines: usize,
}

impl Default for Spacer {
    /// `new Spacer()` — defaults to 1 line (spacer.ts:8).
    fn default() -> Self {
        Spacer::new(1)
    }
}

impl Spacer {
    /// `constructor(lines = 1)` (spacer.ts:8-10).
    pub fn new(lines: usize) -> Self {
        Spacer { lines }
    }

    /// `setLines(lines)` (spacer.ts:12-14).
    pub fn set_lines(&mut self, lines: usize) {
        self.lines = lines;
    }

    /// The configured line count (TS field access).
    pub fn lines(&self) -> usize {
        self.lines
    }
}

impl Component for Spacer {
    /// `render(_width)` (spacer.ts:18-22) — `lines` empty strings, ignoring
    /// the viewport width.
    fn render(&mut self, _width: usize) -> Vec<String> {
        vec![String::new(); self.lines]
    }

    /// `invalidate()` (spacer.ts:16-18) — no cached state to invalidate
    /// currently.
    fn invalidate(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    // Upstream has no dedicated spacer test file; faithful tests per the
    // port plan.

    #[test]
    fn default_renders_one_empty_line() {
        let mut spacer = Spacer::default();
        assert_eq!(spacer.render(10), vec![String::new()]);
    }

    #[test]
    fn renders_configured_number_of_empty_lines() {
        let mut spacer = Spacer::new(3);
        assert_eq!(spacer.render(10), vec!["", "", ""]);
        // Lines are empty strings, not padded to width.
        assert_eq!(spacer.render(10)[0], "");
    }

    #[test]
    fn ignores_viewport_width() {
        let mut spacer = Spacer::new(2);
        assert_eq!(spacer.render(5), vec!["", ""]);
        assert_eq!(spacer.render(500), vec!["", ""]);
    }

    #[test]
    fn set_lines_changes_rendered_height() {
        let mut spacer = Spacer::new(1);
        spacer.set_lines(4);
        assert_eq!(spacer.render(10).len(), 4);
        assert_eq!(spacer.lines(), 4);
    }

    #[test]
    fn zero_lines_render_nothing() {
        let mut spacer = Spacer::new(0);
        assert!(spacer.render(10).is_empty());
    }
}
