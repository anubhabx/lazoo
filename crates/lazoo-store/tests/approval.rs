use lazoo_protocol::{
    ApprovalDecider, ApprovalDecision, ApprovalKind, ApprovalState, AuthorityMode,
    EmployeeCheckoutPolicy, MessageSender, ProviderObservation, UserMessageEndpoint,
    UserMessageKind,
};
use lazoo_store::{
    BeginSessionInput, BeginTurnSubmissionInput, CloseApprovalInput, CreateConversationInput,
    CreateEmployeeInput, DecideApprovalInput, ListApprovalsInput, MarkSessionReadyInput,
    RecordTurnInput, RequestApprovalInput, RequestTurnCancelInput, Store, StoreError,
};
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

struct Fixture {
    store: Store,
    turn_id: Uuid,
    session_id: Uuid,
    boot_id: Uuid,
}

fn approval_fixture(authority: AuthorityMode) -> (tempfile::TempDir, Fixture) {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let employee = store
        .create_employee(&CreateEmployeeInput {
            request_id: "create-rhea-request".to_owned(),
            idempotency_key: "create-rhea".to_owned(),
            name: "Rhea".to_owned(),
            provider: "codex".to_owned(),
            role: "Lead".to_owned(),
            instructions: String::new(),
            model: None,
            reasoning_effort: None,
            native_settings: json!({}),
            authority_mode: authority,
            checkout_policy: EmployeeCheckoutPolicy::Isolated,
            activation_policy: lazoo_protocol::ActivationPolicy::Manual,
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
            request_id: "create-conversation-request".to_owned(),
            idempotency_key: "create-conversation".to_owned(),
            project_id: None,
            lead_employee_id: employee.id,
            title: None,
        })
        .unwrap()
        .conversation;
    let turn = store
        .record_turn(&RecordTurnInput {
            request_id: "record-turn-request".to_owned(),
            idempotency_key: "record-turn".to_owned(),
            conversation_id: conversation.id,
            author: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            recipient: None,
            task_id: None,
            authority_mode: authority,
            text: "Run the tests".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap()
        .turn;
    let boot_id = Uuid::new_v4();
    let starting = store
        .begin_session(&BeginSessionInput {
            request_id: "warm-request".to_owned(),
            idempotency_key: "warm".to_owned(),
            employee_id: employee.id,
            expected_employee_revision: employee.configuration_revision,
            owner_boot_id: boot_id,
        })
        .unwrap();
    let session_id = store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: starting.session.id,
            expected_revision: starting.session.revision,
            owner_boot_id: boot_id,
            native_session_id: "thread-1".to_owned(),
            native_host_pid: 4242,
        })
        .unwrap()
        .session
        .id;
    store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "begin-request".to_owned(),
            idempotency_key: "begin".to_owned(),
            turn_id: turn.id,
            expected_revision: turn.revision,
            owner_boot_id: boot_id,
            recipient_session_id: session_id,
        })
        .unwrap();
    (
        temporary,
        Fixture {
            store,
            turn_id: turn.id,
            session_id,
            boot_id,
        },
    )
}

fn request(
    fixture: &Fixture,
    key: &str,
    kind: ApprovalKind,
    action: &str,
    scope_expansion: bool,
) -> RequestApprovalInput {
    RequestApprovalInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        turn_id: fixture.turn_id,
        owner_boot_id: fixture.boot_id,
        native_request_id: Some(key.to_owned()),
        kind,
        action: action.to_owned(),
        scope_expansion,
        detail: json!({"command": action, "cwd": "C:\\repo"}),
        pending_timeout_seconds: 900,
    }
}

fn event_types(store: &Store) -> Vec<String> {
    store
        .list_events(0, 1000)
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .collect()
}

#[test]
#[allow(clippy::too_many_lines)] // One ledger walk keeps the revision and replay assertions in order.
fn ask_authority_records_a_pending_approval_and_user_decisions_are_revisioned_and_idempotent() {
    let (_temporary, fixture) = approval_fixture(AuthorityMode::Ask);
    let store = &fixture.store;

    let requested = store
        .request_approval(&request(
            &fixture,
            "approval-1",
            ApprovalKind::CommandExecution,
            "cargo test",
            false,
        ))
        .unwrap();
    let approval = requested.approval;
    assert_eq!(approval.state, ApprovalState::Pending);
    assert_eq!(approval.decider, None);
    assert_eq!(approval.revision, 1);
    assert_eq!(approval.authority, AuthorityMode::Ask);
    assert_eq!(approval.turn_id, fixture.turn_id);
    assert_eq!(approval.task_id, Some(fixture.turn_id));
    assert_eq!(approval.session_id, Some(fixture.session_id));
    assert_eq!(approval.owner_boot_id, fixture.boot_id);
    assert_eq!(approval.kind, ApprovalKind::CommandExecution);
    assert_eq!(approval.action, "cargo test");
    assert_eq!(approval.fingerprint.len(), 64);
    assert_eq!(approval.detail["cwd"], "C:\\repo");
    assert!(approval.decided_at.is_none());
    assert!(approval.expires_at > approval.created_at);
    assert!(requested.event_sequence.is_some());

    let replay = store
        .request_approval(&request(
            &fixture,
            "approval-1",
            ApprovalKind::CommandExecution,
            "cargo test",
            false,
        ))
        .unwrap();
    assert_eq!(replay.approval, approval);
    assert_eq!(replay.event_sequence, None);
    assert_eq!(
        event_types(store)
            .iter()
            .filter(|kind| *kind == "approval.requested")
            .count(),
        1
    );

    let decided = store
        .decide_approval(&DecideApprovalInput {
            request_id: "decide-request".to_owned(),
            idempotency_key: "decide-1".to_owned(),
            approval_id: approval.id,
            expected_revision: 1,
            decision: ApprovalDecision::Approve,
        })
        .unwrap();
    assert_eq!(decided.approval.state, ApprovalState::Approved);
    assert_eq!(decided.approval.decider, Some(ApprovalDecider::User));
    assert_eq!(decided.approval.revision, 2);
    assert!(decided.approval.decided_at.is_some());
    assert!(decided.event_sequence.is_some());

    let replay = store
        .decide_approval(&DecideApprovalInput {
            request_id: "decide-request-again".to_owned(),
            idempotency_key: "decide-1".to_owned(),
            approval_id: approval.id,
            expected_revision: 1,
            decision: ApprovalDecision::Decline,
        })
        .unwrap();
    assert_eq!(replay.approval, decided.approval);
    assert_eq!(replay.event_sequence, None);

    let stale = store
        .decide_approval(&DecideApprovalInput {
            request_id: "decide-stale".to_owned(),
            idempotency_key: "decide-stale".to_owned(),
            approval_id: approval.id,
            expected_revision: 1,
            decision: ApprovalDecision::Decline,
        })
        .unwrap_err();
    assert!(matches!(
        stale,
        StoreError::ApprovalStale {
            current_revision: 2,
            ..
        }
    ));
    let settled = store
        .decide_approval(&DecideApprovalInput {
            request_id: "decide-settled".to_owned(),
            idempotency_key: "decide-settled".to_owned(),
            approval_id: approval.id,
            expected_revision: 2,
            decision: ApprovalDecision::Decline,
        })
        .unwrap_err();
    assert!(matches!(settled, StoreError::ApprovalState { .. }));
    let conflict = store
        .decide_approval(&DecideApprovalInput {
            request_id: "decide-conflict".to_owned(),
            idempotency_key: "decide-1".to_owned(),
            approval_id: Uuid::new_v4(),
            expected_revision: 1,
            decision: ApprovalDecision::Approve,
        })
        .unwrap_err();
    assert!(matches!(conflict, StoreError::ApprovalIdempotencyConflict));

    assert_eq!(
        store.get_approval(approval.id).unwrap().state,
        ApprovalState::Approved
    );
    let pending = store
        .list_approvals(&ListApprovalsInput {
            after_sequence: 0,
            limit: 10,
            state: Some(ApprovalState::Pending),
            turn_id: None,
            task_id: None,
            conversation_id: None,
            employee_id: None,
        })
        .unwrap();
    assert!(pending.is_empty());
    let by_turn = store
        .list_approvals(&ListApprovalsInput {
            after_sequence: 0,
            limit: 10,
            state: None,
            turn_id: Some(fixture.turn_id),
            task_id: Some(fixture.turn_id),
            conversation_id: None,
            employee_id: None,
        })
        .unwrap();
    assert_eq!(by_turn.len(), 1);
    assert!(matches!(
        store.list_approvals(&ListApprovalsInput {
            after_sequence: 0,
            limit: 0,
            state: None,
            turn_id: None,
            task_id: None,
            conversation_id: None,
            employee_id: None,
        }),
        Err(StoreError::ApprovalValidation(_))
    ));
    assert_eq!(
        event_types(store)
            .iter()
            .filter(|kind| *kind == "approval.decided")
            .count(),
        1
    );
}

#[test]
fn policy_auto_approves_inside_scope_and_pends_scope_expansion() {
    let (_temporary, fixture) = approval_fixture(AuthorityMode::SelfDecision);
    let store = &fixture.store;

    let in_scope = store
        .request_approval(&request(
            &fixture,
            "file-change",
            ApprovalKind::FileChange,
            "apply file changes in the workspace",
            false,
        ))
        .unwrap()
        .approval;
    assert_eq!(in_scope.state, ApprovalState::Approved);
    assert_eq!(in_scope.decider, Some(ApprovalDecider::Policy));
    assert_eq!(in_scope.authority, AuthorityMode::SelfDecision);
    assert!(in_scope.decided_at.is_some());

    let widened = store
        .request_approval(&request(
            &fixture,
            "grant-root",
            ApprovalKind::CommandExecution,
            "apply file changes under C:\\",
            true,
        ))
        .unwrap()
        .approval;
    assert_eq!(widened.state, ApprovalState::Pending);
    assert!(widened.scope_expansion);

    let (_autonomous_temporary, autonomous) = approval_fixture(AuthorityMode::FullAutonomy);
    let approved = autonomous
        .store
        .request_approval(&request(
            &autonomous,
            "autonomous",
            ApprovalKind::CommandExecution,
            "rm -rf target",
            true,
        ))
        .unwrap()
        .approval;
    assert_eq!(approved.state, ApprovalState::Approved);
    assert_eq!(approved.decider, Some(ApprovalDecider::Policy));
    assert_eq!(approved.authority, AuthorityMode::FullAutonomy);
}

#[test]
fn approval_requests_require_an_owned_in_flight_turn_and_bounded_input() {
    let (_temporary, fixture) = approval_fixture(AuthorityMode::Ask);
    let store = &fixture.store;

    let mut foreign = request(
        &fixture,
        "foreign-boot",
        ApprovalKind::CommandExecution,
        "cargo test",
        false,
    );
    foreign.owner_boot_id = Uuid::new_v4();
    assert!(matches!(
        store.request_approval(&foreign),
        Err(StoreError::ApprovalTurnOwnership { .. })
    ));

    let mut missing = request(
        &fixture,
        "missing-turn",
        ApprovalKind::CommandExecution,
        "cargo test",
        false,
    );
    missing.turn_id = Uuid::new_v4();
    assert!(matches!(
        store.request_approval(&missing),
        Err(StoreError::TurnNotFound { .. })
    ));

    let empty = request(
        &fixture,
        "empty-action",
        ApprovalKind::CommandExecution,
        "   ",
        false,
    );
    assert!(matches!(
        store.request_approval(&empty),
        Err(StoreError::ApprovalValidation(_))
    ));
    let mut oversized = request(
        &fixture,
        "oversized",
        ApprovalKind::CommandExecution,
        "cargo test",
        false,
    );
    oversized.detail = json!({"command": "x".repeat(17 * 1024)});
    assert!(matches!(
        store.request_approval(&oversized),
        Err(StoreError::ApprovalValidation(_))
    ));
    assert!(
        event_types(store)
            .iter()
            .all(|kind| kind != "approval.requested")
    );

    let turn = store.get_turn(fixture.turn_id).unwrap();
    store
        .request_turn_cancel(&RequestTurnCancelInput {
            request_id: "cancel-request".to_owned(),
            idempotency_key: "cancel".to_owned(),
            turn_id: turn.id,
            expected_revision: turn.revision,
        })
        .unwrap();
    let cancelled = store
        .request_approval(&request(
            &fixture,
            "after-cancel",
            ApprovalKind::CommandExecution,
            "cargo test",
            false,
        ))
        .unwrap()
        .approval;
    assert_eq!(cancelled.state, ApprovalState::Cancelled);
    assert_eq!(cancelled.decider, Some(ApprovalDecider::System));
    assert_eq!(
        cancelled.failure_code.as_deref(),
        Some("LZ-APPROVAL-TURN-CANCELLED")
    );
}

#[test]
fn system_closure_and_restart_reconciliation_fail_closed() {
    let (_temporary, fixture) = approval_fixture(AuthorityMode::Ask);
    let store = &fixture.store;
    let pending = store
        .request_approval(&request(
            &fixture,
            "timeout",
            ApprovalKind::CommandExecution,
            "cargo test",
            false,
        ))
        .unwrap()
        .approval;

    assert!(matches!(
        store.close_approval(&CloseApprovalInput {
            approval_id: pending.id,
            owner_boot_id: Uuid::new_v4(),
            expected_revision: pending.revision,
            state: ApprovalState::Expired,
            failure_code: "LZ-APPROVAL-TIMEOUT".to_owned(),
            failure_message: "timed out".to_owned(),
        }),
        Err(StoreError::ApprovalOwnership { .. })
    ));
    assert!(matches!(
        store.close_approval(&CloseApprovalInput {
            approval_id: pending.id,
            owner_boot_id: fixture.boot_id,
            expected_revision: pending.revision,
            state: ApprovalState::Approved,
            failure_code: "LZ-APPROVAL-TIMEOUT".to_owned(),
            failure_message: "timed out".to_owned(),
        }),
        Err(StoreError::InvalidRecord { .. })
    ));
    let expired = store
        .close_approval(&CloseApprovalInput {
            approval_id: pending.id,
            owner_boot_id: fixture.boot_id,
            expected_revision: pending.revision,
            state: ApprovalState::Expired,
            failure_code: "LZ-APPROVAL-TIMEOUT".to_owned(),
            failure_message: "timed out".to_owned(),
        })
        .unwrap()
        .approval;
    assert_eq!(expired.state, ApprovalState::Expired);
    assert_eq!(expired.decider, Some(ApprovalDecider::System));
    assert_eq!(expired.failure_code.as_deref(), Some("LZ-APPROVAL-TIMEOUT"));
    assert_eq!(expired.revision, 2);
    assert!(matches!(
        store.close_approval(&CloseApprovalInput {
            approval_id: pending.id,
            owner_boot_id: fixture.boot_id,
            expected_revision: 2,
            state: ApprovalState::Cancelled,
            failure_code: "LZ-APPROVAL-TURN-CANCELLED".to_owned(),
            failure_message: "cancelled".to_owned(),
        }),
        Err(StoreError::ApprovalState { .. })
    ));

    let orphan = store
        .request_approval(&request(
            &fixture,
            "orphan",
            ApprovalKind::FileChange,
            "apply file changes in the workspace",
            false,
        ))
        .unwrap()
        .approval;
    let reconciled = store.reconcile_approvals(Uuid::new_v4()).unwrap();
    assert_eq!(reconciled.approvals.len(), 1);
    assert_eq!(reconciled.approvals[0].id, orphan.id);
    assert_eq!(reconciled.approvals[0].state, ApprovalState::Expired);
    assert_eq!(
        reconciled.approvals[0].failure_code.as_deref(),
        Some("LZ-APPROVAL-OWNER-LOST")
    );
    assert!(reconciled.event_sequence.is_some());
    let again = store.reconcile_approvals(Uuid::new_v4()).unwrap();
    assert!(again.approvals.is_empty());
    assert_eq!(again.event_sequence, None);

    let types = event_types(store);
    assert_eq!(
        types
            .iter()
            .filter(|kind| *kind == "approval.expired")
            .count(),
        1
    );
    assert_eq!(
        types
            .iter()
            .filter(|kind| *kind == "approvals.reconciled")
            .count(),
        1
    );
}
