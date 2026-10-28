use std::fs;

use lazoo_providers::{
    ConfigurationApplyError, ConfigurationApplyStatus, ConfigurationCompatibility,
    ConfigurationRecoveryState, DiscoveryEnvironment, ProviderKind, ProviderVersionValidation,
    SelectedMcpIntegration, apply_configuration_preview, apply_configuration_preview_with_intent,
    inspect_configuration_apply_recovery, preview_configuration_apply,
    rollback_configuration_apply,
};
use tempfile::tempdir;

fn setup(text: &str) -> (tempfile::TempDir, DiscoveryEnvironment, std::path::PathBuf) {
    let home = tempdir().unwrap();
    let codex = home.path().join(".codex");
    fs::create_dir_all(&codex).unwrap();
    let target = codex.join("config.toml");
    fs::write(&target, text).unwrap();
    let environment = DiscoveryEnvironment::empty().with_config_home(home.path());
    (home, environment, target)
}

fn selection(replace: bool) -> SelectedMcpIntegration {
    SelectedMcpIntegration {
        provider: ProviderKind::Codex,
        name: "search".into(),
        command: Some("npx".into()),
        args: vec!["-y".into(), "new-server".into()],
        url: None,
        enabled: true,
        replace,
        selected_secret_fields: vec!["API_TOKEN".into()],
    }
}

#[allow(clippy::unnecessary_wraps)] // The production API requires a validating callback.
fn validated(provider: ProviderKind) -> Option<ProviderVersionValidation> {
    Some(ProviderVersionValidation {
        provider,
        version: "0.155.1".into(),
        compatibility: ConfigurationCompatibility::Native,
    })
}

#[test]
fn preview_apply_and_rollback_preserve_unrelated_and_secret_file_values() {
    let (home, environment, target) = setup(
        "model = \"gpt\"\n[mcp_servers.search]\ncommand = \"old\"\nargs = [\"x\"]\nenv = { API_TOKEN = \"secret-stays-in-file\" }\n[mcp_servers.other]\nurl = \"https://other.example/mcp\"\n",
    );
    let backup = home.path().join("runtime-config");
    let preview =
        preview_configuration_apply(&environment, &backup, selection(true), validated).unwrap();
    let public = serde_json::to_string(preview.metadata()).unwrap();
    assert!(!public.contains("secret-stays-in-file"));
    assert_eq!(preview.metadata().changed_entries, ["search"]);
    let receipt = apply_configuration_preview(preview, |_| Ok(())).unwrap();
    let changed = fs::read_to_string(&target).unwrap();
    assert!(changed.contains("model = \"gpt\""));
    assert!(changed.contains("secret-stays-in-file"));
    assert!(changed.contains("new-server"));
    let rolled = rollback_configuration_apply(&environment, &backup, &receipt, |_| Ok(())).unwrap();
    assert_eq!(rolled.status, ConfigurationApplyStatus::RolledBack);
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        "model = \"gpt\"\n[mcp_servers.search]\ncommand = \"old\"\nargs = [\"x\"]\nenv = { API_TOKEN = \"secret-stays-in-file\" }\n[mcp_servers.other]\nurl = \"https://other.example/mcp\"\n"
    );
}

#[test]
fn conflicts_malformed_versions_and_secret_selection_fail_before_writing() {
    let (home, environment, target) =
        setup("[mcp_servers.search]\ncommand = \"old\"\nenv = { API_TOKEN = \"value\" }\n");
    let backup = home.path().join("runtime");
    assert!(matches!(
        preview_configuration_apply(&environment, &backup, selection(false), validated),
        Err(ConfigurationApplyError::NameConflict)
    ));
    let mut wrong_secret = selection(true);
    wrong_secret.selected_secret_fields.clear();
    assert!(matches!(
        preview_configuration_apply(&environment, &backup, wrong_secret, validated),
        Err(ConfigurationApplyError::SecretFieldSelectionRequired)
    ));
    assert!(matches!(
        preview_configuration_apply(&environment, &backup, selection(true), |p| Some(
            ProviderVersionValidation {
                provider: p,
                version: "old".into(),
                compatibility: ConfigurationCompatibility::Unsupported
            }
        )),
        Err(ConfigurationApplyError::UnsupportedVersion)
    ));
    fs::write(&target, "[mcp_servers.search\nAPI_TOKEN = \"unquoted\"").unwrap();
    assert!(matches!(
        preview_configuration_apply(&environment, &backup, selection(true), validated),
        Err(ConfigurationApplyError::MalformedConfiguration)
    ));
}

#[test]
fn stale_preview_and_reprobe_failure_do_not_overwrite_concurrent_or_invalid_state() {
    let (home, environment, target) =
        setup("[mcp_servers.search]\ncommand = \"old\"\nenv = { API_TOKEN = \"value\" }\n");
    let backup = home.path().join("runtime");
    let preview =
        preview_configuration_apply(&environment, &backup, selection(true), validated).unwrap();
    fs::write(&target, "user-change").unwrap();
    assert!(matches!(
        apply_configuration_preview(preview, |_| Ok(())),
        Err(ConfigurationApplyError::StalePreview)
    ));

    fs::write(
        &target,
        "[mcp_servers.search]\ncommand = \"old\"\nenv = { API_TOKEN = \"value\" }\n",
    )
    .unwrap();
    let preview =
        preview_configuration_apply(&environment, &backup, selection(true), validated).unwrap();
    let error = apply_configuration_preview(preview, |path| {
        fs::write(path, "concurrent-change").unwrap();
        Err(ConfigurationApplyError::MalformedConfiguration)
    });
    assert!(matches!(
        error,
        Err(ConfigurationApplyError::ConcurrentDrift)
    ));
    assert_eq!(fs::read_to_string(target).unwrap(), "concurrent-change");
}

#[test]
fn failed_reprobe_restores_only_the_written_candidate_and_rollback_rejects_stale_or_tampered_data()
{
    let original = "[mcp_servers.search]\ncommand = \"old\"\nenv = { API_TOKEN = \"value\" }\n";
    let (home, environment, target) = setup(original);
    let backup = home.path().join("runtime");
    let preview =
        preview_configuration_apply(&environment, &backup, selection(true), validated).unwrap();
    assert!(matches!(
        apply_configuration_preview(preview, |_| Err(
            ConfigurationApplyError::MalformedConfiguration
        )),
        Err(ConfigurationApplyError::MalformedConfiguration)
    ));
    assert_eq!(fs::read_to_string(&target).unwrap(), original);

    let preview =
        preview_configuration_apply(&environment, &backup, selection(true), validated).unwrap();
    let receipt = apply_configuration_preview(preview, |_| Ok(())).unwrap();
    fs::write(&target, "user-change").unwrap();
    assert!(matches!(
        rollback_configuration_apply(&environment, &backup, &receipt, |_| Ok(())),
        Err(ConfigurationApplyError::StalePreview)
    ));

    fs::write(&target, original).unwrap();
    let preview =
        preview_configuration_apply(&environment, &backup, selection(true), validated).unwrap();
    let receipt = apply_configuration_preview(preview, |_| Ok(())).unwrap();
    fs::write(backup.join(&receipt.backup_name), b"tampered").unwrap();
    assert!(matches!(
        rollback_configuration_apply(&environment, &backup, &receipt, |_| Ok(())),
        Err(ConfigurationApplyError::BackupTampered)
    ));
}

#[test]
fn reparse_or_symlink_targets_are_refused() {
    let (home, environment, target) = setup("[mcp_servers.search]\ncommand = \"old\"\n");
    let outside = home.path().join("outside.toml");
    fs::write(&outside, "safe = true").unwrap();
    fs::remove_file(&target).unwrap();
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_file(&outside, &target).is_ok() {
            assert!(matches!(
                preview_configuration_apply(
                    &environment,
                    home.path().join("runtime"),
                    selection(true),
                    validated
                ),
                Err(ConfigurationApplyError::UnsafeTarget)
            ));
        }
    }
}

#[test]
fn missing_known_provider_file_is_created_at_its_derived_target() {
    let home = tempdir().unwrap();
    let environment = DiscoveryEnvironment::empty().with_config_home(home.path());
    let target = home.path().join(".codex").join("config.toml");
    let preview = preview_configuration_apply(
        &environment,
        home.path().join("runtime"),
        selection(false),
        validated,
    )
    .unwrap();
    assert_eq!(preview.metadata().target.file_name(), target.file_name());
    apply_configuration_preview(preview, |_| Ok(())).unwrap();
    assert!(target.is_file());
}

#[test]
fn translated_versions_and_remote_arguments_are_refused() {
    let (home, environment, _) = setup("[mcp_servers.search]\ncommand = \"old\"\n");
    let backup = home.path().join("runtime");
    assert!(matches!(
        preview_configuration_apply(&environment, &backup, selection(true), |provider| Some(
            ProviderVersionValidation {
                provider,
                version: "test".into(),
                compatibility: ConfigurationCompatibility::Translated,
            }
        )),
        Err(ConfigurationApplyError::UnsupportedVersion)
    ));
    let mut remote = selection(true);
    remote.command = None;
    remote.url = Some("https://mcp.example.test".into());
    remote.args = vec!["must-not-be-rendered".into()];
    assert!(matches!(
        preview_configuration_apply(&environment, &backup, remote, validated),
        Err(ConfigurationApplyError::InvalidSelection)
    ));
}

#[test]
fn prepared_receipt_precedes_atomic_replace_and_recovery_only_inspects() {
    let original = "[mcp_servers.search]\ncommand = \"old\"\nenv = { API_TOKEN = \"value\" }\n";
    let (home, environment, target) = setup(original);
    let backup = home.path().join("runtime");
    let preview =
        preview_configuration_apply(&environment, &backup, selection(true), validated).unwrap();
    let receipt = apply_configuration_preview_with_intent(
        preview,
        |receipt| {
            // The native target remains untouched while the durable intent is recorded.
            assert_eq!(fs::read_to_string(&target).unwrap(), original);
            assert_eq!(receipt.status, ConfigurationApplyStatus::Applied);
            Ok(())
        },
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(
        inspect_configuration_apply_recovery(&environment, &backup, &receipt).unwrap(),
        ConfigurationRecoveryState::AppliedPresent
    );
    fs::write(&target, original).unwrap();
    assert_eq!(
        inspect_configuration_apply_recovery(&environment, &backup, &receipt).unwrap(),
        ConfigurationRecoveryState::OriginalPresent
    );
    fs::write(&target, "unrelated writer").unwrap();
    assert_eq!(
        inspect_configuration_apply_recovery(&environment, &backup, &receipt).unwrap(),
        ConfigurationRecoveryState::DriftRefused
    );
}

#[test]
fn rollback_rejects_substituted_receipts_and_reparse_backup_roots() {
    let (home, environment, _) =
        setup("[mcp_servers.search]\ncommand = \"old\"\nenv = { API_TOKEN = \"value\" }\n");
    let backup = home.path().join("runtime");
    let preview =
        preview_configuration_apply(&environment, &backup, selection(true), validated).unwrap();
    let receipt = apply_configuration_preview(preview, |_| Ok(())).unwrap();
    let mut substituted = receipt.clone();
    substituted.revision = "different-revision".into();
    assert!(matches!(
        rollback_configuration_apply(&environment, &backup, &substituted, |_| Ok(())),
        Err(ConfigurationApplyError::InvalidBackupReceipt)
    ));

    #[cfg(windows)]
    {
        let outside = home.path().join("outside-runtime");
        fs::create_dir_all(&outside).unwrap();
        let linked = home.path().join("linked-runtime");
        if std::os::windows::fs::symlink_dir(&outside, &linked).is_ok() {
            let preview =
                preview_configuration_apply(&environment, &linked, selection(true), validated)
                    .unwrap();
            assert!(matches!(
                apply_configuration_preview(preview, |_| Ok(())),
                Err(ConfigurationApplyError::UnsafeTarget)
            ));
        }
    }
}

#[cfg(windows)]
#[test]
fn exclusive_open_refuses_a_target_locked_by_another_handle() {
    use std::os::windows::fs::OpenOptionsExt;

    let (home, environment, target) =
        setup("[mcp_servers.search]\ncommand = \"old\"\nenv = { API_TOKEN = \"value\" }\n");
    let preview = preview_configuration_apply(
        &environment,
        home.path().join("runtime"),
        selection(true),
        validated,
    )
    .unwrap();
    let _lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(target)
        .unwrap();
    assert!(matches!(
        apply_configuration_preview(preview, |_| Ok(())),
        Err(ConfigurationApplyError::Io(_))
    ));
}
