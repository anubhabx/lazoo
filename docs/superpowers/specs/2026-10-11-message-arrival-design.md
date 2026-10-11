# Message Arrival Design

**Date:** 2026-10-11

**Status:** Implemented on `feat/message-arrival-notify` (stacked on `feat/user-notifications`, pull request #5) with deterministic acceptance, for owner review in a pull request before merge.

**Scope:** Backend-only, no schema change. The two tool waits that block a model turn (`task_wait` and `message_send` with `waitForReplySeconds`) wake when the daemon writes to the store instead of re-reading it every 250 ms. No tool, CLI, RPC, protocol, desktop, or Tauri change; no live model use.

## 1. Outcome

`task_wait` and a `message_send` that waits for its reply each re-read the store every 250 ms until their condition held or their deadline passed. A lead waiting on several children for minutes did hundreds of reads for nothing, and every answer reached the model up to 250 ms after it was written.

Both waits now park until the store changes or their deadline passes. A child's completion, a peer question, a reply, a closed message, or a cancelled turn wakes the wait as soon as the write is published, and a quiet wait does no reads at all.

## 2. Rule

- Every daemon write to the store is followed by `publish_events`, which already moves the transactional outbox onto the event bus. It now also wakes `App::store_changed`, one `tokio::sync::Notify`, with `notify_waiters`.
- Each iteration of a wait registers `store_changed.notified()` **before** it reads the store, so a write that lands between the reads and the park still wakes it (a `Notified` future receives `notify_waiters` from the moment it is created).
- The wait then parks on whichever comes first, the store change or its unchanged deadline, and re-checks exactly what it checked before: task states, the caller's peer inbox, the sent message's state, and whether the calling turn was cancelled or ended.
- The project-registration path, the only write that published the outbox by hand, now goes through `publish_events` like every other write.

Spurious wakeups are harmless: an unrelated write only costs one re-check. The deadlines (default 60 s, at most 600 s for `task_wait`; `waitForReplySeconds` for `message_send`) and every result field are unchanged.

## 3. Rules kept

- The store stays authoritative; the notify carries no data and nothing is delivered through it.
- Results consumed inside a wait keep their acknowledgement evidence (`via: task_wait` or `via: message_send`, with the waiting turn id).
- The approval wait and the task dispatcher keep their own notifies; neither changes.

## 4. Deterministic acceptance

`tool_waits_wake_on_the_store_write_instead_of_polling` runs the peer-question story (a lead waits on a delegated child, the worker asks the lead a question and waits for the reply, the lead answers and waits again) on Tokio's paused clock, where virtual time advances only when every task is parked on a timer. The whole lead turn takes less than 250 ms of virtual time with every wait ending early, so no wait slept a poll interval and none ran to its 10 second deadline.

`scripts/acceptance/message-arrival.ps1` runs that test and the existing wait stories, checks that the poll interval is gone from the tool source, and writes `artifacts/acceptance/message-arrival-manifest.json`. No provider smoke or live model turn.

## 5. Explicitly deferred work

- a deadline-driven timed sweep in place of the fixed 15 second cadence;
- typed desktop Inbox binding, the live Codex gate, and everything else NEXT-SESSION lists.

## 6. Review gate

Merge authorizes only the store-change wakeup for the two tool waits; it does not change any wait bound, result shape, or delivery rule.
