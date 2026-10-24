//! MCP client transport seam — the minimal surface of the
//! `@earendil-works/pi-mcp` package (packages/mcp/src) that the coding
//! agent's MCP runtime needs: the JSON-RPC message checks
//! (`protocol/jsonrpc.ts`), the [`McpTransport`] contract
//! (`transports/transport.ts`), and a scripted [`FakeTransport`].
//!
//! The client ([`super::client`]) is a synchronous state machine over this
//! seam; TS asynchrony maps to:
//! - message/error/close delivery is synchronous (TS
//!   `InMemoryTransport` defers with `queueMicrotask`),
//! - listener registration returns a subscription id instead of an
//!   unsubscribe closure,
//! - `AbortSignal` in `McpTransportSendOptions` is flattened to the
//!   [`SendOptions::aborted`] flag ("the sender no longer waits for a
//!   reply"),
//! - request timeouts sleep through an injected [`SleepFn`],
//! - thrown errors are [`McpErrorKind`] values (the TS error classes).

use std::collections::VecDeque;

use serde_json::Value;

use super::client::McpErrorKind;

/// `DEFAULT_MAX_MESSAGE_BYTES` (transports/transport.ts).
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// `JSON_RPC_ERROR_CODES` (protocol/jsonrpc.ts).
pub mod json_rpc_error_codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
}

/// `JsonRpcMessage` — a request, notification or response. Kept as the raw
/// JSON value; [`is_json_rpc_request`], [`is_json_rpc_notification`] and
/// [`is_json_rpc_response`] validate the union like `protocol/jsonrpc.ts`.
pub type JsonRpcMessage = Value;

/// `isObject` (protocol/jsonrpc.ts): object, not null, not array.
pub fn is_object(value: &Value) -> bool {
    value.is_object()
}

/// `isJsonRpcId`: string or finite number.
pub fn is_json_rpc_id(value: &Value) -> bool {
    value.is_string() || value.as_f64().is_some_and(f64::is_finite)
}

/// `isJsonRpcRequest`.
pub fn is_json_rpc_request(value: &Value) -> bool {
    is_object(value)
        && value["jsonrpc"] == "2.0"
        && is_json_rpc_id(&value["id"])
        && value["method"].is_string()
}

/// `isJsonRpcNotification`: a request shape without an `id` key.
pub fn is_json_rpc_notification(value: &Value) -> bool {
    is_object(value)
        && value["jsonrpc"] == "2.0"
        && value.get("id").is_none()
        && value["method"].is_string()
}

/// `isJsonRpcResponse`: success (`result`, no `error`) or a shaped error.
pub fn is_json_rpc_response(value: &Value) -> bool {
    if !is_object(value) || value["jsonrpc"] != "2.0" || !is_json_rpc_id(&value["id"]) {
        return false;
    }
    let has_result = value.get("result").is_some();
    let has_error = value.get("error").is_some();
    if has_result {
        return !has_error;
    }
    if !has_error {
        return false;
    }
    let error = &value["error"];
    is_object(error) && error["code"].is_number() && error["message"].is_string()
}

/// Default `McpConnectionClosedError` message (protocol/jsonrpc.ts).
pub const MCP_CONNECTION_CLOSED_MESSAGE: &str = "MCP connection closed";

/// Injected delay for the client runtime's request timeouts (TS
/// `setTimeout`-based timers): sleeps `ms` milliseconds. Harnesses inject
/// a deterministic fake that records (or fast-forwards) the waits.
pub type SleepFn = std::sync::Arc<dyn Fn(u64) + Send + Sync>;

/// `McpTransportSendOptions`: the `AbortSignal` is flattened to a flag —
/// the sender no longer waits for a reply (cancelled, timed out, answered,
/// or the client closed), so the transport must not dispatch or retry the
/// message. It does not undo a message already dispatched.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SendOptions {
    pub aborted: bool,
}

/// A message listener (`McpTransportMessageListener`).
pub type MessageListener = Box<dyn FnMut(&JsonRpcMessage) + Send>;
/// An error listener (`McpTransportErrorListener`), receiving
/// `error.message`.
pub type ErrorListener = Box<dyn FnMut(&str) + Send>;
/// A close listener (`McpTransportCloseListener`).
pub type CloseListener = Box<dyn FnMut() + Send>;

/// Listener bookkeeping shared by transports (`TransportEvents`).
/// [`TransportEvents::emit_close`] fires at most once per transport.
#[derive(Default)]
pub struct TransportEvents {
    message_listeners: Vec<(usize, MessageListener)>,
    error_listeners: Vec<(usize, ErrorListener)>,
    close_listeners: Vec<(usize, CloseListener)>,
    close_emitted: bool,
    next_id: usize,
}

impl TransportEvents {
    pub fn new() -> Self {
        Self::default()
    }

    /// `onMessage(listener)` — returns the subscription id for
    /// [`TransportEvents::off_message`].
    pub fn on_message(&mut self, listener: MessageListener) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.message_listeners.push((id, listener));
        id
    }

    /// `onError(listener)`.
    pub fn on_error(&mut self, listener: ErrorListener) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.error_listeners.push((id, listener));
        id
    }

    /// `onClose(listener)`.
    pub fn on_close(&mut self, listener: CloseListener) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.close_listeners.push((id, listener));
        id
    }

    /// The unsubscribe closure TS listeners receive.
    pub fn off_message(&mut self, id: usize) {
        self.message_listeners.retain(|(i, _)| *i != id);
    }

    /// The unsubscribe closure TS error listeners receive.
    pub fn off_error(&mut self, id: usize) {
        self.error_listeners.retain(|(i, _)| *i != id);
    }

    /// The unsubscribe closure TS close listeners receive.
    pub fn off_close(&mut self, id: usize) {
        self.close_listeners.retain(|(i, _)| *i != id);
    }

    /// `emitMessage`: notify every message listener in registration order.
    pub fn emit_message(&mut self, message: &JsonRpcMessage) {
        for (_, listener) in &mut self.message_listeners {
            listener(message);
        }
    }

    /// `emitError`: notify every error listener.
    pub fn emit_error(&mut self, error: &str) {
        for (_, listener) in &mut self.error_listeners {
            listener(error);
        }
    }

    /// `emitClose`: at most once per transport.
    pub fn emit_close(&mut self) {
        if self.close_emitted {
            return;
        }
        self.close_emitted = true;
        for (_, listener) in &mut self.close_listeners {
            listener();
        }
    }
}

/// The transport contract `McpClient.connect` drives
/// (`transports/transport.ts`): `start` the connection, `send` messages,
/// `close` once, and deliver incoming messages, errors and the close event
/// through the registered listeners. Failures are [`McpErrorKind`] values —
/// whatever the TS transport threw.
pub trait McpTransport: Send {
    /// `start()`: begin the connection. May deliver messages or report
    /// errors synchronously (scripted transports) — TS transports do so
    /// asynchronously.
    fn start(&mut self) -> Result<(), McpErrorKind>;
    /// `send(message, options?)`: dispatch one message. Must not dispatch
    /// (or retry) after `options.aborted` or after `close`.
    fn send(&mut self, message: &JsonRpcMessage, options: SendOptions) -> Result<(), McpErrorKind>;
    /// `close()`: idempotent; emits `onClose` at most once.
    fn close(&mut self) -> Result<(), McpErrorKind>;
    /// `onMessage(listener)` — returns the subscription id for
    /// [`McpTransport::off_message`].
    fn on_message(&mut self, listener: MessageListener) -> usize;
    /// `onError(listener)` — returns the subscription id for
    /// [`McpTransport::off_error`].
    fn on_error(&mut self, listener: ErrorListener) -> usize;
    /// `onClose(listener)` — returns the subscription id for
    /// [`McpTransport::off_close`].
    fn on_close(&mut self, listener: CloseListener) -> usize;
    /// The unsubscribe closure TS `onMessage` listeners receive.
    fn off_message(&mut self, id: usize);
    /// The unsubscribe closure TS `onError` listeners receive.
    fn off_error(&mut self, id: usize);
    /// The unsubscribe closure TS `onClose` listeners receive.
    fn off_close(&mut self, id: usize);
    /// `setProtocolVersion?(version)` — optional in TS; the default is a
    /// no-op like transports without it.
    fn set_protocol_version(&mut self, _version: &str) {}
}

/// One scripted action of a [`FakeTransport`] step.
#[derive(Debug, Clone)]
pub enum FakeStep {
    /// Deliver a message to the `onMessage` listeners.
    Message(JsonRpcMessage),
    /// Report an error to the `onError` listeners.
    Error(String),
    /// Close the transport: marks it closed and emits `onClose`; the
    /// current operation still succeeds.
    Close,
    /// Fail the operation (`start`/`send`) with this error.
    Fail(McpErrorKind),
}

/// Scripted transport for tests and the parity harness (the seam's stand-in
/// for `transports/in-memory.ts`). `start_steps` run during
/// [`FakeTransport::start`] (first start only). Each
/// [`FakeTransport::send`] pops **one batch** from `send_steps` and runs it
/// — a send whose batch is missing (queue empty) or empty succeeds
/// silently. Batches make scripts line up with the client's send sequence
/// (initialize, notifications/initialized, tools/list, …): the TS harness
/// fake shifts one batch per send the same way.
#[derive(Default)]
pub struct FakeTransport {
    events: TransportEvents,
    started: bool,
    closed: bool,
    /// Steps `start()` performs, in order (first start only).
    pub start_steps: Vec<FakeStep>,
    /// One batch of steps per `send()`, in order.
    pub send_steps: VecDeque<Vec<FakeStep>>,
    /// Every message passed to `send()`, in order.
    pub sent: Vec<JsonRpcMessage>,
    /// The [`SendOptions`] recorded alongside [`FakeTransport::sent`].
    pub sent_options: Vec<SendOptions>,
    /// Protocol versions passed to `set_protocol_version`.
    pub protocol_versions: Vec<String>,
}

impl FakeTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `steps` against the listeners; a `Fail` step aborts with its
    /// error (remaining steps of the batch are dropped).
    fn run_steps(&mut self, steps: &[FakeStep]) -> Result<(), McpErrorKind> {
        for step in steps {
            match step {
                FakeStep::Message(message) => self.events.emit_message(message),
                FakeStep::Error(message) => self.events.emit_error(message),
                FakeStep::Close => {
                    self.closed = true;
                    self.events.emit_close();
                }
                FakeStep::Fail(error) => return Err(error.clone()),
            }
        }
        Ok(())
    }

    /// `emitError` is exposed so tests can simulate transport-level
    /// failures (like `InMemoryTransport.emitError`).
    pub fn emit_error(&mut self, message: &str) {
        self.events.emit_error(message);
    }
}

impl McpTransport for FakeTransport {
    fn start(&mut self) -> Result<(), McpErrorKind> {
        if self.closed {
            return Err(McpErrorKind::connection_closed());
        }
        let first = !self.started;
        self.started = true;
        if first {
            let steps = std::mem::take(&mut self.start_steps);
            let result = self.run_steps(&steps);
            self.start_steps = steps;
            result
        } else {
            Ok(())
        }
    }

    fn send(&mut self, message: &JsonRpcMessage, options: SendOptions) -> Result<(), McpErrorKind> {
        if !self.started || self.closed {
            return Err(McpErrorKind::connection_closed());
        }
        self.sent.push(message.clone());
        self.sent_options.push(options);
        let batch = self.send_steps.pop_front().unwrap_or_default();
        self.run_steps(&batch)
    }

    fn close(&mut self) -> Result<(), McpErrorKind> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.events.emit_close();
        Ok(())
    }

    fn on_message(&mut self, listener: MessageListener) -> usize {
        self.events.on_message(listener)
    }

    fn on_error(&mut self, listener: ErrorListener) -> usize {
        self.events.on_error(listener)
    }

    fn on_close(&mut self, listener: CloseListener) -> usize {
        self.events.on_close(listener)
    }

    fn off_message(&mut self, id: usize) {
        self.events.off_message(id);
    }

    fn off_error(&mut self, id: usize) {
        self.events.off_error(id);
    }

    fn off_close(&mut self, id: usize) {
        self.events.off_close(id);
    }

    fn set_protocol_version(&mut self, version: &str) {
        self.protocol_versions.push(version.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn other(message: &str) -> McpErrorKind {
        McpErrorKind::Other { message: message.to_string() }
    }

    #[test]
    fn json_rpc_validators() {
        assert!(is_json_rpc_request(&json!({ "jsonrpc": "2.0", "id": 1, "method": "x" })));
        assert!(is_json_rpc_request(&json!({ "jsonrpc": "2.0", "id": "a", "method": "x", "params": {} })));
        assert!(!is_json_rpc_request(&json!({ "jsonrpc": "2.0", "method": "x" })));
        assert!(!is_json_rpc_request(&json!({ "jsonrpc": "1.0", "id": 1, "method": "x" })));
        assert!(is_json_rpc_notification(&json!({ "jsonrpc": "2.0", "method": "x" })));
        // `id: null` is present, so it is not a notification.
        assert!(!is_json_rpc_notification(&json!({ "jsonrpc": "2.0", "id": null, "method": "x" })));
        assert!(is_json_rpc_response(&json!({ "jsonrpc": "2.0", "id": 1, "result": {} })));
        assert!(is_json_rpc_response(
            &json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -32601, "message": "nope" } })
        ));
        assert!(!is_json_rpc_response(
            &json!({ "jsonrpc": "2.0", "id": 1, "result": {}, "error": { "code": 1, "message": "x" } })
        ));
        assert!(!is_json_rpc_response(&json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": 1 } })));
        assert!(!is_json_rpc_id(&json!(null)));
        assert!(!is_json_rpc_id(&json!(true)));
    }

    #[test]
    fn fake_transport_delivers_scripted_steps() {
        let mut transport = FakeTransport::new();
        let messages = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let closes = std::sync::Arc::new(std::sync::Mutex::new(0));
        let (m1, e1, c1) = (messages.clone(), errors.clone(), closes.clone());
        transport.on_message(Box::new(move |m| m1.lock().unwrap().push(m.clone())));
        transport.on_error(Box::new(move |e| e1.lock().unwrap().push(e.to_string())));
        transport.on_close(Box::new(move || *c1.lock().unwrap() += 1));

        transport
            .start_steps
            .push(FakeStep::Message(json!({ "jsonrpc": "2.0", "method": "hello" })));
        transport
            .send_steps
            .push_back(vec![FakeStep::Error("boom".to_string()), FakeStep::Message(json!({ "jsonrpc": "2.0", "id": 1, "result": 42 }))]);

        transport.start().unwrap();
        transport
            .send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ask" }), SendOptions::default())
            .unwrap();
        assert_eq!(messages.lock().unwrap().len(), 2);
        assert_eq!(errors.lock().unwrap().len(), 1);
        assert_eq!(errors.lock().unwrap()[0], "boom");
        assert_eq!(transport.sent.len(), 1);
        assert_eq!(transport.sent[0]["method"], "ask");

        // No batch left: the send succeeds silently; options are recorded.
        transport
            .send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "again" }), SendOptions { aborted: true })
            .unwrap();
        assert_eq!(transport.sent.len(), 2);
        assert_eq!(transport.sent_options[0], SendOptions::default());
        assert_eq!(transport.sent_options[1], SendOptions { aborted: true });
    }

    #[test]
    fn fake_transport_close_semantics() {
        let mut transport = FakeTransport::new();
        let closes = std::sync::Arc::new(std::sync::Mutex::new(0));
        let c1 = closes.clone();
        transport.on_close(Box::new(move || *c1.lock().unwrap() += 1));
        transport.start().unwrap();
        transport.close().unwrap();
        transport.close().unwrap(); // idempotent
        assert_eq!(*closes.lock().unwrap(), 1);
        let err = transport
            .send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "x" }), SendOptions::default())
            .unwrap_err();
        assert_eq!(err, McpErrorKind::connection_closed());
        assert_eq!(err.message(), "MCP connection closed");
        // start() after close fails too.
        assert_eq!(transport.start().unwrap_err(), McpErrorKind::connection_closed());
    }

    #[test]
    fn fake_transport_send_before_start_fails() {
        let mut transport = FakeTransport::new();
        let err = transport
            .send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "x" }), SendOptions::default())
            .unwrap_err();
        assert_eq!(err, McpErrorKind::connection_closed());
    }

    #[test]
    fn fake_transport_fail_step_and_close_step() {
        let mut transport = FakeTransport::new();
        transport.start_steps.push(FakeStep::Fail(other("nope")));
        assert_eq!(transport.start().unwrap_err().message(), "nope");

        let mut transport = FakeTransport::new();
        transport
            .send_steps
            .push_back(vec![
                FakeStep::Message(json!({ "jsonrpc": "2.0", "method": "m" })),
                FakeStep::Close,
            ]);
        transport.start().unwrap();
        transport
            .send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "x" }), SendOptions::default())
            .unwrap();
        assert!(transport.closed);
        // Closed by the step: the next send fails.
        assert_eq!(
            transport.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "x" }), SendOptions::default()),
            Err(McpErrorKind::connection_closed())
        );
    }

    #[test]
    fn fake_transport_records_protocol_versions_and_unsubscribe() {
        let mut transport = FakeTransport::new();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(0));
        let s1 = seen.clone();
        let id = transport.on_message(Box::new(move |_| *s1.lock().unwrap() += 1));
        transport.off_message(id);
        transport.set_protocol_version("2025-06-18");
        transport.start().unwrap();
        transport.events.emit_message(&json!({ "jsonrpc": "2.0", "method": "m" }));
        assert_eq!(*seen.lock().unwrap(), 0);
        assert_eq!(transport.protocol_versions, vec!["2025-06-18".to_string()]);
    }
}
