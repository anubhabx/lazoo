//! Desired skill ownership and durable extension operation intents; native plugin state is never truth here.
use crate::{Store, StoreError, employee::insert_event};
use chrono::Utc;
use rusqlite::{TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionState {
    pub revision: u64,
    pub state: Value,
    pub pending: Option<Value>,
}

impl Store {
    pub fn extension_state(&self) -> Result<ExtensionState, StoreError> {
        let connection = self.lock()?;
        let (revision, state, pending): (i64, String, Option<String>) = connection.query_row(
            "SELECT revision,state_json,pending_json FROM extension_state WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        Ok(ExtensionState {
            revision: u64::try_from(revision).map_err(|_| StoreError::NegativeSequence)?,
            state: serde_json::from_str(&state)?,
            pending: pending.map(|s| serde_json::from_str(&s)).transpose()?,
        })
    }

    pub fn begin_extension_operation(
        &self,
        revision: u64,
        intent: &Value,
    ) -> Result<(), StoreError> {
        let mut connection = self.lock()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if tx.execute("UPDATE extension_state SET pending_json=?1 WHERE singleton=1 AND revision=?2 AND pending_json IS NULL", params![serde_json::to_string(intent)?, i64::try_from(revision).map_err(|_| StoreError::NegativeSequence)?])? != 1 {
            return Err(StoreError::InvalidRecord { field: "extension revision", message: "revision changed or an interrupted operation requires recovery".into() });
        }
        insert_event(
            &tx,
            "extension.operation.started",
            intent,
            &Utc::now().to_rfc3339(),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn finish_extension_operation(
        &self,
        revision: u64,
        state: &Value,
        outcome: &Value,
        recovery_needed: bool,
    ) -> Result<u64, StoreError> {
        let mut connection = self.lock()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if tx.execute("UPDATE extension_state SET revision=revision+1,state_json=?1,pending_json=?2 WHERE singleton=1 AND revision=?3 AND pending_json IS NOT NULL", params![serde_json::to_string(state)?, if recovery_needed { Some(serde_json::to_string(outcome)?) } else { None }, i64::try_from(revision).map_err(|_| StoreError::NegativeSequence)?])? != 1 {
            return Err(StoreError::InvalidRecord { field: "extension intent", message: "operation no longer owns the revision".into() });
        }
        insert_event(
            &tx,
            "extension.operation.finished",
            &json!({"revision": revision + 1, "outcome":outcome,"recoveryNeeded":recovery_needed}),
            &Utc::now().to_rfc3339(),
        )?;
        let event = outcome
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("extension");
        insert_event(
            &tx,
            if event == "skill" {
                "skill.changed"
            } else {
                "plugin.changed"
            },
            &json!({"revision":revision+1,"operationId":outcome["operationId"]}),
            &Utc::now().to_rfc3339(),
        )?;
        tx.commit()?;
        Ok(revision + 1)
    }
}
