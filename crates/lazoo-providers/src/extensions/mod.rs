//! Skill files and native plugin adapters. All writable paths are selected by the daemon.
mod files;
mod homes;
pub mod plugins;
mod process;
mod source;
pub use files::*;
pub use homes::*;
pub use process::run_command;
pub use source::*;

use crate::ProviderKind;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    #[error("invalid extension input: {0}")]
    Invalid(String),
    #[error("unsafe path or source: {0}")]
    Unsafe(String),
    #[error("extension limit exceeded: {0}")]
    Limit(&'static str),
    #[error("ownership or content changed: {0}")]
    Drift(String),
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    #[error("native command failed (output withheld)")]
    Command,
    #[error("native command deadline exceeded")]
    Timeout,
    #[error("native result format was not recognized")]
    Format,
    #[error("extension filesystem operation failed")]
    Io(#[from] std::io::Error),
    #[error("extension runtime operation failed")]
    Runtime(#[from] lazoo_runtime::RuntimeError),
    #[error("extension JSON was invalid")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionTarget {
    pub provider: ProviderKind,
    #[serde(default)]
    pub account_id: Option<uuid::Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub relative_path: PathBuf,
    pub file_count: usize,
    pub bytes: u64,
    pub checksum: String,
    pub problems: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExposureMode {
    Link,
    Copy,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySkill {
    #[serde(flatten)]
    pub info: SkillInfo,
    pub source: SkillSource,
    pub resolved_revision: Option<String>,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillExposure {
    pub name: String,
    pub target: ExtensionTarget,
    pub path: PathBuf,
    pub mode: ExposureMode,
    pub checksum: String,
    pub library_path: PathBuf,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillLibrary {
    pub library: Vec<LibrarySkill>,
    pub exposures: Vec<SkillExposure>,
}

#[must_use]
pub fn portable_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.split('-').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
        && ![
            "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
            "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
        ]
        .contains(&name)
}
