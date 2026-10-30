use serde_json::{json, Value};

use crate::{
    output_schema_version, McpToolName, PANEFORGE_MAX_CANARY_POLL_INTERVAL_MS,
    PANEFORGE_MAX_CANARY_TIMEOUT_MS, PANEFORGE_MAX_CONNECT_TIMEOUT_MS, PANEFORGE_MAX_REQUEST_TIMEOUT_MS,
    PANEFORGE_PREFLIGHT_REQUEST_SCHEMA_VERSION, PANEFORGE_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION,
    COMMAND_ACK_SCHEMA_VERSION, EVENT_PAGE_SCHEMA_VERSION, EVENT_QUERY_SCHEMA_VERSION,
    IPC_SCHEMA_VERSION, RESULT_QUERY_SCHEMA_VERSION, RESULT_RESPONSE_SCHEMA_VERSION,
    STATUS_QUERY_SCHEMA_VERSION, STATUS_RESPONSE_SCHEMA_VERSION, WORKER_CANCEL_SCHEMA_VERSION,
    WORKER_DISPATCH_ACCEPTED_SCHEMA_VERSION, WORKER_DISPATCH_SCHEMA_VERSION,
    WORKFLOW_CANCEL_SCHEMA_VERSION, WORKFLOW_CREATE_SCHEMA_VERSION, WORKFLOW_START_SCHEMA_VERSION,
};

pub fn ipc_envelope_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Coordination IPC envelope v1beta1",
        "type": "object",
        "required": ["schemaVersion", "requestId", "message"],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "const": IPC_SCHEMA_VERSION },
            "requestId": { "type": "string", "minLength": 1 },
            "message": { "type": "object" }
        }
    })
}

pub fn mcp_tool_input_schema(name: McpToolName) -> Value {
    match name {
        McpToolName::PaneForgePreflight => paneforge_preflight_input_schema(),
        McpToolName::PaneForgeSyntheticCanary => paneforge_synthetic_canary_input_schema(),
        McpToolName::WorkerDispatch => worker_dispatch_input_schema(),
        McpToolName::WorkflowCreate => workflow_create_input_schema(),
        McpToolName::WorkerCancel => worker_cancel_input_schema(),
        McpToolName::WorkflowStart => revision_command_input_schema(WORKFLOW_START_SCHEMA_VERSION),
        McpToolName::WorkflowCancel => {
            revision_command_input_schema(WORKFLOW_CANCEL_SCHEMA_VERSION)
        }
        McpToolName::WorkflowEvents => workflow_events_input_schema(),
        McpToolName::WorkflowResult => workflow_result_input_schema(),
        McpToolName::CoordinatorStatus
        | McpToolName::AdapterStatus
        | McpToolName::WorkerStatus
        | McpToolName::WorkflowStatus => status_query_input_schema(),
    }
}

pub fn mcp_tool_output_schema(name: McpToolName) -> Value {
    match name {
        McpToolName::PaneForgePreflight => paneforge_preflight_output_schema(),
        McpToolName::PaneForgeSyntheticCanary => paneforge_synthetic_canary_output_schema(),
        McpToolName::WorkerDispatch => worker_dispatch_output_schema(),
        McpToolName::WorkflowEvents => versioned_object_schema(EVENT_PAGE_SCHEMA_VERSION),
        McpToolName::WorkflowResult => workflow_result_output_schema(),
        McpToolName::CoordinatorStatus
        | McpToolName::AdapterStatus
        | McpToolName::WorkerStatus
        | McpToolName::WorkflowStatus => versioned_object_schema(STATUS_RESPONSE_SCHEMA_VERSION),
        McpToolName::WorkerCancel
        | McpToolName::WorkflowCreate
        | McpToolName::WorkflowStart
        | McpToolName::WorkflowCancel => command_ack_output_schema(),
    }
}

fn paneforge_preflight_input_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "paneforge_preflight input v1beta1",
        "type": "object",
        "required": ["schemaVersion"],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "const": PANEFORGE_PREFLIGHT_REQUEST_SCHEMA_VERSION },
            "discoveryPath": {
                "type": ["string", "null"],
                "minLength": 1,
                "maxLength": 4096,
                "default": null
            },
            "discoveryGeneration": {
                "type": "integer",
                "minimum": 1,
                "default": 1
            },
            "connectTimeoutMs": {
                "type": "integer",
                "minimum": 1,
                "maximum": PANEFORGE_MAX_CONNECT_TIMEOUT_MS,
                "default": 2000
            },
            "requestTimeoutMs": {
                "type": "integer",
                "minimum": 1,
                "maximum": PANEFORGE_MAX_REQUEST_TIMEOUT_MS,
                "default": 8000
            }
        }
    })
}

fn paneforge_synthetic_canary_input_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "paneforge_synthetic_canary input v1beta1",
        "type": "object",
        "required": [
            "schemaVersion",
            "requestId",
            "projectPath",
            "workspaceName",
            "projectName",
            "marker"
        ],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "const": PANEFORGE_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION },
            "requestId": {
                "type": "string",
                "minLength": 1,
                "maxLength": 160
            },
            "discoveryPath": {
                "type": ["string", "null"],
                "minLength": 1,
                "maxLength": 4096,
                "default": null
            },
            "projectPath": {
                "type": "string",
                "minLength": 1,
                "maxLength": 4096
            },
            "workspaceName": {
                "type": "string",
                "minLength": 1,
                "maxLength": 96
            },
            "projectName": {
                "type": "string",
                "minLength": 1,
                "maxLength": 96
            },
            "marker": {
                "type": "string",
                "minLength": 1,
                "maxLength": 1024
            },
            "timeoutMs": {
                "type": "integer",
                "minimum": 1,
                "maximum": PANEFORGE_MAX_CANARY_TIMEOUT_MS,
                "default": 420000
            },
            "pollIntervalMs": {
                "type": "integer",
                "minimum": 1,
                "maximum": PANEFORGE_MAX_CANARY_POLL_INTERVAL_MS,
                "default": 750
            },
            "connectTimeoutMs": {
                "type": "integer",
                "minimum": 1,
                "maximum": PANEFORGE_MAX_CONNECT_TIMEOUT_MS,
                "default": 2000
            },
            "requestTimeoutMs": {
                "type": "integer",
                "minimum": 1,
                "maximum": PANEFORGE_MAX_REQUEST_TIMEOUT_MS,
                "default": 12000
            }
        }
    })
}

fn paneforge_preflight_output_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "paneforge_preflight output v1beta1",
        "type": "object",
        "required": [
            "contractVersion",
            "adapterContractVersion",
            "readiness",
            "endpoint",
            "readOnlyOperations",
            "externalWorkerSideEffects",
            "status"
        ],
        "additionalProperties": false,
        "properties": {
            "contractVersion": {
                "const": output_schema_version(McpToolName::PaneForgePreflight)
            },
            "adapterContractVersion": { "type": "string", "minLength": 1 },
            "readiness": { "enum": ["READY", "DEGRADED", "INCOMPATIBLE"] },
            "endpoint": { "type": "object" },
            "readOnlyOperations": {
                "type": "array",
                "items": { "type": "string", "minLength": 1 }
            },
            "externalWorkerSideEffects": { "type": "boolean" },
            "status": { "type": "object" },
            "sessionInventory": { "type": ["object", "null"] },
            "error": { "type": ["object", "null"] }
        }
    })
}

fn paneforge_synthetic_canary_output_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "paneforge_synthetic_canary output v1beta1",
        "type": "object",
        "required": [
            "contractVersion",
            "workspaceGateContractVersion",
            "workspaceGateReadiness",
            "workspaceSetupSideEffects",
            "launchWorkspaceName",
            "repositoryHeadDigest",
            "stateStoreDigest",
            "sessionPreserved",
            "canary"
        ],
        "additionalProperties": false,
        "properties": {
            "contractVersion": {
                "const": output_schema_version(McpToolName::PaneForgeSyntheticCanary)
            },
            "workspaceGateContractVersion": { "type": "string", "minLength": 1 },
            "workspaceGateReadiness": {
                "enum": ["READY", "REQUIRES_SETUP", "DEGRADED", "INCOMPATIBLE"]
            },
            "workspaceSetupSideEffects": { "type": "boolean" },
            "sourceWorkspaceName": { "type": ["string", "null"], "minLength": 1 },
            "launchWorkspaceName": { "type": "string", "minLength": 1 },
            "repositoryHeadDigest": { "type": "string", "minLength": 1 },
            "stateStoreDigest": { "type": "string", "minLength": 1 },
            "sessionPreserved": { "type": "boolean" },
            "canary": { "type": "object" }
        }
    })
}

pub fn worker_dispatch_input_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "worker_dispatch input v1beta1",
        "type": "object",
        "required": ["schemaVersion", "requestId", "repository", "objective", "executor", "timeoutSeconds", "result"],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "const": WORKER_DISPATCH_SCHEMA_VERSION },
            "requestId": { "type": "string", "minLength": 1 },
            "repository": {
                "type": "object",
                "required": ["path", "dirtyPolicy"],
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string", "minLength": 1 },
                    "expectedHead": { "type": ["string", "null"] },
                    "dirtyPolicy": { "enum": ["reject", "allow-declared"] },
                    "workspaceName": { "type": ["string", "null"], "minLength": 1 }
                }
            },
            "objective": { "type": "string", "minLength": 1 },
            "access": { "enum": ["read", "write"] },
            "executor": {
                "type": "object",
                "required": ["cliTool"],
                "additionalProperties": false,
                "properties": { "cliTool": { "const": "codex" } }
            },
            "timeoutSeconds": { "type": "integer", "minimum": 1 },
            "result": {
                "type": "object",
                "required": ["format"],
                "additionalProperties": false,
                "properties": { "format": { "const": "structured-summary" } }
            }
        }
    })
}

fn worker_dispatch_output_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "worker_dispatch accepted output v1beta1",
        "type": "object",
        "required": ["schemaVersion", "accepted", "workflowId", "workerId", "revision", "state"],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "const": WORKER_DISPATCH_ACCEPTED_SCHEMA_VERSION },
            "accepted": { "const": true },
            "workflowId": { "type": "string", "minLength": 1 },
            "workerId": { "type": "string", "minLength": 1 },
            "revision": { "type": "integer", "minimum": 0 },
            "state": { "const": "QUEUED" }
        }
    })
}

fn workflow_create_input_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "workflow_create input v1beta1",
        "type": "object",
        "required": ["schemaVersion", "requestId", "spec"],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "const": WORKFLOW_CREATE_SCHEMA_VERSION },
            "requestId": { "type": "string", "minLength": 1 },
            "spec": { "type": "object" }
        }
    })
}

fn revision_command_input_schema(schema_version: &'static str) -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "revision command input v1beta1",
        "type": "object",
        "required": ["schemaVersion", "requestId", "workflowId", "expectedRevision"],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "const": schema_version },
            "requestId": { "type": "string", "minLength": 1 },
            "workflowId": { "type": "string", "minLength": 1 },
            "expectedRevision": { "type": "integer", "minimum": 0 },
            "reason": { "type": ["string", "null"] }
        }
    })
}

fn worker_cancel_input_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "worker_cancel input v1beta1",
        "type": "object",
        "required": ["schemaVersion", "requestId", "workerId"],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "const": WORKER_CANCEL_SCHEMA_VERSION },
            "requestId": { "type": "string", "minLength": 1 },
            "workerId": { "type": "string", "minLength": 1 },
            "expectedRevision": { "type": ["integer", "null"], "minimum": 0 },
            "reason": { "type": ["string", "null"] }
        }
    })
}

fn status_query_input_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "status query input v1beta1",
        "type": "object",
        "required": ["schemaVersion"],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "const": STATUS_QUERY_SCHEMA_VERSION },
            "workflowId": { "type": ["string", "null"] },
            "workerId": { "type": ["string", "null"] }
        }
    })
}

fn workflow_events_input_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "workflow_events input v1beta1",
        "type": "object",
        "required": ["schemaVersion", "workflowId", "afterSequence", "limit"],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "const": EVENT_QUERY_SCHEMA_VERSION },
            "workflowId": { "type": "string", "minLength": 1 },
            "afterSequence": { "type": "integer", "minimum": 0 },
            "limit": { "type": "integer", "minimum": 1, "maximum": 1000 }
        }
    })
}

fn workflow_result_input_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "workflow_result input v1beta1",
        "type": "object",
        "required": ["schemaVersion", "workflowId"],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "const": RESULT_QUERY_SCHEMA_VERSION },
            "workflowId": { "type": "string", "minLength": 1 }
        }
    })
}

fn command_ack_output_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "command acknowledgement output v1beta1",
        "type": "object",
        "required": ["schemaVersion", "accepted", "revision"],
        "additionalProperties": true,
        "properties": {
            "schemaVersion": { "const": COMMAND_ACK_SCHEMA_VERSION },
            "accepted": { "type": "boolean" },
            "revision": { "type": "integer", "minimum": 0 }
        }
    })
}

fn workflow_result_output_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "workflow_result output v1beta1",
        "type": "object",
        "required": ["schemaVersion", "workflowId", "state", "revision", "workers", "missingResults", "failedWorkers"],
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "const": RESULT_RESPONSE_SCHEMA_VERSION },
            "workflowId": { "type": "string", "minLength": 1 },
            "state": { "enum": ["complete", "partial", "pending", "unavailable"] },
            "revision": { "type": "integer", "minimum": 0 },
            "workers": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["workerId", "stepId", "state", "summary", "completedAtMs", "truncated"],
                    "additionalProperties": false,
                    "properties": {
                        "workerId": { "type": "string", "minLength": 1 },
                        "stepId": { "type": "string", "minLength": 1 },
                        "state": { "enum": ["SUCCEEDED", "FAILED", "CANCELLED"] },
                        "summary": { "type": "string" },
                        "completedAtMs": { "type": "integer", "minimum": 0 },
                        "truncated": { "type": "boolean" },
                        "artifactSha256": { "type": ["string", "null"] }
                    }
                }
            },
            "missingResults": {
                "type": "array",
                "items": { "type": "string", "minLength": 1 }
            },
            "failedWorkers": {
                "type": "array",
                "items": { "type": "string", "minLength": 1 }
            }
        }
    })
}

fn versioned_object_schema(version: &'static str) -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "required": ["schemaVersion"],
        "properties": {
            "schemaVersion": { "const": version }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{public_mcp_tool_contracts, McpToolKind};

    #[test]
    fn worker_dispatch_schema_snapshot_contains_required_contract_fields() {
        let schema = worker_dispatch_input_schema();
        assert_eq!(
            schema["properties"]["schemaVersion"]["const"],
            WORKER_DISPATCH_SCHEMA_VERSION
        );
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("requestId")));
        assert_eq!(
            schema["properties"]["executor"]["properties"]["cliTool"]["const"],
            "codex"
        );
        assert_eq!(
            schema["properties"]["repository"]["properties"]["workspaceName"]["type"],
            json!(["string", "null"])
        );
    }

    #[test]
    fn paneforge_input_schemas_are_closed_and_match_dto_defaults() {
        let preflight = mcp_tool_input_schema(McpToolName::PaneForgePreflight);
        assert_eq!(preflight["additionalProperties"], false);
        assert_eq!(
            preflight["properties"]["schemaVersion"]["const"],
            PANEFORGE_PREFLIGHT_REQUEST_SCHEMA_VERSION
        );
        assert_eq!(
            preflight["properties"]["discoveryPath"]["default"],
            Value::Null
        );
        assert_eq!(preflight["properties"]["discoveryGeneration"]["default"], 1);
        assert_eq!(
            preflight["properties"]["connectTimeoutMs"]["default"],
            2_000
        );
        assert_eq!(
            preflight["properties"]["connectTimeoutMs"]["maximum"],
            30_000
        );
        assert_eq!(
            preflight["properties"]["requestTimeoutMs"]["default"],
            8_000
        );
        assert_eq!(
            preflight["properties"]["requestTimeoutMs"]["maximum"],
            120_000
        );
        assert_eq!(preflight["properties"]["discoveryGeneration"]["minimum"], 1);

        let canary = mcp_tool_input_schema(McpToolName::PaneForgeSyntheticCanary);
        assert_eq!(canary["additionalProperties"], false);
        assert_eq!(
            canary["properties"]["schemaVersion"]["const"],
            PANEFORGE_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION
        );
        assert_eq!(
            canary["properties"]["discoveryPath"]["default"],
            Value::Null
        );
        assert_eq!(canary["properties"]["projectPath"]["maxLength"], 4_096);
        assert_eq!(canary["properties"]["workspaceName"]["maxLength"], 96);
        assert_eq!(canary["properties"]["projectName"]["maxLength"], 96);
        assert_eq!(canary["properties"]["marker"]["maxLength"], 1_024);
        assert_eq!(canary["properties"]["timeoutMs"]["default"], 420_000);
        assert_eq!(canary["properties"]["timeoutMs"]["maximum"], 900_000);
        assert_eq!(canary["properties"]["pollIntervalMs"]["default"], 750);
        assert_eq!(canary["properties"]["pollIntervalMs"]["maximum"], 10_000);
        assert_eq!(canary["properties"]["connectTimeoutMs"]["default"], 2_000);
        assert_eq!(canary["properties"]["connectTimeoutMs"]["maximum"], 30_000);
        assert_eq!(canary["properties"]["requestTimeoutMs"]["default"], 12_000);
        assert_eq!(canary["properties"]["requestTimeoutMs"]["maximum"], 120_000);
        for required in [
            "schemaVersion",
            "requestId",
            "projectPath",
            "workspaceName",
            "projectName",
            "marker",
        ] {
            assert!(canary["required"]
                .as_array()
                .unwrap()
                .contains(&json!(required)));
        }
    }

    #[test]
    fn paneforge_output_schemas_are_closed_and_require_public_surfaces() {
        let preflight = mcp_tool_output_schema(McpToolName::PaneForgePreflight);
        assert_eq!(preflight["additionalProperties"], false);
        assert_eq!(
            preflight["properties"]["contractVersion"]["const"],
            "paneforge-adapter-readonly-preflight/v1beta1"
        );
        for required in [
            "contractVersion",
            "readiness",
            "externalWorkerSideEffects",
            "status",
        ] {
            assert!(preflight["required"]
                .as_array()
                .unwrap()
                .contains(&json!(required)));
        }

        let canary = mcp_tool_output_schema(McpToolName::PaneForgeSyntheticCanary);
        assert_eq!(canary["additionalProperties"], false);
        assert_eq!(
            canary["properties"]["contractVersion"]["const"],
            "coordination.real-paneforge-canary-runner/v1beta1"
        );
        for required in [
            "contractVersion",
            "workspaceSetupSideEffects",
            "launchWorkspaceName",
            "sessionPreserved",
            "canary",
        ] {
            assert!(canary["required"]
                .as_array()
                .unwrap()
                .contains(&json!(required)));
        }
    }

    #[test]
    fn every_public_mcp_tool_has_input_and_output_schema_version() {
        for contract in public_mcp_tool_contracts() {
            assert!(
                !contract.input_schema_version.is_empty(),
                "{} input",
                contract.name
            );
            assert!(
                !contract.output_schema_version.is_empty(),
                "{} output",
                contract.name
            );
            let name = McpToolName::ALL
                .iter()
                .copied()
                .find(|candidate| candidate.as_str() == contract.name)
                .unwrap();
            let input = mcp_tool_input_schema(name);
            let output = mcp_tool_output_schema(name);
            assert!(input["required"]
                .as_array()
                .unwrap()
                .contains(&json!("schemaVersion")));
            let output_version = output["properties"]["schemaVersion"]["const"]
                .as_str()
                .or_else(|| output["properties"]["contractVersion"]["const"].as_str());
            assert_eq!(
                output_version,
                Some(contract.output_schema_version.as_str())
            );
        }
    }

    #[test]
    fn command_and_query_tools_stay_classified() {
        let contracts = public_mcp_tool_contracts();
        let worker_dispatch = contracts
            .iter()
            .find(|contract| contract.name == "worker_dispatch")
            .unwrap();
        let workflow_result = contracts
            .iter()
            .find(|contract| contract.name == "workflow_result")
            .unwrap();
        assert_eq!(worker_dispatch.kind, McpToolKind::Command);
        assert_eq!(workflow_result.kind, McpToolKind::Query);
    }

    #[test]
    fn workflow_result_schema_contains_phase6_fan_in_fields() {
        let schema = workflow_result_output_schema();
        assert_eq!(
            schema["properties"]["schemaVersion"]["const"],
            RESULT_RESPONSE_SCHEMA_VERSION
        );
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("missingResults")));
        assert!(schema["properties"]["workers"]["items"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("stepId")));
    }
}
