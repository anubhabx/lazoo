use lazoo_protocol::{
    AcknowledgeIncidentParams, INCIDENT_CONTRACT_V1, IncidentEscalation, IncidentEscalationReason,
    IncidentListParams, IncidentPriority, IncidentRecord, IncidentRemedyKind, IncidentRemedyRecord,
    IncidentSource, IncidentState, ResolveIncidentParams,
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
        escalation: IncidentEscalation::Lead,
        remediation_deadline_at: Some("2026-09-23T00:07:00Z".to_owned()),
        escalated_at: None,
        escalation_reason: None,
        remedies: vec![IncidentRemedyRecord {
            id: Uuid::nil(),
            incident_id: Uuid::nil(),
            attempt: 1,
            kind: IncidentRemedyKind::RedelegateTask,
            lead_employee_id: Uuid::nil(),
            lead_task_id: Uuid::nil(),
            turn_id: Uuid::nil(),
            target_task_id: Uuid::nil(),
            result_task_id: Some(Uuid::nil()),
            occurrences_at_remedy: 3,
            note: "try another worker".to_owned(),
            created_at: "2026-09-23T00:03:00Z".to_owned(),
        }],
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
    assert_eq!(
        serde_json::to_string(&IncidentEscalation::Remediated).unwrap(),
        r#""remediated""#
    );
    assert_eq!(
        serde_json::to_string(&IncidentEscalationReason::RecurredAfterRemedy).unwrap(),
        r#""recurred-after-remedy""#
    );
    assert_eq!(
        serde_json::to_string(&IncidentRemedyKind::CancelTask).unwrap(),
        r#""cancel-task""#
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
    assert_eq!(value["escalation"], "lead");
    assert_eq!(value["remediationDeadlineAt"], "2026-09-23T00:07:00Z");
    assert_eq!(value["remedies"][0]["kind"], "redelegate-task");
    assert_eq!(value["remedies"][0]["occurrencesAtRemedy"], 3);
    let parsed: IncidentRecord = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(parsed, record);

    // Records written before schema 12 (event payloads) carry no escalation fields.
    let mut legacy = value;
    for field in [
        "escalation",
        "remediationDeadlineAt",
        "escalatedAt",
        "escalationReason",
        "remedies",
    ] {
        legacy.as_object_mut().unwrap().remove(field);
    }
    let legacy: IncidentRecord = serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy.escalation, IncidentEscalation::None);
    assert!(legacy.remedies.is_empty());
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
    let escalated: IncidentListParams =
        serde_json::from_value(json!({"escalation": "user"})).unwrap();
    assert_eq!(escalated.escalation, Some(IncidentEscalation::User));
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
