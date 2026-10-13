# Gate runner verification

Date completed: 2026-10-13 IST

Branch: `chore/gate-runner`

Base: `test/core-scenario-dirty-work` (pull request #28, stacked on #27 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: `scripts/gate.ps1`, the repository's full gate as one command, with a daemon contract test that pins what it runs and excludes, and continuity documentation. No production code changed.

## Outcome

PASS.

Every backend slice since the first has been gated by the same sequence, run by hand or from an ad-hoc runner: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`, `cargo build --workspace --release --locked`, then every acceptance script except `provider-adapters.ps1` (a live model turn, run only when explicitly authorized) and `desktop-shell.ps1` (the separately owned desktop), the real-binary ones with `-SkipBuild` and `native-session-ownership.ps1` with `-Provider codex`. The runner now holds that sequence: a cargo failure stops it, an acceptance failure is recorded and the run continues, and `artifacts/acceptance/gate-manifest.json` records every step's command, exit code, and duration. `-AcceptanceOnly`, `-Only`, and `-Skip` narrow a run; the real-binary scripts are recognized by their `[switch]$SkipBuild` parameter, so a new script joins the gate by existing. The first run of the runner on this branch surfaced its own defect, fixed before the run recorded here: a step's output was returned from the helper function along with its boolean, which hid the cargo test output and would have let a failing test step continue; it is now shown on the host and the boolean stands alone. The contract test `gate_runner_runs_the_whole_gate_and_excludes_only_the_live_and_ui_scripts` pins the four cargo commands, the two exclusions, the switches, and the manifest, and that no live model turn is ever part of it.

No live model turn was run.

## Fresh verification matrix

The gate on this branch was run by the runner itself: `pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1` from `C:\workspace\Orchestrator` after the final change, exit 0, `artifacts/acceptance/gate-manifest.json` status `PASS` with 35 steps.

| Step | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test --workspace --locked` | 0 | 414 passed, 0 failed, 11 ignored (explicit live-provider tests), 70 test binaries (59 test targets and 10 doc-test runs, as every gate today). New: `lazood` 1 (the gate runner contract). |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.6 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| every acceptance script except `provider-adapters` and `desktop-shell` | 0 | 31 PASS manifests at schema 15, run by the runner with `-SkipBuild` where accepted and `-Provider codex` for `native-session-ownership`. |
| `git diff --check` | 0 | No whitespace errors. |

## Boundary evidence

- No production code changed: the diff is one script, one contract test, the gate manifest, and documentation.
- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- The runner never runs `provider-adapters.ps1` or `desktop-shell.ps1`; a live model turn stays an explicit, separate command.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.
