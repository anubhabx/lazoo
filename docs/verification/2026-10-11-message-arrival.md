# Message arrival verification

Date completed: 2026-10-11 IST

Branch: `feat/message-arrival-notify`

Base: `feat/user-notifications` at `88ee4b4` (pull request #5, open, stacked on #4, #3, and #2)

Product version: `0.1.0`

Database schema: 13 (no migration)

Scope: the daemon's store-change wakeup for the `task_wait` and `message_send` tool waits, a dev-only Tokio `test-util` feature for the paused-clock proof, acceptance harness, and continuity documentation. The desktop app, Tauri surface, tools, CLI, RPC methods, protocol, and store are unchanged.

## Outcome

PASS.

`task_wait` and a `message_send` waiting for its reply no longer re-read the store every 250 ms. `publish_events`, which follows every daemon store write, wakes one `tokio::sync::Notify`; each wait iteration registers for it before reading the store and then parks on it or its unchanged deadline. A child's completion, a peer question, a reply, a closed message, or a cancelled turn ends the wait as soon as it is published, and a quiet wait does no reads. Deadlines, result fields, and acknowledgement evidence are unchanged. The project-registration path, the only write that published the outbox by hand, now goes through `publish_events`.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| One `Notify` woken from `publish_events` | Every daemon write is already followed by it to move the outbox onto the event bus, so no write path needs its own hook and a forgotten one would also be missing from the event stream. |
| Register before reading | A `Notified` future receives `notify_waiters` from the moment it is created, so a write landing between the reads and the park still wakes the wait. |
| No fallback poll | The store is written only by this daemon, and every write publishes; the deadline still bounds every wait. |
| Paused-clock proof | Tokio's paused clock advances only when every task is parked on a timer, so the test measures exactly how long the waits slept, deterministically, with no wall-clock threshold. |
| Turn-ended check shared by both waits | Extracted to `ensure_turn_still_waiting`, which also kept `tool_task_wait` under clippy's line limit. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/message-arrival-notify` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean after extracting the shared turn check. |
| `cargo test --workspace --locked` | 0 | One invocation, 353 passed, 0 failed, 11 ignored (explicit live-provider tests). New: `lazood` 2 (the paused-clock wake test and the acceptance-script contract; 93 daemon tests). |
| Paused-clock test against the previous polling code | 101 | Expected failure: "the waits slept 500ms of virtual time instead of waking on the write", which proves the test detects polling. |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `scripts\acceptance\message-arrival.ps1` | 0 | PASS schema-13 manifest, `deterministic-fake-transport`, 6 commands, every evidence flag true. |
| `user-notifications`, `incident-thresholds`, `lead-remediation`, `idle-suspension`, `activation-policy`, `coalesced-notices`, `peer-messaging`, `result-delivery`, `minimum-incidents`, `collaboration-tools`, `approval-gate`, `automatic-task-dispatch` | 0 | PASS schema-13 manifests. |
| `first-slice`, `employee-roster`, `collaboration-bus`, `conversation-delivery`, `task-delegation-graph`, each with `-SkipBuild` | 0 | PASS manifests against the real release binaries, with the store-change wakeup running in the real daemon. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest; real Codex CLI, non-model session contract only. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI). `npm --prefix apps\lazoo-desktop\ui run check` was not run because the frontend is unchanged.

## Message-arrival acceptance evidence

`artifacts/acceptance/message-arrival-manifest.json` records literal `PASS`, schema 13, and `deterministic-fake-transport`. Its focused commands prove:

- on the paused clock, a lead's delegate, question, reply, and completion exchange takes under 250 ms of virtual time: `task_wait` woke for the peer question, `message_send` for the reply, and the second `task_wait` for the child's completion, and none timed out;
- the existing early-return and result-consumption stories still pass, with results read inside a wait keeping their `via` evidence;
- `WAIT_POLL_INTERVAL` is gone from the tool source; and
- schema is 13 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, tool, CLI command, RPC method, protocol type, or store schema changed.
- The notify carries no data; the store stays authoritative and nothing is delivered through it.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

A message priority field with its dead-letter threshold, configurable thresholds, a deadline-driven sweep, the live Codex gate, typed desktop Inbox binding, and everything else NEXT-SESSION lists remain separate reviewed slices.
