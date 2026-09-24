# Result Delivery Implementation Plan

**Goal:** Deliver child task outcomes and promoted incidents to the lead that owns the task tree after its turn has ended, without ever letting employee-authored traffic start a provider process.

**Architecture:** `lazoo-domain` renders the notice text. `lazoo-store` queues `result` and `status` notices idempotently, records the turn that delivers one on top of the existing turn ledger, and lets `task_wait` consume a result. The daemon runs a notice-delivery loop that wakes on every published event and reuses the turn delivery pipeline; `task_wait` acknowledges what it returns.

**Tech Stack:** Rust 1.98, Serde, rusqlite/SQLite WAL, Tokio broadcast wake, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-09-24-result-delivery-design.md`

## Global Constraints

- No schema change; no changes under `apps/lazoo-desktop`; no CLI change.
- A notice never activates a dormant employee, widens authority, retries, or deletes.
- Every queue, record, and consume is idempotent by child task, incident, or message id.
- No provider smoke or live model use in acceptance.

---

### Task 1: Domain rendering

- [x] `crates/lazoo-domain/src/delivery.rs`: `TaskResultNotice`, `IncidentNotice`, `render_task_result_text`, `render_incident_notice_text`, `MAX_NOTICE_OUTPUT_BYTES`.
- [x] Tests in `crates/lazoo-domain/tests/delivery.rs`.

### Task 2: Store notices

- [x] `crates/lazoo-store/src/delivery.rs`: `queue_task_result`, `pending_task_results`, `queue_incident_notice`, `pending_incident_notices`, `pending_lead_notices`, `record_notice_turn`, `consume_task_result`; `TaskResultWithoutParent`, `IncidentNoticeNotApplicable`, `MessageNotNotice` errors.
- [x] Crate-visible helpers in `message.rs`, `conversation.rs`, and `task.rs`.
- [x] Tests in `crates/lazoo-store/tests/delivery.rs`.

### Task 3: Daemon loop and tool hook

- [x] `apps/lazood/src/app/delivery.rs`: `run_notice_delivery`, `deliver_pending_notices_once`, per-notice delivery and cancellation rules.
- [x] `apps/lazood/src/app/tools.rs`: `task_wait` consumes the results it returns.
- [x] `apps/lazood/src/server.rs`: spawn and stop the loop beside the dispatcher.
- [x] Scenario tests in `apps/lazood/tests/daemon.rs` plus the acceptance-script contract test.

### Task 4: Acceptance and continuity

- [x] `scripts/acceptance/result-delivery.ps1` writes `artifacts/acceptance/result-delivery-manifest.json`.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-09-24-result-delivery.md`.
