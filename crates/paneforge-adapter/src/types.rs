use std::time::Duration;

use coordination_core::{CoordinationError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::redact::{digest_header_values, redact_endpoint_for_report, redact_sensitive_text};

pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 2_000;
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 8_000;
pub const DEFAULT_RECONCILE_WINDOW_MS: i64 = 10 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AdapterHeader {
    pub name: String,
    pub value: String,
}

impl AdapterHeader {
    pub fn new(
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, CoordinationError> {
        let header = Self {
            name: name.into(),
            value: value.into(),
        };
        header.validate()?;
        Ok(header)
    }

    pub fn validate(&self) -> Result<(), CoordinationError> {
        if self.name.trim().is_empty() {
            return Err(CoordinationError::invalid_request(
                "header name must not be empty",
            ));
        }
        if self
            .name
            .bytes()
            .any(|byte| byte <= 0x20 || byte >= 0x7f || byte == b':')
        {
            return Err(CoordinationError::invalid_request(
                "header name contains invalid characters",
            ));
        }
        if self.value.chars().any(|ch| ch == '\r' || ch == '\n') {
            return Err(CoordinationError::invalid_request(
                "header value contains invalid line break",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PaneForgeEndpointConfig {
    pub endpoint: String,
    #[serde(default)]
    pub headers: Vec<AdapterHeader>,
    pub discovery_generation: u64,
}

impl PaneForgeEndpointConfig {
    pub fn new(
        endpoint: impl Into<String>,
        headers: Vec<AdapterHeader>,
        discovery_generation: u64,
    ) -> Result<Self, CoordinationError> {
        let config = Self {
            endpoint: endpoint.into(),
            headers,
            discovery_generation,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), CoordinationError> {
        if self.endpoint.trim().is_empty() {
            return Err(CoordinationError::invalid_request(
                "paneforge MCP endpoint must not be empty",
            ));
        }
        for header in &self.headers {
            header.validate()?;
        }
        Ok(())
    }

    pub fn sanitized(&self) -> SanitizedEndpointConfig {
        SanitizedEndpointConfig {
            endpoint: redact_endpoint_for_report(&self.endpoint),
            header_names: self
                .headers
                .iter()
                .map(|header| header.name.clone())
                .collect(),
            header_value_digest: digest_header_values(&self.headers),
            discovery_generation: self.discovery_generation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SanitizedEndpointConfig {
    pub endpoint: String,
    pub header_names: Vec<String>,
    pub header_value_digest: String,
    pub discovery_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterTimeouts {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
}

impl AdapterTimeouts {
    pub const fn new(connect_timeout: Duration, request_timeout: Duration) -> Self {
        Self {
            connect_timeout,
            request_timeout,
        }
    }

    pub fn validate(&self) -> Result<(), CoordinationError> {
        if self.connect_timeout.is_zero() || self.request_timeout.is_zero() {
            return Err(CoordinationError::invalid_request(
                "adapter timeouts must be greater than zero",
            ));
        }
        Ok(())
    }
}

impl Default for AdapterTimeouts {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_millis(DEFAULT_CONNECT_TIMEOUT_MS),
            request_timeout: Duration::from_millis(DEFAULT_REQUEST_TIMEOUT_MS),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AdapterReadiness {
    Ready,
    Degraded,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CapabilitySnapshot {
    pub readiness: AdapterReadiness,
    pub server_version: Option<String>,
    pub protocol_version: Option<String>,
    pub tools: Vec<String>,
    pub capability_digest: String,
    pub launch_task_properties: Vec<String>,
    pub launch_task_accepts_client_request_id: bool,
    #[serde(default)]
    pub dispatch_ready: bool,
    #[serde(default)]
    pub get_session_status_available: bool,
    #[serde(default)]
    pub get_session_output_available: bool,
    #[serde(default)]
    pub submit_to_session_available: bool,
    #[serde(default)]
    pub kill_session_available: bool,
    #[serde(default)]
    pub wait_for_session_available: bool,
    #[serde(default)]
    pub write_to_session_available: bool,
    pub kill_session_accepts_reason: bool,
}

impl CapabilitySnapshot {
    pub fn supports_tool(&self, tool_name: &str) -> bool {
        self.tools.iter().any(|tool| tool == tool_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LaunchTaskResult {
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    pub binding_id: Option<String>,
    pub runtime_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub session_id: String,
    pub status: String,
    pub terminal: bool,
    pub succeeded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum KillSessionOutcome {
    Accepted,
    AlreadyAbsent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DiscoveredSessionBinding {
    pub task_id: Option<String>,
    pub session_id: String,
    pub binding_id: Option<String>,
    pub match_basis: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AdapterStatusSnapshot {
    pub contract_version: String,
    pub readiness: AdapterReadiness,
    pub endpoint: SanitizedEndpointConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<CapabilitySnapshot>,
}

pub(crate) fn find_string(value: &Value, names: &[&str]) -> Option<String> {
    for name in names {
        if let Some(found) = value.get(*name).and_then(Value::as_str) {
            return Some(found.to_owned());
        }
        if let Some(found) = value
            .get(*name)
            .and_then(Value::as_object)
            .and_then(|nested| nested.get("id"))
            .and_then(Value::as_str)
        {
            return Some(found.to_owned());
        }
    }
    value
        .as_object()
        .into_iter()
        .flat_map(Map::values)
        .find_map(|nested| {
            nested
                .is_object()
                .then(|| find_string(nested, names))
                .flatten()
        })
}

pub(crate) fn safe_adapter_error(code: ErrorCode, message: impl AsRef<str>) -> CoordinationError {
    CoordinationError::new(code, redact_sensitive_text(message.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_endpoint_does_not_expose_query_token() {
        let endpoint = format!("http://127.0.0.1:39000/mcp?{}=TOKEN_PLACEHOLDER", "token");
        let config = PaneForgeEndpointConfig::new(
            endpoint,
            vec![AdapterHeader::new("Authorization", "Bearer TOKEN_PLACEHOLDER").unwrap()],
            3,
        )
        .unwrap();

        let sanitized = config.sanitized();

        assert_eq!(sanitized.endpoint, "http://127.0.0.1:39000/mcp?<redacted>");
        assert!(!sanitized.endpoint.contains("TOKEN_PLACEHOLDER"));
        assert_eq!(sanitized.header_names, vec!["Authorization"]);
        assert_ne!(sanitized.header_value_digest, "TOKEN_PLACEHOLDER");
        assert_eq!(sanitized.discovery_generation, 3);
    }
}
