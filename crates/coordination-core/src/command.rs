use serde::{Deserialize, Serialize};

use crate::{
    AccessMode, IdempotencyKey, RepositoryRef, Revision, StepId, WorkerId, WorkflowId,
    WorkflowSpecV1Beta1,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum CoordinationCommand {
    WorkflowCreate {
        idempotency_key: IdempotencyKey,
        spec: WorkflowSpecV1Beta1,
    },
    WorkflowStart {
        workflow_id: WorkflowId,
        expected_revision: Revision,
    },
    WorkflowCancel {
        workflow_id: WorkflowId,
        expected_revision: Revision,
        reason: Option<String>,
    },
    WorkerDispatch {
        idempotency_key: IdempotencyKey,
        workflow_id: Option<WorkflowId>,
        repository: RepositoryRef,
        objective: String,
        access: AccessMode,
        executor: WorkerExecutor,
        timeout_seconds: u64,
    },
    WorkerCancel {
        worker_id: WorkerId,
        expected_revision: Option<Revision>,
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerExecutor {
    pub cli_tool: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum CoordinationQuery {
    CoordinatorStatus,
    AdapterStatus,
    WorkerStatus {
        worker_id: WorkerId,
    },
    WorkflowStatus {
        workflow_id: WorkflowId,
    },
    WorkflowEvents {
        workflow_id: WorkflowId,
        after_sequence: u64,
        limit: u32,
    },
    WorkflowResult {
        workflow_id: WorkflowId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StepCommandTarget {
    pub workflow_id: WorkflowId,
    pub step_id: StepId,
    pub expected_revision: Revision,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallerId, RequestId};

    #[test]
    fn command_serialization_keeps_command_and_query_meaning_distinct() {
        let command = CoordinationCommand::WorkerDispatch {
            idempotency_key: IdempotencyKey::try_new(
                "worker_dispatch",
                CallerId::try_new("caller").unwrap(),
                RequestId::try_new("request").unwrap(),
            )
            .unwrap(),
            workflow_id: None,
            repository: crate::RepositoryRef {
                path: "D:/repo".to_owned(),
                expected_head: Some("HEAD".to_owned()),
                dirty_policy: crate::DirtyPolicy::Reject,
                workspace_name: None,
            },
            objective: "inspect".to_owned(),
            access: AccessMode::Read,
            executor: WorkerExecutor {
                cli_tool: "codex".to_owned(),
            },
            timeout_seconds: 120,
        };
        let query = CoordinationQuery::CoordinatorStatus;

        let command_json = serde_json::to_value(command).expect("serialize command");
        let query_json = serde_json::to_value(query).expect("serialize query");
        assert_eq!(command_json["type"], "worker_dispatch");
        assert_eq!(query_json["type"], "coordinator_status");
    }
}
