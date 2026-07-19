//! Shared domain types and configuration for the Sandbox platform.

pub mod agent;
pub mod api;
pub mod config;
pub mod error;
pub mod id;
pub mod model;

pub use error::{CoreError, CoreResult};
pub use id::{AssignmentId, NodeId, OperationId, SandboxId};
