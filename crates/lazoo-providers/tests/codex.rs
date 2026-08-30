use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use lazoo_providers::{
    CodexAdapter, DiscoveryEnvironment, ProviderAdapter, ProviderCapabilities, ProviderEvent,
    ProviderKind, ProviderSession, ProviderVerification, SessionRequest, TurnRequest,
    normalize_codex_message,
};
use serde_json::json;

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
