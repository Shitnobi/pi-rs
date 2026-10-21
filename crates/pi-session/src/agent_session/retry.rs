//! TS auto-retry slice (agent-session.ts ~3739–3845): `_isRetryableError`
//! routing lives in [`super::event_handler`]; this module ports
//! `_prepareRetry` (exponential backoff with an abortable sleep),
//! `_omitRecoveryAttempt` (durable context-edit omission of the failed
//! attempt), `abortRetry()`/`isRetrying`, and `retryDelayMs`
//! (packages/ai/src/utils/retry.ts).
//!
//! **Sleep seam (plan phase3 §3):** TS `sleep(delayMs, signal)`
//! (utils/sleep.ts) resolves on timeout and rejects on abort. The Rust
//! port reuses [`pi_agent::host::Sleeper`] with the retry
//! [`AbortHandle`] standing in for the `_retryAbortController` signal —
//! the harness injects a recording sleeper that fast-forwards (or holds
//! the sleep pending until abort) so backoff parity is bit-exact.

use std::sync::Arc;

use serde_json::{json, Value};

use pi_agent::host::AbortHandle;

use super::events::MsgId;
use super::AgentSession;

/// `DEFAULT_MAX_AGENT_RETRY_DELAY_MS` (retry.ts) — 60 seconds.
pub const DEFAULT_MAX_AGENT_RETRY_DELAY_MS: f64 = 60_000.0;

/// JS `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// JS `Number.isSafeInteger(value)`.
fn is_safe_integer(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0 && value.abs() <= MAX_SAFE_INTEGER
}

/// TS `retryDelayMs(policy, attempt)` (packages/ai/src/utils/retry.ts):
/// `min(baseDelayMs * 2^max(0, attempt-1), maxAgentDelayMs ?? 60_000)` with
/// a safe-integer guard (f64 semantics — `2 ** k` can exceed
/// `MAX_SAFE_INTEGER`, e.g. for a huge `baseDelayMs`).
pub fn retry_delay_ms(base_delay_ms: f64, max_agent_delay_ms: Option<f64>, attempt: f64) -> f64 {
    let delay = base_delay_ms * 2f64.powf((attempt - 1.0).max(0.0));
    let safe_delay = if is_safe_integer(delay) { delay } else { MAX_SAFE_INTEGER };
    safe_delay.min(max_agent_delay_ms.unwrap_or(DEFAULT_MAX_AGENT_RETRY_DELAY_MS))
}

/// `{ type: "auto_retry_start", attempt, maxAttempts, delayMs, errorMessage }`.
pub fn auto_retry_start_event(
    attempt: f64,
    max_attempts: f64,
    delay_ms: f64,
    error_message: &str,
) -> Value {
    json!({
        "type": "auto_retry_start",
        "attempt": pi_core::ai::js_number(attempt),
        "maxAttempts": pi_core::ai::js_number(max_attempts),
        "delayMs": pi_core::ai::js_number(delay_ms),
        "errorMessage": error_message,
    })
}

impl AgentSession {
    /// TS `_omitRecoveryAttempt(message, toolResults = [])` — `Err` models
    /// the projected-message guard throw.
    fn omit_recovery_attempt(&mut self, message: MsgId, tool_results: &[MsgId]) -> Result<(), String> {
        let mut targets = vec![message];
        targets.extend_from_slice(tool_results);
        let mut target_ids = Vec::with_capacity(targets.len());
        for &target in &targets {
            target_ids.push(self.find_persisted_message_entry_id(target));
        }
        let unresolved_projected_target = targets.iter().enumerate().any(|(index, &target)| {
            target_ids[index].is_none() && self.state.state_message_ids.contains(&target)
        });
        if unresolved_projected_target {
            return Err(
                "Cannot persist recovery omission because a projected message has no source entry"
                    .to_string(),
            );
        }
        for target_id in target_ids.into_iter().flatten() {
            let edit_id = self.session_manager.append_context_edit(&target_id, None)?;
            if let Some(entry) = self.session_manager.get_entry(&edit_id) {
                self.emit(json!({ "type": "entry_appended", "entry": entry }));
            }
        }
        self.refresh_finalized_context();
        Ok(())
    }

    /// TS `_prepareRetry(message)` — prepare a retryable error for
    /// continuation with exponential backoff. Returns whether the caller
    /// should continue the agent.
    pub(super) async fn prepare_retry(&mut self, message: MsgId) -> Result<bool, String> {
        let settings = self.settings.retry_settings();
        if !settings.enabled {
            return Ok(false);
        }

        self.state.retry_attempt += 1.0;

        if self.state.retry_attempt > settings.max_retries {
            // Preserve the completed attempt count so post-run handling
            // can emit the final failure.
            self.state.retry_attempt -= 1.0;
            return Ok(false);
        }

        let delay_ms = retry_delay_ms(
            settings.base_delay_ms,
            Some(settings.max_agent_delay_ms),
            self.state.retry_attempt,
        );
        // TS `message.errorMessage || "Unknown error"` (empty string is
        // falsy).
        let error_message = self.state.arena[message]
            .get("errorMessage")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let error_message = if error_message.is_empty() { "Unknown error" } else { error_message };

        let attempt = self.state.retry_attempt;
        let max_attempts = settings.max_retries;
        self.emit(auto_retry_start_event(attempt, max_attempts, delay_ms, error_message));

        // Keep the failed attempt in raw history while durably omitting it
        // from the model projection.
        self.omit_recovery_attempt(message, &[])?;

        // Wait with exponential backoff (abortable). TS
        // `sleep(delayMs, signal)`; the f64→u64 cast saturates like JS's
        // timeout coercion (parity cases keep delays integral).
        let handle = AbortHandle::new();
        *self.state.retry_abort.lock().expect("retry abort slot") = Some(handle.clone());
        let outcome = self.sleeper.sleep(delay_ms as u64, &handle).await;
        // TS `finally { this._retryAbortController = undefined; }`.
        *self.state.retry_abort.lock().expect("retry abort slot") = None;
        if outcome.is_err() {
            // Aborted during sleep — emit end event so UI can clean up.
            self.finish_cancelled_retry();
            return Ok(false);
        }

        Ok(true)
    }

    /// TS `abortRetry()` — cancel an in-progress retry.
    pub fn abort_retry(&self) {
        if let Some(handle) = self.state.retry_abort.lock().expect("retry abort slot").as_ref() {
            handle.abort();
        }
    }

    /// TS `get isRetrying` — whether auto-retry is currently in progress.
    pub fn is_retrying(&self) -> bool {
        self.state.retry_abort.lock().expect("retry abort slot").is_some()
    }

    /// Harness seam: TS parity scripts call `session.abortRetry()` from
    /// script context while a prompt run holds the session; the shared
    /// retry-abort slot is the Rust mirror of that reach-in.
    pub fn abort_retry_tap(&self) -> Arc<dyn Fn() + Send + Sync> {
        let slot = Arc::clone(&self.state.retry_abort);
        Arc::new(move || {
            if let Some(handle) = slot.lock().expect("retry abort slot").as_ref() {
                handle.abort();
            }
        })
    }
}

