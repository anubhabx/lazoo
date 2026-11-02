//! Public quota observations and counters. Helpers join the caller's transaction.
use chrono::{DateTime, Duration, Utc};
use lazoo_protocol::{
    AccountUsageBucket, AccountUsageParams, AccountUsageResult, SessionAccount, TurnState,
    UsageAccountFilter, UsageBucketSize, UsageObservability, UsageWindow,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use uuid::Uuid;

use crate::{Store, StoreError, accounts};

const RETENTION_SECONDS: i64 = 90 * 86_400;
const MAX_ROWS: usize = 10_000;

#[derive(Default)]
pub(crate) struct UsageDelta {
    pub sessions: u64,
    pub turns_completed: u64,
    pub turns_failed: u64,
    pub requests: u64,
    pub failures: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub tokens_reported: u64,
}

fn sql_count(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::AccountValidation("usage count exceeds storage range"))
}

fn account_key(id: Option<Uuid>) -> String {
    id.map_or_else(String::new, |id| id.to_string())
}

pub(crate) fn add_delta(
    c: &Connection,
    provider: &str,
    account_id: Option<Uuid>,
    at: i64,
    delta: &UsageDelta,
) -> Result<(), StoreError> {
    c.execute("INSERT INTO provider_usage_hourly (start,provider,account_key,sessions,turns_completed,turns_failed,requests,failures,input_tokens,output_tokens,tokens_reported)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
        ON CONFLICT(start,provider,account_key) DO UPDATE SET
        sessions=sessions+excluded.sessions,turns_completed=turns_completed+excluded.turns_completed,
        turns_failed=turns_failed+excluded.turns_failed,requests=requests+excluded.requests,failures=failures+excluded.failures,
        input_tokens=CASE WHEN excluded.input_tokens IS NULL THEN input_tokens ELSE COALESCE(input_tokens,0)+excluded.input_tokens END,
        output_tokens=CASE WHEN excluded.output_tokens IS NULL THEN output_tokens ELSE COALESCE(output_tokens,0)+excluded.output_tokens END,
        tokens_reported=tokens_reported+excluded.tokens_reported",
        params![at.div_euclid(3600)*3600,provider,account_key(account_id),sql_count(delta.sessions)?,sql_count(delta.turns_completed)?,sql_count(delta.turns_failed)?,sql_count(delta.requests)?,sql_count(delta.failures)?,delta.input_tokens.map(sql_count).transpose()?,delta.output_tokens.map(sql_count).transpose()?,sql_count(delta.tokens_reported)?])?;
    let cutoff = Utc::now().timestamp() - RETENTION_SECONDS;
    c.execute(
        "DELETE FROM provider_usage_hourly WHERE start < ?1",
        [cutoff],
    )?;
    c.execute(
        "DELETE FROM provider_usage_watermarks WHERE updated_at < ?1",
        [cutoff],
    )?;
    Ok(())
}

pub(crate) fn session_started(c: &Connection, id: Uuid, provider: &str) -> Result<(), StoreError> {
    let account = accounts::binding(c, id)?
        .and_then(|binding| binding.upstream_account_id.or(binding.account_id));
    add_delta(
        c,
        provider,
        account,
        Utc::now().timestamp(),
        &UsageDelta {
            sessions: 1,
            ..Default::default()
        },
    )
}

/// Move a successfully started session's count with a pre-acceptance API binding CAS.
pub(crate) fn rebind_session(
    c: &Connection,
    session_id: Uuid,
    provider: &str,
    previous: Uuid,
    next: Uuid,
) -> Result<(), StoreError> {
    let started: Option<String> = c
        .query_row(
            "SELECT started_at FROM employee_sessions WHERE id=?1",
            [session_id.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let Some(at) = started
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|date| date.timestamp())
    else {
        return Ok(());
    };
    let changed = c.execute("UPDATE provider_usage_hourly SET sessions=sessions-1 WHERE start=?1 AND provider=?2 AND account_key=?3 AND sessions>0",params![at.div_euclid(3600)*3600,provider,previous.to_string()])?;
    if changed != 0 {
        add_delta(
            c,
            provider,
            Some(next),
            at,
            &UsageDelta {
                sessions: 1,
                ..Default::default()
            },
        )?;
    }
    Ok(())
}

pub(crate) fn session_account(
    c: &Connection,
    session_id: Uuid,
) -> Result<Option<SessionAccount>, StoreError> {
    accounts::binding(c, session_id)?
        .and_then(|binding| binding.upstream_account_id.or(binding.account_id))
        .map(|id| {
            Ok(SessionAccount {
                id,
                label: accounts::account(c, id)?.spec.label,
            })
        })
        .transpose()
}

fn native_owner(
    c: &Connection,
    provider: &str,
    native_id: Option<&str>,
) -> Result<Option<Uuid>, StoreError> {
    let id: Option<String> = c.query_row("SELECT COALESCE(upstream_account_id,account_id) FROM provider_account_bindings WHERE provider=?1 AND native_session_id=?2 ORDER BY rowid DESC LIMIT 1",
        params![provider,native_id], |row| row.get(0)).optional()?.flatten();
    id.map(|id| {
        Uuid::parse_str(&id).map_err(|_| StoreError::AccountValidation("invalid usage owner"))
    })
    .transpose()
}

/// Called after the turn revision/idempotency checks and before its transaction commits.
pub(crate) fn observe_turn(
    c: &Connection,
    turn_id: Uuid,
    provider: &str,
    native_id: Option<&str>,
    payload: &Value,
    transition: Option<lazoo_protocol::TurnState>,
) -> Result<(), StoreError> {
    let account_id = native_owner(c, provider, native_id)?;
    let kind = payload.get("type").and_then(Value::as_str);
    if kind == Some("quota") {
        let windows: Vec<UsageWindow> = serde_json::from_value(
            payload
                .get("windows")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )?;
        write_windows(c, provider, account_id, &windows)?;
        if payload.get("exhausted").and_then(Value::as_bool) == Some(true) {
            mark_exhausted(c, provider, account_id)?;
        }
    }
    if provider == "codex"
        && kind == Some("error")
        && matches!(
            payload.get("code").and_then(Value::as_str),
            Some(
                "usageLimitExceeded"
                    | "\"usageLimitExceeded\""
                    | "rateLimitExceeded"
                    | "\"rateLimitExceeded\""
            )
        )
    {
        mark_exhausted(c, provider, account_id)?;
    }
    let mut delta = UsageDelta::default();
    if kind == Some("usage") {
        let input = payload
            .get("inputTokens")
            .and_then(Value::as_u64)
            .filter(|value| *value <= i64::MAX.cast_unsigned());
        let output = payload
            .get("outputTokens")
            .and_then(Value::as_u64)
            .filter(|value| *value <= i64::MAX.cast_unsigned());
        if input.is_some() || output.is_some() {
            // Codex totals are cumulative per native thread. Result events from other
            // adapters are snapshots per Lazoo turn. A high-water mark also deduplicates replay.
            let scope = if provider == "codex" {
                format!("thread:{}", native_id.unwrap_or_default())
            } else {
                format!("turn:{turn_id}")
            };
            let old: (Option<u64>, Option<u64>) = c.query_row("SELECT input_tokens,output_tokens FROM provider_usage_watermarks WHERE provider=?1 AND scope=?2", params![provider,scope], |row| Ok((row.get::<_,Option<i64>>(0)?.map(i64::cast_unsigned),row.get::<_,Option<i64>>(1)?.map(i64::cast_unsigned)))).optional()?.unwrap_or_default();
            delta.input_tokens = input.map(|value| {
                value.saturating_sub(
                    old.0
                        .or_else(|| payload.get("inputTokensBefore").and_then(Value::as_u64))
                        .unwrap_or(0),
                )
            });
            delta.output_tokens = output.map(|value| {
                value.saturating_sub(
                    old.1
                        .or_else(|| payload.get("outputTokensBefore").and_then(Value::as_u64))
                        .unwrap_or(0),
                )
            });
            delta.tokens_reported = 1;
            // A first reported zero is evidence too. Keep high watermarks across stale notifications.
            c.execute("INSERT INTO provider_usage_watermarks(provider,scope,input_tokens,output_tokens,updated_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(provider,scope) DO UPDATE SET input_tokens=COALESCE(MAX(COALESCE(input_tokens,0),excluded.input_tokens),input_tokens),output_tokens=COALESCE(MAX(COALESCE(output_tokens,0),excluded.output_tokens),output_tokens),updated_at=excluded.updated_at", params![provider,scope,input.map(sql_count).transpose()?,output.map(sql_count).transpose()?,Utc::now().timestamp()])?;
        }
    }
    delta.turns_completed = u64::from(transition == Some(TurnState::Completed));
    delta.turns_failed = u64::from(matches!(
        transition,
        Some(TurnState::TerminalFailed | TurnState::RetryableFailed)
    ));
    if delta.turns_completed + delta.turns_failed + delta.tokens_reported > 0 {
        add_delta(c, provider, account_id, Utc::now().timestamp(), &delta)?;
    }
    Ok(())
}

pub(crate) fn current_window(window: &UsageWindow, now: DateTime<Utc>) -> bool {
    window
        .resets_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|reset| reset > now)
}

pub(crate) fn windows_for(
    c: &Connection,
    provider: &str,
    account_id: Option<Uuid>,
) -> Result<Vec<UsageWindow>, StoreError> {
    let mut stmt = c.prepare("SELECT window_json FROM provider_usage_windows WHERE provider=?1 AND account_key=?2 ORDER BY kind")?;
    stmt.query_map(params![provider, account_key(account_id)], |row| {
        row.get::<_, String>(0)
    })?
    .map(|row| Ok(serde_json::from_str(&row?)?))
    .collect()
}

pub(crate) fn write_windows(
    c: &Connection,
    provider: &str,
    account_id: Option<Uuid>,
    windows: &[UsageWindow],
) -> Result<(), StoreError> {
    if windows.len() > 16 {
        return Err(StoreError::AccountValidation("too many usage windows"));
    }
    for observed in windows {
        if !matches!(
            observed.kind.as_str(),
            "primary"
                | "secondary"
                | "five-hour"
                | "weekly"
                | "weekly-opus"
                | "weekly-sonnet"
                | "subscription"
                | "requests"
                | "tokens"
                | "input-tokens"
                | "output-tokens"
                | "project-tokens"
        ) || !matches!(
            observed.source.as_str(),
            "codex-app-server"
                | "claude-stream"
                | "claude-statusline"
                | "openai-headers"
                | "anthropic-headers"
        ) || observed
            .used_percent
            .is_some_and(|value| !value.is_finite() || value < 0.0)
            || observed.window_minutes.is_some_and(|value| value > 43_200)
            || observed
                .resets_at
                .as_deref()
                .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_err())
            || DateTime::parse_from_rfc3339(&observed.observed_at).is_err()
            || observed.plan_type.as_deref().is_some_and(|value| {
                !matches!(
                    value,
                    "free"
                        | "plus"
                        | "pro"
                        | "team"
                        | "business"
                        | "enterprise"
                        | "edu"
                        | "unknown"
                        | "go"
                )
            })
        {
            return Err(StoreError::AccountValidation("invalid public usage window"));
        }
        let mut window = observed.clone();
        provider.clone_into(&mut window.provider);
        window.account_id = account_id;
        let previous: Option<String> = c.query_row("SELECT window_json FROM provider_usage_windows WHERE provider=?1 AND account_key=?2 AND kind=?3",params![provider,account_key(account_id),window.kind],|row| row.get(0)).optional()?;
        if let Some(previous) = previous {
            let previous: UsageWindow = serde_json::from_str(&previous)?;
            if DateTime::parse_from_rfc3339(&previous.observed_at).ok()
                > DateTime::parse_from_rfc3339(&window.observed_at).ok()
            {
                continue;
            }
            if window.plan_type.is_none() {
                window.plan_type = previous.plan_type;
            }
        }
        c.execute("INSERT INTO provider_usage_windows(provider,account_key,kind,window_json) VALUES(?1,?2,?3,?4) ON CONFLICT(provider,account_key,kind) DO UPDATE SET window_json=excluded.window_json",params![provider,account_key(account_id),window.kind,serde_json::to_string(&window)?])?;
    }
    Ok(())
}

fn mark_exhausted(c: &Connection, provider: &str, id: Option<Uuid>) -> Result<(), StoreError> {
    let Some(id) = id else {
        return Ok(());
    };
    let now = Utc::now();
    let windows = windows_for(c, provider, Some(id))?;
    let until = windows
        .iter()
        .filter(|window| window.used_percent.is_some_and(|used| used >= 100.0))
        .filter_map(|window| {
            window
                .resets_at
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        })
        .map(|date| date.with_timezone(&Utc))
        .filter(|date| *date > now)
        .max()
        .unwrap_or(now + Duration::minutes(15))
        .min(now + Duration::days(30));
    let mut record = accounts::account(c, id)?;
    let previous = record
        .exhausted_until
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|date| date.with_timezone(&Utc));
    record.exhausted_until = Some(
        previous
            .map_or(until, |previous| previous.max(until))
            .to_rfc3339(),
    );
    accounts::save_record(c, &record)
}

impl Store {
    pub fn observe_session_usage_windows(
        &self,
        session_id: Uuid,
        windows: &[UsageWindow],
    ) -> Result<(), StoreError> {
        let mut c = self.lock()?;
        let tx = c.transaction()?;
        let binding = accounts::binding(&tx, session_id)?.ok_or(StoreError::AccountUnavailable)?;
        write_windows(
            &tx,
            &binding.provider,
            binding.upstream_account_id.or(binding.account_id),
            windows,
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn provider_session_account(
        &self,
        session_id: Uuid,
    ) -> Result<Option<SessionAccount>, StoreError> {
        session_account(&*self.lock()?, session_id)
    }

    /// No network request or model invocation: only locally observed data.
    #[allow(clippy::too_many_lines)] // Validate bounds and execute the two bounded read queries together.
    pub fn provider_account_usage(
        &self,
        query: &AccountUsageParams,
    ) -> Result<AccountUsageResult, StoreError> {
        let now = Utc::now();
        let parse = |value: &str| {
            DateTime::parse_from_rfc3339(value)
                .map(|date| date.with_timezone(&Utc))
                .map_err(|_| StoreError::AccountValidation("usage timestamps must be RFC3339"))
        };
        let to = query.to.as_deref().map(parse).transpose()?.unwrap_or(now);
        let from = query
            .from
            .as_deref()
            .map(parse)
            .transpose()?
            .unwrap_or(to - Duration::days(7));
        if from >= to
            || to - from > Duration::days(90)
            || from < now - Duration::days(90)
            || to > now + Duration::minutes(1)
        {
            return Err(StoreError::AccountValidation(
                "usage range must be within the last 90 days",
            ));
        }
        let provider = query.provider.as_deref();
        if provider.is_some_and(|value| {
            !matches!(
                value,
                "codex"
                    | "claude-code"
                    | "opencode"
                    | "github-copilot"
                    | "antigravity"
                    | "grok-build"
            )
        }) {
            return Err(StoreError::AccountValidation("unknown usage provider"));
        }
        let account_filter = match query.account_id {
            UsageAccountFilter::All => None,
            UsageAccountFilter::Account(id) => Some(account_key(id)),
        };
        let width = match query.bucket {
            UsageBucketSize::Hour => 3600,
            UsageBucketSize::Day => 86_400,
        };
        let c = self.lock()?;
        let mut stmt = c.prepare("SELECT (start / ?1) * ?1 AS bucket,provider,account_key,SUM(sessions),SUM(turns_completed),SUM(turns_failed),SUM(requests),SUM(failures),SUM(input_tokens),SUM(output_tokens),SUM(tokens_reported) FROM provider_usage_hourly WHERE start >= ?2 AND start < ?3 AND (?4 IS NULL OR provider=?4) AND (?5 IS NULL OR account_key=?5) GROUP BY bucket,provider,account_key ORDER BY bucket,provider,account_key LIMIT 10001")?;
        let buckets = stmt
            .query_map(
                params![
                    width,
                    from.timestamp().div_euclid(3600) * 3600,
                    to.timestamp(),
                    provider,
                    account_filter
                ],
                |row| {
                    let start: i64 = row.get(0)?;
                    let account: String = row.get(2)?;
                    Ok(AccountUsageBucket {
                        start: DateTime::from_timestamp(start, 0)
                            .unwrap_or_default()
                            .to_rfc3339(),
                        provider: row.get(1)?,
                        account_id: Uuid::parse_str(&account).ok(),
                        sessions: row.get::<_, i64>(3)?.cast_unsigned(),
                        turns_completed: row.get::<_, i64>(4)?.cast_unsigned(),
                        turns_failed: row.get::<_, i64>(5)?.cast_unsigned(),
                        requests: row.get::<_, i64>(6)?.cast_unsigned(),
                        failures: row.get::<_, i64>(7)?.cast_unsigned(),
                        input_tokens: row.get::<_, Option<i64>>(8)?.map(i64::cast_unsigned),
                        output_tokens: row.get::<_, Option<i64>>(9)?.map(i64::cast_unsigned),
                        tokens_reported: row.get::<_, i64>(10)?.cast_unsigned(),
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if buckets.len() > MAX_ROWS {
            return Err(StoreError::AccountValidation(
                "usage result exceeds 10000 rows; narrow the range or account filter",
            ));
        }
        let mut stmt = c.prepare("SELECT window_json FROM provider_usage_windows WHERE (?1 IS NULL OR provider=?1) AND (?2 IS NULL OR account_key=?2) ORDER BY provider,account_key,kind LIMIT 10001")?;
        let windows = stmt
            .query_map(params![provider, account_filter], |row| {
                row.get::<_, String>(0)
            })?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect::<Result<Vec<_>, StoreError>>()?;
        if windows.len() > MAX_ROWS {
            return Err(StoreError::AccountValidation(
                "too many usage windows; narrow the filter",
            ));
        }
        let observable_providers = [
            ("codex", true, true, "App-server primary/secondary subscription windows; API headers only through api-pool. Tokens are reported thread totals."),
            ("claude-code", true, true, "Native statusline quota windows or legacy stream rate_limit_event when supplied; API headers through api-pool. Token directions only in legacy stream results; native context occupancy is excluded."),
            ("opencode", true, true, "API windows only through api-pool; native subscription windows not observable. Owned HTTP assistant message token counts or legacy ACP reports when supplied."),
            ("github-copilot", false, true, "Subscription windows and quota-specific errors are not observable through the integrated ACP interface. Directional token reports only when supplied; context usage is not quota."),
            ("antigravity", false, true, "Subscription windows not observable. Token fields only in legacy machine results when supplied; governed native hooks do not report tokens."),
            ("grok-build", false, false, "Usage windows and token directions not observable through this adapter."),
        ].into_iter().map(|(name,windows,tokens,reason)| (name.into(),UsageObservability {windows,tokens,reason:Some(reason.into())})).collect();
        Ok(AccountUsageResult {
            buckets,
            windows,
            observable_providers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazoo_protocol::{
        AccountCredentialKind, AccountRoutingMode, ProviderAccountPolicy, ProviderAccountSpec,
        TurnState,
    };
    use serde_json::json;

    fn setup() -> (tempfile::TempDir, Store, Uuid) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_path(dir.path().join("usage.db")).unwrap();
        let id = Uuid::new_v4();
        store
            .put_provider_account(
                &ProviderAccountSpec {
                    id,
                    provider: "codex".into(),
                    label: "Work".into(),
                    credential_kind: AccountCredentialKind::NativeProfile,
                    profile_directory: dir.path().join("profile").to_string_lossy().into_owned(),
                    credential_environment: None,
                    base_url: None,
                    api_protocol: None,
                    backend_provider: None,
                    models: vec![],
                    enabled: true,
                },
                0,
                None,
            )
            .unwrap();
        store
            .set_provider_account_policy(&ProviderAccountPolicy {
                provider: "codex".into(),
                mode: AccountRoutingMode::Priority,
                account_id: None,
                revision: 0,
                order: vec![id],
            })
            .unwrap();
        let session = Uuid::new_v4();
        store
            .bind_provider_account(session, "codex", None, None)
            .unwrap();
        store
            .bind_provider_native_identity(session, "native")
            .unwrap();
        (dir, store, id)
    }

    #[test]
    fn ledger_differences_cumulative_tokens_filters_null_and_rolls_back() {
        let (_dir, store, id) = setup();
        let turn = Uuid::new_v4();
        {
            let mut c = store.lock().unwrap();
            let tx = c.transaction().unwrap();
            for (input, output) in [(100, 20), (100, 20), (140, 25), (90, 10)] {
                observe_turn(
                    &tx,
                    turn,
                    "codex",
                    Some("native"),
                    &json!({"type":"usage","inputTokens":input,"outputTokens":output}),
                    None,
                )
                .unwrap();
            }
            observe_turn(
                &tx,
                turn,
                "codex",
                Some("native"),
                &json!({}),
                Some(TurnState::Completed),
            )
            .unwrap();
            observe_turn(
                &tx,
                Uuid::new_v4(),
                "claude-code",
                None,
                &json!({}),
                Some(TurnState::TerminalFailed),
            )
            .unwrap();
            tx.commit().unwrap();
            let tx = c.transaction().unwrap();
            add_delta(
                &tx,
                "codex",
                Some(id),
                Utc::now().timestamp(),
                &UsageDelta {
                    requests: 99,
                    ..Default::default()
                },
            )
            .unwrap();
            // An aborted event must not leave aggregate counts behind.
            tx.rollback().unwrap();
        }
        let all = store
            .provider_account_usage(&AccountUsageParams::default())
            .unwrap();
        let bucket = all
            .buckets
            .iter()
            .find(|b| b.account_id == Some(id))
            .unwrap();
        assert_eq!(
            (bucket.input_tokens, bucket.output_tokens),
            (Some(140), Some(25))
        );
        assert_eq!(bucket.turns_completed, 1);
        assert_eq!(bucket.requests, 0);
        let personal: AccountUsageParams =
            serde_json::from_value(json!({"accountId":null})).unwrap();
        let personal = store.provider_account_usage(&personal).unwrap();
        assert_eq!(personal.buckets.len(), 1);
        assert_eq!(personal.buckets[0].input_tokens, None);
        assert_eq!(personal.buckets[0].turns_failed, 1);
        let named: AccountUsageParams =
            serde_json::from_value(json!({"accountId":id,"bucket":"hour"})).unwrap();
        assert_eq!(
            store.provider_account_usage(&named).unwrap().buckets.len(),
            1
        );
    }

    #[test]
    fn resumed_codex_excludes_tokens_before_the_first_observed_turn() {
        let (_dir, store, id) = setup();
        {
            let mut c = store.lock().unwrap();
            let tx = c.transaction().unwrap();
            for (input, output, before_input, before_output) in
                [(1040, 210, 1000, 200), (1100, 225, 1040, 210)]
            {
                observe_turn(&tx, Uuid::new_v4(), "codex", Some("native"),
                    &json!({"type":"usage","inputTokens":input,"outputTokens":output,"inputTokensBefore":before_input,"outputTokensBefore":before_output}), None).unwrap();
            }
            tx.commit().unwrap();
        }
        let result = store
            .provider_account_usage(&AccountUsageParams::default())
            .unwrap();
        assert_eq!(result.buckets[0].account_id, Some(id));
        assert_eq!(result.buckets[0].input_tokens, Some(100));
        assert_eq!(result.buckets[0].output_tokens, Some(25));
        assert_eq!(result.buckets[0].tokens_reported, 2);
    }

    #[test]
    fn hourly_daily_retention_and_query_bounds_are_explicit() {
        let (_dir, store, id) = setup();
        let now = Utc::now().timestamp();
        let day = (now / 86_400 - 1) * 86_400;
        {
            let mut c = store.lock().unwrap();
            let tx = c.transaction().unwrap();
            for at in [day, day + 3600, now - RETENTION_SECONDS - 3600] {
                add_delta(
                    &tx,
                    "codex",
                    Some(id),
                    at,
                    &UsageDelta {
                        sessions: 1,
                        input_tokens: Some(0),
                        tokens_reported: 1,
                        ..Default::default()
                    },
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        let result = store
            .provider_account_usage(&AccountUsageParams::default())
            .unwrap();
        assert_eq!(result.buckets.len(), 1);
        assert_eq!(result.buckets[0].sessions, 2);
        assert_eq!(result.buckets[0].input_tokens, Some(0));
        assert_eq!(result.buckets[0].output_tokens, None);
        let hourly = AccountUsageParams {
            bucket: UsageBucketSize::Hour,
            ..Default::default()
        };
        assert_eq!(
            store.provider_account_usage(&hourly).unwrap().buckets.len(),
            2
        );
        for value in [
            json!({"from":"secret-not-a-date"}),
            json!({"from":(Utc::now()-Duration::days(91)).to_rfc3339()}),
            json!({"provider":"unknown"}),
            json!({"to":(Utc::now()+Duration::days(1)).to_rfc3339()}),
        ] {
            let query = serde_json::from_value(value).unwrap();
            let error = store
                .provider_account_usage(&query)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("secret-not-a-date"));
        }
    }

    #[test]
    fn structured_exhaustion_uses_reset_and_expires_without_revision_changes() {
        let (_dir, store, id) = setup();
        let reset = (Utc::now() + Duration::hours(5)).to_rfc3339();
        let window = UsageWindow {
            provider: "codex".into(),
            account_id: Some(id),
            kind: "primary".into(),
            used_percent: Some(100.0),
            remaining: None,
            limit: None,
            window_minutes: Some(300),
            resets_at: Some(reset.clone()),
            observed_at: Utc::now().to_rfc3339(),
            source: "codex-app-server".into(),
            plan_type: None,
        };
        {
            let mut c = store.lock().unwrap();
            let tx = c.transaction().unwrap();
            write_windows(&tx, "codex", Some(id), std::slice::from_ref(&window)).unwrap();
            observe_turn(&tx,Uuid::new_v4(),"codex",Some("native"),&json!({"type":"error","code":"\"usageLimitExceeded\"","message":"secret-error-text"}),None).unwrap();
            tx.commit().unwrap();
        }
        let record = store.provider_account(id).unwrap();
        assert_eq!(record.exhausted_until, Some(reset));
        assert_eq!(record.revision, 1);
        let usage = store
            .provider_account_usage(&AccountUsageParams::default())
            .unwrap();
        assert!(
            !serde_json::to_string(&usage)
                .unwrap()
                .contains("secret-error-text")
        );
        assert!(
            store
                .bind_provider_account(Uuid::new_v4(), "codex", None, None)
                .is_err()
        );
        {
            let c = store.lock().unwrap();
            let mut record = accounts::account(&c, id).unwrap();
            record.exhausted_until = Some((Utc::now() - Duration::seconds(1)).to_rfc3339());
            accounts::save_record(&c, &record).unwrap();
            let mut expired = window;
            expired.resets_at = record.exhausted_until;
            expired.observed_at = Utc::now().to_rfc3339();
            write_windows(&c, "codex", Some(id), &[expired]).unwrap();
        }
        assert!(
            store
                .provider_account(id)
                .unwrap()
                .exhausted_until
                .is_none()
        );
        assert!(
            store
                .bind_provider_account(Uuid::new_v4(), "codex", None, None)
                .is_ok()
        );
    }
}
