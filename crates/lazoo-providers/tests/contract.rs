use lazoo_providers::{
    DiscoverySource, ExecutableCandidate, ProviderCapabilities, ProviderKind, ProviderVerification,
    rank_candidates, redact_json,
};
use serde_json::json;

#[test]
fn provider_kinds_are_stable_and_parseable() {
    let cases = [
        (ProviderKind::Codex, "codex"),
        (ProviderKind::OpenCode, "opencode"),
        (ProviderKind::GrokBuild, "grok-build"),
        (ProviderKind::Antigravity, "antigravity"),
        (ProviderKind::ClaudeCode, "claude-code"),
    ];

    for (kind, expected) in cases {
        assert_eq!(kind.as_str(), expected);
        assert_eq!(expected.parse::<ProviderKind>().unwrap(), kind);
        assert_eq!(
            serde_json::to_string(&kind).unwrap(),
            format!(r#""{expected}""#)
        );
    }
}

#[test]
fn capabilities_default_to_false_and_states_are_truthful() {
    let capabilities = ProviderCapabilities::default();

    assert!(!capabilities.session);
    assert!(!capabilities.stream);
    assert!(!capabilities.cancel);
    assert!(!capabilities.resume);
    assert!(!capabilities.terminal);
    assert!(!capabilities.permission);
    assert!(!capabilities.mcp);
    assert!(!capabilities.plugins);
    assert!(!capabilities.skills);
    assert!(!capabilities.connectors);
    assert!(!capabilities.native_message);
    assert!(!capabilities.model_selection);
    assert_eq!(
        serde_json::to_string(&ProviderVerification::NotDetected).unwrap(),
        r#""not-detected""#,
    );
    assert_eq!(
        serde_json::to_string(&ProviderVerification::Unverified).unwrap(),
        r#""unverified""#,
    );
}

#[test]
fn discovery_deduplicates_windows_paths_and_keeps_source_priority() {
    let candidates = rank_candidates(vec![
        ExecutableCandidate::new(r"C:\Tools\codex.exe", DiscoverySource::KnownLocation),
        ExecutableCandidate::new(r"c:/tools/CODEX.exe", DiscoverySource::InheritedPath),
    ]);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].source, DiscoverySource::InheritedPath);
}

#[test]
fn redaction_removes_nested_secret_like_values() {
    let value = json!({
        "token": "secret-token",
        "nested": {
            "apiKey": "secret-key",
            "authorization": "Bearer secret",
            "message": "safe"
        },
        "array": [{"password": "hunter2"}]
    });

    assert_eq!(
        redact_json(value),
        json!({
            "token": "[REDACTED]",
            "nested": {
                "apiKey": "[REDACTED]",
                "authorization": "[REDACTED]",
                "message": "safe"
            },
            "array": [{"password": "[REDACTED]"}]
        }),
    );
}
