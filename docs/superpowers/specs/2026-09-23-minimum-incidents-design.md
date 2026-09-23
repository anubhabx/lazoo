# Minimum Incidents Design

**Date:** 2026-09-23

**Status:** Implemented on `feat/minimum-incidents` (based on `feat/collaboration-tools`) with deterministic acceptance, for owner review before merge.

**Scope:** Backend-only. Schema 9 adds the human-readable incident ledger; the daemon folds committed failures into it, exposes `incident.list|show|acknowledge|resolve`, and declares `incident_report` and `incident_list` to Codex sessions; `lazooctl incident` maps the RPCs. No desktop or Tauri change, no lead notification or bounded remediation, no user notification channel, no live model use.

## 1. Outcome

Before this slice a failure left only a `failure_code` on its own row (session, turn, message, task dispatch, approval) plus a raw count of keeper emergency records. An operator had to read several ledgers to learn that one employee had stalled three times in five minutes, and an employee had no way to see or report an operational problem.

Lazoo now answers four questions without inference:

1. Which operational problem is open right now, in plain language, with the next safe action?
2. How many equivalent failures folded into it, and did they cross the threshold that promotes it to `priority`?
3. Which employee, project, task, session, turn, or message does it point at, and what did the provider actually say?
4. Has the local user acknowledged or resolved it, at which revision?

The failure row stays authoritative. An incident is derived evidence that names the situation; it never changes authority, retries work, or hides the underlying record.

## 2. Fixed boundaries

- Only the local user settles incidents. `incident.acknowledge` and `incident.resolve` are revision-checked, idempotent, and reject any other transition.
- Incidents never retry, restart, re-authenticate, change authority, or delete anything. They record what Lazoo did and the recommended next action.
- Every raise is idempotent by command key, so a replayed RPC, a repeated dispatch attempt, or a second daemon boot never double counts.
- Equivalent failures share one row: the fingerprint is SHA-256 of code, employee, and project. A resolved incident never reopens; a new occurrence opens a new row.
- Employee tools act only as the turn's recipient endpoint. `incident_list` shows incidents in the caller's project or about the caller; `incident_report` records `LZ-EMPLOYEE-REPORTED` with bounded text and cannot set a code, priority, or state.
- `desktop.snapshot`, Tauri commands and capabilities, and `apps/lazoo-desktop` are byte-for-byte unchanged; the snapshot keeps only its existing emergency-record count.

## 3. Where incidents come from

| Source | Code | Raised when |
|---|---|---|
| `session.warm` / task dispatch | `LZ-SESSION-START-FAILED` | the provider session never became ready or reported no owned process |
| turn delivery | `LZ-SESSION-STALLED`, `LZ-MESSAGE-TIMEOUT`, `LZ-PROCESS-EXIT`, `LZ-PROVIDER-AUTH`, `LZ-PROVIDER-INCOMPATIBLE`, `LZ-TURN-PROVIDER` | the provider rejected, stalled, timed out, exited, or returned no completion evidence; the turn keeps its own failure code in the detail |
| `message.fail` | `LZ-MESSAGE-DEADLETTER` | a delivery exhausted its attempts or failed non-retryably |
| task dispatch | `LZ-WORKTREE-CONFLICT`, `LZ-CONFIG-INCOMPATIBLE`, or the dispatch failure code | workspace preparation failed or configuration drifted before provider I/O |
| daemon restart | `LZ-SESSION-OWNER-LOST`, `LZ-MESSAGE-OWNER-LOST`, `LZ-TURN-OWNER-LOST`, `LZ-TASK-DISPATCH-OWNER-LOST` | reconciliation closed or marked uncertain work owned by a previous boot |
| keeper spool | `LZ-DAEMON-UNAVAILABLE` (or the spool code) | `emergency_incident_imports` rows fold in at startup, once per record |
| employee | `LZ-EMPLOYEE-REPORTED` | `incident_report` from a daemon-managed Codex session |

Incidents are raised immediately after the failure transaction commits, in their own immediate transaction. The failure row is therefore never missing its incident because of a partial write inside one transaction; a crash between the two leaves the failure row authoritative and unfolded. Restart reconciliation raises its own incidents for owner loss, so the common crash path is covered. Folding the raise into each failure transaction is a later upgrade.

## 4. Thresholds

`lazoo-domain` owns the rule: three equivalent occurrences inside a five-minute window promote the incident to `priority` and emit `incident.escalated`. The window restarts when more than five minutes pass since it started or when clocks move backwards. Priority never demotes; resolution closes the row. Lead notification, bounded remediation, and user notifications are deferred.

## 5. Durable records

Schema 9 adds `incidents` (one row per fingerprint while open or acknowledged) and `incident_commands` (idempotency for raise, acknowledge, resolve). Each row carries contract version, code, fingerprint, title, explanation, next action, optional scope ids, source (`daemon`, `keeper`, `employee`), bounded detail, occurrence and window counters, first/last/window timestamps, priority, state (`open`, `acknowledged`, `resolved`), revision, and settle timestamps. Every effective mutation writes one audit event (`incident.raised`, `incident.recurred`, `incident.escalated`, `incident.acknowledged`, `incident.resolved`) and one outbox row in the same transaction. A recurrence reopens an acknowledged incident and replaces its latest scope and detail.

## 6. RPC, CLI, and tools

- `incident.list` with optional `state`, `priority`, `code`, `employeeId`, `projectId`, `afterSequence`, and a 1..=1024 limit; `incident.show`; `incident.acknowledge` and `incident.resolve` with `id`, `expectedRevision`, `idempotencyKey`.
- `lazooctl incident list|show|acknowledge|resolve`; `LZ-INCIDENT-*` maps to CLI exit code 15.
- `incident_report {title, explanation?, detail?}` and `incident_list {}` join the Codex collaboration tool catalog.

## 7. Deterministic acceptance

`scripts/acceptance/minimum-incidents.ps1` runs focused domain, store, daemon fake-backend, and CLI proofs and confirms the desktop tree is unchanged. It must show the taxonomy text, the threshold window, fold/escalate/replay semantics, revisioned acknowledge and resolve, keeper spool folding exactly once, session start failure, provider failure escalation settled through RPC, dead-letter and dispatch-failure scope, restart owner loss folded once per employee, employee tools scoped to the caller, CLI mapping and exit code 15, and schema 9.

## 8. Explicitly deferred work

- structured lead notification of promoted incidents and bounded remediation (needs activation controls);
- user notification channels beyond the ledger and CLI;
- per-code thresholds ("two unexpected exits", "one dead-lettered priority message") beyond the generic rule;
- raising inside the failure transaction;
- typed desktop Inbox binding for incidents;
- other-provider incident sources beyond the daemon-managed Codex path.

## 9. Review gate

The owner should review this design together with the branch. Merge authorizes only the backend incident ledger and its deterministic acceptance.
