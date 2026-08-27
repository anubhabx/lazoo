use std::{
    env,
    path::{Path, PathBuf},
};

use crate::{OwnerSecurity, RuntimeError, io_error};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataRoot {
    root: PathBuf,
}

impl DataRoot {
    pub fn resolve() -> Result<Self, RuntimeError> {
        if let Some(path) = env::var_os("LAZOO_DATA_ROOT") {
            return Ok(Self::from_path(path));
        }

        let local_app_data =
            env::var_os("LOCALAPPDATA").ok_or(RuntimeError::MissingEnvironment("LOCALAPPDATA"))?;
        Ok(Self::from_path(PathBuf::from(local_app_data).join("Lazoo")))
    }

    #[must_use]
    pub fn from_path(path: impl AsRef<Path>) -> Self {
        Self {
            root: path.as_ref().to_path_buf(),
        }
    }

    pub fn ensure(&self) -> Result<(), RuntimeError> {
        let directories = [
            self.state_dir(),
            self.runtime_dir(),
            self.spool_pending(),
            self.spool_reconciled(),
            self.daemon_logs(),
            self.keeper_logs(),
        ];
        for directory in &directories {
            std::fs::create_dir_all(directory).map_err(|error| io_error(directory, error))?;
        }
        let owner = OwnerSecurity::current()?;
        owner.apply_to_path(&self.root)?;
        for directory in &directories {
            owner.apply_to_path(directory)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn state_dir(&self) -> PathBuf {
        self.root.join("state")
    }

    #[must_use]
    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("runtime")
    }

    #[must_use]
    pub fn state_db(&self) -> PathBuf {
        self.state_dir().join("lazoo.db")
    }

    #[must_use]
    pub fn installation_file(&self) -> PathBuf {
        self.runtime_dir().join("install.json")
    }

    #[must_use]
    pub fn keeper_state_file(&self) -> PathBuf {
        self.runtime_dir().join("keeper-state.json")
    }

    #[must_use]
    pub fn spool_active(&self) -> PathBuf {
        self.root.join("spool").join("active.open")
    }

    #[must_use]
    pub fn spool_pending(&self) -> PathBuf {
        self.root.join("spool").join("pending")
    }

    #[must_use]
    pub fn spool_reconciled(&self) -> PathBuf {
        self.root.join("spool").join("reconciled")
    }

    #[must_use]
    pub fn daemon_logs(&self) -> PathBuf {
        self.root.join("logs").join("lazood")
    }

    #[must_use]
    pub fn keeper_logs(&self) -> PathBuf {
        self.root.join("logs").join("keeper")
    }
}
