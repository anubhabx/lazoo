//! Explicit, zero-model acceptance for reopening the retained Antigravity native conversation.
//!
//! This test never submits a `TurnRequest`.  The operator must provide the exact originally
//! owned managed-workspace directory and retained fixture conversation identifier.

use std::{env, path::PathBuf};

use lazoo_providers::{
    NativeViewLaunchDescriptor, ProviderKind, ProviderRegistry, ProviderSession, SessionRequest,
};

const LIVE_GATE: &str = "LAZOO_LIVE_ANTIGRAVITY_RESUME";
const WORKSPACE_ENV: &str = "LAZOO_ANTIGRAVITY_RESUME_CWD";
const HELPER_ENV: &str = "LAZOO_NATIVE_HOOK_HELPER";
const SESSION_ENV: &str = "LAZOO_ANTIGRAVITY_RESUME_ID";
const RETAINED_MARKER: &str = "NATIVE_theater_7";

fn exact_owned_workspace() -> PathBuf {
    assert_eq!(
        env::var(LIVE_GATE).as_deref(),
        Ok("1"),
        "set {LIVE_GATE}=1 before this ignored zero-model resume acceptance"
    );
    let workspace = env::var_os(WORKSPACE_ENV)
        .map(PathBuf::from)
        .expect("set LAZOO_ANTIGRAVITY_RESUME_CWD to the exact owned fixture workspace");
    assert!(
        workspace.is_absolute() && workspace.is_dir(),
        "resume workspace must be an existing absolute directory"
    );
    assert!(
        env::var_os(HELPER_ENV)
            .map(PathBuf::from)
            .is_some_and(|path| path.is_file()),
        "LAZOO_NATIVE_HOOK_HELPER must point to an existing lazooctl binary"
    );
    workspace
}

#[tokio::test]
#[ignore = "explicit zero-model Antigravity resume against the retained owned fixture"]
async fn antigravity_retained_native_history_resume_without_prompt() {
    let workspace = exact_owned_workspace();
    let native_id = env::var(SESSION_ENV).expect("provide the retained fixture conversation ID");
    uuid::Uuid::parse_str(&native_id).expect("native conversation ID must be a UUID");
    let mut request = SessionRequest::new(&workspace)
        .resume(&native_id)
        .with_managed_workspace()
        .with_native_workspace_trust_approved();
    request.startup_timeout_millis = 30_000;
    let session = ProviderRegistry::default()
        .start(ProviderKind::Antigravity, &request)
        .await
        .expect("resume retained owned Antigravity conversation");
    assert_eq!(
        session.id(),
        native_id,
        "resume changed the native conversation identity"
    );
    let view = session
        .open_native_view()
        .await
        .expect("open retained native view")
        .expect("resumed interactive session exposes a native view");
    let NativeViewLaunchDescriptor::ExistingTerminal { terminal, .. } = view.launch_descriptor()
    else {
        panic!("Antigravity resume must expose its owned terminal, not a second controller");
    };
    let output = terminal
        .read_output(0, 64 * 1024)
        .expect("read retained native terminal history");
    assert!(
        !output.gap,
        "retained history exceeded the bounded native-view buffer"
    );
    assert!(
        String::from_utf8_lossy(&output.bytes).contains(RETAINED_MARKER),
        "retained native history is missing the prior acceptance marker"
    );
    // No turn call: this is strictly resume/view/stop evidence, never another model prompt.
    view.close();
    session
        .stop()
        .await
        .expect("stop resumed native process cleanly");
    assert!(!session.native_view_available());
}
