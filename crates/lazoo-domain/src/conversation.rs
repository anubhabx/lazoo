use lazoo_protocol::{MessageSender, TurnState};
use serde_json::Value;
use thiserror::Error;

pub const MAX_CONVERSATION_TITLE_BYTES: usize = 200;
pub const MAX_TURN_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_CONVERSATION_COMMAND_KEY_BYTES: usize = 200;
pub const MAX_TURN_EVENT_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_TURN_TRANSCRIPT_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ConversationValidationError {
    #[error("conversation title must contain at most 200 UTF-8 bytes")]
    TitleTooLong,
    #[error("turn text must not be empty")]
    TurnTextEmpty,
    #[error("turn text must contain at most 262144 UTF-8 bytes")]
    TurnTextTooLarge,
    #[error("conversation idempotency key must not be empty")]
    CommandKeyEmpty,
    #[error("conversation idempotency key must contain at most 200 UTF-8 bytes")]
    CommandKeyTooLong,
    #[error("turn event payload must be a JSON object")]
    EventPayloadNotObject,
    #[error("turn event payload must contain at most 65536 JSON bytes")]
    EventPayloadTooLarge,
    #[error("turn transcript must contain at most 524288 UTF-8 bytes")]
    TranscriptTooLarge,
    #[error("message sender is not valid for the envelope version")]
    EnvelopeSenderVersion,
    #[error("message envelope version is not supported")]
    EnvelopeVersionUnsupported,
    #[error("employee-authored turns cannot activate a dormant recipient")]
    EmployeeActivationNotAllowed,
    #[error("conversation list limit must be between 1 and 1024")]
    ListLimitOutOfRange,
    #[error("turn state transition from {from:?} to {to:?} is invalid")]
    InvalidTurnTransition { from: TurnState, to: TurnState },
}

pub fn validate_conversation_title(
    value: Option<&str>,
) -> Result<Option<String>, ConversationValidationError> {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    match value {
        Some(value) if value.len() > MAX_CONVERSATION_TITLE_BYTES => {
            Err(ConversationValidationError::TitleTooLong)
        }
        Some(value) => Ok(Some(value.to_owned())),
        None => Ok(None),
    }
}

pub fn validate_turn_text(value: &str) -> Result<(), ConversationValidationError> {
    if value.trim().is_empty() {
        Err(ConversationValidationError::TurnTextEmpty)
    } else if value.len() > MAX_TURN_TEXT_BYTES {
        Err(ConversationValidationError::TurnTextTooLarge)
    } else {
        Ok(())
    }
}

pub fn validate_conversation_command_key(value: &str) -> Result<(), ConversationValidationError> {
    if value.trim().is_empty() {
        Err(ConversationValidationError::CommandKeyEmpty)
    } else if value.len() > MAX_CONVERSATION_COMMAND_KEY_BYTES {
        Err(ConversationValidationError::CommandKeyTooLong)
    } else {
        Ok(())
    }
}

pub fn validate_turn_event_payload(value: &Value) -> Result<(), ConversationValidationError> {
    if !value.is_object() {
        Err(ConversationValidationError::EventPayloadNotObject)
    } else if serialized_len(value) > MAX_TURN_EVENT_PAYLOAD_BYTES {
        Err(ConversationValidationError::EventPayloadTooLarge)
    } else {
        Ok(())
    }
}

pub const fn validate_turn_transcript_size(
    current_bytes: usize,
    additional_bytes: usize,
) -> Result<(), ConversationValidationError> {
    if current_bytes.saturating_add(additional_bytes) > MAX_TURN_TRANSCRIPT_BYTES {
        Err(ConversationValidationError::TranscriptTooLarge)
    } else {
        Ok(())
    }
}

pub const fn validate_envelope_sender(
    version: u32,
    sender: &MessageSender,
) -> Result<(), ConversationValidationError> {
    match (version, sender) {
        (1, MessageSender::Employee(_))
        | (2, MessageSender::Employee(_) | MessageSender::User(_)) => Ok(()),
        (1, MessageSender::User(_)) => Err(ConversationValidationError::EnvelopeSenderVersion),
        _ => Err(ConversationValidationError::EnvelopeVersionUnsupported),
    }
}

pub const fn validate_activation_request(
    author: &MessageSender,
    activate_if_needed: bool,
) -> Result<(), ConversationValidationError> {
    if activate_if_needed && matches!(author, MessageSender::Employee(_)) {
        Err(ConversationValidationError::EmployeeActivationNotAllowed)
    } else {
        Ok(())
    }
}

pub const fn validate_conversation_list_limit(
    limit: u32,
) -> Result<(), ConversationValidationError> {
    if limit >= 1 && limit <= 1024 {
        Ok(())
    } else {
        Err(ConversationValidationError::ListLimitOutOfRange)
    }
}

pub const fn validate_turn_transition(
    from: TurnState,
    to: TurnState,
) -> Result<(), ConversationValidationError> {
    let allowed = matches!(
        (from, to),
        (
            TurnState::Recorded,
            TurnState::Submitting | TurnState::Cancelled
        ) | (
            TurnState::Submitting,
            TurnState::Accepted
                | TurnState::RetryableFailed
                | TurnState::TerminalFailed
                | TurnState::Cancelled
                | TurnState::Uncertain
        ) | (
            TurnState::Accepted,
            TurnState::Completed
                | TurnState::TerminalFailed
                | TurnState::Cancelled
                | TurnState::Uncertain
        ) | (
            TurnState::RetryableFailed,
            TurnState::Recorded | TurnState::Cancelled
        )
    );
    if allowed {
        Ok(())
    } else {
        Err(ConversationValidationError::InvalidTurnTransition { from, to })
    }
}

fn serialized_len(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |serialized| serialized.len())
}
