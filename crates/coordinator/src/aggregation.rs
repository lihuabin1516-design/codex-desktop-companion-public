use std::collections::{BTreeMap, BTreeSet};

use coordination_core::{FanInPolicy, ResultAggregationState, WorkerId, WorkerState, WorkflowId};
use coordination_protocol::{
    WorkerResultSummary, WorkflowResultResponse, RESULT_RESPONSE_SCHEMA_VERSION,
};
use coordination_store::{CoordinationStore, ResultRecord, WorkerRecord};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AggregatedWorkflowResult {
    pub workflow_id: WorkflowId,
    pub revision: coordination_core::Revision,
    pub state: ResultAggregationState,
    pub workers: Vec<WorkerResultSummary>,
    pub missing_results: Vec<WorkerId>,
    pub failed_workers: Vec<WorkerId>,
}

impl AggregatedWorkflowResult {
    pub fn to_protocol_response(&self) -> WorkflowResultResponse {
        WorkflowResultResponse {
            schema_version: RESULT_RESPONSE_SCHEMA_VERSION.to_owned(),
            workflow_id: self.workflow_id.clone(),
            state: self.state,
            revision: self.revision,
            workers: self.workers.clone(),
            missing_results: self.missing_results.clone(),
            failed_workers: self.failed_workers.clone(),
        }
    }
}

pub fn aggregate_workflow_result(
    store: &CoordinationStore,
    workflow_id: &WorkflowId,
) -> Result<AggregatedWorkflowResult, coordination_core::CoordinationError> {
    let workflow = store.workflow(workflow_id)?;
    let workers = store.workers_for_workflow(workflow_id)?;
    let results = store.workflow_results(workflow_id)?;
    let result_by_worker = results
        .into_iter()
        .map(|result| (result.worker_id.clone(), result))
        .collect::<BTreeMap<_, _>>();

    if workers.is_empty() {
        return Ok(AggregatedWorkflowResult {
            workflow_id: workflow_id.clone(),
            revision: workflow.revision,
            state: ResultAggregationState::Unavailable,
            workers: Vec::new(),
            missing_results: Vec::new(),
            failed_workers: Vec::new(),
        });
    }

    let required_steps = workflow
        .spec
        .workflow
        .steps
        .iter()
        .filter(|step| step.required)
        .map(|step| step.step_id.clone())
        .collect::<BTreeSet<_>>();
    let mut summaries = Vec::new();
    let mut missing_results = Vec::new();
    let mut failed_workers = Vec::new();
    let mut successful_required = 0_u32;
    let mut required_total = 0_u32;
    let mut any_active = false;

    for worker in &workers {
        if !worker.state.is_terminal() {
            any_active = true;
        }
        let is_required = required_steps.is_empty() || required_steps.contains(&worker.step_id);
        if is_required {
            required_total = required_total.saturating_add(1);
            if worker.state == WorkerState::Succeeded {
                successful_required = successful_required.saturating_add(1);
            }
        }
        if matches!(worker.state, WorkerState::Failed | WorkerState::Cancelled) {
            failed_workers.push(worker.worker_id.clone());
        }
        match result_by_worker.get(&worker.worker_id) {
            Some(result) => summaries.push(summary_from_record(worker, result)),
            None if worker.state.is_terminal() => missing_results.push(worker.worker_id.clone()),
            None => {}
        }
    }

    let policy_complete = match workflow.spec.workflow.fan_in {
        FanInPolicy::FirstSuccess => successful_required > 0,
        FanInPolicy::AllowPartial { min_successful } => successful_required >= min_successful,
        FanInPolicy::AllRequired => {
            missing_results.is_empty()
                && failed_workers.is_empty()
                && successful_required == required_total
        }
    };

    let state = if any_active {
        ResultAggregationState::Pending
    } else if summaries.is_empty() {
        ResultAggregationState::Unavailable
    } else if policy_complete
        && (failed_workers.is_empty() || workflow.spec.workflow.fan_in == FanInPolicy::FirstSuccess)
    {
        ResultAggregationState::Complete
    } else {
        ResultAggregationState::Partial
    };

    Ok(AggregatedWorkflowResult {
        workflow_id: workflow_id.clone(),
        revision: workflow.revision,
        state,
        workers: summaries,
        missing_results,
        failed_workers,
    })
}

fn summary_from_record(worker: &WorkerRecord, result: &ResultRecord) -> WorkerResultSummary {
    WorkerResultSummary {
        worker_id: worker.worker_id.clone(),
        step_id: worker.step_id.clone(),
        state: result.worker_state,
        summary: result.summary.clone(),
        completed_at_ms: result.completed_at_ms,
        truncated: result.truncated,
        artifact_sha256: result.artifact_sha256.clone(),
    }
}
