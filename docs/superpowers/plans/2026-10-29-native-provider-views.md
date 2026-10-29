# Native provider view integration plan

> **For agentic workers:** Use superpowers:subagent-driven-development for bounded
> components, then independent review. Keep the sole Cargo lane coordinated.

**Goal:** Preserve Codex's proven view behavior while supporting OpenCode's
restricted native attach and Claude/Antigravity's one owned interactive terminal.

**Architecture:** Generalize the internal native-view lease behind the existing
session view/status RPCs. A lease either launches a restricted native remote
viewer or borrows an already owned native terminal. A borrowed view is read-only;
closing it releases only the view. Provider session ownership remains separate.

**Tech stack:** Existing Rust daemon/provider/runtime contracts, Windows ConPTY
and Job Objects, authenticated loopback native protocols and hook callbacks.

## 1. Generalized view ownership

Files: `crates/lazoo-providers/src/native_view.rs`, `codex_view.rs`,
`crates/lazoo-runtime/src/terminal.rs`, `apps/lazood/src/app/terminal.rs`,
`apps/lazood/src/app/terminal/native_view.rs`, provider and daemon tests.

- [x] Separate provider-neutral `NativeViewLease`/launch descriptor from Codex
  WebSocket details. Use a crate-private backend interface with close/watch,
  descriptor and optional existing `TerminalSession`. Only provider code constructs
  leases. Preserve token Debug redaction and do not serialize launch overlays.
- [x] Keep the Codex bridge behavior and tests unchanged except type routing.
- [x] Add a lease for an already-owned interactive terminal: close marks that
  view closed and never stops its process. Process exit closes the logical view.
  A fresh view can reconnect to retained output/history under existing bounds.
- [x] Register borrowed terminal records with independent view IDs, ordinary
  revision/boot/idempotency fences and the existing terminal count/cache limits.
  All byte-input paths refuse borrowed views. Resize can target the shared PTY.
  Stop, replacement, failed-open cleanup and shutdown must distinguish view
  ownership from canonical process ownership; update every existing stop path.
- [x] Add runtime terminal Job Object accounting for canonical interactive
  providers without moving their process trees into infrastructure accounting.
- [x] Prove borrowed close/reopen and read-only input with a real ConPTY fixture;
  prove provider stop/exit closes its view, and preserve existing Codex lifecycle
  tests. Run focused tests when the Cargo lane is available.

## 2. OpenCode native attach

Files: `crates/lazoo-providers/src/opencode.rs`, `opencode_http.rs`,
`opencode_view.rs`, provider fixtures/tests. Installed-version feasibility is
under `artifacts/provider-parity/opencode`.

- [x] Launch the owned OpenCode HTTP server on authenticated loopback as the canonical
  controller. Lazoo owns the exact session/cwd, turn submission, permission decisions,
  cancellation, and correlated event stream. ACP is not the canonical OpenCode route.
- [x] Provide a per-view restricted asynchronous HTTP/SSE bridge. Forward only
  verified, scoped startup/history/status reads and notifications for the exact
  session/cwd. Refuse prompt, permission, interrupt, configuration, MCP, fork and
  other mutations. Authenticate the native viewer separately from the upstream
  server. Never return broad session lists or provider configuration secrets.
- [x] Attach the genuine native TUI using the lease's private endpoint. Verify
  actual streamed content and history rather than only successful process start.
- [x] Inject the shared collaboration MCP bridge through the owned per-session HTTP
  configuration with bearer authentication; preserve active-turn identity and governed
  configuration safeguards. Refuse unknown permissions and persistent grants.
- [x] Test real HTTP/SSE/auth/scoping/control refusal plus native acceptance with
  short local-fixture/free-model turns. Enable capability only after integration.

## 3. Interactive provider adapters

Claude and Antigravity use one owned ConPTY as canonical session. Exact hook
schemas and native lifecycle evidence are recorded in their provider feasibility
directories. Use structured native hook/session/transcript events for durable
truth, never terminal screen scraping. User input is serialized by the adapter,
with native prompt acceptance matched to the active canonical turn. Handle
multiline text, cancellation, pending callbacks, stop and explicit resume.

Only the authenticated hook callback may ask the turn observer for permissions;
return a one-action native decision and fail closed on errors/timeouts. The view
cannot type a prompt or answer an approval. Configure only generated per-session
hooks/MCP state. Honor native trust/setup boundaries for actual user projects.
Implementation and release acceptance are complete. See the provider parity
handoff and final manifest for the exact provider versions, proofs and limits.

Completion evidence: [provider parity handoff](../../UI-PROVIDER-PARITY-HANDOFF.md).
