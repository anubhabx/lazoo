use std::{
    collections::BTreeMap, ffi::OsString, fmt, future::Future, path::PathBuf, pin::Pin,
    str::FromStr, time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{DiscoveryEnvironment, DiscoverySource};

pub const ADAPTER_CONTRACT_V1: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    Codex,
    #[serde(rename = "opencode")]
    OpenCode,
    GrokBuild,
    Antigravity,
    ClaudeCode,
}

impl ProviderKind {
    pub const ALL: [Self; 5] = [
        Self::Codex,
        Self::OpenCode,
        Self::GrokBuild,
        Self::Antigravity,
        Self::ClaudeCode,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
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
            "grok-build" | "grok" => Ok(Self::GrokBuild),
            "antigravity" | "agy" => Ok(Self::Antigravity),
            "claude-code" | "claude" => Ok(Self::ClaudeCode),
            _ => Err(ProviderError::UnknownProvider(value.to_owned())),
        }
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: BTreeMap<OsString, OsString>,
}

impl CommandSpec {
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn arg(mut self, value: impl Into<OsString>) -> Self {
        self.args.push(value.into());
        self
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionRequest {
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub workspace_roots: Vec<PathBuf>,
    pub startup_timeout_millis: u64,
    pub progress_timeout_millis: u64,
    pub turn_timeout_millis: u64,
}

impl SessionRequest {
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            session_id: None,
            model: None,
            effort: None,
            permission_mode: None,
            workspace_roots: Vec::new(),
            startup_timeout_millis: 15_000,
            progress_timeout_millis: 30_000,
            turn_timeout_millis: 300_000,
        }
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
}

impl TurnRequest {
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            correlation_id: Uuid::new_v4(),
            prompt: prompt.into(),
        }
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
    Usage {
        provider: ProviderKind,
        session_id: String,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
        cost_usd: Option<f64>,
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

pub trait TurnEventObserver: Send + Sync {
    fn observe(&self, event: &ProviderEvent) -> Result<(), ProviderError>;
}

pub trait ProviderSession: Send + Sync {
    fn id(&self) -> String;
    fn native_host_pid(&self) -> Option<u32>;
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
    #[error("provider does not support {capability}: {provider}")]
    Unsupported {
        provider: ProviderKind,
        capability: &'static str,
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
