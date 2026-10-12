use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use tokio::{process::Command, time::timeout};

use crate::{
    ADAPTER_CONTRACT_V1, AdapterFuture, AuthenticationEvidence, AuthenticationState, CommandSpec,
    DiscoveryEnvironment, ExecutableCandidate, ProviderAdapter, ProviderCapabilities,
    ProviderError, ProviderEvent, ProviderKind, ProviderProbe, ProviderSession,
    ProviderVerification, SessionRequest, TurnRequest,
    acp::{AcpHandshake, AcpSession, initialize_acp, start_acp_session},
    json_rpc::JsonRpcProcess,
};

const OPENCODE_PROTOCOL: &str = "acp/v1";
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_VERSION_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default)]
pub struct OpenCodeAdapter {
    executable: Option<PathBuf>,
}

impl OpenCodeAdapter {
    #[must_use]
    pub fn with_executable(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: Some(executable.into()),
        }
    }

    #[must_use]
    pub fn command(executable: &Path, request: &SessionRequest) -> CommandSpec {
        CommandSpec::new(executable, &request.cwd).arg("acp")
    }

    fn executable_candidates(
        &self,
        environment: &DiscoveryEnvironment,
    ) -> Vec<ExecutableCandidate> {
        self.executable.as_ref().map_or_else(
            || environment.candidates(ProviderKind::OpenCode, &["opencode.exe", "opencode"]),
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
            "OpenCode ACP initialization",
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

    async fn start_session(
        &self,
        request: &SessionRequest,
    ) -> Result<OpenCodeSession, ProviderError> {
        let environment = DiscoveryEnvironment::current();
        let candidate = self
            .executable_candidates(&environment)
            .into_iter()
            .next()
            .ok_or(ProviderError::NotDetected(ProviderKind::OpenCode))?;
        let (session, _) = start_acp_session(
            ProviderKind::OpenCode,
            Self::command(&candidate.path, request),
            request,
        )
        .await?;
        Ok(OpenCodeSession(session))
    }
}

impl ProviderAdapter for OpenCodeAdapter {
    type Session = OpenCodeSession;

    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenCode
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
                return Ok(ProviderProbe::not_detected(ProviderKind::OpenCode));
            };
            Ok(self.probe_candidate(candidate).await)
        })
    }

    fn start<'a>(&'a self, request: &'a SessionRequest) -> AdapterFuture<'a, Self::Session> {
        Box::pin(async move { self.start_session(request).await })
    }
}

#[derive(Debug)]
pub struct OpenCodeSession(AcpSession);

impl ProviderSession for OpenCodeSession {
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

async fn read_version(path: &Path) -> Result<String, ProviderError> {
    let output = timeout(
        VERSION_TIMEOUT,
        Command::new(path).arg("--version").output(),
    )
    .await
    .map_err(|_| ProviderError::Timeout {
        phase: "OpenCode version probe",
    })??;
    if !output.status.success() {
        return Err(ProviderError::Protocol(format!(
            "OpenCode version command exited with code {:?}",
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
            "OpenCode version output exceeded the limit".to_owned(),
        ));
    }
    let version = String::from_utf8_lossy(bytes).trim().to_owned();
    if version.is_empty() {
        Err(ProviderError::Protocol(
            "OpenCode version command returned empty output".to_owned(),
        ))
    } else {
        Ok(version)
    }
}

fn capabilities(handshake: &AcpHandshake) -> ProviderCapabilities {
    ProviderCapabilities {
        session: true,
        stream: true,
        cancel: true,
        resume: handshake.response.agent_capabilities.load_session,
        terminal: false,
        permission: true,
        mcp: false,
        plugins: false,
        skills: false,
        connectors: false,
        native_message: false,
        model_selection: false,
    }
}

fn configured_probe(
    candidate: ExecutableCandidate,
    version: String,
    handshake: &AcpHandshake,
) -> ProviderProbe {
    let authentication = AuthenticationEvidence {
        native: AuthenticationState::Unknown,
        external: AuthenticationState::Unknown,
        detail: Some(
            "ACP initialization succeeded; model-backed authentication was not exercised"
                .to_owned(),
        ),
    };
    let mut evidence = vec![
        "version command completed".to_owned(),
        "ACP initialize handshake completed".to_owned(),
        "model execution was not attempted by this probe".to_owned(),
    ];
    if let Some(agent_info) = handshake.raw.get("agentInfo")
        && let Some(name) = agent_info.get("name").and_then(serde_json::Value::as_str)
    {
        evidence.push(format!("ACP agent: {name}"));
    }
    if !handshake.response.auth_methods.is_empty() {
        evidence.push("ACP advertised one or more native authentication methods".to_owned());
    }
    ProviderProbe {
        contract_version: ADAPTER_CONTRACT_V1,
        provider: ProviderKind::OpenCode,
        verification: ProviderVerification::Unverified,
        executable: Some(candidate.path),
        discovery_source: Some(candidate.source),
        version: Some(version),
        protocol: Some(OPENCODE_PROTOCOL.to_owned()),
        capabilities: capabilities(handshake),
        authentication,
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
        provider: ProviderKind::OpenCode,
        verification: ProviderVerification::Degraded,
        executable: Some(candidate.path),
        discovery_source: Some(candidate.source),
        version,
        protocol: Some(OPENCODE_PROTOCOL.to_owned()),
        capabilities: ProviderCapabilities::default(),
        authentication: AuthenticationEvidence::unknown(),
        degradation_reason: Some(reason),
        evidence: Vec::new(),
        checked_at: chrono::Utc::now().to_rfc3339(),
    }
}
