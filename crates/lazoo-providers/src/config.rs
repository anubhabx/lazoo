//! Read-only provider configuration discovery (brief section 16.1, steps 1 to 5): find each
//! provider's known configuration file, parse only the formats Lazoo knows, redact secrets,
//! hash and normalize the integrations, and name exact duplicates and same-name conflicts.
//! Nothing here writes, and provider-native files stay authoritative.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{DiscoveryEnvironment, ProviderKind};

pub const PROVIDER_CONFIGURATION_V1: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigurationFormat {
    Toml,
    Json,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntegrationKind {
    McpServer,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntegrationTransport {
    Stdio,
    Http,
}

/// One configuration file Lazoo knows how to read. A missing file is reported, not an error;
/// a malformed one is reported with a message that never quotes the file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderConfigurationFile {
    pub provider: ProviderKind,
    pub path: PathBuf,
    pub format: ConfigurationFormat,
    pub exists: bool,
    pub parsed: bool,
    pub error: Option<String>,
}

/// One integration as a provider has it configured, with every secret value removed:
/// `secretKeys` names the environment variables and headers the provider will send, never
/// their values, and a URL keeps no query string.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DiscoveredIntegration {
    pub provider: ProviderKind,
    pub name: String,
    pub kind: IntegrationKind,
    pub transport: IntegrationTransport,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub secret_keys: Vec<String>,
    pub enabled: bool,
    /// SHA-256 of the normalized transport, command, arguments, and URL; equal identities are
    /// the same integration wherever it is configured.
    pub identity: String,
    pub source: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderConfigurationReport {
    pub version: u32,
    pub scanned_at: String,
    pub files: Vec<ProviderConfigurationFile>,
    pub integrations: Vec<DiscoveredIntegration>,
    /// Groups of `provider/name` whose identities are equal.
    pub duplicates: Vec<Vec<String>>,
    /// Groups of `provider/name` that share a name but not an identity.
    pub conflicts: Vec<Vec<String>>,
    /// Providers with no known configuration format, and entries Lazoo could not classify.
    pub unsupported: Vec<String>,
}

/// Reads every known provider configuration file under the environment's config home. Only
/// the formats below are parsed; everything else is reported as unsupported.
#[must_use]
pub fn discover_provider_configuration(
    environment: &DiscoveryEnvironment,
) -> ProviderConfigurationReport {
    let mut report = ProviderConfigurationReport {
        version: PROVIDER_CONFIGURATION_V1,
        scanned_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        files: Vec::new(),
        integrations: Vec::new(),
        duplicates: Vec::new(),
        conflicts: Vec::new(),
        unsupported: Vec::new(),
    };
    for provider in ProviderKind::ALL {
        match known_configuration(environment, provider) {
            Some((path, format)) => read_file(&mut report, provider, &path, format),
            None => report
                .unsupported
                .push(format!("{provider}: no known configuration format")),
        }
    }
    report.duplicates = groups(&report.integrations, |integration| {
        integration.identity.clone()
    });
    report.conflicts = groups(&report.integrations, |integration| {
        integration.name.to_lowercase()
    })
    .into_iter()
    .filter(|group| {
        let identities = group
            .iter()
            .filter_map(|label| {
                report
                    .integrations
                    .iter()
                    .find(|integration| integration_label(integration) == *label)
            })
            .map(|integration| integration.identity.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        identities.len() > 1
    })
    .collect();
    report
}

fn known_configuration(
    environment: &DiscoveryEnvironment,
    provider: ProviderKind,
) -> Option<(PathBuf, ConfigurationFormat)> {
    match provider {
        ProviderKind::Codex => {
            let codex_home = match environment.codex_home() {
                Some(codex_home) => codex_home.to_path_buf(),
                None => environment.config_home()?.join(".codex"),
            };
            Some((codex_home.join("config.toml"), ConfigurationFormat::Toml))
        }
        ProviderKind::ClaudeCode => Some((
            environment.config_home()?.join(".claude.json"),
            ConfigurationFormat::Json,
        )),
        ProviderKind::OpenCode => Some((
            environment
                .config_home()?
                .join(".config")
                .join("opencode")
                .join("opencode.json"),
            ConfigurationFormat::Json,
        )),
        ProviderKind::GrokBuild | ProviderKind::Antigravity => None,
    }
}

fn read_file(
    report: &mut ProviderConfigurationReport,
    provider: ProviderKind,
    path: &Path,
    format: ConfigurationFormat,
) {
    let mut file = ProviderConfigurationFile {
        provider,
        path: path.to_path_buf(),
        format,
        exists: path.is_file(),
        parsed: false,
        error: None,
    };
    if file.exists {
        // ponytail: parser messages can quote the offending line, which may hold a secret, so
        // the report says only that the file did not parse.
        match fs::read_to_string(path) {
            Ok(text) => match parse(provider, path, format, &text) {
                Ok(parsed) => {
                    file.parsed = true;
                    for entry in parsed {
                        match entry {
                            Ok(integration) => report.integrations.push(integration),
                            Err(unsupported) => report.unsupported.push(unsupported),
                        }
                    }
                }
                Err(()) => {
                    file.error = Some(
                        format!("{provider} configuration is not valid {format:?}").to_lowercase(),
                    );
                }
            },
            Err(error) => {
                file.error = Some(format!(
                    "{provider} configuration could not be read: {}",
                    error.kind()
                ));
            }
        }
    }
    report.files.push(file);
}

type ParsedEntries = Vec<Result<DiscoveredIntegration, String>>;

fn parse(
    provider: ProviderKind,
    path: &Path,
    format: ConfigurationFormat,
    text: &str,
) -> Result<ParsedEntries, ()> {
    match (provider, format) {
        (ProviderKind::Codex, ConfigurationFormat::Toml) => {
            let table: toml::Table = text.parse().map_err(|_| ())?;
            Ok(codex_servers(&table, path))
        }
        (ProviderKind::ClaudeCode, ConfigurationFormat::Json) => {
            let value: Value = serde_json::from_str(text).map_err(|_| ())?;
            Ok(claude_servers(&value, path))
        }
        (ProviderKind::OpenCode, ConfigurationFormat::Json) => {
            let value: Value = serde_json::from_str(text).map_err(|_| ())?;
            Ok(opencode_servers(&value, path))
        }
        _ => Ok(Vec::new()),
    }
}

/// `[mcp_servers.<name>]` with `command`/`args`/`env` or `url`/`http_headers`/`env_http_headers`.
fn codex_servers(table: &toml::Table, path: &Path) -> ParsedEntries {
    let Some(servers) = table.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    servers
        .iter()
        .map(|(name, server)| {
            let server = server
                .as_table()
                .ok_or_else(|| format!("codex/{name}: not a table"))?;
            let command = server.get("command").and_then(toml::Value::as_str);
            let args = server
                .get("args")
                .and_then(toml::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(toml::Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let url = server.get("url").and_then(toml::Value::as_str);
            let mut secret_keys = Vec::new();
            for field in ["env", "http_headers", "env_http_headers"] {
                if let Some(keys) = server.get(field).and_then(toml::Value::as_table) {
                    secret_keys.extend(keys.keys().cloned());
                }
            }
            let enabled = server
                .get("enabled")
                .and_then(toml::Value::as_bool)
                .unwrap_or(true);
            build(
                ProviderKind::Codex,
                name,
                command,
                args,
                url,
                secret_keys,
                enabled,
                path,
            )
        })
        .collect()
}

/// `mcpServers.<name>` with `command`/`args`/`env`, or `type: http|sse` with `url`/`headers`.
fn claude_servers(value: &Value, path: &Path) -> ParsedEntries {
    json_servers(value.get("mcpServers"), path, |name, server| {
        let command = server.get("command").and_then(Value::as_str);
        let args = string_array(server.get("args"));
        let url = server.get("url").and_then(Value::as_str);
        let mut secret_keys = Vec::new();
        for field in ["env", "headers"] {
            secret_keys.extend(object_keys(server.get(field)));
        }
        let enabled = !server
            .get("disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        build(
            ProviderKind::ClaudeCode,
            name,
            command,
            args,
            url,
            secret_keys,
            enabled,
            path,
        )
    })
}

/// `mcp.<name>` with `type: local` (`command` array, `environment`) or `type: remote` (`url`,
/// `headers`), and `enabled`.
fn opencode_servers(value: &Value, path: &Path) -> ParsedEntries {
    json_servers(value.get("mcp"), path, |name, server| {
        let mut command_line = string_array(server.get("command")).into_iter();
        let command = command_line.next();
        let args: Vec<String> = command_line.collect();
        let url = server.get("url").and_then(Value::as_str);
        let mut secret_keys = Vec::new();
        for field in ["environment", "headers"] {
            secret_keys.extend(object_keys(server.get(field)));
        }
        let enabled = server
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        build(
            ProviderKind::OpenCode,
            name,
            command.as_deref(),
            args,
            url,
            secret_keys,
            enabled,
            path,
        )
    })
}

fn json_servers(
    servers: Option<&Value>,
    path: &Path,
    entry: impl Fn(&str, &Value) -> Result<DiscoveredIntegration, String>,
) -> ParsedEntries {
    let Some(servers) = servers.and_then(Value::as_object) else {
        return Vec::new();
    };
    let _ = path;
    servers
        .iter()
        .map(|(name, server)| {
            if server.is_object() {
                entry(name, server)
            } else {
                Err(format!("{name}: not an object"))
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)] // One integration has exactly these facts.
fn build(
    provider: ProviderKind,
    name: &str,
    command: Option<&str>,
    args: Vec<String>,
    url: Option<&str>,
    mut secret_keys: Vec<String>,
    enabled: bool,
    source: &Path,
) -> Result<DiscoveredIntegration, String> {
    let (transport, url) = match (command, url) {
        (_, Some(url)) => {
            let (base, had_query) = strip_query(url);
            if had_query {
                secret_keys.push("url-query".to_owned());
            }
            (IntegrationTransport::Http, Some(base))
        }
        (Some(_), None) => (IntegrationTransport::Stdio, None),
        (None, None) => return Err(format!("{provider}/{name}: neither command nor url")),
    };
    let command =
        (transport == IntegrationTransport::Stdio).then(|| command.unwrap_or_default().to_owned());
    secret_keys.sort();
    secret_keys.dedup();
    let identity = identity(transport, command.as_deref(), &args, url.as_deref());
    Ok(DiscoveredIntegration {
        provider,
        name: name.to_owned(),
        kind: IntegrationKind::McpServer,
        transport,
        command,
        args,
        url,
        secret_keys,
        enabled,
        identity,
        source: source.to_path_buf(),
    })
}

fn strip_query(url: &str) -> (String, bool) {
    url.split_once('?').map_or_else(
        || (url.to_owned(), false),
        |(base, _)| (base.to_owned(), true),
    )
}

fn identity(
    transport: IntegrationTransport,
    command: Option<&str>,
    args: &[String],
    url: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{transport:?}").to_lowercase());
    hasher.update([0x1f]);
    hasher.update(
        command
            .unwrap_or_default()
            .replace('/', "\\")
            .to_lowercase(),
    );
    hasher.update([0x1f]);
    for argument in args {
        hasher.update(argument.replace('/', "\\").to_lowercase());
        hasher.update([0x1e]);
    }
    hasher.update([0x1f]);
    hasher.update(url.unwrap_or_default().trim_end_matches('/').to_lowercase());
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn groups(
    integrations: &[DiscoveredIntegration],
    key: impl Fn(&DiscoveredIntegration) -> String,
) -> Vec<Vec<String>> {
    let mut buckets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for integration in integrations {
        buckets
            .entry(key(integration))
            .or_default()
            .push(integration_label(integration));
    }
    buckets
        .into_values()
        .filter(|labels| labels.len() > 1)
        .map(|mut labels| {
            labels.sort();
            labels
        })
        .collect()
}

fn integration_label(integration: &DiscoveredIntegration) -> String {
    format!("{}/{}", integration.provider, integration.name)
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn object_keys(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_object)
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}
