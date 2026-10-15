# Workspace actions verification (phase 3)

Date completed: 2026-10-13 IST

Branch: `feat/workspace-actions`

Base: `feat/workspace-conversation` (pull request #19, stacked on #18 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: `actions` on the workspace conversation's lead header and worker cards, the two domain rules behind them, the Inbox action builders shared inside `lazoo-domain`, the lead turn's revision in the store read, one optional field on `conversation.rename`, acceptance harness, and continuity documentation. No new RPC; the CLI, the desktop app, Tauri surface, `desktop.snapshot`, and collaboration tools are unchanged.

## Outcome

PASS.

Section 6 of the desktop workspace contract (`docs/superpowers/specs/2026-10-13-desktop-workspace-contract-design.md`) is now served: `workspace.conversation` names, on the header and on each card, the actions the rows allow right now, as `InboxAction` data for an existing revision-checked RPC with the `expectedRevision` the user saw. The header offers `rename` always (the caller sets `title`), `cancel-turn` while the lead's newest turn is unfinished or retryable, `retry-turn` while it is retryable, and `stop-session` while the lead's session is idle with no unfinished turn. A card offers `accept` and a user `cancel` for a proposal, the three resolutions for an uncertain task, the pending approval's `approve` and `decline` first when one names the task or its active turn, `cancel` for a waiting or running task, and nothing once the task is terminal or already cancelling. The caller adds a fresh `idempotencyKey` and sends exactly that request; naming an RPC grants nothing and every named RPC keeps its own guards.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| The Inbox's `task_actions`, `approval_actions`, and `action` builders are shared inside `lazoo-domain` | One shape for an action everywhere the desktop sees one (contract principle 4); a card's accept is byte-for-byte the Inbox proposal's accept. |
| A blocked waiting or running worker still offers `cancel` | `task.cancel` accepts an accepted or running task whatever its employee's availability or incidents; offering it keeps the dock truthful about what the user can do. |
| `conversation.rename` accepts an optional `idempotencyKey` it does not record | Every other action takes one; a rename is naturally idempotent, so accepting the key costs nothing and lets the desktop send every action the same way. |
| `stop-session` is offered only when no lead turn is unfinished | Guardrail: never stop a session that has an unfinished turn on it; the header does not name what the daemon would refuse. |
| The lead's newest turn carries its revision from the store | `turn.cancel` and `turn.retry` are revision-checked; the header must cite the revision the user saw. |
| `#[allow(clippy::too_many_lines)]` on the domain table test and the daemon story test | One table over every card state and header condition, and one user session acting from both, as the repository's other story tests are. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/workspace-actions` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean with the two story allowances. |
| `cargo test --workspace --locked` | 0 | One invocation, 407 passed, 0 failed, 11 ignored (explicit live-provider tests), 69 test binaries. New: `lazoo-domain` 1 (the `workspace` test binary), `lazood` 2 (the actions story and the acceptance-script contract; 116 daemon tests). |
| `cargo build --workspace --release --locked` | 0 | Locked release build (up to date in 0.7 s: the task-graph acceptance contract test inside the workspace test run had just rebuilt the release binaries from the same sources). |
| `scripts\acceptance\workspace-actions.ps1` | 0 | PASS schema-15 manifest, `deterministic-fake-transport`, 5 commands, every evidence flag true. |
| every other acceptance script (deterministic ones; the real-binary ones with `-SkipBuild`; `resource-budget -SkipBuild`; `native-session-ownership -SkipBuild -Provider codex`) | 0 | All 29 PASS manifests at schema 15. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Workspace-actions acceptance evidence

`artifacts/acceptance/workspace-actions-manifest.json` records literal `PASS`, schema 15, and `deterministic-fake-transport`. Its focused commands prove:

- the action rules: a proposal offers `accept` then a user `cancel` with the proposal's revision; an uncertain task the three `task.resolve` resolutions; a waiting or running task `cancel`; a pending approval its `approve` and `decline` first and a decided one nothing extra; a cancel-requested or terminal task nothing; the header offers `rename` always with the conversation's revision and a null title, `cancel-turn` for an accepted or retryable lead turn with the turn's revision, `retry-turn` only for a retryable one, and `stop-session` only for an idle session with no unfinished turn (not for a suspended one);
- through the daemon, a dormant lead offers only `rename` and a warmed one `rename` and `stop-session`; a proposal's card offers `accept` and `cancel`, sending `accept` exactly as named with a fresh idempotency key is accepted and the card then offers only `cancel` at the new revision; the header's `rename` sent with the caller's title changes the title and the next header cites the new revision; a completed card offers nothing; the header's `stop-session` sent as named suspends the lead, after which only `rename` remains; and no event names the workspace;
- the phase 2 story still passes unchanged, so the header and cards stay truthful; and
- schema is 15 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, migration, event, collaboration tool, or CLI command changed; no RPC was added; `conversation.rename` gained one optional, ignored field.
- The actions are data: nothing is stored, nothing is granted, and every named RPC keeps its own revision, actor, and state guards.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

The desktop binding of the three workspace read models and their actions (the UI owner's capability review), and everything else NEXT-SESSION lists, remain separate reviewed slices.
