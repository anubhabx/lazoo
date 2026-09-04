# Durable Task and Delegation Graph Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a backend-only, restart-durable task/delegation graph whose state advances only from explicit user decisions or linked provider evidence.

**Architecture:** A user-authored turn creates its root task atomically and reuses the turn ID, text, canonical message, conversation, and lead. Child tasks form an immutable bounded tree, inherit the most restrictive authority, and create their canonical delegation message only when accepted. Task state/evidence is transactional with audit/outbox publication and is correlated to existing turn/session truth without automatically dispatching workers.

**Tech Stack:** Rust 1.98, Serde, UUID, rusqlite/SQLite WAL, JSON-RPC over the existing authenticated named pipe, Clap CLI, PowerShell 5-compatible release acceptance.

**Spec:** `docs/superpowers/specs/2026-09-04-task-delegation-graph-design.md`

## Global Constraints

- Windows-first, local-first, single-user only.
- Schema advances from 5 to 6 without rewriting migration checksums.
- `ask` is the default authority mode; authority order is `ask < self-decision < full-autonomy`.
- Root task ID equals its creating user-turn ID; no second prompt/message path.
- Maximum child depth is 4 and maximum nonterminal direct children is 8.
- Task `running` requires linked native acceptance or durable progress evidence.
- No automatic worker dispatch, provider tool bridge, worktree mutation, ConPTY, generic approval engine, managed memory, resource scheduling, provider smoke, or live model invocation.
- No changes under `apps/lazoo-desktop`; `desktop.snapshot` and Tauri capabilities remain unchanged.

---

### Task 1: Versioned task protocol and validation

**Files:**
- Create: `crates/lazoo-protocol/src/task.rs`
- Modify: `crates/lazoo-protocol/src/lib.rs`
- Create: `crates/lazoo-protocol/tests/task.rs`
- Create: `crates/lazoo-domain/src/task.rs`
- Modify: `crates/lazoo-domain/src/lib.rs`
- Create: `crates/lazoo-domain/tests/task.rs`

**Interfaces:**
- Consumes: `AuthorityMode`, `MessageEndpoint`, `MessageReference`, `MessageSender`, and `TurnState` from `lazoo-protocol`.
- Produces: the task contract and validation functions used by store, daemon, CLI, and later tasks.

- [ ] **Step 1: Write failing protocol serialization tests**

Add literal JSON round-trip cases for these exact public types:

```rust
pub const TASK_CONTRACT_V1: u32 = 1;

pub enum TaskRole { Lead, Worker }
pub enum TaskState {
    Proposed, Accepted, Running, CancelRequested,
    Completed, Failed, Cancelled, Uncertain,
}
pub enum TaskEvidenceKind {
    Proposed, Accepted, Started, Progress, Completed,
    Failed, CancelRequested, Cancelled, Uncertain,
}
pub enum TaskReportKind { Progress, Completed, Failed }
pub enum TaskResolution { Completed, Failed, Cancelled }

pub struct TaskRecord {
    pub sequence: u64,
    pub version: u32,
    pub id: Uuid,
    pub root_task_id: Uuid,
    pub parent_task_id: Option<Uuid>,
    pub depth: u32,
    pub project_id: Option<Uuid>,
    pub conversation_id: Uuid,
    pub creation_turn_id: Option<Uuid>,
    pub delegation_message_id: Option<Uuid>,
    pub creator: MessageSender,
    pub assigned_employee_id: Uuid,
    pub role: TaskRole,
    pub objective: String,
    pub references: Vec<MessageReference>,
    pub requested_authority: AuthorityMode,
    pub effective_authority: AuthorityMode,
    pub employee_configuration_revision: u64,
    pub roster_revision: u64,
    pub state: TaskState,
    pub revision: u64,
    pub active_turn_id: Option<Uuid>,
    pub active_session_id: Option<Uuid>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub terminal_at: Option<String>,
}

pub struct TaskEvidence {
    pub sequence: u64,
    pub id: Uuid,
    pub task_id: Uuid,
    pub task_revision: u64,
    pub kind: TaskEvidenceKind,
    pub actor: MessageSender,
    pub message_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub detail: Value,
    pub created_at: String,
}

pub struct TaskSummary {
    pub sequence: u64,
    pub id: Uuid,
    pub root_task_id: Uuid,
    pub parent_task_id: Option<Uuid>,
    pub depth: u32,
    pub project_id: Option<Uuid>,
    pub conversation_id: Uuid,
    pub assigned_employee_id: Uuid,
    pub role: TaskRole,
    pub objective: String,
    pub requested_authority: AuthorityMode,
    pub effective_authority: AuthorityMode,
    pub state: TaskState,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
    pub terminal_at: Option<String>,
}
pub struct TaskDetail {
    pub task: TaskRecord,
    pub children: Vec<TaskSummary>,
    pub evidence: Vec<TaskEvidence>,
}
pub struct TaskMutationResult {
    pub task: TaskRecord,
    pub delegation_message: Option<MessageRecord>,
    pub event_sequence: Option<u64>,
}
pub struct TaskReconcileResult {
    pub tasks: Vec<TaskRecord>,
    pub event_sequence: Option<u64>,
}
```

Define these strict camel-case parameter structs. Every mutation includes `idempotencyKey`; every state mutation includes `expectedRevision`. List defaults are `afterSequence = 0` and `limit = 100`.

```rust
pub struct DelegateTaskParams {
    pub parent_task_id: Uuid,
    pub actor: MessageEndpoint,
    pub assigned_employee_id: Uuid,
    pub objective: String,
    #[serde(default)]
    pub references: Vec<MessageReference>,
    pub requested_authority: Option<AuthorityMode>,
    pub roster_revision: u64,
    pub idempotency_key: String,
}
pub struct AcceptTaskParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub idempotency_key: String,
}
pub struct StartTaskParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub actor: MessageEndpoint,
    pub turn_id: Uuid,
    #[serde(default = "empty_object")]
    pub evidence: Value,
    pub idempotency_key: String,
}
pub struct ReportTaskParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub actor: MessageEndpoint,
    pub kind: TaskReportKind,
    #[serde(default = "empty_object")]
    pub evidence: Value,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub idempotency_key: String,
}
pub struct CancelTaskParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub actor: MessageSender,
    #[serde(default = "empty_object")]
    pub evidence: Value,
    pub idempotency_key: String,
}
pub struct ResolveTaskParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub resolution: TaskResolution,
    #[serde(default = "empty_object")]
    pub evidence: Value,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub idempotency_key: String,
}
pub struct TaskListParams {
    #[serde(default)]
    pub after_sequence: u64,
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    pub root_task_id: Option<Uuid>,
    pub parent_task_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub assigned_employee_id: Option<Uuid>,
    pub state: Option<TaskState>,
}
pub struct TaskShowParams { pub id: Uuid }
```

- [ ] **Step 2: Run protocol tests and verify RED**

Run: `cargo test -p lazoo-protocol --test task`

Expected: compilation fails because `lazoo_protocol::task` and the task contract types do not exist.

- [ ] **Step 3: Implement the exact protocol types**

Use `#[serde(rename_all = "kebab-case")]` for enums and `#[serde(deny_unknown_fields, rename_all = "camelCase")]` for records and parameters. Export every public task type from `crates/lazoo-protocol/src/lib.rs`. Write `TaskSummary` with the identity, ancestry, assignment, authority, state, revision, and timestamp fields from `TaskRecord`; do not use flattening or untyped metadata.

- [ ] **Step 4: Run protocol tests and verify GREEN**

Run: `cargo test -p lazoo-protocol --test task`

Expected: all task protocol tests pass and unknown fields are rejected.

- [ ] **Step 5: Write failing domain tests**

Name the production break caught by each test and assert literal expected errors for:

- empty/oversized objectives using the current turn-text limit;
- more than 32 references or invalid reference metadata;
- task evidence that is not a JSON object or exceeds 16 KiB;
- blank/oversized failure code and message;
- depth 5 and a requested authority greater than its parent/assignee ceiling;
- allowed and rejected lifecycle transitions; and
- authority minimum across all three authority modes.

- [ ] **Step 6: Run domain tests and verify RED**

Run: `cargo test -p lazoo-domain --test task`

Expected: compilation fails because task validation functions and `TaskValidationError` are absent.

- [ ] **Step 7: Implement minimal validation**

Expose these exact functions:

```rust
pub fn validate_task_objective(value: &str) -> Result<String, TaskValidationError>;
pub fn validate_task_references(value: &[MessageReference]) -> Result<(), TaskValidationError>;
pub fn validate_task_evidence(value: &Value) -> Result<(), TaskValidationError>;
pub fn validate_task_failure(code: &str, message: &str) -> Result<(), TaskValidationError>;
pub fn validate_task_depth(depth: u32) -> Result<(), TaskValidationError>;
pub const fn effective_authority(
    parent: AuthorityMode,
    requested: AuthorityMode,
    assignee: AuthorityMode,
) -> AuthorityMode;
pub const fn task_transition_allowed(from: TaskState, to: TaskState) -> bool;
```

Reuse message-reference validation instead of duplicating its limits.

- [ ] **Step 8: Verify focused suites and commit**

Run: `cargo test -p lazoo-protocol -p lazoo-domain`

Expected: both packages pass with no warnings.

Commit: `feat: define durable task contracts`

---

### Task 2: Schema 6, migration backfill, and root task creation

**Files:**
- Create: `migrations/0006_task_graph.sql`
- Modify: `crates/lazoo-store/src/migration.rs`
- Create: `crates/lazoo-store/src/task.rs`
- Modify: `crates/lazoo-store/src/lib.rs`
- Modify: `crates/lazoo-store/src/conversation.rs`
- Modify: `crates/lazoo-store/src/message.rs`
- Modify: `crates/lazoo-store/tests/store.rs`
- Create: `crates/lazoo-store/tests/task.rs`
- Modify: `crates/lazoo-store/tests/conversation.rs`
- Modify: `crates/lazoo-store/tests/message.rs`

**Interfaces:**
- Consumes: Task 1 types/validators and the existing transactional `record_turn` path.
- Produces: schema-6 task persistence, ordered reads, and one atomic user-turn/root-task creation path.

- [ ] **Step 1: Write failing schema and backfill tests**

Extend store migration tests to require schema 6 for new databases and upgrades from versions 1 through 5. Build a version-5 fixture containing user and employee turns in every `TurnState`, upgrade it, and assert literal mappings from the design: user turn IDs become root task IDs, user messages point to those tasks, employee legacy turns/messages have null task IDs, authority is `ask`, and prior audit/command data remains intact. Add message tests proving an unknown task ID and mismatched project/conversation scope fail without inserting a message.

- [ ] **Step 2: Write the failing root-task transaction test**

Submit one user turn and assert:

```rust
assert_eq!(result.turn.id, result.task.as_ref().unwrap().id);
assert_eq!(result.turn.task_id, Some(result.turn.id));
assert_eq!(result.message.envelope.task_id, Some(result.turn.id));
assert_eq!(result.task.as_ref().unwrap().objective, "inspect the parser");
assert_eq!(result.task.as_ref().unwrap().state, TaskState::Accepted);
```

Replay the same idempotency key and assert one task, one `accepted` evidence row, and no new audit/outbox row. Add a user follow-up with an explicit existing root task ID and assert it creates no second task. Add an employee-authored turn without a task ID and assert rejection.

- [ ] **Step 3: Run store tests and verify RED**

Run: `cargo test -p lazoo-store --test store --test task --test conversation`

Expected: schema remains 5 and task fields/methods are missing.

- [ ] **Step 4: Add schema 6**

Create `tasks`, `task_evidence`, and `task_commands` with SQL checks for contract version, role, authority, state, positive revisions, depth 0..=4, and root/parent shape. Add indexes for root/parent order, conversation order, assignee/state order, and evidence task order. Add `task_id` to `conversation_turns`. Rebuild `messages` and its dependent delivery/attempt/dead-letter/command tables so non-null task IDs reference `tasks(id)`. Perform the conservative version-5 backfill exactly as specified in the design.

- [ ] **Step 5: Implement root task persistence and reads**

Add these store interfaces:

```rust
pub struct RecordTurnInput {
    // retain current fields
    pub task_id: Option<Uuid>,
    pub authority_mode: AuthorityMode,
}

pub fn list_tasks(&self, input: &ListTasksInput) -> Result<Vec<TaskSummary>, StoreError>;
pub fn get_task(&self, task_id: Uuid) -> Result<TaskDetail, StoreError>;
```

Extend `TurnMutationResult` with `task: Option<TaskRecord>`. In the same immediate transaction as a new user turn, insert the root task using `turn_id`, insert one `accepted` evidence row, and point both turn and message to it. Existing-task follow-ups validate root/conversation/project/recipient/nonterminal invariants. Idempotent replay resolves the turn first and returns the already-created task without new writes. Validate every non-null message task reference against the task's project/conversation scope and return distinct store errors for missing tasks and scope mismatch.

- [ ] **Step 6: Verify focused store tests and commit**

Run: `cargo test -p lazoo-store --test store --test task --test conversation`

Expected: new-database, all upgrade, root creation, replay, follow-up, and employee-reference cases pass.

Run: `git diff --check`

Commit: `feat: persist root task identity`

---

### Task 3: Delegation, authority, evidence, and lifecycle mutations

**Files:**
- Modify: `crates/lazoo-store/src/task.rs`
- Modify: `crates/lazoo-store/src/lib.rs`
- Modify: `crates/lazoo-store/tests/task.rs`
- Modify: `crates/lazoo-store/tests/message.rs`

**Interfaces:**
- Consumes: schema-6 task tables, employee/session/roster queries, and transactional canonical message insertion.
- Produces: idempotent task delegation and lifecycle methods for daemon RPC.

- [ ] **Step 1: Write failing delegation tests**

Cover one behavior per test:

- `ask` creates `proposed` with no message;
- user acceptance creates one canonical `delegation` message from the parent assignee to child assignee;
- non-ask delegation calculates the literal most-restrictive authority and creates accepted task/message atomically;
- replay returns the same child/message IDs;
- stale roster/configuration, disabled/unavailable assignee, self-delegation, project/conversation mismatch, depth 5, and ninth nonterminal direct child fail before writes;
- employee actor must be the parent assignee and its cited session must belong to it; and
- no child can expand parent authority.

- [ ] **Step 2: Run the delegation tests and verify RED**

Run: `cargo test -p lazoo-store --test task delegation_`

Expected: compilation fails because delegation and acceptance store methods are absent.

- [ ] **Step 3: Implement delegation and acceptance transactions**

Add exact inputs/methods:

```rust
pub struct DelegateTaskInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub parent_task_id: Uuid,
    pub actor: MessageEndpoint,
    pub assigned_employee_id: Uuid,
    pub objective: String,
    pub references: Vec<MessageReference>,
    pub requested_authority: Option<AuthorityMode>,
    pub roster_revision: u64,
}
pub struct AcceptTaskInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub task_id: Uuid,
    pub expected_revision: u64,
}

pub fn delegate_task(&self, input: &DelegateTaskInput)
    -> Result<TaskMutationResult, StoreError>;
pub fn accept_task(&self, input: &AcceptTaskInput)
    -> Result<TaskMutationResult, StoreError>;
```

Resolve command replay first. Lock the parent, actor session, current roster, and assignee configuration in one immediate transaction. Compute immutable ancestry/depth/authority, enforce fan-out, and insert evidence/audit/outbox/command rows. Factor the existing internal message insert so accepted delegation and message creation share the transaction without calling the public store method recursively.

- [ ] **Step 4: Write failing lifecycle tests**

Cover start evidence, progress, completion, failure, cancellation, active-child refusal, stale revision, wrong actor/session, terminal immutability, and local-user resolution from uncertain. For `task.start`, create a real stored turn and assert that `recorded` is rejected while `accepted` with matching task/recipient/session becomes running.

- [ ] **Step 5: Run lifecycle tests and verify RED**

Run: `cargo test -p lazoo-store --test task lifecycle_`

Expected: compilation fails because task lifecycle methods are absent.

- [ ] **Step 6: Implement lifecycle mutations**

Add:

```rust
pub fn start_task(&self, input: &StartTaskInput) -> Result<TaskMutationResult, StoreError>;
pub fn report_task(&self, input: &ReportTaskInput) -> Result<TaskMutationResult, StoreError>;
pub fn cancel_task(&self, input: &CancelTaskInput) -> Result<TaskMutationResult, StoreError>;
pub fn resolve_task(&self, input: &ResolveTaskInput) -> Result<TaskMutationResult, StoreError>;
```

Every method resolves command replay, requires the expected revision/state/actor, validates evidence, updates once, appends exactly one evidence/audit/outbox row, stores the command result, and commits. Cancellation of proposed/accepted work is immediately terminal; running work becomes `cancel-requested`. Do not recursively cancel children or infer provider cancellation.

- [ ] **Step 7: Verify the complete store suite and commit**

Run: `cargo test -p lazoo-store`

Expected: all store tests pass with no warnings and all pre-schema-6 migrations remain covered.

Commit: `feat: manage durable task delegation`

---

### Task 4: Link turn evidence and restart reconciliation to task truth

**Files:**
- Modify: `crates/lazoo-store/src/conversation.rs`
- Modify: `crates/lazoo-store/src/task.rs`
- Modify: `crates/lazoo-store/src/lib.rs`
- Modify: `crates/lazoo-store/tests/conversation.rs`
- Modify: `crates/lazoo-store/tests/task.rs`
- Modify: `apps/lazood/src/app/turn.rs`
- Modify: `apps/lazood/tests/daemon.rs`
- Modify: `crates/lazoo-providers/src/contract.rs`
- Modify: `crates/lazoo-providers/src/codex.rs`
- Modify: `crates/lazoo-providers/tests/contract.rs`
- Modify: `crates/lazoo-providers/tests/codex.rs`

**Interfaces:**
- Consumes: existing turn observe/fail/cancel/uncertain transactions and Task 3 state helpers.
- Produces: atomic turn-to-task state projection plus startup task reconciliation.

- [ ] **Step 1: Write failing turn/task projection tests**

For a root and an accepted child with linked turns, assert:

- first native acceptance/progress changes accepted to running exactly once;
- later events append progress without duplicating `started`;
- completed turn completes a task with no nonterminal descendants;
- completed turn leaves a parent running when a child is active;
- terminal failure/cancellation maps to task failure/cancellation;
- uncertain turn maps running/cancel-requested task to uncertain; and
- mismatched task/conversation/project/recipient/session links fail atomically.

Also add a Codex request-capture test in which the session default is `auto` and the task turn override is `ask`; assert the emitted `turn/start.approvalPolicy` is the ask-mode value. The production break it catches is a task running with broader session authority than the user granted.

- [ ] **Step 2: Run focused tests and verify RED**

Run: `cargo test -p lazoo-store --test conversation task_`

Expected: turns change while the linked task remains accepted.

- [ ] **Step 3: Implement atomic task projection**

Inside the existing turn transaction, call private task helpers with the same `Transaction`. Do not call public lock-taking methods. Projection writes task evidence/audit/outbox only when the durable task meaning changes; raw provider events remain in `conversation_turn_events` and are referenced rather than copied wholesale.

Extend the provider turn contract without changing its default behavior:

```rust
pub struct TurnRequest {
    pub correlation_id: Uuid,
    pub prompt: String,
    pub permission_mode: Option<String>,
}

impl TurnRequest {
    #[must_use]
    pub fn with_permission_mode(mut self, mode: impl Into<String>) -> Self {
        self.permission_mode = Some(mode.into());
        self
    }
}
```

Codex `turn_start_params` must prefer `TurnRequest.permission_mode` over `SessionRequest.permission_mode`. The daemon derives the override from the linked task's effective authority. Reject task-backed delivery with `LZ-TASK-AUTHORITY-UNSUPPORTED` if the selected adapter cannot honor the required ceiling; do not fall back to a broader session permission.

- [ ] **Step 4: Write failing restart tests**

Persist running and cancel-requested tasks linked to owned turns, reopen the store, run existing turn reconciliation followed by task reconciliation, and assert `LZ-TASK-OWNER-LOST`, no PID/native-handle adoption, accepted/proposed tasks unchanged, one summary audit/outbox event, and idempotent second reconciliation.

- [ ] **Step 5: Implement task reconciliation and daemon startup order**

Add:

```rust
pub fn reconcile_tasks(&self) -> Result<TaskReconcileResult, StoreError>;
```

Call it after `reconcile_turns` during `App` construction and publish its pending outbox evidence through the existing startup publication path.

- [ ] **Step 6: Verify store/daemon suites and commit**

Run: `cargo test -p lazoo-store -p lazoo-providers -p lazood`

Expected: all task projection, restart, existing conversation-delivery, and daemon tests pass.

Commit: `feat: reconcile task execution evidence`

---

### Task 5: Daemon RPC and CLI task workflow

**Files:**
- Modify: `apps/lazood/src/app.rs`
- Create: `apps/lazood/src/app/task.rs`
- Modify: `apps/lazood/src/app/turn.rs`
- Modify: `apps/lazood/tests/daemon.rs`
- Modify: `apps/lazooctl/src/lib.rs`
- Modify: `apps/lazooctl/tests/cli.rs`
- Modify: `crates/lazoo-protocol/src/conversation.rs`
- Modify: `crates/lazoo-protocol/tests/contracts.rs`

**Interfaces:**
- Consumes: Task 1 RPC params and Tasks 2-4 store methods.
- Produces: eight task RPCs, `turn.submit` task/authority mapping, JSON CLI output, and exit code 13 for task errors.

- [ ] **Step 1: Write failing CLI mapping tests**

Add literal request assertions for:

```text
task delegate
task accept
task start
task report
task cancel
task resolve
task list
task show
```

Assert strict camel-case JSON, automatic UUID idempotency defaults, parsed references/evidence objects, list filters, `turn submit --task-id`, `turn submit --authority-mode` defaulting to `ask`, and exit code 13 for every `LZ-TASK-*` error.

- [ ] **Step 2: Run CLI tests and verify RED**

Run: `cargo test -p lazooctl --test cli`

Expected: task commands and turn task/authority flags are absent.

- [ ] **Step 3: Implement CLI parsing and request mapping**

Add `TaskCommand` to the top-level Clap enum and one pure `task_request` mapper. Parse task state/report/resolution/authority through explicit functions that return stable user-facing errors. Preserve existing JSON output behavior.

- [ ] **Step 4: Write failing daemon RPC tests**

Exercise all eight methods through `App::handle`. Assert successful workflow, idempotent replay, filters/order, stable error codes/data, event publication, and exact desktop snapshot equality before/after task operations. Include an attempt to claim a proposed task's nonexistent delegation message and prove no provider/session method was called.

- [ ] **Step 5: Run daemon tests and verify RED**

Run: `cargo test -p lazood --test daemon task_`

Expected: requests return `LZ-METHOD-NOT-FOUND` or task contract fields are not yet mapped.

- [ ] **Step 6: Implement daemon handlers and error mapping**

Route the eight methods to store inputs, validate list limits 1..=1024 and nonzero revisions before store calls, and publish pending outbox events after every effective mutation. Map store/domain failures to the distinct `LZ-TASK-*` families listed in the design. Extend `turn.submit` with optional task ID and default-ask authority while preserving idempotent replay.

- [ ] **Step 7: Verify CLI/daemon/protocol suites and commit**

Run: `cargo test -p lazoo-protocol -p lazooctl -p lazood`

Expected: all task and existing RPC/CLI contract tests pass.

Commit: `feat: expose task graph controls`

---

### Task 6: Backend-only release acceptance and continuity closeout

**Files:**
- Create: `scripts/acceptance/task-delegation-graph.ps1`
- Create: `artifacts/acceptance/task-delegation-graph-manifest.json`
- Create: `docs/verification/2026-09-04-task-delegation-graph.md`
- Modify: `apps/lazood/tests/daemon.rs`
- Modify: `scripts/acceptance/first-slice.ps1`
- Modify: `scripts/acceptance/employee-roster.ps1`
- Modify: `scripts/acceptance/native-session-ownership.ps1`
- Modify: `scripts/acceptance/collaboration-bus.ps1`
- Modify: `scripts/acceptance/conversation-delivery.ps1`
- Modify: `docs/NEXT-SESSION.md`
- Modify: `docs/PROJECT-BRIEF.md`
- Modify: `README.md`

**Interfaces:**
- Consumes: release binaries and deterministic turn-transport fixture.
- Produces: schema-6 manifests, exact gate evidence, and the next backend handoff without product-readiness claims.

- [ ] **Step 1: Write the failing executable acceptance test**

Run the future script from the daemon test using a temporary output path, then deserialize its JSON and require literal boolean/number/string fields for every item in design section 12. The test must fail because the script does not exist; do not grep script source as proof.

- [ ] **Step 2: Run the acceptance test and verify RED**

Run: `cargo test -p lazood --test daemon task_graph_acceptance_executes_backend_only_contract`

Expected: FAIL because `scripts/acceptance/task-delegation-graph.ps1` is absent.

- [ ] **Step 3: Implement isolated PowerShell release acceptance**

Use a newly created, resolved, validated temporary data root; hidden daemon processes; release binaries; the deterministic turn-transport fixture; and JSON CLI requests. Exercise root creation/replay/follow-up, ask proposal/acceptance, authority narrowing, routing/depth/fan-out failures, native-evidence start, progress/outcomes, child blocking, owner-loss restart, uncertainty resolution, and audit/outbox replay. Cleanup may terminate only PIDs started by the script and remove only the validated temporary root.

- [ ] **Step 4: Update every pre-existing acceptance schema assertion**

Change existing scripts from schema 5 to schema 6 without altering their behavioral scope. Run their executable contract tests so a missed or stale schema expectation fails before the full matrix.

- [ ] **Step 5: Run the full fresh verification matrix**

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
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\task-delegation-graph.ps1 -SkipBuild
```

Expected: every command exits 0; record the exact workspace pass/fail/ignored counts and exact PASS command counts/timings from all six manifests. Do not run provider smoke or a live model turn.

- [ ] **Step 6: Verify boundaries and line endings**

Run:

```powershell
git diff --name-only main...HEAD
git diff --check main...HEAD
git status --short
git diff --quiet main...HEAD -- apps/lazoo-desktop
git diff --quiet -- apps/lazoo-desktop
rg -n "provider smoke|apps[\\/]lazoo-desktop|ConPTY|git worktree" scripts\acceptance\task-delegation-graph.ps1
```

Expected: both desktop diff commands exit 0; no whitespace errors; no task acceptance code invokes provider smoke, desktop paths, ConPTY, or Git worktree mutation. Interpret `rg` matches manually so comments/assertions cannot be mistaken for forbidden execution.

- [ ] **Step 7: Record verification and next handoff**

Write exact commands, exits, test counts, manifest evidence, TDD red/green evidence, exclusions, and any manual/blocked gates in `docs/verification/2026-09-04-task-delegation-graph.md`. Update the brief, README, schema assertions, and `NEXT-SESSION.md` to describe only implemented behavior. The next handoff must identify automatic accepted-task dispatch plus project/worktree isolation as unimplemented and must not emit `UI BACKEND CONTRACT: GO` until the stable desktop-facing contract genuinely exists.

- [ ] **Step 8: Commit closeout**

Run: `git diff --check`

Commit: `test: verify durable task delegation`

---

## Plan self-review

- Spec coverage: identity, user-turn mapping, immutable input, lifecycle, ancestry, roles, depth/fan-out, authority, actor limits, evidence, owner loss, stable errors, CLI-only controls, migration, acceptance, and exclusions each map to a task above.
- Placeholder scan: no `TBD`, deferred implementation instruction, unspecified error-handling step, or cross-task “same as above” step remains.
- Type consistency: `TaskRecord`, `TaskEvidence`, `TaskMutationResult`, state/report/resolution enums, RPC names, store method names, authority ordering, and task/turn/message IDs are used consistently across all six tasks.
- Boundary check: no plan task modifies desktop files or authorizes live provider/model use, provider tool injection, worker dispatch, worktrees, ConPTY, generic approvals, memory, incidents, or resource scheduling.
