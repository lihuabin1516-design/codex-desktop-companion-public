use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::Serialize;

use crate::config::configured_ccpanes_path;

pub const CCPANES_TARGET_ENV: &str = "CODEX_CCPANES_EXE";
pub const CODEX_TARGET_ENV: &str = "CODEX_CLI_PATH";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedExecutable {
    #[serde(skip_serializing)]
    pub path: PathBuf,
    pub source: String,
}

pub fn resolve_ccpanes(explicit: Option<&Path>) -> Result<ResolvedExecutable> {
    let configured = env::var_os(CCPANES_TARGET_ENV)
        .filter(|value| !value.is_empty())
        .or_else(|| configured_ccpanes_path().map(PathBuf::into_os_string));
    resolve_ccpanes_with(
        explicit,
        configured.as_deref(),
        env::var_os("LOCALAPPDATA").as_deref(),
        env::var_os("ProgramFiles").as_deref(),
        env::var_os("ProgramFiles(x86)").as_deref(),
        env::var_os("PATH").as_deref(),
    )
}

pub fn resolve_codex() -> Result<ResolvedExecutable> {
    if let Some(configured) = env::var_os(CODEX_TARGET_ENV).filter(|value| !value.is_empty()) {
        if let Ok(resolved) = resolved_file(Path::new(&configured), "environment") {
            return Ok(resolved);
        }
    }
    find_on_path("codex").ok_or_else(|| anyhow::anyhow!("codex_cli_not_found"))
}

pub fn external_command_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

pub fn resolve_ccpanes_with(
    explicit: Option<&Path>,
    configured: Option<&OsStr>,
    local_app_data: Option<&OsStr>,
    program_files: Option<&OsStr>,
    program_files_x86: Option<&OsStr>,
    path: Option<&OsStr>,
) -> Result<ResolvedExecutable> {
    if let Some(path) = explicit {
        return resolved_file(path, "explicit");
    }
    if let Some(path) = configured.filter(|value| !value.is_empty()) {
        return resolved_file(Path::new(path), "environment");
    }

    let mut candidates = Vec::new();
    if let Some(root) = local_app_data {
        let root = PathBuf::from(root);
        candidates.push((root.join("cc-panes").join("cc-panes.exe"), "local-app-data"));
        candidates.push((
            root.join("Programs").join("cc-panes").join("cc-panes.exe"),
            "local-programs",
        ));
    }
    if let Some(root) = program_files {
        candidates.push((
            PathBuf::from(root).join("cc-panes").join("cc-panes.exe"),
            "program-files",
        ));
    }
    if let Some(root) = program_files_x86 {
        candidates.push((
            PathBuf::from(root).join("cc-panes").join("cc-panes.exe"),
            "program-files-x86",
        ));
    }
    for (candidate, source) in candidates {
        if candidate.is_file() {
            return resolved_file(&candidate, source);
        }
    }
    if let Some(path) = path {
        if let Some(found) = find_on_path_with("cc-panes", path) {
            return Ok(found);
        }
    }
    bail!("official_ccpanes_executable_not_found")
}

fn resolved_file(path: &Path, source: &str) -> Result<ResolvedExecutable> {
    if !path.is_file() {
        bail!("executable_not_found");
    }
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if env::current_exe()
        .ok()
        .and_then(|current| fs::canonicalize(current).ok())
        .as_ref()
        == Some(&resolved)
    {
        bail!("target_resolves_to_companion");
    }
    Ok(ResolvedExecutable {
        path: resolved,
        source: source.to_owned(),
    })
}

fn find_on_path(name: &str) -> Option<ResolvedExecutable> {
    find_on_path_with(name, &env::var_os("PATH")?)
}

fn find_on_path_with(name: &str, path: &OsStr) -> Option<ResolvedExecutable> {
    #[cfg(windows)]
    let extensions = [".exe", ".cmd", ".bat", ".ps1", ""];
    #[cfg(not(windows))]
    let extensions = [""];

    for directory in env::split_paths(path) {
        for extension in extensions {
            let candidate = directory.join(format!("{name}{extension}"));
            if candidate.is_file() {
                return resolved_file(&candidate, "path").ok();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, b"fixture").expect("write fixture");
    }

    #[test]
    fn explicit_path_has_priority() {
        let root = tempfile::tempdir().expect("tempdir");
        let explicit = root.path().join("explicit").join("cc-panes.exe");
        let configured = root.path().join("configured").join("cc-panes.exe");
        touch(&explicit);
        touch(&configured);
        let resolved = resolve_ccpanes_with(
            Some(&explicit),
            Some(configured.as_os_str()),
            None,
            None,
            None,
            None,
        )
        .expect("resolve explicit");
        assert_eq!(resolved.source, "explicit");
        assert_eq!(
            resolved.path,
            fs::canonicalize(explicit).expect("canonical")
        );
    }

    #[test]
    fn standard_local_app_data_path_is_detected() {
        let root = tempfile::tempdir().expect("tempdir");
        let executable = root.path().join("cc-panes").join("cc-panes.exe");
        touch(&executable);
        let resolved =
            resolve_ccpanes_with(None, None, Some(root.path().as_os_str()), None, None, None)
                .expect("resolve local app data");
        assert_eq!(resolved.source, "local-app-data");
    }

    #[test]
    fn path_fallback_prefers_executable_extension() {
        let root = tempfile::tempdir().expect("tempdir");
        let executable = root.path().join("cc-panes.exe");
        touch(&executable);
        let resolved =
            resolve_ccpanes_with(None, None, None, None, None, Some(root.path().as_os_str()))
                .expect("resolve path");
        assert_eq!(resolved.source, "path");
    }

    #[cfg(windows)]
    #[test]
    fn extended_windows_paths_are_normalized_for_script_invocation() {
        assert_eq!(
            external_command_path(Path::new(r"\\?\C:\Tools\codex.cmd")),
            PathBuf::from(r"C:\Tools\codex.cmd")
        );
        assert_eq!(
            external_command_path(Path::new(r"\\?\UNC\server\share\codex.cmd")),
            PathBuf::from(r"\\server\share\codex.cmd")
        );
    }
}
