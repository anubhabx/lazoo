# Incident Thresholds Design

**Date:** 2026-10-10

**Status:** Implemented on `feat/incident-thresholds` (stacked on `feat/lead-remediation`, pull request #3) with deterministic acceptance, for owner review in a pull request before merge.

**Scope:** Backend-only. No schema change. `lazoo-domain` gains a per-code persistent-failure threshold, and the store applies it when an incident is first recorded as well as when it folds. No desktop, tool, CLI, or RPC shape change, no live model use.

## 1. Outcome

Before this slice every incident code was promoted to `priority` by the same rule: three equivalent failures inside five minutes. The brief's section 14.2 names stricter defaults for the failures that matter most, and with bounded lead remediation in place a promotion now has consequences (a lead window or a user notification), so treating a stall like a one-off timeout delayed both by two more failures.

Lazoo now promotes:

| Failure | Code | Promoted by |
|---|---|---|
| Missed progress evidence past the adapter's deadline | `LZ-SESSION-STALLED` | the first occurrence |
| A daemon crash recorded by the keeper | `LZ-DAEMON-UNAVAILABLE` | the first occurrence |
| An employee process that dies while it holds assigned work (the incident names a task) | `LZ-PROCESS-EXIT` | the first occurrence |
| Unexpected exits of the same employee process | `LZ-PROCESS-EXIT` without a task | the second occurrence |
| Everything else | any other code | the third occurrence, as before |

All counts are of equivalent failures (same code, employee, and project) inside the existing five-minute window.

## 2. Rule

`incident_threshold(code, namesTask)` in `lazoo-domain` is the table above, and `incident_threshold_crossed(code, namesTask, windowOccurrences)` replaces the single-argument check. `INCIDENT_THRESHOLD_OCCURRENCES` (3) stays the default. For a fold, `namesTask` is whether the incident's scope names a task after this occurrence (the occurrence's own task, else the incident's last one).

## 3. Born-priority incidents

A code whose threshold is one is promoted by the occurrence that creates the incident. The store inserts it as `priority` and applies the same promotion escalation as a fold does: a lead window when it is about a task below an active root and not about that root's own employee, otherwise `escalation: user` with reason `no-lead`. Its one event is `incident.escalated` (not `incident.raised`), so every promotion is announced by the same event type. Later occurrences fold into it as recurrences exactly as before, and are judged against the lead's window.

## 4. Rules kept

- The fold, the window, acknowledge, resolve, the one notice per incident, and the whole remediation and escalation path are unchanged; they simply see some promotions sooner.
- A stall, crash, or death is never retried, restarted, or re-authenticated because of its threshold; promotion only tells the lead (or the user).

## 5. Deterministic acceptance

`scripts/acceptance/incident-thresholds.ps1` runs domain, store, and daemon fake-backend proofs and confirms the desktop tree is unchanged, writing `artifacts/acceptance/incident-thresholds-manifest.json`. No provider smoke or live model turn.

## 6. Explicitly deferred work

- "one dead-lettered priority message": message envelopes have no priority field, so there is nothing to key it on; it waits for a reviewed message-priority contract;
- configurable thresholds and windows (project, employee, or global), which need a settings surface;
- user notification channels, and everything else NEXT-SESSION lists.

## 7. Review gate

Merge authorizes only the per-code default thresholds above and born-priority promotion; it does not authorize configurable thresholds, a message priority field, or any other capability.
