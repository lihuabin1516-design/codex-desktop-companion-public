use std::fmt;

use serde::{Deserialize, Serialize};

use crate::Revision;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    SchemaVersionUnsupported,
    RevisionConflict,
    InvalidTransition,
    RepositoryBaselineChanged,
    AdapterNotReady,
    AdapterIncompatible,
    CapacityExhausted,
    BudgetExhausted,
    DispatchResultUnknown,
    ExternalBindingConflict,
    WorkerNotFound,
    WorkflowNotFound,
    CancelPending,
    ResultPending,
    PersistenceUnavailable,
    AuthorizationFailed,
    InternalInvariantViolation,
}

impl ErrorCode {
    pub const ALL: &'static [Self] = &[
        Self::InvalidRequest,
        Self::SchemaVersionUnsupported,
        Self::RevisionConflict,
        Self::InvalidTransition,
        Self::RepositoryBaselineChanged,
        Self::AdapterNotReady,
        Self::AdapterIncompatible,
        Self::CapacityExhausted,
        Self::BudgetExhausted,
        Self::DispatchResultUnknown,
        Self::ExternalBindingConflict,
        Self::WorkerNotFound,
        Self::WorkflowNotFound,
        Self::CancelPending,
        Self::ResultPending,
        Self::PersistenceUnavailable,
        Self::AuthorizationFailed,
        Self::InternalInvariantViolation,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::SchemaVersionUnsupported => "schema_version_unsupported",
            Self::RevisionConflict => "revision_conflict",
            Self::InvalidTransition => "invalid_transition",
            Self::RepositoryBaselineChanged => "repository_baseline_changed",
            Self::AdapterNotReady => "adapter_not_ready",
            Self::AdapterIncompatible => "adapter_incompatible",
            Self::CapacityExhausted => "capacity_exhausted",
            Self::BudgetExhausted => "budget_exhausted",
            Self::DispatchResultUnknown => "dispatch_result_unknown",
            Self::ExternalBindingConflict => "external_binding_conflict",
            Self::WorkerNotFound => "worker_not_found",
            Self::WorkflowNotFound => "workflow_not_found",
            Self::CancelPending => "cancel_pending",
            Self::ResultPending => "result_pending",
            Self::PersistenceUnavailable => "persistence_unavailable",
            Self::AuthorizationFailed => "authorization_failed",
            Self::InternalInvariantViolation => "internal_invariant_violation",
        }
    }

    pub const fn default_retryable(self) -> bool {
        matches!(
            self,
            Self::AdapterNotReady
                | Self::CapacityExhausted
                | Self::DispatchResultUnknown
                | Self::CancelPending
                | Self::ResultPending
                | Self::PersistenceUnavailable
        )
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoordinationError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<Revision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl CoordinationError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: code.default_retryable(),
            correlation_id: None,
            current_revision: None,
            current_state: None,
            retry_after_ms: None,
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message)
    }

    pub fn schema_version_unsupported(actual: impl AsRef<str>, expected: impl AsRef<str>) -> Self {
        Self::new(
            ErrorCode::SchemaVersionUnsupported,
            format!(
                "schema version {} is outside supported contract {}",
                actual.as_ref(),
                expected.as_ref()
            ),
        )
    }

    pub fn revision_conflict(expected: Revision, current: Revision) -> Self {
        Self::new(
            ErrorCode::RevisionConflict,
            format!(
                "expected revision {} but current revision is {}",
                expected.get(),
                current.get()
            ),
        )
        .with_current_revision(current)
    }

    pub fn invalid_transition(
        entity: &'static str,
        from: impl AsRef<str>,
        to: impl AsRef<str>,
    ) -> Self {
        Self::new(
            ErrorCode::InvalidTransition,
            format!(
                "invalid {entity} state transition: {} -> {}",
                from.as_ref(),
                to.as_ref()
            ),
        )
        .with_current_state(from.as_ref())
    }

    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }

    pub fn with_current_revision(mut self, current_revision: Revision) -> Self {
        self.current_revision = Some(current_revision);
        self
    }

    pub fn with_current_state(mut self, current_state: impl Into<String>) -> Self {
        self.current_state = Some(current_state.into());
        self
    }

    pub fn with_retry_after_ms(mut self, retry_after_ms: u64) -> Self {
        self.retry_after_ms = Some(retry_after_ms);
        self
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }
}

impl fmt::Display for CoordinationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CoordinationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_error_code_snapshot_matches_authority_contract() {
        let codes = ErrorCode::ALL
            .iter()
            .map(|code| code.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            codes,
            vec![
                "invalid_request",
                "schema_version_unsupported",
                "revision_conflict",
                "invalid_transition",
                "repository_baseline_changed",
                "adapter_not_ready",
                "adapter_incompatible",
                "capacity_exhausted",
                "budget_exhausted",
                "dispatch_result_unknown",
                "external_binding_conflict",
                "worker_not_found",
                "workflow_not_found",
                "cancel_pending",
                "result_pending",
                "persistence_unavailable",
                "authorization_failed",
                "internal_invariant_violation",
            ]
        );
    }

    #[test]
    fn structured_error_carries_revision_and_retry_contract() {
        let error = CoordinationError::revision_conflict(Revision::new(2), Revision::new(3));
        assert_eq!(error.code, ErrorCode::RevisionConflict);
        assert_eq!(error.current_revision, Some(Revision::new(3)));
        assert!(!error.retryable);

        let retry = CoordinationError::new(ErrorCode::ResultPending, "result is pending")
            .with_retry_after_ms(250);
        assert!(retry.retryable);
        assert_eq!(retry.retry_after_ms, Some(250));
    }
}
