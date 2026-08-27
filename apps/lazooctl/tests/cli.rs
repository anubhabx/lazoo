use std::path::PathBuf;

use clap::Parser;
use lazoo_protocol::{CheckoutMode, RpcError};
use lazooctl::{Cli, Command, ProjectCommand, exit_code_for_rpc, render_result};
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
