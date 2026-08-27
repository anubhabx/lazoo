use std::{fs, path::Path};

use lazoo_protocol::CheckoutMode;
use lazoo_runtime::DataRoot;
use lazoo_store::{RegisterProjectInput, Store, StoreError};
use tempfile::tempdir;

fn registration(key: &str, path: &Path) -> RegisterProjectInput {
    RegisterProjectInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        repository_identity: format!("identity-{}", path.display()),
        canonical_path: path.to_path_buf(),
        display_name: "Example".to_owned(),
        checkout_mode: CheckoutMode::Isolated,
    }
}

#[test]
fn store_enables_wal_foreign_keys_and_schema_version() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();

    let status = store.status().unwrap();

    assert_eq!(status.schema_version, 1);
    assert_eq!(status.journal_mode.to_lowercase(), "wal");
    assert!(status.foreign_keys);
}

#[test]
fn project_registration_is_atomic_and_idempotent() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let input = registration("same-key", temporary.path());

    let first = store.register_project(&input).unwrap();
    let second = store.register_project(&input).unwrap();

    assert_eq!(first, second);
    assert_eq!(store.list_projects().unwrap().len(), 1);
    assert_eq!(store.list_events(0, 100).unwrap().len(), 1);
    assert_eq!(store.status().unwrap().outbox_pending, 1);
}

#[test]
fn same_repository_with_a_new_command_is_a_conflict() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    store
        .register_project(&registration("first", temporary.path()))
        .unwrap();

    let error = store
        .register_project(&registration("second", temporary.path()))
        .unwrap_err();

    assert!(matches!(error, StoreError::ProjectConflict { .. }));
}

#[test]
fn audit_sequence_continues_after_reopen_and_outbox_replays() {
    let temporary = tempdir().unwrap();
    let database = temporary.path().join("lazoo.db");
    let store = Store::open_path(&database).unwrap();
    let first = store
        .register_project(&registration("first", temporary.path()))
        .unwrap();
    assert_eq!(first.event_sequence, 1);
    drop(store);

    let store = Store::open_path(&database).unwrap();
    let published = store.publish_pending_outbox().unwrap();

    assert_eq!(published.len(), 1);
    assert_eq!(published[0].sequence, 1);
    assert_eq!(store.status().unwrap().outbox_pending, 0);
}

#[test]
fn changed_applied_migration_checksum_is_rejected() {
    let temporary = tempdir().unwrap();
    let database = temporary.path().join("lazoo.db");
    let store = Store::open_path(&database).unwrap();
    store
        .execute_for_test("UPDATE schema_migrations SET checksum = 'changed' WHERE version = 1")
        .unwrap();
    drop(store);

    let error = Store::open_path(database).unwrap_err();

    assert!(matches!(
        error,
        StoreError::MigrationChecksum { version: 1 }
    ));
}

#[test]
fn emergency_segments_import_exactly_once_and_reconcile() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());
    root.ensure().unwrap();
    let pending = root.spool_pending().join("incident-1.ndjson");
    fs::write(
        &pending,
        concat!(
            r#"{"incident_id":"incident-1","code":"LZ-DAEMON-UNAVAILABLE","#,
            r#""message":"restart budget exhausted","occurred_at":"2026-08-28T00:00:00Z"}"#,
            "\n"
        ),
    )
    .unwrap();
    let store = Store::open_path(root.state_db()).unwrap();

    assert_eq!(store.import_pending_incidents(&root).unwrap(), 1);
    assert_eq!(store.import_pending_incidents(&root).unwrap(), 0);
    assert_eq!(store.status().unwrap().emergency_incident_count, 1);
    assert!(!pending.exists());
    assert!(root.spool_reconciled().join("incident-1.ndjson").exists());
}
