//! HTTP MCP adapter contract for official CC-Panes.
//!
//! This crate owns the Phase 4 adapter seam: explicit discovery input,
//! authentication header injection, MCP initialize/tools negotiation, bounded
//! HTTP/SSE request handling, tool result translation, and unknown dispatch
//! reconciliation from public CC-Panes session/pane listings. It does not own
//! Workflow state, retries, scheduling, MCP stdio, Launcher behavior, or real
//! CC-Panes database/session internals.

pub mod capability;
pub mod client;
pub mod discovery;
pub mod fake;
pub mod http;
pub mod preflight;
pub mod reconcile;
pub mod redact;
pub mod types;
pub mod workspace;

pub use capability::*;
pub use client::*;
pub use discovery::*;
pub use fake::*;
pub use http::*;
pub use preflight::*;
pub use reconcile::*;
pub use redact::*;
pub use types::*;
pub use workspace::*;

pub const CCPANES_ADAPTER_CONTRACT_VERSION: &str = "ccpanes-adapter/v1beta1";
pub const CCPANES_HTTP_MCP_ADAPTER_NAME: &str = "cc-panes-http-mcp";
pub const CCPANES_MCP_PROTOCOL_VERSION: &str = "2025-03-26";
