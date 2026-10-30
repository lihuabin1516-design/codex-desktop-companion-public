use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use coordination_core::{
    require_expected_revision, AccessMode, CoordinationError, CoordinationEventKind, ErrorCode,
    EventId, ExternalBindingId, LeaseId, OutboxId, OutboxState, ResultAggregationState, Revision,
    StepId, WorkerId, WorkerState, WorkflowId, WorkflowSpecV1Beta1, WorkflowState,
    COORDINATION_EVENT_SCHEMA_VERSION,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::error::{
    binding_conflict, invariant_error, map_io, map_json, map_sql, persistence_error,
    revision_from_i64, to_i64, SqlResultExt, StoreResult,
};
use crate::model::{
    AcquireLeaseRequest, CreateWorkflowRequest, EnqueueWorkerDispatchRequest, EventRecord,
    ExternalBindingRecord, ExternalBindingState, Idempotent, IntegrityReport, LeaseMode,
    LeaseRecord, MigrationRecord, OutboxRecord, RecordExternalCancelSuccessRequest,
    RecordExternalLaunchSuccessRequest, RecordWorkerResultRequest,
    RequestExternalWorkerCancelRequest, ResultRecord, RetentionRecord, StoreCapacityCounts,
    StoreOptions, UpsertRetentionRequest, WorkerCancelRecord, WorkerDispatchRecord,
    WorkerDispatchReplay, WorkerRecord, WorkflowRecord, WorkflowReplay, WorkflowWorkerCounts,
};
use crate::schema::{backup_connection, configure_connection, migrate};

const DEFAULT_IDEMPOTENCY_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
const RECOVERY_ERROR: &str = "coordinator restarted while outbox item was SENDING";

#[derive(Debug)]
pub struct CoordinationStore {
    path: PathBuf,
    connection: Connection,
}

impl CoordinationStore {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        Self::open_with_options(path, StoreOptions::default())
    }

    pub fn open_with_options(path: impl AsRef<Path>, options: StoreOptions) -> StoreResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(map_io)?;
        }
        let mut connection = Connection::open(&path).map_err(map_sql)?;
        configure_connection(&connection)?;
        migrate(
            &mut connection,
            &path,
            options.migration_backup_dir.as_deref(),
        )?;
        let mut store = Self { path, connection };
        store.recover_interrupted_outbox()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn schema_version(&self) -> StoreResult<i32> {
        self.connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .store_result()
    }

    pub fn migrations(&self) -> StoreResult<Vec<MigrationRecord>> {
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT version, description, applied_at_ms
                FROM coordination_schema_migrations
                ORDER BY version
                "#,
            )
            .store_result()?;
        let migrations = statement
            .query_map([], |row| {
                Ok(MigrationRecord {
                    version: row.get(0)?,
                    description: row.get(1)?,
                    applied_at_ms: row.get(2)?,
                })
            })
            .store_result()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .store_result()?;
        Ok(migrations)
    }

    pub fn backup(&self, destination: impl AsRef<Path>) -> StoreResult<()> {
        backup_connection(&self.connection, destination.as_ref())
    }

    pub fn integrity_check(&self) -> StoreResult<IntegrityReport> {
        let sqlite_result: String = self
            .connection
            .pragma_query_value(None, "integrity_check", |row| row.get(0))
            .store_result()?;
        if sqlite_result != "ok" {
            return Err(persistence_error(format!(
                "SQLite integrity check returned {sqlite_result}"
            )));
        }
        self.verify_event_chains()?;
        Ok(IntegrityReport {
            sqlite_result,
            event_chains_verified: true,
        })
    }

    pub fn create_workflow(
        &mut self,
        request: CreateWorkflowRequest,
    ) -> StoreResult<Idempotent<WorkflowRecord>> {
        request.spec.require_supported_schema()?;
        request
            .spec
            .validate()
            .map_err(|error| CoordinationError::invalid_request(error.to_string()))?;

        let transaction = self.immediate_transaction()?;
        let scope = idempotency_scope(&request.idempotency_key)?;
        if let Some(response_json) = load_idempotency_response(&transaction, &scope)? {
            let replay: WorkflowReplay = serde_json::from_str(&response_json).map_err(map_json)?;
            let workflow = workflow_by_id_tx(&transaction, &replay.workflow_id)?;
            transaction.commit().store_result()?;
            return Ok(Idempotent::replayed(workflow));
        }

        let now = now_ms();
        let spec_json = serde_json::to_string(&request.spec).map_err(map_json)?;
        transaction
            .execute(
                r#"
                INSERT INTO coordination_workflows (
                    workflow_id, state, revision, spec_schema_version, spec_json,
                    caller_id, created_at_ms, updated_at_ms, retention_until_ms
                )
                VALUES (?1, 'DRAFT', ?2, ?3, ?4, ?5, ?6, ?6, ?7)
                "#,
                params![
                    request.workflow_id.as_str(),
                    to_i64(Revision::INITIAL.get(), "workflow revision")?,
                    request.spec.schema_version,
                    spec_json,
                    request.spec.metadata.caller_id,
                    now,
                    request.retention_until_ms,
                ],
            )
            .store_result()?;
        append_event_tx(
            &transaction,
            &request.workflow_id,
            Revision::INITIAL,
            CoordinationEventKind::WorkflowCreated,
            &request.correlation_id,
            now,
        )?;
        if let Some(retain_until_ms) = request.retention_until_ms {
            upsert_retention_tx(
                &transaction,
                "workflow",
                request.workflow_id.as_str(),
                Some(retain_until_ms),
                None,
                "workflow retention",
                now,
            )?;
        }
        let replay = WorkflowReplay {
            workflow_id: request.workflow_id.clone(),
        };
        insert_idempotency_response(&transaction, &request.idempotency_key, &scope, &replay, now)?;
        let workflow = workflow_by_id_tx(&transaction, &request.workflow_id)?;
        transaction.commit().store_result()?;
        Ok(Idempotent::created(workflow))
    }

    pub fn workflow(&self, workflow_id: &WorkflowId) -> StoreResult<WorkflowRecord> {
        workflow_by_id(&self.connection, workflow_id)
    }

    pub fn worker(&self, worker_id: &WorkerId) -> StoreResult<WorkerRecord> {
        worker_by_id(&self.connection, worker_id)
    }

    pub fn workers_for_workflow(&self, workflow_id: &WorkflowId) -> StoreResult<Vec<WorkerRecord>> {
        workers_for_workflow(&self.connection, workflow_id)
    }

    pub fn capacity_counts(&self) -> StoreResult<StoreCapacityCounts> {
        let active_workers = count_workers_by_states(
            &self.connection,
            &[
                WorkerState::Dispatching,
                WorkerState::DispatchUnknown,
                WorkerState::Running,
                WorkerState::Cancelling,
            ],
        )?;
        let queued_workers = count_workers_by_states(&self.connection, &[WorkerState::Queued])?;
        let running_workflows = count_workflows_by_states(
            &self.connection,
            &[
                WorkflowState::Running,
                WorkflowState::Paused,
                WorkflowState::Cancelling,
            ],
        )?;
        let ready_workflows = count_workflows_by_states(&self.connection, &[WorkflowState::Ready])?;
        Ok(StoreCapacityCounts {
            active_workers,
            queued_workers,
            running_workflows,
            ready_workflows,
        })
    }

    pub fn workflow_worker_counts(
        &self,
        workflow_id: &WorkflowId,
    ) -> StoreResult<WorkflowWorkerCounts> {
        workflow_worker_counts(&self.connection, workflow_id)
    }

    pub fn running_workflows(&self) -> StoreResult<Vec<WorkflowRecord>> {
        let mut statement = self
            .connection
            .prepare(
                workflow_select_sql("WHERE state = 'RUNNING' ORDER BY created_at_ms, workflow_id")
                    .as_str(),
            )
            .store_result()?;
        let workflows = statement
            .query_map([], row_to_workflow)
            .store_result()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .store_result()?;
        Ok(workflows)
    }

    pub fn has_active_exclusive_lease(
        &self,
        canonical_resource: &str,
        now_ms: i64,
    ) -> StoreResult<bool> {
        let count: i64 = self
            .connection
            .query_row(
                r#"
                SELECT COUNT(*)
                FROM coordination_leases
                WHERE canonical_resource = ?1
                  AND mode = 'EXCLUSIVE_WRITE'
                  AND released_at_ms IS NULL
                  AND expires_at_ms > ?2
                "#,
                params![canonical_resource, now_ms],
                |row| row.get(0),
            )
            .store_result()?;
        Ok(count > 0)
    }

    pub fn external_binding(
        &self,
        binding_id: &ExternalBindingId,
    ) -> StoreResult<ExternalBindingRecord> {
        binding_by_id(&self.connection, binding_id)
    }

    pub fn external_binding_for_worker(
        &self,
        worker_id: &WorkerId,
    ) -> StoreResult<Option<ExternalBindingRecord>> {
        binding_by_worker(&self.connection, worker_id)
    }

    pub fn outbox(&self, outbox_id: &OutboxId) -> StoreResult<OutboxRecord> {
        outbox_by_id(&self.connection, outbox_id)
    }

    pub fn transition_workflow_state(
        &mut self,
        workflow_id: &WorkflowId,
        expected_revision: Revision,
        next: WorkflowState,
        correlation_id: &str,
    ) -> StoreResult<WorkflowRecord> {
        let transaction = self.immediate_transaction()?;
        let current = workflow_by_id_tx(&transaction, workflow_id)?;
        let next_revision = require_expected_revision(expected_revision, current.revision)?;
        current.state.ensure_transition(next)?;
        let now = now_ms();
        let changed = transaction
            .execute(
                r#"
                UPDATE coordination_workflows
                SET state = ?2,
                    revision = ?3,
                    updated_at_ms = ?4,
                    started_at_ms = CASE
                        WHEN ?2 = 'RUNNING' THEN COALESCE(started_at_ms, ?4)
                        ELSE started_at_ms
                    END,
                    finished_at_ms = CASE
                        WHEN ?5 THEN COALESCE(finished_at_ms, ?4)
                        ELSE finished_at_ms
                    END
                WHERE workflow_id = ?1 AND revision = ?6
                "#,
                params![
                    workflow_id.as_str(),
                    next.as_str(),
                    to_i64(next_revision.get(), "workflow revision")?,
                    now,
                    next.is_terminal(),
                    to_i64(expected_revision.get(), "workflow revision")?,
                ],
            )
            .store_result()?;
        if changed != 1 {
            let latest = workflow_by_id_tx(&transaction, workflow_id)?;
            return Err(CoordinationError::revision_conflict(
                expected_revision,
                latest.revision,
            ));
        }
        append_event_tx(
            &transaction,
            workflow_id,
            next_revision,
            CoordinationEventKind::WorkflowStateChanged {
                from: current.state,
                to: next,
            },
            correlation_id,
            now,
        )?;
        let workflow = workflow_by_id_tx(&transaction, workflow_id)?;
        transaction.commit().store_result()?;
        Ok(workflow)
    }

    pub fn enqueue_worker_dispatch(
        &mut self,
        request: EnqueueWorkerDispatchRequest,
    ) -> StoreResult<Idempotent<WorkerDispatchRecord>> {
        validate_dispatch_request(&request)?;
        let transaction = self.immediate_transaction()?;
        let scope = idempotency_scope(&request.idempotency_key)?;
        if let Some(response_json) = load_idempotency_response(&transaction, &scope)? {
            let replay: WorkerDispatchReplay =
                serde_json::from_str(&response_json).map_err(map_json)?;
            let record = WorkerDispatchRecord {
                workflow: workflow_by_id_tx(&transaction, &replay.workflow_id)?,
                worker: worker_by_id_tx(&transaction, &replay.worker_id)?,
                binding: binding_by_id_tx(&transaction, &replay.binding_id)?,
                outbox: outbox_by_id_tx(&transaction, &replay.outbox_id)?,
            };
            transaction.commit().store_result()?;
            return Ok(Idempotent::replayed(record));
        }

        if binding_exists_tx(&transaction, &request.binding_id)? {
            return Err(binding_conflict("external binding id is already present"));
        }

        let current_workflow = workflow_by_id_tx(&transaction, &request.workflow_id)?;
        if current_workflow.state.is_terminal() {
            return Err(CoordinationError::invalid_transition(
                "workflow",
                current_workflow.state.as_str(),
                "enqueue_worker_dispatch",
            ));
        }
        let next_workflow_revision = require_expected_revision(
            request.expected_workflow_revision,
            current_workflow.revision,
        )?;
        let now = now_ms();
        transaction
            .execute(
                r#"
                INSERT INTO coordination_workers (
                    worker_id, workflow_id, step_id, state, revision, objective,
                    access, created_at_ms, updated_at_ms
                )
                VALUES (?1, ?2, ?3, 'QUEUED', ?4, ?5, ?6, ?7, ?7)
                "#,
                params![
                    request.worker_id.as_str(),
                    request.workflow_id.as_str(),
                    request.step_id.as_str(),
                    to_i64(Revision::INITIAL.get(), "worker revision")?,
                    request.objective,
                    access_as_sql(request.access),
                    now,
                ],
            )
            .store_result()?;
        transaction
            .execute(
                r#"
                INSERT INTO coordination_external_bindings (
                    binding_id, workflow_id, worker_id, adapter, generation,
                    dispatch_marker, prompt_sha256, state, created_at_ms, updated_at_ms
                )
                VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, 'PENDING', ?7, ?7)
                "#,
                params![
                    request.binding_id.as_str(),
                    request.workflow_id.as_str(),
                    request.worker_id.as_str(),
                    request.adapter,
                    request.dispatch_marker,
                    request.prompt_sha256,
                    now,
                ],
            )
            .store_result()?;
        let payload_json = serde_json::to_string(&request.payload).map_err(map_json)?;
        transaction
            .execute(
                r#"
                INSERT INTO coordination_outbox (
                    outbox_id, workflow_id, worker_id, state, revision, attempt,
                    generation, command_type, payload_json, idempotency_scope,
                    not_before_ms, created_at_ms, updated_at_ms
                )
                VALUES (?1, ?2, ?3, 'PENDING', ?4, 0, 0, 'launch_task', ?5, ?6, ?7, ?8, ?8)
                "#,
                params![
                    request.outbox_id.as_str(),
                    request.workflow_id.as_str(),
                    request.worker_id.as_str(),
                    to_i64(Revision::INITIAL.get(), "outbox revision")?,
                    payload_json,
                    scope,
                    request.not_before_ms,
                    now,
                ],
            )
            .store_result()?;
        let changed = transaction
            .execute(
                r#"
                UPDATE coordination_workflows
                SET revision = ?2, updated_at_ms = ?3
                WHERE workflow_id = ?1 AND revision = ?4
                "#,
                params![
                    request.workflow_id.as_str(),
                    to_i64(next_workflow_revision.get(), "workflow revision")?,
                    now,
                    to_i64(
                        request.expected_workflow_revision.get(),
                        "workflow revision"
                    )?,
                ],
            )
            .store_result()?;
        if changed != 1 {
            let latest = workflow_by_id_tx(&transaction, &request.workflow_id)?;
            return Err(CoordinationError::revision_conflict(
                request.expected_workflow_revision,
                latest.revision,
            ));
        }
        append_event_tx(
            &transaction,
            &request.workflow_id,
            Revision::INITIAL,
            CoordinationEventKind::WorkerCreated {
                worker_id: request.worker_id.clone(),
            },
            &request.correlation_id,
            now,
        )?;
        let replay = WorkerDispatchReplay {
            workflow_id: request.workflow_id.clone(),
            worker_id: request.worker_id.clone(),
            binding_id: request.binding_id.clone(),
            outbox_id: request.outbox_id.clone(),
        };
        insert_idempotency_response(&transaction, &request.idempotency_key, &scope, &replay, now)?;
        if let Some(retain_until_ms) = request.retention_until_ms {
            upsert_retention_tx(
                &transaction,
                "worker",
                request.worker_id.as_str(),
                Some(retain_until_ms),
                None,
                "worker retention",
                now,
            )?;
        }
        let record = WorkerDispatchRecord {
            workflow: workflow_by_id_tx(&transaction, &request.workflow_id)?,
            worker: worker_by_id_tx(&transaction, &request.worker_id)?,
            binding: binding_by_id_tx(&transaction, &request.binding_id)?,
            outbox: outbox_by_id_tx(&transaction, &request.outbox_id)?,
        };
        transaction.commit().store_result()?;
        Ok(Idempotent::created(record))
    }

    pub fn transition_worker_state(
        &mut self,
        worker_id: &WorkerId,
        expected_revision: Revision,
        next: WorkerState,
        correlation_id: &str,
    ) -> StoreResult<WorkerRecord> {
        let transaction = self.immediate_transaction()?;
        let current = worker_by_id_tx(&transaction, worker_id)?;
        let next_revision = require_expected_revision(expected_revision, current.revision)?;
        current.state.ensure_transition(next)?;
        let now = now_ms();
        let changed = transaction
            .execute(
                r#"
                UPDATE coordination_workers
                SET state = ?2,
                    revision = ?3,
                    updated_at_ms = ?4,
                    started_at_ms = CASE
                        WHEN ?2 = 'RUNNING' THEN COALESCE(started_at_ms, ?4)
                        ELSE started_at_ms
                    END,
                    finished_at_ms = CASE
                        WHEN ?5 THEN COALESCE(finished_at_ms, ?4)
                        ELSE finished_at_ms
                    END
                WHERE worker_id = ?1 AND revision = ?6
                "#,
                params![
                    worker_id.as_str(),
                    next.as_str(),
                    to_i64(next_revision.get(), "worker revision")?,
                    now,
                    next.is_terminal(),
                    to_i64(expected_revision.get(), "worker revision")?,
                ],
            )
            .store_result()?;
        if changed != 1 {
            let latest = worker_by_id_tx(&transaction, worker_id)?;
            return Err(CoordinationError::revision_conflict(
                expected_revision,
                latest.revision,
            ));
        }
        append_event_tx(
            &transaction,
            &current.workflow_id,
            next_revision,
            CoordinationEventKind::WorkerStateChanged {
                worker_id: worker_id.clone(),
                from: current.state,
                to: next,
            },
            correlation_id,
            now,
        )?;
        if next.is_terminal() {
            release_worker_leases_tx(&transaction, worker_id, "worker terminal", now)?;
        }
        let worker = worker_by_id_tx(&transaction, worker_id)?;
        transaction.commit().store_result()?;
        Ok(worker)
    }

    pub fn cancel_queued_worker_before_dispatch(
        &mut self,
        worker_id: &WorkerId,
        expected_revision: Option<Revision>,
        reason: Option<&str>,
        correlation_id: &str,
    ) -> StoreResult<WorkerRecord> {
        let transaction = self.immediate_transaction()?;
        let current = worker_by_id_tx(&transaction, worker_id)?;
        if current.state != WorkerState::Queued {
            return Err(CoordinationError::invalid_transition(
                "worker",
                current.state.as_str(),
                "cancel_before_dispatch",
            ));
        }
        if let Some(expected) = expected_revision {
            require_expected_revision(expected, current.revision)?;
        }

        let cancelling_revision = current.revision.next();
        let cancelled_revision = cancelling_revision.next();
        let outbox = pending_outbox_by_worker_tx(&transaction, worker_id)?;
        let now = now_ms();
        transaction
            .execute(
                r#"
                UPDATE coordination_workers
                SET state = 'CANCELLED',
                    revision = ?2,
                    updated_at_ms = ?3,
                    finished_at_ms = COALESCE(finished_at_ms, ?3)
                WHERE worker_id = ?1
                  AND state = 'QUEUED'
                  AND revision = ?4
                "#,
                params![
                    worker_id.as_str(),
                    to_i64(cancelled_revision.get(), "worker revision")?,
                    now,
                    to_i64(current.revision.get(), "worker revision")?,
                ],
            )
            .store_result()?;
        if let Some(outbox) = outbox {
            let outbox_revision = outbox.revision.next();
            outbox.state.ensure_transition(OutboxState::Cancelled)?;
            transaction
                .execute(
                    r#"
                    UPDATE coordination_outbox
                    SET state = 'CANCELLED',
                        revision = ?2,
                        last_error = COALESCE(?3, last_error),
                        updated_at_ms = ?4
                    WHERE outbox_id = ?1
                      AND state IN ('PENDING', 'RETRY_WAIT')
                      AND revision = ?5
                    "#,
                    params![
                        outbox.outbox_id.as_str(),
                        to_i64(outbox_revision.get(), "outbox revision")?,
                        reason,
                        now,
                        to_i64(outbox.revision.get(), "outbox revision")?,
                    ],
                )
                .store_result()?;
            append_event_tx(
                &transaction,
                &current.workflow_id,
                outbox_revision,
                CoordinationEventKind::OutboxStateChanged {
                    outbox_id: outbox.outbox_id,
                    from: outbox.state,
                    to: OutboxState::Cancelled,
                },
                correlation_id,
                now,
            )?;
        }
        transaction
            .execute(
                r#"
                UPDATE coordination_external_bindings
                SET state = 'CANCELLED', updated_at_ms = ?2
                WHERE worker_id = ?1 AND state = 'PENDING'
                "#,
                params![worker_id.as_str(), now],
            )
            .store_result()?;
        append_event_tx(
            &transaction,
            &current.workflow_id,
            cancelling_revision,
            CoordinationEventKind::WorkerStateChanged {
                worker_id: worker_id.clone(),
                from: WorkerState::Queued,
                to: WorkerState::Cancelling,
            },
            correlation_id,
            now,
        )?;
        append_event_tx(
            &transaction,
            &current.workflow_id,
            cancelled_revision,
            CoordinationEventKind::WorkerStateChanged {
                worker_id: worker_id.clone(),
                from: WorkerState::Cancelling,
                to: WorkerState::Cancelled,
            },
            correlation_id,
            now,
        )?;
        let worker = worker_by_id_tx(&transaction, worker_id)?;
        transaction.commit().store_result()?;
        Ok(worker)
    }

    pub fn transition_outbox_state(
        &mut self,
        outbox_id: &OutboxId,
        expected_revision: Revision,
        next: OutboxState,
        last_error: Option<&str>,
        correlation_id: &str,
    ) -> StoreResult<OutboxRecord> {
        let transaction = self.immediate_transaction()?;
        let current = outbox_by_id_tx(&transaction, outbox_id)?;
        let next_revision = require_expected_revision(expected_revision, current.revision)?;
        current.state.ensure_transition(next)?;
        let now = now_ms();
        let changed = transaction
            .execute(
                r#"
                UPDATE coordination_outbox
                SET state = ?2, revision = ?3, last_error = ?4, updated_at_ms = ?5
                WHERE outbox_id = ?1 AND revision = ?6
                "#,
                params![
                    outbox_id.as_str(),
                    next.as_str(),
                    to_i64(next_revision.get(), "outbox revision")?,
                    last_error,
                    now,
                    to_i64(expected_revision.get(), "outbox revision")?,
                ],
            )
            .store_result()?;
        if changed != 1 {
            let latest = outbox_by_id_tx(&transaction, outbox_id)?;
            return Err(CoordinationError::revision_conflict(
                expected_revision,
                latest.revision,
            ));
        }
        append_event_tx(
            &transaction,
            &current.workflow_id,
            next_revision,
            CoordinationEventKind::OutboxStateChanged {
                outbox_id: outbox_id.clone(),
                from: current.state,
                to: next,
            },
            correlation_id,
            now,
        )?;
        let outbox = outbox_by_id_tx(&transaction, outbox_id)?;
        transaction.commit().store_result()?;
        Ok(outbox)
    }

    pub fn record_dispatch_result_unknown(
        &mut self,
        outbox_id: &OutboxId,
        expected_outbox_revision: Revision,
        last_error: &str,
        correlation_id: &str,
    ) -> StoreResult<()> {
        let transaction = self.immediate_transaction()?;
        let outbox = outbox_by_id_tx(&transaction, outbox_id)?;
        let outbox_revision = require_expected_revision(expected_outbox_revision, outbox.revision)?;
        outbox.state.ensure_transition(OutboxState::ResultUnknown)?;
        let worker_id = outbox.worker_id.clone().ok_or_else(|| {
            invariant_error("dispatch outbox is missing workerId for unknown-result recovery")
        })?;
        let worker = worker_by_id_tx(&transaction, &worker_id)?;
        let worker_revision = worker.revision.next();
        worker
            .state
            .ensure_transition(WorkerState::DispatchUnknown)?;
        let binding = binding_by_worker(&transaction, &worker_id)?.ok_or_else(|| {
            invariant_error("dispatch worker is missing binding for unknown-result recovery")
        })?;
        if !matches!(
            binding.state,
            ExternalBindingState::Pending | ExternalBindingState::Active
        ) {
            return Err(CoordinationError::invalid_transition(
                "external_binding",
                binding.state.as_str(),
                ExternalBindingState::ResultUnknown.as_str(),
            ));
        }

        let now = now_ms();
        let outbox_changed = transaction
            .execute(
                r#"
                UPDATE coordination_outbox
                SET state = 'RESULT_UNKNOWN',
                    revision = ?2,
                    last_error = ?3,
                    updated_at_ms = ?4
                WHERE outbox_id = ?1
                  AND revision = ?5
                  AND state = 'SENDING'
                "#,
                params![
                    outbox_id.as_str(),
                    to_i64(outbox_revision.get(), "outbox revision")?,
                    last_error,
                    now,
                    to_i64(expected_outbox_revision.get(), "outbox revision")?,
                ],
            )
            .store_result()?;
        if outbox_changed != 1 {
            let latest = outbox_by_id_tx(&transaction, outbox_id)?;
            return Err(CoordinationError::revision_conflict(
                expected_outbox_revision,
                latest.revision,
            ));
        }
        transaction
            .execute(
                r#"
                UPDATE coordination_workers
                SET state = 'DISPATCH_UNKNOWN',
                    revision = ?2,
                    updated_at_ms = ?3
                WHERE worker_id = ?1
                  AND revision = ?4
                  AND state = 'DISPATCHING'
                "#,
                params![
                    worker_id.as_str(),
                    to_i64(worker_revision.get(), "worker revision")?,
                    now,
                    to_i64(worker.revision.get(), "worker revision")?,
                ],
            )
            .store_result()?;
        transaction
            .execute(
                r#"
                UPDATE coordination_external_bindings
                SET state = 'RESULT_UNKNOWN',
                    generation = generation + 1,
                    updated_at_ms = ?2,
                    last_reconciled_at_ms = ?2
                WHERE binding_id = ?1
                  AND state IN ('PENDING', 'ACTIVE')
                "#,
                params![binding.binding_id.as_str(), now],
            )
            .store_result()?;
        append_event_tx(
            &transaction,
            &outbox.workflow_id,
            outbox_revision,
            CoordinationEventKind::OutboxStateChanged {
                outbox_id: outbox_id.clone(),
                from: OutboxState::Sending,
                to: OutboxState::ResultUnknown,
            },
            correlation_id,
            now,
        )?;
        append_event_tx(
            &transaction,
            &outbox.workflow_id,
            worker_revision,
            CoordinationEventKind::WorkerStateChanged {
                worker_id,
                from: WorkerState::Dispatching,
                to: WorkerState::DispatchUnknown,
            },
            correlation_id,
            now,
        )?;
        transaction.commit().store_result()
    }

    pub fn record_external_launch_success(
        &mut self,
        request: RecordExternalLaunchSuccessRequest,
    ) -> StoreResult<ExternalBindingRecord> {
        validate_external_launch_success_request(&request)?;
        let transaction = self.immediate_transaction()?;
        let current = binding_by_id_tx(&transaction, &request.binding_id)?;
        if current.generation != request.expected_generation {
            return Err(binding_conflict(
                "external binding generation does not match claimed outbox generation",
            ));
        }

        if current.state == ExternalBindingState::Active {
            if current.external_task_ref == request.external_task_ref
                && current.external_session_ref.as_deref()
                    == Some(request.external_session_ref.as_str())
            {
                transaction.commit().store_result()?;
                return Ok(current);
            }
            return Err(binding_conflict(
                "external binding is already active with different external refs",
            ));
        }
        if !matches!(
            current.state,
            ExternalBindingState::Pending | ExternalBindingState::ResultUnknown
        ) {
            return Err(CoordinationError::invalid_transition(
                "external_binding",
                current.state.as_str(),
                ExternalBindingState::Active.as_str(),
            ));
        }

        let duplicate_session_refs: i64 = transaction
            .query_row(
                r#"
                SELECT COUNT(*)
                FROM coordination_external_bindings
                WHERE external_session_ref = ?1
                  AND binding_id <> ?2
                "#,
                params![
                    request.external_session_ref.as_str(),
                    request.binding_id.as_str(),
                ],
                |row| row.get(0),
            )
            .store_result()?;
        if duplicate_session_refs > 0 {
            return Err(binding_conflict(
                "external session ref is already bound to another worker",
            ));
        }

        let now = now_ms();
        let changed = transaction
            .execute(
                r#"
                UPDATE coordination_external_bindings
                SET external_task_ref = ?2,
                    external_session_ref = ?3,
                    state = 'ACTIVE',
                    updated_at_ms = ?4,
                    last_reconciled_at_ms = ?4
                WHERE binding_id = ?1
                  AND generation = ?5
                  AND state IN ('PENDING', 'RESULT_UNKNOWN')
                "#,
                params![
                    request.binding_id.as_str(),
                    request.external_task_ref.as_deref(),
                    request.external_session_ref.as_str(),
                    now,
                    to_i64(request.expected_generation, "binding generation")?,
                ],
            )
            .store_result()?;
        if changed != 1 {
            let latest = binding_by_id_tx(&transaction, &request.binding_id)?;
            if latest.generation != request.expected_generation {
                return Err(binding_conflict(
                    "external binding generation changed before launch success was recorded",
                ));
            }
            return Err(CoordinationError::invalid_transition(
                "external_binding",
                latest.state.as_str(),
                ExternalBindingState::Active.as_str(),
            ));
        }
        let binding = binding_by_id_tx(&transaction, &request.binding_id)?;
        transaction.commit().store_result()?;
        Ok(binding)
    }

    pub fn request_external_worker_cancel(
        &mut self,
        request: RequestExternalWorkerCancelRequest,
    ) -> StoreResult<WorkerCancelRecord> {
        validate_external_cancel_request(&request)?;
        let transaction = self.immediate_transaction()?;
        let current_worker = worker_by_id_tx(&transaction, &request.worker_id)?;
        let binding = binding_by_worker(&transaction, &request.worker_id)?.ok_or_else(|| {
            binding_conflict(format!(
                "external binding not found for worker: {}",
                request.worker_id
            ))
        })?;
        let Some(session_id) = binding.external_session_ref.as_deref() else {
            return Err(binding_conflict(
                "external binding has no session ref to cancel",
            ));
        };

        if binding.state == ExternalBindingState::CancelRequested {
            let outbox = cancel_outbox_by_worker_tx(&transaction, &request.worker_id)?;
            transaction.commit().store_result()?;
            return Ok(WorkerCancelRecord {
                worker: current_worker,
                binding,
                outbox,
                replayed: true,
            });
        }

        if binding.state != ExternalBindingState::Active
            && binding.state != ExternalBindingState::ResultUnknown
        {
            return Err(CoordinationError::invalid_transition(
                "external_binding",
                binding.state.as_str(),
                ExternalBindingState::CancelRequested.as_str(),
            ));
        }
        if !matches!(
            current_worker.state,
            WorkerState::Dispatching | WorkerState::DispatchUnknown | WorkerState::Running
        ) {
            return Err(CoordinationError::invalid_transition(
                "worker",
                current_worker.state.as_str(),
                WorkerState::Cancelling.as_str(),
            ));
        }
        if let Some(expected) = request.expected_worker_revision {
            require_expected_revision(expected, current_worker.revision)?;
        }

        let now = now_ms();
        let next_worker_revision = current_worker.revision.next();
        let payload_json = serde_json::to_string(&serde_json::json!({
            "operation": "kill_session",
            "sessionId": session_id,
            "reason": request
                .reason
                .as_deref()
                .unwrap_or("worker cancellation"),
            "workerId": request.worker_id.as_str(),
            "bindingId": binding.binding_id.as_str(),
        }))
        .map_err(map_json)?;

        transaction
            .execute(
                r#"
                UPDATE coordination_workers
                SET state = 'CANCELLING',
                    revision = ?2,
                    updated_at_ms = ?3
                WHERE worker_id = ?1
                  AND revision = ?4
                "#,
                params![
                    request.worker_id.as_str(),
                    to_i64(next_worker_revision.get(), "worker revision")?,
                    now,
                    to_i64(current_worker.revision.get(), "worker revision")?,
                ],
            )
            .store_result()?;
        transaction
            .execute(
                r#"
                UPDATE coordination_external_bindings
                SET state = 'CANCEL_REQUESTED',
                    updated_at_ms = ?2,
                    last_reconciled_at_ms = ?2
                WHERE binding_id = ?1
                  AND state IN ('ACTIVE', 'RESULT_UNKNOWN')
                "#,
                params![binding.binding_id.as_str(), now],
            )
            .store_result()?;
        transaction
            .execute(
                r#"
                INSERT INTO coordination_outbox (
                    outbox_id, workflow_id, worker_id, state, revision, attempt,
                    generation, command_type, payload_json, idempotency_scope,
                    not_before_ms, created_at_ms, updated_at_ms
                )
                VALUES (?1, ?2, ?3, 'PENDING', ?4, 0, 0, 'kill_session', ?5, ?6, ?7, ?8, ?8)
                "#,
                params![
                    request.outbox_id.as_str(),
                    current_worker.workflow_id.as_str(),
                    request.worker_id.as_str(),
                    to_i64(Revision::INITIAL.get(), "outbox revision")?,
                    payload_json,
                    format!("worker_cancel:{}", request.worker_id.as_str()),
                    request.not_before_ms,
                    now,
                ],
            )
            .store_result()?;
        append_event_tx(
            &transaction,
            &current_worker.workflow_id,
            next_worker_revision,
            CoordinationEventKind::WorkerStateChanged {
                worker_id: request.worker_id.clone(),
                from: current_worker.state,
                to: WorkerState::Cancelling,
            },
            &request.correlation_id,
            now,
        )?;
        let record = WorkerCancelRecord {
            worker: worker_by_id_tx(&transaction, &request.worker_id)?,
            binding: binding_by_id_tx(&transaction, &binding.binding_id)?,
            outbox: outbox_by_id_tx(&transaction, &request.outbox_id)?,
            replayed: false,
        };
        transaction.commit().store_result()?;
        Ok(record)
    }

    pub fn record_external_cancel_success(
        &mut self,
        request: RecordExternalCancelSuccessRequest,
    ) -> StoreResult<ExternalBindingRecord> {
        validate_external_cancel_success_request(&request)?;
        let transaction = self.immediate_transaction()?;
        let current = binding_by_id_tx(&transaction, &request.binding_id)?;
        if current.state == ExternalBindingState::Cancelled {
            transaction.commit().store_result()?;
            return Ok(current);
        }
        if current.state != ExternalBindingState::CancelRequested {
            return Err(CoordinationError::invalid_transition(
                "external_binding",
                current.state.as_str(),
                ExternalBindingState::Cancelled.as_str(),
            ));
        }
        let now = now_ms();
        transaction
            .execute(
                r#"
                UPDATE coordination_external_bindings
                SET state = 'CANCELLED',
                    updated_at_ms = ?2,
                    last_reconciled_at_ms = ?2
                WHERE binding_id = ?1 AND state = 'CANCEL_REQUESTED'
                "#,
                params![request.binding_id.as_str(), now],
            )
            .store_result()?;
        let binding = binding_by_id_tx(&transaction, &request.binding_id)?;
        transaction.commit().store_result()?;
        Ok(binding)
    }

    pub fn due_outbox(&self, now_ms: i64, limit: u32) -> StoreResult<Vec<OutboxRecord>> {
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT outbox_id, workflow_id, worker_id, state, revision, attempt,
                       generation, command_type, payload_json, idempotency_scope,
                       not_before_ms, last_error, created_at_ms, updated_at_ms
                FROM coordination_outbox
                WHERE state IN ('PENDING', 'RETRY_WAIT') AND not_before_ms <= ?1
                ORDER BY created_at_ms, outbox_id
                LIMIT ?2
                "#,
            )
            .store_result()?;
        let outbox = statement
            .query_map(
                params![now_ms, i64::from(limit.clamp(1, 1_000))],
                row_to_outbox,
            )
            .store_result()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .store_result()?;
        Ok(outbox)
    }

    pub fn claim_outbox(
        &mut self,
        outbox_id: &OutboxId,
        now_ms: i64,
    ) -> StoreResult<Option<OutboxRecord>> {
        let transaction = self.immediate_transaction()?;
        let Some(current) = optional_outbox_for_claim(&transaction, outbox_id, now_ms)? else {
            transaction.commit().store_result()?;
            return Ok(None);
        };
        current.state.ensure_transition(OutboxState::Sending)?;
        let next_revision = current.revision.next();
        let next_attempt = current.attempt.saturating_add(1);
        let next_generation = current.generation.saturating_add(1);
        let changed = transaction
            .execute(
                r#"
                UPDATE coordination_outbox
                SET state = 'SENDING',
                    revision = ?2,
                    attempt = ?3,
                    generation = ?4,
                    updated_at_ms = ?5
                WHERE outbox_id = ?1
                  AND revision = ?6
                  AND state IN ('PENDING', 'RETRY_WAIT')
                  AND not_before_ms <= ?5
                "#,
                params![
                    outbox_id.as_str(),
                    to_i64(next_revision.get(), "outbox revision")?,
                    next_attempt,
                    to_i64(next_generation, "outbox generation")?,
                    now_ms,
                    to_i64(current.revision.get(), "outbox revision")?,
                ],
            )
            .store_result()?;
        if changed != 1 {
            transaction.commit().store_result()?;
            return Ok(None);
        }
        if let Some(worker_id) = current.worker_id.as_ref() {
            let worker = worker_by_id_tx(&transaction, worker_id)?;
            if worker.state == WorkerState::Queued {
                let worker_revision = worker.revision.next();
                transaction
                    .execute(
                        r#"
                        UPDATE coordination_workers
                        SET state = 'DISPATCHING', revision = ?2, updated_at_ms = ?3
                        WHERE worker_id = ?1 AND revision = ?4
                        "#,
                        params![
                            worker_id.as_str(),
                            to_i64(worker_revision.get(), "worker revision")?,
                            now_ms,
                            to_i64(worker.revision.get(), "worker revision")?,
                        ],
                    )
                    .store_result()?;
                append_event_tx(
                    &transaction,
                    &worker.workflow_id,
                    worker_revision,
                    CoordinationEventKind::WorkerStateChanged {
                        worker_id: worker_id.clone(),
                        from: WorkerState::Queued,
                        to: WorkerState::Dispatching,
                    },
                    "outbox-claim",
                    now_ms,
                )?;
            }
            transaction
                .execute(
                    r#"
                    UPDATE coordination_external_bindings
                    SET generation = ?2, updated_at_ms = ?3
                    WHERE worker_id = ?1 AND state = 'PENDING'
                    "#,
                    params![
                        worker_id.as_str(),
                        to_i64(next_generation, "binding generation")?,
                        now_ms,
                    ],
                )
                .store_result()?;
        }
        append_event_tx(
            &transaction,
            &current.workflow_id,
            next_revision,
            CoordinationEventKind::OutboxStateChanged {
                outbox_id: outbox_id.clone(),
                from: current.state,
                to: OutboxState::Sending,
            },
            "outbox-claim",
            now_ms,
        )?;
        let outbox = outbox_by_id_tx(&transaction, outbox_id)?;
        transaction.commit().store_result()?;
        Ok(Some(outbox))
    }

    pub fn events(
        &self,
        workflow_id: &WorkflowId,
        after_sequence: u64,
        limit: u32,
    ) -> StoreResult<Vec<EventRecord>> {
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT event_id, workflow_id, sequence, revision, schema_version,
                       kind_json, correlation_id, occurred_at_ms, prev_hash, event_hash
                FROM coordination_events
                WHERE workflow_id = ?1 AND sequence > ?2
                ORDER BY sequence
                LIMIT ?3
                "#,
            )
            .store_result()?;
        let events = statement
            .query_map(
                params![
                    workflow_id.as_str(),
                    to_i64(after_sequence, "event sequence")?,
                    i64::from(limit.clamp(1, 5_000)),
                ],
                row_to_event,
            )
            .store_result()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .store_result()?;
        Ok(events)
    }

    pub fn acquire_lease(&mut self, request: AcquireLeaseRequest) -> StoreResult<LeaseRecord> {
        validate_lease_request(&request)?;
        let transaction = self.immediate_transaction()?;
        workflow_by_id_tx(&transaction, &request.owner_workflow_id)?;
        if let Some(worker_id) = request.owner_worker_id.as_ref() {
            worker_by_id_tx(&transaction, worker_id)?;
        }
        let now = now_ms();
        release_expired_leases_tx(&transaction, now)?;
        let active_conflicts: i64 = transaction
            .query_row(
                r#"
                SELECT COUNT(*)
                FROM coordination_leases
                WHERE canonical_resource = ?1
                  AND released_at_ms IS NULL
                  AND expires_at_ms > ?2
                  AND (?3 = 'EXCLUSIVE_WRITE' OR mode = 'EXCLUSIVE_WRITE')
                "#,
                params![request.canonical_resource, now, request.mode.as_str()],
                |row| row.get(0),
            )
            .store_result()?;
        if active_conflicts > 0 {
            return Err(binding_conflict(
                "active lease conflicts with requested lease mode",
            ));
        }
        transaction
            .execute(
                r#"
                INSERT INTO coordination_leases (
                    lease_id, canonical_resource, resource_sha256, mode,
                    owner_workflow_id, owner_worker_id, generation,
                    acquired_at_ms, heartbeat_at_ms, expires_at_ms
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7, ?8)
                "#,
                params![
                    request.lease_id.as_str(),
                    request.canonical_resource,
                    request.resource_sha256,
                    request.mode.as_str(),
                    request.owner_workflow_id.as_str(),
                    request.owner_worker_id.as_ref().map(WorkerId::as_str),
                    now,
                    now.saturating_add(request.ttl_ms),
                ],
            )
            .store_result()?;
        let lease = lease_by_id_tx(&transaction, &request.lease_id)?;
        transaction.commit().store_result()?;
        Ok(lease)
    }

    pub fn release_lease(&mut self, lease_id: &LeaseId, reason: &str) -> StoreResult<LeaseRecord> {
        let transaction = self.immediate_transaction()?;
        lease_by_id_tx(&transaction, lease_id)?;
        let now = now_ms();
        transaction
            .execute(
                r#"
                UPDATE coordination_leases
                SET released_at_ms = COALESCE(released_at_ms, ?2),
                    release_reason = COALESCE(release_reason, ?3)
                WHERE lease_id = ?1
                "#,
                params![lease_id.as_str(), now, reason],
            )
            .store_result()?;
        let lease = lease_by_id_tx(&transaction, lease_id)?;
        transaction.commit().store_result()?;
        Ok(lease)
    }

    pub fn record_worker_result(
        &mut self,
        request: RecordWorkerResultRequest,
    ) -> StoreResult<ResultRecord> {
        let transaction = self.immediate_transaction()?;
        workflow_by_id_tx(&transaction, &request.workflow_id)?;
        worker_by_id_tx(&transaction, &request.worker_id)?;
        let now = now_ms();
        transaction
            .execute(
                r#"
                INSERT INTO coordination_results (
                    workflow_id, worker_id, worker_state, aggregation_state,
                    summary, artifact_sha256, truncated, completed_at_ms,
                    retention_until_ms
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(workflow_id, worker_id) DO UPDATE SET
                    worker_state = excluded.worker_state,
                    aggregation_state = excluded.aggregation_state,
                    summary = excluded.summary,
                    artifact_sha256 = excluded.artifact_sha256,
                    truncated = excluded.truncated,
                    completed_at_ms = excluded.completed_at_ms,
                    retention_until_ms = excluded.retention_until_ms
                "#,
                params![
                    request.workflow_id.as_str(),
                    request.worker_id.as_str(),
                    request.worker_state.as_str(),
                    result_state_as_sql(request.aggregation_state),
                    request.summary,
                    request.artifact_sha256,
                    bool_as_i64(request.truncated),
                    request.completed_at_ms,
                    request.retention_until_ms,
                ],
            )
            .store_result()?;
        append_event_tx(
            &transaction,
            &request.workflow_id,
            Revision::INITIAL,
            CoordinationEventKind::ResultRecorded {
                worker_id: request.worker_id.clone(),
                truncated: request.truncated,
            },
            "result-record",
            now,
        )?;
        let result = result_by_worker_tx(&transaction, &request.workflow_id, &request.worker_id)?;
        transaction.commit().store_result()?;
        Ok(result)
    }

    pub fn workflow_results(&self, workflow_id: &WorkflowId) -> StoreResult<Vec<ResultRecord>> {
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT workflow_id, worker_id, worker_state, aggregation_state,
                       summary, artifact_sha256, truncated, completed_at_ms,
                       retention_until_ms
                FROM coordination_results
                WHERE workflow_id = ?1
                ORDER BY worker_id
                "#,
            )
            .store_result()?;
        let results = statement
            .query_map(params![workflow_id.as_str()], row_to_result)
            .store_result()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .store_result()?;
        Ok(results)
    }

    pub fn upsert_retention(
        &mut self,
        request: UpsertRetentionRequest,
    ) -> StoreResult<RetentionRecord> {
        let transaction = self.immediate_transaction()?;
        let now = now_ms();
        upsert_retention_tx(
            &transaction,
            &request.entity_kind,
            &request.entity_id,
            request.retain_until_ms,
            request.max_bytes,
            &request.reason,
            now,
        )?;
        let record = retention_by_id_tx(&transaction, &request.entity_kind, &request.entity_id)?;
        transaction.commit().store_result()?;
        Ok(record)
    }

    pub fn retention(
        &self,
        entity_kind: &str,
        entity_id: &str,
    ) -> StoreResult<Option<RetentionRecord>> {
        self.connection
            .query_row(
                r#"
                SELECT entity_kind, entity_id, retain_until_ms, max_bytes, reason,
                       created_at_ms, updated_at_ms
                FROM coordination_retention
                WHERE entity_kind = ?1 AND entity_id = ?2
                "#,
                params![entity_kind, entity_id],
                row_to_retention,
            )
            .optional()
            .store_result()
    }

    fn recover_interrupted_outbox(&mut self) -> StoreResult<()> {
        let interrupted = {
            let mut statement = self
                .connection
                .prepare(
                    r#"
                    SELECT outbox_id, workflow_id, worker_id, revision
                    FROM coordination_outbox
                    WHERE state = 'SENDING'
                    ORDER BY created_at_ms, outbox_id
                    "#,
                )
                .store_result()?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .store_result()?
                .collect::<rusqlite::Result<Vec<_>>>()
                .store_result()?;
            rows
        };
        if interrupted.is_empty() {
            return Ok(());
        }

        let transaction = self.immediate_transaction()?;
        for (outbox_id, workflow_id, worker_id, revision_value) in interrupted {
            let outbox_id = OutboxId::try_new(outbox_id)?;
            let workflow_id = WorkflowId::try_new(workflow_id)?;
            let current_revision = revision_from_i64(revision_value)?;
            let next_revision = current_revision.next();
            let now = now_ms();
            transaction
                .execute(
                    r#"
                    UPDATE coordination_outbox
                    SET state = 'RESULT_UNKNOWN',
                        revision = ?2,
                        updated_at_ms = ?3,
                        last_error = COALESCE(last_error, ?4)
                    WHERE outbox_id = ?1 AND state = 'SENDING'
                    "#,
                    params![
                        outbox_id.as_str(),
                        to_i64(next_revision.get(), "outbox revision")?,
                        now,
                        RECOVERY_ERROR,
                    ],
                )
                .store_result()?;
            append_event_tx(
                &transaction,
                &workflow_id,
                next_revision,
                CoordinationEventKind::OutboxStateChanged {
                    outbox_id: outbox_id.clone(),
                    from: OutboxState::Sending,
                    to: OutboxState::ResultUnknown,
                },
                "outbox-recovery",
                now,
            )?;
            if let Some(worker_id) = worker_id {
                let worker_id = WorkerId::try_new(worker_id)?;
                let worker = worker_by_id_tx(&transaction, &worker_id)?;
                if worker.state == WorkerState::Dispatching {
                    let worker_revision = worker.revision.next();
                    transaction
                        .execute(
                            r#"
                            UPDATE coordination_workers
                            SET state = 'DISPATCH_UNKNOWN',
                                revision = ?2,
                                updated_at_ms = ?3
                            WHERE worker_id = ?1 AND revision = ?4
                            "#,
                            params![
                                worker_id.as_str(),
                                to_i64(worker_revision.get(), "worker revision")?,
                                now,
                                to_i64(worker.revision.get(), "worker revision")?,
                            ],
                        )
                        .store_result()?;
                    append_event_tx(
                        &transaction,
                        &workflow_id,
                        worker_revision,
                        CoordinationEventKind::WorkerStateChanged {
                            worker_id: worker_id.clone(),
                            from: WorkerState::Dispatching,
                            to: WorkerState::DispatchUnknown,
                        },
                        "outbox-recovery",
                        now,
                    )?;
                }
                transaction
                    .execute(
                        r#"
                        UPDATE coordination_external_bindings
                        SET state = 'RESULT_UNKNOWN',
                            generation = generation + 1,
                            updated_at_ms = ?2,
                            last_reconciled_at_ms = ?2
                        WHERE worker_id = ?1
                          AND state IN ('PENDING', 'ACTIVE')
                        "#,
                        params![worker_id.as_str(), now],
                    )
                    .store_result()?;
            }
        }
        transaction.commit().store_result()
    }

    fn verify_event_chains(&self) -> StoreResult<()> {
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT workflow_id, sequence, revision, schema_version, kind_json,
                       correlation_id, prev_hash, event_hash, occurred_at_ms
                FROM coordination_events
                ORDER BY workflow_id, sequence
                "#,
            )
            .store_result()?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            })
            .store_result()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .store_result()?;

        let mut current_workflow = String::new();
        let mut expected_sequence = 1_i64;
        let mut expected_prev_hash = String::new();
        for (
            workflow_id,
            sequence,
            revision,
            schema_version,
            kind_json,
            correlation_id,
            prev_hash,
            event_hash,
            occurred_at_ms,
        ) in rows
        {
            if workflow_id != current_workflow {
                current_workflow = workflow_id.clone();
                expected_sequence = 1;
                expected_prev_hash.clear();
            }
            if sequence != expected_sequence {
                return Err(invariant_error(format!(
                    "event sequence gap for workflow {workflow_id}: expected {expected_sequence}, got {sequence}"
                )));
            }
            if prev_hash != expected_prev_hash {
                return Err(invariant_error(format!(
                    "event previous hash mismatch for workflow {workflow_id} sequence {sequence}"
                )));
            }
            let recalculated = calculate_event_hash(EventHashInput {
                workflow_id: &workflow_id,
                sequence: sequence as u64,
                revision: Revision::new(revision as u64),
                schema_version: &schema_version,
                kind_json: &kind_json,
                correlation_id: &correlation_id,
                prev_hash: &prev_hash,
                occurred_at_ms,
            });
            if recalculated != event_hash {
                return Err(invariant_error(format!(
                    "event hash mismatch for workflow {workflow_id} sequence {sequence}"
                )));
            }
            expected_sequence += 1;
            expected_prev_hash = event_hash;
        }
        Ok(())
    }

    fn immediate_transaction(&mut self) -> StoreResult<Transaction<'_>> {
        self.connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .store_result()
    }
}

fn validate_dispatch_request(request: &EnqueueWorkerDispatchRequest) -> StoreResult<()> {
    if request.objective.trim().is_empty() {
        return Err(CoordinationError::invalid_request(
            "worker objective must not be empty",
        ));
    }
    if request.adapter.trim().is_empty() {
        return Err(CoordinationError::invalid_request(
            "adapter must not be empty",
        ));
    }
    if request.dispatch_marker.trim().is_empty() {
        return Err(CoordinationError::invalid_request(
            "dispatch marker must not be empty",
        ));
    }
    if request.prompt_sha256.trim().is_empty() {
        return Err(CoordinationError::invalid_request(
            "prompt sha256 must not be empty",
        ));
    }
    Ok(())
}

fn validate_external_launch_success_request(
    request: &RecordExternalLaunchSuccessRequest,
) -> StoreResult<()> {
    if request.expected_generation == 0 {
        return Err(CoordinationError::invalid_request(
            "external binding generation must come from a claimed outbox",
        ));
    }
    if invalid_external_ref(&request.external_session_ref) {
        return Err(CoordinationError::invalid_request(
            "external session ref must be non-empty and contain no control characters",
        ));
    }
    if request
        .external_task_ref
        .as_deref()
        .is_some_and(invalid_external_ref)
    {
        return Err(CoordinationError::invalid_request(
            "external task ref must be non-empty and contain no control characters",
        ));
    }
    if request.correlation_id.trim().is_empty() {
        return Err(CoordinationError::invalid_request(
            "correlation id must not be empty",
        ));
    }
    Ok(())
}

fn validate_external_cancel_request(
    request: &RequestExternalWorkerCancelRequest,
) -> StoreResult<()> {
    if request.correlation_id.trim().is_empty() {
        return Err(CoordinationError::invalid_request(
            "correlation id must not be empty",
        ));
    }
    if request
        .reason
        .as_deref()
        .is_some_and(|reason| reason.trim().is_empty() || reason.chars().any(char::is_control))
    {
        return Err(CoordinationError::invalid_request(
            "cancel reason must be non-empty and contain no control characters",
        ));
    }
    Ok(())
}

fn validate_external_cancel_success_request(
    request: &RecordExternalCancelSuccessRequest,
) -> StoreResult<()> {
    if request.correlation_id.trim().is_empty() {
        return Err(CoordinationError::invalid_request(
            "correlation id must not be empty",
        ));
    }
    Ok(())
}

fn invalid_external_ref(value: &str) -> bool {
    value.trim().is_empty() || value.chars().any(char::is_control)
}

fn validate_lease_request(request: &AcquireLeaseRequest) -> StoreResult<()> {
    if request.canonical_resource.trim().is_empty() {
        return Err(CoordinationError::invalid_request(
            "canonical resource must not be empty",
        ));
    }
    if request.resource_sha256.trim().is_empty() {
        return Err(CoordinationError::invalid_request(
            "resource sha256 must not be empty",
        ));
    }
    if request.ttl_ms <= 0 {
        return Err(CoordinationError::invalid_request(
            "lease ttl must be greater than zero",
        ));
    }
    Ok(())
}

fn idempotency_scope(key: &coordination_core::IdempotencyKey) -> StoreResult<String> {
    serde_json::to_string(&(
        key.tool.as_str(),
        key.caller_id.as_str(),
        key.request_id.as_str(),
    ))
    .map_err(map_json)
}

fn load_idempotency_response(
    transaction: &Transaction<'_>,
    scope: &str,
) -> StoreResult<Option<String>> {
    transaction
        .query_row(
            "SELECT response_json FROM coordination_idempotency_records WHERE scope_key = ?1",
            params![scope],
            |row| row.get(0),
        )
        .optional()
        .store_result()
}

fn insert_idempotency_response<T: serde::Serialize>(
    transaction: &Transaction<'_>,
    key: &coordination_core::IdempotencyKey,
    scope: &str,
    response: &T,
    now: i64,
) -> StoreResult<()> {
    let response_json = serde_json::to_string(response).map_err(map_json)?;
    transaction
        .execute(
            r#"
            INSERT INTO coordination_idempotency_records (
                scope_key, tool, caller_id, request_id, response_json, state,
                created_at_ms, expires_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, 'COMPLETED', ?6, ?7)
            "#,
            params![
                scope,
                key.tool,
                key.caller_id.as_str(),
                key.request_id.as_str(),
                response_json,
                now,
                now.saturating_add(DEFAULT_IDEMPOTENCY_RETENTION_MS),
            ],
        )
        .store_result()?;
    Ok(())
}

fn workflow_by_id(
    connection: &Connection,
    workflow_id: &WorkflowId,
) -> StoreResult<WorkflowRecord> {
    connection
        .query_row(
            workflow_select_sql("WHERE workflow_id = ?1").as_str(),
            params![workflow_id.as_str()],
            row_to_workflow,
        )
        .optional()
        .store_result()?
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::WorkflowNotFound,
                format!("workflow not found: {workflow_id}"),
            )
            .retryable(false)
        })
}

fn workflow_by_id_tx(
    transaction: &Transaction<'_>,
    workflow_id: &WorkflowId,
) -> StoreResult<WorkflowRecord> {
    transaction
        .query_row(
            workflow_select_sql("WHERE workflow_id = ?1").as_str(),
            params![workflow_id.as_str()],
            row_to_workflow,
        )
        .optional()
        .store_result()?
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::WorkflowNotFound,
                format!("workflow not found: {workflow_id}"),
            )
            .retryable(false)
        })
}

fn worker_by_id(connection: &Connection, worker_id: &WorkerId) -> StoreResult<WorkerRecord> {
    connection
        .query_row(
            worker_select_sql("WHERE worker_id = ?1").as_str(),
            params![worker_id.as_str()],
            row_to_worker,
        )
        .optional()
        .store_result()?
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::WorkerNotFound,
                format!("worker not found: {worker_id}"),
            )
            .retryable(false)
        })
}

fn worker_by_id_tx(
    transaction: &Transaction<'_>,
    worker_id: &WorkerId,
) -> StoreResult<WorkerRecord> {
    transaction
        .query_row(
            worker_select_sql("WHERE worker_id = ?1").as_str(),
            params![worker_id.as_str()],
            row_to_worker,
        )
        .optional()
        .store_result()?
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::WorkerNotFound,
                format!("worker not found: {worker_id}"),
            )
            .retryable(false)
        })
}

fn workers_for_workflow(
    connection: &Connection,
    workflow_id: &WorkflowId,
) -> StoreResult<Vec<WorkerRecord>> {
    let mut statement = connection
        .prepare(
            worker_select_sql("WHERE workflow_id = ?1 ORDER BY created_at_ms, worker_id").as_str(),
        )
        .store_result()?;
    let workers = statement
        .query_map(params![workflow_id.as_str()], row_to_worker)
        .store_result()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .store_result()?;
    Ok(workers)
}

fn workflow_worker_counts(
    connection: &Connection,
    workflow_id: &WorkflowId,
) -> StoreResult<WorkflowWorkerCounts> {
    workflow_by_id(connection, workflow_id)?;
    let active_workers = count_workers_for_workflow_by_states(
        connection,
        workflow_id,
        &[
            WorkerState::Dispatching,
            WorkerState::DispatchUnknown,
            WorkerState::Running,
            WorkerState::Cancelling,
        ],
    )?;
    let queued_workers =
        count_workers_for_workflow_by_states(connection, workflow_id, &[WorkerState::Queued])?;
    let terminal_workers = count_workers_for_workflow_by_states(
        connection,
        workflow_id,
        &[
            WorkerState::Succeeded,
            WorkerState::Failed,
            WorkerState::Cancelled,
        ],
    )?;
    let total_workers: u32 = connection
        .query_row(
            "SELECT COUNT(*) FROM coordination_workers WHERE workflow_id = ?1",
            params![workflow_id.as_str()],
            |row| row.get(0),
        )
        .store_result()?;
    Ok(WorkflowWorkerCounts {
        active_workers,
        queued_workers,
        terminal_workers,
        total_workers,
    })
}

fn binding_by_id(
    connection: &Connection,
    binding_id: &ExternalBindingId,
) -> StoreResult<ExternalBindingRecord> {
    connection
        .query_row(
            binding_select_sql("WHERE binding_id = ?1").as_str(),
            params![binding_id.as_str()],
            row_to_binding,
        )
        .optional()
        .store_result()?
        .ok_or_else(|| binding_conflict(format!("external binding not found: {binding_id}")))
}

fn binding_by_worker(
    connection: &Connection,
    worker_id: &WorkerId,
) -> StoreResult<Option<ExternalBindingRecord>> {
    connection
        .query_row(
            binding_select_sql("WHERE worker_id = ?1 ORDER BY generation DESC LIMIT 1").as_str(),
            params![worker_id.as_str()],
            row_to_binding,
        )
        .optional()
        .store_result()
}

fn binding_by_id_tx(
    transaction: &Transaction<'_>,
    binding_id: &ExternalBindingId,
) -> StoreResult<ExternalBindingRecord> {
    transaction
        .query_row(
            binding_select_sql("WHERE binding_id = ?1").as_str(),
            params![binding_id.as_str()],
            row_to_binding,
        )
        .optional()
        .store_result()?
        .ok_or_else(|| binding_conflict(format!("external binding not found: {binding_id}")))
}

fn binding_exists_tx(
    transaction: &Transaction<'_>,
    binding_id: &ExternalBindingId,
) -> StoreResult<bool> {
    let exists: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM coordination_external_bindings WHERE binding_id = ?1",
            params![binding_id.as_str()],
            |row| row.get(0),
        )
        .store_result()?;
    Ok(exists > 0)
}

fn outbox_by_id(connection: &Connection, outbox_id: &OutboxId) -> StoreResult<OutboxRecord> {
    connection
        .query_row(
            outbox_select_sql("WHERE outbox_id = ?1").as_str(),
            params![outbox_id.as_str()],
            row_to_outbox,
        )
        .optional()
        .store_result()?
        .ok_or_else(|| {
            CoordinationError::invalid_request(format!("outbox item not found: {outbox_id}"))
        })
}

fn outbox_by_id_tx(
    transaction: &Transaction<'_>,
    outbox_id: &OutboxId,
) -> StoreResult<OutboxRecord> {
    transaction
        .query_row(
            outbox_select_sql("WHERE outbox_id = ?1").as_str(),
            params![outbox_id.as_str()],
            row_to_outbox,
        )
        .optional()
        .store_result()?
        .ok_or_else(|| {
            CoordinationError::invalid_request(format!("outbox item not found: {outbox_id}"))
        })
}

fn optional_outbox_for_claim(
    transaction: &Transaction<'_>,
    outbox_id: &OutboxId,
    now_ms: i64,
) -> StoreResult<Option<OutboxRecord>> {
    transaction
        .query_row(
            outbox_select_sql(
                "WHERE outbox_id = ?1 AND state IN ('PENDING', 'RETRY_WAIT') AND not_before_ms <= ?2",
            )
            .as_str(),
            params![outbox_id.as_str(), now_ms],
            row_to_outbox,
        )
        .optional()
        .store_result()
}

fn pending_outbox_by_worker_tx(
    transaction: &Transaction<'_>,
    worker_id: &WorkerId,
) -> StoreResult<Option<OutboxRecord>> {
    transaction
        .query_row(
            outbox_select_sql(
                "WHERE worker_id = ?1 AND state IN ('PENDING', 'RETRY_WAIT') ORDER BY created_at_ms LIMIT 1",
            )
            .as_str(),
            params![worker_id.as_str()],
            row_to_outbox,
        )
        .optional()
        .store_result()
}

fn cancel_outbox_by_worker_tx(
    transaction: &Transaction<'_>,
    worker_id: &WorkerId,
) -> StoreResult<OutboxRecord> {
    transaction
        .query_row(
            outbox_select_sql(
                "WHERE worker_id = ?1 AND command_type = 'kill_session' ORDER BY created_at_ms LIMIT 1",
            )
            .as_str(),
            params![worker_id.as_str()],
            row_to_outbox,
        )
        .optional()
        .store_result()?
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::InternalInvariantViolation,
                format!("cancel outbox missing for worker: {worker_id}"),
            )
            .retryable(false)
        })
}

fn count_workers_by_states(connection: &Connection, states: &[WorkerState]) -> StoreResult<u32> {
    let state_list = sql_state_list(states.iter().map(|state| state.as_str()));
    let sql = format!("SELECT COUNT(*) FROM coordination_workers WHERE state IN ({state_list})");
    connection
        .query_row(&sql, [], |row| row.get(0))
        .store_result()
}

fn count_workers_for_workflow_by_states(
    connection: &Connection,
    workflow_id: &WorkflowId,
    states: &[WorkerState],
) -> StoreResult<u32> {
    let state_list = sql_state_list(states.iter().map(|state| state.as_str()));
    let sql = format!(
        "SELECT COUNT(*) FROM coordination_workers WHERE workflow_id = ?1 AND state IN ({state_list})"
    );
    connection
        .query_row(&sql, params![workflow_id.as_str()], |row| row.get(0))
        .store_result()
}

fn count_workflows_by_states(
    connection: &Connection,
    states: &[WorkflowState],
) -> StoreResult<u32> {
    let state_list = sql_state_list(states.iter().map(|state| state.as_str()));
    let sql = format!("SELECT COUNT(*) FROM coordination_workflows WHERE state IN ({state_list})");
    connection
        .query_row(&sql, [], |row| row.get(0))
        .store_result()
}

fn sql_state_list<'a>(states: impl Iterator<Item = &'a str>) -> String {
    states
        .map(|state| format!("'{state}'"))
        .collect::<Vec<_>>()
        .join(",")
}

fn lease_by_id_tx(transaction: &Transaction<'_>, lease_id: &LeaseId) -> StoreResult<LeaseRecord> {
    transaction
        .query_row(
            lease_select_sql("WHERE lease_id = ?1").as_str(),
            params![lease_id.as_str()],
            row_to_lease,
        )
        .optional()
        .store_result()?
        .ok_or_else(|| CoordinationError::invalid_request(format!("lease not found: {lease_id}")))
}

fn result_by_worker_tx(
    transaction: &Transaction<'_>,
    workflow_id: &WorkflowId,
    worker_id: &WorkerId,
) -> StoreResult<ResultRecord> {
    transaction
        .query_row(
            r#"
            SELECT workflow_id, worker_id, worker_state, aggregation_state,
                   summary, artifact_sha256, truncated, completed_at_ms,
                   retention_until_ms
            FROM coordination_results
            WHERE workflow_id = ?1 AND worker_id = ?2
            "#,
            params![workflow_id.as_str(), worker_id.as_str()],
            row_to_result,
        )
        .optional()
        .store_result()?
        .ok_or_else(|| CoordinationError::invalid_request("result not found"))
}

fn retention_by_id_tx(
    transaction: &Transaction<'_>,
    entity_kind: &str,
    entity_id: &str,
) -> StoreResult<RetentionRecord> {
    transaction
        .query_row(
            r#"
            SELECT entity_kind, entity_id, retain_until_ms, max_bytes, reason,
                   created_at_ms, updated_at_ms
            FROM coordination_retention
            WHERE entity_kind = ?1 AND entity_id = ?2
            "#,
            params![entity_kind, entity_id],
            row_to_retention,
        )
        .optional()
        .store_result()?
        .ok_or_else(|| CoordinationError::invalid_request("retention metadata not found"))
}

fn upsert_retention_tx(
    transaction: &Transaction<'_>,
    entity_kind: &str,
    entity_id: &str,
    retain_until_ms: Option<i64>,
    max_bytes: Option<u64>,
    reason: &str,
    now: i64,
) -> StoreResult<()> {
    if entity_kind.trim().is_empty() || entity_id.trim().is_empty() || reason.trim().is_empty() {
        return Err(CoordinationError::invalid_request(
            "retention entity kind, entity id, and reason must not be empty",
        ));
    }
    transaction
        .execute(
            r#"
            INSERT INTO coordination_retention (
                entity_kind, entity_id, retain_until_ms, max_bytes, reason,
                created_at_ms, updated_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
            ON CONFLICT(entity_kind, entity_id) DO UPDATE SET
                retain_until_ms = excluded.retain_until_ms,
                max_bytes = excluded.max_bytes,
                reason = excluded.reason,
                updated_at_ms = excluded.updated_at_ms
            "#,
            params![
                entity_kind,
                entity_id,
                retain_until_ms,
                max_bytes
                    .map(|value| to_i64(value, "retention max_bytes"))
                    .transpose()?,
                reason,
                now,
            ],
        )
        .store_result()?;
    Ok(())
}

fn release_expired_leases_tx(transaction: &Transaction<'_>, now: i64) -> StoreResult<()> {
    transaction
        .execute(
            r#"
            UPDATE coordination_leases
            SET released_at_ms = ?1,
                release_reason = COALESCE(release_reason, 'expired')
            WHERE released_at_ms IS NULL AND expires_at_ms <= ?1
            "#,
            params![now],
        )
        .store_result()?;
    Ok(())
}

fn release_worker_leases_tx(
    transaction: &Transaction<'_>,
    worker_id: &WorkerId,
    reason: &str,
    now: i64,
) -> StoreResult<()> {
    transaction
        .execute(
            r#"
            UPDATE coordination_leases
            SET released_at_ms = COALESCE(released_at_ms, ?2),
                release_reason = COALESCE(release_reason, ?3)
            WHERE owner_worker_id = ?1 AND released_at_ms IS NULL
            "#,
            params![worker_id.as_str(), now, reason],
        )
        .store_result()?;
    Ok(())
}

fn append_event_tx(
    transaction: &Transaction<'_>,
    workflow_id: &WorkflowId,
    revision: Revision,
    kind: CoordinationEventKind,
    correlation_id: &str,
    occurred_at_ms: i64,
) -> StoreResult<EventRecord> {
    let (sequence, prev_hash) = transaction
        .query_row(
            r#"
            SELECT COALESCE(MAX(sequence), 0), COALESCE(
                (SELECT event_hash FROM coordination_events
                 WHERE workflow_id = ?1
                 ORDER BY sequence DESC
                 LIMIT 1),
                ''
            )
            FROM coordination_events
            WHERE workflow_id = ?1
            "#,
            params![workflow_id.as_str()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .store_result()?;
    let next_sequence = u64::try_from(sequence)
        .map_err(|_| invariant_error("stored event sequence is negative"))?
        .saturating_add(1);
    let kind_json = serde_json::to_string(&kind).map_err(map_json)?;
    let event_hash = calculate_event_hash(EventHashInput {
        workflow_id: workflow_id.as_str(),
        sequence: next_sequence,
        revision,
        schema_version: COORDINATION_EVENT_SCHEMA_VERSION,
        kind_json: &kind_json,
        correlation_id,
        prev_hash: &prev_hash,
        occurred_at_ms,
    });
    let event_id = EventId::try_new(format!(
        "event-{next_sequence}-{}",
        event_hash.chars().take(16).collect::<String>()
    ))?;
    transaction
        .execute(
            r#"
            INSERT INTO coordination_events (
                event_id, workflow_id, sequence, revision, schema_version,
                kind_json, correlation_id, occurred_at_ms, prev_hash, event_hash
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                event_id.as_str(),
                workflow_id.as_str(),
                to_i64(next_sequence, "event sequence")?,
                to_i64(revision.get(), "event revision")?,
                COORDINATION_EVENT_SCHEMA_VERSION,
                kind_json,
                correlation_id,
                occurred_at_ms,
                prev_hash,
                event_hash,
            ],
        )
        .store_result()?;
    Ok(EventRecord {
        event_id,
        workflow_id: workflow_id.clone(),
        sequence: next_sequence,
        revision,
        schema_version: COORDINATION_EVENT_SCHEMA_VERSION.to_owned(),
        kind,
        correlation_id: correlation_id.to_owned(),
        occurred_at_ms,
        prev_hash,
        event_hash,
    })
}

struct EventHashInput<'a> {
    workflow_id: &'a str,
    sequence: u64,
    revision: Revision,
    schema_version: &'a str,
    kind_json: &'a str,
    correlation_id: &'a str,
    prev_hash: &'a str,
    occurred_at_ms: i64,
}

fn calculate_event_hash(input: EventHashInput<'_>) -> String {
    let mut hasher = Sha256::new();
    for part in [
        input.workflow_id,
        &input.sequence.to_string(),
        &input.revision.get().to_string(),
        input.schema_version,
        input.kind_json,
        input.correlation_id,
        input.prev_hash,
        &input.occurred_at_ms.to_string(),
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

fn workflow_select_sql(where_clause: &str) -> String {
    format!(
        r#"
        SELECT workflow_id, state, revision, spec_json, created_at_ms,
               updated_at_ms, started_at_ms, finished_at_ms, retention_until_ms
        FROM coordination_workflows
        {where_clause}
        "#
    )
}

fn worker_select_sql(where_clause: &str) -> String {
    format!(
        r#"
        SELECT worker_id, workflow_id, step_id, state, revision, objective,
               access, created_at_ms, updated_at_ms, started_at_ms, finished_at_ms
        FROM coordination_workers
        {where_clause}
        "#
    )
}

fn binding_select_sql(where_clause: &str) -> String {
    format!(
        r#"
        SELECT binding_id, workflow_id, worker_id, adapter, generation,
               dispatch_marker, prompt_sha256, external_task_ref,
               external_session_ref, state, created_at_ms, updated_at_ms,
               last_reconciled_at_ms
        FROM coordination_external_bindings
        {where_clause}
        "#
    )
}

fn outbox_select_sql(where_clause: &str) -> String {
    format!(
        r#"
        SELECT outbox_id, workflow_id, worker_id, state, revision, attempt,
               generation, command_type, payload_json, idempotency_scope,
               not_before_ms, last_error, created_at_ms, updated_at_ms
        FROM coordination_outbox
        {where_clause}
        "#
    )
}

fn lease_select_sql(where_clause: &str) -> String {
    format!(
        r#"
        SELECT lease_id, canonical_resource, resource_sha256, mode,
               owner_workflow_id, owner_worker_id, generation, acquired_at_ms,
               heartbeat_at_ms, expires_at_ms, released_at_ms, release_reason
        FROM coordination_leases
        {where_clause}
        "#
    )
}

fn row_to_workflow(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowRecord> {
    let workflow_id: String = row.get(0)?;
    let state: String = row.get(1)?;
    let revision: i64 = row.get(2)?;
    let spec_json: String = row.get(3)?;
    Ok(WorkflowRecord {
        workflow_id: WorkflowId::try_new(workflow_id).map_err(sql_contract_error)?,
        state: parse_workflow_state(&state).map_err(sql_contract_error)?,
        revision: revision_from_i64(revision).map_err(sql_contract_error)?,
        spec: serde_json::from_str::<WorkflowSpecV1Beta1>(&spec_json).map_err(sql_json_error)?,
        created_at_ms: row.get(4)?,
        updated_at_ms: row.get(5)?,
        started_at_ms: row.get(6)?,
        finished_at_ms: row.get(7)?,
        retention_until_ms: row.get(8)?,
    })
}

fn row_to_worker(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkerRecord> {
    let worker_id: String = row.get(0)?;
    let workflow_id: String = row.get(1)?;
    let step_id: String = row.get(2)?;
    let state: String = row.get(3)?;
    let revision: i64 = row.get(4)?;
    let access: String = row.get(6)?;
    Ok(WorkerRecord {
        worker_id: WorkerId::try_new(worker_id).map_err(sql_contract_error)?,
        workflow_id: WorkflowId::try_new(workflow_id).map_err(sql_contract_error)?,
        step_id: StepId::try_new(step_id).map_err(sql_contract_error)?,
        state: parse_worker_state(&state).map_err(sql_contract_error)?,
        revision: revision_from_i64(revision).map_err(sql_contract_error)?,
        objective: row.get(5)?,
        access: parse_access(&access).map_err(sql_contract_error)?,
        created_at_ms: row.get(7)?,
        updated_at_ms: row.get(8)?,
        started_at_ms: row.get(9)?,
        finished_at_ms: row.get(10)?,
    })
}

fn row_to_binding(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExternalBindingRecord> {
    let binding_id: String = row.get(0)?;
    let workflow_id: String = row.get(1)?;
    let worker_id: String = row.get(2)?;
    let generation: i64 = row.get(4)?;
    let state: String = row.get(9)?;
    Ok(ExternalBindingRecord {
        binding_id: ExternalBindingId::try_new(binding_id).map_err(sql_contract_error)?,
        workflow_id: WorkflowId::try_new(workflow_id).map_err(sql_contract_error)?,
        worker_id: WorkerId::try_new(worker_id).map_err(sql_contract_error)?,
        adapter: row.get(3)?,
        generation: u64::try_from(generation).map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Integer,
                Box::new(invariant_error("binding generation is negative")),
            )
        })?,
        dispatch_marker: row.get(5)?,
        prompt_sha256: row.get(6)?,
        external_task_ref: row.get(7)?,
        external_session_ref: row.get(8)?,
        state: parse_binding_state(&state).map_err(sql_contract_error)?,
        created_at_ms: row.get(10)?,
        updated_at_ms: row.get(11)?,
        last_reconciled_at_ms: row.get(12)?,
    })
}

fn row_to_outbox(row: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxRecord> {
    let outbox_id: String = row.get(0)?;
    let workflow_id: String = row.get(1)?;
    let worker_id: Option<String> = row.get(2)?;
    let state: String = row.get(3)?;
    let revision: i64 = row.get(4)?;
    let generation: i64 = row.get(6)?;
    let payload_json: String = row.get(8)?;
    Ok(OutboxRecord {
        outbox_id: OutboxId::try_new(outbox_id).map_err(sql_contract_error)?,
        workflow_id: WorkflowId::try_new(workflow_id).map_err(sql_contract_error)?,
        worker_id: worker_id
            .map(WorkerId::try_new)
            .transpose()
            .map_err(sql_contract_error)?,
        state: parse_outbox_state(&state).map_err(sql_contract_error)?,
        revision: revision_from_i64(revision).map_err(sql_contract_error)?,
        attempt: row.get(5)?,
        generation: u64::try_from(generation).map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Integer,
                Box::new(invariant_error("outbox generation is negative")),
            )
        })?,
        command_type: row.get(7)?,
        payload: serde_json::from_str(&payload_json).map_err(sql_json_error)?,
        idempotency_scope: row.get(9)?,
        not_before_ms: row.get(10)?,
        last_error: row.get(11)?,
        created_at_ms: row.get(12)?,
        updated_at_ms: row.get(13)?,
    })
}

fn row_to_lease(row: &rusqlite::Row<'_>) -> rusqlite::Result<LeaseRecord> {
    let lease_id: String = row.get(0)?;
    let mode: String = row.get(3)?;
    let workflow_id: String = row.get(4)?;
    let worker_id: Option<String> = row.get(5)?;
    let generation: i64 = row.get(6)?;
    Ok(LeaseRecord {
        lease_id: LeaseId::try_new(lease_id).map_err(sql_contract_error)?,
        canonical_resource: row.get(1)?,
        resource_sha256: row.get(2)?,
        mode: parse_lease_mode(&mode).map_err(sql_contract_error)?,
        owner_workflow_id: WorkflowId::try_new(workflow_id).map_err(sql_contract_error)?,
        owner_worker_id: worker_id
            .map(WorkerId::try_new)
            .transpose()
            .map_err(sql_contract_error)?,
        generation: u64::try_from(generation).map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Integer,
                Box::new(invariant_error("lease generation is negative")),
            )
        })?,
        acquired_at_ms: row.get(7)?,
        heartbeat_at_ms: row.get(8)?,
        expires_at_ms: row.get(9)?,
        released_at_ms: row.get(10)?,
        release_reason: row.get(11)?,
    })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRecord> {
    let event_id: String = row.get(0)?;
    let workflow_id: String = row.get(1)?;
    let sequence: i64 = row.get(2)?;
    let revision: i64 = row.get(3)?;
    let kind_json: String = row.get(5)?;
    Ok(EventRecord {
        event_id: EventId::try_new(event_id).map_err(sql_contract_error)?,
        workflow_id: WorkflowId::try_new(workflow_id).map_err(sql_contract_error)?,
        sequence: u64::try_from(sequence).map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Integer,
                Box::new(invariant_error("event sequence is negative")),
            )
        })?,
        revision: revision_from_i64(revision).map_err(sql_contract_error)?,
        schema_version: row.get(4)?,
        kind: serde_json::from_str(&kind_json).map_err(sql_json_error)?,
        correlation_id: row.get(6)?,
        occurred_at_ms: row.get(7)?,
        prev_hash: row.get(8)?,
        event_hash: row.get(9)?,
    })
}

fn row_to_result(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResultRecord> {
    let workflow_id: String = row.get(0)?;
    let worker_id: String = row.get(1)?;
    let worker_state: String = row.get(2)?;
    let aggregation_state: String = row.get(3)?;
    let truncated: i64 = row.get(6)?;
    Ok(ResultRecord {
        workflow_id: WorkflowId::try_new(workflow_id).map_err(sql_contract_error)?,
        worker_id: WorkerId::try_new(worker_id).map_err(sql_contract_error)?,
        worker_state: parse_worker_state(&worker_state).map_err(sql_contract_error)?,
        aggregation_state: parse_result_state(&aggregation_state).map_err(sql_contract_error)?,
        summary: row.get(4)?,
        artifact_sha256: row.get(5)?,
        truncated: truncated != 0,
        completed_at_ms: row.get(7)?,
        retention_until_ms: row.get(8)?,
    })
}

fn row_to_retention(row: &rusqlite::Row<'_>) -> rusqlite::Result<RetentionRecord> {
    let max_bytes: Option<i64> = row.get(3)?;
    Ok(RetentionRecord {
        entity_kind: row.get(0)?,
        entity_id: row.get(1)?,
        retain_until_ms: row.get(2)?,
        max_bytes: max_bytes.map(u64::try_from).transpose().map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Integer,
                Box::new(invariant_error("retention max_bytes is negative")),
            )
        })?,
        reason: row.get(4)?,
        created_at_ms: row.get(5)?,
        updated_at_ms: row.get(6)?,
    })
}

fn sql_contract_error(error: CoordinationError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn sql_json_error(error: serde_json::Error) -> rusqlite::Error {
    sql_contract_error(map_json(error))
}

fn parse_workflow_state(value: &str) -> StoreResult<WorkflowState> {
    match value {
        "DRAFT" => Ok(WorkflowState::Draft),
        "READY" => Ok(WorkflowState::Ready),
        "RUNNING" => Ok(WorkflowState::Running),
        "PAUSED" => Ok(WorkflowState::Paused),
        "CANCELLING" => Ok(WorkflowState::Cancelling),
        "SUCCEEDED" => Ok(WorkflowState::Succeeded),
        "PARTIAL_FAILED" => Ok(WorkflowState::PartialFailed),
        "FAILED" => Ok(WorkflowState::Failed),
        "CANCELLED" => Ok(WorkflowState::Cancelled),
        other => Err(invariant_error(format!("unknown workflow state {other}"))),
    }
}

fn parse_worker_state(value: &str) -> StoreResult<WorkerState> {
    match value {
        "QUEUED" => Ok(WorkerState::Queued),
        "DISPATCHING" => Ok(WorkerState::Dispatching),
        "DISPATCH_UNKNOWN" => Ok(WorkerState::DispatchUnknown),
        "RUNNING" => Ok(WorkerState::Running),
        "CANCELLING" => Ok(WorkerState::Cancelling),
        "SUCCEEDED" => Ok(WorkerState::Succeeded),
        "FAILED" => Ok(WorkerState::Failed),
        "CANCELLED" => Ok(WorkerState::Cancelled),
        other => Err(invariant_error(format!("unknown worker state {other}"))),
    }
}

fn parse_outbox_state(value: &str) -> StoreResult<OutboxState> {
    match value {
        "PENDING" => Ok(OutboxState::Pending),
        "SENDING" => Ok(OutboxState::Sending),
        "ACKNOWLEDGED" => Ok(OutboxState::Acknowledged),
        "RESULT_UNKNOWN" => Ok(OutboxState::ResultUnknown),
        "RETRY_WAIT" => Ok(OutboxState::RetryWait),
        "CANCELLED" => Ok(OutboxState::Cancelled),
        other => Err(invariant_error(format!("unknown outbox state {other}"))),
    }
}

fn parse_binding_state(value: &str) -> StoreResult<ExternalBindingState> {
    match value {
        "PENDING" => Ok(ExternalBindingState::Pending),
        "ACTIVE" => Ok(ExternalBindingState::Active),
        "RESULT_UNKNOWN" => Ok(ExternalBindingState::ResultUnknown),
        "CANCEL_REQUESTED" => Ok(ExternalBindingState::CancelRequested),
        "SUCCEEDED" => Ok(ExternalBindingState::Succeeded),
        "FAILED" => Ok(ExternalBindingState::Failed),
        "CANCELLED" => Ok(ExternalBindingState::Cancelled),
        "SUPERSEDED" => Ok(ExternalBindingState::Superseded),
        other => Err(invariant_error(format!(
            "unknown external binding state {other}"
        ))),
    }
}

fn parse_lease_mode(value: &str) -> StoreResult<LeaseMode> {
    match value {
        "SHARED_READ" => Ok(LeaseMode::SharedRead),
        "EXCLUSIVE_WRITE" => Ok(LeaseMode::ExclusiveWrite),
        other => Err(invariant_error(format!("unknown lease mode {other}"))),
    }
}

fn parse_access(value: &str) -> StoreResult<AccessMode> {
    match value {
        "read" => Ok(AccessMode::Read),
        "write" => Ok(AccessMode::Write),
        other => Err(invariant_error(format!("unknown access mode {other}"))),
    }
}

fn parse_result_state(value: &str) -> StoreResult<ResultAggregationState> {
    match value {
        "complete" => Ok(ResultAggregationState::Complete),
        "partial" => Ok(ResultAggregationState::Partial),
        "pending" => Ok(ResultAggregationState::Pending),
        "unavailable" => Ok(ResultAggregationState::Unavailable),
        other => Err(invariant_error(format!(
            "unknown result aggregation state {other}"
        ))),
    }
}

const fn access_as_sql(value: AccessMode) -> &'static str {
    match value {
        AccessMode::Read => "read",
        AccessMode::Write => "write",
    }
}

const fn result_state_as_sql(value: ResultAggregationState) -> &'static str {
    match value {
        ResultAggregationState::Complete => "complete",
        ResultAggregationState::Partial => "partial",
        ResultAggregationState::Pending => "pending",
        ResultAggregationState::Unavailable => "unavailable",
    }
}

const fn bool_as_i64(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_hash_changes_when_payload_changes() {
        let first = calculate_event_hash(EventHashInput {
            workflow_id: "workflow",
            sequence: 1,
            revision: Revision::new(1),
            schema_version: COORDINATION_EVENT_SCHEMA_VERSION,
            kind_json: r#"{"type":"workflow_created"}"#,
            correlation_id: "corr",
            prev_hash: "",
            occurred_at_ms: 10,
        });
        let second = calculate_event_hash(EventHashInput {
            workflow_id: "workflow",
            sequence: 1,
            revision: Revision::new(1),
            schema_version: COORDINATION_EVENT_SCHEMA_VERSION,
            kind_json: r#"{"type":"worker_created","payload":{"workerId":"worker"}}"#,
            correlation_id: "corr",
            prev_hash: "",
            occurred_at_ms: 10,
        });
        assert_ne!(first, second);
    }

    #[test]
    fn state_parsers_cover_phase2_persistent_states() {
        assert_eq!(
            parse_worker_state("DISPATCH_UNKNOWN").unwrap(),
            WorkerState::DispatchUnknown
        );
        assert_eq!(
            parse_outbox_state("RESULT_UNKNOWN").unwrap(),
            OutboxState::ResultUnknown
        );
        assert_eq!(
            parse_binding_state("CANCEL_REQUESTED").unwrap(),
            ExternalBindingState::CancelRequested
        );
    }
}
