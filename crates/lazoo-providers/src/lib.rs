mod acp;
mod antigravity;
mod claude;
mod codex;
mod contract;
mod discovery;
mod grok;
mod json_rpc;
mod ndjson;
mod opencode;
mod redaction;
mod registry;

pub use acp::{ProgressDeadline, normalize_acp_message};
pub use antigravity::{AntigravityAdapter, AntigravitySession, normalize_antigravity_message};
pub use claude::{ClaudeAdapter, ClaudeExternalAuth, ClaudeSession, normalize_claude_message};
pub use codex::{
    CodexAdapter, CodexSession, normalize_codex_message, normalize_codex_permission_request,
    normalize_codex_tool_call,
};
pub use contract::{
    ADAPTER_CONTRACT_V1, AdapterFuture, AuthenticationEvidence, AuthenticationState, CommandSpec,
    ExecutableCandidate, PermissionDecision, PermissionKind, PermissionRequest, ProviderAdapter,
    ProviderCapabilities, ProviderError, ProviderEvent, ProviderKind, ProviderProbe,
    ProviderSession, ProviderVerification, SessionRequest, ToolCall, ToolResult, ToolSpec,
    TurnEventObserver, TurnRequest,
};
pub use discovery::{DiscoveryEnvironment, DiscoverySource, rank_candidates};
pub use grok::{GrokAdapter, GrokSession};
pub use opencode::{OpenCodeAdapter, OpenCodeSession};
pub use redaction::redact_json;
pub use registry::{ProviderDiscovery, ProviderRegistry, ProviderSessionHandle};
