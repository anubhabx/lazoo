use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use lazoo_providers::{
    AntigravityAdapter, DiscoveryEnvironment, DiscoverySource, ProviderAdapter, ProviderEvent,
    ProviderKind, ProviderSession, ProviderVerification, SessionRequest, TurnRequest,
    normalize_antigravity_message,
};

#[test]
fn agy_discovery_reads_a_persisted_or_known_executable() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("agy.exe");
    fs::write(&executable, b"fixture").unwrap();
    let environment = DiscoveryEnvironment::empty()
        .with_known_location(ProviderKind::Antigravity, executable.clone());

    let candidates = AntigravityAdapter::default().discover(&environment);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].path, executable);
    assert_eq!(candidates[0].source, DiscoverySource::KnownLocation);
}

#[test]
fn agy_launch_uses_long_lived_stream_json_without_bypassing_permissions() {
    let request = SessionRequest::new(r"C:\repo")
        .resume("conversation-1")
        .with_model("gemini-3.1-pro-high")
        .with_effort("high")
        .with_permission_mode("plan");
    let command = AntigravityAdapter::command(Path::new(r"C:\Tools\agy.exe"), &request);

    assert_eq!(command.executable, PathBuf::from(r"C:\Tools\agy.exe"));
    assert_eq!(command.cwd, PathBuf::from(r"C:\repo"));
    assert!(command.args.windows(2).any(|pair| {
        pair == [
            OsString::from("--input-format"),
            OsString::from("stream-json"),
        ]
    }));
    assert!(command.args.windows(2).any(|pair| {
        pair == [
            OsString::from("--output-format"),
            OsString::from("stream-json"),
        ]
    }));
    assert!(command.args.windows(2).any(|pair| {
        pair == [
            OsString::from("--conversation"),
            OsString::from("conversation-1"),
        ]
    }));
    assert!(command.args.windows(2).any(|pair| {
        pair == [
            OsString::from("--model"),
            OsString::from("gemini-3.1-pro-high"),
        ]
    }));
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| { pair == [OsString::from("--effort"), OsString::from("high")] })
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| { pair == [OsString::from("--mode"), OsString::from("plan")] })
    );
    assert!(
        !command
            .args
            .iter()
            .any(|arg| arg == "--dangerously-skip-permissions")
    );
    assert!(
        command
            .env
            .keys()
            .all(|key| { !key.to_string_lossy().to_ascii_lowercase().contains("token") })
    );
}

#[test]
fn agy_turn_input_uses_the_native_user_event_schema() {
    assert_eq!(
        AntigravityAdapter::turn_input(&TurnRequest::new("hello")),
        serde_json::json!({
            "event": "user",
            "message": {"content": [{"type": "text", "text": "hello"}]}
        })
    );
}

#[test]
fn agy_stream_normalizes_conversation_text_tools_usage_and_result() {
    let events = include_str!("fixtures/antigravity/turn.jsonl")
        .lines()
        .flat_map(|line| {
            normalize_antigravity_message(&serde_json::from_str(line).unwrap()).unwrap()
        })
        .collect::<Vec<_>>();

    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::SessionStarted { session_id, .. }
            if session_id == "conversation-1")
    }));
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TextDelta { text, .. } if text == "Hello")
    }));
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::ThoughtDelta { text, .. } if text == "Thinking")
    }));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::ToolUpdate { .. }))
    );
    assert!(events.iter().any(|event| {
        matches!(
            event,
            ProviderEvent::Usage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                total_tokens: Some(120),
                ..
            }
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. }
            if output == "Hello")
    }));
}

#[tokio::test]
#[ignore = "requires an installed Antigravity CLI"]
async fn live_agy_non_model_probe() {
    let probe = AntigravityAdapter::default()
        .probe(&DiscoveryEnvironment::current())
        .await
        .unwrap();

    println!("{}", serde_json::to_string_pretty(&probe).unwrap());
    assert_eq!(probe.provider, ProviderKind::Antigravity);
    assert_eq!(probe.verification, ProviderVerification::Configured);
    assert_eq!(probe.protocol.as_deref(), Some("agy/stream-json-v1"));
}

#[tokio::test]
#[ignore = "requires an installed and authenticated Antigravity CLI"]
async fn live_agy_model_backed_turn_and_resume() {
    let cwd = std::env::current_dir().unwrap();
    let adapter = AntigravityAdapter::default();
    let session = adapter
        .start(&SessionRequest::new(&cwd).with_permission_mode("plan"))
        .await
        .unwrap();
    let events = session
        .turn(&TurnRequest::new(
            "Reply with exactly LAZOO_AGY_OK. Do not use tools.",
        ))
        .await
        .unwrap();
    let session_id = session.id();
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. }
            if output.trim() == "LAZOO_AGY_OK")
    }));
    let continued_events = session
        .turn(&TurnRequest::new(
            "Reply with exactly LAZOO_AGY_CONTINUE_OK. Do not use tools.",
        ))
        .await
        .unwrap();
    assert_eq!(session.id(), session_id);
    assert!(continued_events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. }
            if output.trim() == "LAZOO_AGY_CONTINUE_OK")
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
    let resumed_events = resumed
        .turn(&TurnRequest::new(
            "Reply with exactly LAZOO_AGY_RESUME_OK. Do not use tools.",
        ))
        .await
        .unwrap();
    println!(
        "{}",
        serde_json::to_string_pretty(&resumed_events.last()).unwrap()
    );
    assert!(resumed_events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. }
            if output.trim() == "LAZOO_AGY_RESUME_OK")
    }));
    resumed.stop().await.unwrap();
}
