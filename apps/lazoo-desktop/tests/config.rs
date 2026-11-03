use std::{fs, path::PathBuf};

use serde_json::Value;

fn manifest_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read_json(relative: &str) -> Value {
    let path = manifest_path(relative);
    let bytes = fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("{} is invalid JSON: {error}", path.display()))
}

#[test]
fn tauri_configuration_is_static_local_and_single_windowed() {
    let config = read_json("tauri.conf.json");
    let windows = config["app"]["windows"].as_array().expect("windows array");
    let csp = config["app"]["security"]["csp"]
        .as_str()
        .expect("CSP string");

    assert_eq!(config["productName"], "Lazoo");
    assert_eq!(config["identifier"], "com.anubhab.lazoo");
    assert_eq!(config["build"]["frontendDist"], "ui/dist");
    assert_eq!(
        config["build"]["beforeDevCommand"],
        "npm --prefix ui run dev"
    );
    assert_eq!(
        config["build"]["beforeBuildCommand"],
        "npm --prefix ui run build"
    );
    assert_eq!(config["build"]["devUrl"], "http://127.0.0.1:1420");
    assert_eq!(config["app"]["withGlobalTauri"], false);
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0]["label"], "main");
    assert_eq!(windows[0]["title"], "Lazoo — Local control room");
    assert_eq!(windows[0]["minWidth"], 860);
    assert_eq!(windows[0]["minHeight"], 620);
    assert_eq!(windows[0]["resizable"], true);
    assert_eq!(windows[0]["decorations"], false);
    assert_eq!(windows[0]["theme"], "Dark");
    assert_eq!(config["bundle"]["active"], false);
    assert!(csp.contains("default-src 'self'"));
    assert!(
        config["app"]["security"]
            .get("dangerousDisableAssetCspModification")
            .is_none()
    );
}

#[test]
fn main_capability_allows_only_core_window_and_desktop_bridge_commands() {
    let capability = read_json("capabilities/main.json");

    assert_eq!(capability["windows"], serde_json::json!(["main"]));
    assert_eq!(
        capability["permissions"],
        serde_json::json!([
            "core:default",
            "core:window:default",
            "core:window:allow-close",
            "core:window:allow-minimize",
            "core:window:allow-toggle-maximize",
            "core:window:allow-start-dragging",
            "core:window:allow-is-maximized",
            "core:window:allow-set-focus",
            "allow-daemon-call",
            "allow-events-subscribe",
            "allow-events-unsubscribe",
            "allow-terminal-stream-start",
            "allow-terminal-stream-stop",
            "allow-terminal-stream-focus",
            "allow-terminal-write",
            "allow-voice-state",
            "allow-voice-settings-set",
            "allow-voice-key-set",
            "allow-voice-key-clear",
            "allow-voice-whisper-install",
            "allow-voice-whisper-remove",
            "allow-voice-transcribe",
            "allow-voice-speak",
            "allow-voice-insert-text",
            "allow-voice-copy-text",
            "allow-voice-overlay",
            "allow-voice-open-microphone-settings"
        ])
    );
}

#[test]
fn voice_overlay_capability_only_listens_and_media_is_allowed_by_csp() {
    let capability = read_json("capabilities/voice-overlay.json");
    assert_eq!(capability["windows"], serde_json::json!(["voice-overlay"]));
    assert_eq!(
        capability["permissions"],
        serde_json::json!(["core:event:allow-listen", "core:event:allow-unlisten"])
    );

    let config = read_json("tauri.conf.json");
    for key in ["csp", "devCsp"] {
        let csp = config["app"]["security"][key].as_str().expect("CSP string");
        assert!(csp.contains("media-src 'self' blob:"), "{key}");
    }
}

#[test]
fn build_manifest_declares_only_the_desktop_bridge_commands() {
    let build_script = fs::read_to_string(manifest_path("build.rs")).expect("build.rs");
    let commands = [
        "daemon_call",
        "events_subscribe",
        "events_unsubscribe",
        "terminal_stream_start",
        "terminal_stream_stop",
        "terminal_stream_focus",
        "terminal_write",
        "voice_state",
        "voice_settings_set",
        "voice_key_set",
        "voice_key_clear",
        "voice_whisper_install",
        "voice_whisper_remove",
        "voice_transcribe",
        "voice_speak",
        "voice_insert_text",
        "voice_copy_text",
        "voice_overlay",
    ];
    let declared = commands
        .iter()
        .map(|command| format!("\"{command}\","))
        .collect::<Vec<_>>();

    for entry in &declared {
        assert!(build_script.contains(entry.as_str()), "{entry}");
    }
    assert!(!build_script.contains("load_desktop_snapshot"));
    assert!(!build_script.contains("system.shutdown"));
    assert!(!build_script.contains("project.register"));
}

#[test]
fn failed_acceptance_cleanup_is_scoped_to_the_keeper_process_tree() {
    let script = fs::read_to_string(manifest_path("../../scripts/acceptance/desktop-shell.ps1"))
        .expect("desktop acceptance script");

    assert!(script.contains("function Stop-OwnedProcessTree"));
    assert!(script.contains("function Stop-OwnedProcess"));
    assert!(script.contains("ParentProcessId"));
    assert!(script.contains("Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue"));
    assert!(script.contains("cleanup tolerates root exiting before stop request"));
    assert!(script.contains("Stop-OwnedProcess $desktop"));
    assert!(!script.contains("Stop-Process -Id $desktop.Id -Force"));
    assert!(!script.contains("Stop-Process -Id $cleanupProbeOther.Id -Force"));
    assert!(!script.contains("Stop-Process -Id $cleanupExitProbe.Id -Force"));
    assert!(!script.contains("ExecutablePath -eq $script:daemonExe"));
}

#[test]
fn production_binary_enables_tauri_custom_protocol_by_default() {
    let cargo_manifest =
        fs::read_to_string(manifest_path("Cargo.toml")).expect("desktop Cargo.toml");

    assert!(cargo_manifest.contains("default = [\"custom-protocol\"]"));
    assert!(cargo_manifest.contains("custom-protocol = [\"tauri/custom-protocol\"]"));
}

#[test]
fn desktop_acceptance_covers_frontend_assets_and_native_interactions() {
    let script = fs::read_to_string(manifest_path("../../scripts/acceptance/desktop-shell.ps1"))
        .expect("desktop acceptance script");

    assert!(script.contains("npm --prefix apps/lazoo-desktop/ui ci"));
    assert!(script.contains("npm --prefix apps/lazoo-desktop/ui run build"));
    assert!(script.contains("function Test-DesktopRuntimeAssets"));
    assert!(script.contains("inline script content"));
    assert!(script.contains("remote runtime asset"));
    assert!(script.contains("Open command palette"));
    assert!(script.contains("Minimize window"));
    assert!(script.contains("Maximize or restore window"));
    assert!(script.contains("Close window"));
    assert!(script.contains("function Send-WindowKeys"));
    assert!(script.contains("'^k'"));
    assert!(script.contains("Search commands"));
    assert!(script.contains("'inbox{ENTER}'"));
    assert!(script.contains("'^n'"));
    assert!(script.contains("What should the team work on?"));
    assert!(script.contains("Collapse Desktop acceptance"));
    assert!(script.contains("Set up your team"));
    assert!(script.contains("Nothing needs you"));
    assert!(script.contains("Register a project"));
    assert!(script.contains("$Configuration"));
    assert!(script.contains("'{ESC}'"));
    assert!(script.contains("function Invoke-AutomationElement"));
    assert!(script.contains("1180, 760"));
    assert!(script.contains("860, 620"));
    assert!(script.contains("highDpiScreenshot"));
    assert!(script.contains("commandPaletteScreenshot"));
    assert!(script.contains("settingsScreenshot"));
    assert!(script.contains("TextScaleFactor"));
    assert!(script.contains("BLOCKED: Windows text scaling"));
}
