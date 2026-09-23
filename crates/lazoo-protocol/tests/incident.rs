use lazoo_protocol::{
    AcknowledgeIncidentParams, INCIDENT_CONTRACT_V1, IncidentListParams, IncidentPriority,
    IncidentRecord, IncidentSource, IncidentState, ResolveIncidentParams,
};
use serde_json::json;
use uuid::Uuid;

fn incident_record() -> IncidentRecord {
    IncidentRecord {
        sequence: 7,
        version: INCIDENT_CONTRACT_V1,
        id: Uuid::nil(),
        code: "LZ-SESSION-STALLED".to_owned(),
        fingerprint: "abc".to_owned(),
        title: "Session accepted work but produced no progress".to_owned(),
        explanation: "No progress evidence arrived.".to_owned(),
        next_action: "Retry explicitly.".to_owned(),
        project_id: None,
        employee_id: Some(Uuid::nil()),
        provider: Some("codex".to_owned()),
        task_id: None,
        session_id: Some(Uuid::nil()),
        turn_id: Some(Uuid::nil()),
        message_id: None,
        source: IncidentSource::Daemon,
        detail: json!({"failureCode": "LZ-TURN-PROVIDER"}),
        occurrences: 3,
        window_occurrences: 3,
        first_occurred_at: "2026-09-23T00:00:00Z".to_owned(),
        last_occurred_at: "2026-09-23T00:02:00Z".to_owned(),
        window_started_at: "2026-09-23T00:00:00Z".to_owned(),
        priority: IncidentPriority::Priority,
        state: IncidentState::Open,
        revision: 3,
        created_at: "2026-09-23T00:00:00Z".to_owned(),
        updated_at: "2026-09-23T00:02:00Z".to_owned(),
        acknowledged_at: None,
        resolved_at: None,
    }
}

#[test]
fn incident_enums_use_stable_kebab_case_wire_values() {
    assert_eq!(
        serde_json::to_string(&IncidentState::Acknowledged).unwrap(),
        r#""acknowledged""#
    );
    assert_eq!(
        serde_json::to_string(&IncidentPriority::Priority).unwrap(),
        r#""priority""#
    );
    assert_eq!(
        serde_json::to_string(&IncidentSource::Keeper).unwrap(),
        r#""keeper""#
    );
    assert_eq!(
        serde_json::from_str::<IncidentState>(r#""resolved""#).unwrap(),
        IncidentState::Resolved
    );
}

#[test]
fn incident_record_round_trips_with_camel_case_keys() {
    let record = incident_record();
    let value = serde_json::to_value(&record).unwrap();
    assert_eq!(value["nextAction"], "Retry explicitly.");
    assert_eq!(value["windowOccurrences"], 3);
    assert_eq!(value["firstOccurredAt"], "2026-09-23T00:00:00Z");
    assert!(value.get("next_action").is_none());
    let parsed: IncidentRecord = serde_json::from_value(value).unwrap();
    assert_eq!(parsed, record);
}

#[test]
fn incident_params_have_defaults_and_reject_unknown_fields() {
    let list: IncidentListParams = serde_json::from_value(json!({})).unwrap();
    assert_eq!(list, IncidentListParams::default());
    assert_eq!(list.limit, 100);
    let filtered: IncidentListParams =
        serde_json::from_value(json!({"state": "open", "priority": "priority", "code": "LZ-X"}))
            .unwrap();
    assert_eq!(filtered.state, Some(IncidentState::Open));
    assert_eq!(filtered.code.as_deref(), Some("LZ-X"));
    assert!(serde_json::from_value::<IncidentListParams>(json!({"severity": "high"})).is_err());
    let acknowledge: AcknowledgeIncidentParams = serde_json::from_value(json!({
        "id": Uuid::nil(),
        "expectedRevision": 2,
        "idempotencyKey": "ack-1"
    }))
    .unwrap();
    assert_eq!(acknowledge.expected_revision, 2);
    assert!(
        serde_json::from_value::<ResolveIncidentParams>(json!({"id": Uuid::nil()})).is_err(),
        "resolution must cite the revision it is bound to"
    );
}
