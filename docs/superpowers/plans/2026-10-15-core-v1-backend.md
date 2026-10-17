# Core v1 backend completion plan

> **For agentic workers:** Use superpowers:subagent-driven-development with implementation and independent review. The owner's 2026-10-15 instruction authorizes proceeding with core v1 backend work; UI, connectors, vector retrieval, and voice are outside this task.

**Goal:** Complete the backend requirements in PROJECT-BRIEF section 24, with explicit evidence for each requirement and no substitution of fixture tests for live provider gates.

**Architecture:** Preserve the existing daemon/store/provider boundaries. New capabilities use durable record-first operations, revision checks, bounded storage and queues, and fail-closed authority. Keep the desktop contract and frontend unchanged.

**Tech stack:** Rust 1.98, SQLite, authenticated Windows named pipes, Windows Job Objects and ConPTY, native provider protocols.

## Starting point

The owner requested checking out and pulling main before implementation. Main was fast-forwarded to `dfa35f1`; that main was merged into `codex/core-v1-backend` at `ab00236`, preserving the existing backend work through `5803f06`. No implementation changes preceded the pull. The earlier baseline run was stopped and is not completion evidence.

## Work order and gates

- [x] Pull main and reconcile the existing backend branch.
- [x] Audit section 24 against code and current handoff.
- [x] Investigate the previously recorded CLI heap-corruption exit. The fresh release passed 2,048 invocations at concurrency 16; the earlier crash did not reproduce and no causal fix is claimed.
- [x] Bridge ACP per-action permissions through the existing approval observer and ledger. Preserve streaming evidence, cancellation, deadlines, session identity, and one-action-only grants. Prove malformed requests, unavailable one-shot options, observer failure, cancellation, and deadline expiry all refuse approval.
- [ ] External gate: demonstrate the complete native authority boundary before enabling other-provider governed delivery. Approval bridges are implemented and fixture-tested, but native auto-allow rules remain an explicit refusal boundary.
- [x] Implement native terminal lifecycle and separately framed bounded I/O with ownership, resize, reconnect/overflow, and cleanup tests.
- [x] Implement bounded compressed source archives, typed memory, durable summarization jobs, scoped retrieval, and bounded context injection.
- [x] Implement selected configuration federation with version validation, secret-safe diffs, backup, atomic apply, re-probe, rollback, and desired/observed state.
- [x] Implement infrastructure resource enforcement, rolling accounting and background backpressure, excluding employee process trees.
- [x] Complete backend install/recovery verification and the full deterministic gate (37 steps passed; subsequent compatibility fixes have separate focused and release-binary checks indexed in `artifacts/acceptance/core-v1-final-verification.json`).
- [x] Pass the short live smoke tests for the required providers. Updated 2026-10-16: the owner excluded Grok; OpenCode 1.19.0 passed with Big Pickle and Claude 2.1.300 passed through the restored native subscription, joining the existing Codex and Antigravity passes. The first post-login Claude timeout is retained as historical evidence, with no causal fix claimed.
- [ ] Complete native lifecycle/authority acceptance beyond the short reply tests: tools, permission coverage, cancellation, resume/recovery, and terminal attachment where supported. Deterministic coverage landed 2026-10-17 for cancellation on every adapter, suspend-then-attach for Claude Code and Codex, and resume on wake (Claude Code resumes; Codex starts fresh with a recorded reason); the live proofs remain.
- [ ] Explain the intermittent CLI timeout: one run timed out after 850 successes; the final diagnostic run passed 4,096 calls at concurrency 16. This is non-reproduction, not a causal fix.

## First implementation: ACP approval transport

Files: `crates/lazoo-providers/src/acp.rs`, `opencode.rs`, `grok.rs`; protocol fixture and provider tests. Reuse `PermissionRequest`, `PermissionDecision`, and `TurnEventObserver` without widening the durable approval kinds.

1. Add fixture-driven failing tests for an observed ACP turn: incremental events arrive before prompt completion; an `allow_once` option is selected only after `Approve`; `allow_always` is never selected; wrong session, unknown tool kind, malformed options, missing observer, error and cancellation result in `cancelled`.
2. Add an observer-aware ACP turn path and forward it from both public session wrappers. Continue collecting returned events while observing each event at arrival.
3. Parse the pinned ACP v1 schema. Accept only typed execute/edit requests with unambiguous one-shot options. Treat uncertain scope as expansion. Bound and redact action details. Keep the native request identity bound to the active session/turn.
4. Select the exact offered `allow_once` option only after the durable observer approves it. Every other path returns `cancelled`; never request a session-wide grant.
5. Bound approval waiting by the remaining turn deadline and support native cancellation independently from the process reader lock. Approval waiting does not masquerade as provider progress.
6. Run provider tests and clippy, then independently review the adapter and daemon boundaries before changing daemon eligibility.

## Verification rules

Every new behavior needs a failing behavioral test followed by a passing run. Run the full gate only on a settled source tree. Keep exact logs and a requirement-status record; a successful compile or deterministic adapter fixture does not prove live provider support. Do not edit user provider configurations or run paid model turns merely to satisfy a status label.

## Implementation review outcome

The completed checkboxes describe the bounded implemented services, not full section-24 acceptance. See `docs/verification/2026-10-15-core-v1-backend-status.md` for limits: generic terminals are not provider-session takeover; canonical archives are not every raw frame; the CPU job excludes desktop; native configuration apply is limited to the exact reviewed Codex 0.155.1 and 0.162.0-alpha.17.2 MCP formats.

Independent review identified and verified fixes for archive recovery/quota reservations, configuration lost-reply replay, terminal stop/reap ownership, worker lease fencing, worker restart settlement, successful attempt state, and private-session reservation before native activation. Memory context recall is verified across new tasks within its allowed project/Home scope.
