# Next session: post-dispatch backend boundary

## Current owner boundary

Backend orchestration remains the active scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities without a separate reviewed desktop contract.

This handoff does not authorize ConPTY, terminal I/O, approval workflows, managed memory, resource governance, scheduling, other-provider task delivery, provider smoke, or a live model turn.

## Completed backend baseline

- Database schema 7 adds durable `task_dispatches` to the employee, session, message, conversation, turn, task/evidence, audit, and outbox ledgers.
- Accepted Codex child tasks are selected by the daemon, claimed exactly once, and dispatched automatically when the assigned employee has no active session. Session stop wakes the dispatcher; one failed child does not block healthy siblings.
- The worker turn reuses the canonical delegation message and its correlation identity. No duplicate collaboration message is created.
- Projectless work uses a deterministic owner-protected directory. Project-scoped work uses a deterministic `lazoo/task-<task-id>` Git branch and retained worktree under the Lazoo data root. Dirty files in the registered canonical checkout are never modified.
- Dispatch is record-first. `accepted` remains queued truth; the task becomes `running` only after provider-native acceptance or durable progress.
- Employee configuration drift, unsupported authority/provider paths, workspace preparation failure, and other pre-provider failures terminalize the task, canonical message, and dispatch together. A sibling remains eligible.
- Cancellation before provider I/O atomically cancels the queued canonical message, turn, and dispatch without invoking the provider. Once provider I/O begins, cancellation waits for native cancellation evidence; if native completion arrives first, completion is the terminal winner.
- Startup reconciliation fails a prior-boot pre-provider dispatch closed with `LZ-TASK-DISPATCH-OWNER-LOST`. A possibly in-flight prior-boot turn makes both task and dispatch `uncertain`; it is never retried automatically.
- Task worktrees are retained for audit. Automatic cleanup, merge, deletion, and branch rewriting are deliberately absent.
- `task.show` surfaces dispatch state and workspace metadata. The desktop snapshot and every file under `apps/lazoo-desktop` remain unchanged.

## Deterministic acceptance

`scripts/acceptance/automatic-task-dispatch.ps1` runs focused fake-transport and disposable-repository proofs for background wakeup, canonical-message reuse, projectless and Git-worktree execution, dirty-checkout preservation, failure isolation, cancellation, cleanup, and restart behavior. It writes `artifacts/acceptance/automatic-task-dispatch-manifest.json`.

No provider smoke or live model turn is part of this slice. The previously authorized schema-5 Codex exact-output gate remains historical conversation-delivery evidence only.

## Next backend objective

Choose and design one separately reviewed slice: ConPTY/native terminal attachment, approvals/policy enforcement, or typed desktop task/dispatch binding. Do not combine these boundaries implicitly.

Do not emit `UI BACKEND CONTRACT: GO` yet. The backend dispatch read model exists, but the typed desktop-facing task/dispatch contract has not been separately reviewed.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the durable record-first ledgers and native provider evidence.
- Treat `accepted` as queued control-plane state, never as proof that a provider ran.
- Never retry an uncertain dispatch automatically.
- Never delete or rewrite a retained task worktree without an explicit cleanup contract.
- Keep UI, ConPTY, approvals, managed memory, scheduling, resource governance, other-provider expansion, and live model use outside this completed slice.
