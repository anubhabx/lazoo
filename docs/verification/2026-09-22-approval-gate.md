# Approval gate verification

Date completed: 2026-09-22 IST

Branch: `claude/approval-gate`

Base: `main` at `54d212f` (`feat: dispatch accepted tasks automatically`)

Product version: `0.1.0`

Database schema: 8

Scope: backend protocol, domain policy, provider contract, Codex adapter, store, daemon, CLI, acceptance harness, and continuity documentation. Desktop product code and contract are unchanged.

## Outcome

PASS. Codex command-execution and file-change approval requests are no longer answered with a hard-coded `decline`. Each request is recorded first in the schema-8 `approvals` ledger, evaluated against the linked task's effective authority, and answered `accept` only when policy (`self-decision` in scope, `full-autonomy`) or the local user (`ask`, or `self-decision` scope expansion) approves it. Decisions are revision-bound to one fingerprinted action; no session-wide or prefix grant exists.

Pending approvals expire after the daemon's fifteen-minute default, close as `cancelled` when their turn is cancelled or ends, and close as `expired` with `LZ-APPROVAL-OWNER-LOST` when the owning daemon boot is lost. Every closure answers the provider `decline`, and the provider is always answered before a decision failure is surfaced. The wait is excluded from the Codex turn budget, so a pending approval never trips the progress or turn timeout.

The implementation governs the daemon-managed Codex path only. ACP providers keep their existing fail-closed `session/request_permission` cancellation. No live model turn or provider smoke was run for this slice.

## Design notes

This slice landed design and implementation together under the owner's standing "look, understand, build" instruction for the session; `docs/superpowers/specs/2026-09-22-approval-gate-design.md` and `docs/superpowers/plans/2026-09-22-approval-gate.md` record the contract for owner review before merge. Two contracts were established red-first during implementation:

| Contract | RED evidence | GREEN evidence |
|---|---|---|
| Desktop snapshot stays free of approval fields | The first daemon test asserted the serialized snapshot contained no `approval` substring and failed, because recent audit events (`approval.requested`) already flow through the existing `recentEvents` field. | The assertion now checks that the snapshot key set is unchanged and that no `approvals`/`pendingApprovals` field exists, matching the actual contract: no new field, existing event stream untouched. |
| Schema advance is visible everywhere it is asserted | `status_reports_protocol_store_and_boot_identity`, the embedded `task_graph_acceptance_executes_backend_only_contract`, and four acceptance-script string contracts failed against schema 7 expectations. | Store tests, daemon contract tests, and every acceptance script now assert schema 8; all manifests were regenerated. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `claude/approval-gate` after the final fixes.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | All workspace targets linted with warnings denied. |
| `cargo check --workspace --all-targets` | 0 | All workspace targets checked. |
| `cargo test --workspace --locked` | 0 | Entire Rust workspace passed; 56 daemon integration tests (5 new approval tests), 4 new store approval tests, 5 new domain tests, 3 new protocol tests, 12 Codex adapter tests (2 new), and 18 CLI tests (2 new). Explicit live-provider tests remained ignored. |
| `cargo build --workspace --release --locked` | 0 | Locked release build passed (rebuilt by the embedded task-graph acceptance during the test run). |
| `npm --prefix apps/lazoo-desktop/ui run check` | 0 | Prettier, TypeScript, Vitest, and the Vite production build passed on the unchanged frontend. |
| `scripts\acceptance\approval-gate.ps1` | 0 | PASS schema-8 manifest, `deterministic-fake-transport`. |
| `scripts\acceptance\first-slice.ps1 -SkipBuild` | 0 | PASS manifest; database schema 8. |
| `scripts\acceptance\employee-roster.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest; non-model session contract only, real Codex CLI. |
| `scripts\acceptance\collaboration-bus.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\conversation-delivery.ps1 -SkipBuild` | 0 | PASS deterministic manifest. |
| `scripts\acceptance\task-delegation-graph.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\automatic-task-dispatch.ps1` | 0 | PASS schema-8 fake-transport manifest. |

## Approval-gate acceptance evidence

`artifacts/acceptance/approval-gate-manifest.json` records literal `PASS`, schema 8, and `deterministic-fake-transport`. Its focused commands prove:

- the policy table: `ask` always waits, `self-decision` acts in scope and waits on scope expansion, `full-autonomy` proceeds;
- an `ask` turn holds the provider until `approval.decide`, lists as pending, forwards `accept`, and completes;
- decline, expiry (`LZ-APPROVAL-TIMEOUT`), and turn cancellation (`LZ-APPROVAL-TURN-CANCELLED`) each answer the provider `decline` with matching durable state and audit events;
- `self-decision` approves in-scope file changes with `decider = policy` and waits on a `grantRoot` scope expansion;
- request and decision replays are write-free, stale revisions and settled rows fail closed, and requests require an owned in-flight turn and bounded input;
- pending approvals from a previous daemon boot expire with `LZ-APPROVAL-OWNER-LOST` and one `approvals.reconciled` event;
- the Codex adapter answers the native request id with `{"decision": "accept"}` after an approving observer and `{"decision": "decline"}` without one, and normalizes requests into bounded, redacted evidence;
- CLI mapping for `approval list|show|approve|decline` and exit code 14; and
- no provider smoke, external model use, or desktop UI change.

## Boundary evidence

- `git diff --check` passed.
- Both staged and unstaged diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, or frontend source changed.
- No session-wide, prefix, or host grant path exists; `approval.decide` accepts only `approve` or `decline` for one approval id and revision.
- No live provider/model gate was added.
- No push or pull request was created.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

ACP approval bridging, per-capability policy, per-project expiry and notifications, typed desktop Inbox binding, ConPTY/native terminal attachment, managed memory, scheduling, resource governance, other-provider task delivery, and task-worktree cleanup remain separate reviewed slices.
