//! MCP client transport seam — the minimal surface of the
//! `@earendil-works/pi-mcp` package (packages/mcp/src) that the coding
//! agent's MCP runtime needs: the JSON-RPC message checks
//! (`protocol/jsonrpc.ts`), the [`McpTransport`] contract
//! (`transports/transport.ts`), and a scripted [`FakeTransport`].
//!
//! The real client (`client.ts`, 621 LOC) is ported in a later tranche as
//! a synchronous state machine over this seam; TS asynchrony maps to:
//! - message/error/close delivery is synchronous (TS
//!   `InMemoryTransport` defers with `queueMicrotask`),
//! - listener registration returns a subscription id instead of an
//!   unsubscribe closure,
//! - `AbortSignal` in `McpTransportSendOptions` is flattened to the
//!   [`SendOptions::aborted`] flag ("the sender no longer waits for a
//!   reply"),
//! - request timeouts sleep through an injected [`SleepFn`].

use std::collections::VecDeque;

use serde_json::Value;

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

/// An error thrown by a transport operation: the TS `McpConnectionClosedError`
/// (client state machine treats it specially) or any other thrown `Error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// `McpConnectionClosedError` — default message
    /// [`MCP_CONNECTION_CLOSED_MESSAGE`].
    ConnectionClosed(String),
    /// Any other thrown `Error`; the payload is `error.message`.
    Other(String),
}

impl TransportError {
    /// `new McpConnectionClosedError()`.
    pub fn connection_closed() -> Self {
        TransportError::ConnectionClosed(MCP_CONNECTION_CLOSED_MESSAGE.to_string())
    }

    /// `error.message`.
    pub fn message(&self) -> &str {
        match self {
            TransportError::ConnectionClosed(m) | TransportError::Other(m) => m,
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for TransportError {}

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
pub type MessageListener = Box<dyn FnMut(&JsonRpcMessage)>;
/// An error listener (`McpTransportErrorListener`), receiving
/// `error.message`.
pub type ErrorListener = Box<dyn FnMut(&str)>;
/// A close listener (`McpTransportCloseListener`).
pub type CloseListener = Box<dyn FnMut()>;

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
/// through the registered listeners.
pub trait McpTransport {
    /// `start()`: begin the connection. May deliver messages or report
    /// errors synchronously (scripted transports) — TS transports do so
    /// asynchronously.
    fn start(&mut self) -> Result<(), TransportError>;
    /// `send(message, options?)`: dispatch one message. Must not dispatch
    /// (or retry) after `options.aborted` or after `close`.
    fn send(&mut self, message: &JsonRpcMessage, options: SendOptions) -> Result<(), TransportError>;
    /// `close()`: idempotent; emits `onClose` at most once.
    fn close(&mut self) -> Result<(), TransportError>;
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
    /// Fail the operation (`start`/`send`) with this error.
    Fail(TransportError),
    /// Close the transport: marks it closed and emits `onClose`; the
    /// current operation still succeeds.
    Close,
}

/// Scripted transport for tests and the parity harness (the seam's stand-in
/// for `transports/in-memory.ts`). `start_steps` run during [`FakeTransport::start`];
/// each [`FakeTransport::send`] consumes `send_steps` until the queue is
/// empty or a [`FakeStep::Fail`] ends it with an error — a send with no
/// steps left succeeds silently.
#[derive(Default)]
pub struct FakeTransport {
    events: TransportEvents,
    started: bool,
    closed: bool,
    /// Steps `start()` performs, in order (first start only).
    pub start_steps: VecDeque<FakeStep>,
    /// Steps each `send()` performs, in order.
    pub send_steps: VecDeque<FakeStep>,
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
    /// error (remaining steps are dropped).
    fn run_steps(&mut self, steps: &mut VecDeque<FakeStep>) -> Result<(), TransportError> {
        while let Some(step) = steps.pop_front() {
            match step {
                FakeStep::Message(message) => self.events.emit_message(&message),
                FakeStep::Error(message) => self.events.emit_error(&message),
                FakeStep::Close => {
                    self.closed = true;
                    self.events.emit_close();
                }
                FakeStep::Fail(error) => return Err(error),
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
    fn start(&mut self) -> Result<(), TransportError> {
        if self.closed {
            return Err(TransportError::connection_closed());
        }
        let first = !self.started;
        self.started = true;
        if first {
            let mut steps = std::mem::take(&mut self.start_steps);
            let result = self.run_steps(&mut steps);
            self.start_steps = steps;
            result
        } else {
            Ok(())
        }
    }

    fn send(&mut self, message: &JsonRpcMessage, options: SendOptions) -> Result<(), TransportError> {
        if !self.started || self.closed {
            return Err(TransportError::connection_closed());
        }
        self.sent.push(message.clone());
        self.sent_options.push(options);
        let mut steps = std::mem::take(&mut self.send_steps);
        let result = self.run_steps(&mut steps);
        self.send_steps = steps;
        result
    }

    fn close(&mut self) -> Result<(), TransportError> {
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
        let messages = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let errors = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let closes = std::rc::Rc::new(std::cell::RefCell::new(0));
        let (m1, e1, c1) = (messages.clone(), errors.clone(), closes.clone());
        transport.on_message(Box::new(move |m| m1.borrow_mut().push(m.clone())));
        transport.on_error(Box::new(move |e| e1.borrow_mut().push(e.to_string())));
        transport.on_close(Box::new(move || *c1.borrow_mut() += 1));

        transport
            .start_steps
            .push_back(FakeStep::Message(json!({ "jsonrpc": "2.0", "method": "hello" })));
        transport.send_steps.push_back(FakeStep::Error("boom".to_string()));
        transport
            .send_steps
            .push_back(FakeStep::Message(json!({ "jsonrpc": "2.0", "id": 1, "result": 42 })));

        transport.start().unwrap();
        transport
            .send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ask" }), SendOptions::default())
            .unwrap();
        assert_eq!(messages.borrow().len(), 2);
        assert_eq!(errors.borrow().len(), 1);
        assert_eq!(errors.borrow()[0], "boom");
        assert_eq!(transport.sent.len(), 1);
        assert_eq!(transport.sent[0]["method"], "ask");

        // No steps left: the send succeeds silently; options are recorded.
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
        let closes = std::rc::Rc::new(std::cell::RefCell::new(0));
        let c1 = closes.clone();
        transport.on_close(Box::new(move || *c1.borrow_mut() += 1));
        transport.start().unwrap();
        transport.close().unwrap();
        transport.close().unwrap(); // idempotent
        assert_eq!(*closes.borrow(), 1);
        let err = transport
            .send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "x" }), SendOptions::default())
            .unwrap_err();
        assert_eq!(err, TransportError::connection_closed());
        assert_eq!(err.message(), "MCP connection closed");
        // start() after close fails too.
        assert_eq!(transport.start().unwrap_err(), TransportError::connection_closed());
    }

    #[test]
    fn fake_transport_send_before_start_fails() {
        let mut transport = FakeTransport::new();
        let err = transport
            .send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "x" }), SendOptions::default())
            .unwrap_err();
        assert_eq!(err, TransportError::connection_closed());
    }

    #[test]
    fn fake_transport_fail_step_and_close_step() {
        let mut transport = FakeTransport::new();
        transport.start_steps.push_back(FakeStep::Fail(TransportError::Other("nope".to_string())));
        assert_eq!(transport.start().unwrap_err().message(), "nope");

        let mut transport = FakeTransport::new();
        transport
            .send_steps
            .push_back(FakeStep::Message(json!({ "jsonrpc": "2.0", "method": "m" })));
        transport.send_steps.push_back(FakeStep::Close);
        transport.start().unwrap();
        transport
            .send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "x" }), SendOptions::default())
            .unwrap();
        assert!(transport.closed);
        // Closed by the step: the next send fails.
        assert_eq!(
            transport.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "x" }), SendOptions::default()),
            Err(TransportError::connection_closed())
        );
    }

    #[test]
    fn fake_transport_records_protocol_versions_and_unsubscribe() {
        let mut transport = FakeTransport::new();
        let seen = std::rc::Rc::new(std::cell::RefCell::new(0));
        let s1 = seen.clone();
        let id = transport.on_message(Box::new(move |_| *s1.borrow_mut() += 1));
        transport.off_message(id);
        transport.set_protocol_version("2025-06-18");
        transport.start().unwrap();
        transport.events_emit_for_test();
        assert_eq!(*seen.borrow(), 0);
        assert_eq!(transport.protocol_versions, vec!["2025-06-18".to_string()]);
    }

    impl FakeTransport {
        fn events_emit_for_test(&mut self) {
            self.events.emit_message(&json!({ "jsonrpc": "2.0", "method": "m" }));
        }
    }
}
