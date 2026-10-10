# Incident Thresholds Implementation Plan

**Goal:** Apply the brief's section 14.2 default thresholds: a stall, a daemon crash, or an employee process death under assigned work is promoted by its first occurrence, unexpected exits by their second, and everything else by its third as before.

**Architecture:** `lazoo-domain` adds `incident_threshold(code, namesTask)` and makes `incident_threshold_crossed` take the code and task flag. `lazoo-store`'s `raise_incident` uses it on fold and, for a new incident, inserts it as `priority` with the promotion escalation when its threshold is one.

**Tech Stack:** Rust 1.98, rusqlite/SQLite WAL, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-10-incident-thresholds-design.md`

## Global Constraints

- No migration, no protocol, tool, CLI, RPC, or desktop change.
- Promotion never retries, restarts, or re-authenticates anything.
- No provider smoke or live model use in acceptance.

---

### Task 1: Domain

- [x] `crates/lazoo-domain/src/incident.rs`: `incident_threshold` and the three-argument `incident_threshold_crossed`; exported from `lib.rs`.
- [x] `crates/lazoo-domain/tests/incident.rs`: the threshold table.

### Task 2: Store

- [x] `crates/lazoo-store/src/incident.rs`: fold uses the code's threshold; a new incident whose threshold is one is inserted as `priority` with `promotion_escalation` and emits `incident.escalated`.
- [x] `crates/lazoo-store/tests/incident.rs`: task-less stalls and crashes promoted at once, a second unexpected exit promoted, the generic tests moved to a default-threshold code. `crates/lazoo-store/tests/delivery.rs`: a stall or death on a child opens the lead window at once.

### Task 3: Daemon and acceptance

- [x] `apps/lazood/tests/daemon.rs`: the keeper crash folded at startup is priority and the user's; the acceptance-script contract.
- [x] `scripts/acceptance/incident-thresholds.ps1` writes `artifacts/acceptance/incident-thresholds-manifest.json`.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-10-10-incident-thresholds.md`.
