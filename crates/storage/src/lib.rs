//! Durable control-plane state. PostgreSQL is the production backend; the
//! in-memory implementation keeps local development dependency-free.

mod memory;
mod postgres;

use std::sync::Arc;

use async_trait::async_trait;
use sandbox_core::{
    NodeId, OperationId, SandboxId,
    api::{CompleteAssignmentRequest, HeartbeatRequest},
    config::{StoreConfig, StoreKind},
    model::{Assignment, AssignmentKind, NodeRecord, Operation, Sandbox, Tunnel, TunnelState},
};
use secrecy::ExposeSecret;
use thiserror::Error;

pub use memory::MemoryStore;
pub use postgres::PgStore;

pub type StoreRef = Arc<dyn Store>;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("record not found: {0}")]
    NotFound(String),
    #[error("store conflict: {0}")]
    Conflict(String),
    #[error("store backend error: {0}")]
    Backend(String),
    #[error("store serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("missing database_url for postgres store")]
    MissingDatabaseUrl,
}

pub type StoreResult<T> = Result<T, StoreError>;

#[async_trait]
pub trait Store: Send + Sync {
    fn backend_name(&self) -> &'static str;
    async fn upsert_node(&self, node: NodeRecord) -> StoreResult<()>;
    async fn heartbeat_node(&self, id: NodeId, heartbeat: HeartbeatRequest) -> StoreResult<()>;
    async fn list_nodes(&self) -> StoreResult<Vec<NodeRecord>>;
    async fn create_sandbox(
        &self,
        sandbox: Sandbox,
        assignment: Assignment,
        operation: Operation,
    ) -> StoreResult<()>;
    async fn get_sandbox(&self, id: SandboxId) -> StoreResult<Sandbox>;
    async fn update_sandbox(&self, sandbox: Sandbox) -> StoreResult<()>;
    async fn list_sandboxes(&self, tenant: Option<&str>) -> StoreResult<Vec<Sandbox>>;
    async fn find_tunnel_by_hostname(&self, hostname: &str) -> StoreResult<Option<Tunnel>>;
    async fn create_assignment(
        &self,
        assignment: Assignment,
        operation: Operation,
    ) -> StoreResult<()>;
    async fn lease_assignments(
        &self,
        node_id: NodeId,
        limit: usize,
        lease_seconds: i64,
    ) -> StoreResult<Vec<Assignment>>;
    async fn complete_assignment(&self, request: CompleteAssignmentRequest) -> StoreResult<()>;
    async fn get_operation(&self, id: OperationId) -> StoreResult<Operation>;
}

fn apply_tunnel_completion(
    sandbox: &mut Sandbox,
    kind: &AssignmentKind,
    success: bool,
    error: Option<&str>,
) {
    match kind {
        AssignmentKind::Create { tunnels, .. } => {
            for tunnel in tunnels {
                if let Some(record) = sandbox
                    .tunnels
                    .iter_mut()
                    .find(|record| record.id == tunnel.id)
                {
                    record.state = if success {
                        TunnelState::Active
                    } else {
                        TunnelState::Failed
                    };
                    record.failure = (!success).then(|| {
                        error
                            .unwrap_or("sandbox creation failed before tunnel activation")
                            .to_owned()
                    });
                }
            }
        }
        AssignmentKind::Expose { tunnel } => {
            if let Some(record) = sandbox
                .tunnels
                .iter_mut()
                .find(|record| record.id == tunnel.id)
            {
                record.state = if success {
                    TunnelState::Active
                } else {
                    TunnelState::Failed
                };
                record.failure =
                    (!success).then(|| error.unwrap_or("tunnel activation failed").to_owned());
            }
        }
        AssignmentKind::Unexpose { tunnel } if success => {
            sandbox.tunnels.retain(|record| record.id != tunnel.id);
            sandbox.spec.exposures.retain(|exposure| {
                exposure.container_port != tunnel.container_port
                    || exposure.subdomain.as_deref() != Some(tunnel.subdomain.as_str())
            });
        }
        AssignmentKind::Unexpose { tunnel } => {
            if let Some(record) = sandbox
                .tunnels
                .iter_mut()
                .find(|record| record.id == tunnel.id)
            {
                record.state = TunnelState::Failed;
                record.failure = Some(error.unwrap_or("tunnel removal failed").to_owned());
            }
        }
        AssignmentKind::Delete if success => {
            sandbox.tunnels.clear();
            sandbox.spec.exposures.clear();
        }
        AssignmentKind::Exec { .. } | AssignmentKind::Delete => {}
    }
}

pub async fn connect(config: &StoreConfig) -> StoreResult<StoreRef> {
    match config.kind {
        StoreKind::Memory => Ok(Arc::new(MemoryStore::default())),
        StoreKind::Postgres => {
            let url = config
                .database_url
                .as_ref()
                .ok_or(StoreError::MissingDatabaseUrl)?;
            let store = PgStore::connect(url.expose_secret(), config.max_connections).await?;
            store.migrate().await?;
            Ok(Arc::new(store))
        }
    }
}
