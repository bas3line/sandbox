use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{AssignmentId, CoreError, CoreResult, NodeId, OperationId, SandboxId};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationPreference {
    #[default]
    Auto,
    Container,
    Microvm,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationTier {
    Container,
    Microvm,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataSensitivity {
    Public,
    #[default]
    Internal,
    Confidential,
    Restricted,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    #[default]
    Deny,
    RestrictedEgress,
    OpenEgress,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceSpec {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub disk_mib: u32,
    pub pids: u32,
}

impl Default for ResourceSpec {
    fn default() -> Self {
        Self {
            cpu_millis: 1_000,
            memory_mib: 1_024,
            disk_mib: 10_240,
            pids: 256,
        }
    }
}

impl ResourceSpec {
    #[must_use]
    pub fn fits_within(&self, available: &Self) -> bool {
        self.cpu_millis <= available.cpu_millis
            && self.memory_mib <= available.memory_mib
            && self.disk_mib <= available.disk_mib
            && self.pids <= available.pids
    }

    pub fn validate(&self) -> CoreResult<()> {
        if self.cpu_millis == 0 || self.memory_mib < 64 || self.disk_mib < 64 || self.pids == 0 {
            return Err(CoreError::Validation(
                "resources require cpu > 0, memory/disk >= 64 MiB, and pids > 0".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlacementConstraints {
    #[serde(default)]
    pub required_labels: BTreeMap<String, String>,
    pub preferred_region: Option<String>,
    #[serde(default)]
    pub anti_affinity_keys: BTreeSet<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortExposure {
    pub container_port: u16,
    pub protocol: ExposureProtocol,
    pub subdomain: Option<String>,
    #[serde(default)]
    pub authenticated: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposureProtocol {
    #[default]
    Http,
    Tcp,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkloadSignals {
    #[serde(default)]
    pub untrusted_repository: bool,
    #[serde(default)]
    pub executes_generated_code: bool,
    #[serde(default)]
    pub needs_secrets: bool,
    #[serde(default)]
    pub host_mounts: bool,
    #[serde(default)]
    pub privileged: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SandboxSpec {
    pub tenant: String,
    pub image: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub resources: ResourceSpec,
    #[serde(default)]
    pub network: NetworkMode,
    #[serde(default)]
    pub isolation: IsolationPreference,
    #[serde(default)]
    pub sensitivity: DataSensitivity,
    #[serde(default)]
    pub signals: WorkloadSignals,
    #[serde(default = "default_ttl_seconds")]
    pub ttl_seconds: u64,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub placement: PlacementConstraints,
    #[serde(default)]
    pub exposures: Vec<PortExposure>,
    pub agent: Option<String>,
}

const fn default_ttl_seconds() -> u64 {
    3_600
}

impl SandboxSpec {
    pub fn validate(&self) -> CoreResult<()> {
        self.resources.validate()?;
        if self.tenant.trim().is_empty() {
            return Err(CoreError::Validation("tenant cannot be empty".into()));
        }
        if self.image.trim().is_empty() || self.image.contains(char::is_whitespace) {
            return Err(CoreError::Validation(
                "image must be a non-empty OCI reference".into(),
            ));
        }
        if !(30..=604_800).contains(&self.ttl_seconds) {
            return Err(CoreError::Validation(
                "ttl_seconds must be between 30 seconds and 7 days".into(),
            ));
        }
        if self.signals.privileged {
            return Err(CoreError::Validation(
                "privileged sandboxes are never accepted through the public API".into(),
            ));
        }
        for key in self.env.keys() {
            let valid = !key.is_empty()
                && key
                    .chars()
                    .all(|character| character == '_' || character.is_ascii_alphanumeric());
            if !valid || key.starts_with(|character: char| character.is_ascii_digit()) {
                return Err(CoreError::Validation(format!(
                    "invalid environment key: {key}"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxState {
    Scheduled,
    Creating,
    Running,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Sandbox {
    pub id: SandboxId,
    pub spec: SandboxSpec,
    pub state: SandboxState,
    pub node_id: NodeId,
    pub isolation: IsolationTier,
    pub risk_score: u16,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NodeCapacity {
    pub total: ResourceSpec,
    pub available: ResourceSpec,
    #[serde(default)]
    pub max_sandboxes: u32,
    #[serde(default)]
    pub running_sandboxes: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NodeRecord {
    pub id: NodeId,
    pub name: String,
    pub region: String,
    pub zone: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub capacity: NodeCapacity,
    #[serde(default)]
    pub supported_tiers: BTreeSet<IsolationTier>,
    #[serde(default)]
    pub warm_images: BTreeSet<String>,
    #[serde(default)]
    pub pressure: f32,
    #[serde(default)]
    pub draining: bool,
    pub last_seen: DateTime<Utc>,
}

impl NodeRecord {
    #[must_use]
    pub fn is_healthy(&self, now: DateTime<Utc>, heartbeat_timeout_seconds: i64) -> bool {
        !self.draining
            && self.pressure.is_finite()
            && self.pressure < 1.0
            && now.signed_duration_since(self.last_seen).num_seconds() <= heartbeat_timeout_seconds
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandSpec {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_command_timeout")]
    pub timeout_seconds: u64,
}

const fn default_command_timeout() -> u64 {
    300
}

impl CommandSpec {
    pub fn validate(&self) -> CoreResult<()> {
        if self.argv.is_empty() || self.argv[0].trim().is_empty() {
            return Err(CoreError::Validation(
                "argv must contain an executable".into(),
            ));
        }
        if !(1..=86_400).contains(&self.timeout_seconds) {
            return Err(CoreError::Validation(
                "timeout_seconds must be between 1 second and 24 hours".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssignmentKind {
    Create {
        spec: SandboxSpec,
        isolation: IsolationTier,
    },
    Exec {
        command: CommandSpec,
    },
    Delete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentState {
    Pending,
    Leased,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Assignment {
    pub id: AssignmentId,
    pub operation_id: OperationId,
    pub sandbox_id: SandboxId,
    pub node_id: NodeId,
    pub kind: AssignmentKind,
    pub state: AssignmentState,
    pub attempt: u32,
    pub lease_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Pending,
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Operation {
    pub id: OperationId,
    pub sandbox_id: SandboxId,
    pub state: OperationState,
    pub output: Option<CommandOutput>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LifecycleEvent {
    pub event_id: uuid::Uuid,
    pub event_type: String,
    pub tenant: String,
    pub sandbox_id: SandboxId,
    pub node_id: Option<NodeId>,
    pub timestamp: DateTime<Utc>,
    pub attributes: BTreeMap<String, String>,
}
