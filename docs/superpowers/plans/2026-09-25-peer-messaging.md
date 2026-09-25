# Peer Messaging Implementation Plan

**Goal:** Let a daemon-managed Codex employee ask another employee in its task tree a question and answer one, over the existing message bus and notice path, without ever activating a dormant employee or completing a task by accident.

**Architecture:** `lazoo-domain` validates peer content and renders the turn text. `lazoo-store` inserts peer messages and replies under one new command scope, resolves the recipient's task inside the caller's tree, lets the notice path deliver them, and marks them read inside a turn. The daemon serves `message_send` and `message_reply` and extends `task_wait` with an inbox. No schema change.

**Tech Stack:** Rust 1.98, Serde, rusqlite/SQLite WAL, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-09-25-peer-messaging-design.md`

## Global Constraints

- No schema change; no changes under `apps/lazoo-desktop`; no CLI change.
- A peer message never activates a dormant employee, widens authority, approves anything, or completes a task.
- Every send, reply, and read is idempotent by tool call or by message and turn.
- No provider smoke or live model use in acceptance.

---

### Task 1: Domain

- [x] `crates/lazoo-domain/src/delivery.rs`: `PeerMessageNotice`, `MAX_PEER_MESSAGE_BYTES`, `validate_peer_message_content`, `render_peer_message_text`.
- [x] `MessageValidationError::{PeerContentEmpty, PeerContentTooLarge}`.
- [x] Tests in `crates/lazoo-domain/tests/delivery.rs`.

### Task 2: Store

- [x] `crates/lazoo-store/src/delivery.rs`: `send_peer_message`, `reply_peer_message`, `pending_peer_messages`, `consume_peer_message`; `message.peer` scope joins the notice path (`pending_lead_notices`, `record_notice_turn` with the running-only rule); shared `consume_queued_message_tx` closes a stale notice turn.
- [x] `crates/lazoo-store/src/task.rs`: a completed peer turn projects as progress.
- [x] `InsertMessageInput.id` so a body can name its own message; `PeerRecipientSelf`, `PeerRecipientTaskNotFound`, `PeerMessageScope` errors.
- [x] Tests in `crates/lazoo-store/tests/delivery.rs`.

### Task 3: Daemon

- [x] `apps/lazood/src/app/tools.rs`: catalog entries, `message_send` with bounded reply wait, `message_reply`, inbox consumption in `task_wait` and `message_send`, `LZ-TOOL-*` mapping.
- [x] Fake backend in `apps/lazood/tests/daemon.rs`: per-session scripted calls and the `$message-id` placeholder; three scenarios plus the acceptance-script contract.

### Task 4: Acceptance and continuity

- [x] `scripts/acceptance/peer-messaging.ps1` writes `artifacts/acceptance/peer-messaging-manifest.json`.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-09-25-peer-messaging.md`.
