# Desktop Inbox Implementation Plan

**Goal:** Serve the typed desktop Inbox read model from the daemon (`inbox.list`, `inbox.counts`, `lazooctl inbox`) so the separately owned desktop can bind its global Inbox to one derivation instead of re-deriving "what needs the user" from four list RPCs and the raw event stream.

**Architecture:** No schema change. `lazoo-protocol` gains the `InboxItem`, `InboxCounts`, `InboxListParams`, and result types (`INBOX_CONTRACT_V1`). `lazoo-domain` owns the whole rule as pure functions: which approval, task, and incident rows are items, their attention (`action-required`, `watching`, `acknowledged`, `resolved`), severity, title, required action, scope, deadline, detail, and actions (data naming an existing revision-checked RPC), plus counts, filter membership, urgency order, and the archive cursor. `lazoo-store` reads the source rows under one lock with the event high-water mark (`inbox_sources` for the unresolved set, `inbox_archive` for a cursor-bounded page of what left it). `lazood` maps params to those two reads and the domain rule; `lazooctl inbox list|counts` maps to the two methods. No tool gains access and nothing is stored.

**Tech Stack:** Rust 1.98, rusqlite/SQLite WAL, clap, Tokio, PowerShell acceptance.

**Spec:** `docs/superpowers/specs/2026-10-11-desktop-inbox-contract-design.md` (section 9's proposed answers adopted as defaults on 2026-10-13).

## Global Constraints

- No migration; schema stays 15. No change under `apps/lazoo-desktop`, no Tauri command, no `desktop.snapshot` field.
- Derived, never stored: no table, no read state, no event, no idempotency key, no new mutation.
- The badge counts only `action-required`; a lead's window is `watching`; an acknowledged escalation is visible but uncounted.
- Every action is data for an RPC the CLI already serves; the daemon grants the desktop nothing by naming it.
- No provider smoke or live model use in acceptance.

---

### Task 1: Protocol, domain, and store

- [x] `crates/lazoo-protocol/src/inbox.rs`: `InboxFilter`, `InboxKind`, `InboxAttention`, `InboxSeverity`, `InboxDeadline`, `InboxScope`, `InboxAction`, `InboxItem`, `InboxCounts`, `InboxListParams` (limit defaults to 200), `InboxListResult`, `InboxCountsResult`.
- [x] `crates/lazoo-domain/src/inbox.rs`: `approval_inbox_item`, `task_inbox_item`, `incident_inbox_item`, `inbox_filter_matches`, `inbox_counts`, `sort_unresolved_inbox`, `sort_inbox_archive`, `InboxCursor`, `validate_inbox_limit` (1 to 500).
- [x] `crates/lazoo-store/src/inbox.rs`: `inbox_sources` (pending approvals; `proposed` and `uncertain` tasks; open or acknowledged incidents escalated to the lead or the user, each with its task's conversation; employees; event high-water) and `inbox_archive` (decided approvals; terminal tasks once `proposed` or `uncertain`; resolved or remediated escalations), each source bounded and cut at the (`updatedAt`, id) cursor.
- [x] Tests in `crates/lazoo-protocol/tests/inbox.rs` and `crates/lazoo-domain/tests/inbox.rs`.

### Task 2: Daemon and CLI

- [x] `apps/lazood/src/app.rs` routes and `apps/lazood/src/app/inbox.rs`: `inbox.list` (filters, limit, archive cursor, `truncated`, `nextBefore`, `eventHighWater`) and `inbox.counts`; `LZ-INBOX-INVALID` for a bad limit, cursor, or a cursor outside the resolved filter.
- [x] `apps/lazooctl/src/lib.rs`: `inbox list [--filter] [--limit] [--before]` and `inbox counts`.
- [x] `apps/lazood/tests/daemon.rs` and `apps/lazooctl/tests/cli.rs`: the pending-approval story, the tasks-and-incidents story (lead window watched then counted, acknowledged uncounted, every action through the RPC it names, archive paging), the tool-catalog boundary, the CLI mapping, and the acceptance-script contract.

### Task 3: Acceptance and continuity

- [x] `scripts/acceptance/desktop-inbox.ps1` writes `artifacts/acceptance/desktop-inbox-manifest.json`.
- [x] README, PROJECT-BRIEF Stage 2 status, NEXT-SESSION, the design document's status and adopted answers, and `docs/verification/2026-10-13-desktop-inbox.md`.
