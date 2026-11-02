//! Account metadata and ownership are durable; plaintext credentials never enter the store.
use chrono::Utc;
use lazoo_protocol::{
    AccountCredentialKind, AccountRoutingMode, ProviderAccountBinding, ProviderAccountPolicy,
    ProviderAccountRecord, ProviderAccountSpec, ProviderObservation,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::json;
use uuid::Uuid;

use crate::{Store, StoreError, employee};

/// A non-secret snapshot that binds a smoke result to the account-routing state it exercised.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderAccountVerificationGuard {
    provider: String,
    policy_revision: u64,
    accounts: Vec<(Uuid, u64)>,
}

impl Store {
    pub fn provider_accounts(&self) -> Result<Vec<ProviderAccountRecord>, StoreError> {
        let connection = self.lock()?;
        records(&connection)?
            .into_iter()
            .map(|record| enrich(&connection, record))
            .collect()
    }

    pub fn provider_account(&self, id: Uuid) -> Result<ProviderAccountRecord, StoreError> {
        let connection = self.lock()?;
        enrich(&connection, account(&connection, id)?)
    }

    /// A new account uses revision zero. Existing secrets are retained unless replaced.
    pub fn put_provider_account(
        &self,
        spec: &ProviderAccountSpec,
        expected_revision: u64,
        protected_secret: Option<&[u8]>,
    ) -> Result<ProviderAccountRecord, StoreError> {
        validate_spec(spec)?;
        if protected_secret.is_some_and(|value| value.is_empty() || value.len() > 32_768) {
            return Err(StoreError::AccountValidation(
                "invalid protected credential size",
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = maybe_account(&transaction, spec.id)?;
        if previous.as_ref().map_or(0, |record| record.revision) != expected_revision {
            return Err(StoreError::AccountStale);
        }
        if let Some(previous) = &previous {
            if previous.deleted
                || previous.spec.provider != spec.provider
                || previous.spec.credential_kind != spec.credential_kind
                || previous.spec.profile_directory != spec.profile_directory
                || previous.spec.credential_environment != spec.credential_environment
                || previous.spec.base_url != spec.base_url
                || previous.spec.api_protocol != spec.api_protocol
                || previous.spec.backend_provider != spec.backend_provider
            {
                return Err(StoreError::AccountValidation(
                    "account identity is immutable; add a new account",
                ));
            }
        } else if records(&transaction)?
            .iter()
            .filter(|record| !record.deleted)
            .count()
            >= 128
        {
            return Err(StoreError::AccountValidation("account capacity reached"));
        }
        let has_secret =
            protected_secret.is_some() || previous.as_ref().is_some_and(|record| record.has_secret);
        if spec.credential_kind == AccountCredentialKind::ApiKey && !has_secret {
            return Err(StoreError::AccountValidation(
                "API-key accounts require a credential",
            ));
        }
        if spec.credential_kind == AccountCredentialKind::NativeProfile && has_secret {
            return Err(StoreError::AccountValidation(
                "native profiles cannot store an API key",
            ));
        }
        let now = Utc::now().to_rfc3339();
        let pool_changed = previous.as_ref().is_none_or(|previous| {
            protected_secret.is_some()
                || previous.spec.enabled != spec.enabled
                || previous.spec.models != spec.models
        });
        let mut record = previous.unwrap_or(ProviderAccountRecord {
            spec: spec.clone(),
            revision: 0,
            has_secret,
            deleted: false,
            selections: 0,
            requests: 0,
            failures: 0,
            cooldown_until: 0,
            last_status: None,
            last_latency_millis: None,
            updated_at: now.clone(),
            created_at: now.clone(),
            reserve_percent: None,
            exhausted_until: None,
            reserve_enforceable: false,
            reserve_reason: None,
            windows: Vec::new(),
        });
        record.spec.clone_from(spec);
        record.revision += 1;
        record.has_secret = has_secret;
        record.updated_at.clone_from(&now);
        if protected_secret.is_some() {
            record.cooldown_until = 0;
        }
        transaction.execute(
            "INSERT INTO provider_accounts (id, provider, record_json, protected_secret) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(id) DO UPDATE SET record_json = excluded.record_json, protected_secret = COALESCE(excluded.protected_secret, provider_accounts.protected_secret)",
            params![spec.id.to_string(), spec.provider, serde_json::to_string(&record)?, protected_secret],
        )?;
        employee::insert_event(
            &transaction,
            "provider.account.updated",
            &json!({"account": &record}),
            &now,
        )?;
        if pool_changed
            && policy(&transaction, &spec.provider)?.mode != AccountRoutingMode::Inherited
        {
            invalidate_provider_observation(&transaction, &spec.provider, &now)?;
        }
        transaction.commit()?;
        Ok(record)
    }

    pub fn set_provider_account_preferences(
        &self,
        input: &lazoo_protocol::AccountPreferencesParams,
    ) -> Result<ProviderAccountRecord, StoreError> {
        if input.reserve_percent.is_some_and(|reserve| reserve > 90) {
            return Err(StoreError::AccountValidation(
                "reservePercent must be null or 0 through 90",
            ));
        }
        let mut connection = self.lock()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut record = account(&tx, input.id)?;
        if record.revision != input.expected_revision {
            return Err(StoreError::AccountStale);
        }
        if record.deleted {
            return Err(StoreError::AccountUnavailable);
        }
        record.reserve_percent = input.reserve_percent;
        record.revision += 1;
        record.updated_at = Utc::now().to_rfc3339();
        save_record(&tx, &record)?;
        employee::insert_event(
            &tx,
            "provider.account.preferences",
            &json!({"id":input.id,"reservePercent":input.reserve_percent,"revision":record.revision}),
            &record.updated_at,
        )?;
        invalidate_provider_observation(&tx, &record.spec.provider, &record.updated_at)?;
        let result = enrich(&tx, record)?;
        tx.commit()?;
        Ok(result)
    }

    /// Preserve a tombstone for historical ownership while forgetting the protected credential.
    pub fn remove_provider_account(
        &self,
        id: Uuid,
        expected_revision: u64,
    ) -> Result<ProviderAccountRecord, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut record = account(&transaction, id)?;
        if record.revision != expected_revision {
            return Err(StoreError::AccountStale);
        }
        record.deleted = true;
        record.has_secret = false;
        record.spec.enabled = false;
        record.revision += 1;
        record.updated_at = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE provider_accounts SET record_json = ?2, protected_secret = NULL WHERE id = ?1",
            params![id.to_string(), serde_json::to_string(&record)?],
        )?;
        employee::insert_event(
            &transaction,
            "provider.account.removed",
            &json!({"id": id}),
            &record.updated_at,
        )?;
        if policy(&transaction, &record.spec.provider)?.mode != AccountRoutingMode::Inherited {
            invalidate_provider_observation(
                &transaction,
                &record.spec.provider,
                &record.updated_at,
            )?;
        }
        transaction.commit()?;
        Ok(record)
    }

    pub fn provider_account_secret(&self, id: Uuid) -> Result<Option<Vec<u8>>, StoreError> {
        let connection = self.lock()?;
        let record = account(&connection, id)?;
        if record.deleted || !record.spec.enabled {
            return Err(StoreError::AccountUnavailable);
        }
        Ok(connection.query_row(
            "SELECT protected_secret FROM provider_accounts WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )?)
    }

    pub fn provider_account_policy(
        &self,
        provider: &str,
    ) -> Result<ProviderAccountPolicy, StoreError> {
        policy(&*self.lock()?, provider)
    }

    pub fn provider_account_verification_guard(
        &self,
        provider: &str,
    ) -> Result<ProviderAccountVerificationGuard, StoreError> {
        let connection = self.lock()?;
        verification_guard(&connection, provider)
    }

    /// Records a model-backed proof only if account routing has not changed since `guard`.
    /// Returns `false` for a stale guard without modifying the stored observation.
    pub fn record_verified_provider_observation(
        &self,
        observation: &ProviderObservation,
        guard: &ProviderAccountVerificationGuard,
    ) -> Result<bool, StoreError> {
        if observation.provider != guard.provider || observation.verification != "verified" {
            return Err(StoreError::AccountValidation(
                "verified observation does not match its account guard",
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if verification_guard(&transaction, &guard.provider)? != *guard {
            transaction.commit()?;
            return Ok(false);
        }
        let now = Utc::now().to_rfc3339();
        employee::write_provider_observation(&transaction, observation)?;
        let revision = employee::advance_roster(&transaction, &now)?;
        employee::insert_event(
            &transaction,
            "roster.refreshed",
            &json!({"providers":[&guard.provider], "rosterRevision": revision}),
            &now,
        )?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn set_provider_account_policy(
        &self,
        desired: &ProviderAccountPolicy,
    ) -> Result<ProviderAccountPolicy, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = policy(&transaction, &desired.provider)?;
        if previous.revision != desired.revision {
            return Err(StoreError::AccountStale);
        }
        if desired.order.len() > 128 {
            return Err(StoreError::AccountValidation("account order is too large"));
        }
        let mut seen = std::collections::HashSet::new();
        for id in &desired.order {
            let record = account(&transaction, *id)?;
            if !seen.insert(*id) || record.deleted || record.spec.provider != desired.provider {
                return Err(StoreError::AccountValidation(
                    "order must contain distinct live accounts of this provider",
                ));
            }
        }
        if desired.mode == AccountRoutingMode::Pinned {
            let record = account(
                &transaction,
                desired.account_id.ok_or(StoreError::AccountValidation(
                    "pinned routing requires an account",
                ))?,
            )?;
            if record.spec.provider != desired.provider || record.deleted || !record.spec.enabled {
                return Err(StoreError::AccountUnavailable);
            }
        } else if desired.account_id.is_some() {
            return Err(StoreError::AccountValidation(
                "only pinned routing accepts an account",
            ));
        }
        let mut updated = desired.clone();
        updated.revision += 1;
        transaction.execute("INSERT INTO provider_account_policies (provider, policy_json) VALUES (?1, ?2) ON CONFLICT(provider) DO UPDATE SET policy_json = excluded.policy_json",
            params![updated.provider, serde_json::to_string(&updated)?])?;
        employee::insert_event(
            &transaction,
            "provider.account.policy",
            &json!({"policy": &updated}),
            &Utc::now().to_rfc3339(),
        )?;
        if previous.mode != updated.mode
            || previous.account_id != updated.account_id
            || previous.order != updated.order
        {
            let now = Utc::now().to_rfc3339();
            invalidate_provider_observation(&transaction, &updated.provider, &now)?;
        }
        transaction.commit()?;
        Ok(updated)
    }

    /// Select atomically, then persist the account before any provider process is launched.
    /// A native resume resolves its previous owner, never a new pool member.
    pub fn bind_provider_account(
        &self,
        session_id: Uuid,
        provider: &str,
        model: Option<&str>,
        resume_native_id: Option<&str>,
    ) -> Result<Option<ProviderAccountRecord>, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(binding) = binding(&transaction, session_id)? {
            if binding.provider != provider {
                return Err(StoreError::AccountUnavailable);
            }
            return bound_profile(&transaction, &binding, model);
        }
        let previous = if let Some(native_id) = resume_native_id {
            let previous_id: Option<String> = transaction.query_row(
                "SELECT session_id FROM provider_account_bindings WHERE provider = ?1 AND native_session_id = ?2 ORDER BY rowid DESC LIMIT 1",
                params![provider, native_id], |row| row.get(0),
            ).optional()?;
            previous_id
                .map(|id| binding(&transaction, parse_id(&id)?))
                .transpose()?
                .flatten()
        } else {
            None
        };
        let selected = if resume_native_id.is_some() {
            match &previous {
                Some(binding) => bound_profile(&transaction, binding, model)?,
                None if policy(&transaction, provider)?.mode == AccountRoutingMode::Inherited => {
                    None
                }
                None => return Err(StoreError::AccountUnavailable),
            }
        } else {
            let routing = policy(&transaction, provider)?;
            match routing.mode {
                AccountRoutingMode::Inherited => None,
                AccountRoutingMode::Pinned => {
                    let selected = bound_account(&transaction, routing.account_id, model)?;
                    if let Some(record) = &selected
                        && !quota_eligible(&transaction, record)?
                    {
                        return Err(StoreError::AccountUnavailable);
                    }
                    selected
                }
                AccountRoutingMode::Balanced
                | AccountRoutingMode::Priority
                | AccountRoutingMode::ApiPool => Some(select_account(
                    &transaction,
                    provider,
                    model,
                    routing.mode == AccountRoutingMode::ApiPool,
                    &[],
                    None,
                )?),
            }
        };
        let prior_upstream = previous
            .as_ref()
            .and_then(|binding| binding.upstream_account_id)
            .map(|id| id.to_string());
        let routing_mode = previous.as_ref().map_or_else(
            || policy(&transaction, provider).map(|policy| policy.mode),
            |binding| Ok(binding.routing_mode),
        )?;
        let selected_id = selected.as_ref().map(|record| record.spec.id.to_string());
        transaction.execute("INSERT INTO provider_account_bindings (session_id, provider, account_id, upstream_account_id, routing_mode) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id.to_string(), provider, selected_id, prior_upstream.or_else(|| selected_id.clone()), serde_json::to_string(&routing_mode)?])?;
        transaction.commit()?;
        Ok(selected)
    }

    pub fn provider_account_binding(
        &self,
        session_id: Uuid,
    ) -> Result<Option<ProviderAccountBinding>, StoreError> {
        binding(&*self.lock()?, session_id)
    }

    /// Smoke sessions have no durable employee/native history. Never remove a real owner.
    pub fn release_transient_account_binding(&self, session_id: Uuid) -> Result<(), StoreError> {
        self.lock()?.execute(
            "DELETE FROM provider_account_bindings WHERE session_id = ?1 AND native_session_id IS NULL AND NOT EXISTS (SELECT 1 FROM employee_sessions WHERE id = ?1)",
            [session_id.to_string()],
        )?;
        Ok(())
    }

    /// Compare-and-swap used only by the proxy before any upstream response is accepted.
    pub fn rebind_provider_api_account(
        &self,
        session_id: Uuid,
        expected: Uuid,
        next: Uuid,
    ) -> Result<(), StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = account(&transaction, expected)?;
        let new = account(&transaction, next)?;
        if !old.spec.same_api_route(&new.spec)
            || new.deleted
            || !new.spec.enabled
            || new.cooldown_until > Utc::now().timestamp()
            || !quota_eligible(&transaction, &new)?
        {
            return Err(StoreError::AccountUnavailable);
        }
        let changed = transaction.execute("UPDATE provider_account_bindings SET upstream_account_id = ?3 WHERE session_id = ?1 AND upstream_account_id = ?2 AND routing_mode = ?4",
            params![session_id.to_string(), expected.to_string(), next.to_string(), serde_json::to_string(&AccountRoutingMode::ApiPool)?])?;
        if changed != 1 {
            return Err(StoreError::AccountStale);
        }
        crate::account_usage::rebind_session(
            &transaction,
            session_id,
            &new.spec.provider,
            expected,
            next,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn bind_provider_native_identity(
        &self,
        session_id: Uuid,
        native_id: &str,
    ) -> Result<(), StoreError> {
        if native_id.is_empty() || native_id.len() > 1024 {
            return Err(StoreError::AccountValidation(
                "invalid native session identity",
            ));
        }
        self.lock()?.execute(
            "UPDATE provider_account_bindings SET native_session_id = ?2 WHERE session_id = ?1",
            params![session_id.to_string(), native_id],
        )?;
        Ok(())
    }

    /// Used only for a fresh HTTP request without established continuation ownership.
    pub fn select_pool_account(
        &self,
        route: &ProviderAccountSpec,
        model: Option<&str>,
        excluded: &[Uuid],
    ) -> Result<ProviderAccountRecord, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let selected = select_account(
            &transaction,
            &route.provider,
            model,
            true,
            excluded,
            Some(route),
        )?;
        transaction.commit()?;
        Ok(selected)
    }

    pub fn observe_provider_account(
        &self,
        id: Uuid,
        status: u16,
        latency_millis: u64,
        retry_after_seconds: u64,
    ) -> Result<(), StoreError> {
        self.observe_provider_account_windows(id, status, latency_millis, retry_after_seconds, &[])
    }

    pub fn observe_provider_account_windows(
        &self,
        id: Uuid,
        status: u16,
        latency_millis: u64,
        retry_after_seconds: u64,
        windows: &[lazoo_protocol::UsageWindow],
    ) -> Result<(), StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut record = account(&transaction, id)?;
        record.requests += 1;
        record.last_status = Some(status);
        record.last_latency_millis = Some(latency_millis);
        if status >= 400 {
            record.failures += 1;
            let seconds = match status {
                401 | 403 => 3600,
                429 => retry_after_seconds.clamp(1, 86_400),
                500..=599 => retry_after_seconds.clamp(1, 60),
                _ => 0,
            };
            record.cooldown_until =
                Utc::now().timestamp() + i64::try_from(seconds).unwrap_or(86_400);
        }
        if status == 429 {
            record.exhausted_until = chrono::DateTime::from_timestamp(record.cooldown_until, 0)
                .map(|time| time.to_rfc3339());
        }
        crate::account_usage::write_windows(
            &transaction,
            &record.spec.provider,
            Some(id),
            windows,
        )?;
        crate::account_usage::add_delta(
            &transaction,
            &record.spec.provider,
            Some(id),
            Utc::now().timestamp(),
            &crate::account_usage::UsageDelta {
                requests: 1,
                failures: u64::from(status >= 400),
                ..Default::default()
            },
        )?;
        save_record(&transaction, &record)?;
        transaction.commit()?;
        Ok(())
    }
}

/// Account/policy changes make an ambient provider observation unsafe to reuse. Retain facts
/// about a missing or degraded executable, but require a fresh selected-account smoke for every
/// readiness/authentication claim.
fn invalidate_provider_observation(
    transaction: &rusqlite::Transaction<'_>,
    provider: &str,
    now: &str,
) -> Result<(), StoreError> {
    let Some(mut observation) = employee::query_provider_observation(transaction, provider)? else {
        return Ok(());
    };
    let prior = observation.clone();
    if !matches!(
        observation.verification.as_str(),
        "not-detected" | "degraded"
    ) {
        "unverified".clone_into(&mut observation.verification);
    }
    observation.authentication = json!({"native":"unknown","external":"unknown"});
    if observation == prior {
        return Ok(());
    }
    now.clone_into(&mut observation.checked_at);
    transaction.execute(
        "UPDATE provider_observations SET verification = ?2, authentication_json = ?3, checked_at = ?4 WHERE provider = ?1",
        params![provider, observation.verification, serde_json::to_string(&observation.authentication)?, observation.checked_at],
    )?;
    let revision = employee::advance_roster(transaction, now)?;
    employee::insert_event(
        transaction,
        "roster.refreshed",
        &json!({"providers":[provider], "rosterRevision": revision}),
        now,
    )?;
    Ok(())
}

fn validate_spec(spec: &ProviderAccountSpec) -> Result<(), StoreError> {
    if spec.id.is_nil()
        || !matches!(
            spec.provider.as_str(),
            "codex" | "claude-code" | "opencode" | "antigravity" | "github-copilot" | "grok-build"
        )
    {
        return Err(StoreError::AccountValidation(
            "invalid provider or account identity",
        ));
    }
    if spec.label.trim().is_empty()
        || spec.label.len() > 120
        || spec.label.chars().any(char::is_control)
    {
        return Err(StoreError::AccountValidation(
            "label must contain 1 to 120 printable bytes",
        ));
    }
    if !std::path::Path::new(&spec.profile_directory).is_absolute()
        || spec.profile_directory.len() > 4096
    {
        return Err(StoreError::AccountValidation(
            "profile directory must be an absolute path",
        ));
    }
    if spec.models.len() > 64
        || spec.models.iter().any(|model| {
            model.is_empty() || model.len() > 200 || model.chars().any(char::is_control)
        })
    {
        return Err(StoreError::AccountValidation("invalid model restrictions"));
    }
    if spec.credential_kind == AccountCredentialKind::NativeProfile
        && (spec.credential_environment.is_some()
            || spec.base_url.is_some()
            || spec.api_protocol.is_some())
    {
        return Err(StoreError::AccountValidation(
            "native profiles do not accept API configuration",
        ));
    }
    Ok(())
}

fn records(connection: &Connection) -> Result<Vec<ProviderAccountRecord>, StoreError> {
    let mut statement =
        connection.prepare("SELECT record_json FROM provider_accounts ORDER BY id")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
}

fn verification_guard(
    connection: &Connection,
    provider: &str,
) -> Result<ProviderAccountVerificationGuard, StoreError> {
    let policy = policy(connection, provider)?;
    let mut accounts = records(connection)?
        .into_iter()
        .filter(|record| record.spec.provider == provider)
        .map(|record| (record.spec.id, record.revision))
        .collect::<Vec<_>>();
    accounts.sort_unstable_by_key(|(id, _)| *id);
    Ok(ProviderAccountVerificationGuard {
        provider: provider.to_owned(),
        policy_revision: policy.revision,
        accounts,
    })
}
fn maybe_account(
    connection: &Connection,
    id: Uuid,
) -> Result<Option<ProviderAccountRecord>, StoreError> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT record_json FROM provider_accounts WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(StoreError::from))
        .transpose()
}
pub(crate) fn account(
    connection: &Connection,
    id: Uuid,
) -> Result<ProviderAccountRecord, StoreError> {
    maybe_account(connection, id)?.ok_or(StoreError::AccountNotFound(id))
}
fn policy(connection: &Connection, provider: &str) -> Result<ProviderAccountPolicy, StoreError> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT policy_json FROM provider_account_policies WHERE provider = ?1",
            [provider],
            |row| row.get(0),
        )
        .optional()?;
    raw.map_or_else(
        || Ok(ProviderAccountPolicy::inherited(provider)),
        |raw| Ok(serde_json::from_str(&raw)?),
    )
}
pub(crate) fn binding(
    connection: &Connection,
    session_id: Uuid,
) -> Result<Option<ProviderAccountBinding>, StoreError> {
    let raw = connection.query_row("SELECT provider, account_id, upstream_account_id, native_session_id, routing_mode FROM provider_account_bindings WHERE session_id = ?1", [session_id.to_string()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, String>(4)?))
    }).optional()?;
    raw.map(
        |(provider, account_id, upstream_account_id, native_session_id, routing_mode)| {
            Ok(ProviderAccountBinding {
                session_id,
                provider,
                routing_mode: serde_json::from_str(&routing_mode)?,
                account_id: account_id.map(|id| parse_id(&id)).transpose()?,
                upstream_account_id: upstream_account_id.map(|id| parse_id(&id)).transpose()?,
                native_session_id,
            })
        },
    )
    .transpose()
}
fn parse_id(value: &str) -> Result<Uuid, StoreError> {
    Uuid::parse_str(value)
        .map_err(|_| StoreError::AccountValidation("invalid stored account identity"))
}
fn eligible(record: &ProviderAccountRecord, model: Option<&str>) -> bool {
    !record.deleted
        && record.spec.enabled
        && record.cooldown_until <= Utc::now().timestamp()
        && record.spec.accepts_model(model)
}
fn bound_account(
    connection: &Connection,
    id: Option<Uuid>,
    model: Option<&str>,
) -> Result<Option<ProviderAccountRecord>, StoreError> {
    id.map(|id| {
        let record = account(connection, id)?;
        if !eligible(&record, model) {
            return Err(StoreError::AccountUnavailable);
        }
        Ok(record)
    })
    .transpose()
}
fn bound_profile(
    connection: &Connection,
    binding: &ProviderAccountBinding,
    model: Option<&str>,
) -> Result<Option<ProviderAccountRecord>, StoreError> {
    if binding.routing_mode != AccountRoutingMode::ApiPool {
        return bound_account(connection, binding.account_id, model);
    }
    let Some(id) = binding.account_id else {
        return Err(StoreError::AccountUnavailable);
    };
    let record = account(connection, id)?;
    if record.deleted || !record.spec.enabled {
        return Err(StoreError::AccountUnavailable);
    }
    let _upstream = bound_account(connection, binding.upstream_account_id.or(Some(id)), model)?;
    Ok(Some(record))
}
fn select_account(
    connection: &Connection,
    provider: &str,
    model: Option<&str>,
    api_only: bool,
    excluded: &[Uuid],
    compatible: Option<&ProviderAccountSpec>,
) -> Result<ProviderAccountRecord, StoreError> {
    let mut candidates = records(connection)?
        .into_iter()
        .filter(|record| {
            record.spec.provider == provider
                && eligible(record, model)
                && !excluded.contains(&record.spec.id)
                && compatible.is_none_or(|route| route.same_api_route(&record.spec))
                && (!api_only
                    || (record.spec.credential_kind == AccountCredentialKind::ApiKey
                        && record.spec.api_protocol.is_some()))
        })
        .collect::<Vec<_>>();
    let routing = policy(connection, provider)?;
    let mut eligible_candidates = Vec::new();
    for record in candidates.drain(..) {
        if quota_eligible(connection, &record)? {
            eligible_candidates.push(record);
        }
    }
    let priority =
        routing.mode == AccountRoutingMode::Priority || (api_only && !routing.order.is_empty());
    eligible_candidates.sort_by_key(|record| {
        (
            if priority { 0 } else { record.selections },
            routing
                .order
                .iter()
                .position(|id| *id == record.spec.id)
                .unwrap_or(usize::MAX),
            chrono::DateTime::parse_from_rfc3339(&record.created_at).ok(),
            record.spec.id,
        )
    });
    let mut selected = eligible_candidates
        .into_iter()
        .next()
        .ok_or(StoreError::AccountUnavailable)?;
    selected.selections += 1;
    save_record(connection, &selected)?;
    Ok(selected)
}
pub(crate) fn save_record(
    connection: &Connection,
    record: &ProviderAccountRecord,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE provider_accounts SET record_json = ?2 WHERE id = ?1",
        params![record.spec.id.to_string(), serde_json::to_string(record)?],
    )?;
    Ok(())
}

fn quota_eligible(
    connection: &Connection,
    record: &ProviderAccountRecord,
) -> Result<bool, StoreError> {
    let now = Utc::now();
    if record
        .exhausted_until
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|until| until > now)
    {
        return Ok(false);
    }
    let threshold = f64::from(100 - record.reserve_percent.unwrap_or(0));
    Ok(
        !crate::account_usage::windows_for(
            connection,
            &record.spec.provider,
            Some(record.spec.id),
        )?
        .iter()
        .any(|window| {
            crate::account_usage::current_window(window, now)
                && window.used_percent.is_some_and(|used| used >= threshold)
        }),
    )
}

fn enrich(
    connection: &Connection,
    mut record: ProviderAccountRecord,
) -> Result<ProviderAccountRecord, StoreError> {
    record.windows =
        crate::account_usage::windows_for(connection, &record.spec.provider, Some(record.spec.id))?;
    record.reserve_enforceable = record.windows.iter().any(|window| {
        crate::account_usage::current_window(window, Utc::now()) && window.used_percent.is_some()
    });
    record.reserve_reason = (!record.reserve_enforceable).then(|| {
        "No current provider-reported percentage window with a future reset is available".into()
    });
    if record
        .exhausted_until
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|until| until <= Utc::now())
    {
        record.exhausted_until = None;
    }
    Ok(record)
}
