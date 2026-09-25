# Next session: post-coalesced-notices backend boundary

## Current owner boundary

Backend orchestration remains the active scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities without a separate reviewed desktop contract.

This handoff does not authorize ConPTY, terminal I/O, session-wide approval grants, per-capability policy, managed memory, resource governance, scheduling, other-provider task delivery, approval bridging, or tool surfaces, automatic activation of a dormant employee, cross-tree or user-to-employee messaging through the tools, bounded remediation, user notification channels, provider smoke, or a live model turn.

## Completed backend baseline

- Schema stays at 9. Every notice and peer message queued for one recipient and task at the moment a notice turn is recorded rides that one turn, oldest first under a fixed header; a single notice is delivered with exactly the text it had before. The oldest notice is the turn's canonical message and each companion is linked through its own `turn.record` command keyed `notice-turn:<messageId>:<turnId>`, so every notice keeps its own claim, acknowledgement (`{turnId, canonicalMessageId}`), failure, retry, and cancellation evidence beside the canonical message.
- A batch is bounded by the 256 KiB turn text limit; notices that do not fit wait for the next turn. Nothing joins a turn after it is recorded.
- A notice read inside the recipient's own turn (`task_wait`, `message_send`) closes any `recorded` turn that carried it; the other notices that turn carried stay queued and are recorded again on a fresh turn, never on the closed one, once the recipient's task is free.
- A turn carrying only peer messages projects as task progress. A turn that carries at least one result or incident notice projects like any turn and completes the task once no child is left, which is what the result alone would have done.
- The daemon pass still queues results for terminal children and notices for promoted incidents, then walks the queued notices oldest first, delivering each turn once per pass and re-reading a notice before acting so one carried earlier in the pass is left alone.
- Activation policy is unchanged: employee-authored traffic never starts a provider process. A coalesced turn for a dormant recipient stays `recorded` with its notices `queued` until the local user resumes it.
- The peer-messaging (`176979c`), result-delivery (`b320946`), minimum-incidents (`762aec2`, `24a851a`), and collaboration-tools (`d72de10`) slices underneath are unchanged: eight tools, the same refusal codes, the same `task_wait` and `message_send` inbox, and every acceptance script asserts schema 9.
- The desktop snapshot, the CLI, and every file under `apps/lazoo-desktop` remain unchanged.

## Deterministic acceptance

`scripts/acceptance/coalesced-notices.ps1` runs focused domain, store, and daemon fake-backend proofs and writes `artifacts/acceptance/coalesced-notices-manifest.json`.

No provider smoke or live model turn is part of this slice. A live Codex gate (a real model calling the eight tools, asking a sibling a question through `message_send`, and receiving several results as one notice turn after its turn ended) remains the natural next authorized live proof and must be run explicitly.

## Next backend objective

Choose and design one separately reviewed slice:

- **Activation policy and bounded automatic activation**: a per-employee `activationPolicy` (schema 10) that lets a queued coalesced turn start a session for a dormant recipient under explicit bounds (one activation per recorded turn, the recipient's own task tree only, stop after the turn), with the default staying manual. Coalescing now makes "one activation per queued batch" a single recorded turn.
- **Live Codex tool-call, peer-message, and coalesced-notice gate**: one explicitly authorized model turn proving the eight tools, one peer exchange, and one coalesced result turn end to end through the real app-server.
- **Message-arrival notification**: replace the 250 ms polls inside `task_wait` and `message_send` with a store-change notify, keeping the same bounded waits and evidence.

Do not combine these boundaries implicitly. Bounded lead remediation (at most two policy-approved remedies inside five minutes), user notification channels, cross-tree and user-to-employee messaging through the tools, ConPTY, typed desktop Inbox binding (tasks, approvals, incidents, notices, messages), and ACP approval bridging remain available candidates behind those.

Do not emit `UI BACKEND CONTRACT: GO` yet. Approval, dispatch, task, tool, incident, notice, and message read models exist in the daemon, but the typed desktop-facing contract has not been separately reviewed.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the durable record-first ledgers and native provider evidence.
- A notice or peer message is derived evidence: the task, message, and incident rows it points at stay authoritative, and neither ever retries work, restarts anything, re-authenticates, changes authority, or deletes data.
- Never merge, summarise, or truncate one notice into another; a coalesced turn carries each notice whole and each keeps its own message row and evidence.
- Never let employee-authored traffic start a provider process; only the local user resumes a dormant employee until a reviewed activation policy exists.
- A notice or peer turn runs under the recipient task's effective authority; governed actions inside it still flow through the schema-8 approval gate, and neither a tool result nor a peer message is ever an approval.
- Never let a tool name a different actor, widen authority, or bypass a store guard; the daemon fixes the actor to the turn's recipient endpoint.
- Never deliver a peer message to a task that has not started, never let a peer-only turn complete a task, and never reuse a closed notice turn.
- Never declare tools on a resumed Codex thread; the adapter fails closed until Codex accepts `dynamicTools` on `thread/resume`.
- Never retry an uncertain dispatch, a failed notice or peer turn, or an expired approval automatically.
- Never delete or rewrite a retained task worktree without an explicit cleanup contract.
- Keep UI, ConPTY, session-wide grants, managed memory, scheduling, resource governance, other-provider expansion, and live model use outside this completed slice.
