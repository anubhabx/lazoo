use lazoo_domain::{
    MessageValidationError, retry_delay_seconds, validate_acknowledgement, validate_max_attempts,
    validate_message_command_key, validate_message_content, validate_message_failure,
};
use lazoo_protocol::MessageReference;
use serde_json::json;

#[test]
fn message_body_is_non_null_and_bounded() {
    assert_eq!(
        validate_message_content(&serde_json::Value::Null, &[]),
        Err(MessageValidationError::BodyNull)
    );
    assert_eq!(
        validate_message_content(&json!({"text": "x".repeat(256 * 1024)}), &[]),
        Err(MessageValidationError::BodyTooLarge)
    );
    assert!(validate_message_content(&json!({"text": "hello"}), &[]).is_ok());
}

#[test]
fn message_references_are_structured_and_bounded() {
    let too_many = (0..33)
        .map(|index| MessageReference {
            kind: "file".to_owned(),
            target: format!("file-{index}"),
            metadata: json!({}),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        validate_message_content(&json!({"text": "hello"}), &too_many),
        Err(MessageValidationError::TooManyReferences)
    );

    let invalid_metadata = [MessageReference {
        kind: "file".to_owned(),
        target: "README.md".to_owned(),
        metadata: json!(["not", "an", "object"]),
    }];
    assert_eq!(
        validate_message_content(&json!({"text": "hello"}), &invalid_metadata),
        Err(MessageValidationError::ReferenceMetadataNotObject)
    );

    let invalid_kind = [MessageReference {
        kind: " ".to_owned(),
        target: "README.md".to_owned(),
        metadata: json!({}),
    }];
    assert_eq!(
        validate_message_content(&json!({"text": "hello"}), &invalid_kind),
        Err(MessageValidationError::ReferenceKindEmpty)
    );

    let invalid_target = [MessageReference {
        kind: "file".to_owned(),
        target: "x".repeat(2049),
        metadata: json!({}),
    }];
    assert_eq!(
        validate_message_content(&json!({"text": "hello"}), &invalid_target),
        Err(MessageValidationError::ReferenceTargetTooLong)
    );
}

#[test]
fn command_acknowledgement_and_failure_fields_are_bounded() {
    assert_eq!(
        validate_message_command_key(" "),
        Err(MessageValidationError::CommandKeyEmpty)
    );
    assert_eq!(
        validate_message_command_key(&"x".repeat(257)),
        Err(MessageValidationError::CommandKeyTooLong)
    );
    assert_eq!(
        validate_acknowledgement("ack-1", &json!([])),
        Err(MessageValidationError::AcknowledgementEvidenceNotObject)
    );
    assert_eq!(
        validate_acknowledgement("", &json!({})),
        Err(MessageValidationError::AcknowledgementKeyEmpty)
    );
    assert_eq!(
        validate_message_failure("", "safe message"),
        Err(MessageValidationError::FailureCodeEmpty)
    );
    assert_eq!(
        validate_message_failure("LZ-TEST", &"x".repeat(1025)),
        Err(MessageValidationError::FailureMessageTooLong)
    );
    assert!(validate_message_command_key("send-1").is_ok());
    assert!(validate_acknowledgement("ack-1", &json!({"provider": "codex"})).is_ok());
    assert!(validate_message_failure("LZ-TEST", "safe message").is_ok());
}

#[test]
fn retry_budget_and_schedule_are_fixed_and_bounded() {
    assert_eq!(
        validate_max_attempts(0),
        Err(MessageValidationError::MaxAttemptsOutOfRange)
    );
    assert_eq!(
        validate_max_attempts(6),
        Err(MessageValidationError::MaxAttemptsOutOfRange)
    );
    assert!(validate_max_attempts(1).is_ok());
    assert!(validate_max_attempts(5).is_ok());
    assert_eq!(retry_delay_seconds(1), Some(1));
    assert_eq!(retry_delay_seconds(2), Some(5));
    assert_eq!(retry_delay_seconds(3), Some(30));
    assert_eq!(retry_delay_seconds(4), Some(120));
    assert_eq!(retry_delay_seconds(5), None);
}
