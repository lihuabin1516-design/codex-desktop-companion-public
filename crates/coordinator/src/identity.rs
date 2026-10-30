use std::path::{Path, PathBuf};

use coordination_protocol::IPC_SCHEMA_VERSION;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeIdentity {
    pub digest: String,
    pub protocol_major: String,
}

impl RuntimeIdentity {
    pub fn new(
        data_dir: impl AsRef<Path>,
        database_path: impl AsRef<Path>,
        ipc_endpoint: impl AsRef<str>,
    ) -> Self {
        let protocol_major = ipc_protocol_major().to_owned();
        let data_dir = stable_path_text(data_dir.as_ref());
        let database_path = stable_path_text(database_path.as_ref());
        let endpoint = ipc_endpoint.as_ref();
        let digest = stable_digest(&[&data_dir, &database_path, endpoint, &protocol_major]);
        Self {
            digest: format!("coordinator-{digest}"),
            protocol_major,
        }
    }
}

pub fn ipc_protocol_major() -> &'static str {
    IPC_SCHEMA_VERSION
        .rsplit_once('/')
        .map(|(_, version)| version)
        .unwrap_or("v1beta1")
}

fn stable_path_text(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        PathBuf::from(path)
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| PathBuf::from(path))
    };
    absolute
        .canonicalize()
        .unwrap_or(absolute)
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn stable_digest(parts: &[&str]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for part in parts {
        for byte in part.as_bytes().iter().copied().chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_identity_changes_when_ipc_endpoint_changes() {
        let first = RuntimeIdentity::new("data", "data/coordination.db", "pipe-a");
        let second = RuntimeIdentity::new("data", "data/coordination.db", "pipe-b");
        assert_ne!(first.digest, second.digest);
        assert_eq!(first.protocol_major, "v1beta1");
    }
}
