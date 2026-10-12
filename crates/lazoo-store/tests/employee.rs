use lazoo_protocol::{
    ActivationPolicy, AuthorityMode, EmployeeCheckoutPolicy, ProviderObservation,
};
use lazoo_store::{
    CreateEmployeeInput, DisableEmployeeInput, RefreshProviderObservationsInput, Store, StoreError,
    UpdateEmployeeInput,
};
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
        activation_policy: lazoo_protocol::ActivationPolicy::Manual,
        idle_timeout_seconds: 0,
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

fn update_role(
    key: &str,
    employee_id: uuid::Uuid,
    expected_revision: u64,
    role: Option<&str>,
) -> UpdateEmployeeInput {
    UpdateEmployeeInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        employee_id,
        expected_revision,
        name: None,
        provider: None,
        role: role.map(str::to_owned),
        instructions: None,
        model: None,
        reasoning_effort: None,
        native_settings: None,
        authority_mode: None,
        checkout_policy: None,
        activation_policy: None,
        idle_timeout_seconds: None,
        enabled: None,
        provider_observation: None,
    }
}

fn disable(key: &str, employee_id: uuid::Uuid, expected_revision: u64) -> DisableEmployeeInput {
    DisableEmployeeInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        employee_id,
        expected_revision,
    }
}

fn refreshed_observation(verification: &str, checked_at: &str) -> ProviderObservation {
    let mut observation = creation("observation", "unused")
        .provider_observation
        .unwrap();
    verification.clone_into(&mut observation.verification);
    checked_at.clone_into(&mut observation.checked_at);
    observation.degradation_reason =
        (verification == "degraded").then(|| "provider readiness failed".to_owned());
    observation
}

fn refresh(key: &str, observations: Vec<ProviderObservation>) -> RefreshProviderObservationsInput {
    RefreshProviderObservationsInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        observations,
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

#[test]
fn update_requires_current_revision_and_noop_does_not_advance_roster() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let created = store.create_employee(&creation("create", "Rhea")).unwrap();

    let changed = store
        .update_employee(&update_role(
            "change-role",
            created.employee.id,
            1,
            Some("Principal lead"),
        ))
        .unwrap();

    assert_eq!(changed.employee.role, "Principal lead");
    assert_eq!(changed.employee.configuration_revision, 2);
    assert_eq!(changed.roster_revision, 2);
    assert_eq!(changed.event_sequence, Some(2));

    let noop = store
        .update_employee(&update_role("noop", created.employee.id, 2, None))
        .unwrap();

    assert_eq!(noop.employee, changed.employee);
    assert_eq!(noop.roster_revision, 2);
    assert_eq!(noop.event_sequence, None);
    assert_eq!(store.list_events(0, 100).unwrap().len(), 2);
}

#[test]
fn disable_preserves_record_and_rejects_stale_revision() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let created = store.create_employee(&creation("create", "Rhea")).unwrap();

    let disabled = store
        .disable_employee(&disable("disable", created.employee.id, 1))
        .unwrap();

    assert!(!disabled.employee.enabled);
    assert_eq!(disabled.employee.configuration_revision, 2);
    assert!(disabled.employee.disabled_at.is_some());
    assert_eq!(disabled.roster_revision, 2);
    assert_eq!(
        store.get_employee(created.employee.id).unwrap(),
        disabled.employee
    );

    let error = store
        .disable_employee(&disable("stale-disable", created.employee.id, 1))
        .unwrap_err();
    assert!(matches!(
        error,
        StoreError::EmployeeStale {
            current_revision: 2,
            ..
        }
    ));
    assert_eq!(store.list_events(0, 100).unwrap().len(), 2);
}

#[test]
fn observation_timestamp_only_refresh_does_not_advance_roster_revision() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    store.create_employee(&creation("create", "Rhea")).unwrap();

    let timestamp_only = store
        .refresh_provider_observations(&refresh(
            "timestamp-only",
            vec![refreshed_observation("configured", "2026-08-30T01:00:00Z")],
        ))
        .unwrap();

    assert_eq!(timestamp_only.roster.revision, 1);
    assert_eq!(timestamp_only.event_sequence, None);
    assert_eq!(
        timestamp_only.roster.entries[0].availability,
        lazoo_protocol::EmployeeAvailability::Dormant
    );
    assert_eq!(store.list_events(0, 100).unwrap().len(), 1);

    let changed = store
        .refresh_provider_observations(&refresh(
            "degraded",
            vec![refreshed_observation("degraded", "2026-08-30T02:00:00Z")],
        ))
        .unwrap();

    assert_eq!(changed.roster.revision, 2);
    assert_eq!(changed.event_sequence, Some(2));
    assert_eq!(
        changed.roster.entries[0].availability,
        lazoo_protocol::EmployeeAvailability::Offline
    );
    assert_eq!(
        changed.roster.entries[0].availability_reason.as_deref(),
        Some("provider readiness failed")
    );
}

#[test]
fn observation_for_an_unused_provider_does_not_change_roster_revision() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();

    let result = store
        .refresh_provider_observations(&refresh(
            "unused",
            vec![refreshed_observation("configured", "2026-08-30T01:00:00Z")],
        ))
        .unwrap();

    assert_eq!(result.roster.revision, 0);
    assert_eq!(result.event_sequence, None);
    assert!(result.roster.entries.is_empty());
    assert!(store.list_events(0, 100).unwrap().is_empty());
}

#[test]
fn activation_policy_defaults_to_manual_and_a_change_bumps_the_configuration_revision() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let created = store
        .create_employee(&creation("rhea", "Rhea"))
        .unwrap()
        .employee;
    assert_eq!(created.activation_policy, ActivationPolicy::Manual);

    let mut opt_in = update_role("policy", created.id, 1, None);
    opt_in.activation_policy = Some(ActivationPolicy::NoticeTurns);
    let updated = store.update_employee(&opt_in).unwrap();
    assert_eq!(
        updated.employee.activation_policy,
        ActivationPolicy::NoticeTurns
    );
    assert_eq!(updated.employee.configuration_revision, 2);
    assert_eq!(updated.roster_revision, 2);
    assert!(updated.event_sequence.is_some());
    assert_eq!(
        store.get_employee(created.id).unwrap().activation_policy,
        ActivationPolicy::NoticeTurns
    );

    // Setting the same policy again changes nothing and bumps nothing.
    let mut same = update_role("policy-again", created.id, 2, None);
    same.activation_policy = Some(ActivationPolicy::NoticeTurns);
    let unchanged = store.update_employee(&same).unwrap();
    assert_eq!(unchanged.employee.configuration_revision, 2);
    assert_eq!(unchanged.event_sequence, None);

    // A row written without the schema-10 column takes the migration default: manual.
    store
        .execute_for_test(
            "INSERT INTO employees (
                id, name, provider, role, instructions, native_settings_json,
                authority_mode, checkout_policy, enabled, configuration_revision,
                created_at, updated_at
             ) VALUES (
                '00000000-0000-4000-8000-000000000009', 'Legacy', 'codex', '', '',
                '{}', '\"ask\"', '\"isolated\"', 1, 1,
                '2026-08-30T00:00:00Z', '2026-08-30T00:00:00Z'
             )",
        )
        .unwrap();
    let legacy = store
        .get_employee("00000000-0000-4000-8000-000000000009".parse().unwrap())
        .unwrap();
    assert_eq!(legacy.activation_policy, ActivationPolicy::Manual);
    assert_eq!(legacy.authority_mode, AuthorityMode::Ask);
}

#[test]
fn idle_timeout_defaults_to_never_and_a_change_bumps_the_configuration_revision() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let created = store
        .create_employee(&creation("rhea", "Rhea"))
        .unwrap()
        .employee;
    assert_eq!(created.idle_timeout_seconds, 0);

    let mut opt_in = update_role("idle", created.id, 1, None);
    opt_in.idle_timeout_seconds = Some(900);
    let updated = store.update_employee(&opt_in).unwrap();
    assert_eq!(updated.employee.idle_timeout_seconds, 900);
    assert_eq!(updated.employee.configuration_revision, 2);
    assert!(updated.event_sequence.is_some());
    assert_eq!(
        store.get_employee(created.id).unwrap().idle_timeout_seconds,
        900
    );

    // Setting the same timeout again changes nothing and bumps nothing.
    let mut same = update_role("idle-again", created.id, 2, None);
    same.idle_timeout_seconds = Some(900);
    let unchanged = store.update_employee(&same).unwrap();
    assert_eq!(unchanged.employee.configuration_revision, 2);
    assert_eq!(unchanged.event_sequence, None);

    // The schema refuses a timeout the RPC boundary would have refused.
    let mut too_short = update_role("idle-too-short", created.id, 2, None);
    too_short.idle_timeout_seconds = Some(30);
    assert!(store.update_employee(&too_short).is_err());
    assert_eq!(
        store.get_employee(created.id).unwrap().idle_timeout_seconds,
        900
    );

    // A row written without the schema-11 column takes the migration default: never.
    store
        .execute_for_test(
            "INSERT INTO employees (
                id, name, provider, role, instructions, native_settings_json,
                authority_mode, checkout_policy, enabled, configuration_revision,
                created_at, updated_at
             ) VALUES (
                '00000000-0000-4000-8000-000000000011', 'Legacy', 'codex', '', '',
                '{}', '\"ask\"', '\"isolated\"', 1, 1,
                '2026-08-30T00:00:00Z', '2026-08-30T00:00:00Z'
             )",
        )
        .unwrap();
    let legacy = store
        .get_employee("00000000-0000-4000-8000-000000000011".parse().unwrap())
        .unwrap();
    assert_eq!(legacy.idle_timeout_seconds, 0);
}
