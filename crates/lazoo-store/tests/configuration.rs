use lazoo_store::{
    BeginConfigurationApplyInput, ConfigurationOperationState, CreateConfigurationPreviewInput,
    RecordConfigurationReceiptInput, Store, StoreError,
};
use serde_json::json;
use tempfile::tempdir;
use uuid::Uuid;

const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn input(id: Uuid) -> CreateConfigurationPreviewInput {
    CreateConfigurationPreviewInput {
        id,
        provider: "codex".into(),
        target: "C:\\fixture\\.codex\\config.toml".into(),
        preview: json!({"changedEntries":["search"],"originalHash":HASH,"candidateHash":OTHER_HASH}),
        desired: json!({"provider":"codex","name":"search","command":"npx","args":["-y"],"url":null,"enabled":true,"replace":false,"selectedSecretFields":[]}),
        observed: json!({"target":"C:\\fixture\\.codex\\config.toml","originalHash":HASH}),
        original_hash: HASH.into(),
        candidate_hash: OTHER_HASH.into(),
        preview_revision: HASH.into(),
    }
}

#[test]
fn preview_intent_receipt_and_rollback_state_are_revisioned_and_receipt_server_owned() {
    let directory = tempdir().unwrap();
    let store = Store::open_path(directory.path().join("lazoo.db")).unwrap();
    let id = Uuid::new_v4();
    let preview = store.create_configuration_preview(&input(id)).unwrap();
    assert_eq!(preview.state, ConfigurationOperationState::Previewed);
    assert_eq!(preview.revision, 1);
    assert!(preview.receipt.is_none());

    assert!(matches!(
        store.begin_configuration_apply(&BeginConfigurationApplyInput {
            id,
            expected_revision: 9
        }),
        Err(StoreError::ConfigurationStale { .. })
    ));
    let intent = store
        .begin_configuration_apply(&BeginConfigurationApplyInput {
            id,
            expected_revision: 1,
        })
        .unwrap();
    assert_eq!(intent.state, ConfigurationOperationState::ApplyIntent);
    let receipt_input = RecordConfigurationReceiptInput {
        id,
        expected_revision: intent.revision,
        backup_name: "codex-fixture.backup".into(),
        backup_hash: HASH.into(),
        receipt: json!({"backupName":"codex-fixture.backup","backupHash":HASH,"originalHash":HASH,"appliedHash":OTHER_HASH}),
        observed: json!({"appliedHash":OTHER_HASH}),
    };
    let prepared = store.prepare_configuration_receipt(&receipt_input).unwrap();
    assert_eq!(prepared.state, ConfigurationOperationState::Prepared);
    let applied = store
        .record_configuration_receipt(&RecordConfigurationReceiptInput {
            expected_revision: prepared.revision,
            ..receipt_input
        })
        .unwrap();
    assert_eq!(applied.state, ConfigurationOperationState::Applied);
    assert!(applied.receipt.is_some());
    let rolled = store
        .settle_configuration_recovery(
            id,
            applied.revision,
            ConfigurationOperationState::RolledBack,
            &json!({"rollback":"verified"}),
        )
        .unwrap();
    assert_eq!(rolled.state, ConfigurationOperationState::RolledBack);
}

#[test]
fn preview_cap_and_unsafe_receipt_metadata_are_refused() {
    let directory = tempdir().unwrap();
    let store = Store::open_path(directory.path().join("lazoo.db")).unwrap();
    let id = Uuid::new_v4();
    let preview = store.create_configuration_preview(&input(id)).unwrap();
    let intent = store
        .begin_configuration_apply(&BeginConfigurationApplyInput {
            id,
            expected_revision: preview.revision,
        })
        .unwrap();
    assert!(matches!(
        store.prepare_configuration_receipt(&RecordConfigurationReceiptInput {
            id,
            expected_revision: intent.revision,
            backup_name: "..\\outside".into(),
            backup_hash: HASH.into(),
            receipt: json!({}),
            observed: json!({})
        }),
        Err(StoreError::InvalidRecord { .. })
    ));
}

#[test]
fn secret_bearing_values_cannot_enter_the_durable_preview() {
    let directory = tempdir().unwrap();
    let store = Store::open_path(directory.path().join("lazoo.db")).unwrap();
    let mut preview = input(Uuid::new_v4());
    preview.observed = json!({"headers":{"Authorization":"Bearer secret"}});
    assert!(matches!(
        store.create_configuration_preview(&preview),
        Err(StoreError::InvalidRecord { .. })
    ));
    for key in ["accessToken", "Authorization", "Environment", "API_KEY"] {
        let mut preview = input(Uuid::new_v4());
        preview.observed = json!({key: "secret"});
        assert!(matches!(
            store.create_configuration_preview(&preview),
            Err(StoreError::InvalidRecord { .. })
        ));
    }
}

#[test]
fn intent_without_receipt_and_prepared_retry_have_explicit_stateful_outcomes() {
    let directory = tempdir().unwrap();
    let store = Store::open_path(directory.path().join("lazoo.db")).unwrap();
    let intent_id = Uuid::new_v4();
    let preview = store
        .create_configuration_preview(&input(intent_id))
        .unwrap();
    let intent = store
        .begin_configuration_apply(&BeginConfigurationApplyInput {
            id: intent_id,
            expected_revision: preview.revision,
        })
        .unwrap();
    assert_eq!(
        store.configuration_recovery_candidates().unwrap(),
        vec![intent.clone()]
    );
    let released = store
        .settle_configuration_recovery(
            intent_id,
            intent.revision,
            ConfigurationOperationState::RolledBack,
            &json!({"recovery":"intent-without-receipt-no-mutation"}),
        )
        .unwrap();
    assert_eq!(released.state, ConfigurationOperationState::RolledBack);

    let prepared_id = Uuid::new_v4();
    let preview = store
        .create_configuration_preview(&input(prepared_id))
        .unwrap();
    let intent = store
        .begin_configuration_apply(&BeginConfigurationApplyInput {
            id: prepared_id,
            expected_revision: preview.revision,
        })
        .unwrap();
    let receipt = RecordConfigurationReceiptInput {
        id: prepared_id,
        expected_revision: intent.revision,
        backup_name: "codex-retry.backup".into(),
        backup_hash: HASH.into(),
        receipt: json!({"backupName":"codex-retry.backup","backupHash":HASH,"originalHash":HASH,"appliedHash":OTHER_HASH}),
        observed: json!({"preparedAppliedHash":OTHER_HASH}),
    };
    let prepared = store.prepare_configuration_receipt(&receipt).unwrap();
    assert!(
        store
            .configuration_recovery_candidates()
            .unwrap()
            .iter()
            .any(|candidate| candidate.id == prepared_id
                && candidate.state == ConfigurationOperationState::Prepared)
    );
    let applied = store
        .record_configuration_receipt(&RecordConfigurationReceiptInput {
            expected_revision: prepared.revision,
            ..receipt
        })
        .unwrap();
    assert_eq!(applied.state, ConfigurationOperationState::Applied);
}
