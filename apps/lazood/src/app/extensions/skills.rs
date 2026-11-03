use super::{
    App, ExtensionService, PreviewHeader, extension_error, invalid_params, parse, store_error,
    to_value,
};
use lazoo_protocol::RpcError;
use lazoo_providers::{
    ProviderKind,
    extensions::{
        ExposureMode, ExtensionError, FileTree, LibrarySkill, SkillExposure, SkillInfo,
        SkillLibrary, SkillSource, SkillTransaction, check_ancestors, protect_dir, read_tree,
        stage_source, tree_checksum, verify_exposure,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::PathBuf};
use uuid::Uuid;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SkillPreview {
    #[serde(flatten)]
    pub header: PreviewHeader,
    source: SkillSource,
    skills: Vec<SkillInfo>,
    resolved_revision: Option<String>,
    conflicts: Vec<Value>,
    update_name: Option<String>,
    #[serde(skip)]
    staging: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallParams {
    preview_id: Uuid,
    names: Vec<String>,
    #[serde(default)]
    providers: Vec<ProviderKind>,
    mode: ExposureMode,
    #[serde(default)]
    replace: bool,
    expected_revision: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExposeParams {
    name: String,
    providers: Vec<ProviderKind>,
    mode: ExposureMode,
    expected_revision: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UnexposeParams {
    name: String,
    providers: Vec<ProviderKind>,
    expected_revision: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoveParams {
    name: String,
    expected_revision: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdatePreviewParams {
    name: String,
}

struct PreparedSkillMutation {
    revision: u64,
    before: SkillLibrary,
    after: SkillLibrary,
    trees: Vec<(PathBuf, Option<String>, Option<FileTree>)>,
    preview_id: Option<Uuid>,
}

impl App {
    pub(super) async fn skill_source_preview(
        &self,
        params: Value,
        service: &mut ExtensionService,
    ) -> Result<Value, RpcError> {
        let source: SkillSource = parse(params)?;
        self.create_skill_preview(source, None, service).await
    }

    pub(super) async fn skill_update_preview(
        &self,
        params: Value,
        service: &mut ExtensionService,
    ) -> Result<Value, RpcError> {
        let request: UpdatePreviewParams = parse(params)?;
        let (_, library) = self.extension_library()?;
        let skill = library
            .library
            .iter()
            .find(|s| s.info.name == request.name)
            .ok_or_else(|| invalid_params("skill is not in the Lazoo library"))?;
        self.create_skill_preview(skill.source.clone(), Some(request.name), service)
            .await
    }

    async fn create_skill_preview(
        &self,
        source: SkillSource,
        update_name: Option<String>,
        service: &mut ExtensionService,
    ) -> Result<Value, RpcError> {
        let header = self.preview_header(service)?;
        let staging = self
            .extension_root()
            .join("staging")
            .join(header.preview_id.to_string());
        protect_dir(&staging).map_err(extension_error)?;
        lazoo_runtime::atomic_write(
            &staging.join("preview.json"),
            &serde_json::to_vec(&header).map_err(|_| extension_error(ExtensionError::Format))?,
        )
        .map_err(|e| extension_error(e.into()))?;
        let (skills, resolved_revision) = match stage_source(&source, &staging).await {
            Ok(result) => result,
            Err(error) => {
                // Failed imports must not evade the active-preview disk quota.
                if check_ancestors(&staging).is_ok() {
                    let _ = fs::remove_dir_all(&staging);
                }
                return Err(extension_error(error));
            }
        };
        let (_, library) = self.extension_library()?;
        let (inventory, _) = super::inventory::inventory(&self.extension_homes()?, &library);
        let mut conflicts = Vec::new();
        for skill in &skills {
            if let Some(old) = library.library.iter().find(|s| s.info.name == skill.name) {
                conflicts.push(json!({"name":skill.name,"scope":"library","kind":if old.info.checksum==skill.checksum{"duplicate"}else{"conflict"},"replaceAllowed":true}));
            }
            for entry in inventory.iter().filter(|e| e["name"] == skill.name) {
                conflicts.push(json!({"name":skill.name,"scope":"harness","target":entry["target"],"path":entry["path"],"kind":if entry["checksum"]==skill.checksum{"duplicate"}else{"conflict"},"managed":entry["managed"],"replaceAllowed":entry["managed"]}));
            }
        }
        let preview = SkillPreview {
            header,
            source,
            skills,
            resolved_revision,
            conflicts,
            update_name,
            staging,
        };
        let result = to_value(&preview);
        service.skills.insert(preview.header.preview_id, preview);
        Ok(result)
    }

    pub(super) async fn skill_mutate(
        &self,
        method: &str,
        params: Value,
        service: &mut ExtensionService,
    ) -> Result<Value, RpcError> {
        let prepared = match method {
            "skill.install" | "skill.update" => {
                self.prepare_install(method, &parse(params)?, service)?
            }
            "skill.expose" => {
                let request: ExposeParams = parse(params)?;
                let (_, before) = self.check_extension_revision(request.expected_revision)?;
                let mut after = before.clone();
                self.add_exposures(&mut after, &request.name, &request.providers, request.mode)?;
                PreparedSkillMutation {
                    revision: request.expected_revision,
                    before,
                    after,
                    trees: Vec::new(),
                    preview_id: None,
                }
            }
            "skill.unexpose" => {
                let request: UnexposeParams = parse(params)?;
                let (_, before) = self.check_extension_revision(request.expected_revision)?;
                if request.providers.is_empty() {
                    return Err(invalid_params("providers must not be empty"));
                }
                let mut after = before.clone();
                after.exposures.retain(|e| {
                    e.name != request.name || !request.providers.contains(&e.target.provider)
                });
                PreparedSkillMutation {
                    revision: request.expected_revision,
                    before,
                    after,
                    trees: Vec::new(),
                    preview_id: None,
                }
            }
            "skill.remove" => {
                let request: RemoveParams = parse(params)?;
                let (_, before) = self.check_extension_revision(request.expected_revision)?;
                let old = before
                    .library
                    .iter()
                    .find(|s| s.info.name == request.name)
                    .ok_or_else(|| invalid_params("skill is not managed"))?;
                let mut after = before.clone();
                after.exposures.retain(|e| e.name != request.name);
                after.library.retain(|s| s.info.name != request.name);
                let trees = vec![(
                    self.library_path(&request.name),
                    Some(old.info.checksum.clone()),
                    None,
                )];
                PreparedSkillMutation {
                    revision: request.expected_revision,
                    before,
                    after,
                    trees,
                    preview_id: None,
                }
            }
            _ => return Err(invalid_params("unknown skill mutation")),
        };
        self.apply_skills(method, prepared, service).await
    }

    fn library_path(&self, name: &str) -> PathBuf {
        self.extension_root().join("library").join(name)
    }

    fn prepare_install(
        &self,
        method: &str,
        request: &InstallParams,
        service: &ExtensionService,
    ) -> Result<PreparedSkillMutation, RpcError> {
        let (_, before) = self.check_extension_revision(request.expected_revision)?;
        let preview = service
            .skills
            .get(&request.preview_id)
            .ok_or_else(|| RpcError {
                code: "LZ-EXTENSION-PREVIEW-EXPIRED".into(),
                message: "preview is missing, consumed or expired; preview again".into(),
                data: None,
            })?;
        if preview.header.revision != request.expected_revision {
            return Err(extension_error(ExtensionError::Drift(
                "preview was created at a different revision".into(),
            )));
        }
        if request.names.is_empty()
            || request.names.len() > 32
            || request.names.iter().collect::<BTreeSet<_>>().len() != request.names.len()
        {
            return Err(invalid_params("select 1..32 unique skill names"));
        }
        if method == "skill.update"
            && (preview
                .update_name
                .as_ref()
                .is_none_or(|n| request.names != [n.clone()])
                || !request.replace)
        {
            return Err(invalid_params(
                "update requires its update preview, the same single name, and replace=true",
            ));
        }
        let mut after = before.clone();
        let mut trees = Vec::new();
        for name in &request.names {
            let candidates: Vec<_> = preview
                .skills
                .iter()
                .enumerate()
                .filter(|(_, s)| &s.name == name)
                .collect();
            if candidates.len() != 1 {
                return Err(invalid_params(
                    "selected name is absent or ambiguous in the source",
                ));
            }
            let (index, info) = candidates[0];
            if !info.problems.is_empty() {
                return Err(invalid_params("selected skill has validation problems"));
            }
            let tree = read_tree(&preview.staging.join("skills").join(index.to_string()))
                .map_err(extension_error)?;
            if tree_checksum(&tree) != info.checksum {
                return Err(extension_error(ExtensionError::Drift(
                    "staged source changed".into(),
                )));
            }
            let old = after.library.iter().find(|s| s.info.name == *name).cloned();
            if old
                .as_ref()
                .is_some_and(|s| s.info.checksum != info.checksum)
                && !request.replace
            {
                return Err(RpcError {
                    code: "LZ-SKILL-CONFLICT".into(),
                    message: "same-name library content differs; explicit replace is required"
                        .into(),
                    data: None,
                });
            }
            let new = LibrarySkill {
                info: info.clone(),
                source: preview.source.clone(),
                resolved_revision: preview.resolved_revision.clone(),
                revision: old.as_ref().map_or(1, |s| s.revision + 1),
            };
            after.library.retain(|s| s.info.name != *name);
            after.library.push(new);
            for exposure in after.exposures.iter_mut().filter(|e| e.name == *name) {
                exposure.checksum.clone_from(&info.checksum);
            }
            trees.push((
                self.library_path(name),
                old.map(|s| s.info.checksum),
                Some(tree),
            ));
            self.add_exposures(&mut after, name, &request.providers, request.mode)?;
        }
        after.library.sort_by(|a, b| a.info.name.cmp(&b.info.name));
        Ok(PreparedSkillMutation {
            revision: request.expected_revision,
            before,
            after,
            trees,
            preview_id: Some(request.preview_id),
        })
    }

    fn add_exposures(
        &self,
        library: &mut SkillLibrary,
        name: &str,
        providers: &[ProviderKind],
        mode: ExposureMode,
    ) -> Result<(), RpcError> {
        let skill = library
            .library
            .iter()
            .find(|s| s.info.name == name)
            .ok_or_else(|| invalid_params("skill is not managed"))?
            .clone();
        let homes = self.extension_homes()?;
        let (inventory, _) = super::inventory::inventory(&homes, library);
        for home in homes
            .iter()
            .filter(|h| providers.contains(&h.target.provider))
        {
            let path = home.skill_target(name).map_err(extension_error)?;
            check_ancestors(
                path.parent()
                    .ok_or_else(|| invalid_params("missing skill parent"))?,
            )
            .map_err(extension_error)?;
            let existing = library.exposures.iter().find(|e| e.path == path).cloned();
            if existing.is_none() {
                // Shared roots can introduce a collision even when the dedicated destination is empty.
                if inventory.iter().any(|e| {
                    e["target"] == to_value(&home.target)
                        && e["name"] == name
                        && e["checksum"] != skill.info.checksum
                        && e["managed"] != true
                }) {
                    return Err(RpcError{code:"LZ-SKILL-UNMANAGED-CONFLICT".into(),message:"an unmanaged skill with different content is already visible to this target".into(),data:Some(json!({"target":home.target,"name":name}))});
                }
                if fs::symlink_metadata(&path).is_ok() {
                    return Err(RpcError{code:"LZ-SKILL-UNMANAGED-CONFLICT".into(),message:"destination exists and is not owned by Lazoo; it will not be overwritten, even if identical".into(),data:Some(json!({"path":path}))});
                }
            }
            library.exposures.retain(|e| e.path != path);
            library.exposures.push(SkillExposure {
                name: name.into(),
                target: home.target.clone(),
                path,
                mode,
                checksum: skill.info.checksum.clone(),
                library_path: self.library_path(name),
            });
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // Keep intent, filesystem transaction and ledger finalization together.
    async fn apply_skills(
        &self,
        method: &str,
        prepared: PreparedSkillMutation,
        service: &mut ExtensionService,
    ) -> Result<Value, RpcError> {
        let PreparedSkillMutation {
            revision,
            before,
            after,
            trees,
            preview_id,
        } = prepared;
        // Validate every touched owned exposure before the first filesystem mutation.
        let changed = |e: &SkillExposure, other: &SkillLibrary| {
            other
                .exposures
                .iter()
                .find(|v| v.path == e.path)
                .is_none_or(|v| v.checksum != e.checksum || v.mode != e.mode)
        };
        let removals: Vec<_> = before
            .exposures
            .iter()
            .filter(|e| changed(e, &after) || trees.iter().any(|(p, _, _)| p == &e.library_path))
            .collect();
        let additions: Vec<_> = after
            .exposures
            .iter()
            .filter(|e| changed(e, &before) || trees.iter().any(|(p, _, _)| p == &e.library_path))
            .collect();
        for e in &removals {
            verify_exposure(e).map_err(extension_error)?;
        }
        for (path, expected, _) in &trees {
            if let Some(hash) = expected {
                if tree_checksum(&read_tree(path).map_err(extension_error)?) != *hash {
                    return Err(extension_error(ExtensionError::Drift(
                        "library content was edited".into(),
                    )));
                }
            } else if path.exists() {
                return Err(extension_error(ExtensionError::Drift(
                    "unowned library destination".into(),
                )));
            }
        }
        let operation_id = Uuid::new_v4();
        let journal_dir = self
            .extension_root()
            .join("operations")
            .join(operation_id.to_string());
        protect_dir(&journal_dir).map_err(extension_error)?;
        let mut transaction =
            SkillTransaction::new(journal_dir.join("journal.json")).map_err(extension_error)?;
        self.store.begin_extension_operation(revision,&json!({"operationId":operation_id,"kind":"skill","method":method,"journal":journal_dir.join("journal.json")})).map_err(store_error)?;
        self.publish_events()?;
        let applied = async {
            // Unlink first so backup/library replacement never leaves a live junction pointing at removed bytes.
            for e in removals {
                verify_exposure(e)?;
                transaction
                    .replace(
                        &e.path,
                        Some(if e.mode == ExposureMode::Link {
                            "link"
                        } else {
                            &e.checksum
                        }),
                        None,
                        None,
                        (e.mode == ExposureMode::Link).then_some(e.library_path.as_path()),
                    )
                    .await?;
            }
            for (path, expected, tree) in &trees {
                transaction
                    .replace(path, expected.as_deref(), tree.as_ref(), None, None)
                    .await?;
            }
            for e in additions {
                match e.mode {
                    ExposureMode::Copy => {
                        let tree = read_tree(&e.library_path)?;
                        transaction
                            .replace(&e.path, None, Some(&tree), None, None)
                            .await?;
                    }
                    ExposureMode::Link => {
                        transaction
                            .replace(&e.path, None, None, Some(&e.library_path), None)
                            .await?;
                    }
                }
                verify_exposure(e)?;
            }
            transaction.commit()
        }
        .await;
        let (state, status, error, recovery_needed) = match applied {
            Ok(()) => (to_value(&after), "applied", None, false),
            Err(error) => {
                let rollback = transaction.rollback();
                (
                    to_value(&before),
                    if rollback.is_ok() {
                        "rolled-back"
                    } else {
                        "recovery-needed"
                    },
                    Some(extension_error(error)),
                    rollback.is_err(),
                )
            }
        };
        let outcome = json!({"operationId":operation_id,"kind":"skill","method":method,"status":status,"error":error,"backupDirectory":journal_dir,"requiresProviderRestart":true});
        let revision = self
            .store
            .finish_extension_operation(revision, &state, &outcome, recovery_needed)
            .map_err(store_error)?;
        self.publish_events()?;
        if let Some(id) = preview_id
            && let Some(preview) = service.skills.remove(&id)
        {
            check_ancestors(&preview.staging).map_err(extension_error)?;
            let _ = fs::remove_dir_all(preview.staging);
        }
        Ok(
            json!({"revision":revision,"outcome":outcome,"library":state["library"],"exposures":state["exposures"]}),
        )
    }
}
