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

/// The stress harness keeps the stderr of exit-0 calls, because a governor initialization-lock
/// timeout degrades a call to un-enrolled with exit 0 and only a stderr line, and it classifies a
/// client deadline by the LZ-IPC phase message rather than by any `timed out` text, which the
/// governor's non-fatal line would also match.
#[test]
fn cli_stress_harness_keeps_exit_zero_stderr_and_classifies_ipc_deadlines_only() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = std::fs::read_to_string(repository.join("scripts/test-cli-stress.ps1"))
        .expect("CLI stress script");

    for required in [
        "stderrOnSuccess = @($results | Where-Object { $_.exitCode -eq 0 -and $_.stderr.Trim() -ne '' }",
        "-match 'IPC I/O failed: .*timed out'",
    ] {
        assert!(
            script.contains(required),
            "stress script must contain {required}"
        );
    }
    assert!(
        !script.contains("-match 'timed out'"),
        "stress script must not classify a deadline by bare `timed out` text"
    );
}
