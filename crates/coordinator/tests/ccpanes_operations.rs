use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use ccpanes_adapter::{
    FakeCcpanesBehavior, FakeCcpanesServer, TOOL_GET_SESSION_OUTPUT, TOOL_KILL_SESSION,
    TOOL_LAUNCH_TASK, TOOL_SUBMIT_TO_SESSION, TOOL_WRITE_TO_SESSION,
};
use coordination_coordinator::{
    prepare_synthetic_git_repository, CoordinatorConfig, CoordinatorRuntime,
};
use coordination_core::{ErrorCode, RequestId};
use coordination_protocol::{
    CcpanesPreflightInput, CcpanesSyntheticCanaryInput, IpcEnvelope, IpcMessage, IpcResponse,
    CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION, CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION,
    IPC_SCHEMA_VERSION,
};
use serde_json::json;
use tempfile::tempdir;

const DISCOVERY_ENV: &str = "CODEX_CCPANES_DISCOVERY_FILE";

#[test]
fn ccpanes_preflight_returns_ready_report_without_external_worker_side_effects() {
    let dir = tempdir().unwrap();
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior::default()).unwrap();
    let discovery = write_discovery(dir.path(), &fake);
    let mut runtime = open_runtime(dir.path());

    let response = runtime.handle_ipc(IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new("ccpanes-preflight-ready").unwrap(),
        message: IpcMessage::CcpanesPreflight(CcpanesPreflightInput {
            schema_version: CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION.to_owned(),
            discovery_path: Some(discovery.to_string_lossy().into_owned()),
            discovery_generation: 3,
            connect_timeout_ms: 100,
            request_timeout_ms: 250,
        }),
    });

    assert!(response.ok, "{:?}", response.error);
    let IpcResponse::Json { value } = response.response.unwrap() else {
        panic!("preflight response must be structured JSON");
    };
    assert_eq!(value["readiness"], "READY");
    assert_eq!(value["externalWorkerSideEffects"], false);
    assert!(value["error"].is_null());
    assert!(fake
        .records()
        .iter()
        .all(|record| record.tool_name.as_deref() != Some(TOOL_LAUNCH_TASK)));
}

#[test]
fn ccpanes_preflight_without_discovery_returns_adapter_not_ready() {
    without_discovery_env(|| {
        let dir = tempdir().unwrap();
        let mut runtime = open_runtime(dir.path());

        let response = runtime.handle_ipc(IpcEnvelope {
            schema_version: IPC_SCHEMA_VERSION.to_owned(),
            request_id: RequestId::try_new("ccpanes-preflight-missing").unwrap(),
            message: IpcMessage::CcpanesPreflight(CcpanesPreflightInput {
                schema_version: CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION.to_owned(),
                discovery_path: None,
                discovery_generation: 1,
                connect_timeout_ms: 100,
                request_timeout_ms: 250,
            }),
        });

        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, ErrorCode::AdapterNotReady);
    });
}

#[test]
fn ccpanes_preflight_reports_typed_incompatible_when_dispatch_requalification_fails() {
    let dir = tempdir().unwrap();
    let behavior = FakeCcpanesBehavior::default();
    let tools = behavior
        .tools
        .into_iter()
        .filter(|tool| tool["name"] != TOOL_GET_SESSION_OUTPUT)
        .collect();
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        tools,
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let discovery = write_discovery(dir.path(), &fake);
    let mut runtime = open_runtime(dir.path());

    let response = runtime.handle_ipc(IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new("ccpanes-preflight-incompatible").unwrap(),
        message: IpcMessage::CcpanesPreflight(CcpanesPreflightInput {
            schema_version: CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION.to_owned(),
            discovery_path: Some(discovery.to_string_lossy().into_owned()),
            discovery_generation: 4,
            connect_timeout_ms: 100,
            request_timeout_ms: 250,
        }),
    });

    assert!(response.ok, "{:?}", response.error);
    let IpcResponse::Json { value } = response.response.unwrap() else {
        panic!("preflight response must be structured JSON");
    };
    assert_eq!(value["readiness"], "INCOMPATIBLE");
    assert_eq!(value["status"]["readiness"], "INCOMPATIBLE");
    assert_eq!(value["status"]["capability"]["dispatchReady"], false);
    assert_eq!(value["error"]["code"], "adapter_incompatible");
    assert_eq!(value["externalWorkerSideEffects"], false);
    assert!(fake
        .records()
        .iter()
        .all(|record| record.tool_name.as_deref() != Some(TOOL_LAUNCH_TASK)));
}

#[test]
fn ccpanes_synthetic_canary_launches_once_and_converges_store_without_control_writes() {
    let dir = tempdir().unwrap();
    let registration =
        prepare_synthetic_git_repository(dir.path(), "global-ccpanes-canary").unwrap();
    let workspace_name = "codex-ccpanes-phase7-v0123";
    let project_name = Path::new(&registration.repository.path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        complete_launches: true,
        workspaces: vec![json!({"id": "workspace-id", "name": workspace_name})],
        projects: vec![json!({
            "id": "project-id",
            "name": project_name,
            "path": registration.repository.path,
            "workspaceName": workspace_name
        })],
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let discovery = write_discovery(dir.path(), &fake);
    let mut runtime = open_runtime(dir.path());

    let response = runtime.handle_ipc(IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new("ccpanes-canary-envelope").unwrap(),
        message: IpcMessage::CcpanesSyntheticCanary(CcpanesSyntheticCanaryInput {
            schema_version: CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION.to_owned(),
            request_id: RequestId::try_new("global-canary-1").unwrap(),
            discovery_path: Some(discovery.to_string_lossy().into_owned()),
            project_path: registration.repository.path,
            workspace_name: workspace_name.to_owned(),
            project_name,
            marker: "tuptuo citehtnys".to_owned(),
            timeout_ms: 2_000,
            poll_interval_ms: 10,
            connect_timeout_ms: 100,
            request_timeout_ms: 250,
        }),
    });

    assert!(response.ok, "{:?}", response.error);
    let IpcResponse::Json { value } = response.response.unwrap() else {
        panic!("synthetic canary response must be structured JSON");
    };
    assert_eq!(value["sessionPreserved"], true);
    assert_eq!(value["canary"]["markerObserved"], true);
    assert_eq!(value["canary"]["finalStatusIsAccepted"], true);
    assert_eq!(value["canary"]["bindingState"], "ACTIVE");
    assert_eq!(value["canary"]["outboxState"], "ACKNOWLEDGED");
    assert_eq!(value["canary"]["workerState"], "SUCCEEDED");
    assert_eq!(value["canary"]["workflowState"], "SUCCEEDED");
    assert_eq!(value["canary"]["resultState"], "complete");
    assert_eq!(value["canary"]["launchTaskCalls"], 1);
    assert_eq!(value["canary"]["submitCalls"], 0);
    assert_eq!(value["canary"]["killCalls"], 0);
    assert_eq!(value["canary"]["writeCalls"], 0);

    let records = fake.records();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.tool_name.as_deref() == Some(TOOL_LAUNCH_TASK))
            .count(),
        1
    );
    assert_eq!(
        fake.last_tool_arguments(TOOL_LAUNCH_TASK).unwrap()["cliTool"],
        "codex"
    );
    for forbidden in [
        TOOL_SUBMIT_TO_SESSION,
        TOOL_KILL_SESSION,
        TOOL_WRITE_TO_SESSION,
    ] {
        assert!(records
            .iter()
            .all(|record| record.tool_name.as_deref() != Some(forbidden)));
    }
}

#[test]
fn ccpanes_synthetic_canary_fails_closed_for_dirty_or_unregistered_fixture() {
    let dir = tempdir().unwrap();
    let registration =
        prepare_synthetic_git_repository(dir.path(), "dirty-global-ccpanes-canary").unwrap();
    fs::write(
        Path::new(&registration.repository.path).join("dirty.txt"),
        b"dirty",
    )
    .unwrap();
    let workspace_name = "codex-ccpanes-phase7-v0123-dirty";
    let project_name = "dirty-compat-fixture";
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        complete_launches: true,
        workspaces: vec![json!({"id": "workspace-id", "name": workspace_name})],
        projects: vec![json!({
            "id": "project-id",
            "name": project_name,
            "path": registration.repository.path,
            "workspaceName": workspace_name
        })],
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let discovery = write_discovery(dir.path(), &fake);
    let mut runtime = open_runtime(dir.path());
    let dirty_response = runtime.handle_ipc(IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new("ccpanes-canary-dirty-envelope").unwrap(),
        message: IpcMessage::CcpanesSyntheticCanary(canary_input(
            "dirty-canary",
            &discovery,
            &registration.repository.path,
            workspace_name,
            project_name,
        )),
    });

    assert!(!dirty_response.ok);
    assert_eq!(
        dirty_response.error.unwrap().code,
        ErrorCode::RepositoryBaselineChanged
    );
    assert!(fake
        .records()
        .iter()
        .all(|record| record.tool_name.as_deref() != Some(TOOL_LAUNCH_TASK)));

    let clean_dir = tempdir().unwrap();
    let clean_registration =
        prepare_synthetic_git_repository(clean_dir.path(), "unregistered-global-canary").unwrap();
    let unregistered_fake = FakeCcpanesServer::start(FakeCcpanesBehavior::default()).unwrap();
    let unregistered_discovery = write_discovery(clean_dir.path(), &unregistered_fake);
    let mut clean_runtime = open_runtime(clean_dir.path());
    let unregistered_response = clean_runtime.handle_ipc(IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new("ccpanes-canary-unregistered-envelope").unwrap(),
        message: IpcMessage::CcpanesSyntheticCanary(canary_input(
            "unregistered-canary",
            &unregistered_discovery,
            &clean_registration.repository.path,
            "codex-ccpanes-phase7-v0123-unregistered",
            "unregistered-fixture",
        )),
    });

    assert!(!unregistered_response.ok);
    assert_eq!(
        unregistered_response.error.unwrap().code,
        ErrorCode::AdapterNotReady
    );
    assert!(unregistered_fake
        .records()
        .iter()
        .all(|record| record.tool_name.as_deref() != Some(TOOL_LAUNCH_TASK)));
}

#[test]
fn ccpanes_synthetic_canary_requires_committed_synthetic_repository_marker() {
    let dir = tempdir().unwrap();
    let registration =
        prepare_synthetic_git_repository(dir.path(), "markerless-global-ccpanes-canary").unwrap();
    let repository_path = Path::new(&registration.repository.path);
    fs::remove_file(repository_path.join(".codex-ccpanes-synthetic-repository.json")).unwrap();
    let commit = Command::new("git")
        .args([
            "-c",
            "user.name=Codex CC-Panes Synthetic Fixture",
            "-c",
            "user.email=codex-ccpanes-synthetic@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-am",
            "remove synthetic marker",
        ])
        .current_dir(repository_path)
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "{}",
        String::from_utf8_lossy(&commit.stderr)
    );

    let workspace_name = "codex-ccpanes-phase7-v0123-markerless";
    let project_name = "markerless-compat-fixture";
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        complete_launches: true,
        workspaces: vec![json!({"id": "workspace-id", "name": workspace_name})],
        projects: vec![json!({
            "id": "project-id",
            "name": project_name,
            "path": registration.repository.path,
            "workspace": workspace_name
        })],
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let discovery = write_discovery(dir.path(), &fake);
    let mut runtime = open_runtime(dir.path());

    let response = runtime.handle_ipc(IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new("ccpanes-canary-markerless-envelope").unwrap(),
        message: IpcMessage::CcpanesSyntheticCanary(canary_input(
            "markerless-canary",
            &discovery,
            &registration.repository.path,
            workspace_name,
            project_name,
        )),
    });

    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, ErrorCode::InvalidRequest);
    assert!(fake
        .records()
        .iter()
        .all(|record| record.tool_name.as_deref() != Some(TOOL_LAUNCH_TASK)));
}

fn open_runtime(root: &Path) -> CoordinatorRuntime {
    let data_dir = root.join("coordination-state");
    CoordinatorRuntime::open(
        data_dir.join("coordination.db"),
        &data_dir,
        "ccpanes-operations-test",
        CoordinatorConfig::default(),
    )
    .unwrap()
}

fn write_discovery(root: &Path, fake: &FakeCcpanesServer) -> std::path::PathBuf {
    let discovery = root.join("mcp-orchestrator.json");
    fs::write(
        &discovery,
        serde_json::to_vec(&json!({
            "mcpServers": {
                "ccpanes": {
                    "url": fake.endpoint(),
                    "headers": {}
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    discovery
}

fn canary_input(
    request_id: &str,
    discovery: &Path,
    project_path: &str,
    workspace_name: &str,
    project_name: &str,
) -> CcpanesSyntheticCanaryInput {
    CcpanesSyntheticCanaryInput {
        schema_version: CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new(request_id).unwrap(),
        discovery_path: Some(discovery.to_string_lossy().into_owned()),
        project_path: project_path.to_owned(),
        workspace_name: workspace_name.to_owned(),
        project_name: project_name.to_owned(),
        marker: "tuptuo citehtnys".to_owned(),
        timeout_ms: 2_000,
        poll_interval_ms: 10,
        connect_timeout_ms: 100,
        request_timeout_ms: 250,
    }
}

fn without_discovery_env(test: impl FnOnce()) {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let previous: Option<OsString> = std::env::var_os(DISCOVERY_ENV);
    std::env::remove_var(DISCOVERY_ENV);
    test();
    match previous {
        Some(value) => std::env::set_var(DISCOVERY_ENV, value),
        None => std::env::remove_var(DISCOVERY_ENV),
    }
}
