use super::{
    App, ApplyParams, ExtensionService, PreviewHeader, extension_error, invalid_params, parse,
    store_error, to_value,
};
use lazoo_protocol::RpcError;
use lazoo_providers::{
    ProviderKind,
    extensions::{
        ExtensionError, ExtensionTarget, FileTree, HarnessHome, check_ancestors,
        plugins::{self, MarketplaceSelection, PluginAction, PluginObservation},
        protect_dir, read_bounded, read_tree, tree_checksum, write_tree,
    },
};
use lazoo_runtime::atomic_write;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginRequest {
    action: PluginAction,
    targets: Vec<ExtensionTarget>,
    name: String,
    #[serde(default)]
    marketplace: Option<MarketplaceSelection>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PluginPreview {
    #[serde(flatten)]
    pub header: PreviewHeader,
    request: PluginRequest,
    targets: Vec<PlannedTarget>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlannedTarget {
    target: ExtensionTarget,
    before: Option<PluginObservation>,
    commands: Vec<Vec<String>>,
    error: Option<Value>,
    #[serde(skip)]
    executable: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginListParams {
    #[serde(default)]
    target: Option<ExtensionTarget>,
}

struct NativeBackup {
    files: Vec<(PathBuf, Option<Vec<u8>>)>,
    package: Option<(PathBuf, FileTree)>,
}

impl App {
    pub(super) async fn plugin_list(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let request: PluginListParams = parse(params)?;
        let homes = if let Some(target) = request.target {
            vec![self.extension_home(&target)?]
        } else {
            self.extension_homes()?
        };
        let mut targets = Vec::new();
        for home in homes {
            let result = match self.extension_executable(home.target.provider) {
                Ok(executable) => {
                    if method == "plugin.marketplace.list" {
                        if plugins::capabilities(home.target.provider)["actions"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|v| v == "marketplace.list"))
                        {
                            plugins::marketplaces(&executable, &home)
                                .await
                                .map_err(extension_error)
                                .map(|m| json!({"marketplaces":m}))
                        } else {
                            Err(extension_error(ExtensionError::Unsupported(
                                "marketplace listing unavailable",
                            )))
                        }
                    } else {
                        plugins::observe(&executable, &home)
                            .await
                            .map_err(extension_error)
                            .map(|o| to_value(&o))
                    }
                }
                Err(error) => Err(error),
            };
            targets.push(match result {
                Ok(observed) => json!({"target":home.target,"observed":observed,"error":null}),
                Err(error) => json!({"target":home.target,"observed":null,"error":error}),
            });
        }
        let state = self.store.extension_state().map_err(store_error)?;
        Ok(
            json!({"revision":state.revision,"targets":targets,"capabilities":ProviderKind::ALL.map(plugins::capabilities),"pending":state.pending}),
        )
    }

    pub(super) async fn plugin_preview(
        &self,
        params: Value,
        service: &mut ExtensionService,
    ) -> Result<Value, RpcError> {
        let request: PluginRequest = parse(params)?;
        if request.targets.is_empty() || request.targets.len() > 16 {
            return Err(invalid_params("select 1..16 distinct plugin targets"));
        }
        for (index, target) in request.targets.iter().enumerate() {
            if request.targets[index + 1..].contains(target) {
                return Err(invalid_params("plugin targets must be distinct"));
            }
        }
        plugins::validate_identifier(&request.name).map_err(extension_error)?;
        if let Some(marketplace) = &request.marketplace {
            plugins::validate_marketplace(marketplace).map_err(extension_error)?;
            if request.action == PluginAction::MarketplaceAdd && request.name != marketplace.name {
                return Err(invalid_params(
                    "marketplace add name must match marketplace.name",
                ));
            }
            if request.action == PluginAction::Install
                && !request.name.ends_with(&format!("@{}", marketplace.name))
            {
                return Err(invalid_params(
                    "cross-harness install name must be plugin@marketplace matching marketplace.name",
                ));
            }
        }
        let header = self.preview_header(service)?;
        let mut targets = Vec::new();
        for target in &request.targets {
            let result = self.plan_plugin_target(&request, target).await;
            targets.push(match result {
                Ok(planned) => planned,
                Err(error) => PlannedTarget {
                    target: target.clone(),
                    before: None,
                    commands: Vec::new(),
                    error: Some(to_value(&error)),
                    executable: None,
                },
            });
        }
        let preview = PluginPreview {
            header,
            request,
            targets,
        };
        let result = to_value(&preview);
        service.plugins.insert(preview.header.preview_id, preview);
        Ok(result)
    }

    async fn plan_plugin_target(
        &self,
        request: &PluginRequest,
        target: &ExtensionTarget,
    ) -> Result<PlannedTarget, RpcError> {
        let home = self.extension_home(target)?;
        let command = plugins::mutation_args(
            target.provider,
            request.action,
            &request.name,
            request.marketplace.as_ref(),
        )
        .map_err(extension_error)?;
        let executable = self.extension_executable(target.provider)?;
        let before = plugins::observe(&executable, &home)
            .await
            .map_err(extension_error)?;
        let mut commands = Vec::new();
        if matches!(
            request.action,
            PluginAction::Install | PluginAction::MarketplaceAdd
        ) && let Some(marketplace) = &request.marketplace
        {
            if before.marketplaces.as_array().is_some_and(|a| {
                a.iter().any(|m| {
                    m["name"] == marketplace.name
                        && !plugins::marketplace_source_matches(&m["source"], &marketplace.source)
                })
            }) {
                return Err(extension_error(ExtensionError::Drift(
                    "marketplace name is already registered with a different or unknown source"
                        .into(),
                )));
            }
            if request.action == PluginAction::Install
                && !before
                    .marketplaces
                    .as_array()
                    .is_some_and(|a| a.iter().any(|m| m["name"] == marketplace.name))
            {
                commands.push(
                    plugins::mutation_args(
                        target.provider,
                        PluginAction::MarketplaceAdd,
                        &marketplace.name,
                        Some(marketplace),
                    )
                    .map_err(extension_error)?,
                );
            }
        }
        commands.push(command);
        Ok(PlannedTarget {
            target: target.clone(),
            before: Some(before),
            commands,
            error: None,
            executable: Some(executable),
        })
    }

    pub(super) async fn plugin_mutate(
        &self,
        method: &str,
        params: Value,
        service: &mut ExtensionService,
    ) -> Result<Value, RpcError> {
        let request: ApplyParams = parse(params)?;
        let (state, _) = self.check_extension_revision(request.expected_revision)?;
        let preview = service
            .plugins
            .get(&request.preview_id)
            .cloned()
            .ok_or_else(|| RpcError {
                code: "LZ-EXTENSION-PREVIEW-EXPIRED".into(),
                message: "plugin preview is missing, consumed or expired".into(),
                data: None,
            })?;
        if preview.header.revision != request.expected_revision
            || preview.request.action.method() != method
        {
            return Err(invalid_params(
                "method and revision must match the plugin preview",
            ));
        }
        let operation_id = Uuid::new_v4();
        let backup_dir = self
            .extension_root()
            .join("operations")
            .join(operation_id.to_string());
        protect_dir(&backup_dir).map_err(extension_error)?;
        self.store.begin_extension_operation(request.expected_revision,&json!({"operationId":operation_id,"kind":"plugin","method":method,"targets":preview.request.targets,"backupDirectory":backup_dir})).map_err(store_error)?;
        self.publish_events()?;
        let mut outcomes = Vec::new();
        let mut recovery = false;
        for (index, planned) in preview.targets.iter().enumerate() {
            let outcome = if let Some(error) = &planned.error {
                json!({"target":planned.target,"status":"unsupported-or-unavailable","error":error,"observed":null,"rollbackStatus":"not-needed"})
            } else {
                match self
                    .apply_plugin_target(
                        &preview.request,
                        planned,
                        &backup_dir.join(index.to_string()),
                    )
                    .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        json!({"target":planned.target,"status":"refused","error":error,"observed":null,"rollbackStatus":"not-needed"})
                    }
                }
            };
            recovery |= outcome["status"] == "recovery-needed";
            outcomes.push(outcome);
        }
        let outcome = json!({"operationId":operation_id,"kind":"plugin","method":method,"targets":outcomes,"sessionEffect":"Requires provider restart"});
        let revision = self
            .store
            .finish_extension_operation(request.expected_revision, &state.state, &outcome, recovery)
            .map_err(store_error)?;
        self.publish_events()?;
        service.plugins.remove(&request.preview_id);
        Ok(json!({"revision":revision,"outcome":outcome}))
    }

    async fn apply_plugin_target(
        &self,
        request: &PluginRequest,
        planned: &PlannedTarget,
        backup_dir: &Path,
    ) -> Result<Value, RpcError> {
        let home = self.extension_home(&planned.target)?;
        let executable = self.extension_executable(home.target.provider)?;
        if planned.executable.as_ref() != Some(&executable) {
            return Err(extension_error(ExtensionError::Drift(
                "native executable changed after preview".into(),
            )));
        }
        let before = plugins::observe(&executable, &home)
            .await
            .map_err(extension_error)?;
        if to_value(&before)
            != to_value(
                planned
                    .before
                    .as_ref()
                    .ok_or_else(|| invalid_params("preview observation missing"))?,
            )
        {
            return Err(extension_error(ExtensionError::Drift(
                "native plugin state/version changed after preview".into(),
            )));
        }
        let backup = backup_native(&home, request, &before, backup_dir).map_err(extension_error)?;
        let mut error = None;
        for args in &planned.commands {
            if let Err(failure) = plugins::invoke(&executable, &home, args).await {
                error = Some(extension_error(failure));
                break;
            }
        }
        let observed = plugins::observe(&executable, &home).await;
        if error.is_none()
            && observed.as_ref().is_ok_and(|o| {
                plugins::observed_satisfies(request.action, &request.name, o)
                    && request.marketplace.as_ref().is_none_or(|m| {
                        !matches!(
                            request.action,
                            PluginAction::Install | PluginAction::MarketplaceAdd
                        ) || o.marketplaces.as_array().is_some_and(|a| {
                            a.iter().any(|entry| {
                                entry["name"] == m.name
                                    && plugins::marketplace_source_matches(
                                        &entry["source"],
                                        &m.source,
                                    )
                            })
                        })
                    })
            })
        {
            return Ok(
                json!({"target":planned.target,"status":"applied","error":null,"observed":observed.ok(),"rollbackStatus":"not-needed","backupDirectory":backup_dir}),
            );
        }
        if error.is_none() {
            error = Some(extension_error(ExtensionError::Drift(
                "native read-back did not confirm requested state".into(),
            )));
        }
        // Compensate an install first; uninstall uses its bounded private package snapshot.
        if request.action == PluginAction::Install
            && !plugins::observed_satisfies(PluginAction::Install, &request.name, &before)
            && observed
                .as_ref()
                .is_ok_and(|o| plugins::observed_satisfies(PluginAction::Install, &request.name, o))
            && let Ok(args) = plugins::mutation_args(
                home.target.provider,
                PluginAction::Uninstall,
                &request.name,
                None,
            )
        {
            let _ = plugins::invoke(&executable, &home, &args).await;
        }
        let rollback = restore_native(&backup);
        let restored = plugins::observe(&executable, &home).await;
        let rolled_back = rollback.is_ok()
            && restored
                .as_ref()
                .is_ok_and(|o| to_value(o) == to_value(&before));
        Ok(
            json!({"target":planned.target,"status":if rolled_back{"rolled-back"}else{"recovery-needed"},"error":error,"observed":restored.ok(),"rollbackStatus":if rolled_back{"verified"}else{"failed-or-unverified"},"backupDirectory":backup_dir,"limitations":["native package downloads may leave unused cache files; no unknown cache files are deleted"]}),
        )
    }
}

fn backup_native(
    home: &HarnessHome,
    request: &PluginRequest,
    before: &PluginObservation,
    dir: &Path,
) -> Result<NativeBackup, ExtensionError> {
    protect_dir(dir)?;
    let mut files = Vec::new();
    for (index, path) in plugins::control_files(home).into_iter().enumerate() {
        check_ancestors(&path)?;
        let bytes = match read_bounded(&path, 1024 * 1024) {
            Ok(bytes) => {
                atomic_write(&dir.join(format!("config-{index}")), &bytes)?;
                Some(bytes)
            }
            Err(ExtensionError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        files.push((path, bytes));
    }
    let package = if request.action == PluginAction::Uninstall {
        let installed = before
            .plugins
            .iter()
            .find(|p| plugins::matches_plugin(p, &request.name))
            .ok_or_else(|| ExtensionError::Invalid("plugin is not installed".into()))?;
        let path = installed.path.as_ref().ok_or(ExtensionError::Unsupported(
            "native inventory does not expose an install path for a recoverable uninstall",
        ))?;
        check_ancestors(path)?;
        if !path.starts_with(&home.config) {
            return Err(ExtensionError::Unsafe(
                "plugin install path is outside the selected home".into(),
            ));
        }
        let tree = read_tree(path)?;
        write_tree(&dir.join("package"), &tree)?;
        Some((path.clone(), tree))
    } else {
        None
    };
    atomic_write(
        &dir.join("manifest.json"),
        &serde_json::to_vec(
            &json!({"target":home.target,"files":files.iter().map(|(p,b)|json!({"path":p,"existed":b.is_some()})).collect::<Vec<_>>(),"package":package.as_ref().map(|(p,t)|json!({"path":p,"checksum":tree_checksum(t)}))}),
        )?,
    )?;
    Ok(NativeBackup { files, package })
}

fn restore_native(backup: &NativeBackup) -> Result<(), ExtensionError> {
    // Native CLIs do not expose a compare-and-swap token for their writes. A changed
    // control file could include concurrent user edits; never guess that it is ours.
    // Native compensation may already have restored these bytes. Otherwise retain
    // the private backup and require recovery instead of overwriting external data.
    for (path, before) in &backup.files {
        check_ancestors(path)?;
        let current = match read_bounded(path, 1024 * 1024) {
            Ok(bytes) => Some(bytes),
            Err(ExtensionError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if &current != before {
            return Err(ExtensionError::Drift(
                "native control file changed; automatic restoration would risk concurrent edits"
                    .into(),
            ));
        }
    }
    if let Some((path, tree)) = &backup.package {
        if path.exists() {
            if tree_checksum(&read_tree(path)?) != tree_checksum(tree) {
                return Err(ExtensionError::Drift(
                    "package changed during rollback".into(),
                ));
            }
        } else {
            write_tree(path, tree)?;
        }
    }
    Ok(())
}
