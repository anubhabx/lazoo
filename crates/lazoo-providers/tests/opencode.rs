use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use lazoo_providers::{
    DiscoveryEnvironment, OpenCodeAdapter, ProgressDeadline, ProviderAdapter, ProviderError,
    ProviderEvent, ProviderKind, ProviderSession, ProviderVerification, SessionRequest,
    TurnRequest, normalize_acp_message,
};

#[test]
fn opencode_launches_acp_for_the_requested_workspace() {
    let request = SessionRequest::new(r"C:\repo");
    let command = OpenCodeAdapter::command(Path::new(r"C:\Tools\opencode.exe"), &request);

    assert_eq!(command.executable, PathBuf::from(r"C:\Tools\opencode.exe"));
    assert_eq!(command.args, vec![OsString::from("acp")]);
    assert_eq!(command.cwd, PathBuf::from(r"C:\repo"));
}

#[test]
fn opencode_updates_normalize_text_tools_permissions_and_usage() {
    let events = include_str!("fixtures/opencode/session-updates.jsonl")
        .lines()
        .map(|line| {
            normalize_acp_message(ProviderKind::OpenCode, &serde_json::from_str(line).unwrap())
                .unwrap()
                .unwrap()
        })
        .collect::<Vec<_>>();

    assert!(matches!(events[0], ProviderEvent::TextDelta { ref text, .. } if text == "Hello"));
    assert!(
        matches!(events[1], ProviderEvent::ThoughtDelta { ref text, .. } if text == "Thinking")
    );
    assert!(
        matches!(events[2], ProviderEvent::ToolUpdate { ref status, .. } if status == "pending")
    );
    assert!(
        matches!(events[3], ProviderEvent::ToolUpdate { ref status, .. } if status == "completed")
    );
    assert!(matches!(
        events[4],
        ProviderEvent::Usage {
            total_tokens: Some(1200),
            ..
        }
    ));
    assert!(matches!(
        events[5],
        ProviderEvent::PermissionRequested { ref options, .. }
            if options == &["allow_once".to_owned(), "reject_once".to_owned()]
    ));
}

#[test]
fn opencode_acceptance_without_progress_becomes_stalled() {
    let now = Instant::now();
    let mut deadline = ProgressDeadline::new(Duration::from_secs(30));
    deadline.accepted_at(now.checked_sub(Duration::from_secs(31)).unwrap());

    assert!(matches!(
        deadline.check_at(now),
        Err(ProviderError::Stalled { millis: 30_000 })
    ));
}

#[tokio::test]
#[ignore = "requires an installed OpenCode ACP server"]
async fn live_opencode_acp_handshake() {
    let probe = OpenCodeAdapter::default()
        .probe(&DiscoveryEnvironment::current())
        .await
        .unwrap();

    println!("{}", serde_json::to_string_pretty(&probe).unwrap());
    assert_eq!(probe.provider, ProviderKind::OpenCode);
    assert!(matches!(
        probe.verification,
        ProviderVerification::Configured | ProviderVerification::Unverified
    ));
    assert_eq!(probe.protocol.as_deref(), Some("acp/v1"));
}

#[tokio::test]
#[ignore = "requires an installed and authenticated OpenCode ACP server"]
async fn live_opencode_model_backed_turn() {
    let request = SessionRequest::new(std::env::current_dir().unwrap());
    let session = OpenCodeAdapter::default().start(&request).await.unwrap();
    let events = session
        .turn(&TurnRequest::new(
            "Reply with exactly LAZOO_OPENCODE_OK. Do not use tools.",
        ))
        .await
        .unwrap();

    let session_id = session.id().to_owned();
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. }
            if output.trim() == "LAZOO_OPENCODE_OK")
    }));
    session.stop().await.unwrap();

    let resumed = OpenCodeAdapter::default()
        .start(&SessionRequest::new(std::env::current_dir().unwrap()).resume(session_id))
        .await
        .unwrap();
    let resumed_events = resumed
        .turn(&TurnRequest::new(
            "Reply with exactly LAZOO_OPENCODE_RESUME_OK. Do not use tools.",
        ))
        .await
        .unwrap();
    println!(
        "{}",
        serde_json::to_string_pretty(&resumed_events.last()).unwrap()
    );
    assert!(resumed_events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. }
            if output.trim() == "LAZOO_OPENCODE_RESUME_OK")
    }));
    resumed.stop().await.unwrap();
}
