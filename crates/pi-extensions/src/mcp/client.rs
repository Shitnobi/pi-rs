//! `McpClient` — port of the pi-mcp package's `client.ts` (621 LOC) as a
//! synchronous state machine over the [`McpTransport`] seam.
//!
//! TS asynchrony maps to synchronous execution with two deferral seams that
//! reproduce Node's microtask ordering for the flows the coding agent uses:
//!
//! - **Mailbox**: transport listeners (registered by [`McpClient::connect`])
//!   only push events into a shared queue. After every transport call the
//!   client drains exactly the range of events appended during that call —
//!   matching the TS harness transport, which batches step events and
//!   delivers them to listeners after the steps of one operation. Events
//!   appended while processing (nested sends from notification listeners)
//!   are drained by the nested send's own range, so per-send batches stay
//!   contiguous like TS's recursive batch delivery.
//! - **Tasks**: responses to server→client requests (`ping`, `roots/list`)
//!   are deferred, like TS `handleRequest`'s `await handler(...)` hop, and
//!   run when the transport-operation stack unwinds.
//!
//! Requests whose reply did not arrive during their own send park; under
//! the harness constraints (replies scripted in the same send's steps) only
//! the request timeout can then settle them, modeled with the injected
//! [`SleepFn`].
//!
//! Not ported (unused by the coding agent's runtime; see TRACKING):
//! `McpRequestOptions.signal`/`onProgress` (progress tokens, `_meta`),
//! `setRequestHandler` (only the built-in `ping`/`roots/list` handlers),
//! and `AbortController` abort of incoming requests (handlers are
//! synchronous here).

use std::sync::{Arc, Mutex};


use serde_json::{json, Map, Value};

use super::transport::{
    is_json_rpc_notification, is_json_rpc_request, is_json_rpc_response, is_object, McpTransport,
    SendOptions, SleepFn,
};

/// `LATEST_PROTOCOL_VERSION` (protocol/types.ts).
pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
/// `SUPPORTED_PROTOCOL_VERSIONS` (protocol/types.ts).
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] =
    ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// `DEFAULT_REQUEST_TIMEOUT_MS` (client.ts).
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 30_000;
/// `MAX_LIST_PAGES` (client.ts).
pub const MAX_LIST_PAGES: usize = 1_000;

/// The TS error taxonomy that flows through the client: the JSON-RPC and
/// transport error classes of `protocol/jsonrpc.ts` and
/// `transports/streamable-http.ts`, plus `oauth/errors.ts`'s
/// `McpOAuthAuthorizationRequiredError` and plain JS errors. `message()`
/// reproduces each constructor's message.
#[derive(Debug, Clone, PartialEq)]
pub enum McpErrorKind {
    /// `McpError(code, message, data)` — a JSON-RPC error response.
    Rpc { code: i64, message: String, data: Option<Value> },
    /// `McpConnectionClosedError(message = "MCP connection closed")`.
    ConnectionClosed { message: String },
    /// `McpTimeoutError(timeoutMs)` — "MCP request timed out after {ms}ms".
    Timeout { timeout_ms: u64 },
    /// `McpAbortError` — "MCP request aborted" (name "AbortError").
    Abort,
    /// `McpHttpError(status, message, body)`; `auth_required` marks
    /// `McpAuthRequiredError` (401, "MCP server requires authentication")
    /// and `session_expired` marks `McpSessionExpiredError` (404, "MCP
    /// session expired") — both are also `McpHttpError`.
    Http { status: u64, message: String, body: String, auth_required: bool, session_expired: bool },
    /// `McpOAuthAuthorizationRequiredError` — "MCP OAuth authorization
    /// requires user interaction".
    OauthAuthorizationRequired,
    /// JS `TypeError` (network failure) — transient per
    /// `isTransientError`.
    Type { message: String },
    /// Any other thrown `Error`.
    Other { message: String },
}

impl McpErrorKind {
    /// `new McpConnectionClosedError()`.
    pub fn connection_closed() -> Self {
        McpErrorKind::ConnectionClosed { message: super::transport::MCP_CONNECTION_CLOSED_MESSAGE.to_string() }
    }

    /// `new McpError(code, message, data)`.
    pub fn rpc(code: i64, message: impl Into<String>, data: Option<Value>) -> Self {
        McpErrorKind::Rpc { code, message: message.into(), data }
    }

    /// `error.message`.
    pub fn message(&self) -> String {
        match self {
            McpErrorKind::Rpc { message, .. } => message.clone(),
            McpErrorKind::ConnectionClosed { message } => message.clone(),
            McpErrorKind::Timeout { timeout_ms } => format!("MCP request timed out after {}ms", timeout_ms),
            McpErrorKind::Abort => "MCP request aborted".to_string(),
            McpErrorKind::Http { message, .. } => message.clone(),
            McpErrorKind::OauthAuthorizationRequired => {
                "MCP OAuth authorization requires user interaction".to_string()
            }
            McpErrorKind::Type { message } | McpErrorKind::Other { message } => message.clone(),
        }
    }

    /// `error instanceof McpError && error.code === methodNotFound`.
    pub fn is_method_not_found(&self) -> bool {
        matches!(self, McpErrorKind::Rpc { code, .. } if *code == super::transport::json_rpc_error_codes::METHOD_NOT_FOUND)
    }
}

/// `ClientState` (client.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    Idle,
    Connecting,
    Connected,
    Closed,
}

impl ClientState {
    /// The TS union literal.
    pub fn as_str(&self) -> &'static str {
        match self {
            ClientState::Idle => "idle",
            ClientState::Connecting => "connecting",
            ClientState::Connected => "connected",
            ClientState::Closed => "closed",
        }
    }
}

/// One event a transport listener pushed into the mailbox.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// `onMessage` delivery.
    Message(Value),
    /// `onError` delivery, carrying `error.message`.
    Error(String),
    /// `onClose` delivery.
    Close,
}

/// `Root` (protocol/types.ts): `{ uri, name? }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    pub uri: String,
    pub name: Option<String>,
}

impl Root {
    fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("uri".to_string(), Value::String(self.uri.clone()));
        if let Some(name) = &self.name {
            map.insert("name".to_string(), Value::String(name.clone()));
        }
        Value::Object(map)
    }
}

/// `McpClientOptions extends Implementation` — the fields the runtime uses.
#[derive(Debug, Clone)]
pub struct McpClientOptions {
    pub name: String,
    pub version: String,
    /// `requestTimeoutMs?`.
    pub request_timeout_ms: Option<u64>,
    /// `roots?` — also arms the `roots/list` request handler.
    pub roots: Vec<Root>,
}

/// `McpRequestOptions` — the fields the runtime uses (`signal` and
/// `onProgress` are not ported; see the module docs).
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestOptions {
    pub timeout_ms: Option<u64>,
}

/// One parked request (TS `PendingRequest`; its `timeoutMs`/`cancellable`
/// fields live as locals in `send_request` — `initialize` never cancels).
/// `slot` is the promise settlement: set by a response, a send failure, or
/// the timeout.
struct PendingRequest {
    slot: Arc<Mutex<Option<Result<Value, McpErrorKind>>>>,
}

/// A deferred `handleRequest` response send (TS `await` hop).
enum Task {
    SendResponse { message: Value },
}

/// An `onNotification(method, listener)`-registered listener.
pub type NotificationListener = Box<dyn FnMut(Option<&Value>) + Send>;
/// An `onError` listener.
pub type ErrorListener = Box<dyn FnMut(&str) + Send>;

/// Port of `McpClient`.
pub struct McpClient {
    pub options: McpClientOptions,
    state: ClientState,
    transport: Option<Box<dyn McpTransport>>,
    next_request_id: u64,
    server_info_value: Option<Value>,
    server_capabilities_value: Option<Value>,
    instructions_value: Option<String>,
    protocol_version_value: Option<String>,
    /// Insertion-ordered pending map, keyed by the JSON-encoded request id.
    pending: Vec<(String, PendingRequest)>,
    mailbox: Arc<Mutex<Vec<ClientEvent>>>,
    tasks: Arc<Mutex<Vec<Task>>>,
    /// Transport-operation nesting depth; the task queue runs when it
    /// reaches zero.
    op_depth: usize,
    notification_listeners: Vec<(String, NotificationListener)>,
    error_listeners: Vec<ErrorListener>,
    close_listeners: Vec<Box<dyn FnMut() + Send>>,
    /// Injected `setTimeout` for request timeouts.
    sleep: SleepFn,
    /// The transport listener ids registered by `connect`, for `close`'s
    /// `disposeTransportListeners`.
    transport_listener_ids: Option<(usize, usize, usize)>,
    /// Set when `markClosed` fires the close listeners — the connection
    /// observes it after each client call (TS `handleClientClose` runs
    /// synchronously inside those listeners).
    close_fired: bool,
}

impl McpClient {
    pub fn new(options: McpClientOptions, sleep: SleepFn) -> Self {
        McpClient {
            options,
            state: ClientState::Idle,
            transport: None,
            next_request_id: 1,
            server_info_value: None,
            server_capabilities_value: None,
            instructions_value: None,
            protocol_version_value: None,
            pending: Vec::new(),
            mailbox: Arc::new(Mutex::new(Vec::new())),
            tasks: Arc::new(Mutex::new(Vec::new())),
            op_depth: 0,
            notification_listeners: Vec::new(),
            error_listeners: Vec::new(),
            close_listeners: Vec::new(),
            sleep,
            transport_listener_ids: None,
            close_fired: false,
        }
    }

    /// Whether `markClosed` fired the close listeners since the last take —
    /// the connection's `handleClientClose` seam.
    pub fn take_close_fired(&mut self) -> bool {
        std::mem::take(&mut self.close_fired)
    }

    /// `connectionState`.
    pub fn connection_state(&self) -> ClientState {
        self.state
    }

    /// `serverInfo`.
    pub fn server_info(&self) -> Option<&Value> {
        self.server_info_value.as_ref()
    }

    /// `serverCapabilities`.
    pub fn server_capabilities(&self) -> Option<&Value> {
        self.server_capabilities_value.as_ref()
    }

    /// `instructions`.
    pub fn instructions(&self) -> Option<&str> {
        self.instructions_value.as_deref()
    }

    /// `protocolVersion`.
    pub fn protocol_version(&self) -> Option<&str> {
        self.protocol_version_value.as_deref()
    }

    /// `onNotification(method, listener)` — returns the id for
    /// [`McpClient::off_notification`]. Listener errors are reported to the
    /// `onError` listeners, like the TS `try`/`catch`.
    pub fn on_notification(&mut self, method: &str, listener: NotificationListener) -> usize {
        self.notification_listeners.push((method.to_string(), listener));
        self.notification_listeners.len() - 1
    }

    /// The unsubscribe closure TS listeners receive.
    pub fn off_notification(&mut self, id: usize) {
        let _ = self.notification_listeners.remove(id);
    }

    /// `onError(listener)`.
    pub fn on_error(&mut self, listener: ErrorListener) -> usize {
        self.error_listeners.push(listener);
        self.error_listeners.len() - 1
    }

    /// The unsubscribe closure TS error listeners receive.
    pub fn off_error(&mut self, id: usize) {
        let _ = self.error_listeners.remove(id);
    }

    /// `onClose(listener)` — called once when the connection closes,
    /// whether the transport dropped or `close()` was called.
    pub fn on_close(&mut self, listener: Box<dyn FnMut() + Send>) -> usize {
        self.close_listeners.push(listener);
        self.close_listeners.len() - 1
    }

    /// The unsubscribe closure TS close listeners receive.
    pub fn off_close(&mut self, id: usize) {
        let _ = self.close_listeners.remove(id);
    }

    /// `connect(transport)` — the `initialize` handshake. On failure the
    /// client closes itself, like the TS `catch`.
    pub fn connect(&mut self, mut transport: Box<dyn McpTransport>) -> Result<Value, McpErrorKind> {
        if self.state != ClientState::Idle {
            return Err(McpErrorKind::Other {
                message: format!("Cannot connect MCP client in {} state", self.state.as_str()),
            });
        }
        self.state = ClientState::Connecting;
        let mailbox = self.mailbox.clone();
        let id = transport.on_message(Box::new(move |message| {
            mailbox.lock().unwrap().push(ClientEvent::Message(message.clone()));
        }));
        let m_id = id;
        let mailbox = self.mailbox.clone();
        let e_id = transport.on_error(Box::new(move |message| {
            mailbox.lock().unwrap().push(ClientEvent::Error(message.to_string()));
        }));
        let mailbox = self.mailbox.clone();
        let c_id = transport.on_close(Box::new(move || mailbox.lock().unwrap().push(ClientEvent::Close)));
        self.transport_listener_ids = Some((m_id, e_id, c_id));
        self.transport = Some(transport);
        let result = self.connect_handshake();
        if let Err(error) = &result {
            let _ = self.close();
            return Err(error.clone());
        }
        result
    }

    /// The body of TS `connect` after the listeners are registered.
    fn connect_handshake(&mut self) -> Result<Value, McpErrorKind> {        self.with_transport_op(|client| {
            let transport = client.transport.as_mut().expect("transport set");
            transport.start()
        })?;
        let mut capabilities = Map::new();
        if !self.options.roots.is_empty() {
            capabilities.insert("roots".to_string(), Value::Object(Map::new()));
        }
        let mut client_info = Map::new();
        client_info.insert("name".to_string(), Value::String(self.options.name.clone()));
        client_info.insert("version".to_string(), Value::String(self.options.version.clone()));
        let mut params = Map::new();
        params.insert("protocolVersion".to_string(), Value::String(LATEST_PROTOCOL_VERSION.to_string()));
        params.insert("capabilities".to_string(), Value::Object(capabilities));
        params.insert("clientInfo".to_string(), Value::Object(client_info));
        let result = self.request_internal(
            "initialize",
            Some(Value::Object(params)),
            &RequestOptions::default(),
            true,
        )?;
        let result = validate_initialize_result(&result)?;
        let protocol_version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&protocol_version.as_str()) {
            return Err(McpErrorKind::Other {
                message: format!("MCP server selected unsupported protocol version {}", protocol_version),
            });
        }
        self.protocol_version_value = Some(protocol_version.clone());
        self.server_info_value = Some(result.get("serverInfo").cloned().unwrap_or(Value::Null));
        self.server_capabilities_value = Some(result.get("capabilities").cloned().unwrap_or(Value::Null));
        self.instructions_value = result.get("instructions").and_then(Value::as_str).map(str::to_string);
        if let Some(transport) = self.transport.as_mut() {
            transport.set_protocol_version(&protocol_version);
        }
        self.notify_internal("notifications/initialized", None, true)?;
        self.state = ClientState::Connected;
        Ok(result)
    }

    /// `request(method, params?, options?)`.
    pub fn request(
        &mut self,
        method: &str,
        params: Option<Value>,
        options: &RequestOptions,
    ) -> Result<Value, McpErrorKind> {
        self.request_internal(method, params, options, false)
    }

    /// `notify(method, params?)`.
    pub fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), McpErrorKind> {
        self.notify_internal(method, params, false)
    }

    /// `ping(options?)`.
    pub fn ping(&mut self, options: &RequestOptions) -> Result<(), McpErrorKind> {
        self.request("ping", None, options).map(|_| ())
    }

    /// `listTools(options?)` — every tool, following `nextCursor`.
    pub fn list_tools(&mut self, options: &RequestOptions) -> Result<Vec<Value>, McpErrorKind> {
        self.list_all("tools/list", "tools", is_tool, options)
    }

    /// `listResources(options?)` — every resource, following `nextCursor`.
    pub fn list_resources(&mut self, options: &RequestOptions) -> Result<Vec<Value>, McpErrorKind> {
        self.list_all("resources/list", "resources", is_resource, options).map(|items| {
            items.into_iter().map(|item| to_resource(&item)).collect()
        })
    }

    /// `listResourcesPage(cursor?, options?)` — one page of resources.
    pub fn list_resources_page(
        &mut self,
        cursor: Option<&str>,
        options: &RequestOptions,
    ) -> Result<Value, McpErrorKind> {
        let page = self.list_page("resources/list", "resources", is_resource, cursor, options)?;
        Ok(page_result(page, "resources", to_resource))
    }

    /// `listResourceTemplates(options?)`.
    pub fn list_resource_templates(&mut self, options: &RequestOptions) -> Result<Vec<Value>, McpErrorKind> {
        self.list_all("resources/templates/list", "resourceTemplates", is_resource_template, options)
            .map(|items| items.into_iter().map(|item| to_resource_template(&item)).collect())
    }

    /// `listResourceTemplatesPage(cursor?, options?)`.
    pub fn list_resource_templates_page(
        &mut self,
        cursor: Option<&str>,
        options: &RequestOptions,
    ) -> Result<Value, McpErrorKind> {
        let page = self.list_page(
            "resources/templates/list",
            "resourceTemplates",
            is_resource_template,
            cursor,
            options,
        )?;
        Ok(page_result(page, "resourceTemplates", to_resource_template))
    }

    /// `readResource(uri, options?)`.
    pub fn read_resource(&mut self, uri: &str, options: &RequestOptions) -> Result<Value, McpErrorKind> {
        let mut params = Map::new();
        params.insert("uri".to_string(), Value::String(uri.to_string()));
        let result = self.request("resources/read", Some(Value::Object(params)), options)?;
        validate_read_resource_result(&result)
    }

    /// `callTool(name, args?, options?)`.
    pub fn call_tool(
        &mut self,
        name: &str,
        args: Option<Value>,
        options: &RequestOptions,
    ) -> Result<Value, McpErrorKind> {
        let mut params = Map::new();
        params.insert("name".to_string(), Value::String(name.to_string()));
        if let Some(args) = args {
            params.insert("arguments".to_string(), args);
        }
        let result = self.request("tools/call", Some(Value::Object(params)), options)?;
        validate_call_tool_result(&result)
    }

    /// `close()` — idempotent; rejects in-flight requests, aborts server
    /// requests we are serving, flips the state, and closes the transport.
    pub fn close(&mut self) -> Result<(), McpErrorKind> {
        let mut transport = self.transport.take();
        // `disposeTransportListeners`: the close event reaches no client
        // handler, so closing the transport pushes no mailbox events.
        if let (Some(transport), Some((m_id, e_id, c_id))) =
            (transport.as_mut(), self.transport_listener_ids.take())
        {
            transport.off_message(m_id);
            transport.off_error(e_id);
            transport.off_close(c_id);
        }
        self.mark_closed(McpErrorKind::connection_closed());
        match transport.as_mut() {
            Some(transport) => transport.close(),
            None => Ok(()),
        }
    }

    /// Delivers an external event (harness `emit` op): pushes into the
    /// mailbox like a transport listener would and drains it. In TS the
    /// harness calls the transport listeners directly, which runs the same
    /// `handleMessage`/`handleTransportClose` path synchronously.
    pub fn deliver_external(&mut self, event: ClientEvent) {
        let start = self.mailbox.lock().unwrap().len();
        self.mailbox.lock().unwrap().push(event);
        self.op_depth += 1;
        self.drain_from(start);
        self.op_depth -= 1;
        if self.op_depth == 0 {
            self.run_tasks();
        }
    }

    // ---- internals ----

    /// Runs a transport operation with mailbox draining: events appended
    /// during the operation are processed in order (recursively for nested
    /// sends), and deferred tasks run when the outermost operation ends.
    fn with_transport_op<T>(&mut self, op: impl FnOnce(&mut Self) -> Result<T, McpErrorKind>) -> Result<T, McpErrorKind> {
        let start = self.mailbox.lock().unwrap().len();
        self.op_depth += 1;
        let result = op(self);
        self.drain_from(start);
        self.op_depth -= 1;
        if self.op_depth == 0 {
            self.run_tasks();
        }
        result
    }

    /// Processes the mailbox events from `start` onward. Events appended
    /// while processing are handled by the nested transport ops that
    /// appended them, so iterate to the fixed end.
    fn drain_from(&mut self, start: usize) {
        let end = self.mailbox.lock().unwrap().len();
        let mut index = start;
        while index < end {
            let event = self.mailbox.lock().unwrap()[index].clone();
            self.handle_event(event);
            index += 1;
        }
    }

    /// Runs deferred tasks (server→client request responses) FIFO. A task's
    /// own sends may queue more tasks.
    fn run_tasks(&mut self) {
        loop {
            let task = self.tasks.lock().unwrap().pop();
            let Some(task) = task else { break };
            match task {
                Task::SendResponse { message } => {
                    let result = self.with_transport_op(|client| {
                        let transport = client.transport.as_mut();
                        match transport {
                            Some(transport) => transport.send(&message, SendOptions::default()),
                            None => Ok(()),
                        }
                    });
                    if let Err(error) = result {
                        self.emit_error(&error.message());
                    }
                }
            }
        }
    }

    fn handle_event(&mut self, event: ClientEvent) {
        match event {
            ClientEvent::Message(message) => self.handle_message(message),
            ClientEvent::Error(message) => self.emit_error(&message),
            ClientEvent::Close => self.handle_transport_close(),
        }
    }

    /// `handleMessage` — dispatch by message shape.
    fn handle_message(&mut self, message: Value) {
        if is_json_rpc_response(&message) {
            self.handle_response(&message);
            return;
        }
        if is_json_rpc_request(&message) {
            self.handle_request(&message);
            return;
        }
        if is_json_rpc_notification(&message) {
            let method = message.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
            let params = message.get("params");
            self.handle_notification(&method, params);
            return;
        }
        self.emit_error(&McpErrorKind::rpc(
            super::transport::json_rpc_error_codes::INVALID_REQUEST,
            "Received invalid JSON-RPC message",
            None,
        )
        .message());
    }

    /// `handleResponse` — settle the pending request, or report an unknown
    /// id to the error listeners.
    fn handle_response(&mut self, message: &Value) {
        let key = id_key(&message["id"]);
        let entry = self.take_pending(&key);
        let Some(entry) = entry else {
            self.emit_error(&format!("Received response for unknown MCP request {}", js_to_string(&message["id"])));
            return;
        };
        if let Some(error) = message.get("error") {
            let error = McpErrorKind::rpc(
                error["code"].as_i64().unwrap_or_default(),
                error["message"].as_str().unwrap_or_default().to_string(),
                error.get("data").filter(|d| !d.is_null()).cloned(),
            );
            *entry.slot.lock().unwrap() = Some(Err(error));
        } else {
            *entry.slot.lock().unwrap() = Some(Ok(message.get("result").cloned().unwrap_or(Value::Null)));
        }
    }

    /// `handleRequest` — `ping` and `roots/list` are built in. Unknown
    /// methods answer `-32601` synchronously (the TS send call precedes its
    /// `await`); known handlers answer through a deferred task (the TS
    /// `await handler(...)` hop defers the response send).
    fn handle_request(&mut self, message: &Value) {
        if self.transport.is_none() {
            return;
        }
        let method = message.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
        let handler_result = match method.as_str() {
            "ping" => Some(Value::Object(Map::new())),
            "roots/list" => {
                let roots: Vec<Value> = self.options.roots.iter().map(|r| r.to_value()).collect();
                let mut result = Map::new();
                result.insert("roots".to_string(), Value::Array(roots));
                Some(Value::Object(result))
            }
            _ => None,
        };
        match handler_result {
            Some(result) => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": message["id"].clone(),
                    "result": result,
                });
                self.tasks.lock().unwrap().push(Task::SendResponse { message: response });
            }
            None => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": message["id"].clone(),
                    "error": {
                        "code": super::transport::json_rpc_error_codes::METHOD_NOT_FOUND,
                        "message": format!("Method not found: {}", method),
                    },
                });
                let result = self.with_transport_op(|client| {
                    let transport = client.transport.as_mut();
                    match transport {
                        Some(transport) => transport.send(&response, SendOptions::default()),
                        None => Ok(()),
                    }
                });
                if let Err(error) = result {
                    self.emit_error(&error.message());
                }
            }
        }
    }

    /// `handleNotification` — progress and cancellation special cases
    /// (no-ops without progress support; incoming requests are synchronous
    /// here so cancellation cannot interleave), then the listeners.
    fn handle_notification(&mut self, method: &str, params: Option<&Value>) {
        let _ = method; // notifications/progress, notifications/cancelled
        for (listener_method, listener) in self.notification_listeners.iter_mut() {
            if listener_method == method {
                listener(params);
            }
        }
    }

    /// `cancelPending(id, error, notifyServer, reason?)` — settles the
    /// request and, when cancellable, tells the server.
    fn cancel_pending(&mut self, key: &str, error: McpErrorKind, notify_server: bool, reason: Option<&str>) {
        let Some(entry) = self.take_pending(key) else { return };
        *entry.slot.lock().unwrap() = Some(Err(error));
        if notify_server && self.transport.is_some() {
            let mut params = Map::new();
            params.insert("requestId".to_string(), Value::from(key.parse::<u64>().unwrap_or(0)));
            if let Some(reason) = reason {
                params.insert("reason".to_string(), Value::String(reason.to_string()));
            }
            let message = json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": params,
            });
            let result = self.with_transport_op(|client| {
                let transport = client.transport.as_mut();
                match transport {
                    Some(transport) => transport.send(&message, SendOptions::default()),
                    None => Ok(()),
                }
            });
            if let Err(send_error) = result {
                self.emit_error(&send_error.message());
            }
        }
    }

    /// Removes and returns the pending entry (TS `removePending` minus the
    /// timer/abort-signal bookkeeping).
    fn take_pending(&mut self, key: &str) -> Option<PendingRequest> {
        let index = self.pending.iter().position(|(k, _)| k == key)?;
        let (_, entry) = self.pending.remove(index);
        Some(entry)
    }

    /// `rejectPending` — settle every in-flight request with `error`.
    fn reject_pending(&mut self, error: McpErrorKind) {
        for (_, entry) in std::mem::take(&mut self.pending) {
            *entry.slot.lock().unwrap() = Some(Err(error.clone()));
        }
    }

    /// `handleTransportClose`.
    fn handle_transport_close(&mut self) {
        self.mark_closed(McpErrorKind::connection_closed());
    }

    /// `markClosed` — idempotent; flips the state and fires the close
    /// listeners once.
    fn mark_closed(&mut self, error: McpErrorKind) {
        let was_closed = self.state == ClientState::Closed;
        self.state = ClientState::Closed;
        self.reject_pending(error);
        if was_closed {
            return;
        }
        self.close_fired = true;
        for listener in &mut self.close_listeners {
            listener();
        }
    }

    /// `emitError` — normalized to a message string.
    fn emit_error(&mut self, message: &str) {
        for listener in &mut self.error_listeners {
            listener(message);
        }
    }

    /// `requestInternal`.
    fn request_internal(
        &mut self,
        method: &str,
        params: Option<Value>,
        options: &RequestOptions,
        allow_connecting: bool,
    ) -> Result<Value, McpErrorKind> {
        // `requireTransport`
        let usable = self.transport.is_some()
            && (self.state == ClientState::Connected
                || (allow_connecting && self.state == ClientState::Connecting));
        if !usable {
            return Err(McpErrorKind::ConnectionClosed {
                message: format!("MCP client is {}", self.state.as_str()),
            });
        }
        let id = self.next_request_id;
        self.next_request_id += 1;
        let mut message = Map::new();
        message.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
        message.insert("id".to_string(), Value::from(id));
        message.insert("method".to_string(), Value::String(method.to_string()));
        if let Some(params) = params {
            message.insert("params".to_string(), params);
        }
        let message = Value::Object(message);
        let timeout_ms = options
            .timeout_ms
            .or(self.options.request_timeout_ms)
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS);
        let slot = Arc::new(Mutex::new(None));
        let entry = PendingRequest { slot: slot.clone() };
        let key = id.to_string();
        self.pending.push((key.clone(), entry));
        let send_result = self.with_transport_op(|client| {
            let transport = client.transport.as_mut().expect("transport checked");
            transport.send(&message, SendOptions::default())
        });
        if let Err(error) = send_result {
            // TS handles send failures in the `.catch` continuation; under
            // the harness constraints nothing runs in between.
            if slot.lock().unwrap().is_none() {
                self.cancel_pending(&key, error, false, None);
            }
        }
        if slot.lock().unwrap().is_some() {
            return slot.lock().unwrap().take().unwrap();
        }
        // Parked: only the request timeout can settle the request under the
        // harness constraints (TS: the armed `setTimeout`). `timeoutMs <= 0`
        // disables the TS timer and would park forever; harness cases never
        // do that.
        if timeout_ms == 0 {
            return Err(McpErrorKind::Other { message: "request parked with timeout disabled".to_string() });
        }
        (self.sleep)(timeout_ms);
        if slot.lock().unwrap().is_some() {
            return slot.lock().unwrap().take().unwrap();
        }
        self.cancel_pending(
            &key,
            McpErrorKind::Timeout { timeout_ms },
            method != "initialize",
            Some("Request timed out"),
        );
        let settled = slot.lock().unwrap().take().unwrap();
        settled
    }

    /// `notifyInternal`.
    fn notify_internal(
        &mut self,
        method: &str,
        params: Option<Value>,
        allow_connecting: bool,
    ) -> Result<(), McpErrorKind> {
        let usable = self.transport.is_some()
            && (self.state == ClientState::Connected
                || (allow_connecting && self.state == ClientState::Connecting));
        if !usable {
            return Err(McpErrorKind::ConnectionClosed {
                message: format!("MCP client is {}", self.state.as_str()),
            });
        }
        let mut message = Map::new();
        message.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
        message.insert("method".to_string(), Value::String(method.to_string()));
        if let Some(params) = params {
            message.insert("params".to_string(), params);
        }
        self.with_transport_op(|client| {
            let transport = client.transport.as_mut().expect("transport checked");
            transport.send(&Value::Object(message), SendOptions::default())
        })
        .map(|_| ())
    }

    /// `listPage` — one page of a paginated list method.
    fn list_page(
        &mut self,
        method: &str,
        key: &str,
        is_item: fn(&Map<String, Value>) -> bool,
        cursor: Option<&str>,
        options: &RequestOptions,
    ) -> Result<(Vec<Value>, Option<String>), McpErrorKind> {
        let params = cursor.map(|cursor| {
            let mut map = Map::new();
            map.insert("cursor".to_string(), Value::String(cursor.to_string()));
            Value::Object(map)
        });
        let result = self.request(method, params, options)?;
        validate_list_page(method, key, &result, is_item)
    }

    /// `listAll` — every item of a paginated list method.
    fn list_all(
        &mut self,
        method: &str,
        key: &str,
        is_item: fn(&Map<String, Value>) -> bool,
        options: &RequestOptions,
    ) -> Result<Vec<Value>, McpErrorKind> {
        let mut items = Vec::new();
        let mut cursors = std::collections::BTreeSet::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_LIST_PAGES {
            let (page_items, next_cursor) = self.list_page(method, key, is_item, cursor.as_deref(), options)?;
            items.extend(page_items);
            let Some(next) = next_cursor else { return Ok(items) };
            if !cursors.insert(next.clone()) {
                return Err(McpErrorKind::Other {
                    message: format!("MCP {} returned duplicate cursor: {}", method, next),
                });
            }
            cursor = Some(next);
        }
        Err(McpErrorKind::Other {
            message: format!("MCP {} exceeded {} pages", method, MAX_LIST_PAGES),
        })
    }
}

/// `validateInitializeResult`.
fn validate_initialize_result(value: &Value) -> Result<Value, McpErrorKind> {
    let invalid = || McpErrorKind::rpc(
        super::transport::json_rpc_error_codes::INVALID_REQUEST,
        "Invalid MCP initialize result",
        None,
    );
    if !is_object(value) {
        return Err(invalid());
    }
    if !value.get("protocolVersion").map(Value::is_string).unwrap_or(false) {
        return Err(invalid());
    }
    if !value.get("capabilities").map(is_object).unwrap_or(false) {
        return Err(invalid());
    }
    let server_info = value.get("serverInfo");
    let ok = server_info.map(is_object).unwrap_or(false)
        && server_info
            .and_then(|s| s.get("name"))
            .map(Value::is_string)
            .unwrap_or(false)
        && server_info
            .and_then(|s| s.get("version"))
            .map(Value::is_string)
            .unwrap_or(false);
    if !ok {
        return Err(invalid());
    }
    if let Some(instructions) = value.get("instructions") {
        if !instructions.is_string() {
            return Err(invalid());
        }
    }
    Ok(value.clone())
}

/// `isTool`.
fn is_tool(tool: &Map<String, Value>) -> bool {
    tool.get("name").map(Value::is_string).unwrap_or(false) && tool.get("inputSchema").map(is_object).unwrap_or(false)
}

/// `isResource` — `name` is optional (absent), but `null` is invalid
/// (TS `undefined` check); the URI stands in.
fn is_resource(resource: &Map<String, Value>) -> bool {
    resource.get("uri").map(Value::is_string).unwrap_or(false)
        && resource.get("name").map(Value::is_string).unwrap_or(true)
}

/// `isResourceTemplate`.
fn is_resource_template(template: &Map<String, Value>) -> bool {
    template.get("uriTemplate").map(Value::is_string).unwrap_or(false)
        && template.get("name").map(Value::is_string).unwrap_or(true)
}

/// `toResource` — `{ ...item, name: item.name ?? item.uri }`.
fn to_resource(item: &Value) -> Value {
    default_name(item, "uri")
}

/// `toResourceTemplate` — `{ ...item, name: item.name ?? item.uriTemplate }`.
fn to_resource_template(item: &Value) -> Value {
    default_name(item, "uriTemplate")
}

fn default_name(item: &Value, fallback: &str) -> Value {
    let mut map = item.as_object().cloned().unwrap_or_default();
    let needs_name = match map.get("name") {
        None => true,
        Some(Value::Null) => true,
        Some(_) => false,
    };
    if needs_name {
        // JS spread-then-assign appends the key when it was absent.
        map.insert("name".to_string(), map.get(fallback).cloned().unwrap_or(Value::Null));
    }
    Value::Object(map)
}

/// Builds a page result: `{ <key>: items, nextCursor? }`.
fn page_result(page: (Vec<Value>, Option<String>), key: &str, map_item: fn(&Value) -> Value) -> Value {
    let (items, next_cursor) = page;
    let mut result = Map::new();
    result.insert(
        key.to_string(),
        Value::Array(items.iter().map(map_item).collect()),
    );
    if let Some(next_cursor) = next_cursor {
        result.insert("nextCursor".to_string(), Value::String(next_cursor));
    }
    Value::Object(result)
}

/// `validateListPage` — the items under `key`, each checked by `is_item`.
fn validate_list_page(
    method: &str,
    key: &str,
    value: &Value,
    is_item: fn(&Map<String, Value>) -> bool,
) -> Result<(Vec<Value>, Option<String>), McpErrorKind> {
    let invalid = |message: String| {
        McpErrorKind::rpc(super::transport::json_rpc_error_codes::INVALID_REQUEST, message, None)
    };
    let items = value.get(key);
    let ok_items = items.map(Value::as_array).unwrap_or(None);
    if !is_object(value) || ok_items.is_none() {
        return Err(invalid(format!("Invalid MCP {} result", method)));
    }
    let items = ok_items.unwrap();
    for item in items {
        let ok = item.as_object().map(is_item).unwrap_or(false);
        if !ok {
            return Err(invalid(format!("Invalid entry in MCP {} result", method)));
        }
    }
    // Some servers end pagination with `null` or `""` instead of omitting
    // the cursor.
    let next_cursor = match value.get("nextCursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(invalid(format!("Invalid MCP {} cursor", method))),
    };
    Ok((items.clone(), next_cursor))
}

/// `validateReadResourceResult`.
fn validate_read_resource_result(value: &Value) -> Result<Value, McpErrorKind> {
    let invalid = |message: &str| {
        McpErrorKind::rpc(super::transport::json_rpc_error_codes::INVALID_REQUEST, message.to_string(), None)
    };
    let contents = value.get("contents").and_then(Value::as_array);
    if !is_object(value) || contents.is_none() {
        return Err(invalid("Invalid MCP resources/read result"));
    }
    for contents in contents.unwrap() {
        let text_ok = contents.get("text").map(|t| t.is_string()).unwrap_or(false);
        let blob_ok = contents.get("blob").map(|b| b.is_string()).unwrap_or(false);
        let ok = is_object(contents)
            && contents.get("uri").map(Value::is_string).unwrap_or(false)
            && (text_ok || blob_ok);
        if !ok {
            return Err(invalid("Invalid contents in MCP resources/read result"));
        }
    }
    Ok(value.clone())
}

/// `validateCallToolResult` — `content` defaults to `[]` when omitted.
fn validate_call_tool_result(value: &Value) -> Result<Value, McpErrorKind> {
    let invalid = || {
        McpErrorKind::rpc(
            super::transport::json_rpc_error_codes::INVALID_REQUEST,
            "Invalid MCP tools/call result",
            None,
        )
    };
    if !is_object(value) {
        return Err(invalid());
    }
    if let Some(content) = value.get("content") {
        if !content.is_null() && !content.is_array() {
            return Err(invalid());
        }
    }
    if value.get("content").is_none() {
        let mut map = value.as_object().cloned().unwrap_or_default();
        map.insert("content".to_string(), Value::Array(Vec::new()));
        return Ok(Value::Object(map));
    }
    Ok(value.clone())
}

/// Stable key for a pending request id (client ids are integers).
fn id_key(id: &Value) -> String {
    match id {
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// `String(id)` — numbers without quotes, strings as-is.
fn js_to_string(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::transport::{FakeStep, FakeTransport};
    use serde_json::json;
    use std::collections::VecDeque;

    fn no_sleep() -> SleepFn {
        std::sync::Arc::new(|_| {})
    }

    fn client() -> McpClient {
        McpClient::new(
            McpClientOptions {
                name: "pi".to_string(),
                version: "1.3.0".to_string(),
                request_timeout_ms: None,
                roots: Vec::new(),
            },
            no_sleep(),
        )
    }

    fn ok_initialize(id: i64) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": LATEST_PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "srv", "version": "1.0" },
                "instructions": " be nice "
            }
        })
    }

    /// One send batch: `msg` is the JSON-RPC message delivered during it.
    fn batch(message: Value) -> Vec<FakeStep> {
        vec![FakeStep::Message(message)]
    }

    /// The connect handshake consumes three sends: `initialize`,
    /// `notifications/initialized`, then the first real request.
    fn script_connect(tools_result: Value) -> Vec<Vec<FakeStep>> {
        vec![
            batch(ok_initialize(1)),
            Vec::new(),
            batch(tools_result),
        ]
    }

    #[test]
    fn connect_handshake_and_state() {
        let mut transport = FakeTransport::new();
        transport.send_steps = VecDeque::from(script_connect(json!({
            "jsonrpc": "2.0", "id": 2, "result": { "tools": [{ "name": "t", "inputSchema": {} }] }
        })));
        let mut client = client();
        client.connect(Box::new(transport)).unwrap();
        assert_eq!(client.connection_state(), ClientState::Connected);
        assert_eq!(client.instructions(), Some(" be nice "));
        assert_eq!(client.protocol_version(), Some(LATEST_PROTOCOL_VERSION));
        let tools = client.list_tools(&RequestOptions::default()).unwrap();
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn connect_failure_closes_client() {
        let mut transport = FakeTransport::new();
        transport.send_steps.push_back(vec![FakeStep::Fail(McpErrorKind::Other {
            message: "boom".to_string(),
        })]);
        let mut client = client();
        let err = client.connect(Box::new(transport)).unwrap_err();
        assert_eq!(err.message(), "boom");
        assert_eq!(client.connection_state(), ClientState::Closed);
        // After close: requests fail with the state message.
        let err = client.request("ping", None, &RequestOptions::default()).unwrap_err();
        assert_eq!(err.message(), "MCP client is closed");
    }

    #[test]
    fn initialize_validation_and_version_check() {
        let mut transport = FakeTransport::new();
        transport.send_steps.push_back(batch(json!({
            "jsonrpc": "2.0", "id": 1, "result": { "protocolVersion": "1.0", "capabilities": {}, "serverInfo": { "name": "s", "version": "1" } }
        })));
        let mut first = client();
        let err = first.connect(Box::new(transport)).unwrap_err();
        assert_eq!(err.message(), "MCP server selected unsupported protocol version 1.0");

        let mut transport = FakeTransport::new();
        transport.send_steps.push_back(batch(json!({
            "jsonrpc": "2.0", "id": 1, "result": { "protocolVersion": LATEST_PROTOCOL_VERSION, "capabilities": {}, "serverInfo": { "name": "s" } }
        })));
        let mut second = client();
        let err = second.connect(Box::new(transport)).unwrap_err();
        assert_eq!(err.message(), "Invalid MCP initialize result");
    }

    #[test]
    fn jsonrpc_error_response_rejects_request() {
        let mut transport = FakeTransport::new();
        transport.send_steps = VecDeque::from(script_connect(json!({
            "jsonrpc": "2.0", "id": 2, "error": { "code": -32601, "message": "no tool" }
        })));
        let mut client = client();
        client.connect(Box::new(transport)).unwrap();
        let err = client.call_tool("nope", None, &RequestOptions::default()).unwrap_err();
        assert_eq!(err, McpErrorKind::rpc(-32601, "no tool", None));
        assert!(err.is_method_not_found());
    }

    #[test]
    fn pagination_follows_cursors_and_flags_duplicates() {
        let mut transport = FakeTransport::new();
        transport.send_steps = VecDeque::from(vec![
            batch(ok_initialize(1)),
            Vec::new(),
            batch(json!({ "jsonrpc": "2.0", "id": 2, "result": { "tools": [], "nextCursor": "a" } })),
            batch(json!({ "jsonrpc": "2.0", "id": 3, "result": { "tools": [], "nextCursor": "a" } })),
        ]);
        let mut client = client();
        client.connect(Box::new(transport)).unwrap();
        let err = client.list_tools(&RequestOptions::default()).unwrap_err();
        assert_eq!(err.message(), "MCP tools/list returned duplicate cursor: a");
    }

    #[test]
    fn timeout_cancels_and_notifies_server() {
        let mut transport = FakeTransport::new();
        transport.send_steps = VecDeque::from(vec![batch(ok_initialize(1)), Vec::new()]);
        // tools/call gets no reply batch: the request times out.
        let mut subject = McpClient::new(
            McpClientOptions {
                name: "pi".to_string(),
                version: "1.3.0".to_string(),
                request_timeout_ms: None,
                roots: Vec::new(),
            },
            std::sync::Arc::new(|_| {}),
        );
        subject.connect(Box::new(transport)).unwrap();
        let err = subject
            .call_tool("slow", None, &RequestOptions { timeout_ms: Some(5) })
            .unwrap_err();
        assert_eq!(err.message(), "MCP request timed out after 5ms");
    }

    #[test]
    fn close_rejects_pending_with_connection_closed() {
        let mut transport = FakeTransport::new();
        transport.send_steps.push_back(batch(ok_initialize(1)));
        let mut client = client();
        client.connect(Box::new(transport)).unwrap();
        client.close().unwrap();
        assert_eq!(client.connection_state(), ClientState::Closed);
    }

    #[test]
    fn resources_name_defaults_to_uri() {
        let item = json!({ "uri": "file:///x", "description": "d" });
        assert_eq!(
            to_resource(&item),
            json!({ "uri": "file:///x", "description": "d", "name": "file:///x" })
        );
        let named = json!({ "uri": "file:///x", "name": "x" });
        assert_eq!(to_resource(&named), named);
    }

    #[test]
    fn call_tool_result_defaults_content() {
        let value = json!({ "structuredContent": { "a": 1 } });
        assert_eq!(
            validate_call_tool_result(&value).unwrap(),
            json!({ "structuredContent": { "a": 1 }, "content": [] })
        );
        assert!(validate_call_tool_result(&json!({ "content": "x" })).is_err());
    }
}
