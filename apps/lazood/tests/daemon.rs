use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use lazoo_protocol::{
    AuthorityMode, CheckoutMode, DesktopSnapshot, EmployeeCheckoutPolicy, JsonRpcRequest,
    MessageDeliveryState, ProviderObservation, TurnState,
};
use lazoo_providers::{
    DiscoveryEnvironment, ProviderError, ProviderEvent, ProviderKind, SessionRequest,
    TurnEventObserver, TurnRequest,
};
use lazoo_runtime::DataRoot;
use lazoo_store::{CreateConversationInput, CreateEmployeeInput, Store};
use lazood::{App, ManagedSession, SessionBackend, SessionBackendFuture};
use serde_json::json;
use tempfile::tempdir;
use tokio::{sync::Semaphore, time::timeout};

fn git_init(path: &Path) {
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

fn request(id: &str, method: &str, params: serde_json::Value) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: id.to_owned(),
        method: method.to_owned(),
        params,
    }
}

fn test_app() -> (tempfile::TempDir, App) {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    (
        temporary,
        App::new_with_provider_environment(store, root, DiscoveryEnvironment::empty()).unwrap(),
    )
}

#[derive(Clone)]
struct FakeSessionBackend {
    starts: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
    turns: Arc<AtomicUsize>,
    cancels: Arc<AtomicUsize>,
    gate_events: Arc<AtomicBool>,
    event_permits: Arc<Semaphore>,
    turn_events: Arc<Mutex<Vec<ProviderEvent>>>,
    turn_requests: Arc<Mutex<Vec<TurnRequest>>>,
    turn_failure: Arc<Mutex<Option<String>>>,
    turn_retryable_failure: Arc<AtomicBool>,
    working_directories: Arc<Mutex<Vec<PathBuf>>>,
}

impl Default for FakeSessionBackend {
    fn default() -> Self {
        Self {
            starts: Arc::new(AtomicUsize::new(0)),
            stops: Arc::new(AtomicUsize::new(0)),
            turns: Arc::new(AtomicUsize::new(0)),
            cancels: Arc::new(AtomicUsize::new(0)),
            gate_events: Arc::new(AtomicBool::new(false)),
            event_permits: Arc::new(Semaphore::new(0)),
            turn_events: Arc::new(Mutex::new(Vec::new())),
            turn_requests: Arc::new(Mutex::new(Vec::new())),
            turn_failure: Arc::new(Mutex::new(None)),
            turn_retryable_failure: Arc::new(AtomicBool::new(false)),
            working_directories: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

struct FakeManagedSession {
    id: String,
    stops: Arc<AtomicUsize>,
    turns: Arc<AtomicUsize>,
    cancels: Arc<AtomicUsize>,
    gate_events: Arc<AtomicBool>,
    event_permits: Arc<Semaphore>,
    turn_events: Arc<Mutex<Vec<ProviderEvent>>>,
    turn_requests: Arc<Mutex<Vec<TurnRequest>>>,
    turn_failure: Arc<Mutex<Option<String>>>,
    turn_retryable_failure: Arc<AtomicBool>,
}

impl ManagedSession for FakeManagedSession {
    fn id(&self) -> String {
        self.id.clone()
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
            self.turns.fetch_add(1, Ordering::SeqCst);
            self.turn_requests.lock().unwrap().push(request.clone());
            if self.turn_retryable_failure.load(Ordering::SeqCst) {
                return Err(ProviderError::Timeout { phase: "fake turn" });
            }
            let failure = self.turn_failure.lock().unwrap().clone();
            if let Some(message) = failure {
                return Err(ProviderError::Protocol(message));
            }
            let scripted = self.turn_events.lock().unwrap().clone();
            let mut returned = Vec::with_capacity(scripted.len());
            for event in scripted {
                if self.gate_events.load(Ordering::SeqCst) {
                    self.event_permits.acquire().await.unwrap().forget();
                }
                observer.observe(&event)?;
                returned.push(event);
            }
            Ok(returned)
        })
    }

    fn cancel(&self) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        Box::pin(async move {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn stop(&self) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        Box::pin(async move {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

impl SessionBackend for FakeSessionBackend {
    fn start<'a>(
        &'a self,
        _provider: ProviderKind,
        request: &'a SessionRequest,
    ) -> SessionBackendFuture<'a> {
        Box::pin(async move {
            let number = self.starts.fetch_add(1, Ordering::SeqCst) + 1;
            self.working_directories
                .lock()
                .unwrap()
                .push(request.cwd.clone());
            Ok(Box::new(FakeManagedSession {
                id: format!("native-{number}"),
                stops: Arc::clone(&self.stops),
                turns: Arc::clone(&self.turns),
                cancels: Arc::clone(&self.cancels),
                gate_events: Arc::clone(&self.gate_events),
                event_permits: Arc::clone(&self.event_permits),
                turn_events: Arc::clone(&self.turn_events),
                turn_requests: Arc::clone(&self.turn_requests),
                turn_failure: Arc::clone(&self.turn_failure),
                turn_retryable_failure: Arc::clone(&self.turn_retryable_failure),
            }) as Box<dyn ManagedSession>)
        })
    }
}

struct FailingSessionBackend;

impl SessionBackend for FailingSessionBackend {
    fn start<'a>(
        &'a self,
        _provider: ProviderKind,
        _request: &'a SessionRequest,
    ) -> SessionBackendFuture<'a> {
        Box::pin(async {
            Err(ProviderError::Protocol(
                "private provider diagnostic".to_owned(),
            ))
        })
    }
}

fn configured_employee(store: &Store) -> lazoo_protocol::EmployeeRecord {
    configured_employee_named(store, "Rhea", "create-rhea")
}

fn configured_employee_named(
    store: &Store,
    name: &str,
    idempotency_key: &str,
) -> lazoo_protocol::EmployeeRecord {
    store
        .create_employee(&CreateEmployeeInput {
            request_id: format!("{idempotency_key}-request"),
            idempotency_key: idempotency_key.to_owned(),
            name: name.to_owned(),
            provider: "codex".to_owned(),
            role: "Lead".to_owned(),
            instructions: String::new(),
            model: Some("test-model".to_owned()),
            reasoning_effort: Some("low".to_owned()),
            native_settings: json!({}),
            authority_mode: AuthorityMode::Ask,
            checkout_policy: EmployeeCheckoutPolicy::Isolated,
            enabled: true,
            provider_observation: Some(ProviderObservation {
                provider: "codex".to_owned(),
                verification: "configured".to_owned(),
                version: Some("test".to_owned()),
                protocol: Some("app-server/v2".to_owned()),
                capabilities: json!({"session": true}),
                authentication: json!({"native": "ready", "external": "unknown"}),
                degradation_reason: None,
                checked_at: "2026-08-31T00:00:00Z".to_owned(),
            }),
        })
        .unwrap()
        .employee
}

fn message_test_app() -> (
    tempfile::TempDir,
    DataRoot,
    Store,
    App,
    uuid::Uuid,
    uuid::Uuid,
) {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let sender = configured_employee_named(&store, "Rhea", "create-rhea");
    let recipient = configured_employee_named(&store, "Mira", "create-mira");
    let app = App::new_with_provider_environment(
        store.clone(),
        root.clone(),
        DiscoveryEnvironment::empty(),
    )
    .unwrap();
    (temporary, root, store, app, sender.id, recipient.id)
}

fn session_test_app() -> (tempfile::TempDir, App, FakeSessionBackend, uuid::Uuid) {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = configured_employee(&store);
    let backend = FakeSessionBackend::default();
    let app = App::new_with_session_backend(
        store,
        root,
        DiscoveryEnvironment::empty(),
        Arc::new(backend.clone()),
    )
    .unwrap();
    (temporary, app, backend, employee.id)
}

fn turn_test_app() -> (
    tempfile::TempDir,
    DataRoot,
    Store,
    App,
    FakeSessionBackend,
    uuid::Uuid,
    uuid::Uuid,
) {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = configured_employee(&store);
    let conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "create-conversation-request".to_owned(),
            idempotency_key: "create-conversation".to_owned(),
            project_id: None,
            lead_employee_id: employee.id,
            title: Some("Delivery fixture".to_owned()),
        })
        .unwrap()
        .conversation;
    let backend = FakeSessionBackend::default();
    let app = App::new_with_session_backend(
        store.clone(),
        root.clone(),
        DiscoveryEnvironment::empty(),
        Arc::new(backend.clone()),
    )
    .unwrap();
    (
        temporary,
        root,
        store,
        app,
        backend,
        employee.id,
        conversation.id,
    )
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    timeout(Duration::from_secs(2), async {
        loop {
            if predicate() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("condition should become true");
}

async fn assert_desktop_is_roster_neutral(app: &App) {
    let desktop = app
        .handle(request("desktop", "desktop.snapshot", json!({})))
        .await
        .result
        .unwrap();
    assert!(desktop.get("employees").is_none());
    assert!(desktop.get("roster").is_none());
    assert!(desktop.get("providers").is_none());
    assert!(desktop.get("messages").is_none());
    assert!(desktop.get("collaboration").is_none());
    assert!(desktop.get("conversations").is_none());
    assert!(desktop.get("turns").is_none());
}

#[tokio::test]
async fn conversation_create_list_and_show_are_idempotent_and_backend_only() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = configured_employee(&store);
    let app =
        App::new_with_provider_environment(store, root, DiscoveryEnvironment::empty()).unwrap();

    let create_params = json!({
        "projectId": null,
        "leadEmployeeId": employee.id,
        "title": "  Provider delivery  ",
        "idempotencyKey": "create-conversation"
    });
    let created = app
        .handle(request(
            "conversation-create-1",
            "conversation.create",
            create_params.clone(),
        ))
        .await
        .result
        .unwrap();
    let replay = app
        .handle(request(
            "conversation-create-2",
            "conversation.create",
            create_params,
        ))
        .await
        .result
        .unwrap();

    assert_eq!(created["conversation"]["id"], replay["conversation"]["id"]);
    assert_eq!(created["conversation"]["title"], "Provider delivery");
    assert!(created["eventSequence"].is_number());
    assert!(replay["eventSequence"].is_null());

    let conversation_id = created["conversation"]["id"].as_str().unwrap();
    let listed = app
        .handle(request(
            "conversation-list",
            "conversation.list",
            json!({"afterSequence": 0, "limit": 100}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], conversation_id);

    let shown = app
        .handle(request(
            "conversation-show",
            "conversation.show",
            json!({"id": conversation_id}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(shown["conversation"]["id"], conversation_id);
    assert_eq!(shown["turns"], json!([]));
    assert_desktop_is_roster_neutral(&app).await;
}

#[tokio::test]
async fn conversation_create_supports_project_scope_and_stable_errors() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = configured_employee(&store);
    let app =
        App::new_with_provider_environment(store, root, DiscoveryEnvironment::empty()).unwrap();

    let repository = temporary.path().join("repository");
    git_init(&repository);
    let registered = app
        .handle(request(
            "project-register",
            "project.register",
            json!({
                "path": repository,
                "displayName": "Fixture",
                "checkoutMode": "isolated",
                "idempotencyKey": "register-fixture"
            }),
        ))
        .await
        .result
        .unwrap();
    let project_id = registered["project"]["id"].as_str().unwrap();
    let scoped = app
        .handle(request(
            "conversation-create",
            "conversation.create",
            json!({
                "projectId": project_id,
                "leadEmployeeId": employee.id,
                "title": null,
                "idempotencyKey": "create-scoped-conversation"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(scoped["conversation"]["projectId"], project_id);

    let missing_lead = app
        .handle(request(
            "conversation-missing-lead",
            "conversation.create",
            json!({
                "projectId": null,
                "leadEmployeeId": uuid::Uuid::new_v4(),
                "title": null,
                "idempotencyKey": "missing-lead"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(missing_lead.code, "LZ-CONVERSATION-LEAD-NOT-FOUND");

    let invalid_limit = app
        .handle(request(
            "conversation-invalid-limit",
            "conversation.list",
            json!({"afterSequence": 0, "limit": 0}),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(invalid_limit.code, "LZ-CONVERSATION-INVALID");
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Keep the durable pre/post-acceptance checkpoints together.
async fn turn_submit_persists_before_provider_acceptance_and_acknowledges_on_evidence() {
    let (_temporary, root, _store, app, backend, employee_id, conversation_id) = turn_test_app();
    backend.gate_events.store(true, Ordering::SeqCst);
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("native-turn-1".to_owned()),
            raw: Some(json!({"private": "discard"})),
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("native-turn-1".to_owned()),
            status: "completed".to_owned(),
            output: Some("done".to_owned()),
            raw: None,
        },
    ]);
    app.handle(request(
        "warm-turn-session",
        "session.warm",
        json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-turn-session"
        }),
    ))
    .await
    .result
    .unwrap();

    let submit_params = json!({
        "conversationId": conversation_id,
        "author": {"kind": "user"},
        "recipient": null,
        "text": "Reply with done.",
        "activateIfNeeded": false,
        "maxAttempts": 3,
        "idempotencyKey": "submit-turn"
    });
    let pending = tokio::spawn({
        let app = app.clone();
        let submit_params = submit_params.clone();
        async move {
            app.handle(request("turn-submit-1", "turn.submit", submit_params))
                .await
        }
    });
    wait_until(|| backend.turns.load(Ordering::SeqCst) == 1).await;

    let durable_store = Store::open_path(root.state_db()).unwrap();
    let summary = durable_store
        .list_turns(conversation_id, 0, 100)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let submitting = durable_store.get_turn(summary.id).unwrap();
    let delivering = durable_store.get_message(submitting.message_id).unwrap();
    assert_eq!(submitting.state, TurnState::Submitting);
    assert_eq!(submitting.text, "Reply with done.");
    assert!(submitting.message_attempt_id.is_some());
    assert_eq!(delivering.delivery.state, MessageDeliveryState::Delivering);
    assert_eq!(delivering.attempts.len(), 1);
    let recorded_events = durable_store.list_events(0, 100).unwrap();
    assert!(
        recorded_events
            .iter()
            .any(|event| event.event_type == "turn.recorded")
    );
    assert!(
        recorded_events
            .iter()
            .any(|event| event.event_type == "turn.submitting")
    );

    backend.event_permits.add_permits(1);
    wait_until(|| {
        durable_store
            .get_turn(summary.id)
            .is_ok_and(|turn| turn.state == TurnState::Accepted)
    })
    .await;
    let accepted = durable_store.get_turn(summary.id).unwrap();
    let acknowledged = durable_store.get_message(accepted.message_id).unwrap();
    assert_eq!(accepted.native_session_id.as_deref(), Some("native-1"));
    assert_eq!(accepted.native_turn_id.as_deref(), Some("native-turn-1"));
    assert_eq!(
        acknowledged.delivery.state,
        MessageDeliveryState::Acknowledged
    );

    backend.event_permits.add_permits(1);
    let completed = timeout(Duration::from_secs(2), pending)
        .await
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    assert_eq!(completed["turn"]["state"], "completed");
    assert_eq!(completed["turn"]["events"].as_array().unwrap().len(), 2);
    assert!(
        completed["turn"]["events"][0]["payload"]
            .get("raw")
            .is_none()
    );

    let replay = app
        .handle(request("turn-submit-2", "turn.submit", submit_params))
        .await
        .result
        .unwrap();
    assert_eq!(replay["turn"]["id"], completed["turn"]["id"]);
    assert_eq!(backend.turns.load(Ordering::SeqCst), 1);

    let listed = app
        .handle(request(
            "turn-list",
            "turn.list",
            json!({"conversationId": conversation_id, "afterSequence": 0, "limit": 100}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], completed["turn"]["id"]);
    let shown = app
        .handle(request(
            "turn-show",
            "turn.show",
            json!({"id": completed["turn"]["id"]}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(shown["state"], "completed");
    let invalid_limit = app
        .handle(request(
            "turn-list-invalid",
            "turn.list",
            json!({"conversationId": conversation_id, "afterSequence": 0, "limit": 0}),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(invalid_limit.code, "LZ-TURN-INVALID");
}

#[tokio::test]
async fn task_authority_narrows_the_permission_mode_for_the_provider_turn() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = store
        .create_employee(&CreateEmployeeInput {
            request_id: "create-autonomous-request".to_owned(),
            idempotency_key: "create-autonomous".to_owned(),
            name: "Rhea".to_owned(),
            provider: "codex".to_owned(),
            role: "Lead".to_owned(),
            instructions: String::new(),
            model: Some("test-model".to_owned()),
            reasoning_effort: Some("low".to_owned()),
            native_settings: json!({}),
            authority_mode: AuthorityMode::FullAutonomy,
            checkout_policy: EmployeeCheckoutPolicy::Isolated,
            enabled: true,
            provider_observation: Some(ProviderObservation {
                provider: "codex".to_owned(),
                verification: "configured".to_owned(),
                version: Some("test".to_owned()),
                protocol: Some("app-server/v2".to_owned()),
                capabilities: json!({"session": true, "permission": true}),
                authentication: json!({"native": "ready", "external": "unknown"}),
                degradation_reason: None,
                checked_at: "2026-09-04T00:00:00Z".to_owned(),
            }),
        })
        .unwrap()
        .employee;
    let conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "create-authority-conversation-request".to_owned(),
            idempotency_key: "create-authority-conversation".to_owned(),
            project_id: None,
            lead_employee_id: employee.id,
            title: None,
        })
        .unwrap()
        .conversation;
    let backend = FakeSessionBackend::default();
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("authority-turn".to_owned()),
            raw: None,
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("authority-turn".to_owned()),
            status: "completed".to_owned(),
            output: Some("done".to_owned()),
            raw: None,
        },
    ]);
    let app = App::new_with_session_backend(
        store,
        root,
        DiscoveryEnvironment::empty(),
        Arc::new(backend.clone()),
    )
    .unwrap();
    app.handle(request(
        "warm-authority-session",
        "session.warm",
        json!({
            "employeeId": employee.id,
            "expectedEmployeeRevision": employee.configuration_revision,
            "idempotencyKey": "warm-authority-session"
        }),
    ))
    .await
    .result
    .unwrap();

    app.handle(request(
        "submit-authority-turn",
        "turn.submit",
        json!({
            "conversationId": conversation.id,
            "author": {"kind": "user"},
            "recipient": null,
            "text": "Stay inside ask authority",
            "activateIfNeeded": false,
            "maxAttempts": 3,
            "idempotencyKey": "submit-authority-turn"
        }),
    ))
    .await
    .result
    .unwrap();

    let requests = backend.turn_requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].permission_mode.as_deref(), Some("ask"));
}

#[tokio::test]
async fn turn_submit_without_observed_evidence_never_acknowledges_message() {
    let (_temporary, _root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    app.handle(request(
        "warm-turn-session",
        "session.warm",
        json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-turn-session"
        }),
    ))
    .await
    .result
    .unwrap();

    let error = app
        .handle(request(
            "turn-submit",
            "turn.submit",
            json!({
                "conversationId": conversation_id,
                "author": {"kind": "user"},
                "recipient": null,
                "text": "No event fixture",
                "idempotencyKey": "submit-no-evidence"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(error.code, "LZ-TURN-UNCERTAIN");
    assert_eq!(backend.turns.load(Ordering::SeqCst), 1);
    let turn = store
        .get_turn(store.list_turns(conversation_id, 0, 100).unwrap()[0].id)
        .unwrap();
    assert_eq!(turn.state, TurnState::Uncertain);
    assert_ne!(
        store.get_message(turn.message_id).unwrap().delivery.state,
        MessageDeliveryState::Acknowledged
    );
}

#[tokio::test]
async fn task_turn_rejects_a_provider_without_authority_override_support_before_io() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = store
        .create_employee(&CreateEmployeeInput {
            request_id: "create-claude-request".to_owned(),
            idempotency_key: "create-claude".to_owned(),
            name: "Claude lead".to_owned(),
            provider: "claude".to_owned(),
            role: "Lead".to_owned(),
            instructions: String::new(),
            model: None,
            reasoning_effort: None,
            native_settings: json!({}),
            authority_mode: AuthorityMode::Ask,
            checkout_policy: EmployeeCheckoutPolicy::Isolated,
            enabled: true,
            provider_observation: Some(ProviderObservation {
                provider: "claude".to_owned(),
                verification: "configured".to_owned(),
                version: Some("test".to_owned()),
                protocol: Some("acp".to_owned()),
                capabilities: json!({"session": true}),
                authentication: json!({"native": "ready", "external": "unknown"}),
                degradation_reason: None,
                checked_at: "2026-08-31T00:00:00Z".to_owned(),
            }),
        })
        .unwrap()
        .employee;
    let conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "create-conversation-request".to_owned(),
            idempotency_key: "create-conversation".to_owned(),
            project_id: None,
            lead_employee_id: employee.id,
            title: None,
        })
        .unwrap()
        .conversation;
    let backend = FakeSessionBackend::default();
    let app = App::new_with_session_backend(
        store.clone(),
        root,
        DiscoveryEnvironment::empty(),
        Arc::new(backend.clone()),
    )
    .unwrap();
    app.handle(request(
        "warm-claude",
        "session.warm",
        json!({
            "employeeId": employee.id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-claude"
        }),
    ))
    .await
    .result
    .unwrap();

    let error = app
        .handle(request(
            "turn-submit",
            "turn.submit",
            json!({
                "conversationId": conversation.id,
                "author": {"kind": "user"},
                "recipient": null,
                "text": "Unsupported provider fixture",
                "idempotencyKey": "submit-claude"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(error.code, "LZ-TASK-AUTHORITY-UNSUPPORTED");
    assert_eq!(backend.turns.load(Ordering::SeqCst), 0);
    let turn = store
        .get_turn(store.list_turns(conversation.id, 0, 100).unwrap()[0].id)
        .unwrap();
    assert_eq!(turn.state, TurnState::Recorded);
    assert_eq!(
        store.get_message(turn.message_id).unwrap().delivery.state,
        MessageDeliveryState::Queued
    );
}

#[tokio::test]
async fn turn_submit_activation_defaults_false_and_explicit_user_activation_waits_for_ready() {
    let (_temporary, _root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    let inactive = app
        .handle(request(
            "turn-inactive",
            "turn.submit",
            json!({
                "conversationId": conversation_id,
                "author": {"kind": "user"},
                "recipient": null,
                "text": "Do not activate",
                "idempotencyKey": "inactive-turn"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(inactive.code, "LZ-TURN-SESSION-NOT-READY");
    assert_eq!(backend.starts.load(Ordering::SeqCst), 0);
    assert_eq!(backend.turns.load(Ordering::SeqCst), 0);

    let second_conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "create-activation-conversation-request".to_owned(),
            idempotency_key: "create-activation-conversation".to_owned(),
            project_id: None,
            lead_employee_id: employee_id,
            title: None,
        })
        .unwrap()
        .conversation;
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("activated-turn".to_owned()),
            raw: None,
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("activated-turn".to_owned()),
            status: "completed".to_owned(),
            output: Some("ready first".to_owned()),
            raw: None,
        },
    ]);
    let activated = app
        .handle(request(
            "turn-activated",
            "turn.submit",
            json!({
                "conversationId": second_conversation.id,
                "author": {"kind": "user"},
                "recipient": null,
                "text": "Activate then deliver",
                "activateIfNeeded": true,
                "idempotencyKey": "activated-turn"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(activated["turn"]["state"], "completed");
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    assert_eq!(backend.turns.load(Ordering::SeqCst), 1);
    assert_eq!(activated["turn"]["nativeSessionId"], "native-1");
}

#[tokio::test]
async fn native_terminal_statuses_close_accepted_turns_without_uncertainty() {
    for (status, expected_state) in [
        ("failed", TurnState::TerminalFailed),
        ("interrupted", TurnState::Cancelled),
    ] {
        let (_temporary, _root, store, app, backend, employee_id, conversation_id) =
            turn_test_app();
        app.handle(request(
            &format!("warm-{status}"),
            "session.warm",
            json!({
                "employeeId": employee_id,
                "expectedEmployeeRevision": 1,
                "idempotencyKey": format!("warm-{status}")
            }),
        ))
        .await
        .result
        .unwrap();
        backend.turn_events.lock().unwrap().extend([
            ProviderEvent::TurnStarted {
                provider: ProviderKind::Codex,
                session_id: "native-1".to_owned(),
                turn_id: Some(format!("native-turn-{status}")),
                raw: None,
            },
            ProviderEvent::TurnCompleted {
                provider: ProviderKind::Codex,
                session_id: "native-1".to_owned(),
                turn_id: Some(format!("native-turn-{status}")),
                status: status.to_owned(),
                output: None,
                raw: None,
            },
        ]);

        let result = app
            .handle(request(
                &format!("turn-{status}"),
                "turn.submit",
                json!({
                    "conversationId": conversation_id,
                    "author": {"kind": "user"},
                    "recipient": null,
                    "text": format!("Native {status}"),
                    "idempotencyKey": format!("turn-{status}")
                }),
            ))
            .await
            .result
            .unwrap();

        assert_eq!(
            serde_json::from_value::<TurnState>(result["turn"]["state"].clone()).unwrap(),
            expected_state
        );
        let turn_id = result["turn"]["id"].as_str().unwrap().parse().unwrap();
        let turn = store.get_turn(turn_id).unwrap();
        assert_eq!(turn.state, expected_state);
        assert_eq!(
            store.get_message(turn.message_id).unwrap().delivery.state,
            MessageDeliveryState::Acknowledged
        );
    }
}

#[tokio::test]
async fn accepted_turn_allows_session_level_events_without_reproving_acceptance() {
    let (_temporary, _root, _store, app, backend, employee_id, conversation_id) = turn_test_app();
    app.handle(request(
        "warm-session-level-progress",
        "session.warm",
        json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-session-level-progress"
        }),
    ))
    .await
    .result
    .unwrap();
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("native-turn-with-usage".to_owned()),
            raw: None,
        },
        ProviderEvent::Usage {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            input_tokens: Some(10),
            output_tokens: Some(2),
            total_tokens: Some(12),
            cost_usd: None,
            raw: None,
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("native-turn-with-usage".to_owned()),
            status: "completed".to_owned(),
            output: Some("done".to_owned()),
            raw: None,
        },
    ]);

    let result = app
        .handle(request(
            "turn-with-session-level-progress",
            "turn.submit",
            json!({
                "conversationId": conversation_id,
                "author": {"kind": "user"},
                "recipient": null,
                "text": "Accept one session-level event",
                "idempotencyKey": "turn-with-session-level-progress"
            }),
        ))
        .await
        .result
        .unwrap();

    assert_eq!(result["turn"]["state"], "completed");
    assert_eq!(
        result["turn"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "usage")
            .count(),
        1
    );
}

#[tokio::test]
async fn turn_submit_employee_author_cannot_activate_dormant_recipient() {
    let (_temporary, _root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    let error = app
        .handle(request(
            "turn-employee-activation",
            "turn.submit",
            json!({
                "conversationId": conversation_id,
                "author": {"employeeId": employee_id, "sessionId": null},
                "recipient": null,
                "text": "Employee activation is forbidden",
                "activateIfNeeded": true,
                "idempotencyKey": "employee-activation"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(error.code, "LZ-TURN-INVALID");
    assert_eq!(backend.starts.load(Ordering::SeqCst), 0);
    assert!(
        store
            .list_turns(conversation_id, 0, 100)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn turn_submit_replay_resumes_only_recorded_queued_work_after_session_warm() {
    let (_temporary, _root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    let submit_params = json!({
        "conversationId": conversation_id,
        "author": {"kind": "user"},
        "recipient": null,
        "text": "Resume after warm",
        "idempotencyKey": "resume-recorded"
    });
    let first = app
        .handle(request(
            "turn-submit-before-warm",
            "turn.submit",
            submit_params.clone(),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(first.code, "LZ-TURN-SESSION-NOT-READY");
    let recorded = store
        .get_turn(store.list_turns(conversation_id, 0, 100).unwrap()[0].id)
        .unwrap();
    assert_eq!(recorded.state, TurnState::Recorded);
    assert!(recorded.message_attempt_id.is_none());
    assert_eq!(
        store
            .get_message(recorded.message_id)
            .unwrap()
            .delivery
            .state,
        MessageDeliveryState::Queued
    );

    app.handle(request(
        "warm-resume-session",
        "session.warm",
        json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-resume-session"
        }),
    ))
    .await
    .result
    .unwrap();
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("resumed-native-turn".to_owned()),
            raw: None,
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("resumed-native-turn".to_owned()),
            status: "completed".to_owned(),
            output: None,
            raw: None,
        },
    ]);
    let resumed = app
        .handle(request(
            "turn-submit-after-warm",
            "turn.submit",
            submit_params,
        ))
        .await
        .result
        .unwrap();
    assert_eq!(resumed["turn"]["id"], recorded.id.to_string());
    assert_eq!(resumed["turn"]["state"], "completed");
    assert_eq!(resumed["message"]["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(backend.turns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn turn_submit_replay_revalidates_configuration_and_provider_availability() {
    use lazoo_store::RefreshProviderObservationsInput;

    let (_temporary, _root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    let stale_params = json!({
        "conversationId": conversation_id,
        "author": {"kind": "user"},
        "recipient": null,
        "text": "Configuration guard",
        "idempotencyKey": "configuration-guard"
    });
    app.handle(request(
        "turn-record-configuration",
        "turn.submit",
        stale_params.clone(),
    ))
    .await;
    app.handle(request(
        "employee-update-before-delivery",
        "employee.update",
        json!({
            "id": employee_id,
            "expectedRevision": 1,
            "role": "Changed lead",
            "idempotencyKey": "change-lead-role"
        }),
    ))
    .await
    .result
    .unwrap();
    let stale = app
        .handle(request(
            "turn-replay-configuration",
            "turn.submit",
            stale_params,
        ))
        .await
        .error
        .unwrap();
    assert_eq!(stale.code, "LZ-TURN-EMPLOYEE-STALE");

    let second_conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "create-availability-conversation-request".to_owned(),
            idempotency_key: "create-availability-conversation".to_owned(),
            project_id: None,
            lead_employee_id: employee_id,
            title: None,
        })
        .unwrap()
        .conversation;
    let availability_params = json!({
        "conversationId": second_conversation.id,
        "author": {"kind": "user"},
        "recipient": null,
        "text": "Availability guard",
        "idempotencyKey": "availability-guard"
    });
    app.handle(request(
        "turn-record-availability",
        "turn.submit",
        availability_params.clone(),
    ))
    .await;
    store
        .refresh_provider_observations(&RefreshProviderObservationsInput {
            request_id: "degrade-provider-request".to_owned(),
            idempotency_key: "degrade-provider".to_owned(),
            observations: vec![ProviderObservation {
                provider: "codex".to_owned(),
                verification: "degraded".to_owned(),
                version: Some("test".to_owned()),
                protocol: Some("app-server/v2".to_owned()),
                capabilities: json!({"session": true}),
                authentication: json!({"native": "blocked", "external": "unknown"}),
                degradation_reason: Some("fixture degraded".to_owned()),
                checked_at: "2026-09-04T00:00:00Z".to_owned(),
            }],
        })
        .unwrap();
    let unavailable = app
        .handle(request(
            "turn-replay-availability",
            "turn.submit",
            availability_params,
        ))
        .await
        .error
        .unwrap();
    assert_eq!(unavailable.code, "LZ-TURN-EMPLOYEE-NOT-READY");
    assert_eq!(backend.starts.load(Ordering::SeqCst), 0);
    assert_eq!(backend.turns.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn turn_retry_reuses_turn_and_message_after_retry_is_due() {
    let (_temporary, _root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    app.handle(request(
        "warm-retry-session",
        "session.warm",
        json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-retry-session"
        }),
    ))
    .await
    .result
    .unwrap();
    backend.turn_retryable_failure.store(true, Ordering::SeqCst);
    let failed_error = app
        .handle(request(
            "turn-submit-retryable",
            "turn.submit",
            json!({
                "conversationId": conversation_id,
                "author": {"kind": "user"},
                "recipient": null,
                "text": "Retry me",
                "maxAttempts": 3,
                "idempotencyKey": "retryable-turn"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(failed_error.code, "LZ-TURN-PROVIDER");
    let failed = store
        .get_turn(store.list_turns(conversation_id, 0, 100).unwrap()[0].id)
        .unwrap();
    assert_eq!(failed.state, TurnState::RetryableFailed);
    let message_id = failed.message_id;

    let early = app
        .handle(request(
            "turn-retry-early",
            "turn.retry",
            json!({
                "id": failed.id,
                "expectedRevision": failed.revision,
                "idempotencyKey": "retry-early"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(early.code, "LZ-TURN-RETRY-NOT-DUE");

    store
        .execute_for_test(&format!(
            "UPDATE message_deliveries SET next_attempt_at = '2020-01-01T00:00:00Z' WHERE message_id = '{message_id}';"
        ))
        .unwrap();
    backend
        .turn_retryable_failure
        .store(false, Ordering::SeqCst);
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("retried-native-turn".to_owned()),
            raw: None,
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("retried-native-turn".to_owned()),
            status: "completed".to_owned(),
            output: Some("retried".to_owned()),
            raw: None,
        },
    ]);
    let retried = app
        .handle(request(
            "turn-retry",
            "turn.retry",
            json!({
                "id": failed.id,
                "expectedRevision": failed.revision,
                "idempotencyKey": "retry-due"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(retried["turn"]["id"], failed.id.to_string());
    assert_eq!(retried["message"]["envelope"]["id"], message_id.to_string());
    assert_eq!(retried["message"]["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(retried["turn"]["state"], "completed");
    assert_eq!(backend.turns.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn turn_cancel_before_provider_io_cancels_turn_and_message() {
    let (_temporary, _root, store, app, backend, _employee_id, conversation_id) = turn_test_app();
    app.handle(request(
        "turn-record-only",
        "turn.submit",
        json!({
            "conversationId": conversation_id,
            "author": {"kind": "user"},
            "recipient": null,
            "text": "Cancel before delivery",
            "idempotencyKey": "cancel-recorded"
        }),
    ))
    .await;
    let recorded = store
        .get_turn(store.list_turns(conversation_id, 0, 100).unwrap()[0].id)
        .unwrap();
    let cancelled = app
        .handle(request(
            "turn-cancel-recorded",
            "turn.cancel",
            json!({
                "id": recorded.id,
                "expectedRevision": recorded.revision,
                "idempotencyKey": "cancel-recorded-command"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(cancelled["turn"]["state"], "cancelled");
    assert_eq!(cancelled["message"]["delivery"]["state"], "cancelled");
    assert_eq!(backend.cancels.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn turn_cancel_after_acceptance_waits_for_native_cancellation_evidence() {
    let (_temporary, _root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    backend.gate_events.store(true, Ordering::SeqCst);
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("cancel-native-turn".to_owned()),
            raw: None,
        },
        ProviderEvent::Cancelled {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("cancel-native-turn".to_owned()),
            raw: None,
        },
    ]);
    app.handle(request(
        "warm-cancel-session",
        "session.warm",
        json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-cancel-session"
        }),
    ))
    .await
    .result
    .unwrap();
    let submitting = tokio::spawn({
        let app = app.clone();
        async move {
            app.handle(request(
                "turn-submit-cancel",
                "turn.submit",
                json!({
                    "conversationId": conversation_id,
                    "author": {"kind": "user"},
                    "recipient": null,
                    "text": "Cancel after acceptance",
                    "idempotencyKey": "cancel-accepted"
                }),
            ))
            .await
        }
    });
    wait_until(|| backend.turns.load(Ordering::SeqCst) == 1).await;
    let turn_id = store.list_turns(conversation_id, 0, 100).unwrap()[0].id;
    backend.event_permits.add_permits(1);
    wait_until(|| {
        store
            .get_turn(turn_id)
            .is_ok_and(|turn| turn.state == TurnState::Accepted)
    })
    .await;
    let accepted = store.get_turn(turn_id).unwrap();
    let cancelling = tokio::spawn({
        let app = app.clone();
        async move {
            app.handle(request(
                "turn-cancel-accepted",
                "turn.cancel",
                json!({
                    "id": turn_id,
                    "expectedRevision": accepted.revision,
                    "idempotencyKey": "cancel-accepted-command"
                }),
            ))
            .await
        }
    });
    wait_until(|| backend.cancels.load(Ordering::SeqCst) == 1).await;
    let requested = store.get_turn(turn_id).unwrap();
    assert_eq!(requested.state, TurnState::Accepted);
    assert!(requested.cancel_requested_at.is_some());
    assert!(!cancelling.is_finished());

    backend.event_permits.add_permits(1);
    let cancelled = timeout(Duration::from_secs(2), cancelling)
        .await
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    let submitted = timeout(Duration::from_secs(2), submitting)
        .await
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    assert_eq!(cancelled["turn"]["state"], "cancelled");
    assert_eq!(submitted["turn"]["state"], "cancelled");
    assert_eq!(cancelled["message"]["delivery"]["state"], "acknowledged");
}

#[tokio::test]
async fn turn_cancel_with_missing_live_handle_becomes_uncertain() {
    let (_temporary, _root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    backend.gate_events.store(true, Ordering::SeqCst);
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("orphaned-native-turn".to_owned()),
            raw: None,
        },
        ProviderEvent::Progress {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            message: Some("still running".to_owned()),
            raw: None,
        },
    ]);
    app.handle(request(
        "warm-orphan-session",
        "session.warm",
        json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-orphan-session"
        }),
    ))
    .await
    .result
    .unwrap();
    let submitting = tokio::spawn({
        let app = app.clone();
        async move {
            app.handle(request(
                "turn-submit-orphan",
                "turn.submit",
                json!({
                    "conversationId": conversation_id,
                    "author": {"kind": "user"},
                    "recipient": null,
                    "text": "Lose the live handle",
                    "idempotencyKey": "orphaned-turn"
                }),
            ))
            .await
        }
    });
    wait_until(|| backend.turns.load(Ordering::SeqCst) == 1).await;
    let turn_id = store.list_turns(conversation_id, 0, 100).unwrap()[0].id;
    backend.event_permits.add_permits(1);
    wait_until(|| {
        store
            .get_turn(turn_id)
            .is_ok_and(|turn| turn.state == TurnState::Accepted)
    })
    .await;
    app.shutdown_live_sessions().await;
    let accepted = store.get_turn(turn_id).unwrap();

    let error = app
        .handle(request(
            "turn-cancel-orphan",
            "turn.cancel",
            json!({
                "id": turn_id,
                "expectedRevision": accepted.revision,
                "idempotencyKey": "cancel-orphaned-turn"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(error.code, "LZ-TURN-UNCERTAIN");
    assert_eq!(backend.cancels.load(Ordering::SeqCst), 0);
    assert_eq!(store.get_turn(turn_id).unwrap().state, TurnState::Uncertain);

    backend.event_permits.add_permits(1);
    let submitted = timeout(Duration::from_secs(2), submitting)
        .await
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    assert_eq!(submitted["turn"]["state"], "uncertain");
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Keep the end-to-end transition assertions in one scenario.
async fn message_send_claim_and_acknowledge_are_idempotent_and_backend_only() {
    let (_temporary, _root, _store, app, sender, recipient) = message_test_app();
    let send_params = json!({
        "sender": {"employeeId": sender},
        "recipient": {"employeeId": recipient},
        "kind": "peer",
        "body": {"text": "Need the spacing token."},
        "idempotencyKey": "send-message"
    });
    let sent = app
        .handle(request("send-1", "message.send", send_params.clone()))
        .await
        .result
        .unwrap();
    let replay = app
        .handle(request("send-2", "message.send", send_params))
        .await
        .result
        .unwrap();
    assert_eq!(
        sent["message"]["envelope"]["id"],
        replay["message"]["envelope"]["id"]
    );
    assert_eq!(sent["message"]["envelope"]["version"], 1);
    assert_eq!(sent["message"]["delivery"]["state"], "queued");
    assert_eq!(sent["message"]["delivery"]["revision"], 1);
    assert_eq!(sent["message"]["attempts"], json!([]));
    assert_eq!(
        sent["message"]["activationIntent"]["requiresActivation"],
        true
    );
    assert!(sent["eventSequence"].is_number());
    assert!(replay["eventSequence"].is_null());
    let message_id = sent["message"]["envelope"]["id"].as_str().unwrap();

    let listed = app
        .handle(request(
            "list",
            "message.list",
            json!({"afterSequence": 0, "limit": 100}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], message_id);
    let shown = app
        .handle(request("show", "message.show", json!({"id": message_id})))
        .await
        .result
        .unwrap();
    assert_eq!(
        shown["envelope"]["body"],
        json!({"text": "Need the spacing token."})
    );

    let claimed = app
        .handle(request(
            "claim",
            "message.claim",
            json!({
                "id": message_id,
                "expectedRevision": 1,
                "idempotencyKey": "claim-message"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(claimed["message"]["delivery"]["state"], "delivering");
    let attempt_id = claimed["message"]["attempts"][0]["id"].as_str().unwrap();

    let wrong_attempt = app
        .handle(request(
            "wrong-ack",
            "message.acknowledge",
            json!({
                "id": message_id,
                "expectedRevision": 2,
                "attemptId": uuid::Uuid::new_v4(),
                "acknowledgementKey": "provider-ack-1",
                "evidence": {"progress": true},
                "idempotencyKey": "wrong-ack"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(wrong_attempt.code, "LZ-MESSAGE-ATTEMPT");

    let acknowledge_params = json!({
        "id": message_id,
        "expectedRevision": 2,
        "attemptId": attempt_id,
        "acknowledgementKey": "provider-ack-1",
        "evidence": {"progress": true},
        "idempotencyKey": "ack-message"
    });
    let acknowledged = app
        .handle(request(
            "ack-1",
            "message.acknowledge",
            acknowledge_params.clone(),
        ))
        .await
        .result
        .unwrap();
    let ack_replay = app
        .handle(request("ack-2", "message.acknowledge", acknowledge_params))
        .await
        .result
        .unwrap();
    assert_eq!(acknowledged["message"]["delivery"]["state"], "acknowledged");
    assert_eq!(acknowledged["message"], ack_replay["message"]);
    assert!(ack_replay["eventSequence"].is_null());
    assert_desktop_is_roster_neutral(&app).await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Retry, exhaustion, and cancellation share one durable fixture.
async fn message_retry_dead_letter_cancel_and_errors_are_stable() {
    let (_temporary, _root, store, app, sender, recipient) = message_test_app();
    let sent = app
        .handle(request(
            "send-retry",
            "message.send",
            json!({
                "sender": {"employeeId": sender},
                "recipient": {"employeeId": recipient},
                "kind": "peer",
                "body": {"text": "Retry me."},
                "maxAttempts": 2,
                "idempotencyKey": "send-retry"
            }),
        ))
        .await
        .result
        .unwrap();
    let message_id = sent["message"]["envelope"]["id"].as_str().unwrap();
    let stale = app
        .handle(request(
            "stale-claim",
            "message.claim",
            json!({
                "id": message_id,
                "expectedRevision": 2,
                "idempotencyKey": "stale-claim"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(stale.code, "LZ-MESSAGE-STALE");
    assert_eq!(stale.data.unwrap()["currentRevision"], 1);

    let claimed = app
        .handle(request(
            "claim-retry",
            "message.claim",
            json!({
                "id": message_id,
                "expectedRevision": 1,
                "idempotencyKey": "claim-retry"
            }),
        ))
        .await
        .result
        .unwrap();
    let attempt_id = claimed["message"]["attempts"][0]["id"].as_str().unwrap();
    let failed = app
        .handle(request(
            "fail-retry",
            "message.fail",
            json!({
                "id": message_id,
                "expectedRevision": 2,
                "attemptId": attempt_id,
                "retryable": true,
                "failureCode": "LZ-MESSAGE-TIMEOUT",
                "failureMessage": "recipient acknowledgement was not observed",
                "idempotencyKey": "fail-retry"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(failed["message"]["delivery"]["state"], "retryable-failed");
    let early = app
        .handle(request(
            "early-retry",
            "message.retry",
            json!({
                "id": message_id,
                "expectedRevision": 3,
                "idempotencyKey": "early-retry"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(early.code, "LZ-MESSAGE-RETRY-NOT-DUE");
    assert!(early.data.unwrap()["nextAttemptAt"].is_string());
    store
        .execute_for_test(&format!(
            "UPDATE message_deliveries SET next_attempt_at = '2026-01-01T00:00:00Z' WHERE message_id = '{message_id}'"
        ))
        .unwrap();
    let retrying = app
        .handle(request(
            "retry",
            "message.retry",
            json!({
                "id": message_id,
                "expectedRevision": 3,
                "idempotencyKey": "retry"
            }),
        ))
        .await
        .result
        .unwrap();
    let final_attempt_id = retrying["message"]["attempts"][1]["id"].as_str().unwrap();
    let exhausted = app
        .handle(request(
            "exhausted",
            "message.fail",
            json!({
                "id": message_id,
                "expectedRevision": 4,
                "attemptId": final_attempt_id,
                "retryable": true,
                "failureCode": "LZ-MESSAGE-TIMEOUT",
                "failureMessage": "recipient acknowledgement was not observed",
                "idempotencyKey": "exhausted"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(exhausted["message"]["delivery"]["state"], "terminal-failed");
    assert_eq!(
        exhausted["message"]["deadLetter"]["failureCode"],
        "LZ-MESSAGE-TIMEOUT"
    );

    let cancellable = app
        .handle(request(
            "send-cancel",
            "message.send",
            json!({
                "sender": {"employeeId": sender},
                "recipient": {"employeeId": recipient},
                "kind": "status",
                "body": {"text": "Cancel me."},
                "idempotencyKey": "send-cancel"
            }),
        ))
        .await
        .result
        .unwrap();
    let cancellable_id = cancellable["message"]["envelope"]["id"].as_str().unwrap();
    let cancelled = app
        .handle(request(
            "cancel",
            "message.cancel",
            json!({
                "id": cancellable_id,
                "expectedRevision": 1,
                "idempotencyKey": "cancel"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(cancelled["message"]["delivery"]["state"], "cancelled");
}

#[tokio::test]
async fn message_restart_reconciles_an_uncertain_attempt() {
    let (_temporary, root, store, app, sender, recipient) = message_test_app();
    let sent = app
        .handle(request(
            "send-restart",
            "message.send",
            json!({
                "sender": {"employeeId": sender},
                "recipient": {"employeeId": recipient},
                "kind": "peer",
                "body": {"text": "Survive restart."},
                "idempotencyKey": "send-restart"
            }),
        ))
        .await
        .result
        .unwrap();
    let message_id = sent["message"]["envelope"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    app.handle(request(
        "claim-restart",
        "message.claim",
        json!({
            "id": message_id,
            "expectedRevision": 1,
            "idempotencyKey": "claim-restart"
        }),
    ))
    .await
    .result
    .unwrap();
    drop(app);
    drop(store);

    let reopened = Store::open_path(root.state_db()).unwrap();
    let status_store = reopened.clone();
    let restarted =
        App::new_with_provider_environment(reopened, root, DiscoveryEnvironment::empty()).unwrap();
    assert_eq!(status_store.status().unwrap().outbox_pending, 0);
    let shown = restarted
        .handle(request(
            "show-reconciled",
            "message.show",
            json!({"id": message_id}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(shown["delivery"]["state"], "retryable-failed");
    assert_eq!(shown["attempts"][0]["failureCode"], "LZ-MESSAGE-OWNER-LOST");
}

#[tokio::test]
async fn status_reports_protocol_store_and_boot_identity() {
    let (_temporary, app) = test_app();

    let response = app
        .handle(request("status-1", "system.status", json!({})))
        .await;
    let result = response.result.unwrap();

    assert_eq!(result["protocolVersion"], 1);
    assert_eq!(result["schemaVersion"], 6);
    assert_eq!(result["journalMode"], "wal");
    assert!(
        result["bootId"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(result["projectCount"], 0);
}

#[tokio::test]
async fn project_registration_and_listing_use_real_git_identity() {
    let (temporary, app) = test_app();
    let repository = temporary.path().join("repository");
    std::fs::create_dir_all(&repository).unwrap();
    git_init(&repository);

    let registration = app
        .handle(request(
            "register-1",
            "project.register",
            json!({
                "path": repository,
                "displayName": "Example",
                "checkoutMode": CheckoutMode::LeadMain,
                "idempotencyKey": "register-example"
            }),
        ))
        .await;
    let project = &registration.result.as_ref().unwrap()["project"];
    assert_eq!(project["displayName"], "Example");
    assert_eq!(project["checkoutMode"], "lead-main");
    assert_eq!(registration.result.as_ref().unwrap()["eventSequence"], 1);

    let listing = app
        .handle(request("list-1", "project.list", json!({})))
        .await;
    assert_eq!(listing.result.unwrap().as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn repeated_idempotency_key_produces_one_logical_event() {
    let (temporary, app) = test_app();
    let repository = temporary.path().join("repository");
    std::fs::create_dir_all(&repository).unwrap();
    git_init(&repository);
    let parameters = json!({
        "path": repository,
        "idempotencyKey": "one-logical-command"
    });

    let first = app
        .handle(request(
            "register-1",
            "project.register",
            parameters.clone(),
        ))
        .await;
    let second = app
        .handle(request("register-2", "project.register", parameters))
        .await;
    let events = app
        .handle(request(
            "events-1",
            "events.list",
            json!({"after": 0, "limit": 100}),
        ))
        .await;

    assert_eq!(first.result, second.result);
    assert_eq!(events.result.unwrap().as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_method_returns_a_stable_rpc_error() {
    let (_temporary, app) = test_app();

    let response = app
        .handle(request("unknown-1", "unknown.method", json!({})))
        .await;

    let error = response.error.unwrap();
    assert_eq!(error.code, "LZ-METHOD-NOT-FOUND");
    assert_eq!(error.data.unwrap()["method"], "unknown.method");
}

#[tokio::test]
async fn subscription_has_catch_up_then_live_event_without_duplication() {
    let (temporary, app) = test_app();
    let first_repository = temporary.path().join("first");
    let second_repository = temporary.path().join("second");
    std::fs::create_dir_all(&first_repository).unwrap();
    std::fs::create_dir_all(&second_repository).unwrap();
    git_init(&first_repository);
    git_init(&second_repository);
    app.handle(request(
        "register-1",
        "project.register",
        json!({"path": first_repository, "idempotencyKey": "first"}),
    ))
    .await;

    let (catch_up, mut live) = app.subscribe(0).unwrap();
    assert_eq!(catch_up.len(), 1);
    assert_eq!(catch_up[0].sequence, 1);

    app.handle(request(
        "register-2",
        "project.register",
        json!({"path": second_repository, "idempotencyKey": "second"}),
    ))
    .await;
    let event = timeout(Duration::from_secs(2), live.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(event.sequence, 2);
}

#[tokio::test]
async fn shutdown_request_sets_the_planned_stop_signal() {
    let (_temporary, app) = test_app();
    let mut shutdown = app.shutdown_receiver();

    let response = app
        .handle(request("shutdown-1", "system.shutdown", json!({})))
        .await;

    assert_eq!(response.result.unwrap()["planned"], true);
    timeout(Duration::from_secs(2), shutdown.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(*shutdown.borrow());
}

#[tokio::test]
async fn desktop_snapshot_returns_status_projects_and_recent_events() {
    let (temporary, app) = test_app();
    let repository = temporary.path().join("desktop-project");
    std::fs::create_dir_all(&repository).unwrap();
    git_init(&repository);
    app.handle(request(
        "register-desktop",
        "project.register",
        json!({
            "path": repository,
            "displayName": "Desktop project",
            "idempotencyKey": "desktop-project"
        }),
    ))
    .await;

    let response = app
        .handle(request("desktop-1", "desktop.snapshot", json!({})))
        .await;
    let snapshot: DesktopSnapshot =
        serde_json::from_value(response.result.expect("snapshot result")).unwrap();

    assert_eq!(snapshot.status.project_count, 1);
    assert_eq!(snapshot.status.event_high_water, 1);
    assert_eq!(snapshot.projects.len(), 1);
    assert_eq!(snapshot.projects[0].display_name, "Desktop project");
    assert_eq!(snapshot.events.len(), 1);
    assert_eq!(snapshot.events[0].sequence, 1);
    assert_eq!(snapshot.events[0].event_type, "project.registered");
    assert!(snapshot.captured_at.ends_with('Z'));
}

#[tokio::test]
async fn desktop_snapshot_rejects_nonempty_parameters() {
    let (_temporary, app) = test_app();

    let response = app
        .handle(request(
            "desktop-invalid",
            "desktop.snapshot",
            json!({"method": "system.shutdown"}),
        ))
        .await;

    let error = response.error.expect("nonempty params must fail");
    assert_eq!(error.code, "LZ-INVALID-PARAMS");
}

#[tokio::test]
async fn provider_list_reports_all_providers_without_changing_desktop_snapshot() {
    let (_temporary, app) = test_app();

    let providers = app
        .handle(request("providers", "provider.list", json!({})))
        .await
        .result
        .unwrap();
    assert_eq!(providers.as_array().unwrap().len(), 5);

    let snapshot = app
        .handle(request("desktop", "desktop.snapshot", json!({})))
        .await
        .result
        .unwrap();
    assert!(snapshot.get("providers").is_none());
}

#[tokio::test]
async fn provider_methods_reject_unknown_names_with_a_stable_error() {
    let (_temporary, app) = test_app();

    for method in ["provider.probe", "provider.smoke"] {
        let params = if method == "provider.smoke" {
            json!({"provider": "missing", "prompt": "Reply OK"})
        } else {
            json!({"provider": "missing"})
        };
        let error = app
            .handle(request("provider-error", method, params))
            .await
            .error
            .unwrap();
        assert_eq!(error.code, "LZ-PROVIDER-UNKNOWN");
    }
}

#[tokio::test]
async fn employee_create_update_disable_and_roster_are_durable_and_ui_neutral() {
    let (_temporary, app) = test_app();

    let created = app
        .handle(request(
            "employee-create",
            "employee.create",
            json!({
                "name": "Rhea",
                "provider": "codex",
                "role": "Lead",
                "idempotencyKey": "create-rhea"
            }),
        ))
        .await
        .result
        .unwrap();
    let employee_id = created["employee"]["id"].as_str().unwrap();
    assert_eq!(created["employee"]["name"], "Rhea");
    assert_eq!(created["employee"]["authorityMode"], "ask");
    assert_eq!(created["employee"]["checkoutPolicy"], "isolated");
    assert_eq!(created["rosterRevision"], 1);

    let employees = app
        .handle(request("employee-list", "employee.list", json!({})))
        .await
        .result
        .unwrap();
    assert_eq!(employees.as_array().unwrap().len(), 1);
    let shown = app
        .handle(request(
            "employee-show",
            "employee.show",
            json!({"id": employee_id}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(shown["name"], "Rhea");

    let roster = app
        .handle(request("roster-get", "roster.get", json!({})))
        .await
        .result
        .unwrap();
    assert_eq!(roster["revision"], 1);
    assert_eq!(roster["entries"][0]["availability"], "offline");
    assert_eq!(
        roster["entries"][0]["providerObservation"]["verification"],
        "not-detected"
    );

    let updated = app
        .handle(request(
            "employee-update",
            "employee.update",
            json!({
                "id": employee_id,
                "expectedRevision": 1,
                "role": "Principal lead",
                "idempotencyKey": "update-rhea"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(updated["employee"]["configurationRevision"], 2);
    assert_eq!(updated["rosterRevision"], 2);

    let stale = app
        .handle(request(
            "employee-stale",
            "employee.update",
            json!({
                "id": employee_id,
                "expectedRevision": 1,
                "role": "Stale edit",
                "idempotencyKey": "stale-rhea"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(stale.code, "LZ-EMPLOYEE-STALE");
    assert_eq!(stale.data.unwrap(), json!({"currentRevision": 2}));

    let disabled = app
        .handle(request(
            "employee-disable",
            "employee.disable",
            json!({
                "id": employee_id,
                "expectedRevision": 2,
                "idempotencyKey": "disable-rhea"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(disabled["employee"]["enabled"], false);
    assert_eq!(disabled["rosterRevision"], 3);

    assert_desktop_is_roster_neutral(&app).await;
}

#[tokio::test]
async fn employee_validation_rejects_credential_like_native_settings() {
    let (_temporary, app) = test_app();

    let error = app
        .handle(request(
            "employee-invalid",
            "employee.create",
            json!({
                "name": "Rhea",
                "provider": "codex",
                "nativeSettings": {"api_key": "must-not-be-stored"},
                "idempotencyKey": "invalid-rhea"
            }),
        ))
        .await
        .error
        .unwrap();

    assert_eq!(error.code, "LZ-EMPLOYEE-INVALID");
    assert!(!error.message.contains("must-not-be-stored"));
}

#[tokio::test]
async fn roster_refresh_is_non_model_and_does_not_advance_unchanged_state() {
    let (_temporary, app) = test_app();
    app.handle(request(
        "employee-create",
        "employee.create",
        json!({
            "name": "Rhea",
            "provider": "codex",
            "idempotencyKey": "create-rhea"
        }),
    ))
    .await;

    let refreshed = app
        .handle(request(
            "roster-refresh",
            "roster.refresh",
            json!({"idempotencyKey": "refresh-roster"}),
        ))
        .await
        .result
        .unwrap();

    assert_eq!(refreshed["roster"]["revision"], 1);
    assert!(refreshed["eventSequence"].is_null());
    assert_eq!(refreshed["roster"]["entries"][0]["availability"], "offline");
}

#[tokio::test]
async fn session_warm_list_show_stop_is_idempotent_and_backend_only() {
    let (_temporary, app, backend, employee_id) = session_test_app();
    let warm_params = json!({
        "employeeId": employee_id,
        "expectedEmployeeRevision": 1,
        "idempotencyKey": "warm-rhea"
    });

    let first = app
        .handle(request("warm-1", "session.warm", warm_params.clone()))
        .await
        .result
        .unwrap();
    let replay = app
        .handle(request("warm-2", "session.warm", warm_params))
        .await
        .result
        .unwrap();
    let session_id = first["session"]["id"].as_str().unwrap();

    assert_eq!(first["session"]["state"], "idle");
    assert_eq!(first["session"]["nativeSessionId"], "native-1");
    assert_eq!(first["session"]["nativeHostPid"], 4242);
    assert_eq!(first["session"], replay["session"]);
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    {
        let working_directories = backend.working_directories.lock().unwrap();
        assert_eq!(working_directories.len(), 1);
        assert!(working_directories[0].starts_with(app.data_root().root()));
    }

    let listed = app
        .handle(request("list", "session.list", json!({})))
        .await
        .result
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    let shown = app
        .handle(request("show", "session.show", json!({"id": session_id})))
        .await
        .result
        .unwrap();
    assert_eq!(shown["id"], session_id);

    let stop_params = json!({
        "id": session_id,
        "expectedRevision": 2,
        "idempotencyKey": "stop-rhea"
    });
    let stopped = app
        .handle(request("stop-1", "session.stop", stop_params.clone()))
        .await
        .result
        .unwrap();
    let stop_replay = app
        .handle(request("stop-2", "session.stop", stop_params))
        .await
        .result
        .unwrap();

    assert_eq!(stopped["session"]["state"], "suspended");
    assert_eq!(stopped["session"], stop_replay["session"]);
    assert_eq!(backend.stops.load(Ordering::SeqCst), 1);
    let roster = app
        .handle(request("roster", "roster.get", json!({})))
        .await
        .result
        .unwrap();
    assert_eq!(roster["entries"][0]["availability"], "suspended");
    assert_desktop_is_roster_neutral(&app).await;
}

#[tokio::test]
async fn session_stale_revision_has_a_stable_error_family() {
    let (_temporary, app, _backend, employee_id) = session_test_app();
    let warmed = app
        .handle(request(
            "warm",
            "session.warm",
            json!({
                "employeeId": employee_id,
                "expectedEmployeeRevision": 1,
                "idempotencyKey": "warm-rhea"
            }),
        ))
        .await
        .result
        .unwrap();
    let session_id = warmed["session"]["id"].as_str().unwrap();

    let error = app
        .handle(request(
            "stale-stop",
            "session.stop",
            json!({
                "id": session_id,
                "expectedRevision": 1,
                "idempotencyKey": "stale-stop"
            }),
        ))
        .await
        .error
        .unwrap();

    assert_eq!(error.code, "LZ-SESSION-STALE");
    assert_eq!(error.data.unwrap(), json!({"currentRevision": 2}));
}

#[tokio::test]
async fn planned_shutdown_stops_and_suspends_live_sessions() {
    let (_temporary, app, backend, employee_id) = session_test_app();
    app.handle(request(
        "warm",
        "session.warm",
        json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-rhea"
        }),
    ))
    .await;

    app.shutdown_live_sessions().await;

    assert_eq!(backend.stops.load(Ordering::SeqCst), 1);
    let sessions = app
        .handle(request("sessions", "session.list", json!({})))
        .await
        .result
        .unwrap();
    assert_eq!(sessions[0]["state"], "suspended");
}

#[tokio::test]
async fn failed_native_start_is_redacted_and_persisted() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = configured_employee(&store);
    let app = App::new_with_session_backend(
        store,
        root,
        DiscoveryEnvironment::empty(),
        Arc::new(FailingSessionBackend),
    )
    .unwrap();

    let error = app
        .handle(request(
            "warm",
            "session.warm",
            json!({
                "employeeId": employee.id,
                "expectedEmployeeRevision": 1,
                "idempotencyKey": "warm-rhea"
            }),
        ))
        .await
        .error
        .unwrap();

    assert_eq!(error.code, "LZ-SESSION-START-FAILED");
    assert!(!error.message.contains("private provider diagnostic"));
    let sessions = app
        .handle(request("sessions", "session.list", json!({})))
        .await
        .result
        .unwrap();
    assert_eq!(sessions[0]["state"], "failed");
    assert_eq!(sessions[0]["failureCode"], "LZ-SESSION-START-FAILED");
}

#[test]
fn app_startup_reconciles_prior_boot_sessions_without_adopting_a_pid() {
    use lazoo_store::{BeginSessionInput, MarkSessionReadyInput};

    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = configured_employee(&store);
    let prior_boot = uuid::Uuid::new_v4();
    let starting = store
        .begin_session(&BeginSessionInput {
            request_id: "warm-request".to_owned(),
            idempotency_key: "warm-rhea".to_owned(),
            employee_id: employee.id,
            expected_employee_revision: 1,
            owner_boot_id: prior_boot,
        })
        .unwrap();
    store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: starting.session.id,
            expected_revision: 1,
            owner_boot_id: prior_boot,
            native_session_id: "prior-native".to_owned(),
            native_host_pid: 4242,
        })
        .unwrap();

    let _app = App::new_with_session_backend(
        store.clone(),
        root,
        DiscoveryEnvironment::empty(),
        Arc::new(FakeSessionBackend::default()),
    )
    .unwrap();

    let reconciled = store.get_session(starting.session.id).unwrap();
    assert_eq!(
        reconciled.state,
        lazoo_protocol::SessionLifecycleState::Failed
    );
    assert_eq!(
        reconciled.failure_code.as_deref(),
        Some("LZ-SESSION-OWNER-LOST")
    );
    assert!(reconciled.native_host_pid.is_none());
}

#[test]
#[allow(clippy::too_many_lines)] // Prior-boot session and turn state form one recovery fixture.
fn app_startup_reconciles_prior_boot_turns_without_adopting_provider_work() {
    use lazoo_protocol::{MessageSender, UserMessageEndpoint, UserMessageKind};
    use lazoo_store::{
        BeginSessionInput, BeginTurnSubmissionInput, MarkSessionReadyInput, ObserveTurnInput,
        RecordTurnInput,
    };

    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = configured_employee(&store);
    let prior_boot = uuid::Uuid::new_v4();
    let starting = store
        .begin_session(&BeginSessionInput {
            request_id: "warm-prior-turn".to_owned(),
            idempotency_key: "warm-prior-turn".to_owned(),
            employee_id: employee.id,
            expected_employee_revision: 1,
            owner_boot_id: prior_boot,
        })
        .unwrap();
    let ready = store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: starting.session.id,
            expected_revision: starting.session.revision,
            owner_boot_id: prior_boot,
            native_session_id: "prior-native".to_owned(),
            native_host_pid: 4242,
        })
        .unwrap();
    let conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "prior-conversation-request".to_owned(),
            idempotency_key: "prior-conversation".to_owned(),
            project_id: None,
            lead_employee_id: employee.id,
            title: None,
        })
        .unwrap()
        .conversation;
    let recorded = store
        .record_turn(&RecordTurnInput {
            request_id: "prior-turn-request".to_owned(),
            idempotency_key: "prior-turn".to_owned(),
            conversation_id: conversation.id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::Ask,
            text: "Prior accepted turn".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap();
    let submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "prior-turn-begin-request".to_owned(),
            idempotency_key: "prior-turn-begin".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: recorded.turn.revision,
            owner_boot_id: prior_boot,
            recipient_session_id: ready.session.id,
        })
        .unwrap();
    let accepted = store
        .observe_turn(&ObserveTurnInput {
            request_id: "prior-turn-accepted-request".to_owned(),
            idempotency_key: "prior-turn-accepted".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: submitting.turn.revision,
            kind: "turn-started".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("prior-native".to_owned()),
            native_turn_id: Some("prior-native-turn".to_owned()),
            payload: json!({"progress": true}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap();
    assert_eq!(
        accepted.message.delivery.state,
        MessageDeliveryState::Acknowledged
    );

    let backend = FakeSessionBackend::default();
    let _app = App::new_with_session_backend(
        store.clone(),
        root,
        DiscoveryEnvironment::empty(),
        Arc::new(backend.clone()),
    )
    .unwrap();

    let reconciled = store.get_turn(recorded.turn.id).unwrap();
    assert_eq!(reconciled.state, TurnState::Uncertain);
    assert_eq!(
        reconciled.failure_code.as_deref(),
        Some("LZ-TURN-OWNER-LOST")
    );
    assert!(reconciled.owner_boot_id.is_none());
    let reconciled_task = store.get_task(recorded.turn.id).unwrap().task;
    assert_eq!(reconciled_task.state, lazoo_protocol::TaskState::Uncertain);
    assert_eq!(
        reconciled_task.failure_code.as_deref(),
        Some("LZ-TASK-OWNER-LOST")
    );
    assert!(reconciled_task.active_turn_id.is_none());
    assert!(reconciled_task.active_session_id.is_none());
    assert_eq!(
        store
            .get_message(reconciled.message_id)
            .unwrap()
            .delivery
            .state,
        MessageDeliveryState::Acknowledged
    );
    assert_eq!(backend.starts.load(Ordering::SeqCst), 0);
    assert_eq!(backend.turns.load(Ordering::SeqCst), 0);
}

#[test]
fn employee_roster_acceptance_contract_is_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/employee-roster.ps1");
    let script = std::fs::read_to_string(&script_path).expect("employee roster acceptance script");

    for required in [
        "employee', 'create",
        "employee', 'update",
        "employee', 'disable",
        "employee', 'show",
        "roster', 'show",
        "roster', 'refresh",
        "LZ-EMPLOYEE-STALE",
        "restartDurability",
        "schemaVersion 5",
        "providerSmokeAbsent",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in [
        "provider', 'smoke",
        "session', 'warm",
        "conpty",
        "worktree",
        "apps\\lazoo-desktop",
    ] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

#[test]
fn native_session_acceptance_contract_covers_containment_and_owner_loss() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/native-session-ownership.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("native session ownership acceptance script");

    for required in [
        "lazoo-job-containment.exe",
        "session', 'warm",
        "session', 'stop",
        "LZ-SESSION-OWNER-LOST",
        "creationTimeContainment",
        "completeTreeTerminated",
        "nativeHostPid",
        "providerSmokeAbsent",
        "schemaVersion 5",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    assert!(!script.contains("provider', 'smoke"));
    assert!(!script.contains("apps\\lazoo-desktop"));
}

#[test]
fn collaboration_bus_acceptance_contract_is_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/collaboration-bus.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("collaboration bus acceptance script");

    for required in [
        "message', 'send",
        "message', 'claim",
        "message', 'acknowledge",
        "message', 'fail",
        "message', 'retry",
        "message', 'cancel",
        "LZ-MESSAGE-RETRY-NOT-DUE",
        "LZ-MESSAGE-OWNER-LOST",
        "deadLetterEvidence",
        "restartDurability",
        "auditOutboxEvidence",
        "schemaVersion 5",
        "providerSmokeAbsent",
        "desktopCapabilityExpansionAbsent",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in [
        "provider', 'smoke",
        "session', 'warm",
        "conpty",
        "worktree",
        "apps\\lazoo-desktop",
    ] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

#[test]
fn conversation_delivery_acceptance_contract_is_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/conversation-delivery.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("conversation delivery acceptance script");
    let fixture_path = repository.join("tests/fixtures/turn-transport/src/main.rs");
    let fixture = std::fs::read_to_string(&fixture_path).expect("turn transport fixture");

    for required in [
        "schema5",
        "projectlessConversation",
        "projectConversation",
        "userAuthorship",
        "employeeAuthorship",
        "recordFirstTurn",
        "envelopeV2",
        "activationExplicit",
        "employeeActivationBlocked",
        "nativeTurnIdBeforeAcknowledgement",
        "progressDurable",
        "nativeTerminalMapping",
        "retryIdentityPreserved",
        "cancelEvidenceRequired",
        "ownerLossUncertain",
        "transcriptBounded",
        "auditOutboxEvidence",
        "modelInvocationAbsent",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "outerCodexContextIsolated",
        "cleanProviderTreeStop",
        "desktopCapabilityExpansionAbsent",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in [
        "provider', 'smoke",
        "apps\\lazoo-desktop",
        "conpty",
        "worktree",
    ] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
    assert!(!script.contains("LAZOO_TURN_OK"));
    assert!(fixture.contains("turn/start"));
    assert!(fixture.contains("turn/interrupt"));
    assert!(fixture.contains("FAIL_AFTER_ACCEPTANCE"));
    assert!(fixture.contains("thread/tokenUsage/updated"));
    assert!(fixture.contains("thread/start"));
    assert!(fixture.contains("thread/resume"));
    assert!(fixture.contains("--version"));
}
