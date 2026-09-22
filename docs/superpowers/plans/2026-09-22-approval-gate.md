# Approval Gate Implementation Plan

**Goal:** Replace the Codex adapter's hard-coded `decline` with a durable, policy-driven approval gate that the local user controls under `ask`, that policy answers under `self-decision` and `full-autonomy`, and that fails closed on expiry, cancellation, and daemon loss.

**Architecture:** The adapter contract gains a provider-neutral `PermissionRequest` and an async `decide_permission` observer hook. The Codex adapter normalizes native approval requests, awaits the daemon's decision, answers Codex, and only then surfaces failures. The daemon records the request first, applies the domain policy, parks the provider while the user decides, and closes the row itself on expiry or turn cancellation. Schema 8 adds the approval ledger.

**Tech Stack:** Rust 1.98, Serde, rusqlite/SQLite WAL, JSON-RPC over the authenticated named pipe, Clap CLI, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-09-22-approval-gate-design.md`

## Global Constraints

- No changes under `apps/lazoo-desktop`; `desktop.snapshot` and Tauri capabilities are unchanged.
- No session-wide or prefix grants; one decision binds one fingerprinted action at one revision.
- Every failure, timeout, cancellation, and restart path answers the provider `decline`.
- Codex is always answered before a decision error is propagated.
- No provider smoke or live model use in acceptance.

---

### Task 1: Protocol and domain contracts

- [x] `crates/lazoo-protocol/src/approval.rs`: `ApprovalKind`, `ApprovalState`, `ApprovalDecider`, `ApprovalDecision`, `ApprovalRecord`, mutation/reconcile results, list/show/decide params.
- [x] `crates/lazoo-domain/src/approval.rs`: `approval_policy`, bounded action/detail/list-limit validation, SHA-256 `approval_fingerprint`.
- [x] Tests: `crates/lazoo-protocol/tests/approval.rs`, `crates/lazoo-domain/tests/approval.rs`.

### Task 2: Adapter contract and Codex bridge

- [x] `crates/lazoo-providers/src/contract.rs`: `PermissionKind`, `PermissionDecision`, `PermissionRequest`, default fail-closed `TurnEventObserver::decide_permission`.
- [x] `crates/lazoo-providers/src/codex.rs`: `normalize_codex_permission_request` (bounded, redacted, scope-expansion detection), `CodexSession::handle_server_request` awaits the observer, answers `accept`/`decline`, records the decision on the `PermissionRequested` event, and excludes the wait from the turn budget.
- [x] Fixture: `fake-codex-app-server.ps1` emits `item/commandExecution/requestApproval` for prompts containing `REQUEST_APPROVAL` and records the client's response.
- [x] Tests in `crates/lazoo-providers/tests/codex.rs`.

### Task 3: Schema 8 and store ledger

- [x] `migrations/0008_approvals.sql`: `approvals`, `approval_commands`, indexes.
- [x] `crates/lazoo-store/src/approval.rs`: `request_approval` (record-first policy), `decide_approval` (user), `close_approval` (system), `get_approval`, `list_approvals`, `reconcile_approvals`; `LZ-APPROVAL-*` store errors.
- [x] Tests in `crates/lazoo-store/tests/approval.rs`; schema assertions advance to 8.

### Task 4: Daemon governance and RPC

- [x] `apps/lazood/src/app/approval.rs`: `App::govern_permission` wait loop with 250 ms re-read, notify on decide/cancel, expiry and turn-cancel closure; `approval.decide|list|show` handlers.
- [x] `apps/lazood/src/app/turn.rs`: `AppTurnObserver::decide_permission`; `turn.cancel` wakes pending approvals.
- [x] `apps/lazood/src/app.rs`: routing, `approval_notify`, default fifteen-minute timeout with a test override, startup `reconcile_approvals`.
- [x] Tests in `apps/lazood/tests/daemon.rs` through the fake session backend.

### Task 5: CLI

- [x] `lazooctl approval list|show|approve|decline`; `LZ-APPROVAL-*` maps to exit code 14.
- [x] Tests in `apps/lazooctl/tests/cli.rs`.

### Task 6: Acceptance and continuity

- [x] `scripts/acceptance/approval-gate.ps1` writes `artifacts/acceptance/approval-gate-manifest.json`.
- [x] README, PROJECT-BRIEF status, NEXT-SESSION, and `docs/verification/2026-09-22-approval-gate.md`.
