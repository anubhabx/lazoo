use std::{fmt::Write as _, fs, path::Path};

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

fn assert_no_foreign_key_violations(database: &Path) {
    use rusqlite::Connection;

    let connection = Connection::open(database).unwrap();
    let violations: i64 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
}

#[test]
fn store_enables_wal_foreign_keys_and_schema_version() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();

    let status = store.status().unwrap();

    assert_eq!(status.schema_version, 9);
    assert_eq!(status.journal_mode.to_lowercase(), "wal");
    assert!(status.foreign_keys);
    assert_no_foreign_key_violations(&temporary.path().join("lazoo.db"));
}

#[test]
fn version_one_database_upgrades_to_version_eight_without_rewriting_version_one() {
    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};

    let temporary = tempdir().unwrap();
    let database = temporary.path().join("lazoo.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                checksum TEXT NOT NULL,
                applied_at TEXT NOT NULL
            );",
        )
        .unwrap();
    let migration = include_str!("../../../migrations/0001_control_plane.sql");
    connection.execute_batch(migration).unwrap();
    let mut checksum = String::with_capacity(64);
    for byte in Sha256::digest(migration.as_bytes()) {
        write!(&mut checksum, "{byte:02x}").unwrap();
    }
    connection
        .execute(
            "INSERT INTO schema_migrations (version, checksum, applied_at)
             VALUES (1, ?1, '2026-08-28T00:00:00Z')",
            params![checksum],
        )
        .unwrap();
    drop(connection);

    let store = Store::open_path(&database).unwrap();

    assert_eq!(store.status().unwrap().schema_version, 9);
    store
        .execute_for_test(
            "INSERT INTO employees (
                id, name, provider, role, instructions, native_settings_json,
                authority_mode, checkout_policy, enabled, configuration_revision,
                created_at, updated_at
             ) VALUES (
                '00000000-0000-4000-8000-000000000001', 'Rhea', 'codex', '', '',
                '{}', 'ask', 'isolated', 1, 1,
                '2026-08-30T00:00:00Z', '2026-08-30T00:00:00Z'
             )",
        )
        .unwrap();
    assert_no_foreign_key_violations(&database);
}

#[test]
fn version_two_database_upgrades_to_version_seven_with_session_tables() {
    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};

    let temporary = tempdir().unwrap();
    let database = temporary.path().join("lazoo.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                checksum TEXT NOT NULL,
                applied_at TEXT NOT NULL
            );",
        )
        .unwrap();
    for (version, migration) in [
        (
            1,
            include_str!("../../../migrations/0001_control_plane.sql"),
        ),
        (
            2,
            include_str!("../../../migrations/0002_employee_roster.sql"),
        ),
    ] {
        connection.execute_batch(migration).unwrap();
        let mut checksum = String::with_capacity(64);
        for byte in Sha256::digest(migration.as_bytes()) {
            write!(&mut checksum, "{byte:02x}").unwrap();
        }
        connection
            .execute(
                "INSERT INTO schema_migrations (version, checksum, applied_at)
                 VALUES (?1, ?2, '2026-08-30T00:00:00Z')",
                params![version, checksum],
            )
            .unwrap();
    }
    drop(connection);

    let store = Store::open_path(&database).unwrap();

    assert_eq!(store.status().unwrap().schema_version, 9);
    store
        .execute_for_test(
            "INSERT INTO employees (
                id, name, provider, role, instructions, native_settings_json,
                authority_mode, checkout_policy, enabled, configuration_revision,
                created_at, updated_at
             ) VALUES (
                '00000000-0000-4000-8000-000000000001', 'Rhea', 'codex', '', '',
                '{}', 'ask', 'isolated', 1, 1,
                '2026-08-31T00:00:00Z', '2026-08-31T00:00:00Z'
             );
             INSERT INTO employee_sessions (
                id, employee_id, employee_configuration_revision, provider, state,
                revision, owner_boot_id, created_at, updated_at
             ) VALUES (
                '00000000-0000-4000-8000-000000000002',
                '00000000-0000-4000-8000-000000000001', 1, 'codex', 'starting', 1,
                '00000000-0000-4000-8000-000000000003',
                '2026-08-31T00:00:00Z', '2026-08-31T00:00:00Z'
             )",
        )
        .unwrap();
    assert_no_foreign_key_violations(&database);
}

#[test]
fn version_three_database_upgrades_to_version_seven_with_message_tables() {
    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};

    let temporary = tempdir().unwrap();
    let database = temporary.path().join("lazoo.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                checksum TEXT NOT NULL,
                applied_at TEXT NOT NULL
            );",
        )
        .unwrap();
    for (version, migration) in [
        (
            1,
            include_str!("../../../migrations/0001_control_plane.sql"),
        ),
        (
            2,
            include_str!("../../../migrations/0002_employee_roster.sql"),
        ),
        (
            3,
            include_str!("../../../migrations/0003_employee_sessions.sql"),
        ),
    ] {
        connection.execute_batch(migration).unwrap();
        let mut checksum = String::with_capacity(64);
        for byte in Sha256::digest(migration.as_bytes()) {
            write!(&mut checksum, "{byte:02x}").unwrap();
        }
        connection
            .execute(
                "INSERT INTO schema_migrations (version, checksum, applied_at)
                 VALUES (?1, ?2, '2026-09-02T00:00:00Z')",
                params![version, checksum],
            )
            .unwrap();
    }
    drop(connection);

    let store = Store::open_path(&database).unwrap();

    assert_eq!(store.status().unwrap().schema_version, 9);
    store
        .execute_for_test(
            "INSERT INTO employees (
                id, name, provider, role, instructions, native_settings_json,
                authority_mode, checkout_policy, enabled, configuration_revision,
                created_at, updated_at
             ) VALUES (
                '00000000-0000-4000-8000-000000000001', 'Rhea', 'codex', '', '',
                '{}', 'ask', 'isolated', 1, 1,
                '2026-09-02T00:00:00Z', '2026-09-02T00:00:00Z'
             );
             INSERT INTO messages (
                id, envelope_version, idempotency_key, correlation_id,
                sender_kind, sender_employee_id, recipient_employee_id, kind, body_json,
                references_json, created_at
             ) VALUES (
                '00000000-0000-4000-8000-000000000002', 1, 'message-1',
                '00000000-0000-4000-8000-000000000002',
                'employee',
                '00000000-0000-4000-8000-000000000001',
                '00000000-0000-4000-8000-000000000001', 'peer', '{}', '[]',
                '2026-09-02T00:00:00Z'
             )",
        )
        .unwrap();
    assert_no_foreign_key_violations(&database);
}

#[test]
#[allow(clippy::too_many_lines)]
fn version_four_database_upgrades_to_version_seven_without_losing_message_history() {
    use lazoo_protocol::{MessageDeliveryState, MessageSender};
    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};
    use uuid::Uuid;

    let temporary = tempdir().unwrap();
    let database = temporary.path().join("lazoo.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                checksum TEXT NOT NULL,
                applied_at TEXT NOT NULL
             );",
        )
        .unwrap();
    for (version, migration) in [
        (
            1,
            include_str!("../../../migrations/0001_control_plane.sql"),
        ),
        (
            2,
            include_str!("../../../migrations/0002_employee_roster.sql"),
        ),
        (
            3,
            include_str!("../../../migrations/0003_employee_sessions.sql"),
        ),
        (
            4,
            include_str!("../../../migrations/0004_collaboration_bus.sql"),
        ),
    ] {
        connection.execute_batch(migration).unwrap();
        let mut checksum = String::with_capacity(64);
        for byte in Sha256::digest(migration.as_bytes()) {
            write!(&mut checksum, "{byte:02x}").unwrap();
        }
        connection
            .execute(
                "INSERT INTO schema_migrations (version, checksum, applied_at)
                 VALUES (?1, ?2, '2026-09-03T00:00:00Z')",
                params![version, checksum],
            )
            .unwrap();
    }

    let employee_id = Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap();
    let message_id = Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap();
    let attempt_id = Uuid::parse_str("00000000-0000-4000-8000-000000000003").unwrap();
    connection
        .execute_batch(
            "INSERT INTO employees (
                id, name, provider, role, instructions, native_settings_json,
                authority_mode, checkout_policy, enabled, configuration_revision,
                created_at, updated_at
             ) VALUES (
                '00000000-0000-4000-8000-000000000001', 'Rhea', 'codex', '', '',
                '{}', 'ask', 'isolated', 1, 1,
                '2026-09-03T00:00:00Z', '2026-09-03T00:00:00Z'
             );
             INSERT INTO messages (
                id, envelope_version, idempotency_key, correlation_id,
                sender_employee_id, recipient_employee_id, kind, body_json,
                references_json, created_at
             ) VALUES (
                '00000000-0000-4000-8000-000000000002', 1, 'message-1',
                '00000000-0000-4000-8000-000000000002',
                '00000000-0000-4000-8000-000000000001',
                '00000000-0000-4000-8000-000000000001', 'peer',
                '{\"text\":\"hello\"}', '[]', '2026-09-03T00:00:00Z'
             );
             INSERT INTO message_deliveries (
                message_id, state, revision, attempt_count, max_attempts,
                acknowledged_at, updated_at
             ) VALUES (
                '00000000-0000-4000-8000-000000000002', 'acknowledged', 3, 1, 3,
                '2026-09-03T00:00:02Z', '2026-09-03T00:00:02Z'
             );
             INSERT INTO message_delivery_attempts (
                id, message_id, attempt_number, state, started_at, finished_at,
                acknowledgement_key, acknowledgement_evidence_json
             ) VALUES (
                '00000000-0000-4000-8000-000000000003',
                '00000000-0000-4000-8000-000000000002', 1, 'acknowledged',
                '2026-09-03T00:00:01Z', '2026-09-03T00:00:02Z', 'native-turn-1',
                '{\"progress\":true}'
             );
             INSERT INTO message_commands (
                scope, idempotency_key, request_id, message_id, created_at
             ) VALUES (
                'message.send', 'message-1', 'request-message-1',
                '00000000-0000-4000-8000-000000000002', '2026-09-03T00:00:00Z'
             );",
        )
        .unwrap();
    drop(connection);

    let store = Store::open_path(&database).unwrap();
    assert_eq!(store.status().unwrap().schema_version, 9);

    let message = store.get_message(message_id).unwrap();
    assert_eq!(message.envelope.version, 1);
    assert_eq!(
        message.envelope.sender,
        MessageSender::Employee(lazoo_protocol::MessageEndpoint {
            employee_id,
            session_id: None,
        })
    );
    assert_eq!(message.delivery.state, MessageDeliveryState::Acknowledged);
    assert_eq!(message.delivery.revision, 3);
    assert_eq!(message.attempts.len(), 1);
    assert_eq!(message.attempts[0].id, attempt_id);
    assert_eq!(
        message.attempts[0].acknowledgement_key.as_deref(),
        Some("native-turn-1")
    );
    assert_no_foreign_key_violations(&database);
}

#[test]
#[allow(clippy::too_many_lines)]
fn version_five_turns_backfill_conservative_task_identity_and_state() {
    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};
    use uuid::Uuid;

    let temporary = tempdir().unwrap();
    let database = temporary.path().join("lazoo.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                checksum TEXT NOT NULL,
                applied_at TEXT NOT NULL
             );",
        )
        .unwrap();
    for (version, migration) in [
        (
            1,
            include_str!("../../../migrations/0001_control_plane.sql"),
        ),
        (
            2,
            include_str!("../../../migrations/0002_employee_roster.sql"),
        ),
        (
            3,
            include_str!("../../../migrations/0003_employee_sessions.sql"),
        ),
        (
            4,
            include_str!("../../../migrations/0004_collaboration_bus.sql"),
        ),
        (
            5,
            include_str!("../../../migrations/0005_conversations.sql"),
        ),
    ] {
        connection.execute_batch(migration).unwrap();
        let mut checksum = String::with_capacity(64);
        for byte in Sha256::digest(migration.as_bytes()) {
            write!(&mut checksum, "{byte:02x}").unwrap();
        }
        connection
            .execute(
                "INSERT INTO schema_migrations (version, checksum, applied_at)
                 VALUES (?1, ?2, '2026-09-04T00:00:00Z')",
                params![version, checksum],
            )
            .unwrap();
    }

    let employee_id = Uuid::from_u128(1);
    let conversation_id = Uuid::from_u128(2);
    connection
        .execute(
            "INSERT INTO employees (
                id, name, provider, role, instructions, native_settings_json,
                authority_mode, checkout_policy, enabled, configuration_revision,
                created_at, updated_at
             ) VALUES (?1, 'Rhea', 'codex', 'Lead', '', '{}', 'full-autonomy',
                'isolated', 1, 7, ?2, ?2)",
            params![employee_id.to_string(), "2026-09-04T00:00:00Z"],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO conversations (
                id, contract_version, lead_employee_id,
                lead_employee_configuration_revision, revision, created_at, updated_at
             ) VALUES (?1, 1, ?2, 7, 1, ?3, ?3)",
            params![
                conversation_id.to_string(),
                employee_id.to_string(),
                "2026-09-04T00:00:00Z"
            ],
        )
        .unwrap();

    let mappings = [
        ("recorded", "accepted"),
        ("submitting", "running"),
        ("accepted", "running"),
        ("completed", "completed"),
        ("retryable-failed", "accepted"),
        ("terminal-failed", "failed"),
        ("cancelled", "cancelled"),
        ("uncertain", "uncertain"),
    ];
    let mut user_turn_ids = Vec::new();
    for (index, (turn_state, _)) in mappings.iter().enumerate() {
        for (author_offset, author_kind) in ["user", "employee"].iter().enumerate() {
            let ordinal = u128::try_from(index * 2 + author_offset + 10).unwrap();
            let turn_id = Uuid::from_u128(ordinal);
            let message_id = Uuid::from_u128(ordinal + 100);
            if *author_kind == "user" {
                user_turn_ids.push(turn_id);
            }
            connection
                .execute(
                    "INSERT INTO messages (
                        id, envelope_version, idempotency_key, correlation_id, sender_kind,
                        sender_employee_id, recipient_employee_id, conversation_id, task_id,
                        kind, body_json, references_json, created_at
                     ) VALUES (?1, 2, ?2, ?1, ?3, ?4, ?5, ?6, ?7, 'peer', '{}', '[]', ?8)",
                    params![
                        message_id.to_string(),
                        format!("legacy-message-{ordinal}"),
                        author_kind,
                        (*author_kind == "employee").then(|| employee_id.to_string()),
                        employee_id.to_string(),
                        conversation_id.to_string(),
                        turn_id.to_string(),
                        "2026-09-04T00:00:00Z",
                    ],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO conversation_turns (
                        id, contract_version, conversation_id, author_kind, author_employee_id,
                        recipient_employee_id, recipient_employee_configuration_revision, text,
                        message_id, correlation_id, state, revision, created_at, updated_at
                     ) VALUES (?1, 1, ?2, ?3, ?4, ?5, 7, ?6, ?7, ?7, ?8, 1, ?9, ?9)",
                    params![
                        turn_id.to_string(),
                        conversation_id.to_string(),
                        author_kind,
                        (*author_kind == "employee").then(|| employee_id.to_string()),
                        employee_id.to_string(),
                        format!("legacy {author_kind} {turn_state}"),
                        message_id.to_string(),
                        turn_state,
                        "2026-09-04T00:00:00Z",
                    ],
                )
                .unwrap();
        }
    }
    connection
        .execute(
            "INSERT INTO turn_commands (scope, idempotency_key, request_id, turn_id, created_at)
             VALUES ('turn.record', 'legacy-command', 'legacy-request', ?1, ?2)",
            params![user_turn_ids[0].to_string(), "2026-09-04T00:00:00Z"],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO audit_events (id, event_type, payload_json, created_at)
             VALUES (?1, 'legacy.event', '{}', ?2)",
            params![Uuid::from_u128(500).to_string(), "2026-09-04T00:00:00Z"],
        )
        .unwrap();
    drop(connection);

    let store = Store::open_path(&database).unwrap();
    assert_eq!(store.status().unwrap().schema_version, 9);
    drop(store);

    let connection = Connection::open(&database).unwrap();
    let task_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(task_count, 8);
    for (turn_state, task_state) in mappings {
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*)
                 FROM conversation_turns turn
                 JOIN tasks task ON task.id = turn.task_id
                 JOIN messages message ON message.id = turn.message_id
                 WHERE turn.author_kind = 'user' AND turn.state = ?1
                   AND task.id = turn.id AND task.root_task_id = turn.id
                   AND task.state = ?2
                   AND task.requested_authority = 'ask'
                   AND task.effective_authority = 'ask'
                   AND message.task_id = turn.id",
                params![turn_state, task_state],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "missing user backfill for {turn_state}");
    }
    let employee_links: i64 = connection
        .query_row(
            "SELECT COUNT(*)
             FROM conversation_turns turn
             JOIN messages message ON message.id = turn.message_id
             WHERE turn.author_kind = 'employee'
               AND (turn.task_id IS NOT NULL OR message.task_id IS NOT NULL)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(employee_links, 0);
    let command_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM turn_commands WHERE idempotency_key = 'legacy-command'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(command_count, 1);
    let event_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM audit_events WHERE event_type = 'legacy.event'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(event_count, 1);
    assert_no_foreign_key_violations(&database);
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
