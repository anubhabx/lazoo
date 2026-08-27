use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    thread,
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    DataRoot, RuntimeError, SingleInstance, atomic_write, io_error,
    security::{protect_for_current_user, unprotect_for_current_user},
};

#[derive(Clone, Debug)]
pub struct Installation {
    metadata: InstallationMetadata,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InstallationMetadata {
    installation_id: Uuid,
    pipe_name: String,
    protected_nonce: String,
}

impl Installation {
    pub fn load_or_create(root: &DataRoot) -> Result<Self, RuntimeError> {
        root.ensure()?;
        let lock_id = installation_lock_id(root);
        let path = root.installation_file();
        let _guard = match SingleInstance::acquire(lock_id, "installation-create") {
            Ok(guard) => guard,
            Err(RuntimeError::AlreadyRunning { .. }) => {
                for _ in 0..100 {
                    if path.exists() {
                        let bytes = fs::read(&path).map_err(|error| io_error(&path, error))?;
                        let metadata = serde_json::from_slice(&bytes)?;
                        return Ok(Self { metadata });
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                return Err(RuntimeError::AlreadyRunning {
                    role: "installation-create".to_owned(),
                });
            }
            Err(error) => return Err(error),
        };

        if path.exists() {
            let bytes = fs::read(&path).map_err(|error| io_error(&path, error))?;
            let metadata = serde_json::from_slice(&bytes)?;
            return Ok(Self { metadata });
        }

        let installation_id = Uuid::new_v4();
        let mut nonce = Vec::with_capacity(32);
        nonce.extend_from_slice(Uuid::new_v4().as_bytes());
        nonce.extend_from_slice(Uuid::new_v4().as_bytes());
        let protected_nonce = STANDARD.encode(protect_for_current_user(&nonce)?);
        let metadata = InstallationMetadata {
            installation_id,
            pipe_name: format!(r"\\.\pipe\lazoo-control-{}", installation_id.simple()),
            protected_nonce,
        };
        let bytes = serde_json::to_vec_pretty(&metadata)?;
        atomic_write(&path, &bytes)?;
        Ok(Self { metadata })
    }

    #[must_use]
    pub fn id(&self) -> Uuid {
        self.metadata.installation_id
    }

    #[must_use]
    pub fn pipe_name(&self) -> &str {
        &self.metadata.pipe_name
    }

    pub fn unprotect_nonce(&self) -> Result<Vec<u8>, RuntimeError> {
        let protected = STANDARD.decode(&self.metadata.protected_nonce)?;
        unprotect_for_current_user(&protected)
    }
}

fn installation_lock_id(root: &DataRoot) -> Uuid {
    let mut hasher = DefaultHasher::new();
    root.root()
        .to_string_lossy()
        .to_lowercase()
        .hash(&mut hasher);
    let high = u128::from(hasher.finish()) << 64;
    Uuid::from_u128(high | u128::from(hasher.finish()))
}
