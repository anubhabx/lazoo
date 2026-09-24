# Next session: post-result-delivery backend boundary

## Current owner boundary

Backend orchestration remains the active scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities without a separate reviewed desktop contract.

This handoff does not authorize ConPTY, terminal I/O, session-wide approval grants, per-capability policy, managed memory, resource governance, scheduling, other-provider task delivery, approval bridging, or tool surfaces, peer-message tools, automatic activation of a dormant employee, bounded remediation, user notification channels, provider smoke, or a live model turn.

## Completed backend baseline

- Schema stays at 9. The message ledger's `result` and `status` kinds now carry lead notices: one `result` per terminal child task, addressed to the parent task's employee and threaded under the delegation; one `status` per priority incident about a child task, addressed to the root task's employee. Both are queued idempotently by child task or incident id under their own `message_commands` scopes and are the only messages the daemon auto-delivers.
- The daemon runs a notice-delivery loop beside the task dispatcher: one pass at boot and one after every published event. A pass queues pending results and incident notices, then delivers each queued notice as one provider turn that reuses the notice as its canonical message, under the lead task's effective authority, through the same delivery pipeline as `turn.submit`.
- Activation policy: a notice is delivered only when the lead has an idle session owned by this boot and no unfinished turn on its task. A dormant lead is never activated by employee-authored traffic; the notice turn stays `recorded` until the local user resumes the lead with `session.warm` or a user turn. A notice whose task is terminal, or whose lead was disabled or reconfigured, is cancelled with the reason logged.
- `task_wait` consumes the results it returns: the notice is queued if missing, then claimed and acknowledged with `{via: "task_wait", turnId}` as evidence, so an outcome the lead already read is never delivered again.
- Incidents about the lead itself, about the root task, without a task, below priority, or on a terminal tree never notify the lead.
- The collaboration-tools (`d72de10`) and minimum-incidents (`762aec2`, `24a851a`) slices underneath are unchanged, and every acceptance script asserts schema 9.
- The desktop snapshot, the CLI, and every file under `apps/lazoo-desktop` remain unchanged.

## Deterministic acceptance

`scripts/acceptance/result-delivery.ps1` runs focused domain, store, and daemon fake-backend proofs and writes `artifacts/acceptance/result-delivery-manifest.json`.

No provider smoke or live model turn is part of this slice. A live Codex gate (a real model calling `employees_list`, `task_delegate`, and `incident_report`, then receiving a result notice after its turn ended) remains the natural next authorized live proof and must be run explicitly.

## Next backend objective

Choose and design one separately reviewed slice:

- **Peer messaging tools**: `message_send` and `message_reply` for daemon-managed Codex sessions over the existing bus, delivered through the notice path with the same activation policy (idle owned session only, never activating a dormant recipient), linked to both tasks, and never changing task ownership, approvals, or permissions.
- **Activation policy and bounded automatic activation**: a per-employee `activationPolicy` (schema 10) that lets a queued notice or peer message start a session for a dormant lead under explicit bounds (one activation per queued batch, the lead's own task tree only, stop after the turn), with the default staying manual.
- **Live Codex tool-call and notice gate**: one explicitly authorized model turn proving the six tools and a result notice end to end through the real app-server.

Do not combine these boundaries implicitly. Bounded lead remediation (at most two policy-approved remedies inside five minutes), user notification channels, ConPTY, typed desktop Inbox binding (tasks, approvals, incidents, notices), and ACP approval bridging remain available candidates behind those.

Do not emit `UI BACKEND CONTRACT: GO` yet. Approval, dispatch, task, tool, incident, and notice read models exist in the daemon, but the typed desktop-facing contract has not been separately reviewed.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the durable record-first ledgers and native provider evidence.
- A notice is derived evidence: the child task, message, and incident rows it points at stay authoritative, and a notice never retries work, restarts anything, re-authenticates, changes authority, or deletes data.
- Never let employee-authored traffic start a provider process; only the local user resumes a dormant employee until a reviewed activation policy exists.
- A notice turn runs under the lead task's effective authority; governed actions inside it still flow through the schema-8 approval gate, and a tool result is never an approval.
- Never let a tool name a different actor, widen authority, or bypass a store guard; the daemon fixes the actor to the turn's recipient endpoint.
- Never declare tools on a resumed Codex thread; the adapter fails closed until Codex accepts `dynamicTools` on `thread/resume`.
- Never retry an uncertain dispatch, a failed notice turn, or an expired approval automatically.
- Never delete or rewrite a retained task worktree without an explicit cleanup contract.
- Keep UI, ConPTY, session-wide grants, managed memory, scheduling, resource governance, other-provider expansion, and live model use outside this completed slice.
