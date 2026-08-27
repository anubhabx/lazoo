use std::fs;

use lazoo_runtime::{DataRoot, RuntimeError, atomic_write};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct KeeperState {
    pub exit_times_millis: Vec<u64>,
    pub consecutive_failures: u32,
    pub last_exit_code: Option<i32>,
    pub last_incident_id: Option<String>,
}

impl KeeperState {
    pub fn load(root: &DataRoot) -> Result<Self, RuntimeError> {
        let path = root.keeper_state_file();
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(&path).map_err(|source| RuntimeError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn save(&self, root: &DataRoot) -> Result<(), RuntimeError> {
        let bytes = serde_json::to_vec_pretty(self)?;
        atomic_write(&root.keeper_state_file(), &bytes)
    }
}
