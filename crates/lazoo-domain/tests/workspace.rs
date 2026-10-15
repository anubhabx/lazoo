use lazoo_domain::{
    ConversationFacts, MAX_WORKER_LABEL_BYTES, UNTITLED_CONVERSATION, WorkerFacts,
    WorkspaceValidationError, active_descendant_ids, conversation_marker, conversation_title,
    lead_actions, lead_lifecycle, lead_startability, turn_in_flight, validate_workspace_limit,
    worker_actions, worker_label, worker_state,
};
use lazoo_protocol::{
    ActivationPolicy, ApprovalKind, ApprovalRecord, ApprovalState, AuthorityMode,
    ConversationMarker, ConversationRecord, EmployeeAvailability, EmployeeCheckoutPolicy,
    EmployeeRecord, EmployeeSessionRecord, InboxAction, IncidentEscalation, IncidentPriority,
    IncidentRecord, IncidentSource, IncidentState, LeadLifecycle, MessageEndpoint, MessageSender,
    RosterEntry, SessionLifecycleState, TaskDispatchRecord, TaskDispatchState, TaskRecord,
    TaskRole, TaskState, TaskSummary, TurnState, WorkerCounts, WorkerState,
};
use serde_json::json;
use uuid::Uuid;

fn entry(availability: EmployeeAvailability, reason: Option<&str>) -> RosterEntry {
    RosterEntry {
        employee: EmployeeRecord {
            id: Uuid::from_u128(1),
            name: "Rhea".to_owned(),
            provider: "codex".to_owned(),
            role: "Lead".to_owned(),
            instructions: String::new(),
            model: None,
            reasoning_effort: None,
            native_settings: json!({}),
            authority_mode: AuthorityMode::Ask,
            checkout_policy: EmployeeCheckoutPolicy::Isolated,
            activation_policy: ActivationPolicy::Manual,
            idle_timeout_seconds: 0,
            enabled: availability != EmployeeAvailability::Disabled,
            configuration_revision: 1,
            created_at: "2026-10-13T00:00:00Z".to_owned(),
            updated_at: "2026-10-13T00:00:00Z".to_owned(),
            disabled_at: None,
        },
        availability,
        availability_reason: reason.map(str::to_owned),
        provider_observation: None,
        session: None,
    }
}

#[test]
fn the_marker_prefers_attention_then_activity_then_failure_then_completion() {
    let quiet = ConversationFacts::default();
    assert_eq!(conversation_marker(&quiet), (None, None));

    let mut facts = ConversationFacts {
        unresolved: 2,
        active_turns: 1,
        workers: WorkerCounts {
            total: 3,
            running: 1,
            waiting: 1,
            failed: 1,
            completed: 0,
        },
        root_task_state: Some(TaskState::Failed),
        last_turn_state: Some(TurnState::TerminalFailed),
    };
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::NeedsAttention),
            Some("2 items need you".to_owned())
        )
    );
    facts.unresolved = 1;
    assert_eq!(
        conversation_marker(&facts).1.as_deref(),
        Some("1 item needs you")
    );
    facts.unresolved = 0;
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::Active),
            Some("1 worker running".to_owned())
        )
    );
    facts.workers.running = 0;
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::Active),
            Some("a turn is in flight".to_owned())
        )
    );
    facts.active_turns = 0;
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::Active),
            Some("1 waiting to run".to_owned())
        )
    );
    facts.workers.waiting = 0;
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::Failed),
            Some("the plan failed".to_owned())
        )
    );
    facts.root_task_state = Some(TaskState::Completed);
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::Failed),
            Some("the last turn failed".to_owned())
        )
    );
    facts.last_turn_state = Some(TurnState::Completed);
    assert_eq!(
        conversation_marker(&facts),
        (
            Some(ConversationMarker::Completed),
            Some("the plan completed".to_owned())
        )
    );
    facts.root_task_state = Some(TaskState::Cancelled);
    assert_eq!(conversation_marker(&facts), (None, None));
}

#[test]
fn titles_come_from_the_store_then_the_first_request_then_a_default() {
    assert_eq!(
        conversation_title(Some("  Parser fix  "), Some("ignored")),
        ("Parser fix".to_owned(), false)
    );
    assert_eq!(
        conversation_title(Some("   "), Some("\n  Fix the parser\nand more")),
        ("Fix the parser".to_owned(), true)
    );
    let long = "x".repeat(200);
    let (title, derived) = conversation_title(None, Some(&long));
    assert!(derived);
    assert_eq!(title.len(), 80);
    assert_eq!(
        conversation_title(None, None),
        (UNTITLED_CONVERSATION.to_owned(), true)
    );
    assert_eq!(
        conversation_title(None, Some("   \n  ")),
        (UNTITLED_CONVERSATION.to_owned(), true)
    );
}

#[test]
fn startability_follows_roster_availability_with_lazoo_words_for_each_block() {
    for availability in [
        EmployeeAvailability::Dormant,
        EmployeeAvailability::Idle,
        EmployeeAvailability::Suspended,
    ] {
        assert_eq!(lead_startability(&entry(availability, None)), (true, None));
    }
    assert_eq!(
        lead_startability(&entry(
            EmployeeAvailability::Disabled,
            Some("employee is disabled")
        )),
        (false, Some("Rhea is disabled".to_owned()))
    );
    assert_eq!(
        lead_startability(&entry(EmployeeAvailability::Starting, None)),
        (false, Some("Rhea is starting".to_owned()))
    );
    assert_eq!(
        lead_startability(&entry(
            EmployeeAvailability::Offline,
            Some("provider executable was not detected")
        )),
        (
            false,
            Some("Rhea's harness is offline: provider executable was not detected".to_owned())
        )
    );
    assert_eq!(
        lead_startability(&entry(EmployeeAvailability::Unverified, None)),
        (
            false,
            Some("Rhea's harness is unverified: not authenticated or compatible".to_owned())
        )
    );
    assert_eq!(
        lead_startability(&entry(
            EmployeeAvailability::Failed,
            Some("LZ-SESSION-OWNER-LOST")
        )),
        (
            false,
            Some("Rhea's last session failed: LZ-SESSION-OWNER-LOST".to_owned())
        )
    );
    assert_eq!(
        validate_workspace_limit(0),
        Err(WorkspaceValidationError::Limit)
    );
    assert_eq!(
        validate_workspace_limit(501),
        Err(WorkspaceValidationError::Limit)
    );
    assert_eq!(validate_workspace_limit(200), Ok(()));
}

fn session(state: SessionLifecycleState) -> EmployeeSessionRecord {
    EmployeeSessionRecord {
        id: Uuid::from_u128(9),
        employee_id: Uuid::from_u128(1),
        employee_configuration_revision: 1,
        provider: "codex".to_owned(),
        state,
        revision: 1,
        owner_boot_id: None,
        native_session_id: None,
        native_host_pid: None,
        failure_code: None,
        failure_message: None,
        created_at: "2026-10-13T00:00:00Z".to_owned(),
        updated_at: "2026-10-13T00:00:00Z".to_owned(),
        started_at: None,
        stopped_at: None,
    }
}

fn task(state: TaskState) -> TaskRecord {
    TaskRecord {
        sequence: 2,
        version: 1,
        id: Uuid::from_u128(20),
        root_task_id: Uuid::from_u128(10),
        parent_task_id: Some(Uuid::from_u128(10)),
        depth: 1,
        project_id: None,
        conversation_id: Uuid::from_u128(5),
        creation_turn_id: None,
        delegation_message_id: None,
        creator: MessageSender::Employee(MessageEndpoint {
            employee_id: Uuid::from_u128(1),
            session_id: None,
        }),
        assigned_employee_id: Uuid::from_u128(2),
        role: TaskRole::Worker,
        objective: "  Implement the parser fix\nwith tests".to_owned(),
        references: Vec::new(),
        requested_authority: AuthorityMode::Ask,
        effective_authority: AuthorityMode::Ask,
        employee_configuration_revision: 1,
        roster_revision: 1,
        state,
        revision: 1,
        active_turn_id: None,
        active_session_id: None,
        failure_code: None,
        failure_message: None,
        created_at: "2026-10-13T00:00:00Z".to_owned(),
        updated_at: "2026-10-13T00:00:00Z".to_owned(),
        terminal_at: None,
    }
}

fn dispatch(state: TaskDispatchState) -> TaskDispatchRecord {
    TaskDispatchRecord {
        task_id: Uuid::from_u128(20),
        message_id: Uuid::from_u128(30),
        owner_boot_id: Uuid::from_u128(40),
        project_id: None,
        project_configuration_revision: None,
        workspace_kind: None,
        workspace_path: None,
        base_commit: None,
        branch_name: None,
        turn_id: None,
        session_id: None,
        state,
        revision: 1,
        failure_code: None,
        failure_message: None,
        created_at: "2026-10-13T00:00:00Z".to_owned(),
        updated_at: "2026-10-13T00:00:00Z".to_owned(),
    }
}

fn approval() -> ApprovalRecord {
    ApprovalRecord {
        sequence: 1,
        version: 1,
        id: Uuid::from_u128(50),
        turn_id: Uuid::from_u128(60),
        task_id: Some(Uuid::from_u128(20)),
        conversation_id: Uuid::from_u128(5),
        project_id: None,
        employee_id: Uuid::from_u128(2),
        session_id: None,
        owner_boot_id: Uuid::from_u128(40),
        provider: "codex".to_owned(),
        native_session_id: None,
        native_turn_id: None,
        native_request_id: None,
        kind: ApprovalKind::CommandExecution,
        action: "cargo test".to_owned(),
        detail: json!({}),
        fingerprint: "f".to_owned(),
        authority: AuthorityMode::Ask,
        scope_expansion: false,
        state: ApprovalState::Pending,
        decider: None,
        revision: 1,
        failure_code: None,
        failure_message: None,
        created_at: "2026-10-13T00:00:00Z".to_owned(),
        expires_at: "2026-10-13T00:10:00Z".to_owned(),
        updated_at: "2026-10-13T00:00:00Z".to_owned(),
        decided_at: None,
    }
}

fn incident(escalation: IncidentEscalation) -> IncidentRecord {
    IncidentRecord {
        sequence: 1,
        version: 1,
        id: Uuid::from_u128(70),
        code: "LZ-SESSION-STALLED".to_owned(),
        fingerprint: "f".to_owned(),
        title: "Mira's session stalled".to_owned(),
        explanation: String::new(),
        next_action: String::new(),
        project_id: None,
        employee_id: Some(Uuid::from_u128(2)),
        provider: None,
        task_id: Some(Uuid::from_u128(20)),
        session_id: None,
        turn_id: None,
        message_id: None,
        source: IncidentSource::Daemon,
        detail: json!({}),
        occurrences: 1,
        window_occurrences: 1,
        first_occurred_at: "2026-10-13T00:00:00Z".to_owned(),
        last_occurred_at: "2026-10-13T00:00:00Z".to_owned(),
        window_started_at: "2026-10-13T00:00:00Z".to_owned(),
        priority: IncidentPriority::Priority,
        state: IncidentState::Open,
        revision: 1,
        created_at: "2026-10-13T00:00:00Z".to_owned(),
        updated_at: "2026-10-13T00:00:00Z".to_owned(),
        acknowledged_at: None,
        resolved_at: None,
        escalation,
        remediation_deadline_at: None,
        escalated_at: None,
        escalation_reason: None,
        remedies: Vec::new(),
        user_notification: None,
        user_notified_at: None,
    }
}

fn facts(task: &TaskRecord) -> WorkerFacts<'_> {
    WorkerFacts {
        task,
        roster: None,
        dispatch: None,
        active_turn_state: None,
        pending_approval: None,
        blocking_incident: None,
    }
}

#[test]
fn the_lead_lifecycle_follows_the_session_and_its_newest_turn() {
    assert_eq!(lead_lifecycle(None, None), LeadLifecycle::Dormant);
    assert_eq!(
        lead_lifecycle(None, Some(TurnState::Uncertain)),
        LeadLifecycle::Unknown
    );
    let idle = session(SessionLifecycleState::Idle);
    assert_eq!(lead_lifecycle(Some(&idle), None), LeadLifecycle::Idle);
    assert_eq!(
        lead_lifecycle(Some(&idle), Some(TurnState::Completed)),
        LeadLifecycle::Idle
    );
    for state in [
        TurnState::Recorded,
        TurnState::Submitting,
        TurnState::Accepted,
    ] {
        assert!(turn_in_flight(state));
        assert_eq!(
            lead_lifecycle(Some(&idle), Some(state)),
            LeadLifecycle::Busy
        );
    }
    assert!(!turn_in_flight(TurnState::RetryableFailed));
    for (state, lifecycle) in [
        (SessionLifecycleState::Starting, LeadLifecycle::Starting),
        (SessionLifecycleState::Stopping, LeadLifecycle::Stopping),
        (SessionLifecycleState::Suspended, LeadLifecycle::Suspended),
        (SessionLifecycleState::Failed, LeadLifecycle::Failed),
    ] {
        assert_eq!(
            lead_lifecycle(Some(&session(state)), Some(TurnState::Accepted)),
            lifecycle
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)] // The whole state table, one rule after another.
fn worker_cards_take_the_first_state_that_holds_with_lazoo_words() {
    let state = |task: &TaskRecord| worker_state(&facts(task));

    assert_eq!(
        state(&task(TaskState::Uncertain)),
        (
            WorkerState::Unknown,
            "The outcome is uncertain: decide how the task ended".to_owned()
        )
    );
    let pending = approval();
    let running = task(TaskState::Running);
    assert_eq!(
        worker_state(&WorkerFacts {
            pending_approval: Some(&pending),
            ..facts(&running)
        }),
        (
            WorkerState::NeedsApproval,
            "Waiting for your approval: cargo test".to_owned()
        )
    );
    assert_eq!(
        state(&task(TaskState::CancelRequested)),
        (
            WorkerState::Interrupted,
            "Being interrupted: cancel requested".to_owned()
        )
    );
    assert_eq!(
        state(&task(TaskState::Cancelled)),
        (
            WorkerState::Interrupted,
            "Interrupted: the task was cancelled".to_owned()
        )
    );
    let mut failed = task(TaskState::Failed);
    assert_eq!(
        state(&failed),
        (WorkerState::Failed, "Failed: see the task".to_owned())
    );
    failed.failure_code = Some("LZ-PROCESS-EXIT".to_owned());
    assert_eq!(state(&failed).1, "Failed: LZ-PROCESS-EXIT");
    assert_eq!(
        state(&task(TaskState::Completed)),
        (WorkerState::Complete, "Completed".to_owned())
    );
    assert_eq!(
        state(&task(TaskState::Proposed)),
        (
            WorkerState::Blocked,
            "Waiting for you to accept the task".to_owned()
        )
    );

    // Blocked by the worker's availability, then by an escalated incident; `starting` is not
    // a block.
    let accepted = task(TaskState::Accepted);
    let disabled = entry(EmployeeAvailability::Disabled, Some("employee is disabled"));
    assert_eq!(
        worker_state(&WorkerFacts {
            roster: Some(&disabled),
            ..facts(&accepted)
        }),
        (WorkerState::Blocked, "Blocked: Rhea is disabled".to_owned())
    );
    let starting = entry(EmployeeAvailability::Starting, None);
    assert_eq!(
        worker_state(&WorkerFacts {
            roster: Some(&starting),
            ..facts(&accepted)
        })
        .0,
        WorkerState::Waiting
    );
    let lead_window = incident(IncidentEscalation::Lead);
    assert_eq!(
        worker_state(&WorkerFacts {
            blocking_incident: Some(&lead_window),
            ..facts(&running)
        }),
        (
            WorkerState::Blocked,
            "Blocked: Mira's session stalled (escalated to the lead)".to_owned()
        )
    );
    let escalated = incident(IncidentEscalation::User);
    assert_eq!(
        worker_state(&WorkerFacts {
            blocking_incident: Some(&escalated),
            ..facts(&running)
        })
        .1,
        "Blocked: Mira's session stalled (escalated to you)"
    );

    // Running needs an accepted or submitting turn; otherwise the worker waits.
    assert_eq!(
        worker_state(&WorkerFacts {
            active_turn_state: Some(TurnState::Accepted),
            ..facts(&running)
        }),
        (
            WorkerState::Running,
            "Running: turn accepted by the harness".to_owned()
        )
    );
    assert_eq!(
        worker_state(&WorkerFacts {
            active_turn_state: Some(TurnState::Submitting),
            ..facts(&running)
        })
        .1,
        "Running: turn submitting"
    );
    assert_eq!(
        worker_state(&WorkerFacts {
            active_turn_state: Some(TurnState::Completed),
            ..facts(&running)
        }),
        (WorkerState::Waiting, "Waiting between turns".to_owned())
    );
    assert_eq!(
        state(&accepted),
        (
            WorkerState::Waiting,
            "Waiting to be dispatched when the worker is free".to_owned()
        )
    );
    for (dispatch_state, words) in [
        (TaskDispatchState::Preparing, "Preparing the workspace"),
        (
            TaskDispatchState::WorkspaceReady,
            "Starting the worker's session",
        ),
        (
            TaskDispatchState::TurnRecorded,
            "Waiting for the first turn to start",
        ),
        (
            TaskDispatchState::Failed,
            "Waiting after an unfinished dispatch",
        ),
    ] {
        let record = dispatch(dispatch_state);
        assert_eq!(
            worker_state(&WorkerFacts {
                dispatch: Some(&record),
                ..facts(&accepted)
            }),
            (WorkerState::Waiting, words.to_owned())
        );
    }

    // The label is the objective's first line, bounded.
    assert_eq!(
        worker_label(&accepted.objective),
        "Implement the parser fix"
    );
    assert_eq!(worker_label(&"y".repeat(300)).len(), MAX_WORKER_LABEL_BYTES);
    assert_eq!(worker_label("  \n "), "");
}

#[test]
#[allow(clippy::too_many_lines)] // Every card state and every header condition, in one table.
fn cards_and_the_header_offer_only_the_actions_their_rows_allow() {
    let names = |actions: &[InboxAction]| {
        actions
            .iter()
            .map(|action| format!("{}={}", action.action, action.method))
            .collect::<Vec<_>>()
    };

    // Cards: a proposal, an uncertain task, a waiting or running task, a pending approval on
    // top, and nothing once terminal or already cancelling.
    let proposed = task(TaskState::Proposed);
    let actions = worker_actions(&proposed, None, false);
    assert_eq!(
        names(&actions),
        ["accept=task.accept", "cancel=task.cancel"]
    );
    assert_eq!(actions[0].params["expectedRevision"], 1);
    assert_eq!(actions[1].params["actor"], json!({"kind": "user"}));
    assert_eq!(
        names(&worker_actions(&task(TaskState::Uncertain), None, false)),
        [
            "resolve-completed=task.resolve",
            "resolve-failed=task.resolve",
            "resolve-cancelled=task.resolve"
        ]
    );
    for state in [TaskState::Accepted, TaskState::Running] {
        assert_eq!(
            names(&worker_actions(&task(state), None, false)),
            ["cancel=task.cancel"]
        );
    }
    let pending = approval();
    assert_eq!(
        names(&worker_actions(
            &task(TaskState::Running),
            Some(&pending),
            false
        )),
        [
            "approve=approval.decide",
            "decline=approval.decide",
            "cancel=task.cancel"
        ]
    );
    let mut decided = approval();
    decided.state = ApprovalState::Approved;
    assert_eq!(
        names(&worker_actions(
            &task(TaskState::Running),
            Some(&decided),
            false
        )),
        ["cancel=task.cancel"]
    );
    for state in [
        TaskState::CancelRequested,
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Cancelled,
    ] {
        assert!(
            worker_actions(&task(state), None, false).is_empty(),
            "{state:?}"
        );
    }
    // Active descendants take away what the store would refuse: cancel, and the completed or
    // cancelled resolutions; the approval's decisions and a failed resolution stay.
    assert!(worker_actions(&task(TaskState::Running), None, true).is_empty());
    assert_eq!(
        names(&worker_actions(
            &task(TaskState::Running),
            Some(&pending),
            true
        )),
        ["approve=approval.decide", "decline=approval.decide"]
    );
    assert_eq!(
        names(&worker_actions(&task(TaskState::Uncertain), None, true)),
        ["resolve-failed=task.resolve"]
    );

    // The header: rename always; cancel an unfinished or retryable turn; retry a retryable
    // one; stop an idle session only when no turn is unfinished.
    let conversation = ConversationRecord {
        sequence: 1,
        version: 1,
        id: Uuid::from_u128(5),
        project_id: None,
        lead_employee_id: Uuid::from_u128(1),
        lead_employee_configuration_revision: 1,
        title: None,
        revision: 3,
        created_at: "2026-10-13T00:00:00Z".to_owned(),
        updated_at: "2026-10-13T00:00:00Z".to_owned(),
    };
    let rename_only = lead_actions(&conversation, None, None);
    assert_eq!(names(&rename_only), ["rename=conversation.rename"]);
    assert_eq!(
        rename_only[0].params,
        json!({"id": conversation.id, "expectedRevision": 3, "title": null})
    );
    let idle = session(SessionLifecycleState::Idle);
    assert_eq!(
        names(&lead_actions(
            &conversation,
            Some(&idle),
            Some((Uuid::from_u128(60), TurnState::Completed, 4))
        )),
        ["rename=conversation.rename", "stop-session=session.stop"]
    );
    let busy = lead_actions(
        &conversation,
        Some(&idle),
        Some((Uuid::from_u128(60), TurnState::Accepted, 4)),
    );
    assert_eq!(
        names(&busy),
        ["rename=conversation.rename", "cancel-turn=turn.cancel"]
    );
    assert_eq!(
        busy[1].params,
        json!({"id": Uuid::from_u128(60), "expectedRevision": 4})
    );
    assert_eq!(
        names(&lead_actions(
            &conversation,
            Some(&idle),
            Some((Uuid::from_u128(60), TurnState::RetryableFailed, 4))
        )),
        [
            "rename=conversation.rename",
            "cancel-turn=turn.cancel",
            "retry-turn=turn.retry",
            "stop-session=session.stop"
        ]
    );
    // A retry needs an idle session to be delivered on; a suspended or absent one offers only
    // the cancel.
    for session in [None, Some(session(SessionLifecycleState::Suspended))] {
        assert_eq!(
            names(&lead_actions(
                &conversation,
                session.as_ref(),
                Some((Uuid::from_u128(60), TurnState::RetryableFailed, 4))
            )),
            ["rename=conversation.rename", "cancel-turn=turn.cancel"]
        );
    }
    assert_eq!(
        names(&lead_actions(
            &conversation,
            Some(&session(SessionLifecycleState::Suspended)),
            None
        )),
        ["rename=conversation.rename"]
    );
}

#[test]
fn active_descendants_are_found_through_every_ancestor() {
    let summary = |id: u64, parent: Option<u64>, state: TaskState| TaskSummary {
        sequence: id,
        id: Uuid::from_u128(u128::from(id)),
        root_task_id: Uuid::from_u128(1),
        parent_task_id: parent.map(|parent| Uuid::from_u128(u128::from(parent))),
        depth: u32::from(parent.is_some()),
        project_id: None,
        conversation_id: Uuid::from_u128(5),
        assigned_employee_id: Uuid::from_u128(2),
        role: TaskRole::Worker,
        objective: String::new(),
        requested_authority: AuthorityMode::Ask,
        effective_authority: AuthorityMode::Ask,
        state,
        revision: 1,
        created_at: "2026-10-13T00:00:00Z".to_owned(),
        updated_at: "2026-10-13T00:00:00Z".to_owned(),
        terminal_at: None,
    };
    // root 1 > 2 (running) > 3 (accepted); root 1 > 4 (running) > 5 (completed); root 6 alone.
    let tasks = [
        summary(1, None, TaskState::Running),
        summary(2, Some(1), TaskState::Running),
        summary(3, Some(2), TaskState::Accepted),
        summary(4, Some(1), TaskState::Running),
        summary(5, Some(4), TaskState::Completed),
        summary(6, None, TaskState::Completed),
    ];
    let blocked = active_descendant_ids(&tasks);
    assert_eq!(
        blocked.into_iter().collect::<Vec<_>>(),
        [Uuid::from_u128(1), Uuid::from_u128(2)]
    );
    assert!(active_descendant_ids(&[]).is_empty());
}
