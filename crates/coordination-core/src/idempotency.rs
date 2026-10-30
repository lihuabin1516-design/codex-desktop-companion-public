use serde::{Deserialize, Serialize};

use crate::{CallerId, CoordinationError, ErrorCode, RequestId};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IdempotencyKey {
    pub tool: String,
    pub caller_id: CallerId,
    pub request_id: RequestId,
}

impl IdempotencyKey {
    pub fn try_new(
        tool: impl Into<String>,
        caller_id: CallerId,
        request_id: RequestId,
    ) -> Result<Self, CoordinationError> {
        let tool = tool.into();
        if tool.trim().is_empty() {
            return Err(CoordinationError::new(
                ErrorCode::InvalidRequest,
                "tool must not be empty",
            ));
        }
        Ok(Self {
            tool,
            caller_id,
            request_id,
        })
    }

    pub fn duplicate_of(&self, other: &Self) -> bool {
        self == other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_request_scope_is_tool_caller_and_request_id() {
        let caller = CallerId::try_new("codex-app").unwrap();
        let request = RequestId::try_new("request-1").unwrap();
        let first = IdempotencyKey::try_new("worker_dispatch", caller.clone(), request.clone())
            .expect("first key");
        let replay = IdempotencyKey::try_new("worker_dispatch", caller, request).expect("replay");
        assert!(first.duplicate_of(&replay));

        let different_tool = IdempotencyKey::try_new(
            "workflow_create",
            CallerId::try_new("codex-app").unwrap(),
            RequestId::try_new("request-1").unwrap(),
        )
        .expect("other tool");
        assert!(!first.duplicate_of(&different_tool));
    }
}
