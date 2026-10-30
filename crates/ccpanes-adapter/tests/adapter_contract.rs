use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::Duration;

use ccpanes_adapter::{
    ccpanes_endpoint_from_discovery_value, dedicated_workspace_name_for_project_path,
    default_fake_tools, ensure_dedicated_workspace_project, negotiate_capabilities,
    read_ccpanes_discovery_file, require_dispatch_requalification,
    run_dedicated_workspace_project_gate, run_read_only_preflight, AdapterHeader, AdapterReadiness,
    AdapterTimeouts, CapabilitySnapshot, CcpanesAdapter, CcpanesEndpointConfig,
    DedicatedWorkspaceReadiness, DedicatedWorkspaceSpec, FakeCcpanesBehavior, FakeCcpanesServer,
    FakeJsonRpcError, FakeResponseMode, BASELINE_REQUIRED_CCPANES_TOOLS,
    CCPANES_MCP_PROTOCOL_VERSION, DISPATCH_CAPABILITY_TOOLS, OPTIONAL_SESSION_CONTROL_TOOLS,
    TOOL_ADD_PROJECT_TO_WORKSPACE, TOOL_CREATE_WORKSPACE, TOOL_GET_SESSION_OUTPUT,
    TOOL_GET_SESSION_STATUS, TOOL_KILL_SESSION, TOOL_LAUNCH_TASK, TOOL_LIST_PANES,
    TOOL_LIST_PROJECTS, TOOL_LIST_SESSIONS, TOOL_LIST_WORKSPACES, TOOL_SUBMIT_TO_SESSION,
    TOOL_WAIT_FOR_SESSION, TOOL_WRITE_TO_SESSION,
};
use coordination_core::ErrorCode;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

fn config(endpoint: &str, token: &str, generation: u64) -> CcpanesEndpointConfig {
    CcpanesEndpointConfig::new(
        endpoint.to_owned(),
        vec![AdapterHeader::new("Authorization", format!("Bearer {token}")).unwrap()],
        generation,
    )
    .unwrap()
}

fn short_timeouts() -> AdapterTimeouts {
    AdapterTimeouts::new(Duration::from_millis(100), Duration::from_millis(150))
}

fn launch_args() -> serde_json::Value {
    json!({
        "projectPath": "D:/SYNTHETIC",
        "prompt": "produce result",
        "title": "[CODEX-CCPANES:workflow-1:worker-1] worker",
        "cliTool": "codex",
        "clientRequestId": "request-1",
        "futureRejected": "not forwarded"
    })
}

#[test]
fn reconcile_session_keeps_output_available_before_terminal_state() {
    let fake = FakeCcpanesServer::start(FakeCcpanesBehavior::default()).unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(fake.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();
    let capability = adapter.discover().unwrap();
    let launched = adapter.launch_task(launch_args(), &capability).unwrap();

    let snapshot = adapter
        .reconcile_session(launched.session_id.as_deref().unwrap())
        .unwrap();

    assert_eq!(snapshot.status, "running");
    assert!(!snapshot.terminal);
    assert_eq!(
        snapshot.output.as_deref(),
        Some("synthetic output for session-1")
    );
}

fn v0123_capability_fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/ccpanes-v0.12.3-capabilities.json")).unwrap()
}

fn fake_tools_with_rotation_marker() -> Vec<Value> {
    let mut tools = default_fake_tools();
    let launch_tool = tools
        .iter_mut()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some(TOOL_LAUNCH_TASK))
        .expect("launch_task fixture");
    launch_tool
        .get_mut("inputSchema")
        .and_then(|schema| schema.get_mut("properties"))
        .and_then(Value::as_object_mut)
        .expect("launch_task properties fixture")
        .insert("phase7RotationMarker".to_owned(), json!({"type": "string"}));
    tools
}

fn assert_json_omits<T: Serialize>(label: &str, value: &T, forbidden_values: &[&str]) {
    let serialized = serde_json::to_string(value).unwrap();
    for forbidden in forbidden_values {
        assert!(
            !serialized.contains(forbidden),
            "{label} exposed forbidden value {forbidden}"
        );
    }
}

fn assert_no_worker_or_workspace_side_effects(server: &FakeCcpanesServer) {
    let records = server.records();
    for forbidden_tool in [
        TOOL_CREATE_WORKSPACE,
        TOOL_ADD_PROJECT_TO_WORKSPACE,
        TOOL_LAUNCH_TASK,
        TOOL_GET_SESSION_STATUS,
        TOOL_GET_SESSION_OUTPUT,
        TOOL_SUBMIT_TO_SESSION,
        TOOL_KILL_SESSION,
        TOOL_WAIT_FOR_SESSION,
        TOOL_WRITE_TO_SESSION,
    ] {
        assert!(
            records
                .iter()
                .all(|record| record.tool_name.as_deref() != Some(forbidden_tool)),
            "rotation canary invoked side-effect or per-session tool {forbidden_tool}"
        );
    }
}

#[test]
fn fake_json_server_drives_launch_status_output_submit_kill_and_reconcile() {
    let behavior = FakeCcpanesBehavior {
        required_authorization: Some("Bearer TOKEN_ONE".to_owned()),
        ..FakeCcpanesBehavior::default()
    };
    let server = FakeCcpanesServer::start(behavior).unwrap();
    let mut adapter =
        CcpanesAdapter::new(config(server.endpoint(), "TOKEN_ONE", 1), short_timeouts()).unwrap();

    let capability = adapter.discover().unwrap();
    assert_eq!(capability.readiness, AdapterReadiness::Ready);
    assert_eq!(
        capability.protocol_version.as_deref(),
        Some(CCPANES_MCP_PROTOCOL_VERSION)
    );
    assert_eq!(adapter.session_id(), Some("fake-mcp-session"));

    let launched = adapter.launch_task(launch_args(), &capability).unwrap();
    assert_eq!(launched.task_id.as_deref(), Some("task-1"));
    assert_eq!(launched.session_id.as_deref(), Some("session-1"));
    assert_eq!(launched.runtime_kind.as_deref(), Some("local"));

    let forwarded = server.last_tool_arguments("launch_task").unwrap();
    assert_eq!(forwarded["clientRequestId"], "request-1");
    assert!(forwarded.get("futureRejected").is_none());

    let status = adapter.get_session_status("session-1").unwrap();
    assert_eq!(status["status"], "running");
    let submitted = adapter
        .submit_to_session("session-1", "synthetic steer")
        .unwrap();
    assert_eq!(submitted["accepted"], true);
    let outcome = adapter
        .kill_session("session-1", Some("synthetic cancel"), true)
        .unwrap();
    assert_eq!(format!("{outcome:?}"), "Accepted");
    let snapshot = adapter.reconcile_session("session-1").unwrap();
    assert_eq!(snapshot.status, "cancelled");
    assert!(snapshot.terminal);
    assert!(!snapshot.succeeded);
    assert!(snapshot.output.unwrap().contains("synthetic output"));

    let prompt_hash = hex::encode(Sha256::digest(b"produce result"));
    let binding = adapter
        .find_unknown_dispatch(
            "request-1",
            "[CODEX-CCPANES:workflow-1:worker-1]",
            &prompt_hash,
            1_001,
        )
        .unwrap()
        .unwrap();
    assert_eq!(binding.session_id, "session-1");
    assert_eq!(binding.match_basis, "clientRequestId");

    let records = server.records();
    assert!(records
        .iter()
        .all(|record| { !serde_json::to_string(record).unwrap().contains("TOKEN_ONE") }));
}

#[test]
fn v0123_capability_fixture_negotiates_session_primitives() {
    let fixture = v0123_capability_fixture();
    assert_eq!(fixture["source"]["tag"], "v0.12.3");
    assert_eq!(
        fixture["source"]["commit"],
        "7fad08d8a210015eb12af283c720394e7324f471"
    );
    let tool_names = fixture["toolsListResult"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        tool_names,
        BTreeSet::from([
            TOOL_LAUNCH_TASK,
            TOOL_LIST_SESSIONS,
            TOOL_LIST_PANES,
            TOOL_GET_SESSION_STATUS,
            TOOL_GET_SESSION_OUTPUT,
            TOOL_SUBMIT_TO_SESSION,
            TOOL_KILL_SESSION,
            TOOL_WAIT_FOR_SESSION,
            TOOL_WRITE_TO_SESSION,
        ])
    );
    assert_eq!(
        BASELINE_REQUIRED_CCPANES_TOOLS,
        [TOOL_LAUNCH_TASK, TOOL_LIST_SESSIONS, TOOL_LIST_PANES]
    );
    assert_eq!(
        DISPATCH_CAPABILITY_TOOLS,
        [TOOL_GET_SESSION_STATUS, TOOL_GET_SESSION_OUTPUT]
    );
    assert_eq!(
        OPTIONAL_SESSION_CONTROL_TOOLS,
        [
            TOOL_SUBMIT_TO_SESSION,
            TOOL_KILL_SESSION,
            TOOL_WAIT_FOR_SESSION,
            TOOL_WRITE_TO_SESSION,
        ]
    );
    let required_fields = |tool_name: &str| {
        fixture["toolsListResult"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == tool_name)
            .unwrap()["inputSchema"]["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        required_fields(TOOL_LAUNCH_TASK),
        BTreeSet::from(["projectPath"])
    );
    assert_eq!(
        required_fields(TOOL_GET_SESSION_OUTPUT),
        BTreeSet::from(["sessionId"])
    );
    assert_eq!(
        required_fields(TOOL_KILL_SESSION),
        BTreeSet::from(["sessionId"])
    );
    assert_eq!(
        required_fields(TOOL_WAIT_FOR_SESSION),
        BTreeSet::from(["waitFor"])
    );
    assert_eq!(
        required_fields(TOOL_WRITE_TO_SESSION),
        BTreeSet::from(["sessionId", "text"])
    );

    let capability =
        negotiate_capabilities(&fixture["initializeResult"], &fixture["toolsListResult"]).unwrap();

    assert_eq!(capability.readiness, AdapterReadiness::Ready);
    assert!(capability.dispatch_ready);
    assert!(capability.get_session_status_available);
    assert!(capability.get_session_output_available);
    assert!(capability.submit_to_session_available);
    assert!(capability.kill_session_available);
    assert!(capability.wait_for_session_available);
    assert!(capability.write_to_session_available);
    assert!(!capability.kill_session_accepts_reason);
}

#[test]
fn optional_session_tools_do_not_break_capability_negotiation() {
    let fixture = v0123_capability_fixture();
    let mut tools = fixture["toolsListResult"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    tools.retain(|tool| {
        !matches!(
            tool["name"].as_str(),
            Some(TOOL_SUBMIT_TO_SESSION)
                | Some(TOOL_KILL_SESSION)
                | Some(TOOL_WAIT_FOR_SESSION)
                | Some(TOOL_WRITE_TO_SESSION)
        )
    });

    let capability =
        negotiate_capabilities(&fixture["initializeResult"], &json!({"tools": tools})).unwrap();

    assert_eq!(capability.readiness, AdapterReadiness::Ready);
    assert!(capability.dispatch_ready);
    assert!(!capability.submit_to_session_available);
    assert!(!capability.kill_session_available);
    assert!(!capability.wait_for_session_available);
    assert!(!capability.write_to_session_available);
}

#[test]
fn unused_launch_fields_do_not_break_capability_negotiation() {
    let fixture = v0123_capability_fixture();
    let mut tools = fixture["toolsListResult"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    let launch = tools
        .iter_mut()
        .find(|tool| tool["name"] == TOOL_LAUNCH_TASK)
        .unwrap();
    let properties = launch["inputSchema"]["properties"].as_object_mut().unwrap();
    for unused in [
        "providerId",
        "modelId",
        "providerSelection",
        "profileId",
        "runtimeKind",
        "resumeId",
        "paneId",
        "layoutId",
        "layoutName",
        "placement",
    ] {
        properties.remove(unused);
    }

    let capability =
        negotiate_capabilities(&fixture["initializeResult"], &json!({"tools": tools})).unwrap();

    assert_eq!(capability.readiness, AdapterReadiness::Ready);
    assert!(capability.dispatch_ready);
    assert!(capability
        .launch_task_properties
        .iter()
        .any(|property| property == "cliTool"));
}

#[test]
fn missing_dispatch_capability_blocks_launch_before_worker_side_effect() {
    let fixture = v0123_capability_fixture();
    let tools = fixture["toolsListResult"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| tool["name"] != TOOL_GET_SESSION_OUTPUT)
        .cloned()
        .collect();
    let server = FakeCcpanesServer::start(FakeCcpanesBehavior {
        tools,
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();

    let capability = adapter.discover().unwrap();
    assert_eq!(capability.readiness, AdapterReadiness::Ready);
    assert!(!capability.dispatch_ready);
    assert!(!capability.get_session_output_available);

    let error = adapter
        .launch_task(launch_args(), &capability)
        .expect_err("dispatch capability is incomplete");
    assert_eq!(error.code, ErrorCode::AdapterIncompatible);
    assert!(server
        .records()
        .iter()
        .all(|record| record.tool_name.as_deref() != Some(TOOL_LAUNCH_TASK)));
}

#[test]
fn missing_optional_session_tools_fail_typed_without_tool_call() {
    let fixture = v0123_capability_fixture();
    let tools = fixture["toolsListResult"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| {
            !matches!(
                tool["name"].as_str(),
                Some(TOOL_WAIT_FOR_SESSION) | Some(TOOL_WRITE_TO_SESSION)
            )
        })
        .cloned()
        .collect();
    let server = FakeCcpanesServer::start(FakeCcpanesBehavior {
        tools,
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();

    let capability = adapter.discover().unwrap();
    assert!(!capability.wait_for_session_available);
    assert!(!capability.write_to_session_available);

    let wait_error = adapter
        .wait_for_session("session-1", &["completed"], Some(1_000))
        .expect_err("wait_for_session is absent");
    assert_eq!(wait_error.code, ErrorCode::AdapterIncompatible);
    let write_error = adapter
        .write_to_session("session-1", "\u{3}")
        .expect_err("write_to_session is absent");
    assert_eq!(write_error.code, ErrorCode::AdapterIncompatible);
    assert!(server.records().iter().all(|record| {
        !matches!(
            record.tool_name.as_deref(),
            Some(TOOL_WAIT_FOR_SESSION) | Some(TOOL_WRITE_TO_SESSION)
        )
    }));
}

#[test]
fn malformed_optional_session_tool_schema_is_typed_incompatible() {
    let fixture = v0123_capability_fixture();
    let mut tools = fixture["toolsListResult"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    let write_tool = tools
        .iter_mut()
        .find(|tool| tool["name"] == TOOL_WRITE_TO_SESSION)
        .unwrap();
    write_tool["inputSchema"]["required"] = json!(["sessionId"]);

    let error = negotiate_capabilities(&fixture["initializeResult"], &json!({"tools": tools}))
        .expect_err("declared optional tool has incompatible schema");
    assert_eq!(error.code, ErrorCode::AdapterIncompatible);
    assert!(error.message.contains(TOOL_WRITE_TO_SESSION));
    assert!(error.message.contains("text"));
}

#[test]
fn used_tool_argument_types_must_match_adapter_payloads() {
    let fixture = v0123_capability_fixture();
    let mut tools = fixture["toolsListResult"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    let output_tool = tools
        .iter_mut()
        .find(|tool| tool["name"] == TOOL_GET_SESSION_OUTPUT)
        .unwrap();
    output_tool["inputSchema"]["properties"]["sessionId"]["type"] = json!("integer");

    let error = negotiate_capabilities(&fixture["initializeResult"], &json!({"tools": tools}))
        .expect_err("sessionId must accept string payloads");
    assert_eq!(error.code, ErrorCode::AdapterIncompatible);
    assert!(error.message.contains(TOOL_GET_SESSION_OUTPUT));
    assert!(error.message.contains("sessionId"));

    let mut tools = fixture["toolsListResult"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    let wait_tool = tools
        .iter_mut()
        .find(|tool| tool["name"] == TOOL_WAIT_FOR_SESSION)
        .unwrap();
    wait_tool["inputSchema"]["properties"]["waitFor"]["items"]["type"] = json!("integer");

    let error = negotiate_capabilities(&fixture["initializeResult"], &json!({"tools": tools}))
        .expect_err("waitFor items must accept string payloads");
    assert_eq!(error.code, ErrorCode::AdapterIncompatible);
    assert!(error.message.contains(TOOL_WAIT_FOR_SESSION));
    assert!(error.message.contains("waitFor"));
}

#[test]
fn v0123_kill_schema_omits_reason_and_optional_wait_write_are_callable() {
    let fixture = v0123_capability_fixture();
    let tools = fixture["toolsListResult"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    let server = FakeCcpanesServer::start(FakeCcpanesBehavior {
        tools,
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();
    let capability = adapter.discover().unwrap();
    let launched = adapter.launch_task(launch_args(), &capability).unwrap();
    assert_eq!(launched.task_id.as_deref(), Some("task-1"));
    assert_eq!(launched.session_id.as_deref(), Some("session-1"));
    assert_eq!(launched.runtime_kind.as_deref(), Some("local"));
    let session_id = launched.session_id.as_deref().unwrap();

    let wait = adapter
        .wait_for_session(session_id, &["running"], Some(1_000))
        .unwrap();
    assert_eq!(wait["satisfied"], true);
    let written = adapter.write_to_session(session_id, "\u{3}").unwrap();
    assert_eq!(written["success"], true);
    adapter
        .kill_session(session_id, Some("not accepted by v0.12.3"), false)
        .unwrap();

    let kill_arguments = server.last_tool_arguments(TOOL_KILL_SESSION).unwrap();
    assert_eq!(kill_arguments, json!({"sessionId": session_id}));
}

#[test]
fn fake_sse_server_uses_same_adapter_contract() {
    let behavior = FakeCcpanesBehavior {
        response_mode: FakeResponseMode::Sse,
        ..FakeCcpanesBehavior::default()
    };
    let server = FakeCcpanesServer::start(behavior).unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();

    let capability = adapter.discover().unwrap();
    let launched = adapter.launch_task(launch_args(), &capability).unwrap();
    assert_eq!(launched.session_id.as_deref(), Some("session-1"));
}

#[test]
fn response_timeout_after_launch_is_dispatch_result_unknown() {
    let behavior = FakeCcpanesBehavior {
        hang_after_request: true,
        ..FakeCcpanesBehavior::default()
    };
    let server = FakeCcpanesServer::start(behavior).unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();
    let capability = CapabilitySnapshot {
        readiness: AdapterReadiness::Ready,
        server_version: Some("fake".to_owned()),
        protocol_version: Some(CCPANES_MCP_PROTOCOL_VERSION.to_owned()),
        tools: vec!["launch_task".to_owned()],
        capability_digest: "digest".to_owned(),
        launch_task_properties: vec!["projectPath".to_owned(), "prompt".to_owned()],
        launch_task_accepts_client_request_id: true,
        dispatch_ready: true,
        get_session_status_available: true,
        get_session_output_available: true,
        submit_to_session_available: true,
        kill_session_available: true,
        wait_for_session_available: false,
        write_to_session_available: false,
        kill_session_accepts_reason: true,
    };

    let error = adapter
        .launch_task(launch_args(), &capability)
        .expect_err("launch response timeout");
    assert_eq!(error.code, ErrorCode::DispatchResultUnknown);
}

#[test]
fn connect_phase_failure_is_adapter_not_ready() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    drop(listener);
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(endpoint, Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();

    let error = adapter.discover().expect_err("connect failure");
    assert_eq!(error.code, ErrorCode::AdapterNotReady);
}

#[test]
fn discovery_and_token_rotation_are_reflected_without_secret_disclosure() {
    let server_one = FakeCcpanesServer::start(FakeCcpanesBehavior {
        required_authorization: Some("Bearer TOKEN_ALPHA".to_owned()),
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let server_two = FakeCcpanesServer::start(FakeCcpanesBehavior {
        required_authorization: Some("Bearer TOKEN_BETA".to_owned()),
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let dir = tempdir().unwrap();
    let first_path = dir.path().join("ccpanes-alpha.json");
    let second_path = dir.path().join("ccpanes-beta.json");
    fs::write(
        &first_path,
        json!({
            "mcpServers": {
                "ccpanes": {
                    "url": server_one.endpoint(),
                    "headers": {"Authorization": "Bearer TOKEN_ALPHA"}
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        &second_path,
        json!({
            "mcpServers": {
                "ccpanes": {
                    "url": server_two.endpoint(),
                    "headers": {"Authorization": "Bearer TOKEN_BETA"}
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let first = read_ccpanes_discovery_file(&first_path, 1).unwrap();
    let second = read_ccpanes_discovery_file(&second_path, 2).unwrap();
    assert_ne!(first.endpoint, second.endpoint);
    assert_ne!(
        first.sanitized().header_value_digest,
        second.sanitized().header_value_digest
    );

    let first_json = serde_json::to_string(&first.sanitized()).unwrap();
    let second_json = serde_json::to_string(&second.sanitized()).unwrap();
    assert!(!first_json.contains("TOKEN_ALPHA"));
    assert!(!second_json.contains("TOKEN_BETA"));

    CcpanesAdapter::new(first, short_timeouts())
        .unwrap()
        .discover()
        .unwrap();
    CcpanesAdapter::new(second, short_timeouts())
        .unwrap()
        .discover()
        .unwrap();
}

#[test]
fn phase7_token_discovery_rotation_canary_refreshes_preflight_without_secret_leak() {
    let server_one = FakeCcpanesServer::start(FakeCcpanesBehavior {
        required_authorization: Some("Bearer TOKEN_ALPHA".to_owned()),
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let server_two = FakeCcpanesServer::start(FakeCcpanesBehavior {
        required_authorization: Some("Bearer TOKEN_BETA".to_owned()),
        tools: fake_tools_with_rotation_marker(),
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let dir = tempdir().unwrap();
    let first_path = dir.path().join("ccpanes-alpha.json");
    let second_path = dir.path().join("ccpanes-beta.json");
    fs::write(
        &first_path,
        json!({
            "mcpServers": {
                "ccpanes": {
                    "url": server_one.endpoint(),
                    "headers": {"Authorization": "Bearer TOKEN_ALPHA"}
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        &second_path,
        json!({
            "mcpServers": {
                "ccpanes": {
                    "url": server_two.endpoint(),
                    "headers": {"Authorization": "Bearer TOKEN_BETA"}
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let first = read_ccpanes_discovery_file(&first_path, 7).unwrap();
    let first_header_digest = first.sanitized().header_value_digest;
    let mut adapter = CcpanesAdapter::new(first, short_timeouts()).unwrap();
    let first_report = run_read_only_preflight(&mut adapter);

    assert_eq!(first_report.readiness, AdapterReadiness::Ready);
    assert!(first_report.error.is_none());
    assert_eq!(first_report.endpoint.discovery_generation, 7);
    assert_eq!(
        first_report.endpoint.header_names,
        vec!["Authorization".to_owned()]
    );
    assert_eq!(
        first_report.endpoint.header_value_digest,
        first_header_digest
    );
    let first_capability = first_report.status.capability.as_ref().unwrap();
    assert!(!first_capability
        .launch_task_properties
        .iter()
        .any(|property| property == "phase7RotationMarker"));
    assert_no_worker_or_workspace_side_effects(&server_one);
    assert_json_omits("first preflight report", &first_report, &["TOKEN_ALPHA"]);
    assert_json_omits(
        "first fake records",
        &server_one.records(),
        &["TOKEN_ALPHA"],
    );
    assert!(server_one.records().iter().all(|record| {
        record
            .authorization_digest
            .as_ref()
            .is_some_and(|digest| digest.len() == 64)
    }));

    let stale_config = CcpanesEndpointConfig::new(
        server_two.endpoint().to_owned(),
        vec![AdapterHeader::new("Authorization", "Bearer TOKEN_ALPHA").unwrap()],
        8,
    )
    .unwrap();
    let mut stale_adapter = CcpanesAdapter::new(stale_config, short_timeouts()).unwrap();
    let stale_report = run_read_only_preflight(&mut stale_adapter);

    assert_eq!(stale_report.readiness, AdapterReadiness::Degraded);
    assert_eq!(
        stale_report.error.as_ref().unwrap().code,
        ErrorCode::AuthorizationFailed
    );
    assert_eq!(stale_report.endpoint.discovery_generation, 8);
    assert_no_worker_or_workspace_side_effects(&server_two);
    assert_json_omits(
        "stale-token preflight report",
        &stale_report,
        &["TOKEN_ALPHA", "TOKEN_BETA"],
    );

    let second = read_ccpanes_discovery_file(&second_path, 9).unwrap();
    let second_header_digest = second.sanitized().header_value_digest;
    let mut rotated_adapter = CcpanesAdapter::new(second, short_timeouts()).unwrap();
    let second_report = run_read_only_preflight(&mut rotated_adapter);

    assert_eq!(second_report.readiness, AdapterReadiness::Ready);
    assert!(second_report.error.is_none());
    assert_eq!(second_report.endpoint.discovery_generation, 9);
    assert_eq!(
        second_report.endpoint.header_value_digest,
        second_header_digest
    );
    assert_ne!(
        first_report.endpoint.endpoint,
        second_report.endpoint.endpoint
    );
    assert_ne!(
        first_report.endpoint.header_value_digest,
        second_report.endpoint.header_value_digest
    );
    let second_capability = second_report.status.capability.as_ref().unwrap();
    assert_ne!(
        first_capability.capability_digest,
        second_capability.capability_digest
    );
    assert!(second_capability
        .launch_task_properties
        .iter()
        .any(|property| property == "phase7RotationMarker"));
    assert_no_worker_or_workspace_side_effects(&server_two);
    assert_json_omits(
        "second preflight report",
        &second_report,
        &["TOKEN_ALPHA", "TOKEN_BETA"],
    );
    assert_json_omits(
        "second fake records",
        &server_two.records(),
        &["TOKEN_ALPHA", "TOKEN_BETA"],
    );
    assert!(server_two.records().iter().all(|record| {
        record
            .authorization_digest
            .as_ref()
            .is_some_and(|digest| digest.len() == 64)
    }));
}

#[test]
fn structured_and_text_business_errors_map_to_safe_public_errors() {
    let mut jsonrpc_errors = BTreeMap::new();
    jsonrpc_errors.insert(
        "tools/call:launch_task".to_owned(),
        FakeJsonRpcError::new(-32602, "invalid Authorization: Bearer TOKEN_PLACEHOLDER"),
    );
    let server = FakeCcpanesServer::start(FakeCcpanesBehavior {
        jsonrpc_errors,
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();
    let capability = adapter.discover().unwrap();
    let error = adapter
        .launch_task(launch_args(), &capability)
        .expect_err("structured tool error");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(!error.message.contains("TOKEN_PLACEHOLDER"));

    let mut tool_text_errors = BTreeMap::new();
    tool_text_errors.insert(
        "launch_task".to_owned(),
        "错误: Authorization: Bearer TOKEN_PLACEHOLDER 项目路径未注册".to_owned(),
    );
    let server = FakeCcpanesServer::start(FakeCcpanesBehavior {
        tool_text_errors,
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();
    let capability = adapter.discover().unwrap();
    let error = adapter
        .launch_task(launch_args(), &capability)
        .expect_err("text tool error");
    assert_eq!(error.code, ErrorCode::AdapterNotReady);
    assert!(!error.message.contains("TOKEN_PLACEHOLDER"));
}

#[test]
fn discovery_value_missing_required_url_fails_closed() {
    let error = ccpanes_endpoint_from_discovery_value(
        &json!({"mcpServers": {"ccpanes": {"headers": {}}}}),
        1,
    )
    .expect_err("missing url");
    assert_eq!(error.code, ErrorCode::AdapterNotReady);
}

#[test]
fn read_only_preflight_negotiates_inventory_without_worker_side_effects() {
    let server = FakeCcpanesServer::start(FakeCcpanesBehavior::default()).unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();

    let report = run_read_only_preflight(&mut adapter);

    assert_eq!(report.readiness, AdapterReadiness::Ready);
    assert!(!report.external_worker_side_effects);
    assert!(report.error.is_none());
    assert_eq!(report.session_inventory.as_ref().unwrap().session_count, 0);
    assert!(report
        .status
        .capability
        .as_ref()
        .unwrap()
        .tools
        .iter()
        .any(|tool| tool == TOOL_LAUNCH_TASK));

    let records = server.records();
    assert!(records.iter().any(|record| record.method == "initialize"));
    assert!(records.iter().any(|record| record.method == "tools/list"));
    assert!(records
        .iter()
        .any(|record| record.tool_name.as_deref() == Some(TOOL_LIST_SESSIONS)));
    assert!(records
        .iter()
        .any(|record| record.tool_name.as_deref() == Some(TOOL_LIST_PANES)));
    for forbidden_tool in [
        TOOL_LAUNCH_TASK,
        TOOL_GET_SESSION_STATUS,
        TOOL_GET_SESSION_OUTPUT,
        TOOL_SUBMIT_TO_SESSION,
        TOOL_KILL_SESSION,
        TOOL_WAIT_FOR_SESSION,
        TOOL_WRITE_TO_SESSION,
    ] {
        assert!(
            records
                .iter()
                .all(|record| record.tool_name.as_deref() != Some(forbidden_tool)),
            "preflight invoked side-effect or per-session tool {forbidden_tool}"
        );
    }
}

#[test]
fn read_only_preflight_dispatch_requalification_requires_dispatch_capability() {
    let fixture = v0123_capability_fixture();
    let tools = fixture["toolsListResult"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| tool["name"] != TOOL_GET_SESSION_OUTPUT)
        .cloned()
        .collect();
    let server = FakeCcpanesServer::start(FakeCcpanesBehavior {
        tools,
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();

    let report = run_read_only_preflight(&mut adapter);

    assert_eq!(report.readiness, AdapterReadiness::Ready);
    assert!(
        !report
            .status
            .capability
            .as_ref()
            .expect("capability snapshot")
            .dispatch_ready
    );
    let error = require_dispatch_requalification(&report)
        .expect_err("basic READY without dispatch capability must fail the update gate");
    assert_eq!(error.code, ErrorCode::AdapterIncompatible);
    assert_no_worker_or_workspace_side_effects(&server);
}

#[test]
fn read_only_preflight_dispatch_requalification_accepts_v0123_fixture() {
    let fixture = v0123_capability_fixture();
    let server = FakeCcpanesServer::start(FakeCcpanesBehavior {
        tools: fixture["toolsListResult"]["tools"]
            .as_array()
            .unwrap()
            .clone(),
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();

    let report = run_read_only_preflight(&mut adapter);

    require_dispatch_requalification(&report).unwrap();
    assert_no_worker_or_workspace_side_effects(&server);
}

#[test]
fn dedicated_workspace_project_gate_is_read_only_for_existing_synthetic_fixture() {
    let repo = tempdir().unwrap();
    let project_path = repo.path().to_string_lossy().replace('\\', "/");
    let workspace_name = "codex-ccpanes-phase7-fixture";
    let behavior = FakeCcpanesBehavior {
        workspaces: vec![json!({
            "id": "workspace-secret-id",
            "name": workspace_name
        })],
        projects: vec![json!({
            "id": "project-secret-id",
            "name": "phase7-project",
            "path": project_path,
            "workspace": workspace_name
        })],
        ..FakeCcpanesBehavior::default()
    };
    let server = FakeCcpanesServer::start(behavior).unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();

    let report = run_dedicated_workspace_project_gate(
        &mut adapter,
        DedicatedWorkspaceSpec {
            workspace_name: workspace_name.to_owned(),
            project_path: repo.path().to_string_lossy().into_owned(),
            project_name: "phase7-project".to_owned(),
        },
    );

    assert_eq!(report.readiness, DedicatedWorkspaceReadiness::Ready);
    assert!(report.error.is_none());
    assert!(report.launch_task_accepts_workspace_name);
    assert!(!report.external_workspace_side_effects);
    assert!(report.workspace.is_some());
    assert!(report.project.is_some());

    let serialized = serde_json::to_string(&report).unwrap();
    assert!(!serialized.contains("workspace-secret-id"));
    assert!(!serialized.contains("project-secret-id"));
    assert!(!serialized.contains(&project_path));

    let records = server.records();
    assert!(records.iter().any(|record| record.method == "initialize"));
    assert!(records.iter().any(|record| record.method == "tools/list"));
    assert!(records
        .iter()
        .any(|record| record.tool_name.as_deref() == Some(TOOL_LIST_WORKSPACES)));
    assert!(records
        .iter()
        .any(|record| record.tool_name.as_deref() == Some(TOOL_LIST_PROJECTS)));
    for forbidden_tool in [
        TOOL_CREATE_WORKSPACE,
        TOOL_ADD_PROJECT_TO_WORKSPACE,
        TOOL_LAUNCH_TASK,
        TOOL_GET_SESSION_STATUS,
        TOOL_GET_SESSION_OUTPUT,
        TOOL_SUBMIT_TO_SESSION,
        TOOL_KILL_SESSION,
        TOOL_WAIT_FOR_SESSION,
        TOOL_WRITE_TO_SESSION,
    ] {
        assert!(
            records
                .iter()
                .all(|record| record.tool_name.as_deref() != Some(forbidden_tool)),
            "workspace gate invoked side-effect or per-session tool {forbidden_tool}"
        );
    }
}

#[test]
fn dedicated_workspace_project_gate_reports_setup_without_creating_when_missing() {
    let repo = tempdir().unwrap();
    let server = FakeCcpanesServer::start(FakeCcpanesBehavior::default()).unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();

    let report = run_dedicated_workspace_project_gate(
        &mut adapter,
        DedicatedWorkspaceSpec {
            workspace_name: "codex-ccpanes-phase7-missing".to_owned(),
            project_path: repo.path().to_string_lossy().into_owned(),
            project_name: "missing-project".to_owned(),
        },
    );

    assert_eq!(report.readiness, DedicatedWorkspaceReadiness::RequiresSetup);
    assert!(report.error.is_none());
    assert!(report
        .deferred_setup_operations
        .contains(&TOOL_CREATE_WORKSPACE.to_owned()));
    assert!(report
        .deferred_setup_operations
        .contains(&TOOL_ADD_PROJECT_TO_WORKSPACE.to_owned()));
    assert!(!report.external_workspace_side_effects);
    assert!(server.records().iter().all(|record| {
        record.tool_name.as_deref() != Some(TOOL_CREATE_WORKSPACE)
            && record.tool_name.as_deref() != Some(TOOL_ADD_PROJECT_TO_WORKSPACE)
    }));
}

#[test]
fn dedicated_workspace_project_setup_creates_project_path_workspace_and_adds_project() {
    let repo = tempdir().unwrap();
    let project_path = repo.path().to_string_lossy().into_owned();
    let workspace_name = dedicated_workspace_name_for_project_path(&project_path);
    let project_name = repo
        .path()
        .file_name()
        .expect("temp dir basename")
        .to_string_lossy()
        .into_owned();
    let server = FakeCcpanesServer::start(FakeCcpanesBehavior::default()).unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();

    let report = ensure_dedicated_workspace_project(
        &mut adapter,
        DedicatedWorkspaceSpec {
            workspace_name: workspace_name.clone(),
            project_path: project_path.clone(),
            project_name,
        },
    );

    assert_eq!(report.readiness, DedicatedWorkspaceReadiness::Ready);
    assert!(report.error.is_none());
    assert!(report.external_workspace_side_effects);
    let create_args = server.last_tool_arguments(TOOL_CREATE_WORKSPACE).unwrap();
    assert_eq!(create_args["name"], workspace_name);
    assert_eq!(create_args["path"], project_path);
    let add_args = server
        .last_tool_arguments(TOOL_ADD_PROJECT_TO_WORKSPACE)
        .unwrap();
    assert_eq!(add_args["workspaceName"], workspace_name);
    assert_eq!(add_args["projectPath"], project_path);
    assert!(
        add_args.get("projectName").is_none(),
        "CC-Panes v0.12.3 add_project_to_workspace derives project name from projectPath"
    );
    let initialize_count = server
        .records()
        .iter()
        .filter(|record| record.method == "initialize")
        .count();
    assert_eq!(
        initialize_count, 1,
        "dedicated workspace setup must not reinitialize an existing MCP transport session"
    );
    assert!(server
        .records()
        .iter()
        .all(|record| record.tool_name.as_deref() != Some(TOOL_LAUNCH_TASK)));
}

#[test]
fn dedicated_workspace_project_gate_rejects_existing_workspace_with_wrong_path() {
    let repo = tempdir().unwrap();
    let project_path = repo.path().to_string_lossy().into_owned();
    let workspace_name = dedicated_workspace_name_for_project_path(&project_path);
    let server = FakeCcpanesServer::start(FakeCcpanesBehavior {
        workspaces: vec![json!({
            "id": "workspace-id",
            "name": workspace_name,
            "path": "D:/DIFFERENT_ROOT"
        })],
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();

    let report = ensure_dedicated_workspace_project(
        &mut adapter,
        DedicatedWorkspaceSpec {
            workspace_name,
            project_path,
            project_name: "project".to_owned(),
        },
    );

    assert_eq!(report.readiness, DedicatedWorkspaceReadiness::Incompatible);
    assert_eq!(
        report.error.as_ref().unwrap().code,
        ErrorCode::AdapterIncompatible
    );
    assert!(report
        .error
        .as_ref()
        .unwrap()
        .message
        .contains("workspace path"));
    assert!(!report.external_workspace_side_effects);
    assert!(server.records().iter().all(|record| {
        record.tool_name.as_deref() != Some(TOOL_CREATE_WORKSPACE)
            && record.tool_name.as_deref() != Some(TOOL_ADD_PROJECT_TO_WORKSPACE)
    }));
}

#[test]
fn dedicated_workspace_project_gate_requires_exact_project_name_path_and_workspace() {
    let repo = tempdir().unwrap();
    let project_path = repo.path().to_string_lossy().replace('\\', "/");
    let workspace_name = "codex-ccpanes-phase7-exact-project";
    let cases = [
        (
            "missing-workspace",
            json!({
                "id": "project-id",
                "name": "phase7-project",
                "path": project_path
            }),
        ),
        (
            "wrong-workspace",
            json!({
                "id": "project-id",
                "name": "phase7-project",
                "path": project_path,
                "workspace": "codex-ccpanes-phase7-other"
            }),
        ),
        (
            "wrong-project-name",
            json!({
                "id": "project-id",
                "name": "different-project",
                "path": project_path,
                "workspace": workspace_name
            }),
        ),
    ];

    for (case, project) in cases {
        let server = FakeCcpanesServer::start(FakeCcpanesBehavior {
            workspaces: vec![json!({"id": "workspace-id", "name": workspace_name})],
            projects: vec![project],
            ..FakeCcpanesBehavior::default()
        })
        .unwrap();
        let mut adapter = CcpanesAdapter::new(
            CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
            short_timeouts(),
        )
        .unwrap();

        let report = run_dedicated_workspace_project_gate(
            &mut adapter,
            DedicatedWorkspaceSpec {
                workspace_name: workspace_name.to_owned(),
                project_path: repo.path().to_string_lossy().into_owned(),
                project_name: "phase7-project".to_owned(),
            },
        );

        assert_eq!(
            report.readiness,
            DedicatedWorkspaceReadiness::RequiresSetup,
            "{case}"
        );
        assert!(report.project.is_none(), "{case}");
        assert!(server.records().iter().all(|record| {
            !matches!(
                record.tool_name.as_deref(),
                Some(TOOL_CREATE_WORKSPACE | TOOL_ADD_PROJECT_TO_WORKSPACE | TOOL_LAUNCH_TASK)
            )
        }));
    }
}

#[test]
fn dedicated_workspace_project_gate_requires_workspace_project_capabilities() {
    let repo = tempdir().unwrap();
    let tools = default_fake_tools()
        .into_iter()
        .filter(|tool| {
            tool.get("name").and_then(serde_json::Value::as_str) != Some(TOOL_LIST_WORKSPACES)
        })
        .collect();
    let server = FakeCcpanesServer::start(FakeCcpanesBehavior {
        tools,
        ..FakeCcpanesBehavior::default()
    })
    .unwrap();
    let mut adapter = CcpanesAdapter::new(
        CcpanesEndpointConfig::new(server.endpoint(), Vec::new(), 1).unwrap(),
        short_timeouts(),
    )
    .unwrap();

    let report = run_dedicated_workspace_project_gate(
        &mut adapter,
        DedicatedWorkspaceSpec {
            workspace_name: "codex-ccpanes-phase7-missing-tool".to_owned(),
            project_path: repo.path().to_string_lossy().into_owned(),
            project_name: "missing-tool-project".to_owned(),
        },
    );

    assert_eq!(report.readiness, DedicatedWorkspaceReadiness::Incompatible);
    assert_eq!(
        report.error.as_ref().unwrap().code,
        ErrorCode::AdapterIncompatible
    );
    assert!(report
        .error
        .as_ref()
        .unwrap()
        .message
        .contains(TOOL_LIST_WORKSPACES));
    assert!(server
        .records()
        .iter()
        .all(|record| record.tool_name.as_deref() != Some(TOOL_LIST_PROJECTS)));
}
