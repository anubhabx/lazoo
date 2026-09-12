mod atomic_file;
mod data_root;
mod installation;
mod notification;
mod process_job;
mod security;
mod single_instance;
mod task_worktree;

pub use atomic_file::atomic_write;
pub use data_root::DataRoot;
pub use installation::Installation;
pub use notification::NativeNotifier;
pub use process_job::{
    JobChild, JobCommand, JobObject, LaunchGate, session_host_path, wait_for_launch_gate,
};
pub use security::OwnerSecurity;
pub use single_instance::SingleInstance;
pub use task_worktree::{PreparedGitTaskWorktree, prepare_git_task_worktree};

use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("required environment variable is missing: {0}")]
    MissingEnvironment(&'static str),
    #[error("I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Windows operation failed: {0}")]
    Windows(#[from] windows::core::Error),
    #[error("runtime metadata is invalid: {0}")]
    InvalidMetadata(#[from] serde_json::Error),
    #[error("base64 data is invalid: {0}")]
    InvalidBase64(#[from] base64::DecodeError),
    #[error("Windows returned invalid UTF-16 text: {0}")]
    InvalidUtf16(#[from] std::string::FromUtf16Error),
    #[error("a Lazoo {role} instance is already running")]
    AlreadyRunning { role: String },
    #[error("protected data is empty")]
    EmptyProtectedData,
    #[error("Git operation failed while {operation}")]
    Git { operation: &'static str },
}

pub(crate) fn io_error(path: impl Into<PathBuf>, source: io::Error) -> RuntimeError {
    RuntimeError::Io {
        path: path.into(),
        source,
    }
}
