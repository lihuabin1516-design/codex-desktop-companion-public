use coordination_core::{CoordinationError, ErrorCode};
use serde_json::{json, Map, Value};

use crate::capability::{
    negotiate_capabilities, TOOL_GET_SESSION_OUTPUT, TOOL_GET_SESSION_STATUS, TOOL_KILL_SESSION,
    TOOL_LAUNCH_TASK, TOOL_LIST_PANES, TOOL_LIST_SESSIONS, TOOL_SUBMIT_TO_SESSION,
    TOOL_WAIT_FOR_SESSION, TOOL_WRITE_TO_SESSION,
};
use crate::http::{HttpTransportFailure, McpHttpClient, TransportFailureKind, TransportPhase};
use crate::reconcile::match_unknown_dispatch;
use crate::redact::compact_safe_message;
use crate::types::{find_string, safe_adapter_error};
use crate::{
    AdapterReadiness, AdapterStatusSnapshot, AdapterTimeouts, CapabilitySnapshot,
    CcpanesEndpointConfig, DiscoveredSessionBinding, KillSessionOutcome, LaunchTaskResult,
    SessionSnapshot, CCPANES_ADAPTER_CONTRACT_VERSION, CCPANES_MCP_PROTOCOL_VERSION,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectBoundary {
    NoExternalWorker,
    ExternalWorkerMayHaveStarted,
}

#[derive(Debug, Clone)]
pub struct CcpanesAdapter {
    config: CcpanesEndpointConfig,
    transport: McpHttpClient,
    session_id: Option<String>,
    request_sequence: u64,
    capability: Option<CapabilitySnapshot>,
}

impl CcpanesAdapter {
    pub fn new(
        config: CcpanesEndpointConfig,
        timeouts: AdapterTimeouts,
    ) -> Result<Self, CoordinationError> {
        config.validate()?;
        let transport = McpHttpClient::new(&config, timeouts)
            .map_err(|error| map_transport_failure(error, EffectBoundary::NoExternalWorker))?;
        Ok(Self {
            config,
            transport,
            session_id: None,
            request_sequence: 0,
            capability: None,
        })
    }

    pub fn config(&self) -> &CcpanesEndpointConfig {
        &self.config
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn status_snapshot(&self) -> AdapterStatusSnapshot {
        AdapterStatusSnapshot {
            contract_version: CCPANES_ADAPTER_CONTRACT_VERSION.to_owned(),
            readiness: self
                .capability
                .as_ref()
                .map(|capability| capability.readiness)
                .unwrap_or(AdapterReadiness::Degraded),
            endpoint: self.config.sanitized(),
            capability: self.capability.clone(),
        }
    }

    pub fn discover(&mut self) -> Result<CapabilitySnapshot, CoordinationError> {
        let initialize = self.call_mcp(
            "initialize",
            json!({
                "protocolVersion": CCPANES_MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "codex-ccpanes-coordinator",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
            EffectBoundary::NoExternalWorker,
        )?;
        self.notify("notifications/initialized", json!({}))?;
        let tools = self.call_mcp("tools/list", json!({}), EffectBoundary::NoExternalWorker)?;
        let capability = negotiate_capabilities(&initialize, &tools)?;
        self.capability = Some(capability.clone());
        Ok(capability)
    }

    pub fn launch_task(
        &mut self,
        arguments: Value,
        capability: &CapabilitySnapshot,
    ) -> Result<LaunchTaskResult, CoordinationError> {
        if !capability.dispatch_ready {
            return Err(CoordinationError::new(
                ErrorCode::AdapterIncompatible,
                "CC-Panes launch capability requires get_session_status and get_session_output",
            ));
        }
        let arguments = filter_launch_arguments(arguments, capability);
        let result = self.call_tool(
            TOOL_LAUNCH_TASK,
            arguments,
            EffectBoundary::ExternalWorkerMayHaveStarted,
        )?;
        Ok(LaunchTaskResult {
            task_id: find_string(&result, &["taskId", "task_id", "id"]),
            session_id: find_string(&result, &["sessionId", "session_id"]),
            binding_id: find_string(&result, &["bindingId", "binding_id"]),
            runtime_kind: find_string(&result, &["runtimeKind", "runtime_kind"]),
        })
    }

    pub fn list_sessions(&mut self) -> Result<Value, CoordinationError> {
        self.call_tool(
            TOOL_LIST_SESSIONS,
            json!({}),
            EffectBoundary::NoExternalWorker,
        )
    }

    pub fn list_panes(&mut self) -> Result<Value, CoordinationError> {
        self.call_tool(TOOL_LIST_PANES, json!({}), EffectBoundary::NoExternalWorker)
    }

    pub fn list_workspaces(&mut self) -> Result<Value, CoordinationError> {
        self.call_tool(
            crate::TOOL_LIST_WORKSPACES,
            json!({}),
            EffectBoundary::NoExternalWorker,
        )
    }

    pub fn list_projects(&mut self) -> Result<Value, CoordinationError> {
        self.call_tool(
            crate::TOOL_LIST_PROJECTS,
            json!({}),
            EffectBoundary::NoExternalWorker,
        )
    }

    pub fn create_workspace(
        &mut self,
        name: &str,
        path: Option<&str>,
    ) -> Result<Value, CoordinationError> {
        self.require_tool(crate::TOOL_CREATE_WORKSPACE)?;
        let mut arguments = Map::new();
        arguments.insert("name".to_owned(), Value::String(name.to_owned()));
        if let Some(path) = path {
            arguments.insert("path".to_owned(), Value::String(path.to_owned()));
        }
        self.call_tool(
            crate::TOOL_CREATE_WORKSPACE,
            Value::Object(arguments),
            EffectBoundary::NoExternalWorker,
        )
    }

    pub fn add_project_to_workspace(
        &mut self,
        workspace_name: &str,
        project_path: &str,
    ) -> Result<Value, CoordinationError> {
        self.require_tool(crate::TOOL_ADD_PROJECT_TO_WORKSPACE)?;
        self.call_tool(
            crate::TOOL_ADD_PROJECT_TO_WORKSPACE,
            json!({
                "workspaceName": workspace_name,
                "projectPath": project_path
            }),
            EffectBoundary::NoExternalWorker,
        )
    }

    pub fn get_session_status(&mut self, session_id: &str) -> Result<Value, CoordinationError> {
        self.require_tool(TOOL_GET_SESSION_STATUS)?;
        self.call_tool(
            TOOL_GET_SESSION_STATUS,
            json!({"sessionId": session_id}),
            EffectBoundary::NoExternalWorker,
        )
    }

    pub fn get_session_output(&mut self, session_id: &str) -> Result<Value, CoordinationError> {
        self.require_tool(TOOL_GET_SESSION_OUTPUT)?;
        self.call_tool(
            TOOL_GET_SESSION_OUTPUT,
            json!({"sessionId": session_id}),
            EffectBoundary::NoExternalWorker,
        )
    }

    pub fn submit_to_session(
        &mut self,
        session_id: &str,
        text: &str,
    ) -> Result<Value, CoordinationError> {
        self.require_tool(TOOL_SUBMIT_TO_SESSION)?;
        self.call_tool(
            TOOL_SUBMIT_TO_SESSION,
            json!({"sessionId": session_id, "text": text}),
            EffectBoundary::NoExternalWorker,
        )
    }

    pub fn kill_session(
        &mut self,
        session_id: &str,
        reason: Option<&str>,
        accepts_reason: bool,
    ) -> Result<KillSessionOutcome, CoordinationError> {
        self.require_tool(TOOL_KILL_SESSION)?;
        match self.call_tool(
            TOOL_KILL_SESSION,
            kill_session_arguments(session_id, reason, accepts_reason),
            EffectBoundary::NoExternalWorker,
        ) {
            Ok(_) => Ok(KillSessionOutcome::Accepted),
            Err(error) if is_session_absent_error(&error) => Ok(KillSessionOutcome::AlreadyAbsent),
            Err(error) => Err(error),
        }
    }

    pub fn wait_for_session(
        &mut self,
        session_id: &str,
        wait_for: &[&str],
        timeout_ms: Option<u64>,
    ) -> Result<Value, CoordinationError> {
        self.require_tool(TOOL_WAIT_FOR_SESSION)?;
        let mut arguments = Map::new();
        arguments.insert("sessionId".to_owned(), Value::String(session_id.to_owned()));
        arguments.insert(
            "waitFor".to_owned(),
            Value::Array(
                wait_for
                    .iter()
                    .map(|status| Value::String((*status).to_owned()))
                    .collect(),
            ),
        );
        if let Some(timeout_ms) = timeout_ms {
            arguments.insert("timeoutMs".to_owned(), Value::from(timeout_ms));
        }
        self.call_tool(
            TOOL_WAIT_FOR_SESSION,
            Value::Object(arguments),
            EffectBoundary::NoExternalWorker,
        )
    }

    pub fn write_to_session(
        &mut self,
        session_id: &str,
        text: &str,
    ) -> Result<Value, CoordinationError> {
        self.require_tool(TOOL_WRITE_TO_SESSION)?;
        self.call_tool(
            TOOL_WRITE_TO_SESSION,
            json!({"sessionId": session_id, "text": text}),
            EffectBoundary::NoExternalWorker,
        )
    }

    pub fn reconcile_session(
        &mut self,
        session_id: &str,
    ) -> Result<SessionSnapshot, CoordinationError> {
        let status_result = self.get_session_status(session_id);
        if status_result
            .as_ref()
            .err()
            .is_some_and(is_session_absent_error)
        {
            return Ok(SessionSnapshot {
                session_id: session_id.to_owned(),
                status: "not_found".to_owned(),
                terminal: true,
                succeeded: false,
                output: None,
            });
        }
        let status_value = structured_result(&status_result?);
        let status = status_value
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| find_string(&status_value, &["state"]))
            .unwrap_or_else(|| "unknown".to_owned())
            .to_ascii_lowercase();
        let terminal = matches!(
            status.as_str(),
            "completed"
                | "succeeded"
                | "success"
                | "failed"
                | "error"
                | "killed"
                | "cancelled"
                | "canceled"
                | "closed"
                | "exited"
        );
        let succeeded = matches!(
            status.as_str(),
            "completed" | "succeeded" | "success" | "closed" | "exited"
        );
        let output = match self.get_session_output(session_id) {
            Ok(value) => Some(tool_output_text(&structured_result(&value))),
            Err(error) if is_session_absent_error(&error) => None,
            Err(error) => return Err(error),
        };
        Ok(SessionSnapshot {
            session_id: session_id.to_owned(),
            status,
            terminal,
            succeeded,
            output,
        })
    }

    pub fn find_unknown_dispatch(
        &mut self,
        client_request_id: &str,
        title_marker: &str,
        prompt_hash: &str,
        created_at_ms: i64,
    ) -> Result<Option<DiscoveredSessionBinding>, CoordinationError> {
        let sessions = self.list_sessions()?;
        let panes = self.list_panes()?;
        match_unknown_dispatch(
            &structured_result(&sessions),
            &structured_result(&panes),
            client_request_id,
            title_marker,
            prompt_hash,
            created_at_ms,
        )
    }

    fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        effect: EffectBoundary,
    ) -> Result<Value, CoordinationError> {
        let result = self.call_mcp(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments
            }),
            effect,
        )?;
        tool_result_payload(name, &result)
    }

    fn require_tool(&self, tool_name: &str) -> Result<(), CoordinationError> {
        let capability = self.capability.as_ref().ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::AdapterNotReady,
                "CC-Panes capabilities have not been negotiated",
            )
        })?;
        if capability.supports_tool(tool_name) {
            Ok(())
        } else {
            Err(CoordinationError::new(
                ErrorCode::AdapterIncompatible,
                format!("CC-Panes capability {tool_name} is unavailable"),
            ))
        }
    }

    fn call_mcp(
        &mut self,
        method: &str,
        params: Value,
        effect: EffectBoundary,
    ) -> Result<Value, CoordinationError> {
        self.request_sequence = self.request_sequence.saturating_add(1);
        let request_id = format!("ccpanes-adapter-{}", self.request_sequence);
        let request = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params
        });
        let response = self
            .transport
            .post_json(self.session_id.as_deref(), &request)
            .map_err(|error| map_transport_failure(error, effect))?;
        if let Some(session_id) = response.header("mcp-session-id") {
            self.session_id = Some(session_id.to_owned());
        }
        if response.status_code == 401 || response.status_code == 403 {
            return Err(safe_adapter_error(
                ErrorCode::AuthorizationFailed,
                format!(
                    "cc-panes MCP authorization failed with HTTP {}",
                    response.status_code
                ),
            ));
        }
        if !matches!(response.status_code, 200..=299) {
            return Err(safe_adapter_error(
                ErrorCode::AdapterNotReady,
                format!("cc-panes MCP returned HTTP {}", response.status_code),
            ));
        }
        let value = parse_mcp_response(&response.body, &request_id).map_err(|error| {
            if effect == EffectBoundary::ExternalWorkerMayHaveStarted {
                CoordinationError::new(ErrorCode::DispatchResultUnknown, error.message)
            } else {
                error
            }
        })?;
        if let Some(error) = value.get("error") {
            return Err(map_jsonrpc_error(error));
        }
        Ok(value.get("result").cloned().unwrap_or(value))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), CoordinationError> {
        self.request_sequence = self.request_sequence.saturating_add(1);
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        let response = self
            .transport
            .post_json(self.session_id.as_deref(), &request)
            .map_err(|error| map_transport_failure(error, EffectBoundary::NoExternalWorker))?;
        if let Some(session_id) = response.header("mcp-session-id") {
            self.session_id = Some(session_id.to_owned());
        }
        if !matches!(response.status_code, 200..=299) {
            return Err(safe_adapter_error(
                ErrorCode::AdapterNotReady,
                format!(
                    "cc-panes MCP notification returned HTTP {}",
                    response.status_code
                ),
            ));
        }
        Ok(())
    }
}

pub fn parse_mcp_response(body: &str, request_id: &str) -> Result<Value, CoordinationError> {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        return correlate_jsonrpc_response(value, request_id);
    }
    let mut last_value = None;
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(data).map_err(|error| {
            CoordinationError::new(
                ErrorCode::AdapterNotReady,
                format!("decode CC-Panes MCP SSE data failed: {error}"),
            )
        })?;
        if value.get("id").and_then(Value::as_str) == Some(request_id) {
            return Ok(value);
        }
        last_value = Some(value);
    }
    if let Some(value) = last_value {
        return correlate_jsonrpc_response(value, request_id);
    }
    Err(CoordinationError::new(
        ErrorCode::AdapterNotReady,
        "CC-Panes MCP response did not contain JSON data",
    ))
}

pub fn tool_result_payload(tool_name: &str, value: &Value) -> Result<Value, CoordinationError> {
    let text = tool_result_text(value);
    if matches!(
        tool_name,
        TOOL_KILL_SESSION | TOOL_GET_SESSION_STATUS | TOOL_GET_SESSION_OUTPUT
    ) && text.is_some_and(looks_like_session_not_found)
    {
        return Err(session_absent_error());
    }
    if value
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(safe_adapter_error(
            ErrorCode::AdapterNotReady,
            format!(
                "cc-panes {tool_name} failed: {}",
                compact_safe_message(text.unwrap_or("tool returned isError=true"), 512)
            ),
        ));
    }
    if let Some(text) = text {
        if looks_like_tool_error(text) {
            return Err(safe_adapter_error(
                ErrorCode::AdapterNotReady,
                format!(
                    "cc-panes {tool_name} failed: {}",
                    compact_safe_message(text, 512)
                ),
            ));
        }
    }
    Ok(structured_result(value))
}

pub fn kill_session_arguments(
    session_id: &str,
    reason: Option<&str>,
    accepts_reason: bool,
) -> Value {
    let mut arguments = Map::new();
    arguments.insert("sessionId".to_owned(), Value::String(session_id.to_owned()));
    if accepts_reason {
        arguments.insert(
            "reason".to_owned(),
            Value::String(reason.unwrap_or("workflow cancellation").to_owned()),
        );
    }
    Value::Object(arguments)
}

fn correlate_jsonrpc_response(value: Value, request_id: &str) -> Result<Value, CoordinationError> {
    if value.get("id").is_some() && value.get("id").and_then(Value::as_str) != Some(request_id) {
        return Err(CoordinationError::new(
            ErrorCode::AdapterNotReady,
            "CC-Panes MCP response id did not match request id",
        ));
    }
    Ok(value)
}

fn map_transport_failure(error: HttpTransportFailure, effect: EffectBoundary) -> CoordinationError {
    match (error.phase, error.kind, effect) {
        (TransportPhase::Connect, TransportFailureKind::Timeout, _) => CoordinationError::new(
            ErrorCode::AdapterNotReady,
            "cc-panes MCP connect timeout before external dispatch",
        )
        .with_retry_after_ms(1_000),
        (TransportPhase::Connect, _, _) => CoordinationError::new(
            ErrorCode::AdapterNotReady,
            compact_safe_message(&error.message, 256),
        )
        .with_retry_after_ms(1_000),
        (
            TransportPhase::Response,
            TransportFailureKind::Timeout,
            EffectBoundary::ExternalWorkerMayHaveStarted,
        ) => CoordinationError::new(
            ErrorCode::DispatchResultUnknown,
            "cc-panes MCP response timeout after dispatch request was sent",
        ),
        (TransportPhase::Response, _, EffectBoundary::ExternalWorkerMayHaveStarted) => {
            CoordinationError::new(
                ErrorCode::DispatchResultUnknown,
                compact_safe_message(&error.message, 256),
            )
        }
        (TransportPhase::Response, _, EffectBoundary::NoExternalWorker) => CoordinationError::new(
            ErrorCode::AdapterNotReady,
            compact_safe_message(&error.message, 256),
        )
        .with_retry_after_ms(1_000),
    }
}

fn map_jsonrpc_error(error: &Value) -> CoordinationError {
    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown CC-Panes MCP error");
    let public_code = match code {
        -32601 => ErrorCode::AdapterIncompatible,
        -32602 => ErrorCode::InvalidRequest,
        -32001 => ErrorCode::AuthorizationFailed,
        _ => ErrorCode::AdapterNotReady,
    };
    safe_adapter_error(
        public_code,
        format!(
            "cc-panes MCP error {code}: {}",
            compact_safe_message(message, 512)
        ),
    )
}

fn filter_launch_arguments(mut arguments: Value, capability: &CapabilitySnapshot) -> Value {
    if let Some(object) = arguments.as_object_mut() {
        if !capability.launch_task_accepts_client_request_id {
            object.remove("clientRequestId");
        }
        let accepted = capability
            .launch_task_properties
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        object.retain(|name, _| accepted.contains(name.as_str()));
    }
    arguments
}

fn structured_result(value: &Value) -> Value {
    value
        .get("structuredContent")
        .cloned()
        .or_else(|| {
            value
                .get("content")
                .and_then(Value::as_array)
                .and_then(|values| values.first())
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str)
                .and_then(|text| serde_json::from_str(text).ok())
        })
        .unwrap_or_else(|| value.clone())
}

fn tool_result_text(value: &Value) -> Option<&str> {
    value
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
}

fn tool_output_text(value: &Value) -> String {
    if let Some(content) = value.get("content").and_then(Value::as_str) {
        return content.to_owned();
    }
    if let Some(lines) = value.get("lines").and_then(Value::as_array) {
        let lines = lines.iter().filter_map(Value::as_str).collect::<Vec<_>>();
        if !lines.is_empty() {
            return lines.join("\n");
        }
    }
    if let Some(output) = value.get("output").and_then(Value::as_str) {
        return output.to_owned();
    }
    if let Some(text) = tool_result_text(value) {
        return text.to_owned();
    }
    serde_json::to_string(value).unwrap_or_default()
}

fn looks_like_tool_error(value: &str) -> bool {
    let trimmed = value.trim_start();
    trimmed.starts_with("错误:")
        || trimmed.starts_with("错误：")
        || trimmed
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("error:"))
}

fn looks_like_session_not_found(value: &str) -> bool {
    let lowercase = value.to_ascii_lowercase();
    let explicit_not_found = lowercase.contains("session not found")
        || lowercase.contains("[not_found]")
        || lowercase.contains("\"code\":\"not_found\"")
        || lowercase.contains("\"code\": \"not_found\"");
    let localized_not_found = value.contains("会话")
        && (value.contains("不存在") || value.contains("未找到"))
        && lowercase.contains("404");
    explicit_not_found || localized_not_found
}

fn session_absent_error() -> CoordinationError {
    CoordinationError::new(
        ErrorCode::AdapterNotReady,
        "cc-panes session is already absent",
    )
    .with_current_state("SESSION_NOT_FOUND")
    .retryable(false)
}

fn is_session_absent_error(error: &CoordinationError) -> bool {
    error.current_state.as_deref() == Some("SESSION_NOT_FOUND")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{tool_schema_value, TOOL_LAUNCH_TASK};

    #[test]
    fn parses_json_mcp_response_and_requires_correlation() {
        let value = parse_mcp_response(
            r#"{"jsonrpc":"2.0","id":"request","result":{"ok":true}}"#,
            "request",
        )
        .unwrap();
        assert_eq!(value["result"]["ok"], true);

        let error = parse_mcp_response(
            r#"{"jsonrpc":"2.0","id":"other","result":{"ok":true}}"#,
            "request",
        )
        .expect_err("wrong id");
        assert_eq!(error.code, ErrorCode::AdapterNotReady);
    }

    #[test]
    fn parses_sse_mcp_response_and_ignores_keepalive() {
        let value = parse_mcp_response(
            "data: \nid: 0\nretry: 3000\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"request\",\"result\":{\"ok\":true}}\n",
            "request",
        )
        .unwrap();
        assert_eq!(value["result"]["ok"], true);
    }

    #[test]
    fn launch_task_filters_arguments_by_negotiated_schema() {
        let capability = CapabilitySnapshot {
            readiness: AdapterReadiness::Ready,
            server_version: None,
            protocol_version: Some(CCPANES_MCP_PROTOCOL_VERSION.to_owned()),
            tools: vec![TOOL_LAUNCH_TASK.to_owned()],
            capability_digest: "digest".to_owned(),
            launch_task_properties: vec!["projectPath".to_owned(), "prompt".to_owned()],
            launch_task_accepts_client_request_id: false,
            dispatch_ready: true,
            get_session_status_available: true,
            get_session_output_available: true,
            submit_to_session_available: false,
            kill_session_available: false,
            wait_for_session_available: false,
            write_to_session_available: false,
            kill_session_accepts_reason: false,
        };
        let filtered = filter_launch_arguments(
            json!({
                "projectPath": "D:/PROJECT",
                "prompt": "TASK",
                "clientRequestId": "request-1",
                "future": true
            }),
            &capability,
        );
        assert_eq!(
            filtered,
            json!({"projectPath": "D:/PROJECT", "prompt": "TASK"})
        );
    }

    #[test]
    fn kill_session_reason_is_schema_gated() {
        assert_eq!(
            kill_session_arguments("session-1", Some("synthetic"), false),
            json!({"sessionId": "session-1"})
        );
        assert_eq!(
            kill_session_arguments("session-1", Some("synthetic"), true),
            json!({"sessionId": "session-1", "reason": "synthetic"})
        );
    }

    #[test]
    fn session_output_content_lines_and_historical_output_are_compatible() {
        assert_eq!(
            tool_output_text(&json!({"content": "content shape"})),
            "content shape"
        );
        assert_eq!(
            tool_output_text(&json!({"lines": ["line one", "line two"]})),
            "line one\nline two"
        );
        assert_eq!(
            tool_output_text(&json!({"output": "historical output shape"})),
            "historical output shape"
        );
    }

    #[test]
    fn tool_result_payload_translates_business_text_and_session_absence() {
        let error = tool_result_payload(
            TOOL_LAUNCH_TASK,
            &json!({
                "content": [{"type": "text", "text": "错误: Authorization: Bearer TOKEN_PLACEHOLDER 项目路径未注册"}],
                "isError": false
            }),
        )
        .expect_err("business text error");
        assert_eq!(error.code, ErrorCode::AdapterNotReady);
        assert!(!error.message.contains("TOKEN_PLACEHOLDER"));

        let error = tool_result_payload(
            TOOL_KILL_SESSION,
            &json!({
                "content": [{"type": "text", "text": "error: [NOT_FOUND] Session not found: session-1"}],
                "isError": true
            }),
        )
        .expect_err("session missing");
        assert!(is_session_absent_error(&error));
    }

    #[test]
    fn structured_jsonrpc_error_maps_to_public_code_and_redacts() {
        let error = map_jsonrpc_error(&json!({
            "code": -32602,
            "message": "invalid Authorization: Bearer TOKEN_PLACEHOLDER"
        }));
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(!error.message.contains("TOKEN_PLACEHOLDER"));
    }

    #[test]
    fn capability_fixture_keeps_launch_tool_name_available() {
        let tool = tool_schema_value(TOOL_LAUNCH_TASK, &["projectPath"], &["projectPath"]);
        assert_eq!(tool["name"], TOOL_LAUNCH_TASK);
    }
}
