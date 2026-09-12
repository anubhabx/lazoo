use lazoo_protocol::{
    AcceptTaskParams, AuthorityMode, CancelTaskParams, DelegateTaskParams, MessageEndpoint,
    MessageSender, ReportTaskParams, ResolveTaskParams, StartTaskParams, TASK_CONTRACT_V1,
    TaskDetail, TaskDispatchRecord, TaskDispatchState, TaskEvidence, TaskEvidenceKind,
    TaskListParams, TaskMutationResult, TaskRecord, TaskReportKind, TaskResolution, TaskRole,
    TaskShowParams, TaskState, TaskSummary, TaskWorkspaceKind,
};
use serde_json::{Value, json};
use uuid::Uuid;

fn employee_endpoint(employee_id: Uuid) -> MessageEndpoint {
    MessageEndpoint {
        employee_id,
        session_id: None,
    }
}

fn task_record() -> TaskRecord {
    let task_id = Uuid::from_u128(1);
    let conversation_id = Uuid::from_u128(2);
    let employee_id = Uuid::from_u128(3);

    TaskRecord {
        sequence: 7,
        version: TASK_CONTRACT_V1,
        id: task_id,
        root_task_id: task_id,
        parent_task_id: None,
        depth: 0,
        project_id: None,
        conversation_id,
        creation_turn_id: Some(task_id),
        delegation_message_id: None,
        creator: MessageSender::Employee(employee_endpoint(employee_id)),
        assigned_employee_id: employee_id,
        role: TaskRole::Lead,
        objective: "Ship the delegation graph".to_owned(),
        references: Vec::new(),
        requested_authority: AuthorityMode::SelfDecision,
        effective_authority: AuthorityMode::Ask,
        employee_configuration_revision: 4,
        roster_revision: 9,
        state: TaskState::Running,
        revision: 2,
        active_turn_id: Some(task_id),
        active_session_id: None,
        failure_code: None,
        failure_message: None,
        created_at: "2026-09-04T10:00:00Z".to_owned(),
        updated_at: "2026-09-04T10:01:00Z".to_owned(),
        terminal_at: None,
    }
}

#[test]
fn task_enums_use_stable_kebab_case_wire_values() {
    assert_eq!(TASK_CONTRACT_V1, 1);
    assert_eq!(serde_json::to_value(TaskRole::Lead).unwrap(), json!("lead"));
    assert_eq!(
        serde_json::to_value(TaskRole::Worker).unwrap(),
        json!("worker")
    );
    assert_eq!(
        serde_json::to_value(TaskState::CancelRequested).unwrap(),
        json!("cancel-requested")
    );
    assert_eq!(
        serde_json::to_value(TaskEvidenceKind::Progress).unwrap(),
        json!("progress")
    );
    assert_eq!(
        serde_json::to_value(TaskReportKind::Completed).unwrap(),
        json!("completed")
    );
    assert_eq!(
        serde_json::to_value(TaskResolution::Cancelled).unwrap(),
        json!("cancelled")
    );
    assert_eq!(
        serde_json::to_value(TaskDispatchState::WorkspaceReady).unwrap(),
        json!("workspace-ready")
    );
    assert_eq!(
        serde_json::to_value(TaskDispatchState::Cancelled).unwrap(),
        json!("cancelled")
    );
    assert_eq!(
        serde_json::to_value(TaskWorkspaceKind::GitWorktree).unwrap(),
        json!("git-worktree")
    );
}

#[test]
fn task_commands_are_strict_camel_case_contracts_with_safe_defaults() {
    let parent_task_id = Uuid::from_u128(1);
    let employee_id = Uuid::from_u128(3);
    let turn_id = Uuid::from_u128(4);

    let delegate: DelegateTaskParams = serde_json::from_value(json!({
        "parentTaskId": parent_task_id,
        "actor": { "employeeId": employee_id, "sessionId": null },
        "assignedEmployeeId": employee_id,
        "objective": "Investigate the failing gate",
        "rosterRevision": 8,
        "idempotencyKey": "delegate-1"
    }))
    .unwrap();
    assert!(delegate.references.is_empty());
    assert_eq!(delegate.requested_authority, None);

    let start: StartTaskParams = serde_json::from_value(json!({
        "id": parent_task_id,
        "expectedRevision": 1,
        "actor": { "employeeId": employee_id, "sessionId": null },
        "turnId": turn_id,
        "idempotencyKey": "start-1"
    }))
    .unwrap();
    assert_eq!(start.evidence, json!({}));

    let report: ReportTaskParams = serde_json::from_value(json!({
        "id": parent_task_id,
        "expectedRevision": 2,
        "actor": { "employeeId": employee_id, "sessionId": null },
        "kind": "progress",
        "failureCode": null,
        "failureMessage": null,
        "idempotencyKey": "report-1"
    }))
    .unwrap();
    assert_eq!(report.evidence, json!({}));

    let cancel: CancelTaskParams = serde_json::from_value(json!({
        "id": parent_task_id,
        "expectedRevision": 3,
        "actor": { "kind": "user" },
        "idempotencyKey": "cancel-1"
    }))
    .unwrap();
    assert_eq!(cancel.evidence, json!({}));

    let resolve: ResolveTaskParams = serde_json::from_value(json!({
        "id": parent_task_id,
        "expectedRevision": 4,
        "resolution": "completed",
        "failureCode": null,
        "failureMessage": null,
        "idempotencyKey": "resolve-1"
    }))
    .unwrap();
    assert_eq!(resolve.evidence, json!({}));

    let accept = AcceptTaskParams {
        id: parent_task_id,
        expected_revision: 0,
        idempotency_key: "accept-1".to_owned(),
    };
    assert_eq!(
        serde_json::to_value(accept).unwrap(),
        json!({
            "id": parent_task_id,
            "expectedRevision": 0,
            "idempotencyKey": "accept-1"
        })
    );

    let unknown = serde_json::from_value::<AcceptTaskParams>(json!({
        "id": parent_task_id,
        "expectedRevision": 0,
        "idempotencyKey": "accept-2",
        "unexpected": true
    }));
    assert!(unknown.is_err());
}

#[test]
fn task_read_models_preserve_identity_lineage_and_evidence() {
    let task = task_record();
    let evidence = TaskEvidence {
        sequence: 8,
        id: Uuid::from_u128(5),
        task_id: task.id,
        task_revision: task.revision,
        kind: TaskEvidenceKind::Progress,
        actor: task.creator,
        message_id: None,
        turn_id: task.active_turn_id,
        session_id: None,
        detail: json!({ "gate": "cargo test" }),
        created_at: "2026-09-04T10:01:00Z".to_owned(),
    };
    let summary = TaskSummary {
        sequence: task.sequence,
        id: task.id,
        root_task_id: task.root_task_id,
        parent_task_id: task.parent_task_id,
        depth: task.depth,
        project_id: task.project_id,
        conversation_id: task.conversation_id,
        assigned_employee_id: task.assigned_employee_id,
        role: task.role,
        objective: task.objective.clone(),
        requested_authority: task.requested_authority,
        effective_authority: task.effective_authority,
        state: task.state,
        revision: task.revision,
        created_at: task.created_at.clone(),
        updated_at: task.updated_at.clone(),
        terminal_at: task.terminal_at.clone(),
    };
    let detail = TaskDetail {
        task: task.clone(),
        children: vec![summary],
        evidence: vec![evidence],
        dispatch: Some(TaskDispatchRecord {
            task_id: task.id,
            message_id: Uuid::from_u128(5),
            owner_boot_id: Uuid::from_u128(6),
            project_id: None,
            project_configuration_revision: None,
            workspace_kind: Some(TaskWorkspaceKind::Projectless),
            workspace_path: Some(r"C:\Lazoo\task-workspaces\projectless\1".to_owned()),
            base_commit: None,
            branch_name: None,
            turn_id: Some(Uuid::from_u128(7)),
            session_id: Some(Uuid::from_u128(8)),
            state: TaskDispatchState::TurnRecorded,
            revision: 3,
            failure_code: None,
            failure_message: None,
            created_at: "2026-09-04T10:00:30Z".to_owned(),
            updated_at: "2026-09-04T10:00:45Z".to_owned(),
        }),
    };
    let wire = serde_json::to_value(detail).unwrap();

    assert_eq!(wire["task"]["rootTaskId"], json!(task.id));
    assert_eq!(wire["task"]["creationTurnId"], json!(task.id));
    assert_eq!(wire["children"][0]["effectiveAuthority"], json!("ask"));
    assert_eq!(wire["evidence"][0]["taskRevision"], json!(2));
    assert_eq!(wire["dispatch"]["state"], json!("turn-recorded"));
    assert_eq!(
        wire["evidence"][0]["detail"],
        json!({ "gate": "cargo test" })
    );

    let result = TaskMutationResult {
        task,
        delegation_message: None,
        event_sequence: Some(9),
    };
    assert_eq!(
        serde_json::to_value(result).unwrap()["delegationMessage"],
        Value::Null
    );
}

#[test]
fn task_list_defaults_and_filters_are_explicit() {
    let defaults: TaskListParams = serde_json::from_value(json!({})).unwrap();
    assert_eq!(defaults, TaskListParams::default());
    assert_eq!(defaults.after_sequence, 0);
    assert_eq!(defaults.limit, 100);

    let task_id = Uuid::from_u128(1);
    let filtered: TaskListParams = serde_json::from_value(json!({
        "afterSequence": 10,
        "limit": 25,
        "rootTaskId": task_id,
        "parentTaskId": null,
        "conversationId": null,
        "projectId": null,
        "assignedEmployeeId": null,
        "state": "uncertain"
    }))
    .unwrap();
    assert_eq!(filtered.root_task_id, Some(task_id));
    assert_eq!(filtered.state, Some(TaskState::Uncertain));

    let show = TaskShowParams { id: task_id };
    assert_eq!(
        serde_json::to_value(show).unwrap(),
        json!({ "id": task_id })
    );
}
