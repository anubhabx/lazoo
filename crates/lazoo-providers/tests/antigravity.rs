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
    let mut request = SessionRequest::new(r"C:\repo")
        .resume("conversation-1")
        .with_model("gemini-3.1-pro-high")
        .with_effort("high")
        .with_permission_mode("plan");
    request
        .launch_environment
        .set("USERPROFILE", r"C:\profiles\agy");
    let command = AntigravityAdapter::command(Path::new(r"C:\Tools\agy.exe"), &request);

    assert_eq!(command.executable, PathBuf::from(r"C:\Tools\agy.exe"));
    assert_eq!(command.cwd, PathBuf::from(r"C:\repo"));
    assert_eq!(
        command.env[&OsString::from("USERPROFILE")],
        OsString::from(r"C:\profiles\agy")
    );
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

fn fake_agy_stall() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/antigravity/fake-agy-stall.cmd")
}

async fn wait_for_file(path: PathBuf) {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !path.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{} was not written", path.display()));
}

fn read_host_pid(directory: &Path) -> u32 {
    fs::read_to_string(directory.join("host-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn host_is_running(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

async fn assert_host_gone(pid: u32) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while host_is_running(pid) {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("fake host {pid} is still running"));
}

#[tokio::test]
async fn agy_cancel_during_active_turn_kills_host_and_reports_cancelled() {
    let cwd = tempfile::tempdir().unwrap();
    let session = AntigravityAdapter::with_executable(fake_agy_stall())
        .start(&SessionRequest::new(cwd.path()))
        .await
        .unwrap();
    let turn = TurnRequest::new("fixture");
    let cancel = async {
        wait_for_file(cwd.path().join("turn-started")).await;
        session.cancel().await.unwrap();
    };

    let (events, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(session.turn(&turn), cancel)
    })
    .await
    .expect("a cancelled turn ends long before the fake's 30 s stall");

    let events = events.unwrap();
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TextDelta { text, .. } if text == "before cancel")
    }));
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::Cancelled { provider: ProviderKind::Antigravity, session_id, .. }
            if session_id == "conversation-1" && *session_id == session.id())
    }));
    assert!(!events.iter().any(|event| {
        matches!(
            event,
            ProviderEvent::TurnCompleted { .. } | ProviderEvent::Error { .. }
        )
    }));
    session.stop().await.unwrap();
    assert_host_gone(read_host_pid(cwd.path())).await;
}

#[tokio::test]
async fn agy_cancel_without_an_active_turn_is_a_no_op_that_keeps_the_host() {
    let cwd = tempfile::tempdir().unwrap();
    let session = AntigravityAdapter::with_executable(fake_agy_stall())
        .start(&SessionRequest::new(cwd.path()))
        .await
        .unwrap();
    wait_for_file(cwd.path().join("host-pid")).await;
    let pid = read_host_pid(cwd.path());
    assert!(host_is_running(pid));

    session.cancel().await.unwrap();
    session.cancel().await.unwrap();

    assert!(
        host_is_running(pid),
        "an idle cancel must not kill the host"
    );
    session.stop().await.unwrap();
    assert_host_gone(pid).await;
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

// Live counterpart of `agy_cancel_during_active_turn_kills_host_and_reports_cancelled`. The
// Antigravity adapter returns its events only after the turn, so there is no first-delta signal to
// wait for: the cancel fires after a short bounded wait (6 s). One short live model turn on the
// owner's account.
#[tokio::test]
#[ignore = "requires an installed and authenticated Antigravity CLI; sends one live model turn"]
async fn live_agy_cancel_mid_turn_reports_cancelled() {
    let session = AntigravityAdapter::default()
        .start(&SessionRequest::new(std::env::current_dir().unwrap()).with_permission_mode("plan"))
        .await
        .unwrap();
    let request = TurnRequest::new(
        "Count slowly from 1 to 400, one number per line, with no other text. Do not use tools.",
    );
    let started = std::time::Instant::now();
    let cancel = async {
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
        session.cancel().await.unwrap();
        started.elapsed()
    };
    let (outcome, cancel_at) = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        tokio::join!(session.turn(&request), cancel)
    })
    .await
    .expect("a cancelled live turn must return long before the 60 s bound");
    println!(
        "cancel_after={cancel_at:?} returned_after={:?}",
        started.elapsed()
    );
    let events = outcome.unwrap();
    println!(
        "events={} terminal event: {:?}",
        events.len(),
        events.last()
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::Cancelled { .. })),
        "expected a Cancelled event, got: {:?}",
        events.last()
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ProviderEvent::TurnCompleted { .. })),
        "a cancelled turn must not also complete"
    );
    session.stop().await.unwrap();
}
