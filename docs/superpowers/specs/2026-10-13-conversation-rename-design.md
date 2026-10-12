# Conversation Rename Design

**Date:** 2026-10-13

**Status:** Implemented on `feat/conversation-rename` (stacked on `feat/resource-budget`, pull request #14) with deterministic acceptance, for owner review in a pull request before merge.

**Scope:** Backend-only, no schema change. One mutation, `conversation.rename`, and `lazooctl conversation rename`. No desktop app, Tauri, `desktop.snapshot`, or collaboration-tool change; no live model use.

## 1. Outcome

The owner-approved information architecture says conversation titles "are generated from the first request, are user-editable, and remain stable after editing" (section 6.3). The ledger stored a title since schema 5 but nothing could change it after `conversation.create`. The user can now set or clear a title:

- `lazooctl conversation rename <id> --expected-revision N --title "Parser fix"`;
- `lazooctl conversation rename <id> --expected-revision N` clears it, so a derived title (workspace contract, section 4.1) shows again.

## 2. Rule

`Store::rename_conversation` validates the title with the same rule as `conversation.create` (trimmed, empty clears, at most `MAX_CONVERSATION_TITLE_BYTES`), reads the conversation inside one immediate transaction, and:

- when the stored title already equals the requested one, commits nothing, returns the record, and emits no event, whatever revision was cited (naturally idempotent, as the threshold writes are);
- otherwise requires `expected_revision` to be the current revision (`LZ-CONVERSATION-STALE` carries the current one), writes the title, advances the revision, touches `updated_at`, and emits `conversation.renamed` with the record and the previous title.

The daemon refuses `expectedRevision: 0` and unknown fields with `LZ-CONVERSATION-INVALID`; an unknown conversation is `LZ-CONVERSATION-NOT-FOUND`.

## 3. Rules kept

- Only the local user renames, through the daemon and CLI; no collaboration tool can.
- A rename changes nothing else: turns, tasks, messages, the lead, and the project are untouched, and the conversation's `sequence` and `createdAt` are stable.
- `conversation.list` and `conversation.show` carry the new title and revision; `desktop.snapshot` is unchanged.
- No idempotency key or command row: the same-title no-op makes a replay harmless, and `conversation_commands` keeps its `conversation.create` scope without a migration.

## 4. Deterministic acceptance

`scripts/acceptance/conversation-rename.ps1` runs the store, CLI, and daemon proofs and writes `artifacts/acceptance/conversation-rename-manifest.json`. No provider smoke or live model turn.

## 5. Explicitly deferred work

- deriving a title from the first user turn and the desktop's rename action (the workspace contract, pull request #13);
- renaming from a collaboration tool (not wanted: titles are the user's).

## 6. Review gate

Merge authorizes only the one mutation, its CLI command, and the rule above.
