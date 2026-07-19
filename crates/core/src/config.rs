use std::{collections::BTreeMap, net::SocketAddr, path::Path};

use secrecy::SecretString;
use serde::Deserialize;
use url::Url;

use crate::{CoreResult, model::ResourceSpec};

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SandboxConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub store: StoreConfig,
    #[serde(default)]
    pub bus: BusConfig,
    #[serde(default)]
    pub node: NodeConfig,
    #[serde(default)]
    pub policy: PolicyConfig,
}

impl SandboxConfig {
    pub fn load(path: Option<&Path>) -> CoreResult<Self> {
        let mut builder = config::Config::builder();
        if let Some(path) = path {
            builder = builder.add_source(config::File::from(path).required(true));
        } else {
            builder = builder.add_source(config::File::with_name("sandbox.toml").required(false));
        }
        builder
            .add_source(
                config::Environment::with_prefix("SANDBOX")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()?
            .try_deserialize()
            .map_err(Into::into)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,
    pub api_token: Option<SecretString>,
    pub node_token: Option<SecretString>,
    #[serde(default)]
    pub allow_unauthenticated_dev: bool,
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_seconds: u64,
    #[serde(default = "default_heartbeat_timeout")]
    pub heartbeat_timeout_seconds: i64,
    #[serde(default = "default_assignment_lease")]
    pub assignment_lease_seconds: i64,
    #[serde(default = "default_body_limit")]
    pub request_body_limit_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            api_token: None,
            node_token: None,
            allow_unauthenticated_dev: false,
            heartbeat_interval_seconds: default_heartbeat_interval(),
            heartbeat_timeout_seconds: default_heartbeat_timeout(),
            assignment_lease_seconds: default_assignment_lease(),
            request_body_limit_bytes: default_body_limit(),
        }
    }
}

fn default_bind() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 8080))
}
const fn default_heartbeat_interval() -> u64 {
    10
}
const fn default_heartbeat_timeout() -> i64 {
    45
}
const fn default_assignment_lease() -> i64 {
    30
}
const fn default_body_limit() -> usize {
    1_048_576
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreKind {
    #[default]
    Memory,
    Postgres,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct StoreConfig {
    #[serde(default)]
    pub kind: StoreKind,
    pub database_url: Option<SecretString>,
    #[serde(default = "default_pool_size")]
    pub max_connections: u32,
}

const fn default_pool_size() -> u32 {
    16
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusKind {
    #[default]
    Memory,
    Nats,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BusConfig {
    #[serde(default)]
    pub kind: BusKind,
    #[serde(default = "default_nats_url")]
    pub nats_url: String,
    #[serde(default = "default_event_subject")]
    pub subject: String,
}

impl Default for BusConfig {
    fn default() -> Self {
        Self {
            kind: BusKind::Memory,
            nats_url: default_nats_url(),
            subject: default_event_subject(),
        }
    }
}

fn default_nats_url() -> String {
    "nats://127.0.0.1:4222".into()
}
fn default_event_subject() -> String {
    "sandbox.events".into()
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    #[default]
    Docker,
    External,
}

#[derive(Clone, Debug, Deserialize)]
pub struct NodeConfig {
    #[serde(default = "default_control_plane_url")]
    pub control_plane_url: Url,
    pub token: Option<SecretString>,
    #[serde(default = "default_node_name")]
    pub name: String,
    #[serde(default = "default_region")]
    pub region: String,
    #[serde(default = "default_zone")]
    pub zone: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub runtime: RuntimeKind,
    pub external_driver: Option<String>,
    #[serde(default = "default_restricted_network")]
    pub docker_restricted_network: String,
    #[serde(default = "default_node_resources")]
    pub resources: ResourceSpec,
    #[serde(default = "default_max_sandboxes")]
    pub max_sandboxes: u32,
    #[serde(default = "default_state_dir")]
    pub state_dir: String,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            control_plane_url: default_control_plane_url(),
            token: None,
            name: default_node_name(),
            region: default_region(),
            zone: default_zone(),
            labels: BTreeMap::new(),
            runtime: RuntimeKind::Docker,
            external_driver: None,
            docker_restricted_network: default_restricted_network(),
            resources: default_node_resources(),
            max_sandboxes: default_max_sandboxes(),
            state_dir: default_state_dir(),
        }
    }
}

fn default_control_plane_url() -> Url {
    Url::parse("http://127.0.0.1:8080").expect("constant URL is valid")
}
fn default_node_name() -> String {
    "sandboxd".into()
}
fn default_region() -> String {
    "local".into()
}
fn default_zone() -> String {
    "local-a".into()
}
fn default_state_dir() -> String {
    "/var/lib/sandbox".into()
}
fn default_restricted_network() -> String {
    "sandbox-egress".into()
}
const fn default_max_sandboxes() -> u32 {
    100
}
const fn default_node_resources() -> ResourceSpec {
    ResourceSpec {
        cpu_millis: 8_000,
        memory_mib: 16_384,
        disk_mib: 102_400,
        pids: 4_096,
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct PolicyConfig {
    #[serde(default = "default_max_ttl")]
    pub max_ttl_seconds: u64,
    #[serde(default = "default_microvm_threshold")]
    pub microvm_risk_threshold: u16,
    #[serde(default = "default_output_limit")]
    pub max_output_bytes: usize,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            max_ttl_seconds: default_max_ttl(),
            microvm_risk_threshold: default_microvm_threshold(),
            max_output_bytes: default_output_limit(),
        }
    }
}

const fn default_max_ttl() -> u64 {
    86_400
}
const fn default_microvm_threshold() -> u16 {
    55
}
const fn default_output_limit() -> usize {
    1_048_576
}
