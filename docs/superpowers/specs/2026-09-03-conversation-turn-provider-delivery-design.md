# Lazoo Durable Conversations and Codex Turn Delivery Design

**Status:** Approved by the owner through the standing backend-slice direction, the explicit `docs/NEXT-SESSION.md` handoff, and the instruction to proceed with the recommended choices

**Scope:** Backend-only durable conversations, immutable turn inputs, append-only provider events, canonical message delivery, and one evidence-based Codex App Server transport. No desktop, Tauri command, `desktop.snapshot`, ConPTY, task, worktree, approval, managed-memory, or resource-governor capability.

## Outcome

Lazoo can create a projectless or project-scoped conversation, record a user- or employee-authored turn before provider I/O, route that turn through the existing canonical collaboration message, activate an explicitly selected lead when the local user requests it, deliver the turn to a live Codex App Server session, and preserve native acceptance, progress, completion, failure, cancellation, and owner-loss evidence.

The slice establishes a truthful provider-delivery path without claiming that a turn was delivered merely because bytes were written or an asynchronous request returned success. A canonical message becomes acknowledged only after Lazoo has durably recorded a provider-native turn identifier or normalized progress event. Other provider adapters remain available for discovery, probing, smoke tests, and session ownership, but daemon-managed conversation delivery remains disabled for them until each receives its own acceptance evidence.

## Constraints carried forward

- Windows first, local first, and one local user.
- Backend and communication layers only; `apps/lazoo-desktop` stays unchanged.
- Reuse the canonical collaboration envelope and delivery attempt ledger.
- Record the conversation turn, delivery intent, audit event, and outbox row before provider I/O.
- Preserve native authentication, models, tools, MCP, plugins, skills, permissions, and session identity.
- Keep `ask` as the default authority mode. This slice does not invent an approval system.
- Delivery is at least once before native acceptance and never silently repeated after an uncertain outcome.
- Deterministic acceptance never invokes a model. The Codex live gate is separate, explicit, and quota-consuming.
- Stop before task orchestration, worktree mutation, ConPTY, policy approvals, managed memory, or UI binding.

## Current interface evidence

The installed `codex-cli 0.144.1` and its generated App Server v2 schema expose the lifecycle required by this slice:

1. initialize the stdio connection;
2. use `thread/start` or `thread/resume` without starting a model turn;
3. use `turn/start` to receive a native turn object and identifier;
4. observe `turn/started`, item/progress notifications, and `turn/completed`; and
5. use `turn/interrupt` with the native thread and turn identifiers.

The existing Codex adapter already performs the handshake, preserves the native thread identifier, normalizes progress, and interrupts an active turn. Its current `turn` method returns accumulated events only after completion. This slice adds an observer boundary so acceptance and progress can be persisted as they occur.

## Rejected shortcuts

### A separate conversation transport

A conversation-only delivery table that bypasses `messages` would create two competing delivery paths. It would undermine correlation, retries, dead letters, restart reconciliation, and future employee-to-employee delivery. This design rejects it.

### A synthetic user employee

Creating a hidden employee to stand in for the local user would make authorship and audit history false, pollute the roster, and grant an implementation detail an employee identity. This design rejects it.

### Generic live delivery to every adapter

The provider trait normalizes five adapters, but only Codex is part of this slice's live acceptance contract. Enabling all five on the strength of shared Rust types would overstate support. The daemon returns a stable unsupported-provider error for non-Codex conversation delivery until later slices add provider-specific evidence.

## Versioned actor-aware message envelope

The version-1 message envelope supports employee senders only. A user-authored prompt cannot truthfully use it, so the canonical envelope gains version 2 rather than adding another bus.

`MessageSender` is an untagged strict union:

- an existing `MessageEndpoint { employeeId, sessionId }`, preserving the serialized version-1 shape; or
- `UserMessageEndpoint { kind: "user" }` for the one local owner.

Recipients remain employee endpoints. Existing `message.send` commands continue to create version-1 employee-to-employee envelopes. Conversation turn delivery creates version-2 envelopes for both user and employee authors so that the envelope version identifies the conversation-aware contract. Existing version-1 records remain readable without JSON shape changes.

Schema 5 rebuilds the message parent and dependent delivery tables in foreign-key-safe order so `sender_employee_id` can become nullable and a checked `sender_kind` column can be added. Version-1 rows copy as `sender_kind = 'employee'`. A version-2 user row requires `sender_kind = 'user'` with null sender employee/session values. An employee row requires a valid sender employee and an optional owned session. Every migration test ends with `PRAGMA foreign_key_check` returning no rows.

## Conversation contract

A `ConversationRecord` contains:

- stable UUID and contract version 1;
- optional registered project UUID, where null means projectless;
- selected lead employee UUID;
- the lead configuration revision observed at creation;
- optional bounded title supplied by the caller;
- monotonic local sequence and revision;
- creation and update timestamps.

Conversation creation is idempotent. The lead must exist, be enabled, and be route-ready. A project UUID, when supplied, must identify a registered project. Projectless conversations use the selected employee session's protected Lazoo scratch directory; this slice does not create a Git worktree or project memory.

The daemon and CLI expose:

- `conversation.create`;
- `conversation.list` ordered by local sequence;
- `conversation.show` including bounded turn summaries; and
- `turn.list` and `turn.show` for paginated durable history.

Renaming, deleting, archiving, participant management, and UI navigation are later work.

## Turn contract

A `ConversationTurn` represents one immutable authored input addressed to one employee. It contains:

- stable UUID, contract version 1, and conversation UUID;
- author as the local user or an employee endpoint;
- recipient employee UUID and optional selected Lazoo session UUID;
- recipient employee configuration revision captured at record time;
- immutable UTF-8 text input, bounded to 256 KiB;
- canonical message UUID, message attempt UUID when claimed, and correlation UUID;
- `recorded`, `submitting`, `accepted`, `completed`, `retryable-failed`, `terminal-failed`, `cancelled`, or `uncertain` state;
- monotonic revision;
- optional owning daemon boot UUID while provider I/O is in flight;
- optional provider, native session identifier, and native turn identifier;
- safe failure code/message and lifecycle timestamps.

`recorded` means durable but no provider operation has started. `submitting` means a canonical delivery attempt exists and provider I/O may have begun. `accepted` requires durable native turn-ID or progress evidence. `retryable-failed` describes a definite pre-acceptance failure whose canonical message retry deadline has not been exhausted. `completed`, `terminal-failed`, and `cancelled` describe observed terminal outcomes. `uncertain` is terminal for automatic delivery and means provider side effects or partial work may have occurred but Lazoo cannot prove the result.

Every normalized provider event is appended to `conversation_turn_events` with a per-turn sequence. Raw provider payloads are not stored. Text fields use the existing 64 KiB per-value bound, and the durable transcript is capped at 512 KiB per turn. When the cap is reached, one explicit truncation event is recorded and later verbose deltas are omitted without losing lifecycle evidence.

## Record-first submission

`turn.submit` performs one immediate transaction before provider I/O:

1. resolve an idempotent replay before revalidating mutable employee state;
2. validate the conversation, author, recipient, project scope, and input;
3. insert the immutable turn in `recorded` state;
4. insert a version-2 canonical message envelope referencing the conversation and turn;
5. insert the queued message delivery record;
6. insert the turn and message command mappings; and
7. append audit events and transactional outbox rows.

The message body contains only the local turn UUID and immutable text required for transport. The message correlation UUID becomes the provider `TurnRequest.correlation_id`. Replaying the same submit idempotency key never creates another turn, message, event, or delivery attempt. When the replay finds the original turn still `recorded` with a queued message and no attempt, the daemon may resume that original undelivered operation; every later state is returned without another provider call.

After the transaction commits, the daemon resolves recipient activation and delivery. A failure before provider I/O leaves durable `recorded`/queued state with a stable error. A failure after a delivery attempt begins transitions both ledgers consistently.

## Activation boundary

This slice has no general approval engine, so activation stays narrow:

- An already idle live session owned by the current daemon may receive the turn.
- A user-authored turn may set `activateIfNeeded = true`. The explicit local-user command authorizes only warming the conversation's selected lead through the existing `session.warm` lifecycle.
- `activateIfNeeded` defaults to false.
- Employee-authored turns cannot activate a dormant recipient in this slice. They remain durably recorded and return `LZ-TURN-ACTIVATION-REQUIRES-APPROVAL` until the user warms the employee or a later policy slice authorizes activation.
- A changed, disabled, unavailable, or non-Codex recipient fails before provider I/O with stable conversation/turn errors; no fallback employee is selected.

Session startup remains separate from turn delivery. A warm session is not evidence that the turn was accepted.

## Provider event observer

The provider contract gains a synchronous, fallible `TurnEventObserver` used from each adapter's async event loop. The observer receives already normalized and redacted `ProviderEvent` values. Provider smoke retains a collecting observer, so its public behavior does not change.

For Codex delivery:

1. `turn/start` returns a native turn ID;
2. the adapter emits a normalized `TurnStarted` event immediately from that response;
3. the daemon observer appends the event, records the native session/turn IDs, moves the turn to `accepted`, and acknowledges the canonical message attempt in one transaction;
4. later normalized events append in order and update safe progress timestamps;
5. `turn/completed` moves the turn to `completed`, `terminal-failed`, or `cancelled` according to the native status; and
6. an observer persistence error triggers best-effort `turn/interrupt`, closes the daemon operation, and records an uncertain outcome where possible.

The observer cannot approve provider-native permission requests. The existing fail-closed permission behavior remains intact.

## Delivery failures and retries

Before native acceptance, definite provider rejection or startup failure closes the message attempt through the existing failure transition. Retryable failures put the turn in `retryable-failed` and use the existing bounded delivery schedule; exhausted or non-retryable failures put it in `terminal-failed`. `turn.retry` is the only command that advances a due retryable message back to queued and starts another delivery attempt for the same immutable local turn.

After native acceptance, the message remains acknowledged because delivery to the provider session is proven. A later provider failure changes only the turn result. An uncertain turn is never retried automatically because Codex App Server does not accept Lazoo's logical idempotency key for `turn/start`; repeating it could duplicate tool side effects.

Submit, retry, and cancel commands are idempotent and revision-checked. Invalid transitions return stable `LZ-CONVERSATION-*` or `LZ-TURN-*` errors on CLI exit code 12.

## Cancellation

`turn.cancel` first records a revision-checked cancellation request and audit/outbox event. If the turn is only `recorded`, it cancels the queued canonical message without provider I/O. If the turn is `submitting` or `accepted`, the daemon calls the live session's provider cancellation method.

A normalized cancellation or cancelled completion makes the turn `cancelled`. If no live handle exists or the interrupt outcome cannot be observed, the turn becomes `uncertain`; it is not reported as cancelled merely because `turn/interrupt` accepted bytes.

## Restart and owner-loss reconciliation

Daemon startup reconciles conversations after native sessions and collaboration messages:

- `recorded` turns with queued delivery remain eligible for submission replay of the same idempotent command;
- `submitting` turns become `uncertain` with `LZ-TURN-OWNER-LOST`, because provider I/O may have started;
- `accepted` turns without a recorded terminal event become `uncertain` with `LZ-TURN-OWNER-LOST`, because provider tools may have produced side effects before Job Object termination;
- `retryable-failed` turns retain the canonical message retry deadline, while already terminal turns remain unchanged; and
- every effective reconciliation writes one audit/outbox event and preserves message/turn identity.

Reconciliation never adopts a native PID, native session, or turn from a prior daemon boot.

## Deterministic and live acceptance

The daemon uses an injected fake managed session for deterministic acceptance. The fake exposes explicit gates for:

- provider rejection before acceptance;
- native turn-ID acceptance;
- progress and terminal events;
- delayed completion plus concurrent cancellation;
- event-persistence failure;
- handle loss and daemon restart; and
- duplicate submit/retry/cancel calls.

`scripts/acceptance/conversation-delivery.ps1` uses isolated state and real release binaries with a deterministic fixture transport. It proves schema 5 migration, projectless and project conversations, user/employee authorship, record-first ordering, envelope-v2 compatibility, explicit activation rules, accepted-evidence gating, retry, cancellation, owner-loss uncertainty, audit/outbox publication, transcript bounds, and zero desktop expansion without invoking a model.

A separate opt-in Codex gate submits one minimal prompt through an owned live session and verifies:

- native session identity is preserved;
- a native turn ID is recorded before acknowledgement;
- progress or completion is durable;
- the turn and canonical message correlate; and
- the provider tree stops cleanly.

The live gate is labeled quota-consuming, runs only after deterministic and release gates pass, and is never hidden inside ordinary acceptance.

## Verification and closeout

The implementation is complete only when all of these are fresh and green:

1. protocol, domain, store, provider, daemon, and CLI focused suites;
2. schema upgrades from versions 1 through 4 plus foreign-key integrity;
3. full `cargo test --workspace`, formatting, Clippy, all-target checks, and locked release build;
4. first-slice, employee-roster, native-session, collaboration-bus, and conversation-delivery acceptance manifests;
5. the explicit Codex live turn gate, or an exact blocker if quota/authentication prevents it;
6. `git diff --check` and line-ending checks;
7. proof that `apps/lazoo-desktop`, Tauri capabilities, and `desktop.snapshot` did not change; and
8. synchronized README, project brief, next-session handoff, and verification evidence.

The closeout must distinguish deterministic transport proof from current live Codex proof and must not claim other provider delivery, ConPTY, delegation, worktrees, approvals, managed memory, or product-ready UI.
