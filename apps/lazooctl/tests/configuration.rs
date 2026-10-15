use clap::Parser;
use lazooctl::{Cli, command_request};
use serde_json::json;

#[test]
fn selected_configuration_requests_are_server_owned_and_never_accept_receipts() {
    let cli = Cli::try_parse_from([
        "lazooctl",
        "provider",
        "config-preview",
        "--selection",
        r#"{"provider":"codex","name":"search","command":"npx","args":["-y"],"url":null,"enabled":true,"replace":false,"selectedSecretFields":[]}"#,
    ])
    .unwrap();
    let (method, params, stream) = command_request(&cli.command);
    assert_eq!(method, "provider.config.select.preview");
    assert!(!stream);
    assert_eq!(
        params,
        json!({"selection":{"provider":"codex","name":"search","command":"npx","args":["-y"],"url":null,"enabled":true,"replace":false,"selectedSecretFields":[]}})
    );
    assert!(
        Cli::try_parse_from([
            "lazooctl",
            "provider",
            "config-apply",
            "00000000-0000-4000-8000-000000000001",
            "--expected-revision",
            "2",
            "--receipt",
            "forbidden"
        ])
        .is_err()
    );
}
