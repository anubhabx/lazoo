# Minimum Incidents Implementation Plan

**Goal:** Turn committed failure records into one human-readable, threshold-aware incident per code, employee, and project that the local user can acknowledge and resolve and that employees can read and report through their collaboration tools.

**Architecture:** `lazoo-domain` owns the code taxonomy text, validation, fingerprint, and threshold window. `lazoo-store` owns schema 9 (`incidents`, `incident_commands`) with fold-or-open raising, revisioned settling, listing, and keeper spool folding. The daemon raises incidents right after each failure commit and at startup, routes the RPCs, and adds two tools; `lazooctl incident` maps them.

**Tech Stack:** Rust 1.98, Serde, rusqlite/SQLite WAL, JSON-RPC over the authenticated named pipe, Clap CLI, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-09-23-minimum-incidents-design.md`

## Global Constraints

- No changes under `apps/lazoo-desktop`; `desktop.snapshot` and Tauri capabilities are unchanged.
- Incidents never retry, restart, re-authenticate, change authority, or delete anything.
- Every raise is idempotent by command key; only the local user settles incidents.
- No provider smoke or live model use in acceptance.

---

### Task 1: Domain and protocol

- [x] `crates/lazoo-domain/src/incident.rs`: `describe_incident`, validation, `incident_fingerprint`, `incident_window`, `incident_threshold_crossed`.
- [x] `crates/lazoo-protocol/src/incident.rs`: `IncidentState`, `IncidentPriority`, `IncidentSource`, `IncidentRecord`, mutation result, list/show/acknowledge/resolve params.
- [x] Tests: `crates/lazoo-domain/tests/incident.rs`, `crates/lazoo-protocol/tests/incident.rs`.

### Task 2: Schema 9 and store ledger

- [x] `migrations/0009_incidents.sql`.
- [x] `crates/lazoo-store/src/incident.rs`: `raise_incident`, `acknowledge_incident`, `resolve_incident`, `get_incident`, `list_incidents`, `reconcile_emergency_incidents`; `LZ-INCIDENT-*` store errors.
- [x] Tests in `crates/lazoo-store/tests/incident.rs`; schema assertions advance to 9.

### Task 3: Daemon

- [x] `apps/lazood/src/app/incident.rs`: `note_incident`, session/provider/startup helpers, provider-error and dispatch-failure classification, RPC handlers, error mapping.
- [x] Raise sites: `record_session_failure`, `fail_message` dead letters, turn rejection/uncertainty/no-evidence/cancel-uncertain, dispatch failure before provider I/O, startup reconciliation and keeper spool fold.
- [x] `apps/lazood/src/app/tools.rs`: `incident_report`, `incident_list`.
- [x] Tests in `apps/lazood/tests/daemon.rs` plus the acceptance-script contract test.

### Task 4: CLI

- [x] `lazooctl incident list|show|acknowledge|resolve`; `LZ-INCIDENT-*` maps to exit code 15.
- [x] Tests in `apps/lazooctl/tests/cli.rs`.

### Task 5: Acceptance and continuity

- [x] `scripts/acceptance/minimum-incidents.ps1` writes `artifacts/acceptance/minimum-incidents-manifest.json`; every other acceptance script asserts schema 9.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-09-23-minimum-incidents.md`.
