use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    AssignmentId, NodeId, OperationId, SandboxId,
    model::{
        Assignment, CommandOutput, CommandSpec, NodeRecord, Operation, Sandbox, SandboxSpec,
        SandboxState,
    },
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    pub request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub store: String,
    pub now: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateSandboxRequest {
    pub spec: SandboxSpec,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateSandboxResponse {
    pub sandbox: Sandbox,
    pub operation: Operation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecSandboxRequest {
    pub command: CommandSpec,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OperationResponse {
    pub operation: Operation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ListSandboxesResponse {
    pub sandboxes: Vec<Sandbox>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RegisterNodeRequest {
    pub node: NodeRecord,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RegisterNodeResponse {
    pub node_id: NodeId,
    pub heartbeat_interval_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HeartbeatRequest {
    pub capacity: crate::model::NodeCapacity,
    pub pressure: f32,
    pub warm_images: std::collections::BTreeSet<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LeaseAssignmentsResponse {
    pub assignments: Vec<Assignment>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CompleteAssignmentRequest {
    pub assignment_id: AssignmentId,
    pub operation_id: OperationId,
    pub sandbox_id: SandboxId,
    pub success: bool,
    pub sandbox_state: SandboxState,
    pub output: Option<CommandOutput>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ListQuery {
    pub tenant: Option<String>,
}
