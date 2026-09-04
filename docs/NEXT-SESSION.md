# Next session: durable task and delegation graph design

## Current owner boundary

Backend and communication layers remain the active scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities while designing the next backend sequence.

The approved desktop information architecture remains in `docs/superpowers/specs/2026-08-30-lazoo-desktop-information-architecture-design.md`. This handoff does not authorize ConPTY, worktree mutation, task execution, policy expansion, managed memory, resource governance, or other-provider conversation delivery.

## Completed backend baseline

- Database schema 5 stores durable employees, provider observations, native employee sessions, actor-aware collaboration envelopes, projectless or project-scoped conversations, immutable turns, append-only normalized provider events, delivery attempts, dead letters, command outcomes, audit events, and outbox rows.
- `conversation.create/list/show` and `turn.submit/retry/cancel/list/show` are available through the daemon and CLI. Conversation/turn failures use CLI exit code 12.
- User-authored turns may explicitly activate the selected lead. Employee-authored turns cannot activate dormant recipients. Daemon-managed conversation delivery is enabled only for Codex.
- Turn submission is record-first. Lazoo acknowledges the canonical message only after it durably records a native turn ID or progress evidence. Definite pre-acceptance failures follow bounded retry policy; accepted or uncertain work is never retried automatically.
- Native `completed`, `failed`, and `interrupted` outcomes map to `completed`, `terminal-failed`, and `cancelled`. Session-level events after acceptance no longer have to re-prove acceptance.
- Restart reconciliation leaves recorded queued work eligible for an idempotent replay, marks prior-boot submitting/accepted work uncertain with `LZ-TURN-OWNER-LOST`, and never adopts a prior provider PID or native turn.
- The deterministic conversation-delivery manifest is PASS with 32 commands, including terminal mapping, outer Codex context isolation, planned-shutdown provider-tree cleanup, and no model invocation.
- The full deterministic matrix is green: 196 workspace tests passed, 0 failed, 11 live-only tests ignored; format, Clippy, all-target check, locked release build, and all five backend acceptance manifests passed.

## Completed live evidence

Two pre-fix Codex attempts returned `LZ-TURN-UNCERTAIN` and were not claimed. They exposed outer desktop-task inheritance and an overbroad acceptance-evidence guard for normal session-level events after native acceptance. Both defects are fixed and covered by real-shaped deterministic regressions.

After fresh user authorization, the post-fix gate passed on Codex 0.153.1:

- `conversation-delivery-live-manifest.json`: PASS, 27,064 ms, 9 commands, schema 5;
- exact output: `LAZOO_TURN_OK`;
- native thread: `01a06bf9-d23a-7390-89ae-36f5d1c957e1`;
- native turn: `01a06bf9-d960-76e0-9dc5-24696592e886`;
- canonical message acknowledged only after native acceptance;
- local turn completed;
- planned daemon shutdown removed the owned provider tree; and
- isolated temporary data was removed.

The native Codex rollout independently records the same native turn, exact final output, token usage, and terminal task completion.

## Next-session objective

Write a separate reviewed design and implementation plan for the next narrow backend slice: a durable task/delegation graph with explicit lead and authority boundaries. Do not implement it until the design is reviewed.

The design must decide:

1. task identity, projectless/project scope, lifecycle states, immutable creation input, revision rules, and restart truth;
2. how one user-owned task maps to its lead conversation without creating a second prompt or message path;
3. delegation identity and ancestry, lead/worker roles, bounded depth/fan-out, correlation to canonical messages and turns, and idempotent assignment;
4. which actions a lead may perform inside user-granted scope and which always require the user; no employee may grant approval, expand authority, or alter its own policy;
5. durable accept/start/progress/complete/fail/cancel evidence and how owner loss or uncertain provider outcomes affect parent and child task truth;
6. deterministic acceptance coverage, stable errors, audit/outbox publication, and CLI-only inspection/mutation surfaces; and
7. explicit exclusions for worktree creation, ConPTY, terminal I/O, approval UI, resource scheduling, managed memory, other-provider delivery, and desktop capability expansion.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse employees, sessions, canonical collaboration envelopes, conversations, and turns; do not create parallel identity or delivery systems.
- Keep `ask` as the default authority mode. Delegation carries a bounded subset of user authority and never creates new authority.
- Treat task orchestration as durable control-plane truth, not as evidence that provider work executed.
- Require native acceptance or durable progress before marking provider-backed work started.
- Keep UI, worktrees, ConPTY, approvals, managed memory, resource governance, and unrelated provider expansion outside this design.
