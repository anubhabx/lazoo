//! A private, per-session MCP transport for native provider clients.
//!
//! This module deliberately owns no collaboration state.  It merely transports a declared tool
//! call to the `TurnEventObserver` borrowed by the active turn.  Consequently a stale HTTP
//! request cannot acquire the authority of a later turn.

use std::{
    convert::Infallible,
    fmt::{self, Write as _},
    future::Future,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{
    Method, Request, Response, StatusCode,
    body::Incoming,
    header::{AUTHORIZATION, CONNECTION, CONTENT_TYPE, ORIGIN},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, mpsc, oneshot, watch},
    time::timeout,
};
use uuid::Uuid;

use crate::{ProviderError, ProviderKind, ToolCall, ToolResult, ToolSpec, TurnEventObserver};

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_QUEUE: usize = 16;
const MAX_CONNECTIONS: usize = 8;
#[cfg(not(test))]
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const HEADER_READ_TIMEOUT: Duration = Duration::from_millis(50);
/// Bounds only parsing an already accepted HTTP body; it never limits collaboration work.
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(15);
/// Matches `TurnRequest::new`'s default turn timeout. The active turn itself can finish or be
/// cancelled sooner; this is solely a fail-closed ceiling for a stuck observer callback.
const MAX_OBSERVER_DURATION: Duration = Duration::from_secs(300);

type JsonResponse = Response<Full<Bytes>>;

struct QueuedCall {
    call: ToolCall,
    response: oneshot::Sender<ToolResult>,
}

struct ActiveTurn {
    epoch: u64,
    session_id: String,
    turn_id: Option<String>,
    sender: mpsc::Sender<QueuedCall>,
    cancelled: watch::Sender<bool>,
}

struct State {
    active: Option<ActiveTurn>,
    next_epoch: u64,
    closed: bool,
}

struct Shared {
    provider: ProviderKind,
    endpoint: String,
    authorization: String,
    tools: Vec<ToolSpec>,
    connections: Arc<Semaphore>,
    state: Mutex<State>,
}

/// An authenticated listener which exists for the lifetime of one provider session.
pub(crate) struct SessionMcpBridge {
    shared: Arc<Shared>,
    shutdown: watch::Sender<bool>,
}

impl fmt::Debug for SessionMcpBridge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Tool names and bearer material can reveal capabilities and authority boundaries.
        formatter
            .debug_struct("SessionMcpBridge")
            .field("provider", &self.shared.provider)
            .field("endpoint", &self.shared.endpoint)
            .field("capabilities", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl SessionMcpBridge {
    pub(crate) async fn open(
        provider: ProviderKind,
        tools: Vec<ToolSpec>,
    ) -> Result<Self, ProviderError> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let shared = Arc::new(Shared {
            provider,
            endpoint: endpoint(address),
            authorization: bearer(),
            tools,
            connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
            state: Mutex::new(State {
                active: None,
                next_epoch: 0,
                closed: false,
            }),
        });
        let task_shared = Arc::clone(&shared);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() { break; }
                    }
                    accepted = listener.accept() => match accepted {
                        Ok((stream, peer)) if peer.ip().is_loopback() => {
                            let Ok(permit) = Arc::clone(&task_shared.connections).try_acquire_owned() else {
                                // Drop excess connections before parsing any input.
                                continue;
                            };
                            let service_shared = Arc::clone(&task_shared);
                            let mut connection_shutdown = shutdown_rx.clone();
                            tokio::spawn(async move {
                                let _permit = permit;
                                let mut builder = http1::Builder::new();
                                builder
                                    .timer(TokioTimer::new())
                                    .header_read_timeout(HEADER_READ_TIMEOUT)
                                    .keep_alive(false)
                                    .max_headers(32)
                                    .max_buf_size(MAX_BODY_BYTES);
                                let connection = builder.serve_connection(TokioIo::new(stream), service_fn(move |request| {
                                        handle(Arc::clone(&service_shared), request)
                                    }));
                                tokio::select! {
                                    _ = connection => {},
                                    _ = connection_shutdown.changed() => {},
                                }
                            });
                        }
                        Ok(_) => {}
                        Err(_) if *shutdown_rx.borrow() => break,
                        Err(_) => break,
                    },
                }
            }
        });
        Ok(Self { shared, shutdown })
    }

    #[must_use]
    pub(crate) fn endpoint(&self) -> &str {
        &self.shared.endpoint
    }

    /// This value is only for the provider's private launch configuration.  It is intentionally
    /// excluded from `Debug` and every protocol response.
    #[must_use]
    pub(crate) fn authorization(&self) -> String {
        self.shared.authorization.clone()
    }

    /// Binds a provider's first authoritative native id to an active pending session. This is
    /// deliberately one-way: queued calls carrying the old id are rejected at admission.
    pub(crate) fn rebind_pending_session(
        &self,
        expected_pending_session_id: &str,
        native_session_id: &str,
    ) -> Result<(), ProviderError> {
        if !expected_pending_session_id.starts_with("pending:")
            || native_session_id.is_empty()
            || native_session_id.starts_with("pending:")
            || native_session_id == expected_pending_session_id
        {
            return Err(ProviderError::Protocol(
                "invalid pending MCP session rebind".to_owned(),
            ));
        }
        let mut state = lock(&self.shared.state)?;
        if state.closed {
            return Err(ProviderError::Protocol("MCP bridge is closed".to_owned()));
        }
        let Some(active) = state.active.as_mut() else {
            return Err(ProviderError::Protocol(
                "MCP bridge has no active turn".to_owned(),
            ));
        };
        if active.session_id != expected_pending_session_id {
            return Err(ProviderError::Protocol(
                "MCP pending session binding is stale".to_owned(),
            ));
        }
        native_session_id.clone_into(&mut active.session_id);
        Ok(())
    }

    pub(crate) async fn during_turn<T, F>(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
        observer: &dyn TurnEventObserver,
        work: F,
    ) -> Result<T, ProviderError>
    where
        F: Future<Output = Result<T, ProviderError>> + Send,
        T: Send,
    {
        let (sender, mut receiver) = mpsc::channel(MAX_QUEUE);
        let (cancelled, mut cancellation) = watch::channel(false);
        let epoch = {
            let mut state = lock(&self.shared.state)?;
            if state.closed {
                return Err(ProviderError::Protocol("MCP bridge is closed".to_owned()));
            }
            if state.active.is_some() {
                return Err(ProviderError::Protocol(
                    "MCP bridge already has an active turn".to_owned(),
                ));
            }
            state.next_epoch = state.next_epoch.saturating_add(1);
            let epoch = state.next_epoch;
            state.active = Some(ActiveTurn {
                epoch,
                session_id: session_id.to_owned(),
                turn_id: turn_id.map(str::to_owned),
                sender,
                cancelled,
            });
            epoch
        };
        let _cleanup = ActiveTurnGuard {
            shared: Arc::clone(&self.shared),
            epoch,
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                biased;
                result = &mut work => return result,
                queued = receiver.recv() => match queued {
                    Some(queued) => {
                        // Admission and future construction are one mutex-linearized operation.
                        // Thus close either wins before the observer is invoked, or it loses and
                        // cancels the admitted future through `cancellation` below.
                        let Ok(callback) = admit_callback(&self.shared, epoch, observer, &queued.call) else {
                            let _ = queued.response.send(ToolResult::failure("LZ-MCP-STALE", "MCP turn is no longer active"));
                            continue;
                        };
                        if *cancellation.borrow() {
                            let _ = queued.response.send(ToolResult::failure("LZ-MCP-STALE", "MCP turn is no longer active"));
                            continue;
                        }
                        tokio::select! {
                            biased;
                            _ = cancellation.changed() => {
                                let _ = queued.response.send(ToolResult::failure("LZ-MCP-STALE", "MCP turn is no longer active"));
                            }
                            turn = &mut work => return turn,
                            callback_result = timeout(MAX_OBSERVER_DURATION, callback) => {
                                let result = match callback_result {
                                    Ok(Ok(result)) => result,
                                    Ok(Err(error)) => ToolResult::failure("LZ-MCP-OBSERVER", error.to_string()),
                                    Err(_) => ToolResult::failure("LZ-MCP-OBSERVER-TIMEOUT", "Lazoo collaboration callback exceeded the turn limit"),
                                };
                                let _ = queued.response.send(result);
                            }
                        }
                    }
                    None => return work.await,
                },
            }
        }
    }

    pub(crate) fn close(&self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.closed = true;
            if let Some(active) = state.active.take() {
                let _ = active.cancelled.send(true);
            }
        }
        let _ = self.shutdown.send(true);
    }
}

impl Drop for SessionMcpBridge {
    fn drop(&mut self) {
        self.close();
    }
}

struct ActiveTurnGuard {
    shared: Arc<Shared>,
    epoch: u64,
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.state.lock()
            && state
                .active
                .as_ref()
                .is_some_and(|active| active.epoch == self.epoch)
            && let Some(active) = state.active.take()
        {
            let _ = active.cancelled.send(true);
        }
    }
}

async fn handle(
    shared: Arc<Shared>,
    request: Request<Incoming>,
) -> Result<JsonResponse, Infallible> {
    Ok(handle_inner(shared, request).await)
}

async fn handle_inner(shared: Arc<Shared>, request: Request<Incoming>) -> JsonResponse {
    if request.method() != Method::POST || request.uri().path() != "/mcp" {
        return refusal(
            StatusCode::METHOD_NOT_ALLOWED,
            "LZ-MCP-ROUTE",
            "only POST /mcp is supported",
        );
    }
    if request.headers().contains_key(ORIGIN) {
        return refusal(
            StatusCode::FORBIDDEN,
            "LZ-MCP-ORIGIN",
            "browser origins are refused",
        );
    }
    if !authorized(&shared, &request) {
        return refusal(
            StatusCode::UNAUTHORIZED,
            "LZ-MCP-AUTH",
            "authorization refused",
        );
    }
    if request
        .headers()
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_BODY_BYTES)
    {
        return refusal(
            StatusCode::PAYLOAD_TOO_LARGE,
            "LZ-MCP-BOUND",
            "request exceeds the MCP limit",
        );
    }
    let bytes = match timeout(BODY_READ_TIMEOUT, request.into_body().collect()).await {
        Ok(Ok(collected)) => collected.to_bytes(),
        Ok(Err(_)) => {
            return refusal(
                StatusCode::BAD_REQUEST,
                "LZ-MCP-BODY",
                "request body is invalid",
            );
        }
        Err(_) => {
            return refusal(
                StatusCode::REQUEST_TIMEOUT,
                "LZ-MCP-TIMEOUT",
                "request body timed out",
            );
        }
    };
    if bytes.len() > MAX_BODY_BYTES {
        return refusal(
            StatusCode::PAYLOAD_TOO_LARGE,
            "LZ-MCP-BOUND",
            "request exceeds the MCP limit",
        );
    }
    let message: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return refusal(
                StatusCode::BAD_REQUEST,
                "LZ-MCP-JSON",
                "request must be JSON",
            );
        }
    };
    dispatch(shared, message).await
}

async fn dispatch(shared: Arc<Shared>, message: Value) -> JsonResponse {
    if message.get("jsonrpc") != Some(&Value::String("2.0".to_owned())) {
        return rpc_error(None, -32600, "invalid JSON-RPC version");
    }
    let id = message.get("id").cloned();
    if let Some(id) = &id
        && !matches!(id, Value::String(_) | Value::Number(_))
    {
        return rpc_error(None, -32600, "invalid JSON-RPC id");
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return rpc_error(id.as_ref(), -32600, "missing JSON-RPC method");
    };
    if method == "notifications/initialized" {
        return empty(StatusCode::ACCEPTED);
    }
    let Some(id) = id else {
        return empty(StatusCode::ACCEPTED);
    };
    match method {
        "initialize" => {
            let version = message
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str);
            if version.is_none_or(str::is_empty) {
                return rpc_error(Some(&id), -32602, "missing MCP protocol version");
            }
            // MCP initialization negotiates: a server returns one of its supported versions even
            // when a client requests a newer version. The client decides whether it supports it.
            rpc_result(
                &id,
                &json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "lazoo-session-bridge", "version": env!("CARGO_PKG_VERSION")}
                }),
            )
        }
        "ping" => rpc_result(&id, &json!({})),
        "tools/list" => rpc_result(&id, &json!({"tools": shared.tools})),
        "tools/call" => call_tool(shared, id, message.get("params")).await,
        _ => rpc_error(Some(&id), -32601, "MCP method is not available"),
    }
}

async fn call_tool(shared: Arc<Shared>, id: Value, params: Option<&Value>) -> JsonResponse {
    let Some(params) = params else {
        return rpc_error(Some(&id), -32602, "tools/call requires params");
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return rpc_error(Some(&id), -32602, "tool name is required");
    };
    if !shared.tools.iter().any(|tool| tool.name == name) {
        return rpc_error(Some(&id), -32602, "unknown Lazoo tool");
    }
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if serde_json::to_vec(&arguments).map_or(true, |value| value.len() > MAX_BODY_BYTES) {
        return rpc_error(Some(&id), -32602, "tool arguments exceed the MCP limit");
    }
    let (sender, call) = match active_call(&shared, name, arguments) {
        Ok(value) => value,
        Err(error) => return rpc_error(Some(&id), -32000, &error),
    };
    let (result_sender, result_receiver) = oneshot::channel();
    match sender.try_send(QueuedCall {
        call,
        response: result_sender,
    }) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            return rpc_error(Some(&id), -32000, "MCP tool queue is full");
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            return rpc_error(Some(&id), -32000, "MCP turn is no longer active");
        }
    }
    match result_receiver.await {
        Ok(result) => tool_result(&id, &result),
        Err(_) => rpc_error(Some(&id), -32000, "MCP turn is no longer active"),
    }
}

fn active_call(
    shared: &Shared,
    name: &str,
    arguments: Value,
) -> Result<(mpsc::Sender<QueuedCall>, ToolCall), String> {
    let state = lock(&shared.state).map_err(|error| error.to_string())?;
    let Some(active) = &state.active else {
        return Err("MCP calls are refused outside an active turn".to_owned());
    };
    if state.closed {
        return Err("MCP bridge is closed".to_owned());
    }
    // Identity is from the active binding, never supplied in the MCP payload.
    Ok((
        active.sender.clone(),
        ToolCall {
            provider: shared.provider,
            session_id: active.session_id.clone(),
            turn_id: active.turn_id.clone(),
            call_id: Uuid::new_v4().to_string(),
            name: name.to_owned(),
            arguments,
        },
    ))
}

/// Atomically admits a callback and constructs its observer future.  `close` uses the same
/// mutex, so it cannot win between authority admission and synchronous `call_tool` construction.
fn admit_callback<'a>(
    shared: &Shared,
    epoch: u64,
    observer: &'a dyn TurnEventObserver,
    call: &'a ToolCall,
) -> Result<crate::AdapterFuture<'a, ToolResult>, ProviderError> {
    let state = lock(&shared.state)?;
    if state.closed
        || !state
            .active
            .as_ref()
            .is_some_and(|active| active.epoch == epoch && active.session_id == call.session_id)
    {
        return Err(ProviderError::Protocol(
            "MCP turn is no longer active".to_owned(),
        ));
    }
    let callback = observer.call_tool(call);
    drop(state);
    Ok(callback)
}

fn tool_result(id: &Value, result: &ToolResult) -> JsonResponse {
    let mut text = serde_json::to_string(&result.content)
        .unwrap_or_else(|_| "{\"error\":\"unserializable result\"}".to_owned());
    if text.len() > MAX_RESPONSE_BYTES {
        "{\"error\":{\"code\":\"LZ-MCP-BOUND\",\"message\":\"tool result exceeds the MCP limit\"}}"
            .clone_into(&mut text);
    }
    rpc_result(
        id,
        &json!({"content": [{"type": "text", "text": text}], "isError": !result.success}),
    )
}

fn authorized(shared: &Shared, request: &Request<Incoming>) -> bool {
    request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some(shared.authorization.as_str())
}

fn lock(state: &Mutex<State>) -> Result<std::sync::MutexGuard<'_, State>, ProviderError> {
    state
        .lock()
        .map_err(|_| ProviderError::Protocol("MCP bridge state is poisoned".to_owned()))
}

fn endpoint(address: SocketAddr) -> String {
    format!("http://{address}/mcp")
}

fn bearer() -> String {
    // Three UUIDv4 values contribute 366 random input bits before the value is reduced to the
    // 256-bit SHA-256 digest used as the bearer.  The digest width alone is not an entropy claim.
    let mut digest = Sha256::new();
    digest.update(Uuid::new_v4().as_bytes());
    digest.update(Uuid::new_v4().as_bytes());
    digest.update(Uuid::new_v4().as_bytes());
    let digest = digest.finalize();
    let mut value = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("Bearer {value}")
}

fn empty(status: StatusCode) -> JsonResponse {
    Response::builder()
        .status(status)
        .header(CONNECTION, "close")
        .body(Full::new(Bytes::new()))
        .expect("fixed response")
}

fn refusal(status: StatusCode, code: &str, message: &str) -> JsonResponse {
    json_response(
        status,
        &json!({"error": {"code": code, "message": message}}),
    )
}
fn rpc_result(id: &Value, result: &Value) -> JsonResponse {
    json_response(
        StatusCode::OK,
        &json!({"jsonrpc":"2.0", "id":id, "result":result}),
    )
}
fn rpc_error(id: Option<&Value>, code: i64, message: &str) -> JsonResponse {
    json_response(
        StatusCode::OK,
        &json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message}}),
    )
}
fn json_response(status: StatusCode, value: &Value) -> JsonResponse {
    let encoded = serde_json::to_vec(&value).expect("fixed JSON response");
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .header(CONNECTION, "close")
        .body(Full::new(Bytes::from(encoded)))
        .expect("fixed response")
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::{AdapterFuture, ProviderEvent};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        sync::{Notify, oneshot},
    };

    struct Observer(StdMutex<Vec<ToolCall>>);

    impl TurnEventObserver for Observer {
        fn observe(&self, _event: &ProviderEvent) -> Result<(), ProviderError> {
            Ok(())
        }

        fn call_tool<'a>(&'a self, call: &'a ToolCall) -> AdapterFuture<'a, ToolResult> {
            self.0.lock().expect("calls mutex").push(call.clone());
            Box::pin(async { Ok(ToolResult::success(json!({"accepted": true}))) })
        }
    }

    struct BlockingObserver {
        started: Arc<Notify>,
    }

    impl TurnEventObserver for BlockingObserver {
        fn observe(&self, _event: &ProviderEvent) -> Result<(), ProviderError> {
            Ok(())
        }

        fn call_tool<'a>(&'a self, _call: &'a ToolCall) -> AdapterFuture<'a, ToolResult> {
            self.started.notify_one();
            Box::pin(std::future::pending())
        }
    }

    struct DelayedObserver;

    impl TurnEventObserver for DelayedObserver {
        fn observe(&self, _event: &ProviderEvent) -> Result<(), ProviderError> {
            Ok(())
        }

        fn call_tool<'a>(&'a self, _call: &'a ToolCall) -> AdapterFuture<'a, ToolResult> {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(ToolResult::success(json!({"delayed": true})))
            })
        }
    }

    struct SynchronousObserver(AtomicUsize);

    impl TurnEventObserver for SynchronousObserver {
        fn observe(&self, _event: &ProviderEvent) -> Result<(), ProviderError> {
            Ok(())
        }

        fn call_tool<'a>(&'a self, _call: &'a ToolCall) -> AdapterFuture<'a, ToolResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(ToolResult::success(json!({"unexpected": true}))) })
        }
    }

    fn tool() -> ToolSpec {
        ToolSpec {
            name: "delegate".to_owned(),
            description: "delegate work".to_owned(),
            input_schema: json!({"type":"object"}),
        }
    }

    async fn exchange(
        bridge: &SessionMcpBridge,
        authorization: &str,
        body: Value,
        origin: bool,
    ) -> String {
        let target = bridge
            .endpoint()
            .trim_start_matches("http://")
            .trim_end_matches("/mcp");
        let mut stream = TcpStream::connect(target).await.expect("connect bridge");
        let encoded = serde_json::to_vec(&body).expect("encode request");
        let origin = if origin {
            "Origin: http://localhost\r\n"
        } else {
            ""
        };
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: {target}\r\nAuthorization: {authorization}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{origin}\r\n",
            encoded.len()
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write headers");
        stream.write_all(&encoded).await.expect("write body");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .expect("read response");
        String::from_utf8(response).expect("utf8 response")
    }

    #[tokio::test]
    async fn http_refuses_unauthenticated_browser_and_inactive_calls() {
        let bridge = SessionMcpBridge::open(ProviderKind::Codex, vec![tool()])
            .await
            .expect("open");
        let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}});
        let bad = exchange(&bridge, "Bearer wrong", initialize.clone(), false).await;
        assert!(bad.starts_with("HTTP/1.1 401"), "{bad:?}");
        let browser = exchange(&bridge, &bridge.authorization(), initialize, true).await;
        assert!(browser.starts_with("HTTP/1.1 403"));
        let inactive = exchange(&bridge, &bridge.authorization(), json!({"jsonrpc":"2.0","id":"call","method":"tools/call","params":{"name":"delegate","arguments":{}}}), false).await;
        assert!(inactive.contains("outside an active turn"));
        bridge.close();
    }

    #[tokio::test]
    async fn initialize_negotiates_legacy_version_for_newer_client() {
        let bridge = SessionMcpBridge::open(ProviderKind::Codex, vec![tool()])
            .await
            .expect("open");
        let newer = exchange(
            &bridge,
            &bridge.authorization(),
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}),
            false,
        )
        .await;
        assert!(newer.contains(MCP_PROTOCOL_VERSION), "{newer:?}");
        assert!(!newer.contains("unsupported MCP protocol version"));
        let malformed = exchange(
            &bridge,
            &bridge.authorization(),
            json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":""}}),
            false,
        )
        .await;
        assert!(malformed.contains("missing MCP protocol version"));
        bridge.close();
    }

    #[tokio::test]
    async fn pending_session_rebinds_once_and_rejects_old_callbacks() {
        let bridge = Arc::new(
            SessionMcpBridge::open(ProviderKind::Antigravity, vec![tool()])
                .await
                .expect("open"),
        );
        let observer = Arc::new(Observer(StdMutex::new(Vec::new())));
        let (done, wait) = oneshot::channel::<()>();
        let turn_bridge = Arc::clone(&bridge);
        let turn_observer = Arc::clone(&observer);
        let turn = tokio::spawn(async move {
            turn_bridge
                .during_turn(
                    "pending:session",
                    Some("turn"),
                    turn_observer.as_ref(),
                    async move {
                        let _ = wait.await;
                        Ok::<_, ProviderError>(())
                    },
                )
                .await
        });
        tokio::task::yield_now().await;
        bridge
            .rebind_pending_session("pending:session", "native-session")
            .expect("bind native id");
        assert!(
            bridge
                .rebind_pending_session("pending:session", "another-native-session")
                .is_err()
        );
        let old_call = ToolCall {
            provider: ProviderKind::Antigravity,
            session_id: "pending:session".to_owned(),
            turn_id: Some("turn".to_owned()),
            call_id: "old".to_owned(),
            name: "delegate".to_owned(),
            arguments: json!({}),
        };
        assert!(admit_callback(&bridge.shared, 1, observer.as_ref(), &old_call).is_err());
        let response = exchange(
            &bridge,
            &bridge.authorization(),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"delegate","arguments":{}}}),
            false,
        )
        .await;
        assert!(response.contains("accepted"), "{response:?}");
        let calls = observer.0.lock().expect("calls mutex").clone();
        assert_eq!(calls[0].session_id, "native-session");
        done.send(()).expect("finish turn");
        turn.await.expect("turn task").expect("turn result");
        bridge.close();
    }

    #[tokio::test]
    async fn http_binds_tool_identity_to_active_turn_and_cleans_up() {
        let bridge = Arc::new(
            SessionMcpBridge::open(ProviderKind::ClaudeCode, vec![tool()])
                .await
                .expect("open"),
        );
        let observer = Arc::new(Observer(StdMutex::new(Vec::new())));
        let (done, wait) = oneshot::channel::<()>();
        let turn_bridge = Arc::clone(&bridge);
        let turn_observer = Arc::clone(&observer);
        let turn = tokio::spawn(async move {
            turn_bridge
                .during_turn(
                    "session-bound",
                    Some("turn-bound"),
                    turn_observer.as_ref(),
                    async move {
                        let _ = wait.await;
                        Ok::<_, ProviderError>(())
                    },
                )
                .await
        });
        tokio::task::yield_now().await;
        let auth = bridge.authorization();
        let initialized = exchange(&bridge, &auth, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}), false).await;
        assert!(initialized.contains("2025-06-18"), "{initialized:?}");
        let listed = exchange(
            &bridge,
            &auth,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            false,
        )
        .await;
        assert!(listed.contains("delegate"));
        let called = exchange(&bridge, &auth, json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"delegate","arguments":{"untrustedSession":"no"}}}), false).await;
        assert!(called.contains("accepted"));
        let calls = observer.0.lock().expect("calls mutex").clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].session_id, "session-bound");
        assert_eq!(calls[0].turn_id.as_deref(), Some("turn-bound"));
        done.send(()).expect("finish turn");
        turn.await.expect("turn task").expect("turn result");
        let stale = exchange(&bridge, &auth, json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"delegate","arguments":{}}}), false).await;
        assert!(stale.contains("outside an active turn"));
        bridge.close();
    }

    #[tokio::test]
    async fn close_cancels_in_flight_observer_and_releases_request() {
        let bridge = Arc::new(
            SessionMcpBridge::open(ProviderKind::OpenCode, vec![tool()])
                .await
                .expect("open"),
        );
        let started = Arc::new(Notify::new());
        let observer = Arc::new(BlockingObserver {
            started: Arc::clone(&started),
        });
        let turn_bridge = Arc::clone(&bridge);
        let turn_observer = Arc::clone(&observer);
        let turn = tokio::spawn(async move {
            turn_bridge
                .during_turn(
                    "old",
                    Some("old-turn"),
                    turn_observer.as_ref(),
                    std::future::pending::<Result<(), ProviderError>>(),
                )
                .await
        });
        tokio::task::yield_now().await;
        let auth = bridge.authorization();
        let request_bridge = Arc::clone(&bridge);
        let request = tokio::spawn(async move {
            exchange(&request_bridge, &auth, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"delegate","arguments":{}}}), false).await
        });
        started.notified().await;
        bridge.close();
        let response = request.await.expect("request task");
        assert!(
            response.is_empty()
                || response.contains("no longer active")
                || response.contains("outside an active turn"),
            "{response:?}"
        );
        turn.abort();
    }

    #[tokio::test]
    async fn close_winning_before_admission_blocks_callback_construction() {
        let bridge = Arc::new(
            SessionMcpBridge::open(ProviderKind::OpenCode, vec![tool()])
                .await
                .expect("open"),
        );
        let observer = Arc::new(SynchronousObserver(AtomicUsize::new(0)));
        let turn_bridge = Arc::clone(&bridge);
        let turn_observer = Arc::clone(&observer);
        let turn = tokio::spawn(async move {
            turn_bridge
                .during_turn(
                    "session",
                    Some("turn"),
                    turn_observer.as_ref(),
                    std::future::pending::<Result<(), ProviderError>>(),
                )
                .await
        });
        tokio::task::yield_now().await;
        bridge.close();
        let call = ToolCall {
            provider: ProviderKind::OpenCode,
            session_id: "session".to_owned(),
            turn_id: Some("turn".to_owned()),
            call_id: "call".to_owned(),
            name: "delegate".to_owned(),
            arguments: json!({}),
        };
        assert!(admit_callback(&bridge.shared, 1, observer.as_ref(), &call).is_err());
        assert_eq!(observer.0.load(Ordering::SeqCst), 0);
        turn.abort();
    }

    #[tokio::test]
    async fn delayed_observer_outlives_body_read_deadline() {
        let bridge = Arc::new(
            SessionMcpBridge::open(ProviderKind::OpenCode, vec![tool()])
                .await
                .expect("open"),
        );
        let observer = Arc::new(DelayedObserver);
        let (done, wait) = oneshot::channel::<()>();
        let turn_bridge = Arc::clone(&bridge);
        let turn_observer = Arc::clone(&observer);
        let turn = tokio::spawn(async move {
            turn_bridge
                .during_turn(
                    "session",
                    Some("turn"),
                    turn_observer.as_ref(),
                    async move {
                        let _ = wait.await;
                        Ok::<_, ProviderError>(())
                    },
                )
                .await
        });
        tokio::task::yield_now().await;
        let response = exchange(
            &bridge,
            &bridge.authorization(),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"delegate","arguments":{}}}),
            false,
        )
        .await;
        assert!(response.contains("delayed"), "{response:?}");
        done.send(()).expect("finish turn");
        turn.await.expect("turn task").expect("turn result");
        bridge.close();
    }

    #[tokio::test]
    async fn slow_headers_expire_and_release_connection_capacity() {
        let bridge = SessionMcpBridge::open(ProviderKind::Codex, vec![tool()])
            .await
            .expect("open");
        let target = bridge
            .endpoint()
            .trim_start_matches("http://")
            .trim_end_matches("/mcp");
        let mut slow_clients = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            let mut stream = TcpStream::connect(target)
                .await
                .expect("connect slow client");
            stream
                .write_all(b"POST /mcp HTTP/1.1\r\nHost: localhost\r\n")
                .await
                .expect("write partial headers");
            slow_clients.push(stream);
        }
        tokio::time::sleep(HEADER_READ_TIMEOUT + Duration::from_millis(25)).await;
        for mut stream in slow_clients {
            let mut response = Vec::new();
            stream
                .read_to_end(&mut response)
                .await
                .expect("read closed slow client");
            assert!(
                response.is_empty(),
                "slow header received an unexpected response"
            );
        }
        let response = exchange(
            &bridge,
            &bridge.authorization(),
            json!({"jsonrpc":"2.0","id":1,"method":"ping"}),
            false,
        )
        .await;
        assert!(response.contains("200 OK"), "{response:?}");
        bridge.close();
    }
}
