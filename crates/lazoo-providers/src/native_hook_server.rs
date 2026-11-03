//! Authenticated, session-scoped callback endpoint for native interactive hooks.
//!
//! The endpoint deliberately carries only short-lived hook envelopes.  It does not turn terminal
//! output into control evidence: the interactive adapter consumes these structured events while a
//! turn is active and asks the existing observer for each individual approval.

use std::{
    fmt::Write as _,
    sync::{Arc, Mutex},
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Method, Request, Response, StatusCode,
    body::Incoming,
    header::{AUTHORIZATION, CONTENT_TYPE, ORIGIN},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, mpsc, oneshot},
    time::timeout,
};
use uuid::Uuid;

use crate::{PermissionDecision, ProviderError, ProviderKind};

const MAX_BODY: usize = 64 * 1024;
const MAX_QUEUE: usize = 16;
const BODY_TIMEOUT: Duration = Duration::from_secs(10);
const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: usize = 8;

#[derive(Clone, Debug)]
pub(crate) struct NativeHookEvent {
    pub event: String,
    pub payload: Value,
}

pub(crate) struct QueuedHook {
    pub(crate) request_id: String,
    pub(crate) event: NativeHookEvent,
    pub(crate) reply: oneshot::Sender<Value>,
}
struct State {
    closed: bool,
    sender: mpsc::Sender<QueuedHook>,
    quota: Option<crate::ProviderEvent>,
}

/// Private loopback callback server.  The opaque bearer and queue are unique per native session.
pub(crate) struct NativeHookServer {
    endpoint: String,
    token: String,
    state: Arc<Mutex<State>>,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl NativeHookServer {
    pub(crate) async fn open(
        _provider: ProviderKind,
    ) -> Result<(Self, mpsc::Receiver<QueuedHook>), ProviderError> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let token = token();
        let (sender, receiver) = mpsc::channel(MAX_QUEUE);
        let state = Arc::new(Mutex::new(State {
            closed: false,
            sender,
            quota: None,
        }));
        let (shutdown, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let connection_shutdown = shutdown.clone();
        let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let accepting_state = Arc::clone(&state);
        let accepting_auth = token.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = shutdown_rx.changed() => if changed.is_err() || *shutdown_rx.borrow() { return; },
                    accepted = listener.accept() => match accepted {
                        Ok((stream, _)) => {
                            let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else { continue; };
                            let state = Arc::clone(&accepting_state);
                            let auth = accepting_auth.clone();
                            let mut closed = connection_shutdown.subscribe();
                            tokio::spawn(async move {
                                let mut builder = http1::Builder::new();
                                builder.timer(TokioTimer::new()).header_read_timeout(HEADER_TIMEOUT).max_buf_size(64 * 1024);
                                let connection = builder.serve_connection(TokioIo::new(stream), service_fn(move |request| {
                                    let state = Arc::clone(&state);
                                    let auth = auth.clone();
                                    async move { handle(state, &auth, request).await }
                                }));
                                tokio::select! { _ = closed.changed() => {}, _ = connection => {} }
                                drop(permit);
                            });
                        }
                        Err(_) => return,
                    }
                }
            }
        });
        Ok((
            Self {
                endpoint: format!("http://{address}/hook"),
                token,
                state,
                shutdown,
            },
            receiver,
        ))
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub(crate) fn token(&self) -> &str {
        &self.token
    }
    pub(crate) fn take_quota(&self, session_id: &str) -> Option<crate::ProviderEvent> {
        let quota = self.state.lock().ok()?.quota.take()?;
        matches!(&quota, crate::ProviderEvent::Quota { session_id: id, .. } if id == session_id)
            .then_some(quota)
    }
    pub(crate) fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
        }
        let _ = self.shutdown.send(true);
    }
}

impl Drop for NativeHookServer {
    fn drop(&mut self) {
        self.close();
    }
}

#[allow(clippy::too_many_lines)] // Keep callback authentication and admission in one auditable path.
async fn handle(
    state: Arc<Mutex<State>>,
    token: &str,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let refuse = |status, code: &str| response(status, &json!({"error": code}));
    if request.method() != Method::POST || request.uri().path() != "/hook" {
        return Ok(refuse(StatusCode::METHOD_NOT_ALLOWED, "LZ-HOOK-ROUTE"));
    }
    if request.headers().contains_key(ORIGIN) {
        return Ok(refuse(StatusCode::FORBIDDEN, "LZ-HOOK-ORIGIN"));
    }
    if request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        != Some(&format!("Bearer {token}"))
    {
        return Ok(refuse(StatusCode::UNAUTHORIZED, "LZ-HOOK-AUTH"));
    }
    if request
        .headers()
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|n| n > MAX_BODY)
    {
        return Ok(refuse(StatusCode::PAYLOAD_TOO_LARGE, "LZ-HOOK-BOUND"));
    }
    let bytes = match timeout(
        BODY_TIMEOUT,
        Limited::new(request.into_body(), MAX_BODY + 1).collect(),
    )
    .await
    {
        Ok(Ok(body)) => body.to_bytes(),
        _ => return Ok(refuse(StatusCode::BAD_REQUEST, "LZ-HOOK-BODY")),
    };
    if bytes.len() > MAX_BODY {
        return Ok(refuse(StatusCode::PAYLOAD_TOO_LARGE, "LZ-HOOK-BOUND"));
    }
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return Ok(refuse(StatusCode::BAD_REQUEST, "LZ-HOOK-JSON")),
    };
    let Some(request_id) = value
        .get("requestId")
        .and_then(Value::as_str)
        .filter(|v| v.len() == 36 && Uuid::parse_str(v).is_ok())
    else {
        return Ok(refuse(StatusCode::BAD_REQUEST, "LZ-HOOK-REQUEST"));
    };
    let Some(event) = value
        .get("event")
        .and_then(Value::as_str)
        .filter(|v| v.len() <= 64)
    else {
        return Ok(refuse(StatusCode::BAD_REQUEST, "LZ-HOOK-EVENT"));
    };
    let payload = value.get("payload").cloned().unwrap_or(Value::Null);
    // Statuslines are observations, never control decisions. Retain only the latest
    // normalized numeric snapshot and reply immediately even when the session is idle.
    if event == "StatusLine" {
        let Ok(mut state) = state.lock() else {
            return Ok(refuse(StatusCode::GONE, "LZ-HOOK-CLOSED"));
        };
        if state.closed {
            return Ok(refuse(StatusCode::GONE, "LZ-HOOK-CLOSED"));
        }
        if let Some(quota) = crate::quota::claude_statusline(&payload) {
            state.quota = Some(quota);
        }
        return Ok(response(
            StatusCode::OK,
            &json!({"requestId":request_id,"event":event,"result":{}}),
        ));
    }
    let (reply_tx, reply_rx) = oneshot::channel();
    let sender = match state.lock() {
        Ok(state) if !state.closed => state.sender.clone(),
        _ => return Ok(refuse(StatusCode::GONE, "LZ-HOOK-CLOSED")),
    };
    let request_id = request_id.to_owned();
    let event_name = event.to_owned();
    match sender.try_send(QueuedHook {
        request_id: request_id.clone(),
        event: NativeHookEvent {
            event: event_name.clone(),
            payload,
        },
        reply: reply_tx,
    }) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            return Ok(refuse(StatusCode::TOO_MANY_REQUESTS, "LZ-HOOK-QUEUE"));
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            return Ok(refuse(StatusCode::GONE, "LZ-HOOK-STALE"));
        }
    }
    match timeout(Duration::from_secs(600), reply_rx).await {
        Ok(Ok(value)) => Ok(response(
            StatusCode::OK,
            &json!({"requestId":request_id,"event":event_name,"result":value}),
        )),
        _ => Ok(response(
            StatusCode::GATEWAY_TIMEOUT,
            &json!({"error":"LZ-HOOK-TIMEOUT"}),
        )),
    }
}

pub(crate) fn native_decision(
    provider: ProviderKind,
    event: &str,
    decision: PermissionDecision,
) -> Value {
    let allowed = matches!(decision, PermissionDecision::Approve);
    match provider {
        ProviderKind::ClaudeCode => {
            if event == "PermissionRequest" {
                json!({"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":if allowed {"allow"} else {"deny"},"message":if allowed {"Lazoo approved this action"} else {"Lazoo denied this action"},"interrupt":!allowed}}})
            } else {
                json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":if allowed {"allow"} else {"deny"},"permissionDecisionReason":if allowed {"Lazoo approved this action"} else {"Lazoo denied this action"}}})
            }
        }
        ProviderKind::Antigravity => {
            json!({"decision":if allowed {"auto_approve"} else {"deny"},"reason":if allowed {"Lazoo approved this action"} else {"Lazoo denied this action"}})
        }
        _ => json!({"decision":"deny","reason":"unsupported native hook provider"}),
    }
}

fn response(status: StatusCode, value: &Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(value).expect("JSON value"),
        )))
        .expect("fixed response")
}
fn token() -> String {
    let mut hash = Sha256::new();
    hash.update(Uuid::new_v4().as_bytes());
    hash.update(Uuid::new_v4().as_bytes());
    hex(hash.finalize().as_slice())
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::native_decision;
    use super::*;
    use crate::{PermissionDecision, ProviderKind};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };

    #[test]
    fn native_decisions_are_provider_specific_and_fail_closed() {
        let claude = native_decision(
            ProviderKind::ClaudeCode,
            "PreToolUse",
            PermissionDecision::Decline,
        );
        assert_eq!(
            claude
                .pointer("/hookSpecificOutput/permissionDecision")
                .and_then(|v| v.as_str()),
            Some("deny")
        );
        let agy = native_decision(
            ProviderKind::Antigravity,
            "PreToolUse",
            PermissionDecision::Approve,
        );
        assert_eq!(
            agy.get("decision").and_then(|v| v.as_str()),
            Some("auto_approve")
        );
        let permission = native_decision(
            ProviderKind::ClaudeCode,
            "PermissionRequest",
            PermissionDecision::Decline,
        );
        assert_eq!(
            permission
                .pointer("/hookSpecificOutput/decision/behavior")
                .and_then(|v| v.as_str()),
            Some("deny")
        );
        assert!(
            permission
                .pointer("/hookSpecificOutput/permissionDecision")
                .is_none()
        );
    }

    #[tokio::test]
    async fn statusline_is_authenticated_redacted_and_never_waits_for_a_control_hook() {
        let (server, mut received) = NativeHookServer::open(ProviderKind::ClaudeCode)
            .await
            .unwrap();
        let target = server
            .endpoint()
            .trim_start_matches("http://")
            .trim_end_matches("/hook");
        let body = json!({"requestId":Uuid::new_v4(),"event":"StatusLine","payload":{"session_id":"owned","rate_limits":{"five_hour":{"used_percentage":81,"resets_at":2_000_000_000}},"apiKey":"secret-fixture-value"}}).to_string();
        for (token, status) in [("wrong", "HTTP/1.1 401"), (server.token(), "HTTP/1.1 200")] {
            let mut stream = TcpStream::connect(target).await.unwrap();
            let request = format!(
                "POST /hook HTTP/1.1\r\nHost: {target}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut reply = Vec::new();
            timeout(Duration::from_secs(2), stream.read_to_end(&mut reply))
                .await
                .unwrap()
                .unwrap();
            assert!(String::from_utf8_lossy(&reply).starts_with(status));
            if token == "wrong" {
                assert!(server.take_quota("owned").is_none());
            }
        }
        assert!(received.try_recv().is_err());
        let quota = server.take_quota("owned").unwrap();
        assert!(
            !serde_json::to_string(&quota)
                .unwrap()
                .contains("secret-fixture-value")
        );
        assert!(server.take_quota("owned").is_none());
        server.close();
    }

    #[tokio::test]
    async fn loopback_server_authenticates_envelopes_and_closes() {
        let (server, mut received) = NativeHookServer::open(ProviderKind::ClaudeCode)
            .await
            .unwrap();
        let target = server
            .endpoint()
            .trim_start_matches("http://")
            .trim_end_matches("/hook");
        let token = server.token().to_owned();
        let responder = tokio::spawn(async move {
            let hook = received.recv().await.unwrap();
            assert_eq!(hook.event.event, "SessionStart");
            hook.reply.send(json!({})).unwrap();
        });
        let body = json!({"requestId":Uuid::new_v4().to_string(),"event":"SessionStart","payload":{"session_id":"s"}}).to_string();
        let mut stream = TcpStream::connect(target).await.unwrap();
        let request = format!(
            "POST /hook HTTP/1.1\r\nHost: {target}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).await.unwrap();
        assert!(String::from_utf8_lossy(&reply).starts_with("HTTP/1.1 200"));
        responder.await.unwrap();
        server.close();
    }
}
