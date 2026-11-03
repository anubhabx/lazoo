use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROVIDER_ACCOUNTS_V1: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountCredentialKind {
    NativeProfile,
    ApiKey,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountRoutingMode {
    Inherited,
    Balanced,
    Priority,
    Pinned,
    ApiPool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountApiProtocol {
    Responses,
    Anthropic,
}

/// Public account configuration. Authentication material never belongs in this record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderAccountSpec {
    pub id: Uuid,
    pub provider: String,
    pub label: String,
    pub credential_kind: AccountCredentialKind,
    pub profile_directory: String,
    pub credential_environment: Option<String>,
    pub base_url: Option<String>,
    pub api_protocol: Option<AccountApiProtocol>,
    /// `OpenCode`'s backend identifier, for example `openai` or `anthropic`.
    pub backend_provider: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    pub enabled: bool,
}

impl ProviderAccountSpec {
    /// Accept both `OpenCode`'s `backend/model` selection and the upstream's bare model name.
    #[must_use]
    pub fn accepts_model(&self, model: Option<&str>) -> bool {
        let mut normalized = model;
        if self.provider == "opencode"
            && let (Some(backend), Some((prefix, name))) = (
                self.backend_provider.as_deref(),
                model.and_then(|value| value.split_once('/')),
            )
        {
            if prefix != backend {
                return false;
            }
            normalized = Some(name);
        }
        self.models.is_empty()
            || normalized.is_some_and(|model| {
                self.models.iter().any(|allowed| {
                    allowed == model
                        || (self.provider == "opencode"
                            && self
                                .backend_provider
                                .as_deref()
                                .is_some_and(|backend| allowed == &format!("{backend}/{model}")))
                })
            })
    }

    #[must_use]
    pub fn same_api_route(&self, other: &Self) -> bool {
        self.credential_kind == AccountCredentialKind::ApiKey
            && other.credential_kind == AccountCredentialKind::ApiKey
            && self.provider == other.provider
            && self.api_protocol.is_some()
            && self.api_protocol == other.api_protocol
            && self.backend_provider == other.backend_provider
            && self.credential_environment == other.credential_environment
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderAccountRecord {
    pub spec: ProviderAccountSpec,
    pub revision: u64,
    pub has_secret: bool,
    pub deleted: bool,
    pub selections: u64,
    pub requests: u64,
    pub failures: u64,
    pub cooldown_until: i64,
    pub last_status: Option<u16>,
    pub last_latency_millis: Option<u64>,
    pub updated_at: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub reserve_percent: Option<u8>,
    #[serde(default)]
    pub exhausted_until: Option<String>,
    #[serde(default)]
    pub reserve_enforceable: bool,
    #[serde(default)]
    pub reserve_reason: Option<String>,
    #[serde(default)]
    pub windows: Vec<UsageWindow>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderAccountPolicy {
    pub provider: String,
    pub mode: AccountRoutingMode,
    pub account_id: Option<Uuid>,
    pub revision: u64,
    #[serde(default)]
    pub order: Vec<Uuid>,
}

impl ProviderAccountPolicy {
    #[must_use]
    pub fn inherited(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            mode: AccountRoutingMode::Inherited,
            account_id: None,
            revision: 0,
            order: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderAccountBinding {
    pub session_id: Uuid,
    pub provider: String,
    pub routing_mode: AccountRoutingMode,
    pub account_id: Option<Uuid>,
    /// HTTP credential owner may differ after a rejection before the first accepted response.
    pub upstream_account_id: Option<Uuid>,
    pub native_session_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderAccountsSnapshot {
    pub version: u32,
    pub accounts: Vec<ProviderAccountRecord>,
    pub policies: Vec<ProviderAccountPolicy>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionAccount {
    pub id: Uuid,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageWindow {
    pub provider: String,
    pub account_id: Option<Uuid>,
    pub kind: String,
    pub used_percent: Option<f64>,
    pub remaining: Option<u64>,
    pub limit: Option<u64>,
    pub window_minutes: Option<u32>,
    pub resets_at: Option<String>,
    pub observed_at: String,
    pub source: String,
    pub plan_type: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageBucketSize {
    Hour,
    #[default]
    Day,
}

/// Unlike Option<Uuid>, preserves absent (all accounts) versus explicit null (personal).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum UsageAccountFilter {
    #[default]
    All,
    Account(Option<Uuid>),
}

impl<'de> Deserialize<'de> for UsageAccountFilter {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<Uuid>::deserialize(deserializer).map(Self::Account)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AccountUsageParams {
    pub from: Option<String>,
    pub to: Option<String>,
    #[serde(default)]
    pub bucket: UsageBucketSize,
    pub provider: Option<String>,
    #[serde(default)]
    pub account_id: UsageAccountFilter,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AccountPreferencesParams {
    pub id: Uuid,
    pub expected_revision: u64,
    pub reserve_percent: Option<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageBucket {
    pub start: String,
    pub provider: String,
    pub account_id: Option<Uuid>,
    pub sessions: u64,
    pub turns_completed: u64,
    pub turns_failed: u64,
    pub requests: u64,
    pub failures: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub tokens_reported: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageObservability {
    pub windows: bool,
    pub tokens: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageResult {
    pub buckets: Vec<AccountUsageBucket>,
    pub windows: Vec<UsageWindow>,
    pub observable_providers: std::collections::BTreeMap<String, UsageObservability>,
}
