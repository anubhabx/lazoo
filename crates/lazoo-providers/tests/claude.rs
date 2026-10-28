use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use lazoo_providers::{
    AdapterFuture, AuthenticationState, ClaudeAdapter, ClaudeExternalAuth, DiscoveryEnvironment,
    DiscoverySource, PermissionDecision, PermissionRequest, ProviderAdapter, ProviderEvent,
    ProviderKind, ProviderSession, ProviderVerification, SessionRequest, TurnEventObserver,
    TurnRequest, normalize_claude_message,
};
use tokio::sync::Notify;

struct PermissionObserver {
    events: Mutex<Vec<ProviderEvent>>,
    requests: Mutex<Vec<PermissionRequest>>,
    entered: Notify,
    release: Notify,
    released: AtomicBool,
    approve: AtomicBool,
    decisions: AtomicUsize,
}

impl PermissionObserver {
    fn new(approve: bool) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
            entered: Notify::new(),
            release: Notify::new(),
            released: AtomicBool::new(false),
            approve: AtomicBool::new(approve),
            decisions: AtomicUsize::new(0),
        }
    }
}

impl TurnEventObserver for PermissionObserver {
    fn observe(&self, event: &ProviderEvent) -> Result<(), lazoo_providers::ProviderError> {
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
    fn decide_permission<'a>(
        &'a self,
        request: &'a PermissionRequest,
    ) -> AdapterFuture<'a, PermissionDecision> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request.clone());
            self.decisions.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            while !self.released.load(Ordering::SeqCst) {
                let notified = self.release.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.released.load(Ordering::SeqCst) {
                    break;
                }
                notified.await;
            }
            Ok(if self.approve.load(Ordering::SeqCst) {
                PermissionDecision::Approve
            } else {
                PermissionDecision::Decline
            })
        })
    }
}

fn fake_claude() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude/fake-claude.cmd")
}

fn fake_claude_auth(mode: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/claude/fake-claude-{mode}.cmd"))
}

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
        ["--permission-prompts", "host"],
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

#[tokio::test]
async fn claude_host_permission_is_streamed_then_replies_with_one_action_approval() {
    let cwd = tempfile::tempdir().unwrap();
    let adapter = ClaudeAdapter::with_executable(fake_claude());
    let session = adapter
        .start(&SessionRequest::new(cwd.path()))
        .await
        .unwrap();
    let observer = PermissionObserver::new(true);
    let turn = TurnRequest::new("fixture");
    let work = session.turn_observed(&turn, &observer);
    tokio::pin!(work);
    tokio::select! {
        () = observer.entered.notified() => {}
        result = &mut work => panic!("permission did not wait: {result:?}"),
    }
    assert!(observer.events.lock().unwrap().iter().any(|event| matches!(event, ProviderEvent::TextDelta { text, .. } if text == "before permission")));
    let permission = observer.requests.lock().unwrap().pop().unwrap();
    assert_eq!(
        permission.kind,
        lazoo_providers::PermissionKind::CommandExecution
    );
    assert!(permission.scope_expansion);
    assert!(!permission.detail.to_string().contains("super-secret-token"));
    observer.released.store(true, Ordering::SeqCst);
    observer.release.notify_waiters();
    let events = work.await.unwrap();
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::PermissionRequested { request_id: Some(id), .. } if id == "permission-1")));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. } if output == "allow")));
    assert!(
        !serde_json::to_string(&observer.events.lock().unwrap().clone())
            .unwrap()
            .contains("super-secret-token")
    );
    session.stop().await.unwrap();
}

#[tokio::test]
async fn claude_host_permission_declines_without_duplicate_authorization() {
    let cwd = tempfile::tempdir().unwrap();
    let adapter = ClaudeAdapter::with_executable(fake_claude());
    let session = adapter
        .start(&SessionRequest::new(cwd.path()))
        .await
        .unwrap();
    let observer = PermissionObserver::new(false);
    let turn = TurnRequest::new("fixture");
    let work = session.turn_observed(&turn, &observer);
    tokio::pin!(work);
    tokio::select! {
        () = observer.entered.notified() => {}
        result = &mut work => panic!("permission did not wait: {result:?}"),
    }
    observer.released.store(true, Ordering::SeqCst);
    observer.release.notify_waiters();
    let events = work.await.unwrap();
    assert_eq!(observer.decisions.load(Ordering::SeqCst), 1);
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. } if output == "deny")));
    session.stop().await.unwrap();
}

#[tokio::test]
async fn claude_replays_the_first_response_for_a_duplicate_native_request_id() {
    let cwd = tempfile::tempdir().unwrap();
    let session = ClaudeAdapter::with_executable(fake_claude())
        .start(&SessionRequest::new(cwd.path()))
        .await
        .unwrap();
    let observer = PermissionObserver::new(true);
    let turn = TurnRequest::new("duplicate");
    let work = session.turn_observed(&turn, &observer);
    tokio::pin!(work);
    tokio::select! {
        () = observer.entered.notified() => {}
        result = &mut work => panic!("permission did not wait: {result:?}"),
    }
    observer.released.store(true, Ordering::SeqCst);
    observer.release.notify_waiters();
    let events = work.await.unwrap();
    assert_eq!(observer.decisions.load(Ordering::SeqCst), 1);
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::TurnCompleted { output: Some(output), .. } if output == "allow/allow")));
    session.stop().await.unwrap();
}

#[tokio::test]
async fn claude_refuses_queued_startup_frames_before_writing_a_prompt() {
    let cwd = tempfile::tempdir().unwrap();
    let session = ClaudeAdapter::with_executable(fake_claude())
        .start(&SessionRequest::new(cwd.path()).with_model("queued-startup"))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !cwd.path().join("queued-ready").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let error = session
        .turn(&TurnRequest::new("fixture"))
        .await
        .unwrap_err();
    assert!(
        matches!(error, lazoo_providers::ProviderError::InvalidEvent(message) if message.contains("before the first turn prompt"))
    );
    session.stop().await.unwrap();
}

#[tokio::test]
async fn claude_discards_only_correlated_startup_hook_frames_before_the_prompt() {
    let cwd = tempfile::tempdir().unwrap();
    let session = ClaudeAdapter::with_executable(fake_claude())
        .start(&SessionRequest::new(cwd.path()).with_model("startup-hooks"))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !cwd.path().join("startup-hooks-ready").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let events = session.turn(&TurnRequest::new("fixture")).await.unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::TurnCompleted { output: Some(output), .. } if output == "deny"
    )));
    session.stop().await.unwrap();
}

#[tokio::test]
async fn claude_refuses_mismatched_startup_hook_before_writing_a_prompt() {
    let cwd = tempfile::tempdir().unwrap();
    let session = ClaudeAdapter::with_executable(fake_claude())
        .start(&SessionRequest::new(cwd.path()).with_model("wrong-session-hook"))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !cwd.path().join("startup-hooks-ready").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let error = session
        .turn(&TurnRequest::new("fixture"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            lazoo_providers::ProviderError::InvalidEvent(ref message)
                if message.contains("before the first turn prompt")
        ),
        "{error:?}"
    );
    session.stop().await.unwrap();
}

#[tokio::test]
async fn claude_refuses_non_startup_hook_before_writing_a_prompt() {
    let cwd = tempfile::tempdir().unwrap();
    let session = ClaudeAdapter::with_executable(fake_claude())
        .start(&SessionRequest::new(cwd.path()).with_model("non-startup-hook"))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !cwd.path().join("startup-hooks-ready").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let error = session
        .turn(&TurnRequest::new("fixture"))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        lazoo_providers::ProviderError::InvalidEvent(message)
            if message.contains("before the first turn prompt")
    ));
    session.stop().await.unwrap();
}

#[tokio::test]
async fn claude_bounds_startup_hook_drain_before_writing_a_prompt() {
    let cwd = tempfile::tempdir().unwrap();
    let session = ClaudeAdapter::with_executable(fake_claude())
        .start(&SessionRequest::new(cwd.path()).with_model("startup-hook-flood"))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !cwd.path().join("startup-hooks-ready").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let error = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        session.turn(&TurnRequest::new("fixture")),
    )
    .await
    .expect("startup hook flood must not block")
    .unwrap_err();
    assert!(matches!(
        error,
        lazoo_providers::ProviderError::InvalidEvent(message)
            if message.contains("too many startup hook frames")
    ));
    session.stop().await.unwrap();
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
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { status, .. } if status == "completed")
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

#[tokio::test]
async fn claude_probe_accepts_logged_out_exit_one_status() {
    let probe = ClaudeAdapter::with_executable(fake_claude_auth("logged-out"))
        .probe(&DiscoveryEnvironment::empty())
        .await
        .unwrap();

    assert_eq!(probe.authentication.native, AuthenticationState::NeedsLogin);
    assert_eq!(probe.authentication.external, AuthenticationState::Unknown);
    assert_eq!(probe.verification, ProviderVerification::Degraded);
    assert!(probe.degradation_reason.is_some());
}

#[tokio::test]
async fn claude_probe_refuses_malformed_or_other_nonzero_auth_status() {
    for mode in ["malformed", "error"] {
        let probe = ClaudeAdapter::with_executable(fake_claude_auth(mode))
            .probe(&DiscoveryEnvironment::empty())
            .await
            .unwrap();
        assert_eq!(probe.verification, ProviderVerification::Degraded);
        assert_eq!(probe.authentication.native, AuthenticationState::Unknown);
        assert!(
            probe
                .degradation_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("native auth probe failed"))
        );
    }
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
            .any(|event| matches!(event, ProviderEvent::TurnCompleted { status, .. } if status == "completed"))
    );
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TurnCompleted { status, .. } if status == "failed")
    }));
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
async fn claude_cancel_during_active_turn_kills_host_and_reports_cancelled() {
    let cwd = tempfile::tempdir().unwrap();
    let session = ClaudeAdapter::with_executable(fake_claude())
        .start(&SessionRequest::new(cwd.path()).with_model("stall-turn"))
        .await
        .unwrap();
    let turn = TurnRequest::new("fixture");
    let observer = PermissionObserver::new(true);
    let cancel = async {
        wait_for_file(cwd.path().join("turn-started")).await;
        session.cancel().await.unwrap();
    };

    let (events, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(session.turn_observed(&turn, &observer), cancel)
    })
    .await
    .expect("a cancelled turn ends long before the fake's 30 s stall");

    let events = events.unwrap();
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::TextDelta { text, .. } if text == "before cancel")
    }));
    assert!(events.iter().any(|event| {
        matches!(event, ProviderEvent::Cancelled { provider: ProviderKind::ClaudeCode, session_id, .. }
            if *session_id == session.id())
    }));
    assert!(!events.iter().any(|event| {
        matches!(
            event,
            ProviderEvent::TurnCompleted { .. } | ProviderEvent::Error { .. }
        )
    }));
    // The daemon records the cancellation from its observer, so the adapter must observe the
    // `Cancelled` event and not only return it (a live cancel ended `uncertain` before this).
    assert!(
        observer
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, ProviderEvent::Cancelled { .. })),
        "the observer must see the cancellation"
    );
    session.stop().await.unwrap();
    assert_host_gone(read_host_pid(cwd.path())).await;
}

#[tokio::test]
async fn claude_cancel_without_an_active_turn_is_a_no_op_that_keeps_the_host() {
    let cwd = tempfile::tempdir().unwrap();
    let session = ClaudeAdapter::with_executable(fake_claude())
        .start(&SessionRequest::new(cwd.path()).with_model("stall-turn"))
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
