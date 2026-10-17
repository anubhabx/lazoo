use std::{
    ffi::OsString,
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
/// The session permission mode the daemon derives from full autonomy.
const FULL_AUTONOMY_PERMISSION_MODE: &str = "auto";
/// `OpenCode`'s inline permissions config variable ("Inlined json permissions config").
const PERMISSION_ENV: &str = "OPENCODE_PERMISSION";
/// A top-level `"*"` rule covers built-in and MCP tools alike.
const ALLOW_ALL_PERMISSION: &str = r#"{"*":"allow"}"#;
/// Every tool asks, so each governed action raises `session/request_permission` for the
/// daemon's approval observer; only the read-only, non-governed tools (`read`, `glob`, `grep`,
/// `list`) are allowed by name. The catch-all comes first because the last matching rule wins,
/// and an unknown or newly added tool (writes, `bash`, `webfetch`, `websearch`, `task`, MCP
/// tools, `external_directory`, `todowrite`) stays under `"*": "ask"` and never auto-runs.
/// `read` uses the object form with `OpenCode`'s documented default env-file patterns, so a
/// shorthand `"read": "allow"` does not widen it to `.env` files (`.env.example` stays readable).
const GOVERNED_PERMISSION: &str = r#"{"*":"ask","read":{"*":"allow","*.env":"deny","*.env.*":"deny","*.env.example":"allow"},"glob":"allow","grep":"allow","list":"allow"}"#;
/// Environment variables that can replace or add to the `OpenCode` configuration and, per the live
/// precedence probe, could carry permission rules the inline block does not outrank.
const CONFIG_ENVIRONMENT: [&str; 3] = [
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_CONFIG_DIR",
];
/// Configuration keys that can grant or change permissions (`tools` and `mode` are the legacy
/// spellings). Matched as quoted keys, so a false positive only refuses.
const PERMISSION_CONFIG_KEYS: [&str; 5] = ["permission", "agent", "agents", "mode", "tools"];
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
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

    /// The launch of a session: allow-all for the full-autonomy mode and the governed block for
    /// every other mode, including none and `untrusted` (the smoke), so no session ever runs on
    /// the owner's own permission configuration. Only the version probe, which sends no prompt,
    /// launches without a block.
    #[must_use]
    pub fn command(executable: &Path, request: &SessionRequest) -> CommandSpec {
        let block = if request.permission_mode.as_deref() == Some(FULL_AUTONOMY_PERMISSION_MODE) {
            ALLOW_ALL_PERMISSION
        } else {
            GOVERNED_PERMISSION
        };
        Self::launch(executable, request, Some(block))
    }

    // The inline block is the only permission source Lazoo sets; no config file is written.
    fn launch(executable: &Path, request: &SessionRequest, block: Option<&str>) -> CommandSpec {
        let mut command = CommandSpec::new(executable, &request.cwd).arg("acp");
        if let Some(block) = block {
            command
                .env
                .insert(OsString::from(PERMISSION_ENV), OsString::from(block));
        }
        command
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
            match JsonRpcProcess::spawn_standard(&Self::launch(&candidate.path, &request, None)) {
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
        if request.permission_mode.as_deref() != Some(FULL_AUTONOMY_PERMISSION_MODE)
            && governed_config_conflict(
                &request.cwd,
                &|name| std::env::var_os(name).is_some(),
                home_directory().as_deref(),
            )
        {
            // The live precedence probe (2026-10-17) showed a project `opencode.json` that allows
            // `bash` beating the inline block, so a governed session fails closed rather than
            // run where the block may not hold.
            return Err(ProviderError::Unsupported {
                provider: ProviderKind::OpenCode,
                capability: "governed permissions beside an OpenCode permission, agent, plugin, or environment configuration",
            });
        }
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

    fn turn_observed<'a>(
        &'a self,
        request: &'a TurnRequest,
        observer: &'a dyn crate::TurnEventObserver,
    ) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        self.0.turn_observed(request, observer)
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

fn home_directory() -> Option<PathBuf> {
    ["USERPROFILE", "HOME"]
        .iter()
        .find_map(|name| std::env::var_os(name).filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

/// Whether the session's working directory (or an ancestor up to the repository root), or the
/// environment, carries `OpenCode` configuration that can override the inline permission block:
/// an `opencode.json` or `opencode.jsonc` naming a permission-bearing key (or unreadable), any
/// `.opencode` directory (agents, plugins, tools), or an `OPENCODE_CONFIG*` variable. The scan
/// stops before the user's home directory: its `.opencode` is `OpenCode`'s own installation and
/// global configuration, which is the owner's, not the project's (see the record's retained
/// boundaries).
fn governed_config_conflict(
    cwd: &Path,
    environment_has: &dyn Fn(&str) -> bool,
    home: Option<&Path>,
) -> bool {
    if CONFIG_ENVIRONMENT.iter().any(|name| environment_has(name)) {
        return true;
    }
    for directory in cwd.ancestors() {
        if home.is_some_and(|home| {
            home.to_string_lossy()
                .eq_ignore_ascii_case(&directory.to_string_lossy())
        }) {
            break;
        }
        if directory.join(".opencode").exists() {
            return true;
        }
        for name in ["opencode.json", "opencode.jsonc"] {
            let file = directory.join(name);
            if !file.exists() {
                continue;
            }
            let small = std::fs::metadata(&file).is_ok_and(|meta| meta.len() <= MAX_CONFIG_BYTES);
            let Some(text) = small.then(|| std::fs::read_to_string(&file).ok()).flatten() else {
                return true;
            };
            if names_a_key(&text) {
                return true;
            }
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    false
}

fn names_a_key(text: &str) -> bool {
    PERMISSION_CONFIG_KEYS.iter().any(|key| {
        let quoted = format!("\"{key}\"");
        text.match_indices(&quoted)
            .any(|(index, _)| text[index + quoted.len()..].trim_start().starts_with(':'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> bool {
        false
    }

    #[test]
    fn governed_sessions_refuse_configuration_that_can_override_the_block() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        let work = root.join("a").join("b");
        std::fs::create_dir_all(&work).unwrap();
        assert!(!governed_config_conflict(&work, &none, None));

        // Unrelated configuration does not refuse; permission-bearing keys do, even in an
        // ancestor and even with comments, and an unparseable file is not trusted either way
        // (it is only refused when it names a key).
        std::fs::write(root.join("opencode.json"), r#"{"model":"x","theme":"y"}"#).unwrap();
        assert!(!governed_config_conflict(&work, &none, None));
        for text in [
            r#"{"permission":{"bash":"allow"}}"#,
            r#"{"agent" : {"build":{"permission":{"bash":"allow"}}}}"#,
            "// c\n{\"tools\": {\"bash\": true}}",
            r#"{"mode":{"x":{}}}"#,
        ] {
            std::fs::write(root.join("opencode.json"), text).unwrap();
            assert!(governed_config_conflict(&work, &none, None), "{text}");
        }
        std::fs::remove_file(root.join("opencode.json")).unwrap();

        std::fs::write(work.join("opencode.jsonc"), r#"{"agent":{}}"#).unwrap();
        assert!(governed_config_conflict(&work, &none, None));
        std::fs::remove_file(work.join("opencode.jsonc")).unwrap();

        std::fs::create_dir(work.join(".opencode")).unwrap();
        assert!(governed_config_conflict(&work, &none, None));
        std::fs::remove_dir(work.join(".opencode")).unwrap();
        assert!(!governed_config_conflict(&work, &none, None));

        for name in CONFIG_ENVIRONMENT {
            assert!(governed_config_conflict(
                &work,
                &|variable| variable == name,
                None
            ));
        }
    }

    #[test]
    fn the_scan_stops_before_the_home_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path();
        std::fs::create_dir(home.join(".opencode")).unwrap();
        let work = home.join("data").join("session");
        std::fs::create_dir_all(&work).unwrap();
        assert!(governed_config_conflict(&work, &none, None));
        assert!(!governed_config_conflict(&work, &none, Some(home)));
        std::fs::create_dir(work.join(".opencode")).unwrap();
        assert!(governed_config_conflict(&work, &none, Some(home)));
    }

    #[test]
    fn the_scan_stops_at_the_repository_root() {
        let temporary = tempfile::tempdir().unwrap();
        let outer = temporary.path();
        std::fs::write(outer.join("opencode.json"), r#"{"permission":{}}"#).unwrap();
        let repository = outer.join("repo");
        std::fs::create_dir_all(repository.join(".git")).unwrap();
        assert!(!governed_config_conflict(&repository, &none, None));
    }
}
