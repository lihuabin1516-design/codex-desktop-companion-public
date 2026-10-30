use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use paneforge_adapter::{
    dedicated_workspace_name_for_project_path, ensure_dedicated_workspace_project,
    read_paneforge_discovery_file, AdapterTimeouts, PaneForgeAdapter, DedicatedWorkspaceReadiness,
    DedicatedWorkspaceSpec,
};
use coordination_core::{
    AccessMode, CallerId, CoordinationCommand, CoordinationError, DirtyPolicy, ErrorCode,
    IdempotencyKey, RepositoryRef, RequestId, WorkerExecutor,
};
use coordination_protocol::{IpcEnvelope, IPC_SCHEMA_VERSION};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealPaneForgeCanaryArgs {
    pub discovery: PathBuf,
    pub state_dir: PathBuf,
    pub project_path: PathBuf,
    pub workspace_name: String,
    pub project_name: String,
    pub marker: String,
    pub timeout: Duration,
    pub poll_interval: Duration,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
}

pub const REAL_PANEFORGE_CANARY_RUNNER_CONTRACT_VERSION: &str =
    "coordination.real-paneforge-canary-runner/v1beta1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RealPaneForgeCanaryReport {
    pub contract_version: String,
    pub workspace_gate_contract_version: String,
    pub workspace_gate_readiness: DedicatedWorkspaceReadiness,
    pub workspace_setup_side_effects: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_workspace_name: Option<String>,
    pub launch_workspace_name: String,
    pub repository_head_digest: String,
    pub state_store_digest: String,
    pub session_preserved: bool,
    pub canary: crate::PaneForgeCanaryReport,
}

pub fn parse_real_paneforge_canary_args<I, S>(
    args: I,
) -> Result<RealPaneForgeCanaryArgs, CoordinationError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut discovery = None;
    let mut state_dir = None;
    let mut project_path = None;
    let mut workspace_name = None;
    let mut project_name = None;
    let mut marker = None;
    let mut timeout_ms = 420_000_u64;
    let mut poll_interval_ms = 750_u64;
    let mut connect_timeout_ms = 2_000_u64;
    let mut request_timeout_ms = 12_000_u64;
    let mut args = args.into_iter().map(Into::into);

    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy();
        match arg.as_ref() {
            "--discovery" => discovery = Some(required_path(&mut args, "--discovery")?),
            "--state-dir" => state_dir = Some(required_path(&mut args, "--state-dir")?),
            "--project-path" => project_path = Some(required_path(&mut args, "--project-path")?),
            "--workspace-name" => {
                workspace_name = Some(required_string(&mut args, "--workspace-name")?)
            }
            "--project-name" => project_name = Some(required_string(&mut args, "--project-name")?),
            "--marker" => marker = Some(required_string(&mut args, "--marker")?),
            "--timeout-ms" => timeout_ms = required_u64(&mut args, "--timeout-ms")?,
            "--poll-interval-ms" => {
                poll_interval_ms = required_u64(&mut args, "--poll-interval-ms")?
            }
            "--connect-timeout-ms" => {
                connect_timeout_ms = required_u64(&mut args, "--connect-timeout-ms")?
            }
            "--request-timeout-ms" => {
                request_timeout_ms = required_u64(&mut args, "--request-timeout-ms")?
            }
            other => {
                return Err(CoordinationError::invalid_request(format!(
                    "unknown argument {other}"
                )));
            }
        }
    }

    Ok(RealPaneForgeCanaryArgs {
        discovery: required_argument(discovery, "--discovery")?,
        state_dir: required_argument(state_dir, "--state-dir")?,
        project_path: required_argument(project_path, "--project-path")?,
        workspace_name: required_argument(workspace_name, "--workspace-name")?,
        project_name: required_argument(project_name, "--project-name")?,
        marker: required_argument(marker, "--marker")?,
        timeout: Duration::from_millis(timeout_ms),
        poll_interval: Duration::from_millis(poll_interval_ms),
        connect_timeout: Duration::from_millis(connect_timeout_ms),
        request_timeout: Duration::from_millis(request_timeout_ms),
    })
}

pub fn run_real_paneforge_canary(
    args: RealPaneForgeCanaryArgs,
) -> Result<RealPaneForgeCanaryReport, CoordinationError> {
    let expected_response = canary_response_marker(&args.marker)?;
    prepare_fresh_state_dir(&args.state_dir)?;
    let project_path = args.project_path.to_string_lossy().into_owned();
    let snapshot = crate::inspect_git_worktree(&args.project_path)?;
    if !snapshot.dirty_paths.is_empty() {
        return Err(CoordinationError::new(
            ErrorCode::RepositoryBaselineChanged,
            "real canary repository must be clean",
        )
        .with_current_state("DIRTY_WORKTREE"));
    }
    let head = snapshot.head.ok_or_else(|| {
        CoordinationError::new(
            ErrorCode::RepositoryBaselineChanged,
            "real canary repository must have a committed HEAD",
        )
    })?;
    crate::verify_synthetic_repository_marker(&args.project_path)?;

    let endpoint = read_paneforge_discovery_file(&args.discovery, 1)?;
    let mut adapter = PaneForgeAdapter::new(
        endpoint,
        AdapterTimeouts::new(args.connect_timeout, args.request_timeout),
    )?;
    let source_workspace_name = Some(args.workspace_name.clone());
    let launch_workspace_name =
        launch_workspace_name_for_project(&project_path, source_workspace_name.as_deref());
    let workspace_gate = ensure_dedicated_workspace_project(
        &mut adapter,
        DedicatedWorkspaceSpec {
            workspace_name: launch_workspace_name.clone(),
            project_path: project_path.clone(),
            project_name: args.project_name.clone(),
        },
    );
    if workspace_gate.readiness != DedicatedWorkspaceReadiness::Ready {
        return Err(workspace_gate.error.clone().unwrap_or_else(|| {
            CoordinationError::new(
                ErrorCode::AdapterNotReady,
                format!(
                    "dedicated Workspace/Project gate is {:?}",
                    workspace_gate.readiness
                ),
            )
        }));
    }
    let capability = workspace_gate.status.capability.clone().ok_or_else(|| {
        CoordinationError::new(
            ErrorCode::AdapterNotReady,
            "dedicated Workspace/Project gate did not retain capability snapshot",
        )
    })?;

    let database_path = args.state_dir.join("coordination.db");
    let mut runtime = crate::CoordinatorRuntime::open(
        &database_path,
        &args.state_dir,
        "paneforge-v0.12.3-real-canary",
        crate::CoordinatorConfig::default(),
    )?;
    let request_digest = hex::encode(Sha256::digest(args.marker.as_bytes()));
    let request_id = RequestId::try_new(format!("real-canary-{}", &request_digest[..24]))?;
    let response = runtime.handle_ipc(IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: request_id.clone(),
        message: coordination_protocol::IpcMessage::command(CoordinationCommand::WorkerDispatch {
            idempotency_key: IdempotencyKey::try_new(
                "worker_dispatch",
                CallerId::try_new("real-paneforge-canary")?,
                request_id,
            )?,
            workflow_id: None,
            repository: RepositoryRef {
                path: project_path,
                expected_head: Some(head.clone()),
                dirty_policy: DirtyPolicy::Reject,
                workspace_name: source_workspace_name.clone(),
            },
            objective: format!(
                "只读兼容验收：保持项目文件原样，不改变当前工作目录。将下方 challenge 按 Unicode 字符逆序，\
并报告当前工作目录。严格输出两行：第一行只写逆序结果；第二行格式为 PANEFORGE_CANARY_CWD=<当前工作目录完整路径>；不要复述原 challenge。\nchallenge:\n{}",
                args.marker
            ),
            access: AccessMode::Read,
            executor: WorkerExecutor {
                cli_tool: "codex".to_owned(),
            },
            timeout_seconds: args.timeout.as_secs().max(1),
        }),
    });
    if let Some(error) = response.error {
        return Err(error);
    }
    if response.response.is_none() {
        return Err(CoordinationError::new(
            ErrorCode::InternalInvariantViolation,
            "real canary dispatch did not return an IPC response",
        ));
    }

    runtime.set_dispatch_ready(true);
    let canary = runtime
        .run_next_paneforge_canary(
            &mut adapter,
            &capability,
            crate::PaneForgeCanaryPolicy {
                expected_marker: expected_response,
                timeout: args.timeout,
                poll_interval: args.poll_interval,
            },
            current_time_ms()?,
        )?
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::InternalInvariantViolation,
                "real canary dispatch queue was empty",
            )
        })?;

    Ok(RealPaneForgeCanaryReport {
        contract_version: REAL_PANEFORGE_CANARY_RUNNER_CONTRACT_VERSION.to_owned(),
        workspace_gate_contract_version: workspace_gate.contract_version,
        workspace_gate_readiness: workspace_gate.readiness,
        workspace_setup_side_effects: workspace_gate.external_workspace_side_effects,
        source_workspace_name,
        launch_workspace_name,
        repository_head_digest: digest(&head),
        state_store_digest: digest(&database_path.to_string_lossy()),
        session_preserved: true,
        canary,
    })
}

fn launch_workspace_name_for_project(
    project_path: &str,
    _source_workspace_name: Option<&str>,
) -> String {
    dedicated_workspace_name_for_project_path(project_path)
}

fn canary_response_marker(challenge: &str) -> Result<String, CoordinationError> {
    let response = challenge.chars().rev().collect::<String>();
    if response == challenge {
        return Err(CoordinationError::invalid_request(
            "real canary marker challenge must not be palindromic",
        ));
    }
    Ok(response)
}

fn required_path<I>(args: &mut I, name: &'static str) -> Result<PathBuf, CoordinationError>
where
    I: Iterator<Item = OsString>,
{
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| CoordinationError::invalid_request(format!("{name} requires a value")))
}

fn required_string<I>(args: &mut I, name: &'static str) -> Result<String, CoordinationError>
where
    I: Iterator<Item = OsString>,
{
    let value = args
        .next()
        .ok_or_else(|| CoordinationError::invalid_request(format!("{name} requires a value")))?
        .to_string_lossy()
        .into_owned();
    if value.trim().is_empty() {
        return Err(CoordinationError::invalid_request(format!(
            "{name} must not be empty"
        )));
    }
    Ok(value)
}

fn required_u64<I>(args: &mut I, name: &'static str) -> Result<u64, CoordinationError>
where
    I: Iterator<Item = OsString>,
{
    let value = required_string(args, name)?;
    let parsed = value.parse::<u64>().map_err(|error| {
        CoordinationError::invalid_request(format!("{name} must be an integer: {error}"))
    })?;
    if parsed == 0 {
        return Err(CoordinationError::invalid_request(format!(
            "{name} must be greater than zero"
        )));
    }
    Ok(parsed)
}

fn required_argument<T>(value: Option<T>, name: &'static str) -> Result<T, CoordinationError> {
    value.ok_or_else(|| {
        CoordinationError::invalid_request(format!("missing required argument {name}"))
    })
}

fn prepare_fresh_state_dir(path: &PathBuf) -> Result<(), CoordinationError> {
    if path.exists() {
        let mut entries = fs::read_dir(path).map_err(|error| {
            CoordinationError::new(
                ErrorCode::PersistenceUnavailable,
                format!("inspect real canary state directory failed: {error}"),
            )
        })?;
        if entries
            .next()
            .transpose()
            .map_err(|error| {
                CoordinationError::new(
                    ErrorCode::PersistenceUnavailable,
                    format!("inspect real canary state directory failed: {error}"),
                )
            })?
            .is_some()
        {
            return Err(CoordinationError::invalid_request(
                "real canary state directory must be empty",
            ));
        }
    } else {
        fs::create_dir_all(path).map_err(|error| {
            CoordinationError::new(
                ErrorCode::PersistenceUnavailable,
                format!("create real canary state directory failed: {error}"),
            )
        })?;
    }
    Ok(())
}

fn current_time_ms() -> Result<i64, CoordinationError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            CoordinationError::new(
                ErrorCode::InternalInvariantViolation,
                format!("system time is before UNIX epoch: {error}"),
            )
        })?
        .as_millis();
    i64::try_from(millis).map_err(|_| {
        CoordinationError::new(
            ErrorCode::InternalInvariantViolation,
            "system time milliseconds exceed i64",
        )
    })
}

fn digest(value: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(value.as_bytes())))
}
