use std::fmt::Write;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::StoreError;

const VERSION: u32 = 2;
const MIGRATIONS: &[(u32, &str)] = &[
    (
        1,
        include_str!("../../../migrations/0001_control_plane.sql"),
    ),
    (
        2,
        include_str!("../../../migrations/0002_employee_roster.sql"),
    ),
];

pub fn apply_migrations(connection: &mut Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            checksum TEXT NOT NULL,
            applied_at TEXT NOT NULL
        );",
    )?;
    let maximum: u32 = connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;
    if maximum > VERSION {
        return Err(StoreError::UnknownMigration {
            found: maximum,
            supported: VERSION,
        });
    }

    for &(version, migration) in MIGRATIONS {
        let digest = Sha256::digest(migration.as_bytes());
        let mut checksum = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(&mut checksum, "{byte:02x}").expect("writing to a String cannot fail");
        }
        let applied: Option<String> = connection
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version = ?1",
                [version],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(applied) = applied {
            if applied != checksum {
                return Err(StoreError::MigrationChecksum { version });
            }
            continue;
        }

        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(migration)?;
        transaction.execute(
            "INSERT INTO schema_migrations (version, checksum, applied_at)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![version, checksum],
        )?;
        transaction.commit()?;
    }
    Ok(())
}
