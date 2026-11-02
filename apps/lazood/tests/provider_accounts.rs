use lazoo_protocol::JsonRpcRequest;
use lazoo_providers::DiscoveryEnvironment;
use lazoo_runtime::DataRoot;
use lazoo_store::Store;
use lazood::App;
use serde_json::{Value, json};
use uuid::Uuid;

async fn call(app: &App, method: &str, params: Value) -> lazoo_protocol::JsonRpcResponse {
    app.handle(JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Uuid::new_v4().to_string(),
        method: method.into(),
        params,
    })
    .await
}

#[tokio::test]
async fn owner_account_rpc_protects_credentials_and_fences_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = DataRoot::from_path(temp.path());
    root.ensure().unwrap();
    let store = Store::open_path(root.state_dir().join("test.db")).unwrap();
    let app =
        App::new_with_provider_environment(store.clone(), root, DiscoveryEnvironment::empty())
            .unwrap();
    let id = Uuid::new_v4();
    let request = json!({"expectedRevision": 0, "idempotencyKey":"ui-transport-key", "secret": "private-rpc-key-fixture", "spec": {
        "id": id, "provider": "codex", "label": "Work", "credentialKind": "api-key", "profileDirectory": "",
        "credentialEnvironment": "OPENAI_API_KEY", "baseUrl": null, "apiProtocol": "responses", "backendProvider": null,
        "models": [], "enabled": true
    }});
    let created = call(&app, "provider.accounts.put", request).await;
    assert!(created.error.is_none(), "{created:?}");
    assert_eq!(created.result.as_ref().unwrap()["revision"], 1);
    for response in [
        created,
        call(&app, "provider.accounts.list", json!({})).await,
    ] {
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("private-rpc-key-fixture")
        );
    }
    let cipher = store.provider_account_secret(id).unwrap().unwrap();
    assert!(!String::from_utf8_lossy(&cipher).contains("private-rpc-key-fixture"));
    assert_eq!(
        lazoo_runtime::unprotect_for_current_user(&cipher).unwrap(),
        b"private-rpc-key-fixture"
    );
    let stale = call(
        &app,
        "provider.accounts.remove",
        json!({"id": id, "expectedRevision": 0}),
    )
    .await;
    assert_eq!(stale.error.unwrap().code, "LZ-ACCOUNT-STALE");
    let policy = call(&app, "provider.accounts.policy", json!({"provider":"codex", "mode":"pinned", "accountId":id, "revision":0,"idempotencyKey":"policy-ui-key"})).await;
    assert!(policy.error.is_none(), "{policy:?}");
    let removed = call(
        &app,
        "provider.accounts.remove",
        json!({"id": id, "expectedRevision": 1}),
    )
    .await;
    assert!(removed.error.is_none());
    assert!(store.provider_account_secret(id).is_err());
}

#[tokio::test]
async fn account_rpc_never_quotes_invalid_credentials_or_accepts_arbitrary_environment() {
    let temp = tempfile::tempdir().unwrap();
    let root = DataRoot::from_path(temp.path());
    root.ensure().unwrap();
    let app = App::new_with_provider_environment(
        Store::open_path(root.state_dir().join("test.db")).unwrap(),
        root,
        DiscoveryEnvironment::empty(),
    )
    .unwrap();
    let malformed = call(
        &app,
        "provider.accounts.put",
        json!({"spec":"private-token-fixture"}),
    )
    .await;
    assert!(malformed.error.is_some());
    assert!(
        !serde_json::to_string(&malformed)
            .unwrap()
            .contains("private-token-fixture")
    );
    for variable in ["PATH", "NODE_OPTIONS", "COPILOT_ALLOW_ALL"] {
        let result = call(&app, "provider.accounts.put", json!({"expectedRevision":0, "secret":"fixture", "spec": {
            "id":Uuid::new_v4(), "provider":"github-copilot", "label":"Invalid", "credentialKind":"api-key", "profileDirectory":"",
            "credentialEnvironment":variable, "baseUrl":null, "apiProtocol":null, "backendProvider":null, "enabled":true
        }})).await;
        assert!(result.error.is_some(), "accepted {variable}");
    }
    let unsupported_endpoint = call(&app, "provider.accounts.put", json!({"expectedRevision":0,"secret":"fixture","spec":{
        "id":Uuid::new_v4(),"provider":"github-copilot","label":"Invalid endpoint","credentialKind":"api-key","profileDirectory":"",
        "credentialEnvironment":"COPILOT_GITHUB_TOKEN","baseUrl":"https://example.com/v1","apiProtocol":null,"backendProvider":null,"enabled":true
    }})).await;
    assert!(
        unsupported_endpoint.error.is_some(),
        "reject unsupported endpoints when saving, before a session launch"
    );
}
