use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use lazoo_providers::{
    AdapterFuture, CopilotAdapter, DiscoveryEnvironment, PermissionDecision, PermissionRequest,
    ProviderAdapter, ProviderError, ProviderEvent, ProviderKind, ProviderSession,
    ProviderVerification, SessionRequest, TurnEventObserver, TurnRequest,
};
use tempfile::tempdir;

fn fake_acp() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/acp/fake-acp.cmd")
}

fn fake_acp_with_resume() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/acp/fake-acp-resume.cmd")
}

fn fake_acp_without_http_mcp() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/acp/fake-acp-no-http.cmd")
}

fn assert_lazoo_mcp(directory: &Path, name: &str) {
    let text = std::fs::read_to_string(directory.join(name)).unwrap();
    let value: serde_json::Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}').trim()).unwrap();
    let servers = value["mcpServers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["name"], "lazoo");
    assert!(
        servers[0]["headers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|header| {
                header["name"] == "Authorization"
                    && header["value"]
                        .as_str()
                        .is_some_and(|value| value.starts_with("Bearer "))
            })
    );
}

struct ApprovingObserver {
    requests: Mutex<Vec<PermissionRequest>>,
    approve: AtomicBool,
}
impl ApprovingObserver {
    fn new() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            approve: AtomicBool::new(true),
        }
    }
}
impl TurnEventObserver for ApprovingObserver {
    fn observe(&self, _: &ProviderEvent) -> Result<(), ProviderError> {
        Ok(())
    }
    fn decide_permission<'a>(
        &'a self,
        request: &'a PermissionRequest,
    ) -> AdapterFuture<'a, PermissionDecision> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request.clone());
            Ok(if self.approve.load(Ordering::SeqCst) {
                PermissionDecision::Approve
            } else {
                PermissionDecision::Decline
            })
        })
    }
}

#[test]
fn copilot_acp_launch_honors_selection_and_does_not_auto_approve() {
    let mut request = SessionRequest::new(r"C:\repo")
        .resume("session-1")
        .with_model("gpt-5.4")
        .with_effort("high");
    request
        .launch_environment
        .set("COPILOT_ALLOW_ALL", "untrusted-shell-value");
    let command = CopilotAdapter::command(Path::new(r"C:\Tools\copilot.exe"), &request);
    assert_eq!(command.executable, PathBuf::from(r"C:\Tools\copilot.exe"));
    assert!(
        command
            .args
            .windows(2)
            .any(|args| args == [OsString::from("--model"), OsString::from("gpt-5.4")])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|args| args == [OsString::from("--effort"), OsString::from("high")])
    );
    assert!(command.args.contains(&OsString::from("--acp")));
    assert!(command.args.contains(&OsString::from("--no-auto-update")));
    assert!(command.args.contains(&OsString::from("--no-remote-export")));
    assert!(
        command
            .env_remove
            .contains(&OsString::from("COPILOT_ALLOW_ALL"))
    );
    assert!(
        !command
            .env
            .contains_key(&OsString::from("COPILOT_ALLOW_ALL"))
    );
    assert!(
        !command
            .args
            .iter()
            .any(|arg| arg == "--allow-all" || arg == "--allow-all-tools")
    );
    assert!(
        !command
            .args
            .iter()
            .any(|arg| arg.to_string_lossy().starts_with("--resume")),
        "ACP uses session/load, not CLI resume"
    );
}

#[tokio::test]
async fn copilot_acp_fixture_governs_permission_and_resumes_without_a_model_probe() {
    let workspace = tempdir().unwrap();
    let adapter = CopilotAdapter::with_executable(fake_acp());
    let session = adapter
        .start(&SessionRequest::new(workspace.path()))
        .await
        .unwrap();
    assert_lazoo_mcp(workspace.path(), "fake-acp-session-new.json");
    let observer = ApprovingObserver::new();
    let events = session
        .turn_observed(&TurnRequest::new("EXECUTE"), &observer)
        .await
        .unwrap();
    assert_eq!(observer.requests.lock().unwrap().len(), 1);
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::PermissionRequested {
            provider: ProviderKind::GitHubCopilot,
            ..
        }
    )));
    session.cancel().await.unwrap();
    session.stop().await.unwrap();
    let resumable = CopilotAdapter::with_executable(fake_acp_with_resume());
    let first = resumable
        .start(&SessionRequest::new(workspace.path()))
        .await
        .unwrap();
    assert_lazoo_mcp(workspace.path(), "fake-acp-session-new.json");
    let id = first.id();
    first.stop().await.unwrap();
    let resumed = resumable
        .start(&SessionRequest::new(workspace.path()).resume(&id))
        .await
        .unwrap();
    assert_eq!(resumed.id(), id);
    assert_lazoo_mcp(workspace.path(), "fake-acp-session-load.json");
    resumed.stop().await.unwrap();
}

#[tokio::test]
async fn copilot_rejects_missing_http_mcp_before_session_new() {
    let workspace = tempdir().unwrap();
    let mut request = SessionRequest::new(workspace.path());
    request.launch_environment.set("FAKE_ACP_NO_HTTP_MCP", "1");
    let error = CopilotAdapter::with_executable(fake_acp())
        .start(&request)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ProviderError::Unsupported {
            provider: ProviderKind::GitHubCopilot,
            capability: "HTTP MCP servers",
            ..
        }
    ));
    assert!(!workspace.path().join("fake-acp-session-new.json").exists());
}

#[tokio::test]
async fn copilot_probe_degrades_when_acp_lacks_http_mcp() {
    let probe = CopilotAdapter::with_executable(fake_acp_without_http_mcp())
        .probe(&DiscoveryEnvironment::empty())
        .await
        .unwrap();
    assert_eq!(probe.verification, ProviderVerification::Degraded);
    assert!(!probe.capabilities.mcp);
    assert_eq!(
        probe.degradation_reason.as_deref(),
        Some("ACP initialize handshake did not advertise required HTTP MCP support")
    );
}

#[test]
fn copilot_discovery_accepts_direct_platform_binary_override() {
    let adapter = CopilotAdapter::with_executable(r"C:\Tools\copilot.exe");
    let candidates = adapter.discover(&DiscoveryEnvironment::empty());
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].path, PathBuf::from(r"C:\Tools\copilot.exe"));
}

#[tokio::test]
#[ignore = "requires the installed GitHub Copilot CLI; ACP initialization sends no model request"]
async fn live_copilot_acp_probe_does_not_send_a_model_request() {
    let probe = CopilotAdapter::default()
        .probe(&DiscoveryEnvironment::current())
        .await
        .unwrap();
    println!("{}", serde_json::to_string_pretty(&probe).unwrap());
    assert_eq!(probe.provider, ProviderKind::GitHubCopilot);
    assert!(
        probe
            .evidence
            .iter()
            .any(|entry| entry == "model execution was not attempted by this probe")
    );
}

#[tokio::test]
#[ignore = "explicit bounded live Copilot model acceptance using the owner's existing native login"]
async fn live_copilot_one_text_turn_with_all_tool_permissions_declined() {
    assert_eq!(
        std::env::var("LAZOO_LIVE_COPILOT_SMOKE").as_deref(),
        Ok("1")
    );
    let workspace = tempdir().unwrap();
    let mut request = SessionRequest::new(workspace.path());
    request.startup_timeout_millis = 30_000;
    let session = CopilotAdapter::default()
        .start(&request)
        .await
        .expect("start Copilot ACP session");
    let observer = ApprovingObserver::new();
    observer.approve.store(false, Ordering::SeqCst);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        session.turn_observed(
            &TurnRequest::new("Reply with exactly COPILOT_OK. Do not use any tools."),
            &observer,
        ),
    )
    .await;
    session.stop().await.expect("stop bounded Copilot smoke");
    let events = result
        .expect("Copilot smoke deadline")
        .expect("Copilot text response");
    let output = serde_json::to_string(&events).unwrap();
    assert!(
        output.contains("COPILOT_OK"),
        "native provider did not return the expected marker"
    );
    println!(
        "COPILOT_LIVE_MODEL_OK tools_requested={}",
        observer.requests.lock().unwrap().len()
    );
}
