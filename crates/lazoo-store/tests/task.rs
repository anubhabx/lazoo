use lazoo_protocol::{
    AuthorityMode, EmployeeCheckoutPolicy, MessageEndpoint, MessageKind, MessageSender,
    ProviderObservation, SessionLifecycleState, TaskDispatchState, TaskEvidenceKind,
    TaskReportKind, TaskResolution, TaskState, TaskWorkspaceKind, TurnState, UserMessageEndpoint,
    UserMessageKind,
};
use lazoo_store::{
    AcceptTaskInput, BeginSessionInput, BeginTurnSubmissionInput, CancelTaskInput,
    ClaimTaskDispatchInput, CreateConversationInput, CreateEmployeeInput, DeferTaskDispatchInput,
    DelegateTaskInput, FailSessionInput, FailTaskDispatchInput, ListTasksInput,
    MarkSessionReadyInput, MarkTaskDispatchWorkspaceReadyInput, ObserveTurnInput,
    RecordTaskDispatchTurnInput, RecordTurnInput, ReportTaskInput, ResolveTaskInput,
    SendMessageInput, StartTaskInput, Store, StoreError,
};
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

fn employee(name: &str) -> CreateEmployeeInput {
    employee_with(name, AuthorityMode::Ask, true, true)
}

#[test]
#[allow(clippy::too_many_lines)] // This is a sequential durability and replay contract test.
fn accepted_child_dispatch_claim_is_durable_exclusive_and_idempotent() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee_with(
            "Rhea",
            AuthorityMode::FullAutonomy,
            true,
            true,
        ))
        .unwrap()
        .employee;
    let worker = store
        .create_employee(&employee_with(
            "Mira",
            AuthorityMode::FullAutonomy,
            true,
            true,
        ))
        .unwrap()
        .employee;
    let conversation_id = create_conversation(&store, "dispatch-conversation", lead.id);
    let root = root_task(
        &store,
        "dispatch-root",
        conversation_id,
        AuthorityMode::FullAutonomy,
    );
    let lead_session = warm_session(&store, "dispatch-lead", lead.id);
    let child = store
        .delegate_task(&delegate_input(
            "dispatch-child",
            root.id,
            MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(lead_session),
            },
            worker.id,
            Some(AuthorityMode::SelfDecision),
            store.roster_snapshot().unwrap().revision,
        ))
        .unwrap();
    let message_id = child.task.delegation_message_id.unwrap();
    let owner_boot_id = Uuid::new_v4();
    let input = ClaimTaskDispatchInput {
        task_id: child.task.id,
        owner_boot_id,
    };

    assert_eq!(
        store.pending_task_dispatches(10).unwrap(),
        vec![child.task.id]
    );
    let claimed = store.claim_task_dispatch(&input).unwrap();
    assert!(store.pending_task_dispatches(10).unwrap().is_empty());
    assert_eq!(claimed.task_id, child.task.id);
    assert_eq!(claimed.message_id, message_id);
    assert_eq!(claimed.owner_boot_id, owner_boot_id);
    assert_eq!(claimed.state, TaskDispatchState::Preparing);
    assert_eq!(claimed.revision, 1);
    assert!(claimed.workspace_path.is_none());
    assert!(claimed.turn_id.is_none());

    let event_count = store.list_events(0, 100).unwrap().len();
    let replay = store.claim_task_dispatch(&input).unwrap();
    assert_eq!(replay, claimed);
    assert_eq!(store.list_events(0, 100).unwrap().len(), event_count);
    assert_eq!(
        store.get_task(child.task.id).unwrap().dispatch,
        Some(claimed)
    );

    let conflict = store
        .claim_task_dispatch(&ClaimTaskDispatchInput {
            task_id: child.task.id,
            owner_boot_id: Uuid::new_v4(),
        })
        .unwrap_err();
    assert!(matches!(conflict, StoreError::TaskDispatchOwned { .. }));

    let ready_input = MarkTaskDispatchWorkspaceReadyInput {
        task_id: child.task.id,
        owner_boot_id,
        expected_revision: 1,
        workspace_kind: TaskWorkspaceKind::Projectless,
        workspace_path: r"C:\Lazoo\task-workspaces\projectless\task".to_owned(),
        base_commit: None,
        branch_name: None,
    };
    let mut ready = store
        .mark_task_dispatch_workspace_ready(&ready_input)
        .unwrap();
    assert_eq!(ready.state, TaskDispatchState::WorkspaceReady);
    assert_eq!(ready.revision, 2);
    assert_eq!(ready.workspace_kind, Some(TaskWorkspaceKind::Projectless));
    assert_eq!(
        ready.workspace_path,
        Some(ready_input.workspace_path.clone())
    );

    let event_count = store.list_events(0, 100).unwrap().len();
    let ready_replay = store
        .mark_task_dispatch_workspace_ready(&ready_input)
        .unwrap();
    assert_eq!(ready_replay, ready);
    assert_eq!(store.list_events(0, 100).unwrap().len(), event_count);

    let racing_session = store
        .begin_session(&BeginSessionInput {
            request_id: "dispatch-racing-session-request".to_owned(),
            idempotency_key: "dispatch-racing-session".to_owned(),
            employee_id: worker.id,
            expected_employee_revision: worker.configuration_revision,
            owner_boot_id: Uuid::new_v4(),
        })
        .unwrap()
        .session;
    assert!(matches!(
        store.begin_session(&BeginSessionInput {
            request_id: "dispatch-conflicting-session-request".to_owned(),
            idempotency_key: "dispatch-conflicting-session".to_owned(),
            employee_id: worker.id,
            expected_employee_revision: worker.configuration_revision,
            owner_boot_id,
        }),
        Err(StoreError::SessionConflict { .. })
    ));
    store
        .defer_task_dispatch(&DeferTaskDispatchInput {
            task_id: child.task.id,
            owner_boot_id,
            expected_revision: ready.revision,
            active_session_id: racing_session.id,
        })
        .unwrap();
    assert!(store.get_task(child.task.id).unwrap().dispatch.is_none());
    assert!(store.pending_task_dispatches(10).unwrap().is_empty());
    store
        .fail_session(&FailSessionInput {
            session_id: racing_session.id,
            expected_revision: racing_session.revision,
            failure_code: "LZ-TEST-SESSION-RELEASED".to_owned(),
            failure_message: "release the concurrent session for dispatch retry".to_owned(),
        })
        .unwrap();
    assert_eq!(
        store.pending_task_dispatches(10).unwrap(),
        vec![child.task.id]
    );
    let reclaimed = store.claim_task_dispatch(&input).unwrap();
    let mut retry_ready_input = ready_input.clone();
    retry_ready_input.expected_revision = reclaimed.revision;
    ready = store
        .mark_task_dispatch_workspace_ready(&retry_ready_input)
        .unwrap();

    let worker_starting = store
        .begin_session(&BeginSessionInput {
            request_id: "dispatch-worker-session-request".to_owned(),
            idempotency_key: "dispatch-worker-session".to_owned(),
            employee_id: worker.id,
            expected_employee_revision: worker.configuration_revision,
            owner_boot_id,
        })
        .unwrap();
    let worker_session = store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: worker_starting.session.id,
            expected_revision: worker_starting.session.revision,
            owner_boot_id,
            native_session_id: "native-dispatch-worker".to_owned(),
            native_host_pid: 4243,
        })
        .unwrap()
        .session;
    let message_count = store.list_messages(0, 100).unwrap().len();
    let turn_input = RecordTaskDispatchTurnInput {
        task_id: child.task.id,
        owner_boot_id,
        expected_dispatch_revision: ready.revision,
        session_id: worker_session.id,
    };
    let recorded = store.record_task_dispatch_turn(&turn_input).unwrap();
    assert_eq!(recorded.turn.state, TurnState::Recorded);
    assert_eq!(recorded.turn.task_id, Some(child.task.id));
    assert_eq!(recorded.turn.message_id, message_id);
    assert_eq!(recorded.turn.author, child.task.creator);
    assert_eq!(recorded.turn.recipient.employee_id, worker.id);
    assert_eq!(recorded.turn.recipient.session_id, Some(worker_session.id));
    assert_eq!(recorded.turn.text, child.task.objective);
    assert_eq!(store.list_messages(0, 100).unwrap().len(), message_count);

    let recorded_replay = store.record_task_dispatch_turn(&turn_input).unwrap();
    assert_eq!(recorded_replay.turn.id, recorded.turn.id);
    assert_eq!(recorded_replay.event_sequence, None);
    let dispatch = store.get_task(child.task.id).unwrap().dispatch.unwrap();
    assert_eq!(dispatch.state, TaskDispatchState::TurnRecorded);
    assert_eq!(dispatch.revision, 3);
    assert_eq!(dispatch.turn_id, Some(recorded.turn.id));
    assert_eq!(dispatch.session_id, Some(worker_session.id));

    store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "dispatch-begin-request".to_owned(),
            idempotency_key: "dispatch-begin".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: recorded.turn.revision,
            owner_boot_id,
            recipient_session_id: worker_session.id,
        })
        .unwrap();
    let replacement_boot_id = Uuid::new_v4();
    store.reconcile_sessions(replacement_boot_id).unwrap();
    store.reconcile_messages().unwrap();
    store.reconcile_turns(replacement_boot_id).unwrap();
    let reconciled = store.reconcile_tasks().unwrap();
    assert_eq!(reconciled.tasks.len(), 1);
    let detail = store.get_task(child.task.id).unwrap();
    assert_eq!(detail.task.state, TaskState::Uncertain);
    assert_eq!(detail.dispatch.unwrap().state, TaskDispatchState::Uncertain);
}

fn employee_with(
    name: &str,
    authority_mode: AuthorityMode,
    enabled: bool,
    route_ready: bool,
) -> CreateEmployeeInput {
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
        authority_mode,
        checkout_policy: EmployeeCheckoutPolicy::Isolated,
        enabled,
        provider_observation: route_ready.then(|| ProviderObservation {
            provider: "codex".to_owned(),
            verification: "configured".to_owned(),
            version: Some("0.144.1".to_owned()),
            protocol: Some("app-server/v2".to_owned()),
            capabilities: json!({"session": true}),
            authentication: json!({"native": "ready", "external": "unknown"}),
            degradation_reason: None,
            checked_at: "2026-09-04T00:00:00Z".to_owned(),
        }),
    }
}

#[test]
fn stale_pre_provider_dispatch_terminalizes_the_task_and_canonical_message_on_restart() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee_with(
            "Rhea",
            AuthorityMode::FullAutonomy,
            true,
            true,
        ))
        .unwrap()
        .employee;
    let worker = store
        .create_employee(&employee_with(
            "Mira",
            AuthorityMode::FullAutonomy,
            true,
            true,
        ))
        .unwrap()
        .employee;
    let conversation_id = create_conversation(&store, "failure-conversation", lead.id);
    let root = root_task(
        &store,
        "failure-root",
        conversation_id,
        AuthorityMode::FullAutonomy,
    );
    let lead_session = warm_session(&store, "failure-lead", lead.id);
    let child = store
        .delegate_task(&delegate_input(
            "failure-child",
            root.id,
            MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(lead_session),
            },
            worker.id,
            Some(AuthorityMode::SelfDecision),
            store.roster_snapshot().unwrap().revision,
        ))
        .unwrap();
    let owner_boot_id = Uuid::new_v4();
    store
        .claim_task_dispatch(&ClaimTaskDispatchInput {
            task_id: child.task.id,
            owner_boot_id,
        })
        .unwrap();

    let replacement_boot_id = Uuid::new_v4();
    let reconciled = store
        .reconcile_task_dispatches(replacement_boot_id)
        .unwrap();
    assert_eq!(reconciled.tasks.len(), 1);

    let failed = store
        .fail_task_dispatch(&FailTaskDispatchInput {
            task_id: child.task.id,
            owner_boot_id: replacement_boot_id,
            failure_code: "LZ-TASK-DISPATCH-OWNER-LOST".to_owned(),
            failure_message: "the daemon stopped before provider acceptance".to_owned(),
        })
        .unwrap();
    assert_eq!(failed.task.state, TaskState::Failed);
    assert_eq!(
        failed.task.failure_code.as_deref(),
        Some("LZ-TASK-DISPATCH-OWNER-LOST")
    );
    assert_eq!(failed.dispatch.state, TaskDispatchState::Failed);
    assert_eq!(
        failed.message.delivery.state,
        lazoo_protocol::MessageDeliveryState::TerminalFailed
    );
    assert_eq!(failed.message.attempts.len(), 1);
    assert!(failed.message.dead_letter.is_some());
    assert_eq!(
        store
            .get_task(child.task.id)
            .unwrap()
            .evidence
            .last()
            .unwrap()
            .kind,
        TaskEvidenceKind::Failed
    );

    let replay = store
        .fail_task_dispatch(&FailTaskDispatchInput {
            task_id: child.task.id,
            owner_boot_id: replacement_boot_id,
            failure_code: "LZ-TASK-DISPATCH-OWNER-LOST".to_owned(),
            failure_message: "the daemon stopped before provider acceptance".to_owned(),
        })
        .unwrap();
    assert_eq!(replay.task, failed.task);
    assert_eq!(replay.dispatch, failed.dispatch);
    assert_eq!(replay.message, failed.message);
    assert_eq!(replay.event_sequence, None);
}

#[test]
fn cancelling_a_claimed_dispatch_cancels_the_canonical_message_atomically() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee_with(
            "Cancel lead",
            AuthorityMode::FullAutonomy,
            true,
            true,
        ))
        .unwrap()
        .employee;
    let worker = store
        .create_employee(&employee_with(
            "Cancel worker",
            AuthorityMode::FullAutonomy,
            true,
            true,
        ))
        .unwrap()
        .employee;
    let conversation_id = create_conversation(&store, "cancel-dispatch-conversation", lead.id);
    let root = root_task(
        &store,
        "cancel-dispatch-root",
        conversation_id,
        AuthorityMode::FullAutonomy,
    );
    let lead_session = warm_session(&store, "cancel-dispatch-lead", lead.id);
    let child = store
        .delegate_task(&delegate_input(
            "cancel-dispatch-child",
            root.id,
            MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(lead_session),
            },
            worker.id,
            Some(AuthorityMode::SelfDecision),
            store.roster_snapshot().unwrap().revision,
        ))
        .unwrap();
    store
        .claim_task_dispatch(&ClaimTaskDispatchInput {
            task_id: child.task.id,
            owner_boot_id: Uuid::new_v4(),
        })
        .unwrap();

    let cancelled = store
        .cancel_task(&CancelTaskInput {
            request_id: "cancel-dispatch-request".to_owned(),
            idempotency_key: "cancel-dispatch".to_owned(),
            task_id: child.task.id,
            expected_revision: child.task.revision,
            actor: MessageSender::User(UserMessageEndpoint {
                kind: UserMessageKind::User,
            }),
            evidence: json!({"reason": "user requested"}),
        })
        .unwrap();

    assert_eq!(cancelled.task.state, TaskState::Cancelled);
    assert_eq!(
        cancelled.delegation_message.unwrap().delivery.state,
        lazoo_protocol::MessageDeliveryState::Cancelled
    );
    assert_eq!(
        store
            .get_task(child.task.id)
            .unwrap()
            .dispatch
            .unwrap()
            .state,
        TaskDispatchState::Cancelled
    );
}

fn create_conversation(store: &Store, key: &str, lead_employee_id: Uuid) -> Uuid {
    store
        .create_conversation(&CreateConversationInput {
            request_id: format!("request-{key}"),
            idempotency_key: key.to_owned(),
            project_id: None,
            lead_employee_id,
            title: None,
        })
        .unwrap()
        .conversation
        .id
}

fn warm_session(store: &Store, key: &str, employee_id: Uuid) -> Uuid {
    ready_session(store, key, employee_id).0
}

fn ready_session(store: &Store, key: &str, employee_id: Uuid) -> (Uuid, Uuid) {
    let owner_boot_id = Uuid::new_v4();
    let starting = store
        .begin_session(&BeginSessionInput {
            request_id: format!("request-warm-{key}"),
            idempotency_key: format!("warm-{key}"),
            employee_id,
            expected_employee_revision: 1,
            owner_boot_id,
        })
        .unwrap();
    assert_eq!(starting.session.state, SessionLifecycleState::Starting);
    let session_id = store
        .mark_session_ready(&MarkSessionReadyInput {
            session_id: starting.session.id,
            expected_revision: 1,
            owner_boot_id,
            native_session_id: format!("native-{key}"),
            native_host_pid: 4242,
        })
        .unwrap()
        .session
        .id;
    (session_id, owner_boot_id)
}

fn root_task(
    store: &Store,
    key: &str,
    conversation_id: Uuid,
    authority_mode: AuthorityMode,
) -> lazoo_protocol::TaskRecord {
    let mut input = record_input(key, conversation_id, user(), None, "root objective");
    input.authority_mode = authority_mode;
    store.record_turn(&input).unwrap().task.unwrap()
}

fn delegate_input(
    key: &str,
    parent_task_id: Uuid,
    actor: MessageEndpoint,
    assigned_employee_id: Uuid,
    requested_authority: Option<AuthorityMode>,
    roster_revision: u64,
) -> DelegateTaskInput {
    DelegateTaskInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        parent_task_id,
        actor,
        assigned_employee_id,
        objective: format!("objective {key}"),
        references: Vec::new(),
        requested_authority,
        roster_revision,
    }
}

fn user() -> MessageSender {
    MessageSender::User(UserMessageEndpoint {
        kind: UserMessageKind::User,
    })
}

fn record_input(
    key: &str,
    conversation_id: Uuid,
    author: MessageSender,
    task_id: Option<Uuid>,
    text: &str,
) -> RecordTurnInput {
    RecordTurnInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        conversation_id,
        author,
        recipient: None,
        task_id,
        authority_mode: AuthorityMode::Ask,
        text: text.to_owned(),
        activate_if_needed: false,
        max_attempts: 3,
    }
}

fn list_for_conversation(conversation_id: Uuid) -> ListTasksInput {
    ListTasksInput {
        after_sequence: 0,
        limit: 100,
        root_task_id: None,
        parent_task_id: None,
        conversation_id: Some(conversation_id),
        project_id: None,
        assigned_employee_id: None,
        state: None,
    }
}

#[test]
fn user_turn_creates_one_atomic_root_task_and_replay_is_write_free() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store.create_employee(&employee("Rhea")).unwrap().employee;
    let conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "conversation-request".to_owned(),
            idempotency_key: "conversation".to_owned(),
            project_id: None,
            lead_employee_id: lead.id,
            title: None,
        })
        .unwrap()
        .conversation;

    let input = record_input("root", conversation.id, user(), None, "inspect the parser");
    let result = store.record_turn(&input).unwrap();
    let task = result.task.as_ref().unwrap();
    assert_eq!(result.turn.id, task.id);
    assert_eq!(result.turn.task_id, Some(result.turn.id));
    assert_eq!(result.message.envelope.task_id, Some(result.turn.id));
    assert_eq!(task.objective, "inspect the parser");
    assert_eq!(task.state, TaskState::Accepted);
    assert_eq!(task.assigned_employee_id, lead.id);
    assert_eq!(task.requested_authority, AuthorityMode::Ask);
    assert_eq!(task.effective_authority, AuthorityMode::Ask);
    assert_eq!(task.active_turn_id, Some(task.id));
    assert!(task.active_session_id.is_none());

    let event_count = store.list_events(0, 100).unwrap().len();
    let pending_outbox = store.status().unwrap().outbox_pending;
    let replay = store.record_turn(&input).unwrap();
    assert_eq!(replay.turn.id, result.turn.id);
    assert_eq!(replay.task, result.task);
    assert_eq!(replay.event_sequence, None);
    assert_eq!(store.list_events(0, 100).unwrap().len(), event_count);
    assert_eq!(store.status().unwrap().outbox_pending, pending_outbox);

    let listed = store
        .list_tasks(&list_for_conversation(conversation.id))
        .unwrap();
    assert_eq!(listed.len(), 1);
    let detail = store.get_task(task.id).unwrap();
    assert!(detail.children.is_empty());
    assert_eq!(detail.evidence.len(), 1);
    assert_eq!(detail.evidence[0].kind, TaskEvidenceKind::Accepted);

    let follow_up = store
        .record_turn(&record_input(
            "follow-up",
            conversation.id,
            user(),
            Some(task.id),
            "add regression coverage",
        ))
        .unwrap();
    assert_eq!(follow_up.turn.task_id, Some(task.id));
    assert_eq!(follow_up.message.envelope.task_id, Some(task.id));
    assert_eq!(follow_up.task.as_ref().map(|value| value.id), Some(task.id));
    assert_eq!(
        store
            .list_tasks(&list_for_conversation(conversation.id))
            .unwrap()
            .len(),
        1
    );

    let employee_author = MessageSender::Employee(MessageEndpoint {
        employee_id: lead.id,
        session_id: None,
    });
    let error = store
        .record_turn(&record_input(
            "employee-without-task",
            conversation.id,
            employee_author,
            None,
            "unlinked work",
        ))
        .unwrap_err();
    assert!(matches!(error, StoreError::TurnTaskRequired));
}

#[test]
fn canonical_messages_reject_missing_or_cross_scope_tasks_without_writes() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store.create_employee(&employee("Rhea")).unwrap().employee;
    let first = store
        .create_conversation(&CreateConversationInput {
            request_id: "first-request".to_owned(),
            idempotency_key: "first".to_owned(),
            project_id: None,
            lead_employee_id: lead.id,
            title: None,
        })
        .unwrap()
        .conversation;
    let second = store
        .create_conversation(&CreateConversationInput {
            request_id: "second-request".to_owned(),
            idempotency_key: "second".to_owned(),
            project_id: None,
            lead_employee_id: lead.id,
            title: None,
        })
        .unwrap()
        .conversation;
    let root = store
        .record_turn(&record_input("root", first.id, user(), None, "root"))
        .unwrap()
        .task
        .unwrap();
    let baseline = store.list_messages(0, 100).unwrap().len();

    let base = SendMessageInput {
        request_id: "message-request".to_owned(),
        idempotency_key: "message".to_owned(),
        sender: MessageEndpoint {
            employee_id: lead.id,
            session_id: None,
        },
        recipient: MessageEndpoint {
            employee_id: lead.id,
            session_id: None,
        },
        project_id: None,
        conversation_id: Some(first.id),
        task_id: Some(Uuid::new_v4()),
        correlation_id: None,
        causation_id: None,
        kind: MessageKind::Peer,
        body: json!({"text": "missing"}),
        references: Vec::new(),
        max_attempts: 3,
    };
    let missing = store.send_message(&base).unwrap_err();
    assert!(matches!(missing, StoreError::MessageTaskNotFound { .. }));

    let mismatched = SendMessageInput {
        request_id: "scope-request".to_owned(),
        idempotency_key: "scope".to_owned(),
        conversation_id: Some(second.id),
        task_id: Some(root.id),
        ..base
    };
    let scope = store.send_message(&mismatched).unwrap_err();
    assert!(matches!(scope, StoreError::MessageTaskScope { .. }));
    assert_eq!(store.list_messages(0, 100).unwrap().len(), baseline);
}

#[test]
fn delegation_ask_stays_proposed_until_user_acceptance_creates_one_message() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store.create_employee(&employee("Rhea")).unwrap().employee;
    let worker = store.create_employee(&employee("Mira")).unwrap().employee;
    let conversation_id = create_conversation(&store, "conversation", lead.id);
    let root = root_task(&store, "root", conversation_id, AuthorityMode::Ask);
    let lead_session = warm_session(&store, "lead", lead.id);
    let input = delegate_input(
        "ask-child",
        root.id,
        MessageEndpoint {
            employee_id: lead.id,
            session_id: Some(lead_session),
        },
        worker.id,
        None,
        store.roster_snapshot().unwrap().revision,
    );

    let proposed = store.delegate_task(&input).unwrap();
    assert_eq!(proposed.task.state, TaskState::Proposed);
    assert_eq!(proposed.task.parent_task_id, Some(root.id));
    assert_eq!(proposed.task.root_task_id, root.id);
    assert_eq!(proposed.task.depth, 1);
    assert!(proposed.delegation_message.is_none());
    let proposed_detail = store.get_task(proposed.task.id).unwrap();
    assert_eq!(proposed_detail.evidence.len(), 1);
    assert_eq!(proposed_detail.evidence[0].kind, TaskEvidenceKind::Proposed);

    let accept = AcceptTaskInput {
        request_id: "accept-request".to_owned(),
        idempotency_key: "accept-child".to_owned(),
        task_id: proposed.task.id,
        expected_revision: 1,
    };
    let accepted = store.accept_task(&accept).unwrap();
    let replay = store.accept_task(&accept).unwrap();
    let message = accepted.delegation_message.as_ref().unwrap();
    assert_eq!(accepted.task.state, TaskState::Accepted);
    assert_eq!(accepted.task.revision, 2);
    assert_eq!(
        accepted.task.delegation_message_id,
        Some(message.envelope.id)
    );
    assert_eq!(
        message.envelope.sender,
        MessageSender::Employee(input.actor)
    );
    assert_eq!(message.envelope.recipient.employee_id, worker.id);
    assert_eq!(message.envelope.task_id, Some(accepted.task.id));
    assert_eq!(
        message.envelope.body,
        json!({"objective": "objective ask-child"})
    );
    assert_eq!(replay.task.id, accepted.task.id);
    assert_eq!(
        replay.delegation_message.as_ref().unwrap().envelope.id,
        message.envelope.id
    );
    assert_eq!(replay.event_sequence, None);
    assert_eq!(store.get_task(proposed.task.id).unwrap().evidence.len(), 2);
}

#[test]
fn delegation_non_ask_narrows_authority_and_accepts_atomically() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee_with(
            "Rhea",
            AuthorityMode::FullAutonomy,
            true,
            true,
        ))
        .unwrap()
        .employee;
    let worker = store
        .create_employee(&employee_with(
            "Mira",
            AuthorityMode::FullAutonomy,
            true,
            true,
        ))
        .unwrap()
        .employee;
    let conversation_id = create_conversation(&store, "conversation", lead.id);
    let root = root_task(&store, "root", conversation_id, AuthorityMode::FullAutonomy);
    let lead_session = warm_session(&store, "lead", lead.id);
    let input = delegate_input(
        "bounded-child",
        root.id,
        MessageEndpoint {
            employee_id: lead.id,
            session_id: Some(lead_session),
        },
        worker.id,
        Some(AuthorityMode::SelfDecision),
        store.roster_snapshot().unwrap().revision,
    );

    let child = store.delegate_task(&input).unwrap();
    assert_eq!(child.task.requested_authority, AuthorityMode::SelfDecision);
    assert_eq!(child.task.effective_authority, AuthorityMode::SelfDecision);
    assert_eq!(child.task.state, TaskState::Accepted);
    assert!(child.delegation_message.is_some());

    let replay = store.delegate_task(&input).unwrap();
    assert_eq!(replay.task.id, child.task.id);
    assert_eq!(
        replay.delegation_message.as_ref().unwrap().envelope.id,
        child.delegation_message.as_ref().unwrap().envelope.id
    );
    assert_eq!(replay.event_sequence, None);
}

#[test]
fn delegation_rejects_wrong_actor_session_self_assignment_and_authority_expansion() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store.create_employee(&employee("Rhea")).unwrap().employee;
    let worker = store.create_employee(&employee("Mira")).unwrap().employee;
    let conversation_id = create_conversation(&store, "conversation", lead.id);
    let root = root_task(&store, "root", conversation_id, AuthorityMode::Ask);
    let lead_session = warm_session(&store, "lead", lead.id);
    let worker_session = warm_session(&store, "worker", worker.id);
    let roster_revision = store.roster_snapshot().unwrap().revision;

    let wrong_actor = store
        .delegate_task(&delegate_input(
            "wrong-actor",
            root.id,
            MessageEndpoint {
                employee_id: worker.id,
                session_id: Some(worker_session),
            },
            lead.id,
            None,
            roster_revision,
        ))
        .unwrap_err();
    assert!(matches!(wrong_actor, StoreError::TaskActorMismatch { .. }));

    let wrong_session = store
        .delegate_task(&delegate_input(
            "wrong-session",
            root.id,
            MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(worker_session),
            },
            worker.id,
            None,
            roster_revision,
        ))
        .unwrap_err();
    assert!(matches!(wrong_session, StoreError::TaskActorSession { .. }));

    let self_assignment = store
        .delegate_task(&delegate_input(
            "self",
            root.id,
            MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(lead_session),
            },
            lead.id,
            None,
            roster_revision,
        ))
        .unwrap_err();
    assert!(matches!(
        self_assignment,
        StoreError::TaskSelfDelegation { .. }
    ));

    let expansion = store
        .delegate_task(&delegate_input(
            "expansion",
            root.id,
            MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(lead_session),
            },
            worker.id,
            Some(AuthorityMode::FullAutonomy),
            roster_revision,
        ))
        .unwrap_err();
    assert!(matches!(
        expansion,
        StoreError::TaskAuthorityExpansion { .. }
    ));
}

#[test]
fn delegation_rejects_stale_roster_and_unavailable_assignee_without_writes() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store.create_employee(&employee("Rhea")).unwrap().employee;
    let mut unavailable_input = employee_with("Mira", AuthorityMode::Ask, true, false);
    unavailable_input.provider = "opencode".to_owned();
    let unavailable = store.create_employee(&unavailable_input).unwrap().employee;
    let conversation_id = create_conversation(&store, "conversation", lead.id);
    let root = root_task(&store, "root", conversation_id, AuthorityMode::Ask);
    let lead_session = warm_session(&store, "lead", lead.id);
    let current = store.roster_snapshot().unwrap().revision;
    let actor = MessageEndpoint {
        employee_id: lead.id,
        session_id: Some(lead_session),
    };

    let stale = store
        .delegate_task(&delegate_input(
            "stale",
            root.id,
            actor,
            unavailable.id,
            None,
            current - 1,
        ))
        .unwrap_err();
    assert!(matches!(stale, StoreError::TaskRosterStale { .. }));

    let unavailable_error = store
        .delegate_task(&delegate_input(
            "unavailable",
            root.id,
            actor,
            unavailable.id,
            None,
            current,
        ))
        .unwrap_err();
    assert!(matches!(
        unavailable_error,
        StoreError::TaskAssigneeUnavailable { .. }
    ));
    assert!(store.get_task(root.id).unwrap().children.is_empty());
}

#[test]
fn delegation_enforces_depth_four_and_eight_active_children() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let first = store
        .create_employee(&employee_with(
            "Rhea",
            AuthorityMode::FullAutonomy,
            true,
            true,
        ))
        .unwrap()
        .employee;
    let second = store
        .create_employee(&employee_with(
            "Mira",
            AuthorityMode::FullAutonomy,
            true,
            true,
        ))
        .unwrap()
        .employee;
    let conversation_id = create_conversation(&store, "conversation", first.id);
    let root = root_task(&store, "root", conversation_id, AuthorityMode::FullAutonomy);
    let first_session = warm_session(&store, "first", first.id);
    let second_session = warm_session(&store, "second", second.id);
    let roster_revision = store.roster_snapshot().unwrap().revision;

    for index in 0..8 {
        store
            .delegate_task(&delegate_input(
                &format!("fanout-{index}"),
                root.id,
                MessageEndpoint {
                    employee_id: first.id,
                    session_id: Some(first_session),
                },
                second.id,
                Some(AuthorityMode::FullAutonomy),
                roster_revision,
            ))
            .unwrap();
    }
    let ninth = store
        .delegate_task(&delegate_input(
            "fanout-8",
            root.id,
            MessageEndpoint {
                employee_id: first.id,
                session_id: Some(first_session),
            },
            second.id,
            Some(AuthorityMode::FullAutonomy),
            roster_revision,
        ))
        .unwrap_err();
    assert!(matches!(ninth, StoreError::TaskFanoutLimit { .. }));

    let second_root = root_task(
        &store,
        "second-root",
        conversation_id,
        AuthorityMode::FullAutonomy,
    );
    let mut parent_task = second_root;
    for depth in 1..=4 {
        let (actor_id, actor_session, assignee_id) = if depth % 2 == 1 {
            (first.id, first_session, second.id)
        } else {
            (second.id, second_session, first.id)
        };
        parent_task = store
            .delegate_task(&delegate_input(
                &format!("depth-{depth}"),
                parent_task.id,
                MessageEndpoint {
                    employee_id: actor_id,
                    session_id: Some(actor_session),
                },
                assignee_id,
                Some(AuthorityMode::FullAutonomy),
                roster_revision,
            ))
            .unwrap()
            .task;
        assert_eq!(parent_task.depth, depth);
    }
    let depth_five = store
        .delegate_task(&delegate_input(
            "depth-5",
            parent_task.id,
            MessageEndpoint {
                employee_id: first.id,
                session_id: Some(first_session),
            },
            second.id,
            Some(AuthorityMode::FullAutonomy),
            roster_revision,
        ))
        .unwrap_err();
    assert!(matches!(depth_five, StoreError::TaskDepthLimit { .. }));
}

#[test]
fn lifecycle_start_requires_native_accepted_matching_turn() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store.create_employee(&employee("Rhea")).unwrap().employee;
    let (session_id, owner_boot_id) = ready_session(&store, "lead", lead.id);
    let conversation_id = create_conversation(&store, "conversation", lead.id);
    let recorded = store
        .record_turn(&record_input(
            "root",
            conversation_id,
            user(),
            None,
            "root objective",
        ))
        .unwrap();
    let task = recorded.task.as_ref().unwrap();
    let actor = MessageEndpoint {
        employee_id: lead.id,
        session_id: Some(session_id),
    };
    let premature = store
        .start_task(&StartTaskInput {
            request_id: "premature-request".to_owned(),
            idempotency_key: "premature".to_owned(),
            task_id: task.id,
            expected_revision: 1,
            actor,
            turn_id: recorded.turn.id,
            evidence: json!({"native": "missing"}),
        })
        .unwrap_err();
    assert!(matches!(
        premature,
        StoreError::TaskAcceptanceEvidence { .. }
    ));

    let submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "begin-request".to_owned(),
            idempotency_key: "begin".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: 1,
            owner_boot_id,
            recipient_session_id: session_id,
        })
        .unwrap();
    let accepted = store
        .observe_turn(&ObserveTurnInput {
            request_id: "observe-request".to_owned(),
            idempotency_key: "observe".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: submitting.turn.revision,
            kind: "turn-started".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("native-lead".to_owned()),
            native_turn_id: Some("native-turn-1".to_owned()),
            payload: json!({"status": "in-progress"}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap();
    assert_eq!(accepted.turn.state, TurnState::Accepted);

    let started = store
        .start_task(&StartTaskInput {
            request_id: "start-request".to_owned(),
            idempotency_key: "start".to_owned(),
            task_id: task.id,
            expected_revision: 1,
            actor,
            turn_id: accepted.turn.id,
            evidence: json!({"nativeTurnId": "native-turn-1"}),
        })
        .unwrap();
    assert_eq!(started.task.state, TaskState::Running);
    assert_eq!(started.task.revision, 2);
    assert_eq!(started.task.active_turn_id, Some(accepted.turn.id));
    assert_eq!(started.task.active_session_id, Some(session_id));
    let detail = store.get_task(task.id).unwrap();
    assert_eq!(detail.evidence.len(), 2);
    assert_eq!(detail.evidence[1].kind, TaskEvidenceKind::Started);

    let cancel_requested = store
        .cancel_task(&CancelTaskInput {
            request_id: "cancel-request".to_owned(),
            idempotency_key: "cancel".to_owned(),
            task_id: started.task.id,
            expected_revision: started.task.revision,
            actor: user(),
            evidence: json!({"reason": "user stopped work"}),
        })
        .unwrap();
    assert_eq!(cancel_requested.task.state, TaskState::CancelRequested);
    assert!(cancel_requested.task.terminal_at.is_none());
    assert_eq!(cancel_requested.task.active_turn_id, Some(accepted.turn.id));
}

#[test]
#[allow(clippy::too_many_lines)] // Keep the revision and evidence sequence in one scenario.
fn lifecycle_progress_completion_and_failure_are_revisioned_and_terminal() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store.create_employee(&employee("Rhea")).unwrap().employee;
    let (session_id, owner_boot_id) = ready_session(&store, "lead", lead.id);
    let conversation_id = create_conversation(&store, "conversation", lead.id);
    let recorded = store
        .record_turn(&record_input("root", conversation_id, user(), None, "root"))
        .unwrap();
    let submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "begin-request".to_owned(),
            idempotency_key: "begin".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: 1,
            owner_boot_id,
            recipient_session_id: session_id,
        })
        .unwrap();
    store
        .observe_turn(&ObserveTurnInput {
            request_id: "observe-request".to_owned(),
            idempotency_key: "observe".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: submitting.turn.revision,
            kind: "turn-started".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("native-lead".to_owned()),
            native_turn_id: Some("native-turn-1".to_owned()),
            payload: json!({"status": "in-progress"}),
            transition: Some(TurnState::Accepted),
        })
        .unwrap();
    let actor = MessageEndpoint {
        employee_id: lead.id,
        session_id: Some(session_id),
    };
    let running = store
        .start_task(&StartTaskInput {
            request_id: "start-request".to_owned(),
            idempotency_key: "start".to_owned(),
            task_id: recorded.turn.id,
            expected_revision: 1,
            actor,
            turn_id: recorded.turn.id,
            evidence: json!({"nativeTurnId": "native-turn-1"}),
        })
        .unwrap();
    let progress = store
        .report_task(&ReportTaskInput {
            request_id: "progress-request".to_owned(),
            idempotency_key: "progress".to_owned(),
            task_id: running.task.id,
            expected_revision: running.task.revision,
            actor,
            kind: TaskReportKind::Progress,
            evidence: json!({"completed": 3, "total": 5}),
            failure_code: None,
            failure_message: None,
        })
        .unwrap();
    assert_eq!(progress.task.state, TaskState::Running);
    assert_eq!(progress.task.revision, 3);

    let completed = store
        .report_task(&ReportTaskInput {
            request_id: "complete-request".to_owned(),
            idempotency_key: "complete".to_owned(),
            task_id: progress.task.id,
            expected_revision: progress.task.revision,
            actor,
            kind: TaskReportKind::Completed,
            evidence: json!({"tests": "passed"}),
            failure_code: None,
            failure_message: None,
        })
        .unwrap();
    assert_eq!(completed.task.state, TaskState::Completed);
    assert_eq!(completed.task.revision, 4);
    assert!(completed.task.terminal_at.is_some());
    assert!(completed.task.active_turn_id.is_none());
    let kinds = store
        .get_task(completed.task.id)
        .unwrap()
        .evidence
        .into_iter()
        .map(|evidence| evidence.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            TaskEvidenceKind::Accepted,
            TaskEvidenceKind::Started,
            TaskEvidenceKind::Progress,
            TaskEvidenceKind::Completed,
        ]
    );

    let terminal = store
        .report_task(&ReportTaskInput {
            request_id: "terminal-request".to_owned(),
            idempotency_key: "terminal".to_owned(),
            task_id: completed.task.id,
            expected_revision: completed.task.revision,
            actor,
            kind: TaskReportKind::Failed,
            evidence: json!({"late": true}),
            failure_code: Some("LZ-LATE".to_owned()),
            failure_message: Some("late failure".to_owned()),
        })
        .unwrap_err();
    assert!(matches!(terminal, StoreError::TaskState { .. }));
}

#[test]
fn lifecycle_cancellation_blocks_active_children_and_preserves_provider_uncertainty() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store.create_employee(&employee("Rhea")).unwrap().employee;
    let worker = store.create_employee(&employee("Mira")).unwrap().employee;
    let (lead_session, _) = ready_session(&store, "lead", lead.id);
    let conversation_id = create_conversation(&store, "conversation", lead.id);
    let root = root_task(&store, "root", conversation_id, AuthorityMode::Ask);
    let child = store
        .delegate_task(&delegate_input(
            "child",
            root.id,
            MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(lead_session),
            },
            worker.id,
            None,
            store.roster_snapshot().unwrap().revision,
        ))
        .unwrap();
    let user_actor = user();
    let blocked = store
        .cancel_task(&CancelTaskInput {
            request_id: "blocked-request".to_owned(),
            idempotency_key: "blocked".to_owned(),
            task_id: root.id,
            expected_revision: root.revision,
            actor: user_actor,
            evidence: json!({"reason": "stop"}),
        })
        .unwrap_err();
    assert!(matches!(blocked, StoreError::TaskActiveChildren { .. }));

    let child_cancelled = store
        .cancel_task(&CancelTaskInput {
            request_id: "child-cancel-request".to_owned(),
            idempotency_key: "child-cancel".to_owned(),
            task_id: child.task.id,
            expected_revision: child.task.revision,
            actor: user_actor,
            evidence: json!({"reason": "superseded"}),
        })
        .unwrap();
    assert_eq!(child_cancelled.task.state, TaskState::Cancelled);
    assert!(child_cancelled.task.terminal_at.is_some());

    let root_cancelled = store
        .cancel_task(&CancelTaskInput {
            request_id: "root-cancel-request".to_owned(),
            idempotency_key: "root-cancel".to_owned(),
            task_id: root.id,
            expected_revision: root.revision,
            actor: user_actor,
            evidence: json!({"reason": "stop"}),
        })
        .unwrap();
    assert_eq!(root_cancelled.task.state, TaskState::Cancelled);
}

#[test]
fn lifecycle_user_resolution_is_the_only_exit_from_uncertain() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store.create_employee(&employee("Rhea")).unwrap().employee;
    let conversation_id = create_conversation(&store, "conversation", lead.id);
    let root = root_task(&store, "root", conversation_id, AuthorityMode::Ask);
    store
        .execute_for_test(&format!(
            "UPDATE tasks SET state = 'uncertain', revision = 2 WHERE id = '{}'",
            root.id
        ))
        .unwrap();

    let resolved = store
        .resolve_task(&ResolveTaskInput {
            request_id: "resolve-request".to_owned(),
            idempotency_key: "resolve".to_owned(),
            task_id: root.id,
            expected_revision: 2,
            resolution: TaskResolution::Completed,
            evidence: json!({"decision": "verified manually"}),
            failure_code: None,
            failure_message: None,
        })
        .unwrap();
    assert_eq!(resolved.task.state, TaskState::Completed);
    assert_eq!(resolved.task.revision, 3);
    assert_eq!(
        store
            .get_task(root.id)
            .unwrap()
            .evidence
            .last()
            .unwrap()
            .kind,
        TaskEvidenceKind::Completed
    );
    let replay = store
        .resolve_task(&ResolveTaskInput {
            request_id: "resolve-replay-request".to_owned(),
            idempotency_key: "resolve".to_owned(),
            task_id: root.id,
            expected_revision: 2,
            resolution: TaskResolution::Completed,
            evidence: json!({"decision": "verified manually"}),
            failure_code: None,
            failure_message: None,
        })
        .unwrap();
    assert_eq!(replay.task, resolved.task);
    assert_eq!(replay.event_sequence, None);
}

#[test]
fn lifecycle_failure_rejects_wrong_actor_session_and_stale_revision() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store.create_employee(&employee("Rhea")).unwrap().employee;
    let worker = store.create_employee(&employee("Mira")).unwrap().employee;
    let (lead_session, _) = ready_session(&store, "lead", lead.id);
    let (worker_session, _) = ready_session(&store, "worker", worker.id);
    let conversation_id = create_conversation(&store, "conversation", lead.id);
    let root = root_task(&store, "root", conversation_id, AuthorityMode::Ask);
    let failure = |key: &str, actor: MessageEndpoint, expected_revision: u64| ReportTaskInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        task_id: root.id,
        expected_revision,
        actor,
        kind: TaskReportKind::Failed,
        evidence: json!({"exitCode": 1}),
        failure_code: Some("LZ-WORKER-FAILED".to_owned()),
        failure_message: Some("worker exited".to_owned()),
    };

    let wrong_actor = store
        .report_task(&failure(
            "wrong-actor",
            MessageEndpoint {
                employee_id: worker.id,
                session_id: Some(worker_session),
            },
            1,
        ))
        .unwrap_err();
    assert!(matches!(wrong_actor, StoreError::TaskActorMismatch { .. }));

    let wrong_session = store
        .report_task(&failure(
            "wrong-session",
            MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(worker_session),
            },
            1,
        ))
        .unwrap_err();
    assert!(matches!(wrong_session, StoreError::TaskActorSession { .. }));

    let failed = store
        .report_task(&failure(
            "failed",
            MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(lead_session),
            },
            1,
        ))
        .unwrap();
    assert_eq!(failed.task.state, TaskState::Failed);
    assert_eq!(
        failed.task.failure_code.as_deref(),
        Some("LZ-WORKER-FAILED")
    );
    assert_eq!(
        failed.task.failure_message.as_deref(),
        Some("worker exited")
    );

    let stale = store
        .report_task(&failure(
            "stale",
            MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(lead_session),
            },
            1,
        ))
        .unwrap_err();
    assert!(matches!(stale, StoreError::TaskStale { .. }));
}

#[test]
#[allow(clippy::too_many_lines)] // The restart fixture proves all preserved and reconciled states together.
fn task_restart_reconciliation_marks_only_inflight_work_uncertain_once() {
    let temporary = tempdir().unwrap();
    let database_path = temporary.path().join("lazoo.db");
    let (running_id, cancel_requested_id, accepted_id, proposed_id) = {
        let store = Store::open_path(&database_path).unwrap();
        let lead = store.create_employee(&employee("Rhea")).unwrap().employee;
        let worker = store.create_employee(&employee("Mira")).unwrap().employee;
        let (session_id, old_boot_id) = ready_session(&store, "restart-lead", lead.id);
        let conversation_id = create_conversation(&store, "restart", lead.id);

        let start_root = |key: &str| {
            let recorded = store
                .record_turn(&record_input(key, conversation_id, user(), None, key))
                .unwrap();
            let submitting = store
                .begin_turn_submission(&BeginTurnSubmissionInput {
                    request_id: format!("request-{key}-begin"),
                    idempotency_key: format!("{key}-begin"),
                    turn_id: recorded.turn.id,
                    expected_revision: recorded.turn.revision,
                    owner_boot_id: old_boot_id,
                    recipient_session_id: session_id,
                })
                .unwrap();
            store
                .observe_turn(&ObserveTurnInput {
                    request_id: format!("request-{key}-start"),
                    idempotency_key: format!("{key}-start"),
                    turn_id: recorded.turn.id,
                    expected_revision: submitting.turn.revision,
                    kind: "turn-started".to_owned(),
                    provider: "codex".to_owned(),
                    native_session_id: Some("native-restart".to_owned()),
                    native_turn_id: Some(format!("native-{key}")),
                    payload: json!({"status": "in-progress"}),
                    transition: Some(TurnState::Accepted),
                })
                .unwrap();
            recorded.turn.id
        };

        let running_id = start_root("running");
        let cancel_requested_id = start_root("cancel-requested");
        let cancel_requested = store.get_task(cancel_requested_id).unwrap().task;
        store
            .cancel_task(&CancelTaskInput {
                request_id: "request-restart-cancel".to_owned(),
                idempotency_key: "restart-cancel".to_owned(),
                task_id: cancel_requested.id,
                expected_revision: cancel_requested.revision,
                actor: user(),
                evidence: json!({"reason": "stop"}),
            })
            .unwrap();

        let accepted = root_task(&store, "accepted", conversation_id, AuthorityMode::Ask);
        let proposed = store
            .delegate_task(&delegate_input(
                "proposed",
                accepted.id,
                MessageEndpoint {
                    employee_id: lead.id,
                    session_id: Some(session_id),
                },
                worker.id,
                Some(AuthorityMode::Ask),
                store.roster_snapshot().unwrap().revision,
            ))
            .unwrap()
            .task;
        store.publish_pending_outbox().unwrap();
        (running_id, cancel_requested_id, accepted.id, proposed.id)
    };

    let store = Store::open_path(&database_path).unwrap();
    let new_boot_id = Uuid::new_v4();
    let turns = store.reconcile_turns(new_boot_id).unwrap();
    assert_eq!(turns.turns.len(), 2);
    let result = store.reconcile_tasks().unwrap();
    assert_eq!(result.tasks.len(), 2);
    assert!(result.event_sequence.is_some());
    for task_id in [running_id, cancel_requested_id] {
        let task = store.get_task(task_id).unwrap();
        assert_eq!(task.task.state, TaskState::Uncertain);
        assert_eq!(
            task.task.failure_code.as_deref(),
            Some("LZ-TASK-OWNER-LOST")
        );
        assert!(task.task.active_turn_id.is_none());
        assert!(task.task.active_session_id.is_none());
        assert_eq!(
            task.evidence.last().unwrap().kind,
            TaskEvidenceKind::Uncertain
        );
    }
    assert_eq!(
        store.get_task(accepted_id).unwrap().task.state,
        TaskState::Accepted
    );
    assert_eq!(
        store.get_task(proposed_id).unwrap().task.state,
        TaskState::Proposed
    );
    assert_eq!(
        store
            .list_events(0, 1000)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == "tasks.reconciled")
            .count(),
        1
    );

    let event_count = store.list_events(0, 1000).unwrap().len();
    let replay = store.reconcile_tasks().unwrap();
    assert!(replay.tasks.is_empty());
    assert_eq!(replay.event_sequence, None);
    assert_eq!(store.list_events(0, 1000).unwrap().len(), event_count);
}
