use lazoo_protocol::{
    MemoryCleanupPreview, MemoryQueueState, MemoryRecordType, MemorySearchParams,
    MemorySourceArchive,
};
use lazoo_store::{
    ArchiveMemoryRawFramesInput, ArchiveMemoryTurnInput, FailMemorySummaryInput,
    SetMemoryPolicyInput, Store, StoreError,
};
use sha2::Digest as _;
use std::fmt::Write as _;
use tempfile::tempdir;
use uuid::Uuid;

fn seeded_store() -> (tempfile::TempDir, Store, Uuid, Uuid) {
    let temp = tempdir().unwrap();
    let store = Store::open_path(temp.path().join("memory.db")).unwrap();
    let project = Uuid::new_v4();
    store
        .execute_for_test(&format!(
            "INSERT INTO projects(id,repository_identity,canonical_path,display_name,checkout_mode,configuration_revision,created_at) VALUES('{project}','memory-policy-{project}','C:/memory-policy','memory-policy','manual',1,'2020-01-01T00:00:00Z');"
        ))
        .unwrap();
    let home_turn = Uuid::new_v4();
    let project_turn = Uuid::new_v4();
    let home_conversation = Uuid::new_v4();
    let project_conversation = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let home_id = Uuid::new_v4();
    let project_id = Uuid::new_v4();
    store.execute_for_test(&format!(
        "PRAGMA foreign_keys=OFF;
         INSERT INTO conversations(id,contract_version,project_id,lead_employee_id,lead_employee_configuration_revision,revision,created_at,updated_at) VALUES('{home_conversation}',1,NULL,'{employee}',1,1,'2020-01-01T00:00:00Z','2020-01-01T00:00:00Z');
         INSERT INTO conversations(id,contract_version,project_id,lead_employee_id,lead_employee_configuration_revision,revision,created_at,updated_at) VALUES('{project_conversation}',1,'{project}','{employee}',1,1,'2020-01-01T00:00:00Z','2020-01-01T00:00:00Z');
         INSERT INTO conversation_turns(id,contract_version,conversation_id,author_kind,recipient_employee_id,recipient_employee_configuration_revision,text,message_id,correlation_id,state,revision,transcript_bytes,transcript_truncated,created_at,updated_at) VALUES('{home_turn}',1,'{home_conversation}','user','{employee}',1,'home','{home_turn}','{home_turn}','completed',1,0,0,'2020-01-01T00:00:00Z','2020-01-01T00:00:00Z');
         INSERT INTO conversation_turns(id,contract_version,conversation_id,author_kind,recipient_employee_id,recipient_employee_configuration_revision,text,message_id,correlation_id,state,revision,transcript_bytes,transcript_truncated,created_at,updated_at) VALUES('{project_turn}',1,'{project_conversation}','user','{employee}',1,'project','{project_turn}','{project_turn}','completed',1,0,0,'2020-01-01T00:00:00Z','2020-01-01T00:00:00Z');
         INSERT INTO memory_archives(source_turn_id,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,created_at) VALUES ('{home_turn}','homehash',NULL,'','homehash.zst',1,1,0,'2020-01-01T00:00:00Z');
         INSERT INTO memory_archives(source_turn_id,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,created_at) VALUES ('{project_turn}','projecthash','{project}','{project}','projecthash.zst',1,1,0,'2020-01-01T00:00:00Z');
         INSERT INTO memory_records(id,revision,project_id,scope_key,source_turn_id,source_hash,record_type,text,confidence,importance,created_at,active) VALUES ('{home_id}',1,NULL,'','{home_turn}','homehash','fact','C++ literal quote phrase',1,10,'2020-01-01T00:00:00Z',1);
         INSERT INTO memory_records(id,revision,project_id,scope_key,source_turn_id,source_hash,record_type,text,confidence,importance,created_at,active) VALUES ('{project_id}',1,'{project}','{project}','{project_turn}','projecthash','constraint','project isolated architecture',1,99,'2020-01-01T00:00:00Z',1);
         PRAGMA foreign_keys=ON;"
    )).unwrap();
    (temp, store, project, home_turn)
}

#[test]
fn memory_policy_is_persistent_and_revision_checked() {
    let temp = tempdir().unwrap();
    let db = temp.path().join("memory.db");
    let store = Store::open_path(&db).unwrap();
    let default = store.memory_policy(None).unwrap();
    assert_eq!(default.quota_bytes, 256 * 1024 * 1024);
    let updated = store
        .set_memory_policy(&SetMemoryPolicyInput {
            project_id: None,
            expected_revision: default.revision,
            retention_enabled: false,
            retention_days: 31,
            quota_bytes: 1_048_576,
            max_source_bytes: 65_536,
            allowed_types: vec![MemoryRecordType::Fact, MemoryRecordType::Constraint],
            inject_enabled: false,
        })
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert!(!updated.retention_enabled);
    drop(store);
    let reopened = Store::open_path(&db).unwrap();
    assert_eq!(reopened.memory_policy(None).unwrap(), updated);
    assert!(matches!(
        reopened.set_memory_policy(&SetMemoryPolicyInput {
            project_id: None,
            expected_revision: 1,
            retention_enabled: true,
            retention_days: 30,
            quota_bytes: 1,
            max_source_bytes: 1,
            allowed_types: vec![],
            inject_enabled: true,
        }),
        Err(StoreError::InvalidRecord {
            field: "memory policy revision",
            ..
        })
    ));
}

#[test]
fn global_policy_is_inherited_only_until_a_scope_has_its_own_override() {
    let temp = tempdir().unwrap();
    let store = Store::open_path(temp.path().join("memory.db")).unwrap();
    let global = store.global_memory_policy().unwrap();
    store
        .set_global_memory_policy(&SetMemoryPolicyInput {
            project_id: None,
            expected_revision: global.revision,
            retention_enabled: false,
            retention_days: global.retention_days,
            quota_bytes: global.quota_bytes,
            max_source_bytes: global.max_source_bytes,
            allowed_types: global.allowed_types.clone(),
            inject_enabled: global.inject_enabled,
        })
        .unwrap();
    let project = Uuid::new_v4();
    store
        .execute_for_test(&format!(
            "INSERT INTO projects(id,repository_identity,canonical_path,display_name,checkout_mode,configuration_revision,created_at) VALUES('{project}','global-policy-{project}','C:/global-policy','global-policy','manual',1,'2020-01-01T00:00:00Z');"
        ))
        .unwrap();
    assert!(
        !store
            .inherited_memory_policy(Some(project))
            .unwrap()
            .retention_enabled
    );
    let local = store.memory_policy(Some(project)).unwrap();
    store
        .set_memory_policy(&SetMemoryPolicyInput {
            project_id: Some(project),
            expected_revision: local.revision,
            retention_enabled: true,
            retention_days: local.retention_days,
            quota_bytes: local.quota_bytes,
            max_source_bytes: local.max_source_bytes,
            allowed_types: local.allowed_types,
            inject_enabled: local.inject_enabled,
        })
        .unwrap();
    assert!(
        store
            .inherited_memory_policy(Some(project))
            .unwrap()
            .retention_enabled
    );
}

#[test]
#[allow(clippy::too_many_lines)] // one fixture establishes every durable source relationship
fn memory_search_is_scope_isolated_and_fts_metacharacters_are_literals() {
    let (_temp, store, project, home_turn) = seeded_store();
    let source = store.get_turn(home_turn).unwrap();
    let task = Uuid::new_v4();
    let message = Uuid::new_v4();
    let owner = Uuid::new_v4();
    store.execute_for_test(&format!("PRAGMA foreign_keys=OFF;
        UPDATE conversation_turns SET task_id='{task}' WHERE id='{home_turn}';
        INSERT INTO tasks(id,contract_version,root_task_id,parent_task_id,depth,project_id,conversation_id,creation_turn_id,delegation_message_id,creator_kind,creator_employee_id,creator_session_id,assigned_employee_id,role,objective,references_json,requested_authority,effective_authority,employee_configuration_revision,roster_revision,state,revision,active_turn_id,active_session_id,failure_code,failure_message,created_at,updated_at,terminal_at) VALUES('{task}',1,'{task}',NULL,0,NULL,'{}',NULL,NULL,'user',NULL,NULL,'{}','lead','memory filter','[]','ask','ask',1,1,'completed',1,NULL,NULL,NULL,NULL,'2020-01-01T00:00:00Z','2020-01-01T00:00:00Z','2020-01-01T00:00:00Z');
        INSERT INTO task_dispatches(task_id,message_id,owner_boot_id,project_id,project_configuration_revision,workspace_kind,workspace_path,base_commit,branch_name,turn_id,session_id,state,revision,created_at,updated_at) VALUES('{task}','{message}','{owner}',NULL,NULL,'git-worktree','C:/memory','base','memory-filter-branch',NULL,NULL,'workspace-ready',1,'2020-01-01T00:00:00Z','2020-01-01T00:00:00Z'); PRAGMA foreign_keys=ON;", source.conversation_id, source.recipient.employee_id)).unwrap();
    let home = store
        .search_memory(&MemorySearchParams {
            project_id: None,
            employee_id: None,
            conversation_id: None,
            task_id: None,
            root_task_id: None,
            branch: None,
            query: "C++ literal".into(),
            record_types: vec![],
            limit: 20,
        })
        .unwrap();
    assert_eq!(home.len(), 1);
    assert_eq!(home[0].project_id, None);
    let scoped_source = store
        .search_memory(&MemorySearchParams {
            project_id: None,
            employee_id: Some(source.recipient.employee_id),
            conversation_id: Some(source.conversation_id),
            task_id: Some(task),
            root_task_id: Some(task),
            branch: Some("memory-filter-branch".into()),
            query: "literal".into(),
            record_types: vec![MemoryRecordType::Fact],
            limit: 20,
        })
        .unwrap();
    assert_eq!(scoped_source.len(), 1);
    let wrong_employee = store
        .search_memory(&MemorySearchParams {
            employee_id: Some(Uuid::new_v4()),
            ..MemorySearchParams {
                project_id: None,
                employee_id: None,
                conversation_id: None,
                task_id: None,
                root_task_id: None,
                branch: None,
                query: "literal".into(),
                record_types: vec![],
                limit: 20,
            }
        })
        .unwrap();
    assert!(wrong_employee.is_empty());
    let wrong_branch = store
        .search_memory(&MemorySearchParams {
            branch: Some("not-an-authoritative-branch".into()),
            ..MemorySearchParams {
                project_id: None,
                employee_id: None,
                conversation_id: None,
                task_id: None,
                root_task_id: None,
                branch: None,
                query: "literal".into(),
                record_types: vec![],
                limit: 20,
            }
        })
        .unwrap();
    assert!(wrong_branch.is_empty());
    assert!(
        store
            .search_memory(&MemorySearchParams {
                project_id: None,
                employee_id: None,
                conversation_id: None,
                task_id: None,
                root_task_id: None,
                branch: None,
                query: "literal".into(),
                record_types: vec![MemoryRecordType::Constraint],
                limit: 20,
            })
            .unwrap()
            .is_empty()
    );
    let scoped = store
        .search_memory(&MemorySearchParams {
            project_id: Some(project),
            employee_id: None,
            conversation_id: None,
            task_id: None,
            root_task_id: None,
            branch: None,
            query: "project architecture".into(),
            record_types: vec![],
            limit: 20,
        })
        .unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].project_id, Some(project));
    assert!(
        store
            .search_memory(&MemorySearchParams {
                project_id: None,
                employee_id: None,
                conversation_id: None,
                task_id: None,
                root_task_id: None,
                branch: None,
                query: "* OR x".into(),
                record_types: vec![],
                limit: 20
            })
            .is_ok()
    );
    let recent = store
        .search_memory(&MemorySearchParams {
            project_id: None,
            employee_id: None,
            conversation_id: None,
            task_id: None,
            root_task_id: None,
            branch: None,
            query: String::new(),
            record_types: vec![],
            limit: 20,
        })
        .unwrap();
    assert_eq!(recent.len(), 1);
}

#[test]
fn context_is_bounded_and_record_validation_precedes_storage() {
    let (_temp, store, _project, _) = seeded_store();
    let pack = store
        .memory_context(&MemorySearchParams {
            project_id: None,
            employee_id: None,
            conversation_id: None,
            task_id: None,
            root_task_id: None,
            branch: None,
            query: "literal".into(),
            record_types: vec![],
            limit: 100,
        })
        .unwrap();
    assert!(pack.records.len() <= 8);
    assert!(
        pack.records
            .iter()
            .map(|record| record.text.len())
            .sum::<usize>()
            <= 2048
    );
    let policy = store.memory_policy(None).unwrap();
    store
        .set_memory_policy(&SetMemoryPolicyInput {
            project_id: None,
            expected_revision: policy.revision,
            retention_enabled: policy.retention_enabled,
            retention_days: policy.retention_days,
            quota_bytes: policy.quota_bytes,
            max_source_bytes: policy.max_source_bytes,
            allowed_types: vec![MemoryRecordType::Constraint],
            inject_enabled: true,
        })
        .unwrap();
    assert!(
        store
            .memory_context(&MemorySearchParams {
                project_id: None,
                employee_id: None,
                conversation_id: None,
                task_id: None,
                root_task_id: None,
                branch: None,
                query: String::new(),
                record_types: vec![],
                limit: 8,
            })
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
fn cleanup_rejects_stale_or_traversal_plans_before_deleting_files() {
    let (temp, store, _project, _) = seeded_store();
    let root = temp.path().join("archives");
    std::fs::create_dir(&root).unwrap();
    let outside = temp.path().join("outside.txt");
    std::fs::write(&outside, "keep").unwrap();
    let stale = MemoryCleanupPreview {
        project_id: None,
        archive_names: vec!["../outside.txt".into()],
        source_hashes: vec!["homehash".into()],
        bytes: 1,
    };
    assert!(matches!(
        store.apply_memory_cleanup(&root, &stale),
        Err(StoreError::InvalidRecord {
            field: "memory cleanup",
            ..
        })
    ));
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "keep");
}

#[test]
fn queue_dedupes_claims_once_and_recovers_a_lost_owner() {
    let (_temp, store, _project, home_turn) = seeded_store();
    let archive = MemorySourceArchive {
        source_turn_id: home_turn,
        source_hash: "homehash".into(),
        project_id: None,
        archive_name: "homehash.zst".into(),
        compressed_bytes: 1,
        uncompressed_bytes: 1,
        truncated: false,
        created_at: "2020-01-01T00:00:00Z".into(),
    };
    // The store APIs below exercise queue transaction behavior; this fixture's archive was
    // seeded without the unrelated conversation graph.
    store.execute_for_test("PRAGMA foreign_keys=OFF;").unwrap();
    let job = store.enqueue_memory_summary(&archive).unwrap();
    assert_eq!(store.enqueue_memory_summary(&archive).unwrap().id, job.id);
    assert_eq!(
        store.list_memory_summary_jobs(None, 20).unwrap(),
        vec![job.clone()]
    );
    let owner = Uuid::new_v4();
    assert_eq!(
        store.claim_memory_summary(owner).unwrap().unwrap().id,
        job.id
    );
    assert!(
        store
            .claim_memory_summary(Uuid::new_v4())
            .unwrap()
            .is_none()
    );
    assert_eq!(store.recover_memory_summaries(Uuid::new_v4()).unwrap(), 1);
    let claimed = store.claim_memory_summary(Uuid::new_v4()).unwrap().unwrap();
    assert_eq!(claimed.id, job.id);
    assert_eq!(claimed.state, MemoryQueueState::Claimed);
    let failed = store
        .fail_memory_summary(&FailMemorySummaryInput {
            job_id: job.id,
            owner_boot_id: claimed.lease_owner_boot_id.unwrap(),
            expected_attempt: claimed.attempts,
            message: "sk-secret-token".into(),
        })
        .unwrap();
    assert_eq!(failed.state, MemoryQueueState::Pending);
    assert_eq!(failed.failure_message.as_deref(), Some("[redacted]"));
}

#[test]
fn expired_same_boot_lease_cannot_complete_or_fail_with_an_old_claim() {
    let (_temp, store, _project, home_turn) = seeded_store();
    let archive = MemorySourceArchive {
        source_turn_id: home_turn,
        source_hash: "homehash".into(),
        project_id: None,
        archive_name: "homehash.zst".into(),
        compressed_bytes: 1,
        uncompressed_bytes: 1,
        truncated: false,
        created_at: "2020-01-01T00:00:00Z".into(),
    };
    store.execute_for_test("PRAGMA foreign_keys=OFF;").unwrap();
    let job = store.enqueue_memory_summary(&archive).unwrap();
    let owner = Uuid::new_v4();
    let first = store.claim_memory_summary(owner).unwrap().unwrap();
    store
        .execute_for_test(&format!(
            "UPDATE memory_summary_jobs SET lease_expires_at='2000-01-01T00:00:00Z' WHERE id='{}'",
            job.id
        ))
        .unwrap();
    let reclaimed = store.claim_memory_summary(owner).unwrap().unwrap();
    assert!(reclaimed.attempts > first.attempts);
    assert!(matches!(
        store.fail_memory_summary(&FailMemorySummaryInput {
            job_id: job.id,
            owner_boot_id: owner,
            expected_attempt: first.attempts,
            message: "late".into()
        }),
        Err(StoreError::InvalidRecord {
            field: "memory job",
            ..
        })
    ));
    let failed = store
        .fail_memory_summary(&FailMemorySummaryInput {
            job_id: job.id,
            owner_boot_id: owner,
            expected_attempt: reclaimed.attempts,
            message: "current claim".into(),
        })
        .unwrap();
    assert_eq!(failed.state, MemoryQueueState::Pending);
}

#[test]
fn cleanup_pending_intent_replays_after_a_crash_boundary() {
    let (temp, store, _project, home_turn) = seeded_store();
    let root = temp.path().join("recovery-archives");
    std::fs::create_dir(&root).unwrap();
    let name = "homehash.zst";
    std::fs::write(root.join(name), "archive bytes").unwrap();
    store.execute_for_test(&format!(
        "PRAGMA foreign_keys=OFF;
         INSERT INTO memory_archive_intents(source_turn_id,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,state,created_at)
         VALUES('{home_turn}','homehash',NULL,'','{name}',1,1,0,'cleanup-pending','2020-01-01T00:00:00Z');
         PRAGMA foreign_keys=ON;"
    )).unwrap();
    assert_eq!(store.reconcile_memory_archives(&root).unwrap(), 1);
    assert!(!root.join(name).exists());
    let stale = store
        .preview_memory_cleanup(None, "9999-01-01T00:00:00Z")
        .unwrap();
    assert!(stale.archive_names.is_empty());
}

#[test]
fn write_pending_intent_without_a_file_is_dropped_after_a_crash_boundary() {
    let (temp, store, _project, home_turn) = seeded_store();
    let root = temp.path().join("write-recovery");
    std::fs::create_dir(&root).unwrap();
    store.execute_for_test(&format!(
        "PRAGMA foreign_keys=OFF;
         INSERT OR REPLACE INTO memory_archive_intents(source_turn_id,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,state,created_at)
         VALUES('{home_turn}','homehash',NULL,'','missing.zst',1,1,0,'write-pending','2020-01-01T00:00:00Z');
         DELETE FROM memory_archives WHERE source_turn_id='{home_turn}';
         PRAGMA foreign_keys=ON;"
    )).unwrap();
    assert_eq!(store.reconcile_memory_archives(&root).unwrap(), 1);
    assert!(
        store
            .preview_memory_cleanup(None, "9999-01-01T00:00:00Z")
            .unwrap()
            .archive_names
            .is_empty()
    );
}

#[test]
fn tampered_write_pending_file_is_not_promoted_to_archive_metadata() {
    let (temp, store, _project, home_turn) = seeded_store();
    let root = temp.path().join("tampered-recovery");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("tampered.zst"), "not zstd").unwrap();
    store.execute_for_test(&format!(
        "PRAGMA foreign_keys=OFF;
         DELETE FROM memory_archives WHERE source_turn_id='{home_turn}';
         INSERT OR REPLACE INTO memory_archive_intents(source_turn_id,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,state,created_at) VALUES('{home_turn}','homehash',NULL,'','tampered.zst',8,1,0,'write-pending','2020-01-01T00:00:00Z');
         PRAGMA foreign_keys=ON;"
    )).unwrap();
    assert_eq!(store.reconcile_memory_archives(&root).unwrap(), 1);
    assert!(
        store
            .preview_memory_cleanup(None, "9999-01-01T00:00:00Z")
            .unwrap()
            .archive_names
            .is_empty()
    );
}

#[test]
fn quota_rejection_writes_no_file_and_existing_archive_retries_at_quota() {
    let temp = tempdir().unwrap();
    let store = Store::open_path(temp.path().join("quota.db")).unwrap();
    let conversation = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let turn = Uuid::new_v4();
    store.execute_for_test(&format!(
        "PRAGMA foreign_keys=OFF;
         INSERT INTO conversations(id,contract_version,project_id,lead_employee_id,lead_employee_configuration_revision,revision,created_at,updated_at) VALUES('{conversation}',1,NULL,'{employee}',1,1,'2020-01-01T00:00:00Z','2020-01-01T00:00:00Z');
         INSERT INTO conversation_turns(id,contract_version,conversation_id,author_kind,recipient_employee_id,recipient_employee_configuration_revision,text,message_id,correlation_id,state,revision,transcript_bytes,transcript_truncated,created_at,updated_at) VALUES('{turn}',1,'{conversation}','user','{employee}',1,'safe source','{turn}','{turn}','completed',1,0,0,'2020-01-01T00:00:00Z','2020-01-01T00:00:00Z');
         PRAGMA foreign_keys=ON;"
    )).unwrap();
    let root = temp.path().join("quota-archives");
    let policy = store.memory_policy(None).unwrap();
    store
        .set_memory_policy(&SetMemoryPolicyInput {
            project_id: None,
            expected_revision: policy.revision,
            retention_enabled: true,
            retention_days: 30,
            quota_bytes: 1,
            max_source_bytes: 65_536,
            allowed_types: policy.allowed_types.clone(),
            inject_enabled: true,
        })
        .unwrap();
    assert!(matches!(
        store.archive_memory_turn(&ArchiveMemoryTurnInput {
            source_turn_id: turn,
            project_id: None,
            archive_root: root.clone()
        }),
        Err(StoreError::InvalidRecord {
            field: "memory quota",
            ..
        })
    ));
    assert!(!root.exists() || std::fs::read_dir(&root).unwrap().next().is_none());
    let current = store.memory_policy(None).unwrap();
    store
        .set_memory_policy(&SetMemoryPolicyInput {
            project_id: None,
            expected_revision: current.revision,
            retention_enabled: true,
            retention_days: 30,
            quota_bytes: 1_048_576,
            max_source_bytes: 65_536,
            allowed_types: current.allowed_types.clone(),
            inject_enabled: true,
        })
        .unwrap();
    let first = store
        .archive_memory_turn(&ArchiveMemoryTurnInput {
            source_turn_id: turn,
            project_id: None,
            archive_root: root.clone(),
        })
        .unwrap();
    let source = store.read_memory_source(&root, turn).unwrap();
    assert!(
        std::str::from_utf8(&source)
            .unwrap()
            .contains("safe source")
    );
    let current = store.memory_policy(None).unwrap();
    store
        .set_memory_policy(&SetMemoryPolicyInput {
            project_id: None,
            expected_revision: current.revision,
            retention_enabled: true,
            retention_days: 30,
            quota_bytes: first.compressed_bytes,
            max_source_bytes: 65_536,
            allowed_types: current.allowed_types.clone(),
            inject_enabled: true,
        })
        .unwrap();
    assert_eq!(
        store
            .archive_memory_turn(&ArchiveMemoryTurnInput {
                source_turn_id: turn,
                project_id: None,
                archive_root: root
            })
            .unwrap(),
        first
    );
}

/// One completed Home turn in a fresh store, seeded as the quota test does.
fn completed_home_turn(store: &Store) -> Uuid {
    let conversation = Uuid::new_v4();
    let employee = Uuid::new_v4();
    let turn = Uuid::new_v4();
    store.execute_for_test(&format!(
        "PRAGMA foreign_keys=OFF;
         INSERT INTO conversations(id,contract_version,project_id,lead_employee_id,lead_employee_configuration_revision,revision,created_at,updated_at) VALUES('{conversation}',1,NULL,'{employee}',1,1,'2020-01-01T00:00:00Z','2020-01-01T00:00:00Z');
         INSERT INTO conversation_turns(id,contract_version,conversation_id,author_kind,recipient_employee_id,recipient_employee_configuration_revision,text,message_id,correlation_id,state,revision,transcript_bytes,transcript_truncated,created_at,updated_at) VALUES('{turn}',1,'{conversation}','user','{employee}',1,'raw source','{turn}','{turn}','completed',1,0,0,'2020-01-01T00:00:00Z','2020-01-01T00:00:00Z');
         PRAGMA foreign_keys=ON;"
    )).unwrap();
    turn
}

fn raw_frames(count: usize, bytes_each: usize) -> Vec<serde_json::Value> {
    (1..=count)
        .map(|sequence| {
            serde_json::json!({
                "sequence": sequence,
                "type": "textDelta",
                "raw": {"text": "x".repeat(bytes_each), "token": "secret"}
            })
        })
        .collect()
}

fn ndjson_lines(bytes: &[u8]) -> Vec<serde_json::Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn home_policy_with(store: &Store, retention_enabled: bool, quota_bytes: u64, cap: u64) {
    let current = store.memory_policy(None).unwrap();
    store
        .set_memory_policy(&SetMemoryPolicyInput {
            project_id: None,
            expected_revision: current.revision,
            retention_enabled,
            retention_days: 30,
            quota_bytes,
            max_source_bytes: cap,
            allowed_types: current.allowed_types.clone(),
            inject_enabled: true,
        })
        .unwrap();
}

#[test]
fn raw_frame_archive_is_hashed_capped_and_counted_against_quota() {
    let temp = tempdir().unwrap();
    let store = Store::open_path(temp.path().join("raw.db")).unwrap();
    let turn = completed_home_turn(&store);
    let root = temp.path().join("raw-archives");
    home_policy_with(&store, true, 1_048_576, 65_536);
    let input = || ArchiveMemoryRawFramesInput {
        source_turn_id: turn,
        archive_root: root.clone(),
        frames: raw_frames(100, 1000),
        dropped_frames: 0,
    };
    let raw = store.archive_memory_raw_frames(input()).unwrap().unwrap();
    assert!(raw.truncated);
    assert!(raw.uncompressed_bytes <= 65_536);
    assert!(raw.archive_name.contains("-raw-"));
    assert_eq!(raw.source_turn_id, turn);
    assert!(root.join(&raw.archive_name).is_file());

    let bytes = store.read_memory_raw_frames(&root, turn).unwrap();
    assert_eq!(bytes.len() as u64, raw.uncompressed_bytes);
    let digest = sha2::Sha256::digest(&bytes)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            write!(hex, "{byte:02x}").unwrap();
            hex
        });
    assert_eq!(digest, raw.source_hash);
    let lines = ndjson_lines(&bytes);
    assert_eq!(lines[0]["type"], "lazoo-raw-frames");
    assert_eq!(lines[0]["sourceTurnId"], turn.to_string());
    let trailer = lines.last().unwrap();
    assert_eq!(trailer["type"], "lazoo-raw-frames-truncated");
    let dropped = trailer["droppedFrames"].as_u64().unwrap();
    assert!(dropped > 0);
    assert_eq!(trailer["limitBytes"], 65_536);
    assert_eq!(lines.len() as u64, 2 + 100 - dropped);
    assert!(
        lines[1..lines.len() - 1]
            .iter()
            .all(|frame| frame["raw"]["token"] == "[redacted]")
    );
    assert!(!std::str::from_utf8(&bytes).unwrap().contains("secret"));
    // The same frames again return the recorded archive instead of a second file.
    assert_eq!(
        store.archive_memory_raw_frames(input()).unwrap().unwrap(),
        raw
    );

    // Raw frames count against the one quota the canonical archive uses.
    home_policy_with(&store, true, raw.compressed_bytes, 65_536);
    assert!(matches!(
        store.archive_memory_turn(&ArchiveMemoryTurnInput {
            source_turn_id: turn,
            project_id: None,
            archive_root: root.clone(),
        }),
        Err(StoreError::InvalidRecord {
            field: "memory quota",
            ..
        })
    ));
    assert!(store.list_memory_archive_errors(None).unwrap().is_empty());
    assert!(store.read_memory_source(&root, turn).is_err());
}

#[test]
fn raw_frame_archive_respects_retention_opt_out() {
    let temp = tempdir().unwrap();
    let store = Store::open_path(temp.path().join("opt-out.db")).unwrap();
    let turn = completed_home_turn(&store);
    let root = temp.path().join("opt-out-archives");
    let input = |frames: Vec<serde_json::Value>| ArchiveMemoryRawFramesInput {
        source_turn_id: turn,
        archive_root: root.clone(),
        frames,
        dropped_frames: 0,
    };
    home_policy_with(&store, false, 1_048_576, 65_536);
    assert!(matches!(
        store.archive_memory_raw_frames(input(raw_frames(2, 16))),
        Err(StoreError::InvalidRecord {
            field: "memory retention",
            ..
        })
    ));
    assert!(!root.exists() || std::fs::read_dir(&root).unwrap().next().is_none());
    assert!(matches!(
        store.read_memory_raw_frames(&root, turn),
        Err(StoreError::InvalidRecord {
            field: "memory source",
            ..
        })
    ));
    // Nothing captured is nothing to retain, whatever the policy says.
    assert!(
        store
            .archive_memory_raw_frames(input(Vec::new()))
            .unwrap()
            .is_none()
    );
    // Only a completed turn is retained, even with retention back on.
    home_policy_with(&store, true, 1_048_576, 65_536);
    store
        .execute_for_test(&format!(
            "UPDATE conversation_turns SET state='accepted' WHERE id='{turn}'"
        ))
        .unwrap();
    assert!(matches!(
        store.archive_memory_raw_frames(input(raw_frames(2, 16))),
        Err(StoreError::InvalidRecord {
            field: "memory source",
            ..
        })
    ));
    assert!(!root.exists() || std::fs::read_dir(&root).unwrap().next().is_none());
}

#[test]
fn raw_frames_and_canonical_archive_coexist_and_cleanup_removes_both() {
    let temp = tempdir().unwrap();
    let store = Store::open_path(temp.path().join("both.db")).unwrap();
    let turn = completed_home_turn(&store);
    let root = temp.path().join("both-archives");
    let canonical_input = ArchiveMemoryTurnInput {
        source_turn_id: turn,
        project_id: None,
        archive_root: root.clone(),
    };
    let raw = store
        .archive_memory_raw_frames(ArchiveMemoryRawFramesInput {
            source_turn_id: turn,
            archive_root: root.clone(),
            frames: raw_frames(3, 32),
            dropped_frames: 0,
        })
        .unwrap()
        .unwrap();
    assert!(!raw.truncated);
    // A raw archive neither hides the turn from canonical archiving nor becomes a summary source.
    assert!(
        store
            .list_memory_archive_candidates(10)
            .unwrap()
            .iter()
            .any(|candidate| candidate.source_turn_id == turn)
    );
    assert!(store.enqueue_memory_summary(&raw).is_err());

    let canonical = store.archive_memory_turn(&canonical_input).unwrap();
    assert_ne!(canonical.archive_name, raw.archive_name);
    assert!(store.list_memory_archive_candidates(10).unwrap().is_empty());
    store.enqueue_memory_summary(&canonical).unwrap();
    assert!(
        std::str::from_utf8(&store.read_memory_source(&root, turn).unwrap())
            .unwrap()
            .contains("raw source")
    );
    assert_eq!(
        ndjson_lines(&store.read_memory_raw_frames(&root, turn).unwrap()).len(),
        4
    );

    let preview = store
        .preview_memory_cleanup(None, "9999-01-01T00:00:00Z")
        .unwrap();
    assert_eq!(preview.archive_names.len(), 2);
    assert_eq!(
        preview.bytes,
        raw.compressed_bytes + canonical.compressed_bytes
    );
    assert_eq!(store.apply_memory_cleanup(&root, &preview).unwrap(), 2);
    assert!(!root.join(&raw.archive_name).exists());
    assert!(!root.join(&canonical.archive_name).exists());
    assert!(
        store
            .preview_memory_cleanup(None, "9999-01-01T00:00:00Z")
            .unwrap()
            .archive_names
            .is_empty()
    );
    assert!(store.read_memory_source(&root, turn).is_err());
    assert!(store.read_memory_raw_frames(&root, turn).is_err());

    // Recovery replays one kind without touching the other: a raw write-pending intent whose
    // file never landed is dropped while the re-created canonical archive stays readable.
    let canonical = store.archive_memory_turn(&canonical_input).unwrap();
    store.execute_for_test(&format!(
        "PRAGMA foreign_keys=OFF;
         INSERT INTO memory_archive_intents(source_turn_id,source_kind,source_hash,project_id,scope_key,archive_name,compressed_bytes,uncompressed_bytes,truncated,state,created_at)
         VALUES('{turn}','raw-frames','rawhash',NULL,'','home-raw-missing.zst',1,1,0,'write-pending','2020-01-01T00:00:00Z');
         PRAGMA foreign_keys=ON;"
    )).unwrap();
    assert_eq!(store.reconcile_memory_archives(&root).unwrap(), 1);
    assert_eq!(
        store.read_memory_source(&root, turn).unwrap().len() as u64,
        canonical.uncompressed_bytes
    );
    assert!(store.read_memory_raw_frames(&root, turn).is_err());
    assert!(store.list_memory_archive_errors(None).unwrap().is_empty());
}
