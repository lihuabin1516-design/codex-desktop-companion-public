//! SQLite persistence boundary for the Codex + PaneForge Coordination Plane.
//!
//! This crate owns durable Store, Event, Outbox, ExternalBinding, Lease,
//! Result, idempotency, migration, backup, and integrity mechanics. It depends
//! on `coordination-core` domain contracts and intentionally avoids MCP
//! transport, PaneForge adapter, Windows, process-control, and Launcher code.

mod error;
mod model;
mod schema;
mod store;

pub use error::*;
pub use model::*;
pub use schema::{STORE_SCHEMA_VERSION, STORE_SCHEMA_VERSION_TEXT};
pub use store::*;
