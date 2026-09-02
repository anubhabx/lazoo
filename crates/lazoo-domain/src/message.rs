use lazoo_protocol::MessageReference;
use serde_json::Value;
use thiserror::Error;

const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_REFERENCES: usize = 32;
const MAX_REFERENCE_KIND_CHARS: usize = 64;
const MAX_REFERENCE_TARGET_BYTES: usize = 2_048;
const MAX_REFERENCE_METADATA_BYTES: usize = 16 * 1024;
const MAX_COMMAND_KEY_BYTES: usize = 256;
const MAX_ACKNOWLEDGEMENT_KEY_BYTES: usize = 256;
const MAX_ACKNOWLEDGEMENT_EVIDENCE_BYTES: usize = 16 * 1024;
const MAX_FAILURE_CODE_BYTES: usize = 128;
const MAX_FAILURE_MESSAGE_BYTES: usize = 1_024;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum MessageValidationError {
    #[error("message body must not be null")]
    BodyNull,
    #[error("message body must contain at most 262144 JSON bytes")]
    BodyTooLarge,
    #[error("message must contain at most 32 references")]
    TooManyReferences,
    #[error("message reference kind must not be empty")]
    ReferenceKindEmpty,
    #[error("message reference kind must contain at most 64 characters")]
    ReferenceKindTooLong,
    #[error("message reference target must not be empty")]
    ReferenceTargetEmpty,
    #[error("message reference target must contain at most 2048 UTF-8 bytes")]
    ReferenceTargetTooLong,
    #[error("message reference metadata must be a JSON object")]
    ReferenceMetadataNotObject,
    #[error("message reference metadata must contain at most 16384 JSON bytes")]
    ReferenceMetadataTooLarge,
    #[error("message idempotency key must not be empty")]
    CommandKeyEmpty,
    #[error("message idempotency key must contain at most 256 UTF-8 bytes")]
    CommandKeyTooLong,
    #[error("message acknowledgement key must not be empty")]
    AcknowledgementKeyEmpty,
    #[error("message acknowledgement key must contain at most 256 UTF-8 bytes")]
    AcknowledgementKeyTooLong,
    #[error("message acknowledgement evidence must be a JSON object")]
    AcknowledgementEvidenceNotObject,
    #[error("message acknowledgement evidence must contain at most 16384 JSON bytes")]
    AcknowledgementEvidenceTooLarge,
    #[error("message failure code must not be empty")]
    FailureCodeEmpty,
    #[error("message failure code must contain at most 128 UTF-8 bytes")]
    FailureCodeTooLong,
    #[error("message failure message must not be empty")]
    FailureMessageEmpty,
    #[error("message failure message must contain at most 1024 UTF-8 bytes")]
    FailureMessageTooLong,
    #[error("message max attempts must be between 1 and 5")]
    MaxAttemptsOutOfRange,
}

pub fn validate_message_content(
    body: &Value,
    references: &[MessageReference],
) -> Result<(), MessageValidationError> {
    if body.is_null() {
        return Err(MessageValidationError::BodyNull);
    }
    if serialized_len(body) > MAX_BODY_BYTES {
        return Err(MessageValidationError::BodyTooLarge);
    }
    if references.len() > MAX_REFERENCES {
        return Err(MessageValidationError::TooManyReferences);
    }
    for reference in references {
        if reference.kind.trim().is_empty() {
            return Err(MessageValidationError::ReferenceKindEmpty);
        }
        if reference.kind.chars().count() > MAX_REFERENCE_KIND_CHARS {
            return Err(MessageValidationError::ReferenceKindTooLong);
        }
        if reference.target.trim().is_empty() {
            return Err(MessageValidationError::ReferenceTargetEmpty);
        }
        if reference.target.len() > MAX_REFERENCE_TARGET_BYTES {
            return Err(MessageValidationError::ReferenceTargetTooLong);
        }
        if !reference.metadata.is_object() {
            return Err(MessageValidationError::ReferenceMetadataNotObject);
        }
        if serialized_len(&reference.metadata) > MAX_REFERENCE_METADATA_BYTES {
            return Err(MessageValidationError::ReferenceMetadataTooLarge);
        }
    }
    Ok(())
}

pub fn validate_message_command_key(value: &str) -> Result<(), MessageValidationError> {
    if value.trim().is_empty() {
        Err(MessageValidationError::CommandKeyEmpty)
    } else if value.len() > MAX_COMMAND_KEY_BYTES {
        Err(MessageValidationError::CommandKeyTooLong)
    } else {
        Ok(())
    }
}

pub fn validate_acknowledgement(key: &str, evidence: &Value) -> Result<(), MessageValidationError> {
    if key.trim().is_empty() {
        return Err(MessageValidationError::AcknowledgementKeyEmpty);
    }
    if key.len() > MAX_ACKNOWLEDGEMENT_KEY_BYTES {
        return Err(MessageValidationError::AcknowledgementKeyTooLong);
    }
    if !evidence.is_object() {
        return Err(MessageValidationError::AcknowledgementEvidenceNotObject);
    }
    if serialized_len(evidence) > MAX_ACKNOWLEDGEMENT_EVIDENCE_BYTES {
        return Err(MessageValidationError::AcknowledgementEvidenceTooLarge);
    }
    Ok(())
}

pub fn validate_message_failure(code: &str, message: &str) -> Result<(), MessageValidationError> {
    if code.trim().is_empty() {
        return Err(MessageValidationError::FailureCodeEmpty);
    }
    if code.len() > MAX_FAILURE_CODE_BYTES {
        return Err(MessageValidationError::FailureCodeTooLong);
    }
    if message.trim().is_empty() {
        return Err(MessageValidationError::FailureMessageEmpty);
    }
    if message.len() > MAX_FAILURE_MESSAGE_BYTES {
        return Err(MessageValidationError::FailureMessageTooLong);
    }
    Ok(())
}

pub const fn validate_max_attempts(value: u32) -> Result<(), MessageValidationError> {
    if value >= 1 && value <= 5 {
        Ok(())
    } else {
        Err(MessageValidationError::MaxAttemptsOutOfRange)
    }
}

#[must_use]
pub const fn retry_delay_seconds(attempt_count: u32) -> Option<i64> {
    match attempt_count {
        1 => Some(1),
        2 => Some(5),
        3 => Some(30),
        4 => Some(120),
        _ => None,
    }
}

fn serialized_len(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |serialized| serialized.len())
}
