# Read model resume verification

Date completed: 2026-10-13 IST

Branch: `test/read-model-resume`

Base: `feat/provider-migration-preview` (pull request #24, stacked on #23 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: one deterministic story that proves the desktop's live-update contract for every derived read model (the Inbox counts, `workspace.conversations`, `workspace.launcher`, `workspace.conversation`): a subscriber that resumes from the `eventHighWater` a read reports loses no later change. The phase 2 acceptance script runs it. No production code changed.

## Outcome

PASS.

The Inbox contract and the workspace contract (section 5) tell the desktop to read a model, keep its `eventHighWater`, subscribe to events after it, and re-read on the event kinds each model depends on. Pull request #22 made every workspace result report the lowest mark its reads reflect; this story proves the invariant that makes resumption safe, for all four models, across five changes: right after a read, `events.list { after: eventHighWater }` is empty (the mark is never ahead of the ledger); after the lead warms, after the lead's turn delegates under ask, and after the user accepts the proposal, renames the conversation, and disables the worker, the events after each model's earlier mark carry the session, turn, task, conversation, and employee events the contract names for it; each model re-read reflects the change (the lead idle, the launcher's Mira idle, one Inbox item then none, the sidebar row `needs-attention` then `active` with the new title, the header's card `blocked` with "Blocked: Mira is disabled"); and each new mark covers the newest event seen. No change written between a read and its mark is ever below that mark.

No live model turn was run.

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `test/read-model-resume` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean with the one story allowance. |
| `cargo test --workspace --locked` | 0 | One invocation, 413 passed, 0 failed, 11 ignored (explicit live-provider tests), 70 test binaries. New: `lazood` 1 (the resume story). |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.7 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| `scripts\acceptance\workspace-conversation.ps1` | 0 | PASS schema-15 manifest; `marksResumeWithoutLoss` is a new evidence flag. |
| every other acceptance script (deterministic ones; the real-binary ones with `-SkipBuild`; `resource-budget -SkipBuild`; `native-session-ownership -SkipBuild -Provider codex`) | 0 | All 30 PASS manifests at schema 15. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Boundary evidence

- No production code changed: the diff is one daemon story, one acceptance evidence flag, and documentation.
- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.
