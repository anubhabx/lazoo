# Backend integration guide for the first UI

Work in progress: the 2026-10-28 readiness plan and final verification record
determine which additions below have passed. The frontend is not implemented by
this work. Full core v1 acceptance remains broader than UI preparation.

## Sources of truth

Use the typed structures in `crates/lazoo-protocol` and the daemon's RPCs. The
existing desktop snapshot is a shell/health read, not a replacement for the
workspace, Inbox, roster, memory, configuration, and terminal APIs. Keep the Rust
client and daemon from the same build while these pre-release contracts evolve.
Expose explicit Tauri commands to the frontend; do not expose arbitrary RPC or
process execution. No frontend binding is added here.

| UI surface | Backend reads |
| --- | --- |
| Health and registered project rail | `desktop.snapshot`, `system.status`, `project.list` |
| Home/project launch | `workspace.launcher` |
| Conversation sidebar | `workspace.conversations` |
| Active work, lead and workers | `workspace.conversation`, `turn.show` |
| Global attention queue | `inbox.list`, `inbox.counts` |
| Employee settings | `employee.list`, `employee.show`, `roster.get`, `provider.list` |
| Configuration discovery and selected migration | `provider.config`, `provider.config.preview`, `provider.config.select.preview`, `provider.config.operations.show` |
| Shared memory | `memory.policy.show`, `memory.search`, `memory.queue.status`, `memory.worker.show` |
| Resource display | `system.resources`, `session.resources` |
| Owner terminal | Terminal lifecycle RPCs and the separate authenticated byte channel |

The exact supported method names are dispatched in `apps/lazood/src/app.rs`;
avoid deriving additional methods from this table. The CLI provides executable
examples through `--help` and `--json`.

## Launch and capability handling

The top-level launcher `authorityModes` lists the vocabulary. The selected lead's
`authorityModes` is its usable subset. Require `startable`, show `blockingReason`
when false, and never offer a mode absent from that subset. `defaultAuthority`
must not be treated as an independent grant. An employee's configured authority
is a ceiling; choosing another value in the UI cannot widen it.

`collaborationToolsAvailable` reports whether Lazoo injects the roster, delegation,
wait, reporting, and peer-message tools. It is currently true only for Codex.
A startable conversational provider with that field false cannot be presented
as providing automatic team orchestration.

Current daemon policy, independently of whether a provider is installed or logged in:

| Provider | Conversation authority offered by Lazoo | Collaboration tools |
| --- | --- | --- |
| Codex | Modes up to the employee's configured ceiling | Yes |
| OpenCode | Ask/Self Decision up to a governed employee's ceiling; only Full Autonomy for a Full Autonomy employee | No |
| Claude Code | Full Autonomy only, and only for a Full Autonomy employee | No |
| Antigravity | No governed conversation delivery | No |
| Grok Build | Excluded from core v1 and refused for governed delivery | No |

These are Lazoo implementation limits, not claims about every feature of the
vendor products. OpenCode governed starts also inspect configuration for native
permission overrides and may refuse at launch. A launcher read is advisory;
the mutation always rechecks its actual operating conditions.

The existing launch contract is two operations: `conversation.create`, then
`turn.submit` with `activateIfNeeded: true`. It is not an atomic transaction across
the provider. Persist a distinct idempotency key per logical operation until its
outcome is known. Reuse that same key and same payload to reconcile a lost reply;
do not generate another key and accidentally create another request. A created
conversation may remain empty if its first submission cannot be recorded.

## Execution, errors, and recovery

`Recorded` and a queued message mean durable intent only. Show execution only when
native acceptance/progress proves it. An approval means Lazoo granted that one
action, not that the provider executed it. Never automatically resend an
`Uncertain` turn or task.

After a refusal with `data.turnId`, read `turn.show` and refresh the workspace.
Preserve the returned failure code/message for a useful explanation. A transient
session readiness failure differs from a permanent provider/authority refusal;
do not map every error to either “running” or “try again”.

A direct user root refused with `LZ-TASK-AUTHORITY-UNSUPPORTED` is settled as
`terminal-failed` together with its canonical message and failed root task.
Its failed task evidence uses the user actor, no session ID, and
`detail.source: "daemon-pre-provider-rejection"`. No native acceptance is implied.
An identical submission key returns the existing failed record instead of
creating another attempt. A new request after changing configuration is a new
logical operation and needs its own key. Temporary dormant/activation/readiness
failures keep their previous explicit retry or cancellation behavior.

Send the server-provided Inbox/header/worker action with its revision, adding
only its required inputs and a fresh logical idempotency key. On a stale revision,
re-read before offering the action again. Do not change authority, resolve
uncertain work, or send a cancellation merely to clear a UI error state.

## Events and reconnect

Every workspace/Inbox model carries `eventHighWater`. Retain it and subscribe
with `events.subscribe { after: eventHighWater }`; the server replays durable
events and then streams. The mark is a lower bound across the response's reads,
so events after it may already exist when the read returns. Reprocessing an event
and refreshing is safe; advancing past an unseen event is not.

Replay uses bounded pages until the captured durable high-water is reached. Live
notifications, including broadcast lag, wake another durable drain; they are not
sent directly past unseen audit rows. Clients retain the last safely consumed
cursor if a slow connection is closed by the existing write deadline.

Debounce relevant-event refreshes to at most one read per 250 ms. See the
[contract index](superpowers/specs/2026-10-13-desktop-backend-contract-index.md)
for the event families per surface. On disconnect retain last-known information,
show connection uncertainty, and reconnect from the last safely consumed cursor.
Do not change durable lifecycle state because the desktop lost its connection.

## Older conversation history

`workspace.conversation` accepts optional `beforeTurnSequence`. Omit it, or send
null, for the latest page. For an older page, pass the preceding response's
`nextBeforeTurnSequence` and the same conversation ID. The cursor is exclusive;
each page is ordered oldest to newest. `turnLimit` is 1 through 500 (default 50),
and a non-null cursor must be positive and fit a signed 64-bit sequence. Stop when
`nextBeforeTurnSequence` is null. `turnsTruncated` describes older rows beyond the
requested page, not whether newer rows exist above it.

Deduplicate/merge by turn ID when refreshing. A new turn arriving after the first
page cannot move the exclusive boundary of an older page. Only the returned turn
history is paged: the lead header, root task, workers and attention still describe
current state. Fetch an individual turn's text/events with `turn.show`; summary
rows are not complete transcripts. CLI example:

```powershell
lazooctl --json workspace show <conversation-id> --turn-limit 50 --before-turn-sequence <cursor>
```

## Native terminal boundary

Follow the [terminal client contract](superpowers/specs/2026-10-28-terminal-client-contract.md)
for authenticated framing, byte offsets, output gaps, boot/revision fencing, and
input acknowledgement uncertainty. An actual native TUI is available through
attachment to a suspended Codex or Claude session. A live protocol-driven session
cannot currently be mirrored into that TUI. The existing product brief requires
native terminals. On 2026-10-28 the owner explicitly required the actual native
TUI to remain visible during orchestrated work. Concurrent native integration is
therefore a readiness blocker; suspend-then-attach does not satisfy the first UI.

Given `lead.session.id` or a worker card's session ID, call
`session.terminal.status { id }` (CLI: `session terminal-status <id>`). Its fields
are `bootId`, `sessionId`, `sessionRevision`, `opening`, nullable `terminal`,
`attachable`, nullable `blockingCode`, and nullable `blockingReason`. A live
attachment returns the terminal record needed for reconnect and byte I/O; an
opening attachment must be allowed to settle before sending another request.
Known incompatibility is data, while missing sessions and storage faults remain
RPC errors. An exited terminal may retain metadata until the bounded cache evicts
it; its screen bytes are not retained by that metadata.

Use `sessionRevision` for a new explicit attach request and the terminal's own
revision/boot ID for resize/stop. Eligibility is advisory and is checked again
when attaching. Status reads do not reserve slots or activate employees. Terminal
exit is observed through bounded status/output polling, so session audit events
alone are not an exit notification stream.

## Remaining full-product work

Non-Codex collaboration tools, broader native authority coverage, concurrent native
TUI integration, additional configuration federation formats/plugins/skills, and
the remaining whole-product performance windows are not completed by this guide.
Connectors, vectors, and voice remain later phases. The summary worker remains
opt-in and unconfigured by default. Historical live-provider results must be
dated separately from fresh deterministic verification.
