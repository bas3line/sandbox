use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AssignmentId, NodeId, OperationId, SandboxId,
    model::{
        Assignment, CommandOutput, CommandSpec, ExposureProtocol, NodeRecord, Operation, Sandbox,
        SandboxSpec, SandboxState, Tunnel,
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
    #[serde(default)]
    pub tunnels: TunnelCapabilities,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TunnelCapabilities {
    pub enabled: bool,
    pub base_domain: Option<String>,
    pub public_scheme: Option<String>,
    #[serde(default)]
    pub protocols: Vec<ExposureProtocol>,
    #[serde(default)]
    pub local_relay_enabled: bool,
    pub local_relay_url: Option<String>,
}

/// Messages sent from the hosted relay to a connected `sandbox http` client.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalRelayServerMessage {
    Ready {
        public_url: String,
        hostname: String,
        expires_at: DateTime<Utc>,
    },
    HttpRequest {
        request_id: Uuid,
        method: String,
        uri: String,
        headers: Vec<(String, String)>,
        body_base64: String,
    },
    WebSocketOpen {
        request_id: Uuid,
        uri: String,
        headers: Vec<(String, String)>,
    },
    WebSocketFrame {
        request_id: Uuid,
        frame: RelayWebSocketFrame,
    },
    WebSocketClose {
        request_id: Uuid,
    },
    Error {
        message: String,
    },
}

/// Messages sent from the local CLI back to the hosted relay.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalRelayClientMessage {
    HttpResponse {
        request_id: Uuid,
        status: u16,
        headers: Vec<(String, String)>,
        body_base64: String,
    },
    WebSocketReady {
        request_id: Uuid,
        error: Option<String>,
    },
    WebSocketFrame {
        request_id: Uuid,
        frame: RelayWebSocketFrame,
    },
    WebSocketClosed {
        request_id: Uuid,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum RelayWebSocketFrame {
    Text(String),
    Binary(String),
    Ping(String),
    Pong(String),
    Close { code: Option<u16>, reason: String },
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
pub struct CreateTunnelRequest {
    pub container_port: u16,
    #[serde(default)]
    pub protocol: ExposureProtocol,
    pub subdomain: Option<String>,
    #[serde(default)]
    pub authenticated: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TunnelOperationResponse {
    pub tunnel: Tunnel,
    pub operation: Operation,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TunnelAuthorizationQuery {
    pub domain: String,
}
