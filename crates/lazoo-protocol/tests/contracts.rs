use std::path::PathBuf;

use lazoo_protocol::{
    CheckoutMode, DesktopSnapshot, JsonRpcRequest, JsonRpcResponse, MAX_FRAME_BYTES, PROTOCOL_V1,
    SystemStatus,
};
use serde_json::json;
use uuid::Uuid;

#[test]
fn protocol_constants_are_stable() {
    assert_eq!(PROTOCOL_V1, 1);
    assert_eq!(MAX_FRAME_BYTES, 1024 * 1024);
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
