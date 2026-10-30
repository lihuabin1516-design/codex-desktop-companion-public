use std::time::Duration;

use ccpanes_adapter::{
    dedicated_workspace_name_for_project_path, AdapterTimeouts, CcpanesAdapter,
    CcpanesEndpointConfig, FakeCcpanesBehavior, FakeCcpanesServer, TOOL_GET_SESSION_OUTPUT,
    TOOL_GET_SESSION_STATUS, TOOL_LAUNCH_TASK,
};
use coordination_coordinator::{
    prepare_synthetic_git_repository, CcpanesCanaryPolicy, CoordinatorConfig, CoordinatorRuntime,
};
use coordination_core::{
    AccessMode, CallerId, CoordinationCommand, DirtyPolicy, ErrorCode, IdempotencyKey, OutboxState,
    RepositoryRef, RequestId, WorkerExecutor, WorkerState,
};
use coordination_protocol::{IpcEnvelope, IpcMessage, IpcResponse, IPC_SCHEMA_VERSION};
use coordination_store::ExternalBindingState;
use tempfile::tempdir;

#[test]
fn coordinator_executes_one_adapter_launch_and_converges_canary_store_state() {
    let dir = tempdir().unwrap();
    let repository =
        prepare_synthetic_git_repository(dir.path(), "codex-ccpanes-phase7-real-seam").unwrap();
    let expected_launch_workspace =
        dedicated_workspace_name_for_project_path(&repository.repository.path);
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        complete_launches: true,
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(fake.endpoint(), Vec::new(), 1).unwrap(),
        AdapterTimeouts::default(),
    )
    .unwrap();
    let capability = adapter.discover().unwrap();
    let database = dir.path().join("coordination.db");
    let mut runtime = CoordinatorRuntime::open(
        &database,
        dir.path(),
        "ccpanes-real-seam-test",
        CoordinatorConfig::default(),
    )
    .unwrap();
    let response = runtime.handle_ipc(IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new("real-seam-dispatch").unwrap(),
        message: IpcMessage::command(CoordinationCommand::WorkerDispatch {
            idempotency_key: IdempotencyKey::try_new(
                "worker_dispatch",
                CallerId::try_new("real-canary-test").unwrap(),
                RequestId::try_new("real-canary-test-1").unwrap(),
            )
            .unwrap(),
            workflow_id: None,
            repository: RepositoryRef {
                path: repository.repository.path.clone(),
                expected_head: repository.repository.expected_head,
                dirty_policy: DirtyPolicy::Reject,
                workspace_name: Some("codex-ccpanes-phase7-real-seam".to_owned()),
            },
            objective: "print synthetic output and a CCPANES_CANARY_CWD line, then stop".to_owned(),
            access: AccessMode::Read,
            executor: WorkerExecutor {
                cli_tool: "codex".to_owned(),
            },
            timeout_seconds: 120,
        }),
    });
    let IpcResponse::Json { value } = response.response.unwrap() else {
        panic!("worker dispatch response");
    };
    let worker_id =
        coordination_core::WorkerId::try_new(value["workerId"].as_str().expect("worker id"))
            .unwrap();
    let workflow_id =
        coordination_core::WorkflowId::try_new(value["workflowId"].as_str().expect("workflow id"))
            .unwrap();

    runtime.set_dispatch_ready(true);
    let report = runtime
        .run_next_ccpanes_canary(
            &mut adapter,
            &capability,
            CcpanesCanaryPolicy {
                expected_marker: "synthetic output".to_owned(),
                timeout: Duration::from_secs(2),
                poll_interval: Duration::from_millis(10),
            },
            1,
        )
        .unwrap()
        .expect("claimed canary");

    assert!(report.marker_observed);
    assert!(report.final_status_is_accepted);
    assert_eq!(report.final_status, "completed");
    assert_eq!(
        runtime.store().worker(&worker_id).unwrap().state,
        WorkerState::Succeeded
    );
    let binding = runtime
        .store()
        .external_binding_for_worker(&worker_id)
        .unwrap()
        .unwrap();
    assert_eq!(binding.state, ExternalBindingState::Active);
    assert!(binding.external_session_ref.is_some());
    let outbox = runtime
        .store()
        .due_outbox(i64::MAX, 10)
        .unwrap()
        .into_iter()
        .next();
    assert!(outbox.is_none());
    assert_eq!(
        runtime
            .aggregate_workflow_result(&workflow_id)
            .unwrap()
            .state,
        coordination_core::ResultAggregationState::Complete
    );

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
    assert_eq!(
        fake.last_tool_arguments(TOOL_LAUNCH_TASK).unwrap()["workspaceName"],
        expected_launch_workspace
    );
    assert!(records
        .iter()
        .any(|record| record.tool_name.as_deref() == Some(TOOL_GET_SESSION_STATUS)));
    assert!(records
        .iter()
        .any(|record| record.tool_name.as_deref() == Some(TOOL_GET_SESSION_OUTPUT)));
    assert_eq!(report.outbox_state, OutboxState::Acknowledged);
}

#[test]
fn coordinator_routes_source_workspace_to_project_dedicated_launch_workspace() {
    let dir = tempdir().unwrap();
    let repository =
        prepare_synthetic_git_repository(dir.path(), "codex-ccpanes-source-workspace-route")
            .unwrap();
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        complete_launches: true,
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(fake.endpoint(), Vec::new(), 1).unwrap(),
        AdapterTimeouts::default(),
    )
    .unwrap();
    let capability = adapter.discover().unwrap();
    let mut runtime = CoordinatorRuntime::open(
        dir.path().join("coordination.db"),
        dir.path(),
        "ccpanes-source-workspace-route-test",
        CoordinatorConfig::default(),
    )
    .unwrap();
    let source_workspace_name = "user-source-workspace";
    let expected_launch_workspace =
        dedicated_workspace_name_for_project_path(&repository.repository.path);
    let response = runtime.handle_ipc(IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new("source-workspace-route-dispatch").unwrap(),
        message: IpcMessage::command(CoordinationCommand::WorkerDispatch {
            idempotency_key: IdempotencyKey::try_new(
                "worker_dispatch",
                CallerId::try_new("source-workspace-route-test").unwrap(),
                RequestId::try_new("source-workspace-route-test-1").unwrap(),
            )
            .unwrap(),
            workflow_id: None,
            repository: RepositoryRef {
                path: repository.repository.path.clone(),
                expected_head: repository.repository.expected_head,
                dirty_policy: DirtyPolicy::Reject,
                workspace_name: Some(source_workspace_name.to_owned()),
            },
            objective: "print synthetic output and a CCPANES_CANARY_CWD line, then stop".to_owned(),
            access: AccessMode::Read,
            executor: WorkerExecutor {
                cli_tool: "codex".to_owned(),
            },
            timeout_seconds: 120,
        }),
    });
    assert!(response.ok, "{:?}", response.error);

    runtime.set_dispatch_ready(true);
    let report = runtime
        .run_next_ccpanes_canary(
            &mut adapter,
            &capability,
            CcpanesCanaryPolicy {
                expected_marker: "synthetic output".to_owned(),
                timeout: Duration::from_secs(2),
                poll_interval: Duration::from_millis(10),
            },
            1,
        )
        .unwrap()
        .expect("claimed canary");

    let launch_args = fake.last_tool_arguments(TOOL_LAUNCH_TASK).unwrap();
    assert_eq!(launch_args["projectPath"], repository.repository.path);
    assert_eq!(launch_args["workspaceName"], expected_launch_workspace);
    assert_ne!(launch_args["workspaceName"], source_workspace_name);
    assert_eq!(
        report.source_workspace_name.as_deref(),
        Some(source_workspace_name)
    );
    assert_eq!(report.launch_workspace_name, expected_launch_workspace);
    assert!(report.cwd_matched);
}

#[test]
fn coordinator_canary_fails_closed_when_cwd_marker_is_missing() {
    let dir = tempdir().unwrap();
    let repository =
        prepare_synthetic_git_repository(dir.path(), "codex-ccpanes-cwd-missing").unwrap();
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        complete_launches: true,
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(fake.endpoint(), Vec::new(), 1).unwrap(),
        AdapterTimeouts::default(),
    )
    .unwrap();
    let capability = adapter.discover().unwrap();
    let mut runtime = CoordinatorRuntime::open(
        dir.path().join("coordination.db"),
        dir.path(),
        "ccpanes-cwd-missing-test",
        CoordinatorConfig::default(),
    )
    .unwrap();
    let response = runtime.handle_ipc(IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new("cwd-missing-dispatch").unwrap(),
        message: IpcMessage::command(CoordinationCommand::WorkerDispatch {
            idempotency_key: IdempotencyKey::try_new(
                "worker_dispatch",
                CallerId::try_new("cwd-missing-test").unwrap(),
                RequestId::try_new("cwd-missing-test-1").unwrap(),
            )
            .unwrap(),
            workflow_id: None,
            repository: RepositoryRef {
                path: repository.repository.path,
                expected_head: repository.repository.expected_head,
                dirty_policy: DirtyPolicy::Reject,
                workspace_name: Some("source-workspace-root".to_owned()),
            },
            objective: "print synthetic output and stop".to_owned(),
            access: AccessMode::Read,
            executor: WorkerExecutor {
                cli_tool: "codex".to_owned(),
            },
            timeout_seconds: 120,
        }),
    });
    let IpcResponse::Json { value } = response.response.unwrap() else {
        panic!("worker dispatch response");
    };
    let worker_id =
        coordination_core::WorkerId::try_new(value["workerId"].as_str().expect("worker id"))
            .unwrap();
    let workflow_id =
        coordination_core::WorkflowId::try_new(value["workflowId"].as_str().expect("workflow id"))
            .unwrap();

    runtime.set_dispatch_ready(true);
    let error = runtime
        .run_next_ccpanes_canary(
            &mut adapter,
            &capability,
            CcpanesCanaryPolicy {
                expected_marker: "synthetic output".to_owned(),
                timeout: Duration::from_secs(2),
                poll_interval: Duration::from_millis(10),
            },
            1,
        )
        .expect_err("cwd marker must be part of the canary contract");

    assert_eq!(error.code, ErrorCode::AdapterIncompatible);
    assert_eq!(error.current_state.as_deref(), Some("CWD_MARKER_MISSING"));
    assert_eq!(
        runtime.store().worker(&worker_id).unwrap().state,
        WorkerState::Failed
    );
    assert_eq!(
        runtime
            .aggregate_workflow_result(&workflow_id)
            .unwrap()
            .state,
        coordination_core::ResultAggregationState::Partial
    );
}

#[test]
fn coordinator_canary_reopen_recovers_dropped_launch_response_as_result_unknown() {
    let dir = tempdir().unwrap();
    let repository =
        prepare_synthetic_git_repository(dir.path(), "codex-ccpanes-real-seam-unknown").unwrap();
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        drop_launch_response_after_side_effect: true,
        complete_launches: true,
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(fake.endpoint(), Vec::new(), 1).unwrap(),
        AdapterTimeouts::new(Duration::from_millis(100), Duration::from_millis(150)),
    )
    .unwrap();
    let capability = adapter.discover().unwrap();
    let database = dir.path().join("coordination.db");
    let mut runtime = CoordinatorRuntime::open(
        &database,
        dir.path(),
        "ccpanes-real-seam-unknown-test",
        CoordinatorConfig::default(),
    )
    .unwrap();
    let response = runtime.handle_ipc(IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new("real-seam-unknown-dispatch").unwrap(),
        message: IpcMessage::command(CoordinationCommand::WorkerDispatch {
            idempotency_key: IdempotencyKey::try_new(
                "worker_dispatch",
                CallerId::try_new("real-canary-unknown-test").unwrap(),
                RequestId::try_new("real-canary-unknown-test-1").unwrap(),
            )
            .unwrap(),
            workflow_id: None,
            repository: RepositoryRef {
                path: repository.repository.path,
                expected_head: repository.repository.expected_head,
                dirty_policy: DirtyPolicy::Reject,
                workspace_name: Some("codex-ccpanes-real-seam-unknown".to_owned()),
            },
            objective: "print synthetic output and stop".to_owned(),
            access: AccessMode::Read,
            executor: WorkerExecutor {
                cli_tool: "codex".to_owned(),
            },
            timeout_seconds: 120,
        }),
    });
    let IpcResponse::Json { value } = response.response.unwrap() else {
        panic!("worker dispatch response");
    };
    let worker_id =
        coordination_core::WorkerId::try_new(value["workerId"].as_str().expect("worker id"))
            .unwrap();

    runtime.set_dispatch_ready(true);
    let plan = runtime.tick(0).unwrap();
    let outbox_id = plan.dispatches[0].outbox_id.clone();
    let binding_id = runtime
        .store()
        .external_binding_for_worker(&worker_id)
        .unwrap()
        .unwrap()
        .binding_id;
    let error = runtime
        .run_next_ccpanes_canary(
            &mut adapter,
            &capability,
            CcpanesCanaryPolicy {
                expected_marker: "synthetic output".to_owned(),
                timeout: Duration::from_secs(2),
                poll_interval: Duration::from_millis(10),
            },
            1,
        )
        .expect_err("dropped launch response must remain unknown");
    assert_eq!(error.code, ErrorCode::DispatchResultUnknown);
    assert_eq!(
        runtime.store().outbox(&outbox_id).unwrap().state,
        OutboxState::ResultUnknown
    );
    assert_eq!(
        runtime.store().worker(&worker_id).unwrap().state,
        WorkerState::DispatchUnknown
    );
    assert_eq!(
        runtime.store().external_binding(&binding_id).unwrap().state,
        ExternalBindingState::ResultUnknown
    );
    drop(runtime);

    let mut reopened = CoordinatorRuntime::open(
        &database,
        dir.path(),
        "ccpanes-real-seam-unknown-test",
        CoordinatorConfig::default(),
    )
    .unwrap();
    assert_eq!(
        reopened.store().outbox(&outbox_id).unwrap().state,
        OutboxState::ResultUnknown
    );
    assert_eq!(
        reopened.store().worker(&worker_id).unwrap().state,
        WorkerState::DispatchUnknown
    );
    assert_eq!(
        reopened
            .store()
            .external_binding(&binding_id)
            .unwrap()
            .state,
        ExternalBindingState::ResultUnknown
    );
    reopened.set_dispatch_ready(true);
    assert!(reopened.tick(2).unwrap().dispatches.is_empty());
    assert_eq!(
        fake.records()
            .iter()
            .filter(|record| record.tool_name.as_deref() == Some(TOOL_LAUNCH_TASK))
            .count(),
        1
    );
}
