use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{CoordinationError, Revisioned};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkflowState {
    Draft,
    Ready,
    Running,
    Paused,
    Cancelling,
    Succeeded,
    PartialFailed,
    Failed,
    Cancelled,
}

const WORKFLOW_STATES: [WorkflowState; 9] = [
    WorkflowState::Draft,
    WorkflowState::Ready,
    WorkflowState::Running,
    WorkflowState::Paused,
    WorkflowState::Cancelling,
    WorkflowState::Succeeded,
    WorkflowState::PartialFailed,
    WorkflowState::Failed,
    WorkflowState::Cancelled,
];

impl WorkflowState {
    pub const ALL: &'static [Self] = &WORKFLOW_STATES;

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Ready => "READY",
            Self::Running => "RUNNING",
            Self::Paused => "PAUSED",
            Self::Cancelling => "CANCELLING",
            Self::Succeeded => "SUCCEEDED",
            Self::PartialFailed => "PARTIAL_FAILED",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::PartialFailed | Self::Failed | Self::Cancelled
        )
    }

    pub const fn allowed_next(self) -> &'static [Self] {
        match self {
            Self::Draft => &[Self::Ready, Self::Cancelled],
            Self::Ready => &[Self::Running, Self::Paused, Self::Cancelling],
            Self::Running => &[
                Self::Succeeded,
                Self::PartialFailed,
                Self::Failed,
                Self::Paused,
                Self::Cancelling,
            ],
            Self::Paused => &[Self::Ready, Self::Running, Self::Cancelling],
            Self::Cancelling => &[Self::Cancelled, Self::Failed],
            Self::Succeeded | Self::PartialFailed | Self::Failed | Self::Cancelled => &[],
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        self.allowed_next().contains(&next)
    }

    pub fn ensure_transition(self, next: Self) -> Result<(), CoordinationError> {
        ensure_transition(
            "workflow",
            self.as_str(),
            next.as_str(),
            self.can_transition_to(next),
        )
    }

    pub fn transition(
        current: Revisioned<Self>,
        next: Self,
    ) -> Result<Revisioned<Self>, CoordinationError> {
        current.value.ensure_transition(next)?;
        Ok(Revisioned::new(next, current.revision.next()))
    }
}

impl fmt::Display for WorkflowState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkerState {
    Queued,
    Dispatching,
    DispatchUnknown,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
}

const WORKER_STATES: [WorkerState; 8] = [
    WorkerState::Queued,
    WorkerState::Dispatching,
    WorkerState::DispatchUnknown,
    WorkerState::Running,
    WorkerState::Cancelling,
    WorkerState::Succeeded,
    WorkerState::Failed,
    WorkerState::Cancelled,
];

impl WorkerState {
    pub const ALL: &'static [Self] = &WORKER_STATES;

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "QUEUED",
            Self::Dispatching => "DISPATCHING",
            Self::DispatchUnknown => "DISPATCH_UNKNOWN",
            Self::Running => "RUNNING",
            Self::Cancelling => "CANCELLING",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    pub const fn allowed_next(self) -> &'static [Self] {
        match self {
            Self::Queued => &[Self::Dispatching, Self::Cancelling],
            Self::Dispatching => &[
                Self::Running,
                Self::DispatchUnknown,
                Self::Failed,
                Self::Cancelling,
            ],
            Self::DispatchUnknown => &[Self::Running, Self::Failed, Self::Cancelling],
            Self::Running => &[Self::Succeeded, Self::Failed, Self::Cancelling],
            Self::Cancelling => &[Self::Cancelled, Self::Failed],
            Self::Succeeded | Self::Failed | Self::Cancelled => &[],
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        self.allowed_next().contains(&next)
    }

    pub fn ensure_transition(self, next: Self) -> Result<(), CoordinationError> {
        ensure_transition(
            "worker",
            self.as_str(),
            next.as_str(),
            self.can_transition_to(next),
        )
    }

    pub fn transition(
        current: Revisioned<Self>,
        next: Self,
    ) -> Result<Revisioned<Self>, CoordinationError> {
        current.value.ensure_transition(next)?;
        Ok(Revisioned::new(next, current.revision.next()))
    }
}

impl fmt::Display for WorkerState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OutboxState {
    Pending,
    Sending,
    Acknowledged,
    ResultUnknown,
    RetryWait,
    Cancelled,
}

const OUTBOX_STATES: [OutboxState; 6] = [
    OutboxState::Pending,
    OutboxState::Sending,
    OutboxState::Acknowledged,
    OutboxState::ResultUnknown,
    OutboxState::RetryWait,
    OutboxState::Cancelled,
];

impl OutboxState {
    pub const ALL: &'static [Self] = &OUTBOX_STATES;

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Sending => "SENDING",
            Self::Acknowledged => "ACKNOWLEDGED",
            Self::ResultUnknown => "RESULT_UNKNOWN",
            Self::RetryWait => "RETRY_WAIT",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Acknowledged | Self::ResultUnknown | Self::Cancelled
        )
    }

    pub const fn allowed_next(self) -> &'static [Self] {
        match self {
            Self::Pending => &[Self::Sending, Self::RetryWait, Self::Cancelled],
            Self::Sending => &[
                Self::Acknowledged,
                Self::ResultUnknown,
                Self::RetryWait,
                Self::Cancelled,
            ],
            Self::RetryWait => &[Self::Pending, Self::Cancelled],
            Self::Acknowledged | Self::ResultUnknown | Self::Cancelled => &[],
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        self.allowed_next().contains(&next)
    }

    pub fn ensure_transition(self, next: Self) -> Result<(), CoordinationError> {
        ensure_transition(
            "outbox",
            self.as_str(),
            next.as_str(),
            self.can_transition_to(next),
        )
    }

    pub fn transition(
        current: Revisioned<Self>,
        next: Self,
    ) -> Result<Revisioned<Self>, CoordinationError> {
        current.value.ensure_transition(next)?;
        Ok(Revisioned::new(next, current.revision.next()))
    }
}

impl fmt::Display for OutboxState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn ensure_transition(
    entity: &'static str,
    from: &'static str,
    to: &'static str,
    allowed: bool,
) -> Result<(), CoordinationError> {
    if allowed {
        Ok(())
    } else {
        Err(CoordinationError::invalid_transition(entity, from, to))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultAggregationState {
    Complete,
    Partial,
    Pending,
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorCode, Revision};

    fn assert_matrix<T>(
        states: &[T],
        expected_next: impl Fn(T) -> &'static [T],
        can: impl Fn(T, T) -> bool,
    ) where
        T: Copy + Eq + std::fmt::Debug + 'static,
    {
        for &from in states {
            for &to in states {
                assert_eq!(
                    can(from, to),
                    expected_next(from).contains(&to),
                    "transition matrix mismatch for {from:?} -> {to:?}"
                );
            }
        }
    }

    fn expected_workflow_next(state: WorkflowState) -> &'static [WorkflowState] {
        match state {
            WorkflowState::Draft => &[WorkflowState::Ready, WorkflowState::Cancelled],
            WorkflowState::Ready => &[
                WorkflowState::Running,
                WorkflowState::Paused,
                WorkflowState::Cancelling,
            ],
            WorkflowState::Running => &[
                WorkflowState::Succeeded,
                WorkflowState::PartialFailed,
                WorkflowState::Failed,
                WorkflowState::Paused,
                WorkflowState::Cancelling,
            ],
            WorkflowState::Paused => &[
                WorkflowState::Ready,
                WorkflowState::Running,
                WorkflowState::Cancelling,
            ],
            WorkflowState::Cancelling => &[WorkflowState::Cancelled, WorkflowState::Failed],
            WorkflowState::Succeeded
            | WorkflowState::PartialFailed
            | WorkflowState::Failed
            | WorkflowState::Cancelled => &[],
        }
    }

    fn expected_worker_next(state: WorkerState) -> &'static [WorkerState] {
        match state {
            WorkerState::Queued => &[WorkerState::Dispatching, WorkerState::Cancelling],
            WorkerState::Dispatching => &[
                WorkerState::Running,
                WorkerState::DispatchUnknown,
                WorkerState::Failed,
                WorkerState::Cancelling,
            ],
            WorkerState::DispatchUnknown => &[
                WorkerState::Running,
                WorkerState::Failed,
                WorkerState::Cancelling,
            ],
            WorkerState::Running => &[
                WorkerState::Succeeded,
                WorkerState::Failed,
                WorkerState::Cancelling,
            ],
            WorkerState::Cancelling => &[WorkerState::Cancelled, WorkerState::Failed],
            WorkerState::Succeeded | WorkerState::Failed | WorkerState::Cancelled => &[],
        }
    }

    fn expected_outbox_next(state: OutboxState) -> &'static [OutboxState] {
        match state {
            OutboxState::Pending => &[
                OutboxState::Sending,
                OutboxState::RetryWait,
                OutboxState::Cancelled,
            ],
            OutboxState::Sending => &[
                OutboxState::Acknowledged,
                OutboxState::ResultUnknown,
                OutboxState::RetryWait,
                OutboxState::Cancelled,
            ],
            OutboxState::RetryWait => &[OutboxState::Pending, OutboxState::Cancelled],
            OutboxState::Acknowledged | OutboxState::ResultUnknown | OutboxState::Cancelled => &[],
        }
    }

    #[test]
    fn workflow_transition_matrix_covers_all_legal_and_illegal_edges() {
        assert_matrix(
            WorkflowState::ALL,
            expected_workflow_next,
            WorkflowState::can_transition_to,
        );
        assert!(WorkflowState::Running.can_transition_to(WorkflowState::Cancelling));
        assert!(WorkflowState::Running.can_transition_to(WorkflowState::PartialFailed));
        assert!(!WorkflowState::Succeeded.can_transition_to(WorkflowState::Running));
    }

    #[test]
    fn worker_transition_matrix_covers_dispatch_unknown_and_cancelling() {
        assert_matrix(
            WorkerState::ALL,
            expected_worker_next,
            WorkerState::can_transition_to,
        );
        assert!(WorkerState::Dispatching.can_transition_to(WorkerState::DispatchUnknown));
        assert!(WorkerState::DispatchUnknown.can_transition_to(WorkerState::Running));
        assert!(WorkerState::Running.can_transition_to(WorkerState::Cancelling));
        assert!(!WorkerState::DispatchUnknown.can_transition_to(WorkerState::Succeeded));
    }

    #[test]
    fn outbox_transition_matrix_covers_all_legal_and_illegal_edges() {
        assert_matrix(
            OutboxState::ALL,
            expected_outbox_next,
            OutboxState::can_transition_to,
        );
        assert!(OutboxState::Sending.can_transition_to(OutboxState::ResultUnknown));
        assert!(OutboxState::RetryWait.can_transition_to(OutboxState::Pending));
        assert!(!OutboxState::Acknowledged.can_transition_to(OutboxState::Pending));
    }

    #[test]
    fn illegal_transition_returns_public_error_code_and_current_state() {
        let error = WorkerState::DispatchUnknown
            .ensure_transition(WorkerState::Succeeded)
            .expect_err("illegal transition");
        assert_eq!(error.code, ErrorCode::InvalidTransition);
        assert_eq!(error.current_state.as_deref(), Some("DISPATCH_UNKNOWN"));
    }

    #[test]
    fn legal_transition_increments_revision() {
        let current = Revisioned::new(WorkflowState::Ready, Revision::new(4));
        let next =
            WorkflowState::transition(current, WorkflowState::Running).expect("legal transition");
        assert_eq!(next.value, WorkflowState::Running);
        assert_eq!(next.revision, Revision::new(5));
    }
}
