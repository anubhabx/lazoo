use super::{
    ExtensionError, FileTree, MAX_BYTES, MAX_FILES, SkillInfo, check_ancestors, protect_dir,
    read_source_tree, run_command, safe_relative, skill_info, write_tree,
};
use lazoo_runtime::LaunchEnvironment;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillSource {
    pub source: String,
    #[serde(default = "auto")]
    pub source_kind: String,
    #[serde(default)]
    pub git_ref: Option<String>,
}
fn auto() -> String {
    "auto".into()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceAddress {
    Local(PathBuf),
    Git {
        url: String,
        reference: Option<String>,
        subpath: PathBuf,
        selected: Option<String>,
    },
}

#[allow(clippy::too_many_lines)] // Address forms share the same validation boundary.
pub fn normalize_source(source: &SkillSource) -> Result<SourceAddress, ExtensionError> {
    let text = source.source.trim();
    if text.is_empty()
        || text.len() > 2048
        || text.contains(['\n', '\r', '\0', '?', '#'])
        || text.starts_with('-')
    {
        return Err(ExtensionError::Invalid(
            "source is empty, too long, or contains unsafe characters".into(),
        ));
    }
    if !["auto", "local", "github", "skills-sh", "git"].contains(&source.source_kind.as_str()) {
        return Err(ExtensionError::Invalid("unknown sourceKind".into()));
    }
    if source.source_kind == "local" || Path::new(text).exists() || Path::new(text).is_absolute() {
        let path = std::path::absolute(text)?;
        check_ancestors(&path)?;
        return Ok(SourceAddress::Local(path));
    }
    let mut reference = source.git_ref.clone();
    if reference.as_ref().is_some_and(|r| {
        r.is_empty()
            || r.starts_with('-')
            || r.contains(['\n', '\r', ':', '\\'])
            || r.split('/').any(|s| s == "..")
    }) {
        return Err(ExtensionError::Invalid("unsafe git ref".into()));
    }
    let mut subpath = PathBuf::new();
    let mut selected = None;
    let url = if let Some(rest) = text.strip_prefix("https://skills.sh/") {
        let pieces: Vec<_> = rest.trim_end_matches('/').split('/').collect();
        if !(2..=3).contains(&pieces.len()) {
            return Err(ExtensionError::Invalid(
                "skills.sh source needs owner/repo[/skill]".into(),
            ));
        }
        if pieces.len() == 3 {
            selected = Some(pieces[2].to_owned());
        }
        format!("https://github.com/{}/{}.git", pieces[0], pieces[1])
    } else if let Some(rest) = text.strip_prefix("https://github.com/") {
        let pieces: Vec<_> = rest.trim_end_matches('/').split('/').collect();
        if pieces.len() < 2 {
            return Err(ExtensionError::Invalid(
                "GitHub source needs owner/repo".into(),
            ));
        }
        if pieces.len() > 2 {
            if pieces.len() < 4 || pieces[2] != "tree" {
                return Err(ExtensionError::Invalid(
                    "expected a GitHub /tree/<ref>/<path> URL".into(),
                ));
            }
            if let Some(explicit) = &reference {
                let tail = pieces[3..].join("/");
                if tail == *explicit {
                    subpath = PathBuf::new();
                } else if let Some(path) = tail.strip_prefix(&format!("{explicit}/")) {
                    subpath = PathBuf::from(path);
                } else {
                    subpath = pieces[4..].iter().collect();
                }
            } else {
                reference = Some(pieces[3].into());
                subpath = pieces[4..].iter().collect();
            }
        }
        format!(
            "https://github.com/{}/{}.git",
            pieces[0],
            pieces[1].trim_end_matches(".git")
        )
    } else if text.starts_with("https://") {
        let rest = text.trim_start_matches("https://");
        if rest.contains('@') || !rest.contains('/') || rest.contains('%') || rest.contains('\\') {
            return Err(ExtensionError::Invalid(
                "credential-free HTTPS git URL required".into(),
            ));
        }
        text.to_owned()
    } else {
        let (repo, skill) = text
            .split_once('@')
            .map_or((text, None), |(a, b)| (a, Some(b)));
        let pieces: Vec<_> = repo.split('/').collect();
        if pieces.len() != 2 {
            return Err(ExtensionError::Invalid(
                "source must be a local path, owner/repo, or HTTPS URL".into(),
            ));
        }
        selected = skill.map(str::to_owned);
        format!("https://github.com/{repo}.git")
    };
    let components = url.trim_start_matches("https://").split('/');
    if components
        .clone()
        .any(|p| p.is_empty() || p == ".." || p == "." || p.contains(['@', '%', '\\', ':']))
        || (!subpath.as_os_str().is_empty() && !safe_relative(&subpath))
    {
        return Err(ExtensionError::Unsafe(
            "URL traversal, credentials or escaped path".into(),
        ));
    }
    if reference.as_ref().is_some_and(|r| {
        r.is_empty()
            || r.starts_with('-')
            || r.contains(['\n', '\r', '\0', ':', '\\'])
            || r.split('/').any(|p| p == "..")
    }) {
        return Err(ExtensionError::Invalid("unsafe git ref".into()));
    }
    Ok(SourceAddress::Git {
        url,
        reference,
        subpath,
        selected,
    })
}

#[allow(clippy::too_many_lines)] // Keep staging and its bounded git lifetime together.
pub async fn stage_source(
    source: &SkillSource,
    staging: &Path,
) -> Result<(Vec<SkillInfo>, Option<String>), ExtensionError> {
    protect_dir(staging)?;
    let address = normalize_source(source)?;
    let (input, revision, selected) = match address {
        SourceAddress::Local(path) => (path, None, None),
        SourceAddress::Git {
            url,
            reference,
            subpath,
            selected,
        } => {
            let repo = staging.join("repository");
            let commit_ref = reference
                .as_ref()
                .filter(|r| r.len() == 40 && r.bytes().all(|b| b.is_ascii_hexdigit()))
                .cloned();
            let mut env = LaunchEnvironment::default();
            env.set("GIT_TERMINAL_PROMPT", "0");
            env.set("GCM_INTERACTIVE", "never");
            env.set("GIT_CONFIG_NOSYSTEM", "1");
            env.set("GIT_CONFIG_GLOBAL", "NUL");
            for key in [
                "GIT_CONFIG_COUNT",
                "GIT_CONFIG_PARAMETERS",
                "GIT_DIR",
                "GIT_WORK_TREE",
                "GIT_INDEX_FILE",
                "GIT_OBJECT_DIRECTORY",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                "GIT_SSH_COMMAND",
                "GIT_ASKPASS",
                "SSH_ASKPASS",
            ] {
                env.remove(key);
            }
            let mut args = vec![
                "-c".into(),
                "core.hooksPath=NUL".into(),
                "-c".into(),
                "protocol.file.allow=never".into(),
                "clone".into(),
                "--depth=1".into(),
                "--no-checkout".into(),
                "--no-recurse-submodules".into(),
            ];
            if let Some(reference) = reference.filter(|_| commit_ref.is_none()) {
                args.extend(["--branch".into(), reference]);
            }
            args.extend(["--".into(), url, repo.to_string_lossy().into_owned()]);
            git_bounded(&args, staging, &env, staging).await?;
            if let Some(commit) = &commit_ref {
                git_bounded(
                    &[
                        "fetch".into(),
                        "--depth=1".into(),
                        "origin".into(),
                        commit.clone(),
                    ],
                    &repo,
                    &env,
                    staging,
                )
                .await?;
            }
            let revision = git_bounded(
                &[
                    "rev-parse".into(),
                    if commit_ref.is_some() {
                        "FETCH_HEAD"
                    } else {
                        "HEAD"
                    }
                    .into(),
                ],
                &repo,
                &env,
                staging,
            )
            .await?;
            let revision = String::from_utf8_lossy(&revision).trim().to_owned();
            if revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(ExtensionError::Format);
            }
            let entries = git_bounded(
                &["ls-tree".into(), "-r".into(), revision.clone()],
                &repo,
                &env,
                staging,
            )
            .await?;
            if String::from_utf8_lossy(&entries)
                .lines()
                .any(|l| l.starts_with("120000") || l.starts_with("160000"))
            {
                return Err(ExtensionError::Unsafe(
                    "git source contains symlinks or submodules".into(),
                ));
            }
            git_bounded(
                &[
                    "-c".into(),
                    "core.hooksPath=NUL".into(),
                    "checkout".into(),
                    "--force".into(),
                    revision.clone(),
                    "--".into(),
                    ".".into(),
                ],
                &repo,
                &env,
                staging,
            )
            .await?;
            (repo.join(subpath), Some(revision), selected)
        }
    };
    let tree = read_source_tree(&input)?;
    let roots = discover_skill_roots(&tree)?;
    let mut skills = Vec::new();
    for relative in roots {
        let contents: FileTree = tree
            .iter()
            .filter_map(|(name, bytes)| {
                name.strip_prefix(&relative)
                    .ok()
                    .map(|p| (p.to_path_buf(), bytes.clone()))
            })
            .collect();
        let info = skill_info(relative.clone(), &contents);
        if selected.as_ref().is_some_and(|name| name != &info.name) {
            continue;
        }
        let destination = staging.join("skills").join(skills.len().to_string());
        write_tree(&destination, &contents)?;
        skills.push(info);
    }
    if skills.is_empty() {
        return Err(ExtensionError::Invalid(
            "no SKILL.md found within the discovery depth".into(),
        ));
    }
    Ok((skills, revision))
}

async fn git_bounded(
    args: &[String],
    cwd: &Path,
    env: &LaunchEnvironment,
    staging: &Path,
) -> Result<Vec<u8>, ExtensionError> {
    tokio::select! {
        result = run_command(Path::new("git.exe"),args,cwd,env,Duration::from_secs(60)) => { let output = result?; staging_usage(staging)?; Ok(output) },
        result = async { loop { tokio::time::sleep(Duration::from_millis(100)).await; staging_usage(staging)?; } #[allow(unreachable_code)] Ok::<(),ExtensionError>(()) } => { result?; Err(ExtensionError::Limit("git staging")) }
    }
}

fn staging_usage(root: &Path) -> Result<(), ExtensionError> {
    let mut todo = vec![root.to_path_buf()];
    let mut count = 0;
    let mut bytes = 0;
    while let Some(path) = todo.pop() {
        if !path.exists() {
            continue;
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            count += 1;
            bytes += meta.len();
            if is_unsafe(&meta) || count > MAX_FILES * 2 || bytes > MAX_BYTES * 2 {
                return Err(ExtensionError::Limit(
                    "git staging bytes/entries or reparse point",
                ));
            }
            if meta.is_dir() {
                todo.push(entry.path());
            }
        }
    }
    Ok(())
}
fn is_unsafe(meta: &fs::Metadata) -> bool {
    super::is_link(meta)
}

pub fn discover_skill_roots(tree: &FileTree) -> Result<Vec<PathBuf>, ExtensionError> {
    if tree.contains_key(Path::new("SKILL.md")) {
        return Ok(vec![PathBuf::new()]);
    }
    let mut roots = Vec::new();
    let mut preferred = vec![
        PathBuf::from("skills"),
        PathBuf::from("skills/.curated"),
        PathBuf::from("skills/.experimental"),
        PathBuf::from("skills/.system"),
    ];
    preferred.extend(
        [
            ".claude/skills",
            ".agents/skills",
            ".codex/skills",
            ".copilot/skills",
            ".opencode/skills",
            ".gemini/skills",
        ]
        .map(PathBuf::from),
    );
    for manifest in [
        ".claude-plugin/marketplace.json",
        ".claude-plugin/plugin.json",
    ] {
        if let Some(bytes) = tree.get(Path::new(manifest)) {
            let value: serde_json::Value = serde_json::from_slice(bytes)?;
            let mut skill_paths = Vec::new();
            if let Some(items) = value["skills"].as_array() {
                skill_paths.extend(
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned),
                );
            }
            if let Some(plugins) = value["plugins"].as_array() {
                for plugin in plugins {
                    if let Some(items) = plugin["skills"].as_array() {
                        let prefix = plugin["source"].as_str().unwrap_or("");
                        for item in items.iter().filter_map(serde_json::Value::as_str) {
                            skill_paths.push(if prefix.is_empty() {
                                item.to_owned()
                            } else {
                                format!("{prefix}/{}", item.strip_prefix("./").unwrap_or(item))
                            });
                        }
                    }
                }
            }
            for item in skill_paths {
                let path = PathBuf::from(item.strip_prefix("./").unwrap_or(&item));
                if !safe_relative(&path) {
                    return Err(ExtensionError::Unsafe(
                        "plugin manifest skill path traversal".into(),
                    ));
                }
                preferred.push(path);
            }
        }
    }
    preferred.push(PathBuf::new());
    let mut seen = BTreeSet::new();
    for prefix in preferred {
        let mut candidates: Vec<_> = tree
            .keys()
            .filter(|p| p.file_name().is_some_and(|n| n == "SKILL.md"))
            .filter_map(|p| p.parent())
            .filter(|p| {
                p.strip_prefix(&prefix)
                    .is_ok_and(|tail| tail.components().count() <= 3)
            })
            .collect();
        candidates.sort_by_key(|p| (p.components().count(), p.to_path_buf()));
        for path in candidates {
            if roots
                .iter()
                .any(|parent: &PathBuf| path.starts_with(parent))
            {
                continue;
            }
            if seen.insert(path.to_path_buf()) {
                roots.retain(|existing| !existing.starts_with(path));
                roots.push(path.to_path_buf());
            }
        }
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(s: &str) -> SkillSource {
        SkillSource {
            source: s.into(),
            source_kind: "auto".into(),
            git_ref: None,
        }
    }
    #[test]
    fn source_addresses_reject_credentials_and_traversal() {
        for input in [
            "https://x:secret@host/repo",
            "https://github.com/o/r/tree/main/../escape",
            "https://host/r?token=secret",
            "https://skills.sh/../repo",
        ] {
            assert!(normalize_source(&source(input)).is_err(), "{input}");
        }
        assert!(matches!(
            normalize_source(&source("https://skills.sh/owner/repo/name")).unwrap(),
            SourceAddress::Git {
                selected: Some(_),
                ..
            }
        ));
    }
    #[test]
    fn shallow_skill_shadows_nested_children() {
        let tree = [
            ("skills/outer/SKILL.md", vec![]),
            ("skills/outer/inner/SKILL.md", vec![]),
            ("skills/.curated/other/SKILL.md", vec![]),
        ]
        .into_iter()
        .map(|(p, b)| (PathBuf::from(p), b))
        .collect();
        let roots = discover_skill_roots(&tree).unwrap();
        assert_eq!(roots.len(), 2);
        assert!(!roots.contains(&PathBuf::from("skills/outer/inner")));
    }

    #[test]
    fn manifest_traversal_is_not_normalized_away() {
        for path in ["../outside", "./../outside", "/outside", "C:/outside"] {
            let tree = [(
                PathBuf::from(".claude-plugin/plugin.json"),
                serde_json::to_vec(&serde_json::json!({"skills":[path]})).unwrap(),
            )]
            .into_iter()
            .collect();
            assert!(discover_skill_roots(&tree).is_err(), "{path}");
        }
    }

    #[test]
    fn explicit_git_ref_disambiguates_slashes_in_tree_urls() {
        let mut input = source("https://github.com/owner/repo/tree/feature/branch/skills/name");
        input.git_ref = Some("feature/branch".into());
        assert!(
            matches!(normalize_source(&input).unwrap(), SourceAddress::Git { reference: Some(reference), subpath, .. } if reference == "feature/branch" && subpath == Path::new("skills/name"))
        );
    }
}
