# Activation Policy Implementation Plan

**Goal:** Let the local user opt one employee at a time into bounded automatic activation, so a recorded notice turn may start one session for a dormant `notice-turns` recipient, deliver that turn on its own task, and stop the session, while `manual` stays the default and behaves exactly as before.

**Architecture:** `lazoo-protocol` adds the `ActivationPolicy` enum and the `activationPolicy` employee field. `lazoo-domain` states the one rule (`notice_turn_may_activate`). `lazoo-store` adds schema 10 (one defaulted column) and carries the field through create, update, and reads. The daemon's notice pass, on the dormant-recipient error it already recognises, consults the policy, starts one session keyed to the turn, delivers, and stops. `lazooctl employee` exposes the flag.

**Tech Stack:** Rust 1.98, Serde, rusqlite/SQLite WAL, Tokio, clap, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-09-28-activation-policy-design.md`

## Global Constraints

- One migration, `0010_activation_policy.sql`, with a `manual` default; no other schema change; no changes under `apps/lazoo-desktop`; no tool-catalog change.
- `manual` employees, including every employee written before schema 10, behave byte-for-byte as before.
- At most one activation per recorded notice turn; the session is stopped when the turn ends; a failed activation is never retried.
- No provider smoke or live model use in acceptance.

---

### Task 1: Protocol and domain

- [x] `crates/lazoo-protocol/src/types.rs`: `ActivationPolicy { Manual (default), NoticeTurns }` in kebab-case; `activation_policy` on `EmployeeRecord`, `CreateEmployeeParams` (serde default), and `UpdateEmployeeParams` (optional).
- [x] `crates/lazoo-domain/src/delivery.rs`: `notice_turn_may_activate`.
- [x] Tests in `crates/lazoo-protocol/tests/contracts.rs` and `crates/lazoo-domain/tests/delivery.rs`.

### Task 2: Store

- [x] `migrations/0010_activation_policy.sql` registered in `crates/lazoo-store/src/migration.rs`.
- [x] `crates/lazoo-store/src/employee.rs`: the field on `CreateEmployeeInput` and `UpdateEmployeeInput`, insert, update, change detection, and both reads.
- [x] Tests in `crates/lazoo-store/tests/employee.rs`; schema assertions raised to 10 in `crates/lazoo-store/tests/store.rs` and `crates/lazoo-store/tests/incident.rs`.

### Task 3: Daemon and CLI

- [x] `apps/lazood/src/app.rs`: `employee.create` and `employee.update` pass the field through.
- [x] `apps/lazood/src/app/delivery.rs`: `activate_for_notice_turn` on `LZ-TURN-SESSION-NOT-READY`: policy check, `begin_session` keyed `notice-activation:<turnId>`, replay means wait, `start_new_session`, deliver, `stop_session_command`.
- [x] `apps/lazooctl/src/lib.rs`: `--activation-policy manual|notice-turns` on `employee create` (default `manual`) and `employee update`; `activationPolicy` in both requests.
- [x] `apps/lazood/tests/daemon.rs`: policy-aware tool harness, scripted start refusal on the fake backend, the activation scenario, the failed-activation scenario, the acceptance-script contract, and schema assertions raised to 10. `apps/lazooctl/tests/cli.rs`: flag mapping.

### Task 4: Acceptance and continuity

- [x] `scripts/acceptance/activation-policy.ps1` writes `artifacts/acceptance/activation-policy-manifest.json`; every other acceptance script asserts schema 10.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-09-28-activation-policy.md`.
