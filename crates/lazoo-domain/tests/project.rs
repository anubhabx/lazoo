use std::{path::Path, process::Command};

use lazoo_domain::{ProjectError, inspect_git_repository, normalize_windows_identity};
use tempfile::tempdir;

#[test]
fn windows_project_identity_is_case_and_separator_insensitive() {
    assert_eq!(
        normalize_windows_identity(Path::new(r"C:\Users\Example\Repo")),
        "c:/users/example/repo"
    );
}

#[test]
fn non_git_directory_is_rejected_cleanly() {
    let directory = tempdir().unwrap();

    let error = inspect_git_repository(directory.path()).unwrap_err();

    assert!(matches!(error, ProjectError::NotGitRepository { .. }));
}

#[test]
fn git_repository_resolves_worktree_and_common_directory() {
    let directory = tempdir().unwrap();
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .arg(directory.path())
        .status()
        .unwrap();
    assert!(status.success());

    let inspection = inspect_git_repository(directory.path()).unwrap();

    assert_eq!(
        inspection.canonical_worktree,
        directory.path().canonicalize().unwrap()
    );
    assert!(inspection.canonical_git_common_dir.ends_with(".git"));
    assert_eq!(
        inspection.identity,
        normalize_windows_identity(&inspection.canonical_git_common_dir)
    );
}
