use std::collections::{BTreeMap, BTreeSet, VecDeque};

use coordination_core::{
    AccessMode, CoordinationError, OutboxId, WorkerId, WorkflowId, WorkflowState,
};
use coordination_store::{
    CoordinationStore, OutboxRecord, StoreCapacityCounts, WorkflowWorkerCounts,
};
use serde::{Deserialize, Serialize};

use crate::CoordinatorConfig;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DispatchCandidate {
    pub outbox_id: OutboxId,
    pub workflow_id: WorkflowId,
    pub worker_id: WorkerId,
    pub command_type: String,
    pub attempt: u32,
    pub generation: u64,
}

impl DispatchCandidate {
    pub fn from_outbox(outbox: &OutboxRecord) -> Option<Self> {
        Some(Self {
            outbox_id: outbox.outbox_id.clone(),
            workflow_id: outbox.workflow_id.clone(),
            worker_id: outbox.worker_id.clone()?,
            command_type: outbox.command_type.clone(),
            attempt: outbox.attempt,
            generation: outbox.generation,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerPlan {
    pub dispatches: Vec<DispatchCandidate>,
    pub next_wait_ms: u64,
    pub frozen: bool,
    pub backpressured: bool,
}

impl SchedulerPlan {
    pub fn idle(next_wait_ms: u64) -> Self {
        Self {
            dispatches: Vec::new(),
            next_wait_ms,
            frozen: false,
            backpressured: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicy {
    pub min_ms: u64,
    pub max_ms: u64,
}

impl BackoffPolicy {
    pub const fn new(min_ms: u64, max_ms: u64) -> Self {
        Self { min_ms, max_ms }
    }

    pub fn crash_delay_ms(self, consecutive_failures: u32, salt: &str) -> u64 {
        if consecutive_failures == 0 {
            return 0;
        }
        let exponent = consecutive_failures.saturating_sub(1).min(16);
        let multiplier = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
        let base = self.min_ms.saturating_mul(multiplier).min(self.max_ms);
        let jitter_window = (self.min_ms / 4).max(1);
        let jitter = stable_jitter(salt, consecutive_failures) % jitter_window;
        base.saturating_add(jitter).min(self.max_ms)
    }
}

pub fn select_fair_dispatches(
    store: &CoordinationStore,
    due: Vec<OutboxRecord>,
    capacity: StoreCapacityCounts,
    config: &CoordinatorConfig,
    dispatch_available: usize,
    queue_available: usize,
    now_ms: i64,
) -> Result<SchedulerPlan, CoordinationError> {
    if due.is_empty() {
        return Ok(SchedulerPlan::idle(config.idle_wait_ms));
    }
    if !config.dispatch_enabled || dispatch_available == 0 || queue_available == 0 {
        return Ok(SchedulerPlan {
            dispatches: Vec::new(),
            next_wait_ms: config.idle_wait_ms,
            frozen: !config.dispatch_enabled || dispatch_available == 0,
            backpressured: queue_available == 0,
        });
    }

    let host_slots = config
        .host_max_active_workers
        .saturating_sub(capacity.active_workers) as usize;
    let max_dispatches = host_slots
        .min(dispatch_available)
        .min(queue_available)
        .min(due.len());
    if max_dispatches == 0 {
        return Ok(SchedulerPlan {
            dispatches: Vec::new(),
            next_wait_ms: config.idle_wait_ms,
            frozen: false,
            backpressured: true,
        });
    }

    let mut order = Vec::<WorkflowId>::new();
    let mut groups = BTreeMap::<WorkflowId, VecDeque<OutboxRecord>>::new();
    for outbox in due {
        if outbox.worker_id.is_none() || outbox.command_type != "launch_task" {
            continue;
        }
        if !groups.contains_key(&outbox.workflow_id) {
            order.push(outbox.workflow_id.clone());
        }
        groups
            .entry(outbox.workflow_id.clone())
            .or_default()
            .push_back(outbox);
    }

    let mut counts = BTreeMap::<WorkflowId, WorkflowWorkerCounts>::new();
    let mut planned_per_workflow = BTreeMap::<WorkflowId, u32>::new();
    let mut planned_write_resources = BTreeSet::<String>::new();
    let mut selected = Vec::new();
    let mut made_progress = true;
    while selected.len() < max_dispatches && made_progress {
        made_progress = false;
        for workflow_id in &order {
            if selected.len() >= max_dispatches {
                break;
            }
            let Some(group) = groups.get_mut(workflow_id) else {
                continue;
            };
            let Some(candidate_outbox) = group.front() else {
                continue;
            };
            let workflow = store.workflow(workflow_id)?;
            if !matches!(workflow.state, WorkflowState::Running) {
                group.pop_front();
                continue;
            }
            let workflow_counts = match counts.get(workflow_id) {
                Some(existing) => *existing,
                None => {
                    let loaded = store.workflow_worker_counts(workflow_id)?;
                    counts.insert(workflow_id.clone(), loaded);
                    loaded
                }
            };
            let planned = planned_per_workflow.get(workflow_id).copied().unwrap_or(0);
            let max_parallel = workflow.spec.workflow.scheduling.max_parallel_workers;
            let workflow_slots =
                max_parallel.saturating_sub(workflow_counts.active_workers.saturating_add(planned));
            let tick_slots = config
                .max_dispatches_per_workflow_tick
                .saturating_sub(planned);
            if workflow_slots == 0 || tick_slots == 0 {
                continue;
            }
            let Some(candidate) = DispatchCandidate::from_outbox(candidate_outbox) else {
                group.pop_front();
                continue;
            };
            let write_resource =
                match write_resource_for_candidate(store, &workflow, &candidate, now_ms)? {
                    CandidateWriteResource::Read => None,
                    CandidateWriteResource::Blocked => continue,
                    CandidateWriteResource::Available(resource) => Some(resource),
                };
            if let Some(resource) = write_resource.as_ref() {
                if planned_write_resources.contains(resource) {
                    continue;
                }
            }
            group.pop_front();
            if let Some(resource) = write_resource {
                planned_write_resources.insert(resource);
            }
            selected.push(candidate);
            planned_per_workflow.insert(workflow_id.clone(), planned.saturating_add(1));
            made_progress = true;
        }
    }

    Ok(SchedulerPlan {
        dispatches: selected,
        next_wait_ms: config.idle_wait_ms,
        frozen: false,
        backpressured: false,
    })
}

enum CandidateWriteResource {
    Read,
    Available(String),
    Blocked,
}

fn write_resource_for_candidate(
    store: &CoordinationStore,
    workflow: &coordination_store::WorkflowRecord,
    candidate: &DispatchCandidate,
    now_ms: i64,
) -> Result<CandidateWriteResource, CoordinationError> {
    let worker = store.worker(&candidate.worker_id)?;
    if worker.access != AccessMode::Write {
        return Ok(CandidateWriteResource::Read);
    }
    let resource = canonical_worktree_resource(&workflow.spec.workflow.repository.path);
    if store.has_active_exclusive_lease(&resource, now_ms)? {
        Ok(CandidateWriteResource::Blocked)
    } else {
        Ok(CandidateWriteResource::Available(resource))
    }
}

pub fn canonical_worktree_resource(path: &str) -> String {
    let normalized = path.trim().replace('\\', "/");
    let without_trailing = normalized.trim_end_matches('/');
    without_trailing.to_ascii_lowercase()
}

fn stable_jitter(salt: &str, failures: u32) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in salt
        .as_bytes()
        .iter()
        .copied()
        .chain(failures.to_le_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
