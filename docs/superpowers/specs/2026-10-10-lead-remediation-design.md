# Bounded Lead Remediation Design

**Date:** 2026-10-10

**Status:** Implemented on `feat/lead-remediation` (stacked on `feat/idle-suspension` at `4d2ff45`, pull request #2, because it needs schema 11) with deterministic acceptance, for owner review in a pull request before merge.

**Scope:** Backend-only. Schema 12 adds four incident columns (`escalation`, `remediation_deadline_at`, `escalated_at`, `escalation_reason`) and one table (`incident_remedies`). The daemon declares a ninth tool, `incident_remedy`, and its 15-second sweep closes remediation windows. `lazooctl incident list` gains `--escalation`. No desktop or Tauri change, no live model use.

## 1. Outcome

Before this slice an incident promoted to `priority` on a child task reached the lead as one `status` notice and then nothing else happened: the lead had no bounded way to act on it, and nothing told the user that the lead had not fixed it. The brief's section 14.2 asks for both, and step 10 of the core acceptance scenario ("the threshold notifies Rhea, allows bounded remediation, and escalates to the user when unresolved") depends on it.

Lazoo now answers three questions without inference:

1. Who may act on a promoted incident first? The lead of the task tree it belongs to, for five minutes, with at most two remedies from a fixed set, recorded beside the incident.
2. When does the user hear about it? As soon as the lead's window can no longer fix it: at promotion when there is no lead, at the first recurrence after the second remedy, and otherwise when the window closes, unless the failure has not recurred since the lead's last remedy.
3. What did anyone do about it? Every remedy is a row naming the lead, its turn, the attempt, the affected task, the new task for a redelegation, and the occurrence count it was applied at; every escalation names its reason.

## 2. Rule

`remediation_verdict(windowClosed, remedies, recurredSinceLastRemedy)` in `lazoo-domain` is the whole rule:

| Window | Remedies applied | Recurred since the last remedy | Verdict |
|---|---|---|---|
| any | 2 | yes | user (`remedies-exhausted`) |
| open | any | otherwise | the lead may still act |
| closed | 0 | - | user (`no-remedy`) |
| closed | 1 or 2 | yes | user (`recurred-after-remedy`) |
| closed | 1 or 2 | no | `remediated` |

Two constants bound it: `MAX_INCIDENT_REMEDIES = 2` and `INCIDENT_REMEDIATION_WINDOW_SECONDS = 300`, the same five minutes as the promotion threshold window.

The rule is applied in exactly two places. `raise_incident` applies it to a new occurrence of an incident whose escalation is `lead` (a recurrence after every recorded remedy) or `remediated` (judged with a closed window), so a failure that comes back after the second remedy, or after a remediated window, reaches the user in the same transaction that records it. `escalate_due_incidents(now)` applies it once each `lead` window has passed, and is the only place an incident becomes `remediated`.

## 3. Escalation states

| `escalation` | Meaning |
|---|---|
| `none` | Normal incident, never promoted (the default for every row). |
| `lead` | Promoted on a task below an active root, not about that root's own employee; `remediationDeadlineAt` is the promotion time plus five minutes. Remedies apply only when the task is a direct child of the root (section 4). |
| `remediated` | The window closed and the failure has not recurred since the lead's last remedy. The incident stays open for the user to review and resolve. |
| `user` | A priority notification for the local user, with `escalatedAt` and `escalationReason` (`no-lead`, `no-remedy`, `remedies-exhausted`, `recurred-after-remedy`, `schema-upgrade`). |

Promotion (normal to priority, unchanged at three equivalent failures inside five minutes) decides between `lead` and `user` with the same predicate that decides whether the lead gets a notice at all (`incident_lead_root`, now shared by both): the incident names a task below an active root, and is not about the root's own employee. Anything else (no task, the root task itself, a terminal tree, the lead's own failure) has no lead to remediate it and goes to the user at once. The migration sends every unresolved priority incident written before schema 12 to the user with reason `schema-upgrade`.

Escalation changes bump the incident revision, so a stale acknowledge or resolve is refused as before. Only the user acknowledges or resolves an incident; escalation never does either.

## 4. Remedies

The set is fixed and small, and each remedy reuses an existing guarded store path inside the same immediate transaction that records it:

- `cancel-task` cancels the incident's task through the same transition `task.cancel` uses (`cancel_task_tx`), with the lead's endpoint recorded as the actor. A task whose dispatch turn is in flight becomes `cancel-requested`, and the daemon then asks the provider to cancel that turn exactly as `task.cancel` does (`cancel_dispatch_turn`, now shared).
- `redelegate-task` delegates the incident's task's objective again as a new child of the lead's root task, to `employeeId` (default: the same employee), through `delegate_task_tx`: the cited roster revision, a route-ready assignee, authority narrowing, depth and fan-out limits, and no self-delegation all apply, and `ask` authority still proposes the task for the user. Only a `failed` or `cancelled` task can be redelegated, so the old and new tasks never run at once, and an `uncertain` task (which may have run) is never redelegated. The new task's delegation references the old task and the incident.

Bounds, all checked in the store:

- Window: the incident is unresolved, its escalation is `lead`, and its deadline has not passed.
- Attempts: at most two per incident, by count in the store and by `CHECK (attempt BETWEEN 1 AND 2)` with `UNIQUE (incident_id, attempt)` in the schema.
- Scope: the calling turn's task is the root task of the incident's tree, the turn's recipient is that root's employee, and the incident's task is a direct child of it. A grandchild's failure is the user's after the window.
- Atomicity: a remedy row exists only for an action that committed in the same transaction, so a refused remedy (window, attempts, scope, target state, or any task guard) consumes no attempt.
- Idempotency: the remedy is keyed by the tool call (`tool:<turnId>:<callId>`); a replay returns the recorded remedy without writing, and the same key with a different incident or kind is refused.

Retry of a `retryable-failed` dispatch turn and a session restart are deliberately not remedies: both would start a provider process from employee-authored traffic. Cancel and redelegate cover the same ground through the dispatcher's own bounded path.

## 5. Durable records

Migration `0012_lead_remediation.sql`:

- `incidents.escalation TEXT NOT NULL DEFAULT 'none'` with a `CHECK` on the four values; `remediation_deadline_at`, `escalated_at`, and `escalation_reason` (checked) are nullable; an index on `(escalation, sequence)`.
- `incident_remedies(sequence, id, incident_id, attempt, kind, idempotency_key UNIQUE, request_id, lead_employee_id, lead_task_id, turn_id, target_task_id, result_task_id, occurrences_at_remedy, note, created_at)`.

`IncidentRecord` gains `escalation`, `remediationDeadlineAt`, `escalatedAt`, `escalationReason`, and `remedies` (each `IncidentRemedyRecord`), all serde-defaulted so incident payloads in events written before schema 12 still parse. `IncidentListParams` gains an optional `escalation` filter. New events: `incident.remedy-applied`, `incident.remediated`, and `incident.escalated-to-user`; promotion still emits `incident.escalated`.

The lead's `status` notice text and body carry `remediationDeadlineAt` while the window is the lead's, and say that the incident goes to the user if it is still recurring then. `incident_list` shows each incident's escalation, deadline, and remedies applied.

## 6. Tool

`incident_remedy` is declared with the other eight tools on every daemon-managed Codex session: `{incidentId, remedy: "cancel-task" | "redelegate-task", employeeId?, rosterRevision?, requestedAuthority?, note?}`. `rosterRevision` is required for `redelegate-task`; the redelegation-only fields are refused for `cancel-task`. Refusals answer `success = false` with `LZ-INCIDENT-REMEDY-CLOSED`, `LZ-INCIDENT-REMEDY-SCOPE`, `LZ-INCIDENT-REMEDY-TARGET`, `LZ-TOOL-INVALID`, or the task guard's own code, and never end the turn. The result names the attempt, the remedies remaining, the deadline, and the affected or new task with its state.

## 7. Sweep

The daemon's timed sweep (renamed `run_timed_sweeps`) now runs `escalate_due_incidents_once(now)` beside idle suspension every 15 seconds, so a window closes up to 15 seconds after its deadline. One immediate transaction judges every passed `lead` window; changes are published as events. Escalation is a notification: it starts no turn or session, and cancels, retries, or reassigns nothing.

## 8. Rules kept from the earlier slices

- The promotion threshold, the fold, acknowledge, resolve, and the one-notice-per-incident rule are unchanged.
- Employee-authored traffic still never starts a provider process on its own; a remedy only cancels or creates tasks, and a redelegated task is dispatched by the ordinary dispatcher when its worker is free.
- Authority never widens: cancellation needs no authority beyond owning the parent task, and redelegation is narrowed exactly like `task_delegate`.

## 9. Deterministic acceptance

`scripts/acceptance/lead-remediation.ps1` runs domain, store, CLI, and daemon fake-backend proofs and confirms the desktop tree is unchanged, writing `artifacts/acceptance/lead-remediation-manifest.json`. No provider smoke or live model turn.

## 10. Explicitly deferred work

- user notification channels beyond the incident ledger and event stream (a native Windows toast, desktop Inbox binding);
- remedies that start a provider process (turn retry, session restart) and remedies on grandchildren;
- the other section 14.2 thresholds (two unexpected exits, one death with assigned work, missed heartbeat, one dead-lettered priority message, daemon crash) and configurable thresholds and windows;
- the live Codex gate, message-arrival notification, cross-tree and user-to-employee messaging, ConPTY, per-capability policy, managed memory, scheduling, and resource governance.

## 11. Review gate

The owner should review this design together with the pull request. Merge authorizes only the schema-12 columns and table, the `incident_remedy` tool with its two remedies and bounds, the escalation sweep, and the CLI filter; it does not authorize any other remedy, notification channel, desktop work, live provider consumption, or any deferred capability above.
