use lazoo_protocol::{
    ActivationPolicy, AuthorityMode, EmployeeCheckoutPolicy, JsonRpcRequest, ProviderObservation,
};
use lazoo_providers::DiscoveryEnvironment;
use lazoo_runtime::DataRoot;
use lazoo_store::{CreateEmployeeInput, Store};
use lazood::App;
use serde_json::json;
use tempfile::tempdir;

fn request(method: &str, params: serde_json::Value) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: method.into(),
        method: method.into(),
        params,
    }
}

#[tokio::test]
async fn worker_control_requires_the_employee_native_model_and_authority_ceiling() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let employee = store
        .create_employee(&CreateEmployeeInput {
            request_id: "worker-control-employee".into(),
            idempotency_key: "worker-control-employee".into(),
            name: "Memory worker".into(),
            provider: "codex".into(),
            role: "Summarizer".into(),
            instructions: "Summarize archived memory.".into(),
            model: Some("gpt-6.1".into()),
            reasoning_effort: None,
            native_settings: json!({}),
            authority_mode: AuthorityMode::Ask,
            checkout_policy: EmployeeCheckoutPolicy::Isolated,
            activation_policy: ActivationPolicy::Manual,
            idle_timeout_seconds: 0,
            enabled: true,
            provider_observation: Some(ProviderObservation {
                provider: "codex".into(),
                verification: "configured".into(),
                version: None,
                protocol: None,
                capabilities: json!({}),
                authentication: json!({}),
                degradation_reason: None,
                checked_at: "2026-10-15T00:00:00Z".into(),
            }),
        })
        .unwrap()
        .employee;
    let app =
        App::new_with_provider_environment(store, root, DiscoveryEnvironment::empty()).unwrap();
    let rejected = app
        .handle(request(
            "memory.worker.configure",
            json!({
                "employeeId": employee.id,
                "model": "different-model",
                "requestsPerMinute": 1,
                "enabled": true,
                "expectedRevision": null,
            }),
        ))
        .await;
    assert!(rejected.error.is_some());
    let configured = app
        .handle(request(
            "memory.worker.configure",
            json!({
                "employeeId": employee.id,
                "model": "gpt-6.1",
                "requestsPerMinute": 1,
                "enabled": true,
                "expectedRevision": null,
            }),
        ))
        .await;
    let configured = configured.result.unwrap();
    assert_eq!(configured["provider"], "codex");
    assert_eq!(configured["model"], "gpt-6.1");
    assert_eq!(
        configured["projectPermission"]["state"],
        "nativeEmployeeOnly"
    );
    let disabled = app
        .handle(request(
            "memory.worker.configure",
            json!({
                "employeeId": employee.id, "model": "no-longer-selected",
                "requestsPerMinute": 1, "enabled": false,
                "expectedRevision": configured["settings"]["revision"],
            }),
        ))
        .await;
    assert_eq!(disabled.result.unwrap()["settings"]["enabled"], false);
}
