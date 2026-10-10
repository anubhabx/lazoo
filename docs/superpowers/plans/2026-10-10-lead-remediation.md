# Bounded Lead Remediation Implementation Plan

**Goal:** Give the lead of a task tree a bounded first response to an incident promoted on one of its children (at most two remedies within five minutes), and turn whatever that response does not settle into a priority notification for the user, deterministically and without the daemon ever retrying, restarting, or reassigning work on its own.

**Architecture:** `lazoo-protocol` adds the escalation, reason, and remedy types and the incident fields. `lazoo-domain` states the bounds and the whole rule (`remediation_verdict`) and names the window in the lead's notice text. `lazoo-store` adds schema 12, decides `lead` or `user` at promotion, judges recurrences, closes windows in `escalate_due_incidents`, and applies a remedy atomically with its action through `cancel_task_tx` and `delegate_task_tx`, both split out of the existing commands. The daemon declares `incident_remedy`, reuses `task.cancel`'s dispatch-turn cancel, and runs the escalation sweep on the idle-suspension ticker. `lazooctl incident list` gains `--escalation`.

**Tech Stack:** Rust 1.98, Serde, rusqlite/SQLite WAL, chrono, Tokio, clap, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-10-lead-remediation-design.md`

## Global Constraints

- One migration, `0012_lead_remediation.sql`; no changes under `apps/lazoo-desktop`.
- A remedy row exists only for an action that committed with it; a refused remedy consumes no attempt.
- Remedies never start a provider process; escalation never changes work; only the user resolves an incident.
- No provider smoke or live model use in acceptance.

---

### Task 1: Protocol and domain

- [x] `crates/lazoo-protocol/src/incident.rs`: `IncidentEscalation`, `IncidentEscalationReason`, `IncidentRemedyKind`, `IncidentRemedyRecord`; five serde-defaulted fields on `IncidentRecord`; `escalation` on `IncidentListParams`.
- [x] `crates/lazoo-domain/src/incident.rs`: `MAX_INCIDENT_REMEDIES`, `INCIDENT_REMEDIATION_WINDOW_SECONDS`, `RemediationVerdict`, `remediation_verdict`.
- [x] `crates/lazoo-domain/src/delivery.rs`: `IncidentNotice.remediation_deadline_at` and the window sentence.
- [x] Tests in `crates/lazoo-protocol/tests/incident.rs`, `crates/lazoo-domain/tests/incident.rs`, and `crates/lazoo-domain/tests/delivery.rs`.

### Task 2: Store

- [x] `migrations/0012_lead_remediation.sql` registered in `crates/lazoo-store/src/migration.rs`.
- [x] `crates/lazoo-store/src/task.rs`: `delegate_task_tx` and `cancel_task_tx` split out of `delegate_task` and `cancel_task` without behavior change.
- [x] `crates/lazoo-store/src/incident.rs`: escalation columns on every read, remedies loaded with each incident, the `escalation` list filter, promotion and recurrence escalation in `raise_incident`, `escalate_due_incidents`, `apply_incident_remedy` with `ApplyIncidentRemedyInput`, `IncidentRemedyAction`, and `IncidentRemedyResult`, and the shared `incident_lead_root`.
- [x] `crates/lazoo-store/src/delivery.rs`: `queue_incident_notice_tx` uses `incident_lead_root` and carries the deadline into the notice.
- [x] `crates/lazoo-store/src/lib.rs`: `IncidentRemedyClosed`, `IncidentRemedyScope`, `IncidentRemedyTarget`.
- [x] Tests in `crates/lazoo-store/tests/delivery.rs` (both windows, the bounds, no-lead promotion), `crates/lazoo-store/tests/incident.rs` (task-less promotion), and `crates/lazoo-store/tests/store.rs` (schema-11 upgrade); schema assertions raised to 12.

### Task 3: Daemon and CLI

- [x] `apps/lazood/src/app/tools.rs`: the `incident_remedy` spec, arguments, handler, and `remedy_store_error`; `incident_list` shows escalation, deadline, and remedies applied.
- [x] `apps/lazood/src/app/task.rs`: `cancel_dispatch_turn` shared by `task.cancel` and the remedy.
- [x] `apps/lazood/src/app/incident.rs`: `escalate_due_incidents_once(now)` and the `escalation` filter on `incident.list`.
- [x] `apps/lazood/src/app/idle.rs` and `apps/lazood/src/server.rs`: `run_timed_sweeps` runs idle suspension and escalation on one 15-second ticker.
- [x] `apps/lazooctl/src/lib.rs`: `incident list --escalation`.
- [x] `apps/lazood/tests/daemon.rs`: the notice-turn remedy scenario, the sweep scenario, the acceptance-script contract, the nine-tool catalog, and schema 12. `apps/lazooctl/tests/cli.rs`: filter mapping.

### Task 4: Acceptance and continuity

- [x] `scripts/acceptance/lead-remediation.ps1` writes `artifacts/acceptance/lead-remediation-manifest.json`; every other acceptance script asserts schema 12.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-10-10-lead-remediation.md`.
