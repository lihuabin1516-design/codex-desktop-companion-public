use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};

use paneforge_adapter::dedicated_workspace_name_for_project_path;
use coordination_core::{
    AccessMode, BudgetSpec, CallerId, CoordinationCommand, CoordinationError, CoordinationEvent,
    CoordinationQuery, ErrorCode, ExecutorSpec, FanInPolicy, IdempotencyKey, LeaseId, OutboxId,
    RepositoryRef, RequestId, Revision, SchedulingSpec, StepId, StepSpec, WorkerExecutor, WorkerId,
    WorkerState, WorkflowDefinition, WorkflowId, WorkflowMetadata, WorkflowSpecV1Beta1,
    WorkflowState, WORKFLOW_SPEC_KIND, WORKFLOW_SPEC_SCHEMA_VERSION,
};
use coordination_protocol::{
    AdapterStatusResponse, CommandAck, CoordinatorHealthStatus, ExternalBindingStatus,
    IpcCapacitySnapshot, IpcEnvelope, IpcHealthRequest, IpcHealthResponse, IpcMessage,
    IpcMetricsSnapshot, IpcResponse, IpcResponseEnvelope, WorkerDispatchAccepted,
    WorkerStatusResponse, WorkflowEventsResponse, WorkflowStatusResponse, WorkflowWorkerStatus,
    COORDINATOR_HEALTH_SCHEMA_VERSION, EVENT_PAGE_SCHEMA_VERSION, IPC_SCHEMA_VERSION,
    STATUS_RESPONSE_SCHEMA_VERSION,
};
use coordination_store::{
    AcquireLeaseRequest, CoordinationStore, CreateWorkflowRequest, EnqueueWorkerDispatchRequest,
    LeaseMode, OutboxRecord, RequestExternalWorkerCancelRequest, WorkerRecord,
};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    aggregate_workflow_result, canonical_worktree_resource, BoundedQueue, CoordinatorConfig,
    CoordinatorSemaphores, DispatchCandidate, LogLevel, MetricsSnapshot, RuntimeIdentity,
    RuntimePermit, SchedulerPlan, StructuredLogRecord,
};

#[derive(Debug)]
pub struct ClaimedDispatch {
    pub outbox: OutboxRecord,
    pub permit: RuntimePermit,
}

#[derive(Debug)]
pub struct ClaimedControl {
    pub outbox: OutboxRecord,
    pub permit: RuntimePermit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAdvance {
    pub workflow_id: WorkflowId,
    pub revision: Revision,
    pub workflow_state: WorkflowState,
    pub enqueued_workers: Vec<WorkerId>,
    pub cancelled_workers: Vec<WorkerId>,
    pub timed_out_workers: Vec<WorkerId>,
}

impl WorkflowAdvance {
    fn from_workflow(workflow: &coordination_store::WorkflowRecord) -> Self {
        Self {
            workflow_id: workflow.workflow_id.clone(),
            revision: workflow.revision,
            workflow_state: workflow.state,
            enqueued_workers: Vec::new(),
            cancelled_workers: Vec::new(),
            timed_out_workers: Vec::new(),
        }
    }

    fn refresh_from(&mut self, workflow: &coordination_store::WorkflowRecord) {
        self.revision = workflow.revision;
        self.workflow_state = workflow.state;
    }
}

#[derive(Debug)]
pub struct CoordinatorRuntime {
    store: CoordinationStore,
    config: CoordinatorConfig,
    identity: RuntimeIdentity,
    semaphores: CoordinatorSemaphores,
    dispatch_queue: BoundedQueue<DispatchCandidate>,
    metrics: MetricsSnapshot,
    logs: Vec<StructuredLogRecord>,
    dispatch_ready: bool,
    data_dir: PathBuf,
}

impl CoordinatorRuntime {
    pub fn open(
        database_path: impl AsRef<Path>,
        data_dir: impl AsRef<Path>,
        ipc_endpoint: impl AsRef<str>,
        config: CoordinatorConfig,
    ) -> Result<Self, CoordinationError> {
        config.validate()?;
        let data_dir = data_dir.as_ref().to_path_buf();
        let store = CoordinationStore::open(database_path.as_ref())?;
        let identity = RuntimeIdentity::new(&data_dir, database_path, ipc_endpoint);
        let mut runtime = Self::from_store(store, identity, config)?;
        runtime.data_dir = data_dir;
        Ok(runtime)
    }

    pub fn from_store(
        store: CoordinationStore,
        identity: RuntimeIdentity,
        config: CoordinatorConfig,
    ) -> Result<Self, CoordinationError> {
        config.validate()?;
        let semaphores = CoordinatorSemaphores::new(
            config.dispatch_inflight,
            config.reconcile_inflight,
            config.control_inflight,
        );
        let dispatch_queue = BoundedQueue::new(config.work_queue_capacity)?;
        let data_dir = store
            .path()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Self {
            store,
            config,
            identity,
            semaphores,
            dispatch_queue,
            metrics: MetricsSnapshot::default(),
            logs: Vec::new(),
            dispatch_ready: false,
            data_dir,
        })
    }

    pub fn store(&self) -> &CoordinationStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut CoordinationStore {
        &mut self.store
    }

    pub fn identity(&self) -> &RuntimeIdentity {
        &self.identity
    }

    pub fn metrics(&self) -> MetricsSnapshot {
        self.metrics
    }

    pub fn logs(&self) -> &[StructuredLogRecord] {
        &self.logs
    }

    pub fn set_dispatch_ready(&mut self, ready: bool) {
        self.dispatch_ready = ready;
    }

    pub fn health_response(&self) -> Result<IpcHealthResponse, CoordinationError> {
        let capacity = self.store.capacity_counts()?;
        let status = if !self.config.dispatch_enabled || !self.dispatch_ready {
            CoordinatorHealthStatus::Frozen
        } else {
            CoordinatorHealthStatus::Healthy
        };
        Ok(IpcHealthResponse {
            schema_version: COORDINATOR_HEALTH_SCHEMA_VERSION.to_owned(),
            protocol_version: IPC_SCHEMA_VERSION.to_owned(),
            runtime_identity: self.identity.digest.clone(),
            store_schema_version: self.store.schema_version()?,
            status,
            dispatch_ready: self.dispatch_ready && self.config.dispatch_enabled,
            capacity: IpcCapacitySnapshot {
                host_max_active_workers: self.config.host_max_active_workers,
                active_workers: capacity.active_workers,
                queued_workers: capacity.queued_workers,
                dispatch_inflight_limit: self.semaphores.dispatch.limit(),
                dispatch_inflight_available: self.semaphores.dispatch.available(),
                reconcile_inflight_limit: self.semaphores.reconcile.limit(),
                reconcile_inflight_available: self.semaphores.reconcile.available(),
                control_inflight_limit: self.semaphores.control.limit(),
                control_inflight_available: self.semaphores.control.available(),
            },
            metrics: IpcMetricsSnapshot {
                ticks: self.metrics.ticks,
                idle_ticks: self.metrics.idle_ticks,
                dispatch_claims: self.metrics.dispatch_claims,
                dispatch_frozen: self.metrics.dispatch_frozen,
                backpressure_rejections: self.metrics.backpressure_rejections,
                cancellations: self.metrics.cancellations,
                aggregation_queries: self.metrics.aggregation_queries,
            },
        })
    }

    pub fn handle_ipc(&mut self, envelope: IpcEnvelope) -> IpcResponseEnvelope {
        let request_id = envelope.request_id.clone();
        let result = match envelope.message {
            IpcMessage::Hello(hello) => coordination_protocol::require_schema_version(
                &hello.protocol_version,
                IPC_SCHEMA_VERSION,
            )
            .map(|()| IpcResponse::Hello {
                protocol_version: IPC_SCHEMA_VERSION.to_owned(),
            }),
            IpcMessage::Health(request) => self.handle_health_request(request),
            IpcMessage::PaneForgePreflight(input) => {
                crate::run_paneforge_preflight(input).map(|value| IpcResponse::Json { value })
            }
            IpcMessage::PaneForgeSyntheticCanary(input) => {
                crate::run_paneforge_synthetic_canary(input, &self.data_dir)
                    .map(|value| IpcResponse::Json { value })
            }
            IpcMessage::Command(command) => self.handle_command(*command),
            IpcMessage::Query(query) => self.handle_query(*query),
        };
        match result {
            Ok(response) => IpcResponseEnvelope {
                schema_version: IPC_SCHEMA_VERSION.to_owned(),
                request_id,
                ok: true,
                response: Some(response),
                error: None,
            },
            Err(error) => IpcResponseEnvelope {
                schema_version: IPC_SCHEMA_VERSION.to_owned(),
                request_id,
                ok: false,
                response: None,
                error: Some(error),
            },
        }
    }

    pub fn tick(&mut self, now_ms: i64) -> Result<SchedulerPlan, CoordinationError> {
        self.metrics.ticks = self.metrics.ticks.saturating_add(1);
        for workflow in self.store.running_workflows()? {
            let _ = self.advance_workflow(&workflow.workflow_id, now_ms)?;
        }
        if !self.config.dispatch_enabled || !self.dispatch_ready {
            self.metrics.dispatch_frozen = self.metrics.dispatch_frozen.saturating_add(1);
            return Ok(SchedulerPlan {
                dispatches: Vec::new(),
                next_wait_ms: self.config.idle_wait_ms,
                frozen: true,
                backpressured: false,
            });
        }

        let due = self
            .store
            .due_outbox(now_ms, self.config.due_outbox_scan_limit)?;
        if due.is_empty() {
            self.metrics.idle_ticks = self.metrics.idle_ticks.saturating_add(1);
            return Ok(SchedulerPlan::idle(self.config.idle_wait_ms));
        }
        let capacity = self.store.capacity_counts()?;
        let plan = crate::select_fair_dispatches(
            &self.store,
            due,
            capacity,
            &self.config,
            self.semaphores.dispatch.available(),
            self.dispatch_queue.available(),
            now_ms,
        )?;
        for candidate in &plan.dispatches {
            if let Err(error) = self.dispatch_queue.try_push(candidate.clone()) {
                self.metrics.backpressure_rejections =
                    self.metrics.backpressure_rejections.saturating_add(1);
                self.push_log(
                    StructuredLogRecord::new(LogLevel::Warn, "dispatch_queue_backpressure")
                        .with_field("errorCode", error.code.as_str()),
                );
                break;
            }
        }
        if plan.dispatches.is_empty() && plan.backpressured {
            self.metrics.backpressure_rejections =
                self.metrics.backpressure_rejections.saturating_add(1);
        }
        Ok(plan)
    }

    pub fn claim_next_dispatch(
        &mut self,
        now_ms: i64,
    ) -> Result<Option<ClaimedDispatch>, CoordinationError> {
        let Some(candidate) = self.dispatch_queue.pop_front() else {
            return Ok(None);
        };
        let Some(permit) = self.semaphores.dispatch.try_acquire() else {
            self.dispatch_queue.try_push(candidate)?;
            return Ok(None);
        };
        let lease_id = match self.acquire_dispatch_lease(&candidate) {
            Ok(lease_id) => lease_id,
            Err(error) if error.code == ErrorCode::ExternalBindingConflict => {
                self.dispatch_queue.try_push(candidate)?;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let claimed = self.store.claim_outbox(&candidate.outbox_id, now_ms)?;
        if let Some(outbox) = claimed {
            self.metrics.dispatch_claims = self.metrics.dispatch_claims.saturating_add(1);
            Ok(Some(ClaimedDispatch { outbox, permit }))
        } else {
            if let Some(lease_id) = lease_id {
                let _ = self
                    .store
                    .release_lease(&lease_id, "outbox claim skipped")?;
            }
            Ok(None)
        }
    }

    pub fn claim_next_control(
        &mut self,
        now_ms: i64,
    ) -> Result<Option<ClaimedControl>, CoordinationError> {
        let Some(candidate) = self
            .store
            .due_outbox(now_ms, self.config.due_outbox_scan_limit)?
            .into_iter()
            .find(|outbox| outbox.command_type == "kill_session")
        else {
            return Ok(None);
        };
        let Some(permit) = self.semaphores.control.try_acquire() else {
            return Ok(None);
        };
        let claimed = self.store.claim_outbox(&candidate.outbox_id, now_ms)?;
        Ok(claimed.map(|outbox| ClaimedControl { outbox, permit }))
    }

    fn acquire_dispatch_lease(
        &mut self,
        candidate: &DispatchCandidate,
    ) -> Result<Option<LeaseId>, CoordinationError> {
        let worker = self.store.worker(&candidate.worker_id)?;
        if worker.access != AccessMode::Write {
            return Ok(None);
        }
        let workflow = self.store.workflow(&candidate.workflow_id)?;
        let resource = canonical_worktree_resource(&workflow.spec.workflow.repository.path);
        let lease_id = lease_id_for_key("lease", &candidate.worker_id)?;
        self.store.acquire_lease(AcquireLeaseRequest {
            lease_id: lease_id.clone(),
            canonical_resource: resource.clone(),
            resource_sha256: sha256_hex(resource.as_bytes()),
            mode: LeaseMode::ExclusiveWrite,
            owner_workflow_id: candidate.workflow_id.clone(),
            owner_worker_id: Some(candidate.worker_id.clone()),
            ttl_ms: self.config.worker_lease_ttl_ms,
        })?;
        Ok(Some(lease_id))
    }

    pub fn request_worker_cancel(
        &mut self,
        worker_id: &WorkerId,
        expected_revision: Option<Revision>,
        reason: Option<&str>,
    ) -> Result<coordination_store::WorkerRecord, CoordinationError> {
        let worker = self.store.worker(worker_id)?;
        let updated = if worker.state == WorkerState::Queued {
            self.store.cancel_queued_worker_before_dispatch(
                worker_id,
                expected_revision,
                reason,
                "coordinator-worker-cancel",
            )?
        } else if self
            .store
            .external_binding_for_worker(worker_id)?
            .is_some_and(|binding| {
                binding.external_session_ref.is_some()
                    && matches!(
                        binding.state,
                        coordination_store::ExternalBindingState::Active
                            | coordination_store::ExternalBindingState::ResultUnknown
                            | coordination_store::ExternalBindingState::CancelRequested
                    )
            })
        {
            self.store
                .request_external_worker_cancel(RequestExternalWorkerCancelRequest {
                    worker_id: worker_id.clone(),
                    expected_worker_revision: expected_revision,
                    outbox_id: cancel_outbox_id_for_worker(worker_id)?,
                    reason: reason.map(str::to_owned),
                    correlation_id: "coordinator-worker-cancel".to_owned(),
                    not_before_ms: 0,
                })?
                .worker
        } else if worker.state == WorkerState::Cancelling {
            worker
        } else {
            let expected = expected_revision.unwrap_or(worker.revision);
            self.store.transition_worker_state(
                worker_id,
                expected,
                WorkerState::Cancelling,
                "coordinator-worker-cancel",
            )?
        };
        self.metrics.cancellations = self.metrics.cancellations.saturating_add(1);
        self.push_log(
            StructuredLogRecord::new(LogLevel::Info, "worker_cancel_requested")
                .with_field("workerId", worker_id.as_str()),
        );
        Ok(updated)
    }

    pub fn request_workflow_cancel(
        &mut self,
        workflow_id: &WorkflowId,
        expected_revision: Revision,
        reason: Option<&str>,
    ) -> Result<coordination_store::WorkflowRecord, CoordinationError> {
        let workflow = self.store.transition_workflow_state(
            workflow_id,
            expected_revision,
            WorkflowState::Cancelling,
            "coordinator-workflow-cancel",
        )?;
        let workers = self.store.workers_for_workflow(workflow_id)?;
        for worker in workers {
            if !worker.state.is_terminal() {
                let _ =
                    self.request_worker_cancel(&worker.worker_id, Some(worker.revision), reason)?;
            }
        }
        Ok(workflow)
    }

    pub fn start_workflow(
        &mut self,
        workflow_id: &WorkflowId,
        expected_revision: Revision,
        now_ms: i64,
    ) -> Result<WorkflowAdvance, CoordinationError> {
        let workflow = self.store.workflow(workflow_id)?;
        match workflow.state {
            WorkflowState::Draft => {
                let ready = self.store.transition_workflow_state(
                    workflow_id,
                    expected_revision,
                    WorkflowState::Ready,
                    "coordinator-workflow-start-ready",
                )?;
                self.store.transition_workflow_state(
                    workflow_id,
                    ready.revision,
                    WorkflowState::Running,
                    "coordinator-workflow-start-running",
                )?;
            }
            WorkflowState::Ready | WorkflowState::Paused => {
                self.store.transition_workflow_state(
                    workflow_id,
                    expected_revision,
                    WorkflowState::Running,
                    "coordinator-workflow-start-running",
                )?;
            }
            WorkflowState::Running => {
                require_current_revision(expected_revision, workflow.revision)?;
            }
            other => {
                return Err(CoordinationError::invalid_transition(
                    "workflow",
                    other.as_str(),
                    WorkflowState::Running.as_str(),
                ));
            }
        }
        self.advance_workflow(workflow_id, now_ms)
    }

    pub fn advance_workflow(
        &mut self,
        workflow_id: &WorkflowId,
        now_ms: i64,
    ) -> Result<WorkflowAdvance, CoordinationError> {
        let workflow = self.store.workflow(workflow_id)?;
        let mut advance = WorkflowAdvance::from_workflow(&workflow);
        self.expire_timed_out_workers(workflow_id, now_ms, &mut advance)?;
        self.control_wall_clock_timeout(workflow_id, now_ms, &mut advance)?;
        self.cancel_remaining_after_first_success(workflow_id, &mut advance)?;
        self.enqueue_runnable_steps(workflow_id, &mut advance)?;
        self.finalize_workflow_if_ready(workflow_id, &mut advance)?;
        let workflow = self.store.workflow(workflow_id)?;
        advance.refresh_from(&workflow);
        Ok(advance)
    }

    pub fn pause_workflow(
        &mut self,
        workflow_id: &WorkflowId,
        expected_revision: Revision,
    ) -> Result<coordination_store::WorkflowRecord, CoordinationError> {
        self.store.transition_workflow_state(
            workflow_id,
            expected_revision,
            WorkflowState::Paused,
            "coordinator-workflow-pause",
        )
    }

    pub fn resume_workflow(
        &mut self,
        workflow_id: &WorkflowId,
        expected_revision: Revision,
    ) -> Result<coordination_store::WorkflowRecord, CoordinationError> {
        let counts = self.store.workflow_worker_counts(workflow_id)?;
        let target = if counts.active_workers > 0 {
            WorkflowState::Running
        } else {
            WorkflowState::Ready
        };
        self.store.transition_workflow_state(
            workflow_id,
            expected_revision,
            target,
            "coordinator-workflow-resume",
        )
    }

    pub fn aggregate_workflow_result(
        &mut self,
        workflow_id: &WorkflowId,
    ) -> Result<crate::AggregatedWorkflowResult, CoordinationError> {
        self.metrics.aggregation_queries = self.metrics.aggregation_queries.saturating_add(1);
        aggregate_workflow_result(&self.store, workflow_id)
    }

    fn expire_timed_out_workers(
        &mut self,
        workflow_id: &WorkflowId,
        now_ms: i64,
        advance: &mut WorkflowAdvance,
    ) -> Result<(), CoordinationError> {
        let workflow = self.store.workflow(workflow_id)?;
        let step_timeout_by_id = workflow
            .spec
            .workflow
            .steps
            .iter()
            .map(|step| (step.step_id.clone(), step.timeout_seconds))
            .collect::<BTreeMap<_, _>>();
        let workers = self.store.workers_for_workflow(workflow_id)?;
        for worker in workers {
            if !matches!(
                worker.state,
                WorkerState::Dispatching | WorkerState::DispatchUnknown | WorkerState::Running
            ) {
                continue;
            }
            let timeout_seconds = step_timeout_by_id
                .get(&worker.step_id)
                .copied()
                .unwrap_or(workflow.spec.workflow.budget.timeout_seconds);
            let started_at_ms = worker.started_at_ms.unwrap_or(worker.updated_at_ms);
            if deadline_elapsed(started_at_ms, timeout_seconds, now_ms) {
                let timed_out = self.store.transition_worker_state(
                    &worker.worker_id,
                    worker.revision,
                    WorkerState::Failed,
                    "coordinator-worker-timeout",
                )?;
                advance.timed_out_workers.push(timed_out.worker_id);
            }
        }
        Ok(())
    }

    fn control_wall_clock_timeout(
        &mut self,
        workflow_id: &WorkflowId,
        now_ms: i64,
        advance: &mut WorkflowAdvance,
    ) -> Result<(), CoordinationError> {
        let workflow = self.store.workflow(workflow_id)?;
        if !matches!(
            workflow.state,
            WorkflowState::Running | WorkflowState::Cancelling
        ) {
            return Ok(());
        }
        let Some(started_at_ms) = workflow.started_at_ms else {
            return Ok(());
        };
        if !deadline_elapsed(
            started_at_ms,
            workflow.spec.workflow.budget.timeout_seconds,
            now_ms,
        ) {
            return Ok(());
        }
        for worker in self.store.workers_for_workflow(workflow_id)? {
            if worker.state.is_terminal() || worker.state == WorkerState::Cancelling {
                continue;
            }
            let cancelled = self.request_worker_cancel(
                &worker.worker_id,
                Some(worker.revision),
                Some("workflow timeout"),
            )?;
            advance.cancelled_workers.push(cancelled.worker_id);
        }
        let workflow = self.store.workflow(workflow_id)?;
        if matches!(
            workflow.state,
            WorkflowState::Ready | WorkflowState::Running | WorkflowState::Paused
        ) {
            let cancelled = self.store.transition_workflow_state(
                workflow_id,
                workflow.revision,
                WorkflowState::Cancelling,
                "coordinator-workflow-timeout",
            )?;
            advance.refresh_from(&cancelled);
        }
        Ok(())
    }

    fn cancel_remaining_after_first_success(
        &mut self,
        workflow_id: &WorkflowId,
        advance: &mut WorkflowAdvance,
    ) -> Result<(), CoordinationError> {
        let workflow = self.store.workflow(workflow_id)?;
        if workflow.state != WorkflowState::Running
            || workflow.spec.workflow.fan_in != FanInPolicy::FirstSuccess
        {
            return Ok(());
        }
        let workers = self.store.workers_for_workflow(workflow_id)?;
        if !workers
            .iter()
            .any(|worker| worker.state == WorkerState::Succeeded)
        {
            return Ok(());
        }
        for worker in workers {
            if worker.state.is_terminal() || worker.state == WorkerState::Cancelling {
                continue;
            }
            let cancelled = self.request_worker_cancel(
                &worker.worker_id,
                Some(worker.revision),
                Some("first-success fan-in satisfied"),
            )?;
            advance.cancelled_workers.push(cancelled.worker_id);
        }
        Ok(())
    }

    fn enqueue_runnable_steps(
        &mut self,
        workflow_id: &WorkflowId,
        advance: &mut WorkflowAdvance,
    ) -> Result<(), CoordinationError> {
        let workflow = self.store.workflow(workflow_id)?;
        if workflow.state != WorkflowState::Running {
            return Ok(());
        }
        let existing_workers = self.store.workers_for_workflow(workflow_id)?;
        if workflow.spec.workflow.fan_in == FanInPolicy::FirstSuccess
            && existing_workers
                .iter()
                .any(|worker| worker.state == WorkerState::Succeeded)
        {
            return Ok(());
        }
        let spec_step_ids = workflow
            .spec
            .workflow
            .steps
            .iter()
            .map(|step| step.step_id.clone())
            .collect::<BTreeSet<_>>();
        if existing_workers
            .iter()
            .any(|worker| !spec_step_ids.contains(&worker.step_id))
        {
            return Ok(());
        }
        let mut state_by_step = existing_workers
            .iter()
            .map(|worker| (worker.step_id.clone(), worker.state))
            .collect::<BTreeMap<_, _>>();
        let mut worker_budget_used = existing_workers.len() as u32;
        for step in workflow.spec.workflow.steps.clone() {
            if state_by_step.contains_key(&step.step_id) {
                continue;
            }
            if worker_budget_used >= workflow.spec.workflow.budget.max_workers {
                return Err(CoordinationError::new(
                    ErrorCode::BudgetExhausted,
                    "workflow maxWorkers budget is exhausted",
                ));
            }
            if !step
                .depends_on
                .iter()
                .all(|dependency| state_by_step.get(dependency) == Some(&WorkerState::Succeeded))
            {
                continue;
            }
            let dispatch = self.enqueue_workflow_step(&workflow, &step)?;
            state_by_step.insert(step.step_id.clone(), WorkerState::Queued);
            worker_budget_used = worker_budget_used.saturating_add(1);
            advance
                .enqueued_workers
                .push(dispatch.value.worker.worker_id);
        }
        Ok(())
    }

    fn enqueue_workflow_step(
        &mut self,
        workflow: &coordination_store::WorkflowRecord,
        step: &StepSpec,
    ) -> Result<
        coordination_store::Idempotent<coordination_store::WorkerDispatchRecord>,
        CoordinationError,
    > {
        let caller_id = CallerId::try_new(workflow.spec.metadata.caller_id.clone())?;
        let request_id = RequestId::try_new(stable_id(
            "request",
            &[workflow.workflow_id.as_str(), step.step_id.as_str()],
        ))?;
        let idempotency_key =
            IdempotencyKey::try_new("workflow_step_dispatch", caller_id, request_id)?;
        let worker_id = worker_id_for_key("worker", &idempotency_key)?;
        let outbox_id = outbox_id_for_key("outbox", &idempotency_key)?;
        let binding_id = binding_id_for_key("binding", &idempotency_key)?;
        let dispatch_marker = stable_marker(&idempotency_key);
        let prompt_sha256 = stable_hex(&[
            idempotency_key.tool.as_str(),
            idempotency_key.caller_id.as_str(),
            idempotency_key.request_id.as_str(),
            workflow.spec.workflow.repository.path.as_str(),
            step.objective.as_str(),
        ]);
        let payload = launch_task_payload(
            &workflow.spec.workflow.repository,
            &step.objective,
            &step.executor.cli_tool,
            &idempotency_key,
            &dispatch_marker,
            &prompt_sha256,
        );
        let latest = self.store.workflow(&workflow.workflow_id)?;
        self.store
            .enqueue_worker_dispatch(EnqueueWorkerDispatchRequest {
                idempotency_key,
                workflow_id: workflow.workflow_id.clone(),
                expected_workflow_revision: latest.revision,
                worker_id,
                step_id: step.step_id.clone(),
                outbox_id,
                binding_id,
                objective: step.objective.clone(),
                access: step.access,
                adapter: "paneforge-http-mcp".to_owned(),
                dispatch_marker,
                prompt_sha256,
                payload,
                correlation_id: "coordinator-workflow-step-dispatch".to_owned(),
                not_before_ms: 0,
                retention_until_ms: workflow.retention_until_ms,
            })
    }

    fn finalize_workflow_if_ready(
        &mut self,
        workflow_id: &WorkflowId,
        advance: &mut WorkflowAdvance,
    ) -> Result<(), CoordinationError> {
        let workflow = self.store.workflow(workflow_id)?;
        if !matches!(
            workflow.state,
            WorkflowState::Running | WorkflowState::Cancelling
        ) {
            return Ok(());
        }
        let workers = self.store.workers_for_workflow(workflow_id)?;
        if workers.iter().any(|worker| !worker.state.is_terminal()) {
            return Ok(());
        }
        let state_by_step = workers
            .iter()
            .map(|worker| (worker.step_id.clone(), worker.state))
            .collect::<BTreeMap<_, _>>();
        if workflow.state == WorkflowState::Running
            && workflow_has_runnable_uncreated_step(&workflow, &state_by_step)
        {
            return Ok(());
        }
        let target = if workflow.state == WorkflowState::Cancelling {
            if workers
                .iter()
                .any(|worker| worker.state == WorkerState::Failed)
            {
                WorkflowState::Failed
            } else {
                WorkflowState::Cancelled
            }
        } else {
            terminal_state_for_policy(
                &workflow.spec.workflow.fan_in,
                &workflow.spec.workflow.steps,
                &workers,
            )
        };
        let updated = self.store.transition_workflow_state(
            workflow_id,
            workflow.revision,
            target,
            "coordinator-workflow-fan-in",
        )?;
        advance.refresh_from(&updated);
        Ok(())
    }

    fn handle_command(
        &mut self,
        command: CoordinationCommand,
    ) -> Result<IpcResponse, CoordinationError> {
        match command {
            CoordinationCommand::WorkflowCreate {
                idempotency_key,
                spec,
            } => self.handle_workflow_create(idempotency_key, spec),
            CoordinationCommand::WorkflowStart {
                workflow_id,
                expected_revision,
            } => {
                let advance = self.start_workflow(&workflow_id, expected_revision, 0)?;
                let mut ack = CommandAck::accepted(advance.revision);
                ack.workflow_id = Some(advance.workflow_id);
                ack.state = Some(advance.workflow_state.as_str().to_owned());
                json_response(ack)
            }
            CoordinationCommand::WorkflowCancel {
                workflow_id,
                expected_revision,
                reason,
            } => {
                let workflow = self.request_workflow_cancel(
                    &workflow_id,
                    expected_revision,
                    reason.as_deref(),
                )?;
                let mut ack = CommandAck::accepted(workflow.revision);
                ack.workflow_id = Some(workflow.workflow_id);
                ack.state = Some(workflow.state.as_str().to_owned());
                json_response(ack)
            }
            CoordinationCommand::WorkerDispatch {
                idempotency_key,
                workflow_id,
                repository,
                objective,
                access,
                executor,
                timeout_seconds,
            } => self.handle_worker_dispatch(
                idempotency_key,
                workflow_id,
                repository,
                objective,
                access,
                executor,
                timeout_seconds,
            ),
            CoordinationCommand::WorkerCancel {
                worker_id,
                expected_revision,
                reason,
            } => {
                let worker =
                    self.request_worker_cancel(&worker_id, expected_revision, reason.as_deref())?;
                let mut ack = CommandAck::accepted(worker.revision);
                ack.workflow_id = Some(worker.workflow_id);
                ack.worker_id = Some(worker.worker_id);
                ack.state = Some(worker.state.as_str().to_owned());
                json_response(ack)
            }
        }
    }

    fn handle_query(&mut self, query: CoordinationQuery) -> Result<IpcResponse, CoordinationError> {
        match query {
            CoordinationQuery::CoordinatorStatus => self
                .health_response()
                .and_then(json_response_value)
                .map(|value| IpcResponse::Json { value }),
            CoordinationQuery::AdapterStatus => {
                let response = AdapterStatusResponse {
                    schema_version: STATUS_RESPONSE_SCHEMA_VERSION.to_owned(),
                    readiness: if self.dispatch_ready && self.config.dispatch_enabled {
                        "READY".to_owned()
                    } else {
                        "FROZEN".to_owned()
                    },
                    dispatch_ready: self.dispatch_ready && self.config.dispatch_enabled,
                    reason: Some(
                        "adapter runtime connection remains outside the Phase 5 synthetic seam"
                            .to_owned(),
                    ),
                };
                json_response(response)
            }
            CoordinationQuery::WorkerStatus { worker_id } => {
                let worker = self.store.worker(&worker_id)?;
                let external_binding =
                    self.store
                        .external_binding_for_worker(&worker_id)?
                        .map(|binding| ExternalBindingStatus {
                            binding_id: binding.binding_id,
                            adapter: binding.adapter,
                            generation: binding.generation,
                            state: binding.state.as_str().to_owned(),
                            has_external_session: binding.external_session_ref.is_some(),
                        });
                let response = WorkerStatusResponse {
                    schema_version: STATUS_RESPONSE_SCHEMA_VERSION.to_owned(),
                    worker_id: worker.worker_id,
                    workflow_id: worker.workflow_id,
                    step_id: worker.step_id,
                    state: worker.state,
                    revision: worker.revision,
                    objective: worker.objective,
                    access: worker.access,
                    external_binding,
                };
                json_response(response)
            }
            CoordinationQuery::WorkflowStatus { workflow_id } => {
                let workflow = self.store.workflow(&workflow_id)?;
                let workers = self
                    .store
                    .workers_for_workflow(&workflow_id)?
                    .into_iter()
                    .map(|worker| WorkflowWorkerStatus {
                        worker_id: worker.worker_id,
                        step_id: worker.step_id,
                        state: worker.state,
                        revision: worker.revision,
                    })
                    .collect();
                let response = WorkflowStatusResponse {
                    schema_version: STATUS_RESPONSE_SCHEMA_VERSION.to_owned(),
                    workflow_id: workflow.workflow_id,
                    state: workflow.state,
                    revision: workflow.revision,
                    workers,
                };
                json_response(response)
            }
            CoordinationQuery::WorkflowEvents {
                workflow_id,
                after_sequence,
                limit,
            } => {
                let events = self.store.events(&workflow_id, after_sequence, limit)?;
                let next_after_sequence = events
                    .last()
                    .map(|event| event.sequence)
                    .unwrap_or(after_sequence);
                let events = events
                    .into_iter()
                    .map(|event| CoordinationEvent {
                        schema_version: event.schema_version,
                        event_id: event.event_id,
                        workflow_id: event.workflow_id,
                        sequence: event.sequence,
                        revision: event.revision,
                        kind: event.kind,
                        correlation_id: event.correlation_id,
                    })
                    .collect();
                let response = WorkflowEventsResponse {
                    schema_version: EVENT_PAGE_SCHEMA_VERSION.to_owned(),
                    workflow_id,
                    events,
                    next_after_sequence,
                };
                json_response(response)
            }
            CoordinationQuery::WorkflowResult { workflow_id } => {
                let aggregate = self.aggregate_workflow_result(&workflow_id)?;
                json_response(aggregate.to_protocol_response())
            }
        }
    }

    fn handle_workflow_create(
        &mut self,
        idempotency_key: IdempotencyKey,
        spec: WorkflowSpecV1Beta1,
    ) -> Result<IpcResponse, CoordinationError> {
        let workflow_id = workflow_id_for_key("workflow", &idempotency_key)?;
        let created = self.store.create_workflow(CreateWorkflowRequest {
            idempotency_key,
            workflow_id,
            spec,
            correlation_id: "ipc-workflow-create".to_owned(),
            retention_until_ms: None,
        })?;
        let mut ack = CommandAck::accepted(created.value.revision);
        ack.workflow_id = Some(created.value.workflow_id);
        ack.state = Some(created.value.state.as_str().to_owned());
        ack.replayed = created.replayed;
        json_response(ack)
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_worker_dispatch(
        &mut self,
        idempotency_key: IdempotencyKey,
        workflow_id: Option<WorkflowId>,
        repository: RepositoryRef,
        objective: String,
        access: AccessMode,
        executor: WorkerExecutor,
        timeout_seconds: u64,
    ) -> Result<IpcResponse, CoordinationError> {
        let workflow_id = match workflow_id {
            Some(workflow_id) => workflow_id,
            None => {
                let workflow_id = workflow_id_for_key("workflow", &idempotency_key)?;
                let spec = single_worker_spec(
                    &workflow_id,
                    &idempotency_key,
                    &repository,
                    &objective,
                    access,
                    &executor,
                    timeout_seconds,
                )?;
                let create_key =
                    derived_idempotency_key(&idempotency_key, "worker_dispatch.workflow")?;
                let created = self.store.create_workflow(CreateWorkflowRequest {
                    idempotency_key: create_key,
                    workflow_id: workflow_id.clone(),
                    spec,
                    correlation_id: "ipc-worker-dispatch-create-workflow".to_owned(),
                    retention_until_ms: None,
                })?;
                self.ensure_workflow_running(
                    &created.value.workflow_id,
                    "ipc-worker-dispatch-start-workflow",
                )?;
                workflow_id
            }
        };
        let running =
            self.ensure_workflow_running(&workflow_id, "ipc-worker-dispatch-start-workflow")?;
        let worker_id = worker_id_for_key("worker", &idempotency_key)?;
        let step_id = step_id_for_key("step", &idempotency_key)?;
        let outbox_id = outbox_id_for_key("outbox", &idempotency_key)?;
        let binding_id = binding_id_for_key("binding", &idempotency_key)?;
        let dispatch_marker = stable_marker(&idempotency_key);
        let prompt_sha256 = stable_hex(&[
            idempotency_key.tool.as_str(),
            idempotency_key.caller_id.as_str(),
            idempotency_key.request_id.as_str(),
            repository.path.as_str(),
            objective.as_str(),
        ]);
        let payload = launch_task_payload(
            &repository,
            &objective,
            &executor.cli_tool,
            &idempotency_key,
            &dispatch_marker,
            &prompt_sha256,
        );
        let dispatch = self
            .store
            .enqueue_worker_dispatch(EnqueueWorkerDispatchRequest {
                idempotency_key,
                workflow_id,
                expected_workflow_revision: running.revision,
                worker_id,
                step_id,
                outbox_id,
                binding_id,
                objective,
                access,
                adapter: "paneforge-http-mcp".to_owned(),
                dispatch_marker,
                prompt_sha256,
                payload,
                correlation_id: "ipc-worker-dispatch".to_owned(),
                not_before_ms: 0,
                retention_until_ms: None,
            })?;
        json_response(WorkerDispatchAccepted {
            schema_version: coordination_protocol::WORKER_DISPATCH_ACCEPTED_SCHEMA_VERSION
                .to_owned(),
            accepted: true,
            workflow_id: dispatch.value.workflow.workflow_id,
            worker_id: dispatch.value.worker.worker_id,
            revision: dispatch.value.worker.revision,
            state: dispatch.value.worker.state,
        })
    }

    fn ensure_workflow_running(
        &mut self,
        workflow_id: &WorkflowId,
        correlation_id: &str,
    ) -> Result<coordination_store::WorkflowRecord, CoordinationError> {
        let workflow = self.store.workflow(workflow_id)?;
        match workflow.state {
            WorkflowState::Draft => {
                let ready = self.store.transition_workflow_state(
                    workflow_id,
                    workflow.revision,
                    WorkflowState::Ready,
                    correlation_id,
                )?;
                self.store.transition_workflow_state(
                    workflow_id,
                    ready.revision,
                    WorkflowState::Running,
                    correlation_id,
                )
            }
            WorkflowState::Ready | WorkflowState::Paused => self.store.transition_workflow_state(
                workflow_id,
                workflow.revision,
                WorkflowState::Running,
                correlation_id,
            ),
            WorkflowState::Running => Ok(workflow),
            other => Err(CoordinationError::invalid_transition(
                "workflow",
                other.as_str(),
                "RUNNING",
            )),
        }
    }

    fn handle_health_request(
        &self,
        request: IpcHealthRequest,
    ) -> Result<IpcResponse, CoordinationError> {
        coordination_protocol::require_schema_version(
            &request.protocol_version,
            IPC_SCHEMA_VERSION,
        )?;
        if let Some(requested_identity) = request.runtime_identity {
            if requested_identity != self.identity.digest {
                return Err(CoordinationError::new(
                    ErrorCode::InternalInvariantViolation,
                    "coordinator runtime identity mismatch",
                ));
            }
        }
        self.health_response().map(IpcResponse::Health)
    }

    fn push_log(&mut self, record: StructuredLogRecord) {
        if self.logs.len() >= 256 {
            self.logs.remove(0);
        }
        self.logs.push(record);
    }
}

fn json_response<T: Serialize>(value: T) -> Result<IpcResponse, CoordinationError> {
    json_response_value(value).map(|value| IpcResponse::Json { value })
}

fn json_response_value<T: Serialize>(value: T) -> Result<Value, CoordinationError> {
    serde_json::to_value(value).map_err(|error| {
        CoordinationError::new(
            ErrorCode::InternalInvariantViolation,
            format!("serialize IPC response failed: {error}"),
        )
    })
}

fn single_worker_spec(
    workflow_id: &WorkflowId,
    idempotency_key: &IdempotencyKey,
    repository: &RepositoryRef,
    objective: &str,
    access: AccessMode,
    executor: &WorkerExecutor,
    timeout_seconds: u64,
) -> Result<WorkflowSpecV1Beta1, CoordinationError> {
    Ok(WorkflowSpecV1Beta1 {
        schema_version: WORKFLOW_SPEC_SCHEMA_VERSION.to_owned(),
        kind: WORKFLOW_SPEC_KIND.to_owned(),
        metadata: WorkflowMetadata {
            name: workflow_id.as_str().to_owned(),
            caller_id: idempotency_key.caller_id.as_str().to_owned(),
            labels: Default::default(),
        },
        workflow: WorkflowDefinition {
            objective: objective.to_owned(),
            repository: repository.clone(),
            scheduling: SchedulingSpec {
                max_parallel_workers: 1,
                max_writer_workers_per_worktree: 1,
            },
            budget: BudgetSpec {
                max_workers: 1,
                timeout_seconds,
                ..BudgetSpec::default()
            },
            fan_in: FanInPolicy::AllRequired,
            steps: vec![StepSpec {
                step_id: step_id_for_key("step", idempotency_key)?,
                objective: objective.to_owned(),
                depends_on: Vec::new(),
                access,
                executor: ExecutorSpec {
                    cli_tool: executor.cli_tool.clone(),
                },
                timeout_seconds,
                required: true,
            }],
        },
    })
}

fn launch_task_payload(
    repository: &RepositoryRef,
    objective: &str,
    cli_tool: &str,
    idempotency_key: &IdempotencyKey,
    dispatch_marker: &str,
    prompt_sha256: &str,
) -> Value {
    let launch_workspace_name = launch_workspace_name_for_repository(repository);
    let mut payload = json!({
        "projectPath": repository.path.clone(),
        "prompt": format!("{objective}\n\n[coordination-marker:{dispatch_marker}]\n[prompt-sha256:{prompt_sha256}]"),
        "title": format!("codex-paneforge {dispatch_marker}"),
        "cliTool": cli_tool,
        "clientRequestId": idempotency_key.request_id.as_str(),
        "workspaceName": launch_workspace_name,
        "createdAtMs": 0
    });
    if let Some(source_workspace_name) = repository.workspace_name.as_deref() {
        if let Some(object) = payload.as_object_mut() {
            object.insert(
                "sourceWorkspaceName".to_owned(),
                Value::String(source_workspace_name.to_owned()),
            );
        }
    }
    payload
}

fn launch_workspace_name_for_repository(repository: &RepositoryRef) -> String {
    dedicated_workspace_name_for_project_path(&repository.path)
}

fn require_current_revision(
    expected: Revision,
    current: Revision,
) -> Result<(), CoordinationError> {
    if expected == current {
        Ok(())
    } else {
        Err(CoordinationError::revision_conflict(expected, current))
    }
}

fn deadline_elapsed(started_at_ms: i64, timeout_seconds: u64, now_ms: i64) -> bool {
    let timeout_ms = i64::try_from(timeout_seconds.saturating_mul(1_000)).unwrap_or(i64::MAX);
    now_ms >= started_at_ms.saturating_add(timeout_ms)
}

fn workflow_has_runnable_uncreated_step(
    workflow: &coordination_store::WorkflowRecord,
    state_by_step: &BTreeMap<StepId, WorkerState>,
) -> bool {
    workflow.spec.workflow.steps.iter().any(|step| {
        !state_by_step.contains_key(&step.step_id)
            && step
                .depends_on
                .iter()
                .all(|dependency| state_by_step.get(dependency) == Some(&WorkerState::Succeeded))
    })
}

fn terminal_state_for_policy(
    policy: &FanInPolicy,
    steps: &[StepSpec],
    workers: &[WorkerRecord],
) -> WorkflowState {
    let state_by_step = workers
        .iter()
        .map(|worker| (worker.step_id.clone(), worker.state))
        .collect::<BTreeMap<_, _>>();
    match policy {
        FanInPolicy::AllRequired => {
            let required_steps = steps
                .iter()
                .filter(|step| step.required)
                .map(|step| step.step_id.clone())
                .collect::<BTreeSet<_>>();
            let required_succeeded = required_steps
                .iter()
                .all(|step_id| state_by_step.get(step_id) == Some(&WorkerState::Succeeded));
            if !required_succeeded {
                return WorkflowState::Failed;
            }
            if workers
                .iter()
                .any(|worker| matches!(worker.state, WorkerState::Failed | WorkerState::Cancelled))
            {
                WorkflowState::PartialFailed
            } else {
                WorkflowState::Succeeded
            }
        }
        FanInPolicy::AllowPartial { min_successful } => {
            let successes = workers
                .iter()
                .filter(|worker| worker.state == WorkerState::Succeeded)
                .count() as u32;
            if successes < *min_successful {
                WorkflowState::Failed
            } else if successes as usize == steps.len()
                && workers
                    .iter()
                    .all(|worker| worker.state == WorkerState::Succeeded)
            {
                WorkflowState::Succeeded
            } else {
                WorkflowState::PartialFailed
            }
        }
        FanInPolicy::FirstSuccess => {
            if workers
                .iter()
                .any(|worker| worker.state == WorkerState::Succeeded)
            {
                WorkflowState::Succeeded
            } else {
                WorkflowState::Failed
            }
        }
    }
}

fn derived_idempotency_key(
    source: &IdempotencyKey,
    tool: &'static str,
) -> Result<IdempotencyKey, CoordinationError> {
    IdempotencyKey::try_new(tool, source.caller_id.clone(), source.request_id.clone())
}

fn stable_marker(idempotency_key: &IdempotencyKey) -> String {
    format!(
        "marker-{}",
        &stable_hex(&[
            idempotency_key.tool.as_str(),
            idempotency_key.caller_id.as_str(),
            idempotency_key.request_id.as_str(),
        ])[..16]
    )
}

fn workflow_id_for_key(
    prefix: &'static str,
    key: &IdempotencyKey,
) -> Result<WorkflowId, CoordinationError> {
    WorkflowId::try_new(stable_id(
        prefix,
        &[
            key.tool.as_str(),
            key.caller_id.as_str(),
            key.request_id.as_str(),
        ],
    ))
}

fn worker_id_for_key(
    prefix: &'static str,
    key: &IdempotencyKey,
) -> Result<WorkerId, CoordinationError> {
    WorkerId::try_new(stable_id(
        prefix,
        &[
            key.tool.as_str(),
            key.caller_id.as_str(),
            key.request_id.as_str(),
        ],
    ))
}

fn step_id_for_key(
    prefix: &'static str,
    key: &IdempotencyKey,
) -> Result<StepId, CoordinationError> {
    StepId::try_new(stable_id(
        prefix,
        &[
            key.tool.as_str(),
            key.caller_id.as_str(),
            key.request_id.as_str(),
        ],
    ))
}

fn outbox_id_for_key(
    prefix: &'static str,
    key: &IdempotencyKey,
) -> Result<OutboxId, CoordinationError> {
    OutboxId::try_new(stable_id(
        prefix,
        &[
            key.tool.as_str(),
            key.caller_id.as_str(),
            key.request_id.as_str(),
        ],
    ))
}

fn cancel_outbox_id_for_worker(worker_id: &WorkerId) -> Result<OutboxId, CoordinationError> {
    OutboxId::try_new(stable_id("cancel-outbox", &[worker_id.as_str()]))
}

fn binding_id_for_key(
    prefix: &'static str,
    key: &IdempotencyKey,
) -> Result<coordination_core::ExternalBindingId, CoordinationError> {
    coordination_core::ExternalBindingId::try_new(stable_id(
        prefix,
        &[
            key.tool.as_str(),
            key.caller_id.as_str(),
            key.request_id.as_str(),
        ],
    ))
}

fn lease_id_for_key(
    prefix: &'static str,
    worker_id: &WorkerId,
) -> Result<LeaseId, CoordinationError> {
    LeaseId::try_new(stable_id(prefix, &[worker_id.as_str()]))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn stable_id(prefix: &str, parts: &[&str]) -> String {
    format!("{prefix}-{}", &stable_hex(parts)[..24])
}

fn stable_hex(parts: &[&str]) -> String {
    let mut output = String::with_capacity(64);
    for lane in 0..4_u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64 ^ lane;
        for part in parts {
            for byte in part.as_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            hash ^= 0xff;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        output.push_str(&format!("{hash:016x}"));
    }
    output
}
