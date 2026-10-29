use chrono::Utc;
use lazoo_domain::validate_message_command_key;
use lazoo_protocol::{
    NATIVE_TRUST_CONTRACT_V1, NativeTrustDecider, NativeTrustDecision, NativeTrustListParams,
    NativeTrustMutationResult, NativeTrustNotificationState, NativeTrustPolicy,
    NativeTrustReconcileResult, NativeTrustRequestRecord, NativeTrustState,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::json;
use uuid::Uuid;

use crate::{Store, StoreError, employee};

const TRUST_COLUMNS: &str = "sequence, id, provider, canonical_workspace_path, managed_workspace, session_id, employee_id, owner_boot_id, state, decider, revision, notification_state, notification_claim_id, created_at, updated_at, decided_at";

#[derive(Clone, Debug)]
pub struct RequestNativeTrustInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub provider: String,
    pub canonical_workspace_path: String,
    pub managed_workspace: bool,
    pub session_id: Uuid,
    pub employee_id: Uuid,
    pub owner_boot_id: Uuid,
}
#[derive(Clone, Debug)]
pub struct ResolveNativeTrustInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub trust_id: Uuid,
    pub expected_revision: u64,
    pub decision: NativeTrustDecision,
    pub auto_accept_managed_next_time: bool,
}
#[derive(Clone, Debug)]
pub struct CancelNativeTrustInput {
    pub trust_id: Uuid,
    pub expected_revision: u64,
}
#[derive(Clone, Debug)]
pub struct ClaimNativeTrustNotificationInput {
    pub trust_id: Uuid,
    pub claimant_id: Uuid,
}

impl Store {
    pub fn request_native_trust(
        &self,
        input: &RequestNativeTrustInput,
    ) -> Result<NativeTrustMutationResult, StoreError> {
        validate_input(input)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) =
            trust_for_command(&transaction, "native-trust.request", &input.idempotency_key)?
        {
            if existing.provider != input.provider
                || existing.canonical_workspace_path != input.canonical_workspace_path
                || existing.session_id != input.session_id
            {
                return Err(StoreError::NativeTrustIdempotencyConflict);
            }
            transaction.commit()?;
            return Ok(NativeTrustMutationResult {
                trust: existing,
                event_sequence: None,
            });
        }
        let now = Utc::now().to_rfc3339();
        let prior = transaction.query_row(
            &format!("SELECT {TRUST_COLUMNS} FROM native_workspace_trust_requests WHERE provider = ?1 AND canonical_workspace_path = ?2 AND state = 'approved'"),
            params![input.provider, input.canonical_workspace_path], raw_trust).optional()?;
        let policy = policy_for_provider(&transaction, &input.provider)?;
        let (state, decider) = if prior.is_some()
            || (input.managed_workspace && policy.auto_accept_managed_next_time)
        {
            (NativeTrustState::Approved, Some(NativeTrustDecider::Policy))
        } else {
            (NativeTrustState::Pending, None)
        };
        let id = Uuid::new_v4();
        transaction.execute(
            "INSERT INTO native_workspace_trust_requests (id, idempotency_key, provider, canonical_workspace_path, managed_workspace, session_id, employee_id, owner_boot_id, state, decider, revision, notification_state, notification_claim_id, created_at, updated_at, decided_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11, NULL, ?12, ?12, ?13)",
            params![id.to_string(), input.idempotency_key, input.provider, input.canonical_workspace_path, i64::from(input.managed_workspace), input.session_id.to_string(), input.employee_id.to_string(), input.owner_boot_id.to_string(), name(state)?, decider.map(name).transpose()?, name(if state == NativeTrustState::Pending { NativeTrustNotificationState::Pending } else { NativeTrustNotificationState::Settled })?, now, (state != NativeTrustState::Pending).then(|| now.clone())],
        )?;
        insert_command(
            &transaction,
            "native-trust.request",
            &input.idempotency_key,
            &input.request_id,
            id,
            &now,
        )?;
        let trust = required_trust(&transaction, id)?;
        let event_sequence = employee::insert_event(
            &transaction,
            "native-trust.requested",
            &json!({"trust": &trust}),
            &now,
        )?;
        transaction.commit()?;
        Ok(NativeTrustMutationResult {
            trust,
            event_sequence: Some(event_sequence),
        })
    }

    pub fn resolve_native_trust(
        &self,
        input: &ResolveNativeTrustInput,
    ) -> Result<NativeTrustMutationResult, StoreError> {
        validate_message_command_key(&input.idempotency_key).map_err(|error| {
            StoreError::NativeTrustValidation {
                field: "idempotency key",
                message: error.to_string(),
            }
        })?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) =
            trust_for_command(&transaction, "native-trust.resolve", &input.idempotency_key)?
        {
            if existing.id != input.trust_id {
                return Err(StoreError::NativeTrustIdempotencyConflict);
            }
            transaction.commit()?;
            return Ok(NativeTrustMutationResult {
                trust: existing,
                event_sequence: None,
            });
        }
        let current = required_trust(&transaction, input.trust_id)?;
        if current.revision != input.expected_revision {
            return Err(StoreError::NativeTrustStale {
                trust_id: current.id,
                current_revision: current.revision,
            });
        }
        if current.state != NativeTrustState::Pending {
            return Err(StoreError::NativeTrustState {
                trust_id: current.id,
                state: name(current.state)?,
                expected: "pending",
            });
        }
        let state = match input.decision {
            NativeTrustDecision::Approve => NativeTrustState::Approved,
            NativeTrustDecision::Decline => NativeTrustState::Declined,
        };
        let now = Utc::now().to_rfc3339();
        transaction.execute("UPDATE native_workspace_trust_requests SET state = ?2, decider = 'user', revision = revision + 1, notification_state = 'settled', notification_claim_id = NULL, updated_at = ?3, decided_at = ?3 WHERE id = ?1", params![current.id.to_string(), name(state)?, now])?;
        if input.decision == NativeTrustDecision::Approve && input.auto_accept_managed_next_time {
            transaction.execute("INSERT INTO native_workspace_trust_policies (provider, auto_accept_managed_next_time, revision, updated_at) VALUES (?1, 1, 1, ?2) ON CONFLICT(provider) DO UPDATE SET auto_accept_managed_next_time = 1, revision = revision + 1, updated_at = excluded.updated_at", params![current.provider, now])?;
        }
        insert_command(
            &transaction,
            "native-trust.resolve",
            &input.idempotency_key,
            &input.request_id,
            current.id,
            &now,
        )?;
        let trust = required_trust(&transaction, current.id)?;
        let event_sequence = employee::insert_event(
            &transaction,
            "native-trust.decided",
            &json!({"trust": &trust}),
            &now,
        )?;
        transaction.commit()?;
        Ok(NativeTrustMutationResult {
            trust,
            event_sequence: Some(event_sequence),
        })
    }

    pub fn cancel_native_trust(
        &self,
        input: &CancelNativeTrustInput,
    ) -> Result<NativeTrustMutationResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = required_trust(&transaction, input.trust_id)?;
        if current.revision != input.expected_revision {
            return Err(StoreError::NativeTrustStale {
                trust_id: current.id,
                current_revision: current.revision,
            });
        }
        if current.state != NativeTrustState::Pending {
            return Err(StoreError::NativeTrustState {
                trust_id: current.id,
                state: name(current.state)?,
                expected: "pending",
            });
        }
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE native_workspace_trust_requests SET state = 'cancelled', decider = 'system', revision = revision + 1, notification_state = 'settled', notification_claim_id = NULL, updated_at = ?2, decided_at = ?2 WHERE id = ?1",
            params![current.id.to_string(), now],
        )?;
        let trust = required_trust(&transaction, current.id)?;
        let event_sequence = employee::insert_event(
            &transaction,
            "native-trust.cancelled",
            &json!({"trust": &trust}),
            &now,
        )?;
        transaction.commit()?;
        Ok(NativeTrustMutationResult {
            trust,
            event_sequence: Some(event_sequence),
        })
    }

    pub fn get_native_trust(&self, trust_id: Uuid) -> Result<NativeTrustRequestRecord, StoreError> {
        let connection = self.lock()?;
        required_trust(&connection, trust_id)
    }
    pub fn get_native_trust_policy(&self, provider: &str) -> Result<NativeTrustPolicy, StoreError> {
        validate_provider(provider)?;
        let connection = self.lock()?;
        policy_for_provider(&connection, provider)
    }
    pub fn set_native_trust_policy(
        &self,
        provider: &str,
        expected_revision: u64,
        auto_accept_managed_next_time: bool,
    ) -> Result<NativeTrustPolicy, StoreError> {
        validate_provider(provider)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = policy_for_provider(&transaction, provider)?;
        if current.revision != expected_revision {
            return Err(StoreError::NativeTrustPolicyStale {
                provider: provider.to_owned(),
                current_revision: current.revision,
            });
        }
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "INSERT INTO native_workspace_trust_policies (provider, auto_accept_managed_next_time, revision, updated_at) VALUES (?1, ?2, 1, ?3) ON CONFLICT(provider) DO UPDATE SET auto_accept_managed_next_time = excluded.auto_accept_managed_next_time, revision = revision + 1, updated_at = excluded.updated_at",
            params![provider, i64::from(auto_accept_managed_next_time), now],
        )?;
        let policy = policy_for_provider(&transaction, provider)?;
        transaction.commit()?;
        Ok(policy)
    }
    pub fn reset_native_trust_policy(
        &self,
        provider: &str,
        expected_revision: u64,
    ) -> Result<NativeTrustPolicy, StoreError> {
        validate_provider(provider)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = policy_for_provider(&transaction, provider)?;
        if current.revision != expected_revision {
            return Err(StoreError::NativeTrustPolicyStale {
                provider: provider.to_owned(),
                current_revision: current.revision,
            });
        }
        let now = Utc::now().to_rfc3339();
        transaction.execute("INSERT INTO native_workspace_trust_policies (provider, auto_accept_managed_next_time, revision, updated_at) VALUES (?1, 0, 1, ?2) ON CONFLICT(provider) DO UPDATE SET auto_accept_managed_next_time = 0, revision = revision + 1, updated_at = excluded.updated_at", params![provider, now])?;
        let policy = policy_for_provider(&transaction, provider)?;
        transaction.commit()?;
        Ok(policy)
    }
    pub fn claim_native_trust_notification(
        &self,
        input: &ClaimNativeTrustNotificationInput,
    ) -> Result<Option<NativeTrustRequestRecord>, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().to_rfc3339();
        let changed = transaction.execute("UPDATE native_workspace_trust_requests SET notification_state = 'claimed', notification_claim_id = ?2, revision = revision + 1, updated_at = ?3 WHERE id = ?1 AND state = 'pending' AND notification_state = 'pending'", params![input.trust_id.to_string(), input.claimant_id.to_string(), now])?;
        let trust = if changed == 1 {
            Some(required_trust(&transaction, input.trust_id)?)
        } else {
            None
        };
        transaction.commit()?;
        Ok(trust)
    }
    pub fn settle_native_trust_notification(
        &self,
        trust_id: Uuid,
        claim_id: Uuid,
    ) -> Result<(), StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().to_rfc3339();
        let changed = transaction.execute("UPDATE native_workspace_trust_requests SET notification_state = 'settled', notification_claim_id = NULL, revision = revision + 1, updated_at = ?3 WHERE id = ?1 AND state = 'pending' AND notification_state = 'claimed' AND notification_claim_id = ?2", params![trust_id.to_string(), claim_id.to_string(), now])?;
        if changed == 0 {
            let current = required_trust(&transaction, trust_id)?;
            if current.state != NativeTrustState::Pending {
                return Err(StoreError::NativeTrustState {
                    trust_id,
                    state: name(current.state)?,
                    expected: "pending",
                });
            }
        }
        transaction.commit()?;
        Ok(())
    }
    pub fn reconcile_native_trust(
        &self,
        owner_boot_id: Uuid,
    ) -> Result<NativeTrustReconcileResult, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().to_rfc3339();
        let ids: Vec<String> = transaction.prepare("SELECT id FROM native_workspace_trust_requests WHERE state = 'pending' AND owner_boot_id <> ?1 ORDER BY sequence")?.query_map([owner_boot_id.to_string()], |row| row.get(0))?.collect::<Result<_, _>>()?;
        let mut trusts = Vec::with_capacity(ids.len());
        for id in ids {
            transaction.execute("UPDATE native_workspace_trust_requests SET state = 'cancelled', decider = 'system', revision = revision + 1, notification_state = 'settled', notification_claim_id = NULL, updated_at = ?2, decided_at = ?2 WHERE id = ?1", params![id, now])?;
            trusts.push(required_trust(
                &transaction,
                Uuid::parse_str(&id).map_err(|error| StoreError::InvalidRecord {
                    field: "native trust id",
                    message: error.to_string(),
                })?,
            )?);
        }
        let event_sequence = if trusts.is_empty() {
            None
        } else {
            Some(employee::insert_event(
                &transaction,
                "native-trust.reconciled",
                &json!({"trusts": &trusts}),
                &now,
            )?)
        };
        transaction.commit()?;
        Ok(NativeTrustReconcileResult {
            trusts,
            event_sequence,
        })
    }

    pub fn list_native_trust(
        &self,
        input: &NativeTrustListParams,
    ) -> Result<Vec<NativeTrustRequestRecord>, StoreError> {
        if input.limit == 0 || input.limit > 500 {
            return Err(StoreError::NativeTrustValidation {
                field: "list limit",
                message: "must be between 1 and 500".to_owned(),
            });
        }
        let connection = self.lock()?;
        let state = input.state.map(name).transpose()?;
        let mut statement = connection.prepare(&format!(
            "SELECT {TRUST_COLUMNS} FROM native_workspace_trust_requests WHERE sequence > ?1 AND (?3 IS NULL OR state = ?3) ORDER BY sequence LIMIT ?2"
        ))?;
        let rows = statement.query_map(
            params![
                i64::try_from(input.after_sequence).map_err(|_| StoreError::NegativeSequence)?,
                i64::from(input.limit),
                state,
            ],
            raw_trust,
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn pending_native_trust(
        &self,
        limit: u32,
    ) -> Result<Vec<NativeTrustRequestRecord>, StoreError> {
        self.list_native_trust(&NativeTrustListParams {
            after_sequence: 0,
            limit,
            state: Some(NativeTrustState::Pending),
        })
    }
}

fn validate_input(input: &RequestNativeTrustInput) -> Result<(), StoreError> {
    validate_provider(&input.provider)?;
    bounded("workspace path", &input.canonical_workspace_path, 32_768)?;
    validate_message_command_key(&input.idempotency_key).map_err(|error| {
        StoreError::NativeTrustValidation {
            field: "idempotency key",
            message: error.to_string(),
        }
    })
}
fn validate_provider(provider: &str) -> Result<(), StoreError> {
    bounded("provider", provider, 256)
}
fn bounded(field: &'static str, value: &str, maximum: usize) -> Result<(), StoreError> {
    if value.is_empty() || value.len() > maximum {
        Err(StoreError::NativeTrustValidation {
            field,
            message: format!("must be between 1 and {maximum} bytes"),
        })
    } else {
        Ok(())
    }
}
fn name(value: impl serde::Serialize) -> Result<String, StoreError> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| StoreError::InvalidRecord {
            field: "native trust enum",
            message: "enum did not serialize to a string".to_owned(),
        })
}
fn insert_command(
    connection: &rusqlite::Transaction<'_>,
    scope: &str,
    key: &str,
    request_id: &str,
    trust_id: Uuid,
    now: &str,
) -> Result<(), StoreError> {
    connection.execute("INSERT INTO native_workspace_trust_commands (scope, idempotency_key, request_id, trust_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)", params![scope, key, request_id, trust_id.to_string(), now])?;
    Ok(())
}
fn trust_for_command(
    connection: &Connection,
    scope: &str,
    key: &str,
) -> Result<Option<NativeTrustRequestRecord>, StoreError> {
    connection.query_row("SELECT trust_id FROM native_workspace_trust_commands WHERE scope = ?1 AND idempotency_key = ?2", params![scope, key], |row| row.get::<_, String>(0)).optional()?.map(|id| required_trust(connection, Uuid::parse_str(&id).map_err(|error| StoreError::InvalidRecord { field: "native trust command id", message: error.to_string() })?)).transpose()
}
fn policy_for_provider(
    connection: &Connection,
    provider: &str,
) -> Result<NativeTrustPolicy, StoreError> {
    let policy = connection.query_row("SELECT provider, auto_accept_managed_next_time, revision, updated_at FROM native_workspace_trust_policies WHERE provider = ?1", [provider], |row| Ok(NativeTrustPolicy { provider: row.get(0)?, auto_accept_managed_next_time: row.get::<_, i64>(1)? != 0, revision: u64::try_from(row.get::<_, i64>(2)?).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, 0))?, updated_at: row.get(3)? })).optional()?;
    Ok(policy.unwrap_or_else(|| NativeTrustPolicy {
        provider: provider.to_owned(),
        auto_accept_managed_next_time: false,
        revision: 0,
        updated_at: String::new(),
    }))
}
fn required_trust(
    connection: &Connection,
    trust_id: Uuid,
) -> Result<NativeTrustRequestRecord, StoreError> {
    connection
        .query_row(
            &format!("SELECT {TRUST_COLUMNS} FROM native_workspace_trust_requests WHERE id = ?1"),
            [trust_id.to_string()],
            raw_trust,
        )
        .optional()?
        .ok_or(StoreError::NativeTrustNotFound { trust_id })
}
fn raw_trust(row: &rusqlite::Row<'_>) -> rusqlite::Result<NativeTrustRequestRecord> {
    let uuid = |index| -> rusqlite::Result<Uuid> {
        let value: String = row.get(index)?;
        Uuid::parse_str(&value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    };
    let parse = |index| -> rusqlite::Result<_> {
        let value: String = row.get(index)?;
        serde_json::from_value(serde_json::Value::String(value)).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    };
    Ok(NativeTrustRequestRecord {
        sequence: u64::try_from(row.get::<_, i64>(0)?)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, 0))?,
        version: NATIVE_TRUST_CONTRACT_V1,
        id: uuid(1)?,
        provider: row.get(2)?,
        canonical_workspace_path: row.get(3)?,
        managed_workspace: row.get::<_, i64>(4)? != 0,
        session_id: uuid(5)?,
        employee_id: uuid(6)?,
        owner_boot_id: uuid(7)?,
        state: parse(8)?,
        decider: row
            .get::<_, Option<String>>(9)?
            .map(|value| {
                serde_json::from_value(serde_json::Value::String(value)).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        9,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .transpose()?,
        revision: u64::try_from(row.get::<_, i64>(10)?)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(10, 0))?,
        notification_state: serde_json::from_value(serde_json::Value::String(row.get(11)?))
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    11,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        notification_claim_id: row
            .get::<_, Option<String>>(12)?
            .map(|value| {
                Uuid::parse_str(&value).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        12,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .transpose()?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        decided_at: row.get(15)?,
    })
}
