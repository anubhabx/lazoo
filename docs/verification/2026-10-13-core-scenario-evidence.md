# Core scenario evidence verification (step 12)

Date completed: 2026-10-13 IST

Branch: `test/core-scenario-evidence`

Base: `test/read-model-resume` (pull request #25, stacked on #24 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: the core scenario story (`the_core_scenario_runs_end_to_end_on_the_deterministic_provider`) now covers the part of the brief's step 12 that the backend can hold today: the first worker reports its completion through the `task_report` tool with commit and verification evidence, and the story proves that evidence is durable, attributed, and readable. The acceptance script's step map and the continuity documentation follow. No production code changed.

## Outcome

PASS; step 12 moves from NOT COVERED to PARTIAL.

Section 21.2's step 12 reads "results include commits, verification, task links, messages, incidents, and concise memory records". Task links, messages, and incidents were already proven by the result-delivery and minimum-incidents gates; commits and verification needed a worker to report them, and memory records need Stage 5. The deterministic worker session for the first child now issues `task_report { kind: completed, evidence: { commits, verification } }` before its native turn ends (the tool report outranks native completion, as the collaboration-tools gate proves). The story asserts the task's `completed` evidence row carries the commit sha and the verification exit code, names the worker's own dispatch session, and is returned by `task.show`; both results still reach Rhea once each, so the report did not duplicate the result notice. Memory records remain outside the story until Stage 5 exists.

No live model turn was run.

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `test/core-scenario-evidence` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test --workspace --locked` | 0 | One invocation, 413 passed, 0 failed, 11 ignored (explicit live-provider tests), 70 test binaries; no new test, the story grew. |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.7 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| `scripts\acceptance\core-scenario.ps1` | 0 | PASS schema-15 manifest whose `steps.step12.status` is now `PARTIAL` with the evidence above. |
| every other acceptance script (deterministic ones; the real-binary ones with `-SkipBuild`; `resource-budget -SkipBuild`; `native-session-ownership -SkipBuild -Provider codex`) | 0 | All 30 PASS manifests at schema 15. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Boundary evidence

- No production code changed: the diff is the daemon story, the acceptance script's step map, and documentation.
- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Memory records (Stage 5) and the live scenario across real providers remain separate slices; the shape of commit and verification evidence stays free-form until the owner defines a reporting contract for workers.
