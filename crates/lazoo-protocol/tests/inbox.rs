use lazoo_protocol::{
    INBOX_CONTRACT_V1, InboxAction, InboxAttention, InboxCounts, InboxCountsResult, InboxDeadline,
    InboxDeadlineKind, InboxFilter, InboxItem, InboxKind, InboxListParams, InboxListResult,
    InboxScope, InboxSeverity,
};
use serde_json::{Value, json};
use uuid::Uuid;

#[test]
fn inbox_list_params_default_to_two_hundred_and_reject_unknown_fields() {
    let params: InboxListParams = serde_json::from_value(json!({"filter": "all"})).unwrap();
    assert_eq!(params.filter, InboxFilter::All);
    assert_eq!(params.limit, 200);
    assert_eq!(params.before, None);
    let paged: InboxListParams = serde_json::from_value(json!({
        "filter": "resolved",
        "limit": 5,
        "before": "2026-10-13T00:00:00Z|00000000-0000-0000-0000-000000000001"
    }))
    .unwrap();
    assert_eq!(paged.filter, InboxFilter::Resolved);
    assert_eq!(paged.limit, 5);
    assert!(paged.before.is_some());
    assert!(serde_json::from_value::<InboxListParams>(json!({})).is_err());
    assert!(
        serde_json::from_value::<InboxListParams>(json!({"filter": "all", "unknown": 1})).is_err()
    );
    for (filter, wire) in [
        (InboxFilter::All, "all"),
        (InboxFilter::Approvals, "approvals"),
        (InboxFilter::Permissions, "permissions"),
        (InboxFilter::Incidents, "incidents"),
        (InboxFilter::Resolved, "resolved"),
    ] {
        assert_eq!(serde_json::to_value(filter).unwrap(), json!(wire));
    }
}

#[test]
fn inbox_items_and_counts_serialize_with_camel_case_keys_and_kebab_case_values() {
    assert_eq!(INBOX_CONTRACT_V1, 1);
    let item = InboxItem {
        kind: InboxKind::TaskUncertain,
        id: Uuid::from_u128(7),
        revision: 3,
        attention: InboxAttention::ActionRequired,
        severity: InboxSeverity::Priority,
        subtype: "task-uncertain".to_owned(),
        title: "Task outcome uncertain: Ship it".to_owned(),
        required_action: "Decide how the task ended".to_owned(),
        state: "uncertain".to_owned(),
        scope: InboxScope {
            project_id: None,
            conversation_id: Some(Uuid::from_u128(2)),
            employee_id: Some(Uuid::from_u128(3)),
            employee_name: Some("Mira".to_owned()),
            provider: Some("codex".to_owned()),
            task_id: Some(Uuid::from_u128(7)),
            turn_id: None,
            session_id: None,
        },
        created_at: "2026-10-13T00:00:00Z".to_owned(),
        updated_at: "2026-10-13T00:01:00Z".to_owned(),
        deadline: Some(InboxDeadline {
            kind: InboxDeadlineKind::Remediation,
            at: "2026-10-13T00:05:00Z".to_owned(),
        }),
        detail: json!({"objective": "Ship it"}),
        actions: vec![InboxAction {
            action: "resolve-failed".to_owned(),
            label: "Mark failed".to_owned(),
            method: "task.resolve".to_owned(),
            params: json!({"id": Uuid::from_u128(7), "expectedRevision": 3, "resolution": "failed"}),
        }],
    };
    let value = serde_json::to_value(&item).unwrap();
    assert_eq!(value["kind"], json!("task-uncertain"));
    assert_eq!(value["attention"], json!("action-required"));
    assert_eq!(value["severity"], json!("priority"));
    assert_eq!(value["requiredAction"], json!("Decide how the task ended"));
    assert_eq!(value["scope"]["projectId"], Value::Null);
    assert_eq!(value["scope"]["employeeName"], json!("Mira"));
    assert_eq!(value["deadline"]["kind"], json!("remediation"));
    assert_eq!(value["actions"][0]["method"], json!("task.resolve"));
    assert_eq!(serde_json::from_value::<InboxItem>(value).unwrap(), item);

    let counts = InboxCounts::default();
    assert_eq!(
        serde_json::to_value(counts).unwrap(),
        json!({"unresolved": 0, "approvals": 0, "permissions": 0, "incidents": 0, "watching": 0})
    );
    let list = InboxListResult {
        version: INBOX_CONTRACT_V1,
        items: vec![item],
        counts,
        event_high_water: 42,
        truncated: false,
        next_before: None,
    };
    let value = serde_json::to_value(&list).unwrap();
    assert_eq!(value["eventHighWater"], json!(42));
    assert_eq!(value["nextBefore"], Value::Null);
    assert_eq!(
        serde_json::from_value::<InboxListResult>(value).unwrap(),
        list
    );
    let counts_result = InboxCountsResult {
        version: INBOX_CONTRACT_V1,
        counts,
        event_high_water: 42,
    };
    assert_eq!(
        serde_json::to_value(counts_result).unwrap(),
        json!({"version": 1, "counts": counts, "eventHighWater": 42})
    );
}
