# Next session: post-collaboration-tools backend boundary

## Current owner boundary

Backend orchestration remains the active scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities without a separate reviewed desktop contract.

This handoff does not authorize ConPTY, terminal I/O, session-wide approval grants, per-capability policy, managed memory, resource governance, scheduling, other-provider task delivery, approval bridging, or tool surfaces, peer-message or incident tools, provider smoke, or a live model turn.

## Completed backend baseline

- Database schema stays at 8. No migration landed with the collaboration-tools slice.
- The adapter contract carries `ToolSpec`, `ToolCall`, `ToolResult`, `SessionRequest::tools`, and a default fail-closed `TurnEventObserver::call_tool` that answers `LZ-TOOL-UNATTACHED`.
- The Codex adapter opts into the app-server `experimentalApi` only when tools are declared, sends them as `dynamicTools` on `thread/start`, refuses tools on `thread/resume`, bounds `item/tool/call` arguments to 64 KiB, asks the daemon, answers Codex before surfacing any observer failure, and records each call as a `lazoo:<tool>` turn event.
- The daemon declares `employees_list`, `task_delegate`, `task_wait`, and `task_report` to every Codex session it starts. Every call acts as the turn's recipient employee and session; `task_delegate` reuses `Store::delegate_task` (roster revision, self-delegation, authority narrowing, depth, fan-out), `task_wait` polls the caller's task tree for at most ten minutes and aborts when the caller's turn ends, and `task_report` reuses `Store::report_task`. Refusals answer `success = false` and never end the turn.
- `ask` delegations stay `proposed` until `task.accept`; accepted delegations wake the existing dispatcher. A worker's explicit `task_report failed` remains authoritative over its later native completion.
- The desktop snapshot and every file under `apps/lazoo-desktop` remain unchanged.

## Deterministic acceptance

`scripts/acceptance/collaboration-tools.ps1` runs focused contract, Codex-fixture, daemon fake-backend, and store schema proofs and writes `artifacts/acceptance/collaboration-tools-manifest.json`.

No provider smoke or live model turn is part of this slice. A live Codex tool-call gate (a real model calling `employees_list` and `task_delegate`) is the natural next authorized live proof and must be run explicitly.

## Next backend objective

Choose and design one separately reviewed slice:

- **Activation controls and result delivery**: a lead whose turn ended before its children finished has no way to receive their outcomes; today only an in-turn `task_wait` reads them. Decide how child completion reaches a dormant or idle lead (queued completion message, explicit user resume, or bounded automatic activation under policy) without violating "employee-authored turns cannot activate dormant recipients" by accident.
- **Minimum incidents**: normalize provider failures, stalls, dead letters, and owner loss into the human-readable incident ledger with thresholds and an `incident.list` tool.
- **Peer messaging tools**: `message.send` and `message.reply` over the existing bus, including the activation policy above.

Do not combine these boundaries implicitly. ConPTY, typed desktop Inbox binding, and ACP approval bridging remain available candidates behind those.

Do not emit `UI BACKEND CONTRACT: GO` yet. Approval, dispatch, task, and tool read models exist in the daemon, but the typed desktop-facing contract has not been separately reviewed.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the durable record-first ledgers and native provider evidence.
- A tool result is never an approval; governed actions inside a child turn still flow through the schema-8 approval gate.
- Never let a tool name a different actor, widen authority, or bypass a store guard; the daemon fixes the actor to the turn's recipient endpoint.
- Never declare tools on a resumed Codex thread; the adapter fails closed until Codex accepts `dynamicTools` on `thread/resume`.
- Never retry an uncertain dispatch or re-answer an expired approval automatically.
- Never delete or rewrite a retained task worktree without an explicit cleanup contract.
- Keep UI, ConPTY, session-wide grants, managed memory, scheduling, resource governance, other-provider expansion, and live model use outside this completed slice.
