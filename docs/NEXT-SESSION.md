# Next session: post-activation-policy backend boundary

## Current owner boundary

Backend orchestration remains the active scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities without a separate reviewed desktop contract.

This handoff does not authorize ConPTY, terminal I/O, session-wide approval grants, per-capability policy, managed memory, resource governance, scheduling, other-provider task delivery, approval bridging, or tool surfaces, any activation trigger other than a recorded notice turn for an employee the user set to `notice-turns`, cross-tree or user-to-employee messaging through the tools, bounded remediation, user notification channels, provider smoke, or a live model turn.

## Completed backend baseline

- Schema is 10: `employees.activation_policy` (`manual` by default, `notice-turns` on request) sits beside `authority_mode` and `checkout_policy`, is exposed on `EmployeeRecord`, `employee.create`, `employee.update`, and `lazooctl employee create/update --activation-policy`, and changes like every other employee setting: revisioned, refused while the employee has an active session, and stale for any session started under the old revision. Every employee written before schema 10 reads `manual`.
- The daemon's notice pass, on the dormant-recipient case it already recognised (`LZ-TURN-SESSION-NOT-READY`), consults the recipient's policy. Under `notice-turns` it starts one session keyed `notice-activation:<turnId>`, delivers the recorded turn on the recipient's own task, and stops the session (`notice-activation:<turnId>:stop`), so the recipient is `suspended` again before the pass moves on. A replay of that key means the turn already used its one activation and waits for the user. A refused start fails the session, raises an `LZ-SESSION-START-FAILED` incident, and is never retried; the user's `session.warm` still delivers the same turn and the user's session is never stopped by a notice.
- Under `manual` nothing changed: employee-authored traffic never starts a provider process, a coalesced turn for a dormant recipient stays `recorded` with its notices `queued` until the local user resumes it, and employee-authored `turn.submit` with `activateIfNeeded` is still refused.
- The coalesced-notices (`2c28a0e`), peer-messaging (`176979c`), result-delivery (`b320946`), minimum-incidents (`762aec2`, `24a851a`), and collaboration-tools (`d72de10`) slices underneath are unchanged: eight tools, the same refusal codes, the same `task_wait` and `message_send` inbox, one notice turn per recipient batch, each notice with its own evidence. Every acceptance script now asserts schema 10.
- The desktop snapshot and every file under `apps/lazoo-desktop` remain unchanged.
- The repository lives at `github.com/anubhabx/lazoo`; every slice lands as a pull request against `main`.

## Deterministic acceptance

`scripts/acceptance/activation-policy.ps1` runs focused protocol, domain, store, CLI, and daemon fake-backend proofs and writes `artifacts/acceptance/activation-policy-manifest.json`.

No provider smoke or live model turn is part of this slice. A live Codex gate (a real model calling the eight tools, asking a sibling a question through `message_send`, receiving several results as one notice turn after its turn ended, and being woken once by policy for that turn) remains the natural next authorized live proof and must be run explicitly.

## Next backend objective

Choose and design one separately reviewed slice:

- **Message-arrival notification**: replace the 250 ms polls inside `task_wait` and `message_send` with a store-change notify, keeping the same bounded waits and evidence.
- **Live Codex tool-call, peer-message, coalesced-notice, and policy-activation gate**: one explicitly authorized model turn proving the eight tools, one peer exchange, one coalesced result turn, and one policy activation end to end through the real app-server.
- **Idle suspension**: a per-employee idle timeout that suspends a user-warmed session after a bounded quiet period, so a `notice-turns` employee can be left to finish a delegation without a session outliving it.

Do not combine these boundaries implicitly. Bounded lead remediation (at most two policy-approved remedies inside five minutes), user notification channels, cross-tree and user-to-employee messaging through the tools, ConPTY, typed desktop Inbox binding (tasks, approvals, incidents, notices, messages), and ACP approval bridging remain available candidates behind those.

Do not emit `UI BACKEND CONTRACT: GO` yet. Approval, dispatch, task, tool, incident, notice, message, and activation-policy read models exist in the daemon, but the typed desktop-facing contract has not been separately reviewed.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the durable record-first ledgers and native provider evidence.
- A notice or peer message is derived evidence: the task, message, and incident rows it points at stay authoritative, and neither ever retries work, restarts anything, re-authenticates, changes authority, or deletes data.
- Never merge, summarise, or truncate one notice into another; a coalesced turn carries each notice whole and each keeps its own message row and evidence.
- Never let employee-authored traffic start a provider process on its own. Only a recorded notice turn for an employee the user set to `notice-turns` may, once per turn, on that employee's own task, and the session stops with the turn; everyone else waits for the local user. No tool can read or set the policy.
- A notice or peer turn runs under the recipient task's effective authority; governed actions inside it still flow through the schema-8 approval gate, and neither a tool result nor a peer message is ever an approval.
- Never let a tool name a different actor, widen authority, or bypass a store guard; the daemon fixes the actor to the turn's recipient endpoint.
- Never deliver a peer message to a task that has not started, never let a peer-only turn complete a task, and never reuse a closed notice turn.
- Never declare tools on a resumed Codex thread; every activation is a fresh `thread/start`, and the adapter fails closed until Codex accepts `dynamicTools` on `thread/resume`.
- Never retry an activation, an uncertain dispatch, a failed notice or peer turn, or an expired approval automatically.
- Never delete or rewrite a retained task worktree without an explicit cleanup contract.
- Keep UI, ConPTY, session-wide grants, managed memory, scheduling, resource governance, other-provider expansion, and live model use outside this completed slice.
