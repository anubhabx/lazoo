mod acp;
mod codex;
mod contract;
mod discovery;
mod grok;
mod json_rpc;
mod opencode;
mod redaction;

pub use acp::{ProgressDeadline, normalize_acp_message};
pub use codex::{CodexAdapter, CodexSession, normalize_codex_message};
pub use contract::{
    ADAPTER_CONTRACT_V1, AdapterFuture, AuthenticationEvidence, AuthenticationState, CommandSpec,
    ExecutableCandidate, ProviderAdapter, ProviderCapabilities, ProviderError, ProviderEvent,
    ProviderKind, ProviderProbe, ProviderSession, ProviderVerification, SessionRequest,
    TurnRequest,
};
pub use discovery::{DiscoveryEnvironment, DiscoverySource, rank_candidates};
pub use grok::{GrokAdapter, GrokSession};
pub use opencode::{OpenCodeAdapter, OpenCodeSession};
pub use redaction::redact_json;
