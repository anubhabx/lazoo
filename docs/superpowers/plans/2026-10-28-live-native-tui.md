# Live native Codex TUI implementation plan

> For agentic workers: use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans`; coordinate a single Cargo executor.

Goal: keep the genuine Codex TUI visible on the same session while Lazoo owns
turn submission, approvals and collaboration tools. Follow the companion spec
`../specs/2026-10-28-live-native-tui.md` and the successful local prototype.

## 1. Internal terminal environment

- [x] Extend `crates/lazoo-runtime/src/terminal.rs` with a private per-command
  environment overlay builder and a correctly terminated sorted Windows Unicode
  environment block. Inherit other variables without modifying process globals.
  Redact overlay values from Debug. Public protocol terminal params stay unchanged.
- [x] Add behavioral coverage to `crates/lazoo-runtime/tests/terminal.rs`: child
  sees its overlay, parent remains unchanged, invalid NUL/name is rejected, and
  secret values never appear in Debug. Run targeted tests before/after the change.

## 2. Codex-only broker

- [x] Add `crates/lazoo-providers/src/codex_broker.rs`: single reader/writer owned
  process, broker IDs and pending response map, bounded driver and viewer queues,
  timeouts and deterministic cancellation/stop. Expose request, next_message,
  respond, notification subscription, process identity/resources and stop methods.
- [x] Test with the repository's disposable protocol fixture: controller request
  and viewer read interleave with one server tool/approval request; replies cannot
  cross, slow viewers do not block the driver, and shutdown/overflow releases all
  waiters and the native process. Keep other providers' transport unchanged.
- [x] Integrate only live `CodexSession` in `src/codex.rs`; preserve probe behavior,
  current cancellation/evidence, dynamic tools, permission mapping and deadlines.

## 3. Restricted native-view lease

- [x] Add `crates/lazoo-providers/src/codex_view.rs` using documented
  `tokio-tungstenite 0.30` loopback handshakes and bounded message configuration;
  pin dependencies in the workspace and lockfile. Read current docs before use.
- [x] Implement the strict method/thread/cwd allowlist, genuine bootstrap snapshot,
  first-turn empty-page boundary, history forwarding, scoped notifications,
  one-viewer lease and redacted token/environment descriptor from the spec.
- [x] Add protocol tests for bad auth/Origin, duplicate viewer, read-only bootstrap,
  wrong-thread/unknown/mutation/server-response refusal, history errors, lag and
  close. Use actual WebSocket exchanges, not only allowlist helper assertions.

## 4. Daemon integration

- [x] Add the optional internal handle through `contract.rs`, `registry.rs` and
  daemon `ManagedSession`, with default unavailable implementations for fixtures.
- [x] Add typed `session.terminal.view` params/result in protocol and CLI mapping,
  explicit view availability fields in session status/launcher, and daemon RPC
  dispatch. Preserve the existing suspended attach behavior.
- [x] Implement view reservations/open/reconnect/cleanup in
  `apps/lazood/src/app/terminal.rs` (split view-specific code into a module if needed).
  Bind each view to one exact live session/revision and keep secrets internal.
- [x] Exercise live-turn/pending-approval preservation through production native
  acceptance. Add daemon tests for replay/conflicts, close/reopen, stale revision,
  unsupported sessions and session-stop cleanup. Review reserved-session and exact
  owner/replacement guards alongside the existing boot-fencing tests; keep token
  values outside serializable records and cover Debug redaction explicitly.

## 5. Real native acceptance and final gate

- [x] Promote the bounded prototype into a rerunnable local-fixture acceptance
  check using the production RPC/bridge: actual native TUI screen bytes show
  controller streaming while dynamic-tool and approval responses remain Lazoo's.
  Verify history reconnect and reject direct native turn/control attempts.
- [x] Independently review transport/security/lifecycle and spec compliance; fix
  findings with targeted regression coverage. Keep unsupported provider truth clear.
- [x] Run the final full gate and 2,048-call CLI stress (16 concurrent). Preserve
  new manifests separately from historical acceptance records. Update the UI
  handoff, next-session record and exact dated verification before declaring ready.

Completed on 2026-10-29: [final verification](../../verification/2026-10-29-ui-backend-readiness.md)
records 44 gate steps, 13 release native checks and 2,048 clean CLI stress calls.
