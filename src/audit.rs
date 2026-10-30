use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::hashing::hash_path;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildAuditReport {
    pub schema_version: u32,
    pub root_path_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_path: Option<String>,
    pub threshold_bytes: u64,
    pub scanned_target_directories: usize,
    pub findings: Vec<BuildFinding>,
    pub read_errors: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildFinding {
    pub path_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub bytes: u64,
    pub exceeds_threshold: bool,
}

pub fn audit_builds(
    root: &Path,
    threshold_bytes: u64,
    max_depth: usize,
    show_paths: bool,
) -> Result<BuildAuditReport> {
    if !root.is_dir() {
        anyhow::bail!("audit_root_is_not_directory");
    }
    let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut target_dirs = Vec::new();
    let mut read_errors = 0;
    find_target_dirs(&canonical, 0, max_depth, &mut target_dirs, &mut read_errors);
    let mut findings = Vec::with_capacity(target_dirs.len());
    for path in target_dirs {
        let mut errors = 0;
        let bytes = directory_size(&path, &mut errors)
            .with_context(|| format!("measure {}", path.display()))?;
        read_errors += errors;
        findings.push(BuildFinding {
            path_hash: hash_path(&path),
            path: show_paths.then(|| path.display().to_string()),
            bytes,
            exceeds_threshold: bytes >= threshold_bytes,
        });
    }
    findings.sort_by_key(|finding| std::cmp::Reverse(finding.bytes));
    Ok(BuildAuditReport {
        schema_version: 1,
        root_path_hash: hash_path(&canonical),
        root_path: show_paths.then(|| canonical.display().to_string()),
        threshold_bytes,
        scanned_target_directories: findings.len(),
        findings,
        read_errors,
    })
}

fn find_target_dirs(
    current: &Path,
    depth: usize,
    max_depth: usize,
    output: &mut Vec<PathBuf>,
    read_errors: &mut usize,
) {
    if depth > max_depth {
        return;
    }
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(_) => {
            *read_errors += 1;
            return;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else {
            *read_errors += 1;
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            *read_errors += 1;
            continue;
        };
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let path = entry.path();
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case("target")
        {
            output.push(path);
            continue;
        }
        find_target_dirs(&path, depth + 1, max_depth, output, read_errors);
    }
}

fn directory_size(root: &Path, read_errors: &mut usize) -> Result<u64> {
    let mut total = 0_u64;
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            *read_errors += 1;
            return Err(error.into());
        }
    };
    for entry in entries {
        let Ok(entry) = entry else {
            *read_errors += 1;
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            *read_errors += 1;
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            match directory_size(&entry.path(), read_errors) {
                Ok(size) => total = total.saturating_add(size),
                Err(_) => *read_errors += 1,
            }
        } else if file_type.is_file() {
            match entry.metadata() {
                Ok(metadata) => total = total.saturating_add(metadata.len()),
                Err(_) => *read_errors += 1,
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_orders_target_directories_without_following_links() {
        let root = tempfile::tempdir().expect("tempdir");
        let small = root.path().join("a").join("target");
        let large = root.path().join("b").join("nested").join("target");
        fs::create_dir_all(&small).expect("small");
        fs::create_dir_all(&large).expect("large");
        fs::write(small.join("a.bin"), vec![0_u8; 8]).expect("small file");
        fs::write(large.join("b.bin"), vec![0_u8; 32]).expect("large file");
        let report = audit_builds(root.path(), 16, 4, true).expect("audit");
        assert_eq!(report.scanned_target_directories, 2);
        assert_eq!(report.findings[0].bytes, 32);
        assert!(report.findings[0].exceeds_threshold);
        assert!(!report.findings[1].exceeds_threshold);
        assert!(report.findings.iter().all(|finding| finding.path.is_some()));
    }
}
