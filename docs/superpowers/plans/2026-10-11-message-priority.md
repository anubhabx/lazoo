# Message Priority Implementation Plan

**Goal:** Give every message a priority and make one dead-lettered priority message a priority incident (brief sections 11.1 and 14.2), whatever path dead-lettered it.

**Architecture:** Schema 14 adds `messages.priority`. `lazoo-protocol` adds `MessagePriority` to the envelope, summary, and `message.send` params. `lazoo-store` writes it on every insert (incident notices `priority`, the user's `message.send` as asked, everything else `normal`), reads it on every read, derives `priority_message` inside `raise_incident` from the message and its dead letter, and lists priority dead letters without an incident. `lazoo-domain` adds `IncidentFacts` to the threshold rule. `lazood` raises the dead-letter incident through one helper from `message.fail` and from `note_priority_dead_letters_once` at startup and in the timed sweep. `lazooctl message send` gains `--priority`.

**Tech Stack:** Rust 1.98, rusqlite/SQLite WAL (JSON functions for the backfill), Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-11-message-priority-design.md`

## Global Constraints

- One migration, `0014_message_priority.sql`; no change under `apps/lazoo-desktop`.
- No collaboration tool can read or set priority; delivery order is unchanged.
- One incident per dead letter; no provider smoke or live model use in acceptance.

---

### Task 1: Protocol, domain, and store

- [x] `crates/lazoo-protocol/src/message.rs`: `MessagePriority` and the serde-defaulted `priority` fields.
- [x] `crates/lazoo-domain/src/incident.rs`: `IncidentFacts` and the priority dead-letter threshold.
- [x] `migrations/0014_message_priority.sql` registered in `crates/lazoo-store/src/migration.rs`.
- [x] `crates/lazoo-store/src/message.rs`, `delivery.rs`, `conversation.rs`, `task.rs`: priority on every insert and read.
- [x] `crates/lazoo-store/src/incident.rs`: `priority_dead_letter`, `unnoted_priority_dead_letters`, `dead_letter_incident_key`.
- [x] Tests in `crates/lazoo-protocol/tests/contracts.rs`, `crates/lazoo-domain/tests/incident.rs`, `crates/lazoo-store/tests/incident.rs`, and `crates/lazoo-store/tests/store.rs` (with the shared `database_at` helper); schema assertions raised to 14.

### Task 2: Daemon and CLI

- [x] `apps/lazood/src/app/incident.rs`: `note_dead_letter` and `note_priority_dead_letters_once`; `app.rs`: `message.send` passes priority and `message.fail` uses the helper; `idle.rs`: the sweep runs the pass first; startup runs it after reconciliation.
- [x] `apps/lazooctl/src/lib.rs`: `message send --priority`.
- [x] `apps/lazood/tests/daemon.rs` and `apps/lazooctl/tests/cli.rs`: the lost-notice sweep, the `message.fail` path, the CLI mapping, and the acceptance-script contract.

### Task 3: Acceptance and continuity

- [x] `scripts/acceptance/message-priority.ps1` writes `artifacts/acceptance/message-priority-manifest.json`; every other acceptance script asserts schema 14.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-10-11-message-priority.md`.
