# Codex Collaboration Tools Implementation Plan

**Goal:** Let a daemon-managed Codex employee read the live roster, delegate child tasks, wait on them, and report structured outcomes from inside its native session, through the same fail-closed store paths the CLI uses.

**Architecture:** The adapter contract gains `ToolSpec`, `ToolCall`, `ToolResult`, a `tools` list on `SessionRequest`, and a default fail-closed `TurnEventObserver::call_tool`. The Codex adapter declares the tools as app-server `dynamicTools` (opting into `experimentalApi` only then), answers `item/tool/call` with the daemon's result, and surfaces each call as a turn event. The daemon declares the catalog for Codex sessions and serves the four tools as the turn's recipient endpoint. No schema change.

**Tech Stack:** Rust 1.98, Serde, rusqlite/SQLite WAL, Codex app-server v2 JSON-RPC over stdio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-09-23-collaboration-tools-design.md`

## Global Constraints

- No changes under `apps/lazoo-desktop`; `desktop.snapshot` and Tauri capabilities are unchanged.
- Tools act only as the turn's recipient employee and session; nothing approves, widens, or bypasses store guards.
- Refused calls answer `success = false` and never end the turn; only ledger failures propagate.
- `experimentalApi` only with declared tools; resume with tools fails closed.
- No provider smoke or live model use in acceptance.

---

### Task 1: Adapter contract

- [x] `crates/lazoo-providers/src/contract.rs`: `ToolSpec`, `ToolCall`, `ToolResult`, `SessionRequest::tools` (serde default) and `with_tools`, default fail-closed `call_tool`.
- [x] Export from `crates/lazoo-providers/src/lib.rs`.
- [x] Test: `observer_tool_calls_fail_closed_without_an_attached_control_plane` in `crates/lazoo-providers/tests/contract.rs`.

### Task 2: Codex adapter

- [x] `crates/lazoo-providers/src/codex.rs`: `initialize` takes the experimental flag; `thread_start_params` adds `dynamicTools`; `thread_resume_params` refuses tools; `handle_server_request` routes `item/tool/call` to `handle_tool_call`, which bounds arguments, asks the observer, answers Codex first, and returns a `ToolUpdate` event; `normalize_codex_tool_call` is public.
- [x] Fixture `fake-codex-app-server.ps1` records `initialize` and `thread/start` params and issues `item/tool/call` for prompts containing `CALL_TOOL`.
- [x] Tests in `crates/lazoo-providers/tests/codex.rs`.

### Task 3: Daemon tools

- [x] `apps/lazood/src/app/tools.rs`: catalog, `call_collaboration_tool`, `employees_list`, `task_delegate`, `task_wait`, `task_report`.
- [x] `apps/lazood/src/app/turn.rs`: `AppTurnObserver::call_tool`.
- [x] `apps/lazood/src/app.rs`: declare the catalog for Codex sessions in `start_new_session`.
- [x] Fake backend in `apps/lazood/tests/daemon.rs` records declared tool names, issues scripted calls after the first native event, and records results; three scenario tests plus the acceptance-script contract test.

### Task 4: Acceptance and continuity

- [x] `scripts/acceptance/collaboration-tools.ps1` writes `artifacts/acceptance/collaboration-tools-manifest.json`.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-09-23-collaboration-tools.md`.
