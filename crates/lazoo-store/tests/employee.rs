use lazoo_protocol::{AuthorityMode, EmployeeCheckoutPolicy, ProviderObservation};
use lazoo_store::{CreateEmployeeInput, Store, StoreError};
use serde_json::json;
use tempfile::tempdir;

fn creation(key: &str, name: &str) -> CreateEmployeeInput {
    CreateEmployeeInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        name: name.to_owned(),
        provider: "codex".to_owned(),
        role: "Lead".to_owned(),
        instructions: "Coordinate the team.".to_owned(),
        model: None,
        reasoning_effort: None,
        native_settings: json!({}),
        authority_mode: AuthorityMode::Ask,
        checkout_policy: EmployeeCheckoutPolicy::Isolated,
        enabled: true,
        provider_observation: Some(ProviderObservation {
            provider: "codex".to_owned(),
            verification: "configured".to_owned(),
            version: Some("0.150.0".to_owned()),
            protocol: Some("app-server".to_owned()),
            capabilities: json!({"session": true}),
            authentication: json!({"native": "ready", "external": "unknown"}),
            degradation_reason: None,
            checked_at: "2026-08-30T00:00:00Z".to_owned(),
        }),
    }
}

#[test]
fn employee_creation_is_atomic_idempotent_and_case_insensitively_unique() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();

    let first = store
        .create_employee(&creation("same-key", "Rhea"))
        .unwrap();
    let replay = store
        .create_employee(&creation("same-key", "Rhea"))
        .unwrap();

    assert_eq!(first, replay);
    assert_eq!(first.employee.name, "Rhea");
    assert_eq!(first.employee.configuration_revision, 1);
    assert_eq!(first.roster_revision, 1);
    assert_eq!(first.event_sequence, Some(1));
    assert_eq!(store.list_employees().unwrap().len(), 1);
    assert_eq!(
        store.get_employee(first.employee.id).unwrap(),
        first.employee
    );
    assert_eq!(store.roster_snapshot().unwrap().revision, 1);
    assert_eq!(store.list_events(0, 100).unwrap().len(), 1);
    assert_eq!(store.status().unwrap().outbox_pending, 1);

    let error = store
        .create_employee(&creation("new-key", "rhea"))
        .unwrap_err();
    assert!(matches!(error, StoreError::EmployeeConflict { .. }));
    assert_eq!(store.list_events(0, 100).unwrap().len(), 1);
}

#[test]
fn employee_and_roster_survive_store_reopen() {
    let temporary = tempdir().unwrap();
    let database = temporary.path().join("lazoo.db");
    let store = Store::open_path(&database).unwrap();
    let created = store
        .create_employee(&creation("create-rhea", "Rhea"))
        .unwrap();
    drop(store);

    let reopened = Store::open_path(&database).unwrap();

    assert_eq!(
        reopened.get_employee(created.employee.id).unwrap(),
        created.employee
    );
    assert_eq!(reopened.roster_snapshot().unwrap().revision, 1);
}
