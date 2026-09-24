# Result delivery and activation controls verification

Date completed: 2026-09-24 IST

Branch: `feat/result-delivery`

Base: `feat/minimum-incidents` at `24a851a` (`fix: close the schema-9 gate gaps in minimum incidents`), itself based on `feat/collaboration-tools` at `d72de10` and `main` at `81773b8`

Product version: `0.1.0`

Database schema: 9 (unchanged; no migration in this slice)

Scope: domain rendering, store notice ledger on the existing message and turn tables, daemon notice-delivery loop and `task_wait` hook, acceptance harness, and continuity documentation. Desktop product code and contract, the CLI, and the schema are unchanged.

## Outcome

PASS.

A lead now learns what its delegated work produced after its own turn has ended. Every child task that reaches `completed`, `failed`, `cancelled`, or `uncertain` by any path becomes one durable `result` message from the worker to the employee that owns the parent task, threaded under the delegation and carrying the state, failure, native output bounded to 16 KiB, and the number of unfinished siblings. Every incident promoted to `priority` about a child task becomes one `status` message to the lead of the tree. The daemon delivers each notice as a single provider turn on the lead's idle session, reusing the notice as the turn's canonical message and running under the lead task's effective authority; a lead already inside `task_wait` receives the outcome there and the notice is acknowledged with that turn as evidence.

The activation decision requested by the previous handoff is: employee-authored traffic never starts a provider process. A notice waits, with its turn already recorded, until the local user resumes the lead with `session.warm` or a user turn. Incidents about the lead itself, about the root task, or on a terminal tree never notify the lead. Bounded automatic activation of a dormant lead is deferred behind a per-employee activation policy that needs a migration.

No live model turn was run.

## Design notes

Design and implementation landed together under the owner's standing instruction to continue the workstreams; `docs/superpowers/specs/2026-09-24-result-delivery-design.md` and `docs/superpowers/plans/2026-09-24-result-delivery.md` record the contract for owner review before merge.

| Decision | Reason |
|---|---|
| No schema change: `result` and `status` message kinds and the turn ledger carry notices | The schema-4 bus already reserved those kinds; a notice turn is an ordinary turn whose `message_id` is the notice, so delivery, acknowledgement, failure, cancellation, and task projection reuse the existing paths. |
| Notices are queued after the task or incident commit, idempotently by child task or incident id, from a daemon pass that wakes on every published event | Seven store paths terminalize a task; one derived pass keyed on the ledgers covers all of them and recovers after a restart without touching any of them. |
| Only messages queued under the `task.result` and `incident.notice` command scopes are auto-delivered | Peer messages sent through `message.send` keep their manual claim and acknowledge path, so the collaboration-bus acceptance is unaffected. |
| A notice turn is recorded only while no other turn on the lead's task is `recorded`, `submitting`, or `accepted` | A user turn always takes precedence, and two notices for one lead are delivered one after the other. |
| A notice turn projects like any completed turn: with no unfinished child left it completes the lead's task | Matches the existing rule for user turns; the lead had its chance to delegate more or report before ending the turn. |
| A notice whose task is terminal, or whose lead was disabled or reconfigured, is cancelled rather than retried | Nobody is left to act on it; the child task, message, and incident rows keep the evidence, and the user made the change that closed the recipient. |

Two consequences surfaced while running the full gate against the real daemon:

| Finding | Evidence | Resolution |
|---|---|---|
| `task-delegation-graph.ps1` completed a parent task by hand after cancelling its child | The real daemon's notice loop delivered the cancelled child's result to the lead's idle fixture session, the fixture ended that turn, the parent completed, and the script's scripted `task report completed` failed with `LZ-TASK-STALE`. | The script now waits for the parent to complete through the notice, asserts one acknowledged `result` notice for it, and proves the manual completion is refused with `LZ-TASK-STATE` (`childResultNoticeCompletesParent`). Roots whose user turn is still `recorded` are unaffected because the store guard treats that turn as unfinished. |
| Three daemon acceptance-contract tests still required the string `schemaVersion 8` | Commit `24a851a` corrected five acceptance scripts to schema 9 after that day's workspace test run, so `employee_roster`, `native_session`, and `collaboration_bus` contract tests failed on this branch's first full run. | The three tests require `schemaVersion 9`. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/result-delivery` after the final fixes.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | All workspace targets linted with warnings denied. |
| `cargo test --workspace --locked` | 0 | Entire Rust workspace passed: 71 daemon integration tests (5 new: four notice scenarios and the acceptance-script contract), 5 new store delivery tests, 4 new domain delivery tests, 20 CLI tests, 16 Codex adapter tests, all other suites unchanged. Explicit live-provider tests remained ignored. |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `npm --prefix apps\lazoo-desktop\ui run check` | 0 | Prettier, TypeScript, 47 Vitest tests in 5 files, and the Vite production build passed on the unchanged frontend. |
| `scripts\acceptance\result-delivery.ps1` | 0 | PASS schema-9 manifest, `deterministic-fake-transport`, 15 commands, every evidence flag true. |
| `scripts\acceptance\minimum-incidents.ps1` | 0 | PASS schema-9 manifest. |
| `scripts\acceptance\collaboration-tools.ps1` | 0 | PASS schema-9 manifest. |
| `scripts\acceptance\approval-gate.ps1` | 0 | PASS schema-9 manifest. |
| `scripts\acceptance\first-slice.ps1 -SkipBuild` | 0 | PASS manifest; database schema 9. |
| `scripts\acceptance\employee-roster.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest; real Codex CLI accepting the six declared tools, non-model session contract only. |
| `scripts\acceptance\collaboration-bus.ps1 -SkipBuild` | 0 | PASS manifest; manual peer-message claim and acknowledge path unaffected by the notice loop. |
| `scripts\acceptance\conversation-delivery.ps1 -SkipBuild` | 0 | PASS deterministic manifest. |
| `scripts\acceptance\task-delegation-graph.ps1 -SkipBuild` | 0 | PASS manifest with the new `childResultNoticeCompletesParent` evidence, run against the real daemon with the notice loop active. |
| `scripts\acceptance\automatic-task-dispatch.ps1` | 0 | PASS schema-9 fake-transport manifest. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` requires a live model turn (`LAZOO_CODEX_OK`) that the handoff does not authorize. `desktop-shell.ps1 -SkipBuild` was attempted once and fails on a desktop accessibility-text assertion (`Search commands`) against the unchanged, separately owned UI; its tracked manifest was left as committed.

## Result-delivery acceptance evidence

`artifacts/acceptance/result-delivery-manifest.json` records literal `PASS`, schema 9, and `deterministic-fake-transport`. Its focused commands prove:

- a result notice names the child, worker, state, objective, failure, bounded output, remaining siblings, and parent, truncates oversized output on a UTF-8 boundary, and an incident notice names the code, title, occurrences, next action, incident id, and the no-retry boundary;
- a terminal child queues exactly one `result` to the parent's employee, threaded under the delegation, replays without writes, and a root or non-terminal task is refused;
- a notice turn reuses the notice as its canonical message with the worker as author and the lead as recipient, replays to the same turn, refuses non-notice messages, and waits while another turn on the lead's task is unfinished;
- `task_wait` consumes a result with the waiting turn as evidence, replays without writes, and a later notice turn is refused because the notice is no longer queued;
- a turn belonging to another lead cannot consume the result, and the refusal leaves nothing behind;
- three timeouts on a child promote an incident that notifies the lead once as a `status` message with incident and task references, while a normal incident and incidents about the lead itself never do;
- an idle lead receives the child's result as one more turn on its existing session after its own turn ended, with no new provider process, the notice acknowledged, and its task completed;
- a lead that read the outcome through `task_wait` never receives a second delivery;
- a stopped lead is never activated by a notice, the notice turn stays recorded, and an explicit `session.warm` by the user lets the next pass deliver the same turn;
- a promoted incident reaches the lead once as a status turn while its task stays running; and
- schema remains 9 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed.
- Both staged and unstaged diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, CLI command, or migration changed.
- No notice starts a provider process, widens authority, retries work, restarts anything, re-authenticates, or deletes data.
- No push or pull request was created.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Per-employee activation policy with bounded automatic activation of a dormant lead, `message_send` and `message_reply` peer tools on the same delivery path, coalescing several notices into one turn, bounded lead remediation, user notification channels, a task-change notification replacing the pass-per-event wake, typed desktop Inbox binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance remain separate reviewed slices.
