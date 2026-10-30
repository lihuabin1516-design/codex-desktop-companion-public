//! stdio MCP boundary for Codex App.
//!
//! This crate owns JSON-RPC/MCP request handling, public tool exposure, input
//! validation, public error mapping, and the local IPC client seam. Workflow,
//! Worker, Outbox, Event, and Result state remain owned by the Coordinator.

pub mod ipc_client;
pub mod jsonrpc;
pub mod server;
pub mod stdio;

pub use ipc_client::*;
pub use jsonrpc::*;
pub use server::*;
pub use stdio::*;

pub const COORDINATION_MCP_CONTRACT_VERSION: &str = "coordination-mcp/v1beta1";
pub const MCP_SERVER_NAME: &str = "codex-ccpanes-mcp";
