//! Generic undo stack with clone-on-push semantics — port of `undo-stack.ts`.
//!
//! The TS original stores deep clones of state snapshots
//! (`structuredClone`). In Rust, `S: Clone` gives the same detached-snapshot
//! guarantee for the plain data the editor stores. Popped snapshots are
//! returned directly (no re-cloning) since they are already detached.

#[derive(Debug, Default, Clone)]
pub struct UndoStack<S: Clone> {
    stack: Vec<S>,
}

impl<S: Clone> UndoStack<S> {
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// Push a deep clone of the given state onto the stack.
    pub fn push(&mut self, state: &S) {
        self.stack.push(state.clone());
    }

    /// Pop and return the most recent snapshot, or `None` if empty.
    pub fn pop(&mut self) -> Option<S> {
        self.stack.pop()
    }

    /// Remove all snapshots.
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pop_roundtrip() {
        let mut stack: UndoStack<String> = UndoStack::new();
        assert_eq!(stack.pop(), None);
        stack.push(&"first".to_string());
        stack.push(&"second".to_string());
        assert_eq!(stack.len(), 2);
        assert_eq!(stack.pop(), Some("second".to_string()));
        assert_eq!(stack.pop(), Some("first".to_string()));
        assert_eq!(stack.pop(), None);
    }

    #[test]
    fn push_clones_the_state() {
        let mut stack: UndoStack<Vec<String>> = UndoStack::new();
        let mut state = vec!["a".to_string()];
        stack.push(&state);
        state.push("b".to_string());
        // The snapshot must be unaffected by later mutation of the original.
        assert_eq!(stack.pop(), Some(vec!["a".to_string()]));
    }

    #[test]
    fn clear_removes_all() {
        let mut stack: UndoStack<u32> = UndoStack::new();
        stack.push(&1);
        stack.push(&2);
        stack.clear();
        assert!(stack.is_empty());
        assert_eq!(stack.pop(), None);
    }
}
