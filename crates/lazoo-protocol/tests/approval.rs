use lazoo_protocol::{
    APPROVAL_CONTRACT_V1, ApprovalDecider, ApprovalDecision, ApprovalKind, ApprovalListParams,
    ApprovalMutationResult, ApprovalRecord, ApprovalShowParams, ApprovalState, AuthorityMode,
    DecideApprovalParams,
};
use serde_json::{Value, json};
use uuid::Uuid;

fn approval_record() -> ApprovalRecord {
    ApprovalRecord {
        sequence: 3,
        version: APPROVAL_CONTRACT_V1,
        id: Uuid::from_u128(1),
        turn_id: Uuid::from_u128(2),
        task_id: Some(Uuid::from_u128(2)),
        conversation_id: Uuid::from_u128(3),
        project_id: None,
        employee_id: Uuid::from_u128(4),
        session_id: Some(Uuid::from_u128(5)),
        owner_boot_id: Uuid::from_u128(6),
        provider: "codex".to_owned(),
        native_session_id: Some("thread-1".to_owned()),
        native_turn_id: Some("turn-1".to_owned()),
        native_request_id: Some("7".to_owned()),
        kind: ApprovalKind::CommandExecution,
        action: "cargo test".to_owned(),
        detail: json!({"command": "cargo test", "cwd": "C:\\repo"}),
        fingerprint: "ab".repeat(32),
        authority: AuthorityMode::Ask,
        scope_expansion: false,
        state: ApprovalState::Pending,
        decider: None,
        revision: 1,
        failure_code: None,
        failure_message: None,
        created_at: "2026-09-22T10:00:00Z".to_owned(),
        expires_at: "2026-09-22T10:15:00Z".to_owned(),
        updated_at: "2026-09-22T10:00:00Z".to_owned(),
        decided_at: None,
    }
}

#[test]
fn approval_enums_use_stable_kebab_case_wire_values() {
    assert_eq!(APPROVAL_CONTRACT_V1, 1);
    assert_eq!(
        serde_json::to_value(ApprovalKind::CommandExecution).unwrap(),
        json!("command-execution")
    );
    assert_eq!(
        serde_json::to_value(ApprovalKind::FileChange).unwrap(),
        json!("file-change")
    );
    for (state, wire) in [
        (ApprovalState::Pending, "pending"),
        (ApprovalState::Approved, "approved"),
        (ApprovalState::Declined, "declined"),
        (ApprovalState::Expired, "expired"),
        (ApprovalState::Cancelled, "cancelled"),
    ] {
        assert_eq!(serde_json::to_value(state).unwrap(), json!(wire));
    }
    for (decider, wire) in [
        (ApprovalDecider::Policy, "policy"),
        (ApprovalDecider::User, "user"),
        (ApprovalDecider::System, "system"),
    ] {
        assert_eq!(serde_json::to_value(decider).unwrap(), json!(wire));
    }
    assert_eq!(
        serde_json::to_value(ApprovalDecision::Approve).unwrap(),
        json!("approve")
    );
    assert_eq!(
        serde_json::to_value(ApprovalDecision::Decline).unwrap(),
        json!("decline")
    );
}

#[test]
fn approval_record_round_trips_with_camel_case_keys() {
    let record = approval_record();
    let value = serde_json::to_value(&record).unwrap();
    assert_eq!(value["turnId"], json!(Uuid::from_u128(2)));
    assert_eq!(value["nativeRequestId"], json!("7"));
    assert_eq!(value["scopeExpansion"], json!(false));
    assert_eq!(value["expiresAt"], json!("2026-09-22T10:15:00Z"));
    assert_eq!(value["decider"], Value::Null);
    let decoded: ApprovalRecord = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, record);

    let result = ApprovalMutationResult {
        approval: record,
        event_sequence: Some(9),
    };
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value["eventSequence"], json!(9));
    assert_eq!(
        serde_json::from_value::<ApprovalMutationResult>(value).unwrap(),
        result
    );
}

#[test]
fn approval_params_have_defaults_and_reject_unknown_fields() {
    let params: ApprovalListParams = serde_json::from_value(json!({})).unwrap();
    assert_eq!(params, ApprovalListParams::default());
    assert_eq!(params.limit, 100);
    assert_eq!(params.after_sequence, 0);
    let params: ApprovalListParams =
        serde_json::from_value(json!({"state": "pending", "limit": 5})).unwrap();
    assert_eq!(params.state, Some(ApprovalState::Pending));
    assert_eq!(params.limit, 5);

    let decide: DecideApprovalParams = serde_json::from_value(json!({
        "id": Uuid::from_u128(1),
        "expectedRevision": 1,
        "decision": "approve",
        "idempotencyKey": "decide-1"
    }))
    .unwrap();
    assert_eq!(decide.decision, ApprovalDecision::Approve);
    assert!(
        serde_json::from_value::<DecideApprovalParams>(json!({
            "id": Uuid::from_u128(1),
            "expectedRevision": 1,
            "decision": "approve",
            "idempotencyKey": "decide-1",
            "grantForSession": true
        }))
        .is_err()
    );
    assert!(serde_json::from_value::<ApprovalShowParams>(json!({"id": "nope"})).is_err());
}
