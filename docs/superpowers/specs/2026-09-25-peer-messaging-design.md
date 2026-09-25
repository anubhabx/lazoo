# Peer Messaging Tools Design

**Date:** 2026-09-25

**Status:** Implemented on `feat/peer-messaging` (based on `feat/result-delivery`) with deterministic acceptance, for owner review before merge.

**Scope:** Backend-only. No schema change: peer messages are ordinary `peer` and `reply` rows in the schema-4 message ledger, queued under one new `message_commands` scope, and delivered through the notice path the result-delivery slice built. Daemon-managed Codex sessions gain two tools, `message_send` and `message_reply`, and `task_wait` hands over peer messages that arrive while the caller waits. No desktop or Tauri change, no CLI change, no automatic activation, no cross-tree messaging, no live model use.

## 1. Outcome

Before this slice an employee could only talk to its lead through the task it was given: it could delegate, wait, report, and raise an incident, but it could not ask a question and it could not answer one. The brief's peer clarification flow (a worker asks the lead or a sibling for the minimum missing fact and continues) had no path.

Lazoo now answers four questions without inference:

1. Who asked whom what, from which task to which task, and in which thread? Every peer message is one durable message row linking the sender's task and the recipient's task; a reply carries the original's correlation and causation.
2. How did the recipient read it: as a turn on its idle session, inside its own `task_wait`, or inside its own `message_send` wait? The acknowledgement evidence names the tool and the turn.
3. Why has a message not been read yet: the recipient is busy on another turn, dormant, or its task has not started.
4. Did answering a question end anybody's task? No. A peer turn only progresses the recipient's task; the result notices that follow, or the employee's own turn, complete it.

## 2. Tools

| Tool | Purpose | Guards | Result |
|---|---|---|---|
| `message_send` | Send `text` to `employeeId`; optionally block up to `waitForReplySeconds` (max 600) for the reply | sender is the turn's own endpoint on its own task; recipient is another employee with an `accepted` or `running` task in the caller's root task tree; content is 1 to 16 KiB | `{messageId, state, recipientEmployeeId, recipientTaskId, timedOut, reply, messages}` where `messages` is everything read during the wait and `reply` is the one threaded under this message |
| `message_reply` | Answer message `messageId` with `text` | the original is a peer message addressed to the caller's employee and task; the original sender's task is still `accepted` or `running` | `{messageId, state, recipientEmployeeId, recipientTaskId, replyToMessageId}` |
| `task_wait` (extended) | unchanged, and returns early when a peer message arrives for the caller | unchanged | adds `messages`; `timedOut` is false when the wait ended for a message |

Refusals answer the model without ending the turn: `LZ-TOOL-INVALID` for a message to oneself, empty or oversized text, or malformed arguments; `LZ-TOOL-SCOPE` for an employee without an active task in the caller's tree, an unknown message, or a reply to a message not addressed to the caller. A recipient's task that ends before it reads a message closes the message as `cancelled`, and a sender blocked in `message_send` returns at once with that state and no reply.

## 3. Delivery

```text
message_send / message_reply ──▶ store send_peer_message / reply_peer_message
                                   │ one message row, scope message.peer, kind peer|reply,
                                   │ task_id = recipient's task, body.text = rendered turn text
                                   ▼
        ┌─── recipient inside task_wait or message_send ──▶ consumed there (claim + acknowledge
        │                                                    with {via, turnId} as evidence)
        └─── recipient idle, task running, no unfinished turn ──▶ notice loop records the
                                                                   peer turn and delivers it
```

The notice loop is unchanged: it lists queued messages under the three auto-delivery scopes oldest first and delivers each as one provider turn reusing the message as the turn's canonical message. Two rules are specific to peer messages:

| Rule | Reason |
|---|---|
| A peer turn is recorded only while the recipient's task is `running` | Delivering to an `accepted` task would start that task with a peer message instead of its objective, and the dispatcher would never run. The message waits until the task starts. |
| A completed peer turn projects as progress, never as completion | With no unfinished child left, a completed turn otherwise completes the task; a peer message delivered just before the child's result would then cancel that result. Result notices and the employee's own turns keep completing tasks. |

A message read inside a turn closes any notice turn recorded earlier for the same message (the recipient was dormant at the time) so that stale turn can neither deliver the message twice nor block later notices. This also closes the same gap for result notices consumed through `task_wait` after the user resumes a dormant lead.

## 4. Activation policy

Unchanged from the result-delivery decision: employee-authored traffic never starts a provider process. A peer message to a dormant employee stays `queued` with its turn recorded until the local user resumes that employee with `session.warm` or a user turn; the next pass delivers it. `task_wait` returning early for a peer message is how a sender reaches a lead that is already busy waiting, without activating anyone.

## 5. Durable records

No new tables. A peer message is one `messages` row under the `message.peer` command scope with sender endpoint (employee and session), recipient employee, project, conversation, the recipient's task as `task_id`, `causation_id` pointing at the message it answers, two `task` references (sender's and recipient's task) plus any the model supplied, and a body holding the raw `content`, both task ids, the sender's name, `replyToMessageId`, and the rendered turn `text`. A peer turn is an ordinary `conversation_turns` row under `turn.record` with key `notice-turn:<messageId>`. In-turn reads are `message.claim` and `message.acknowledge` commands keyed by message and turn with evidence `{via: "task_wait" | "message_send", turnId}`.

## 6. Fixed boundaries

- A peer message is not an approval and changes nothing about task ownership, authority, approvals, or permissions; governed actions inside a peer turn still flow through the schema-8 approval gate.
- The daemon fixes the sender to the turn's recipient endpoint; there is no way to name another sender.
- Messaging stays inside one task tree: an employee without an `accepted` or `running` task under the caller's root task cannot be addressed, so no dormant employee is ever activated and no unrelated tree is touched.
- Content is refused, never truncated, above 16 KiB; the rendered turn text carries the content plus a fixed envelope naming the sender, both tasks, the message id to cite, and the non-approval boundary.
- `desktop.snapshot`, Tauri commands and capabilities, `apps/lazoo-desktop`, the CLI, and schema 9 are byte-for-byte unchanged.

## 7. Deterministic acceptance

`scripts/acceptance/peer-messaging.ps1` runs domain, store, and daemon fake-backend proofs and confirms the desktop tree is unchanged. It must show: bounded truthful peer text; a message links both tasks, queues once, and waits for an accepted recipient; a reply threads under the question and returns to the sender's task; in-turn consumption carries evidence and closes a stale notice turn; a peer turn never completes the recipient's task; an idle lead receives a worker's question as a turn and the waiting worker receives the reply inline; `task_wait` returns early with a question so the lead answers inside its own turn; a dormant recipient is never activated, the user's resume delivers the waiting peer turn, and refusals never end the turn; schema stays 9.

## 8. Explicitly deferred work

- per-employee activation policy with bounded automatic activation of a dormant recipient (needs a migration);
- messaging an employee outside the caller's task tree, and user-to-employee messages through the same tools;
- coalescing several queued notices and peer messages into one turn;
- a task-change or message-arrival notification replacing the 250 ms polls;
- bounded lead remediation, user notification channels, typed desktop Inbox binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance.

## 9. Review gate

The owner should review this design together with the branch. Merge authorizes only the backend peer-message tools, their delivery through the existing notice path, and the deterministic acceptance; it does not authorize automatic activation, desktop work, live provider consumption, or any deferred capability above.
