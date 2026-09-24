# Result Delivery and Activation Controls Design

**Date:** 2026-09-24

**Status:** Implemented on `feat/result-delivery` (based on `feat/minimum-incidents`) with deterministic acceptance, for owner review before merge.

**Scope:** Backend-only. No schema change: the schema-4 message ledger already had unused `result` and `status` kinds. The store folds terminal child tasks and promoted incidents into durable lead notices, records the turn that delivers a notice, and lets `task_wait` consume a result it already showed the lead. The daemon runs one notice-delivery loop beside the task dispatcher. No desktop or Tauri change, no CLI change, no peer-message tools, no bounded remediation, no automatic process activation, no live model use.

## 1. Outcome

Before this slice a lead that delegated work and ended its turn (or timed out in `task_wait`) never learned what its children produced, and a promoted incident about its task tree reached only the user. The task graph, dispatcher, tools, and incident ledger were complete, but the loop back to the lead was open.

Lazoo now answers four questions without inference:

1. Which child outcome or promoted incident is waiting for which lead, on which task, in which state (`queued`, `delivering`, `acknowledged`, `cancelled`)?
2. How did it reach the lead: as a notice turn on its idle session, or inside the lead's own `task_wait` answer?
3. Why has a notice not been delivered yet: the lead is busy on another turn, dormant, or its task is already terminal?
4. What exactly did the lead read: the notice turn's text is the rendered envelope, bounded and stored on the turn.

## 2. Activation policy

The brief lists a direct peer message and a delegated task among activation triggers, but the handoff asked for a decision that never lets employee-authored traffic activate a dormant recipient by accident. The decision:

| Lead state | What happens to a queued notice |
|---|---|
| In-flight turn on its task (any state `recorded`, `submitting`, `accepted`), or `active_turn_id` set | Waits. `task_wait` inside that turn delivers child results directly and acknowledges their notices. |
| Idle session owned by this daemon boot, task `accepted` or `running` | Delivered now as one provider turn per notice, sequentially, using the existing turn pipeline and the task's effective authority. |
| No owned idle session (dormant, stopped, prior boot, provider not ready) | Waits, with the notice turn already recorded. `session.warm` or a user turn by the local user is the only way to resume the lead; the next pass then delivers. |
| Task terminal, or lead disabled or reconfigured | The notice is cancelled with the reason logged; the child task, message, and incident rows keep the evidence. |

A notice therefore never starts a provider process, never widens authority, and never replaces a user turn. Bounded policy-gated automatic activation of a dormant lead remains a separate slice behind a per-employee activation policy field; it needs a migration and the brief's per-employee configuration.

## 3. Notices

| Notice | Source | Sender → recipient | Kind | Linked task | Text |
|---|---|---|---|---|---|
| Task result | A child task reaching `completed`, `failed`, `cancelled`, or `uncertain` by any path (native completion, `task_report`, cancellation, dispatch failure, restart) | child's assigned employee → parent task's assigned employee | `result`, threaded under the delegation message's correlation | the parent task | child id, worker name, state, objective, failure, native output bounded to 16 KiB, count of unfinished siblings, next tools |
| Incident notice | An incident promoted to `priority`, unresolved, linked to a non-root task in an active tree, about an employee other than the lead | failing task's employee → root task's assigned employee | `status` | the root task | code, title, explanation, occurrences, next action, incident id, the no-retry boundary |

Both are queued idempotently, keyed by child task id (`task-result:<id>`) or incident id (`incident-notice:<id>`) under their own `message_commands` scopes. Only messages queued under those scopes are auto-delivered; peer messages sent through `message.send` keep their manual claim and acknowledge path.

## 4. Delivery

```text
task / incident commit ──▶ notice pass (daemon loop, one pass per published event)
                             │ pending_task_results / pending_incident_notices → queue notice
                             │ pending_lead_notices (queued, oldest first)
                             ▼
                      record_notice_turn  (reuses the notice as the turn's canonical message;
                             │             fails closed while another turn on the task is unfinished)
                             ▼
                      deliver_recorded_turn (same path as turn.submit: delivery target,
                             │                 idle owned session, begin submission, provider turn)
                             ▼
             turn accepted → message acknowledged; turn completed → task projection as usual
```

`task_wait` consumes results on its own: for every terminal child it returns, it queues the result if missing, then claims and acknowledges it with `{via: "task_wait", turnId}` as evidence, so the same outcome is never delivered twice.

## 5. Durable records

No new tables. A notice is one `messages` row plus its delivery and command rows and a `message.queued` event. A notice turn is one `conversation_turns` row whose `message_id` is the notice, recorded under the existing `turn.record` command scope with key `notice-turn:<messageId>`, and a `turn.recorded` event carrying `notice: true`. Delivery, acknowledgement, failure, cancellation, and task projection reuse the schema-5 through schema-9 paths unchanged.

## 6. Fixed boundaries

- A notice is derived evidence; it never retries work, restarts anything, changes authority, or deletes data.
- A notice turn runs under the lead task's effective authority; governed actions inside it still flow through the schema-8 approval gate.
- Provider failures during a notice turn land on the turn, message, and incident ledgers exactly as user turns do; the notice becomes `retryable-failed` or `terminal-failed` and is not retried automatically.
- Incidents about the lead itself, about the root task, without a task, below priority, or on a terminal tree never notify the lead; the user sees them through `incident.list`.
- `desktop.snapshot`, Tauri commands and capabilities, `apps/lazoo-desktop`, the CLI, and schema 9 are byte-for-byte unchanged.

## 7. Deterministic acceptance

`scripts/acceptance/result-delivery.ps1` runs domain, store, and daemon fake-backend proofs and confirms the desktop tree is unchanged. It must show: bounded truthful notice text; results queue once per child and thread under the delegation; a notice turn reuses the notice message and waits while another turn is unfinished; `task_wait` consumes with evidence and a foreign turn is refused; priority incidents notify the lead once and never about itself; an idle lead receives a result turn after its own turn ended; `task_wait` prevents a second delivery; a dormant lead is never activated and an explicit `session.warm` flushes the waiting notice; a promoted incident reaches the lead once; schema stays 9.

## 8. Explicitly deferred work

- per-employee activation policy with bounded automatic activation of a dormant lead (needs a migration);
- coalescing several queued notices into one turn;
- `message_send` and `message_reply` peer tools on the same delivery path;
- bounded lead remediation and user notification channels;
- a task-change notification replacing the pass-per-event wake;
- typed desktop Inbox binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance.

## 9. Review gate

The owner should review this design together with the branch. Merge authorizes only the backend notice ledger, its delivery loop, and the deterministic acceptance; it does not authorize automatic activation, desktop work, live provider consumption, or any deferred capability above.
