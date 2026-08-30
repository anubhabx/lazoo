use std::path::PathBuf;

use clap::Parser;
use lazoo_protocol::{CheckoutMode, RpcError};
use lazooctl::{
    Cli, Command, EmployeeCommand, ProjectCommand, RosterCommand, SessionCommand, command_request,
    exit_code_for_rpc, render_result,
};
use serde_json::json;

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
