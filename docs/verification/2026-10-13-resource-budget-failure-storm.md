# Resource budget failure storm verification

Date completed: 2026-10-13 IST

Branch: `test/resource-budget-failure-storm`

Base: `test/core-scenario-peer` (pull request #31, stacked on #30 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: a third window in `scripts/acceptance/resource-budget.ps1`, the failure storm the brief's section 20.2 says the 20 percent gate must be demonstrated under, measured against the real keeper and daemon the same way as the idle and routing windows. No production code changed.

## Outcome

PASS.

For one window the script drives the daemon's failure path as fast as the CLI can: each cycle sends a one-attempt message between two employees, claims it, and fails it, so every cycle dead-letters and raises an incident, which folds, is promoted by the section 14.2 threshold, escalates to the user (no lead), and is announced once by the daemon's timed sweep. The daemon's and keeper's CPU time over the window, as a share of the whole machine, must stay under the brief's 20 percent hard ceiling, and their working set under the 750 MiB active-core target. The manifest records the cycles driven, the dead letters and incidents the ledger holds afterwards, the CPU share, and the memory.

The window exercises what exists today: message delivery, dead-lettering, incident folding and promotion, user escalation, and the notification sweep. Memory indexing, terminal streaming, and voice remain unmeasured until those features exist, and enforcement (section 20.2) is still not part of this gate.

The first standalone run of the extended script on this branch failed in the unchanged routing window: `lazooctl project list` printed its result and then exited with status -1073740940 (0xC0000374, heap corruption) at process exit, once in roughly 2,200 calls; the re-run and the gate run passed, and the observation is recorded in NEXT-SESSION for the owner. No live model turn was run. One Windows notification is expected during the window: the storm's folded incident is announced once, as the user-notification rule says.

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `test/resource-budget-failure-storm` after the final change, through `scripts\gate.ps1`.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test --workspace --locked` | 0 | 414 passed, 0 failed, 11 ignored (explicit live-provider tests), 70 test binaries; the resource-budget contract test grew its required literals. |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.6 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| `scripts\acceptance\resource-budget.ps1 -SkipBuild` | 0 | PASS manifest: idle 0.0000 CPU share and 18 MiB; routing 2165 calls at 0.0035; failure storm 399 cycles at 0.0117 (ceiling 0.20) with 78 MiB, 399 dead letters, 1 incidents afterwards. |
| every other acceptance script | 0 | All 30 PASS manifests at schema 15; `gate-manifest.json` records all 35 steps PASS. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Boundary evidence

- No production code changed: the diff is the acceptance script, its contract test's literals, and documentation.
- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- Nothing is enforced: the window measures and asserts; no limit, throttle, or CPU-rate control is set.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.
