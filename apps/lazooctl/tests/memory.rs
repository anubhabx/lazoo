use clap::Parser;
use lazooctl::{Cli, command_request};

#[test]
fn memory_injection_can_be_disabled_explicitly() {
    let cli = Cli::try_parse_from([
        "lazooctl",
        "memory",
        "policy-set",
        "--global",
        "--expected-revision",
        "1",
        "--inject-enabled",
        "false",
    ])
    .unwrap();
    let (method, params, _) = command_request(&cli.command);
    assert_eq!(method, "memory.policy.set");
    assert_eq!(params["injectEnabled"], false);
}
