//! The LSP session client.
//!
//! [`Client`] spawns (or is handed) an LSP server's stdio, performs the
//! `initialize`/`initialized` handshake, captures the server's advertised
//! capabilities, correlates requests with responses by JSON-RPC id, applies a
//! per-request timeout, and detects server exit.
//!
//! Two background tasks own the I/O: a reader and a writer.
//!
//! - The **reader** reads framed messages and dispatches by shape. Responses
//!   resolve the pending `oneshot` keyed by [`Id`]; server-to-client
//!   notifications are forwarded to a [`tokio::sync::broadcast`] channel for
//!   later change requests; server-to-client requests are answered with
//!   `MethodNotFound`. On EOF it marks the client dead and fails every pending
//!   request with [`Error::ServerExited`].
//! - The **writer** drains an `mpsc` channel so frames are never interleaved.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lsp_types::{InitializeParams, InitializeResult, ServerCapabilities};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use super::message::{self, ErrorObject, Id, Notification, Response};
use super::transport;
use crate::error::{Error, Result};

/// The default per-request timeout.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The capacity of the server-notification broadcast channel.
const NOTIFICATION_CAPACITY: usize = 256;

/// State shared between the client and its reader/writer tasks.
struct Shared {
    /// Set once the server has exited or its stdio has closed.
    dead: bool,
    /// Pending requests, keyed by the id that will resolve them.
    pending: HashMap<Id, oneshot::Sender<Result<Value>>>,
}

/// An LSP session over a pair of async streams.
///
/// `R` is the stream the client reads the server's output from and `W` is the
/// stream it writes the server's input to. Production code uses
/// [`Client::spawn`], which wires a child process's stdio; tests use
/// [`Client::new`] over a `tokio::io::duplex` pair.
pub struct Client<R, W> {
    outgoing: mpsc::UnboundedSender<Vec<u8>>,
    shared: Arc<Mutex<Shared>>,
    next_id: AtomicI64,
    notifications: broadcast::Sender<Notification>,
    capabilities: Mutex<Option<ServerCapabilities>>,
    timeout: Duration,
    /// The spawned server, if any. Dropping it kills the child (`kill_on_drop`).
    _child: Option<Child>,
    reader_handle: JoinHandle<()>,
    writer_handle: JoinHandle<()>,
    _io: PhantomData<fn() -> (R, W)>,
}

// Requests, notifications, and queries need no bounds on the stream types; only
// construction (which spawns the I/O tasks) does.
impl<R, W> Client<R, W> {
    /// Perform the `initialize` handshake and capture the server's capabilities.
    pub async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let value = self
            .request(
                message::methods::INITIALIZE,
                Some(serde_json::to_value(params)?),
            )
            .await?;
        let result: InitializeResult = serde_json::from_value(value)?;
        *self.capabilities.lock().unwrap() = Some(result.capabilities.clone());
        Ok(result)
    }

    /// Send the `initialized` notification that completes the handshake.
    pub async fn initialized(&self) -> Result<()> {
        self.notify(message::methods::INITIALIZED, Some(serde_json::json!({})))
    }

    /// Send a request and await its response under the per-request timeout.
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = Id::Number(self.next_id.fetch_add(1, Ordering::SeqCst));
        let (sender, receiver) = oneshot::channel();
        {
            let mut shared = self.shared.lock().unwrap();
            if shared.dead {
                return Err(Error::ServerExited);
            }
            shared.pending.insert(id.clone(), sender);
        }

        if let Err(err) = self.send(&message::request(id.clone(), method, params)) {
            self.shared.lock().unwrap().pending.remove(&id);
            return Err(err);
        }

        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(result)) => result,
            // The sender was dropped without a value: the client is dead.
            Ok(Err(_recv)) => Err(Error::ServerExited),
            Err(_elapsed) => {
                self.shared.lock().unwrap().pending.remove(&id);
                Err(Error::Timeout(self.timeout))
            }
        }
    }

    /// Send a notification (no response is expected or awaited).
    pub fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        self.send(&message::notification(method, params))
    }

    /// Subscribe to server-to-client notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.notifications.subscribe()
    }

    /// The server capabilities captured during the handshake, if it has run.
    pub fn capabilities(&self) -> Option<ServerCapabilities> {
        self.capabilities.lock().unwrap().clone()
    }

    /// Whether the server is still reachable (its stdio has not closed).
    pub fn is_alive(&self) -> bool {
        !self.shared.lock().unwrap().dead
    }

    fn send<T: Serialize>(&self, message: &T) -> Result<()> {
        let body = serde_json::to_vec(message)?;
        self.outgoing.send(body).map_err(|_| Error::ServerExited)
    }
}

impl<R, W> Client<R, W>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    /// Build a client over a reader and a writer, starting its I/O tasks.
    pub fn new(reader: R, writer: W, timeout: Duration) -> Self {
        let (outgoing, outgoing_rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Mutex::new(Shared {
            dead: false,
            pending: HashMap::new(),
        }));
        let (notifications, _) = broadcast::channel(NOTIFICATION_CAPACITY);

        let reader_handle = tokio::spawn(reader_task(
            reader,
            Arc::clone(&shared),
            notifications.clone(),
            outgoing.clone(),
        ));
        let writer_handle = tokio::spawn(writer_task(writer, outgoing_rx, Arc::clone(&shared)));

        Client {
            outgoing,
            shared,
            next_id: AtomicI64::new(1),
            notifications,
            capabilities: Mutex::new(None),
            timeout,
            _child: None,
            reader_handle,
            writer_handle,
            _io: PhantomData,
        }
    }
}

impl Client<ChildStdout, ChildStdin> {
    /// Spawn an LSP server and complete the `initialize`/`initialized` handshake.
    pub async fn spawn(command: &str, args: &[String]) -> Result<Self> {
        let mut child = transport::spawn(command, args)?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Protocol("server stdout was not captured".to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Protocol("server stdin was not captured".to_string()))?;

        let client = Client::new(stdout, stdin, DEFAULT_REQUEST_TIMEOUT);
        let mut client = client;
        client._child = Some(child);

        let params = message::initialize_params(Some(std::process::id()));
        client.initialize(params).await?;
        client.initialized().await?;
        Ok(client)
    }
}

impl<R, W> Drop for Client<R, W> {
    fn drop(&mut self) {
        self.reader_handle.abort();
        self.writer_handle.abort();
        // `_child`, dropped after this method, kills the server via `kill_on_drop`.
    }
}

async fn reader_task<R>(
    mut reader: R,
    shared: Arc<Mutex<Shared>>,
    notifications: broadcast::Sender<Notification>,
    outgoing: mpsc::UnboundedSender<Vec<u8>>,
) where
    R: AsyncRead + Unpin + Send,
{
    loop {
        match transport::read_frame(&mut reader).await {
            Ok(Some(body)) => dispatch(&body, &shared, &notifications, &outgoing),
            // Clean EOF at a message boundary: the server has closed its output.
            Ok(None) => break,
            // A framing or I/O error means the session is unusable.
            Err(_) => break,
        }
    }
    fail_all(&shared);
}

fn dispatch(
    body: &[u8],
    shared: &Arc<Mutex<Shared>>,
    notifications: &broadcast::Sender<Notification>,
    outgoing: &mpsc::UnboundedSender<Vec<u8>>,
) {
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return,
    };
    let object = match value.as_object() {
        Some(object) => object,
        None => return,
    };
    let has_id = object.contains_key("id");
    let has_method = object.contains_key("method");

    if has_id && has_method {
        // A server-to-client request. Answer it so the server is not left
        // waiting; the adapter has no handlers for these yet.
        if let Some(id) = object.get("id").and_then(parse_id) {
            let response = Response {
                jsonrpc: message::JSONRPC_VERSION.to_string(),
                id,
                result: None,
                error: Some(ErrorObject {
                    code: message::METHOD_NOT_FOUND,
                    message: "method not found".to_string(),
                    data: None,
                }),
            };
            if let Ok(bytes) = serde_json::to_vec(&response) {
                let _ = outgoing.send(bytes);
            }
        }
    } else if has_id {
        // A response to one of our requests.
        let id = match object.get("id").and_then(parse_id) {
            Some(id) => id,
            None => return,
        };
        let result = match object.get("error") {
            Some(error) if !error.is_null() => {
                let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
                    .to_string();
                Err(Error::JsonRpc { code, message })
            }
            _ => Ok(object.get("result").cloned().unwrap_or(Value::Null)),
        };
        if let Some(sender) = shared.lock().unwrap().pending.remove(&id) {
            let _ = sender.send(result);
        }
    } else if has_method {
        // A server-to-client notification.
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = object.get("params").cloned();
        let _ = notifications.send(Notification {
            jsonrpc: message::JSONRPC_VERSION.to_string(),
            method,
            params,
        });
    }
}

fn parse_id(value: &Value) -> Option<Id> {
    if let Some(number) = value.as_i64() {
        Some(Id::Number(number))
    } else {
        value.as_str().map(|s| Id::String(s.to_string()))
    }
}

async fn writer_task<W>(
    mut writer: W,
    mut outgoing: mpsc::UnboundedReceiver<Vec<u8>>,
    shared: Arc<Mutex<Shared>>,
) where
    W: AsyncWrite + Unpin + Send,
{
    while let Some(body) = outgoing.recv().await {
        if transport::write_frame(&mut writer, &body).await.is_err() {
            break;
        }
    }
    let _ = tokio::io::AsyncWriteExt::shutdown(&mut writer).await;
    fail_all(&shared);
}

/// Mark the client dead and fail every pending request.
fn fail_all(shared: &Arc<Mutex<Shared>>) {
    let mut shared = shared.lock().unwrap();
    shared.dead = true;
    for (_id, sender) in shared.pending.drain() {
        let _ = sender.send(Err(Error::ServerExited));
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use lsp_types::{
        GotoDefinitionParams, OneOf, Position, TextDocumentIdentifier, TextDocumentPositionParams,
        WorkDoneProgressParams,
    };
    use serde_json::json;
    use tokio::io::DuplexStream;

    use super::*;
    use crate::lsp::documents::Documents;

    const BUFFER: usize = 64 * 1024;

    /// A small in-harness fake LSP server driven over a duplex pair.
    async fn run_fake_server(mut reader: DuplexStream, mut writer: DuplexStream) {
        let mut held: Vec<(Value, Value)> = Vec::new();
        let mut probe_id: Option<i64> = None;

        loop {
            let frame = match transport::read_frame(&mut reader).await {
                Ok(Some(frame)) => frame,
                Ok(None) | Err(_) => break,
            };
            let message: Value = match serde_json::from_slice(&frame) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let method = message
                .get("method")
                .and_then(Value::as_str)
                .map(str::to_string);
            let id = message.get("id").cloned();
            let params = message.get("params").cloned().unwrap_or(Value::Null);

            match method.as_deref() {
                Some("initialize") => {
                    write_value(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": id.unwrap(),
                            "result": {"capabilities": {
                                "definitionProvider": true,
                                "hoverProvider": true,
                                "textDocumentSync": 1
                            }}
                        }),
                    )
                    .await;
                    // A server-to-client notification for later change requests.
                    write_value(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "method": "window/logMessage",
                            "params": {"type": 3, "message": "fake server ready"}
                        }),
                    )
                    .await;
                    // A server-to-client request, which the client must answer
                    // with MethodNotFound.
                    probe_id = Some(4242);
                    write_value(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": 4242,
                            "method": "workspace/applyEdit",
                            "params": {}
                        }),
                    )
                    .await;
                }
                Some("initialized") => {}
                Some("textDocument/definition") => {
                    let uri = params["textDocument"]["uri"].clone();
                    let result = json!([{
                        "uri": uri,
                        "range": {
                            "start": {"line": 1, "character": 2},
                            "end": {"line": 1, "character": 5}
                        }
                    }]);
                    write_value(
                        &mut writer,
                        &json!({"jsonrpc": "2.0", "id": id.unwrap(), "result": result}),
                    )
                    .await;
                }
                Some("test/echo") => {
                    write_value(
                        &mut writer,
                        &json!({"jsonrpc": "2.0", "id": id.unwrap(), "result": params}),
                    )
                    .await;
                }
                Some("test/defer") => {
                    held.push((id.unwrap(), params));
                    if held.len() >= 2 {
                        // Reply in reverse order to prove correlation is by id.
                        for (id, params) in held.drain(..).rev() {
                            write_value(
                                &mut writer,
                                &json!({"jsonrpc": "2.0", "id": id, "result": params}),
                            )
                            .await;
                        }
                    }
                }
                Some("test/hang") => {}
                Some("test/exit") => break,
                Some(_) => {
                    if let Some(id) = id {
                        write_value(
                            &mut writer,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": {"code": message::METHOD_NOT_FOUND, "message": "method not found"}
                            }),
                        )
                        .await;
                    }
                }
                None => {
                    // A response from the client, answering our probe request.
                    if let (Some(probe), Some(id)) = (probe_id, id.as_ref().and_then(Value::as_i64))
                    {
                        if probe == id {
                            let code = message
                                .get("error")
                                .and_then(|e| e.get("code"))
                                .and_then(Value::as_i64);
                            write_value(
                                &mut writer,
                                &json!({
                                    "jsonrpc": "2.0",
                                    "method": "test/methodNotFoundSeen",
                                    "params": {"ok": code == Some(message::METHOD_NOT_FOUND)}
                                }),
                            )
                            .await;
                            probe_id = None;
                        }
                    }
                }
            }
        }
    }

    async fn write_value(writer: &mut DuplexStream, value: &Value) {
        let body = serde_json::to_vec(value).unwrap();
        transport::write_frame(writer, &body).await.unwrap();
    }

    fn connected(timeout: Duration) -> Client<DuplexStream, DuplexStream> {
        let (client_writer, server_reader) = tokio::io::duplex(BUFFER);
        let (server_writer, client_reader) = tokio::io::duplex(BUFFER);
        tokio::spawn(run_fake_server(server_reader, server_writer));
        Client::new(client_reader, client_writer, timeout)
    }

    async fn handshake(client: &Client<DuplexStream, DuplexStream>) {
        client
            .initialize(message::initialize_params(Some(1)))
            .await
            .unwrap();
        client.initialized().await.unwrap();
    }

    #[tokio::test]
    async fn handshake_captures_server_capabilities() {
        let client = connected(Duration::from_secs(2));
        let result = client
            .initialize(message::initialize_params(Some(1)))
            .await
            .unwrap();
        assert_eq!(
            result.capabilities.definition_provider,
            Some(OneOf::Left(true))
        );
        let capabilities = client.capabilities().expect("capabilities captured");
        assert_eq!(capabilities.definition_provider, Some(OneOf::Left(true)));
        assert_eq!(capabilities.hover_provider, Some(true.into()));
    }

    #[tokio::test]
    async fn definition_request_succeeds_after_did_open() {
        let client = connected(Duration::from_secs(2));
        handshake(&client).await;

        let path = Path::new("/tmp/lsp-mcp-tests/example.rs");
        let mut documents = Documents::new();
        documents
            .ensure_open(&client, path, "fn main() {}")
            .unwrap();
        let uri = crate::lsp::path_to_uri(path).unwrap();
        assert!(documents.is_open(&uri));

        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position::new(0, 0),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
        };
        let result = client
            .request(
                message::methods::DEFINITION,
                Some(serde_json::to_value(params).unwrap()),
            )
            .await
            .unwrap();
        let locations = result.as_array().expect("an array of locations");
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0]["range"]["start"]["line"], 1);
    }

    #[tokio::test]
    async fn two_in_flight_requests_correlate_by_id() {
        let client = connected(Duration::from_secs(2));
        let first = client.request("test/defer", Some(json!("first")));
        let second = client.request("test/defer", Some(json!("second")));
        let (first, second) = tokio::join!(first, second);
        assert_eq!(first.unwrap(), json!("first"));
        assert_eq!(second.unwrap(), json!("second"));
    }

    #[tokio::test]
    async fn request_past_its_timeout_fails_cleanly() {
        let client = connected(Duration::from_millis(200));
        handshake(&client).await;
        let err = client.request("test/hang", None).await.unwrap_err();
        assert!(matches!(err, Error::Timeout(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn server_exit_mid_session_fails_pending_requests() {
        let client = connected(Duration::from_secs(2));
        handshake(&client).await;
        let err = client.request("test/exit", None).await.unwrap_err();
        assert!(matches!(err, Error::ServerExited), "got {err:?}");
        assert!(!client.is_alive());
    }

    #[tokio::test]
    async fn error_response_becomes_a_json_rpc_error() {
        let client = connected(Duration::from_secs(2));
        handshake(&client).await;
        let err = client.request("test/unknown", None).await.unwrap_err();
        match err {
            Error::JsonRpc { code, message } => {
                assert_eq!(code, message::METHOD_NOT_FOUND);
                assert_eq!(message, "method not found");
            }
            other => panic!("expected JsonRpc, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn notifications_are_forwarded_and_server_requests_answered() {
        let client = connected(Duration::from_secs(2));
        let mut notifications = client.subscribe();
        handshake(&client).await;

        let mut saw_log_message = false;
        let mut saw_method_not_found = false;
        for _ in 0..4 {
            let received = tokio::time::timeout(Duration::from_secs(2), notifications.recv()).await;
            let note = match received {
                Ok(Ok(note)) => note,
                _ => break,
            };
            match note.method.as_str() {
                "window/logMessage" => saw_log_message = true,
                "test/methodNotFoundSeen" => {
                    saw_method_not_found = true;
                    assert_eq!(note.params.unwrap()["ok"], json!(true));
                }
                _ => {}
            }
            if saw_log_message && saw_method_not_found {
                break;
            }
        }
        assert!(saw_log_message, "expected the logMessage notification");
        assert!(
            saw_method_not_found,
            "expected the client to answer the server request with MethodNotFound"
        );
    }
}
