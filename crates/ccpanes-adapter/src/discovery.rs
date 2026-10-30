use std::fs;
use std::net::SocketAddr;
use std::path::Path;

use coordination_core::{CoordinationError, ErrorCode};
use serde_json::Value;

use crate::{AdapterHeader, CcpanesEndpointConfig};

pub fn read_ccpanes_discovery_file(
    path: impl AsRef<Path>,
    discovery_generation: u64,
) -> Result<CcpanesEndpointConfig, CoordinationError> {
    let content = fs::read_to_string(path.as_ref()).map_err(|error| {
        CoordinationError::new(
            ErrorCode::AdapterNotReady,
            format!("read cc-panes discovery file failed: {error}"),
        )
    })?;
    let document: Value =
        serde_json::from_str(content.trim_start_matches('\u{feff}')).map_err(|error| {
            CoordinationError::new(
                ErrorCode::InvalidRequest,
                format!("decode cc-panes discovery JSON failed: {error}"),
            )
        })?;
    ccpanes_endpoint_from_discovery_value(&document, discovery_generation)
}

pub fn ccpanes_endpoint_from_discovery_value(
    document: &Value,
    discovery_generation: u64,
) -> Result<CcpanesEndpointConfig, CoordinationError> {
    if let Some(server) = document
        .get("mcpServers")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get("ccpanes"))
        .and_then(Value::as_object)
    {
        return endpoint_from_mcp_server(server, discovery_generation);
    }
    endpoint_from_v0123_daemon_manifest(document, discovery_generation)
}

fn endpoint_from_mcp_server(
    server: &serde_json::Map<String, Value>,
    discovery_generation: u64,
) -> Result<CcpanesEndpointConfig, CoordinationError> {
    let endpoint = server
        .get("url")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::AdapterNotReady,
                "cc-panes discovery URL is missing",
            )
        })?;
    let headers = server
        .get("headers")
        .and_then(Value::as_object)
        .map(|headers| {
            headers
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| AdapterHeader::new(name.clone(), value.to_owned()))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    CcpanesEndpointConfig::new(endpoint.to_owned(), headers, discovery_generation)
}

fn endpoint_from_v0123_daemon_manifest(
    document: &Value,
    discovery_generation: u64,
) -> Result<CcpanesEndpointConfig, CoordinationError> {
    let address = document
        .get("addr")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<SocketAddr>().ok())
        .filter(|value| value.ip().is_loopback())
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::AdapterNotReady,
                "cc-panes discovery must contain mcpServers.ccpanes or a loopback daemon addr",
            )
        })?;
    let token = document
        .get("token")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && !value.chars().any(char::is_control))
        .ok_or_else(|| {
            CoordinationError::new(
                ErrorCode::AdapterNotReady,
                "cc-panes daemon discovery token is missing",
            )
        })?;
    let host = if address.is_ipv6() {
        format!("[{}]", address.ip())
    } else {
        address.ip().to_string()
    };
    CcpanesEndpointConfig::new(
        format!("http://{host}:{}/mcp", address.port()),
        vec![AdapterHeader::new(
            "Authorization",
            format!("Bearer {token}"),
        )?],
        discovery_generation,
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn discovery_value_extracts_endpoint_and_headers() {
        let config = ccpanes_endpoint_from_discovery_value(
            &json!({
                "mcpServers": {
                    "ccpanes": {
                        "url": "http://127.0.0.1:39000/mcp",
                        "headers": {"Authorization": "Bearer TOKEN"}
                    }
                }
            }),
            7,
        )
        .unwrap();
        assert_eq!(config.endpoint, "http://127.0.0.1:39000/mcp");
        assert_eq!(config.headers[0].name, "Authorization");
        assert_eq!(config.discovery_generation, 7);
    }

    #[test]
    fn discovery_missing_server_fails_closed() {
        let error = ccpanes_endpoint_from_discovery_value(&json!({}), 1).expect_err("missing");
        assert_eq!(error.code, ErrorCode::AdapterNotReady);
    }

    #[test]
    fn v0123_daemon_manifest_extracts_loopback_endpoint_and_bearer_token() {
        let config = ccpanes_endpoint_from_discovery_value(
            &json!({
                "addr": "127.0.0.1:62674",
                "token": "TOKEN_PLACEHOLDER",
                "pid": 14932,
                "startedAt": 1786416024555_i64
            }),
            9,
        )
        .unwrap();

        assert_eq!(config.endpoint, "http://127.0.0.1:62674/mcp");
        assert_eq!(config.headers.len(), 1);
        assert_eq!(config.headers[0].name, "Authorization");
        assert_eq!(config.headers[0].value, "Bearer TOKEN_PLACEHOLDER");
        assert_eq!(config.discovery_generation, 9);
    }

    #[test]
    fn daemon_manifest_requires_loopback_addr_and_nonempty_token() {
        for document in [
            json!({"addr": "0.0.0.0:62674", "token": "TOKEN"}),
            json!({"addr": "example.com:62674", "token": "TOKEN"}),
            json!({"addr": "127.0.0.1:62674", "token": ""}),
        ] {
            let error = ccpanes_endpoint_from_discovery_value(&document, 1)
                .expect_err("unsafe daemon manifest must fail closed");
            assert_eq!(error.code, ErrorCode::AdapterNotReady);
        }
    }
}
