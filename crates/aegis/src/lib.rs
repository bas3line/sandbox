//! AEGIS: Adaptive Execution Guard and Isolation Scheduler.
//!
//! AEGIS first derives a workload risk score and minimum isolation tier. It then
//! rejects unsafe or impossible nodes before ranking the remaining candidates by
//! dominant-resource headroom, fragmentation, pressure, locality, and image warmth.

use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use sandbox_core::{
    NodeId,
    model::{
        DataSensitivity, IsolationPreference, IsolationTier, NetworkMode, NodeRecord, ResourceSpec,
        SandboxSpec,
    },
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AegisPolicy {
    pub microvm_risk_threshold: u16,
    pub heartbeat_timeout_seconds: i64,
    pub max_host_pressure: f32,
    pub reserve_ratio: f64,
}

impl Default for AegisPolicy {
    fn default() -> Self {
        Self {
            microvm_risk_threshold: 55,
            heartbeat_timeout_seconds: 45,
            max_host_pressure: 0.92,
            reserve_ratio: 0.05,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlacementDecision {
    pub node_id: NodeId,
    pub isolation: IsolationTier,
    pub risk_score: u16,
    pub placement_score: i64,
    pub reasons: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ScheduleError {
    #[error("privileged execution is forbidden")]
    PrivilegedExecution,
    #[error("no healthy node satisfies isolation, labels, and resource requirements")]
    NoEligibleNode,
}

#[derive(Clone, Debug)]
pub struct AegisScheduler {
    policy: AegisPolicy,
}

impl AegisScheduler {
    #[must_use]
    pub fn new(policy: AegisPolicy) -> Self {
        Self { policy }
    }

    #[must_use]
    pub fn policy(&self) -> &AegisPolicy {
        &self.policy
    }

    pub fn schedule(
        &self,
        spec: &SandboxSpec,
        nodes: &[NodeRecord],
        now: DateTime<Utc>,
    ) -> Result<PlacementDecision, ScheduleError> {
        if spec.signals.privileged {
            return Err(ScheduleError::PrivilegedExecution);
        }

        let risk_score = risk_score(spec);
        let isolation = required_isolation(spec, risk_score, self.policy.microvm_risk_threshold);
        let mut candidates = nodes
            .iter()
            .filter_map(|node| self.score_node(spec, node, isolation, risk_score, now))
            .collect::<Vec<_>>();

        candidates.sort_by(|left, right| {
            right
                .placement_score
                .cmp(&left.placement_score)
                .then_with(|| left.node_id.cmp(&right.node_id))
        });
        candidates
            .into_iter()
            .next()
            .ok_or(ScheduleError::NoEligibleNode)
    }

    fn score_node(
        &self,
        spec: &SandboxSpec,
        node: &NodeRecord,
        isolation: IsolationTier,
        risk_score: u16,
        now: DateTime<Utc>,
    ) -> Option<PlacementDecision> {
        if !node.is_healthy(now, self.policy.heartbeat_timeout_seconds)
            || node.pressure > self.policy.max_host_pressure
            || !node.supported_tiers.contains(&isolation)
            || (!spec.exposures.is_empty() && !node.supports_http_tunnels)
            || !spec.resources.fits_within(&node.capacity.available)
            || (node.capacity.max_sandboxes > 0
                && node.capacity.running_sandboxes >= node.capacity.max_sandboxes)
            || !labels_match(spec, node)
        {
            return None;
        }

        let post = remaining(&node.capacity.available, &spec.resources)?;
        let total = &node.capacity.total;
        let dominant_headroom = [
            ratio(post.cpu_millis, total.cpu_millis),
            ratio(post.memory_mib, total.memory_mib),
            ratio(post.disk_mib, total.disk_mib),
            ratio(post.pids, total.pids),
        ]
        .into_iter()
        .fold(1.0_f64, f64::min);
        if dominant_headroom < self.policy.reserve_ratio {
            return None;
        }

        let resource_spread = spread([
            ratio(post.cpu_millis, total.cpu_millis),
            ratio(post.memory_mib, total.memory_mib),
            ratio(post.disk_mib, total.disk_mib),
            ratio(post.pids, total.pids),
        ]);
        let pressure = f64::from(node.pressure.clamp(0.0, 1.0));
        let warm_bonus = if node.warm_images.contains(&spec.image) {
            120
        } else {
            0
        };
        let region_bonus = match spec.placement.preferred_region.as_deref() {
            Some(region) if region == node.region => 80,
            Some(_) => 0,
            None => 20,
        };
        let headroom_score = (dominant_headroom * 500.0).round() as i64;
        let fragmentation_penalty = (resource_spread * 220.0).round() as i64;
        let pressure_penalty = (pressure * 300.0).round() as i64;
        let packing_bonus = packing_bonus(dominant_headroom);
        let score = 1_000 + headroom_score + warm_bonus + region_bonus + packing_bonus
            - fragmentation_penalty
            - pressure_penalty;

        Some(PlacementDecision {
            node_id: node.id,
            isolation,
            risk_score,
            placement_score: score,
            reasons: vec![
                format!("risk={risk_score}; required_isolation={isolation:?}"),
                format!("dominant_headroom={dominant_headroom:.3}"),
                format!("resource_fragmentation={resource_spread:.3}"),
                format!("host_pressure={pressure:.3}"),
                format!("warm_image={}", node.warm_images.contains(&spec.image)),
            ],
        })
    }
}

#[must_use]
pub fn risk_score(spec: &SandboxSpec) -> u16 {
    let mut score = match spec.sensitivity {
        DataSensitivity::Public => 0,
        DataSensitivity::Internal => 10,
        DataSensitivity::Confidential => 25,
        DataSensitivity::Restricted => 45,
    };
    score += match spec.network {
        NetworkMode::Deny => 0,
        NetworkMode::RestrictedEgress => 10,
        NetworkMode::OpenEgress => 25,
    };
    if spec.signals.untrusted_repository {
        score += 20;
    }
    if spec.signals.executes_generated_code {
        score += 20;
    }
    if spec.signals.needs_secrets {
        score += 15;
    }
    if spec.signals.host_mounts {
        score += 25;
    }
    if spec.signals.privileged {
        score += 100;
    }
    if !spec.exposures.is_empty() {
        score += 15;
    }
    if spec.ttl_seconds > 86_400 {
        score += 10;
    }
    score.min(100)
}

#[must_use]
pub fn required_isolation(spec: &SandboxSpec, score: u16, microvm_threshold: u16) -> IsolationTier {
    match spec.isolation {
        IsolationPreference::Container if score < microvm_threshold => IsolationTier::Container,
        IsolationPreference::Container => IsolationTier::Microvm,
        IsolationPreference::Microvm => IsolationTier::Microvm,
        IsolationPreference::Auto if score >= microvm_threshold => IsolationTier::Microvm,
        IsolationPreference::Auto => IsolationTier::Container,
    }
}

fn labels_match(spec: &SandboxSpec, node: &NodeRecord) -> bool {
    spec.placement
        .required_labels
        .iter()
        .all(|(key, value)| node.labels.get(key) == Some(value))
}

fn remaining(available: &ResourceSpec, requested: &ResourceSpec) -> Option<ResourceSpec> {
    Some(ResourceSpec {
        cpu_millis: available.cpu_millis.checked_sub(requested.cpu_millis)?,
        memory_mib: available.memory_mib.checked_sub(requested.memory_mib)?,
        disk_mib: available.disk_mib.checked_sub(requested.disk_mib)?,
        pids: available.pids.checked_sub(requested.pids)?,
    })
}

fn ratio(value: u32, total: u32) -> f64 {
    if total == 0 {
        0.0
    } else {
        f64::from(value) / f64::from(total)
    }
}

fn spread(values: [f64; 4]) -> f64 {
    let min = values.into_iter().fold(1.0_f64, f64::min);
    let max = values.into_iter().fold(0.0_f64, f64::max);
    max - min
}

fn packing_bonus(headroom: f64) -> i64 {
    match headroom.partial_cmp(&0.35).unwrap_or(Ordering::Less) {
        Ordering::Less => 70,
        Ordering::Equal => 50,
        Ordering::Greater => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use chrono::Utc;
    use sandbox_core::{
        NodeId,
        model::{
            DataSensitivity, ExposureProtocol, IsolationPreference, IsolationTier, NetworkMode,
            NodeCapacity, NodeRecord, PortExposure, ResourceSpec, SandboxSpec, WorkloadSignals,
        },
    };

    use super::{AegisScheduler, risk_score};

    fn spec() -> SandboxSpec {
        SandboxSpec {
            tenant: "acme".into(),
            image: "ubuntu:24.04".into(),
            command: Vec::new(),
            env: BTreeMap::new(),
            resources: ResourceSpec::default(),
            network: NetworkMode::Deny,
            isolation: IsolationPreference::Auto,
            sensitivity: DataSensitivity::Internal,
            signals: WorkloadSignals::default(),
            ttl_seconds: 3_600,
            labels: BTreeMap::new(),
            placement: Default::default(),
            exposures: Vec::new(),
            agent: None,
        }
    }

    fn node(name: &str, pressure: f32, warm: bool) -> NodeRecord {
        let total = ResourceSpec {
            cpu_millis: 8_000,
            memory_mib: 16_384,
            disk_mib: 100_000,
            pids: 4_096,
        };
        NodeRecord {
            id: NodeId::new(),
            name: name.into(),
            region: "local".into(),
            zone: "a".into(),
            labels: BTreeMap::new(),
            capacity: NodeCapacity {
                total: total.clone(),
                available: total,
                max_sandboxes: 100,
                running_sandboxes: 0,
            },
            supported_tiers: BTreeSet::from([IsolationTier::Container, IsolationTier::Microvm]),
            warm_images: if warm {
                BTreeSet::from(["ubuntu:24.04".into()])
            } else {
                BTreeSet::new()
            },
            pressure,
            draining: false,
            supports_http_tunnels: true,
            last_seen: Utc::now(),
        }
    }

    #[test]
    fn generated_untrusted_secret_workload_requires_microvm() {
        let mut workload = spec();
        workload.sensitivity = DataSensitivity::Confidential;
        workload.signals = WorkloadSignals {
            untrusted_repository: true,
            executes_generated_code: true,
            needs_secrets: true,
            ..Default::default()
        };
        assert!(risk_score(&workload) >= 55);
        let decision = AegisScheduler::new(Default::default())
            .schedule(&workload, &[node("n1", 0.1, false)], Utc::now())
            .expect("eligible microVM node");
        assert_eq!(decision.isolation, IsolationTier::Microvm);
    }

    #[test]
    fn warm_low_pressure_node_wins() {
        let workload = spec();
        let cold = node("cold", 0.40, false);
        let warm = node("warm", 0.05, true);
        let decision = AegisScheduler::new(Default::default())
            .schedule(&workload, &[cold, warm.clone()], Utc::now())
            .expect("eligible node");
        assert_eq!(decision.node_id, warm.id);
    }

    #[test]
    fn labels_are_a_hard_gate() {
        let mut workload = spec();
        workload
            .placement
            .required_labels
            .insert("gpu".into(), "true".into());
        let result = AegisScheduler::new(Default::default()).schedule(
            &workload,
            &[node("n1", 0.1, false)],
            Utc::now(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn high_risk_container_request_is_upgraded() {
        let mut workload = spec();
        workload.isolation = IsolationPreference::Container;
        workload.sensitivity = DataSensitivity::Restricted;
        workload.network = NetworkMode::OpenEgress;
        let decision = AegisScheduler::new(Default::default()).schedule(
            &workload,
            &[node("n1", 0.1, false)],
            Utc::now(),
        );
        assert!(matches!(decision, Ok(value) if value.isolation == IsolationTier::Microvm));
    }

    #[test]
    fn exposure_requires_a_tunnel_capable_worker() {
        let mut workload = spec();
        workload.exposures.push(PortExposure {
            container_port: 3_000,
            protocol: ExposureProtocol::Http,
            subdomain: None,
            authenticated: false,
        });
        let mut incapable = node("no-edge", 0.1, false);
        incapable.supports_http_tunnels = false;
        let result =
            AegisScheduler::new(Default::default()).schedule(&workload, &[incapable], Utc::now());
        assert!(result.is_err());
    }
}
