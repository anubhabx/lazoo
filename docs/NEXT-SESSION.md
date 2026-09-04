# Next session: post-fix Codex live turn, then task/delegation design

## Current owner boundary

Backend and communication layers remain this branch's scope. UI implementation is owned separately. Do not change the desktop frontend, Tauri command surface, `desktop.snapshot`, or UI capabilities while finishing this backend sequence.

The approved desktop information architecture remains in `docs/superpowers/specs/2026-08-30-lazoo-desktop-information-architecture-design.md`. This handoff does not authorize ConPTY, worktree mutation, task execution, policy expansion, managed memory, or other-provider conversation delivery.

## Current backend state

- Database schema 5 stores durable employees, provider observations, native employee sessions, actor-aware collaboration envelopes, projectless or project-scoped conversations, immutable turns, append-only normalized provider events, delivery attempts, dead letters, command outcomes, audit events, and outbox rows.
- `conversation.create/list/show` and `turn.submit/retry/cancel/list/show` are available through the daemon and CLI. Conversation/turn failures use CLI exit code 12.
- User-authored turns may explicitly activate the selected lead with `activateIfNeeded`; employee-authored turns cannot activate dormant recipients. Daemon-managed conversation delivery is enabled only for Codex.
- Turn submission is record-first. Lazoo acknowledges the canonical message only after it durably records a native turn ID or progress evidence. Definite pre-acceptance failures follow bounded retry policy; accepted or uncertain work is never retried automatically.
- Native `completed`, `failed`, and `interrupted` outcomes map to `completed`, `terminal-failed`, and `cancelled`. Session-level events after acceptance no longer have to re-prove acceptance.
- Restart reconciliation leaves recorded queued work eligible for an idempotent replay, marks prior-boot submitting/accepted work uncertain with `LZ-TURN-OWNER-LOST`, and never adopts a prior provider PID or native turn.
- The deterministic conversation-delivery acceptance uses real release binaries and a disposable Codex App Server protocol fixture. Its current manifest is PASS with 32 commands, `nativeTerminalMapping = PASS`, `outerCodexContextIsolated = PASS`, `cleanProviderTreeStop = PASS`, and `modelInvocationAbsent = PASS`.
- The full post-fix deterministic matrix is green: 196 workspace tests passed, 0 failed, 11 live-only tests ignored; format, Clippy, all-target check, locked release build, and all five backend acceptance manifests passed.

## Live-gate status

Two explicit quota-consuming Codex checks were attempted before the final persistence fix. Both correctly remained unclaimed and wrote a FAIL live manifest with public error `LZ-TURN-UNCERTAIN`.

The second isolated run created native thread `01a0694d-5ac1-72e2-bc1e-9fbe70862ef0` and turn `01a0694d-60f2-7bd0-a072-6426489fda71`. A read-only App Server `thread/read` showed native status `interrupted`, no items, and no provider error. The failure was traced to `Store::observe_turn`: after the synthetic native-ID event moved the turn to `accepted`, a normal session-level event without a turn ID was incorrectly subjected to the original acceptance-evidence guard. The observer failed and the adapter performed its designed best-effort interrupt. Regression coverage now emits that real-shaped session-level event and passes.

No third model prompt was issued in the same verification pass. Therefore deterministic delivery is verified, but post-fix live completion and exact output are not yet verified.

A fresh non-model preflight on 2026-09-04 found the currently selected desktop Codex executable at version 0.153.1. `codex login status` exited 0 with ChatGPT login active; the ignored live App Server handshake test passed with `verification = configured`, `protocol = app-server/v2`, and native authentication `ready`; and the ignored live session-start test passed creation-time containment plus a clean stop without starting a model turn. These checks prove current readiness around the remaining gate, not model-backed conversation completion.

## Next-session objective

1. Re-run exactly one explicit Codex live gate only with fresh user authorization for the quota-consuming prompt:

   ```powershell
   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\conversation-delivery.ps1 -SkipBuild -Provider codex -LivePrompt "Reply with exactly LAZOO_TURN_OK. Do not use tools."
   ```

2. Require `conversation-delivery-live-manifest.json` to report PASS, exact `LAZOO_TURN_OK`, a durable native turn ID, acknowledged canonical message, completed turn, and clean provider-tree stop. If it fails, preserve the exact blocker and do not repeat the prompt automatically.
3. Refresh the verification document and finish branch integration only after that result is recorded truthfully.
4. Once the live gate is green, write a separate reviewed design and plan for the next proposed backend slice: a durable task/delegation graph with explicit lead/authority boundaries. Do not combine it opportunistically with worktree mutation, ConPTY, approvals, managed memory, resource governance, or UI binding.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Reuse the canonical collaboration envelope and conversation turn; do not create a second provider message path.
- Keep Codex conversation delivery distinct from earlier provider-smoke evidence and from unverified other-provider delivery.
- Never call a turn delivered merely because stdin accepted bytes or an RPC returned success.
- Keep `ask` as the default authority mode. No model may grant user approval or broaden its own authority.
- Keep model-backed gates explicit, isolated, and visibly quota-consuming.
