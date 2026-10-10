# Next session: post-idle-suspension backend boundary

## Current owner boundary

Backend orchestration remains the active scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities without a separate reviewed desktop contract.

This handoff does not authorize ConPTY, terminal I/O, session-wide approval grants, per-capability policy, managed memory, resource governance, scheduling, other-provider task delivery, approval bridging, or tool surfaces, any activation trigger other than a recorded notice turn for an employee the user set to `notice-turns`, any suspension trigger other than the employee's own idle timeout, cross-tree or user-to-employee messaging through the tools, bounded remediation, user notification channels, provider smoke, or a live model turn.

## Completed backend baseline

- Schema is 11: `employees.idle_timeout_seconds` (0, never, by default; otherwise 60 to 86400) sits beside `activation_policy`, is exposed on `EmployeeRecord`, `employee.create`, `employee.update`, and `lazooctl employee create/update --idle-timeout-seconds`, and changes like every other employee setting: revisioned, refused while the employee has an active session, and stale for any session started under the old revision. Every employee written before schema 11 reads 0.
- The daemon's idle sweep runs every 15 seconds. An idle session owned by this boot whose employee's timeout has passed since the session started or a turn on it last changed, with no submitting or accepted turn on it, is stopped through the ordinary stop path under the `session.stop` key `idle-suspend:<sessionId>`; the store re-checks due-ness in the same transaction that marks it `stopping`. A failed provider stop fails the session with an `LZ-SESSION-STOP-FAILED` incident and is never retried. The sweep never starts a session.
- Delivery of a recorded turn resolves the recipient's current idle session from the roster instead of the session named when the turn was recorded, so a turn is no longer stranded on a session that a user stop or idle suspension ended.
- Suspension ends the native thread. The next user turn activates the employee with a fresh session; a `notice-turns` employee is woken by its next notice turn exactly once and stopped after it; a `manual` employee waits for the user.
- The activation-policy (`c8b9c8f`), coalesced-notices (`2c28a0e`), peer-messaging (`176979c`), result-delivery (`b320946`), minimum-incidents (`762aec2`, `24a851a`), and collaboration-tools (`d72de10`) slices underneath are unchanged: eight tools, the same refusal codes, the same `task_wait` and `message_send` inbox, one notice turn per recipient batch, one policy activation per recorded turn. Every acceptance script now asserts schema 11.
- The desktop snapshot and every file under `apps/lazoo-desktop` remain unchanged.
- The repository lives at `github.com/anubhabx/lazoo`; every slice lands as a pull request against `main`.

## Deterministic acceptance

`scripts/acceptance/idle-suspension.ps1` runs focused protocol, domain, store, CLI, and daemon fake-backend proofs and writes `artifacts/acceptance/idle-suspension-manifest.json`.

No provider smoke or live model turn is part of this slice. A live Codex gate (a real model calling the eight tools, asking a sibling a question through `message_send`, receiving several results as one notice turn after its turn ended, being suspended by its idle timeout, and being woken once by policy for that turn) remains the natural next authorized live proof and must be run explicitly.

The owner's laptop had a full C: drive during this slice's gate (23 GB in `%TEMP%\Adobe`); `cargo test --workspace --locked` in one invocation and `npm --prefix apps\lazoo-desktop\ui run check` should be re-run once there is room. See `docs/verification/2026-10-10-idle-suspension.md`.

## Next backend objective

Choose and design one separately reviewed slice:

- **Message-arrival notification**: replace the 250 ms polls inside `task_wait` and `message_send` with a store-change notify, keeping the same bounded waits and evidence.
- **Live Codex tool-call, peer-message, coalesced-notice, policy-activation, and idle-suspension gate**: one explicitly authorized model turn proving the eight tools, one peer exchange, one coalesced result turn, one idle suspension, and one policy activation end to end through the real app-server.
- **Bounded lead remediation**: when an incident is promoted, let the lead attempt at most two policy-approved remedies inside five minutes before the incident escalates to the user, as the brief's section 14.2 requires.

Do not combine these boundaries implicitly. User notification channels, cross-tree and user-to-employee messaging through the tools, ConPTY, typed desktop Inbox binding (tasks, approvals, incidents, notices, messages), a deadline-driven idle sweep, and ACP approval bridging remain available candidates behind those.

Do not emit `UI BACKEND CONTRACT: GO` yet. Approval, dispatch, task, tool, incident, notice, message, activation-policy, and idle-timeout read models exist in the daemon, but the typed desktop-facing contract has not been separately reviewed.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the durable record-first ledgers and native provider evidence.
- A notice or peer message is derived evidence: the task, message, and incident rows it points at stay authoritative, and neither ever retries work, restarts anything, re-authenticates, changes authority, or deletes data.
- Never merge, summarise, or truncate one notice into another; a coalesced turn carries each notice whole and each keeps its own message row and evidence.
- Never let employee-authored traffic start a provider process on its own. Only a recorded notice turn for an employee the user set to `notice-turns` may, once per turn, on that employee's own task, and the session stops with the turn; everyone else waits for the local user. No tool can read or set the policy.
- Never stop a session that has an unfinished turn on it, and never stop a session for idleness unless its own employee's timeout, set by the user, has passed. No tool can read or set the idle timeout.
- A notice or peer turn runs under the recipient task's effective authority; governed actions inside it still flow through the schema-8 approval gate, and neither a tool result nor a peer message is ever an approval.
- Never let a tool name a different actor, widen authority, or bypass a store guard; the daemon fixes the actor to the turn's recipient endpoint.
- Never deliver a peer message to a task that has not started, never let a peer-only turn complete a task, and never reuse a closed notice turn.
- Never declare tools on a resumed Codex thread; every activation is a fresh `thread/start`, and the adapter fails closed until Codex accepts `dynamicTools` on `thread/resume`.
- Never retry an activation, a suspension stop, an uncertain dispatch, a failed notice or peer turn, or an expired approval automatically.
- Never delete or rewrite a retained task worktree without an explicit cleanup contract.
- Keep UI, ConPTY, session-wide grants, managed memory, scheduling, resource governance, other-provider expansion, and live model use outside this completed slice.
