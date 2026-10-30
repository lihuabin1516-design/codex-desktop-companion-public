use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use coordination_core::{CoordinationError, DirtyPolicy, ErrorCode, RepositoryRef};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

pub const SYNTHETIC_REPOSITORY_CONTRACT_VERSION: &str = "coordination.synthetic-repository/v1beta1";
pub const SYNTHETIC_REPOSITORY_MARKER_FILE: &str = ".codex-paneforge-synthetic-repository.json";
const SYNTHETIC_REPOSITORY_KIND: &str = "codex-paneforge-phase7-synthetic-repository";
const GITIGNORE_FILE: &str = ".gitignore";
const BASELINE_FILE: &str = "baseline.md";
const TASK_FILE: &str = "tasks/phase7-readme.md";
const SYNTHETIC_REPOSITORY_GITIGNORE: &str = "\
# Official PaneForge synthetic canary runtime metadata.
/.paneforge/history/
/.paneforge/projects.csv
/CLAUDE.md
";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GitRepositorySnapshot {
    pub worktree_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    pub status_digest: String,
    #[serde(default)]
    pub dirty_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SyntheticRepositoryRegistration {
    pub contract_version: String,
    pub repository: RepositoryRef,
    pub snapshot: GitRepositorySnapshot,
    pub marker_sha256: String,
    pub recovery_guard: SyntheticRepositoryRecoveryGuard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SyntheticRepositoryRecoveryGuard {
    pub parent_dir: PathBuf,
    pub repository_dir: PathBuf,
    pub marker_file: PathBuf,
    pub marker_sha256: String,
    pub cleanup_policy: String,
}

pub fn prepare_synthetic_git_repository(
    parent_dir: impl AsRef<Path>,
    repository_name: &str,
) -> Result<SyntheticRepositoryRegistration, CoordinationError> {
    validate_synthetic_repository_name(repository_name)?;
    let parent_dir = canonical_existing_dir(parent_dir.as_ref(), "synthetic repository parent")?;
    let repository_dir = parent_dir.join(repository_name);

    if repository_dir.exists() {
        if !repository_dir.is_dir() {
            return Err(CoordinationError::invalid_request(
                "synthetic repository target exists but is not a directory",
            ));
        }
        if !directory_is_empty(&repository_dir)? {
            return Err(CoordinationError::invalid_request(
                "synthetic repository target must be absent or empty",
            ));
        }
    } else {
        fs::create_dir(&repository_dir).map_err(|error| {
            CoordinationError::new(
                ErrorCode::PersistenceUnavailable,
                format!("create synthetic repository directory failed: {error}"),
            )
        })?;
    }

    let repository_dir =
        canonical_child_path(&parent_dir, &repository_dir, "synthetic repository target")?;
    write_synthetic_repository_files(&repository_dir)?;
    initialize_git_repository(&repository_dir)?;
    let snapshot = inspect_git_worktree(&repository_dir)?;
    if !snapshot.dirty_paths.is_empty() || snapshot.head.is_none() {
        return Err(CoordinationError::new(
            ErrorCode::InternalInvariantViolation,
            "synthetic repository baseline did not converge to a clean commit",
        ));
    }

    let marker_file = repository_dir.join(SYNTHETIC_REPOSITORY_MARKER_FILE);
    let marker_sha256 = sha256_file_hex(&marker_file)?;
    let recovery_guard =
        verify_synthetic_repository_recovery_guard(&parent_dir, &repository_dir, &marker_sha256)?;
    let repository = RepositoryRef {
        path: repository_dir.to_string_lossy().into_owned(),
        expected_head: snapshot.head.clone(),
        dirty_policy: DirtyPolicy::Reject,
        workspace_name: None,
    };

    Ok(SyntheticRepositoryRegistration {
        contract_version: SYNTHETIC_REPOSITORY_CONTRACT_VERSION.to_owned(),
        repository,
        snapshot,
        marker_sha256,
        recovery_guard,
    })
}

pub fn inspect_git_worktree(
    path: impl AsRef<Path>,
) -> Result<GitRepositorySnapshot, CoordinationError> {
    let path = path.as_ref();
    if !path.is_dir() {
        return Err(CoordinationError::invalid_request(
            "repository path must be an existing directory",
        ));
    }
    let inside = run_git(
        path,
        &["rev-parse", "--is-inside-work-tree"],
        "inspect worktree",
    )?;
    if String::from_utf8_lossy(&inside).trim() != "true" {
        return Err(CoordinationError::invalid_request(
            "repository path is not a git worktree",
        ));
    }

    let top_level = run_git(
        path,
        &["rev-parse", "--show-toplevel"],
        "locate worktree root",
    )?;
    let worktree_root = canonical_child_or_same(path, String::from_utf8_lossy(&top_level).trim())?;
    let requested = fs::canonicalize(path).map_err(|error| {
        CoordinationError::invalid_request(format!("canonicalize repository path failed: {error}"))
    })?;
    if requested != worktree_root {
        return Err(CoordinationError::invalid_request(
            "repository path must point at the git worktree root",
        ));
    }

    let branch = run_git(path, &["rev-parse", "--abbrev-ref", "HEAD"], "read branch")
        .ok()
        .map(|value| String::from_utf8_lossy(&value).trim().to_owned())
        .filter(|value| !value.is_empty());
    let head = run_git(path, &["rev-parse", "--verify", "HEAD"], "read HEAD")
        .ok()
        .map(|value| String::from_utf8_lossy(&value).trim().to_owned())
        .filter(|value| !value.is_empty());
    let status = run_git(
        path,
        &[
            "-c",
            "core.quotepath=false",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ],
        "read status",
    )?;
    let dirty_paths = parse_porcelain_v1_z_paths(&status)?;
    Ok(GitRepositorySnapshot {
        worktree_root,
        branch,
        head,
        status_digest: sha256_hex(&status),
        dirty_paths,
    })
}

pub fn validate_repository_baseline(
    repository: &RepositoryRef,
) -> Result<GitRepositorySnapshot, CoordinationError> {
    let snapshot = inspect_git_worktree(&repository.path)?;
    if let Some(expected_head) = repository.expected_head.as_deref() {
        if snapshot.head.as_deref() != Some(expected_head) {
            return Err(CoordinationError::new(
                ErrorCode::RepositoryBaselineChanged,
                "repository HEAD changed from expected baseline",
            )
            .with_current_state("HEAD_MISMATCH")
            .retryable(false));
        }
    }
    if repository.dirty_policy == DirtyPolicy::Reject && !snapshot.dirty_paths.is_empty() {
        return Err(CoordinationError::new(
            ErrorCode::RepositoryBaselineChanged,
            "repository has undeclared dirty paths",
        )
        .with_current_state("DIRTY_WORKTREE")
        .retryable(false));
    }
    Ok(snapshot)
}

pub fn verify_synthetic_repository_recovery_guard(
    parent_dir: impl AsRef<Path>,
    repository_dir: impl AsRef<Path>,
    expected_marker_sha256: &str,
) -> Result<SyntheticRepositoryRecoveryGuard, CoordinationError> {
    let parent_dir = canonical_existing_dir(parent_dir.as_ref(), "synthetic repository parent")?;
    let repository_dir = canonical_child_path(
        &parent_dir,
        repository_dir.as_ref(),
        "synthetic repository recovery target",
    )?;
    let marker_file = repository_dir.join(SYNTHETIC_REPOSITORY_MARKER_FILE);
    let marker_sha256 = verify_synthetic_repository_marker(&repository_dir)?;
    if marker_sha256 != expected_marker_sha256 {
        return Err(CoordinationError::invalid_request(
            "synthetic repository marker digest does not match registration",
        ));
    }

    Ok(SyntheticRepositoryRecoveryGuard {
        parent_dir,
        repository_dir,
        marker_file,
        marker_sha256,
        cleanup_policy:
            "cleanup is permitted only after parent containment and marker digest match".to_owned(),
    })
}

pub fn verify_synthetic_repository_marker(
    repository_dir: impl AsRef<Path>,
) -> Result<String, CoordinationError> {
    let marker_file = repository_dir
        .as_ref()
        .join(SYNTHETIC_REPOSITORY_MARKER_FILE);
    if !marker_file.is_file() {
        return Err(CoordinationError::invalid_request(
            "synthetic repository is missing its marker file",
        ));
    }
    let marker = fs::read(&marker_file).map_err(|error| {
        CoordinationError::new(
            ErrorCode::PersistenceUnavailable,
            format!("read synthetic repository marker failed: {error}"),
        )
    })?;
    let marker_json: serde_json::Value = serde_json::from_slice(&marker).map_err(|error| {
        CoordinationError::invalid_request(format!(
            "synthetic repository marker JSON is invalid: {error}"
        ))
    })?;
    if marker_json
        .get("contractVersion")
        .and_then(serde_json::Value::as_str)
        != Some(SYNTHETIC_REPOSITORY_CONTRACT_VERSION)
        || marker_json
            .get("repositoryKind")
            .and_then(serde_json::Value::as_str)
            != Some(SYNTHETIC_REPOSITORY_KIND)
    {
        return Err(CoordinationError::invalid_request(
            "synthetic repository marker does not match the Phase 7 contract",
        ));
    }
    Ok(sha256_hex(&marker))
}

fn validate_synthetic_repository_name(name: &str) -> Result<(), CoordinationError> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(CoordinationError::invalid_request(
            "synthetic repository name must be 1..=64 ASCII letters, digits, dash, underscore, or dot",
        ))
    }
}

fn canonical_existing_dir(path: &Path, label: &'static str) -> Result<PathBuf, CoordinationError> {
    if !path.is_dir() {
        return Err(CoordinationError::invalid_request(format!(
            "{label} must be an existing directory"
        )));
    }
    fs::canonicalize(path).map_err(|error| {
        CoordinationError::invalid_request(format!("canonicalize {label} failed: {error}"))
    })
}

fn canonical_child_path(
    parent_dir: &Path,
    child_path: &Path,
    label: &'static str,
) -> Result<PathBuf, CoordinationError> {
    let canonical_child = fs::canonicalize(child_path).map_err(|error| {
        CoordinationError::invalid_request(format!("canonicalize {label} failed: {error}"))
    })?;
    if canonical_child == parent_dir || !canonical_child.starts_with(parent_dir) {
        return Err(CoordinationError::invalid_request(format!(
            "{label} must stay inside the synthetic repository parent"
        )));
    }
    Ok(canonical_child)
}

fn canonical_child_or_same(path: &Path, child_path: &str) -> Result<PathBuf, CoordinationError> {
    let canonical = fs::canonicalize(child_path).map_err(|error| {
        CoordinationError::invalid_request(format!(
            "canonicalize git worktree root failed: {error}"
        ))
    })?;
    let requested = fs::canonicalize(path).map_err(|error| {
        CoordinationError::invalid_request(format!("canonicalize requested path failed: {error}"))
    })?;
    if canonical == requested || requested.starts_with(&canonical) {
        Ok(canonical)
    } else {
        Err(CoordinationError::invalid_request(
            "git worktree root is outside the requested repository path",
        ))
    }
}

fn directory_is_empty(path: &Path) -> Result<bool, CoordinationError> {
    Ok(fs::read_dir(path)
        .map_err(|error| {
            CoordinationError::new(
                ErrorCode::PersistenceUnavailable,
                format!("read synthetic repository directory failed: {error}"),
            )
        })?
        .next()
        .is_none())
}

fn write_synthetic_repository_files(repository_dir: &Path) -> Result<(), CoordinationError> {
    fs::create_dir_all(repository_dir.join("tasks")).map_err(|error| {
        CoordinationError::new(
            ErrorCode::PersistenceUnavailable,
            format!("create synthetic repository tasks directory failed: {error}"),
        )
    })?;
    let marker = json!({
        "contractVersion": SYNTHETIC_REPOSITORY_CONTRACT_VERSION,
        "repositoryKind": SYNTHETIC_REPOSITORY_KIND,
        "sideEffectScope": "dedicated-synthetic-git-repository",
        "externalWorkerSideEffects": false,
        "cleanupPolicy": "delete only when parent containment and marker digest match"
    });
    write_file(
        &repository_dir.join(SYNTHETIC_REPOSITORY_MARKER_FILE),
        &format!("{marker}\n"),
    )?;
    write_file(
        &repository_dir.join(GITIGNORE_FILE),
        SYNTHETIC_REPOSITORY_GITIGNORE,
    )?;
    write_file(
        &repository_dir.join(BASELINE_FILE),
        "# Codex PaneForge Phase 7 synthetic repository\n\nbaseline fixture\n",
    )?;
    write_file(
        &repository_dir.join(TASK_FILE),
        "This repository is a dedicated synthetic canary fixture for Phase 7.\n",
    )?;
    Ok(())
}

fn write_file(path: &Path, contents: &str) -> Result<(), CoordinationError> {
    fs::write(path, contents).map_err(|error| {
        CoordinationError::new(
            ErrorCode::PersistenceUnavailable,
            format!("write synthetic repository file failed: {error}"),
        )
    })
}

fn initialize_git_repository(repository_dir: &Path) -> Result<(), CoordinationError> {
    run_git(
        repository_dir,
        &["init", "--quiet"],
        "initialize repository",
    )?;
    run_git(
        repository_dir,
        &["config", "core.autocrlf", "false"],
        "configure autocrlf",
    )?;
    run_git(
        repository_dir,
        &["config", "commit.gpgsign", "false"],
        "configure commit signing",
    )?;
    run_git(repository_dir, &["add", "."], "stage baseline")?;
    run_git(
        repository_dir,
        &[
            "-c",
            "user.name=Codex PaneForge Synthetic Fixture",
            "-c",
            "user.email=codex-paneforge-synthetic@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "synthetic baseline",
        ],
        "commit baseline",
    )?;
    Ok(())
}

fn run_git(
    path: &Path,
    arguments: &[&str],
    action: &'static str,
) -> Result<Vec<u8>, CoordinationError> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(path)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| {
            CoordinationError::new(
                ErrorCode::PersistenceUnavailable,
                format!("run git for {action} failed: {error}"),
            )
        })?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let message = compact_command_error(&output.stderr);
        Err(CoordinationError::new(
            ErrorCode::RepositoryBaselineChanged,
            format!("git {action} failed: {message}"),
        )
        .retryable(false))
    }
}

fn parse_porcelain_v1_z_paths(output: &[u8]) -> Result<Vec<String>, CoordinationError> {
    let records = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect::<Vec<_>>();
    let mut paths = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.len() < 4 || record[2] != b' ' {
            return Err(CoordinationError::new(
                ErrorCode::RepositoryBaselineChanged,
                "git status porcelain record is invalid",
            )
            .retryable(false));
        }
        let status = &record[..2];
        paths.push(normalize_repository_path(&String::from_utf8_lossy(
            &record[3..],
        )));
        if status.iter().any(|state| matches!(*state, b'R' | b'C')) {
            index += 1;
            if index >= records.len() {
                return Err(CoordinationError::new(
                    ErrorCode::RepositoryBaselineChanged,
                    "git status porcelain rename record is incomplete",
                )
                .retryable(false));
            }
            paths.push(normalize_repository_path(&String::from_utf8_lossy(
                records[index],
            )));
        }
        index += 1;
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn normalize_repository_path(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_owned()
}

fn sha256_file_hex(path: &Path) -> Result<String, CoordinationError> {
    fs::read(path)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| {
            CoordinationError::new(
                ErrorCode::PersistenceUnavailable,
                format!("read synthetic repository marker for digest failed: {error}"),
            )
        })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push(hex_digit(byte >> 4));
        output.push(hex_digit(byte & 0x0f));
    }
    output
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        10..=15 => char::from(b'a' + value - 10),
        _ => unreachable!("hex nibble"),
    }
}

fn compact_command_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        "no stderr".to_owned()
    } else {
        compact.chars().take(512).collect()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn phase7_prepares_dedicated_synthetic_git_repository_with_clean_baseline() {
        let parent = tempdir().unwrap();
        let registration =
            prepare_synthetic_git_repository(parent.path(), "codex-paneforge-phase7").unwrap();

        assert_eq!(
            registration.contract_version,
            SYNTHETIC_REPOSITORY_CONTRACT_VERSION
        );
        assert_eq!(registration.repository.dirty_policy, DirtyPolicy::Reject);
        assert_eq!(
            registration.repository.expected_head,
            registration.snapshot.head
        );
        assert!(registration.repository.expected_head.is_some());
        assert!(registration.snapshot.dirty_paths.is_empty());
        assert_eq!(registration.snapshot.status_digest.len(), 64);
        assert_eq!(registration.marker_sha256.len(), 64);
        assert!(registration
            .recovery_guard
            .repository_dir
            .starts_with(&registration.recovery_guard.parent_dir));

        let validated = validate_repository_baseline(&registration.repository).unwrap();
        assert_eq!(validated.head, registration.snapshot.head);
        assert!(validated.dirty_paths.is_empty());
    }

    #[test]
    fn phase7_synthetic_repository_refuses_existing_user_content() {
        let parent = tempdir().unwrap();
        let target = parent.path().join("occupied");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("user.txt"), "user data\n").unwrap();

        let error = prepare_synthetic_git_repository(parent.path(), "occupied").unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(target.join("user.txt").is_file());
        assert!(!target.join(".git").exists());
    }

    #[test]
    fn phase7_synthetic_repository_name_rejects_path_escape() {
        let parent = tempdir().unwrap();

        let error = prepare_synthetic_git_repository(parent.path(), "../escape").unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn phase7_repository_baseline_rejects_dirty_and_head_drift() {
        let parent = tempdir().unwrap();
        let registration =
            prepare_synthetic_git_repository(parent.path(), "baseline-check").unwrap();
        let repository_dir = PathBuf::from(&registration.repository.path);

        fs::write(repository_dir.join("untracked.txt"), "dirty\n").unwrap();
        let dirty = validate_repository_baseline(&registration.repository).unwrap_err();
        assert_eq!(dirty.code, ErrorCode::RepositoryBaselineChanged);
        assert_eq!(dirty.current_state.as_deref(), Some("DIRTY_WORKTREE"));

        run_git(&repository_dir, &["add", "."], "stage dirty fixture").unwrap();
        run_git(
            &repository_dir,
            &[
                "-c",
                "user.name=Codex PaneForge Synthetic Fixture",
                "-c",
                "user.email=codex-paneforge-synthetic@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "drift",
            ],
            "commit drift fixture",
        )
        .unwrap();

        let head_drift = validate_repository_baseline(&registration.repository).unwrap_err();
        assert_eq!(head_drift.code, ErrorCode::RepositoryBaselineChanged);
        assert_eq!(head_drift.current_state.as_deref(), Some("HEAD_MISMATCH"));
    }

    #[test]
    fn phase7_synthetic_repository_ignores_paneforge_history_runtime_metadata() {
        let parent = tempdir().unwrap();
        let registration =
            prepare_synthetic_git_repository(parent.path(), "paneforge-history-metadata").unwrap();
        let repository_dir = PathBuf::from(&registration.repository.path);
        let history_dir = repository_dir.join(".paneforge/history");
        fs::create_dir_all(&history_dir).unwrap();
        for name in ["history.db", "history.db-shm", "history.db-wal"] {
            fs::write(history_dir.join(name), b"synthetic runtime metadata").unwrap();
        }
        let blobs_dir = history_dir.join("blobs");
        fs::create_dir_all(&blobs_dir).unwrap();
        fs::write(
            blobs_dir.join("synthetic-output-digest"),
            b"synthetic terminal history blob",
        )
        .unwrap();

        let snapshot = validate_repository_baseline(&registration.repository).unwrap();

        assert!(snapshot.dirty_paths.is_empty());
        assert_eq!(snapshot.head, registration.snapshot.head);
    }

    #[test]
    fn phase7_synthetic_repository_ignores_paneforge_workspace_setup_metadata() {
        let parent = tempdir().unwrap();
        let registration =
            prepare_synthetic_git_repository(parent.path(), "paneforge-workspace-metadata").unwrap();
        let repository_dir = PathBuf::from(&registration.repository.path);
        let paneforge_dir = repository_dir.join(".paneforge");
        fs::create_dir_all(&paneforge_dir).unwrap();
        fs::write(
            paneforge_dir.join("projects.csv"),
            b"id,name,path\nsynthetic,synthetic,synthetic\n",
        )
        .unwrap();
        fs::write(
            repository_dir.join("CLAUDE.md"),
            b"# Synthetic PaneForge workspace notes\n",
        )
        .unwrap();

        let snapshot = validate_repository_baseline(&registration.repository).unwrap();

        assert!(snapshot.dirty_paths.is_empty());
        assert_eq!(snapshot.head, registration.snapshot.head);
    }

    #[test]
    fn phase7_recovery_guard_requires_marker_digest_and_parent_containment() {
        let parent = tempdir().unwrap();
        let registration = prepare_synthetic_git_repository(parent.path(), "guarded-repo").unwrap();

        let guard = verify_synthetic_repository_recovery_guard(
            parent.path(),
            &registration.recovery_guard.repository_dir,
            &registration.marker_sha256,
        )
        .unwrap();
        assert_eq!(guard.marker_sha256, registration.marker_sha256);

        let wrong_parent = tempdir().unwrap();
        let outside = verify_synthetic_repository_recovery_guard(
            wrong_parent.path(),
            &registration.recovery_guard.repository_dir,
            &registration.marker_sha256,
        )
        .unwrap_err();
        assert_eq!(outside.code, ErrorCode::InvalidRequest);

        let wrong_marker_sha256 = "0".repeat(64);
        let wrong_digest = verify_synthetic_repository_recovery_guard(
            parent.path(),
            &registration.recovery_guard.repository_dir,
            &wrong_marker_sha256,
        )
        .unwrap_err();
        assert_eq!(wrong_digest.code, ErrorCode::InvalidRequest);
    }
}
