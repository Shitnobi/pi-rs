//! Port of `streamAssistantResponse` from
//! `packages/agent/src/agent-loop.ts` (lines ~296–470): stream an assistant
//! response from the LLM, transforming `AgentMessage[]` → `Message[]` at
//! the call boundary, replaying the partial-message protocol events, and
//! finalizing from `response.result()` (never from the event payload).
//!
//! The final message carries `thinkingLevel: config.reasoning ?? "off"`
//! attached as the LAST key (TS `Object.assign(await response.result(),
//! { thinkingLevel })` — with `preserve_order`, `Map::insert` replaces an
//! existing key in place and appends a new key last, matching JS spread).

use serde_json::Value;

use pi_providers::pi_ai::transcript::{normalize_context, transcript_messages};

use crate::host::AbortHandle;
use crate::r#loop::{AgentEventSink, LoopConfig, LoopStreamFn};
use crate::types::{AgentContext, AgentEvent};
#[cfg(test)]
use crate::types::BoxFuture;

/// Attach `thinkingLevel` to a result message (TS `Object.assign`):
/// added/overridden as the LAST key when the target is an object.
fn with_thinking_level(result: &Value, reasoning: Option<&str>) -> Value {
    let mut out = result.clone();
    if let Some(map) = out.as_object_mut() {
        map.insert(
            "thinkingLevel".to_string(),
            Value::String(reasoning.unwrap_or("off").to_string()),
        );
    }
    out
}

/// Port of `streamAssistantResponse` — returns the final assistant message
/// (with `thinkingLevel` attached), pushes/replaces partial messages on
/// `context.messages`, and emits the message event sequence.
///
/// `Err` models a `streamFn` throw (the TS "must not throw" contract is
/// violated only by test fakes; it propagates out of the loop).
pub async fn stream_assistant_response(
    context: &mut AgentContext,
    config: &LoopConfig,
    signal: Option<&AbortHandle>,
    emit: &dyn AgentEventSink,
    stream_fn: &LoopStreamFn,
) -> Result<Value, String> {
    let _ = signal;

    // Apply the context transform if configured (AgentMessage[] → AgentMessage[]).
    let mut messages = context.messages.clone();
    if let Some(mode) = &config.transform_context {
        messages = mode.apply(&messages);
    }

    // Convert to LLM-compatible messages (AgentMessage[] → Message[]).
    let llm_messages = config.convert_to_llm.apply(&messages);
    let llm_context = normalize_context(&serde_json::json!({ "messages": llm_messages }));
    let context_value =
        serde_json::json!({ "messages": transcript_messages(&llm_context) });

    // Resolve API key (important for expiring tokens):
    // (getApiKey ? await getApiKey(model.provider) : undefined) || config.apiKey
    let provider = config
        .model
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let dynamic = match &config.get_api_key {
        Some(hook) => hook(&provider).await,
        None => None,
    };
    let resolved_api_key = match dynamic {
        Some(key) if !key.is_empty() => Some(key),
        _ => config.api_key.clone(),
    };
    let options = serde_json::json!({
        "apiKey": resolved_api_key.map(Value::String).unwrap_or(Value::Null),
    });

    let response = stream_fn(&config.model, &context_value, &options).await?;
    // Record the requested level, whichever stream function answered.
    let result = |response: &crate::provider::StreamOutput| {
        with_thinking_level(&response.result, config.reasoning.as_deref())
    };

    let mut partial_message: Option<Value> = None;
    let mut added_partial = false;

    for event in &response.events {
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "start" => {
                let partial = event.get("partial").cloned().unwrap_or(Value::Null);
                context.messages.push(partial.clone());
                added_partial = true;
                partial_message = Some(partial.clone());
                emit.emit(AgentEvent::MessageStart { message: partial }).await;
            }
            "text_start" | "text_delta" | "text_end" | "thinking_start" | "thinking_delta"
            | "thinking_end" | "toolcall_start" | "toolcall_delta" | "toolcall_end" => {
                if partial_message.is_some() {
                    let partial = event.get("partial").cloned().unwrap_or(Value::Null);
                    if let Some(last) = context.messages.last_mut() {
                        *last = partial.clone();
                    }
                    partial_message = Some(partial.clone());
                    emit.emit(AgentEvent::MessageUpdate {
                        message: partial,
                        assistant_message_event: event.clone(),
                    })
                    .await;
                }
            }
            "done" | "error" => {
                let final_message = result(&response);
                finalize_partial(context, added_partial, &final_message);
                if !added_partial {
                    emit.emit(AgentEvent::MessageStart { message: final_message.clone() }).await;
                }
                emit.emit(AgentEvent::MessageEnd { message: final_message.clone() }).await;
                return Ok(final_message);
            }
            _ => {}
        }
    }

    // Stream ended without a terminal event: same finalization.
    let final_message = result(&response);
    finalize_partial(context, added_partial, &final_message);
    if !added_partial {
        emit.emit(AgentEvent::MessageStart { message: final_message.clone() }).await;
    }
    emit.emit(AgentEvent::MessageEnd { message: final_message.clone() }).await;
    Ok(final_message)
}

/// Replace the last context message with the final message when a partial
/// was added, else push it (TS shared tail of `streamAssistantResponse`).
fn finalize_partial(context: &mut AgentContext, added_partial: bool, final_message: &Value) {
    if added_partial {
        if let Some(last) = context.messages.last_mut() {
            *last = final_message.clone();
        }
    } else {
        context.messages.push(final_message.clone());
    }
}

/// Test helper: a minimal instantly-ready stream function over owned turns.
#[cfg(test)]
pub(crate) fn scripted_stream_fn(
    turns: Vec<crate::provider::StreamOutput>,
) -> impl Fn(&Value, &Value, &Value) -> BoxFuture<'static, Result<crate::provider::StreamOutput, String>> + Send + Sync {
    use std::sync::{Arc, Mutex};
    let turns = Arc::new(Mutex::new(turns));
    move |_model: &Value, _context: &Value, _options: &Value| {
        let turns = Arc::clone(&turns);
        Box::pin(async move {
            let mut queue = turns.lock().expect("scripted stream");
            if queue.is_empty() {
                return Err("provider script exhausted".to_string());
            }
            Ok(queue.remove(0))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r#loop::{AgentEventSink, LoopConfig};
    use crate::types::AgentEvent;
    use serde_json::json;

    struct Recorder(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);
    impl AgentEventSink for Recorder {
        fn emit<'a>(&'a self, event: AgentEvent) -> BoxFuture<'a, ()> {
            self.0.lock().unwrap().push(event.to_value());
            Box::pin(async {})
        }
    }

    fn config(reasoning: Option<&str>) -> LoopConfig {
        LoopConfig {
            model: json!({ "id": "m", "provider": "p" }),
            reasoning: reasoning.map(str::to_string),
            ..LoopConfig::default()
        }
    }

    fn turn(events: Vec<Value>, result: Value) -> crate::provider::StreamOutput {
        crate::provider::StreamOutput { events: events.into(), result }
    }

    #[test]
    fn start_deltas_done_partial_replacement_and_thinking_level_last() {
        let events = vec![
            json!({ "type": "start", "partial": json!({ "role": "assistant", "content": [] }) }),
            json!({ "type": "text_delta", "delta": "Hi", "partial": json!({ "role": "assistant", "content": [{ "type": "text", "text": "Hi" }] }) }),
            json!({ "type": "done", "reason": "stop", "message": { "junk": true } }),
        ];
        let result = json!({ "role": "assistant", "content": [{ "type": "text", "text": "Hi" }], "stopReason": "stop" });
        let mut context = AgentContext { messages: vec![json!({ "role": "user", "content": "hi" })], tools: None };
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Recorder(std::sync::Arc::clone(&recorded));
        let stream_fn = scripted_stream_fn(vec![turn(events, result)]);
        let out = crate::run_current_thread(stream_assistant_response(
            &mut context, &config(Some("medium")), None, &sink, &stream_fn,
        ))
        .unwrap();
        // thinkingLevel is the LAST key.
        let keys: Vec<&String> = out.as_object().unwrap().keys().collect();
        assert_eq!(*keys.last().unwrap(), "thinkingLevel");
        assert_eq!(out["thinkingLevel"], json!("medium"));
        // The partial was replaced by the final message in context.
        assert_eq!(context.messages.len(), 2);
        assert_eq!(context.messages[1]["stopReason"], json!("stop"));
        // Event sequence: message_start (copy of partial), message_update, message_end.
        let events = recorded.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["type"], json!("message_start"));
        assert_eq!(events[1]["type"], json!("message_update"));
        assert_eq!(events[1]["assistantMessageEvent"]["delta"], json!("Hi"));
        assert_eq!(events[2]["type"], json!("message_end"));
    }

    #[test]
    fn plain_done_emits_message_start_at_finalization() {
        let events = vec![json!({ "type": "done", "reason": "stop", "message": null })];
        let result = json!({ "role": "assistant", "content": [], "stopReason": "stop", "thinkingLevel": "stale" });
        let mut context = AgentContext::default();
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Recorder(std::sync::Arc::clone(&recorded));
        let stream_fn = scripted_stream_fn(vec![turn(events, result)]);
        let out = crate::run_current_thread(stream_assistant_response(
            &mut context, &config(None), None, &sink, &stream_fn,
        ))
        .unwrap();
        assert_eq!(out["thinkingLevel"], json!("off"));
        // Existing thinkingLevel keeps its original key slot (Object.assign).
        let keys: Vec<&String> = out.as_object().unwrap().keys().collect();
        assert_eq!(*keys.last().unwrap(), "thinkingLevel");
        let events = recorded.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], json!("message_start"));
        assert_eq!(events[1]["type"], json!("message_end"));
        assert_eq!(context.messages.len(), 1);
    }

    #[test]
    fn empty_script_finalizes_after_loop() {
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Recorder(std::sync::Arc::clone(&recorded));
        let stream_fn = scripted_stream_fn(vec![turn(vec![], json!({ "role": "assistant", "stopReason": "stop" }))]);
        let mut context = AgentContext::default();
        let out = crate::run_current_thread(stream_assistant_response(
            &mut context, &config(None), None, &sink, &stream_fn,
        ))
        .unwrap();
        assert_eq!(out["thinkingLevel"], json!("off"));
        let events = recorded.lock().unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn update_events_without_start_are_ignored() {
        let events = vec![
            json!({ "type": "text_delta", "delta": "x", "partial": json!({ "role": "assistant" }) }),
            json!({ "type": "done", "reason": "stop" }),
        ];
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Recorder(std::sync::Arc::clone(&recorded));
        let stream_fn = scripted_stream_fn(vec![turn(events, json!({ "role": "assistant", "stopReason": "stop" }))]);
        let mut context = AgentContext::default();
        crate::run_current_thread(stream_assistant_response(
            &mut context, &config(None), None, &sink, &stream_fn,
        ))
        .unwrap();
        let events = recorded.lock().unwrap();
        assert_eq!(events.len(), 2, "no message_update without a start event");
    }
}
