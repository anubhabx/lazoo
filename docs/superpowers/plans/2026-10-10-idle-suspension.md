# Idle Suspension Implementation Plan

**Goal:** Let the local user give each employee an idle timeout, so the daemon stops that employee's idle session once no turn has touched it for the whole timeout, never while a turn on it is unfinished, while 0 stays the default and behaves exactly as before.

**Architecture:** `lazoo-protocol` adds the `idleTimeoutSeconds` employee field. `lazoo-domain` states the bound (`validate_idle_timeout`) and the rule (`idle_session_due`). `lazoo-store` adds schema 11 (one defaulted, checked column), carries the field through create, update, and reads, and adds `idle_sessions_due` and `begin_idle_session_stop`, which re-checks due-ness in the transaction that marks the session `stopping`. The daemon's new `idle.rs` sweep finishes each stop through the stop path `session.stop` already uses, and delivery resolves the recipient's current session instead of the one named when the turn was recorded. `lazooctl employee` exposes the flag.

**Tech Stack:** Rust 1.98, Serde, rusqlite/SQLite WAL, chrono, Tokio, clap, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-10-idle-suspension-design.md`

## Global Constraints

- One migration, `0011_idle_suspension.sql`, with a 0 default; no other schema change; no changes under `apps/lazoo-desktop`; no tool-catalog change.
- Employees with a timeout of 0, including every employee written before schema 11, behave as before.
- No session with an unfinished turn is ever stopped by the sweep; each session is stopped at most once under `idle-suspend:<sessionId>`; a failed stop is never retried.
- No provider smoke or live model use in acceptance.

---

### Task 1: Protocol and domain

- [x] `crates/lazoo-protocol/src/types.rs`: `idle_timeout_seconds` on `EmployeeRecord`, `CreateEmployeeParams` (serde default 0), and `UpdateEmployeeParams` (optional).
- [x] `crates/lazoo-domain/src/employee.rs`: `MIN_IDLE_TIMEOUT_SECONDS`, `MAX_IDLE_TIMEOUT_SECONDS`, `validate_idle_timeout`, `idle_session_due`, and the `IdleTimeoutOutOfRange` validation error.
- [x] Tests in `crates/lazoo-protocol/tests/contracts.rs` and `crates/lazoo-domain/tests/employee.rs`.

### Task 2: Store

- [x] `migrations/0011_idle_suspension.sql` registered in `crates/lazoo-store/src/migration.rs`.
- [x] `crates/lazoo-store/src/employee.rs`: the field on `CreateEmployeeInput` and `UpdateEmployeeInput`, insert, update, change detection, and both reads, appended last so no positional column index moves.
- [x] `crates/lazoo-store/src/session.rs`: `idle_sessions_due`, `begin_idle_session_stop` with `BeginIdleSessionStopInput`, the shared `mark_stopping` used by both stop commands, and `session_idle_due`.
- [x] Tests in `crates/lazoo-store/tests/employee.rs` and `crates/lazoo-store/tests/session.rs`; every `CreateEmployeeInput` and `UpdateEmployeeInput` literal carries the field; schema assertions raised to 11 in `crates/lazoo-store/tests/store.rs` and `crates/lazoo-store/tests/incident.rs`.

### Task 3: Daemon and CLI

- [x] `apps/lazood/src/app.rs`: `employee.create` and `employee.update` validate and pass the field; `stop_session_command` split so `finish_session_stop_command` serves both stop paths.
- [x] `apps/lazood/src/app/idle.rs`: `run_idle_suspension` (15-second sweep until shutdown) and `suspend_idle_sessions_once(now)`; `apps/lazood/src/server.rs` spawns the sweep and aborts it before `shutdown_live_sessions`.
- [x] `apps/lazood/src/app/turn.rs`: `delivery_target` resolves the recipient's current idle session from the roster instead of the session named on the turn.
- [x] `apps/lazooctl/src/lib.rs`: `--idle-timeout-seconds` on `employee create` (default 0) and `employee update`; `idleTimeoutSeconds` in both requests.
- [x] `apps/lazood/tests/daemon.rs`: `tool_harness_idle`, the suspension scenario, the unfinished-turn and zero-timeout scenario, the stranded-turn scenario, the `notice-turns` wake-after-suspension scenario, the acceptance-script contract, and schema assertions raised to 11. `apps/lazooctl/tests/cli.rs`: flag mapping.

### Task 4: Acceptance and continuity

- [x] `scripts/acceptance/idle-suspension.ps1` writes `artifacts/acceptance/idle-suspension-manifest.json`; every other acceptance script asserts schema 11.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-10-10-idle-suspension.md`.
