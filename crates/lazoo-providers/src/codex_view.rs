//! Restricted loopback WebSocket bridge for one genuine native Codex TUI.
//!
//! Viewer messages can only perform narrow, thread-scoped reads. The controller-owned broker is
//! still the only path which writes app-server traffic for turns, tools, and approvals.

use std::{
    collections::VecDeque,
    ffi::OsString,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use crate::{
    CommandSpec, NativeViewLaunchDescriptor, NativeViewLease, ProviderError,
    codex_broker::CodexBrokerReadHandle, native_view::NativeViewBackend,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
    task::{AbortHandle, JoinHandle},
};
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        Message,
        handshake::server::{ErrorResponse, Request, Response},
        http::{HeaderValue, StatusCode},
        protocol::WebSocketConfig,
    },
};
use uuid::Uuid;

const TOKEN_ENV: &str = "LAZOO_NATIVE_VIEW_TOKEN";
const MAX_FRAME_BYTES: usize = 256 * 1024;
const MAX_PAGE_SIZE: u64 = 100;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_HANDSHAKES: usize = 4;
const MAX_BUFFERED_NOTIFICATIONS: usize = 256;
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// Inputs captured from the controller-owned session, never supplied by the viewer.
#[derive(Clone, Debug)]
pub(crate) struct CodexNativeViewConfig {
    pub broker: CodexBrokerReadHandle,
    pub genuine_thread_start: Value,
    pub executable: PathBuf,
    pub cwd: PathBuf,
    pub launch_environment: lazoo_runtime::LaunchEnvironment,
}
#[derive(Debug)]
struct LeaseState {
    host_pid: Option<u32>,
    closed: AtomicBool,
    viewer_connected: AtomicBool,
    handshakes: AtomicUsize,
    connection_tasks: Mutex<Vec<AbortHandle>>,
    closed_tx: watch::Sender<bool>,
}
/// Codex's restricted loopback backend for a provider-neutral native-view lease.
struct CodexViewBackend {
    state: Arc<LeaseState>,
    listener_task: Mutex<Option<JoinHandle<()>>>,
    address: SocketAddr,
    token: String,
    session_id: String,
    executable: PathBuf,
    cwd: PathBuf,
    launch_environment: lazoo_runtime::LaunchEnvironment,
}
impl CodexViewBackend {
    /// Binds a fresh loopback endpoint. Two `UUIDv4` values provide at least 244 bits of entropy.
    pub(crate) async fn open(
        config: CodexNativeViewConfig,
    ) -> Result<NativeViewLease, ProviderError> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let (closed_tx, closed_rx) = watch::channel(false);
        let state = Arc::new(LeaseState {
            host_pid: config.broker.native_host_pid(),
            closed: AtomicBool::new(false),
            viewer_connected: AtomicBool::new(false),
            handshakes: AtomicUsize::new(0),
            connection_tasks: Mutex::new(Vec::with_capacity(MAX_HANDSHAKES)),
            closed_tx,
        });
        let session_id = config.broker.session_id().to_owned();
        let server = ViewServer {
            broker: config.broker,
            genuine_thread_start: config.genuine_thread_start,
            cwd: config.cwd.clone(),
            token: token.clone(),
            state: Arc::clone(&state),
        };
        let listener_task = tokio::spawn(accept_loop(listener, server, closed_rx));
        Ok(NativeViewLease::from_backend(Self {
            state,
            listener_task: Mutex::new(Some(listener_task)),
            address,
            token,
            session_id,
            executable: config.executable,
            cwd: config.cwd,
            launch_environment: config.launch_environment,
        }))
    }
    /// Returns a genuine native Codex resume command with its token only in `env`.
    #[must_use]
    fn descriptor(&self) -> NativeViewLaunchDescriptor {
        let remote_url = format!("ws://{}", self.address);
        let mut command = CommandSpec::new(&self.executable, &self.cwd)
            .arg("resume")
            .arg(&self.session_id)
            .arg("--remote")
            .arg(&remote_url)
            .arg("--remote-auth-token-env")
            .arg(TOKEN_ENV)
            .arg("--no-alt-screen");
        command.apply_environment(&self.launch_environment);
        command
            .env
            .insert(OsString::from(TOKEN_ENV), OsString::from(&self.token));
        NativeViewLaunchDescriptor::Remote {
            remote_url,
            command,
        }
    }
}

impl NativeViewBackend for CodexViewBackend {
    fn close(&self) {
        tracing::warn!(
            host_pid = self.state.host_pid,
            reason = "view-lease-close",
            canonical_termination = false,
            "closing Codex viewer lease only"
        );
        if self.state.close()
            && let Some(task) = self.listener_task.lock().unwrap().take()
        {
            task.abort();
        }
    }

    fn is_closed(&self) -> bool {
        self.state.closed.load(Ordering::Acquire)
    }

    fn closed_subscription(&self) -> watch::Receiver<bool> {
        self.state.closed_tx.subscribe()
    }

    fn launch_descriptor(&self) -> NativeViewLaunchDescriptor {
        self.descriptor()
    }
}

impl LeaseState {
    fn close(&self) -> bool {
        if self.closed.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.closed_tx.send_replace(true);
        let mut tasks = self.connection_tasks.lock().unwrap();
        for task in tasks.drain(..) {
            task.abort();
        }
        true
    }
}
impl crate::CodexSession {
    #[must_use]
    pub(crate) fn native_view_supported(&self) -> bool {
        let (broker, bootstrap, _, _, _) = self.live_view_bootstrap();
        !broker.is_closed() && bootstrap.is_object()
    }

    pub(crate) async fn open_native_view_lease(&self) -> Result<NativeViewLease, ProviderError> {
        let (broker, genuine_thread_start, executable, cwd, launch_environment) =
            self.live_view_bootstrap();
        if broker.is_closed() || !genuine_thread_start.is_object() {
            return Err(ProviderError::Unsupported {
                provider: crate::ProviderKind::Codex,
                capability: "live native view",
                detail: Some("the controlled Codex host is unavailable".to_owned()),
            });
        }
        CodexViewBackend::open(CodexNativeViewConfig {
            broker,
            genuine_thread_start,
            executable,
            cwd,
            launch_environment,
        })
        .await
    }
}

#[derive(Clone)]
struct ViewServer {
    broker: CodexBrokerReadHandle,
    genuine_thread_start: Value,
    cwd: PathBuf,
    token: String,
    state: Arc<LeaseState>,
}
async fn accept_loop(listener: TcpListener, server: ViewServer, mut closed: watch::Receiver<bool>) {
    let mut broker_closed = server.broker.closed_subscription();
    if *closed.borrow_and_update()
        || *broker_closed.borrow_and_update()
        || server.broker.is_closed()
    {
        server.state.close();
        return;
    }
    loop {
        tokio::select! {
            changed = closed.changed() => { let _ = changed; return; }
            changed = broker_closed.changed() => {
                let _ = changed;
                server.state.close();
                return;
            }
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { return; };
                if server.state.handshakes.fetch_add(1, Ordering::AcqRel) >= MAX_HANDSHAKES {
                    server.state.handshakes.fetch_sub(1, Ordering::AcqRel);
                    drop(stream);
                    continue;
                }
                let server = server.clone();
                let task_state = Arc::clone(&server.state);
                let connection_closed = closed.clone();
                let task = tokio::spawn(async move {
                    tokio::task::yield_now().await;
                    let state = Arc::clone(&server.state);
                    let result = serve_connection(stream, server, connection_closed).await;
                    tracing::warn!(host_pid = state.host_pid, reason = "view-connection-ended", failed = result.is_err(),
                        canonical_termination = false, "Codex viewer disconnected");
                    state.handshakes.fetch_sub(1, Ordering::AcqRel);
                    let task_id = tokio::task::id();
                    state.connection_tasks.lock().unwrap().retain(|task| task.id() != task_id);
                });
                let abort = task.abort_handle();
                let mut tasks = task_state.connection_tasks.lock().unwrap();
                // A completed connection can race its registration on another runtime worker.
                tasks.retain(|task| !task.is_finished());
                if task_state.closed.load(Ordering::Acquire) {
                    abort.abort();
                } else {
                    tasks.push(abort);
                }
            }
        }
    }
}
async fn serve_connection(
    stream: TcpStream,
    server: ViewServer,
    closed: watch::Receiver<bool>,
) -> Result<(), ProviderError> {
    let token = server.token.clone();
    let state = Arc::clone(&server.state);
    let admitted = Arc::new(AtomicBool::new(false));
    let callback_admitted = Arc::clone(&admitted);
    #[allow(clippy::result_large_err)] // tungstenite fixes the handshake callback error type.
    let callback =
        move |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
            if request.headers().contains_key("origin") {
                return Err(reject(
                    StatusCode::FORBIDDEN,
                    "browser origins are not accepted",
                ));
            }
            let expected = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| reject(StatusCode::UNAUTHORIZED, "invalid authorization"))?;
            if request.headers().get("authorization") != Some(&expected) {
                return Err(reject(
                    StatusCode::UNAUTHORIZED,
                    "bearer authentication required",
                ));
            }
            if state.viewer_connected.swap(true, Ordering::AcqRel) {
                return Err(reject(
                    StatusCode::CONFLICT,
                    "a native viewer is already connected",
                ));
            }
            callback_admitted.store(true, Ordering::Release);
            Ok(response)
        };
    let config = WebSocketConfig::default()
        .read_buffer_size(8 * 1024)
        .write_buffer_size(8 * 1024)
        .max_write_buffer_size(MAX_FRAME_BYTES * 4)
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES));
    let websocket = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        accept_hdr_async_with_config(stream, callback, Some(config)),
    )
    .await;
    let websocket = match websocket {
        Ok(Ok(websocket)) => websocket,
        Ok(Err(error)) => {
            if admitted.swap(false, Ordering::AcqRel) {
                server
                    .state
                    .viewer_connected
                    .store(false, Ordering::Release);
            }
            return Err(ProviderError::Protocol(format!(
                "native view handshake failed: {error}"
            )));
        }
        Err(_) => {
            if admitted.swap(false, Ordering::AcqRel) {
                server
                    .state
                    .viewer_connected
                    .store(false, Ordering::Release);
            }
            return Err(ProviderError::Protocol(
                "native view handshake timed out".to_owned(),
            ));
        }
    };
    let result = serve_websocket(websocket, &server, closed).await;
    server
        .state
        .viewer_connected
        .store(false, Ordering::Release);
    result
}
fn reject(status: StatusCode, message: &'static str) -> ErrorResponse {
    let mut response =
        tokio_tungstenite::tungstenite::http::Response::new(Some(message.to_owned()));
    *response.status_mut() = status;
    response
}
async fn serve_websocket(
    websocket: tokio_tungstenite::WebSocketStream<TcpStream>,
    server: &ViewServer,
    mut closed: watch::Receiver<bool>,
) -> Result<(), ProviderError> {
    let (mut writer, mut reader) = websocket.split();
    let mut notifications = server.broker.subscribe_notifications();
    let mut broker_closed = server.broker.closed_subscription();
    if *closed.borrow_and_update()
        || *broker_closed.borrow_and_update()
        || server.broker.is_closed()
    {
        return Ok(());
    }
    let mut bootstrap_ready = false;
    let mut buffered_notifications = VecDeque::with_capacity(MAX_BUFFERED_NOTIFICATIONS);
    loop {
        tokio::select! {
            changed = closed.changed() => { let _ = changed; return Ok(()); }
            changed = broker_closed.changed() => { let _ = changed; return Ok(()); }
            incoming = reader.next() => { let Some(incoming) = incoming else { return Ok(()); }; match incoming.map_err(|error| ProviderError::Protocol(format!("native view read failed: {error}")))? {
                Message::Text(text) => {
                    if text.len() > MAX_FRAME_BYTES { return Err(ProviderError::Protocol("native view frame exceeded limit".to_owned())); }
                    if let Some(reply) = handle_view_request(&text, server).await? {
                        send_text(&mut writer, reply.payload, closed.clone()).await?;
                        if reply.completes_bootstrap {
                            bootstrap_ready = true;
                            while let Some(notification) = buffered_notifications.pop_front() {
                                send_text(&mut writer, serde_json::to_string(&notification)?, closed.clone()).await?;
                            }
                        }
                    }
                }
                Message::Binary(bytes) if bytes.len() > MAX_FRAME_BYTES => return Err(ProviderError::Protocol("native view binary frame exceeded limit".to_owned())),
                Message::Close(_) => return Ok(()), Message::Ping(payload) => send_message(&mut writer, Message::Pong(payload), closed.clone()).await?, _ => {}
            }}
            notification = notifications.next(READ_TIMEOUT, "native view notification") => match notification {
                Ok(notification) if notification_is_scoped(&notification, server.broker.session_id()) => {
                    if bootstrap_ready {
                        send_text(&mut writer, serde_json::to_string(&notification)?, closed.clone()).await?;
                    } else if buffered_notifications.len() == MAX_BUFFERED_NOTIFICATIONS {
                        return Err(ProviderError::Protocol("native view bootstrap notification buffer requires resync".to_owned()));
                    } else {
                        buffered_notifications.push_back(notification);
                    }
                }
                Ok(_) | Err(ProviderError::Timeout { .. }) => {},
                Err(error) => return Err(error),
            }
        }
    }
}
async fn send_text<S>(
    writer: &mut S,
    text: String,
    closed: watch::Receiver<bool>,
) -> Result<(), ProviderError>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    if text.len() > MAX_FRAME_BYTES {
        return Err(ProviderError::Protocol(
            "native view reply exceeded limit".to_owned(),
        ));
    }
    send_message(writer, Message::Text(text.into()), closed).await
}
async fn send_message<S>(
    writer: &mut S,
    message: Message,
    mut closed: watch::Receiver<bool>,
) -> Result<(), ProviderError>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    if *closed.borrow_and_update() {
        return Err(ProviderError::Protocol(
            "native view closed while writing".to_owned(),
        ));
    }
    tokio::select! {
        changed = closed.changed() => { let _ = changed; Err(ProviderError::Protocol("native view closed while writing".to_owned())) }
        sent = tokio::time::timeout(WRITE_TIMEOUT, writer.send(message)) => match sent {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(write_error(&error)),
            Err(_) => Err(ProviderError::Protocol("native view write timed out".to_owned())),
        }
    }
}
fn write_error(error: &tokio_tungstenite::tungstenite::Error) -> ProviderError {
    ProviderError::Protocol(format!("native view write failed: {error}"))
}
struct ViewReply {
    payload: String,
    completes_bootstrap: bool,
}
async fn handle_view_request(
    raw: &str,
    server: &ViewServer,
) -> Result<Option<ViewReply>, ProviderError> {
    let frame: Value = serde_json::from_str(raw)
        .map_err(|_| ProviderError::Protocol("native view sent invalid JSON".to_owned()))?;
    let Some(id) = frame.get("id").cloned() else {
        return Ok(None);
    };
    let Some(method) = frame.get("method").and_then(Value::as_str) else {
        return Ok(Some(ViewReply {
            payload: error_reply(&id, "viewer responses and server requests are refused"),
            completes_bootstrap: false,
        }));
    };
    let params = frame.get("params").cloned().unwrap_or_else(|| json!({}));
    let (reply, completes_bootstrap) = match route_read(method, params, server).await {
        Ok(result) => (
            json!({"id": id, "result": result}),
            method == "thread/resume",
        ),
        Err(error) => (
            json!({"id": id, "error": {"code": -32001, "message": client_error(&error)}}),
            false,
        ),
    };
    Ok(Some(ViewReply {
        payload: serde_json::to_string(&reply)?,
        completes_bootstrap,
    }))
}
async fn route_read(
    method: &str,
    params: Value,
    server: &ViewServer,
) -> Result<Value, ProviderError> {
    match method {
        "initialize" => Ok(server.broker.cached_initialize()),
        "thread/resume" => bootstrap_resume(params, server).await,
        "thread/read" => forward_thread_read(params, server).await,
        "thread/turns/list" | "thread/items/list" => history_read(method, params, server).await,
        "thread/list" => single_thread_list(server).await,
        "thread/unsubscribe" => {
            ensure_optional_thread(&params, server.broker.session_id())?;
            Ok(json!({"status": "unsubscribed"}))
        }
        "account/rateLimits/read" => {
            server
                .broker
                .request(method, Value::Null, READ_TIMEOUT, "native quota read")
                .await
        }
        "account/read" => {
            server
                .broker
                .request(
                    method,
                    json!({"refreshToken": false}),
                    READ_TIMEOUT,
                    "native account read",
                )
                .await
        }
        "hooks/list" | "skills/list" => {
            ensure_cwds(&params, &server.cwd)?;
            server
                .broker
                .request(
                    method,
                    json!({"cwds": [server.cwd], "forceReload": false}),
                    READ_TIMEOUT,
                    "native scoped read",
                )
                .await
        }
        "model/list" => {
            server
                .broker
                .request(
                    method,
                    bounded_page(&params)?,
                    READ_TIMEOUT,
                    "native status read",
                )
                .await
        }
        "configRequirements/read" => {
            ensure_empty_params(&params, method)?;
            server
                .broker
                .request(method, json!({}), READ_TIMEOUT, "native status read")
                .await
        }
        _ => Err(ProviderError::Unsupported {
            provider: crate::ProviderKind::Codex,
            capability: "restricted native view read",
            detail: Some(method.to_owned()),
        }),
    }
}
async fn bootstrap_resume(params: Value, server: &ViewServer) -> Result<Value, ProviderError> {
    ensure_thread(&params, server.broker.session_id())?;
    let read = read_thread_metadata(server).await?;
    let mut thread = read.get("thread").cloned().ok_or_else(|| {
        ProviderError::Protocol("native view thread/read omitted thread".to_owned())
    })?;
    thread["canAcceptDirectInput"] = Value::Bool(false);
    let mut result = server.genuine_thread_start.clone();
    result
        .as_object_mut()
        .ok_or_else(|| {
            ProviderError::Protocol("genuine Codex thread start was not an object".to_owned())
        })?
        .insert("thread".to_owned(), thread);
    Ok(result)
}
async fn forward_thread_read(params: Value, server: &ViewServer) -> Result<Value, ProviderError> {
    ensure_thread(&params, server.broker.session_id())?;
    let mut read = read_thread_metadata(server).await?;
    force_no_direct_input(&mut read)?;
    Ok(read)
}

async fn read_thread_metadata(server: &ViewServer) -> Result<Value, ProviderError> {
    // A new thread has no persisted rollout yet. Its canonical start response is authoritative.
    if server.broker.is_new_empty_thread() {
        return Ok(json!({"thread": server.genuine_thread_start["thread"]}));
    }
    server
        .broker
        .request(
            "thread/read",
            json!({"threadId": server.broker.session_id(), "includeTurns": false}),
            READ_TIMEOUT,
            "native thread read",
        )
        .await
}
async fn history_read(
    method: &str,
    params: Value,
    server: &ViewServer,
) -> Result<Value, ProviderError> {
    ensure_thread(&params, server.broker.session_id())?;
    let params = bounded_page(&params)?;
    if server.broker.is_new_empty_thread() {
        return Ok(json!({"data": [], "nextCursor": null}));
    }
    server
        .broker
        .request(method, params, READ_TIMEOUT, "native history read")
        .await
}
async fn single_thread_list(server: &ViewServer) -> Result<Value, ProviderError> {
    let read = read_thread_metadata(server).await?;
    let mut thread = read.get("thread").cloned().ok_or_else(|| {
        ProviderError::Protocol("native view thread/read omitted thread".to_owned())
    })?;
    force_no_direct_input_on_thread(&mut thread)?;
    Ok(json!({"data": [thread], "nextCursor": null}))
}
fn ensure_thread(params: &Value, session_id: &str) -> Result<(), ProviderError> {
    match params.get("threadId").and_then(Value::as_str) {
        Some(id) if id == session_id => Ok(()),
        _ => Err(ProviderError::Protocol(
            "native viewer request targeted a different or missing thread".to_owned(),
        )),
    }
}
fn ensure_optional_thread(params: &Value, session_id: &str) -> Result<(), ProviderError> {
    match params.get("threadId") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(id)) if id == session_id => Ok(()),
        _ => Err(ProviderError::Protocol(
            "native viewer request targeted a different thread".to_owned(),
        )),
    }
}
fn ensure_cwds(params: &Value, cwd: &std::path::Path) -> Result<(), ProviderError> {
    let expected = cwd.to_string_lossy();
    match params.get("cwds").and_then(Value::as_array) {
        Some(cwds) if cwds.len() == 1 && cwds[0].as_str() == Some(expected.as_ref()) => Ok(()),
        _ => Err(ProviderError::Protocol(
            "native viewer scoped read used another or no cwd".to_owned(),
        )),
    }
}
fn bounded_page(params: &Value) -> Result<Value, ProviderError> {
    let mut object = params.as_object().cloned().ok_or_else(|| {
        ProviderError::Protocol("native viewer parameters must be an object".to_owned())
    })?;
    match object.get("limit") {
        Some(Value::Null) | None => {
            object.insert("limit".to_owned(), Value::from(MAX_PAGE_SIZE));
        }
        Some(value) => match value.as_u64() {
            Some(limit) if (1..=MAX_PAGE_SIZE).contains(&limit) => {}
            _ => {
                return Err(ProviderError::Protocol(
                    "native viewer page limit was invalid".to_owned(),
                ));
            }
        },
    }
    Ok(Value::Object(object))
}
fn ensure_empty_params(params: &Value, method: &str) -> Result<(), ProviderError> {
    match params.as_object() {
        Some(object) if object.is_empty() => Ok(()),
        _ => Err(ProviderError::Protocol(format!(
            "native viewer {method} parameters were not canonical"
        ))),
    }
}
fn notification_is_scoped(notification: &Value, session_id: &str) -> bool {
    notification.get("id").is_none()
        && notification.get("method").is_some()
        && (notification
            .pointer("/params/threadId")
            .and_then(Value::as_str)
            == Some(session_id)
            || notification
                .pointer("/params/thread/id")
                .and_then(Value::as_str)
                == Some(session_id))
}
fn force_no_direct_input(read: &mut Value) -> Result<(), ProviderError> {
    let thread = read.get_mut("thread").ok_or_else(|| {
        ProviderError::Protocol("native view thread/read omitted thread".to_owned())
    })?;
    force_no_direct_input_on_thread(thread)
}
fn force_no_direct_input_on_thread(thread: &mut Value) -> Result<(), ProviderError> {
    let object = thread.as_object_mut().ok_or_else(|| {
        ProviderError::Protocol("native view thread was not an object".to_owned())
    })?;
    object.insert("canAcceptDirectInput".to_owned(), Value::Bool(false));
    Ok(())
}
fn error_reply(id: &Value, message: &'static str) -> String {
    json!({"id": id, "error": {"code": -32600, "message": message}}).to_string()
}
fn client_error(error: &ProviderError) -> &'static str {
    match error {
        ProviderError::Unsupported { .. } => "native viewer method is not allowed",
        _ => "native viewer read failed; resync required",
    }
}
#[cfg(test)]
mod tests {
    use super::{MAX_PAGE_SIZE, bounded_page, ensure_cwds, ensure_thread, notification_is_scoped};
    use serde_json::json;
    #[test]
    fn rejects_wrong_thread_before_any_upstream_read() {
        assert!(ensure_thread(&json!({"threadId":"other"}), "bound").is_err());
    }
    #[test]
    fn bounds_native_history_pages() {
        assert!(bounded_page(&json!({"limit": MAX_PAGE_SIZE + 1})).is_err());
        assert!(bounded_page(&json!({"limit": 0})).is_err());
        assert!(bounded_page(&json!({"limit": "100"})).is_err());
        assert_eq!(
            bounded_page(&json!({})).unwrap()["limit"],
            json!(MAX_PAGE_SIZE)
        );
        assert_eq!(
            bounded_page(&json!({"limit": null})).unwrap()["limit"],
            json!(MAX_PAGE_SIZE)
        );
    }
    #[test]
    fn forwards_only_notifications_proven_to_be_bound_thread() {
        assert!(notification_is_scoped(
            &json!({"method":"item/agentMessage/delta","params":{"threadId":"bound"}}),
            "bound"
        ));
        assert!(!notification_is_scoped(
            &json!({"method":"account/updated","params":{}}),
            "bound"
        ));
        assert!(!notification_is_scoped(
            &json!({"id":"x","result":{}}),
            "bound"
        ));
        assert!(!notification_is_scoped(
            &json!({"method":"item/agentMessage/delta","params":{"threadId":"foreign","metadata":{"threadId":"bound"}}}),
            "bound"
        ));
    }
    #[test]
    fn rejects_empty_or_extra_cwd_scopes() {
        let cwd = std::path::Path::new("C:\\bound");
        assert!(ensure_cwds(&json!({"cwds": []}), cwd).is_err());
        assert!(ensure_cwds(&json!({"cwds": ["C:\\bound", "C:\\bound"]}), cwd).is_err());
        assert!(ensure_cwds(&json!({"cwds": ["C:\\bound"]}), cwd).is_ok());
    }
}
