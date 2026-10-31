//! Restricted HTTP/SSE bridge for `OpenCode`'s native attach route.
//!
//! The attached `OpenCode` TUI is a viewer for the HTTP-owned canonical session. This bridge
//! exposes only that session's snapshot, history, filtered events, and sanitized bootstrap
//! metadata. Prompts, approvals, interrupts, and MCP mutations can never reach the server.

use std::{
    convert::Infallible,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Frame, Incoming},
    header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, ORIGIN},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::{TokioExecutor, TokioIo, TokioTimer},
};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, mpsc, watch},
    time::timeout,
};
use uuid::Uuid;

use crate::native_view::NativeViewBackend;
use crate::{CommandSpec, NativeViewLaunchDescriptor, NativeViewLease, ProviderError};

const TOKEN_ENV: &str = "OPENCODE_SERVER_PASSWORD";
const DIRECTORY_HEADER: &str = "x-opencode-directory";
const MAX_BODY: usize = 512 * 1024;
const MAX_METADATA_BODY: usize = 4 * 1024 * 1024;
const MAX_SSE_EVENT: usize = 64 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: usize = 8;
type Body = BoxBody<Bytes, Infallible>;

#[derive(Clone, Debug)]
pub(crate) struct OpenCodeNativeViewConfig {
    pub executable: PathBuf,
    pub cwd: PathBuf,
    pub session_id: String,
    pub upstream: SocketAddr,
    pub upstream_password: String,
    pub launch_environment: lazoo_runtime::LaunchEnvironment,
}

pub(crate) async fn open(
    config: OpenCodeNativeViewConfig,
) -> Result<NativeViewLease, ProviderError> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let address = listener.local_addr()?;
    let (closed_tx, closed_rx) = watch::channel(false);
    let state = Arc::new(State {
        closed: AtomicBool::new(false),
        connections: AtomicUsize::new(0),
        permits: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        closed_tx,
        address,
        viewer_password: token(),
        upstream: config.upstream,
        upstream_password: config.upstream_password,
        session_id: config.session_id,
        executable: config.executable,
        cwd: config.cwd,
        launch_environment: config.launch_environment,
    });
    tokio::spawn(accept(listener, Arc::clone(&state), closed_rx));
    Ok(NativeViewLease::from_backend(Backend(state)))
}

struct State {
    closed: AtomicBool,
    connections: AtomicUsize,
    permits: Arc<Semaphore>,
    closed_tx: watch::Sender<bool>,
    address: SocketAddr,
    viewer_password: String,
    upstream: SocketAddr,
    upstream_password: String,
    session_id: String,
    executable: PathBuf,
    cwd: PathBuf,
    launch_environment: lazoo_runtime::LaunchEnvironment,
}
struct Backend(Arc<State>);
impl NativeViewBackend for Backend {
    fn close(&self) {
        if !self.0.closed.swap(true, Ordering::AcqRel) {
            self.0.closed_tx.send_replace(true);
        }
    }
    fn is_closed(&self) -> bool {
        self.0.closed.load(Ordering::Acquire)
    }
    fn closed_subscription(&self) -> watch::Receiver<bool> {
        self.0.closed_tx.subscribe()
    }
    fn launch_descriptor(&self) -> NativeViewLaunchDescriptor {
        let remote_url = format!("http://{}", self.0.address);
        let mut command = CommandSpec::new(&self.0.executable, &self.0.cwd)
            .arg("attach")
            .arg(&remote_url)
            .arg("--session")
            .arg(&self.0.session_id)
            .arg("--dir")
            .arg(&self.0.cwd)
            .arg("--pure");
        command.apply_environment(&self.0.launch_environment);
        command
            .env
            .insert(TOKEN_ENV.into(), self.0.viewer_password.clone().into());
        NativeViewLaunchDescriptor::Remote {
            remote_url,
            command,
        }
    }
}

async fn accept(listener: TcpListener, state: Arc<State>, mut closed: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            changed = closed.changed() => { let _ = changed; return; }
            incoming = listener.accept() => {
                let Ok((stream, peer)) = incoming else { return; };
                if !peer.ip().is_loopback() || state.closed.load(Ordering::Acquire) { continue; }
            let Ok(permit) = Arc::clone(&state.permits).try_acquire_owned() else { continue; };
            state.connections.fetch_add(1, Ordering::AcqRel);
                let task_state = Arc::clone(&state); let mut task_closed = state.closed_tx.subscribe();
                tokio::spawn(async move {
                    let handler_state = Arc::clone(&task_state);
                let mut builder = http1::Builder::new();
                builder.timer(TokioTimer::new()).header_read_timeout(HEADER_TIMEOUT).max_buf_size(64 * 1024).max_headers(32);
                let connection = builder.serve_connection(TokioIo::new(stream), service_fn(move |request| handle(Arc::clone(&handler_state), request)));
                    tokio::select! { result = connection => { let _ = result; } changed = task_closed.changed() => { let _ = changed; } }
                    task_state.connections.fetch_sub(1, Ordering::AcqRel);
                drop(permit);
                });
            }
        }
    }
}

async fn handle(
    state: Arc<State>,
    request: Request<Incoming>,
) -> Result<Response<Body>, Infallible> {
    Ok(
        match timeout(IO_TIMEOUT, handle_inner(state, request)).await {
            Ok(response) => response,
            Err(_) => refuse(StatusCode::GATEWAY_TIMEOUT),
        },
    )
}
async fn handle_inner(state: Arc<State>, request: Request<Incoming>) -> Response<Body> {
    if state.closed.load(Ordering::Acquire) {
        return refuse(StatusCode::GONE);
    }
    if request.headers().contains_key(ORIGIN) {
        return refuse(StatusCode::FORBIDDEN);
    }
    if !authorized(&request, &state.viewer_password) {
        return refuse(StatusCode::UNAUTHORIZED);
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return refuse(StatusCode::FORBIDDEN);
    }
    let Some(route) = scoped_route(&request, &state.session_id, &state.cwd) else {
        return refuse(StatusCode::FORBIDDEN);
    };
    // The TUI asks for this global configuration only to initialize its preferences. Do not
    // proxy it: an OpenCode config can contain provider options, environment values, or MCP
    // settings. The client has safe defaults for absent preferences.
    if route == Route::Config {
        return json_response(&serde_json::json!({}));
    }
    // `/provider` is OpenCode's global catalog, which can exceed the bridge's
    // bounded metadata budget and is not required to render the selected
    // session. The selected provider binding is separately exposed through
    // the sanitized `/config/providers` endpoint.
    if route == Route::Provider {
        return json_response(&serde_json::json!({
            "all": [],
            "default": {},
            "connected": [],
        }));
    }
    if let Some(metadata) = local_metadata(route, &state.cwd) {
        return json_response(&metadata);
    }
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |value| value.as_str());
    let uri: hyper::Uri = match format!("http://{}{}", state.upstream, path).parse() {
        Ok(uri) => uri,
        Err(_) => return refuse(StatusCode::BAD_REQUEST),
    };
    let mut connector = HttpConnector::new();
    connector.enforce_http(true);
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);
    let Ok(upstream_request) = Request::builder()
        .method(request.method())
        .uri(uri)
        .header(AUTHORIZATION, basic_header(&state.upstream_password))
        .header(DIRECTORY_HEADER, state.cwd.to_string_lossy().as_ref())
        .body(Full::new(Bytes::new()))
    else {
        return refuse(StatusCode::BAD_REQUEST);
    };
    let Ok(upstream) = client.request(upstream_request).await else {
        return refuse(StatusCode::BAD_GATEWAY);
    };
    if matches!(route, Route::Events | Route::GlobalEvents) && request.method() == Method::GET {
        return filtered_events(upstream, state, route == Route::GlobalEvents).await;
    }
    if matches!(route, Route::Provider | Route::Providers) {
        return sanitized_provider_response(upstream).await;
    }
    if route == Route::Agents {
        return sanitized_agent_response(upstream).await;
    }
    bounded_response(upstream).await
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Route {
    Session,
    Messages,
    Events,
    GlobalEvents,
    Providers,
    Provider,
    Agents,
    SessionList,
    Config,
    Path,
    Project,
    ProjectDirectories,
    Command,
    Lsp,
    Mcp,
    Resource,
    Formatter,
    SessionStatus,
    ProviderAuth,
    Vcs,
    Capabilities,
    Console,
    ApiLocation,
    ApiData,
}
fn scoped_route(request: &Request<Incoming>, session_id: &str, cwd: &Path) -> Option<Route> {
    let session = format!("/session/{session_id}");
    match request.uri().path() {
        "/session" if valid_session_list_query(request.uri().query(), cwd) => {
            Some(Route::SessionList)
        }
        path if path == session => {
            same_directory(request.uri().query(), cwd).then_some(Route::Session)
        }
        path if path == format!("{session}/message") => {
            valid_message_query(request.uri().query(), cwd).then_some(Route::Messages)
        }
        "/config/providers" if same_directory(request.uri().query(), cwd) => Some(Route::Providers),
        "/config" if same_directory(request.uri().query(), cwd) => Some(Route::Config),
        "/provider" if same_directory(request.uri().query(), cwd) => Some(Route::Provider),
        "/path" if same_directory(request.uri().query(), cwd) => Some(Route::Path),
        "/project/current" if same_directory(request.uri().query(), cwd) => Some(Route::Project),
        "/project/lazoo-view/directories" if same_directory(request.uri().query(), cwd) => {
            Some(Route::ProjectDirectories)
        }
        "/command" if same_directory(request.uri().query(), cwd) => Some(Route::Command),
        "/lsp" if same_directory(request.uri().query(), cwd) => Some(Route::Lsp),
        "/mcp" if same_directory(request.uri().query(), cwd) => Some(Route::Mcp),
        "/experimental/resource" if same_directory(request.uri().query(), cwd) => {
            Some(Route::Resource)
        }
        "/formatter" if same_directory(request.uri().query(), cwd) => Some(Route::Formatter),
        "/session/status" if same_directory(request.uri().query(), cwd) => {
            Some(Route::SessionStatus)
        }
        "/provider/auth" if same_directory(request.uri().query(), cwd) => Some(Route::ProviderAuth),
        "/vcs" if same_directory(request.uri().query(), cwd) => Some(Route::Vcs),
        "/experimental/capabilities" if same_directory(request.uri().query(), cwd) => {
            Some(Route::Capabilities)
        }
        "/experimental/console" if same_directory(request.uri().query(), cwd) => {
            Some(Route::Console)
        }
        "/api/location" if same_api_location(request.uri().query(), cwd) => {
            Some(Route::ApiLocation)
        }
        "/api/agent" | "/api/integration" | "/api/model" | "/api/provider" | "/api/reference"
        | "/api/command" | "/api/skill"
            if same_api_location(request.uri().query(), cwd) =>
        {
            Some(Route::ApiData)
        }
        "/agent" if same_directory(request.uri().query(), cwd) => Some(Route::Agents),
        "/global/event" if same_directory(request.uri().query(), cwd) => Some(Route::GlobalEvents),
        "/event"
            if request
                .uri()
                .query()
                .is_none_or(|value| same_directory(Some(value), cwd)) =>
        {
            Some(Route::Events)
        }
        _ => None,
    }
}

fn local_metadata(route: Route, cwd: &Path) -> Option<serde_json::Value> {
    let directory = cwd.to_string_lossy().into_owned();
    match route {
        Route::Path => Some(serde_json::json!({
            "home": "",
            "state": "",
            "config": "",
            "worktree": directory.clone(),
            "directory": directory,
        })),
        Route::Project => Some(serde_json::json!({
            "id": "lazoo-view",
            "worktree": directory,
            "time": { "created": 0, "updated": 0 },
            "sandboxes": [],
        })),
        Route::ProjectDirectories => Some(serde_json::json!([{ "directory": directory }])),
        Route::Command | Route::Lsp | Route::Formatter | Route::SessionList => {
            Some(serde_json::json!([]))
        }
        Route::Mcp | Route::Resource | Route::SessionStatus | Route::ProviderAuth | Route::Vcs => {
            Some(serde_json::json!({}))
        }
        Route::Capabilities => Some(serde_json::json!({ "backgroundSubagents": false })),
        Route::Console => Some(serde_json::json!({
            "consoleManagedProviders": [],
            "switchableOrgCount": 0,
        })),
        Route::ApiLocation => Some(serde_json::json!({
            "directory": directory,
            "project": { "id": "lazoo-view", "directory": cwd.to_string_lossy() },
        })),
        Route::ApiData => Some(serde_json::json!({
            "location": {
                "directory": directory,
                "project": { "id": "lazoo-view", "directory": cwd.to_string_lossy() },
            },
            "data": [],
        })),
        _ => None,
    }
}

async fn bounded_response(response: Response<Incoming>) -> Response<Body> {
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_BODY)
    {
        return refuse(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let status = response.status();
    let content_type = response.headers().get(CONTENT_TYPE).cloned();
    let bytes = match bounded_collect(response.into_body()).await {
        Ok(bytes) => bytes,
        Err(status) => return refuse(status),
    };
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(CONTENT_TYPE, content_type);
    }
    builder
        .body(boxed(Full::new(bytes)))
        .expect("fixed response")
}
async fn sanitized_provider_response(response: Response<Incoming>) -> Response<Body> {
    let status = response.status();
    let bytes = match bounded_collect_with_limit(response.into_body(), MAX_METADATA_BODY).await {
        Ok(bytes) => bytes,
        Err(status) => return refuse(status),
    };
    if !status.is_success() {
        return Response::builder()
            .status(status)
            .body(boxed(Full::new(bytes)))
            .expect("fixed response");
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return refuse(StatusCode::BAD_GATEWAY);
    };
    let value = sanitize_provider_value(&value);
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(boxed(Full::new(Bytes::from(value.to_string()))))
        .expect("fixed response")
}
async fn sanitized_agent_response(response: Response<Incoming>) -> Response<Body> {
    let status = response.status();
    let bytes = match bounded_collect_with_limit(response.into_body(), MAX_METADATA_BODY).await {
        Ok(bytes) => bytes,
        Err(status) => return refuse(status),
    };
    if !status.is_success() {
        return Response::builder()
            .status(status)
            .body(boxed(Full::new(bytes)))
            .expect("fixed response");
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return refuse(StatusCode::BAD_GATEWAY);
    };
    let value = sanitize_agent_value(&value);
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(boxed(Full::new(Bytes::from(value.to_string()))))
        .expect("fixed response")
}

fn json_response(value: &serde_json::Value) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(boxed(Full::new(Bytes::from(value.to_string()))))
        .expect("fixed response")
}

fn sanitize_agent_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(sanitize_agent_value).collect())
        }
        serde_json::Value::Object(values) => {
            let mut out = serde_json::Map::new();
            for key in ["id", "name", "mode", "hidden", "native", "color", "model"] {
                if let Some(value) = values.get(key) {
                    out.insert(key.to_owned(), sanitize_agent_field(key, value));
                }
            }
            if out.is_empty() {
                // The endpoint is commonly a map keyed by agent id. Its values are
                // individually sanitized, rather than copying unknown agent fields.
                for (key, value) in values {
                    out.insert(key.clone(), sanitize_agent_value(value));
                }
            }
            serde_json::Value::Object(out)
        }
        // A scalar can only occur under a known allowed field or in an empty
        // endpoint response. It carries no nested configuration or credential data.
        _ => value.clone(),
    }
}

fn sanitize_agent_field(key: &str, value: &serde_json::Value) -> serde_json::Value {
    if key != "model" || !value.is_object() {
        return value.clone();
    }
    let Some(model) = value.as_object() else {
        return value.clone();
    };
    let mut safe = serde_json::Map::new();
    for key in ["providerID", "modelID", "provider", "model"] {
        if let Some(value) = model.get(key) {
            safe.insert(key.to_owned(), value.clone());
        }
    }
    serde_json::Value::Object(safe)
}
fn sanitize_provider_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(sanitize_provider_entry).collect())
        }
        serde_json::Value::Object(values) => {
            // `/provider` may be a map keyed by provider id. Keys are identifiers;
            // values are individually projected so a provider's arbitrary options do
            // not survive the bridge.
            serde_json::Value::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), sanitize_provider_entry(value)))
                    .collect(),
            )
        }
        _ => serde_json::Value::Null,
    }
}
fn sanitize_provider_entry(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(sanitize_provider_entry).collect())
        }
        serde_json::Value::Object(values) => {
            let mut out = serde_json::Map::new();
            for key in [
                "id",
                "name",
                "displayName",
                "models",
                "model",
                "providerID",
                "modelID",
                "contextWindow",
                "limit",
                "input",
                "output",
                "reasoning",
                "toolCall",
            ] {
                if let Some(value) = values.get(key) {
                    out.insert(
                        key.to_owned(),
                        if key == "models" {
                            sanitize_models(value)
                        } else {
                            value.clone()
                        },
                    );
                }
            }
            serde_json::Value::Object(out)
        }
        _ => serde_json::Value::Null,
    }
}
fn sanitize_models(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(sanitize_provider_entry).collect())
        }
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), sanitize_provider_entry(value)))
                .collect(),
        ),
        _ => serde_json::Value::Null,
    }
}
async fn bounded_collect(body: Incoming) -> Result<Bytes, StatusCode> {
    bounded_collect_with_limit(body, MAX_BODY).await
}
async fn bounded_collect_with_limit(
    mut body: Incoming,
    max_body: usize,
) -> Result<Bytes, StatusCode> {
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| StatusCode::BAD_GATEWAY)?;
        if let Ok(data) = frame.into_data() {
            if data.len() > max_body.saturating_sub(bytes.len()) {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            bytes.extend_from_slice(&data);
        }
    }
    Ok(Bytes::from(bytes))
}
async fn filtered_events(
    response: Response<Incoming>,
    state: Arc<State>,
    global_envelope: bool,
) -> Response<Body> {
    if !response.status().is_success() {
        return bounded_response(response).await;
    }
    let (sender, receiver) = mpsc::channel::<Result<Frame<Bytes>, Infallible>>(8);
    let mut body = response.into_body();
    let mut closed = state.closed_tx.subscribe();
    tokio::spawn(async move {
        let mut filter = SseFilter::new(
            state.session_id.clone(),
            global_envelope.then(|| state.cwd.to_string_lossy().into_owned()),
        );
        loop {
            tokio::select! {
                changed = closed.changed() => { let _ = changed; return; }
                frame = body.frame() => match frame { Some(Ok(frame)) => if let Ok(data) = frame.into_data() { for event in filter.push(&data) { if sender.send(Ok(Frame::data(event))).await.is_err() { return; } } }, Some(Err(_)) | None => return }
            }
        }
    });
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|frame| (frame, receiver))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .header("cache-control", "no-cache")
        .body(StreamBody::new(stream).boxed())
        .expect("fixed response")
}

struct SseFilter {
    session_id: String,
    global_directory: Option<String>,
    pending: Vec<u8>,
}
impl SseFilter {
    fn new(session_id: String, global_directory: Option<String>) -> Self {
        Self {
            session_id,
            global_directory,
            pending: Vec::new(),
        }
    }
    fn push(&mut self, chunk: &[u8]) -> Vec<Bytes> {
        if chunk.len() > MAX_SSE_EVENT.saturating_sub(self.pending.len()) {
            self.pending.clear();
            return Vec::new();
        }
        self.pending.extend_from_slice(chunk);
        let mut accepted = Vec::new();
        while let Some(end) = sse_record_end(&self.pending) {
            let record = self.pending.drain(..end).collect::<Vec<_>>();
            if event_for_session(&record, &self.session_id, self.global_directory.as_deref()) {
                accepted.push(Bytes::from(record));
            }
        }
        accepted
    }
}
fn sse_record_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .or_else(|| {
            bytes
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| index + 2)
        })
}
fn event_for_session(record: &[u8], session_id: &str, global_directory: Option<&str>) -> bool {
    let Ok(text) = std::str::from_utf8(record) else {
        return false;
    };
    let data = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>()
        .join("\n");
    let value: serde_json::Value = match serde_json::from_str(&data) {
        Ok(value) => value,
        Err(_) => return false,
    };
    match global_directory {
        Some(directory) => {
            value.get("directory").and_then(serde_json::Value::as_str) == Some(directory)
                && value
                    .get("payload")
                    .is_some_and(|payload| event_owned_by_session(payload, session_id))
        }
        None => event_owned_by_session(&value, session_id),
    }
}
/// Ownership comes only from the documented structured envelope. Unknown shapes fail closed:
/// foreign assistant/tool text may quote this session ID.
fn event_owned_by_session(event: &serde_json::Value, session_id: &str) -> bool {
    let Some(kind) = event.get("type").and_then(serde_json::Value::as_str) else {
        return false;
    };
    if kind == "server.connected" {
        return event
            .get("properties")
            .is_none_or(serde_json::Value::is_null)
            || event
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .is_some_and(serde_json::Map::is_empty);
    }
    let Some(properties) = event
        .get("properties")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    let owned = |value: Option<&serde_json::Value>| {
        value.and_then(serde_json::Value::as_str) == Some(session_id)
    };
    match kind {
        kind if kind.starts_with("session.") => {
            owned(properties.get("sessionID"))
                || owned(properties.get("info").and_then(|v| v.get("id")))
                || owned(properties.get("session").and_then(|v| v.get("id")))
        }
        kind if kind.starts_with("message.") || kind.starts_with("permission.") => {
            owned(properties.get("sessionID"))
                || owned(properties.get("info").and_then(|v| v.get("sessionID")))
                || owned(properties.get("message").and_then(|v| v.get("sessionID")))
                || owned(properties.get("part").and_then(|v| v.get("sessionID")))
                || owned(properties.get("request").and_then(|v| v.get("sessionID")))
        }
        _ => false,
    }
}
fn authorized(request: &Request<Incoming>, password: &str) -> bool {
    request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some(basic_header(password).as_str())
}
fn refuse(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(boxed(Full::new(Bytes::new())))
        .expect("fixed response")
}
fn boxed(body: Full<Bytes>) -> Body {
    body.map_err(|never| match never {}).boxed()
}
fn token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn basic_header(password: &str) -> String {
    format!("Basic {}", base64(&format!("opencode:{password}")))
}
fn base64(value: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = value.as_bytes();
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(char::from(TABLE[((value >> 18) & 63) as usize]));
        output.push(char::from(TABLE[((value >> 12) & 63) as usize]));
        output.push(if chunk.len() > 1 {
            char::from(TABLE[((value >> 6) & 63) as usize])
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            char::from(TABLE[(value & 63) as usize])
        } else {
            '='
        });
    }
    output
}
fn same_directory(query: Option<&str>, cwd: &Path) -> bool {
    let Some(query) = query else {
        return false;
    };
    let mut values = query.split('&');
    let Some(value) = values
        .next()
        .and_then(|value| value.strip_prefix("directory="))
        .and_then(decode)
    else {
        return false;
    };
    values.next().is_none() && value == cwd.to_string_lossy()
}

fn valid_message_query(query: Option<&str>, cwd: &Path) -> bool {
    let Some(query) = query else {
        return false;
    };
    let mut has_directory = false;
    for field in query.split('&') {
        let Some((key, value)) = field.split_once('=') else {
            return false;
        };
        match key {
            "directory" if !has_directory => {
                has_directory = decode(value).as_deref() == Some(cwd.to_string_lossy().as_ref());
                if !has_directory {
                    return false;
                }
            }
            "limit" => {}
            _ => return false,
        }
    }
    has_directory
}

fn valid_session_list_query(query: Option<&str>, cwd: &Path) -> bool {
    let Some(query) = query else {
        return false;
    };
    let mut has_directory = false;
    for field in query.split('&') {
        let Some((key, value)) = field.split_once('=') else {
            return false;
        };
        match key {
            "directory" if !has_directory => {
                has_directory = decode(value).as_deref() == Some(cwd.to_string_lossy().as_ref());
                if !has_directory {
                    return false;
                }
            }
            "start" | "end" | "limit" | "path" | "scope" => {}
            _ => return false,
        }
    }
    has_directory
}

fn same_api_location(query: Option<&str>, cwd: &Path) -> bool {
    let Some(query) = query else {
        return false;
    };
    let expected = cwd.to_string_lossy();
    let mut location = false;
    let mut directory = false;
    for field in query.split('&') {
        let Some((key, value)) = field.split_once('=') else {
            return false;
        };
        let Some(key) = decode(key) else {
            return false;
        };
        let Some(value) = decode(value) else {
            return false;
        };
        match key.as_str() {
            "location[directory]" if !location && value == expected => location = true,
            "directory" if !directory && value == expected => directory = true,
            _ => return false,
        }
    }
    location
}
fn decode(value: &str) -> Option<String> {
    let mut output = Vec::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            output.push(hex(bytes[index + 1])? << 4 | hex(bytes[index + 2])?);
            index += 3;
        } else {
            output.push(if bytes[index] == b'+' {
                b' '
            } else {
                bytes[index]
            });
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}
const fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[test]
    fn basic_auth_has_scheme_and_directory_scope_is_exact() {
        assert_eq!(basic_header("x"), "Basic b3BlbmNvZGU6eA==");
        let cwd = PathBuf::from(r"C:\work tree");
        assert!(same_directory(Some("directory=C%3A%5Cwork+tree"), &cwd));
        assert!(!same_directory(
            Some("directory=C%3A%5Cwork+tree&other=x"),
            &cwd
        ));
    }
    #[test]
    fn sse_forwards_only_complete_events_for_the_active_session() {
        let mut filter = SseFilter::new("ses_active".to_owned(), None);
        assert_eq!(
            filter
                .push(b"data: {\"type\":\"server.connected\",\"properties\":{}}\n\n")
                .len(),
            1
        );
        assert!(
            filter
                .push(b"event: message\ndata: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"sessionID\":\"ses_")
                .is_empty()
        );
        assert_eq!(filter.push(b"active\",\"text\":\"hello\"}}}\n\n").len(), 1);
        assert!(
            filter
                .push(b"data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"sessionID\":\"ses_other\",\"text\":\"ses_active secret\"}}}\n\n")
                .is_empty()
        );
        let mut global = SseFilter::new("ses_active".to_owned(), Some(r"C:\repo".to_owned()));
        assert_eq!(global.push(b"data: {\"directory\":\"C:\\\\repo\",\"payload\":{\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"sessionID\":\"ses_active\",\"text\":\"live\"}}}}\n\n").len(), 1);
        assert!(global.push(b"data: {\"directory\":\"C:\\\\repo\",\"payload\":{\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"sessionID\":\"ses_other\",\"text\":\"ses_active secret\"}}}}\n\n").is_empty());
    }
    #[test]
    fn native_view_scope_does_not_allow_other_sessions() {
        let cwd = PathBuf::from(r"C:\repo");
        assert!(same_directory(Some("directory=C%3A%5Crepo"), &cwd));
        assert!(!same_directory(Some("directory=C%3A%5Cother"), &cwd));
        assert!(same_api_location(
            Some("location[directory]=C%3A%5Crepo"),
            &cwd
        ));
        assert!(same_api_location(
            Some("directory=C%3A%5Crepo&location%5Bdirectory%5D=C%3A%5Crepo"),
            &cwd
        ));
        assert!(!same_api_location(
            Some("location[directory]=C%3A%5Crepo&workspace=other"),
            &cwd
        ));
        assert!(valid_session_list_query(
            Some("start=1&path=&directory=C%3A%5Crepo"),
            &cwd
        ));
        assert!(!valid_session_list_query(
            Some("directory=C%3A%5Crepo&directory=C%3A%5Crepo"),
            &cwd
        ));
        assert_eq!(sse_record_end(b"data: {}\n\n"), Some(10));
    }

    #[test]
    fn agent_metadata_drops_prompts_options_permissions_and_credentials() {
        let raw = serde_json::json!({
            "primary": {
                "id": "primary",
                "name": "Primary",
                "mode": "primary",
                "hidden": false,
                "native": true,
                "color": "blue",
                "model": { "providerID": "fixture", "modelID": "echo", "apiKey": "secret" },
                "prompt": "private instructions",
                "options": { "baseURL": "http://secret" },
                "permission": { "edit": "allow" }
            }
        });
        let safe = sanitize_agent_value(&raw);
        assert_eq!(safe["primary"]["name"], "Primary");
        assert_eq!(safe["primary"]["model"]["providerID"], "fixture");
        assert!(safe["primary"].get("prompt").is_none());
        assert!(safe["primary"].get("options").is_none());
        assert!(safe["primary"].get("permission").is_none());
        assert!(safe["primary"]["model"].get("apiKey").is_none());
    }

    #[test]
    fn provider_metadata_drops_options_and_credentials() {
        let raw = serde_json::json!({
            "fixture": {
                "id": "fixture",
                "name": "Fixture",
                "options": { "apiKey": "secret", "baseURL": "http://private" },
                "models": {
                    "echo": { "id": "echo", "name": "Echo", "apiKey": "secret" }
                }
            }
        });
        let safe = sanitize_provider_value(&raw);
        assert_eq!(safe["fixture"]["name"], "Fixture");
        assert_eq!(safe["fixture"]["models"]["echo"]["id"], "echo");
        assert!(safe["fixture"].get("options").is_none());
        assert!(safe["fixture"]["models"]["echo"].get("apiKey").is_none());
    }

    async fn raw_get(address: SocketAddr, request: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        String::from_utf8(response).unwrap()
    }

    #[tokio::test]
    async fn bridge_auth_scope_history_sse_and_mutations_are_enforced() {
        let upstream_listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_address = upstream_listener.local_addr().unwrap();
        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
        let upstream = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = upstream_listener.accept().await else {
                    return;
                };
                let seen_tx = seen_tx.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request: Request<Incoming>| {
                        let seen_tx = seen_tx.clone();
                        async move {
                            seen_tx
                                .send((
                                    request.uri().path().to_owned(),
                                    request.headers().get(AUTHORIZATION).cloned(),
                                ))
                                .unwrap();
                            let body = match request.uri().path() {
                                "/session/ses_active" => r#"{"id":"ses_active","status":"busy"}"#,
                                "/session/ses_active/message" => {
                                    r#"[{"sessionID":"ses_active","text":"history"}]"#
                                }
                                "/event" => {
                                    "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"sessionID\":\"ses_other\",\"text\":\"hidden\"}}}\n\ndata: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"sessionID\":\"ses_active\",\"text\":\"live\"}}}\n\n"
                                }
                                _ => "unexpected",
                            };
                            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(body))))
                        }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        let cwd = std::env::current_dir().unwrap();
        let lease = open(OpenCodeNativeViewConfig {
            executable: PathBuf::from("opencode"),
            cwd: cwd.clone(),
            session_id: "ses_active".to_owned(),
            upstream: upstream_address,
            upstream_password: "upstream-secret".to_owned(),
            launch_environment: lazoo_runtime::LaunchEnvironment::default(),
        })
        .await
        .unwrap();
        let descriptor = lease.launch_descriptor();
        let NativeViewLaunchDescriptor::Remote {
            remote_url,
            command,
        } = descriptor
        else {
            panic!("remote view");
        };
        let address: SocketAddr = remote_url.trim_start_matches("http://").parse().unwrap();
        let viewer_password = command
            .env
            .get(&std::ffi::OsString::from(TOKEN_ENV))
            .unwrap()
            .to_string_lossy();
        let basic = basic_header(&viewer_password);
        let directory = cwd
            .to_string_lossy()
            .replace('\\', "%5C")
            .replace(':', "%3A")
            .replace(' ', "+");
        let session = raw_get(address, &format!("GET /session/ses_active?directory={directory} HTTP/1.1\r\nHost: localhost\r\nAuthorization: {basic}\r\nConnection: close\r\n\r\n")).await;
        assert!(session.starts_with("HTTP/1.1 200"), "{session}");
        assert!(session.contains("busy"));
        let history = raw_get(address, &format!("GET /session/ses_active/message?directory={directory} HTTP/1.1\r\nHost: localhost\r\nAuthorization: {basic}\r\nConnection: close\r\n\r\n")).await;
        assert!(history.contains("history"));
        let events = raw_get(address, &format!("GET /event HTTP/1.1\r\nHost: localhost\r\nAuthorization: {basic}\r\nConnection: close\r\n\r\n")).await;
        assert!(events.contains("live"));
        assert!(!events.contains("hidden"));
        let denied = raw_get(address, &format!("POST /session/ses_active/message HTTP/1.1\r\nHost: localhost\r\nAuthorization: {basic}\r\nConnection: close\r\n\r\n")).await;
        assert!(denied.starts_with("HTTP/1.1 403"));
        let origin = raw_get(address, &format!("GET /session/ses_active?directory={directory} HTTP/1.1\r\nHost: localhost\r\nAuthorization: {basic}\r\nOrigin: http://evil.example\r\nConnection: close\r\n\r\n")).await;
        assert!(origin.starts_with("HTTP/1.1 403"));
        assert_eq!(seen_rx.recv().await.unwrap().0, "/session/ses_active");
        assert_eq!(
            seen_rx.recv().await.unwrap().0,
            "/session/ses_active/message"
        );
        assert_eq!(seen_rx.recv().await.unwrap().0, "/event");
        let closed = lease.closed_subscription();
        assert!(!closed.has_changed().unwrap());
        lease.close();
        assert!(closed.has_changed().unwrap());
        upstream.abort();
    }
}
