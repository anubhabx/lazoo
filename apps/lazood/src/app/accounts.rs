//! Owner-only account control and private launch environment construction.
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use lazoo_protocol::{
    AccountApiProtocol, AccountCredentialKind, AccountRoutingMode, PROVIDER_ACCOUNTS_V1,
    ProviderAccountPolicy, ProviderAccountRecord, ProviderAccountSpec, ProviderAccountsSnapshot,
    TerminalOpenParams,
};
use lazoo_providers::ProviderKind;
use lazoo_runtime::{
    LaunchEnvironment, OwnerSecurity, TerminalCommand, protect_for_current_user,
    unprotect_for_current_user,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{App, RpcError, invalid_params, to_value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PutAccount {
    spec: ProviderAccountSpec,
    expected_revision: u64,
    secret: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AccountRevision {
    id: Uuid,
    expected_revision: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct VerifyRoute {
    provider: String,
    model: Option<String>,
}

pub(super) struct SmokeAccountBinding {
    store: lazoo_store::Store,
    pub(super) id: Uuid,
}

impl SmokeAccountBinding {
    pub(super) fn new(store: lazoo_store::Store) -> Self {
        Self {
            store,
            id: Uuid::new_v4(),
        }
    }
}

impl Drop for SmokeAccountBinding {
    fn drop(&mut self) {
        if self
            .store
            .release_transient_account_binding(self.id)
            .is_err()
        {
            tracing::warn!("could not release transient account binding");
        }
    }
}

// Desktop mutations include a transport key. Account writes use their durable ID and
// expected revision as the replay fence, so the key itself contains no stored payload.
fn account_request(mut params: Value) -> Result<Value, RpcError> {
    if let Some(object) = params.as_object_mut()
        && let Some(key) = object.remove("idempotencyKey")
        && !key
            .as_str()
            .is_some_and(|key| !key.is_empty() && key.len() <= 256)
    {
        return Err(invalid_params("invalid account transport key"));
    }
    Ok(params)
}

impl App {
    /// An explicit owner action is the only account setup action that spends model quota.
    pub(super) async fn account_verify(&self, params: Value) -> Result<Value, RpcError> {
        let params: VerifyRoute = serde_json::from_value(account_request(params)?)
            .map_err(|_| invalid_params("invalid routing verification"))?;
        self.provider_smoke(json!({"provider":params.provider,"model":params.model,
            "prompt":"Reply with exactly LAZOO_ACCOUNT_OK. Do not use any tools."}))
            .await
    }

    pub(super) fn overlay_account_observation(
        &self,
        observation: &mut lazoo_protocol::ProviderObservation,
    ) {
        match self.store.provider_account_policy(&observation.provider) {
            Ok(policy) if policy.mode != AccountRoutingMode::Inherited => {
                managed_account_observation(observation);
            }
            Ok(_) => {}
            Err(_) => {
                observation.verification = "degraded".into();
                observation.degradation_reason =
                    Some("account routing policy could not be read".into());
            }
        }
    }
    pub(super) async fn prepare_session_account(
        &self,
        session_id: Uuid,
        provider: ProviderKind,
        request: &mut lazoo_providers::SessionRequest,
    ) -> Result<Option<super::account_proxy::AccountProxy>, RpcError> {
        let account = self
            .store
            .bind_provider_account(
                session_id,
                provider.as_str(),
                request.model.as_deref(),
                request.session_id.as_deref(),
            )
            .map_err(account_error)?;
        let Some(account) = account else {
            return Ok(None);
        };
        validate_managed_model(&account.spec, request.model.as_deref())?;
        let (environment, proxy) = self
            .bound_account_environment(session_id, &account, request.session_id.is_some())
            .await?;
        request.launch_environment = environment;
        Ok(proxy)
    }

    pub(super) async fn bound_account_environment(
        &self,
        session_id: Uuid,
        account: &ProviderAccountRecord,
        resumed: bool,
    ) -> Result<
        (
            LaunchEnvironment,
            Option<super::account_proxy::AccountProxy>,
        ),
        RpcError,
    > {
        if account.deleted || !account.spec.enabled {
            return Err(account_error(lazoo_store::StoreError::AccountUnavailable));
        }
        let binding = self
            .store
            .provider_account_binding(session_id)
            .map_err(account_error)?
            .ok_or_else(|| account_failure("account ownership is unavailable"))?;
        if binding.account_id != Some(account.spec.id) {
            return Err(account_failure("account ownership changed"));
        }
        if binding.routing_mode == AccountRoutingMode::ApiPool {
            let proxy = super::account_proxy::AccountProxy::start(
                self.store.clone(),
                session_id,
                account,
                resumed,
            )
            .await?;
            let mut environment = profile_environment(&account.spec);
            environment.set(
                account
                    .spec
                    .credential_environment
                    .as_deref()
                    .ok_or_else(|| account_failure("API key type is missing"))?,
                proxy.token(),
            );
            let protocol = account
                .spec
                .api_protocol
                .ok_or_else(|| account_failure("API protocol is missing"))?;
            apply_endpoint(&mut environment, &account.spec, &proxy.endpoint(protocol))?;
            Ok((environment, Some(proxy)))
        } else {
            if account.cooldown_until > chrono::Utc::now().timestamp() {
                return Err(account_error(lazoo_store::StoreError::AccountUnavailable));
            }
            Ok((self.account_environment(account)?, None))
        }
    }

    pub(super) fn account_list(&self, params: &Value) -> Result<Value, RpcError> {
        super::require_empty_params(params, "provider.accounts.list")?;
        let accounts = self.store.provider_accounts().map_err(account_error)?;
        let policies = ProviderKind::ALL
            .into_iter()
            .map(|provider| self.store.provider_account_policy(provider.as_str()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(account_error)?;
        Ok(to_value(ProviderAccountsSnapshot {
            version: PROVIDER_ACCOUNTS_V1,
            accounts,
            policies,
        }))
    }

    pub(super) fn account_put(&self, params: Value) -> Result<Value, RpcError> {
        // Never include deserialize errors: they can quote a submitted credential.
        let mut params: PutAccount = serde_json::from_value(account_request(params)?)
            .map_err(|_| invalid_params("invalid account request"))?;
        let provider = ProviderKind::from_str(&params.spec.provider)
            .map_err(|_| invalid_params("unknown account provider"))?;
        params.spec.provider = provider.to_string();
        validate_account_api(&params.spec)?;
        if params.spec.profile_directory.is_empty() {
            params.spec.profile_directory = self
                .root
                .root()
                .join("provider-accounts")
                .join(params.spec.id.to_string())
                .to_string_lossy()
                .into_owned();
        }
        let path = PathBuf::from(&params.spec.profile_directory);
        if !path.is_absolute() {
            return Err(invalid_params("profile directory must be absolute"));
        }
        // The account controls creation of its own directory, never edits the owner's login.
        if !path.exists() {
            std::fs::create_dir_all(&path)
                .map_err(|_| account_failure("could not create account profile"))?;
            OwnerSecurity::current()
                .and_then(|owner| owner.apply_to_path(&path))
                .map_err(|_| account_failure("could not protect account profile"))?;
        }
        if !path.is_dir() {
            return Err(invalid_params("profile directory must be a directory"));
        }
        let protected = params
            .secret
            .as_ref()
            .map(|secret| {
                if secret.trim().is_empty()
                    || secret.len() > 16_384
                    || secret.chars().any(char::is_control)
                {
                    return Err(invalid_params("invalid credential"));
                }
                protect_for_current_user(secret.as_bytes())
                    .map_err(|_| account_failure("could not protect credential"))
            })
            .transpose()?;
        let record = self
            .store
            .put_provider_account(&params.spec, params.expected_revision, protected.as_deref())
            .map_err(account_error)?;
        self.publish_events()?;
        Ok(to_value(record))
    }

    pub(super) fn account_remove(&self, params: Value) -> Result<Value, RpcError> {
        let params: AccountRevision = serde_json::from_value(account_request(params)?)
            .map_err(|_| invalid_params("invalid account revision"))?;
        let record = self
            .store
            .remove_provider_account(params.id, params.expected_revision)
            .map_err(account_error)?;
        self.publish_events()?;
        Ok(to_value(record))
    }

    pub(super) fn account_policy_set(&self, params: Value) -> Result<Value, RpcError> {
        let mut policy: ProviderAccountPolicy = serde_json::from_value(account_request(params)?)
            .map_err(|_| invalid_params("invalid account policy"))?;
        let provider = ProviderKind::from_str(&policy.provider)
            .map_err(|_| invalid_params("unknown account provider"))?;
        policy.provider = provider.to_string();
        if policy.mode == AccountRoutingMode::ApiPool
            && !matches!(
                provider,
                ProviderKind::Codex | ProviderKind::ClaudeCode | ProviderKind::OpenCode
            )
        {
            return Err(invalid_params(
                "this provider supports session routing; compatible API proxying is unavailable",
            ));
        }
        let policy = self
            .store
            .set_provider_account_policy(&policy)
            .map_err(account_error)?;
        self.publish_events()?;
        Ok(to_value(policy))
    }

    /// Opens the provider's own login UI in the selected profile. No account credentials appear
    /// in terminal metadata or command-line arguments. The owner completes interactive sign-in.
    pub(super) async fn account_login(&self, params: Value) -> Result<Value, RpcError> {
        let params: AccountRevision = serde_json::from_value(account_request(params)?)
            .map_err(|_| invalid_params("invalid account revision"))?;
        let account = self
            .store
            .provider_account(params.id)
            .map_err(account_error)?;
        if account.revision != params.expected_revision {
            return Err(account_error(lazoo_store::StoreError::AccountStale));
        }
        if account.deleted
            || !account.spec.enabled
            || account.spec.credential_kind != AccountCredentialKind::NativeProfile
        {
            return Err(invalid_params("login requires an enabled native profile"));
        }
        let provider =
            ProviderKind::from_str(&account.spec.provider).map_err(super::provider_error)?;
        let executable = self
            .providers
            .discover(&self.provider_environment)
            .into_iter()
            .find(|entry| entry.provider == provider)
            .and_then(|entry| entry.candidate)
            .ok_or_else(|| account_failure("provider executable was not detected"))?
            .path;
        let args: Vec<String> = match provider {
            ProviderKind::Codex | ProviderKind::GitHubCopilot => vec!["login".into()],
            ProviderKind::ClaudeCode | ProviderKind::OpenCode => {
                vec!["auth".into(), "login".into()]
            }
            ProviderKind::Antigravity | ProviderKind::GrokBuild => Vec::new(),
        };
        let environment = self.account_environment(&account)?;
        let params = TerminalOpenParams {
            executable: executable.to_string_lossy().into_owned(),
            args: args.clone(),
            cwd: account.spec.profile_directory.clone(),
            columns: 100,
            rows: 32,
            idempotency_key: format!("account-login:{}:{}", account.spec.id, Uuid::new_v4()),
        };
        let command = environment.apply_to_terminal(
            TerminalCommand::new(executable, &params.cwd, params.columns, params.rows).args(args),
        );
        let terminal = self
            .terminal_open_with_command(params, command, None)
            .await
            .map_err(|_| account_failure("account login terminal could not start"))?;
        Ok(to_value(terminal))
    }

    pub(super) fn account_environment(
        &self,
        account: &ProviderAccountRecord,
    ) -> Result<LaunchEnvironment, RpcError> {
        if account.deleted || !account.spec.enabled {
            return Err(account_error(lazoo_store::StoreError::AccountUnavailable));
        }
        let mut environment = profile_environment(&account.spec);
        if account.spec.credential_kind == AccountCredentialKind::ApiKey {
            let protected = self
                .store
                .provider_account_secret(account.spec.id)
                .map_err(account_error)?
                .ok_or_else(|| account_failure("account credential is unavailable"))?;
            let bytes = unprotect_for_current_user(&protected).map_err(|_| {
                account_failure("account credential cannot be opened for this Windows user")
            })?;
            let secret = String::from_utf8(bytes)
                .map_err(|_| account_failure("account credential is invalid"))?;
            environment.set(
                account
                    .spec
                    .credential_environment
                    .as_deref()
                    .ok_or_else(|| account_failure("credential environment is missing"))?,
                secret,
            );
            if let Some(base_url) = &account.spec.base_url {
                apply_endpoint(&mut environment, &account.spec, base_url)?;
            }
        }
        Ok(environment)
    }
}

fn validate_managed_model(spec: &ProviderAccountSpec, model: Option<&str>) -> Result<(), RpcError> {
    // A configured endpoint does not select OpenCode's default model. Require the backend
    // explicitly so an account verification cannot succeed through a different/free provider.
    if spec.provider == "opencode"
        && spec.credential_kind == AccountCredentialKind::ApiKey
        && !model
            .and_then(|value| value.split_once('/'))
            .is_some_and(|(backend, model)| {
                spec.backend_provider.as_deref() == Some(backend) && !model.is_empty()
            })
    {
        return Err(account_failure(
            "managed OpenCode API credentials require a matching backend/model selection",
        ));
    }
    Ok(())
}

fn managed_account_observation(observation: &mut lazoo_protocol::ProviderObservation) {
    // Personal authentication is not evidence about the selected account. Retain actual
    // binary/transport failures; a compatible transport still needs an explicit routed smoke.
    let authentication_only_failure = observation.degradation_reason.as_deref()
        == Some("provider authentication is not ready")
        && ["native", "external"].iter().any(|channel| {
            matches!(
                observation
                    .authentication
                    .get(channel)
                    .and_then(Value::as_str),
                Some("needs-login" | "blocked")
            )
        });
    if observation
        .capabilities
        .get("session")
        .and_then(Value::as_bool)
        != Some(true)
        || observation.verification == "not-detected"
        || (observation.verification == "degraded" && !authentication_only_failure)
    {
        return;
    }
    observation.verification = "configured".into();
    observation.degradation_reason = None;
    observation.authentication = json!({"native":"unknown","external":"unknown",
        "detail":"managed account routing requires an explicit model verification"});
}

pub(super) fn profile_environment(spec: &ProviderAccountSpec) -> LaunchEnvironment {
    let mut environment = LaunchEnvironment::default();
    // Clear all alternate credential sources before adding the one selected key/profile.
    for name in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "OPENAI_BASE_URL",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_USE_MANTLE",
        "CLAUDE_CODE_USE_ANTHROPIC_AWS",
        "COPILOT_GITHUB_TOKEN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GOOGLE_GENERATIVE_AI_API_KEY",
        "GOOGLE_API_KEY",
        "GEMINI_API_KEY",
        "OPENROUTER_API_KEY",
        "COPILOT_PROVIDER_API_KEY",
        "COPILOT_PROVIDER_BASE_URL",
        "COPILOT_PROVIDER_TYPE",
        "LAZOO_OPENCODE_PROVIDER_CONFIG",
        "OPENCODE_CONFIG",
        "OPENCODE_CONFIG_CONTENT",
        "OPENCODE_CONFIG_DIR",
    ] {
        environment.remove(name);
    }
    let root = Path::new(&spec.profile_directory);
    match spec.provider.as_str() {
        "codex" => environment.set("CODEX_HOME", root),
        "claude-code" => {
            environment.set("CLAUDE_CONFIG_DIR", root);
            environment.set("CLAUDE_SECURESTORAGE_CONFIG_DIR", root);
        }
        "github-copilot" => {
            environment.set("COPILOT_HOME", root);
            environment.set("GH_CONFIG_DIR", root.join("gh"));
        }
        "opencode" => {
            environment.set("HOME", root);
            environment.set("USERPROFILE", root);
            environment.set("APPDATA", root.join("AppData/Roaming"));
            environment.set("LOCALAPPDATA", root.join("AppData/Local"));
            for (name, suffix) in [
                ("XDG_DATA_HOME", "data"),
                ("XDG_CONFIG_HOME", "config"),
                ("XDG_STATE_HOME", "state"),
                ("XDG_CACHE_HOME", "cache"),
            ] {
                environment.set(name, root.join(suffix));
            }
            if spec.credential_kind == AccountCredentialKind::ApiKey
                && let (Some(backend), Some(key)) = (
                    spec.backend_provider.as_deref(),
                    spec.credential_environment.as_deref(),
                )
            {
                // Preserve the provider's default endpoint while marking this as an explicit
                // managed backend. The adapter also restricts enabled providers to these keys.
                environment.set(
                    "LAZOO_OPENCODE_PROVIDER_CONFIG",
                    json!({
                        (backend): {"options": {"apiKey": format!("{{env:{key}}}")}}
                    })
                    .to_string(),
                );
            }
        }
        _ => {
            environment.set("HOME", root);
            environment.set("USERPROFILE", root);
            environment.set("APPDATA", root.join("AppData/Roaming"));
            environment.set("LOCALAPPDATA", root.join("AppData/Local"));
        }
    }
    environment
}

pub(super) fn apply_endpoint(
    environment: &mut LaunchEnvironment,
    spec: &ProviderAccountSpec,
    endpoint: &str,
) -> Result<(), RpcError> {
    match spec.provider.as_str() {
        "codex" => environment.set("OPENAI_BASE_URL", endpoint),
        // Anthropic's native SDK appends /v1/messages; OpenCode's AI SDK appends /messages.
        "claude-code" => environment.set(
            "ANTHROPIC_BASE_URL",
            endpoint.trim_end_matches('/').trim_end_matches("/v1"),
        ),
        "opencode" => {
            // Provider adapter merges this restricted object into its own MCP/permission config.
            environment.set("LAZOO_OPENCODE_PROVIDER_CONFIG", serde_json::to_string(&json!({
                spec.backend_provider.as_deref().unwrap_or("openai"): {"options": {"baseURL": endpoint, "apiKey": format!("{{env:{}}}", spec.credential_environment.as_deref().unwrap_or("OPENAI_API_KEY"))}}
            })).map_err(|_| account_failure("could not prepare provider endpoint"))?);
        }
        _ => {
            return Err(invalid_params(
                "custom API endpoints are not supported for this provider",
            ));
        }
    }
    Ok(())
}

fn validate_account_api(spec: &ProviderAccountSpec) -> Result<(), RpcError> {
    if spec.credential_kind == AccountCredentialKind::NativeProfile {
        return Ok(());
    }
    let accepted = match spec.provider.as_str() {
        "codex" => matches!(
            spec.credential_environment.as_deref(),
            Some("OPENAI_API_KEY")
        ),
        "claude-code" => matches!(
            spec.credential_environment.as_deref(),
            Some("ANTHROPIC_API_KEY" | "CLAUDE_CODE_OAUTH_TOKEN")
        ),
        "opencode" => matches!(
            spec.credential_environment.as_deref(),
            Some(
                "OPENAI_API_KEY"
                    | "ANTHROPIC_API_KEY"
                    | "GOOGLE_GENERATIVE_AI_API_KEY"
                    | "OPENROUTER_API_KEY"
            )
        ),
        "github-copilot" => matches!(
            spec.credential_environment.as_deref(),
            Some("COPILOT_GITHUB_TOKEN")
        ),
        _ => false,
    };
    if !accepted {
        return Err(invalid_params(
            "unsupported credential environment for this provider",
        ));
    }
    if let Some(url) = &spec.base_url {
        if !matches!(spec.provider.as_str(), "codex" | "claude-code" | "opencode") {
            return Err(invalid_params(
                "custom API endpoints are not supported for this provider",
            ));
        }
        validate_endpoint(url)?;
    }
    if let Some(protocol) = spec.api_protocol {
        let valid = match spec.provider.as_str() {
            "codex" => protocol == AccountApiProtocol::Responses,
            "claude-code" => {
                protocol == AccountApiProtocol::Anthropic
                    && spec.credential_environment.as_deref() == Some("ANTHROPIC_API_KEY")
            }
            "opencode" => match spec.backend_provider.as_deref() {
                Some("openai") => {
                    protocol == AccountApiProtocol::Responses
                        && spec.credential_environment.as_deref() == Some("OPENAI_API_KEY")
                }
                Some("anthropic") => {
                    protocol == AccountApiProtocol::Anthropic
                        && spec.credential_environment.as_deref() == Some("ANTHROPIC_API_KEY")
                }
                _ => false,
            },
            _ => false,
        };
        if !valid {
            return Err(invalid_params(
                "unsupported API pool protocol for this credential",
            ));
        }
    }
    if spec.provider == "opencode"
        && spec.backend_provider.as_ref().is_none_or(|name| {
            name.is_empty()
                || name.len() > 80
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        })
    {
        return Err(invalid_params(
            "OpenCode keys require a backend provider identifier",
        ));
    }
    Ok(())
}

pub(super) fn validate_endpoint(value: &str) -> Result<reqwest::Url, RpcError> {
    let url = reqwest::Url::parse(value).map_err(|_| invalid_params("invalid API endpoint"))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(invalid_params(
            "API endpoint requires HTTPS (HTTP only on loopback), without credentials, query or fragment",
        ));
    }
    Ok(url)
}

pub(super) fn account_failure(message: &str) -> RpcError {
    RpcError {
        code: "LZ-ACCOUNT".into(),
        message: message.into(),
        data: None,
    }
}
#[allow(clippy::needless_pass_by_value)] // Consumes Store errors at Result::map_err boundaries.
pub(super) fn account_error(error: lazoo_store::StoreError) -> RpcError {
    match error {
        lazoo_store::StoreError::AccountValidation(message) => invalid_params(message),
        lazoo_store::StoreError::AccountStale => RpcError {
            code: "LZ-ACCOUNT-STALE".into(),
            message: "account settings changed; reload before editing".into(),
            data: None,
        },
        lazoo_store::StoreError::AccountNotFound(_) => account_failure("account was not found"),
        lazoo_store::StoreError::AccountUnavailable => {
            account_failure("no eligible account is available; check the account or pool settings")
        }
        _ => account_failure("account storage operation failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(directory: &Path, provider: &str) -> ProviderAccountSpec {
        ProviderAccountSpec {
            id: Uuid::new_v4(),
            provider: provider.into(),
            label: "Fixture".into(),
            credential_kind: AccountCredentialKind::NativeProfile,
            profile_directory: directory.to_string_lossy().into_owned(),
            credential_environment: None,
            base_url: None,
            api_protocol: None,
            backend_provider: None,
            models: vec![],
            enabled: true,
        }
    }

    #[test]
    fn managed_opencode_keys_require_an_explicit_matching_backend_model() {
        let temp = tempfile::tempdir().unwrap();
        let mut account = spec(temp.path(), "opencode");
        assert!(validate_managed_model(&account, None).is_ok());
        assert!(
            !profile_environment(&account)
                .values()
                .contains_key(std::ffi::OsStr::new("LAZOO_OPENCODE_PROVIDER_CONFIG"))
        );
        account.credential_kind = AccountCredentialKind::ApiKey;
        account.backend_provider = Some("openai".into());
        account.credential_environment = Some("OPENAI_API_KEY".into());
        let environment = profile_environment(&account);
        let config: Value = serde_json::from_str(
            &environment
                .values()
                .get(std::ffi::OsStr::new("LAZOO_OPENCODE_PROVIDER_CONFIG"))
                .unwrap()
                .to_string_lossy(),
        )
        .unwrap();
        assert_eq!(
            config,
            json!({"openai":{"options":{"apiKey":"{env:OPENAI_API_KEY}"}}})
        );
        for model in [
            None,
            Some("gpt-5"),
            Some("anthropic/claude-sonnet"),
            Some("openai/"),
        ] {
            assert!(validate_managed_model(&account, model).is_err());
        }
        assert!(validate_managed_model(&account, Some("openai/gpt-5")).is_ok());
        account.provider = "codex".into();
        assert!(validate_managed_model(&account, None).is_ok());
    }

    #[test]
    fn managed_routing_does_not_use_personal_login_or_hide_transport_failures() {
        let mut observation = lazoo_protocol::ProviderObservation {
            provider: "codex".into(),
            verification: "degraded".into(),
            version: Some("fixture".into()),
            protocol: Some("app-server/v2".into()),
            capabilities: json!({"session":true}),
            authentication: json!({"native":"needs-login","external":"unknown"}),
            degradation_reason: Some("provider authentication is not ready".into()),
            checked_at: "fixture".into(),
        };
        managed_account_observation(&mut observation);
        assert_eq!(observation.authentication["native"], "unknown");
        assert_eq!(observation.verification, "configured");
        assert!(observation.degradation_reason.is_none());
        assert_eq!(
            lazoo_domain::derive_availability(true, Some(&observation)).0,
            lazoo_protocol::EmployeeAvailability::Unverified
        );
        let temp = tempfile::tempdir().unwrap();
        let store = lazoo_store::Store::open_path(temp.path().join("test.db")).unwrap();
        let mut verified = observation.clone();
        verified.verification = "verified".into();
        verified.authentication = json!({"native":"ready","external":"unknown"});
        store
            .create_employee(&lazoo_store::CreateEmployeeInput {
                request_id: "create".into(),
                idempotency_key: "create".into(),
                name: "routed-worker".into(),
                provider: "codex".into(),
                role: "worker".into(),
                instructions: String::new(),
                model: None,
                reasoning_effort: None,
                native_settings: json!({}),
                authority_mode: lazoo_protocol::AuthorityMode::Ask,
                checkout_policy: lazoo_protocol::EmployeeCheckoutPolicy::Isolated,
                activation_policy: lazoo_protocol::ActivationPolicy::Manual,
                idle_timeout_seconds: 0,
                enabled: true,
                provider_observation: Some(verified),
            })
            .unwrap();
        store
            .refresh_provider_observations(&lazoo_store::RefreshProviderObservationsInput {
                request_id: "refresh".into(),
                idempotency_key: "refresh".into(),
                observations: vec![observation.clone()],
            })
            .unwrap();
        let roster = store.roster_snapshot().unwrap();
        assert_eq!(
            roster.entries[0]
                .provider_observation
                .as_ref()
                .unwrap()
                .verification,
            "verified"
        );
        for state in ["not-detected", "degraded"] {
            observation.verification = state.into();
            observation.degradation_reason = Some("actual transport failure".into());
            managed_account_observation(&mut observation);
            assert_eq!(observation.verification, state);
            assert_eq!(
                lazoo_domain::derive_availability(true, Some(&observation)).0,
                lazoo_protocol::EmployeeAvailability::Offline
            );
        }
    }

    #[test]
    fn selected_profiles_scrub_alternate_credentials_and_launch_environment_is_private() {
        let temp = tempfile::tempdir().unwrap();
        for (provider, key) in [
            ("codex", "CODEX_HOME"),
            ("claude-code", "CLAUDE_CONFIG_DIR"),
            ("github-copilot", "COPILOT_HOME"),
            ("opencode", "HOME"),
            ("antigravity", "USERPROFILE"),
        ] {
            let environment = profile_environment(&spec(temp.path(), provider));
            assert_eq!(
                environment.values().get(std::ffi::OsStr::new(key)).unwrap(),
                temp.path().as_os_str()
            );
            for removed in [
                "OPENAI_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "GITHUB_TOKEN",
                "OPENROUTER_API_KEY",
                "GOOGLE_API_KEY",
                "OPENCODE_CONFIG_CONTENT",
            ] {
                assert!(
                    environment.removals().iter().any(|name| name == removed),
                    "{provider} inherited {removed}"
                );
            }
        }
        let mut request = lazoo_providers::SessionRequest::new(temp.path());
        request
            .launch_environment
            .set("OPENAI_API_KEY", "not-for-snapshots");
        assert!(
            !serde_json::to_string(&request)
                .unwrap()
                .contains("not-for-snapshots")
        );
        assert!(!format!("{request:?}").contains("not-for-snapshots"));
    }

    #[test]
    fn transient_binding_is_released_after_success_and_early_error() {
        let temp = tempfile::tempdir().unwrap();
        let store = lazoo_store::Store::open_path(temp.path().join("test.db")).unwrap();
        for early_error in [false, true] {
            let guard = SmokeAccountBinding::new(store.clone());
            let id = guard.id;
            let outcome: Result<(), ()> = (|| {
                let _guard = guard;
                store
                    .bind_provider_account(id, "codex", None, None)
                    .unwrap();
                assert!(store.provider_account_binding(id).unwrap().is_some());
                if early_error {
                    return Err(());
                }
                Ok(())
            })();
            assert_eq!(outcome.is_err(), early_error);
            assert!(store.provider_account_binding(id).unwrap().is_none());
        }
    }
}
