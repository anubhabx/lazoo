# Durable Conversations and Codex Turn Delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a schema-5 durable conversation/turn ledger that records canonical delivery before provider I/O and delivers one turn through an owned Codex App Server session using durable native acceptance evidence.

**Architecture:** Version the canonical message sender so the local user can be represented truthfully, then persist conversations, immutable turn input, append-only normalized events, and cross-ledger transitions in SQLite. Extend the provider session with an observed-turn path, override it for Codex so the `turn/start` response is recorded immediately, and orchestrate explicit activation, submission, retry, cancellation, and recovery through daemon RPC plus CLI commands.

**Tech Stack:** Rust 1.98.0 MSVC, Tokio, Serde/serde_json, rusqlite with SQLite WAL and immediate transactions, UUID, Clap, JSON-RPC named-pipe IPC, PowerShell 5.1 acceptance scripts, Codex App Server v2 over stdio

**Spec:** `docs/superpowers/specs/2026-09-03-conversation-turn-provider-delivery-design.md`

## Global Constraints

- Windows first, local first, and one local user.
- Backend and communication layers only; `apps/lazoo-desktop` stays unchanged.
- Reuse the canonical collaboration envelope and delivery attempt ledger.
- Record the conversation turn, delivery intent, audit event, and outbox row before provider I/O.
- Preserve native authentication, models, tools, MCP, plugins, skills, permissions, and session identity.
- Keep `ask` as the default authority mode; only an explicit user-authored command may request lead activation.
- Enable daemon-managed live delivery only for Codex in this slice.
- Never equate a successful write or accepted async request with delivery; require a durably recorded native turn ID or progress event.
- Never automatically retry a turn after acceptance or an uncertain outcome.
- Deterministic acceptance must not invoke a model; the live Codex gate is separate and quota-consuming.
- Do not add ConPTY, task, worktree, approval, managed-memory, resource-governor, Tauri, `desktop.snapshot`, or UI capabilities.

---

### Task 1: Versioned actor and conversation protocol contracts

**Files:**

- Create: `crates/lazoo-protocol/src/conversation.rs`
- Modify: `crates/lazoo-protocol/src/message.rs`
- Modify: `crates/lazoo-protocol/src/lib.rs`
- Modify: `crates/lazoo-protocol/tests/contracts.rs`
- Create: `crates/lazoo-domain/src/conversation.rs`
- Modify: `crates/lazoo-domain/src/lib.rs`
- Create: `crates/lazoo-domain/tests/conversation.rs`

**Interfaces:**

- Consumes: existing `MessageEndpoint`, `MessageKind`, `AuthorityMode`, strict camel-case Serde conventions, and message size limits.
- Produces: `MessageSender`, `UserMessageEndpoint`, `ConversationRecord`, `ConversationSummary`, `ConversationTurnRecord`, `ConversationTurnSummary`, `ConversationTurnEvent`, `ConversationMutationResult`, `TurnMutationResult`, `TurnReconcileResult`, `TurnState`, conversation/turn RPC parameter types, and conversation validation helpers.

- [x] **Step 1: Write failing message-version and conversation serialization tests**

Add contract assertions that version-1 employee senders retain the existing JSON shape and version-2 user senders serialize distinctly:

```rust
let employee = MessageSender::Employee(MessageEndpoint {
    employee_id: Uuid::nil(),
    session_id: None,
});
assert_eq!(serde_json::to_value(employee).unwrap(), json!({
    "employeeId": Uuid::nil(),
    "sessionId": null
}));

let user = MessageSender::User(UserMessageEndpoint {
    kind: UserMessageKind::User,
});
assert_eq!(serde_json::to_value(user).unwrap(), json!({"kind": "user"}));
```

Add strict round-trip tests for `ConversationRecord`, every `TurnState`, `CreateConversationParams`, `ConversationListParams`, `ConversationShowParams`, `SubmitTurnParams`, `RetryTurnParams`, `CancelTurnParams`, `TurnListParams`, and `TurnShowParams`. Assert unknown fields fail deserialization and list defaults are `afterSequence = 0`, `limit = 100`.

- [x] **Step 2: Run protocol tests and verify RED**

Run:

```powershell
cargo test -p lazoo-protocol --test contracts conversation_
cargo test -p lazoo-protocol --test contracts message_contracts_are_strict_versioned_and_kebab_case
```

Expected: compilation fails because actor-aware senders and conversation types do not exist.

- [x] **Step 3: Implement the strict protocol types**

Define these public shapes in `conversation.rs` and re-export them from `lib.rs`:

```rust
pub const CONVERSATION_CONTRACT_V1: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TurnState {
    Recorded,
    Submitting,
    Accepted,
    Completed,
    RetryableFailed,
    TerminalFailed,
    Cancelled,
    Uncertain,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SubmitTurnParams {
    pub conversation_id: Uuid,
    pub author: MessageSender,
    pub recipient: Option<MessageEndpoint>,
    pub text: String,
    #[serde(default)]
    pub activate_if_needed: bool,
    #[serde(default = "default_turn_max_attempts")]
    pub max_attempts: u32,
    pub idempotency_key: String,
}
```

Define records with every field from the design, including `owner_boot_id`, native identifiers, safe failure fields, lifecycle timestamps, and per-turn event sequence. Change `MessageEnvelope.sender` and `MessageSummary.sender` from `MessageEndpoint` to `MessageSender`. Keep `SendMessageParams.sender` as `MessageEndpoint` so the existing `message.send` RPC remains employee-to-employee and version 1.

- [x] **Step 4: Write failing domain validation tests**

Cover title trimming and its 200-byte limit, nonempty UTF-8 turn text capped at 256 KiB, idempotency keys capped at 200 bytes, event payload objects capped at 64 KiB, transcript bytes capped at 512 KiB, list limits `1..=1024`, user activation restrictions, envelope-version/sender compatibility, and allowed state transitions.

Key assertions:

```rust
assert!(validate_turn_text("hello").is_ok());
assert_eq!(validate_turn_text("   ").unwrap_err(), ConversationValidationError::TurnTextEmpty);
assert!(validate_envelope_sender(1, &employee).is_ok());
assert_eq!(
    validate_envelope_sender(1, &user).unwrap_err(),
    ConversationValidationError::EnvelopeSenderVersion
);
assert!(validate_envelope_sender(2, &user).is_ok());
```

- [x] **Step 5: Run domain tests and verify RED**

Run: `cargo test -p lazoo-domain --test conversation`

Expected: compilation fails because conversation validators and transition checks are absent.

- [x] **Step 6: Implement minimal validation and transitions**

Add `validate_conversation_title`, `validate_turn_text`, `validate_conversation_command_key`, `validate_turn_event_payload`, `validate_envelope_sender`, `validate_turn_transition`, and size constants. Permit only:

```text
recorded -> submitting | cancelled
submitting -> accepted | retryable-failed | terminal-failed | cancelled | uncertain
accepted -> completed | terminal-failed | cancelled | uncertain
retryable-failed -> recorded | cancelled
completed | terminal-failed | cancelled | uncertain -> no transition
```

- [x] **Step 7: Verify focused suites and commit**

Run:

```powershell
cargo test -p lazoo-protocol --test contracts
cargo test -p lazoo-domain --test conversation
git diff --check
```

Expected: both suites pass with no formatting or whitespace errors.

Commit: `feat: define conversation turn contracts`

### Task 2: Schema 5 and atomic cross-ledger persistence

**Files:**

- Create: `migrations/0005_conversations.sql`
- Modify: `crates/lazoo-store/src/migration.rs`
- Create: `crates/lazoo-store/src/conversation.rs`
- Modify: `crates/lazoo-store/src/message.rs`
- Modify: `crates/lazoo-store/src/lib.rs`
- Create: `crates/lazoo-store/tests/conversation.rs`
- Modify: `crates/lazoo-store/tests/message.rs`
- Modify: `crates/lazoo-store/tests/store.rs`

**Interfaces:**

- Consumes: Task 1 protocol/domain types plus existing message transaction, audit, outbox, retry, command-replay, employee, session, and project helpers.
- Produces: `CreateConversationInput`, `RecordTurnInput`, `BeginTurnSubmissionInput`, `ObserveTurnInput`, `FailTurnInput`, `RetryTurnInput`, `RequestTurnCancelInput`, `FinishTurnCancelInput`, `MarkTurnUncertainInput`, and `reconcile_turns` store methods.

- [x] **Step 1: Write failing migration compatibility tests**

Extend `store.rs` to require schema 5 for new databases and upgrades from schemas 1, 2, 3, and 4. Seed a schema-4 employee message before upgrade, reopen the store, and assert its envelope is still version 1 with the same employee sender and delivery history. End every migration case with:

```rust
let violations: i64 = connection.query_row(
    "SELECT COUNT(*) FROM pragma_foreign_key_check",
    [],
    |row| row.get(0),
)?;
assert_eq!(violations, 0);
```

- [x] **Step 2: Run migration tests and verify RED**

Run:

```powershell
cargo test -p lazoo-store --test store schema_
cargo test -p lazoo-store --test store version_
```

Expected: assertions report schema 4 because migration 5 is absent.

- [x] **Step 3: Add the schema-5 migration**

Rebuild `messages`, `message_deliveries`, `message_delivery_attempts`, `message_dead_letters`, and `message_commands` in dependency order. Preserve all existing IDs, sequences, revisions, attempts, dead letters, and command mappings. The rebuilt message parent uses:

```sql
envelope_version INTEGER NOT NULL CHECK (envelope_version IN (1, 2)),
sender_kind TEXT NOT NULL CHECK (sender_kind IN ('employee', 'user')),
sender_employee_id TEXT REFERENCES employees(id),
sender_session_id TEXT REFERENCES employee_sessions(id),
CHECK (
    (sender_kind = 'employee' AND sender_employee_id IS NOT NULL) OR
    (sender_kind = 'user' AND sender_employee_id IS NULL AND sender_session_id IS NULL)
)
```

Create `conversations`, `conversation_turns`, `conversation_turn_events`, `conversation_commands`, and `turn_commands`. Add checks for scope, states, revisions, unique per-turn event sequences, one canonical message per turn, nullable owner boot/native IDs, and foreign keys to projects, employees, sessions, conversations, and messages.

- [x] **Step 4: Make message loading actor-aware**

Update message row parsing and insertion so existing `send_message` creates version 1 with `sender_kind = 'employee'`. Add transaction-scoped internal helpers used by conversation persistence:

```rust
pub(crate) fn insert_message_tx(
    transaction: &Transaction<'_>,
    input: &InsertMessageInput,
) -> Result<MessageMutationResult, StoreError>;

pub(crate) fn claim_message_tx(
    transaction: &Transaction<'_>,
    input: &ClaimMessageInput,
) -> Result<MessageMutationResult, StoreError>;
```

`InsertMessageInput` contains every existing send field plus `version: u32` and `sender: MessageSender`; public `send_message` constructs version 1 with an employee sender. Add equivalent internal helpers for acknowledge, fail, retry, and cancel so a turn transition and its canonical delivery transition can commit atomically.

- [x] **Step 5: Write failing conversation and turn persistence tests**

Cover:

- idempotent projectless and project conversation creation;
- missing/disabled/unavailable lead rejection;
- user- and employee-authored turns;
- user sender stored as envelope version 2 without a synthetic employee;
- immutable text and ordered list/show;
- record-first audit/outbox order;
- atomic begin submission plus message claim;
- acceptance only after a native ID/progress observation;
- append-only event order and transcript truncation;
- retryable/terminal pre-acceptance failure;
- cancel before I/O and cancel after acceptance;
- stale revisions and command replays; and
- owner-loss reconciliation to `uncertain`.

The record-first test must assert:

```rust
assert_eq!(recorded.turn.state, TurnState::Recorded);
assert_eq!(recorded.message.delivery.state, MessageDeliveryState::Queued);
assert!(recorded.turn.message_attempt_id.is_none());
assert!(recorded.turn.native_turn_id.is_none());
assert_eq!(replay.turn.id, recorded.turn.id);
assert_eq!(replay.event_sequence, None);
```

- [x] **Step 6: Run store tests and verify RED**

Run: `cargo test -p lazoo-store --test conversation --test message --test store`

Expected: compilation fails because conversation store methods and schema-5 types are absent.

- [x] **Step 7: Implement conversation creation and record-first turn insertion**

Expose these methods on `Store`:

```rust
pub fn create_conversation(&self, input: &CreateConversationInput)
    -> Result<ConversationMutationResult, StoreError>;
pub fn list_conversations(&self, after_sequence: u64, limit: u32)
    -> Result<Vec<ConversationSummary>, StoreError>;
pub fn get_conversation(&self, id: Uuid)
    -> Result<ConversationRecord, StoreError>;
pub fn record_turn(&self, input: &RecordTurnInput)
    -> Result<TurnMutationResult, StoreError>;
pub fn list_turns(&self, conversation_id: Uuid, after_sequence: u64, limit: u32)
    -> Result<Vec<ConversationTurnSummary>, StoreError>;
pub fn get_turn(&self, id: Uuid)
    -> Result<ConversationTurnRecord, StoreError>;
```

`record_turn` must insert the turn, version-2 message, delivery intent, both command mappings, audit events, and outbox rows in one immediate transaction. Resolve command replay before current-state validation.

- [x] **Step 8: Implement atomic delivery transitions**

Expose:

```rust
pub fn begin_turn_submission(&self, input: &BeginTurnSubmissionInput)
    -> Result<TurnMutationResult, StoreError>;
pub fn observe_turn(&self, input: &ObserveTurnInput)
    -> Result<TurnMutationResult, StoreError>;
pub fn fail_turn(&self, input: &FailTurnInput)
    -> Result<TurnMutationResult, StoreError>;
pub fn retry_turn(&self, input: &RetryTurnInput)
    -> Result<TurnMutationResult, StoreError>;
pub fn request_turn_cancel(&self, input: &RequestTurnCancelInput)
    -> Result<TurnMutationResult, StoreError>;
pub fn finish_turn_cancel(&self, input: &FinishTurnCancelInput)
    -> Result<TurnMutationResult, StoreError>;
pub fn mark_turn_uncertain(&self, input: &MarkTurnUncertainInput)
    -> Result<TurnMutationResult, StoreError>;
pub fn reconcile_turns(&self, current_boot_id: Uuid)
    -> Result<TurnReconcileResult, StoreError>;
```

Every effective mutation must update the linked message when required, append exactly one audit/outbox event per public state change, persist the command mapping, and return no new event sequence on replay. `ObserveTurnInput` accepts only normalized/redacted payloads and handles acceptance plus message acknowledgement in one transaction.

- [x] **Step 9: Verify the complete store suite and commit**

Run:

```powershell
cargo test -p lazoo-store
cargo fmt --all -- --check
git diff --check
```

Expected: schema 5, all upgrade paths, message compatibility, conversation behavior, and foreign-key integrity pass.

Commit: `feat: persist conversations and turn delivery`

### Task 3: Observable Codex turn acceptance

**Files:**

- Modify: `crates/lazoo-providers/src/contract.rs`
- Modify: `crates/lazoo-providers/src/codex.rs`
- Modify: `crates/lazoo-providers/src/registry.rs`
- Modify: `crates/lazoo-providers/src/lib.rs`
- Modify: `crates/lazoo-providers/tests/contract.rs`
- Modify: `crates/lazoo-providers/tests/codex.rs`

**Interfaces:**

- Consumes: existing `ProviderSession::turn`, `ProviderEvent`, Codex JSON-RPC control, redaction, progress timeout, and `turn/interrupt` support.
- Produces: `TurnEventObserver`, default `ProviderSession::turn_observed`, and a Codex override that publishes native acceptance before collecting completion.

- [ ] **Step 1: Write failing observer contract tests**

Create a collecting observer and assert a provider using the default path receives each existing event in order. Add a rejecting observer and assert its failure stops the observed call with a stable observer error while leaving ordinary `turn` behavior unchanged.

Define the wished-for interface in the test:

```rust
pub trait TurnEventObserver: Send + Sync {
    fn observe(&self, event: &ProviderEvent) -> Result<(), ProviderError>;
}

fn turn_observed<'a>(
    &'a self,
    request: &'a TurnRequest,
    observer: &'a dyn TurnEventObserver,
) -> AdapterFuture<'a, Vec<ProviderEvent>>;
```

- [ ] **Step 2: Run contract tests and verify RED**

Run: `cargo test -p lazoo-providers --test contract observed_`

Expected: compilation fails because `TurnEventObserver` and `turn_observed` do not exist.

- [ ] **Step 3: Implement the default observed-turn path**

Add `ProviderError::Observer(String)`. The default implementation awaits the existing `turn`, then invokes the observer once for each normalized event in order. Re-export the observer trait. Existing OpenCode, Grok, Antigravity, and Claude implementations require no behavior change.

- [ ] **Step 4: Write failing Codex immediate-acceptance tests**

Drive the existing fake App Server fixture so `turn/start` returns `turn-1` but the `turn/started` notification is delayed. Assert the observer sees exactly one `ProviderEvent::TurnStarted` with `thread-1`/`turn-1` before the first streamed delta, and that the returned vector does not duplicate the delayed native notification.

Add a rejecting observer assertion that the adapter sends best-effort `turn/interrupt` for the same thread/turn and returns `ProviderError::Observer`.

- [ ] **Step 5: Run Codex tests and verify RED**

Run: `cargo test -p lazoo-providers --test codex observed_`

Expected: the default observer receives no event until completion and the immediate-order assertion fails.

- [ ] **Step 6: Override Codex observed delivery**

Split Codex turn handling into acceptance and collection. Immediately after validating `/turn/id` from the `turn/start` response, emit:

```rust
ProviderEvent::TurnStarted {
    provider: ProviderKind::Codex,
    session_id: self.session_id.clone(),
    turn_id: Some(turn_id.clone()),
    raw: None,
}
```

Call the observer for every subsequent normalized event before retaining it. Suppress only the duplicate `turn/started` notification with the same thread/turn pair. On observer error, send best-effort `turn/interrupt`, clear `active_turn`, and return the observer error. Keep progress and total timeouts unchanged.

- [ ] **Step 7: Verify provider suites and commit**

Run:

```powershell
cargo test -p lazoo-providers --test contract --test codex
cargo test -p lazoo-providers
cargo fmt --all -- --check
git diff --check
```

Expected: non-live provider tests pass; quota-consuming ignored tests remain ignored.

Commit: `feat: observe codex turn acceptance`

### Task 4: Daemon conversation and turn orchestration

**Files:**

- Create: `apps/lazood/src/app/turn.rs`
- Modify: `apps/lazood/src/app.rs`
- Modify: `apps/lazood/src/lib.rs`
- Modify: `apps/lazood/tests/daemon.rs`

**Interfaces:**

- Consumes: Task 2 store transitions, Task 3 observed provider path, existing session warm/start/stop lifecycle, provider registry, event publication, and RPC error conventions.
- Produces: daemon RPC handlers for conversation/turn operations, `ManagedSession::turn_observed`, cancellation, deterministic fake delivery, and startup reconciliation.

- [ ] **Step 1: Extend the fake managed-session contract in tests**

Add `turn_observed` and `cancel` to the daemon's `ManagedSession` trait and update `FakeSessionBackend` so each fake can emit an exact event script, reject before acceptance, block until released, or fail an observer call. Retain existing session start/stop counters.

- [ ] **Step 2: Write failing conversation RPC tests**

Cover `conversation.create/list/show` for projectless and registered-project scope, idempotent replay, route-ready lead validation, stable errors, list bounds, and unchanged `DesktopSnapshot` serialization.

Run: `cargo test -p lazood --test daemon conversation_`

Expected: RPC responses contain `LZ-METHOD-NOT-FOUND`.

- [ ] **Step 3: Implement conversation RPC handlers in the focused module**

Declare `mod turn;` from `app.rs`. Put the new parameter parsing, store calls, and conversation/turn error mapping in `app/turn.rs`, exposing only `pub(super)` methods needed by `App::handle`. Add these dispatch cases:

```rust
"conversation.create" => self.create_conversation(&request),
"conversation.list" => self.conversation_list(request.params),
"conversation.show" => self.conversation_show(request.params),
"turn.submit" => self.submit_turn(&request).await,
"turn.retry" => self.retry_turn(&request).await,
"turn.cancel" => self.cancel_turn(&request).await,
"turn.list" => self.turn_list(request.params),
"turn.show" => self.turn_show(request.params),
```

Map conversation/turn failures to exit-family codes `LZ-CONVERSATION-*` and `LZ-TURN-*`. Do not add a desktop field or command.

- [ ] **Step 4: Write failing record-first and acceptance-evidence tests**

Submit a user turn to an already warm fake Codex session. Pause the fake before its first event and assert through a second store handle that the turn is `submitting`, the message attempt exists, and input/audit/outbox rows are durable. Release `TurnStarted`, then assert the turn becomes `accepted` and the linked message becomes `acknowledged` with native IDs. Release completion and assert ordered durable events plus `completed` state.

Add tests proving a fake successful write without an observed event does not acknowledge the message, and a non-Codex employee returns `LZ-TURN-PROVIDER-UNSUPPORTED` before provider I/O.

- [ ] **Step 5: Run turn delivery tests and verify RED**

Run: `cargo test -p lazood --test daemon turn_submit_`

Expected: `turn.submit` is unavailable and the fake session cannot accept observed turns.

- [ ] **Step 6: Implement active-session delivery and observation**

Add `live_turns: Arc<Mutex<BTreeMap<Uuid, Arc<dyn ManagedSession>>>>` to `App`. Implement an `AppTurnObserver` that owns a cloned `Store` and event sender, converts `ProviderEvent` into bounded `ObserveTurnInput`, commits it, and publishes resulting outbox rows.

For `turn.submit`:

1. call `record_turn`;
2. return terminal/retry states without provider I/O on replay;
3. resolve the specified or active recipient session;
4. if absent, allow only `author = user` plus `activateIfNeeded = true` to call the existing session start path;
5. reject every provider except Codex;
6. call `begin_turn_submission` before invoking the session;
7. insert the local turn/session pair in `live_turns`;
8. call `turn_observed` with the immutable text and message correlation UUID;
9. convert definite pre-acceptance errors through `fail_turn`, but preserve accepted/partial operations as `uncertain` when the provider result is not provable; and
10. remove the live turn mapping on every return path.

Refactor `start_new_session` only enough to return the typed ready session record to both `session.warm` and turn activation. Do not change its ownership or Job Object guarantees.

- [ ] **Step 7: Write failing activation, retry, cancel, and reconciliation tests**

Cover:

- `activateIfNeeded` defaults false;
- explicit user activation uses one session start and does not send before ready;
- employee authors cannot activate dormant recipients;
- configuration/availability is revalidated immediately before dispatch;
- submit replay resumes only a `recorded` turn with a queued message and no attempt;
- due retry reuses the same local turn/message and creates the next attempt;
- early retry is rejected;
- cancel-before-I/O cancels both records;
- cancel-after-acceptance invokes provider cancel and waits for observed evidence;
- missing live handle produces `uncertain`, not `cancelled`; and
- startup reconciliation marks `submitting`/`accepted` prior-boot turns uncertain without PID/session adoption.

- [ ] **Step 8: Run lifecycle tests and verify RED**

Run:

```powershell
cargo test -p lazood --test daemon turn_
cargo test -p lazood --test daemon app_startup_reconciles_
```

Expected: the missing lifecycle handlers and reconciliation assertions fail.

- [ ] **Step 9: Implement retry, cancel, and startup reconciliation**

Use Task 2 atomic transitions. `turn.retry` first advances the due canonical message and local turn, then uses the same delivery function. `turn.cancel` persists its request before calling `ManagedSession::cancel`; only an observed native cancellation finishes as cancelled. Call `store.reconcile_turns(boot_id)` after session and message reconciliation during `App` construction, then publish pending outbox events.

- [ ] **Step 10: Verify daemon suites and commit**

Run:

```powershell
cargo test -p lazood
cargo clippy -p lazood --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Expected: all daemon unit/integration tests pass without warnings, and existing session/message behavior remains green.

Commit: `feat: deliver durable conversation turns`

### Task 5: CLI conversation workflow

**Files:**

- Modify: `apps/lazooctl/src/lib.rs`
- Modify: `apps/lazooctl/tests/cli.rs`

**Interfaces:**

- Consumes: Task 1 RPC params and Task 4 daemon method names/error families.
- Produces: `conversation create/list/show` and `turn submit/retry/cancel/list/show` CLI mappings with stable JSON and exit code 12.

- [ ] **Step 1: Write failing CLI parser and mapping tests**

Assert exact method names and camel-case payloads for:

```powershell
lazooctl conversation create <lead-employee-id> --title "First task"
lazooctl conversation list --after-sequence 0 --limit 100
lazooctl conversation show <conversation-id>
lazooctl turn submit <conversation-id> --text "Reply with exactly READY" --activate-if-needed
lazooctl turn submit <conversation-id> --author-employee <employee-id> --recipient-employee <employee-id> --text "status"
lazooctl turn retry <turn-id> --expected-revision 3
lazooctl turn cancel <turn-id> --expected-revision 2
lazooctl turn list <conversation-id> --after-sequence 0 --limit 100
lazooctl turn show <turn-id>
```

Assert generated idempotency keys are nonempty UUID strings, `--json` preserves the protocol result, and both `LZ-CONVERSATION-*` and `LZ-TURN-*` map to exit code 12.

- [ ] **Step 2: Run CLI tests and verify RED**

Run:

```powershell
cargo test -p lazooctl --test cli conversation_
cargo test -p lazooctl --test cli turn_
```

Expected: Clap rejects the missing subcommands and mapping types do not exist.

- [ ] **Step 3: Implement CLI commands and request mapping**

Add `ConversationCommand` and `TurnCommand` enums. Parse UUIDs through Clap, accept one UTF-8 `--text` value, represent the default author as `UserMessageEndpoint { kind: User }`, require both employee IDs for employee-authored delivery, and generate idempotency UUIDs when omitted. Do not expose raw provider payloads or an arbitrary daemon method.

- [ ] **Step 4: Verify CLI and daemon integration and commit**

Run:

```powershell
cargo test -p lazooctl --test cli
cargo test -p lazooctl -p lazood
cargo clippy -p lazooctl --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Expected: CLI mappings, daemon handlers, stable JSON, and exit codes pass.

Commit: `feat: expose conversation turn controls`

### Task 6: Deterministic acceptance, live Codex gate, and continuity closeout

**Files:**

- Create: `tests/fixtures/turn-transport/Cargo.toml`
- Create: `tests/fixtures/turn-transport/src/main.rs`
- Modify: `Cargo.toml`
- Create: `scripts/acceptance/conversation-delivery.ps1`
- Create: `artifacts/acceptance/conversation-delivery-manifest.json`
- Create: `docs/verification/2026-09-03-conversation-turn-provider-delivery.md`
- Modify: `apps/lazood/tests/daemon.rs`
- Modify: `scripts/acceptance/first-slice.ps1`
- Modify: `docs/NEXT-SESSION.md`
- Modify: `docs/PROJECT-BRIEF.md`
- Modify: `README.md`

**Interfaces:**

- Consumes: release daemon/CLI binaries, schema 5, fake turn transport, all prior acceptance contracts, and explicit live Codex provider support.
- Produces: isolated deterministic acceptance manifest, explicit live-gate evidence or exact blocker, synchronized project state, and the next backend handoff.

- [ ] **Step 1: Write the failing acceptance-contract test**

Require `conversation-delivery.ps1` to contain markers for:

```text
schema5
projectlessConversation
projectConversation
userAuthorship
employeeAuthorship
recordFirstTurn
envelopeV2
activationExplicit
employeeActivationBlocked
nativeTurnIdBeforeAcknowledgement
progressDurable
retryIdentityPreserved
cancelEvidenceRequired
ownerLossUncertain
transcriptBounded
auditOutboxEvidence
modelInvocationAbsent
desktopCapabilityExpansionAbsent
```

Assert the deterministic script contains no `provider smoke`, no live Codex prompt, no desktop path, no ConPTY command, and no worktree mutation.

- [ ] **Step 2: Run the contract test and verify RED**

Run: `cargo test -p lazood --test daemon conversation_delivery_acceptance_contract_is_backend_only`

Expected: the test fails because the script and fixture are absent.

- [ ] **Step 3: Implement the deterministic fixture and acceptance script**

The fixture is built and staged as a disposable `codex.exe` ahead of the real executable only inside deterministic acceptance. It implements `--version` plus the Codex App Server v2 stdio methods used by Lazoo: initialize/account readiness, `thread/start`, `thread/resume`, `turn/start`, streamed acceptance/progress/completion notifications, `turn/interrupt`, and process exit before or after acceptance. The fixture accepts deterministic native thread/turn IDs from its isolated environment and never connects to a model or network service.

The PowerShell script must:

- validate a unique temporary data root before cleanup;
- run hidden release daemon/fixture processes;
- prepend only the validated fixture directory to the child daemon's PATH, then create route-ready Codex employees through the ordinary non-model probe;
- exercise every marker above through JSON CLI calls;
- force one daemon restart during an accepted turn;
- verify message, turn, audit, and outbox evidence after restart;
- write the manifest even on failure; and
- stop only PIDs it created and remove only its validated temporary root.

- [ ] **Step 4: Run focused deterministic acceptance**

Run:

```powershell
cargo test -p lazood --test daemon conversation_delivery_acceptance_contract_is_backend_only
cargo build --workspace --release --locked
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\conversation-delivery.ps1 -SkipBuild
```

Expected: the contract test passes and `conversation-delivery-manifest.json` reports `PASS` with `modelInvocationAbsent = true`.

- [ ] **Step 5: Run the complete deterministic gate matrix**

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
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\conversation-delivery.ps1 -SkipBuild
```

Record exact test counts, ignored live-only counts, command exit codes, manifest durations, and failure text.

- [ ] **Step 6: Run one explicit quota-consuming Codex live gate**

Only after Step 5 passes, remove the fixture PATH override and run the real installed Codex path once with a minimal prompt that cannot authorize tools or broaden permissions:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\conversation-delivery.ps1 -SkipBuild -Provider codex -LivePrompt "Reply with exactly LAZOO_TURN_OK. Do not use tools."
```

Expected: native session identity, native turn ID before message acknowledgement, durable completion, exact `LAZOO_TURN_OK` output, and clean provider-tree stop. If authentication, policy, rate limit, or quota blocks the call, preserve the exact blocker and do not label live delivery passed.

- [ ] **Step 7: Verify scope, line endings, and continuity**

Run:

```powershell
git diff --name-only main...HEAD
git diff --check main...HEAD
git status --short
git diff --quiet main...HEAD -- apps/lazoo-desktop
git ls-files --eol
git check-attr text eol -- docs/superpowers/specs/2026-09-03-conversation-turn-provider-delivery-design.md docs/superpowers/plans/2026-09-03-conversation-turn-provider-delivery.md
```

Expected: no desktop diff, no whitespace errors, no unexpected CRLF/LF mismatch, and only conversation/delivery scope plus synchronized continuity artifacts.

Update README and project brief to describe only verified behavior. Rewrite `docs/NEXT-SESSION.md` around the next unmet backend dependency; do not claim ConPTY, task orchestration, worktrees, approvals, managed memory, other provider delivery, or product readiness.

- [ ] **Step 8: Write verification evidence and commit closeout**

The verification document must list every command, exit code, exact test count, acceptance marker, live result or blocker, and explicit exclusion. Confirm every plan checkbox maps to committed behavior before the final commit.

Run:

```powershell
git diff --check
git diff --cached --check
```

Commit: `test: verify conversation turn delivery`
