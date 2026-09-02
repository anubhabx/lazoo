use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use lazoo_protocol::{
    AuthorityMode, CheckoutMode, DesktopSnapshot, EmployeeCheckoutPolicy, JsonRpcRequest,
    ProviderObservation,
};
use lazoo_providers::{DiscoveryEnvironment, ProviderError, ProviderKind, SessionRequest};
use lazoo_runtime::DataRoot;
use lazoo_store::{CreateEmployeeInput, Store};
use lazood::{App, ManagedSession, SessionBackend, SessionBackendFuture};
use serde_json::json;
use tempfile::tempdir;
use tokio::time::timeout;

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

#[derive(Clone, Default)]
struct FakeSessionBackend {
    starts: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
    working_directories: Arc<Mutex<Vec<PathBuf>>>,
}

struct FakeManagedSession {
    id: String,
    stops: Arc<AtomicUsize>,
}

impl ManagedSession for FakeManagedSession {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn native_host_pid(&self) -> Option<u32> {
        Some(4242)
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
    assert_eq!(result["schemaVersion"], 4);
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
        "schemaVersion 4",
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
        "schemaVersion 4",
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
        "schemaVersion 4",
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
