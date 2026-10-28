mod acp;
mod antigravity;
mod claude;
mod codex;
mod codex_broker;
mod codex_view;
mod config;
mod config_apply;
mod contract;
mod discovery;
mod grok;
mod json_rpc;
mod ndjson;
mod opencode;
mod preview;
mod redaction;
mod registry;

pub use acp::{ProgressDeadline, normalize_acp_message};
pub use antigravity::{AntigravityAdapter, AntigravitySession, normalize_antigravity_message};
pub use claude::{ClaudeAdapter, ClaudeExternalAuth, ClaudeSession, normalize_claude_message};
pub use codex::{
    CodexAdapter, CodexSession, normalize_codex_message, normalize_codex_permission_request,
    normalize_codex_tool_call,
};
pub use codex_view::{NativeViewLaunchDescriptor, NativeViewLease};
pub use config::{
    ConfigurationFormat, DiscoveredIntegration, IntegrationKind, IntegrationTransport,
    PROVIDER_CONFIGURATION_V1, ProviderConfigurationFile, ProviderConfigurationReport,
    discover_provider_configuration,
};
pub use config_apply::{
    ConfigurationApplyError, ConfigurationApplyPreview, ConfigurationApplyReceipt,
    ConfigurationApplyStatus, ConfigurationCompatibility, ConfigurationPreviewMetadata,
    ConfigurationRecoveryState, PROVIDER_CONFIGURATION_APPLY_V1, ProviderVersionValidation,
    SelectedMcpIntegration, apply_configuration_preview, apply_configuration_preview_with_intent,
    inspect_configuration_apply_recovery, preview_configuration_apply,
    rollback_configuration_apply,
};
pub use contract::{
    ADAPTER_CONTRACT_V1, AdapterFuture, AuthenticationEvidence, AuthenticationState, CommandSpec,
    ExecutableCandidate, PermissionDecision, PermissionKind, PermissionRequest, ProviderAdapter,
    ProviderCapabilities, ProviderError, ProviderEvent, ProviderKind, ProviderProbe,
    ProviderSession, ProviderVerification, SessionRequest, ToolCall, ToolResult, ToolSpec,
    TurnEventObserver, TurnRequest, attach_args,
};
pub use discovery::{DiscoveryEnvironment, DiscoverySource, rank_candidates};
pub use grok::{GrokAdapter, GrokSession};
pub use opencode::{OpenCodeAdapter, OpenCodeSession};
pub use preview::{
    MigrationCandidate, MigrationPreview, MigrationProposal, MigrationSource, MigrationSummary,
    PROVIDER_MIGRATION_PREVIEW_V1, migration_preview,
};
pub use redaction::redact_json;
pub use registry::{ProviderDiscovery, ProviderRegistry, ProviderSessionHandle};
