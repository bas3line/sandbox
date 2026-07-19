use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use sandbox_core::{
    AssignmentId, NodeId, OperationId, SandboxId,
    api::{CompleteAssignmentRequest, HeartbeatRequest},
    model::{Assignment, AssignmentState, NodeRecord, Operation, OperationState, Sandbox, Tunnel},
};
use tokio::sync::RwLock;

use crate::{Store, StoreError, StoreResult, apply_tunnel_completion};

#[derive(Default)]
struct State {
    nodes: HashMap<NodeId, NodeRecord>,
    sandboxes: HashMap<SandboxId, Sandbox>,
    assignments: HashMap<AssignmentId, Assignment>,
    operations: HashMap<OperationId, Operation>,
}

#[derive(Default)]
pub struct MemoryStore {
    state: RwLock<State>,
}

#[async_trait]
impl Store for MemoryStore {
    fn backend_name(&self) -> &'static str {
        "memory"
    }

    async fn upsert_node(&self, node: NodeRecord) -> StoreResult<()> {
        self.state.write().await.nodes.insert(node.id, node);
        Ok(())
    }

    async fn heartbeat_node(&self, id: NodeId, heartbeat: HeartbeatRequest) -> StoreResult<()> {
        let mut state = self.state.write().await;
        let node = state
            .nodes
            .get_mut(&id)
            .ok_or_else(|| StoreError::NotFound(format!("node {id}")))?;
        node.capacity = heartbeat.capacity;
        node.pressure = heartbeat.pressure;
        node.warm_images = heartbeat.warm_images;
        node.last_seen = Utc::now();
        Ok(())
    }

    async fn list_nodes(&self) -> StoreResult<Vec<NodeRecord>> {
        Ok(self.state.read().await.nodes.values().cloned().collect())
    }

    async fn create_sandbox(
        &self,
        sandbox: Sandbox,
        assignment: Assignment,
        operation: Operation,
    ) -> StoreResult<()> {
        let mut state = self.state.write().await;
        if state.sandboxes.contains_key(&sandbox.id) {
            return Err(StoreError::Conflict(format!("sandbox {}", sandbox.id)));
        }
        state.sandboxes.insert(sandbox.id, sandbox);
        state.assignments.insert(assignment.id, assignment);
        state.operations.insert(operation.id, operation);
        Ok(())
    }

    async fn get_sandbox(&self, id: SandboxId) -> StoreResult<Sandbox> {
        self.state
            .read()
            .await
            .sandboxes
            .get(&id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("sandbox {id}")))
    }

    async fn update_sandbox(&self, sandbox: Sandbox) -> StoreResult<()> {
        let mut state = self.state.write().await;
        if !state.sandboxes.contains_key(&sandbox.id) {
            return Err(StoreError::NotFound(format!("sandbox {}", sandbox.id)));
        }
        state.sandboxes.insert(sandbox.id, sandbox);
        Ok(())
    }

    async fn list_sandboxes(&self, tenant: Option<&str>) -> StoreResult<Vec<Sandbox>> {
        let mut result = self
            .state
            .read()
            .await
            .sandboxes
            .values()
            .filter(|sandbox| tenant.is_none_or(|value| sandbox.spec.tenant == value))
            .cloned()
            .collect::<Vec<_>>();
        result.sort_by_key(|sandbox| std::cmp::Reverse(sandbox.created_at));
        Ok(result)
    }

    async fn find_tunnel_by_hostname(&self, hostname: &str) -> StoreResult<Option<Tunnel>> {
        Ok(self
            .state
            .read()
            .await
            .sandboxes
            .values()
            .flat_map(|sandbox| sandbox.tunnels.iter())
            .find(|tunnel| {
                tunnel.hostname == hostname
                    && tunnel.state == sandbox_core::model::TunnelState::Active
            })
            .cloned())
    }

    async fn create_assignment(
        &self,
        assignment: Assignment,
        operation: Operation,
    ) -> StoreResult<()> {
        let mut state = self.state.write().await;
        if !state.sandboxes.contains_key(&assignment.sandbox_id) {
            return Err(StoreError::NotFound(format!(
                "sandbox {}",
                assignment.sandbox_id
            )));
        }
        state.assignments.insert(assignment.id, assignment);
        state.operations.insert(operation.id, operation);
        Ok(())
    }

    async fn lease_assignments(
        &self,
        node_id: NodeId,
        limit: usize,
        lease_seconds: i64,
    ) -> StoreResult<Vec<Assignment>> {
        let mut state = self.state.write().await;
        let now = Utc::now();
        let mut ids = state
            .assignments
            .values()
            .filter(|assignment| {
                assignment.node_id == node_id
                    && (assignment.state == AssignmentState::Pending
                        || (assignment.state == AssignmentState::Leased
                            && assignment.lease_until.is_some_and(|until| until <= now)))
            })
            .map(|assignment| assignment.id)
            .collect::<Vec<_>>();
        ids.sort();
        ids.truncate(limit.min(100));
        let mut leased = Vec::with_capacity(ids.len());
        for id in ids {
            let leased_assignment = if let Some(assignment) = state.assignments.get_mut(&id) {
                assignment.state = AssignmentState::Leased;
                assignment.attempt = assignment.attempt.saturating_add(1);
                assignment.lease_until = Some(now + Duration::seconds(lease_seconds));
                Some((assignment.clone(), assignment.operation_id))
            } else {
                None
            };
            if let Some((assignment, operation_id)) = leased_assignment {
                leased.push(assignment);
                if let Some(operation) = state.operations.get_mut(&operation_id) {
                    operation.state = OperationState::Running;
                    operation.updated_at = now;
                }
            }
        }
        Ok(leased)
    }

    async fn complete_assignment(&self, request: CompleteAssignmentRequest) -> StoreResult<()> {
        let mut state = self.state.write().await;
        let assignment = state
            .assignments
            .get_mut(&request.assignment_id)
            .ok_or_else(|| StoreError::NotFound(format!("assignment {}", request.assignment_id)))?;
        if assignment.operation_id != request.operation_id
            || assignment.sandbox_id != request.sandbox_id
        {
            return Err(StoreError::Conflict(
                "assignment completion identifiers do not match".into(),
            ));
        }
        let assignment_kind = assignment.kind.clone();
        assignment.state = if request.success {
            AssignmentState::Completed
        } else {
            AssignmentState::Failed
        };
        assignment.lease_until = None;
        let now = Utc::now();
        let operation = state
            .operations
            .get_mut(&request.operation_id)
            .ok_or_else(|| StoreError::NotFound(format!("operation {}", request.operation_id)))?;
        operation.state = if request.success {
            OperationState::Succeeded
        } else {
            OperationState::Failed
        };
        operation.output = request.output;
        operation.error = request.error.clone();
        operation.updated_at = now;
        let sandbox = state
            .sandboxes
            .get_mut(&request.sandbox_id)
            .ok_or_else(|| StoreError::NotFound(format!("sandbox {}", request.sandbox_id)))?;
        apply_tunnel_completion(
            sandbox,
            &assignment_kind,
            request.success,
            request.error.as_deref(),
        );
        sandbox.state = request.sandbox_state;
        sandbox.failure = request.error;
        sandbox.updated_at = now;
        Ok(())
    }

    async fn get_operation(&self, id: OperationId) -> StoreResult<Operation> {
        self.state
            .read()
            .await
            .operations
            .get(&id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("operation {id}")))
    }
}
