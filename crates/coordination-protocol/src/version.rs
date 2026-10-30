use coordination_core::{CoordinationError, ErrorCode};

pub const MCP_SCHEMA_VERSION: &str = "coordination.mcp/v1beta1";
pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
pub const IPC_SCHEMA_VERSION: &str = "coordination.ipc/v1beta1";
pub const COORDINATOR_HEALTH_SCHEMA_VERSION: &str = "coordination.coordinator-health/v1beta1";
pub const WORKER_DISPATCH_SCHEMA_VERSION: &str = "coordination.worker-dispatch/v1beta1";
pub const WORKER_DISPATCH_ACCEPTED_SCHEMA_VERSION: &str =
    "coordination.worker-dispatch.accepted/v1beta1";
pub const WORKER_CANCEL_SCHEMA_VERSION: &str = "coordination.worker-cancel/v1beta1";
pub const CCPANES_PREFLIGHT_REQUEST_SCHEMA_VERSION: &str =
    "coordination.ccpanes-preflight-request/v1beta1";
pub const CCPANES_SYNTHETIC_CANARY_REQUEST_SCHEMA_VERSION: &str =
    "coordination.ccpanes-synthetic-canary-request/v1beta1";
pub const WORKFLOW_CREATE_SCHEMA_VERSION: &str = "coordination.workflow-create/v1beta1";
pub const WORKFLOW_START_SCHEMA_VERSION: &str = "coordination.workflow-start/v1beta1";
pub const WORKFLOW_CANCEL_SCHEMA_VERSION: &str = "coordination.workflow-cancel/v1beta1";
pub const COMMAND_ACK_SCHEMA_VERSION: &str = "coordination.command-ack/v1beta1";
pub const STATUS_QUERY_SCHEMA_VERSION: &str = "coordination.status-query/v1beta1";
pub const STATUS_RESPONSE_SCHEMA_VERSION: &str = "coordination.status-response/v1beta1";
pub const RESULT_QUERY_SCHEMA_VERSION: &str = "coordination.result-query/v1beta1";
pub const RESULT_RESPONSE_SCHEMA_VERSION: &str = "coordination.result-response/v1beta1";
pub const EVENT_QUERY_SCHEMA_VERSION: &str = "coordination.event-query/v1beta1";
pub const EVENT_PAGE_SCHEMA_VERSION: &str = "coordination.event-page/v1beta1";

pub fn require_schema_version(actual: &str, expected: &str) -> Result<(), CoordinationError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CoordinationError::schema_version_unsupported(
            actual, expected,
        ))
    }
}

pub fn negotiate_schema_version<'a>(
    requested: &str,
    supported: &'a [&'a str],
) -> Result<&'a str, CoordinationError> {
    supported
        .iter()
        .copied()
        .find(|version| *version == requested)
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::SchemaVersionUnsupported,
                format!("schema version {requested} is not in supported set"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_schema_version_negotiation_accepts_v1beta1() {
        assert_eq!(
            negotiate_schema_version(IPC_SCHEMA_VERSION, &[IPC_SCHEMA_VERSION]).unwrap(),
            IPC_SCHEMA_VERSION
        );
    }

    #[test]
    fn unknown_schema_version_is_typed_error() {
        let error = negotiate_schema_version("coordination.ipc/v9", &[IPC_SCHEMA_VERSION])
            .expect_err("unknown version");
        assert_eq!(error.code, ErrorCode::SchemaVersionUnsupported);
    }
}
