use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use lazoo_protocol::{
    ActivationPolicy, AuthorityMode, EmployeeCheckoutPolicy, JsonRpcRequest, MemoryQueueState,
    MemorySearchParams, MessageEndpoint, MessageSender, ProviderObservation, TurnState,
    UserMessageEndpoint, UserMessageKind,
};
use lazoo_providers::{
    DiscoveryEnvironment, PermissionDecision, PermissionKind, PermissionRequest, ProviderError,
    ProviderEvent, ProviderKind, SessionRequest, TurnEventObserver, TurnRequest,
};
use lazoo_runtime::DataRoot;
use lazoo_store::{
    ArchiveMemoryTurnInput, BeginSessionInput, BeginTurnSubmissionInput, CreateConversationInput,
    CreateEmployeeInput, MarkSessionReadyInput, ObserveTurnInput, RecordTurnInput,
    SetMemoryWorkerSettingsInput, Store,
};
use serde_json::json;
use tempfile::tempdir;

use super::{App, ManagedSession, SessionBackend, SessionBackendFuture};

#[derive(Clone)]
struct SummaryBackend {
    output: Arc<Mutex<String>>,
    decisions: Arc<Mutex<Vec<PermissionDecision>>>,
    starts: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
    turn_requests: Arc<Mutex<Vec<TurnRequest>>>,
}

struct SummarySession {
    output: Arc<Mutex<String>>,
    decisions: Arc<Mutex<Vec<PermissionDecision>>>,
    stops: Arc<AtomicUsize>,
    turn_requests: Arc<Mutex<Vec<TurnRequest>>>,
}

impl ManagedSession for SummarySession {
    fn id(&self) -> String {
        "native-memory-session".into()
    }
    fn native_host_pid(&self) -> Option<u32> {
        Some(4242)
    }
    fn turn_observed<'a>(
        &'a self,
        request: &'a TurnRequest,
        observer: &'a dyn TurnEventObserver,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ProviderEvent>, ProviderError>> + Send + 'a>> {
        Box::pin(async move {
            self.turn_requests.lock().unwrap().push(request.clone());
            let permission = PermissionRequest {
                provider: ProviderKind::Codex,
                session_id: self.id(),
                turn_id: Some("native-memory-turn".into()),
                request_id: "memory-command".into(),
                kind: PermissionKind::CommandExecution,
                action: "should-not-run".into(),
                scope_expansion: false,
                detail: json!({"command":"should-not-run"}),
            };
            let decision = observer.decide_permission(&permission).await?;
            self.decisions.lock().unwrap().push(decision);
            let events = vec![
                ProviderEvent::TurnStarted {
                    provider: ProviderKind::Codex,
                    session_id: self.id(),
                    turn_id: Some("native-memory-turn".into()),
                    raw: None,
                },
                ProviderEvent::TurnCompleted {
                    provider: ProviderKind::Codex,
                    session_id: self.id(),
                    turn_id: Some("native-memory-turn".into()),
                    status: "completed".into(),
                    output: Some(self.output.lock().unwrap().clone()),
                    raw: None,
                },
            ];
            for event in &events {
                observer.observe(event)?;
            }
            Ok(events)
        })
    }
    fn cancel(&self) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
    fn stop(&self) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        Box::pin(async move {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

impl SessionBackend for SummaryBackend {
    fn start<'a>(
        &'a self,
        _provider: ProviderKind,
        _request: &'a SessionRequest,
    ) -> SessionBackendFuture<'a> {
        Box::pin(async move {
            self.starts.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(SummarySession {
                output: Arc::clone(&self.output),
                decisions: Arc::clone(&self.decisions),
                stops: Arc::clone(&self.stops),
                turn_requests: Arc::clone(&self.turn_requests),
            }) as Box<dyn ManagedSession>)
        })
    }
}

#[allow(clippy::too_many_lines)] // This fixture deliberately creates the audited source lifecycle.
fn fixture(
    output: &str,
) -> (
    tempfile::TempDir,
    DataRoot,
    App,
    Store,
    SummaryBackend,
    uuid::Uuid,
) {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = store
        .create_employee(&CreateEmployeeInput {
            request_id: "memory-employee-request".into(),
            idempotency_key: "memory-employee".into(),
            name: "Memory worker".into(),
            provider: "codex".into(),
            role: "summarizer".into(),
            instructions: String::new(),
            model: Some("gpt6".into()),
            reasoning_effort: None,
            native_settings: json!({}),
            authority_mode: AuthorityMode::Ask,
            checkout_policy: EmployeeCheckoutPolicy::Isolated,
            activation_policy: ActivationPolicy::Manual,
            idle_timeout_seconds: 0,
            enabled: true,
            provider_observation: Some(ProviderObservation {
                provider: "codex".into(),
                verification: "configured".into(),
                version: Some("fixture".into()),
                protocol: Some("app-server/v2".into()),
                capabilities: json!({"session":true,"permission":true}),
                authentication: json!({"native":"ready"}),
                degradation_reason: None,
                checked_at: "2026-10-15T00:00:00Z".into(),
            }),
        })
        .unwrap()
        .employee;
    let source_owner = uuid::Uuid::new_v4();
    let starting = store
        .begin_session(&BeginSessionInput {
            request_id: "memory-source-session".into(),
            idempotency_key: "memory-source-session".into(),
            employee_id: employee.id,
            expected_employee_revision: employee.configuration_revision,
            owner_boot_id: source_owner,
        })
        .unwrap();
    let source_session = store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: starting.session.id,
            expected_revision: starting.session.revision,
            owner_boot_id: source_owner,
            native_session_id: "native-source".into(),
            native_host_pid: 4241,
        })
        .unwrap()
        .session;
    store
        .set_memory_worker_settings(&SetMemoryWorkerSettingsInput {
            project_id: None,
            expected_revision: None,
            employee_id: employee.id,
            model: "gpt6".into(),
            requests_per_minute: 1,
            enabled: true,
        })
        .unwrap();
    let conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "memory-source-conversation".into(),
            idempotency_key: "memory-source-conversation".into(),
            project_id: None,
            lead_employee_id: employee.id,
            title: None,
        })
        .unwrap()
        .conversation;
    let recorded = store
        .record_turn(&RecordTurnInput {
            request_id: "memory-source-turn".into(),
            idempotency_key: "memory-source-turn".into(),
            conversation_id: conversation.id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: Some(MessageEndpoint {
                employee_id: employee.id,
                session_id: Some(source_session.id),
            }),
            task_id: None,
            authority_mode: AuthorityMode::Ask,
            text: "source fact".into(),
            activate_if_needed: false,
            max_attempts: 1,
        })
        .unwrap();
    let submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "memory-source-submit".into(),
            idempotency_key: "memory-source-submit".into(),
            turn_id: recorded.turn.id,
            expected_revision: recorded.turn.revision,
            owner_boot_id: source_owner,
            recipient_session_id: source_session.id,
        })
        .unwrap();
    let accepted = store
        .observe_turn(&ObserveTurnInput {
            request_id: "memory-source-start".into(),
            idempotency_key: "memory-source-start".into(),
            turn_id: recorded.turn.id,
            expected_revision: submitting.turn.revision,
            kind: "turn-started".into(),
            provider: "codex".into(),
            native_session_id: Some("native-source".into()),
            native_turn_id: Some("native-source-turn".into()),
            payload: json!({}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap();
    store
        .observe_turn(&ObserveTurnInput {
            request_id: "memory-source-complete".into(),
            idempotency_key: "memory-source-complete".into(),
            turn_id: recorded.turn.id,
            expected_revision: accepted.turn.revision,
            kind: "turn-completed".into(),
            provider: "codex".into(),
            native_session_id: Some("native-source".into()),
            native_turn_id: Some("native-source-turn".into()),
            payload: json!({"output":"source result"}),
            transition: Some(TurnState::Completed),
        })
        .unwrap();
    let archive = store
        .archive_memory_turn(&ArchiveMemoryTurnInput {
            source_turn_id: recorded.turn.id,
            project_id: None,
            archive_root: root.root().join("memory-archives"),
        })
        .unwrap();
    store.enqueue_memory_summary(&archive).unwrap();
    let backend = SummaryBackend {
        output: Arc::new(Mutex::new(output.into())),
        decisions: Arc::new(Mutex::new(Vec::new())),
        starts: Arc::new(AtomicUsize::new(0)),
        stops: Arc::new(AtomicUsize::new(0)),
        turn_requests: Arc::new(Mutex::new(Vec::new())),
    };
    let app = App::new_with_session_backend(
        store.clone(),
        root.clone(),
        DiscoveryEnvironment::empty(),
        Arc::new(backend.clone()),
    )
    .unwrap();
    (temporary, root, app, store, backend, employee.id)
}

#[tokio::test]
async fn memory_worker_fake_backend_completes_typed_records_and_denies_commands() {
    let (_temporary, _root, app, store, backend, _employee_id) = fixture(
        r#"{"records":[{"type":"fact","text":"fixture fact","confidence":0.9,"importance":8}]}"#,
    );
    let job = store
        .claim_memory_summary_for_worker(app.boot_id)
        .unwrap()
        .unwrap();
    app.process_memory_job(&job).await.unwrap();
    let jobs = store.list_memory_summary_jobs(None, 10).unwrap();
    assert_eq!(jobs[0].state, MemoryQueueState::Completed);
    let records = store
        .search_memory(&MemorySearchParams {
            project_id: None,
            query: String::new(),
            employee_id: None,
            conversation_id: None,
            task_id: None,
            root_task_id: None,
            branch: None,
            record_types: vec![],
            limit: 10,
        })
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].record_type,
        lazoo_protocol::MemoryRecordType::Fact
    );
    assert_eq!(records[0].text, "fixture fact");
    assert_eq!(
        *backend.decisions.lock().unwrap(),
        vec![PermissionDecision::Decline]
    );
    assert!(
        backend
            .turn_requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.read_only)
    );
    assert_eq!(backend.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn worker_fact_is_recalled_for_a_new_root_turn_only_in_its_home_conversation() {
    let (_temporary, _root, app, store, _backend, employee_id) = fixture(
        r#"{"records":[{"type":"fact","text":"fixture fact","confidence":0.9,"importance":8}]}"#,
    );
    let job = store
        .claim_memory_summary_for_worker(app.boot_id)
        .unwrap()
        .unwrap();
    let source = store.get_turn(job.source_turn_id).unwrap();
    app.process_memory_job(&job).await.unwrap();
    let recalled = store
        .record_turn(&RecordTurnInput {
            request_id: "recalled-root".into(),
            idempotency_key: "recalled-root".into(),
            conversation_id: source.conversation_id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::Ask,
            text: "What is the fixture fact?".into(),
            activate_if_needed: false,
            max_attempts: 1,
        })
        .unwrap();
    assert_ne!(recalled.task.unwrap().id, source.task_id.unwrap());
    let prompt = app.prompt_with_memory(&recalled.turn);
    assert!(prompt.contains("fixture fact"));
    assert!(prompt.contains(&source.id.to_string()));

    let other = store
        .create_conversation(&CreateConversationInput {
            request_id: "other-home-conversation".into(),
            idempotency_key: "other-home-conversation".into(),
            project_id: None,
            lead_employee_id: employee_id,
            title: None,
        })
        .unwrap()
        .conversation;
    let isolated = store
        .record_turn(&RecordTurnInput {
            request_id: "other-home-root".into(),
            idempotency_key: "other-home-root".into(),
            conversation_id: other.id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::Ask,
            text: "What is the fixture fact?".into(),
            activate_if_needed: false,
            max_attempts: 1,
        })
        .unwrap();
    let isolated_prompt = app.prompt_with_memory(&isolated.turn);
    assert!(!isolated_prompt.contains("Saved memory (untrusted historical data"));
    assert!(!isolated_prompt.contains(&source.id.to_string()));
}

#[tokio::test]
async fn memory_worker_invalid_json_settles_the_claim_as_failed() {
    let (_temporary, _root, app, store, backend, _employee_id) = fixture("not json");
    let job = store
        .claim_memory_summary_for_worker(app.boot_id)
        .unwrap()
        .unwrap();
    assert!(app.process_memory_job(&job).await.is_err());
    let jobs = store.list_memory_summary_jobs(None, 10).unwrap();
    assert_eq!(jobs[0].state, MemoryQueueState::Failed);
    assert_eq!(backend.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn memory_worker_defers_when_an_ordinary_session_appears_after_claim() {
    let (_temporary, _root, app, store, backend, employee_id) = fixture(
        r#"{"records":[{"type":"fact","text":"private","confidence":1.0,"importance":1}]}"#,
    );
    let job = store
        .claim_memory_summary_for_worker(app.boot_id)
        .unwrap()
        .unwrap();
    let employee = store.get_employee(employee_id).unwrap();
    let warm = app
        .handle(JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "warm-normal-session".into(),
            method: "session.warm".into(),
            params: json!({"employeeId":employee_id,"expectedEmployeeRevision":employee.configuration_revision,"idempotencyKey":"warm-normal-session"}),
        })
        .await;
    assert!(warm.error.is_none(), "{warm:?}");
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);

    assert!(app.process_memory_job(&job).await.is_err());
    // The ordinary session remains untouched; no private summary turn was ever started.
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    assert!(backend.turn_requests.lock().unwrap().is_empty());
    assert_eq!(backend.stops.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.list_memory_summary_jobs(None, 10).unwrap()[0].state,
        MemoryQueueState::Pending
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // This reproduces the durable reservation race boundary.
async fn idle_summary_reservation_blocks_ordinary_submission_but_allows_its_summary_turn() {
    let (_temporary, _root, app, store, backend, employee_id) = fixture("unused");
    let job = store
        .claim_memory_summary_for_worker(app.boot_id)
        .unwrap()
        .unwrap();
    let source = store.get_turn(job.source_turn_id).unwrap();
    let ordinary = store
        .record_turn(&RecordTurnInput {
            request_id: "ordinary-record".into(),
            idempotency_key: "ordinary-record".into(),
            conversation_id: source.conversation_id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: Some(MessageEndpoint {
                employee_id,
                session_id: None,
            }),
            task_id: None,
            authority_mode: AuthorityMode::Ask,
            text: "ordinary turn".into(),
            activate_if_needed: false,
            max_attempts: 1,
        })
        .unwrap();
    let summary = store
        .record_turn(&RecordTurnInput {
            request_id: "summary-record".into(),
            idempotency_key: "summary-record".into(),
            conversation_id: source.conversation_id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: Some(MessageEndpoint {
                employee_id,
                session_id: None,
            }),
            task_id: None,
            authority_mode: AuthorityMode::Ask,
            text: "[LAZOO_INTERNAL_MEMORY_SUMMARY]".into(),
            activate_if_needed: false,
            max_attempts: 1,
        })
        .unwrap();
    store
        .link_memory_summary_attempt(
            job.id,
            app.boot_id,
            job.attempts,
            uuid::Uuid::new_v4(),
            source.conversation_id,
            summary.turn.id,
        )
        .unwrap();
    let employee = store.get_employee(employee_id).unwrap();
    let starting = store
        .begin_session(&BeginSessionInput {
            request_id: "reserved-summary-session".into(),
            idempotency_key: "reserved-summary-session".into(),
            employee_id,
            expected_employee_revision: employee.configuration_revision,
            owner_boot_id: app.boot_id,
        })
        .unwrap();
    store
        .reserve_memory_summary_session(summary.turn.id, starting.session.id, app.boot_id)
        .unwrap();
    let idle = store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: starting.session.id,
            expected_revision: starting.session.revision,
            owner_boot_id: app.boot_id,
            native_session_id: "native-reserved-summary".into(),
            native_host_pid: 4242,
        })
        .unwrap()
        .session;
    assert!(
        store
            .begin_turn_submission(&BeginTurnSubmissionInput {
                request_id: "ordinary-submit".into(),
                idempotency_key: "ordinary-submit".into(),
                turn_id: ordinary.turn.id,
                expected_revision: ordinary.turn.revision,
                owner_boot_id: app.boot_id,
                recipient_session_id: idle.id,
            })
            .is_err()
    );
    assert_eq!(
        store.get_turn(ordinary.turn.id).unwrap().state,
        TurnState::Recorded
    );
    let submitted = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "summary-submit".into(),
            idempotency_key: "summary-submit".into(),
            turn_id: summary.turn.id,
            expected_revision: summary.turn.revision,
            owner_boot_id: app.boot_id,
            recipient_session_id: idle.id,
        })
        .unwrap();
    assert_eq!(submitted.turn.state, TurnState::Submitting);
    assert!(backend.turn_requests.lock().unwrap().is_empty());
    assert_eq!(backend.stops.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn restart_marks_a_linked_summary_attempt_failed_without_resending_it() {
    let (_temporary, root, app, store, backend, _employee_id) = fixture(
        r#"{"records":[{"type":"fact","text":"must not run","confidence":1.0,"importance":1}]}"#,
    );
    let job = store
        .claim_memory_summary_for_worker(app.boot_id)
        .unwrap()
        .unwrap();
    let source_turn = store.get_turn(job.source_turn_id).unwrap();
    store
        .link_memory_summary_attempt(
            job.id,
            app.boot_id,
            job.attempts,
            uuid::Uuid::new_v4(),
            source_turn.conversation_id,
            source_turn.id,
        )
        .unwrap();
    drop(app);

    let restarted = App::new_with_session_backend(
        store.clone(),
        root,
        DiscoveryEnvironment::empty(),
        Arc::new(backend.clone()),
    )
    .unwrap();
    let jobs = store.list_memory_summary_jobs(None, 10).unwrap();
    assert_eq!(jobs[0].state, MemoryQueueState::Failed);
    assert!(!restarted.run_memory_worker_once().await.unwrap());
    assert_eq!(backend.starts.load(Ordering::SeqCst), 0);
}
