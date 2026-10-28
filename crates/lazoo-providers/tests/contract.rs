use std::{ffi::OsString, sync::Mutex};

use lazoo_providers::{
    CommandSpec, DiscoverySource, ExecutableCandidate, ProviderCapabilities, ProviderError,
    ProviderEvent, ProviderKind, ProviderSession, ProviderVerification, ToolCall, ToolResult,
    TurnEventObserver, TurnRequest, attach_args, rank_candidates, redact_json,
};
use serde_json::json;

#[allow(dead_code)]
fn contained_host_pid_is_part_of_the_session_contract(session: &impl ProviderSession) -> u32 {
    session
        .native_host_pid()
        .expect("a live provider session must expose its contained host PID")
}

#[test]
fn attach_args_name_only_the_documented_resume_shapes_and_refuse_placeholders() {
    assert_eq!(
        attach_args(ProviderKind::ClaudeCode, "0b8e4c2a-session"),
        Some(vec!["--resume".to_owned(), "0b8e4c2a-session".to_owned()])
    );
    assert_eq!(
        attach_args(ProviderKind::Codex, "thread-7"),
        Some(vec!["resume".to_owned(), "thread-7".to_owned()])
    );
    for provider in [
        ProviderKind::OpenCode,
        ProviderKind::GrokBuild,
        ProviderKind::Antigravity,
    ] {
        assert_eq!(attach_args(provider, "session-1"), None, "{provider}");
    }
    for provider in ProviderKind::ALL {
        assert_eq!(attach_args(provider, ""), None, "{provider} empty id");
        assert_eq!(
            attach_args(provider, "pending:0b8e4c2a"),
            None,
            "{provider} placeholder id"
        );
    }
}

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
fn turn_permission_override_is_explicit_and_opt_in() {
    let ordinary = TurnRequest::new("hello");
    assert_eq!(ordinary.permission_mode, None);

    let restricted = ordinary.with_permission_mode("ask");
    assert_eq!(restricted.permission_mode.as_deref(), Some("ask"));
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

#[test]
fn command_spec_debug_keeps_environment_keys_but_redacts_values() {
    let mut command = CommandSpec::new(r"C:\Tools\codex.exe", r"C:\workspace");
    command.env.insert(
        OsString::from("CODEX_NATIVE_VIEW_TOKEN"),
        OsString::from("super-secret-bearer"),
    );

    let debug = format!("{command:?}");
    assert!(debug.contains("CODEX_NATIVE_VIEW_TOKEN"));
    assert!(!debug.contains("super-secret-bearer"));
    assert!(debug.contains("<redacted>"));
}

#[test]
fn normalized_events_use_camel_case_fields() {
    let event = ProviderEvent::SessionStarted {
        provider: ProviderKind::Codex,
        session_id: "thread-1".to_owned(),
        resumed: false,
        raw: None,
    };

    let value = serde_json::to_value(event).unwrap();
    assert_eq!(value["type"], "sessionStarted");
    assert_eq!(value["sessionId"], "thread-1");
    assert!(value.get("session_id").is_none());
}

struct FixtureSession;

impl ProviderSession for FixtureSession {
    fn id(&self) -> String {
        "fixture-session".to_owned()
    }

    fn native_host_pid(&self) -> Option<u32> {
        Some(42)
    }

    fn turn<'a>(
        &'a self,
        _request: &'a TurnRequest,
    ) -> lazoo_providers::AdapterFuture<'a, Vec<ProviderEvent>> {
        Box::pin(async {
            Ok(vec![
                ProviderEvent::TurnStarted {
                    provider: ProviderKind::Codex,
                    session_id: "fixture-session".to_owned(),
                    turn_id: Some("turn-1".to_owned()),
                    raw: None,
                },
                ProviderEvent::TurnCompleted {
                    provider: ProviderKind::Codex,
                    session_id: "fixture-session".to_owned(),
                    turn_id: Some("turn-1".to_owned()),
                    status: "completed".to_owned(),
                    output: Some("done".to_owned()),
                    raw: None,
                },
            ])
        })
    }

    fn cancel(&self) -> lazoo_providers::AdapterFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn stop(&self) -> lazoo_providers::AdapterFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct CollectingObserver {
    events: Mutex<Vec<ProviderEvent>>,
}

impl TurnEventObserver for CollectingObserver {
    fn observe(&self, event: &ProviderEvent) -> Result<(), ProviderError> {
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}

struct RejectingObserver;

impl TurnEventObserver for RejectingObserver {
    fn observe(&self, _event: &ProviderEvent) -> Result<(), ProviderError> {
        Err(ProviderError::Observer(
            "persistence rejected event".to_owned(),
        ))
    }
}

#[tokio::test]
async fn observed_default_path_delivers_normalized_events_in_order() {
    let session = FixtureSession;
    let request = TurnRequest::new("hello");
    let collector = CollectingObserver::default();

    let returned = session.turn_observed(&request, &collector).await.unwrap();
    let captured = collector.events.lock().unwrap().clone();

    assert_eq!(captured, returned);
    assert!(matches!(captured[0], ProviderEvent::TurnStarted { .. }));
    assert!(matches!(captured[1], ProviderEvent::TurnCompleted { .. }));
}

#[tokio::test]
async fn observed_default_path_stops_on_observer_error_without_changing_plain_turn() {
    let session = FixtureSession;
    let request = TurnRequest::new("hello");

    let error = session
        .turn_observed(&request, &RejectingObserver)
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProviderError::Observer(message) if message == "persistence rejected event")
    );

    let ordinary = session.turn(&request).await.unwrap();
    assert_eq!(ordinary.len(), 2);
}

#[tokio::test]
async fn observer_tool_calls_fail_closed_without_an_attached_control_plane() {
    let call = ToolCall {
        provider: ProviderKind::Codex,
        session_id: "thread-1".to_owned(),
        turn_id: Some("turn-1".to_owned()),
        call_id: "call-1".to_owned(),
        name: "task_delegate".to_owned(),
        arguments: json!({"objective": "anything"}),
    };
    let result = CollectingObserver::default()
        .call_tool(&call)
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.content["error"]["code"], "LZ-TOOL-UNATTACHED");
    let failure = ToolResult::failure("LZ-TOOL-UNKNOWN", "no such tool");
    assert!(!failure.success);
    assert_eq!(failure.content["error"]["message"], "no such tool");
    assert!(ToolResult::success(json!({"ok": true})).success);
}
