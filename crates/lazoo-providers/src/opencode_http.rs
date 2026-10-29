//! `OpenCode`'s owned HTTP server transport.
//!
//! The server, session, MCP listener, and optional native attach view have the same lifetime.
//! In particular, an attach view is a restricted view of this session's server; it never starts a
//! second controller or silently creates a second `OpenCode` session.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex as LifecycleMutex},
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{
    Method, Request, StatusCode, Uri,
    body::Incoming,
    header::{AUTHORIZATION, CONTENT_TYPE},
};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use lazoo_runtime::{JobAccounting, JobChild, JobCommand};
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, Notify},
    time::{Instant, timeout},
};
use uuid::Uuid;

use crate::{
    AdapterFuture, PermissionDecision, PermissionKind, PermissionRequest, ProviderError,
    ProviderEvent, ProviderKind, ProviderSession, SessionRequest, TurnEventObserver, TurnRequest,
    mcp_bridge::SessionMcpBridge,
    opencode::{
        CONFIG_CONTENT_ENV, GOVERNED_PERMISSION, PERMISSION_ENV, SERVER_PASSWORD_ENV, config_home,
        governed_config_conflict, home_directory,
    },
    opencode_view::{self, OpenCodeNativeViewConfig},
};

const START_PHASE: &str = "OpenCode HTTP server startup";
const TURN_PHASE: &str = "OpenCode HTTP turn";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_SSE_EVENT_BYTES: usize = 256 * 1024;
const MAX_TURN_IDENTIFIERS: usize = 1024;
const MAX_TURN_IDENTIFIER_BYTES: usize = 512;
const MAX_TURN_RETAINED_TEXT_BYTES: usize = 1024 * 1024;

type HttpClient = Client<HttpConnector, Full<Bytes>>;

/// A canonical session owned by one `opencode serve` process.
///
/// Debug deliberately excludes the server password, MCP bearer token, and tool declarations.
pub struct OpenCodeHttpSession {
    child: Option<Arc<Mutex<JobChild>>>,
    client: HttpClient,
    server: SocketAddr,
    password: String,
    session_id: String,
    executable: PathBuf,
    cwd: PathBuf,
    bridge: SessionMcpBridge,
    turn_timeout: Duration,
    turn_lock: Arc<Mutex<()>>,
    model: Option<Value>,
    variant: Option<String>,
    declared_tools: HashSet<String>,
    cancelled: AtomicBool,
    poisoned: AtomicBool,
    active_turn: AtomicBool,
    cancel_notify: Notify,
    lifecycle: LifecycleMutex<()>,
}

impl std::fmt::Debug for OpenCodeHttpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenCodeHttpSession")
            .field("session_id", &self.session_id)
            .field("server", &self.server)
            .field("executable", &self.executable)
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

struct ActiveTurnGuard<'a> {
    session: &'a OpenCodeHttpSession,
    complete: bool,
}
impl ActiveTurnGuard<'_> {
    fn complete(&mut self) {
        self.complete = true;
    }
}
impl Drop for ActiveTurnGuard<'_> {
    fn drop(&mut self) {
        self.session.active_turn.store(false, Ordering::Release);
        if !self.complete {
            self.session.poisoned.store(true, Ordering::Release);
            if let Some(child) = &self.session.child
                && let Ok(child) = child.try_lock()
            {
                let _ = child.terminate_tree(1);
            }
        }
    }
}

impl OpenCodeHttpSession {
    pub(crate) async fn start(
        executable: &Path,
        request: &SessionRequest,
    ) -> Result<Self, ProviderError> {
        if let Some(detail) = governed_config_conflict(
            &request.cwd,
            &|name| std::env::var_os(name).is_some(),
            home_directory().as_deref(),
            config_home().as_deref(),
        ) {
            return Err(ProviderError::Unsupported {
                provider: ProviderKind::OpenCode,
                capability: "governed permissions beside an OpenCode permission, agent, plugin, or environment configuration",
                detail: Some(detail),
            });
        }
        let server = reserve_loopback()?;
        let (model, variant) = selected_model(request)?;
        let password = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let bridge = SessionMcpBridge::open(ProviderKind::OpenCode, request.tools.clone()).await?;
        let config = mcp_config(&bridge);
        let command = JobCommand::new(executable, &request.cwd)
            .args([
                OsString::from("serve"),
                OsString::from("--pure"),
                OsString::from("--hostname"),
                OsString::from("127.0.0.1"),
                OsString::from("--port"),
                OsString::from(server.port().to_string()),
            ])
            .env(PERMISSION_ENV, GOVERNED_PERMISSION)
            .env(SERVER_PASSWORD_ENV, &password)
            .env(CONFIG_CONTENT_ENV, config);
        let child = match JobChild::spawn(&command) {
            Ok(child) => Arc::new(Mutex::new(child)),
            Err(error) => {
                bridge.close();
                return Err(error.into());
            }
        };
        let client = loopback_client();
        let mut session = Self {
            child: Some(child),
            client,
            server,
            password,
            session_id: String::new(),
            executable: executable.to_path_buf(),
            cwd: request.cwd.clone(),
            bridge,
            turn_timeout: request.turn_timeout(),
            turn_lock: Arc::new(Mutex::new(())),
            model,
            variant,
            declared_tools: request.tools.iter().map(|tool| tool.name.clone()).collect(),
            cancelled: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            active_turn: AtomicBool::new(false),
            cancel_notify: Notify::new(),
            lifecycle: LifecycleMutex::new(()),
        };
        if let Err(error) = session.wait_ready(request.startup_timeout()).await {
            let _ = session.stop().await;
            return Err(error);
        }
        let result = if let Some(id) = request.session_id.as_deref() {
            session
                .get_json(&format!("/session/{id}"))
                .await
                .map(|_| id.to_owned())
        } else {
            session
                .post_json("/session", json!({"title":"Lazoo session"}))
                .await
                .and_then(|value| session_id(&value))
        };
        match result {
            Ok(id) => {
                session.session_id = id;
                Ok(session)
            }
            Err(error) => {
                let _ = session.stop().await;
                Err(error)
            }
        }
    }

    async fn wait_ready(&self, deadline: Duration) -> Result<(), ProviderError> {
        let until = Instant::now() + deadline;
        loop {
            if self.get_json("/session").await.is_ok() {
                return Ok(());
            }
            if Instant::now() >= until {
                return Err(ProviderError::Timeout { phase: START_PHASE });
            }
            {
                if let Some(child) = &self.child {
                    let mut child = child.lock().await;
                    if let Some(status) = child.try_wait()? {
                        return Err(ProviderError::ProcessExited {
                            code: status.code(),
                        });
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, Vec<u8>), ProviderError> {
        let uri: Uri = format!(
            "http://{}{}?directory={}",
            self.server,
            path,
            percent_encode(&self.cwd.to_string_lossy())
        )
        .parse()
        .map_err(|error| ProviderError::Protocol(format!("invalid OpenCode HTTP URI: {error}")))?;
        let bytes = body.map(|value| serde_json::to_vec(&value)).transpose()?;
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(AUTHORIZATION, basic_header(&self.password));
        if bytes.is_some() {
            builder = builder.header(CONTENT_TYPE, "application/json");
        }
        let request = builder
            .body(Full::new(Bytes::from(bytes.unwrap_or_default())))
            .map_err(|error| {
                ProviderError::Protocol(format!("invalid OpenCode HTTP request: {error}"))
            })?;
        let request_timeout = if path.ends_with("/message") {
            self.turn_timeout
        } else {
            HTTP_TIMEOUT
        };
        let response = timeout(request_timeout, self.client.request(request))
            .await
            .map_err(|_| ProviderError::Timeout { phase: TURN_PHASE })?
            .map_err(|error| {
                ProviderError::Protocol(format!("OpenCode HTTP request failed: {error}"))
            })?;
        let status = response.status();
        let bytes = timeout(request_timeout, collect_body(response.into_body()))
            .await
            .map_err(|_| ProviderError::Timeout { phase: TURN_PHASE })??;
        Ok((status, bytes))
    }

    async fn get_json(&self, path: &str) -> Result<Value, ProviderError> {
        self.json_response(Method::GET, path, None).await
    }
    async fn post_json(&self, path: &str, body: Value) -> Result<Value, ProviderError> {
        self.json_response(Method::POST, path, Some(body)).await
    }
    async fn json_response(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ProviderError> {
        let (status, bytes) = self.request(method, path, body).await?;
        if !status.is_success() {
            return Err(ProviderError::Protocol(format!(
                "OpenCode HTTP {path} returned {status}"
            )));
        }
        serde_json::from_slice(&bytes).map_err(ProviderError::from)
    }

    async fn turn_inner(
        &self,
        request: &TurnRequest,
        observer: Option<&dyn TurnEventObserver>,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        let _turn = self.turn_lock.lock().await;
        {
            let _lifecycle = self.lifecycle.lock().map_err(|_| {
                ProviderError::Protocol("OpenCode lifecycle state is poisoned".to_owned())
            })?;
            if self.poisoned.load(Ordering::Acquire) {
                return Err(ProviderError::Protocol(
                    "OpenCode HTTP session is poisoned after a failed turn".to_owned(),
                ));
            }
            if self.active_turn.load(Ordering::Acquire) {
                return Err(ProviderError::Protocol(
                    "OpenCode HTTP session already has an active turn".to_owned(),
                ));
            }
            self.cancelled.store(false, Ordering::Release);
            self.active_turn.store(true, Ordering::Release);
        }
        let mut active_guard = ActiveTurnGuard {
            session: self,
            complete: false,
        };
        let result = async {
            let message_id = format!("msg_{}", request.correlation_id.simple());
            let stream = self.open_event_stream().await?;
            if self.cancelled.load(Ordering::Acquire) {
                return Err(ProviderError::Protocol(
                    "OpenCode turn cancelled before prompt admission".to_owned(),
                ));
            }
            let path = format!("/session/{}/message", self.session_id);
            let mut body =
                json!({"messageID": message_id, "parts":[{"type":"text", "text": request.prompt}]});
            if let Some(model) = &self.model {
                body["model"] = model.clone();
            }
            if let Some(variant) = &self.variant {
                body["variant"] = Value::String(variant.clone());
            }
            let prompt = self.post_json(&path, body);
            tokio::pin!(prompt);
            let mut events = Vec::new();
            stream_turn(
                self,
                stream,
                &message_id,
                self.turn_timeout,
                observer,
                request.read_only,
                &self.declared_tools,
                &mut prompt,
                &mut events,
            )
            .await?;
            Ok(events)
        }
        .await;
        if result.is_err() {
            self.poisoned.store(true, Ordering::Release);
            let _ = self
                .post_json(&format!("/session/{}/abort", self.session_id), json!({}))
                .await;
        }
        if result.is_ok() {
            active_guard.complete();
        }
        result
    }

    async fn open_event_stream(&self) -> Result<Incoming, ProviderError> {
        let uri: Uri = format!(
            "http://{}/event?directory={}",
            self.server,
            percent_encode(&self.cwd.to_string_lossy())
        )
        .parse()
        .map_err(|error| ProviderError::Protocol(format!("invalid OpenCode event URI: {error}")))?;
        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(AUTHORIZATION, basic_header(&self.password))
            .body(Full::new(Bytes::new()))
            .map_err(|error| {
                ProviderError::Protocol(format!("invalid OpenCode event request: {error}"))
            })?;
        let response = timeout(HTTP_TIMEOUT, self.client.request(request))
            .await
            .map_err(|_| ProviderError::Timeout { phase: TURN_PHASE })?
            .map_err(|error| {
                ProviderError::Protocol(format!("OpenCode event stream failed: {error}"))
            })?;
        if !response.status().is_success() {
            return Err(ProviderError::Protocol(format!(
                "OpenCode event stream returned {}",
                response.status()
            )));
        }
        Ok(response.into_body())
    }
}

impl ProviderSession for OpenCodeHttpSession {
    fn id(&self) -> String {
        self.session_id.clone()
    }
    fn native_host_pid(&self) -> Option<u32> {
        self.child
            .as_ref()
            .and_then(|child| child.try_lock().ok().and_then(|child| child.id()))
    }
    fn resources(&self) -> Option<JobAccounting> {
        self.child.as_ref().and_then(|child| {
            child
                .try_lock()
                .ok()
                .and_then(|child| child.job().accounting().ok())
        })
    }
    fn native_view_available(&self) -> bool {
        if self.poisoned.load(Ordering::Acquire) {
            return false;
        }
        self.child.as_ref().is_some_and(|child| {
            child
                .try_lock()
                .is_ok_and(|mut child| child.try_wait().ok().flatten().is_none())
        })
    }
    fn open_native_view(&self) -> AdapterFuture<'_, Option<crate::NativeViewLease>> {
        Box::pin(async move {
            if self.poisoned.load(Ordering::Acquire) {
                return Err(ProviderError::Protocol(
                    "OpenCode HTTP session is poisoned".to_owned(),
                ));
            }
            opencode_view::open(OpenCodeNativeViewConfig {
                executable: self.executable.clone(),
                cwd: self.cwd.clone(),
                session_id: self.session_id.clone(),
                upstream: self.server,
                upstream_password: self.password.clone(),
            })
            .await
            .map(Some)
        })
    }
    fn turn<'a>(&'a self, request: &'a TurnRequest) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        Box::pin(async move { self.turn_inner(request, None).await })
    }
    fn turn_observed<'a>(
        &'a self,
        request: &'a TurnRequest,
        observer: &'a dyn TurnEventObserver,
    ) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        Box::pin(async move {
            self.bridge
                .during_turn(
                    &self.session_id,
                    Some(&format!("msg_{}", request.correlation_id.simple())),
                    observer,
                    self.turn_inner(request, Some(observer)),
                )
                .await
        })
    }
    fn cancel(&self) -> AdapterFuture<'_, ()> {
        Box::pin(async move {
            {
                let _lifecycle = self.lifecycle.lock().map_err(|_| {
                    ProviderError::Protocol("OpenCode lifecycle state is poisoned".to_owned())
                })?;
                if !self.active_turn.load(Ordering::Acquire) {
                    return Ok(());
                }
                self.cancelled.store(true, Ordering::Release);
                // Retire this host after cancellation so a delayed abort can never affect a
                // subsequent turn. Durable history can be resumed by a fresh owned server.
                self.poisoned.store(true, Ordering::Release);
                self.cancel_notify.notify_waiters();
            }
            let _ = self
                .post_json(&format!("/session/{}/abort", self.session_id), json!({}))
                .await;
            if let Some(child) = &self.child {
                let child = child.lock().await;
                child.terminate_tree(1)?;
            }
            Ok(())
        })
    }
    fn stop(&self) -> AdapterFuture<'_, ()> {
        Box::pin(async move {
            self.bridge.close();
            if let Some(child) = &self.child {
                let mut child = child.lock().await;
                if child.try_wait()?.is_none() {
                    child.terminate_tree(1)?;
                }
            }
            Ok(())
        })
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn stream_turn(
    session: &OpenCodeHttpSession,
    mut body: Incoming,
    client_message_id: &str,
    turn_timeout: Duration,
    observer: Option<&dyn TurnEventObserver>,
    read_only: bool,
    declared_tools: &HashSet<String>,
    prompt: &mut std::pin::Pin<
        &mut impl std::future::Future<Output = Result<Value, ProviderError>>,
    >,
    events: &mut Vec<ProviderEvent>,
) -> Result<(), ProviderError> {
    let deadline = Instant::now() + turn_timeout;
    let mut buffer = Vec::new();
    let mut state = TurnEventState::default();
    let mut prompt_finished = false;
    loop {
        if session.cancelled.load(Ordering::Acquire) {
            push_observed_event(
                events,
                observer,
                ProviderEvent::Cancelled {
                    provider: ProviderKind::OpenCode,
                    session_id: session.session_id.clone(),
                    turn_id: Some(client_message_id.to_owned()),
                    raw: None,
                },
            )?;
            return Ok(());
        }
        let remain = deadline
            .checked_duration_since(Instant::now())
            .ok_or(ProviderError::Timeout { phase: TURN_PHASE })?;
        let frame = tokio::select! {
            biased;
            () = wait_cancelled(session) => {
                push_observed_event(events, observer, ProviderEvent::Cancelled { provider: ProviderKind::OpenCode, session_id: session.session_id.clone(), turn_id: Some(client_message_id.to_owned()), raw: None })?;
                return Ok(());
            }
            result = &mut *prompt, if !prompt_finished => {
                result?;
                prompt_finished = true;
                continue;
            }
            result = timeout(remain, body.frame()) => result
                .map_err(|_| ProviderError::Timeout { phase: TURN_PHASE })?
                .transpose()
                .map_err(|error| ProviderError::Protocol(format!("OpenCode SSE failed: {error}")))?,
        };
        let Some(frame) = frame else {
            return Err(ProviderError::Protocol(
                "OpenCode event stream ended before turn completion".to_owned(),
            ));
        };
        let Some(data) = frame.data_ref() else {
            continue;
        };
        if buffer.len().saturating_add(data.len()) > MAX_SSE_EVENT_BYTES {
            return Err(ProviderError::Protocol(
                "OpenCode SSE event exceeded the limit".to_owned(),
            ));
        }
        buffer.extend_from_slice(data);
        while let Some(end) = sse_end(&buffer) {
            let record: Vec<u8> = buffer.drain(..end).collect();
            if record.len() > MAX_SSE_EVENT_BYTES {
                return Err(ProviderError::Protocol(
                    "OpenCode SSE event exceeded the limit".to_owned(),
                ));
            }
            let Some(value) = sse_json(&record)? else {
                continue;
            };
            if let Some(event) =
                normalize_event(&session.session_id, client_message_id, &value, &mut state)
            {
                if let ProviderEvent::PermissionRequested {
                    request_id: Some(id),
                    ..
                } = &event
                {
                    let Some(observer) = observer else {
                        session
                            .post_json(
                                &permission_reply_path(&session.session_id, id, &value),
                                permission_reply_body(&value, "reject"),
                            )
                            .await?;
                        events.push(event);
                        continue;
                    };
                    observer.observe(&event)?;
                    let permission = match permission_request(
                        &session.session_id,
                        client_message_id,
                        id,
                        &value,
                        read_only,
                        declared_tools,
                    ) {
                        Ok(permission) => permission,
                        Err(error) => {
                            session
                                .post_json(
                                    &permission_reply_path(&session.session_id, id, &value),
                                    permission_reply_body(&value, "reject"),
                                )
                                .await?;
                            events.push(ProviderEvent::Error {
                                provider: ProviderKind::OpenCode,
                                session_id: Some(session.session_id.clone()),
                                code: "OPENCODE_PERMISSION_DENIED".to_owned(),
                                message: error.to_string(),
                                retryable: false,
                                raw: Some(value.clone()),
                            });
                            continue;
                        }
                    };
                    let remaining = deadline
                        .checked_duration_since(Instant::now())
                        .ok_or(ProviderError::Timeout { phase: TURN_PHASE })?;
                    let decision = if is_declared_lazoo_mcp_action(
                        &permission.action,
                        declared_tools,
                    ) {
                        // The SessionMcpBridge and observer's call_tool gate still enforce
                        // declared-tool identity and task authority. Avoid a second native
                        // Ask decision for that same Lazoo-owned roster action.
                        PermissionDecision::Approve
                    } else {
                        tokio::select! {
                            biased;
                            () = wait_cancelled(session) => {
                                session.post_json(&permission_reply_path(&session.session_id, id, &value), permission_reply_body(&value, "reject")).await?;
                                push_observed_event(events, Some(observer), ProviderEvent::Cancelled { provider: ProviderKind::OpenCode, session_id: session.session_id.clone(), turn_id: Some(client_message_id.to_owned()), raw: Some(value.clone()) })?;
                                return Ok(());
                            }
                            result = timeout(remaining, observer.decide_permission(&permission)) => match result {
                                Ok(Ok(decision)) => decision,
                                Ok(Err(error)) => {
                                    session.post_json(&permission_reply_path(&session.session_id, id, &value), permission_reply_body(&value, "reject")).await?;
                                    return Err(error);
                                }
                                Err(_) => {
                                    session.post_json(&permission_reply_path(&session.session_id, id, &value), permission_reply_body(&value, "reject")).await?;
                                    return Err(ProviderError::Timeout { phase: TURN_PHASE });
                                }
                            },
                        }
                    };
                    let reply = if decision == PermissionDecision::Approve
                        && !session.cancelled.load(Ordering::Acquire)
                    {
                        "once"
                    } else {
                        "reject"
                    };
                    session
                        .post_json(
                            &permission_reply_path(&session.session_id, id, &value),
                            permission_reply_body(&value, reply),
                        )
                        .await?;
                } else if let Some(observer) = observer {
                    observer.observe(&event)?;
                }
                let complete = matches!(
                    event,
                    ProviderEvent::TurnCompleted { .. } | ProviderEvent::Cancelled { .. }
                );
                if matches!(event, ProviderEvent::Error { .. }) {
                    return Err(ProviderError::Protocol(
                        "OpenCode session reported a terminal error".to_owned(),
                    ));
                }
                events.push(event);
                if complete {
                    if !prompt_finished {
                        (&mut *prompt).await?;
                    }
                    return Ok(());
                }
            }
        }
    }
}

fn push_observed_event(
    events: &mut Vec<ProviderEvent>,
    observer: Option<&dyn TurnEventObserver>,
    event: ProviderEvent,
) -> Result<(), ProviderError> {
    if let Some(observer) = observer {
        observer.observe(&event)?;
    }
    events.push(event);
    Ok(())
}

async fn wait_cancelled(session: &OpenCodeHttpSession) {
    let notified = session.cancel_notify.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    if !session.cancelled.load(Ordering::Acquire) {
        notified.await;
    }
}

#[derive(Default)]
struct TurnEventState {
    admitted: bool,
    assistant_messages: HashSet<String>,
    text_parts: HashSet<String>,
    part_text: HashMap<String, String>,
    retained_text_bytes: usize,
}

fn turn_state_error(session_id: &str, value: &Value, message: &str) -> ProviderEvent {
    ProviderEvent::Error {
        provider: ProviderKind::OpenCode,
        session_id: Some(session_id.to_owned()),
        code: "OPENCODE_TURN_STATE_BOUND".to_owned(),
        message: message.to_owned(),
        retryable: false,
        raw: Some(value.clone()),
    }
}

fn identifier_exceeds_bound(value: &str, existing: &HashSet<String>) -> bool {
    value.len() > MAX_TURN_IDENTIFIER_BYTES
        || (!existing.contains(value) && existing.len() >= MAX_TURN_IDENTIFIERS)
}

#[allow(clippy::too_many_lines)]
fn normalize_event(
    session_id: &str,
    client_message_id: &str,
    value: &Value,
    state: &mut TurnEventState,
) -> Option<ProviderEvent> {
    let kind = value.get("type")?.as_str()?;
    let props = value.get("properties")?;
    let event_session = props
        .get("sessionID")
        .and_then(Value::as_str)
        .or_else(|| props.pointer("/info/sessionID").and_then(Value::as_str))
        .or_else(|| props.pointer("/part/sessionID").and_then(Value::as_str));
    if event_session != Some(session_id) {
        return None;
    }
    let parent = props
        .get("messageID")
        .and_then(Value::as_str)
        .or_else(|| props.pointer("/part/messageID").and_then(Value::as_str))
        .or_else(|| props.pointer("/tool/messageID").and_then(Value::as_str));
    match kind {
        "message.updated" => {
            let info = props.get("info")?;
            let id = info.get("id").and_then(Value::as_str)?;
            if id == client_message_id && !state.admitted {
                state.admitted = true;
                return Some(ProviderEvent::TurnStarted {
                    provider: ProviderKind::OpenCode,
                    session_id: session_id.to_owned(),
                    turn_id: Some(client_message_id.to_owned()),
                    raw: Some(value.clone()),
                });
            }
            if info.get("parentID").and_then(Value::as_str) == Some(client_message_id) {
                if identifier_exceeds_bound(id, &state.assistant_messages) {
                    return Some(turn_state_error(
                        session_id,
                        value,
                        "OpenCode turn message ownership exceeded its bound",
                    ));
                }
                state.assistant_messages.insert(id.to_owned());
            }
            None
        }
        "session.next.prompted" if parent == Some(client_message_id) && !state.admitted => {
            state.admitted = true;
            Some(ProviderEvent::TurnStarted {
                provider: ProviderKind::OpenCode,
                session_id: session_id.to_owned(),
                turn_id: Some(client_message_id.to_owned()),
                raw: Some(value.clone()),
            })
        }
        "message.part.updated" => {
            let part = props.get("part")?;
            if part.get("type").and_then(Value::as_str) != Some("text") {
                return None;
            }
            let message = part.get("messageID").and_then(Value::as_str)?;
            if !state.assistant_messages.contains(message) {
                return None;
            }
            let part_id = part.get("id").and_then(Value::as_str)?;
            let text = part.get("text").and_then(Value::as_str)?;
            if identifier_exceeds_bound(part_id, &state.text_parts) {
                return Some(turn_state_error(
                    session_id,
                    value,
                    "OpenCode turn part ownership exceeded its bound",
                ));
            }
            state.text_parts.insert(part_id.to_owned());
            let prior = state.part_text.entry(part_id.to_owned()).or_default();
            let delta = text.strip_prefix(prior.as_str())?;
            if delta.is_empty() {
                return None;
            }
            let added = text.len().saturating_sub(prior.len());
            if state.retained_text_bytes.saturating_add(added) > MAX_TURN_RETAINED_TEXT_BYTES {
                return Some(turn_state_error(
                    session_id,
                    value,
                    "OpenCode turn retained text exceeded its bound",
                ));
            }
            state.retained_text_bytes += added;
            text.clone_into(prior);
            Some(ProviderEvent::TextDelta {
                provider: ProviderKind::OpenCode,
                session_id: session_id.to_owned(),
                turn_id: Some(client_message_id.to_owned()),
                text: delta.to_owned(),
                raw: Some(value.clone()),
            })
        }
        "message.part.delta" => {
            let message = props.get("messageID").and_then(Value::as_str)?;
            if !state.assistant_messages.contains(message) {
                return None;
            }
            if props.get("field").and_then(Value::as_str) != Some("text") {
                return None;
            }
            let part_id = props.get("partID").and_then(Value::as_str)?;
            if !state.text_parts.contains(part_id) {
                return None;
            }
            let text = props.get("delta").and_then(Value::as_str)?;
            let prior = state.part_text.get_mut(part_id)?;
            if state.retained_text_bytes.saturating_add(text.len()) > MAX_TURN_RETAINED_TEXT_BYTES {
                return Some(turn_state_error(
                    session_id,
                    value,
                    "OpenCode turn retained text exceeded its bound",
                ));
            }
            state.retained_text_bytes += text.len();
            prior.push_str(text);
            Some(ProviderEvent::TextDelta {
                provider: ProviderKind::OpenCode,
                session_id: session_id.to_owned(),
                turn_id: Some(client_message_id.to_owned()),
                text: text.to_owned(),
                raw: Some(value.clone()),
            })
        }
        "permission.asked" | "permission.v2.asked"
            if parent == Some(client_message_id) || state.admitted =>
        {
            let id = props
                .get("requestID")
                .or_else(|| props.get("id"))
                .and_then(Value::as_str)?;
            Some(ProviderEvent::PermissionRequested {
                provider: ProviderKind::OpenCode,
                session_id: session_id.to_owned(),
                request_id: Some(id.to_owned()),
                options: vec!["once".to_owned(), "reject".to_owned()],
                raw: Some(value.clone()),
            })
        }
        "session.idle" if state.admitted => Some(ProviderEvent::TurnCompleted {
            provider: ProviderKind::OpenCode,
            session_id: session_id.to_owned(),
            turn_id: Some(client_message_id.to_owned()),
            status: "completed".to_owned(),
            output: None,
            raw: Some(value.clone()),
        }),
        "session.error"
            if state.admitted
                && (props.pointer("/error/type").and_then(Value::as_str)
                    == Some("MessageAbortedError")
                    || props.pointer("/error/name").and_then(Value::as_str)
                        == Some("MessageAbortedError")) =>
        {
            Some(ProviderEvent::Cancelled {
                provider: ProviderKind::OpenCode,
                session_id: session_id.to_owned(),
                turn_id: Some(client_message_id.to_owned()),
                raw: Some(value.clone()),
            })
        }
        "session.error" if state.admitted => Some(ProviderEvent::Error {
            provider: ProviderKind::OpenCode,
            session_id: Some(session_id.to_owned()),
            code: "OPENCODE_SESSION_ERROR".to_owned(),
            message: props
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("OpenCode reported an error")
                .to_owned(),
            retryable: false,
            raw: Some(value.clone()),
        }),
        _ => None,
    }
}

fn permission_request(
    session_id: &str,
    turn_id: &str,
    request_id: &str,
    raw: &Value,
    read_only: bool,
    declared_tools: &HashSet<String>,
) -> Result<PermissionRequest, ProviderError> {
    let props = raw.get("properties").ok_or_else(|| {
        ProviderError::InvalidEvent("permission event has no properties".to_owned())
    })?;
    let action = props
        .get("permission")
        .or_else(|| props.get("action"))
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::InvalidEvent("permission event has no action".to_owned()))?;
    // OpenCode namespaces MCP calls as `<server>_<tool>`. This bridge owns only the
    // `lazoo` server, so accept its one prefix only after the unprefixed tool name
    // matches the turn's declared catalog.
    let declared_mcp = is_declared_lazoo_mcp_action(action, declared_tools);
    let scope_expansion = matches!(action, "bash" | "command" | "shell");
    let kind = match action {
        // These actions are read-only by OpenCode's native contract, but are still submitted to
        // the observer rather than silently approved.
        "read" | "glob" | "grep" | "list" | "webfetch" | "websearch" => {
            PermissionKind::CommandExecution
        }
        "bash" | "command" | "shell" => {
            if read_only {
                return Err(ProviderError::InvalidEvent(
                    "read-only turn declined command execution".to_owned(),
                ));
            }
            PermissionKind::CommandExecution
        }
        "edit" | "write" | "patch" | "todowrite" => {
            if read_only {
                return Err(ProviderError::InvalidEvent(
                    "read-only turn declined file or state mutation".to_owned(),
                ));
            }
            PermissionKind::FileChange
        }
        // A task creates another native controller and external_directory widens filesystem
        // scope. Neither is safe for a canonical single-controller session.
        "task" | "subagent" | "external_directory" => {
            return Err(ProviderError::InvalidEvent(
                "unsafe OpenCode permission action".to_owned(),
            ));
        }
        _ if declared_mcp => {
            if read_only {
                return Err(ProviderError::InvalidEvent(
                    "read-only turn declined Lazoo MCP tool".to_owned(),
                ));
            }
            PermissionKind::CommandExecution
        }
        _ => {
            return Err(ProviderError::InvalidEvent(
                "unknown OpenCode permission action".to_owned(),
            ));
        }
    };
    Ok(PermissionRequest {
        provider: ProviderKind::OpenCode,
        session_id: session_id.to_owned(),
        turn_id: Some(turn_id.to_owned()),
        request_id: request_id.to_owned(),
        kind,
        action: action.to_owned(),
        scope_expansion,
        detail: props.clone(),
    })
}

fn is_declared_lazoo_mcp_action(action: &str, declared_tools: &HashSet<String>) -> bool {
    action
        .strip_prefix("lazoo_")
        .is_some_and(|tool| declared_tools.contains(tool))
}

fn session_id(value: &Value) -> Result<String, ProviderError> {
    value
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            ProviderError::InvalidEvent("OpenCode session create response has no id".to_owned())
        })
}

fn permission_reply_path(session_id: &str, request_id: &str, raw: &Value) -> String {
    if raw.get("type").and_then(Value::as_str) == Some("permission.v2.asked") {
        format!("/api/session/{session_id}/permission/{request_id}/reply")
    } else {
        format!("/session/{session_id}/permissions/{request_id}")
    }
}

fn permission_reply_body(raw: &Value, reply: &str) -> Value {
    if raw.get("type").and_then(Value::as_str) == Some("permission.v2.asked") {
        json!({"reply": reply})
    } else {
        json!({"response": reply})
    }
}

async fn collect_body(mut body: Incoming) -> Result<Vec<u8>, ProviderError> {
    let mut output = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| {
            ProviderError::Protocol(format!("OpenCode HTTP response failed: {error}"))
        })?;
        let Some(data) = frame.data_ref() else {
            continue;
        };
        if output.len().saturating_add(data.len()) > MAX_RESPONSE_BYTES {
            return Err(ProviderError::Protocol(
                "OpenCode HTTP response exceeded the limit".to_owned(),
            ));
        }
        output.extend_from_slice(data);
    }
    Ok(output)
}

fn selected_model(
    request: &SessionRequest,
) -> Result<(Option<Value>, Option<String>), ProviderError> {
    let model = match request.model.as_deref() {
        None => None,
        Some(value) => {
            let Some((provider_id, model_id)) = value.split_once('/') else {
                return Err(ProviderError::Unsupported {
                    provider: ProviderKind::OpenCode,
                    capability: "an OpenCode model in provider/model form",
                    detail: Some("model selection was not provider/model".to_owned()),
                });
            };
            if provider_id.is_empty() || model_id.is_empty() || model_id.contains('/') {
                return Err(ProviderError::Unsupported {
                    provider: ProviderKind::OpenCode,
                    capability: "an OpenCode model in provider/model form",
                    detail: Some("model selection was not provider/model".to_owned()),
                });
            }
            Some(json!({"providerID": provider_id, "modelID": model_id}))
        }
    };
    let variant = request
        .effort
        .as_ref()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    Ok((model, variant))
}
fn reserve_loopback() -> Result<SocketAddr, ProviderError> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;
    drop(listener);
    Ok(address)
}
fn loopback_client() -> HttpClient {
    let mut connector = HttpConnector::new();
    connector.enforce_http(true);
    Client::builder(TokioExecutor::new()).build(connector)
}
fn mcp_config(bridge: &SessionMcpBridge) -> String {
    json!({"mcp":{"lazoo":{"type":"remote","url":bridge.endpoint(),"headers":{"Authorization":bridge.authorization()}}}}).to_string()
}
fn basic_header(password: &str) -> String {
    format!("Basic {}", base64(&format!("opencode:{password}")))
}
fn base64(value: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    value
        .as_bytes()
        .chunks(3)
        .flat_map(|part| {
            let bits = (u32::from(part[0]) << 16)
                | (u32::from(*part.get(1).unwrap_or(&0)) << 8)
                | u32::from(*part.get(2).unwrap_or(&0));
            [
                TABLE[((bits >> 18) & 63) as usize] as char,
                TABLE[((bits >> 12) & 63) as usize] as char,
                if part.len() > 1 {
                    TABLE[((bits >> 6) & 63) as usize] as char
                } else {
                    '='
                },
                if part.len() > 2 {
                    TABLE[(bits & 63) as usize] as char
                } else {
                    '='
                },
            ]
        })
        .collect()
}
fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![char::from(byte)]
            }
            b' ' => vec!['+'],
            byte => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}
fn sse_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(2)
        .position(|pair| pair == b"\n\n")
        .map(|index| index + 2)
        .or_else(|| {
            bytes
                .windows(4)
                .position(|pair| pair == b"\r\n\r\n")
                .map(|index| index + 4)
        })
}
fn sse_json(record: &[u8]) -> Result<Option<Value>, ProviderError> {
    let payload: Vec<u8> = record
        .split(|byte| *byte == b'\n')
        .filter_map(|line| line.strip_prefix(b"data:"))
        .flat_map(|line| line.iter().copied().chain(std::iter::once(b'\n')))
        .collect();
    if payload.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(ProviderError::from)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use hyper::{Response, service::service_fn};
    use hyper_util::rt::TokioIo;
    use tokio::sync::mpsc;

    use super::*;

    #[derive(Default)]
    struct Observer(
        StdMutex<Vec<PermissionRequest>>,
        StdMutex<Vec<ProviderEvent>>,
    );
    impl TurnEventObserver for Observer {
        fn observe(&self, event: &ProviderEvent) -> Result<(), ProviderError> {
            self.1.lock().unwrap().push(event.clone());
            Ok(())
        }
        fn decide_permission<'a>(
            &'a self,
            request: &'a PermissionRequest,
        ) -> AdapterFuture<'a, PermissionDecision> {
            self.0.lock().unwrap().push(request.clone());
            Box::pin(async { Ok(PermissionDecision::Approve) })
        }
    }

    struct RejectingObserver;
    impl TurnEventObserver for RejectingObserver {
        fn observe(&self, _: &ProviderEvent) -> Result<(), ProviderError> {
            Ok(())
        }
        fn decide_permission<'a>(
            &'a self,
            _: &'a PermissionRequest,
        ) -> AdapterFuture<'a, PermissionDecision> {
            Box::pin(async { Ok(PermissionDecision::Decline) })
        }
    }

    #[derive(Default)]
    struct StalledObserver(StdMutex<Vec<ProviderEvent>>);
    impl TurnEventObserver for StalledObserver {
        fn observe(&self, event: &ProviderEvent) -> Result<(), ProviderError> {
            self.0.lock().unwrap().push(event.clone());
            Ok(())
        }
        fn decide_permission<'a>(
            &'a self,
            _: &'a PermissionRequest,
        ) -> AdapterFuture<'a, PermissionDecision> {
            Box::pin(std::future::pending())
        }
    }

    async fn mock_session(
        events: &'static str,
    ) -> (
        OpenCodeHttpSession,
        mpsc::UnboundedReceiver<(String, Value)>,
    ) {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, seen_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let seen_tx = seen_tx.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request: Request<Incoming>| {
                        let seen_tx = seen_tx.clone();
                        async move {
                            let path = request.uri().path().to_owned();
                            let body = request.into_body().collect().await.unwrap().to_bytes();
                            let value = if body.is_empty() {
                                Value::Null
                            } else {
                                serde_json::from_slice(&body).unwrap()
                            };
                            let _ = seen_tx.send((path.clone(), value));
                            let body = if path == "/event" {
                                events.to_owned()
                            } else {
                                "true".to_owned()
                            };
                            Ok::<_, std::convert::Infallible>(Response::new(Full::new(
                                Bytes::from(body),
                            )))
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        let bridge = SessionMcpBridge::open(ProviderKind::OpenCode, vec![])
            .await
            .unwrap();
        (
            OpenCodeHttpSession {
                child: None,
                client: loopback_client(),
                server: address,
                password: "test".to_owned(),
                session_id: "ses_owned".to_owned(),
                executable: PathBuf::from("opencode"),
                cwd: PathBuf::from(r"C:\repo"),
                bridge,
                turn_timeout: Duration::from_secs(2),
                turn_lock: Arc::new(Mutex::new(())),
                model: None,
                variant: None,
                declared_tools: HashSet::new(),
                cancelled: AtomicBool::new(false),
                poisoned: AtomicBool::new(false),
                active_turn: AtomicBool::new(false),
                cancel_notify: Notify::new(),
                lifecycle: LifecycleMutex::new(()),
            },
            seen_rx,
        )
    }

    #[tokio::test]
    async fn transport_posts_prompt_filters_foreign_events_and_replies_once() {
        let events = concat!(
            "data: {\"type\":\"session.next.prompted\",\"properties\":{\"sessionID\":\"foreign\",\"messageID\":\"msg_x\"}}\n\n",
            "data: {\"type\":\"message.updated\",\"properties\":{\"info\":{\"id\":\"msg_00000000000000000000000000000000\",\"sessionID\":\"ses_owned\"}}}\n\n",
            "data: {\"type\":\"message.updated\",\"properties\":{\"info\":{\"id\":\"msg_assistant\",\"parentID\":\"msg_00000000000000000000000000000000\",\"sessionID\":\"ses_owned\"}}}\n\n",
            "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"prt_hello\",\"type\":\"text\",\"sessionID\":\"ses_owned\",\"messageID\":\"msg_assistant\",\"text\":\"hello\"}}}\n\n",
            "data: {\"type\":\"permission.asked\",\"properties\":{\"sessionID\":\"ses_owned\",\"requestID\":\"perm_1\",\"permission\":\"bash\",\"tool\":{\"messageID\":\"msg_00000000000000000000000000000000\"}}}\n\n",
            "data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"ses_owned\"}}\n\n"
        );
        let (session, mut seen) = mock_session(events).await;
        let observer = Observer::default();
        let mut turn = TurnRequest::new("say hello");
        turn.correlation_id = Uuid::nil();
        let output = tokio::time::timeout(
            Duration::from_secs(1),
            session.turn_observed(&turn, &observer),
        )
        .await
        .expect("transport turn timed out")
        .unwrap();
        assert!(output.iter().any(
            |event| matches!(event, ProviderEvent::TextDelta { text, .. } if text == "hello")
        ));
        assert_eq!(observer.0.lock().unwrap().len(), 1);
        let captured = observer.1.lock().unwrap().clone();
        assert!(matches!(
            captured.first(),
            Some(ProviderEvent::TurnStarted { .. })
        ));
        assert_eq!(
            captured
                .iter()
                .filter(|event| matches!(event, ProviderEvent::TurnStarted { .. }))
                .count(),
            1
        );
        let mut paths = Vec::new();
        for _ in 0..3 {
            paths.push(
                tokio::time::timeout(Duration::from_secs(1), seen.recv())
                    .await
                    .expect("mock route timed out")
                    .expect("mock server closed"),
            );
        }
        assert!(
            paths
                .iter()
                .any(|(path, body)| path == "/session/ses_owned/message"
                    && body["messageID"] == "msg_00000000000000000000000000000000"
                    && body["parts"][0]["text"] == "say hello")
        );
        assert!(paths.iter().any(
            |(path, body)| path == "/session/ses_owned/permissions/perm_1"
                && body["response"] == "once"
        ));
        session.cancel().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), seen.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn transport_rejects_a_governed_permission() {
        let events = concat!(
            "data: {\"type\":\"session.next.prompted\",\"properties\":{\"sessionID\":\"ses_owned\",\"messageID\":\"msg_00000000000000000000000000000000\"}}\n\n",
            "data: {\"type\":\"permission.asked\",\"properties\":{\"sessionID\":\"ses_owned\",\"requestID\":\"perm_reject\",\"permission\":\"bash\",\"tool\":{\"messageID\":\"msg_00000000000000000000000000000000\"}}}\n\n",
            "data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"ses_owned\"}}\n\n"
        );
        let (session, mut seen) = mock_session(events).await;
        let mut turn = TurnRequest::new("no");
        turn.correlation_id = Uuid::nil();
        session
            .turn_observed(&turn, &RejectingObserver)
            .await
            .unwrap();
        let mut reply = None;
        for _ in 0..3 {
            let value = seen.recv().await.unwrap();
            if value.0.contains("permissions") {
                reply = Some(value.1);
            }
        }
        assert_eq!(reply.unwrap()["response"], "reject");
    }

    #[tokio::test]
    async fn transport_without_observer_rejects_permission_instead_of_stalling() {
        let events = concat!(
            "data: {\"type\":\"message.updated\",\"properties\":{\"info\":{\"id\":\"msg_00000000000000000000000000000000\",\"sessionID\":\"ses_owned\"}}}\n\n",
            "data: {\"type\":\"permission.asked\",\"properties\":{\"sessionID\":\"ses_owned\",\"requestID\":\"perm_default\",\"permission\":\"bash\"}}\n\n",
            "data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"ses_owned\"}}\n\n"
        );
        let (session, mut seen) = mock_session(events).await;
        let mut turn = TurnRequest::new("no observer");
        turn.correlation_id = Uuid::nil();
        session.turn(&turn).await.unwrap();
        let mut reject = None;
        for _ in 0..3 {
            let item = seen.recv().await.unwrap();
            if item.0.contains("permissions") {
                reject = Some(item.1);
            }
        }
        assert_eq!(reject.unwrap()["response"], "reject");
    }

    #[tokio::test]
    async fn cancelled_stalled_observer_does_not_hold_the_turn() {
        let events = concat!(
            "data: {\"type\":\"message.updated\",\"properties\":{\"info\":{\"id\":\"msg_00000000000000000000000000000000\",\"sessionID\":\"ses_owned\"}}}\n\n",
            "data: {\"type\":\"permission.asked\",\"properties\":{\"sessionID\":\"ses_owned\",\"requestID\":\"perm_stall\",\"permission\":\"bash\"}}\n\n"
        );
        let (session, _) = mock_session(events).await;
        let session = Arc::new(session);
        let worker = Arc::clone(&session);
        let observer = Arc::new(StalledObserver::default());
        let captured = Arc::clone(&observer);
        let turn = TurnRequest {
            correlation_id: Uuid::nil(),
            prompt: "cancel".to_owned(),
            permission_mode: None,
            read_only: false,
        };
        let task =
            tokio::spawn(async move { worker.turn_observed(&turn, observer.as_ref()).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if captured
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| matches!(event, ProviderEvent::PermissionRequested { .. }))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("permission must reach the observer before cancellation");
        session.cancel().await.unwrap();
        let events = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("cancel must release observer")
            .unwrap()
            .unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ProviderEvent::Cancelled { .. }))
        );
        assert!(
            captured
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, ProviderEvent::Cancelled { .. }))
        );
    }

    #[tokio::test]
    async fn transport_replies_to_v2_permission_at_its_native_route() {
        let events = concat!(
            "data: {\"type\":\"session.next.prompted\",\"properties\":{\"sessionID\":\"ses_owned\",\"messageID\":\"msg_00000000000000000000000000000000\"}}\n\n",
            "data: {\"type\":\"permission.v2.asked\",\"properties\":{\"sessionID\":\"ses_owned\",\"id\":\"v2_1\",\"action\":\"read\",\"source\":{\"type\":\"tool\",\"messageID\":\"msg_00000000000000000000000000000000\",\"callID\":\"call\"}}}\n\n",
            "data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"ses_owned\"}}\n\n"
        );
        let (session, mut seen) = mock_session(events).await;
        let mut turn = TurnRequest::new("read");
        turn.correlation_id = Uuid::nil();
        session
            .turn_observed(&turn, &Observer::default())
            .await
            .unwrap();
        let mut reply = None;
        for _ in 0..3 {
            let item = seen.recv().await.unwrap();
            if item.0.contains("/api/session/") {
                reply = Some(item);
            }
        }
        let (path, body) = reply.unwrap();
        assert_eq!(path, "/api/session/ses_owned/permission/v2_1/reply");
        assert_eq!(body["reply"], "once");
    }

    #[test]
    fn cancelled_error_and_model_selection_are_normalized() {
        let mut active = TurnEventState {
            admitted: true,
            ..Default::default()
        };
        let cancelled = json!({"type":"session.error","properties":{"sessionID":"ses","error":{"name":"MessageAbortedError","data":{"message":"aborted"}}}});
        assert!(matches!(
            normalize_event("ses", "msg_1", &cancelled, &mut active),
            Some(ProviderEvent::Cancelled { .. })
        ));
        let request = SessionRequest::new(".")
            .with_model("opencode/big-pickle")
            .with_effort("high");
        assert_eq!(
            selected_model(&request).unwrap(),
            (
                Some(json!({"providerID":"opencode","modelID":"big-pickle"})),
                Some("high".to_owned())
            )
        );
        assert!(selected_model(&SessionRequest::new(".").with_model("not-a-model")).is_err());
    }

    #[test]
    fn read_only_permission_policy_is_explicit_and_fail_closed() {
        let raw = json!({"properties":{"permission":"read"}});
        assert!(permission_request("ses", "msg", "perm", &raw, true, &HashSet::new()).is_ok());
        let mutation = json!({"properties":{"permission":"write"}});
        assert!(
            permission_request("ses", "msg", "perm", &mutation, true, &HashSet::new()).is_err()
        );
        let unknown = json!({"properties":{"permission":"new-native-tool"}});
        assert!(
            permission_request("ses", "msg", "perm", &unknown, false, &HashSet::new()).is_err()
        );
        let mut catalog = HashSet::new();
        catalog.insert("lazoo_query".to_owned());
        let mcp = json!({"properties":{"permission":"lazoo_query"}});
        assert!(permission_request("ses", "msg", "perm", &mcp, false, &catalog).is_err());
        let namespaced = json!({"properties":{"permission":"lazoo_lazoo_query"}});
        assert!(permission_request("ses", "msg", "perm", &namespaced, false, &catalog).is_ok());
        let foreign = json!({"properties":{"permission":"other_lazoo_query"}});
        assert!(permission_request("ses", "msg", "perm", &foreign, false, &catalog).is_err());
    }

    #[test]
    fn turn_state_bounds_fail_closed_without_truncating_output() {
        let mut state = TurnEventState::default();
        state.assistant_messages.insert("assistant".to_owned());
        state.text_parts.insert("part".to_owned());
        state.part_text.insert("part".to_owned(), String::new());
        state.retained_text_bytes = MAX_TURN_RETAINED_TEXT_BYTES;
        let delta = json!({"type":"message.part.delta","properties":{"sessionID":"ses","messageID":"assistant","partID":"part","field":"text","delta":"x"}});
        assert!(matches!(
            normalize_event("ses", "user", &delta, &mut state),
            Some(ProviderEvent::Error { .. })
        ));
        assert_eq!(state.part_text["part"], "");

        let oversized_id = "x".repeat(MAX_TURN_IDENTIFIER_BYTES + 1);
        let message = json!({"type":"message.updated","properties":{"info":{"sessionID":"ses","id":oversized_id,"parentID":"user"}}});
        assert!(matches!(
            normalize_event("ses", "user", &message, &mut state),
            Some(ProviderEvent::Error { .. })
        ));
        let identifiers = (0..MAX_TURN_IDENTIFIERS).map(|i| i.to_string()).collect();
        assert!(identifier_exceeds_bound("new", &identifiers));
        assert!(!identifier_exceeds_bound("0", &identifiers));
    }

    #[test]
    fn legacy_message_and_part_events_only_emit_owned_assistant_deltas() {
        let mut state = TurnEventState::default();
        let user = json!({"type":"message.updated","properties":{"info":{"id":"msg_user","sessionID":"ses"}}});
        assert!(matches!(
            normalize_event("ses", "msg_user", &user, &mut state),
            Some(ProviderEvent::TurnStarted { turn_id: Some(id), .. }) if id == "msg_user"
        ));
        assert!(normalize_event("ses", "msg_user", &user, &mut state).is_none());
        let duplicate = json!({"type":"session.next.prompted","properties":{"sessionID":"ses","messageID":"msg_user"}});
        assert!(normalize_event("ses", "msg_user", &duplicate, &mut state).is_none());
        let assistant = json!({"type":"message.updated","properties":{"info":{"id":"msg_assistant","parentID":"msg_user","sessionID":"ses"}}});
        assert!(normalize_event("ses", "msg_user", &assistant, &mut state).is_none());
        let first = json!({"type":"message.part.updated","properties":{"part":{"id":"prt_1","type":"text","sessionID":"ses","messageID":"msg_assistant","text":"hello"}}});
        let second = json!({"type":"message.part.updated","properties":{"part":{"id":"prt_1","type":"text","sessionID":"ses","messageID":"msg_assistant","text":"hello world"}}});
        assert!(
            matches!(normalize_event("ses", "msg_user", &first, &mut state), Some(ProviderEvent::TextDelta { text, .. }) if text == "hello")
        );
        assert!(
            matches!(normalize_event("ses", "msg_user", &second, &mut state), Some(ProviderEvent::TextDelta { text, .. }) if text == " world")
        );
        let delta = json!({"type":"message.part.delta","properties":{"sessionID":"ses","messageID":"msg_assistant","partID":"prt_1","field":"text","delta":"!"}});
        let cumulative = json!({"type":"message.part.updated","properties":{"part":{"id":"prt_1","type":"text","sessionID":"ses","messageID":"msg_assistant","text":"hello world!"}}});
        assert!(
            matches!(normalize_event("ses", "msg_user", &delta, &mut state), Some(ProviderEvent::TextDelta { text, .. }) if text == "!")
        );
        assert!(normalize_event("ses", "msg_user", &cumulative, &mut state).is_none());
        let reasoning = json!({"type":"message.part.updated","properties":{"part":{"id":"prt_r","type":"reasoning","sessionID":"ses","messageID":"msg_assistant","text":"private"}}});
        assert!(normalize_event("ses", "msg_user", &reasoning, &mut state).is_none());
        let non_text_field = json!({"type":"message.part.delta","properties":{"sessionID":"ses","messageID":"msg_assistant","partID":"prt_1","field":"input","delta":"private"}});
        assert!(normalize_event("ses", "msg_user", &non_text_field, &mut state).is_none());
        let empty = json!({"type":"message.part.updated","properties":{"part":{"id":"prt_empty","type":"text","sessionID":"ses","messageID":"msg_assistant","text":""}}});
        let after_empty = json!({"type":"message.part.delta","properties":{"sessionID":"ses","messageID":"msg_assistant","partID":"prt_empty","field":"text","delta":"hello"}});
        assert!(normalize_event("ses", "msg_user", &empty, &mut state).is_none());
        assert!(
            matches!(normalize_event("ses", "msg_user", &after_empty, &mut state), Some(ProviderEvent::TextDelta { text, .. }) if text == "hello")
        );
        let foreign = json!({"type":"message.part.delta","properties":{"sessionID":"ses","messageID":"foreign","partID":"x","field":"text","delta":"no"}});
        assert!(normalize_event("ses", "msg_user", &foreign, &mut state).is_none());
    }
}
