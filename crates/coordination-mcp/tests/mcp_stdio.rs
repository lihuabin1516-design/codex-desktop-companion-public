use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io::Cursor;
use std::rc::Rc;

use ccpanes_adapter::{
    dedicated_workspace_name_for_project_path, run_dedicated_workspace_project_gate,
    run_read_only_preflight, AdapterReadiness, AdapterTimeouts, CcpanesAdapter,
    CcpanesEndpointConfig, DedicatedWorkspaceReadiness, DedicatedWorkspaceSpec,
    FakeCcpanesBehavior, FakeCcpanesServer, FakeSessionSeed, TOOL_ADD_PROJECT_TO_WORKSPACE,
    TOOL_CREATE_WORKSPACE, TOOL_GET_SESSION_OUTPUT, TOOL_GET_SESSION_STATUS, TOOL_KILL_SESSION,
    TOOL_LAUNCH_TASK, TOOL_LIST_PANES, TOOL_LIST_SESSIONS, TOOL_SUBMIT_TO_SESSION,
};
use coordination_coordinator::{
    prepare_synthetic_git_repository, CoordinatorConfig, CoordinatorRuntime,
};
use coordination_core::{
    CoordinationError, ErrorCode, ExternalBindingId, OutboxId, OutboxState, ResultAggregationState,
    Revision, StepId, WorkerId, WorkerState, WorkflowId, WorkflowState, WORKFLOW_SPEC_KIND,
    WORKFLOW_SPEC_SCHEMA_VERSION,
};
use coordination_mcp::{default_caller_id, serve, CoordinatorIpcClient, JsonRpcRequest, McpServer};
use coordination_protocol::{
    CcpanesPreflightInput, CcpanesSyntheticCanaryInput, IpcEnvelope, IpcMessage, IpcResponse,
    CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION, CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION,
    RESULT_QUERY_SCHEMA_VERSION, STATUS_QUERY_SCHEMA_VERSION, WORKER_CANCEL_SCHEMA_VERSION,
    WORKER_DISPATCH_SCHEMA_VERSION, WORKFLOW_CANCEL_SCHEMA_VERSION, WORKFLOW_CREATE_SCHEMA_VERSION,
    WORKFLOW_START_SCHEMA_VERSION,
};
use coordination_store::{
    ExternalBindingState, RecordExternalCancelSuccessRequest, RecordExternalLaunchSuccessRequest,
    RecordWorkerResultRequest,
};
use serde_json::{json, Value};
use tempfile::{tempdir, TempDir};

#[derive(Debug)]
struct ClaimedLaunch {
    worker_id: WorkerId,
    step_id: StepId,
    outbox_id: OutboxId,
    outbox_revision: Revision,
    outbox_generation: u64,
    binding_id: ExternalBindingId,
    payload: Value,
}

#[derive(Clone)]
struct SharedRuntimeClient {
    runtime: Rc<RefCell<CoordinatorRuntime>>,
}

#[derive(Clone)]
struct RecordingClient {
    envelopes: Rc<RefCell<Vec<IpcEnvelope>>>,
    response: Value,
}

impl CoordinatorIpcClient for SharedRuntimeClient {
    fn exchange(&mut self, envelope: IpcEnvelope) -> Result<IpcResponse, CoordinationError> {
        let response = self.runtime.borrow_mut().handle_ipc(envelope);
        if response.ok {
            response.response.ok_or_else(|| {
                CoordinationError::new(
                    ErrorCode::InternalInvariantViolation,
                    "test runtime returned empty success response",
                )
            })
        } else {
            Err(response.error.unwrap_or_else(|| {
                CoordinationError::new(
                    ErrorCode::InternalInvariantViolation,
                    "test runtime returned empty error response",
                )
            }))
        }
    }
}

impl CoordinatorIpcClient for RecordingClient {
    fn exchange(&mut self, envelope: IpcEnvelope) -> Result<IpcResponse, CoordinationError> {
        self.envelopes.borrow_mut().push(envelope);
        Ok(IpcResponse::Json {
            value: self.response.clone(),
        })
    }
}

fn runtime_fixture() -> (TempDir, Rc<RefCell<CoordinatorRuntime>>) {
    let dir = tempdir().unwrap();
    let runtime = CoordinatorRuntime::open(
        dir.path().join("coordination.db"),
        dir.path(),
        "mcp-test-ipc",
        CoordinatorConfig::default(),
    )
    .unwrap();
    (dir, Rc::new(RefCell::new(runtime)))
}

fn server(runtime: Rc<RefCell<CoordinatorRuntime>>) -> McpServer<SharedRuntimeClient> {
    McpServer::new(SharedRuntimeClient { runtime }, default_caller_id())
}

fn recording_server(
    response: Value,
) -> (McpServer<RecordingClient>, Rc<RefCell<Vec<IpcEnvelope>>>) {
    let envelopes = Rc::new(RefCell::new(Vec::new()));
    let server = McpServer::new(
        RecordingClient {
            envelopes: Rc::clone(&envelopes),
            response,
        },
        default_caller_id(),
    );
    (server, envelopes)
}

fn request(id: &str, method: &str, params: Value) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: Some(json!(id)),
        method: method.to_owned(),
        params,
    }
}

fn call_tool<C: CoordinatorIpcClient>(
    server: &mut McpServer<C>,
    id: &str,
    name: &str,
    arguments: Value,
) -> Value {
    let response = server
        .handle(request(
            id,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        ))
        .expect("response");
    assert!(
        response.error.is_none(),
        "unexpected MCP error: {:?}",
        response.error
    );
    response.result.unwrap()["structuredContent"].clone()
}

fn call_tool_error<C: CoordinatorIpcClient>(
    server: &mut McpServer<C>,
    id: &str,
    name: &str,
    arguments: Value,
) -> Value {
    let response = server
        .handle(request(
            id,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        ))
        .expect("response");
    let error = response.error.expect("MCP error");
    error.data.unwrap()["coordinationError"].clone()
}

fn dispatch_args(request_id: &str) -> Value {
    json!({
        "schemaVersion": WORKER_DISPATCH_SCHEMA_VERSION,
        "requestId": request_id,
        "repository": {
            "path": "D:/SYNTHETIC",
            "expectedHead": "HEAD",
            "dirtyPolicy": "reject"
        },
        "objective": "inspect synthetic fixture",
        "access": "read",
        "executor": {
            "cliTool": "codex"
        },
        "timeoutSeconds": 1200,
        "result": {
            "format": "structured-summary"
        }
    })
}

fn dispatch_args_for_repository(
    request_id: &str,
    repository_path: &str,
    expected_head: &str,
    workspace_name: &str,
) -> Value {
    json!({
        "schemaVersion": WORKER_DISPATCH_SCHEMA_VERSION,
        "requestId": request_id,
        "repository": {
            "path": repository_path,
            "expectedHead": expected_head,
            "dirtyPolicy": "reject",
            "workspaceName": workspace_name
        },
        "objective": "inspect dedicated synthetic fixture",
        "access": "read",
        "executor": {
            "cliTool": "codex"
        },
        "timeoutSeconds": 1200,
        "result": {
            "format": "structured-summary"
        }
    })
}

fn workflow_result_args(workflow_id: &str) -> Value {
    json!({
        "schemaVersion": RESULT_QUERY_SCHEMA_VERSION,
        "workflowId": workflow_id
    })
}

fn worker_status_args(worker_id: &str) -> Value {
    json!({
        "schemaVersion": STATUS_QUERY_SCHEMA_VERSION,
        "workerId": worker_id
    })
}

fn workflow_status_args(workflow_id: &str) -> Value {
    json!({
        "schemaVersion": STATUS_QUERY_SCHEMA_VERSION,
        "workflowId": workflow_id
    })
}

fn workflow_create_args(request_id: &str) -> Value {
    json!({
        "schemaVersion": WORKFLOW_CREATE_SCHEMA_VERSION,
        "requestId": request_id,
        "spec": {
            "schemaVersion": WORKFLOW_SPEC_SCHEMA_VERSION,
            "kind": WORKFLOW_SPEC_KIND,
            "metadata": {
                "name": "phase6-mcp",
                "callerId": "codex-app",
                "labels": {}
            },
            "workflow": {
                "objective": "phase6 fan-out",
                "repository": {
                    "path": "D:/SYNTHETIC",
                    "expectedHead": "HEAD",
                    "dirtyPolicy": "reject"
                },
                "scheduling": {
                    "maxParallelWorkers": 3,
                    "maxWriterWorkersPerWorktree": 1
                },
                "budget": {
                    "maxWorkers": 3,
                    "timeoutSeconds": 120,
                    "maxLogBytesPerWorker": 1048576,
                    "maxArtifactBytes": 1048576
                },
                "fanIn": {
                    "policy": "allow-partial",
                    "minSuccessful": 2
                },
                "steps": [
                    {
                        "stepId": "step-01",
                        "objective": "one",
                        "dependsOn": [],
                        "access": "read",
                        "executor": { "cliTool": "codex" },
                        "timeoutSeconds": 120,
                        "required": true
                    },
                    {
                        "stepId": "step-02",
                        "objective": "two",
                        "dependsOn": [],
                        "access": "read",
                        "executor": { "cliTool": "codex" },
                        "timeoutSeconds": 120,
                        "required": true
                    },
                    {
                        "stepId": "step-03",
                        "objective": "three",
                        "dependsOn": [],
                        "access": "read",
                        "executor": { "cliTool": "codex" },
                        "timeoutSeconds": 120,
                        "required": true
                    }
                ]
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn workflow_create_args_for_repository(
    request_id: &str,
    name: &str,
    repository_path: &str,
    expected_head: &str,
    workspace_name: &str,
    step_count: u32,
    max_parallel_workers: u32,
) -> Value {
    let steps = (1..=step_count)
        .map(|index| {
            json!({
                "stepId": format!("step-{index:02}"),
                "objective": format!("inspect shard {index:02}"),
                "dependsOn": [],
                "access": "read",
                "executor": { "cliTool": "codex" },
                "timeoutSeconds": 120,
                "required": true
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schemaVersion": WORKFLOW_CREATE_SCHEMA_VERSION,
        "requestId": request_id,
        "spec": {
            "schemaVersion": WORKFLOW_SPEC_SCHEMA_VERSION,
            "kind": WORKFLOW_SPEC_KIND,
            "metadata": {
                "name": name,
                "callerId": "codex-app",
                "labels": {}
            },
            "workflow": {
                "objective": format!("phase7 capacity canary {name}"),
                "repository": {
                    "path": repository_path,
                    "expectedHead": expected_head,
                    "dirtyPolicy": "reject",
                    "workspaceName": workspace_name
                },
                "scheduling": {
                    "maxParallelWorkers": max_parallel_workers,
                    "maxWriterWorkersPerWorktree": 1
                },
                "budget": {
                    "maxWorkers": step_count,
                    "timeoutSeconds": 120,
                    "maxLogBytesPerWorker": 1048576,
                    "maxArtifactBytes": 1048576
                },
                "fanIn": {
                    "policy": "all-required"
                },
                "steps": steps
            }
        }
    })
}

fn workflow_start_args(request_id: &str, workflow_id: &str, expected_revision: Value) -> Value {
    json!({
        "schemaVersion": WORKFLOW_START_SCHEMA_VERSION,
        "requestId": request_id,
        "workflowId": workflow_id,
        "expectedRevision": expected_revision
    })
}

fn claim_dispatch_batch(
    runtime: &mut CoordinatorRuntime,
    expected: usize,
    now_ms: i64,
    workspace_name: &str,
) -> Vec<ClaimedLaunch> {
    let mut claims = Vec::new();
    for _ in 0..expected {
        let claimed = runtime
            .claim_next_dispatch(now_ms)
            .unwrap()
            .expect("claimed dispatch");
        let outbox = &claimed.outbox;
        assert_eq!(outbox.state, OutboxState::Sending);
        assert_eq!(outbox.payload["workspaceName"], workspace_name);
        let worker_id = outbox.worker_id.clone().expect("worker id");
        let worker = runtime.store().worker(&worker_id).unwrap();
        assert_eq!(worker.state, WorkerState::Dispatching);
        let binding = runtime
            .store()
            .external_binding_for_worker(&worker_id)
            .unwrap()
            .unwrap();
        assert_eq!(binding.state, ExternalBindingState::Pending);
        claims.push(ClaimedLaunch {
            worker_id,
            step_id: worker.step_id,
            outbox_id: outbox.outbox_id.clone(),
            outbox_revision: outbox.revision,
            outbox_generation: outbox.generation,
            binding_id: binding.binding_id,
            payload: outbox.payload.clone(),
        });
    }
    assert!(runtime.claim_next_dispatch(now_ms).unwrap().is_none());
    claims
}

#[test]
fn stdio_initialize_tools_list_and_tools_call_round_trip() {
    let (_dir, runtime) = runtime_fixture();
    let mut server = server(runtime);
    let input = [
        json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","id":"list","method":"tools/list","params":{}}),
        json!({
            "jsonrpc":"2.0",
            "id":"status",
            "method":"tools/call",
            "params":{
                "name":"coordinator_status",
                "arguments":{"schemaVersion": STATUS_QUERY_SCHEMA_VERSION}
            }
        }),
    ]
    .into_iter()
    .map(|value| value.to_string())
    .collect::<Vec<_>>()
    .join("\n")
        + "\n";
    let mut output = Vec::new();

    serve(&mut server, Cursor::new(input.into_bytes()), &mut output).unwrap();

    let lines = String::from_utf8(output).unwrap();
    let responses = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 3);
    assert_eq!(
        responses[0]["result"]["serverInfo"]["name"],
        "codex-ccpanes-mcp"
    );
    let tool_names = responses[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(tool_names.contains(&"worker_dispatch"));
    assert!(tool_names.contains(&"workflow_result"));
    assert!(tool_names.contains(&"ccpanes_preflight"));
    assert!(tool_names.contains(&"ccpanes_synthetic_canary"));
    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    let preflight = tools
        .iter()
        .find(|tool| tool["name"] == "ccpanes_preflight")
        .unwrap();
    let canary = tools
        .iter()
        .find(|tool| tool["name"] == "ccpanes_synthetic_canary")
        .unwrap();
    assert_eq!(preflight["annotations"]["readOnlyHint"], true);
    assert_eq!(canary["annotations"]["readOnlyHint"], false);
    assert_eq!(
        responses[2]["result"]["structuredContent"]["schemaVersion"],
        coordination_protocol::COORDINATOR_HEALTH_SCHEMA_VERSION
    );
}

#[test]
fn ccpanes_tools_route_exact_inputs_over_ipc_and_preserve_structured_response() {
    let structured = json!({
        "contractVersion": "fixture-response/v1",
        "status": {"ready": true}
    });
    let (mut server, envelopes) = recording_server(structured.clone());

    let preflight_result = call_tool(
        &mut server,
        "preflight",
        "ccpanes_preflight",
        json!({
            "schemaVersion": CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION,
            "discoveryPath": "C:/fixture/mcp-orchestrator.json",
            "discoveryGeneration": 7,
            "connectTimeoutMs": 2000,
            "requestTimeoutMs": 8000
        }),
    );
    assert_eq!(preflight_result, structured);

    let canary_result = call_tool(
        &mut server,
        "canary",
        "ccpanes_synthetic_canary",
        json!({
            "schemaVersion": CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION,
            "requestId": "global-canary-1",
            "discoveryPath": "C:/fixture/mcp-orchestrator.json",
            "projectPath": "C:/fixture/project",
            "workspaceName": "codex-ccpanes-phase7-v0123",
            "projectName": "compat-fixture",
            "marker": "CODEX_CCPANES_GLOBAL_CANARY",
            "timeoutMs": 420000,
            "pollIntervalMs": 750,
            "connectTimeoutMs": 2000,
            "requestTimeoutMs": 12000
        }),
    );
    assert_eq!(canary_result, structured);

    let envelopes = envelopes.borrow();
    assert_eq!(envelopes.len(), 2);
    assert_eq!(envelopes[0].request_id.as_str(), "ccpanes-preflight-1");
    assert_eq!(
        envelopes[0].message,
        IpcMessage::CcpanesPreflight(CcpanesPreflightInput {
            schema_version: CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION.to_owned(),
            discovery_path: Some("C:/fixture/mcp-orchestrator.json".to_owned()),
            discovery_generation: 7,
            connect_timeout_ms: 2_000,
            request_timeout_ms: 8_000,
        })
    );
    assert_eq!(envelopes[1].request_id.as_str(), "global-canary-1");
    assert_eq!(
        envelopes[1].message,
        IpcMessage::CcpanesSyntheticCanary(CcpanesSyntheticCanaryInput {
            schema_version: CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION.to_owned(),
            request_id: coordination_core::RequestId::try_new("global-canary-1").unwrap(),
            discovery_path: Some("C:/fixture/mcp-orchestrator.json".to_owned()),
            project_path: "C:/fixture/project".to_owned(),
            workspace_name: "codex-ccpanes-phase7-v0123".to_owned(),
            project_name: "compat-fixture".to_owned(),
            marker: "CODEX_CCPANES_GLOBAL_CANARY".to_owned(),
            timeout_ms: 420_000,
            poll_interval_ms: 750,
            connect_timeout_ms: 2_000,
            request_timeout_ms: 12_000,
        })
    );
}

#[test]
fn worker_dispatch_status_and_result_are_routed_through_coordinator_ipc() {
    let (_dir, runtime) = runtime_fixture();
    let mut server = server(runtime);

    let accepted = call_tool(
        &mut server,
        "dispatch",
        "worker_dispatch",
        dispatch_args("dispatch-1"),
    );
    let worker_id = accepted["workerId"].as_str().unwrap();
    let workflow_id = accepted["workflowId"].as_str().unwrap();
    assert_eq!(accepted["state"], "QUEUED");

    let status = call_tool(
        &mut server,
        "worker-status",
        "worker_status",
        worker_status_args(worker_id),
    );
    assert_eq!(status["workerId"], worker_id);
    assert_eq!(status["state"], "QUEUED");
    assert_eq!(status["externalBinding"]["state"], "PENDING");

    let result = call_tool(
        &mut server,
        "workflow-result",
        "workflow_result",
        workflow_result_args(workflow_id),
    );
    assert_eq!(result["state"], "pending");
    assert_eq!(result["workers"].as_array().unwrap().len(), 0);
}

#[test]
fn worker_cancel_transitions_queued_worker_to_cancelled() {
    let (_dir, runtime) = runtime_fixture();
    let mut server = server(runtime);
    let accepted = call_tool(
        &mut server,
        "dispatch",
        "worker_dispatch",
        dispatch_args("dispatch-cancel"),
    );
    let worker_id = accepted["workerId"].as_str().unwrap();

    let cancel = call_tool(
        &mut server,
        "cancel",
        "worker_cancel",
        json!({
            "schemaVersion": WORKER_CANCEL_SCHEMA_VERSION,
            "requestId": "cancel-1",
            "workerId": worker_id,
            "expectedRevision": accepted["revision"],
            "reason": "synthetic cancel"
        }),
    );
    assert_eq!(cancel["accepted"], true);
    assert_eq!(cancel["state"], "CANCELLED");

    let status = call_tool(
        &mut server,
        "worker-status",
        "worker_status",
        worker_status_args(worker_id),
    );
    assert_eq!(status["state"], "CANCELLED");
}

#[test]
fn request_replay_creates_one_worker() {
    let (_dir, runtime) = runtime_fixture();
    let mut server = server(runtime.clone());

    let first = call_tool(
        &mut server,
        "dispatch-1",
        "worker_dispatch",
        dispatch_args("replay-1"),
    );
    let second = call_tool(
        &mut server,
        "dispatch-2",
        "worker_dispatch",
        dispatch_args("replay-1"),
    );

    assert_eq!(first["workflowId"], second["workflowId"]);
    assert_eq!(first["workerId"], second["workerId"]);
    let workflow_id = WorkflowId::try_new(first["workflowId"].as_str().unwrap()).unwrap();
    assert_eq!(
        runtime
            .borrow()
            .store()
            .workers_for_workflow(&workflow_id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn workflow_create_start_fans_out_multiple_steps_through_mcp() {
    let (_dir, runtime) = runtime_fixture();
    let mut server = server(runtime);

    let created = call_tool(
        &mut server,
        "workflow-create",
        "workflow_create",
        workflow_create_args("workflow-create-1"),
    );
    let workflow_id = created["workflowId"].as_str().unwrap();
    assert_eq!(created["state"], "DRAFT");

    let started = call_tool(
        &mut server,
        "workflow-start",
        "workflow_start",
        json!({
            "schemaVersion": WORKFLOW_START_SCHEMA_VERSION,
            "requestId": "workflow-start-1",
            "workflowId": workflow_id,
            "expectedRevision": created["revision"]
        }),
    );
    assert_eq!(started["state"], "RUNNING");

    let status = call_tool(
        &mut server,
        "workflow-status",
        "workflow_status",
        workflow_status_args(workflow_id),
    );
    assert_eq!(status["workers"].as_array().unwrap().len(), 3);
    assert!(status["workers"]
        .as_array()
        .unwrap()
        .iter()
        .all(|worker| worker["state"] == "QUEUED"));
}

#[test]
fn mcp_restart_queries_same_workflow_from_coordinator_state() {
    let (_dir, runtime) = runtime_fixture();
    let (workflow_id, worker_id) = {
        let mut first_server = server(runtime.clone());
        let accepted = call_tool(
            &mut first_server,
            "dispatch",
            "worker_dispatch",
            dispatch_args("restart-1"),
        );
        (
            accepted["workflowId"].as_str().unwrap().to_owned(),
            accepted["workerId"].as_str().unwrap().to_owned(),
        )
    };

    let mut restarted_server = server(runtime);
    let workflow = call_tool(
        &mut restarted_server,
        "workflow-status",
        "workflow_status",
        workflow_status_args(&workflow_id),
    );
    assert_eq!(workflow["workflowId"], workflow_id);
    assert_eq!(workflow["state"], "RUNNING");
    assert_eq!(workflow["workers"][0]["workerId"], worker_id);
}

#[test]
fn unknown_tool_input_schema_version_returns_public_error() {
    let (_dir, runtime) = runtime_fixture();
    let mut server = server(runtime);
    let mut arguments = dispatch_args("bad-schema");
    arguments["schemaVersion"] = json!("coordination.worker-dispatch/v9");

    let error = call_tool_error(&mut server, "bad-schema", "worker_dispatch", arguments);

    assert_eq!(error["code"], "schema_version_unsupported");
    assert_eq!(error["retryable"], false);
}

#[test]
fn fake_ccpanes_complete_process_e2e_returns_aggregated_result() {
    let (_dir, runtime) = runtime_fixture();
    let mut server = server(runtime.clone());
    let accepted = call_tool(
        &mut server,
        "dispatch",
        "worker_dispatch",
        dispatch_args("fake-e2e"),
    );
    let worker_id = WorkerId::try_new(accepted["workerId"].as_str().unwrap()).unwrap();
    let workflow_id = WorkflowId::try_new(accepted["workflowId"].as_str().unwrap()).unwrap();

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
    let (outbox_id, outbox_revision, payload) = {
        let mut runtime = runtime.borrow_mut();
        runtime.set_dispatch_ready(true);
        let plan = runtime.tick(0).unwrap();
        assert_eq!(plan.dispatches.len(), 1);
        let claimed = runtime.claim_next_dispatch(0).unwrap().unwrap();
        (
            claimed.outbox.outbox_id,
            claimed.outbox.revision,
            claimed.outbox.payload,
        )
    };

    let launched = adapter.launch_task(payload, &capability).unwrap();
    assert_eq!(launched.session_id.as_deref(), Some("session-1"));
    let snapshot = adapter.reconcile_session("session-1").unwrap();
    assert!(snapshot.terminal);
    assert!(snapshot.succeeded);
    let output = snapshot.output.unwrap();
    assert!(output.contains("synthetic output"));
    assert_eq!(
        fake.records()
            .iter()
            .filter(|record| record.tool_name.as_deref() == Some("launch_task"))
            .count(),
        1
    );

    let artifact_sha256 = "f".repeat(64);
    {
        let mut runtime = runtime.borrow_mut();
        runtime
            .store_mut()
            .transition_worker_state(
                &worker_id,
                Revision::new(2),
                WorkerState::Running,
                "running",
            )
            .unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &worker_id,
                Revision::new(3),
                WorkerState::Succeeded,
                "succeeded",
            )
            .unwrap();
        runtime
            .store_mut()
            .transition_outbox_state(
                &outbox_id,
                outbox_revision,
                coordination_core::OutboxState::Acknowledged,
                None,
                "ack",
            )
            .unwrap();
        runtime
            .store_mut()
            .record_worker_result(RecordWorkerResultRequest {
                workflow_id: workflow_id.clone(),
                worker_id: worker_id.clone(),
                worker_state: WorkerState::Succeeded,
                aggregation_state: ResultAggregationState::Complete,
                summary: output,
                artifact_sha256: Some(artifact_sha256.clone()),
                truncated: false,
                completed_at_ms: 1,
                retention_until_ms: None,
            })
            .unwrap();
        let workflow = runtime.store().workflow(&workflow_id).unwrap();
        runtime
            .store_mut()
            .transition_workflow_state(
                &workflow_id,
                workflow.revision,
                WorkflowState::Succeeded,
                "workflow-succeeded",
            )
            .unwrap();
    }

    let result = call_tool(
        &mut server,
        "workflow-result",
        "workflow_result",
        workflow_result_args(workflow_id.as_str()),
    );
    assert_eq!(result["state"], "complete");
    assert_eq!(result["workers"][0]["workerId"], worker_id.as_str());
    assert_eq!(result["workers"][0]["artifactSha256"], artifact_sha256);
}

#[test]
fn phase7_single_worker_success_canary_uses_dedicated_repo_workspace_and_records_result() {
    let (dir, runtime) = runtime_fixture();
    let repository_registration =
        prepare_synthetic_git_repository(dir.path(), "codex-ccpanes-phase7-success").unwrap();
    let workspace_name =
        dedicated_workspace_name_for_project_path(&repository_registration.repository.path);
    let project_name = "phase7-success-project";
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        complete_launches: true,
        workspaces: vec![json!({"id": "workspace-secret-id", "name": workspace_name})],
        projects: vec![json!({
            "id": "project-secret-id",
            "name": project_name,
            "path": repository_registration.repository.path.clone(),
            "workspaceName": workspace_name
        })],
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(fake.endpoint(), Vec::new(), 1).unwrap(),
        AdapterTimeouts::default(),
    )
    .unwrap();
    let workspace_report = run_dedicated_workspace_project_gate(
        &mut adapter,
        DedicatedWorkspaceSpec {
            workspace_name: workspace_name.clone(),
            project_path: repository_registration.repository.path.clone(),
            project_name: project_name.to_owned(),
        },
    );
    assert_eq!(
        workspace_report.readiness,
        DedicatedWorkspaceReadiness::Ready
    );
    assert!(workspace_report.launch_task_accepts_workspace_name);
    assert!(!workspace_report.external_workspace_side_effects);
    assert_eq!(
        fake.records()
            .iter()
            .filter(|record| {
                matches!(
                    record.tool_name.as_deref(),
                    Some(TOOL_CREATE_WORKSPACE | TOOL_ADD_PROJECT_TO_WORKSPACE | TOOL_LAUNCH_TASK)
                )
            })
            .count(),
        0
    );

    let capability = adapter.discover().unwrap();
    let mut server = server(runtime.clone());
    let accepted = call_tool(
        &mut server,
        "phase7-dispatch",
        "worker_dispatch",
        dispatch_args_for_repository(
            "phase7-success",
            &repository_registration.repository.path,
            repository_registration
                .repository
                .expected_head
                .as_deref()
                .unwrap(),
            &workspace_name,
        ),
    );
    let worker_id = WorkerId::try_new(accepted["workerId"].as_str().unwrap()).unwrap();
    let workflow_id = WorkflowId::try_new(accepted["workflowId"].as_str().unwrap()).unwrap();

    let (outbox_id, outbox_revision, outbox_generation, payload, binding_id) = {
        let mut runtime = runtime.borrow_mut();
        runtime.set_dispatch_ready(true);
        let plan = runtime.tick(0).unwrap();
        assert_eq!(plan.dispatches.len(), 1);
        let claimed = runtime.claim_next_dispatch(0).unwrap().unwrap();
        let binding = runtime
            .store()
            .external_binding_for_worker(&worker_id)
            .unwrap()
            .unwrap();
        (
            claimed.outbox.outbox_id,
            claimed.outbox.revision,
            claimed.outbox.generation,
            claimed.outbox.payload,
            binding.binding_id,
        )
    };
    assert_eq!(payload["workspaceName"], workspace_name);

    let launched = adapter.launch_task(payload, &capability).unwrap();
    assert_eq!(launched.session_id.as_deref(), Some("session-1"));
    assert_eq!(
        fake.last_tool_arguments(TOOL_LAUNCH_TASK).unwrap()["workspaceName"],
        workspace_name
    );
    let session_id = launched.session_id.as_deref().unwrap();
    let snapshot = adapter.reconcile_session(session_id).unwrap();
    assert!(snapshot.terminal);
    assert!(snapshot.succeeded);
    let output = snapshot.output.unwrap();
    assert!(output.contains("synthetic output"));

    let artifact_sha256 = "7".repeat(64);
    {
        let mut runtime = runtime.borrow_mut();
        runtime
            .store_mut()
            .record_external_launch_success(RecordExternalLaunchSuccessRequest {
                binding_id,
                expected_generation: outbox_generation,
                external_task_ref: launched.task_id.clone(),
                external_session_ref: session_id.to_owned(),
                correlation_id: "phase7-success-launch-record".to_owned(),
            })
            .unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &worker_id,
                Revision::new(2),
                WorkerState::Running,
                "phase7-success-running",
            )
            .unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &worker_id,
                Revision::new(3),
                WorkerState::Succeeded,
                "phase7-success-succeeded",
            )
            .unwrap();
        runtime
            .store_mut()
            .transition_outbox_state(
                &outbox_id,
                outbox_revision,
                OutboxState::Acknowledged,
                None,
                "phase7-success-ack",
            )
            .unwrap();
        runtime
            .store_mut()
            .record_worker_result(RecordWorkerResultRequest {
                workflow_id: workflow_id.clone(),
                worker_id: worker_id.clone(),
                worker_state: WorkerState::Succeeded,
                aggregation_state: ResultAggregationState::Complete,
                summary: output,
                artifact_sha256: Some(artifact_sha256.clone()),
                truncated: false,
                completed_at_ms: 1,
                retention_until_ms: None,
            })
            .unwrap();
        runtime.advance_workflow(&workflow_id, 1).unwrap();
    }

    let status = call_tool(
        &mut server,
        "phase7-worker-status",
        "worker_status",
        worker_status_args(worker_id.as_str()),
    );
    assert_eq!(status["state"], "SUCCEEDED");
    assert_eq!(status["externalBinding"]["state"], "ACTIVE");
    assert_eq!(status["externalBinding"]["hasExternalSession"], true);

    let result = call_tool(
        &mut server,
        "phase7-workflow-result",
        "workflow_result",
        workflow_result_args(workflow_id.as_str()),
    );
    assert_eq!(result["state"], "complete");
    assert_eq!(result["workers"][0]["workerId"], worker_id.as_str());
    assert_eq!(result["workers"][0]["artifactSha256"], artifact_sha256);

    let records = fake.records();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.tool_name.as_deref() == Some(TOOL_LAUNCH_TASK))
            .count(),
        1
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| {
                matches!(
                    record.tool_name.as_deref(),
                    Some(
                        TOOL_CREATE_WORKSPACE
                            | TOOL_ADD_PROJECT_TO_WORKSPACE
                            | TOOL_SUBMIT_TO_SESSION
                            | TOOL_KILL_SESSION
                    )
                )
            })
            .count(),
        0
    );
}

#[test]
fn phase7_cancellation_canary_uses_dedicated_repo_workspace_and_kills_session_once() {
    let (dir, runtime) = runtime_fixture();
    let repository_registration =
        prepare_synthetic_git_repository(dir.path(), "codex-ccpanes-phase7-cancel").unwrap();
    let workspace_name =
        dedicated_workspace_name_for_project_path(&repository_registration.repository.path);
    let project_name = "phase7-cancel-project";
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        workspaces: vec![json!({"id": "workspace-secret-id", "name": workspace_name})],
        projects: vec![json!({
            "id": "project-secret-id",
            "name": project_name,
            "path": repository_registration.repository.path.clone(),
            "workspaceName": workspace_name
        })],
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(fake.endpoint(), Vec::new(), 1).unwrap(),
        AdapterTimeouts::default(),
    )
    .unwrap();
    let workspace_report = run_dedicated_workspace_project_gate(
        &mut adapter,
        DedicatedWorkspaceSpec {
            workspace_name: workspace_name.clone(),
            project_path: repository_registration.repository.path.clone(),
            project_name: project_name.to_owned(),
        },
    );
    assert_eq!(
        workspace_report.readiness,
        DedicatedWorkspaceReadiness::Ready
    );
    assert!(!workspace_report.external_workspace_side_effects);

    let capability = adapter.discover().unwrap();
    let mut server = server(runtime.clone());
    let accepted = call_tool(
        &mut server,
        "phase7-cancel-dispatch",
        "worker_dispatch",
        dispatch_args_for_repository(
            "phase7-cancel",
            &repository_registration.repository.path,
            repository_registration
                .repository
                .expected_head
                .as_deref()
                .unwrap(),
            &workspace_name,
        ),
    );
    let worker_id = WorkerId::try_new(accepted["workerId"].as_str().unwrap()).unwrap();
    let workflow_id = WorkflowId::try_new(accepted["workflowId"].as_str().unwrap()).unwrap();

    let (launch_outbox_id, launch_outbox_revision, launch_generation, launch_payload, binding_id) = {
        let mut runtime = runtime.borrow_mut();
        runtime.set_dispatch_ready(true);
        let plan = runtime.tick(0).unwrap();
        assert_eq!(plan.dispatches.len(), 1);
        let claimed = runtime.claim_next_dispatch(0).unwrap().unwrap();
        let binding = runtime
            .store()
            .external_binding_for_worker(&worker_id)
            .unwrap()
            .unwrap();
        (
            claimed.outbox.outbox_id,
            claimed.outbox.revision,
            claimed.outbox.generation,
            claimed.outbox.payload,
            binding.binding_id,
        )
    };
    assert_eq!(launch_payload["workspaceName"], workspace_name);

    let launched = adapter.launch_task(launch_payload, &capability).unwrap();
    let session_id = launched.session_id.as_deref().unwrap();
    assert_eq!(session_id, "session-1");
    assert_eq!(
        fake.last_tool_arguments(TOOL_LAUNCH_TASK).unwrap()["workspaceName"],
        workspace_name
    );
    {
        let mut runtime = runtime.borrow_mut();
        runtime
            .store_mut()
            .record_external_launch_success(RecordExternalLaunchSuccessRequest {
                binding_id: binding_id.clone(),
                expected_generation: launch_generation,
                external_task_ref: launched.task_id.clone(),
                external_session_ref: session_id.to_owned(),
                correlation_id: "phase7-cancel-launch-record".to_owned(),
            })
            .unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &worker_id,
                Revision::new(2),
                WorkerState::Running,
                "phase7-cancel-running",
            )
            .unwrap();
        runtime
            .store_mut()
            .transition_outbox_state(
                &launch_outbox_id,
                launch_outbox_revision,
                OutboxState::Acknowledged,
                None,
                "phase7-cancel-launch-ack",
            )
            .unwrap();
    }

    let workflow_status = call_tool(
        &mut server,
        "phase7-cancel-workflow-status-before",
        "workflow_status",
        workflow_status_args(workflow_id.as_str()),
    );
    assert_eq!(workflow_status["state"], "RUNNING");
    let cancel = call_tool(
        &mut server,
        "phase7-workflow-cancel",
        "workflow_cancel",
        json!({
            "schemaVersion": WORKFLOW_CANCEL_SCHEMA_VERSION,
            "requestId": "phase7-cancel-request",
            "workflowId": workflow_id.as_str(),
            "expectedRevision": workflow_status["revision"],
            "reason": "phase7 synthetic cancellation canary"
        }),
    );
    assert_eq!(cancel["accepted"], true);
    assert_eq!(cancel["state"], "CANCELLING");

    let duplicate_worker_cancel = call_tool(
        &mut server,
        "phase7-duplicate-worker-cancel",
        "worker_cancel",
        json!({
            "schemaVersion": WORKER_CANCEL_SCHEMA_VERSION,
            "requestId": "phase7-cancel-request-replay",
            "workerId": worker_id.as_str(),
            "expectedRevision": Revision::new(3),
            "reason": "phase7 duplicate cancel must not emit another stop"
        }),
    );
    assert_eq!(duplicate_worker_cancel["state"], "CANCELLING");

    let status = call_tool(
        &mut server,
        "phase7-cancel-worker-status",
        "worker_status",
        worker_status_args(worker_id.as_str()),
    );
    assert_eq!(status["state"], "CANCELLING");
    assert_eq!(status["externalBinding"]["state"], "CANCEL_REQUESTED");
    assert_eq!(status["externalBinding"]["hasExternalSession"], true);

    let claimed_control = {
        let mut runtime = runtime.borrow_mut();
        assert_eq!(
            runtime
                .store()
                .due_outbox(0, 10)
                .unwrap()
                .iter()
                .filter(|outbox| outbox.command_type == "kill_session")
                .count(),
            1
        );
        runtime.claim_next_control(0).unwrap().unwrap().outbox
    };
    assert_eq!(claimed_control.command_type, "kill_session");
    assert_eq!(claimed_control.payload["sessionId"], session_id);
    assert_eq!(
        claimed_control.payload["reason"],
        "phase7 synthetic cancellation canary"
    );

    let kill_outcome = adapter
        .kill_session(
            session_id,
            claimed_control.payload["reason"].as_str(),
            capability.kill_session_accepts_reason,
        )
        .unwrap();
    assert_eq!(format!("{kill_outcome:?}"), "Accepted");
    let snapshot = adapter.reconcile_session(session_id).unwrap();
    assert!(snapshot.terminal);
    assert!(!snapshot.succeeded);
    assert_eq!(snapshot.status, "cancelled");
    assert!(snapshot.output.unwrap().contains("synthetic output"));
    assert_eq!(
        fake.records()
            .iter()
            .filter(|record| record.tool_name.as_deref() == Some(TOOL_KILL_SESSION))
            .count(),
        1
    );
    assert_eq!(
        fake.last_tool_arguments(TOOL_KILL_SESSION).unwrap()["sessionId"],
        session_id
    );

    {
        let mut runtime = runtime.borrow_mut();
        runtime
            .store_mut()
            .transition_outbox_state(
                &claimed_control.outbox_id,
                claimed_control.revision,
                OutboxState::Acknowledged,
                None,
                "phase7-cancel-kill-ack",
            )
            .unwrap();
        runtime
            .store_mut()
            .record_external_cancel_success(RecordExternalCancelSuccessRequest {
                binding_id,
                correlation_id: "phase7-cancel-kill-record".to_owned(),
            })
            .unwrap();
        let current_worker = runtime.store().worker(&worker_id).unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &worker_id,
                current_worker.revision,
                WorkerState::Cancelled,
                "phase7-cancel-worker-terminal",
            )
            .unwrap();
        runtime
            .store_mut()
            .record_worker_result(RecordWorkerResultRequest {
                workflow_id: workflow_id.clone(),
                worker_id: worker_id.clone(),
                worker_state: WorkerState::Cancelled,
                aggregation_state: ResultAggregationState::Partial,
                summary: "synthetic worker cancelled after fake kill_session".to_owned(),
                artifact_sha256: None,
                truncated: false,
                completed_at_ms: 1,
                retention_until_ms: None,
            })
            .unwrap();
        runtime.advance_workflow(&workflow_id, 1).unwrap();
    }

    let workflow_status = call_tool(
        &mut server,
        "phase7-cancel-workflow-status-after",
        "workflow_status",
        workflow_status_args(workflow_id.as_str()),
    );
    assert_eq!(workflow_status["state"], "CANCELLED");
    let result = call_tool(
        &mut server,
        "phase7-cancel-workflow-result",
        "workflow_result",
        workflow_result_args(workflow_id.as_str()),
    );
    assert_eq!(result["state"], "partial");
    assert_eq!(result["workers"][0]["workerId"], worker_id.as_str());
    assert_eq!(result["workers"][0]["state"], "CANCELLED");
    assert_eq!(result["failedWorkers"][0], worker_id.as_str());

    let records = fake.records();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.tool_name.as_deref() == Some(TOOL_LAUNCH_TASK))
            .count(),
        1
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| {
                matches!(
                    record.tool_name.as_deref(),
                    Some(
                        TOOL_CREATE_WORKSPACE
                            | TOOL_ADD_PROJECT_TO_WORKSPACE
                            | TOOL_SUBMIT_TO_SESSION
                    )
                )
            })
            .count(),
        0
    );
}

#[test]
fn phase7_coordinator_restart_canary_reopens_store_and_finishes_fake_worker_once() {
    let dir = tempdir().unwrap();
    let database_path = dir.path().join("coordination.db");
    let runtime = Rc::new(RefCell::new(
        CoordinatorRuntime::open(
            &database_path,
            dir.path(),
            "phase7-restart-ipc",
            CoordinatorConfig::default(),
        )
        .unwrap(),
    ));
    let repository_registration =
        prepare_synthetic_git_repository(dir.path(), "codex-ccpanes-phase7-restart").unwrap();
    let workspace_name =
        dedicated_workspace_name_for_project_path(&repository_registration.repository.path);
    let project_name = "phase7-restart-project";
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        complete_launches: true,
        workspaces: vec![json!({"id": "workspace-secret-id", "name": workspace_name})],
        projects: vec![json!({
            "id": "project-secret-id",
            "name": project_name,
            "path": repository_registration.repository.path.clone(),
            "workspaceName": workspace_name
        })],
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(fake.endpoint(), Vec::new(), 1).unwrap(),
        AdapterTimeouts::default(),
    )
    .unwrap();
    let workspace_report = run_dedicated_workspace_project_gate(
        &mut adapter,
        DedicatedWorkspaceSpec {
            workspace_name: workspace_name.clone(),
            project_path: repository_registration.repository.path.clone(),
            project_name: project_name.to_owned(),
        },
    );
    assert_eq!(
        workspace_report.readiness,
        DedicatedWorkspaceReadiness::Ready
    );
    assert!(!workspace_report.external_workspace_side_effects);
    let capability = adapter.discover().unwrap();

    let mut first_server = server(runtime.clone());
    let accepted = call_tool(
        &mut first_server,
        "phase7-restart-dispatch",
        "worker_dispatch",
        dispatch_args_for_repository(
            "phase7-restart",
            &repository_registration.repository.path,
            repository_registration
                .repository
                .expected_head
                .as_deref()
                .unwrap(),
            &workspace_name,
        ),
    );
    let worker_id = WorkerId::try_new(accepted["workerId"].as_str().unwrap()).unwrap();
    let workflow_id = WorkflowId::try_new(accepted["workflowId"].as_str().unwrap()).unwrap();
    let (outbox_id, outbox_revision, outbox_generation, payload, binding_id) = {
        let mut runtime = runtime.borrow_mut();
        runtime.set_dispatch_ready(true);
        let plan = runtime.tick(0).unwrap();
        assert_eq!(plan.dispatches.len(), 1);
        let claimed = runtime.claim_next_dispatch(0).unwrap().unwrap();
        let binding = runtime
            .store()
            .external_binding_for_worker(&worker_id)
            .unwrap()
            .unwrap();
        (
            claimed.outbox.outbox_id,
            claimed.outbox.revision,
            claimed.outbox.generation,
            claimed.outbox.payload,
            binding.binding_id,
        )
    };
    assert_eq!(payload["workspaceName"], workspace_name);
    let launched = adapter.launch_task(payload, &capability).unwrap();
    let session_id = launched.session_id.as_deref().unwrap().to_owned();
    {
        let mut runtime = runtime.borrow_mut();
        runtime
            .store_mut()
            .record_external_launch_success(RecordExternalLaunchSuccessRequest {
                binding_id: binding_id.clone(),
                expected_generation: outbox_generation,
                external_task_ref: launched.task_id.clone(),
                external_session_ref: session_id.clone(),
                correlation_id: "phase7-restart-launch-record".to_owned(),
            })
            .unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &worker_id,
                Revision::new(2),
                WorkerState::Running,
                "phase7-restart-running",
            )
            .unwrap();
        runtime
            .store_mut()
            .transition_outbox_state(
                &outbox_id,
                outbox_revision,
                OutboxState::Acknowledged,
                None,
                "phase7-restart-launch-ack",
            )
            .unwrap();
    }
    drop(first_server);
    drop(runtime);

    let restarted_runtime = Rc::new(RefCell::new(
        CoordinatorRuntime::open(
            &database_path,
            dir.path(),
            "phase7-restart-ipc",
            CoordinatorConfig::default(),
        )
        .unwrap(),
    ));
    let mut restarted_server = server(restarted_runtime.clone());
    let worker_status = call_tool(
        &mut restarted_server,
        "phase7-restart-worker-status",
        "worker_status",
        worker_status_args(worker_id.as_str()),
    );
    assert_eq!(worker_status["state"], "RUNNING");
    assert_eq!(worker_status["externalBinding"]["state"], "ACTIVE");
    assert_eq!(worker_status["externalBinding"]["hasExternalSession"], true);
    let workflow_status = call_tool(
        &mut restarted_server,
        "phase7-restart-workflow-status",
        "workflow_status",
        workflow_status_args(workflow_id.as_str()),
    );
    assert_eq!(workflow_status["state"], "RUNNING");
    {
        let mut runtime = restarted_runtime.borrow_mut();
        runtime.set_dispatch_ready(true);
        let plan = runtime.tick(1).unwrap();
        assert_eq!(plan.dispatches.len(), 0);
    }

    let snapshot = adapter.reconcile_session(&session_id).unwrap();
    assert!(snapshot.terminal);
    assert!(snapshot.succeeded);
    let output = snapshot.output.unwrap();
    let artifact_sha256 = "8".repeat(64);
    {
        let mut runtime = restarted_runtime.borrow_mut();
        let current_worker = runtime.store().worker(&worker_id).unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &worker_id,
                current_worker.revision,
                WorkerState::Succeeded,
                "phase7-restart-succeeded",
            )
            .unwrap();
        runtime
            .store_mut()
            .record_worker_result(RecordWorkerResultRequest {
                workflow_id: workflow_id.clone(),
                worker_id: worker_id.clone(),
                worker_state: WorkerState::Succeeded,
                aggregation_state: ResultAggregationState::Complete,
                summary: output,
                artifact_sha256: Some(artifact_sha256.clone()),
                truncated: false,
                completed_at_ms: 1,
                retention_until_ms: None,
            })
            .unwrap();
        runtime.advance_workflow(&workflow_id, 1).unwrap();
    }
    let result = call_tool(
        &mut restarted_server,
        "phase7-restart-workflow-result",
        "workflow_result",
        workflow_result_args(workflow_id.as_str()),
    );
    assert_eq!(result["state"], "complete");
    assert_eq!(result["workers"][0]["workerId"], worker_id.as_str());
    assert_eq!(result["workers"][0]["artifactSha256"], artifact_sha256);

    let records = fake.records();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.tool_name.as_deref() == Some(TOOL_LAUNCH_TASK))
            .count(),
        1
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| {
                matches!(
                    record.tool_name.as_deref(),
                    Some(
                        TOOL_CREATE_WORKSPACE
                            | TOOL_ADD_PROJECT_TO_WORKSPACE
                            | TOOL_SUBMIT_TO_SESSION
                            | TOOL_KILL_SESSION
                    )
                )
            })
            .count(),
        0
    );
}

#[test]
fn phase7_unknown_dispatch_canary_reconciles_dropped_launch_response_once() {
    let dir = tempdir().unwrap();
    let database_path = dir.path().join("coordination.db");
    let runtime = Rc::new(RefCell::new(
        CoordinatorRuntime::open(
            &database_path,
            dir.path(),
            "phase7-unknown-ipc",
            CoordinatorConfig::default(),
        )
        .unwrap(),
    ));
    let repository_registration =
        prepare_synthetic_git_repository(dir.path(), "codex-ccpanes-phase7-unknown").unwrap();
    let workspace_name =
        dedicated_workspace_name_for_project_path(&repository_registration.repository.path);
    let project_name = "phase7-unknown-project";
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        drop_launch_response_after_side_effect: true,
        complete_launches: true,
        workspaces: vec![json!({"id": "workspace-secret-id", "name": workspace_name})],
        projects: vec![json!({
            "id": "project-secret-id",
            "name": project_name,
            "path": repository_registration.repository.path.clone(),
            "workspaceName": workspace_name
        })],
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(fake.endpoint(), Vec::new(), 1).unwrap(),
        AdapterTimeouts::new(
            std::time::Duration::from_millis(100),
            std::time::Duration::from_millis(150),
        ),
    )
    .unwrap();
    let workspace_report = run_dedicated_workspace_project_gate(
        &mut adapter,
        DedicatedWorkspaceSpec {
            workspace_name: workspace_name.clone(),
            project_path: repository_registration.repository.path.clone(),
            project_name: project_name.to_owned(),
        },
    );
    assert_eq!(
        workspace_report.readiness,
        DedicatedWorkspaceReadiness::Ready
    );
    assert!(!workspace_report.external_workspace_side_effects);
    let capability = adapter.discover().unwrap();

    let mut first_server = server(runtime.clone());
    let accepted = call_tool(
        &mut first_server,
        "phase7-unknown-dispatch",
        "worker_dispatch",
        dispatch_args_for_repository(
            "phase7-unknown",
            &repository_registration.repository.path,
            repository_registration
                .repository
                .expected_head
                .as_deref()
                .unwrap(),
            &workspace_name,
        ),
    );
    let worker_id = WorkerId::try_new(accepted["workerId"].as_str().unwrap()).unwrap();
    let workflow_id = WorkflowId::try_new(accepted["workflowId"].as_str().unwrap()).unwrap();
    let (outbox_id, launch_payload, binding_id) = {
        let mut runtime = runtime.borrow_mut();
        runtime.set_dispatch_ready(true);
        let plan = runtime.tick(0).unwrap();
        assert_eq!(plan.dispatches.len(), 1);
        let claimed = runtime.claim_next_dispatch(0).unwrap().unwrap();
        let binding = runtime
            .store()
            .external_binding_for_worker(&worker_id)
            .unwrap()
            .unwrap();
        assert_eq!(claimed.outbox.state, OutboxState::Sending);
        assert_eq!(binding.state, ExternalBindingState::Pending);
        (
            claimed.outbox.outbox_id,
            claimed.outbox.payload,
            binding.binding_id,
        )
    };
    assert_eq!(launch_payload["workspaceName"], workspace_name);

    let launch_error = adapter
        .launch_task(launch_payload.clone(), &capability)
        .expect_err("dropped launch response must be unknown");
    assert_eq!(launch_error.code, ErrorCode::DispatchResultUnknown);
    assert_eq!(
        fake.last_tool_arguments(TOOL_LAUNCH_TASK).unwrap()["workspaceName"],
        workspace_name
    );
    {
        let runtime = runtime.borrow();
        assert_eq!(
            runtime.store().outbox(&outbox_id).unwrap().state,
            OutboxState::Sending
        );
        assert_eq!(
            runtime.store().worker(&worker_id).unwrap().state,
            WorkerState::Dispatching
        );
        let binding = runtime.store().external_binding(&binding_id).unwrap();
        assert_eq!(binding.state, ExternalBindingState::Pending);
        assert!(binding.external_session_ref.is_none());
    }
    drop(first_server);
    drop(runtime);

    let restarted_runtime = Rc::new(RefCell::new(
        CoordinatorRuntime::open(
            &database_path,
            dir.path(),
            "phase7-unknown-ipc",
            CoordinatorConfig::default(),
        )
        .unwrap(),
    ));
    let mut restarted_server = server(restarted_runtime.clone());
    let unknown_status = call_tool(
        &mut restarted_server,
        "phase7-unknown-worker-status-before-reconcile",
        "worker_status",
        worker_status_args(worker_id.as_str()),
    );
    assert_eq!(unknown_status["state"], "DISPATCH_UNKNOWN");
    assert_eq!(unknown_status["externalBinding"]["state"], "RESULT_UNKNOWN");
    assert_eq!(
        unknown_status["externalBinding"]["hasExternalSession"],
        false
    );
    assert_eq!(unknown_status["externalBinding"]["generation"], 2);
    {
        let mut runtime = restarted_runtime.borrow_mut();
        runtime.set_dispatch_ready(true);
        let plan = runtime.tick(1).unwrap();
        assert_eq!(plan.dispatches.len(), 0);
        assert_eq!(
            runtime.store().outbox(&outbox_id).unwrap().state,
            OutboxState::ResultUnknown
        );
    }

    let recovered_binding = restarted_runtime
        .borrow()
        .store()
        .external_binding(&binding_id)
        .unwrap();
    assert_eq!(recovered_binding.state, ExternalBindingState::ResultUnknown);
    let discovered = adapter
        .find_unknown_dispatch(
            launch_payload["clientRequestId"].as_str().unwrap(),
            launch_payload["title"].as_str().unwrap(),
            &recovered_binding.prompt_sha256,
            recovered_binding.created_at_ms,
        )
        .unwrap()
        .unwrap();
    assert_eq!(discovered.session_id, "session-1");
    assert_eq!(discovered.task_id.as_deref(), Some("task-1"));
    assert_eq!(discovered.match_basis, "clientRequestId");

    let snapshot = adapter.reconcile_session(&discovered.session_id).unwrap();
    assert!(snapshot.terminal);
    assert!(snapshot.succeeded);
    let output = snapshot.output.unwrap();
    let artifact_sha256 = "9".repeat(64);
    {
        let mut runtime = restarted_runtime.borrow_mut();
        let current_binding = runtime.store().external_binding(&binding_id).unwrap();
        runtime
            .store_mut()
            .record_external_launch_success(RecordExternalLaunchSuccessRequest {
                binding_id: binding_id.clone(),
                expected_generation: current_binding.generation,
                external_task_ref: discovered.task_id.clone(),
                external_session_ref: discovered.session_id.clone(),
                correlation_id: "phase7-unknown-reconcile-launch-record".to_owned(),
            })
            .unwrap();
        let current_worker = runtime.store().worker(&worker_id).unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &worker_id,
                current_worker.revision,
                WorkerState::Running,
                "phase7-unknown-running",
            )
            .unwrap();
        let running_worker = runtime.store().worker(&worker_id).unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &worker_id,
                running_worker.revision,
                WorkerState::Succeeded,
                "phase7-unknown-succeeded",
            )
            .unwrap();
        runtime
            .store_mut()
            .record_worker_result(RecordWorkerResultRequest {
                workflow_id: workflow_id.clone(),
                worker_id: worker_id.clone(),
                worker_state: WorkerState::Succeeded,
                aggregation_state: ResultAggregationState::Complete,
                summary: output,
                artifact_sha256: Some(artifact_sha256.clone()),
                truncated: false,
                completed_at_ms: 1,
                retention_until_ms: None,
            })
            .unwrap();
        runtime.advance_workflow(&workflow_id, 1).unwrap();
    }

    let reconciled_status = call_tool(
        &mut restarted_server,
        "phase7-unknown-worker-status-after-reconcile",
        "worker_status",
        worker_status_args(worker_id.as_str()),
    );
    assert_eq!(reconciled_status["state"], "SUCCEEDED");
    assert_eq!(reconciled_status["externalBinding"]["state"], "ACTIVE");
    assert_eq!(
        reconciled_status["externalBinding"]["hasExternalSession"],
        true
    );
    let result = call_tool(
        &mut restarted_server,
        "phase7-unknown-workflow-result",
        "workflow_result",
        workflow_result_args(workflow_id.as_str()),
    );
    assert_eq!(result["state"], "complete");
    assert_eq!(result["workers"][0]["workerId"], worker_id.as_str());
    assert_eq!(result["workers"][0]["artifactSha256"], artifact_sha256);

    let records = fake.records();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.tool_name.as_deref() == Some(TOOL_LAUNCH_TASK))
            .count(),
        1
    );
    assert!(records
        .iter()
        .any(|record| record.tool_name.as_deref() == Some(TOOL_LIST_SESSIONS)));
    assert!(records
        .iter()
        .any(|record| record.tool_name.as_deref() == Some(TOOL_LIST_PANES)));
    assert_eq!(
        records
            .iter()
            .filter(|record| {
                matches!(
                    record.tool_name.as_deref(),
                    Some(
                        TOOL_CREATE_WORKSPACE
                            | TOOL_ADD_PROJECT_TO_WORKSPACE
                            | TOOL_SUBMIT_TO_SESSION
                            | TOOL_KILL_SESSION
                    )
                )
            })
            .count(),
        0
    );
}

#[test]
fn phase7_ccpanes_process_restart_canary_recovers_fake_endpoint_without_duplicate_launch() {
    let dir = tempdir().unwrap();
    let database_path = dir.path().join("coordination.db");
    let runtime = Rc::new(RefCell::new(
        CoordinatorRuntime::open(
            &database_path,
            dir.path(),
            "phase7-ccpanes-process-restart-ipc",
            CoordinatorConfig::default(),
        )
        .unwrap(),
    ));
    let repository_registration =
        prepare_synthetic_git_repository(dir.path(), "codex-ccpanes-phase7-process-restart")
            .unwrap();
    let workspace_name =
        dedicated_workspace_name_for_project_path(&repository_registration.repository.path);
    let project_name = "phase7-process-restart-project";
    let fake_a = FakeCcpanesServer::start(FakeCcpanesBehavior {
        workspaces: vec![json!({"id": "workspace-secret-id-a", "name": workspace_name})],
        projects: vec![json!({
            "id": "project-secret-id-a",
            "name": project_name,
            "path": repository_registration.repository.path.clone(),
            "workspaceName": workspace_name
        })],
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let old_endpoint = fake_a.endpoint().to_owned();
    let short_timeouts = AdapterTimeouts::new(
        std::time::Duration::from_millis(100),
        std::time::Duration::from_millis(150),
    );
    let mut adapter_a = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(fake_a.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts,
    )
    .unwrap();
    let workspace_report = run_dedicated_workspace_project_gate(
        &mut adapter_a,
        DedicatedWorkspaceSpec {
            workspace_name: workspace_name.clone(),
            project_path: repository_registration.repository.path.clone(),
            project_name: project_name.to_owned(),
        },
    );
    assert_eq!(
        workspace_report.readiness,
        DedicatedWorkspaceReadiness::Ready
    );
    assert!(!workspace_report.external_workspace_side_effects);
    let capability_a = adapter_a.discover().unwrap();

    let mut server = server(runtime.clone());
    let accepted = call_tool(
        &mut server,
        "phase7-process-restart-dispatch",
        "worker_dispatch",
        dispatch_args_for_repository(
            "phase7-process-restart",
            &repository_registration.repository.path,
            repository_registration
                .repository
                .expected_head
                .as_deref()
                .unwrap(),
            &workspace_name,
        ),
    );
    let worker_id = WorkerId::try_new(accepted["workerId"].as_str().unwrap()).unwrap();
    let workflow_id = WorkflowId::try_new(accepted["workflowId"].as_str().unwrap()).unwrap();
    let (outbox_id, outbox_revision, outbox_generation, launch_payload, binding_id) = {
        let mut runtime = runtime.borrow_mut();
        runtime.set_dispatch_ready(true);
        let plan = runtime.tick(0).unwrap();
        assert_eq!(plan.dispatches.len(), 1);
        let claimed = runtime.claim_next_dispatch(0).unwrap().unwrap();
        let binding = runtime
            .store()
            .external_binding_for_worker(&worker_id)
            .unwrap()
            .unwrap();
        assert_eq!(claimed.outbox.state, OutboxState::Sending);
        assert_eq!(binding.state, ExternalBindingState::Pending);
        (
            claimed.outbox.outbox_id,
            claimed.outbox.revision,
            claimed.outbox.generation,
            claimed.outbox.payload,
            binding.binding_id,
        )
    };
    assert_eq!(launch_payload["workspaceName"], workspace_name);

    let launched = adapter_a
        .launch_task(launch_payload.clone(), &capability_a)
        .unwrap();
    let session_id = launched.session_id.as_deref().unwrap().to_owned();
    {
        let mut runtime = runtime.borrow_mut();
        runtime
            .store_mut()
            .record_external_launch_success(RecordExternalLaunchSuccessRequest {
                binding_id: binding_id.clone(),
                expected_generation: outbox_generation,
                external_task_ref: launched.task_id.clone(),
                external_session_ref: session_id.clone(),
                correlation_id: "phase7-process-restart-launch-record".to_owned(),
            })
            .unwrap();
        let dispatching_worker = runtime.store().worker(&worker_id).unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &worker_id,
                dispatching_worker.revision,
                WorkerState::Running,
                "phase7-process-restart-running",
            )
            .unwrap();
        runtime
            .store_mut()
            .transition_outbox_state(
                &outbox_id,
                outbox_revision,
                OutboxState::Acknowledged,
                None,
                "phase7-process-restart-launch-ack",
            )
            .unwrap();
    }
    let fake_a_records = fake_a.records();
    drop(fake_a);

    let degraded_report = run_read_only_preflight(&mut adapter_a);
    assert_eq!(degraded_report.readiness, AdapterReadiness::Degraded);
    assert_eq!(
        degraded_report.error.as_ref().unwrap().code,
        ErrorCode::AdapterNotReady
    );
    assert!(!degraded_report.external_worker_side_effects);
    let stale_reconcile = adapter_a
        .reconcile_session(&session_id)
        .expect_err("stopped fake process endpoint must degrade reconcile");
    assert_eq!(stale_reconcile.code, ErrorCode::AdapterNotReady);

    let worker_during_outage = call_tool(
        &mut server,
        "phase7-process-restart-worker-status-during-outage",
        "worker_status",
        worker_status_args(worker_id.as_str()),
    );
    assert_eq!(worker_during_outage["state"], "RUNNING");
    assert_eq!(worker_during_outage["externalBinding"]["state"], "ACTIVE");
    assert_eq!(
        worker_during_outage["externalBinding"]["hasExternalSession"],
        true
    );
    let workflow_during_outage = call_tool(
        &mut server,
        "phase7-process-restart-workflow-status-during-outage",
        "workflow_status",
        workflow_status_args(workflow_id.as_str()),
    );
    assert_eq!(workflow_during_outage["state"], "RUNNING");
    let result_during_outage = call_tool(
        &mut server,
        "phase7-process-restart-result-during-outage",
        "workflow_result",
        workflow_result_args(workflow_id.as_str()),
    );
    assert_eq!(result_during_outage["state"], "pending");
    {
        let mut runtime = runtime.borrow_mut();
        runtime.set_dispatch_ready(false);
        let frozen_plan = runtime.tick(1).unwrap();
        assert!(frozen_plan.frozen);
        assert_eq!(frozen_plan.dispatches.len(), 0);
    }

    let fake_b = FakeCcpanesServer::start(FakeCcpanesBehavior {
        complete_launches: true,
        workspaces: vec![json!({"id": "workspace-secret-id-b", "name": workspace_name})],
        projects: vec![json!({
            "id": "project-secret-id-b",
            "name": project_name,
            "path": repository_registration.repository.path.clone(),
            "workspaceName": workspace_name
        })],
        sessions: vec![FakeSessionSeed {
            task_id: launched
                .task_id
                .clone()
                .unwrap_or_else(|| "task-1".to_owned()),
            session_id: session_id.clone(),
            binding_id: launched
                .binding_id
                .clone()
                .unwrap_or_else(|| "binding-1".to_owned()),
            status: "completed".to_owned(),
            title: launch_payload
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned),
            prompt: launch_payload
                .get("prompt")
                .and_then(Value::as_str)
                .map(str::to_owned),
            client_request_id: launch_payload
                .get("clientRequestId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            created_at_ms: launch_payload
                .get("createdAtMs")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            output: "synthetic output after fake CC-Panes process restart".to_owned(),
        }],
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    assert_ne!(old_endpoint, fake_b.endpoint());
    let mut adapter_b = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(fake_b.endpoint(), Vec::new(), 2).unwrap(),
        short_timeouts,
    )
    .unwrap();
    let recovered_report = run_read_only_preflight(&mut adapter_b);
    assert_eq!(recovered_report.readiness, AdapterReadiness::Ready);
    assert!(recovered_report.error.is_none());
    assert_eq!(recovered_report.endpoint.discovery_generation, 2);
    assert_ne!(
        degraded_report.endpoint.endpoint,
        recovered_report.endpoint.endpoint
    );
    let recovered_inventory = recovered_report.session_inventory.as_ref().unwrap();
    assert_eq!(recovered_inventory.session_count, 1);
    assert_eq!(recovered_inventory.terminal_session_count, 1);
    assert_eq!(recovered_inventory.active_session_count, 0);
    assert_eq!(
        recovered_report
            .status
            .capability
            .as_ref()
            .unwrap()
            .readiness,
        AdapterReadiness::Ready
    );
    {
        let mut runtime = runtime.borrow_mut();
        runtime.set_dispatch_ready(true);
        let recovered_plan = runtime.tick(2).unwrap();
        assert!(!recovered_plan.frozen);
        assert_eq!(recovered_plan.dispatches.len(), 0);
    }

    let snapshot = adapter_b.reconcile_session(&session_id).unwrap();
    assert!(snapshot.terminal);
    assert!(snapshot.succeeded);
    let output = snapshot.output.unwrap();
    assert!(output.contains("after fake CC-Panes process restart"));
    let artifact_sha256 = "a".repeat(64);
    {
        let mut runtime = runtime.borrow_mut();
        let running_worker = runtime.store().worker(&worker_id).unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &worker_id,
                running_worker.revision,
                WorkerState::Succeeded,
                "phase7-process-restart-succeeded",
            )
            .unwrap();
        runtime
            .store_mut()
            .record_worker_result(RecordWorkerResultRequest {
                workflow_id: workflow_id.clone(),
                worker_id: worker_id.clone(),
                worker_state: WorkerState::Succeeded,
                aggregation_state: ResultAggregationState::Complete,
                summary: output,
                artifact_sha256: Some(artifact_sha256.clone()),
                truncated: false,
                completed_at_ms: 2,
                retention_until_ms: None,
            })
            .unwrap();
        runtime.advance_workflow(&workflow_id, 2).unwrap();
    }
    let final_status = call_tool(
        &mut server,
        "phase7-process-restart-worker-status-after-reconcile",
        "worker_status",
        worker_status_args(worker_id.as_str()),
    );
    assert_eq!(final_status["state"], "SUCCEEDED");
    assert_eq!(final_status["externalBinding"]["state"], "ACTIVE");
    let result = call_tool(
        &mut server,
        "phase7-process-restart-workflow-result",
        "workflow_result",
        workflow_result_args(workflow_id.as_str()),
    );
    assert_eq!(result["state"], "complete");
    assert_eq!(result["workers"][0]["workerId"], worker_id.as_str());
    assert_eq!(result["workers"][0]["artifactSha256"], artifact_sha256);

    let fake_b_records = fake_b.records();
    assert_eq!(
        fake_a_records
            .iter()
            .chain(fake_b_records.iter())
            .filter(|record| record.tool_name.as_deref() == Some(TOOL_LAUNCH_TASK))
            .count(),
        1
    );
    assert!(fake_b_records
        .iter()
        .all(|record| record.tool_name.as_deref() != Some(TOOL_LAUNCH_TASK)));
    assert!(fake_b_records
        .iter()
        .any(|record| record.tool_name.as_deref() == Some(TOOL_LIST_SESSIONS)));
    assert!(fake_b_records
        .iter()
        .any(|record| record.tool_name.as_deref() == Some(TOOL_LIST_PANES)));
    assert!(fake_b_records
        .iter()
        .any(|record| record.tool_name.as_deref() == Some(TOOL_GET_SESSION_STATUS)));
    assert!(fake_b_records
        .iter()
        .any(|record| record.tool_name.as_deref() == Some(TOOL_GET_SESSION_OUTPUT)));
    assert_eq!(
        fake_a_records
            .iter()
            .chain(fake_b_records.iter())
            .filter(|record| {
                matches!(
                    record.tool_name.as_deref(),
                    Some(
                        TOOL_CREATE_WORKSPACE
                            | TOOL_ADD_PROJECT_TO_WORKSPACE
                            | TOOL_SUBMIT_TO_SESSION
                            | TOOL_KILL_SESSION
                    )
                )
            })
            .count(),
        0
    );
}

#[test]
fn phase7_multi_worker_capacity_canary_batches_fake_launches_without_overdispatch() {
    let dir = tempdir().unwrap();
    let database_path = dir.path().join("coordination.db");
    let config = CoordinatorConfig {
        host_max_active_workers: 3,
        dispatch_inflight: 3,
        max_dispatches_per_workflow_tick: 3,
        work_queue_capacity: 6,
        ..CoordinatorConfig::default()
    };
    let runtime = Rc::new(RefCell::new(
        CoordinatorRuntime::open(
            &database_path,
            dir.path(),
            "phase7-multi-worker-capacity-ipc",
            config,
        )
        .unwrap(),
    ));
    let repository_registration =
        prepare_synthetic_git_repository(dir.path(), "codex-ccpanes-phase7-capacity").unwrap();
    let workspace_name =
        dedicated_workspace_name_for_project_path(&repository_registration.repository.path);
    let project_name = "phase7-capacity-project";
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior {
        complete_launches: true,
        workspaces: vec![json!({"id": "workspace-secret-id", "name": workspace_name})],
        projects: vec![json!({
            "id": "project-secret-id",
            "name": project_name,
            "path": repository_registration.repository.path.clone(),
            "workspaceName": workspace_name
        })],
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(fake.endpoint(), Vec::new(), 1).unwrap(),
        AdapterTimeouts::default(),
    )
    .unwrap();
    let workspace_report = run_dedicated_workspace_project_gate(
        &mut adapter,
        DedicatedWorkspaceSpec {
            workspace_name: workspace_name.clone(),
            project_path: repository_registration.repository.path.clone(),
            project_name: project_name.to_owned(),
        },
    );
    assert_eq!(
        workspace_report.readiness,
        DedicatedWorkspaceReadiness::Ready
    );
    assert!(workspace_report.launch_task_accepts_workspace_name);
    assert!(!workspace_report.external_workspace_side_effects);
    let capability = adapter.discover().unwrap();

    let mut server = server(runtime.clone());
    let created = call_tool(
        &mut server,
        "phase7-capacity-workflow-create",
        "workflow_create",
        workflow_create_args_for_repository(
            "phase7-capacity-create",
            "phase7-capacity",
            &repository_registration.repository.path,
            repository_registration
                .repository
                .expected_head
                .as_deref()
                .unwrap(),
            &workspace_name,
            6,
            6,
        ),
    );
    assert_eq!(created["state"], "DRAFT");
    let workflow_id = WorkflowId::try_new(created["workflowId"].as_str().unwrap()).unwrap();
    let started = call_tool(
        &mut server,
        "phase7-capacity-workflow-start",
        "workflow_start",
        workflow_start_args(
            "phase7-capacity-start",
            workflow_id.as_str(),
            created["revision"].clone(),
        ),
    );
    assert_eq!(started["state"], "RUNNING");
    let started_status = call_tool(
        &mut server,
        "phase7-capacity-status-after-start",
        "workflow_status",
        workflow_status_args(workflow_id.as_str()),
    );
    assert_eq!(started_status["workers"].as_array().unwrap().len(), 6);
    assert!(started_status["workers"]
        .as_array()
        .unwrap()
        .iter()
        .all(|worker| worker["state"] == "QUEUED"));

    let first_batch = {
        let mut runtime = runtime.borrow_mut();
        runtime.set_dispatch_ready(true);
        let first_plan = runtime.tick(0).unwrap();
        assert_eq!(first_plan.dispatches.len(), 3);
        let claims = claim_dispatch_batch(&mut runtime, 3, 0, &workspace_name);
        let counts = runtime
            .store()
            .workflow_worker_counts(&workflow_id)
            .unwrap();
        assert_eq!(counts.active_workers, 3);
        assert_eq!(counts.queued_workers, 3);
        let saturated_plan = runtime.tick(0).unwrap();
        assert!(saturated_plan.dispatches.is_empty());
        claims
    };

    let mut launched_sessions = Vec::new();
    for claim in &first_batch {
        let launched = adapter
            .launch_task(claim.payload.clone(), &capability)
            .unwrap();
        let session_id = launched.session_id.as_deref().unwrap().to_owned();
        launched_sessions.push((claim.worker_id.clone(), session_id.clone()));
        let mut runtime = runtime.borrow_mut();
        runtime
            .store_mut()
            .record_external_launch_success(RecordExternalLaunchSuccessRequest {
                binding_id: claim.binding_id.clone(),
                expected_generation: claim.outbox_generation,
                external_task_ref: launched.task_id.clone(),
                external_session_ref: session_id,
                correlation_id: format!("phase7-capacity-launch-{}", claim.step_id.as_str()),
            })
            .unwrap();
        let dispatching_worker = runtime.store().worker(&claim.worker_id).unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &claim.worker_id,
                dispatching_worker.revision,
                WorkerState::Running,
                "phase7-capacity-running",
            )
            .unwrap();
        runtime
            .store_mut()
            .transition_outbox_state(
                &claim.outbox_id,
                claim.outbox_revision,
                OutboxState::Acknowledged,
                None,
                "phase7-capacity-launch-ack",
            )
            .unwrap();
    }
    {
        let mut runtime = runtime.borrow_mut();
        let counts = runtime
            .store()
            .workflow_worker_counts(&workflow_id)
            .unwrap();
        assert_eq!(counts.active_workers, 3);
        assert_eq!(counts.queued_workers, 3);
        let active_plan = runtime.tick(1).unwrap();
        assert!(active_plan.dispatches.is_empty());
    }
    assert_eq!(
        fake.records()
            .iter()
            .filter(|record| record.tool_name.as_deref() == Some(TOOL_LAUNCH_TASK))
            .count(),
        3
    );

    for claim in &first_batch {
        let session_id = launched_sessions
            .iter()
            .find(|(worker_id, _)| worker_id == &claim.worker_id)
            .map(|(_, session_id)| session_id.as_str())
            .unwrap();
        let snapshot = adapter.reconcile_session(session_id).unwrap();
        assert!(snapshot.terminal);
        assert!(snapshot.succeeded);
        let output = snapshot.output.unwrap();
        let mut runtime = runtime.borrow_mut();
        let running_worker = runtime.store().worker(&claim.worker_id).unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &claim.worker_id,
                running_worker.revision,
                WorkerState::Succeeded,
                "phase7-capacity-succeeded",
            )
            .unwrap();
        runtime
            .store_mut()
            .record_worker_result(RecordWorkerResultRequest {
                workflow_id: workflow_id.clone(),
                worker_id: claim.worker_id.clone(),
                worker_state: WorkerState::Succeeded,
                aggregation_state: ResultAggregationState::Complete,
                summary: output,
                artifact_sha256: Some(format!("{:064x}", launched_sessions.len())),
                truncated: false,
                completed_at_ms: 1,
                retention_until_ms: None,
            })
            .unwrap();
    }
    {
        let mut runtime = runtime.borrow_mut();
        runtime.advance_workflow(&workflow_id, 1).unwrap();
        let counts = runtime
            .store()
            .workflow_worker_counts(&workflow_id)
            .unwrap();
        assert_eq!(counts.active_workers, 0);
        assert_eq!(counts.queued_workers, 3);
    }

    let second_batch = {
        let mut runtime = runtime.borrow_mut();
        let second_plan = runtime.tick(2).unwrap();
        assert_eq!(second_plan.dispatches.len(), 3);
        let claims = claim_dispatch_batch(&mut runtime, 3, 2, &workspace_name);
        let counts = runtime
            .store()
            .workflow_worker_counts(&workflow_id)
            .unwrap();
        assert_eq!(counts.active_workers, 3);
        assert_eq!(counts.queued_workers, 0);
        let saturated_plan = runtime.tick(2).unwrap();
        assert!(saturated_plan.dispatches.is_empty());
        claims
    };

    for claim in &second_batch {
        let launched = adapter
            .launch_task(claim.payload.clone(), &capability)
            .unwrap();
        let session_id = launched.session_id.as_deref().unwrap().to_owned();
        launched_sessions.push((claim.worker_id.clone(), session_id.clone()));
        {
            let mut runtime = runtime.borrow_mut();
            runtime
                .store_mut()
                .record_external_launch_success(RecordExternalLaunchSuccessRequest {
                    binding_id: claim.binding_id.clone(),
                    expected_generation: claim.outbox_generation,
                    external_task_ref: launched.task_id.clone(),
                    external_session_ref: session_id.clone(),
                    correlation_id: format!("phase7-capacity-launch-{}", claim.step_id.as_str()),
                })
                .unwrap();
            let dispatching_worker = runtime.store().worker(&claim.worker_id).unwrap();
            runtime
                .store_mut()
                .transition_worker_state(
                    &claim.worker_id,
                    dispatching_worker.revision,
                    WorkerState::Running,
                    "phase7-capacity-running",
                )
                .unwrap();
            runtime
                .store_mut()
                .transition_outbox_state(
                    &claim.outbox_id,
                    claim.outbox_revision,
                    OutboxState::Acknowledged,
                    None,
                    "phase7-capacity-launch-ack",
                )
                .unwrap();
        }
        let snapshot = adapter.reconcile_session(&session_id).unwrap();
        assert!(snapshot.terminal);
        assert!(snapshot.succeeded);
        let output = snapshot.output.unwrap();
        let mut runtime = runtime.borrow_mut();
        let running_worker = runtime.store().worker(&claim.worker_id).unwrap();
        runtime
            .store_mut()
            .transition_worker_state(
                &claim.worker_id,
                running_worker.revision,
                WorkerState::Succeeded,
                "phase7-capacity-succeeded",
            )
            .unwrap();
        runtime
            .store_mut()
            .record_worker_result(RecordWorkerResultRequest {
                workflow_id: workflow_id.clone(),
                worker_id: claim.worker_id.clone(),
                worker_state: WorkerState::Succeeded,
                aggregation_state: ResultAggregationState::Complete,
                summary: output,
                artifact_sha256: Some(format!("{:064x}", launched_sessions.len())),
                truncated: false,
                completed_at_ms: 2,
                retention_until_ms: None,
            })
            .unwrap();
    }
    {
        let mut runtime = runtime.borrow_mut();
        runtime.advance_workflow(&workflow_id, 2).unwrap();
        let counts = runtime
            .store()
            .workflow_worker_counts(&workflow_id)
            .unwrap();
        assert_eq!(counts.active_workers, 0);
        assert_eq!(counts.queued_workers, 0);
    }

    let final_status = call_tool(
        &mut server,
        "phase7-capacity-final-status",
        "workflow_status",
        workflow_status_args(workflow_id.as_str()),
    );
    assert_eq!(final_status["state"], "SUCCEEDED");
    assert_eq!(final_status["workers"].as_array().unwrap().len(), 6);
    assert!(final_status["workers"]
        .as_array()
        .unwrap()
        .iter()
        .all(|worker| worker["state"] == "SUCCEEDED"));
    let result = call_tool(
        &mut server,
        "phase7-capacity-result",
        "workflow_result",
        workflow_result_args(workflow_id.as_str()),
    );
    assert_eq!(result["state"], "complete");
    assert_eq!(result["workers"].as_array().unwrap().len(), 6);
    let step_ids = result["workers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|worker| worker["stepId"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    let expected_steps = (1..=6)
        .map(|index| format!("step-{index:02}"))
        .collect::<BTreeSet<_>>();
    assert_eq!(step_ids, expected_steps);
    assert!(result["missingResults"].as_array().unwrap().is_empty());
    assert!(result["failedWorkers"].as_array().unwrap().is_empty());

    let records = fake.records();
    let launch_records = records
        .iter()
        .filter(|record| record.tool_name.as_deref() == Some(TOOL_LAUNCH_TASK))
        .collect::<Vec<_>>();
    assert_eq!(launch_records.len(), 6);
    let client_request_ids = launch_records
        .iter()
        .map(|record| {
            record.arguments.as_ref().unwrap()["clientRequestId"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(client_request_ids.len(), 6);
    let unique_sessions = launched_sessions
        .iter()
        .map(|(_, session_id)| session_id.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(unique_sessions.len(), 6);
    let unique_bindings = first_batch
        .iter()
        .chain(second_batch.iter())
        .map(|claim| claim.binding_id.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(unique_bindings.len(), 6);
    for (worker_id, _) in &launched_sessions {
        let binding = runtime
            .borrow()
            .store()
            .external_binding_for_worker(worker_id)
            .unwrap()
            .unwrap();
        assert_eq!(binding.state, ExternalBindingState::Active);
        assert!(binding.external_session_ref.is_some());
    }
    assert_eq!(
        records
            .iter()
            .filter(|record| {
                matches!(
                    record.tool_name.as_deref(),
                    Some(
                        TOOL_CREATE_WORKSPACE
                            | TOOL_ADD_PROJECT_TO_WORKSPACE
                            | TOOL_SUBMIT_TO_SESSION
                            | TOOL_KILL_SESSION
                    )
                )
            })
            .count(),
        0
    );
}
