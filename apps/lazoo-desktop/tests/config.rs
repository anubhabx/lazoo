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
    assert_eq!(config["build"]["frontendDist"], "ui");
    assert_eq!(config["app"]["withGlobalTauri"], true);
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0]["label"], "main");
    assert_eq!(windows[0]["title"], "Lazoo — Local control room");
    assert_eq!(windows[0]["minWidth"], 860);
    assert_eq!(windows[0]["minHeight"], 620);
    assert_eq!(windows[0]["resizable"], true);
    assert_eq!(config["bundle"]["active"], false);
    assert!(csp.contains("default-src 'self'"));
    assert!(
        config["app"]["security"]
            .get("dangerousDisableAssetCspModification")
            .is_none()
    );
}

#[test]
fn main_capability_allows_only_core_and_snapshot_command() {
    let capability = read_json("capabilities/main.json");

    assert_eq!(capability["windows"], serde_json::json!(["main"]));
    assert_eq!(
        capability["permissions"],
        serde_json::json!(["core:default", "allow-load-desktop-snapshot"])
    );
}

#[test]
fn build_manifest_declares_only_snapshot_command() {
    let build_script = fs::read_to_string(manifest_path("build.rs")).expect("build.rs");

    assert!(build_script.contains("commands(&[\"load_desktop_snapshot\"])"));
    assert!(!build_script.contains("system.shutdown"));
    assert!(!build_script.contains("project.register"));
}

#[test]
fn failed_acceptance_cleanup_is_scoped_to_the_keeper_process_tree() {
    let script = fs::read_to_string(manifest_path("../../scripts/acceptance/desktop-shell.ps1"))
        .expect("desktop acceptance script");

    assert!(script.contains("function Stop-OwnedProcessTree"));
    assert!(script.contains("ParentProcessId"));
    assert!(!script.contains("ExecutablePath -eq $script:daemonExe"));
}
