//! Owner extension control. One gate serializes all mutations, including each provider's native CLI.
mod inventory;
mod plugins;
mod skills;

use super::{App, invalid_params, store_error, to_value};
use chrono::{DateTime, Utc};
use lazoo_protocol::{AccountCredentialKind, RpcError};
use lazoo_providers::{
    ProviderKind,
    extensions::{ExtensionError, ExtensionTarget, HarnessHome, SkillLibrary, protect_dir},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, str::FromStr};
use uuid::Uuid;

#[derive(Default)]
pub(super) struct ExtensionService {
    skills: BTreeMap<Uuid, skills::SkillPreview>,
    plugins: BTreeMap<Uuid, plugins::PluginPreview>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyParams {
    preview_id: Uuid,
    expected_revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewHeader {
    preview_id: Uuid,
    revision: u64,
    expires_at: DateTime<Utc>,
}

impl App {
    pub(super) async fn extension_rpc(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        let mut service = self.extensions.lock().await;
        self.expire_skill_staging(&service)?;
        // The cap bounds both memory and disk. Expired previews are removed only from owned UUID staging paths.
        let expired: Vec<_> = service
            .skills
            .iter()
            .filter(|(_, p)| p.header.expires_at < Utc::now())
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            service.skills.remove(&id);
            let path = self.extension_root().join("staging").join(id.to_string());
            if path.exists() {
                lazoo_providers::extensions::check_ancestors(&path).map_err(extension_error)?;
                std::fs::remove_dir_all(path).map_err(|e| extension_error(e.into()))?;
            }
        }
        service
            .plugins
            .retain(|_, p| p.header.expires_at >= Utc::now());
        match method {
            "skill.list" => {
                super::require_empty_params(&params, method)?;
                self.skill_list()
            }
            "skill.source.preview" => self.skill_source_preview(params, &mut service).await,
            "skill.update.preview" => self.skill_update_preview(params, &mut service).await,
            "skill.install" | "skill.update" | "skill.expose" | "skill.unexpose"
            | "skill.remove" => self.skill_mutate(method, params, &mut service).await,
            "plugin.list" | "plugin.marketplace.list" => self.plugin_list(method, params).await,
            "plugin.preview" => self.plugin_preview(params, &mut service).await,
            "plugin.install"
            | "plugin.uninstall"
            | "plugin.enable"
            | "plugin.disable"
            | "plugin.marketplace.add"
            | "plugin.marketplace.remove" => self.plugin_mutate(method, params, &mut service).await,
            _ => Err(RpcError {
                code: "LZ-METHOD-NOT-FOUND".into(),
                message: "extension method not found".into(),
                data: None,
            }),
        }
    }

    fn extension_root(&self) -> PathBuf {
        self.root.root().join("extensions")
    }

    fn extension_library(&self) -> Result<(lazoo_store::ExtensionState, SkillLibrary), RpcError> {
        let state = self.store.extension_state().map_err(store_error)?;
        let library = serde_json::from_value(state.state.clone())
            .map_err(|_| extension_error(ExtensionError::Format))?;
        Ok((state, library))
    }

    fn check_extension_revision(
        &self,
        revision: u64,
    ) -> Result<(lazoo_store::ExtensionState, SkillLibrary), RpcError> {
        let (state, library) = self.extension_library()?;
        if state.pending.is_some() {
            return Err(RpcError {code:"LZ-EXTENSION-RECOVERY-REQUIRED".into(),message:"an interrupted extension operation requires inspection of its private journal before further mutations".into(),data:Some(json!({"revision":state.revision,"pending":state.pending}))});
        }
        if state.revision != revision {
            return Err(RpcError {
                code: "LZ-EXTENSION-STALE".into(),
                message: "extension revision changed; refresh and preview again".into(),
                data: Some(json!({"currentRevision":state.revision})),
            });
        }
        Ok((state, library))
    }

    fn extension_homes(&self) -> Result<Vec<HarnessHome>, RpcError> {
        let mut homes = Vec::new();
        for provider in ProviderKind::ALL {
            homes.push(
                HarnessHome::personal(provider, &self.provider_environment)
                    .map_err(extension_error)?,
            );
        }
        for account in self.store.provider_accounts().map_err(store_error)? {
            if account.deleted
                || account.spec.credential_kind != AccountCredentialKind::NativeProfile
            {
                continue;
            }
            let provider = ProviderKind::from_str(&account.spec.provider)
                .map_err(|_| invalid_params("account provider is invalid"))?;
            let home = HarnessHome::personal(provider, &self.provider_environment)
                .map_err(extension_error)?
                .profile(
                    account.spec.id,
                    std::path::Path::new(&account.spec.profile_directory),
                    &super::accounts::profile_environment(&account.spec),
                );
            homes.push(home);
        }
        Ok(homes)
    }

    fn extension_home(&self, target: &ExtensionTarget) -> Result<HarnessHome, RpcError> {
        self.extension_homes()?.into_iter().find(|h|&h.target==target).ok_or_else(||invalid_params("target must name a personal harness or an existing native profile of that harness"))
    }

    fn extension_executable(&self, provider: ProviderKind) -> Result<PathBuf, RpcError> {
        self.providers
            .discover(&self.provider_environment)
            .into_iter()
            .find(|d| d.provider == provider)
            .and_then(|d| d.candidate)
            .map(|c| c.path)
            .ok_or_else(|| RpcError {
                code: "LZ-EXTENSION-NOT-DETECTED".into(),
                message: "native harness executable was not detected".into(),
                data: None,
            })
    }

    fn preview_header(&self, service: &ExtensionService) -> Result<PreviewHeader, RpcError> {
        let staged = std::fs::read_dir(self.extension_root().join("staging"))
            .map_or(0, |items| items.take(8).count());
        if staged.max(service.skills.len()) + service.plugins.len() >= 8 {
            return Err(extension_error(ExtensionError::Limit(
                "8 active previews; wait for expiry",
            )));
        }
        let revision = self.store.extension_state().map_err(store_error)?.revision;
        protect_dir(&self.extension_root()).map_err(extension_error)?;
        Ok(PreviewHeader {
            preview_id: Uuid::new_v4(),
            revision,
            expires_at: Utc::now() + chrono::Duration::minutes(10),
        })
    }

    fn expire_skill_staging(&self, service: &ExtensionService) -> Result<(), RpcError> {
        let root = self.extension_root().join("staging");
        if !root.exists() {
            return Ok(());
        }
        lazoo_providers::extensions::check_ancestors(&root).map_err(extension_error)?;
        for entry in std::fs::read_dir(root)
            .map_err(|e| extension_error(e.into()))?
            .take(9)
        {
            let entry = entry.map_err(|e| extension_error(e.into()))?;
            let Ok(id) = Uuid::parse_str(&entry.file_name().to_string_lossy()) else {
                continue;
            };
            if service.skills.contains_key(&id) {
                continue;
            }
            let path = entry.path();
            lazoo_providers::extensions::check_ancestors(&path).map_err(extension_error)?;
            let marker = path.join("preview.json");
            if std::fs::metadata(&marker).is_ok_and(|m| m.len() <= 4096)
                && let Ok(bytes) = std::fs::read(marker)
                && let Ok(header) = serde_json::from_slice::<PreviewHeader>(&bytes)
                && header.preview_id == id
                && header.expires_at < Utc::now()
            {
                std::fs::remove_dir_all(path).map_err(|e| extension_error(e.into()))?;
            }
        }
        Ok(())
    }
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|_| {
        invalid_params("invalid extension parameters; check the documented request shape")
    })
}

#[allow(clippy::needless_pass_by_value)] // Used directly as the Result::map_err adapter.
fn extension_error(error: ExtensionError) -> RpcError {
    let code = match &error {
        ExtensionError::Invalid(_) => "LZ-INVALID-PARAMS",
        ExtensionError::Unsafe(_) => "LZ-EXTENSION-UNSAFE",
        ExtensionError::Limit(_) => "LZ-EXTENSION-LIMIT",
        ExtensionError::Drift(_) => "LZ-EXTENSION-DRIFT",
        ExtensionError::Unsupported(_) => "LZ-EXTENSION-UNSUPPORTED",
        ExtensionError::Command => "LZ-EXTENSION-NATIVE-FAILED",
        ExtensionError::Timeout => "LZ-EXTENSION-TIMEOUT",
        ExtensionError::Format | ExtensionError::Json(_) => "LZ-EXTENSION-FORMAT",
        ExtensionError::Io(_) | ExtensionError::Runtime(_) => "LZ-EXTENSION-IO",
    };
    RpcError {
        code: code.into(),
        message: error.to_string(),
        data: None,
    }
}
