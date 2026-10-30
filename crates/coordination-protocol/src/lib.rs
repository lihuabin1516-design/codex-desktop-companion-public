//! Public MCP and local IPC contracts for the Coordination Plane.
//!
//! This crate defines schemas and DTOs only. Runtime transports, persistence,
//! PaneForge HTTP calls, and process control belong to later crates.

pub mod ipc;
pub mod mcp;
pub mod schema;
pub mod version;

pub use ipc::*;
pub use mcp::*;
pub use schema::*;
pub use version::*;
