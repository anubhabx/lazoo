mod project;

pub use project::{
    GitRepositoryInspection, ProjectError, inspect_git_repository, normalize_windows_identity,
};
