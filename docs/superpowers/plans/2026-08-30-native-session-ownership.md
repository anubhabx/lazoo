# Native Session Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans and superpowers:test-driven-development task-by-task. Do not delegate unless the owner explicitly requests parallel agents.

**Goal:** Add durable employee-session lifecycle and creation-time Windows Job Object containment without changing the desktop or starting model turns.

**Architecture:** A protocol-transparent gated host preserves current provider transports while placing each native process tree in a dedicated kill-on-close Job Object before provider launch. Migration 3 persists two-phase session state and command identity. The daemon owns live adapter handles, reconciles prior-boot state, and exposes narrow warm/list/show/stop CLI operations.

**Tech Stack:** Rust 1.98, windows-rs 0.62, Tokio, Serde, Rusqlite/SQLite WAL, Clap, existing provider adapters and authenticated JSON-RPC named pipe.

---

### Task 1: Creation-time Job Object substrate

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/lazoo-runtime/Cargo.toml`
- Create: `crates/lazoo-runtime/src/process_job.rs`
- Modify: `crates/lazoo-runtime/src/lib.rs`
- Create: `apps/lazoo-session-host/Cargo.toml`
- Create: `apps/lazoo-session-host/src/main.rs`
- Create: `tests/fixtures/session-tree/Cargo.toml`
- Create: `tests/fixtures/session-tree/src/main.rs`
- Create: `tests/fixtures/job-containment/Cargo.toml`
- Create: `tests/fixtures/job-containment/src/main.rs`
- Test: `crates/lazoo-runtime/tests/process_job.rs`

- [x] Write failing lifecycle/containment contract tests first.
- [x] Implement kill-on-close `JobObject`, random launch gate, and `JobChild` wrapper.
- [x] Implement the transparent host and deterministic root/descendant fixture.
- [x] Prove the host waits for assignment, both fixture PIDs belong to the job, and job termination removes both.
- [x] Commit as `feat: contain native session process trees`.

### Task 2: Route provider session processes through the host

**Files:**
- Modify: `crates/lazoo-providers/Cargo.toml`
- Modify: `crates/lazoo-providers/src/json_rpc.rs`
- Modify: `crates/lazoo-providers/src/ndjson.rs`
- Modify: `crates/lazoo-providers/src/contract.rs`
- Modify: provider session implementations and contract tests

- [x] Write failing tests for host PID exposure and complete-tree forced stop.
- [x] Replace direct session `tokio::process::Child` ownership with `JobChild`.
- [x] Expose only the host PID through `ProviderSession`; retain protocol behavior.
- [x] Run provider fixture tests and the existing provider conformance suite.
- [x] Commit as `feat: supervise provider session trees`.

### Task 3: Session contracts, migration 3, and roster states

**Files:**
- Create: `migrations/0003_employee_sessions.sql`
- Modify: `crates/lazoo-protocol/src/types.rs`
- Modify: `crates/lazoo-protocol/tests/contracts.rs`
- Modify: `crates/lazoo-domain/src/roster.rs`
- Modify: `crates/lazoo-domain/tests/roster.rs`
- Create: `crates/lazoo-store/src/session.rs`
- Modify: `crates/lazoo-store/src/lib.rs`
- Modify: `crates/lazoo-store/src/migration.rs`
- Create: `crates/lazoo-store/tests/session.rs`
- Modify: `crates/lazoo-store/tests/store.rs`

- [x] Write failing strict-contract, migration, idempotency, stale-revision, and reconciliation tests.
- [x] Add durable session rows and command-to-session mappings in schema 3.
- [x] Implement begin/ready/fail/stop/suspend/reconcile transactions with audit/outbox and roster revisions.
- [x] Map latest session state into truthful roster availability.
- [x] Commit as `feat: persist native session ownership`.

### Task 4: Daemon and CLI warm/stop workflow

**Files:**
- Modify: `apps/lazood/src/app.rs`
- Modify: `apps/lazood/src/server.rs`
- Modify: `apps/lazood/tests/daemon.rs`
- Modify: `apps/lazooctl/src/lib.rs`
- Modify: `apps/lazooctl/tests/cli.rs`
- Modify: `crates/lazoo-runtime/src/data_root.rs`

- [x] Write failing daemon/CLI tests using an injected fake session backend.
- [x] Add shared live-handle ownership to `App` and startup reconciliation.
- [x] Implement `session.warm/list/show/stop` with stable `LZ-SESSION-*` errors and CLI exit code 10.
- [x] Gracefully stop live sessions during planned daemon shutdown.
- [x] Keep `desktop.snapshot` and Tauri capability scope unchanged.
- [x] Commit as `feat: control native employee sessions`.

### Task 5: Crash/restart acceptance and closeout

**Files:**
- Create: `scripts/acceptance/native-session-ownership.ps1`
- Create: `artifacts/acceptance/native-session-ownership-manifest.json`
- Create: `docs/verification/2026-08-30-native-session-ownership.md`
- Modify: `scripts/acceptance/first-slice.ps1`
- Modify: `docs/NEXT-SESSION.md`
- Modify: `docs/PROJECT-BRIEF.md`
- Modify: `README.md`

- [ ] Write the backend-only acceptance contract test and verify RED.
- [ ] Verify creation-time root/descendant containment with release fixtures.
- [ ] Warm a real installed provider without sending a prompt, verify idempotency and idle truth, then stop it.
- [ ] Force daemon loss, prove the job removes the tree, restart, and verify `LZ-SESSION-OWNER-LOST` reconciliation.
- [ ] Run the complete workspace, lint, format, locked release, original control-plane, roster, and new session acceptances.
- [ ] Record exact evidence and exclusions; commit as `test: verify native session ownership`.
