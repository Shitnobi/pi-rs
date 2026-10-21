//! The queue surface of `class Agent` from `packages/agent/src/agent.ts`
//! (lines 281–335): `steer`/`followUp` queues, their modes, and the
//! peek/clear/has-queued API the agent loop polls at drain points.
//!
//! Tranche 0 splits this surface out of the (later) stateful `Agent` port
//! so the queue semantics land with parity first. The full `Agent`
//! (state, listeners, run lifecycle) builds on this in a later tranche.

use serde_json::Value;

use crate::queue::PendingMessageQueue;
use crate::types::QueueMode;

/// The steering/follow-up queue pair owned by an `Agent`
/// (TS `Agent#steeringQueue` / `Agent#followUpQueue` plus their public
/// methods).
#[derive(Debug, Clone)]
pub struct AgentQueues {
    steering: PendingMessageQueue,
    follow_up: PendingMessageQueue,
}

impl AgentQueues {
    /// TS constructor: `new PendingMessageQueue(steeringMode ?? "one-at-a-time")`
    /// for both queues.
    pub fn new(steering_mode: QueueMode, follow_up_mode: QueueMode) -> Self {
        AgentQueues {
            steering: PendingMessageQueue::new(steering_mode),
            follow_up: PendingMessageQueue::new(follow_up_mode),
        }
    }

    /// TS `Agent#steer(message)` — queue a message to be injected after the
    /// current assistant turn finishes.
    pub fn steer(&mut self, message: Value) {
        self.steering.enqueue(message);
    }

    /// TS `Agent#followUp(message)` — queue a message to run only after the
    /// agent would otherwise stop.
    pub fn follow_up(&mut self, message: Value) {
        self.follow_up.enqueue(message);
    }

    /// TS `Agent#clearSteeringQueue()`.
    pub fn clear_steering_queue(&mut self) {
        self.steering.clear();
    }

    /// TS `Agent#clearFollowUpQueue()`.
    pub fn clear_follow_up_queue(&mut self) {
        self.follow_up.clear();
    }

    /// TS `Agent#clearAllQueues()`.
    pub fn clear_all_queues(&mut self) {
        self.clear_steering_queue();
        self.clear_follow_up_queue();
    }

    /// TS `Agent#hasQueuedMessages()` — true when either queue still
    /// contains pending messages.
    pub fn has_queued_messages(&self) -> bool {
        self.steering.has_items() || self.follow_up.has_items()
    }

    /// TS `Agent#peekQueuedMessages()` — preview the messages selected for
    /// the next turn without consuming them. Steering shadows follow-ups.
    pub fn peek_queued_messages(&self) -> Vec<Value> {
        let steering = self.steering.peek();
        if !steering.is_empty() {
            return steering;
        }
        self.follow_up.peek()
    }

    /// TS `Agent#steeringMode` getter.
    pub fn steering_mode(&self) -> QueueMode {
        self.steering.mode
    }

    /// TS `Agent#steeringMode` setter.
    pub fn set_steering_mode(&mut self, mode: QueueMode) {
        self.steering.mode = mode;
    }

    /// TS `Agent#followUpMode` getter.
    pub fn follow_up_mode(&self) -> QueueMode {
        self.follow_up.mode
    }

    /// TS `Agent#followUpMode` setter.
    pub fn set_follow_up_mode(&mut self, mode: QueueMode) {
        self.follow_up.mode = mode;
    }

    /// Drain steering messages (the loop's `getSteeringMessages` seam,
    /// TS `agent-loop.ts`). Consumes per the steering queue's mode.
    pub fn drain_steering(&mut self) -> Vec<Value> {
        self.steering.drain()
    }

    /// Drain follow-up messages (the loop's `getFollowUpMessages` seam).
    /// Consumes per the follow-up queue's mode.
    pub fn drain_follow_up(&mut self) -> Vec<Value> {
        self.follow_up.drain()
    }
}

impl Default for AgentQueues {
    /// TS defaults: both modes `"one-at-a-time"`.
    fn default() -> Self {
        AgentQueues::new(QueueMode::default(), QueueMode::default())
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
    fn defaults_match_ts() {
        let queues = AgentQueues::default();
        assert_eq!(queues.steering_mode(), QueueMode::OneAtATime);
        assert_eq!(queues.follow_up_mode(), QueueMode::OneAtATime);
        assert!(!queues.has_queued_messages());
        assert!(queues.peek_queued_messages().is_empty());
    }

    /// Mirrors agent.test.ts "previews the next selected queued messages
    /// without consuming them" (steeringMode one-at-a-time, followUpMode all).
    #[test]
    fn preview_does_not_consume_and_steering_shadows_follow_up() {
        let mut queues = AgentQueues::new(QueueMode::OneAtATime, QueueMode::All);
        let first = msg("first steering");
        let second = msg("second steering");
        let follow_up = msg("follow-up");
        queues.steer(first.clone());
        queues.steer(second.clone());
        queues.follow_up(follow_up.clone());

        assert_eq!(queues.peek_queued_messages(), vec![first.clone()]);
        assert_eq!(queues.peek_queued_messages(), vec![first.clone()]);
        queues.clear_steering_queue();
        assert_eq!(queues.peek_queued_messages(), vec![follow_up.clone()]);
        assert_eq!(queues.peek_queued_messages(), vec![follow_up], "still not consumed");
    }

    /// Mirrors agent.test.ts "clearSteeringQueue falls back to follow-up"
    /// behavior (lines 947–957).
    #[test]
    fn clear_steering_falls_back_to_follow_up() {
        let mut queues = AgentQueues::default();
        let steering = msg("steering");
        let follow_up = msg("follow-up");
        queues.steer(steering.clone());
        queues.follow_up(follow_up.clone());
        assert_eq!(queues.peek_queued_messages(), vec![steering]);
        queues.clear_steering_queue();
        assert_eq!(queues.peek_queued_messages(), vec![follow_up]);
    }

    #[test]
    fn set_modes_flips_peek() {
        let mut queues = AgentQueues::default();
        queues.steer(msg("a"));
        queues.steer(msg("b"));
        assert_eq!(queues.peek_queued_messages().len(), 1);
        queues.set_steering_mode(QueueMode::All);
        assert_eq!(queues.steering_mode(), QueueMode::All);
        assert_eq!(queues.peek_queued_messages().len(), 2);
        queues.set_steering_mode(QueueMode::OneAtATime);
        assert_eq!(queues.peek_queued_messages().len(), 1);
    }

    #[test]
    fn follow_up_mode_all_peeks_all_follow_ups() {
        let mut queues = AgentQueues::new(QueueMode::OneAtATime, QueueMode::All);
        queues.follow_up(msg("f1"));
        queues.follow_up(msg("f2"));
        assert_eq!(queues.peek_queued_messages().len(), 2);
    }

    #[test]
    fn clear_variants() {
        let mut queues = AgentQueues::default();
        queues.steer(msg("s"));
        queues.follow_up(msg("f"));
        queues.clear_follow_up_queue();
        assert!(queues.has_queued_messages());
        queues.clear_steering_queue();
        assert!(!queues.has_queued_messages());
        queues.steer(msg("s2"));
        queues.follow_up(msg("f2"));
        queues.clear_all_queues();
        assert!(!queues.has_queued_messages());
        assert!(queues.peek_queued_messages().is_empty());
    }

    #[test]
    fn drains_consume_per_mode() {
        let mut queues = AgentQueues::new(QueueMode::OneAtATime, QueueMode::All);
        queues.steer(msg("s1"));
        queues.steer(msg("s2"));
        queues.follow_up(msg("f1"));
        queues.follow_up(msg("f2"));
        assert_eq!(queues.drain_steering(), vec![msg("s1")]);
        assert_eq!(queues.drain_steering(), vec![msg("s2")]);
        assert_eq!(queues.drain_follow_up(), vec![msg("f1"), msg("f2")]);
        assert!(!queues.has_queued_messages());
    }
}
