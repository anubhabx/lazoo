use lazoo_protocol::{
    AuthorityMode, EmployeeCheckoutPolicy, IncidentPriority, IncidentSource, MessageDeliveryState,
    MessageEndpoint, MessageKind, MessageSender, ProviderObservation, SessionLifecycleState,
    TaskState, TurnState, UserMessageEndpoint, UserMessageKind,
};
use lazoo_store::{
    BeginSessionInput, BeginTurnSubmissionInput, CancelTaskInput, ConsumeTaskResultInput,
    CreateConversationInput, CreateEmployeeInput, DelegateTaskInput, MarkSessionReadyInput,
    ObserveTurnInput, QueueIncidentNoticeInput, QueueTaskResultInput, RaiseIncidentInput,
    RecordNoticeTurnInput, RecordTurnInput, Store, StoreError,
};
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

struct Fixture {
    store: Store,
    lead: Uuid,
    lead_session: Uuid,
    worker: Uuid,
    root: lazoo_protocol::TaskRecord,
    child: lazoo_protocol::TaskRecord,
}

/// A lead with an idle session, a root task from a user turn, and one accepted child task
/// delegated to a worker, exactly as the daemon leaves them before dispatch.
#[allow(clippy::too_many_lines)] // One sequential fixture mirrors the daemon's lead-turn lifecycle.
fn fixture() -> (tempfile::TempDir, Fixture) {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let lead = store
        .create_employee(&employee("Rhea", AuthorityMode::FullAutonomy))
        .unwrap()
        .employee;
    let worker = store
        .create_employee(&employee("Mira", AuthorityMode::FullAutonomy))
        .unwrap()
        .employee;
    let conversation_id = store
        .create_conversation(&CreateConversationInput {
            request_id: "request-conversation".to_owned(),
            idempotency_key: "conversation".to_owned(),
            project_id: None,
            lead_employee_id: lead.id,
            title: None,
        })
        .unwrap()
        .conversation
        .id;
    let recorded = store
        .record_turn(&RecordTurnInput {
            request_id: "request-root".to_owned(),
            idempotency_key: "root".to_owned(),
            conversation_id,
            author: user(),
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::FullAutonomy,
            text: "Coordinate the parser fix".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap();
    let root = recorded.task.unwrap();
    let (lead_session, owner_boot_id) = warm_session(&store, "lead", lead.id);
    // The lead's user turn runs to completion exactly as the daemon drives it: submitted,
    // accepted by the provider, delegating the child mid-turn, then completed while the
    // child keeps the root task running.
    let submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: "request-root-begin".to_owned(),
            idempotency_key: "root-begin".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: recorded.turn.revision,
            owner_boot_id,
            recipient_session_id: lead_session,
        })
        .unwrap();
    let accepted = store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-root-start".to_owned(),
            idempotency_key: "root-start".to_owned(),
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
    let child = store
        .delegate_task(&DelegateTaskInput {
            request_id: "request-child".to_owned(),
            idempotency_key: "child".to_owned(),
            parent_task_id: root.id,
            actor: MessageEndpoint {
                employee_id: lead.id,
                session_id: Some(lead_session),
            },
            assigned_employee_id: worker.id,
            objective: "Implement the parser fix".to_owned(),
            references: Vec::new(),
            requested_authority: Some(AuthorityMode::SelfDecision),
            roster_revision: store.roster_snapshot().unwrap().revision,
        })
        .unwrap()
        .task;
    assert_eq!(child.state, TaskState::Accepted);
    let completed = store
        .observe_turn(&ObserveTurnInput {
            request_id: "request-root-complete".to_owned(),
            idempotency_key: "root-complete".to_owned(),
            turn_id: recorded.turn.id,
            expected_revision: accepted.turn.revision,
            kind: "turn-completed".to_owned(),
            provider: "codex".to_owned(),
            native_session_id: Some("native-lead".to_owned()),
            native_turn_id: Some("native-turn-1".to_owned()),
            payload: json!({"status": "completed", "output": "delegated"}),
            transition: Some(TurnState::Completed),
        })
        .unwrap();
    assert_eq!(completed.turn.state, TurnState::Completed);
    let root = completed.task.unwrap();
    assert_eq!(root.id, root.root_task_id);
    assert_eq!(root.state, TaskState::Running);
    assert!(root.active_turn_id.is_none());
    (
        temporary,
        Fixture {
            store,
            lead: lead.id,
            lead_session,
            worker: worker.id,
            root,
            child,
        },
    )
}

fn cancel_child(fixture: &Fixture) -> lazoo_protocol::TaskRecord {
    let cancelled = fixture
        .store
        .cancel_task(&CancelTaskInput {
            request_id: "request-cancel-child".to_owned(),
            idempotency_key: "cancel-child".to_owned(),
            task_id: fixture.child.id,
            expected_revision: fixture.child.revision,
            actor: user(),
            evidence: json!({"reason": "test"}),
        })
        .unwrap()
        .task;
    assert_eq!(cancelled.state, TaskState::Cancelled);
    cancelled
}

#[test]
fn terminal_child_results_queue_once_to_the_parent_lead() {
    let (_temporary, fixture) = fixture();
    let store = &fixture.store;

    let premature = store
        .queue_task_result(&QueueTaskResultInput {
            request_id: "request-early".to_owned(),
            child_task_id: fixture.child.id,
        })
        .unwrap_err();
    assert!(matches!(premature, StoreError::TaskState { .. }));
    let rootless = store
        .queue_task_result(&QueueTaskResultInput {
            request_id: "request-root-result".to_owned(),
            child_task_id: fixture.root.id,
        })
        .unwrap_err();
    assert!(matches!(
        rootless,
        StoreError::TaskResultWithoutParent { .. }
    ));
    assert!(store.pending_task_results(10).unwrap().is_empty());

    cancel_child(&fixture);
    assert_eq!(
        store.pending_task_results(10).unwrap(),
        vec![fixture.child.id]
    );
    let queued = store
        .queue_task_result(&QueueTaskResultInput {
            request_id: "request-result".to_owned(),
            child_task_id: fixture.child.id,
        })
        .unwrap();
    assert!(queued.event_sequence.is_some());
    let envelope = &queued.message.envelope;
    assert_eq!(envelope.kind, MessageKind::Result);
    assert_eq!(
        envelope.sender,
        MessageSender::Employee(MessageEndpoint {
            employee_id: fixture.worker,
            session_id: None
        })
    );
    assert_eq!(envelope.recipient.employee_id, fixture.lead);
    assert_eq!(envelope.task_id, Some(fixture.root.id));
    assert_eq!(envelope.conversation_id, Some(fixture.root.conversation_id));
    assert_eq!(
        envelope.causation_id, fixture.child.delegation_message_id,
        "the result threads under the delegation"
    );
    assert_eq!(envelope.body["kind"], "task-result");
    assert_eq!(envelope.body["childTaskId"], fixture.child.id.to_string());
    assert_eq!(envelope.body["state"], "cancelled");
    assert_eq!(envelope.body["remainingChildren"], 0);
    let text = envelope.body["text"].as_str().unwrap();
    assert!(text.contains("assigned to Mira finished with state cancelled"));
    assert!(text.contains(&fixture.root.id.to_string()));
    assert_eq!(envelope.references.len(), 1);
    assert_eq!(envelope.references[0].target, fixture.child.id.to_string());
    assert_eq!(queued.message.delivery.state, MessageDeliveryState::Queued);

    let replay = store
        .queue_task_result(&QueueTaskResultInput {
            request_id: "request-result-replay".to_owned(),
            child_task_id: fixture.child.id,
        })
        .unwrap();
    assert_eq!(replay.message, queued.message);
    assert!(replay.event_sequence.is_none());
    assert!(store.pending_task_results(10).unwrap().is_empty());
    let notices = store.pending_lead_notices(10).unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].envelope.id, envelope.id);
}

#[test]
fn notice_turns_reuse_the_notice_message_and_wait_for_the_lead_to_be_free() {
    let (_temporary, fixture) = fixture();
    let store = &fixture.store;
    cancel_child(&fixture);
    let notice = store
        .queue_task_result(&QueueTaskResultInput {
            request_id: "request-result".to_owned(),
            child_task_id: fixture.child.id,
        })
        .unwrap()
        .message;

    let ordinary = store.list_messages(0, 100).unwrap();
    let delegation = ordinary
        .iter()
        .find(|message| message.kind == MessageKind::Delegation)
        .unwrap();
    let refused = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-not-notice".to_owned(),
            message_id: delegation.id,
        })
        .unwrap_err();
    assert!(matches!(refused, StoreError::MessageNotNotice { .. }));

    let recorded = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-notice-turn".to_owned(),
            message_id: notice.envelope.id,
        })
        .unwrap();
    assert!(recorded.event_sequence.is_some());
    let turn = &recorded.turn;
    assert_eq!(turn.state, TurnState::Recorded);
    assert_eq!(turn.message_id, notice.envelope.id);
    assert_eq!(turn.task_id, Some(fixture.root.id));
    assert_eq!(turn.recipient.employee_id, fixture.lead);
    assert!(turn.recipient.session_id.is_none());
    assert_eq!(turn.author, notice.envelope.sender);
    assert_eq!(turn.text, notice.envelope.body["text"].as_str().unwrap());
    assert_eq!(turn.correlation_id, notice.envelope.correlation_id);
    assert_eq!(
        recorded.task.as_ref().map(|task| task.id),
        Some(fixture.root.id)
    );

    let replay = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-notice-turn-replay".to_owned(),
            message_id: notice.envelope.id,
        })
        .unwrap();
    assert_eq!(replay.turn, recorded.turn);
    assert!(replay.event_sequence.is_none());

    // A second notice for the same lead waits until the first notice turn is finished.
    let second_child = store
        .delegate_task(&DelegateTaskInput {
            request_id: "request-second-child".to_owned(),
            idempotency_key: "second-child".to_owned(),
            parent_task_id: fixture.root.id,
            actor: MessageEndpoint {
                employee_id: fixture.lead,
                session_id: Some(fixture.lead_session),
            },
            assigned_employee_id: fixture.worker,
            objective: "Second objective".to_owned(),
            references: Vec::new(),
            requested_authority: Some(AuthorityMode::SelfDecision),
            roster_revision: store.roster_snapshot().unwrap().revision,
        })
        .unwrap()
        .task;
    store
        .cancel_task(&CancelTaskInput {
            request_id: "request-cancel-second".to_owned(),
            idempotency_key: "cancel-second".to_owned(),
            task_id: second_child.id,
            expected_revision: second_child.revision,
            actor: user(),
            evidence: json!({}),
        })
        .unwrap();
    let second_notice = store
        .queue_task_result(&QueueTaskResultInput {
            request_id: "request-second-result".to_owned(),
            child_task_id: second_child.id,
        })
        .unwrap()
        .message;
    let blocked = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-second-turn".to_owned(),
            message_id: second_notice.envelope.id,
        })
        .unwrap_err();
    assert!(matches!(blocked, StoreError::TurnTaskScope { .. }));
    assert_eq!(store.pending_lead_notices(10).unwrap().len(), 2);
}

#[test]
fn task_wait_consumes_a_result_with_the_waiting_turn_as_evidence() {
    let (_temporary, fixture) = fixture();
    let store = &fixture.store;
    cancel_child(&fixture);
    let lead_turn = store
        .list_turns(fixture.root.conversation_id, 0, 10)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();

    let consumed = store
        .consume_task_result(&ConsumeTaskResultInput {
            request_id: "request-consume".to_owned(),
            child_task_id: fixture.child.id,
            turn_id: lead_turn.id,
        })
        .unwrap();
    assert!(consumed.event_sequence.is_some());
    assert_eq!(consumed.message.envelope.kind, MessageKind::Result);
    assert_eq!(
        consumed.message.delivery.state,
        MessageDeliveryState::Acknowledged
    );
    let attempt = consumed.message.attempts.last().unwrap();
    assert_eq!(
        attempt.acknowledgement_key.as_deref(),
        Some(format!("task-wait:{}", lead_turn.id).as_str())
    );
    assert_eq!(
        attempt.acknowledgement_evidence.as_ref().unwrap()["via"],
        "task_wait"
    );
    assert!(store.pending_task_results(10).unwrap().is_empty());
    assert!(store.pending_lead_notices(10).unwrap().is_empty());

    let replay = store
        .consume_task_result(&ConsumeTaskResultInput {
            request_id: "request-consume-replay".to_owned(),
            child_task_id: fixture.child.id,
            turn_id: lead_turn.id,
        })
        .unwrap();
    assert_eq!(replay.message, consumed.message);
    assert!(replay.event_sequence.is_none());

    let delivered = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-late-turn".to_owned(),
            message_id: consumed.message.envelope.id,
        })
        .unwrap_err();
    assert!(matches!(delivered, StoreError::MessageState { .. }));
}

#[test]
fn a_foreign_turn_cannot_consume_another_leads_result() {
    let (_temporary, fixture) = fixture();
    let store = &fixture.store;
    cancel_child(&fixture);
    let stranger = store
        .create_employee(&employee("Noor", AuthorityMode::FullAutonomy))
        .unwrap()
        .employee;
    let other_conversation = store
        .create_conversation(&CreateConversationInput {
            request_id: "request-other".to_owned(),
            idempotency_key: "other".to_owned(),
            project_id: None,
            lead_employee_id: stranger.id,
            title: None,
        })
        .unwrap()
        .conversation
        .id;
    let foreign_turn = store
        .record_turn(&RecordTurnInput {
            request_id: "request-foreign".to_owned(),
            idempotency_key: "foreign".to_owned(),
            conversation_id: other_conversation,
            author: user(),
            recipient: None,
            task_id: None,
            authority_mode: AuthorityMode::Ask,
            text: "Unrelated work".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap()
        .turn;
    let refused = store
        .consume_task_result(&ConsumeTaskResultInput {
            request_id: "request-foreign-consume".to_owned(),
            child_task_id: fixture.child.id,
            turn_id: foreign_turn.id,
        })
        .unwrap_err();
    assert!(matches!(refused, StoreError::TurnTaskScope { .. }));
    // The refusal rolls the whole consume back: no notice was queued behind the stranger.
    assert!(store.pending_lead_notices(10).unwrap().is_empty());
    assert_eq!(
        store.pending_task_results(10).unwrap(),
        vec![fixture.child.id]
    );
}

#[test]
fn priority_incidents_on_a_child_notify_the_lead_once() {
    let (_temporary, fixture) = fixture();
    let store = &fixture.store;
    let raise = |key: &str, employee: Uuid, task: Uuid| RaiseIncidentInput {
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

    let first = store
        .raise_incident(&raise("worker-1", fixture.worker, fixture.child.id))
        .unwrap()
        .incident;
    assert_eq!(first.priority, IncidentPriority::Normal);
    let normal = store
        .queue_incident_notice(&QueueIncidentNoticeInput {
            request_id: "request-normal".to_owned(),
            incident_id: first.id,
        })
        .unwrap_err();
    assert!(matches!(
        normal,
        StoreError::IncidentNoticeNotApplicable { .. }
    ));
    assert!(store.pending_incident_notices(10).unwrap().is_empty());

    store
        .raise_incident(&raise("worker-2", fixture.worker, fixture.child.id))
        .unwrap();
    let escalated = store
        .raise_incident(&raise("worker-3", fixture.worker, fixture.child.id))
        .unwrap()
        .incident;
    assert_eq!(escalated.priority, IncidentPriority::Priority);
    assert_eq!(
        store.pending_incident_notices(10).unwrap(),
        vec![escalated.id]
    );

    let queued = store
        .queue_incident_notice(&QueueIncidentNoticeInput {
            request_id: "request-notice".to_owned(),
            incident_id: escalated.id,
        })
        .unwrap();
    assert!(queued.event_sequence.is_some());
    let envelope = &queued.message.envelope;
    assert_eq!(envelope.kind, MessageKind::Status);
    assert_eq!(envelope.recipient.employee_id, fixture.lead);
    assert_eq!(envelope.task_id, Some(fixture.root.id));
    assert_eq!(
        envelope.sender,
        MessageSender::Employee(MessageEndpoint {
            employee_id: fixture.worker,
            session_id: None
        })
    );
    assert_eq!(envelope.body["kind"], "incident-notice");
    assert_eq!(envelope.body["code"], "LZ-MESSAGE-TIMEOUT");
    assert_eq!(envelope.body["occurrences"], 3);
    assert_eq!(envelope.body["taskId"], fixture.child.id.to_string());
    let text = envelope.body["text"].as_str().unwrap();
    assert!(text.contains("Priority incident LZ-MESSAGE-TIMEOUT"));
    assert!(text.contains(" for Mira: "));
    assert_eq!(envelope.references.len(), 2);
    assert_eq!(envelope.references[0].kind, "incident");
    assert_eq!(envelope.references[1].kind, "task");

    let replay = store
        .queue_incident_notice(&QueueIncidentNoticeInput {
            request_id: "request-notice-replay".to_owned(),
            incident_id: escalated.id,
        })
        .unwrap();
    assert_eq!(replay.message, queued.message);
    assert!(replay.event_sequence.is_none());
    assert!(store.pending_incident_notices(10).unwrap().is_empty());

    // Incidents about the lead itself never notify the lead.
    for key in ["lead-1", "lead-2", "lead-3"] {
        store
            .raise_incident(&raise(key, fixture.lead, fixture.root.id))
            .unwrap();
    }
    assert!(store.pending_incident_notices(10).unwrap().is_empty());
    let recorded = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-status-turn".to_owned(),
            message_id: envelope.id,
        })
        .unwrap();
    assert_eq!(recorded.turn.state, TurnState::Recorded);
    assert_eq!(recorded.turn.text, text);
}

fn employee(name: &str, authority_mode: AuthorityMode) -> CreateEmployeeInput {
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
        enabled: true,
        provider_observation: Some(ProviderObservation {
            provider: "codex".to_owned(),
            verification: "configured".to_owned(),
            version: Some("0.153.4".to_owned()),
            protocol: Some("app-server/v2".to_owned()),
            capabilities: json!({"session": true}),
            authentication: json!({"native": "ready", "external": "unknown"}),
            degradation_reason: None,
            checked_at: "2026-09-24T00:00:00Z".to_owned(),
        }),
    }
}

fn warm_session(store: &Store, key: &str, employee_id: Uuid) -> (Uuid, Uuid) {
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

fn user() -> MessageSender {
    MessageSender::User(UserMessageEndpoint {
        kind: UserMessageKind::User,
    })
}
