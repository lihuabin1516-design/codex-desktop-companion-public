use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::discovery::{external_command_path, resolve_ccpanes};
use crate::hashing::{hash_bytes, hash_file, hash_path};
use crate::logging::{product_home, unix_ms};

pub const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionConfig {
    pub schema_version: u32,
    pub ccpanes_executable: PathBuf,
    pub source: String,
    pub generated_unix_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEvidence {
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationReport {
    pub schema_version: u32,
    pub applied: bool,
    pub source: String,
    pub candidate_path_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_path: Option<String>,
    pub config_path_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    pub before: FileEvidence,
    pub after: FileEvidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_path_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
}

#[derive(Debug, Default)]
pub struct MigrationRequest {
    pub explicit_target: Option<PathBuf>,
    pub legacy_config: Option<PathBuf>,
    pub target_config: Option<PathBuf>,
    pub apply: bool,
    pub show_paths: bool,
}

pub fn default_config_path() -> PathBuf {
    product_home().join("config.json")
}

pub fn load_default_config() -> Result<Option<CompanionConfig>> {
    load_config(&default_config_path())
}

pub fn load_config(path: &Path) -> Result<Option<CompanionConfig>> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path).with_context(|| format!("read config {}", path.display()))?;
    let config: CompanionConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse config {}", path.display()))?;
    if config.schema_version != CONFIG_SCHEMA_VERSION {
        anyhow::bail!("unsupported_config_schema_{}", config.schema_version);
    }
    if !config.ccpanes_executable.is_file() {
        anyhow::bail!("configured_ccpanes_executable_not_found");
    }
    Ok(Some(config))
}

pub fn configured_ccpanes_path() -> Option<PathBuf> {
    load_default_config()
        .ok()
        .flatten()
        .map(|config| config.ccpanes_executable)
}

pub fn migrate(request: MigrationRequest) -> Result<MigrationReport> {
    let config_path = request.target_config.unwrap_or_else(default_config_path);
    let (candidate, source) = migration_candidate(
        request.explicit_target.as_deref(),
        request.legacy_config.as_deref(),
    )?;
    if !candidate.is_file() {
        anyhow::bail!("migration_candidate_is_not_file");
    }
    let candidate = external_command_path(
        &fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.to_path_buf()),
    );
    let before = file_evidence(&config_path)?;
    let mut after = before.clone();
    let mut backup_path = None;

    if request.apply {
        let parent = config_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("config_path_has_no_parent"))?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create config directory {}", parent.display()))?;

        if before.exists {
            let current_hash = hash_file(&config_path)?;
            if before.sha256.as_deref() != Some(current_hash.as_str()) {
                anyhow::bail!("config_changed_during_migration");
            }
            let backup_dir = parent.join("backups");
            fs::create_dir_all(&backup_dir)
                .with_context(|| format!("create backup directory {}", backup_dir.display()))?;
            let backup = backup_dir.join(format!("config-before-{}.json", unix_ms()));
            fs::copy(&config_path, &backup)
                .with_context(|| format!("backup config to {}", backup.display()))?;
            backup_path = Some(backup);
        }

        let config = CompanionConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            ccpanes_executable: candidate.clone(),
            source: source.clone(),
            generated_unix_ms: unix_ms(),
        };
        let bytes = serde_json::to_vec_pretty(&config)?;
        atomic_write(&config_path, &bytes)?;
        after = file_evidence(&config_path)?;
    }

    Ok(MigrationReport {
        schema_version: 1,
        applied: request.apply,
        source,
        candidate_path_hash: hash_path(&candidate),
        candidate_path: request.show_paths.then(|| candidate.display().to_string()),
        config_path_hash: hash_path(&config_path),
        config_path: request
            .show_paths
            .then(|| config_path.display().to_string()),
        before,
        after,
        backup_path_hash: backup_path.as_ref().map(|path| hash_path(path)),
        backup_path: request
            .show_paths
            .then(|| backup_path.as_ref().map(|path| path.display().to_string()))
            .flatten(),
    })
}

fn migration_candidate(
    explicit: Option<&Path>,
    legacy_config: Option<&Path>,
) -> Result<(PathBuf, String)> {
    if let Some(path) = explicit {
        return Ok((path.to_path_buf(), "explicit".into()));
    }
    if let Some(path) = legacy_config {
        return Ok((read_legacy_target(path)?, "legacy-config".into()));
    }
    if let Some(path) = env::var_os("LIVEAGENT_CCPANES_EXE").filter(|value| !value.is_empty()) {
        return Ok((PathBuf::from(path), "legacy-environment".into()));
    }
    let resolved = resolve_ccpanes(None)?;
    Ok((resolved.path, format!("discovery:{}", resolved.source)))
}

fn read_legacy_target(path: &Path) -> Result<PathBuf> {
    let bytes = fs::read(path).with_context(|| format!("read legacy config {}", path.display()))?;
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
        for key in [
            "ccpanesExecutable",
            "ccpanesExe",
            "officialCcpanesExecutable",
            "target",
        ] {
            if let Some(path) = value.get(key).and_then(|value| value.as_str()) {
                return Ok(PathBuf::from(path));
            }
        }
    }
    let text = String::from_utf8(bytes).context("legacy config is not UTF-8 or JSON")?;
    let value = text.trim().trim_matches('"');
    if value.is_empty() {
        anyhow::bail!("legacy_config_has_no_ccpanes_path");
    }
    Ok(PathBuf::from(value))
}

fn file_evidence(path: &Path) -> Result<FileEvidence> {
    if !path.is_file() {
        return Ok(FileEvidence {
            exists: false,
            bytes: None,
            sha256: None,
        });
    }
    let bytes = fs::read(path).with_context(|| format!("read evidence {}", path.display()))?;
    Ok(FileEvidence {
        exists: true,
        bytes: Some(bytes.len() as u64),
        sha256: Some(hash_bytes(&bytes)),
    })
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("config_path_has_no_parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("config_path_has_no_filename"))?
        .to_string_lossy();
    let temporary = parent.join(format!(".{name}.tmp-{}", unix_ms()));
    fs::write(&temporary, bytes)
        .with_context(|| format!("write temporary config {}", temporary.display()))?;
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("replace config {}", path.display()))?;
    }
    fs::rename(&temporary, path).with_context(|| format!("publish config {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_does_not_write_configuration() {
        let root = tempfile::tempdir().expect("tempdir");
        let executable = root.path().join("cc-panes.exe");
        fs::write(&executable, b"fixture").expect("fixture");
        let config = root.path().join("config.json");
        let report = migrate(MigrationRequest {
            explicit_target: Some(executable),
            target_config: Some(config.clone()),
            apply: false,
            show_paths: false,
            legacy_config: None,
        })
        .expect("dry run");
        assert!(!report.applied);
        assert!(!config.exists());
        assert!(!report.before.exists);
        assert!(!report.after.exists);
    }

    #[test]
    fn apply_backs_up_existing_configuration_and_loads_new_target() {
        let root = tempfile::tempdir().expect("tempdir");
        let old = root.path().join("old-cc-panes.exe");
        let new = root.path().join("cc-panes.exe");
        fs::write(&old, b"old").expect("old fixture");
        fs::write(&new, b"new").expect("new fixture");
        let config_path = root.path().join("config.json");
        let existing = CompanionConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            ccpanes_executable: old,
            source: "old".into(),
            generated_unix_ms: 1,
        };
        fs::write(
            &config_path,
            serde_json::to_vec_pretty(&existing).expect("serialize"),
        )
        .expect("existing config");

        let report = migrate(MigrationRequest {
            explicit_target: Some(new.clone()),
            target_config: Some(config_path.clone()),
            apply: true,
            show_paths: true,
            legacy_config: None,
        })
        .expect("apply");
        assert!(report.applied);
        assert!(report.before.exists);
        assert!(report.after.exists);
        assert!(report.backup_path.is_some());
        let loaded = load_config(&config_path)
            .expect("load")
            .expect("config present");
        let expected = external_command_path(&fs::canonicalize(&new).expect("canonical target"));
        assert_eq!(loaded.ccpanes_executable, expected);
    }

    #[test]
    fn legacy_json_keys_are_supported() {
        let root = tempfile::tempdir().expect("tempdir");
        let executable = root.path().join("cc-panes.exe");
        fs::write(&executable, b"fixture").expect("fixture");
        let legacy = root.path().join("legacy.json");
        fs::write(
            &legacy,
            serde_json::to_vec(&serde_json::json!({
                "officialCcpanesExecutable": executable
            }))
            .expect("serialize legacy"),
        )
        .expect("legacy");
        assert_eq!(
            read_legacy_target(&legacy).expect("legacy target"),
            executable
        );
    }
}
