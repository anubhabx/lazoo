use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use lazoo_providers::{
    AdapterFuture, CodexAdapter, DiscoveryEnvironment, PermissionDecision, PermissionKind,
    PermissionRequest, ProviderAdapter, ProviderCapabilities, ProviderError, ProviderEvent,
    ProviderKind, ProviderSession, ProviderVerification, SessionRequest, ToolCall, ToolResult,
    ToolSpec, TurnEventObserver, TurnRequest, normalize_codex_message,
    normalize_codex_permission_request, normalize_codex_tool_call,
};
use serde_json::json;
use tempfile::tempdir;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::header::{AUTHORIZATION, ORIGIN},
    },
};

#[derive(Default)]
struct CollectingObserver {
    events: Mutex<Vec<ProviderEvent>>,
}

impl TurnEventObserver for CollectingObserver {
    fn observe(&self, event: &ProviderEvent) -> Result<(), ProviderError> {
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}

struct RejectingObserver;

impl TurnEventObserver for RejectingObserver {
    fn observe(&self, _event: &ProviderEvent) -> Result<(), ProviderError> {
        Err(ProviderError::Observer(
            "fixture persistence failure".to_owned(),
        ))
    }
}

fn fake_codex() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/fake-codex.cmd")
}

fn ensure_session_host() {
    static READY: OnceLock<()> = OnceLock::new();
    READY.get_or_init(|| {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .to_path_buf();
        let status = Command::new(env!("CARGO"))
            .current_dir(workspace)
            .args(["build", "-p", "lazoo-session-host"])
            .status()
            .unwrap();
        assert!(status.success(), "lazoo-session-host fixture build failed");
    });
}

#[test]
fn codex_launches_app_server_over_stdio() {
    let command = CodexAdapter::command(Path::new(r"C:\Tools\codex.exe"), Path::new(r"C:\repo"));

    assert_eq!(command.executable, PathBuf::from(r"C:\Tools\codex.exe"));
    assert_eq!(
        command.args,
        ["app-server", "--listen", "stdio://"]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(command.cwd, PathBuf::from(r"C:\repo"));
}

#[test]
fn codex_fallback_is_structured_json_and_never_bypasses_permissions() {
    let request = SessionRequest::new(r"C:\repo").with_model("gpt-5.6-sol");
    let command =
        CodexAdapter::fallback_command(Path::new(r"C:\Tools\codex.exe"), &request, "Reply OK");

    assert_eq!(
        command.args,
        [
            "exec",
            "--json",
            "--cd",
            r"C:\repo",
            "--model",
            "gpt-5.6-sol",
            "Reply OK",
        ]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>()
    );
    assert!(
        !command
            .args
            .iter()
            .any(|argument| { argument.to_string_lossy().contains("dangerously-bypass") })
    );
}

#[test]
fn codex_private_key_uses_a_process_local_provider_without_putting_the_key_in_argv() {
    let mut request = SessionRequest::new(r"C:\repo");
    request
        .launch_environment
        .set("OPENAI_API_KEY", "secret-account-key");
    request
        .launch_environment
        .set("OPENAI_BASE_URL", "https://gateway.example/v1");
    let command =
        CodexAdapter::fallback_command(Path::new(r"C:\Tools\codex.exe"), &request, "Reply OK");
    let args = command
        .args
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(args.contains(&r#"model_provider="lazoo-account""#.to_owned()));
    assert!(args.contains(&r#"model_providers.lazoo-account.name="openai""#.to_owned()));
    assert!(args.contains(&r#"model_providers.lazoo-account.env_key="OPENAI_API_KEY""#.to_owned()));
    assert!(args.contains(
        &r#"model_providers.lazoo-account.base_url="https://gateway.example/v1""#.to_owned()
    ));
    assert!(
        !args
            .iter()
            .any(|argument| argument.contains("secret-account-key"))
    );
    assert_eq!(
        command.env[&OsString::from("OPENAI_API_KEY")],
        OsString::from("secret-account-key")
    );
}

#[test]
fn codex_declares_only_capabilities_backed_by_app_server() {
    assert_eq!(
        CodexAdapter::capabilities(),
        ProviderCapabilities {
            session: true,
            stream: true,
            cancel: true,
            resume: true,
            terminal: true,
            permission: true,
            mcp: true,
            plugins: true,
            skills: true,
            connectors: true,
            native_message: false,
            model_selection: true,
        }
    );
}

#[test]
fn codex_notifications_normalize_turn_items_and_completion() {
    let events = include_str!("fixtures/codex/turn.jsonl")
        .lines()
        .map(|line| {
            normalize_codex_message(&serde_json::from_str(line).unwrap())
                .unwrap()
                .unwrap()
        })
        .collect::<Vec<_>>();

    assert!(matches!(
        events[0],
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            ref session_id,
            ref turn_id,
            ..
        } if session_id == "thread-1" && turn_id.as_deref() == Some("turn-1")
    ));
    assert!(matches!(
        events[1],
        ProviderEvent::TextDelta { ref text, .. } if text == "Hello "
    ));
    assert!(matches!(
        events[3],
        ProviderEvent::ToolUpdate {
            ref tool_name,
            ref status,
            ..
        } if tool_name.as_deref() == Some("commandExecution") && status == "inProgress"
    ));
    assert!(matches!(
        events[5],
        ProviderEvent::TurnCompleted { ref status, .. } if status == "completed"
    ));
}

#[test]
fn codex_interrupt_correlates_the_active_turn() {
    assert_eq!(
        CodexAdapter::interrupt_params("thread-1", "turn-1"),
        json!({"threadId": "thread-1", "turnId": "turn-1"}),
    );
}

#[tokio::test]
async fn observed_codex_turn_emits_response_acceptance_before_stream_without_duplicate() {
    ensure_session_host();
    let temporary = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    let collector = CollectingObserver::default();

    let returned = session
        .turn_observed(&TurnRequest::new("hello"), &collector)
        .await
        .unwrap();
    let captured = collector.events.lock().unwrap().clone();

    assert_eq!(captured, returned);
    assert!(matches!(
        captured[0],
        ProviderEvent::TurnStarted {
            ref session_id,
            ref turn_id,
            raw: None,
            ..
        } if session_id == "thread-1" && turn_id.as_deref() == Some("turn-1")
    ));
    assert!(
        matches!(&captured[1], ProviderEvent::Quota { windows, raw: None, .. }
        if windows.len() == 1 && windows[0].used_percent == Some(25.0)
            && windows[0].source == "codex-app-server")
    );
    assert!(matches!(captured[2], ProviderEvent::TextDelta { .. }));
    assert_eq!(
        captured
            .iter()
            .filter(|event| matches!(event, ProviderEvent::TurnStarted { .. }))
            .count(),
        1
    );
    session.stop().await.unwrap();
}

#[tokio::test]
async fn codex_turn_permission_override_narrows_the_session_default() {
    ensure_session_host();
    let temporary = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(temporary.path()).with_permission_mode("auto"))
        .await
        .unwrap();

    session
        .turn(&TurnRequest::new("hello").with_permission_mode("ask"))
        .await
        .unwrap();
    session.stop().await.unwrap();

    let request_text =
        fs::read_to_string(temporary.path().join("fake-codex-turn-start.json")).unwrap();
    let request: serde_json::Value =
        serde_json::from_str(request_text.trim_start_matches('\u{feff}')).unwrap();
    assert_eq!(request["approvalPolicy"], "on-request");
}

#[tokio::test]
async fn codex_read_only_turn_uses_native_read_only_no_network_policy() {
    ensure_session_host();
    let temporary = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    session
        .turn(&TurnRequest::new("inspect").read_only())
        .await
        .unwrap();
    session.stop().await.unwrap();
    let request_text =
        fs::read_to_string(temporary.path().join("fake-codex-turn-start.json")).unwrap();
    let request: serde_json::Value =
        serde_json::from_str(request_text.trim_start_matches('\u{feff}')).unwrap();
    assert_eq!(request["approvalPolicy"], "untrusted");
    assert_eq!(
        request["sandboxPolicy"],
        serde_json::json!({"type":"readOnly","networkAccess":false})
    );
}

#[tokio::test]
async fn observed_codex_rejection_interrupts_the_accepted_native_turn() {
    ensure_session_host();
    let temporary = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();

    let error = session
        .turn_observed(&TurnRequest::new("hello"), &RejectingObserver)
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProviderError::Observer(message) if message == "fixture persistence failure")
    );
    session.stop().await.unwrap();

    let interrupt_text =
        fs::read_to_string(temporary.path().join("fake-codex-interrupt.json")).unwrap();
    let interrupt: serde_json::Value =
        serde_json::from_str(interrupt_text.trim_start_matches('\u{feff}')).unwrap();
    assert_eq!(
        interrupt,
        json!({"threadId": "thread-1", "turnId": "turn-1"})
    );
}

#[test]
fn codex_errors_keep_retryability_and_redact_raw_secrets() {
    let message = json!({
        "jsonrpc": "2.0",
        "method": "error",
        "params": {
            "threadId": "thread-1",
            "turnId": "turn-1",
            "willRetry": true,
            "error": {"message": "temporary failure", "token": "secret"}
        }
    });

    let event = normalize_codex_message(&message).unwrap().unwrap();
    let ProviderEvent::Error {
        retryable,
        raw: Some(raw),
        ..
    } = event
    else {
        panic!("expected normalized error");
    };
    assert!(retryable);
    assert_eq!(raw["error"]["token"], "[REDACTED]");
}

#[test]
fn codex_detection_without_handshake_stays_detected() {
    let probe = CodexAdapter::detected_probe(
        PathBuf::from(r"C:\Tools\codex.exe"),
        "0.150.0-alpha.12.2".to_owned(),
    );

    assert_eq!(probe.provider, ProviderKind::Codex);
    assert_eq!(probe.verification, ProviderVerification::Detected);
    assert!(probe.authentication.detail.is_none());
}

#[tokio::test]
#[ignore = "requires an installed and configured Codex App Server"]
async fn live_codex_app_server_handshake() {
    let probe = CodexAdapter::default()
        .probe(&DiscoveryEnvironment::current())
        .await
        .unwrap();

    println!("{}", serde_json::to_string_pretty(&probe).unwrap());
    assert_eq!(probe.provider, ProviderKind::Codex);
    assert!(matches!(
        probe.verification,
        ProviderVerification::Configured | ProviderVerification::Unverified
    ));
    assert_eq!(probe.protocol.as_deref(), Some("app-server/v2"));
}

#[tokio::test]
#[ignore = "requires an installed and configured Codex App Server; starts no model turn"]
async fn live_codex_session_start_reports_contained_host() {
    let session = CodexAdapter::default()
        .start(&SessionRequest::new(std::env::current_dir().unwrap()))
        .await
        .unwrap();

    assert!(session.native_host_pid().is_some());
    session.stop().await.unwrap();
}

#[tokio::test]
#[ignore = "uses the configured Codex account and consumes provider quota"]
async fn live_codex_single_turn_and_resume() {
    let adapter = CodexAdapter::default();
    let request = SessionRequest::new(std::env::current_dir().unwrap());
    let session = adapter.start(&request).await.unwrap();
    let session_id = session.id();
    let events = session
        .turn(&TurnRequest::new(
            "Reply with exactly LAZOO_CODEX_OK. Do not use tools.",
        ))
        .await
        .unwrap();
    println!("{}", serde_json::to_string_pretty(&events).unwrap());
    let output = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert!(output.contains("LAZOO_CODEX_OK"), "output was {output:?}");
    session.stop().await.unwrap();

    let resumed = adapter
        .start(&SessionRequest::new(std::env::current_dir().unwrap()).resume(&session_id))
        .await
        .unwrap();
    assert_eq!(resumed.id(), session_id);
    resumed.stop().await.unwrap();
}

#[derive(Default)]
struct ApprovingObserver {
    requests: Mutex<Vec<PermissionRequest>>,
}

impl TurnEventObserver for ApprovingObserver {
    fn observe(&self, _event: &ProviderEvent) -> Result<(), ProviderError> {
        Ok(())
    }

    fn decide_permission<'a>(
        &'a self,
        request: &'a PermissionRequest,
    ) -> AdapterFuture<'a, PermissionDecision> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request.clone());
            Ok(PermissionDecision::Approve)
        })
    }
}

#[test]
fn codex_approval_requests_normalize_into_bounded_provider_neutral_requests() {
    let command = normalize_codex_permission_request(
        "item/commandExecution/requestApproval",
        "7",
        &json!({
            "threadId": "thread-1",
            "turnId": "turn-1",
            "itemId": "item-9",
            "command": ["cargo", "test", "--workspace"],
            "cwd": "C:\\repo",
            "reason": "run the suite",
            "apiKey": "sk-secret"
        }),
        None,
    );
    assert_eq!(command.kind, PermissionKind::CommandExecution);
    assert_eq!(command.request_id, "7");
    assert_eq!(command.session_id, "thread-1");
    assert_eq!(command.turn_id.as_deref(), Some("turn-1"));
    assert_eq!(command.action, "cargo test --workspace");
    assert!(!command.scope_expansion);
    assert_eq!(command.detail["command"], "cargo test --workspace");
    assert_eq!(command.detail["cwd"], "C:\\repo");
    assert_eq!(command.detail["reason"], "run the suite");
    assert!(command.detail.get("apiKey").is_none());

    let widened = normalize_codex_permission_request(
        "item/fileChange/requestApproval",
        "8",
        &json!({"threadId": "thread-1", "itemId": "item-10", "grantRoot": "D:\\elsewhere"}),
        Some("active-turn".to_owned()),
    );
    assert_eq!(widened.kind, PermissionKind::FileChange);
    assert!(widened.scope_expansion);
    assert_eq!(widened.turn_id.as_deref(), Some("active-turn"));
    assert_eq!(widened.action, "apply file changes under D:\\elsewhere");

    let oversized = normalize_codex_permission_request(
        "item/commandExecution/requestApproval",
        "9",
        &json!({"threadId": "thread-1", "command": "x".repeat(10_000)}),
        None,
    );
    assert_eq!(oversized.action.len(), 1024);
    assert_eq!(oversized.detail["command"].as_str().unwrap().len(), 4096);
}

#[tokio::test]
async fn observed_codex_approval_requests_are_answered_with_the_lazoo_decision() {
    ensure_session_host();
    let approving_dir = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(approving_dir.path()))
        .await
        .unwrap();
    let observer = ApprovingObserver::default();
    let events = session
        .turn_observed(&TurnRequest::new("please REQUEST_APPROVAL"), &observer)
        .await
        .unwrap();
    session.stop().await.unwrap();

    let response_text = fs::read_to_string(
        approving_dir
            .path()
            .join("fake-codex-approval-response.json"),
    )
    .unwrap();
    let response: serde_json::Value =
        serde_json::from_str(response_text.trim_start_matches('\u{feff}').trim()).unwrap();
    assert_eq!(response["id"], "approval-1");
    assert_eq!(response["result"]["decision"], "accept");
    let requests = observer.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].request_id, "approval-1");
    assert_eq!(requests[0].action, "cargo test");
    assert_eq!(requests[0].turn_id.as_deref(), Some("turn-1"));
    let permission = events
        .iter()
        .find(|event| matches!(event, ProviderEvent::PermissionRequested { .. }))
        .expect("the approval request is surfaced as a permission event");
    let ProviderEvent::PermissionRequested { raw: Some(raw), .. } = permission else {
        panic!("permission event should retain bounded raw evidence");
    };
    assert_eq!(raw["lazooDecision"], "accept");
    assert_eq!(raw["lazooAction"], "cargo test");
    assert!(matches!(
        events.last(),
        Some(ProviderEvent::TurnCompleted { .. })
    ));

    let declining_dir = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(declining_dir.path()))
        .await
        .unwrap();
    session
        .turn_observed(
            &TurnRequest::new("please REQUEST_APPROVAL"),
            &CollectingObserver::default(),
        )
        .await
        .unwrap();
    session.stop().await.unwrap();
    let response_text = fs::read_to_string(
        declining_dir
            .path()
            .join("fake-codex-approval-response.json"),
    )
    .unwrap();
    let response: serde_json::Value =
        serde_json::from_str(response_text.trim_start_matches('\u{feff}').trim()).unwrap();
    assert_eq!(
        response["result"]["decision"], "decline",
        "an observer without an approval flow fails closed"
    );
}

#[derive(Default)]
struct ToolAnsweringObserver {
    calls: Mutex<Vec<ToolCall>>,
}

impl TurnEventObserver for ToolAnsweringObserver {
    fn observe(&self, _event: &ProviderEvent) -> Result<(), ProviderError> {
        Ok(())
    }

    fn call_tool<'a>(&'a self, call: &'a ToolCall) -> AdapterFuture<'a, ToolResult> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(call.clone());
            Ok(ToolResult::success(
                json!({"taskId": "task-1", "state": "accepted"}),
            ))
        })
    }
}

fn lazoo_tools() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "task_delegate".to_owned(),
        description: "Delegate a scoped task to a named employee.".to_owned(),
        input_schema: json!({"type": "object", "properties": {"objective": {"type": "string"}}}),
    }]
}

fn read_fixture_json(directory: &Path, name: &str) -> serde_json::Value {
    let text = fs::read_to_string(directory.join(name)).unwrap();
    serde_json::from_str(text.trim_start_matches('\u{feff}').trim()).unwrap()
}

async fn native_view_request(
    viewer: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    viewer
        .send(Message::Text(
            json!({"id": id, "method": method, "params": params})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), viewer.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let reply: serde_json::Value =
            serde_json::from_str(message.into_text().unwrap().as_ref()).unwrap();
        if reply["id"] == json!(id) {
            return reply;
        }
    }
}

fn native_view_request_with_token(
    descriptor: &lazoo_providers::NativeViewLaunchDescriptor,
    token: &str,
) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    let mut request = descriptor
        .remote_url()
        .expect("Codex uses a remote native viewer")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    request
}

#[tokio::test]
async fn native_view_history_failures_cannot_end_canonical_work() {
    ensure_session_host();
    for trigger in ["oversized-view-history", "flood-view-history"] {
        let temporary = tempdir().unwrap();
        let session = CodexAdapter::with_executable(fake_codex())
            .start(&SessionRequest::new(temporary.path()))
            .await
            .unwrap();
        session
            .turn(&TurnRequest::new("persist history"))
            .await
            .unwrap();
        fs::write(temporary.path().join(trigger), "").unwrap();
        let lease = session.open_native_view().await.unwrap().unwrap();
        let descriptor = lease.launch_descriptor();
        let token = descriptor.command().env[&OsString::from("LAZOO_NATIVE_VIEW_TOKEN")]
            .to_string_lossy()
            .into_owned();
        let (mut viewer, _) = connect_async(native_view_request_with_token(&descriptor, &token))
            .await
            .unwrap();
        viewer
            .send(Message::Text(
                json!({"id": 1, "method": "thread/turns/list",
            "params": {"threadId": "thread-1", "limit": 100}})
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        // Either a bounded read error or loss of this view is acceptable.
        let _ = tokio::time::timeout(Duration::from_secs(5), viewer.next())
            .await
            .unwrap();
        lease.close();
        let events = session
            .turn(&TurnRequest::new("canonical work continues"))
            .await
            .unwrap_or_else(|error| panic!("{trigger} ended canonical work: {error}"));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ProviderEvent::TurnCompleted { .. }))
        );
        session.stop().await.unwrap();
    }
}

#[tokio::test]
async fn native_view_oversized_history_during_turn_preserves_completion() {
    ensure_session_host();
    let temporary = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    session
        .turn(&TurnRequest::new("persist history"))
        .await
        .unwrap();
    let lease = session.open_native_view().await.unwrap().unwrap();
    let descriptor = lease.launch_descriptor();
    let token = descriptor.command().env[&OsString::from("LAZOO_NATIVE_VIEW_TOKEN")]
        .to_string_lossy()
        .into_owned();
    let (mut viewer, _) = connect_async(native_view_request_with_token(&descriptor, &token))
        .await
        .unwrap();
    let prompt = TurnRequest::new("VIEW_DURING_TURN");
    let (turn, ()) = tokio::join!(session.turn(&prompt), async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !temporary.path().join("fake-codex-wait-view").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let reply = native_view_request(
            &mut viewer,
            8,
            "thread/turns/list",
            json!({"threadId":"thread-1","limit":100}),
        )
        .await;
        assert_eq!(reply["error"]["code"], -32001);
    });
    assert!(
        turn.unwrap()
            .iter()
            .any(|event| matches!(event, ProviderEvent::TurnCompleted { .. }))
    );
    lease.close();
    session.stop().await.unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One authenticated viewer lifecycle exercises each authority boundary.
async fn native_view_websocket_is_authenticated_read_only_and_preserves_history_errors() {
    ensure_session_host();
    let temporary = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    let lease = session.open_native_view().await.unwrap().unwrap();
    let descriptor = lease.launch_descriptor();
    let command = descriptor.command();
    let token = command
        .env
        .get(&OsString::from("LAZOO_NATIVE_VIEW_TOKEN"))
        .unwrap()
        .to_string_lossy()
        .into_owned();

    assert!(
        connect_async(native_view_request_with_token(&descriptor, "wrong"))
            .await
            .is_err()
    );
    let mut origin_request = native_view_request_with_token(&descriptor, &token);
    origin_request
        .headers_mut()
        .insert(ORIGIN, "https://example.invalid".parse().unwrap());
    assert!(connect_async(origin_request).await.is_err());

    let (mut viewer, _) = connect_async(native_view_request_with_token(&descriptor, &token))
        .await
        .unwrap();
    assert!(
        connect_async(native_view_request_with_token(&descriptor, &token))
            .await
            .is_err()
    );

    let initialize = native_view_request(&mut viewer, 1, "initialize", json!({})).await;
    assert_eq!(initialize["result"]["userAgent"], "fake-codex-app-server");
    let requirements =
        native_view_request(&mut viewer, 11, "configRequirements/read", json!({})).await;
    assert_eq!(requirements["result"], json!({}), "{requirements}");
    let hooks = native_view_request(
        &mut viewer,
        12,
        "hooks/list",
        json!({"cwds":[temporary.path().to_string_lossy()],"forceReload":true}),
    )
    .await;
    assert_eq!(hooks["result"]["data"], json!([]));
    let resumed = native_view_request(
        &mut viewer,
        2,
        "thread/resume",
        json!({"threadId":"thread-1", "approvalPolicy":"never", "cwd":"C:\\other"}),
    )
    .await;
    assert_eq!(resumed["result"]["thread"]["id"], "thread-1");
    assert_eq!(resumed["result"]["thread"]["canAcceptDirectInput"], false);
    let thread_read = native_view_request(
        &mut viewer,
        20,
        "thread/read",
        json!({"threadId":"thread-1"}),
    )
    .await;
    assert_eq!(
        thread_read["result"]["thread"]["canAcceptDirectInput"],
        false
    );
    let thread_list = native_view_request(&mut viewer, 21, "thread/list", json!({})).await;
    assert_eq!(
        thread_list["result"]["data"][0]["canAcceptDirectInput"],
        false
    );

    let wrong_thread =
        native_view_request(&mut viewer, 3, "thread/read", json!({"threadId":"other"})).await;
    assert_eq!(wrong_thread["error"]["code"], -32001);
    // Sidebar reads the restricted bridge does not forward are refused, not proxied: a forwarded
    // `plugin/list` closed the controller app-server on codex-cli 0.165.0-alpha.1.2.
    let plugins = native_view_request(&mut viewer, 99, "plugin/list", json!({})).await;
    assert_eq!(plugins["error"]["code"], -32001, "{plugins}");
    let mutation = native_view_request(
        &mut viewer,
        4,
        "turn/start",
        json!({"threadId":"thread-1", "input":[]}),
    )
    .await;
    assert_eq!(mutation["error"]["code"], -32001);
    viewer
        .send(Message::Text(
            json!({"id":"approval-1","result":{"decision":"accept"}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let response_refusal: serde_json::Value = serde_json::from_str(
        viewer
            .next()
            .await
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap()
            .as_ref(),
    )
    .unwrap();
    assert_eq!(response_refusal["error"]["code"], -32600);
    assert!(!temporary.path().join("fake-codex-turn-start.json").exists());

    let empty_history = native_view_request(
        &mut viewer,
        5,
        "thread/items/list",
        json!({"threadId":"thread-1", "limit":5}),
    )
    .await;
    assert_eq!(empty_history["result"]["data"], json!([]));

    session
        .turn(&TurnRequest::new("fixture turn"))
        .await
        .unwrap();
    let history_error = native_view_request(
        &mut viewer,
        6,
        "thread/items/list",
        json!({"threadId":"thread-1", "limit":5}),
    )
    .await;
    assert_eq!(history_error["error"]["code"], -32001);

    let mut closed = lease.closed_subscription();
    session.stop().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), closed.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(lease.is_closed());
}

#[tokio::test]
async fn native_view_buffers_notifications_until_successful_thread_resume() {
    ensure_session_host();
    let temporary = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    let lease = session.open_native_view().await.unwrap().unwrap();
    let descriptor = lease.launch_descriptor();
    let token = descriptor
        .command()
        .env
        .get(&OsString::from("LAZOO_NATIVE_VIEW_TOKEN"))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let (mut viewer, _) = connect_async(native_view_request_with_token(&descriptor, &token))
        .await
        .unwrap();

    session
        .turn(&TurnRequest::new("notification before viewer resume"))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), viewer.next())
            .await
            .is_err()
    );
    let resumed = native_view_request(
        &mut viewer,
        1,
        "thread/resume",
        json!({"threadId":"thread-1"}),
    )
    .await;
    assert_eq!(resumed["result"]["thread"]["canAcceptDirectInput"], false);
    let notification: serde_json::Value = serde_json::from_str(
        tokio::time::timeout(Duration::from_secs(2), viewer.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap()
            .as_ref(),
    )
    .unwrap();
    assert_eq!(notification["method"], "turn/started");
    session.stop().await.unwrap();
}

#[tokio::test]
async fn native_view_close_terminates_a_nonreading_viewer() {
    ensure_session_host();
    let temporary = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    let lease = session.open_native_view().await.unwrap().unwrap();
    let descriptor = lease.launch_descriptor();
    let token = descriptor
        .command()
        .env
        .get(&OsString::from("LAZOO_NATIVE_VIEW_TOKEN"))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let (mut viewer, _) = connect_async(native_view_request_with_token(&descriptor, &token))
        .await
        .unwrap();
    viewer
        .send(Message::Text(
            json!({"id":1,"method":"thread/resume","params":{"threadId":"thread-1"}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    lease.close();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), viewer.next())
            .await
            .is_ok()
    );
    session.stop().await.unwrap();
}

#[test]
fn codex_tool_calls_normalize_with_the_active_turn_fallback() {
    let call = normalize_codex_tool_call(
        &json!({
            "threadId": "thread-1",
            "callId": "call-7",
            "tool": "employees_list",
            "arguments": {}
        }),
        Some("active-turn".to_owned()),
    );
    assert_eq!(call.provider, ProviderKind::Codex);
    assert_eq!(call.session_id, "thread-1");
    assert_eq!(call.turn_id.as_deref(), Some("active-turn"));
    assert_eq!(call.call_id, "call-7");
    assert_eq!(call.name, "employees_list");
    assert_eq!(call.arguments, json!({}));
    let explicit = normalize_codex_tool_call(
        &json!({"threadId": "thread-1", "turnId": "turn-9", "callId": "c", "tool": "t"}),
        None,
    );
    assert_eq!(explicit.turn_id.as_deref(), Some("turn-9"));
    assert_eq!(explicit.arguments, serde_json::Value::Null);
}

#[tokio::test]
async fn observed_codex_tool_calls_are_answered_with_the_lazoo_result() {
    ensure_session_host();
    let answering_dir = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(answering_dir.path()).with_tools(lazoo_tools()))
        .await
        .unwrap();
    let observer = ToolAnsweringObserver::default();
    let events = session
        .turn_observed(&TurnRequest::new("please CALL_TOOL"), &observer)
        .await
        .unwrap();
    session.stop().await.unwrap();

    let initialize = read_fixture_json(answering_dir.path(), "fake-codex-initialize.json");
    assert_eq!(initialize["capabilities"]["experimentalApi"], true);
    let thread_start = read_fixture_json(answering_dir.path(), "fake-codex-thread-start.json");
    assert_eq!(thread_start["dynamicTools"][0]["type"], "function");
    assert_eq!(thread_start["dynamicTools"][0]["name"], "task_delegate");
    assert_eq!(
        thread_start["dynamicTools"][0]["inputSchema"]["type"],
        "object"
    );

    let response = read_fixture_json(answering_dir.path(), "fake-codex-tool-response.json");
    assert_eq!(response["id"], "tool-1");
    assert_eq!(response["result"]["success"], true);
    assert_eq!(response["result"]["contentItems"][0]["type"], "inputText");
    let content: serde_json::Value = serde_json::from_str(
        response["result"]["contentItems"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(content["taskId"], "task-1");

    let calls = observer.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].call_id, "call-1");
    assert_eq!(calls[0].name, "task_delegate");
    assert_eq!(calls[0].turn_id.as_deref(), Some("turn-1"));
    assert_eq!(calls[0].arguments["objective"], "Fix the parser");
    let tool_event = events
        .iter()
        .find(|event| {
            matches!(
                event,
                ProviderEvent::ToolUpdate { tool_name: Some(name), .. } if name == "lazoo:task_delegate"
            )
        })
        .expect("the tool call is surfaced as a tool event");
    let ProviderEvent::ToolUpdate {
        status,
        raw: Some(raw),
        ..
    } = tool_event
    else {
        panic!("tool event should retain bounded raw evidence");
    };
    assert_eq!(status, "completed");
    assert_eq!(raw["callId"], "call-1");
    assert_eq!(raw["arguments"]["objective"], "Fix the parser");
    assert_eq!(
        raw["arguments"]["apiKey"], "[REDACTED]",
        "tool arguments are redacted before they reach the ledger"
    );
    assert!(matches!(
        events.last(),
        Some(ProviderEvent::TurnCompleted { .. })
    ));

    let unattached_dir = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(unattached_dir.path()).with_tools(lazoo_tools()))
        .await
        .unwrap();
    let events = session
        .turn_observed(
            &TurnRequest::new("please CALL_TOOL"),
            &CollectingObserver::default(),
        )
        .await
        .unwrap();
    session.stop().await.unwrap();
    let response = read_fixture_json(unattached_dir.path(), "fake-codex-tool-response.json");
    assert_eq!(
        response["result"]["success"], false,
        "an observer without the control plane fails closed"
    );
    assert!(
        response["result"]["contentItems"][0]["text"]
            .as_str()
            .unwrap()
            .contains("LZ-TOOL-UNATTACHED")
    );
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::ToolUpdate { status, .. } if status == "failed"
    )));
}

#[tokio::test]
async fn codex_sessions_without_tools_stay_stable_and_resume_with_tools_fails_closed() {
    ensure_session_host();
    let temporary = tempdir().unwrap();
    let session = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    session.stop().await.unwrap();
    let initialize = read_fixture_json(temporary.path(), "fake-codex-initialize.json");
    assert_eq!(initialize["capabilities"]["experimentalApi"], false);
    let thread_start = read_fixture_json(temporary.path(), "fake-codex-thread-start.json");
    assert!(thread_start.get("dynamicTools").is_none());

    let error = CodexAdapter::with_executable(fake_codex())
        .start(
            &SessionRequest::new(temporary.path())
                .resume("thread-1")
                .with_tools(lazoo_tools()),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ProviderError::Unsupported {
            provider: ProviderKind::Codex,
            capability: "dynamic tools on thread/resume",
            ..
        }
    ));
}

#[tokio::test]
async fn codex_refuses_dynamic_tool_names_outside_the_responses_api_alphabet() {
    ensure_session_host();
    let temporary = tempdir().unwrap();
    let dotted = vec![ToolSpec {
        name: "task.delegate".to_owned(),
        description: "dotted names are rejected by the Codex Responses API".to_owned(),
        input_schema: json!({"type": "object"}),
    }];
    let error = CodexAdapter::with_executable(fake_codex())
        .start(&SessionRequest::new(temporary.path()).with_tools(dotted))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, ProviderError::Protocol(message) if message.contains("task.delegate")),
        "unexpected error: {error}"
    );
}
