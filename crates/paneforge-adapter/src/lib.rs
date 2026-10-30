//! HTTP MCP adapter contract for official PaneForge.
//!
//! This crate owns the Phase 4 adapter seam: explicit discovery input,
//! authentication header injection, MCP initialize/tools negotiation, bounded
//! HTTP/SSE request handling, tool result translation, and unknown dispatch
//! reconciliation from public PaneForge session/pane listings. It does not own
//! Workflow state, retries, scheduling, MCP stdio, Launcher behavior, or real
//! PaneForge database/session internals.

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

pub const PANEFORGE_ADAPTER_CONTRACT_VERSION: &str = "paneforge-adapter/v1beta1";
pub const PANEFORGE_HTTP_MCP_ADAPTER_NAME: &str = "paneforge-http-mcp";
pub const PANEFORGE_MCP_PROTOCOL_VERSION: &str = "2025-03-26";
