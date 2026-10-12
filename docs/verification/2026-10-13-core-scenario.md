# Core scenario verification

Date completed: 2026-10-13 IST

Branch: `feat/core-scenario`

Base: `feat/provider-config-discovery` (pull request #16, stacked on #15 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: one deterministic story test for the brief's section 21.2 core end-to-end scenario, the acceptance gate that maps every one of its fifteen steps to evidence, and continuity documentation. No production code changed.

## Outcome

PASS, with the map below.

Core v1 (brief section 24) requires that "the end-to-end acceptance scenario passes". Section 21.2 lists fifteen steps. `scripts/acceptance/core-scenario.ps1` now runs one daemon story that walks ten of them in a single daemon lifetime on the deterministic provider, with a restart in the middle, and names for every other step the acceptance script that already proves it, or says that it is partial or not covered. The manifest carries that map, step by step, so the claim is exact.

| Step | Status | Evidence |
|---|---|---|
| 1. Rhea leads; four peers across providers | PASS | the story: Mira (Codex), Oren (OpenCode), Pia (Grok Build), Quinn (Antigravity) |
| 2. All dormant; one disabled | PASS | `roster.get` before any session: four `dormant`, Quinn `disabled` |
| 3. The user starts a project conversation | PASS, projectless | the story runs projectless; the project-scoped conversation and worktree are `conversation-delivery` and `task-delegation-graph` |
| 4. Rhea reads the roster and proposes a plan | PASS | `employees_list` then two `task_delegate` calls, and a third in a follow-up turn under a new root once the first plan finished |
| 5. Ask mode presents precise delegation approvals | PASS | both children `proposed`, counted by the Inbox, accepted by the user; provider permission approvals are `approval-gate` |
| 6. Approved employees activate into independent sessions and workspaces | PASS | each accepted task runs on a fresh session for Mira in its own projectless workspace, the session stops with its turn, the next queued task wakes when she is free; Git worktrees are `automatic-task-dispatch` |
| 7. A peer asks a direct clarification | PROVEN ELSEWHERE | `peer-messaging`, `coalesced-notices` |
| 8. The disabled employee cannot receive dispatch | PASS | `task_delegate` to Quinn is `LZ-TASK-ASSIGNEE-UNAVAILABLE` and writes nothing |
| 9. A provider stalls; Lazoo detects, retries safely, raises a clean incident | PASS (since `feat/core-scenario-stall`, pull request #21; PARTIAL before) | the third task's dispatch turn stalls on the deterministic provider (the adapters' progress deadline, proven on the real adapter by `opencode_acceptance_without_progress_becomes_stalled`, reported as a stalled provider error), the turn is recorded `retryable-failed`, the task stays `accepted` for a bounded retry by the lead or the user and is never retried automatically, the worker session is stopped, and one `LZ-SESSION-STALLED` incident on the task is raised and promoted at once; see `docs/verification/2026-10-13-core-scenario-stall.md` |
| 10. Threshold, bounded remediation, user escalation | PASS | lead window on the queued task, one `cancel-task` remedy in the notice turn, recurrence, `recurred-after-remedy` escalation on the sweep, Inbox acknowledgement |
| 11. Restart during in-flight work reconciles without loss or duplication | PASS, restart | a second daemon boot over the same store: the prior boot's sessions are owner-lost and never adopted, every task is still there exactly once, the acknowledgement survives; the in-flight attempt itself is `message_restart_reconciles_an_uncertain_attempt` (`collaboration-bus`) and `native-session-ownership` |
| 12. Results include commits, verification, task links, messages, incidents, memory records | NOT COVERED | task links, messages, and incidents are `result-delivery`; commits and verification evidence need the worker to report them; memory records need Stage 5 |
| 13. Integration follows authority and never overwrites dirty work | PROVEN ELSEWHERE | `project_task_dispatch_uses_an_isolated_worktree_and_preserves_dirty_user_files` (`automatic-task-dispatch`), run again by this gate |
| 14. Employees return to idle and suspend after the policy timeout | PASS | on the restarted daemon Mira's timeout is one minute; her warmed session is still idle at 59 s and suspended at 61 s; the roster says `suspended` |
| 15. Lazoo stays inside its resource budget | PROVEN ELSEWHERE | `resource-budget` (idle and routing windows) |

No live model turn was run; the live scenario across real providers remains a separately authorized gate.

## Design notes

| Decision | Reason |
|---|---|
| One story test rather than a script that chains the other scripts | The scenario's point is that the pieces compose in one daemon lifetime: the same store, the same sessions, a restart in the middle. Chaining scripts would prove nothing new. |
| The step map lives in the manifest | A "PASS" for the scenario would overclaim; the manifest says per step whether this run proved it, another gate does, or nothing does yet. |
| The story runs projectless and raises the stall directly | Worktree creation and stall detection have their own gates; repeating their Git and timeout choreography here would make the story slow and flaky without proving more. |
| The third task is delegated in a follow-up turn under a new root | A finished plan is not reopened; the follow-up proves the user can keep working with the same lead while the first plan's results are already in. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/core-scenario` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean with the one story-test allowance. |
| `cargo test --workspace --locked` | 0 | One invocation, 393 passed, 0 failed, 11 ignored (explicit live-provider tests), 68 test binaries. New: `lazood` 2 (the scenario story and the acceptance-script contract; 109 daemon tests). |
| `cargo build --workspace --release --locked` | 0 | Locked release build (unchanged binaries). |
| `scripts\acceptance\core-scenario.ps1` | 0 | PASS schema-15 manifest, `deterministic-fake-transport`, 5 commands, the step map above. |
| every other acceptance script (deterministic ones; the real-binary ones with `-SkipBuild`; `resource-budget -SkipBuild`; `native-session-ownership -SkipBuild -Provider codex`) | 0 | PASS manifests at schema 15. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No daemon method, CLI command, migration, event, or tool changed; the story drives the daemon through its public RPCs and the fake provider.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Step 12's commit and verification evidence and memory records (Stage 5), the live scenario across real providers, and everything else NEXT-SESSION lists remain separate reviewed slices.
