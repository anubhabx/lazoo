# Core scenario stall verification (step 9)

Date completed: 2026-10-13 IST

Branch: `feat/core-scenario-stall`

Base: `feat/workspace-actions` (pull request #20, stacked on #19 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: the core scenario story (`the_core_scenario_runs_end_to_end_on_the_deterministic_provider`) now runs the brief's step 9 for real instead of raising the stall by hand: the deterministic provider stalls the third task's dispatch turn, and the daemon's own path records, bounds, and reports it. The acceptance script's step map and the continuity documentation follow. No production code changed.

## Outcome

PASS.

Section 21.2's step 9 reads "one provider stalls after accepting work; Lazoo detects missing progress, retries safely, then raises a clean incident". The story previously raised `LZ-SESSION-STALLED` through the store as the daemon would and called the step partial. The deterministic session backend now has a `turn_stall` switch that makes a turn return the adapters' stalled error (`ProviderError::Stalled`, what every adapter returns when its progress deadline passes, proven on the real OpenCode adapter by `opencode_acceptance_without_progress_becomes_stalled`). With it set for the third task's dispatch, the story proves what Lazoo does with a stall: the dispatch returns `LZ-TURN-PROVIDER`, the dispatch turn is `retryable-failed` with that failure, the task stays `accepted` so the lead or the user can retry it within bounds (the daemon never retries a dispatch automatically), the worker's session is stopped, and exactly one `LZ-SESSION-STALLED` incident is raised on the task for the worker with one occurrence, born `priority` and escalated to the lead at once (section 14.2). Step 10 then continues from that real incident: the lead's notice turn, one remedy, the simulated recurrence, and the user escalation are unchanged.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| The stall is injected at the adapter boundary, not inside an adapter | The story runs on the deterministic backend; the adapters' progress deadlines are unit-tested where they live. The daemon's behaviour from the stalled error onward is what the scenario proves. |
| The recurrence stays simulated | A cancelled task cannot be dispatched again, so the second stall that closes the remediation window is still raised through the store as the daemon would. |
| "Retries safely" is a bounded, never automatic retry | Guardrail: the daemon never retries an uncertain or failed dispatch on its own; the retryable-failed turn and the accepted task are left for the lead's remedy or the user's `turn.retry`. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/core-scenario-stall` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test --workspace --locked` | 0 | One invocation, 407 passed, 0 failed, 11 ignored (explicit live-provider tests), 69 test binaries; no new test, the story grew. |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.7 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| `scripts\acceptance\core-scenario.ps1` | 0 | PASS schema-15 manifest whose `steps.step9.status` is now `PASS`. |
| every other acceptance script (deterministic ones; the real-binary ones with `-SkipBuild`; `resource-budget -SkipBuild`; `native-session-ownership -SkipBuild -Provider codex`) | 0 | All 29 PASS manifests at schema 15. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Boundary evidence

- No production code changed: the diff is the daemon test suite's deterministic backend and story, the acceptance script's step map, and documentation.
- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Step 12 (commit and verification evidence reported by the worker, and memory records once Stage 5 exists) and the live scenario across real providers remain separate slices.
