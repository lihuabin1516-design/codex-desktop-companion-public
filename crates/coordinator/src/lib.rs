//! Coordinator runtime contract for the Codex + PaneForge Coordination Plane.
//!
//! This crate owns Phase 3 runtime semantics: single-instance identity, IPC
//! health handling, bounded scheduling, capacity/budget admission,
//! pause/resume/cancel orchestration, structured logs/metrics, and result
//! aggregation, repository baseline guards, Phase 7 synthetic Git canary
//! fixtures, and the explicit foreground PaneForge Adapter execution seam.

pub mod aggregation;
pub mod paneforge_operations;
pub mod config;
pub mod execution;
pub mod identity;
pub mod metrics;
pub mod queue;
pub mod real_canary;
pub mod repository;
pub mod runtime;
pub mod scheduler;
pub mod semaphore;
pub mod single_instance;

pub use aggregation::*;
pub use paneforge_operations::*;
pub use config::*;
pub use execution::*;
pub use identity::*;
pub use metrics::*;
pub use queue::*;
pub use real_canary::*;
pub use repository::*;
pub use runtime::*;
pub use scheduler::*;
pub use semaphore::*;
pub use single_instance::*;

pub const COORDINATOR_RUNTIME_CONTRACT_VERSION: &str = "coordination.coordinator-runtime/v1beta1";
