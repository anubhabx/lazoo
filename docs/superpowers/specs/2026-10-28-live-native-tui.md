# Live native TUI without a second controller

The owner explicitly requires the actual harness TUI to remain visible during
orchestrated work. A resumed stopped session, fork or merged transcript cannot
satisfy this requirement. This design implements the Codex path first, where
Lazoo already provides its full collaboration tool contract. Other providers
retain explicit capability limitations; this is not full core v1 acceptance.

## Evidence and selected approach

The installed Codex 0.153.4 TUI accepts `--remote` and bearer-token authentication.
The local probe in `artifacts/readiness/native-tui/probe_codex_remote.py` launched
the genuine TUI through Lazoo's existing ConPTY transport and connected it to the
same app-server thread controlled by a private stdio connection. A local HTTP
Responses fixture produced one controller-submitted turn, one dynamic-tool call,
and streamed text. Run `run-5cdf93da507b4e8c80ad82bfb2330061` observed two local
fixture requests, one tool answer by the controller, completion and the text in
native screen bytes. It sent no external model request. This is prototype
evidence, not the production implementation or a live-provider acceptance gate.

Directly adding an ordinary app-server client is rejected: TUI bootstrap supplies
its own approval/configuration overrides, and server requests would have another
possible responder. Suspending or forking fails the owner's requirement. Select
a restricted native-view bridge over the existing controller-owned stdio host.
No generic chat renderer is introduced and no upstream provider fork is needed.

## Codex transport and view isolation

Implement a Codex-only broker; leave the shared `JsonRpcProcess` used by other
providers unchanged. One owned native child, stdin writer and background stdout
reader serve the existing Codex turn driver and allowed viewer reads. Every
upstream request gets a unique broker-generated ID and a bounded pending waiter.
Only the matching waiter receives its response. Provider server requests (both
`method` and `id`) reach only the existing driver, preserving its authority,
approval and collaboration callbacks. Viewer responses never reach provider stdin.

Queue memory and request count are bounded. The reader never blocks on a slow
viewer; overflow disconnects that view for an explicit resync. Driver overflow
fails the transport closed and terminates the child, allowing existing uncertain
turn handling to retain evidence instead of losing a request silently. Stop and
drop terminate the owned process tree, cancel reader tasks and release waiters.

Each session view uses an ephemeral loopback WebSocket listener and a random
256-bit bearer capability. Reject browser Origins, absent/wrong tokens and extra
viewer connections. A lease owns the listener and token; a new open creates a
fresh lease after the old one closes. Bind a view to exactly one native thread.
Secrets never enter protocol records, command arguments, errors or logs.

Allow only startup/read methods verified by the installed TUI: initialization,
scoped thread metadata/history, model/account/config requirements, hooks, skills,
plugins and relevant status reads. Enforce thread ID and cwd scoping and bound
page sizes. Do not forward account refresh/login, configuration writes, thread
creation/resume/fork, turn start/steer/interrupt, tool responses or unknown methods.
Thread listings expose only the bound thread. Optional unsupported sidebar reads
return a clear failure rather than gaining broader access.

The viewer's `initialize` returns cached genuine initialization metadata.
`thread/resume` is a view bootstrap: combine the controller's genuine start
configuration with a fresh thread read, ignoring viewer overrides, and advertise
`canAcceptDirectInput: false`. The native client then disables direct typing.
History reads use native turn/item pagination. Before the first user message,
the installed server reports an unmaterialized thread: return an empty page only
when the broker knows that exact thread has accepted no turns. Never replace an
arbitrary history failure with an empty transcript. Later history was observed
working in the local simulation (one turn and three items).

Acquire the notification receiver before fetching a bootstrap snapshot; buffer
bounded notifications during initialization and deliver them after bootstrap.
Retain genuine provider notifications, scoped to the bound thread; suppress
server requests and internal RPC replies. A viewer close, rejected key command
or failed connection must not cancel a canonical turn or close its provider host.

## Daemon and terminal contract

Expose an optional internal live-view handle through `ProviderSession`, registry
and `ManagedSession`, defaulting to unavailable; only the implemented Codex path
supplies it. No controller endpoint or secret is serialized to the frontend.

Add `session.terminal.view` with session ID, expected session revision, dimensions
and logical idempotency key. It validates an owned active session, refuses reserved
summary sessions, obtains one view lease and launches the provider's genuine TUI
as a separately supervised terminal. Keep `session.terminal.attach` as the
exclusive suspended-session takeover operation. `session.terminal.status` gains
explicit live-view capability and reason fields, and reports any live view's
terminal identity for reconnect. The launcher names live-view support honestly.

Use an internal-only terminal environment overlay for TERM and the bearer token.
Do not add arbitrary environment variables to public `terminal.open`. Terminal
metadata stores executable/arguments/cwd/dimensions but never the overlay. Terminal
resize/read/write keep their existing boot/revision fencing and byte transport.
Keyboard input is safe because the bridge rejects control RPCs; scrolling and
native navigation remain available. Prompts and approvals stay in Lazoo controls.

View reservations are revision-bound and idempotent. Closing a viewer releases
its lease without changing task/turn state. Session stop/replacement/failure and
daemon shutdown close the listener and viewer process; a new daemon boot cannot
reuse a prior capability or terminal ID. New view creation must not hold the
session registry mutex across slow provider/process I/O.

## Required acceptance

1. Interleaved controller/view reads, notifications and provider server requests
   route exactly once with one child process, both idle and during a long turn.
2. Viewer mutation, wrong-thread access, fake server responses and unauthorized
   connections do not produce upstream control writes or policy/tool changes.
3. Bootstrap cannot replace controller authority; native history works both for a
   new empty thread and an already materialized thread.
4. Slow viewers, oversized frames and queue overflow cannot stall the controller
   or consume unbounded memory; lifecycle shutdown leaves no listener or process.
5. Daemon view open/status/reconnect/close follow revisions/idempotency and leave
   canonical work running; token values never appear in metadata or audit events.
6. Real installed Codex plus local model fixtures prove native streaming and
   controller-owned dynamic tools, with approval routing independently exercised.
7. Final formatting, warnings-denied lint, workspace tests, release acceptance
   and CLI stress pass on the settled implementation. No frontend source change.
