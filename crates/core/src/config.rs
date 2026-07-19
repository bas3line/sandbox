use std::{collections::BTreeMap, net::SocketAddr, path::Path};

use secrecy::SecretString;
use serde::Deserialize;
use url::Url;

use crate::{
    CoreError, CoreResult,
    model::{ResourceSpec, validate_dns_label},
};

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
    #[serde(default)]
    pub tunnel: TunnelConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TunnelConfig {
    #[serde(default)]
    pub enabled: bool,
    pub base_domain: Option<String>,
    #[serde(default = "default_public_scheme")]
    pub public_scheme: String,
    #[serde(default = "default_tunnel_network_prefix")]
    pub docker_network_prefix: String,
    #[serde(default = "default_tunnel_config_dir")]
    pub config_dir: String,
    #[serde(default = "default_tunnel_edge_container")]
    pub edge_container: String,
    #[serde(default = "default_tunnel_entrypoint")]
    pub edge_entrypoint: String,
    #[serde(default = "default_true")]
    pub edge_tls: bool,
    #[serde(default = "default_tunnel_cert_resolver")]
    pub edge_cert_resolver: Option<String>,
    #[serde(default = "default_max_tunnels")]
    pub max_per_sandbox: usize,
    /// Allow ephemeral tunnels from a developer workstation over the relay WebSocket.
    #[serde(default)]
    pub local_relay_enabled: bool,
    /// Require the operator bearer token before accepting a local relay connection.
    #[serde(default = "default_true")]
    pub local_relay_require_auth: bool,
    /// Internal URL used by the HTTP edge for exact-host local relay routes.
    #[serde(default = "default_local_relay_upstream")]
    pub local_relay_upstream: String,
    /// Maximum number of simultaneous local relay sessions on one controller.
    #[serde(default = "default_max_local_relays")]
    pub max_local_relays: usize,
    /// Maximum simultaneous local relays attributed to one client address.
    #[serde(default = "default_max_local_relays_per_client")]
    pub max_local_relays_per_client: usize,
    /// Maximum request or response body transported by the HTTP relay.
    #[serde(default = "default_local_relay_body_limit")]
    pub local_relay_body_limit_bytes: usize,
    /// Hard lifetime for an ephemeral local relay session.
    #[serde(default = "default_local_relay_ttl")]
    pub local_relay_ttl_seconds: u64,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_domain: None,
            public_scheme: default_public_scheme(),
            docker_network_prefix: default_tunnel_network_prefix(),
            config_dir: default_tunnel_config_dir(),
            edge_container: default_tunnel_edge_container(),
            edge_entrypoint: default_tunnel_entrypoint(),
            edge_tls: true,
            edge_cert_resolver: default_tunnel_cert_resolver(),
            max_per_sandbox: default_max_tunnels(),
            local_relay_enabled: false,
            local_relay_require_auth: true,
            local_relay_upstream: default_local_relay_upstream(),
            max_local_relays: default_max_local_relays(),
            max_local_relays_per_client: default_max_local_relays_per_client(),
            local_relay_body_limit_bytes: default_local_relay_body_limit(),
            local_relay_ttl_seconds: default_local_relay_ttl(),
        }
    }
}

impl TunnelConfig {
    pub fn validate(&self) -> CoreResult<()> {
        if !self.enabled {
            return Ok(());
        }
        let domain = self
            .base_domain
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CoreError::Validation(
                    "tunnel.base_domain is required when tunnels are enabled".into(),
                )
            })?;
        if domain.len() > 253
            || domain.starts_with('.')
            || domain.ends_with('.')
            || domain.contains('*')
            || domain.split('.').count() < 2
        {
            return Err(CoreError::Validation(
                "tunnel.base_domain must be a lowercase DNS name without a wildcard or trailing dot"
                    .into(),
            ));
        }
        for label in domain.split('.') {
            validate_dns_label(label)?;
        }
        if !matches!(self.public_scheme.as_str(), "http" | "https") {
            return Err(CoreError::Validation(
                "tunnel.public_scheme must be http or https".into(),
            ));
        }
        validate_config_token("tunnel.docker_network_prefix", &self.docker_network_prefix)?;
        validate_config_token("tunnel.edge_container", &self.edge_container)?;
        validate_config_token("tunnel.edge_entrypoint", &self.edge_entrypoint)?;
        if let Some(resolver) = &self.edge_cert_resolver {
            validate_config_token("tunnel.edge_cert_resolver", resolver)?;
        }
        if !self.config_dir.starts_with('/') || self.config_dir.contains('\0') {
            return Err(CoreError::Validation(
                "tunnel.config_dir must be an absolute path".into(),
            ));
        }
        if !(1..=32).contains(&self.max_per_sandbox) {
            return Err(CoreError::Validation(
                "tunnel.max_per_sandbox must be between 1 and 32".into(),
            ));
        }
        let relay_upstream = Url::parse(&self.local_relay_upstream).map_err(|_| {
            CoreError::Validation("tunnel.local_relay_upstream must be a valid HTTP URL".into())
        })?;
        if relay_upstream.scheme() != "http"
            || relay_upstream.host_str().is_none()
            || relay_upstream.cannot_be_a_base()
            || relay_upstream.username() != ""
            || relay_upstream.password().is_some()
        {
            return Err(CoreError::Validation(
                "tunnel.local_relay_upstream must be an internal HTTP URL without credentials"
                    .into(),
            ));
        }
        if !(1..=10_000).contains(&self.max_local_relays)
            || !(1..=100).contains(&self.max_local_relays_per_client)
        {
            return Err(CoreError::Validation(
                "local relay limits are outside the supported range".into(),
            ));
        }
        if !(65_536..=67_108_864).contains(&self.local_relay_body_limit_bytes) {
            return Err(CoreError::Validation(
                "tunnel.local_relay_body_limit_bytes must be between 64 KiB and 64 MiB".into(),
            ));
        }
        if !(60..=86_400).contains(&self.local_relay_ttl_seconds) {
            return Err(CoreError::Validation(
                "tunnel.local_relay_ttl_seconds must be between 60 seconds and 24 hours".into(),
            ));
        }
        Ok(())
    }

    pub fn base_domain(&self) -> CoreResult<&str> {
        self.base_domain
            .as_deref()
            .ok_or_else(|| CoreError::Validation("tunnel.base_domain is not configured".into()))
    }
}

fn validate_config_token(name: &str, value: &str) -> CoreResult<()> {
    let maximum = if name == "tunnel.docker_network_prefix" {
        40
    } else {
        128
    };
    let valid = !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(CoreError::Validation(format!(
            "{name} must use only letters, digits, dots, underscores, and hyphens"
        )))
    }
}

fn default_public_scheme() -> String {
    "https".into()
}
fn default_tunnel_network_prefix() -> String {
    "sandbox-tunnel".into()
}
fn default_tunnel_config_dir() -> String {
    "/var/lib/sandbox/tunnels".into()
}
fn default_tunnel_edge_container() -> String {
    "sandbox-tunnel-edge".into()
}
fn default_tunnel_entrypoint() -> String {
    "websecure".into()
}
fn default_tunnel_cert_resolver() -> Option<String> {
    Some("letsencrypt".into())
}
fn default_local_relay_upstream() -> String {
    "http://controller:8080".into()
}
const fn default_true() -> bool {
    true
}
const fn default_max_tunnels() -> usize {
    8
}
const fn default_max_local_relays() -> usize {
    100
}
const fn default_max_local_relays_per_client() -> usize {
    3
}
const fn default_local_relay_body_limit() -> usize {
    16 * 1_048_576
}
const fn default_local_relay_ttl() -> u64 {
    14_400
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

#[cfg(test)]
mod tests {
    use super::TunnelConfig;

    #[test]
    fn disabled_tunnels_do_not_require_a_domain() {
        assert!(TunnelConfig::default().validate().is_ok());
    }

    #[test]
    fn enabled_tunnels_require_a_safe_lowercase_domain() {
        let valid = TunnelConfig {
            enabled: true,
            base_domain: Some("tunnel.example.com".into()),
            ..TunnelConfig::default()
        };
        assert!(valid.validate().is_ok());

        for domain in [
            "*.example.com",
            "Tunnel.example.com",
            "example.com.",
            "localhost",
        ] {
            let invalid = TunnelConfig {
                enabled: true,
                base_domain: Some(domain.into()),
                ..TunnelConfig::default()
            };
            assert!(invalid.validate().is_err(), "{domain}");
        }
    }

    #[test]
    fn tunnel_network_prefix_leaves_room_for_a_uuid() {
        let invalid = TunnelConfig {
            enabled: true,
            base_domain: Some("tunnel.example.com".into()),
            docker_network_prefix: "x".repeat(41),
            ..TunnelConfig::default()
        };
        assert!(invalid.validate().is_err());
    }
}
