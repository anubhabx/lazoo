use crate::{
    AdapterFuture, AntigravityAdapter, AntigravitySession, ClaudeAdapter, ClaudeSession,
    CodexAdapter, CodexSession, CopilotAdapter, CopilotSession, DiscoveryEnvironment,
    ExecutableCandidate, GrokAdapter, GrokSession, NativeInteractiveSession, OpenCodeAdapter,
    OpenCodeHttpSession, OpenCodeSession, ProviderAdapter, ProviderError, ProviderEvent,
    ProviderKind, ProviderProbe, ProviderSession, SessionRequest, TurnEventObserver, TurnRequest,
};

#[derive(Clone, Debug)]
pub struct ProviderDiscovery {
    pub provider: ProviderKind,
    pub candidate: Option<ExecutableCandidate>,
}

#[derive(Clone, Debug, Default)]
pub struct ProviderRegistry {
    codex: CodexAdapter,
    opencode: OpenCodeAdapter,
    copilot: CopilotAdapter,
    grok: GrokAdapter,
    antigravity: AntigravityAdapter,
    claude: ClaudeAdapter,
}

impl ProviderRegistry {
    #[must_use]
    pub fn discover(&self, environment: &DiscoveryEnvironment) -> Vec<ProviderDiscovery> {
        ProviderKind::ALL
            .into_iter()
            .map(|provider| ProviderDiscovery {
                provider,
                candidate: self
                    .discover_provider(provider, environment)
                    .into_iter()
                    .next(),
            })
            .collect()
    }

    pub async fn probe(
        &self,
        provider: ProviderKind,
        environment: &DiscoveryEnvironment,
    ) -> Result<ProviderProbe, ProviderError> {
        let mut probe = match provider {
            ProviderKind::Codex => self.codex.probe(environment).await,
            ProviderKind::OpenCode => self.opencode.probe(environment).await,
            ProviderKind::GitHubCopilot => self.copilot.probe(environment).await,
            ProviderKind::GrokBuild => self.grok.probe(environment).await,
            ProviderKind::Antigravity => self.antigravity.probe(environment).await,
            ProviderKind::ClaudeCode => self.claude.probe(environment).await,
        }?;
        // Registry starts use the governed native transports. Legacy standalone machine
        // adapters keep their own narrower capability contracts for compatibility.
        if probe.capabilities.session
            && matches!(
                provider,
                ProviderKind::OpenCode | ProviderKind::ClaudeCode | ProviderKind::Antigravity
            )
        {
            probe.capabilities.permission = true;
            probe.capabilities.mcp = true;
        }
        Ok(probe)
    }

    pub async fn start(
        &self,
        provider: ProviderKind,
        request: &SessionRequest,
    ) -> Result<ProviderSessionHandle, ProviderError> {
        match provider {
            ProviderKind::Codex => self
                .codex
                .start(request)
                .await
                .map(ProviderSessionHandle::Codex),
            ProviderKind::OpenCode => {
                let candidate = self
                    .opencode
                    .discover(&DiscoveryEnvironment::current())
                    .into_iter()
                    .next()
                    .ok_or(ProviderError::NotDetected(provider))?;
                OpenCodeHttpSession::start(&candidate.path, request)
                    .await
                    .map(ProviderSessionHandle::OpenCodeHttp)
            }
            ProviderKind::GitHubCopilot => self
                .copilot
                .start(request)
                .await
                .map(ProviderSessionHandle::GitHubCopilot),
            ProviderKind::GrokBuild => self
                .grok
                .start(request)
                .await
                .map(ProviderSessionHandle::GrokBuild),
            ProviderKind::Antigravity | ProviderKind::ClaudeCode => {
                let candidate = self
                    .discover_provider(provider, &DiscoveryEnvironment::current())
                    .into_iter()
                    .next()
                    .ok_or(ProviderError::NotDetected(provider))?;
                NativeInteractiveSession::start(provider, &candidate.path, request)
                    .await
                    .map(ProviderSessionHandle::Interactive)
            }
        }
    }

    fn discover_provider(
        &self,
        provider: ProviderKind,
        environment: &DiscoveryEnvironment,
    ) -> Vec<ExecutableCandidate> {
        match provider {
            ProviderKind::Codex => self.codex.discover(environment),
            ProviderKind::OpenCode => self.opencode.discover(environment),
            ProviderKind::GitHubCopilot => self.copilot.discover(environment),
            ProviderKind::GrokBuild => self.grok.discover(environment),
            ProviderKind::Antigravity => self.antigravity.discover(environment),
            ProviderKind::ClaudeCode => self.claude.discover(environment),
        }
    }
}

#[derive(Debug)]
pub enum ProviderSessionHandle {
    Codex(CodexSession),
    OpenCode(OpenCodeSession),
    OpenCodeHttp(OpenCodeHttpSession),
    GitHubCopilot(CopilotSession),
    GrokBuild(GrokSession),
    Antigravity(AntigravitySession),
    ClaudeCode(ClaudeSession),
    Interactive(NativeInteractiveSession),
}

impl ProviderSession for ProviderSessionHandle {
    fn take_usage_windows(&self) -> Vec<lazoo_protocol::UsageWindow> {
        match self {
            Self::Interactive(session) => session.take_usage_windows(),
            _ => Vec::new(),
        }
    }
    fn native_view_available(&self) -> bool {
        match self {
            Self::Codex(session) => session.native_view_available(),
            Self::OpenCode(session) => session.native_view_available(),
            Self::OpenCodeHttp(session) => session.native_view_available(),
            Self::GitHubCopilot(session) => session.native_view_available(),
            Self::Interactive(session) => session.native_view_available(),
            _ => false,
        }
    }

    fn open_native_view(&self) -> AdapterFuture<'_, Option<crate::NativeViewLease>> {
        match self {
            Self::Codex(session) => session.open_native_view(),
            Self::OpenCode(session) => session.open_native_view(),
            Self::OpenCodeHttp(session) => session.open_native_view(),
            Self::GitHubCopilot(session) => session.open_native_view(),
            Self::Interactive(session) => session.open_native_view(),
            _ => Box::pin(async { Ok(None) }),
        }
    }

    fn id(&self) -> String {
        match self {
            Self::Codex(session) => session.id(),
            Self::OpenCode(session) => session.id(),
            Self::OpenCodeHttp(session) => session.id(),
            Self::GitHubCopilot(session) => session.id(),
            Self::GrokBuild(session) => session.id(),
            Self::Antigravity(session) => session.id(),
            Self::ClaudeCode(session) => session.id(),
            Self::Interactive(session) => session.id(),
        }
    }

    fn native_host_pid(&self) -> Option<u32> {
        match self {
            Self::Codex(session) => session.native_host_pid(),
            Self::OpenCode(session) => session.native_host_pid(),
            Self::OpenCodeHttp(session) => session.native_host_pid(),
            Self::GitHubCopilot(session) => session.native_host_pid(),
            Self::GrokBuild(session) => session.native_host_pid(),
            Self::Antigravity(session) => session.native_host_pid(),
            Self::ClaudeCode(session) => session.native_host_pid(),
            Self::Interactive(session) => session.native_host_pid(),
        }
    }

    fn resources(&self) -> Option<lazoo_runtime::JobAccounting> {
        match self {
            Self::Codex(session) => session.resources(),
            Self::OpenCode(session) => session.resources(),
            Self::OpenCodeHttp(session) => session.resources(),
            Self::GitHubCopilot(session) => session.resources(),
            Self::GrokBuild(session) => session.resources(),
            Self::Antigravity(session) => session.resources(),
            Self::ClaudeCode(session) => session.resources(),
            Self::Interactive(session) => session.resources(),
        }
    }

    fn turn<'a>(&'a self, request: &'a TurnRequest) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        match self {
            Self::Codex(session) => session.turn(request),
            Self::OpenCode(session) => session.turn(request),
            Self::OpenCodeHttp(session) => session.turn(request),
            Self::GitHubCopilot(session) => session.turn(request),
            Self::GrokBuild(session) => session.turn(request),
            Self::Antigravity(session) => session.turn(request),
            Self::ClaudeCode(session) => session.turn(request),
            Self::Interactive(session) => session.turn(request),
        }
    }

    fn turn_observed<'a>(
        &'a self,
        request: &'a TurnRequest,
        observer: &'a dyn TurnEventObserver,
    ) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        match self {
            Self::Codex(session) => session.turn_observed(request, observer),
            Self::OpenCode(session) => session.turn_observed(request, observer),
            Self::OpenCodeHttp(session) => session.turn_observed(request, observer),
            Self::GitHubCopilot(session) => session.turn_observed(request, observer),
            Self::GrokBuild(session) => session.turn_observed(request, observer),
            Self::Antigravity(session) => session.turn_observed(request, observer),
            Self::ClaudeCode(session) => session.turn_observed(request, observer),
            Self::Interactive(session) => session.turn_observed(request, observer),
        }
    }

    fn cancel(&self) -> AdapterFuture<'_, ()> {
        match self {
            Self::Codex(session) => session.cancel(),
            Self::OpenCode(session) => session.cancel(),
            Self::OpenCodeHttp(session) => session.cancel(),
            Self::GitHubCopilot(session) => session.cancel(),
            Self::GrokBuild(session) => session.cancel(),
            Self::Antigravity(session) => session.cancel(),
            Self::ClaudeCode(session) => session.cancel(),
            Self::Interactive(session) => session.cancel(),
        }
    }

    fn stop(&self) -> AdapterFuture<'_, ()> {
        match self {
            Self::Codex(session) => session.stop(),
            Self::OpenCode(session) => session.stop(),
            Self::OpenCodeHttp(session) => session.stop(),
            Self::GitHubCopilot(session) => session.stop(),
            Self::GrokBuild(session) => session.stop(),
            Self::Antigravity(session) => session.stop(),
            Self::ClaudeCode(session) => session.stop(),
            Self::Interactive(session) => session.stop(),
        }
    }
}
