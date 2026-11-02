use lazoo_protocol::{
    AccountCredentialKind, AccountRoutingMode, ProviderAccountPolicy, ProviderAccountSpec,
    ProviderObservation,
};
use lazoo_store::{RefreshProviderObservationsInput, Store};
use uuid::Uuid;

fn quota(id: Uuid, used: f64, seconds: i64) -> lazoo_protocol::UsageWindow {
    lazoo_protocol::UsageWindow {
        provider: "codex".into(),
        account_id: Some(id),
        kind: "primary".into(),
        used_percent: Some(used),
        remaining: None,
        limit: None,
        window_minutes: Some(300),
        resets_at: Some((chrono::Utc::now() + chrono::Duration::seconds(seconds)).to_rfc3339()),
        observed_at: chrono::Utc::now().to_rfc3339(),
        source: "codex-app-server".into(),
        plan_type: Some("pro".into()),
    }
}

#[test]
#[allow(clippy::too_many_lines)] // Exercise transitions on one durable binding, including resume.
fn priority_reserves_exhaustion_and_resume_keep_ownership() {
    use lazoo_protocol::AccountPreferencesParams;
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_path(directory.path().join("accounts.db")).unwrap();
    let first = profile(directory.path(), "first");
    let second = profile(directory.path(), "second");
    store.put_provider_account(&first, 0, None).unwrap();
    store.put_provider_account(&second, 0, None).unwrap();
    let mut policy = store
        .set_provider_account_policy(&ProviderAccountPolicy {
            provider: "codex".into(),
            mode: AccountRoutingMode::Priority,
            account_id: None,
            revision: 0,
            order: vec![second.id, first.id],
        })
        .unwrap();
    let session = Uuid::new_v4();
    assert_eq!(
        store
            .bind_provider_account(session, "codex", None, None)
            .unwrap()
            .unwrap()
            .spec
            .id,
        second.id
    );
    store
        .bind_provider_native_identity(session, "owned-thread")
        .unwrap();
    assert_eq!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, None)
            .unwrap()
            .unwrap()
            .spec
            .id,
        second.id
    );
    let preference = AccountPreferencesParams {
        id: second.id,
        expected_revision: 1,
        reserve_percent: Some(20),
    };
    let record = store.set_provider_account_preferences(&preference).unwrap();
    assert!(!record.reserve_enforceable);
    assert!(record.reserve_reason.is_some());
    assert!(store.set_provider_account_preferences(&preference).is_err());
    assert!(
        store
            .set_provider_account_preferences(&AccountPreferencesParams {
                id: second.id,
                expected_revision: 2,
                reserve_percent: Some(91)
            })
            .is_err()
    );
    // No observed window: report unknown; do not fabricate a reserve block.
    assert_eq!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, None)
            .unwrap()
            .unwrap()
            .spec
            .id,
        second.id
    );
    store
        .observe_provider_account_windows(second.id, 200, 1, 0, &[quota(second.id, 79.9, 600)])
        .unwrap();
    assert!(
        store
            .provider_account(second.id)
            .unwrap()
            .reserve_enforceable
    );
    assert_eq!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, None)
            .unwrap()
            .unwrap()
            .spec
            .id,
        second.id
    );
    store
        .observe_provider_account_windows(second.id, 200, 1, 0, &[quota(second.id, 80.0, 600)])
        .unwrap();
    assert_eq!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, None)
            .unwrap()
            .unwrap()
            .spec
            .id,
        first.id
    );
    // Reserve admission never silently switches an accepted or resumed native thread.
    assert_eq!(
        store
            .bind_provider_account(session, "codex", None, None)
            .unwrap()
            .unwrap()
            .spec
            .id,
        second.id
    );
    assert_eq!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, Some("owned-thread"))
            .unwrap()
            .unwrap()
            .spec
            .id,
        second.id
    );
    policy.mode = AccountRoutingMode::Pinned;
    policy.account_id = Some(second.id);
    policy = store.set_provider_account_policy(&policy).unwrap();
    assert!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, None)
            .is_err()
    );
    store
        .observe_provider_account_windows(second.id, 200, 1, 0, &[quota(second.id, 100.0, -1)])
        .unwrap();
    assert!(
        !store
            .provider_account(second.id)
            .unwrap()
            .reserve_enforceable
    );
    assert_eq!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, None)
            .unwrap()
            .unwrap()
            .spec
            .id,
        second.id
    );
    policy.mode = AccountRoutingMode::Priority;
    policy.account_id = None;
    store.set_provider_account_policy(&policy).unwrap();
    store
        .observe_provider_account(second.id, 429, 1, 60)
        .unwrap();
    assert!(
        store
            .provider_account(second.id)
            .unwrap()
            .exhausted_until
            .is_some()
    );
    assert_eq!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, None)
            .unwrap()
            .unwrap()
            .spec
            .id,
        first.id
    );
    store
        .observe_provider_account(first.id, 429, 1, 60)
        .unwrap();
    assert!(matches!(
        store.bind_provider_account(Uuid::new_v4(), "codex", None, None),
        Err(lazoo_store::StoreError::AccountUnavailable)
    ));
}

#[test]
fn unlisted_accounts_follow_creation_order_and_balanced_ties_use_order() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_path(directory.path().join("accounts.db")).unwrap();
    let first = profile(directory.path(), "first");
    let second = profile(directory.path(), "second");
    store.put_provider_account(&first, 0, None).unwrap();
    store.put_provider_account(&second, 0, None).unwrap();
    let mut policy = store
        .set_provider_account_policy(&ProviderAccountPolicy {
            provider: "codex".into(),
            mode: AccountRoutingMode::Priority,
            account_id: None,
            revision: 0,
            order: vec![],
        })
        .unwrap();
    assert_eq!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, None)
            .unwrap()
            .unwrap()
            .spec
            .id,
        first.id
    );
    policy.mode = AccountRoutingMode::Balanced;
    policy.order = vec![first.id, second.id];
    policy = store.set_provider_account_policy(&policy).unwrap();
    assert_eq!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, None)
            .unwrap()
            .unwrap()
            .spec
            .id,
        second.id
    );
    assert_eq!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, None)
            .unwrap()
            .unwrap()
            .spec
            .id,
        first.id
    );
    policy.order = vec![first.id, first.id];
    assert!(store.set_provider_account_policy(&policy).is_err());
    policy.order = vec![Uuid::new_v4()];
    assert!(store.set_provider_account_policy(&policy).is_err());
}

#[test]
fn api_pool_uses_explicit_priority_and_returns_the_serving_label() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_path(directory.path().join("accounts.db")).unwrap();
    let mut first = profile(directory.path(), "first");
    first.credential_kind = AccountCredentialKind::ApiKey;
    first.credential_environment = Some("OPENAI_API_KEY".into());
    first.api_protocol = Some(lazoo_protocol::AccountApiProtocol::Responses);
    let mut second = first.clone();
    second.id = Uuid::new_v4();
    second.label = "second".into();
    second.profile_directory = directory
        .path()
        .join("second")
        .to_string_lossy()
        .into_owned();
    store
        .put_provider_account(&first, 0, Some(b"protected-fixture"))
        .unwrap();
    store
        .put_provider_account(&second, 0, Some(b"protected-fixture"))
        .unwrap();
    store
        .set_provider_account_policy(&ProviderAccountPolicy {
            provider: "codex".into(),
            mode: AccountRoutingMode::ApiPool,
            account_id: None,
            revision: 0,
            order: vec![second.id, first.id],
        })
        .unwrap();
    for _ in 0..3 {
        assert_eq!(
            store
                .select_pool_account(&first, None, &[])
                .unwrap()
                .spec
                .id,
            second.id
        );
    }
    let session = Uuid::new_v4();
    store
        .bind_provider_account(session, "codex", None, None)
        .unwrap();
    let owner = store.provider_session_account(session).unwrap().unwrap();
    assert_eq!(owner.id, second.id);
    assert_eq!(owner.label, "second");
    store
        .observe_provider_account(second.id, 429, 1, 60)
        .unwrap();
    assert_eq!(
        store
            .select_pool_account(&first, None, &[])
            .unwrap()
            .spec
            .id,
        first.id
    );
}

#[test]
fn migration_creates_durable_account_policy_and_binding_tables() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("accounts.db");
    let _store = Store::open_path(&path).unwrap();
    let connection = rusqlite::Connection::open(path).unwrap();
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('provider_accounts', 'provider_account_policies', 'provider_account_bindings')",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(
        count, 3,
        "account choices and resume ownership must survive restart"
    );
}

fn profile(directory: &std::path::Path, label: &str) -> ProviderAccountSpec {
    ProviderAccountSpec {
        id: Uuid::new_v4(),
        provider: "codex".into(),
        label: label.into(),
        credential_kind: AccountCredentialKind::NativeProfile,
        profile_directory: directory.join(label).to_string_lossy().into_owned(),
        credential_environment: None,
        base_url: None,
        api_protocol: None,
        backend_provider: None,
        models: Vec::new(),
        enabled: true,
    }
}

#[test]
fn balance_and_native_resume_ownership_survive_restart_and_policy_changes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("accounts.db");
    let store = Store::open_path(&path).unwrap();
    let first = profile(directory.path(), "first");
    let second = profile(directory.path(), "second");
    store.put_provider_account(&first, 0, None).unwrap();
    store.put_provider_account(&second, 0, None).unwrap();
    store
        .set_provider_account_policy(&ProviderAccountPolicy {
            order: Vec::new(),
            provider: "codex".into(),
            mode: AccountRoutingMode::Balanced,
            account_id: None,
            revision: 0,
        })
        .unwrap();
    let session = Uuid::new_v4();
    let chosen = store
        .bind_provider_account(session, "codex", None, None)
        .unwrap()
        .unwrap();
    let other = store
        .bind_provider_account(Uuid::new_v4(), "codex", None, None)
        .unwrap()
        .unwrap();
    assert_ne!(chosen.spec.id, other.spec.id);
    store
        .bind_provider_native_identity(session, "native-1")
        .unwrap();
    store
        .set_provider_account_policy(&ProviderAccountPolicy {
            order: Vec::new(),
            provider: "codex".into(),
            mode: AccountRoutingMode::Pinned,
            account_id: Some(other.spec.id),
            revision: 1,
        })
        .unwrap();
    drop(store);
    let store = Store::open_path(&path).unwrap();
    let resumed = store
        .bind_provider_account(Uuid::new_v4(), "codex", None, Some("native-1"))
        .unwrap()
        .unwrap();
    assert_eq!(resumed.spec.id, chosen.spec.id);
    let mut disabled = chosen.spec;
    disabled.enabled = false;
    store
        .put_provider_account(&disabled, chosen.revision, None)
        .unwrap();
    assert!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, Some("native-1"))
            .is_err()
    );
    assert!(
        store
            .bind_provider_account(
                Uuid::new_v4(),
                "codex",
                None,
                Some("untracked-native-history")
            )
            .is_err()
    );
}

#[test]
fn unavailable_pool_cannot_fall_back_to_ambient_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_path(directory.path().join("accounts.db")).unwrap();
    assert!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, None)
            .unwrap()
            .is_none()
    );
    store
        .set_provider_account_policy(&ProviderAccountPolicy {
            order: Vec::new(),
            provider: "codex".into(),
            mode: AccountRoutingMode::Balanced,
            account_id: None,
            revision: 0,
        })
        .unwrap();
    assert!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", None, None)
            .is_err()
    );
    let mut account = profile(directory.path(), "restricted");
    account.models = vec!["allowed".into()];
    store.put_provider_account(&account, 0, None).unwrap();
    assert!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", Some("different"), None)
            .is_err()
    );
    store
        .observe_provider_account(account.id, 429, 10, 600)
        .unwrap();
    assert!(
        store
            .bind_provider_account(Uuid::new_v4(), "codex", Some("allowed"), None)
            .is_err()
    );
}

#[test]
fn edits_are_revision_fenced_and_deletion_forgets_secret_without_rebinding_history() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_path(directory.path().join("accounts.db")).unwrap();
    let mut spec = profile(directory.path(), "api");
    spec.credential_kind = AccountCredentialKind::ApiKey;
    spec.credential_environment = Some("OPENAI_API_KEY".into());
    let protected = lazoo_runtime::protect_for_current_user(b"private-token-fixture").unwrap();
    let record = store
        .put_provider_account(&spec, 0, Some(&protected))
        .unwrap();
    let listed = serde_json::to_string(&store.provider_accounts().unwrap()).unwrap();
    assert!(!listed.contains("private-token-fixture"));
    assert!(!listed.contains("protected_secret"));
    assert!(store.put_provider_account(&spec, 0, None).is_err());
    let encrypted = store.provider_account_secret(spec.id).unwrap().unwrap();
    assert_eq!(
        lazoo_runtime::unprotect_for_current_user(&encrypted).unwrap(),
        b"private-token-fixture"
    );
    let removed = store
        .remove_provider_account(spec.id, record.revision)
        .unwrap();
    assert!(removed.deleted);
    assert!(!removed.has_secret);
    assert!(store.provider_account_secret(spec.id).is_err());
    assert!(
        store
            .put_provider_account(&spec, removed.revision, Some(&protected))
            .is_err()
    );
}

#[test]
fn simultaneous_session_selection_is_balanced_and_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_path(directory.path().join("accounts.db")).unwrap();
    for name in ["a", "b"] {
        store
            .put_provider_account(&profile(directory.path(), name), 0, None)
            .unwrap();
    }
    store
        .set_provider_account_policy(&ProviderAccountPolicy {
            order: Vec::new(),
            provider: "codex".into(),
            mode: AccountRoutingMode::Balanced,
            account_id: None,
            revision: 0,
        })
        .unwrap();
    std::thread::scope(|scope| {
        for _ in 0..20 {
            let store = store.clone();
            scope.spawn(move || {
                let session = Uuid::new_v4();
                let chosen = store
                    .bind_provider_account(session, "codex", None, None)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    chosen.spec.id,
                    store
                        .bind_provider_account(session, "codex", None, None)
                        .unwrap()
                        .unwrap()
                        .spec
                        .id
                );
            });
        }
    });
    let counts = store
        .provider_accounts()
        .unwrap()
        .into_iter()
        .map(|record| record.selections)
        .collect::<Vec<_>>();
    assert_eq!(counts, vec![10, 10]);
}

#[test]
fn pool_failover_filters_protocol_and_backend_before_selecting_and_normalizes_models() {
    use lazoo_protocol::AccountApiProtocol;
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_path(directory.path().join("accounts.db")).unwrap();
    let mut route = profile(directory.path(), "source");
    route.provider = "opencode".into();
    route.credential_kind = AccountCredentialKind::ApiKey;
    route.credential_environment = Some("OPENAI_API_KEY".into());
    route.backend_provider = Some("openai".into());
    route.api_protocol = Some(AccountApiProtocol::Responses);
    route.models = vec!["test-model".into()];
    route.id = Uuid::from_u128(10);
    store
        .put_provider_account(&route, 0, Some(b"opaque-protected-fixture"))
        .unwrap();
    let mut incompatible = route.clone();
    incompatible.id = Uuid::from_u128(1);
    incompatible.backend_provider = Some("anthropic".into());
    incompatible.api_protocol = Some(AccountApiProtocol::Anthropic);
    incompatible.credential_environment = Some("ANTHROPIC_API_KEY".into());
    store
        .put_provider_account(&incompatible, 0, Some(b"opaque-protected-fixture"))
        .unwrap();
    let mut compatible = route.clone();
    compatible.id = Uuid::from_u128(20);
    store
        .put_provider_account(&compatible, 0, Some(b"opaque-protected-fixture"))
        .unwrap();
    assert!(route.accepts_model(Some("openai/test-model")));
    assert!(!route.accepts_model(Some("anthropic/test-model")));
    assert_eq!(
        store
            .select_pool_account(&route, Some("test-model"), &[route.id])
            .unwrap()
            .spec
            .id,
        compatible.id
    );
    assert_eq!(
        store.provider_account(incompatible.id).unwrap().selections,
        0
    );
    store
        .set_provider_account_policy(&ProviderAccountPolicy {
            order: Vec::new(),
            provider: "opencode".into(),
            mode: AccountRoutingMode::Pinned,
            account_id: Some(route.id),
            revision: 0,
        })
        .unwrap();
    let session = Uuid::new_v4();
    store
        .bind_provider_account(session, "opencode", Some("openai/test-model"), None)
        .unwrap();
    assert!(
        store
            .rebind_provider_api_account(session, route.id, compatible.id)
            .is_err(),
        "a direct pinned route cannot become a proxy route"
    );
}

fn verified_observation() -> ProviderObservation {
    ProviderObservation {
        provider: "codex".into(),
        verification: "verified".into(),
        version: Some("1.0".into()),
        protocol: Some("app-server/v2".into()),
        capabilities: serde_json::json!({"session": true}),
        authentication: serde_json::json!({"native":"ready","external":"ready"}),
        degradation_reason: None,
        checked_at: "2026-11-01T00:00:00Z".into(),
    }
}

fn observation(path: &std::path::Path) -> ProviderObservation {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.query_row(
        "SELECT verification, version, protocol, capabilities_json, authentication_json, degradation_reason, checked_at FROM provider_observations WHERE provider = 'codex'",
        [],
        |row| Ok(ProviderObservation {
            provider: "codex".into(), verification: row.get(0)?, version: row.get(1)?, protocol: row.get(2)?,
            capabilities: serde_json::from_str(&row.get::<_, String>(3)?).unwrap(), authentication: serde_json::from_str(&row.get::<_, String>(4)?).unwrap(),
            degradation_reason: row.get(5)?, checked_at: row.get(6)?,
        }),
    ).unwrap()
}

#[test]
fn account_routing_changes_revoke_global_readiness_but_label_edits_do_not() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("accounts.db");
    let store = Store::open_path(&path).unwrap();
    store
        .refresh_provider_observations(&RefreshProviderObservationsInput {
            request_id: "seed".into(),
            idempotency_key: "seed".into(),
            observations: vec![verified_observation()],
        })
        .unwrap();
    let before = store.roster_snapshot().unwrap().revision;
    store
        .set_provider_account_policy(&ProviderAccountPolicy {
            order: Vec::new(),
            provider: "codex".into(),
            mode: AccountRoutingMode::Balanced,
            account_id: None,
            revision: 0,
        })
        .unwrap();
    let invalidated = observation(&path);
    assert_eq!(invalidated.verification, "unverified");
    assert_eq!(
        invalidated.authentication,
        serde_json::json!({"native":"unknown","external":"unknown"})
    );
    assert_eq!(invalidated.version.as_deref(), Some("1.0"));
    assert_eq!(
        invalidated.capabilities,
        serde_json::json!({"session": true})
    );
    assert!(store.roster_snapshot().unwrap().revision > before);

    store
        .refresh_provider_observations(&RefreshProviderObservationsInput {
            request_id: "reseed".into(),
            idempotency_key: "reseed".into(),
            observations: vec![verified_observation()],
        })
        .unwrap();
    let mut account = profile(directory.path(), "first");
    account.credential_kind = AccountCredentialKind::ApiKey;
    account.credential_environment = Some("OPENAI_API_KEY".into());
    let record = store
        .put_provider_account(&account, 0, Some(b"first-secret"))
        .unwrap();
    store
        .refresh_provider_observations(&RefreshProviderObservationsInput {
            request_id: "reseed-label".into(),
            idempotency_key: "reseed-label".into(),
            observations: vec![verified_observation()],
        })
        .unwrap();
    let mut renamed = account.clone();
    renamed.label = "renamed".into();
    let renamed = store
        .put_provider_account(&renamed, record.revision, None)
        .unwrap();
    assert_eq!(observation(&path).verification, "verified");
    store
        .put_provider_account(&renamed.spec, renamed.revision, Some(b"rotated-secret"))
        .unwrap();
    assert_eq!(observation(&path).verification, "unverified");
}

#[test]
fn stale_account_verification_guard_cannot_restore_readiness_after_key_rotation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("accounts.db");
    let store = Store::open_path(&path).unwrap();
    let mut account = profile(directory.path(), "key");
    account.credential_kind = AccountCredentialKind::ApiKey;
    account.credential_environment = Some("OPENAI_API_KEY".into());
    let account = store
        .put_provider_account(&account, 0, Some(b"first-secret"))
        .unwrap();
    store
        .set_provider_account_policy(&ProviderAccountPolicy {
            order: Vec::new(),
            provider: "codex".into(),
            mode: AccountRoutingMode::Pinned,
            account_id: Some(account.spec.id),
            revision: 0,
        })
        .unwrap();
    store
        .refresh_provider_observations(&RefreshProviderObservationsInput {
            request_id: "seed-verified".into(),
            idempotency_key: "seed-verified".into(),
            observations: vec![verified_observation()],
        })
        .unwrap();
    let stale = store.provider_account_verification_guard("codex").unwrap();
    let rotated = store
        .put_provider_account(&account.spec, account.revision, Some(b"rotated-secret"))
        .unwrap();
    assert!(
        !store
            .record_verified_provider_observation(&verified_observation(), &stale)
            .unwrap()
    );
    assert!(observation(&path).verification != "verified");
    let fresh = store.provider_account_verification_guard("codex").unwrap();
    assert!(
        store
            .record_verified_provider_observation(&verified_observation(), &fresh)
            .unwrap()
    );
    assert_eq!(observation(&path).verification, "verified");
    assert!(rotated.revision > account.revision);
}
