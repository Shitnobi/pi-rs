//! `McpServerConnection` — port of `extensions/mcp/runtime.ts` (454 LOC):
//! one configured server, reconnecting lazily when a call finds the
//! connection gone.
//!
//! The TS class is async; this port is synchronous over the
//! [`McpClient`](super::client::McpClient) seam (see `client.rs` for the
//! async→sync mapping). The pieces of TS asynchrony that surface here:
//!
//! - **Deferred connection events**: TS registers client listeners that
//!   call back into the connection synchronously
//!   (`notifications/tools/list_changed` → `refreshTools`,
//!   `onClose` → `handleClientClose`). Rust closures cannot borrow the
//!   connection, so they push [`ConnEvent`]s into a shared queue that the
//!   connection drains ([`McpServerConnection::process_pending_events`])
//!   after every client call — the same points where TS continuations
//!   resume. Under the harness constraints (see the working doc) the
//!   observable order matches.
//! - **`notifications/message`** logging stays synchronous (the log sink is
//!   shared through `Rc<RefCell<…>>`), like the TS listener.
//! - **`opening`**: the TS promise dedupes concurrent `getClient` calls;
//!   there is no concurrency at harness op boundaries, so it is omitted.
//! - Detached clients (session-expired retry, transport close) are dropped
//!   instead of retained: TS keeps them alive only through closures, and
//!   their residual behavior needs events after disconnect, which the
//!   harness cases exclude.
//!
//! Not ported (needs real transports; see TRACKING): `createDefaultTransport`
//! (stdio/streamable-http construction, `expandHome`, env/header
//! resolution) and the stderr tail (`StdioTransport` only — fake
//! transports are never stdio, so `stderrTail` stays `None`).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use pi_core::mcp_servers::{McpServerConfig, McpServerKind};

use super::client::{
    ClientEvent, ClientState, McpClient, McpClientOptions, McpErrorKind, RequestOptions, Root,
};
use super::transport::{McpTransport, SleepFn};

/// `VERSION` (config.ts) — the coding-agent package version sent as
/// `clientInfo.version`.
pub const VERSION: &str = "1.3.0";
/// `DEFAULT_TIMEOUT_SECONDS` (runtime.ts).
pub const DEFAULT_TIMEOUT_SECONDS: f64 = 60.0;
/// `STDERR_TAIL_CHARS` (runtime.ts) — unused with fake transports.
pub const STDERR_TAIL_CHARS: usize = 2_000;
/// `CONNECT_RETRY_DELAYS_MS` (runtime.ts): delays between attempts to
/// connect to an HTTP server that failed with a transient error.
pub const CONNECT_RETRY_DELAYS_MS: [u64; 2] = [250, 1_000];

/// `ServerState` (runtime.ts): `disconnected` means the connection dropped
/// (for example the stdio server exited); the next call reconnects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    Connecting,
    Connected,
    Disconnected,
    NeedsAuth,
    Failed,
    Closed,
}

impl ServerState {
    /// The TS union literal.
    pub fn as_str(&self) -> &'static str {
        match self {
            ServerState::Connecting => "connecting",
            ServerState::Connected => "connected",
            ServerState::Disconnected => "disconnected",
            ServerState::NeedsAuth => "needs-auth",
            ServerState::Failed => "failed",
            ServerState::Closed => "closed",
        }
    }
}

/// `McpServerEntry` — the fields the connection uses (the full TS entry
/// also carries `source`/`scope`/`override`, config-file concerns covered
/// by the mcp-config tranche).
#[derive(Debug, Clone)]
pub struct McpServerEntry {
    pub name: String,
    pub config: McpServerConfig,
}

/// The auth-provider seam (`extensions/mcp/oauth.ts`): fully lazy, so the
/// port models only presence (which drives `needsSignIn`) and the
/// `settled()` no-op that `close()` awaits.
#[derive(Debug, Clone, Copy, Default)]
pub struct McpAuthProvider;

impl McpAuthProvider {
    /// `settled()` — resolves when no refresh is running; there never is
    /// one without real OAuth flows.
    pub fn settled(&self) {}
}

/// `McpOAuthCredentialStore` seam: the connection calls `forServer(name,
/// url)` eagerly at construction when the server uses OAuth; nothing else
/// happens without real flows.
pub trait McpCredentialStore: Send {
    fn for_server(&mut self, _name: &str, _url: &str) {}
}

impl McpCredentialStore for () {}

/// `McpServerLog` seam: `write(server, params)` with the clock modeled by
/// the implementation (the TS harness pins `Date`). `params` is `None`
/// when the notification carried no `params` (TS `undefined`).
pub trait McpLogSink: Send {
    fn write(&mut self, server: &str, params: Option<&Value>);
}

/// The `onTools` hook: receives a snapshot of the connected server.
pub type ConnectionToolsHook = Box<dyn FnMut(&McpConnectionSnapshot) + Send>;

/// The `onChange` hook.
pub type ConnectionChangeHook = Box<dyn FnMut(&McpConnectionSnapshot) + Send>;

/// `McpTransportFactory`.
pub type TransportFactory = Box<
    dyn FnMut(&McpServerEntry, &str, Option<&McpAuthProvider>) -> Box<dyn McpTransport> + Send,
>;

/// A deferred connection event (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnEvent {
    /// `notifications/tools/list_changed` on client `seq`.
    ToolsListChanged(u64),
    /// The client `seq` closed (transport drop or `close()`).
    ClientClosed(u64),
}

/// `McpServerConnectionOptions` — the TS constructor options.
pub struct McpServerConnectionOptions {
    pub entry: McpServerEntry,
    pub cwd: String,
    pub create_transport: TransportFactory,
    pub credentials: Box<dyn McpCredentialStore>,
    /// `onTools(connection)` — invoked with a snapshot of the freshly
    /// connected server.
    pub on_tools: ConnectionToolsHook,
    /// `onChange(connection)?` — invoked with a snapshot after every state
    /// change.
    pub on_change: Option<ConnectionChangeHook>,
    pub log: Option<Box<dyn McpLogSink>>,
    /// Injected timers: connect retries, the `withClient` transient retry,
    /// and client request timeouts.
    pub sleep: SleepFn,
}

/// What `onTools`/`onChange` receive. The TS callbacks get the live
/// connection; a snapshot keeps the Rust callbacks from re-entering the
/// connection while it is being mutated (they run under the same call).
#[derive(Debug, Clone)]
pub struct McpConnectionSnapshot {
    pub name: String,
    pub state: ServerState,
    pub tools: Vec<Value>,
    pub has_resources: bool,
    pub instructions: Option<String>,
    pub error: Option<String>,
    pub oauth_url: Option<String>,
    pub timeout_ms: u64,
}

/// `McpServerConnection` — one configured server. Reconnects lazily when a
/// call finds the connection gone.
pub struct McpServerConnection {
    pub entry: McpServerEntry,
    pub state: ServerState,
    pub error: Option<String>,
    pub tools: Vec<Value>,
    /// Whether the server offers resources (`capabilities.resources`
    /// present), which the resource tools list and read on demand.
    pub has_resources: bool,
    /// Server instructions from `initialize`, describing its tools as a
    /// group.
    pub instructions: Option<String>,
    /// Last OAuth challenge from the server; never set without real OAuth
    /// flows.
    pub challenge: Option<Value>,
    client: Option<McpClient>,
    client_id: Option<u64>,
    next_client_seq: u64,
    shutdown: bool,
    /// Stderr of the last stdio server that failed to connect — always
    /// `None` with fake transports.
    stderr_tail: Option<String>,
    cwd: String,
    create_transport: TransportFactory,
    auth_provider: Option<McpAuthProvider>,
    on_tools: ConnectionToolsHook,
    on_change: Option<ConnectionChangeHook>,
    log: Option<Arc<Mutex<Box<dyn McpLogSink>>>>,
    pending_events: Arc<Mutex<VecDeque<ConnEvent>>>,
    sleep: SleepFn,
}

/// `isTransientError`: network failures and overloaded or restarting
/// servers, which are worth another attempt.
fn is_transient_error(error: &McpErrorKind) -> bool {
    match error {
        McpErrorKind::Http { status, .. } => {
            *status == 408 || *status == 429 || (*status >= 500 && *status != 501)
        }
        McpErrorKind::Type { .. } => true,
        _ => false,
    }
}

/// JS truthiness for capability and config checks.
fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

/// `usesOAuth`: HTTP servers authenticate with OAuth unless the config
/// supplies an `Authorization` header or `auth`.
fn uses_oauth(entry: &McpServerEntry) -> bool {
    if entry.config.kind != McpServerKind::Http {
        return false;
    }
    if is_truthy(entry.config.fields.get("auth")) {
        return false;
    }
    let has_authorization = entry
        .config
        .fields
        .get("headers")
        .and_then(Value::as_object)
        .map(|headers| headers.keys().any(|k| k.eq_ignore_ascii_case("authorization")))
        .unwrap_or(false);
    !has_authorization
}

/// `withoutTemplates`: servers that do not implement
/// `resources/templates/list` have no templates.
fn without_templates<T>(result: Result<T, McpErrorKind>, empty: T) -> Result<T, McpErrorKind> {
    match result {
        Err(error) if error.is_method_not_found() => Ok(empty),
        other => other,
    }
}

/// POSIX `pathToFileURL` (node:url): `file://` plus the path with every
/// byte percent-encoded unless RFC 3986 unreserved or `/`.
pub fn path_to_file_url(path: &str) -> String {
    let mut out = String::from("file://");
    for byte in path.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~' | '/') {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    out
}

/// POSIX `basename`.
pub fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some((_, base)) => base.to_string(),
        None => trimmed.to_string(),
    }
}

impl McpServerConnection {
    pub fn new(mut options: McpServerConnectionOptions) -> Self {
        let url = if uses_oauth(&options.entry) {
            options.entry.config.url().map(str::to_string)
        } else {
            None
        };
        let provider = if options.entry.config.kind == McpServerKind::Http {
            options
                .entry
                .config
                .fields
                .get("auth")
                .and_then(Value::as_object)
                .and_then(|auth| auth.get("provider"))
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        };
        let auth_provider = if let Some(url) = &url {
            // TS `createMcpAuthProvider` captures
            // `credentials.forServer(name, url)` at construction; the rest
            // is lazy.
            options.credentials.for_server(&options.entry.name, url);
            Some(McpAuthProvider)
        } else if provider.is_some() {
            // Read on every request, so the provider's refreshes apply;
            // MCP stores no copy. Presence-only in the port.
            Some(McpAuthProvider)
        } else {
            None
        };
        McpServerConnection {
            entry: options.entry,
            state: ServerState::Connecting,
            error: None,
            tools: Vec::new(),
            has_resources: false,
            instructions: None,
            challenge: None,
            client: None,
            client_id: None,
            next_client_seq: 0,
            shutdown: false,
            stderr_tail: None,
            cwd: options.cwd,
            create_transport: options.create_transport,
            auth_provider,
            on_tools: options.on_tools,
            on_change: options.on_change,
            log: options.log.map(|log| Arc::new(Mutex::new(log))),
            pending_events: Arc::new(Mutex::new(VecDeque::new())),
            sleep: options.sleep,
        }
    }

    /// The snapshot handed to `onTools`/`onChange`.
    pub fn snapshot(&self) -> McpConnectionSnapshot {
        McpConnectionSnapshot {
            name: self.entry.name.clone(),
            state: self.state,
            tools: self.tools.clone(),
            has_resources: self.has_resources,
            instructions: self.instructions.clone(),
            error: self.error.clone(),
            oauth_url: self.oauth_url(),
            timeout_ms: self.timeout_ms(),
        }
    }

    /// `name`.
    pub fn name(&self) -> &str {
        &self.entry.name
    }

    /// `timeoutMs`: `(config.timeout ?? 60) * 1000`.
    pub fn timeout_ms(&self) -> u64 {
        let seconds = self
            .entry
            .config
            .fields
            .get("timeout")
            .and_then(Value::as_f64)
            .unwrap_or(DEFAULT_TIMEOUT_SECONDS);
        (seconds * 1000.0) as u64
    }

    /// `oauthUrl`: the server URL when the server authenticates with OAuth.
    pub fn oauth_url(&self) -> Option<String> {
        if uses_oauth(&self.entry) {
            self.entry.config.url().map(str::to_string)
        } else {
            None
        }
    }

    /// `callTool`.
    pub fn call_tool(
        &mut self,
        name: &str,
        args: Option<Value>,
        options: &RequestOptions,
    ) -> Result<Value, McpErrorKind> {
        self.with_client(false, |client| client.call_tool(name, args.clone(), options))
    }

    /// `readResource`.
    pub fn read_resource(&mut self, uri: &str, options: &RequestOptions) -> Result<Value, McpErrorKind> {
        self.with_client(true, |client| client.read_resource(uri, options))
    }

    /// `resourcesPage`.
    pub fn resources_page(
        &mut self,
        cursor: Option<&str>,
        options: &RequestOptions,
    ) -> Result<Value, McpErrorKind> {
        self.with_client(true, |client| client.list_resources_page(cursor, options))
    }

    /// `resourceTemplatesPage`.
    pub fn resource_templates_page(
        &mut self,
        cursor: Option<&str>,
        options: &RequestOptions,
    ) -> Result<Value, McpErrorKind> {
        self.with_client(
            true,
            |client| without_templates(client.list_resource_templates_page(cursor, options), serde_json::json!({ "resourceTemplates": [] })),
        )
    }

    /// `allResources`.
    pub fn all_resources(&mut self, options: &RequestOptions) -> Result<Vec<Value>, McpErrorKind> {
        self.with_client(true, |client| client.list_resources(options))
    }

    /// `allResourceTemplates`.
    pub fn all_resource_templates(&mut self, options: &RequestOptions) -> Result<Vec<Value>, McpErrorKind> {
        self.with_client(
            true,
            |client| without_templates(client.list_resource_templates(options), Vec::new()),
        )
    }

    /// `reconnect`: connect again with fresh credentials, for example after
    /// signing in.
    pub fn reconnect(&mut self) -> Result<(), McpErrorKind> {
        if let Some(seq) = self.client_id {
            self.drop_client(seq);
        }
        self.get_client().map(|_| ())
    }

    /// `signOut`: disconnect after the stored credentials were removed.
    pub fn sign_out(&mut self) {
        if let Some(seq) = self.client_id {
            self.drop_client(seq);
        }
        if !self.shutdown {
            self.mark_needs_auth();
        }
    }

    /// `close`: resolves once no transport of this server is open.
    pub fn close(&mut self) {
        self.shutdown = true;
        self.state = ServerState::Closed;
        self.changed();
        if let Some(seq) = self.client_id {
            self.drop_client(seq);
        }
        if let Some(auth) = &self.auth_provider {
            auth.settled();
        }
        self.process_pending_events();
    }

    /// Harness `emit` op: delivers an external event through the current
    /// client (TS: the harness calls the transport listeners directly).
    /// No-op while disconnected (case constraint: no emits while
    /// disconnected).
    pub fn deliver_external(&mut self, event: ClientEvent) {
        if let Some(client) = self.client.as_mut() {
            client.deliver_external(event);
        }
        self.process_pending_events();
    }

    /// Whether the server authenticates with OAuth and has an auth
    /// provider armed (drives `needsSignIn`).
    pub fn has_auth_provider(&self) -> bool {
        self.auth_provider.is_some()
    }

    // ---- internals ----

    /// `withClient`: run a request, reconnecting when needed. `read_only`
    /// requests are retried once after a transient HTTP error; tool calls
    /// are not, since they may have run.
    fn with_client<T>(
        &mut self,
        read_only: bool,
        run: impl Fn(&mut McpClient) -> Result<T, McpErrorKind>,
    ) -> Result<T, McpErrorKind> {
        let mut attempt = 1;
        loop {
            let seq = self.get_client()?;
            let result = match self.client.as_mut() {
                Some(client) => run(client),
                None => unreachable!("get_client assigned a client"),
            };
            self.process_pending_events();
            let error = match result {
                Ok(value) => return Ok(value),
                Err(error) => error,
            };
            if read_only && attempt == 1 && is_transient_error(&error) {
                (self.sleep)(CONNECT_RETRY_DELAYS_MS[0]);
                attempt += 1;
                continue;
            }
            if matches!(&error, McpErrorKind::Http { session_expired: true, .. }) && attempt == 1 {
                // The server no longer knows the session (restart, deploy),
                // so it did not run the request. Retry once on a new
                // session. The old client is detached but not closed:
                // closing would fail its other in-flight calls, which
                // instead get the same 404 and retry the same way.
                if self.client_id == Some(seq) {
                    self.client = None;
                    self.client_id = None;
                }
                attempt += 1;
                continue;
            }
            if !self.needs_sign_in(&error) {
                return Err(error);
            }
            self.drop_client(seq);
            self.mark_needs_auth();
            return Err(McpErrorKind::Other { message: self.sign_in_required_message() });
        }
    }

    /// `getClient` — the connected client, opening one when needed.
    fn get_client(&mut self) -> Result<u64, McpErrorKind> {
        if self.shutdown {
            return Err(McpErrorKind::Other {
                message: format!("MCP server \"{}\" is shut down", self.entry.name),
            });
        }
        if let Some(client) = &self.client {
            if client.connection_state() == ClientState::Connected {
                return Ok(self.client_id.unwrap_or(0));
            }
        }
        // TS `opening` dedupes concurrent calls; there is no concurrency
        // at harness op boundaries.
        self.open()
    }

    /// `open`: connect, retrying transient HTTP failures.
    fn open(&mut self) -> Result<u64, McpErrorKind> {
        self.state = ServerState::Connecting;
        self.changed();
        let retries: &[u64] = if self.entry.config.kind == McpServerKind::Http {
            &CONNECT_RETRY_DELAYS_MS
        } else {
            &[]
        };
        let mut attempt = 0;
        loop {
            self.stderr_tail = None;
            match self.connect_once() {
                Ok(seq) => return Ok(seq),
                Err(error) => {
                    let delay = retries.get(attempt).copied();
                    if self.shutdown || delay.is_none() || !is_transient_error(&error) {
                        return Err(self.connect_failed(error));
                    }
                    (self.sleep)(delay.unwrap());
                    if self.shutdown {
                        return Err(self.connect_failed(error));
                    }
                }
            }
            attempt += 1;
        }
    }

    /// `connectOnce`.
    fn connect_once(&mut self) -> Result<u64, McpErrorKind> {
        self.next_client_seq += 1;
        let seq = self.next_client_seq;
        let mut client = McpClient::new(
            McpClientOptions {
                name: "pi".to_string(),
                version: VERSION.to_string(),
                request_timeout_ms: Some(self.timeout_ms()),
                roots: vec![Root {
                    uri: path_to_file_url(&self.cwd),
                    name: Some(basename(&self.cwd)),
                }],
            },
            self.sleep.clone(),
        );
        // `notifications/message` → the server log, registered before the
        // transport so messages during the handshake are captured.
        if let Some(log) = &self.log {
            let log = log.clone();
            let name = self.entry.name.clone();
            client.on_notification(
                "notifications/message",
                Box::new(move |params| log.lock().unwrap().write(&name, params)),
            );
        }
        let transport = (self.create_transport)(&self.entry, &self.cwd, self.auth_provider.as_ref());
        if let Err(error) = client.connect(transport) {
            self.process_pending_events();
            let _ = client.close();
            return Err(error);
        }
        self.process_pending_events();
        // `notifications/tools/list_changed` → refreshTools; `onClose` →
        // handleClientClose. Both defer through the pending-event queue.
        let queue = self.pending_events.clone();
        client.on_notification(
            "notifications/tools/list_changed",
            Box::new(move |_| queue.lock().unwrap().push_back(ConnEvent::ToolsListChanged(seq))),
        );
        let queue = self.pending_events.clone();
        client.on_close(Box::new(move || queue.lock().unwrap().push_back(ConnEvent::ClientClosed(seq))));
        // Servers without the tools capability (prompts or resources only)
        // do not answer tools/list.
        let tools = if is_truthy(client.server_capabilities().and_then(|c| c.get("tools"))) {
            match client.list_tools(&RequestOptions::default()) {
                Ok(tools) => tools,
                Err(error) => {
                    self.process_pending_events_with(Some(&mut client));
                    let _ = client.close();
                    return Err(error);
                }
            }
        } else {
            Vec::new()
        };
        // TS: a `tools/list_changed` notification delivered during the
        // handshake's tools/list starts a refresh whose continuation
        // attaches during the send — so it runs BEFORE this point, and its
        // result is discarded because `this.client` is not yet the
        // connecting client (the request itself is still sent and
        // observable).
        self.process_pending_events_with(Some(&mut client));
        if self.shutdown {
            let _ = client.close();
            return Err(McpErrorKind::Other { message: "shut down while connecting".to_string() });
        }
        if client.connection_state() != ClientState::Connected {
            let _ = client.close();
            return Err(McpErrorKind::Other { message: "connection closed during setup".to_string() });
        }
        self.client = Some(client);
        self.client_id = Some(seq);
        self.tools = tools;
        self.has_resources = self
            .client
            .as_ref()
            .and_then(|c| c.server_capabilities())
            .map(|c| c.get("resources").is_some())
            .unwrap_or(false);
        self.instructions = self
            .client
            .as_ref()
            .and_then(|c| c.instructions())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        self.state = ServerState::Connected;
        self.error = None;
        let snapshot = self.snapshot();
        (self.on_tools)(&snapshot);
        self.changed();
        Ok(seq)
    }

    /// `connectFailed`.
    fn connect_failed(&mut self, error: McpErrorKind) -> McpErrorKind {
        if self.needs_sign_in(&error) && !self.shutdown {
            self.mark_needs_auth();
            return McpErrorKind::Other { message: self.sign_in_required_message() };
        }
        self.state = if self.shutdown { ServerState::Closed } else { ServerState::Failed };
        self.error = Some(match &self.stderr_tail {
            // `MCP server "x" failed to connect: message\n<stderr tail>`
            Some(tail) => format!("{}\n{}", error.message(), tail),
            None => error.message(),
        });
        let message = format!(
            "MCP server \"{}\" failed to connect: {}",
            self.entry.name,
            self.error.clone().unwrap_or_default()
        );
        self.changed();
        McpErrorKind::Other { message }
    }

    /// `handleClientClose`: the transport dropped. The next call
    /// reconnects; until then the status shows why.
    fn handle_client_close(&mut self, seq: u64) {
        if self.client_id != Some(seq) || self.shutdown {
            return;
        }
        self.client = None;
        self.client_id = None;
        self.state = ServerState::Disconnected;
        // The stderr variant needs a stdio transport; fakes are never one.
        self.error = Some("Connection closed".to_string());
        self.changed();
    }

    /// `refreshTools`. `client` is the connecting client during
    /// [`McpServerConnection::connect_once`] (TS runs `listTools` on the
    /// registered client even while it is not yet `this.client`); when
    /// `None`, the current client is used when it matches `seq`.
    fn refresh_tools_with(&mut self, seq: u64, client: Option<&mut McpClient>) {
        let result = match client
            .or_else(|| self.client.as_mut().filter(|_| self.client_id == Some(seq)))
        {
            Some(client) => client.list_tools(&RequestOptions::default()),
            // TS would run on the registered (detached) client; the port
            // drops detached clients, so this is unreachable under the
            // harness constraints.
            None => return,
        };
        self.process_pending_events();
        match result {
            Ok(tools) => {
                if self.client_id != Some(seq) || self.shutdown {
                    // TS early-returns before `changed()` too.
                    return;
                }
                self.tools = tools;
                let snapshot = self.snapshot();
        (self.on_tools)(&snapshot);
            }
            Err(error) => {
                self.error = Some(format!("Failed to refresh tools: {}", error.message()));
            }
        }
        self.changed();
    }

    /// `dropClient`.
    fn drop_client(&mut self, seq: u64) {
        if self.client_id == Some(seq) {
            self.client_id = None;
            if let Some(mut client) = self.client.take() {
                let _ = client.close();
            }
        }
    }

    /// `markNeedsAuth`.
    fn mark_needs_auth(&mut self) {
        self.state = ServerState::NeedsAuth;
        self.error = None;
        self.changed();
    }

    /// `changed`.
    fn changed(&mut self) {
        if self.on_change.is_some() {
            let snapshot = self.snapshot();
            if let Some(on_change) = &mut self.on_change {
                on_change(&snapshot);
            }
        }
    }

    /// `needsSignIn`: OAuth servers that still reject the request after a
    /// refresh need the user to sign in again.
    fn needs_sign_in(&self, error: &McpErrorKind) -> bool {
        match error {
            McpErrorKind::OauthAuthorizationRequired => true,
            McpErrorKind::Http { auth_required: true, .. } => self.auth_provider.is_some(),
            _ => false,
        }
    }

    /// `signInRequiredMessage`.
    fn sign_in_required_message(&self) -> String {
        let provider = if self.entry.config.kind == McpServerKind::Http {
            self.entry
                .config
                .fields
                .get("auth")
                .and_then(Value::as_object)
                .and_then(|auth| auth.get("provider"))
                .and_then(Value::as_str)
        } else {
            None
        };
        format!(
            "MCP server \"{}\" requires sign-in. Run {} to sign in.",
            self.entry.name,
            match provider {
                Some(provider) => format!("/login {}", provider),
                None => "/mcp".to_string(),
            }
        )
    }

    /// Drains the deferred connection events in order. Processing may
    /// queue more (a refresh receives another `tools/list_changed`), so
    /// loop until empty — matching TS's re-entrant `void refreshTools`.
    /// `client` is the connecting client during `connect_once`.
    fn process_pending_events_with(&mut self, client: Option<&mut McpClient>) {
        let mut client = client;
        loop {
            let event = self.pending_events.lock().unwrap().pop_front();
            let Some(event) = event else { break };
            match event {
                ConnEvent::ToolsListChanged(seq) => {
                    self.refresh_tools_with(seq, client.as_deref_mut())
                }
                ConnEvent::ClientClosed(seq) => self.handle_client_close(seq),
            }
        }
    }

    fn process_pending_events(&mut self) {
        self.process_pending_events_with(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::client::LATEST_PROTOCOL_VERSION;
    use crate::mcp::transport::{FakeStep, FakeTransport, SendOptions};
    use serde_json::{json, Map};
    use std::collections::VecDeque;

    /// Recording sleep fn (no real sleeping).
    type Sleeps = std::sync::Arc<std::sync::Mutex<Vec<u64>>>;

    fn recorder() -> (SleepFn, Sleeps) {
        let sleeps = Sleeps::default();
        let record = sleeps.clone();
        (
            std::sync::Arc::new(move |ms| record.lock().unwrap().push(ms)),
            sleeps,
        )
    }

    fn http_entry(name: &str) -> McpServerEntry {
        let mut fields = Map::new();
        fields.insert("url".to_string(), json!("https://example.test/mcp"));
        McpServerEntry {
            name: name.to_string(),
            config: McpServerConfig { kind: McpServerKind::Http, fields },
        }
    }

    fn stdio_entry(name: &str) -> McpServerEntry {
        let mut fields = Map::new();
        fields.insert("command".to_string(), json!("run-server"));
        McpServerEntry {
            name: name.to_string(),
            config: McpServerConfig { kind: McpServerKind::Stdio, fields },
        }
    }

    fn step(message: Value) -> FakeStep {
        FakeStep::Message(message)
    }

    fn initialize_result() -> Value {
        json!({
            "protocolVersion": LATEST_PROTOCOL_VERSION,
            "capabilities": { "tools": {}, "resources": {} },
            "serverInfo": { "name": "srv", "version": "1" },
            "instructions": "  hello  "
        })
    }

    /// A full transport script: `[start_steps, send_batch_1, …]`. The
    /// connect handshake is initialize + notifications/initialized +
    /// tools/list; `extra` appends further send batches.
    fn connect_script(tools: Value, extra: Vec<Vec<FakeStep>>) -> Vec<Vec<FakeStep>> {
        let mut script = vec![
            Vec::<FakeStep>::new(),
            vec![step(json!({ "jsonrpc": "2.0", "id": 1, "result": initialize_result() }))],
            Vec::new(),
            vec![step(json!({ "jsonrpc": "2.0", "id": 2, "result": tools }))],
        ];
        script.extend(extra);
        script
    }

    /// A `Box<dyn McpTransport>` over a shared `FakeTransport`, so the
    /// harness keeps handles to the transports the factory created.
    struct SharedTransport(Arc<Mutex<FakeTransport>>);

    impl McpTransport for SharedTransport {
        fn start(&mut self) -> Result<(), McpErrorKind> {
            self.0.lock().unwrap().start()
        }
        fn send(&mut self, message: &Value, options: SendOptions) -> Result<(), McpErrorKind> {
            self.0.lock().unwrap().send(message, options)
        }
        fn close(&mut self) -> Result<(), McpErrorKind> {
            self.0.lock().unwrap().close()
        }
        fn on_message(&mut self, listener: super::super::transport::MessageListener) -> usize {
            self.0.lock().unwrap().on_message(listener)
        }
        fn on_error(&mut self, listener: super::super::transport::ErrorListener) -> usize {
            self.0.lock().unwrap().on_error(listener)
        }
        fn on_close(&mut self, listener: super::super::transport::CloseListener) -> usize {
            self.0.lock().unwrap().on_close(listener)
        }
        fn off_message(&mut self, id: usize) {
            self.0.lock().unwrap().off_message(id)
        }
        fn off_error(&mut self, id: usize) {
            self.0.lock().unwrap().off_error(id)
        }
        fn off_close(&mut self, id: usize) {
            self.0.lock().unwrap().off_close(id)
        }
        fn set_protocol_version(&mut self, version: &str) {
            self.0.lock().unwrap().set_protocol_version(version)
        }
    }

    struct Harness {
        connection: Option<McpServerConnection>,
        transports: Arc<Mutex<Vec<Arc<Mutex<FakeTransport>>>>>,
        scripts: Arc<Mutex<VecDeque<Vec<Vec<FakeStep>>>>>,
        markers: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Harness {
        fn new(entry: McpServerEntry) -> (Self, Sleeps) {
            let transports: Arc<Mutex<Vec<Arc<Mutex<FakeTransport>>>>> = Arc::new(Mutex::new(Vec::new()));
            let scripts: Arc<Mutex<VecDeque<Vec<Vec<FakeStep>>>>> = Arc::new(Mutex::new(VecDeque::new()));
            let markers: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
            let (sleep, sleeps) = recorder();
            let factory_transports = transports.clone();
            let factory_scripts = scripts.clone();
            let factory = move |_entry: &McpServerEntry,
                                _cwd: &str,
                                _auth: Option<&McpAuthProvider>|
                  -> Box<dyn McpTransport> {
                let script = factory_scripts.lock().unwrap().pop_front();
                let mut transport = FakeTransport::new();
                if let Some(script) = script {
                    transport.start_steps = script.first().cloned().unwrap_or_default();
                    transport.send_steps =
                        VecDeque::from(script.into_iter().skip(1).collect::<Vec<_>>());
                }
                let shared = Arc::new(Mutex::new(transport));
                factory_transports.lock().unwrap().push(shared.clone());
                Box::new(SharedTransport(shared))
            };
            let on_tools = {
                let markers = markers.clone();
                move |_: &McpConnectionSnapshot| markers.lock().unwrap().push("onTools")
            };
            let on_change = {
                let markers = markers.clone();
                move |_: &McpConnectionSnapshot| markers.lock().unwrap().push("onChange")
            };
            let connection = McpServerConnection::new(McpServerConnectionOptions {
                entry,
                cwd: "/tmp/pi-parity-mcc".to_string(),
                create_transport: Box::new(factory),
                credentials: Box::new(()),
                on_tools: Box::new(on_tools),
                on_change: Some(Box::new(on_change)),
                log: None,
                sleep,
            });
            (
                Harness { connection: Some(connection), transports, scripts, markers },
                sleeps,
            )
        }

        /// Scripts are consumed front-first; push the first transport's
        /// script first.
        fn push_script(&self, script: Vec<Vec<FakeStep>>) {
            self.scripts.lock().unwrap().push_back(script);
        }

        fn conn(&mut self) -> &mut McpServerConnection {
            self.connection.as_mut().unwrap()
        }

        fn sent(&self) -> Vec<Value> {
            self.transports
                .lock()
                .unwrap()
                .iter()
                .flat_map(|t| t.lock().unwrap().sent.clone())
                .collect()
        }
    }

    fn assert_send<T: Send>() {}

    #[test]
    fn connection_and_client_are_send() {
        assert_send::<McpServerConnection>();
        assert_send::<McpClient>();
        assert_send::<Box<dyn McpTransport>>();
        assert_send::<Box<dyn McpLogSink>>();
    }

    #[test]
    fn connect_happy_path() {
        let (mut h, sleeps) = Harness::new(http_entry("srv"));
        h.push_script(connect_script(
            json!({ "tools": [{ "name": "greet", "inputSchema": {} }] }),
            vec![vec![step(json!({ "jsonrpc": "2.0", "id": 3, "result": { "content": [] } }))]],
        ));
        let result = h.conn().call_tool("greet", Some(json!({ "who": "world" })), &RequestOptions::default());
        assert_eq!(result.unwrap(), json!({ "content": [] }));
        assert_eq!(h.conn().state, ServerState::Connected);
        assert_eq!(h.conn().error, None);
        assert_eq!(h.conn().instructions.as_deref(), Some("hello"));
        assert!(h.conn().has_resources);
        assert_eq!(h.conn().tools.len(), 1);
        assert_eq!(h.conn().name(), "srv");
        assert_eq!(
            *h.markers.lock().unwrap(),
            vec!["onChange", "onTools", "onChange"],
            "connecting changed, then onTools + changed"
        );
        assert!(sleeps.lock().unwrap().is_empty());
    }

    #[test]
    fn initialize_params_shape() {
        let (mut h, _) = Harness::new(http_entry("srv"));
        h.push_script(connect_script(json!({ "tools": [] }), Vec::new()));
        // callTool without a reply batch parks → timeout (fake sleep).
        let _ = h.conn().call_tool("t", None, &RequestOptions { timeout_ms: Some(1) });
        let sent = h.sent();
        assert_eq!(sent[0]["method"], "initialize");
        assert_eq!(sent[0]["params"]["protocolVersion"], LATEST_PROTOCOL_VERSION);
        assert_eq!(sent[0]["params"]["clientInfo"], json!({ "name": "pi", "version": "1.3.0" }));
        assert_eq!(sent[0]["params"]["capabilities"], json!({ "roots": {} }));
        assert_eq!(sent[1]["method"], "notifications/initialized");
        assert_eq!(sent[2]["method"], "tools/list");
        assert_eq!(sent[0]["params"].get("roots"), None, "roots stay client-side");
    }

    #[test]
    fn roots_list_handler_uses_cwd() {
        let (mut h, _) = Harness::new(http_entry("srv"));
        h.push_script(connect_script(json!({ "tools": [] }), Vec::new()));
        let _ = h.conn().call_tool("t", None, &RequestOptions { timeout_ms: Some(1) });
        h.conn().deliver_external(ClientEvent::Message(json!({
            "jsonrpc": "2.0", "id": 99, "method": "roots/list"
        })));
        let last = h.sent().last().unwrap().clone();
        assert_eq!(last["id"], 99);
        assert_eq!(
            last["result"]["roots"],
            json!([{ "uri": "file:///tmp/pi-parity-mcc", "name": "pi-parity-mcc" }])
        );
    }

    #[test]
    fn connect_failure_marks_failed() {
        let (mut h, _) = Harness::new(http_entry("srv"));
        h.push_script(vec![vec![FakeStep::Fail(McpErrorKind::Other { message: "boom".to_string() })]]);
        let err = h.conn().call_tool("t", None, &RequestOptions::default()).unwrap_err();
        assert_eq!(err.message(), "MCP server \"srv\" failed to connect: boom");
        assert_eq!(h.conn().state, ServerState::Failed);
        assert_eq!(h.conn().error.as_deref(), Some("boom"));
    }

    #[test]
    fn http_connect_retries_transient_errors() {
        let (mut h, sleeps) = Harness::new(http_entry("srv"));
        // Scripts are FIFO: the failing transport is consumed first, then
        // the retry's.
        h.push_script(vec![vec![FakeStep::Fail(McpErrorKind::Type { message: "fetch failed".to_string() })]]);
        h.push_script(connect_script(
            json!({ "tools": [] }),
            vec![vec![step(json!({ "jsonrpc": "2.0", "id": 3, "result": { "content": [] } }))]],
        ));
        let _ = h.conn().call_tool("t", None, &RequestOptions::default());
        assert_eq!(h.conn().state, ServerState::Connected);
        assert_eq!(*sleeps.lock().unwrap(), vec![250]);
        assert_eq!(h.transports.lock().unwrap().len(), 2);
    }

    #[test]
    fn stdio_connect_does_not_retry() {
        let (mut h, sleeps) = Harness::new(stdio_entry("srv"));
        h.push_script(vec![vec![FakeStep::Fail(McpErrorKind::Type { message: "fetch failed".to_string() })]]);
        let err = h.conn().call_tool("t", None, &RequestOptions::default()).unwrap_err();
        assert_eq!(err.message(), "MCP server \"srv\" failed to connect: fetch failed");
        assert_eq!(h.conn().state, ServerState::Failed);
        assert!(sleeps.lock().unwrap().is_empty());
    }

    #[test]
    fn auth_required_with_provider_marks_needs_auth() {
        let (mut h, _) = Harness::new(http_entry("srv"));
        // A url entry without auth or an Authorization header arms the
        // auth provider (usesOAuth).
        assert!(h.conn().has_auth_provider());
        h.push_script(vec![vec![FakeStep::Fail(McpErrorKind::Http {
            status: 401,
            message: "MCP server requires authentication".to_string(),
            body: String::new(),
            auth_required: true,
            session_expired: false,
        })]]);
        let err = h.conn().call_tool("t", None, &RequestOptions::default()).unwrap_err();
        assert_eq!(err.message(), "MCP server \"srv\" requires sign-in. Run /mcp to sign in.");
        assert_eq!(h.conn().state, ServerState::NeedsAuth);
        assert_eq!(h.conn().error, None);
    }

    #[test]
    fn oauth_required_marks_needs_auth() {
        let (mut h, _) = Harness::new(stdio_entry("srv"));
        // stdio → no auth provider, but OauthAuthorizationRequired always
        // needs sign-in.
        assert!(!h.conn().has_auth_provider());
        h.push_script(vec![vec![FakeStep::Fail(McpErrorKind::OauthAuthorizationRequired)]]);
        let err = h.conn().call_tool("t", None, &RequestOptions::default()).unwrap_err();
        assert_eq!(err.message(), "MCP server \"srv\" requires sign-in. Run /mcp to sign in.");
        assert_eq!(h.conn().state, ServerState::NeedsAuth);
    }

    #[test]
    fn session_expired_retries_on_new_client() {
        let (mut h, _) = Harness::new(http_entry("srv"));
        // Transport 1: connects, then callTool fails with session expired.
        h.push_script(connect_script(
            json!({ "tools": [] }),
            vec![vec![FakeStep::Fail(McpErrorKind::Http {
                status: 404,
                message: "MCP session expired".to_string(),
                body: String::new(),
                auth_required: false,
                session_expired: true,
            })]],
        ));
        // Transport 2: the retry connects a fresh client that answers.
        h.push_script(connect_script(
            json!({ "tools": [] }),
            vec![vec![step(json!({ "jsonrpc": "2.0", "id": 3, "result": { "content": [] } }))]],
        ));
        let result = h.conn().call_tool("t", None, &RequestOptions::default());
        assert!(result.is_ok(), "{:?}", result.err());
        assert_eq!(h.conn().state, ServerState::Connected);
        assert_eq!(h.transports.lock().unwrap().len(), 2);
    }

    #[test]
    fn transport_close_mid_op_marks_disconnected() {
        let (mut h, _) = Harness::new(http_entry("srv"));
        h.push_script(connect_script(json!({ "tools": [] }), vec![vec![FakeStep::Close]]));
        let err = h.conn().call_tool("t", None, &RequestOptions { timeout_ms: Some(1) }).unwrap_err();
        // The close rejects the pending request with the default message.
        assert_eq!(err.message(), "MCP connection closed");
        assert_eq!(h.conn().state, ServerState::Disconnected);
        assert_eq!(h.conn().error.as_deref(), Some("Connection closed"));
        assert_eq!(*h.markers.lock().unwrap(), vec!["onChange", "onTools", "onChange", "onChange"]);
    }

    #[test]
    fn close_then_call_rejects_shut_down() {
        let (mut h, _) = Harness::new(http_entry("srv"));
        h.conn().close();
        assert_eq!(h.conn().state, ServerState::Closed);
        let err = h.conn().call_tool("t", None, &RequestOptions::default()).unwrap_err();
        assert_eq!(err.message(), "MCP server \"srv\" is shut down");
    }

    #[test]
    fn sign_out_marks_needs_auth() {
        let (mut h, _) = Harness::new(http_entry("srv"));
        h.push_script(connect_script(json!({ "tools": [] }), Vec::new()));
        let _ = h.conn().call_tool("t", None, &RequestOptions { timeout_ms: Some(1) });
        h.conn().sign_out();
        assert_eq!(h.conn().state, ServerState::NeedsAuth);
        assert_eq!(h.conn().error, None);
    }

    #[test]
    fn tools_list_changed_refreshes_tools() {
        let (mut h, _) = Harness::new(http_entry("srv"));
        // tools/list batch carries a list_changed notification; the
        // refresh's own tools/list is the next send batch. TS: the refresh
        // continuation attaches DURING the handshake's tools/list send, so
        // it runs before `this.client = client` — the refresh request IS
        // sent (observable) but `this.client !== client` discards its
        // result (no onTools/onChange; tools stay []). Verified
        // differentially by mcc/tools-list-changed-refresh.
        let mut script = vec![
            Vec::<FakeStep>::new(),
            vec![step(json!({ "jsonrpc": "2.0", "id": 1, "result": initialize_result() }))],
            Vec::new(),
            vec![
                step(json!({ "jsonrpc": "2.0", "id": 2, "result": { "tools": [] } })),
                step(json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" })),
            ],
            // The refresh's tools/list reply (consumed, then discarded).
            vec![step(json!({ "jsonrpc": "2.0", "id": 3, "result": { "tools": [{ "name": "new", "inputSchema": {} }] } }))],
        ];
        // The op's own request (callTool timeout 1ms, no reply).
        script.push(Vec::new());
        h.push_script(script);
        let _ = h.conn().call_tool("t", None, &RequestOptions { timeout_ms: Some(1) });
        assert_eq!(h.conn().tools.len(), 0);
        // 6 sends: initialize, initialized, the handshake's tools/list,
        // the refresh's (discarded) tools/list, the callTool (no reply —
        // 1ms timeout), and the notifications/cancelled it triggers
        // (missing batch → silent success).
        assert_eq!(h.sent().len(), 6);
        assert_eq!(h.sent()[3]["method"], "tools/list");
        assert_eq!(h.sent()[5]["method"], "notifications/cancelled");
        assert_eq!(h.sent()[5]["params"]["requestId"], 4);
        assert_eq!(
            *h.markers.lock().unwrap(),
            vec!["onChange", "onTools", "onChange"]
        );
    }

    #[test]
    fn reconnect_uses_a_new_transport() {
        let (mut h, _) = Harness::new(http_entry("srv"));
        h.push_script(connect_script(json!({ "tools": [] }), Vec::new()));
        let _ = h.conn().call_tool("t", None, &RequestOptions { timeout_ms: Some(1) });
        h.push_script(connect_script(json!({ "tools": [{ "name": "x", "inputSchema": {} }] }), Vec::new()));
        let _ = h.conn().reconnect();
        assert_eq!(h.conn().state, ServerState::Connected);
        assert_eq!(h.conn().tools.len(), 1);
        assert_eq!(h.transports.lock().unwrap().len(), 2);
    }

    #[test]
    fn path_and_basename_helpers() {
        assert_eq!(path_to_file_url("/tmp/pi-parity-mcc"), "file:///tmp/pi-parity-mcc");
        assert_eq!(path_to_file_url("/tmp/a b"), "file:///tmp/a%20b");
        assert_eq!(basename("/tmp/pi-parity-mcc"), "pi-parity-mcc");
        assert_eq!(basename("/tmp/x/"), "x");
        assert_eq!(basename("x"), "x");
    }
}
