use serde::{Deserialize, Serialize};

use crate::{
    EventId, OutboxId, OutboxState, Revision, WorkerId, WorkerState, WorkflowId, WorkflowState,
};

pub const COORDINATION_EVENT_SCHEMA_VERSION: &str = "coordination.event/v1beta1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CoordinationEvent {
    pub schema_version: String,
    pub event_id: EventId,
    pub workflow_id: WorkflowId,
    pub sequence: u64,
    pub revision: Revision,
    pub kind: CoordinationEventKind,
    pub correlation_id: String,
}

impl CoordinationEvent {
    pub fn is_supported_schema(&self) -> bool {
        self.schema_version == COORDINATION_EVENT_SCHEMA_VERSION
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum CoordinationEventKind {
    WorkflowCreated,
    WorkflowStateChanged {
        from: WorkflowState,
        to: WorkflowState,
    },
    WorkerCreated {
        worker_id: WorkerId,
    },
    WorkerStateChanged {
        worker_id: WorkerId,
        from: WorkerState,
        to: WorkerState,
    },
    OutboxStateChanged {
        outbox_id: OutboxId,
        from: OutboxState,
        to: OutboxState,
    },
    ResultRecorded {
        worker_id: WorkerId,
        truncated: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_schema_version_is_explicit() {
        let event = CoordinationEvent {
            schema_version: COORDINATION_EVENT_SCHEMA_VERSION.to_owned(),
            event_id: EventId::try_new("event-1").unwrap(),
            workflow_id: WorkflowId::try_new("workflow-1").unwrap(),
            sequence: 1,
            revision: Revision::new(1),
            kind: CoordinationEventKind::WorkflowCreated,
            correlation_id: "corr".to_owned(),
        };
        assert!(event.is_supported_schema());
        assert_eq!(
            serde_json::to_value(event).unwrap()["kind"]["type"],
            "workflow_created"
        );
    }
}
