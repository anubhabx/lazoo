use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

use agent_client_protocol::schema::v1::{HttpHeader, McpServer, McpServerHttp};
use tokio::{process::Command, time::timeout};

use crate::{
    ADAPTER_CONTRACT_V1, AdapterFuture, AuthenticationEvidence, AuthenticationState, CommandSpec,
    DiscoveryEnvironment, ExecutableCandidate, ProviderAdapter, ProviderCapabilities,
    ProviderError, ProviderEvent, ProviderKind, ProviderProbe, ProviderSession,
    ProviderVerification, SessionRequest, TurnRequest,
    acp::{AcpHandshake, AcpSession, initialize_acp, start_acp_session_with_mcp},
    json_rpc::JsonRpcProcess,
    mcp_bridge::SessionMcpBridge,
};

const COPILOT_PROTOCOL: &str = "acp/v1";
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_VERSION_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default)]
pub struct CopilotAdapter {
    executable: Option<PathBuf>,
}

impl CopilotAdapter {
    #[must_use]
    pub fn with_executable(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: Some(executable.into()),
        }
    }

    /// Starts Copilot's documented stdio ACP server. Permission flags are deliberately absent:
    /// ACP permission requests remain governed by Lazoo's observer. MCP servers are scoped to the
    /// ACP session and never written to the owner's Copilot configuration.
    #[must_use]
    pub fn command(executable: &Path, request: &SessionRequest) -> CommandSpec {
        let mut command = CommandSpec::new(executable, &request.cwd)
            .arg("--acp")
            // The ACP host is Lazoo-owned. Do not let it update itself or export a session.
            .arg("--no-auto-update")
            .arg("--no-remote-export");
        for root in &request.workspace_roots {
            command = command.arg("--add-dir").arg(root.as_os_str());
        }
        if let Some(model) = &request.model {
            command = command.arg("--model").arg(model);
        }
        if let Some(effort) = &request.effort {
            command = command.arg("--effort").arg(effort);
        }
        command.apply_environment(&request.launch_environment);
        // An ambient opt-in bypasses Copilot's approval UI. Lazoo is the permission authority
        // for ACP sessions, so never inherit it from an interactive shell or personal profile.
        let allow_all = OsString::from("COPILOT_ALLOW_ALL");
        command.env.remove(&allow_all);
        command.env_remove.push(allow_all);
        command
    }

    fn executable_candidates(
        &self,
        environment: &DiscoveryEnvironment,
    ) -> Vec<ExecutableCandidate> {
        if let Some(path) = &self.executable {
            return vec![ExecutableCandidate::new(
                path.clone(),
                crate::DiscoverySource::KnownLocation,
            )];
        }
        let launchers = environment.candidates(
            ProviderKind::GitHubCopilot,
            &["copilot.exe", "copilot", "copilot.cmd"],
        );
        let mut candidates = launchers.clone();
        // npm's PATH entry is commonly the JavaScript launcher. Its sibling package directory
        // contains the platform binary; add it only after an existing launcher was found.
        for launcher in launchers {
            if let Some(directory) = launcher.path.parent() {
                let native = directory
                    .join("node_modules")
                    .join("@github")
                    .join("copilot")
                    .join("node_modules")
                    .join("@github")
                    .join("copilot-win32-x64")
                    .join("copilot.exe");
                if native.is_file() {
                    candidates.push(ExecutableCandidate::new(native, launcher.source));
                }
            }
        }
        candidates.sort_by_key(|candidate| {
            (
                !candidate.path.to_string_lossy().ends_with("copilot.exe"),
                candidate.path.clone(),
            )
        });
        candidates
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
            "GitHub Copilot ACP initialization",
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
    ) -> Result<CopilotSession, ProviderError> {
        let candidate = self
            .executable_candidates(&DiscoveryEnvironment::current())
            .into_iter()
            .next()
            .ok_or(ProviderError::NotDetected(ProviderKind::GitHubCopilot))?;
        let bridge =
            SessionMcpBridge::open(ProviderKind::GitHubCopilot, request.tools.clone()).await?;
        let mcp = McpServer::Http(McpServerHttp::new("lazoo", bridge.endpoint()).headers(vec![
            HttpHeader::new("Authorization", bridge.authorization()),
        ]));
        let started = start_acp_session_with_mcp(
            ProviderKind::GitHubCopilot,
            Self::command(&candidate.path, request),
            request,
            vec![mcp],
        )
        .await;
        match started {
            Ok((session, _)) => Ok(CopilotSession { session, bridge }),
            Err(error) => {
                bridge.close();
                Err(error)
            }
        }
    }
}

impl ProviderAdapter for CopilotAdapter {
    type Session = CopilotSession;
    fn kind(&self) -> ProviderKind {
        ProviderKind::GitHubCopilot
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
                return Ok(ProviderProbe::not_detected(ProviderKind::GitHubCopilot));
            };
            Ok(self.probe_candidate(candidate).await)
        })
    }
    fn start<'a>(&'a self, request: &'a SessionRequest) -> AdapterFuture<'a, Self::Session> {
        Box::pin(async move { self.start_session(request).await })
    }
}

#[derive(Debug)]
pub struct CopilotSession {
    session: AcpSession,
    bridge: SessionMcpBridge,
}

impl ProviderSession for CopilotSession {
    fn id(&self) -> String {
        self.session.id()
    }
    fn native_host_pid(&self) -> Option<u32> {
        self.session.native_host_pid()
    }
    fn resources(&self) -> Option<lazoo_runtime::JobAccounting> {
        self.session.resources()
    }
    fn turn<'a>(&'a self, request: &'a TurnRequest) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        self.session.turn(request)
    }
    fn turn_observed<'a>(
        &'a self,
        request: &'a TurnRequest,
        observer: &'a dyn crate::TurnEventObserver,
    ) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        Box::pin(async move {
            let session_id = self.session.id();
            let turn_id = request.correlation_id.to_string();
            self.bridge
                .during_turn(
                    &session_id,
                    Some(&turn_id),
                    observer,
                    self.session.turn_observed(request, observer),
                )
                .await
        })
    }
    fn cancel(&self) -> AdapterFuture<'_, ()> {
        self.session.cancel()
    }
    fn stop(&self) -> AdapterFuture<'_, ()> {
        Box::pin(async move {
            self.bridge.close();
            self.session.stop().await
        })
    }
}

async fn read_version(path: &Path) -> Result<String, ProviderError> {
    let output = timeout(
        VERSION_TIMEOUT,
        Command::new(path).arg("--version").output(),
    )
    .await
    .map_err(|_| ProviderError::Timeout {
        phase: "GitHub Copilot version probe",
    })??;
    if !output.status.success() {
        return Err(ProviderError::Protocol(format!(
            "GitHub Copilot version command exited with code {:?}",
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
            "GitHub Copilot version output exceeded the limit".to_owned(),
        ));
    }
    let version = String::from_utf8_lossy(bytes).trim().to_owned();
    if version.is_empty() {
        Err(ProviderError::Protocol(
            "GitHub Copilot version command returned empty output".to_owned(),
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
        mcp: handshake.response.agent_capabilities.mcp_capabilities.http,
        plugins: false,
        skills: false,
        connectors: false,
        native_message: false,
        model_selection: true,
    }
}

fn configured_probe(
    candidate: ExecutableCandidate,
    version: String,
    handshake: &AcpHandshake,
) -> ProviderProbe {
    let mut evidence = vec![
        "version command completed".to_owned(),
        "ACP initialize handshake completed".to_owned(),
        "model execution was not attempted by this probe".to_owned(),
    ];
    if let Some(name) = handshake
        .raw
        .get("agentInfo")
        .and_then(|v| v.get("name"))
        .and_then(serde_json::Value::as_str)
    {
        evidence.push(format!("ACP agent: {name}"));
    }
    if !handshake.response.auth_methods.is_empty() {
        evidence.push("ACP advertised one or more native authentication methods".to_owned());
    }
    let mut probe = ProviderProbe {
        contract_version: ADAPTER_CONTRACT_V1,
        provider: ProviderKind::GitHubCopilot,
        verification: ProviderVerification::Unverified,
        executable: Some(candidate.path),
        discovery_source: Some(candidate.source),
        version: Some(version),
        protocol: Some(COPILOT_PROTOCOL.to_owned()),
        capabilities: capabilities(handshake),
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
    };
    if !probe.capabilities.mcp {
        probe.verification = ProviderVerification::Degraded;
        probe.degradation_reason =
            Some("ACP initialize handshake did not advertise required HTTP MCP support".to_owned());
        probe
            .evidence
            .push("ACP did not advertise HTTP MCP support".to_owned());
    }
    probe
}

fn degraded_probe(
    candidate: ExecutableCandidate,
    version: Option<String>,
    reason: String,
) -> ProviderProbe {
    ProviderProbe {
        contract_version: ADAPTER_CONTRACT_V1,
        provider: ProviderKind::GitHubCopilot,
        verification: ProviderVerification::Degraded,
        executable: Some(candidate.path),
        discovery_source: Some(candidate.source),
        version,
        protocol: Some(COPILOT_PROTOCOL.to_owned()),
        capabilities: ProviderCapabilities::default(),
        authentication: AuthenticationEvidence::unknown(),
        degradation_reason: Some(reason),
        evidence: Vec::new(),
        checked_at: chrono::Utc::now().to_rfc3339(),
    }
}
