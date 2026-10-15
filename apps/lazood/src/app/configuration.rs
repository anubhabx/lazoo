//! Server-owned provider configuration orchestration.
//!
//! The RPC never accepts a receipt or a filesystem path. It persists a safe selection and
//! metadata preview, re-renders from that selection immediately before apply, then records intent
//! before the native write. Prepared retries inspect the server-owned receipt before any write.

use lazoo_providers::{
    ConfigurationCompatibility, ConfigurationPreviewMetadata, ProviderKind, ProviderProbe,
    ProviderVerification, SelectedMcpIntegration, apply_configuration_preview_with_intent,
    preview_configuration_apply,
};
use lazoo_store::{
    BeginConfigurationApplyInput, CreateConfigurationPreviewInput, RecordConfigurationReceiptInput,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{App, RpcError, invalid_params, provider_error, store_error, to_value};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConfigurationPreviewParams {
    pub selection: SelectedMcpIntegration,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ConfigurationOperationParams {
    pub operation_id: Uuid,
    pub expected_revision: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigurationShowParams {
    operation_id: Uuid,
}

impl App {
    /// Startup recovery is inspection-only. A prepared operation is never replayed or restored
    /// automatically because a different process may have changed the provider file.
    pub(super) fn configuration_recover_startup(&self) -> Result<(), lazoo_store::StoreError> {
        for operation in self.store.configuration_recovery_candidates()? {
            let (state, observed) = match operation.receipt.as_ref().and_then(|value| {
                serde_json::from_value::<lazoo_providers::ConfigurationApplyReceipt>(value.clone())
                    .ok()
            }) {
                // ApplyIntent is written before backup preparation, so it proves native mutation
                // did not start. Release its active-target lock rather than treating it as drift.
                None => (
                    lazoo_store::ConfigurationOperationState::RolledBack,
                    json!({"recovery":"intent-without-receipt-no-mutation"}),
                ),
                Some(receipt) => match lazoo_providers::inspect_configuration_apply_recovery(
                    &self.provider_environment,
                    self.configuration_backup_root(),
                    &receipt,
                ) {
                    Ok(lazoo_providers::ConfigurationRecoveryState::OriginalPresent) => (
                        lazoo_store::ConfigurationOperationState::RolledBack,
                        json!({"recovery":"original-present","target":receipt.target}),
                    ),
                    Ok(lazoo_providers::ConfigurationRecoveryState::AppliedPresent) => (
                        lazoo_store::ConfigurationOperationState::RecoveryNeeded,
                        json!({"recovery":"applied-present","target":receipt.target}),
                    ),
                    Ok(lazoo_providers::ConfigurationRecoveryState::DriftRefused) | Err(_) => (
                        lazoo_store::ConfigurationOperationState::DriftRefused,
                        json!({"recovery":"drift-refused","target":receipt.target}),
                    ),
                },
            };
            self.store.settle_configuration_recovery(
                operation.id,
                operation.revision,
                state,
                &observed,
            )?;
        }
        Ok(())
    }

    pub(super) async fn configuration_select_preview(
        &self,
        value: Value,
    ) -> Result<Value, RpcError> {
        let params: ConfigurationPreviewParams =
            serde_json::from_value(value).map_err(|error| invalid_params(error.to_string()))?;
        let validation = self
            .configuration_validation(params.selection.provider)
            .await?;
        let preview = preview_configuration_apply(
            &self.provider_environment,
            self.configuration_backup_root(),
            params.selection.clone(),
            |_| Some(validation),
        )
        .map_err(|error| configuration_error(&error))?;
        let metadata = preview.metadata().clone();
        let operation = self
            .store
            .create_configuration_preview(&CreateConfigurationPreviewInput {
                id: Uuid::new_v4(),
                provider: metadata.provider.to_string(),
                target: metadata.target.clone(),
                preview: to_value(&metadata),
                desired: to_value(&params.selection),
                observed: observed(&metadata),
                original_hash: metadata.original_hash.clone(),
                candidate_hash: metadata.candidate_hash.clone(),
                preview_revision: metadata.revision.clone(),
            })
            .map_err(store_error)?;
        self.publish_events()?;
        Ok(to_value(operation))
    }

    #[allow(clippy::too_many_lines)] // Persist/prepare/write/reprobe transitions must remain adjacent.
    pub(super) async fn configuration_apply(&self, value: Value) -> Result<Value, RpcError> {
        let params: ConfigurationOperationParams =
            serde_json::from_value(value).map_err(|error| invalid_params(error.to_string()))?;
        let operation = self
            .store
            .configuration_operation(params.operation_id)
            .map_err(store_error)?;
        if operation.state == lazoo_store::ConfigurationOperationState::Applied {
            // Lost RPC replies are idempotent even though preparation advanced the revision.
            return Ok(to_value(operation));
        }
        if operation.revision != params.expected_revision {
            return Err(store_error(lazoo_store::StoreError::ConfigurationStale {
                id: operation.id,
                current_revision: operation.revision,
            }));
        }
        if operation.state == lazoo_store::ConfigurationOperationState::Prepared {
            return self.configuration_resume_prepared(&operation, true);
        }
        let selection: SelectedMcpIntegration =
            serde_json::from_value(operation.desired.clone())
                .map_err(|_| invalid_params("stored configuration selection is invalid"))?;
        let validation = self.configuration_validation(selection.provider).await?;
        let preview = preview_configuration_apply(
            &self.provider_environment,
            self.configuration_backup_root(),
            selection.clone(),
            |_| Some(validation),
        )
        .map_err(|error| configuration_error(&error))?;
        if preview.metadata().revision != operation.preview_revision
            || preview.metadata().original_hash != operation.original_hash
            || preview.metadata().candidate_hash != operation.candidate_hash
            || preview.metadata().target != operation.target
        {
            return Err(configuration_error(
                &lazoo_providers::ConfigurationApplyError::StalePreview,
            ));
        }
        let intent = self
            .store
            .begin_configuration_apply(&BeginConfigurationApplyInput {
                id: operation.id,
                expected_revision: operation.revision,
            })
            .map_err(store_error)?;
        self.publish_events()?;
        let receipt = match apply_configuration_preview_with_intent(
            preview,
            |receipt| {
                self.store
                    .prepare_configuration_receipt(&RecordConfigurationReceiptInput {
                        id: operation.id,
                        expected_revision: intent.revision,
                        backup_name: receipt.backup_name.clone(),
                        backup_hash: receipt.backup_hash.clone(),
                        receipt: to_value(receipt),
                        observed: json!({"preparedAppliedHash": receipt.applied_hash, "target": receipt.target}),
                    })
                    .map_err(|_| lazoo_providers::ConfigurationApplyError::InvalidBackupReceipt)?;
                self.publish_events()
                    .map_err(|_| lazoo_providers::ConfigurationApplyError::InvalidBackupReceipt)
            },
            |path| {
                let report =
                    lazoo_providers::discover_provider_configuration(&self.provider_environment);
                if report.files.iter().any(|file| {
                    file.parsed && file.path.canonicalize().ok().as_deref() == Some(path)
                }) {
                    Ok(())
                } else {
                    Err(lazoo_providers::ConfigurationApplyError::MalformedConfiguration)
                }
            },
        ) {
            Ok(receipt) => receipt,
            Err(error) => {
                // Atomic-write or re-probe failure may have restored the original bytes. Inspect
                // immediately so a Prepared record cannot remain an ambiguous active lock.
                let current = self
                    .store
                    .configuration_operation(operation.id)
                    .map_err(store_error)?;
                if current.state == lazoo_store::ConfigurationOperationState::Prepared {
                    let _ = self.configuration_resume_prepared(&current, false)?;
                }
                return Err(configuration_error(&error));
            }
        };
        let prepared = self
            .store
            .configuration_operation(operation.id)
            .map_err(store_error)?;
        let recorded = self
            .store
            .record_configuration_receipt(&RecordConfigurationReceiptInput {
                id: operation.id,
                expected_revision: prepared.revision,
                backup_name: receipt.backup_name.clone(),
                backup_hash: receipt.backup_hash.clone(),
                receipt: to_value(&receipt),
                observed: json!({"appliedHash": receipt.applied_hash, "target": receipt.target}),
            })
            .map_err(store_error)?;
        self.publish_events()?;
        Ok(to_value(recorded))
    }

    fn configuration_resume_prepared(
        &self,
        operation: &lazoo_store::ConfigurationOperation,
        reprobe_applied: bool,
    ) -> Result<Value, RpcError> {
        let receipt: lazoo_providers::ConfigurationApplyReceipt = operation
            .receipt
            .clone()
            .and_then(|value| serde_json::from_value(value).ok())
            .ok_or_else(|| invalid_params("stored prepared configuration receipt is invalid"))?;
        let recovery = lazoo_providers::inspect_configuration_apply_recovery(
            &self.provider_environment,
            self.configuration_backup_root(),
            &receipt,
        )
        .unwrap_or(lazoo_providers::ConfigurationRecoveryState::DriftRefused);
        let settled = match recovery {
            lazoo_providers::ConfigurationRecoveryState::OriginalPresent => self
                .store
                .settle_configuration_recovery(
                    operation.id,
                    operation.revision,
                    lazoo_store::ConfigurationOperationState::RolledBack,
                    &json!({"recovery":"original-present","target":receipt.target}),
                )
                .map_err(store_error)?,
            lazoo_providers::ConfigurationRecoveryState::AppliedPresent if reprobe_applied => {
                if let Err(error) = self.configuration_reprobe(&receipt.target) {
                    self.store
                        .settle_configuration_recovery(
                            operation.id,
                            operation.revision,
                            lazoo_store::ConfigurationOperationState::RecoveryNeeded,
                            &json!({"recovery":"applied-present-reprobe-failed","target":receipt.target}),
                        )
                        .map_err(store_error)?;
                    self.publish_events()?;
                    return Err(configuration_error(&error));
                }
                self.store
                    .record_configuration_receipt(&RecordConfigurationReceiptInput {
                        id: operation.id,
                        expected_revision: operation.revision,
                        backup_name: receipt.backup_name.clone(),
                        backup_hash: receipt.backup_hash.clone(),
                        receipt: to_value(&receipt),
                        observed: json!({"appliedHash": receipt.applied_hash, "target": receipt.target}),
                    })
                    .map_err(store_error)?
            }
            lazoo_providers::ConfigurationRecoveryState::AppliedPresent => self
                .store
                .settle_configuration_recovery(
                    operation.id,
                    operation.revision,
                    lazoo_store::ConfigurationOperationState::RecoveryNeeded,
                    &json!({"recovery":"applied-present-after-error","target":receipt.target}),
                )
                .map_err(store_error)?,
            lazoo_providers::ConfigurationRecoveryState::DriftRefused => self
                .store
                .settle_configuration_recovery(
                    operation.id,
                    operation.revision,
                    lazoo_store::ConfigurationOperationState::DriftRefused,
                    &json!({"recovery":"drift-refused","target":receipt.target}),
                )
                .map_err(store_error)?,
        };
        self.publish_events()?;
        Ok(to_value(settled))
    }

    fn configuration_reprobe(
        &self,
        path: &std::path::Path,
    ) -> Result<(), lazoo_providers::ConfigurationApplyError> {
        let report = lazoo_providers::discover_provider_configuration(&self.provider_environment);
        if report
            .files
            .iter()
            .any(|file| file.parsed && file.path.canonicalize().ok().as_deref() == Some(path))
        {
            Ok(())
        } else {
            Err(lazoo_providers::ConfigurationApplyError::MalformedConfiguration)
        }
    }

    pub(super) fn configuration_rollback(&self, value: Value) -> Result<Value, RpcError> {
        let params: ConfigurationOperationParams =
            serde_json::from_value(value).map_err(|error| invalid_params(error.to_string()))?;
        let operation = self
            .store
            .configuration_operation(params.operation_id)
            .map_err(store_error)?;
        if operation.revision != params.expected_revision {
            return Err(store_error(lazoo_store::StoreError::ConfigurationStale {
                id: operation.id,
                current_revision: operation.revision,
            }));
        }
        let receipt = operation
            .receipt
            .clone()
            .ok_or_else(|| invalid_params("operation has no server-owned apply receipt"))?;
        let receipt: lazoo_providers::ConfigurationApplyReceipt =
            serde_json::from_value(receipt)
                .map_err(|_| invalid_params("stored configuration receipt is invalid"))?;
        let rolled = lazoo_providers::rollback_configuration_apply(
            &self.provider_environment,
            self.configuration_backup_root(),
            &receipt,
            |path| {
                let report =
                    lazoo_providers::discover_provider_configuration(&self.provider_environment);
                if report.files.iter().any(|file| {
                    file.parsed && file.path.canonicalize().ok().as_deref() == Some(path)
                }) {
                    Ok(())
                } else {
                    Err(lazoo_providers::ConfigurationApplyError::MalformedConfiguration)
                }
            },
        )
        .map_err(|error| configuration_error(&error))?;
        let state = if rolled.status == lazoo_providers::ConfigurationApplyStatus::RolledBack {
            lazoo_store::ConfigurationOperationState::RolledBack
        } else {
            lazoo_store::ConfigurationOperationState::RecoveryNeeded
        };
        let settled = self.store.settle_configuration_recovery(
            operation.id,
            operation.revision,
            state,
            &json!({"rollbackStatus": rolled.status, "target": rolled.target, "originalHash": rolled.original_hash}),
        ).map_err(store_error)?;
        self.publish_events()?;
        Ok(to_value(settled))
    }

    pub(super) fn configuration_show(&self, value: Value) -> Result<Value, RpcError> {
        let params: ConfigurationShowParams =
            serde_json::from_value(value).map_err(|error| invalid_params(error.to_string()))?;
        self.store
            .configuration_operation(params.operation_id)
            .map(to_value)
            .map_err(store_error)
    }

    fn configuration_backup_root(&self) -> std::path::PathBuf {
        self.root.runtime_dir().join("provider-config-backups")
    }

    async fn configuration_validation(
        &self,
        provider: ProviderKind,
    ) -> Result<lazoo_providers::ProviderVersionValidation, RpcError> {
        let probe = self
            .providers
            .probe(provider, &self.provider_environment)
            .await
            .map_err(provider_error)?;
        let version = probe.version.clone().ok_or_else(|| RpcError {
            code: "LZ-CONFIG-VERSION-UNSUPPORTED".into(),
            message: "provider did not supply a supported version".into(),
            data: Some(json!({"provider": provider, "evidence": probe.evidence})),
        })?;
        let compatibility = supported_compatibility(&probe, &version).ok_or_else(|| RpcError {
            code: "LZ-CONFIG-VERSION-UNSUPPORTED".into(),
            message: "detected provider version is outside the reviewed MCP configuration range"
                .into(),
            data: Some(
                json!({"provider": provider, "version": version, "evidence": probe.evidence}),
            ),
        })?;
        Ok(lazoo_providers::ProviderVersionValidation {
            provider,
            version,
            compatibility,
        })
    }
}

fn supported_compatibility(
    probe: &ProviderProbe,
    version: &str,
) -> Option<ConfigurationCompatibility> {
    // A successful Codex protocol handshake proves configuration support independently of
    // model authentication. A new profile may need login before inference, not before setup.
    let codex_needs_login = probe.provider == ProviderKind::Codex
        && probe.verification == ProviderVerification::Unverified
        && probe.protocol.as_deref() == Some("app-server/v2")
        && probe.authentication.native == lazoo_providers::AuthenticationState::NeedsLogin
        && probe.degradation_reason.is_none();
    if !(matches!(
        probe.verification,
        ProviderVerification::Detected
            | ProviderVerification::Configured
            | ProviderVerification::Verified
    ) || codex_needs_login)
        || !probe.capabilities.mcp
    {
        return None;
    }
    // The current Claude registry probe reports MCP unavailable. Do not infer write support
    // from a version match; enable it only after registry capability evidence is reviewed.
    if probe.provider == ProviderKind::ClaudeCode {
        return None;
    }
    if probe.provider == ProviderKind::Codex && reviewed_codex_mcp_version(version) {
        // Each value is pinned to independently reviewed configuration evidence.
        Some(ConfigurationCompatibility::Native)
    } else {
        None
    }
}

fn reviewed_codex_mcp_version(version: &str) -> bool {
    version
        .split_whitespace()
        .any(|token| matches!(token, "0.155.1" | "0.162.0-alpha.17.2"))
}

fn observed(metadata: &ConfigurationPreviewMetadata) -> Value {
    json!({"providerVersion": metadata.provider_version, "target": metadata.target, "originalHash": metadata.original_hash, "candidateHash": metadata.candidate_hash, "compatibility": metadata.compatibility})
}

fn configuration_error(error: &lazoo_providers::ConfigurationApplyError) -> RpcError {
    RpcError {
        code: "LZ-CONFIGURATION-REFUSED".into(),
        message: error.to_string(),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use lazoo_providers::{AuthenticationEvidence, ProviderCapabilities};
    use lazoo_runtime::DataRoot;
    use lazoo_store::{
        BeginConfigurationApplyInput, CreateConfigurationPreviewInput,
        RecordConfigurationReceiptInput, Store,
    };
    use tempfile::tempdir;

    use super::*;

    fn probe(provider: ProviderKind, version: &str, mcp: bool) -> ProviderProbe {
        ProviderProbe {
            contract_version: 1,
            provider,
            verification: ProviderVerification::Verified,
            executable: None,
            discovery_source: None,
            version: Some(version.into()),
            protocol: None,
            capabilities: ProviderCapabilities {
                mcp,
                ..ProviderCapabilities::default()
            },
            authentication: AuthenticationEvidence::unknown(),
            degradation_reason: None,
            evidence: vec!["fixture".into()],
            checked_at: "2026-10-15T00:00:00Z".into(),
        }
    }

    #[test]
    fn reviewed_codex_configuration_accepts_a_handshake_without_model_authentication() {
        let version = "codex-cli 0.162.0-alpha.17.2";
        let mut candidate = probe(ProviderKind::Codex, version, true);
        candidate.verification = ProviderVerification::Unverified;
        candidate.protocol = Some("app-server/v2".into());
        candidate.authentication.native = lazoo_providers::AuthenticationState::NeedsLogin;
        assert_eq!(
            supported_compatibility(&candidate, version),
            Some(ConfigurationCompatibility::Native)
        );
        candidate.verification = ProviderVerification::Degraded;
        assert_eq!(supported_compatibility(&candidate, version), None);
        candidate.verification = ProviderVerification::Unverified;
        candidate.protocol = None;
        assert_eq!(supported_compatibility(&candidate, version), None);
        candidate.protocol = Some("app-server/v2".into());
        candidate.degradation_reason = Some("handshake incomplete".into());
        assert_eq!(supported_compatibility(&candidate, version), None);
    }

    #[test]
    fn only_exact_reviewed_detected_versions_are_eligible() {
        assert_eq!(
            supported_compatibility(
                &probe(ProviderKind::Codex, "codex-cli 0.155.1", true),
                "codex-cli 0.155.1"
            ),
            Some(ConfigurationCompatibility::Native)
        );
        assert_eq!(
            supported_compatibility(
                &probe(ProviderKind::Codex, "codex-cli 0.162.0-alpha.17.2", true),
                "codex-cli 0.162.0-alpha.17.2"
            ),
            Some(ConfigurationCompatibility::Native)
        );
        assert_eq!(
            supported_compatibility(&probe(ProviderKind::Codex, "0.155.2", true), "0.155.2"),
            None
        );
        assert_eq!(
            supported_compatibility(
                &probe(ProviderKind::Codex, "codex-cli 0.162.0-alpha.17.3", true),
                "codex-cli 0.162.0-alpha.17.3"
            ),
            None
        );
        assert_eq!(
            supported_compatibility(&probe(ProviderKind::OpenCode, "1.9.9", true), "1.9.9"),
            None
        );
        assert_eq!(
            supported_compatibility(
                &probe(ProviderKind::ClaudeCode, "2.1.278", false),
                "2.1.278"
            ),
            None
        );
        assert_eq!(
            supported_compatibility(&probe(ProviderKind::ClaudeCode, "2.1.278", true), "2.1.278"),
            None
        );
    }

    #[tokio::test]
    async fn lost_apply_reply_returns_the_same_finalized_operation() {
        const ORIGINAL: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        const APPLIED: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let temporary = tempdir().unwrap();
        let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
        let id = Uuid::new_v4();
        let preview = store
            .create_configuration_preview(&CreateConfigurationPreviewInput {
                id,
                provider: "codex".into(),
                target: temporary.path().join(".codex").join("config.toml"),
                preview: json!({"originalHash":ORIGINAL,"candidateHash":APPLIED}),
                desired: json!({"provider":"codex","name":"fixture","command":"npx","args":[],"url":null,"enabled":true,"replace":false,"selectedSecretFields":[]}),
                observed: json!({"originalHash":ORIGINAL}),
                original_hash: ORIGINAL.into(),
                candidate_hash: APPLIED.into(),
                preview_revision: ORIGINAL.into(),
            })
            .unwrap();
        let intent = store
            .begin_configuration_apply(&BeginConfigurationApplyInput {
                id,
                expected_revision: preview.revision,
            })
            .unwrap();
        let prepared = store
            .prepare_configuration_receipt(&RecordConfigurationReceiptInput {
                id,
                expected_revision: intent.revision,
                backup_name: "codex-fixture.backup".into(),
                backup_hash: ORIGINAL.into(),
                receipt: json!({"backupName":"codex-fixture.backup","backupHash":ORIGINAL}),
                observed: json!({"preparedAppliedHash":APPLIED}),
            })
            .unwrap();
        let applied = store
            .record_configuration_receipt(&RecordConfigurationReceiptInput {
                id,
                expected_revision: prepared.revision,
                backup_name: "codex-fixture.backup".into(),
                backup_hash: ORIGINAL.into(),
                receipt: json!({"backupName":"codex-fixture.backup","backupHash":ORIGINAL}),
                observed: json!({"appliedHash":APPLIED}),
            })
            .unwrap();
        let app = App::new_with_provider_environment(
            store,
            DataRoot::from_path(temporary.path().join("data")),
            lazoo_providers::DiscoveryEnvironment::empty(),
        )
        .unwrap();
        let retry = app
            .configuration_apply(json!({"operationId":id,"expectedRevision":preview.revision}))
            .await
            .unwrap();
        assert_eq!(retry["id"], json!(id));
        assert_eq!(retry["revision"], json!(applied.revision));
        // The read-only CLI sends only the identity; showing an operation must not require
        // the caller to already know its current revision.
        let shown = app.configuration_show(json!({"operationId": id})).unwrap();
        assert_eq!(shown, retry);
        assert_eq!(
            retry["state"],
            json!(lazoo_store::ConfigurationOperationState::Applied)
        );
    }
}
