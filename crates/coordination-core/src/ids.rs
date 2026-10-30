use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{CoordinationError, ErrorCode};

const MAX_ID_LEN: usize = 160;

fn validate_id(kind: &'static str, value: impl Into<String>) -> Result<String, CoordinationError> {
    let value = value.into();
    if value.trim().is_empty() {
        return Err(CoordinationError::new(
            ErrorCode::InvalidRequest,
            format!("{kind} must not be empty"),
        ));
    }
    if value.len() > MAX_ID_LEN {
        return Err(CoordinationError::new(
            ErrorCode::InvalidRequest,
            format!("{kind} exceeds {MAX_ID_LEN} bytes"),
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(CoordinationError::new(
            ErrorCode::InvalidRequest,
            format!("{kind} contains control characters"),
        ));
    }
    Ok(value)
}

macro_rules! opaque_id {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn try_new(value: impl Into<String>) -> Result<Self, CoordinationError> {
                validate_id($kind, value).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

opaque_id!(WorkflowId, "workflowId");
opaque_id!(WorkerId, "workerId");
opaque_id!(StepId, "stepId");
opaque_id!(OutboxId, "outboxId");
opaque_id!(EventId, "eventId");
opaque_id!(LeaseId, "leaseId");
opaque_id!(ExternalBindingId, "externalBindingId");
opaque_id!(RequestId, "requestId");
opaque_id!(CallerId, "callerId");
opaque_id!(CorrelationId, "correlationId");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_reject_empty_and_control_characters() {
        assert_eq!(
            WorkflowId::try_new("   ").expect_err("empty id").code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            RequestId::try_new("abc\n123")
                .expect_err("control char")
                .code,
            ErrorCode::InvalidRequest
        );
    }

    #[test]
    fn ids_preserve_exact_external_value_after_validation() {
        let id = WorkerId::try_new("worker-01").expect("valid id");
        assert_eq!(id.as_str(), "worker-01");
        assert_eq!(id.to_string(), "worker-01");
    }
}
