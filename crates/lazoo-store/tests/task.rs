use lazoo_protocol::{
    AuthorityMode, EmployeeCheckoutPolicy, MessageEndpoint, MessageKind, MessageSender,
    ProviderObservation, TaskEvidenceKind, TaskState, UserMessageEndpoint, UserMessageKind,
};
use lazoo_store::{
    CreateConversationInput, CreateEmployeeInput, ListTasksInput, RecordTurnInput,
    SendMessageInput, Store, StoreError,
};
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

fn employee(name: &str) -> CreateEmployeeInput {
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
        enabled: true,
        provider_observation: Some(ProviderObservation {
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
