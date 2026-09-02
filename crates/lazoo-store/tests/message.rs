use lazoo_protocol::{
    AuthorityMode, EmployeeCheckoutPolicy, MessageDeliveryState, MessageEndpoint, MessageKind,
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
