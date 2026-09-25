use lazoo_domain::{MAX_PEER_MESSAGE_BYTES, MAX_TURN_TEXT_BYTES};
use lazoo_protocol::{
    AuthorityMode, EmployeeCheckoutPolicy, IncidentPriority, IncidentSource, MessageDeliveryState,
    MessageEndpoint, MessageKind, MessageMutationResult, MessageSender, ProviderObservation,
    SessionLifecycleState, TaskState, TurnState, UserMessageEndpoint, UserMessageKind,
};
use lazoo_store::{
    BeginSessionInput, BeginTurnSubmissionInput, CancelTaskInput, ConsumePeerMessageInput,
    ConsumeTaskResultInput, CreateConversationInput, CreateEmployeeInput, DelegateTaskInput,
    MarkSessionReadyInput, ObserveTurnInput, QueueIncidentNoticeInput, QueueTaskResultInput,
    RaiseIncidentInput, RecordNoticeTurnInput, RecordTurnInput, ReplyPeerMessageInput,
    SendPeerMessageInput, Store, StoreError,
};
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

struct Fixture {
    store: Store,
    lead: Uuid,
    lead_session: Uuid,
    boot_id: Uuid,
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
            boot_id: owner_boot_id,
            worker: worker.id,
            root,
            child,
        },
    )
}

fn lead_turn(fixture: &Fixture) -> lazoo_protocol::ConversationTurnSummary {
    fixture
        .store
        .list_turns(fixture.root.conversation_id, 0, 10)
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
}

/// A turn on the child task addressed to the worker, recorded by the user so a store test can
/// act as the worker without the daemon's dispatch pipeline.
fn worker_turn(fixture: &Fixture, key: &str) -> lazoo_protocol::ConversationTurnRecord {
    fixture
        .store
        .record_turn(&RecordTurnInput {
            request_id: format!("request-{key}"),
            idempotency_key: key.to_owned(),
            conversation_id: fixture.root.conversation_id,
            author: user(),
            recipient: Some(MessageEndpoint {
                employee_id: fixture.worker,
                session_id: None,
            }),
            task_id: Some(fixture.child.id),
            authority_mode: AuthorityMode::SelfDecision,
            text: "Implement the parser fix".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap()
        .turn
}

fn send(
    fixture: &Fixture,
    key: &str,
    turn_id: Uuid,
    recipient: Uuid,
    content: &str,
) -> Result<MessageMutationResult, StoreError> {
    fixture.store.send_peer_message(&SendPeerMessageInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        turn_id,
        recipient_employee_id: recipient,
        content: content.to_owned(),
        references: Vec::new(),
    })
}

fn reply(
    fixture: &Fixture,
    key: &str,
    turn_id: Uuid,
    message_id: Uuid,
    content: &str,
) -> Result<MessageMutationResult, StoreError> {
    fixture.store.reply_peer_message(&ReplyPeerMessageInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        turn_id,
        message_id,
        content: content.to_owned(),
        references: Vec::new(),
    })
}

fn consume(
    fixture: &Fixture,
    key: &str,
    message_id: Uuid,
    turn_id: Uuid,
    via: &'static str,
) -> Result<MessageMutationResult, StoreError> {
    fixture
        .store
        .consume_peer_message(&ConsumePeerMessageInput {
            request_id: format!("request-{key}"),
            message_id,
            turn_id,
            via,
        })
}

/// Drives a recorded lead turn through submission, acceptance, and completion on the lead's
/// idle session, exactly as the daemon does, and returns the lead's task afterwards.
fn complete_lead_turn(fixture: &Fixture, turn_id: Uuid, key: &str) -> lazoo_protocol::TaskRecord {
    let store = &fixture.store;
    let turn = store.get_turn(turn_id).unwrap();
    let submitting = store
        .begin_turn_submission(&BeginTurnSubmissionInput {
            request_id: format!("request-{key}-begin"),
            idempotency_key: format!("{key}-begin"),
            turn_id,
            expected_revision: turn.revision,
            owner_boot_id: fixture.boot_id,
            recipient_session_id: fixture.lead_session,
        })
        .unwrap();
    let mut revision = submitting.turn.revision;
    let mut task = None;
    for (stage, status, transition) in [
        ("start", "in-progress", TurnState::Accepted),
        ("complete", "completed", TurnState::Completed),
    ] {
        let observed = store
            .observe_turn(&ObserveTurnInput {
                request_id: format!("request-{key}-{stage}"),
                idempotency_key: format!("{key}-{stage}"),
                turn_id,
                expected_revision: revision,
                kind: format!("turn-{stage}"),
                provider: "codex".to_owned(),
                native_session_id: Some("native-lead".to_owned()),
                native_turn_id: Some(format!("native-{key}")),
                payload: json!({"status": status, "output": "noted"}),
                transition: Some(transition),
            })
            .unwrap();
        revision = observed.turn.revision;
        task = observed.task;
    }
    task.unwrap()
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

#[test]
#[allow(clippy::too_many_lines)] // Refusals and the full envelope of one message share a fixture.
fn peer_messages_link_both_tasks_and_queue_once() {
    let (_temporary, fixture) = fixture();
    let store = &fixture.store;
    let lead_turn = lead_turn(&fixture);
    let stranger = store
        .create_employee(&employee("Noor", AuthorityMode::FullAutonomy))
        .unwrap()
        .employee;

    let before = store.list_messages(0, 100).unwrap().len();
    let selfish = send(&fixture, "self", lead_turn.id, fixture.lead, "hello").unwrap_err();
    assert!(matches!(selfish, StoreError::PeerRecipientSelf { .. }));
    let outside = send(&fixture, "outside", lead_turn.id, stranger.id, "hello").unwrap_err();
    assert!(matches!(
        outside,
        StoreError::PeerRecipientTaskNotFound { .. }
    ));
    let empty = send(&fixture, "empty", lead_turn.id, fixture.worker, "   ").unwrap_err();
    assert!(matches!(empty, StoreError::MessageValidation(_)));
    let oversized = send(
        &fixture,
        "big",
        lead_turn.id,
        fixture.worker,
        &"x".repeat(MAX_PEER_MESSAGE_BYTES + 1),
    )
    .unwrap_err();
    assert!(matches!(oversized, StoreError::MessageValidation(_)));
    assert_eq!(
        store.list_messages(0, 100).unwrap().len(),
        before,
        "a refused send leaves nothing behind"
    );

    let sent = send(
        &fixture,
        "question",
        lead_turn.id,
        fixture.worker,
        "Use the spacing token from tokens.md.",
    )
    .unwrap();
    assert!(sent.event_sequence.is_some());
    let envelope = &sent.message.envelope;
    assert_eq!(envelope.kind, MessageKind::Peer);
    assert_eq!(
        envelope.sender,
        MessageSender::Employee(MessageEndpoint {
            employee_id: fixture.lead,
            session_id: Some(fixture.lead_session),
        }),
        "the sender is the turn's own endpoint"
    );
    assert_eq!(
        envelope.recipient,
        MessageEndpoint {
            employee_id: fixture.worker,
            session_id: None,
        }
    );
    assert_eq!(envelope.task_id, Some(fixture.child.id));
    assert_eq!(envelope.conversation_id, Some(fixture.root.conversation_id));
    assert_eq!(envelope.causation_id, None);
    assert_eq!(envelope.correlation_id, envelope.id);
    assert_eq!(envelope.body["kind"], "peer-message");
    assert_eq!(
        envelope.body["content"],
        "Use the spacing token from tokens.md."
    );
    assert_eq!(envelope.body["senderEmployeeName"], "Rhea");
    assert_eq!(envelope.body["senderTaskId"], fixture.root.id.to_string());
    assert_eq!(
        envelope.body["recipientTaskId"],
        fixture.child.id.to_string()
    );
    assert!(envelope.body["replyToMessageId"].is_null());
    let text = envelope.body["text"].as_str().unwrap();
    assert!(
        text.starts_with(&format!(
            "[Lazoo peer message] Rhea (task {}) sent you message {} about your task {}:",
            fixture.root.id, envelope.id, fixture.child.id
        )),
        "{text}"
    );
    assert_eq!(
        envelope
            .references
            .iter()
            .map(|reference| reference.target.clone())
            .collect::<Vec<_>>(),
        vec![fixture.root.id.to_string(), fixture.child.id.to_string()],
        "both tasks are linked"
    );
    assert_eq!(sent.message.delivery.state, MessageDeliveryState::Queued);

    let replay = send(
        &fixture,
        "question",
        lead_turn.id,
        fixture.worker,
        "different text",
    )
    .unwrap();
    assert_eq!(replay.message, sent.message);
    assert!(replay.event_sequence.is_none());

    let inbox = store
        .pending_peer_messages(fixture.worker, fixture.child.id)
        .unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].envelope.id, envelope.id);
    assert!(
        store
            .pending_peer_messages(fixture.lead, fixture.root.id)
            .unwrap()
            .is_empty()
    );
    let notices = store.pending_lead_notices(10).unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].envelope.id, envelope.id);

    // The child has not started: delivering the message now would start it with a peer
    // message instead of its objective, so the message waits.
    let early = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-early".to_owned(),
            message_id: envelope.id,
        })
        .unwrap_err();
    assert!(matches!(early, StoreError::TurnTaskScope { .. }));
}

#[test]
fn replies_thread_under_the_question_and_return_to_the_sender_task() {
    let (_temporary, fixture) = fixture();
    let store = &fixture.store;
    let lead_turn = lead_turn(&fixture);
    let question = send(
        &fixture,
        "question",
        lead_turn.id,
        fixture.worker,
        "Which token?",
    )
    .unwrap()
    .message;
    let worker_turn = worker_turn(&fixture, "worker-turn");

    // Only the addressed employee, from the addressed task, can reply, and only to a peer message.
    let own = reply(
        &fixture,
        "own",
        lead_turn.id,
        question.envelope.id,
        "Answering myself",
    )
    .unwrap_err();
    assert!(matches!(own, StoreError::PeerMessageScope { .. }));
    let delegation = store
        .list_messages(0, 100)
        .unwrap()
        .into_iter()
        .find(|message| message.kind == MessageKind::Delegation)
        .unwrap();
    let not_peer = reply(&fixture, "not-peer", worker_turn.id, delegation.id, "hi").unwrap_err();
    assert!(matches!(not_peer, StoreError::PeerMessageScope { .. }));
    let missing = reply(&fixture, "missing", worker_turn.id, Uuid::new_v4(), "hi").unwrap_err();
    assert!(matches!(missing, StoreError::MessageNotFound { .. }));

    let answer = reply(
        &fixture,
        "answer",
        worker_turn.id,
        question.envelope.id,
        "spacing.md",
    )
    .unwrap();
    assert!(answer.event_sequence.is_some());
    let envelope = &answer.message.envelope;
    assert_eq!(envelope.kind, MessageKind::Reply);
    assert_eq!(envelope.causation_id, Some(question.envelope.id));
    assert_eq!(envelope.correlation_id, question.envelope.correlation_id);
    assert_eq!(
        envelope.sender,
        MessageSender::Employee(MessageEndpoint {
            employee_id: fixture.worker,
            session_id: None,
        })
    );
    assert_eq!(envelope.recipient.employee_id, fixture.lead);
    assert_eq!(envelope.task_id, Some(fixture.root.id));
    assert_eq!(
        envelope.body["replyToMessageId"],
        question.envelope.id.to_string()
    );
    assert_eq!(envelope.body["senderTaskId"], fixture.child.id.to_string());
    let text = envelope.body["text"].as_str().unwrap();
    assert!(
        text.starts_with(&format!(
            "[Lazoo peer reply] Mira (task {}) replied to your message {} with message {}",
            fixture.child.id, question.envelope.id, envelope.id
        )),
        "{text}"
    );
    assert_eq!(
        store
            .pending_peer_messages(fixture.lead, fixture.root.id)
            .unwrap()
            .len(),
        1
    );

    // The idle lead receives the reply as a turn authored by the worker.
    let recorded = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-reply-turn".to_owned(),
            message_id: envelope.id,
        })
        .unwrap();
    assert_eq!(recorded.turn.state, TurnState::Recorded);
    assert_eq!(recorded.turn.text, text);
    assert_eq!(recorded.turn.author, envelope.sender);
    assert_eq!(recorded.turn.task_id, Some(fixture.root.id));
}

#[test]
#[allow(clippy::too_many_lines)] // In-turn consumption and the stale-turn cleanup share a fixture.
fn peer_messages_are_consumed_in_turn_with_evidence_and_close_an_orphan_notice_turn() {
    let (_temporary, fixture) = fixture();
    let store = &fixture.store;
    let lead_turn = lead_turn(&fixture);
    let question = send(
        &fixture,
        "question",
        lead_turn.id,
        fixture.worker,
        "Which token?",
    )
    .unwrap()
    .message;
    let worker_turn = worker_turn(&fixture, "worker-turn");

    let foreign = consume(
        &fixture,
        "foreign",
        question.envelope.id,
        lead_turn.id,
        "task_wait",
    )
    .unwrap_err();
    assert!(matches!(foreign, StoreError::PeerMessageScope { .. }));

    let consumed = consume(
        &fixture,
        "consume",
        question.envelope.id,
        worker_turn.id,
        "task_wait",
    )
    .unwrap();
    assert!(consumed.event_sequence.is_some());
    assert_eq!(
        consumed.message.delivery.state,
        MessageDeliveryState::Acknowledged
    );
    let attempt = consumed.message.attempts.last().unwrap();
    assert_eq!(
        attempt.acknowledgement_key.as_deref(),
        Some(format!("task-wait:{}", worker_turn.id).as_str())
    );
    assert_eq!(
        attempt.acknowledgement_evidence.as_ref().unwrap()["via"],
        "task_wait"
    );
    assert!(
        store
            .pending_peer_messages(fixture.worker, fixture.child.id)
            .unwrap()
            .is_empty()
    );
    let replay = consume(
        &fixture,
        "consume-replay",
        question.envelope.id,
        worker_turn.id,
        "task_wait",
    )
    .unwrap();
    assert_eq!(replay.message, consumed.message);
    assert!(replay.event_sequence.is_none());
    let late = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-late".to_owned(),
            message_id: question.envelope.id,
        })
        .unwrap_err();
    assert!(matches!(late, StoreError::MessageState { .. }));

    // A reply reaches a dormant lead as a recorded notice turn. When the user resumes the lead
    // and it reads the reply inside its new turn, that stale notice turn is closed so it can
    // neither deliver the reply twice nor block later notices.
    let answer = reply(
        &fixture,
        "answer",
        worker_turn.id,
        question.envelope.id,
        "spacing.md",
    )
    .unwrap()
    .message;
    let orphan = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-orphan".to_owned(),
            message_id: answer.envelope.id,
        })
        .unwrap()
        .turn;
    assert_eq!(orphan.state, TurnState::Recorded);
    let resumed = store
        .record_turn(&RecordTurnInput {
            request_id: "request-resume".to_owned(),
            idempotency_key: "resume".to_owned(),
            conversation_id: fixture.root.conversation_id,
            author: user(),
            recipient: None,
            task_id: Some(fixture.root.id),
            authority_mode: AuthorityMode::FullAutonomy,
            text: "Carry on".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap()
        .turn;
    let read = consume(
        &fixture,
        "read",
        answer.envelope.id,
        resumed.id,
        "message_send",
    )
    .unwrap();
    assert_eq!(
        read.message.delivery.state,
        MessageDeliveryState::Acknowledged
    );
    assert_eq!(
        read.message
            .attempts
            .last()
            .unwrap()
            .acknowledgement_evidence
            .as_ref()
            .unwrap()["via"],
        "message_send"
    );
    assert_eq!(
        store.get_turn(orphan.id).unwrap().state,
        TurnState::Cancelled,
        "the stale notice turn is closed"
    );
    assert!(store.pending_lead_notices(10).unwrap().is_empty());
    let events: Vec<String> = store
        .list_events(0, 1000)
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .collect();
    assert!(events.iter().any(|kind| kind == "turn.cancelled"));
    assert_eq!(
        store.get_task(fixture.root.id).unwrap().task.state,
        TaskState::Running,
        "closing the stale turn touches no task"
    );
}

#[test]
fn peer_turns_never_complete_the_recipient_task() {
    let (_temporary, fixture) = fixture();
    let store = &fixture.store;
    let worker_turn = worker_turn(&fixture, "worker-turn");
    let question = send(
        &fixture,
        "question",
        worker_turn.id,
        fixture.lead,
        "Is the parser fix still wanted?",
    )
    .unwrap()
    .message;
    assert_eq!(question.envelope.task_id, Some(fixture.root.id));
    cancel_child(&fixture);
    assert_eq!(
        store.get_task(fixture.root.id).unwrap().task.state,
        TaskState::Running
    );

    // With no unfinished child left, a result notice turn would complete the lead's task; the
    // peer turn that is delivered first must not, or the result would be lost.
    let recorded = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-question-turn".to_owned(),
            message_id: question.envelope.id,
        })
        .unwrap();
    let after_peer = complete_lead_turn(&fixture, recorded.turn.id, "question");
    assert_eq!(after_peer.state, TaskState::Running);
    assert!(after_peer.active_turn_id.is_none());
    assert_eq!(
        store
            .get_message(question.envelope.id)
            .unwrap()
            .delivery
            .state,
        MessageDeliveryState::Acknowledged
    );

    let result = store
        .queue_task_result(&QueueTaskResultInput {
            request_id: "request-result".to_owned(),
            child_task_id: fixture.child.id,
        })
        .unwrap()
        .message;
    let recorded = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-result-turn".to_owned(),
            message_id: result.envelope.id,
        })
        .unwrap();
    let after_result = complete_lead_turn(&fixture, recorded.turn.id, "result");
    assert_eq!(after_result.state, TaskState::Completed);
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

// ---------------------------------------------------------------------------------------------
// Coalesced notice turns
// ---------------------------------------------------------------------------------------------

/// Delegates one more child to the worker, cancels it, and queues its result notice.
fn queue_cancelled_child_result(fixture: &Fixture, key: &str) -> lazoo_protocol::MessageRecord {
    let store = &fixture.store;
    let child = store
        .delegate_task(&DelegateTaskInput {
            request_id: format!("request-{key}"),
            idempotency_key: key.to_owned(),
            parent_task_id: fixture.root.id,
            actor: MessageEndpoint {
                employee_id: fixture.lead,
                session_id: Some(fixture.lead_session),
            },
            assigned_employee_id: fixture.worker,
            objective: format!("Objective {key}"),
            references: Vec::new(),
            requested_authority: Some(AuthorityMode::SelfDecision),
            roster_revision: store.roster_snapshot().unwrap().revision,
        })
        .unwrap()
        .task;
    store
        .cancel_task(&CancelTaskInput {
            request_id: format!("request-cancel-{key}"),
            idempotency_key: format!("cancel-{key}"),
            task_id: child.id,
            expected_revision: child.revision,
            actor: user(),
            evidence: json!({}),
        })
        .unwrap();
    store
        .queue_task_result(&QueueTaskResultInput {
            request_id: format!("request-result-{key}"),
            child_task_id: child.id,
        })
        .unwrap()
        .message
}

fn text_of(message: &lazoo_protocol::MessageRecord) -> &str {
    message.envelope.body["text"].as_str().unwrap()
}

#[test]
#[allow(clippy::too_many_lines)] // One batch is recorded, replayed, delivered, and audited.
fn queued_notices_for_one_recipient_coalesce_into_one_turn_with_evidence_for_each() {
    let (_temporary, fixture) = fixture();
    let store = &fixture.store;
    let worker_turn = worker_turn(&fixture, "worker-turn");
    let question = send(
        &fixture,
        "question",
        worker_turn.id,
        fixture.lead,
        "Is the parser fix still wanted?",
    )
    .unwrap()
    .message;
    cancel_child(&fixture);
    let first = store
        .queue_task_result(&QueueTaskResultInput {
            request_id: "request-first".to_owned(),
            child_task_id: fixture.child.id,
        })
        .unwrap()
        .message;
    let second = queue_cancelled_child_result(&fixture, "second");
    assert_eq!(store.pending_lead_notices(10).unwrap().len(), 3);

    let recorded = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-batch".to_owned(),
            message_id: question.envelope.id,
        })
        .unwrap();
    let turn = &recorded.turn;
    assert_eq!(turn.state, TurnState::Recorded);
    assert_eq!(
        turn.message_id, question.envelope.id,
        "the requested notice is the turn's canonical message"
    );
    assert!(
        turn.text
            .starts_with("[Lazoo notices] 3 Lazoo notices are waiting"),
        "{}",
        turn.text
    );
    let positions: Vec<usize> = [&question, &first, &second]
        .iter()
        .map(|message| turn.text.find(text_of(message)).expect("carried whole"))
        .collect();
    assert!(positions[0] < positions[1] && positions[1] < positions[2]);

    let replay = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-companion".to_owned(),
            message_id: second.envelope.id,
        })
        .unwrap();
    assert_eq!(
        replay.turn.id, turn.id,
        "a companion replays to the same turn"
    );
    assert!(replay.event_sequence.is_none());
    assert_eq!(
        store.pending_lead_notices(10).unwrap().len(),
        3,
        "every notice stays queued until the turn is submitted"
    );

    let task = complete_lead_turn(&fixture, turn.id, "batch");
    assert_eq!(
        task.state,
        TaskState::Completed,
        "a turn carrying results completes the task once no child is left"
    );
    for message in [&question, &first, &second] {
        let delivered = store.get_message(message.envelope.id).unwrap();
        assert_eq!(delivered.delivery.state, MessageDeliveryState::Acknowledged);
    }
    let canonical = store.get_message(question.envelope.id).unwrap();
    assert_eq!(
        canonical
            .attempts
            .last()
            .unwrap()
            .acknowledgement_evidence
            .as_ref()
            .unwrap()["status"],
        "in-progress",
        "the canonical notice keeps the provider's acceptance evidence"
    );
    for companion in [&first, &second] {
        let delivered = store.get_message(companion.envelope.id).unwrap();
        let attempt = delivered.attempts.last().unwrap();
        let evidence = attempt.acknowledgement_evidence.as_ref().unwrap();
        assert_eq!(evidence["turnId"], json!(turn.id));
        assert_eq!(evidence["canonicalMessageId"], json!(question.envelope.id));
        assert_eq!(
            attempt.acknowledgement_key.as_deref(),
            Some(format!("coalesced:{}", turn.id).as_str())
        );
    }
    assert!(store.pending_lead_notices(10).unwrap().is_empty());
}

#[test]
#[allow(clippy::too_many_lines)] // One batch is recorded, split by an in-turn read, and re-recorded.
fn a_notice_read_inside_another_turn_releases_its_batch_for_a_fresh_turn() {
    let (_temporary, fixture) = fixture();
    let store = &fixture.store;
    cancel_child(&fixture);
    let first = store
        .queue_task_result(&QueueTaskResultInput {
            request_id: "request-first".to_owned(),
            child_task_id: fixture.child.id,
        })
        .unwrap()
        .message;
    let second = queue_cancelled_child_result(&fixture, "second");
    // A third child stays accepted so the lead's task keeps running throughout.
    let third = store
        .delegate_task(&DelegateTaskInput {
            request_id: "request-third".to_owned(),
            idempotency_key: "third".to_owned(),
            parent_task_id: fixture.root.id,
            actor: MessageEndpoint {
                employee_id: fixture.lead,
                session_id: Some(fixture.lead_session),
            },
            assigned_employee_id: fixture.worker,
            objective: "Objective third".to_owned(),
            references: Vec::new(),
            requested_authority: Some(AuthorityMode::SelfDecision),
            roster_revision: store.roster_snapshot().unwrap().revision,
        })
        .unwrap()
        .task;
    assert_eq!(third.state, TaskState::Accepted);
    let batch = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-batch".to_owned(),
            message_id: first.envelope.id,
        })
        .unwrap()
        .turn;
    assert!(
        batch.text.starts_with("[Lazoo notices] 2 "),
        "{}",
        batch.text
    );

    // The lead was dormant; the user resumes it and it reads the first result in task_wait.
    let resumed = store
        .record_turn(&RecordTurnInput {
            request_id: "request-resume".to_owned(),
            idempotency_key: "resume".to_owned(),
            conversation_id: fixture.root.conversation_id,
            author: user(),
            recipient: None,
            task_id: Some(fixture.root.id),
            authority_mode: AuthorityMode::FullAutonomy,
            text: "Carry on".to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap()
        .turn;
    let read = store
        .consume_task_result(&ConsumeTaskResultInput {
            request_id: "request-read".to_owned(),
            child_task_id: fixture.child.id,
            turn_id: resumed.id,
        })
        .unwrap();
    assert_eq!(
        read.message.delivery.state,
        MessageDeliveryState::Acknowledged
    );
    assert_eq!(
        store.get_turn(batch.id).unwrap().state,
        TurnState::Cancelled,
        "the undelivered batch turn is closed"
    );
    let waiting = store.pending_lead_notices(10).unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(
        waiting[0].envelope.id, second.envelope.id,
        "the other result stays queued"
    );
    let blocked = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-blocked".to_owned(),
            message_id: second.envelope.id,
        })
        .unwrap_err();
    assert!(matches!(blocked, StoreError::TurnTaskScope { .. }));

    let task = complete_lead_turn(&fixture, resumed.id, "resume");
    assert_eq!(task.state, TaskState::Running);
    let fresh = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-fresh".to_owned(),
            message_id: second.envelope.id,
        })
        .unwrap()
        .turn;
    assert_ne!(fresh.id, batch.id, "the closed turn is never reused");
    assert_eq!(fresh.state, TurnState::Recorded);
    assert_eq!(fresh.message_id, second.envelope.id);
    assert_eq!(
        fresh.text,
        text_of(&second),
        "one remaining notice is delivered as itself"
    );
    let task = complete_lead_turn(&fixture, fresh.id, "fresh");
    assert_eq!(task.state, TaskState::Running);
    assert_eq!(
        store
            .get_message(second.envelope.id)
            .unwrap()
            .delivery
            .state,
        MessageDeliveryState::Acknowledged
    );
    assert!(store.pending_lead_notices(10).unwrap().is_empty());
}

#[test]
fn a_coalesced_turn_stays_within_the_turn_text_limit() {
    let (_temporary, fixture) = fixture();
    let store = &fixture.store;
    let worker_turn = worker_turn(&fixture, "worker-turn");
    let ids: Vec<Uuid> = (0..17)
        .map(|index| {
            send(
                &fixture,
                &format!("q{index}"),
                worker_turn.id,
                fixture.lead,
                &"x".repeat(MAX_PEER_MESSAGE_BYTES),
            )
            .unwrap()
            .message
            .envelope
            .id
        })
        .collect();

    let turn = store
        .record_notice_turn(&RecordNoticeTurnInput {
            request_id: "request-batch".to_owned(),
            message_id: ids[0],
        })
        .unwrap()
        .turn;
    assert!(turn.text.len() <= MAX_TURN_TEXT_BYTES);
    let carried = turn.text.matches("[Lazoo peer message]").count();
    assert!((2..17).contains(&carried), "carried {carried}");
    assert!(
        ids[..carried]
            .iter()
            .all(|id| turn.text.contains(&id.to_string())),
        "the oldest messages ride first"
    );
    assert!(!turn.text.contains(&ids[carried].to_string()));

    let task = complete_lead_turn(&fixture, turn.id, "batch");
    assert_eq!(
        task.state,
        TaskState::Running,
        "a turn carrying only peer messages never completes the task"
    );
    let waiting = store.pending_lead_notices(20).unwrap();
    assert_eq!(
        waiting.len(),
        17 - carried,
        "the overflow waits for the next turn"
    );
    assert_eq!(waiting[0].envelope.id, ids[carried]);
}
