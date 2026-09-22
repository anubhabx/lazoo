use lazoo_domain::{
    ApprovalPolicyOutcome, ApprovalValidationError, MAX_APPROVAL_ACTION_BYTES,
    approval_fingerprint, approval_policy, validate_approval_action, validate_approval_detail,
    validate_approval_list_limit,
};
use lazoo_protocol::{ApprovalKind, AuthorityMode};
use serde_json::json;

#[test]
fn ask_always_waits_for_the_user() {
    for scope_expansion in [false, true] {
        assert_eq!(
            approval_policy(AuthorityMode::Ask, scope_expansion),
            ApprovalPolicyOutcome::RequireUser
        );
    }
}

#[test]
fn self_decision_acts_inside_scope_but_waits_on_scope_expansion() {
    assert_eq!(
        approval_policy(AuthorityMode::SelfDecision, false),
        ApprovalPolicyOutcome::AutoApprove
    );
    assert_eq!(
        approval_policy(AuthorityMode::SelfDecision, true),
        ApprovalPolicyOutcome::RequireUser
    );
}

#[test]
fn full_autonomy_proceeds_without_a_user_decision() {
    for scope_expansion in [false, true] {
        assert_eq!(
            approval_policy(AuthorityMode::FullAutonomy, scope_expansion),
            ApprovalPolicyOutcome::AutoApprove
        );
    }
}

#[test]
fn fingerprint_is_stable_and_binds_kind_action_and_scope() {
    let base = approval_fingerprint(ApprovalKind::CommandExecution, "cargo test", false);
    assert_eq!(base.len(), 64);
    assert_eq!(
        base,
        approval_fingerprint(ApprovalKind::CommandExecution, "cargo test", false)
    );
    assert_ne!(
        base,
        approval_fingerprint(ApprovalKind::CommandExecution, "cargo test", true)
    );
    assert_ne!(
        base,
        approval_fingerprint(ApprovalKind::FileChange, "cargo test", false)
    );
    assert_ne!(
        base,
        approval_fingerprint(ApprovalKind::CommandExecution, "cargo build", false)
    );
}

#[test]
fn action_and_detail_are_bounded() {
    assert_eq!(
        validate_approval_action("  cargo test  ").unwrap(),
        "cargo test"
    );
    assert_eq!(
        validate_approval_action("   "),
        Err(ApprovalValidationError::ActionEmpty)
    );
    assert_eq!(
        validate_approval_action(&"x".repeat(MAX_APPROVAL_ACTION_BYTES + 1)),
        Err(ApprovalValidationError::ActionTooLong)
    );
    assert_eq!(
        validate_approval_detail(&json!({"cwd": "C:\\repo"})),
        Ok(())
    );
    assert_eq!(
        validate_approval_detail(&json!(["not", "an", "object"])),
        Err(ApprovalValidationError::DetailNotObject)
    );
    assert_eq!(
        validate_approval_detail(&json!({"command": "y".repeat(17 * 1024)})),
        Err(ApprovalValidationError::DetailTooLarge)
    );
    assert_eq!(
        validate_approval_list_limit(0),
        Err(ApprovalValidationError::ListLimit)
    );
    assert_eq!(
        validate_approval_list_limit(1025),
        Err(ApprovalValidationError::ListLimit)
    );
    assert_eq!(validate_approval_list_limit(1024), Ok(()));
}
