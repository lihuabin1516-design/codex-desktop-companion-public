//! Domain contracts for the Codex + CC-Panes Coordination Plane.
//!
//! This crate owns in-memory semantics only: identifiers, revisions, state
//! machines, commands, queries, events, error codes, and WorkflowSpec v1beta1.
//! It intentionally stays free of store, transport, adapter, Windows, and
//! launcher dependencies.

pub mod command;
pub mod errors;
pub mod event;
pub mod idempotency;
pub mod ids;
pub mod revision;
pub mod spec;
pub mod state;

pub use command::*;
pub use errors::*;
pub use event::*;
pub use idempotency::*;
pub use ids::*;
pub use revision::*;
pub use spec::*;
pub use state::*;

pub const COORDINATION_CORE_CONTRACT_VERSION: &str = "coordination-core/v1beta1";
