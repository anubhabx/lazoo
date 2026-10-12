use std::fs;

use lazoo_providers::{
    DiscoveryEnvironment, MigrationProposal, PROVIDER_MIGRATION_PREVIEW_V1, ProviderKind,
    discover_provider_configuration, migration_preview,
};
use tempfile::tempdir;

const CODEX_CONFIG: &str = r#"
[mcp_servers.context7]
url = "https://mcp.context7.com/mcp"

[mcp_servers.context7.http_headers]
CONTEXT7_API_KEY = "ctx7sk-SECRET-ONE"

[mcp_servers.paper]
url = "http://127.0.0.1:29979/mcp"

[mcp_servers.ghost]
startup_timeout_sec = 120.0
"#;

const CLAUDE_CONFIG: &str = r#"{
  "mcpServers": {
    "context7": {"type": "http", "url": "https://MCP.context7.com/mcp/", "headers": {"CONTEXT7_API_KEY": "ctx7sk-SECRET-THREE"}},
    "paper": {"type": "http", "url": "http://127.0.0.1:31000/mcp?token=SECRET-FOUR"},
    "obsidian": {"command": "npx", "args": ["obsidian-mcp"], "env": {"OBSIDIAN_TOKEN": "SECRET-FIVE"}}
  }
}"#;

#[test]
fn the_preview_proposes_manage_merge_or_choose_per_identity_and_never_reads_a_secret() {
    let home = tempdir().unwrap();
    fs::create_dir_all(home.path().join(".codex")).unwrap();
    fs::write(home.path().join(".codex").join("config.toml"), CODEX_CONFIG).unwrap();
    fs::write(home.path().join(".claude.json"), CLAUDE_CONFIG).unwrap();
    let environment = DiscoveryEnvironment::empty().with_config_home(home.path());
    let report = discover_provider_configuration(&environment);
    let preview = migration_preview(&report);

    assert_eq!(preview.version, PROVIDER_MIGRATION_PREVIEW_V1);
    assert_eq!(preview.scanned_at, report.scanned_at);
    assert_eq!(
        preview.providers,
        [
            ProviderKind::Codex,
            ProviderKind::OpenCode,
            ProviderKind::ClaudeCode
        ]
    );
    let names: Vec<(&str, MigrationProposal)> = preview
        .candidates
        .iter()
        .map(|candidate| (candidate.name.as_str(), candidate.proposal))
        .collect();
    assert_eq!(
        names,
        [
            ("context7", MigrationProposal::Merge),
            ("obsidian", MigrationProposal::Manage),
            ("paper", MigrationProposal::Choose),
            ("paper", MigrationProposal::Choose),
        ]
    );

    // The exact duplicate merges: one entry, both sources, OpenCode missing, the secret key
    // named once and its value nowhere.
    let context7 = &preview.candidates[0];
    assert_eq!(context7.sources.len(), 2);
    assert_eq!(
        context7.present_in,
        [ProviderKind::Codex, ProviderKind::ClaudeCode]
    );
    assert_eq!(context7.missing_from, [ProviderKind::OpenCode]);
    assert_eq!(context7.secret_keys, ["CONTEXT7_API_KEY"]);
    assert!(
        context7
            .reason
            .contains("identically in codex, claude-code")
    );
    assert!(context7.conflicts_with.is_empty());

    // A lone integration is managed as is.
    let obsidian = &preview.candidates[1];
    assert_eq!(obsidian.present_in, [ProviderKind::ClaudeCode]);
    assert_eq!(obsidian.command.as_deref(), Some("npx"));
    assert_eq!(obsidian.secret_keys, ["OBSIDIAN_TOKEN"]);
    assert!(obsidian.reason.ends_with("Lazoo would manage it as is"));

    // The same name with different identities needs a choice, each naming the other.
    let paper: Vec<&_> = preview.candidates.iter().skip(2).collect();
    assert_eq!(paper[0].conflicts_with.len(), 1);
    assert_eq!(paper[1].conflicts_with.len(), 1);
    assert_ne!(paper[0].conflicts_with, paper[1].conflicts_with);
    assert!(
        paper
            .iter()
            .all(|candidate| candidate.reason.contains("choose one"))
    );
    let claude_paper = paper
        .iter()
        .find(|candidate| candidate.present_in == [ProviderKind::ClaudeCode])
        .unwrap();
    assert_eq!(claude_paper.secret_keys, ["url-query"]);
    assert_eq!(
        claude_paper.url.as_deref(),
        Some("http://127.0.0.1:31000/mcp")
    );

    // Unsupported entries pass through, and the summary counts every proposal.
    assert!(
        preview
            .unsupported
            .iter()
            .any(|entry| entry == "codex/ghost: neither command nor url")
    );
    assert_eq!(preview.summary.manage, 1);
    assert_eq!(preview.summary.merge, 1);
    assert_eq!(preview.summary.choose, 2);
    assert_eq!(
        preview.summary.unsupported,
        u64::try_from(preview.unsupported.len()).unwrap()
    );
    let text = serde_json::to_string(&preview).unwrap();
    assert!(!text.contains("SECRET"), "{text}");

    // No configuration at all is an empty preview, not an error.
    let empty = migration_preview(&discover_provider_configuration(
        &DiscoveryEnvironment::empty().with_config_home(tempdir().unwrap().path()),
    ));
    assert!(empty.candidates.is_empty());
    assert_eq!(
        empty.summary.manage + empty.summary.merge + empty.summary.choose,
        0
    );
}
