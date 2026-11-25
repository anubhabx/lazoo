use std::{
    collections::{BTreeMap, HashMap},
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{ExecutableCandidate, ProviderKind};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoverySource {
    InheritedPath,
    PersistedUserPath,
    PersistedMachinePath,
    KnownLocation,
}

impl DiscoverySource {
    const fn priority(self) -> u8 {
        match self {
            Self::InheritedPath => 0,
            Self::PersistedUserPath => 1,
            Self::PersistedMachinePath => 2,
            Self::KnownLocation => 3,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DiscoveryEnvironment {
    inherited_path: Vec<PathBuf>,
    persisted_user_path: Vec<PathBuf>,
    persisted_machine_path: Vec<PathBuf>,
    known_locations: BTreeMap<ProviderKind, Vec<PathBuf>>,
    /// The user profile that provider configuration files hang off (`USERPROFILE`).
    config_home: Option<PathBuf>,
    /// `CODEX_HOME` when set; otherwise Codex lives at `<config_home>/.codex`.
    codex_home: Option<PathBuf>,
    extension_homes: BTreeMap<String, PathBuf>,
}

impl DiscoveryEnvironment {
    #[must_use]
    pub fn current() -> Self {
        let inherited_path = env::var_os("PATH")
            .as_deref()
            .map(env::split_paths)
            .into_iter()
            .flatten()
            .collect();
        let (persisted_user_path, persisted_machine_path) = persisted_paths();
        let mut environment = Self {
            inherited_path,
            persisted_user_path,
            persisted_machine_path,
            known_locations: BTreeMap::new(),
            config_home: env::var_os("USERPROFILE")
                .or_else(|| env::var_os("HOME"))
                .map(PathBuf::from),
            codex_home: env::var_os("CODEX_HOME").map(PathBuf::from),
            extension_homes: [
                "CLAUDE_CONFIG_DIR",
                "COPILOT_HOME",
                "XDG_CONFIG_HOME",
                "OPENCODE_CONFIG_DIR",
            ]
            .into_iter()
            .filter_map(|key| env::var_os(key).map(|value| (key.to_owned(), PathBuf::from(value))))
            .collect(),
        };
        environment.add_default_known_locations();
        environment
    }

    #[must_use]
    pub const fn empty() -> Self {
        Self {
            inherited_path: Vec::new(),
            persisted_user_path: Vec::new(),
            persisted_machine_path: Vec::new(),
            known_locations: BTreeMap::new(),
            config_home: None,
            codex_home: None,
            extension_homes: BTreeMap::new(),
        }
    }

    /// The profile directory provider configuration files are read from.
    #[must_use]
    pub fn with_config_home(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_home = Some(path.into());
        self
    }

    /// An explicit `CODEX_HOME`.
    #[must_use]
    pub fn with_codex_home(mut self, path: impl Into<PathBuf>) -> Self {
        self.codex_home = Some(path.into());
        self
    }

    #[must_use]
    pub fn config_home(&self) -> Option<&Path> {
        self.config_home.as_deref()
    }

    #[must_use]
    pub fn codex_home(&self) -> Option<&Path> {
        self.codex_home.as_deref()
    }

    #[must_use]
    pub fn extension_home(&self, key: &str) -> Option<&Path> {
        self.extension_homes.get(key).map(PathBuf::as_path)
    }

    #[must_use]
    pub fn with_extension_home(mut self, key: &str, path: impl Into<PathBuf>) -> Self {
        self.extension_homes.insert(key.to_owned(), path.into());
        self
    }

    #[must_use]
    pub fn with_persisted_user_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.persisted_user_path.push(path.into());
        self
    }

    #[must_use]
    pub fn with_known_location(mut self, provider: ProviderKind, path: impl Into<PathBuf>) -> Self {
        self.known_locations
            .entry(provider)
            .or_default()
            .push(path.into());
        self
    }

    #[must_use]
    pub fn candidates(
        &self,
        provider: ProviderKind,
        executable_names: &[&str],
    ) -> Vec<ExecutableCandidate> {
        let mut candidates = Vec::new();
        append_path_candidates(
            &mut candidates,
            &self.inherited_path,
            executable_names,
            DiscoverySource::InheritedPath,
        );
        append_path_candidates(
            &mut candidates,
            &self.persisted_user_path,
            executable_names,
            DiscoverySource::PersistedUserPath,
        );
        append_path_candidates(
            &mut candidates,
            &self.persisted_machine_path,
            executable_names,
            DiscoverySource::PersistedMachinePath,
        );
        if let Some(paths) = self.known_locations.get(&provider) {
            candidates.extend(
                paths
                    .iter()
                    .filter(|path| path.is_file())
                    .cloned()
                    .map(|path| ExecutableCandidate::new(path, DiscoverySource::KnownLocation)),
            );
        }
        rank_candidates(candidates)
    }

    fn add_default_known_locations(&mut self) {
        if let Some(user_profile) = env::var_os("USERPROFILE").map(PathBuf::from) {
            self.known_locations
                .entry(ProviderKind::ClaudeCode)
                .or_default()
                .push(user_profile.join(".local").join("bin").join("claude.exe"));
            self.known_locations
                .entry(ProviderKind::OpenCode)
                .or_default()
                .push(user_profile.join(".bun").join("bin").join("opencode.exe"));
            self.known_locations
                .entry(ProviderKind::GrokBuild)
                .or_default()
                .push(user_profile.join(".grok").join("bin").join("grok.exe"));
        }
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            self.known_locations
                .entry(ProviderKind::Antigravity)
                .or_default()
                .push(local_app_data.join("agy").join("bin").join("agy.exe"));
            let codex_bins = local_app_data.join("OpenAI").join("Codex").join("bin");
            if let Ok(entries) = fs::read_dir(codex_bins) {
                self.known_locations
                    .entry(ProviderKind::Codex)
                    .or_default()
                    .extend(
                        entries
                            .filter_map(Result::ok)
                            .map(|entry| entry.path().join("codex.exe")),
                    );
            }
        }
        // The Windows npm package contains a platform-native executable. Prefer it over the
        // JavaScript shim when present so ACP runs as a directly contained child process.
        if let Some(node_root) = env::var_os("NVM_SYMLINK").map(PathBuf::from) {
            self.known_locations
                .entry(ProviderKind::GitHubCopilot)
                .or_default()
                .push(
                    node_root
                        .join("node_modules")
                        .join("@github")
                        .join("copilot")
                        .join("node_modules")
                        .join("@github")
                        .join("copilot-win32-x64")
                        .join("copilot.exe"),
                );
        }
    }
}

fn append_path_candidates(
    output: &mut Vec<ExecutableCandidate>,
    directories: &[PathBuf],
    executable_names: &[&str],
    source: DiscoverySource,
) {
    for directory in directories {
        for name in executable_names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                output.push(ExecutableCandidate::new(candidate, source));
            }
        }
    }
}

#[must_use]
pub fn rank_candidates(mut candidates: Vec<ExecutableCandidate>) -> Vec<ExecutableCandidate> {
    candidates.sort_by_key(|candidate| candidate.source.priority());
    let mut seen = HashMap::<OsString, usize>::new();
    let mut unique: Vec<ExecutableCandidate> = Vec::new();
    for candidate in candidates {
        let key = windows_path_key(&candidate.path);
        if let Some(index) = seen.get(&key).copied() {
            if candidate.source.priority() < unique[index].source.priority() {
                unique[index] = candidate;
            }
        } else {
            seen.insert(key, unique.len());
            unique.push(candidate);
        }
    }
    unique
}

fn windows_path_key(path: &Path) -> OsString {
    OsString::from(
        path.as_os_str()
            .to_string_lossy()
            .replace('/', "\\")
            .to_lowercase(),
    )
}

#[cfg(windows)]
fn persisted_paths() -> (Vec<PathBuf>, Vec<PathBuf>) {
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    };

    let user = read_registry_path(&RegKey::predef(HKEY_CURRENT_USER), "Environment");
    let machine = read_registry_path(
        &RegKey::predef(HKEY_LOCAL_MACHINE),
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
    );
    (user, machine)
}

#[cfg(windows)]
fn read_registry_path(root: &winreg::RegKey, key: &str) -> Vec<PathBuf> {
    root.open_subkey(key)
        .and_then(|environment| environment.get_value::<String, _>("Path"))
        .ok()
        .as_deref()
        .map(OsStr::new)
        .map(env::split_paths)
        .into_iter()
        .flatten()
        .collect()
}

#[cfg(not(windows))]
const fn persisted_paths() -> (Vec<PathBuf>, Vec<PathBuf>) {
    (Vec::new(), Vec::new())
}
