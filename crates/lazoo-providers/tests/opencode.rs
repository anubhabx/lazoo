use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use lazoo_providers::{
    AdapterFuture, DiscoveryEnvironment, OpenCodeAdapter, PermissionDecision, PermissionRequest,
    ProgressDeadline, ProviderAdapter, ProviderError, ProviderEvent, ProviderKind, ProviderSession,
    ProviderVerification, SessionRequest, TurnEventObserver, TurnRequest, normalize_acp_message,
};
use tempfile::tempdir;
use tokio::{sync::Notify, time::timeout};

struct GateObserver {
    events: Mutex<Vec<ProviderEvent>>,
    requests: Mutex<Vec<PermissionRequest>>,
    entered: Notify,
    release: Notify,
    approve: AtomicBool,
    decisions: AtomicUsize,
}

impl GateObserver {
    fn approving() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
            entered: Notify::new(),
            release: Notify::new(),
            approve: AtomicBool::new(true),
            decisions: AtomicUsize::new(0),
        }
    }
}

impl TurnEventObserver for GateObserver {
    fn observe(&self, event: &ProviderEvent) -> Result<(), ProviderError> {
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
            self.release.notified().await;
            Ok(if self.approve.load(Ordering::SeqCst) {
                PermissionDecision::Approve
            } else {
                PermissionDecision::Decline
            })
        })
    }
}

struct ErrorObserver;

impl TurnEventObserver for ErrorObserver {
    fn observe(&self, _event: &ProviderEvent) -> Result<(), ProviderError> {
        Ok(())
    }

    fn decide_permission<'a>(
        &'a self,
        _request: &'a PermissionRequest,
    ) -> AdapterFuture<'a, PermissionDecision> {
        Box::pin(async { Err(ProviderError::Observer("approval store failed".to_owned())) })
    }
}

fn fake_acp() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/acp/fake-acp.cmd")
}

fn response(directory: &Path, name: &str) -> serde_json::Value {
    let path = directory.join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        let names = std::fs::read_dir(directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        panic!("missing {} ({error}); files: {names:?}", path.display());
    });
    serde_json::from_str(text.trim_start_matches('\u{feff}').trim()).unwrap()
}

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
async fn observed_acp_turn_streams_before_completion_and_selects_the_offered_numeric_allow_once() {
    let temporary = tempdir().unwrap();
    let session = OpenCodeAdapter::with_executable(fake_acp())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    let observer = GateObserver::approving();
    let request = TurnRequest::new("NUMERIC_ID");
    let turn = session.turn_observed(&request, &observer);
    tokio::pin!(turn);
    timeout(Duration::from_secs(2), async {
        tokio::select! {
            () = observer.entered.notified() => {},
            result = &mut turn => panic!("turn completed before approval gate: {result:?}"),
        }
    })
    .await
    .unwrap();
    assert!(temporary.path().join("fake-acp-pending.txt").exists());
    assert!(!temporary.path().join("fake-acp-response-1.json").exists());
    assert!(observer.events.lock().unwrap().iter().any(
        |event| matches!(event, ProviderEvent::TextDelta { text, .. } if text == "before approval")
    ));
    assert!(observer.events.lock().unwrap().iter().all(
        |event| !matches!(event, ProviderEvent::TextDelta { text, .. } if text == "stale-before-turn")
    ));
    observer.release.notify_one();
    let returned = timeout(Duration::from_secs(2), &mut turn)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*observer.events.lock().unwrap(), returned);
    let wire = response(temporary.path(), "fake-acp-response-1.json");
    assert_eq!(wire["id"], 77);
    assert_eq!(wire["result"]["outcome"]["outcome"], "selected");
    assert_eq!(wire["result"]["outcome"]["optionId"], "allow-one");
    assert_eq!(observer.requests.lock().unwrap().len(), 1);
    session.stop().await.unwrap();
}

#[tokio::test]
async fn observed_acp_refuses_malformed_and_duplicate_requests_without_reauthorizing() {
    for prompt in ["MISMATCH", "MISSING_ONCE", "UNKNOWN_KIND"] {
        let temporary = tempdir().unwrap();
        let session = OpenCodeAdapter::with_executable(fake_acp())
            .start(&SessionRequest::new(temporary.path()))
            .await
            .unwrap();
        let observer = GateObserver::approving();
        let events = session
            .turn_observed(&TurnRequest::new(prompt), &observer)
            .await
            .unwrap();
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, ProviderEvent::PermissionRequested { .. }))
        );
        assert_eq!(
            response(temporary.path(), "fake-acp-response-1.json")["result"]["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(observer.decisions.load(Ordering::SeqCst), 0);
        session.stop().await.unwrap();
    }

    let temporary = tempdir().unwrap();
    let session = OpenCodeAdapter::with_executable(fake_acp())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    let observer = GateObserver::approving();
    let request = TurnRequest::new("DUPLICATE NEW_ID");
    let turn = session.turn_observed(&request, &observer);
    tokio::pin!(turn);
    timeout(Duration::from_secs(2), async { tokio::select! { () = observer.entered.notified() => {}, result = &mut turn => panic!("turn completed early: {result:?}"), } }).await.unwrap();
    observer.release.notify_one();
    turn.await.unwrap();
    assert_eq!(observer.decisions.load(Ordering::SeqCst), 1);
    assert_eq!(
        response(temporary.path(), "fake-acp-response-2.json")["result"]["outcome"]["outcome"],
        "cancelled"
    );
    session.stop().await.unwrap();
}

#[tokio::test]
async fn observed_acp_permission_titles_are_not_durable_and_cannot_be_reauthorized() {
    let temporary = tempdir().unwrap();
    let session = OpenCodeAdapter::with_executable(fake_acp())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    let observer = GateObserver::approving();
    let request = TurnRequest::new("SECRET_TITLE DUPLICATE NEW_ID");
    let turn = session.turn_observed(&request, &observer);
    tokio::pin!(turn);
    timeout(Duration::from_secs(2), async {
        tokio::select! {
            () = observer.entered.notified() => {},
            result = &mut turn => panic!("turn completed early: {result:?}"),
        }
    })
    .await
    .unwrap();
    observer.release.notify_one();
    turn.await.unwrap();
    let requests = observer.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].action.contains("secret-title-token"));
    assert!(
        !requests[0]
            .detail
            .to_string()
            .contains("secret-title-token")
    );
    assert_eq!(observer.decisions.load(Ordering::SeqCst), 1);
    assert_eq!(
        response(temporary.path(), "fake-acp-response-2.json")["result"]["outcome"]["outcome"],
        "cancelled"
    );
    session.stop().await.unwrap();
}

#[tokio::test]
async fn acp_native_cancellation_missing_observer_and_deadline_all_reply_cancelled() {
    let temporary = tempdir().unwrap();
    let session = OpenCodeAdapter::with_executable(fake_acp())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    session.turn(&TurnRequest::new("EXECUTE")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        response(temporary.path(), "fake-acp-response-1.json")["result"]["outcome"]["outcome"],
        "cancelled"
    );
    session.stop().await.unwrap();

    let temporary = tempdir().unwrap();
    let session = OpenCodeAdapter::with_executable(fake_acp())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    session
        .turn_observed(&TurnRequest::new("EXECUTE"), &ErrorObserver)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        response(temporary.path(), "fake-acp-response-1.json")["result"]["outcome"]["outcome"],
        "cancelled"
    );
    session.stop().await.unwrap();

    let temporary = tempdir().unwrap();
    let session = OpenCodeAdapter::with_executable(fake_acp())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    let observer = GateObserver::approving();
    let request = TurnRequest::new("EXECUTE");
    let turn = session.turn_observed(&request, &observer);
    tokio::pin!(turn);
    timeout(Duration::from_secs(2), async { tokio::select! { () = observer.entered.notified() => {}, result = &mut turn => panic!("turn completed early: {result:?}"), } }).await.unwrap();
    session.cancel().await.unwrap();
    turn.await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(temporary.path().join("fake-acp-cancel.txt").exists());
    assert_eq!(
        response(temporary.path(), "fake-acp-response-1.json")["result"]["outcome"]["outcome"],
        "cancelled"
    );
    session.stop().await.unwrap();

    let temporary = tempdir().unwrap();
    let mut session_request = SessionRequest::new(temporary.path());
    session_request.turn_timeout_millis = 1_000;
    let session = OpenCodeAdapter::with_executable(fake_acp())
        .start(&session_request)
        .await
        .unwrap();
    let observer = GateObserver::approving();
    let request = TurnRequest::new("EXECUTE");
    let outcome = timeout(
        Duration::from_secs(3),
        session.turn_observed(&request, &observer),
    )
    .await
    .expect("approval waiting must not outlive the turn deadline");
    assert!(matches!(outcome, Err(ProviderError::Timeout { .. })));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        response(temporary.path(), "fake-acp-response-1.json")["result"]["outcome"]["outcome"],
        "cancelled"
    );
    session.stop().await.unwrap();
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

    let session_id = session.id();
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

struct FirstDeltaObserver {
    events: Mutex<Vec<ProviderEvent>>,
    first_delta: Notify,
}

impl TurnEventObserver for FirstDeltaObserver {
    fn observe(&self, event: &ProviderEvent) -> Result<(), ProviderError> {
        if matches!(event, ProviderEvent::TextDelta { .. }) {
            self.first_delta.notify_one();
        }
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}

// Live counterpart of the ACP cancel fixture test: the real OpenCode server streams a long answer
// and `cancel()` after the first text delta (or after 20 s without one) must end the turn with a
// `Cancelled` event and no `TurnCompleted`. One short model turn on the free model.
#[tokio::test]
#[ignore = "requires an installed and authenticated OpenCode ACP server; sends one live model turn"]
async fn live_opencode_cancel_mid_turn_reports_cancelled() {
    let session = OpenCodeAdapter::default()
        .start(&SessionRequest::new(std::env::current_dir().unwrap()))
        .await
        .unwrap();
    let observer = FirstDeltaObserver {
        events: Mutex::new(Vec::new()),
        first_delta: Notify::new(),
    };
    let request = TurnRequest::new(
        "Count slowly from 1 to 400, one number per line, with no other text. Do not use tools.",
    );
    let started = Instant::now();
    let cancel = async {
        let saw_delta = timeout(Duration::from_secs(20), observer.first_delta.notified())
            .await
            .is_ok();
        let cancel_at = started.elapsed();
        session.cancel().await.unwrap();
        (saw_delta, cancel_at)
    };
    let (outcome, (saw_delta, cancel_at)) = timeout(Duration::from_secs(60), async {
        tokio::join!(session.turn_observed(&request, &observer), cancel)
    })
    .await
    .expect("a cancelled live turn must return long before the 60 s bound");
    let events = outcome.unwrap();
    println!(
        "saw_delta={saw_delta} cancel_after={cancel_at:?} returned_after={:?} events={}",
        started.elapsed(),
        events.len()
    );
    println!("terminal event: {:?}", events.last());
    let seen = observer.events.lock().unwrap().clone();
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
    assert!(
        seen.iter()
            .any(|event| matches!(event, ProviderEvent::Cancelled { .. })),
        "the observer must see the Cancelled event"
    );
    session.stop().await.unwrap();
}
