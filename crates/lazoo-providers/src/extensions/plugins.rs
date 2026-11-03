//! Native plugin operations. Never edit Codex TOML or `OpenCode` JSONC to simulate unsupported commands.
use super::{ExtensionError, HarnessHome, read_bounded, run_command};
use crate::{ProviderKind, redact_json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PluginAction {
    Install,
    Uninstall,
    Enable,
    Disable,
    MarketplaceAdd,
    MarketplaceRemove,
}

impl PluginAction {
    #[must_use]
    pub const fn method(self) -> &'static str {
        match self {
            Self::Install => "plugin.install",
            Self::Uninstall => "plugin.uninstall",
            Self::Enable => "plugin.enable",
            Self::Disable => "plugin.disable",
            Self::MarketplaceAdd => "plugin.marketplace.add",
            Self::MarketplaceRemove => "plugin.marketplace.remove",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarketplaceSelection {
    pub name: String,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRecord {
    pub name: String,
    pub enabled: Option<bool>,
    pub version: Option<String>,
    pub marketplace: Option<String>,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginObservation {
    pub plugins: Vec<PluginRecord>,
    pub marketplaces: Value,
    pub provider_version: String,
    pub compatibility: String,
    pub limitations: Vec<String>,
}

#[must_use]
pub fn capabilities(provider: ProviderKind) -> Value {
    let (label, actions, limitation) = match provider {
        ProviderKind::ClaudeCode => (
            "Native",
            vec![
                "list",
                "marketplace.list",
                "marketplace.add",
                "marketplace.remove",
                "install",
                "uninstall",
                "enable",
                "disable",
            ],
            "user scope only",
        ),
        ProviderKind::Codex => (
            "Native",
            vec![
                "list",
                "marketplace.list",
                "marketplace.add",
                "marketplace.remove",
                "install",
                "uninstall",
            ],
            "enable/disable unsupported: no native CLI command; reviewed TOML writer is MCP-only",
        ),
        ProviderKind::GitHubCopilot => (
            "Native",
            vec![
                "list",
                "marketplace.list",
                "marketplace.add",
                "marketplace.remove",
                "install",
                "uninstall",
            ],
            "installed state read from comment-bearing native config and settings; enable/disable unsupported: installed 1.0.79 rejects the advertised plugins command outside its dashboard",
        ),
        ProviderKind::Antigravity => (
            "Native",
            vec!["list", "install", "uninstall", "enable", "disable"],
            "marketplace management and cross-harness Claude marketplace conversion unsupported by this adapter",
        ),
        ProviderKind::OpenCode => (
            "Translated with documented limitations",
            vec!["list", "install"],
            "npm modules only; local .js/.ts are inventoried; removal and toggles unsupported; no Claude-format marketplaces",
        ),
        ProviderKind::GrokBuild => ("Unsupported", vec![], "excluded from core v1"),
    };
    json!({"provider":provider,"compatibility":label,"actions":actions,"limitations":[limitation],"sessionEffect":"Requires provider restart","authentication":"Requires user OAuth or native setup"})
}

pub fn validate_identifier(value: &str) -> Result<(), ExtensionError> {
    if value.is_empty()
        || value.len() > 256
        || value.starts_with('-')
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._/-".contains(&b))
        || value.split('/').any(|v| v == "..")
    {
        return Err(ExtensionError::Invalid(
            "plugin/marketplace identifier contains unsafe characters".into(),
        ));
    }
    Ok(())
}

pub fn validate_marketplace(selection: &MarketplaceSelection) -> Result<(), ExtensionError> {
    validate_identifier(&selection.name)?;
    super::normalize_source(&super::SkillSource {
        source: selection.source.clone(),
        source_kind: "auto".into(),
        git_ref: None,
    })?;
    // Native plugin installers accept these values as sources, not command fragments.
    if selection.source.starts_with('-') {
        return Err(ExtensionError::Invalid(
            "marketplace option injection".into(),
        ));
    }
    Ok(())
}

#[must_use]
pub fn marketplace_source_matches(observed: &Value, requested: &str) -> bool {
    fn identity(source: &str) -> Option<Value> {
        let input = super::SkillSource {
            source: source.into(),
            source_kind: "auto".into(),
            git_ref: None,
        };
        match super::normalize_source(&input).ok()? {
            super::SourceAddress::Local(path) => {
                Some(json!({"local":path.to_string_lossy().replace('/', "\\").to_lowercase()}))
            }
            super::SourceAddress::Git {
                url,
                reference,
                selected,
                subpath,
            } => Some(
                json!({"git":url.trim_end_matches(".git"),"ref":reference.or(selected),"path":subpath}),
            ),
        }
    }
    observed.as_str().is_some_and(|source| {
        identity(source).is_some_and(|actual| Some(actual) == identity(requested))
    })
}

pub fn mutation_args(
    provider: ProviderKind,
    action: PluginAction,
    name: &str,
    marketplace: Option<&MarketplaceSelection>,
) -> Result<Vec<String>, ExtensionError> {
    validate_identifier(name)?;
    if let Some(selection) = marketplace {
        validate_marketplace(selection)?;
    }
    let action_name = action.method().trim_start_matches("plugin.");
    if !capabilities(provider)["actions"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v == action_name))
    {
        return Err(ExtensionError::Unsupported(
            "native plugin action is unavailable; inspect capabilities",
        ));
    }
    if marketplace.is_some()
        && matches!(provider, ProviderKind::OpenCode | ProviderKind::Antigravity)
    {
        return Err(ExtensionError::Unsupported(
            "Claude-format marketplace not supported by this adapter",
        ));
    }
    let args: Vec<&str> = match (provider, action) {
        (ProviderKind::ClaudeCode, PluginAction::Install) => {
            vec!["plugin", "install", name, "--scope", "user"]
        }
        (ProviderKind::ClaudeCode, PluginAction::Uninstall) => {
            vec!["plugin", "uninstall", name, "--scope", "user"]
        }
        (ProviderKind::ClaudeCode, PluginAction::Enable) => {
            vec!["plugin", "enable", name, "--scope", "user"]
        }
        (ProviderKind::ClaudeCode, PluginAction::Disable) => {
            vec!["plugin", "disable", name, "--scope", "user"]
        }
        (ProviderKind::Codex, PluginAction::Install) => vec!["plugin", "add", name],
        (ProviderKind::Codex, PluginAction::Uninstall) => vec!["plugin", "remove", name],
        (ProviderKind::GitHubCopilot | ProviderKind::Antigravity, PluginAction::Install) => {
            vec!["plugin", "install", name]
        }
        (ProviderKind::GitHubCopilot | ProviderKind::Antigravity, PluginAction::Uninstall) => {
            vec!["plugin", "uninstall", name]
        }
        (ProviderKind::Antigravity, PluginAction::Enable) => vec!["plugin", "enable", name],
        (ProviderKind::Antigravity, PluginAction::Disable) => vec!["plugin", "disable", name],
        (ProviderKind::OpenCode, PluginAction::Install) => vec!["plugin", name, "-g"],
        (_, PluginAction::MarketplaceAdd) => vec![
            "plugin",
            "marketplace",
            "add",
            &marketplace
                .ok_or_else(|| ExtensionError::Invalid("marketplace source required".into()))?
                .source,
        ],
        (_, PluginAction::MarketplaceRemove) => vec!["plugin", "marketplace", "remove", name],
        _ => return Err(ExtensionError::Unsupported("native action unavailable")),
    };
    Ok(args.into_iter().map(str::to_owned).collect())
}

pub async fn invoke(
    exe: &Path,
    home: &HarnessHome,
    args: &[String],
) -> Result<Vec<u8>, ExtensionError> {
    run_command(
        exe,
        args,
        &home.home,
        &home.environment,
        Duration::from_secs(45),
    )
    .await
}

pub async fn observe(exe: &Path, home: &HarnessHome) -> Result<PluginObservation, ExtensionError> {
    let provider = home.target.provider;
    if provider == ProviderKind::GrokBuild {
        return Err(ExtensionError::Unsupported("Grok Build is excluded"));
    }
    let version = invoke(exe, home, &["--version".into()]).await?;
    let provider_version = String::from_utf8_lossy(&version)
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_owned();
    if provider_version.len() > 128 || provider_version.chars().any(char::is_control) {
        return Err(ExtensionError::Format);
    }
    let plugins = if provider == ProviderKind::OpenCode {
        opencode_plugins(home)?
    } else if provider == ProviderKind::GitHubCopilot {
        invoke(exe, home, &["plugin".into(), "list".into()]).await?;
        copilot_plugins(home)?
    } else {
        let output = invoke(
            exe,
            home,
            &["plugin".into(), "list".into(), "--json".into()],
        )
        .await?;
        parse_plugins(provider, &serde_json::from_slice(&output)?)?
    };
    let marketplaces = marketplaces(exe, home).await?;
    let caps = capabilities(provider);
    Ok(PluginObservation {
        plugins,
        marketplaces,
        provider_version,
        compatibility: caps["compatibility"]
            .as_str()
            .unwrap_or("Unsupported")
            .into(),
        limitations: caps["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
    })
}

pub async fn marketplaces(exe: &Path, home: &HarnessHome) -> Result<Value, ExtensionError> {
    match home.target.provider {
        ProviderKind::ClaudeCode | ProviderKind::Codex => {
            let output = invoke(
                exe,
                home,
                &[
                    "plugin".into(),
                    "marketplace".into(),
                    "list".into(),
                    "--json".into(),
                ],
            )
            .await?;
            let value: Value = serde_json::from_slice(&output)?;
            // Only public, credential-free identities; raw native output never escapes.
            normalize_marketplaces(&value)
        }
        ProviderKind::GitHubCopilot => {
            let output = invoke(
                exe,
                home,
                &["plugin".into(), "marketplace".into(), "list".into()],
            )
            .await?;
            parse_copilot_marketplaces(&output)
        }
        _ => Ok(json!([])),
    }
}

fn parse_copilot_marketplaces(bytes: &[u8]) -> Result<Value, ExtensionError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ExtensionError::Format)?;
    let mut result = Vec::new();
    let mut recognized = false;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if [
            "Included with GitHub Copilot:",
            "Registered marketplaces:",
            "No marketplaces registered.",
        ]
        .contains(&line)
        {
            recognized = true;
            continue;
        }
        let line = line
            .strip_prefix('◆')
            .or_else(|| line.strip_prefix('•'))
            .ok_or(ExtensionError::Format)?
            .trim();
        let (name, source) = line.split_once(" (").ok_or(ExtensionError::Format)?;
        validate_identifier(name)?;
        let (_, source) = source
            .strip_suffix(')')
            .and_then(|s| s.split_once(": "))
            .ok_or(ExtensionError::Format)?;
        result.push(json!({"name":name,"source":public_source(&json!(source))}));
    }
    if !recognized {
        return Err(ExtensionError::Format);
    }
    result.sort_by_key(|m| m["name"].as_str().unwrap_or("").to_owned());
    Ok(json!(result))
}

fn normalize_marketplaces(value: &Value) -> Result<Value, ExtensionError> {
    let items = value
        .as_array()
        .or_else(|| value.get("marketplaces").and_then(Value::as_array))
        .ok_or(ExtensionError::Format)?;
    let mut result = Vec::new();
    for item in items {
        let name = item
            .get("name")
            .or_else(|| item.get("marketplaceName"))
            .and_then(Value::as_str)
            .ok_or(ExtensionError::Format)?;
        validate_identifier(name)?;
        let source = item
            .get("repo")
            .or_else(|| item.get("url"))
            .or_else(|| item.get("marketplaceSource"))
            .or_else(|| item.get("root").filter(|_| item.get("source").is_none()))
            .unwrap_or(&item["source"]);
        result.push(json!({"name":name,"source":public_source(source)}));
    }
    result.sort_by_key(|item| item["name"].as_str().unwrap_or("").to_owned());
    Ok(Value::Array(result))
}

fn public_source(value: &Value) -> Value {
    let value = redact_json(value.clone());
    let source = value
        .as_str()
        .or_else(|| value.get("repo").and_then(Value::as_str))
        .or_else(|| value.get("url").and_then(Value::as_str))
        .or_else(|| value.get("source").and_then(Value::as_str));
    match source {
        Some(s) if !s.contains(['?', '#', '@']) && s.len() < 1024 => json!(s),
        _ => Value::Null,
    }
}

pub fn parse_plugins(
    provider: ProviderKind,
    value: &Value,
) -> Result<Vec<PluginRecord>, ExtensionError> {
    let items = if provider == ProviderKind::Codex {
        value.get("installed").and_then(Value::as_array)
    } else {
        value.as_array()
    }
    .ok_or(ExtensionError::Format)?;
    let mut result = Vec::new();
    for item in items {
        let name = item
            .get("id")
            .or_else(|| item.get("pluginId"))
            .or_else(|| item.get("name"))
            .and_then(Value::as_str)
            .ok_or(ExtensionError::Format)?;
        validate_identifier(name)?;
        let version = item
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if version
            .as_ref()
            .is_some_and(|v| v.len() > 128 || v.chars().any(char::is_control))
        {
            return Err(ExtensionError::Format);
        }
        let marketplace = item
            .get("marketplaceName")
            .or_else(|| item.get("marketplace"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| name.split_once('@').map(|(_, m)| m.to_owned()));
        if let Some(m) = &marketplace {
            validate_identifier(m)?;
        }
        result.push(PluginRecord {
            name: name.into(),
            enabled: item.get("enabled").and_then(Value::as_bool),
            version,
            marketplace,
            path: item
                .get("installPath")
                .or_else(|| item.get("path"))
                .or_else(|| item.get("cache_path"))
                .or_else(|| {
                    (provider == ProviderKind::Codex && item["source"]["source"] == "local")
                        .then(|| item["source"].get("path"))
                        .flatten()
                })
                .and_then(Value::as_str)
                .map(PathBuf::from),
        });
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

fn copilot_plugins(home: &HarnessHome) -> Result<Vec<PluginRecord>, ExtensionError> {
    let config = read_jsonc_optional(&home.config.join("config.json"))?;
    let settings = read_jsonc_optional(&home.config.join("settings.json"))?;
    let mut result = parse_plugins(
        ProviderKind::Antigravity,
        config.get("installedPlugins").unwrap_or(&json!([])),
    )?;
    for entry in &mut result {
        let qualified = entry
            .marketplace
            .as_ref()
            .map_or_else(|| entry.name.clone(), |m| format!("{}@{m}", entry.name));
        if let Some(enabled) = settings["enabledPlugins"][&qualified]
            .as_bool()
            .or_else(|| settings["enabledPlugins"][&entry.name].as_bool())
        {
            entry.enabled = Some(enabled);
        }
    }
    Ok(result)
}

fn opencode_plugins(home: &HarnessHome) -> Result<Vec<PluginRecord>, ExtensionError> {
    let mut result = Vec::new();
    for filename in ["opencode.json", "opencode.jsonc"] {
        let config = read_jsonc_optional(&home.config.join(filename))?;
        if let Some(items) = config["plugin"].as_array() {
            for value in items {
                let name = value.as_str().ok_or(ExtensionError::Format)?;
                validate_identifier(name)?;
                result.push(PluginRecord {
                    name: name.into(),
                    enabled: Some(true),
                    version: None,
                    marketplace: None,
                    path: None,
                });
            }
        }
    }
    let dir = home.config.join("plugins");
    if dir.is_dir() {
        for item in fs::read_dir(dir)?.take(4097) {
            let path = item?.path();
            if path.extension().is_some_and(|x| x == "js" || x == "ts") {
                result.push(PluginRecord {
                    name: path.file_name().unwrap().to_string_lossy().into_owned(),
                    enabled: Some(true),
                    version: None,
                    marketplace: None,
                    path: Some(path),
                });
            }
        }
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result.dedup_by(|a, b| a.name == b.name);
    Ok(result)
}

pub fn read_jsonc_optional(path: &Path) -> Result<Value, ExtensionError> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let bytes = read_bounded(path, 1024 * 1024)?;
    parse_jsonc(&bytes)
}

pub fn parse_jsonc(bytes: &[u8]) -> Result<Value, ExtensionError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ExtensionError::Format)?
        .trim_start_matches('\u{feff}');
    let mut out = text.as_bytes().to_vec();
    let mut i = 0;
    let mut quoted = false;
    let mut escaped = false;
    while i < out.len() {
        let b = out[i];
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
            i += 1;
            continue;
        }
        if b == b'"' {
            quoted = true;
            i += 1;
            continue;
        }
        if b == b'/' && out.get(i + 1) == Some(&b'/') {
            while i < out.len() && out[i] != b'\n' {
                out[i] = b' ';
                i += 1;
            }
        } else if b == b'/' && out.get(i + 1) == Some(&b'*') {
            out[i] = b' ';
            out[i + 1] = b' ';
            i += 2;
            let mut ended = false;
            while i < out.len() {
                if out[i] == b'*' && out.get(i + 1) == Some(&b'/') {
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    i += 2;
                    ended = true;
                    break;
                }
                out[i] = b' ';
                i += 1;
            }
            if !ended {
                return Err(ExtensionError::Format);
            }
        } else {
            i += 1;
        }
    }
    quoted = false;
    escaped = false;
    for i in 0..out.len() {
        let b = out[i];
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else if b == b'"' {
            quoted = true;
        } else if b == b','
            && out[i + 1..]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| *b == b'}' || *b == b']')
        {
            out[i] = b' ';
        }
    }
    Ok(serde_json::from_slice(&out)?)
}

/// Known native control files only. Private backup bytes are never returned by RPC.
#[must_use]
pub fn control_files(home: &HarnessHome) -> Vec<PathBuf> {
    let names: &[&str] = match home.target.provider {
        ProviderKind::ClaudeCode => &[
            "settings.json",
            "plugins/installed_plugins.json",
            "plugins/known_marketplaces.json",
        ],
        ProviderKind::Codex => &[
            "config.toml",
            "plugins/installed_plugins.json",
            "plugins/marketplaces.json",
        ],
        ProviderKind::GitHubCopilot => &["config.json", "settings.json"],
        ProviderKind::Antigravity => &["config.json"],
        ProviderKind::OpenCode => &["opencode.json", "opencode.jsonc"],
        ProviderKind::GrokBuild => &[],
    };
    names.iter().map(|name| home.config.join(name)).collect()
}

#[must_use]
pub fn matches_plugin(plugin: &PluginRecord, requested: &str) -> bool {
    if plugin.name == requested {
        return true;
    }
    if plugin.name.starts_with('@') || requested.starts_with('@') {
        return false;
    }
    let (name, market) = requested
        .split_once('@')
        .map_or((requested, None), |(n, m)| (n, Some(m)));
    let (actual_name, actual_market) = plugin.name.split_once('@').map_or(
        (plugin.name.as_str(), plugin.marketplace.as_deref()),
        |(n, m)| (n, Some(m)),
    );
    name == actual_name && market.is_none_or(|m| Some(m) == actual_market)
}

#[must_use]
pub fn observed_satisfies(action: PluginAction, name: &str, after: &PluginObservation) -> bool {
    let found = after.plugins.iter().find(|p| matches_plugin(p, name));
    match action {
        PluginAction::Install => found.is_some(),
        PluginAction::Uninstall => found.is_none(),
        PluginAction::Enable => found.is_some_and(|p| p.enabled == Some(true)),
        PluginAction::Disable => found.is_some_and(|p| p.enabled == Some(false)),
        PluginAction::MarketplaceAdd => after
            .marketplaces
            .as_array()
            .is_some_and(|items| items.iter().any(|m| m["name"] == name)),
        PluginAction::MarketplaceRemove => after
            .marketplaces
            .as_array()
            .is_some_and(|items| items.iter().all(|m| m["name"] != name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marketplace_sources_compare_identity_and_parse_native_shapes() {
        assert!(marketplace_source_matches(
            &json!("https://github.com/owner/repo.git"),
            "owner/repo"
        ));
        assert!(!marketplace_source_matches(
            &json!("other/repo"),
            "owner/repo"
        ));
        assert!(!marketplace_source_matches(
            &json!("owner/repo@other"),
            "owner/repo@main"
        ));
        assert_eq!(
            normalize_marketplaces(
                &json!([{"name":"fixture","source":"github","repo":"owner/repo"}])
            )
            .unwrap()[0]["source"],
            "owner/repo"
        );
        let codex = normalize_marketplaces(&json!({"marketplaces":[
            {"name":"local","root":"C:\\fixture\\market"},
            {"name":"remote","root":"C:\\fixture\\cache","marketplaceSource":{"sourceType":"git","source":"https://github.com/owner/repo.git"}}
        ]})).unwrap();
        assert_eq!(codex[0]["source"], "C:\\fixture\\market");
        assert!(marketplace_source_matches(
            &codex[1]["source"],
            "owner/repo"
        ));
        let installed = parse_plugins(
            ProviderKind::Codex,
            &json!({"installed":[{
                "pluginId":"fixture@local","marketplaceName":"local","enabled":true,
                "source":{"source":"local","path":"C:\\fixture\\plugins\\fixture"}
            }]}),
        )
        .unwrap();
        assert_eq!(
            installed[0].path,
            Some(PathBuf::from("C:\\fixture\\plugins\\fixture"))
        );
    }

    #[test]
    fn plugin_identity_checks_the_requested_marketplace() {
        let record = PluginRecord {
            name: "fixture".into(),
            enabled: Some(true),
            version: None,
            marketplace: Some("actual".into()),
            path: None,
        };
        assert!(matches_plugin(&record, "fixture"));
        assert!(matches_plugin(&record, "fixture@actual"));
        assert!(!matches_plugin(&record, "fixture@wrong"));
    }
    #[test]
    fn jsonc_preserves_comment_markers_inside_strings() {
        assert_eq!(
            parse_jsonc(
                br#"// header
        {"url":"https://host/a//b", /* x */ "plugin":["foo",],}"#
            )
            .unwrap()["url"],
            "https://host/a//b"
        );
    }
    #[test]
    fn copilot_marketplaces_include_native_defaults_and_local_sources() {
        let result = parse_copilot_marketplaces("Included with GitHub Copilot:\n  ◆ copilot-plugins (GitHub: github/copilot-plugins)\n\nRegistered marketplaces:\n  • fixture (Local: C:\\temp\\fixture)\n".as_bytes()).unwrap();
        assert_eq!(result[0]["name"], "copilot-plugins");
        assert_eq!(result[1]["source"], "C:\\temp\\fixture");
        assert!(parse_copilot_marketplaces(b"new unknown CLI format").is_err());
    }
    #[test]
    fn unsupported_config_edits_never_produce_commands() {
        for p in [
            ProviderKind::Codex,
            ProviderKind::GitHubCopilot,
            ProviderKind::OpenCode,
            ProviderKind::GrokBuild,
        ] {
            assert!(mutation_args(p, PluginAction::Disable, "test", None).is_err());
        }
    }
    #[test]
    fn copilot_advertised_but_unavailable_toggle_is_refused() {
        assert!(
            mutation_args(
                ProviderKind::GitHubCopilot,
                PluginAction::Enable,
                "test@market",
                None
            )
            .is_err()
        );
    }
}
