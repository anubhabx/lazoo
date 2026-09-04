# Durable task/delegation graph verification

Date completed: 2026-09-05 IST

Branch: `codex/task-delegation-graph`

Base: `main` at `f9b0364`

Product version: `0.1.0`

Database schema: 6
Scope: backend/domain/protocol/store/provider/daemon/CLI, acceptance, and continuity documentation only

## Outcome

PASS. The approved durable task/delegation graph is implemented and verified. New user turns create one root task atomically, child delegation is bounded and authority-limited, task lifecycle truth is evidence-backed, prior-boot execution becomes uncertain, and all eight task RPC/CLI operations are available. The desktop source, Tauri command surface, `desktop.snapshot`, and capabilities remain unchanged.

Accepted child tasks are queued control-plane truth. Automatic worker dispatch, project/worktree isolation, ConPTY, terminal I/O, general approvals, managed memory, scheduling, resource governance, and other-provider conversation delivery are not implemented by this slice.

## TDD evidence

The implementation was developed with focused red/green cycles before broad verification:

| Contract | RED evidence | GREEN evidence |
|---|---|---|
| Executable release acceptance | `cargo test -p lazood --test daemon task_graph_acceptance_executes_backend_only_contract` exited 1 because `scripts/acceptance/task-delegation-graph.ps1` did not exist. | The same test later exited 0, executed the release harness, parsed its BOM-less manifest, and asserted every required evidence field. |
| Running-parent cancellation | The focused store test expected `TaskActiveChildren` but received `Ok(... state: CancelRequested ...)`. | `cargo test -p lazoo-store --test conversation task_parent_completion_waits_for_an_active_child_with_an_exact_linked_turn` exited 0 after the running branch gained the same descendant guard as accepted/proposed cancellation. |
| Self-delegation error family | The daemon workflow expected `LZ-TASK-SELF-DELEGATION` but received `LZ-TASK-STORE`. | `cargo test -p lazood --test daemon task_rpc_workflow_is_durable_idempotent_ordered_and_provider_neutral` exited 0 after the distinct mapping was added. |
| Root active-turn ownership and follow-up | The focused daemon follow-up test initially surfaced generic `LZ-TURN-STORE` behavior while a second recorded turn tried to overtake the accepted root's reserved creation turn. | The root keeps its creation-turn reservation, the stable delivery guard rejects overtaking as `LZ-TURN-TASK-SCOPE`, and completed turns release ownership. Both focused tests exited 0; the acceptance harness then proved an executable follow-up can reuse the root after the creation turn completes while an active child keeps the parent nonterminal. |
| Uncertain-task replay | `cargo test -p lazood --test daemon turn_submit_replay_blocks_an_uncertain_task_before_provider_io` exited 1 because the recorded follow-up reached the provider after its task became `uncertain`. | The same focused test exited 0 after delivery eligibility was restricted to `accepted` or `running`; the replay returned `LZ-TURN-TASK-SCOPE` with zero provider calls. |
| Existing conversation acceptance | The schema-6 conversation harness exited 1 with `LZ-TURN-STORE` for an employee turn that lacked the now-required task link. | The harness exited 0 after creating and accepting an ask-mode child assigned to the employee, then submitting the employee-authored turn against that exact child task. |

The acceptance harness itself also exposed and corrected Windows-specific verifier behavior: multiple `cargo.exe` command discoveries are reduced to the first application, and the manifest is written as UTF-8 without a BOM for strict Rust JSON parsing.

## Fresh verification matrix

The following commands were run in this order from `C:\workspace\Orchestrator\.worktrees\task-delegation-graph` after the final code fixes:

| Command | Exit | Evidence |
|---|---:|---|
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-rust-toolchain.ps1` | 0 | Cargo 1.98.0 at `C:\Users\anubhab\.cargo\bin\cargo.exe`; MSVC toolchain verification PASS. |
| `cargo test --workspace` | 0 | 234 passed, 0 failed, 11 ignored. The ignored tests are explicit live provider gates. |
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | Finished successfully with warnings denied. |
| `cargo check --workspace --all-targets` | 0 | All targets checked successfully. |
| `cargo build --workspace --release --locked` | 0 | Locked release build finished successfully. |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\first-slice.ps1 -SkipBuild` | 0 | PASS manifest. |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\employee-roster.ps1 -SkipBuild` | 0 | PASS manifest. |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\native-session-ownership.ps1 -SkipBuild` | 0 | PASS manifest. |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\collaboration-bus.ps1 -SkipBuild` | 0 | PASS manifest. |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\conversation-delivery.ps1 -SkipBuild` | 0 | PASS manifest. |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\task-delegation-graph.ps1 -SkipBuild` | 0 | PASS manifest. |

Workspace-test accounting:

- passed: 234;
- failed: 0;
- ignored: 11;
- daemon integration tests: 47 passed;
- store task tests: 13 passed;
- store conversation tests: 10 passed;
- CLI tests: 16 passed; and
- protocol task tests: 4 passed.

## Acceptance manifests

| Manifest | Status | Schema evidence | Duration | Commands | Expected nonzero commands |
|---|---|---:|---:|---:|---:|
| `first-slice-manifest.json` | PASS | database schema 6 | 10,544 ms | 14 | 0 |
| `employee-roster-manifest.json` | PASS | schema 6 | 7,648 ms | 17 | 1 |
| `native-session-ownership-manifest.json` | PASS | schema 6 | 14,483 ms | 18 | 1 |
| `collaboration-bus-manifest.json` | PASS | schema 6 | 6,980 ms | 26 | 2 |
| `conversation-delivery-manifest.json` | PASS | schema 6 | 24,937 ms | 38 | 3 |
| `task-delegation-graph-manifest.json` | PASS | schema 6 | 11,255 ms | 81 | 16 |

Nonzero commands are required negative-path assertions with the expected CLI exit family; every manifest status is literal `PASS`. The task manifest's 19 boolean evidence fields are all `true`:

- root identity, replay without duplication, and existing-root follow-up linkage;
- ask proposal without a message and user acceptance with one canonical delegation message;
- authority narrowing plus stale roster/configuration, unavailable assignee, self-delegation, depth-5, and ninth-child guards;
- native evidence required, ordered progress/terminal evidence, and active-descendant blocking;
- owner-loss uncertainty, append-only idempotent resolution, and restart correlations; and
- provider smoke, external model use, worktree mutation, ConPTY, desktop expansion, and UI changes absent.

## Boundary verification

The prescribed boundary commands produced:

- `git diff --check main...HEAD`: exit 0;
- `git diff --check`: exit 0;
- `git diff --quiet main...HEAD -- apps/lazoo-desktop`: exit 0;
- `git diff --quiet -- apps/lazoo-desktop`: exit 0; and
- `git status --short -- apps/lazoo-desktop`: empty.

The task acceptance script's only matches for the exclusion scan are assertions that no `provider smoke` command ran and read-only Git checks of `apps/lazoo-desktop`. It contains no provider-smoke invocation, live model selection, worktree mutation, ConPTY operation, desktop mutation, or UI operation. Building/testing the workspace compiled the existing desktop crate but did not modify it.

## Durable behavior proved

- Schema 6 upgrades schemas 1 through 5 without changing earlier migration checksums; schema-5 turns backfill conservative root-task identity and state.
- A new user turn, canonical message, root task, initial accepted evidence, audit event, outbox row, and command outcome are atomic. Root task ID equals creation-turn ID.
- Existing-task turns retain their own turn identity but point at the existing task. Delivery is allowed only while the task is `accepted` or `running` and no different turn owns execution; terminal, cancel-requested, uncertain, or concurrently active tasks reject it before provider I/O.
- The graph is immutable, depth-bounded to four delegation edges, and limited to eight nonterminal direct children.
- `ask` delegation creates a proposed task only. User acceptance atomically creates exactly one delegation message. Non-ask delegation creates accepted task/message state without expanding authority.
- Task actors and active sessions are revision/scope checked. Stable task failures use CLI exit 13, including a distinct self-delegation family.
- A new root reserves its creation turn to prevent delivery overtaking, but that reservation alone does not make the task `running`. Accepted delegated children remain queued without active execution. Native acceptance or progress evidence is required before `running`, completed turns release ownership, and progress/outcomes are ordered and revisioned.
- Parent completion and cancellation are rejected while any descendant is nonterminal.
- Task authority narrows Codex per-turn permissions below a broader session default. Task-backed delivery through an adapter that cannot honor the ceiling fails closed.
- Startup reconciliation follows sessions, messages, turns, then tasks; it never adopts a prior PID, native session, or native turn.
- User resolution is the only exit from `uncertain`, and idempotent replay adds no evidence, audit event, or outbox record.

## Explicitly absent or deferred

- No provider smoke was run.
- No live model turn was run.
- No `apps/lazoo-desktop` source, Tauri command, capability, or snapshot field changed.
- No automatic accepted-task dispatch exists.
- No project/worktree creation, ownership, cleanup, or integration path exists.
- No ConPTY or terminal-I/O path exists.
- No generic approvals, managed memory, scheduling, resource governor, or other-provider turn delivery was added.
- `UI BACKEND CONTRACT: GO` is intentionally withheld.

## Next handoff

The next reviewed backend slice should design automatic accepted-task dispatch together with project/worktree isolation. It must preserve canonical-message idempotency, roster/configuration freshness, task authority, native acceptance before `running`, restart uncertainty, dirty-user-state safety, and explicit retention/recovery. Only after that contract is stable should a typed desktop task binding be proposed.
