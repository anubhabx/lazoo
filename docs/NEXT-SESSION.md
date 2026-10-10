# Next session: post-user-notifications backend boundary

## Current owner boundary

Backend orchestration remains the active scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities without a separate reviewed desktop contract.

This handoff does not authorize ConPTY, terminal I/O, session-wide approval grants, per-capability policy, managed memory, resource governance, scheduling, other-provider task delivery, approval bridging, or tool surfaces, any activation trigger other than a recorded notice turn for an employee the user set to `notice-turns`, any suspension trigger other than the employee's own idle timeout, any remedy other than `cancel-task` and `redelegate-task` on a direct child of the lead's root task, any notification other than the one Windows notification per user escalation, cross-tree or user-to-employee messaging through the tools, provider smoke, or a live model turn.

## Completed backend baseline

- Schema is 12. An incident promoted to `priority` on a task below an active root (and not about that root's own employee) opens the lead's remediation window: `escalation: lead` with `remediationDeadlineAt` five minutes after promotion, named in the lead's `status` notice. Any other promotion goes to the user at once (`escalation: user`, reason `no-lead`), and every unresolved priority incident from before schema 12 was moved to the user (`schema-upgrade`).
- Inside the window the lead may apply at most two remedies through the ninth tool, `incident_remedy`: `cancel-task` cancels the incident's task (a direct child of the lead's root) with the lead as actor, and an in-flight dispatch turn is asked to cancel exactly as `task.cancel` does; `redelegate-task` sends a `failed` or `cancelled` child's objective out again through every delegation guard. Each remedy commits with its action and is listed under the incident's `remedies`; a refused one consumes no attempt.
- `remediation_verdict` in `lazoo-domain` is the whole rule. The daemon's timed sweep (`run_timed_sweeps`, every 15 seconds beside idle suspension) closes passed windows: no recurrence since the lead's last remedy makes the incident `remediated`; anything else unresolved becomes `escalation: user` with `no-remedy` or `recurred-after-remedy`. A recurrence after the second remedy, or after a remediated window, reaches the user in the transaction that records it. Escalation never acknowledges, resolves, retries, restarts, or reassigns anything.
- `lazooctl incident list --escalation user` lists the priority notifications; `incident show` includes the remedies.
- Section 14.2's default thresholds apply per code (no schema change): `LZ-SESSION-STALLED`, `LZ-DAEMON-UNAVAILABLE`, and an `LZ-PROCESS-EXIT` tied to a task are promoted by their first occurrence (the incident is born `priority` with its escalation, and its event is `incident.escalated`), any other `LZ-PROCESS-EXIT` by its second, and every other code by three inside five minutes. The dead-lettered priority message threshold waits for a message priority field.
- Schema 13 announces each open incident that reaches `escalation: user` on the Windows desktop at most once, through `UserNotifier` (default `NativeNotifier`, the keeper's toast) from the timed sweep: claimed before it is shown, one notification per pass (a count when several arrive together), recorded as `shown` or `failed` and never retried, and settled as `keeper` without a second notification for a keeper record.
- The idle-suspension (`4d2ff45`, pull request #2), activation-policy (`c8b9c8f`), coalesced-notices (`2c28a0e`), peer-messaging (`176979c`), result-delivery (`b320946`), minimum-incidents (`762aec2`, `24a851a`), and collaboration-tools (`d72de10`) slices underneath are unchanged apart from the shared `cancel_task_tx`, `delegate_task_tx`, `cancel_dispatch_turn`, and `incident_lead_root` helpers they now route through. Every acceptance script asserts schema 12.
- The desktop snapshot and every file under `apps/lazoo-desktop` remain unchanged.
- The repository lives at `github.com/anubhabx/lazoo`; every slice lands as a pull request. Lead remediation is pull request #3, stacked on #2; incident thresholds are pull request #4, stacked on #3; user notifications are pull request #5, stacked on #4. Merge in that order; each retargets when its base merges.

## Deterministic acceptance

`scripts/acceptance/lead-remediation.ps1` runs focused domain, store, CLI, and daemon fake-backend proofs and writes `artifacts/acceptance/lead-remediation-manifest.json`; `scripts/acceptance/incident-thresholds.ps1` does the same for the thresholds and writes `artifacts/acceptance/incident-thresholds-manifest.json`; `scripts/acceptance/user-notifications.ps1` proves the notification with a recording notifier and writes `artifacts/acceptance/user-notifications-manifest.json`.

No provider smoke or live model turn is part of this slice. A live Codex gate (a real model calling the nine tools, asking a sibling a question through `message_send`, receiving several results as one notice turn after its turn ended, applying a remedy to a promoted incident, being suspended by its idle timeout, and being woken once by policy for that turn) remains the natural next authorized live proof and must be run explicitly.

See `docs/verification/2026-10-10-lead-remediation.md`, `docs/verification/2026-10-10-incident-thresholds.md`, and `docs/verification/2026-10-10-user-notifications.md`.

## Next backend objective

Choose and design one separately reviewed slice:

- **Message-arrival notification**: replace the 250 ms polls inside `task_wait` and `message_send` with a store-change notify, keeping the same bounded waits and evidence.
- **Live Codex gate**: one explicitly authorized model turn proving the nine tools, one peer exchange, one coalesced result turn, one remedy, one idle suspension, and one policy activation end to end through the real app-server.
- **Typed desktop Inbox contract**: a reviewed, typed read model for tasks, approvals, incidents (with escalation, remedies, and notification state), notices, and messages, so the separately owned UI can bind to it; design only until the UI owner agrees.

Do not combine these boundaries implicitly. Configurable thresholds, a message priority field with its dead-letter threshold, notification actions and quiet hours (tasks, approvals, incidents, notices, messages), cross-tree and user-to-employee messaging, remedies that start a provider process, ConPTY, a deadline-driven sweep, and ACP approval bridging remain available candidates behind those.

Do not emit `UI BACKEND CONTRACT: GO` yet. Approval, dispatch, task, tool, incident, escalation, remedy, notice, message, activation-policy, and idle-timeout read models exist in the daemon, but the typed desktop-facing contract has not been separately reviewed.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the durable record-first ledgers and native provider evidence.
- A notice or peer message is derived evidence: the task, message, and incident rows it points at stay authoritative, and neither ever retries work, restarts anything, re-authenticates, changes authority, or deletes data.
- Never merge, summarise, or truncate one notice into another; a coalesced turn carries each notice whole and each keeps its own message row and evidence.
- Never let employee-authored traffic start a provider process on its own. Only a recorded notice turn for an employee the user set to `notice-turns` may, once per turn, on that employee's own task, and the session stops with the turn; everyone else waits for the local user. No tool can read or set the policy.
- Never stop a session that has an unfinished turn on it, and never stop a session for idleness unless its own employee's timeout, set by the user, has passed. No tool can read or set the idle timeout.
- Never let a lead apply more than two remedies to one incident, after its window, outside a direct child of its own root task, or as anything but itself; never record a remedy whose action did not commit; never let a remedy or an escalation acknowledge or resolve an incident, start a provider process, or redelegate an `uncertain` task.
- A notice or peer turn runs under the recipient task's effective authority; governed actions inside it still flow through the schema-8 approval gate, and neither a tool result nor a peer message is ever an approval.
- Never let a tool name a different actor, widen authority, or bypass a store guard; the daemon fixes the actor to the turn's recipient endpoint.
- Never deliver a peer message to a task that has not started, never let a peer-only turn complete a task, and never reuse a closed notice turn.
- Never declare tools on a resumed Codex thread; every activation is a fresh `thread/start`, and the adapter fails closed until Codex accepts `dynamicTools` on `thread/resume`.
- Never retry an activation, a suspension stop, an uncertain dispatch, a failed notice or peer turn, or an expired approval automatically.
- Never delete or rewrite a retained task worktree without an explicit cleanup contract.
- Keep UI, ConPTY, session-wide grants, managed memory, scheduling, resource governance, other-provider expansion, and live model use outside this completed slice.
