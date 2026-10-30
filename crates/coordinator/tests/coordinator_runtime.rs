use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use coordination_coordinator::{
    BackoffPolicy, BoundedQueue, CoordinatorConfig, CoordinatorRuntime, RuntimeIdentity,
    SingleInstanceGuard,
};
use coordination_core::{
    AccessMode, BudgetSpec, CallerId, DirtyPolicy, ErrorCode, ExecutorSpec, ExternalBindingId,
    FanInPolicy, IdempotencyKey, OutboxId, OutboxState, RepositoryRef, RequestId,
    ResultAggregationState, Revision, SchedulingSpec, StepId, StepSpec, WorkerId, WorkerState,
    WorkflowDefinition, WorkflowId, WorkflowMetadata, WorkflowSpecV1Beta1, WorkflowState,
    WORKFLOW_SPEC_KIND, WORKFLOW_SPEC_SCHEMA_VERSION,
};
use coordination_protocol::{
    IpcEnvelope, IpcHealthRequest, IpcMessage, IpcResponse, IPC_SCHEMA_VERSION,
};
use coordination_store::{
    CoordinationStore, CreateWorkflowRequest, EnqueueWorkerDispatchRequest, ExternalBindingState,
    OutboxRecord, RecordWorkerResultRequest, WorkerRecord,
};
use serde_json::json;
use tempfile::tempdir;

fn db_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("coordination.db")
}

fn workflow_id(value: &str) -> WorkflowId {
    WorkflowId::try_new(value).unwrap()
}

fn worker_id(value: &str) -> WorkerId {
    WorkerId::try_new(value).unwrap()
}

fn step_id(value: &str) -> StepId {
    StepId::try_new(value).unwrap()
}

fn outbox_id(value: &str) -> OutboxId {
    OutboxId::try_new(value).unwrap()
}

fn binding_id(value: &str) -> ExternalBindingId {
    ExternalBindingId::try_new(value).unwrap()
}

fn request_key(tool: &str, request: &str) -> IdempotencyKey {
    IdempotencyKey::try_new(
        tool,
        CallerId::try_new("codex-app").unwrap(),
        RequestId::try_new(request).unwrap(),
    )
    .unwrap()
}

fn valid_spec(name: &str, steps: &[&str]) -> WorkflowSpecV1Beta1 {
    WorkflowSpecV1Beta1 {
        schema_version: WORKFLOW_SPEC_SCHEMA_VERSION.to_owned(),
        kind: WORKFLOW_SPEC_KIND.to_owned(),
        metadata: WorkflowMetadata {
            name: name.to_owned(),
            caller_id: "codex-app".to_owned(),
            labels: BTreeMap::new(),
        },
        workflow: WorkflowDefinition {
            objective: format!("objective {name}"),
            repository: RepositoryRef {
                path: "D:/SYNTHETIC".to_owned(),
                expected_head: Some("HEAD".to_owned()),
                dirty_policy: DirtyPolicy::Reject,
                workspace_name: None,
            },
            scheduling: SchedulingSpec::default(),
            budget: BudgetSpec::default(),
            fan_in: FanInPolicy::AllRequired,
            steps: steps
                .iter()
                .map(|step| StepSpec {
                    step_id: step_id(step),
                    objective: format!("do {step}"),
                    depends_on: Vec::new(),
                    access: AccessMode::Read,
                    executor: ExecutorSpec::default(),
                    timeout_seconds: 120,
                    required: true,
                })
                .collect(),
        },
    }
}

fn phase6_spec(
    name: &str,
    step_count: u32,
    fan_in: FanInPolicy,
    max_parallel_workers: u32,
    access: AccessMode,
) -> WorkflowSpecV1Beta1 {
    let mut spec = valid_spec(name, &[]);
    spec.workflow.scheduling.max_parallel_workers = max_parallel_workers;
    spec.workflow.budget.max_workers = step_count;
    spec.workflow.budget.timeout_seconds = 1;
    spec.workflow.fan_in = fan_in;
    spec.workflow.steps = (1..=step_count)
        .map(|index| StepSpec {
            step_id: step_id(&format!("step-{index:02}")),
            objective: format!("do step {index:02}"),
            depends_on: Vec::new(),
            access,
            executor: ExecutorSpec::default(),
            timeout_seconds: 1,
            required: true,
        })
        .collect();
    spec
}

fn dag_spec(name: &str) -> WorkflowSpecV1Beta1 {
    let mut spec = phase6_spec(name, 3, FanInPolicy::AllRequired, 3, AccessMode::Read);
    spec.workflow.steps[0].step_id = step_id("root");
    spec.workflow.steps[0].objective = "root".to_owned();
    spec.workflow.steps[1].step_id = step_id("child");
    spec.workflow.steps[1].objective = "child".to_owned();
    spec.workflow.steps[1].depends_on = vec![step_id("root")];
    spec.workflow.steps[2].step_id = step_id("grandchild");
    spec.workflow.steps[2].objective = "grandchild".to_owned();
    spec.workflow.steps[2].depends_on = vec![step_id("child")];
    spec
}

fn create_draft_workflow(
    store: &mut CoordinationStore,
    workflow: &str,
    request: &str,
    spec: WorkflowSpecV1Beta1,
) -> WorkflowId {
    let id = workflow_id(workflow);
    store
        .create_workflow(CreateWorkflowRequest {
            idempotency_key: request_key("workflow_create", request),
            workflow_id: id.clone(),
            spec,
            correlation_id: format!("corr-{request}"),
            retention_until_ms: None,
        })
        .unwrap();
    id
}

fn create_running_workflow(
    store: &mut CoordinationStore,
    workflow: &str,
    request: &str,
) -> WorkflowId {
    let id = workflow_id(workflow);
    store
        .create_workflow(CreateWorkflowRequest {
            idempotency_key: request_key("workflow_create", request),
            workflow_id: id.clone(),
            spec: valid_spec(workflow, &["step-1", "step-2", "step-3"]),
            correlation_id: format!("corr-{request}"),
            retention_until_ms: None,
        })
        .unwrap();
    store
        .transition_workflow_state(&id, Revision::INITIAL, WorkflowState::Ready, "ready")
        .unwrap();
    store
        .transition_workflow_state(&id, Revision::new(2), WorkflowState::Running, "running")
        .unwrap();
    id
}

fn dispatch_request(
    store: &CoordinationStore,
    workflow_id: &WorkflowId,
    suffix: &str,
    request: &str,
) -> EnqueueWorkerDispatchRequest {
    EnqueueWorkerDispatchRequest {
        idempotency_key: request_key("worker_dispatch", request),
        workflow_id: workflow_id.clone(),
        expected_workflow_revision: store.workflow(workflow_id).unwrap().revision,
        worker_id: worker_id(&format!("worker-{suffix}")),
        step_id: step_id(&format!("step-{suffix}")),
        outbox_id: outbox_id(&format!("outbox-{suffix}")),
        binding_id: binding_id(&format!("binding-{suffix}")),
        objective: format!("inspect {suffix}"),
        access: AccessMode::Read,
        adapter: "paneforge-http-mcp".to_owned(),
        dispatch_marker: format!("marker-{suffix}"),
        prompt_sha256: format!("{:064x}", suffix.len()),
        payload: json!({
            "operation": "launch_task",
            "marker": format!("marker-{suffix}")
        }),
        correlation_id: format!("corr-{request}"),
        not_before_ms: 0,
        retention_until_ms: None,
    }
}

fn worker_for_step(
    store: &CoordinationStore,
    workflow_id: &WorkflowId,
    step: &str,
) -> WorkerRecord {
    let step_id = step_id(step);
    store
        .workers_for_workflow(workflow_id)
        .unwrap()
        .into_iter()
        .find(|worker| worker.step_id == step_id)
        .expect("worker for step")
}

fn due_outbox_for_worker(store: &CoordinationStore, worker_id: &WorkerId) -> OutboxRecord {
    store
        .due_outbox(0, 1_000)
        .unwrap()
        .into_iter()
        .find(|outbox| outbox.worker_id.as_ref() == Some(worker_id))
        .expect("due outbox for worker")
}

fn finish_queued_step(
    runtime: &mut CoordinatorRuntime,
    workflow_id: &WorkflowId,
    step: &str,
    final_state: WorkerState,
    summary: &str,
) -> WorkerId {
    let worker = worker_for_step(runtime.store(), workflow_id, step);
    let outbox = due_outbox_for_worker(runtime.store(), &worker.worker_id);
    let claimed = runtime
        .store_mut()
        .claim_outbox(&outbox.outbox_id, 0)
        .unwrap()
        .unwrap();
    runtime
        .store_mut()
        .transition_worker_state(
            &worker.worker_id,
            Revision::new(2),
            WorkerState::Running,
            "test-running",
        )
        .unwrap();
    runtime
        .store_mut()
        .transition_worker_state(
            &worker.worker_id,
            Revision::new(3),
            final_state,
            "test-terminal",
        )
        .unwrap();
    runtime
        .store_mut()
        .transition_outbox_state(
            &claimed.outbox_id,
            claimed.revision,
            OutboxState::Acknowledged,
            None,
            "test-ack",
        )
        .unwrap();
    runtime
        .store_mut()
        .record_worker_result(RecordWorkerResultRequest {
            workflow_id: workflow_id.clone(),
            worker_id: worker.worker_id.clone(),
            worker_state: final_state,
            aggregation_state: if final_state == WorkerState::Succeeded {
                ResultAggregationState::Complete
            } else {
                ResultAggregationState::Partial
            },
            summary: summary.to_owned(),
            artifact_sha256: Some(format!("{:064x}", summary.len())),
            truncated: false,
            completed_at_ms: 1,
            retention_until_ms: None,
        })
        .unwrap();
    worker.worker_id
}

fn runtime_from_path(
    path: &Path,
    data_dir: &Path,
    config: CoordinatorConfig,
) -> CoordinatorRuntime {
    CoordinatorRuntime::open(path, data_dir, "test-ipc", config).unwrap()
}

#[test]
fn phase6_workflow_start_enqueues_twelve_read_only_steps_and_host_limit_caps_dispatch() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let workflow = {
        let mut store = CoordinationStore::open(&path).unwrap();
        create_draft_workflow(
            &mut store,
            "workflow-twelve",
            "create-twelve",
            phase6_spec(
                "workflow-twelve",
                12,
                FanInPolicy::AllRequired,
                12,
                AccessMode::Read,
            ),
        )
    };
    let config = CoordinatorConfig {
        host_max_active_workers: 4,
        dispatch_inflight: 12,
        max_dispatches_per_workflow_tick: 12,
        ..CoordinatorConfig::default()
    };
    let mut runtime = runtime_from_path(&path, dir.path(), config);
    let advance = runtime
        .start_workflow(&workflow, Revision::INITIAL, 0)
        .unwrap();
    assert_eq!(advance.enqueued_workers.len(), 12);
    assert_eq!(
        runtime
            .store()
            .workflow_worker_counts(&workflow)
            .unwrap()
            .queued_workers,
        12
    );

    runtime.set_dispatch_ready(true);
    let plan = runtime.tick(0).unwrap();
    assert_eq!(plan.dispatches.len(), 4);
    for _ in 0..4 {
        runtime.claim_next_dispatch(0).unwrap().unwrap();
    }
    let counts = runtime.store().workflow_worker_counts(&workflow).unwrap();
    assert_eq!(counts.active_workers, 4);
    assert_eq!(counts.queued_workers, 8);
    assert!(runtime.tick(0).unwrap().dispatches.is_empty());
}

#[test]
fn phase6_dag_dependencies_unlock_after_successful_parent() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let workflow = {
        let mut store = CoordinationStore::open(&path).unwrap();
        create_draft_workflow(
            &mut store,
            "workflow-dag",
            "create-dag",
            dag_spec("workflow-dag"),
        )
    };
    let mut runtime = runtime_from_path(&path, dir.path(), CoordinatorConfig::default());
    let initial = runtime
        .start_workflow(&workflow, Revision::INITIAL, 0)
        .unwrap();
    assert_eq!(initial.enqueued_workers.len(), 1);
    assert_eq!(
        worker_for_step(runtime.store(), &workflow, "root").state,
        WorkerState::Queued
    );
    assert!(runtime
        .store()
        .workers_for_workflow(&workflow)
        .unwrap()
        .iter()
        .all(|worker| worker.step_id != step_id("child")));

    finish_queued_step(
        &mut runtime,
        &workflow,
        "root",
        WorkerState::Succeeded,
        "root-ok",
    );
    let child = runtime.advance_workflow(&workflow, 0).unwrap();
    assert_eq!(child.enqueued_workers.len(), 1);
    assert_eq!(
        worker_for_step(runtime.store(), &workflow, "child").state,
        WorkerState::Queued
    );
    assert!(runtime
        .store()
        .workers_for_workflow(&workflow)
        .unwrap()
        .iter()
        .all(|worker| worker.step_id != step_id("grandchild")));

    finish_queued_step(
        &mut runtime,
        &workflow,
        "child",
        WorkerState::Succeeded,
        "child-ok",
    );
    let grandchild = runtime.advance_workflow(&workflow, 0).unwrap();
    assert_eq!(grandchild.enqueued_workers.len(), 1);
    assert_eq!(
        worker_for_step(runtime.store(), &workflow, "grandchild").state,
        WorkerState::Queued
    );
}

#[test]
fn phase6_allow_partial_policy_preserves_failure_source_and_step_order() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let workflow = {
        let mut store = CoordinationStore::open(&path).unwrap();
        create_draft_workflow(
            &mut store,
            "workflow-partial",
            "create-partial",
            phase6_spec(
                "workflow-partial",
                3,
                FanInPolicy::AllowPartial { min_successful: 2 },
                3,
                AccessMode::Read,
            ),
        )
    };
    let mut runtime = runtime_from_path(&path, dir.path(), CoordinatorConfig::default());
    runtime
        .start_workflow(&workflow, Revision::INITIAL, 0)
        .unwrap();
    finish_queued_step(
        &mut runtime,
        &workflow,
        "step-01",
        WorkerState::Succeeded,
        "one",
    );
    let failed_worker = finish_queued_step(
        &mut runtime,
        &workflow,
        "step-02",
        WorkerState::Failed,
        "two-failed",
    );
    finish_queued_step(
        &mut runtime,
        &workflow,
        "step-03",
        WorkerState::Succeeded,
        "three",
    );

    let advance = runtime.advance_workflow(&workflow, 0).unwrap();
    assert_eq!(advance.workflow_state, WorkflowState::PartialFailed);
    let aggregate = runtime.aggregate_workflow_result(&workflow).unwrap();
    assert_eq!(aggregate.state, ResultAggregationState::Partial);
    assert_eq!(aggregate.failed_workers, vec![failed_worker]);
    let ordered_steps = aggregate
        .workers
        .iter()
        .map(|worker| worker.step_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ordered_steps, vec!["step-01", "step-02", "step-03"]);
}

#[test]
fn phase6_first_success_cancels_remaining_workers_and_completes_policy() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let workflow = {
        let mut store = CoordinationStore::open(&path).unwrap();
        create_draft_workflow(
            &mut store,
            "workflow-first-success",
            "create-first-success",
            phase6_spec(
                "workflow-first-success",
                3,
                FanInPolicy::FirstSuccess,
                3,
                AccessMode::Read,
            ),
        )
    };
    let mut runtime = runtime_from_path(&path, dir.path(), CoordinatorConfig::default());
    runtime
        .start_workflow(&workflow, Revision::INITIAL, 0)
        .unwrap();
    finish_queued_step(
        &mut runtime,
        &workflow,
        "step-01",
        WorkerState::Succeeded,
        "winner",
    );

    let advance = runtime.advance_workflow(&workflow, 0).unwrap();
    assert_eq!(advance.workflow_state, WorkflowState::Succeeded);
    assert_eq!(advance.cancelled_workers.len(), 2);
    assert_eq!(
        worker_for_step(runtime.store(), &workflow, "step-02").state,
        WorkerState::Cancelled
    );
    assert_eq!(
        runtime.aggregate_workflow_result(&workflow).unwrap().state,
        ResultAggregationState::Complete
    );
}

#[test]
fn phase6_workflow_timeout_controls_remaining_workers() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let workflow = {
        let mut store = CoordinationStore::open(&path).unwrap();
        create_draft_workflow(
            &mut store,
            "workflow-timeout",
            "create-timeout",
            phase6_spec(
                "workflow-timeout",
                3,
                FanInPolicy::AllRequired,
                3,
                AccessMode::Read,
            ),
        )
    };
    let mut runtime = runtime_from_path(&path, dir.path(), CoordinatorConfig::default());
    runtime
        .start_workflow(&workflow, Revision::INITIAL, 0)
        .unwrap();
    let first = worker_for_step(runtime.store(), &workflow, "step-01");
    let outbox = due_outbox_for_worker(runtime.store(), &first.worker_id);
    runtime
        .store_mut()
        .claim_outbox(&outbox.outbox_id, 0)
        .unwrap()
        .unwrap();
    runtime
        .store_mut()
        .transition_worker_state(
            &first.worker_id,
            Revision::new(2),
            WorkerState::Running,
            "running-before-timeout",
        )
        .unwrap();

    let advance = runtime.advance_workflow(&workflow, i64::MAX).unwrap();
    assert_eq!(advance.workflow_state, WorkflowState::Failed);
    assert_eq!(advance.timed_out_workers, vec![first.worker_id.clone()]);
    assert_eq!(
        runtime.store().worker(&first.worker_id).unwrap().state,
        WorkerState::Failed
    );
    assert_eq!(
        worker_for_step(runtime.store(), &workflow, "step-02").state,
        WorkerState::Cancelled
    );
}

#[test]
fn phase6_synthetic_pressure_dispatches_twenty_four_of_forty_eight() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let workflow = {
        let mut store = CoordinationStore::open(&path).unwrap();
        create_draft_workflow(
            &mut store,
            "workflow-pressure",
            "create-pressure",
            phase6_spec(
                "workflow-pressure",
                48,
                FanInPolicy::AllRequired,
                48,
                AccessMode::Read,
            ),
        )
    };
    let config = CoordinatorConfig {
        host_max_active_workers: 24,
        dispatch_inflight: 24,
        max_dispatches_per_workflow_tick: 48,
        work_queue_capacity: 64,
        ..CoordinatorConfig::default()
    };
    let mut runtime = runtime_from_path(&path, dir.path(), config);
    runtime
        .start_workflow(&workflow, Revision::INITIAL, 0)
        .unwrap();
    runtime.set_dispatch_ready(true);

    let plan = runtime.tick(0).unwrap();
    assert_eq!(plan.dispatches.len(), 24);
    for _ in 0..24 {
        runtime.claim_next_dispatch(0).unwrap().unwrap();
    }
    let counts = runtime.store().workflow_worker_counts(&workflow).unwrap();
    assert_eq!(counts.active_workers, 24);
    assert_eq!(counts.queued_workers, 24);
}

#[test]
fn phase6_single_worktree_writer_is_mutexed_by_lease() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let workflow = {
        let mut store = CoordinationStore::open(&path).unwrap();
        create_draft_workflow(
            &mut store,
            "workflow-writer",
            "create-writer",
            phase6_spec(
                "workflow-writer",
                2,
                FanInPolicy::AllRequired,
                2,
                AccessMode::Write,
            ),
        )
    };
    let mut runtime = runtime_from_path(&path, dir.path(), CoordinatorConfig::default());
    runtime
        .start_workflow(&workflow, Revision::INITIAL, 0)
        .unwrap();
    runtime.set_dispatch_ready(true);

    let plan = runtime.tick(0).unwrap();
    assert_eq!(plan.dispatches.len(), 1);
    let first_claim = runtime.claim_next_dispatch(0).unwrap().unwrap();
    let first_worker = first_claim.outbox.worker_id.clone().unwrap();
    assert!(runtime.tick(0).unwrap().dispatches.is_empty());

    runtime
        .store_mut()
        .transition_worker_state(
            &first_worker,
            Revision::new(2),
            WorkerState::Running,
            "writer-running",
        )
        .unwrap();
    runtime
        .store_mut()
        .transition_worker_state(
            &first_worker,
            Revision::new(3),
            WorkerState::Succeeded,
            "writer-succeeded",
        )
        .unwrap();

    let second_plan = runtime.tick(0).unwrap();
    assert_eq!(second_plan.dispatches.len(), 1);
    assert_ne!(second_plan.dispatches[0].worker_id, first_worker);
}

#[test]
fn single_instance_lock_uses_runtime_identity_and_releases_on_drop() {
    let dir = tempdir().unwrap();
    let identity = RuntimeIdentity::new(dir.path(), dir.path().join("coordination.db"), "ipc");
    let lock_path = dir.path().join("coordinator.lock");
    let guard = SingleInstanceGuard::acquire(&lock_path, &identity).unwrap();

    let conflict = SingleInstanceGuard::acquire(&lock_path, &identity)
        .expect_err("second same-identity acquire");
    assert_eq!(conflict.code, ErrorCode::CapacityExhausted);
    drop(guard);
    SingleInstanceGuard::acquire(&lock_path, &identity).unwrap();
}

#[test]
fn ipc_health_reports_schema_capacity_and_identity_conflict() {
    let dir = tempdir().unwrap();
    let mut runtime = runtime_from_path(&db_path(&dir), dir.path(), CoordinatorConfig::default());
    let identity = runtime.identity().digest.clone();
    let envelope = IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new("health-1").unwrap(),
        message: IpcMessage::Health(IpcHealthRequest {
            protocol_version: IPC_SCHEMA_VERSION.to_owned(),
            runtime_identity: Some(identity),
        }),
    };
    let response = runtime.handle_ipc(envelope);
    assert!(response.ok);
    assert!(matches!(response.response, Some(IpcResponse::Health(_))));

    let mismatch = IpcEnvelope {
        schema_version: IPC_SCHEMA_VERSION.to_owned(),
        request_id: RequestId::try_new("health-2").unwrap(),
        message: IpcMessage::Health(IpcHealthRequest {
            protocol_version: IPC_SCHEMA_VERSION.to_owned(),
            runtime_identity: Some("other-runtime".to_owned()),
        }),
    };
    let rejected = runtime.handle_ipc(mismatch);
    assert!(!rejected.ok);
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InternalInvariantViolation
    );
}

#[test]
fn idle_tick_uses_bounded_wait_and_no_hot_loop() {
    let dir = tempdir().unwrap();
    let mut runtime = runtime_from_path(&db_path(&dir), dir.path(), CoordinatorConfig::default());
    runtime.set_dispatch_ready(true);
    let plan = runtime.tick(0).unwrap();

    assert!(plan.dispatches.is_empty());
    assert!(plan.next_wait_ms >= 1_000);
    assert_eq!(runtime.metrics().idle_ticks, 1);
}

#[test]
fn dispatch_freeze_keeps_due_outbox_pending() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    {
        let mut store = CoordinationStore::open(&path).unwrap();
        let workflow = create_running_workflow(&mut store, "workflow-freeze", "create-freeze");
        store
            .enqueue_worker_dispatch(dispatch_request(
                &store,
                &workflow,
                "freeze",
                "dispatch-freeze",
            ))
            .unwrap();
    }
    let mut runtime = runtime_from_path(&path, dir.path(), CoordinatorConfig::default());
    runtime.set_dispatch_ready(false);
    let plan = runtime.tick(0).unwrap();

    assert!(plan.frozen);
    assert!(plan.dispatches.is_empty());
    assert_eq!(runtime.store().due_outbox(0, 10).unwrap().len(), 1);
}

#[test]
fn two_workflows_receive_fair_dispatch_slots() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    {
        let mut store = CoordinationStore::open(&path).unwrap();
        let first = create_running_workflow(&mut store, "workflow-a", "create-a");
        store
            .enqueue_worker_dispatch(dispatch_request(&store, &first, "a1", "dispatch-a1"))
            .unwrap();
        store
            .enqueue_worker_dispatch(dispatch_request(&store, &first, "a2", "dispatch-a2"))
            .unwrap();
        let second = create_running_workflow(&mut store, "workflow-b", "create-b");
        store
            .enqueue_worker_dispatch(dispatch_request(&store, &second, "b1", "dispatch-b1"))
            .unwrap();
        store
            .enqueue_worker_dispatch(dispatch_request(&store, &second, "b2", "dispatch-b2"))
            .unwrap();
    }
    let config = CoordinatorConfig {
        dispatch_inflight: 2,
        ..CoordinatorConfig::default()
    };
    let mut runtime = runtime_from_path(&path, dir.path(), config);
    runtime.set_dispatch_ready(true);
    let plan = runtime.tick(0).unwrap();

    assert_eq!(plan.dispatches.len(), 2);
    assert_eq!(plan.dispatches[0].workflow_id, workflow_id("workflow-a"));
    assert_eq!(plan.dispatches[1].workflow_id, workflow_id("workflow-b"));
    let first_claim = runtime.claim_next_dispatch(0).unwrap().unwrap();
    assert_eq!(first_claim.outbox.state, OutboxState::Sending);
    drop(first_claim);
}

#[test]
fn cancel_before_dispatch_finishes_worker_and_cancels_outbox() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let (worker, outbox, binding) = {
        let mut store = CoordinationStore::open(&path).unwrap();
        let workflow = create_running_workflow(&mut store, "workflow-cancel", "create-cancel");
        let dispatch = store
            .enqueue_worker_dispatch(dispatch_request(
                &store,
                &workflow,
                "cancel",
                "dispatch-cancel",
            ))
            .unwrap();
        (
            dispatch.value.worker.worker_id,
            dispatch.value.outbox.outbox_id,
            dispatch.value.binding.binding_id,
        )
    };
    let mut runtime = runtime_from_path(&path, dir.path(), CoordinatorConfig::default());
    let cancelled = runtime
        .request_worker_cancel(&worker, Some(Revision::INITIAL), Some("test cancel"))
        .unwrap();

    assert_eq!(cancelled.state, WorkerState::Cancelled);
    assert_eq!(
        runtime.store().outbox(&outbox).unwrap().state,
        OutboxState::Cancelled
    );
    assert_eq!(
        runtime.store().external_binding(&binding).unwrap().state,
        ExternalBindingState::Cancelled
    );
    assert!(runtime.store().due_outbox(0, 10).unwrap().is_empty());
}

#[test]
fn restart_with_pending_outbox_keeps_dispatch_schedulable() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    {
        let mut store = CoordinationStore::open(&path).unwrap();
        let workflow = create_running_workflow(&mut store, "workflow-restart", "create-restart");
        store
            .enqueue_worker_dispatch(dispatch_request(
                &store,
                &workflow,
                "restart",
                "dispatch-restart",
            ))
            .unwrap();
    }
    let mut runtime = runtime_from_path(&path, dir.path(), CoordinatorConfig::default());
    runtime.set_dispatch_ready(true);
    let plan = runtime.tick(0).unwrap();

    assert_eq!(plan.dispatches.len(), 1);
    assert_eq!(
        runtime
            .store()
            .outbox(&outbox_id("outbox-restart"))
            .unwrap()
            .state,
        OutboxState::Pending
    );
}

#[test]
fn bounded_queue_rejects_over_capacity_work() {
    let mut queue = BoundedQueue::new(1).unwrap();
    queue.try_push("first").unwrap();
    let error = queue.try_push("second").expect_err("queue capacity");
    assert_eq!(error.code, ErrorCode::CapacityExhausted);
    assert_eq!(queue.pop_front(), Some("first"));
}

#[test]
fn crash_backoff_is_bounded_and_increasing() {
    let policy = BackoffPolicy::new(1_000, 60_000);
    let first = policy.crash_delay_ms(1, "runtime");
    let second = policy.crash_delay_ms(2, "runtime");
    let tenth = policy.crash_delay_ms(10, "runtime");

    assert!(first >= 1_000);
    assert!(second > first);
    assert!(tenth <= 60_000);
}

#[test]
fn pause_and_resume_workflow_preserve_revision_contract() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let workflow = {
        let mut store = CoordinationStore::open(&path).unwrap();
        create_running_workflow(&mut store, "workflow-pause", "create-pause")
    };
    let mut runtime = runtime_from_path(&path, dir.path(), CoordinatorConfig::default());
    let paused = runtime.pause_workflow(&workflow, Revision::new(3)).unwrap();
    assert_eq!(paused.state, WorkflowState::Paused);
    let resumed = runtime.resume_workflow(&workflow, paused.revision).unwrap();
    assert_eq!(resumed.state, WorkflowState::Ready);
}

#[test]
fn result_aggregator_preserves_missing_failed_worker_source() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let workflow = {
        let mut store = CoordinationStore::open(&path).unwrap();
        let workflow =
            create_running_workflow(&mut store, "workflow-aggregate", "create-aggregate");
        let success = store
            .enqueue_worker_dispatch(dispatch_request(&store, &workflow, "agg1", "dispatch-agg1"))
            .unwrap();
        let failure = store
            .enqueue_worker_dispatch(dispatch_request(&store, &workflow, "agg2", "dispatch-agg2"))
            .unwrap();
        let success_outbox = store
            .claim_outbox(&success.value.outbox.outbox_id, 0)
            .unwrap()
            .unwrap();
        let failure_outbox = store
            .claim_outbox(&failure.value.outbox.outbox_id, 0)
            .unwrap()
            .unwrap();
        store
            .transition_worker_state(
                &success.value.worker.worker_id,
                Revision::new(2),
                WorkerState::Running,
                "success-running",
            )
            .unwrap();
        store
            .transition_worker_state(
                &success.value.worker.worker_id,
                Revision::new(3),
                WorkerState::Succeeded,
                "success-done",
            )
            .unwrap();
        store
            .transition_worker_state(
                &failure.value.worker.worker_id,
                Revision::new(2),
                WorkerState::Running,
                "failure-running",
            )
            .unwrap();
        store
            .transition_worker_state(
                &failure.value.worker.worker_id,
                Revision::new(3),
                WorkerState::Failed,
                "failure-done",
            )
            .unwrap();
        store
            .transition_outbox_state(
                &success.value.outbox.outbox_id,
                success_outbox.revision,
                OutboxState::Acknowledged,
                None,
                "success-ack",
            )
            .unwrap();
        store
            .transition_outbox_state(
                &failure.value.outbox.outbox_id,
                failure_outbox.revision,
                OutboxState::ResultUnknown,
                Some("failed before result artifact"),
                "failure-unknown",
            )
            .unwrap();
        store
            .record_worker_result(RecordWorkerResultRequest {
                workflow_id: workflow.clone(),
                worker_id: success.value.worker.worker_id,
                worker_state: WorkerState::Succeeded,
                aggregation_state: ResultAggregationState::Complete,
                summary: "ok".to_owned(),
                artifact_sha256: Some("digest".to_owned()),
                truncated: false,
                completed_at_ms: 1,
                retention_until_ms: None,
            })
            .unwrap();
        workflow
    };
    let mut runtime = runtime_from_path(&path, dir.path(), CoordinatorConfig::default());
    let aggregate = runtime.aggregate_workflow_result(&workflow).unwrap();

    assert_eq!(aggregate.state, ResultAggregationState::Partial);
    assert_eq!(aggregate.workers.len(), 1);
    assert_eq!(aggregate.missing_results, vec![worker_id("worker-agg2")]);
    assert_eq!(aggregate.failed_workers, vec![worker_id("worker-agg2")]);
}
