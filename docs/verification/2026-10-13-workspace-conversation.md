# Workspace conversation verification (phase 2)

Date completed: 2026-10-13 IST

Branch: `feat/workspace-conversation`

Base: `feat/workspace-read-models` (pull request #18, stacked on #17 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: the header, card, and result protocol types, the domain rules (lead lifecycle, worker card state, bounded label), the store's one-conversation read, the daemon's `workspace.conversation`, `lazooctl workspace show`, acceptance harness, and continuity documentation. The desktop app, Tauri surface, `desktop.snapshot`, and collaboration tools are unchanged; the actions phase of the contract (section 6) is not part of this slice.

## Outcome

PASS.

The third read model of the desktop workspace contract (`docs/superpowers/specs/2026-10-13-desktop-workspace-contract-design.md`) is now served, derived on every call and never stored. `workspace.conversation { id, turnLimit? }` returns the conversation record with its title, the scope (a registered project's id, name, canonical path, and checkout mode, or Home), the lead header (employee, role, harness, model, effort, roster availability, the newest root task's effective authority, the lead's latest session with its native host pid and private working directory, a truthful lifecycle, and the unfinished turn if any), the newest root task, the newest `turnLimit` turns oldest first with a truncation flag, one worker card per task below a root in the information architecture's eight states with Lazoo's words beside the row's own state, and the conversation's own unresolved Inbox items as `attention`. Section 9's questions 3 and 5 were taken as proposed on the owner's direction to finish as much as possible (an unresolved incident escalated to the lead or the user blocks its worker; the Inbox items are embedded), as questions 1, 2, 4, 6, and 7 were for phase 1.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| The lifecycle also reports `stopping` | A session being stopped is neither idle nor suspended; the header shows truthful states only (information architecture section 8.1). |
| The header reports the lead's private working directory, not a project workspace | `start_new_session` starts a lead in `DataRoot::session_working_dir(session)`; a lead session does not run in the project checkout today, so claiming a checkout path would be untrue. The project scope is reported separately. |
| `authority` is null before the first root task | A conversation has no authority until a user turn creates its root task; the launcher chooses it per start. |
| `blocked` also covers an acknowledged escalation | An acknowledged escalation is still an unresolved problem on the task; only `resolved` lifts it. |
| Cards carry `rootTaskId` and `effectiveAuthority`, and no `actions` | A follow-up user turn after a completed root creates a new root, so cards name theirs; an always-empty `actions` field would suggest a surface that does not exist yet (phase 3). |
| The store reads the worker tree with one small read per task | The tree is bounded (fan-out 8, depth 4); one joined query would be longer than it is worth. |
| `#[allow(clippy::too_many_lines)]` on the domain table test and the daemon story test | One state table and one user session over a conversation's whole life, as the repository's other story tests are. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/workspace-conversation` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean with the two story allowances. |
| `cargo test --workspace --locked` | 0 | One invocation, 404 passed, 0 failed, 11 ignored (explicit live-provider tests), 69 test binaries. New: `lazoo-domain` 2 (the `workspace` test binary), `lazooctl` 1, `lazood` 2 (the workspace conversation story and the acceptance-script contract; 114 daemon tests). |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.55 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| `scripts\acceptance\workspace-conversation.ps1` | 0 | PASS schema-15 manifest, `deterministic-fake-transport`, 6 commands, every evidence flag true. |
| every other acceptance script (deterministic ones; the real-binary ones with `-SkipBuild`; `resource-budget -SkipBuild`; `native-session-ownership -SkipBuild -Provider codex`) | 0 | All 28 PASS manifests at schema 15. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Workspace-conversation acceptance evidence

`artifacts/acceptance/workspace-conversation-manifest.json` records literal `PASS`, schema 15, and `deterministic-fake-transport`. Its focused commands prove:

- the lifecycle rule: no session is `dormant`, an idle session with the lead's newest turn unfinished is `busy`, an uncertain newest turn is `unknown`, and otherwise the session's own `starting`, `stopping`, `suspended`, or `failed`; the worker state table in the contract's order with Lazoo's words for every rule (`uncertain` → unknown; a pending approval → needs-approval naming the action; cancel-requested and cancelled → interrupted; failed with its code; completed; proposed → blocked on the user; a disabled, offline, unverified, or failed worker → blocked with the roster's words while `starting` is not a block; an incident escalated to the lead or the user → blocked naming it; running only with a submitting or accepted turn; otherwise waiting with the dispatch stage) and the label bounded to 120 bytes;
- `lazooctl workspace show <id> [--turn-limit]` maps to `workspace.conversation`;
- through the daemon, a fresh conversation has a dormant lead, no authority, no root task, no workers, no attention, and no turns; a zero limit, an unknown field, and an unknown conversation are refused with `LZ-WORKSPACE-INVALID` and `LZ-CONVERSATION-NOT-FOUND`; a warmed lead is idle with its session and a working directory named after it; the lead's first turn delegating two tasks under ask derives the title, sets the root task and the header's authority to `ask`, lists two blocked cards waiting for the user with bounded labels and two proposals in `attention`; accepting one makes it wait for dispatch, cancelling the other as the user interrupts it with a terminal time and empties `attention`; dispatch completes the accepted worker on a projectless workspace with its session and turn released, its turn joins the conversation's turns, and `turnLimit: 1` returns only the newest with `turnsTruncated`; the result reaching the lead completes the root while the lead stays idle; stopping the lead's session makes the header `suspended`; no event names the workspace and the desktop snapshot is unchanged; and
- schema is 15 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, migration, event, or collaboration tool changed; the daemon gains `workspace.conversation`, and `lazooctl` gains `workspace show`.
- The workspace stores nothing and mutates nothing; no card or header names an action yet.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

The actions phase of the contract (section 6), the desktop binding of all three read models, and everything else NEXT-SESSION lists remain separate reviewed slices.
