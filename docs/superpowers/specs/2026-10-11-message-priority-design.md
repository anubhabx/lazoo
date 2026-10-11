# Message Priority Design

**Date:** 2026-10-11

**Status:** Implemented on `feat/message-priority` (stacked on `feat/message-arrival-notify`, pull request #6) with deterministic acceptance, for owner review in a pull request before merge.

**Scope:** Backend-only. Schema 14 adds `messages.priority`. Every incident notice to a lead is a priority message, the user may send one through `message.send`, and one dead-lettered priority message is a priority incident, whatever failure dead-lettered it. No desktop app, Tauri, or collaboration-tool change; no live model use.

## 1. Outcome

Section 11.1 of the brief lists priority in the message envelope, and section 14.2 lists "one dead-lettered priority message" among the default persistent-failure thresholds. Neither existed: every message weighed the same, so a lead's notice about a promoted incident that exhausted its delivery attempts was folded with ordinary dead letters (three inside five minutes) or, when its notice turn failed, never reached the incident ledger as a dead letter at all.

Now:

1. Every message carries `priority` (`normal` or `priority`) in its envelope, its summary, and the store.
2. The incident notice that tells a lead about a promoted incident is always `priority`; `message.send` (and `lazooctl message send --priority`) lets the local user send one. Every other message the daemon writes (delegations, results, peer questions and replies, user turns) is `normal`, and the collaboration tools cannot set it.
3. A dead-lettered priority message raises `LZ-MESSAGE-DEADLETTER` promoted by its first occurrence, through the same promotion, lead-window, user-escalation, and notification path as every other priority incident.

## 2. Rule

- `lazoo_domain::incident_threshold(code, facts)` now takes `IncidentFacts { names_task, priority_message }`. `LZ-MESSAGE-DEADLETTER` with `priority_message` has threshold 1; without it, the default three inside five minutes still applies. No other code reads `priority_message`.
- `raise_incident` derives `priority_message` inside its own transaction from the message it names: the code is `LZ-MESSAGE-DEADLETTER` and that message's priority is `priority`. Callers cannot assert it.
- A dead letter is committed by whichever path exhausted the message (`message.fail`, a failed notice or user turn, a failed task dispatch, or restart reconciliation). The incident always follows from the durable dead letter: `message.fail` raises it at once, as before, and the daemon's `note_priority_dead_letters_once` pass raises it for every other priority dead letter that has none yet. The pass runs at startup and at the head of each 15 second timed sweep, so a promoted dead letter is escalated and announced by the same sweep that raises it.
- One incident per dead letter: both paths use the key `dead-letter:<message id>:<attempt id>` (`lazoo_store::dead_letter_incident_key`), and the pass lists only priority dead letters whose key has not been raised. A daemon that stops between the dead letter and the incident raises it after restart.

## 3. Durable records

Migration `0014_message_priority.sql`: `messages.priority TEXT NOT NULL DEFAULT 'normal'` checked to `normal` or `priority`. Incident notices written before schema 14 become `priority`, so one of them already dead-lettered is raised by the first pass after the upgrade. `MessageEnvelope`, `MessageSummary`, and `SendMessageParams` gain `priority`, serde-defaulted to `normal` so older records and requests parse unchanged.

## 4. Rules kept

- Priority changes only the dead-letter threshold. Delivery order, coalescing (oldest first), retry budgets, activation, and expiry are unchanged, and no tool can raise a message's priority.
- The dead letter stays authoritative; the incident never retries, resends, or cancels the message.
- A normal dead letter raised by a turn or task failure is still reported by that failure's own incident, not a second dead-letter incident.

## 5. Deterministic acceptance

`scripts/acceptance/message-priority.ps1` runs protocol, domain, store, CLI, and daemon fake-backend proofs and writes `artifacts/acceptance/message-priority-manifest.json`. No provider smoke or live model turn.

## 6. Explicitly deferred work

- priority-ordered delivery and backpressure (section 11.3);
- per-employee or per-tool priority, and message expiry;
- configurable thresholds, typed desktop Inbox binding, and everything else NEXT-SESSION lists.

## 7. Review gate

Merge authorizes only the schema-14 column, the priority incident notice, the user's `message.send` priority, and the one-dead-letter threshold with its pass; it does not authorize priority scheduling, tool-set priority, or desktop changes.
