# Core scenario dirty work verification (step 13)

Date completed: 2026-10-13 IST

Branch: `test/core-scenario-dirty-work`

Base: `test/core-scenario-project` (pull request #27, stacked on #26 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: the core scenario story (`the_core_scenario_runs_end_to_end_on_the_deterministic_provider`) now proves the brief's step 13 itself: integration follows the configured authority and never overwrites dirty user work. The acceptance script's step map and the continuity documentation follow. No production code changed.

## Outcome

PASS; step 13 moves from PROVEN ELSEWHERE to PASS inside the story (the running-provider-turn proof in `automatic-task-dispatch` stays as its second witness).

Before any worker starts, the story writes an uncommitted edit into the project's canonical checkout. After both dispatches it asserts that every task ran under the `ask` authority the user configured, that each task's worktree started from the committed base (the file holds the committed content, not the edit), and that the canonical checkout still holds the user's edit byte for byte. Nothing in the story, the dispatch, or the result path touched the user's work.

No live model turn was run.

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `test/core-scenario-dirty-work` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test --workspace --locked` | 0 | One invocation, 413 passed, 0 failed, 11 ignored (explicit live-provider tests), 70 test binaries; no new test, the story grew. |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.6 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| `scripts\acceptance\core-scenario.ps1` | 0 | PASS schema-15 manifest; `steps.step13.status` is `PASS` after the story. |
| every other acceptance script (deterministic ones; the real-binary ones with `-SkipBuild`; `resource-budget -SkipBuild`; `native-session-ownership -SkipBuild -Provider codex`) | 0 | All 30 PASS manifests at schema 15. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Boundary evidence

- No production code changed: the diff is the daemon story, the acceptance script's step map, and documentation.
- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Steps 7 and 15 stay proven by their own gates (a peer exchange needs two Codex workers in one tree, which the scenario's cast of one Codex worker and three other-provider peers does not have until other-provider dispatch exists; the resource budget is measured against real binaries). Step 12's memory records need Stage 5.
