//! Port of `PendingMessageQueue` from `packages/agent/src/agent.ts`
//! (lines 143–174): the queue behind the agent's `steer`/`followUp`
//! surface.
//!
//! Pure logic, no traits (plan §4): messages are `serde_json::Value`, and
//! the mode controls how many queued messages a drain point consumes.

use serde_json::Value;

use crate::types::QueueMode;

/// Queue of pending user messages (TS `class PendingMessageQueue`).
///
/// - `peek` in `"all"` mode returns every queued message; in
///   `"one-at-a-time"` mode it returns only the oldest (or an empty vec).
/// - `drain` peeks, then removes exactly the drained count from the front.
#[derive(Debug, Clone)]
pub struct PendingMessageQueue {
    messages: Vec<Value>,
    /// Controls how queued messages are drained. Public like the TS field.
    pub mode: QueueMode,
}

impl PendingMessageQueue {
    pub fn new(mode: QueueMode) -> Self {
        PendingMessageQueue { messages: Vec::new(), mode }
    }

    /// TS `enqueue(message)`.
    pub fn enqueue(&mut self, message: Value) {
        self.messages.push(message);
    }

    /// TS `hasItems()`.
    pub fn has_items(&self) -> bool {
        !self.messages.is_empty()
    }

    /// TS `peek()` — the messages selected for the next drain point,
    /// without consuming them.
    pub fn peek(&self) -> Vec<Value> {
        match self.mode {
            QueueMode::All => self.messages.clone(),
            QueueMode::OneAtATime => match self.messages.first() {
                Some(first) => vec![first.clone()],
                None => Vec::new(),
            },
        }
    }

    /// TS `drain()` — peek, then drop the drained count from the front
    /// (TS `this.messages.slice(drained.length)`).
    pub fn drain(&mut self) -> Vec<Value> {
        let drained = self.peek();
        // Removes from the front, preserving the order of what remains —
        // NOT a swap_remove (see the JS-semantics traps).
        self.messages.drain(..drained.len());
        drained
    }

    /// TS `clear()`.
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

impl Default for PendingMessageQueue {
    fn default() -> Self {
        PendingMessageQueue::new(QueueMode::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(text: &str) -> Value {
        json!({ "role": "user", "content": [{ "type": "text", "text": text }], "timestamp": 1 })
    }

    #[test]
    fn peek_one_at_a_time_returns_first_only() {
        let mut queue = PendingMessageQueue::new(QueueMode::OneAtATime);
        queue.enqueue(msg("a"));
        queue.enqueue(msg("b"));
        assert!(queue.has_items());
        assert_eq!(queue.peek(), vec![msg("a")]);
        assert_eq!(queue.peek(), vec![msg("a")], "peek does not consume");
    }

    #[test]
    fn peek_all_returns_everything() {
        let mut queue = PendingMessageQueue::new(QueueMode::All);
        queue.enqueue(msg("a"));
        queue.enqueue(msg("b"));
        assert_eq!(queue.peek(), vec![msg("a"), msg("b")]);
    }

    #[test]
    fn drain_removes_only_the_drained_count() {
        let mut queue = PendingMessageQueue::new(QueueMode::OneAtATime);
        queue.enqueue(msg("a"));
        queue.enqueue(msg("b"));
        queue.enqueue(msg("c"));
        assert_eq!(queue.drain(), vec![msg("a")]);
        assert_eq!(queue.drain(), vec![msg("b")]);
        assert_eq!(queue.peek(), vec![msg("c")]);
        assert_eq!(queue.drain(), vec![msg("c")]);
        assert!(!queue.has_items());
        assert!(queue.drain().is_empty());
    }

    #[test]
    fn drain_all_mode_empties_queue() {
        let mut queue = PendingMessageQueue::new(QueueMode::All);
        queue.enqueue(msg("a"));
        queue.enqueue(msg("b"));
        assert_eq!(queue.drain(), vec![msg("a"), msg("b")]);
        assert!(!queue.has_items());
    }

    #[test]
    fn clear_empties_and_peek_on_empty_is_empty() {
        let mut queue = PendingMessageQueue::new(QueueMode::All);
        queue.enqueue(msg("a"));
        queue.clear();
        assert!(!queue.has_items());
        assert!(queue.peek().is_empty());
        queue.clear();
    }

    #[test]
    fn mode_flip_changes_peek_without_touching_contents() {
        let mut queue = PendingMessageQueue::new(QueueMode::OneAtATime);
        queue.enqueue(msg("a"));
        queue.enqueue(msg("b"));
        queue.mode = QueueMode::All;
        assert_eq!(queue.peek(), vec![msg("a"), msg("b")]);
        queue.mode = QueueMode::OneAtATime;
        assert_eq!(queue.peek(), vec![msg("a")]);
    }

    #[test]
    fn default_mode_is_one_at_a_time() {
        assert_eq!(PendingMessageQueue::default().mode, QueueMode::OneAtATime);
    }
}
