use std::path::PathBuf;

use clap::Parser;
use lazoo_protocol::{CheckoutMode, MessageKind, RpcError};
use lazooctl::{
    Cli, Command, ConversationCommand, EmployeeCommand, MessageCommand, ProjectCommand,
    RosterCommand, SessionCommand, TurnCommand, command_request, exit_code_for_rpc, render_result,
};
use serde_json::json;

const SENDER_ID: &str = "00000000-0000-4000-8000-000000000001";
const RECIPIENT_ID: &str = "00000000-0000-4000-8000-000000000002";
const MESSAGE_ID: &str = "00000000-0000-4000-8000-000000000003";
const ATTEMPT_ID: &str = "00000000-0000-4000-8000-000000000004";
const CONVERSATION_ID: &str = "00000000-0000-4000-8000-000000000005";
const TURN_ID: &str = "00000000-0000-4000-8000-000000000006";
const TASK_ID: &str = "00000000-0000-4000-8000-000000000007";
const ROOT_TASK_ID: &str = "00000000-0000-4000-8000-000000000008";
const SESSION_ID: &str = "00000000-0000-4000-8000-000000000009";

#[test]
fn project_register_defaults_to_isolated() {
    let cli = Cli::try_parse_from(["lazooctl", "project", "register", "C:\\repo"]).unwrap();
    let Command::Project(ProjectCommand::Register { checkout_mode, .. }) = cli.command else {
        panic!("expected register command");
    };
    assert_eq!(checkout_mode, CheckoutMode::Isolated);
}

#[test]
fn project_register_accepts_explicit_lead_main() {
    let cli = Cli::try_parse_from([
        "lazooctl",
        "--json",
        "project",
        "register",
        "C:\\repo",
        "--checkout-mode",
        "lead-main",
        "--idempotency-key",
        "same",
    ])
    .unwrap();
    let Command::Project(ProjectCommand::Register {
        path,
        checkout_mode,
        idempotency_key,
        ..
    }) = cli.command
    else {
        panic!("expected register command");
    };
    assert!(cli.json);
    assert_eq!(path, PathBuf::from("C:\\repo"));
    assert_eq!(checkout_mode, CheckoutMode::LeadMain);
    assert_eq!(idempotency_key.as_deref(), Some("same"));
}

#[test]
fn json_output_preserves_stable_protocol_keys() {
    let output = render_result(&json!({"protocolVersion": 1, "bootId": "boot"}), true).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output).unwrap()["protocolVersion"],
        1
    );
    assert!(!output.contains("\u{1b}["));
}

#[test]
fn rpc_error_families_have_stable_exit_codes() {
    let cases = [
        ("LZ-INVALID-PARAMS", 2),
        ("LZ-IPC", 3),
        ("LZ-PROTOCOL-INCOMPATIBLE", 4),
        ("LZ-IPC-AUTH", 5),
        ("LZ-PROJECT-CONFLICT", 6),
        ("LZ-STORE", 7),
        ("LZ-PROVIDER-TIMEOUT", 8),
        ("LZ-PROVIDER-NOT-DETECTED", 8),
        ("LZ-EMPLOYEE-STALE", 9),
        ("LZ-EMPLOYEE-NOT-FOUND", 9),
        ("LZ-SESSION-STALE", 10),
        ("LZ-MESSAGE-STALE", 11),
        ("LZ-UNEXPECTED", 10),
    ];
    for (code, expected) in cases {
        assert_eq!(
            exit_code_for_rpc(&RpcError {
                code: code.to_owned(),
                message: "failure".to_owned(),
                data: None
            }),
            expected
        );
    }
}

#[test]
fn message_cli_maps_send_envelope() {
    let send = Cli::try_parse_from([
        "lazooctl",
        "message",
        "send",
        SENDER_ID,
        RECIPIENT_ID,
        "--kind",
        "peer",
        "--body",
        r#"{"text":"Need the spacing token."}"#,
        "--references",
        r#"[{"kind":"file","target":"tokens.css","metadata":{}}]"#,
        "--max-attempts",
        "2",
        "--idempotency-key",
        "send-message",
    ])
    .unwrap();
    let Command::Message(MessageCommand::Send { kind, .. }) = &send.command else {
        panic!("expected message send command");
    };
    assert_eq!(*kind, MessageKind::Peer);
    assert_eq!(
        command_request(&send.command),
        (
            "message.send",
            json!({
                "sender": {"employeeId": SENDER_ID, "sessionId": null},
                "recipient": {"employeeId": RECIPIENT_ID, "sessionId": null},
                "projectId": null,
                "conversationId": null,
                "taskId": null,
                "correlationId": null,
                "causationId": null,
                "kind": "peer",
                "body": {"text": "Need the spacing token."},
                "references": [{"kind": "file", "target": "tokens.css", "metadata": {}}],
                "maxAttempts": 2,
                "idempotencyKey": "send-message"
            }),
            false
        )
    );
}

#[test]
fn message_cli_maps_delivery_outcomes() {
    let claim = Cli::try_parse_from([
        "lazooctl",
        "message",
        "claim",
        MESSAGE_ID,
        "--expected-revision",
        "1",
        "--idempotency-key",
        "claim-message",
    ])
    .unwrap();
    assert_eq!(command_request(&claim.command).0, "message.claim");

    let acknowledge = Cli::try_parse_from([
        "lazooctl",
        "message",
        "acknowledge",
        MESSAGE_ID,
        "--expected-revision",
        "2",
        "--attempt-id",
        ATTEMPT_ID,
        "--acknowledgement-key",
        "provider-ack-1",
        "--evidence",
        r#"{"progress":true}"#,
        "--idempotency-key",
        "ack-message",
    ])
    .unwrap();
    assert_eq!(
        command_request(&acknowledge.command),
        (
            "message.acknowledge",
            json!({
                "id": MESSAGE_ID,
                "expectedRevision": 2,
                "attemptId": ATTEMPT_ID,
                "acknowledgementKey": "provider-ack-1",
                "evidence": {"progress": true},
                "idempotencyKey": "ack-message"
            }),
            false
        )
    );

    let fail = Cli::try_parse_from([
        "lazooctl",
        "message",
        "fail",
        MESSAGE_ID,
        "--expected-revision",
        "2",
        "--attempt-id",
        ATTEMPT_ID,
        "--retryable",
        "true",
        "--failure-code",
        "LZ-MESSAGE-TIMEOUT",
        "--failure-message",
        "acknowledgement timed out",
        "--idempotency-key",
        "fail-message",
    ])
    .unwrap();
    assert_eq!(command_request(&fail.command).0, "message.fail");
}

#[test]
fn message_cli_maps_retry_cancel_and_reads() {
    for (arguments, method) in [
        (
            vec![
                "lazooctl",
                "message",
                "retry",
                MESSAGE_ID,
                "--expected-revision",
                "3",
                "--idempotency-key",
                "retry-message",
            ],
            "message.retry",
        ),
        (
            vec![
                "lazooctl",
                "message",
                "cancel",
                MESSAGE_ID,
                "--expected-revision",
                "3",
                "--idempotency-key",
                "cancel-message",
            ],
            "message.cancel",
        ),
        (
            vec!["lazooctl", "message", "list", "--after-sequence", "4"],
            "message.list",
        ),
        (
            vec!["lazooctl", "message", "show", MESSAGE_ID],
            "message.show",
        ),
    ] {
        let cli = Cli::try_parse_from(arguments).unwrap();
        assert_eq!(command_request(&cli.command).0, method);
    }

    let generated_key = Cli::try_parse_from([
        "lazooctl",
        "message",
        "claim",
        MESSAGE_ID,
        "--expected-revision",
        "1",
    ])
    .unwrap();
    assert!(
        command_request(&generated_key.command).1["idempotencyKey"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
}

#[test]
fn session_cli_maps_warm_list_show_and_stop() {
    let employee_id = "00000000-0000-4000-8000-000000000001";
    let session_id = "00000000-0000-4000-8000-000000000002";
    let warm = Cli::try_parse_from([
        "lazooctl",
        "session",
        "warm",
        employee_id,
        "--expected-employee-revision",
        "3",
        "--idempotency-key",
        "warm-rhea",
    ])
    .unwrap();
    assert!(matches!(
        warm.command,
        Command::Session(SessionCommand::Warm { .. })
    ));
    assert_eq!(
        command_request(&warm.command),
        (
            "session.warm",
            json!({
                "employeeId": employee_id,
                "expectedEmployeeRevision": 3,
                "idempotencyKey": "warm-rhea"
            }),
            false
        )
    );

    let list = Cli::try_parse_from(["lazooctl", "session", "list"]).unwrap();
    assert_eq!(
        command_request(&list.command),
        ("session.list", json!({}), false)
    );

    let show = Cli::try_parse_from(["lazooctl", "session", "show", session_id]).unwrap();
    assert_eq!(
        command_request(&show.command),
        ("session.show", json!({"id": session_id}), false)
    );

    let stop = Cli::try_parse_from([
        "lazooctl",
        "session",
        "stop",
        session_id,
        "--expected-revision",
        "2",
        "--idempotency-key",
        "stop-rhea",
    ])
    .unwrap();
    assert_eq!(
        command_request(&stop.command),
        (
            "session.stop",
            json!({
                "id": session_id,
                "expectedRevision": 2,
                "idempotencyKey": "stop-rhea"
            }),
            false
        )
    );
}

#[test]
fn provider_cli_maps_list_probe_and_explicit_smoke_commands() {
    let list = Cli::try_parse_from(["lazooctl", "provider", "list"]).unwrap();
    assert_eq!(
        command_request(&list.command),
        ("provider.list", json!({}), false)
    );

    let probe = Cli::try_parse_from(["lazooctl", "provider", "probe", "codex"]).unwrap();
    assert_eq!(
        command_request(&probe.command),
        ("provider.probe", json!({"provider": "codex"}), false)
    );

    let smoke = Cli::try_parse_from([
        "lazooctl",
        "provider",
        "smoke",
        "grok-build",
        "--prompt",
        "Reply OK",
    ])
    .unwrap();
    assert_eq!(
        command_request(&smoke.command),
        (
            "provider.smoke",
            json!({"provider": "grok-build", "prompt": "Reply OK"}),
            false
        )
    );
}

#[test]
fn employee_cli_maps_mutations_and_roster_refresh() {
    let create = Cli::try_parse_from([
        "lazooctl",
        "employee",
        "create",
        "Rhea",
        "--provider",
        "codex",
    ])
    .unwrap();
    let Command::Employee(EmployeeCommand::Create {
        authority_mode,
        checkout_policy,
        ..
    }) = &create.command
    else {
        panic!("expected employee create");
    };
    assert_eq!(*authority_mode, lazoo_protocol::AuthorityMode::Ask);
    assert_eq!(
        *checkout_policy,
        lazoo_protocol::EmployeeCheckoutPolicy::Isolated
    );
    let (method, params, follows) = command_request(&create.command);
    assert_eq!(method, "employee.create");
    assert_eq!(params["nativeSettings"], json!({}));
    assert!(!follows);

    let employee_id = "00000000-0000-4000-8000-000000000001";
    let update = Cli::try_parse_from([
        "lazooctl",
        "employee",
        "update",
        employee_id,
        "--expected-revision",
        "1",
        "--role",
        "Lead",
    ])
    .unwrap();
    assert_eq!(command_request(&update.command).0, "employee.update");

    let disable = Cli::try_parse_from([
        "lazooctl",
        "employee",
        "disable",
        employee_id,
        "--expected-revision",
        "2",
    ])
    .unwrap();
    assert_eq!(command_request(&disable.command).0, "employee.disable");

    let refresh = Cli::try_parse_from(["lazooctl", "roster", "refresh"]).unwrap();
    assert!(matches!(
        refresh.command,
        Command::Roster(RosterCommand::Refresh { .. })
    ));
    assert_eq!(command_request(&refresh.command).0, "roster.refresh");
}

#[test]
fn conversation_cli_maps_create_list_and_show() {
    let create = Cli::try_parse_from([
        "lazooctl",
        "--json",
        "conversation",
        "create",
        RECIPIENT_ID,
        "--title",
        "First task",
        "--idempotency-key",
        "create-conversation",
    ])
    .unwrap();
    assert!(create.json);
    assert!(matches!(
        create.command,
        Command::Conversation(ConversationCommand::Create { .. })
    ));
    assert_eq!(
        command_request(&create.command),
        (
            "conversation.create",
            json!({
                "projectId": null,
                "leadEmployeeId": RECIPIENT_ID,
                "title": "First task",
                "idempotencyKey": "create-conversation"
            }),
            false
        )
    );

    let list = Cli::try_parse_from([
        "lazooctl",
        "conversation",
        "list",
        "--after-sequence",
        "0",
        "--limit",
        "100",
    ])
    .unwrap();
    assert_eq!(
        command_request(&list.command),
        (
            "conversation.list",
            json!({"afterSequence": 0, "limit": 100}),
            false
        )
    );

    let show = Cli::try_parse_from(["lazooctl", "conversation", "show", CONVERSATION_ID]).unwrap();
    assert_eq!(
        command_request(&show.command),
        ("conversation.show", json!({"id": CONVERSATION_ID}), false)
    );

    let generated =
        Cli::try_parse_from(["lazooctl", "conversation", "create", RECIPIENT_ID]).unwrap();
    let generated_key = command_request(&generated.command).1["idempotencyKey"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(uuid::Uuid::parse_str(&generated_key).is_ok());
}

#[test]
fn turn_cli_maps_user_and_employee_submission() {
    let user = Cli::try_parse_from([
        "lazooctl",
        "turn",
        "submit",
        CONVERSATION_ID,
        "--text",
        "Reply with exactly READY",
        "--activate-if-needed",
        "--idempotency-key",
        "submit-user-turn",
    ])
    .unwrap();
    assert!(matches!(
        user.command,
        Command::Turn(TurnCommand::Submit { .. })
    ));
    assert_eq!(
        command_request(&user.command),
        (
            "turn.submit",
            json!({
                "conversationId": CONVERSATION_ID,
                "author": {"kind": "user"},
                "recipient": null,
                "taskId": null,
                "authorityMode": "ask",
                "text": "Reply with exactly READY",
                "activateIfNeeded": true,
                "maxAttempts": 3,
                "idempotencyKey": "submit-user-turn"
            }),
            false
        )
    );

    let employee = Cli::try_parse_from([
        "lazooctl",
        "turn",
        "submit",
        CONVERSATION_ID,
        "--author-employee",
        SENDER_ID,
        "--recipient-employee",
        RECIPIENT_ID,
        "--text",
        "status",
        "--idempotency-key",
        "submit-employee-turn",
    ])
    .unwrap();
    assert_eq!(
        command_request(&employee.command),
        (
            "turn.submit",
            json!({
                "conversationId": CONVERSATION_ID,
                "author": {"employeeId": SENDER_ID, "sessionId": null},
                "recipient": {"employeeId": RECIPIENT_ID, "sessionId": null},
                "taskId": null,
                "authorityMode": "ask",
                "text": "status",
                "activateIfNeeded": false,
                "maxAttempts": 3,
                "idempotencyKey": "submit-employee-turn"
            }),
            false
        )
    );

    let missing_recipient = Cli::try_parse_from([
        "lazooctl",
        "turn",
        "submit",
        CONVERSATION_ID,
        "--author-employee",
        SENDER_ID,
        "--text",
        "status",
    ]);
    assert!(missing_recipient.is_err());
}

#[test]
fn turn_cli_maps_retry_cancel_list_and_show() {
    for (arguments, method, expected) in [
        (
            vec![
                "lazooctl",
                "turn",
                "retry",
                TURN_ID,
                "--expected-revision",
                "3",
                "--idempotency-key",
                "retry-turn",
            ],
            "turn.retry",
            json!({
                "id": TURN_ID,
                "expectedRevision": 3,
                "idempotencyKey": "retry-turn"
            }),
        ),
        (
            vec![
                "lazooctl",
                "turn",
                "cancel",
                TURN_ID,
                "--expected-revision",
                "2",
                "--idempotency-key",
                "cancel-turn",
            ],
            "turn.cancel",
            json!({
                "id": TURN_ID,
                "expectedRevision": 2,
                "idempotencyKey": "cancel-turn"
            }),
        ),
        (
            vec![
                "lazooctl",
                "turn",
                "list",
                CONVERSATION_ID,
                "--after-sequence",
                "0",
                "--limit",
                "100",
            ],
            "turn.list",
            json!({
                "conversationId": CONVERSATION_ID,
                "afterSequence": 0,
                "limit": 100
            }),
        ),
        (
            vec!["lazooctl", "turn", "show", TURN_ID],
            "turn.show",
            json!({"id": TURN_ID}),
        ),
    ] {
        let cli = Cli::try_parse_from(arguments).unwrap();
        assert_eq!(command_request(&cli.command), (method, expected, false));
    }

    let generated = Cli::try_parse_from([
        "lazooctl",
        "turn",
        "retry",
        TURN_ID,
        "--expected-revision",
        "3",
    ])
    .unwrap();
    let generated_key = command_request(&generated.command).1["idempotencyKey"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(uuid::Uuid::parse_str(&generated_key).is_ok());
}

#[test]
fn conversation_and_turn_errors_use_exit_code_twelve() {
    for code in [
        "LZ-CONVERSATION-NOT-FOUND",
        "LZ-CONVERSATION-INVALID",
        "LZ-TURN-STALE",
        "LZ-TURN-UNCERTAIN",
    ] {
        assert_eq!(
            exit_code_for_rpc(&RpcError {
                code: code.to_owned(),
                message: "failure".to_owned(),
                data: None,
            }),
            12
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn task_cli_maps_the_complete_workflow_and_turn_authority() {
    let cases = [
        (
            vec![
                "lazooctl",
                "task",
                "delegate",
                ROOT_TASK_ID,
                "--actor-employee",
                SENDER_ID,
                "--actor-session",
                SESSION_ID,
                "--assigned-employee",
                RECIPIENT_ID,
                "--objective",
                "inspect parser",
                "--references",
                "[]",
                "--requested-authority",
                "self-decision",
                "--roster-revision",
                "7",
                "--idempotency-key",
                "delegate-task",
            ],
            "task.delegate",
            json!({
                "parentTaskId": ROOT_TASK_ID,
                "actor": {"employeeId": SENDER_ID, "sessionId": SESSION_ID},
                "assignedEmployeeId": RECIPIENT_ID,
                "objective": "inspect parser",
                "references": [],
                "requestedAuthority": "self-decision",
                "rosterRevision": 7,
                "idempotencyKey": "delegate-task"
            }),
        ),
        (
            vec![
                "lazooctl",
                "task",
                "accept",
                TASK_ID,
                "--expected-revision",
                "1",
                "--idempotency-key",
                "accept-task",
            ],
            "task.accept",
            json!({"id": TASK_ID, "expectedRevision": 1, "idempotencyKey": "accept-task"}),
        ),
        (
            vec![
                "lazooctl",
                "task",
                "start",
                TASK_ID,
                "--expected-revision",
                "2",
                "--actor-employee",
                RECIPIENT_ID,
                "--actor-session",
                SESSION_ID,
                "--turn-id",
                TURN_ID,
                "--evidence",
                "{\"native\":true}",
                "--idempotency-key",
                "start-task",
            ],
            "task.start",
            json!({"id": TASK_ID, "expectedRevision": 2, "actor": {"employeeId": RECIPIENT_ID, "sessionId": SESSION_ID}, "turnId": TURN_ID, "evidence": {"native": true}, "idempotencyKey": "start-task"}),
        ),
        (
            vec![
                "lazooctl",
                "task",
                "report",
                TASK_ID,
                "--expected-revision",
                "3",
                "--actor-employee",
                RECIPIENT_ID,
                "--actor-session",
                SESSION_ID,
                "--kind",
                "failed",
                "--evidence",
                "{\"exitCode\":1}",
                "--failure-code",
                "worker-failed",
                "--failure-message",
                "worker failed",
                "--idempotency-key",
                "report-task",
            ],
            "task.report",
            json!({"id": TASK_ID, "expectedRevision": 3, "actor": {"employeeId": RECIPIENT_ID, "sessionId": SESSION_ID}, "kind": "failed", "evidence": {"exitCode": 1}, "failureCode": "worker-failed", "failureMessage": "worker failed", "idempotencyKey": "report-task"}),
        ),
        (
            vec![
                "lazooctl",
                "task",
                "cancel",
                TASK_ID,
                "--expected-revision",
                "3",
                "--evidence",
                "{\"reason\":\"stop\"}",
                "--idempotency-key",
                "cancel-task",
            ],
            "task.cancel",
            json!({"id": TASK_ID, "expectedRevision": 3, "actor": {"kind": "user"}, "evidence": {"reason": "stop"}, "idempotencyKey": "cancel-task"}),
        ),
        (
            vec![
                "lazooctl",
                "task",
                "resolve",
                TASK_ID,
                "--expected-revision",
                "4",
                "--resolution",
                "completed",
                "--evidence",
                "{\"verified\":true}",
                "--idempotency-key",
                "resolve-task",
            ],
            "task.resolve",
            json!({"id": TASK_ID, "expectedRevision": 4, "resolution": "completed", "evidence": {"verified": true}, "failureCode": null, "failureMessage": null, "idempotencyKey": "resolve-task"}),
        ),
        (
            vec![
                "lazooctl",
                "task",
                "list",
                "--after-sequence",
                "5",
                "--limit",
                "20",
                "--root-task-id",
                ROOT_TASK_ID,
                "--conversation-id",
                CONVERSATION_ID,
                "--assigned-employee-id",
                RECIPIENT_ID,
                "--state",
                "running",
            ],
            "task.list",
            json!({"afterSequence": 5, "limit": 20, "rootTaskId": ROOT_TASK_ID, "parentTaskId": null, "conversationId": CONVERSATION_ID, "projectId": null, "assignedEmployeeId": RECIPIENT_ID, "state": "running"}),
        ),
        (
            vec!["lazooctl", "task", "show", TASK_ID],
            "task.show",
            json!({"id": TASK_ID}),
        ),
    ];
    for (arguments, method, expected) in cases {
        let cli = Cli::try_parse_from(arguments).unwrap();
        assert!(matches!(cli.command, Command::Task(_)));
        assert_eq!(command_request(&cli.command), (method, expected, false));
    }

    let turn = Cli::try_parse_from([
        "lazooctl",
        "turn",
        "submit",
        CONVERSATION_ID,
        "--task-id",
        TASK_ID,
        "--authority-mode",
        "self-decision",
        "--text",
        "continue",
    ])
    .unwrap();
    let params = command_request(&turn.command).1;
    assert_eq!(params["taskId"], TASK_ID);
    assert_eq!(params["authorityMode"], "self-decision");

    let default_turn = Cli::try_parse_from([
        "lazooctl",
        "turn",
        "submit",
        CONVERSATION_ID,
        "--text",
        "begin",
    ])
    .unwrap();
    assert_eq!(
        command_request(&default_turn.command).1["authorityMode"],
        "ask"
    );
}

#[test]
fn task_errors_use_exit_code_thirteen() {
    for code in [
        "LZ-TASK-NOT-FOUND",
        "LZ-TASK-STALE",
        "LZ-TASK-STATE",
        "LZ-TASK-AUTHORITY-UNSUPPORTED",
        "LZ-TASK-INVALID",
    ] {
        assert_eq!(
            exit_code_for_rpc(&RpcError {
                code: code.to_owned(),
                message: "failure".to_owned(),
                data: None,
            }),
            13
        );
    }
}
