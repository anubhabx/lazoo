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
/// The global configuration keys that carry agent-scoped permissions, which the live probe showed
/// beating the inline block. A global top-level `permission` is deliberately absent: the block
/// wins over it.
const AGENT_SCOPED_CONFIG_KEYS: [&str; 3] = ["agent", "agents", "mode"];
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
            && let Some(detail) = governed_config_conflict(
                &request.cwd,
                &|name| std::env::var_os(name).is_some(),
                home_directory().as_deref(),
                config_home().as_deref(),
            )
        {
            // The live precedence probes (2026-10-17) showed a project or global agent-scoped
            // permission beating the inline block, so a governed session fails closed rather
            // than run where the block may not hold.
            return Err(ProviderError::Unsupported {
                provider: ProviderKind::OpenCode,
                capability: "governed permissions beside an OpenCode permission, agent, plugin, or environment configuration",
                detail: Some(detail),
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

/// The owner's XDG configuration root as `OpenCode` reads it (`XDG_CONFIG_HOME || ~/.config`): the
/// variable when it is set and non-empty, else none.
fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Why a governed session must not start, or `None` when nothing the scan knows can outrank the
/// inline permission block. The reason names the file, directory, or variable that triggered the
/// refusal and never its contents.
///
/// Project rule: the session's working directory (or an ancestor up to the repository root), or
/// the environment, carries `OpenCode` configuration that can override the block: an
/// `opencode.json` or `opencode.jsonc` naming a permission-bearing key (or unreadable), any
/// `.opencode` directory (agents, plugins, tools), or an `OPENCODE_CONFIG*` variable. The project
/// scan stops before the user's home directory: its `.opencode` is the owner's own installation
/// and global configuration, which the global rule below covers (see the record's retained
/// boundaries).
///
/// Global rule: the owner's configuration directories (`$XDG_CONFIG_HOME/opencode` when that is
/// set, else `~/.config/opencode`, plus `~/.opencode`) refuse a governed start when
/// `opencode.json`/`opencode.jsonc` names an agent-scoped carrier (`agent`, `agents`, `mode`),
/// when an `agent`/`agents` subdirectory holds any `*.md` agent definition, or when a file or
/// directory there cannot be read. The live probe (2026-10-17) showed agent-scoped permissions
/// beating the inline block wherever they live, while a global top-level `permission` key does
/// NOT: the block wins there, so a global `permission` (or legacy `tools`) alone never refuses.
fn governed_config_conflict(
    cwd: &Path,
    environment_has: &dyn Fn(&str) -> bool,
    home: Option<&Path>,
    config_home: Option<&Path>,
) -> Option<String> {
    for name in CONFIG_ENVIRONMENT {
        if environment_has(name) {
            return Some(format!("the environment variable {name} is set"));
        }
    }
    project_config_conflict(cwd, home).or_else(|| global_config_conflict(home, config_home))
}

fn project_config_conflict(cwd: &Path, home: Option<&Path>) -> Option<String> {
    for directory in cwd.ancestors() {
        if home.is_some_and(|home| {
            home.to_string_lossy()
                .eq_ignore_ascii_case(&directory.to_string_lossy())
        }) {
            break;
        }
        let dot_directory = directory.join(".opencode");
        if dot_directory.exists() {
            return Some(format!(
                "the project directory {} exists",
                dot_directory.display()
            ));
        }
        for name in ["opencode.json", "opencode.jsonc"] {
            let file = directory.join(name);
            if !file.exists() {
                continue;
            }
            match read_configuration(&file) {
                None => return Some(format!("{} is unreadable or too large", file.display())),
                Some(text) if names_a_key(&text, &PERMISSION_CONFIG_KEYS) => {
                    return Some(format!(
                        "the project configuration {} names a permission-bearing key",
                        file.display()
                    ));
                }
                Some(_) => {}
            }
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    None
}

fn global_config_conflict(home: Option<&Path>, config_home: Option<&Path>) -> Option<String> {
    let mut roots = Vec::new();
    match (config_home, home) {
        (Some(config_home), _) => roots.push(config_home.join("opencode")),
        (None, Some(home)) => roots.push(home.join(".config").join("opencode")),
        (None, None) => {}
    }
    if let Some(home) = home {
        roots.push(home.join(".opencode"));
    }
    roots.iter().find_map(|root| global_root_conflict(root))
}

fn global_root_conflict(root: &Path) -> Option<String> {
    for name in ["opencode.json", "opencode.jsonc"] {
        let file = root.join(name);
        match std::fs::metadata(&file) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Some(format!("{} is unreadable", file.display())),
            Ok(_) => {}
        }
        match read_configuration(&file) {
            None => return Some(format!("{} is unreadable or too large", file.display())),
            Some(text) if names_a_key(&text, &AGENT_SCOPED_CONFIG_KEYS) => {
                return Some(format!(
                    "the global configuration {} names an agent-scoped key",
                    file.display()
                ));
            }
            Some(_) => {}
        }
    }
    for name in ["agent", "agents"] {
        let directory = root.join(name);
        match agent_definition(&directory, 0) {
            Ok(None) => {}
            Ok(Some(file)) => {
                return Some(format!(
                    "the global agent definition {} exists",
                    file.display()
                ));
            }
            Err(()) => return Some(format!("{} is unreadable", directory.display())),
        }
    }
    None
}

/// The first `*.md` file under `directory` (an absent directory holds none), or `Err` when a
/// directory cannot be read or nests deeper than the bound.
fn agent_definition(directory: &Path, depth: usize) -> Result<Option<PathBuf>, ()> {
    const MAX_AGENT_DEPTH: usize = 8;
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    for entry in entries {
        let path = entry.map_err(|_| ())?.path();
        if path.is_dir() {
            if depth >= MAX_AGENT_DEPTH {
                return Err(());
            }
            if let Some(found) = agent_definition(&path, depth + 1)? {
                return Ok(Some(found));
            }
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// The configuration text, or `None` when the file is unreadable, not UTF-8, or too large.
fn read_configuration(file: &Path) -> Option<String> {
    let small = std::fs::metadata(file).is_ok_and(|meta| meta.len() <= MAX_CONFIG_BYTES);
    small.then(|| std::fs::read_to_string(file).ok()).flatten()
}

fn names_a_key(text: &str, keys: &[&str]) -> bool {
    keys.iter().any(|key| {
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

    fn conflict(cwd: &Path) -> Option<String> {
        governed_config_conflict(cwd, &none, None, None)
    }

    #[test]
    fn governed_sessions_refuse_configuration_that_can_override_the_block() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        let work = root.join("a").join("b");
        std::fs::create_dir_all(&work).unwrap();
        assert_eq!(conflict(&work), None);

        // Unrelated configuration does not refuse; permission-bearing keys do, even in an
        // ancestor and even with comments, and an unparseable file is not trusted either way
        // (it is only refused when it names a key).
        std::fs::write(root.join("opencode.json"), r#"{"model":"x","theme":"y"}"#).unwrap();
        assert_eq!(conflict(&work), None);
        for text in [
            r#"{"permission":{"bash":"allow"}}"#,
            r#"{"agent" : {"build":{"permission":{"bash":"allow"}}}}"#,
            "// c\n{\"tools\": {\"bash\": true}}",
            r#"{"mode":{"x":{}}}"#,
        ] {
            std::fs::write(root.join("opencode.json"), text).unwrap();
            let detail = conflict(&work).unwrap_or_default();
            assert!(detail.contains("opencode.json"), "{text}: {detail}");
            assert!(!detail.contains("bash"), "contents must not leak: {detail}");
        }
        std::fs::remove_file(root.join("opencode.json")).unwrap();

        std::fs::write(work.join("opencode.jsonc"), r#"{"agent":{}}"#).unwrap();
        assert!(conflict(&work).is_some_and(|detail| detail.contains("opencode.jsonc")));
        std::fs::remove_file(work.join("opencode.jsonc")).unwrap();

        std::fs::create_dir(work.join(".opencode")).unwrap();
        assert!(conflict(&work).is_some_and(|detail| detail.contains(".opencode")));
        std::fs::remove_dir(work.join(".opencode")).unwrap();
        assert_eq!(conflict(&work), None);

        for name in CONFIG_ENVIRONMENT {
            let detail = governed_config_conflict(&work, &|variable| variable == name, None, None);
            assert!(detail.is_some_and(|detail| detail.contains(name)));
        }
    }

    #[test]
    fn the_scan_stops_before_the_home_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path();
        std::fs::create_dir(home.join(".opencode")).unwrap();
        let work = home.join("data").join("session");
        std::fs::create_dir_all(&work).unwrap();
        assert!(conflict(&work).is_some());
        // Below the home directory the project rule stops, and the global rule finds nothing
        // agent-scoped in a bare `~/.opencode` (OpenCode's own installation).
        assert_eq!(
            governed_config_conflict(&work, &none, Some(home), None),
            None
        );
        std::fs::create_dir(work.join(".opencode")).unwrap();
        assert!(governed_config_conflict(&work, &none, Some(home), None).is_some());
    }

    #[test]
    fn the_scan_stops_at_the_repository_root() {
        let temporary = tempfile::tempdir().unwrap();
        let outer = temporary.path();
        std::fs::write(outer.join("opencode.json"), r#"{"permission":{}}"#).unwrap();
        let repository = outer.join("repo");
        std::fs::create_dir_all(repository.join(".git")).unwrap();
        assert_eq!(conflict(&repository), None);
    }

    #[test]
    fn the_global_scan_refuses_agent_scoped_carriers_and_names_the_file() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let xdg = temporary.path().join("xdg");
        let work = temporary.path().join("work");
        for directory in [&home, &xdg, &work] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::create_dir(work.join(".git")).unwrap();
        let scan = |config_home: Option<&Path>| {
            governed_config_conflict(&work, &none, Some(&home), config_home)
        };
        assert_eq!(scan(None), None);
        assert_eq!(scan(Some(&xdg)), None);

        // With XDG_CONFIG_HOME set, only `$XDG_CONFIG_HOME/opencode` is the config root; without
        // it the root is `~/.config/opencode`.
        let xdg_root = xdg.join("opencode");
        let default_root = home.join(".config").join("opencode");
        let dot_root = home.join(".opencode");
        for root in [&xdg_root, &default_root, &dot_root] {
            std::fs::create_dir_all(root).unwrap();
        }

        // Agent-scoped carriers refuse; the detail names the file and never its contents.
        for (root, config_home) in [
            (&xdg_root, Some(xdg.as_path())),
            (&default_root, None),
            (&dot_root, None),
        ] {
            for (name, text) in [
                (
                    "opencode.json",
                    r#"{"agent":{"build":{"permission":{"bash":"allow"}}}}"#,
                ),
                ("opencode.jsonc", "// c\n{\"agents\": {}}"),
                ("opencode.json", r#"{"mode":{"build":{}}}"#),
            ] {
                let file = root.join(name);
                std::fs::write(&file, text).unwrap();
                let detail = scan(config_home).unwrap_or_default();
                assert!(detail.contains(&file.display().to_string()), "{detail}");
                assert!(
                    !detail.contains("allow"),
                    "contents must not leak: {detail}"
                );
                std::fs::remove_file(&file).unwrap();
            }
            assert_eq!(scan(config_home), None);
        }

        // The default root is ignored while XDG_CONFIG_HOME names another.
        std::fs::write(default_root.join("opencode.json"), r#"{"agent":{}}"#).unwrap();
        assert_eq!(scan(Some(&xdg)), None);
        assert!(scan(None).is_some());
        std::fs::remove_file(default_root.join("opencode.json")).unwrap();

        // An `agent` or `agents` directory refuses only for a markdown definition.
        for directory in ["agent", "agents"] {
            let agents = xdg_root.join(directory);
            std::fs::create_dir_all(agents.join("nested")).unwrap();
            std::fs::write(agents.join("notes.txt"), "x").unwrap();
            assert_eq!(scan(Some(&xdg)), None, "{directory} without markdown");
            let definition = agents.join("nested").join("build.MD");
            std::fs::write(&definition, "---\npermission:\n  bash: allow\n---\n").unwrap();
            let detail = scan(Some(&xdg)).unwrap_or_default();
            assert!(
                detail.contains(&definition.display().to_string()),
                "{detail}"
            );
            assert!(!detail.contains("bash"), "contents must not leak: {detail}");
            std::fs::remove_dir_all(&agents).unwrap();
        }
        assert_eq!(scan(Some(&xdg)), None);
    }

    #[test]
    fn a_global_top_level_permission_alone_does_not_refuse() {
        // The live probe (2026-10-17): a global top-level `permission.bash: allow` lost to the
        // inline block (the `execute` request still arrived), so only agent-scoped carriers
        // refuse. Legacy `tools` is likewise not an agent-scoped carrier.
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let xdg = temporary.path().join("xdg");
        let work = temporary.path().join("work");
        std::fs::create_dir_all(xdg.join("opencode")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(work.join(".git")).unwrap();
        std::fs::write(
            xdg.join("opencode").join("opencode.json"),
            r#"{"permission":{"bash":"allow"},"tools":{"bash":true},"model":"x"}"#,
        )
        .unwrap();
        assert_eq!(
            governed_config_conflict(&work, &none, Some(&home), Some(&xdg)),
            None
        );
    }

    #[test]
    fn the_global_scan_fails_closed_on_an_unreadable_file_or_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let xdg = temporary.path().join("xdg");
        let work = temporary.path().join("work");
        let root = xdg.join("opencode");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(work.join(".git")).unwrap();
        let scan = || governed_config_conflict(&work, &none, Some(&home), Some(&xdg));

        // A configuration that is not UTF-8 cannot be read as text.
        std::fs::write(root.join("opencode.json"), [0xff, 0xfe, 0x00, 0x9f]).unwrap();
        assert!(scan().is_some_and(|detail| detail.contains("opencode.json")));
        std::fs::remove_file(root.join("opencode.json")).unwrap();

        // An oversize configuration is not trusted either.
        let oversize = vec![b' '; usize::try_from(MAX_CONFIG_BYTES).unwrap() + 1];
        std::fs::write(root.join("opencode.jsonc"), oversize).unwrap();
        assert!(scan().is_some_and(|detail| detail.contains("opencode.jsonc")));
        std::fs::remove_file(root.join("opencode.jsonc")).unwrap();

        // A configuration path that is a directory cannot be read as a file.
        std::fs::create_dir(root.join("opencode.json")).unwrap();
        assert!(scan().is_some_and(|detail| detail.contains("opencode.json")));
        std::fs::remove_dir(root.join("opencode.json")).unwrap();

        // An `agent` path that is a file, not a directory, cannot be listed.
        std::fs::write(root.join("agent"), "x").unwrap();
        assert!(scan().is_some_and(|detail| detail.contains("agent")));
        std::fs::remove_file(root.join("agent")).unwrap();
        assert_eq!(scan(), None);
    }
}
