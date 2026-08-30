# Next session: continue the backend before resuming desktop implementation

## Current owner verdict

The owner approved the target Lazoo desktop information architecture, then explicitly shelved UI implementation until the backend is complete.

No UI or frontend work should resume while this backend-first direction is active. The current read-only desktop remains the technically verified baseline. The approved target structure remains documented in `docs/superpowers/specs/2026-08-30-lazoo-desktop-information-architecture-design.md`.

## Current backend state

- The daemon, keeper, CLI, SQLite WAL store, transactional outbox, authenticated named-pipe IPC, project registration, restart recovery, emergency spool, and read-only desktop snapshot are implemented.
- Native adapters for Codex, OpenCode, Grok Build, Antigravity, and Claude Code are implemented behind a versioned provider contract.
- Codex, OpenCode, Grok Build, and Antigravity passed fresh model-backed and resume/continuation gates.
- Claude Code passes non-model readiness checks but remains blocked before model execution by the configured organization's access policy.
- Durable employee records and revisioned roster state are implemented at database schema version 2. Create/update/disable mutations are idempotent and revision-checked, provider observations are safe and non-model, disabled identities survive restart, and employee/roster fields remain outside `desktop.snapshot`.
- Persistent native-session ownership, Job Objects, ConPTY, worktree automation, task/message delivery, policy approvals, and the resource governor remain unimplemented.

## Next-session objective

Design and implement the next narrow backend vertical slice: persistent native employee-session ownership with Windows Job Object containment and restart reconciliation. Keep it daemon/CLI-only; ConPTY terminal streaming may be included only if the process-ownership contract is already proven in the same slice.

The slice should establish:

1. durable desired/observed session records bound to an existing employee and provider capability contract;
2. one Windows Job Object per owned native employee process tree, with kill-on-close and explicit resource/accounting boundaries;
3. idempotent start/stop/reconcile commands with stable errors and immutable audit/outbox events;
4. daemon-restart reconciliation that never adopts an unrelated process and truthfully marks lost or incompatible sessions;
5. bounded graceful-stop, forced-stop, unexpected-exit, and crash-recovery behavior;
6. isolated acceptance evidence for containment, restart reconciliation, stale-command rejection, and zero UI capability expansion.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Keep UI and `desktop.snapshot` unchanged.
- Do not start provider model sessions from employee list or roster commands.
- Do not claim ConPTY, native terminal interaction, conversation, delegation, worktree automation, approval, or peer-message support until each is implemented and accepted.
- Provider availability must remain evidence-based: discovery or a non-model handshake is not model support.
- Ask remains the default authority mode; new editing employees default to isolated checkout policy.
- Session mutations must write domain state, audit event, and outbox record transactionally.
- Preserve employee identity and history; process cleanup must never delete employee records or unrelated native state.
