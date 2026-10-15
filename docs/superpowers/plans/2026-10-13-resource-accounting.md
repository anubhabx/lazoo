# Resource Accounting Implementation Plan

**Goal:** Display what section 20 of the brief says Lazoo must display: each live employee session's native process tree use, measured separately from the daemon's own use, without storing or enforcing anything.

**Architecture:** No schema change. `lazoo-runtime` reads Job Object accounting (`JobObject::accounting` → `JobAccounting`) and the calling process's accounting (`current_process_accounting` → `ProcessAccounting`). Every provider session keeps its transport's job handle and answers `ProviderSession::resources`; the daemon's `ManagedSession` carries it through. `lazoo-protocol` gains `SessionResources`, `SessionResourcesParams`, and `DaemonResources`. `lazood` serves `session.resources` (live sessions only, else `LZ-SESSION-NOT-LIVE`) and `system.resources`; `lazooctl session resources` and `lazooctl system resources` map to them.

**Tech Stack:** Rust 1.98, `windows` 0.62 (`Win32_System_JobObjects`, `Win32_System_Threading`, `Win32_System_ProcessStatus`), clap, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-13-resource-accounting-design.md`

## Global Constraints

- No migration; schema stays 15. No change under `apps/lazoo-desktop`, no Tauri command, no `desktop.snapshot`, `system.status`, `session.show`, or `session.list` field.
- Point samples only: nothing stored, no event, no limit or rate control on any job.
- A provider's use is reported beside the daemon's and never counted against it.
- No provider smoke or live model use in acceptance.

---

### Task 1: Runtime and providers

- [x] `crates/lazoo-runtime/src/process_job.rs`: `JobAccounting`, `JobObject::accounting` (basic accounting plus extended limit peaks; `active_processes` now reads through it), `ProcessAccounting`, `current_process_accounting`; the `Win32_System_ProcessStatus` feature.
- [x] `crates/lazoo-providers`: `ProviderSession::resources` (default `None`); `JsonRpcProcess::job` and `NdjsonProcess::job`; the Codex, ACP (OpenCode, Grok Build), Antigravity, and Claude Code sessions keep their job and answer it; `ProviderSessionHandle` delegates.
- [x] `crates/lazoo-runtime/tests/process_job.rs`: a real process in a real job, live and after termination; the test process's own accounting.

### Task 2: Protocol, daemon, and CLI

- [x] `crates/lazoo-protocol/src/types.rs`: `SessionResourcesParams`, `SessionResources`, `DaemonResources`.
- [x] `apps/lazood/src/app.rs`: `ManagedSession::resources`, routes; `apps/lazood/src/app/resources.rs`: the two handlers.
- [x] `apps/lazooctl/src/lib.rs`: `session resources <id>`, `system resources`.
- [x] `apps/lazood/tests/daemon.rs` and `apps/lazooctl/tests/cli.rs`: the user's session through the daemon (fake session with fixed accounting), the not-live and not-found refusals, the unchanged snapshot, the CLI mapping, and the acceptance-script contract.

### Task 3: Acceptance and continuity

- [x] `scripts/acceptance/resource-accounting.ps1` writes `artifacts/acceptance/resource-accounting-manifest.json`.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-10-13-resource-accounting.md`.
