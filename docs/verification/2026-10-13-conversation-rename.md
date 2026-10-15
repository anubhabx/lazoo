# Conversation rename verification

Date completed: 2026-10-13 IST

Branch: `feat/conversation-rename`

Base: `feat/resource-budget` (pull request #14, stacked on #13, #12, and #11)

Product version: `0.1.0`

Database schema: 15 (no migration)

Scope: `RenameConversationParams`, `Store::rename_conversation` with `StoreError::ConversationStale`, the daemon's `conversation.rename` and its `LZ-CONVERSATION-STALE` mapping, `lazooctl conversation rename`, acceptance harness, and continuity documentation. The desktop app, Tauri surface, `desktop.snapshot`, and collaboration tools are unchanged.

## Outcome

PASS.

The information architecture (section 6.3) makes conversation titles user-editable; the ledger stored a title since schema 5 but nothing could change it. `conversation.rename { id, expectedRevision, title | null }` now sets or clears it. The same title again changes nothing and emits no event, whatever revision is cited; a different title needs the current revision, advances it, touches `updatedAt`, and emits `conversation.renamed` with the previous title. `LZ-CONVERSATION-STALE` carries the current revision; `expectedRevision: 0`, unknown fields, and an over-long title are `LZ-CONVERSATION-INVALID`; an unknown conversation is `LZ-CONVERSATION-NOT-FOUND`. `lazooctl conversation rename <id> --expected-revision N [--title T]` maps to it; omitting `--title` clears.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| Natural idempotency (same title is a no-op) instead of a command row | `conversation_commands` is constrained to the `conversation.create` scope; widening it is a migration for a write whose replay is already harmless. |
| The revision is checked only when the title would change | A replay of a successful rename cites the old revision and must succeed as a no-op; a conflicting rename must not win. |
| Blank titles clear | The same trim-and-empty rule `conversation.create` uses, so there is one notion of "no title". |
| `previousTitle` in the event | The audit trail says what the user replaced without a second read. |

## Fresh verification matrix

All commands were run from `C:\workspace\Orchestrator` on branch `feat/conversation-rename` after the final change.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test --workspace --locked` | 0 | One invocation, 386 passed, 0 failed, 11 ignored (explicit live-provider tests), 67 test binaries. New: `lazoo-store` 1, `lazooctl` 1, `lazood` 2 (the rename story and the acceptance-script contract; 105 daemon tests, counting the resource-budget contract test from pull request #14). |
| `cargo build --workspace --release --locked` | 0 | Locked release build. |
| `scripts\acceptance\conversation-rename.ps1` | 0 | PASS schema-15 manifest, `deterministic-fake-transport`, 5 commands, every evidence flag true. |
| `resource-accounting`, `desktop-inbox`, `configurable-thresholds`, `message-priority`, `message-arrival`, `user-notifications`, `incident-thresholds`, `lead-remediation`, `idle-suspension`, `activation-policy`, `coalesced-notices`, `peer-messaging`, `result-delivery`, `minimum-incidents`, `collaboration-tools`, `approval-gate`, `automatic-task-dispatch` | 0 | PASS schema-15 manifests. |
| `first-slice`, `employee-roster`, `collaboration-bus`, `conversation-delivery`, `task-delegation-graph`, each with `-SkipBuild` | 0 | PASS manifests against the real release binaries, with the schema-15 migration in the real daemon. |
| `scripts\acceptance\native-session-ownership.ps1 -SkipBuild -Provider codex` | 0 | PASS manifest on the first run, straight after `resource-budget`; real Codex CLI, non-model session contract only. |
| `scripts\acceptance\resource-budget.ps1 -SkipBuild` | 0 | PASS manifest: idle CPU share 0.000 (0 ms), routing 1900 calls at CPU share 0.0021, both working sets under 350 MiB, no provider session. |
| `git diff --check` | 0 | No whitespace errors. |

Not run, as in every prior backend slice: `provider-adapters.ps1 -Provider codex` (live model turn) and `desktop-shell.ps1` (unchanged, separately owned UI). `npm --prefix apps\lazoo-desktop\ui run check` was not run because the frontend is unchanged.

## Conversation-rename acceptance evidence

`artifacts/acceptance/conversation-rename-manifest.json` records literal `PASS`, schema 15, and `deterministic-fake-transport`. Its focused commands prove:

- in the store, a rename replaces the title at the revision the user saw and advances it with an event; the same title again returns the record unchanged with no event whatever revision is cited; a different title at a stale revision is `ConversationStale` with the current revision and writes nothing; a blank title clears; an over-long title and an unknown conversation are refused before any write; and the events are `conversation.created` then two `conversation.renamed`;
- `lazooctl conversation rename` maps with and without `--title` and requires `--expected-revision`;
- through the daemon, an untitled conversation is renamed, replayed without an event, refused at a stale revision with the current one, refused for a zero revision, an unknown field, an over-long title, and an unknown id with stable codes, carried by `conversation.list` and `conversation.show`, cleared with `title: null`, and the desktop snapshot is unchanged; and
- schema is 15 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --check` passed; both diffs under `apps/lazoo-desktop` are empty.
- No Tauri command, capability, `desktop.snapshot` field, frontend source, migration, or collaboration tool changed; the daemon gains `conversation.rename` and `lazooctl conversation` gains `rename`.
- Only the local user renames; a rename changes no turn, task, message, lead, or project.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

Deriving a title from the first user turn and the desktop's rename action belong to the workspace contract (pull request #13); everything else NEXT-SESSION lists remains a separate reviewed slice.
