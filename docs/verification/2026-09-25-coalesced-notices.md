# Coalesced notice turns verification

Date completed: 2026-09-25 IST

Branch: `feat/coalesced-notices`

Base: `feat/peer-messaging` at `176979c` (`feat: let employees message peers through the notice path`), itself based on `feat/result-delivery` at `b320946`, `feat/minimum-incidents` at `24a851a`, `feat/collaboration-tools` at `d72de10`, and `main` at `81773b8`

Product version: `0.1.0`

Database schema: 9 (unchanged; no migration in this slice)

Scope: domain rendering of one turn from several notices, store batch gathering and companion delivery on the existing message and turn tables, one delivery per turn per daemon pass, acceptance harness, and continuity documentation. Desktop product code and contract, the CLI, the tool catalog, and the schema are unchanged.

## Outcome

PASS.

A lead that was busy or dormant while several children finished, incidents were promoted, or peers wrote to it now receives everything queued for its task as one provider turn, oldest first under a fixed header, instead of one turn per notice. The oldest notice is the turn's canonical message; every other notice is a companion linked through its own `turn.record` command and follows the canonical message through claim, acknowledgement, failure, retry, and cancellation under its own command key, so `lazooctl message list` still shows one delivery state and one acknowledgement per notice. A single queued notice is delivered with exactly the text it had before, so every earlier notice and peer-messaging acceptance still passes unchanged except the one scenario that asserted two separate turns on resume.

A notice read inside the recipient's own turn (the user resumed a dormant lead and it called `task_wait`) closes the recorded turn that carried it; the other notices stay queued and are recorded again on a fresh turn, never on the closed one. A turn carrying only peer messages still never completes the task; a turn carrying a result or incident notice completes it once no child is left, exactly as the result alone would have.

Activation policy is unchanged: employee-authored traffic never starts a provider process, and a coalesced turn for a dormant recipient waits, recorded, until the local user resumes it.

No live model turn was run.

## Design notes

Design and implementation landed together under the owner's standing instruction to continue the workstreams; `docs/superpowers/specs/2026-09-25-coalesced-notices-design.md` and `docs/superpowers/plans/2026-09-25-coalesced-notices.md` record the contract for owner review before merge.

| Decision | Reason |
|---|---|
| No schema change: companions are linked by one `turn.record` command each, keyed `notice-turn:<messageId>:<turnId>` | The command table already answers "which turn delivers this message" for the canonical notice; one row per companion answers it for every notice, and the same rows list what a turn carries. |
| The turn id is part of the key, and the older `notice-turn:<messageId>` form is matched by prefix | A notice read inside another turn closes its recorded batch turn while the rest stay queued; those need a fresh turn, which a key by message alone would refuse under the `(scope, key)` primary key. A replay whose turn is `cancelled` while the message is still `queued` therefore records anew. |
| A recorded turn is immutable: nothing joins it after recording | Rewriting a recorded turn's text and swapping its canonical message would have been a second bookkeeping path; a later notice simply waits for the next turn, which the existing "no unfinished turn" guard already enforces. |
| A batch stops at the 256 KiB turn text limit and the overflow waits | The provider prompt limit is the only bound that matters; sixteen 16 KiB notices already exceed it. |
| A turn carrying only peer messages projects as progress; a turn carrying any result or incident notice projects like any turn | The peer-only rule existed so a question could not cancel a result turn that followed; a turn that carries the result itself is that result turn. |
| Companion acknowledgements cite `{turnId, canonicalMessageId}` under key `coalesced:<turnId>` | The provider's acceptance evidence belongs to the canonical message once; a companion's evidence names the turn and the message that holds it. |
| The daemon re-reads each queued notice before acting and delivers each turn once per pass | The pass iterates notices, but several now share a turn; without the re-read a result carried earlier in the pass was cancelled against a stale revision after the task completed. |

One expected contract change surfaced in the existing suites:

| Finding | Evidence | Resolution |
|---|---|---|
| The dormant-recipient peer scenario asserted the peer message and the child's result as two turns on resume | With both queued while the lead was dormant, the resume now delivers one `[Lazoo notices] 2` turn, and that turn completes the task because it carries the result. | The scenario now asserts three turns, one coalesced prompt containing both notices, the peer message as the canonical message, and the result acknowledged with the turn as evidence; its refused-calls contract is unchanged. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/coalesced-notices` after the final fixes.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | All workspace targets linted with warnings denied. |
| `cargo test --workspace --locked` | 0 | Entire Rust workspace passed: 77 daemon integration tests (2 new: the two-result scenario and the acceptance-script contract; 1 rewritten), 12 store delivery tests (3 new), 8 domain delivery tests (1 new), 20 CLI tests, 16 Codex adapter tests, all other suites unchanged. Explicit live-provider tests remained ignored. |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `npm --prefix apps\lazoo-desktop\ui run check` | 0 | Prettier, TypeScript, Vitest, and the Vite production build passed on the unchanged frontend. |
| `scripts\acceptance\coalesced-notices.ps1` | 0 | PASS schema-9 manifest, `deterministic-fake-transport`, 10 commands, every evidence flag true. |
| `scripts\acceptance\peer-messaging.ps1` | 0 | PASS schema-9 manifest; the dormant-recipient scenario now proves one coalesced turn on resume. |
| `scripts\acceptance\result-delivery.ps1` | 0 | PASS schema-9 manifest; single notices are delivered exactly as before. |
| `scripts\acceptance\minimum-incidents.ps1` | 0 | PASS schema-9 manifest. |
| `scripts\acceptance\collaboration-tools.ps1` | 0 | PASS schema-9 manifest; the tool catalog is unchanged. |
| `scripts\acceptance\approval-gate.ps1` | 0 | PASS schema-9 manifest. |
| `scripts\acceptance\automatic-task-dispatch.ps1` | 0 | PASS schema-9 fake-transport manifest. |
| `scripts\acceptance\first-slice.ps1 -SkipBuild` | 0 | PASS manifest; database schema 9. |
| `scripts\acceptance\employee-roster.ps1 -SkipBuild` | 0 | PASS manifest. |
| `scripts\acceptance\collaboration-bus.ps1 -SkipBuild` | 0 | PASS manifest; manual `message.send` claim and acknowledge path unaffected. |
| `scripts\acceptance\conversation-delivery.ps1 -SkipBuild` | 0 | PASS deterministic manifest. |
| `scripts\acceptance\task-delegation-graph.ps1 -SkipBuild` | 0 | PASS manifest, run against the real daemon with the notice loop active. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest; real Codex CLI accepting the eight declared tools, non-model session contract only. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` requires a live model turn (`LAZOO_CODEX_OK`) that the handoff does not authorize, and `desktop-shell.ps1` asserts against the unchanged, separately owned UI; its tracked manifest was left as committed.

## Coalesced-notices acceptance evidence

`artifacts/acceptance/coalesced-notices-manifest.json` records literal `PASS`, schema 9, and `deterministic-fake-transport`. Its focused commands prove:

- several notices render oldest first under one header that says how many are waiting and that each is separate evidence, and one notice renders unchanged;
- a peer question and two cancelled children's results queued for one lead become one recorded turn whose canonical message is the requested notice, whose text carries all three in order, that replays to the same turn from any member, that keeps every notice queued until submission, and whose completion acknowledges the canonical notice with the provider's evidence and each companion with the turn and canonical message id, completing the task because it carried results;
- a result read inside a resumed user turn closes the two-notice batch turn, leaves the other result queued, refuses a new turn while the user turn is unfinished, and once it ends records the remaining result on a fresh turn with its own single-notice text, never the closed turn;
- seventeen 16 KiB peer messages stop at the turn text limit with the overflow waiting, oldest first, and a peer-only turn leaves the task running;
- a peer-only turn never completes the recipient's task while the result turn that follows does;
- two child results reach an idle lead as one turn on its existing session, oldest first, with no new provider process, the oldest result as the canonical message, the other acknowledged with the turn as evidence, and the task completed;
- a single child result is delivered exactly as before;
- a dormant lead is never activated, and after the user's explicit `session.warm` the waiting peer message and result arrive as one turn inside which four refused calls answer stable codes without ending the turn; and
- schema remains 9 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed.
- Both staged and unstaged diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, CLI command, tool declaration, or migration changed.
- No notice is merged, summarised, or truncated into another; no coalesced turn starts a provider process, widens authority, approves anything, changes task ownership, retries work, or deletes data.
- No push or pull request was created.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Per-employee activation policy with bounded automatic activation of a dormant recipient (now one activation per recorded turn), cross-tree and user-to-employee messaging through the tools, a message-arrival notification replacing the 250 ms polls, a live Codex tool-call and coalesced-notice gate, bounded lead remediation, user notification channels, typed desktop Inbox binding, ConPTY, per-capability policy, managed memory, scheduling, and resource governance remain separate reviewed slices.
