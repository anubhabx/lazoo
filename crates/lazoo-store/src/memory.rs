use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use chrono::{Duration, Utc};
use lazoo_protocol::{
    MemoryCleanupPreview, MemoryContextPack, MemoryPolicy, MemoryRecord, MemoryRecordType,
    MemorySearchParams, MemorySourceArchive, MemorySummaryJob, TurnState,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{Store, StoreError, employee};

const DEFAULT_QUOTA: u64 = 256 * 1024 * 1024;
const DEFAULT_SOURCE_CAP: u64 = 4 * 1024 * 1024;
const MAX_RESULT: u32 = 100;
const GLOBAL_SCOPE_KEY: &str = "__global__";
/// Archive kinds. `turn` is the bounded canonical turn the summary worker reads; `raw-frames`
/// holds the provider's redacted raw payloads for the same turn and is never summarized.
const TURN_KIND: &str = "turn";
const RAW_FRAMES_KIND: &str = "raw-frames";

#[derive(Clone, Debug)]
pub struct ArchiveMemoryTurnInput {
    pub source_turn_id: Uuid,
    pub project_id: Option<Uuid>,
    pub archive_root: PathBuf,
}
/// The raw provider payloads the daemon captured for one completed turn, in observation order.
/// `dropped_frames` counts payloads the daemon already left out to stay under the policy cap.
#[derive(Clone, Debug)]
pub struct ArchiveMemoryRawFramesInput {
    pub source_turn_id: Uuid,
    pub archive_root: PathBuf,
    pub frames: Vec<serde_json::Value>,
    pub dropped_frames: u64,
}
/// One archive write, whichever kind: the source is already redacted and within the cap.
struct SourceArchiveWrite<'a> {
    kind: &'static str,
    source_turn_id: Uuid,
    scope: Option<Uuid>,
    source: &'a [u8],
    truncated: bool,
    policy: &'a MemoryPolicy,
    archive_root: &'a Path,
}
/// A completed turn awaiting a source archive. Internal summary-worker turns are excluded by
/// the query that produces these values, so their model output never becomes a source archive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryArchiveCandidate {
    pub source_turn_id: Uuid,
    pub project_id: Option<Uuid>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct MemoryArchiveError {
    pub source_turn_id: Uuid,
    pub source_hash: String,
    pub archive_name: String,
    pub compressed_bytes: u64,
}
#[derive(Clone, Debug)]
pub struct CreateMemoryRecordInput {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub source_turn_id: Uuid,
    pub source_hash: String,
    pub record_type: MemoryRecordType,
    pub text: String,
    pub confidence: f64,
    pub importance: u8,
}
#[derive(Clone, Debug)]
pub struct SetMemoryPolicyInput {
    pub project_id: Option<Uuid>,
    pub expected_revision: u64,
    pub retention_enabled: bool,
    pub retention_days: u32,
    pub quota_bytes: u64,
    pub max_source_bytes: u64,
    pub allowed_types: Vec<MemoryRecordType>,
    pub inject_enabled: bool,
}
#[derive(Clone, Debug)]
pub struct CompleteMemorySummaryInput {
    pub job_id: Uuid,
    pub owner_boot_id: Uuid,
    /// The generation returned by `claim_memory_summary`.  A boot id alone is not
    /// sufficient because an expired lease may be reclaimed by the same process.
    pub expected_attempt: u32,
    pub records: Vec<CreateMemoryRecordInput>,
}
#[derive(Clone, Debug)]
pub struct FailMemorySummaryInput {
    pub job_id: Uuid,
    pub owner_boot_id: Uuid,
    /// The generation returned by `claim_memory_summary`.
    pub expected_attempt: u32,
    pub message: String,
}

impl Store {
    pub fn list_memory_archive_candidates(
        &self,
        limit: u32,
    ) -> Result<Vec<MemoryArchiveCandidate>, StoreError> {
        if limit == 0 || limit > 10 {
            return Err(invalid("memory archive batch", "must be between 1 and 10"));
        }
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT t.id,c.project_id FROM conversation_turns t JOIN conversations c ON c.id=t.conversation_id LEFT JOIN memory_archives a ON a.source_turn_id=t.id AND a.source_kind='turn' LEFT JOIN memory_archive_intents intent ON intent.source_turn_id=t.id AND intent.source_kind='turn' AND intent.state='error' LEFT JOIN memory_summary_attempts attempt ON attempt.turn_id=t.id WHERE t.state='completed' AND a.source_turn_id IS NULL AND intent.source_turn_id IS NULL AND attempt.turn_id IS NULL ORDER BY t.updated_at,t.id LIMIT ?1",
        )?;
        let rows = statement.query_map([i64::from(limit)], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        rows.map(|row| {
            let (id, project) = row?;
            Ok(MemoryArchiveCandidate {
                source_turn_id: Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
                project_id: project
                    .map(|value| Uuid::parse_str(&value).map_err(|_| rusqlite::Error::InvalidQuery))
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>, rusqlite::Error>>()
        .map_err(StoreError::from)
    }
    pub fn archive_memory_turn(
        &self,
        input: &ArchiveMemoryTurnInput,
    ) -> Result<MemorySourceArchive, StoreError> {
        let turn = self.get_turn(input.source_turn_id)?;
        let scope = turn_scope(self, input.source_turn_id)?;
        if scope != input.project_id {
            return Err(invalid("memory project", "source turn scope differs"));
        }
        let mut source_value = serde_json::to_value(&turn)?;
        redact_value(&mut source_value);
        let source = serde_json::to_vec(&source_value)?;
        let policy = self.inherited_memory_policy(scope)?;
        if !policy.retention_enabled {
            return Err(invalid("memory retention", "source retention is disabled"));
        }
        if source.len() as u64 > policy.max_source_bytes {
            return Err(invalid(
                "memory source",
                "sanitized source exceeds configured byte cap",
            ));
        }
        self.write_source_archive(&SourceArchiveWrite {
            kind: TURN_KIND,
            source_turn_id: input.source_turn_id,
            scope,
            source: &source,
            truncated: turn.transcript_truncated,
            policy: &policy,
            archive_root: &input.archive_root,
        })
    }
    /// Retains the provider's raw frames of one completed turn as a `raw-frames` archive: an
    /// NDJSON header, one redacted line per frame while the policy cap allows, and a trailer
    /// naming the dropped frames when the cap was reached. It shares the quota, the retention
    /// opt-out and the integrity path with the canonical archive and is never summarized.
    pub fn archive_memory_raw_frames(
        &self,
        input: ArchiveMemoryRawFramesInput,
    ) -> Result<Option<MemorySourceArchive>, StoreError> {
        if input.frames.is_empty() && input.dropped_frames == 0 {
            return Ok(None);
        }
        let turn = self.get_turn(input.source_turn_id)?;
        if turn.state != TurnState::Completed {
            return Err(invalid(
                "memory source",
                "raw frames are retained for completed turns only",
            ));
        }
        let scope = turn_scope(self, input.source_turn_id)?;
        let policy = self.inherited_memory_policy(scope)?;
        if !policy.retention_enabled {
            return Err(invalid("memory retention", "source retention is disabled"));
        }
        let source_turn_id = input.source_turn_id;
        let archive_root = input.archive_root.clone();
        let (source, dropped) = raw_frames_source(input, policy.max_source_bytes)?;
        self.write_source_archive(&SourceArchiveWrite {
            kind: RAW_FRAMES_KIND,
            source_turn_id,
            scope,
            source: &source,
            truncated: dropped > 0,
            policy: &policy,
            archive_root: &archive_root,
        })
        .map(Some)
    }
    /// The shared write sequence for every archive kind: hash, compress, reserve quota with a
    /// `write-pending` intent, place the file, then record the archive and drop the intent.
    fn write_source_archive(
        &self,
        write: &SourceArchiveWrite<'_>,
    ) -> Result<MemorySourceArchive, StoreError> {
        let hash = hex_hash(write.source);
        let archive_name = archive_name(write.kind, write.scope, &hash);
        let root = validate_archive_root(write.archive_root)?;
        let target = root.join(&archive_name);
        let compressed = zstd::stream::encode_all(write.source, 3)
            .map_err(|e| invalid("memory archive", &e.to_string()))?;
        let compressed_bytes = u64::try_from(compressed.len())
            .map_err(|_| invalid("memory archive", "compressed length overflow"))?;
        let compressed_len = i64::try_from(compressed_bytes)
            .map_err(|_| invalid("memory archive", "compressed length overflow"))?;
        let source_len = i64::try_from(write.source.len())
            .map_err(|_| invalid("memory archive", "source length overflow"))?;
        let turn_id = write.source_turn_id.to_string();
        let now = Utc::now().to_rfc3339();
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = archive_from_tx_optional(&tx, write.source_turn_id, write.kind)? {
            tx.commit()?;
            return Ok(existing);
        }
        // Both kinds share one per-scope quota, so raw frames can never exceed it on their own.
        let used: i64 = tx.query_row(
            "SELECT COALESCE((SELECT SUM(compressed_bytes) FROM memory_archives WHERE scope_key=?1),0) + COALESCE((SELECT SUM(compressed_bytes) FROM memory_archive_intents WHERE scope_key=?1 AND state IN ('write-pending','error')),0)",
            [scope_key(write.scope)],
            |r| r.get(0),
        )?;
        if u64::try_from(used)
            .map_err(|_| invalid("memory quota", "invalid stored quota"))?
            .saturating_add(compressed_bytes)
            > write.policy.quota_bytes
        {
            return Err(invalid("memory quota", "archive quota exceeded"));
        }
        tx.execute("INSERT OR REPLACE INTO memory_archive_intents(source_turn_id,source_kind,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,state,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'write-pending',?10)", params![turn_id,write.kind,hash,write.scope.map(|x| x.to_string()),scope_key(write.scope),archive_name,compressed_len,source_len,i64::from(write.truncated),now])?;
        tx.commit()?;
        drop(c);
        if target.exists() {
            let metadata = fs::symlink_metadata(&target).map_err(|e| ioerr(&target, e))?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err(invalid(
                    "memory archive",
                    "existing archive is not a regular file",
                ));
            }
            verify_archive(&target, write.source, write.policy.max_source_bytes)?;
        } else {
            let temp = root.join(format!(".{hash}.{}.tmp", Uuid::new_v4()));
            {
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temp)
                    .map_err(|e| ioerr(&temp, e))?;
                f.write_all(&compressed).map_err(|e| ioerr(&temp, e))?;
                f.sync_all().map_err(|e| ioerr(&temp, e))?;
            }
            fs::rename(&temp, &target).map_err(|e| ioerr(&target, e))?;
        }
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO memory_archives(source_turn_id,source_kind,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![turn_id,write.kind,hash,write.scope.map(|x|x.to_string()),scope_key(write.scope),archive_name,compressed_len,source_len,i64::from(write.truncated),now])?;
        tx.execute(
            "DELETE FROM memory_archive_intents WHERE source_turn_id=?1 AND source_kind=?2",
            params![turn_id, write.kind],
        )?;
        let rec = archive_from_tx(&tx, write.source_turn_id, write.kind)?;
        employee::insert_event(
            &tx,
            "memory.archived",
            &json!({"archive":rec,"kind":write.kind}),
            &now,
        )?;
        tx.commit()?;
        Ok(rec)
    }
    pub fn memory_policy(&self, project_id: Option<Uuid>) -> Result<MemoryPolicy, StoreError> {
        let c = self.lock()?;
        policy_from(&c, project_id)
    }
    pub fn global_memory_policy(&self) -> Result<MemoryPolicy, StoreError> {
        let connection = self.lock()?;
        policy_from_scope(&connection, None, GLOBAL_SCOPE_KEY)
    }
    pub fn set_global_memory_policy(
        &self,
        input: &SetMemoryPolicyInput,
    ) -> Result<MemoryPolicy, StoreError> {
        if input.project_id.is_some() {
            return Err(invalid(
                "memory policy",
                "global policy cannot name a project",
            ));
        }
        if input.retention_days == 0
            || input.quota_bytes > 1 << 40
            || input.max_source_bytes == 0
            || input.max_source_bytes > 64 << 20
        {
            return Err(invalid("memory policy", "out of bounds"));
        }
        let mut connection = self.lock()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = policy_from_scope(&tx, None, GLOBAL_SCOPE_KEY)?;
        if current.revision != input.expected_revision {
            return Err(StoreError::InvalidRecord {
                field: "memory policy revision",
                message: format!("current revision is {}", current.revision),
            });
        }
        let now = Utc::now().to_rfc3339();
        tx.execute("INSERT INTO memory_policies(scope_key,project_id,revision,retention_enabled,retention_days,quota_bytes,max_source_bytes,allowed_types_json,inject_enabled,updated_at) VALUES(?1,NULL,2,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(scope_key) DO UPDATE SET revision=memory_policies.revision+1,retention_enabled=excluded.retention_enabled,retention_days=excluded.retention_days,quota_bytes=excluded.quota_bytes,max_source_bytes=excluded.max_source_bytes,allowed_types_json=excluded.allowed_types_json,inject_enabled=excluded.inject_enabled,updated_at=excluded.updated_at", params![GLOBAL_SCOPE_KEY,i64::from(input.retention_enabled),input.retention_days,i64::try_from(input.quota_bytes).map_err(|_| invalid("memory policy", "quota overflow"))?,i64::try_from(input.max_source_bytes).map_err(|_| invalid("memory policy", "source cap overflow"))?,serde_json::to_string(&input.allowed_types)?,i64::from(input.inject_enabled),now])?;
        let result = policy_from_scope(&tx, None, GLOBAL_SCOPE_KEY)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn inherited_memory_policy(
        &self,
        project_id: Option<Uuid>,
    ) -> Result<MemoryPolicy, StoreError> {
        let connection = self.lock()?;
        // Home (`None`) is a real local scope.  It can override the global default just
        // as a project can; only an absent scope-local row inherits global settings.
        if policy_exists(&connection, &scope_key(project_id))? {
            return policy_from(&connection, project_id);
        }
        policy_from_scope(&connection, project_id, GLOBAL_SCOPE_KEY)
    }
    pub fn reconcile_memory_archives(&self, archive_root: &Path) -> Result<u64, StoreError> {
        let root = validate_archive_root(archive_root)?;
        let mut connection = self.lock()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Intents and archives are keyed by `archive_name` here: it is unique in both tables,
        // so replaying one kind never touches the other kind's row for the same turn.
        let mut statement = tx.prepare("SELECT source_turn_id,source_kind,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,created_at FROM memory_archive_intents WHERE state='write-pending'")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let mut count = 0_u64;
        for (
            turn,
            kind,
            hash,
            project,
            scope,
            name,
            compressed,
            uncompressed,
            truncated,
            created,
        ) in rows
        {
            let path = root.join(&name);
            if !path.exists() {
                tx.execute(
                    "DELETE FROM memory_archive_intents WHERE archive_name=?1",
                    [&name],
                )?;
            } else if verify_intent_archive(&path, &hash, compressed, uncompressed)? {
                tx.execute("INSERT OR IGNORE INTO memory_archives(source_turn_id,source_kind,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![turn,kind,hash,project,scope,name,compressed,uncompressed,truncated,created])?;
                tx.execute(
                    "DELETE FROM memory_archive_intents WHERE archive_name=?1",
                    [&name],
                )?;
            } else {
                tx.execute(
                    "UPDATE memory_archive_intents SET state='error' WHERE archive_name=?1",
                    [&name],
                )?;
            }
            count += 1;
        }
        let mut cleanup_statement = tx.prepare(
            "SELECT archive_name FROM memory_archive_intents WHERE state='cleanup-pending'",
        )?;
        let cleanup_rows = cleanup_statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(cleanup_statement);
        for name in cleanup_rows {
            let path = root.join(&name);
            if path.exists() {
                let metadata = fs::symlink_metadata(&path).map_err(|error| ioerr(&path, error))?;
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                    return Err(invalid(
                        "memory cleanup",
                        "cleanup archive is not a regular file",
                    ));
                }
                fs::remove_file(&path).map_err(|error| ioerr(&path, error))?;
            }
            tx.execute("DELETE FROM memory_archives WHERE archive_name=?1", [&name])?;
            tx.execute(
                "DELETE FROM memory_archive_intents WHERE archive_name=?1",
                [&name],
            )?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }
    pub fn read_memory_source(
        &self,
        archive_root: &Path,
        source_turn_id: Uuid,
    ) -> Result<Vec<u8>, StoreError> {
        self.read_archive(archive_root, source_turn_id, TURN_KIND)
    }
    /// The retained raw provider frames of one turn as NDJSON; see `archive_memory_raw_frames`.
    pub fn read_memory_raw_frames(
        &self,
        archive_root: &Path,
        source_turn_id: Uuid,
    ) -> Result<Vec<u8>, StoreError> {
        self.read_archive(archive_root, source_turn_id, RAW_FRAMES_KIND)
    }
    fn read_archive(
        &self,
        archive_root: &Path,
        source_turn_id: Uuid,
        kind: &str,
    ) -> Result<Vec<u8>, StoreError> {
        let root = validate_archive_root(archive_root)?;
        let connection = self.lock()?;
        let archive: (String, String, i64, i64) = connection.query_row(
            "SELECT archive_name,source_hash,compressed_bytes,uncompressed_bytes FROM memory_archives WHERE source_turn_id=?1 AND source_kind=?2",
            params![source_turn_id.to_string(), kind],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).optional()?.ok_or_else(|| invalid("memory source", "archive was not found"))?;
        let path = root.join(&archive.0);
        if !verify_intent_archive(&path, &archive.1, archive.2, archive.3)? {
            return Err(invalid("memory source", "archive integrity check failed"));
        }
        let file = fs::File::open(&path).map_err(|error| ioerr(&path, error))?;
        let mut decoder = zstd::stream::read::Decoder::new(file)
            .map_err(|error| invalid("memory source", &error.to_string()))?
            .take(
                u64::try_from(archive.3)
                    .map_err(|_| invalid("memory source", "invalid archive length"))?
                    .saturating_add(1),
            );
        let mut bytes = Vec::new();
        decoder
            .read_to_end(&mut bytes)
            .map_err(|error| invalid("memory source", &error.to_string()))?;
        Ok(bytes)
    }
    pub fn set_memory_policy(&self, i: &SetMemoryPolicyInput) -> Result<MemoryPolicy, StoreError> {
        if i.retention_days == 0
            || i.quota_bytes > 1 << 40
            || i.max_source_bytes == 0
            || i.max_source_bytes > 64 << 20
        {
            return Err(invalid("memory policy", "out of bounds"));
        }
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = policy_from(&tx, i.project_id)?;
        if current.revision != i.expected_revision {
            return Err(StoreError::InvalidRecord {
                field: "memory policy revision",
                message: format!("current revision is {}", current.revision),
            });
        }
        let now = Utc::now().to_rfc3339();
        tx.execute("INSERT INTO memory_policies(scope_key,project_id,revision,retention_enabled,retention_days,quota_bytes,max_source_bytes,allowed_types_json,inject_enabled,updated_at) VALUES(?1,?2,2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(scope_key) DO UPDATE SET revision=memory_policies.revision+1,retention_enabled=excluded.retention_enabled,retention_days=excluded.retention_days,quota_bytes=excluded.quota_bytes,max_source_bytes=excluded.max_source_bytes,allowed_types_json=excluded.allowed_types_json,inject_enabled=excluded.inject_enabled,updated_at=excluded.updated_at",params![scope_key(i.project_id),i.project_id.map(|x|x.to_string()),i64::from(i.retention_enabled),i.retention_days,i64::try_from(i.quota_bytes).map_err(|_| invalid("memory policy", "quota overflow"))?,i64::try_from(i.max_source_bytes).map_err(|_| invalid("memory policy", "source cap overflow"))?,serde_json::to_string(&i.allowed_types)?,i64::from(i.inject_enabled),now])?;
        let result = policy_from(&tx, i.project_id)?;
        employee::insert_event(&tx, "memory.policy-set", &json!({"policy":result}), &now)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn search_memory(&self, p: &MemorySearchParams) -> Result<Vec<MemoryRecord>, StoreError> {
        if p.limit == 0 || p.limit > MAX_RESULT {
            return Err(invalid("memory limit", "must be 1..=100"));
        }
        let c = self.lock()?;
        if p.branch
            .as_deref()
            .is_some_and(|branch| branch.is_empty() || branch.len() > 255)
        {
            return Err(invalid("memory branch", "must be 1..=255 bytes"));
        }
        let query = literal_fts(&p.query);
        let types = p
            .record_types
            .iter()
            .map(|kind| {
                serde_json::to_string(kind)
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_owned()
            })
            .collect::<Vec<_>>();
        let type_clause = if types.is_empty() {
            String::new()
        } else {
            format!(
                " AND r.record_type IN ({})",
                types
                    .iter()
                    .map(|kind| format!("'{kind}'"))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        let source_filters = " AND (?3 IS NULL OR t.recipient_employee_id=?3) AND (?4 IS NULL OR t.conversation_id=?4) AND (?5 IS NULL OR t.task_id=?5) AND (?6 IS NULL OR EXISTS (SELECT 1 FROM tasks root_task WHERE root_task.id=t.task_id AND root_task.root_task_id=?6)) AND (?7 IS NULL OR EXISTS (SELECT 1 FROM task_dispatches dispatch WHERE dispatch.task_id=t.task_id AND dispatch.workspace_kind='git-worktree' AND dispatch.branch_name=?7))";
        let sql = if query.is_empty() {
            format!(
                "SELECT r.id,r.revision,r.project_id,r.source_turn_id,r.source_hash,r.record_type,r.text,r.confidence,r.importance,r.created_at,r.active,r.superseded_at FROM memory_records r JOIN conversation_turns t ON t.id=r.source_turn_id WHERE r.scope_key=?1 AND r.active=1{type_clause}{source_filters} ORDER BY r.importance DESC,r.created_at DESC LIMIT ?8"
            )
        } else {
            format!(
                "SELECT r.id,r.revision,r.project_id,r.source_turn_id,r.source_hash,r.record_type,r.text,r.confidence,r.importance,r.created_at,r.active,r.superseded_at FROM memory_records r JOIN memory_records_fts ON memory_records_fts.rowid=r.rowid JOIN conversation_turns t ON t.id=r.source_turn_id WHERE r.scope_key=?1 AND r.active=1 AND memory_records_fts.text MATCH ?2{type_clause}{source_filters} ORDER BY bm25(memory_records_fts),r.importance DESC,r.created_at DESC LIMIT ?8"
            )
        };
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map(
            params![
                scope_key(p.project_id),
                query,
                p.employee_id.map(|id| id.to_string()),
                p.conversation_id.map(|id| id.to_string()),
                p.task_id.map(|id| id.to_string()),
                p.root_task_id.map(|id| id.to_string()),
                p.branch,
                p.limit
            ],
            raw_record,
        )?;
        rows.map(|row| record_from(row?)).collect()
    }
    pub fn memory_context(&self, p: &MemorySearchParams) -> Result<MemoryContextPack, StoreError> {
        let policy = self.inherited_memory_policy(p.project_id)?;
        if !policy.inject_enabled {
            return Ok(MemoryContextPack {
                records: vec![],
                truncated: false,
                byte_budget: 2048,
            });
        }
        let allowed_types = if p.record_types.is_empty() {
            policy.allowed_types.clone()
        } else {
            p.record_types
                .iter()
                .copied()
                .filter(|record_type| policy.allowed_types.contains(record_type))
                .collect()
        };
        let found = self.search_memory(&MemorySearchParams {
            limit: 8,
            record_types: allowed_types,
            ..p.clone()
        })?;
        let mut bytes = 0usize;
        let mut kept = Vec::new();
        let mut truncated = false;
        for r in found {
            let n = r.text.len();
            if bytes + n > 2048 {
                truncated = true;
                continue;
            }
            bytes += n;
            kept.push(r);
        }
        Ok(MemoryContextPack {
            records: kept,
            truncated,
            byte_budget: 2048,
        })
    }
    pub fn enqueue_memory_summary(
        &self,
        a: &MemorySourceArchive,
    ) -> Result<MemorySummaryJob, StoreError> {
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let archived = archive_from_tx(&tx, a.source_turn_id, TURN_KIND)?;
        if archived.project_id != a.project_id || archived.source_hash != a.source_hash {
            return Err(invalid(
                "memory archive",
                "supplied archive metadata differs from store",
            ));
        }
        let now = Utc::now().to_rfc3339();
        tx.execute("INSERT INTO memory_summary_jobs(id,source_hash,source_turn_id,project_id,scope_key,state,attempts,due_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'pending',0,?6,?6,?6) ON CONFLICT(scope_key,source_hash) DO NOTHING",params![Uuid::new_v4().to_string(),a.source_hash,a.source_turn_id.to_string(),a.project_id.map(|x|x.to_string()),scope_key(a.project_id),now])?;
        let j = job_for_hash(&tx, a.project_id, &a.source_hash)?;
        tx.commit()?;
        Ok(j)
    }
    pub fn claim_memory_summary(
        &self,
        owner: Uuid,
    ) -> Result<Option<MemorySummaryJob>, StoreError> {
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().to_rfc3339();
        let id:Option<String>=tx.query_row("SELECT id FROM memory_summary_jobs WHERE (state='pending' AND due_at<=?1) OR (state='claimed' AND lease_expires_at<=?1) ORDER BY due_at,id LIMIT 1",[&now],|r|r.get(0)).optional()?;
        if let Some(id) = id {
            tx.execute("UPDATE memory_summary_jobs SET state='claimed',lease_owner_boot_id=?2,lease_expires_at=?3,attempts=attempts+1,updated_at=?4 WHERE id=?1 AND (state='pending' OR (state='claimed' AND lease_expires_at<=?4))",params![id,owner.to_string(),(Utc::now()+Duration::minutes(5)).to_rfc3339(),now])?;
            let j = job_by_id(
                &tx,
                Uuid::parse_str(&id).map_err(|e| invalid("job id", &e.to_string()))?,
            )?;
            tx.commit()?;
            Ok(Some(j))
        } else {
            tx.commit()?;
            Ok(None)
        }
    }
    pub fn complete_memory_summary(
        &self,
        i: &CompleteMemorySummaryInput,
    ) -> Result<MemorySummaryJob, StoreError> {
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let job = job_by_id(&tx, i.job_id)?;
        claim_owned(&tx, &job, i.owner_boot_id, i.expected_attempt)?;
        for r in &i.records {
            if r.project_id != job.project_id
                || r.source_turn_id != job.source_turn_id
                || r.source_hash != job.source_hash
            {
                return Err(invalid("memory record", "must match claimed source"));
            }
            insert_record(&tx, r)?;
        }
        let now = Utc::now().to_rfc3339();
        tx.execute("UPDATE memory_summary_jobs SET state='completed',lease_owner_boot_id=NULL,updated_at=?2 WHERE id=?1",params![i.job_id.to_string(),now])?;
        // Completion and attempt state must commit together.  Recovery treats a submitted
        // attempt as uncertain, so a completed job may never leave that link submitted.
        tx.execute(
            "UPDATE memory_summary_attempts SET state='completed',finished_at=?3 WHERE job_id=?1 AND attempt=?2",
            params![i.job_id.to_string(), i.expected_attempt, now],
        )?;
        employee::insert_event(
            &tx,
            "memory.summary-completed",
            &json!({"jobId":i.job_id}),
            &now,
        )?;
        let out = job_by_id(&tx, i.job_id)?;
        tx.commit()?;
        Ok(out)
    }
    pub fn fail_memory_summary(
        &self,
        i: &FailMemorySummaryInput,
    ) -> Result<MemorySummaryJob, StoreError> {
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let job = job_by_id(&tx, i.job_id)?;
        claim_owned(&tx, &job, i.owner_boot_id, i.expected_attempt)?;
        let now = Utc::now();
        let failed = job.attempts >= 5;
        let due = now
            + Duration::seconds(if failed {
                0
            } else {
                backoff(&job.source_hash, job.attempts)
            });
        let msg = redact_bounded(&i.message, 512);
        tx.execute("UPDATE memory_summary_jobs SET state=?2,lease_owner_boot_id=NULL,due_at=?3,failure_message=?4,updated_at=?5 WHERE id=?1",params![i.job_id.to_string(),if failed{"failed"}else{"pending"},due.to_rfc3339(),msg,now.to_rfc3339()])?;
        let out = job_by_id(&tx, i.job_id)?;
        tx.commit()?;
        Ok(out)
    }
    pub fn recover_memory_summaries(&self, boot: Uuid) -> Result<u64, StoreError> {
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().to_rfc3339();
        let n=tx.execute("UPDATE memory_summary_jobs SET state=CASE WHEN attempts>=5 THEN 'failed' ELSE 'pending' END,lease_owner_boot_id=NULL,updated_at=?2 WHERE state='claimed' AND lease_owner_boot_id<>?1 AND NOT EXISTS (SELECT 1 FROM memory_summary_attempts a WHERE a.job_id=memory_summary_jobs.id AND a.state IN ('submitted','uncertain'))",params![boot.to_string(),now])?;
        tx.commit()?;
        Ok(n as u64)
    }
    /// Returns the bounded, scope-local durable summary queue.  This is intentionally a
    /// diagnostic surface: it exposes pending/failed work without claiming or changing it.
    pub fn list_memory_summary_jobs(
        &self,
        project_id: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<MemorySummaryJob>, StoreError> {
        if limit == 0 || limit > 100 {
            return Err(invalid("memory queue limit", "must be between 1 and 100"));
        }
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id,source_hash,source_turn_id,project_id,state,attempts,due_at,lease_owner_boot_id,created_at,updated_at,failure_message FROM memory_summary_jobs WHERE scope_key=?1 ORDER BY CASE state WHEN 'failed' THEN 0 WHEN 'pending' THEN 1 WHEN 'claimed' THEN 2 ELSE 3 END,updated_at DESC,id LIMIT ?2",
        )?;
        let rows =
            statement.query_map(params![scope_key(project_id), i64::from(limit)], job_row)?;
        rows.map(|row| row.map_err(StoreError::from)).collect()
    }
    pub fn list_memory_archive_errors(
        &self,
        project_id: Option<Uuid>,
    ) -> Result<Vec<MemoryArchiveError>, StoreError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare("SELECT source_turn_id,source_hash,archive_name,compressed_bytes FROM memory_archive_intents WHERE scope_key=?1 AND state='error' ORDER BY created_at,source_turn_id")?;
        let rows = statement.query_map([scope_key(project_id)], |row| {
            Ok(MemoryArchiveError {
                source_turn_id: Uuid::parse_str(&row.get::<_, String>(0)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                source_hash: row.get(1)?,
                archive_name: row.get(2)?,
                compressed_bytes: row.get::<_, i64>(3)?.cast_unsigned(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }
    pub fn preview_memory_cleanup(
        &self,
        project_id: Option<Uuid>,
        before: &str,
    ) -> Result<MemoryCleanupPreview, StoreError> {
        let c = self.lock()?;
        let mut st=c.prepare("SELECT archive_name,source_hash,compressed_bytes FROM memory_archives WHERE scope_key=?1 AND created_at<?2 ORDER BY created_at")?;
        let rows = st.query_map(params![scope_key(project_id), before], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        let mut names = vec![];
        let mut hashes = vec![];
        let mut bytes = 0;
        for x in rows {
            let (a, b, c) = x?;
            names.push(a);
            hashes.push(b);
            bytes +=
                u64::try_from(c).map_err(|_| invalid("memory cleanup", "invalid byte count"))?;
        }
        Ok(MemoryCleanupPreview {
            project_id,
            archive_names: names,
            source_hashes: hashes,
            bytes,
        })
    }
    pub fn apply_memory_cleanup(
        &self,
        root: &Path,
        preview: &MemoryCleanupPreview,
    ) -> Result<u64, StoreError> {
        let root = validate_archive_root(root)?;
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if preview.archive_names.len() != preview.source_hashes.len() {
            return Err(invalid(
                "memory cleanup",
                "plan has mismatched archive identities",
            ));
        }
        for (name, hash) in preview.archive_names.iter().zip(&preview.source_hashes) {
            let current: Option<String> = tx.query_row(
                "SELECT archive_name FROM memory_archives WHERE scope_key=?1 AND source_hash=?2",
                params![scope_key(preview.project_id), hash],
                |row| row.get(0),
            ).optional()?;
            if current.as_deref() != Some(name) {
                return Err(invalid("memory cleanup", "preview is stale"));
            }
            tx.execute("INSERT OR REPLACE INTO memory_archive_intents(source_turn_id,source_kind,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,state,created_at) SELECT source_turn_id,source_kind,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,'cleanup-pending',created_at FROM memory_archives WHERE scope_key=?1 AND source_hash=?2", params![scope_key(preview.project_id), hash])?;
        }
        tx.commit()?;
        drop(c);
        let mut removed = 0;
        for (n, h) in preview.archive_names.iter().zip(&preview.source_hashes) {
            if Path::new(n).file_name().and_then(|x| x.to_str()) != Some(n.as_str())
                || !Path::new(n)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zst"))
            {
                return Err(invalid("archive name", "unsafe path"));
            }
            let p = root.join(n);
            if p.exists() {
                let md = fs::symlink_metadata(&p).map_err(|e| ioerr(&p, e))?;
                if md.file_type().is_symlink() {
                    return Err(invalid("archive path", "symlink refused"));
                }
                fs::remove_file(&p).map_err(|e| ioerr(&p, e))?;
            }
            let mut c = self.lock()?;
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            removed += tx.execute(
                "DELETE FROM memory_archives WHERE scope_key=?1 AND source_hash=?2",
                params![scope_key(preview.project_id), h],
            )?;
            tx.execute("DELETE FROM memory_archive_intents WHERE state='cleanup-pending' AND scope_key=?1 AND source_hash=?2",params![scope_key(preview.project_id),h])?;
            tx.commit()?;
        }
        Ok(removed as u64)
    }
}

/// Builds the bounded NDJSON body: the header that keys the archive to its turn, each redacted
/// frame that still fits under the cap with room for the trailer, and the trailer whenever a
/// frame was dropped here or by the daemon's capture. Returns the body and the dropped count.
fn raw_frames_source(
    input: ArchiveMemoryRawFramesInput,
    cap: u64,
) -> Result<(Vec<u8>, u64), StoreError> {
    let limit = usize::try_from(cap).map_err(|_| invalid("memory source", "invalid source cap"))?;
    let trailer = |dropped: u64| {
        let mut line =
            json!({"type":"lazoo-raw-frames-truncated","droppedFrames":dropped,"limitBytes":cap})
                .to_string();
        line.push('\n');
        line
    };
    let mut source =
        json!({"type":"lazoo-raw-frames","version":1,"sourceTurnId":input.source_turn_id})
            .to_string();
    source.push('\n');
    // Reserve the widest trailer up front so appending it can never breach the cap.
    let reserve = trailer(u64::MAX).len();
    if source.len() + reserve > limit {
        return Err(invalid(
            "memory source",
            "configured byte cap cannot hold the raw-frame header and trailer",
        ));
    }
    let mut dropped = input.dropped_frames;
    for mut frame in input.frames {
        redact_value(&mut frame);
        let line = frame.to_string();
        if source.len() + line.len() + 1 + reserve > limit {
            dropped += 1;
            continue;
        }
        source.push_str(&line);
        source.push('\n');
    }
    if dropped > 0 {
        source.push_str(&trailer(dropped));
    }
    Ok((source.into_bytes(), dropped))
}
fn turn_scope(s: &Store, id: Uuid) -> Result<Option<Uuid>, StoreError> {
    let c = s.lock()?;
    let x:Option<String>=c.query_row("SELECT c.project_id FROM conversation_turns t JOIN conversations c ON c.id=t.conversation_id WHERE t.id=?1",[id.to_string()],|r|r.get(0)).optional()?.flatten();
    x.map(|v| Uuid::parse_str(&v).map_err(|e| invalid("conversation project", &e.to_string())))
        .transpose()
}
fn scope_key(p: Option<Uuid>) -> String {
    p.map(|x| x.to_string()).unwrap_or_default()
}
fn scope_filename(project_id: Option<Uuid>) -> String {
    project_id.map_or_else(|| "home".to_owned(), |id| id.simple().to_string())
}
fn policy_from(c: &rusqlite::Connection, p: Option<Uuid>) -> Result<MemoryPolicy, StoreError> {
    policy_from_scope(c, p, &scope_key(p))
}
#[allow(clippy::many_single_char_names)]
fn policy_from_scope(
    c: &rusqlite::Connection,
    p: Option<Uuid>,
    scope: &str,
) -> Result<MemoryPolicy, StoreError> {
    let row=c.query_row("SELECT revision,retention_enabled,retention_days,quota_bytes,max_source_bytes,allowed_types_json,inject_enabled FROM memory_policies WHERE scope_key=?1",[scope],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?,r.get::<_,i64>(6)?))).optional()?;
    match row {
        Some((
            revision,
            retention_enabled,
            retention_days,
            quota_bytes,
            max_source_bytes,
            allowed_types,
            inject_enabled,
        )) => Ok(MemoryPolicy {
            project_id: p,
            revision: u64::try_from(revision)
                .map_err(|_| invalid("memory policy", "invalid revision"))?,
            retention_enabled: retention_enabled == 1,
            retention_days: u32::try_from(retention_days)
                .map_err(|_| invalid("memory policy", "invalid retention days"))?,
            quota_bytes: u64::try_from(quota_bytes)
                .map_err(|_| invalid("memory policy", "invalid quota"))?,
            max_source_bytes: u64::try_from(max_source_bytes)
                .map_err(|_| invalid("memory policy", "invalid source cap"))?,
            allowed_types: serde_json::from_str(&allowed_types)?,
            inject_enabled: inject_enabled == 1,
        }),
        None => Ok(MemoryPolicy {
            project_id: p,
            revision: 1,
            retention_enabled: true,
            retention_days: 30,
            quota_bytes: DEFAULT_QUOTA,
            max_source_bytes: DEFAULT_SOURCE_CAP,
            allowed_types: vec![
                MemoryRecordType::Decision,
                MemoryRecordType::Action,
                MemoryRecordType::Achievement,
                MemoryRecordType::Question,
                MemoryRecordType::Failure,
                MemoryRecordType::Constraint,
                MemoryRecordType::Fact,
                MemoryRecordType::Summary,
            ],
            inject_enabled: true,
        }),
    }
}
fn policy_exists(connection: &rusqlite::Connection, scope: &str) -> Result<bool, StoreError> {
    Ok(connection.query_row(
        "SELECT COUNT(*) FROM memory_policies WHERE scope_key=?1",
        [scope],
        |row| row.get::<_, i64>(0),
    )? > 0)
}
fn insert_record(
    tx: &rusqlite::Transaction<'_>,
    i: &CreateMemoryRecordInput,
) -> Result<MemoryRecord, StoreError> {
    let text = redact(&i.text);
    if text.len() > 8192 || !(0.0..=1.0).contains(&i.confidence) || i.importance > 100 {
        return Err(invalid("memory record", "invalid text or confidence"));
    }
    let archived: Option<String> = tx
        .query_row(
            "SELECT source_hash FROM memory_archives WHERE source_turn_id=?1 AND source_kind='turn'",
            [i.source_turn_id.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    if archived.as_deref() != Some(&i.source_hash)
        || archive_scope(tx, i.source_turn_id, TURN_KIND)? != i.project_id
    {
        return Err(invalid("memory source", "archive/hash missing"));
    }
    let now = Utc::now().to_rfc3339();
    tx.execute("INSERT INTO memory_records(id,revision,project_id,scope_key,source_turn_id,source_hash,record_type,text,confidence,importance,created_at,active) VALUES(?1,1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1)",params![i.id.to_string(),i.project_id.map(|x|x.to_string()),scope_key(i.project_id),i.source_turn_id.to_string(),i.source_hash,serde_json::to_string(&i.record_type)?.trim_matches('"'),text,i.confidence,i.importance,now])?;
    record_by_id(tx, i.id)
}
type RawMemoryRecord = (
    String,
    i64,
    Option<String>,
    String,
    String,
    String,
    String,
    f64,
    i64,
    String,
    i64,
    Option<String>,
);
fn raw_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawMemoryRecord> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
    ))
}
fn record_from(x: RawMemoryRecord) -> Result<MemoryRecord, StoreError> {
    Ok(MemoryRecord {
        id: Uuid::parse_str(&x.0).map_err(|e| invalid("memory id", &e.to_string()))?,
        revision: u64::try_from(x.1).map_err(|_| invalid("memory record", "invalid revision"))?,
        project_id: x
            .2
            .map(|v| Uuid::parse_str(&v).map_err(|e| invalid("memory project", &e.to_string())))
            .transpose()?,
        source_turn_id: Uuid::parse_str(&x.3)
            .map_err(|e| invalid("source turn", &e.to_string()))?,
        source_hash: x.4,
        record_type: serde_json::from_str(&format!("\"{}\"", x.5))?,
        text: x.6,
        confidence: x.7,
        importance: u8::try_from(x.8)
            .map_err(|_| invalid("memory record", "invalid importance"))?,
        created_at: x.9,
        active: x.10 == 1,
        superseded_at: x.11,
    })
}
fn record_by_id(tx: &rusqlite::Transaction<'_>, id: Uuid) -> Result<MemoryRecord, StoreError> {
    record_from(tx.query_row("SELECT id,revision,project_id,source_turn_id,source_hash,record_type,text,confidence,importance,created_at,active,superseded_at FROM memory_records WHERE id=?1",[id.to_string()],raw_record)?)
}
fn archive_name(kind: &str, scope: Option<Uuid>, hash: &str) -> String {
    let scope = scope_filename(scope);
    if kind == RAW_FRAMES_KIND {
        format!("{scope}-raw-{hash}.zst")
    } else {
        format!("{scope}-{hash}.zst")
    }
}
fn archive_from_tx(
    tx: &rusqlite::Transaction<'_>,
    id: Uuid,
    kind: &str,
) -> Result<MemorySourceArchive, StoreError> {
    tx.query_row("SELECT source_turn_id,source_hash,project_id,archive_name,compressed_bytes,uncompressed_bytes,truncated,created_at FROM memory_archives WHERE source_turn_id=?1 AND source_kind=?2",params![id.to_string(),kind],|r|Ok(MemorySourceArchive{source_turn_id:Uuid::parse_str(&r.get::<_,String>(0)?).unwrap(),source_hash:r.get(1)?,project_id:r.get::<_,Option<String>>(2)?.map(|v|Uuid::parse_str(&v).unwrap()),archive_name:r.get(3)?,compressed_bytes:r.get::<_,i64>(4)?.cast_unsigned(),uncompressed_bytes:r.get::<_,i64>(5)?.cast_unsigned(),truncated:r.get::<_,i64>(6)?==1,created_at:r.get(7)?})).map_err(StoreError::from)
}
fn archive_from_tx_optional(
    tx: &rusqlite::Transaction<'_>,
    id: Uuid,
    kind: &str,
) -> Result<Option<MemorySourceArchive>, StoreError> {
    tx.query_row("SELECT source_turn_id,source_hash,project_id,archive_name,compressed_bytes,uncompressed_bytes,truncated,created_at FROM memory_archives WHERE source_turn_id=?1 AND source_kind=?2", params![id.to_string(), kind], |r| Ok(MemorySourceArchive { source_turn_id: Uuid::parse_str(&r.get::<_, String>(0)?).unwrap(), source_hash: r.get(1)?, project_id: r.get::<_, Option<String>>(2)?.map(|value| Uuid::parse_str(&value).unwrap()), archive_name: r.get(3)?, compressed_bytes: r.get::<_, i64>(4)?.cast_unsigned(), uncompressed_bytes: r.get::<_, i64>(5)?.cast_unsigned(), truncated: r.get::<_, i64>(6)? == 1, created_at: r.get(7)? })).optional().map_err(StoreError::from)
}
fn archive_scope(
    tx: &rusqlite::Transaction<'_>,
    id: Uuid,
    kind: &str,
) -> Result<Option<Uuid>, StoreError> {
    let value: Option<String> = tx
        .query_row(
            "SELECT project_id FROM memory_archives WHERE source_turn_id=?1 AND source_kind=?2",
            params![id.to_string(), kind],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    value
        .map(|id| {
            Uuid::parse_str(&id).map_err(|error| invalid("memory project", &error.to_string()))
        })
        .transpose()
}
fn job_by_id(tx: &rusqlite::Transaction<'_>, id: Uuid) -> Result<MemorySummaryJob, StoreError> {
    tx.query_row("SELECT id,source_hash,source_turn_id,project_id,state,attempts,due_at,lease_owner_boot_id,created_at,updated_at,failure_message FROM memory_summary_jobs WHERE id=?1",[id.to_string()],job_row).optional()?.ok_or_else(||invalid("memory job","not found"))
}
fn job_for_hash(
    tx: &rusqlite::Transaction<'_>,
    p: Option<Uuid>,
    h: &str,
) -> Result<MemorySummaryJob, StoreError> {
    tx.query_row("SELECT id,source_hash,source_turn_id,project_id,state,attempts,due_at,lease_owner_boot_id,created_at,updated_at,failure_message FROM memory_summary_jobs WHERE scope_key=?1 AND source_hash=?2",params![scope_key(p),h],job_row).map_err(StoreError::from)
}
fn job_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MemorySummaryJob> {
    Ok(MemorySummaryJob {
        id: Uuid::parse_str(&r.get::<_, String>(0)?).unwrap(),
        source_hash: r.get(1)?,
        source_turn_id: Uuid::parse_str(&r.get::<_, String>(2)?).unwrap(),
        project_id: r
            .get::<_, Option<String>>(3)?
            .map(|v| Uuid::parse_str(&v).unwrap()),
        state: serde_json::from_str(&format!("\"{}\"", r.get::<_, String>(4)?)).unwrap(),
        attempts: u32::try_from(r.get::<_, i64>(5)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
        due_at: r.get(6)?,
        lease_owner_boot_id: r
            .get::<_, Option<String>>(7)?
            .map(|v| Uuid::parse_str(&v).unwrap()),
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        failure_message: r.get(10)?,
    })
}
fn claim_owned(
    tx: &rusqlite::Transaction<'_>,
    j: &MemorySummaryJob,
    o: Uuid,
    expected_attempt: u32,
) -> Result<(), StoreError> {
    let now = Utc::now().to_rfc3339();
    let valid: i64 = tx.query_row("SELECT COUNT(*) FROM memory_summary_jobs WHERE id=?1 AND state='claimed' AND lease_owner_boot_id=?2 AND attempts=?3 AND lease_expires_at>?4", params![j.id.to_string(),o.to_string(),expected_attempt,now], |row| row.get(0))?;
    if valid == 1 {
        Ok(())
    } else {
        Err(invalid("memory job", "not claimed by owner"))
    }
}
fn literal_fts(s: &str) -> String {
    s.split_whitespace()
        .map(|x| format!("\"{}\"", x.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}
fn hex_hash(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        })
}
fn verify_archive(path: &Path, expected: &[u8], cap: u64) -> Result<(), StoreError> {
    let file = fs::File::open(path).map_err(|error| ioerr(path, error))?;
    let mut input = zstd::stream::read::Decoder::new(file)
        .map_err(|error| invalid("memory archive", &error.to_string()))?
        .take(cap.saturating_add(1));
    let mut decoded = Vec::new();
    input
        .read_to_end(&mut decoded)
        .map_err(|error| invalid("memory archive", &error.to_string()))?;
    if decoded.len() as u64 > cap || decoded != expected {
        return Err(invalid(
            "memory archive",
            "archive content or bound is invalid",
        ));
    }
    Ok(())
}
fn verify_intent_archive(
    path: &Path,
    source_hash: &str,
    compressed: i64,
    uncompressed: i64,
) -> Result<bool, StoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| ioerr(path, error))?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len()
            != u64::try_from(compressed)
                .map_err(|_| invalid("memory archive", "invalid intent size"))?
    {
        return Ok(false);
    }
    let file = fs::File::open(path).map_err(|error| ioerr(path, error))?;
    let Ok(decoder) = zstd::stream::read::Decoder::new(file) else {
        return Ok(false);
    };
    let mut decoder = decoder.take(
        u64::try_from(uncompressed)
            .map_err(|_| invalid("memory archive", "invalid intent size"))?
            .saturating_add(1),
    );
    let mut bytes = Vec::new();
    if decoder.read_to_end(&mut bytes).is_err() {
        return Ok(false);
    }
    Ok(i64::try_from(bytes.len()).ok() == Some(uncompressed) && hex_hash(&bytes) == source_hash)
}
fn redact(s: &str) -> String {
    let mut x = s.to_owned();
    for key in ["sk-", "api_key", "token=", "access_token", "refresh_token"] {
        if let Some(i) = x.to_lowercase().find(key) {
            x.replace_range(i..x.len(), "[redacted]");
            break;
        }
    }
    x
}
fn redact_bounded(s: &str, limit: usize) -> String {
    redact(s).chars().take(limit).collect()
}
fn redact_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => *text = redact(text),
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact_value),
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(
                    key.to_ascii_lowercase().as_str(),
                    "api_key" | "apikey" | "authorization" | "token" | "secret" | "password"
                ) {
                    *value = serde_json::Value::String("[redacted]".to_owned());
                } else {
                    redact_value(value);
                }
            }
        }
        _ => {}
    }
}
fn backoff(h: &str, a: u32) -> i64 {
    let n = u64::from_str_radix(&h[..8.min(h.len())], 16).unwrap_or(0) % 31;
    (1_i64 << a.min(10)) * 30 + i64::try_from(n).unwrap_or(0)
}
fn validate_archive_root(p: &Path) -> Result<PathBuf, StoreError> {
    if p.as_os_str().is_empty() {
        return Err(invalid("archive root", "empty"));
    }
    for ancestor in p.ancestors() {
        if ancestor.exists()
            && fs::symlink_metadata(ancestor)
                .map_err(|error| ioerr(ancestor, error))?
                .file_type()
                .is_symlink()
        {
            return Err(invalid("archive root", "symlink/reparse ancestor refused"));
        }
    }
    fs::create_dir_all(p).map_err(|error| ioerr(p, error))?;
    let canonical = fs::canonicalize(p).map_err(|error| ioerr(p, error))?;
    let mut cursor = canonical.as_path();
    while let Some(parent) = cursor.parent() {
        let metadata = fs::symlink_metadata(cursor).map_err(|error| ioerr(cursor, error))?;
        if metadata.file_type().is_symlink() {
            return Err(invalid("archive root", "symlink/reparse ancestor refused"));
        }
        cursor = parent;
    }
    Ok(canonical)
}
fn invalid(field: &'static str, msg: &str) -> StoreError {
    StoreError::InvalidRecord {
        field,
        message: msg.to_owned(),
    }
}
fn ioerr(path: &Path, e: std::io::Error) -> StoreError {
    StoreError::Io {
        path: path.to_path_buf(),
        source: e,
    }
}
