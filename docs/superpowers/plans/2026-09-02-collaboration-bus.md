# Durable Collaboration Bus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans and superpowers:test-driven-development task-by-task. Do not delegate unless the owner explicitly requests parallel agents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a durable, ordered, revisioned employee-message ledger with explicit attempt acknowledgement, bounded retry/dead-letter behavior, restart reconciliation, and daemon/CLI controls without delivering a provider turn.

**Architecture:** Schema 4 separates immutable message envelopes from mutable delivery state and append-only attempt/dead-letter evidence. The store owns all state transitions in immediate transactions that also append audit/outbox events. The daemon validates bounded protocol inputs and exposes message RPCs; the CLI maps them one-for-one while leaving the desktop contract untouched.

**Tech Stack:** Rust 1.98, Serde, Rusqlite/SQLite WAL, Chrono, UUID, Tokio, Clap, existing authenticated JSON-RPC named pipe and transactional outbox.

---

### Task 1: Strict message protocol and validation contracts

**Files:**
- Create: `crates/lazoo-protocol/src/message.rs`
- Modify: `crates/lazoo-protocol/src/lib.rs`
- Modify: `crates/lazoo-protocol/tests/contracts.rs`
- Create: `crates/lazoo-domain/src/message.rs`
- Modify: `crates/lazoo-domain/src/lib.rs`
- Create: `crates/lazoo-domain/tests/message.rs`

- [ ] **Step 1: Write the failing protocol contract test**

Add a test that deserializes strict camel-case send parameters, verifies `MessageKind` and `MessageDeliveryState` kebab-case names, verifies the default maximum of three attempts, and rejects an unknown field:

```rust
#[test]
fn message_contracts_are_strict_versioned_and_kebab_case() {
    let sender = Uuid::new_v4();
    let recipient = Uuid::new_v4();
    let params: SendMessageParams = serde_json::from_value(json!({
        "sender": {"employeeId": sender},
        "recipient": {"employeeId": recipient},
        "kind": "peer",
        "body": {"text": "Need the spacing token."},
        "idempotencyKey": "message-1"
    })).unwrap();
    assert_eq!(params.max_attempts, 3);
    assert_eq!(serde_json::to_string(&MessageDeliveryState::RetryableFailed).unwrap(), r#""retryable-failed""#);
    assert!(serde_json::from_value::<SendMessageParams>(json!({
        "sender": {"employeeId": sender},
        "recipient": {"employeeId": recipient},
        "kind": "peer",
        "body": {"text": "Need the spacing token."},
        "idempotencyKey": "message-1",
        "unexpected": true
    })).is_err());
}
```

- [ ] **Step 2: Run the focused protocol test and verify RED**

Run: `cargo test -p lazoo-protocol --test contracts message_contracts_are_strict_versioned_and_kebab_case`

Expected: compilation fails because message contract types are not exported.

- [ ] **Step 3: Add the complete protocol surface**

Define and export these strict camel-case records from `message.rs`:

```rust
pub enum MessageKind { Delegation, Peer, Reply, Result, Status, System }
pub enum MessageDeliveryState { Queued, Delivering, Acknowledged, RetryableFailed, TerminalFailed, Cancelled }
pub struct MessageEndpoint { pub employee_id: Uuid, pub session_id: Option<Uuid> }
pub struct MessageReference { pub kind: String, pub target: String, pub metadata: Value }
pub struct MessageEnvelope { pub version: u32, pub id: Uuid, pub idempotency_key: String, pub correlation_id: Uuid, pub causation_id: Option<Uuid>, pub sender: MessageEndpoint, pub recipient: MessageEndpoint, pub project_id: Option<Uuid>, pub conversation_id: Option<Uuid>, pub task_id: Option<Uuid>, pub kind: MessageKind, pub body: Value, pub references: Vec<MessageReference>, pub created_at: String }
pub struct MessageActivationIntent { pub employee_id: Uuid, pub session_id: Option<Uuid>, pub requires_activation: bool }
pub struct MessageDelivery { pub state: MessageDeliveryState, pub revision: u64, pub attempt_count: u32, pub max_attempts: u32, pub next_attempt_at: Option<String>, pub last_failure_code: Option<String>, pub last_failure_message: Option<String>, pub acknowledged_at: Option<String>, pub terminal_at: Option<String>, pub updated_at: String }
pub struct MessageDeliveryAttempt { pub id: Uuid, pub number: u32, pub state: MessageDeliveryState, pub started_at: String, pub finished_at: Option<String>, pub failure_code: Option<String>, pub failure_message: Option<String>, pub acknowledgement_key: Option<String>, pub acknowledgement_evidence: Option<Value> }
pub struct MessageDeadLetter { pub attempt_id: Uuid, pub failure_code: String, pub failure_message: String, pub created_at: String }
pub struct MessageRecord { pub sequence: u64, pub envelope: MessageEnvelope, pub activation_intent: MessageActivationIntent, pub delivery: MessageDelivery, pub attempts: Vec<MessageDeliveryAttempt>, pub dead_letter: Option<MessageDeadLetter> }
pub struct MessageSummary { pub sequence: u64, pub id: Uuid, pub correlation_id: Uuid, pub causation_id: Option<Uuid>, pub sender: MessageEndpoint, pub recipient: MessageEndpoint, pub project_id: Option<Uuid>, pub conversation_id: Option<Uuid>, pub task_id: Option<Uuid>, pub kind: MessageKind, pub state: MessageDeliveryState, pub revision: u64, pub attempt_count: u32, pub max_attempts: u32, pub next_attempt_at: Option<String>, pub created_at: String, pub updated_at: String }
```

Also define strict parameters for send, claim, acknowledge, fail, retry, cancel, list, and show plus `MessageMutationResult` and `MessageReconcileResult`. Send defaults `references` to an empty vector and `maxAttempts` to three. List defaults to `afterSequence = 0` and `limit = 100`.

- [ ] **Step 4: Run the protocol suite and verify GREEN**

Run: `cargo test -p lazoo-protocol`

Expected: all protocol tests pass.

- [ ] **Step 5: Write failing domain validation tests**

Cover non-null bodies, the 256 KiB body limit, at most 32 references, nonempty bounded reference kind/target, JSON-object metadata capped at 16 KiB, 1..=5 attempts, bounded idempotency/acknowledgement/failure fields, and the fixed retry delays `[1, 5, 30, 120]` seconds.

- [ ] **Step 6: Run the focused domain test and verify RED**

Run: `cargo test -p lazoo-domain --test message`

Expected: compilation fails because message validators do not exist.

- [ ] **Step 7: Implement minimal domain validation**

Expose `validate_message_content`, `validate_message_command_key`, `validate_acknowledgement`, `validate_message_failure`, `validate_max_attempts`, and `retry_delay_seconds`. Each function returns a stable `MessageValidationError` variant and performs only the limits from the design.

- [ ] **Step 8: Run protocol/domain suites and commit**

Run: `cargo test -p lazoo-protocol -p lazoo-domain`

Expected: all focused tests pass with no warnings.

Commit: `feat: define collaboration message contracts`

### Task 2: Schema 4 and record-first message persistence

**Files:**
- Create: `migrations/0004_collaboration_bus.sql`
- Modify: `crates/lazoo-store/src/migration.rs`
- Modify: `crates/lazoo-store/src/lib.rs`
- Create: `crates/lazoo-store/src/message.rs`
- Create: `crates/lazoo-store/tests/message.rs`
- Modify: `crates/lazoo-store/tests/store.rs`

- [ ] **Step 1: Write failing migration and send/list/show tests**

Create employees through the existing store helpers, send two projectless messages, replay the first idempotency key, and assert:

```rust
assert_eq!(first.message.envelope.id, replay.message.envelope.id);
assert_eq!(replay.event_sequence, None);
assert_eq!(first.message.delivery.state, MessageDeliveryState::Queued);
assert!(first.message.attempts.is_empty());
assert!(store.list_messages(first.message.sequence, 100)?[0].sequence > first.message.sequence);
assert_eq!(store.get_message(first.message.envelope.id)?.envelope.body, json!({"text": "hello"}));
```

Extend migration tests to assert schema 4 on new databases and upgrades from versions 1, 2, and 3 while retaining the existing checksum checks.

- [ ] **Step 2: Run store tests and verify RED**

Run: `cargo test -p lazoo-store --test message --test store`

Expected: compilation fails because schema 4 and message store methods are absent.

- [ ] **Step 3: Add schema 4**

Create `messages`, `message_deliveries`, `message_delivery_attempts`, `message_dead_letters`, and `message_commands`. Enforce the six delivery states with SQL checks, use foreign keys for existing employee/session/project records, use a monotonic `messages.sequence`, and use unique `(message_id, attempt_number)` plus one dead-letter row per message.

- [ ] **Step 4: Implement record-first send and ordered reads**

Add `SendMessageInput` and these store methods:

```rust
pub fn send_message(&self, input: &SendMessageInput) -> Result<MessageMutationResult, StoreError>;
pub fn list_messages(&self, after_sequence: u64, limit: u32) -> Result<Vec<MessageSummary>, StoreError>;
pub fn get_message(&self, message_id: Uuid) -> Result<MessageRecord, StoreError>;
```

Within one immediate transaction, `send_message` must validate durable references, resolve causation/correlation, insert the immutable envelope, insert queued revision 1, insert the command mapping, append `message.queued`, and create its outbox row. Resolve an idempotent replay before revalidating current employee state.

- [ ] **Step 5: Run focused store tests and verify GREEN**

Run: `cargo test -p lazoo-store --test message --test store`

Expected: all focused tests pass, including version 1/2/3 upgrades.

- [ ] **Step 6: Commit**

Run: `git diff --check`

Commit: `feat: persist collaboration message envelopes`

### Task 3: Attempt lifecycle, bounded retry, dead letters, and recovery

**Files:**
- Modify: `crates/lazoo-store/src/message.rs`
- Modify: `crates/lazoo-store/src/lib.rs`
- Modify: `crates/lazoo-store/tests/message.rs`

- [ ] **Step 1: Write failing state-machine tests**

Cover these behaviors with one assertion-focused test per transition:

```rust
let delivering = store.claim_message(&claim_input)?;
assert_eq!(delivering.message.delivery.state, MessageDeliveryState::Delivering);
assert_eq!(delivering.message.attempts.len(), 1);

let acknowledged = store.acknowledge_message(&ack_input)?;
assert_eq!(acknowledged.message.delivery.state, MessageDeliveryState::Acknowledged);

let failed = store.fail_message(&retryable_failure)?;
assert_eq!(failed.message.delivery.state, MessageDeliveryState::RetryableFailed);
assert!(failed.message.delivery.next_attempt_at.is_some());

let exhausted = store.fail_message(&final_retryable_failure)?;
assert_eq!(exhausted.message.delivery.state, MessageDeliveryState::TerminalFailed);
assert!(exhausted.message.dead_letter.is_some());
```

Also assert stale revisions, stale attempt IDs, invalid source states, early retry, command replay, cancellation, and no duplicate dead-letter row.

- [ ] **Step 2: Run the state-machine tests and verify RED**

Run: `cargo test -p lazoo-store --test message`

Expected: compilation fails because mutation methods and message-specific errors are absent.

- [ ] **Step 3: Implement transactional transitions**

Add inputs and methods for `claim_message`, `acknowledge_message`, `fail_message`, `retry_message`, and `cancel_message`. Each method must:

1. resolve command replay first;
2. load the current record and require the expected revision;
3. require the allowed source state and active attempt where applicable;
4. update delivery and attempt/dead-letter records;
5. append one matching audit/outbox event; and
6. persist the command mapping before commit.

Use `retry_delay_seconds(attempt_count)` and RFC 3339 timestamps. Return `MessageRetryNotDue { next_attempt_at }` before changing state.

- [ ] **Step 4: Write failing restart-reconciliation test**

Claim a message, drop the store, reopen it, call `reconcile_messages`, and assert that the attempt is closed with `LZ-MESSAGE-OWNER-LOST`, the message becomes retryable when budget remains, the logical message ID is unchanged, and the event/outbox row is durable. Add the max-attempts-one variant and assert one terminal dead letter.

- [ ] **Step 5: Implement restart reconciliation and verify GREEN**

Add:

```rust
pub fn reconcile_messages(&self) -> Result<MessageReconcileResult, StoreError>;
```

It must process every `delivering` message in sequence order in one immediate transaction, close its active attempt, choose retryable or terminal state using the existing budget, append dead-letter evidence when terminal, and emit one `messages.reconciled` audit/outbox event when any rows changed.

Run: `cargo test -p lazoo-store --test message`

Expected: all message store tests pass.

- [ ] **Step 6: Run the complete store suite and commit**

Run: `cargo test -p lazoo-store`

Expected: all store tests pass with no warnings.

Commit: `feat: manage collaboration delivery attempts`

### Task 4: Daemon RPC and CLI workflow

**Files:**
- Modify: `apps/lazood/src/app.rs`
- Modify: `apps/lazood/tests/daemon.rs`
- Modify: `apps/lazooctl/src/lib.rs`
- Modify: `apps/lazooctl/tests/cli.rs`

- [ ] **Step 1: Write failing CLI mapping tests**

Parse and map `message send/claim/acknowledge/fail/retry/cancel/list/show`. Assert exact RPC method names, camel-case payload keys, automatic UUID idempotency defaults, one JSON-array references input, and CLI exit code 11 for every `LZ-MESSAGE-*` error.

- [ ] **Step 2: Run CLI tests and verify RED**

Run: `cargo test -p lazooctl --test cli`

Expected: compilation fails because `MessageCommand` is absent.

- [ ] **Step 3: Implement the CLI surface**

Add a `MessageCommand` Clap enum and a `message_request` mapper. `send` accepts employee IDs, optional session/project/conversation/task/correlation/causation IDs, `--kind`, `--body` JSON, `--references` JSON array, `--max-attempts`, and optional idempotency key. Mutation commands accept expected revision and the attempt/acknowledgement/failure fields defined by the protocol.

- [ ] **Step 4: Write failing daemon RPC tests**

Using the test store, create two employees and cover record-first send, list ordering, show, claim, stale revision, attempt-correlated acknowledgement, retry scheduling/exhaustion, cancellation, command replay, restart reconciliation, and stable error data. Snapshot the serialized `DesktopSnapshot` before and after message operations and assert it gains no message field or command.

- [ ] **Step 5: Run daemon tests and verify RED**

Run: `cargo test -p lazood --test daemon message_`

Expected: tests fail with `LZ-METHOD-NOT-FOUND` for message RPCs.

- [ ] **Step 6: Implement daemon validation and error mapping**

Add the eight RPC methods to `App::handle`, call `store.reconcile_messages()` during construction after session reconciliation, publish pending outbox events after each effective mutation, and map store/domain failures to stable `LZ-MESSAGE-*` errors. `message.list` must reject limits outside 1..=1024. Do not call provider registry/session backends from any message method.

- [ ] **Step 7: Verify daemon and CLI suites and commit**

Run: `cargo test -p lazooctl -p lazood`

Expected: all daemon and CLI tests pass.

Commit: `feat: expose collaboration bus controls`

### Task 5: Backend-only acceptance and continuity closeout

**Files:**
- Create: `scripts/acceptance/collaboration-bus.ps1`
- Create: `artifacts/acceptance/collaboration-bus-manifest.json`
- Create: `docs/verification/2026-09-02-collaboration-bus.md`
- Modify: `apps/lazood/tests/daemon.rs`
- Modify: `scripts/acceptance/first-slice.ps1`
- Modify: `docs/NEXT-SESSION.md`
- Modify: `docs/PROJECT-BRIEF.md`
- Modify: `README.md`

- [ ] **Step 1: Write the failing acceptance-contract test**

Require the script to contain markers for schema 4, idempotent record-first send, ordered replay, acknowledgement correlation, early-retry rejection, retry exhaustion, dead-letter evidence, restart reconciliation, cancellation, audit/outbox evidence, and `providerSmokeAbsent`. Assert the script does not contain `provider smoke`, desktop paths, ConPTY, worktree mutations, or model-turn commands.

- [ ] **Step 2: Run the contract test and verify RED**

Run: `cargo test -p lazood --test daemon collaboration_bus_acceptance_contract_is_backend_only`

Expected: test fails because the acceptance script is absent.

- [ ] **Step 3: Implement isolated release acceptance**

The PowerShell script must use a verified temporary data root, hidden daemon process, release binaries, and JSON CLI calls. It must create two employees, exercise every message mutation, wait only for the one-second first retry deadline, force daemon loss while one attempt is delivering, restart, inspect durable state, verify ordered event evidence, and always clean up only its validated temporary root/process.

- [ ] **Step 4: Run focused and full gates**

Run in this order:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-rust-toolchain.ps1
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --all-targets
cargo build --workspace --release --locked
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\first-slice.ps1 -SkipBuild
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\employee-roster.ps1 -SkipBuild
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\native-session-ownership.ps1 -SkipBuild
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\collaboration-bus.ps1 -SkipBuild
```

Expected: every command exits 0; the workspace test count and ignored live-only count are recorded exactly from output; all four acceptance manifests report `PASS`.

- [ ] **Step 5: Verify boundaries and line endings**

Run:

```powershell
git diff --name-only main...HEAD
git diff --check main...HEAD
git status --short
git diff --quiet main...HEAD -- apps/lazoo-desktop
```

Expected: no desktop file is listed or changed, no whitespace errors are reported, and only collaboration-bus scope plus synchronized continuity documentation is present.

- [ ] **Step 6: Record verification and next handoff**

Write the exact commands, exit codes, test counts, manifest markers, exclusions, and any manual/blocked gates in the verification document. Update the project brief, README, first-slice schema expectation, and `NEXT-SESSION.md` to mark the collaboration bus implemented and identify conversation/turn plus provider-delivery transport as the next backend dependency without claiming ConPTY or product readiness.

- [ ] **Step 7: Commit closeout**

Run: `git diff --check`

Commit: `test: verify durable collaboration bus`
