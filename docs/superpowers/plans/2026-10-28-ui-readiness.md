# Backend preparation for UI implementation

The owner requested a verified foundation on which UI development can comfortably
start, with efficient execution. This continues core v1; it does not implement the
frontend or claim every full-product acceptance gate has passed.

Base: `feat/core-v1-completion` at `9c7fa9d`, containing current `origin/main`
(`dfa35f1`, verified after fetching). Work: `codex/backend-ui-readiness` in an
isolated managed worktree. Existing provider configurations and live user data
must remain untouched. Builds use one job and omit development debug symbols
because the host has little free memory.

## Work and acceptance

- [x] Make launcher capabilities match actual provider/authority dispatch. Include
  effective authority and collaboration availability; preserve the employee's
  authority ceiling. Correct read-model event high-water capture where necessary.
- [x] Settle permanent authority rejections without orphaned queued work; preserve
  stale revision fences and the existing explicit recovery for temporary failures.
  Preserve record-first evidence and idempotency; never automatically resend work
  that may have reached a model.
- [x] Provide bounded backward history paging without losing current session and
  worker state or missing later events.
- [x] Expose native terminal attachment status and recovery metadata through a
  typed backend contract. Keep terminal bytes on their authenticated separate
  channel and preserve exclusive ownership of native sessions.
- [x] Replace stale desktop contract guidance with current request/response,
  event refresh, error/retry, capability and terminal integration instructions.
- [x] Repair event subscriptions so reconnects replay more than 1,024 durable
  events in bounded pages and live notification lag cannot skip durable events.
- [x] Independently review specification compliance and implementation quality,
  fix findings, then run the settled-tree deterministic gate and a bounded CLI
  stress check. Record exact results and remaining product limitations.

Completed on 2026-10-29: [final verification](../../verification/2026-10-29-ui-backend-readiness.md)
records 44 gate steps, 13 release native checks and 2,048 clean CLI stress calls.

The initial audit identified real mismatches: launcher availability ignores
provider authority restrictions; only Codex receives collaboration tools;
unsupported delivery leaves recorded work queued; the workspace history exposes
no older-page cursor; and the contract index says terminal streaming does not
exist even though it does. The work above addresses these integration gaps.

## Native terminal decision

The owner confirmed on 2026-10-28: **the actual native TUI must stay visible during
orchestrated work**. The production implementation now provides a genuine Codex
TUI view of the same controller-owned thread, including streaming and history
reconnect while Lazoo retains canonical turns, authority and collaboration
responses. The separate suspended attach operation remains available. Concurrent
native integration for other providers remains outside this verified UI starting
point. See the [live-view plan](2026-10-28-live-native-tui.md).

## Verification boundaries

Use focused failing behavioral tests before implementation and passing tests
afterwards. Avoid multiple concurrent Rust builds. Live model calls are excluded
from the deterministic gate. Historical live evidence from 2026-10-17 is dated
evidence, not a current provider compatibility claim. Connectors, vectors and
voice remain later phases. Full core v1 completion and permission to build the
UI are separate from the backend readiness evidence recorded here.
