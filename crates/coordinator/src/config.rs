use coordination_core::{CoordinationError, ErrorCode};
use serde::{Deserialize, Serialize};

pub const DEFAULT_HOST_MAX_ACTIVE_WORKERS: u32 = 24;
pub const DEFAULT_DISPATCH_INFLIGHT: usize = 6;
pub const DEFAULT_RECONCILE_INFLIGHT: usize = 24;
pub const DEFAULT_CONTROL_INFLIGHT: usize = 12;
pub const DEFAULT_MAX_DISPATCHES_PER_WORKFLOW_TICK: u32 = 12;
pub const DEFAULT_WORK_QUEUE_CAPACITY: usize = 128;
pub const DEFAULT_IDLE_WAIT_MS: u64 = 1_000;
pub const DEFAULT_ADAPTER_PROBE_INTERVAL_MS: u64 = 15_000;
pub const DEFAULT_CRASH_BACKOFF_MIN_MS: u64 = 1_000;
pub const DEFAULT_CRASH_BACKOFF_MAX_MS: u64 = 60_000;
pub const DEFAULT_WORKER_LEASE_TTL_MS: i64 = 30 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CoordinatorConfig {
    pub dispatch_enabled: bool,
    pub host_max_active_workers: u32,
    pub max_dispatches_per_workflow_tick: u32,
    pub dispatch_inflight: usize,
    pub reconcile_inflight: usize,
    pub control_inflight: usize,
    pub due_outbox_scan_limit: u32,
    pub work_queue_capacity: usize,
    pub idle_wait_ms: u64,
    pub adapter_probe_interval_ms: u64,
    pub crash_backoff_min_ms: u64,
    pub crash_backoff_max_ms: u64,
    pub worker_lease_ttl_ms: i64,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            dispatch_enabled: true,
            host_max_active_workers: DEFAULT_HOST_MAX_ACTIVE_WORKERS,
            max_dispatches_per_workflow_tick: DEFAULT_MAX_DISPATCHES_PER_WORKFLOW_TICK,
            dispatch_inflight: DEFAULT_DISPATCH_INFLIGHT,
            reconcile_inflight: DEFAULT_RECONCILE_INFLIGHT,
            control_inflight: DEFAULT_CONTROL_INFLIGHT,
            due_outbox_scan_limit: 1_000,
            work_queue_capacity: DEFAULT_WORK_QUEUE_CAPACITY,
            idle_wait_ms: DEFAULT_IDLE_WAIT_MS,
            adapter_probe_interval_ms: DEFAULT_ADAPTER_PROBE_INTERVAL_MS,
            crash_backoff_min_ms: DEFAULT_CRASH_BACKOFF_MIN_MS,
            crash_backoff_max_ms: DEFAULT_CRASH_BACKOFF_MAX_MS,
            worker_lease_ttl_ms: DEFAULT_WORKER_LEASE_TTL_MS,
        }
    }
}

impl CoordinatorConfig {
    pub fn validate(&self) -> Result<(), CoordinationError> {
        if self.host_max_active_workers == 0 || self.host_max_active_workers > 256 {
            return Err(CoordinationError::new(
                ErrorCode::CapacityExhausted,
                "hostMaxActiveWorkers must be within 1..=256",
            ));
        }
        if self.max_dispatches_per_workflow_tick == 0 || self.max_dispatches_per_workflow_tick > 256
        {
            return Err(CoordinationError::new(
                ErrorCode::CapacityExhausted,
                "maxDispatchesPerWorkflowTick must be within 1..=256",
            ));
        }
        validate_usize("dispatchInflight", self.dispatch_inflight, 1, 64)?;
        validate_usize("reconcileInflight", self.reconcile_inflight, 1, 256)?;
        validate_usize("controlInflight", self.control_inflight, 1, 128)?;
        validate_usize("workQueueCapacity", self.work_queue_capacity, 1, 10_000)?;
        if self.due_outbox_scan_limit == 0 || self.due_outbox_scan_limit > 10_000 {
            return Err(CoordinationError::new(
                ErrorCode::CapacityExhausted,
                "dueOutboxScanLimit must be within 1..=10000",
            ));
        }
        if self.idle_wait_ms < 1_000 {
            return Err(CoordinationError::invalid_request(
                "idleWaitMs must be at least 1000",
            ));
        }
        if self.adapter_probe_interval_ms < 15_000 {
            return Err(CoordinationError::invalid_request(
                "adapterProbeIntervalMs must be at least 15000",
            ));
        }
        if self.crash_backoff_min_ms == 0 || self.crash_backoff_min_ms > self.crash_backoff_max_ms {
            return Err(CoordinationError::invalid_request(
                "crash backoff bounds are invalid",
            ));
        }
        if self.worker_lease_ttl_ms <= 0 {
            return Err(CoordinationError::invalid_request(
                "workerLeaseTtlMs must be greater than zero",
            ));
        }
        Ok(())
    }
}

fn validate_usize(
    name: &'static str,
    value: usize,
    min: usize,
    max: usize,
) -> Result<(), CoordinationError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(CoordinationError::new(
            ErrorCode::CapacityExhausted,
            format!("{name} must be within {min}..={max}"),
        ))
    }
}
