use coordination_core::{
    CoordinationCommand, CoordinationError, CoordinationQuery, ErrorCode, RequestId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mcp::{PaneForgePreflightInput, PaneForgeSyntheticCanaryInput};
use crate::version::{require_schema_version, IPC_SCHEMA_VERSION};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IpcEnvelope {
    pub schema_version: String,
    pub request_id: RequestId,
    pub message: IpcMessage,
}

impl IpcEnvelope {
    pub fn from_value(value: Value) -> Result<Self, CoordinationError> {
        let envelope: Self = serde_json::from_value(value).map_err(|error| {
            CoordinationError::new(ErrorCode::InvalidRequest, format!("invalid IPC: {error}"))
        })?;
        envelope.validate_contract()?;
        Ok(envelope)
    }

    pub fn validate_contract(&self) -> Result<(), CoordinationError> {
        require_schema_version(&self.schema_version, IPC_SCHEMA_VERSION)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum IpcMessage {
    Hello(IpcHello),
    Health(IpcHealthRequest),
    PaneForgePreflight(PaneForgePreflightInput),
    PaneForgeSyntheticCanary(PaneForgeSyntheticCanaryInput),
    Command(Box<CoordinationCommand>),
    Query(Box<CoordinationQuery>),
}

impl IpcMessage {
    pub fn command(command: CoordinationCommand) -> Self {
        Self::Command(Box::new(command))
    }

    pub fn query(query: CoordinationQuery) -> Self {
        Self::Query(Box::new(query))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IpcHello {
    pub client_name: String,
    pub client_version: String,
    pub protocol_version: String,
    pub nonce: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IpcHealthRequest {
    pub protocol_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IpcResponseEnvelope {
    pub schema_version: String,
    pub request_id: RequestId,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<IpcResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CoordinationError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum IpcResponse {
    Hello { protocol_version: String },
    Health(IpcHealthResponse),
    Accepted,
    Json { value: Value },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IpcHealthResponse {
    pub schema_version: String,
    pub protocol_version: String,
    pub runtime_identity: String,
    pub store_schema_version: i32,
    pub status: CoordinatorHealthStatus,
    pub dispatch_ready: bool,
    pub capacity: IpcCapacitySnapshot,
    pub metrics: IpcMetricsSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CoordinatorHealthStatus {
    Healthy,
    Frozen,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IpcCapacitySnapshot {
    pub host_max_active_workers: u32,
    pub active_workers: u32,
    pub queued_workers: u32,
    pub dispatch_inflight_limit: usize,
    pub dispatch_inflight_available: usize,
    pub reconcile_inflight_limit: usize,
    pub reconcile_inflight_available: usize,
    pub control_inflight_limit: usize,
    pub control_inflight_available: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IpcMetricsSnapshot {
    pub ticks: u64,
    pub idle_ticks: u64,
    pub dispatch_claims: u64,
    pub dispatch_frozen: u64,
    pub backpressure_rejections: u64,
    pub cancellations: u64,
    pub aggregation_queries: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use coordination_core::ErrorCode;

    #[test]
    fn ipc_envelope_accepts_supported_schema() {
        let value = json!({
            "schemaVersion": IPC_SCHEMA_VERSION,
            "requestId": "ipc-1",
            "message": {
                "type": "hello",
                "payload": {
                    "clientName": "coordination-mcp",
                    "clientVersion": "0.1.0-beta.3",
                    "protocolVersion": IPC_SCHEMA_VERSION,
                    "nonce": "nonce"
                }
            }
        });
        let envelope = IpcEnvelope::from_value(value).expect("valid IPC envelope");
        assert_eq!(envelope.request_id.as_str(), "ipc-1");
    }

    #[test]
    fn ipc_health_message_and_response_keep_schema_versions_explicit() {
        let value = json!({
            "schemaVersion": IPC_SCHEMA_VERSION,
            "requestId": "ipc-health-1",
            "message": {
                "type": "health",
                "payload": {
                    "protocolVersion": IPC_SCHEMA_VERSION,
                    "runtimeIdentity": "runtime-digest"
                }
            }
        });
        let envelope = IpcEnvelope::from_value(value).expect("valid health envelope");
        assert!(matches!(envelope.message, IpcMessage::Health(_)));

        let response = IpcResponse::Health(IpcHealthResponse {
            schema_version: crate::COORDINATOR_HEALTH_SCHEMA_VERSION.to_owned(),
            protocol_version: IPC_SCHEMA_VERSION.to_owned(),
            runtime_identity: "runtime-digest".to_owned(),
            store_schema_version: 1,
            status: CoordinatorHealthStatus::Healthy,
            dispatch_ready: true,
            capacity: IpcCapacitySnapshot {
                host_max_active_workers: 24,
                active_workers: 0,
                queued_workers: 0,
                dispatch_inflight_limit: 6,
                dispatch_inflight_available: 6,
                reconcile_inflight_limit: 24,
                reconcile_inflight_available: 24,
                control_inflight_limit: 12,
                control_inflight_available: 12,
            },
            metrics: IpcMetricsSnapshot {
                ticks: 0,
                idle_ticks: 0,
                dispatch_claims: 0,
                dispatch_frozen: 0,
                backpressure_rejections: 0,
                cancellations: 0,
                aggregation_queries: 0,
            },
        });
        let serialized = serde_json::to_value(response).unwrap();
        assert_eq!(
            serialized["payload"]["schemaVersion"],
            crate::COORDINATOR_HEALTH_SCHEMA_VERSION
        );
    }

    #[test]
    fn ipc_envelope_rejects_unknown_schema_version() {
        let value = json!({
            "schemaVersion": "coordination.ipc/v9",
            "requestId": "ipc-1",
            "message": {
                "type": "hello",
                "payload": {
                    "clientName": "coordination-mcp",
                    "clientVersion": "0.1.0-beta.3",
                    "protocolVersion": "coordination.ipc/v9",
                    "nonce": "nonce"
                }
            }
        });
        let error = IpcEnvelope::from_value(value).expect_err("unknown IPC schema");
        assert_eq!(error.code, ErrorCode::SchemaVersionUnsupported);
    }
}
