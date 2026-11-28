//! Workflows document: one revision-checked JSON file under `<data root>/workflows/`.
//! The desktop only stores it; the UI owns the schema, the scheduler and the runs.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use lazoo_runtime::{DataRoot, OwnerSecurity, atomic_write};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use crate::client::DesktopFailure;

/// The largest document, serialized, that a save accepts.
const MAX_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;
const FILE_NAME: &str = "workflows.json";

/// Serialises load-check-write so two saves cannot interleave.
#[derive(Default)]
pub struct WorkflowsRuntime {
    lock: Mutex<()>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Stored {
    revision: u64,
    document: Value,
}

#[derive(Debug, Serialize)]
pub struct WorkflowsLoaded {
    revision: u64,
    document: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowsSaved {
    revision: u64,
}

fn failure(code: &str, message: &str) -> DesktopFailure {
    DesktopFailure::new(code, message)
}

fn runtime_failure(message: &str) -> DesktopFailure {
    failure("LZ-DESKTOP-RUNTIME", message)
}

fn workflows_dir() -> Result<PathBuf, DesktopFailure> {
    let root = DataRoot::resolve()
        .map_err(|_| runtime_failure("The Lazoo data directory could not be resolved."))?;
    let path = root.root().join("workflows");
    let unavailable = || runtime_failure("The Lazoo workflows directory could not be prepared.");
    fs::create_dir_all(&path).map_err(|_| unavailable())?;
    OwnerSecurity::current()
        .and_then(|owner| owner.apply_to_path(&path))
        .map_err(|_| unavailable())?;
    Ok(path)
}

/// A missing file is revision 0 with no document; an unreadable one is an error, never a reset.
fn load_at(dir: &Path) -> Result<(u64, Option<Value>), DesktopFailure> {
    let bytes = match fs::read(dir.join(FILE_NAME)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, None)),
        Err(_) => return Err(corrupt()),
    };
    let stored = serde_json::from_slice::<Stored>(&bytes).map_err(|_| corrupt())?;
    Ok((stored.revision, Some(stored.document)))
}

fn corrupt() -> DesktopFailure {
    failure(
        "LZ-WORKFLOWS-CORRUPT",
        "The workflows file could not be read. It was left untouched; repair or remove workflows.json to continue.",
    )
}

fn save_at(dir: &Path, document: Value, expected: u64) -> Result<u64, DesktopFailure> {
    if !document.is_object() {
        return Err(failure(
            "LZ-WORKFLOWS-INVALID",
            "The workflows document must be a JSON object.",
        ));
    }
    let (current, _) = load_at(dir)?;
    if current != expected {
        return Err(failure(
            "LZ-WORKFLOWS-CONFLICT",
            "The workflows changed elsewhere. Reload and apply your change again.",
        ));
    }
    let revision = current + 1;
    let bytes = serde_json::to_vec(&Stored { revision, document })
        .map_err(|_| runtime_failure("The workflows could not be encoded."))?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(failure(
            "LZ-WORKFLOWS-TOO-LARGE",
            "The workflows are larger than 2 MB. Remove old runs or workflows.",
        ));
    }
    atomic_write(&dir.join(FILE_NAME), &bytes)
        .map_err(|_| runtime_failure("The workflows could not be saved."))?;
    Ok(revision)
}

fn guard(runtime: &WorkflowsRuntime) -> std::sync::MutexGuard<'_, ()> {
    runtime
        .lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[tauri::command]
pub async fn workflows_load(
    runtime: State<'_, WorkflowsRuntime>,
) -> Result<WorkflowsLoaded, DesktopFailure> {
    let _guard = guard(&runtime);
    let (revision, document) = load_at(&workflows_dir()?)?;
    Ok(WorkflowsLoaded { revision, document })
}

#[tauri::command]
pub async fn workflows_save(
    runtime: State<'_, WorkflowsRuntime>,
    document: Value,
    revision: u64,
) -> Result<WorkflowsSaved, DesktopFailure> {
    let _guard = guard(&runtime);
    let revision = save_at(&workflows_dir()?, document, revision)?;
    Ok(WorkflowsSaved { revision })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("lazoo-workflows-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn code(result: Result<u64, DesktopFailure>) -> String {
        result.expect_err("must fail").code
    }

    #[test]
    fn missing_file_is_revision_zero_without_a_document() {
        let dir = TempDir::new();
        assert_eq!(load_at(&dir.0).expect("load"), (0, None));
    }

    #[test]
    fn save_then_load_round_trips_and_bumps_the_revision() {
        let dir = TempDir::new();
        let first = json!({ "version": 1, "workflows": [] });
        assert_eq!(save_at(&dir.0, first.clone(), 0).expect("save"), 1);
        assert_eq!(load_at(&dir.0).expect("load"), (1, Some(first)));
        let second = json!({ "version": 1, "workflows": [{ "id": "a" }] });
        assert_eq!(save_at(&dir.0, second.clone(), 1).expect("save"), 2);
        assert_eq!(load_at(&dir.0).expect("load"), (2, Some(second)));
    }

    #[test]
    fn stale_revision_is_a_conflict_and_changes_nothing() {
        let dir = TempDir::new();
        save_at(&dir.0, json!({ "a": 1 }), 0).expect("save");
        assert_eq!(
            code(save_at(&dir.0, json!({ "a": 2 }), 0)),
            "LZ-WORKFLOWS-CONFLICT"
        );
        assert_eq!(
            code(save_at(&dir.0, json!({ "a": 2 }), 5)),
            "LZ-WORKFLOWS-CONFLICT"
        );
        assert_eq!(load_at(&dir.0).expect("load"), (1, Some(json!({ "a": 1 }))));
    }

    #[test]
    fn oversized_documents_are_refused() {
        let dir = TempDir::new();
        let big = json!({ "blob": "x".repeat(MAX_DOCUMENT_BYTES) });
        assert_eq!(code(save_at(&dir.0, big, 0)), "LZ-WORKFLOWS-TOO-LARGE");
        assert_eq!(load_at(&dir.0).expect("load"), (0, None));
    }

    #[test]
    fn corrupt_file_is_reported_and_never_overwritten() {
        let dir = TempDir::new();
        fs::write(dir.0.join(FILE_NAME), b"{ not json").expect("write");
        assert_eq!(
            load_at(&dir.0).expect_err("corrupt").code,
            "LZ-WORKFLOWS-CORRUPT"
        );
        assert_eq!(
            code(save_at(&dir.0, json!({ "a": 1 }), 0)),
            "LZ-WORKFLOWS-CORRUPT"
        );
        assert_eq!(
            fs::read(dir.0.join(FILE_NAME)).expect("read"),
            b"{ not json"
        );
    }

    #[test]
    fn non_object_documents_are_refused() {
        let dir = TempDir::new();
        for document in [json!([1, 2]), json!("text"), json!(3), Value::Null] {
            assert_eq!(code(save_at(&dir.0, document, 0)), "LZ-WORKFLOWS-INVALID");
        }
    }
}
