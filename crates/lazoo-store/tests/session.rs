use chrono::{TimeDelta, Utc};
use lazoo_protocol::{
    AuthorityMode, EmployeeAvailability, EmployeeCheckoutPolicy, ProviderObservation,
    SessionLifecycleState,
};
use lazoo_store::{
    BeginIdleSessionStopInput, BeginSessionInput, BeginSessionStopInput,
    BindPendingNativeSessionInput, CreateEmployeeInput, FailSessionInput, FinishSessionStopInput,
    MarkSessionReadyInput, Store, StoreError, UpdateEmployeeInput,
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
        idle_timeout_seconds: 0,
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

fn ready_session(store: &Store, employee_id: Uuid, owner: Uuid, key: &str) -> Uuid {
    let starting = store
        .begin_session(&begin(employee_id, owner, key))
        .unwrap();
    store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: starting.session.id,
            expected_revision: starting.session.revision,
            owner_boot_id: owner,
            native_session_id: format!("thread-{key}"),
            native_host_pid: 4242,
        })
        .unwrap()
        .session
        .id
}

#[test]
fn pending_native_session_binds_once_with_live_owner_revision_and_idle_guards() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let employee = store.create_employee(&employee()).unwrap().employee;
    let owner = Uuid::new_v4();
    let starting = store
        .begin_session(&begin(employee.id, owner, "warm-pending"))
        .unwrap();
    let ready = store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: starting.session.id,
            expected_revision: starting.session.revision,
            owner_boot_id: owner,
            native_session_id: "pending:ag-session".to_owned(),
            native_host_pid: 4242,
        })
        .unwrap();
    let bound = store
        .bind_pending_native_session(&BindPendingNativeSessionInput {
            session_id: ready.session.id,
            expected_revision: ready.session.revision,
            owner_boot_id: owner,
            expected_pending_native_session_id: "pending:ag-session".to_owned(),
            native_session_id: "ag-native-session".to_owned(),
        })
        .unwrap();
    assert_eq!(
        bound.session.native_session_id.as_deref(),
        Some("ag-native-session")
    );
    assert_eq!(bound.session.revision, ready.session.revision + 1);
    assert!(bound.event_sequence.is_some());
    assert!(matches!(
        store
            .bind_pending_native_session(&BindPendingNativeSessionInput {
                session_id: ready.session.id,
                expected_revision: ready.session.revision,
                owner_boot_id: owner,
                expected_pending_native_session_id: "pending:ag-session".to_owned(),
                native_session_id: "second-native-session".to_owned(),
            })
            .unwrap_err(),
        StoreError::SessionStale { .. }
    ));
    assert!(matches!(
        store
            .bind_pending_native_session(&BindPendingNativeSessionInput {
                session_id: bound.session.id,
                expected_revision: bound.session.revision,
                owner_boot_id: Uuid::new_v4(),
                expected_pending_native_session_id: "pending:ag-session".to_owned(),
                native_session_id: "third-native-session".to_owned(),
            })
            .unwrap_err(),
        StoreError::SessionOwnership { .. }
    ));
}

#[test]
fn idle_sessions_are_due_only_after_their_employee_timeout_and_stop_once_under_their_own_key() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let mut timed = employee();
    timed.idle_timeout_seconds = 600;
    let timed = store.create_employee(&timed).unwrap().employee;
    let mut never = employee();
    "create-mira".clone_into(&mut never.idempotency_key);
    "Mira".clone_into(&mut never.name);
    let never = store.create_employee(&never).unwrap().employee;
    let owner = Uuid::new_v4();
    let session_id = ready_session(&store, timed.id, owner, "warm-rhea");
    let never_session_id = ready_session(&store, never.id, owner, "warm-mira");
    let now = Utc::now();
    let due_ids = |at| -> Vec<Uuid> {
        store
            .idle_sessions_due(owner, at)
            .unwrap()
            .into_iter()
            .map(|session| session.id)
            .collect()
    };

    assert!(
        due_ids(now).is_empty(),
        "a session that just started is busy enough"
    );
    assert!(due_ids(now + TimeDelta::seconds(590)).is_empty());
    assert_eq!(due_ids(now + TimeDelta::seconds(601)), vec![session_id]);
    assert!(
        due_ids(now + TimeDelta::days(30))
            .iter()
            .all(|id| *id != never_session_id),
        "an employee whose timeout is 0 is never suspended"
    );
    assert!(
        store
            .idle_sessions_due(Uuid::new_v4(), now + TimeDelta::days(30))
            .unwrap()
            .is_empty(),
        "another boot's sessions are not this daemon's to suspend"
    );

    let early = BeginIdleSessionStopInput {
        session_id,
        owner_boot_id: owner,
        now: now + TimeDelta::seconds(10),
    };
    assert!(store.begin_idle_session_stop(&early).unwrap().is_none());
    assert_eq!(
        store.get_session(session_id).unwrap().state,
        SessionLifecycleState::Idle
    );

    let due = BeginIdleSessionStopInput {
        now: now + TimeDelta::seconds(601),
        ..early
    };
    let stopping = store.begin_idle_session_stop(&due).unwrap().unwrap();
    assert_eq!(stopping.session.state, SessionLifecycleState::Stopping);
    assert!(stopping.event_sequence.is_some());
    let replay = store.begin_idle_session_stop(&due).unwrap().unwrap();
    assert_eq!(replay.session.id, session_id);
    assert!(replay.event_sequence.is_none(), "one stop per session");
    assert!(due_ids(now + TimeDelta::days(30)).is_empty());

    // The idle stop is an ordinary session.stop command: a user stop with the same key replays
    // it instead of stopping anything.
    let user_replay = store
        .begin_session_stop(&BeginSessionStopInput {
            request_id: "user-stop".to_owned(),
            idempotency_key: format!("idle-suspend:{session_id}"),
            session_id,
            expected_revision: 1,
        })
        .unwrap();
    assert!(user_replay.event_sequence.is_none());
    let suspended = store
        .finish_session_stop(&FinishSessionStopInput {
            session_id,
            expected_revision: stopping.session.revision,
        })
        .unwrap();
    assert_eq!(suspended.session.state, SessionLifecycleState::Suspended);
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
            idle_timeout_seconds: None,
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
