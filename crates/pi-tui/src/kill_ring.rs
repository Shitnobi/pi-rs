//! Ring buffer for Emacs-style kill/yank operations — port of `kill-ring.ts`.
//!
//! Tracks killed (deleted) text entries. Consecutive kills can accumulate
//! into a single entry. Supports yank (paste most recent) and yank-pop
//! (cycle through older entries).

#[derive(Debug, Default, Clone)]
pub struct KillRing {
    ring: Vec<String>,
}

impl KillRing {
    pub fn new() -> Self {
        Self { ring: Vec::new() }
    }

    /// Add text to the kill ring.
    ///
    /// * `prepend` — if accumulating, prepend (backward deletion) or append
    ///   (forward deletion)
    /// * `accumulate` — merge with the most recent entry instead of creating
    ///   a new one
    pub fn push(&mut self, text: &str, prepend: bool, accumulate: bool) {
        if text.is_empty() {
            return;
        }

        if accumulate && !self.ring.is_empty() {
            let last = self.ring.pop().unwrap();
            self.ring.push(if prepend {
                format!("{text}{last}")
            } else {
                format!("{last}{text}")
            });
        } else {
            self.ring.push(text.to_string());
        }
    }

    /// Get the most recent entry without modifying the ring.
    pub fn peek(&self) -> Option<&str> {
        self.ring.last().map(|s| s.as_str())
    }

    /// Move the last entry to the front (for yank-pop cycling).
    pub fn rotate(&mut self) {
        if self.ring.len() > 1 {
            let last = self.ring.pop().unwrap();
            self.ring.insert(0, last);
        }
    }

    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_peek() {
        let mut ring = KillRing::new();
        assert_eq!(ring.peek(), None);
        ring.push("hello", false, false);
        assert_eq!(ring.peek(), Some("hello"));
        ring.push("world", false, false);
        assert_eq!(ring.peek(), Some("world"));
        assert_eq!(ring.len(), 2);
    }

    #[test]
    fn empty_push_is_ignored() {
        let mut ring = KillRing::new();
        ring.push("", false, false);
        assert_eq!(ring.len(), 0);
    }

    #[test]
    fn accumulate_appends_or_prepends() {
        let mut ring = KillRing::new();
        ring.push("hello", false, false);
        ring.push(" world", false, true);
        assert_eq!(ring.peek(), Some("hello world"));
        assert_eq!(ring.len(), 1);

        ring.push("say ", true, true);
        assert_eq!(ring.peek(), Some("say hello world"));
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn accumulate_on_empty_ring_pushes_normally() {
        let mut ring = KillRing::new();
        ring.push("a", true, true);
        assert_eq!(ring.peek(), Some("a"));
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn rotate_cycles_entries() {
        let mut ring = KillRing::new();
        ring.push("a", false, false);
        ring.push("b", false, false);
        ring.push("c", false, false);
        assert_eq!(ring.peek(), Some("c"));
        ring.rotate();
        assert_eq!(ring.peek(), Some("b"));
        ring.rotate();
        assert_eq!(ring.peek(), Some("a"));
        ring.rotate(); // single entry again — no-op when length 1? still 3 entries
        assert_eq!(ring.peek(), Some("c"));
        assert_eq!(ring.len(), 3);
    }

    #[test]
    fn rotate_with_one_entry_is_noop() {
        let mut ring = KillRing::new();
        ring.push("only", false, false);
        ring.rotate();
        assert_eq!(ring.peek(), Some("only"));
        assert_eq!(ring.len(), 1);
    }
}
