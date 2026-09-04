# Next session: automatic task dispatch and project isolation

## Current owner boundary

Backend orchestration remains the active scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities while designing the next backend sequence.

The approved desktop information architecture remains in `docs/superpowers/specs/2026-08-30-lazoo-desktop-information-architecture-design.md`. This handoff does not authorize ConPTY, terminal I/O, approval workflows, managed memory, resource governance, scheduling, or other-provider conversation delivery.

## Completed backend baseline

- Database schema 6 stores durable employees, provider observations, native employee sessions, actor-aware collaboration envelopes, conversations, turns, task ancestry, append-only task/turn evidence, delivery attempts, dead letters, command outcomes, audit events, and outbox rows.
- Every new user-authored `turn.submit` atomically creates one accepted root task whose task ID equals its creation-turn ID. Existing-task follow-ups reuse the root and do not create a second tree.
- `task.delegate/accept/start/report/cancel/resolve/list/show` are available through the daemon and CLI. Stable task failures use `LZ-TASK-*` and CLI exit code 13.
- Delegation is an immutable tree bounded to depth 4 and eight nonterminal direct children. `ask` creates a proposal without a message or provider action; user acceptance creates one canonical delegation message. Non-ask delegation remains bounded by the most restrictive parent, request, and assignee authority.
- A new root reserves its creation turn so a later follow-up cannot overtake initial delivery; that reservation is not `running` evidence. Accepted delegated children remain queued without an active turn. Provider evidence projects task state and ordered evidence atomically, and completed turns release execution ownership. Active descendants block parent completion and cancellation.
- Task-backed Codex turns receive a per-turn permission ceiling derived from effective task authority. Unsupported providers fail closed instead of running with broader permissions.
- Restart reconciliation runs sessions, messages, turns, then tasks. Prior-boot running or cancel-requested tasks become `uncertain` with `LZ-TASK-OWNER-LOST`; user resolution is append-only and idempotent.
- Accepted tasks are queued control-plane truth only. Lazoo does not automatically dispatch a canonical delegation message to a worker provider in this slice.
- The full deterministic matrix is green: 234 workspace tests passed, 0 failed, and 11 live-only tests were ignored; format, Clippy, all-target check, locked release build, and all six backend acceptance manifests passed.

## Fresh deterministic acceptance

- `first-slice-manifest.json`: PASS, 10,544 ms, 14 commands, database schema 6.
- `employee-roster-manifest.json`: PASS, 7,648 ms, 17 commands, schema 6.
- `native-session-ownership-manifest.json`: PASS, 14,483 ms, 18 commands, schema 6.
- `collaboration-bus-manifest.json`: PASS, 6,980 ms, 26 commands, schema 6.
- `conversation-delivery-manifest.json`: PASS, 24,937 ms, 38 commands, schema 6.
- `task-delegation-graph-manifest.json`: PASS, 11,255 ms, 81 commands, schema 6.

The task manifest proves root identity and replay, follow-up linkage, ask proposal/acceptance, canonical delegation messages, authority bounds, stale/unavailable/self/depth/fan-out guards, native evidence, ordered outcomes, descendant blocking, owner-loss reconciliation, idempotent uncertainty resolution, audit/outbox correlation, and all declared exclusions. Expected nonzero CLI exits in the manifests are asserted negative-path evidence, not gate failures.

## Historical live evidence

The previously authorized Codex live gate remains valid historical evidence for the schema-5 conversation slice: exact output `LAZOO_TURN_OK`, durable native thread/turn identity, acknowledgement after native acceptance, completed local turn, and clean provider-tree shutdown. No provider smoke or live model turn was run for the task-graph slice.

## Next backend objective

Design automatic dispatch for accepted tasks together with the project/worktree isolation contract it depends on. The next slice must define, before implementation:

- when a queued accepted task becomes eligible for provider delivery;
- how projectless work differs from repository work;
- how isolated worktrees are created, owned, retained, recovered, and surfaced without touching dirty user state;
- how canonical delegation-message delivery and task start evidence remain atomic and idempotent;
- how authority, provider capability, roster freshness, session ownership, cancellation, and restart recovery fail closed; and
- which daemon/CLI contracts are stable enough for a later typed desktop binding.

Do not emit `UI BACKEND CONTRACT: GO` yet. Automatic dispatch, worktree ownership, and the stable desktop-facing task contract are not implemented.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the durable task, message, conversation, turn, employee, roster, and session ledgers.
- Treat `accepted` as queued control-plane state, never as proof that a provider ran.
- Require native acceptance or durable progress before `running`.
- Never create or mutate a Git worktree until a reviewed isolation/cleanup contract exists.
- Keep UI, ConPTY, approvals, managed memory, scheduling, resource governance, and unrelated provider expansion outside the next slice.
