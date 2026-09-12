# Automatic task dispatch verification

Date completed: 2026-09-12 IST

Branch: `codex/automatic-task-dispatch`

Base: `main` at `218282855200533b573bc7ed16c9b26d32bb2b7b`

Product version: `0.1.0`

Database schema: 7

Scope: backend protocol, runtime, store, daemon, acceptance harnesses, and continuity documentation. Desktop product code and contract are unchanged.

## Outcome

PASS. Accepted Codex child tasks are now selected and dispatched automatically when the assigned employee has no active session. Dispatch is record-first, reuses the canonical delegation message, preserves task authority, and runs in a deterministic private projectless directory or retained isolated Git worktree. Native evidence is still required before a task becomes `running`.

Pre-provider configuration/workspace failures fail task, message, and dispatch together. Concurrent employee sessions defer rather than fail a claimed task. Prior-boot pre-provider claims fail closed, while possibly in-flight turns become `uncertain` and are never retried automatically. Cancellation is deterministic at the provider-I/O boundary: pre-I/O cancellation invokes no provider turn; after provider entry it waits for native evidence, and an already-arriving native completion wins.

The implementation is Codex-only. No live model turn or provider smoke was run for this slice. Worktrees are retained for audit; automatic merge, cleanup, deletion, and branch rewriting are absent.

## TDD and review evidence

Focused red/green cycles established the race behavior before the broad gate:

| Contract | RED evidence | GREEN evidence |
|---|---|---|
| Submitting-window cancellation | The focused daemon test failed with provider cancellation count `1` instead of `2`, proving the task could terminalize without cancelling an in-flight submission. | The state machine now distinguishes pre-I/O cancellation from entered provider I/O. A gated regression cancels after durable `submitting` but before provider entry and asserts the provider turn count does not increase. |
| Concurrent employee session | The focused store test failed to compile because `DeferTaskDispatchInput` and `defer_task_dispatch` did not exist. | The store now removes only the owned pre-provider claim, preserves accepted task/message truth, records `task.dispatch-deferred`, excludes the task while the competing session is active, and makes it pending after that session ends. |
| Completion versus cancellation | Review found that `cancel-requested + completed` had no task projection. | The projection now makes completion terminal when native completion arrives before cancellation evidence; the daemon regression exercises that order. |
| Legacy conversation delivery | The executable acceptance failed with `LZ-TURN-TASK-SCOPE` because automatic dispatch completed a child before the harness's manual employee turn. | The harness now warms the manually controlled employee before accepting the child, preserving the original employee-authorship contract while correctly excluding that active session from auto-dispatch. |

Two read-only review passes were performed. The first identified cancellation, session-race, and freshness concerns. The implementation added deterministic race coverage and documented why active sessions are ineligible for task-specific workspace dispatch. The final focused review reported no actionable correctness, security, or regression issues.

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator\.worktrees\automatic-task-dispatch` after the final fixes.

| Command | Exit | Evidence |
|---|---:|---|
| `scripts\verify-rust-toolchain.ps1` | 0 | Cargo 1.98.0 MSVC toolchain verification passed. |
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | All workspace targets linted with warnings denied. |
| `cargo check --workspace --all-targets --locked` | 0 | All workspace targets checked. |
| `cargo test --workspace --locked` | 0 | Entire Rust workspace passed; 51 daemon integration tests and 16 store task tests passed. Explicit live-provider tests remained ignored. |
| `cargo build --workspace --release --locked` | 0 | Locked release build passed. |
| `npm --prefix apps/lazoo-desktop/ui run check` | 0 | Prettier, TypeScript, 47 Vitest tests, and the Vite production build passed. |
| `scripts\acceptance\first-slice.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\employee-roster.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest; non-model session contract only. |
| `scripts\acceptance\collaboration-bus.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\conversation-delivery.ps1 -SkipBuild` | 0 | PASS deterministic manifest. |
| `scripts\acceptance\task-delegation-graph.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\automatic-task-dispatch.ps1` | 0 | PASS schema-7 fake-transport manifest. |

## Automatic-dispatch acceptance evidence

`artifacts/acceptance/automatic-task-dispatch-manifest.json` records literal `PASS`, schema 7, and `deterministic-fake-transport`. Its focused commands prove:

- background wake-up after an assigned employee's active session stops;
- one canonical delegation message and correlation identity reused by the worker turn;
- owner-protected projectless execution;
- deterministic Git branch/worktree creation without modifying dirty canonical files;
- employee configuration drift and sibling failure isolation;
- concurrent-session deferral and retry eligibility;
- pre-provider owner-loss failure and in-flight restart uncertainty;
- cancellation before provider entry, after native acceptance, and against an arriving completion;
- provider session stop on success, cancellation, and failure; and
- no provider smoke, external model use, or desktop UI change.

## Boundary evidence

- `git diff --check` passed.
- Both staged and unstaged diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, or frontend source changed.
- No live provider/model gate was added to the automatic-dispatch acceptance.
- No push or pull request was created.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

ConPTY/native terminal attachment, approvals and policy enforcement, managed memory, scheduling, resource governance, other-provider task delivery, typed desktop task binding, and task-worktree cleanup remain separate reviewed slices.
