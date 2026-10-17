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
    AdapterFuture, DiscoveryEnvironment, OpenCodeAdapter, PermissionDecision, PermissionKind,
    PermissionRequest, ProgressDeadline, ProviderAdapter, ProviderError, ProviderEvent,
    ProviderKind, ProviderSession, ProviderVerification, SessionRequest, TurnEventObserver,
    TurnRequest, normalize_acp_message,
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

const GOVERNED_BLOCK: &str = r#"{"*":"ask","read":{"*":"allow","*.env":"deny","*.env.*":"deny","*.env.example":"allow"},"glob":"allow","grep":"allow","list":"allow"}"#;

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
    assert_eq!(
        command.env.get(&OsString::from("OPENCODE_PERMISSION")),
        Some(&OsString::from(GOVERNED_BLOCK)),
        "a request with no permission mode gets the governed block, never the owner's own rules"
    );
}

#[test]
fn opencode_launch_is_prompt_free_only_under_full_autonomy() {
    let executable = Path::new(r"C:\Tools\opencode.exe");
    let key = OsString::from("OPENCODE_PERMISSION");

    // `auto` is the permission mode the daemon derives from full autonomy.
    let autonomous = OpenCodeAdapter::command(
        executable,
        &SessionRequest::new(r"C:\repo").with_permission_mode("auto"),
    );
    assert_eq!(autonomous.args, vec![OsString::from("acp")]);
    assert_eq!(
        autonomous.env.get(&key),
        Some(&OsString::from(r#"{"*":"allow"}"#))
    );
    assert_eq!(autonomous.env.len(), 1, "no other variable is injected");
    let parsed: serde_json::Value =
        serde_json::from_str(&autonomous.env[&key].to_string_lossy()).unwrap();
    assert_eq!(parsed, serde_json::json!({"*": "allow"}));

    // Every other mode, including none-recognized ones, gets the governed block (never the
    // owner's own rules, never allow-all).
    for mode in [
        "untrusted",
        "plan",
        "default",
        "dontAsk",
        "full-autonomy",
        "self-decision",
        "AUTO",
        "ASK",
        "Accept-Edits",
        " ask",
        "",
    ] {
        let command = OpenCodeAdapter::command(
            executable,
            &SessionRequest::new(r"C:\repo").with_permission_mode(mode),
        );
        assert_eq!(
            command.env.get(&key),
            Some(&OsString::from(GOVERNED_BLOCK)),
            "permission mode {mode:?} must get the governed block"
        );
        assert_eq!(command.args, vec![OsString::from("acp")]);
    }
}

#[test]
fn opencode_launch_makes_every_governed_tool_ask_under_ask_and_self_decision() {
    let executable = Path::new(r"C:\Tools\opencode.exe");
    let key = OsString::from("OPENCODE_PERMISSION");
    let expected = GOVERNED_BLOCK;

    // `ask` and `accept-edits` are the modes the daemon derives from `ask` and `self-decision`.
    for mode in ["ask", "accept-edits"] {
        let command = OpenCodeAdapter::command(
            executable,
            &SessionRequest::new(r"C:\repo").with_permission_mode(mode),
        );
        assert_eq!(command.args, vec![OsString::from("acp")]);
        assert_eq!(command.env.len(), 1, "no other variable is injected");
        assert_eq!(command.env.get(&key), Some(&OsString::from(expected)));
        let parsed: serde_json::Value =
            serde_json::from_str(&command.env[&key].to_string_lossy()).unwrap();
        let rules = parsed.as_object().unwrap();
        // The catch-all comes first (the last matching rule wins) and asks; only the read-only,
        // non-governed tools are allowed by name.
        assert_eq!(rules.keys().next().map(String::as_str), Some("*"));
        assert_eq!(rules["*"], "ask");
        let allowed = rules
            .iter()
            .filter(|(_, action)| *action == "allow")
            .map(|(tool, _)| tool.as_str())
            .collect::<Vec<_>>();
        assert_eq!(allowed, ["glob", "grep", "list"]);
        // `read` is the object form with OpenCode's documented env-file defaults, so it is
        // never the shorthand allow that would open `.env` files.
        assert_eq!(
            rules["read"],
            serde_json::json!({
                "*": "allow", "*.env": "deny", "*.env.*": "deny", "*.env.example": "allow"
            })
        );
        for governed in [
            "bash",
            "edit",
            "write",
            "webfetch",
            "websearch",
            "task",
            "external_directory",
            "todowrite",
        ] {
            assert!(
                !rules.contains_key(governed),
                "{governed} must stay under the catch-all ask"
            );
        }
    }
}

#[tokio::test]
async fn governed_sessions_refuse_a_project_configuration_that_can_override_the_block() {
    // The live precedence probe showed a project `opencode.json` allowing `bash` beating the
    // inline block, so a governed session fails closed before any process starts. The
    // prompt-free session has no governed rules to bypass and is unaffected.
    let temporary = tempdir().unwrap();
    std::fs::write(
        temporary.path().join("opencode.json"),
        r#"{"permission":{"bash":"allow"},"agent":{"build":{"permission":{"bash":"allow"}}}}"#,
    )
    .unwrap();
    for mode in [Some("ask"), Some("accept-edits"), Some("untrusted"), None] {
        let mut request = SessionRequest::new(temporary.path());
        if let Some(mode) = mode {
            request = request.with_permission_mode(mode);
        }
        let outcome = OpenCodeAdapter::with_executable(fake_acp())
            .start(&request)
            .await;
        assert!(
            matches!(outcome, Err(ProviderError::Unsupported { .. })),
            "mode {mode:?} must be refused beside a permission configuration"
        );
    }
    let session = OpenCodeAdapter::with_executable(fake_acp())
        .start(&SessionRequest::new(temporary.path()).with_permission_mode("auto"))
        .await
        .unwrap();
    session.stop().await.unwrap();

    std::fs::remove_file(temporary.path().join("opencode.json")).unwrap();
    std::fs::create_dir(temporary.path().join(".opencode")).unwrap();
    let outcome = OpenCodeAdapter::with_executable(fake_acp())
        .start(&SessionRequest::new(temporary.path()).with_permission_mode("ask"))
        .await;
    assert!(matches!(outcome, Err(ProviderError::Unsupported { .. })));
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
    let events = turn.await.unwrap();
    assert_eq!(observer.decisions.load(Ordering::SeqCst), 1);
    assert_eq!(
        response(temporary.path(), "fake-acp-response-2.json")["result"]["outcome"]["outcome"],
        "cancelled"
    );
    // The reused tool call id is refused with the same recorded `permissionRefused` payload.
    let refusal = refusal_payloads(&events);
    assert_eq!(refusal.len(), 1, "{events:?}");
    assert_eq!(refusal[0]["permissionRefused"]["toolCallId"], "call-1");
    assert!(
        refusal[0]["permissionRefused"]["reason"]
            .as_str()
            .unwrap()
            .contains("reused a tool call id")
    );
    session.stop().await.unwrap();
}

#[tokio::test]
async fn observed_acp_records_a_reused_jsonrpc_id_and_refusals_are_not_progress() {
    // A reused JSON-RPC id is refused and recorded too.
    let temporary = tempdir().unwrap();
    let session = OpenCodeAdapter::with_executable(fake_acp())
        .start(&SessionRequest::new(temporary.path()))
        .await
        .unwrap();
    let observer = GateObserver::approving();
    let request = TurnRequest::new("DUPLICATE");
    let turn = session.turn_observed(&request, &observer);
    tokio::pin!(turn);
    timeout(Duration::from_secs(5), async {
        tokio::select! {
            () = observer.entered.notified() => {},
            result = &mut turn => panic!("turn completed early: {result:?}"),
        }
    })
    .await
    .unwrap();
    observer.release.notify_one();
    let events = turn.await.unwrap();
    let refusal = refusal_payloads(&events);
    assert_eq!(refusal.len(), 1, "{events:?}");
    assert!(
        refusal[0]["permissionRefused"]["reason"]
            .as_str()
            .unwrap()
            .contains("reused a JSON-RPC request id")
    );
    session.stop().await.unwrap();

    // A model that keeps asking for a refused tool never counts as making progress: the
    // progress deadline still fires while the refusals are being recorded.
    let temporary = tempdir().unwrap();
    let mut request = SessionRequest::new(temporary.path());
    request.progress_timeout_millis = 1_500;
    let session = OpenCodeAdapter::with_executable(fake_acp())
        .start(&request)
        .await
        .unwrap();
    let observer = GateObserver::approving();
    let outcome = timeout(
        Duration::from_secs(20),
        session.turn_observed(&TurnRequest::new("LOOP_REFUSED"), &observer),
    )
    .await
    .unwrap();
    assert!(
        matches!(outcome, Err(ProviderError::Stalled { millis: 1_500 })),
        "{outcome:?}"
    );
    assert_eq!(observer.decisions.load(Ordering::SeqCst), 0);
    assert!(
        observer
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, ProviderEvent::Progress { raw: Some(raw), .. } if raw.get("permissionRefused").is_some())),
        "the refusals were observed"
    );
    let _ = session.stop().await;
}

fn refusal_payloads(events: &[ProviderEvent]) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Progress { raw: Some(raw), .. }
                if raw.get("permissionRefused").is_some() =>
            {
                Some(raw.clone())
            }
            _ => None,
        })
        .collect()
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
async fn observed_acp_admits_command_edit_delete_and_move_as_governed_requests() {
    for (prompt, expected_kind, expected_action) in [
        (
            "KIND_EXECUTE",
            PermissionKind::CommandExecution,
            "execute an ACP command",
        ),
        ("edit-kind", PermissionKind::FileChange, "edit ACP files"),
        (
            "KIND_DELETE",
            PermissionKind::FileChange,
            "delete ACP files",
        ),
        ("KIND_MOVE", PermissionKind::FileChange, "move ACP files"),
    ] {
        let temporary = tempdir().unwrap();
        let session = OpenCodeAdapter::with_executable(fake_acp())
            .start(&SessionRequest::new(temporary.path()))
            .await
            .unwrap();
        let observer = GateObserver::approving();
        let request = TurnRequest::new(prompt);
        let turn = session.turn_observed(&request, &observer);
        tokio::pin!(turn);
        timeout(Duration::from_secs(5), async {
            tokio::select! {
                () = observer.entered.notified() => {},
                result = &mut turn => panic!("turn completed before approval gate: {result:?}"),
            }
        })
        .await
        .unwrap();
        observer.release.notify_one();
        let events = timeout(Duration::from_secs(5), &mut turn)
            .await
            .unwrap()
            .unwrap();
        let requests = observer.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1, "{prompt}");
        assert_eq!(requests[0].kind, expected_kind, "{prompt}");
        assert_eq!(requests[0].action, expected_action, "{prompt}");
        assert!(requests[0].scope_expansion, "{prompt}");
        let wire = response(temporary.path(), "fake-acp-response-1.json");
        assert_eq!(wire["result"]["outcome"]["outcome"], "selected", "{prompt}");
        assert_eq!(
            wire["result"]["outcome"]["optionId"], "allow-one",
            "{prompt}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ProviderEvent::PermissionRequested { .. })),
            "{prompt}"
        );
        session.stop().await.unwrap();
    }
}

#[tokio::test]
async fn observed_acp_refuses_every_other_kind_with_a_cancel_and_a_recorded_reason() {
    for (prompt, kind) in [
        ("KIND_READ", "read"),
        ("KIND_SEARCH", "search"),
        ("KIND_THINK", "think"),
        ("KIND_FETCH", "fetch"),
        ("KIND_OTHER", "other"),
        ("UNKNOWN_KIND", "network"),
    ] {
        let temporary = tempdir().unwrap();
        let session = OpenCodeAdapter::with_executable(fake_acp())
            .start(&SessionRequest::new(temporary.path()).with_permission_mode("ask"))
            .await
            .unwrap();
        let observer = GateObserver::approving();
        let events = session
            .turn_observed(&TurnRequest::new(prompt), &observer)
            .await
            .unwrap();

        // The observer is never asked, so no approval can exist for the request.
        assert_eq!(observer.decisions.load(Ordering::SeqCst), 0, "{kind}");
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, ProviderEvent::PermissionRequested { .. })),
            "{kind}"
        );
        // It is answered with a one-shot cancel, never a selected option (so never a grant).
        let wire = response(temporary.path(), "fake-acp-response-1.json");
        assert_eq!(wire["result"]["outcome"]["outcome"], "cancelled", "{kind}");
        assert!(
            wire["result"]["outcome"].get("optionId").is_none(),
            "{kind}"
        );
        // The refusal and its reason are recorded in the event's raw payload.
        let refusal = events
            .iter()
            .find_map(|event| match event {
                ProviderEvent::Progress {
                    message: Some(message),
                    raw: Some(raw),
                    ..
                } if message.starts_with("ACP permission request refused") => Some(raw.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{kind}: the refusal was not recorded: {events:?}"));
        assert_eq!(refusal["permissionRefused"]["toolKind"], kind);
        assert_eq!(refusal["permissionRefused"]["answer"], "cancelled");
        assert_eq!(refusal["permissionRefused"]["toolCallId"], "call-1");
        assert!(
            refusal["permissionRefused"]["reason"]
                .as_str()
                .unwrap()
                .contains("unsupported tool kind"),
            "{kind}"
        );
        // The title is not durable (it can carry secrets) and is not in the record.
        assert!(
            !refusal.to_string().contains("Write fixture file"),
            "{kind}"
        );
        session.stop().await.unwrap();
    }
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
