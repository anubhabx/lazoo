# Desktop Inbox verification

Date completed: 2026-10-13 IST

Branch: `feat/desktop-inbox`

Base: `main` at `eff4279` (pull request #9 merged)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: the Inbox protocol types, the domain derivation rule, the two store reads, the daemon's `inbox.list` and `inbox.counts`, `lazooctl inbox list|counts`, acceptance harness, and continuity documentation. The desktop app, Tauri surface, `desktop.snapshot`, and collaboration tools are unchanged.

## Outcome

PASS.

The daemon now serves the desktop Inbox read model that `docs/superpowers/specs/2026-10-11-desktop-inbox-contract-design.md` proposed, with section 9's proposed answers adopted as defaults on the owner's direction to finish as much as possible. `inbox.list { filter, limit?, before? }` derives, on every call, what needs the user from the approval, task, and incident ledgers: a pending approval, a `proposed` task, an `uncertain` task, and an open incident escalated to the user are `action-required`; an open incident in a lead's remediation window is `watching` with its deadline; an acknowledged escalation is visible but uncounted; a decided approval, a terminal task that was once `proposed` or `uncertain`, and a resolved or remediated escalation are the `resolved` archive, newest change first, paged with an opaque (`updatedAt`, id) cursor. `inbox.counts {}` is the rail badge: `unresolved` counts only `action-required` items. Every item carries its source row's `id`, `revision`, and `state`, Lazoo's own `title` and `requiredAction`, a deep-link `scope`, kind-specific `detail`, and its applicable `actions` as data naming the existing revision-checked RPC with the `expectedRevision` the user saw. The result names the event high-water mark the desktop should subscribe from. Nothing is stored, no event or mutation was added, and no collaboration tool can read the Inbox.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| Section 9's proposed answers adopted as defaults | The owner asked to finish as much as possible; each answer is a rule in `lazoo_domain::inbox`, so the UI owner can reopen any of them as a code change, not a schema or contract break. |
| `actions` emitted as data in phase 1 | The daemon grants nothing by naming an RPC the CLI already serves; emitting them now means phase 2 needs only the desktop capability review. |
| `resolve-failed` carries the task's own `failureCode` and `failureMessage` (or Lazoo's words) | `task.resolve` refuses `failed` without both, so the action must be sendable as emitted. |
| The whole rule is pure in `lazoo-domain`; the store only reads rows under one lock | Keeps the derivation testable without a database and keeps `eventHighWater` consistent with the rows it reflects. |
| Archive tasks are terminal tasks with a `proposed` or `uncertain` evidence row | The task table cannot say what a task once was; the evidence ledger can, without a new column. |
| Each unresolved source is capped at 1000 rows; archive timestamps compare as strings | Marked `ponytail:` in the code. An Inbox past that needs paging upstream; a millisecond and a second-precision stamp from two tables can swap archive neighbours, which the user can live with. |
| Counts always describe the unresolved set, even on the `resolved` filter | The badge must not change because the user opened the archive. |
| `#[allow(clippy::too_many_lines)]` on the two daemon story tests | Each is one user session read top to bottom, as the repository's other story tests are. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/desktop-inbox` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean with the two story-test allowances. |
| `cargo test --workspace --locked` | 0 | TODO_WORKSPACE_TOTALS |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `scripts\acceptance\desktop-inbox.ps1` | 0 | PASS schema-15 manifest, `deterministic-fake-transport`, 12 commands, every evidence flag true. |
| `configurable-thresholds`, `message-priority`, `message-arrival`, `user-notifications`, `incident-thresholds`, `lead-remediation`, `idle-suspension`, `activation-policy`, `coalesced-notices`, `peer-messaging`, `result-delivery`, `minimum-incidents`, `collaboration-tools`, `approval-gate`, `automatic-task-dispatch` | 0 | PASS schema-15 manifests. |
| `first-slice`, `employee-roster`, `collaboration-bus`, `conversation-delivery`, `task-delegation-graph`, each with `-SkipBuild` | 0 | PASS manifests against the real release binaries, with the schema-15 migration in the real daemon. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | TODO_NATIVE_EXIT | TODO_NATIVE_EVIDENCE |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI). `npm --prefix apps\lazoo-desktop\ui run check` was not run because the frontend is unchanged.

## Desktop-inbox acceptance evidence

`artifacts/acceptance/desktop-inbox-manifest.json` records literal `PASS`, schema 15, and `deterministic-fake-transport`. Its focused commands prove:

- the Inbox contracts are strict (`filter` required, unknown fields refused, `limit` defaults to 200), and items, counts, and results serialize with camel-case keys and kebab-case values;
- a pending approval is `action-required` with an `expires` deadline and `approve`/`decline` actions naming `approval.decide` with its revision, and a decided one is the archive with no actions; a `proposed` task is a counted `normal` proposal with `task.accept` and a user `task.cancel`, an `uncertain` task is a counted `priority` item with three `task.resolve` actions (`failed` carrying the task's failure), accepted and running tasks are not items, and a terminal task once `uncertain` or `proposed` is the archive under that kind; only escalated incidents are items, an open user escalation counts with `incident.acknowledge` and `incident.resolve`, a lead window is `watching` with a `remediation` deadline and no actions, an acknowledged escalation keeps only `resolve`, and resolved or remediated escalations are the archive;
- counts, filters, and urgency order follow attention, then deadline, then age, and the archive orders newest change first; cursors round-trip and are refused when malformed, and the limit is bounded to 1 to 500;
- `lazooctl inbox list|counts` maps to the two methods with the five filters;
- through the daemon, a pending approval is the only counted item, its scope names the employee, provider, and conversation, its action's params plus a fresh idempotency key decide it, the archive then holds it with no actions, the high-water mark is the store's own, no Inbox event exists, and the desktop snapshot is unchanged; parameters outside the contract are `LZ-INBOX-INVALID` and `inbox.counts` refuses parameters;
- through the daemon, proposals, uncertain tasks, and escalated incidents are derived from their rows, counted items order first, a lead's window is watched and then counted when the sweep closes it without a remedy, an acknowledged escalation stays visible but uncounted, every action removes its item through the RPC it names, the archive pages newest first with the opaque cursor, and no tool in a session's catalog names the Inbox; and
- schema is 15 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, migration, event type, or collaboration tool changed; the daemon gains `inbox.list` and `inbox.counts`, and `lazooctl` gains `inbox list|counts`.
- The Inbox stores nothing and mutates nothing; every action is an existing revision-checked RPC with the user as actor.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld; the implemented Inbox shape waits for the UI owner's review.

## Retained follow-up boundaries

The desktop binding of the Inbox (the UI owner's work), a typed desktop conversation and task contract, the live Codex gate, and everything else NEXT-SESSION lists remain separate reviewed slices.
