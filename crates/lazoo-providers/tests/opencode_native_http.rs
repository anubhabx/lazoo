//! Opt-in, zero-external-cost acceptance for the canonical `OpenCode` HTTP session.

use std::{
    env,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{
    Request, Response, body::Incoming, header::CONTENT_TYPE, server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use lazoo_providers::{
    AdapterFuture, NativeViewLaunchDescriptor, PermissionDecision, PermissionRequest,
    ProviderError, ProviderEvent, ProviderKind, ProviderRegistry, ProviderSession, SessionRequest,
    ToolCall, ToolResult, ToolSpec, TurnEventObserver, TurnRequest,
};
use lazoo_runtime::{TerminalCommand, TerminalSession};
use tempfile::tempdir;

const GATE: &str = "LAZOO_LIVE_OPENCODE_LOCAL_FIXTURE";
const MARKER: &str = "FIXTURE_ASSISTANT_RENDERED_MARKER";
static TURN: AtomicUsize = AtomicUsize::new(0);
static MCP_PHASE: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
struct PingObserver(Mutex<Vec<ToolCall>>);

impl TurnEventObserver for PingObserver {
    fn observe(&self, _event: &ProviderEvent) -> Result<(), ProviderError> {
        Ok(())
    }

    fn call_tool<'a>(&'a self, call: &'a ToolCall) -> AdapterFuture<'a, ToolResult> {
        Box::pin(async move {
            self.0.lock().unwrap().push(call.clone());
            if call.name == "lazoo_ping" && call.arguments == serde_json::json!({}) {
                Ok(ToolResult::success(serde_json::json!({"status":"pong"})))
            } else {
                Ok(ToolResult::failure(
                    "LZ-FIXTURE-TOOL-REJECTED",
                    "undeclared tool",
                ))
            }
        })
    }

    fn decide_permission<'a>(
        &'a self,
        request: &'a PermissionRequest,
    ) -> AdapterFuture<'a, PermissionDecision> {
        Box::pin(async move {
            Ok(if request.action == "lazoo_lazoo_ping" {
                PermissionDecision::Approve
            } else {
                PermissionDecision::Decline
            })
        })
    }
}

fn bounded_tool() -> ToolSpec {
    ToolSpec {
        name: "lazoo_ping".to_owned(),
        description: "Return the fixed local-fixture acceptance response.".to_owned(),
        input_schema: serde_json::json!({"type":"object","additionalProperties":false}),
    }
}

fn attach(descriptor: &NativeViewLaunchDescriptor) -> TerminalSession {
    let command = descriptor.command();
    let mut terminal =
        TerminalCommand::new(command.executable, command.cwd, 120, 40).args(command.args);
    for (key, value) in command.env {
        terminal = terminal.env(key, value);
    }
    TerminalSession::spawn(terminal).expect("native attach ConPTY")
}

async fn contains(terminal: &TerminalSession, marker: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut after = 0;
    let mut seen = String::new();
    while Instant::now() < deadline {
        let output = terminal.read_output(after, 64 * 1024).expect("TUI output");
        after = output.end;
        seen.push_str(&String::from_utf8_lossy(&output.bytes));
        if seen.contains(marker) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let tail = seen
        .chars()
        .rev()
        .take(2_000)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    println!("tui marker missing marker={marker} tail={tail:?}");
    false
}

async fn fixture() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let service = service_fn(|request: Request<Incoming>| async move {
                    let method = request.method().to_string();
                    let path = request.uri().path().to_owned();
                    let request_body = request.into_body().collect().await.unwrap().to_bytes();
                    let request_json =
                        serde_json::from_slice::<serde_json::Value>(&request_body).ok();
                    let streaming = request_json
                        .as_ref()
                        .and_then(|value| value.get("stream").and_then(serde_json::Value::as_bool))
                        .unwrap_or(false);
                    let tools = request_json
                        .as_ref()
                        .and_then(|value| value.get("tools"))
                        .and_then(serde_json::Value::as_array)
                        .map(|tools| {
                            tools
                                .iter()
                                .filter_map(|tool| {
                                    tool.pointer("/function/name")
                                        .and_then(serde_json::Value::as_str)
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let requests_ping = request_json
                        .as_ref()
                        .is_some_and(|value| value.to_string().contains("REQUEST_LAZOO_PING"));
                    println!(
                        "fixture request method={method} path={path} stream={streaming} bytes={} tools={tools:?}",
                        request_body.len(),
                    );
                    let marker = format!("{MARKER}_{}", TURN.fetch_add(1, Ordering::SeqCst) + 1);
                    if streaming {
                        if requests_ping && MCP_PHASE.fetch_add(1, Ordering::SeqCst) == 0 {
                            let name = tools
                                .iter()
                                .find(|name| name.contains("lazoo_ping"))
                                .unwrap();
                            let body = format!(
                                "data: {{\"id\":\"fixture\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"index\":0,\"delta\":{{\"tool_calls\":[{{\"index\":0,\"id\":\"call_ping\",\"type\":\"function\",\"function\":{{\"name\":\"{name}\",\"arguments\":\"{{}}\"}}}}]}},\"finish_reason\":null}}]}}\n\ndata: {{\"id\":\"fixture\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\ndata: [DONE]\n\n"
                            );
                            return Ok::<_, std::convert::Infallible>(
                                Response::builder()
                                    .header(CONTENT_TYPE, "text/event-stream")
                                    .body(Full::new(Bytes::from(body)))
                                    .unwrap(),
                            );
                        }
                        let body = format!(
                            "data: {{\"id\":\"fixture\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":\"{marker}\"}},\"finish_reason\":null}}]}}\n\ndata: {{\"id\":\"fixture\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
                        );
                        Ok::<_, std::convert::Infallible>(
                            Response::builder()
                                .header(CONTENT_TYPE, "text/event-stream")
                                .body(Full::new(Bytes::from(body)))
                                .unwrap(),
                        )
                    } else {
                        let body = serde_json::json!({"id":"fixture","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":marker},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}).to_string();
                        Ok(Response::new(Full::new(Bytes::from(body))))
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    address
}

#[tokio::test]
#[ignore = "set LAZOO_LIVE_OPENCODE_LOCAL_FIXTURE=1; real local serve/attach, no external model"]
async fn local_fixture_renders_canonical_history_after_reopen() {
    if env::var_os(GATE).is_none() {
        return;
    }
    TURN.store(0, Ordering::SeqCst);
    MCP_PHASE.store(0, Ordering::SeqCst);
    println!("phase=fixture-start");
    let address = fixture().await;
    let workspace = tempdir().unwrap();
    let config = serde_json::json!({"provider":{"fixture":{"npm":"@ai-sdk/openai-compatible","name":"fixture","options":{"baseURL":format!("http://{address}/v1"),"apiKey":"fixture"},"models":{"echo":{"name":"echo"}}}}});
    std::fs::write(workspace.path().join("opencode.json"), config.to_string()).unwrap();
    let mut request = SessionRequest::new(workspace.path())
        .with_model("fixture/echo")
        .with_tools(vec![bounded_tool()]);
    request.turn_timeout_millis = 30_000;
    request.progress_timeout_millis = 10_000;
    println!("phase=registry-start fixture={address}");
    let session = ProviderRegistry::default()
        .start(ProviderKind::OpenCode, &request)
        .await
        .unwrap();
    println!("phase=session-started id={}", session.id());
    let view = session.open_native_view().await.unwrap().unwrap();
    println!("phase=view-created");
    let first = attach(&view.launch_descriptor());
    println!("phase=tui-spawned pid={}", first.pid());
    assert!(
        contains(&first, "OC | Lazoo session").await,
        "native TUI did not finish bootstrap before the live turn"
    );
    println!("phase=tui-ready");
    println!("phase=first-turn-sent");
    let events = session
        .turn(&TurnRequest::new(
            "Answer briefly without using the fixture marker text.",
        ))
        .await
        .unwrap();
    assert!(events.iter().any(
        |event| matches!(event, ProviderEvent::TextDelta { text, .. } if text.contains(&format!("{MARKER}_1")))
    ));
    assert!(
        contains(&first, &format!("{MARKER}_1")).await,
        "real native TUI did not render fixture assistant content"
    );
    view.close();
    let _ = first.stop();
    assert!(
        session.native_view_available(),
        "view close stopped canonical serve host"
    );
    let second_events = session
        .turn(&TurnRequest::new(
            "Answer another brief response without the marker text.",
        ))
        .await
        .unwrap();
    assert!(second_events.iter().any(
        |event| matches!(event, ProviderEvent::TextDelta { text, .. } if text.contains(&format!("{MARKER}_2")))
    ));
    let observer = PingObserver::default();
    session
        .turn_observed(
            &TurnRequest::new("REQUEST_LAZOO_PING: call the declared ping function exactly once."),
            &observer,
        )
        .await
        .unwrap();
    let calls = observer.0.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "lazoo_ping");
    assert_eq!(calls[0].arguments, serde_json::json!({}));
    let reopened = session.open_native_view().await.unwrap().unwrap();
    let second = attach(&reopened.launch_descriptor());
    assert!(
        contains(&second, &format!("{MARKER}_1")).await,
        "reopened TUI did not render canonical history"
    );
    reopened.close();
    let _ = second.stop();
    let session_id = session.id();
    session.stop().await.unwrap();

    println!("phase=resume-start id={session_id}");
    let resumed = ProviderRegistry::default()
        .start(
            ProviderKind::OpenCode,
            &SessionRequest::new(workspace.path()).resume(&session_id),
        )
        .await
        .unwrap();
    assert_eq!(resumed.id(), session_id);
    let resumed_view = resumed.open_native_view().await.unwrap().unwrap();
    let resumed_terminal = attach(&resumed_view.launch_descriptor());
    assert!(
        contains(&resumed_terminal, &format!("{MARKER}_1")).await,
        "zero-model resumed TUI did not render canonical history"
    );
    resumed_view.close();
    let _ = resumed_terminal.stop();
    resumed.stop().await.unwrap();
}
