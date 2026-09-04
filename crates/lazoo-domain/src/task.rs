use lazoo_protocol::{AuthorityMode, MessageReference, TaskState};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::{
    ConversationValidationError, MessageValidationError, validate_message_content,
    validate_message_failure, validate_turn_text,
};

pub const MAX_TASK_DEPTH: u32 = 4;
pub const MAX_TASK_EVIDENCE_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum TaskValidationError {
    #[error("task objective must not be empty")]
    ObjectiveEmpty,
    #[error("task objective must contain at most 262144 UTF-8 bytes")]
    ObjectiveTooLarge,
    #[error("task reference is invalid: {0}")]
    Reference(MessageValidationError),
    #[error("task evidence must be a JSON object")]
    EvidenceNotObject,
    #[error("task evidence must contain at most 16384 JSON bytes")]
    EvidenceTooLarge,
    #[error("task failure code must not be empty")]
    FailureCodeEmpty,
    #[error("task failure code must contain at most 128 UTF-8 bytes")]
    FailureCodeTooLong,
    #[error("task failure message must not be empty")]
    FailureMessageEmpty,
    #[error("task failure message must contain at most 1024 UTF-8 bytes")]
    FailureMessageTooLong,
    #[error("task depth must be between 0 and 4")]
    DepthOutOfRange,
}

pub fn validate_task_objective(value: &str) -> Result<String, TaskValidationError> {
    validate_turn_text(value).map_err(|error| match error {
        ConversationValidationError::TurnTextEmpty => TaskValidationError::ObjectiveEmpty,
        ConversationValidationError::TurnTextTooLarge => TaskValidationError::ObjectiveTooLarge,
        _ => unreachable!("turn text validation returned an unrelated error"),
    })?;
    Ok(value.trim().to_owned())
}

pub fn validate_task_references(value: &[MessageReference]) -> Result<(), TaskValidationError> {
    validate_message_content(&Value::Object(Map::new()), value)
        .map_err(TaskValidationError::Reference)
}

pub fn validate_task_evidence(value: &Value) -> Result<(), TaskValidationError> {
    if !value.is_object() {
        Err(TaskValidationError::EvidenceNotObject)
    } else if serialized_len(value) > MAX_TASK_EVIDENCE_BYTES {
        Err(TaskValidationError::EvidenceTooLarge)
    } else {
        Ok(())
    }
}

pub fn validate_task_failure(code: &str, message: &str) -> Result<(), TaskValidationError> {
    validate_message_failure(code, message).map_err(|error| match error {
        MessageValidationError::FailureCodeEmpty => TaskValidationError::FailureCodeEmpty,
        MessageValidationError::FailureCodeTooLong => TaskValidationError::FailureCodeTooLong,
        MessageValidationError::FailureMessageEmpty => TaskValidationError::FailureMessageEmpty,
        MessageValidationError::FailureMessageTooLong => TaskValidationError::FailureMessageTooLong,
        _ => unreachable!("message failure validation returned an unrelated error"),
    })
}

pub const fn validate_task_depth(depth: u32) -> Result<(), TaskValidationError> {
    if depth <= MAX_TASK_DEPTH {
        Ok(())
    } else {
        Err(TaskValidationError::DepthOutOfRange)
    }
}

#[must_use]
pub const fn effective_authority(
    parent: AuthorityMode,
    requested: AuthorityMode,
    assignee: AuthorityMode,
) -> AuthorityMode {
    let parent_rank = authority_rank(parent);
    let requested_rank = authority_rank(requested);
    let assignee_rank = authority_rank(assignee);
    let minimum = if parent_rank < requested_rank {
        parent_rank
    } else {
        requested_rank
    };
    let minimum = if minimum < assignee_rank {
        minimum
    } else {
        assignee_rank
    };
    authority_from_rank(minimum)
}

#[must_use]
pub const fn task_transition_allowed(from: TaskState, to: TaskState) -> bool {
    matches!(
        (from, to),
        (
            TaskState::Proposed,
            TaskState::Accepted | TaskState::Cancelled
        ) | (
            TaskState::Accepted,
            TaskState::Running | TaskState::Cancelled | TaskState::Failed
        ) | (
            TaskState::Running,
            TaskState::Completed
                | TaskState::Failed
                | TaskState::CancelRequested
                | TaskState::Uncertain
        ) | (
            TaskState::CancelRequested,
            TaskState::Cancelled | TaskState::Failed | TaskState::Uncertain
        ) | (
            TaskState::Uncertain,
            TaskState::Completed | TaskState::Failed | TaskState::Cancelled
        )
    )
}

const fn authority_rank(value: AuthorityMode) -> u8 {
    match value {
        AuthorityMode::Ask => 0,
        AuthorityMode::SelfDecision => 1,
        AuthorityMode::FullAutonomy => 2,
    }
}

const fn authority_from_rank(value: u8) -> AuthorityMode {
    match value {
        0 => AuthorityMode::Ask,
        1 => AuthorityMode::SelfDecision,
        _ => AuthorityMode::FullAutonomy,
    }
}

fn serialized_len(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |serialized| serialized.len())
}
