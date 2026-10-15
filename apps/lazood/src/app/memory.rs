use lazoo_protocol::{MemoryRecordType, MemorySearchParams, RpcError};
use lazoo_store::{ArchiveMemoryTurnInput, SetMemoryPolicyInput};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::Duration;
use uuid::Uuid;

use super::{App, invalid_params, store_error, to_value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ArchiveParams {
    source_turn_id: Uuid,
    project_id: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CleanupParams {
    project_id: Option<Uuid>,
    before: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CleanupApplyParams {
    preview: lazoo_protocol::MemoryCleanupPreview,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct QueueStatusParams {
    project_id: Option<Uuid>,
    #[serde(default = "default_queue_limit")]
    limit: u32,
}
const fn default_queue_limit() -> u32 {
    20
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PolicySetParams {
    #[serde(default)]
    global: bool,
    project_id: Option<Uuid>,
    expected_revision: u64,
    retention_enabled: bool,
    retention_days: u32,
    quota_bytes: u64,
    max_source_bytes: u64,
    allowed_types: Vec<MemoryRecordType>,
    inject_enabled: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Scope {
    #[serde(default)]
    global: bool,
    project_id: Option<Uuid>,
}

impl App {
    pub(super) fn memory_archive_root(&self) -> std::path::PathBuf {
        self.root.root().join("memory-archives")
    }
    pub(super) fn memory_policy_show(&self, params: &Value) -> Result<Value, RpcError> {
        let scope = optional_scope(params)?;
        Ok(to_value(if scope.global {
            self.store.global_memory_policy().map_err(store_error)?
        } else {
            self.store
                .memory_policy(scope.project_id)
                .map_err(store_error)?
        }))
    }
    pub(super) fn memory_policy_set(&self, params: &Value) -> Result<Value, RpcError> {
        let input: PolicySetParams =
            serde_json::from_value(params.clone()).map_err(|e| invalid_params(e.to_string()))?;
        if input.global && input.project_id.is_some() {
            return Err(invalid_params(
                "global and projectId are mutually exclusive",
            ));
        }
        let store_input = SetMemoryPolicyInput {
            project_id: input.project_id,
            expected_revision: input.expected_revision,
            retention_enabled: input.retention_enabled,
            retention_days: input.retention_days,
            quota_bytes: input.quota_bytes,
            max_source_bytes: input.max_source_bytes,
            allowed_types: input.allowed_types,
            inject_enabled: input.inject_enabled,
        };
        let policy = if input.global {
            self.store.set_global_memory_policy(&store_input)
        } else {
            self.store.set_memory_policy(&store_input)
        }
        .map_err(store_error)?;
        Ok(to_value(policy))
    }
    pub(super) fn memory_archive(&self, params: &Value) -> Result<Value, RpcError> {
        let input: ArchiveParams =
            serde_json::from_value(params.clone()).map_err(|e| invalid_params(e.to_string()))?;
        let archive = self
            .store
            .archive_memory_turn(&ArchiveMemoryTurnInput {
                source_turn_id: input.source_turn_id,
                project_id: input.project_id,
                archive_root: self.memory_archive_root(),
            })
            .map_err(store_error)?;
        let job = self
            .store
            .enqueue_memory_summary(&archive)
            .map_err(store_error)?;
        Ok(to_value(json!({"archive": archive, "job": job})))
    }
    pub(super) fn memory_search(&self, params: &Value) -> Result<Value, RpcError> {
        let input: MemorySearchParams =
            serde_json::from_value(params.clone()).map_err(|e| invalid_params(e.to_string()))?;
        Ok(to_value(
            self.store.search_memory(&input).map_err(store_error)?,
        ))
    }
    pub(super) fn memory_context(&self, params: &Value) -> Result<Value, RpcError> {
        let input: MemorySearchParams =
            serde_json::from_value(params.clone()).map_err(|e| invalid_params(e.to_string()))?;
        Ok(to_value(
            self.store.memory_context(&input).map_err(store_error)?,
        ))
    }
    pub(super) fn memory_cleanup_preview(&self, params: &Value) -> Result<Value, RpcError> {
        let input: CleanupParams =
            serde_json::from_value(params.clone()).map_err(|e| invalid_params(e.to_string()))?;
        Ok(to_value(
            self.store
                .preview_memory_cleanup(input.project_id, &input.before)
                .map_err(store_error)?,
        ))
    }
    pub(super) fn memory_cleanup_apply(&self, params: &Value) -> Result<Value, RpcError> {
        let input: CleanupApplyParams =
            serde_json::from_value(params.clone()).map_err(|e| invalid_params(e.to_string()))?;
        Ok(to_value(
            json!({"removed": self.store.apply_memory_cleanup(&self.memory_archive_root(), &input.preview).map_err(store_error)?}),
        ))
    }
    pub(super) fn memory_queue_status(&self, params: &Value) -> Result<Value, RpcError> {
        let input: QueueStatusParams =
            serde_json::from_value(params.clone()).map_err(|e| invalid_params(e.to_string()))?;
        let jobs = self
            .store
            .list_memory_summary_jobs(input.project_id, input.limit)
            .map_err(store_error)?;
        let archive_errors = self
            .store
            .list_memory_archive_errors(input.project_id)
            .map_err(store_error)?;
        Ok(to_value(
            json!({"jobs":jobs,"archiveErrors":archive_errors,"archiveErrorCount":archive_errors.len()}),
        ))
    }
    pub(super) fn memory_recover(&self) -> Result<(), lazoo_store::StoreError> {
        self.store
            .reconcile_memory_archives(&self.memory_archive_root())?;
        self.store.recover_linked_memory_summary_attempts()?;
        self.store.recover_memory_summaries(self.boot_id)?;
        Ok(())
    }

    /// One serial, bounded sweep keeps compression outside the synchronous provider observer.
    pub(crate) async fn run_memory_archiver(&self) {
        let mut ticks = tokio::time::interval(Duration::from_secs(60));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            if let Err(error) = self.run_memory_archiver_once().await {
                tracing::warn!(%error, "memory archive sweep failed");
            }
        }
    }

    pub(crate) async fn run_memory_archiver_once(&self) -> Result<u32, String> {
        if self.background_throttled() {
            return Ok(0);
        }
        let store = self.store.clone();
        let root = self.memory_archive_root();
        tokio::task::spawn_blocking(move || {
            let candidates = store
                .list_memory_archive_candidates(10)
                .map_err(|error| error.to_string())?;
            let mut archived = 0_u32;
            for candidate in candidates {
                if !store
                    .inherited_memory_policy(candidate.project_id)
                    .map_err(|error| error.to_string())?
                    .retention_enabled
                {
                    continue;
                }
                let archive = match store.archive_memory_turn(&ArchiveMemoryTurnInput {
                        source_turn_id: candidate.source_turn_id,
                        project_id: candidate.project_id,
                        archive_root: root.clone(),
                    }) {
                    Ok(archive) => archive,
                    Err(error) => { tracing::warn!(%error, source_turn_id=%candidate.source_turn_id, "memory archive candidate skipped"); continue; }
                };
                if let Err(error) = store.enqueue_memory_summary(&archive) { tracing::warn!(%error, source_turn_id=%candidate.source_turn_id, "memory archive queue enqueue skipped"); continue; }
                archived += 1;
            }
            Ok(archived)
        })
        .await
        .map_err(|error| format!("memory archive task join failed: {error}"))?
    }
}
fn optional_scope(value: &Value) -> Result<Scope, RpcError> {
    let scope = serde_json::from_value::<Scope>(value.clone())
        .map_err(|e| invalid_params(e.to_string()))?;
    if scope.global && scope.project_id.is_some() {
        return Err(invalid_params(
            "global and projectId are mutually exclusive",
        ));
    }
    Ok(scope)
}
