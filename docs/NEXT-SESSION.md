# Next session: post-incidents backend boundary

## Current owner boundary

Backend orchestration remains the active scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities without a separate reviewed desktop contract.

This handoff does not authorize ConPTY, terminal I/O, session-wide approval grants, per-capability policy, managed memory, resource governance, scheduling, other-provider task delivery, approval bridging, or tool surfaces, peer-message tools, lead notification or bounded remediation, user notification channels, provider smoke, or a live model turn.

## Completed backend baseline

- Database schema 9 adds the `incidents` ledger and `incident_commands` to the employee, session, message, conversation, turn, task/evidence, dispatch, approval, audit, and outbox ledgers.
- The daemon raises an incident right after it commits a session start failure, a provider rejection, stall, timeout, exit, or missing completion evidence, a dead letter, a dispatch failure before provider I/O, and restart owner loss, and folds keeper emergency spool records in at startup. Raising is idempotent per command key; equivalent failures share one row per code, employee, and project; three occurrences inside five minutes promote it to `priority`.
- `incident.list|show|acknowledge|resolve` are local-user RPCs; acknowledge and resolve are revision-checked and idempotent. `lazooctl incident` maps them with exit code 15 for `LZ-INCIDENT-*`.
- Codex sessions declare six Lazoo tools: `employees_list`, `task_delegate`, `task_wait`, `task_report`, `incident_report`, `incident_list`. Every call acts as the turn's recipient endpoint and answers `success = false` with a stable code instead of ending the turn.
- The collaboration-tools slice (commit `d72de10`) underneath this one is unchanged: dynamic tools are declared only on Codex `thread/start` behind `experimentalApi`, with names in the Responses API alphabet.
- The desktop snapshot and every file under `apps/lazoo-desktop` remain unchanged.

## Deterministic acceptance

`scripts/acceptance/minimum-incidents.ps1` runs focused domain, store, daemon fake-backend, and CLI proofs and writes `artifacts/acceptance/minimum-incidents-manifest.json`. Every other acceptance script now asserts schema 9.

No provider smoke or live model turn is part of this slice. A live Codex tool-call gate (a real model calling `employees_list`, `task_delegate`, and `incident_report`) remains the natural next authorized live proof and must be run explicitly.

## Next backend objective

Choose and design one separately reviewed slice:

- **Activation controls and result delivery**: a lead whose turn ended before its children finished has no way to receive their outcomes, and a promoted incident has no way to reach the lead. Decide how child completion and threshold events reach a dormant or idle lead (queued message, explicit user resume, or bounded policy-gated activation) without letting employee-authored traffic activate dormant recipients by accident. This unblocks the brief's lead notification and bounded remediation.
- **Peer messaging tools**: `message_send` and `message_reply` over the existing bus, including the activation policy above.
- **Live Codex tool-call gate**: one explicitly authorized model turn proving the six tools end to end through the real app-server.

Do not combine these boundaries implicitly. ConPTY, typed desktop Inbox binding (tasks, approvals, incidents), and ACP approval bridging remain available candidates behind those.

Do not emit `UI BACKEND CONTRACT: GO` yet. Approval, dispatch, task, tool, and incident read models exist in the daemon, but the typed desktop-facing contract has not been separately reviewed.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the durable record-first ledgers and native provider evidence.
- An incident is derived evidence: the failure row it points at stays authoritative, and an incident never retries work, restarts anything, re-authenticates, changes authority, or deletes data.
- A tool result is never an approval; governed actions inside a child turn still flow through the schema-8 approval gate.
- Never let a tool name a different actor, widen authority, or bypass a store guard; the daemon fixes the actor to the turn's recipient endpoint.
- Never declare tools on a resumed Codex thread; the adapter fails closed until Codex accepts `dynamicTools` on `thread/resume`.
- Never retry an uncertain dispatch or re-answer an expired approval automatically.
- Never delete or rewrite a retained task worktree without an explicit cleanup contract.
- Keep UI, ConPTY, session-wide grants, managed memory, scheduling, resource governance, other-provider expansion, and live model use outside this completed slice.
