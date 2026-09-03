use lazoo_domain::{
    ConversationValidationError, validate_activation_request, validate_conversation_command_key,
    validate_conversation_list_limit, validate_conversation_title, validate_envelope_sender,
    validate_turn_event_payload, validate_turn_text, validate_turn_transcript_size,
    validate_turn_transition,
};
use lazoo_protocol::{
    MessageEndpoint, MessageSender, TurnState, UserMessageEndpoint, UserMessageKind,
};
use serde_json::json;
use uuid::Uuid;

fn employee_sender() -> MessageSender {
    MessageSender::Employee(MessageEndpoint {
        employee_id: Uuid::new_v4(),
        session_id: None,
    })
}

fn user_sender() -> MessageSender {
    MessageSender::User(UserMessageEndpoint {
        kind: UserMessageKind::User,
    })
}

#[test]
fn conversation_title_and_command_key_are_normalized_and_bounded() {
    assert_eq!(
        validate_conversation_title(Some("  First task  ")).unwrap(),
        Some("First task".to_owned())
    );
    assert_eq!(validate_conversation_title(Some("   ")).unwrap(), None);
    assert_eq!(validate_conversation_title(None).unwrap(), None);
    assert_eq!(
        validate_conversation_title(Some(&"x".repeat(201))),
        Err(ConversationValidationError::TitleTooLong)
    );
    assert_eq!(
        validate_conversation_command_key(" "),
        Err(ConversationValidationError::CommandKeyEmpty)
    );
    assert_eq!(
        validate_conversation_command_key(&"x".repeat(201)),
        Err(ConversationValidationError::CommandKeyTooLong)
    );
    assert!(validate_conversation_command_key("conversation-1").is_ok());
}

#[test]
fn turn_text_event_payload_and_transcript_are_bounded() {
    assert_eq!(
        validate_turn_text("   "),
        Err(ConversationValidationError::TurnTextEmpty)
    );
    assert_eq!(
        validate_turn_text(&"x".repeat(256 * 1024 + 1)),
        Err(ConversationValidationError::TurnTextTooLarge)
    );
    assert!(validate_turn_text("hello").is_ok());

    assert_eq!(
        validate_turn_event_payload(&json!(["not", "an", "object"])),
        Err(ConversationValidationError::EventPayloadNotObject)
    );
    assert_eq!(
        validate_turn_event_payload(&json!({"text": "x".repeat(64 * 1024)})),
        Err(ConversationValidationError::EventPayloadTooLarge)
    );
    assert!(validate_turn_event_payload(&json!({"status": "started"})).is_ok());

    assert!(validate_turn_transcript_size(512 * 1024 - 1, 1).is_ok());
    assert_eq!(
        validate_turn_transcript_size(512 * 1024, 1),
        Err(ConversationValidationError::TranscriptTooLarge)
    );
}

#[test]
fn actor_version_and_activation_rules_are_truthful() {
    let employee = employee_sender();
    let user = user_sender();
    assert!(validate_envelope_sender(1, &employee).is_ok());
    assert_eq!(
        validate_envelope_sender(1, &user),
        Err(ConversationValidationError::EnvelopeSenderVersion)
    );
    assert!(validate_envelope_sender(2, &employee).is_ok());
    assert!(validate_envelope_sender(2, &user).is_ok());
    assert!(validate_activation_request(&user, true).is_ok());
    assert_eq!(
        validate_activation_request(&employee, true),
        Err(ConversationValidationError::EmployeeActivationNotAllowed)
    );
    assert!(validate_activation_request(&employee, false).is_ok());
}

#[test]
fn list_limits_and_turn_transitions_are_explicit() {
    assert_eq!(
        validate_conversation_list_limit(0),
        Err(ConversationValidationError::ListLimitOutOfRange)
    );
    assert_eq!(
        validate_conversation_list_limit(1025),
        Err(ConversationValidationError::ListLimitOutOfRange)
    );
    assert!(validate_conversation_list_limit(1).is_ok());
    assert!(validate_conversation_list_limit(1024).is_ok());

    let allowed = [
        (TurnState::Recorded, TurnState::Submitting),
        (TurnState::Recorded, TurnState::Cancelled),
        (TurnState::Submitting, TurnState::Accepted),
        (TurnState::Submitting, TurnState::RetryableFailed),
        (TurnState::Submitting, TurnState::TerminalFailed),
        (TurnState::Submitting, TurnState::Cancelled),
        (TurnState::Submitting, TurnState::Uncertain),
        (TurnState::Accepted, TurnState::Completed),
        (TurnState::Accepted, TurnState::TerminalFailed),
        (TurnState::Accepted, TurnState::Cancelled),
        (TurnState::Accepted, TurnState::Uncertain),
        (TurnState::RetryableFailed, TurnState::Recorded),
        (TurnState::RetryableFailed, TurnState::Cancelled),
    ];
    for (from, to) in allowed {
        assert!(
            validate_turn_transition(from, to).is_ok(),
            "{from:?} -> {to:?}"
        );
    }

    for terminal in [
        TurnState::Completed,
        TurnState::TerminalFailed,
        TurnState::Cancelled,
        TurnState::Uncertain,
    ] {
        assert_eq!(
            validate_turn_transition(terminal, TurnState::Recorded),
            Err(ConversationValidationError::InvalidTurnTransition {
                from: terminal,
                to: TurnState::Recorded,
            })
        );
    }
    assert_eq!(
        validate_turn_transition(TurnState::Recorded, TurnState::Completed),
        Err(ConversationValidationError::InvalidTurnTransition {
            from: TurnState::Recorded,
            to: TurnState::Completed,
        })
    );
}
