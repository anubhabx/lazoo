use lazoo_protocol::{
    AuthorityMode, EmployeeCheckoutPolicy, MessageDeliveryState, MessageEndpoint, MessageKind,
    MessageSender,
};
use lazoo_store::{
    AcknowledgeMessageInput, CancelMessageInput, ClaimMessageInput, FailMessageInput,
    RetryMessageInput, StoreError,
};
use lazoo_store::{CreateEmployeeInput, SendMessageInput, Store};
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

fn employee(name: &str) -> CreateEmployeeInput {
    CreateEmployeeInput {
        request_id: format!("create-{name}"),
        idempotency_key: format!("create-{name}"),
        name: name.to_owned(),
        provider: "codex".to_owned(),
        role: String::new(),
        instructions: String::new(),
        model: None,
        reasoning_effort: None,
        native_settings: json!({}),
        authority_mode: AuthorityMode::Ask,
        checkout_policy: EmployeeCheckoutPolicy::Isolated,
        activation_policy: lazoo_protocol::ActivationPolicy::Manual,
        idle_timeout_seconds: 0,
        enabled: true,
        provider_observation: None,
    }
}

fn send(key: &str, sender: Uuid, recipient: Uuid, causation_id: Option<Uuid>) -> SendMessageInput {
    SendMessageInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        sender: MessageEndpoint {
            employee_id: sender,
            session_id: None,
        },
        recipient: MessageEndpoint {
            employee_id: recipient,
            session_id: None,
        },
        project_id: None,
        conversation_id: None,
        task_id: None,
        correlation_id: None,
        causation_id,
        kind: MessageKind::Peer,
        body: json!({"text": key}),
        references: Vec::new(),
        max_attempts: 3,
    }
}

#[test]
fn send_is_record_first_idempotent_and_ordered() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let sender = store.create_employee(&employee("Rhea")).unwrap().employee;
    let recipient = store.create_employee(&employee("Mira")).unwrap().employee;

    let first = store
        .send_message(&send("message-1", sender.id, recipient.id, None))
        .unwrap();
    let replay = store
        .send_message(&send("message-1", sender.id, recipient.id, None))
        .unwrap();
    let second = store
        .send_message(&send(
            "message-2",
            recipient.id,
            sender.id,
            Some(first.message.envelope.id),
        ))
        .unwrap();

    assert_eq!(first.message.envelope.id, replay.message.envelope.id);
    assert!(first.event_sequence.is_some());
    assert_eq!(replay.event_sequence, None);
    assert_eq!(first.message.sequence, 1);
    assert_eq!(second.message.sequence, 2);
    assert_eq!(
        first.message.envelope.correlation_id,
        first.message.envelope.id
    );
    assert_eq!(
        second.message.envelope.correlation_id,
        first.message.envelope.correlation_id
    );
    assert_eq!(first.message.delivery.state, MessageDeliveryState::Queued);
    assert_eq!(first.message.envelope.version, 1);
    assert_eq!(
        first.message.envelope.sender,
        MessageSender::Employee(MessageEndpoint {
            employee_id: sender.id,
            session_id: None,
        })
    );
    assert_eq!(first.message.delivery.revision, 1);
    assert_eq!(first.message.delivery.attempt_count, 0);
    assert!(first.message.attempts.is_empty());
    assert!(first.message.dead_letter.is_none());
    assert!(first.message.activation_intent.requires_activation);

    let replay_page = store.list_messages(first.message.sequence, 100).unwrap();
    assert_eq!(replay_page.len(), 1);
    assert_eq!(replay_page[0].id, second.message.envelope.id);
    assert_eq!(replay_page[0].sequence, second.message.sequence);

    let shown = store.get_message(first.message.envelope.id).unwrap();
    assert_eq!(shown.envelope.body, json!({"text": "message-1"}));
    assert_eq!(shown, replay.message);
    assert_eq!(store.status().unwrap().outbox_pending, 4);
}

fn message_pair(store: &Store) -> (Uuid, Uuid) {
    let sender = store.create_employee(&employee("Rhea")).unwrap().employee;
    let recipient = store.create_employee(&employee("Mira")).unwrap().employee;
    (sender.id, recipient.id)
}

fn claim(key: &str, message_id: Uuid, expected_revision: u64) -> ClaimMessageInput {
    ClaimMessageInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        message_id,
        expected_revision,
    }
}

#[test]
fn claim_and_acknowledge_are_attempt_correlated_and_idempotent() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let (sender, recipient) = message_pair(&store);
    let queued = store
        .send_message(&send("message-ack", sender, recipient, None))
        .unwrap();

    let delivering = store
        .claim_message(&claim("claim-ack", queued.message.envelope.id, 1))
        .unwrap();
    let claim_replay = store
        .claim_message(&claim("claim-ack", queued.message.envelope.id, 1))
        .unwrap();

    assert_eq!(
        delivering.message.delivery.state,
        MessageDeliveryState::Delivering
    );
    assert_eq!(delivering.message.delivery.revision, 2);
    assert_eq!(delivering.message.delivery.attempt_count, 1);
    assert_eq!(delivering.message.attempts.len(), 1);
    assert_eq!(claim_replay.message, delivering.message);
    assert_eq!(claim_replay.event_sequence, None);
    let attempt_id = delivering.message.attempts[0].id;

    let wrong_attempt = store
        .acknowledge_message(&AcknowledgeMessageInput {
            request_id: "wrong-ack".to_owned(),
            idempotency_key: "wrong-ack".to_owned(),
            message_id: queued.message.envelope.id,
            expected_revision: 2,
            attempt_id: Uuid::new_v4(),
            acknowledgement_key: "provider-ack-1".to_owned(),
            evidence: json!({"progress": true}),
        })
        .unwrap_err();
    assert!(matches!(wrong_attempt, StoreError::MessageAttempt { .. }));

    let ack_input = AcknowledgeMessageInput {
        request_id: "ack-request".to_owned(),
        idempotency_key: "ack-message".to_owned(),
        message_id: queued.message.envelope.id,
        expected_revision: 2,
        attempt_id,
        acknowledgement_key: "provider-ack-1".to_owned(),
        evidence: json!({"progress": true}),
    };
    let acknowledged = store.acknowledge_message(&ack_input).unwrap();
    let ack_replay = store.acknowledge_message(&ack_input).unwrap();

    assert_eq!(
        acknowledged.message.delivery.state,
        MessageDeliveryState::Acknowledged
    );
    assert_eq!(acknowledged.message.delivery.revision, 3);
    assert!(acknowledged.message.delivery.acknowledged_at.is_some());
    assert_eq!(
        acknowledged.message.attempts[0].state,
        MessageDeliveryState::Acknowledged
    );
    assert_eq!(
        acknowledged.message.attempts[0]
            .acknowledgement_key
            .as_deref(),
        Some("provider-ack-1")
    );
    assert_eq!(ack_replay.message, acknowledged.message);
    assert_eq!(ack_replay.event_sequence, None);
}

#[test]
fn retry_is_scheduled_and_exhaustion_creates_one_dead_letter() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let (sender, recipient) = message_pair(&store);
    let mut input = send("message-retry", sender, recipient, None);
    input.max_attempts = 2;
    let queued = store.send_message(&input).unwrap();
    let first_attempt = store
        .claim_message(&claim("claim-retry", queued.message.envelope.id, 1))
        .unwrap();
    let first_attempt_id = first_attempt.message.attempts[0].id;

    let first_failure_input = FailMessageInput {
        request_id: "fail-request-1".to_owned(),
        idempotency_key: "fail-retry-1".to_owned(),
        message_id: queued.message.envelope.id,
        expected_revision: 2,
        attempt_id: first_attempt_id,
        retryable: true,
        failure_code: "LZ-MESSAGE-TIMEOUT".to_owned(),
        failure_message: "recipient acknowledgement was not observed".to_owned(),
    };
    let failed = store.fail_message(&first_failure_input).unwrap();
    assert_eq!(
        failed.message.delivery.state,
        MessageDeliveryState::RetryableFailed
    );
    assert_eq!(failed.message.delivery.revision, 3);
    assert!(failed.message.delivery.next_attempt_at.is_some());
    assert!(failed.message.dead_letter.is_none());

    let retry_input = RetryMessageInput {
        request_id: "retry-request".to_owned(),
        idempotency_key: "retry-message".to_owned(),
        message_id: queued.message.envelope.id,
        expected_revision: 3,
    };
    assert!(matches!(
        store.retry_message(&retry_input).unwrap_err(),
        StoreError::MessageRetryNotDue { .. }
    ));
    store
        .execute_for_test(&format!(
            "UPDATE message_deliveries SET next_attempt_at = '2026-01-01T00:00:00Z' WHERE message_id = '{}'",
            queued.message.envelope.id
        ))
        .unwrap();

    let retrying = store.retry_message(&retry_input).unwrap();
    let retry_replay = store.retry_message(&retry_input).unwrap();
    assert_eq!(
        retrying.message.delivery.state,
        MessageDeliveryState::Delivering
    );
    assert_eq!(retrying.message.delivery.revision, 4);
    assert_eq!(retrying.message.delivery.attempt_count, 2);
    assert_eq!(retrying.message.attempts.len(), 2);
    assert_ne!(retrying.message.attempts[1].id, first_attempt_id);
    assert_eq!(retry_replay.message, retrying.message);
    let second_attempt_id = retrying.message.attempts[1].id;

    let final_failure_input = FailMessageInput {
        request_id: "fail-request-2".to_owned(),
        idempotency_key: "fail-retry-2".to_owned(),
        message_id: queued.message.envelope.id,
        expected_revision: 4,
        attempt_id: second_attempt_id,
        retryable: true,
        failure_code: "LZ-MESSAGE-TIMEOUT".to_owned(),
        failure_message: "recipient acknowledgement was not observed".to_owned(),
    };
    let exhausted = store.fail_message(&final_failure_input).unwrap();
    let exhausted_replay = store.fail_message(&final_failure_input).unwrap();
    assert_eq!(
        exhausted.message.delivery.state,
        MessageDeliveryState::TerminalFailed
    );
    assert_eq!(exhausted.message.delivery.revision, 5);
    assert_eq!(
        exhausted.message.dead_letter.as_ref().unwrap().attempt_id,
        second_attempt_id
    );
    assert_eq!(exhausted_replay.message, exhausted.message);
    assert_eq!(exhausted_replay.event_sequence, None);
}

#[test]
fn cancellation_and_revision_checks_preserve_terminal_truth() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let (sender, recipient) = message_pair(&store);
    let queued = store
        .send_message(&send("message-cancel", sender, recipient, None))
        .unwrap();

    assert!(matches!(
        store
            .claim_message(&claim("stale-claim", queued.message.envelope.id, 2))
            .unwrap_err(),
        StoreError::MessageStale {
            current_revision: 1,
            ..
        }
    ));

    let cancel_input = CancelMessageInput {
        request_id: "cancel-request".to_owned(),
        idempotency_key: "cancel-message".to_owned(),
        message_id: queued.message.envelope.id,
        expected_revision: 1,
    };
    let cancelled = store.cancel_message(&cancel_input).unwrap();
    let replay = store.cancel_message(&cancel_input).unwrap();
    assert_eq!(
        cancelled.message.delivery.state,
        MessageDeliveryState::Cancelled
    );
    assert_eq!(cancelled.message.delivery.revision, 2);
    assert_eq!(replay.message, cancelled.message);
    assert_eq!(replay.event_sequence, None);
    assert!(matches!(
        store
            .claim_message(&claim("claim-cancelled", queued.message.envelope.id, 2))
            .unwrap_err(),
        StoreError::MessageState { .. }
    ));
}

#[test]
fn restart_reconciliation_closes_uncertain_attempts_without_new_messages() {
    let temporary = tempdir().unwrap();
    let database = temporary.path().join("lazoo.db");
    let store = Store::open_path(&database).unwrap();
    let (sender, recipient) = message_pair(&store);
    let retryable = store
        .send_message(&send("message-reconcile", sender, recipient, None))
        .unwrap();
    let mut terminal_input = send("message-reconcile-terminal", sender, recipient, None);
    terminal_input.max_attempts = 1;
    let terminal = store.send_message(&terminal_input).unwrap();
    store
        .claim_message(&claim("claim-reconcile", retryable.message.envelope.id, 1))
        .unwrap();
    store
        .claim_message(&claim(
            "claim-reconcile-terminal",
            terminal.message.envelope.id,
            1,
        ))
        .unwrap();
    drop(store);

    let store = Store::open_path(&database).unwrap();
    let reconciled = store.reconcile_messages().unwrap();
    let replay = store.reconcile_messages().unwrap();

    assert_eq!(reconciled.messages.len(), 2);
    assert!(reconciled.event_sequence.is_some());
    assert!(replay.messages.is_empty());
    assert_eq!(replay.event_sequence, None);
    let retryable = store.get_message(retryable.message.envelope.id).unwrap();
    assert_eq!(
        retryable.delivery.state,
        MessageDeliveryState::RetryableFailed
    );
    assert_eq!(
        retryable.attempts[0].failure_code.as_deref(),
        Some("LZ-MESSAGE-OWNER-LOST")
    );
    assert!(retryable.dead_letter.is_none());
    let terminal = store.get_message(terminal.message.envelope.id).unwrap();
    assert_eq!(
        terminal.delivery.state,
        MessageDeliveryState::TerminalFailed
    );
    assert_eq!(
        terminal.dead_letter.as_ref().unwrap().failure_code,
        "LZ-MESSAGE-OWNER-LOST"
    );
    assert_eq!(store.list_messages(0, 100).unwrap().len(), 2);
}
