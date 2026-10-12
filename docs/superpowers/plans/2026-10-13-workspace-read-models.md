# Workspace Read Models Implementation Plan (phase 1)

**Goal:** Serve the first two read models of the desktop workspace contract, `workspace.conversations` (a space's conversation history with derived titles, markers, and the remembered lead) and `workspace.launcher` (who can lead right now, why not, and the start request as data), so the separately owned desktop can bind its sidebar and launcher.

**Architecture:** No schema change. `lazoo-protocol` gains the workspace types (`WORKSPACE_CONTRACT_V1`). `lazoo-domain` owns the rules as pure functions: the sidebar marker, the derived title, lead startability. `lazoo-store` reads one space's conversations with their turn and task facts under one lock (`space_conversations`). `lazood` derives the rows (reusing the Inbox derivation for the counted-item facts) and the launcher; `lazooctl workspace conversations|launcher` maps to the two methods.

**Tech Stack:** Rust 1.98, rusqlite/SQLite WAL, clap, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-13-desktop-workspace-contract-design.md` (sections 3.1, 3.2, 4.1 to 4.3, 5); section 3.3's `workspace.conversation` (header and worker dock) is phase 2.

## Global Constraints

- No migration; schema stays 15. No change under `apps/lazoo-desktop`; `desktop.snapshot` unchanged.
- Derived, never stored: no table, no read state, no event, no mutation; the start request is data for `conversation.create` and `turn.submit`.
- No provider smoke or live model use in acceptance.

---

### Task 1: Protocol, domain, and store

- [x] `crates/lazoo-protocol/src/workspace.rs`: `ConversationMarker`, `WorkerCounts`, `ConversationRow`, `LeadCandidate`, `WorkspaceStartAction`, the two params and results.
- [x] `crates/lazoo-domain/src/workspace.rs`: `conversation_marker`, `conversation_title`, `lead_startability`, `validate_workspace_limit`.
- [x] `crates/lazoo-store/src/workspace.rs`: `space_conversations` (one space, newest first, cursor, limit + 1) with the first user turn, last turn state, active turns, worker counts, newest root state, and last terminal time per conversation.
- [x] `crates/lazoo-domain/tests/workspace.rs`.

### Task 2: Daemon and CLI

- [x] `apps/lazood/src/app.rs` routes and `apps/lazood/src/app/workspace.rs` handlers (`LZ-WORKSPACE-INVALID`, `LZ-WORKSPACE-PROJECT-NOT-FOUND`); the Inbox derivation shared for counted items.
- [x] `apps/lazooctl/src/lib.rs`: `workspace conversations [--project-id] [--limit] [--before]` and `workspace launcher [--project-id]`.
- [x] `apps/lazood/tests/daemon.rs` and `apps/lazooctl/tests/cli.rs`: Home and a project as separate spaces, markers following the rows through proposal, acceptance, dispatch, and result, derived then stored titles, paging, the launcher's blocks and remembered lead, the unchanged snapshot, the CLI mapping, and the acceptance-script contract.

### Task 3: Acceptance and continuity

- [x] `scripts/acceptance/workspace-read-models.ps1` writes `artifacts/acceptance/workspace-read-models-manifest.json`.
- [x] README, NEXT-SESSION, the contract design's status, and `docs/verification/2026-10-13-workspace-read-models.md`.
