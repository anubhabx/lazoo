use std::collections::BTreeMap;

use lazoo_domain::{
    InboxCursor, InboxEmployees, InboxValidationError, approval_inbox_item, inbox_counts,
    inbox_filter_matches, incident_inbox_item, sort_inbox_archive, sort_unresolved_inbox,
    task_inbox_item, validate_inbox_limit,
};
use lazoo_protocol::{
    APPROVAL_CONTRACT_V1, ActivationPolicy, ApprovalKind, ApprovalRecord, ApprovalState,
    AuthorityMode, EmployeeCheckoutPolicy, EmployeeRecord, INCIDENT_CONTRACT_V1, InboxAttention,
    InboxDeadlineKind, InboxFilter, InboxKind, InboxSeverity, IncidentEscalation,
    IncidentEscalationReason, IncidentPriority, IncidentRecord, IncidentSource, IncidentState,
    MessageSender, TASK_CONTRACT_V1, TaskRecord, TaskRole, TaskState, UserMessageEndpoint,
    UserMessageKind,
};
use serde_json::json;
use uuid::Uuid;

const MIRA: Uuid = Uuid::from_u128(3);

fn employees() -> InboxEmployees {
    let mut employees = BTreeMap::new();
    employees.insert(
        MIRA,
        EmployeeRecord {
            id: MIRA,
            name: "Mira".to_owned(),
            provider: "codex".to_owned(),
            role: "Worker".to_owned(),
            instructions: String::new(),
            model: None,
            reasoning_effort: None,
            native_settings: json!({}),
            authority_mode: AuthorityMode::Ask,
            checkout_policy: EmployeeCheckoutPolicy::Isolated,
            activation_policy: ActivationPolicy::Manual,
            idle_timeout_seconds: 0,
            enabled: true,
            configuration_revision: 1,
            created_at: "2026-10-13T00:00:00Z".to_owned(),
            updated_at: "2026-10-13T00:00:00Z".to_owned(),
            disabled_at: None,
        },
    );
    employees
}

fn approval(id: u64, state: ApprovalState) -> ApprovalRecord {
    ApprovalRecord {
        sequence: id,
        version: APPROVAL_CONTRACT_V1,
        id: Uuid::from_u128(u128::from(id)),
        turn_id: Uuid::from_u128(20),
        task_id: Some(Uuid::from_u128(21)),
        conversation_id: Uuid::from_u128(22),
        project_id: None,
        employee_id: MIRA,
        session_id: Some(Uuid::from_u128(23)),
        owner_boot_id: Uuid::from_u128(24),
        provider: "codex".to_owned(),
        native_session_id: None,
        native_turn_id: None,
        native_request_id: None,
        kind: ApprovalKind::CommandExecution,
        action: "cargo test".to_owned(),
        detail: json!({"command": "cargo test"}),
        fingerprint: "ab".repeat(32),
        authority: AuthorityMode::Ask,
        scope_expansion: false,
        state,
        decider: None,
        revision: 2,
        failure_code: None,
        failure_message: None,
        created_at: "2026-10-13T00:00:00Z".to_owned(),
        expires_at: "2026-10-13T00:15:00Z".to_owned(),
        updated_at: "2026-10-13T00:01:00Z".to_owned(),
        decided_at: None,
    }
}

fn task(id: u64, state: TaskState) -> TaskRecord {
    TaskRecord {
        sequence: id,
        version: TASK_CONTRACT_V1,
        id: Uuid::from_u128(u128::from(id)),
        root_task_id: Uuid::from_u128(30),
        parent_task_id: Some(Uuid::from_u128(30)),
        depth: 1,
        project_id: Some(Uuid::from_u128(31)),
        conversation_id: Uuid::from_u128(22),
        creation_turn_id: None,
        delegation_message_id: None,
        creator: MessageSender::User(UserMessageEndpoint {
            kind: UserMessageKind::User,
        }),
        assigned_employee_id: MIRA,
        role: TaskRole::Worker,
        objective: "Implement the parser fix".to_owned(),
        references: Vec::new(),
        requested_authority: AuthorityMode::Ask,
        effective_authority: AuthorityMode::Ask,
        employee_configuration_revision: 1,
        roster_revision: 1,
        state,
        revision: 1,
        active_turn_id: None,
        active_session_id: None,
        failure_code: Some("LZ-TURN-OWNER-LOST".to_owned()),
        failure_message: None,
        created_at: "2026-10-13T00:00:30Z".to_owned(),
        updated_at: "2026-10-13T00:02:00Z".to_owned(),
        terminal_at: None,
    }
}

fn incident(id: u64, state: IncidentState, escalation: IncidentEscalation) -> IncidentRecord {
    IncidentRecord {
        sequence: id,
        version: INCIDENT_CONTRACT_V1,
        id: Uuid::from_u128(u128::from(id)),
        code: "LZ-SESSION-STALLED".to_owned(),
        fingerprint: "abc".to_owned(),
        title: "Session accepted work but produced no progress".to_owned(),
        explanation: "No progress evidence arrived.".to_owned(),
        next_action: "Retry explicitly.".to_owned(),
        project_id: None,
        employee_id: Some(MIRA),
        provider: None,
        task_id: Some(Uuid::from_u128(21)),
        session_id: None,
        turn_id: None,
        message_id: None,
        source: IncidentSource::Daemon,
        detail: json!({}),
        occurrences: 1,
        window_occurrences: 1,
        first_occurred_at: "2026-10-13T00:03:00Z".to_owned(),
        last_occurred_at: "2026-10-13T00:03:00Z".to_owned(),
        window_started_at: "2026-10-13T00:03:00Z".to_owned(),
        priority: IncidentPriority::Priority,
        state,
        revision: 4,
        created_at: "2026-10-13T00:03:00Z".to_owned(),
        updated_at: "2026-10-13T00:03:00Z".to_owned(),
        acknowledged_at: None,
        resolved_at: None,
        escalation,
        remediation_deadline_at: (escalation == IncidentEscalation::Lead)
            .then(|| "2026-10-13T00:08:00Z".to_owned()),
        escalated_at: None,
        escalation_reason: (escalation == IncidentEscalation::User)
            .then_some(IncidentEscalationReason::NoLead),
        remedies: Vec::new(),
        user_notified_at: None,
        user_notification: None,
    }
}

#[test]
fn a_pending_approval_needs_the_user_and_a_decided_one_is_the_archive() {
    let item = approval_inbox_item(&approval(1, ApprovalState::Pending), &employees());
    assert_eq!(item.kind, InboxKind::Approval);
    assert_eq!(item.attention, InboxAttention::ActionRequired);
    assert_eq!(item.severity, InboxSeverity::Priority);
    assert_eq!(item.subtype, "command-execution");
    assert_eq!(item.state, "pending");
    assert_eq!(item.title, "Approval requested: cargo test");
    assert_eq!(item.required_action, "Approve or decline: cargo test");
    assert_eq!(item.scope.employee_name.as_deref(), Some("Mira"));
    assert_eq!(item.scope.provider.as_deref(), Some("codex"));
    assert_eq!(item.scope.conversation_id, Some(Uuid::from_u128(22)));
    let deadline = item.deadline.as_ref().unwrap();
    assert_eq!(deadline.kind, InboxDeadlineKind::Expires);
    assert_eq!(deadline.at, "2026-10-13T00:15:00Z");
    assert_eq!(item.detail["authority"], json!("ask"));
    assert_eq!(item.detail["detail"]["command"], json!("cargo test"));
    let actions: Vec<(&str, &str)> = item
        .actions
        .iter()
        .map(|action| (action.action.as_str(), action.method.as_str()))
        .collect();
    assert_eq!(
        actions,
        [
            ("approve", "approval.decide"),
            ("decline", "approval.decide")
        ]
    );
    assert_eq!(
        item.actions[1].params,
        json!({"id": Uuid::from_u128(1), "expectedRevision": 2, "decision": "decline"})
    );

    let declined = approval_inbox_item(&approval(2, ApprovalState::Declined), &employees());
    assert_eq!(declined.attention, InboxAttention::Resolved);
    assert_eq!(declined.state, "declined");
    assert_eq!(declined.deadline, None);
    assert!(declined.actions.is_empty());
}

#[test]
fn proposed_and_uncertain_tasks_need_the_user_and_running_ones_are_not_items() {
    let proposal = task_inbox_item(&task(4, TaskState::Proposed), false, &employees()).unwrap();
    assert_eq!(proposal.kind, InboxKind::TaskProposal);
    assert_eq!(proposal.attention, InboxAttention::ActionRequired);
    assert_eq!(proposal.severity, InboxSeverity::Normal);
    assert_eq!(proposal.title, "Task proposed: Implement the parser fix");
    assert_eq!(proposal.required_action, "Accept or cancel the task");
    assert_eq!(proposal.scope.project_id, Some(Uuid::from_u128(31)));
    assert_eq!(proposal.scope.task_id, Some(Uuid::from_u128(4)));
    assert_eq!(proposal.detail["creator"], json!({"kind": "user"}));
    assert_eq!(proposal.actions[0].method, "task.accept");
    assert_eq!(
        proposal.actions[1].params,
        json!({"id": Uuid::from_u128(4), "expectedRevision": 1, "actor": {"kind": "user"}})
    );

    let uncertain = task_inbox_item(&task(5, TaskState::Uncertain), false, &employees()).unwrap();
    assert_eq!(uncertain.kind, InboxKind::TaskUncertain);
    assert_eq!(uncertain.severity, InboxSeverity::Priority);
    assert_eq!(uncertain.subtype, "task-uncertain");
    assert_eq!(uncertain.required_action, "Decide how the task ended");
    assert_eq!(uncertain.detail["failureCode"], json!("LZ-TURN-OWNER-LOST"));
    let resolutions: Vec<_> = uncertain
        .actions
        .iter()
        .map(|action| action.params["resolution"].clone())
        .collect();
    assert_eq!(
        resolutions,
        [json!("completed"), json!("failed"), json!("cancelled")]
    );
    assert_eq!(
        uncertain.actions[1].params["failureCode"],
        json!("LZ-TURN-OWNER-LOST")
    );
    assert_eq!(uncertain.actions[0].params.get("failureCode"), None);
    assert!(
        uncertain
            .actions
            .iter()
            .all(|action| action.method == "task.resolve")
    );

    for state in [
        TaskState::Accepted,
        TaskState::Running,
        TaskState::CancelRequested,
    ] {
        assert!(task_inbox_item(&task(6, state), true, &employees()).is_none());
    }
    let archived = task_inbox_item(&task(7, TaskState::Failed), true, &employees()).unwrap();
    assert_eq!(archived.kind, InboxKind::TaskUncertain);
    assert_eq!(archived.attention, InboxAttention::Resolved);
    assert_eq!(archived.state, "failed");
    assert!(archived.actions.is_empty());
    let archived_proposal =
        task_inbox_item(&task(8, TaskState::Cancelled), false, &employees()).unwrap();
    assert_eq!(archived_proposal.kind, InboxKind::TaskProposal);
}

#[test]
fn only_escalated_incidents_are_items_and_only_user_escalations_count() {
    let conversation = Some(Uuid::from_u128(22));
    let user = incident_inbox_item(
        &incident(10, IncidentState::Open, IncidentEscalation::User),
        conversation,
        &employees(),
    )
    .unwrap();
    assert_eq!(user.kind, InboxKind::Incident);
    assert_eq!(user.attention, InboxAttention::ActionRequired);
    assert_eq!(user.subtype, "LZ-SESSION-STALLED");
    assert_eq!(user.title, "Session accepted work but produced no progress");
    assert_eq!(user.required_action, "Retry explicitly.");
    assert_eq!(user.scope.conversation_id, conversation);
    assert_eq!(user.scope.provider.as_deref(), Some("codex"));
    assert_eq!(user.detail["escalationReason"], json!("no-lead"));
    let actions: Vec<_> = user
        .actions
        .iter()
        .map(|action| action.method.as_str())
        .collect();
    assert_eq!(actions, ["incident.acknowledge", "incident.resolve"]);
    assert_eq!(
        user.actions[0].params,
        json!({"id": Uuid::from_u128(10), "expectedRevision": 4})
    );

    let watching = incident_inbox_item(
        &incident(11, IncidentState::Open, IncidentEscalation::Lead),
        None,
        &employees(),
    )
    .unwrap();
    assert_eq!(watching.attention, InboxAttention::Watching);
    assert_eq!(
        watching.deadline.as_ref().map(|deadline| deadline.kind),
        Some(InboxDeadlineKind::Remediation)
    );
    assert!(watching.actions.is_empty());

    let acknowledged = incident_inbox_item(
        &incident(12, IncidentState::Acknowledged, IncidentEscalation::User),
        None,
        &employees(),
    )
    .unwrap();
    assert_eq!(acknowledged.attention, InboxAttention::Acknowledged);
    assert_eq!(acknowledged.actions[0].method, "incident.resolve");
    assert_eq!(acknowledged.actions.len(), 1);

    for (state, escalation) in [
        (IncidentState::Resolved, IncidentEscalation::User),
        (IncidentState::Open, IncidentEscalation::Remediated),
    ] {
        let archived =
            incident_inbox_item(&incident(13, state, escalation), None, &employees()).unwrap();
        assert_eq!(archived.attention, InboxAttention::Resolved);
        assert!(archived.actions.is_empty());
    }
    assert!(
        incident_inbox_item(
            &incident(14, IncidentState::Open, IncidentEscalation::None),
            None,
            &employees()
        )
        .is_none()
    );
}

#[test]
fn counts_filters_and_order_follow_attention_then_deadline_then_age() {
    let employees = employees();
    let mut items = vec![
        incident_inbox_item(
            &incident(12, IncidentState::Acknowledged, IncidentEscalation::User),
            None,
            &employees,
        )
        .unwrap(),
        incident_inbox_item(
            &incident(11, IncidentState::Open, IncidentEscalation::Lead),
            None,
            &employees,
        )
        .unwrap(),
        incident_inbox_item(
            &incident(10, IncidentState::Open, IncidentEscalation::User),
            None,
            &employees,
        )
        .unwrap(),
        task_inbox_item(&task(5, TaskState::Uncertain), false, &employees).unwrap(),
        task_inbox_item(&task(4, TaskState::Proposed), false, &employees).unwrap(),
        approval_inbox_item(&approval(1, ApprovalState::Pending), &employees),
    ];
    sort_unresolved_inbox(&mut items);
    let order: Vec<(InboxKind, InboxAttention)> = items
        .iter()
        .map(|item| (item.kind, item.attention))
        .collect();
    assert_eq!(
        order,
        [
            // Counted items first; the approval carries the only deadline among them.
            (InboxKind::Approval, InboxAttention::ActionRequired),
            // Then oldest first: both tasks were created before the incident.
            (InboxKind::TaskProposal, InboxAttention::ActionRequired),
            (InboxKind::TaskUncertain, InboxAttention::ActionRequired),
            (InboxKind::Incident, InboxAttention::ActionRequired),
            (InboxKind::Incident, InboxAttention::Watching),
            (InboxKind::Incident, InboxAttention::Acknowledged),
        ]
    );
    let counts = inbox_counts(&items);
    assert_eq!(counts.unresolved, 4);
    assert_eq!(counts.approvals, 2);
    assert_eq!(counts.permissions, 0);
    assert_eq!(counts.incidents, 2);
    assert_eq!(counts.watching, 1);

    let members = |filter: InboxFilter| {
        items
            .iter()
            .filter(|item| inbox_filter_matches(filter, item))
            .count()
    };
    assert_eq!(members(InboxFilter::All), 6);
    assert_eq!(members(InboxFilter::Approvals), 2);
    assert_eq!(members(InboxFilter::Permissions), 0);
    assert_eq!(members(InboxFilter::Incidents), 4);
    assert_eq!(members(InboxFilter::Resolved), 0);

    let mut archive = vec![
        approval_inbox_item(&approval(2, ApprovalState::Expired), &employees),
        task_inbox_item(&task(7, TaskState::Failed), true, &employees).unwrap(),
    ];
    assert!(
        archive
            .iter()
            .all(|item| inbox_filter_matches(InboxFilter::Resolved, item))
    );
    sort_inbox_archive(&mut archive);
    assert_eq!(
        archive[0].kind,
        InboxKind::TaskUncertain,
        "newest change first"
    );
    assert_eq!(inbox_counts(&archive).unresolved, 0);
}

#[test]
fn cursors_round_trip_and_limits_are_bounded() {
    let item = approval_inbox_item(&approval(1, ApprovalState::Expired), &employees());
    let cursor = InboxCursor::of(&item);
    assert_eq!(
        cursor.render(),
        "2026-10-13T00:01:00Z|00000000-0000-0000-0000-000000000001"
    );
    assert_eq!(InboxCursor::parse(&cursor.render()).unwrap(), cursor);
    for invalid in [
        "",
        "nope",
        "|",
        "2026|not-a-uuid",
        "|00000000-0000-0000-0000-000000000001",
    ] {
        assert_eq!(
            InboxCursor::parse(invalid).unwrap_err(),
            InboxValidationError::Cursor
        );
    }
    assert_eq!(validate_inbox_limit(0), Err(InboxValidationError::Limit));
    assert_eq!(validate_inbox_limit(501), Err(InboxValidationError::Limit));
    assert_eq!(validate_inbox_limit(500), Ok(()));
}
