# Provider parity before UI implementation

The owner extended the UI readiness requirement on 2026-10-29 to OpenCode, Claude
Code and Antigravity, then clarified: "Bring thing to where the Codex support
stands right now. Do not overshoot." Grok is excluded; its subscription is no
longer available. Small tests on the four planned providers are authorized.

The reference is the Codex checkpoint at `e965fa3`, with production source at
`feab4d5`. The previous verification remains valid for Codex but is insufficient
to start the requested multi-provider UI. This work extends the already approved
native-view and canonical orchestration contracts rather than redefining the app.

## Required parity

For each included provider, prove:

1. A named employee can lead or receive delegated work, using its real provider
   session and selected model, with durable canonical turns and progress evidence.
2. The existing collaboration catalog operates under the active employee/session/
   turn identity. Roster, delegation, wait/report, peer messages and incidents use
   the same daemon guards. Native tool replies do not create another controller.
3. Ask, Self Decision and Full Autonomy respect the employee ceiling and existing
   per-action approval ledger. Configuration auto-allows, native UI controls and
   alternate native tools must not bypass that authority.
4. The actual native TUI shows the same ongoing session while Lazoo orchestrates.
   A fabricated transcript, fork or suspend-then-resume is insufficient. A native
   remote viewer or an owned interactive process with a read-only byte view may
   satisfy this if its control and lifecycle boundaries are proven.
5. View close/reopen does not cancel accepted work or resolve pending approvals.
   History, terminal identity/revision fencing, session stop, daemon shutdown,
   cancellation and explicit recovery remain truthful and bounded. A provider
   restart never silently resends uncertain work.
6. Launcher and terminal capability fields become positive only after production
   implementation and acceptance. Unsupported or unproven states stay explicit.

## Design choices and evidence gates

### Shared native-view lease contract

The internal `NativeViewLease` has two non-serializable launch descriptors. A remote descriptor
starts a restricted viewer with its private environment overlay; an existing-terminal descriptor
borrows the provider's canonical `TerminalSession`. The latter receives an independent daemon
terminal ID, revision and bounded cache/idempotency records, but never gains write authority.
Closing, replacing or shutting down that logical view must not stop the canonical process. Its
lease closes when the canonical process exits; reopening a new view retains the terminal's bounded
output history. Resize remains available because it targets the shared ConPTY.

Remote and borrowed descriptors must both retain token redaction and keep overlays out of protocol
records. Terminal input rejects borrowed views before any bytes reach the canonical process.

Prefer the existing provider control protocol plus a restricted native viewer,
as implemented for Codex. This preserves structured events and isolates viewers.
OpenCode uses an owned HTTP canonical controller and a separately authenticated, restricted
native viewer. The viewer receives only scoped history/status/event reads for its exact
session/cwd and refuses prompts, permissions, interruption, configuration, MCP mutation and
session enumeration. The real local-fixture release acceptance now proves this route; see
[the parity handoff](../../UI-PROVIDER-PARITY-HANDOFF.md).

Where native interactive and machine-control modes are separate, evaluate one
owned interactive ConPTY process, canonical prompt input, and documented native
hooks/control events. That approach is acceptable only with structured turn and
permission evidence, reliable prompt framing, fail-closed callback failure and
read-only viewer access. Screen scraping alone cannot settle durable work or
authorize actions. Do not create a second uncontrolled native session.

A session-scoped MCP bridge may expose the existing collaboration tools where
the provider lacks Codex-style dynamic tools. It must use native per-session
configuration, bounded requests and private authentication, and reject calls
outside the active turn or from a stale session. User configuration is not edited
to inject Lazoo's own control tools.

If no route meets a provider's control requirements, retain concrete installed-
version evidence and continue independent provider work. Do not advertise parity
or silently weaken the owner's native-TUI requirement.

## Scope and verification

This is provider parity with the current Codex checkpoint. It does not expand
connectors, vector search, voice, general configuration federation or unrelated
performance projects. Configuration changes needed to launch a governed provider
remain part of its adapter implementation. No frontend is built here.

Use disposable repositories/state and local model fixtures for protocol and
failure tests. Keep any real provider turns short and purposeful; record the
provider, model/channel, version, request count and outcome. Do not upgrade CLIs,
change personal configuration, print credentials, or test Grok.

Before completion, require focused regression tests, independent control/lifecycle
review, a settled-tree full gate, release native acceptance for every provider,
small live collaboration/permission/lifecycle tests, and the existing CLI stress
check. Keep one Cargo lane and run performance checks without competing workloads.
