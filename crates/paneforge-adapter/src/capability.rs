use std::collections::{BTreeMap, BTreeSet};

use coordination_core::{CoordinationError, ErrorCode};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{AdapterReadiness, CapabilitySnapshot, PANEFORGE_MCP_PROTOCOL_VERSION};

pub const TOOL_LAUNCH_TASK: &str = "launch_task";
pub const TOOL_LIST_SESSIONS: &str = "list_sessions";
pub const TOOL_LIST_PANES: &str = "list_panes";
pub const TOOL_GET_SESSION_STATUS: &str = "get_session_status";
pub const TOOL_GET_SESSION_OUTPUT: &str = "get_session_output";
pub const TOOL_SUBMIT_TO_SESSION: &str = "submit_to_session";
pub const TOOL_KILL_SESSION: &str = "kill_session";
pub const TOOL_WAIT_FOR_SESSION: &str = "wait_for_session";
pub const TOOL_WRITE_TO_SESSION: &str = "write_to_session";
pub const TOOL_LIST_WORKSPACES: &str = "list_workspaces";
pub const TOOL_LIST_PROJECTS: &str = "list_projects";
pub const TOOL_CREATE_WORKSPACE: &str = "create_workspace";
pub const TOOL_ADD_PROJECT_TO_WORKSPACE: &str = "add_project_to_workspace";

pub const BASELINE_REQUIRED_PANEFORGE_TOOLS: &[&str] =
    &[TOOL_LAUNCH_TASK, TOOL_LIST_SESSIONS, TOOL_LIST_PANES];

pub const DISPATCH_CAPABILITY_TOOLS: &[&str] = &[TOOL_GET_SESSION_STATUS, TOOL_GET_SESSION_OUTPUT];

pub const OPTIONAL_SESSION_CONTROL_TOOLS: &[&str] = &[
    TOOL_SUBMIT_TO_SESSION,
    TOOL_KILL_SESSION,
    TOOL_WAIT_FOR_SESSION,
    TOOL_WRITE_TO_SESSION,
];

const LAUNCH_TASK_USED_PROPERTIES: &[&str] =
    &["projectPath", "prompt", "title", "workspaceName", "cliTool"];

#[derive(Debug, Clone, Copy)]
enum ToolArgumentKind {
    String,
    Integer,
    StringArray,
}

pub fn negotiate_capabilities(
    initialize_result: &Value,
    tools_result: &Value,
) -> Result<CapabilitySnapshot, CoordinationError> {
    validate_protocol(initialize_result)?;
    let tools = tools_result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::AdapterIncompatible,
                "tools/list omitted tools array",
            )
        })?;

    let launch_task_properties = validate_tool_contract(
        tools,
        TOOL_LAUNCH_TASK,
        LAUNCH_TASK_USED_PROPERTIES,
        &["projectPath"],
    )?;
    for property in LAUNCH_TASK_USED_PROPERTIES {
        validate_tool_argument_kind(tools, TOOL_LAUNCH_TASK, property, ToolArgumentKind::String)?;
    }
    validate_tool_contract(tools, TOOL_LIST_SESSIONS, &[], &[])?;
    validate_tool_contract(tools, TOOL_LIST_PANES, &[], &[])?;
    let get_session_status_available = validate_optional_tool_contract(
        tools,
        TOOL_GET_SESSION_STATUS,
        &["sessionId"],
        &["sessionId"],
    )?
    .is_some();
    if get_session_status_available {
        validate_tool_argument_kind(
            tools,
            TOOL_GET_SESSION_STATUS,
            "sessionId",
            ToolArgumentKind::String,
        )?;
    }
    let get_session_output_available = validate_optional_tool_contract(
        tools,
        TOOL_GET_SESSION_OUTPUT,
        &["sessionId"],
        &["sessionId"],
    )?
    .is_some();
    if get_session_output_available {
        validate_tool_argument_kind(
            tools,
            TOOL_GET_SESSION_OUTPUT,
            "sessionId",
            ToolArgumentKind::String,
        )?;
    }
    let submit_to_session_available = validate_optional_tool_contract(
        tools,
        TOOL_SUBMIT_TO_SESSION,
        &["sessionId", "text"],
        &["sessionId", "text"],
    )?
    .is_some();
    if submit_to_session_available {
        validate_tool_argument_kind(
            tools,
            TOOL_SUBMIT_TO_SESSION,
            "sessionId",
            ToolArgumentKind::String,
        )?;
        validate_tool_argument_kind(
            tools,
            TOOL_SUBMIT_TO_SESSION,
            "text",
            ToolArgumentKind::String,
        )?;
    }
    let kill_session_available =
        validate_optional_tool_contract(tools, TOOL_KILL_SESSION, &["sessionId"], &["sessionId"])?
            .is_some();
    if kill_session_available {
        validate_tool_argument_kind(
            tools,
            TOOL_KILL_SESSION,
            "sessionId",
            ToolArgumentKind::String,
        )?;
    }
    let wait_for_session_available = validate_optional_tool_contract(
        tools,
        TOOL_WAIT_FOR_SESSION,
        &["sessionId", "launchId", "waitFor", "timeoutMs"],
        &["waitFor"],
    )?
    .is_some();
    if wait_for_session_available {
        validate_tool_argument_kind(
            tools,
            TOOL_WAIT_FOR_SESSION,
            "sessionId",
            ToolArgumentKind::String,
        )?;
        validate_tool_argument_kind(
            tools,
            TOOL_WAIT_FOR_SESSION,
            "waitFor",
            ToolArgumentKind::StringArray,
        )?;
        validate_tool_argument_kind(
            tools,
            TOOL_WAIT_FOR_SESSION,
            "timeoutMs",
            ToolArgumentKind::Integer,
        )?;
    }
    let write_to_session_available = validate_optional_tool_contract(
        tools,
        TOOL_WRITE_TO_SESSION,
        &["sessionId", "text"],
        &["sessionId", "text"],
    )?
    .is_some();
    if write_to_session_available {
        validate_tool_argument_kind(
            tools,
            TOOL_WRITE_TO_SESSION,
            "sessionId",
            ToolArgumentKind::String,
        )?;
        validate_tool_argument_kind(
            tools,
            TOOL_WRITE_TO_SESSION,
            "text",
            ToolArgumentKind::String,
        )?;
    }

    let names = tool_names(tools);
    let launch_task_accepts_client_request_id = tools
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some(TOOL_LAUNCH_TASK))
        .is_some_and(|tool| tool_accepts_argument(tool, "clientRequestId"));
    let kill_session_accepts_reason = tools
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some(TOOL_KILL_SESSION))
        .is_some_and(|tool| tool_accepts_argument(tool, "reason"));
    if kill_session_accepts_reason {
        validate_tool_argument_kind(tools, TOOL_KILL_SESSION, "reason", ToolArgumentKind::String)?;
    }

    Ok(CapabilitySnapshot {
        readiness: AdapterReadiness::Ready,
        server_version: initialize_result
            .get("serverInfo")
            .and_then(|value| value.get("version"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        protocol_version: initialize_result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .map(str::to_owned),
        tools: names,
        capability_digest: capability_digest(initialize_result, tools),
        launch_task_properties,
        launch_task_accepts_client_request_id,
        dispatch_ready: get_session_status_available && get_session_output_available,
        get_session_status_available,
        get_session_output_available,
        submit_to_session_available,
        kill_session_available,
        wait_for_session_available,
        write_to_session_available,
        kill_session_accepts_reason,
    })
}

pub fn validate_protocol(initialize_result: &Value) -> Result<(), CoordinationError> {
    let actual = initialize_result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if actual == PANEFORGE_MCP_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(CoordinationError::new(
            ErrorCode::AdapterIncompatible,
            format!("expected PaneForge MCP protocol {PANEFORGE_MCP_PROTOCOL_VERSION}, got {actual}"),
        ))
    }
}

pub fn validate_tool_contract(
    tools: &[Value],
    name: &str,
    expected_properties: &[&str],
    expected_required: &[&str],
) -> Result<Vec<String>, CoordinationError> {
    let tool = tools
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::AdapterIncompatible,
                format!("required PaneForge tool {name} is missing"),
            )
        })?;
    let schema = tool_schema(tool).ok_or_else(|| {
        CoordinationError::new(
            ErrorCode::AdapterIncompatible,
            format!("PaneForge tool {name} has no input schema"),
        )
    })?;
    if !schema_accepts_type(schema, &["object"]) {
        return Err(CoordinationError::new(
            ErrorCode::AdapterIncompatible,
            format!("PaneForge tool {name} input schema must accept an object"),
        ));
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let missing_properties = expected_properties
        .iter()
        .filter(|property| !properties.contains(**property))
        .copied()
        .collect::<Vec<_>>();
    let missing_required = expected_required
        .iter()
        .filter(|property| !required.contains(**property))
        .copied()
        .collect::<Vec<_>>();
    if !missing_properties.is_empty() || !missing_required.is_empty() {
        return Err(CoordinationError::new(
            ErrorCode::AdapterIncompatible,
            format!(
                "PaneForge tool {name} schema missing properties={missing_properties:?} required={missing_required:?}"
            ),
        ));
    }
    Ok(properties.into_iter().collect())
}

pub fn validate_optional_tool_contract(
    tools: &[Value],
    name: &str,
    expected_properties: &[&str],
    expected_required: &[&str],
) -> Result<Option<Vec<String>>, CoordinationError> {
    if tools
        .iter()
        .all(|tool| tool.get("name").and_then(Value::as_str) != Some(name))
    {
        return Ok(None);
    }
    validate_tool_contract(tools, name, expected_properties, expected_required).map(Some)
}

fn validate_tool_argument_kind(
    tools: &[Value],
    tool_name: &str,
    property_name: &str,
    expected: ToolArgumentKind,
) -> Result<(), CoordinationError> {
    let property = tools
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some(tool_name))
        .and_then(tool_schema)
        .and_then(|schema| schema.get("properties"))
        .and_then(Value::as_object)
        .and_then(|properties| properties.get(property_name))
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::AdapterIncompatible,
                format!("PaneForge tool {tool_name} schema omitted property {property_name}"),
            )
        })?;
    let compatible = match expected {
        ToolArgumentKind::String => schema_accepts_type(property, &["string"]),
        ToolArgumentKind::Integer => schema_accepts_type(property, &["integer", "number"]),
        ToolArgumentKind::StringArray => {
            schema_accepts_type(property, &["array"])
                && property
                    .get("items")
                    .is_some_and(|items| schema_accepts_type(items, &["string"]))
        }
    };
    if compatible {
        Ok(())
    } else {
        Err(CoordinationError::new(
            ErrorCode::AdapterIncompatible,
            format!(
                "PaneForge tool {tool_name} property {property_name} has incompatible input type"
            ),
        ))
    }
}

fn schema_accepts_type(schema: &Value, accepted: &[&str]) -> bool {
    match schema.get("type") {
        Some(Value::String(actual)) => accepted.contains(&actual.as_str()),
        Some(Value::Array(actual)) => actual
            .iter()
            .filter_map(Value::as_str)
            .any(|actual| accepted.contains(&actual)),
        _ => false,
    }
}

pub fn tool_accepts_argument(tool: &Value, argument: &str) -> bool {
    let Some(schema) = tool_schema(tool) else {
        return false;
    };
    schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|properties| properties.contains_key(argument))
        || schema
            .get("additionalProperties")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

pub fn default_fake_tools() -> Vec<Value> {
    let mut tools = vec![
        tool_schema_value(
            TOOL_LAUNCH_TASK,
            &[
                "projectPath",
                "prompt",
                "title",
                "workspaceName",
                "layoutName",
                "cliTool",
                "providerId",
                "providerSelection",
                "runtimeKind",
                "clientRequestId",
            ],
            &["projectPath"],
        ),
        tool_schema_value(TOOL_LIST_SESSIONS, &[], &[]),
        tool_schema_value(TOOL_LIST_PANES, &[], &[]),
        tool_schema_value(TOOL_LIST_WORKSPACES, &[], &[]),
        tool_schema_value(TOOL_LIST_PROJECTS, &[], &[]),
        tool_schema_value(TOOL_CREATE_WORKSPACE, &["name", "path"], &["name"]),
        tool_schema_value(
            TOOL_ADD_PROJECT_TO_WORKSPACE,
            &["workspaceName", "projectPath"],
            &["workspaceName", "projectPath"],
        ),
        tool_schema_value(TOOL_GET_SESSION_STATUS, &["sessionId"], &["sessionId"]),
        tool_schema_value(TOOL_GET_SESSION_OUTPUT, &["sessionId"], &["sessionId"]),
        tool_schema_value(
            TOOL_SUBMIT_TO_SESSION,
            &["sessionId", "text"],
            &["sessionId", "text"],
        ),
        tool_schema_value(TOOL_KILL_SESSION, &["sessionId", "reason"], &["sessionId"]),
        tool_schema_value(
            TOOL_WAIT_FOR_SESSION,
            &["sessionId", "launchId", "waitFor", "timeoutMs"],
            &["waitFor"],
        ),
        tool_schema_value(
            TOOL_WRITE_TO_SESSION,
            &["sessionId", "text"],
            &["sessionId", "text"],
        ),
    ];
    let wait_for_session = tools
        .iter_mut()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some(TOOL_WAIT_FOR_SESSION))
        .expect("default fake wait_for_session tool");
    wait_for_session["inputSchema"]["properties"]["waitFor"] =
        json!({"type": "array", "items": {"type": "string"}});
    wait_for_session["inputSchema"]["properties"]["timeoutMs"] =
        json!({"type": ["integer", "null"], "minimum": 0});
    tools
}

pub fn tool_schema_value(name: &str, properties: &[&str], required: &[&str]) -> Value {
    let property_map = properties
        .iter()
        .map(|property| ((*property).to_owned(), json!({"type": "string"})))
        .collect::<serde_json::Map<_, _>>();
    json!({
        "name": name,
        "inputSchema": {
            "type": "object",
            "properties": property_map,
            "required": required,
            "additionalProperties": false
        }
    })
}

fn tool_schema(tool: &Value) -> Option<&Value> {
    tool.get("inputSchema").or_else(|| tool.get("input_schema"))
}

fn tool_names(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

fn capability_digest(initialize_result: &Value, tools: &[Value]) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct DigestInput {
        protocol_version: Option<String>,
        tools: BTreeMap<String, Value>,
    }

    let normalized_tools = tools
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name")?.as_str()?.to_owned();
            let schema = tool_schema(tool).cloned().unwrap_or_else(|| json!({}));
            Some((name, schema))
        })
        .collect::<BTreeMap<_, _>>();
    let input = DigestInput {
        protocol_version: initialize_result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .map(str::to_owned),
        tools: normalized_tools,
    };
    let canonical = serde_json::to_string(&input).unwrap_or_default();
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn missing_tool_or_field_is_incompatible() {
        let tools = vec![tool_schema_value(
            TOOL_LAUNCH_TASK,
            &["projectPath"],
            &["projectPath"],
        )];
        let error = validate_tool_contract(
            &tools,
            TOOL_LAUNCH_TASK,
            &["projectPath", "providerId"],
            &["projectPath"],
        )
        .expect_err("missing field");
        assert_eq!(error.code, ErrorCode::AdapterIncompatible);
        assert!(error.message.contains("providerId"));

        let error =
            validate_tool_contract(&tools, TOOL_KILL_SESSION, &[], &[]).expect_err("missing tool");
        assert_eq!(error.code, ErrorCode::AdapterIncompatible);
    }

    #[test]
    fn additive_optional_tool_field_is_compatible() {
        let tools = vec![tool_schema_value(
            TOOL_LAUNCH_TASK,
            &["projectPath", "providerId", "futureOptionalField"],
            &["projectPath"],
        )];
        let accepted = validate_tool_contract(
            &tools,
            TOOL_LAUNCH_TASK,
            &["projectPath", "providerId"],
            &["projectPath"],
        )
        .unwrap();
        assert!(accepted.iter().any(|field| field == "futureOptionalField"));
    }

    #[test]
    fn tool_input_schema_root_must_accept_an_object() {
        let tools = vec![json!({
            "name": TOOL_LIST_SESSIONS,
            "inputSchema": {
                "type": "string"
            }
        })];

        let error = validate_tool_contract(&tools, TOOL_LIST_SESSIONS, &[], &[])
            .expect_err("non-object tool input schema must fail closed");

        assert_eq!(error.code, ErrorCode::AdapterIncompatible);
        assert!(error.message.contains(TOOL_LIST_SESSIONS));
        assert!(error.message.contains("object"));
    }

    #[test]
    fn unknown_protocol_version_is_adapter_incompatible() {
        let error = validate_protocol(&json!({"protocolVersion": "2099-01-01"}))
            .expect_err("unknown protocol");
        assert_eq!(error.code, ErrorCode::AdapterIncompatible);
    }
}
