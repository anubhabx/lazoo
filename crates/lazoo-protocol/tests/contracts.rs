use std::path::PathBuf;

use lazoo_protocol::{
    ActivationPolicy, AuthorityMode, CancelTurnParams, CheckoutMode, ConversationListParams,
    CreateConversationParams, CreateEmployeeParams, DesktopSnapshot, EmployeeCheckoutPolicy,
    JsonRpcRequest, JsonRpcResponse, MAX_FRAME_BYTES, MessageDeliveryState, MessageEndpoint,
    MessageKind, MessageSender, PROTOCOL_V1, ProviderProbeParams, ProviderSmokeParams,
    ProviderSmokeResult, RetryTurnParams, SendMessageParams, SessionLifecycleState,
    SubmitTurnParams, SystemStatus, TurnListParams, TurnState, UpdateEmployeeParams,
    UserMessageEndpoint, UserMessageKind, WarmSessionParams,
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
    assert_eq!(params.activation_policy, ActivationPolicy::Manual);
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
fn message_senders_preserve_employee_shape_and_identify_the_local_user() {
    let employee_id = Uuid::new_v4();
    let employee = MessageSender::Employee(MessageEndpoint {
        employee_id,
        session_id: None,
    });
    assert_eq!(
        serde_json::to_value(employee).unwrap(),
        json!({"employeeId": employee_id, "sessionId": null})
    );

    let user = MessageSender::User(UserMessageEndpoint {
        kind: UserMessageKind::User,
    });
    assert_eq!(serde_json::to_value(user).unwrap(), json!({"kind": "user"}));
    assert!(
        serde_json::from_value::<MessageSender>(json!({"kind": "user", "extra": true})).is_err()
    );
}

#[test]
fn conversation_contracts_are_strict_and_use_bounded_list_defaults() {
    let lead_employee_id = Uuid::new_v4();
    let create: CreateConversationParams = serde_json::from_value(json!({
        "leadEmployeeId": lead_employee_id,
        "title": "First task",
        "idempotencyKey": "conversation-1"
    }))
    .unwrap();
    assert_eq!(create.lead_employee_id, lead_employee_id);
    assert_eq!(create.project_id, None);
    assert_eq!(create.title.as_deref(), Some("First task"));

    let list: ConversationListParams = serde_json::from_value(json!({})).unwrap();
    assert_eq!(list.after_sequence, 0);
    assert_eq!(list.limit, 100);
    assert!(
        serde_json::from_value::<CreateConversationParams>(json!({
            "leadEmployeeId": lead_employee_id,
            "idempotencyKey": "conversation-1",
            "unexpected": true
        }))
        .is_err()
    );
}

#[test]
fn turn_contracts_are_strict_versioned_and_kebab_case() {
    let conversation_id = Uuid::new_v4();
    let submit: SubmitTurnParams = serde_json::from_value(json!({
        "conversationId": conversation_id,
        "author": {"kind": "user"},
        "text": "Reply with exactly READY",
        "idempotencyKey": "turn-1"
    }))
    .unwrap();
    assert_eq!(submit.conversation_id, conversation_id);
    assert_eq!(
        submit.author,
        MessageSender::User(UserMessageEndpoint {
            kind: UserMessageKind::User
        })
    );
    assert_eq!(submit.recipient, None);
    assert_eq!(submit.task_id, None);
    assert_eq!(submit.authority_mode, lazoo_protocol::AuthorityMode::Ask);
    assert!(!submit.activate_if_needed);
    assert_eq!(submit.max_attempts, 3);
    let task_id = Uuid::new_v4();
    let task_submit: SubmitTurnParams = serde_json::from_value(json!({
        "conversationId": conversation_id,
        "author": {"kind": "user"},
        "taskId": task_id,
        "authorityMode": "self-decision",
        "text": "continue",
        "idempotencyKey": "turn-task"
    }))
    .unwrap();
    assert_eq!(task_submit.task_id, Some(task_id));
    assert_eq!(
        task_submit.authority_mode,
        lazoo_protocol::AuthorityMode::SelfDecision
    );

    let retry: RetryTurnParams = serde_json::from_value(json!({
        "id": Uuid::new_v4(),
        "expectedRevision": 3,
        "idempotencyKey": "retry-1"
    }))
    .unwrap();
    assert_eq!(retry.expected_revision, 3);
    let cancel: CancelTurnParams = serde_json::from_value(json!({
        "id": Uuid::new_v4(),
        "expectedRevision": 2,
        "idempotencyKey": "cancel-1"
    }))
    .unwrap();
    assert_eq!(cancel.expected_revision, 2);

    let list: TurnListParams = serde_json::from_value(json!({
        "conversationId": conversation_id
    }))
    .unwrap();
    assert_eq!(list.after_sequence, 0);
    assert_eq!(list.limit, 100);
    assert_eq!(
        serde_json::to_string(&TurnState::RetryableFailed).unwrap(),
        r#""retryable-failed""#
    );
    assert!(
        serde_json::from_value::<SubmitTurnParams>(json!({
            "conversationId": conversation_id,
            "author": {"kind": "user"},
            "text": "hello",
            "idempotencyKey": "turn-1",
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

#[test]
fn activation_policies_round_trip_as_kebab_case_and_default_to_manual() {
    assert_eq!(
        serde_json::to_string(&ActivationPolicy::NoticeTurns).unwrap(),
        r#""notice-turns""#
    );
    assert_eq!(
        serde_json::from_str::<ActivationPolicy>(r#""manual""#).unwrap(),
        ActivationPolicy::Manual
    );
    assert_eq!(ActivationPolicy::default(), ActivationPolicy::Manual);
    assert!(serde_json::from_str::<ActivationPolicy>(r#""automatic""#).is_err());
}

#[test]
fn idle_timeouts_default_to_never_on_create_and_stay_unchanged_when_an_update_omits_them() {
    let create: CreateEmployeeParams = serde_json::from_value(json!({
        "name": "Rhea",
        "provider": "codex",
        "idempotencyKey": "create-rhea"
    }))
    .unwrap();
    assert_eq!(create.idle_timeout_seconds, 0);

    let update = |extra: serde_json::Value| -> UpdateEmployeeParams {
        let mut params = json!({
            "id": "00000000-0000-4000-8000-000000000001",
            "expectedRevision": 1,
            "idempotencyKey": "update-rhea"
        });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(params).unwrap()
    };
    assert_eq!(update(json!({})).idle_timeout_seconds, None);
    assert_eq!(
        update(json!({"idleTimeoutSeconds": 900})).idle_timeout_seconds,
        Some(900)
    );
    assert_eq!(
        update(json!({"idleTimeoutSeconds": 0})).idle_timeout_seconds,
        Some(0)
    );
}

#[test]
fn message_priority_is_kebab_case_and_defaults_to_normal_for_older_records() {
    use lazoo_protocol::{MessageEnvelope, MessagePriority};

    let sender = Uuid::new_v4();
    let params: SendMessageParams = serde_json::from_value(json!({
        "sender": {"employeeId": sender},
        "recipient": {"employeeId": sender},
        "kind": "status",
        "body": {"text": "Deploy is blocked."},
        "idempotencyKey": "message-1"
    }))
    .unwrap();
    assert_eq!(params.priority, MessagePriority::Normal);
    let urgent: SendMessageParams = serde_json::from_value(json!({
        "sender": {"employeeId": sender},
        "recipient": {"employeeId": sender},
        "kind": "status",
        "priority": "priority",
        "body": {"text": "Deploy is blocked."},
        "idempotencyKey": "message-2"
    }))
    .unwrap();
    assert_eq!(urgent.priority, MessagePriority::Priority);
    assert_eq!(
        serde_json::to_string(&MessagePriority::Priority).unwrap(),
        r#""priority""#
    );

    let id = Uuid::new_v4();
    let older: MessageEnvelope = serde_json::from_value(json!({
        "version": 1,
        "id": id,
        "idempotencyKey": "older",
        "correlationId": id,
        "causationId": null,
        "sender": {"employeeId": sender, "sessionId": null},
        "recipient": {"employeeId": sender, "sessionId": null},
        "projectId": null,
        "conversationId": null,
        "taskId": null,
        "kind": "peer",
        "body": {"text": "hello"},
        "references": [],
        "createdAt": "2026-10-11T00:00:00Z"
    }))
    .unwrap();
    assert_eq!(older.priority, MessagePriority::Normal);
    assert_eq!(serde_json::to_value(&older).unwrap()["priority"], "normal");
}
