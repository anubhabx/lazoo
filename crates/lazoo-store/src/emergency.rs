use std::{fs, path::PathBuf};

use lazoo_runtime::DataRoot;
use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::StoreError;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmergencyIncident {
    pub incident_id: String,
    pub code: String,
    pub message: String,
    pub occurred_at: String,
}

pub fn import_pending_incidents(
    connection: &mut Connection,
    root: &DataRoot,
) -> Result<usize, StoreError> {
    let mut paths: Vec<PathBuf> = fs::read_dir(root.spool_pending())
        .map_err(|source| StoreError::Io {
            path: root.spool_pending(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "ndjson")
        })
        .collect();
    paths.sort();

    let mut imported = 0_usize;
    for path in paths {
        let contents = fs::read_to_string(&path).map_err(|source| StoreError::Io {
            path: path.clone(),
            source,
        })?;
        let incidents: Vec<EmergencyIncident> = contents
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line).map_err(|error| StoreError::InvalidEmergencySegment {
                    path: path.clone(),
                    message: error.to_string(),
                })
            })
            .collect::<Result<_, _>>()?;

        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for incident in incidents {
            imported += transaction.execute(
                "INSERT OR IGNORE INTO emergency_incident_imports
                 (incident_id, code, message, occurred_at, imported_at)
                 VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![
                    incident.incident_id,
                    incident.code,
                    incident.message,
                    incident.occurred_at
                ],
            )?;
        }
        transaction.commit()?;

        let destination = root
            .spool_reconciled()
            .join(
                path.file_name()
                    .ok_or_else(|| StoreError::InvalidEmergencySegment {
                        path: path.clone(),
                        message: "segment has no file name".to_owned(),
                    })?,
            );
        fs::rename(&path, &destination).map_err(|source| StoreError::Io {
            path: destination,
            source,
        })?;
    }
    Ok(imported)
}
