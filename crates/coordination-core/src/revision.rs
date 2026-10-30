use std::fmt;

use serde::{Deserialize, Serialize};

use crate::CoordinationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    pub const ZERO: Self = Self(0);
    pub const INITIAL: Self = Self(1);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

pub fn require_expected_revision(
    expected: Revision,
    current: Revision,
) -> Result<Revision, CoordinationError> {
    if expected == current {
        Ok(current.next())
    } else {
        Err(CoordinationError::revision_conflict(expected, current))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Revisioned<T> {
    pub value: T,
    pub revision: Revision,
}

impl<T> Revisioned<T> {
    pub const fn new(value: T, revision: Revision) -> Self {
        Self { value, revision }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;

    #[test]
    fn matching_revision_returns_next_revision() {
        assert_eq!(
            require_expected_revision(Revision::new(7), Revision::new(7)),
            Ok(Revision::new(8))
        );
    }

    #[test]
    fn stale_revision_returns_typed_conflict() {
        let error = require_expected_revision(Revision::new(6), Revision::new(7))
            .expect_err("stale revision");
        assert_eq!(error.code, ErrorCode::RevisionConflict);
        assert_eq!(error.current_revision, Some(Revision::new(7)));
    }
}
