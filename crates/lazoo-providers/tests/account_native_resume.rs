use std::{ffi::OsString, path::Path};

use lazoo_providers::{
    CommandSpec, ProviderKind, prepare_native_resume_command, prepare_native_resume_terminal,
};
use lazoo_runtime::{LaunchEnvironment, TerminalCommand};

#[test]
fn codex_native_resume_uses_the_selected_key_without_exposing_it_in_arguments() {
    let mut environment = LaunchEnvironment::default();
    environment.set("OPENAI_API_KEY", "resume-secret");
    let command = prepare_native_resume_command(
        ProviderKind::Codex,
        CommandSpec::new("codex", Path::new(r"C:\repo"))
            .arg("resume")
            .arg("thread-1"),
        &environment,
    )
    .unwrap();
    let args = command
        .args
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(args.contains(&r#"model_providers.lazoo-account.name="openai""#.to_owned()));
    assert!(!args.iter().any(|value| value.contains("resume-secret")));
    assert_eq!(
        command.env[&OsString::from("OPENAI_API_KEY")],
        OsString::from("resume-secret")
    );

    let terminal = prepare_native_resume_terminal(
        ProviderKind::Codex,
        TerminalCommand::new("codex", r"C:\repo", 80, 24)
            .arg("resume")
            .arg("thread-1"),
        &environment,
    )
    .unwrap();
    assert!(!format!("{terminal:?}").contains("resume-secret"));
}

#[test]
fn opencode_native_resume_converts_only_the_private_provider_marker() {
    let mut environment = LaunchEnvironment::default();
    environment.set("LAZOO_OPENCODE_PROVIDER_CONFIG", r#"{"openai":{"options":{"baseURL":"https://gateway.example/v1","apiKey":"{env:OPENAI_API_KEY}"}}}"#);
    environment.set("OPENCODE_CONFIG_CONTENT", r#"{"permission":{"*":"allow"}}"#);
    let command = prepare_native_resume_command(
        ProviderKind::OpenCode,
        CommandSpec::new("opencode", Path::new(r"C:\repo")).arg("--resume=session-1"),
        &environment,
    )
    .unwrap();
    let config = command.env[&OsString::from("OPENCODE_CONFIG_CONTENT")].to_string_lossy();
    let config: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert_eq!(
        config["provider"]["openai"]["options"]["baseURL"],
        "https://gateway.example/v1"
    );
    assert_eq!(config["enabled_providers"], serde_json::json!(["openai"]));
    assert!(config.get("permission").is_none());
    assert!(
        !command
            .env
            .contains_key(&OsString::from("LAZOO_OPENCODE_PROVIDER_CONFIG"))
    );
    for name in [
        "OPENCODE_CONFIG",
        "OPENCODE_CONFIG_CONTENT",
        "OPENCODE_CONFIG_DIR",
        "LAZOO_OPENCODE_PROVIDER_CONFIG",
    ] {
        assert!(command.env_remove.contains(&OsString::from(name)));
    }
}
