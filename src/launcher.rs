use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::json;

use crate::discovery::resolve_paneforge;
use crate::environment::{current_sanitized_keys, sanitize_command};
use crate::hashing::hash_path;
use crate::logging::append_event;

#[derive(Debug, Default)]
pub struct LaunchRequest {
    pub explicit_target: Option<PathBuf>,
    pub arguments: Vec<OsString>,
    pub wait: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchReceipt {
    pub pid: u32,
    pub target_source: String,
    pub target_path_hash: String,
    pub removed_environment_keys: Vec<String>,
    pub exit_code: Option<i32>,
}

pub fn launch(request: LaunchRequest) -> Result<LaunchReceipt> {
    let target = resolve_paneforge(request.explicit_target.as_deref())?;
    let removed = current_sanitized_keys();
    let mut command = Command::new(&target.path);
    command.args(&request.arguments);
    sanitize_command(&mut command, &removed);

    #[cfg(windows)]
    if !request.wait {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("launch official PaneForge ({})", target.source))?;
    let pid = child.id();
    let exit_status = if request.wait {
        Some(child.wait().context("wait for official PaneForge")?)
    } else {
        None
    };
    let receipt = LaunchReceipt {
        pid,
        target_source: target.source,
        target_path_hash: hash_path(&target.path),
        removed_environment_keys: removed
            .iter()
            .map(|key| key.to_string_lossy().into_owned())
            .collect(),
        exit_code: exit_status.as_ref().and_then(ExitStatus::code),
    };
    let _ = append_event(
        "paneforge_launch",
        json!({
            "pid": receipt.pid,
            "targetSource": receipt.target_source,
            "targetPathHash": receipt.target_path_hash,
            "removedEnvironmentKeys": receipt.removed_environment_keys,
            "wait": request.wait,
            "exitCode": receipt.exit_code,
        }),
    );
    Ok(receipt)
}

pub fn probe_target_path(explicit: Option<&Path>) -> Result<PathBuf> {
    Ok(resolve_paneforge(explicit)?.path)
}
