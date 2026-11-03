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
async fn preferences_and_usage_rpc_shapes_are_fenced_bounded_and_public() {
    let temp = tempfile::tempdir().unwrap();
    let root = DataRoot::from_path(temp.path());
    root.ensure().unwrap();
    let store = Store::open_path(root.state_dir().join("test.db")).unwrap();
    let app =
        App::new_with_provider_environment(store.clone(), root, DiscoveryEnvironment::empty())
            .unwrap();
    let id = Uuid::new_v4();
    let created=call(&app,"provider.accounts.put",json!({"expectedRevision":0,"secret":"private-usage-key","spec":{"id":id,"provider":"codex","label":"Work","credentialKind":"api-key","profileDirectory":"","credentialEnvironment":"OPENAI_API_KEY","baseUrl":null,"apiProtocol":"responses","backendProvider":null,"models":[],"enabled":true}})).await;
    assert!(created.error.is_none(), "{created:?}");
    let preferences = call(
        &app,
        "provider.accounts.preferences",
        json!({"id":id,"expectedRevision":1,"reservePercent":25,"idempotencyKey":"prefs"}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(preferences["revision"], 2);
    assert_eq!(preferences["reservePercent"], 25);
    assert_eq!(preferences["reserveEnforceable"], false);
    assert!(preferences["reserveReason"].is_string());
    let stale = call(
        &app,
        "provider.accounts.preferences",
        json!({"id":id,"expectedRevision":1,"reservePercent":10}),
    )
    .await;
    assert_eq!(stale.error.unwrap().code, "LZ-ACCOUNT-STALE");
    let policy = call(
        &app,
        "provider.accounts.policy",
        json!({"provider":"codex","mode":"priority","order":[id],"revision":0}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(policy["order"], json!([id]));
    store.observe_provider_account(id, 429, 25, 60).unwrap();
    let listed = call(&app, "provider.accounts.list", json!({}))
        .await
        .result
        .unwrap();
    assert!(listed["accounts"][0]["exhaustedUntil"].is_string());
    assert!(listed["accounts"][0]["windows"].is_array());
    let usage = call(
        &app,
        "provider.accounts.usage",
        json!({"accountId":id,"bucket":"hour"}),
    )
    .await
    .result
    .unwrap();
    assert_eq!(usage["buckets"][0]["requests"], 1);
    assert_eq!(usage["buckets"][0]["failures"], 1);
    assert!(usage["buckets"][0]["inputTokens"].is_null());
    assert_eq!(usage["buckets"][0]["tokensReported"], 0);
    assert_eq!(
        usage["observableProviders"]["github-copilot"]["windows"],
        false
    );
    assert!(
        call(&app, "provider.accounts.usage", json!({"accountId":null}))
            .await
            .result
            .unwrap()["buckets"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for invalid in [
        json!({"from":"private-usage-key"}),
        json!({"bucket":"private-usage-key"}),
        json!({"unknown":"private-usage-key"}),
    ] {
        let result = call(&app, "provider.accounts.usage", invalid).await;
        assert!(result.error.is_some());
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("private-usage-key")
        );
    }
    for value in [preferences, policy, listed, usage] {
        assert!(!value.to_string().contains("private-usage-key"));
    }
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
