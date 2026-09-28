use lazoo_protocol::{
    AuthorityMode, EmployeeAvailability, EmployeeCheckoutPolicy, ProviderObservation,
    SessionLifecycleState,
};
use lazoo_store::{
    BeginSessionInput, BeginSessionStopInput, CreateEmployeeInput, FailSessionInput,
    FinishSessionStopInput, MarkSessionReadyInput, Store, StoreError, UpdateEmployeeInput,
};
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

fn employee() -> CreateEmployeeInput {
    CreateEmployeeInput {
        request_id: "create-request".to_owned(),
        idempotency_key: "create-rhea".to_owned(),
        name: "Rhea".to_owned(),
        provider: "codex".to_owned(),
        role: "Lead".to_owned(),
        instructions: String::new(),
        model: None,
        reasoning_effort: None,
        native_settings: json!({}),
        authority_mode: AuthorityMode::Ask,
        checkout_policy: EmployeeCheckoutPolicy::Isolated,
        activation_policy: lazoo_protocol::ActivationPolicy::Manual,
        enabled: true,
        provider_observation: Some(ProviderObservation {
            provider: "codex".to_owned(),
            verification: "configured".to_owned(),
            version: Some("0.151.0".to_owned()),
            protocol: Some("app-server/v2".to_owned()),
            capabilities: json!({"session": true}),
            authentication: json!({"native": "ready", "external": "unknown"}),
            degradation_reason: None,
            checked_at: "2026-08-31T00:00:00Z".to_owned(),
        }),
    }
}

fn begin(employee_id: Uuid, owner_boot_id: Uuid, key: &str) -> BeginSessionInput {
    BeginSessionInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        employee_id,
        expected_employee_revision: 1,
        owner_boot_id,
    }
}

#[test]
fn session_begin_is_idempotent_and_prevents_a_second_active_session() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let employee = store.create_employee(&employee()).unwrap().employee;
    let owner = Uuid::new_v4();

    let first = store
        .begin_session(&begin(employee.id, owner, "warm-rhea"))
        .unwrap();
    let replay = store
        .begin_session(&begin(employee.id, owner, "warm-rhea"))
        .unwrap();

    assert_eq!(first.session.id, replay.session.id);
    assert_eq!(first.session.state, SessionLifecycleState::Starting);
    assert_eq!(first.session.revision, 1);
    assert_eq!(first.roster_revision, 2);
    assert_eq!(store.list_sessions().unwrap().len(), 1);
    assert!(matches!(
        store
            .begin_session(&begin(employee.id, owner, "second-warm"))
            .unwrap_err(),
        StoreError::SessionConflict { .. }
    ));
}

#[test]
fn ready_stop_and_suspend_are_revision_checked_and_update_roster_truth() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let employee = store.create_employee(&employee()).unwrap().employee;
    let owner = Uuid::new_v4();
    let starting = store
        .begin_session(&begin(employee.id, owner, "warm-rhea"))
        .unwrap();

    let ready = store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: starting.session.id,
            expected_revision: 1,
            owner_boot_id: owner,
            native_session_id: "thread-1".to_owned(),
            native_host_pid: 4242,
        })
        .unwrap();
    let warm_replay = store
        .begin_session(&begin(employee.id, owner, "warm-rhea"))
        .unwrap();
    assert_eq!(warm_replay.session, ready.session);
    assert!(warm_replay.event_sequence.is_none());
    assert_eq!(ready.session.state, SessionLifecycleState::Idle);
    assert_eq!(ready.session.revision, 2);
    assert_eq!(ready.roster_revision, 3);
    let roster = store.roster_snapshot().unwrap();
    assert_eq!(roster.entries[0].availability, EmployeeAvailability::Idle);
    assert_eq!(
        roster.entries[0].session.as_ref().unwrap().id,
        ready.session.id
    );

    assert!(matches!(
        store
            .mark_session_ready(&MarkSessionReadyInput {
                session_id: ready.session.id,
                expected_revision: 1,
                owner_boot_id: owner,
                native_session_id: "thread-1".to_owned(),
                native_host_pid: 4242,
            })
            .unwrap_err(),
        StoreError::SessionStale {
            current_revision: 2,
            ..
        }
    ));

    let stopping = store
        .begin_session_stop(&BeginSessionStopInput {
            request_id: "stop-request".to_owned(),
            idempotency_key: "stop-rhea".to_owned(),
            session_id: ready.session.id,
            expected_revision: 2,
        })
        .unwrap();
    assert_eq!(stopping.session.state, SessionLifecycleState::Stopping);
    assert_eq!(stopping.session.revision, 3);
    assert_eq!(stopping.roster_revision, 3);

    let suspended = store
        .finish_session_stop(&FinishSessionStopInput {
            session_id: ready.session.id,
            expected_revision: 3,
        })
        .unwrap();
    let stop_replay = store
        .begin_session_stop(&BeginSessionStopInput {
            request_id: "stop-request".to_owned(),
            idempotency_key: "stop-rhea".to_owned(),
            session_id: ready.session.id,
            expected_revision: 2,
        })
        .unwrap();
    assert_eq!(stop_replay.session, suspended.session);
    assert!(stop_replay.event_sequence.is_none());
    assert_eq!(suspended.session.state, SessionLifecycleState::Suspended);
    assert_eq!(suspended.session.revision, 4);
    assert_eq!(suspended.roster_revision, 4);
    assert_eq!(
        store.roster_snapshot().unwrap().entries[0].availability,
        EmployeeAvailability::Suspended
    );
}

#[test]
fn failed_start_releases_the_active_slot_and_preserves_safe_failure_truth() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let employee = store.create_employee(&employee()).unwrap().employee;
    let owner = Uuid::new_v4();
    let starting = store
        .begin_session(&begin(employee.id, owner, "warm-rhea"))
        .unwrap();

    let failed = store
        .fail_session(&FailSessionInput {
            session_id: starting.session.id,
            expected_revision: 1,
            failure_code: "LZ-SESSION-START-FAILED".to_owned(),
            failure_message: "provider session did not become ready".to_owned(),
        })
        .unwrap();

    assert_eq!(failed.session.state, SessionLifecycleState::Failed);
    assert!(failed.session.owner_boot_id.is_none());
    assert_eq!(
        store.roster_snapshot().unwrap().entries[0]
            .availability_reason
            .as_deref(),
        Some("provider session did not become ready")
    );
    assert!(
        store
            .begin_session(&begin(employee.id, owner, "retry-rhea"))
            .is_ok()
    );
}

#[test]
fn prior_boot_active_sessions_reconcile_to_failed_without_pid_adoption() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let employee = store.create_employee(&employee()).unwrap().employee;
    let prior_boot = Uuid::new_v4();
    let starting = store
        .begin_session(&begin(employee.id, prior_boot, "warm-rhea"))
        .unwrap();
    store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: starting.session.id,
            expected_revision: 1,
            owner_boot_id: prior_boot,
            native_session_id: "thread-1".to_owned(),
            native_host_pid: 4242,
        })
        .unwrap();

    let reconciled = store.reconcile_sessions(Uuid::new_v4()).unwrap();
    assert_eq!(reconciled.sessions.len(), 1);
    let failed = &reconciled.sessions[0];
    assert_eq!(failed.state, SessionLifecycleState::Failed);
    assert_eq!(
        failed.failure_code.as_deref(),
        Some("LZ-SESSION-OWNER-LOST")
    );
    assert!(failed.native_host_pid.is_none());
    assert_eq!(
        store.roster_snapshot().unwrap().entries[0].availability,
        EmployeeAvailability::Failed
    );

    let replay = store.reconcile_sessions(Uuid::new_v4()).unwrap();
    assert!(replay.sessions.is_empty());
    assert!(replay.event_sequence.is_none());
}

#[test]
fn warm_requires_route_ready_provider_and_active_sessions_lock_employee_configuration() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let mut offline = employee();
    offline.name = "Offline".to_owned();
    offline.idempotency_key = "create-offline".to_owned();
    offline.provider_observation = None;
    let offline = store.create_employee(&offline).unwrap().employee;
    assert!(matches!(
        store
            .begin_session(&begin(offline.id, Uuid::new_v4(), "warm-offline"))
            .unwrap_err(),
        StoreError::SessionProviderNotReady { .. }
    ));

    let employee = store.create_employee(&employee()).unwrap().employee;
    let active = store
        .begin_session(&begin(employee.id, Uuid::new_v4(), "warm-rhea"))
        .unwrap();
    let error = store
        .update_employee(&UpdateEmployeeInput {
            request_id: "update-request".to_owned(),
            idempotency_key: "update-active".to_owned(),
            employee_id: employee.id,
            expected_revision: 1,
            name: None,
            provider: None,
            role: Some("Principal lead".to_owned()),
            instructions: None,
            model: None,
            reasoning_effort: None,
            native_settings: None,
            authority_mode: None,
            checkout_policy: None,
            activation_policy: None,
            enabled: None,
            provider_observation: None,
        })
        .unwrap_err();
    assert!(matches!(
        error,
        StoreError::EmployeeSessionActive {
            session_id,
            ..
        } if session_id == active.session.id
    ));
}
