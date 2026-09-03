use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
};

use lazoo_providers::{
    CodexAdapter, DiscoveryEnvironment, ProviderAdapter, ProviderCapabilities, ProviderError,
    ProviderEvent, ProviderKind, ProviderSession, ProviderVerification, SessionRequest,
    TurnEventObserver, TurnRequest, normalize_codex_message,
};
use serde_json::json;
use tempfile::tempdir;

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
    assert!(matches!(captured[1], ProviderEvent::TextDelta { .. }));
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
