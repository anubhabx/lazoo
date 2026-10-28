# Core scenario project verification (steps 3 and 6)

Date completed: 2026-10-13 IST

Branch: `test/core-scenario-project`

Base: `test/core-scenario-evidence` (pull request #26, stacked on #25 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: the core scenario story (`the_core_scenario_runs_end_to_end_on_the_deterministic_provider`) now runs its conversation in a registered project, as the brief's step 3 says, so every dispatched task activates inside its own retained git worktree, as step 6 says. The acceptance script's step map and the continuity documentation follow. No production code changed.

## Outcome

PASS; step 3 is no longer "PASS, projectless".

The story registers a temporary git repository with a base commit through `project.register`, creates the conversation in it through `conversation.create`, and runs every later step there. Step 6 now asserts that each accepted task's dispatch names the project, a `git-worktree` workspace with a branch and base commit, a directory that holds the repository's file and is never the canonical checkout, one worktree per task, and that each worker session started inside its task's worktree (the deterministic backend records its working directories). The stalled third task (step 9), the restart (step 11), and the idle suspension (step 14) run unchanged over the project conversation. The dirty-checkout proof (step 13) stays where it is. One test helper changed with it: the simulated recurrence that closes the remediation window now carries the task's own project, because incidents fold per code, employee, and project and a projectless recurrence would have opened a second incident instead of recurring the first.

No live model turn was run.

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `test/core-scenario-project` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test --workspace --locked` | 0 | One invocation, 413 passed, 0 failed, 11 ignored (explicit live-provider tests), 70 test binaries; no new test, the story grew. |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.7 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| `scripts\acceptance\core-scenario.ps1` | 0 | PASS schema-15 manifest; `steps.step3` no longer notes projectless, `steps.step6` names the worktrees. |
| every other acceptance script (deterministic ones; the real-binary ones with `-SkipBuild`; `resource-budget -SkipBuild`; `native-session-ownership -SkipBuild -Provider codex`) | 0 | All 30 PASS manifests at schema 15. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Boundary evidence

- No production code changed: the diff is the daemon story, the acceptance script's step map, and documentation.
- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.
