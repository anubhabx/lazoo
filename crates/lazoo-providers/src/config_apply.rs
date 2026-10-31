//! Transactional, secret-safe application of one selected native MCP entry.
//!
//! This deliberately owns no durable desired/actual state.  Its preview retains the original
//! and candidate bytes privately; callers only receive serializable hashes and changed names.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use chrono::{SecondsFormat, Utc};
use lazoo_runtime::{OwnerSecurity, atomic_write};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{DiscoveryEnvironment, ProviderKind};

pub const PROVIDER_CONFIGURATION_APPLY_V1: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigurationCompatibility {
    Native,
    Translated,
    Unsupported,
    RestartRequired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderVersionValidation {
    pub provider: ProviderKind,
    pub version: String,
    pub compatibility: ConfigurationCompatibility,
}

/// A user-authored selection.  It intentionally has no credential values: existing selected
/// secret fields remain in the provider file and are never copied through this API.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SelectedMcpIntegration {
    pub provider: ProviderKind,
    pub name: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub enabled: bool,
    pub replace: bool,
    pub selected_secret_fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConfigurationPreviewMetadata {
    pub version: u32,
    pub provider: ProviderKind,
    pub provider_version: String,
    pub compatibility: ConfigurationCompatibility,
    pub target: PathBuf,
    pub target_existed: bool,
    pub original_hash: String,
    pub candidate_hash: String,
    pub changed_entries: Vec<String>,
    pub changed_keys: Vec<String>,
    pub revision: String,
}

/// Private bytes prevent accidental preview serialization or credential disclosure.
pub struct ConfigurationApplyPreview {
    metadata: ConfigurationPreviewMetadata,
    original: Option<Vec<u8>>,
    candidate: Vec<u8>,
    backup_root: PathBuf,
}

impl ConfigurationApplyPreview {
    #[must_use]
    pub fn metadata(&self) -> &ConfigurationPreviewMetadata {
        &self.metadata
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConfigurationApplyReceipt {
    pub version: u32,
    pub provider: ProviderKind,
    pub target: PathBuf,
    pub backup_name: String,
    pub backup_hash: String,
    pub original_hash: String,
    pub applied_hash: String,
    pub revision: String,
    pub status: ConfigurationApplyStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigurationApplyStatus {
    Applied,
    RolledBack,
    RollbackValidationFailed,
    ConcurrentDrift,
}

/// Read-only crash-recovery result. Recovery never writes a provider file by itself.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigurationRecoveryState {
    /// The target still has the pre-apply bytes; no native mutation was observed.
    OriginalPresent,
    /// The target has the prepared candidate bytes. An operator must choose the next action.
    AppliedPresent,
    /// The target, receipt, or backup does not match the prepared operation.
    DriftRefused,
}

#[derive(Debug, Error)]
pub enum ConfigurationApplyError {
    #[error("provider configuration format is not supported")]
    UnsupportedProvider,
    #[error("provider version was not validated")]
    VersionNotValidated,
    #[error("provider version is not compatible with this configuration operation")]
    UnsupportedVersion,
    #[error("configuration target is unsafe")]
    UnsafeTarget,
    #[error("configuration is malformed")]
    MalformedConfiguration,
    #[error("selection is invalid")]
    InvalidSelection,
    #[error("an entry with that name already exists")]
    NameConflict,
    #[error("existing secret fields require an exact explicit selection")]
    SecretFieldSelectionRequired,
    #[error("the preview is stale because the configuration changed")]
    StalePreview,
    #[error("configuration changed concurrently; recovery did not overwrite it")]
    ConcurrentDrift,
    #[error("backup receipt is invalid")]
    InvalidBackupReceipt,
    #[error("backup is missing or was modified")]
    BackupTampered,
    #[error("configuration I/O failed")]
    Io(#[from] std::io::Error),
    #[error("runtime write failed")]
    Runtime(#[from] lazoo_runtime::RuntimeError),
}

/// Render a private candidate for one known provider file. `validate` is deliberately required:
/// this library cannot infer compatibility from an executable's presence.
pub fn preview_configuration_apply(
    environment: &DiscoveryEnvironment,
    backup_root: impl AsRef<Path>,
    selection: SelectedMcpIntegration,
    validate: impl FnOnce(ProviderKind) -> Option<ProviderVersionValidation>,
) -> Result<ConfigurationApplyPreview, ConfigurationApplyError> {
    validate_selection(&selection)?;
    let validation =
        validate(selection.provider).ok_or(ConfigurationApplyError::VersionNotValidated)?;
    if validation.provider != selection.provider {
        return Err(ConfigurationApplyError::VersionNotValidated);
    }
    if !matches!(
        validation.compatibility,
        ConfigurationCompatibility::Native | ConfigurationCompatibility::RestartRequired
    ) {
        return Err(ConfigurationApplyError::UnsupportedVersion);
    }
    let target = known_target(environment, selection.provider)?;
    let target = safe_target(&target)?;
    let original = if target.exists() {
        Some(fs::read(&target)?)
    } else {
        None
    };
    let candidate = render(selection.provider, original.as_deref(), &selection)?;
    let original_hash = hash_optional(original.as_deref());
    let candidate_hash = hash(&candidate);
    let revision = hash(format!("{original_hash}:{candidate_hash}:{}", selection.name).as_bytes());
    Ok(ConfigurationApplyPreview {
        metadata: ConfigurationPreviewMetadata {
            version: PROVIDER_CONFIGURATION_APPLY_V1,
            provider: selection.provider,
            provider_version: validation.version,
            compatibility: validation.compatibility,
            target,
            target_existed: original.is_some(),
            original_hash,
            candidate_hash,
            changed_entries: vec![selection.name],
            changed_keys: vec![entry_root(selection.provider).to_owned()],
            revision,
        },
        original,
        candidate,
        backup_root: backup_root.as_ref().to_path_buf(),
    })
}

/// Write a preview after the caller durably records the server-owned receipt. The replacement is
/// same-directory and crash-safe against a torn target file. A non-cooperating external writer can
/// still race the hash check and Windows replacement; callers must retain the prepared receipt and
/// use [`inspect_configuration_apply_recovery`] after a crash rather than claim universal CAS.
pub fn apply_configuration_preview(
    preview: ConfigurationApplyPreview,
    reprobe: impl FnOnce(&Path) -> Result<(), ConfigurationApplyError>,
) -> Result<ConfigurationApplyReceipt, ConfigurationApplyError> {
    apply_configuration_preview_with_intent(preview, |_| Ok(()), reprobe)
}

/// As [`apply_configuration_preview`], but invokes `persist_intent` after the owner-protected
/// backup exists and before native replacement. If it fails, the provider config is untouched.
pub fn apply_configuration_preview_with_intent(
    preview: ConfigurationApplyPreview,
    persist_intent: impl FnOnce(&ConfigurationApplyReceipt) -> Result<(), ConfigurationApplyError>,
    reprobe: impl FnOnce(&Path) -> Result<(), ConfigurationApplyError>,
) -> Result<ConfigurationApplyReceipt, ConfigurationApplyError> {
    let target = safe_target(&preview.metadata.target)?;
    let backup_root = safe_backup_root(&preview.backup_root)?;
    let current = read_target(&target)?;
    if hash_optional(current.as_deref()) != preview.metadata.original_hash {
        return Err(ConfigurationApplyError::StalePreview);
    }
    let (backup_name, backup_hash) =
        write_backup(&backup_root, &preview.metadata, preview.original.as_deref())?;
    let applied_hash = hash(&preview.candidate);
    let receipt = ConfigurationApplyReceipt {
        version: PROVIDER_CONFIGURATION_APPLY_V1,
        provider: preview.metadata.provider,
        target,
        backup_name,
        backup_hash,
        original_hash: preview.metadata.original_hash,
        applied_hash,
        revision: preview.metadata.revision,
        status: ConfigurationApplyStatus::Applied,
    };
    persist_intent(&receipt)?;
    atomic_write(&receipt.target, &preview.candidate)?;
    if reprobe(&receipt.target).is_ok() {
        return Ok(receipt);
    }
    if hash_optional(read_target(&receipt.target)?.as_deref()) != receipt.applied_hash {
        return Err(ConfigurationApplyError::ConcurrentDrift);
    }
    restore_target(
        &receipt.target,
        preview.original.as_deref(),
        &receipt.applied_hash,
    )?;
    Err(ConfigurationApplyError::MalformedConfiguration)
}

/// Roll back one receipt after validating both the current file and the owner-protected backup.
pub fn rollback_configuration_apply(
    environment: &DiscoveryEnvironment,
    backup_root: impl AsRef<Path>,
    receipt: &ConfigurationApplyReceipt,
    reprobe: impl FnOnce(&Path) -> Result<(), ConfigurationApplyError>,
) -> Result<ConfigurationApplyReceipt, ConfigurationApplyError> {
    if receipt.version != PROVIDER_CONFIGURATION_APPLY_V1 || !single_filename(&receipt.backup_name)
    {
        return Err(ConfigurationApplyError::InvalidBackupReceipt);
    }
    if receipt.status != ConfigurationApplyStatus::Applied {
        return Err(ConfigurationApplyError::InvalidBackupReceipt);
    }
    let expected = safe_target(&known_target(environment, receipt.provider)?)?;
    let target = safe_target(&receipt.target)?;
    let backup_root = safe_backup_root(backup_root)?;
    if target != expected || hash_optional(read_target(&target)?.as_deref()) != receipt.applied_hash
    {
        return Err(ConfigurationApplyError::StalePreview);
    }
    let backup = backup_root.join(&receipt.backup_name);
    if backup.parent() != Some(backup_root.as_path()) || is_reparse_or_symlink(&backup)? {
        return Err(ConfigurationApplyError::InvalidBackupReceipt);
    }
    let old = fs::read(&backup).map_err(|_| ConfigurationApplyError::BackupTampered)?;
    if hash(&old) != receipt.backup_hash {
        return Err(ConfigurationApplyError::BackupTampered);
    }
    let backup = decode_backup(&old)?;
    if backup.provider != receipt.provider
        || backup.target != target
        || backup.revision != receipt.revision
        || backup.original_hash != receipt.original_hash
        || backup.applied_hash != receipt.applied_hash
        || hash_optional(backup.original.as_deref()) != receipt.original_hash
    {
        return Err(ConfigurationApplyError::InvalidBackupReceipt);
    }
    restore_target(&target, backup.original.as_deref(), &receipt.applied_hash)?;
    let mut rolled = receipt.clone();
    rolled.status = match reprobe(&target) {
        Ok(()) => ConfigurationApplyStatus::RolledBack,
        Err(_) => ConfigurationApplyStatus::RollbackValidationFailed,
    };
    Ok(rolled)
}

/// Inspect a prepared receipt after process restart. This validates the canonical target, backup
/// envelope and all receipt bindings, then reports the observed file hash without mutating it.
pub fn inspect_configuration_apply_recovery(
    environment: &DiscoveryEnvironment,
    backup_root: impl AsRef<Path>,
    receipt: &ConfigurationApplyReceipt,
) -> Result<ConfigurationRecoveryState, ConfigurationApplyError> {
    if receipt.version != PROVIDER_CONFIGURATION_APPLY_V1
        || receipt.status != ConfigurationApplyStatus::Applied
        || !single_filename(&receipt.backup_name)
    {
        return Ok(ConfigurationRecoveryState::DriftRefused);
    }
    let expected = safe_target(&known_target(environment, receipt.provider)?)?;
    let target = safe_target(&receipt.target)?;
    if target != expected {
        return Ok(ConfigurationRecoveryState::DriftRefused);
    }
    let root = safe_backup_root(backup_root)?;
    let path = root.join(&receipt.backup_name);
    if path.parent() != Some(root.as_path()) || is_reparse_or_symlink(&path)? {
        return Ok(ConfigurationRecoveryState::DriftRefused);
    }
    let Ok(bytes) = fs::read(path) else {
        return Ok(ConfigurationRecoveryState::DriftRefused);
    };
    let Ok(backup) = decode_backup(&bytes) else {
        return Ok(ConfigurationRecoveryState::DriftRefused);
    };
    if hash(&bytes) != receipt.backup_hash
        || backup.provider != receipt.provider
        || backup.target != target
        || backup.revision != receipt.revision
        || backup.original_hash != receipt.original_hash
        || backup.applied_hash != receipt.applied_hash
        || hash_optional(backup.original.as_deref()) != receipt.original_hash
    {
        return Ok(ConfigurationRecoveryState::DriftRefused);
    }
    Ok(
        if hash_optional(read_target(&target)?.as_deref()) == receipt.original_hash {
            ConfigurationRecoveryState::OriginalPresent
        } else if hash_optional(read_target(&target)?.as_deref()) == receipt.applied_hash {
            ConfigurationRecoveryState::AppliedPresent
        } else {
            ConfigurationRecoveryState::DriftRefused
        },
    )
}

fn known_target(
    environment: &DiscoveryEnvironment,
    provider: ProviderKind,
) -> Result<PathBuf, ConfigurationApplyError> {
    match provider {
        ProviderKind::Codex => Ok(environment
            .codex_home()
            .map_or_else(
                || {
                    environment
                        .config_home()
                        .map(|p| p.join(".codex").join("config.toml"))
                },
                |p| Some(p.join("config.toml")),
            )
            .ok_or(ConfigurationApplyError::UnsupportedProvider)?),
        ProviderKind::ClaudeCode => Ok(environment
            .config_home()
            .map(|p| p.join(".claude.json"))
            .ok_or(ConfigurationApplyError::UnsupportedProvider)?),
        ProviderKind::OpenCode => Ok(environment
            .config_home()
            .map(|p| p.join(".config").join("opencode").join("opencode.json"))
            .ok_or(ConfigurationApplyError::UnsupportedProvider)?),
        ProviderKind::GitHubCopilot | ProviderKind::GrokBuild | ProviderKind::Antigravity => {
            Err(ConfigurationApplyError::UnsupportedProvider)
        }
    }
}

fn safe_target(target: &Path) -> Result<PathBuf, ConfigurationApplyError> {
    let parent = target
        .parent()
        .ok_or(ConfigurationApplyError::UnsafeTarget)?;
    ensure_safe_ancestors(parent)?;
    if target.exists() && is_reparse_or_symlink(target)? {
        return Err(ConfigurationApplyError::UnsafeTarget);
    }
    // A known provider config may not exist yet.  Canonicalize the nearest existing safe
    // ancestor and rebuild only the missing lexical components derived above, never a caller
    // supplied destination.
    let mut existing = parent;
    let mut missing = Vec::new();
    while !existing.exists() {
        missing.push(
            existing
                .file_name()
                .ok_or(ConfigurationApplyError::UnsafeTarget)?,
        );
        existing = existing
            .parent()
            .ok_or(ConfigurationApplyError::UnsafeTarget)?;
    }
    let mut canonical_parent = existing
        .canonicalize()
        .map_err(|_| ConfigurationApplyError::UnsafeTarget)?;
    for component in missing.iter().rev() {
        canonical_parent.push(component);
    }
    Ok(canonical_parent.join(
        target
            .file_name()
            .ok_or(ConfigurationApplyError::UnsafeTarget)?,
    ))
}

fn ensure_safe_ancestors(path: &Path) -> Result<(), ConfigurationApplyError> {
    let mut cursor = PathBuf::new();
    for component in path.components() {
        cursor.push(component.as_os_str());
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        if cursor.exists() && is_reparse_or_symlink(&cursor)? {
            return Err(ConfigurationApplyError::UnsafeTarget);
        }
    }
    Ok(())
}

fn is_reparse_or_symlink(path: &Path) -> Result<bool, ConfigurationApplyError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(true);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        Ok(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }
    #[cfg(not(windows))]
    Ok(false)
}

fn validate_selection(selection: &SelectedMcpIntegration) -> Result<(), ConfigurationApplyError> {
    if selection.name.is_empty()
        || selection.name.len() > 128
        || selection.name.contains(['\n', '\r', '/', '\\'])
        || selection.args.len() > 64
        || selection
            .args
            .iter()
            .any(|a| a.len() > 1024 || a.contains(['\n', '\r']))
    {
        return Err(ConfigurationApplyError::InvalidSelection);
    }
    match (&selection.command, &selection.url) {
        (Some(command), None)
            if !command.is_empty() && command.len() <= 4096 && !command.contains(['\n', '\r']) =>
        {
            Ok(())
        }
        (None, Some(url))
            if url.len() <= 4096
                && !url.contains(['\n', '\r'])
                && selection.args.is_empty()
                && !url.contains('?')
                && (url.starts_with("https://") || url.starts_with("http://")) =>
        {
            Ok(())
        }
        _ => Err(ConfigurationApplyError::InvalidSelection),
    }
}

fn render(
    provider: ProviderKind,
    original: Option<&[u8]>,
    selection: &SelectedMcpIntegration,
) -> Result<Vec<u8>, ConfigurationApplyError> {
    match provider {
        ProviderKind::Codex => render_codex(original, selection),
        ProviderKind::ClaudeCode => render_claude(original, selection),
        ProviderKind::OpenCode => render_opencode(original, selection),
        ProviderKind::GitHubCopilot | ProviderKind::GrokBuild | ProviderKind::Antigravity => {
            Err(ConfigurationApplyError::UnsupportedProvider)
        }
    }
}

fn render_codex(
    original: Option<&[u8]>,
    selection: &SelectedMcpIntegration,
) -> Result<Vec<u8>, ConfigurationApplyError> {
    let text = original.map_or(Ok(String::new()), |b| {
        String::from_utf8(b.to_vec()).map_err(|_| ConfigurationApplyError::MalformedConfiguration)
    })?;
    let mut root: toml::Table = if text.is_empty() {
        toml::Table::new()
    } else {
        text.parse()
            .map_err(|_| ConfigurationApplyError::MalformedConfiguration)?
    };
    let servers = root
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or(ConfigurationApplyError::MalformedConfiguration)?;
    let old = servers.get(&selection.name).cloned();
    let mut table = existing_toml(old, selection)?;
    write_toml_entry(&mut table, selection);
    servers.insert(selection.name.clone(), toml::Value::Table(table));
    toml::to_string_pretty(&root)
        .map(String::into_bytes)
        .map_err(|_| ConfigurationApplyError::MalformedConfiguration)
}

fn existing_toml(
    old: Option<toml::Value>,
    selection: &SelectedMcpIntegration,
) -> Result<toml::Table, ConfigurationApplyError> {
    match old {
        None => Ok(toml::Table::new()),
        Some(value) => {
            if !selection.replace {
                return Err(ConfigurationApplyError::NameConflict);
            }
            let table = value
                .as_table()
                .cloned()
                .ok_or(ConfigurationApplyError::MalformedConfiguration)?;
            let found = toml_secret_fields(&table);
            exact_secret_fields(&found, &selection.selected_secret_fields)?;
            Ok(table)
        }
    }
}

fn write_toml_entry(table: &mut toml::Table, selection: &SelectedMcpIntegration) {
    table.remove("command");
    table.remove("args");
    table.remove("url");
    if let Some(command) = &selection.command {
        table.insert("command".into(), toml::Value::String(command.clone()));
        table.insert(
            "args".into(),
            toml::Value::Array(
                selection
                    .args
                    .iter()
                    .cloned()
                    .map(toml::Value::String)
                    .collect(),
            ),
        );
    }
    if let Some(url) = &selection.url {
        table.insert("url".into(), toml::Value::String(url.clone()));
    }
    table.insert("enabled".into(), toml::Value::Boolean(selection.enabled));
}

fn render_claude(
    original: Option<&[u8]>,
    selection: &SelectedMcpIntegration,
) -> Result<Vec<u8>, ConfigurationApplyError> {
    let mut root = json_root(original)?;
    let servers = root
        .as_object_mut()
        .ok_or(ConfigurationApplyError::MalformedConfiguration)?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or(ConfigurationApplyError::MalformedConfiguration)?;
    let old = servers.get(&selection.name).cloned();
    let mut entry = existing_json(old, selection, &["env", "headers"])?;
    write_claude_entry(&mut entry, selection);
    servers.insert(selection.name.clone(), serde_json::Value::Object(entry));
    serde_json::to_vec_pretty(&root).map_err(|_| ConfigurationApplyError::MalformedConfiguration)
}

fn write_claude_entry(
    entry: &mut serde_json::Map<String, serde_json::Value>,
    selection: &SelectedMcpIntegration,
) {
    for key in ["command", "args", "url"] {
        entry.remove(key);
    }
    if let Some(command) = &selection.command {
        entry.insert("type".into(), serde_json::Value::String("stdio".into()));
        entry.insert("command".into(), serde_json::Value::String(command.clone()));
        entry.insert("args".into(), serde_json::json!(selection.args));
    } else if let Some(url) = &selection.url {
        entry.insert("type".into(), serde_json::Value::String("http".into()));
        entry.insert("url".into(), serde_json::Value::String(url.clone()));
    }
    entry.insert(
        "disabled".into(),
        serde_json::Value::Bool(!selection.enabled),
    );
}

fn render_opencode(
    original: Option<&[u8]>,
    selection: &SelectedMcpIntegration,
) -> Result<Vec<u8>, ConfigurationApplyError> {
    let mut root = json_root(original)?;
    let servers = root
        .as_object_mut()
        .ok_or(ConfigurationApplyError::MalformedConfiguration)?
        .entry("mcp")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or(ConfigurationApplyError::MalformedConfiguration)?;
    let old = servers.get(&selection.name).cloned();
    let mut entry = existing_json(old, selection, &["environment", "headers"])?;
    for key in ["command", "url"] {
        entry.remove(key);
    }
    if let Some(command) = &selection.command {
        entry.insert("type".into(), serde_json::Value::String("local".into()));
        let mut line = vec![command.clone()];
        line.extend(selection.args.clone());
        entry.insert("command".into(), serde_json::json!(line));
    } else if let Some(url) = &selection.url {
        entry.insert("type".into(), serde_json::Value::String("remote".into()));
        entry.insert("url".into(), serde_json::Value::String(url.clone()));
    }
    entry.insert("enabled".into(), serde_json::Value::Bool(selection.enabled));
    servers.insert(selection.name.clone(), serde_json::Value::Object(entry));
    serde_json::to_vec_pretty(&root).map_err(|_| ConfigurationApplyError::MalformedConfiguration)
}

fn json_root(original: Option<&[u8]>) -> Result<serde_json::Value, ConfigurationApplyError> {
    match original {
        None => Ok(serde_json::json!({})),
        Some(bytes) => serde_json::from_slice(bytes)
            .map_err(|_| ConfigurationApplyError::MalformedConfiguration),
    }
}
fn existing_json(
    old: Option<serde_json::Value>,
    selection: &SelectedMcpIntegration,
    secret_roots: &[&str],
) -> Result<serde_json::Map<String, serde_json::Value>, ConfigurationApplyError> {
    match old {
        None => Ok(serde_json::Map::new()),
        Some(value) => {
            if !selection.replace {
                return Err(ConfigurationApplyError::NameConflict);
            }
            let entry = value
                .as_object()
                .cloned()
                .ok_or(ConfigurationApplyError::MalformedConfiguration)?;
            let keys = secret_roots
                .iter()
                .flat_map(|root| {
                    entry
                        .get(*root)
                        .and_then(serde_json::Value::as_object)
                        .into_iter()
                        .flat_map(|o| o.keys().cloned())
                })
                .collect();
            exact_secret_fields(&keys, &selection.selected_secret_fields)?;
            Ok(entry)
        }
    }
}
fn toml_secret_fields(table: &toml::Table) -> BTreeSet<String> {
    ["env", "http_headers", "env_http_headers"]
        .iter()
        .flat_map(|key| {
            table
                .get(*key)
                .and_then(toml::Value::as_table)
                .into_iter()
                .flat_map(|v| v.keys().cloned())
        })
        .collect()
}
fn exact_secret_fields(
    found: &BTreeSet<String>,
    selected: &[String],
) -> Result<(), ConfigurationApplyError> {
    let selected: BTreeSet<_> = selected.iter().cloned().collect();
    if found == &selected {
        Ok(())
    } else {
        Err(ConfigurationApplyError::SecretFieldSelectionRequired)
    }
}
fn entry_root(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Codex => "mcp_servers",
        ProviderKind::ClaudeCode => "mcpServers",
        ProviderKind::OpenCode => "mcp",
        ProviderKind::GitHubCopilot | ProviderKind::GrokBuild | ProviderKind::Antigravity => "",
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BackupEnvelope {
    provider: ProviderKind,
    target: PathBuf,
    revision: String,
    original_hash: String,
    applied_hash: String,
    original: Option<Vec<u8>>,
}

fn safe_backup_root(root: impl AsRef<Path>) -> Result<PathBuf, ConfigurationApplyError> {
    let root = root.as_ref();
    ensure_safe_ancestors(root)?;
    let mut existing = root;
    let mut missing = Vec::new();
    while !existing.exists() {
        missing.push(
            existing
                .file_name()
                .ok_or(ConfigurationApplyError::UnsafeTarget)?,
        );
        existing = existing
            .parent()
            .ok_or(ConfigurationApplyError::UnsafeTarget)?;
    }
    if is_reparse_or_symlink(existing)? {
        return Err(ConfigurationApplyError::UnsafeTarget);
    }
    let mut canonical = existing
        .canonicalize()
        .map_err(|_| ConfigurationApplyError::UnsafeTarget)?;
    for component in missing.iter().rev() {
        canonical.push(component);
    }
    fs::create_dir_all(&canonical)?;
    ensure_safe_ancestors(&canonical)?;
    if is_reparse_or_symlink(&canonical)? {
        return Err(ConfigurationApplyError::UnsafeTarget);
    }
    Ok(canonical)
}

fn write_backup(
    root: &Path,
    metadata: &ConfigurationPreviewMetadata,
    original: Option<&[u8]>,
) -> Result<(String, String), ConfigurationApplyError> {
    fs::create_dir_all(root)?;
    let owner = OwnerSecurity::current()?;
    owner.apply_to_path(root)?;
    let stamp = Utc::now()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
        .replace(':', "-");
    let name = format!(
        "{}-{}-{}.backup",
        metadata.provider,
        stamp,
        &metadata.revision[..12]
    );
    let path = root.join(&name);
    if !single_filename(&name) || path.exists() {
        return Err(ConfigurationApplyError::InvalidBackupReceipt);
    }
    let bytes = encode_backup(metadata, original)?;
    atomic_write(&path, &bytes)?;
    owner.apply_to_path(&path)?;
    Ok((name, hash(&bytes)))
}
fn encode_backup(
    metadata: &ConfigurationPreviewMetadata,
    original: Option<&[u8]>,
) -> Result<Vec<u8>, ConfigurationApplyError> {
    serde_json::to_vec(&BackupEnvelope {
        provider: metadata.provider,
        target: metadata.target.clone(),
        revision: metadata.revision.clone(),
        original_hash: metadata.original_hash.clone(),
        applied_hash: metadata.candidate_hash.clone(),
        original: original.map(ToOwned::to_owned),
    })
    .map_err(|_| ConfigurationApplyError::InvalidBackupReceipt)
}
fn decode_backup(bytes: &[u8]) -> Result<BackupEnvelope, ConfigurationApplyError> {
    serde_json::from_slice(bytes).map_err(|_| ConfigurationApplyError::BackupTampered)
}
fn read_target(path: &Path) -> Result<Option<Vec<u8>>, ConfigurationApplyError> {
    if !path.exists() {
        return Ok(None);
    }
    if is_reparse_or_symlink(path)? {
        return Err(ConfigurationApplyError::UnsafeTarget);
    }
    Ok(Some(fs::read(path)?))
}

fn restore_target(
    path: &Path,
    original: Option<&[u8]>,
    expected_applied_hash: &str,
) -> Result<(), ConfigurationApplyError> {
    if hash_optional(read_target(path)?.as_deref()) != expected_applied_hash {
        return Err(ConfigurationApplyError::ConcurrentDrift);
    }
    if let Some(bytes) = original {
        atomic_write(path, bytes)?;
    } else {
        // A missing original is restored only while the exact applied file remains present.
        fs::remove_file(path)?;
    }
    Ok(())
}
fn single_filename(value: &str) -> bool {
    Path::new(value).components().count() == 1 && Path::new(value).file_name().is_some()
}
fn hash_optional(bytes: Option<&[u8]>) -> String {
    bytes.map_or_else(|| hash(b"absent"), hash)
}
fn hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .fold(String::with_capacity(64), |mut value, byte| {
            use std::fmt::Write as _;
            let _ = write!(value, "{byte:02x}");
            value
        })
}
