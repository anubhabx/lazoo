# Conversation Rename Implementation Plan

**Goal:** Let the local user set or clear a conversation's title, as the information architecture's section 6.3 requires, without a schema change.

**Architecture:** `lazoo-protocol` gains `RenameConversationParams`. `lazoo-store` gains `RenameConversationInput` and `Store::rename_conversation` (same-title no-op without an event; otherwise revision-checked write plus `conversation.renamed`) and the `ConversationStale` error. `lazood` serves `conversation.rename`; `lazooctl conversation rename` maps to it.

**Tech Stack:** Rust 1.98, rusqlite/SQLite WAL, clap, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-13-conversation-rename-design.md`

## Global Constraints

- No migration; schema stays 15. No change under `apps/lazoo-desktop`; `desktop.snapshot` unchanged.
- Only the local user renames; no collaboration tool can.
- No provider smoke or live model use in acceptance.

---

### Task 1: Protocol and store

- [x] `crates/lazoo-protocol/src/conversation.rs`: `RenameConversationParams { id, expectedRevision, title }`.
- [x] `crates/lazoo-store/src/conversation.rs`: `RenameConversationInput`, `Store::rename_conversation`; `crates/lazoo-store/src/lib.rs`: `StoreError::ConversationStale`.
- [x] `crates/lazoo-store/tests/conversation.rs`: revisioned, naturally idempotent, stale refused, blank clears, validation and existence before any write, event order.

### Task 2: Daemon and CLI

- [x] `apps/lazood/src/app.rs` route and `apps/lazood/src/app/turn.rs` handler and `LZ-CONVERSATION-STALE` mapping.
- [x] `apps/lazooctl/src/lib.rs`: `conversation rename <id> --expected-revision N [--title T]`.
- [x] `apps/lazood/tests/daemon.rs` and `apps/lazooctl/tests/cli.rs`: the user's session through the daemon, the stable errors, list and show, the CLI mapping, and the acceptance-script contract.

### Task 3: Acceptance and continuity

- [x] `scripts/acceptance/conversation-rename.ps1` writes `artifacts/acceptance/conversation-rename-manifest.json`.
- [x] README, NEXT-SESSION, the workspace contract's rename references, and `docs/verification/2026-10-13-conversation-rename.md`.
