//! Runtime boundary used by `sandboxd` workers.
//!
//! Docker is the built-in baseline. Strong multi-tenant isolation is connected
//! through the external driver protocol so Firecracker, Kata, gVisor, or a
//! company-specific runtime can be upgraded independently from the control plane.

mod docker;
mod external;
mod tunnel;

use std::sync::Arc;

use async_trait::async_trait;
use sandbox_core::{
    SandboxId,
    config::{NodeConfig, RuntimeKind, TunnelConfig},
    model::{CommandOutput, CommandSpec, IsolationTier, SandboxSpec, Tunnel},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use docker::DockerRuntime;
pub use external::ExternalRuntime;

pub type RuntimeRef = Arc<dyn SandboxRuntime>;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime command failed: {0}")]
    Command(String),
    #[error("runtime I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("runtime protocol failed: {0}")]
    Protocol(#[from] serde_json::Error),
    #[error("runtime operation timed out after {0} seconds")]
    Timeout(u64),
    #[error("external runtime driver is required when runtime=external")]
    MissingExternalDriver,
    #[error("isolation tier {0:?} is unsupported by this runtime")]
    UnsupportedTier(IsolationTier),
    #[error("HTTP tunnels are disabled on this worker")]
    TunnelsDisabled,
    #[error("runtime tunnel configuration failed: {0}")]
    Tunnel(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeCapabilities {
    pub name: String,
    pub version: String,
    pub tiers: Vec<IsolationTier>,
    #[serde(default)]
    pub supports_http_tunnels: bool,
}

#[async_trait]
pub trait SandboxRuntime: Send + Sync {
    async fn probe(&self) -> Result<RuntimeCapabilities, RuntimeError>;
    async fn create(
        &self,
        id: SandboxId,
        spec: &SandboxSpec,
        isolation: IsolationTier,
    ) -> Result<(), RuntimeError>;
    async fn exec(
        &self,
        id: SandboxId,
        command: &CommandSpec,
    ) -> Result<CommandOutput, RuntimeError>;
    async fn expose(&self, id: SandboxId, tunnel: &Tunnel) -> Result<(), RuntimeError>;
    async fn unexpose(&self, id: SandboxId, tunnel: &Tunnel) -> Result<(), RuntimeError>;
    async fn delete(&self, id: SandboxId) -> Result<(), RuntimeError>;
}

pub fn from_config(
    config: &NodeConfig,
    tunnel: &TunnelConfig,
    output_limit: usize,
) -> Result<RuntimeRef, RuntimeError> {
    match config.runtime {
        RuntimeKind::Docker => Ok(Arc::new(DockerRuntime::new(
            config.docker_restricted_network.clone(),
            tunnel.clone(),
            output_limit,
        ))),
        RuntimeKind::External => {
            let driver = config
                .external_driver
                .as_ref()
                .ok_or(RuntimeError::MissingExternalDriver)?;
            Ok(Arc::new(ExternalRuntime::new(driver, output_limit)))
        }
    }
}
