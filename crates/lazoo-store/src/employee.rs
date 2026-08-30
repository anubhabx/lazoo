use chrono::Utc;
use lazoo_protocol::{
    AuthorityMode, EmployeeAvailability, EmployeeCheckoutPolicy, EmployeeMutationResult,
    EmployeeRecord, ProviderObservation, RosterEntry, RosterSnapshot,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Store, StoreError};

#[derive(Clone, Debug)]
pub struct CreateEmployeeInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub name: String,
    pub provider: String,
    pub role: String,
    pub instructions: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub native_settings: Value,
    pub authority_mode: AuthorityMode,
    pub checkout_policy: EmployeeCheckoutPolicy,
    pub enabled: bool,
    pub provider_observation: Option<ProviderObservation>,
}

#[derive(Clone, Debug)]
pub struct UpdateEmployeeInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub employee_id: Uuid,
    pub expected_revision: u64,
    pub name: Option<String>,
    pub provider: Option<String>,
    pub role: Option<String>,
    pub instructions: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub native_settings: Option<Value>,
    pub authority_mode: Option<AuthorityMode>,
    pub checkout_policy: Option<EmployeeCheckoutPolicy>,
    pub enabled: Option<bool>,
    pub provider_observation: Option<ProviderObservation>,
}

#[derive(Clone, Debug)]
pub struct DisableEmployeeInput {
    pub request_id: String,
    pub idempotency_key: String,
    pub employee_id: Uuid,
    pub expected_revision: u64,
}

impl Store {
    pub fn create_employee(
        &self,
        input: &CreateEmployeeInput,
    ) -> Result<EmployeeMutationResult, StoreError> {
        let mut connection = self.lock()?;
        if let Some(response) =
            processed_response(&connection, "employee.create", &input.idempotency_key)?
        {
            return Ok(serde_json::from_str(&response)?);
        }
        if let Some(employee_id) = connection
            .query_row(
                "SELECT id FROM employees WHERE name = ?1 COLLATE NOCASE",
                [&input.name],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return Err(StoreError::EmployeeConflict {
                employee_id: parse_uuid(&employee_id, "employees.id")?,
            });
        }

        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let employee_id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        if let Some(observation) = &input.provider_observation {
            upsert_provider_observation(&transaction, observation)?;
        }
        transaction.execute(
            "INSERT INTO employees (
                id, name, provider, role, instructions, model, reasoning_effort,
                native_settings_json, authority_mode, checkout_policy, enabled,
                configuration_revision, created_at, updated_at, disabled_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, ?12, ?13)",
            params![
                employee_id.to_string(),
                input.name,
                input.provider,
                input.role,
                input.instructions,
                input.model,
                input.reasoning_effort,
                serde_json::to_string(&input.native_settings)?,
                serde_json::to_string(&input.authority_mode)?,
                serde_json::to_string(&input.checkout_policy)?,
                i64::from(input.enabled),
                now,
                (!input.enabled).then_some(now.clone()),
            ],
        )?;
        let roster_revision = advance_roster(&transaction, &now)?;
        let employee = query_employee(&transaction, employee_id)?
            .ok_or(StoreError::EmployeeNotFound { employee_id })?;
        let event_sequence = insert_event(
            &transaction,
            "employee.created",
            &json!({"employee": employee, "rosterRevision": roster_revision}),
            &now,
        )?;
        let result = EmployeeMutationResult {
            employee,
            roster_revision,
            event_sequence: Some(event_sequence),
        };
        insert_processed_response(
            &transaction,
            "employee.create",
            &input.idempotency_key,
            &input.request_id,
            &result,
            &now,
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn list_employees(&self) -> Result<Vec<EmployeeRecord>, StoreError> {
        let connection = self.lock()?;
        query_employees(&connection)
    }

    pub fn update_employee(
        &self,
        input: &UpdateEmployeeInput,
    ) -> Result<EmployeeMutationResult, StoreError> {
        let mut connection = self.lock()?;
        if let Some(response) =
            processed_response(&connection, "employee.update", &input.idempotency_key)?
        {
            return Ok(serde_json::from_str(&response)?);
        }
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = query_employee(&transaction, input.employee_id)?.ok_or(
            StoreError::EmployeeNotFound {
                employee_id: input.employee_id,
            },
        )?;
        require_revision(&current, input.expected_revision)?;

        let mut desired = current.clone();
        if let Some(name) = &input.name {
            desired.name.clone_from(name);
        }
        if let Some(provider) = &input.provider {
            desired.provider.clone_from(provider);
        }
        if let Some(role) = &input.role {
            desired.role.clone_from(role);
        }
        if let Some(instructions) = &input.instructions {
            desired.instructions.clone_from(instructions);
        }
        if let Some(model) = &input.model {
            desired.model = Some(model.clone());
        }
        if let Some(reasoning_effort) = &input.reasoning_effort {
            desired.reasoning_effort = Some(reasoning_effort.clone());
        }
        if let Some(native_settings) = &input.native_settings {
            desired.native_settings.clone_from(native_settings);
        }
        if let Some(authority_mode) = input.authority_mode {
            desired.authority_mode = authority_mode;
        }
        if let Some(checkout_policy) = input.checkout_policy {
            desired.checkout_policy = checkout_policy;
        }
        if let Some(enabled) = input.enabled {
            desired.enabled = enabled;
        }

        if desired.name.to_lowercase() != current.name.to_lowercase()
            && let Some(employee_id) = transaction
                .query_row(
                    "SELECT id FROM employees WHERE name = ?1 COLLATE NOCASE AND id <> ?2",
                    params![desired.name, input.employee_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
        {
            return Err(StoreError::EmployeeConflict {
                employee_id: parse_uuid(&employee_id, "employees.id")?,
            });
        }

        let now = Utc::now().to_rfc3339();
        if !configuration_changed(&current, &desired) {
            let roster_revision = roster_revision(&transaction)?;
            let result = EmployeeMutationResult {
                employee: current,
                roster_revision,
                event_sequence: None,
            };
            insert_processed_response(
                &transaction,
                "employee.update",
                &input.idempotency_key,
                &input.request_id,
                &result,
                &now,
            )?;
            transaction.commit()?;
            return Ok(result);
        }

        desired.configuration_revision = current.configuration_revision.saturating_add(1);
        desired.updated_at.clone_from(&now);
        desired.disabled_at = match (current.enabled, desired.enabled) {
            (true, false) => Some(now.clone()),
            (false, true) => None,
            _ => current.disabled_at.clone(),
        };
        if let Some(observation) = &input.provider_observation {
            upsert_provider_observation(&transaction, observation)?;
        }
        write_employee(&transaction, &desired)?;
        let roster_revision = advance_roster(&transaction, &now)?;
        let employee = query_employee(&transaction, input.employee_id)?.ok_or(
            StoreError::EmployeeNotFound {
                employee_id: input.employee_id,
            },
        )?;
        let event_sequence = insert_event(
            &transaction,
            "employee.updated",
            &json!({"employee": employee, "rosterRevision": roster_revision}),
            &now,
        )?;
        let result = EmployeeMutationResult {
            employee,
            roster_revision,
            event_sequence: Some(event_sequence),
        };
        insert_processed_response(
            &transaction,
            "employee.update",
            &input.idempotency_key,
            &input.request_id,
            &result,
            &now,
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn disable_employee(
        &self,
        input: &DisableEmployeeInput,
    ) -> Result<EmployeeMutationResult, StoreError> {
        let mut connection = self.lock()?;
        if let Some(response) =
            processed_response(&connection, "employee.disable", &input.idempotency_key)?
        {
            return Ok(serde_json::from_str(&response)?);
        }
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut employee = query_employee(&transaction, input.employee_id)?.ok_or(
            StoreError::EmployeeNotFound {
                employee_id: input.employee_id,
            },
        )?;
        require_revision(&employee, input.expected_revision)?;
        let now = Utc::now().to_rfc3339();
        if !employee.enabled {
            let result = EmployeeMutationResult {
                roster_revision: roster_revision(&transaction)?,
                employee,
                event_sequence: None,
            };
            insert_processed_response(
                &transaction,
                "employee.disable",
                &input.idempotency_key,
                &input.request_id,
                &result,
                &now,
            )?;
            transaction.commit()?;
            return Ok(result);
        }

        employee.enabled = false;
        employee.configuration_revision = employee.configuration_revision.saturating_add(1);
        employee.updated_at.clone_from(&now);
        employee.disabled_at = Some(now.clone());
        write_employee(&transaction, &employee)?;
        let roster_revision = advance_roster(&transaction, &now)?;
        let event_sequence = insert_event(
            &transaction,
            "employee.disabled",
            &json!({"employee": employee, "rosterRevision": roster_revision}),
            &now,
        )?;
        let result = EmployeeMutationResult {
            employee,
            roster_revision,
            event_sequence: Some(event_sequence),
        };
        insert_processed_response(
            &transaction,
            "employee.disable",
            &input.idempotency_key,
            &input.request_id,
            &result,
            &now,
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn get_employee(&self, employee_id: Uuid) -> Result<EmployeeRecord, StoreError> {
        let connection = self.lock()?;
        query_employee(&connection, employee_id)?
            .ok_or(StoreError::EmployeeNotFound { employee_id })
    }

    pub fn roster_snapshot(&self) -> Result<RosterSnapshot, StoreError> {
        let connection = self.lock()?;
        let (revision, updated_at) = connection.query_row(
            "SELECT revision, updated_at FROM roster_state WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )?;
        let employees = query_employees(&connection)?;
        let entries = employees
            .into_iter()
            .map(|employee| {
                let observation = query_provider_observation(&connection, &employee.provider)?;
                let availability = if employee.enabled {
                    EmployeeAvailability::Offline
                } else {
                    EmployeeAvailability::Disabled
                };
                Ok(RosterEntry {
                    employee,
                    availability,
                    availability_reason: Some(match availability {
                        EmployeeAvailability::Disabled => "employee is disabled".to_owned(),
                        _ => "provider availability has not been derived".to_owned(),
                    }),
                    provider_observation: observation,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(RosterSnapshot {
            revision: u64::try_from(revision).map_err(|_| StoreError::NegativeSequence)?,
            entries,
            updated_at,
        })
    }
}

fn processed_response(
    connection: &Connection,
    scope: &str,
    idempotency_key: &str,
) -> Result<Option<String>, StoreError> {
    connection
        .query_row(
            "SELECT response_json FROM processed_commands WHERE scope = ?1 AND idempotency_key = ?2",
            params![scope, idempotency_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(StoreError::from)
}

fn insert_processed_response(
    transaction: &Transaction<'_>,
    scope: &str,
    idempotency_key: &str,
    request_id: &str,
    response: &impl serde::Serialize,
    now: &str,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO processed_commands (scope, idempotency_key, request_id, response_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            scope,
            idempotency_key,
            request_id,
            serde_json::to_string(response)?,
            now
        ],
    )?;
    Ok(())
}

fn upsert_provider_observation(
    transaction: &Transaction<'_>,
    observation: &ProviderObservation,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO provider_observations (
            provider, verification, version, protocol, capabilities_json,
            authentication_json, degradation_reason, checked_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(provider) DO UPDATE SET
            verification = excluded.verification,
            version = excluded.version,
            protocol = excluded.protocol,
            capabilities_json = excluded.capabilities_json,
            authentication_json = excluded.authentication_json,
            degradation_reason = excluded.degradation_reason,
            checked_at = excluded.checked_at",
        params![
            observation.provider,
            observation.verification,
            observation.version,
            observation.protocol,
            serde_json::to_string(&observation.capabilities)?,
            serde_json::to_string(&observation.authentication)?,
            observation.degradation_reason,
            observation.checked_at,
        ],
    )?;
    Ok(())
}

fn advance_roster(transaction: &Transaction<'_>, now: &str) -> Result<u64, StoreError> {
    transaction.execute(
        "UPDATE roster_state SET revision = revision + 1, updated_at = ?1 WHERE singleton = 1",
        [now],
    )?;
    let revision: i64 = transaction.query_row(
        "SELECT revision FROM roster_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    u64::try_from(revision).map_err(|_| StoreError::NegativeSequence)
}

fn roster_revision(connection: &Connection) -> Result<u64, StoreError> {
    let revision: i64 = connection.query_row(
        "SELECT revision FROM roster_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    u64::try_from(revision).map_err(|_| StoreError::NegativeSequence)
}

fn require_revision(employee: &EmployeeRecord, expected_revision: u64) -> Result<(), StoreError> {
    if employee.configuration_revision == expected_revision {
        Ok(())
    } else {
        Err(StoreError::EmployeeStale {
            employee_id: employee.id,
            current_revision: employee.configuration_revision,
        })
    }
}

fn configuration_changed(current: &EmployeeRecord, desired: &EmployeeRecord) -> bool {
    current.name != desired.name
        || current.provider != desired.provider
        || current.role != desired.role
        || current.instructions != desired.instructions
        || current.model != desired.model
        || current.reasoning_effort != desired.reasoning_effort
        || current.native_settings != desired.native_settings
        || current.authority_mode != desired.authority_mode
        || current.checkout_policy != desired.checkout_policy
        || current.enabled != desired.enabled
}

fn write_employee(
    transaction: &Transaction<'_>,
    employee: &EmployeeRecord,
) -> Result<(), StoreError> {
    transaction.execute(
        "UPDATE employees SET
            name = ?2, provider = ?3, role = ?4, instructions = ?5, model = ?6,
            reasoning_effort = ?7, native_settings_json = ?8, authority_mode = ?9,
            checkout_policy = ?10, enabled = ?11, configuration_revision = ?12,
            updated_at = ?13, disabled_at = ?14
         WHERE id = ?1",
        params![
            employee.id.to_string(),
            employee.name,
            employee.provider,
            employee.role,
            employee.instructions,
            employee.model,
            employee.reasoning_effort,
            serde_json::to_string(&employee.native_settings)?,
            serde_json::to_string(&employee.authority_mode)?,
            serde_json::to_string(&employee.checkout_policy)?,
            i64::from(employee.enabled),
            i64::try_from(employee.configuration_revision)
                .map_err(|_| StoreError::NegativeSequence)?,
            employee.updated_at,
            employee.disabled_at,
        ],
    )?;
    Ok(())
}

fn insert_event(
    transaction: &Transaction<'_>,
    event_type: &str,
    payload: &Value,
    now: &str,
) -> Result<u64, StoreError> {
    let event_id = Uuid::new_v4();
    transaction.execute(
        "INSERT INTO audit_events (id, event_type, payload_json, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            event_id.to_string(),
            event_type,
            serde_json::to_string(payload)?,
            now
        ],
    )?;
    let event_sequence =
        u64::try_from(transaction.last_insert_rowid()).map_err(|_| StoreError::NegativeSequence)?;
    transaction.execute(
        "INSERT INTO outbox (id, audit_sequence, created_at) VALUES (?1, ?2, ?3)",
        params![
            Uuid::new_v4().to_string(),
            i64::try_from(event_sequence).map_err(|_| StoreError::NegativeSequence)?,
            now
        ],
    )?;
    Ok(event_sequence)
}

fn query_employees(connection: &Connection) -> Result<Vec<EmployeeRecord>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT id, name, provider, role, instructions, model, reasoning_effort,
                native_settings_json, authority_mode, checkout_policy, enabled,
                configuration_revision, created_at, updated_at, disabled_at
         FROM employees ORDER BY created_at, id",
    )?;
    let rows = statement.query_map([], raw_employee)?;
    rows.map(|row| employee_from_raw(row?)).collect()
}

fn query_employee(
    connection: &Connection,
    employee_id: Uuid,
) -> Result<Option<EmployeeRecord>, StoreError> {
    connection
        .query_row(
            "SELECT id, name, provider, role, instructions, model, reasoning_effort,
                    native_settings_json, authority_mode, checkout_policy, enabled,
                    configuration_revision, created_at, updated_at, disabled_at
             FROM employees WHERE id = ?1",
            [employee_id.to_string()],
            raw_employee,
        )
        .optional()?
        .map(employee_from_raw)
        .transpose()
}

fn query_provider_observation(
    connection: &Connection,
    provider: &str,
) -> Result<Option<ProviderObservation>, StoreError> {
    connection
        .query_row(
            "SELECT provider, verification, version, protocol, capabilities_json,
                    authentication_json, degradation_reason, checked_at
             FROM provider_observations WHERE provider = ?1",
            [provider],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?
        .map(|row| {
            Ok(ProviderObservation {
                provider: row.0,
                verification: row.1,
                version: row.2,
                protocol: row.3,
                capabilities: serde_json::from_str(&row.4)?,
                authentication: serde_json::from_str(&row.5)?,
                degradation_reason: row.6,
                checked_at: row.7,
            })
        })
        .transpose()
}

type RawEmployee = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    i64,
    i64,
    String,
    String,
    Option<String>,
);

fn raw_employee(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawEmployee> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
    ))
}

fn employee_from_raw(row: RawEmployee) -> Result<EmployeeRecord, StoreError> {
    Ok(EmployeeRecord {
        id: parse_uuid(&row.0, "employees.id")?,
        name: row.1,
        provider: row.2,
        role: row.3,
        instructions: row.4,
        model: row.5,
        reasoning_effort: row.6,
        native_settings: serde_json::from_str(&row.7)?,
        authority_mode: serde_json::from_str(&row.8)?,
        checkout_policy: serde_json::from_str(&row.9)?,
        enabled: row.10 == 1,
        configuration_revision: u64::try_from(row.11).map_err(|_| StoreError::NegativeSequence)?,
        created_at: row.12,
        updated_at: row.13,
        disabled_at: row.14,
    })
}

fn parse_uuid(value: &str, field: &'static str) -> Result<Uuid, StoreError> {
    Uuid::parse_str(value).map_err(|error| StoreError::InvalidRecord {
        field,
        message: error.to_string(),
    })
}
