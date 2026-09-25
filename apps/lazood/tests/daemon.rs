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
    DiscoveryEnvironment, PermissionDecision, PermissionKind, PermissionRequest, ProviderError,
    ProviderEvent, ProviderKind, SessionRequest, ToolCall, ToolResult, TurnEventObserver,
    TurnRequest,
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

fn git(path: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git command failed: {args:?}");
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
    cancel_releases_event: Arc<AtomicBool>,
    gate_events: Arc<AtomicBool>,
    event_permits: Arc<Semaphore>,
    gate_start: Arc<AtomicBool>,
    start_permits: Arc<Semaphore>,
    gate_turn_ready: Arc<AtomicBool>,
    turn_ready_permits: Arc<Semaphore>,
    turn_events: Arc<Mutex<Vec<ProviderEvent>>>,
    turn_requests: Arc<Mutex<Vec<TurnRequest>>>,
    turn_failure: Arc<Mutex<Option<String>>>,
    turn_retryable_failure: Arc<AtomicBool>,
    working_directories: Arc<Mutex<Vec<PathBuf>>>,
    permission_requests: Arc<Mutex<Vec<PermissionRequest>>>,
    permission_decisions: Arc<Mutex<Vec<PermissionDecision>>>,
    /// Scripted Lazoo tool calls; the first turn that runs drains and issues them after its
    /// first scripted event, mirroring a model calling tools once the native turn is accepted.
    tool_calls: Arc<Mutex<Vec<ToolCall>>>,
    tool_results: Arc<Mutex<Vec<ToolResult>>>,
    session_tools: Arc<Mutex<Vec<Vec<String>>>>,
}

impl Default for FakeSessionBackend {
    fn default() -> Self {
        Self {
            starts: Arc::new(AtomicUsize::new(0)),
            stops: Arc::new(AtomicUsize::new(0)),
            turns: Arc::new(AtomicUsize::new(0)),
            cancels: Arc::new(AtomicUsize::new(0)),
            cancel_releases_event: Arc::new(AtomicBool::new(false)),
            gate_events: Arc::new(AtomicBool::new(false)),
            event_permits: Arc::new(Semaphore::new(0)),
            gate_start: Arc::new(AtomicBool::new(false)),
            start_permits: Arc::new(Semaphore::new(0)),
            gate_turn_ready: Arc::new(AtomicBool::new(false)),
            turn_ready_permits: Arc::new(Semaphore::new(0)),
            turn_events: Arc::new(Mutex::new(Vec::new())),
            turn_requests: Arc::new(Mutex::new(Vec::new())),
            turn_failure: Arc::new(Mutex::new(None)),
            turn_retryable_failure: Arc::new(AtomicBool::new(false)),
            working_directories: Arc::new(Mutex::new(Vec::new())),
            permission_requests: Arc::new(Mutex::new(Vec::new())),
            permission_decisions: Arc::new(Mutex::new(Vec::new())),
            tool_calls: Arc::new(Mutex::new(Vec::new())),
            tool_results: Arc::new(Mutex::new(Vec::new())),
            session_tools: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

struct FakeManagedSession {
    id: String,
    stops: Arc<AtomicUsize>,
    turns: Arc<AtomicUsize>,
    cancels: Arc<AtomicUsize>,
    cancel_releases_event: Arc<AtomicBool>,
    gate_events: Arc<AtomicBool>,
    event_permits: Arc<Semaphore>,
    gate_turn_ready: Arc<AtomicBool>,
    turn_ready_permits: Arc<Semaphore>,
    turn_events: Arc<Mutex<Vec<ProviderEvent>>>,
    turn_requests: Arc<Mutex<Vec<TurnRequest>>>,
    turn_failure: Arc<Mutex<Option<String>>>,
    turn_retryable_failure: Arc<AtomicBool>,
    permission_requests: Arc<Mutex<Vec<PermissionRequest>>>,
    permission_decisions: Arc<Mutex<Vec<PermissionDecision>>>,
    tool_calls: Arc<Mutex<Vec<ToolCall>>>,
    tool_results: Arc<Mutex<Vec<ToolResult>>>,
}

impl ManagedSession for FakeManagedSession {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn native_host_pid(&self) -> Option<u32> {
        Some(4242)
    }

    fn ready_for_turn(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        Box::pin(async move {
            if self.gate_turn_ready.load(Ordering::SeqCst) {
                self.turn_ready_permits.acquire().await.unwrap().forget();
            }
            Ok(())
        })
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
            let permissions = self.permission_requests.lock().unwrap().clone();
            for permission in permissions {
                let decision = observer.decide_permission(&permission).await?;
                self.permission_decisions.lock().unwrap().push(decision);
            }
            let scripted = self.turn_events.lock().unwrap().clone();
            let mut returned = Vec::with_capacity(scripted.len());
            for (index, event) in scripted.into_iter().enumerate() {
                if self.gate_events.load(Ordering::SeqCst) {
                    self.event_permits.acquire().await.unwrap().forget();
                }
                observer.observe(&event)?;
                returned.push(event);
                if index == 0 {
                    // Each session issues only the calls scripted for it; `$message-id` stands
                    // for the id a live model would read from its last tool result or prompt.
                    let calls = {
                        let mut queued = self.tool_calls.lock().unwrap();
                        let (mine, others): (Vec<_>, Vec<_>) = std::mem::take(&mut *queued)
                            .into_iter()
                            .partition(|call| call.session_id == self.id);
                        *queued = others;
                        mine
                    };
                    for mut call in calls {
                        let last_result = self.tool_results.lock().unwrap().last().cloned();
                        substitute_message_id(&mut call, &request.prompt, last_result.as_ref());
                        let result = observer.call_tool(&call).await?;
                        self.tool_results.lock().unwrap().push(result);
                    }
                }
            }
            Ok(returned)
        })
    }

    fn cancel(&self) -> Pin<Box<dyn Future<Output = Result<(), ProviderError>> + Send + '_>> {
        Box::pin(async move {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            if self.cancel_releases_event.load(Ordering::SeqCst) {
                self.event_permits.add_permits(1);
            }
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
            if self.gate_start.load(Ordering::SeqCst) {
                self.start_permits.acquire().await.unwrap().forget();
            }
            self.working_directories
                .lock()
                .unwrap()
                .push(request.cwd.clone());
            self.session_tools
                .lock()
                .unwrap()
                .push(request.tools.iter().map(|tool| tool.name.clone()).collect());
            Ok(Box::new(FakeManagedSession {
                id: format!("native-{number}"),
                stops: Arc::clone(&self.stops),
                turns: Arc::clone(&self.turns),
                cancels: Arc::clone(&self.cancels),
                cancel_releases_event: Arc::clone(&self.cancel_releases_event),
                gate_events: Arc::clone(&self.gate_events),
                event_permits: Arc::clone(&self.event_permits),
                gate_turn_ready: Arc::clone(&self.gate_turn_ready),
                turn_ready_permits: Arc::clone(&self.turn_ready_permits),
                turn_events: Arc::clone(&self.turn_events),
                turn_requests: Arc::clone(&self.turn_requests),
                turn_failure: Arc::clone(&self.turn_failure),
                turn_retryable_failure: Arc::clone(&self.turn_retryable_failure),
                permission_requests: Arc::clone(&self.permission_requests),
                permission_decisions: Arc::clone(&self.permission_decisions),
                tool_calls: Arc::clone(&self.tool_calls),
                tool_results: Arc::clone(&self.tool_results),
            }) as Box<dyn ManagedSession>)
        })
    }
}

/// Replaces `"$message-id"` arguments with the message id from the last tool result's inbox
/// (`messages[0].messageId`) or, failing that, from the turn prompt's `message id <uuid>`.
fn substitute_message_id(call: &mut ToolCall, prompt: &str, last_result: Option<&ToolResult>) {
    let Some(arguments) = call.arguments.as_object_mut() else {
        return;
    };
    if !arguments.values().any(|value| value == "$message-id") {
        return;
    }
    let from_result = last_result.and_then(|result| {
        result.content["messages"][0]["messageId"]
            .as_str()
            .map(str::to_owned)
    });
    let from_prompt = prompt.split("message id ").nth(1).map(|rest| {
        rest.chars()
            .take_while(|character| character.is_ascii_hexdigit() || *character == '-')
            .collect::<String>()
    });
    let Some(id) = from_result.or(from_prompt) else {
        return;
    };
    for value in arguments.values_mut() {
        if value == "$message-id" {
            *value = json!(id);
        }
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

#[tokio::test]
#[allow(clippy::too_many_lines)] // One fixture proves dispatch, retained evidence, wake-up, and session cleanup.
async fn accepted_projectless_child_dispatches_automatically_through_the_canonical_message() {
    use lazoo_protocol::{
        MessageSender, TaskDispatchState, TaskState, UserMessageEndpoint, UserMessageKind,
    };
    use lazoo_store::{BeginSessionInput, MarkSessionReadyInput, RecordTurnInput};

    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee_input = |name: &str, key: &str| CreateEmployeeInput {
        request_id: format!("{key}-request"),
        idempotency_key: key.to_owned(),
        name: name.to_owned(),
        provider: "codex".to_owned(),
        role: "Worker".to_owned(),
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
            checked_at: "2026-09-12T00:00:00Z".to_owned(),
        }),
    };
    let lead = store
        .create_employee(&employee_input("Rhea", "dispatch-rhea"))
        .unwrap()
        .employee;
    let worker = store
        .create_employee(&employee_input("Mira", "dispatch-mira"))
        .unwrap()
        .employee;
    let conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "dispatch-conversation-request".to_owned(),
            idempotency_key: "dispatch-conversation".to_owned(),
            project_id: None,
            lead_employee_id: lead.id,
            title: None,
        })
        .unwrap()
        .conversation;
    let root_task = store
        .record_turn(&RecordTurnInput {
            request_id: "dispatch-root-request".to_owned(),
            idempotency_key: "dispatch-root".to_owned(),
            conversation_id: conversation.id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::FullAutonomy,
            text: "Lead objective".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap()
        .task
        .unwrap();
    let boot_id = uuid::Uuid::new_v4();
    let lead_starting = store
        .begin_session(&BeginSessionInput {
            request_id: "dispatch-lead-session-request".to_owned(),
            idempotency_key: "dispatch-lead-session".to_owned(),
            employee_id: lead.id,
            expected_employee_revision: lead.configuration_revision,
            owner_boot_id: boot_id,
        })
        .unwrap();
    let lead_session = store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: lead_starting.session.id,
            expected_revision: lead_starting.session.revision,
            owner_boot_id: boot_id,
            native_session_id: "native-lead".to_owned(),
            native_host_pid: 4241,
        })
        .unwrap()
        .session;
    let backend = FakeSessionBackend::default();
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-2".to_owned(),
            turn_id: Some("native-dispatched-turn".to_owned()),
            raw: None,
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-2".to_owned(),
            turn_id: Some("native-dispatched-turn".to_owned()),
            status: "completed".to_owned(),
            output: Some("worker result".to_owned()),
            raw: None,
        },
    ]);
    let app = App::new_with_session_backend(
        store.clone(),
        root.clone(),
        DiscoveryEnvironment::empty(),
        Arc::new(backend.clone()),
    )
    .unwrap();
    let busy = app
        .handle(request(
            "dispatch-worker-busy",
            "session.warm",
            json!({
                "employeeId": worker.id,
                "expectedEmployeeRevision": worker.configuration_revision,
                "idempotencyKey": "dispatch-worker-busy"
            }),
        ))
        .await
        .result
        .unwrap();
    let delegated = app
        .handle(request(
            "dispatch-child-request",
            "task.delegate",
            json!({
                "parentTaskId": root_task.id,
                "actor": {
                    "employeeId": lead.id,
                    "sessionId": lead_session.id
                },
                "assignedEmployeeId": worker.id,
                "objective": "Implement the parser fix",
                "references": [],
                "requestedAuthority": "self-decision",
                "rosterRevision": store.roster_snapshot().unwrap().revision,
                "idempotencyKey": "dispatch-child"
            }),
        ))
        .await
        .result
        .unwrap();
    let task_id: uuid::Uuid = delegated["task"]["id"].as_str().unwrap().parse().unwrap();
    let message_id = delegated["delegationMessage"]["envelope"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    app.dispatch_pending_tasks_once().await.unwrap();
    assert!(store.get_task(task_id).unwrap().dispatch.is_none());
    let dispatcher_app = app.clone();
    let dispatcher = tokio::spawn(async move { dispatcher_app.run_task_dispatcher().await });
    app.handle(request(
        "dispatch-worker-release",
        "session.stop",
        json!({
            "id": busy["session"]["id"],
            "expectedRevision": busy["session"]["revision"],
            "idempotencyKey": "dispatch-worker-release"
        }),
    ))
    .await
    .result
    .unwrap();

    let completed = timeout(Duration::from_secs(5), async {
        loop {
            if store.get_task(task_id).unwrap().task.state == TaskState::Completed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        completed.is_ok(),
        "task detail after wake: {:?}; starts {}; stops {}; turns {}",
        store.get_task(task_id).unwrap(),
        backend.starts.load(Ordering::SeqCst),
        backend.stops.load(Ordering::SeqCst),
        backend.turns.load(Ordering::SeqCst)
    );

    let detail = store.get_task(task_id).unwrap();
    assert_eq!(detail.task.state, TaskState::Completed);
    let dispatch = detail.dispatch.unwrap();
    assert_eq!(dispatch.state, TaskDispatchState::TurnRecorded);
    assert_eq!(
        dispatch.workspace_path,
        Some(
            root.projectless_task_working_dir(task_id)
                .to_string_lossy()
                .into_owned()
        )
    );
    assert_eq!(
        store.get_message(message_id).unwrap().delivery.state,
        MessageDeliveryState::Acknowledged
    );
    assert_eq!(backend.starts.load(Ordering::SeqCst), 2);
    assert_eq!(backend.turns.load(Ordering::SeqCst), 1);
    assert_eq!(backend.stops.load(Ordering::SeqCst), 2);
    assert_eq!(
        backend.working_directories.lock().unwrap().last(),
        Some(&root.projectless_task_working_dir(task_id))
    );
    dispatcher.abort();
    let _ = dispatcher.await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One fixture exercises independent pre-provider failure and cancellation boundaries.
async fn automatic_dispatch_configuration_drift_terminalizes_before_provider_io() {
    use lazoo_protocol::{
        MessageSender, TaskDispatchState, TaskState, UserMessageEndpoint, UserMessageKind,
    };
    use lazoo_store::{
        BeginSessionInput, MarkSessionReadyInput, RecordTurnInput, UpdateEmployeeInput,
    };

    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee_input = |name: &str, key: &str, provider: &str| CreateEmployeeInput {
        request_id: format!("{key}-request"),
        idempotency_key: key.to_owned(),
        name: name.to_owned(),
        provider: provider.to_owned(),
        role: "Worker".to_owned(),
        instructions: String::new(),
        model: Some("test-model".to_owned()),
        reasoning_effort: Some("low".to_owned()),
        native_settings: json!({}),
        authority_mode: AuthorityMode::FullAutonomy,
        checkout_policy: EmployeeCheckoutPolicy::Isolated,
        enabled: true,
        provider_observation: Some(ProviderObservation {
            provider: provider.to_owned(),
            verification: "configured".to_owned(),
            version: Some("test".to_owned()),
            protocol: Some("test".to_owned()),
            capabilities: json!({"session": true}),
            authentication: json!({"native": "ready"}),
            degradation_reason: None,
            checked_at: "2026-09-12T00:00:00Z".to_owned(),
        }),
    };
    let lead = store
        .create_employee(&employee_input("Rhea", "dispatch-failure-rhea", "codex"))
        .unwrap()
        .employee;
    let worker = store
        .create_employee(&employee_input("Mira", "dispatch-failure-mira", "codex"))
        .unwrap()
        .employee;
    let healthy_worker = store
        .create_employee(&employee_input("Noor", "dispatch-failure-healthy", "codex"))
        .unwrap()
        .employee;
    let conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "dispatch-failure-conversation-request".to_owned(),
            idempotency_key: "dispatch-failure-conversation".to_owned(),
            project_id: None,
            lead_employee_id: lead.id,
            title: None,
        })
        .unwrap()
        .conversation;
    let root_task = store
        .record_turn(&RecordTurnInput {
            request_id: "dispatch-failure-root-request".to_owned(),
            idempotency_key: "dispatch-failure-root".to_owned(),
            conversation_id: conversation.id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::FullAutonomy,
            text: "Lead objective".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap()
        .task
        .unwrap();
    let boot_id = uuid::Uuid::new_v4();
    let lead_starting = store
        .begin_session(&BeginSessionInput {
            request_id: "dispatch-failure-lead-session-request".to_owned(),
            idempotency_key: "dispatch-failure-lead-session".to_owned(),
            employee_id: lead.id,
            expected_employee_revision: lead.configuration_revision,
            owner_boot_id: boot_id,
        })
        .unwrap();
    let lead_session = store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: lead_starting.session.id,
            expected_revision: lead_starting.session.revision,
            owner_boot_id: boot_id,
            native_session_id: "native-failure-lead".to_owned(),
            native_host_pid: 4241,
        })
        .unwrap()
        .session;
    let backend = FakeSessionBackend::default();
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("native-healthy-sibling".to_owned()),
            raw: None,
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("native-healthy-sibling".to_owned()),
            status: "completed".to_owned(),
            output: Some("healthy sibling result".to_owned()),
            raw: None,
        },
    ]);
    let app = App::new_with_session_backend(
        store.clone(),
        root,
        DiscoveryEnvironment::empty(),
        Arc::new(backend.clone()),
    )
    .unwrap();
    let delegated = app
        .handle(request(
            "dispatch-failure-child-request",
            "task.delegate",
            json!({
                "parentTaskId": root_task.id,
                "actor": {"employeeId": lead.id, "sessionId": lead_session.id},
                "assignedEmployeeId": worker.id,
                "objective": "Use the unsupported worker",
                "references": [],
                "requestedAuthority": "self-decision",
                "rosterRevision": store.roster_snapshot().unwrap().revision,
                "idempotencyKey": "dispatch-failure-child"
            }),
        ))
        .await
        .result
        .unwrap();
    let task_id: uuid::Uuid = delegated["task"]["id"].as_str().unwrap().parse().unwrap();
    let message_id: uuid::Uuid = delegated["delegationMessage"]["envelope"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    store
        .update_employee(&UpdateEmployeeInput {
            request_id: "dispatch-drift-request".to_owned(),
            idempotency_key: "dispatch-drift".to_owned(),
            employee_id: worker.id,
            expected_revision: worker.configuration_revision,
            name: None,
            provider: None,
            role: Some("Updated worker".to_owned()),
            instructions: None,
            model: None,
            reasoning_effort: None,
            native_settings: None,
            authority_mode: None,
            checkout_policy: None,
            enabled: None,
            provider_observation: None,
        })
        .unwrap();
    let healthy = app
        .handle(request(
            "dispatch-healthy-child-request",
            "task.delegate",
            json!({
                "parentTaskId": root_task.id,
                "actor": {"employeeId": lead.id, "sessionId": lead_session.id},
                "assignedEmployeeId": healthy_worker.id,
                "objective": "Dispatch the healthy sibling",
                "references": [],
                "requestedAuthority": "self-decision",
                "rosterRevision": store.roster_snapshot().unwrap().revision,
                "idempotencyKey": "dispatch-healthy-child"
            }),
        ))
        .await
        .result
        .unwrap();
    let healthy_task_id: uuid::Uuid = healthy["task"]["id"].as_str().unwrap().parse().unwrap();

    let error = app.dispatch_pending_tasks_once().await.unwrap_err();
    assert_eq!(error.code, "LZ-TASK-EMPLOYEE-STALE");
    let detail = store.get_task(task_id).unwrap();
    assert_eq!(detail.task.state, TaskState::Failed);
    assert_eq!(
        detail.task.failure_code.as_deref(),
        Some(error.code.as_str())
    );
    assert_eq!(detail.dispatch.unwrap().state, TaskDispatchState::Failed);
    assert_eq!(
        store.get_message(message_id).unwrap().delivery.state,
        MessageDeliveryState::TerminalFailed
    );
    assert_eq!(
        store.get_task(healthy_task_id).unwrap().task.state,
        TaskState::Completed
    );
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    assert_eq!(backend.turns.load(Ordering::SeqCst), 1);

    backend.gate_start.store(true, Ordering::SeqCst);
    let cancelled = app
        .handle(request(
            "dispatch-cancel-child-request",
            "task.delegate",
            json!({
                "parentTaskId": root_task.id,
                "actor": {"employeeId": lead.id, "sessionId": lead_session.id},
                "assignedEmployeeId": healthy_worker.id,
                "objective": "Cancel after session launch begins",
                "references": [],
                "requestedAuthority": "self-decision",
                "rosterRevision": store.roster_snapshot().unwrap().revision,
                "idempotencyKey": "dispatch-cancel-child"
            }),
        ))
        .await
        .result
        .unwrap();
    let cancelled_task_id: uuid::Uuid = cancelled["task"]["id"].as_str().unwrap().parse().unwrap();
    let cancelled_message_id: uuid::Uuid = cancelled["delegationMessage"]["envelope"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let dispatch_app = app.clone();
    let pending = tokio::spawn(async move { dispatch_app.dispatch_pending_tasks_once().await });
    timeout(Duration::from_secs(2), async {
        while backend.starts.load(Ordering::SeqCst) != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    app.handle(request(
        "dispatch-cancel-request",
        "task.cancel",
        json!({
            "id": cancelled_task_id,
            "expectedRevision": cancelled["task"]["revision"],
            "actor": {"kind": "user"},
            "evidence": {"reason": "acceptance race"},
            "idempotencyKey": "dispatch-cancel"
        }),
    ))
    .await
    .result
    .unwrap();
    backend.start_permits.add_permits(1);
    assert!(pending.await.unwrap().is_err());
    let cancelled_detail = store.get_task(cancelled_task_id).unwrap();
    assert_eq!(cancelled_detail.task.state, TaskState::Cancelled);
    assert_eq!(
        cancelled_detail.dispatch.unwrap().state,
        TaskDispatchState::Cancelled
    );
    assert_eq!(
        store
            .get_message(cancelled_message_id)
            .unwrap()
            .delivery
            .state,
        MessageDeliveryState::Cancelled
    );
    assert_eq!(backend.stops.load(Ordering::SeqCst), 2);

    backend.gate_start.store(false, Ordering::SeqCst);
    backend.gate_events.store(true, Ordering::SeqCst);
    backend.cancel_releases_event.store(true, Ordering::SeqCst);
    *backend.turn_events.lock().unwrap() = vec![
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-3".to_owned(),
            turn_id: Some("native-cancel-running".to_owned()),
            raw: None,
        },
        ProviderEvent::Cancelled {
            provider: ProviderKind::Codex,
            session_id: "native-3".to_owned(),
            turn_id: Some("native-cancel-running".to_owned()),
            raw: None,
        },
    ];
    let running = app
        .handle(request(
            "dispatch-running-cancel-child-request",
            "task.delegate",
            json!({
                "parentTaskId": root_task.id,
                "actor": {"employeeId": lead.id, "sessionId": lead_session.id},
                "assignedEmployeeId": healthy_worker.id,
                "objective": "Cancel after native acceptance",
                "references": [],
                "requestedAuthority": "self-decision",
                "rosterRevision": store.roster_snapshot().unwrap().revision,
                "idempotencyKey": "dispatch-running-cancel-child"
            }),
        ))
        .await
        .result
        .unwrap();
    let running_task_id: uuid::Uuid = running["task"]["id"].as_str().unwrap().parse().unwrap();
    let dispatch_app = app.clone();
    let running_dispatch =
        tokio::spawn(async move { dispatch_app.dispatch_pending_tasks_once().await });
    backend.event_permits.add_permits(1);
    timeout(Duration::from_secs(2), async {
        while store.get_task(running_task_id).unwrap().task.state != TaskState::Running {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let running_detail = store.get_task(running_task_id).unwrap();
    let cancelled_running = app
        .handle(request(
            "dispatch-running-cancel-request",
            "task.cancel",
            json!({
                "id": running_task_id,
                "expectedRevision": running_detail.task.revision,
                "actor": {"kind": "user"},
                "evidence": {"reason": "stop automatic worker"},
                "idempotencyKey": "dispatch-running-cancel"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(cancelled_running["task"]["state"], "cancelled");
    assert!(running_dispatch.await.unwrap().is_ok());
    assert_eq!(backend.cancels.load(Ordering::SeqCst), 1);
    assert_eq!(backend.stops.load(Ordering::SeqCst), 3);

    backend.gate_turn_ready.store(true, Ordering::SeqCst);
    let submitting = app
        .handle(request(
            "dispatch-submitting-cancel-child-request",
            "task.delegate",
            json!({
                "parentTaskId": root_task.id,
                "actor": {"employeeId": lead.id, "sessionId": lead_session.id},
                "assignedEmployeeId": healthy_worker.id,
                "objective": "Cancel during native submission",
                "references": [],
                "requestedAuthority": "self-decision",
                "rosterRevision": store.roster_snapshot().unwrap().revision,
                "idempotencyKey": "dispatch-submitting-cancel-child"
            }),
        ))
        .await
        .result
        .unwrap();
    let submitting_task_id: uuid::Uuid =
        submitting["task"]["id"].as_str().unwrap().parse().unwrap();
    let dispatch_app = app.clone();
    let submitting_dispatch =
        tokio::spawn(async move { dispatch_app.dispatch_pending_tasks_once().await });
    timeout(Duration::from_secs(2), async {
        loop {
            let detail = store.get_task(submitting_task_id).unwrap();
            if let Some(turn_id) = detail.dispatch.and_then(|dispatch| dispatch.turn_id)
                && store.get_turn(turn_id).unwrap().state == TurnState::Submitting
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let submitting_detail = store.get_task(submitting_task_id).unwrap();
    let cancelled_submitting = app
        .handle(request(
            "dispatch-submitting-cancel-request",
            "task.cancel",
            json!({
                "id": submitting_task_id,
                "expectedRevision": submitting_detail.task.revision,
                "actor": {"kind": "user"},
                "evidence": {"reason": "stop submitting automatic worker"},
                "idempotencyKey": "dispatch-submitting-cancel"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(cancelled_submitting["task"]["state"], "cancelled");
    assert_eq!(backend.turns.load(Ordering::SeqCst), 2);
    assert_eq!(backend.cancels.load(Ordering::SeqCst), 1);
    backend.turn_ready_permits.add_permits(1);
    assert!(submitting_dispatch.await.unwrap().is_ok());
    assert_eq!(backend.stops.load(Ordering::SeqCst), 4);

    backend.gate_turn_ready.store(false, Ordering::SeqCst);
    *backend.turn_events.lock().unwrap() = vec![
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-5".to_owned(),
            turn_id: Some("native-completion-wins".to_owned()),
            raw: None,
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-5".to_owned(),
            turn_id: Some("native-completion-wins".to_owned()),
            status: "completed".to_owned(),
            output: Some("completion won the race".to_owned()),
            raw: None,
        },
    ];
    let completing = app
        .handle(request(
            "dispatch-completion-race-child-request",
            "task.delegate",
            json!({
                "parentTaskId": root_task.id,
                "actor": {"employeeId": lead.id, "sessionId": lead_session.id},
                "assignedEmployeeId": healthy_worker.id,
                "objective": "Let native completion win its cancellation race",
                "references": [],
                "requestedAuthority": "self-decision",
                "rosterRevision": store.roster_snapshot().unwrap().revision,
                "idempotencyKey": "dispatch-completion-race-child"
            }),
        ))
        .await
        .result
        .unwrap();
    let completing_task_id: uuid::Uuid =
        completing["task"]["id"].as_str().unwrap().parse().unwrap();
    let dispatch_app = app.clone();
    let completing_dispatch =
        tokio::spawn(async move { dispatch_app.dispatch_pending_tasks_once().await });
    backend.event_permits.add_permits(1);
    timeout(Duration::from_secs(2), async {
        while store.get_task(completing_task_id).unwrap().task.state != TaskState::Running {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let completing_detail = store.get_task(completing_task_id).unwrap();
    let completion_won = app
        .handle(request(
            "dispatch-completion-race-cancel-request",
            "task.cancel",
            json!({
                "id": completing_task_id,
                "expectedRevision": completing_detail.task.revision,
                "actor": {"kind": "user"},
                "evidence": {"reason": "completion may already be arriving"},
                "idempotencyKey": "dispatch-completion-race-cancel"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(completion_won["task"]["state"], "completed");
    assert!(completing_dispatch.await.unwrap().is_ok());
    assert_eq!(backend.turns.load(Ordering::SeqCst), 3);
    assert_eq!(backend.cancels.load(Ordering::SeqCst), 2);
    assert_eq!(backend.stops.load(Ordering::SeqCst), 5);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // The Git isolation contract needs repository setup and end-to-end assertions.
async fn project_task_dispatch_uses_an_isolated_worktree_and_preserves_dirty_user_files() {
    use lazoo_protocol::{
        MessageSender, TaskDispatchState, TaskState, TaskWorkspaceKind, UserMessageEndpoint,
        UserMessageKind,
    };
    use lazoo_store::{
        BeginSessionInput, MarkSessionReadyInput, RecordTurnInput, RegisterProjectInput,
    };

    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let repository = temporary.path().join("repository");
    git_init(&repository);
    git(
        &repository,
        &["config", "user.email", "lazoo@example.invalid"],
    );
    git(&repository, &["config", "user.name", "Lazoo Test"]);
    git(&repository, &["config", "core.autocrlf", "false"]);
    std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
    git(&repository, &["add", "tracked.txt"]);
    git(&repository, &["commit", "--quiet", "-m", "base"]);
    std::fs::write(repository.join("tracked.txt"), "dirty user edit\n").unwrap();

    let store = Store::open_path(root.state_db()).unwrap();
    let project = store
        .register_project(&RegisterProjectInput {
            request_id: "dispatch-project-request".to_owned(),
            idempotency_key: "dispatch-project".to_owned(),
            repository_identity: "dispatch-project-identity".to_owned(),
            canonical_path: repository.clone(),
            display_name: "Dispatch fixture".to_owned(),
            checkout_mode: CheckoutMode::Isolated,
        })
        .unwrap()
        .project;
    let employee_input = |name: &str, key: &str| CreateEmployeeInput {
        request_id: format!("{key}-request"),
        idempotency_key: key.to_owned(),
        name: name.to_owned(),
        provider: "codex".to_owned(),
        role: "Worker".to_owned(),
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
            authentication: json!({"native": "ready"}),
            degradation_reason: None,
            checked_at: "2026-09-12T00:00:00Z".to_owned(),
        }),
    };
    let lead = store
        .create_employee(&employee_input("Rhea", "dispatch-project-rhea"))
        .unwrap()
        .employee;
    let worker = store
        .create_employee(&employee_input("Mira", "dispatch-project-mira"))
        .unwrap()
        .employee;
    let conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "dispatch-project-conversation-request".to_owned(),
            idempotency_key: "dispatch-project-conversation".to_owned(),
            project_id: Some(project.id),
            lead_employee_id: lead.id,
            title: None,
        })
        .unwrap()
        .conversation;
    let root_task = store
        .record_turn(&RecordTurnInput {
            request_id: "dispatch-project-root-request".to_owned(),
            idempotency_key: "dispatch-project-root".to_owned(),
            conversation_id: conversation.id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::FullAutonomy,
            text: "Lead project objective".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap()
        .task
        .unwrap();
    let boot_id = uuid::Uuid::new_v4();
    let lead_starting = store
        .begin_session(&BeginSessionInput {
            request_id: "dispatch-project-lead-session-request".to_owned(),
            idempotency_key: "dispatch-project-lead-session".to_owned(),
            employee_id: lead.id,
            expected_employee_revision: lead.configuration_revision,
            owner_boot_id: boot_id,
        })
        .unwrap();
    let lead_session = store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: lead_starting.session.id,
            expected_revision: lead_starting.session.revision,
            owner_boot_id: boot_id,
            native_session_id: "native-project-lead".to_owned(),
            native_host_pid: 4241,
        })
        .unwrap()
        .session;
    let backend = FakeSessionBackend::default();
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("native-project-turn".to_owned()),
            raw: None,
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("native-project-turn".to_owned()),
            status: "completed".to_owned(),
            output: Some("project worker result".to_owned()),
            raw: None,
        },
    ]);
    let app = App::new_with_session_backend(
        store.clone(),
        root.clone(),
        DiscoveryEnvironment::empty(),
        Arc::new(backend.clone()),
    )
    .unwrap();
    let delegated = app
        .handle(request(
            "dispatch-project-child-request",
            "task.delegate",
            json!({
                "parentTaskId": root_task.id,
                "actor": {"employeeId": lead.id, "sessionId": lead_session.id},
                "assignedEmployeeId": worker.id,
                "objective": "Implement in isolation",
                "references": [],
                "requestedAuthority": "self-decision",
                "rosterRevision": store.roster_snapshot().unwrap().revision,
                "idempotencyKey": "dispatch-project-child"
            }),
        ))
        .await
        .result
        .unwrap();
    let task_id: uuid::Uuid = delegated["task"]["id"].as_str().unwrap().parse().unwrap();

    app.dispatch_pending_tasks_once().await.unwrap();

    let detail = store.get_task(task_id).unwrap();
    assert_eq!(detail.task.state, TaskState::Completed);
    let dispatch = detail.dispatch.unwrap();
    assert_eq!(dispatch.state, TaskDispatchState::TurnRecorded);
    assert_eq!(
        dispatch.workspace_kind,
        Some(TaskWorkspaceKind::GitWorktree)
    );
    let task_worktree = root.git_task_working_dir(project.id, task_id);
    assert_eq!(
        dispatch.workspace_path.as_deref(),
        Some(task_worktree.to_string_lossy().as_ref())
    );
    assert_eq!(
        backend.working_directories.lock().unwrap().as_slice(),
        std::slice::from_ref(&task_worktree)
    );
    assert_eq!(
        std::fs::read_to_string(repository.join("tracked.txt")).unwrap(),
        "dirty user edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(task_worktree.join("tracked.txt")).unwrap(),
        "base\n"
    );
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
async fn task_rpc_reads_are_available_without_changing_the_desktop_contract() {
    let (_temporary, app) = test_app();
    let mut before = app
        .handle(request(
            "desktop-before-tasks",
            "desktop.snapshot",
            json!({}),
        ))
        .await
        .result
        .unwrap();

    let listed = app
        .handle(request(
            "task-list-empty",
            "task.list",
            json!({
                "afterSequence": 0,
                "limit": 100,
                "rootTaskId": null,
                "parentTaskId": null,
                "conversationId": null,
                "projectId": null,
                "assignedEmployeeId": null,
                "state": null
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(listed, json!([]));

    let mut after = app
        .handle(request(
            "desktop-after-tasks",
            "desktop.snapshot",
            json!({}),
        ))
        .await
        .result
        .unwrap();
    before.as_object_mut().unwrap().remove("capturedAt");
    after.as_object_mut().unwrap().remove("capturedAt");
    before["status"]
        .as_object_mut()
        .unwrap()
        .remove("uptimeMillis");
    after["status"]
        .as_object_mut()
        .unwrap()
        .remove("uptimeMillis");
    assert_eq!(after, before);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Keep all eight RPCs and their shared durable identities together.
async fn task_rpc_workflow_is_durable_idempotent_ordered_and_provider_neutral() {
    use lazoo_protocol::{MessageEndpoint, MessageSender, UserMessageEndpoint, UserMessageKind};
    use lazoo_store::{BeginTurnSubmissionInput, ObserveTurnInput, RecordTurnInput};

    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let lead = configured_employee_named(&store, "Rhea", "task-rhea");
    let worker = configured_employee_named(&store, "Mira", "task-mira");
    let conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "task-conversation-request".to_owned(),
            idempotency_key: "task-conversation".to_owned(),
            project_id: None,
            lead_employee_id: lead.id,
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
    for employee in [&lead, &worker] {
        app.handle(request(
            &format!("warm-task-{}", employee.id),
            "session.warm",
            json!({
                "employeeId": employee.id,
                "expectedEmployeeRevision": employee.configuration_revision,
                "idempotencyKey": format!("warm-task-{}", employee.id)
            }),
        ))
        .await
        .result
        .unwrap();
    }
    let sessions = store.list_sessions().unwrap();
    let lead_session = sessions
        .iter()
        .find(|item| item.employee_id == lead.id)
        .unwrap();
    let worker_session = sessions
        .iter()
        .find(|item| item.employee_id == worker.id)
        .unwrap();
    let lead_endpoint = MessageEndpoint {
        employee_id: lead.id,
        session_id: Some(lead_session.id),
    };
    let worker_endpoint = MessageEndpoint {
        employee_id: worker.id,
        session_id: Some(worker_session.id),
    };
    let root_turn = store
        .record_turn(&RecordTurnInput {
            request_id: "task-root-request".to_owned(),
            idempotency_key: "task-root".to_owned(),
            conversation_id: conversation.id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: Some(lead_endpoint),
            task_id: None,
            authority_mode: AuthorityMode::Ask,
            text: "root objective".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap();
    let root_task_id = root_turn.task.as_ref().unwrap().id;
    let desktop_before = app
        .handle(request(
            "task-desktop-before",
            "desktop.snapshot",
            json!({}),
        ))
        .await
        .result
        .unwrap();

    let self_delegation = app
        .handle(request(
            "task-self-delegation",
            "task.delegate",
            json!({
                "parentTaskId": root_task_id,
                "actor": lead_endpoint,
                "assignedEmployeeId": lead.id,
                "objective": "invalid self delegation",
                "references": [],
                "requestedAuthority": "ask",
                "rosterRevision": store.roster_snapshot().unwrap().revision,
                "idempotencyKey": "task-self-delegation"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(self_delegation.code, "LZ-TASK-SELF-DELEGATION");

    let delegate_params = json!({
        "parentTaskId": root_task_id,
        "actor": lead_endpoint,
        "assignedEmployeeId": worker.id,
        "objective": "child objective",
        "references": [],
        "requestedAuthority": "ask",
        "rosterRevision": store.roster_snapshot().unwrap().revision,
        "idempotencyKey": "task-child"
    });
    let proposed = app
        .handle(request(
            "task-delegate",
            "task.delegate",
            delegate_params.clone(),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(proposed["task"]["state"], "proposed");
    assert_eq!(proposed["delegationMessage"], json!(null));
    let child_id = proposed["task"]["id"].as_str().unwrap();
    let replay = app
        .handle(request(
            "task-delegate-replay",
            "task.delegate",
            delegate_params,
        ))
        .await
        .result
        .unwrap();
    assert_eq!(replay["task"]["id"], child_id);
    assert_eq!(replay["eventSequence"], json!(null));

    let starts_before_claim = backend.starts.load(Ordering::SeqCst);
    let missing_message = app
        .handle(request(
            "claim-proposed-task-message",
            "message.claim",
            json!({"id": child_id, "expectedRevision": 1, "idempotencyKey": "claim-proposed"}),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(missing_message.code, "LZ-MESSAGE-NOT-FOUND");
    assert_eq!(backend.starts.load(Ordering::SeqCst), starts_before_claim);
    assert_eq!(backend.turns.load(Ordering::SeqCst), 0);

    let accepted = app
        .handle(request(
            "task-accept",
            "task.accept",
            json!({"id": child_id, "expectedRevision": 1, "idempotencyKey": "accept-child"}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(accepted["task"]["state"], "accepted");
    assert_eq!(
        accepted["delegationMessage"]["envelope"]["taskId"],
        child_id
    );

    let child_uuid = uuid::Uuid::parse_str(child_id).unwrap();
    let child_turn = store
        .record_turn(&RecordTurnInput {
            request_id: "child-turn-request".to_owned(),
            idempotency_key: "child-turn".to_owned(),
            conversation_id: conversation.id,
            author: MessageSender::Employee(lead_endpoint),
            recipient: Some(worker_endpoint),
            task_id: Some(child_uuid),
            authority_mode: AuthorityMode::Ask,
            text: "perform child".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap();
    let child_submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "child-begin-request".to_owned(),
            idempotency_key: "child-begin".to_owned(),
            turn_id: child_turn.turn.id,
            expected_revision: child_turn.turn.revision,
            owner_boot_id: worker_session.owner_boot_id.unwrap(),
            recipient_session_id: worker_session.id,
        })
        .unwrap();
    store
        .observe_turn(&ObserveTurnInput {
            request_id: "child-started-request".to_owned(),
            idempotency_key: "child-started".to_owned(),
            turn_id: child_turn.turn.id,
            expected_revision: child_submitting.turn.revision,
            kind: "turn-started".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("native-2".to_owned()),
            native_turn_id: Some("native-child".to_owned()),
            payload: json!({"progress": true}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap();
    let started = app
        .handle(request(
            "task-start",
            "task.start",
            json!({
                "id": child_id,
                "expectedRevision": 2,
                "actor": worker_endpoint,
                "turnId": child_turn.turn.id,
                "evidence": {"nativeTurnId": "native-child"},
                "idempotencyKey": "start-child"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(started["task"]["state"], "running");
    let progress = app
        .handle(request(
            "task-report",
            "task.report",
            json!({
                "id": child_id,
                "expectedRevision": 3,
                "actor": worker_endpoint,
                "kind": "progress",
                "evidence": {"completed": 1, "total": 2},
                "failureCode": null,
                "failureMessage": null,
                "idempotencyKey": "report-child"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(progress["task"]["revision"], 4);
    let cancelled = app
        .handle(request(
            "task-cancel",
            "task.cancel",
            json!({
                "id": child_id,
                "expectedRevision": 4,
                "actor": {"kind": "user"},
                "evidence": {"reason": "stop"},
                "idempotencyKey": "cancel-child"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(cancelled["task"]["state"], "cancel-requested");

    let uncertain_turn = store
        .record_turn(&RecordTurnInput {
            request_id: "uncertain-root-request".to_owned(),
            idempotency_key: "uncertain-root".to_owned(),
            conversation_id: conversation.id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: Some(lead_endpoint),
            task_id: None,
            authority_mode: AuthorityMode::Ask,
            text: "uncertain objective".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap();
    store
        .execute_for_test(&format!(
            "UPDATE tasks SET state = 'uncertain', revision = 2 WHERE id = '{}'",
            uncertain_turn.turn.id
        ))
        .unwrap();
    let resolved = app
        .handle(request(
            "task-resolve",
            "task.resolve",
            json!({
                "id": uncertain_turn.turn.id,
                "expectedRevision": 2,
                "resolution": "completed",
                "evidence": {"verified": true},
                "failureCode": null,
                "failureMessage": null,
                "idempotencyKey": "resolve-uncertain"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(resolved["task"]["state"], "completed");

    let listed = app
        .handle(request(
            "task-list",
            "task.list",
            json!({"conversationId": conversation.id, "limit": 100}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 3);
    assert_eq!(listed[0]["id"], root_task_id.to_string());
    let shown = app
        .handle(request("task-show", "task.show", json!({"id": child_id})))
        .await
        .result
        .unwrap();
    assert_eq!(shown["task"]["id"], child_id);
    assert!(shown["evidence"].as_array().unwrap().len() >= 4);

    let missing = app
        .handle(request(
            "task-missing",
            "task.show",
            json!({"id": uuid::Uuid::new_v4()}),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(missing.code, "LZ-TASK-NOT-FOUND");
    assert!(missing.data.unwrap().get("taskId").is_some());
    assert!(
        store
            .list_events(0, 1000)
            .unwrap()
            .iter()
            .any(|event| event.event_type == "task.progressed")
    );

    let mut desktop_after = app
        .handle(request("task-desktop-after", "desktop.snapshot", json!({})))
        .await
        .result
        .unwrap();
    let mut desktop_before = desktop_before;
    desktop_before.as_object_mut().unwrap().remove("capturedAt");
    desktop_after.as_object_mut().unwrap().remove("capturedAt");
    desktop_before["status"]
        .as_object_mut()
        .unwrap()
        .remove("uptimeMillis");
    desktop_after["status"]
        .as_object_mut()
        .unwrap()
        .remove("uptimeMillis");
    assert!(desktop_before.get("tasks").is_none());
    assert!(desktop_after.get("tasks").is_none());
    assert_eq!(desktop_before["projects"], desktop_after["projects"]);
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
async fn turn_submit_follow_up_uses_the_existing_root_and_blocks_a_terminal_root_replay() {
    let (_temporary, _root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    let root_params = json!({
        "conversationId": conversation_id,
        "author": {"kind": "user"},
        "recipient": null,
        "text": "root objective",
        "authorityMode": "ask",
        "idempotencyKey": "root-before-warm"
    });
    let first = app
        .handle(request(
            "root-before-warm",
            "turn.submit",
            root_params.clone(),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(first.code, "LZ-TURN-SESSION-NOT-READY");
    let root_id = store.list_turns(conversation_id, 0, 100).unwrap()[0].id;
    assert_eq!(
        store.get_task(root_id).unwrap().task.active_turn_id,
        Some(root_id)
    );
    let follow_up_params = json!({
        "conversationId": conversation_id,
        "author": {"kind": "user"},
        "recipient": null,
        "taskId": root_id,
        "text": "follow-up",
        "authorityMode": "ask",
        "idempotencyKey": "root-follow-up"
    });
    let reserved = app
        .handle(request(
            "root-follow-up-reserved",
            "turn.submit",
            follow_up_params.clone(),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(reserved.code, "LZ-TURN-TASK-SCOPE");
    let turns = store.list_turns(conversation_id, 0, 100).unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].task_id, Some(root_id));
    app.handle(request(
        "warm-follow-up-session",
        "session.warm",
        json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-follow-up-session"
        }),
    ))
    .await
    .result
    .unwrap();
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("root-native-turn".to_owned()),
            raw: None,
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("root-native-turn".to_owned()),
            status: "completed".to_owned(),
            output: None,
            raw: None,
        },
    ]);

    let completed = app
        .handle(request("root-after-warm", "turn.submit", root_params))
        .await
        .result
        .unwrap();
    assert_eq!(completed["turn"]["id"], root_id.to_string());
    assert_eq!(completed["turn"]["state"], "completed");
    assert_eq!(completed["task"]["id"], root_id.to_string());
    assert_eq!(completed["task"]["state"], "completed");

    let replay = app
        .handle(request(
            "root-terminal-follow-up",
            "turn.submit",
            follow_up_params,
        ))
        .await
        .error
        .unwrap();
    assert_eq!(replay.code, "LZ-TURN-TASK-SCOPE");
    assert_eq!(backend.turns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn turn_submit_replay_blocks_an_uncertain_task_before_provider_io() {
    let (_temporary, _root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    let root_params = json!({
        "conversationId": conversation_id,
        "author": {"kind": "user"},
        "recipient": null,
        "text": "uncertain root",
        "authorityMode": "ask",
        "idempotencyKey": "uncertain-root-before-warm"
    });
    app.handle(request(
        "uncertain-root-before-warm",
        "turn.submit",
        root_params,
    ))
    .await;
    let root_id = store.list_turns(conversation_id, 0, 100).unwrap()[0].id;
    let follow_up_params = json!({
        "conversationId": conversation_id,
        "author": {"kind": "user"},
        "recipient": null,
        "taskId": root_id,
        "text": "follow-up after owner loss",
        "authorityMode": "ask",
        "idempotencyKey": "uncertain-root-follow-up"
    });
    let reserved = app
        .handle(request(
            "uncertain-root-follow-up-reserved",
            "turn.submit",
            follow_up_params.clone(),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(reserved.code, "LZ-TURN-TASK-SCOPE");
    store
        .execute_for_test(&format!(
            "UPDATE tasks SET state = 'uncertain', revision = revision + 1, \
             active_turn_id = NULL, active_session_id = NULL WHERE id = '{root_id}'"
        ))
        .unwrap();
    app.handle(request(
        "warm-uncertain-follow-up-session",
        "session.warm",
        json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-uncertain-follow-up-session"
        }),
    ))
    .await
    .result
    .unwrap();
    backend.turn_events.lock().unwrap().extend([
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("forbidden-uncertain-turn".to_owned()),
            raw: None,
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("forbidden-uncertain-turn".to_owned()),
            status: "completed".to_owned(),
            output: None,
            raw: None,
        },
    ]);

    let replay = app
        .handle(request(
            "uncertain-root-follow-up-replay",
            "turn.submit",
            follow_up_params,
        ))
        .await
        .error
        .unwrap();
    assert_eq!(replay.code, "LZ-TURN-TASK-SCOPE");
    assert_eq!(backend.turns.load(Ordering::SeqCst), 0);
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
    assert_eq!(result["schemaVersion"], 9);
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
        "schemaVersion 9",
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
        "schemaVersion 9",
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
        "schemaVersion 9",
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
        "schema8",
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

#[test]
fn automatic_dispatch_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script =
        std::fs::read_to_string(repository.join("scripts/acceptance/automatic-task-dispatch.ps1"))
            .unwrap();
    for required in [
        "accepted_projectless_child_dispatches_automatically_through_the_canonical_message",
        "project_task_dispatch_uses_an_isolated_worktree_and_preserves_dirty_user_files",
        "automatic_dispatch_configuration_drift_terminalizes_before_provider_io",
        "stale_pre_provider_dispatch_terminalizes_the_task_and_canonical_message_on_restart",
        "accepted_child_dispatch_claim_is_durable_exclusive_and_idempotent",
        "cancelling_a_claimed_dispatch_cancels_the_canonical_message_atomically",
        "deterministic-fake-transport",
        "RETAINED_FOR_AUDIT",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in [
        "provider', 'smoke",
        "apps\\lazoo-desktop\\src",
        "LivePrompt",
    ] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

#[test]
fn task_graph_acceptance_executes_backend_only_contract() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/task-delegation-graph.ps1");
    let temporary = tempdir().unwrap();
    let output_path = temporary.path().join("task-manifest.json");
    let status = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script_path)
        .arg("-OutputPath")
        .arg(&output_path)
        .status()
        .expect("task graph acceptance process");
    assert!(status.success(), "task graph acceptance must pass");

    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&output_path).expect("task graph acceptance manifest"),
    )
    .unwrap();
    assert_eq!(manifest["status"], "PASS");
    assert_eq!(manifest["schemaVersion"], 9);
    assert_eq!(manifest["mode"], "deterministic-fixture");
    for field in [
        "rootIdentity",
        "replayNoDuplicate",
        "followUpJoinedRoot",
        "askProposalNoMessage",
        "userAcceptanceCanonicalMessage",
        "authorityNarrowed",
        "guardFailures",
        "nativeEvidenceRequired",
        "evidenceOrdered",
        "activeDescendantBlocked",
        "childResultNoticeCompletesParent",
        "ownerLossUncertain",
        "uncertaintyResolutionIdempotent",
        "restartCorrelations",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "worktreeMutationAbsent",
        "conptyAbsent",
        "desktopCapabilityExpansionAbsent",
        "uiChangesAbsent",
    ] {
        assert_eq!(manifest["evidence"][field], true, "missing {field}");
    }
    assert!(manifest["commands"].as_array().unwrap().len() >= 20);
    assert!(
        manifest["commands"]
            .as_array()
            .unwrap()
            .iter()
            .all(|command| command["exitCode"].as_i64().is_some())
    );
}

fn permission_request(
    kind: PermissionKind,
    action: &str,
    scope_expansion: bool,
) -> PermissionRequest {
    PermissionRequest {
        provider: ProviderKind::Codex,
        session_id: "native-1".to_owned(),
        turn_id: Some("approval-turn".to_owned()),
        request_id: format!("request-{action}"),
        kind,
        action: action.to_owned(),
        scope_expansion,
        detail: json!({"command": action, "cwd": "C:\\repo"}),
    }
}

fn completed_turn_events(status: &str) -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::TurnStarted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("approval-turn".to_owned()),
            raw: None,
        },
        ProviderEvent::TurnCompleted {
            provider: ProviderKind::Codex,
            session_id: "native-1".to_owned(),
            turn_id: Some("approval-turn".to_owned()),
            status: status.to_owned(),
            output: Some("done".to_owned()),
            raw: None,
        },
    ]
}

struct ApprovalHarness {
    store: Store,
    app: App,
    backend: FakeSessionBackend,
    conversation_id: uuid::Uuid,
}

fn approval_harness(
    authority: AuthorityMode,
    timeout: Duration,
    permissions: Vec<PermissionRequest>,
    completion_status: &str,
) -> (tempfile::TempDir, ApprovalHarness) {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = store
        .create_employee(&CreateEmployeeInput {
            request_id: "create-approval-lead-request".to_owned(),
            idempotency_key: "create-approval-lead".to_owned(),
            name: "Rhea".to_owned(),
            provider: "codex".to_owned(),
            role: "Lead".to_owned(),
            instructions: String::new(),
            model: Some("test-model".to_owned()),
            reasoning_effort: Some("low".to_owned()),
            native_settings: json!({}),
            authority_mode: authority,
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
                checked_at: "2026-09-22T00:00:00Z".to_owned(),
            }),
        })
        .unwrap()
        .employee;
    let conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "create-approval-conversation-request".to_owned(),
            idempotency_key: "create-approval-conversation".to_owned(),
            project_id: None,
            lead_employee_id: employee.id,
            title: None,
        })
        .unwrap()
        .conversation;
    let backend = FakeSessionBackend::default();
    backend
        .turn_events
        .lock()
        .unwrap()
        .extend(completed_turn_events(completion_status));
    backend
        .permission_requests
        .lock()
        .unwrap()
        .extend(permissions);
    let app = App::new_with_session_backend(
        store.clone(),
        root,
        DiscoveryEnvironment::empty(),
        Arc::new(backend.clone()),
    )
    .unwrap()
    .with_approval_timeout(timeout);
    (
        temporary,
        ApprovalHarness {
            store,
            app,
            backend,
            conversation_id: conversation.id,
        },
    )
}

async fn warm_lead(harness: &ApprovalHarness) {
    let employee = harness.store.list_employees().unwrap().remove(0);
    harness
        .app
        .handle(request(
            "warm-approval-lead",
            "session.warm",
            json!({
                "employeeId": employee.id,
                "expectedEmployeeRevision": employee.configuration_revision,
                "idempotencyKey": "warm-approval-lead"
            }),
        ))
        .await
        .result
        .unwrap();
}

fn submit_governed_turn(
    harness: &ApprovalHarness,
    authority: &str,
) -> tokio::task::JoinHandle<lazoo_protocol::JsonRpcResponse> {
    let app = harness.app.clone();
    let conversation_id = harness.conversation_id;
    let authority = authority.to_owned();
    tokio::spawn(async move {
        app.handle(request(
            "submit-governed-turn",
            "turn.submit",
            json!({
                "conversationId": conversation_id,
                "author": {"kind": "user"},
                "recipient": null,
                "authorityMode": authority,
                "text": "Run the governed command",
                "activateIfNeeded": false,
                "maxAttempts": 3,
                "idempotencyKey": "submit-governed-turn"
            }),
        ))
        .await
    })
}

async fn wait_for_pending_approval(harness: &ApprovalHarness) -> serde_json::Value {
    timeout(Duration::from_secs(5), async {
        loop {
            let listed = harness
                .app
                .handle(request(
                    "list-pending-approvals",
                    "approval.list",
                    json!({"state": "pending"}),
                ))
                .await
                .result
                .unwrap();
            if let Some(first) = listed.as_array().and_then(|items| items.first()) {
                return first.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a pending approval should be listed while the provider waits")
}

fn approval_event_types(store: &Store) -> Vec<String> {
    store
        .list_events(0, 1000)
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .filter(|kind| kind.starts_with("approval"))
        .collect()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One governed turn proves wait, decision, replay, and snapshot boundaries together.
async fn ask_turn_waits_for_the_user_decision_and_forwards_approval_to_the_provider() {
    let (_temporary, harness) = approval_harness(
        AuthorityMode::Ask,
        Duration::from_secs(60),
        vec![permission_request(
            PermissionKind::CommandExecution,
            "cargo test",
            false,
        )],
        "completed",
    );
    let snapshot_before = harness
        .app
        .handle(request("desktop-before", "desktop.snapshot", json!({})))
        .await
        .result
        .unwrap();
    warm_lead(&harness).await;
    let submission = submit_governed_turn(&harness, "ask");

    let pending = wait_for_pending_approval(&harness).await;
    assert_eq!(pending["state"], "pending");
    assert_eq!(pending["kind"], "command-execution");
    assert_eq!(pending["action"], "cargo test");
    assert_eq!(pending["authority"], "ask");
    assert_eq!(pending["scopeExpansion"], false);
    assert!(
        harness
            .backend
            .permission_decisions
            .lock()
            .unwrap()
            .is_empty()
    );
    assert!(!submission.is_finished());

    let decided = harness
        .app
        .handle(request(
            "approve-command",
            "approval.decide",
            json!({
                "id": pending["id"],
                "expectedRevision": pending["revision"],
                "decision": "approve",
                "idempotencyKey": "approve-command"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(decided["approval"]["state"], "approved");
    assert_eq!(decided["approval"]["decider"], "user");
    assert_eq!(decided["approval"]["revision"], 2);

    let submitted = timeout(Duration::from_secs(5), submission)
        .await
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    assert_eq!(submitted["turn"]["state"], "completed");
    assert_eq!(
        *harness.backend.permission_decisions.lock().unwrap(),
        vec![PermissionDecision::Approve]
    );
    let shown = harness
        .app
        .handle(request(
            "show-approval",
            "approval.show",
            json!({"id": pending["id"]}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(shown["state"], "approved");
    assert_eq!(shown["turnId"], submitted["turn"]["id"]);
    assert_eq!(shown["taskId"], submitted["turn"]["id"]);
    assert_eq!(
        approval_event_types(&harness.store),
        vec!["approval.requested", "approval.decided"]
    );

    let replay = harness
        .app
        .handle(request(
            "approve-command-again",
            "approval.decide",
            json!({
                "id": pending["id"],
                "expectedRevision": pending["revision"],
                "decision": "decline",
                "idempotencyKey": "approve-command"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(replay["approval"]["state"], "approved");
    assert_eq!(replay["eventSequence"], serde_json::Value::Null);
    let stale = harness
        .app
        .handle(request(
            "approve-command-stale",
            "approval.decide",
            json!({
                "id": pending["id"],
                "expectedRevision": 1,
                "decision": "approve",
                "idempotencyKey": "approve-command-stale"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(stale.code, "LZ-APPROVAL-STALE");

    let snapshot_after = harness
        .app
        .handle(request("desktop-after", "desktop.snapshot", json!({})))
        .await
        .result
        .unwrap();
    let snapshot_keys = |value: &serde_json::Value| {
        value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        snapshot_keys(&snapshot_before),
        snapshot_keys(&snapshot_after)
    );
    assert!(snapshot_after.get("approvals").is_none());
    assert!(snapshot_after.get("pendingApprovals").is_none());
}

#[tokio::test]
async fn declined_and_expired_approvals_fail_closed_at_the_provider() {
    let (_temporary, declined) = approval_harness(
        AuthorityMode::Ask,
        Duration::from_secs(60),
        vec![permission_request(
            PermissionKind::FileChange,
            "apply file changes in the workspace",
            false,
        )],
        "completed",
    );
    warm_lead(&declined).await;
    let submission = submit_governed_turn(&declined, "ask");
    let pending = wait_for_pending_approval(&declined).await;
    declined
        .app
        .handle(request(
            "decline-change",
            "approval.decide",
            json!({
                "id": pending["id"],
                "expectedRevision": pending["revision"],
                "decision": "decline",
                "idempotencyKey": "decline-change"
            }),
        ))
        .await
        .result
        .unwrap();
    let submitted = timeout(Duration::from_secs(5), submission)
        .await
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    assert_eq!(submitted["turn"]["state"], "completed");
    assert_eq!(
        *declined.backend.permission_decisions.lock().unwrap(),
        vec![PermissionDecision::Decline]
    );

    let (_expired_temporary, expired) = approval_harness(
        AuthorityMode::Ask,
        Duration::from_secs(1),
        vec![permission_request(
            PermissionKind::CommandExecution,
            "cargo test",
            false,
        )],
        "completed",
    );
    warm_lead(&expired).await;
    let submission = submit_governed_turn(&expired, "ask");
    let pending = wait_for_pending_approval(&expired).await;
    let submitted = timeout(Duration::from_secs(10), submission)
        .await
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    assert_eq!(submitted["turn"]["state"], "completed");
    assert_eq!(
        *expired.backend.permission_decisions.lock().unwrap(),
        vec![PermissionDecision::Decline]
    );
    let approval_id: uuid::Uuid = pending["id"].as_str().unwrap().parse().unwrap();
    let closed = expired.store.get_approval(approval_id).unwrap();
    assert_eq!(closed.state, lazoo_protocol::ApprovalState::Expired);
    assert_eq!(
        closed.decider,
        Some(lazoo_protocol::ApprovalDecider::System)
    );
    assert_eq!(closed.failure_code.as_deref(), Some("LZ-APPROVAL-TIMEOUT"));
    assert_eq!(
        approval_event_types(&expired.store),
        vec!["approval.requested", "approval.expired"]
    );
    let late = expired
        .app
        .handle(request(
            "approve-late",
            "approval.decide",
            json!({
                "id": pending["id"],
                "expectedRevision": closed.revision,
                "decision": "approve",
                "idempotencyKey": "approve-late"
            }),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(late.code, "LZ-APPROVAL-STATE");
}

#[tokio::test]
async fn self_decision_policy_auto_approves_in_scope_and_waits_on_scope_expansion() {
    let (_temporary, harness) = approval_harness(
        AuthorityMode::SelfDecision,
        Duration::from_secs(60),
        vec![
            permission_request(
                PermissionKind::FileChange,
                "apply file changes in the workspace",
                false,
            ),
            permission_request(
                PermissionKind::CommandExecution,
                "apply file changes under C:\\",
                true,
            ),
        ],
        "completed",
    );
    warm_lead(&harness).await;
    let submission = submit_governed_turn(&harness, "self-decision");
    let pending = wait_for_pending_approval(&harness).await;
    assert_eq!(pending["scopeExpansion"], true);
    assert_eq!(pending["authority"], "self-decision");
    assert_eq!(
        *harness.backend.permission_decisions.lock().unwrap(),
        vec![PermissionDecision::Approve]
    );
    harness
        .app
        .handle(request(
            "decline-widening",
            "approval.decide",
            json!({
                "id": pending["id"],
                "expectedRevision": pending["revision"],
                "decision": "decline",
                "idempotencyKey": "decline-widening"
            }),
        ))
        .await
        .result
        .unwrap();
    timeout(Duration::from_secs(5), submission)
        .await
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        *harness.backend.permission_decisions.lock().unwrap(),
        vec![PermissionDecision::Approve, PermissionDecision::Decline]
    );
    let approvals = harness
        .app
        .handle(request("list-all-approvals", "approval.list", json!({})))
        .await
        .result
        .unwrap();
    let approvals = approvals.as_array().unwrap();
    assert_eq!(approvals.len(), 2);
    assert_eq!(approvals[0]["state"], "approved");
    assert_eq!(approvals[0]["decider"], "policy");
    assert_eq!(approvals[0]["kind"], "file-change");
    assert_eq!(approvals[1]["state"], "declined");
    assert_eq!(approvals[1]["decider"], "user");
}

#[tokio::test]
async fn cancelling_a_turn_with_a_pending_approval_declines_the_provider_and_records_cancellation()
{
    let (_temporary, harness) = approval_harness(
        AuthorityMode::Ask,
        Duration::from_secs(60),
        vec![permission_request(
            PermissionKind::CommandExecution,
            "cargo test",
            false,
        )],
        "interrupted",
    );
    warm_lead(&harness).await;
    let submission = submit_governed_turn(&harness, "ask");
    let pending = wait_for_pending_approval(&harness).await;
    let turn_id: uuid::Uuid = pending["turnId"].as_str().unwrap().parse().unwrap();
    let turn = harness.store.get_turn(turn_id).unwrap();
    let cancelled = harness
        .app
        .handle(request(
            "cancel-governed-turn",
            "turn.cancel",
            json!({
                "id": turn_id,
                "expectedRevision": turn.revision,
                "idempotencyKey": "cancel-governed-turn"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(cancelled["turn"]["state"], "cancelled");
    let submitted = timeout(Duration::from_secs(5), submission)
        .await
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    assert_eq!(submitted["turn"]["state"], "cancelled");
    assert_eq!(
        *harness.backend.permission_decisions.lock().unwrap(),
        vec![PermissionDecision::Decline]
    );
    let approval_id: uuid::Uuid = pending["id"].as_str().unwrap().parse().unwrap();
    let closed = harness.store.get_approval(approval_id).unwrap();
    assert_eq!(closed.state, lazoo_protocol::ApprovalState::Cancelled);
    assert_eq!(
        closed.failure_code.as_deref(),
        Some("LZ-APPROVAL-TURN-CANCELLED")
    );
    assert_eq!(
        approval_event_types(&harness.store),
        vec!["approval.requested", "approval.cancelled"]
    );
}

#[tokio::test]
async fn pending_approvals_from_a_previous_boot_expire_with_owner_loss_on_restart() {
    let (_temporary, harness) = approval_harness(
        AuthorityMode::Ask,
        Duration::from_secs(60),
        vec![permission_request(
            PermissionKind::CommandExecution,
            "cargo test",
            false,
        )],
        "completed",
    );
    warm_lead(&harness).await;
    let submission = submit_governed_turn(&harness, "ask");
    let pending = wait_for_pending_approval(&harness).await;
    submission.abort();
    let _ = submission.await;

    let restarted = App::new_with_provider_environment(
        harness.store.clone(),
        harness.app.data_root().clone(),
        DiscoveryEnvironment::empty(),
    )
    .unwrap();
    let shown = restarted
        .handle(request(
            "show-restarted-approval",
            "approval.show",
            json!({"id": pending["id"]}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(shown["state"], "expired");
    assert_eq!(shown["decider"], "system");
    assert_eq!(shown["failureCode"], "LZ-APPROVAL-OWNER-LOST");
    assert!(
        approval_event_types(&harness.store)
            .iter()
            .any(|kind| kind == "approvals.reconciled")
    );
}

struct ToolHarness {
    store: Store,
    app: App,
    backend: FakeSessionBackend,
    conversation_id: uuid::Uuid,
    lead: lazoo_protocol::EmployeeRecord,
    worker: lazoo_protocol::EmployeeRecord,
}

fn tool_harness(authority: AuthorityMode) -> (tempfile::TempDir, ToolHarness) {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee_input = |name: &str, role: &str, key: &str| CreateEmployeeInput {
        request_id: format!("{key}-request"),
        idempotency_key: key.to_owned(),
        name: name.to_owned(),
        provider: "codex".to_owned(),
        role: role.to_owned(),
        instructions: String::new(),
        model: Some("test-model".to_owned()),
        reasoning_effort: Some("low".to_owned()),
        native_settings: json!({}),
        authority_mode: authority,
        checkout_policy: EmployeeCheckoutPolicy::Isolated,
        enabled: true,
        provider_observation: Some(ProviderObservation {
            provider: "codex".to_owned(),
            verification: "configured".to_owned(),
            version: Some("test".to_owned()),
            protocol: Some("app-server/v2".to_owned()),
            capabilities: json!({"session": true, "permission": true, "mcp": true}),
            authentication: json!({"native": "ready", "external": "unknown"}),
            degradation_reason: None,
            checked_at: "2026-09-23T00:00:00Z".to_owned(),
        }),
    };
    let lead = store
        .create_employee(&employee_input("Rhea", "Lead", "tools-rhea"))
        .unwrap()
        .employee;
    let worker = store
        .create_employee(&employee_input("Mira", "Worker", "tools-mira"))
        .unwrap()
        .employee;
    let conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "tools-conversation-request".to_owned(),
            idempotency_key: "tools-conversation".to_owned(),
            project_id: None,
            lead_employee_id: lead.id,
            title: None,
        })
        .unwrap()
        .conversation;
    let backend = FakeSessionBackend::default();
    backend
        .turn_events
        .lock()
        .unwrap()
        .extend(completed_turn_events("completed"));
    let app = App::new_with_session_backend(
        store.clone(),
        root,
        DiscoveryEnvironment::empty(),
        Arc::new(backend.clone()),
    )
    .unwrap();
    (
        temporary,
        ToolHarness {
            store,
            app,
            backend,
            conversation_id: conversation.id,
            lead,
            worker,
        },
    )
}

fn tool_call(call_id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    session_tool_call("native-1", call_id, name, arguments)
}

/// A scripted call issued by one fake session; sessions are numbered in start order.
fn session_tool_call(
    session_id: &str,
    call_id: &str,
    name: &str,
    arguments: serde_json::Value,
) -> ToolCall {
    ToolCall {
        provider: ProviderKind::Codex,
        session_id: session_id.to_owned(),
        turn_id: Some("approval-turn".to_owned()),
        call_id: call_id.to_owned(),
        name: name.to_owned(),
        arguments,
    }
}

async fn warm_tool_lead(harness: &ToolHarness) {
    harness
        .app
        .handle(request(
            "warm-tool-lead",
            "session.warm",
            json!({
                "employeeId": harness.lead.id,
                "expectedEmployeeRevision": harness.lead.configuration_revision,
                "idempotencyKey": "warm-tool-lead"
            }),
        ))
        .await
        .result
        .unwrap();
}

async fn submit_lead_turn(harness: &ToolHarness, authority: &str) -> serde_json::Value {
    let response = timeout(
        Duration::from_secs(20),
        harness.app.handle(request(
            "submit-tool-turn",
            "turn.submit",
            json!({
                "conversationId": harness.conversation_id,
                "author": {"kind": "user"},
                "recipient": null,
                "authorityMode": authority,
                "text": "Coordinate the parser fix",
                "activateIfNeeded": false,
                "maxAttempts": 3,
                "idempotencyKey": "submit-tool-turn"
            }),
        )),
    )
    .await
    .expect("the lead turn completes once tool calls are answered");
    response.result.unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // The full catalog declaration plus one delegate-and-wait story.
async fn lead_turn_lists_employees_delegates_and_waits_through_collaboration_tools() {
    use lazoo_protocol::{MessageSender, TaskState};

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    warm_tool_lead(&harness).await;
    // A live lead cites the rosterRevision returned by employees_list; the scripted call cites
    // the revision after its own session warmed, exactly as that answer would.
    let roster_revision = harness.store.roster_snapshot().unwrap().revision;
    harness.backend.tool_calls.lock().unwrap().extend([
        tool_call("call-1", "employees_list", json!({})),
        tool_call(
            "call-2",
            "task_delegate",
            json!({
                "employeeId": harness.worker.id,
                "objective": "Implement the parser fix",
                "rosterRevision": roster_revision,
                "requestedAuthority": "self-decision"
            }),
        ),
        tool_call("call-3", "task_wait", json!({"timeoutSeconds": 10})),
    ]);
    let dispatcher_app = harness.app.clone();
    let dispatcher = tokio::spawn(async move { dispatcher_app.run_task_dispatcher().await });

    let submitted = submit_lead_turn(&harness, "self-decision").await;
    assert_eq!(submitted["turn"]["state"], "completed");
    dispatcher.abort();
    let _ = dispatcher.await;

    let declared = harness.backend.session_tools.lock().unwrap().clone();
    assert_eq!(
        declared[0],
        vec![
            "employees_list",
            "task_delegate",
            "task_wait",
            "task_report",
            "incident_report",
            "incident_list",
            "message_send",
            "message_reply"
        ],
        "every daemon-managed Codex session declares the Lazoo tool catalog"
    );
    assert_eq!(
        declared.len(),
        2,
        "the worker session declares the same catalog"
    );

    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 3);
    assert!(results[0].success, "{:?}", results[0]);
    assert_eq!(results[0].content["rosterRevision"], roster_revision);
    let employees = results[0].content["employees"].as_array().unwrap();
    assert_eq!(employees.len(), 2);
    let me = employees
        .iter()
        .find(|entry| entry["self"] == true)
        .expect("the caller is marked in the roster");
    assert_eq!(me["name"], "Rhea");
    assert_eq!(me["authorityMode"], "self-decision");

    assert!(results[1].success, "{:?}", results[1]);
    assert_eq!(results[1].content["state"], "accepted");
    assert_eq!(results[1].content["effectiveAuthority"], "self-decision");
    let child_id: uuid::Uuid = results[1].content["taskId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    assert!(results[2].success, "{:?}", results[2]);
    assert_eq!(results[2].content["timedOut"], false);
    let waited = results[2].content["tasks"].as_array().unwrap();
    assert_eq!(waited.len(), 1);
    assert_eq!(waited[0]["taskId"], child_id.to_string());
    assert_eq!(waited[0]["state"], "completed");
    assert_eq!(
        waited[0]["assignedEmployeeId"],
        harness.worker.id.to_string()
    );
    assert_eq!(waited[0]["output"], "done");

    let detail = harness.store.get_task(child_id).unwrap();
    assert_eq!(detail.task.state, TaskState::Completed);
    let root_task_id: uuid::Uuid = submitted["turn"]["taskId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(detail.task.parent_task_id, Some(root_task_id));
    let MessageSender::Employee(creator) = detail.task.creator else {
        panic!("tool delegation is authored by the lead employee endpoint");
    };
    assert_eq!(creator.employee_id, harness.lead.id);
    assert!(creator.session_id.is_some());
    assert!(detail.dispatch.is_some());
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 2);

    let root_task = harness.store.get_task(root_task_id).unwrap();
    assert_eq!(root_task.task.state, TaskState::Completed);
    assert_eq!(root_task.children.len(), 1);
    let event_types: Vec<String> = harness
        .store
        .list_events(0, 1000)
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .collect();
    assert!(event_types.iter().any(|kind| kind == "task.delegated"));
    assert!(event_types.iter().any(|kind| kind == "task.completed"));
}

#[tokio::test]
async fn ask_lead_delegation_through_tools_is_proposed_and_refusals_never_end_the_turn() {
    use lazoo_protocol::TaskState;

    let (_temporary, harness) = tool_harness(AuthorityMode::Ask);
    warm_tool_lead(&harness).await;
    let roster_revision = harness.store.roster_snapshot().unwrap().revision;
    harness.backend.tool_calls.lock().unwrap().extend([
        tool_call(
            "call-1",
            "task_delegate",
            json!({
                "employeeId": harness.worker.id,
                "objective": "Implement the parser fix",
                "rosterRevision": roster_revision
            }),
        ),
        tool_call("call-2", "task_wait", json!({"timeoutSeconds": 0})),
        tool_call("call-3", "memory.search", json!({"query": "parser"})),
        tool_call(
            "call-4",
            "task_delegate",
            json!({"employeeId": harness.worker.id, "rosterRevision": roster_revision}),
        ),
        tool_call(
            "call-5",
            "task_delegate",
            json!({
                "employeeId": harness.worker.id,
                "objective": "Widen authority",
                "rosterRevision": roster_revision,
                "requestedAuthority": "full-autonomy"
            }),
        ),
        tool_call("", "employees_list", json!({})),
    ]);

    let submitted = submit_lead_turn(&harness, "ask").await;
    assert_eq!(
        submitted["turn"]["state"], "completed",
        "refused tool calls never end the provider turn"
    );

    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 6);
    assert!(results[0].success, "{:?}", results[0]);
    assert_eq!(results[0].content["state"], "proposed");
    assert!(
        results[0].content["note"]
            .as_str()
            .unwrap()
            .contains("local user")
    );
    let child_id: uuid::Uuid = results[0].content["taskId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(results[1].success, "{:?}", results[1]);
    assert_eq!(results[1].content["timedOut"], true);
    assert_eq!(results[1].content["tasks"][0]["state"], "proposed");
    assert!(!results[2].success);
    assert_eq!(results[2].content["error"]["code"], "LZ-TOOL-UNKNOWN");
    assert!(!results[3].success);
    assert_eq!(results[3].content["error"]["code"], "LZ-TOOL-INVALID");
    assert!(!results[4].success);
    assert_eq!(
        results[4].content["error"]["code"],
        "LZ-TASK-AUTHORITY-EXPANSION"
    );
    assert!(!results[5].success);
    assert_eq!(results[5].content["error"]["code"], "LZ-TOOL-INVALID");

    harness.app.dispatch_pending_tasks_once().await.unwrap();
    let detail = harness.store.get_task(child_id).unwrap();
    assert_eq!(detail.task.state, TaskState::Proposed);
    assert!(
        detail.dispatch.is_none(),
        "a proposed task never dispatches without user acceptance"
    );
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 1);
    let root_children = harness
        .store
        .get_task(detail.task.parent_task_id.unwrap())
        .unwrap()
        .children;
    assert_eq!(root_children.len(), 1, "invalid delegations write nothing");
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One dispatch fixture proves the tool report outranks native completion.
async fn worker_turn_reports_failure_through_task_report_before_native_completion() {
    use lazoo_protocol::{
        MessageSender, TaskEvidenceKind, TaskState, UserMessageEndpoint, UserMessageKind,
    };
    use lazoo_store::{BeginSessionInput, MarkSessionReadyInput, RecordTurnInput};

    let (_temporary, harness) = tool_harness(AuthorityMode::FullAutonomy);
    let root_task = harness
        .store
        .record_turn(&RecordTurnInput {
            request_id: "tools-root-request".to_owned(),
            idempotency_key: "tools-root".to_owned(),
            conversation_id: harness.conversation_id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::FullAutonomy,
            text: "Lead objective".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap()
        .task
        .unwrap();
    let boot_id = uuid::Uuid::new_v4();
    let lead_starting = harness
        .store
        .begin_session(&BeginSessionInput {
            request_id: "tools-lead-session-request".to_owned(),
            idempotency_key: "tools-lead-session".to_owned(),
            employee_id: harness.lead.id,
            expected_employee_revision: harness.lead.configuration_revision,
            owner_boot_id: boot_id,
        })
        .unwrap();
    let lead_session = harness
        .store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: lead_starting.session.id,
            expected_revision: lead_starting.session.revision,
            owner_boot_id: boot_id,
            native_session_id: "native-lead".to_owned(),
            native_host_pid: 4241,
        })
        .unwrap()
        .session;
    harness.backend.tool_calls.lock().unwrap().extend([
        tool_call(
            "call-1",
            "task_report",
            json!({"kind": "progress", "evidence": {"step": "parsing"}}),
        ),
        tool_call(
            "call-2",
            "task_report",
            json!({
                "kind": "failed",
                "failureCode": "LZ-FIXTURE-BLOCKED",
                "failureMessage": "the dependency is missing",
                "evidence": {"missing": "dependency"}
            }),
        ),
    ]);
    let delegated = harness
        .app
        .handle(request(
            "tools-delegate",
            "task.delegate",
            json!({
                "parentTaskId": root_task.id,
                "actor": {"employeeId": harness.lead.id, "sessionId": lead_session.id},
                "assignedEmployeeId": harness.worker.id,
                "objective": "Implement the parser fix",
                "references": [],
                "requestedAuthority": "full-autonomy",
                "rosterRevision": harness.store.roster_snapshot().unwrap().revision,
                "idempotencyKey": "tools-delegate"
            }),
        ))
        .await
        .result
        .unwrap();
    let task_id: uuid::Uuid = delegated["task"]["id"].as_str().unwrap().parse().unwrap();
    let dispatcher_app = harness.app.clone();
    let dispatcher = tokio::spawn(async move { dispatcher_app.run_task_dispatcher().await });
    harness.app.dispatch_pending_tasks_once().await.unwrap();
    let terminal = timeout(Duration::from_secs(10), async {
        loop {
            let state = harness.store.get_task(task_id).unwrap().task.state;
            if matches!(state, TaskState::Completed | TaskState::Failed) {
                break state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the dispatched worker turn reaches a terminal task state");
    dispatcher.abort();
    let _ = dispatcher.await;

    assert_eq!(terminal, TaskState::Failed);
    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 2);
    assert!(results[0].success, "{:?}", results[0]);
    assert_eq!(results[0].content["state"], "running");
    assert!(results[1].success, "{:?}", results[1]);
    assert_eq!(results[1].content["state"], "failed");
    let detail = harness.store.get_task(task_id).unwrap();
    assert_eq!(
        detail.task.failure_code.as_deref(),
        Some("LZ-FIXTURE-BLOCKED"),
        "the explicit report wins over the later native completion"
    );
    assert_eq!(
        detail.task.failure_message.as_deref(),
        Some("the dependency is missing")
    );
    let reported = detail
        .evidence
        .iter()
        .filter(|evidence| {
            matches!(
                evidence.kind,
                TaskEvidenceKind::Progress | TaskEvidenceKind::Failed
            ) && evidence.session_id == detail.dispatch.as_ref().unwrap().session_id
        })
        .count();
    assert!(
        reported >= 2,
        "tool reports carry the worker's dispatch session"
    );
    assert_eq!(
        detail
            .evidence
            .iter()
            .find(|evidence| evidence.kind == TaskEvidenceKind::Failed)
            .unwrap()
            .detail["missing"],
        "dependency"
    );
    let dispatch_turn = harness
        .store
        .get_turn(detail.dispatch.as_ref().unwrap().turn_id.unwrap())
        .unwrap();
    assert_eq!(dispatch_turn.state, TurnState::Completed);
}

#[test]
fn collaboration_tools_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script =
        std::fs::read_to_string(repository.join("scripts/acceptance/collaboration-tools.ps1"))
            .unwrap();
    for required in [
        "observer_tool_calls_fail_closed_without_an_attached_control_plane",
        "codex_tool_calls_normalize_with_the_active_turn_fallback",
        "observed_codex_tool_calls_are_answered_with_the_lazoo_result",
        "codex_sessions_without_tools_stay_stable_and_resume_with_tools_fails_closed",
        "lead_turn_lists_employees_delegates_and_waits_through_collaboration_tools",
        "ask_lead_delegation_through_tools_is_proposed_and_refusals_never_end_the_turn",
        "worker_turn_reports_failure_through_task_report_before_native_completion",
        "collaboration-tools-manifest.json",
        "deterministic-fake-transport",
        "schemaVersion = 9",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in [
        "provider', 'smoke",
        "apps\\lazoo-desktop\\src",
        "LivePrompt",
    ] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

async fn list_incidents(app: &App, params: serde_json::Value) -> Vec<serde_json::Value> {
    app.handle(request("incident-list", "incident.list", params))
        .await
        .result
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn incident_event_types(store: &Store) -> Vec<String> {
    store
        .list_events(0, 1000)
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .filter(|kind| kind.starts_with("incident"))
        .collect()
}

#[tokio::test]
async fn session_start_failure_raises_a_human_readable_incident() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = configured_employee(&store);
    let app = App::new_with_session_backend(
        store.clone(),
        root,
        DiscoveryEnvironment::empty(),
        Arc::new(FailingSessionBackend),
    )
    .unwrap();
    let snapshot_before = app
        .handle(request("desktop-before", "desktop.snapshot", json!({})))
        .await
        .result
        .unwrap();

    for key in ["warm-fail-1", "warm-fail-2"] {
        let error = app
            .handle(request(
                key,
                "session.warm",
                json!({
                    "employeeId": employee.id,
                    "expectedEmployeeRevision": employee.configuration_revision,
                    "idempotencyKey": key
                }),
            ))
            .await
            .error
            .unwrap();
        assert_eq!(error.code, "LZ-SESSION-START-FAILED");
    }

    let incidents = list_incidents(&app, json!({"state": "open"})).await;
    assert_eq!(
        incidents.len(),
        1,
        "equivalent failures fold into one incident"
    );
    let incident = &incidents[0];
    assert_eq!(incident["code"], "LZ-SESSION-START-FAILED");
    assert_eq!(incident["source"], "daemon");
    assert_eq!(incident["title"], "Employee session failed to start");
    assert!(
        incident["nextAction"]
            .as_str()
            .unwrap()
            .contains("Probe the provider")
    );
    assert_eq!(incident["employeeId"], employee.id.to_string());
    assert_eq!(incident["provider"], "codex");
    assert!(incident["sessionId"].as_str().is_some());
    assert_eq!(incident["occurrences"], 2);
    assert_eq!(incident["priority"], "normal");
    assert!(
        incident["detail"]["failureMessage"]
            .as_str()
            .unwrap()
            .contains("did not become ready")
    );
    assert_eq!(
        incident_event_types(&store),
        vec!["incident.raised", "incident.recurred"]
    );

    let snapshot_after = app
        .handle(request("desktop-after", "desktop.snapshot", json!({})))
        .await
        .result
        .unwrap();
    let keys = |value: &serde_json::Value| {
        value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&snapshot_before), keys(&snapshot_after));
    assert!(
        !snapshot_after.to_string().contains("\"incidents\""),
        "the desktop snapshot gains no incident field in this slice"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One fixture proves escalation and every RPC settlement boundary together.
async fn provider_turn_failures_fold_into_one_incident_escalate_and_settle_through_rpc() {
    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    harness
        .backend
        .turn_retryable_failure
        .store(true, Ordering::SeqCst);
    warm_tool_lead(&harness).await;
    let mut last_turn_id = String::new();
    for index in 1..=3 {
        let key = format!("submit-fail-{index}");
        let error = harness
            .app
            .handle(request(
                &key,
                "turn.submit",
                json!({
                    "conversationId": harness.conversation_id,
                    "author": {"kind": "user"},
                    "recipient": null,
                    "authorityMode": "self-decision",
                    "text": format!("Attempt {index}"),
                    "activateIfNeeded": false,
                    "maxAttempts": 3,
                    "idempotencyKey": key
                }),
            ))
            .await
            .error
            .unwrap();
        assert_eq!(error.code, "LZ-TURN-PROVIDER");
        last_turn_id = error.data.unwrap()["turnId"].as_str().unwrap().to_owned();
    }

    let incidents = list_incidents(&harness.app, json!({})).await;
    assert_eq!(incidents.len(), 1);
    let incident = &incidents[0];
    assert_eq!(incident["code"], "LZ-MESSAGE-TIMEOUT");
    assert_eq!(incident["title"], "Delivery deadline expired");
    assert_eq!(incident["occurrences"], 3);
    assert_eq!(incident["windowOccurrences"], 3);
    assert_eq!(
        incident["priority"], "priority",
        "three equivalent failures inside five minutes cross the threshold"
    );
    assert_eq!(incident["state"], "open");
    assert_eq!(incident["employeeId"], harness.lead.id.to_string());
    assert_eq!(incident["turnId"], last_turn_id);
    assert_eq!(incident["detail"]["failureCode"], "LZ-TURN-PROVIDER");
    assert!(
        incident["detail"]["providerError"]
            .as_str()
            .unwrap()
            .contains("timed out")
    );
    assert_eq!(
        incident_event_types(&harness.store),
        vec!["incident.raised", "incident.recurred", "incident.escalated"]
    );

    let id = incident["id"].clone();
    let acknowledged = harness
        .app
        .handle(request(
            "ack",
            "incident.acknowledge",
            json!({"id": id, "expectedRevision": 3, "idempotencyKey": "ack-incident"}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(acknowledged["incident"]["state"], "acknowledged");
    assert_eq!(acknowledged["incident"]["revision"], 4);
    assert!(acknowledged["eventSequence"].is_number());
    let replay = harness
        .app
        .handle(request(
            "ack-again",
            "incident.acknowledge",
            json!({"id": id, "expectedRevision": 99, "idempotencyKey": "ack-incident"}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(replay["incident"]["revision"], 4);
    assert!(replay["eventSequence"].is_null(), "replays are write-free");
    let stale = harness
        .app
        .handle(request(
            "ack-stale",
            "incident.acknowledge",
            json!({"id": id, "expectedRevision": 3, "idempotencyKey": "ack-stale"}),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(stale.code, "LZ-INCIDENT-STALE");
    let resolved = harness
        .app
        .handle(request(
            "resolve",
            "incident.resolve",
            json!({"id": id, "expectedRevision": 4, "idempotencyKey": "resolve-incident"}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(resolved["incident"]["state"], "resolved");
    assert!(resolved["incident"]["resolvedAt"].is_string());
    let settled = harness
        .app
        .handle(request(
            "resolve-again",
            "incident.resolve",
            json!({"id": id, "expectedRevision": 5, "idempotencyKey": "resolve-twice"}),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(settled.code, "LZ-INCIDENT-STATE");
    let missing = harness
        .app
        .handle(request(
            "show-missing",
            "incident.show",
            json!({"id": uuid::Uuid::new_v4()}),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(missing.code, "LZ-INCIDENT-NOT-FOUND");
    assert!(
        list_incidents(&harness.app, json!({"state": "open"}))
            .await
            .is_empty()
    );
    assert_eq!(
        list_incidents(&harness.app, json!({"state": "resolved"}))
            .await
            .len(),
        1
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Two independent failure paths prove their scope in one fixture.
async fn dead_letters_and_dispatch_failures_raise_incidents_with_their_scope() {
    use lazoo_protocol::{MessageSender, UserMessageEndpoint, UserMessageKind};
    use lazoo_store::{RecordTurnInput, UpdateEmployeeInput};

    let (_temporary, _root, _store, app, sender, recipient) = message_test_app();
    let sent = app
        .handle(request(
            "send-incident",
            "message.send",
            json!({
                "sender": {"employeeId": sender},
                "recipient": {"employeeId": recipient},
                "kind": "peer",
                "body": {"text": "ping"},
                "idempotencyKey": "send-incident"
            }),
        ))
        .await
        .result
        .unwrap();
    let message_id = sent["message"]["envelope"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let claimed = app
        .handle(request(
            "claim-incident",
            "message.claim",
            json!({"id": message_id, "expectedRevision": 1, "idempotencyKey": "claim-incident"}),
        ))
        .await
        .result
        .unwrap();
    let attempt_id = claimed["message"]["attempts"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let fail_params = json!({
        "id": message_id,
        "expectedRevision": 2,
        "attemptId": attempt_id,
        "retryable": false,
        "failureCode": "LZ-MESSAGE-TIMEOUT",
        "failureMessage": "recipient never acknowledged",
        "idempotencyKey": "fail-incident"
    });
    let failed = app
        .handle(request(
            "fail-incident",
            "message.fail",
            fail_params.clone(),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(failed["message"]["delivery"]["state"], "terminal-failed");
    assert!(failed["message"]["deadLetter"].is_object());
    app.handle(request("fail-incident-replay", "message.fail", fail_params))
        .await
        .result
        .unwrap();
    let incidents = list_incidents(&app, json!({"code": "LZ-MESSAGE-DEADLETTER"})).await;
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0]["messageId"], message_id);
    assert_eq!(incidents[0]["employeeId"], recipient.to_string());
    assert_eq!(
        incidents[0]["occurrences"], 1,
        "a replayed failure raises nothing new"
    );
    assert_eq!(incidents[0]["detail"]["failureCode"], "LZ-MESSAGE-TIMEOUT");
    assert_eq!(
        incidents[0]["title"],
        "Message exhausted safe delivery attempts"
    );

    let (_temporary, harness) = tool_harness(AuthorityMode::FullAutonomy);
    let warmed = harness
        .app
        .handle(request(
            "warm-dispatch-lead",
            "session.warm",
            json!({
                "employeeId": harness.lead.id,
                "expectedEmployeeRevision": harness.lead.configuration_revision,
                "idempotencyKey": "warm-dispatch-lead"
            }),
        ))
        .await
        .result
        .unwrap();
    let lead_session_id = warmed["session"]["id"].as_str().unwrap().to_owned();
    let root_task = harness
        .store
        .record_turn(&RecordTurnInput {
            request_id: "incident-root-request".to_owned(),
            idempotency_key: "incident-root".to_owned(),
            conversation_id: harness.conversation_id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::FullAutonomy,
            text: "Lead objective".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap()
        .task
        .unwrap();
    let delegated = harness
        .app
        .handle(request(
            "incident-delegate",
            "task.delegate",
            json!({
                "parentTaskId": root_task.id,
                "actor": {"employeeId": harness.lead.id, "sessionId": lead_session_id},
                "assignedEmployeeId": harness.worker.id,
                "objective": "Use the drifted worker",
                "references": [],
                "requestedAuthority": "full-autonomy",
                "rosterRevision": harness.store.roster_snapshot().unwrap().revision,
                "idempotencyKey": "incident-delegate"
            }),
        ))
        .await
        .result
        .unwrap();
    let task_id = delegated["task"]["id"].as_str().unwrap().to_owned();
    harness
        .store
        .update_employee(&UpdateEmployeeInput {
            request_id: "incident-drift-request".to_owned(),
            idempotency_key: "incident-drift".to_owned(),
            employee_id: harness.worker.id,
            expected_revision: harness.worker.configuration_revision,
            name: None,
            provider: None,
            role: Some("Updated worker".to_owned()),
            instructions: None,
            model: None,
            reasoning_effort: None,
            native_settings: None,
            authority_mode: None,
            checkout_policy: None,
            enabled: None,
            provider_observation: None,
        })
        .unwrap();
    let error = harness.app.dispatch_pending_tasks_once().await.unwrap_err();
    assert_eq!(error.code, "LZ-TASK-EMPLOYEE-STALE");
    let incidents = list_incidents(&harness.app, json!({"code": "LZ-CONFIG-INCOMPATIBLE"})).await;
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0]["taskId"], task_id);
    assert_eq!(incidents[0]["employeeId"], harness.worker.id.to_string());
    assert_eq!(
        incidents[0]["detail"]["failureCode"],
        "LZ-TASK-EMPLOYEE-STALE"
    );
    assert_eq!(
        incidents[0]["title"],
        "Configuration changed before dispatch"
    );
    assert!(incidents[0]["messageId"].is_string());
}

#[tokio::test]
async fn restart_owner_loss_and_emergency_spool_fold_into_startup_incidents_once() {
    use lazoo_store::{BeginSessionInput, MarkSessionReadyInput};

    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let lead = configured_employee_named(&store, "Rhea", "restart-rhea");
    let peer = configured_employee_named(&store, "Mira", "restart-mira");
    let foreign_boot = uuid::Uuid::new_v4();
    for (employee, key, pid) in [(&lead, "lead", 4001), (&peer, "peer", 4002)] {
        let starting = store
            .begin_session(&BeginSessionInput {
                request_id: format!("restart-{key}-request"),
                idempotency_key: format!("restart-{key}"),
                employee_id: employee.id,
                expected_employee_revision: employee.configuration_revision,
                owner_boot_id: foreign_boot,
            })
            .unwrap();
        store
            .mark_session_ready(&MarkSessionReadyInput {
                session_id: starting.session.id,
                expected_revision: starting.session.revision,
                owner_boot_id: foreign_boot,
                native_session_id: format!("native-{key}"),
                native_host_pid: pid,
            })
            .unwrap();
    }
    std::fs::write(
        root.spool_pending().join("incident-1.ndjson"),
        concat!(
            r#"{"incident_id":"incident-1","code":"LZ-DAEMON-UNAVAILABLE","#,
            r#""message":"restart budget exhausted","occurred_at":"2026-09-23T00:00:00Z"}"#,
            "\n"
        ),
    )
    .unwrap();
    assert_eq!(store.import_pending_incidents(&root).unwrap(), 1);

    let app = App::new_with_provider_environment(
        store.clone(),
        root.clone(),
        DiscoveryEnvironment::empty(),
    )
    .unwrap();
    let incidents = list_incidents(&app, json!({})).await;
    assert_eq!(incidents.len(), 3);
    let owner_lost: Vec<_> = incidents
        .iter()
        .filter(|incident| incident["code"] == "LZ-SESSION-OWNER-LOST")
        .collect();
    assert_eq!(
        owner_lost.len(),
        2,
        "one folded incident per employee scope"
    );
    for incident in &owner_lost {
        assert_eq!(incident["title"], "In-flight work lost its daemon");
        assert_eq!(incident["occurrences"], 1);
        assert!(incident["sessionId"].is_string());
        let employee_id = incident["employeeId"].as_str().unwrap();
        assert!(employee_id == lead.id.to_string() || employee_id == peer.id.to_string());
    }
    let keeper = incidents
        .iter()
        .find(|incident| incident["code"] == "LZ-DAEMON-UNAVAILABLE")
        .expect("the emergency spool record folds into an incident");
    assert_eq!(keeper["source"], "keeper");
    assert_eq!(keeper["explanation"], "restart budget exhausted");
    assert_eq!(keeper["firstOccurredAt"], "2026-09-23T00:00:00Z");

    let again =
        App::new_with_provider_environment(store.clone(), root, DiscoveryEnvironment::empty())
            .unwrap();
    let incidents = list_incidents(&again, json!({})).await;
    assert_eq!(incidents.len(), 3, "a second boot raises nothing new");
    assert!(
        incidents
            .iter()
            .all(|incident| incident["occurrences"] == 1)
    );
    let status = again
        .handle(request("status", "system.status", json!({})))
        .await
        .result
        .unwrap();
    assert_eq!(status["schemaVersion"], 9);
    assert_eq!(status["emergencyIncidentCount"], 1);
}

#[tokio::test]
async fn incident_tools_report_and_list_within_the_caller_scope() {
    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    warm_tool_lead(&harness).await;
    harness.backend.tool_calls.lock().unwrap().extend([
        tool_call(
            "call-1",
            "incident_report",
            json!({
                "title": "Dependency missing",
                "explanation": "npm install fails behind the proxy",
                "detail": {"package": "left-pad"}
            }),
        ),
        tool_call(
            "call-2",
            "incident_report",
            json!({"title": "Dependency missing", "explanation": "still failing"}),
        ),
        tool_call("call-3", "incident_list", json!({})),
        tool_call(
            "call-4",
            "incident_report",
            json!({"explanation": "no title"}),
        ),
    ]);

    let submitted = submit_lead_turn(&harness, "self-decision").await;
    assert_eq!(submitted["turn"]["state"], "completed");
    let declared = harness.backend.session_tools.lock().unwrap().clone();
    assert!(declared[0].contains(&"incident_report".to_owned()));
    assert!(declared[0].contains(&"incident_list".to_owned()));

    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 4);
    assert!(results[0].success, "{:?}", results[0]);
    assert_eq!(results[0].content["state"], "open");
    assert_eq!(results[0].content["priority"], "normal");
    assert_eq!(results[0].content["occurrences"], 1);
    assert!(results[1].success, "{:?}", results[1]);
    assert_eq!(
        results[1].content["incidentId"],
        results[0].content["incidentId"]
    );
    assert_eq!(results[1].content["occurrences"], 2);
    assert!(results[2].success, "{:?}", results[2]);
    let visible = results[2].content["incidents"].as_array().unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0]["code"], "LZ-EMPLOYEE-REPORTED");
    assert_eq!(visible[0]["title"], "Dependency missing");
    assert!(
        visible[0]["nextAction"]
            .as_str()
            .unwrap()
            .contains("Review")
    );
    assert!(!results[3].success);
    assert_eq!(results[3].content["error"]["code"], "LZ-TOOL-INVALID");

    let incident_id: uuid::Uuid = results[0].content["incidentId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let incident = harness.store.get_incident(incident_id).unwrap();
    assert_eq!(incident.source, lazoo_protocol::IncidentSource::Employee);
    assert_eq!(incident.employee_id, Some(harness.lead.id));
    assert_eq!(
        incident.task_id.map(|id| id.to_string()),
        submitted["turn"]["taskId"].as_str().map(str::to_owned)
    );
    assert_eq!(
        incident.turn_id.map(|id| id.to_string()),
        submitted["turn"]["id"].as_str().map(str::to_owned)
    );
    assert_eq!(incident.explanation, "still failing");
    assert_eq!(incident.occurrences, 2);
}

#[test]
fn minimum_incidents_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script =
        std::fs::read_to_string(repository.join("scripts/acceptance/minimum-incidents.ps1"))
            .unwrap();
    for required in [
        "known_codes_describe_the_situation_and_the_next_safe_action",
        "equivalent_failures_fold_escalate_at_the_threshold_and_replay_without_writes",
        "acknowledge_and_resolve_are_revisioned_idempotent_and_recurrence_reopens_until_resolved",
        "emergency_spool_records_fold_into_keeper_incidents_exactly_once",
        "session_start_failure_raises_a_human_readable_incident",
        "provider_turn_failures_fold_into_one_incident_escalate_and_settle_through_rpc",
        "dead_letters_and_dispatch_failures_raise_incidents_with_their_scope",
        "restart_owner_loss_and_emergency_spool_fold_into_startup_incidents_once",
        "incident_tools_report_and_list_within_the_caller_scope",
        "incident_cli_maps_list_show_acknowledge_and_resolve",
        "incident_errors_use_exit_code_fifteen",
        "minimum-incidents-manifest.json",
        "deterministic-fake-transport",
        "schemaVersion = 9",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in [
        "provider', 'smoke",
        "apps\\lazoo-desktop\\src",
        "LivePrompt",
    ] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Lead notice delivery: child results and promoted incidents reach the lead after its turn
// ended, without employee-authored traffic ever starting a provider process.
// ---------------------------------------------------------------------------------------------

fn lead_messages(
    store: &Store,
    kind: lazoo_protocol::MessageKind,
) -> Vec<lazoo_protocol::MessageSummary> {
    store
        .list_messages(0, 100)
        .unwrap()
        .into_iter()
        .filter(|message| message.kind == kind)
        .collect()
}

/// Turns addressed to the lead, in order; the worker's dispatch turn shares the conversation.
fn lead_turns(harness: &ToolHarness) -> Vec<lazoo_protocol::ConversationTurnSummary> {
    harness
        .store
        .list_turns(harness.conversation_id, 0, 10)
        .unwrap()
        .into_iter()
        .filter(|turn| turn.recipient.employee_id == harness.lead.id)
        .collect()
}

fn last_prompt(backend: &FakeSessionBackend) -> String {
    backend
        .turn_requests
        .lock()
        .unwrap()
        .last()
        .map(|request| request.prompt.clone())
        .unwrap_or_default()
}

/// Lead turn that delegates one child and ends without waiting; returns (root task id, child id).
async fn delegate_without_waiting(harness: &ToolHarness) -> (uuid::Uuid, uuid::Uuid) {
    use lazoo_protocol::TaskState;

    warm_tool_lead(harness).await;
    let roster_revision = harness.store.roster_snapshot().unwrap().revision;
    harness
        .backend
        .tool_calls
        .lock()
        .unwrap()
        .extend([tool_call(
            "call-1",
            "task_delegate",
            json!({
                "employeeId": harness.worker.id,
                "objective": "Implement the parser fix",
                "rosterRevision": roster_revision,
                "requestedAuthority": "self-decision"
            }),
        )]);
    let submitted = submit_lead_turn(harness, "self-decision").await;
    assert_eq!(submitted["turn"]["state"], "completed");
    let root_task_id: uuid::Uuid = submitted["turn"]["taskId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert!(results[0].success, "{:?}", results[0]);
    let child_id: uuid::Uuid = results[0].content["taskId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let root = harness.store.get_task(root_task_id).unwrap().task;
    assert_eq!(
        root.state,
        TaskState::Running,
        "the lead's task stays running while its child is unfinished"
    );
    assert!(root.active_turn_id.is_none());
    (root_task_id, child_id)
}

#[tokio::test]
async fn child_results_reach_an_idle_lead_as_a_notice_turn_after_its_turn_ended() {
    use lazoo_protocol::{MessageKind, MessageSender, TaskState};

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    let (root_task_id, child_id) = delegate_without_waiting(&harness).await;
    harness.app.dispatch_pending_tasks_once().await.unwrap();
    let child = harness.store.get_task(child_id).unwrap().task;
    assert_eq!(child.state, TaskState::Completed);
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 2);
    assert_eq!(
        harness.store.pending_task_results(10).unwrap(),
        vec![child_id],
        "the terminal child has a result waiting to be queued"
    );
    assert!(lead_messages(&harness.store, MessageKind::Result).is_empty());

    harness.app.deliver_pending_notices_once().await.unwrap();

    assert_eq!(
        harness.backend.turns.load(Ordering::SeqCst),
        3,
        "the notice is delivered as one more turn on the lead's idle session"
    );
    assert_eq!(
        harness.backend.starts.load(Ordering::SeqCst),
        2,
        "no provider process was started for the notice"
    );
    let prompt = last_prompt(&harness.backend);
    assert!(prompt.starts_with("[Lazoo task result]"), "{prompt}");
    assert!(prompt.contains(&child_id.to_string()));
    assert!(prompt.contains("finished with state completed"));
    assert!(prompt.contains("Output:\ndone"));
    assert!(prompt.contains(&root_task_id.to_string()));

    let results = lead_messages(&harness.store, MessageKind::Result);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].state, MessageDeliveryState::Acknowledged);
    assert_eq!(results[0].recipient.employee_id, harness.lead.id);
    assert_eq!(results[0].task_id, Some(root_task_id));
    let MessageSender::Employee(sender) = results[0].sender else {
        panic!("a result notice is authored by the worker endpoint");
    };
    assert_eq!(sender.employee_id, harness.worker.id);

    let turns = lead_turns(&harness);
    assert_eq!(
        turns.len(),
        2,
        "the lead has its user turn and the notice turn; the worker turn is the child's"
    );
    assert_eq!(turns[1].state, TurnState::Completed);
    assert_eq!(turns[1].task_id, Some(root_task_id));
    assert_eq!(turns[1].recipient.employee_id, harness.lead.id);
    let root = harness.store.get_task(root_task_id).unwrap().task;
    assert_eq!(
        root.state,
        TaskState::Completed,
        "the lead ended its notice turn with no child left, so its task completed"
    );

    harness.app.deliver_pending_notices_once().await.unwrap();
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 3);
    assert!(harness.store.pending_lead_notices(10).unwrap().is_empty());
    let event_types: Vec<String> = harness
        .store
        .list_events(0, 1000)
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .collect();
    assert_eq!(
        event_types
            .iter()
            .filter(|kind| *kind == "turn.recorded")
            .count(),
        2
    );
    assert!(
        event_types
            .iter()
            .any(|kind| kind == "message.acknowledged")
    );
}

#[tokio::test]
async fn task_wait_consumes_child_results_so_no_notice_turn_follows() {
    use lazoo_protocol::MessageKind;

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    warm_tool_lead(&harness).await;
    let roster_revision = harness.store.roster_snapshot().unwrap().revision;
    harness.backend.tool_calls.lock().unwrap().extend([
        tool_call(
            "call-1",
            "task_delegate",
            json!({
                "employeeId": harness.worker.id,
                "objective": "Implement the parser fix",
                "rosterRevision": roster_revision,
                "requestedAuthority": "self-decision"
            }),
        ),
        tool_call("call-2", "task_wait", json!({"timeoutSeconds": 10})),
    ]);
    let dispatcher_app = harness.app.clone();
    let dispatcher = tokio::spawn(async move { dispatcher_app.run_task_dispatcher().await });
    let submitted = submit_lead_turn(&harness, "self-decision").await;
    assert_eq!(submitted["turn"]["state"], "completed");
    dispatcher.abort();
    let _ = dispatcher.await;
    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert!(results[1].success, "{:?}", results[1]);
    assert_eq!(results[1].content["timedOut"], false);
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 2);

    let notices = lead_messages(&harness.store, MessageKind::Result);
    assert_eq!(
        notices.len(),
        1,
        "task_wait queued the child's result itself"
    );
    assert_eq!(notices[0].state, MessageDeliveryState::Acknowledged);
    let record = harness.store.get_message(notices[0].id).unwrap();
    let attempt = record.attempts.last().unwrap();
    assert_eq!(
        attempt.acknowledgement_evidence.as_ref().unwrap()["via"],
        "task_wait"
    );
    assert_eq!(
        attempt.acknowledgement_evidence.as_ref().unwrap()["turnId"],
        submitted["turn"]["id"]
    );

    harness.app.deliver_pending_notices_once().await.unwrap();
    assert_eq!(
        harness.backend.turns.load(Ordering::SeqCst),
        2,
        "a result the lead already read through task_wait is never delivered twice"
    );
    assert!(harness.store.pending_task_results(10).unwrap().is_empty());
    assert!(harness.store.pending_lead_notices(10).unwrap().is_empty());
}

#[tokio::test]
async fn notices_wait_for_a_dormant_lead_until_the_user_resumes_it() {
    use lazoo_protocol::{MessageKind, SessionLifecycleState, TaskState};

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    let (root_task_id, child_id) = delegate_without_waiting(&harness).await;
    harness.app.dispatch_pending_tasks_once().await.unwrap();
    assert_eq!(
        harness.store.get_task(child_id).unwrap().task.state,
        TaskState::Completed
    );
    let lead_session = harness
        .store
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|session| {
            session.employee_id == harness.lead.id && session.state == SessionLifecycleState::Idle
        })
        .expect("the lead session is idle after its turn");
    harness
        .app
        .handle(request(
            "stop-lead",
            "session.stop",
            json!({
                "id": lead_session.id,
                "expectedRevision": lead_session.revision,
                "idempotencyKey": "stop-lead"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 2);

    harness.app.deliver_pending_notices_once().await.unwrap();

    assert_eq!(
        harness.backend.starts.load(Ordering::SeqCst),
        2,
        "an employee-authored notice never activates a dormant lead"
    );
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 2);
    let queued = lead_messages(&harness.store, MessageKind::Result);
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].state, MessageDeliveryState::Queued);
    let turns = lead_turns(&harness);
    assert_eq!(turns.len(), 2);
    assert_eq!(
        turns[1].state,
        TurnState::Recorded,
        "the notice turn is recorded and waits for the lead"
    );
    assert_eq!(
        harness.store.get_task(root_task_id).unwrap().task.state,
        TaskState::Running
    );

    // The local user resumes the lead explicitly; the next pass delivers the waiting notice.
    harness
        .app
        .handle(request(
            "warm-lead-again",
            "session.warm",
            json!({
                "employeeId": harness.lead.id,
                "expectedEmployeeRevision": harness.lead.configuration_revision,
                "idempotencyKey": "warm-lead-again"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 3);
    harness.app.deliver_pending_notices_once().await.unwrap();

    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 3);
    assert!(last_prompt(&harness.backend).contains(&child_id.to_string()));
    let delivered = lead_messages(&harness.store, MessageKind::Result);
    assert_eq!(delivered[0].state, MessageDeliveryState::Acknowledged);
    let turns = lead_turns(&harness);
    assert_eq!(
        turns.len(),
        2,
        "the same recorded turn was delivered, not a new one"
    );
    assert_eq!(turns[1].state, TurnState::Completed);
    assert_eq!(
        harness.store.get_task(root_task_id).unwrap().task.state,
        TaskState::Completed
    );
}

#[tokio::test]
async fn escalated_incidents_notify_the_lead_once_as_a_status_turn() {
    use lazoo_protocol::{IncidentPriority, IncidentSource, MessageKind, TaskState};
    use lazoo_store::RaiseIncidentInput;

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    let (root_task_id, child_id) = delegate_without_waiting(&harness).await;
    let raise = |key: &str, employee: uuid::Uuid, task: uuid::Uuid| RaiseIncidentInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        code: "LZ-MESSAGE-TIMEOUT".to_owned(),
        source: IncidentSource::Daemon,
        title: None,
        explanation: None,
        project_id: None,
        employee_id: Some(employee),
        provider: Some("codex".to_owned()),
        task_id: Some(task),
        session_id: None,
        turn_id: None,
        message_id: None,
        detail: json!({"failureCode": "LZ-MESSAGE-TIMEOUT"}),
        occurred_at: None,
    };
    for key in ["timeout-1", "timeout-2"] {
        harness
            .store
            .raise_incident(&raise(key, harness.worker.id, child_id))
            .unwrap();
    }
    harness.app.deliver_pending_notices_once().await.unwrap();
    assert_eq!(
        harness.backend.turns.load(Ordering::SeqCst),
        1,
        "a normal incident does not interrupt the lead"
    );
    let escalated = harness
        .store
        .raise_incident(&raise("timeout-3", harness.worker.id, child_id))
        .unwrap()
        .incident;
    assert_eq!(escalated.priority, IncidentPriority::Priority);

    harness.app.deliver_pending_notices_once().await.unwrap();

    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 2);
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 1);
    let prompt = last_prompt(&harness.backend);
    assert!(
        prompt.starts_with("[Lazoo incident] Priority incident LZ-MESSAGE-TIMEOUT"),
        "{prompt}"
    );
    assert!(prompt.contains(&child_id.to_string()));
    assert!(prompt.contains(&escalated.title));
    assert!(prompt.contains(&escalated.id.to_string()));
    let notices = lead_messages(&harness.store, MessageKind::Status);
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].state, MessageDeliveryState::Acknowledged);
    assert_eq!(notices[0].task_id, Some(root_task_id));
    assert_eq!(
        harness.store.get_task(root_task_id).unwrap().task.state,
        TaskState::Running,
        "the child is still accepted, so the lead's task stays running"
    );

    harness.app.deliver_pending_notices_once().await.unwrap();
    assert_eq!(
        harness.backend.turns.load(Ordering::SeqCst),
        2,
        "one promoted incident produces exactly one notice"
    );

    // Incidents about the lead itself are visible to the user but never sent to the lead.
    for key in ["lead-1", "lead-2", "lead-3"] {
        harness
            .store
            .raise_incident(&raise(key, harness.lead.id, root_task_id))
            .unwrap();
    }
    harness.app.deliver_pending_notices_once().await.unwrap();
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 2);
    assert_eq!(lead_messages(&harness.store, MessageKind::Status).len(), 1);
    assert!(
        harness
            .store
            .pending_incident_notices(10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn result_delivery_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/result-delivery.ps1");
    let script = std::fs::read_to_string(&script_path).expect("result delivery acceptance script");

    for required in [
        "completed_result_names_the_child_its_output_and_the_parent",
        "oversized_output_is_bounded_on_a_utf8_boundary_and_marked",
        "incident_notice_states_the_code_title_action_and_boundary",
        "terminal_child_results_queue_once_to_the_parent_lead",
        "notice_turns_reuse_the_notice_message_and_wait_for_the_lead_to_be_free",
        "task_wait_consumes_a_result_with_the_waiting_turn_as_evidence",
        "a_foreign_turn_cannot_consume_another_leads_result",
        "priority_incidents_on_a_child_notify_the_lead_once",
        "child_results_reach_an_idle_lead_as_a_notice_turn_after_its_turn_ended",
        "task_wait_consumes_child_results_so_no_notice_turn_follows",
        "notices_wait_for_a_dormant_lead_until_the_user_resumes_it",
        "escalated_incidents_notify_the_lead_once_as_a_status_turn",
        "store_enables_wal_foreign_keys_and_schema_version",
        "dormantLeadNeverActivated",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 9",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in [
        "provider', 'smoke",
        "apps\\lazoo-desktop\\src",
        "LivePrompt",
    ] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// Peer messages and replies in the message ledger, oldest first; user turn messages share
/// the `peer` kind, so employee authorship is what distinguishes them.
fn employee_messages(store: &Store) -> Vec<lazoo_protocol::MessageSummary> {
    use lazoo_protocol::{MessageKind, MessageSender};

    store
        .list_messages(0, 100)
        .unwrap()
        .into_iter()
        .filter(|message| {
            matches!(message.kind, MessageKind::Peer | MessageKind::Reply)
                && matches!(message.sender, MessageSender::Employee(_))
        })
        .collect()
}

fn acknowledgement_via(store: &Store, message_id: uuid::Uuid) -> serde_json::Value {
    store
        .get_message(message_id)
        .unwrap()
        .attempts
        .last()
        .and_then(|attempt| attempt.acknowledgement_evidence.clone())
        .map(|evidence| evidence["via"].clone())
        .unwrap_or_default()
}

fn prompts(backend: &FakeSessionBackend) -> Vec<String> {
    backend
        .turn_requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.prompt.clone())
        .collect()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One worker question and one lead answer, end to end.
async fn peer_questions_reach_an_idle_lead_as_a_turn_and_the_reply_returns_to_the_waiting_worker() {
    use lazoo_protocol::{MessageKind, TaskEvidenceKind, TaskState};

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    let (root_task_id, child_id) = delegate_without_waiting(&harness).await;
    // The worker asks the lead mid-turn and waits; the idle lead answers inside the peer turn
    // the notice loop delivers, citing the message id it reads in that turn's text.
    harness.backend.tool_calls.lock().unwrap().extend([
        session_tool_call(
            "native-2",
            "call-w1",
            "message_send",
            json!({
                "employeeId": harness.lead.id,
                "text": "Which spacing token should the parser emit?",
                "waitForReplySeconds": 10
            }),
        ),
        session_tool_call(
            "native-1",
            "call-l2",
            "message_reply",
            json!({"messageId": "$message-id", "text": "Use spacing.md; the token is space-4."}),
        ),
    ]);
    let notices_app = harness.app.clone();
    let notices = tokio::spawn(async move { notices_app.run_notice_delivery().await });
    timeout(
        Duration::from_secs(20),
        harness.app.dispatch_pending_tasks_once(),
    )
    .await
    .expect("the worker turn ends once the reply arrives")
    .unwrap();
    timeout(Duration::from_secs(10), async {
        while harness.store.get_task(root_task_id).unwrap().task.state != TaskState::Completed {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the child's result completes the lead's task");
    notices.abort();
    let _ = notices.await;

    assert_eq!(
        harness.backend.starts.load(Ordering::SeqCst),
        2,
        "no provider process was started for peer traffic"
    );
    assert_eq!(
        harness.backend.turns.load(Ordering::SeqCst),
        4,
        "lead user turn, worker turn, lead peer turn, lead result turn"
    );
    let prompts = prompts(&harness.backend);
    let peer_prompt = prompts
        .iter()
        .find(|prompt| prompt.starts_with("[Lazoo peer message]"))
        .expect("the lead received the question as a turn");
    assert!(
        peer_prompt.contains(&format!("Mira (task {child_id}) sent you message")),
        "{peer_prompt}"
    );
    assert!(peer_prompt.contains("Which spacing token should the parser emit?"));
    assert!(peer_prompt.contains(&format!("about your task {root_task_id}")));
    assert!(peer_prompt.contains("not an approval"));

    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 3);
    assert!(results[1].success, "{:?}", results[1]);
    assert_eq!(
        results[1].content["recipientEmployeeId"],
        harness.worker.id.to_string()
    );
    assert_eq!(results[1].content["recipientTaskId"], child_id.to_string());
    assert!(results[2].success, "{:?}", results[2]);
    assert_eq!(results[2].content["timedOut"], false);
    assert_eq!(results[2].content["state"], "acknowledged");
    assert_eq!(
        results[2].content["reply"]["text"],
        "Use spacing.md; the token is space-4."
    );
    assert_eq!(
        results[2].content["reply"]["fromEmployeeId"],
        harness.lead.id.to_string()
    );
    assert_eq!(
        results[2].content["reply"]["replyToMessageId"],
        results[2].content["messageId"]
    );
    assert_eq!(results[2].content["messages"].as_array().unwrap().len(), 1);

    let messages = employee_messages(&harness.store);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].kind, MessageKind::Peer);
    assert_eq!(messages[0].state, MessageDeliveryState::Acknowledged);
    assert_eq!(messages[0].task_id, Some(root_task_id));
    assert_eq!(messages[1].kind, MessageKind::Reply);
    assert_eq!(messages[1].state, MessageDeliveryState::Acknowledged);
    assert_eq!(messages[1].task_id, Some(child_id));
    assert_eq!(messages[1].causation_id, Some(messages[0].id));
    assert_eq!(
        acknowledgement_via(&harness.store, messages[1].id),
        "message_send"
    );

    let turns = lead_turns(&harness);
    assert_eq!(turns.len(), 3, "user turn, peer turn, result turn");
    assert!(turns.iter().all(|turn| turn.state == TurnState::Completed));
    let root = harness.store.get_task(root_task_id).unwrap();
    assert!(
        root.evidence.iter().any(|evidence| {
            evidence.turn_id == Some(turns[1].id) && evidence.kind == TaskEvidenceKind::Progress
        }),
        "the peer turn only progressed the lead's task"
    );
    assert!(root.evidence.iter().any(|evidence| {
        evidence.turn_id == Some(turns[2].id) && evidence.kind == TaskEvidenceKind::Completed
    }));
    assert_eq!(
        harness.store.get_task(child_id).unwrap().task.state,
        TaskState::Completed
    );
}

#[tokio::test]
async fn task_wait_returns_early_with_a_peer_question_so_the_lead_answers_in_turn() {
    use lazoo_protocol::{MessageKind, TaskState};

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    warm_tool_lead(&harness).await;
    let roster_revision = harness.store.roster_snapshot().unwrap().revision;
    harness.backend.tool_calls.lock().unwrap().extend([
        tool_call(
            "call-1",
            "task_delegate",
            json!({
                "employeeId": harness.worker.id,
                "objective": "Implement the parser fix",
                "rosterRevision": roster_revision,
                "requestedAuthority": "self-decision"
            }),
        ),
        tool_call("call-2", "task_wait", json!({"timeoutSeconds": 10})),
        tool_call(
            "call-3",
            "message_reply",
            json!({"messageId": "$message-id", "text": "space-4"}),
        ),
        tool_call("call-4", "task_wait", json!({"timeoutSeconds": 10})),
        session_tool_call(
            "native-2",
            "call-w1",
            "message_send",
            json!({
                "employeeId": harness.lead.id,
                "text": "Which token?",
                "waitForReplySeconds": 10
            }),
        ),
    ]);
    let dispatcher_app = harness.app.clone();
    let dispatcher = tokio::spawn(async move { dispatcher_app.run_task_dispatcher().await });
    let submitted = submit_lead_turn(&harness, "self-decision").await;
    assert_eq!(submitted["turn"]["state"], "completed");
    dispatcher.abort();
    let _ = dispatcher.await;

    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 5, "{results:?}");
    assert!(results[1].success, "{:?}", results[1]);
    assert_eq!(results[1].content["timedOut"], false);
    assert_eq!(results[1].content["tasks"][0]["state"], "running");
    let inbox = results[1].content["messages"].as_array().unwrap();
    assert_eq!(inbox.len(), 1, "task_wait returned early with the question");
    assert_eq!(inbox[0]["kind"], "peer");
    assert_eq!(inbox[0]["text"], "Which token?");
    assert_eq!(inbox[0]["fromEmployeeId"], harness.worker.id.to_string());
    assert_eq!(inbox[0]["fromEmployeeName"], "Mira");
    assert!(results[2].success, "{:?}", results[2]);
    assert_eq!(
        results[2].content["replyToMessageId"],
        inbox[0]["messageId"]
    );
    assert!(results[3].success, "{:?}", results[3]);
    assert_eq!(results[3].content["reply"]["text"], "space-4");
    assert_eq!(results[3].content["timedOut"], false);
    assert_eq!(results[3].content["state"], "acknowledged");
    assert!(results[4].success, "{:?}", results[4]);
    assert_eq!(results[4].content["timedOut"], false);
    assert_eq!(results[4].content["tasks"][0]["state"], "completed");
    assert!(
        results[4].content["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 2);
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 2);
    assert_eq!(
        lead_turns(&harness).len(),
        1,
        "the whole exchange happened inside the lead's own turn"
    );
    let messages = employee_messages(&harness.store);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].kind, MessageKind::Peer);
    assert_eq!(messages[0].state, MessageDeliveryState::Acknowledged);
    assert_eq!(
        acknowledgement_via(&harness.store, messages[0].id),
        "task_wait"
    );
    assert_eq!(messages[1].kind, MessageKind::Reply);
    assert_eq!(messages[1].state, MessageDeliveryState::Acknowledged);
    assert_eq!(
        acknowledgement_via(&harness.store, messages[1].id),
        "message_send"
    );
    let root_task_id: uuid::Uuid = submitted["turn"]["taskId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        harness.store.get_task(root_task_id).unwrap().task.state,
        TaskState::Completed
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Dormant recipient, resume, refusals, and ordering in one story.
async fn peer_messages_never_activate_a_dormant_recipient_and_refusals_never_end_the_turn() {
    use lazoo_protocol::{MessageKind, SessionLifecycleState, TaskEvidenceKind, TaskState};

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    let (root_task_id, child_id) = delegate_without_waiting(&harness).await;
    let lead_session = harness
        .store
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|session| {
            session.employee_id == harness.lead.id && session.state == SessionLifecycleState::Idle
        })
        .expect("the lead session is idle after its turn");
    harness
        .app
        .handle(request(
            "stop-lead",
            "session.stop",
            json!({
                "id": lead_session.id,
                "expectedRevision": lead_session.revision,
                "idempotencyKey": "stop-lead"
            }),
        ))
        .await
        .result
        .unwrap();

    harness
        .backend
        .tool_calls
        .lock()
        .unwrap()
        .push(session_tool_call(
            "native-2",
            "call-w1",
            "message_send",
            json!({
                "employeeId": harness.lead.id,
                "text": "Heads up: the parser fix is ready for review."
            }),
        ));
    harness.app.dispatch_pending_tasks_once().await.unwrap();
    assert_eq!(
        harness.store.get_task(child_id).unwrap().task.state,
        TaskState::Completed
    );
    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert!(results[1].success, "{:?}", results[1]);
    assert_eq!(results[1].content["state"], "queued");
    assert_eq!(results[1].content["timedOut"], false);
    assert!(results[1].content["reply"].is_null());

    harness.app.deliver_pending_notices_once().await.unwrap();

    assert_eq!(
        harness.backend.starts.load(Ordering::SeqCst),
        2,
        "a peer message never activates a dormant recipient"
    );
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 2);
    let turns = lead_turns(&harness);
    assert_eq!(turns.len(), 2);
    assert_eq!(
        turns[1].state,
        TurnState::Recorded,
        "the peer turn is recorded and waits for the lead"
    );
    let messages = employee_messages(&harness.store);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].state, MessageDeliveryState::Queued);
    let result_notices = lead_messages(&harness.store, MessageKind::Result);
    assert_eq!(result_notices.len(), 1);
    assert_eq!(result_notices[0].state, MessageDeliveryState::Queued);
    assert_eq!(
        harness.store.get_task(root_task_id).unwrap().task.state,
        TaskState::Running
    );

    // The user resumes the lead. Inside the peer turn the lead tries four calls that must all
    // be refused with a stable code and must not end the turn; the child's result then follows
    // in the same pass and completes the lead's task.
    // The resumed lead runs on the third session the backend starts.
    harness.backend.tool_calls.lock().unwrap().extend([
        session_tool_call(
            "native-3",
            "call-l2",
            "message_send",
            json!({"employeeId": harness.lead.id, "text": "note to self"}),
        ),
        session_tool_call(
            "native-3",
            "call-l3",
            "message_send",
            json!({"employeeId": harness.worker.id, "text": "too late"}),
        ),
        session_tool_call(
            "native-3",
            "call-l4",
            "message_reply",
            json!({"messageId": uuid::Uuid::new_v4(), "text": "?"}),
        ),
        session_tool_call(
            "native-3",
            "call-l5",
            "message_send",
            json!({"employeeId": harness.worker.id}),
        ),
    ]);
    harness
        .app
        .handle(request(
            "warm-lead-again",
            "session.warm",
            json!({
                "employeeId": harness.lead.id,
                "expectedEmployeeRevision": harness.lead.configuration_revision,
                "idempotencyKey": "warm-lead-again"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 3);
    harness.app.deliver_pending_notices_once().await.unwrap();

    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 4);
    let prompts = prompts(&harness.backend);
    assert!(
        prompts[2].starts_with("[Lazoo peer message]"),
        "{}",
        prompts[2]
    );
    assert!(prompts[2].contains("Heads up: the parser fix is ready for review."));
    assert!(
        prompts[3].starts_with("[Lazoo task result]"),
        "{}",
        prompts[3]
    );
    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 6);
    let codes: Vec<&str> = results[2..]
        .iter()
        .map(|result| {
            assert!(!result.success);
            result.content["error"]["code"].as_str().unwrap()
        })
        .collect();
    assert_eq!(
        codes,
        vec![
            "LZ-TOOL-INVALID",
            "LZ-TOOL-SCOPE",
            "LZ-TOOL-SCOPE",
            "LZ-TOOL-INVALID"
        ]
    );
    let turns = lead_turns(&harness);
    assert_eq!(
        turns.len(),
        3,
        "the recorded peer turn was delivered, not a new one"
    );
    assert!(turns.iter().all(|turn| turn.state == TurnState::Completed));
    let root = harness.store.get_task(root_task_id).unwrap();
    assert_eq!(root.task.state, TaskState::Completed);
    assert!(
        root.evidence.iter().any(|evidence| {
            evidence.turn_id == Some(turns[1].id) && evidence.kind == TaskEvidenceKind::Progress
        }),
        "the peer turn left the task running for the result that followed"
    );
    assert!(root.evidence.iter().any(|evidence| {
        evidence.turn_id == Some(turns[2].id) && evidence.kind == TaskEvidenceKind::Completed
    }));
    assert_eq!(
        employee_messages(&harness.store)[0].state,
        MessageDeliveryState::Acknowledged
    );
    assert_eq!(
        lead_messages(&harness.store, MessageKind::Result)[0].state,
        MessageDeliveryState::Acknowledged
    );
}

#[test]
fn peer_messaging_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/peer-messaging.ps1");
    let script = std::fs::read_to_string(&script_path).expect("peer messaging acceptance script");

    for required in [
        "peer_message_text_names_the_sender_tasks_and_reply_tool",
        "peer_reply_text_threads_under_the_question",
        "peer_message_content_is_refused_when_empty_or_oversized",
        "peer_messages_link_both_tasks_and_queue_once",
        "replies_thread_under_the_question_and_return_to_the_sender_task",
        "peer_messages_are_consumed_in_turn_with_evidence_and_close_an_orphan_notice_turn",
        "peer_turns_never_complete_the_recipient_task",
        "peer_questions_reach_an_idle_lead_as_a_turn_and_the_reply_returns_to_the_waiting_worker",
        "task_wait_returns_early_with_a_peer_question_so_the_lead_answers_in_turn",
        "peer_messages_never_activate_a_dormant_recipient_and_refusals_never_end_the_turn",
        "store_enables_wal_foreign_keys_and_schema_version",
        "dormantRecipientNeverActivated",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 9",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in [
        "provider', 'smoke",
        "apps\\lazoo-desktop\\src",
        "LivePrompt",
    ] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}
