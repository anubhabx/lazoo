use lazoo_protocol::{CheckoutMode, JsonRpcRequest, JsonRpcResponse, MAX_FRAME_BYTES, PROTOCOL_V1};
use serde_json::json;

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
