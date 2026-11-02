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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderAccountPolicy {
    pub provider: String,
    pub mode: AccountRoutingMode,
    pub account_id: Option<Uuid>,
    pub revision: u64,
}

impl ProviderAccountPolicy {
    #[must_use]
    pub fn inherited(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            mode: AccountRoutingMode::Inherited,
            account_id: None,
            revision: 0,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderAccountsSnapshot {
    pub version: u32,
    pub accounts: Vec<ProviderAccountRecord>,
    pub policies: Vec<ProviderAccountPolicy>,
}
