use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use coordination_core::{
    AccessMode, BudgetSpec, CallerId, DirtyPolicy, ErrorCode, ExecutorSpec, ExternalBindingId,
    FanInPolicy, IdempotencyKey, LeaseId, OutboxId, OutboxState, RepositoryRef, RequestId,
    ResultAggregationState, Revision, SchedulingSpec, StepId, StepSpec, WorkerId, WorkerState,
    WorkflowDefinition, WorkflowId, WorkflowMetadata, WorkflowSpecV1Beta1, WorkflowState,
    WORKFLOW_SPEC_KIND, WORKFLOW_SPEC_SCHEMA_VERSION,
};
use coordination_store::{
    AcquireLeaseRequest, CoordinationStore, EnqueueWorkerDispatchRequest, ExternalBindingState,
    LeaseMode, RecordExternalCancelSuccessRequest, RecordExternalLaunchSuccessRequest,
    RecordWorkerResultRequest, RequestExternalWorkerCancelRequest, StoreOptions,
    UpsertRetentionRequest, STORE_SCHEMA_VERSION,
};
use rusqlite::Connection;
use serde_json::json;
use tempfile::tempdir;

fn db_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("coordination.db")
}

fn id(value: &str) -> WorkflowId {
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

fn valid_spec(name: &str, step: &str) -> WorkflowSpecV1Beta1 {
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
            steps: vec![StepSpec {
                step_id: step_id(step),
                objective: format!("do {step}"),
                depends_on: Vec::new(),
                access: AccessMode::Read,
                executor: ExecutorSpec::default(),
                timeout_seconds: 120,
                required: true,
            }],
        },
    }
}

fn create_workflow(store: &mut CoordinationStore, workflow: &str, request: &str) -> WorkflowId {
    let workflow_id = id(workflow);
    let created = store
        .create_workflow(coordination_store::CreateWorkflowRequest {
            idempotency_key: request_key("workflow_create", request),
            workflow_id: workflow_id.clone(),
            spec: valid_spec(workflow, "step-1"),
            correlation_id: format!("corr-{request}"),
            retention_until_ms: Some(2_000_000),
        })
        .unwrap();
    assert!(!created.replayed);
    assert_eq!(created.value.state, WorkflowState::Draft);
    workflow_id
}

fn dispatch_request(
    workflow_id: &WorkflowId,
    expected_workflow_revision: Revision,
    suffix: &str,
    request: &str,
    outbox: &str,
) -> EnqueueWorkerDispatchRequest {
    EnqueueWorkerDispatchRequest {
        idempotency_key: request_key("worker_dispatch", request),
        workflow_id: workflow_id.clone(),
        expected_workflow_revision,
        worker_id: worker_id(&format!("worker-{suffix}")),
        step_id: step_id(&format!("step-{suffix}")),
        outbox_id: outbox_id(outbox),
        binding_id: binding_id(&format!("binding-{suffix}")),
        objective: format!("inspect {suffix}"),
        access: AccessMode::Read,
        adapter: "cc-panes-http-mcp".to_owned(),
        dispatch_marker: format!("marker-{suffix}"),
        prompt_sha256: format!("{:064x}", suffix.len()),
        payload: json!({
            "operation": "launch_task",
            "marker": format!("marker-{suffix}")
        }),
        correlation_id: format!("corr-{request}"),
        not_before_ms: 0,
        retention_until_ms: Some(2_000_000),
    }
}

fn open_store(path: &Path) -> CoordinationStore {
    CoordinationStore::open(path).unwrap()
}

#[test]
fn fresh_migration_creates_schema_v1_with_wal_and_integrity() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let store = open_store(&path);

    assert_eq!(store.schema_version().unwrap(), STORE_SCHEMA_VERSION);
    let migrations = store.migrations().unwrap();
    assert_eq!(migrations.len(), 1);
    assert_eq!(migrations[0].version, STORE_SCHEMA_VERSION);
    assert_eq!(store.integrity_check().unwrap().sqlite_result, "ok");

    let connection = Connection::open(&path).unwrap();
    let journal_mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
}

#[test]
fn pre_v1_upgrade_creates_backup_before_migration() {
    let dir = tempdir().unwrap();
    let backup_dir = dir.path().join("backups");
    let path = db_path(&dir);
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("CREATE TABLE legacy_marker (value TEXT NOT NULL)", [])
            .unwrap();
        connection
            .execute("INSERT INTO legacy_marker (value) VALUES ('before')", [])
            .unwrap();
    }

    let store = CoordinationStore::open_with_options(
        &path,
        StoreOptions {
            migration_backup_dir: Some(backup_dir.clone()),
        },
    )
    .unwrap();

    assert_eq!(store.schema_version().unwrap(), STORE_SCHEMA_VERSION);
    let backups = fs::read_dir(&backup_dir)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(backups.len(), 1);
    let backup_connection = Connection::open(backups[0].path()).unwrap();
    let legacy_value: String = backup_connection
        .query_row("SELECT value FROM legacy_marker", [], |row| row.get(0))
        .unwrap();
    assert_eq!(legacy_value, "before");
}

#[test]
fn corrupt_database_returns_persistence_unavailable() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    fs::write(&path, b"not a sqlite database").unwrap();

    let error = CoordinationStore::open(&path).expect_err("corrupt db rejected");
    assert_eq!(error.code, ErrorCode::PersistenceUnavailable);
}

#[test]
fn duplicate_workflow_create_replays_original_record() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let mut store = open_store(&path);
    let key = request_key("workflow_create", "same-request");
    let first = store
        .create_workflow(coordination_store::CreateWorkflowRequest {
            idempotency_key: key.clone(),
            workflow_id: id("workflow-original"),
            spec: valid_spec("original", "step-1"),
            correlation_id: "corr".to_owned(),
            retention_until_ms: None,
        })
        .unwrap();
    let replay = store
        .create_workflow(coordination_store::CreateWorkflowRequest {
            idempotency_key: key,
            workflow_id: id("workflow-new-id-ignored"),
            spec: valid_spec("different", "step-1"),
            correlation_id: "corr-replay".to_owned(),
            retention_until_ms: None,
        })
        .unwrap();

    assert!(!first.replayed);
    assert!(replay.replayed);
    assert_eq!(replay.value.workflow_id, id("workflow-original"));
}

#[test]
fn duplicate_worker_dispatch_replays_one_worker_binding_and_outbox() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let mut store = open_store(&path);
    let workflow_id = create_workflow(&mut store, "workflow-dispatch", "create-dispatch");
    let key = request_key("worker_dispatch", "dispatch-once");
    let mut first_request = dispatch_request(
        &workflow_id,
        Revision::INITIAL,
        "one",
        "dispatch-once",
        "outbox-one",
    );
    first_request.idempotency_key = key.clone();
    let first = store.enqueue_worker_dispatch(first_request).unwrap();

    let mut replay_request = dispatch_request(
        &workflow_id,
        Revision::new(999),
        "two",
        "dispatch-once",
        "outbox-two",
    );
    replay_request.idempotency_key = key;
    let replay = store.enqueue_worker_dispatch(replay_request).unwrap();

    assert!(!first.replayed);
    assert!(replay.replayed);
    assert_eq!(replay.value.worker.worker_id, worker_id("worker-one"));
    assert_eq!(store.due_outbox(0, 10).unwrap().len(), 1);
}

#[test]
fn stale_workflow_revision_returns_typed_conflict() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let mut store = open_store(&path);
    let workflow_id = create_workflow(&mut store, "workflow-stale", "create-stale");
    let ready = store
        .transition_workflow_state(
            &workflow_id,
            Revision::INITIAL,
            WorkflowState::Ready,
            "start",
        )
        .unwrap();
    assert_eq!(ready.revision, Revision::new(2));

    let error = store
        .transition_workflow_state(
            &workflow_id,
            Revision::INITIAL,
            WorkflowState::Running,
            "stale",
        )
        .expect_err("stale revision");
    assert_eq!(error.code, ErrorCode::RevisionConflict);
    assert_eq!(error.current_revision, Some(Revision::new(2)));
}

#[test]
fn worker_and_outbox_repositories_enforce_state_machines() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let mut store = open_store(&path);
    let workflow_id = create_workflow(&mut store, "workflow-state", "create-state");
    let dispatch = store
        .enqueue_worker_dispatch(dispatch_request(
            &workflow_id,
            Revision::INITIAL,
            "state",
            "dispatch-state",
            "outbox-state",
        ))
        .unwrap();

    let claimed = store
        .claim_outbox(&dispatch.value.outbox.outbox_id, 0)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.state, OutboxState::Sending);
    assert_eq!(
        store
            .worker(&dispatch.value.worker.worker_id)
            .unwrap()
            .state,
        WorkerState::Dispatching
    );

    let illegal = store
        .transition_worker_state(
            &dispatch.value.worker.worker_id,
            Revision::new(2),
            WorkerState::Succeeded,
            "illegal",
        )
        .expect_err("dispatching cannot directly succeed");
    assert_eq!(illegal.code, ErrorCode::InvalidTransition);

    let unknown = store
        .transition_outbox_state(
            &dispatch.value.outbox.outbox_id,
            claimed.revision,
            OutboxState::ResultUnknown,
            Some("timeout"),
            "unknown",
        )
        .unwrap();
    assert_eq!(unknown.state, OutboxState::ResultUnknown);
}

#[test]
fn external_launch_success_records_session_ref_once_and_rejects_conflicts() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let mut store = open_store(&path);
    let workflow_id = create_workflow(&mut store, "workflow-binding", "create-binding");
    let first = store
        .enqueue_worker_dispatch(dispatch_request(
            &workflow_id,
            Revision::INITIAL,
            "binding-one",
            "dispatch-binding-one",
            "outbox-binding-one",
        ))
        .unwrap();
    let claimed_first = store
        .claim_outbox(&first.value.outbox.outbox_id, 0)
        .unwrap()
        .unwrap();

    let recorded = store
        .record_external_launch_success(RecordExternalLaunchSuccessRequest {
            binding_id: first.value.binding.binding_id.clone(),
            expected_generation: claimed_first.generation,
            external_task_ref: Some("task-1".to_owned()),
            external_session_ref: "session-1".to_owned(),
            correlation_id: "launch-success".to_owned(),
        })
        .unwrap();

    assert_eq!(recorded.state, ExternalBindingState::Active);
    assert_eq!(recorded.external_task_ref.as_deref(), Some("task-1"));
    assert_eq!(recorded.external_session_ref.as_deref(), Some("session-1"));
    assert_eq!(recorded.generation, claimed_first.generation);
    assert!(recorded.last_reconciled_at_ms.is_some());

    let replay = store
        .record_external_launch_success(RecordExternalLaunchSuccessRequest {
            binding_id: first.value.binding.binding_id.clone(),
            expected_generation: claimed_first.generation,
            external_task_ref: Some("task-1".to_owned()),
            external_session_ref: "session-1".to_owned(),
            correlation_id: "launch-success-replay".to_owned(),
        })
        .unwrap();
    assert_eq!(replay, recorded);

    let stale = store
        .record_external_launch_success(RecordExternalLaunchSuccessRequest {
            binding_id: first.value.binding.binding_id.clone(),
            expected_generation: claimed_first.generation.saturating_add(1),
            external_task_ref: Some("task-1".to_owned()),
            external_session_ref: "session-1".to_owned(),
            correlation_id: "launch-success-stale".to_owned(),
        })
        .expect_err("generation mismatch");
    assert_eq!(stale.code, ErrorCode::ExternalBindingConflict);

    let second = store
        .enqueue_worker_dispatch(dispatch_request(
            &workflow_id,
            store.workflow(&workflow_id).unwrap().revision,
            "binding-two",
            "dispatch-binding-two",
            "outbox-binding-two",
        ))
        .unwrap();
    let claimed_second = store
        .claim_outbox(&second.value.outbox.outbox_id, 0)
        .unwrap()
        .unwrap();
    let duplicate_session = store
        .record_external_launch_success(RecordExternalLaunchSuccessRequest {
            binding_id: second.value.binding.binding_id,
            expected_generation: claimed_second.generation,
            external_task_ref: Some("task-2".to_owned()),
            external_session_ref: "session-1".to_owned(),
            correlation_id: "launch-success-duplicate-session".to_owned(),
        })
        .expect_err("session ref must be unique");
    assert_eq!(duplicate_session.code, ErrorCode::ExternalBindingConflict);
}

#[test]
fn external_worker_cancel_requests_one_kill_session_outbox_and_records_completion() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let mut store = open_store(&path);
    let workflow_id = create_workflow(&mut store, "workflow-active-cancel", "create-active-cancel");
    let dispatch = store
        .enqueue_worker_dispatch(dispatch_request(
            &workflow_id,
            Revision::INITIAL,
            "active-cancel",
            "dispatch-active-cancel",
            "outbox-active-cancel",
        ))
        .unwrap();
    let claimed_launch = store
        .claim_outbox(&dispatch.value.outbox.outbox_id, 0)
        .unwrap()
        .unwrap();
    let binding = store
        .record_external_launch_success(RecordExternalLaunchSuccessRequest {
            binding_id: dispatch.value.binding.binding_id.clone(),
            expected_generation: claimed_launch.generation,
            external_task_ref: Some("external-job-active-cancel".to_owned()),
            external_session_ref: "session-active-cancel".to_owned(),
            correlation_id: "launch-active-cancel".to_owned(),
        })
        .unwrap();
    store
        .transition_worker_state(
            &dispatch.value.worker.worker_id,
            Revision::new(2),
            WorkerState::Running,
            "worker-running",
        )
        .unwrap();

    let requested = store
        .request_external_worker_cancel(RequestExternalWorkerCancelRequest {
            worker_id: dispatch.value.worker.worker_id.clone(),
            expected_worker_revision: Some(Revision::new(3)),
            outbox_id: outbox_id("outbox-kill-active-cancel"),
            reason: Some("synthetic active cancel".to_owned()),
            correlation_id: "request-active-cancel".to_owned(),
            not_before_ms: 0,
        })
        .unwrap();
    assert!(!requested.replayed);
    assert_eq!(requested.worker.state, WorkerState::Cancelling);
    assert_eq!(
        requested.binding.state,
        ExternalBindingState::CancelRequested
    );
    assert_eq!(requested.outbox.command_type, "kill_session");
    assert_eq!(
        requested.outbox.payload["sessionId"],
        "session-active-cancel"
    );
    assert_eq!(
        store
            .due_outbox(0, 10)
            .unwrap()
            .iter()
            .filter(|outbox| outbox.command_type == "kill_session")
            .count(),
        1
    );

    let replay = store
        .request_external_worker_cancel(RequestExternalWorkerCancelRequest {
            worker_id: dispatch.value.worker.worker_id.clone(),
            expected_worker_revision: Some(Revision::new(3)),
            outbox_id: outbox_id("outbox-kill-active-cancel"),
            reason: Some("synthetic active cancel replay".to_owned()),
            correlation_id: "request-active-cancel-replay".to_owned(),
            not_before_ms: 0,
        })
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.outbox.outbox_id, requested.outbox.outbox_id);
    assert_eq!(
        store
            .due_outbox(0, 10)
            .unwrap()
            .iter()
            .filter(|outbox| outbox.command_type == "kill_session")
            .count(),
        1
    );

    let claimed_cancel = store
        .claim_outbox(&requested.outbox.outbox_id, 0)
        .unwrap()
        .unwrap();
    store
        .transition_outbox_state(
            &claimed_cancel.outbox_id,
            claimed_cancel.revision,
            OutboxState::Acknowledged,
            None,
            "kill-ack",
        )
        .unwrap();
    let cancelled_binding = store
        .record_external_cancel_success(RecordExternalCancelSuccessRequest {
            binding_id: binding.binding_id,
            correlation_id: "kill-success".to_owned(),
        })
        .unwrap();
    assert_eq!(cancelled_binding.state, ExternalBindingState::Cancelled);
}

#[test]
fn failed_transaction_rolls_back_worker_binding_and_outbox() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let mut store = open_store(&path);
    let first_workflow = create_workflow(&mut store, "workflow-rollback-1", "create-rb1");
    store
        .enqueue_worker_dispatch(dispatch_request(
            &first_workflow,
            Revision::INITIAL,
            "rb1",
            "dispatch-rb1",
            "outbox-conflict",
        ))
        .unwrap();

    let second_workflow = create_workflow(&mut store, "workflow-rollback-2", "create-rb2");
    let error = store
        .enqueue_worker_dispatch(dispatch_request(
            &second_workflow,
            Revision::INITIAL,
            "rb2",
            "dispatch-rb2",
            "outbox-conflict",
        ))
        .expect_err("duplicate outbox id fails");
    assert_eq!(error.code, ErrorCode::PersistenceUnavailable);
    assert_eq!(
        store.workflow(&second_workflow).unwrap().revision,
        Revision::INITIAL
    );
    assert_eq!(
        store
            .worker(&worker_id("worker-rb2"))
            .expect_err("worker insert rolled back")
            .code,
        ErrorCode::WorkerNotFound
    );
}

#[test]
fn pending_outbox_survives_wal_restart_and_claims_once() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let outbox = {
        let mut store = open_store(&path);
        let workflow_id = create_workflow(&mut store, "workflow-wal", "create-wal");
        let dispatch = store
            .enqueue_worker_dispatch(dispatch_request(
                &workflow_id,
                Revision::INITIAL,
                "wal",
                "dispatch-wal",
                "outbox-wal",
            ))
            .unwrap();
        dispatch.value.outbox.outbox_id
    };

    let mut reopened = open_store(&path);
    assert_eq!(reopened.due_outbox(0, 10).unwrap().len(), 1);
    let claimed = reopened.claim_outbox(&outbox, 0).unwrap().unwrap();
    assert_eq!(claimed.state, OutboxState::Sending);
    assert!(reopened.claim_outbox(&outbox, 0).unwrap().is_none());
    assert_eq!(reopened.integrity_check().unwrap().sqlite_result, "ok");
}

#[test]
fn sending_outbox_recovers_as_result_unknown_after_restart() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let (outbox, worker, binding) = {
        let mut store = open_store(&path);
        let workflow_id = create_workflow(&mut store, "workflow-recover", "create-recover");
        let dispatch = store
            .enqueue_worker_dispatch(dispatch_request(
                &workflow_id,
                Revision::INITIAL,
                "recover",
                "dispatch-recover",
                "outbox-recover",
            ))
            .unwrap();
        store
            .claim_outbox(&dispatch.value.outbox.outbox_id, 0)
            .unwrap()
            .unwrap();
        (
            dispatch.value.outbox.outbox_id,
            dispatch.value.worker.worker_id,
            dispatch.value.binding.binding_id,
        )
    };

    let reopened = open_store(&path);
    assert_eq!(
        reopened.outbox(&outbox).unwrap().state,
        OutboxState::ResultUnknown
    );
    assert_eq!(
        reopened.worker(&worker).unwrap().state,
        WorkerState::DispatchUnknown
    );
    assert_eq!(
        reopened.external_binding(&binding).unwrap().state,
        ExternalBindingState::ResultUnknown
    );
}

#[test]
fn recovered_unknown_binding_records_reconciled_launch_once_with_current_generation() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let (outbox, worker, binding) = {
        let mut store = open_store(&path);
        let workflow_id = create_workflow(
            &mut store,
            "workflow-recovered-launch",
            "create-recovered-launch",
        );
        let dispatch = store
            .enqueue_worker_dispatch(dispatch_request(
                &workflow_id,
                Revision::INITIAL,
                "recovered-launch",
                "dispatch-recovered-launch",
                "outbox-recovered-launch",
            ))
            .unwrap();
        store
            .claim_outbox(&dispatch.value.outbox.outbox_id, 0)
            .unwrap()
            .unwrap();
        (
            dispatch.value.outbox.outbox_id,
            dispatch.value.worker.worker_id,
            dispatch.value.binding.binding_id,
        )
    };

    let mut reopened = open_store(&path);
    let recovered = reopened.external_binding(&binding).unwrap();
    assert_eq!(recovered.state, ExternalBindingState::ResultUnknown);
    assert_eq!(recovered.generation, 2);

    let stale = reopened
        .record_external_launch_success(RecordExternalLaunchSuccessRequest {
            binding_id: binding.clone(),
            expected_generation: 1,
            external_task_ref: Some("task-recovered".to_owned()),
            external_session_ref: "session-recovered".to_owned(),
            correlation_id: "recovered-launch-stale".to_owned(),
        })
        .expect_err("recovered generation must be honored");
    assert_eq!(stale.code, ErrorCode::ExternalBindingConflict);

    let recorded = reopened
        .record_external_launch_success(RecordExternalLaunchSuccessRequest {
            binding_id: binding.clone(),
            expected_generation: recovered.generation,
            external_task_ref: Some("task-recovered".to_owned()),
            external_session_ref: "session-recovered".to_owned(),
            correlation_id: "recovered-launch-record".to_owned(),
        })
        .unwrap();

    assert_eq!(recorded.state, ExternalBindingState::Active);
    assert_eq!(
        recorded.external_session_ref.as_deref(),
        Some("session-recovered")
    );
    assert_eq!(
        reopened.outbox(&outbox).unwrap().state,
        OutboxState::ResultUnknown
    );
    assert_eq!(
        reopened.worker(&worker).unwrap().state,
        WorkerState::DispatchUnknown
    );
}

#[test]
fn lease_repository_prevents_writer_conflicts_and_allows_release() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let mut store = open_store(&path);
    let workflow_id = create_workflow(&mut store, "workflow-lease", "create-lease");
    let dispatch = store
        .enqueue_worker_dispatch(dispatch_request(
            &workflow_id,
            Revision::INITIAL,
            "lease",
            "dispatch-lease",
            "outbox-lease",
        ))
        .unwrap();

    let lease = store
        .acquire_lease(AcquireLeaseRequest {
            lease_id: LeaseId::try_new("lease-1").unwrap(),
            canonical_resource: "D:/SYNTHETIC".to_owned(),
            resource_sha256: "digest".to_owned(),
            mode: LeaseMode::ExclusiveWrite,
            owner_workflow_id: workflow_id.clone(),
            owner_worker_id: Some(dispatch.value.worker.worker_id.clone()),
            ttl_ms: 60_000,
        })
        .unwrap();
    assert_eq!(lease.mode, LeaseMode::ExclusiveWrite);

    let conflict = store
        .acquire_lease(AcquireLeaseRequest {
            lease_id: LeaseId::try_new("lease-2").unwrap(),
            canonical_resource: "D:/SYNTHETIC".to_owned(),
            resource_sha256: "digest".to_owned(),
            mode: LeaseMode::SharedRead,
            owner_workflow_id: workflow_id.clone(),
            owner_worker_id: None,
            ttl_ms: 60_000,
        })
        .expect_err("exclusive writer blocks readers");
    assert_eq!(conflict.code, ErrorCode::ExternalBindingConflict);

    let released = store
        .release_lease(&LeaseId::try_new("lease-1").unwrap(), "done")
        .unwrap();
    assert_eq!(released.release_reason.as_deref(), Some("done"));
    let read_lease = store
        .acquire_lease(AcquireLeaseRequest {
            lease_id: LeaseId::try_new("lease-3").unwrap(),
            canonical_resource: "D:/SYNTHETIC".to_owned(),
            resource_sha256: "digest".to_owned(),
            mode: LeaseMode::SharedRead,
            owner_workflow_id: workflow_id,
            owner_worker_id: None,
            ttl_ms: 60_000,
        })
        .unwrap();
    assert_eq!(read_lease.mode, LeaseMode::SharedRead);
}

#[test]
fn result_and_retention_repositories_roundtrip_metadata() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let mut store = open_store(&path);
    let workflow_id = create_workflow(&mut store, "workflow-result", "create-result");
    let dispatch = store
        .enqueue_worker_dispatch(dispatch_request(
            &workflow_id,
            Revision::INITIAL,
            "result",
            "dispatch-result",
            "outbox-result",
        ))
        .unwrap();

    let result = store
        .record_worker_result(RecordWorkerResultRequest {
            workflow_id: workflow_id.clone(),
            worker_id: dispatch.value.worker.worker_id.clone(),
            worker_state: WorkerState::Succeeded,
            aggregation_state: ResultAggregationState::Complete,
            summary: "summary".to_owned(),
            artifact_sha256: Some("artifact-digest".to_owned()),
            truncated: false,
            completed_at_ms: 1_234,
            retention_until_ms: Some(9_999),
        })
        .unwrap();
    assert_eq!(result.summary, "summary");
    assert_eq!(store.workflow_results(&workflow_id).unwrap().len(), 1);

    let retention = store
        .upsert_retention(UpsertRetentionRequest {
            entity_kind: "artifact".to_owned(),
            entity_id: "artifact-1".to_owned(),
            retain_until_ms: Some(9_999),
            max_bytes: Some(1024),
            reason: "artifact retention".to_owned(),
        })
        .unwrap();
    assert_eq!(retention.max_bytes, Some(1024));
    assert_eq!(
        store
            .retention("artifact", "artifact-1")
            .unwrap()
            .unwrap()
            .retain_until_ms,
        Some(9_999)
    );
}

#[test]
fn events_are_incremental_and_integrity_checked() {
    let dir = tempdir().unwrap();
    let path = db_path(&dir);
    let mut store = open_store(&path);
    let workflow_id = create_workflow(&mut store, "workflow-events", "create-events");
    store
        .transition_workflow_state(
            &workflow_id,
            Revision::INITIAL,
            WorkflowState::Ready,
            "ready",
        )
        .unwrap();

    let events = store.events(&workflow_id, 0, 10).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].sequence, 1);
    assert_eq!(events[1].sequence, 2);
    assert_eq!(store.events(&workflow_id, 1, 10).unwrap().len(), 1);
    assert!(store.integrity_check().unwrap().event_chains_verified);
}
