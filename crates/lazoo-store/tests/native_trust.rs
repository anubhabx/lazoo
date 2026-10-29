use lazoo_protocol::{NativeTrustDecision, NativeTrustState};
use lazoo_store::{
    ClaimNativeTrustNotificationInput, RequestNativeTrustInput, ResolveNativeTrustInput, Store,
    StoreError,
};
use tempfile::tempdir;
use uuid::Uuid;

fn request(
    key: &str,
    provider: &str,
    path: &str,
    managed: bool,
    boot_id: Uuid,
) -> RequestNativeTrustInput {
    RequestNativeTrustInput {
        request_id: format!("request-{key}"),
        idempotency_key: key.to_owned(),
        provider: provider.to_owned(),
        canonical_workspace_path: path.to_owned(),
        managed_workspace: managed,
        session_id: Uuid::new_v4(),
        employee_id: Uuid::new_v4(),
        owner_boot_id: boot_id,
    }
}

#[test]
fn native_workspace_trust_defaults_to_pending_and_is_idempotent() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let boot_id = Uuid::new_v4();
    let input = request("one", "codex", r"C:\\repo", true, boot_id);
    let created = store.request_native_trust(&input).unwrap();
    assert_eq!(created.trust.state, NativeTrustState::Pending);
    assert_eq!(created.trust.revision, 1);
    assert!(created.event_sequence.is_some());
    let replay = store.request_native_trust(&input).unwrap();
    assert_eq!(replay.trust, created.trust);
    assert_eq!(replay.event_sequence, None);
}

#[test]
fn native_workspace_trust_decisions_are_revision_fenced_and_decline_never_sets_policy() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let boot_id = Uuid::new_v4();
    let created = store
        .request_native_trust(&request("decline", "claude", r"C:\\repo", true, boot_id))
        .unwrap()
        .trust;
    let declined = store
        .resolve_native_trust(&ResolveNativeTrustInput {
            request_id: "resolve-decline".to_owned(),
            idempotency_key: "resolve-decline".to_owned(),
            trust_id: created.id,
            expected_revision: created.revision,
            decision: NativeTrustDecision::Decline,
            auto_accept_managed_next_time: true,
        })
        .unwrap()
        .trust;
    assert_eq!(declined.state, NativeTrustState::Declined);
    assert!(
        !store
            .get_native_trust_policy("claude")
            .unwrap()
            .auto_accept_managed_next_time
    );
    assert!(matches!(
        store.resolve_native_trust(&ResolveNativeTrustInput {
            request_id: "stale".to_owned(),
            idempotency_key: "stale".to_owned(),
            trust_id: created.id,
            expected_revision: created.revision,
            decision: NativeTrustDecision::Approve,
            auto_accept_managed_next_time: true,
        }),
        Err(StoreError::NativeTrustStale { .. })
    ));
}

#[test]
fn automatic_next_time_applies_only_to_future_managed_exact_provider_workspace() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let boot_id = Uuid::new_v4();
    let first = store
        .request_native_trust(&request("first", "codex", r"C:\\managed", true, boot_id))
        .unwrap()
        .trust;
    let approved = store
        .resolve_native_trust(&ResolveNativeTrustInput {
            request_id: "approve".to_owned(),
            idempotency_key: "approve".to_owned(),
            trust_id: first.id,
            expected_revision: first.revision,
            decision: NativeTrustDecision::Approve,
            auto_accept_managed_next_time: true,
        })
        .unwrap()
        .trust;
    assert_eq!(approved.state, NativeTrustState::Approved);
    assert!(
        store
            .get_native_trust_policy("codex")
            .unwrap()
            .auto_accept_managed_next_time
    );
    let policy = store.get_native_trust_policy("codex").unwrap();
    let disabled = store
        .set_native_trust_policy("codex", policy.revision, false)
        .unwrap();
    assert!(!disabled.auto_accept_managed_next_time);
    assert!(matches!(
        store.set_native_trust_policy("codex", policy.revision, true),
        Err(StoreError::NativeTrustPolicyStale { .. })
    ));
    let policy = store
        .set_native_trust_policy("codex", disabled.revision, true)
        .unwrap();
    assert!(policy.auto_accept_managed_next_time);
    assert_eq!(
        store
            .request_native_trust(&request(
                "managed-next",
                "codex",
                r"C:\\different",
                true,
                boot_id
            ))
            .unwrap()
            .trust
            .state,
        NativeTrustState::Approved
    );
    assert_eq!(
        store
            .request_native_trust(&request(
                "unmanaged",
                "codex",
                r"C:\\outside",
                false,
                boot_id
            ))
            .unwrap()
            .trust
            .state,
        NativeTrustState::Pending
    );
    assert_eq!(
        store
            .request_native_trust(&request(
                "other-provider",
                "claude",
                r"C:\\managed",
                true,
                boot_id
            ))
            .unwrap()
            .trust
            .state,
        NativeTrustState::Pending
    );
    let reset = store
        .reset_native_trust_policy("codex", policy.revision)
        .unwrap();
    assert!(!reset.auto_accept_managed_next_time);
    assert!(matches!(
        store.reset_native_trust_policy("codex", policy.revision),
        Err(StoreError::NativeTrustPolicyStale { .. })
    ));
}

#[test]
fn exact_workspace_approval_reuses_a_session_grant_and_restart_preserves_policy_but_cancels_pending()
 {
    let temporary = tempdir().unwrap();
    let database = temporary.path().join("lazoo.db");
    let store = Store::open_path(&database).unwrap();
    let first_boot = Uuid::new_v4();
    let approved = store
        .request_native_trust(&request(
            "exact-first",
            "codex",
            r"C:\\repo",
            false,
            first_boot,
        ))
        .unwrap()
        .trust;
    store
        .resolve_native_trust(&ResolveNativeTrustInput {
            request_id: "exact-approve".to_owned(),
            idempotency_key: "exact-approve".to_owned(),
            trust_id: approved.id,
            expected_revision: approved.revision,
            decision: NativeTrustDecision::Approve,
            auto_accept_managed_next_time: true,
        })
        .unwrap();
    let reused = store
        .request_native_trust(&request(
            "exact-second",
            "codex",
            r"C:\\repo",
            false,
            first_boot,
        ))
        .unwrap()
        .trust;
    assert_eq!(reused.state, NativeTrustState::Approved);
    assert_ne!(reused.session_id, approved.session_id);
    let pending = store
        .request_native_trust(&request(
            "restart-pending",
            "codex",
            r"C:\\pending",
            false,
            first_boot,
        ))
        .unwrap()
        .trust;
    drop(store);
    let reopened = Store::open_path(&database).unwrap();
    let reconciled = reopened.reconcile_native_trust(Uuid::new_v4()).unwrap();
    assert_eq!(
        reconciled
            .trusts
            .iter()
            .map(|trust| trust.id)
            .collect::<Vec<_>>(),
        vec![pending.id]
    );
    assert_eq!(
        reopened.get_native_trust(pending.id).unwrap().state,
        NativeTrustState::Cancelled
    );
    assert!(
        reopened
            .get_native_trust_policy("codex")
            .unwrap()
            .auto_accept_managed_next_time
    );
}

#[test]
fn pending_notifications_can_be_claimed_and_settled_once() {
    let temporary = tempdir().unwrap();
    let store = Store::open_path(temporary.path().join("lazoo.db")).unwrap();
    let trust = store
        .request_native_trust(&request(
            "notify",
            "codex",
            r"C:\\repo",
            false,
            Uuid::new_v4(),
        ))
        .unwrap()
        .trust;
    let claim = store
        .claim_native_trust_notification(&ClaimNativeTrustNotificationInput {
            trust_id: trust.id,
            claimant_id: Uuid::new_v4(),
        })
        .unwrap();
    assert!(claim.is_some());
    assert!(
        store
            .claim_native_trust_notification(&ClaimNativeTrustNotificationInput {
                trust_id: trust.id,
                claimant_id: Uuid::new_v4()
            })
            .unwrap()
            .is_none()
    );
    store
        .settle_native_trust_notification(trust.id, claim.unwrap().notification_claim_id.unwrap())
        .unwrap();
    assert!(
        store
            .claim_native_trust_notification(&ClaimNativeTrustNotificationInput {
                trust_id: trust.id,
                claimant_id: Uuid::new_v4()
            })
            .unwrap()
            .is_none()
    );
}
