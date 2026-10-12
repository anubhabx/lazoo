# Message Arrival Implementation Plan

**Goal:** Wake `task_wait` and `message_send` when the store changes instead of polling it every 250 ms, with the same bounds and evidence.

**Architecture:** `App` holds `store_changed: Arc<Notify>`. `publish_events`, which follows every daemon store write, calls `notify_waiters`. Each wait iteration registers `notified()` before reading the store and parks on it or its deadline.

**Tech Stack:** Rust 1.98, Tokio (`sync::Notify`, paused-clock `test-util` in dev only), PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-11-message-arrival-design.md`

## Global Constraints

- No migration; schema stays 13. No change under `apps/lazoo-desktop`.
- No wait bound, result field, or acknowledgement evidence changes.
- No provider smoke or live model use in acceptance.

---

### Task 1: Daemon

- [x] `apps/lazood/src/app.rs`: the `store_changed` field, woken in `publish_events`; `register` publishes through `publish_events`.
- [x] `apps/lazood/src/app/tools.rs`: both waits register `notified()` before their reads and `select!` on it or the deadline; `WAIT_POLL_INTERVAL` removed.

### Task 2: Proof

- [x] `apps/lazood/Cargo.toml`: Tokio `test-util` for dev only.
- [x] `apps/lazood/tests/daemon.rs`: the paused-clock wake test and the acceptance-script contract.
- [x] `scripts/acceptance/message-arrival.ps1` writes `artifacts/acceptance/message-arrival-manifest.json`.

### Task 3: Continuity

- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-10-11-message-arrival.md`.
