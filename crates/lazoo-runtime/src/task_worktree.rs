use std::{
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use uuid::Uuid;

use crate::{OwnerSecurity, RuntimeError, io_error};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedGitTaskWorktree {
    pub path: PathBuf,
    pub base_commit: String,
    pub branch_name: String,
}

pub fn prepare_git_task_worktree(
    repository: &Path,
    destination: &Path,
    task_id: Uuid,
) -> Result<PreparedGitTaskWorktree, RuntimeError> {
    let canonical_repository = repository
        .canonicalize()
        .map_err(|source| io_error(repository, source))?;
    let base_commit = git_output(
        &canonical_repository,
        &["rev-parse", "HEAD"],
        "reading base commit",
    )?;
    let branch_name = format!("lazoo/task-{task_id}");

    if destination.exists() {
        return verify_existing(destination, &branch_name, &base_commit);
    }
    let parent = destination.parent().ok_or(RuntimeError::Git {
        operation: "resolving worktree parent",
    })?;
    std::fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    OwnerSecurity::current()?.apply_to_path(parent)?;

    let branch_ref = format!("refs/heads/{branch_name}");
    let branch_exists = git_status(
        &canonical_repository,
        &["show-ref", "--verify", "--quiet", &branch_ref],
    )?
    .success();
    let destination_text = destination.to_string_lossy().into_owned();
    let status = if branch_exists {
        git_status(
            &canonical_repository,
            &["worktree", "add", &destination_text, &branch_name],
        )?
    } else {
        git_status(
            &canonical_repository,
            &[
                "worktree",
                "add",
                "-b",
                &branch_name,
                &destination_text,
                &base_commit,
            ],
        )?
    };
    if !status.success() {
        return Err(RuntimeError::Git {
            operation: "creating isolated task worktree",
        });
    }
    OwnerSecurity::current()?.apply_to_path(destination)?;
    verify_existing(destination, &branch_name, &base_commit)
}

fn verify_existing(
    destination: &Path,
    branch_name: &str,
    base_commit: &str,
) -> Result<PreparedGitTaskWorktree, RuntimeError> {
    let canonical_destination = destination
        .canonicalize()
        .map_err(|source| io_error(destination, source))?;
    let top_level = PathBuf::from(git_output(
        &canonical_destination,
        &["rev-parse", "--show-toplevel"],
        "verifying task worktree",
    )?)
    .canonicalize()
    .map_err(|source| io_error(destination, source))?;
    let actual_branch = git_output(
        &canonical_destination,
        &["symbolic-ref", "--short", "HEAD"],
        "verifying task branch",
    )?;
    let actual_head = git_output(
        &canonical_destination,
        &["rev-parse", "HEAD"],
        "verifying task base commit",
    )?;
    if top_level != canonical_destination
        || actual_branch != branch_name
        || actual_head != base_commit
    {
        return Err(RuntimeError::Git {
            operation: "validating existing task worktree ownership",
        });
    }
    Ok(PreparedGitTaskWorktree {
        path: destination.to_path_buf(),
        base_commit: base_commit.to_owned(),
        branch_name: branch_name.to_owned(),
    })
}

fn git_output(path: &Path, args: &[&str], operation: &'static str) -> Result<String, RuntimeError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|_| RuntimeError::Git { operation })?;
    if !output.status.success() {
        return Err(RuntimeError::Git { operation });
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() {
        return Err(RuntimeError::Git { operation });
    }
    Ok(value)
}

fn git_status(path: &Path, args: &[&str]) -> Result<ExitStatus, RuntimeError> {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .status()
        .map_err(|_| RuntimeError::Git {
            operation: "running Git",
        })
}
