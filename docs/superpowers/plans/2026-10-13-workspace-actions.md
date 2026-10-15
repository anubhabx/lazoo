# Workspace Actions Implementation Plan (phase 3)

**Goal:** Let the conversation header and each worker dock card of `workspace.conversation` name the actions that apply right now, as data for an existing revision-checked RPC, exactly as Inbox items do, so the separately owned desktop can offer accept, cancel, resolve, decide, cancel-turn, retry-turn, stop-session, and rename without a daemon mutation or grant of its own.

**Architecture:** No schema change and no new RPC. `lazoo-protocol` adds `actions: Vec<InboxAction>` to `LeadHeader` and `WorkerCard`. `lazoo-domain` owns the two rules as pure functions beside the phase 2 ones: `worker_actions` (from the task's state and a pending approval, reusing the Inbox's action builders) and `lead_actions` (from the conversation, the lead's session, and the lead's newest turn with its revision). The store's `conversation_workspace` carries the lead turn's revision. The daemon fills the two fields; the CLI needs no change.

**Tech Stack:** Rust 1.98, rusqlite/SQLite WAL, clap, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-13-desktop-workspace-contract-design.md` section 6.

## Global Constraints

- No migration; schema stays 15. No change under `apps/lazoo-desktop`; `desktop.snapshot` unchanged.
- An action is data: it names an existing RPC with the `expectedRevision` the user saw; the caller adds a fresh `idempotencyKey` (and the `title` for `rename`). Naming an RPC grants nothing; every named RPC keeps its own guards.
- An action is offered only when its RPC would accept it now: accept and cancel for a proposal; resolve for an uncertain task; the approval's own decisions while it is pending; cancel for a waiting or running task; cancel-turn for an unfinished or retryable lead turn; retry-turn for a retryable one; stop-session for an idle lead session with no unfinished turn; rename always.
- No provider smoke or live model use in acceptance.

---

### Task 1: Protocol and domain

- [x] `crates/lazoo-protocol/src/workspace.rs`: `actions` on `LeadHeader` and `WorkerCard`.
- [x] `crates/lazoo-domain/src/inbox.rs`: `task_actions`, `approval_actions`, and `action` shared inside the crate.
- [x] `crates/lazoo-domain/src/workspace.rs`: `worker_actions`, `lead_actions`.
- [x] `crates/lazoo-domain/tests/workspace.rs`: the two rules.

### Task 2: Store and daemon

- [x] `crates/lazoo-store/src/workspace.rs`: the lead's newest turn carries its revision.
- [x] `apps/lazood/src/app/workspace.rs`: the header and cards carry their actions.
- [x] `apps/lazood/tests/daemon.rs`: a story that reads the actions and sends three of them exactly as named (accept a proposal, rename, stop the lead), plus the acceptance-script contract.

### Task 3: Acceptance and continuity

- [x] `scripts/acceptance/workspace-actions.ps1` writes `artifacts/acceptance/workspace-actions-manifest.json`.
- [x] README, NEXT-SESSION, the contract design's status, and `docs/verification/2026-10-13-workspace-actions.md`.
