use lazoo_protocol::{
    NativeTrustListParams, NativeTrustPolicyParams, NativeTrustShowParams, NativeTrustState,
    ResetNativeTrustPolicyParams, ResolveNativeTrustParams, SetNativeTrustPolicyParams,
};
use lazoo_store::{
    CancelNativeTrustInput, ClaimNativeTrustNotificationInput, RequestNativeTrustInput,
    ResolveNativeTrustInput, StoreError,
};
use serde_json::{Value, json};
use tokio::time::{Duration, sleep};
use uuid::Uuid;

use super::{App, RpcError, to_value};

const TRUST_POLL_INTERVAL: Duration = Duration::from_millis(250);

struct PendingNativeTrustGuard {
    app: App,
    trust_id: Uuid,
    expected_revision: u64,
    armed: bool,
}
impl PendingNativeTrustGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for PendingNativeTrustGuard {
    fn drop(&mut self) {
        if self.armed
            && self
                .app
                .store
                .cancel_native_trust(&CancelNativeTrustInput {
                    trust_id: self.trust_id,
                    expected_revision: self.expected_revision,
                })
                .is_ok()
        {
            let _ = self.app.publish_events();
        }
    }
}

impl App {
    /// Records a workspace trust prompt before a provider is launched. The await loop never holds
    /// a SQLite connection or app mutex: decisions and session cancellation remain serviceable.
    pub(super) async fn await_native_workspace_trust(
        &self,
        provider: &str,
        canonical_workspace_path: String,
        managed_workspace: bool,
        session_id: Uuid,
        employee_id: Uuid,
    ) -> Result<(), RpcError> {
        let requested = self
            .store
            .request_native_trust(&RequestNativeTrustInput {
                request_id: format!("native-trust-request-{session_id}"),
                idempotency_key: format!("native-trust:{session_id}"),
                provider: provider.to_owned(),
                canonical_workspace_path,
                managed_workspace,
                session_id,
                employee_id,
                owner_boot_id: self.boot_id,
            })
            .map_err(native_trust_store_error)?;
        self.publish_events()?;
        if requested.trust.state == NativeTrustState::Pending {
            self.notify_native_trust_once(requested.trust.id);
        }
        let guarded = self
            .store
            .get_native_trust(requested.trust.id)
            .map_err(native_trust_store_error)?;
        let mut guard = PendingNativeTrustGuard {
            app: self.clone(),
            trust_id: guarded.id,
            expected_revision: guarded.revision,
            armed: guarded.state == NativeTrustState::Pending,
        };

        let mut shutdown = self.shutdown.subscribe();
        loop {
            let trust = self
                .store
                .get_native_trust(requested.trust.id)
                .map_err(native_trust_store_error)?;
            match trust.state {
                NativeTrustState::Approved => {
                    guard.disarm();
                    return Ok(());
                }
                NativeTrustState::Declined | NativeTrustState::Cancelled => {
                    guard.disarm();
                    return Err(RpcError {
                        code: "LZ-NATIVE-TRUST-DECLINED".to_owned(),
                        message: "native workspace access was not approved".to_owned(),
                        data: Some(json!({"trustId": trust.id, "state": trust.state})),
                    });
                }
                NativeTrustState::Pending => {}
            }
            tokio::select! {
                () = self.native_trust_notify.notified() => {}
                () = sleep(TRUST_POLL_INTERVAL) => {}
                changed = shutdown.changed() => {
                    let _ = changed;
                    return Err(RpcError {
                        code: "LZ-NATIVE-TRUST-CANCELLED".to_owned(),
                        message: "daemon shutdown cancelled the pending native trust wait".to_owned(),
                        data: Some(json!({"trustId": trust.id})),
                    });
                }
            }
            let refreshed = self
                .store
                .get_native_trust(requested.trust.id)
                .map_err(native_trust_store_error)?;
            if refreshed.state != NativeTrustState::Pending {
                continue;
            }
            let session = self
                .store
                .get_session(session_id)
                .map_err(native_trust_store_error)?;
            if !matches!(
                session.state,
                lazoo_protocol::SessionLifecycleState::Starting
            ) {
                if let Err(error) = self.store.cancel_native_trust(&CancelNativeTrustInput {
                    trust_id: trust.id,
                    expected_revision: trust.revision,
                }) {
                    if !matches!(
                        error,
                        StoreError::NativeTrustStale { .. } | StoreError::NativeTrustState { .. }
                    ) {
                        return Err(native_trust_store_error(error));
                    }
                } else {
                    self.publish_events()?;
                }
                return Err(RpcError {
                    code: "LZ-NATIVE-TRUST-CANCELLED".to_owned(),
                    message: "session stopped while native workspace trust was pending".to_owned(),
                    data: Some(json!({"trustId": trust.id, "sessionId": session_id})),
                });
            }
        }
    }

    fn notify_native_trust_once(&self, trust_id: Uuid) {
        let claim =
            match self
                .store
                .claim_native_trust_notification(&ClaimNativeTrustNotificationInput {
                    trust_id,
                    claimant_id: self.boot_id,
                }) {
                Ok(Some(claim)) => claim,
                Ok(None) => return,
                Err(error) => {
                    tracing::warn!(%error, %trust_id, "native trust notification claim failed");
                    return;
                }
            };
        let message = format!(
            "{} requests access to {}. Review with lazooctl native-trust list, then approve or decline.",
            claim.provider, claim.canonical_workspace_path
        );
        if let Err(error) = self
            .user_notifier
            .notify("Lazoo workspace trust required", &message)
        {
            tracing::warn!(%error, %trust_id, "native trust notification could not be shown");
        }
        if let Some(claim_id) = claim.notification_claim_id
            && let Err(error) = self
                .store
                .settle_native_trust_notification(trust_id, claim_id)
        {
            tracing::warn!(%error, %trust_id, "native trust notification settlement failed");
        }
    }

    pub(super) fn native_trust_list(&self, value: &Value) -> Result<Value, RpcError> {
        let params: NativeTrustListParams = native_trust_params(value)?;
        self.store
            .list_native_trust(&params)
            .map(to_value)
            .map_err(native_trust_store_error)
    }

    pub(super) fn native_trust_show(&self, value: &Value) -> Result<Value, RpcError> {
        let params: NativeTrustShowParams = native_trust_params(value)?;
        self.store
            .get_native_trust(params.id)
            .map(to_value)
            .map_err(native_trust_store_error)
    }

    pub(super) fn resolve_native_trust(
        &self,
        request: &lazoo_protocol::JsonRpcRequest,
    ) -> Result<Value, RpcError> {
        let params: ResolveNativeTrustParams = native_trust_params(&request.params)?;
        if params.expected_revision == 0 {
            return Err(native_trust_invalid(
                "expectedRevision must be greater than zero",
            ));
        }
        let result = self
            .store
            .resolve_native_trust(&ResolveNativeTrustInput {
                request_id: request.id.clone(),
                idempotency_key: params.idempotency_key,
                trust_id: params.id,
                expected_revision: params.expected_revision,
                decision: params.decision,
                auto_accept_managed_next_time: params.auto_accept_managed_next_time,
            })
            .map_err(native_trust_store_error)?;
        self.publish_events()?;
        self.native_trust_notify.notify_waiters();
        Ok(to_value(result))
    }

    pub(super) fn native_trust_policy_show(&self, value: &Value) -> Result<Value, RpcError> {
        let params: NativeTrustPolicyParams = native_trust_params(value)?;
        self.store
            .get_native_trust_policy(&params.provider)
            .map(to_value)
            .map_err(native_trust_store_error)
    }

    pub(super) fn native_trust_policy_set(&self, value: &Value) -> Result<Value, RpcError> {
        let params: SetNativeTrustPolicyParams = native_trust_params(value)?;
        let policy = self
            .store
            .set_native_trust_policy(
                &params.provider,
                params.expected_revision,
                params.auto_accept_managed_next_time,
            )
            .map_err(native_trust_store_error)?;
        Ok(to_value(policy))
    }

    pub(super) fn native_trust_policy_reset(&self, value: &Value) -> Result<Value, RpcError> {
        let params: ResetNativeTrustPolicyParams = native_trust_params(value)?;
        let policy = self
            .store
            .reset_native_trust_policy(&params.provider, params.expected_revision)
            .map_err(native_trust_store_error)?;
        Ok(to_value(policy))
    }
}

fn native_trust_params<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, RpcError> {
    serde_json::from_value(value.clone()).map_err(|error| native_trust_invalid(error.to_string()))
}

fn native_trust_invalid(message: impl Into<String>) -> RpcError {
    RpcError {
        code: "LZ-NATIVE-TRUST-INVALID".to_owned(),
        message: message.into(),
        data: None,
    }
}

#[allow(clippy::needless_pass_by_value)] // Used directly as the owned Result::map_err adapter.
fn native_trust_store_error(error: StoreError) -> RpcError {
    let (code, data, message) = match &error {
        StoreError::NativeTrustNotFound { trust_id } => (
            "LZ-NATIVE-TRUST-NOT-FOUND",
            Some(json!({"trustId": trust_id})),
            error.to_string(),
        ),
        StoreError::NativeTrustStale {
            trust_id,
            current_revision,
        } => (
            "LZ-NATIVE-TRUST-STALE",
            Some(json!({"trustId": trust_id, "currentRevision": current_revision})),
            error.to_string(),
        ),
        StoreError::NativeTrustPolicyStale {
            provider,
            current_revision,
        } => (
            "LZ-NATIVE-TRUST-POLICY-STALE",
            Some(json!({"provider": provider, "currentRevision": current_revision})),
            error.to_string(),
        ),
        StoreError::NativeTrustState { trust_id, .. } => (
            "LZ-NATIVE-TRUST-STATE",
            Some(json!({"trustId": trust_id})),
            error.to_string(),
        ),
        StoreError::NativeTrustIdempotencyConflict => (
            "LZ-NATIVE-TRUST-IDEMPOTENCY-CONFLICT",
            None,
            error.to_string(),
        ),
        StoreError::NativeTrustValidation { .. } | StoreError::MessageValidation(_) => {
            ("LZ-NATIVE-TRUST-INVALID", None, error.to_string())
        }
        _ => (
            "LZ-NATIVE-TRUST-STORE",
            None,
            "native trust store operation failed".to_owned(),
        ),
    };
    RpcError {
        code: code.to_owned(),
        message,
        data,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use lazoo_protocol::JsonRpcRequest;
    use lazoo_runtime::DataRoot;
    use lazoo_store::Store;
    use serde_json::json;
    use tokio::time::timeout;
    use uuid::Uuid;

    use super::*;
    use crate::app::UserNotifier;

    #[derive(Default)]
    struct Recorder(AtomicUsize);
    impl UserNotifier for Recorder {
        fn notify(&self, _title: &str, _message: &str) -> Result<(), String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn request(id: &str, method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: id.to_owned(),
            method: method.to_owned(),
            params,
        }
    }

    #[tokio::test]
    async fn default_trust_waits_notifies_once_and_auto_policy_never_approves_unmanaged_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let root = DataRoot::from_path(temporary.path().join("data"));
        root.ensure().unwrap();
        let store = Store::open_path(root.state_db()).unwrap();
        let notifier = Arc::new(Recorder::default());
        let app = App::new(store, root)
            .unwrap()
            .with_user_notifier(notifier.clone());
        let employee = Uuid::new_v4();
        let waiting_app = app.clone();
        let waiter = tokio::spawn(async move {
            waiting_app
                .await_native_workspace_trust(
                    "claude-code",
                    r"C:\\owned".to_owned(),
                    true,
                    Uuid::new_v4(),
                    employee,
                )
                .await
        });
        let pending = timeout(Duration::from_secs(1), async {
            loop {
                let pending = app.store.pending_native_trust(10).unwrap();
                if !pending.is_empty() {
                    return pending;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(notifier.0.load(Ordering::SeqCst), 1);
        let approved = app.handle(request("approve", "native.trust.resolve", json!({"id": pending[0].id, "expectedRevision": pending[0].revision, "decision": "approve", "autoAcceptManagedNextTime": true, "idempotencyKey": "approve"}))).await.result.unwrap();
        assert_eq!(approved["trust"]["state"], "approved");
        assert!(waiter.await.unwrap().is_ok());
        let stale = app.handle(request("stale", "native.trust.resolve", json!({"id": pending[0].id, "expectedRevision": pending[0].revision, "decision": "decline", "autoAcceptManagedNextTime": false, "idempotencyKey": "stale"}))).await.error.unwrap();
        assert_eq!(stale.code, "LZ-NATIVE-TRUST-STALE");
        app.await_native_workspace_trust(
            "claude-code",
            r"C:\\owned-next".to_owned(),
            true,
            Uuid::new_v4(),
            employee,
        )
        .await
        .unwrap();
        let abort_app = app.clone();
        let aborted = tokio::spawn(async move {
            abort_app
                .await_native_workspace_trust(
                    "claude-code",
                    r"C:\\outside".to_owned(),
                    false,
                    Uuid::new_v4(),
                    employee,
                )
                .await
        });
        let cancelled = timeout(Duration::from_secs(1), async {
            loop {
                if let Some(trust) = app
                    .store
                    .pending_native_trust(10)
                    .unwrap()
                    .into_iter()
                    .next()
                {
                    return trust;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        aborted.abort();
        let _ = aborted.await;
        let cancelled = timeout(Duration::from_secs(1), async {
            loop {
                let trust = app.store.get_native_trust(cancelled.id).unwrap();
                if trust.state == NativeTrustState::Cancelled {
                    return trust;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(cancelled.state, NativeTrustState::Cancelled);
    }
}
