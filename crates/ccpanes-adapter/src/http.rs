use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::Value;

use crate::{AdapterHeader, AdapterTimeouts, CcpanesEndpointConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status_code: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportPhase {
    Connect,
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFailureKind {
    Timeout,
    Connection,
    InvalidEndpoint,
    InvalidResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpTransportFailure {
    pub phase: TransportPhase,
    pub kind: TransportFailureKind,
    pub message: String,
}

impl HttpTransportFailure {
    fn new(phase: TransportPhase, kind: TransportFailureKind, message: impl Into<String>) -> Self {
        Self {
            phase,
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct McpHttpClient {
    endpoint: ParsedHttpEndpoint,
    headers: Vec<AdapterHeader>,
    timeouts: AdapterTimeouts,
}

impl McpHttpClient {
    pub fn new(
        config: &CcpanesEndpointConfig,
        timeouts: AdapterTimeouts,
    ) -> Result<Self, HttpTransportFailure> {
        timeouts.validate().map_err(|error| {
            HttpTransportFailure::new(
                TransportPhase::Connect,
                TransportFailureKind::InvalidEndpoint,
                error.message,
            )
        })?;
        let endpoint = parse_loopback_http_endpoint(&config.endpoint)?;
        Ok(Self {
            endpoint,
            headers: config.headers.clone(),
            timeouts,
        })
    }

    pub fn post_json(
        &self,
        session_id: Option<&str>,
        body: &Value,
    ) -> Result<HttpResponse, HttpTransportFailure> {
        let body = serde_json::to_string(body).map_err(|error| {
            HttpTransportFailure::new(
                TransportPhase::Response,
                TransportFailureKind::InvalidResponse,
                format!("encode JSON request failed: {error}"),
            )
        })?;
        let address = self.endpoint.socket_addr(self.timeouts.connect_timeout)?;
        let mut stream = TcpStream::connect_timeout(&address, self.timeouts.connect_timeout)
            .map_err(|error| {
                HttpTransportFailure::new(
                    TransportPhase::Connect,
                    io_error_kind(error.kind()),
                    format!("connect to CC-Panes MCP endpoint failed: {error}"),
                )
            })?;
        stream
            .set_read_timeout(Some(self.timeouts.request_timeout))
            .map_err(|error| {
                HttpTransportFailure::new(
                    TransportPhase::Response,
                    TransportFailureKind::Connection,
                    format!("set read timeout failed: {error}"),
                )
            })?;
        stream
            .set_write_timeout(Some(self.timeouts.request_timeout))
            .map_err(|error| {
                HttpTransportFailure::new(
                    TransportPhase::Response,
                    TransportFailureKind::Connection,
                    format!("set write timeout failed: {error}"),
                )
            })?;

        let mut request = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.endpoint.path,
            self.endpoint.host_header,
            body.len()
        );
        if let Some(session_id) = session_id {
            request.push_str("mcp-session-id: ");
            request.push_str(session_id);
            request.push_str("\r\n");
        }
        for header in &self.headers {
            request.push_str(&header.name);
            request.push_str(": ");
            request.push_str(&header.value);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).map_err(|error| {
            HttpTransportFailure::new(
                TransportPhase::Response,
                io_error_kind(error.kind()),
                format!("write CC-Panes MCP request failed: {error}"),
            )
        })?;
        stream.write_all(body.as_bytes()).map_err(|error| {
            HttpTransportFailure::new(
                TransportPhase::Response,
                io_error_kind(error.kind()),
                format!("write CC-Panes MCP body failed: {error}"),
            )
        })?;
        stream.flush().map_err(|error| {
            HttpTransportFailure::new(
                TransportPhase::Response,
                io_error_kind(error.kind()),
                format!("flush CC-Panes MCP request failed: {error}"),
            )
        })?;

        let mut response = Vec::new();
        stream.read_to_end(&mut response).map_err(|error| {
            HttpTransportFailure::new(
                TransportPhase::Response,
                io_error_kind(error.kind()),
                format!("read CC-Panes MCP response failed: {error}"),
            )
        })?;
        parse_http_response(&response)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHttpEndpoint {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub host_header: String,
}

impl ParsedHttpEndpoint {
    fn socket_addr(&self, timeout: Duration) -> Result<SocketAddr, HttpTransportFailure> {
        let mut addrs = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|error| {
                HttpTransportFailure::new(
                    TransportPhase::Connect,
                    io_error_kind(error.kind()),
                    format!("resolve CC-Panes MCP endpoint failed: {error}"),
                )
            })?;
        addrs.next().ok_or_else(|| {
            HttpTransportFailure::new(
                TransportPhase::Connect,
                if timeout.is_zero() {
                    TransportFailureKind::Timeout
                } else {
                    TransportFailureKind::Connection
                },
                "CC-Panes MCP endpoint resolved to no address",
            )
        })
    }
}

pub fn parse_loopback_http_endpoint(
    endpoint: &str,
) -> Result<ParsedHttpEndpoint, HttpTransportFailure> {
    let endpoint = endpoint.trim();
    let Some(rest) = endpoint.strip_prefix("http://") else {
        return Err(HttpTransportFailure::new(
            TransportPhase::Connect,
            TransportFailureKind::InvalidEndpoint,
            "CC-Panes MCP endpoint must use http:// loopback",
        ));
    };
    let (authority, path) = rest
        .split_once('/')
        .map(|(authority, path)| (authority, format!("/{path}")))
        .unwrap_or((rest, "/".to_owned()));
    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        let (host, suffix) = stripped.split_once(']').ok_or_else(|| {
            HttpTransportFailure::new(
                TransportPhase::Connect,
                TransportFailureKind::InvalidEndpoint,
                "bracketed endpoint host is incomplete",
            )
        })?;
        let port = suffix
            .strip_prefix(':')
            .unwrap_or("80")
            .parse()
            .map_err(|_| {
                HttpTransportFailure::new(
                    TransportPhase::Connect,
                    TransportFailureKind::InvalidEndpoint,
                    "endpoint port is invalid",
                )
            })?;
        (host.to_owned(), port)
    } else {
        let (host, port) = authority.rsplit_once(':').unwrap_or((authority, "80"));
        let port = port.parse().map_err(|_| {
            HttpTransportFailure::new(
                TransportPhase::Connect,
                TransportFailureKind::InvalidEndpoint,
                "endpoint port is invalid",
            )
        })?;
        (host.to_owned(), port)
    };
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
        return Err(HttpTransportFailure::new(
            TransportPhase::Connect,
            TransportFailureKind::InvalidEndpoint,
            "CC-Panes MCP endpoint must be loopback",
        ));
    }
    let host_header = if authority.is_empty() {
        host.clone()
    } else {
        authority.to_owned()
    };
    Ok(ParsedHttpEndpoint {
        host,
        port,
        path,
        host_header,
    })
}

pub fn parse_http_response(bytes: &[u8]) -> Result<HttpResponse, HttpTransportFailure> {
    let text = String::from_utf8(bytes.to_vec()).map_err(|error| {
        HttpTransportFailure::new(
            TransportPhase::Response,
            TransportFailureKind::InvalidResponse,
            format!("CC-Panes MCP response was not UTF-8: {error}"),
        )
    })?;
    let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| {
        HttpTransportFailure::new(
            TransportPhase::Response,
            TransportFailureKind::InvalidResponse,
            "CC-Panes MCP response did not contain HTTP headers",
        )
    })?;
    let mut lines = head.lines();
    let status_line = lines.next().ok_or_else(|| {
        HttpTransportFailure::new(
            TransportPhase::Response,
            TransportFailureKind::InvalidResponse,
            "CC-Panes MCP response omitted status line",
        )
    })?;
    let mut parts = status_line.split_whitespace();
    let _http = parts.next();
    let status_code = parts
        .next()
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| {
            HttpTransportFailure::new(
                TransportPhase::Response,
                TransportFailureKind::InvalidResponse,
                "CC-Panes MCP response status code is invalid",
            )
        })?;
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    Ok(HttpResponse {
        status_code,
        headers,
        body: body.to_owned(),
    })
}

fn io_error_kind(kind: std::io::ErrorKind) -> TransportFailureKind {
    match kind {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            TransportFailureKind::Timeout
        }
        std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
            TransportFailureKind::InvalidResponse
        }
        _ => TransportFailureKind::Connection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_http_endpoint_parser_accepts_localhost_only() {
        assert!(parse_loopback_http_endpoint("http://127.0.0.1:38885/mcp").is_ok());
        assert!(parse_loopback_http_endpoint("http://localhost:38885/mcp").is_ok());
        assert!(parse_loopback_http_endpoint("http://[::1]:38885/mcp").is_ok());
        assert_eq!(
            parse_loopback_http_endpoint("https://cc-panes.example.test/mcp")
                .expect_err("not loopback")
                .kind,
            TransportFailureKind::InvalidEndpoint
        );
    }

    #[test]
    fn http_response_parser_extracts_headers_and_body() {
        let response = parse_http_response(
            b"HTTP/1.1 200 OK\r\nMcp-Session-Id: session-1\r\n\r\n{\"ok\":true}",
        )
        .unwrap();
        assert_eq!(response.status_code, 200);
        assert_eq!(response.header("mcp-session-id"), Some("session-1"));
        assert_eq!(response.body, "{\"ok\":true}");
    }
}
