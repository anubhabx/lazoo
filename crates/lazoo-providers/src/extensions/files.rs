use super::{ExposureMode, ExtensionError, SkillExposure, SkillInfo, portable_name, run_command};
use lazoo_runtime::{LaunchEnvironment, OwnerSecurity, atomic_write};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt::Write,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::Duration,
};

pub const MAX_FILES: usize = 4096;
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
pub type FileTree = BTreeMap<PathBuf, Vec<u8>>;

pub fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, ExtensionError> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(ExtensionError::Limit("file bytes"));
    }
    Ok(bytes)
}

#[must_use]
pub fn is_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[must_use]
pub fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && !path.to_string_lossy().contains([':', '\0'])
}

pub fn check_ancestors(path: &Path) -> Result<(), ExtensionError> {
    if !path.is_absolute() || path.components().any(|p| matches!(p, Component::ParentDir)) {
        return Err(ExtensionError::Unsafe(
            "absolute path without traversal required".into(),
        ));
    }
    for parent in path.ancestors() {
        match fs::symlink_metadata(parent) {
            Ok(meta) if is_link(&meta) => {
                return Err(ExtensionError::Unsafe(
                    "reparse point in writable/source path".into(),
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

pub fn protect_dir(path: &Path) -> Result<(), ExtensionError> {
    check_ancestors(path)?;
    fs::create_dir_all(path)?;
    OwnerSecurity::current()?.apply_to_path(path)?;
    Ok(())
}

pub fn read_tree(root: &Path) -> Result<FileTree, ExtensionError> {
    read_tree_with_vcs(root, false)
}

pub fn read_source_tree(root: &Path) -> Result<FileTree, ExtensionError> {
    read_tree_with_vcs(root, true)
}

fn read_tree_with_vcs(root: &Path, skip_git: bool) -> Result<FileTree, ExtensionError> {
    check_ancestors(root)?;
    let mut result = FileTree::new();
    let mut bytes = 0;
    let mut entries = 0;
    walk(
        root,
        root,
        0,
        &mut bytes,
        &mut entries,
        &mut result,
        skip_git,
    )?;
    Ok(result)
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: u32,
    bytes: &mut u64,
    count: &mut usize,
    result: &mut FileTree,
    skip_git: bool,
) -> Result<(), ExtensionError> {
    if depth > 24 {
        return Err(ExtensionError::Limit("directory depth"));
    }
    let mut entries = fs::read_dir(dir)?
        .take(MAX_FILES + 1)
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    if entries.len() > MAX_FILES {
        return Err(ExtensionError::Limit("directory entries"));
    }
    for entry in entries {
        *count += 1;
        if *count > MAX_FILES {
            return Err(ExtensionError::Limit("source entries"));
        }
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| ExtensionError::Unsafe("source containment".into()))?;
        if !safe_relative(relative) {
            return Err(ExtensionError::Unsafe("source path traversal".into()));
        }
        let meta = fs::symlink_metadata(&path)?;
        if is_link(&meta) {
            return Err(ExtensionError::Unsafe(
                "source contains a symlink or junction".into(),
            ));
        }
        if skip_git && entry.file_name() == ".git" {
            continue;
        }
        if meta.is_dir() {
            walk(root, &path, depth + 1, bytes, count, result, skip_git)?;
        } else if meta.is_file() {
            if meta.len() > MAX_FILE_BYTES || result.len() >= MAX_FILES {
                return Err(ExtensionError::Limit("source file size/count"));
            }
            let mut contents = Vec::new();
            fs::File::open(&path)?
                .take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut contents)?;
            *bytes += contents.len() as u64;
            if contents.len() as u64 > MAX_FILE_BYTES || *bytes > MAX_BYTES {
                return Err(ExtensionError::Limit("source bytes"));
            }
            result.insert(relative.to_path_buf(), contents);
        } else {
            return Err(ExtensionError::Unsafe("special source file".into()));
        }
    }
    Ok(())
}

#[must_use]
pub fn tree_checksum(tree: &FileTree) -> String {
    let mut hash = Sha256::new();
    for (path, bytes) in tree {
        let name = path.to_string_lossy().replace('\\', "/");
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    hash.finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

pub fn write_tree(path: &Path, tree: &FileTree) -> Result<(), ExtensionError> {
    protect_dir(path)?;
    for (name, bytes) in tree {
        if !safe_relative(name) {
            return Err(ExtensionError::Unsafe("invalid staged filename".into()));
        }
        let dest = path.join(name);
        if let Some(parent) = dest.parent() {
            protect_dir(parent)?;
        }
        atomic_write(&dest, bytes)?;
        OwnerSecurity::current()?.apply_to_path(&dest)?;
    }
    Ok(())
}

pub fn skill_info(path: PathBuf, tree: &FileTree) -> SkillInfo {
    let mut problems = Vec::new();
    let text = tree
        .get(Path::new("SKILL.md"))
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or("");
    let lines: Vec<_> = text.trim_start_matches('\u{feff}').lines().collect();
    let mut values = BTreeMap::new();
    if lines.first() != Some(&"---") {
        problems.push("SKILL.md requires YAML frontmatter".into());
    } else if let Some(end) = lines.iter().skip(1).position(|l| *l == "---") {
        let front = &lines[1..=end];
        for (index, line) in front.iter().enumerate() {
            if line.starts_with(char::is_whitespace) {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            if !["name", "description"].contains(&key) {
                continue;
            }
            let scalar = value.trim();
            let parsed = if [">", ">-", ">+", "|", "|-", "|+"].contains(&scalar) {
                front[index + 1..]
                    .iter()
                    .take_while(|l| l.starts_with(char::is_whitespace) || l.is_empty())
                    .map(|l| l.trim())
                    .collect::<Vec<_>>()
                    .join(if scalar.starts_with('|') { "\n" } else { " " })
            } else if scalar.starts_with('"') {
                serde_json::from_str::<String>(scalar).unwrap_or_else(|_| {
                    problems.push(format!("{key}: unsupported quoted scalar"));
                    String::new()
                })
            } else if scalar.starts_with('\'') && scalar.ends_with('\'') && scalar.len() >= 2 {
                scalar[1..scalar.len() - 1].replace("''", "'")
            } else if scalar.starts_with(['[', '{', '&', '*', '!']) {
                problems.push(format!("{key}: scalar text required"));
                String::new()
            } else {
                scalar.split(" #").next().unwrap_or("").trim().to_owned()
            };
            if values.insert(key, parsed).is_some() {
                problems.push(format!("duplicate {key} in frontmatter"));
            }
        }
    } else {
        problems.push("unterminated YAML frontmatter".into());
    }
    let name = values.remove("name").unwrap_or_default();
    let description = values.remove("description").unwrap_or_default();
    if !portable_name(&name) {
        problems.push("name must be a portable lowercase kebab name of 1..64 bytes".into());
    }
    if description.is_empty() || description.chars().count() > 1024 {
        problems.push("description must have 1..1024 characters".into());
    }
    SkillInfo {
        name,
        description,
        relative_path: path,
        file_count: tree.len(),
        bytes: tree.values().map(|b| b.len() as u64).sum(),
        checksum: tree_checksum(tree),
        problems,
    }
}

pub fn verify_exposure(exposure: &SkillExposure) -> Result<(), ExtensionError> {
    check_ancestors(
        exposure
            .path
            .parent()
            .ok_or_else(|| ExtensionError::Unsafe("exposure parent".into()))?,
    )?;
    let meta = fs::symlink_metadata(&exposure.path)
        .map_err(|_| ExtensionError::Drift("managed exposure is missing".into()))?;
    let matches = match exposure.mode {
        ExposureMode::Link => {
            is_link(&meta)
                && fs::canonicalize(&exposure.path)? == fs::canonicalize(&exposure.library_path)?
        }
        ExposureMode::Copy => {
            !is_link(&meta) && tree_checksum(&read_tree(&exposure.path)?) == exposure.checksum
        }
    };
    if !matches {
        return Err(ExtensionError::Drift(
            "managed exposure was replaced or edited; refusing removal".into(),
        ));
    }
    Ok(())
}

pub async fn create_junction(path: &Path, target: &Path) -> Result<(), ExtensionError> {
    check_ancestors(path)?;
    check_ancestors(target)?;
    let mut env = LaunchEnvironment::default();
    env.set("LAZOO_JUNCTION_PATH", path);
    env.set("LAZOO_JUNCTION_TARGET", target);
    let shell = std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from("C:/Windows"), PathBuf::from)
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let args = ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:LAZOO_JUNCTION_PATH -Target $env:LAZOO_JUNCTION_TARGET | Out-Null"].map(str::to_owned);
    run_command(
        &shell,
        &args,
        path.parent().unwrap_or(target),
        &env,
        Duration::from_secs(10),
    )
    .await?;
    if fs::canonicalize(path)? != fs::canonicalize(target)? {
        return Err(ExtensionError::Drift("junction read-back failed".into()));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: PathBuf,
    pub before: Option<String>,
    pub after: Option<String>,
    pub backup: PathBuf,
    pub link_target: Option<PathBuf>,
}

/// Persist intent before each rename; backups stay on the target volume outside the skill root.
pub struct SkillTransaction {
    pub changes: Vec<FileChange>,
    journal: PathBuf,
}

impl SkillTransaction {
    pub fn new(journal: PathBuf) -> Result<Self, ExtensionError> {
        let result = Self {
            changes: Vec::new(),
            journal,
        };
        result.save()?;
        Ok(result)
    }
    fn save(&self) -> Result<(), ExtensionError> {
        atomic_write(&self.journal, &serde_json::to_vec(&self.changes)?)?;
        Ok(())
    }

    pub async fn replace(
        &mut self,
        path: &Path,
        expected: Option<&str>,
        tree: Option<&FileTree>,
        link_target: Option<&Path>,
        owned_link: Option<&Path>,
    ) -> Result<(), ExtensionError> {
        check_ancestors(
            path.parent()
                .ok_or_else(|| ExtensionError::Unsafe("missing parent".into()))?,
        )?;
        let existing = fs::symlink_metadata(path).ok();
        match (expected, existing.as_ref()) {
            (None, Some(_)) => {
                return Err(ExtensionError::Drift(
                    "unmanaged destination already exists".into(),
                ));
            }
            (Some("link"), Some(meta))
                if is_link(meta)
                    && owned_link.is_some_and(|target| {
                        fs::canonicalize(path).ok() == fs::canonicalize(target).ok()
                            && fs::canonicalize(target).is_ok()
                    }) => {}
            (Some(hash), Some(meta))
                if !is_link(meta) && tree_checksum(&read_tree(path)?) == hash => {}
            (Some(_), _) => {
                return Err(ExtensionError::Drift(
                    "destination changed before apply".into(),
                ));
            }
            _ => {}
        }
        fs::create_dir_all(path.parent().unwrap())?;
        let backup_root = path
            .parent()
            .and_then(Path::parent)
            .ok_or(ExtensionError::Format)?
            .join(format!(".lazoo-extension-backup-{}", uuid::Uuid::new_v4()));
        protect_dir(&backup_root)?;
        let backup = backup_root.join(format!(
            "{}-{}",
            chrono::Utc::now().timestamp_millis(),
            uuid::Uuid::new_v4()
        ));
        let after = tree
            .map(tree_checksum)
            .or_else(|| link_target.map(|_| "link".to_owned()));
        self.changes.push(FileChange {
            path: path.to_path_buf(),
            before: expected.map(str::to_owned),
            after,
            backup: backup.clone(),
            link_target: link_target.map(Path::to_path_buf),
        });
        self.save()?;
        if existing.is_some() {
            fs::rename(path, &backup)?;
        }
        if let Some(tree) = tree {
            let staged = path.with_file_name(format!(".lazoo-stage-{}", uuid::Uuid::new_v4()));
            write_tree(&staged, tree)?;
            fs::rename(&staged, path)?;
            if tree_checksum(&read_tree(path)?) != tree_checksum(tree) {
                return Err(ExtensionError::Drift("copy read-back mismatch".into()));
            }
        } else if let Some(target) = link_target {
            create_junction(path, target).await?;
        }
        Ok(())
    }

    pub fn rollback(&self) -> Result<(), ExtensionError> {
        for change in self.changes.iter().rev() {
            if fs::symlink_metadata(&change.backup).is_ok() || change.before.is_none() {
                remove_created(change)?;
                if change.before.is_some() {
                    fs::rename(&change.backup, &change.path)?;
                }
            }
        }
        Ok(())
    }

    pub fn commit(&self) -> Result<(), ExtensionError> {
        // Keep owner-protected backups outside every native discovery root for recovery.
        for (index, change) in self.changes.iter().enumerate() {
            // An update removes then re-adds an exposure. Verify its final state.
            if self.changes[index + 1..]
                .iter()
                .any(|later| later.path == change.path)
            {
                continue;
            }
            if let Some(hash) = &change.after {
                if hash == "link" {
                    if fs::canonicalize(&change.path)?
                        != fs::canonicalize(
                            change.link_target.as_ref().ok_or(ExtensionError::Format)?,
                        )?
                    {
                        return Err(ExtensionError::Drift(
                            "junction changed during commit".into(),
                        ));
                    }
                } else if tree_checksum(&read_tree(&change.path)?) != *hash {
                    return Err(ExtensionError::Drift(
                        "content changed during commit".into(),
                    ));
                }
            } else if fs::symlink_metadata(&change.path).is_ok() {
                return Err(ExtensionError::Drift("removed path reappeared".into()));
            }
        }
        Ok(())
    }
}

fn remove_created(change: &FileChange) -> Result<(), ExtensionError> {
    if fs::symlink_metadata(&change.path).is_err() {
        return Ok(());
    }
    check_ancestors(change.path.parent().ok_or(ExtensionError::Format)?)?;
    match change.after.as_deref() {
        Some("link")
            if fs::canonicalize(&change.path)?
                == fs::canonicalize(
                    change.link_target.as_ref().ok_or(ExtensionError::Format)?,
                )? =>
        {
            fs::remove_dir(&change.path)?;
        }
        Some(hash) if tree_checksum(&read_tree(&change.path)?) == hash => {
            fs::remove_dir_all(&change.path)?;
        }
        _ => {
            return Err(ExtensionError::Drift(
                "rollback refused concurrent filesystem change".into(),
            ));
        }
    }
    Ok(())
}
