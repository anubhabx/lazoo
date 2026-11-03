use super::{ExtensionError, ExtensionTarget};
use crate::{DiscoveryEnvironment, ProviderKind};
use lazoo_runtime::LaunchEnvironment;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct HarnessHome {
    pub target: ExtensionTarget,
    pub home: PathBuf,
    pub config: PathBuf,
    pub skill_roots: Vec<PathBuf>,
    pub environment: LaunchEnvironment,
}

impl HarnessHome {
    pub fn personal(
        provider: ProviderKind,
        discovery: &DiscoveryEnvironment,
    ) -> Result<Self, ExtensionError> {
        let home = discovery
            .config_home()
            .ok_or_else(|| ExtensionError::Invalid("USERPROFILE/HOME is unavailable".into()))?
            .to_path_buf();
        let config = match provider {
            ProviderKind::Codex => discovery
                .codex_home()
                .map_or_else(|| home.join(".codex"), Path::to_path_buf),
            ProviderKind::ClaudeCode => discovery
                .extension_home("CLAUDE_CONFIG_DIR")
                .map_or_else(|| home.join(".claude"), Path::to_path_buf),
            ProviderKind::GitHubCopilot => discovery
                .extension_home("COPILOT_HOME")
                .map_or_else(|| home.join(".copilot"), Path::to_path_buf),
            ProviderKind::OpenCode => discovery.extension_home("OPENCODE_CONFIG_DIR").map_or_else(
                || {
                    discovery
                        .extension_home("XDG_CONFIG_HOME")
                        .map_or_else(|| home.join(".config"), Path::to_path_buf)
                        .join("opencode")
                },
                Path::to_path_buf,
            ),
            ProviderKind::Antigravity => home.join(".gemini/config"),
            ProviderKind::GrokBuild => home.join(".grok"),
        };
        let mut result = Self {
            target: ExtensionTarget {
                provider,
                account_id: None,
            },
            home,
            config,
            skill_roots: Vec::new(),
            environment: LaunchEnvironment::default(),
        };
        result.configure();
        Ok(result)
    }

    #[must_use]
    pub fn profile(
        mut self,
        account_id: uuid::Uuid,
        root: &Path,
        environment: &LaunchEnvironment,
    ) -> Self {
        self.target.account_id = Some(account_id);
        match self.target.provider {
            ProviderKind::Codex | ProviderKind::ClaudeCode | ProviderKind::GitHubCopilot => {
                self.config = root.to_path_buf();
            }
            ProviderKind::OpenCode => {
                self.home = root.to_path_buf();
                self.config = root.join("config/opencode");
            }
            _ => {
                self.home = root.to_path_buf();
                self.config = root.join(".gemini/config");
            }
        }
        self.configure();
        for key in environment.removals() {
            self.environment.remove(key);
        }
        for (key, value) in environment.values() {
            self.environment.set(key, value);
        }
        self
    }

    fn configure(&mut self) {
        let provider = self.target.provider;
        self.skill_roots = if provider == ProviderKind::GrokBuild {
            Vec::new()
        } else {
            vec![self.config.join("skills")]
        };
        match provider {
            ProviderKind::Codex | ProviderKind::GitHubCopilot => {
                self.skill_roots.push(self.home.join(".agents/skills"));
            }
            ProviderKind::OpenCode => {
                self.skill_roots.push(self.home.join(".claude/skills"));
                self.skill_roots.push(self.home.join(".agents/skills"));
            }
            ProviderKind::Antigravity => {
                self.skill_roots
                    .push(self.home.join(".gemini/antigravity-cli/skills"));
                self.skill_roots.push(self.home.join(".gemini/skills"));
            }
            _ => {}
        }
        self.environment.set("USERPROFILE", &self.home);
        self.environment.set("HOME", &self.home);
        match provider {
            ProviderKind::Codex => self.environment.set("CODEX_HOME", &self.config),
            ProviderKind::ClaudeCode => self.environment.set("CLAUDE_CONFIG_DIR", &self.config),
            ProviderKind::GitHubCopilot => self.environment.set("COPILOT_HOME", &self.config),
            ProviderKind::OpenCode => {
                self.environment.set(
                    "XDG_CONFIG_HOME",
                    self.config.parent().unwrap_or(&self.home),
                );
                self.environment.set("OPENCODE_CONFIG_DIR", &self.config);
            }
            _ => {}
        }
        self.environment.set("CI", "1");
        self.environment.set("NO_COLOR", "1");
        self.environment.set("GIT_TERMINAL_PROMPT", "0");
        for key in [
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_SESSION_ID",
        ] {
            self.environment.remove(key);
        }
    }

    pub fn skill_target(&self, name: &str) -> Result<PathBuf, ExtensionError> {
        self.skill_roots
            .first()
            .map(|p| p.join(name))
            .ok_or(ExtensionError::Unsupported(
                "skills are not supported by this harness",
            ))
    }
}
