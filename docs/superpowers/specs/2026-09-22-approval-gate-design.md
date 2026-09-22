# Approval Gate and Authority Policy Design

**Date:** 2026-09-22

**Status:** Implemented on `feat/approval-gate` with deterministic acceptance and merged to `main` on 2026-09-23 at the owner's request. The owner's standing instruction for this session was to build the most valuable remaining backend slice, so design and implementation landed together instead of the usual two-step gate.

**Scope:** Backend-only durable approval ledger, authority policy evaluation, a provider-neutral approval bridge in the adapter contract, the Codex adapter answering native approval requests with Lazoo's decision, daemon RPC, and CLI controls. No desktop or Tauri capability change, no session-wide or prefix grants, no ConPTY, no approval delivery for providers other than Codex, no managed memory, and no live model use.

## 1. Outcome

Before this slice the Codex adapter answered every `item/commandExecution/requestApproval` and `item/fileChange/requestApproval` with a hard-coded `decline`. `ask` authority therefore could not gate anything, and `self-decision` and `full-autonomy` were only prompt-level hints to the provider.

Lazoo now answers four questions without inference:

1. Which governed action did a provider ask to perform, on behalf of which employee, turn, task, and session?
2. Which authority applied, and did policy or the local user decide?
3. What exactly was approved, bound to a stable action fingerprint?
4. Why did a pending request end without a decision: expiry, turn cancellation, or daemon loss?

The approval ledger is control-plane truth. An approval being `approved` proves that Lazoo answered the provider `accept`; provider evidence still decides what actually ran.

## 2. Fixed boundaries

- Windows-first, local-first, single-user behavior is unchanged. The local user authenticated by the named pipe is the only actor that may decide a pending approval.
- No employee, peer message, or provider model can approve a governed action. Decisions arrive only through `approval.decide`.
- A decision is bound to one approval row and its revision. There is no `acceptForSession`, prefix, or host grant; the same action asks again next time.
- Every path fails closed. No decision, an error, expiry, cancellation, and daemon loss all answer the provider `decline`.
- The provider is always answered before a decision failure is surfaced, so a native turn never hangs on an unanswered request.
- Waiting for the user never counts against the provider turn or progress budget.
- `desktop.snapshot`, Tauri commands/capabilities, and `apps/lazoo-desktop` remain byte-for-byte outside scope.
- Only the daemon-managed Codex path is governed. ACP providers keep their existing fail-closed cancellation of `session/request_permission`, and Claude Code and Antigravity continue to run under their native permission modes without a Lazoo bridge.

## 3. Governed request path

```text
Codex app-server ── item/*/requestApproval ──▶ CodexSession::handle_server_request
                                                 │ normalize_codex_permission_request
                                                 ▼
                                        TurnEventObserver::decide_permission
                                                 │ (daemon: App::govern_permission)
                                                 ▼
                              Store::request_approval  (record-first, policy applied)
                                                 │
                     pending ──▶ wait: approval.decide | expiry | turn cancel | boot loss
                                                 ▼
                                 PermissionDecision::{Approve, Decline}
                                                 │
Codex app-server ◀── {"decision": "accept" | "decline"} ◀──── adapter answers, then continues
```

The adapter contract gains `PermissionRequest`, `PermissionKind`, `PermissionDecision`, and a default `TurnEventObserver::decide_permission` that declines. An observer that does not attach an approval flow (unobserved turns, provider smoke, tests without the daemon) keeps the historical fail-closed behavior.

Codex request parameters are normalized into a bounded, redacted `PermissionRequest`: the action text is at most 1 KiB, the retained command at most 4 KiB, and only known scalar fields (`threadId`, `turnId`, `itemId`, `approvalId`, `reason`, `cwd`, `grantRoot`, `additionalPermissions`, `networkApprovalContext`) survive. `grantRoot`, additional permissions, or network context mark the request as a scope expansion.

## 4. Policy

Authority is policy, not prompt text. The effective authority is the linked task's effective authority (root and child tasks already carry the most restrictive inherited value); a turn without a task falls back to the employee's configured mode.

| Authority | In-scope request | Scope expansion |
|---|---|---|
| `ask` | pending user decision | pending user decision |
| `self-decision` | approved by policy | pending user decision |
| `full-autonomy` | approved by policy | approved by policy |

Policy decisions are durable rows with `decider = policy`; they are never silent. Scope expansion is the only capability axis distinguished today; per-capability rules are a later slice.

## 5. Durable records and lifecycle

`ApprovalRecord` is contract version 1 and holds: sequence, id, turn, optional task, conversation, optional project, employee, optional session, owning boot, provider, native session/turn/request identity, kind (`command-execution` | `file-change`), bounded action text, bounded redacted detail, SHA-256 fingerprint of kind + action + scope, authority, scope-expansion flag, state, decider, revision, failure code/message, and timestamps including `expiresAt`.

States:

| State | Meaning |
|---|---|
| `pending` | The local user must decide; the provider is waiting. |
| `approved` | Policy or the user approved; the provider was answered `accept`. |
| `declined` | The user declined; the provider was answered `decline`. |
| `expired` | No decision before `expiresAt`, or the owning daemon was lost. Answered `decline`. |
| `cancelled` | The governed turn was cancelled or ended while pending. Answered `decline`. |

Only `pending` can transition, exactly once, and each transition increments the revision. Every effective mutation writes the row, one audit event (`approval.requested`, `approval.decided`, `approval.expired`, `approval.cancelled`, or one `approvals.reconciled` at startup), one outbox row, and, for user and provider commands, one idempotency mapping in the same immediate SQLite transaction. Replays return the original result without new writes; stale revisions and settled rows fail before any write.

## 6. Timing

- The daemon holds one pending timeout (fifteen minutes by default) and stores the resulting `expiresAt`. The store clamps any timeout to one week.
- While a decision is pending the adapter is parked inside its message loop, so no provider progress timeout can fire, and the elapsed wait is subtracted from the turn budget when the loop resumes.
- The daemon re-reads the pending row every 250 ms and immediately on `approval.decide` or `turn.cancel`. It closes the row as `cancelled` when the turn has a cancellation request or is no longer in flight, and as `expired` when `expiresAt` has passed. A concurrent user decision wins the race; the daemon reads the settled row back.
- A request that arrives after the turn already has a cancellation request is recorded as `cancelled` immediately.

## 7. Restart truth

Startup order becomes sessions, messages, turns, approvals, tasks, dispatches, outbox. Every `pending` approval owned by a previous boot closes as `expired` with `LZ-APPROVAL-OWNER-LOST`, because the native request that produced it died with that boot. One `approvals.reconciled` event summarizes the corrections. The turn and task owning that approval were already made `uncertain` by the existing reconciliation.

## 8. RPC and CLI surface

- `approval.list` with optional `state`, `turnId`, `taskId`, `conversationId`, `employeeId`, `afterSequence`, and a 1..=1024 limit.
- `approval.show`
- `approval.decide` with `id`, `expectedRevision`, `decision` (`approve` | `decline`), and `idempotencyKey`.

CLI: `lazooctl approval list|show|approve|decline`. Stable errors use `LZ-APPROVAL-*` (invalid, not found, stale, state, ownership, turn state, turn not found, idempotency conflict, store) and CLI exit code 14.

## 9. Schema 8

Schema 8 adds `approvals` and `approval_commands`. No existing table changes and no backfill is required; historical turns had no governed requests that Lazoo answered other than `decline`, and inventing rows for them would be false evidence.

## 10. Deterministic acceptance

`scripts/acceptance/approval-gate.ps1` runs focused tests with the in-process fake session backend and the disposable Codex app-server fixture, then proves the desktop tree is unchanged. It must show:

- `ask` blocks the provider until the user decides, the approval is listed as pending, the decision is forwarded as `accept`, and the turn completes;
- decline, expiry, and cancellation all answer `decline` with the matching durable state and failure code;
- `self-decision` auto-approves in-scope work with `decider = policy` and waits on scope expansion;
- replayed decisions are write-free, stale revisions and settled rows fail closed;
- pending approvals from a prior boot expire with owner-loss evidence;
- the Codex adapter answers the native request id with `{"decision": "accept"}` after an approving observer and `{"decision": "decline"}` without one, and bounds/redacts the retained request;
- CLI mapping and exit code 14;
- no desktop change, provider smoke, or live model use.

## 11. Explicitly deferred work

- session-wide, prefix, or host grants and provider-native approval amendments;
- per-capability policy (network, connectors, integration, destructive operations) and policy precedence beyond task authority;
- approval delivery for ACP, Claude Code, and Antigravity sessions;
- per-project expiry policy and user notifications for pending approvals;
- typed desktop Inbox binding for approvals;
- ConPTY, managed memory, scheduling, resource governance, and other deferred slices.

## 12. Review gate

The owner should review this design together with the branch. Merge authorizes only the backend approval gate and its deterministic acceptance; it does not authorize desktop work, live provider consumption, or any deferred capability above.
