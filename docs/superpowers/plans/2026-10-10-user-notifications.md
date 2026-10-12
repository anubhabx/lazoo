# User Notifications Implementation Plan

**Goal:** Tell the local user, once and without flooding the desktop, about every incident that reaches `escalation: user`, through the Windows notification the keeper already uses.

**Architecture:** Schema 13 adds `incidents.user_notified_at` and `user_notification`. `lazoo-store` claims unannounced open user escalations in one transaction (settling keeper records as `keeper`) and settles the outcome only on `claimed` rows. `lazood` adds the `UserNotifier` trait (default `NativeNotifier`, replaceable with `App::with_user_notifier`) and `notify_user_escalations_once`, which shows one notification per pass from the timed sweep.

**Tech Stack:** Rust 1.98, rusqlite/SQLite WAL, the `windows` toast API through `lazoo-runtime`, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-10-user-notifications-design.md`

## Global Constraints

- One migration, `0013_user_notifications.sql`; no change under `apps/lazoo-desktop`.
- At most one announcement per incident; one notification per pass; no retry.
- No test shows a real notification; no provider smoke or live model use in acceptance.

---

### Task 1: Protocol and store

- [x] `crates/lazoo-protocol/src/incident.rs`: `IncidentUserNotification` and the two `IncidentRecord` fields.
- [x] `migrations/0013_user_notifications.sql` registered in `crates/lazoo-store/src/migration.rs`.
- [x] `crates/lazoo-store/src/incident.rs`: both columns on every read, `claim_user_notifications`, `settle_user_notifications`.
- [x] Tests in `crates/lazoo-protocol/tests/incident.rs`, `crates/lazoo-store/tests/incident.rs`, and `crates/lazoo-store/tests/store.rs`; schema assertions raised to 13.

### Task 2: Daemon

- [x] `apps/lazood/src/app/incident.rs`: `UserNotifier`, its `NativeNotifier` implementation, and `notify_user_escalations_once`; `apps/lazood/src/app.rs`: the `user_notifier` field and `with_user_notifier`; `apps/lazood/src/app/idle.rs`: the sweep runs it.
- [x] `apps/lazood/tests/daemon.rs`: the recording-notifier scenario and the acceptance-script contract.

### Task 3: Acceptance and continuity

- [x] `scripts/acceptance/user-notifications.ps1` writes `artifacts/acceptance/user-notifications-manifest.json`; every other acceptance script asserts schema 13.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-10-10-user-notifications.md`.
