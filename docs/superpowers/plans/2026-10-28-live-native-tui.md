# Live native Codex TUI implementation plan

> For agentic workers: use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans`; coordinate a single Cargo executor.

Goal: keep the genuine Codex TUI visible on the same session while Lazoo owns
turn submission, approvals and collaboration tools. Follow the companion spec
`../specs/2026-10-28-live-native-tui.md` and the successful local prototype.

## 1. Internal terminal environment

- [ ] Extend `crates/lazoo-runtime/src/terminal.rs` with a private per-command
  environment overlay builder and a correctly terminated sorted Windows Unicode
  environment block. Inherit other variables without modifying process globals.
  Redact overlay values from Debug. Public protocol terminal params stay unchanged.
- [ ] Add behavioral coverage to `crates/lazoo-runtime/tests/terminal.rs`: child
  sees its overlay, parent remains unchanged, invalid NUL/name is rejected, and
  secret values never appear in Debug. Run targeted tests before/after the change.

## 2. Codex-only broker

- [ ] Add `crates/lazoo-providers/src/codex_broker.rs`: single reader/writer owned
  process, broker IDs and pending response map, bounded driver and viewer queues,
  timeouts and deterministic cancellation/stop. Expose request, next_message,
  respond, notification subscription, process identity/resources and stop methods.
- [ ] Test with the repository's disposable protocol fixture: controller request
  and viewer read interleave with one server tool/approval request; replies cannot
  cross, slow viewers do not block the driver, and shutdown/overflow releases all
  waiters and the native process. Keep other providers' transport unchanged.
- [ ] Integrate only live `CodexSession` in `src/codex.rs`; preserve probe behavior,
  current cancellation/evidence, dynamic tools, permission mapping and deadlines.

## 3. Restricted native-view lease

- [ ] Add `crates/lazoo-providers/src/codex_view.rs` using documented
  `tokio-tungstenite 0.30` loopback handshakes and bounded message configuration;
  pin dependencies in the workspace and lockfile. Read current docs before use.
- [ ] Implement the strict method/thread/cwd allowlist, genuine bootstrap snapshot,
  first-turn empty-page boundary, history forwarding, scoped notifications,
  one-viewer lease and redacted token/environment descriptor from the spec.
- [ ] Add protocol tests for bad auth/Origin, duplicate viewer, read-only bootstrap,
  wrong-thread/unknown/mutation/server-response refusal, history errors, lag and
  close. Use actual WebSocket exchanges, not only allowlist helper assertions.

## 4. Daemon integration

- [ ] Add the optional internal handle through `contract.rs`, `registry.rs` and
  daemon `ManagedSession`, with default unavailable implementations for fixtures.
- [ ] Add typed `session.terminal.view` params/result in protocol and CLI mapping,
  explicit view availability fields in session status/launcher, and daemon RPC
  dispatch. Preserve the existing suspended attach behavior.
- [ ] Implement view reservations/open/reconnect/cleanup in
  `apps/lazood/src/app/terminal.rs` (split view-specific code into a module if needed).
  Bind each view to one exact live session/revision and keep secrets internal.
- [ ] Add daemon behavioral tests for live turn unaffected by view close, duplicate
  opens, stale revision, unsupported/reserved sessions, session replacement/stop,
  restart fencing and absence of token values from returned records/events.

## 5. Real native acceptance and final gate

- [ ] Promote the bounded prototype into a rerunnable local-fixture acceptance
  check using the production RPC/bridge: actual native TUI screen bytes show
  controller streaming while dynamic-tool and approval responses remain Lazoo's.
  Verify history reconnect and reject direct native turn/control attempts.
- [ ] Independently review transport/security/lifecycle and spec compliance; fix
  findings with targeted regression coverage. Keep unsupported provider truth clear.
- [ ] Run the final full gate and 2,048-call CLI stress (16 concurrent). Preserve
  new manifests separately from historical acceptance records. Update the UI
  handoff, next-session record and exact dated verification before declaring ready.
