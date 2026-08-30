use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use lazoo_providers::{
    AuthenticationState, ClaudeAdapter, ClaudeExternalAuth, DiscoveryEnvironment, DiscoverySource,
    ProviderAdapter, ProviderEvent, ProviderKind, ProviderSession, ProviderVerification,
    SessionRequest, TurnRequest, normalize_claude_message,
};

#[test]
fn claude_discovery_reads_a_persisted_or_known_executable() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("claude.exe");
    fs::write(&executable, b"fixture").unwrap();
    let environment = DiscoveryEnvironment::empty()
        .with_known_location(ProviderKind::ClaudeCode, executable.clone());

    let candidates = ClaudeAdapter::default().discover(&environment);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].path, executable);
    assert_eq!(candidates[0].source, DiscoverySource::KnownLocation);
}

#[test]
fn claude_launch_uses_stream_json_and_never_bypasses_permissions() {
    let request = SessionRequest::new(r"C:\repo")
        .resume("session-1")
        .with_model("claude-opus-4-1")
        .with_effort("high")
        .with_permission_mode("plan");
    let command = ClaudeAdapter::command(
        Path::new(r"C:\Tools\claude.exe"),
        &request,
        "unused-new-session-id",
    );

    assert_eq!(command.executable, PathBuf::from(r"C:\Tools\claude.exe"));
    assert_eq!(command.cwd, PathBuf::from(r"C:\repo"));
    for required in [
        "--print",
        "--verbose",
        "--replay-user-messages",
        "--include-partial-messages",
        "--no-chrome",
    ] {
        assert!(command.args.iter().any(|argument| argument == required));
    }
    for pair in [
        ["--input-format", "stream-json"],
        ["--output-format", "stream-json"],
        ["--resume", "session-1"],
        ["--model", "claude-opus-4-1"],
        ["--effort", "high"],
        ["--permission-mode", "plan"],
    ] {
        assert!(
            command.args.windows(2).any(|arguments| {
                arguments == [OsString::from(pair[0]), OsString::from(pair[1])]
            })
        );
    }
    assert!(!command.args.iter().any(|argument| {
        matches!(
            argument.to_string_lossy().as_ref(),
            "--dangerously-skip-permissions" | "--allow-dangerously-skip-permissions"
        )
    }));
}

#[test]
fn claude_new_session_has_a_caller_supplied_uuid_and_safe_default_permissions() {
    let command = ClaudeAdapter::command(
        Path::new(r"C:\Tools\claude.exe"),
        &SessionRequest::new(r"C:\repo"),
        "00000000-0000-4000-8000-000000000001",
    );

    assert!(command.args.windows(2).any(|arguments| {
        arguments
            == [
                OsString::from("--session-id"),
                OsString::from("00000000-0000-4000-8000-000000000001"),
            ]
    }));
    assert!(command.args.windows(2).any(|arguments| {
        arguments
            == [
                OsString::from("--permission-mode"),
                OsString::from("dontAsk"),
            ]
    }));
}

#[test]
fn claude_turn_input_uses_the_native_user_message_schema() {
    assert_eq!(
        ClaudeAdapter::turn_input(&TurnRequest::new("hello")),
        serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": "hello"}]
            }
        })
    );
}

#[test]
fn claude_stream_normalizes_session_deltas_tools_usage_and_result() {
    let events = include_str!("fixtures/claude/turn.jsonl")
        .lines()
        .flat_map(|line| normalize_claude_message(&serde_json::from_str(line).unwrap()).unwrap())
        .collect::<Vec<_>>();

    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::SessionStarted { session_id, .. }
            if session_id == "session-1")
    }));
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TextDelta { text, .. } if text == "Hello")
    }));
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::ThoughtDelta { text, .. } if text == "Thinking")
    }));
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::ToolUpdate { tool_name: Some(name), .. }
            if name == "Bash")
    }));
    assert!(
        events
            .iter()
            .any(|event| { matches!(event, ProviderEvent::TerminalUpdate { .. }) })
    );
    assert!(events.iter().any(|event| {
        matches!(
            event,
            ProviderEvent::Usage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                total_tokens: Some(120),
                cost_usd: Some(cost),
                ..
            } if (cost - 0.0025).abs() < f64::EPSILON
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. }
            if output == "Hello")
    }));
}

#[test]
fn claude_auth_evidence_keeps_native_and_external_channels_separate() {
    let native = serde_json::json!({
        "loggedIn": false,
        "authMethod": "none",
        "apiProvider": "firstParty",
        "email": "must-not-leak@example.test"
    });
    let authentication = ClaudeAdapter::authentication_from_status(
        &native,
        ClaudeExternalAuth {
            endpoint_configured: true,
            credential_configured: true,
        },
    );

    assert_eq!(authentication.native, AuthenticationState::NeedsLogin);
    assert_eq!(authentication.external, AuthenticationState::Ready);
    assert!(!format!("{authentication:?}").contains("must-not-leak"));
}

#[test]
fn claude_api_failures_never_normalize_as_successful_turns() {
    let events = normalize_claude_message(&serde_json::json!({
        "type": "result",
        "subtype": "success",
        "is_error": true,
        "terminal_reason": "api_error",
        "session_id": "session-1",
        "result": "organization access is disabled",
        "usage": {"input_tokens": 0, "output_tokens": 0}
    }))
    .unwrap();

    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::Error { code, message, .. }
            if code == "api_error" && message == "organization access is disabled")
    }));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ProviderEvent::TurnCompleted { .. }))
    );
}

#[tokio::test]
#[ignore = "requires an installed Claude Code CLI"]
async fn live_claude_non_model_probe() {
    let probe = ClaudeAdapter::default()
        .probe(&DiscoveryEnvironment::current())
        .await
        .unwrap();

    println!("{}", serde_json::to_string_pretty(&probe).unwrap());
    assert_eq!(probe.provider, ProviderKind::ClaudeCode);
    assert!(matches!(
        probe.verification,
        ProviderVerification::Configured | ProviderVerification::Degraded
    ));
    assert_eq!(
        probe.protocol.as_deref(),
        Some("claude-code/stream-json-v1")
    );
}

#[tokio::test]
#[ignore = "requires an installed and model-enabled Claude Code CLI"]
async fn live_claude_model_backed_turn_and_resume() {
    let cwd = std::env::current_dir().unwrap();
    let adapter = ClaudeAdapter::default();
    let session = adapter
        .start(&SessionRequest::new(&cwd).with_permission_mode("plan"))
        .await
        .unwrap();
    let events = session
        .turn(&TurnRequest::new(
            "Reply with exactly LAZOO_CLAUDE_OK. Do not use tools.",
        ))
        .await
        .unwrap();
    if let Some(ProviderEvent::Error { message, .. }) = events
        .iter()
        .find(|event| matches!(event, ProviderEvent::Error { .. }))
    {
        println!("Claude live error: {message}");
    }
    let session_id = session.id();
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. }
            if output.trim() == "LAZOO_CLAUDE_OK")
    }));
    session.stop().await.unwrap();

    let resumed = adapter
        .start(
            &SessionRequest::new(cwd)
                .resume(session_id)
                .with_permission_mode("plan"),
        )
        .await
        .unwrap();
    let events = resumed
        .turn(&TurnRequest::new(
            "Reply with exactly LAZOO_CLAUDE_RESUME_OK. Do not use tools.",
        ))
        .await
        .unwrap();
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. }
            if output.trim() == "LAZOO_CLAUDE_RESUME_OK")
    }));
    resumed.stop().await.unwrap();
}
