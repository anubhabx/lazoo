# Durable Task and Delegation Graph Design

**Date:** 2026-09-04

**Status:** Proposed for owner review; implementation is not authorized until this design is approved.

**Scope:** Backend-only durable task identity, parent/child delegation, bounded authority inheritance, lifecycle evidence, daemon RPC, and CLI controls. No desktop or Tauri capability change, provider tool injection, automatic worker dispatch, worktree mutation, ConPTY, generic approval engine, managed memory, resource scheduling, or additional provider transport.

## 1. Outcome

Lazoo will gain a durable task graph that answers four questions without inference:

1. Which user request created this work?
2. Which lead owns it and which worker, if any, received each child assignment?
3. What authority was explicitly available at each node of the graph?
4. Which durable evidence justifies the current task state?

The graph is control-plane truth. A task being proposed or accepted does not mean a provider ran it. A provider-backed task becomes `running` only after a linked turn has a native turn ID or durable progress evidence under the existing conversation-delivery contract.

## 2. Fixed boundaries

- Windows-first, local-first, single-user behavior remains unchanged.
- The current user is the only security principal authenticated by the named pipe. Employee actor fields are durable attribution, not a new process-credential system.
- Existing employees, sessions, conversations, turns, canonical messages, audit events, and outbox rows are reused. No parallel prompt, message, or event transport is introduced.
- `ask` remains the default authority mode.
- No employee can accept its own proposal, grant authority, widen inherited authority, change a task's project, change employee policy, or treat a peer message as approval.
- This slice does not create or mutate Git worktrees and does not give a delegated worker project filesystem access.
- Accepted child tasks are not dispatched automatically. A later dispatch/worktree slice will consume their immutable objective and canonical delegation message.
- `desktop.snapshot`, Tauri commands/capabilities, and `apps/lazoo-desktop` remain byte-for-byte outside scope.

## 3. Task identity and the single prompt path

Each new user-authored `turn.submit` creates one root task in the same transaction as the immutable turn and canonical message. The root task ID is the turn ID. The one submitted `text` value is both the turn input and the task's immutable objective; callers do not send a second task prompt.

This yields a stable relationship:

```text
user turn id == root task id
root task.conversation_id == lead conversation id
root task.creation_turn_id == user turn id
root task.root_task_id == root task.id
root task.parent_task_id == null
root task.role == lead
canonical message.task_id == root task.id
```

A lead conversation may contain more than one root task because a later user turn may start a distinct request. A user follow-up may instead pass an existing `taskId`; that turn joins the named root task and does not create another task. The referenced task must be a root in the same conversation and must still be nonterminal.

Employee-authored turns never create root tasks. They must reference an existing task. This prevents a worker or peer from manufacturing user-owned work.

## 4. Durable records

### 4.1 Task record

`TaskRecord` is contract version 1 and contains:

- monotonic task sequence and UUID;
- `rootTaskId`, optional `parentTaskId`, and depth;
- optional project ID and required lead-conversation ID;
- `creationTurnId` for roots or `delegationMessageId` for accepted children;
- immutable creator actor, assigned employee, and role (`lead` or `worker`);
- immutable objective and bounded references;
- requested and effective authority modes;
- employee configuration revision and roster revision used for assignment;
- current lifecycle state and optimistic revision;
- optional active turn/session IDs;
- timestamps plus stable failure code/message when applicable.

Root objectives use the existing turn-text limit. Child objectives use the same limit. References reuse `MessageReference` validation and remain capped at 32. Arbitrary metadata is not added to the task record.

### 4.2 Evidence log

`TaskEvidence` is append-only and ordered per task. It contains:

- sequence and UUID;
- task revision produced by the event;
- evidence kind;
- actor (`user` or an employee endpoint);
- optional message, turn, and session IDs;
- a bounded JSON object of at most 16 KiB;
- creation timestamp.

Evidence kinds are `proposed`, `accepted`, `started`, `progress`, `completed`, `failed`, `cancel-requested`, `cancelled`, and `uncertain`. Creation is represented by the first state evidence: a user-created root begins with `accepted`, an ask-gated child begins with `proposed`, and an allowed non-ask child begins with `accepted`.

Every effective task mutation writes the task state, one evidence row, one audit event, one outbox row, and one idempotency-command mapping in the same immediate SQLite transaction.

### 4.3 Read models

`TaskSummary` contains the task identity, ancestry, assignment, authority, state, revision, and timestamps. `TaskDetail` contains the full task, ordered direct-child summaries, and ordered evidence. Child results are not copied into the parent record.

## 5. Lifecycle and revision rules

The lifecycle states are:

| State | Meaning |
|---|---|
| `proposed` | A lead requested a child delegation under `ask`; no delegation message or provider work exists yet. |
| `accepted` | The user directly submitted the root task, explicitly accepted a proposal, or a bounded non-ask delegation passed the authority checks. Provider work has not yet been proven. |
| `running` | A linked turn has native acceptance or durable progress evidence. |
| `cancel-requested` | Cancellation was requested for a running provider-backed task but native terminal evidence has not arrived. |
| `completed` | A linked turn completed with no active children, or the assigned actor reported completion after every child became terminal. |
| `failed` | Durable terminal failure evidence was recorded. |
| `cancelled` | Work was cancelled before provider start or native cancellation was observed. |
| `uncertain` | Lazoo lost ownership or cannot prove whether accepted provider work continued. |

Allowed state transitions are:

```text
proposed -> accepted | cancelled
accepted -> running | cancelled | failed
running -> completed | failed | cancel-requested | uncertain
cancel-requested -> cancelled | failed | uncertain
uncertain -> completed | failed | cancelled   (local user resolution only)
```

Terminal states are `completed`, `failed`, and `cancelled`. Each state/evidence mutation increments the task revision exactly once. Idempotent replay returns the original result without a new revision, evidence row, audit event, or outbox row. Stale revisions fail before any write.

A parent is never auto-completed merely because its children finished. Explicit parent completion is required unless its own linked lead turn completed and it has no nonterminal children. Parent completion and cancellation are rejected with `LZ-TASK-CHILDREN-ACTIVE` while any direct or transitive child is `proposed`, `accepted`, `running`, `cancel-requested`, or `uncertain`. This avoids silently abandoning delegated work.

## 6. Delegation graph

### 6.1 Ancestry and bounds

The graph is a rooted tree, not an arbitrary dependency DAG. Each child has exactly one parent and one root. Cycles are impossible because ancestry is assigned only at insert time and never changes.

- Maximum child depth is 4; roots are depth 0.
- A task may have at most 8 nonterminal direct children.
- A task cannot delegate to its currently assigned employee.
- The parent and child always share the same project and lead conversation.
- Assignment is immutable. Rerouting creates a replacement child and terminalizes the old child explicitly; it never rewrites history.
- The caller supplies the roster revision used for selection. Lazoo revalidates the target employee and current roster revision before recording an accepted child. A stale/unavailable route fails without silently choosing another employee.

### 6.2 Record-first proposal and acceptance

`task.delegate` records the immutable child objective and ancestry first.

- If effective authority is `ask`, the child is `proposed`. No message, session activation, or provider call occurs.
- If effective authority is `self-decision` or `full-autonomy`, the child is `accepted` and the canonical `delegation` message is created atomically.
- `task.accept` is a local-user command. It changes one proposed child to accepted and creates the canonical delegation message in the same transaction.

The delegation message uses the parent-assigned employee as sender, the child-assigned employee as recipient, the inherited project/conversation IDs, the child task ID, and a body containing the immutable objective. The task's objective is the source; acceptance never asks the caller to submit the prompt again.

The message remains ordinary queued bus state. This slice does not claim or deliver it automatically.

Any other canonical message carrying `taskId` must reference an existing task and must use the task's project and lead-conversation scope. Missing tasks and scope mismatches fail with stable message errors before a message row is inserted. Peer messages may name a task without changing its assignment, authority, or lifecycle.

### 6.3 Authority inheritance

Authority order is `ask < self-decision < full-autonomy`. Effective child authority is the most restrictive of:

1. parent effective authority;
2. the requested child authority, defaulting to the parent value; and
3. the assigned employee's configured authority mode at the cited configuration revision.

An employee-authored command may only narrow this value. `task.accept` records explicit user acceptance but does not raise effective authority. Raising a root or child authority mode is deliberately absent; the user creates a new task if a broader grant is required.

Root `turn.submit` accepts a requested authority mode that defaults to `ask`. Root effective authority is the most restrictive of that request and the lead employee's configured authority at the recorded revision. A follow-up linked to an existing task cannot change the task's authority.

For daemon-managed Codex delivery, task authority is also a runtime ceiling. `TurnRequest` carries a per-turn permission mode derived from the task's effective authority, and the Codex `turn/start` request uses that value in preference to the broader session default. If a provider cannot apply an equally restrictive per-turn mode, Lazoo must reject task-backed delivery rather than silently run with broader permissions. This does not add delivery for any provider other than the already-supported Codex path.

Beyond that provider-native ceiling, this slice's authority model governs only task proposal versus acceptance. It does not itself grant filesystem writes, commands, connectors, network access, worktree creation, integration, or destructive actions. Those remain governed by native provider permissions and later policy/worktree contracts.

## 7. Actor boundaries

The local user may:

- submit a root task through the existing turn path;
- accept an `ask` delegation;
- request cancellation;
- resolve an uncertain task with explicit evidence;
- list and inspect all local tasks.

The employee assigned to a task may:

- propose bounded child tasks;
- append progress for its own running task;
- report completion or failure for its own task;
- request cancellation of its own task.

The parent-assigned employee is the only employee actor that may propose a child. The child cannot accept itself. Employee actions must cite an employee session that belongs to the actor; for start/progress/outcome evidence it must be the task's active session. This is durable attribution under the current user-authenticated daemon boundary, not a claim of cryptographic provider identity.

## 8. Turn and provider evidence

`ConversationTurnRecord` gains `taskId`. Root user turns set it to their own ID. A turn linked to an existing task must use the same conversation, recipient employee, project, and task assignment.

Existing turn transitions update task truth in the same transaction:

- native acceptance/progress: `accepted -> running` with `started` evidence;
- later progress: append `progress` evidence without changing state;
- completion: `running -> completed` only when the task has no nonterminal descendants; otherwise append progress and leave it running;
- terminal failure: `accepted|running|cancel-requested -> failed`;
- native cancellation: `accepted|running|cancel-requested -> cancelled`;
- owner loss or uncertain provider outcome: `running|cancel-requested -> uncertain`.

`task.start` exists for a future dispatcher and deterministic tests. It requires a linked turn already in `accepted` or `completed` with the same task, assigned recipient, and active session. Merely sending bytes, queueing a message, warming a session, or receiving transport success is insufficient.

No accepted child task is automatically converted into a turn in this slice. That boundary prevents Lazoo from running a worker in the daemon's scratch session directory while falsely implying project/worktree isolation.

## 9. Restart truth

Startup order remains:

1. reconcile native sessions;
2. reconcile message attempts;
3. reconcile turns;
4. reconcile tasks;
5. publish pending outbox events.

Proposed and accepted tasks survive unchanged because they have no claimed provider execution. Running and cancel-requested tasks are checked against their linked turn after turn reconciliation:

- a turn made uncertain produces task `uncertain` with `LZ-TASK-OWNER-LOST`;
- a terminal turn produces the matching terminal task evidence;
- no task adopts a prior PID, provider handle, native session, or native turn;
- one `tasks.reconciled` audit/outbox event summarizes every effective startup correction.

An uncertain child keeps every ancestor non-completable until the local user resolves it. Resolution is an append-only user decision and never rewrites the provider or task evidence that caused uncertainty.

## 10. RPC and CLI surface

The version-1 backend methods are:

- `task.delegate`
- `task.accept`
- `task.start`
- `task.report`
- `task.cancel`
- `task.resolve`
- `task.list`
- `task.show`

`task.report` accepts `progress`, `completed`, or `failed`; terminal failures require a stable bounded failure code and safe message. All mutating methods require an idempotency key. All state mutations require the expected task revision.

`task.list` is ordered by monotonic sequence and supports optional root, parent, conversation, project, assigned-employee, and state filters plus `afterSequence` and a 1..=1024 limit. `task.show` returns direct children and all task evidence in order.

Stable errors use `LZ-TASK-*` and CLI exit code 13. At minimum the implementation distinguishes invalid input, not found, stale revision, invalid state, inactive children, depth/fan-out limits, ancestry mismatch, actor/session mismatch, unavailable/stale roster route, authority expansion, missing acceptance evidence, and idempotency conflict. Existing message RPCs add `LZ-MESSAGE-TASK-NOT-FOUND` and `LZ-MESSAGE-TASK-SCOPE` for invalid task references while retaining CLI exit code 11.

## 11. Schema 6 and migration truth

Schema 6 adds `tasks`, `task_evidence`, and `task_commands`; adds `task_id` to conversation turns; and rebuilds the message tables so non-null `messages.task_id` is a foreign key to `tasks(id)`.

Existing user-authored turns are backfilled as root tasks with IDs equal to their turn IDs, `ask` authority, and state conservatively derived from durable turn state. Existing employee-authored turns receive no task association because older data cannot prove a valid parent assignment. Their messages receive a null task ID. No historical provider success is invented.

Backfill mapping is:

| Turn state | Task state |
|---|---|
| `recorded`, `retryable-failed` | `accepted` |
| `submitting`, `accepted` | `running` |
| `completed` | `completed` |
| `terminal-failed` | `failed` |
| `cancelled` | `cancelled` |
| `uncertain` | `uncertain` |

Migration tests cover new databases and upgrades from schemas 1 through 5, including preservation of existing messages, conversations, turns, audit sequence, and command idempotency.

## 12. Deterministic acceptance

The release acceptance script uses the existing deterministic turn-transport fixture and an isolated validated temporary data root. It must prove:

- one user turn creates exactly one root task using the same ID/text/message path;
- replay creates no duplicate task/evidence/event;
- user follow-up can join the existing root;
- `ask` delegation remains proposed with no message or activation;
- local-user acceptance creates exactly one canonical delegation message;
- bounded non-ask delegation is accepted without widening authority;
- stale roster/configuration revision, unavailable employee, self-delegation, depth 5, and ninth active child fail cleanly;
- running requires native acceptance/progress evidence;
- progress and terminal outcome evidence are ordered and revisioned;
- active descendants block parent completion/cancellation;
- daemon restart converts owned in-flight task truth to uncertain without adopting provider state;
- local-user uncertainty resolution is append-only and idempotent;
- audit/outbox events and task/message/turn correlations survive restart; and
- provider smoke, external model use, worktree mutation, ConPTY, desktop capability expansion, and UI changes are absent.

The full gate is the Rust toolchain check, workspace tests, format, Clippy with warnings denied, all-target check, locked release build, and every existing backend acceptance manifest plus the new task-graph manifest.

## 13. Explicitly deferred work

- provider-native task tools and per-session capability tokens;
- automatic accepted-task dispatch and worker conversation creation;
- Git worktree/branch creation, leases, validation, integration, or cleanup;
- ConPTY terminal transport or user takeover;
- generic policy precedence, approval records/cards, and action fingerprints;
- dependency DAGs beyond immutable parent/child ancestry;
- deadlines, priority queues, fairness, scheduling, concurrency, and resource budgets;
- incidents, managed memory, result artifacts, and integration records;
- other-provider daemon-managed turn delivery;
- desktop contract, snapshot, command, capability, or frontend changes.

## 14. Review gate

Implementation may begin only after the owner approves this written design. Approval authorizes the backend slice and its deterministic fixture; it does not authorize live provider/model consumption, desktop work, worktree automation inside Lazoo, or any deferred capability above.
