use lazoo_protocol::{
    AuthorityMode, CheckoutMode, EmployeeCheckoutPolicy, MessageDeliveryState, MessageEndpoint,
    MessageSender, ProviderObservation, SessionLifecycleState, TurnState, UserMessageEndpoint,
    UserMessageKind,
};
use lazoo_store::{
    BeginSessionInput, BeginTurnSubmissionInput, CreateConversationInput, CreateEmployeeInput,
    FailTurnInput, FinishTurnCancelInput, MarkSessionReadyInput, MarkTurnUncertainInput,
    ObserveTurnInput, RecordTurnInput, RegisterProjectInput, RequestTurnCancelInput,
    RetryTurnInput, Store, StoreError,
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
            text: text.to_owned(),
            activate_if_needed: false,
            max_attempts: 3,
        })
        .unwrap()
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
    let employee_turn = record_turn(
        &store,
        "turn-employee",
        conversation.id,
        employee_author,
        None,
        "follow-up",
    );

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
