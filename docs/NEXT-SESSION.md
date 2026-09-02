# Next session: durable backend collaboration bus

## Current owner boundary

Backend and communication layers remain this branch's scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities while continuing this backend sequence.

The approved desktop information architecture remains documented in `docs/superpowers/specs/2026-08-30-lazoo-desktop-information-architecture-design.md`. Backend contracts must become truthful before a UI consumes them, but this handoff does not prescribe or implement UI behavior.

## Current backend state

- The daemon, keeper, CLI, SQLite WAL store, transactional outbox, authenticated named-pipe IPC, project registration, restart recovery, and emergency spool are implemented.
- Native adapters for Codex, OpenCode, Grok Build, Antigravity, and Claude Code are implemented behind a versioned provider contract.
- The last recorded model-backed gates passed for Codex, OpenCode, Grok Build, and Antigravity. Claude Code's last model gate was blocked by organization policy; refresh that evidence before claiming current live support.
- Database schema 3 stores durable employees, provider observations, revisioned roster state, and native employee-session history.
- `session.warm`, `session.list`, `session.show`, and `session.stop` are available through the daemon and CLI. Warm starts an idle provider-native session without sending a model prompt.
- Every owned provider tree is placed in a dedicated kill-on-close Windows Job Object before provider launch. Warm/stop are idempotent and revision-checked; forced daemon loss removes the tree, and the next boot records `LZ-SESSION-OWNER-LOST` without PID adoption.
- The native-session acceptance manifest records creation-time containment, complete-tree termination, real Codex warm/stop, native host PID evidence, stale-stop rejection, bounded stop, durable suspension, daemon-loss cleanup, and owner-loss reconciliation.
- ConPTY, conversation turns, the canonical task/message bus, delivery acknowledgements, worktree automation, policy approvals, resource governance, and managed memory remain unimplemented.

## Next-session objective

Implement the next narrow backend vertical slice: the durable canonical collaboration bus. Keep it daemon/CLI-only and stop before provider turn delivery.

The slice should establish:

1. immutable versioned message envelopes with sender, recipient, correlation, causation, project, task, session, message kind, body/reference metadata, and idempotency identity;
2. transactional record-first send semantics that write the envelope, audit event, and outbox row before any delivery attempt;
3. explicit queued, delivering, acknowledged, retryable-failed, terminal-failed, and cancelled states with monotonic revisions;
4. idempotent send, acknowledge, fail, retry, list, and show commands with stable `LZ-MESSAGE-*` errors;
5. bounded retry scheduling and dead-letter evidence without claiming that bytes written or an accepted native request equals delivery;
6. recipient activation intent that can reference a durable employee/session but does not yet start a model turn or inject a message into a provider;
7. backend-only acceptance evidence for replay, ordering, acknowledgement correlation, retry exhaustion, restart durability, and zero desktop capability expansion.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Record every collaboration envelope durably before using any future provider-native peer-messaging optimization.
- Delivery is at least once; every delivery command and recipient-side acknowledgement must be idempotent by message key.
- Never call a message delivered merely because stdin accepted bytes, an RPC returned success, or an asynchronous provider session was accepted. Later provider delivery must require acknowledgement or session progress evidence.
- Keep UI, `desktop.snapshot`, Tauri permissions, ConPTY, provider turns, worktree mutation, and model quota outside this slice.
- Preserve provider-native authentication, tools, plugins, MCP, skills, permissions, terminals, and session semantics.
- Ask remains the default authority mode. No model may grant user approval or broaden its own authority.
- Do not run `provider smoke` as part of collaboration-bus acceptance.
