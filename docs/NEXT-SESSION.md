# Next session: post-approval-gate backend boundary

## Current owner boundary

Backend orchestration remains the active scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities without a separate reviewed desktop contract.

This handoff does not authorize ConPTY, terminal I/O, session-wide approval grants, per-capability policy, managed memory, resource governance, scheduling, other-provider task delivery or approval bridging, provider smoke, or a live model turn.

## Completed backend baseline

- Database schema 8 adds the durable `approvals` ledger and `approval_commands` to the employee, session, message, conversation, turn, task/evidence, dispatch, audit, and outbox ledgers.
- Codex `item/commandExecution/requestApproval` and `item/fileChange/requestApproval` are no longer auto-declined. The adapter normalizes them into a bounded, redacted `PermissionRequest`, asks the daemon through `TurnEventObserver::decide_permission`, answers Codex `accept` or `decline`, and only then surfaces any decision failure.
- Every request is recorded first. Policy is the linked task's effective authority: `ask` waits for the local user, `self-decision` auto-approves in-scope work and waits on scope expansion (`grantRoot`, additional permissions, network context), `full-autonomy` auto-approves. Policy outcomes are durable rows with `decider = policy`.
- `approval.decide` is the only way to settle a pending approval and is local-user only, revision-checked, idempotent, and bound to one fingerprinted action. No session-wide or prefix grant exists.
- Pending approvals expire after the daemon's fifteen-minute default, close as `cancelled` when their turn is cancelled or ends, and close as `expired` with `LZ-APPROVAL-OWNER-LOST` on restart. Each closure answers the provider `decline`.
- Waiting for the user pauses the provider message loop and is excluded from the Codex turn budget, so a pending approval never trips the progress or turn timeout.
- `approval.list` and `approval.show` expose the ledger; `lazooctl approval list|show|approve|decline` map to them with CLI exit code 14 for `LZ-APPROVAL-*`.
- The desktop snapshot and every file under `apps/lazoo-desktop` remain unchanged.

## Deterministic acceptance

`scripts/acceptance/approval-gate.ps1` runs focused fake-backend, store, Codex-fixture, and CLI proofs for user-gated approval, decline/expiry/cancellation fail-closed behavior, self-decision scope policy, restart owner loss, ledger idempotency, and the adapter's wire answer. It writes `artifacts/acceptance/approval-gate-manifest.json`.

No provider smoke or live model turn is part of this slice.

## Next backend objective

Choose and design one separately reviewed slice: ConPTY/native terminal attachment, typed desktop task/dispatch/approval Inbox binding, or approval bridging for ACP providers with per-capability policy. Do not combine these boundaries implicitly.

Do not emit `UI BACKEND CONTRACT: GO` yet. Approval, dispatch, and task read models exist in the daemon, but the typed desktop-facing contract has not been separately reviewed.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the durable record-first ledgers and native provider evidence.
- Treat `approved` as proof that Lazoo answered the provider `accept`, never as proof that the action ran; provider evidence remains authoritative.
- Never widen a decision beyond the one approval row and revision it names.
- Never retry an uncertain dispatch or re-answer an expired approval automatically.
- Never delete or rewrite a retained task worktree without an explicit cleanup contract.
- Keep UI, ConPTY, session-wide grants, managed memory, scheduling, resource governance, other-provider expansion, and live model use outside this completed slice.
