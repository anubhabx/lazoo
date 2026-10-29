# Session-scoped native collaboration bridge implementation plan

> **For agentic workers:** Use superpowers:subagent-driven-development for this
> bounded component and independent review. The owner authorized provider parity
> with the existing Codex checkpoint; no new user-visible tools are introduced.

**Goal:** Expose the existing Lazoo collaboration catalog to native MCP clients
without granting another path around canonical employee/session/turn identity.

**Architecture:** Add an internal authenticated loopback MCP server per provider
session. Tool requests enter a bounded queue owned by the currently observed turn.
The turn's existing `TurnEventObserver::call_tool` remains the only executor.
The server has no database, public RPC dispatch or caller-supplied actor identity.

**Tech stack:** Rust/Tokio, Hyper HTTP/1, existing JSON tool schema and observer.
Use the pinned MCP 2025-06-18 legacy initialization protocol supported by native
clients, with explicit version negotiation. Context7 sources were checked on
2026-10-29; do not assume current 2026 stateless rules match older native clients.

## Implementation and acceptance

- [x] Add `crates/lazoo-providers/src/mcp_bridge.rs`, internal module registration
  and only the required HTTP dependencies. Interface:

  ```rust
  pub(crate) struct SessionMcpBridge { /* private owned listener/state */ }
  impl SessionMcpBridge {
      pub(crate) async fn open(provider: ProviderKind, tools: Vec<ToolSpec>)
          -> Result<Self, ProviderError>;
      pub(crate) fn endpoint(&self) -> &str;
      pub(crate) fn authorization(&self) -> String;
      pub(crate) async fn during_turn<T, F>(
          &self, session_id: &str, turn_id: Option<&str>,
          observer: &dyn TurnEventObserver, work: F,
      ) -> Result<T, ProviderError>
      where F: Future<Output = Result<T, ProviderError>> + Send, T: Send;
      pub(crate) fn close(&self);
  }
  ```

  `during_turn` installs one epoch-bound active queue before polling `work`,
  services requests through that borrowed observer concurrently with provider
  progress, and removes the queue on every exit, including cancellation/drop.
  Reject concurrent activation. Provider adapters must not start another turn
  until native completion/idle is proven. Queued requests from a closed epoch
  must never become calls on a later observer.

- [x] Write failing behavior tests, then implement real local HTTP exchanges:
  initialization/list/ping; unknown method/tool refusal; wrong bearer and browser
  Origin refusal; no-active-turn refusal; request identity comes from the binding;
  successful observer call and error response; turn completion/cancellation closes
  pending calls; old queued request does not run in the next epoch; close releases
  connections; Debug redacts capabilities.

- [x] Enforce 127.0.0.1 binding, at least 244-bit per-session bearer, no browser
  Origins, fixed `/mcp` endpoint, no public capability serialization, bounded
  connections/headers/bodies/queue/response, bounded header/read/write timeouts,
  strict JSON-RPC IDs and methods, and no arbitrary forwarding. Match Codex's
  64 KiB tool argument/result bounds. Initialization advertises only tools.
  Notifications return HTTP 202; ordinary responses use application/json; GET
  and unsupported session operations return explicit refusal. Preserve valid
  native MCP interoperability rather than inventing new transport conventions.

- [x] Run targeted provider tests with one Cargo job and strict library lint.
  Obtain an independent review before enabling any provider capability.

Provider-specific launch, MCP configuration, approvals and native TUI adapters are
separate integration slices. This component alone is not provider parity evidence.

Completion evidence: [provider parity handoff](../../UI-PROVIDER-PARITY-HANDOFF.md).
