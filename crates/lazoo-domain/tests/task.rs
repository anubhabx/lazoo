use lazoo_domain::{
    MAX_TASK_DEPTH, MAX_TASK_EVIDENCE_BYTES, TaskValidationError, effective_authority,
    task_transition_allowed, validate_task_depth, validate_task_evidence, validate_task_failure,
    validate_task_objective, validate_task_references,
};
use lazoo_protocol::{AuthorityMode, MessageReference, TaskState};
use serde_json::json;

#[test]
fn task_objectives_share_turn_text_limits_and_are_trimmed() {
    assert_eq!(
        validate_task_objective("  investigate the flaky gate  ").unwrap(),
        "investigate the flaky gate"
    );
    assert_eq!(
        validate_task_objective(" \r\n ").unwrap_err(),
        TaskValidationError::ObjectiveEmpty
    );
    assert_eq!(
        validate_task_objective(&"x".repeat(256 * 1024 + 1)).unwrap_err(),
        TaskValidationError::ObjectiveTooLarge
    );
}

#[test]
fn task_references_reuse_message_reference_limits() {
    let valid = MessageReference {
        kind: "file".to_owned(),
        target: "crates/lazoo-domain/src/task.rs".to_owned(),
        metadata: json!({ "line": 1 }),
    };
    validate_task_references(std::slice::from_ref(&valid)).unwrap();

    let too_many = vec![valid; 33];
    assert!(matches!(
        validate_task_references(&too_many),
        Err(TaskValidationError::Reference(_))
    ));
}

#[test]
fn evidence_must_be_a_bounded_json_object() {
    assert!(validate_task_evidence(&json!({ "gate": "cargo test" })).is_ok());
    assert_eq!(
        validate_task_evidence(&json!(["not", "an", "object"])).unwrap_err(),
        TaskValidationError::EvidenceNotObject
    );

    let oversized = json!({ "text": "x".repeat(MAX_TASK_EVIDENCE_BYTES) });
    assert_eq!(
        validate_task_evidence(&oversized).unwrap_err(),
        TaskValidationError::EvidenceTooLarge
    );
}

#[test]
fn failure_details_share_message_failure_limits() {
    assert!(validate_task_failure("LZ-WORKER-FAILED", "process exited").is_ok());
    assert_eq!(
        validate_task_failure(" ", "message").unwrap_err(),
        TaskValidationError::FailureCodeEmpty
    );
    assert_eq!(
        validate_task_failure(&"x".repeat(129), "message").unwrap_err(),
        TaskValidationError::FailureCodeTooLong
    );
    assert_eq!(
        validate_task_failure("code", &"x".repeat(1025)).unwrap_err(),
        TaskValidationError::FailureMessageTooLong
    );
}

#[test]
fn task_depth_is_limited_to_four_delegation_edges() {
    assert_eq!(MAX_TASK_DEPTH, 4);
    assert!(validate_task_depth(0).is_ok());
    assert!(validate_task_depth(MAX_TASK_DEPTH).is_ok());
    assert_eq!(
        validate_task_depth(MAX_TASK_DEPTH + 1).unwrap_err(),
        TaskValidationError::DepthOutOfRange
    );
}

#[test]
fn effective_authority_is_the_most_restrictive_ceiling() {
    assert_eq!(
        effective_authority(
            AuthorityMode::FullAutonomy,
            AuthorityMode::SelfDecision,
            AuthorityMode::FullAutonomy,
        ),
        AuthorityMode::SelfDecision
    );
    assert_eq!(
        effective_authority(
            AuthorityMode::FullAutonomy,
            AuthorityMode::FullAutonomy,
            AuthorityMode::Ask,
        ),
        AuthorityMode::Ask
    );
    assert_eq!(
        effective_authority(
            AuthorityMode::SelfDecision,
            AuthorityMode::FullAutonomy,
            AuthorityMode::FullAutonomy,
        ),
        AuthorityMode::SelfDecision
    );
}

#[test]
fn task_state_machine_allows_only_the_approved_transitions() {
    let allowed = [
        (TaskState::Proposed, TaskState::Accepted),
        (TaskState::Proposed, TaskState::Cancelled),
        (TaskState::Accepted, TaskState::Running),
        (TaskState::Accepted, TaskState::Cancelled),
        (TaskState::Accepted, TaskState::Failed),
        (TaskState::Running, TaskState::Completed),
        (TaskState::Running, TaskState::Failed),
        (TaskState::Running, TaskState::CancelRequested),
        (TaskState::Running, TaskState::Uncertain),
        (TaskState::CancelRequested, TaskState::Cancelled),
        (TaskState::CancelRequested, TaskState::Failed),
        (TaskState::CancelRequested, TaskState::Uncertain),
        (TaskState::Uncertain, TaskState::Completed),
        (TaskState::Uncertain, TaskState::Failed),
        (TaskState::Uncertain, TaskState::Cancelled),
    ];
    let all = [
        TaskState::Proposed,
        TaskState::Accepted,
        TaskState::Running,
        TaskState::CancelRequested,
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Cancelled,
        TaskState::Uncertain,
    ];

    for from in all {
        for to in all {
            assert_eq!(
                task_transition_allowed(from, to),
                allowed.contains(&(from, to)),
                "unexpected transition decision for {from:?} -> {to:?}"
            );
        }
    }
}
