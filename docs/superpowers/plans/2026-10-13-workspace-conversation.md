# Workspace Conversation Implementation Plan (phase 2)

**Goal:** Serve the third read model of the desktop workspace contract, `workspace.conversation` (the active conversation's lead header with its truthful lifecycle, the worker dock with the information architecture's eight states derived from durable rows, the conversation's own Inbox items, and its bounded turns), so the separately owned desktop can bind the terminal header and the worker dock.

**Architecture:** No schema change. `lazoo-protocol` gains the header, card, and result types beside the phase 1 types. `lazoo-domain` owns the two rules as pure functions: the lead lifecycle and the worker card state with Lazoo's words. `lazoo-store` reads one conversation's rows under one lock (`conversation_workspace`): the conversation, its project, the newest root task, the newest turns, and every worker task with its dispatch, active turn state, and session state. `lazood` derives the header (from the roster entry), the cards (reusing the Inbox sources for pending approvals and escalated incidents), and the embedded attention (the Inbox derivation filtered to the conversation); `lazooctl workspace show` maps to the method.

**Tech Stack:** Rust 1.98, rusqlite/SQLite WAL, clap, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-13-desktop-workspace-contract-design.md` (sections 3.3, 4.4, 4.5, 5); section 6's actions are phase 3.

## Global Constraints

- No migration; schema stays 15. No change under `apps/lazoo-desktop`; `desktop.snapshot` unchanged.
- Derived, never stored: no table, no read state, no event, no mutation, no dismissal state for completed cards.
- Truthful states only: `unknown` is reserved for `uncertain` rows; a disconnected daemon is the UI's own state.
- Section 9's questions 3 and 5 take the proposed answers as defaults on the owner's direction to finish as much as possible: an open lead-window incident on a task makes its worker `blocked`, and the conversation's unresolved Inbox items are embedded as `attention`.
- No provider smoke or live model use in acceptance.

---

### Task 1: Protocol, domain, and store

- [x] `crates/lazoo-protocol/src/workspace.rs`: `LeadLifecycle`, `WorkerState`, `LeadSessionSummary`, `LeadHeader`, `WorkerWorkspace`, `WorkerCard`, `WorkspaceScope`, `WorkspaceConversationParams { id, turnLimit }`, `WorkspaceConversationResult`.
- [x] `crates/lazoo-domain/src/workspace.rs`: `lead_lifecycle`, `WorkerFacts`, `worker_state` (the eight states in the contract's order with Lazoo's words), `worker_label` (the objective bounded to 120 bytes); the turn limit reuses `validate_workspace_limit`.
- [x] `crates/lazoo-store/src/workspace.rs`: `conversation_workspace` (the conversation, its project, the first user text, the newest root task, the newest `turnLimit` turns oldest first with a truncation flag, and one row per task below a root with its dispatch, active turn state, and session state).
- [x] `crates/lazoo-domain/tests/workspace.rs`: the lifecycle rule and the worker state table.

### Task 2: Daemon and CLI

- [x] `apps/lazood/src/app.rs` route and `apps/lazood/src/app/workspace.rs` handler (`workspace.conversation`; `LZ-WORKSPACE-INVALID`, `LZ-CONVERSATION-NOT-FOUND`).
- [x] `apps/lazooctl/src/lib.rs`: `workspace show <id> [--turn-limit]`.
- [x] `apps/lazood/tests/daemon.rs` and `apps/lazooctl/tests/cli.rs`: the header through dormant, idle, and suspended; the root task and its authority; cards through `blocked` (proposed), `waiting`, `complete`, and `interrupted`; the embedded attention; bounded turns; the unchanged snapshot; the CLI mapping; the acceptance-script contract.

### Task 3: Acceptance and continuity

- [x] `scripts/acceptance/workspace-conversation.ps1` writes `artifacts/acceptance/workspace-conversation-manifest.json`.
- [x] README, NEXT-SESSION, the contract design's status, and `docs/verification/2026-10-13-workspace-conversation.md`.
