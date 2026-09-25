# Next session: post-peer-messaging backend boundary

## Current owner boundary

Backend orchestration remains the active scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities without a separate reviewed desktop contract.

This handoff does not authorize ConPTY, terminal I/O, session-wide approval grants, per-capability policy, managed memory, resource governance, scheduling, other-provider task delivery, approval bridging, or tool surfaces, automatic activation of a dormant employee, cross-tree or user-to-employee messaging through the tools, bounded remediation, user notification channels, provider smoke, or a live model turn.

## Completed backend baseline

- Schema stays at 9. Peer messages are `peer` and `reply` rows in the message ledger, queued under the `message.peer` command scope with the recipient's task as their task, both tasks referenced, the reply's causation pointing at the message it answers, and the rendered turn text in the body. They are auto-delivered through the same notice path as result and incident notices.
- Every daemon-managed Codex session declares eight tools: `employees_list`, `task_delegate`, `task_wait`, `task_report`, `incident_report`, `incident_list`, `message_send`, and `message_reply`. `message_send` addresses another employee with an `accepted` or `running` task under the caller's root task and can block up to ten minutes for the reply; `message_reply` answers a message addressed to the caller's employee and task. The sender is always the turn's own endpoint.
- A recipient reads peer messages inside its own `task_wait` (which now returns early with `messages`) or `message_send` wait, acknowledged with `{via, turnId}` as evidence, or, when idle with a running task and no unfinished turn, as one provider turn on its existing session. A message read inside a turn closes any notice turn recorded earlier for it while the recipient was dormant.
- A completed peer turn projects as task progress, never completion, so a question delivered just before a child's result cannot cancel that result. Result notices and the employee's own turns keep completing tasks.
- Activation policy is unchanged: employee-authored traffic never starts a provider process. A peer message to a dormant recipient waits until the local user resumes it; a message whose recipient task ends first is cancelled, and a waiting sender returns with that state.
- The result-delivery (`b320946`), minimum-incidents (`762aec2`, `24a851a`), and collaboration-tools (`d72de10`) slices underneath are unchanged, and every acceptance script asserts schema 9.
- The desktop snapshot, the CLI, and every file under `apps/lazoo-desktop` remain unchanged.

## Deterministic acceptance

`scripts/acceptance/peer-messaging.ps1` runs focused domain, store, and daemon fake-backend proofs and writes `artifacts/acceptance/peer-messaging-manifest.json`.

No provider smoke or live model turn is part of this slice. A live Codex gate (a real model calling the eight tools, asking a sibling a question through `message_send`, and receiving a result notice after its turn ended) remains the natural next authorized live proof and must be run explicitly.

## Next backend objective

Choose and design one separately reviewed slice:

- **Activation policy and bounded automatic activation**: a per-employee `activationPolicy` (schema 10) that lets a queued notice or peer message start a session for a dormant recipient under explicit bounds (one activation per queued batch, the recipient's own task tree only, stop after the turn), with the default staying manual.
- **Live Codex tool-call, peer-message, and notice gate**: one explicitly authorized model turn proving the eight tools, one peer exchange, and a result notice end to end through the real app-server.
- **Coalesced notice turns**: several queued notices and peer messages for one recipient delivered as one turn, replacing the one-turn-per-message rule while keeping every message's own acknowledgement evidence.

Do not combine these boundaries implicitly. Bounded lead remediation (at most two policy-approved remedies inside five minutes), user notification channels, cross-tree and user-to-employee messaging through the tools, ConPTY, typed desktop Inbox binding (tasks, approvals, incidents, notices, messages), and ACP approval bridging remain available candidates behind those.

Do not emit `UI BACKEND CONTRACT: GO` yet. Approval, dispatch, task, tool, incident, notice, and message read models exist in the daemon, but the typed desktop-facing contract has not been separately reviewed.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the durable record-first ledgers and native provider evidence.
- A notice or peer message is derived evidence: the task, message, and incident rows it points at stay authoritative, and neither ever retries work, restarts anything, re-authenticates, changes authority, or deletes data.
- Never let employee-authored traffic start a provider process; only the local user resumes a dormant employee until a reviewed activation policy exists.
- A notice or peer turn runs under the recipient task's effective authority; governed actions inside it still flow through the schema-8 approval gate, and neither a tool result nor a peer message is ever an approval.
- Never let a tool name a different actor, widen authority, or bypass a store guard; the daemon fixes the actor to the turn's recipient endpoint.
- Never deliver a peer message to a task that has not started, and never let a peer turn complete a task.
- Never declare tools on a resumed Codex thread; the adapter fails closed until Codex accepts `dynamicTools` on `thread/resume`.
- Never retry an uncertain dispatch, a failed notice or peer turn, or an expired approval automatically.
- Never delete or rewrite a retained task worktree without an explicit cleanup contract.
- Keep UI, ConPTY, session-wide grants, managed memory, scheduling, resource governance, other-provider expansion, and live model use outside this completed slice.
