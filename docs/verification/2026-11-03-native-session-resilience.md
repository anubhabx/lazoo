# Native session resilience verification

Date: 2026-11-03. Worktree: `C:\workspace\Orchestrator\.worktrees\copilot-account-routing`.
Branch: `fix/native-session-resilience`, stacked on `feat/desktop-ui-native-first` (PR #46), above PR #45.

## Changes and evidence

### Codex view isolation

The previous shared stdout reader rejected every line over 512 KiB, including correlated viewer history replies. A fixture returning 600 KiB to `thread/turns/list` reproduced the reported `ProcessExited { code: None }` signature and made the next canonical turn fail. This establishes a concrete trigger, not proof of which close branch the original Run 3 took; that run's broker close diagnostic was not supplied.

The broker retains the single canonical app-server and real native TUI. Viewer requests use a separate `view:` correlation namespace. Oversized viewer replies are drained with bounded memory; a small streaming envelope scanner retains only top-level correlation, including when the id follows the large result. Their waiter receives a read error and canonical stdout continues. Late/cancelled oversized view replies are also drained. Oversized controller messages and meaningful controller queue overflow retain fail-closed behavior.

Viewer writes have a 4 KiB cap and one in-flight slot. Their caller can time out or disappear without cancelling a partially written NDJSON frame or invoking canonical transport shutdown. A detached, bounded write finishes the frame; waiting for the stdin lock can expire without closing the host. Notification fan-out remains bounded/lossy for views; notifications that the canonical turn normalizer ignores no longer fill its 32-message queue. Meaningful events, tools and approval requests retain controller delivery.

Regression coverage: large history while idle and during an active turn, viewer-generated notification flood, slow/nonreading view closure, late responses, cancellation cleanup, controller byte overflow, oversized envelopes with reordered fields/escaped text/nested spoofed ids, and the following canonical frame.

### Installed Codex compatibility

Lazoo's Codex discovery requests `codex.exe`, not the npm `codex.cmd` shim. Current installed binary: `C:\Users\anubhab\AppData\Local\OpenAI\Codex\bin\22cfb1d302613eae\codex.exe`, version **0.166.0-alpha.5.1**; the PATH shim is **0.153.4**. The current resolved candidate matches Run 3's displayed version. Historical per-process resolution was not independently recovered.

The generated installed schema and an ephemeral, no-model app-server probe are summarized in `2026-11-03-native-session-resilience-protocol-evidence.json` (binary/schema hashes included). Metadata, config requirements, hooks, skills and model reads succeeded and the host remained alive. The schema advertises `thread/items/list`, but this binary returns `-32601`, `thread/items/list is not supported yet`; that error remains visible rather than becoming empty history. `thread/turns/list` explicitly refuses ephemeral history. No model turn was submitted.

The viewer allowlist now includes `account/rateLimits/read` with canonical null parameters. `thread/unsubscribe` returns the installed schema's required status locally and never unsubscribes the canonical driver. Hooks/skills reads use only the bound cwd and `forceReload:false`. New-empty-thread metadata comes from the genuine canonical start response. Thread mutation, plugin listing and arbitrary RPCs stay refused. Cached initialization and live notifications still come from the canonical process.

A separate reader process was evaluated and rejected: the installed binary refuses reads of an unloaded thread, so that approach would have broken native history or required a second writer. It is not in the final implementation. Context7's official `/openai/codex` app-server documentation was consulted; installed schema/runtime evidence takes precedence over newer main-branch documentation.

### Busy user turns

**Chosen behavior: durable FIFO queue.** Eligible user submissions are marked in the existing general `processed_commands` ledger (`turn.queue`) and emit `turn.queued`. Their turn stays `recorded`, their message stays `queued`, and no delivery attempt/provider write occurs until the recipient is free. Per-employee gates span activation, delivery and cleanup, and the store transaction independently prevents concurrent active submissions and overtaking earlier queued user turns across conversations. The existing notice service drains the user queue and is explicitly notified after delivery releases its gate.

Idempotent submission returns the same turn/message. A cancelled recorded turn is skipped. Explicit `turn.retry` while the recipient is busy refuses before retry mutation with `LZ-TURN-RECIPIENT-BUSY`; it creates no provider-incompatible incident. Missing/unready/reconfigured recipients do not receive provider writes. After loss of the host or daemon restart, queued work waits for an owned idle session; the queue does not independently activate replacement sessions. An explicit warm or authorized user activation can make it deliverable again. Uncertain/accepted work is never automatically replayed.

Old dormant submissions that were refused with `LZ-TURN-SESSION-NOT-READY` and were not authorized to activate remain recorded-only; they are not silently enrolled in automatic delivery. Private memory-summary turns keep their separate lifecycle.

### Event framing

The control-plane IPC maximum is 1 MiB. Previously `events.list` used only a row limit; serialization could exceed that maximum and `write_frame` closed the connection without sending a response. `events.list` now returns an ordered prefix within a **768 KiB serialized event-array budget**. `limit` remains an upper bound, and callers continue from the last returned sequence, even when the page is shorter than the requested count.

Wire event summaries have a **256 KiB maximum**. Oversized payloads are explicitly replaced with `{ "_lazooOmitted": true, "code": "LZ-EVENT-PAYLOAD-TOO-LARGE", "eventBytes": <original serialized event bytes> }`; event sequence/id/type/timestamp remain intact and the durable audit payload is unchanged. This same projection protects replay and live `events.subscribe`, including the desktop channel. `desktop.snapshot` also bounds its recent-event array. Any other oversized RPC result returns `LZ-RESPONSE-TOO-LARGE` rather than failing with early EOF. This is a response-size error, not a rollback of a completed mutation; refresh the entity when it follows a mutation.

Tests cover page continuation without gaps, large snapshot events, oversized multibyte payloads, actual authenticated named-pipe list responses, and individual replay notifications including a 2 MB event. Broadcast remains only a wake-up; subscription replay reads the ordered audit log.

### Failed resume activation

When and only when a **resume startup returns `ProviderError::ProcessExited`**, record its failed session, begin one new session row, emit `session.resume.failed` linking the original predecessor, failed attempt and fresh attempt, and retry once with no resume id and a new managed working directory. Both attempts remain auditable even if the fresh attempt fails. Missing executable/authentication/protocol/timeout errors are not retried; an initial fresh startup failure is not retried. Neither the already accepted user turn nor its provider work is replayed.

This logic is shared by user activation, warm and notice activation. Task dispatch with its explicit workspace does not resume. Claude's parent-session environment fix is untouched; `interactive_session.rs` has no changes.

## RPC/event handoff to Claude

- `turn.submit`: unchanged result shape; successful busy submission returns `turn.state = "recorded"`, `message.delivery.state = "queued"`. A `turn.queued` audit event carries `{turnId, employeeId}`. UI may show queued for this successful submission, then follow ordinary turn events.
- `turn.retry`: may return `LZ-TURN-RECIPIENT-BUSY` before mutation, with no incident.
- `events.list`: unchanged array shape; shorter byte-bounded pages are valid. Continue with `after = last.sequence`; a short page alone is not proof of end-of-log.
- Event payload omission marker and `LZ-RESPONSE-TOO-LARGE` are described above. Treat an omitted event payload as an invalidation/refresh signal, not complete entity data.
- `session.resume.failed`: `{sessionId, priorSessionId, freshSessionId, reason:"provider-exited-during-resume"}`. Existing session-started/failed/ready events describe each attempt.
- Native viewer bridge errors remain JSON-RPC `-32001`; no desktop terminal RPC signature changed. No schema migration or UI source changes.

## Validation

- PASS: initial red regression tests observed all four original failures before their corresponding fixes.
- PASS: `cargo fmt --all -- --check`.
- PASS: `cargo clippy --workspace --all-targets -- -D warnings` (exit 0).
- PASS: `cargo test --workspace` (exit 0): **738 passed, 0 failed, 23 ignored**, summed across the final workspace result blocks. Ignored tests are the existing opt-in live/native-provider checks and child-only fixture.
- PASS: focused durable-queue and daemon resilience regressions; the final workspace run includes these, the Codex view regressions, the real named-pipe event regression, and the adjusted restart fixture.
- PASS: `git diff --check`; no changes under `apps/lazoo-desktop/ui` or in `crates/lazoo-providers/src/interactive_session.rs`. Branch remains `fix/native-session-resilience`; nothing staged or committed.

Local final logs: `artifacts/native-session-resilience-clippy-final.log`, `artifacts/native-session-resilience-workspace-final.log`. Earlier focused logs: `artifacts/native-session-resilience-queue.log`, `artifacts/native-session-resilience-regressions.log`. Logs and generated probe scratch are excluded from the staging command below; the compact installed-protocol evidence JSON is included.

Every Cargo invocation used `CARGO_BUILD_JOBS=2` and the existing worktree `target`. No release build, target cleanup, authenticated model turn or desktop UI build was requested/run. Existing ignored live-provider tests remain opt-in. The initial broader run found two queue-policy regressions (dormant recorded-only work and private memory work); automatic delivery was narrowed to durable queue markers, retaining the existing acceptance policy. A persistence regression also caught an attempted queue marker in the constrained turn-command ledger; the final implementation uses the existing general processed-command ledger. The restart-reconciliation fixture now uses two employees for its two in-flight turns; all recovery assertions are preserved.

No claim of a full real-desktop replay of Run 3 is made. That remains the external acceptance check for Claude with the new backend debug build. Generated schema/probe scratch stays under `artifacts/native-session-resilience-*` and is not part of staging. The previous untracked account-preferences scratch is untouched.

## Exact staging handoff

Leave all changes uncommitted. From this worktree, Claude can stage only the following paths; no artifacts or UI paths are included:

```powershell
git add -- PR-BODY.md apps/lazood/src/app.rs apps/lazood/src/app/delivery.rs apps/lazood/src/app/event_wire.rs apps/lazood/src/app/resume.rs apps/lazood/src/app/task.rs apps/lazood/src/app/turn.rs apps/lazood/src/server.rs apps/lazood/tests/daemon.rs crates/lazoo-providers/src/codex_broker.rs crates/lazoo-providers/src/codex_frame.rs crates/lazoo-providers/src/codex_view.rs crates/lazoo-providers/src/lib.rs crates/lazoo-providers/tests/codex.rs crates/lazoo-providers/tests/fixtures/codex/fake-codex-app-server.ps1 crates/lazoo-store/src/conversation.rs crates/lazoo-store/src/lib.rs crates/lazoo-store/src/session.rs crates/lazoo-store/tests/conversation.rs crates/lazoo-store/tests/task.rs docs/verification/2026-11-03-native-session-resilience.md docs/verification/2026-11-03-native-session-resilience-protocol-evidence.json
```

Suggested title: `Fix native session resilience and queue busy user turns`.
PR base: `feat/desktop-ui-native-first` (PR #46). Body: root `PR-BODY.md`.


## Real-desktop replay (Claude, 2026-11-03)

Debug build of this branch, disposable data roots, the real desktop driven over CDP, Claude Code lead (claude-sonnet-5-5, notice-turns) delegating to a Codex worker (gpt-6-luna, Codex 0.166.0-alpha.5.1).

| Replay | What happened | Outcome |
|---|---|---|
| e (Codex's commit alone) | The lead's first turn stayed `recorded`, `delivery: queued`, no session: the delivery pass, woken by the queued turn, took the dormant lead's delivery gate, could not activate, and the submission parked the turn | **Regression, fixed** in `ad85211`: the pass leaves a recipient without an idle session to the submission that activates it. Daemon test `resilience_background_delivery_never_strands_a_dormant_leads_first_turn` (twenty fresh daemons, three concurrent passes) fails 3 of 4 runs without the guard and passes every run with it |
| f (with the fix) | Activation, delegation, accept; forge's view opened 0.7 s into its turn; forge worked for 12 s (rollout ends 21:26:37.5 after the test command) and ended 3 s later with `LZ-PROCESS-EXIT`, task uncertain | **Issue 1 still reproduces** |
| g (same build, forge's view never opened) | Forge completed; the Claude lead's turn then ended uncertain with "Claude display sequence exceeds bounds" | View confirmed as the trigger for issue 1; a second bug in the lead found |
| — | Claude's `MessageDisplay` hooks arrive concurrently; a new message's batch 1 landed before batch 0 | **Fixed** in `d5c3098`: per-message reorder buffer, replays and post-final batches ignored, held text flushed at Stop, bounded; unit test covers the live order |
| h (all fixes) | Forge's view opened 1 s into its turn; forge completed; the lead's turn completed after the result notice; a message sent to the busy lead showed "Queued for atlas" | Pass |

Issue 1 is therefore intermittent with this branch (one failure, one pass with the view open; pass without it), not fixed. The daemon logs nothing that says why it ended the provider process (`lazood` fixes its filter at `info` and the exit path records only the incident), so the next step is a warn-level record of every path that terminates a Codex child (broker overflow, viewer disconnect, job object) and the child's real exit status, then one more replay.

Desktop follow-up in the same branch: the composer and Activity say "queued" again (`11a3f74`), and approval deadlines read forward ("expires in 14m"; the past-only formatter made every pending approval say "expires now").
