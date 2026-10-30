use std::fmt;
use std::path::PathBuf;

use coordination_core::{
    AccessMode, ExternalBindingId, IdempotencyKey, LeaseId, OutboxId, OutboxState,
    ResultAggregationState, Revision, StepId, WorkerId, WorkerState, WorkflowId,
    WorkflowSpecV1Beta1, WorkflowState,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreOptions {
    pub migration_backup_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationRecord {
    pub version: i32,
    pub description: String,
    pub applied_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityReport {
    pub sqlite_result: String,
    pub event_chains_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Idempotent<T> {
    pub value: T,
    pub replayed: bool,
}

impl<T> Idempotent<T> {
    pub const fn created(value: T) -> Self {
        Self {
            value,
            replayed: false,
        }
    }

    pub const fn replayed(value: T) -> Self {
        Self {
            value,
            replayed: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRecord {
    pub workflow_id: WorkflowId,
    pub state: WorkflowState,
    pub revision: Revision,
    pub spec: WorkflowSpecV1Beta1,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
    pub retention_until_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRecord {
    pub worker_id: WorkerId,
    pub workflow_id: WorkflowId,
    pub step_id: StepId,
    pub state: WorkerState,
    pub revision: Revision,
    pub objective: String,
    pub access: AccessMode,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreCapacityCounts {
    pub active_workers: u32,
    pub queued_workers: u32,
    pub running_workflows: u32,
    pub ready_workflows: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkflowWorkerCounts {
    pub active_workers: u32,
    pub queued_workers: u32,
    pub terminal_workers: u32,
    pub total_workers: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExternalBindingState {
    Pending,
    Active,
    ResultUnknown,
    CancelRequested,
    Succeeded,
    Failed,
    Cancelled,
    Superseded,
}

impl ExternalBindingState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Active => "ACTIVE",
            Self::ResultUnknown => "RESULT_UNKNOWN",
            Self::CancelRequested => "CANCEL_REQUESTED",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
            Self::Superseded => "SUPERSEDED",
        }
    }
}

impl fmt::Display for ExternalBindingState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalBindingRecord {
    pub binding_id: ExternalBindingId,
    pub workflow_id: WorkflowId,
    pub worker_id: WorkerId,
    pub adapter: String,
    pub generation: u64,
    pub dispatch_marker: String,
    pub prompt_sha256: String,
    pub external_task_ref: Option<String>,
    pub external_session_ref: Option<String>,
    pub state: ExternalBindingState,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_reconciled_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxRecord {
    pub outbox_id: OutboxId,
    pub workflow_id: WorkflowId,
    pub worker_id: Option<WorkerId>,
    pub state: OutboxState,
    pub revision: Revision,
    pub attempt: u32,
    pub generation: u64,
    pub command_type: String,
    pub payload: Value,
    pub idempotency_scope: Option<String>,
    pub not_before_ms: i64,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LeaseMode {
    SharedRead,
    ExclusiveWrite,
}

impl LeaseMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SharedRead => "SHARED_READ",
            Self::ExclusiveWrite => "EXCLUSIVE_WRITE",
        }
    }
}

impl fmt::Display for LeaseMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseRecord {
    pub lease_id: LeaseId,
    pub canonical_resource: String,
    pub resource_sha256: String,
    pub mode: LeaseMode,
    pub owner_workflow_id: WorkflowId,
    pub owner_worker_id: Option<WorkerId>,
    pub generation: u64,
    pub acquired_at_ms: i64,
    pub heartbeat_at_ms: i64,
    pub expires_at_ms: i64,
    pub released_at_ms: Option<i64>,
    pub release_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultRecord {
    pub workflow_id: WorkflowId,
    pub worker_id: WorkerId,
    pub worker_state: WorkerState,
    pub aggregation_state: ResultAggregationState,
    pub summary: String,
    pub artifact_sha256: Option<String>,
    pub truncated: bool,
    pub completed_at_ms: i64,
    pub retention_until_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionRecord {
    pub entity_kind: String,
    pub entity_id: String,
    pub retain_until_ms: Option<i64>,
    pub max_bytes: Option<u64>,
    pub reason: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    pub event_id: coordination_core::EventId,
    pub workflow_id: WorkflowId,
    pub sequence: u64,
    pub revision: Revision,
    pub schema_version: String,
    pub kind: coordination_core::CoordinationEventKind,
    pub correlation_id: String,
    pub occurred_at_ms: i64,
    pub prev_hash: String,
    pub event_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorkflowRequest {
    pub idempotency_key: IdempotencyKey,
    pub workflow_id: WorkflowId,
    pub spec: WorkflowSpecV1Beta1,
    pub correlation_id: String,
    pub retention_until_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnqueueWorkerDispatchRequest {
    pub idempotency_key: IdempotencyKey,
    pub workflow_id: WorkflowId,
    pub expected_workflow_revision: Revision,
    pub worker_id: WorkerId,
    pub step_id: StepId,
    pub outbox_id: OutboxId,
    pub binding_id: ExternalBindingId,
    pub objective: String,
    pub access: AccessMode,
    pub adapter: String,
    pub dispatch_marker: String,
    pub prompt_sha256: String,
    pub payload: Value,
    pub correlation_id: String,
    pub not_before_ms: i64,
    pub retention_until_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerDispatchRecord {
    pub workflow: WorkflowRecord,
    pub worker: WorkerRecord,
    pub binding: ExternalBindingRecord,
    pub outbox: OutboxRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquireLeaseRequest {
    pub lease_id: LeaseId,
    pub canonical_resource: String,
    pub resource_sha256: String,
    pub mode: LeaseMode,
    pub owner_workflow_id: WorkflowId,
    pub owner_worker_id: Option<WorkerId>,
    pub ttl_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordWorkerResultRequest {
    pub workflow_id: WorkflowId,
    pub worker_id: WorkerId,
    pub worker_state: WorkerState,
    pub aggregation_state: ResultAggregationState,
    pub summary: String,
    pub artifact_sha256: Option<String>,
    pub truncated: bool,
    pub completed_at_ms: i64,
    pub retention_until_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordExternalLaunchSuccessRequest {
    pub binding_id: ExternalBindingId,
    pub expected_generation: u64,
    pub external_task_ref: Option<String>,
    pub external_session_ref: String,
    pub correlation_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RequestExternalWorkerCancelRequest {
    pub worker_id: WorkerId,
    pub expected_worker_revision: Option<Revision>,
    pub outbox_id: OutboxId,
    pub reason: Option<String>,
    pub correlation_id: String,
    pub not_before_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerCancelRecord {
    pub worker: WorkerRecord,
    pub binding: ExternalBindingRecord,
    pub outbox: OutboxRecord,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordExternalCancelSuccessRequest {
    pub binding_id: ExternalBindingId,
    pub correlation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertRetentionRequest {
    pub entity_kind: String,
    pub entity_id: String,
    pub retain_until_ms: Option<i64>,
    pub max_bytes: Option<u64>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct WorkflowReplay {
    pub workflow_id: WorkflowId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct WorkerDispatchReplay {
    pub workflow_id: WorkflowId,
    pub worker_id: WorkerId,
    pub binding_id: ExternalBindingId,
    pub outbox_id: OutboxId,
}
