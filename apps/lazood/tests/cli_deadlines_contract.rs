use std::path::Path;

/// The CLI deadlines acceptance script runs only the deterministic in-process and child-helper
/// proofs by exact name, never a provider, and records that it bounds and labels a stalled daemon
/// without claiming a cause for the stress timeout or the heap-corruption exit.
#[test]
fn cli_deadlines_acceptance_contract_is_deterministic_and_backend_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script_path = repository.join("scripts/acceptance/cli-deadlines.ps1");
    let script = std::fs::read_to_string(&script_path).expect("CLI deadlines acceptance script");

    for required in [
        "tests::stalled_daemon_response_times_out_cleanly",
        "tests::response_timeout_tiers_read_only_queries_short_and_everything_else_long",
        "initialization_lock_held_elsewhere_times_out_in_child_helper",
        "'--', '--exact'",
        "stalledDaemonResponseIsLabelledIpcTimeoutExitThree",
        "readOnlyQueriesShortTierUnderHarnessKill",
        "otherRpcsLongTierAboveProviderTurn",
        "governorInitLockTimesOutInsteadOfWaitingForever",
        "providerSmokeAbsent",
        "externalModelUseAbsent",
        "uiChangesAbsent",
        "diff --quiet -- apps/lazoo-desktop",
        "deterministic-fake-transport",
        "schemaVersion = 18",
        "eventsFollow = 'UNBOUNDED_BY_DESIGN'",
        "cliStressTimeoutCause = 'NOT_ESTABLISHED'",
        "heapCorruptionExit = 'NOT_FIXED_NO_FIRST_PARTY_DEFECT_FOUND'",
    ] {
        assert!(
            script.contains(required),
            "acceptance script must contain {required}"
        );
    }
    for forbidden in [
        "provider', 'smoke",
        "LivePrompt",
        "test-cli-stress",
        "--release",
    ] {
        assert!(
            !script.contains(forbidden),
            "acceptance script must not contain {forbidden}"
        );
    }
}
