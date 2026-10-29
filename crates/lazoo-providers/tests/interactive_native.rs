//! Opt-in subscription acceptance for native Claude and Antigravity TUIs.
//!
//! This test sends at most two tiny prompts.  It is not part of normal CI and will not start
//! Claude unless the operator explicitly sets `LAZOO_LIVE_CLAUDE_INTERACTIVE=1`.
//!
//! Before running, build the companion hook binary and point the adapter at it:
//! ```powershell
//! cargo build -p lazooctl
//! $env:LAZOO_NATIVE_HOOK_HELPER = (Resolve-Path target\\debug\\lazooctl.exe)
//! $env:LAZOO_LIVE_CLAUDE_INTERACTIVE = '1'
//! # Optional: permit the one harmless `echo` in turn two; omitted means decline it.
//! $env:LAZOO_LIVE_CLAUDE_APPROVE_COMMAND = '1'
//! cargo test -p lazoo-providers --test interactive_native -- --ignored --nocapture
//! ```
//!
//! The harness never prints provider text, native transcript contents, settings, or tokens.

use std::{
    env,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use lazoo_providers::{
    AdapterFuture, NativeViewLaunchDescriptor, PermissionDecision, PermissionRequest,
    ProviderError, ProviderEvent, ProviderKind, ProviderRegistry, ProviderSession, SessionRequest,
    ToolCall, ToolResult, ToolSpec, TurnEventObserver, TurnRequest,
};
use serde_json::json;

const LIVE_GATE: &str = "LAZOO_LIVE_CLAUDE_INTERACTIVE";
const HELPER_ENV: &str = "LAZOO_NATIVE_HOOK_HELPER";
// This is deliberately absent from the prompt below, so it cannot be a literal prompt echo.
const REPLY_TOKEN: &str = "NATIVE_theater_7";
const COMMAND_TOKEN: &str = "LAZOO_CLAUDE_GOVERNED_COMMAND_20261029";
const AGY_COMMAND_TOKEN: &str = "LAZOO_AGY_GOVERNED_COMMAND_20261029";

#[derive(Default)]
struct NativeObserver {
    events: Mutex<Vec<ProviderEvent>>,
    permissions: Mutex<Vec<(PermissionRequest, PermissionDecision)>>,
    calls: Mutex<Vec<ToolCall>>,
    approved_commands: AtomicUsize,
}

impl NativeObserver {
    fn text(&self) -> String {
        self.events
            .lock()
            .expect("observer events lock")
            .iter()
            .filter_map(|event| match event {
                ProviderEvent::TextDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

impl TurnEventObserver for NativeObserver {
    fn observe(&self, event: &ProviderEvent) -> Result<(), ProviderError> {
        if let ProviderEvent::ToolUpdate {
            tool_name, status, ..
        } = event
        {
            eprintln!("native-tool={tool_name:?} status={status}");
        }
        eprintln!(
            "native-event={}",
            match event {
                ProviderEvent::SessionStarted { .. } => "session-started",
                ProviderEvent::TurnStarted { .. } => "turn-started",
                ProviderEvent::TextDelta { .. } => "text",
                ProviderEvent::TurnCompleted { .. } => "completed",
                _ => "progress",
            }
        );
        self.events
            .lock()
            .expect("observer events lock")
            .push(event.clone());
        Ok(())
    }

    fn decide_permission<'a>(
        &'a self,
        request: &'a PermissionRequest,
    ) -> AdapterFuture<'a, PermissionDecision> {
        Box::pin(async move {
            let safe_echo = request
                .detail
                .pointer("/tool_input/command")
                .and_then(serde_json::Value::as_str)
                == Some(format!("echo {COMMAND_TOKEN}").as_str());
            let safe_agy_echo = request
                .detail
                .pointer("/toolCall/args/CommandLine")
                .and_then(serde_json::Value::as_str)
                == Some(format!("Write-Output {AGY_COMMAND_TOKEN}").as_str());
            let allow = (env::var("LAZOO_LIVE_CLAUDE_APPROVE_COMMAND").as_deref() == Ok("1")
                && safe_echo)
                || (env::var("LAZOO_LIVE_ANTIGRAVITY_APPROVE_COMMAND").as_deref() == Ok("1")
                    && safe_agy_echo);
            let decision = if allow {
                self.approved_commands.fetch_add(1, Ordering::SeqCst);
                PermissionDecision::Approve
            } else {
                PermissionDecision::Decline
            };
            self.permissions
                .lock()
                .expect("observer permission lock")
                .push((request.clone(), decision));
            Ok(decision)
        })
    }

    fn call_tool<'a>(&'a self, call: &'a ToolCall) -> AdapterFuture<'a, ToolResult> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("observer tool lock")
                .push(call.clone());
            if call.name == "lazoo_ping" && call.arguments == json!({}) {
                Ok(ToolResult::success(json!({"status":"pong"})))
            } else {
                Ok(ToolResult::failure(
                    "LZ-LIVE-TOOL-REJECTED",
                    "only the bounded lazoo_ping acceptance tool is available",
                ))
            }
        })
    }
}

fn require_live_opt_in() -> PathBuf {
    assert_eq!(
        env::var(LIVE_GATE).as_deref(),
        Ok("1"),
        "set {LIVE_GATE}=1 before this ignored subscription-backed acceptance test"
    );
    let helper = env::var_os(HELPER_ENV)
        .map(PathBuf::from)
        .expect("set LAZOO_NATIVE_HOOK_HELPER to the built lazooctl companion binary");
    assert!(
        helper.is_file(),
        "LAZOO_NATIVE_HOOK_HELPER must point to an existing lazooctl binary: {}",
        helper.display()
    );
    helper
}

fn bounded_tool() -> ToolSpec {
    ToolSpec {
        name: "lazoo_ping".to_owned(),
        description: "Return the fixed acceptance response for the current Lazoo native-turn test."
            .to_owned(),
        input_schema: json!({"type":"object","additionalProperties":false}),
    }
}

fn completed(events: &[ProviderEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, ProviderEvent::TurnCompleted { .. }))
}

fn borrowed_terminal(lease: &lazoo_providers::NativeViewLease) -> lazoo_runtime::TerminalView {
    match lease.launch_descriptor() {
        NativeViewLaunchDescriptor::ExistingTerminal { terminal, .. } => terminal,
        NativeViewLaunchDescriptor::Remote { .. } => {
            panic!("interactive Claude must expose its canonical terminal, not a remote controller")
        }
    }
}

fn rendered_contains(output: &[u8], expected: &str) -> bool {
    String::from_utf8_lossy(output).contains(expected)
}

async fn assert_native_history_resume(
    provider: ProviderKind,
    request: &SessionRequest,
    native_id: &str,
) {
    let resumed = ProviderRegistry::default()
        .start(provider, &request.clone().resume(native_id))
        .await
        .expect("resume the same owned native conversation without a new prompt");
    assert_eq!(resumed.id(), native_id);
    let view = resumed.open_native_view().await.unwrap().unwrap();
    let terminal = borrowed_terminal(&view);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let retained = loop {
        let output = terminal.read_output(0, 64 * 1024).unwrap();
        if rendered_contains(&output.bytes, REPLY_TOKEN) {
            break true;
        }
        if std::time::Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    view.close();
    resumed.stop().await.unwrap();
    assert!(
        retained,
        "resumed native TUI did not retain its assistant history"
    );
}

#[tokio::test]
#[ignore = "explicit live Antigravity opt-in; two tiny native TUI turns"]
async fn antigravity_native_tui_two_turn_acceptance() {
    assert_eq!(
        env::var("LAZOO_LIVE_ANTIGRAVITY_INTERACTIVE").as_deref(),
        Ok("1")
    );
    assert!(
        env::var_os(HELPER_ENV)
            .map(PathBuf::from)
            .is_some_and(|path| path.is_file())
    );
    let workspace = tempfile::tempdir().unwrap();
    let mut request = SessionRequest::new(workspace.path())
        .with_managed_workspace()
        .with_native_workspace_trust_approved()
        .with_tools(vec![bounded_tool()]);
    request.startup_timeout_millis = 30_000;
    request.turn_timeout_millis = 60_000;
    let session = ProviderRegistry::default()
        .start(ProviderKind::Antigravity, &request)
        .await
        .unwrap();
    let view = session.open_native_view().await.unwrap().unwrap();
    let terminal = borrowed_terminal(&view);
    let observer = NativeObserver::default();
    let first = session.turn_observed(&TurnRequest::new("The word theater has seven letters. Reply with the marker formed as NATIVE_, then the word, then an underscore and its count. Do not use tools."), &observer).await;
    if first.is_err() {
        eprintln!(
            "AGY_FAILED_NATIVE_SCREEN={}",
            String::from_utf8_lossy(&terminal.read_output(0, 64 * 1024).unwrap().bytes)
        );
    }
    let first = first.unwrap();
    assert!(completed(&first));
    assert!(observer.text().contains(REPLY_TOKEN));
    assert!(!session.id().starts_with("pending:"));
    assert!(rendered_contains(
        &terminal.read_output(0, 64 * 1024).unwrap().bytes,
        REPLY_TOKEN
    ));
    let native_id = session.id();
    view.close();
    assert!(terminal.exit_status().unwrap().live);
    let reopened = session.open_native_view().await.unwrap().unwrap();
    let observer = NativeObserver::default();
    let second = session.turn_observed(&TurnRequest::new(format!("Call the lazoo server's lazoo_ping tool exactly once with an empty object, then use run_command to execute exactly `Write-Output {AGY_COMMAND_TOKEN}`. Report the returned status briefly. Do no other work.")), &observer).await;
    if second.is_err() {
        eprintln!(
            "AGY_FAILED_NATIVE_SCREEN={}",
            String::from_utf8_lossy(&terminal.read_output(0, 64 * 1024).unwrap().bytes)
        );
    }
    eprintln!(
        "native-id={} native-mcp-calls={} native-permissions={}",
        session.id(),
        observer.calls.lock().unwrap().len(),
        observer.permissions.lock().unwrap().len()
    );
    reopened.close();
    session.stop().await.unwrap();
    assert!(completed(&second.unwrap()));
    assert_eq!(session.id(), native_id);
    assert_eq!(observer.calls.lock().unwrap().len(), 1);
    assert_eq!(observer.calls.lock().unwrap()[0].name, "lazoo_ping");
    let approved = env::var("LAZOO_LIVE_ANTIGRAVITY_APPROVE_COMMAND").as_deref() == Ok("1");
    assert_eq!(
        observer.approved_commands.load(Ordering::SeqCst),
        usize::from(approved)
    );
    assert_native_history_resume(ProviderKind::Antigravity, &request, &native_id).await;
}

#[tokio::test]
#[ignore = "explicit live Claude opt-in; one bounded MCP and harmless shell turn"]
async fn claude_native_mcp_and_permission_acceptance() {
    let _helper = require_live_opt_in();
    let parent =
        env::var_os("LAZOO_LIVE_NATIVE_WORKSPACE_PARENT").expect("explicit trusted probe parent");
    let workspace = tempfile::tempdir_in(parent).unwrap();
    let mut request = SessionRequest::new(workspace.path())
        .with_managed_workspace()
        .with_native_workspace_trust_approved()
        .with_tools(vec![bounded_tool()]);
    request.turn_timeout_millis = 60_000;
    let session = ProviderRegistry::default()
        .start(ProviderKind::ClaudeCode, &request)
        .await
        .unwrap();
    let observer = NativeObserver::default();
    let turn = TurnRequest::new(format!(
        "Call mcp__lazoo__lazoo_ping with {{}} exactly once, then use Bash to run exactly `echo {COMMAND_TOKEN}`. Do no other work."
    ));
    let result = session.turn_observed(&turn, &observer).await;
    let calls = observer.calls.lock().unwrap().clone();
    let permissions = observer.permissions.lock().unwrap().clone();
    eprintln!(
        "native-mcp-calls={} native-permissions={} approved-commands={}",
        calls.len(),
        permissions.len(),
        observer.approved_commands.load(Ordering::SeqCst)
    );
    session.stop().await.unwrap();
    assert!(completed(&result.unwrap()));
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "lazoo_ping");
    assert_eq!(permissions.len(), 1);
    assert_eq!(observer.approved_commands.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[ignore = "requires explicit native Claude subscription opt-in; sends at most two tiny turns"]
#[allow(clippy::too_many_lines)] // One bounded native lifecycle acceptance story.
async fn claude_native_tui_two_turn_acceptance() {
    // `NativeInteractiveSession::sibling_lazooctl` accepts this explicit path because an
    // integration-test executable normally lives under `target/debug/deps`, not beside lazooctl.
    let helper = require_live_opt_in();
    let workspace = match env::var_os("LAZOO_LIVE_NATIVE_WORKSPACE_PARENT") {
        Some(parent) => tempfile::tempdir_in(parent),
        None => tempfile::tempdir(),
    }
    .expect("temporary native acceptance workspace");
    let mut request = SessionRequest::new(workspace.path())
        .with_managed_workspace()
        .with_native_workspace_trust_approved()
        .with_tools(vec![bounded_tool()]);
    request.turn_timeout_millis = 60_000;
    let registry = ProviderRegistry::default();
    let session = registry
        .start(ProviderKind::ClaudeCode, &request)
        .await
        .expect("start the canonical native Claude TUI");
    let native_id = session.id();
    assert!(
        !native_id.starts_with("pending:"),
        "SessionStart must supply a native id"
    );
    let host_pid = session.native_host_pid().expect("native host pid");
    assert!(host_pid > 0);
    assert!(session.native_view_available());
    assert!(
        session.resources().is_some(),
        "native process must be job-accounted"
    );

    let first_view = session
        .open_native_view()
        .await
        .expect("open read-only native view")
        .expect("interactive session exposes a native view");
    let first_terminal = borrowed_terminal(&first_view);
    assert!(first_terminal.exit_status().expect("terminal status").live);

    let observer = NativeObserver::default();
    let first = session
        .turn_observed(
            &TurnRequest::new(
                "The word theater has seven letters. Reply with the marker formed as NATIVE_, then the word, then an underscore and its count. Do not use tools."
            ),
            &observer,
        )
        .await;
    if first.is_err()
        && let Ok(output) = first_terminal.read_output(0, 64 * 1024)
    {
        eprintln!(
            "FAILED_NATIVE_SCREEN={}",
            String::from_utf8_lossy(&output.bytes)
        );
    }
    let first = first.expect("first native TUI turn");
    assert!(completed(&first));
    // These are structured MessageDisplay events, not terminal-screen or prompt-echo bytes.
    assert!(
        observer.text().contains(REPLY_TOKEN),
        "native assistant response missing"
    );
    assert_eq!(
        session.id(),
        native_id,
        "first turn changed canonical native id"
    );
    let first_render = first_terminal
        .read_output(0, 64 * 1024)
        .expect("read native TUI output after first turn");
    assert!(
        !first_render.gap,
        "short acceptance output must remain in the canonical terminal history"
    );
    assert!(
        rendered_contains(&first_render.bytes, REPLY_TOKEN),
        "the actual native TUI never rendered the expected assistant response"
    );

    // Closing a read-only lease must not close its canonical ConPTY. Reopen and read the same
    // terminal history before Lazoo submits its second (and last) prompt.
    first_view.close();
    assert!(first_view.is_closed());
    let reopened = session
        .open_native_view()
        .await
        .expect("reopen read-only native view")
        .expect("canonical terminal survives view closure");
    let reopened_terminal = borrowed_terminal(&reopened);
    assert_eq!(reopened_terminal.id(), first_terminal.id());
    assert_eq!(
        session.native_host_pid(),
        Some(host_pid),
        "reopening a view must borrow the same canonical ConPTY host"
    );
    let reopened_history = reopened_terminal
        .read_output(0, 64 * 1024)
        .expect("read reopened native TUI history");
    assert!(!reopened_history.gap);
    assert!(
        rendered_contains(&reopened_history.bytes, REPLY_TOKEN),
        "reopened view did not retain the rendered first-turn assistant response"
    );

    let second = session
        .turn_observed(
            &TurnRequest::new(format!(
                "Use the Lazoo tool lazoo_ping with {{}} exactly once, then run exactly `echo {COMMAND_TOKEN}`. Do no other work."
            )),
            &observer,
        )
        .await
        .expect("second native TUI turn");
    assert!(completed(&second));
    assert_eq!(
        session.id(),
        native_id,
        "second turn changed canonical native id"
    );
    assert!(
        observer
            .calls
            .lock()
            .expect("observer tool lock")
            .iter()
            .any(|call| call.name == "lazoo_ping"),
        "native HTTP MCP call was not delivered to the active-turn observer"
    );
    let permissions = observer
        .permissions
        .lock()
        .expect("observer permission lock")
        .clone();
    assert!(
        permissions.iter().any(|(request, _)| request
            .detail
            .pointer("/tool_input/command")
            .and_then(serde_json::Value::as_str)
            == Some(format!("echo {COMMAND_TOKEN}").as_str())),
        "the governed command never reached Lazoo's pre-tool decision path"
    );
    let expected = if env::var("LAZOO_LIVE_CLAUDE_APPROVE_COMMAND").as_deref() == Ok("1") {
        PermissionDecision::Approve
    } else {
        PermissionDecision::Decline
    };
    assert!(
        permissions.iter().any(|(request, decision)| {
            request
                .detail
                .pointer("/tool_input/command")
                .and_then(serde_json::Value::as_str)
                == Some(format!("echo {COMMAND_TOKEN}").as_str())
                && *decision == expected
        }),
        "governed command decision did not match the explicit acceptance mode"
    );
    if expected == PermissionDecision::Approve {
        assert_eq!(observer.approved_commands.load(Ordering::SeqCst), 1);
    }

    reopened.close();
    session
        .stop()
        .await
        .expect("stop canonical native resources");
    assert!(!session.native_view_available());
    assert_native_history_resume(ProviderKind::ClaudeCode, &request, &native_id).await;
    drop(helper); // Never log the companion path; it may contain user-directory details.
}
