use coordination_core::{CoordinationError, ErrorCode, Revision};

pub type StoreResult<T> = Result<T, CoordinationError>;

pub(crate) fn persistence_error(message: impl Into<String>) -> CoordinationError {
    CoordinationError::new(ErrorCode::PersistenceUnavailable, message).retryable(true)
}

pub(crate) fn invariant_error(message: impl Into<String>) -> CoordinationError {
    CoordinationError::new(ErrorCode::InternalInvariantViolation, message).retryable(false)
}

pub(crate) fn binding_conflict(message: impl Into<String>) -> CoordinationError {
    CoordinationError::new(ErrorCode::ExternalBindingConflict, message).retryable(false)
}

pub(crate) fn map_sql(error: rusqlite::Error) -> CoordinationError {
    persistence_error(format!("SQLite persistence operation failed: {error}"))
}

pub(crate) fn map_io(error: std::io::Error) -> CoordinationError {
    persistence_error(format!("filesystem persistence operation failed: {error}"))
}

pub(crate) fn map_json(error: serde_json::Error) -> CoordinationError {
    invariant_error(format!("stored JSON contract is invalid: {error}"))
}

pub(crate) fn to_i64(value: u64, name: &'static str) -> StoreResult<i64> {
    i64::try_from(value).map_err(|_| invariant_error(format!("{name} exceeds SQLite INTEGER")))
}

pub(crate) fn revision_from_i64(value: i64) -> StoreResult<Revision> {
    if value < 0 {
        Err(invariant_error("stored revision is negative"))
    } else {
        Ok(Revision::new(value as u64))
    }
}

pub(crate) trait SqlResultExt<T> {
    fn store_result(self) -> StoreResult<T>;
}

impl<T> SqlResultExt<T> for rusqlite::Result<T> {
    fn store_result(self) -> StoreResult<T> {
        self.map_err(map_sql)
    }
}
