# Next session: durable conversations and provider delivery

## Current owner boundary

Backend and communication layers remain this branch's scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities while continuing this backend sequence.

The approved desktop information architecture remains documented in `docs/superpowers/specs/2026-08-30-lazoo-desktop-information-architecture-design.md`. Backend contracts must become truthful before a UI consumes them, but this handoff does not prescribe or implement UI behavior.

## Current backend state

- The daemon, keeper, CLI, SQLite WAL store, transactional outbox, authenticated named-pipe IPC, project registration, restart recovery, and emergency spool are implemented.
- Native adapters for Codex, OpenCode, Grok Build, Antigravity, and Claude Code are implemented behind a versioned provider contract.
- The last recorded model-backed gates passed for Codex, OpenCode, Grok Build, and Antigravity. Claude Code's last model gate was blocked by organization policy; refresh that evidence before claiming current live support.
- Database schema 4 stores durable employees, provider observations, revisioned roster state, native employee-session history, immutable collaboration envelopes, delivery state, attempts, dead letters, and idempotent command outcomes.
- `session.warm`, `session.list`, `session.show`, and `session.stop` are available through the daemon and CLI. Warm starts an idle provider-native session without sending a model prompt.
- Every owned provider tree is placed in a dedicated kill-on-close Windows Job Object before provider launch. Forced daemon loss removes the tree, and the next boot records `LZ-SESSION-OWNER-LOST` without PID adoption.
- `message.send`, `message.claim`, `message.acknowledge`, `message.fail`, `message.retry`, `message.cancel`, `message.list`, and `message.show` are available through the daemon and CLI. Sends are record-first and ordered; mutations are idempotent and revision-checked.
- Message delivery states are `queued`, `delivering`, `acknowledged`, `retryable-failed`, `terminal-failed`, and `cancelled`. Retry deadlines are bounded, exhaustion creates durable dead-letter evidence, and uncertain attempts reconcile to `LZ-MESSAGE-OWNER-LOST` on restart.
- The collaboration-bus acceptance manifest records idempotent send, causal ordering, acknowledgement correlation, early-retry rejection, retry exhaustion, dead-letter evidence, cancellation, restart durability, owner-loss reconciliation, and audit/outbox publication.
- Message claim currently creates only a durable attempt. It does not activate a recipient, submit bytes to a provider, start a model turn, or claim delivery.
- ConPTY, conversation/turn persistence, provider turn delivery, task orchestration, worktree automation, policy approvals, resource governance, and managed memory remain unimplemented.

## Next-session objective

Implement the next narrow backend vertical slice: a durable conversation/turn ledger connected to one provider-delivery transport. Keep it daemon/CLI-only and stop before task orchestration, UI, or ConPTY.

The slice should establish:

1. durable versioned conversations and turns with user/employee authorship, employee/session ownership, projectless or project scope, correlation to collaboration messages, and immutable input content;
2. record-first turn submission so the conversation turn, message delivery intent, audit event, and outbox row exist before provider I/O;
3. a provider-neutral active-session operation that submits one turn through the provider's existing machine-oriented transport while preserving native authentication, tools, MCP, plugins, skills, permissions, and session identity;
4. recipient activation through the existing session lifecycle when policy permits, without conflating session startup with turn delivery;
5. acknowledgement only after a provider-native turn identifier or observable session-progress event is recorded; successful writes or accepted asynchronous requests alone are not delivery evidence;
6. idempotent submit/cancel/reconcile behavior with stable errors and durable uncertain-outcome evidence across daemon/provider loss;
7. deterministic fake-backend acceptance for every state transition plus a separately explicit, quota-consuming live-provider gate before claiming real model delivery;
8. zero desktop capability expansion and no task/worktree/approval implementation.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the canonical collaboration envelope; do not create a second cross-provider message path.
- Record turns and delivery intent before any provider operation.
- Delivery is at least once. Provider submission and recipient processing must be idempotent by logical message/turn identity wherever the native protocol permits.
- Never call a turn delivered merely because stdin accepted bytes, an RPC returned success, or an asynchronous provider session was accepted. Require a provider turn ID or observed progress evidence.
- Keep UI, `desktop.snapshot`, Tauri permissions, ConPTY, task delegation, worktree mutation, managed memory, and policy expansion outside this slice.
- Preserve provider-native authentication, tools, plugins, MCP, skills, permissions, terminals, and session semantics.
- Ask remains the default authority mode. No model may grant user approval or broaden its own authority.
- Do not hide a model-backed gate inside non-model acceptance; label quota-consuming verification explicitly.
