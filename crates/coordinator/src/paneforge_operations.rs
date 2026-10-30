use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

use paneforge_adapter::{
    read_paneforge_discovery_file, require_dispatch_requalification, run_read_only_preflight,
    AdapterReadiness, AdapterTimeouts, PaneForgeAdapter,
};
use coordination_core::{CoordinationError, ErrorCode};
use coordination_protocol::{PaneForgePreflightInput, PaneForgeSyntheticCanaryInput};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{run_real_paneforge_canary, RealPaneForgeCanaryArgs};

const DISCOVERY_ENV: &str = "CODEX_PANEFORGE_DISCOVERY_FILE";

pub fn run_paneforge_preflight(input: PaneForgePreflightInput) -> Result<Value, CoordinationError> {
    input.validate_contract()?;
    let discovery = resolve_discovery_path(input.discovery_path.as_deref())?;
    let endpoint = read_paneforge_discovery_file(discovery, input.discovery_generation)?;
    let mut adapter = PaneForgeAdapter::new(
        endpoint,
        AdapterTimeouts::new(
            Duration::from_millis(input.connect_timeout_ms),
            Duration::from_millis(input.request_timeout_ms),
        ),
    )?;
    let mut report = run_read_only_preflight(&mut adapter);
    if let Err(error) = require_dispatch_requalification(&report) {
        let readiness = if error.code == ErrorCode::AdapterIncompatible {
            AdapterReadiness::Incompatible
        } else {
            AdapterReadiness::Degraded
        };
        report.readiness = readiness;
        report.status.readiness = readiness;
        report.error = Some(error);
    }
    serialize_operation_report(report)
}

pub fn run_paneforge_synthetic_canary(
    input: PaneForgeSyntheticCanaryInput,
    coordination_data_dir: &Path,
) -> Result<Value, CoordinationError> {
    input.validate_contract()?;
    let discovery = resolve_discovery_path(input.discovery_path.as_deref())?;
    let state_dir = coordination_data_dir
        .join("canaries")
        .join(hex::encode(Sha256::digest(
            input.request_id.as_str().as_bytes(),
        )));
    let report = run_real_paneforge_canary(RealPaneForgeCanaryArgs {
        discovery,
        state_dir,
        project_path: PathBuf::from(input.project_path),
        workspace_name: input.workspace_name,
        project_name: input.project_name,
        marker: input.marker,
        timeout: Duration::from_millis(input.timeout_ms),
        poll_interval: Duration::from_millis(input.poll_interval_ms),
        connect_timeout: Duration::from_millis(input.connect_timeout_ms),
        request_timeout: Duration::from_millis(input.request_timeout_ms),
    })?;
    serialize_operation_report(report)
}

fn resolve_discovery_path(explicit: Option<&str>) -> Result<PathBuf, CoordinationError> {
    explicit
        .map(PathBuf::from)
        .or_else(|| env::var_os(DISCOVERY_ENV).map(PathBuf::from))
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::AdapterNotReady,
                format!("missing discoveryPath or {DISCOVERY_ENV}"),
            )
        })
}

fn serialize_operation_report(report: impl Serialize) -> Result<Value, CoordinationError> {
    serde_json::to_value(report).map_err(|error| {
        CoordinationError::new(
            ErrorCode::InternalInvariantViolation,
            format!("serialize PaneForge operation report failed: {error}"),
        )
    })
}
