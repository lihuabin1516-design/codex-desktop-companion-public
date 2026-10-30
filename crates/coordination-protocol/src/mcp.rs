use std::fmt;

use coordination_core::{
    AccessMode, CallerId, CoordinationError, CoordinationEvent, DirtyPolicy, ErrorCode,
    ExternalBindingId, IdempotencyKey, RepositoryRef, RequestId, ResultAggregationState, Revision,
    StepId, WorkerId, WorkerState, WorkflowId, WorkflowSpecV1Beta1, WorkflowState,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::version::{
    require_schema_version, CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION,
    CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION, COMMAND_ACK_SCHEMA_VERSION,
    RESULT_QUERY_SCHEMA_VERSION, STATUS_QUERY_SCHEMA_VERSION, WORKER_CANCEL_SCHEMA_VERSION,
    WORKER_DISPATCH_ACCEPTED_SCHEMA_VERSION, WORKER_DISPATCH_SCHEMA_VERSION,
    WORKFLOW_CANCEL_SCHEMA_VERSION, WORKFLOW_CREATE_SCHEMA_VERSION, WORKFLOW_START_SCHEMA_VERSION,
};

const CCPANES_PREFLIGHT_OUTPUT_SCHEMA_VERSION: &str = "ccpanes-adapter-readonly-preflight/v1beta1";
const CCPANES_SYNTHETIC_CANARY_OUTPUT_SCHEMA_VERSION: &str =
    "coordination.real-ccpanes-canary-runner/v1beta1";
const MAX_CCPANES_PATH_LENGTH: usize = 4_096;
const MAX_CCPANES_NAME_LENGTH: usize = 96;
const MAX_CCPANES_MARKER_LENGTH: usize = 1_024;
pub const CCPANES_MAX_CONNECT_TIMEOUT_MS: u64 = 30_000;
pub const CCPANES_MAX_REQUEST_TIMEOUT_MS: u64 = 120_000;
pub const CCPANES_MAX_CANARY_TIMEOUT_MS: u64 = 900_000;
pub const CCPANES_MAX_CANARY_POLL_INTERVAL_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpToolName {
    CoordinatorStatus,
    AdapterStatus,
    CcpanesPreflight,
    CcpanesSyntheticCanary,
    WorkerDispatch,
    WorkerStatus,
    WorkerCancel,
    WorkflowCreate,
    WorkflowStart,
    WorkflowStatus,
    WorkflowEvents,
    WorkflowCancel,
    WorkflowResult,
}

impl McpToolName {
    pub const ALL: &'static [Self] = &[
        Self::CoordinatorStatus,
        Self::AdapterStatus,
        Self::CcpanesPreflight,
        Self::CcpanesSyntheticCanary,
        Self::WorkerDispatch,
        Self::WorkerStatus,
        Self::WorkerCancel,
        Self::WorkflowCreate,
        Self::WorkflowStart,
        Self::WorkflowStatus,
        Self::WorkflowEvents,
        Self::WorkflowCancel,
        Self::WorkflowResult,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CoordinatorStatus => "coordinator_status",
            Self::AdapterStatus => "adapter_status",
            Self::CcpanesPreflight => "ccpanes_preflight",
            Self::CcpanesSyntheticCanary => "ccpanes_synthetic_canary",
            Self::WorkerDispatch => "worker_dispatch",
            Self::WorkerStatus => "worker_status",
            Self::WorkerCancel => "worker_cancel",
            Self::WorkflowCreate => "workflow_create",
            Self::WorkflowStart => "workflow_start",
            Self::WorkflowStatus => "workflow_status",
            Self::WorkflowEvents => "workflow_events",
            Self::WorkflowCancel => "workflow_cancel",
            Self::WorkflowResult => "workflow_result",
        }
    }

    pub fn parse(value: &str) -> Result<Self, CoordinationError> {
        match value {
            "coordinator_status" => Ok(Self::CoordinatorStatus),
            "adapter_status" => Ok(Self::AdapterStatus),
            "ccpanes_preflight" => Ok(Self::CcpanesPreflight),
            "ccpanes_synthetic_canary" => Ok(Self::CcpanesSyntheticCanary),
            "worker_dispatch" => Ok(Self::WorkerDispatch),
            "worker_status" => Ok(Self::WorkerStatus),
            "worker_cancel" => Ok(Self::WorkerCancel),
            "workflow_create" => Ok(Self::WorkflowCreate),
            "workflow_start" => Ok(Self::WorkflowStart),
            "workflow_status" => Ok(Self::WorkflowStatus),
            "workflow_events" => Ok(Self::WorkflowEvents),
            "workflow_cancel" => Ok(Self::WorkflowCancel),
            "workflow_result" => Ok(Self::WorkflowResult),
            _ => Err(CoordinationError::invalid_request(format!(
                "unknown MCP tool: {value}"
            ))),
        }
    }

    pub const fn kind(self) -> McpToolKind {
        match self {
            Self::CoordinatorStatus
            | Self::AdapterStatus
            | Self::CcpanesPreflight
            | Self::WorkerStatus
            | Self::WorkflowStatus
            | Self::WorkflowEvents
            | Self::WorkflowResult => McpToolKind::Query,
            Self::WorkerDispatch
            | Self::CcpanesSyntheticCanary
            | Self::WorkerCancel
            | Self::WorkflowCreate
            | Self::WorkflowStart
            | Self::WorkflowCancel => McpToolKind::Command,
        }
    }
}

impl fmt::Display for McpToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpToolKind {
    Command,
    Query,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolContract {
    pub name: String,
    pub kind: McpToolKind,
    pub input_schema_version: String,
    pub output_schema_version: String,
}

pub fn public_mcp_tool_contracts() -> Vec<McpToolContract> {
    McpToolName::ALL
        .iter()
        .copied()
        .map(|name| McpToolContract {
            name: name.as_str().to_owned(),
            kind: name.kind(),
            input_schema_version: input_schema_version(name).to_owned(),
            output_schema_version: output_schema_version(name).to_owned(),
        })
        .collect()
}

pub const fn mcp_tool_description(name: McpToolName) -> &'static str {
    match name {
        McpToolName::CoordinatorStatus => "Read Coordinator health, schema, capacity, and metrics.",
        McpToolName::AdapterStatus => {
            "Read CC-Panes adapter readiness snapshot from the Coordinator."
        }
        McpToolName::CcpanesPreflight => {
            "Run a read-only CC-Panes capability and Session inventory preflight."
        }
        McpToolName::CcpanesSyntheticCanary => {
            "Run one controlled synthetic foreground CC-Panes canary."
        }
        McpToolName::WorkerDispatch => {
            "Create a single-step Workflow and enqueue one external Worker."
        }
        McpToolName::WorkerStatus => "Read one Worker state and external binding summary.",
        McpToolName::WorkerCancel => "Request cancellation of one Worker.",
        McpToolName::WorkflowCreate => "Create a Workflow from WorkflowSpec v1beta1.",
        McpToolName::WorkflowStart => "Start a READY Workflow.",
        McpToolName::WorkflowStatus => "Read Workflow state with Worker summaries.",
        McpToolName::WorkflowEvents => "Read Workflow events after a sequence cursor.",
        McpToolName::WorkflowCancel => "Request cancellation of a Workflow and active Workers.",
        McpToolName::WorkflowResult => "Read aggregated Workflow result summaries.",
    }
}

pub const fn input_schema_version(name: McpToolName) -> &'static str {
    match name {
        McpToolName::WorkerDispatch => WORKER_DISPATCH_SCHEMA_VERSION,
        McpToolName::WorkflowCreate => WORKFLOW_CREATE_SCHEMA_VERSION,
        McpToolName::CcpanesPreflight => CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION,
        McpToolName::CcpanesSyntheticCanary => CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION,
        McpToolName::CoordinatorStatus
        | McpToolName::AdapterStatus
        | McpToolName::WorkerStatus
        | McpToolName::WorkflowStatus => STATUS_QUERY_SCHEMA_VERSION,
        McpToolName::WorkerCancel => WORKER_CANCEL_SCHEMA_VERSION,
        McpToolName::WorkflowStart => WORKFLOW_START_SCHEMA_VERSION,
        McpToolName::WorkflowCancel => WORKFLOW_CANCEL_SCHEMA_VERSION,
        McpToolName::WorkflowEvents => crate::EVENT_QUERY_SCHEMA_VERSION,
        McpToolName::WorkflowResult => RESULT_QUERY_SCHEMA_VERSION,
    }
}

pub const fn output_schema_version(name: McpToolName) -> &'static str {
    match name {
        McpToolName::WorkerDispatch => WORKER_DISPATCH_ACCEPTED_SCHEMA_VERSION,
        McpToolName::CcpanesPreflight => CCPANES_PREFLIGHT_OUTPUT_SCHEMA_VERSION,
        McpToolName::CcpanesSyntheticCanary => CCPANES_SYNTHETIC_CANARY_OUTPUT_SCHEMA_VERSION,
        McpToolName::CoordinatorStatus
        | McpToolName::AdapterStatus
        | McpToolName::WorkerStatus
        | McpToolName::WorkflowStatus => crate::STATUS_RESPONSE_SCHEMA_VERSION,
        McpToolName::WorkerCancel
        | McpToolName::WorkflowCreate
        | McpToolName::WorkflowStart
        | McpToolName::WorkflowCancel => COMMAND_ACK_SCHEMA_VERSION,
        McpToolName::WorkflowEvents => crate::EVENT_PAGE_SCHEMA_VERSION,
        McpToolName::WorkflowResult => crate::RESULT_RESPONSE_SCHEMA_VERSION,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CcpanesPreflightInput {
    pub schema_version: String,
    #[serde(default)]
    pub discovery_path: Option<String>,
    #[serde(default = "default_discovery_generation")]
    pub discovery_generation: u64,
    #[serde(default = "default_preflight_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_preflight_request_timeout_ms")]
    pub request_timeout_ms: u64,
}

impl CcpanesPreflightInput {
    pub fn from_value(value: Value) -> Result<Self, CoordinationError> {
        let input: Self = deserialize_input(value)?;
        input.validate_contract()?;
        Ok(input)
    }

    pub fn validate_contract(&self) -> Result<(), CoordinationError> {
        require_schema_version(
            &self.schema_version,
            CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION,
        )?;
        validate_optional_bounded_string(
            "discoveryPath",
            self.discovery_path.as_deref(),
            MAX_CCPANES_PATH_LENGTH,
        )?;
        validate_positive("discoveryGeneration", self.discovery_generation)?;
        validate_bounded_positive(
            "connectTimeoutMs",
            self.connect_timeout_ms,
            CCPANES_MAX_CONNECT_TIMEOUT_MS,
        )?;
        validate_bounded_positive(
            "requestTimeoutMs",
            self.request_timeout_ms,
            CCPANES_MAX_REQUEST_TIMEOUT_MS,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CcpanesSyntheticCanaryInput {
    pub schema_version: String,
    pub request_id: RequestId,
    #[serde(default)]
    pub discovery_path: Option<String>,
    pub project_path: String,
    pub workspace_name: String,
    pub project_name: String,
    pub marker: String,
    #[serde(default = "default_canary_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_canary_poll_interval_ms")]
    pub poll_interval_ms: u64,
    #[serde(default = "default_canary_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_canary_request_timeout_ms")]
    pub request_timeout_ms: u64,
}

impl CcpanesSyntheticCanaryInput {
    pub fn from_value(value: Value) -> Result<Self, CoordinationError> {
        let input: Self = deserialize_input(value)?;
        input.validate_contract()?;
        Ok(input)
    }

    pub fn validate_contract(&self) -> Result<(), CoordinationError> {
        require_schema_version(
            &self.schema_version,
            CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION,
        )?;
        RequestId::try_new(self.request_id.as_str())?;
        validate_optional_bounded_string(
            "discoveryPath",
            self.discovery_path.as_deref(),
            MAX_CCPANES_PATH_LENGTH,
        )?;
        validate_required_bounded_string(
            "projectPath",
            &self.project_path,
            MAX_CCPANES_PATH_LENGTH,
        )?;
        validate_required_bounded_string(
            "workspaceName",
            &self.workspace_name,
            MAX_CCPANES_NAME_LENGTH,
        )?;
        validate_required_bounded_string(
            "projectName",
            &self.project_name,
            MAX_CCPANES_NAME_LENGTH,
        )?;
        validate_required_bounded_string("marker", &self.marker, MAX_CCPANES_MARKER_LENGTH)?;
        validate_bounded_positive("timeoutMs", self.timeout_ms, CCPANES_MAX_CANARY_TIMEOUT_MS)?;
        validate_bounded_positive(
            "pollIntervalMs",
            self.poll_interval_ms,
            CCPANES_MAX_CANARY_POLL_INTERVAL_MS,
        )?;
        validate_bounded_positive(
            "connectTimeoutMs",
            self.connect_timeout_ms,
            CCPANES_MAX_CONNECT_TIMEOUT_MS,
        )?;
        validate_bounded_positive(
            "requestTimeoutMs",
            self.request_timeout_ms,
            CCPANES_MAX_REQUEST_TIMEOUT_MS,
        )
    }
}

fn deserialize_input<T>(value: Value) -> Result<T, CoordinationError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(value).map_err(|error| {
        CoordinationError::new(
            ErrorCode::InvalidRequest,
            format!("invalid request: {error}"),
        )
    })
}

fn validate_required_bounded_string(
    field: &str,
    value: &str,
    max_length: usize,
) -> Result<(), CoordinationError> {
    if value.trim().is_empty() {
        return Err(CoordinationError::invalid_request(format!(
            "{field} must not be empty"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(CoordinationError::invalid_request(format!(
            "{field} must contain no control characters"
        )));
    }
    if value.chars().count() > max_length {
        return Err(CoordinationError::invalid_request(format!(
            "{field} must not exceed {max_length} characters"
        )));
    }
    Ok(())
}

fn validate_optional_bounded_string(
    field: &str,
    value: Option<&str>,
    max_length: usize,
) -> Result<(), CoordinationError> {
    match value {
        Some(value) => validate_required_bounded_string(field, value, max_length),
        None => Ok(()),
    }
}

fn validate_positive(field: &str, value: u64) -> Result<(), CoordinationError> {
    if value == 0 {
        Err(CoordinationError::invalid_request(format!(
            "{field} must be greater than zero"
        )))
    } else {
        Ok(())
    }
}

fn validate_bounded_positive(
    field: &str,
    value: u64,
    maximum: u64,
) -> Result<(), CoordinationError> {
    validate_positive(field, value)?;
    if value > maximum {
        Err(CoordinationError::invalid_request(format!(
            "{field} must not exceed {maximum}"
        )))
    } else {
        Ok(())
    }
}

fn default_discovery_generation() -> u64 {
    1
}

fn default_preflight_connect_timeout_ms() -> u64 {
    2_000
}

fn default_preflight_request_timeout_ms() -> u64 {
    8_000
}

fn default_canary_timeout_ms() -> u64 {
    420_000
}

fn default_canary_poll_interval_ms() -> u64 {
    750
}

fn default_canary_connect_timeout_ms() -> u64 {
    2_000
}

fn default_canary_request_timeout_ms() -> u64 {
    12_000
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RepositoryInput {
    pub path: String,
    #[serde(default)]
    pub expected_head: Option<String>,
    #[serde(default = "default_dirty_policy")]
    pub dirty_policy: String,
    #[serde(default)]
    pub workspace_name: Option<String>,
}

fn default_dirty_policy() -> String {
    "reject".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerDispatchExecutorInput {
    #[serde(default = "default_codex_cli")]
    pub cli_tool: String,
}

fn default_codex_cli() -> String {
    "codex".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerResultInput {
    #[serde(default = "default_result_format")]
    pub format: String,
}

fn default_result_format() -> String {
    "structured-summary".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerDispatchInput {
    pub schema_version: String,
    pub request_id: RequestId,
    pub repository: RepositoryInput,
    pub objective: String,
    #[serde(default)]
    pub access: AccessMode,
    #[serde(default = "default_worker_dispatch_executor")]
    pub executor: WorkerDispatchExecutorInput,
    #[serde(default = "default_worker_dispatch_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_worker_result")]
    pub result: WorkerResultInput,
}

fn default_worker_dispatch_executor() -> WorkerDispatchExecutorInput {
    WorkerDispatchExecutorInput {
        cli_tool: default_codex_cli(),
    }
}

fn default_worker_result() -> WorkerResultInput {
    WorkerResultInput {
        format: default_result_format(),
    }
}

fn default_worker_dispatch_timeout() -> u64 {
    1_200
}

impl WorkerDispatchInput {
    pub fn from_value(value: Value) -> Result<Self, CoordinationError> {
        let input: Self = serde_json::from_value(value).map_err(|error| {
            CoordinationError::new(
                ErrorCode::InvalidRequest,
                format!("invalid request: {error}"),
            )
        })?;
        input.validate_contract()?;
        Ok(input)
    }

    pub fn validate_contract(&self) -> Result<(), CoordinationError> {
        require_schema_version(&self.schema_version, WORKER_DISPATCH_SCHEMA_VERSION)?;
        if self.repository.path.trim().is_empty() {
            return Err(CoordinationError::invalid_request(
                "repository.path must not be empty",
            ));
        }
        if self
            .repository
            .workspace_name
            .as_deref()
            .is_some_and(|value| value.trim().is_empty() || value.chars().any(char::is_control))
        {
            return Err(CoordinationError::invalid_request(
                "repository.workspaceName must be non-empty and contain no control characters",
            ));
        }
        if !matches!(
            self.repository.dirty_policy.as_str(),
            "reject" | "allow-declared"
        ) {
            return Err(CoordinationError::invalid_request(
                "repository.dirtyPolicy must be reject or allow-declared",
            ));
        }
        if self.objective.trim().is_empty() {
            return Err(CoordinationError::invalid_request(
                "objective must not be empty",
            ));
        }
        if self.executor.cli_tool.trim().is_empty() {
            return Err(CoordinationError::invalid_request(
                "executor.cliTool must not be empty",
            ));
        }
        if self.timeout_seconds == 0 {
            return Err(CoordinationError::invalid_request(
                "timeoutSeconds must be greater than zero",
            ));
        }
        if self.result.format != "structured-summary" {
            return Err(CoordinationError::invalid_request(
                "result.format must be structured-summary",
            ));
        }
        Ok(())
    }

    pub fn idempotency_key(
        &self,
        caller_id: CallerId,
    ) -> Result<IdempotencyKey, CoordinationError> {
        IdempotencyKey::try_new(
            McpToolName::WorkerDispatch.as_str(),
            caller_id,
            self.request_id.clone(),
        )
    }

    pub fn repository_ref(&self) -> Result<RepositoryRef, CoordinationError> {
        self.validate_contract()?;
        let dirty_policy = match self.repository.dirty_policy.as_str() {
            "reject" => DirtyPolicy::Reject,
            "allow-declared" => DirtyPolicy::AllowDeclared,
            _ => {
                return Err(CoordinationError::invalid_request(
                    "repository.dirtyPolicy must be reject or allow-declared",
                ))
            }
        };
        Ok(RepositoryRef {
            path: self.repository.path.clone(),
            expected_head: self.repository.expected_head.clone(),
            dirty_policy,
            workspace_name: self.repository.workspace_name.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerDispatchAccepted {
    pub schema_version: String,
    pub accepted: bool,
    pub workflow_id: WorkflowId,
    pub worker_id: WorkerId,
    pub revision: Revision,
    pub state: WorkerState,
}

impl WorkerDispatchAccepted {
    pub fn queued(workflow_id: WorkflowId, worker_id: WorkerId, revision: Revision) -> Self {
        Self {
            schema_version: WORKER_DISPATCH_ACCEPTED_SCHEMA_VERSION.to_owned(),
            accepted: true,
            workflow_id,
            worker_id,
            revision,
            state: WorkerState::Queued,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowCreateInput {
    pub schema_version: String,
    pub request_id: RequestId,
    pub spec: WorkflowSpecV1Beta1,
}

impl WorkflowCreateInput {
    pub fn from_value(value: Value) -> Result<Self, CoordinationError> {
        let input: Self = serde_json::from_value(value).map_err(|error| {
            CoordinationError::new(
                ErrorCode::InvalidRequest,
                format!("invalid request: {error}"),
            )
        })?;
        input.validate_contract()?;
        Ok(input)
    }

    pub fn validate_contract(&self) -> Result<(), CoordinationError> {
        require_schema_version(&self.schema_version, WORKFLOW_CREATE_SCHEMA_VERSION)?;
        self.spec.require_supported_schema()?;
        self.spec
            .validate()
            .map_err(|error| CoordinationError::invalid_request(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RevisionCommandInput {
    pub schema_version: String,
    pub request_id: RequestId,
    pub workflow_id: WorkflowId,
    pub expected_revision: Revision,
    #[serde(default)]
    pub reason: Option<String>,
}

impl RevisionCommandInput {
    pub fn from_value(value: Value, expected_schema: &str) -> Result<Self, CoordinationError> {
        let input: Self = serde_json::from_value(value).map_err(|error| {
            CoordinationError::new(
                ErrorCode::InvalidRequest,
                format!("invalid request: {error}"),
            )
        })?;
        require_schema_version(&input.schema_version, expected_schema)?;
        Ok(input)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerCancelInput {
    pub schema_version: String,
    pub request_id: RequestId,
    pub worker_id: WorkerId,
    #[serde(default)]
    pub expected_revision: Option<Revision>,
    #[serde(default)]
    pub reason: Option<String>,
}

impl WorkerCancelInput {
    pub fn from_value(value: Value) -> Result<Self, CoordinationError> {
        let input: Self = serde_json::from_value(value).map_err(|error| {
            CoordinationError::new(
                ErrorCode::InvalidRequest,
                format!("invalid request: {error}"),
            )
        })?;
        require_schema_version(&input.schema_version, WORKER_CANCEL_SCHEMA_VERSION)?;
        Ok(input)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EntityStatusInput {
    pub schema_version: String,
    #[serde(default)]
    pub workflow_id: Option<WorkflowId>,
    #[serde(default)]
    pub worker_id: Option<WorkerId>,
}

impl EntityStatusInput {
    pub fn from_value(value: Value) -> Result<Self, CoordinationError> {
        let input: Self = serde_json::from_value(value).map_err(|error| {
            CoordinationError::new(
                ErrorCode::InvalidRequest,
                format!("invalid request: {error}"),
            )
        })?;
        require_schema_version(&input.schema_version, STATUS_QUERY_SCHEMA_VERSION)?;
        Ok(input)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowEventsInput {
    pub schema_version: String,
    pub workflow_id: WorkflowId,
    pub after_sequence: u64,
    pub limit: u32,
}

impl WorkflowEventsInput {
    pub fn from_value(value: Value) -> Result<Self, CoordinationError> {
        let input: Self = serde_json::from_value(value).map_err(|error| {
            CoordinationError::new(
                ErrorCode::InvalidRequest,
                format!("invalid request: {error}"),
            )
        })?;
        require_schema_version(&input.schema_version, crate::EVENT_QUERY_SCHEMA_VERSION)?;
        if input.limit == 0 || input.limit > 1_000 {
            return Err(CoordinationError::invalid_request(
                "workflow_events.limit must be within 1..=1000",
            ));
        }
        Ok(input)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowResultInput {
    pub schema_version: String,
    pub workflow_id: WorkflowId,
}

impl WorkflowResultInput {
    pub fn from_value(value: Value) -> Result<Self, CoordinationError> {
        let input: Self = serde_json::from_value(value).map_err(|error| {
            CoordinationError::new(
                ErrorCode::InvalidRequest,
                format!("invalid request: {error}"),
            )
        })?;
        require_schema_version(&input.schema_version, RESULT_QUERY_SCHEMA_VERSION)?;
        Ok(input)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CommandAck {
    pub schema_version: String,
    pub accepted: bool,
    pub revision: Revision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<WorkflowId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<WorkerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub replayed: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl CommandAck {
    pub fn accepted(revision: Revision) -> Self {
        Self {
            schema_version: COMMAND_ACK_SCHEMA_VERSION.to_owned(),
            accepted: true,
            revision,
            workflow_id: None,
            worker_id: None,
            state: None,
            replayed: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerStatusResponse {
    pub schema_version: String,
    pub worker_id: WorkerId,
    pub workflow_id: WorkflowId,
    pub step_id: StepId,
    pub state: WorkerState,
    pub revision: Revision,
    pub objective: String,
    pub access: AccessMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_binding: Option<ExternalBindingStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExternalBindingStatus {
    pub binding_id: ExternalBindingId,
    pub adapter: String,
    pub generation: u64,
    pub state: String,
    pub has_external_session: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowStatusResponse {
    pub schema_version: String,
    pub workflow_id: WorkflowId,
    pub state: WorkflowState,
    pub revision: Revision,
    pub workers: Vec<WorkflowWorkerStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowWorkerStatus {
    pub worker_id: WorkerId,
    pub step_id: StepId,
    pub state: WorkerState,
    pub revision: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowEventsResponse {
    pub schema_version: String,
    pub workflow_id: WorkflowId,
    pub events: Vec<CoordinationEvent>,
    pub next_after_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AdapterStatusResponse {
    pub schema_version: String,
    pub readiness: String,
    pub dispatch_ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkflowResultResponse {
    pub schema_version: String,
    pub workflow_id: WorkflowId,
    pub state: ResultAggregationState,
    pub revision: Revision,
    pub workers: Vec<WorkerResultSummary>,
    #[serde(default)]
    pub missing_results: Vec<WorkerId>,
    #[serde(default)]
    pub failed_workers: Vec<WorkerId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerResultSummary {
    pub worker_id: WorkerId,
    pub step_id: StepId,
    pub state: WorkerState,
    pub summary: String,
    pub completed_at_ms: i64,
    pub truncated: bool,
    #[serde(default)]
    pub artifact_sha256: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn valid_worker_dispatch_value() -> Value {
        json!({
            "schemaVersion": WORKER_DISPATCH_SCHEMA_VERSION,
            "requestId": "request-1",
            "repository": {
                "path": "D:/PROJECT",
                "expectedHead": "SHA",
                "dirtyPolicy": "reject"
            },
            "objective": "inspect",
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

    fn valid_ccpanes_preflight_value() -> Value {
        json!({
            "schemaVersion": CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION,
            "discoveryGeneration": 1,
            "connectTimeoutMs": 2000,
            "requestTimeoutMs": 8000
        })
    }

    fn valid_ccpanes_synthetic_canary_value() -> Value {
        json!({
            "schemaVersion": CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION,
            "requestId": "global-canary-1",
            "projectPath": "C:/fixture/project",
            "workspaceName": "codex-ccpanes-v0123",
            "projectName": "compat-fixture",
            "marker": "CODEX_CCPANES_GLOBAL_CANARY",
            "timeoutMs": 420000,
            "pollIntervalMs": 750,
            "connectTimeoutMs": 2000,
            "requestTimeoutMs": 12000
        })
    }

    #[test]
    fn public_tool_names_and_kinds_match_phase1_contract() {
        let contracts = public_mcp_tool_contracts();
        let names = contracts
            .iter()
            .map(|contract| contract.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "coordinator_status",
                "adapter_status",
                "ccpanes_preflight",
                "ccpanes_synthetic_canary",
                "worker_dispatch",
                "worker_status",
                "worker_cancel",
                "workflow_create",
                "workflow_start",
                "workflow_status",
                "workflow_events",
                "workflow_cancel",
                "workflow_result",
            ]
        );
        assert_eq!(
            contracts
                .iter()
                .find(|contract| contract.name == "worker_dispatch")
                .expect("worker_dispatch contract")
                .kind,
            McpToolKind::Command
        );
    }

    #[test]
    fn ccpanes_tool_names_parse_with_expected_kinds_and_versions() {
        let preflight = McpToolName::parse("ccpanes_preflight").unwrap();
        assert_eq!(preflight, McpToolName::CcpanesPreflight);
        assert_eq!(preflight.kind(), McpToolKind::Query);
        assert_eq!(
            input_schema_version(preflight),
            CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION
        );
        assert_eq!(
            output_schema_version(preflight),
            "ccpanes-adapter-readonly-preflight/v1beta1"
        );
        assert_eq!(preflight.as_str(), "ccpanes_preflight");
        assert!(mcp_tool_description(preflight).contains("read-only"));

        let canary = McpToolName::parse("ccpanes_synthetic_canary").unwrap();
        assert_eq!(canary, McpToolName::CcpanesSyntheticCanary);
        assert_eq!(canary.kind(), McpToolKind::Command);
        assert_eq!(
            input_schema_version(canary),
            CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION
        );
        assert_eq!(
            output_schema_version(canary),
            "coordination.real-ccpanes-canary-runner/v1beta1"
        );
        assert_eq!(canary.as_str(), "ccpanes_synthetic_canary");
        assert!(mcp_tool_description(canary).contains("synthetic"));
    }

    #[test]
    fn ccpanes_inputs_accept_valid_values_and_apply_defaults() {
        let preflight = CcpanesPreflightInput::from_value(valid_ccpanes_preflight_value()).unwrap();
        assert_eq!(preflight.discovery_path, None);
        assert_eq!(preflight.discovery_generation, 1);
        assert_eq!(preflight.connect_timeout_ms, 2_000);
        assert_eq!(preflight.request_timeout_ms, 8_000);

        let canary =
            CcpanesSyntheticCanaryInput::from_value(valid_ccpanes_synthetic_canary_value())
                .unwrap();
        assert_eq!(canary.discovery_path, None);
        assert_eq!(canary.request_id.as_str(), "global-canary-1");
        assert_eq!(canary.timeout_ms, 420_000);
        assert_eq!(canary.poll_interval_ms, 750);
        assert_eq!(canary.connect_timeout_ms, 2_000);
        assert_eq!(canary.request_timeout_ms, 12_000);

        let default_preflight = CcpanesPreflightInput::from_value(json!({
            "schemaVersion": CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION
        }))
        .unwrap();
        assert_eq!(default_preflight.discovery_generation, 1);
        assert_eq!(default_preflight.connect_timeout_ms, 2_000);
        assert_eq!(default_preflight.request_timeout_ms, 8_000);

        let default_canary = CcpanesSyntheticCanaryInput::from_value(json!({
            "schemaVersion": CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION,
            "requestId": "global-canary-defaults",
            "projectPath": "C:/fixture/project",
            "workspaceName": "codex-ccpanes-v0123",
            "projectName": "compat-fixture",
            "marker": "CODEX_CCPANES_GLOBAL_CANARY"
        }))
        .unwrap();
        assert_eq!(default_canary.timeout_ms, 420_000);
        assert_eq!(default_canary.poll_interval_ms, 750);
        assert_eq!(default_canary.connect_timeout_ms, 2_000);
        assert_eq!(default_canary.request_timeout_ms, 12_000);
    }

    #[test]
    fn ccpanes_inputs_reject_wrong_schema_and_unknown_fields() {
        let mut preflight = valid_ccpanes_preflight_value();
        preflight["schemaVersion"] = json!("coordination.ccpanes-preflight-request/v9");
        assert_eq!(
            CcpanesPreflightInput::from_value(preflight)
                .expect_err("wrong preflight schema")
                .code,
            ErrorCode::SchemaVersionUnsupported
        );

        let mut canary = valid_ccpanes_synthetic_canary_value();
        canary["schemaVersion"] = json!("coordination.ccpanes-synthetic-canary-request/v9");
        assert_eq!(
            CcpanesSyntheticCanaryInput::from_value(canary)
                .expect_err("wrong canary schema")
                .code,
            ErrorCode::SchemaVersionUnsupported
        );

        let mut preflight = valid_ccpanes_preflight_value();
        preflight["unexpected"] = json!(true);
        assert_eq!(
            CcpanesPreflightInput::from_value(preflight)
                .expect_err("unknown preflight field")
                .code,
            ErrorCode::InvalidRequest
        );

        let mut canary = valid_ccpanes_synthetic_canary_value();
        canary["unexpected"] = json!(true);
        assert_eq!(
            CcpanesSyntheticCanaryInput::from_value(canary)
                .expect_err("unknown canary field")
                .code,
            ErrorCode::InvalidRequest
        );
    }

    #[test]
    fn ccpanes_inputs_reject_zero_generation_and_timeouts() {
        for field in [
            "discoveryGeneration",
            "connectTimeoutMs",
            "requestTimeoutMs",
        ] {
            let mut value = valid_ccpanes_preflight_value();
            value[field] = json!(0);
            assert_eq!(
                CcpanesPreflightInput::from_value(value)
                    .expect_err(field)
                    .code,
                ErrorCode::InvalidRequest
            );
        }

        for field in [
            "timeoutMs",
            "pollIntervalMs",
            "connectTimeoutMs",
            "requestTimeoutMs",
        ] {
            let mut value = valid_ccpanes_synthetic_canary_value();
            value[field] = json!(0);
            assert_eq!(
                CcpanesSyntheticCanaryInput::from_value(value)
                    .expect_err(field)
                    .code,
                ErrorCode::InvalidRequest
            );
        }
    }

    #[test]
    fn ccpanes_inputs_reject_timeout_values_above_public_budgets() {
        for (field, maximum) in [
            ("connectTimeoutMs", 30_000_u64),
            ("requestTimeoutMs", 120_000_u64),
        ] {
            let mut accepted = valid_ccpanes_preflight_value();
            accepted[field] = json!(maximum);
            CcpanesPreflightInput::from_value(accepted).expect(field);

            let mut rejected = valid_ccpanes_preflight_value();
            rejected[field] = json!(maximum + 1);
            assert_eq!(
                CcpanesPreflightInput::from_value(rejected)
                    .expect_err(field)
                    .code,
                ErrorCode::InvalidRequest
            );
        }

        for (field, maximum) in [
            ("timeoutMs", 900_000_u64),
            ("pollIntervalMs", 10_000_u64),
            ("connectTimeoutMs", 30_000_u64),
            ("requestTimeoutMs", 120_000_u64),
        ] {
            let mut accepted = valid_ccpanes_synthetic_canary_value();
            accepted[field] = json!(maximum);
            CcpanesSyntheticCanaryInput::from_value(accepted).expect(field);

            let mut rejected = valid_ccpanes_synthetic_canary_value();
            rejected[field] = json!(maximum + 1);
            assert_eq!(
                CcpanesSyntheticCanaryInput::from_value(rejected)
                    .expect_err(field)
                    .code,
                ErrorCode::InvalidRequest
            );
        }
    }

    #[test]
    fn ccpanes_inputs_reject_empty_control_and_unbounded_strings() {
        for invalid in ["   ", "bad\nvalue"] {
            let mut preflight = valid_ccpanes_preflight_value();
            preflight["discoveryPath"] = json!(invalid);
            assert_eq!(
                CcpanesPreflightInput::from_value(preflight)
                    .expect_err("invalid discovery path")
                    .code,
                ErrorCode::InvalidRequest
            );

            for field in [
                "requestId",
                "projectPath",
                "workspaceName",
                "projectName",
                "marker",
            ] {
                let mut canary = valid_ccpanes_synthetic_canary_value();
                canary[field] = json!(invalid);
                assert_eq!(
                    CcpanesSyntheticCanaryInput::from_value(canary)
                        .expect_err(field)
                        .code,
                    ErrorCode::InvalidRequest
                );
            }

            let mut canary = valid_ccpanes_synthetic_canary_value();
            canary["discoveryPath"] = json!(invalid);
            assert_eq!(
                CcpanesSyntheticCanaryInput::from_value(canary)
                    .expect_err("invalid canary discovery path")
                    .code,
                ErrorCode::InvalidRequest
            );
        }

        let mut long_discovery_path = valid_ccpanes_preflight_value();
        long_discovery_path["discoveryPath"] = json!("D".repeat(4_097));
        assert_eq!(
            CcpanesPreflightInput::from_value(long_discovery_path)
                .expect_err("long discovery path")
                .code,
            ErrorCode::InvalidRequest
        );

        let mut long_path = valid_ccpanes_synthetic_canary_value();
        long_path["projectPath"] = json!("P".repeat(4_097));
        assert_eq!(
            CcpanesSyntheticCanaryInput::from_value(long_path)
                .expect_err("long project path")
                .code,
            ErrorCode::InvalidRequest
        );

        for field in ["workspaceName", "projectName"] {
            let mut value = valid_ccpanes_synthetic_canary_value();
            value[field] = json!("N".repeat(97));
            assert_eq!(
                CcpanesSyntheticCanaryInput::from_value(value)
                    .expect_err(field)
                    .code,
                ErrorCode::InvalidRequest
            );
        }

        let mut long_marker = valid_ccpanes_synthetic_canary_value();
        long_marker["marker"] = json!("M".repeat(1_025));
        assert_eq!(
            CcpanesSyntheticCanaryInput::from_value(long_marker)
                .expect_err("long marker")
                .code,
            ErrorCode::InvalidRequest
        );
    }

    #[test]
    fn worker_dispatch_accepts_valid_schema_and_builds_idempotency_key() {
        let input = WorkerDispatchInput::from_value(valid_worker_dispatch_value()).unwrap();
        let key = input
            .idempotency_key(CallerId::try_new("codex-app").unwrap())
            .unwrap();
        assert_eq!(key.tool, "worker_dispatch");
        assert_eq!(key.request_id.as_str(), "request-1");
    }

    #[test]
    fn worker_dispatch_rejects_unknown_schema_version() {
        let mut value = valid_worker_dispatch_value();
        value["schemaVersion"] = json!("coordination.worker-dispatch/v9");
        let error = WorkerDispatchInput::from_value(value).expect_err("unknown schema");
        assert_eq!(error.code, ErrorCode::SchemaVersionUnsupported);
    }

    #[test]
    fn worker_dispatch_rejects_invalid_contract() {
        let mut value = valid_worker_dispatch_value();
        value["objective"] = json!("");
        let error = WorkerDispatchInput::from_value(value).expect_err("invalid objective");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn worker_dispatch_accepts_and_validates_repository_workspace_name() {
        let mut value = valid_worker_dispatch_value();
        value["repository"]["workspaceName"] = json!("codex-ccpanes-phase7-success");
        let input = WorkerDispatchInput::from_value(value).unwrap();
        let repository = input.repository_ref().unwrap();
        assert_eq!(
            repository.workspace_name.as_deref(),
            Some("codex-ccpanes-phase7-success")
        );

        let mut invalid = valid_worker_dispatch_value();
        invalid["repository"]["workspaceName"] = json!("\t");
        let error = WorkerDispatchInput::from_value(invalid).expect_err("invalid workspace name");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn worker_dispatch_duplicate_request_has_stable_scope() {
        let input = WorkerDispatchInput::from_value(valid_worker_dispatch_value()).unwrap();
        let first = input
            .idempotency_key(CallerId::try_new("codex-app").unwrap())
            .unwrap();
        let second = input
            .idempotency_key(CallerId::try_new("codex-app").unwrap())
            .unwrap();
        assert!(first.duplicate_of(&second));
    }

    #[test]
    fn accepted_worker_response_matches_public_output_contract() {
        let response = WorkerDispatchAccepted::queued(
            WorkflowId::try_new("workflow-1").unwrap(),
            WorkerId::try_new("worker-1").unwrap(),
            Revision::new(1),
        );
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(
            value["schemaVersion"],
            WORKER_DISPATCH_ACCEPTED_SCHEMA_VERSION
        );
        assert_eq!(value["accepted"], true);
        assert_eq!(value["state"], "QUEUED");
    }
}
