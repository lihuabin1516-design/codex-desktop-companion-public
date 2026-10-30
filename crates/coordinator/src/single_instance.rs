use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use coordination_core::{CoordinationError, ErrorCode};
use serde::{Deserialize, Serialize};

use crate::RuntimeIdentity;

const SINGLE_INSTANCE_LOCK_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SingleInstanceMetadata {
    pub schema_version: u32,
    pub runtime_identity: String,
    pub pid: u32,
    pub token: String,
    pub created_at_ms: u64,
}

#[derive(Debug)]
pub struct SingleInstanceGuard {
    path: PathBuf,
    token: String,
}

impl SingleInstanceGuard {
    pub fn acquire(
        path: impl AsRef<Path>,
        identity: &RuntimeIdentity,
    ) -> Result<Self, CoordinationError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(map_io)?;
        }
        let token = format!("{}-{}", std::process::id(), now_ms());
        let metadata = SingleInstanceMetadata {
            schema_version: SINGLE_INSTANCE_LOCK_SCHEMA_VERSION,
            runtime_identity: identity.digest.clone(),
            pid: std::process::id(),
            token: token.clone(),
            created_at_ms: now_ms(),
        };
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| map_lock_open_error(error, &path, identity))?;
        let bytes = serde_json::to_vec_pretty(&metadata).map_err(map_json)?;
        file.write_all(&bytes).map_err(map_io)?;
        file.sync_all().map_err(map_io)?;
        Ok(Self { path, token })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        let owned = fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str::<SingleInstanceMetadata>(&text).ok())
            .map(|metadata| metadata.token == self.token)
            .unwrap_or(false);
        if owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn map_lock_open_error(
    error: std::io::Error,
    path: &Path,
    identity: &RuntimeIdentity,
) -> CoordinationError {
    if error.kind() == ErrorKind::AlreadyExists {
        let existing = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<SingleInstanceMetadata>(&text).ok())
            .map(|metadata| metadata.runtime_identity)
            .unwrap_or_else(|| "unknown".to_owned());
        CoordinationError::new(
            ErrorCode::CapacityExhausted,
            format!(
                "coordinator runtime instance already holds identity {existing}; requested {}",
                identity.digest
            ),
        )
        .with_retry_after_ms(1_000)
    } else {
        map_io(error)
    }
}

fn map_io(error: std::io::Error) -> CoordinationError {
    CoordinationError::new(
        ErrorCode::PersistenceUnavailable,
        format!("coordinator runtime lock I/O failed: {error}"),
    )
}

fn map_json(error: serde_json::Error) -> CoordinationError {
    CoordinationError::new(
        ErrorCode::InternalInvariantViolation,
        format!("coordinator runtime lock metadata failed: {error}"),
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
