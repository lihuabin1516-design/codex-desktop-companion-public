use coordination_core::{
    CallerId, CoordinationCommand, CoordinationError, CoordinationQuery, ErrorCode, IdempotencyKey,
    RequestId, WorkerExecutor,
};
use coordination_protocol::{
    mcp_tool_description, mcp_tool_input_schema, mcp_tool_output_schema, CcpanesPreflightInput,
    CcpanesSyntheticCanaryInput, EntityStatusInput, IpcEnvelope, IpcMessage, IpcResponse,
    McpToolKind, McpToolName, RevisionCommandInput, WorkerCancelInput, WorkerDispatchInput,
    WorkflowCreateInput, WorkflowEventsInput, WorkflowResultInput, COMMAND_ACK_SCHEMA_VERSION,
    IPC_SCHEMA_VERSION, MCP_PROTOCOL_VERSION, MCP_SCHEMA_VERSION, WORKFLOW_CANCEL_SCHEMA_VERSION,
    WORKFLOW_START_SCHEMA_VERSION,
};
use serde_json::{json, Value};

use crate::ipc_client::CoordinatorIpcClient;
use crate::jsonrpc::{JsonRpcRequest, JsonRpcResponse};
use crate::MCP_SERVER_NAME;

pub struct McpServer<C> {
    client: C,
    caller_id: CallerId,
    next_ipc_sequence: u64,
}

impl<C> McpServer<C> {
    pub fn new(client: C, caller_id: CallerId) -> Self {
        Self {
            client,
            caller_id,
            next_ipc_sequence: 0,
        }
    }

    pub fn client_mut(&mut self) -> &mut C {
        &mut self.client
    }
}

impl<C> McpServer<C>
where
    C: CoordinatorIpcClient,
{
    pub fn handle(&mut self, request: JsonRpcRequest) -> Option<JsonRpcResponse> {
        if request.jsonrpc != "2.0" {
            return request
                .id
                .map(|id| JsonRpcResponse::error(id, -32600, "invalid JSON-RPC version", None));
        }
        let id = request.id?;
        let result = match request.method.as_str() {
            "initialize" => Ok(initialize_result()),
            "ping" | "notifications/initialized" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_definitions() })),
            "tools/call" => self.call_tool(&request.params),
            _ => Err(method_error(
                -32601,
                format!("method not found: {}", request.method),
            )),
        };
        Some(match result {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(error) => JsonRpcResponse::error(id, error.code, error.message, error.data),
        })
    }

    fn call_tool(&mut self, params: &Value) -> Result<Value, McpRpcError> {
        let name = params.get("name").and_then(Value::as_str).ok_or_else(|| {
            coordination_rpc_error(CoordinationError::invalid_request(
                "tools/call requires name",
            ))
        })?;
        let tool_name = McpToolName::parse(name).map_err(coordination_rpc_error)?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let (request_id, message) = self
            .message_for_tool(tool_name, arguments)
            .map_err(coordination_rpc_error)?;
        let envelope = IpcEnvelope {
            schema_version: IPC_SCHEMA_VERSION.to_owned(),
            request_id,
            message,
        };
        let response = self
            .client
            .exchange(envelope)
            .map_err(coordination_rpc_error)?;
        let structured = ipc_response_to_value(response).map_err(coordination_rpc_error)?;
        tool_call_result(structured).map_err(coordination_rpc_error)
    }

    fn message_for_tool(
        &mut self,
        tool_name: McpToolName,
        arguments: Value,
    ) -> Result<(RequestId, IpcMessage), CoordinationError> {
        match tool_name {
            McpToolName::CoordinatorStatus => {
                EntityStatusInput::from_value(arguments)?;
                Ok((
                    self.next_request_id("coordinator-status")?,
                    IpcMessage::query(CoordinationQuery::CoordinatorStatus),
                ))
            }
            McpToolName::AdapterStatus => {
                EntityStatusInput::from_value(arguments)?;
                Ok((
                    self.next_request_id("adapter-status")?,
                    IpcMessage::query(CoordinationQuery::AdapterStatus),
                ))
            }
            McpToolName::CcpanesPreflight => {
                let input = CcpanesPreflightInput::from_value(arguments)?;
                Ok((
                    self.next_request_id("ccpanes-preflight")?,
                    IpcMessage::CcpanesPreflight(input),
                ))
            }
            McpToolName::CcpanesSyntheticCanary => {
                let input = CcpanesSyntheticCanaryInput::from_value(arguments)?;
                Ok((
                    input.request_id.clone(),
                    IpcMessage::CcpanesSyntheticCanary(input),
                ))
            }
            McpToolName::WorkerStatus => {
                let input = EntityStatusInput::from_value(arguments)?;
                let worker_id = input.worker_id.ok_or_else(|| {
                    CoordinationError::invalid_request("worker_status requires workerId")
                })?;
                Ok((
                    self.next_request_id("worker-status")?,
                    IpcMessage::query(CoordinationQuery::WorkerStatus { worker_id }),
                ))
            }
            McpToolName::WorkflowStatus => {
                let input = EntityStatusInput::from_value(arguments)?;
                let workflow_id = input.workflow_id.ok_or_else(|| {
                    CoordinationError::invalid_request("workflow_status requires workflowId")
                })?;
                Ok((
                    self.next_request_id("workflow-status")?,
                    IpcMessage::query(CoordinationQuery::WorkflowStatus { workflow_id }),
                ))
            }
            McpToolName::WorkflowEvents => {
                let input = WorkflowEventsInput::from_value(arguments)?;
                Ok((
                    self.next_request_id("workflow-events")?,
                    IpcMessage::query(CoordinationQuery::WorkflowEvents {
                        workflow_id: input.workflow_id,
                        after_sequence: input.after_sequence,
                        limit: input.limit,
                    }),
                ))
            }
            McpToolName::WorkflowResult => {
                let input = WorkflowResultInput::from_value(arguments)?;
                Ok((
                    self.next_request_id("workflow-result")?,
                    IpcMessage::query(CoordinationQuery::WorkflowResult {
                        workflow_id: input.workflow_id,
                    }),
                ))
            }
            McpToolName::WorkerDispatch => {
                let input = WorkerDispatchInput::from_value(arguments)?;
                let request_id = input.request_id.clone();
                let idempotency_key = input.idempotency_key(self.caller_id.clone())?;
                let repository = input.repository_ref()?;
                Ok((
                    request_id,
                    IpcMessage::command(CoordinationCommand::WorkerDispatch {
                        idempotency_key,
                        workflow_id: None,
                        repository,
                        objective: input.objective,
                        access: input.access,
                        executor: WorkerExecutor {
                            cli_tool: input.executor.cli_tool,
                        },
                        timeout_seconds: input.timeout_seconds,
                    }),
                ))
            }
            McpToolName::WorkerCancel => {
                let input = WorkerCancelInput::from_value(arguments)?;
                Ok((
                    input.request_id,
                    IpcMessage::command(CoordinationCommand::WorkerCancel {
                        worker_id: input.worker_id,
                        expected_revision: input.expected_revision,
                        reason: input.reason,
                    }),
                ))
            }
            McpToolName::WorkflowCreate => {
                let input = WorkflowCreateInput::from_value(arguments)?;
                let idempotency_key = IdempotencyKey::try_new(
                    McpToolName::WorkflowCreate.as_str(),
                    self.caller_id.clone(),
                    input.request_id.clone(),
                )?;
                Ok((
                    input.request_id,
                    IpcMessage::command(CoordinationCommand::WorkflowCreate {
                        idempotency_key,
                        spec: input.spec,
                    }),
                ))
            }
            McpToolName::WorkflowStart => {
                let input =
                    RevisionCommandInput::from_value(arguments, WORKFLOW_START_SCHEMA_VERSION)?;
                Ok((
                    input.request_id,
                    IpcMessage::command(CoordinationCommand::WorkflowStart {
                        workflow_id: input.workflow_id,
                        expected_revision: input.expected_revision,
                    }),
                ))
            }
            McpToolName::WorkflowCancel => {
                let input =
                    RevisionCommandInput::from_value(arguments, WORKFLOW_CANCEL_SCHEMA_VERSION)?;
                Ok((
                    input.request_id,
                    IpcMessage::command(CoordinationCommand::WorkflowCancel {
                        workflow_id: input.workflow_id,
                        expected_revision: input.expected_revision,
                        reason: input.reason,
                    }),
                ))
            }
        }
    }

    fn next_request_id(&mut self, prefix: &'static str) -> Result<RequestId, CoordinationError> {
        self.next_ipc_sequence = self.next_ipc_sequence.saturating_add(1);
        RequestId::try_new(format!("{prefix}-{}", self.next_ipc_sequence))
    }
}

#[derive(Debug, Clone)]
struct McpRpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

fn method_error(code: i64, message: impl Into<String>) -> McpRpcError {
    McpRpcError {
        code,
        message: message.into(),
        data: None,
    }
}

fn coordination_rpc_error(error: CoordinationError) -> McpRpcError {
    let code = match error.code {
        ErrorCode::InvalidRequest | ErrorCode::SchemaVersionUnsupported => -32602,
        _ => -32000,
    };
    let message = error.message.clone();
    McpRpcError {
        code,
        message,
        data: Some(json!({ "coordinationError": error })),
    }
}

pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {
            "tools": {
                "listChanged": false
            }
        },
        "serverInfo": {
            "name": MCP_SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION")
        },
        "coordinationSchemaVersion": MCP_SCHEMA_VERSION,
        "instructions": "Use high-level Codex + CC-Panes coordination tools. Coordinator owns Workflow state; this MCP server is a stateless IPC boundary."
    })
}

pub fn tool_definitions() -> Vec<Value> {
    McpToolName::ALL
        .iter()
        .copied()
        .map(|tool| {
            json!({
                "name": tool.as_str(),
                "description": mcp_tool_description(tool),
                "inputSchema": mcp_tool_input_schema(tool),
                "outputSchema": mcp_tool_output_schema(tool),
                "annotations": {
                    "readOnlyHint": tool.kind() == McpToolKind::Query
                }
            })
        })
        .collect()
}

fn ipc_response_to_value(response: IpcResponse) -> Result<Value, CoordinationError> {
    match response {
        IpcResponse::Json { value } => Ok(value),
        IpcResponse::Health(value) => serde_json::to_value(value).map_err(serialize_error),
        IpcResponse::Hello { protocol_version } => {
            Ok(json!({ "protocolVersion": protocol_version }))
        }
        IpcResponse::Accepted => Ok(json!({
            "schemaVersion": COMMAND_ACK_SCHEMA_VERSION,
            "accepted": true,
            "revision": 0
        })),
    }
}

fn tool_call_result(structured: Value) -> Result<Value, CoordinationError> {
    let text = serde_json::to_string_pretty(&structured).map_err(serialize_error)?;
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    }))
}

fn serialize_error(error: serde_json::Error) -> CoordinationError {
    CoordinationError::new(
        ErrorCode::InternalInvariantViolation,
        format!("serialize MCP payload failed: {error}"),
    )
}

pub fn default_caller_id() -> CallerId {
    CallerId::try_new("codex-app").expect("static caller id is valid")
}
