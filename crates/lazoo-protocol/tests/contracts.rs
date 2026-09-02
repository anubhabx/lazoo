use std::path::PathBuf;

use lazoo_protocol::{
    AuthorityMode, CheckoutMode, CreateEmployeeParams, DesktopSnapshot, EmployeeCheckoutPolicy,
    JsonRpcRequest, JsonRpcResponse, MAX_FRAME_BYTES, MessageDeliveryState, MessageKind,
    PROTOCOL_V1, ProviderProbeParams, ProviderSmokeParams, ProviderSmokeResult, SendMessageParams,
    SessionLifecycleState, SystemStatus, WarmSessionParams,
};
use serde_json::json;
use uuid::Uuid;

#[test]
fn protocol_constants_are_stable() {
    assert_eq!(PROTOCOL_V1, 1);
    assert_eq!(MAX_FRAME_BYTES, 1024 * 1024);
}

#[test]
fn provider_parameters_and_smoke_result_use_strict_camel_case_contracts() {
    let probe: ProviderProbeParams = serde_json::from_value(json!({"provider": "codex"})).unwrap();
    assert_eq!(probe.provider, "codex");
    assert!(
        serde_json::from_value::<ProviderProbeParams>(
            json!({"provider": "codex", "unexpected": true})
        )
        .is_err()
    );

    let smoke: ProviderSmokeParams = serde_json::from_value(json!({
        "provider": "grok-build",
        "prompt": "Reply OK"
    }))
    .unwrap();
    assert_eq!(smoke.prompt, "Reply OK");

    let result = ProviderSmokeResult {
        provider: "grok-build".to_owned(),
        session_id: "session-1".to_owned(),
        events: vec![json!({"type": "turnCompleted"})],
        transcript_truncated: false,
        checked_at: "2026-08-30T00:00:00Z".to_owned(),
    };
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["sessionId"], "session-1");
    assert_eq!(value["transcriptTruncated"], false);
    assert!(value.get("session_id").is_none());
}

#[test]
fn checkout_mode_defaults_to_isolated() {
    assert_eq!(CheckoutMode::default(), CheckoutMode::Isolated);
}

#[test]
fn checkout_modes_round_trip_as_kebab_case() {
    assert_eq!(
        serde_json::to_string(&CheckoutMode::LeadMain).unwrap(),
        r#""lead-main""#
    );
    assert_eq!(
        serde_json::from_str::<CheckoutMode>(r#""isolated""#).unwrap(),
        CheckoutMode::Isolated
    );
}

#[test]
fn employee_defaults_are_ask_isolated_enabled_and_strict() {
    let params: CreateEmployeeParams = serde_json::from_value(json!({
        "name": "Rhea",
        "provider": "codex",
        "idempotencyKey": "create-rhea"
    }))
    .unwrap();

    assert_eq!(params.authority_mode, AuthorityMode::Ask);
    assert_eq!(params.checkout_policy, EmployeeCheckoutPolicy::Isolated);
    assert!(params.enabled);
    assert_eq!(params.role, "");
    assert_eq!(params.instructions, "");
    assert_eq!(params.native_settings, json!({}));
    assert!(
        serde_json::from_value::<CreateEmployeeParams>(json!({
            "name": "Rhea",
            "provider": "codex",
            "idempotencyKey": "create-rhea",
            "unexpected": true
        }))
        .is_err()
    );
}

#[test]
fn employee_policy_enums_round_trip_as_kebab_case() {
    assert_eq!(
        serde_json::to_string(&AuthorityMode::SelfDecision).unwrap(),
        r#""self-decision""#
    );
    assert_eq!(
        serde_json::to_string(&AuthorityMode::FullAutonomy).unwrap(),
        r#""full-autonomy""#
    );
    assert_eq!(
        serde_json::to_string(&EmployeeCheckoutPolicy::ReadOnly).unwrap(),
        r#""read-only""#
    );
}

#[test]
fn session_contracts_are_strict_and_states_are_kebab_case() {
    let employee_id = Uuid::new_v4();
    let params: WarmSessionParams = serde_json::from_value(json!({
        "employeeId": employee_id,
        "expectedEmployeeRevision": 3,
        "idempotencyKey": "warm-rhea"
    }))
    .unwrap();
    assert_eq!(params.employee_id, employee_id);
    assert_eq!(params.expected_employee_revision, 3);
    assert!(
        serde_json::from_value::<WarmSessionParams>(json!({
            "employeeId": employee_id,
            "expectedEmployeeRevision": 3,
            "idempotencyKey": "warm-rhea",
            "unexpected": true
        }))
        .is_err()
    );
    assert_eq!(
        serde_json::to_string(&SessionLifecycleState::Starting).unwrap(),
        r#""starting""#
    );
}

#[test]
fn message_contracts_are_strict_versioned_and_kebab_case() {
    let sender = Uuid::new_v4();
    let recipient = Uuid::new_v4();
    let params: SendMessageParams = serde_json::from_value(json!({
        "sender": {"employeeId": sender},
        "recipient": {"employeeId": recipient},
        "kind": "peer",
        "body": {"text": "Need the spacing token."},
        "idempotencyKey": "message-1"
    }))
    .unwrap();

    assert_eq!(params.sender.employee_id, sender);
    assert_eq!(params.recipient.employee_id, recipient);
    assert_eq!(params.kind, MessageKind::Peer);
    assert_eq!(params.max_attempts, 3);
    assert!(params.references.is_empty());
    assert_eq!(
        serde_json::to_string(&MessageDeliveryState::RetryableFailed).unwrap(),
        r#""retryable-failed""#
    );
    assert!(
        serde_json::from_value::<SendMessageParams>(json!({
            "sender": {"employeeId": sender},
            "recipient": {"employeeId": recipient},
            "kind": "peer",
            "body": {"text": "Need the spacing token."},
            "idempotencyKey": "message-1",
            "unexpected": true
        }))
        .is_err()
    );
}

#[test]
fn unknown_rpc_request_fields_are_rejected() {
    let json = r#"{
        "jsonrpc":"2.0",
        "id":"request-1",
        "method":"system.status",
        "params":{},
        "extra":true
    }"#;

    assert!(serde_json::from_str::<JsonRpcRequest>(json).is_err());
}

#[test]
fn json_rpc_success_response_has_no_error_member() {
    let response = JsonRpcResponse::success("request-1", json!({"ready": true}));
    let value = serde_json::to_value(response).unwrap();

    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], "request-1");
    assert_eq!(value["result"]["ready"], true);
    assert!(value.get("error").is_none());
}

#[test]
fn desktop_snapshot_uses_strict_camel_case_contract() {
    let snapshot = DesktopSnapshot {
        captured_at: "2026-08-28T00:00:00Z".to_owned(),
        status: SystemStatus {
            daemon_version: "0.1.0".to_owned(),
            boot_id: Uuid::nil(),
            uptime_millis: 15_000,
            protocol_version: PROTOCOL_V1,
            data_root: PathBuf::from(r"C:\Temp\Lazoo"),
            schema_version: 1,
            journal_mode: "wal".to_owned(),
            project_count: 0,
            event_high_water: 0,
            pending_outbox: 0,
            emergency_incident_count: 0,
            keeper: None,
            degraded: Vec::new(),
        },
        projects: Vec::new(),
        events: Vec::new(),
    };

    let value = serde_json::to_value(&snapshot).expect("snapshot serializes");

    assert_eq!(value["capturedAt"], "2026-08-28T00:00:00Z");
    assert!(value.get("captured_at").is_none());
    assert!(value["projects"].as_array().is_some_and(Vec::is_empty));
    assert!(value["events"].as_array().is_some_and(Vec::is_empty));

    let mut invalid = value;
    invalid["unexpected"] = json!(true);
    assert!(serde_json::from_value::<DesktopSnapshot>(invalid).is_err());
}
