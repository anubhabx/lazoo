# Inbox uncertain-task resolutions verification

Date completed: 2026-10-13 IST

Branch: `fix/inbox-uncertain-descendants`

Base: `fix/workspace-action-guards` (pull request #22, stacked on #21 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: the one Inbox gap the workspace action review left open: a `task-uncertain` item offered `resolve-completed` and `resolve-cancelled` for a task with an unfinished descendant, which `task.resolve` refuses (`require_no_active_descendants`). No schema, RPC, or desktop change.

## Outcome

PASS.

`inbox_sources` now carries `active_descendants`, the ids of the listed uncertain tasks that still have a non-terminal descendant, read with the store's own recursive guard (`has_active_descendants`, the query `task.cancel` and `task.resolve` use). `task_inbox_item` takes that fact and keeps only `resolve-failed` for such a task, so an Inbox item never names a resolution the daemon would refuse. The archive passes `false`: a terminal task offers no action. The workspace card had already withheld the same actions (pull request #22); the two surfaces now agree.

No live model turn was run.

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `fix/inbox-uncertain-descendants` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test --workspace --locked` | 0 | One invocation, 408 passed, 0 failed, 11 ignored (explicit live-provider tests), 69 test binaries; the Inbox task-item test grew one assertion. |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.6 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| every acceptance script (deterministic ones; the real-binary ones with `-SkipBuild`; `resource-budget -SkipBuild`; `native-session-ownership -SkipBuild -Provider codex`) | 0 | All 30 PASS manifests at schema 15; `desktop-inbox.ps1` runs the corrected rule. `native-session-ownership` failed once inside the batch (the known process-tree timing flake when it runs right after `task-delegation-graph`: the gracefully stopped native tree outlived its 10-second wait) and passed when re-run alone in 14.5 s; this slice touches no session code. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No RPC, event, migration, collaboration tool, or CLI command changed; the Inbox stays derived and stores nothing.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.
