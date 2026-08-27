use std::{
    path::{Path, PathBuf},
    process::Command,
};

use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitRepositoryInspection {
    pub canonical_worktree: PathBuf,
    pub canonical_git_common_dir: PathBuf,
    pub identity: String,
}

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("project path does not exist or cannot be resolved: {path}")]
    InvalidPath { path: PathBuf },
    #[error("path is not a Git repository: {path}")]
    NotGitRepository { path: PathBuf },
    #[error("Git returned an invalid path for repository: {path}")]
    InvalidGitOutput { path: PathBuf },
}

#[must_use]
pub fn normalize_windows_identity(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

pub fn inspect_git_repository(path: &Path) -> Result<GitRepositoryInspection, ProjectError> {
    let canonical_input = path.canonicalize().map_err(|_| ProjectError::InvalidPath {
        path: path.to_path_buf(),
    })?;

    let worktree_output = git_rev_parse(&canonical_input, "--show-toplevel")?;
    let canonical_worktree = PathBuf::from(worktree_output).canonicalize().map_err(|_| {
        ProjectError::InvalidGitOutput {
            path: canonical_input.clone(),
        }
    })?;

    let common_output = git_rev_parse(&canonical_input, "--git-common-dir")?;
    let common_path = PathBuf::from(common_output);
    let common_path = if common_path.is_absolute() {
        common_path
    } else {
        canonical_worktree.join(common_path)
    };
    let canonical_git_common_dir =
        common_path
            .canonicalize()
            .map_err(|_| ProjectError::InvalidGitOutput {
                path: canonical_input,
            })?;

    Ok(GitRepositoryInspection {
        identity: normalize_windows_identity(&canonical_git_common_dir),
        canonical_worktree,
        canonical_git_common_dir,
    })
}

fn git_rev_parse(path: &Path, argument: &str) -> Result<String, ProjectError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", argument])
        .output()
        .map_err(|_| ProjectError::NotGitRepository {
            path: path.to_path_buf(),
        })?;

    if !output.status.success() {
        return Err(ProjectError::NotGitRepository {
            path: path.to_path_buf(),
        });
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() {
        return Err(ProjectError::InvalidGitOutput {
            path: path.to_path_buf(),
        });
    }

    Ok(value)
}
