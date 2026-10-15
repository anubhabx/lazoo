# Workspace action guards verification

Date completed: 2026-10-13 IST

Branch: `fix/workspace-action-guards`

Base: `feat/core-scenario-stall` (pull request #21, stacked on #20 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: four corrections to the workspace conversation read model from an independent review of pull requests #19 and #20, before the owner's review: two actions that could be named when their RPC would refuse them, an event high-water mark that could overstate what the read reflected, and a silent task bound. No schema, RPC, or desktop change.

## Outcome

PASS.

| Finding | Correction |
|---|---|
| `retry-turn` was offered for any retryable-failed lead turn, but `turn.retry` is delivered only on an idle session this daemon owns (`require_owned_idle_session`); after an idle suspension or a restart the header named an action that returned `LZ-TURN-SESSION-NOT-READY`. | `lead_actions` offers `retry-turn` only while the lead's session is `idle`, and the daemon drops it when that session belongs to another boot. |
| `cancel`, `resolve-completed`, and `resolve-cancelled` were offered for a task with an unfinished descendant, which `task.cancel` and `task.resolve` refuse (`require_no_active_descendants`). | `active_descendant_ids` in `lazoo_domain::workspace` finds every task with a non-terminal descendant from the conversation's task summaries; the store carries it on each `WorkerRow`, and `worker_actions` keeps only the approval's decisions and `resolve-failed` for such a task. |
| `eventHighWater` was the higher of two reads taken under separate locks, so an event between them (a session suspension, a roster change) could carry a sequence at or below the reported mark while the header still showed the older truth; a desktop subscribing from that mark would never learn of it. | Both workspace handlers report the lower mark. A redundant re-read is harmless; a missed one is not. |
| The conversation's tasks were read with one page of 1024, so a long-lived conversation would mis-pick its newest root and drop its newest cards without saying so. | `conversation_tasks` pages on `afterSequence` until a short page. |

The review also noted that `lead_lifecycle` reports `unknown` for an uncertain newest lead turn before it reports `dormant` for a missing session. That order is kept and now stated in the contract's status: an uncertain turn is the stronger truth, and `unknown` is reserved for `uncertain` rows.

No live model turn was run.

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `fix/workspace-action-guards` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test --workspace --locked` | 0 | One invocation, 408 passed, 0 failed, 11 ignored (explicit live-provider tests), 69 test binaries. New: `lazoo-domain` 1 (`active_descendants_are_found_through_every_ancestor`); the action table test covers the two guards. |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.6 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| every acceptance script (deterministic ones; the real-binary ones with `-SkipBuild`; `resource-budget -SkipBuild`; `native-session-ownership -SkipBuild -Provider codex`) | 0 | All 30 PASS manifests at schema 15; `workspace-actions.ps1` runs the corrected table. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No RPC, event, migration, collaboration tool, or CLI command changed; the read models stay derived and store nothing.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

The Inbox's own `task-uncertain` item offers `resolve-completed` and `resolve-cancelled` without knowing about active descendants (pull request #11's shape); an uncertain task with an unfinished child is rare (it must have delegated before its outcome was lost) and the RPC refuses the request cleanly, so this is recorded in NEXT-SESSION rather than changed here.
