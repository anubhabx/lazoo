use std::{
    collections::BTreeMap, ffi::OsString, fmt, future::Future, path::PathBuf, pin::Pin,
    str::FromStr, time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use lazoo_runtime::JobAccounting;

use crate::{DiscoveryEnvironment, DiscoverySource};

pub const ADAPTER_CONTRACT_V1: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    Codex,
    #[serde(rename = "opencode")]
    OpenCode,
    #[serde(rename = "github-copilot")]
    GitHubCopilot,
    GrokBuild,
    Antigravity,
    ClaudeCode,
}

impl ProviderKind {
    pub const ALL: [Self; 6] = [
        Self::Codex,
        Self::OpenCode,
        Self::GitHubCopilot,
        Self::GrokBuild,
        Self::Antigravity,
        Self::ClaudeCode,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::GitHubCopilot => "github-copilot",
            Self::GrokBuild => "grok-build",
            Self::Antigravity => "antigravity",
            Self::ClaudeCode => "claude-code",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ProviderKind {
    type Err = ProviderError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::OpenCode),
            "github-copilot" | "copilot" => Ok(Self::GitHubCopilot),
            "grok-build" | "grok" => Ok(Self::GrokBuild),
            "antigravity" | "agy" => Ok(Self::Antigravity),
            "claude-code" | "claude" => Ok(Self::ClaudeCode),
            _ => Err(ProviderError::UnknownProvider(value.to_owned())),
        }
    }
}

/// The argv that reopens `native_session_id` in the provider's own interactive TUI, for the
/// owner's terminal takeover of a suspended session. The shapes come from the installed CLIs'
/// help: Claude `--resume`, Codex `resume`, `OpenCode` `--session`, Antigravity `--conversation`.
/// Takeover resumes stored native history; it does not submit a model turn. `None` means no supported
/// takeover command, or the native identity has not yet been bound by its authenticated session.
#[must_use]
pub fn attach_args(provider: ProviderKind, native_session_id: &str) -> Option<Vec<String>> {
    if native_session_id.is_empty() || native_session_id.starts_with("pending:") {
        return None;
    }
    let id = native_session_id.to_owned();
    match provider {
        ProviderKind::ClaudeCode => Some(vec!["--resume".to_owned(), id]),
        ProviderKind::Codex => Some(vec!["resume".to_owned(), id]),
        ProviderKind::OpenCode => Some(vec!["--session".to_owned(), id]),
        ProviderKind::GitHubCopilot => Some(vec![format!("--resume={id}")]),
        ProviderKind::Antigravity => Some(vec!["--conversation".to_owned(), id]),
        ProviderKind::GrokBuild => None,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderVerification {
    NotDetected,
    Detected,
    Configured,
    Degraded,
    Unverified,
    Verified,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthenticationState {
    #[default]
    Unknown,
    Ready,
    NeedsLogin,
    Blocked,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AuthenticationEvidence {
    pub native: AuthenticationState,
    pub external: AuthenticationState,
    pub detail: Option<String>,
}

impl AuthenticationEvidence {
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            native: AuthenticationState::Unknown,
            external: AuthenticationState::Unknown,
            detail: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Capability truth is intentionally an explicit bitmap.
pub struct ProviderCapabilities {
    pub session: bool,
    pub stream: bool,
    pub cancel: bool,
    pub resume: bool,
    pub terminal: bool,
    pub permission: bool,
    pub mcp: bool,
    pub plugins: bool,
    pub skills: bool,
    pub connectors: bool,
    pub native_message: bool,
    pub model_selection: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExecutableCandidate {
    pub path: PathBuf,
    pub source: DiscoverySource,
}

impl ExecutableCandidate {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, source: DiscoverySource) -> Self {
        Self {
            path: path.into(),
            source,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderProbe {
    pub contract_version: u32,
    pub provider: ProviderKind,
    pub verification: ProviderVerification,
    pub executable: Option<PathBuf>,
    pub discovery_source: Option<DiscoverySource>,
    pub version: Option<String>,
    pub protocol: Option<String>,
    pub capabilities: ProviderCapabilities,
    pub authentication: AuthenticationEvidence,
    pub degradation_reason: Option<String>,
    pub evidence: Vec<String>,
    pub checked_at: String,
}

impl ProviderProbe {
    #[must_use]
    pub fn not_detected(provider: ProviderKind) -> Self {
        Self {
            contract_version: ADAPTER_CONTRACT_V1,
            provider,
            verification: ProviderVerification::NotDetected,
            executable: None,
            discovery_source: None,
            version: None,
            protocol: None,
            capabilities: ProviderCapabilities::default(),
            authentication: AuthenticationEvidence::unknown(),
            degradation_reason: Some("provider executable was not detected".to_owned()),
            evidence: Vec::new(),
            checked_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct CommandSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: BTreeMap<OsString, OsString>,
    pub env_remove: Vec<OsString>,
}

impl fmt::Debug for CommandSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandSpec")
            .field("executable", &self.executable)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("env_remove", &self.env_remove)
            .field(
                "env",
                &self
                    .env
                    .keys()
                    .map(|name| (name, "<redacted>"))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl CommandSpec {
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: BTreeMap::new(),
            env_remove: Vec::new(),
        }
    }

    #[must_use]
    pub fn arg(mut self, value: impl Into<OsString>) -> Self {
        self.args.push(value.into());
        self
    }

    pub fn apply_environment(&mut self, environment: &lazoo_runtime::LaunchEnvironment) {
        self.env_remove.extend_from_slice(environment.removals());
        self.env.extend(environment.values().clone());
    }
}

/// One Lazoo-owned tool declared to a provider session. Adapters expose it through the provider's
/// native tool mechanism; calls come back through `TurnEventObserver::call_tool`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionRequest {
    /// Credentials and profile selection supplied only by the trusted caller. Never on the wire.
    #[serde(skip)]
    pub launch_environment: lazoo_runtime::LaunchEnvironment,
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub workspace_roots: Vec<PathBuf>,
    pub startup_timeout_millis: u64,
    pub progress_timeout_millis: u64,
    pub turn_timeout_millis: u64,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    /// The caller owns this isolated workspace and permits temporary native hook configuration.
    /// This is never inferred from a file supplied by the provider or repository.
    #[serde(default)]
    pub managed_workspace: bool,
    /// A separate, explicit workspace-trust grant issued by the caller's consent ledger.
    /// Workspace ownership or tool autonomy alone never grants native workspace trust.
    #[serde(default)]
    pub native_workspace_trust_approved: bool,
}

impl SessionRequest {
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            launch_environment: lazoo_runtime::LaunchEnvironment::default(),
            cwd: cwd.into(),
            session_id: None,
            model: None,
            effort: None,
            permission_mode: None,
            workspace_roots: Vec::new(),
            startup_timeout_millis: 15_000,
            progress_timeout_millis: 30_000,
            turn_timeout_millis: 300_000,
            tools: Vec::new(),
            managed_workspace: false,
            native_workspace_trust_approved: false,
        }
    }

    #[must_use]
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    #[must_use]
    pub const fn with_managed_workspace(mut self) -> Self {
        self.managed_workspace = true;
        self
    }

    #[must_use]
    pub const fn with_native_workspace_trust_approved(mut self) -> Self {
        self.native_workspace_trust_approved = true;
        self
    }

    #[must_use]
    pub fn resume(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    #[must_use]
    pub fn with_effort(mut self, effort: impl Into<String>) -> Self {
        self.effort = Some(effort.into());
        self
    }

    #[must_use]
    pub fn with_permission_mode(mut self, mode: impl Into<String>) -> Self {
        self.permission_mode = Some(mode.into());
        self
    }

    #[must_use]
    pub const fn startup_timeout(&self) -> Duration {
        Duration::from_millis(self.startup_timeout_millis)
    }

    #[must_use]
    pub const fn progress_timeout(&self) -> Duration {
        Duration::from_millis(self.progress_timeout_millis)
    }

    #[must_use]
    pub const fn turn_timeout(&self) -> Duration {
        Duration::from_millis(self.turn_timeout_millis)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TurnRequest {
    pub correlation_id: Uuid,
    pub prompt: String,
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub read_only: bool,
}

impl TurnRequest {
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            correlation_id: Uuid::new_v4(),
            prompt: prompt.into(),
            permission_mode: None,
            read_only: false,
        }
    }

    #[must_use]
    pub fn with_permission_mode(mut self, mode: impl Into<String>) -> Self {
        self.permission_mode = Some(mode.into());
        self
    }

    #[must_use]
    pub const fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum ProviderEvent {
    SessionStarted {
        provider: ProviderKind,
        session_id: String,
        resumed: bool,
        raw: Option<Value>,
    },
    TurnStarted {
        provider: ProviderKind,
        session_id: String,
        turn_id: Option<String>,
        raw: Option<Value>,
    },
    TextDelta {
        provider: ProviderKind,
        session_id: String,
        turn_id: Option<String>,
        text: String,
        raw: Option<Value>,
    },
    ThoughtDelta {
        provider: ProviderKind,
        session_id: String,
        turn_id: Option<String>,
        text: String,
        raw: Option<Value>,
    },
    ToolUpdate {
        provider: ProviderKind,
        session_id: String,
        turn_id: Option<String>,
        tool_name: Option<String>,
        status: String,
        raw: Option<Value>,
    },
    TerminalUpdate {
        provider: ProviderKind,
        session_id: String,
        terminal_id: Option<String>,
        output: Option<String>,
        raw: Option<Value>,
    },
    PermissionRequested {
        provider: ProviderKind,
        session_id: String,
        request_id: Option<String>,
        options: Vec<String>,
        raw: Option<Value>,
    },
    Progress {
        provider: ProviderKind,
        session_id: String,
        message: Option<String>,
        raw: Option<Value>,
    },
    Quota {
        provider: ProviderKind,
        session_id: String,
        windows: Vec<lazoo_protocol::UsageWindow>,
        exhausted: bool,
        raw: Option<Value>,
    },
    Usage {
        provider: ProviderKind,
        session_id: String,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
        cost_usd: Option<f64>,
        /// Known history before the first observed report (e.g. a resumed Codex thread).
        #[serde(default)]
        input_tokens_before: Option<u64>,
        #[serde(default)]
        output_tokens_before: Option<u64>,
        raw: Option<Value>,
    },
    TurnCompleted {
        provider: ProviderKind,
        session_id: String,
        turn_id: Option<String>,
        status: String,
        output: Option<String>,
        raw: Option<Value>,
    },
    Cancelled {
        provider: ProviderKind,
        session_id: String,
        turn_id: Option<String>,
        raw: Option<Value>,
    },
    Error {
        provider: ProviderKind,
        session_id: Option<String>,
        code: String,
        message: String,
        retryable: bool,
        raw: Option<Value>,
    },
    ProcessExited {
        provider: ProviderKind,
        session_id: Option<String>,
        code: Option<i32>,
    },
}

pub type AdapterFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ProviderError>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionKind {
    CommandExecution,
    FileChange,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionDecision {
    Approve,
    Decline,
}

/// A provider-neutral approval request. Adapters normalize their native approval protocol into
/// this shape; the control plane decides and the adapter answers the provider.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PermissionRequest {
    pub provider: ProviderKind,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub request_id: String,
    pub kind: PermissionKind,
    pub action: String,
    pub scope_expansion: bool,
    pub detail: Value,
}

/// One invocation of a declared Lazoo tool by the provider model. Arguments are bounded and
/// redacted by the adapter before they reach the control plane.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolCall {
    pub provider: ProviderKind,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

/// The control plane's answer to a tool call. `content` is serialized as text for the provider;
/// `success = false` tells the model the call was refused or failed without ending the turn.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolResult {
    pub success: bool,
    pub content: Value,
}

impl ToolResult {
    #[must_use]
    pub fn success(content: Value) -> Self {
        Self {
            success: true,
            content,
        }
    }

    #[must_use]
    pub fn failure(code: &str, message: impl Into<String>) -> Self {
        Self {
            success: false,
            content: serde_json::json!({"error": {"code": code, "message": message.into()}}),
        }
    }
}

pub trait TurnEventObserver: Send + Sync {
    fn observe(&self, event: &ProviderEvent) -> Result<(), ProviderError>;

    /// Governs one provider approval request. The default keeps the historical fail-closed
    /// behavior: an observer that does not attach an approval flow declines every request.
    fn decide_permission<'a>(
        &'a self,
        _request: &'a PermissionRequest,
    ) -> AdapterFuture<'a, PermissionDecision> {
        Box::pin(async { Ok(PermissionDecision::Decline) })
    }

    /// Answers one Lazoo tool call. The default fails closed: an observer that does not attach
    /// the control plane refuses every call without ending the provider turn.
    fn call_tool<'a>(&'a self, _call: &'a ToolCall) -> AdapterFuture<'a, ToolResult> {
        Box::pin(async {
            Ok(ToolResult::failure(
                "LZ-TOOL-UNATTACHED",
                "Lazoo collaboration tools are not attached to this turn",
            ))
        })
    }
}

pub trait ProviderSession: Send + Sync {
    /// Drain passive observations already received locally; never calls the provider.
    fn take_usage_windows(&self) -> Vec<lazoo_protocol::UsageWindow> {
        Vec::new()
    }
    fn id(&self) -> String;
    fn native_host_pid(&self) -> Option<u32>;
    /// A native read-only view of this exact running session, without another controller.
    fn native_view_available(&self) -> bool {
        false
    }
    fn open_native_view(&self) -> AdapterFuture<'_, Option<crate::NativeViewLease>> {
        Box::pin(async { Ok(None) })
    }
    /// Job Object accounting for the session's native process tree; `None` when the session
    /// owns no job.
    fn resources(&self) -> Option<JobAccounting> {
        None
    }
    fn turn<'a>(&'a self, request: &'a TurnRequest) -> AdapterFuture<'a, Vec<ProviderEvent>>;
    fn turn_observed<'a>(
        &'a self,
        request: &'a TurnRequest,
        observer: &'a dyn TurnEventObserver,
    ) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        Box::pin(async move {
            let events = self.turn(request).await?;
            for event in &events {
                observer.observe(event)?;
            }
            Ok(events)
        })
    }
    fn cancel(&self) -> AdapterFuture<'_, ()>;
    fn stop(&self) -> AdapterFuture<'_, ()>;
}

pub trait ProviderAdapter: Send + Sync {
    type Session: ProviderSession;

    fn kind(&self) -> ProviderKind;
    fn discover(&self, environment: &DiscoveryEnvironment) -> Vec<ExecutableCandidate>;
    fn probe<'a>(
        &'a self,
        environment: &'a DiscoveryEnvironment,
    ) -> AdapterFuture<'a, ProviderProbe>;
    fn start<'a>(&'a self, request: &'a SessionRequest) -> AdapterFuture<'a, Self::Session>;
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("provider executable was not detected: {0}")]
    NotDetected(ProviderKind),
    /// `detail` names the configuration that triggered a refusal (a path or variable name,
    /// never its contents) so the owner can fix it.
    #[error("provider does not support {capability}: {provider}{}", .detail.as_ref().map(|detail| format!(" ({detail})")).unwrap_or_default())]
    Unsupported {
        provider: ProviderKind,
        capability: &'static str,
        detail: Option<String>,
    },
    #[error("provider authentication is not ready: {0}")]
    Authentication(String),
    #[error("provider protocol error: {0}")]
    Protocol(String),
    #[error("provider returned an invalid event: {0}")]
    InvalidEvent(String),
    #[error("provider timed out during {phase}")]
    Timeout { phase: &'static str },
    #[error("provider accepted work but emitted no progress for {millis} ms")]
    Stalled { millis: u64 },
    #[error("provider turn observer failed: {0}")]
    Observer(String),
    #[error("provider process exited unexpectedly with code {code:?}")]
    ProcessExited { code: Option<i32> },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Runtime(#[from] lazoo_runtime::RuntimeError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
