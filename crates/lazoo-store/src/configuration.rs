//! Durable, secret-safe intent and receipt ledger for provider configuration operations.

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{Store, StoreError};

pub const CONFIGURATION_PREVIEW_TTL: Duration = Duration::minutes(10);
pub const CONFIGURATION_PREVIEW_CAP: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigurationOperationState {
    Previewed,
    ApplyIntent,
    Prepared,
    Applied,
    RecoveryNeeded,
    RolledBack,
    DriftRefused,
    Expired,
}

impl ConfigurationOperationState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Previewed => "previewed",
            Self::ApplyIntent => "apply-intent",
            Self::Prepared => "prepared",
            Self::Applied => "applied",
            Self::RecoveryNeeded => "recovery-needed",
            Self::RolledBack => "rolled-back",
            Self::DriftRefused => "drift-refused",
            Self::Expired => "expired",
        }
    }
    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "previewed" => Ok(Self::Previewed),
            "apply-intent" => Ok(Self::ApplyIntent),
            "prepared" => Ok(Self::Prepared),
            "applied" => Ok(Self::Applied),
            "recovery-needed" => Ok(Self::RecoveryNeeded),
            "rolled-back" => Ok(Self::RolledBack),
            "drift-refused" => Ok(Self::DriftRefused),
            "expired" => Ok(Self::Expired),
            _ => Err(StoreError::InvalidRecord {
                field: "configuration state",
                message: "unknown state".into(),
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConfigurationOperation {
    pub id: Uuid,
    pub provider: String,
    pub target: PathBuf,
    /// Public, redacted provider-format metadata only.
    pub preview: Value,
    pub desired: Value,
    pub observed: Value,
    pub original_hash: String,
    pub candidate_hash: String,
    pub preview_revision: String,
    pub state: ConfigurationOperationState,
    pub revision: u64,
    pub backup_name: Option<String>,
    pub backup_hash: Option<String>,
    /// Server-created receipt metadata; no caller-provided receipt is ever accepted by the RPC.
    pub receipt: Option<Value>,
    pub expires_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug)]
pub struct CreateConfigurationPreviewInput {
    pub id: Uuid,
    pub provider: String,
    pub target: PathBuf,
    pub preview: Value,
    pub desired: Value,
    pub observed: Value,
    pub original_hash: String,
    pub candidate_hash: String,
    pub preview_revision: String,
}

#[derive(Clone, Debug)]
pub struct BeginConfigurationApplyInput {
    pub id: Uuid,
    pub expected_revision: u64,
}

#[derive(Clone, Debug)]
pub struct RecordConfigurationReceiptInput {
    pub id: Uuid,
    pub expected_revision: u64,
    pub backup_name: String,
    pub backup_hash: String,
    pub receipt: Value,
    pub observed: Value,
}

impl Store {
    pub fn create_configuration_preview(
        &self,
        input: &CreateConfigurationPreviewInput,
    ) -> Result<ConfigurationOperation, StoreError> {
        validate_preview(input)?;
        let now = Utc::now();
        let expires = now + CONFIGURATION_PREVIEW_TTL;
        let mut connection = self.lock()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        expire_previews(&tx, now)?;
        let active: i64 = tx.query_row(
            "SELECT COUNT(*) FROM configuration_operations WHERE state='previewed' AND expires_at > ?1",
            [now.to_rfc3339()], |row| row.get(0),
        )?;
        if usize::try_from(active).map_err(|_| StoreError::NegativeSequence)?
            >= CONFIGURATION_PREVIEW_CAP
        {
            return Err(StoreError::ConfigurationPreviewCap);
        }
        tx.execute(
            "INSERT INTO configuration_operations(id,provider,target,preview_json,desired_json,observed_json,original_hash,candidate_hash,preview_revision,state,revision,expires_at,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'previewed',1,?10,?11,?11)",
            params![input.id.to_string(), input.provider, input.target.to_string_lossy(), serde_json::to_string(&input.preview)?, serde_json::to_string(&input.desired)?, serde_json::to_string(&input.observed)?, input.original_hash, input.candidate_hash, input.preview_revision, expires.to_rfc3339(), now.to_rfc3339()],
        )?;
        let operation = configuration_from_tx(&tx, input.id)?;
        tx.commit()?;
        Ok(operation)
    }

    pub fn begin_configuration_apply(
        &self,
        input: &BeginConfigurationApplyInput,
    ) -> Result<ConfigurationOperation, StoreError> {
        let now = Utc::now();
        let mut connection = self.lock()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        expire_previews(&tx, now)?;
        let operation = configuration_from_tx(&tx, input.id)?;
        require_configuration_revision(&operation, input.expected_revision)?;
        if operation.state == ConfigurationOperationState::Expired {
            return Err(StoreError::ConfigurationPreviewExpired { id: input.id });
        }
        if operation.state != ConfigurationOperationState::Previewed {
            return Err(configuration_state(&operation, "previewed"));
        }
        tx.execute("UPDATE configuration_operations SET state='apply-intent',revision=revision+1,updated_at=?2 WHERE id=?1", params![input.id.to_string(),now.to_rfc3339()])?;
        let operation = configuration_from_tx(&tx, input.id)?;
        tx.commit()?;
        Ok(operation)
    }

    /// Persist the receipt after the backup has been written but before native replacement.
    pub fn prepare_configuration_receipt(
        &self,
        input: &RecordConfigurationReceiptInput,
    ) -> Result<ConfigurationOperation, StoreError> {
        validate_value("configuration receipt", &input.receipt, 16 * 1024)?;
        validate_value("configuration observed", &input.observed, 64 * 1024)?;
        if input.backup_name.contains(['/', '\\'])
            || input.backup_name.is_empty()
            || input.backup_hash.len() != 64
        {
            return Err(StoreError::InvalidRecord {
                field: "configuration receipt",
                message: "invalid backup metadata".into(),
            });
        }
        let now = Utc::now();
        let mut connection = self.lock()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let operation = configuration_from_tx(&tx, input.id)?;
        require_configuration_revision(&operation, input.expected_revision)?;
        if operation.state != ConfigurationOperationState::ApplyIntent {
            return Err(configuration_state(&operation, "apply-intent"));
        }
        tx.execute("UPDATE configuration_operations SET state='prepared',revision=revision+1,backup_name=?2,backup_hash=?3,receipt_json=?4,observed_json=?5,updated_at=?6 WHERE id=?1", params![input.id.to_string(),input.backup_name,input.backup_hash,serde_json::to_string(&input.receipt)?,serde_json::to_string(&input.observed)?,now.to_rfc3339()])?;
        let operation = configuration_from_tx(&tx, input.id)?;
        tx.commit()?;
        Ok(operation)
    }

    /// Mark a receipt as applied only after the native replacement and re-probe complete.
    pub fn record_configuration_receipt(
        &self,
        input: &RecordConfigurationReceiptInput,
    ) -> Result<ConfigurationOperation, StoreError> {
        validate_value("configuration receipt", &input.receipt, 16 * 1024)?;
        validate_value("configuration observed", &input.observed, 64 * 1024)?;
        let now = Utc::now();
        let mut connection = self.lock()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let operation = configuration_from_tx(&tx, input.id)?;
        require_configuration_revision(&operation, input.expected_revision)?;
        if operation.state != ConfigurationOperationState::Prepared {
            return Err(configuration_state(&operation, "prepared"));
        }
        // The prepared receipt is immutable; finalization only changes state and observation.
        if operation.receipt.as_ref() != Some(&input.receipt) {
            return Err(StoreError::InvalidRecord {
                field: "configuration receipt",
                message: "prepared receipt does not match".into(),
            });
        }
        if operation.backup_name.as_deref() != Some(input.backup_name.as_str())
            || operation.backup_hash.as_deref() != Some(input.backup_hash.as_str())
        {
            return Err(StoreError::InvalidRecord {
                field: "configuration receipt",
                message: "prepared backup metadata does not match".into(),
            });
        }
        tx.execute("UPDATE configuration_operations SET state='applied',revision=revision+1,observed_json=?2,updated_at=?3 WHERE id=?1", params![input.id.to_string(),serde_json::to_string(&input.observed)?,now.to_rfc3339()])?;
        let operation = configuration_from_tx(&tx, input.id)?;
        tx.commit()?;
        Ok(operation)
    }

    pub fn configuration_operation(&self, id: Uuid) -> Result<ConfigurationOperation, StoreError> {
        let connection = self.lock()?;
        configuration_from(&connection, id)
    }

    /// Intents left by a crash are never assumed applied. The daemon must inspect its backup
    /// envelope and hashes, then mark either recovery-needed, rolled-back, or drift-refused.
    pub fn configuration_recovery_candidates(
        &self,
    ) -> Result<Vec<ConfigurationOperation>, StoreError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare("SELECT id,provider,target,preview_json,desired_json,observed_json,original_hash,candidate_hash,preview_revision,state,revision,backup_name,backup_hash,receipt_json,expires_at,created_at,updated_at FROM configuration_operations WHERE state IN ('apply-intent','prepared','recovery-needed') ORDER BY created_at")?;
        statement
            .query_map([], configuration_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn settle_configuration_recovery(
        &self,
        id: Uuid,
        expected_revision: u64,
        state: ConfigurationOperationState,
        observed: &Value,
    ) -> Result<ConfigurationOperation, StoreError> {
        if !matches!(
            state,
            ConfigurationOperationState::RecoveryNeeded
                | ConfigurationOperationState::RolledBack
                | ConfigurationOperationState::DriftRefused
        ) {
            return Err(StoreError::InvalidRecord {
                field: "configuration recovery",
                message: "invalid recovery state".into(),
            });
        }
        validate_value("configuration observed", observed, 64 * 1024)?;
        let now = Utc::now();
        let mut connection = self.lock()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let operation = configuration_from_tx(&tx, id)?;
        require_configuration_revision(&operation, expected_revision)?;
        if !matches!(
            operation.state,
            ConfigurationOperationState::ApplyIntent
                | ConfigurationOperationState::Prepared
                | ConfigurationOperationState::RecoveryNeeded
                | ConfigurationOperationState::Applied
        ) {
            return Err(configuration_state(
                &operation,
                "apply-intent, recovery-needed, or applied",
            ));
        }
        tx.execute("UPDATE configuration_operations SET state=?2,revision=revision+1,observed_json=?3,updated_at=?4 WHERE id=?1", params![id.to_string(),state.as_str(),serde_json::to_string(observed)?,now.to_rfc3339()])?;
        let operation = configuration_from_tx(&tx, id)?;
        tx.commit()?;
        Ok(operation)
    }
}

fn validate_preview(input: &CreateConfigurationPreviewInput) -> Result<(), StoreError> {
    if input.provider.is_empty()
        || input.target.as_os_str().is_empty()
        || input.original_hash.len() != 64
        || input.candidate_hash.len() != 64
        || input.preview_revision.len() != 64
    {
        return Err(StoreError::InvalidRecord {
            field: "configuration preview",
            message: "invalid identity or hash".into(),
        });
    }
    validate_value("configuration preview", &input.preview, 64 * 1024)?;
    validate_value("configuration desired", &input.desired, 64 * 1024)?;
    validate_value("configuration observed", &input.observed, 64 * 1024)
}
fn validate_value(field: &'static str, value: &Value, cap: usize) -> Result<(), StoreError> {
    if contains_secret_value(value) {
        return Err(StoreError::InvalidRecord {
            field,
            message: "secret-bearing configuration values are not storable".into(),
        });
    }
    if serde_json::to_vec(value)?.len() > cap {
        Err(StoreError::InvalidRecord {
            field,
            message: "serialized value exceeds cap".into(),
        })
    } else {
        Ok(())
    }
}
fn contains_secret_value(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(contains_secret_value),
        Value::Object(object) => object
            .iter()
            .any(|(key, value)| secret_key(key) || contains_secret_value(value)),
        _ => false,
    }
}
fn secret_key(key: &str) -> bool {
    let normalized = key
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    matches!(
        normalized.as_slice(),
        b"env"
            | b"headers"
            | b"environment"
            | b"httpheaders"
            | b"envhttpheaders"
            | b"authorization"
            | b"accesstoken"
            | b"apikey"
            | b"token"
            | b"secret"
            | b"password"
            | b"credential"
            | b"bearer"
    )
}
fn expire_previews(tx: &rusqlite::Transaction<'_>, now: DateTime<Utc>) -> Result<(), StoreError> {
    tx.execute("UPDATE configuration_operations SET state='expired',revision=revision+1,updated_at=?1 WHERE state='previewed' AND expires_at <= ?1", [now.to_rfc3339()])?;
    Ok(())
}
fn configuration_from(
    connection: &rusqlite::Connection,
    id: Uuid,
) -> Result<ConfigurationOperation, StoreError> {
    connection.query_row("SELECT id,provider,target,preview_json,desired_json,observed_json,original_hash,candidate_hash,preview_revision,state,revision,backup_name,backup_hash,receipt_json,expires_at,created_at,updated_at FROM configuration_operations WHERE id=?1", [id.to_string()], configuration_row).optional()?.ok_or(StoreError::ConfigurationNotFound { id })
}
fn configuration_from_tx(
    tx: &rusqlite::Transaction<'_>,
    id: Uuid,
) -> Result<ConfigurationOperation, StoreError> {
    tx.query_row("SELECT id,provider,target,preview_json,desired_json,observed_json,original_hash,candidate_hash,preview_revision,state,revision,backup_name,backup_hash,receipt_json,expires_at,created_at,updated_at FROM configuration_operations WHERE id=?1", [id.to_string()], configuration_row).optional()?.ok_or(StoreError::ConfigurationNotFound { id })
}
fn configuration_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConfigurationOperation> {
    let id: String = row.get(0)?;
    let state: String = row.get(9)?;
    let revision: i64 = row.get(10)?;
    Ok(ConfigurationOperation {
        id: Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        provider: row.get(1)?,
        target: PathBuf::from(row.get::<_, String>(2)?),
        preview: serde_json::from_str(&row.get::<_, String>(3)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        desired: serde_json::from_str(&row.get::<_, String>(4)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        observed: serde_json::from_str(&row.get::<_, String>(5)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        original_hash: row.get(6)?,
        candidate_hash: row.get(7)?,
        preview_revision: row.get(8)?,
        state: ConfigurationOperationState::parse(&state)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        revision: u64::try_from(revision).map_err(|_| rusqlite::Error::InvalidQuery)?,
        backup_name: row.get(11)?,
        backup_hash: row.get(12)?,
        receipt: row
            .get::<_, Option<String>>(13)?
            .map(|value| serde_json::from_str(&value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        expires_at: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
    })
}
fn require_configuration_revision(
    operation: &ConfigurationOperation,
    expected: u64,
) -> Result<(), StoreError> {
    if operation.revision == expected {
        Ok(())
    } else {
        Err(StoreError::ConfigurationStale {
            id: operation.id,
            current_revision: operation.revision,
        })
    }
}
fn configuration_state(operation: &ConfigurationOperation, expected: &'static str) -> StoreError {
    StoreError::ConfigurationState {
        id: operation.id,
        state: operation.state.as_str().into(),
        expected,
    }
}
