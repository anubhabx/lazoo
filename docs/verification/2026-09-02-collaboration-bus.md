# Durable Collaboration Bus Verification

Date: 2026-09-02
Platform: Windows
Scope: backend protocol/domain/store/daemon/CLI plus acceptance and continuity documentation
Schema: 4
Result: PASS

## Delivered contract

- Immutable version-1 envelopes with durable sender/recipient endpoints, optional project/conversation/task/session context, correlation/causation, message kind, JSON body/references, and idempotency identity.
- Transactional record-first send: envelope, queued delivery revision, audit event, and outbox entry are committed before an attempt exists.
- Ordered replay plus show, claim, acknowledge, fail, retry, cancel, list, and show controls through the authenticated daemon API and CLI.
- Explicit `queued`, `delivering`, `acknowledged`, `retryable-failed`, `terminal-failed`, and `cancelled` states with monotonic revisions.
- Attempt-correlated acknowledgement, one/five/thirty/one-hundred-twenty-second bounded retry delays, maximum attempt validation, terminal dead letters, and stable `LZ-MESSAGE-*` errors on CLI exit code 11.
- Restart reconciliation closes uncertain attempts with `LZ-MESSAGE-OWNER-LOST`, retains logical message identity, and immediately publishes the reconciliation outbox entry.
- Recipient activation intent is data only. No provider session is activated and no provider turn is submitted.

## Inline-TDD evidence

The focused tests were observed failing before each implementation step:

1. protocol tests failed to compile because message contract types were absent;
2. domain tests failed to compile because message validation functions were absent;
3. store tests failed to compile because message inputs and persistence methods were absent;
4. delivery-state tests failed to compile because claim/acknowledge/fail/retry/cancel methods were absent;
5. daemon tests returned `LZ-METHOD-NOT-FOUND` for message RPCs;
6. the acceptance-contract test failed because `scripts/acceptance/collaboration-bus.ps1` did not exist;
7. the restart regression assertion failed with pending outbox `left: 1`, `right: 0` before startup publication was added.

The corresponding focused suites passed after the minimum implementation. The acceptance harness also exposed Windows PowerShell 5 native JSON quoting, which is now handled without changing the CLI protocol.

## Final command gates

Every command below was run from the collaboration-bus worktree after the final implementation and continuity updates.

| Command | Exit | Evidence |
|---|---:|---|
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-rust-toolchain.ps1` | 0 | `C:\Users\anubhab\.cargo\bin\cargo.exe`; Cargo 1.98.0 release toolchain |
| `cargo test --workspace` | 0 | 156 passed, 0 failed, 11 ignored |
| `cargo fmt --all -- --check` | 0 | no formatting diff |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | no warnings |
| `cargo check --workspace --all-targets` | 0 | all targets checked |
| `cargo build --workspace --release --locked` | 0 | optimized locked release completed |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/first-slice.ps1 -SkipBuild` | 0 | PASS, schema 4, 10,618 ms |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/employee-roster.ps1 -SkipBuild` | 0 | PASS, schema 4, 7,541 ms |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/native-session-ownership.ps1 -SkipBuild` | 0 | PASS, schema 4, 10,816 ms |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/collaboration-bus.ps1 -SkipBuild` | 0 | PASS, schema 4, 6,746 ms |

The 11 ignored tests are the existing opt-in/live provider checks. They were not claimed as passing. Native-session acceptance did run the existing real Codex warm/stop path without sending a model prompt.

## Collaboration acceptance evidence

`artifacts/acceptance/collaboration-bus-manifest.json` reports PASS for:

- `recordFirstSend`, `idempotentSend`, `orderedReplay`, and `causalCorrelation`;
- `acknowledgementCorrelation` and `retryNotDueRejected`;
- `retryExhaustion`, `deadLetterEvidence`, and `cancellation`;
- `restartDurability`, `ownerLossReconciled`, and `auditOutboxEvidence`;
- `providerSmokeAbsent` and `desktopCapabilityExpansionAbsent`.

The manifest contains ordered audit evidence for `message.queued`, `message.delivering`, `message.acknowledged`, `message.retryable-failed`, `message.terminal-failed`, `message.cancelled`, and `messages.reconciled`.

The three pre-existing acceptance manifests were regenerated against schema 4 and also report PASS. Their `releaseBuild` marker is `SKIPPED_BY_CALLER` because the locked workspace release build ran once immediately before the four-script matrix.

## Boundary verification

| Check | Result |
|---|---|
| `git diff --check` | PASS |
| `git diff --quiet main...HEAD -- apps/lazoo-desktop` | exit 0; no committed desktop diff |
| `git diff --quiet -- apps/lazoo-desktop` | exit 0; no uncommitted desktop diff |
| Message acceptance contains provider smoke invocation | no |
| Message acceptance submits a provider/model turn | no |
| `desktop.snapshot` fields or Tauri capabilities expanded | no |

ConPTY, provider-turn delivery, conversation/turn persistence, tasks, worktree automation, approvals, managed memory, resource governance, and product-ready UI remain outside this slice. The next backend dependency is the durable conversation/turn ledger plus evidence-based provider-delivery transport described in `docs/NEXT-SESSION.md`.
