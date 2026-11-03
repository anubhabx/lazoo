use super::super::App;
use lazoo_protocol::RpcError;
use lazoo_providers::extensions::{
    ExposureMode, HarnessHome, SkillLibrary, is_link, read_bounded, read_tree, skill_info,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

impl App {
    pub(super) fn skill_list(&self) -> Result<Value, RpcError> {
        let (state, library) = self.extension_library()?;
        let homes = self.extension_homes()?;
        let (inventory, problems) = inventory(&homes, &library);
        Ok(
            json!({"revision":state.revision,"library":library.library,"exposures":library.exposures,"inventory":inventory,"problems":problems,"pending":state.pending,"targets":homes.iter().map(|h|json!({"target":h.target,"skillRoots":h.skill_roots,"exposureDirectory":h.skill_roots.first(),"compatibility":if h.skill_roots.is_empty(){"Unsupported"}else{"Native"},"limitations":if h.target.provider==lazoo_providers::ProviderKind::Antigravity{vec!["global root inferred from documented config location; installed CLI has no read-only skill listing"]}else{vec![]}})).collect::<Vec<_>>() }),
        )
    }
}

pub(super) fn inventory(homes: &[HarnessHome], library: &SkillLibrary) -> (Vec<Value>, Vec<Value>) {
    let mut result = Vec::new();
    let mut problems = Vec::new();
    for home in homes {
        for root in &home.skill_roots {
            if !root.exists() {
                continue;
            }
            let mut visited = BTreeSet::new();
            let mut count = 0;
            scan(
                root,
                home,
                library,
                0,
                &mut visited,
                &mut count,
                &mut result,
                &mut problems,
            );
        }
    }
    for index in 0..result.len() {
        let duplicate = result
            .iter()
            .filter(|item| {
                item["name"] == result[index]["name"] && item["target"] == result[index]["target"]
            })
            .count()
            > 1;
        let also: Vec<_> = result
            .iter()
            .filter(|item| {
                item["canonicalPath"] == result[index]["canonicalPath"]
                    && item["provider"] != result[index]["provider"]
            })
            .map(|item| item["target"].clone())
            .collect();
        result[index]["duplicate"] = json!(duplicate);
        result[index]["alsoVisibleTo"] = json!(also);
    }
    (result, problems)
}

#[allow(clippy::too_many_arguments)]
fn scan(
    path: &Path,
    home: &HarnessHome,
    library: &SkillLibrary,
    depth: u32,
    visited: &mut BTreeSet<PathBuf>,
    count: &mut usize,
    result: &mut Vec<Value>,
    problems: &mut Vec<Value>,
) {
    *count += 1;
    if *count > 4096 || depth > 6 {
        problems.push(json!({"target":home.target,"path":path,"reason":"inventory scan limit"}));
        return;
    }
    let scanned = (|| -> Result<(), String> {
        let canonical = fs::canonicalize(path).map_err(|_| "unreadable directory")?;
        if !visited.insert(canonical.clone()) {
            return Ok(());
        }
        if path.join("SKILL.md").is_file() {
            // Native skills can contain linked resources. Still show their metadata
            // when a complete, ownership-safe content hash cannot be computed.
            let (tree, content_problem) = match read_tree(&canonical) {
                Ok(tree) => (tree, None),
                Err(error) => {
                    let metadata = read_bounded(&canonical.join("SKILL.md"), 8 * 1024 * 1024)
                        .map_err(|e| e.to_string())?;
                    (
                        [(PathBuf::from("SKILL.md"), metadata)]
                            .into_iter()
                            .collect(),
                        Some(error.to_string()),
                    )
                }
            };
            let mut info = skill_info(PathBuf::new(), &tree);
            let complete = content_problem.is_none();
            if let Some(problem) = content_problem {
                info.problems.push(problem);
            }
            let managed = library.exposures.iter().find(|e| e.path == path);
            let link = fs::symlink_metadata(path).is_ok_and(|m| is_link(&m));
            let kind = if link {
                "link"
            } else if managed.is_some_and(|e| e.mode == ExposureMode::Copy) {
                "copy"
            } else {
                "real-dir"
            };
            result.push(json!({"provider":home.target.provider,"target":home.target,"name":info.name,"description":info.description,"path":path,"canonicalPath":canonical,"kind":kind,"managed":managed.is_some(),"checksum":complete.then_some(info.checksum),"fileCount":complete.then_some(info.file_count),"bytes":complete.then_some(info.bytes),"problems":info.problems,"enabled":skill_enabled(home,path)}));
            return Ok(());
        }
        for entry in fs::read_dir(path).map_err(|_| "cannot list skill directory")? {
            let entry = entry.map_err(|_| "cannot read skill directory entry")?;
            if entry.file_name().to_string_lossy().starts_with(".lazoo-") {
                continue;
            }
            if entry.path().is_dir() {
                scan(
                    &entry.path(),
                    home,
                    library,
                    depth + 1,
                    visited,
                    count,
                    result,
                    problems,
                );
            }
            if *count > 4096 {
                break;
            }
        }
        Ok(())
    })();
    if let Err(reason) = scanned {
        problems.push(json!({"target":home.target,"path":path,"reason":reason}));
    }
}

fn skill_enabled(home: &HarnessHome, path: &Path) -> Option<bool> {
    if home.target.provider != lazoo_providers::ProviderKind::Codex {
        return None;
    }
    let config =
        String::from_utf8(read_bounded(&home.config.join("config.toml"), 1024 * 1024).ok()?)
            .ok()?;
    // Codex's own config remains authoritative; avoid guessing when it cannot be read.
    let value: toml::Value = toml::from_str(&config).ok()?;
    if let Some(entries) = value
        .get("skills")
        .and_then(|s| s.get("config"))
        .and_then(toml::Value::as_array)
    {
        for entry in entries {
            if let Some(selected) = entry.get("path").and_then(toml::Value::as_str)
                && (Path::new(selected) == path || Path::new(selected) == path.join("SKILL.md"))
            {
                return entry.get("enabled").and_then(toml::Value::as_bool);
            }
        }
    }
    Some(true)
}
