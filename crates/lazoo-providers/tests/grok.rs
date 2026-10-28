use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use lazoo_providers::{
    AdapterFuture, DiscoveryEnvironment, GrokAdapter, PermissionDecision, PermissionRequest,
    ProviderAdapter, ProviderError, ProviderEvent, ProviderKind, ProviderSession,
    ProviderVerification, SessionRequest, TurnEventObserver, TurnRequest, normalize_acp_message,
};
use serde_json::json;
use tempfile::tempdir;

struct DecliningObserver;

impl TurnEventObserver for DecliningObserver {
    fn observe(&self, _event: &ProviderEvent) -> Result<(), ProviderError> {
        Ok(())
    }
    fn decide_permission<'a>(
        &'a self,
        _request: &'a PermissionRequest,
    ) -> AdapterFuture<'a, PermissionDecision> {
        Box::pin(async { Ok(PermissionDecision::Decline) })
    }
}

fn fake_acp() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/acp/fake-acp.cmd")
}

#[test]
fn grok_launch_preserves_model_reasoning_and_fail_closed_permissions() {
    let request = SessionRequest::new(r"C:\repo")
        .with_model("grok-code-fast")
        .with_effort("high")
        .with_permission_mode("never");
    let command = GrokAdapter::command(Path::new(r"C:\Tools\grok.exe"), &request);

    assert_eq!(command.executable, PathBuf::from(r"C:\Tools\grok.exe"));
    assert_eq!(
        command.args,
        vec![
            OsString::from("--permission-mode"),
            OsString::from("dontAsk"),
            OsString::from("--model"),
            OsString::from("grok-code-fast"),
            OsString::from("--reasoning-effort"),
            OsString::from("high"),
            OsString::from("agent"),
            OsString::from("stdio"),
        ]
    );
    assert!(!command.args.iter().any(|arg| arg == "--always-approve"));
    assert_eq!(command.cwd, PathBuf::from(r"C:\repo"));
}

#[test]
fn grok_defaults_to_dont_ask_instead_of_inheriting_native_always_approve() {
    let command = GrokAdapter::command(
        Path::new(r"C:\Tools\grok.exe"),
        &SessionRequest::new(r"C:\repo"),
    );

    assert_eq!(
        &command.args[..2],
        &[
            OsString::from("--permission-mode"),
            OsString::from("dontAsk")
        ]
    );
}

#[test]
fn grok_acp_updates_normalize_tools_terminal_permissions_and_usage() {
    let events = include_str!("fixtures/grok/session-updates.jsonl")
        .lines()
        .map(|line| {
            normalize_acp_message(
                ProviderKind::GrokBuild,
                &serde_json::from_str(line).unwrap(),
            )
            .unwrap()
            .unwrap()
        })
        .collect::<Vec<_>>();

    assert!(
        matches!(events[0], ProviderEvent::ToolUpdate { ref status, .. } if status == "pending")
    );
    assert!(matches!(
        events[1],
        ProviderEvent::TerminalUpdate { ref output, .. }
            if output.as_deref() == Some("tests passed\n")
    ));
    assert!(matches!(
        events[2],
        ProviderEvent::PermissionRequested { .. }
    ));
    assert!(matches!(
        events[3],
        ProviderEvent::Usage { total_tokens: Some(135), cost_usd: Some(cost), .. }
            if (cost - 0.25).abs() < f64::EPSILON
    ));
}

#[test]
fn grok_resume_uses_negotiated_session_capability() {
    assert!(GrokAdapter::capabilities(&json!({"loadSession": true})).resume);
    assert!(!GrokAdapter::capabilities(&json!({"loadSession": false})).resume);
}

#[tokio::test]
async fn grok_public_session_forwards_observed_acp_permissions() {
    let temporary = tempdir().unwrap();
    let session = GrokAdapter::with_executable(fake_acp())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    let events = session
        .turn_observed(&TurnRequest::new("EXECUTE"), &DecliningObserver)
        .await
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::PermissionRequested { .. }))
    );
    let text = std::fs::read_to_string(temporary.path().join("fake-acp-response-1.json")).unwrap();
    let response: serde_json::Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}').trim()).unwrap();
    assert_eq!(response["result"]["outcome"]["outcome"], "cancelled");
    session.stop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an installed Grok Build ACP server"]
async fn live_grok_acp_handshake() {
    let probe = GrokAdapter::default()
        .probe(&DiscoveryEnvironment::current())
        .await
        .unwrap();

    println!("{}", serde_json::to_string_pretty(&probe).unwrap());
    assert_eq!(probe.provider, ProviderKind::GrokBuild);
    assert!(matches!(
        probe.verification,
        ProviderVerification::Configured | ProviderVerification::Unverified
    ));
    assert_eq!(probe.protocol.as_deref(), Some("acp/v1"));
}

#[tokio::test]
#[ignore = "requires an installed and authenticated Grok Build ACP server"]
async fn live_grok_model_backed_turn_and_resume() {
    let cwd = std::env::current_dir().unwrap();
    let adapter = GrokAdapter::default();
    let session = adapter.start(&SessionRequest::new(&cwd)).await.unwrap();
    let events = session
        .turn(&TurnRequest::new(
            "Reply with exactly LAZOO_GROK_OK. Do not use tools.",
        ))
        .await
        .unwrap();
    let session_id = session.id();
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. }
            if output.trim() == "LAZOO_GROK_OK")
    }));
    session.stop().await.unwrap();

    let resumed = adapter
        .start(&SessionRequest::new(cwd).resume(session_id))
        .await
        .unwrap();
    let resumed_events = resumed
        .turn(&TurnRequest::new(
            "Reply with exactly LAZOO_GROK_RESUME_OK. Do not use tools.",
        ))
        .await
        .unwrap();
    println!(
        "{}",
        serde_json::to_string_pretty(&resumed_events.last()).unwrap()
    );
    assert!(resumed_events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. }
            if output.trim() == "LAZOO_GROK_RESUME_OK")
    }));
    resumed.stop().await.unwrap();
}
