# Next session: continue the backend before resuming desktop implementation

## Current owner verdict

The owner approved the target Lazoo desktop information architecture, then explicitly shelved UI implementation until the backend is complete.

No UI or frontend work should resume while this backend-first direction is active. The current read-only desktop remains the technically verified baseline. The approved target structure remains documented in `docs/superpowers/specs/2026-08-30-lazoo-desktop-information-architecture-design.md`.

## Current backend state

- The daemon, keeper, CLI, SQLite WAL store, transactional outbox, authenticated named-pipe IPC, project registration, restart recovery, emergency spool, and read-only desktop snapshot are implemented.
- Native adapters for Codex, OpenCode, Grok Build, Antigravity, and Claude Code are implemented behind a versioned provider contract.
- Codex, OpenCode, Grok Build, and Antigravity passed fresh model-backed and resume/continuation gates.
- Claude Code passes non-model readiness checks but remains blocked before model execution by the configured organization's access policy.
- Employee records, revisioned roster state, persistent native-session ownership, Job Objects, ConPTY, worktree automation, task/message delivery, policy approvals, and the resource governor remain unimplemented.

## Next-session objective

Implement the next narrow backend vertical slice: durable employee configuration plus a revisioned live roster derived from truthful provider evidence.

The slice should establish:

1. strict employee identity, provider binding, enabled state, role/instructions, model/native settings, checkout policy, and authority-mode contracts;
2. transactional employee create/update/disable persistence with immutable audit and outbox events;
3. a monotonic roster revision that changes only when effective roster state changes;
4. availability derived from employee configuration and current provider probe truth, without starting a model session;
5. daemon and CLI commands for employee create/list/show/update/disable and roster inspection;
6. restart durability, idempotency, stale-revision rejection, redaction, and offline-provider acceptance evidence.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Keep UI and `desktop.snapshot` unchanged.
- Do not start provider model sessions from list or roster commands.
- Do not claim Job Object, ConPTY, native terminal, conversation, delegation, worktree automation, approval, or peer-message support in this slice.
- Provider availability must remain evidence-based: discovery or a non-model handshake is not model support.
- Ask remains the default authority mode; new editing employees default to isolated checkout policy.
- Employee and roster mutations must write domain state, audit event, and outbox record transactionally.
- Preserve tombstoned identity and history when an employee is disabled or later removed; never silently erase prior references.
