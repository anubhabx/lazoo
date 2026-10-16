use std::fs;

use lazoo_providers::{
    ConfigurationFormat, DiscoveryEnvironment, IntegrationTransport, PROVIDER_CONFIGURATION_V1,
    ProviderKind, discover_provider_configuration,
};
use tempfile::tempdir;

const CODEX_CONFIG: &str = r#"
model = "gpt-5-codex"

[mcp_servers.context7]
url = "https://mcp.context7.com/mcp"

[mcp_servers.context7.http_headers]
CONTEXT7_API_KEY = "SECRET-ONE"

[mcp_servers.firecrawl]
command = "powershell"
args = ["-NoProfile", "-File", 'C:\Users\me\.codex\bin\firecrawl-mcp.ps1']
env = { FIRECRAWL_API_KEY = "SECRET-TWO" }

[mcp_servers.paper]
url = "http://127.0.0.1:29979/mcp"
enabled = false

[mcp_servers.ghost]
startup_timeout_sec = 120.0
"#;

const CLAUDE_CONFIG: &str = r#"{
  "numStartups": 3,
  "mcpServers": {
    "context7": {"type": "http", "url": "https://MCP.context7.com/mcp/", "headers": {"CONTEXT7_API_KEY": "SECRET-THREE"}},
    "paper": {"type": "http", "url": "http://127.0.0.1:31000/mcp?token=SECRET-FOUR"},
    "obsidian": {"command": "npx", "args": ["obsidian-mcp"], "env": {"OBSIDIAN_TOKEN": "SECRET-FIVE"}},
    "broken": 7
  }
}"#;

const OPENCODE_CONFIG: &str = r#"{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "context7": {"type": "remote", "url": "https://mcp.context7.com/mcp", "headers": {"Authorization": "Bearer SECRET-SIX"}},
    "local-tool": {"type": "local", "command": ["bun", "x", "my-mcp"], "environment": {"MY_TOKEN": "SECRET-SEVEN"}, "enabled": false}
  }
}"#;

fn home_with(
    codex: Option<&str>,
    claude: Option<&str>,
    opencode: Option<&str>,
) -> tempfile::TempDir {
    let home = tempdir().unwrap();
    if let Some(text) = codex {
        fs::create_dir_all(home.path().join(".codex")).unwrap();
        fs::write(home.path().join(".codex").join("config.toml"), text).unwrap();
    }
    if let Some(text) = claude {
        fs::write(home.path().join(".claude.json"), text).unwrap();
    }
    if let Some(text) = opencode {
        let directory = home.path().join(".config").join("opencode");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("opencode.json"), text).unwrap();
    }
    home
}

#[test]
#[allow(clippy::too_many_lines)] // One report over three providers, read top to bottom.
fn provider_configuration_is_parsed_from_known_formats_redacted_hashed_and_compared() {
    let home = home_with(
        Some(CODEX_CONFIG),
        Some(CLAUDE_CONFIG),
        Some(OPENCODE_CONFIG),
    );
    let environment = DiscoveryEnvironment::empty().with_config_home(home.path());

    let report = discover_provider_configuration(&environment);
    assert_eq!(report.version, PROVIDER_CONFIGURATION_V1);

    // Every known file was found and parsed; the two providers without a known format are named.
    let files: Vec<(ProviderKind, ConfigurationFormat, bool, bool)> = report
        .files
        .iter()
        .map(|file| (file.provider, file.format, file.exists, file.parsed))
        .collect();
    assert_eq!(
        files,
        [
            (ProviderKind::Codex, ConfigurationFormat::Toml, true, true),
            (
                ProviderKind::OpenCode,
                ConfigurationFormat::Json,
                true,
                true
            ),
            (
                ProviderKind::ClaudeCode,
                ConfigurationFormat::Json,
                true,
                true
            ),
        ]
    );
    assert!(report.files.iter().all(|file| file.error.is_none()));
    assert!(
        report
            .unsupported
            .contains(&"grok-build: no known configuration format".to_owned())
    );
    assert!(
        report
            .unsupported
            .contains(&"antigravity: no known configuration format".to_owned())
    );
    assert!(
        report
            .unsupported
            .contains(&"codex/ghost: neither command nor url".to_owned())
    );
    assert!(
        report
            .unsupported
            .contains(&"broken: not an object".to_owned())
    );

    // No secret value survives anywhere in the report; the keys the provider sends are named.
    let serialized = serde_json::to_string(&report).unwrap();
    for secret in [
        "SECRET-ONE",
        "SECRET-TWO",
        "SECRET-THREE",
        "SECRET-FOUR",
        "SECRET-FIVE",
        "SECRET-SIX",
        "SECRET-SEVEN",
        "token=",
    ] {
        assert!(!serialized.contains(secret), "{secret} leaked");
    }
    let find = |provider: ProviderKind, name: &str| {
        report
            .integrations
            .iter()
            .find(|integration| integration.provider == provider && integration.name == name)
            .unwrap_or_else(|| panic!("{provider}/{name}"))
    };
    let codex_context7 = find(ProviderKind::Codex, "context7");
    assert_eq!(codex_context7.transport, IntegrationTransport::Http);
    assert_eq!(
        codex_context7.url.as_deref(),
        Some("https://mcp.context7.com/mcp")
    );
    assert_eq!(codex_context7.secret_keys, ["CONTEXT7_API_KEY"]);
    assert!(codex_context7.enabled);
    assert_eq!(codex_context7.command, None);
    let firecrawl = find(ProviderKind::Codex, "firecrawl");
    assert_eq!(firecrawl.transport, IntegrationTransport::Stdio);
    assert_eq!(firecrawl.command.as_deref(), Some("powershell"));
    assert_eq!(firecrawl.args.len(), 3);
    assert_eq!(firecrawl.secret_keys, ["FIRECRAWL_API_KEY"]);
    assert!(!find(ProviderKind::Codex, "paper").enabled);
    let claude_paper = find(ProviderKind::ClaudeCode, "paper");
    assert_eq!(
        claude_paper.url.as_deref(),
        Some("http://127.0.0.1:31000/mcp")
    );
    assert_eq!(claude_paper.secret_keys, ["url-query"]);
    let obsidian = find(ProviderKind::ClaudeCode, "obsidian");
    assert_eq!(obsidian.transport, IntegrationTransport::Stdio);
    assert_eq!(obsidian.args, ["obsidian-mcp"]);
    assert_eq!(obsidian.secret_keys, ["OBSIDIAN_TOKEN"]);
    let local_tool = find(ProviderKind::OpenCode, "local-tool");
    assert_eq!(local_tool.command.as_deref(), Some("bun"));
    assert_eq!(local_tool.args, ["x", "my-mcp"]);
    assert_eq!(local_tool.secret_keys, ["MY_TOKEN"]);
    assert!(!local_tool.enabled);

    // The same server configured three times (case and trailing slash aside) is one identity;
    // the same name pointing at two different servers is a conflict.
    assert_eq!(
        find(ProviderKind::ClaudeCode, "context7").identity,
        codex_context7.identity
    );
    assert_eq!(
        report.duplicates,
        [[
            "claude-code/context7",
            "codex/context7",
            "opencode/context7"
        ]]
    );
    assert_eq!(report.conflicts, [["claude-code/paper", "codex/paper"]]);
    assert_ne!(
        claude_paper.identity,
        find(ProviderKind::Codex, "paper").identity
    );
    assert_eq!(report.integrations.len(), 8);
    for integration in &report.integrations {
        assert_eq!(integration.identity.len(), 64);
        assert!(integration.source.is_file());
    }
}

#[test]
fn missing_and_malformed_configuration_is_reported_without_quoting_it_or_failing() {
    let empty = DiscoveryEnvironment::empty();
    let report = discover_provider_configuration(&empty);
    assert!(report.files.is_empty());
    assert!(report.integrations.is_empty());
    assert_eq!(report.unsupported.len(), 5, "{:?}", report.unsupported);

    let home = home_with(
        Some(
            "[mcp_servers.x
SECRET-EIGHT = \"oops\"",
        ),
        None,
        Some(OPENCODE_CONFIG),
    );
    let environment = DiscoveryEnvironment::empty().with_config_home(home.path());
    let report = discover_provider_configuration(&environment);
    let codex = report
        .files
        .iter()
        .find(|file| file.provider == ProviderKind::Codex)
        .unwrap();
    assert!(codex.exists);
    assert!(!codex.parsed);
    let error = codex.error.as_deref().unwrap();
    assert!(error.contains("not valid toml"), "{error}");
    assert!(!error.contains("SECRET-EIGHT"), "{error}");
    let claude = report
        .files
        .iter()
        .find(|file| file.provider == ProviderKind::ClaudeCode)
        .unwrap();
    assert!(!claude.exists);
    assert!(!claude.parsed);
    assert_eq!(claude.error, None);
    assert_eq!(
        report.integrations.len(),
        2,
        "the readable file still counts"
    );
    assert!(report.duplicates.is_empty());
    assert!(report.conflicts.is_empty());

    // An explicit CODEX_HOME wins over <home>/.codex.
    let codex_home = tempdir().unwrap();
    fs::write(codex_home.path().join("config.toml"), CODEX_CONFIG).unwrap();
    let environment = DiscoveryEnvironment::empty()
        .with_config_home(home.path())
        .with_codex_home(codex_home.path());
    let report = discover_provider_configuration(&environment);
    let codex = report
        .files
        .iter()
        .find(|file| file.provider == ProviderKind::Codex)
        .unwrap();
    assert_eq!(codex.path, codex_home.path().join("config.toml"));
    assert!(codex.parsed);
}
