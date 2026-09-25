# Coalesced Notice Turns Design

**Date:** 2026-09-25

**Status:** Implemented on `feat/coalesced-notices` (based on `feat/peer-messaging`) with deterministic acceptance, for owner review before merge.

**Scope:** Backend-only. No schema change: a notice turn is still one `conversation_turns` row whose `message_id` is a notice, but it now carries every other notice queued for the same recipient and task as companions linked through their own `turn.record` command rows. The daemon loop, the tool surface, and the activation policy are unchanged. No desktop or Tauri change, no CLI change, no automatic activation, no live model use.

## 1. Outcome

Before this slice a lead that had been busy or dormant while several children finished, incidents were promoted, or peers asked questions received one provider turn per notice, one after the other. Each turn cost a model round trip, the lead had to reassemble the picture across turns, and a peer turn delivered just before a result had to be projected as progress so the result turn could still complete the task.

Lazoo now delivers every notice queued for one recipient and task at the moment a turn is recorded as one turn, oldest first, and still answers three questions without inference:

1. Which notices did a turn carry? The turn's canonical message is the oldest notice; every companion has its own `turn.record` command naming the turn, and its own acknowledgement citing the turn and the canonical message.
2. What happened to a notice whose turn never ran? A notice read inside another turn (the user resumed a dormant lead and it called `task_wait`) closes the recorded turn; the notices it still carried stay queued and are recorded again on a fresh turn once the recipient is free.
3. Did a coalesced turn end the task? A turn carrying only peer messages still never completes the recipient's task. A turn carrying a result or incident notice projects like any other turn and completes the task once no child is left, which is exactly the outcome the result alone would have produced.

A single queued notice is delivered with the same turn text as before, so every earlier acceptance still holds.

## 2. Turn text

`render_coalesced_notice_text` passes one notice through unchanged. Several are joined oldest first under a fixed header:

```text
[Lazoo notices] 3 Lazoo notices are waiting since your last turn, oldest first. Each one is separate evidence; act on all of them.

[Lazoo task result] ...

[Lazoo incident] ...

[Lazoo peer message] ...
```

Each notice keeps its own envelope, boundary sentence, and ids to cite, so a lead reads three independent facts, not one merged summary. The batch is bounded by the 256 KiB turn text limit: notices are added oldest first until the next one would not fit, and the rest wait for the next turn. A 16 KiB peer message or a result with 16 KiB of output caps a batch near sixteen notices.

## 3. Durable records

No new tables. Every notice a turn delivers has one `turn_commands` row under `turn.record` with key `notice-turn:<messageId>:<turnId>`. The turn id is part of the key so a notice whose recorded turn was closed before delivery can be recorded again under a new turn; the older `notice-turn:<messageId>` form from the result-delivery slice is still recognised by prefix. The newest such command for a message names the turn that delivers it; every such command for a turn names the notices it carries.

The `turn.recorded` event carries `notice: true` and `companions: [ids]`.

## 4. Delivery lifecycle

| Turn transition | Canonical message | Companions |
|---|---|---|
| record | stays `queued` | stay `queued`; one `turn.record` command each |
| begin submission | `message.claim` | `message.claim`, key `turn:<turnId>:<key>:<messageId>` |
| accepted (native evidence) | `message.acknowledge` with the provider payload | `message.acknowledge` with `{turnId, canonicalMessageId}`, key `coalesced:<turnId>` |
| failed | `message.fail` | `message.fail` with the same code and retryability |
| retry | `message.retry` | `message.retry` |
| cancel while recorded or retryable | `message.cancel` | `message.cancel` |
| cancel after provider I/O | `message.cancel` unless acknowledged | `message.cancel` unless acknowledged |
| owner lost on restart | reconciled with every other `delivering` message | reconciled with every other `delivering` message |

`record_notice_turn` gathers the batch: the requested notice leads, then every other queued notice for the same recipient and task follows oldest first; a peer message is skipped while the task is still `accepted`, exactly as before. Replaying with any member returns the same turn. The daemon pass iterates the queued notices oldest first as before, records or replays the turn for each, and delivers each turn once per pass; a notice already carried by a turn earlier in the same pass is skipped.

## 5. Rules kept from the earlier slices

- A notice turn is recorded only while no other turn on the recipient's task is unfinished; nothing joins a turn after it is recorded, so a recorded turn's text is immutable.
- Employee-authored traffic never starts a provider process. A coalesced turn for a dormant recipient stays `recorded` with its notices `queued` until the local user resumes the recipient.
- A notice read inside the recipient's own turn (`task_wait`, `message_send`) is acknowledged with `{via, turnId}` and closes any `recorded` turn that carried it. The other notices that turn carried remain `queued`; once the recipient's task is free again they are recorded on a fresh turn, never on the closed one.
- A turn carrying only peer messages projects as progress. A turn carrying at least one result or incident notice projects like any turn, because the result it carries is the very evidence a separate result turn would have delivered.
- Notices about a terminal task are cancelled, never delivered; the daemon re-reads each notice before acting so a notice acknowledged earlier in the pass is left alone.

## 6. Fixed boundaries

- No notice is merged, summarised, or truncated into another; each keeps its full rendered text, its own message row, and its own acknowledgement evidence.
- A coalesced turn runs under the recipient task's effective authority; governed actions inside it still flow through the schema-8 approval gate, and no notice is an approval.
- `desktop.snapshot`, Tauri commands and capabilities, `apps/lazoo-desktop`, the CLI, the tool catalog, and schema 9 are byte-for-byte unchanged.

## 7. Deterministic acceptance

`scripts/acceptance/coalesced-notices.ps1` runs domain, store, and daemon fake-backend proofs and confirms the desktop tree is unchanged. It must show: several notices render oldest first under one header and one renders unchanged; three queued notices for one lead become one recorded turn that replays from any member, keeps every notice queued until submission, acknowledges each with its own evidence, and completes the task when it carries results; a notice read inside a resumed user turn closes the batch turn, leaves the rest queued, and a fresh turn (never the closed one) delivers them; a batch stops at the turn text limit and the overflow waits; a peer-only turn never completes the task; two child results reach an idle lead as one turn with no new provider process; a single result is delivered exactly as before; a dormant lead is never activated and the user's resume delivers the peer message and the result as one turn; schema stays 9.

## 8. Explicitly deferred work

- per-employee activation policy with bounded automatic activation of a dormant recipient (schema 10);
- messaging an employee outside the caller's task tree, and user-to-employee messages through the same tools;
- a task-change or message-arrival notification replacing the 250 ms polls;
- bounded lead remediation, user notification channels, typed desktop Inbox binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance.

## 9. Review gate

The owner should review this design together with the branch. Merge authorizes only coalesced delivery of already-queued notices and peer messages through the existing notice path and its deterministic acceptance; it does not authorize automatic activation, desktop work, live provider consumption, or any deferred capability above.
