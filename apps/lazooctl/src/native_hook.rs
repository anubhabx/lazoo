//! Private native hook callback. This is not a general HTTP or daemon RPC client.
//!
//! The provider inherits its short-lived endpoint and bearer from the owned child
//! environment. Neither appears in arguments, diagnostics or terminal records.

use std::{io::Read, net::SocketAddr, time::Duration};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Uri, client::conn::http1, header};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::{net::TcpStream, task::JoinHandle, time::timeout};

const MAX_BYTES: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const INPUT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CALLBACK_TIMEOUT: Duration = Duration::from_secs(600);

pub(crate) async fn run_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("__native-hook")) {
        return None;
    }
    let provider = args.next().and_then(|arg| arg.into_string().ok());
    let event = args.next().and_then(|arg| arg.into_string().ok());
    let provider = provider.as_deref().unwrap_or("");
    let event = event.as_deref().unwrap_or("");
    let result = if args.next().is_none() && valid_invocation(provider, event) {
        invoke(provider, event).await
    } else {
        Err(())
    };
    if let Ok(value) = result {
        println!("{value}");
        Some(0)
    } else {
        // Native hooks must fail closed even when the provider ignores stderr.
        println!("{}", refusal(provider, event));
        eprintln!("LZ-NATIVE-HOOK-UNAVAILABLE");
        Some(2)
    }
}

fn valid_invocation(provider: &str, event: &str) -> bool {
    matches!(provider, "claude-code" | "antigravity")
        && matches!(
            event,
            "SessionStart"
                | "UserPromptSubmit"
                | "PreToolUse"
                | "PostToolUse"
                | "PostToolUseFailure"
                | "PermissionRequest"
                | "MessageDisplay"
                | "PreInvocation"
                | "PostInvocation"
                | "Stop"
                | "StopFailure"
                | "SessionEnd"
        )
}

async fn invoke(provider: &str, event: &str) -> Result<Value, ()> {
    let url = std::env::var("LAZOO_NATIVE_HOOK_URL").map_err(|_| ())?;
    let token = std::env::var("LAZOO_NATIVE_HOOK_TOKEN").map_err(|_| ())?;
    let input = timeout(
        INPUT_TIMEOUT,
        tokio::task::spawn_blocking(|| read_input(std::io::stdin().lock())),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())??;
    // `exit` in main terminates any blocked stdin worker on timeout as well.
    post(&url, &token, provider, event, input, MAX_CALLBACK_TIMEOUT).await
}

fn read_input(input: impl Read) -> Result<Value, ()> {
    let mut bytes = Vec::new();
    input
        .take(u64::try_from(MAX_BYTES + 1).map_err(|_| ())?)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > MAX_BYTES {
        return Err(());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    value.is_object().then_some(value).ok_or(())
}

fn endpoint(url: &str) -> Result<SocketAddr, ()> {
    let uri: Uri = url.parse().map_err(|_| ())?;
    let port = uri.port_u16().filter(|port| *port != 0).ok_or(())?;
    if uri.scheme_str() != Some("http")
        || uri.host() != Some("127.0.0.1")
        || uri.path() != "/hook"
        || uri.query().is_some()
        || uri.authority().map(hyper::http::uri::Authority::as_str)
            != Some(format!("127.0.0.1:{port}").as_str())
    {
        return Err(());
    }
    Ok(SocketAddr::from(([127, 0, 0, 1], port)))
}

struct ConnectionTask(JoinHandle<()>);
impl Drop for ConnectionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn post(
    url: &str,
    token: &str,
    provider: &str,
    event: &str,
    payload: Value,
    wait: Duration,
) -> Result<Value, ()> {
    let address = endpoint(url)?;
    if !(32..=256).contains(&token.len()) || !token.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(());
    }
    let request_id = uuid::Uuid::new_v4().to_string();
    let body = serde_json::to_vec(&json!({"requestId":request_id,"event":event,"payload":payload}))
        .map_err(|_| ())?;
    if body.len() > MAX_BYTES {
        return Err(());
    }
    let stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(address))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    timeout(wait.min(MAX_CALLBACK_TIMEOUT), async {
        let (mut sender, connection) = http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|_| ())?;
        let _connection = ConnectionTask(tokio::spawn(async move {
            let _ = connection.await;
        }));
        let request = Request::post("/hook")
            .header(header::HOST, address.to_string())
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONNECTION, "close")
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| ())?;
        let response = sender.send_request(request).await.map_err(|_| ())?;
        if response.status() != hyper::StatusCode::OK {
            return Err(());
        }
        let mut response = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = response.frame().await {
            let frame = frame.map_err(|_| ())?;
            if let Some(data) = frame.data_ref() {
                if bytes.len().saturating_add(data.len()) > MAX_BYTES {
                    return Err(());
                }
                bytes.extend_from_slice(data);
            }
        }
        let reply: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
        if reply.get("requestId").and_then(Value::as_str) != Some(request_id.as_str())
            || reply.get("event").and_then(Value::as_str) != Some(event)
        {
            return Err(());
        }
        let result = reply
            .get("result")
            .filter(|result| valid_reply(provider, event, result))
            .ok_or(())?;
        Ok(result.clone())
    })
    .await
    .map_err(|_| ())?
}

fn valid_reply(provider: &str, event: &str, result: &Value) -> bool {
    let Some(object) = result.as_object() else {
        return false;
    };
    if provider == "antigravity" {
        return if event == "PreToolUse" {
            matches!(
                result.get("decision").and_then(Value::as_str),
                Some("auto_approve" | "deny")
            ) && object
                .keys()
                .all(|key| matches!(key.as_str(), "decision" | "reason"))
        } else {
            object.is_empty()
                || (result.get("decision").and_then(Value::as_str) == Some("allow")
                    && object.len() == 1)
        };
    }
    match event {
        "PreToolUse" | "PermissionRequest" => {
            if object.len() != 1 {
                return false;
            }
            let Some(hook) = result.get("hookSpecificOutput").and_then(Value::as_object) else {
                return false;
            };
            if hook.get("hookEventName").and_then(Value::as_str) != Some(event) {
                return false;
            }
            if event == "PreToolUse" {
                matches!(
                    hook.get("permissionDecision").and_then(Value::as_str),
                    Some("allow" | "deny")
                ) && hook.keys().all(|key| {
                    matches!(
                        key.as_str(),
                        "hookEventName" | "permissionDecision" | "permissionDecisionReason"
                    )
                })
            } else {
                let Some(decision) = hook.get("decision").and_then(Value::as_object) else {
                    return false;
                };
                matches!(
                    decision.get("behavior").and_then(Value::as_str),
                    Some("allow" | "deny")
                ) && hook
                    .keys()
                    .all(|key| matches!(key.as_str(), "hookEventName" | "decision"))
                    && decision
                        .keys()
                        .all(|key| matches!(key.as_str(), "behavior" | "message" | "interrupt"))
            }
        }
        _ => {
            object.is_empty()
                || (result.get("decision").and_then(Value::as_str) == Some("block")
                    && object
                        .keys()
                        .all(|key| matches!(key.as_str(), "decision" | "reason")))
        }
    }
}

fn refusal(provider: &str, event: &str) -> Value {
    let reason = "Lazoo could not authorize this native hook";
    if provider == "antigravity" {
        return json!({"decision":"deny","reason":reason});
    }
    match event {
        "PreToolUse" => json!({"hookSpecificOutput":{
            "hookEventName":"PreToolUse","permissionDecision":"deny",
            "permissionDecisionReason":reason
        }}),
        "PermissionRequest" => json!({"hookSpecificOutput":{
            "hookEventName":"PermissionRequest",
            "decision":{"behavior":"deny","message":reason,"interrupt":true}
        }}),
        _ => json!({"decision":"block","reason":reason}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::{Response, server, service::service_fn};
    use std::convert::Infallible;
    use tokio::{io::AsyncReadExt, net::TcpListener};

    #[test]
    fn callback_is_not_an_arbitrary_network_or_input_client() {
        for value in [
            "https://127.0.0.1:123/hook",
            "http://localhost:123/hook",
            "http://127.0.0.2:123/hook",
            "http://127.0.0.1:123/other",
            "http://127.0.0.1:123/hook?q=1",
            "http://user@127.0.0.1:123/hook",
        ] {
            assert!(endpoint(value).is_err(), "{value}");
        }
        assert!(read_input(&b"[]"[..]).is_err());
        assert!(read_input(std::io::repeat(b' ').take((MAX_BYTES + 1) as u64)).is_err());
        assert!(!valid_invocation("grok-build", "PreToolUse"));
        assert!(!valid_invocation("claude-code", "unknown"));
        assert_eq!(refusal("antigravity", "PreToolUse")["decision"], "deny");
    }

    #[tokio::test]
    async fn callback_delivers_bounded_native_envelope_and_exact_decision() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            server::conn::http1::Builder::new()
                .serve_connection(
                    TokioIo::new(stream),
                    service_fn(|request: Request<hyper::body::Incoming>| async move {
                        assert_eq!(request.uri().path(), "/hook");
                        assert_eq!(
                            request.headers()[header::AUTHORIZATION],
                            format!("Bearer {}", "a".repeat(64))
                        );
                        let body = request.into_body().collect().await.unwrap().to_bytes();
                        let body: Value = serde_json::from_slice(&body).unwrap();
                        assert_eq!(body["event"], "PreToolUse");
                        assert_eq!(body["payload"]["session_id"], "native-session");
                        Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(
                            json!({"requestId":body["requestId"],"event":"PreToolUse",
                                "result":{"decision":"deny","reason":"test"}})
                            .to_string(),
                        ))))
                    }),
                )
                .await
                .unwrap();
        });
        let response = post(
            &format!("http://{address}/hook"),
            &"a".repeat(64),
            "antigravity",
            "PreToolUse",
            json!({"session_id":"native-session"}),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(response, json!({"decision":"deny","reason":"test"}));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn callback_deadline_closes_connection_without_fabricating_permission() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            assert!(stream.read(&mut bytes).await.unwrap() > 0);
            assert_eq!(stream.read(&mut bytes).await.unwrap(), 0);
        });
        assert!(
            post(
                &format!("http://{address}/hook"),
                &"a".repeat(64),
                "claude-code",
                "Stop",
                json!({}),
                Duration::from_millis(100)
            )
            .await
            .is_err()
        );
        timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn empty_stale_cross_event_and_invalid_decision_responses_are_refused() {
        for variant in 0..5 {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), service_fn(move |request: Request<hyper::body::Incoming>| async move {
                    let body = request.into_body().collect().await.unwrap().to_bytes();
                    let body: Value = serde_json::from_slice(&body).unwrap();
                    let mut reply = json!({"requestId":body["requestId"],"event":"PreToolUse",
                        "result":{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny"}}});
                    match variant {
                        0 => reply = json!({}),
                        1 => reply["requestId"] = json!("stale"),
                        2 => reply["event"] = json!("SessionStart"),
                        3 => reply["result"] = json!({}),
                        _ => reply["result"]["hookSpecificOutput"]["permissionDecision"] = json!("ask"),
                    }
                    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(reply.to_string()))))
                })).await.unwrap();
            });
            assert!(
                post(
                    &format!("http://{address}/hook"),
                    &"a".repeat(64),
                    "claude-code",
                    "PreToolUse",
                    json!({}),
                    Duration::from_secs(2)
                )
                .await
                .is_err(),
                "variant {variant}"
            );
            server.await.unwrap();
        }
    }
}
