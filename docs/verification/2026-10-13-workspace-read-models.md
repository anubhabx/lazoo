# Workspace read models verification (phase 1)

Date completed: 2026-10-13 IST

Branch: `feat/workspace-read-models`

Base: `feat/core-scenario` (pull request #17, stacked on #16 to #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: the workspace protocol types, the domain rules (marker, title, startability), the store's one-space read, the daemon's `workspace.conversations` and `workspace.launcher`, `lazooctl workspace conversations|launcher`, acceptance harness, and continuity documentation. The desktop app, Tauri surface, `desktop.snapshot`, and collaboration tools are unchanged; `workspace.conversation` (the header and worker dock) is phase 2.

## Outcome

PASS.

The desktop workspace contract (`docs/superpowers/specs/2026-10-13-desktop-workspace-contract-design.md`) proposed three read models; the first two are now served, derived on every call and never stored. `workspace.conversations { projectId | null, limit?, before? }` returns one space's conversations newest first with a title (the stored one, else the first user request's first line bounded to 80 bytes, else "New work"), a marker that follows the rows (`needs-attention` when a counted Inbox item is scoped to the conversation, `active` when a worker runs, a turn is in flight, or a worker waits, `failed` when the plan or the last turn failed, `completed` when the plan completed, otherwise none) with Lazoo's words for it, worker counts, the newest root's state, the last turn's state, the last terminal time, and the space's remembered lead (its newest conversation's lead), paged with the Inbox's opaque cursor. `workspace.launcher { projectId | null }` returns the project, every employee as a lead candidate with its roster availability, whether a start would succeed now and Lazoo's words for why not, the remembered lead and why it may no longer work, the three authority modes, and the start request as data for `conversation.create` then `turn.submit` with the fields the desktop fills. Section 9's proposed answers were taken as defaults again (titles derive and `conversation.rename` edits them; "unread" is the desktop's own mark against `lastResultAt`; a `failed` lead is a block; lead memory is the newest conversation; launcher options are the employee's configured facts).

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| Phase 1 is the sidebar and the launcher only | They are the two surfaces the information architecture's slice 3 needs; the header and worker dock (slice 4) wait for the UI owner's answers on the `blocked` rule and the embedded Inbox items. |
| Counted items come from the Inbox derivation, not a second rule | One notion of "needs the user" (pull request #11); the workspace counts its items per conversation. |
| Worker counts group `proposed` and `accepted` as waiting, `running` and `cancel-requested` as running, `failed` and `uncertain` as failed | The sidebar needs activity, not the full task state machine; the row also carries the newest root's state and the last turn's state unrenamed. |
| The remembered lead is read with a separate one-row query | Paging must not change which lead the space remembers. |
| `lastResultAt` is the newest `terminal_at` across tasks and turns | The desktop keeps its own "seen" mark against it instead of daemon read state. |
| `#[allow(clippy::too_many_lines)]` on the daemon story test | One user session over Home, a project, and the launcher, as the repository's other story tests are. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/workspace-read-models` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean with the one story-test allowance. |
| `cargo test --workspace --locked` | 0 | One invocation, 399 passed, 0 failed, 11 ignored (explicit live-provider tests), 69 test binaries. New: `lazoo-domain` 3 (the `workspace` test binary), `lazooctl` 1, `lazood` 2 (the workspace story and the acceptance-script contract; 111 daemon tests). |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `scripts\acceptance\workspace-read-models.ps1` | 0 | PASS schema-15 manifest, `deterministic-fake-transport`, 7 commands, every evidence flag true. |
| every other acceptance script | not re-run | The harness stopped the full gate for host memory pressure right after the workspace tests and the release build completed, and forbade an unattended restart. All 27 passed on the base branch (`feat/core-scenario`, pull request #17) minutes earlier, and this slice changes no code they exercise beyond two new daemon routes. Re-run before merge: every `scriptscceptance\*.ps1` except `provider-adapters` and `desktop-shell`, the real-binary ones with `-SkipBuild` and `native-session-ownership -SkipBuild -Provider codex`. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI).

## Workspace-read-models acceptance evidence

`artifacts/acceptance/workspace-read-models-manifest.json` records literal `PASS`, schema 15, and `deterministic-fake-transport`. Its focused commands prove:

- the marker rule prefers attention, then activity (running workers, a turn in flight, waiting workers), then failure (the plan, then the last turn), then completion, with Lazoo's words for each, and nothing otherwise; titles come from the store, then the first request's first line bounded to 80 bytes, then "New work"; startability follows roster availability with a block and Lazoo's words for disabled, starting, offline, unverified, and failed, and the limit is 1 to 500;
- `lazooctl workspace conversations|launcher` map to the two methods with and without a project;
- through the daemon, Home and a registered project are separate spaces that remember different leads; a conversation starts untitled and unmarked, derives its title from the lead's first request, needs attention while its proposal waits, is active while the accepted task waits to run, completes when the result reaches the lead with `lastResultAt` set, and keeps a stored title once renamed; a one-row page is truncated with a cursor that reaches the next row; a zero limit, a bad cursor, and an unknown project are refused with stable codes; the launcher offers both employees as startable with Rhea remembered for Home and Mira for the project, blocks Mira once disabled with "Mira is disabled" and says so for the project's remembered lead, and names the two start requests with `activateIfNeeded: true`; no event names the workspace and the desktop snapshot is unchanged; and
- schema is 15 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, migration, event, or collaboration tool changed; the daemon gains `workspace.conversations` and `workspace.launcher`, and `lazooctl` gains `workspace conversations|launcher`.
- The workspace stores nothing and mutates nothing; the start request is data for two existing RPCs.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

`workspace.conversation` (the lead header, worker dock, and embedded attention; phase 2 of the contract), the desktop binding of all three, and everything else NEXT-SESSION lists remain separate reviewed slices.
