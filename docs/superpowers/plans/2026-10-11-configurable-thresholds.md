# Configurable Thresholds Implementation Plan

**Goal:** Let the local user override section 14.2's persistent-failure thresholds per code without a rebuild, while a death under assigned work and a dead-lettered priority message keep promoting at once.

**Architecture:** Schema 15 adds `incident_thresholds`. `lazoo-domain` replaces the count-only rule with `IncidentThreshold { occurrences, window_seconds }`, `default_incident_threshold`, `incident_threshold(code, facts, configured)`, a windowed `incident_window`, `validate_incident_threshold`, and `DESCRIBED_INCIDENT_CODES`. `lazoo-store` reads the override inside `raise_incident` and adds list, set, and reset. `lazood` serves `incident.threshold.list/set/reset`; `lazooctl incident threshold` maps to them. No tool gains access.

**Tech Stack:** Rust 1.98, rusqlite/SQLite WAL, clap, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-11-configurable-thresholds-design.md`

## Global Constraints

- One migration, `0015_incident_thresholds.sql`; no change under `apps/lazoo-desktop`.
- Only the local user writes thresholds; no collaboration tool reads or sets them.
- A change never demotes, resolves, or re-escalates an incident; no provider smoke or live model use in acceptance.

---

### Task 1: Protocol, domain, and store

- [x] `crates/lazoo-protocol/src/incident.rs`: `IncidentThresholdSource`, `IncidentThresholdRecord`, `IncidentThresholdMutationResult`, `SetIncidentThresholdParams`, `ResetIncidentThresholdParams`.
- [x] `crates/lazoo-domain/src/incident.rs`: the threshold type and rule, the windowed `incident_window`, validation, and the described-code list.
- [x] `migrations/0015_incident_thresholds.sql` registered in `crates/lazoo-store/src/migration.rs`; `crates/lazoo-store/src/incident.rs`: the override inside `raise_incident`, `list_incident_thresholds`, `set_incident_threshold`, `reset_incident_threshold`.
- [x] Tests in `crates/lazoo-protocol/tests/incident.rs`, `crates/lazoo-domain/tests/incident.rs`, and `crates/lazoo-store/tests/incident.rs`; schema assertions raised to 15.

### Task 2: Daemon and CLI

- [x] `apps/lazood/src/app.rs` routes and `apps/lazood/src/app/incident.rs` handlers for the three methods.
- [x] `apps/lazooctl/src/lib.rs`: `incident threshold list|set|reset`.
- [x] `apps/lazood/tests/daemon.rs` and `apps/lazooctl/tests/cli.rs`: the user's session through the daemon, the tool-catalog boundary, the CLI mapping, and the acceptance-script contract.

### Task 3: Acceptance and continuity

- [x] `scripts/acceptance/configurable-thresholds.ps1` writes `artifacts/acceptance/configurable-thresholds-manifest.json`; every other acceptance script asserts schema 15.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-10-11-configurable-thresholds.md`.
