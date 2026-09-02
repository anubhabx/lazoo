# Lazoo Durable Collaboration Bus Design

**Status:** Approved by the owner-reviewed project brief, the explicit `docs/NEXT-SESSION.md` handoff, and the standing direction to continue backend slices autonomously with inline TDD

**Scope:** Backend-only canonical message ledger, delivery state machine, daemon RPC, and CLI controls. No desktop, Tauri command, ConPTY, provider-turn, worktree, task, approval, model, or quota work.

## Outcome

Lazoo can durably record a peer message before any delivery attempt, expose its ordered history, claim a concrete delivery attempt, correlate an acknowledgement or failure to that attempt, schedule bounded retries, dead-letter exhausted work, cancel pending work, and recover an uncertain in-flight attempt after daemon restart.

This slice deliberately stops before recipient activation or provider transport. A recorded activation intent names an employee and may name one of that employee's durable native sessions, but no message command starts a session, sends a model prompt, writes to provider stdin, or treats transport acceptance as delivery.

## Ledger shape

The bus uses four durable record families in schema 4:

1. `messages` stores an immutable version-1 envelope and receives a monotonic local sequence.
2. `message_deliveries` stores the current state, revision, retry budget, scheduling data, and safe public failure summary.
3. `message_delivery_attempts` appends one row whenever a queued or retryable message is claimed. An attempt is closed exactly once by acknowledgement, failure, cancellation, or restart reconciliation.
4. `message_dead_letters` stores terminal evidence when a retry budget is exhausted or a non-retryable failure occurs.

`message_commands` maps every mutating command scope and idempotency key to the durable message. Replaying a command returns the current record without another state transition or audit event.

Every effective mutation writes domain state, an audit event, and the existing transactional outbox row in one immediate SQLite transaction. Read-only list/show operations never publish events or change revisions.

## Versioned envelope

`MessageEnvelope` contains:

- `version`, fixed to `1` for this contract;
- stable message UUID and caller idempotency key;
- correlation UUID and optional causal message UUID;
- sender employee UUID plus optional sender session UUID;
- recipient employee UUID plus optional preferred recipient session UUID;
- optional project, conversation, and task UUIDs;
- a bounded `delegation`, `peer`, `reply`, `result`, `status`, or `system` message kind;
- a non-null JSON body;
- zero or more typed references containing a bounded kind, target, and JSON-object metadata;
- immutable creation timestamp.

If no correlation UUID is supplied, an initial message uses its own UUID. A causal message inherits the causal record's correlation UUID. Supplying a different correlation UUID with a causal message is rejected.

Sender and recipient employees must exist and be enabled at record time. Optional session references must exist and belong to the corresponding employee; historical session states are permitted because the envelope records identity rather than route readiness. An optional project must already be registered. Conversation and task UUIDs are opaque references until their own durable slices add foreign keys.

The recipient employee and optional recipient session are also the activation intent. The record exposes `requiresActivation: true` when no recipient session was supplied. The bus does not resolve, warm, resume, or mutate that intent in this slice.

Bodies are limited to 256 KiB of serialized JSON and cannot be null. A message may contain at most 32 references. Reference kinds are nonempty and at most 64 characters, targets are nonempty and at most 2,048 UTF-8 bytes, and reference metadata must be a JSON object no larger than 16 KiB. Large content therefore remains referenced rather than copied into the envelope.

## Delivery state machine

The externally visible states are exactly:

- `queued`
- `delivering`
- `acknowledged`
- `retryable-failed`
- `terminal-failed`
- `cancelled`

The state transitions are:

```text
send -> queued
queued --claim--> delivering
delivering --acknowledge--> acknowledged
delivering --retryable fail with budget--> retryable-failed
delivering --non-retryable/exhausted fail--> terminal-failed
retryable-failed --retry when due--> delivering
queued|delivering|retryable-failed --cancel--> cancelled
delivering --daemon restart--> retryable-failed or terminal-failed
```

Every effective transition increments the message revision once. `claim` and `retry` append an attempt and increment `attemptCount`. Acknowledge and fail require both the expected message revision and the active attempt UUID, so late evidence from an earlier attempt cannot close a newer one.

Acknowledgement is the only successful terminal state. It requires a bounded acknowledgement key and JSON-object evidence. Bytes accepted by a pipe, stdin, HTTP, or a provider RPC are not acknowledgement.

## Retry and dead-letter rules

The sender chooses `maxAttempts` from 1 through 5; the default is 3. Retryable failures use deterministic delays of 1, 5, 30, and 120 seconds after attempts one through four. `message.retry` rejects work before `nextAttemptAt` with `LZ-MESSAGE-RETRY-NOT-DUE` and returns the timestamp in error data.

A retryable failure on the final permitted attempt becomes `terminal-failed`. Non-retryable failures become terminal immediately. Both paths create one immutable dead-letter record with the final attempt, code, safe message, and timestamp. Replays cannot create another dead-letter row.

Failure codes are nonempty and at most 128 bytes; public failure messages are nonempty and at most 1,024 UTF-8 bytes. The bus stores no raw provider payload in failure fields.

## Restart reconciliation

Only one daemon owns the store writer. During `App` construction, before any new claim can occur, every `delivering` record is treated as uncertain work from the prior daemon:

- its active attempt closes with `LZ-MESSAGE-OWNER-LOST`;
- it becomes `retryable-failed` with the normal bounded delay when budget remains; or
- it becomes `terminal-failed` and receives dead-letter evidence when the budget is exhausted.

This implements at-least-once recovery without claiming that the prior recipient did or did not process the message. Future provider delivery must reuse the message and attempt idempotency identities.

## RPC and CLI contract

The daemon exposes:

- `message.send`
- `message.claim`
- `message.acknowledge`
- `message.fail`
- `message.retry`
- `message.cancel`
- `message.list`
- `message.show`

The CLI mirrors them under `lazooctl message ...`. All mutations require caller idempotency keys. All state-changing commands except send require the current expected revision. Claim, acknowledgement, failure, retry, and cancellation return `MessageMutationResult`; list uses monotonic `afterSequence` plus a bounded limit; show returns the complete record including attempts and dead-letter evidence.

Stable bus errors use `LZ-MESSAGE-*` and CLI exit code 11. Required families cover invalid data, employee/project/session references, not found, stale revision, invalid state, attempt mismatch, correlation mismatch, and retry-not-due. SQLite and serialization faults map to `LZ-MESSAGE-STORE` without exposing raw data.

## Security and scope boundaries

- The bus persists only bounded protocol JSON and safe public failure evidence.
- Employee/session/project references are verified before insert; causal links and attempt links are verified before transition.
- Message tools do not grant approval, change authority, change task ownership, or expand either employee's permissions.
- `desktop.snapshot`, Tauri commands, desktop capabilities, and all files under `apps/lazoo-desktop` remain unchanged.
- ConPTY, provider turns, native peer fast paths, recipient activation, worktree mutation, tasks, approvals, incidents, and resource governance remain later slices.
- Acceptance must not run `provider smoke` or consume model quota.

## Acceptance

Acceptance must prove:

1. schema 4 upgrades schema 1, 2, and 3 databases without rewriting applied migrations;
2. send is idempotent and the envelope is durably recorded before any attempt;
3. message sequences replay in order across list pagination and daemon restart;
4. claim creates one concrete attempt and stale revisions or attempt IDs fail cleanly;
5. acknowledgement correlates to the active attempt and is idempotent;
6. retryable failure schedules a bounded retry and early retry is rejected;
7. retry exhaustion creates one terminal state and one dead-letter record;
8. daemon restart reconciles an uncertain delivering attempt without duplicate logical messages;
9. cancellation is revision-checked and idempotent;
10. audit/outbox evidence exists for every effective state change; and
11. provider smoke, model turns, recipient activation, ConPTY, worktrees, and desktop capability expansion remain absent.
