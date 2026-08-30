use crate::{
    AdapterFuture, AntigravityAdapter, AntigravitySession, ClaudeAdapter, ClaudeSession,
    CodexAdapter, CodexSession, DiscoveryEnvironment, ExecutableCandidate, GrokAdapter,
    GrokSession, OpenCodeAdapter, OpenCodeSession, ProviderAdapter, ProviderError, ProviderEvent,
    ProviderKind, ProviderProbe, ProviderSession, SessionRequest, TurnRequest,
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
        match provider {
            ProviderKind::Codex => self.codex.probe(environment).await,
            ProviderKind::OpenCode => self.opencode.probe(environment).await,
            ProviderKind::GrokBuild => self.grok.probe(environment).await,
            ProviderKind::Antigravity => self.antigravity.probe(environment).await,
            ProviderKind::ClaudeCode => self.claude.probe(environment).await,
        }
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
            ProviderKind::OpenCode => self
                .opencode
                .start(request)
                .await
                .map(ProviderSessionHandle::OpenCode),
            ProviderKind::GrokBuild => self
                .grok
                .start(request)
                .await
                .map(ProviderSessionHandle::GrokBuild),
            ProviderKind::Antigravity => self
                .antigravity
                .start(request)
                .await
                .map(ProviderSessionHandle::Antigravity),
            ProviderKind::ClaudeCode => self
                .claude
                .start(request)
                .await
                .map(ProviderSessionHandle::ClaudeCode),
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
    GrokBuild(GrokSession),
    Antigravity(AntigravitySession),
    ClaudeCode(ClaudeSession),
}

impl ProviderSession for ProviderSessionHandle {
    fn id(&self) -> String {
        match self {
            Self::Codex(session) => session.id(),
            Self::OpenCode(session) => session.id(),
            Self::GrokBuild(session) => session.id(),
            Self::Antigravity(session) => session.id(),
            Self::ClaudeCode(session) => session.id(),
        }
    }

    fn turn<'a>(&'a self, request: &'a TurnRequest) -> AdapterFuture<'a, Vec<ProviderEvent>> {
        match self {
            Self::Codex(session) => session.turn(request),
            Self::OpenCode(session) => session.turn(request),
            Self::GrokBuild(session) => session.turn(request),
            Self::Antigravity(session) => session.turn(request),
            Self::ClaudeCode(session) => session.turn(request),
        }
    }

    fn cancel(&self) -> AdapterFuture<'_, ()> {
        match self {
            Self::Codex(session) => session.cancel(),
            Self::OpenCode(session) => session.cancel(),
            Self::GrokBuild(session) => session.cancel(),
            Self::Antigravity(session) => session.cancel(),
            Self::ClaudeCode(session) => session.cancel(),
        }
    }

    fn stop(&self) -> AdapterFuture<'_, ()> {
        match self {
            Self::Codex(session) => session.stop(),
            Self::OpenCode(session) => session.stop(),
            Self::GrokBuild(session) => session.stop(),
            Self::Antigravity(session) => session.stop(),
            Self::ClaudeCode(session) => session.stop(),
        }
    }
}
