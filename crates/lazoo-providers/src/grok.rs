use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::Value;
use tokio::{process::Command, time::timeout};

use crate::{
    ADAPTER_CONTRACT_V1, AdapterFuture, AuthenticationEvidence, AuthenticationState, CommandSpec,
    DiscoveryEnvironment, ExecutableCandidate, ProviderAdapter, ProviderCapabilities,
    ProviderError, ProviderEvent, ProviderKind, ProviderProbe, ProviderSession,
    ProviderVerification, SessionRequest, TurnRequest,
    acp::{AcpHandshake, AcpSession, initialize_acp, start_acp_session},
    json_rpc::JsonRpcProcess,
};

const GROK_PROTOCOL: &str = "acp/v1";
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_VERSION_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default)]
pub struct GrokAdapter {
    executable: Option<PathBuf>,
}

impl GrokAdapter {
    #[must_use]
    pub fn with_executable(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: Some(executable.into()),
        }
    }

    #[must_use]
    pub fn command(executable: &Path, request: &SessionRequest) -> CommandSpec {
        let mut command = CommandSpec::new(executable, &request.cwd)
            .arg("--permission-mode")
            .arg(permission_mode(request.permission_mode.as_deref()));
        if let Some(model) = &request.model {
            command = command.arg("--model").arg(model);
        }
        if let Some(effort) = &request.effort {
            command = command.arg("--reasoning-effort").arg(effort);
        }
        command.arg("agent").arg("stdio")
    }

    #[must_use]
    pub fn capabilities(agent_capabilities: &Value) -> ProviderCapabilities {
        ProviderCapabilities {
            session: true,
            stream: true,
            cancel: true,
            resume: agent_capabilities
                .get("loadSession")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            terminal: true,
            permission: true,
            mcp: false,
            plugins: false,
            skills: false,
            connectors: false,
            native_message: false,
            model_selection: true,
        }
    }

    fn executable_candidates(
        &self,
        environment: &DiscoveryEnvironment,
    ) -> Vec<ExecutableCandidate> {
        self.executable.as_ref().map_or_else(
            || environment.candidates(ProviderKind::GrokBuild, &["grok.exe", "grok"]),
            |path| {
                vec![ExecutableCandidate::new(
                    path.clone(),
                    crate::DiscoverySource::KnownLocation,
                )]
            },
        )
    }

    async fn probe_candidate(&self, candidate: ExecutableCandidate) -> ProviderProbe {
        let version = match read_version(&candidate.path).await {
            Ok(version) => version,
            Err(error) => {
                return degraded_probe(candidate, None, format!("version probe failed: {error}"));
            }
        };
        let cwd = match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(error) => {
                return degraded_probe(
                    candidate,
                    Some(version),
                    format!("current directory was unavailable: {error}"),
                );
            }
        };
        let request = SessionRequest::new(cwd);
        let mut process =
            match JsonRpcProcess::spawn_standard(&Self::command(&candidate.path, &request)) {
                Ok(process) => process,
                Err(error) => {
                    return degraded_probe(
                        candidate,
                        Some(version),
                        format!("ACP spawn failed: {error}"),
                    );
                }
            };
        let handshake = initialize_acp(
            &mut process,
            request.startup_timeout(),
            "Grok Build ACP initialization",
        )
        .await;
        let stderr = process.stderr_tail().await;
        let _ = process.stop(STOP_TIMEOUT).await;
        match handshake {
            Ok(handshake) => configured_probe(candidate, version, &handshake),
            Err(error) => {
                let suffix = if stderr.is_empty() {
                    String::new()
                } else {
                    " (provider stderr was captured but omitted from public evidence)".to_owned()
                };
                degraded_probe(
                    candidate,
                    Some(version),
                    format!("ACP handshake failed: {error}{suffix}"),
                )
            }
        }
    }

    async fn start_session(&self, request: &SessionRequest) -> Result<GrokSession, ProviderError> {
        validate_permission_mode(request.permission_mode.as_deref())?;
        let environment = DiscoveryEnvironment::current();
        let candidate = self
            .executable_candidates(&environment)
            .into_iter()
            .next()
            .ok_or(ProviderError::NotDetected(ProviderKind::GrokBuild))?;
        let (session, _) = start_acp_session(
            ProviderKind::GrokBuild,
            Self::command(&candidate.path, request),
            request,
        )
        .await?;
        Ok(GrokSession(session))
    }
}

impl ProviderAdapter for GrokAdapter {
    type Session = GrokSession;

    fn kind(&self) -> ProviderKind {
        ProviderKind::GrokBuild
    }

    fn discover(&self, environment: &DiscoveryEnvironment) -> Vec<ExecutableCandidate> {
        self.executable_candidates(environment)
    }

    fn probe<'a>(
        &'a self,
        environment: &'a DiscoveryEnvironment,
    ) -> AdapterFuture<'a, ProviderProbe> {
        Box::pin(async move {
            let Some(candidate) = self.executable_candidates(environment).into_iter().next() else {
                return Ok(ProviderProbe::not_detected(ProviderKind::GrokBuild));
            };
            Ok(self.probe_candidate(candidate).await)
        })
    }

    fn start<'a>(&'a self, request: &'a SessionRequest) -> AdapterFuture<'a, Self::Session> {
        Box::pin(async move { self.start_session(request).await })
    }
}

#[derive(Debug)]
pub struct GrokSession(AcpSession);

impl ProviderSession for GrokSession {
    fn id(&self) -> String {
        self.0.id()
    }

    fn native_host_pid(&self) -> Option<u32> {
        self.0.native_host_pid()
    }

    fn resources(&self) -> Option<lazoo_runtime::JobAccounting> {
        self.0.resources()
    }

    fn turn<'a>(&'a self, request: &'a TurnRequest) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        self.0.turn(request)
    }

    fn cancel(&self) -> AdapterFuture<'_, ()> {
        self.0.cancel()
    }

    fn stop(&self) -> AdapterFuture<'_, ()> {
        self.0.stop()
    }
}

fn permission_mode(mode: Option<&str>) -> &'static str {
    match mode {
        Some("ask" | "default" | "on-request") => "default",
        Some("auto") => "auto",
        Some("accept-edits" | "acceptEdits") => "acceptEdits",
        Some("plan") => "plan",
        _ => "dontAsk",
    }
}

fn validate_permission_mode(mode: Option<&str>) -> Result<(), ProviderError> {
    match mode {
        None
        | Some(
            "ask" | "default" | "on-request" | "auto" | "accept-edits" | "acceptEdits" | "plan"
            | "never" | "dontAsk" | "untrusted",
        ) => Ok(()),
        Some(mode) => Err(ProviderError::Protocol(format!(
            "unsupported Grok permission mode: {mode}"
        ))),
    }
}

async fn read_version(path: &Path) -> Result<String, ProviderError> {
    let output = timeout(
        VERSION_TIMEOUT,
        Command::new(path).arg("--version").output(),
    )
    .await
    .map_err(|_| ProviderError::Timeout {
        phase: "Grok Build version probe",
    })??;
    if !output.status.success() {
        return Err(ProviderError::Protocol(format!(
            "Grok Build version command exited with code {:?}",
            output.status.code()
        )));
    }
    let bytes = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    if bytes.len() > MAX_VERSION_BYTES {
        return Err(ProviderError::Protocol(
            "Grok Build version output exceeded the limit".to_owned(),
        ));
    }
    let version = String::from_utf8_lossy(bytes).trim().to_owned();
    if version.is_empty() {
        Err(ProviderError::Protocol(
            "Grok Build version command returned empty output".to_owned(),
        ))
    } else {
        Ok(version)
    }
}

fn configured_probe(
    candidate: ExecutableCandidate,
    version: String,
    handshake: &AcpHandshake,
) -> ProviderProbe {
    let agent_capabilities = handshake
        .raw
        .get("agentCapabilities")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let mut evidence = vec![
        "version command completed".to_owned(),
        "ACP initialize handshake completed".to_owned(),
        "permission baseline was forced to dontAsk for this probe".to_owned(),
        "model execution was not attempted by this probe".to_owned(),
    ];
    if let Some(agent_info) = handshake.raw.get("agentInfo")
        && let Some(name) = agent_info.get("name").and_then(Value::as_str)
    {
        evidence.push(format!("ACP agent: {name}"));
    }
    ProviderProbe {
        contract_version: ADAPTER_CONTRACT_V1,
        provider: ProviderKind::GrokBuild,
        verification: ProviderVerification::Unverified,
        executable: Some(candidate.path),
        discovery_source: Some(candidate.source),
        version: Some(version),
        protocol: Some(GROK_PROTOCOL.to_owned()),
        capabilities: GrokAdapter::capabilities(&agent_capabilities),
        authentication: AuthenticationEvidence {
            native: AuthenticationState::Unknown,
            external: AuthenticationState::Unknown,
            detail: Some(
                "ACP initialization succeeded; model-backed authentication was not exercised"
                    .to_owned(),
            ),
        },
        degradation_reason: None,
        evidence,
        checked_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn degraded_probe(
    candidate: ExecutableCandidate,
    version: Option<String>,
    reason: String,
) -> ProviderProbe {
    ProviderProbe {
        contract_version: ADAPTER_CONTRACT_V1,
        provider: ProviderKind::GrokBuild,
        verification: ProviderVerification::Degraded,
        executable: Some(candidate.path),
        discovery_source: Some(candidate.source),
        version,
        protocol: Some(GROK_PROTOCOL.to_owned()),
        capabilities: ProviderCapabilities::default(),
        authentication: AuthenticationEvidence::unknown(),
        degradation_reason: Some(reason),
        evidence: Vec::new(),
        checked_at: chrono::Utc::now().to_rfc3339(),
    }
}
