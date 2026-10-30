use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use paneforge_adapter::{
    dedicated_workspace_name_for_project_path, DedicatedWorkspaceReadiness, FakePaneForgeBehavior,
    FakePaneForgeServer, TOOL_ADD_PROJECT_TO_WORKSPACE, TOOL_CREATE_WORKSPACE, TOOL_KILL_SESSION,
    TOOL_LAUNCH_TASK, TOOL_SUBMIT_TO_SESSION, TOOL_WRITE_TO_SESSION,
};
use coordination_coordinator::{
    parse_real_paneforge_canary_args, prepare_synthetic_git_repository, run_real_paneforge_canary,
    RealPaneForgeCanaryArgs,
};
use coordination_core::{OutboxState, ResultAggregationState, WorkerState, WorkflowState};
use coordination_store::ExternalBindingState;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn real_canary_args_require_explicit_dedicated_inputs_and_parse_timeouts() {
    let args = parse_real_paneforge_canary_args([
        "--discovery",
        "C:/fixture/mcp-orchestrator.json",
        "--state-dir",
        "C:/fixture/state",
        "--project-path",
        "C:/fixture/project",
        "--workspace-name",
        "codex-paneforge-phase7-real-fixture",
        "--project-name",
        "real-fixture",
        "--marker",
        "CODEX_PANEFORGE_REAL_MARKER",
        "--timeout-ms",
        "420000",
        "--poll-interval-ms",
        "750",
        "--connect-timeout-ms",
        "2500",
        "--request-timeout-ms",
        "12000",
    ])
    .unwrap();

    assert_eq!(
        args.discovery,
        PathBuf::from("C:/fixture/mcp-orchestrator.json")
    );
    assert_eq!(args.state_dir, PathBuf::from("C:/fixture/state"));
    assert_eq!(args.project_path, PathBuf::from("C:/fixture/project"));
    assert_eq!(args.workspace_name, "codex-paneforge-phase7-real-fixture");
    assert_eq!(args.project_name, "real-fixture");
    assert_eq!(args.marker, "CODEX_PANEFORGE_REAL_MARKER");
    assert_eq!(args.timeout, Duration::from_millis(420_000));
    assert_eq!(args.poll_interval, Duration::from_millis(750));
    assert_eq!(args.connect_timeout, Duration::from_millis(2_500));
    assert_eq!(args.request_timeout, Duration::from_millis(12_000));
}

#[test]
fn real_canary_args_fail_closed_when_a_required_input_is_missing() {
    let error = parse_real_paneforge_canary_args([
        "--discovery",
        "C:/fixture/mcp-orchestrator.json",
        "--state-dir",
        "C:/fixture/state",
    ])
    .unwrap_err();

    assert_eq!(error.message, "missing required argument --project-path");
}

#[test]
fn real_canary_runner_uses_fresh_store_and_preserves_the_launched_session() {
    let dir = tempdir().unwrap();
    let registration = prepare_synthetic_git_repository(dir.path(), "real-canary-project").unwrap();
    let project_path = registration.repository.path.clone();
    let workspace_name = "codex-paneforge-phase7-real-runner";
    let project_name = "real-canary-project";
    let fake = FakePaneForgeServer::start(FakePaneForgeBehavior {
        complete_launches: true,
        workspaces: vec![json!({
            "id": "workspace-secret-id",
            "name": workspace_name
        })],
        projects: vec![json!({
            "id": "project-secret-id",
            "name": project_name,
            "path": project_path,
            "workspaceName": workspace_name
        })],
        ..FakePaneForgeBehavior::default()
    })
    .unwrap();
    let discovery = dir.path().join("mcp-orchestrator.json");
    fs::write(
        &discovery,
        serde_json::to_vec(&json!({
            "mcpServers": {
                "paneforge": {
                    "url": fake.endpoint(),
                    "headers": {}
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let state_dir = dir.path().join("state");

    let challenge = "tuptuo citehtnys";
    let expected_response = "synthetic output";
    let report = run_real_paneforge_canary(RealPaneForgeCanaryArgs {
        discovery,
        state_dir: state_dir.clone(),
        project_path: PathBuf::from(&registration.repository.path),
        workspace_name: workspace_name.to_owned(),
        project_name: project_name.to_owned(),
        marker: challenge.to_owned(),
        timeout: Duration::from_secs(2),
        poll_interval: Duration::from_millis(10),
        connect_timeout: Duration::from_millis(100),
        request_timeout: Duration::from_millis(250),
    })
    .unwrap();

    assert_eq!(
        report.workspace_gate_readiness,
        DedicatedWorkspaceReadiness::Ready
    );
    assert!(report.session_preserved);
    assert!(state_dir.join("coordination.db").is_file());
    assert!(report.canary.marker_observed);
    assert_eq!(report.canary.binding_state, ExternalBindingState::Active);
    assert_eq!(report.canary.outbox_state, OutboxState::Acknowledged);
    assert_eq!(report.canary.worker_state, WorkerState::Succeeded);
    assert_eq!(report.canary.workflow_state, WorkflowState::Succeeded);
    assert_eq!(report.canary.result_state, ResultAggregationState::Complete);
    assert_eq!(report.canary.submit_calls, 0);
    assert_eq!(report.canary.kill_calls, 0);
    assert_eq!(report.canary.write_calls, 0);

    let records = fake.records();
    let launch_prompt = fake.last_tool_arguments(TOOL_LAUNCH_TASK).unwrap()["prompt"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(launch_prompt.contains(challenge));
    assert!(!launch_prompt.contains(expected_response));
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
fn real_canary_runner_creates_project_dedicated_workspace_when_source_workspace_is_not_launchable()
{
    let dir = tempdir().unwrap();
    let registration =
        prepare_synthetic_git_repository(dir.path(), "real-canary-routed-project").unwrap();
    let project_path = registration.repository.path.clone();
    let source_workspace_name = "source-workspace-root";
    let launch_workspace_name = dedicated_workspace_name_for_project_path(&project_path);
    let project_name = "real-canary-routed-project";
    let fake = FakePaneForgeServer::start(FakePaneForgeBehavior {
        complete_launches: true,
        workspaces: vec![json!({
            "id": "source-workspace-id",
            "name": source_workspace_name,
            "path": dir.path().to_string_lossy()
        })],
        projects: vec![json!({
            "id": "source-project-id",
            "name": project_name,
            "path": project_path,
            "workspaceName": source_workspace_name
        })],
        ..FakePaneForgeBehavior::default()
    })
    .unwrap();
    let discovery = dir.path().join("mcp-orchestrator.json");
    fs::write(
        &discovery,
        serde_json::to_vec(&json!({
            "mcpServers": {
                "paneforge": {
                    "url": fake.endpoint(),
                    "headers": {}
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let state_dir = dir.path().join("state-routed");

    let report = run_real_paneforge_canary(RealPaneForgeCanaryArgs {
        discovery,
        state_dir,
        project_path: PathBuf::from(&registration.repository.path),
        workspace_name: source_workspace_name.to_owned(),
        project_name: project_name.to_owned(),
        marker: "tuptuo citehtnys".to_owned(),
        timeout: Duration::from_secs(2),
        poll_interval: Duration::from_millis(10),
        connect_timeout: Duration::from_millis(100),
        request_timeout: Duration::from_millis(250),
    })
    .unwrap();

    assert_eq!(
        report.workspace_gate_readiness,
        DedicatedWorkspaceReadiness::Ready
    );
    assert!(report.workspace_setup_side_effects);
    assert_eq!(
        report.source_workspace_name.as_deref(),
        Some(source_workspace_name)
    );
    assert_eq!(report.launch_workspace_name, launch_workspace_name);
    assert_eq!(report.canary.launch_workspace_name, launch_workspace_name);
    assert!(report.canary.cwd_matched);

    let create_args = fake.last_tool_arguments(TOOL_CREATE_WORKSPACE).unwrap();
    assert_eq!(create_args["name"], launch_workspace_name);
    assert_eq!(create_args["path"], project_path);
    let add_args = fake
        .last_tool_arguments(TOOL_ADD_PROJECT_TO_WORKSPACE)
        .unwrap();
    assert_eq!(add_args["workspaceName"], launch_workspace_name);
    assert_eq!(add_args["projectPath"], project_path);
    let launch_args = fake.last_tool_arguments(TOOL_LAUNCH_TASK).unwrap();
    assert_eq!(launch_args["workspaceName"], launch_workspace_name);
    assert_ne!(launch_args["workspaceName"], source_workspace_name);
    let launch_prompt = launch_args["prompt"].as_str().unwrap();
    assert!(!launch_prompt.contains("Set-Location"));
}
