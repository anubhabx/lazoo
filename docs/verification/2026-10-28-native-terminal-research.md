# Native terminal integration evidence

Read-only checks on 2026-10-28; no provider turns were sent and no provider
configuration or daemon transport was changed.

## Installed versions

The PATH-selected executables returned:

| Provider | Version command result |
| --- | --- |
| Codex | `codex-cli 0.153.4` |
| Claude Code | `2.1.302 (Claude Code)` |
| OpenCode | `1.19.0` |
| Antigravity | `1.4.0` |

Antigravity differs from the 1.3.6 version in the October 17 live verification.
Matching versions for the other three are not a fresh authentication or live
lifecycle test.

## Codex offers a possible shared-server TUI path

The installed `codex --help` and `codex resume --help` expose `--remote ADDR`;
the installed `app-server --help` exposes transport selection and WebSocket
authentication options. This means simultaneous TUI display should not be
dismissed as an inherent vendor impossibility. Lazoo's current adapter instead
spawns a private stdio `JsonRpcProcess` for each native session.

Current official source documentation, retrieved through Context7 `/openai/codex`,
also describes the [remote CLI options](https://github.com/openai/codex/blob/main/codex-rs/cli/src/main.rs),
[shared-server TUI disconnect behavior](https://github.com/openai/codex/blob/main/codex-rs/tui/src/app/exit_summary.rs),
and [server approval requests](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/protocol/common.rs).
These current-main sources may differ from the installed binary; installed help
is the direct evidence for flag availability.

## What is not proven

There is no demonstrated Lazoo-owned multi-client transport, observer-only native
TUI attachment, sole approval responder, or preserved dynamic-tool routing with
that TUI connected.

A subsequent read of the current official
[outgoing-message implementation](https://github.com/openai/codex/blob/main/codex-rs/app-server/src/outgoing_message.rs)
found a concrete issue to address: ordinary thread server requests are sent to the
thread's connection list, pending requests can be replayed to a new connection,
and response callbacks are removed by request ID. Connection ownership is enforced
there for the special user-verification ceremony, not for ordinary requests.
This is source-level evidence from current main, not a live test of the installed
binary. The engineering inference is that attaching a second ordinary client is
insufficient to guarantee Lazoo remains the sole approval/tool responder; explicit
mediation or a demonstrated vendor ownership mechanism is required.

A TUI that can directly start/steer turns or alter settings must not bypass the
canonical turn ledger and authority ceiling. Windows transport behavior also
needs an actual bounded integration test.

Therefore the implemented safe terminal contract remains suspend-then-attach.
The remote TUI path is a concrete candidate for the concurrent-native-TUI work,
not completion evidence or a reason to expose an unauthenticated listener.
