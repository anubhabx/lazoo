# Coalesced Notice Turns Implementation Plan

**Goal:** Deliver every notice and peer message queued for one recipient and task as a single provider turn, oldest first, while every message keeps its own delivery and acknowledgement evidence and no earlier notice rule changes.

**Architecture:** `lazoo-domain` renders one turn text from several notice texts. `lazoo-store` gathers the batch when a notice turn is recorded, links companions to the turn through per-message `turn.record` commands, and carries them through every turn transition beside the canonical message. The daemon delivers each turn once per pass. No schema change.

**Tech Stack:** Rust 1.98, Serde, rusqlite/SQLite WAL, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-09-25-coalesced-notices-design.md`

## Global Constraints

- No schema change; no changes under `apps/lazoo-desktop`; no CLI or tool-catalog change.
- A single queued notice is delivered with exactly the text it had before.
- Every notice keeps its own message row, delivery attempt, and acknowledgement evidence.
- No automatic activation; no provider smoke or live model use in acceptance.

---

### Task 1: Domain

- [x] `crates/lazoo-domain/src/delivery.rs`: `render_coalesced_notice_text` (one text passes through; several join oldest first under a fixed header).
- [x] Test in `crates/lazoo-domain/tests/delivery.rs`.

### Task 2: Store

- [x] `crates/lazoo-store/src/delivery.rs`: per-turn command keys `notice-turn:<messageId>:<turnId>`, `latest_notice_turn`, `notice_member_ids`, `notice_companions`, `queued_notices_for`; `record_notice_turn_tx` gathers the batch within `MAX_TURN_TEXT_BYTES`, ignores a cancelled turn for a still-queued notice, and records one command per member; `consume_queued_message_tx` closes the newest recorded turn; `is_peer_only_turn` replaces `is_peer_message`.
- [x] `crates/lazoo-store/src/conversation.rs`: companions follow the canonical message through claim, acknowledge, fail, retry, and both cancel paths.
- [x] `crates/lazoo-store/src/task.rs`: a turn projects as peer progress only when every notice it carries is a peer message.
- [x] Tests in `crates/lazoo-store/tests/delivery.rs`.

### Task 3: Daemon

- [x] `apps/lazood/src/app/delivery.rs`: one delivery per turn per pass; a notice already carried earlier in the pass is re-read and skipped.
- [x] `apps/lazood/tests/daemon.rs`: two results as one turn; the dormant-recipient scenario now expects one coalesced turn on resume; the acceptance-script contract.

### Task 4: Acceptance and continuity

- [x] `scripts/acceptance/coalesced-notices.ps1` writes `artifacts/acceptance/coalesced-notices-manifest.json`.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, and `docs/verification/2026-09-25-coalesced-notices.md`.
