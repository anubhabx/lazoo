use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

use lazoo_runtime::DataRoot;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

const MAX_RECORD_BYTES: usize = 16 * 1024;
const MAX_SEGMENT_BYTES: u64 = 1024 * 1024;
const MAX_PENDING_SEGMENTS: usize = 8;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmergencyIncident {
    pub incident_id: String,
    pub code: String,
    pub message: String,
    pub occurred_at: String,
}

#[derive(Debug, Error)]
pub enum SpoolError {
    #[error("emergency record is {actual} bytes; maximum is {maximum}")]
    RecordTooLarge { actual: usize, maximum: usize },
    #[error("all emergency spool segments are occupied")]
    Capacity,
    #[error("emergency spool I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("emergency record JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug)]
pub struct EmergencySpool {
    root: DataRoot,
}

impl EmergencySpool {
    #[must_use]
    pub fn new(root: DataRoot) -> Self {
        Self { root }
    }

    pub fn recover_stale(&self) -> Result<Option<PathBuf>, SpoolError> {
        let active = self.root.spool_active();
        if !active.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&active).map_err(|source| io_error(&active, source))?;
        let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n') else {
            return Ok(None);
        };
        let complete = &bytes[..=last_newline];
        let partial = &bytes[last_newline + 1..];
        if complete.is_empty() {
            return Ok(None);
        }
        self.ensure_capacity()?;
        let destination = self.segment_path();
        fs::write(&destination, complete).map_err(|source| io_error(&destination, source))?;
        fs::write(&active, partial).map_err(|source| io_error(&active, source))?;
        Ok(Some(destination))
    }

    pub fn append_and_seal(
        &self,
        incident: &EmergencyIncident,
    ) -> Result<Option<PathBuf>, SpoolError> {
        if self.contains_incident(&incident.incident_id)? {
            return Ok(None);
        }
        let mut record = serde_json::to_vec(incident)?;
        record.push(b'\n');
        if record.len() > MAX_RECORD_BYTES {
            return Err(SpoolError::RecordTooLarge {
                actual: record.len(),
                maximum: MAX_RECORD_BYTES,
            });
        }
        self.ensure_capacity()?;
        let active = self.root.spool_active();
        let active_len = fs::metadata(&active)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if active_len + u64::try_from(record.len()).unwrap_or(u64::MAX) > MAX_SEGMENT_BYTES {
            self.recover_stale()?;
            self.ensure_capacity()?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active)
            .map_err(|source| io_error(&active, source))?;
        file.write_all(&record)
            .map_err(|source| io_error(&active, source))?;
        file.sync_all()
            .map_err(|source| io_error(&active, source))?;
        drop(file);
        let destination = self.segment_path();
        fs::rename(&active, &destination).map_err(|source| io_error(&destination, source))?;
        Ok(Some(destination))
    }

    fn contains_incident(&self, incident_id: &str) -> Result<bool, SpoolError> {
        for entry in fs::read_dir(self.root.spool_pending())
            .map_err(|source| io_error(self.root.spool_pending(), source))?
        {
            let path = entry
                .map_err(|source| io_error(self.root.spool_pending(), source))?
                .path();
            let content = fs::read_to_string(&path).map_err(|source| io_error(&path, source))?;
            if content
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .any(|value| value["incident_id"] == incident_id)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn ensure_capacity(&self) -> Result<(), SpoolError> {
        let count = fs::read_dir(self.root.spool_pending())
            .map_err(|source| io_error(self.root.spool_pending(), source))?
            .count();
        if count >= MAX_PENDING_SEGMENTS {
            Err(SpoolError::Capacity)
        } else {
            Ok(())
        }
    }

    fn segment_path(&self) -> PathBuf {
        self.root
            .spool_pending()
            .join(format!("incident-{}.ndjson", Uuid::new_v4()))
    }
}

fn io_error(path: impl Into<PathBuf>, source: std::io::Error) -> SpoolError {
    SpoolError::Io {
        path: path.into(),
        source,
    }
}
