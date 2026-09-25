# Peer messaging tools verification

Date completed: 2026-09-25 IST

Branch: `feat/peer-messaging`

Base: `feat/result-delivery` at `b320946` (`feat: deliver child results and priority incidents to the lead`), itself based on `feat/minimum-incidents` at `24a851a`, `feat/collaboration-tools` at `d72de10`, and `main` at `81773b8`

Product version: `0.1.0`

Database schema: 9 (unchanged; no migration in this slice)

Scope: domain rendering and validation, store peer-message ledger on the existing message and turn tables, daemon tool surface and in-turn inbox, acceptance harness, and continuity documentation. Desktop product code and contract, the CLI, and the schema are unchanged.

## Outcome

PASS.

A daemon-managed Codex employee can now ask another employee in its task tree a question and answer one. `message_send` records one durable `peer` message from the turn's own endpoint to the recipient's active task, linking both tasks, and can block up to ten minutes for the reply; `message_reply` records a `reply` threaded under the message it answers and addressed back to the sender's task. A recipient reads peer messages inside its own `task_wait`, which now returns early with `messages` so a waiting lead can answer and wait again, or inside its own `message_send` wait, or, when idle with a running task and no unfinished turn, as one provider turn on its existing session through the notice loop the result-delivery slice built. A peer turn only progresses the recipient's task; result notices and the employee's own turns keep completing tasks.

Activation policy is unchanged: employee-authored traffic never starts a provider process. A peer message to a dormant recipient waits, with its turn recorded, until the local user resumes that employee; a message whose recipient task ends first is cancelled and a waiting sender returns with that state. Messaging stays inside one task tree.

No live model turn was run.

## Design notes

Design and implementation landed together under the owner's standing instruction to continue the workstreams; `docs/superpowers/specs/2026-09-25-peer-messaging-design.md` and `docs/superpowers/plans/2026-09-25-peer-messaging.md` record the contract for owner review before merge.

| Decision | Reason |
|---|---|
| No schema change: peer messages are `peer` and `reply` rows queued under a new `message.peer` command scope | The notice loop already delivers any queued message under an auto-delivery scope as one turn reusing the message; adding a scope reuses delivery, acknowledgement, failure, cancellation, and projection unchanged. |
| The recipient is resolved to its `accepted` or `running` task under the caller's root task | Messages stay inside the tree the lead built, an employee outside it cannot be reached, and no dormant employee can be activated by naming it. |
| A peer turn is recorded only while the recipient's task is `running` | Delivering to an `accepted` task would start it with a peer message instead of its objective, and the dispatcher would never run; the message waits until the task starts. |
| A completed peer turn projects as task progress, never completion | With no unfinished child left, a completed turn otherwise completes the task; a question delivered just before the child's result would then cancel that result. |
| `task_wait` returns early when a peer message arrives; `message_send` waits for the reply | A lead is most often inside `task_wait` when a worker needs an answer; the exchange finishes inside both turns without a notice turn and without activating anyone. |
| Reading a message inside a turn closes any `recorded` notice turn for the same message | A dormant recipient's recorded notice turn was otherwise orphaned when the user resumed it and the message was read inside the new turn, and the orphan blocked every later notice turn on that task. This also closes the same gap for result notices consumed through `task_wait`. |
| Content over 16 KiB is refused, not truncated | The sender wrote it and can shorten it; a silently cut question would be answered wrongly. |

One finding surfaced while writing the daemon scenarios:

| Finding | Evidence | Resolution |
|---|---|---|
| The fake session backend drained every scripted tool call in the first turn that ran, so a worker session could never be scripted alongside its lead | The peer scenarios need the worker to call `message_send` in its own dispatch turn while the lead answers in a later turn. | Scripted calls now name their session and are drained per session (`session_tool_call`), and a `$message-id` argument is substituted from the last tool result's inbox or the turn prompt, as a live model would read it. Existing tool tests are unchanged because their calls already named the first session. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/peer-messaging` after the final fixes.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | All workspace targets linted with warnings denied. |
| `cargo test --workspace --locked` | 0 | Entire Rust workspace passed: 75 daemon integration tests (4 new: three peer scenarios and the acceptance-script contract), 9 store delivery tests (4 new), 7 domain delivery tests (3 new), 20 CLI tests, 16 Codex adapter tests, all other suites unchanged. Explicit live-provider tests remained ignored. |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `npm --prefix apps\lazoo-desktop\ui run check` | 0 | Prettier, TypeScript, Vitest, and the Vite production build passed on the unchanged frontend. |
| `scripts\acceptance\peer-messaging.ps1` | 0 | PASS schema-9 manifest, `deterministic-fake-transport`, 12 commands, every evidence flag true. |
| `scripts\acceptance\result-delivery.ps1` | 0 | PASS schema-9 manifest. |
| `scripts\acceptance\minimum-incidents.ps1` | 0 | PASS schema-9 manifest. |
| `scripts\acceptance\collaboration-tools.ps1` | 0 | PASS schema-9 manifest; every daemon-managed Codex session now declares eight tools. |
| `scripts\acceptance\approval-gate.ps1` | 0 | PASS schema-9 manifest. |
| `scripts\acceptance\automatic-task-dispatch.ps1` | 0 | PASS schema-9 fake-transport manifest. |
| `scripts\acceptance\first-slice.ps1 -SkipBuild` | 0 | PASS manifest; database schema 9. |
| `scripts\acceptance\employee-roster.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\collaboration-bus.ps1 -SkipBuild` | 0 | PASS manifest; manual `message.send` claim and acknowledge path unaffected by the new scope. |
| `scripts\acceptance\conversation-delivery.ps1 -SkipBuild` | 0 | PASS deterministic manifest. |
| `scripts\acceptance\task-delegation-graph.ps1 -SkipBuild` | 0 | PASS manifest, run against the real daemon with the notice loop active. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest; real Codex CLI accepting the eight declared tools, non-model session contract only. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` requires a live model turn (`LAZOO_CODEX_OK`) that the handoff does not authorize, and `desktop-shell.ps1` asserts against the unchanged, separately owned UI; its tracked manifest was left as committed.

## Peer-messaging acceptance evidence

`artifacts/acceptance/peer-messaging-manifest.json` records literal `PASS`, schema 9, and `deterministic-fake-transport`. Its focused commands prove:

- a peer message names the sender, both tasks, and the message id to cite in `message_reply`, states that it is not an approval, a reply names the message it answers, and empty or oversized content is refused;
- a message to oneself, to an employee without a task in the tree, or with empty or oversized text is refused and leaves nothing behind; a valid message is authored by the turn's own endpoint, addressed to the recipient's task, references both tasks, replays without writes, appears in the recipient's inbox and in the notice queue, and waits while the recipient's task is still `accepted`;
- only the addressed employee, from the addressed task, can reply, only to a peer message; a reply carries the original's correlation and causation, returns to the sender's task, and reaches an idle lead as a turn authored by the worker;
- a message read inside a turn is claimed and acknowledged with `{via, turnId}` evidence, replays without writes, cannot be read by a foreign turn or delivered again, and reading it closes a stale notice turn recorded while the recipient was dormant;
- a completed peer turn leaves the recipient's task running, and the child's result that follows completes it;
- a worker's question reaches the idle lead as one more turn on its existing session, the lead's reply returns inline to the worker blocked in `message_send`, no provider process starts, and the lead's task completes only through the child's result;
- `task_wait` returns early with the worker's question, the lead answers with `message_reply` and waits again, the worker receives the reply inline, and the whole exchange happens inside two turns with no notice turn;
- a message to a stopped lead stays queued with its turn recorded, an explicit `session.warm` by the user delivers that same turn, four refused calls inside it answer stable codes without ending the turn, and the queued result notice follows in the same pass; and
- schema remains 9 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed.
- Both staged and unstaged diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, CLI command, or migration changed.
- No peer message starts a provider process, widens authority, approves anything, changes task ownership, completes a task, retries work, or deletes data.
- No push or pull request was created.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Per-employee activation policy with bounded automatic activation of a dormant recipient, cross-tree and user-to-employee messaging through the tools, coalescing several notices and peer messages into one turn, a message-arrival notification replacing the 250 ms polls, bounded lead remediation, user notification channels, typed desktop Inbox binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance remain separate reviewed slices.
