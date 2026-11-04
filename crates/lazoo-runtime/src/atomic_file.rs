use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use uuid::Uuid;
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

use crate::{RuntimeError, io_error};

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    let parent = path.parent().ok_or_else(|| {
        io_error(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent"),
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;

    let temporary = temporary_path(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| io_error(&temporary, error))?;
        file.write_all(bytes)
            .map_err(|error| io_error(&temporary, error))?;
        file.sync_all()
            .map_err(|error| io_error(&temporary, error))?;
        drop(file);

        let source = crate::security::win32_path(&temporary);
        let destination = crate::security::win32_path(path);
        unsafe {
            MoveFileExW(
                &source,
                &destination,
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(RuntimeError::Windows)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn temporary_path(path: &Path) -> PathBuf {
    let filename = path
        .file_name()
        .map_or_else(|| "lazoo".into(), |name| name.to_string_lossy());
    path.with_file_name(format!(".{filename}.{}.tmp", Uuid::new_v4()))
}
