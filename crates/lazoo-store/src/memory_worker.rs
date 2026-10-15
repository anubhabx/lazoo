//! Durable, opt-in settings and attempt links for the memory summary worker.
//!
//! The app worker owns dispatch.  This store layer never starts a provider: it only preserves the
//! selected employee/model/rate and the one auditable native turn linked to a claimed job.

use chrono::{Duration, Utc};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use uuid::Uuid;

use crate::{Store, StoreError};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryWorkerSettings {
    pub project_id: Option<Uuid>,
    pub revision: u64,
    pub employee_id: Uuid,
    pub model: String,
    pub requests_per_minute: u32,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct SetMemoryWorkerSettingsInput {
    pub project_id: Option<Uuid>,
    pub expected_revision: Option<u64>,
    pub employee_id: Uuid,
    pub model: String,
    pub requests_per_minute: u32,
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemorySummaryAttempt {
    pub job_id: Uuid,
    pub attempt: u32,
    pub lease_token: Uuid,
    pub conversation_id: Uuid,
    pub turn_id: Uuid,
    pub state: String,
}

impl Store {
    /// Startup recovery deliberately terminalizes linked attempts. Their native state is unknown,
    /// so returning them to pending could submit the same private source twice.
    pub fn recover_linked_memory_summary_attempts(&self) -> Result<u64, StoreError> {
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().to_rfc3339();
        let count = tx.execute("UPDATE memory_summary_jobs SET state='failed',lease_owner_boot_id=NULL,failure_message='internal summary attempt requires review after restart',updated_at=?1 WHERE id IN (SELECT job_id FROM memory_summary_attempts WHERE state IN ('submitted','uncertain')) AND state IN ('claimed','pending')", [&now])?;
        tx.execute("UPDATE memory_summary_attempts SET state='uncertain',finished_at=COALESCE(finished_at,?1) WHERE state='submitted'", [&now])?;
        tx.commit()?;
        Ok(count as u64)
    }
    pub fn settle_memory_summary_attempt_failure(
        &self,
        job_id: Uuid,
        owner: Uuid,
        expected_attempt: u32,
        message: &str,
        uncertain: bool,
    ) -> Result<(), StoreError> {
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().to_rfc3339();
        let valid: i64 = tx.query_row("SELECT COUNT(*) FROM memory_summary_jobs WHERE id=?1 AND state='claimed' AND lease_owner_boot_id=?2 AND attempts=?3 AND lease_expires_at>?4", params![job_id.to_string(),owner.to_string(),expected_attempt,now], |r| r.get(0))?;
        if valid != 1 {
            return Err(StoreError::InvalidRecord {
                field: "memory worker lease",
                message: "attempt is no longer owned".to_owned(),
            });
        }
        let state = if uncertain { "uncertain" } else { "failed" };
        tx.execute(
            "UPDATE memory_summary_attempts SET state=?2,finished_at=?3 WHERE job_id=?1",
            params![job_id.to_string(), state, now],
        )?;
        tx.execute("UPDATE memory_summary_jobs SET state='failed',lease_owner_boot_id=NULL,failure_message=?2,updated_at=?3 WHERE id=?1", params![job_id.to_string(),message.chars().take(512).collect::<String>(),now])?;
        tx.commit()?;
        Ok(())
    }
    pub fn is_memory_summary_turn(&self, turn_id: Uuid) -> Result<bool, StoreError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_summary_attempts WHERE turn_id=?1)",
                [turn_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }

    pub fn is_memory_summary_session(&self, session_id: Uuid) -> Result<bool, StoreError> {
        self.lock()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_summary_attempts WHERE session_id=?1)",
                [session_id.to_string()],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }

    pub fn memory_summary_session(&self, turn_id: Uuid) -> Result<Option<Uuid>, StoreError> {
        let value: Option<String> = self
            .lock()?
            .query_row(
                "SELECT session_id FROM memory_summary_attempts WHERE turn_id=?1",
                [turn_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        value
            .as_deref()
            .map(parse_id)
            .transpose()
            .map_err(StoreError::from)
    }

    /// Reserves the private native context while the session is still Starting. No ordinary turn
    /// may observe a ready, unreserved summary session between activation and turn binding.
    pub fn reserve_memory_summary_session(
        &self,
        turn_id: Uuid,
        session_id: Uuid,
        owner: Uuid,
    ) -> Result<(), StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE memory_summary_attempts SET session_id=?2 WHERE turn_id=?1 AND state='submitted' AND session_id IS NULL AND EXISTS (SELECT 1 FROM memory_summary_jobs j WHERE j.id=job_id AND j.state='claimed' AND j.lease_owner_boot_id=?3 AND j.lease_expires_at>?4 AND j.attempts=attempt) AND EXISTS (SELECT 1 FROM employee_sessions es JOIN conversation_turns t ON t.recipient_employee_id=es.employee_id WHERE es.id=?2 AND t.id=?1 AND es.owner_boot_id=?3 AND es.state='starting')",
            params![turn_id.to_string(), session_id.to_string(), owner.to_string(), Utc::now().to_rfc3339()],
        )?;
        if changed != 1 {
            return Err(StoreError::InvalidRecord {
                field: "memory session reservation",
                message: "summary session is not owned by this live attempt".to_owned(),
            });
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn memory_worker_employee_available(&self, employee_id: Uuid) -> Result<bool, StoreError> {
        self.lock()?.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM employee_sessions WHERE employee_id=?1 AND state IN ('starting','idle','stopping'))",
            [employee_id.to_string()], |row| row.get(0),
        ).map_err(StoreError::from)
    }

    /// Claims only work whose scope has an explicit enabled worker setting and no prior durable
    /// internal-turn link. A linked job is intentionally left visible for reconciliation; it is
    /// never silently resent after a crash.
    pub fn claim_memory_summary_for_worker(
        &self,
        owner: Uuid,
    ) -> Result<Option<lazoo_protocol::MemorySummaryJob>, StoreError> {
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().to_rfc3339();
        let admission_cutoff = (Utc::now() - Duration::minutes(1)).to_rfc3339();
        let id: Option<String> = tx.query_row(
            "SELECT j.id FROM memory_summary_jobs j JOIN memory_worker_settings s ON s.scope_key=j.scope_key AND s.enabled=1 AND (s.last_admitted_at IS NULL OR s.last_admitted_at<=?2) LEFT JOIN memory_summary_attempts a ON a.job_id=j.id WHERE a.job_id IS NULL AND j.state='pending' AND j.due_at<=?1 AND NOT EXISTS (SELECT 1 FROM employee_sessions es WHERE es.employee_id=s.employee_id AND es.state IN ('starting','idle','stopping')) ORDER BY j.due_at,j.id LIMIT 1", params![now, admission_cutoff], |r| r.get(0)
        ).optional()?;
        let Some(id) = id else {
            tx.commit()?;
            return Ok(None);
        };
        let claimed = tx.execute("UPDATE memory_summary_jobs SET state='claimed',lease_owner_boot_id=?2,lease_expires_at=?3,attempts=attempts+1,updated_at=?4 WHERE id=?1 AND state='pending'", params![id,owner.to_string(),(Utc::now()+Duration::minutes(5)).to_rfc3339(),now])?;
        if claimed != 1 {
            return Err(StoreError::InvalidRecord {
                field: "memory worker claim",
                message: "job was no longer pending".to_owned(),
            });
        }
        tx.execute(
            "UPDATE memory_worker_settings SET last_admitted_at=?2 WHERE scope_key=(SELECT scope_key FROM memory_summary_jobs WHERE id=?1)",
            params![id, now],
        )?;
        let job = tx.query_row("SELECT id,source_hash,source_turn_id,project_id,state,attempts,due_at,lease_owner_boot_id,created_at,updated_at,failure_message FROM memory_summary_jobs WHERE id=?1", [id], |r| {
            Ok(lazoo_protocol::MemorySummaryJob { id: parse_id(&r.get::<_,String>(0)?)?, source_hash:r.get(1)?, source_turn_id:parse_id(&r.get::<_,String>(2)?)?, project_id:r.get::<_,Option<String>>(3)?.as_deref().map(parse_id).transpose()?, state:serde_json::from_str(&format!("\"{}\"",r.get::<_,String>(4)?)).map_err(|_|rusqlite::Error::InvalidQuery)?, attempts:u32::try_from(r.get::<_,i64>(5)?).map_err(|_|rusqlite::Error::InvalidQuery)?,due_at:r.get(6)?,lease_owner_boot_id:r.get::<_,Option<String>>(7)?.as_deref().map(parse_id).transpose()?,created_at:r.get(8)?,updated_at:r.get(9)?,failure_message:r.get(10)? })
        })?;
        tx.commit()?;
        Ok(Some(job))
    }
    pub fn memory_worker_settings(
        &self,
        project_id: Option<Uuid>,
    ) -> Result<Option<MemoryWorkerSettings>, StoreError> {
        let c = self.lock()?;
        c.query_row("SELECT revision,employee_id,model,requests_per_minute,enabled FROM memory_worker_settings WHERE scope_key=?1", [worker_scope(project_id)], |r| {
            Ok(MemoryWorkerSettings { project_id, revision: r.get::<_, i64>(0)?.cast_unsigned(), employee_id: parse_id(&r.get::<_, String>(1)?)?, model: r.get(2)?, requests_per_minute: u32::try_from(r.get::<_, i64>(3)?).map_err(|_| rusqlite::Error::InvalidQuery)?, enabled: r.get::<_, i64>(4)? == 1 })
        }).optional().map_err(StoreError::from)
    }

    pub fn set_memory_worker_settings(
        &self,
        input: &SetMemoryWorkerSettingsInput,
    ) -> Result<MemoryWorkerSettings, StoreError> {
        if input.model.is_empty()
            || input.model.len() > 256
            || !(1..=60).contains(&input.requests_per_minute)
        {
            return Err(StoreError::InvalidRecord {
                field: "memory worker settings",
                message: "invalid model or request budget".to_owned(),
            });
        }
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT revision FROM memory_worker_settings WHERE scope_key=?1",
                [worker_scope(input.project_id)],
                |r| r.get(0),
            )
            .optional()?;
        if current.map(i64::cast_unsigned) != input.expected_revision {
            return Err(StoreError::InvalidRecord {
                field: "memory worker revision",
                message: "stale settings".to_owned(),
            });
        }
        // Reject a dangling employee selection before enabling an internal native turn.
        let present: i64 = tx.query_row(
            "SELECT COUNT(*) FROM employees WHERE id=?1",
            [input.employee_id.to_string()],
            |r| r.get(0),
        )?;
        if present != 1 {
            return Err(StoreError::InvalidRecord {
                field: "memory worker employee",
                message: "employee does not exist".to_owned(),
            });
        }
        let revision = current.unwrap_or(0) + 1;
        tx.execute("INSERT INTO memory_worker_settings(scope_key,project_id,revision,employee_id,model,requests_per_minute,enabled,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(scope_key) DO UPDATE SET revision=excluded.revision,employee_id=excluded.employee_id,model=excluded.model,requests_per_minute=excluded.requests_per_minute,enabled=excluded.enabled,updated_at=excluded.updated_at", params![worker_scope(input.project_id), input.project_id.map(|x|x.to_string()), revision, input.employee_id.to_string(), input.model, input.requests_per_minute, i64::from(input.enabled), Utc::now().to_rfc3339()])?;
        tx.commit()?;
        Ok(MemoryWorkerSettings {
            project_id: input.project_id,
            revision: revision.cast_unsigned(),
            employee_id: input.employee_id,
            model: input.model.clone(),
            requests_per_minute: input.requests_per_minute,
            enabled: input.enabled,
        })
    }

    pub fn link_memory_summary_attempt(
        &self,
        job_id: Uuid,
        owner: Uuid,
        attempt: u32,
        lease_token: Uuid,
        conversation_id: Uuid,
        turn_id: Uuid,
    ) -> Result<MemorySummaryAttempt, StoreError> {
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().to_rfc3339();
        let valid: i64 = tx.query_row(
            "SELECT COUNT(*) FROM memory_summary_jobs WHERE id=?1 AND state='claimed' AND lease_owner_boot_id=?2 AND attempts=?3 AND lease_expires_at>?4",
            params![job_id.to_string(), owner.to_string(), attempt, now],
            |r| r.get(0),
        )?;
        if valid != 1 {
            return Err(StoreError::InvalidRecord {
                field: "memory worker lease",
                message: "attempt is no longer owned".to_owned(),
            });
        }
        let inserted = tx.execute("INSERT INTO memory_summary_attempts(job_id,attempt,lease_token,conversation_id,turn_id,state,started_at) VALUES(?1,?2,?3,?4,?5,'submitted',?6) ON CONFLICT(job_id) DO NOTHING", params![job_id.to_string(),attempt,lease_token.to_string(),conversation_id.to_string(),turn_id.to_string(),now])?;
        if inserted != 1 {
            return Err(StoreError::InvalidRecord {
                field: "memory worker attempt",
                message: "job already has a durable internal turn".to_owned(),
            });
        }
        let attempt = tx.query_row("SELECT attempt,lease_token,conversation_id,turn_id,state FROM memory_summary_attempts WHERE job_id=?1", [job_id.to_string()], |r| Ok(MemorySummaryAttempt { job_id, attempt:u32::try_from(r.get::<_,i64>(0)?).map_err(|_| rusqlite::Error::InvalidQuery)?, lease_token:parse_id(&r.get::<_,String>(1)?)?, conversation_id:parse_id(&r.get::<_,String>(2)?)?, turn_id:parse_id(&r.get::<_,String>(3)?)?, state:r.get(4)? }))?;
        tx.commit()?;
        Ok(attempt)
    }
}

fn worker_scope(project_id: Option<Uuid>) -> String {
    project_id.map(|id| id.to_string()).unwrap_or_default()
}
fn parse_id(value: &str) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CompleteMemorySummaryInput;
    use tempfile::tempdir;

    fn store() -> Store {
        Store::open_path(tempdir().unwrap().keep().join("worker.db")).unwrap()
    }
    fn pending(store: &Store, id: Uuid) {
        let c = store.lock().unwrap();
        c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        c.execute("INSERT INTO memory_summary_jobs(id,source_hash,source_turn_id,project_id,scope_key,state,attempts,due_at,created_at,updated_at) VALUES(?1,?2,?3,NULL,'','pending',0,'2000-01-01T00:00:00Z','2000-01-01T00:00:00Z','2000-01-01T00:00:00Z')", params![id.to_string(), id.to_string(), Uuid::new_v4().to_string()]).unwrap();
    }
    fn enable_worker(store: &Store) {
        let employee = Uuid::new_v4();
        let c = store.lock().unwrap();
        c.execute("INSERT INTO employees(id,name,provider,role,instructions,model,native_settings_json,authority_mode,checkout_policy,enabled,configuration_revision,created_at,updated_at) VALUES(?1,'e','codex','r','i','m','{}','ask','private',1,1,'2000-01-01T00:00:00Z','2000-01-01T00:00:00Z')",[employee.to_string()]).unwrap();
        c.execute("INSERT INTO memory_worker_settings(scope_key,project_id,revision,employee_id,model,requests_per_minute,enabled,updated_at) VALUES('',NULL,1,?1,'m',1,1,'2000-01-01T00:00:00Z')",[employee.to_string()]).unwrap();
    }
    #[test]
    fn no_settings_leaves_pending_job_unclaimed() {
        let store = store();
        let job = Uuid::new_v4();
        pending(&store, job);
        assert!(
            store
                .claim_memory_summary_for_worker(Uuid::new_v4())
                .unwrap()
                .is_none()
        );
        let c = store.lock().unwrap();
        let state: String = c
            .query_row(
                "SELECT state FROM memory_summary_jobs WHERE id=?1",
                [job.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "pending");
    }
    #[test]
    fn linked_attempt_recovery_blocks_second_claim_and_fences_wrong_attempt() {
        let store = store();
        let job = Uuid::new_v4();
        pending(&store, job);
        enable_worker(&store);
        let owner = Uuid::new_v4();
        let claimed = store
            .claim_memory_summary_for_worker(owner)
            .unwrap()
            .unwrap();
        store
            .link_memory_summary_attempt(
                job,
                owner,
                claimed.attempts,
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
            )
            .unwrap();
        assert!(
            store
                .link_memory_summary_attempt(
                    job,
                    owner,
                    claimed.attempts,
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                )
                .is_err()
        );
        assert!(
            store
                .settle_memory_summary_attempt_failure(
                    job,
                    owner,
                    claimed.attempts + 1,
                    "bad",
                    false
                )
                .is_err()
        );
        store.recover_linked_memory_summary_attempts().unwrap();
        assert!(
            store
                .claim_memory_summary_for_worker(Uuid::new_v4())
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn durable_admission_spacing_blocks_a_second_scope_claim() {
        let store = store();
        enable_worker(&store);
        pending(&store, Uuid::new_v4());
        pending(&store, Uuid::new_v4());
        assert!(
            store
                .claim_memory_summary_for_worker(Uuid::new_v4())
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .claim_memory_summary_for_worker(Uuid::new_v4())
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn expired_lease_cannot_create_a_durable_attempt_link() {
        let store = store();
        enable_worker(&store);
        let job = Uuid::new_v4();
        pending(&store, job);
        let owner = Uuid::new_v4();
        let claimed = store
            .claim_memory_summary_for_worker(owner)
            .unwrap()
            .unwrap();
        let c = store.lock().unwrap();
        c.execute(
            "UPDATE memory_summary_jobs SET lease_expires_at='2000-01-01T00:00:00Z' WHERE id=?1",
            [job.to_string()],
        )
        .unwrap();
        drop(c);
        assert!(
            store
                .link_memory_summary_attempt(
                    job,
                    owner,
                    claimed.attempts,
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                )
                .is_err()
        );
    }
    #[test]
    fn completion_marks_link_completed_before_recovery() {
        let store = store();
        enable_worker(&store);
        let job = Uuid::new_v4();
        pending(&store, job);
        let owner = Uuid::new_v4();
        let claimed = store
            .claim_memory_summary_for_worker(owner)
            .unwrap()
            .unwrap();
        store
            .link_memory_summary_attempt(
                job,
                owner,
                claimed.attempts,
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
            )
            .unwrap();
        store
            .complete_memory_summary(&CompleteMemorySummaryInput {
                job_id: job,
                owner_boot_id: owner,
                expected_attempt: claimed.attempts,
                records: Vec::new(),
            })
            .unwrap();
        assert_eq!(store.recover_linked_memory_summary_attempts().unwrap(), 0);
        let c = store.lock().unwrap();
        let state: String = c
            .query_row(
                "SELECT state FROM memory_summary_attempts WHERE job_id=?1",
                [job.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "completed");
    }
}
