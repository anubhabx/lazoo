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

#[tokio::test]
async fn provider_completed_and_failed_terminal_events_finish_governed_turns() {
    for (status, expected) in [
        ("completed", TurnState::Completed),
        ("failed", TurnState::TerminalFailed),
    ] {
        let (_temporary, harness) = approval_harness(
            AuthorityMode::Ask,
            Duration::from_secs(1),
            Vec::new(),
            status,
        );
        warm_lead(&harness).await;
        let response = submit_governed_turn(&harness, "ask").await.unwrap();
        assert!(response.error.is_none(), "{response:?}");
        let result = response.result.unwrap();
        assert_eq!(
            result["turn"]["state"],
            serde_json::to_value(expected).unwrap()
        );
    }
}

#[tokio::test]
async fn dropped_provider_approval_observer_closes_pending_record() {
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
    let approval_id: uuid::Uuid = pending["id"].as_str().unwrap().parse().unwrap();
    submission.abort();
    let _ = submission.await;
    let closed = timeout(Duration::from_secs(2), async {
        loop {
            let approval = harness.store.get_approval(approval_id).unwrap();
            if approval.state != lazoo_protocol::ApprovalState::Pending {
                return approval;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(closed.state, lazoo_protocol::ApprovalState::Cancelled);
    assert_eq!(
        closed.failure_code.as_deref(),
        Some("LZ-APPROVAL-OBSERVER-DROPPED")
    );
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
    /// While set, every turn stalls: the provider accepts the input and then shows no progress
    /// inside its deadline, as the adapters report it.
    turn_stall: Arc<AtomicBool>,
    /// Scripted start refusals: each one fails the next `start` before any session is counted.
    fail_starts: Arc<AtomicUsize>,
    working_directories: Arc<Mutex<Vec<PathBuf>>>,
    permission_requests: Arc<Mutex<Vec<PermissionRequest>>>,
    permission_decisions: Arc<Mutex<Vec<PermissionDecision>>>,
    /// Scripted Lazoo tool calls; the first turn that runs drains and issues them after its
    /// first scripted event, mirroring a model calling tools once the native turn is accepted.
    tool_calls: Arc<Mutex<Vec<ToolCall>>>,
    tool_results: Arc<Mutex<Vec<ToolResult>>>,
    session_tools: Arc<Mutex<Vec<Vec<String>>>>,
    /// The native session id each start asked to resume; starts that begin fresh add nothing.
    resume_ids: Arc<Mutex<Vec<String>>>,
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
            turn_stall: Arc::new(AtomicBool::new(false)),
            fail_starts: Arc::new(AtomicUsize::new(0)),
            working_directories: Arc::new(Mutex::new(Vec::new())),
            permission_requests: Arc::new(Mutex::new(Vec::new())),
            permission_decisions: Arc::new(Mutex::new(Vec::new())),
            tool_calls: Arc::new(Mutex::new(Vec::new())),
            tool_results: Arc::new(Mutex::new(Vec::new())),
            session_tools: Arc::new(Mutex::new(Vec::new())),
            resume_ids: Arc::new(Mutex::new(Vec::new())),
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
    turn_stall: Arc<AtomicBool>,
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

    fn resources(&self) -> Option<lazoo_runtime::JobAccounting> {
        Some(lazoo_runtime::JobAccounting {
            active_processes: 2,
            total_processes: 3,
            user_cpu_millis: 1500,
            kernel_cpu_millis: 250,
            peak_process_memory_bytes: 50 * 1024 * 1024,
            peak_job_memory_bytes: 80 * 1024 * 1024,
        })
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
            if self.turn_stall.load(Ordering::SeqCst) {
                return Err(ProviderError::Stalled { millis: 250 });
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
            if self.fail_starts.load(Ordering::SeqCst) > 0 {
                self.fail_starts.fetch_sub(1, Ordering::SeqCst);
                return Err(ProviderError::Protocol("scripted start failure".to_owned()));
            }
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
            if let Some(resumed) = &request.session_id {
                self.resume_ids.lock().unwrap().push(resumed.clone());
            }
            Ok(Box::new(FakeManagedSession {
                // A resumed native session keeps its id, as Claude Code's `--resume` does.
                id: request
                    .session_id
                    .clone()
                    .unwrap_or_else(|| format!("native-{number}")),
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
                turn_stall: Arc::clone(&self.turn_stall),
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
            activation_policy: lazoo_protocol::ActivationPolicy::Manual,
            idle_timeout_seconds: 0,
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
        activation_policy: lazoo_protocol::ActivationPolicy::Manual,
        idle_timeout_seconds: 0,
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
        DiscoveryEnvironment::empty().with_known_location(ProviderKind::Codex, CMD_EXE),
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

    // The owner takes over the suspended worker session: its terminal opens in the retained task
    // workspace, not in the generic session directory.
    let worker_session_id = dispatch.session_id.unwrap();
    wait_until(|| {
        store.get_session(worker_session_id).unwrap().state
            == lazoo_protocol::SessionLifecycleState::Suspended
    })
    .await;
    let worker_session = store.get_session(worker_session_id).unwrap();
    let attached = app
        .handle(request(
            "dispatch-attach",
            "session.terminal.attach",
            json!({
                "id": worker_session_id,
                "expectedRevision": worker_session.revision,
                "columns": 80,
                "rows": 25,
                "idempotencyKey": "dispatch-attach"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(
        attached["terminal"]["cwd"],
        dispatch.workspace_path.unwrap()
    );
    assert_eq!(
        attached["terminal"]["args"],
        json!(["resume", worker_session.native_session_id.unwrap()])
    );
    app.handle(request(
        "dispatch-attach-stop",
        "terminal.stop",
        json!({
            "id": attached["terminal"]["id"],
            "expectedRevision": attached["terminal"]["revision"],
            "expectedBootId": attached["terminal"]["bootId"]
        }),
    ))
    .await
    .result
    .unwrap();
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
        activation_policy: lazoo_protocol::ActivationPolicy::Manual,
        idle_timeout_seconds: 0,
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
            activation_policy: None,
            idle_timeout_seconds: None,
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
        activation_policy: lazoo_protocol::ActivationPolicy::Manual,
        idle_timeout_seconds: 0,
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

/// Warms the lead's session and runs one scripted turn to completion, returning its id after
/// checking that no canonical event payload carries the provider's raw payload.
async fn complete_scripted_turn(
    app: &App,
    backend: &FakeSessionBackend,
    employee_id: uuid::Uuid,
    conversation_id: uuid::Uuid,
    events: Vec<ProviderEvent>,
) -> uuid::Uuid {
    backend.turn_events.lock().unwrap().extend(events);
    app.handle(request(
        "warm-raw-frames",
        "session.warm",
        json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-raw-frames"
        }),
    ))
    .await
    .result
    .unwrap();
    let completed = app
        .handle(request(
            "submit-raw-frames",
            "turn.submit",
            json!({
                "conversationId": conversation_id,
                "author": {"kind": "user"},
                "recipient": null,
                "text": "Reply with done.",
                "activateIfNeeded": false,
                "maxAttempts": 3,
                "idempotencyKey": "submit-raw-frames"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(completed["turn"]["state"], "completed");
    assert!(
        completed["turn"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["payload"].get("raw").is_none())
    );
    completed["turn"]["id"].as_str().unwrap().parse().unwrap()
}

fn raw_frame_lines(store: &Store, root: &DataRoot, turn_id: uuid::Uuid) -> Vec<serde_json::Value> {
    let bytes = store
        .read_memory_raw_frames(&root.root().join("memory-archives"), turn_id)
        .unwrap();
    std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn set_global_memory_retention(app: &App, retention_enabled: bool, max_source_bytes: u64) {
    app.handle(request(
        "policy-raw-frames",
        "memory.policy.set",
        json!({
            "global": true,
            "expectedRevision": 1,
            "retentionEnabled": retention_enabled,
            "retentionDays": 30,
            "quotaBytes": 268_435_456,
            "maxSourceBytes": max_source_bytes,
            "allowedTypes": ["fact"],
            "injectEnabled": true
        }),
    ))
    .await
    .result
    .unwrap();
}

fn raw_turn_started(raw: serde_json::Value) -> ProviderEvent {
    ProviderEvent::TurnStarted {
        provider: ProviderKind::Codex,
        session_id: "native-1".to_owned(),
        turn_id: Some("native-turn-1".to_owned()),
        raw: Some(raw),
    }
}

fn raw_turn_completed(raw: serde_json::Value) -> ProviderEvent {
    ProviderEvent::TurnCompleted {
        provider: ProviderKind::Codex,
        session_id: "native-1".to_owned(),
        turn_id: Some("native-turn-1".to_owned()),
        status: "completed".to_owned(),
        output: Some("done".to_owned()),
        raw: Some(raw),
    }
}

#[tokio::test]
async fn completed_turn_archives_redacted_raw_provider_frames() {
    let (_temporary, root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    let text_delta = ProviderEvent::TextDelta {
        provider: ProviderKind::Codex,
        session_id: "native-1".to_owned(),
        turn_id: Some("native-turn-1".to_owned()),
        text: "hello".to_owned(),
        raw: Some(json!({"delta": "hello"})),
    };
    let turn_id = complete_scripted_turn(
        &app,
        &backend,
        employee_id,
        conversation_id,
        vec![
            raw_turn_started(json!({"private": "keep", "token": "secret"})),
            text_delta.clone(),
            text_delta,
            raw_turn_completed(json!({"done": true})),
        ],
    )
    .await;

    let lines = raw_frame_lines(&store, &root, turn_id);
    assert_eq!(lines[0]["type"], "lazoo-raw-frames");
    assert_eq!(lines[0]["sourceTurnId"], turn_id.to_string());
    // One native message cloned into two events is kept once, and nothing was dropped.
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[1]["type"], "turnStarted");
    assert_eq!(lines[1]["raw"]["private"], "keep");
    assert_eq!(lines[1]["raw"]["token"], "[redacted]");
    assert_eq!(lines[2]["type"], "textDelta");
    assert_eq!(lines[2]["raw"]["delta"], "hello");
    assert_eq!(lines[3]["type"], "turnCompleted");
    assert_eq!(lines[3]["sequence"], 3);
    assert!(!serde_json::to_string(&lines).unwrap().contains("secret"));
    let archive_files = std::fs::read_dir(root.root().join("memory-archives"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(archive_files.len(), 1);
    assert!(archive_files[0].contains("-raw-"));
    // The raw kind is recorded as its own kind and leaves the turn a canonical candidate.
    assert!(
        store
            .list_memory_archive_candidates(10)
            .unwrap()
            .iter()
            .any(|candidate| candidate.source_turn_id == turn_id)
    );
    let archived = store
        .list_events(0, 100)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "memory.archived")
        .collect::<Vec<_>>();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].payload["kind"], "raw-frames");
    assert_eq!(archived[0].payload["archive"]["truncated"], false);
}

#[tokio::test]
async fn raw_frame_capture_is_skipped_when_retention_is_disabled() {
    let (_temporary, root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    set_global_memory_retention(&app, false, 4_194_304).await;
    let turn_id = complete_scripted_turn(
        &app,
        &backend,
        employee_id,
        conversation_id,
        vec![
            raw_turn_started(json!({"private": "keep"})),
            raw_turn_completed(json!({"done": true})),
        ],
    )
    .await;

    // Recovery at start creates the archive root; retention off leaves nothing inside it.
    let archive_root = root.root().join("memory-archives");
    assert!(!archive_root.exists() || std::fs::read_dir(&archive_root).unwrap().next().is_none());
    assert!(matches!(
        store.read_memory_raw_frames(&archive_root, turn_id),
        Err(lazoo_store::StoreError::InvalidRecord {
            field: "memory source",
            ..
        })
    ));
    assert!(
        store
            .list_events(0, 100)
            .unwrap()
            .iter()
            .all(|event| event.event_type != "memory.archived")
    );
}

#[tokio::test]
async fn raw_frame_capture_is_bounded_by_the_policy_cap() {
    let (_temporary, root, store, app, backend, employee_id, conversation_id) = turn_test_app();
    set_global_memory_retention(&app, true, 4096).await;
    let turn_id = complete_scripted_turn(
        &app,
        &backend,
        employee_id,
        conversation_id,
        vec![
            raw_turn_started(json!({"private": "keep"})),
            ProviderEvent::TextDelta {
                provider: ProviderKind::Codex,
                session_id: "native-1".to_owned(),
                turn_id: Some("native-turn-1".to_owned()),
                text: "x".repeat(16 * 1024),
                raw: Some(json!({"text": "y".repeat(16 * 1024)})),
            },
            raw_turn_completed(json!({"done": true})),
        ],
    )
    .await;

    let lines = raw_frame_lines(&store, &root, turn_id);
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[1]["type"], "turnStarted");
    assert_eq!(lines[1]["sequence"], 1);
    // The oversized frame was dropped at the cap; the sequence gap shows where.
    assert_eq!(lines[2]["type"], "turnCompleted");
    assert_eq!(lines[2]["sequence"], 3);
    assert_eq!(lines[3]["type"], "lazoo-raw-frames-truncated");
    assert_eq!(lines[3]["droppedFrames"], 1);
    assert_eq!(lines[3]["limitBytes"], 4096);
    let bytes = store
        .read_memory_raw_frames(&root.root().join("memory-archives"), turn_id)
        .unwrap();
    assert!(bytes.len() <= 4096);
    let archived = store
        .list_events(0, 100)
        .unwrap()
        .into_iter()
        .find(|event| event.event_type == "memory.archived")
        .unwrap();
    assert_eq!(archived.payload["kind"], "raw-frames");
    assert_eq!(archived.payload["archive"]["truncated"], true);
    assert_eq!(
        archived.payload["archive"]["uncompressedBytes"],
        bytes.len()
    );
}

#[test]
fn raw_frame_archive_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/raw-frame-archive.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("raw frame archive acceptance script");

    for required in [
        "raw_frame_archive_is_hashed_capped_and_counted_against_quota",
        "raw_frame_archive_respects_retention_opt_out",
        "raw_frames_and_canonical_archive_coexist_and_cleanup_removes_both",
        "version_eighteen_archives_upgrade_as_canonical_turn_archives",
        "completed_turn_archives_redacted_raw_provider_frames",
        "raw_frame_capture_is_skipped_when_retention_is_disabled",
        "raw_frame_capture_is_bounded_by_the_policy_cap",
        "store_enables_wal_foreign_keys_and_schema_version",
        "rawFramesArchived",
        "redacted",
        "cappedWithTrailer",
        "quotaShared",
        "cleanupCoversBothKinds",
        "optOutWritesNothing",
        "schemaNineteen",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
        "wireFramesWithoutEvents = 'NOT_COVERED'",
        "nonCompletedTurns = 'NOT_COVERED'",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
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
            activation_policy: lazoo_protocol::ActivationPolicy::Manual,
            idle_timeout_seconds: 0,
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
            activation_policy: lazoo_protocol::ActivationPolicy::Manual,
            idle_timeout_seconds: 0,
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
    assert_eq!(result["schemaVersion"], 19);
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

const CMD_EXE: &str = r"C:\Windows\System32\cmd.exe";

/// The session harness with cmd.exe standing in for the Codex executable: known locations are
/// not name-filtered, and cmd.exe ignores `resume <id>` and stays interactive on stdin.
fn attach_test_app() -> (
    tempfile::TempDir,
    Store,
    App,
    FakeSessionBackend,
    uuid::Uuid,
) {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = configured_employee(&store);
    let backend = FakeSessionBackend::default();
    let app = App::new_with_session_backend(
        store.clone(),
        root,
        DiscoveryEnvironment::empty().with_known_location(ProviderKind::Codex, CMD_EXE),
        Arc::new(backend.clone()),
    )
    .unwrap();
    (temporary, store, app, backend, employee.id)
}

/// Warms one session for `employee_id` and stops it, returning the suspended session record.
async fn suspended_session(app: &App, employee_id: uuid::Uuid, key: &str) -> serde_json::Value {
    let warmed = app
        .handle(request(
            key,
            "session.warm",
            json!({
                "employeeId": employee_id,
                "expectedEmployeeRevision": 1,
                "idempotencyKey": format!("{key}-warm")
            }),
        ))
        .await
        .result
        .unwrap();
    app.handle(request(
        key,
        "session.stop",
        json!({
            "id": warmed["session"]["id"],
            "expectedRevision": warmed["session"]["revision"],
            "idempotencyKey": format!("{key}-stop")
        }),
    ))
    .await
    .result
    .unwrap()["session"]
        .clone()
}

fn attach_params(session: &serde_json::Value, key: &str) -> serde_json::Value {
    json!({
        "id": session["id"],
        "expectedRevision": session["revision"],
        "columns": 80,
        "rows": 25,
        "idempotencyKey": key
    })
}

fn stop_terminal_params(terminal: &serde_json::Value) -> serde_json::Value {
    json!({
        "id": terminal["id"],
        "expectedRevision": terminal["revision"],
        "expectedBootId": terminal["bootId"]
    })
}

/// The owner takes over a finished Codex session: the terminal runs the discovered executable
/// with the recorded native id in the session's own directory, a replay returns the same
/// terminal, a second attach is refused while it lives, a stop frees the slot, and each attach
/// is audited while the session row itself stays suspended at its revision.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One owner session: attach, replay, refuse, stop, re-attach, audit.
async fn session_terminal_attach_resumes_a_suspended_codex_session_in_an_owner_terminal() {
    let (_temporary, _store, app, backend, employee_id) = attach_test_app();
    let session = suspended_session(&app, employee_id, "attach").await;
    let session_id: uuid::Uuid = session["id"].as_str().unwrap().parse().unwrap();
    let attach = |id: &str, params: serde_json::Value| {
        app.handle(request(id, "session.terminal.attach", params))
    };

    let attached = attach("attach-1", attach_params(&session, "attach-1"))
        .await
        .result
        .unwrap();
    assert_eq!(attached["sessionId"], session["id"]);
    let terminal = attached["terminal"].clone();
    assert_eq!(terminal["executable"], CMD_EXE);
    assert_eq!(terminal["args"], json!(["resume", "native-1"]));
    assert_eq!(
        terminal["cwd"],
        app.data_root()
            .session_working_dir(session_id)
            .to_string_lossy()
            .into_owned()
    );
    assert_eq!(terminal["live"], true);
    assert_eq!(terminal["revision"], 1);

    let replayed = attach("attach-1-replay", attach_params(&session, "attach-1"))
        .await
        .result
        .unwrap();
    assert_eq!(replayed["terminal"]["id"], terminal["id"]);

    let refused = attach("attach-2", attach_params(&session, "attach-2"))
        .await
        .error
        .unwrap();
    assert_eq!(refused.code, "LZ-SESSION-ATTACHED");
    assert_eq!(
        refused.data.unwrap(),
        json!({"sessionId": session["id"], "terminalId": terminal["id"]})
    );

    let stopped = app
        .handle(request(
            "terminal-stop",
            "terminal.stop",
            stop_terminal_params(&terminal),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(stopped["live"], false);

    let reattached = attach("attach-3", attach_params(&session, "attach-3"))
        .await
        .result
        .unwrap();
    assert_ne!(reattached["terminal"]["id"], terminal["id"]);
    assert_eq!(reattached["terminal"]["live"], true);
    assert_eq!(reattached["terminal"]["args"], terminal["args"]);

    let shown = app
        .handle(request("show", "session.show", json!({"id": session_id})))
        .await
        .result
        .unwrap();
    assert_eq!(
        shown, session,
        "the attach changes nothing on the session row"
    );
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    assert_eq!(backend.stops.load(Ordering::SeqCst), 1);

    let events = app
        .handle(request(
            "events",
            "events.list",
            json!({"after": 0, "limit": 1024}),
        ))
        .await
        .result
        .unwrap();
    let audited = events
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["eventType"] == "session.terminal.attached")
        .map(|event| event["payload"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        audited,
        vec![
            json!({"sessionId": session["id"], "terminalId": terminal["id"]}),
            json!({"sessionId": session["id"], "terminalId": reattached["terminal"]["id"]}),
        ]
    );

    app.handle(request(
        "terminal-stop-2",
        "terminal.stop",
        stop_terminal_params(&reattached["terminal"]),
    ))
    .await
    .result
    .unwrap();
}

/// Only a suspended session may be taken over: an idle session has a live protocol host, a
/// stale revision, an unknown id, and malformed requests are refused before anything spawns.
#[tokio::test]
async fn session_terminal_attach_refuses_idle_sessions_stale_revisions_and_unknown_ids() {
    let (_temporary, _store, app, backend, employee_id) = attach_test_app();
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
        .unwrap()["session"]
        .clone();
    let attach = |id: &str, params: serde_json::Value| {
        app.handle(request(id, "session.terminal.attach", params))
    };

    let idle = attach("idle", attach_params(&warmed, "idle"))
        .await
        .error
        .unwrap();
    assert_eq!(idle.code, "LZ-SESSION-STATE");
    assert_eq!(idle.data.unwrap(), json!({"sessionId": warmed["id"]}));
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    assert_eq!(backend.stops.load(Ordering::SeqCst), 0);

    let suspended = app
        .handle(request(
            "stop",
            "session.stop",
            json!({
                "id": warmed["id"],
                "expectedRevision": warmed["revision"],
                "idempotencyKey": "stop-rhea"
            }),
        ))
        .await
        .result
        .unwrap()["session"]
        .clone();
    let stale = attach("stale", attach_params(&warmed, "stale"))
        .await
        .error
        .unwrap();
    assert_eq!(stale.code, "LZ-SESSION-STALE");
    assert_eq!(
        stale.data.unwrap(),
        json!({"currentRevision": suspended["revision"]})
    );

    let unknown = attach(
        "unknown",
        attach_params(
            &json!({"id": uuid::Uuid::new_v4(), "revision": 1}),
            "unknown",
        ),
    )
    .await
    .error
    .unwrap();
    assert_eq!(unknown.code, "LZ-SESSION-NOT-FOUND");

    let mut zero = attach_params(&suspended, "zero");
    zero["expectedRevision"] = json!(0);
    assert_eq!(
        attach("zero", zero).await.error.unwrap().code,
        "LZ-SESSION-INVALID"
    );
    assert_eq!(
        attach("blank", attach_params(&suspended, ""))
            .await
            .error
            .unwrap()
            .code,
        "LZ-SESSION-INVALID"
    );
    let mut unexpected = attach_params(&suspended, "unexpected");
    unexpected["authority"] = json!("full-autonomy");
    assert_eq!(
        attach("unexpected", unexpected).await.error.unwrap().code,
        "LZ-INVALID-PARAMS"
    );

    let terminals = app
        .handle(request("terminals", "terminal.list", json!({})))
        .await
        .result
        .unwrap();
    assert_eq!(terminals["terminals"], json!([]));
}

/// Without a discovered executable there is nothing to run, and providers without a documented
/// resume command are refused by name rather than guessed at.
#[tokio::test]
async fn session_terminal_attach_fails_closed_without_an_executable_or_a_resume_command() {
    let (_temporary, app, _backend, employee_id) = session_test_app();
    let session = suspended_session(&app, employee_id, "undetected").await;
    let undetected = app
        .handle(request(
            "undetected",
            "session.terminal.attach",
            attach_params(&session, "undetected"),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(undetected.code, "LZ-PROVIDER-NOT-DETECTED");

    let (_temporary, store, app, _backend, _codex) = attach_test_app();
    for (name, provider) in [("Noor", "opencode"), ("Ivo", "antigravity")] {
        let employee = store
            .create_employee(&CreateEmployeeInput {
                request_id: format!("{name}-request"),
                idempotency_key: format!("create-{name}"),
                name: name.to_owned(),
                provider: provider.to_owned(),
                role: "Worker".to_owned(),
                instructions: String::new(),
                model: Some("test-model".to_owned()),
                reasoning_effort: Some("low".to_owned()),
                native_settings: json!({}),
                authority_mode: AuthorityMode::Ask,
                checkout_policy: EmployeeCheckoutPolicy::Isolated,
                activation_policy: lazoo_protocol::ActivationPolicy::Manual,
                idle_timeout_seconds: 0,
                enabled: true,
                provider_observation: Some(ProviderObservation {
                    provider: provider.to_owned(),
                    verification: "configured".to_owned(),
                    version: Some("test".to_owned()),
                    protocol: Some("test".to_owned()),
                    capabilities: json!({"session": true}),
                    authentication: json!({"native": "ready"}),
                    degradation_reason: None,
                    checked_at: "2026-10-17T00:00:00Z".to_owned(),
                }),
            })
            .unwrap()
            .employee;
        let session = suspended_session(&app, employee.id, provider).await;
        let unsupported = app
            .handle(request(
                provider,
                "session.terminal.attach",
                attach_params(&session, provider),
            ))
            .await
            .error
            .unwrap();
        assert_eq!(
            unsupported.code, "LZ-SESSION-ATTACH-UNSUPPORTED",
            "{provider}"
        );
        assert_eq!(
            unsupported.data.unwrap(),
            json!({"sessionId": session["id"]})
        );
    }
    let terminals = app
        .handle(request("terminals", "terminal.list", json!({})))
        .await
        .result
        .unwrap();
    assert_eq!(terminals["terminals"], json!([]));
}

/// An attached suspended session does not block the employee: a new warm session starts as
/// usual in a different directory, and the owner's terminal stays live beside it.
#[tokio::test]
async fn session_terminal_attach_leaves_the_employee_free_for_new_sessions() {
    let (_temporary, _store, app, backend, employee_id) = attach_test_app();
    let session = suspended_session(&app, employee_id, "first").await;
    let attached = app
        .handle(request(
            "attach",
            "session.terminal.attach",
            attach_params(&session, "attach"),
        ))
        .await
        .result
        .unwrap();

    let warmed = app
        .handle(request(
            "warm-again",
            "session.warm",
            json!({
                "employeeId": employee_id,
                "expectedEmployeeRevision": 1,
                "idempotencyKey": "warm-again"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(warmed["session"]["state"], "idle");
    assert_ne!(warmed["session"]["id"], session["id"]);
    assert_eq!(warmed["session"]["nativeSessionId"], "native-2");
    assert_eq!(backend.starts.load(Ordering::SeqCst), 2);

    let terminal = app
        .handle(request(
            "terminal",
            "terminal.show",
            json!({"id": attached["terminal"]["id"]}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(terminal["live"], true);
    app.handle(request(
        "terminal-stop",
        "terminal.stop",
        stop_terminal_params(&attached["terminal"]),
    ))
    .await
    .result
    .unwrap();
}

#[test]
fn session_terminal_attach_acceptance_contract_is_real_binary_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/session-terminal-attach.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("session terminal attach acceptance script");

    for required in [
        "[switch]$SkipBuild",
        "lazoo-turn-transport-fixture.exe",
        "'codex.exe'",
        "LAZOO_TURN_FIXTURE_THREAD_ID",
        "'session', 'warm'",
        "'session', 'attach'",
        "'session', 'stop'",
        "'terminal', 'write'",
        "'terminal', 'read'",
        "'terminal', 'stop'",
        "LZ-SESSION-STATE",
        "LZ-SESSION-ATTACHED",
        "LZ-SESSION-NOT-FOUND",
        "session.terminal.attached",
        "idleAttachRefused",
        "resumeArgumentsFromRecords",
        "cwdFromRecords",
        "oneLiveTerminalPerSession",
        "terminalBytesRoundTrip",
        "reattachAfterStop",
        "sessionRecordUnchanged",
        "auditEventRecorded",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "real-binaries-disposable-data-root-no-provider",
        "schemaVersion = 19",
        "session-terminal-attach-manifest.json",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt", "claude.exe"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
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
        "schemaVersion 19",
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
        "schemaVersion 19",
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
        "schemaVersion 19",
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
    assert_eq!(manifest["schemaVersion"], 19);
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
            activation_policy: lazoo_protocol::ActivationPolicy::Manual,
            idle_timeout_seconds: 0,
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
    // A process crash cannot run the observer's Drop guard. Reconcile while it is still
    // pending, then drop the old future; a deliberate abort is tested separately as Cancelled.
    let restarted = App::new_with_provider_environment(
        harness.store.clone(),
        harness.app.data_root().clone(),
        DiscoveryEnvironment::empty(),
    )
    .unwrap();
    submission.abort();
    let _ = submission.await;
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
    tool_harness_with(authority, lazoo_protocol::ActivationPolicy::Manual)
}

/// The tool harness with the lead and worker under one activation policy; `manual` is the
/// default every other scenario relies on.
fn tool_harness_with(
    authority: AuthorityMode,
    activation: lazoo_protocol::ActivationPolicy,
) -> (tempfile::TempDir, ToolHarness) {
    tool_harness_idle(authority, activation, 0)
}

/// The tool harness with the lead and worker under one activation policy and one idle
/// timeout; 0 (never suspend) is the default every other scenario relies on.
fn tool_harness_idle(
    authority: AuthorityMode,
    activation: lazoo_protocol::ActivationPolicy,
    idle_timeout_seconds: u32,
) -> (tempfile::TempDir, ToolHarness) {
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
        activation_policy: activation,
        idle_timeout_seconds,
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
            "memory_search",
            "employees_list",
            "task_delegate",
            "task_wait",
            "task_report",
            "incident_report",
            "incident_list",
            "incident_remedy",
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
        "schemaVersion = 19",
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
            activation_policy: None,
            idle_timeout_seconds: None,
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
    // One daemon crash crosses its threshold on its own and is the user's at once.
    assert_eq!(keeper["priority"], "priority");
    assert_eq!(keeper["escalation"], "user");
    assert_eq!(keeper["escalationReason"], "no-lead");

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
    assert_eq!(status["schemaVersion"], 19);
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
        "schemaVersion = 19",
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
    let (root_task_id, children) = delegate_children(harness, &["Implement the parser fix"]).await;
    (root_task_id, children[0])
}

/// Lead turn that delegates one child per objective to the worker and ends without waiting;
/// returns the root task id and the child ids in objective order.
async fn delegate_children(
    harness: &ToolHarness,
    objectives: &[&str],
) -> (uuid::Uuid, Vec<uuid::Uuid>) {
    use lazoo_protocol::TaskState;

    warm_tool_lead(harness).await;
    let roster_revision = harness.store.roster_snapshot().unwrap().revision;
    harness
        .backend
        .tool_calls
        .lock()
        .unwrap()
        .extend(objectives.iter().enumerate().map(|(index, objective)| {
            tool_call(
                &format!("call-{}", index + 1),
                "task_delegate",
                json!({
                    "employeeId": harness.worker.id,
                    "objective": objective,
                    "rosterRevision": roster_revision,
                    "requestedAuthority": "self-decision"
                }),
            )
        }));
    let submitted = submit_lead_turn(harness, "self-decision").await;
    assert_eq!(submitted["turn"]["state"], "completed");
    let root_task_id: uuid::Uuid = submitted["turn"]["taskId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let results = harness.backend.tool_results.lock().unwrap().clone();
    let children = results[..objectives.len()]
        .iter()
        .map(|result| {
            assert!(result.success, "{result:?}");
            result.content["taskId"].as_str().unwrap().parse().unwrap()
        })
        .collect();
    let root = harness.store.get_task(root_task_id).unwrap().task;
    assert_eq!(
        root.state,
        TaskState::Running,
        "the lead's task stays running while its children are unfinished"
    );
    assert!(root.active_turn_id.is_none());
    (root_task_id, children)
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
        "schemaVersion = 19",
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
    acknowledgement_evidence(store, message_id)["via"].clone()
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

    // The user resumes the lead. The waiting peer message and the child's result reach it as
    // one coalesced turn; inside it the lead tries four calls that must all be refused with a
    // stable code and must not end the turn, and the result it carried completes the task.
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

    assert_eq!(
        harness.backend.turns.load(Ordering::SeqCst),
        3,
        "the peer message and the result share one turn"
    );
    let prompts = prompts(&harness.backend);
    assert!(
        prompts[2].starts_with("[Lazoo notices] 2 Lazoo notices are waiting"),
        "{}",
        prompts[2]
    );
    assert!(prompts[2].contains("[Lazoo peer message]"));
    assert!(prompts[2].contains("Heads up: the parser fix is ready for review."));
    assert!(prompts[2].contains("[Lazoo task result]"));
    assert!(prompts[2].contains(&child_id.to_string()));
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
        2,
        "the recorded coalesced turn was delivered, not a new one"
    );
    assert!(turns.iter().all(|turn| turn.state == TurnState::Completed));
    let root = harness.store.get_task(root_task_id).unwrap();
    assert_eq!(root.task.state, TaskState::Completed);
    assert!(
        root.evidence.iter().any(|evidence| {
            evidence.turn_id == Some(turns[1].id) && evidence.kind == TaskEvidenceKind::Completed
        }),
        "a turn carrying the child's result completes the task"
    );
    let peer = employee_messages(&harness.store)[0].clone();
    assert_eq!(peer.state, MessageDeliveryState::Acknowledged);
    assert_eq!(
        harness.store.get_turn(turns[1].id).unwrap().message_id,
        peer.id,
        "the older notice is canonical"
    );
    let result = lead_messages(&harness.store, MessageKind::Result)[0].clone();
    assert_eq!(result.state, MessageDeliveryState::Acknowledged);
    assert_eq!(
        acknowledgement_evidence(&harness.store, result.id)["turnId"],
        json!(turns[1].id),
        "the companion result cites the turn that carried it"
    );
}

fn acknowledgement_evidence(store: &Store, message_id: uuid::Uuid) -> serde_json::Value {
    store
        .get_message(message_id)
        .unwrap()
        .attempts
        .last()
        .and_then(|attempt| attempt.acknowledgement_evidence.clone())
        .unwrap_or_default()
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
        "schemaVersion = 19",
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
// Coalesced notice turns: every notice queued for one recipient and task rides one turn.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn queued_notices_reach_an_idle_lead_as_one_turn() {
    use lazoo_protocol::{MessageKind, TaskState};

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    let (root_task_id, children) = delegate_children(
        &harness,
        &["Implement the parser fix", "Write the parser tests"],
    )
    .await;
    // Both children run on the one worker; a second pass covers a dispatch left waiting.
    harness.app.dispatch_pending_tasks_once().await.unwrap();
    harness.app.dispatch_pending_tasks_once().await.unwrap();
    for child in &children {
        assert_eq!(
            harness.store.get_task(*child).unwrap().task.state,
            TaskState::Completed
        );
    }
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 3);
    let starts = harness.backend.starts.load(Ordering::SeqCst);

    harness.app.deliver_pending_notices_once().await.unwrap();

    assert_eq!(
        harness.backend.turns.load(Ordering::SeqCst),
        4,
        "two child results reach the lead as one turn"
    );
    assert_eq!(
        harness.backend.starts.load(Ordering::SeqCst),
        starts,
        "no provider process was started for the notices"
    );
    let prompt = last_prompt(&harness.backend);
    assert!(
        prompt.starts_with("[Lazoo notices] 2 Lazoo notices are waiting"),
        "{prompt}"
    );
    assert_eq!(prompt.matches("[Lazoo task result]").count(), 2);
    assert!(
        prompt.find(&children[0].to_string()) < prompt.find(&children[1].to_string()),
        "notices are carried oldest first"
    );

    let results = lead_messages(&harness.store, MessageKind::Result);
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .all(|result| result.state == MessageDeliveryState::Acknowledged)
    );
    let turns = lead_turns(&harness);
    assert_eq!(
        turns.len(),
        2,
        "the user turn and one coalesced notice turn"
    );
    assert_eq!(turns[1].state, TurnState::Completed);
    assert_eq!(
        harness.store.get_turn(turns[1].id).unwrap().message_id,
        results[0].id,
        "the oldest result is the turn's canonical message"
    );
    let companion = acknowledgement_evidence(&harness.store, results[1].id);
    assert_eq!(companion["turnId"], json!(turns[1].id));
    assert_eq!(companion["canonicalMessageId"], json!(results[0].id));
    assert_eq!(
        harness.store.get_task(root_task_id).unwrap().task.state,
        TaskState::Completed
    );

    harness.app.deliver_pending_notices_once().await.unwrap();
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 4);
    assert!(harness.store.pending_lead_notices(10).unwrap().is_empty());
}

#[test]
fn coalesced_notices_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/coalesced-notices.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("coalesced notices acceptance script");

    for required in [
        "coalesced_notice_text_lists_every_notice_oldest_first_and_passes_one_through",
        "queued_notices_for_one_recipient_coalesce_into_one_turn_with_evidence_for_each",
        "a_notice_read_inside_another_turn_releases_its_batch_for_a_fresh_turn",
        "a_coalesced_turn_stays_within_the_turn_text_limit",
        "peer_turns_never_complete_the_recipient_task",
        "queued_notices_reach_an_idle_lead_as_one_turn",
        "child_results_reach_an_idle_lead_as_a_notice_turn_after_its_turn_ended",
        "peer_messages_never_activate_a_dormant_recipient_and_refusals_never_end_the_turn",
        "store_enables_wal_foreign_keys_and_schema_version",
        "eachNoticeKeepsOwnEvidence",
        "dormantLeadNeverActivated",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
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
// Activation policy: a recorded notice turn wakes its dormant recipient only by that
// recipient's own policy, once per turn, for its own task, and the session stops with the turn.

/// Delegates one child, lets it complete, and stops the lead's session so the child's result
/// finds the lead dormant with two sessions started so far; returns the root and child ids.
async fn dormant_lead_with_a_completed_child(harness: &ToolHarness) -> (uuid::Uuid, uuid::Uuid) {
    use lazoo_protocol::{SessionLifecycleState, TaskState};

    let (root_task_id, child_id) = delegate_without_waiting(harness).await;
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
    (root_task_id, child_id)
}

#[tokio::test]
async fn a_notice_turn_activates_a_dormant_lead_by_policy_and_stops_the_session_after_it() {
    use lazoo_protocol::{ActivationPolicy, MessageKind, SessionLifecycleState, TaskState};

    let (_temporary, harness) =
        tool_harness_with(AuthorityMode::SelfDecision, ActivationPolicy::NoticeTurns);
    let (root_task_id, child_id) = dormant_lead_with_a_completed_child(&harness).await;
    let stops_before = harness.backend.stops.load(Ordering::SeqCst);

    harness.app.deliver_pending_notices_once().await.unwrap();

    assert_eq!(
        harness.backend.starts.load(Ordering::SeqCst),
        3,
        "the notice turn started one session for the lead by policy"
    );
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 3);
    assert!(last_prompt(&harness.backend).contains(&child_id.to_string()));
    let delivered = lead_messages(&harness.store, MessageKind::Result);
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].state, MessageDeliveryState::Acknowledged);
    let turns = lead_turns(&harness);
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].state, TurnState::Completed);
    assert_eq!(
        harness.store.get_task(root_task_id).unwrap().task.state,
        TaskState::Completed
    );
    assert_eq!(
        harness.backend.stops.load(Ordering::SeqCst),
        stops_before + 1,
        "the activated session was stopped as soon as the turn ended"
    );
    let lead_sessions: Vec<_> = harness
        .store
        .list_sessions()
        .unwrap()
        .into_iter()
        .filter(|session| session.employee_id == harness.lead.id)
        .collect();
    assert_eq!(
        lead_sessions.len(),
        2,
        "the user's warm session and the one policy activation"
    );
    assert!(
        lead_sessions
            .iter()
            .all(|session| session.state == SessionLifecycleState::Suspended),
        "the lead is dormant again once the turn ended: {lead_sessions:?}"
    );

    // Nothing is queued any more, so nothing starts again.
    harness.app.deliver_pending_notices_once().await.unwrap();
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 3);
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_failed_activation_is_never_retried_and_the_user_still_resumes_the_lead() {
    use lazoo_protocol::{ActivationPolicy, MessageKind, SessionLifecycleState, TaskState};

    let (_temporary, harness) =
        tool_harness_with(AuthorityMode::SelfDecision, ActivationPolicy::NoticeTurns);
    let (root_task_id, child_id) = dormant_lead_with_a_completed_child(&harness).await;
    harness.backend.fail_starts.store(1, Ordering::SeqCst);

    let failed = harness
        .app
        .deliver_pending_notices_once()
        .await
        .expect_err("the refused start surfaces as the pass error");

    assert_eq!(failed.code, "LZ-SESSION-START-FAILED");
    assert_eq!(
        harness.backend.starts.load(Ordering::SeqCst),
        2,
        "a refused start is not a provider process"
    );
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 2);
    let queued = lead_messages(&harness.store, MessageKind::Result);
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].state, MessageDeliveryState::Queued);
    let turns = lead_turns(&harness);
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].state, TurnState::Recorded);
    let failed_session = harness
        .store
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|session| {
            session.employee_id == harness.lead.id && session.state == SessionLifecycleState::Failed
        })
        .expect("the failed activation is on the session ledger");
    assert_eq!(
        failed_session.failure_code.as_deref(),
        Some("LZ-SESSION-START-FAILED")
    );
    let incidents = list_incidents(&harness.app, json!({"code": "LZ-SESSION-START-FAILED"})).await;
    assert_eq!(incidents.len(), 1, "{incidents:?}");
    assert_eq!(incidents[0]["employeeId"], harness.lead.id.to_string());
    assert_eq!(incidents[0]["sessionId"], failed_session.id.to_string());

    // The same recorded turn never activates the lead a second time.
    harness.app.deliver_pending_notices_once().await.unwrap();
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 2);
    assert_eq!(lead_turns(&harness)[1].state, TurnState::Recorded);
    assert_eq!(
        lead_messages(&harness.store, MessageKind::Result)[0].state,
        MessageDeliveryState::Queued
    );

    // The local user resumes the lead exactly as under `manual`; the waiting turn is delivered
    // on that session, and a session the user started is not stopped by the notice.
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
    let stops_before = harness.backend.stops.load(Ordering::SeqCst);
    harness.app.deliver_pending_notices_once().await.unwrap();

    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 3);
    assert!(last_prompt(&harness.backend).contains(&child_id.to_string()));
    assert_eq!(
        lead_messages(&harness.store, MessageKind::Result)[0].state,
        MessageDeliveryState::Acknowledged
    );
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
    assert_eq!(harness.backend.stops.load(Ordering::SeqCst), stops_before);
}

#[test]
fn activation_policy_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/activation-policy.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("activation policy acceptance script");

    for required in [
        "activation_policies_round_trip_as_kebab_case_and_default_to_manual",
        "notice_turns_activate_a_recipient_only_under_the_notice_turns_policy",
        "activation_policy_defaults_to_manual_and_a_change_bumps_the_configuration_revision",
        "employee_cli_maps_mutations_and_roster_refresh",
        "a_notice_turn_activates_a_dormant_lead_by_policy_and_stops_the_session_after_it",
        "a_failed_activation_is_never_retried_and_the_user_still_resumes_the_lead",
        "notices_wait_for_a_dormant_lead_until_the_user_resumes_it",
        "peer_messages_never_activate_a_dormant_recipient_and_refusals_never_end_the_turn",
        "store_enables_wal_foreign_keys_and_schema_version",
        "manualPolicyNeverActivates",
        "oneActivationPerRecordedTurn",
        "sessionStoppedAfterTurn",
        "failedActivationNeverRetried",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
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
// Idle suspension: a session quiet for its employee's whole idle timeout is stopped by the
// daemon, never while a turn on it is unfinished, and never when the timeout is 0.

fn lead_sessions(
    harness: &ToolHarness,
    state: lazoo_protocol::SessionLifecycleState,
) -> Vec<lazoo_protocol::EmployeeSessionRecord> {
    harness
        .store
        .list_sessions()
        .unwrap()
        .into_iter()
        .filter(|session| session.employee_id == harness.lead.id && session.state == state)
        .collect()
}

#[tokio::test]
async fn an_idle_codex_session_suspends_after_its_employee_timeout_and_the_next_user_turn_starts_fresh_with_a_recorded_reason()
 {
    use chrono::{TimeDelta, Utc};
    use lazoo_protocol::{ActivationPolicy, EmployeeAvailability, SessionLifecycleState};

    let (_temporary, harness) =
        tool_harness_idle(AuthorityMode::SelfDecision, ActivationPolicy::Manual, 900);
    warm_tool_lead(&harness).await;
    let submitted = submit_lead_turn(&harness, "self-decision").await;
    assert_eq!(submitted["turn"]["state"], "completed");
    let session = lead_sessions(&harness, SessionLifecycleState::Idle).remove(0);

    harness
        .app
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::seconds(890))
        .await
        .unwrap();
    assert_eq!(
        harness.backend.stops.load(Ordering::SeqCst),
        0,
        "a session quiet for less than its timeout stays up"
    );

    harness
        .app
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::seconds(901))
        .await
        .unwrap();
    assert_eq!(harness.backend.stops.load(Ordering::SeqCst), 1);
    let suspended = harness.store.get_session(session.id).unwrap();
    assert_eq!(suspended.state, SessionLifecycleState::Suspended);
    assert!(suspended.failure_code.is_none());
    let roster = harness.store.roster_snapshot().unwrap();
    let lead_entry = roster
        .entries
        .iter()
        .find(|entry| entry.employee.id == harness.lead.id)
        .unwrap();
    assert_eq!(lead_entry.availability, EmployeeAvailability::Suspended);
    let events = harness.store.list_events(0, 1024).unwrap();
    for event_type in ["session.stopping", "session.suspended"] {
        assert!(
            events.iter().any(|event| event.event_type == event_type
                && event.payload["session"]["id"] == session.id.to_string()),
            "{event_type} is on the event ledger"
        );
    }

    // Nothing is left to suspend, and a dormant employee is never started by the sweep.
    harness
        .app
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::days(1))
        .await
        .unwrap();
    assert_eq!(harness.backend.stops.load(Ordering::SeqCst), 1);
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 1);

    // The next user turn activates the lead as for any dormant employee: a fresh session,
    // because Codex `thread/resume` cannot carry the Lazoo tools every Codex session declares.
    let resumed = harness
        .app
        .handle(request(
            "resume-after-idle",
            "turn.submit",
            json!({
                "conversationId": harness.conversation_id,
                "author": {"kind": "user"},
                "recipient": null,
                "authorityMode": "self-decision",
                "text": "Pick the parser fix back up",
                "activateIfNeeded": true,
                "maxAttempts": 3,
                "idempotencyKey": "resume-after-idle"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(resumed["turn"]["state"], "completed");
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 2);
    let fresh = lead_sessions(&harness, SessionLifecycleState::Idle);
    assert_eq!(fresh.len(), 1);
    assert_ne!(fresh[0].id, session.id);
    assert!(
        harness.backend.resume_ids.lock().unwrap().is_empty(),
        "no resume was requested of the provider"
    );
    assert_eq!(
        resume_events(&harness.store),
        vec![(
            "session.resume.skipped".to_owned(),
            json!({
                "sessionId": fresh[0].id,
                "priorSessionId": session.id,
                "reason": "codex-dynamic-tools"
            })
        )]
    );
}

fn resume_events(store: &Store) -> Vec<(String, serde_json::Value)> {
    store
        .list_events(0, 4096)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type.starts_with("session.resume"))
        .map(|event| (event.event_type, event.payload))
        .collect()
}

/// A Claude Code lead with a conversation, beside the harness's Codex employees. Claude Code
/// takes governed turns only under full autonomy, so that is its authority.
fn claude_lead(
    harness: &ToolHarness,
    idle_timeout_seconds: u32,
) -> (lazoo_protocol::EmployeeRecord, uuid::Uuid) {
    let lead = harness
        .store
        .create_employee(&CreateEmployeeInput {
            request_id: "resume-cleo-request".to_owned(),
            idempotency_key: "resume-cleo".to_owned(),
            name: "Cleo".to_owned(),
            provider: "claude-code".to_owned(),
            role: "Lead".to_owned(),
            instructions: String::new(),
            model: None,
            reasoning_effort: None,
            native_settings: json!({}),
            authority_mode: AuthorityMode::FullAutonomy,
            checkout_policy: EmployeeCheckoutPolicy::Isolated,
            activation_policy: lazoo_protocol::ActivationPolicy::Manual,
            idle_timeout_seconds,
            enabled: true,
            provider_observation: Some(ProviderObservation {
                provider: "claude-code".to_owned(),
                verification: "configured".to_owned(),
                version: Some("test".to_owned()),
                protocol: Some("test".to_owned()),
                capabilities: json!({"session": true}),
                authentication: json!({"native": "ready", "external": "unknown"}),
                degradation_reason: None,
                checked_at: "2026-10-17T00:00:00Z".to_owned(),
            }),
        })
        .unwrap()
        .employee;
    let conversation = harness
        .store
        .create_conversation(&CreateConversationInput {
            request_id: "resume-cleo-conversation-request".to_owned(),
            idempotency_key: "resume-cleo-conversation".to_owned(),
            project_id: None,
            lead_employee_id: lead.id,
            title: None,
        })
        .unwrap()
        .conversation;
    (lead, conversation.id)
}

async fn submit_user_turn(
    harness: &ToolHarness,
    conversation_id: uuid::Uuid,
    key: &str,
) -> serde_json::Value {
    timeout(
        Duration::from_secs(20),
        harness.app.handle(request(
            key,
            "turn.submit",
            json!({
                "conversationId": conversation_id,
                "author": {"kind": "user"},
                "recipient": null,
                "authorityMode": "full-autonomy",
                "text": "Carry on with the parser fix",
                "activateIfNeeded": true,
                "maxAttempts": 3,
                "idempotencyKey": key
            }),
        )),
    )
    .await
    .expect("the turn completes")
    .result
    .unwrap()
}

fn sessions_of(
    harness: &ToolHarness,
    employee_id: uuid::Uuid,
) -> Vec<lazoo_protocol::EmployeeSessionRecord> {
    harness
        .store
        .list_sessions()
        .unwrap()
        .into_iter()
        .filter(|session| session.employee_id == employee_id)
        .collect()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One story: suspend, resume, suspend, resume again.
async fn an_idle_claude_code_session_suspends_and_the_next_user_turn_resumes_its_native_session() {
    use chrono::{TimeDelta, Utc};
    use lazoo_protocol::{ActivationPolicy, SessionLifecycleState};

    let (_temporary, harness) =
        tool_harness_idle(AuthorityMode::FullAutonomy, ActivationPolicy::Manual, 900);
    let (lead, conversation_id) = claude_lead(&harness, 900);

    // The first activation has no predecessor: a fresh native session and no resume audit.
    let first_turn = submit_user_turn(&harness, conversation_id, "resume-first").await;
    assert_eq!(first_turn["turn"]["state"], "completed");
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 1);
    assert!(harness.backend.resume_ids.lock().unwrap().is_empty());
    assert!(resume_events(&harness.store).is_empty());
    let first = sessions_of(&harness, lead.id).remove(0);
    assert_eq!(first.native_session_id.as_deref(), Some("native-1"));

    harness
        .app
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::seconds(901))
        .await
        .unwrap();
    assert_eq!(
        harness.store.get_session(first.id).unwrap().state,
        SessionLifecycleState::Suspended
    );
    assert_eq!(harness.backend.stops.load(Ordering::SeqCst), 1);

    // The next user turn wakes the lead on the same native conversation, in the same directory.
    let second_turn = submit_user_turn(&harness, conversation_id, "resume-second").await;
    assert_eq!(second_turn["turn"]["state"], "completed");
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 2);
    assert_eq!(
        harness.backend.resume_ids.lock().unwrap().as_slice(),
        ["native-1"]
    );
    let first_directory = harness.app.data_root().session_working_dir(first.id);
    let sessions = sessions_of(&harness, lead.id);
    assert_eq!(sessions.len(), 2);
    let second = &sessions[1];
    assert_ne!(second.id, first.id);
    assert_eq!(second.state, SessionLifecycleState::Idle);
    assert_eq!(second.native_session_id, first.native_session_id);
    assert_eq!(
        harness
            .backend
            .working_directories
            .lock()
            .unwrap()
            .as_slice(),
        [first_directory.clone(), first_directory.clone()],
        "the resume runs in the suspended session's own working directory"
    );
    assert_eq!(
        resume_events(&harness.store),
        vec![(
            "session.resumed".to_owned(),
            json!({
                "sessionId": second.id,
                "priorSessionId": first.id,
                "nativeSessionId": "native-1",
                "workingDirectory": first_directory.to_string_lossy()
            })
        )]
    );

    // A resumed session suspends and resumes like any other, always the same native id.
    harness
        .app
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::seconds(1802))
        .await
        .unwrap();
    submit_user_turn(&harness, conversation_id, "resume-third").await;
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 3);
    assert_eq!(
        harness.backend.resume_ids.lock().unwrap().as_slice(),
        ["native-1", "native-1"]
    );
    assert_eq!(resume_events(&harness.store).len(), 2);
    assert_eq!(
        harness
            .backend
            .working_directories
            .lock()
            .unwrap()
            .last()
            .unwrap(),
        &first_directory,
        "a resume of a resumed session still runs where the conversation began"
    );
}

#[tokio::test]
async fn a_suspended_claude_code_session_that_never_completed_a_turn_is_not_resumed() {
    use chrono::{TimeDelta, Utc};
    use lazoo_protocol::ActivationPolicy;

    let (_temporary, harness) =
        tool_harness_idle(AuthorityMode::FullAutonomy, ActivationPolicy::Manual, 900);
    let (lead, conversation_id) = claude_lead(&harness, 900);
    harness
        .app
        .handle(request(
            "warm-cleo",
            "session.warm",
            json!({
                "employeeId": lead.id,
                "expectedEmployeeRevision": lead.configuration_revision,
                "idempotencyKey": "warm-cleo"
            }),
        ))
        .await
        .result
        .unwrap();
    harness
        .app
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::seconds(901))
        .await
        .unwrap();
    let warmed = sessions_of(&harness, lead.id).remove(0);

    // Claude Code writes its transcript with the first message, so a session that took no turn
    // has nothing to resume (and a resume the provider rejected leaves exactly such a session).
    submit_user_turn(&harness, conversation_id, "resume-after-warm").await;
    assert!(harness.backend.resume_ids.lock().unwrap().is_empty());
    let fresh = sessions_of(&harness, lead.id).remove(1);
    assert_eq!(
        resume_events(&harness.store),
        vec![(
            "session.resume.skipped".to_owned(),
            json!({
                "sessionId": fresh.id,
                "priorSessionId": warmed.id,
                "reason": "no-completed-turn"
            })
        )]
    );
}

#[tokio::test]
async fn idle_suspension_waits_for_an_unfinished_turn_and_never_touches_a_zero_timeout() {
    use chrono::{TimeDelta, Utc};
    use lazoo_protocol::{ActivationPolicy, SessionLifecycleState};

    // A timeout of 0, the default, keeps a warmed session up however long it stays quiet.
    let (_never_temporary, never) = tool_harness(AuthorityMode::SelfDecision);
    warm_tool_lead(&never).await;
    never
        .app
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::days(30))
        .await
        .unwrap();
    assert_eq!(never.backend.stops.load(Ordering::SeqCst), 0);
    assert_eq!(lead_sessions(&never, SessionLifecycleState::Idle).len(), 1);

    // A session with a turn still in flight is not idle, whatever the clock says.
    let (_temporary, harness) =
        tool_harness_idle(AuthorityMode::SelfDecision, ActivationPolicy::Manual, 900);
    warm_tool_lead(&harness).await;
    harness.backend.gate_events.store(true, Ordering::SeqCst);
    let pending = tokio::spawn({
        let app = harness.app.clone();
        let conversation_id = harness.conversation_id;
        async move {
            app.handle(request(
                "long-turn",
                "turn.submit",
                json!({
                    "conversationId": conversation_id,
                    "author": {"kind": "user"},
                    "recipient": null,
                    "authorityMode": "self-decision",
                    "text": "Take your time",
                    "activateIfNeeded": false,
                    "maxAttempts": 3,
                    "idempotencyKey": "long-turn"
                }),
            ))
            .await
        }
    });
    wait_until(|| harness.backend.turns.load(Ordering::SeqCst) == 1).await;
    harness
        .app
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::days(1))
        .await
        .unwrap();
    assert_eq!(
        harness.backend.stops.load(Ordering::SeqCst),
        0,
        "an unfinished turn keeps its session"
    );

    harness.backend.event_permits.add_permits(2);
    let finished = timeout(Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.result.unwrap()["turn"]["state"], "completed");

    // Quiet time counts from the end of the turn, not from the session start.
    harness
        .app
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::seconds(600))
        .await
        .unwrap();
    assert_eq!(harness.backend.stops.load(Ordering::SeqCst), 0);
    harness
        .app
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::seconds(901))
        .await
        .unwrap();
    assert_eq!(harness.backend.stops.load(Ordering::SeqCst), 1);
    assert!(lead_sessions(&harness, SessionLifecycleState::Idle).is_empty());
}

#[tokio::test]
async fn a_turn_recorded_on_a_since_stopped_session_is_delivered_on_the_current_session() {
    use chrono::{TimeDelta, Utc};
    use lazoo_protocol::{
        ActivationPolicy, MessageSender, SessionLifecycleState, UserMessageEndpoint,
    };
    use lazoo_store::RecordTurnInput;

    let (_temporary, harness) =
        tool_harness_idle(AuthorityMode::SelfDecision, ActivationPolicy::Manual, 900);
    warm_tool_lead(&harness).await;
    let first = lead_sessions(&harness, SessionLifecycleState::Idle).remove(0);
    let recorded = harness
        .store
        .record_turn(&RecordTurnInput {
            request_id: "pinned-turn-request".to_owned(),
            idempotency_key: "pinned-turn".to_owned(),
            conversation_id: harness.conversation_id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: lazoo_protocol::UserMessageKind::User,
            }),
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::SelfDecision,
            text: "Recorded while the first session was up".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap();
    assert_eq!(recorded.turn.recipient.session_id, Some(first.id));

    // A turn left recorded does not hold its session up forever: the session stops for
    // idleness before the turn is delivered, and the user warms a new one.
    harness
        .app
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::seconds(901))
        .await
        .unwrap();
    assert_eq!(
        harness.store.get_session(first.id).unwrap().state,
        SessionLifecycleState::Suspended
    );
    harness
        .app
        .handle(request(
            "warm-second",
            "session.warm",
            json!({
                "employeeId": harness.lead.id,
                "expectedEmployeeRevision": harness.lead.configuration_revision,
                "idempotencyKey": "warm-second"
            }),
        ))
        .await
        .result
        .unwrap();
    let second = lead_sessions(&harness, SessionLifecycleState::Idle).remove(0);

    let delivered = harness
        .app
        .handle(request(
            "pinned-turn-replay",
            "turn.submit",
            json!({
                "conversationId": harness.conversation_id,
                "author": {"kind": "user"},
                "recipient": null,
                "authorityMode": "self-decision",
                "text": "Recorded while the first session was up",
                "activateIfNeeded": false,
                "maxAttempts": 3,
                "idempotencyKey": "pinned-turn"
            }),
        ))
        .await
        .result
        .expect("the recorded turn is not stranded on the stopped session");
    assert_eq!(delivered["turn"]["id"], recorded.turn.id.to_string());
    assert_eq!(delivered["turn"]["state"], "completed");
    assert_eq!(
        harness
            .store
            .get_turn(recorded.turn.id)
            .unwrap()
            .recipient
            .session_id,
        Some(second.id)
    );
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_notice_turns_lead_suspended_for_idleness_is_woken_once_by_its_childs_result() {
    use chrono::{TimeDelta, Utc};
    use lazoo_protocol::{ActivationPolicy, MessageKind, SessionLifecycleState, TaskState};

    let (_temporary, harness) = tool_harness_idle(
        AuthorityMode::SelfDecision,
        ActivationPolicy::NoticeTurns,
        900,
    );
    let (root_task_id, child_id) = delegate_without_waiting(&harness).await;
    harness.app.dispatch_pending_tasks_once().await.unwrap();
    assert_eq!(
        harness.store.get_task(child_id).unwrap().task.state,
        TaskState::Completed
    );
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 2);
    let stops_before = harness.backend.stops.load(Ordering::SeqCst);

    // Nobody stops the lead: its own idle timeout does, once the delegation turn has ended.
    harness
        .app
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::seconds(901))
        .await
        .unwrap();
    assert_eq!(
        harness.backend.stops.load(Ordering::SeqCst),
        stops_before + 1
    );
    assert!(lead_sessions(&harness, SessionLifecycleState::Idle).is_empty());

    // The child's result then wakes the lead once by policy, and that session stops with it.
    harness.app.deliver_pending_notices_once().await.unwrap();
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 3);
    assert!(last_prompt(&harness.backend).contains(&child_id.to_string()));
    assert_eq!(
        lead_messages(&harness.store, MessageKind::Result)[0].state,
        MessageDeliveryState::Acknowledged
    );
    assert_eq!(
        harness.store.get_task(root_task_id).unwrap().task.state,
        TaskState::Completed
    );
    assert_eq!(
        harness.backend.stops.load(Ordering::SeqCst),
        stops_before + 2
    );
    assert_eq!(
        lead_sessions(&harness, SessionLifecycleState::Suspended).len(),
        2,
        "the idle-suspended session and the one policy activation"
    );
}

#[test]
fn idle_suspension_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/idle-suspension.ps1");
    let script = std::fs::read_to_string(&script_path).expect("idle suspension acceptance script");

    for required in [
        "idle_timeouts_default_to_never_on_create_and_stay_unchanged_when_an_update_omits_them",
        "idle_timeouts_are_never_or_a_minute_to_a_day_and_due_only_after_the_whole_timeout",
        "idle_timeout_defaults_to_never_and_a_change_bumps_the_configuration_revision",
        "idle_sessions_are_due_only_after_their_employee_timeout_and_stop_once_under_their_own_key",
        "employee_cli_maps_mutations_and_roster_refresh",
        "an_idle_codex_session_suspends_after_its_employee_timeout_and_the_next_user_turn_starts_fresh_with_a_recorded_reason",
        "an_idle_claude_code_session_suspends_and_the_next_user_turn_resumes_its_native_session",
        "a_suspended_claude_code_session_that_never_completed_a_turn_is_not_resumed",
        "idle_suspension_waits_for_an_unfinished_turn_and_never_touches_a_zero_timeout",
        "a_turn_recorded_on_a_since_stopped_session_is_delivered_on_the_current_session",
        "a_notice_turns_lead_suspended_for_idleness_is_woken_once_by_its_childs_result",
        "store_enables_wal_foreign_keys_and_schema_version",
        "zeroTimeoutNeverSuspends",
        "unfinishedTurnKeepsSession",
        "recordedTurnNotStranded",
        "claudeCodeResumedOnWake",
        "codexStartsFreshWithReason",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
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

/// Raises `code` three times for the worker on `task`, which promotes it to priority.
fn promote_child_incident(
    harness: &ToolHarness,
    prefix: &str,
    code: &str,
    task: uuid::Uuid,
) -> lazoo_protocol::IncidentRecord {
    use lazoo_protocol::{IncidentPriority, IncidentSource};
    use lazoo_store::RaiseIncidentInput;

    // Incidents fold per code, employee, and project: a recurrence on a project task carries
    // the task's project.
    let project_id = harness.store.get_task(task).unwrap().task.project_id;
    let mut promoted = None;
    for attempt in 1..=3 {
        promoted = Some(
            harness
                .store
                .raise_incident(&RaiseIncidentInput {
                    request_id: format!("request-{prefix}-{attempt}"),
                    idempotency_key: format!("{prefix}-{attempt}"),
                    code: code.to_owned(),
                    source: IncidentSource::Daemon,
                    title: None,
                    explanation: None,
                    project_id,
                    employee_id: Some(harness.worker.id),
                    provider: Some("codex".to_owned()),
                    task_id: Some(task),
                    session_id: None,
                    turn_id: None,
                    message_id: None,
                    detail: json!({"failureCode": code}),
                    occurred_at: None,
                })
                .unwrap()
                .incident,
        );
    }
    let promoted = promoted.unwrap();
    assert_eq!(promoted.priority, IncidentPriority::Priority);
    promoted
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One remediation window, from notice to user escalation.
async fn a_lead_remedies_a_promoted_child_incident_in_its_notice_turn_within_the_bound() {
    use lazoo_protocol::{IncidentEscalation, IncidentEscalationReason, TaskState};

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    let (root_task_id, child_id) = delegate_without_waiting(&harness).await;
    let incident = promote_child_incident(&harness, "timeout", "LZ-MESSAGE-TIMEOUT", child_id);
    assert_eq!(incident.escalation, IncidentEscalation::Lead);
    let roster_revision = harness.store.roster_snapshot().unwrap().revision;
    // The model reading the notice: cancel the stuck child, send its objective out again, try
    // a third remedy, and read the incident back.
    harness.backend.tool_calls.lock().unwrap().extend([
        tool_call(
            "call-r1",
            "incident_remedy",
            json!({"incidentId": incident.id, "remedy": "cancel-task", "note": "stuck"}),
        ),
        tool_call(
            "call-r2",
            "incident_remedy",
            json!({
                "incidentId": incident.id,
                "remedy": "redelegate-task",
                "rosterRevision": roster_revision,
                "note": "try again on a fresh dispatch"
            }),
        ),
        tool_call(
            "call-r3",
            "incident_remedy",
            json!({"incidentId": incident.id, "remedy": "cancel-task"}),
        ),
        tool_call(
            "call-r4",
            "incident_remedy",
            json!({"incidentId": incident.id, "remedy": "cancel-task", "rosterRevision": 1}),
        ),
        tool_call("call-r5", "incident_list", json!({})),
    ]);

    harness.app.deliver_pending_notices_once().await.unwrap();

    let prompt = last_prompt(&harness.backend);
    assert!(prompt.starts_with("[Lazoo incident] Priority incident LZ-MESSAGE-TIMEOUT"));
    assert!(
        prompt.contains("incident_remedy (cancel-task, redelegate-task) until"),
        "{prompt}"
    );
    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(
        results.len(),
        6,
        "one delegation, then five calls in the notice turn"
    );
    let cancelled = &results[1];
    assert!(cancelled.success, "{cancelled:?}");
    assert_eq!(cancelled.content["remedy"], "cancel-task");
    assert_eq!(cancelled.content["attempt"], 1);
    assert_eq!(cancelled.content["remediesRemaining"], 1);
    assert_eq!(cancelled.content["taskId"], child_id.to_string());
    assert_eq!(cancelled.content["state"], "cancelled");
    let redelegated = &results[2];
    assert!(redelegated.success, "{redelegated:?}");
    assert_eq!(redelegated.content["attempt"], 2);
    assert_eq!(redelegated.content["remediesRemaining"], 0);
    assert_eq!(redelegated.content["state"], "accepted");
    let retry_id: uuid::Uuid = redelegated.content["taskId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(retry_id, child_id);
    let third = &results[3];
    assert!(!third.success);
    assert_eq!(third.content["error"]["code"], "LZ-INCIDENT-REMEDY-CLOSED");
    let malformed = &results[4];
    assert!(!malformed.success);
    assert_eq!(malformed.content["error"]["code"], "LZ-TOOL-INVALID");
    let listed = results[5].content["incidents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|listed| listed["id"] == incident.id.to_string())
        .cloned()
        .unwrap();
    assert_eq!(listed["escalation"], "lead");
    assert_eq!(listed["remediesApplied"], 2);

    let child = harness.store.get_task(child_id).unwrap().task;
    assert_eq!(child.state, TaskState::Cancelled);
    let retry = harness.store.get_task(retry_id).unwrap().task;
    assert_eq!(retry.parent_task_id, Some(root_task_id));
    assert_eq!(retry.assigned_employee_id, harness.worker.id);
    assert_eq!(retry.objective, child.objective);
    assert_eq!(retry.effective_authority, AuthorityMode::SelfDecision);

    let shown = harness
        .app
        .handle(request(
            "show-remedied",
            "incident.show",
            json!({"id": incident.id}),
        ))
        .await
        .result
        .unwrap();
    let remedies = shown["remedies"].as_array().unwrap();
    assert_eq!(remedies.len(), 2);
    assert_eq!(remedies[0]["kind"], "cancel-task");
    assert_eq!(remedies[1]["kind"], "redelegate-task");
    assert_eq!(remedies[1]["resultTaskId"], retry_id.to_string());
    assert_eq!(remedies[0]["leadEmployeeId"], harness.lead.id.to_string());

    // The failure comes back on the redelegated task: both remedies are spent, so the user
    // gets it now rather than at the window's end.
    let recurred = harness
        .store
        .raise_incident(&lazoo_store::RaiseIncidentInput {
            request_id: "request-timeout-4".to_owned(),
            idempotency_key: "timeout-4".to_owned(),
            code: "LZ-MESSAGE-TIMEOUT".to_owned(),
            source: lazoo_protocol::IncidentSource::Daemon,
            title: None,
            explanation: None,
            project_id: None,
            employee_id: Some(harness.worker.id),
            provider: Some("codex".to_owned()),
            task_id: Some(retry_id),
            session_id: None,
            turn_id: None,
            message_id: None,
            detail: json!({}),
            occurred_at: None,
        })
        .unwrap()
        .incident;
    assert_eq!(recurred.escalation, IncidentEscalation::User);
    assert_eq!(
        recurred.escalation_reason,
        Some(IncidentEscalationReason::RemediesExhausted)
    );
    let for_user = list_incidents(&harness.app, json!({"escalation": "user"})).await;
    assert_eq!(for_user.len(), 1);
    assert_eq!(for_user[0]["id"], incident.id.to_string());
    assert_eq!(for_user[0]["escalationReason"], "remedies-exhausted");
}

#[tokio::test]
async fn an_unremedied_window_reaches_the_user_on_the_sweep_and_nothing_else_changes() {
    use chrono::{TimeDelta, Utc};
    use lazoo_protocol::{IncidentEscalation, TaskState};

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    let (root_task_id, child_id) = delegate_without_waiting(&harness).await;
    let incident = promote_child_incident(&harness, "stall", "LZ-SESSION-STALLED", child_id);
    harness.app.deliver_pending_notices_once().await.unwrap();
    let turns = harness.backend.turns.load(Ordering::SeqCst);
    let events_before = harness.store.list_events(0, 10_000).unwrap().len();

    harness.app.escalate_due_incidents_once(Utc::now()).unwrap();
    assert_eq!(
        harness.store.get_incident(incident.id).unwrap().escalation,
        IncidentEscalation::Lead,
        "the window is still open"
    );
    assert_eq!(
        harness.store.list_events(0, 10_000).unwrap().len(),
        events_before
    );

    harness
        .app
        .escalate_due_incidents_once(Utc::now() + TimeDelta::seconds(301))
        .unwrap();
    let escalated = list_incidents(&harness.app, json!({"escalation": "user"})).await;
    assert_eq!(escalated.len(), 1);
    assert_eq!(escalated[0]["id"], incident.id.to_string());
    assert_eq!(escalated[0]["escalationReason"], "no-remedy");
    assert_eq!(
        escalated[0]["state"], "open",
        "only the user resolves an incident"
    );
    assert!(
        incident_event_types(&harness.store).contains(&"incident.escalated-to-user".to_owned())
    );
    // Escalation is a notification, not an action: no turn, no session, no task change.
    assert_eq!(harness.backend.turns.load(Ordering::SeqCst), turns);
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        harness.store.get_task(child_id).unwrap().task.state,
        TaskState::Accepted
    );
    assert_eq!(
        harness.store.get_task(root_task_id).unwrap().task.state,
        TaskState::Running
    );
}

#[test]
fn lead_remediation_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/lead-remediation.ps1");
    let script = std::fs::read_to_string(&script_path).expect("lead remediation acceptance script");

    for required in [
        "remediation_escalates_unless_a_remedy_holds_through_the_window",
        "incident_notice_states_the_code_title_action_and_boundary",
        "a_promoted_child_incident_gives_the_lead_two_remedies_inside_its_window",
        "the_sweep_escalates_an_unremedied_window_and_keeps_a_remedy_that_held",
        "incidents_with_no_lead_to_remediate_go_to_the_user_at_promotion",
        "version_eleven_priority_incidents_upgrade_straight_to_the_user",
        "incident_cli_maps_list_show_acknowledge_and_resolve",
        "a_lead_remedies_a_promoted_child_incident_in_its_notice_turn_within_the_bound",
        "an_unremedied_window_reaches_the_user_on_the_sweep_and_nothing_else_changes",
        "lead_turn_lists_employees_delegates_and_waits_through_collaboration_tools",
        "store_enables_wal_foreign_keys_and_schema_version",
        "twoRemediesPerIncident",
        "refusedRemedyConsumesNoAttempt",
        "unremediedWindowEscalatesToUser",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
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
fn incident_thresholds_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/incident-thresholds.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("incident thresholds acceptance script");

    for required in [
        "section_fourteen_two_thresholds_promote_stalls_crashes_and_deaths_at_once",
        "threshold_window_promotes_three_equivalent_failures_inside_five_minutes",
        "stalls_and_crashes_promote_at_once_and_a_second_unexpected_exit_promotes",
        "equivalent_failures_fold_escalate_at_the_threshold_and_replay_without_writes",
        "a_stall_or_a_death_under_assigned_work_opens_the_lead_window_on_its_first_occurrence",
        "restart_owner_loss_and_emergency_spool_fold_into_startup_incidents_once",
        "an_unremedied_window_reaches_the_user_on_the_sweep_and_nothing_else_changes",
        "a_promoted_child_incident_gives_the_lead_two_remedies_inside_its_window",
        "store_enables_wal_foreign_keys_and_schema_version",
        "secondUnexpectedExitPromotes",
        "keeperCrashReachesUserAtStartup",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
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

/// Records each desktop notification instead of showing it; `fail` refuses every one.
#[derive(Default)]
struct RecordingNotifier {
    shown: Mutex<Vec<(String, String)>>,
    fail: bool,
}

impl lazood::UserNotifier for RecordingNotifier {
    fn notify(&self, title: &str, message: &str) -> Result<(), String> {
        if self.fail {
            return Err("no desktop session".to_owned());
        }
        self.shown
            .lock()
            .unwrap()
            .push((title.to_owned(), message.to_owned()));
        Ok(())
    }
}

fn raise_scoped(
    harness: &ToolHarness,
    key: &str,
    code: &str,
    source: lazoo_protocol::IncidentSource,
    employee: Option<uuid::Uuid>,
) -> lazoo_protocol::IncidentRecord {
    harness
        .store
        .raise_incident(&lazoo_store::RaiseIncidentInput {
            request_id: format!("request-{key}"),
            idempotency_key: key.to_owned(),
            code: code.to_owned(),
            source,
            title: None,
            explanation: None,
            project_id: None,
            employee_id: employee,
            provider: None,
            task_id: None,
            session_id: None,
            turn_id: None,
            message_id: None,
            detail: json!({}),
            occurred_at: None,
        })
        .unwrap()
        .incident
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One desktop, from a single escalation to a coalesced burst.
async fn user_escalations_reach_the_desktop_once_coalesced_and_never_retried() {
    use chrono::{TimeDelta, Utc};
    use lazoo_protocol::IncidentSource;

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    let (_root_task_id, child_id) = delegate_without_waiting(&harness).await;
    let recorder = Arc::new(RecordingNotifier::default());
    let app = harness.app.clone().with_user_notifier(recorder.clone());

    // A stall with no lead is the user's at once and is announced on its own.
    let stall = raise_scoped(
        &harness,
        "stall",
        "LZ-SESSION-STALLED",
        IncidentSource::Daemon,
        Some(harness.worker.id),
    );
    app.notify_user_escalations_once(Utc::now()).unwrap();
    {
        let shown = recorder.shown.lock().unwrap();
        assert_eq!(shown.len(), 1);
        assert_eq!(
            shown[0].0,
            "Lazoo needs you: Session accepted work but produced no progress"
        );
        assert!(shown[0].1.contains("never retries"), "{}", shown[0].1);
        assert!(shown[0].1.len() <= 240);
    }
    let shown_stall = app
        .handle(request(
            "show-stall",
            "incident.show",
            json!({"id": stall.id}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(shown_stall["userNotification"], "shown");
    assert!(shown_stall["userNotifiedAt"].is_string());

    // A lead window is not the user's yet; the keeper's own record is never announced twice.
    let windowed = promote_child_incident(&harness, "timeout", "LZ-MESSAGE-TIMEOUT", child_id);
    raise_scoped(
        &harness,
        "crash",
        "LZ-DAEMON-UNAVAILABLE",
        IncidentSource::Keeper,
        None,
    );
    app.notify_user_escalations_once(Utc::now()).unwrap();
    assert_eq!(recorder.shown.lock().unwrap().len(), 1);
    let keeper = list_incidents(&app, json!({"code": "LZ-DAEMON-UNAVAILABLE"})).await;
    assert_eq!(keeper[0]["userNotification"], "keeper");

    // The window closes unremedied while another failure reaches the user: one notification
    // names both.
    let later = Utc::now() + TimeDelta::seconds(301);
    app.escalate_due_incidents_once(later).unwrap();
    raise_scoped(
        &harness,
        "lead-exit-1",
        "LZ-PROCESS-EXIT",
        IncidentSource::Daemon,
        Some(harness.lead.id),
    );
    let second_exit = raise_scoped(
        &harness,
        "lead-exit-2",
        "LZ-PROCESS-EXIT",
        IncidentSource::Daemon,
        Some(harness.lead.id),
    );
    app.notify_user_escalations_once(later).unwrap();
    {
        let shown = recorder.shown.lock().unwrap();
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[1].0, "Lazoo: 2 incidents need you");
        assert!(shown[1].1.contains(&windowed.title), "{}", shown[1].1);
        assert!(shown[1].1.contains(&second_exit.title), "{}", shown[1].1);
    }
    app.notify_user_escalations_once(later).unwrap();
    assert_eq!(
        recorder.shown.lock().unwrap().len(),
        2,
        "nothing is announced twice"
    );

    // A notification the desktop refuses is recorded as failed and never retried.
    let refusing = harness
        .app
        .clone()
        .with_user_notifier(Arc::new(RecordingNotifier {
            fail: true,
            ..RecordingNotifier::default()
        }));
    let lost = raise_scoped(
        &harness,
        "lost",
        "LZ-SESSION-STALLED",
        IncidentSource::Daemon,
        Some(harness.lead.id),
    );
    refusing.notify_user_escalations_once(Utc::now()).unwrap();
    app.notify_user_escalations_once(Utc::now()).unwrap();
    assert_eq!(recorder.shown.lock().unwrap().len(), 2);
    let lost = harness.store.get_incident(lost.id).unwrap();
    assert_eq!(
        lost.user_notification,
        Some(lazoo_protocol::IncidentUserNotification::Failed)
    );
    // Nothing about the work changed.
    assert_eq!(
        harness.store.get_task(child_id).unwrap().task.state,
        lazoo_protocol::TaskState::Accepted
    );
}

#[test]
fn user_notifications_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/user-notifications.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("user notifications acceptance script");

    for required in [
        "incident_record_round_trips_with_camel_case_keys",
        "user_escalations_are_claimed_once_and_their_outcome_is_never_rewritten",
        "user_escalations_reach_the_desktop_once_coalesced_and_never_retried",
        "version_eleven_priority_incidents_upgrade_straight_to_the_user",
        "store_enables_wal_foreign_keys_and_schema_version",
        "announcedAtMostOnce",
        "burstCoalescedIntoOneNotification",
        "keeperRecordsNotAnnouncedTwice",
        "failedNotificationNeverRetried",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// The paused clock moves only when every task is parked on a timer, so the virtual time the
/// lead's turn takes is exactly what its waits slept: a store poll would cost at least one
/// 250 ms interval, and a missed wakeup would run to the 10 second deadline and time out.
#[tokio::test(start_paused = true)]
async fn tool_waits_wake_on_the_store_write_instead_of_polling() {
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
    let started = tokio::time::Instant::now();
    let submitted = submit_lead_turn(&harness, "self-decision").await;
    let elapsed = started.elapsed();
    dispatcher.abort();
    let _ = dispatcher.await;

    assert_eq!(submitted["turn"]["state"], "completed");
    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 5, "{results:?}");
    // task_wait woke for the peer question, message_send for the reply, and the second
    // task_wait for the child's completion; none of them reached its deadline.
    assert_eq!(results[1].content["messages"][0]["text"], "Which token?");
    assert_eq!(results[1].content["timedOut"], false);
    assert_eq!(results[3].content["reply"]["text"], "space-4");
    assert_eq!(results[3].content["timedOut"], false);
    assert_eq!(results[4].content["tasks"][0]["state"], "completed");
    assert_eq!(results[4].content["timedOut"], false);
    assert!(
        elapsed < Duration::from_millis(250),
        "the waits slept {elapsed:?} of virtual time instead of waking on the write"
    );
}

#[test]
fn message_arrival_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/message-arrival.ps1");
    let script = std::fs::read_to_string(&script_path).expect("message arrival acceptance script");

    for required in [
        "tool_waits_wake_on_the_store_write_instead_of_polling",
        "task_wait_returns_early_with_a_peer_question_so_the_lead_answers_in_turn",
        "task_wait_consumes_child_results_so_no_notice_turn_follows",
        "store_enables_wal_foreign_keys_and_schema_version",
        "wakesOnStoreWrite",
        "pollIntervalAbsent",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

fn dead_letter_incidents(store: &Store) -> Vec<lazoo_protocol::IncidentRecord> {
    store
        .list_incidents(&lazoo_store::ListIncidentsInput {
            limit: 100,
            code: Some("LZ-MESSAGE-DEADLETTER".to_owned()),
            ..lazoo_store::ListIncidentsInput::default()
        })
        .unwrap()
}

/// A lead's incident notice is a priority message. When its notice turn fails for good, nothing
/// else reports the lost notice, so the sweep raises its dead letter as a priority incident; it
/// is about the lead itself, so it goes to the user and is announced by the same sweep.
#[tokio::test]
async fn a_dead_lettered_incident_notice_reaches_the_user_in_one_sweep() {
    use chrono::Utc;
    use lazoo_protocol::{
        IncidentEscalation, IncidentEscalationReason, IncidentPriority, IncidentSource,
        MessageKind, MessagePriority,
    };

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    let (root_task_id, child_id) = delegate_without_waiting(&harness).await;
    let stall = harness
        .store
        .raise_incident(&lazoo_store::RaiseIncidentInput {
            request_id: "request-stall".to_owned(),
            idempotency_key: "stall".to_owned(),
            code: "LZ-SESSION-STALLED".to_owned(),
            source: IncidentSource::Daemon,
            title: None,
            explanation: None,
            project_id: None,
            employee_id: Some(harness.worker.id),
            provider: Some("codex".to_owned()),
            task_id: Some(child_id),
            session_id: None,
            turn_id: None,
            message_id: None,
            detail: json!({}),
            occurred_at: None,
        })
        .unwrap()
        .incident;
    assert_eq!(stall.escalation, IncidentEscalation::Lead);
    *harness.backend.turn_failure.lock().unwrap() = Some("notice refused".to_owned());
    let _ = harness.app.deliver_pending_notices_once().await;

    let notices = lead_messages(&harness.store, MessageKind::Status);
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].priority, MessagePriority::Priority);
    assert_eq!(notices[0].task_id, Some(root_task_id));
    assert_eq!(notices[0].state, MessageDeliveryState::TerminalFailed);
    assert!(
        harness
            .store
            .get_message(notices[0].id)
            .unwrap()
            .dead_letter
            .is_some()
    );
    assert!(
        dead_letter_incidents(&harness.store).is_empty(),
        "the failed notice turn reports only its own provider failure"
    );

    let recorder = Arc::new(RecordingNotifier::default());
    let app = harness.app.clone().with_user_notifier(recorder.clone());
    let now = Utc::now();
    app.note_priority_dead_letters_once().unwrap();
    app.escalate_due_incidents_once(now).unwrap();
    app.notify_user_escalations_once(now).unwrap();
    let raised = dead_letter_incidents(&harness.store);
    assert_eq!(raised.len(), 1);
    assert_eq!(raised[0].priority, IncidentPriority::Priority);
    assert_eq!(raised[0].occurrences, 1);
    assert_eq!(raised[0].employee_id, Some(harness.lead.id));
    assert_eq!(raised[0].message_id, Some(notices[0].id));
    assert_eq!(raised[0].escalation, IncidentEscalation::User);
    assert_eq!(
        raised[0].escalation_reason,
        Some(IncidentEscalationReason::NoLead)
    );
    assert_eq!(raised[0].detail["priority"], "priority");
    {
        let shown = recorder.shown.lock().unwrap();
        assert_eq!(shown.len(), 1);
        assert_eq!(
            shown[0].0,
            "Lazoo needs you: Message exhausted safe delivery attempts"
        );
    }

    // The next sweep raises nothing new and announces nothing again; no work changed.
    app.note_priority_dead_letters_once().unwrap();
    app.notify_user_escalations_once(Utc::now()).unwrap();
    assert_eq!(dead_letter_incidents(&harness.store)[0].occurrences, 1);
    assert_eq!(recorder.shown.lock().unwrap().len(), 1);
    assert_eq!(
        harness.store.get_task(child_id).unwrap().task.state,
        lazoo_protocol::TaskState::Accepted
    );
}

/// The user's own `message.send` may be priority; `message.fail` raises its dead letter at once,
/// promoted for a priority message and folded for a normal one, and the sweep never raises it
/// twice.
#[tokio::test]
async fn message_fail_promotes_a_priority_dead_letter_at_once_and_only_once() {
    use lazoo_protocol::IncidentPriority;

    let (_temporary, _root, store, app, sender, recipient) = message_test_app();
    let mut dead_letters = Vec::new();
    for (key, priority) in [("normal", "normal"), ("urgent", "priority")] {
        let sent = app
            .handle(request(
                &format!("send-{key}"),
                "message.send",
                json!({
                    "sender": {"employeeId": sender},
                    "recipient": {"employeeId": recipient},
                    "kind": "status",
                    "priority": priority,
                    "body": {"text": key},
                    "idempotencyKey": format!("send-{key}")
                }),
            ))
            .await
            .result
            .unwrap();
        assert_eq!(sent["message"]["envelope"]["priority"], priority);
        let message_id = sent["message"]["envelope"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let claimed = app
            .handle(request(
                &format!("claim-{key}"),
                "message.claim",
                json!({"id": message_id, "expectedRevision": 1, "idempotencyKey": format!("claim-{key}")}),
            ))
            .await
            .result
            .unwrap();
        let failed = app
            .handle(request(
                &format!("fail-{key}"),
                "message.fail",
                json!({
                    "id": message_id,
                    "expectedRevision": 2,
                    "attemptId": claimed["message"]["attempts"][0]["id"],
                    "retryable": false,
                    "failureCode": "LZ-TURN-PROVIDER",
                    "failureMessage": "the adapter could not deliver it",
                    "idempotencyKey": format!("fail-{key}")
                }),
            ))
            .await
            .result
            .unwrap();
        assert!(failed["message"]["deadLetter"].is_object());
        dead_letters.push(dead_letter_incidents(&store));
    }
    assert_eq!(dead_letters[0].len(), 1);
    assert_eq!(dead_letters[0][0].priority, IncidentPriority::Normal);
    assert_eq!(dead_letters[1].len(), 1, "both fold into one incident");
    assert_eq!(dead_letters[1][0].occurrences, 2);
    assert_eq!(dead_letters[1][0].priority, IncidentPriority::Priority);

    app.note_priority_dead_letters_once().unwrap();
    assert_eq!(dead_letter_incidents(&store)[0].occurrences, 2);
    let listed = app
        .handle(request(
            "list",
            "message.list",
            json!({"afterSequence": 0, "limit": 10}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(listed[0]["priority"], "normal");
    assert_eq!(listed[1]["priority"], "priority");
}

#[test]
fn message_priority_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/message-priority.ps1");
    let script = std::fs::read_to_string(&script_path).expect("message priority acceptance script");

    for required in [
        "message_priority_is_kebab_case_and_defaults_to_normal_for_older_records",
        "section_fourteen_two_thresholds_promote_stalls_crashes_and_deaths_at_once",
        "one_dead_lettered_priority_message_is_a_priority_incident_raised_once",
        "version_thirteen_incident_notices_upgrade_as_priority_messages",
        "message_cli_maps_send_envelope",
        "a_dead_lettered_incident_notice_reaches_the_user_in_one_sweep",
        "message_fail_promotes_a_priority_dead_letter_at_once_and_only_once",
        "store_enables_wal_foreign_keys_and_schema_version",
        "priorityDeadLetterPromotedAtOnce",
        "incidentNoticesArePriority",
        "toolsCannotSetPriority",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// The local user tunes a threshold through the daemon: a stall stops paging at the first
/// occurrence, a reset restores the default, and a death under assigned work still promotes
/// at once whatever is configured. No collaboration tool can see or change thresholds.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One user session: list, set, apply, a loss, reset, events.
async fn the_user_configures_incident_thresholds_through_the_daemon() {
    use lazoo_protocol::{IncidentPriority, IncidentSource};

    let (_temporary, harness) = tool_harness(AuthorityMode::SelfDecision);
    let (_root_task_id, child_id) = delegate_without_waiting(&harness).await;
    let call = |id: &str, method: &str, params: serde_json::Value| {
        harness.app.handle(request(id, method, params))
    };

    let defaults = call("list", "incident.threshold.list", json!({}))
        .await
        .result
        .unwrap();
    let stall = defaults
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["code"] == "LZ-SESSION-STALLED")
        .unwrap()
        .clone();
    assert_eq!(
        stall,
        json!({
            "code": "LZ-SESSION-STALLED",
            "occurrences": 1,
            "windowSeconds": 300,
            "source": "default",
            "updatedAt": null
        })
    );
    let refused = call(
        "list-params",
        "incident.threshold.list",
        json!({"code": "LZ-SESSION-STALLED"}),
    )
    .await
    .error
    .unwrap();
    assert_eq!(refused.code, "LZ-INVALID-PARAMS");

    let set = call(
        "set",
        "incident.threshold.set",
        json!({"code": "LZ-SESSION-STALLED", "occurrences": 2, "windowSeconds": 600}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(set["threshold"]["source"], "configured");
    assert_eq!(set["threshold"]["windowSeconds"], 600);
    assert!(set["eventSequence"].is_number());
    let invalid = call(
        "set-invalid",
        "incident.threshold.set",
        json!({"code": "LZ-SESSION-STALLED", "occurrences": 0}),
    )
    .await
    .error
    .unwrap();
    assert_eq!(invalid.code, "LZ-INCIDENT-INVALID");

    let first = raise_scoped(
        &harness,
        "stall-1",
        "LZ-SESSION-STALLED",
        IncidentSource::Daemon,
        Some(harness.worker.id),
    );
    assert_eq!(first.priority, IncidentPriority::Normal);
    let second = raise_scoped(
        &harness,
        "stall-2",
        "LZ-SESSION-STALLED",
        IncidentSource::Daemon,
        Some(harness.worker.id),
    );
    assert_eq!(second.priority, IncidentPriority::Priority);

    // Exits are configured to five, but a death under assigned work is still promoted at once.
    call(
        "set-exit",
        "incident.threshold.set",
        json!({"code": "LZ-PROCESS-EXIT", "occurrences": 5}),
    )
    .await
    .result
    .unwrap();
    let death = harness
        .store
        .raise_incident(&lazoo_store::RaiseIncidentInput {
            request_id: "request-death".to_owned(),
            idempotency_key: "death".to_owned(),
            code: "LZ-PROCESS-EXIT".to_owned(),
            source: IncidentSource::Daemon,
            title: None,
            explanation: None,
            project_id: None,
            employee_id: Some(harness.worker.id),
            provider: Some("codex".to_owned()),
            task_id: Some(child_id),
            session_id: None,
            turn_id: None,
            message_id: None,
            detail: json!({}),
            occurred_at: None,
        })
        .unwrap()
        .incident;
    assert_eq!(death.priority, IncidentPriority::Priority);

    let reset = call(
        "reset",
        "incident.threshold.reset",
        json!({"code": "LZ-SESSION-STALLED"}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(reset["threshold"]["source"], "default");
    assert_eq!(reset["threshold"]["occurrences"], 1);
    let events: Vec<String> = harness
        .store
        .list_events(0, 1000)
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .filter(|kind| kind.starts_with("incident.threshold"))
        .collect();
    assert_eq!(
        events,
        [
            "incident.threshold-set",
            "incident.threshold-set",
            "incident.threshold-reset"
        ]
    );

    // No tool in the catalog reads or sets a threshold.
    let tools = harness.backend.session_tools.lock().unwrap().clone();
    assert!(!tools.is_empty());
    assert!(
        tools
            .iter()
            .flatten()
            .all(|name| !name.contains("threshold"))
    );
}

#[test]
fn configurable_thresholds_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/configurable-thresholds.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("configurable thresholds acceptance script");

    for required in [
        "incident_threshold_contracts_are_strict_and_default_to_five_minutes",
        "a_configured_threshold_replaces_the_default_but_never_delays_a_loss",
        "configured_thresholds_apply_from_the_next_occurrence_and_reset_to_the_default",
        "incident_cli_maps_threshold_list_set_and_reset",
        "the_user_configures_incident_thresholds_through_the_daemon",
        "store_enables_wal_foreign_keys_and_schema_version",
        "overrideAppliesFromNextOccurrence",
        "lossesPromoteWhateverIsConfigured",
        "toolsCannotSeeThresholds",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// The user reads the Inbox through the daemon: a pending approval is the only counted item,
/// its actions name `approval.decide` with the revision the user saw, and the decision moves it
/// to the archive. The Inbox stores nothing and the desktop snapshot is unchanged.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One user session: empty, pending, strict parameters, decided.
async fn the_inbox_lists_a_pending_approval_and_its_decision_archives_it() {
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
    let call = |id: &str, method: &str, params: serde_json::Value| {
        harness.app.handle(request(id, method, params))
    };
    let zero =
        json!({"unresolved": 0, "approvals": 0, "permissions": 0, "incidents": 0, "watching": 0});
    let empty = call("counts-empty", "inbox.counts", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(empty["version"], 1);
    assert_eq!(empty["counts"], zero);

    warm_lead(&harness).await;
    let submission = submit_governed_turn(&harness, "ask");
    let pending = wait_for_pending_approval(&harness).await;

    let counts = call("counts", "inbox.counts", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(counts["counts"]["unresolved"], 1);
    assert_eq!(counts["counts"]["approvals"], 1);
    assert_eq!(counts["counts"]["incidents"], 0);
    let listed = call("list", "inbox.list", json!({"filter": "approvals"}))
        .await
        .result
        .unwrap();
    assert_eq!(listed["version"], 1);
    assert_eq!(listed["truncated"], false);
    assert_eq!(listed["nextBefore"], serde_json::Value::Null);
    assert_eq!(listed["counts"], counts["counts"]);
    assert_eq!(listed["eventHighWater"], counts["eventHighWater"]);
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item["kind"], "approval");
    assert_eq!(item["id"], pending["id"]);
    assert_eq!(item["revision"], pending["revision"]);
    assert_eq!(item["attention"], "action-required");
    assert_eq!(item["severity"], "priority");
    assert_eq!(item["subtype"], "command-execution");
    assert_eq!(item["state"], "pending");
    assert_eq!(item["title"], "Approval requested: cargo test");
    assert_eq!(item["requiredAction"], "Approve or decline: cargo test");
    assert_eq!(item["deadline"]["kind"], "expires");
    assert_eq!(item["deadline"]["at"], pending["expiresAt"]);
    assert_eq!(item["scope"]["employeeName"], "Rhea");
    assert_eq!(item["scope"]["provider"], "codex");
    assert_eq!(
        item["scope"]["conversationId"],
        json!(harness.conversation_id)
    );
    assert_eq!(item["scope"]["projectId"], serde_json::Value::Null);
    assert_eq!(item["scope"]["turnId"], pending["turnId"]);
    assert_eq!(item["detail"]["authority"], "ask");
    assert_eq!(item["detail"]["detail"]["command"], "cargo test");
    let approve = &item["actions"][0];
    assert_eq!(approve["action"], "approve");
    assert_eq!(approve["method"], "approval.decide");
    assert_eq!(
        approve["params"],
        json!({"id": pending["id"], "expectedRevision": pending["revision"], "decision": "approve"})
    );
    assert_eq!(item["actions"][1]["params"]["decision"], "decline");
    for (filter, expected) in [("all", 1), ("incidents", 0), ("permissions", 0)] {
        let listed = call(filter, "inbox.list", json!({"filter": filter}))
            .await
            .result
            .unwrap();
        assert_eq!(
            listed["items"].as_array().unwrap().len(),
            expected,
            "{filter}"
        );
    }

    // Parameters are strict: the limit is bounded, only the archive pages, cursors are opaque
    // but checked, and counts take nothing.
    for (id, params) in [
        ("bad-limit", json!({"filter": "all", "limit": 0})),
        (
            "bad-filter-cursor",
            json!({"filter": "all", "before": "x|y"}),
        ),
        (
            "bad-cursor",
            json!({"filter": "resolved", "before": "nope"}),
        ),
        ("bad-unknown", json!({"filter": "all", "unknown": true})),
    ] {
        let error = call(id, "inbox.list", params).await.error.unwrap();
        assert_eq!(error.code, "LZ-INBOX-INVALID", "{id}");
    }
    let error = call("bad-counts", "inbox.counts", json!({"filter": "all"}))
        .await
        .error
        .unwrap();
    assert_eq!(error.code, "LZ-INVALID-PARAMS");

    // The user sends exactly the action's params plus a fresh idempotency key.
    let mut params = approve["params"].clone();
    params["idempotencyKey"] = json!("approve-from-inbox");
    let decided = call("decide", "approval.decide", params)
        .await
        .result
        .unwrap();
    assert_eq!(decided["approval"]["state"], "approved");
    let submitted = submission.await.unwrap().result.unwrap();
    assert_eq!(submitted["turn"]["state"], "completed");

    let after = call("counts-after", "inbox.counts", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(after["counts"], zero);
    let archive = call("archive", "inbox.list", json!({"filter": "resolved"}))
        .await
        .result
        .unwrap();
    let archived = archive["items"].as_array().unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0]["id"], pending["id"]);
    assert_eq!(archived[0]["attention"], "resolved");
    assert_eq!(archived[0]["state"], "approved");
    assert_eq!(archived[0]["revision"], 2);
    assert_eq!(archived[0]["deadline"], serde_json::Value::Null);
    assert_eq!(archived[0]["actions"], json!([]));
    assert_eq!(archive["truncated"], false);
    assert_eq!(archive["counts"], zero);
    assert!(
        call("all-after", "inbox.list", json!({"filter": "all"}))
            .await
            .result
            .unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // The Inbox is derived: the high-water mark is the store's own, and reading it wrote
    // nothing.
    assert_eq!(
        after["eventHighWater"],
        json!(harness.store.status().unwrap().event_high_water)
    );
    assert_eq!(
        approval_event_types(&harness.store),
        ["approval.requested", "approval.decided"]
    );
    assert!(
        harness
            .store
            .list_events(0, 1000)
            .unwrap()
            .iter()
            .all(|event| !event.event_type.starts_with("inbox"))
    );
    assert_desktop_is_roster_neutral(&harness.app).await;
    let snapshot = call("desktop", "desktop.snapshot", json!({}))
        .await
        .result
        .unwrap();
    assert!(snapshot.get("inbox").is_none());
    assert!(snapshot.get("approvals").is_none());
}

/// Proposals, uncertain tasks, and escalated incidents through the daemon: counted items
/// order first, a lead's window is watched until the sweep hands it to the user, an
/// acknowledged escalation stays visible but uncounted, each action removes its item through
/// the RPC it names, and the archive pages newest first. No tool can read the Inbox.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One user session over every item kind, in order.
async fn the_inbox_derives_tasks_and_incidents_and_pages_the_archive() {
    use chrono::{TimeDelta, Utc};
    use lazoo_protocol::{IncidentEscalation, IncidentSource};

    let (_temporary, harness) = tool_harness(AuthorityMode::Ask);
    warm_tool_lead(&harness).await;
    let roster_revision = harness.store.roster_snapshot().unwrap().revision;
    harness.backend.tool_calls.lock().unwrap().extend(
        ["Implement the parser fix", "Write the parser tests"]
            .into_iter()
            .enumerate()
            .map(|(index, objective)| {
                tool_call(
                    &format!("call-{}", index + 1),
                    "task_delegate",
                    json!({
                        "employeeId": harness.worker.id,
                        "objective": objective,
                        "rosterRevision": roster_revision
                    }),
                )
            }),
    );
    let submitted = submit_lead_turn(&harness, "ask").await;
    assert_eq!(submitted["turn"]["state"], "completed");
    let results = harness.backend.tool_results.lock().unwrap().clone();
    let children: Vec<uuid::Uuid> = results[..2]
        .iter()
        .map(|result| {
            assert!(result.success, "{result:?}");
            assert_eq!(result.content["state"], "proposed");
            result.content["taskId"].as_str().unwrap().parse().unwrap()
        })
        .collect();
    let (proposal_id, uncertain_id) = (children[0], children[1]);
    let call = |id: &str, method: &str, params: serde_json::Value| {
        harness.app.handle(request(id, method, params))
    };

    // Restart truth marks the second child uncertain; the row stays authoritative and the Inbox
    // only reads it.
    harness
        .store
        .execute_for_test(&format!(
            "UPDATE tasks SET state = 'uncertain', failure_code = 'LZ-TURN-OWNER-LOST',
                    failure_message = 'the daemon restarted during the turn'
             WHERE id = '{uncertain_id}';
             INSERT INTO task_evidence (id, task_id, task_revision, kind, actor_kind,
                                        detail_json, created_at)
             VALUES ('{}', '{uncertain_id}', 1, 'uncertain', 'user', '{{}}',
                     '2026-10-13T00:00:00+00:00');",
            uuid::Uuid::new_v4()
        ))
        .unwrap();
    // One incident with no lead reaches the user at once, one on the proposed child opens the
    // lead's window, and one more reaches the user and is acknowledged.
    let stall = raise_scoped(
        &harness,
        "stall",
        "LZ-SESSION-STALLED",
        IncidentSource::Daemon,
        Some(harness.worker.id),
    );
    assert_eq!(stall.escalation, IncidentEscalation::User);
    let windowed = promote_child_incident(&harness, "timeout", "LZ-MESSAGE-TIMEOUT", proposal_id);
    assert_eq!(windowed.escalation, IncidentEscalation::Lead);
    let crashed = raise_scoped(
        &harness,
        "crash",
        "LZ-DAEMON-UNAVAILABLE",
        IncidentSource::Daemon,
        None,
    );
    assert_eq!(crashed.escalation, IncidentEscalation::User);
    call(
        "ack",
        "incident.acknowledge",
        json!({"id": crashed.id, "expectedRevision": crashed.revision, "idempotencyKey": "ack-crash"}),
    )
    .await
    .result
    .unwrap();

    let listed = call("list", "inbox.list", json!({"filter": "all"}))
        .await
        .result
        .unwrap();
    let items = listed["items"].as_array().unwrap().clone();
    let order: Vec<(String, String)> = items
        .iter()
        .map(|item| {
            (
                item["kind"].as_str().unwrap().to_owned(),
                item["attention"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(order.len(), 5, "{order:?}");
    // Both tasks were born in one turn, so they tie on age and may land either way round.
    assert!(
        order[..2]
            .iter()
            .all(|(_, attention)| attention == "action-required")
            && order[..2].iter().any(|(kind, _)| kind == "task-proposal")
            && order[..2].iter().any(|(kind, _)| kind == "task-uncertain"),
        "{order:?}"
    );
    assert_eq!(
        order[2..],
        [
            ("incident".to_owned(), "action-required".to_owned()),
            ("incident".to_owned(), "watching".to_owned()),
            ("incident".to_owned(), "acknowledged".to_owned()),
        ],
        "{order:?}"
    );
    assert_eq!(
        listed["counts"],
        json!({"unresolved": 3, "approvals": 1, "permissions": 0, "incidents": 2, "watching": 1})
    );
    let by_id = |id: uuid::Uuid| {
        items
            .iter()
            .find(|item| item["id"] == json!(id))
            .cloned()
            .unwrap()
    };
    let proposal = by_id(proposal_id);
    assert_eq!(proposal["severity"], "normal");
    assert_eq!(proposal["scope"]["employeeName"], "Mira");
    assert_eq!(
        proposal["scope"]["conversationId"],
        json!(harness.conversation_id)
    );
    assert_eq!(proposal["detail"]["effectiveAuthority"], "ask");
    assert_eq!(proposal["actions"][0]["method"], "task.accept");
    assert_eq!(
        proposal["actions"][1]["params"],
        json!({"id": proposal_id, "expectedRevision": 1, "actor": {"kind": "user"}})
    );
    let uncertain = by_id(uncertain_id);
    assert_eq!(uncertain["kind"], "task-uncertain");
    assert_eq!(uncertain["subtype"], "task-uncertain");
    assert_eq!(uncertain["severity"], "priority");
    assert_eq!(uncertain["detail"]["failureCode"], "LZ-TURN-OWNER-LOST");
    let resolve_failed = uncertain["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["action"] == "resolve-failed")
        .cloned()
        .unwrap();
    assert_eq!(resolve_failed["method"], "task.resolve");
    assert_eq!(
        resolve_failed["params"]["failureCode"],
        "LZ-TURN-OWNER-LOST"
    );
    let watched = by_id(windowed.id);
    assert_eq!(watched["subtype"], "LZ-MESSAGE-TIMEOUT");
    assert_eq!(watched["deadline"]["kind"], "remediation");
    assert_eq!(
        watched["deadline"]["at"],
        json!(windowed.remediation_deadline_at)
    );
    assert_eq!(watched["scope"]["taskId"], json!(proposal_id));
    assert_eq!(
        watched["scope"]["conversationId"],
        json!(harness.conversation_id)
    );
    assert_eq!(watched["actions"], json!([]));
    let stalled = by_id(stall.id);
    assert_eq!(stalled["detail"]["escalationReason"], "no-lead");
    assert_eq!(stalled["actions"][0]["method"], "incident.acknowledge");
    assert_eq!(stalled["actions"][1]["method"], "incident.resolve");
    let acknowledged = by_id(crashed.id);
    assert_eq!(acknowledged["state"], "acknowledged");
    assert_eq!(acknowledged["actions"].as_array().unwrap().len(), 1);
    assert_eq!(acknowledged["actions"][0]["method"], "incident.resolve");
    for (filter, expected) in [("approvals", 1), ("incidents", 4), ("permissions", 0)] {
        let listed = call(filter, "inbox.list", json!({"filter": filter}))
            .await
            .result
            .unwrap();
        assert_eq!(
            listed["items"].as_array().unwrap().len(),
            expected,
            "{filter}"
        );
    }

    // Each action removes its item through the RPC it names.
    let mut accept = proposal["actions"][0]["params"].clone();
    accept["idempotencyKey"] = json!("accept-from-inbox");
    assert_eq!(
        call("accept", "task.accept", accept).await.result.unwrap()["task"]["state"],
        "accepted"
    );
    let mut resolve = resolve_failed["params"].clone();
    resolve["idempotencyKey"] = json!("resolve-from-inbox");
    assert_eq!(
        call("resolve", "task.resolve", resolve)
            .await
            .result
            .unwrap()["task"]["state"],
        "failed"
    );
    let mut settle = stalled["actions"][1]["params"].clone();
    settle["idempotencyKey"] = json!("resolve-stall-from-inbox");
    assert_eq!(
        call("resolve-stall", "incident.resolve", settle)
            .await
            .result
            .unwrap()["incident"]["state"],
        "resolved"
    );
    // The sweep closes the lead's window without a remedy: watched becomes counted.
    harness
        .app
        .escalate_due_incidents_once(Utc::now() + TimeDelta::seconds(301))
        .unwrap();
    let counts = call("counts", "inbox.counts", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(
        counts["counts"],
        json!({"unresolved": 1, "approvals": 0, "permissions": 0, "incidents": 1, "watching": 0})
    );
    let remaining = call("remaining", "inbox.list", json!({"filter": "all"}))
        .await
        .result
        .unwrap();
    let remaining = remaining["items"].as_array().unwrap();
    assert_eq!(remaining.len(), 2);
    assert_eq!(remaining[0]["id"], json!(windowed.id));
    assert_eq!(remaining[0]["attention"], "action-required");
    assert_eq!(remaining[0]["detail"]["escalationReason"], "no-remedy");
    assert_eq!(remaining[0]["deadline"], serde_json::Value::Null);
    assert_eq!(remaining[1]["id"], json!(crashed.id));

    // The archive pages newest first with an opaque cursor; the accepted proposal is not
    // terminal and so not archived.
    let first = call(
        "page-1",
        "inbox.list",
        json!({"filter": "resolved", "limit": 1}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["truncated"], true);
    assert_eq!(first["counts"], counts["counts"]);
    let cursor = first["nextBefore"].as_str().unwrap().to_owned();
    let second = call(
        "page-2",
        "inbox.list",
        json!({"filter": "resolved", "limit": 1, "before": cursor}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["truncated"], false);
    assert_ne!(second["items"][0]["id"], first["items"][0]["id"]);
    let whole = call("archive", "inbox.list", json!({"filter": "resolved"}))
        .await
        .result
        .unwrap();
    let archived = whole["items"].as_array().unwrap();
    assert_eq!(archived.len(), 2);
    assert_eq!(archived[0]["id"], first["items"][0]["id"]);
    assert_eq!(archived[1]["id"], second["items"][0]["id"]);
    let archived_task = archived
        .iter()
        .find(|item| item["id"] == json!(uncertain_id))
        .unwrap();
    assert_eq!(archived_task["kind"], "task-uncertain");
    assert_eq!(archived_task["state"], "failed");
    assert_eq!(archived_task["attention"], "resolved");
    assert_eq!(archived_task["actions"], json!([]));
    let archived_incident = archived
        .iter()
        .find(|item| item["id"] == json!(stall.id))
        .unwrap();
    assert_eq!(archived_incident["state"], "resolved");

    // Nothing was stored for the Inbox, and no tool in the catalog can read it.
    assert!(
        harness
            .store
            .list_events(0, 1000)
            .unwrap()
            .iter()
            .all(|event| !event.event_type.starts_with("inbox"))
    );
    let tools = harness.backend.session_tools.lock().unwrap().clone();
    assert!(!tools.is_empty());
    assert!(tools.iter().flatten().all(|name| !name.contains("inbox")));
}

#[test]
fn desktop_inbox_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/desktop-inbox.ps1");
    let script = std::fs::read_to_string(&script_path).expect("desktop inbox acceptance script");

    for required in [
        "inbox_list_params_default_to_two_hundred_and_reject_unknown_fields",
        "a_pending_approval_needs_the_user_and_a_decided_one_is_the_archive",
        "only_escalated_incidents_are_items_and_only_user_escalations_count",
        "counts_filters_and_order_follow_attention_then_deadline_then_age",
        "cursors_round_trip_and_limits_are_bounded",
        "inbox_cli_maps_list_and_counts",
        "the_inbox_lists_a_pending_approval_and_its_decision_archives_it",
        "the_inbox_derives_tasks_and_incidents_and_pages_the_archive",
        "store_enables_wal_foreign_keys_and_schema_version",
        "badgeCountsOnlyActionRequired",
        "leadWindowWatchedThenCounted",
        "archivePagesWithCursor",
        "inboxStoresNothing",
        "toolsCannotSeeInbox",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// The user reads resource use through the daemon: `system.resources` is the daemon's own
/// process, `session.resources` is a live session's native process tree and refuses one that
/// is not live, nothing is stored, and the desktop snapshot is unchanged.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One user session: the daemon, a warm session, then stopped.
async fn the_user_reads_resource_use_for_the_daemon_and_a_live_session() {
    let (_temporary, app, backend, employee_id) = session_test_app();
    let call =
        |id: &str, method: &str, params: serde_json::Value| app.handle(request(id, method, params));

    let daemon = call("daemon", "system.resources", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(daemon["pid"], std::process::id());
    assert!(daemon["workingSetBytes"].as_u64().unwrap() > 0, "{daemon}");
    assert!(
        daemon["peakWorkingSetBytes"].as_u64().unwrap()
            >= daemon["workingSetBytes"].as_u64().unwrap(),
        "{daemon}"
    );
    assert!(daemon["handleCount"].as_u64().unwrap() > 0, "{daemon}");
    assert_eq!(
        daemon["cpuMillis"],
        json!(
            daemon["userCpuMillis"].as_u64().unwrap() + daemon["kernelCpuMillis"].as_u64().unwrap()
        )
    );
    assert_eq!(daemon["liveSessions"], 0);
    assert!(daemon["sampledAt"].is_string());
    let status = call("status", "system.status", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(daemon["bootId"], status["bootId"]);
    assert!(status.get("resources").is_none());
    let refused = call("daemon-params", "system.resources", json!({"pid": 1}))
        .await
        .error
        .unwrap();
    assert_eq!(refused.code, "LZ-INVALID-PARAMS");

    let warmed = call(
        "warm",
        "session.warm",
        json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 1,
            "idempotencyKey": "warm-rhea-resources"
        }),
    )
    .await
    .result
    .unwrap();
    let session_id = warmed["session"]["id"].clone();
    assert_eq!(backend.starts.load(Ordering::SeqCst), 1);
    let session = call("session", "session.resources", json!({"id": session_id}))
        .await
        .result
        .unwrap();
    assert_eq!(session["sessionId"], session_id);
    assert_eq!(session["employeeId"], json!(employee_id));
    assert_eq!(session["provider"], "codex");
    assert_eq!(session["nativeHostPid"], 4242);
    assert_eq!(session["activeProcesses"], 2);
    assert_eq!(session["totalProcesses"], 3);
    assert_eq!(session["userCpuMillis"], 1500);
    assert_eq!(session["kernelCpuMillis"], 250);
    assert_eq!(session["cpuMillis"], 1750);
    assert_eq!(session["peakProcessMemoryBytes"], 52_428_800);
    assert_eq!(session["peakJobMemoryBytes"], 83_886_080);
    assert!(session["sampledAt"].is_string());
    // The provider's use is reported beside the daemon's own, never folded into it.
    let daemon_live = call("daemon-live", "system.resources", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(daemon_live["liveSessions"], 1);
    assert_eq!(daemon_live["pid"], std::process::id());
    assert!(daemon_live.get("activeProcesses").is_none());
    let shown = call("show", "session.show", json!({"id": session_id}))
        .await
        .result
        .unwrap();
    assert!(shown.get("resources").is_none());
    assert!(shown.get("activeProcesses").is_none());

    let missing = call(
        "missing",
        "session.resources",
        json!({"id": uuid::Uuid::new_v4()}),
    )
    .await
    .error
    .unwrap();
    assert_eq!(missing.code, "LZ-SESSION-NOT-FOUND");
    let invalid = call(
        "invalid",
        "session.resources",
        json!({"id": session_id, "window": "5m"}),
    )
    .await
    .error
    .unwrap();
    assert_eq!(invalid.code, "LZ-INVALID-PARAMS");

    call(
        "stop",
        "session.stop",
        json!({
            "id": session_id,
            "expectedRevision": warmed["session"]["revision"],
            "idempotencyKey": "stop-rhea-resources"
        }),
    )
    .await
    .result
    .unwrap();
    let suspended = call("suspended", "session.resources", json!({"id": session_id}))
        .await
        .error
        .unwrap();
    assert_eq!(suspended.code, "LZ-SESSION-NOT-LIVE");
    assert_eq!(suspended.data.unwrap()["state"], "suspended");
    let daemon_after = call("daemon-after", "system.resources", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(daemon_after["liveSessions"], 0);

    // Nothing was stored: no event names a resource sample, and the snapshot is unchanged.
    let events = app.subscribe(0).unwrap().0;
    assert!(
        events
            .iter()
            .all(|event| !event.event_type.contains("resource"))
    );
    let snapshot = call("desktop", "desktop.snapshot", json!({}))
        .await
        .result
        .unwrap();
    assert!(snapshot.get("resources").is_none());
    assert!(snapshot["status"].get("resources").is_none());
    assert_desktop_is_roster_neutral(&app).await;
}

#[test]
fn resource_accounting_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/resource-accounting.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("resource accounting acceptance script");

    for required in [
        "job_accounting_reports_the_tree_and_survives_its_termination",
        "current_process_accounting_reports_this_process",
        "session_and_system_cli_map_resources",
        "the_user_reads_resource_use_for_the_daemon_and_a_live_session",
        "store_enables_wal_foreign_keys_and_schema_version",
        "sessionResourcesOnlyWhileLive",
        "providerUseSeparateFromDaemonUse",
        "resourcesNeverStored",
        "snapshotUnchanged",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

#[test]
fn resource_budget_acceptance_contract_measures_lazoo_itself_without_a_provider() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/resource-budget.ps1");
    let script = std::fs::read_to_string(&script_path).expect("resource budget acceptance script");

    for required in [
        "'system', 'resources'",
        "idleCpuUnderOnePercent",
        "idleMemoryUnder350MiB",
        "routingCpuUnderFivePercent",
        "failureStormCpuUnderTwentyPercent",
        "failureStormMemoryUnder750MiB",
        "$stormCpuCeiling = 0.20",
        "$activeMemoryCeilingBytes = 750MB",
        "'message', 'fail'",
        "noProviderProcess",
        "plannedShutdown",
        "[Environment]::ProcessorCount",
        "$idleCpuCeiling = 0.01",
        "$routingCpuCeiling = 0.05",
        "$idleMemoryCeilingBytes = 350MB",
        "schemaVersion = 19",
        "real-binaries-no-provider",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "session', 'warm", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// The user renames a conversation through the daemon: the title the user saw is replaced at
/// the revision they saw, a replay changes nothing, a stale revision is refused with the current
/// one, the list and show read models carry the new title, and the desktop snapshot is unchanged.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One user session: create, rename, replay, refusals, read back, clear.
async fn the_user_renames_a_conversation_through_the_daemon() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = configured_employee(&store);
    let app =
        App::new_with_provider_environment(store.clone(), root, DiscoveryEnvironment::empty())
            .unwrap();
    let call =
        |id: &str, method: &str, params: serde_json::Value| app.handle(request(id, method, params));

    let created = call(
        "create",
        "conversation.create",
        json!({
            "projectId": null,
            "leadEmployeeId": employee.id,
            "title": null,
            "idempotencyKey": "create-untitled"
        }),
    )
    .await
    .result
    .unwrap();
    let id = created["conversation"]["id"].clone();
    assert_eq!(created["conversation"]["title"], serde_json::Value::Null);
    assert_eq!(created["conversation"]["revision"], 1);

    let renamed = call(
        "rename",
        "conversation.rename",
        json!({"id": id, "expectedRevision": 1, "title": "Parser fix"}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(renamed["conversation"]["title"], "Parser fix");
    assert_eq!(renamed["conversation"]["revision"], 2);
    assert!(renamed["eventSequence"].is_number());

    let replay = call(
        "rename-replay",
        "conversation.rename",
        json!({"id": id, "expectedRevision": 1, "title": "Parser fix"}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(replay["conversation"], renamed["conversation"]);
    assert_eq!(replay["eventSequence"], serde_json::Value::Null);

    let stale = call(
        "rename-stale",
        "conversation.rename",
        json!({"id": id, "expectedRevision": 1, "title": "Something else"}),
    )
    .await
    .error
    .unwrap();
    assert_eq!(stale.code, "LZ-CONVERSATION-STALE");
    assert_eq!(stale.data.unwrap()["currentRevision"], 2);
    for (label, params, code) in [
        (
            "zero",
            json!({"id": id, "expectedRevision": 0, "title": "x"}),
            "LZ-CONVERSATION-INVALID",
        ),
        (
            "unknown",
            json!({"id": id, "expectedRevision": 2, "unknown": true}),
            "LZ-CONVERSATION-INVALID",
        ),
        (
            "too-long",
            json!({"id": id, "expectedRevision": 2, "title": "x".repeat(300)}),
            "LZ-CONVERSATION-INVALID",
        ),
        (
            "missing",
            json!({"id": uuid::Uuid::new_v4(), "expectedRevision": 1, "title": "x"}),
            "LZ-CONVERSATION-NOT-FOUND",
        ),
    ] {
        let error = call(label, "conversation.rename", params)
            .await
            .error
            .unwrap();
        assert_eq!(error.code, code, "{label}");
    }

    let listed = call("list", "conversation.list", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(listed[0]["title"], "Parser fix");
    assert_eq!(listed[0]["revision"], 2);
    let shown = call("show", "conversation.show", json!({"id": id}))
        .await
        .result
        .unwrap();
    assert_eq!(shown["conversation"]["title"], "Parser fix");

    let cleared = call(
        "clear",
        "conversation.rename",
        json!({"id": id, "expectedRevision": 2, "title": null}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(cleared["conversation"]["title"], serde_json::Value::Null);
    assert_eq!(cleared["conversation"]["revision"], 3);

    let events: Vec<String> = store
        .list_events(0, 100)
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .filter(|kind| kind.starts_with("conversation."))
        .collect();
    assert_eq!(
        events,
        [
            "conversation.created",
            "conversation.renamed",
            "conversation.renamed"
        ]
    );
    assert_desktop_is_roster_neutral(&app).await;
}

#[test]
fn conversation_rename_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/conversation-rename.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("conversation rename acceptance script");

    for required in [
        "conversation_rename_is_revisioned_naturally_idempotent_and_validated",
        "conversation_cli_maps_rename_with_and_without_a_title",
        "the_user_renames_a_conversation_through_the_daemon",
        "store_enables_wal_foreign_keys_and_schema_version",
        "renameIsRevisioned",
        "sameTitleEmitsNoEvent",
        "staleRevisionRefused",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// `provider.config` is read-only discovery: the daemon reports what each provider has
/// configured, with secrets redacted, and the desktop snapshot is unchanged.
#[tokio::test]
async fn provider_config_reports_redacted_integrations_without_changing_the_desktop_snapshot() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let home = temporary.path().join("home");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(
        home.join(".codex").join("config.toml"),
        "[mcp_servers.context7]\nurl = \"https://mcp.context7.com/mcp\"\n[mcp_servers.context7.http_headers]\nCONTEXT7_API_KEY = \"ctx7sk-SECRET\"\n",
    )
    .unwrap();
    std::fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers": {"context7": {"type": "http", "url": "https://mcp.context7.com/mcp"}}}"#,
    )
    .unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let app = App::new_with_provider_environment(
        store,
        root,
        DiscoveryEnvironment::empty().with_config_home(&home),
    )
    .unwrap();

    let report = app
        .handle(request("config", "provider.config", json!({})))
        .await
        .result
        .unwrap();
    assert_eq!(report["version"], 1);
    assert_eq!(report["integrations"].as_array().unwrap().len(), 2);
    assert_eq!(report["integrations"][0]["provider"], "codex");
    assert_eq!(report["integrations"][0]["kind"], "mcp-server");
    assert_eq!(report["integrations"][0]["transport"], "http");
    assert_eq!(
        report["integrations"][0]["secretKeys"],
        json!(["CONTEXT7_API_KEY"])
    );
    assert_eq!(
        report["duplicates"],
        json!([["claude-code/context7", "codex/context7"]])
    );
    assert_eq!(report["conflicts"], json!([]));
    assert!(!report.to_string().contains("SECRET"));
    let files = report["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(files[0]["provider"], "codex");
    assert_eq!(files[0]["parsed"], true);
    assert!(report["unsupported"].as_array().unwrap().len() >= 2);

    let refused = app
        .handle(request(
            "config-params",
            "provider.config",
            json!({"provider": "codex"}),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(refused.code, "LZ-INVALID-PARAMS");
    let snapshot = app
        .handle(request("desktop", "desktop.snapshot", json!({})))
        .await
        .result
        .unwrap();
    assert!(snapshot.get("integrations").is_none());
    assert!(snapshot.get("configuration").is_none());
    assert!(
        home.join(".codex").join("config.toml").is_file(),
        "nothing was written or moved"
    );
}

#[test]
fn provider_config_acceptance_contract_is_read_only_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/provider-config-discovery.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("provider config discovery acceptance script");

    for required in [
        "provider_configuration_is_parsed_from_known_formats_redacted_hashed_and_compared",
        "missing_and_malformed_configuration_is_reported_without_quoting_it_or_failing",
        "provider_cli_maps_config",
        "provider_config_reports_redacted_integrations_without_changing_the_desktop_snapshot",
        "store_enables_wal_foreign_keys_and_schema_version",
        "knownFormatsParsed",
        "secretsRedacted",
        "duplicatesAndConflictsNamed",
        "malformedFilesReportedNotQuoted",
        "nothingWritten",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// Brief section 21.2, the core end-to-end scenario, on the deterministic provider: steps 1 to
/// 14 in one daemon lifetime with a restart in the middle, including the peer clarification
/// (7), the stall (9), the dirty-work preservation (13), and the concise memory record (12)
/// written by the memory worker from the completed task's dispatch turn on the restarted boot.
/// The resource budget (15) is proven by its own acceptance script, which the core-scenario
/// manifest names per step.
#[tokio::test]
#[allow(clippy::too_many_lines)] // The scenario is one story by definition.
async fn the_core_scenario_runs_end_to_end_on_the_deterministic_provider() {
    use chrono::{TimeDelta, Utc};
    use lazoo_protocol::{
        IncidentEscalation, IncidentPriority, SessionLifecycleState, TaskEvidenceKind, TaskState,
        TaskWorkspaceKind,
    };
    use lazoo_store::ListIncidentsInput;

    let (temporary, mut harness) = tool_harness(AuthorityMode::Ask);

    // 3. The conversation lives in a registered project: a repository with a base commit, so
    // every dispatched task gets its own retained worktree from it.
    let repository = temporary.path().join("repository");
    git_init(&repository);
    git(
        &repository,
        &["config", "user.email", "lazoo@example.invalid"],
    );
    git(&repository, &["config", "user.name", "Lazoo Test"]);
    git(&repository, &["config", "core.autocrlf", "false"]);
    std::fs::write(repository.join("parser.txt"), "base\n").unwrap();
    git(&repository, &["add", "parser.txt"]);
    git(&repository, &["commit", "--quiet", "-m", "base"]);
    let project_id: uuid::Uuid = harness
        .app
        .handle(request(
            "register-project",
            "project.register",
            json!({"path": repository, "displayName": "Parser", "idempotencyKey": "scenario-project"}),
        ))
        .await
        .result
        .unwrap()["project"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    harness.conversation_id = harness
        .app
        .handle(request(
            "create-project-conversation",
            "conversation.create",
            json!({
                "projectId": project_id,
                "leadEmployeeId": harness.lead.id,
                "title": "Parser fix",
                "idempotencyKey": "scenario-project-conversation"
            }),
        ))
        .await
        .result
        .unwrap()["conversation"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let harness = harness;
    let call = |id: &str, method: &str, params: serde_json::Value| {
        harness.app.handle(request(id, method, params))
    };
    let peer = |name: &str, provider: &str| CreateEmployeeInput {
        request_id: format!("scenario-{name}-request"),
        idempotency_key: format!("scenario-{name}"),
        name: name.to_owned(),
        provider: provider.to_owned(),
        role: "Worker".to_owned(),
        instructions: String::new(),
        model: None,
        reasoning_effort: None,
        native_settings: json!({}),
        authority_mode: AuthorityMode::Ask,
        checkout_policy: EmployeeCheckoutPolicy::Isolated,
        activation_policy: lazoo_protocol::ActivationPolicy::Manual,
        idle_timeout_seconds: 0,
        enabled: true,
        provider_observation: Some(ProviderObservation {
            provider: provider.to_owned(),
            verification: "configured".to_owned(),
            version: Some("test".to_owned()),
            protocol: Some("test".to_owned()),
            capabilities: json!({"session": true}),
            authentication: json!({"native": "ready", "external": "unknown"}),
            degradation_reason: None,
            checked_at: "2026-10-13T00:00:00Z".to_owned(),
        }),
    };

    // 1. Rhea leads; Mira and Nico on Codex and three peers on other providers; one peer is
    // disabled.
    let mut nico_input = peer("Nico", "codex");
    nico_input.model = Some("test-model".to_owned());
    if let Some(observation) = nico_input.provider_observation.as_mut() {
        observation.capabilities = json!({"session": true, "permission": true, "mcp": true});
    }
    let nico = harness.store.create_employee(&nico_input).unwrap().employee;
    let oren = harness
        .store
        .create_employee(&peer("Oren", "opencode"))
        .unwrap()
        .employee;
    let pia = harness
        .store
        .create_employee(&peer("Pia", "grok-build"))
        .unwrap()
        .employee;
    let quinn = harness
        .store
        .create_employee(&peer("Quinn", "antigravity"))
        .unwrap()
        .employee;
    call(
        "disable-quinn",
        "employee.disable",
        json!({
            "id": quinn.id,
            "expectedRevision": quinn.configuration_revision,
            "idempotencyKey": "scenario-disable-quinn"
        }),
    )
    .await
    .result
    .unwrap();

    // 2. Every employee begins dormant; Quinn is disabled.
    let roster = call("roster", "roster.get", json!({}))
        .await
        .result
        .unwrap();
    let entries = roster["entries"].as_array().unwrap().clone();
    assert_eq!(entries.len(), 6);
    let availability = |id: uuid::Uuid| {
        entries
            .iter()
            .find(|entry| entry["employee"]["id"] == json!(id))
            .unwrap()["availability"]
            .clone()
    };
    for id in [harness.lead.id, harness.worker.id, nico.id, oren.id, pia.id] {
        assert_eq!(availability(id), "dormant");
    }
    assert_eq!(availability(quinn.id), "disabled");
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 0);

    // 4. Rhea reads the live roster and proposes a two-part plan for Mira in the project
    // conversation; 8. the disabled peer cannot be selected.
    warm_tool_lead(&harness).await;
    let roster_revision = harness.store.roster_snapshot().unwrap().revision;
    let delegate = |call_id: &str, employee: uuid::Uuid, objective: &str| {
        tool_call(
            call_id,
            "task_delegate",
            json!({"employeeId": employee, "objective": objective, "rosterRevision": roster_revision}),
        )
    };
    harness.backend.tool_calls.lock().unwrap().extend([
        tool_call("call-1", "employees_list", json!({})),
        delegate("call-2", nico.id, "Write the parser tests"),
        delegate("call-3", harness.worker.id, "Implement the parser fix"),
        delegate("call-4", quinn.id, "Review the plan"),
    ]);
    let submitted = submit_lead_turn(&harness, "ask").await;
    assert_eq!(submitted["turn"]["state"], "completed");
    let root_task_id: uuid::Uuid = submitted["turn"]["taskId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 4, "{results:?}");
    assert!(results[0].success, "{:?}", results[0]);
    assert!(
        results[0].content.to_string().contains("Quinn"),
        "the roster the lead reads names the disabled peer, marked unavailable"
    );
    let children: Vec<uuid::Uuid> = results[1..3]
        .iter()
        .map(|result| {
            assert!(result.success, "{result:?}");
            assert_eq!(result.content["state"], "proposed");
            result.content["taskId"].as_str().unwrap().parse().unwrap()
        })
        .collect();
    assert!(
        !results[3].success,
        "a disabled employee cannot receive dispatch"
    );
    assert_eq!(
        results[3].content["error"]["code"], "LZ-TASK-ASSIGNEE-UNAVAILABLE",
        "{:?}",
        results[3]
    );

    // 5. Under ask, Lazoo presents the delegation approvals; the user accepts both.
    let inbox = call("inbox", "inbox.list", json!({"filter": "approvals"}))
        .await
        .result
        .unwrap();
    assert_eq!(inbox["counts"]["approvals"], 2);
    assert_eq!(inbox["counts"]["unresolved"], 2);
    for (index, child) in children.iter().enumerate() {
        let accepted = call(
            &format!("accept-{index}"),
            "task.accept",
            json!({
                "id": child,
                "expectedRevision": 1,
                "idempotencyKey": format!("scenario-accept-{index}")
            }),
        )
        .await
        .result
        .unwrap();
        assert_eq!(accepted["task"]["state"], "accepted");
    }
    let counts = call("inbox-after", "inbox.counts", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(counts["counts"]["unresolved"], 0);

    // 13. The user has uncommitted work in the canonical checkout before any worker starts;
    // nothing below may touch it.
    std::fs::write(repository.join("parser.txt"), "dirty user edit\n").unwrap();

    // 6 and 7. The approved employees activate into independent native sessions and
    // workspaces, one fresh session per task that stops with its turn. Sessions are numbered
    // in start order: Nico's task was delegated first, so his is the second and Mira's the
    // third. Inside her turn Mira asks Nico, not Rhea, for the one fact she is missing and
    // waits for his answer; Nico's turn is waiting on her task and finds the question in its
    // `task_wait`, answers, and both turns finish. Mira then reports her completion through
    // `task_report` with commit and verification evidence before her native turn ends (12).
    let (nico_task, mira_task) = (children[0], children[1]);
    harness.backend.tool_calls.lock().unwrap().extend([
        session_tool_call(
            "native-2",
            "call-n1",
            "task_wait",
            json!({"taskIds": [mira_task], "timeoutSeconds": 10}),
        ),
        session_tool_call(
            "native-2",
            "call-n2",
            "message_reply",
            json!({"messageId": "$message-id", "text": "space-4"}),
        ),
        session_tool_call(
            "native-3",
            "call-m1",
            "message_send",
            json!({
                "employeeId": nico.id,
                "text": "Which spacing token should the parser emit?",
                "waitForReplySeconds": 10
            }),
        ),
        session_tool_call(
            "native-3",
            "call-m2",
            "task_report",
            json!({
                "kind": "completed",
                "evidence": {
                    "commits": [{"sha": "0123abcd", "message": "Fix the parser"}],
                    "verification": [{"command": "cargo test -p parser", "exitCode": 0}]
                }
            }),
        ),
    ]);
    // The first dispatch pass takes Nico's task and stays inside his waiting turn; once his
    // turn is in flight, a second pass sees only Mira's task (his already has a dispatch row)
    // and runs hers to completion, which lets his finish too.
    let first_pass_app = harness.app.clone();
    let first_pass =
        tokio::spawn(async move { first_pass_app.dispatch_pending_tasks_once().await });
    timeout(Duration::from_secs(10), async {
        loop {
            let detail = harness.store.get_task(nico_task).unwrap();
            if detail.task.state == TaskState::Running
                && detail
                    .dispatch
                    .as_ref()
                    .is_some_and(|dispatch| dispatch.turn_id.is_some())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Nico's turn is in flight before Mira's starts");
    timeout(
        Duration::from_secs(20),
        harness.app.dispatch_pending_tasks_once(),
    )
    .await
    .expect("Mira's turn ends once Nico's reply arrives")
    .unwrap();
    let first_pass = timeout(Duration::from_secs(20), first_pass)
        .await
        .expect("Nico's turn ends once he has answered")
        .unwrap();
    assert!(
        first_pass.is_ok()
            || first_pass
                .as_ref()
                .is_err_and(|error| error.code.starts_with("LZ-TASK-")),
        "the first pass finds Mira's task already dispatched: {first_pass:?}"
    );
    assert_eq!(
        harness.backend.starts.load(Ordering::SeqCst),
        3,
        "Rhea's session and one per dispatched task"
    );
    let mut worktrees = std::collections::BTreeSet::new();
    for child in &children {
        let detail = harness.store.get_task(*child).unwrap();
        assert_eq!(detail.task.state, TaskState::Completed);
        let dispatch = detail.dispatch.unwrap();
        assert_eq!(dispatch.project_id, Some(project_id));
        assert_eq!(
            dispatch.workspace_kind,
            Some(TaskWorkspaceKind::GitWorktree)
        );
        assert!(dispatch.branch_name.is_some() && dispatch.base_commit.is_some());
        let workspace = dispatch.workspace_path.unwrap();
        let path = Path::new(&workspace);
        assert!(
            path.is_dir() && path.join("parser.txt").is_file(),
            "{workspace}"
        );
        assert_ne!(path, repository.as_path(), "never the canonical checkout");
        // 13. Each worktree starts from the committed base under the configured authority,
        // and the user's dirty edit is never carried into it.
        assert_eq!(
            std::fs::read_to_string(path.join("parser.txt")).unwrap(),
            "base\n"
        );
        assert_eq!(detail.task.effective_authority, AuthorityMode::Ask);
        assert!(
            worktrees.insert(workspace),
            "each task has its own worktree"
        );
    }
    assert_eq!(
        std::fs::read_to_string(repository.join("parser.txt")).unwrap(),
        "dirty user edit\n",
        "the canonical checkout keeps the user's uncommitted work"
    );
    assert!(
        harness
            .backend
            .working_directories
            .lock()
            .unwrap()
            .iter()
            .filter(|directory| worktrees.contains(&directory.to_string_lossy().into_owned()))
            .count()
            == 2,
        "each worker session started inside its task's worktree"
    );
    assert!(
        harness
            .store
            .list_sessions()
            .unwrap()
            .iter()
            .filter(|session| [harness.worker.id, nico.id].contains(&session.employee_id))
            .all(|session| session.state == lazoo_protocol::SessionLifecycleState::Suspended),
        "a worker's dispatch session stops with its turn"
    );

    // 7. The clarification went from Mira's task to Nico's and back without Rhea: Nico's wait
    // returned early with the question, his reply closed it, Mira's wait returned the answer,
    // the ledger holds both messages between the two tasks, and no peer turn reached Rhea.
    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 8, "{results:?}");
    assert!(results[4].success, "{:?}", results[4]);
    assert_eq!(results[4].content["timedOut"], false);
    let question = &results[4].content["messages"][0];
    assert_eq!(question["fromEmployeeId"], harness.worker.id.to_string());
    assert_eq!(
        question["text"],
        "Which spacing token should the parser emit?"
    );
    assert!(results[5].success, "{:?}", results[5]);
    assert_eq!(
        results[5].content["replyToMessageId"],
        question["messageId"]
    );
    assert!(results[6].success, "{:?}", results[6]);
    assert_eq!(results[6].content["timedOut"], false);
    assert_eq!(results[6].content["reply"]["text"], "space-4");
    assert_eq!(
        results[6].content["reply"]["fromEmployeeId"],
        nico.id.to_string()
    );
    let exchange: Vec<_> = employee_messages(&harness.store)
        .into_iter()
        .filter(|message| {
            matches!(
                message.kind,
                lazoo_protocol::MessageKind::Peer | lazoo_protocol::MessageKind::Reply
            )
        })
        .collect();
    assert_eq!(exchange.len(), 2, "{exchange:?}");
    assert_eq!(
        exchange[0].task_id,
        Some(nico_task),
        "the question is Nico's task's"
    );
    assert_eq!(
        exchange[1].task_id,
        Some(mira_task),
        "the reply is Mira's task's"
    );
    assert!(
        prompts(&harness.backend)
            .iter()
            .all(|prompt| !prompt.starts_with("[Lazoo peer message]")),
        "no peer turn reached Rhea"
    );
    assert_eq!(
        harness.store.pending_task_results(10).unwrap().len(),
        2,
        "both results wait to reach Rhea"
    );

    // 12. The reported commits and verification are the task's durable evidence, readable
    // through `task.show`; the result notice below links the task; incidents and messages are
    // ledgers of their own. The concise memory record is written from this dispatch turn on the
    // restarted boot, after step 14, so the summary is the story's last provider turn.
    let reported = harness.store.get_task(mira_task).unwrap();
    let mira_turn_id = reported.dispatch.as_ref().unwrap().turn_id.unwrap();
    let completed = reported
        .evidence
        .iter()
        .find(|evidence| {
            evidence.kind == TaskEvidenceKind::Completed && evidence.detail.get("commits").is_some()
        })
        .expect("the worker's completed report with its evidence");
    assert_eq!(completed.detail["commits"][0]["sha"], "0123abcd");
    assert_eq!(completed.detail["verification"][0]["exitCode"], 0);
    assert_eq!(
        completed.session_id,
        reported.dispatch.as_ref().unwrap().session_id,
        "the evidence names the worker's own session"
    );
    let shown = call("show-reported", "task.show", json!({"id": mira_task}))
        .await
        .result
        .unwrap();
    assert!(
        shown["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|evidence| evidence["detail"]["commits"][0]["sha"] == "0123abcd"),
        "{shown}"
    );
    harness.app.deliver_pending_notices_once().await.unwrap();
    let prompt = last_prompt(&harness.backend);
    assert!(prompt.contains("[Lazoo task result]"), "{prompt}");
    assert_eq!(
        harness.store.get_task(root_task_id).unwrap().task.state,
        TaskState::Completed,
        "Rhea's first plan is done once both results reached her"
    );

    // 3 and 4 again. The user asks for more; Rhea delegates a third task under a new root, the
    // user accepts it, and it waits in the queue.
    let roster_revision = harness.store.roster_snapshot().unwrap().revision;
    harness
        .backend
        .tool_calls
        .lock()
        .unwrap()
        .extend([tool_call(
            "call-5",
            "task_delegate",
            json!({
                "employeeId": harness.worker.id,
                "objective": "Document the parser",
                "rosterRevision": roster_revision
            }),
        )]);
    let follow_up = timeout(
        Duration::from_secs(20),
        harness.app.handle(request(
            "submit-scenario-follow-up",
            "turn.submit",
            json!({
                "conversationId": harness.conversation_id,
                "author": {"kind": "user"},
                "recipient": null,
                "authorityMode": "ask",
                "text": "Also document the parser",
                "activateIfNeeded": false,
                "maxAttempts": 3,
                "idempotencyKey": "submit-scenario-follow-up"
            }),
        )),
    )
    .await
    .unwrap()
    .result
    .unwrap();
    assert_eq!(follow_up["turn"]["state"], "completed");
    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 9, "{results:?}");
    assert!(results[8].success, "{:?}", results[8]);
    let third: uuid::Uuid = results[8].content["taskId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let second_root_id = harness.store.get_task(third).unwrap().task.root_task_id;
    assert_ne!(
        second_root_id, root_task_id,
        "a finished plan is not reopened"
    );
    call(
        "accept-third",
        "task.accept",
        json!({"id": third, "expectedRevision": 1, "idempotencyKey": "scenario-accept-third"}),
    )
    .await
    .result
    .unwrap();

    // 9. Mira's provider stalls after accepting the third task's dispatch: the adapter's
    // progress deadline surfaces as a stalled turn, the daemon records it as retryable, keeps
    // the task accepted for a bounded retry by the lead or the user (never automatically),
    // stops the session, and raises one clean `LZ-SESSION-STALLED` incident on the task.
    harness.backend.turn_stall.store(true, Ordering::SeqCst);
    let stalled = harness.app.dispatch_pending_tasks_once().await.unwrap_err();
    harness.backend.turn_stall.store(false, Ordering::SeqCst);
    assert_eq!(stalled.code, "LZ-TURN-PROVIDER");
    let stalled_task = harness.store.get_task(third).unwrap();
    assert_eq!(stalled_task.task.state, TaskState::Accepted);
    let stalled_turn = harness
        .store
        .get_turn(stalled_task.dispatch.unwrap().turn_id.unwrap())
        .unwrap();
    assert_eq!(stalled_turn.state, TurnState::RetryableFailed);
    assert_eq!(
        stalled_turn.failure_code.as_deref(),
        Some("LZ-TURN-PROVIDER")
    );
    assert_eq!(
        harness.backend.starts.load(Ordering::SeqCst),
        4,
        "the stalled dispatch ran on its own session"
    );
    assert!(
        harness
            .store
            .list_sessions()
            .unwrap()
            .iter()
            .all(|session| session.state != SessionLifecycleState::Idle
                || session.employee_id == harness.lead.id),
        "the stalled worker session was stopped"
    );
    let stalls: Vec<_> = harness
        .store
        .list_incidents(&ListIncidentsInput {
            code: Some("LZ-SESSION-STALLED".to_owned()),
            limit: 100,
            ..ListIncidentsInput::default()
        })
        .unwrap();
    assert_eq!(stalls.len(), 1, "{stalls:?}");
    let incident = stalls.into_iter().next().unwrap();
    assert_eq!(incident.task_id, Some(third));
    assert_eq!(incident.employee_id, Some(harness.worker.id));
    assert_eq!(incident.occurrences, 1);

    // 10. The stall is promoted at once: Rhea receives the incident with its remediation
    // window as a notice turn, applies one remedy, and when the stall recurs the closed window
    // escalates to the user, who sees it in the Inbox.
    assert_eq!(incident.priority, IncidentPriority::Priority);
    assert_eq!(incident.escalation, IncidentEscalation::Lead);
    harness
        .backend
        .tool_calls
        .lock()
        .unwrap()
        .extend([tool_call(
            "call-r1",
            "incident_remedy",
            json!({
                "incidentId": incident.id,
                "remedy": "cancel-task",
                "note": "the worker stalled before starting"
            }),
        )]);
    harness.app.deliver_pending_notices_once().await.unwrap();
    let prompt = last_prompt(&harness.backend);
    assert!(prompt.contains("[Lazoo incident]"), "{prompt}");
    assert_eq!(
        harness.backend.starts.load(Ordering::SeqCst),
        4,
        "the notices rode Rhea's idle session"
    );
    let remedied = harness.store.get_incident(incident.id).unwrap();
    assert_eq!(remedied.remedies.len(), 1, "{remedied:?}");
    assert_eq!(
        harness.store.get_task(third).unwrap().task.state,
        TaskState::Cancelled
    );
    promote_child_incident(&harness, "stall-again", "LZ-SESSION-STALLED", third);
    harness
        .app
        .escalate_due_incidents_once(Utc::now() + TimeDelta::seconds(301))
        .unwrap();
    let escalated = harness.store.get_incident(incident.id).unwrap();
    assert_eq!(escalated.escalation, IncidentEscalation::User);
    let inbox = call(
        "inbox-incident",
        "inbox.list",
        json!({"filter": "incidents"}),
    )
    .await
    .result
    .unwrap();
    let item = inbox["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == json!(incident.id))
        .cloned()
        .unwrap();
    assert_eq!(item["attention"], "action-required");
    assert_eq!(item["detail"]["escalationReason"], "recurred-after-remedy");
    let mut acknowledge = item["actions"][0]["params"].clone();
    acknowledge["idempotencyKey"] = json!("scenario-acknowledge");
    call("acknowledge", "incident.acknowledge", acknowledge)
        .await
        .result
        .unwrap();

    // 11. Lazoo restarts: the new boot owns none of the prior boot's sessions and adopts none
    // of its work, and every logical task is still there exactly once.
    let tasks_before: Vec<uuid::Uuid> = harness
        .store
        .list_tasks(&lazoo_store::ListTasksInput {
            after_sequence: 0,
            limit: 100,
            root_task_id: None,
            parent_task_id: None,
            conversation_id: None,
            project_id: None,
            assigned_employee_id: None,
            state: None,
        })
        .unwrap()
        .into_iter()
        .map(|task| task.id)
        .collect();
    assert_eq!(tasks_before.len(), 5, "two roots and three children");
    let restarted = App::new_with_session_backend(
        harness.store.clone(),
        DataRoot::from_path(temporary.path().join("data")),
        DiscoveryEnvironment::empty(),
        Arc::new(harness.backend.clone()),
    )
    .unwrap();
    let tasks_after: Vec<uuid::Uuid> = harness
        .store
        .list_tasks(&lazoo_store::ListTasksInput {
            after_sequence: 0,
            limit: 100,
            root_task_id: None,
            parent_task_id: None,
            conversation_id: None,
            project_id: None,
            assigned_employee_id: None,
            state: None,
        })
        .unwrap()
        .into_iter()
        .map(|task| task.id)
        .collect();
    assert_eq!(tasks_after, tasks_before, "no task lost or duplicated");
    let sessions = restarted
        .handle(request("sessions", "session.list", json!({})))
        .await
        .result
        .unwrap();
    assert!(
        sessions
            .as_array()
            .unwrap()
            .iter()
            .all(|session| session["state"] == "failed" || session["state"] == "suspended"),
        "the prior boot's sessions are owner-lost, never adopted: {sessions}"
    );
    assert_eq!(
        harness.store.get_incident(incident.id).unwrap().state,
        lazoo_protocol::IncidentState::Acknowledged,
        "the user's acknowledgement survives the restart"
    );

    // 14. On the restarted daemon Mira's idle timeout is one minute: her session warms, stays
    // idle, and the sweep suspends it once the minute has passed.
    let mira = harness.store.get_employee(harness.worker.id).unwrap();
    restarted
        .handle(request(
            "mira-idle",
            "employee.update",
            json!({
                "id": mira.id,
                "expectedRevision": mira.configuration_revision,
                "idleTimeoutSeconds": 60,
                "idempotencyKey": "scenario-mira-idle"
            }),
        ))
        .await
        .result
        .unwrap();
    let mira = harness.store.get_employee(mira.id).unwrap();
    let warmed = restarted
        .handle(request(
            "warm-mira",
            "session.warm",
            json!({
                "employeeId": mira.id,
                "expectedEmployeeRevision": mira.configuration_revision,
                "idempotencyKey": "scenario-warm-mira"
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(warmed["session"]["state"], "idle");
    let session_id = warmed["session"]["id"].clone();
    restarted
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::seconds(59))
        .await
        .unwrap();
    let still_idle = restarted
        .handle(request(
            "show-idle",
            "session.show",
            json!({"id": session_id}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(still_idle["state"], "idle");
    restarted
        .suspend_idle_sessions_once(Utc::now() + TimeDelta::seconds(61))
        .await
        .unwrap();
    let suspended = restarted
        .handle(request(
            "show-suspended",
            "session.show",
            json!({"id": session_id}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(suspended["state"], "suspended");
    let roster = restarted
        .handle(request("roster-after", "roster.get", json!({})))
        .await
        .result
        .unwrap();
    let mira_entry = roster["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["employee"]["id"] == json!(mira.id))
        .cloned()
        .unwrap();
    assert_eq!(mira_entry["availability"], "suspended");

    // 12, concluded. Sage is a dedicated dormant Codex summary employee: never warmed, never
    // dispatched. The user opts the project into the memory worker on her and archives Mira's
    // completed dispatch turn; the worker's one admission summarizes it through a fresh
    // read-only session into a typed record citing that turn, which `memory.search` finds by
    // the task. The scripted JSON output stands in for a model: the pipeline is proven here,
    // summary quality is not. This is the story's last provider turn, because the swapped
    // output is shared by every later fake turn.
    let mut sage_input = peer("Sage", "codex");
    sage_input.model = Some("test-model".to_owned());
    if let Some(observation) = sage_input.provider_observation.as_mut() {
        observation.capabilities = json!({"session": true, "permission": true, "mcp": true});
    }
    let sage = harness.store.create_employee(&sage_input).unwrap().employee;
    let configured = restarted
        .handle(request(
            "configure-memory-worker",
            "memory.worker.configure",
            json!({
                "projectId": project_id,
                "employeeId": sage.id,
                "model": "test-model",
                "requestsPerMinute": 1,
                "enabled": true,
                "expectedRevision": null
            }),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(configured["settings"]["enabled"], true);
    assert_eq!(configured["provider"], "codex");
    assert_eq!(configured["model"], "test-model");
    assert_eq!(
        configured["projectPermission"]["state"],
        "nativeEmployeeOnly"
    );
    let archived = restarted
        .handle(request(
            "archive-mira-turn",
            "memory.archive",
            json!({"sourceTurnId": mira_turn_id, "projectId": project_id}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(archived["archive"]["sourceTurnId"], json!(mira_turn_id));
    assert_eq!(archived["archive"]["projectId"], json!(project_id));
    assert_eq!(archived["archive"]["truncated"], false);
    assert_eq!(archived["job"]["sourceTurnId"], json!(mira_turn_id));
    assert_eq!(archived["job"]["state"], "pending");
    let archive_root = temporary.path().join("data").join("memory-archives");
    assert!(
        archive_root
            .join(archived["archive"]["archiveName"].as_str().unwrap())
            .is_file(),
        "the compressed source archive is on disk: {archived}"
    );
    // The same read the worker makes: it refuses a file whose size, zstd payload, or SHA-256
    // differs from the recorded row, so this is the archive's integrity check, not a citation.
    let archived_source: serde_json::Value = serde_json::from_slice(
        &harness
            .store
            .read_memory_source(&archive_root, mira_turn_id)
            .expect("the archive verifies against its recorded hash and sizes"),
    )
    .unwrap();
    assert_eq!(archived_source["id"], json!(mira_turn_id));
    assert_eq!(archived_source["taskId"], json!(mira_task));
    let summary_text = "Mira fixed the parser in 0123abcd; cargo test -p parser passed";
    let mut summary_events = completed_turn_events("completed");
    if let Some(ProviderEvent::TurnCompleted { output, .. }) = summary_events.last_mut() {
        *output = Some(
            json!({"records": [{"type": "achievement", "text": summary_text, "confidence": 0.9, "importance": 40}]})
                .to_string(),
        );
    }
    *harness.backend.turn_events.lock().unwrap() = summary_events;
    assert_eq!(
        restarted.summarize_next_memory_job_once().await,
        Ok(true),
        "the worker admits and completes the one pending job"
    );
    let summary_request = harness
        .backend
        .turn_requests
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert!(summary_request.read_only, "the summary turn is read-only");
    assert!(
        summary_request
            .prompt
            .starts_with("[LAZOO_INTERNAL_MEMORY_SUMMARY]"),
        "{}",
        summary_request.prompt
    );
    let queue = restarted
        .handle(request(
            "memory-queue",
            "memory.queue.status",
            json!({"projectId": project_id, "limit": 20}),
        ))
        .await
        .result
        .unwrap();
    assert_eq!(queue["jobs"][0]["state"], "completed", "{queue}");
    assert_eq!(queue["archiveErrorCount"], 0);
    let sessions = restarted
        .handle(request("sessions-after-summary", "session.list", json!({})))
        .await
        .result
        .unwrap();
    let sage_sessions: Vec<&serde_json::Value> = sessions
        .as_array()
        .unwrap()
        .iter()
        .filter(|session| session["employeeId"] == json!(sage.id))
        .collect();
    assert_eq!(
        sage_sessions.len(),
        1,
        "the summary ran on one fresh session for Sage: {sessions}"
    );
    assert_eq!(
        sage_sessions[0]["state"], "suspended",
        "Sage's summary session is stopped after use, never left idle"
    );
    let found = restarted
        .handle(request(
            "memory-search",
            "memory.search",
            json!({"projectId": project_id, "query": "", "taskId": mira_task, "limit": 10}),
        ))
        .await
        .result
        .unwrap();
    let record = found
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["sourceTurnId"] == json!(mira_turn_id))
        .cloned()
        .unwrap_or_else(|| panic!("a typed record cites Mira's dispatch turn: {found}"));
    assert_eq!(record["sourceHash"], archived["archive"]["sourceHash"]);
    assert_eq!(record["recordType"], "achievement");
    assert_eq!(record["active"], true);
    assert_eq!(record["text"], summary_text);
    assert_desktop_is_roster_neutral(&restarted).await;
}

#[test]
fn core_scenario_acceptance_contract_maps_every_step_to_evidence() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/core-scenario.ps1");
    let script = std::fs::read_to_string(&script_path).expect("core scenario acceptance script");

    for required in [
        "the_core_scenario_runs_end_to_end_on_the_deterministic_provider",
        "project_task_dispatch_uses_an_isolated_worktree_and_preserves_dirty_user_files",
        "message_restart_reconciles_an_uncertain_attempt",
        "store_enables_wal_foreign_keys_and_schema_version",
        "step1",
        "step2",
        "step3",
        "step4",
        "step5",
        "step6",
        "step7",
        "step8",
        "step9",
        "step10",
        "step11",
        "step12",
        "step13",
        "step14",
        "step15",
        "PARTIAL",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// The desktop's sidebar and launcher through the daemon: Home and a project are separate
/// spaces, each row carries a derived title and marker that follow the conversation's own
/// rows, the remembered lead is the newest conversation's, the launcher says who can lead and
/// why not, and nothing is stored or added to the desktop snapshot.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One user session over Home, a project, and the launcher.
async fn the_user_browses_a_space_and_its_launcher_through_the_daemon() {
    use lazoo_protocol::TaskState;

    let (temporary, harness) = tool_harness(AuthorityMode::Ask);
    let call = |id: &str, method: &str, params: serde_json::Value| {
        harness.app.handle(request(id, method, params))
    };

    // Home holds the harness conversation; a registered project holds one more.
    let repository = temporary.path().join("repository");
    std::fs::create_dir_all(&repository).unwrap();
    git_init(&repository);
    let registered = call(
        "register",
        "project.register",
        json!({"path": repository, "idempotencyKey": "workspace-project"}),
    )
    .await
    .result
    .unwrap();
    let project_id = registered["project"]["id"].clone();
    let project_conversation = call(
        "create-project-conversation",
        "conversation.create",
        json!({
            "projectId": project_id,
            "leadEmployeeId": harness.worker.id,
            "title": "Release checklist",
            "idempotencyKey": "workspace-project-conversation"
        }),
    )
    .await
    .result
    .unwrap()["conversation"]["id"]
        .clone();

    // Home before any turn: one untitled, unmarked row led by Rhea.
    let home = call("home", "workspace.conversations", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(home["version"], 1);
    assert_eq!(home["projectId"], serde_json::Value::Null);
    let rows = home["conversations"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], json!(harness.conversation_id));
    assert_eq!(rows[0]["title"], "New work");
    assert_eq!(rows[0]["titleDerived"], true);
    assert_eq!(rows[0]["leadName"], "Rhea");
    assert_eq!(rows[0]["provider"], "codex");
    assert_eq!(rows[0]["marker"], serde_json::Value::Null);
    assert_eq!(rows[0]["unresolved"], 0);
    assert_eq!(home["rememberedLeadId"], json!(harness.lead.id));
    assert_eq!(home["truncated"], false);

    // The project space is separate, keeps its stored title, and remembers Mira.
    let project = call(
        "project",
        "workspace.conversations",
        json!({"projectId": project_id}),
    )
    .await
    .result
    .unwrap();
    let rows = project["conversations"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], project_conversation);
    assert_eq!(rows[0]["title"], "Release checklist");
    assert_eq!(rows[0]["titleDerived"], false);
    assert_eq!(project["rememberedLeadId"], json!(harness.worker.id));

    // The lead's turn delegates under ask: the title derives from the request and the proposal
    // makes the conversation need attention.
    warm_tool_lead(&harness).await;
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
                "rosterRevision": roster_revision
            }),
        )]);
    let submitted = submit_lead_turn(&harness, "ask").await;
    assert_eq!(submitted["turn"]["state"], "completed");
    let child_id: uuid::Uuid = harness.backend.tool_results.lock().unwrap()[0].content["taskId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let home = call("home-proposed", "workspace.conversations", json!({}))
        .await
        .result
        .unwrap();
    let row = home["conversations"][0].clone();
    assert_eq!(row["title"], "Coordinate the parser fix");
    assert_eq!(row["titleDerived"], true);
    assert_eq!(row["marker"], "needs-attention");
    assert_eq!(row["markerReason"], "1 item needs you");
    assert_eq!(row["unresolved"], 1);
    assert_eq!(
        row["workers"],
        json!({"total": 1, "running": 0, "waiting": 1, "failed": 0, "completed": 0})
    );
    assert_eq!(row["rootTaskState"], "running");
    assert_eq!(row["lastTurnState"], "completed");

    // Accepting makes it active (waiting to run); dispatch completes it; the result reaching
    // the lead completes the plan.
    call(
        "accept",
        "task.accept",
        json!({"id": child_id, "expectedRevision": 1, "idempotencyKey": "workspace-accept"}),
    )
    .await
    .result
    .unwrap();
    let row = call("home-accepted", "workspace.conversations", json!({}))
        .await
        .result
        .unwrap()["conversations"][0]
        .clone();
    assert_eq!(row["marker"], "active");
    assert_eq!(row["markerReason"], "1 waiting to run");
    harness.app.dispatch_pending_tasks_once().await.unwrap();
    assert_eq!(
        harness.store.get_task(child_id).unwrap().task.state,
        TaskState::Completed
    );
    harness.app.deliver_pending_notices_once().await.unwrap();
    let row = call("home-done", "workspace.conversations", json!({}))
        .await
        .result
        .unwrap()["conversations"][0]
        .clone();
    assert_eq!(row["marker"], "completed");
    assert_eq!(row["rootTaskState"], "completed");
    assert_eq!(row["workers"]["completed"], 1);
    assert!(row["lastResultAt"].is_string());

    // A rename is the stored title from then on.
    call(
        "rename",
        "conversation.rename",
        json!({"id": harness.conversation_id, "expectedRevision": 1, "title": "Parser fix"}),
    )
    .await
    .result
    .unwrap();
    let row = call("home-renamed", "workspace.conversations", json!({}))
        .await
        .result
        .unwrap()["conversations"][0]
        .clone();
    assert_eq!(row["title"], "Parser fix");
    assert_eq!(row["titleDerived"], false);

    // Paging: a one-row page is truncated with a cursor; an unknown project is refused.
    let second = call(
        "create-second",
        "conversation.create",
        json!({
            "projectId": null,
            "leadEmployeeId": harness.lead.id,
            "title": "Second",
            "idempotencyKey": "workspace-second"
        }),
    )
    .await
    .result
    .unwrap()["conversation"]["id"]
        .clone();
    let page = call("page-1", "workspace.conversations", json!({"limit": 1}))
        .await
        .result
        .unwrap();
    assert_eq!(page["conversations"].as_array().unwrap().len(), 1);
    assert_eq!(page["conversations"][0]["id"], second, "newest first");
    assert_eq!(page["truncated"], true);
    let next = call(
        "page-2",
        "workspace.conversations",
        json!({"limit": 1, "before": page["nextBefore"]}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(
        next["conversations"][0]["id"],
        json!(harness.conversation_id)
    );
    assert_eq!(next["truncated"], false);
    for (label, params, code) in [
        ("bad-limit", json!({"limit": 0}), "LZ-WORKSPACE-INVALID"),
        (
            "bad-cursor",
            json!({"before": "nope"}),
            "LZ-WORKSPACE-INVALID",
        ),
        (
            "bad-project",
            json!({"projectId": uuid::Uuid::new_v4()}),
            "LZ-WORKSPACE-PROJECT-NOT-FOUND",
        ),
    ] {
        let error = call(label, "workspace.conversations", params)
            .await
            .error
            .unwrap();
        assert_eq!(error.code, code, "{label}");
    }

    // The launcher: both employees can lead, Rhea is remembered for Home and Mira for the
    // project; disabling Mira blocks her with Lazoo's words and the project's remembered lead
    // says why it no longer works.
    let launcher = call("launcher", "workspace.launcher", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(launcher["version"], 1);
    assert_eq!(launcher["project"], serde_json::Value::Null);
    assert_eq!(launcher["rememberedLeadId"], json!(harness.lead.id));
    assert_eq!(
        launcher["rememberedLeadBlockingReason"],
        serde_json::Value::Null
    );
    assert_eq!(
        launcher["authorityModes"],
        json!(["ask", "self-decision", "full-autonomy"])
    );
    let leads = launcher["leads"].as_array().unwrap();
    assert_eq!(leads.len(), 2);
    assert_eq!(leads[0]["name"], "Mira");
    assert_eq!(leads[1]["name"], "Rhea");
    assert!(
        leads.iter().all(|lead| lead["startable"] == true),
        "{leads:?}"
    );
    assert_eq!(leads[1]["availability"], "idle");
    assert_eq!(launcher["start"][0]["method"], "conversation.create");
    assert_eq!(launcher["start"][1]["method"], "turn.submit");
    assert_eq!(launcher["start"][1]["params"]["activateIfNeeded"], true);
    let mira = harness.store.get_employee(harness.worker.id).unwrap();
    call(
        "disable-mira",
        "employee.disable",
        json!({
            "id": mira.id,
            "expectedRevision": mira.configuration_revision,
            "idempotencyKey": "workspace-disable-mira"
        }),
    )
    .await
    .result
    .unwrap();
    let launcher = call(
        "launcher-project",
        "workspace.launcher",
        json!({"projectId": project_id}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(launcher["project"]["id"], project_id);
    assert_eq!(launcher["rememberedLeadId"], json!(harness.worker.id));
    assert_eq!(launcher["rememberedLeadBlockingReason"], "Mira is disabled");
    let mira_lead = launcher["leads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|lead| lead["employeeId"] == json!(harness.worker.id))
        .cloned()
        .unwrap();
    assert_eq!(mira_lead["startable"], false);
    assert_eq!(mira_lead["blockingReason"], "Mira is disabled");
    let refused = call(
        "launcher-bad",
        "workspace.launcher",
        json!({"projectId": uuid::Uuid::new_v4()}),
    )
    .await
    .error
    .unwrap();
    assert_eq!(refused.code, "LZ-WORKSPACE-PROJECT-NOT-FOUND");

    // Nothing was stored for the workspace, and the snapshot is unchanged.
    assert!(
        harness
            .store
            .list_events(0, 1000)
            .unwrap()
            .iter()
            .all(|event| !event.event_type.starts_with("workspace"))
    );
    let snapshot = call("desktop", "desktop.snapshot", json!({}))
        .await
        .result
        .unwrap();
    assert!(snapshot.get("workspace").is_none());
    assert!(snapshot.get("conversations").is_none());
    assert_desktop_is_roster_neutral(&harness.app).await;
}

#[test]
fn workspace_read_models_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/workspace-read-models.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("workspace read models acceptance script");

    for required in [
        "the_marker_prefers_attention_then_activity_then_failure_then_completion",
        "titles_come_from_the_store_then_the_first_request_then_a_default",
        "startability_follows_roster_availability_with_lazoo_words_for_each_block",
        "workspace_cli_maps_conversations_and_launcher",
        "the_user_browses_a_space_and_its_launcher_through_the_daemon",
        "store_enables_wal_foreign_keys_and_schema_version",
        "markersFollowTheRows",
        "rememberedLeadIsTheNewest",
        "launcherExplainsBlocks",
        "workspaceStoresNothing",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// The active conversation through the daemon: the lead header's lifecycle follows the lead's
/// session, the root task and its authority appear with the first turn, each worker card takes
/// the dock state its rows support with Lazoo's words, the conversation's own Inbox items are
/// embedded, the turns are bounded, and nothing is stored or added to the desktop snapshot.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One user session over a conversation's whole life.
async fn the_user_opens_the_conversation_header_and_worker_dock_through_the_daemon() {
    let (_temporary, harness) = tool_harness(AuthorityMode::Ask);
    let call = |id: &str, method: &str, params: serde_json::Value| {
        harness.app.handle(request(id, method, params))
    };
    let show = |id: &'static str| async move {
        call(
            id,
            "workspace.conversation",
            json!({"id": harness.conversation_id}),
        )
        .await
        .result
        .unwrap()
    };

    // Before anything: a dormant lead, no root task, no workers, nothing to attend to.
    let view = show("fresh").await;
    assert_eq!(view["version"], 1);
    assert_eq!(view["conversation"]["id"], json!(harness.conversation_id));
    assert_eq!(view["title"], "New work");
    assert_eq!(view["titleDerived"], true);
    assert_eq!(view["scope"]["projectId"], serde_json::Value::Null);
    assert_eq!(view["scope"]["projectName"], serde_json::Value::Null);
    assert_eq!(view["lead"]["employeeId"], json!(harness.lead.id));
    assert_eq!(view["lead"]["name"], "Rhea");
    assert_eq!(view["lead"]["role"], "Lead");
    assert_eq!(view["lead"]["provider"], "codex");
    assert_eq!(view["lead"]["model"], "test-model");
    assert_eq!(view["lead"]["availability"], "dormant");
    assert_eq!(view["lead"]["lifecycle"], "dormant");
    assert_eq!(view["lead"]["session"], serde_json::Value::Null);
    assert_eq!(view["lead"]["authority"], serde_json::Value::Null);
    assert_eq!(view["lead"]["activeTurnId"], serde_json::Value::Null);
    assert_eq!(view["rootTask"], serde_json::Value::Null);
    assert_eq!(view["turns"], json!([]));
    assert_eq!(view["turnsTruncated"], false);
    assert_eq!(view["workers"], json!([]));
    assert_eq!(view["attention"], json!([]));
    assert!(view["eventHighWater"].is_u64());

    // Bad parameters and an unknown conversation are refused with stable codes.
    for (label, params, code) in [
        (
            "bad-limit",
            json!({"id": harness.conversation_id, "turnLimit": 0}),
            "LZ-WORKSPACE-INVALID",
        ),
        (
            "bad-field",
            json!({"id": harness.conversation_id, "nope": 1}),
            "LZ-WORKSPACE-INVALID",
        ),
        (
            "unknown",
            json!({"id": uuid::Uuid::new_v4()}),
            "LZ-CONVERSATION-NOT-FOUND",
        ),
    ] {
        let error = call(label, "workspace.conversation", params)
            .await
            .error
            .unwrap();
        assert_eq!(error.code, code, "{label}");
    }

    // A warmed lead is idle, with its session and private working directory in the header.
    warm_tool_lead(&harness).await;
    let view = show("warmed").await;
    assert_eq!(view["lead"]["availability"], "idle");
    assert_eq!(view["lead"]["lifecycle"], "idle");
    assert_eq!(view["lead"]["session"]["state"], "idle");
    let lead_session_id = view["lead"]["session"]["id"].clone();
    assert!(
        view["lead"]["session"]["workingDirectory"]
            .as_str()
            .unwrap()
            .contains(lead_session_id.as_str().unwrap())
    );

    // The lead's first turn delegates two tasks under ask: the root task carries the authority,
    // both workers are blocked on the user's acceptance, and both proposals are the attention.
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
        tool_call(
            "call-2",
            "task_delegate",
            json!({
                "employeeId": harness.worker.id,
                "objective": "Write the parser tests\nand run them",
                "rosterRevision": roster_revision
            }),
        ),
    ]);
    let submitted = submit_lead_turn(&harness, "ask").await;
    assert_eq!(submitted["turn"]["state"], "completed");
    let child_ids: Vec<uuid::Uuid> = harness.backend.tool_results.lock().unwrap()[..2]
        .iter()
        .map(|result| result.content["taskId"].as_str().unwrap().parse().unwrap())
        .collect();
    let view = show("proposed").await;
    assert_eq!(view["title"], "Coordinate the parser fix");
    assert_eq!(view["lead"]["lifecycle"], "idle");
    assert_eq!(view["lead"]["authority"], "ask");
    assert_eq!(view["lead"]["activeTurnId"], serde_json::Value::Null);
    assert_eq!(view["rootTask"]["role"], "lead");
    assert_eq!(view["rootTask"]["state"], "running");
    assert_eq!(view["rootTask"]["effectiveAuthority"], "ask");
    let root_task_id = view["rootTask"]["id"].clone();
    let turns = view["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["state"], "completed");
    assert_eq!(turns[0]["recipient"]["employeeId"], json!(harness.lead.id));
    let workers = view["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 2, "{workers:?}");
    for (worker, child_id) in workers.iter().zip(&child_ids) {
        assert_eq!(worker["taskId"], json!(child_id));
        assert_eq!(worker["rootTaskId"], root_task_id);
        assert_eq!(worker["parentTaskId"], root_task_id);
        assert_eq!(worker["depth"], 1);
        assert_eq!(worker["employeeId"], json!(harness.worker.id));
        assert_eq!(worker["name"], "Mira");
        assert_eq!(worker["provider"], "codex");
        assert_eq!(worker["model"], "test-model");
        assert_eq!(worker["state"], "blocked");
        assert_eq!(worker["stateReason"], "Waiting for you to accept the task");
        assert_eq!(worker["taskState"], "proposed");
        assert_eq!(worker["effectiveAuthority"], "ask");
        assert_eq!(worker["session"], serde_json::Value::Null);
        assert_eq!(worker["workspace"], serde_json::Value::Null);
        assert_eq!(worker["terminalAt"], serde_json::Value::Null);
    }
    assert_eq!(workers[0]["label"], "Implement the parser fix");
    assert_eq!(workers[1]["label"], "Write the parser tests");
    let attention = view["attention"].as_array().unwrap();
    assert_eq!(attention.len(), 2, "{attention:?}");
    assert!(attention.iter().all(|item| item["kind"] == "task-proposal"
        && item["attention"] == "action-required"
        && item["scope"]["conversationId"] == json!(harness.conversation_id)));
    assert_eq!(attention[0]["actions"][0]["method"], "task.accept");

    // Accepting the first makes it wait for dispatch; cancelling the second as the user
    // interrupts it; only one proposal is left to attend to.
    call(
        "accept",
        "task.accept",
        json!({"id": child_ids[0], "expectedRevision": 1, "idempotencyKey": "dock-accept"}),
    )
    .await
    .result
    .unwrap();
    let view = show("accepted").await;
    assert_eq!(view["workers"][0]["state"], "waiting");
    assert_eq!(
        view["workers"][0]["stateReason"],
        "Waiting to be dispatched when the worker is free"
    );
    assert_eq!(view["workers"][0]["taskState"], "accepted");
    assert_eq!(view["attention"].as_array().unwrap().len(), 1);
    call(
        "cancel",
        "task.cancel",
        json!({
            "id": child_ids[1],
            "expectedRevision": 1,
            "actor": {"kind": "user"},
            "idempotencyKey": "dock-cancel"
        }),
    )
    .await
    .result
    .unwrap();
    let view = show("cancelled").await;
    assert_eq!(view["workers"][1]["state"], "interrupted");
    assert_eq!(
        view["workers"][1]["stateReason"],
        "Interrupted: the task was cancelled"
    );
    assert_eq!(view["workers"][1]["taskState"], "cancelled");
    assert!(view["workers"][1]["terminalAt"].is_string());
    assert_eq!(view["attention"], json!([]));

    // Dispatch runs the first worker to completion on its own workspace; the worker's turn
    // joins the conversation's turns, which a small limit bounds newest-first.
    harness.app.dispatch_pending_tasks_once().await.unwrap();
    let view = show("dispatched").await;
    let worker = view["workers"][0].clone();
    assert_eq!(worker["state"], "complete");
    assert_eq!(worker["stateReason"], "Completed");
    assert_eq!(worker["taskState"], "completed");
    assert_eq!(worker["workspace"]["kind"], "projectless");
    assert!(worker["workspace"]["path"].is_string());
    // A terminal task holds no active session or turn any more; its turn is in the list.
    assert_eq!(worker["session"], serde_json::Value::Null);
    assert!(worker["terminalAt"].is_string());
    let turns = view["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1]["taskId"], json!(child_ids[0]));
    assert_eq!(
        turns[1]["recipient"]["employeeId"],
        json!(harness.worker.id)
    );
    assert_eq!(view["turnsTruncated"], false);
    let bounded = call(
        "bounded",
        "workspace.conversation",
        json!({"id": harness.conversation_id, "turnLimit": 1}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(bounded["turns"].as_array().unwrap().len(), 1);
    assert_eq!(bounded["turns"][0]["id"], turns[1]["id"], "the newest turn");
    assert_eq!(bounded["turnsTruncated"], true);

    // The result reaching the lead completes the plan; stopping the lead suspends the header.
    harness.app.deliver_pending_notices_once().await.unwrap();
    let view = show("done").await;
    assert_eq!(view["rootTask"]["state"], "completed");
    assert_eq!(view["lead"]["lifecycle"], "idle");
    let lead_session = harness
        .store
        .get_session(lead_session_id.as_str().unwrap().parse().unwrap())
        .unwrap();
    call(
        "stop",
        "session.stop",
        json!({
            "id": lead_session.id,
            "expectedRevision": lead_session.revision,
            "idempotencyKey": "dock-stop"
        }),
    )
    .await
    .result
    .unwrap();
    let view = show("stopped").await;
    assert_eq!(view["lead"]["lifecycle"], "suspended");
    assert_eq!(view["lead"]["availability"], "suspended");
    assert_eq!(view["lead"]["session"]["state"], "suspended");

    // Nothing was stored for the workspace, and the snapshot is unchanged.
    assert!(
        harness
            .store
            .list_events(0, 1000)
            .unwrap()
            .iter()
            .all(|event| !event.event_type.starts_with("workspace"))
    );
    let snapshot = call("desktop", "desktop.snapshot", json!({}))
        .await
        .result
        .unwrap();
    assert!(snapshot.get("workspace").is_none());
    assert!(snapshot.get("lead").is_none());
    assert_desktop_is_roster_neutral(&harness.app).await;
}

#[test]
fn workspace_conversation_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/workspace-conversation.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("workspace conversation acceptance script");

    for required in [
        "the_lead_lifecycle_follows_the_session_and_its_newest_turn",
        "worker_cards_take_the_first_state_that_holds_with_lazoo_words",
        "workspace_cli_maps_show",
        "the_user_opens_the_conversation_header_and_worker_dock_through_the_daemon",
        "the_desktop_resumes_every_read_model_from_its_event_high_water_without_loss",
        "store_enables_wal_foreign_keys_and_schema_version",
        "lifecycleFollowsTheSession",
        "marksResumeWithoutLoss",
        "workerCardsFollowTheRows",
        "attentionIsEmbedded",
        "workspaceStoresNothing",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// The header and each worker card name the actions their rows allow right now, as data for
/// an existing revision-checked RPC, and sending one exactly as named (plus a fresh
/// idempotency key) is accepted: accept a proposal from its card, rename from the header,
/// and stop the idle lead from the header. Nothing is stored for the actions.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One user session acting from the header and a card.
async fn the_user_acts_through_the_actions_the_header_and_cards_name() {
    let (_temporary, harness) = tool_harness(AuthorityMode::Ask);
    let call = |id: &str, method: &str, params: serde_json::Value| {
        harness.app.handle(request(id, method, params))
    };
    let show = |id: &'static str| async move {
        call(
            id,
            "workspace.conversation",
            json!({"id": harness.conversation_id}),
        )
        .await
        .result
        .unwrap()
    };
    let names = |actions: &serde_json::Value| {
        actions
            .as_array()
            .unwrap()
            .iter()
            .map(|action| action["action"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    // Send an action exactly as the daemon named it, with the caller's idempotency key.
    let send = |label: &str, action: serde_json::Value, extra: serde_json::Value| {
        let mut params = action["params"].clone();
        params["idempotencyKey"] = json!(format!("action-{label}"));
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        call(label, action["method"].as_str().unwrap(), params)
    };

    // A dormant lead offers only rename; a warmed one may also be stopped.
    let view = show("fresh").await;
    assert_eq!(names(&view["lead"]["actions"]), ["rename"]);
    warm_tool_lead(&harness).await;
    let view = show("warmed").await;
    assert_eq!(names(&view["lead"]["actions"]), ["rename", "stop-session"]);

    // A proposal's card offers accept and cancel; sending accept as named is accepted and the
    // card then offers only cancel.
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
                "rosterRevision": roster_revision
            }),
        )]);
    assert_eq!(
        submit_lead_turn(&harness, "ask").await["turn"]["state"],
        "completed"
    );
    let view = show("proposed").await;
    assert_eq!(names(&view["lead"]["actions"]), ["rename", "stop-session"]);
    let card = view["workers"][0].clone();
    assert_eq!(names(&card["actions"]), ["accept", "cancel"]);
    assert_eq!(card["actions"][0]["method"], "task.accept");
    assert_eq!(card["actions"][0]["params"]["expectedRevision"], 1);
    assert_eq!(
        card["actions"][1]["params"]["actor"],
        json!({"kind": "user"})
    );
    let accepted = send("accept", card["actions"][0].clone(), json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(accepted["task"]["state"], "accepted");
    let view = show("accepted").await;
    assert_eq!(names(&view["workers"][0]["actions"]), ["cancel"]);
    assert_eq!(
        view["workers"][0]["actions"][0]["params"]["expectedRevision"],
        2
    );

    // Rename from the header, with the caller's title; the next header cites the new revision.
    let rename = view["lead"]["actions"][0].clone();
    assert_eq!(rename["method"], "conversation.rename");
    assert_eq!(rename["params"]["title"], serde_json::Value::Null);
    send("rename", rename, json!({"title": "Dock"}))
        .await
        .result
        .unwrap();
    let view = show("renamed").await;
    assert_eq!(view["title"], "Dock");
    assert_eq!(view["titleDerived"], false);
    assert_eq!(view["lead"]["actions"][0]["params"]["expectedRevision"], 2);

    // A completed card offers nothing; stopping the lead from the header suspends it and
    // leaves rename alone.
    harness.app.dispatch_pending_tasks_once().await.unwrap();
    harness.app.deliver_pending_notices_once().await.unwrap();
    let view = show("done").await;
    assert_eq!(view["workers"][0]["state"], "complete");
    assert_eq!(view["workers"][0]["actions"], json!([]));
    assert_eq!(view["rootTask"]["state"], "completed");
    let stop = view["lead"]["actions"][1].clone();
    assert_eq!(stop["action"], "stop-session");
    assert_eq!(stop["method"], "session.stop");
    assert_eq!(stop["params"]["id"], view["lead"]["session"]["id"]);
    send("stop", stop, json!({})).await.result.unwrap();
    let view = show("stopped").await;
    assert_eq!(view["lead"]["lifecycle"], "suspended");
    assert_eq!(names(&view["lead"]["actions"]), ["rename"]);

    // The actions are derived: no event names the workspace.
    assert!(
        harness
            .store
            .list_events(0, 1000)
            .unwrap()
            .iter()
            .all(|event| !event.event_type.starts_with("workspace"))
    );
}

#[test]
fn workspace_actions_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/workspace-actions.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("workspace actions acceptance script");

    for required in [
        "cards_and_the_header_offer_only_the_actions_their_rows_allow",
        "the_user_acts_through_the_actions_the_header_and_cards_name",
        "the_user_opens_the_conversation_header_and_worker_dock_through_the_daemon",
        "store_enables_wal_foreign_keys_and_schema_version",
        "actionsFollowTheRows",
        "namedActionsAreAccepted",
        "workspaceStoresNothing",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// The migration preview through the daemon: over the same fixture profile the discovery
/// reads, every candidate names what Lazoo would do and why, no secret value appears, the
/// method takes no parameters, nothing is written, and the desktop snapshot is unchanged.
#[tokio::test]
async fn provider_config_preview_proposes_manage_merge_and_choose_without_writing() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let home = temporary.path().join("home");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    let codex = home.join(".codex").join("config.toml");
    std::fs::write(
        &codex,
        "[mcp_servers.context7]\nurl = \"https://mcp.context7.com/mcp\"\n[mcp_servers.context7.http_headers]\nCONTEXT7_API_KEY = \"ctx7sk-SECRET\"\n[mcp_servers.paper]\nurl = \"http://127.0.0.1:29979/mcp\"\n",
    )
    .unwrap();
    let claude = home.join(".claude.json");
    std::fs::write(
        &claude,
        r#"{"mcpServers": {"context7": {"type": "http", "url": "https://mcp.context7.com/mcp"}, "paper": {"type": "http", "url": "http://127.0.0.1:31000/mcp"}, "obsidian": {"command": "npx", "args": ["obsidian-mcp"]}}}"#,
    )
    .unwrap();
    let before = (
        std::fs::read_to_string(&codex).unwrap(),
        std::fs::read_to_string(&claude).unwrap(),
    );
    let store = Store::open_path(root.state_db()).unwrap();
    let app = App::new_with_provider_environment(
        store,
        root,
        DiscoveryEnvironment::empty().with_config_home(&home),
    )
    .unwrap();

    let preview = app
        .handle(request("preview", "provider.config.preview", json!({})))
        .await
        .result
        .unwrap();
    assert_eq!(preview["version"], 1);
    assert_eq!(
        preview["providers"],
        json!(["codex", "opencode", "claude-code"])
    );
    let candidates = preview["candidates"].as_array().unwrap();
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| (
                candidate["name"].as_str().unwrap(),
                candidate["proposal"].as_str().unwrap()
            ))
            .collect::<Vec<_>>(),
        [
            ("context7", "merge"),
            ("obsidian", "manage"),
            ("paper", "choose"),
            ("paper", "choose")
        ]
    );
    assert_eq!(candidates[0]["presentIn"], json!(["codex", "claude-code"]));
    assert_eq!(candidates[0]["missingFrom"], json!(["opencode"]));
    assert_eq!(candidates[0]["secretKeys"], json!(["CONTEXT7_API_KEY"]));
    assert_eq!(candidates[0]["sources"].as_array().unwrap().len(), 2);
    assert_eq!(candidates[1]["presentIn"], json!(["claude-code"]));
    assert_eq!(candidates[2]["conflictsWith"].as_array().unwrap().len(), 1);
    assert_eq!(
        preview["summary"],
        json!({"manage": 1, "merge": 1, "choose": 2, "unsupported": 2})
    );
    assert!(!preview.to_string().contains("SECRET"));

    let refused = app
        .handle(request(
            "preview-params",
            "provider.config.preview",
            json!({"provider": "codex"}),
        ))
        .await
        .error
        .unwrap();
    assert_eq!(refused.code, "LZ-INVALID-PARAMS");

    // Nothing was written or stored, and the snapshot is unchanged.
    assert_eq!(std::fs::read_to_string(&codex).unwrap(), before.0);
    assert_eq!(std::fs::read_to_string(&claude).unwrap(), before.1);
    assert!(
        app.handle(request("events", "events.list", json!({})))
            .await
            .result
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    let snapshot = app
        .handle(request("desktop", "desktop.snapshot", json!({})))
        .await
        .result
        .unwrap();
    assert!(snapshot.get("preview").is_none());
    assert!(snapshot.get("providerConfig").is_none());
}

#[test]
fn provider_migration_preview_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/provider-migration-preview.ps1");
    let script = std::fs::read_to_string(&script_path)
        .expect("provider migration preview acceptance script");

    for required in [
        "the_preview_proposes_manage_merge_or_choose_per_identity_and_never_reads_a_secret",
        "provider_cli_maps_preview",
        "provider_config_preview_proposes_manage_merge_and_choose_without_writing",
        "store_enables_wal_foreign_keys_and_schema_version",
        "proposalsFollowIdentity",
        "secretsNeverRead",
        "nothingWritten",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}

/// The desktop's live-update contract (workspace contract section 5, Inbox contract): every
/// read model reports an `eventHighWater` such that `events.list { after }` from it is empty
/// right after the read and carries every later change, so a subscriber resuming from the
/// mark loses nothing; after each change the re-read model reflects it and its new mark
/// covers the events seen.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One subscriber resuming four models across five changes.
async fn the_desktop_resumes_every_read_model_from_its_event_high_water_without_loss() {
    let (_temporary, harness) = tool_harness(AuthorityMode::Ask);
    let call = |id: &str, method: &str, params: serde_json::Value| {
        harness.app.handle(request(id, method, params))
    };
    let conversation_id = harness.conversation_id;
    let models: [(&str, &str, serde_json::Value); 4] = [
        ("inbox", "inbox.counts", json!({})),
        ("conversations", "workspace.conversations", json!({})),
        ("launcher", "workspace.launcher", json!({})),
        (
            "conversation",
            "workspace.conversation",
            json!({"id": conversation_id}),
        ),
    ];
    let models = &models;
    let read_marks = |label: &'static str| async move {
        let mut marks = Vec::new();
        for (name, method, params) in models {
            let view = call(&format!("{label}-{name}"), method, params.clone())
                .await
                .result
                .unwrap();
            let mark = view["eventHighWater"].as_u64().unwrap();
            // Nothing is pending right after a read: the mark is never ahead of the ledger.
            let pending = call(
                &format!("{label}-{name}-pending"),
                "events.list",
                json!({"after": mark, "limit": 100}),
            )
            .await
            .result
            .unwrap();
            assert_eq!(pending, json!([]), "{label}: {name} mark {mark} is ahead");
            marks.push((*name, mark));
        }
        marks
    };
    let events_after = |label: &'static str, mark: u64| async move {
        call(label, "events.list", json!({"after": mark, "limit": 200}))
            .await
            .result
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|event| {
                (
                    event["sequence"].as_u64().unwrap(),
                    event["eventType"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<Vec<_>>()
    };

    // Change 1: the lead warms (session and roster events reach the launcher and the header).
    let marks = read_marks("before-warm").await;
    warm_tool_lead(&harness).await;
    for (name, mark) in &marks {
        let seen = events_after("after-warm", *mark).await;
        assert!(
            seen.iter().any(|(_, kind)| kind.starts_with("session.")),
            "{name}: {seen:?}"
        );
    }
    let view = call(
        "warmed",
        "workspace.conversation",
        json!({"id": conversation_id}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(view["lead"]["lifecycle"], "idle");
    let launcher = call("warmed-launcher", "workspace.launcher", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(launcher["leads"][1]["availability"], "idle");

    // Change 2: the lead's turn delegates under ask (turn and task events reach every model,
    // and the proposal reaches the Inbox count).
    let marks = read_marks("before-turn").await;
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
                "rosterRevision": roster_revision
            }),
        )]);
    assert_eq!(
        submit_lead_turn(&harness, "ask").await["turn"]["state"],
        "completed"
    );
    let child_id: uuid::Uuid = harness.backend.tool_results.lock().unwrap()[0].content["taskId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    for (name, mark) in &marks {
        let seen = events_after("after-turn", *mark).await;
        assert!(
            seen.iter().any(|(_, kind)| kind.starts_with("turn."))
                && seen.iter().any(|(_, kind)| kind.starts_with("task.")),
            "{name}: {seen:?}"
        );
    }
    let counts = call("counts-proposed", "inbox.counts", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(counts["counts"]["unresolved"], 1);
    let rows = call("rows-proposed", "workspace.conversations", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(rows["conversations"][0]["marker"], "needs-attention");

    // Change 3: the user accepts (a task event); change 4: renames (a conversation event);
    // change 5: disables Mira (employee and roster events). Each model's re-read reflects
    // every change and its new mark covers everything seen.
    let marks = read_marks("before-actions").await;
    call(
        "accept",
        "task.accept",
        json!({"id": child_id, "expectedRevision": 1, "idempotencyKey": "resume-accept"}),
    )
    .await
    .result
    .unwrap();
    call(
        "rename",
        "conversation.rename",
        json!({"id": conversation_id, "expectedRevision": 1, "title": "Resumed"}),
    )
    .await
    .result
    .unwrap();
    let mira = harness.store.get_employee(harness.worker.id).unwrap();
    call(
        "disable",
        "employee.disable",
        json!({
            "id": mira.id,
            "expectedRevision": mira.configuration_revision,
            "idempotencyKey": "resume-disable"
        }),
    )
    .await
    .result
    .unwrap();
    for (name, mark) in &marks {
        let seen = events_after("after-actions", *mark).await;
        let kinds: Vec<&str> = seen.iter().map(|(_, kind)| kind.as_str()).collect();
        assert!(
            kinds.iter().any(|kind| kind.starts_with("task."))
                && kinds.iter().any(|kind| kind.starts_with("conversation."))
                && kinds.iter().any(|kind| kind.starts_with("employee.")),
            "{name}: {kinds:?}"
        );
        let newest = seen.iter().map(|(sequence, _)| *sequence).max().unwrap();
        let (method, params) = match *name {
            "inbox" => ("inbox.counts", json!({})),
            "conversations" => ("workspace.conversations", json!({})),
            "launcher" => ("workspace.launcher", json!({})),
            _ => ("workspace.conversation", json!({"id": conversation_id})),
        };
        let view = call(&format!("reread-{name}"), method, params)
            .await
            .result
            .unwrap();
        assert!(
            view["eventHighWater"].as_u64().unwrap() >= newest,
            "{name}: mark {} covers {newest}",
            view["eventHighWater"]
        );
        match *name {
            "inbox" => assert_eq!(view["counts"]["unresolved"], 0),
            "conversations" => {
                assert_eq!(view["conversations"][0]["title"], "Resumed");
                assert_eq!(view["conversations"][0]["marker"], "active");
            }
            "launcher" => assert_eq!(view["leads"][0]["blockingReason"], "Mira is disabled"),
            _ => {
                assert_eq!(view["title"], "Resumed");
                assert_eq!(view["workers"][0]["state"], "blocked");
                assert_eq!(
                    view["workers"][0]["stateReason"],
                    "Blocked: Mira is disabled"
                );
            }
        }
    }
    read_marks("settled").await;
}

/// The committed gate runner is the full gate every slice must pass: the four cargo steps in
/// order, every acceptance script except the live model turn and the separately owned
/// desktop, the real-binary scripts with `-SkipBuild`, and a manifest of every step.
#[test]
fn gate_runner_runs_the_whole_gate_and_excludes_only_the_live_and_ui_scripts() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = std::fs::read_to_string(repository.join("scripts/gate.ps1")).expect("gate runner");
    for required in [
        "cargo fmt --all -- --check",
        "cargo clippy --workspace --all-targets --locked -- -D warnings",
        "cargo test --workspace --locked",
        "cargo build --workspace --release --locked",
        "$excluded = @('provider-adapters', 'desktop-shell')",
        r"scripts\acceptance\*.ps1",
        "-SkipBuild",
        "-Provider",
        "gate-manifest.json",
    ] {
        assert!(
            script.contains(required),
            "gate runner must contain {required}"
        );
    }
    assert!(
        !script.contains("LivePrompt") && !script.contains("provider', 'smoke"),
        "the gate runner never runs a live model turn"
    );
}

/// Claude Code is the one other provider that enforces an authority ceiling natively, and only
/// `full-autonomy` (its prompt-free `auto` mode). A Codex lead's delegation to a Claude worker
/// under full autonomy is dispatched and completes on the worker's own session; the same
/// delegation to a Claude worker whose authority is `ask` is proposed, accepted by the user,
/// and then refused at dispatch with the task failed and an incident, as is an Antigravity
/// worker under full autonomy (no prompt-free mode); a Claude lead takes a user turn under
/// full autonomy and refuses one under `ask`. No support is claimed beyond this deterministic
/// path: a live gate per provider still has to run.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One story over every provider and authority pairing.
async fn claude_code_runs_turns_and_tasks_only_under_full_autonomy() {
    use lazoo_protocol::TaskState;
    use lazoo_store::ListIncidentsInput;

    let (_temporary, harness) = tool_harness(AuthorityMode::FullAutonomy);
    let call = |id: &str, method: &str, params: serde_json::Value| {
        harness.app.handle(request(id, method, params))
    };
    let employee = |name: &str, provider: &str, authority: AuthorityMode| CreateEmployeeInput {
        request_id: format!("claude-{name}-request"),
        idempotency_key: format!("claude-{name}"),
        name: name.to_owned(),
        provider: provider.to_owned(),
        role: "Worker".to_owned(),
        instructions: String::new(),
        model: None,
        reasoning_effort: None,
        native_settings: json!({}),
        authority_mode: authority,
        checkout_policy: EmployeeCheckoutPolicy::Isolated,
        activation_policy: lazoo_protocol::ActivationPolicy::Manual,
        idle_timeout_seconds: 0,
        enabled: true,
        provider_observation: Some(ProviderObservation {
            provider: provider.to_owned(),
            verification: "configured".to_owned(),
            version: Some("test".to_owned()),
            protocol: Some("test".to_owned()),
            capabilities: json!({"session": true}),
            authentication: json!({"native": "ready", "external": "unknown"}),
            degradation_reason: None,
            checked_at: "2026-10-13T00:00:00Z".to_owned(),
        }),
    };
    let cleo = harness
        .store
        .create_employee(&employee(
            "Cleo",
            "claude-code",
            AuthorityMode::FullAutonomy,
        ))
        .unwrap()
        .employee;
    let ava = harness
        .store
        .create_employee(&employee("Ava", "claude-code", AuthorityMode::Ask))
        .unwrap()
        .employee;
    let gus = harness
        .store
        .create_employee(&employee("Gus", "antigravity", AuthorityMode::FullAutonomy))
        .unwrap()
        .employee;

    // Rhea (Codex, full autonomy) delegates to all three inside her turn.
    warm_tool_lead(&harness).await;
    let roster_revision = harness.store.roster_snapshot().unwrap().revision;
    let delegate = |call_id: &str, employee: uuid::Uuid, objective: &str| {
        tool_call(
            call_id,
            "task_delegate",
            json!({"employeeId": employee, "objective": objective, "rosterRevision": roster_revision}),
        )
    };
    harness.backend.tool_calls.lock().unwrap().extend([
        delegate("call-1", cleo.id, "Port the parser to Claude"),
        // Ava's ceiling is `ask`, so the lead asks for `ask`; a wider request is refused.
        tool_call(
            "call-2",
            "task_delegate",
            json!({
                "employeeId": ava.id,
                "objective": "Review the port",
                "requestedAuthority": "ask",
                "rosterRevision": roster_revision
            }),
        ),
        delegate("call-3", gus.id, "Draft the release notes"),
    ]);
    assert_eq!(
        submit_lead_turn(&harness, "full-autonomy").await["turn"]["state"],
        "completed"
    );
    let results = harness.backend.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 3, "{results:?}");
    assert!(results.iter().all(|result| result.success), "{results:?}");
    let task_of = |index: usize| -> uuid::Uuid {
        results[index].content["taskId"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    };
    let (cleo_task, ava_task, gus_task) = (task_of(0), task_of(1), task_of(2));
    assert_eq!(results[0].content["effectiveAuthority"], "full-autonomy");
    assert_eq!(results[0].content["state"], "accepted");
    assert_eq!(results[1].content["effectiveAuthority"], "ask");
    assert_eq!(results[1].content["state"], "proposed");
    assert_eq!(results[2].content["effectiveAuthority"], "full-autonomy");
    assert_eq!(results[2].content["state"], "accepted");

    // Cleo's task runs on her own Claude Code session and completes; Gus's is refused at
    // dispatch because Antigravity has no prompt-free mode: the task fails with the refusal
    // and an incident names the incompatible configuration.
    let outcome = harness.app.dispatch_pending_tasks_once().await;
    assert_eq!(
        outcome.unwrap_err().code,
        "LZ-TASK-AUTHORITY-UNSUPPORTED",
        "the pass reports Gus's refusal after running Cleo"
    );
    let cleo_detail = harness.store.get_task(cleo_task).unwrap();
    assert_eq!(cleo_detail.task.state, TaskState::Completed);
    let cleo_session = harness
        .store
        .get_session(cleo_detail.dispatch.unwrap().session_id.unwrap())
        .unwrap();
    assert_eq!(cleo_session.provider, "claude-code");
    assert_eq!(cleo_session.employee_id, cleo.id);
    assert!(
        harness.backend.resume_ids.lock().unwrap().is_empty(),
        "a task dispatch starts its session in the task workspace and never resumes"
    );
    let gus_detail = harness.store.get_task(gus_task).unwrap();
    assert_eq!(gus_detail.task.state, TaskState::Failed);
    assert_eq!(
        gus_detail.task.failure_code.as_deref(),
        Some("LZ-TASK-AUTHORITY-UNSUPPORTED")
    );
    assert!(
        harness
            .store
            .list_incidents(&ListIncidentsInput {
                code: Some("LZ-CONFIG-INCOMPATIBLE".to_owned()),
                limit: 100,
                ..ListIncidentsInput::default()
            })
            .unwrap()
            .iter()
            .any(|incident| incident.task_id == Some(gus_task)),
        "the refused dispatch is an incident"
    );
    assert_eq!(
        harness.backend.starts.load(Ordering::SeqCst),
        2,
        "Rhea's session and Cleo's; nothing started for Ava or Gus"
    );

    // Ava's proposal is accepted by the user, then refused at dispatch: under `ask` Claude
    // Code's prompts stay inside its own stream, so Lazoo fails closed.
    call(
        "accept-ava",
        "task.accept",
        json!({"id": ava_task, "expectedRevision": 1, "idempotencyKey": "accept-ava"}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(
        harness
            .app
            .dispatch_pending_tasks_once()
            .await
            .unwrap_err()
            .code,
        "LZ-TASK-AUTHORITY-UNSUPPORTED"
    );
    assert_eq!(
        harness.store.get_task(ava_task).unwrap().task.state,
        TaskState::Failed
    );
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 2);

    // A Claude Code lead: a user turn under `ask` is refused before any provider I/O; one
    // under full autonomy activates her session and completes.
    let conversation: uuid::Uuid = call(
        "create-cleo-conversation",
        "conversation.create",
        json!({
            "projectId": null,
            "leadEmployeeId": cleo.id,
            "title": "Claude leads",
            "idempotencyKey": "cleo-conversation"
        }),
    )
    .await
    .result
    .unwrap()["conversation"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let submit = |id: &'static str, authority: &'static str| {
        call(
            id,
            "turn.submit",
            json!({
                "conversationId": conversation,
                "author": {"kind": "user"},
                "recipient": null,
                "authorityMode": authority,
                "text": "Summarize the parser",
                "activateIfNeeded": true,
                "maxAttempts": 1,
                "idempotencyKey": id
            }),
        )
    };
    let refused = timeout(Duration::from_secs(20), submit("cleo-ask", "ask"))
        .await
        .unwrap()
        .error
        .unwrap();
    assert_eq!(refused.code, "LZ-TASK-AUTHORITY-UNSUPPORTED");
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 2);
    let completed = timeout(
        Duration::from_secs(20),
        submit("cleo-full", "full-autonomy"),
    )
    .await
    .unwrap()
    .result
    .unwrap();
    assert_eq!(completed["turn"]["state"], "completed");
    assert_eq!(completed["task"]["effectiveAuthority"], "full-autonomy");
    assert_eq!(harness.backend.starts.load(Ordering::SeqCst), 3);
    let cleo_lead_session = harness
        .store
        .list_sessions()
        .unwrap()
        .into_iter()
        .rfind(|session| session.employee_id == cleo.id)
        .unwrap();
    assert_eq!(cleo_lead_session.provider, "claude-code");
    assert!(
        harness.backend.resume_ids.lock().unwrap().is_empty(),
        "a conversation never resumes the task-dispatch session that preceded it"
    );
    assert_eq!(
        resume_events(&harness.store),
        vec![(
            "session.resume.skipped".to_owned(),
            json!({
                "sessionId": cleo_lead_session.id,
                "priorSessionId": cleo_session.id,
                "reason": "task-session"
            })
        )]
    );
    assert_eq!(
        harness.backend.turns.load(Ordering::SeqCst),
        3,
        "Rhea's turn, Cleo's dispatch turn, Cleo's user turn"
    );
}

#[test]
fn claude_full_autonomy_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/claude-full-autonomy.ps1");
    let script =
        std::fs::read_to_string(&script_path).expect("claude full autonomy acceptance script");

    for required in [
        "claude_code_runs_turns_and_tasks_only_under_full_autonomy",
        "task_turn_rejects_a_provider_without_authority_override_support_before_io",
        "store_enables_wal_foreign_keys_and_schema_version",
        "fullAutonomyDelivered",
        "askAndSelfDecisionRefused",
        "otherProvidersRefused",
        "noSupportClaimed",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "deterministic-fake-transport",
        "schemaVersion = 19",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in ["provider', 'smoke", "LivePrompt"] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}
