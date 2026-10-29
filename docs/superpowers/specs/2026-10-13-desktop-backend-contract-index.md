# Desktop Backend Contract Index

**Date:** 2026-10-13

**Backend update, 2026-10-28:** The launcher now exposes per-lead `authorityModes`,
`authorityReason`, and `collaborationToolsAvailable`. Its top-level `authorityModes`
is the vocabulary, not a promise that every lead supports every mode. Select only
from the chosen lead's modes, and require `startable`. Modes never exceed the
employee's configured authority. Only Codex currently receives Lazoo collaboration
tools; other startable providers cannot yet orchestrate workers. The launcher
captures its event high-water before combining reads, so later events can be
replayed even if a write races the read. Terminal streaming is implemented; see
the [native terminal client contract](2026-10-28-terminal-client-contract.md).
The original review status below is historical and does not grant frontend approval.

**Status:** Index for review. It gathers, on one page, every daemon surface the separately owned desktop is expected to bind, as implemented in pull requests #11 to #29 at schema 15, with the defaults the owner and the UI owner still have to ratify. It adds nothing: each row points at the design document that proposed it and the verification record that proved it. `UI BACKEND CONTRACT: GO` is not emitted by this document; it waits for that review.

## 1. How the desktop reads

Every read model below is derived on each call from the durable ledgers and stored nowhere. Each result carries `eventHighWater`, a conservative lower bound on the audit-event sequence its reads reflect. The desktop keeps that mark, calls `events.subscribe { after: mark }` (the framed subscription in `lazood`'s server, which replays `events.list` from `after` and then streams), and re-reads a model, debounced to one read per 250 ms, when an event of the kinds listed for it arrives. Events may already exist after the returned mark if a write raced the reads; replay them and refresh rather than assuming the response is an atomic snapshot. Replaying an already reflected event is safe. Reconnect behavior is covered by `the_desktop_resumes_every_read_model_from_its_event_high_water_without_loss` (pull request #25). On disconnect the desktop keeps the last models, shows its own `Unknown`, and resumes from its mark.

## 2. Read models

| Method and CLI | Parameters | Result | Re-read on | Errors | Design, proof |
|---|---|---|---|---|---|
| `inbox.list` (`lazooctl inbox list`) | `filter: all \| approvals \| permissions \| incidents \| resolved`, `limit 1..500` (200), `before` (resolved only) | `items: InboxItem[]`, `counts`, `truncated`, `nextBefore`, `eventHighWater` | `approval.*`, `task.*`, `incident.*` | `LZ-INBOX-INVALID` | `2026-10-11-desktop-inbox-contract-design.md`; `verification/2026-10-13-desktop-inbox.md` (#11) |
| `inbox.counts` (`lazooctl inbox counts`) | none | `counts: { unresolved, approvals, permissions, incidents, watching }`, `eventHighWater` | same | `LZ-INBOX-INVALID` | same |
| `workspace.conversations` (`lazooctl workspace conversations`) | `projectId \| null` (Home), `limit 1..500` (200), `before` | `conversations: ConversationRow[]` (title, marker, worker counts, `lastResultAt`), `rememberedLeadId`, `truncated`, `nextBefore`, `eventHighWater` | `conversation.*`, `turn.*`, `task.*`, `approval.*`, `incident.*` | `LZ-WORKSPACE-INVALID`, `LZ-WORKSPACE-PROJECT-NOT-FOUND` | `2026-10-13-desktop-workspace-contract-design.md` §3.1, §4.1, §4.2; `verification/2026-10-13-workspace-read-models.md` (#18) |
| `workspace.launcher` (`lazooctl workspace launcher`) | `projectId \| null` | `project`, `rosterRevision`, `leads: LeadCandidate[]` (`startable`, `blockingReason`), `rememberedLeadId`, `rememberedLeadBlockingReason`, `authorityModes`, `start` (two requests as data), `eventHighWater` | `employee.*`, `roster.*`, `session.*`, `conversation.created` | same | same, §3.2, §4.3 |
| `workspace.conversation` (`lazooctl workspace show`) | `id`, `turnLimit 1..500` (50), optional exclusive `beforeTurnSequence` | `conversation`, `title`, `scope`, `lead: LeadHeader` (lifecycle, session, authority, `actions`), `rootTask`, `turns` (requested page, oldest first) with `turnsTruncated` and `nextBeforeTurnSequence`, `workers: WorkerCard[]` (eight states, `actions`), `attention: InboxItem[]`, `eventHighWater` | the sidebar's set plus `session.*`, `employee.*`, `roster.*`, `message.*` | `LZ-WORKSPACE-INVALID`, `LZ-CONVERSATION-NOT-FOUND` | same, §3.3, §4.4, §4.5, §6; `verification/2026-10-13-workspace-conversation.md` (#19), `-workspace-actions.md` (#20), `-workspace-action-guards.md` (#22) |

Supporting reads the desktop may show beside them, unchanged by this index: `system.resources` and `session.resources` (#12), `provider.config` (#16), `provider.config.preview` (#24), `desktop.snapshot` (unchanged since the first slice).

## 3. Actions as data

An `InboxAction` is `{ action, label, method, params }` naming an existing revision-checked RPC with the `expectedRevision` the user saw. The desktop adds a fresh `idempotencyKey` (and the new `title` for `rename`) and sends exactly that request; naming an RPC grants nothing, and every named RPC keeps its own guards. Where they appear:

| Surface | Actions offered now |
|---|---|
| Inbox item (`approval`) | `approve`, `decline` → `approval.decide` |
| Inbox item (`task-proposal`) | `accept` → `task.accept`; `cancel` → `task.cancel` as the user |
| Inbox item (`task-uncertain`) | `resolve-completed`, `resolve-failed`, `resolve-cancelled` → `task.resolve`; only `resolve-failed` while a descendant is unfinished (#23) |
| Inbox item (`incident`) | `acknowledge`, `resolve` → `incident.*` |
| Conversation header | `rename` → `conversation.rename` always; `cancel-turn` → `turn.cancel` while the lead's newest turn is unfinished or retryable; `retry-turn` → `turn.retry` while it is retryable and the lead's session is idle and owned by this daemon; `stop-session` → `session.stop` while the session is idle with no unfinished turn |
| Worker card | the proposal's `accept` and `cancel`; the uncertain task's resolutions; the pending approval's `approve` and `decline` first; `cancel` for a waiting or running task; none once terminal or cancelling; neither `cancel` nor a completed or cancelled resolution while a descendant is unfinished |
| Launcher | `start`: `conversation.create` then `turn.submit { activateIfNeeded: true }`, the only path that starts a provider process from the desktop |

Acting on an action is a desktop capability decision; the daemon emits data only.

## 4. Defaults awaiting ratification

Each was taken as proposed on the owner's direction to finish as much as possible; each is a rule change in `lazoo_domain::inbox` or `lazoo_domain::workspace`, not a schema change.

| Default | Where |
|---|---|
| Inbox: the badge counts only `action-required`; a lead's window is `watching`; acknowledged escalations are visible but uncounted; the archive is paged newest first | Inbox design §9 |
| Titles derive from the first user request (80 bytes) until renamed; "completed unread" is the desktop's own mark against `lastResultAt` | Workspace design §9 Q1, Q2 |
| An unresolved incident escalated to the lead or the user makes its worker `blocked` | Q3 |
| A `failed` lead availability blocks a start | Q4 |
| `attention` embeds the conversation's own unresolved Inbox items | Q5 |
| The remembered lead is the space's newest conversation's lead | Q6 |
| The launcher offers the employee's configured model, effort, and checkout policy as read-only facts | Q7 |
| Lifecycle `unknown` (an uncertain newest lead turn) takes precedence over `dormant`; the header reports the lead's private per-session working directory, with the project scope separate | Workspace design status line |

## 5. Reserved and excluded

`kind: permission` and the Permissions filter stay empty until terminal permission passthrough exists. Terminal streaming (ConPTY) uses the separate authenticated byte channel described in the [native terminal client contract](2026-10-28-terminal-client-contract.md). `session.terminal.attach` requires a suspended session. Codex also supports concurrent read-only native display through `session.terminal.view`; prompts and approvals remain in Lazoo controls. See the [UI handoff](../../UI-BACKEND-HANDOFF.md) for view capability, status, and reconnect behavior. The daemon emits no `inbox.*` or `workspace.*` event and stores no read state, dismissal, or selection.

## 6. What the review decides

Whether the implemented shapes above are the contract the desktop binds; which defaults in section 4 stand; and only then `UI BACKEND CONTRACT: GO`. The desktop's own work follows: `load_inbox`, `load_workspace_conversations`, `load_workspace_launcher`, and `load_workspace_conversation` Tauri commands over the four methods, the debounced re-reads of section 1, and a capability review before any action is sent.
