# Desktop Inbox Contract Design

**Date:** 2026-10-11

**Status:** Proposed. Design only, for review by the owner and the separately owned desktop UI before any implementation. No code, schema, RPC, Tauri, or desktop change is part of this document.

**Scope:** One typed, daemon-derived read model that the desktop's global Inbox (information architecture, section 9) can bind to, built only from ledgers that already exist at schema 15. It names the rows, the badge, the live-update path, and the actions, each mapped to an existing RPC method. Implementation, the Tauri commands, and the desktop work wait for the UI owner's agreement.

## 1. Why a contract, and why now

The owner-approved information architecture makes Inbox "the only permanent attention destination": filters All, Approvals, Permissions, Incidents, and Resolved, a rail badge counting "unresolved items requiring user action, not total events", and items that show type and severity, the required action, scope, conversation, employee, harness, task, age, and state, with a deep link and explicit actions.

Every fact the Inbox needs is now durable and revisioned in the daemon:

| Ledger | Since | What it contributes |
|---|---|---|
| Approvals | schema 8 | Lazoo policy approvals held for the user under `ask` (or a scope expansion under `self-decision`), bound to one action fingerprint and an expiry |
| Tasks | schema 7 | Children `proposed` under `ask` authority, waiting for `task.accept`; `uncertain` tasks waiting for `task.resolve` |
| Incidents | schemas 9, 12, 13, 15 | Human-readable failures with priority, escalation (`lead`, `remediated`, `user`), remedies, desktop notification state, and user-tuned thresholds |
| Messages | schemas 4, 14 | Dead letters, which already raise `LZ-MESSAGE-DEADLETTER` incidents (at once for a priority message) |

But the desktop can see none of it: its only command is `load_desktop_snapshot` (`desktop.snapshot`: status, projects, the last 100 events), and the CLI is the only way to act. Without a reviewed contract, the UI would have to re-derive "what needs the user" from four list RPCs and the raw event stream, and the two would drift. This design moves that derivation into the daemon, once.

## 2. Principles

1. **Derived, never stored.** The Inbox is a view over the approval, task, and incident rows. It has no table, no read state, and no copy of any record; the rows stay authoritative, so the Inbox can never disagree with `lazooctl`.
2. **The badge counts only what the user must do.** An item counts when it can only progress through a user action. Something a lead is remediating, or that the user has already acknowledged, is visible but not counted.
3. **No new mutations.** Every action is an existing, revision-checked, idempotent RPC. The Inbox only tells the UI which one applies, with which parameters.
4. **Distinct types preserved.** Items keep their own kind; failures are incidents with a subtype, so there is no fifth unresolved category (information architecture, section 9).
5. **Read-only first.** Phase 1 gives the desktop a read-only Inbox with deep links, so the UI owner can bind and lay it out without the desktop gaining mutation rights. Actions are phase 2, behind a separate desktop capability review.

## 3. Item kinds

| `kind` | Source rows | Filter | Counts toward badge when | Leaves the unresolved list when |
|---|---|---|---|---|
| `approval` | `approvals` with `state = pending` | Approvals | always (it expires otherwise) | decided, expired, or cancelled |
| `task-proposal` | `tasks` with `state = proposed` | Approvals | always | accepted or cancelled |
| `task-uncertain` | `tasks` with `state = uncertain` | Incidents (subtype `task-uncertain`) | always | resolved |
| `incident` | `incidents` with `state = open` and `escalation = user` | Incidents (subtype from `code`) | always | acknowledged or resolved |
| `incident` | `incidents` with `state = open` and `escalation = lead` | Incidents | never: `attention: watching`, with the lead's `remediationDeadlineAt` | the window closes (`remediated` leaves; `user` starts counting) |
| `incident` | `incidents` with `state = acknowledged` and `escalation = user` | Incidents | never: `attention: acknowledged` | resolved; a recurrence reopens it and it counts again |

Not Inbox items, by design:

- **Normal-priority incidents** (below their threshold) and **remediated** ones: informational, already in `lazooctl incident list`. The information architecture forbids padding the Inbox with informational events.
- **Provider-native permission prompts.** Today every Codex permission request becomes a Lazoo `approval`, so the **Permissions** filter is empty until the terminal work (ConPTY) can surface prompts that stay inside a harness's TUI. The contract reserves `kind: permission` for them.
- **Turn failures and dead letters** are reported through the incidents they already raise (`LZ-TURN-PROVIDER`, `LZ-MESSAGE-DEADLETTER`, and so on), so they appear once, as incidents.

## 4. Read model

### 4.1 `inbox.list`

```text
params: { filter: "all" | "approvals" | "permissions" | "incidents" | "resolved",
          limit?: 1..500 (default 200), before?: string }
result: { version: 1, items: InboxItem[], counts: InboxCounts,
          eventHighWater: u64, truncated: bool, nextBefore?: string }
```

- Unresolved filters return every matching item up to `limit`, ordered by urgency: items that count toward the badge first, then by deadline (`expiresAt` for approvals, `remediationDeadlineAt` for a lead window), then oldest first. `truncated` is true when more exist.
- `resolved` is the archive: items that left the unresolved list, newest `updatedAt` first, paged with the opaque `before` cursor (`updatedAt` plus row id).
- `eventHighWater` is the event sequence the result reflects, so the UI can subscribe from exactly there (section 5).

### 4.2 `InboxItem`

```jsonc
{
  "kind": "approval" | "task-proposal" | "task-uncertain" | "incident" | "permission",
  "id": "<the source row's id>",            // the approval, task, or incident id
  "revision": 3,                            // the row's revision, for expectedRevision
  "attention": "action-required" | "watching" | "acknowledged" | "resolved",
  "severity": "priority" | "normal",
  "subtype": "LZ-SESSION-STALLED",           // incidents: the code; task-uncertain: "task-uncertain"
  "title": "Session accepted work but produced no progress",
  "requiredAction": "Retry the turn explicitly or stop the session; ...",
  "state": "open",                          // the source row's own state, unrenamed
  "scope": {
    "projectId": null,                      // null is Home (projectless)
    "conversationId": "…", "employeeId": "…", "employeeName": "Mira",
    "provider": "codex", "taskId": "…", "turnId": "…", "sessionId": null
  },
  "createdAt": "…", "updatedAt": "…",
  "deadline": { "kind": "expires" | "remediation", "at": "…" } | null,
  "detail": { … },                          // kind-specific, section 4.3
  "actions": [ InboxAction … ]              // phase 2, section 6; empty in phase 1
}
```

`title` and `requiredAction` are always Lazoo's own words: the incident's `title` and `nextAction`; for an approval, "Approve or decline: <action>"; for a proposal, "Accept or cancel the task <objective>"; for an uncertain task, "Decide how the task ended". `severity` is `priority` for every counted item except a proposal, which is `normal`.

### 4.3 `detail` by kind

| `kind` | `detail` fields (all already on the source row) |
|---|---|
| `approval` | `approvalKind` (`command-execution`, `file-change`), `action`, `fingerprint`, `authority`, `scopeExpansion`, the provider's `detail` object bounded to 16 KiB |
| `task-proposal` | `objective`, `requestedAuthority`, `effectiveAuthority`, `parentTaskId`, `depth`, `creator` |
| `task-uncertain` | `objective`, `failureCode`, `failureMessage`, `activeTurnId` |
| `incident` | `code`, `explanation`, `occurrences`, `firstOccurredAt`, `lastOccurredAt`, `escalation`, `escalationReason`, `remedies` (each with kind, attempt, note, result task), `userNotification`, `source` |

Raw provider diagnostics stay behind `incident.show` and `approval.show`, as section 14.1 of the brief requires ("raw diagnostic attachment, hidden behind an explicit details action").

### 4.4 `InboxCounts`

```jsonc
{ "unresolved": 4, "approvals": 2, "permissions": 0, "incidents": 2, "watching": 1 }
```

`unresolved` is the badge: items with `attention: action-required`. `approvals` includes proposals. `watching` is shown in the Incidents filter header, never on the badge.

### 4.5 `inbox.counts`

`{}` → `InboxCounts` plus `eventHighWater`. A cheap call for the rail badge, so the shell need not list items to draw it. `desktop.snapshot` stays unchanged.

## 5. Live updates

Events already carry full records (`approval.*`, `task.*`, `incident.*` with the complete row), but the Inbox is a derivation with ordering, counting, and attention rules. Having the UI re-derive it from events would duplicate that logic. So:

1. The desktop reads `inbox.list` (or `inbox.counts`) and keeps `eventHighWater`.
2. It subscribes with `events.subscribe { after: eventHighWater }`.
3. On any event whose type starts with `approval.`, `approvals.`, `task.`, `tasks.`, or `incident.` (except `incident.threshold-*`), it re-reads `inbox.list` for the active filter, debounced to at most one read per 250 ms.
4. On disconnect it keeps the last items, shows the daemon-disconnected state the information architecture requires, and repeats from step 1 on reconnect, which is the snapshot-plus-delta recovery of brief section 8.3.

Deadlines are absolute timestamps, so the UI renders countdowns locally without polling. An approval that expires produces `approval.expired`, which triggers a re-read.

## 6. Actions (phase 2)

Each item lists the actions that apply to it now, as data:

```jsonc
{ "action": "approve", "label": "Approve once",
  "method": "approval.decide",
  "params": { "id": "…", "expectedRevision": 3, "decision": "approve" } }
```

The UI adds a fresh `idempotencyKey` and sends exactly that request. On `LZ-APPROVAL-STALE`, `LZ-TASK-STALE`, or `LZ-INCIDENT-STALE` (someone acted first) it re-reads the Inbox. No Inbox-specific mutation exists.

| `kind` | Actions → existing RPC |
|---|---|
| `approval` | `approve`, `decline` → `approval.decide` |
| `task-proposal` | `accept` → `task.accept`; `cancel` → `task.cancel` with `actor: {"kind":"user"}` |
| `task-uncertain` | `resolve-completed`, `resolve-failed`, `resolve-cancelled` → `task.resolve` |
| `incident` (user, open) | `acknowledge` → `incident.acknowledge`; `resolve` → `incident.resolve` |
| `incident` (acknowledged) | `resolve` → `incident.resolve` |
| `incident` (lead window) | none: the lead acts first; the user can still see the remedies as they land |

Retrying the failed work behind an incident (`turn.retry`, a fresh `task.delegate`) stays an explicit choice in the conversation the deep link opens. The Inbox never offers an automatic retry (brief section 14.2: "deterministic retry budgets", "never ... repeated speculative repair").

## 7. Deep links

`scope` is the deep link: `projectId` selects the space (null is Home), `conversationId` opens the conversation, and `taskId` or `employeeId` focuses the worker. Focusing "the exact terminal" (information architecture section 9) needs the ConPTY terminal contract; until then the link stops at the conversation and its task inspector.

## 8. Daemon work this implies (after agreement)

- `lazoo-store`: one query per source (pending approvals, proposed and uncertain tasks, open user and lead incidents, and acknowledged user incidents) plus the resolved archive, each bounded and indexed. Approvals have no project column filter today; the project comes from the approval row's own `project_id`.
- `lazoo-protocol`: `InboxItem`, `InboxCounts`, `InboxListParams`, `INBOX_CONTRACT_V1`.
- `lazood`: `inbox.list` and `inbox.counts`; phase 2 fills `actions`.
- Deterministic acceptance: every kind appears and leaves the list on its action, the badge counts only `action-required`, a lead window is watching then counted after escalation, a recurrence reopens an acknowledged incident, and `eventHighWater` plus the subscription loses no change across a forced reconnect.
- No schema change is expected.

The desktop side (Tauri commands such as `load_inbox`, the event subscription in the shell, the layout, and later the action capability) belongs to the UI owner and is not specified here beyond the data it receives.

## 9. Questions for the owner and the UI owner

1. Should an **acknowledged** user escalation leave the badge, as proposed, or count until resolved?
2. Is an empty **Permissions** filter acceptable until terminal permission passthrough exists, or should approvals with `approvalKind: command-execution` be presented there?
3. Should **lead-window** incidents appear at all (as `watching`), or only once they reach the user?
4. Should a **task proposal** count toward the badge, as proposed, given that under `ask` authority the tree waits for it?
5. Is **phase 1 read-only** the right first step for the desktop, with actions after a separate capability review?
6. Is a 250 ms debounced re-read acceptable, or does the UI owner want typed `inbox.changed` events instead?

## 10. Review gate

Agreeing to this document authorizes only an implementation plan for `inbox.list` and `inbox.counts` with the item kinds above. It does not authorize desktop or Tauri changes, a new mutation, or `UI BACKEND CONTRACT: GO`, which waits for the implemented contract's own review.
