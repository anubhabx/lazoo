use lazoo_protocol::{
    AuthorityMode, CheckoutMode, EmployeeCheckoutPolicy, MessageDeliveryState, MessageEndpoint,
    MessageSender, ProviderObservation, SessionLifecycleState, TaskEvidenceKind, TaskState,
    TurnState, UserMessageEndpoint, UserMessageKind,
};
use lazoo_store::{
    AcceptTaskInput, BeginSessionInput, BeginTurnSubmissionInput, CancelTaskInput,
    CreateConversationInput, CreateEmployeeInput, DelegateTaskInput,
    FailRecordedRootTurnBeforeProviderInput, FailTurnInput, FinishTurnCancelInput,
    MarkSessionReadyInput, MarkTurnUncertainInput, ObserveTurnInput, RecordTurnInput,
    RegisterProjectInput, RequestTurnCancelInput, RetryTurnInput, Store, StoreError,
};
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

fn employee(name: &str, enabled: bool, route_ready: bool) -> CreateEmployeeInput {
    CreateEmployeeInput {
        request_id: format!("create-{name}"),
        idempotency_key: format!("create-{name}"),
        name: name.to_owned(),
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
        enabled,
        provider_observation: route_ready.then(|| ProviderObservation {
            provider: "codex".to_owned(),
            verification: "configured".to_owned(),
            version: Some("0.144.1".to_owned()),
            protocol: Some("app-server/v2".to_owned()),
            capabilities: json!({"session": true}),
            authentication: json!({"native": "ready", "external": "unknown"}),
            degradation_reason: None,
            checked_at: "2026-09-03T00:00:00Z".to_owned(),
        }),
    }
}

fn create_conversation(
    store: &Store,
    key: &str,
    lead_employee_id: Uuid,
    project_id: Option<Uuid>,
) -> lazoo_protocol::ConversationMutationResult {
    store
        .create_conversation(&CreateConversationInput {
            request_id: format!("request-{key}"),
            idempotency_key: key.to_owned(),
            project_id,
            lead_employee_id,
            title: Some("  First conversation  ".to_owned()),
        })
        .unwrap()
}

fn user() -> MessageSender {
    MessageSender::User(UserMessageEndpoint {
        kind: UserMessageKind::User,
    })
}

#[test]
fn user_queue_is_durable_fifo_across_conversations_and_cancelled_work_is_skipped() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("queue.db");
    let store = Store::open_path(&path).unwrap();
    let lead = store
        .create_employee(&employee("queue-lead", true, true))
        .unwrap()
        .employee;
    let owner = Uuid::new_v4();
    let session_id = warm_session(&store, lead.id, owner);
    let first_conversation = create_conversation(&store, "queue-first", lead.id, None).conversation;
    let second_conversation =
        create_conversation(&store, "queue-second", lead.id, None).conversation;
    let unqueued = record_turn(
        &store,
        "unqueued",
        first_conversation.id,
        user(),
        None,
        "explicit replay only",
    );
    let first = record_turn(
        &store,
        "queued-1",
        first_conversation.id,
        user(),
        None,
        "first",
    );
    let second = record_turn(
        &store,
        "queued-2",
        second_conversation.id,
        user(),
        None,
        "second",
    );
    for turn in [&first, &second] {
        store.queue_user_turn(turn.turn.id).unwrap();
    }
    drop(store);
    let store = Store::open_path(&path).unwrap();
    let pending = store.pending_user_turns(10).unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|result| result.turn.id)
            .collect::<Vec<_>>(),
        [first.turn.id, second.turn.id]
    );
    assert!(!store.turn_recipient_is_busy(first.turn.id).unwrap());
    let input = BeginTurnSubmissionInput {
        request_id: "queued-begin".to_owned(),
        idempotency_key: "queued-begin".to_owned(),
        turn_id: second.turn.id,
        expected_revision: 1,
        owner_boot_id: owner,
        recipient_session_id: session_id,
    };
    assert!(matches!(
        store.begin_turn_submission(&input),
        Err(StoreError::TurnRecipientBusy { .. })
    ));
    assert!(
        store
            .get_message(second.turn.message_id)
            .unwrap()
            .attempts
            .is_empty()
    );
    store
        .request_turn_cancel(&RequestTurnCancelInput {
            request_id: "cancel-first".to_owned(),
            idempotency_key: "cancel-first".to_owned(),
            turn_id: first.turn.id,
            expected_revision: 1,
        })
        .unwrap();
    let submitting = store.begin_turn_submission(&input).unwrap();
    assert_eq!(submitting.turn.state, TurnState::Submitting);
    assert!(store.turn_recipient_is_busy(unqueued.turn.id).unwrap());
    assert!(store.pending_user_turns(10).unwrap().is_empty());
}

fn record_turn(
    store: &Store,
    key: &str,
    conversation_id: Uuid,
    author: MessageSender,
    recipient: Option<MessageEndpoint>,
    text: &str,
) -> lazoo_protocol::TurnMutationResult {
    store
        .record_turn(&RecordTurnInput {
            request_id: format!("request-{key}"),
            idempotency_key: key.to_owned(),
            conversation_id,
            author,
            recipient,
            task_id: None,
            authority_mode: AuthorityMode::Ask,
            text: text.to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap()
}

#[test]
fn recorded_root_can_be_terminally_refused_before_provider_submission() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("state.db")).unwrap();
    let employee = store
        .create_employee(&employee("lead", true, true))
        .unwrap()
        .employee;
    let conversation =
        create_conversation(&store, "refused-root-conversation", employee.id, None).conversation;
    let recorded = record_turn(
        &store,
        "refused-root",
        conversation.id,
        user(),
        None,
        "a root that cannot reach the provider",
    );

    let refused = store
        .fail_recorded_root_turn_before_provider(&FailRecordedRootTurnBeforeProviderInput {
            request_id: "refuse-root-request".to_owned(),
            idempotency_key: "refuse-root".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: recorded.turn.revision,
            failure_code: "LZ-TASK-AUTHORITY-UNSUPPORTED".to_owned(),
            failure_message: "the selected provider cannot enforce this task authority ceiling"
                .to_owned(),
        })
        .unwrap();
    assert_eq!(refused.turn.state, TurnState::TerminalFailed);
    assert_eq!(refused.turn.revision, 2);
    assert!(refused.turn.submitted_at.is_none());
    assert!(refused.turn.accepted_at.is_none());
    assert!(refused.turn.native_turn_id.is_none());
    assert!(refused.turn.events.is_empty());
    assert_eq!(
        refused.message.delivery.state,
        MessageDeliveryState::TerminalFailed
    );
    let root = store.get_task(recorded.turn.id).unwrap().task;
    assert_eq!(root.state, TaskState::Failed);
    assert!(root.active_turn_id.is_none());
    let failed_evidence = store
        .get_task(recorded.turn.id)
        .unwrap()
        .evidence
        .pop()
        .unwrap();
    assert_eq!(failed_evidence.kind, TaskEvidenceKind::Failed);
    assert_eq!(failed_evidence.actor, user());
    assert_eq!(failed_evidence.session_id, None);
    assert_eq!(
        failed_evidence.detail["source"],
        "daemon-pre-provider-rejection"
    );

    let replay = store
        .fail_recorded_root_turn_before_provider(&FailRecordedRootTurnBeforeProviderInput {
            request_id: "refuse-root-replay-request".to_owned(),
            idempotency_key: "refuse-root".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: recorded.turn.revision,
            failure_code: "LZ-TASK-AUTHORITY-UNSUPPORTED".to_owned(),
            failure_message: "the selected provider cannot enforce this task authority ceiling"
                .to_owned(),
        })
        .unwrap();
    assert_eq!(replay.turn, refused.turn);
    assert_eq!(replay.event_sequence, None);
}

#[test]
fn refusing_a_recorded_root_leaves_stale_submitting_and_non_root_turns_untouched() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("state.db")).unwrap();
    let employee = store
        .create_employee(&employee("refusal-guards", true, true))
        .unwrap()
        .employee;
    let conversation =
        create_conversation(&store, "refusal-guards-conversation", employee.id, None).conversation;
    let root = record_turn(
        &store,
        "refusal-guards-root",
        conversation.id,
        user(),
        None,
        "root",
    );
    let refusal = |turn_id, expected_revision| FailRecordedRootTurnBeforeProviderInput {
        request_id: "refusal-guards-request".to_owned(),
        idempotency_key: format!("refusal-guards-{turn_id}-{expected_revision}"),
        turn_id,
        expected_revision,
        failure_code: "LZ-TASK-AUTHORITY-UNSUPPORTED".to_owned(),
        failure_message: "the selected provider cannot enforce this task authority ceiling"
            .to_owned(),
    };

    assert!(matches!(
        store.fail_recorded_root_turn_before_provider(&refusal(root.turn.id, 0)),
        Err(StoreError::TurnStale { .. })
    ));
    assert_eq!(
        store.get_turn(root.turn.id).unwrap().state,
        TurnState::Recorded
    );
    assert_eq!(
        store
            .get_message(root.message.envelope.id)
            .unwrap()
            .delivery
            .state,
        MessageDeliveryState::Queued
    );

    let session_id = warm_session(&store, employee.id, Uuid::from_u128(900));
    let submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "refusal-guards-begin".to_owned(),
            idempotency_key: "refusal-guards-begin".to_owned(),
            turn_id: root.turn.id,
            expected_revision: root.turn.revision,
            owner_boot_id: Uuid::from_u128(900),
            recipient_session_id: session_id,
        })
        .unwrap();
    assert!(matches!(
        store.fail_recorded_root_turn_before_provider(&refusal(
            root.turn.id,
            submitting.turn.revision
        )),
        Err(StoreError::TurnState { .. })
    ));
    assert_eq!(
        store.get_turn(root.turn.id).unwrap().state,
        TurnState::Submitting
    );

    let non_root = store
        .record_turn(&RecordTurnInput {
            request_id: "refusal-guards-follow-up-request".to_owned(),
            idempotency_key: "refusal-guards-follow-up".to_owned(),
            conversation_id: conversation.id,
            author: user(),
            recipient: None,
            task_id: Some(root.turn.id),
            authority_mode: AuthorityMode::Ask,
            text: "follow-up".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap();
    assert!(matches!(
        store.fail_recorded_root_turn_before_provider(&refusal(
            non_root.turn.id,
            non_root.turn.revision
        )),
        Err(StoreError::TaskTurnLink { .. })
    ));
    assert_eq!(
        store.get_turn(non_root.turn.id).unwrap().state,
        TurnState::Recorded
    );
}

fn warm_session(store: &Store, employee_id: Uuid, owner_boot_id: Uuid) -> Uuid {
    let starting = store
        .begin_session(&BeginSessionInput {
            request_id: "request-warm".to_owned(),
            idempotency_key: format!("warm-{employee_id}"),
            employee_id,
            expected_employee_revision: 1,
            owner_boot_id,
        })
        .unwrap();
    assert_eq!(starting.session.state, SessionLifecycleState::Starting);
    store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: starting.session.id,
            expected_revision: 1,
            owner_boot_id,
            native_session_id: "thread-native-1".to_owned(),
            native_host_pid: 4242,
        })
        .unwrap()
        .session
        .id
}

#[test]
#[allow(clippy::too_many_lines)] // Preserve the exact projected evidence order in one scenario.
fn task_turn_evidence_projects_once_and_reaches_terminal_truth_atomically() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee("Rhea", true, true))
        .unwrap()
        .employee;
    let owner = Uuid::new_v4();
    let session_id = warm_session(&store, lead.id, owner);
    let conversation = create_conversation(&store, "projection", lead.id, None).conversation;
    let recorded = record_turn(
        &store,
        "projected",
        conversation.id,
        user(),
        None,
        "build it",
    );
    let task_id = recorded.task.as_ref().unwrap().id;
    let submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-projected-begin".to_owned(),
            idempotency_key: "projected-begin".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: recorded.turn.revision,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();

    let accepted = store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-projected-start".to_owned(),
            idempotency_key: "projected-start".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: submitting.turn.revision,
            kind: "turn-started".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("native-turn-projected".to_owned()),
            payload: json!({"status": "in-progress"}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap();
    let running = store.get_task(task_id).unwrap();
    assert_eq!(running.task.state, TaskState::Running);
    assert_eq!(running.task.active_turn_id, Some(recorded.turn.id));
    assert_eq!(running.task.active_session_id, Some(session_id));
    assert_eq!(
        running
            .evidence
            .iter()
            .filter(|item| item.kind == TaskEvidenceKind::Started)
            .count(),
        1
    );

    let progress = store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-projected-progress".to_owned(),
            idempotency_key: "projected-progress".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: accepted.turn.revision,
            kind: "text-delta".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("native-turn-projected".to_owned()),
            payload: json!({"text": "working"}),
            transition: None,
        })
        .unwrap();
    let progressed = store.get_task(task_id).unwrap();
    assert_eq!(progressed.task.state, TaskState::Running);
    assert_eq!(
        progressed
            .evidence
            .iter()
            .filter(|item| item.kind == TaskEvidenceKind::Started)
            .count(),
        1
    );
    assert_eq!(
        progressed.evidence.last().unwrap().kind,
        TaskEvidenceKind::Progress
    );

    store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-projected-complete".to_owned(),
            idempotency_key: "projected-complete".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: progress.turn.revision,
            kind: "turn-completed".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("native-turn-projected".to_owned()),
            payload: json!({"status": "completed"}),
            transition: Some(TurnState::Completed),
        })
        .unwrap();
    let completed = store.get_task(task_id).unwrap();
    assert_eq!(completed.task.state, TaskState::Completed);
    assert_eq!(
        completed.evidence.last().unwrap().kind,
        TaskEvidenceKind::Completed
    );
    assert!(completed.task.active_turn_id.is_none());
    assert!(completed.task.active_session_id.is_none());
}

#[test]
#[allow(clippy::too_many_lines)] // Compare the three terminal projections with one fixture.
fn task_terminal_turn_outcomes_project_failure_cancellation_and_uncertainty() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee("Rhea", true, true))
        .unwrap()
        .employee;
    let owner = Uuid::new_v4();
    let session_id = warm_session(&store, lead.id, owner);
    let conversation = create_conversation(&store, "outcomes", lead.id, None).conversation;

    let failed = record_turn(&store, "failed-task", conversation.id, user(), None, "fail");
    let failed_submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-failed-begin".to_owned(),
            idempotency_key: "failed-begin".to_owned(),
            turn_id: failed.turn.id,
            expected_revision: failed.turn.revision,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();
    store
        .fail_turn(&FailTurnInput {
            request_id: "request-failed".to_owned(),
            idempotency_key: "failed".to_owned(),
            turn_id: failed.turn.id,
            expected_revision: failed_submitting.turn.revision,
            retryable: false,
            failure_code: "provider-failed".to_owned(),
            failure_message: "provider rejected the turn".to_owned(),
        })
        .unwrap();
    let failed_task = store.get_task(failed.turn.id).unwrap();
    assert_eq!(failed_task.task.state, TaskState::Failed);
    assert_eq!(
        failed_task.task.failure_code.as_deref(),
        Some("provider-failed")
    );
    assert_eq!(
        failed_task.evidence.last().unwrap().kind,
        TaskEvidenceKind::Failed
    );

    let cancelled = record_turn(
        &store,
        "cancelled-task",
        conversation.id,
        user(),
        None,
        "cancel",
    );
    store
        .request_turn_cancel(&RequestTurnCancelInput {
            request_id: "request-cancelled".to_owned(),
            idempotency_key: "cancelled".to_owned(),
            turn_id: cancelled.turn.id,
            expected_revision: cancelled.turn.revision,
        })
        .unwrap();
    let cancelled_task = store.get_task(cancelled.turn.id).unwrap();
    assert_eq!(cancelled_task.task.state, TaskState::Cancelled);
    assert_eq!(
        cancelled_task.evidence.last().unwrap().kind,
        TaskEvidenceKind::Cancelled
    );

    let uncertain = record_turn(
        &store,
        "uncertain-task",
        conversation.id,
        user(),
        None,
        "uncertain",
    );
    let uncertain_submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-uncertain-begin".to_owned(),
            idempotency_key: "uncertain-begin".to_owned(),
            turn_id: uncertain.turn.id,
            expected_revision: uncertain.turn.revision,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();
    let uncertain_accepted = store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-uncertain-start".to_owned(),
            idempotency_key: "uncertain-start".to_owned(),
            turn_id: uncertain.turn.id,
            expected_revision: uncertain_submitting.turn.revision,
            kind: "turn-started".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("native-turn-uncertain".to_owned()),
            payload: json!({"status": "in-progress"}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap();
    store
        .mark_turn_uncertain(&MarkTurnUncertainInput {
            request_id: "request-uncertain".to_owned(),
            idempotency_key: "uncertain".to_owned(),
            turn_id: uncertain.turn.id,
            expected_revision: uncertain_accepted.turn.revision,
            failure_code: "provider-uncertain".to_owned(),
            failure_message: "provider outcome could not be proven".to_owned(),
        })
        .unwrap();
    let uncertain_task = store.get_task(uncertain.turn.id).unwrap();
    assert_eq!(uncertain_task.task.state, TaskState::Uncertain);
    assert_eq!(
        uncertain_task.evidence.last().unwrap().kind,
        TaskEvidenceKind::Uncertain
    );
}

#[test]
#[allow(clippy::too_many_lines)] // Keep parent and child native evidence in one transaction story.
fn task_parent_completion_waits_for_an_active_child_with_an_exact_linked_turn() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee("Rhea", true, true))
        .unwrap()
        .employee;
    let worker = store
        .create_employee(&employee("Mira", true, true))
        .unwrap()
        .employee;
    let owner = Uuid::new_v4();
    let lead_session = warm_session(&store, lead.id, owner);
    let worker_session = warm_session(&store, worker.id, owner);
    let conversation = create_conversation(&store, "tree", lead.id, None).conversation;
    let root = record_turn(&store, "tree-root", conversation.id, user(), None, "root");
    let root_task = root.task.as_ref().unwrap();
    let root_submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-tree-root-begin".to_owned(),
            idempotency_key: "tree-root-begin".to_owned(),
            turn_id: root.turn.id,
            expected_revision: root.turn.revision,
            owner_boot_id: owner,
            recipient_session_id: lead_session,
        })
        .unwrap();
    let root_accepted = store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-tree-root-start".to_owned(),
            idempotency_key: "tree-root-start".to_owned(),
            turn_id: root.turn.id,
            expected_revision: root_submitting.turn.revision,
            kind: "turn-started".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("native-root".to_owned()),
            payload: json!({"status": "in-progress"}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap();

    let proposed = store
        .delegate_task(&DelegateTaskInput {
            request_id: "request-tree-child".to_owned(),
            idempotency_key: "tree-child".to_owned(),
            parent_task_id: root_task.id,
            actor: MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(lead_session),
            },
            assigned_employee_id: worker.id,
            objective: "child".to_owned(),
            references: Vec::new(),
            requested_authority: Some(AuthorityMode::Ask),
            roster_revision: store.roster_snapshot().unwrap().revision,
        })
        .unwrap();
    let child = store
        .accept_task(&AcceptTaskInput {
            request_id: "request-tree-child-accept".to_owned(),
            idempotency_key: "tree-child-accept".to_owned(),
            task_id: proposed.task.id,
            expected_revision: proposed.task.revision,
        })
        .unwrap();
    let child_turn = store
        .record_turn(&RecordTurnInput {
            request_id: "request-tree-child-turn".to_owned(),
            idempotency_key: "tree-child-turn".to_owned(),
            conversation_id: conversation.id,
            author: MessageSender::Employee(MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(lead_session),
            }),
            recipient: Some(MessageEndpoint {
                employee_id: worker.id,
                session_id: Some(worker_session),
            }),
            task_id: Some(child.task.id),
            authority_mode: AuthorityMode::Ask,
            text: "perform child".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap();
    let child_submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-tree-child-begin".to_owned(),
            idempotency_key: "tree-child-begin".to_owned(),
            turn_id: child_turn.turn.id,
            expected_revision: child_turn.turn.revision,
            owner_boot_id: owner,
            recipient_session_id: worker_session,
        })
        .unwrap();
    store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-tree-child-start".to_owned(),
            idempotency_key: "tree-child-start".to_owned(),
            turn_id: child_turn.turn.id,
            expected_revision: child_submitting.turn.revision,
            kind: "turn-started".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("native-child".to_owned()),
            payload: json!({"status": "in-progress"}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap();

    store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-tree-root-complete".to_owned(),
            idempotency_key: "tree-root-complete".to_owned(),
            turn_id: root.turn.id,
            expected_revision: root_accepted.turn.revision,
            kind: "turn-completed".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("native-root".to_owned()),
            payload: json!({"status": "completed"}),
            transition: Some(TurnState::Completed),
        })
        .unwrap();
    let parent = store.get_task(root_task.id).unwrap();
    assert_eq!(parent.task.state, TaskState::Running);
    assert!(parent.task.active_turn_id.is_none());
    assert!(parent.task.active_session_id.is_none());
    assert_eq!(
        parent.evidence.last().unwrap().kind,
        TaskEvidenceKind::Progress
    );
    assert_eq!(
        store.get_task(child.task.id).unwrap().task.state,
        TaskState::Running
    );
    let blocked = store
        .cancel_task(&CancelTaskInput {
            request_id: "request-tree-root-cancel".to_owned(),
            idempotency_key: "tree-root-cancel".to_owned(),
            task_id: root_task.id,
            expected_revision: parent.task.revision,
            actor: user(),
            evidence: json!({"reason": "stop parent"}),
        })
        .unwrap_err();
    assert!(matches!(blocked, StoreError::TaskActiveChildren { .. }));
}

#[test]
fn conversation_creation_is_idempotent_project_aware_and_route_checked() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee("Rhea", true, true))
        .unwrap()
        .employee;
    let project = store
        .register_project(&RegisterProjectInput {
            request_id: "request-project".to_owned(),
            idempotency_key: "project".to_owned(),
            repository_identity: "example-repository".to_owned(),
            canonical_path: temporary.path().to_path_buf(),
            display_name: "Example".to_owned(),
            checkout_mode: CheckoutMode::Isolated,
        })
        .unwrap()
        .project;

    let projectless = create_conversation(&store, "projectless", lead.id, None);
    let replay = create_conversation(&store, "projectless", lead.id, None);
    let project_conversation = create_conversation(&store, "project", lead.id, Some(project.id));

    assert_eq!(replay.conversation.id, projectless.conversation.id);
    assert_eq!(replay.event_sequence, None);
    assert_eq!(
        projectless.conversation.title.as_deref(),
        Some("First conversation")
    );
    assert_eq!(projectless.conversation.project_id, None);
    assert_eq!(
        project_conversation.conversation.project_id,
        Some(project.id)
    );
    assert_eq!(store.list_conversations(0, 100).unwrap().len(), 2);
    assert_eq!(
        store.get_conversation(projectless.conversation.id).unwrap(),
        projectless.conversation
    );

    let missing = store
        .create_conversation(&CreateConversationInput {
            request_id: "missing".to_owned(),
            idempotency_key: "missing".to_owned(),
            project_id: None,
            lead_employee_id: Uuid::new_v4(),
            title: None,
        })
        .unwrap_err();
    assert!(matches!(missing, StoreError::EmployeeNotFound { .. }));

    let disabled = store
        .create_employee(&employee("Disabled", false, true))
        .unwrap()
        .employee;
    let disabled_error = store
        .create_conversation(&CreateConversationInput {
            request_id: "disabled".to_owned(),
            idempotency_key: "disabled".to_owned(),
            project_id: None,
            lead_employee_id: disabled.id,
            title: None,
        })
        .unwrap_err();
    assert!(matches!(
        disabled_error,
        StoreError::EmployeeDisabled { .. }
    ));

    let mut unavailable_input = employee("Unavailable", true, false);
    unavailable_input.provider = "opencode".to_owned();
    let unavailable = store.create_employee(&unavailable_input).unwrap().employee;
    let unavailable_error = store
        .create_conversation(&CreateConversationInput {
            request_id: "unavailable".to_owned(),
            idempotency_key: "unavailable".to_owned(),
            project_id: None,
            lead_employee_id: unavailable.id,
            title: None,
        })
        .unwrap_err();
    assert!(matches!(
        unavailable_error,
        StoreError::SessionProviderNotReady { .. }
    ));
}

#[test]
fn turn_recording_is_actor_aware_immutable_ordered_and_record_first() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee("Rhea", true, true))
        .unwrap()
        .employee;
    let conversation = create_conversation(&store, "conversation", lead.id, None).conversation;

    let recorded = record_turn(&store, "turn-user", conversation.id, user(), None, "hello");
    let replay = record_turn(
        &store,
        "turn-user",
        conversation.id,
        user(),
        None,
        "changed",
    );
    let employee_author = MessageSender::Employee(MessageEndpoint {
        employee_id: lead.id,
        session_id: None,
    });
    let employee_turn = store
        .record_turn(&RecordTurnInput {
            request_id: "request-turn-employee".to_owned(),
            idempotency_key: "turn-employee".to_owned(),
            conversation_id: conversation.id,
            author: employee_author,
            recipient: None,
            task_id: Some(recorded.turn.id),
            authority_mode: AuthorityMode::Ask,
            text: "follow-up".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap();

    assert_eq!(recorded.turn.state, TurnState::Recorded);
    assert_eq!(
        recorded.message.delivery.state,
        MessageDeliveryState::Queued
    );
    assert!(recorded.turn.message_attempt_id.is_none());
    assert!(recorded.turn.native_turn_id.is_none());
    assert_eq!(replay.turn.id, recorded.turn.id);
    assert_eq!(replay.turn.text, "hello");
    assert_eq!(replay.event_sequence, None);
    assert_eq!(recorded.message.envelope.version, 2);
    assert_eq!(recorded.message.envelope.sender, user());
    assert_eq!(recorded.turn.message_id, recorded.message.envelope.id);
    assert_eq!(
        recorded.turn.correlation_id,
        recorded.message.envelope.correlation_id
    );
    assert_eq!(
        recorded.message.envelope.conversation_id,
        Some(conversation.id)
    );
    assert_eq!(
        recorded.message.envelope.body,
        json!({"turnId": recorded.turn.id, "text": "hello"})
    );
    assert_eq!(employee_turn.message.envelope.version, 2);
    assert_eq!(employee_turn.message.envelope.sender, employee_author);

    let listed = store.list_turns(conversation.id, 0, 100).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, recorded.turn.id);
    assert_eq!(listed[1].id, employee_turn.turn.id);
    assert_eq!(store.get_turn(recorded.turn.id).unwrap().text, "hello");

    let events = store.list_events(0, 100).unwrap();
    let event_names = events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect::<Vec<_>>();
    assert!(
        event_names
            .windows(2)
            .any(|pair| { pair == ["message.queued", "turn.recorded"] })
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn submission_observation_and_failures_update_both_ledgers_atomically() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee("Rhea", true, true))
        .unwrap()
        .employee;
    let owner = Uuid::new_v4();
    let session_id = warm_session(&store, lead.id, owner);
    let conversation = create_conversation(&store, "conversation", lead.id, None).conversation;
    let recorded = record_turn(&store, "turn", conversation.id, user(), None, "hello");

    let submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-begin".to_owned(),
            idempotency_key: "begin".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: 1,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();
    let begin_replay = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-begin-replay".to_owned(),
            idempotency_key: "begin".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: 1,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();
    assert_eq!(submitting.turn.state, TurnState::Submitting);
    assert_eq!(
        submitting.message.delivery.state,
        MessageDeliveryState::Delivering
    );
    assert_eq!(
        submitting.turn.message_attempt_id,
        Some(submitting.message.attempts[0].id)
    );
    assert_eq!(begin_replay.turn, submitting.turn);
    assert_eq!(begin_replay.event_sequence, None);

    let unsupported_acceptance = store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-no-evidence".to_owned(),
            idempotency_key: "no-evidence".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: 2,
            kind: "turn-started".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: None,
            payload: json!({"status": "in-progress"}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap_err();
    assert!(matches!(
        unsupported_acceptance,
        StoreError::TurnAcceptanceEvidence { .. }
    ));
    assert_eq!(store.get_turn(recorded.turn.id).unwrap().revision, 2);
    assert_eq!(
        store
            .get_message(recorded.message.envelope.id)
            .unwrap()
            .delivery
            .state,
        MessageDeliveryState::Delivering
    );

    let accepted = store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-started".to_owned(),
            idempotency_key: "started".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: 2,
            kind: "turn-started".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("turn-native-1".to_owned()),
            payload: json!({"status": "in-progress"}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap();
    assert_eq!(accepted.turn.state, TurnState::Accepted);
    assert_eq!(
        accepted.message.delivery.state,
        MessageDeliveryState::Acknowledged
    );
    assert_eq!(
        accepted.turn.native_turn_id.as_deref(),
        Some("turn-native-1")
    );

    let completed = store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-completed".to_owned(),
            idempotency_key: "completed".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: 3,
            kind: "turn-completed".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("turn-native-1".to_owned()),
            payload: json!({"status": "completed"}),
            transition: Some(TurnState::Completed),
        })
        .unwrap();
    assert_eq!(completed.turn.state, TurnState::Completed);
    assert_eq!(
        completed.message.delivery.state,
        MessageDeliveryState::Acknowledged
    );
    assert_eq!(completed.turn.events.len(), 2);
    assert_eq!(completed.turn.events[0].sequence, 1);
    assert_eq!(completed.turn.events[1].sequence, 2);

    let retry_recorded = record_turn(&store, "retry-turn", conversation.id, user(), None, "retry");
    let retry_submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-retry-begin".to_owned(),
            idempotency_key: "retry-begin".to_owned(),
            turn_id: retry_recorded.turn.id,
            expected_revision: 1,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();
    let failed = store
        .fail_turn(&FailTurnInput {
            request_id: "request-retry-fail".to_owned(),
            idempotency_key: "retry-fail".to_owned(),
            turn_id: retry_recorded.turn.id,
            expected_revision: retry_submitting.turn.revision,
            retryable: true,
            failure_code: "provider-busy".to_owned(),
            failure_message: "provider asked us to retry".to_owned(),
        })
        .unwrap();
    assert_eq!(failed.turn.state, TurnState::RetryableFailed);
    assert_eq!(
        failed.message.delivery.state,
        MessageDeliveryState::RetryableFailed
    );

    store
        .execute_for_test(&format!(
            "UPDATE message_deliveries SET next_attempt_at = '2020-01-01T00:00:00Z' WHERE message_id = '{}';",
            retry_recorded.message.envelope.id
        ))
        .unwrap();
    let retried = store
        .retry_turn(&RetryTurnInput {
            request_id: "request-retry".to_owned(),
            idempotency_key: "retry".to_owned(),
            turn_id: retry_recorded.turn.id,
            expected_revision: failed.turn.revision,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();
    assert_eq!(retried.turn.state, TurnState::Submitting);
    assert_eq!(
        retried.message.delivery.state,
        MessageDeliveryState::Delivering
    );
    assert_eq!(retried.message.attempts.len(), 2);
}

#[test]
fn transcript_is_bounded_once_while_terminal_lifecycle_evidence_is_retained() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee("Rhea", true, true))
        .unwrap()
        .employee;
    let owner = Uuid::new_v4();
    let session_id = warm_session(&store, lead.id, owner);
    let conversation = create_conversation(&store, "conversation", lead.id, None).conversation;
    let recorded = record_turn(&store, "bounded", conversation.id, user(), None, "hello");
    let submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-begin".to_owned(),
            idempotency_key: "begin".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: 1,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();
    let mut current = store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-started".to_owned(),
            idempotency_key: "started".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: submitting.turn.revision,
            kind: "turn-started".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("turn-native-1".to_owned()),
            payload: json!({"status": "in-progress"}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap();

    let verbose = "x".repeat(60 * 1024);
    for index in 0..9 {
        current = store
            .observe_turn(&ObserveTurnInput {
                request_id: format!("request-delta-{index}"),
                idempotency_key: format!("delta-{index}"),
                turn_id: recorded.turn.id,
                expected_revision: current.turn.revision,
                kind: "item-delta".to_owned(),
                provider: "codex".to_owned(),
                native_session_id: Some("thread-native-1".to_owned()),
                native_turn_id: Some("turn-native-1".to_owned()),
                payload: json!({"delta": &verbose}),
                transition: None,
            })
            .unwrap();
    }
    assert!(current.turn.transcript_truncated);
    assert_eq!(
        current
            .turn
            .events
            .iter()
            .filter(|event| event.kind == "transcript-truncated")
            .count(),
        1
    );

    let completed = store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-completed".to_owned(),
            idempotency_key: "completed".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: current.turn.revision,
            kind: "turn-completed".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("turn-native-1".to_owned()),
            payload: json!({"status": "completed"}),
            transition: Some(TurnState::Completed),
        })
        .unwrap();
    assert_eq!(completed.turn.state, TurnState::Completed);
    assert_eq!(completed.turn.events.last().unwrap().kind, "turn-completed");
    assert_eq!(
        completed
            .turn
            .events
            .iter()
            .filter(|event| event.kind == "transcript-truncated")
            .count(),
        1
    );
}

#[test]
fn employee_activation_and_stale_turn_mutations_fail_without_partial_writes() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee("Rhea", true, true))
        .unwrap()
        .employee;
    let owner = Uuid::new_v4();
    let session_id = warm_session(&store, lead.id, owner);
    let conversation = create_conversation(&store, "conversation", lead.id, None).conversation;
    let employee_author = MessageSender::Employee(MessageEndpoint {
        employee_id: lead.id,
        session_id: Some(session_id),
    });
    let activation_error = store
        .record_turn(&RecordTurnInput {
            request_id: "request-activation".to_owned(),
            idempotency_key: "activation".to_owned(),
            conversation_id: conversation.id,
            author: employee_author,
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::Ask,
            text: "hello".to_owned(),
            activate_if_needed: true,
            max_attempts: 3,
        })
        .unwrap_err();
    assert!(matches!(
        activation_error,
        StoreError::ConversationValidation(_)
    ));

    let recorded = record_turn(&store, "stale", conversation.id, user(), None, "hello");
    let submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-begin".to_owned(),
            idempotency_key: "begin".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: 1,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();
    let stale = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-stale".to_owned(),
            idempotency_key: "stale".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: 1,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap_err();
    assert!(matches!(stale, StoreError::TurnStale { .. }));
    assert_eq!(
        store
            .get_message(recorded.message.envelope.id)
            .unwrap()
            .delivery
            .revision,
        submitting.message.delivery.revision
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn terminal_failure_and_post_acceptance_cancel_preserve_message_delivery_truth() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee("Rhea", true, true))
        .unwrap()
        .employee;
    let owner = Uuid::new_v4();
    let session_id = warm_session(&store, lead.id, owner);
    let conversation = create_conversation(&store, "conversation", lead.id, None).conversation;

    let terminal_recorded = store
        .record_turn(&RecordTurnInput {
            request_id: "request-terminal".to_owned(),
            idempotency_key: "terminal".to_owned(),
            conversation_id: conversation.id,
            author: user(),
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::Ask,
            text: "terminal".to_owned(),
            activate_if_needed: false,
            max_attempts: 1,
        })
        .unwrap();
    let terminal_submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-terminal-begin".to_owned(),
            idempotency_key: "terminal-begin".to_owned(),
            turn_id: terminal_recorded.turn.id,
            expected_revision: 1,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();
    let terminal = store
        .fail_turn(&FailTurnInput {
            request_id: "request-terminal-fail".to_owned(),
            idempotency_key: "terminal-fail".to_owned(),
            turn_id: terminal_recorded.turn.id,
            expected_revision: terminal_submitting.turn.revision,
            retryable: true,
            failure_code: "provider-rejected".to_owned(),
            failure_message: "provider rejected the turn".to_owned(),
        })
        .unwrap();
    assert_eq!(terminal.turn.state, TurnState::TerminalFailed);
    assert_eq!(
        terminal.message.delivery.state,
        MessageDeliveryState::TerminalFailed
    );
    assert!(terminal.message.dead_letter.is_some());

    let accepted_recorded = record_turn(
        &store,
        "accepted-cancel",
        conversation.id,
        user(),
        None,
        "cancel after acceptance",
    );
    let accepted_submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-accepted-begin".to_owned(),
            idempotency_key: "accepted-begin".to_owned(),
            turn_id: accepted_recorded.turn.id,
            expected_revision: 1,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();
    let accepted = store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-accepted".to_owned(),
            idempotency_key: "accepted".to_owned(),
            turn_id: accepted_recorded.turn.id,
            expected_revision: accepted_submitting.turn.revision,
            kind: "turn-started".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("turn-native-cancel".to_owned()),
            payload: json!({"status": "in-progress"}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap();
    let cancel_requested = store
        .request_turn_cancel(&RequestTurnCancelInput {
            request_id: "request-accepted-cancel".to_owned(),
            idempotency_key: "accepted-cancel".to_owned(),
            turn_id: accepted_recorded.turn.id,
            expected_revision: accepted.turn.revision,
        })
        .unwrap();
    let cancelled = store
        .finish_turn_cancel(&FinishTurnCancelInput {
            request_id: "request-accepted-cancelled".to_owned(),
            idempotency_key: "accepted-cancelled".to_owned(),
            turn_id: accepted_recorded.turn.id,
            expected_revision: cancel_requested.turn.revision,
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: Some("turn-native-cancel".to_owned()),
            payload: json!({"status": "cancelled"}),
        })
        .unwrap();
    assert_eq!(cancelled.turn.state, TurnState::Cancelled);
    assert_eq!(
        cancelled.message.delivery.state,
        MessageDeliveryState::Acknowledged
    );
}

#[test]
fn cancellation_truncation_and_owner_loss_are_durable_and_idempotent() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee("Rhea", true, true))
        .unwrap()
        .employee;
    let owner = Uuid::new_v4();
    let session_id = warm_session(&store, lead.id, owner);
    let conversation = create_conversation(&store, "conversation", lead.id, None).conversation;

    let queued = record_turn(&store, "queued", conversation.id, user(), None, "queued");
    let cancelled = store
        .request_turn_cancel(&RequestTurnCancelInput {
            request_id: "request-cancel-queued".to_owned(),
            idempotency_key: "cancel-queued".to_owned(),
            turn_id: queued.turn.id,
            expected_revision: 1,
        })
        .unwrap();
    assert_eq!(cancelled.turn.state, TurnState::Cancelled);
    assert_eq!(
        cancelled.message.delivery.state,
        MessageDeliveryState::Cancelled
    );

    let active = record_turn(&store, "active", conversation.id, user(), None, "active");
    let submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-begin-active".to_owned(),
            idempotency_key: "begin-active".to_owned(),
            turn_id: active.turn.id,
            expected_revision: 1,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();
    let requested = store
        .request_turn_cancel(&RequestTurnCancelInput {
            request_id: "request-cancel-active".to_owned(),
            idempotency_key: "cancel-active".to_owned(),
            turn_id: active.turn.id,
            expected_revision: submitting.turn.revision,
        })
        .unwrap();
    assert_eq!(requested.turn.state, TurnState::Submitting);
    assert!(requested.turn.cancel_requested_at.is_some());
    let finished = store
        .finish_turn_cancel(&FinishTurnCancelInput {
            request_id: "request-finish-cancel".to_owned(),
            idempotency_key: "finish-cancel".to_owned(),
            turn_id: active.turn.id,
            expected_revision: requested.turn.revision,
            provider: "codex".to_owned(),
            native_session_id: Some("thread-native-1".to_owned()),
            native_turn_id: None,
            payload: json!({"status": "cancelled"}),
        })
        .unwrap();
    assert_eq!(finished.turn.state, TurnState::Cancelled);
    assert_eq!(
        finished.message.delivery.state,
        MessageDeliveryState::Cancelled
    );

    let lost = record_turn(&store, "lost", conversation.id, user(), None, "lost");
    let lost_submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-begin-lost".to_owned(),
            idempotency_key: "begin-lost".to_owned(),
            turn_id: lost.turn.id,
            expected_revision: 1,
            owner_boot_id: owner,
            recipient_session_id: session_id,
        })
        .unwrap();
    assert_eq!(lost_submitting.turn.state, TurnState::Submitting);
    let reconciled = store.reconcile_turns(Uuid::new_v4()).unwrap();
    assert_eq!(reconciled.turns.len(), 1);
    assert_eq!(reconciled.turns[0].id, lost.turn.id);
    assert_eq!(reconciled.turns[0].state, TurnState::Uncertain);
    assert_eq!(
        reconciled.turns[0].failure_code.as_deref(),
        Some("LZ-TURN-OWNER-LOST")
    );

    let uncertain_replay = store
        .mark_turn_uncertain(&MarkTurnUncertainInput {
            request_id: "request-uncertain".to_owned(),
            idempotency_key: "uncertain".to_owned(),
            turn_id: lost.turn.id,
            expected_revision: reconciled.turns[0].revision,
            failure_code: "ignored".to_owned(),
            failure_message: "ignored".to_owned(),
        })
        .unwrap_err();
    assert!(matches!(uncertain_replay, StoreError::TurnState { .. }));
}

#[test]
fn conversation_rename_is_revisioned_naturally_idempotent_and_validated() {
    use lazoo_store::RenameConversationInput;

    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee("Rhea", true, true))
        .unwrap()
        .employee;
    let created = create_conversation(&store, "rename-fixture", lead.id, None).conversation;
    assert_eq!(created.title.as_deref(), Some("First conversation"));
    assert_eq!(created.revision, 1);

    let renamed = store
        .rename_conversation(&RenameConversationInput {
            conversation_id: created.id,
            expected_revision: 1,
            title: Some("  Parser fix  ".to_owned()),
        })
        .unwrap();
    assert_eq!(renamed.conversation.title.as_deref(), Some("Parser fix"));
    assert_eq!(renamed.conversation.revision, 2);
    assert!(renamed.event_sequence.is_some());
    assert!(renamed.conversation.updated_at >= created.updated_at);

    // The same title again changes nothing and emits nothing, whatever revision is cited.
    let replay = store
        .rename_conversation(&RenameConversationInput {
            conversation_id: created.id,
            expected_revision: 1,
            title: Some("Parser fix".to_owned()),
        })
        .unwrap();
    assert_eq!(replay.conversation, renamed.conversation);
    assert_eq!(replay.event_sequence, None);

    // A different title at a stale revision is refused and changes nothing.
    let stale = store
        .rename_conversation(&RenameConversationInput {
            conversation_id: created.id,
            expected_revision: 1,
            title: Some("Something else".to_owned()),
        })
        .unwrap_err();
    assert!(matches!(
        stale,
        StoreError::ConversationStale {
            current_revision: 2,
            ..
        }
    ));
    assert_eq!(
        store.get_conversation(created.id).unwrap().title.as_deref(),
        Some("Parser fix")
    );

    // Clearing the title is a rename to nothing; blank titles clear too.
    let cleared = store
        .rename_conversation(&RenameConversationInput {
            conversation_id: created.id,
            expected_revision: 2,
            title: Some("   ".to_owned()),
        })
        .unwrap();
    assert_eq!(cleared.conversation.title, None);
    assert_eq!(cleared.conversation.revision, 3);

    // Validation and existence are checked before anything is written.
    assert!(matches!(
        store
            .rename_conversation(&RenameConversationInput {
                conversation_id: created.id,
                expected_revision: 3,
                title: Some("x".repeat(300)),
            })
            .unwrap_err(),
        StoreError::ConversationValidation(_)
    ));
    assert!(matches!(
        store
            .rename_conversation(&RenameConversationInput {
                conversation_id: Uuid::new_v4(),
                expected_revision: 1,
                title: Some("Ghost".to_owned()),
            })
            .unwrap_err(),
        StoreError::ConversationNotFound { .. }
    ));
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
}

#[test]
fn workspace_history_pages_backward_without_duplicates_or_cross_conversation_rows() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("state.db")).unwrap();
    let lead = store
        .create_employee(&employee("lead", true, true))
        .unwrap()
        .employee;
    let conversation = create_conversation(&store, "history", lead.id, None).conversation;
    let other = create_conversation(&store, "other-history", lead.id, None).conversation;
    for index in 0..7 {
        record_turn(
            &store,
            &format!("history-{index}"),
            conversation.id,
            user(),
            None,
            "history",
        );
    }
    record_turn(&store, "other-turn", other.id, user(), None, "other");
    let first = store
        .conversation_workspace_page(conversation.id, 3, None)
        .unwrap();
    let first_ids: Vec<_> = first.turns.iter().map(|turn| turn.id).collect();
    assert_eq!(first.turns.len(), 3);
    let second = store
        .conversation_workspace_page(conversation.id, 3, first.next_before_turn_sequence)
        .unwrap();
    let second_ids: Vec<_> = second.turns.iter().map(|turn| turn.id).collect();
    let third = store
        .conversation_workspace_page(conversation.id, 3, second.next_before_turn_sequence)
        .unwrap();
    assert_eq!(third.turns.len(), 1);
    assert_eq!(third.next_before_turn_sequence, None);
    for page in [&first, &second, &third] {
        assert!(
            page.turns
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
        );
        assert!(
            page.turns
                .iter()
                .all(|turn| turn.conversation_id == conversation.id)
        );
    }
    assert!(first_ids.iter().all(|id| !second_ids.contains(id)));
    assert!(
        second
            .turns
            .iter()
            .all(|turn| turn.conversation_id == conversation.id)
    );
    let newest = record_turn(
        &store,
        "history-new",
        conversation.id,
        user(),
        None,
        "newest",
    );
    let stable = store
        .conversation_workspace_page(conversation.id, 3, first.next_before_turn_sequence)
        .unwrap();
    assert_eq!(
        stable.turns.iter().map(|turn| turn.id).collect::<Vec<_>>(),
        second_ids
    );
    assert_eq!(stable.latest_lead_turn.unwrap().0, newest.turn.id);
    assert_eq!(stable.root_task.unwrap().id, newest.turn.id);
}
