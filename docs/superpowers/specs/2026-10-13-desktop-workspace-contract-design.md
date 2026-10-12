# Desktop Workspace Contract Design

**Date:** 2026-10-13

**Status:** Proposed. Design only, for review by the owner and the separately owned desktop UI before any implementation. No code, schema, RPC, Tauri, or desktop change is part of this document.

**Scope:** The typed, daemon-derived read models the desktop needs after the Inbox (`docs/superpowers/specs/2026-10-11-desktop-inbox-contract-design.md`, implemented in pull request #11) to build the information architecture's slices 3 and 4 (section 19): Home and project navigation with conversation history, the launcher, the active conversation's header, and the worker dock. It names the rows, the derived states, the live-update path, and the actions, each mapped to an existing RPC. Terminal streaming (ConPTY) is outside it; the terminal host binds to a separate framed stream contract when that work exists.

## 1. Why a contract, and why now

The information architecture (sections 6 to 8) needs three things from the daemon that no single method gives today:

1. **Conversation history per space** (section 6.3): rows for Home (projectless) or one project, grouped by age, with a marker only when a conversation is active, needs attention, failed, or completed. `conversation.list` has no project filter and no state; the UI would have to join turns, tasks, approvals, and incidents itself.
2. **A truthful launcher** (sections 7.2 to 7.5): which employees can be the lead right now, why a lead is not startable, and the remembered lead for the space. `roster.get` has the availability truth; nothing says what blocks a start or which lead this space used last.
3. **The active conversation and its workers** (sections 8.1 and 8.2): the terminal header's identity line (employee, role, harness, model, authority, worktree scope, lifecycle), and one worker card per delegated task with one of the eight states the architecture names, derived "from durable task/session records and adapter progress evidence, not from terminal-text scraping" (section 16).

Every fact needed is durable at schema 15: conversations and turns (schema 5 to 7), tasks and dispatch (7), sessions (3), the roster (2), approvals (8), incidents (9 to 15), and the Inbox derivation over them (pull request #11). This design moves the joins and the state rules into the daemon once, as the Inbox did, so the CLI, the desktop, and tests agree.

## 2. Principles

1. **Derived, never stored.** No table, no read state, no copy of any record. A conversation's marker, a worker's state, and a lead's startability are computed from the rows on every read.
2. **Truthful states only.** A state is reported only when a durable record or native evidence supports it. `unknown` is reserved for `uncertain` records; a disconnected daemon is the UI's own `Unknown`, never the daemon's.
3. **No new mutations in phase 1.** Starting work, cancelling, accepting, and deciding are the existing revision-checked RPCs; the read models name them as data, exactly as Inbox actions do.
4. **The Inbox stays the attention surface.** A conversation "needs attention" when it has a counted Inbox item; the approval strip is the conversation's own Inbox items. One derivation, reused.
5. **Read-only first.** Phase 1 binds the sidebar and the launcher; phase 2 the conversation header and worker dock; actions follow the Inbox's capability review.

## 3. Read models

### 3.1 `workspace.conversations`

```text
params: { projectId: Uuid | null, limit?: 1..500 (default 200), before?: string }
result: { version: 1, projectId, conversations: ConversationRow[], rememberedLeadId: Uuid | null,
          eventHighWater: u64, truncated: bool, nextBefore?: string }
```

Rows for one space, newest `updatedAt` first, paged with the Inbox's opaque (`updatedAt`, id) cursor. `projectId: null` is Home. The UI groups rows into Today, Previous 7 days, and Earlier locally.

```jsonc
{
  "id": "…", "revision": 3,
  "title": "Fix the parser",                 // the stored title, or Lazoo's derivation (section 4.1)
  "titleDerived": true,
  "leadEmployeeId": "…", "leadName": "Rhea", "provider": "codex",
  "marker": "active" | "needs-attention" | "failed" | "completed" | null,
  "markerReason": "2 workers running",      // Lazoo's words; null when marker is null
  "unresolved": 2,                           // counted Inbox items scoped to this conversation
  "workers": { "total": 3, "running": 2, "waiting": 0, "failed": 1 },
  "lastTurnState": "completed", "lastResultAt": "…" | null,
  "createdAt": "…", "updatedAt": "…"
}
```

`rememberedLeadId` is the lead of the space's most recent conversation (section 7.3) or null when the space has none; the launcher (3.2) says whether that lead is startable now.

### 3.2 `workspace.launcher`

```text
params: { projectId: Uuid | null }
result: { version: 1, project: ProjectRecord | null, rosterRevision: u64,
          leads: LeadCandidate[], rememberedLeadId: Uuid | null,
          rememberedLeadBlockingReason: string | null,
          authorityModes: ["ask", "self-decision", "full-autonomy"],
          start: { method: "conversation.create", params: {…} } + { method: "turn.submit", params: {…} },
          eventHighWater: u64 }
```

```jsonc
{
  "employeeId": "…", "name": "Rhea", "provider": "codex", "role": "Lead",
  "model": "gpt-5-codex", "reasoningEffort": "medium",
  "defaultAuthority": "ask", "checkoutPolicy": "isolated",
  "availability": "dormant", "availabilityReason": null,
  "startable": true, "blockingReason": null,                 // section 4.3
  "configurationRevision": 4
}
```

`start` is data for the two existing RPCs in order: `conversation.create { projectId, leadEmployeeId, title: null }` then `turn.submit { conversationId, author: {"kind":"user"}, recipient: null, authorityMode, text, activateIfNeeded: true, maxAttempts: 3 }`. The UI fills `leadEmployeeId`, `authorityMode`, `text`, and a fresh `idempotencyKey` per request. A running start cannot be submitted twice because the idempotency key is reused until the response arrives.

### 3.3 `workspace.conversation`

```text
params: { id: Uuid, turnLimit?: 1..500 (default 50) }
result: { version: 1, conversation: ConversationRecord, title, titleDerived,
          scope: { projectId, projectName, checkoutMode } | { projectId: null },
          lead: LeadHeader, rootTask: TaskSummary | null,
          turns: ConversationTurnSummary[],                   // newest last, bounded by turnLimit
          workers: WorkerCard[],                              // oldest first
          attention: InboxItem[],                             // this conversation's unresolved Inbox items
          eventHighWater: u64 }
```

`LeadHeader` is the terminal header's identity line (section 8.1):

```jsonc
{
  "employeeId": "…", "name": "Rhea", "role": "lead", "provider": "codex", "model": "…",
  "authority": "self-decision",                              // the root task's effective authority, else the last user turn's
  "workspace": { "kind": "projectless" | "git-worktree" | "canonical", "path": "…", "branchName": "…" | null },
  "session": { "id": "…", "state": "idle", "nativeHostPid": 4242 } | null,
  "lifecycle": "dormant" | "starting" | "busy" | "idle" | "suspended" | "failed" | "unknown",
  "activeTurnId": "…" | null
}
```

`WorkerCard` is one card per task below the root (section 8.2):

```jsonc
{
  "taskId": "…", "revision": 2, "parentTaskId": "…", "depth": 1,
  "employeeId": "…", "name": "Mira", "provider": "codex", "model": "…",
  "label": "Implement the parser fix",                       // the objective, bounded to 120 bytes
  "state": "running" | "waiting" | "needs-approval" | "blocked" | "failed" | "complete" | "interrupted" | "unknown",
  "stateReason": "Waiting for Mira's session to become free",   // Lazoo's words
  "taskState": "accepted",                                   // the row's own state, unrenamed
  "session": { "id": "…", "state": "idle" } | null,
  "activeTurnId": "…" | null,
  "workspace": { "kind": "git-worktree", "path": "…", "branchName": "…", "baseCommit": "…" } | null,
  "failureCode": null, "failureMessage": null,
  "createdAt": "…", "updatedAt": "…", "terminalAt": null,
  "actions": [ InboxAction … ]                               // phase 3, section 6
}
```

Completed workers stay in the list, so the dock can keep them visible and muted until the UI dismisses them locally (section 8.2); the daemon keeps no dismissal state.

## 4. Derivation rules

### 4.1 Title

The stored `title` when set; otherwise the first user turn's text, trimmed to its first line and 80 bytes, with `titleDerived: true`; otherwise "New work". The architecture makes titles user-editable; that needs a `conversation.rename` mutation that does not exist today and is phase 3 (section 6).

### 4.2 Conversation marker

Evaluated in this order; the first rule that holds wins:

| Marker | Holds when |
|---|---|
| `needs-attention` | the conversation has at least one Inbox item with `attention: action-required` (a pending approval, a proposal, an uncertain task, or a user-escalated incident scoped to it) |
| `active` | any turn is `recorded`, `submitting`, or `accepted`, or any task in the conversation is `accepted`, `running`, or `cancel-requested` |
| `failed` | the root task is `failed`, or the latest user turn is `terminal-failed` |
| `completed` | the root task is `completed`, or every task is terminal and the latest user turn is `completed` |
| null | otherwise (a conversation with no turn yet, or one cancelled) |

`lastResultAt` is the newest `terminalAt` across the conversation's tasks and turns; the UI keeps its own "seen" timestamp per conversation to show "completed unread results" (section 6.3) without daemon read state.

### 4.3 Lead startability

From the roster entry (schema 2's `derive_availability`), in order:

| Availability | `startable` | `blockingReason` |
|---|---|---|
| `disabled` | false | "Rhea is disabled" |
| `offline` | false | "Codex is not installed or not detected" (the roster's `availabilityReason`) |
| `unverified` | false | "Codex is not authenticated or compatible" (the roster's reason) |
| `failed` | false | "Rhea's last session failed: <failureCode>"; the user may still start, so this is a warning the UI shows beside the control, not a block, when `allowFailedStart: true` is set by the UI owner's answer to question 4 |
| `starting` | false | "Rhea is starting" |
| `dormant`, `idle`, `suspended` | true | null |

`rememberedLeadBlockingReason` is that lead's `blockingReason`, or "Rhea was removed" when the employee no longer exists; the launcher then requires a new choice (section 7.3) and never substitutes.

### 4.4 Worker card state

Evaluated in this order; the first rule that holds wins:

| State | Holds when |
|---|---|
| `unknown` | the task is `uncertain` |
| `needs-approval` | a `pending` approval names the task (or a turn on it) |
| `interrupted` | the task is `cancel-requested` or `cancelled` |
| `failed` | the task is `failed` |
| `complete` | the task is `completed` |
| `blocked` | the task is `proposed` (waiting for the user), or its employee's availability is `disabled`, `offline`, `unverified`, or `failed`, or an open incident on the task is escalated to the lead or the user |
| `running` | the task is `running` and its active turn is `submitting` or `accepted` |
| `waiting` | otherwise: `accepted` with no dispatch yet (the worker is busy or dormant), or `running` between turns |

`stateReason` names the rule in Lazoo's words ("Waiting for the user to accept the task", "Blocked: Mira is offline", "Running: turn accepted by Codex", "Interrupted by the lead (cancel-task)"). The architecture's "waiting" and "blocked" are deliberately distinct: waiting resolves on its own, blocked needs someone.

### 4.5 Lead lifecycle

From the lead's current session and the root turn: no session → `dormant`; session `starting` → `starting`; session `idle` with an unfinished turn → `busy`, without → `idle`; `suspended` or `failed` as the session says; an `uncertain` latest turn → `unknown`.

## 5. Live updates

As for the Inbox: the desktop reads a model, keeps `eventHighWater`, subscribes with `events.subscribe { after }`, and re-reads the model, debounced to one read per 250 ms, on:

- `workspace.conversations`: `conversation.*`, `turn.*`, `task.*`, `approval.*`, `incident.*` (except `incident.threshold-*`);
- `workspace.launcher`: `employee.*`, `roster.*`, `session.*`, `conversation.created`;
- `workspace.conversation`: the sidebar's set plus `session.*`, `employee.*`, `roster.*`, and `message.*` for the correlation inspector.

The daemon emits no `workspace.*` event. On disconnect the UI keeps the last models, shows its own `Unknown`, and resumes from its `eventHighWater` on reconnect.

## 6. Actions (phase 3)

Each worker card and the conversation header list the actions that apply now, as `InboxAction` data (method, params with `expectedRevision`, the caller adds `idempotencyKey`):

| Where | Actions → existing RPC |
|---|---|
| worker `blocked` by `proposed` | `accept` → `task.accept`; `cancel` → `task.cancel` (`actor: {"kind":"user"}`) |
| worker `waiting`, `running`, `needs-approval` | `cancel` → `task.cancel` |
| worker `unknown` | `resolve-completed`, `resolve-failed`, `resolve-cancelled` → `task.resolve` |
| worker `needs-approval` | the approval's own `approve` and `decline` (from `attention`) |
| lead `busy` | `cancel-turn` → `turn.cancel` |
| lead latest turn `retryable-failed` | `retry-turn` → `turn.retry` |
| lead session `idle` | `stop-session` → `session.stop` |
| header | `rename` → `conversation.rename` (new mutation, phase 3; see question 1) |

No action starts a provider process on its own; the launcher's `turn.submit` with `activateIfNeeded: true` is the user's explicit start (brief section 9.2).

## 7. Deep links

A conversation row's `id` opens `workspace.conversation`; a worker card's `taskId` and `employeeId` focus the card and, once the terminal contract exists, its terminal; an `attention` item carries the Inbox `scope`. Inbox items already deep-link with `conversationId`, so both directions agree.

## 8. Daemon work this implies (after agreement)

- `lazoo-store`: one query for conversations by project with their latest turn and task aggregates; one for a conversation's tasks with dispatch, session, employee, and pending-approval joins; the roster read already exists.
- `lazoo-domain`: the marker, startability, worker-state, lifecycle, and title rules as pure functions beside `inbox`.
- `lazoo-protocol`: `ConversationRow`, `LeadCandidate`, `LeadHeader`, `WorkerCard`, the three params and results, `WORKSPACE_CONTRACT_V1`.
- `lazood`: `workspace.conversations`, `workspace.launcher`, `workspace.conversation`; `lazooctl workspace conversations|launcher|show`.
- Deterministic acceptance: every marker and worker state from its rows, the remembered lead and each blocking reason, the sidebar cursor, the header's lifecycle through warm, turn, suspend, and failure, and `eventHighWater` plus the subscription losing no change across a reconnect.
- No schema change is expected for phases 1 and 2; `conversation.rename` (phase 3) needs a `conversations.title` write path and an idempotent command, not a migration.

## 9. Questions for the owner and the UI owner

1. **Titles**: derive from the first user turn as proposed, and add `conversation.rename` in phase 3? Or store a generated title at `conversation.create` so it never changes under the user?
2. **"Completed unread"**: UI-local seen timestamps against `lastResultAt`, as proposed, or daemon read state (a table and a mutation the Inbox deliberately avoided)?
3. **`blocked`**: should an open lead-window incident (`watching` in the Inbox) make a worker `blocked`, as proposed, or only a user escalation?
4. **Failed lead**: is a `failed` availability a block or a warning the user can override?
5. **`attention` embedded**: should `workspace.conversation` embed the conversation's Inbox items, as proposed (one derivation), or only their counts with the UI reading `inbox.list`?
6. **Lead memory**: derived from the latest conversation, as proposed, or a stored per-space setting that survives the conversation being deleted (needs Settings, which has no contract yet)?
7. **Launcher options**: phase 1 offers only the employee's configured model, effort, and checkout policy as read-only facts; per-launch overrides wait for capability truth (section 7.2). Agreed?

## 10. Review gate

Agreeing to this document authorizes only an implementation plan for `workspace.conversations`, `workspace.launcher`, and `workspace.conversation` with the rules above, in the phases above. It does not authorize desktop or Tauri changes, the `conversation.rename` mutation, terminal streaming, or `UI BACKEND CONTRACT: GO`, which waits for the implemented contracts' own review.
