use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::Utc;
use sandbox_client::SandboxClient;
use sandbox_core::{
    NodeId, SandboxId,
    api::{CompleteAssignmentRequest, HeartbeatRequest},
    config::SandboxConfig,
    model::{Assignment, AssignmentKind, NodeCapacity, NodeRecord, ResourceSpec, SandboxState},
};
use sandbox_runtime::{RuntimeRef, from_config};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

struct WorkerState {
    allocations: Mutex<HashMap<SandboxId, ResourceSpec>>,
    sandbox_locks: Mutex<HashMap<SandboxId, Arc<Mutex<()>>>>,
}

pub async fn run(config: Arc<SandboxConfig>, cancel: CancellationToken) -> Result<()> {
    let runtime = from_config(&config.node, &config.tunnel, config.policy.max_output_bytes)
        .context("configure runtime")?;
    let capabilities = runtime.probe().await.context("probe sandbox runtime")?;
    info!(runtime = %capabilities.name, version = %capabilities.version, "worker runtime ready");
    let node_id = load_or_create_node_id(Path::new(&config.node.state_dir)).await?;
    let client = SandboxClient::new(
        config.node.control_plane_url.clone(),
        config.node.token.clone(),
    )?;
    let state = Arc::new(WorkerState {
        allocations: Mutex::new(HashMap::new()),
        sandbox_locks: Mutex::new(HashMap::new()),
    });
    let node = node_record(
        &config,
        node_id,
        BTreeSet::from_iter(capabilities.tiers),
        capabilities.supports_http_tunnels,
    );
    register_with_retry(&client, node, &cancel).await?;
    let concurrency = Arc::new(Semaphore::new(16));
    let mut heartbeat_tick = tokio::time::interval(Duration::from_secs(
        config.server.heartbeat_interval_seconds.max(1),
    ));
    let mut poll_tick = tokio::time::interval(Duration::from_millis(500));

    loop {
        tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            _ = heartbeat_tick.tick() => {
                let heartbeat = heartbeat(&config, &state).await;
                if let Err(error) = client.heartbeat(node_id, &heartbeat).await { warn!(%error, "worker heartbeat failed"); }
            }
            _ = poll_tick.tick() => {
                match client.lease_assignments(node_id, 16).await {
                    Ok(response) => {
                        for assignment in response.assignments {
                            let permit = concurrency.clone().acquire_owned().await.context("worker semaphore closed")?;
                            let client = client.clone();
                            let config = config.clone();
                            let runtime = runtime.clone();
                            let state = state.clone();
                            tokio::spawn(async move {
                                let _permit = permit;
                                if let Err(error) = process_assignment(
                                    &client,
                                    &config,
                                    node_id,
                                    runtime,
                                    state,
                                    assignment,
                                )
                                .await
                                {
                                    error!(%error, "assignment processing failed");
                                }
                            });
                        }
                    }
                    Err(error) => warn!(%error, "assignment poll failed"),
                }
            }
        }
    }
}

async fn register_with_retry(
    client: &SandboxClient,
    node: NodeRecord,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut delay = Duration::from_millis(250);
    loop {
        match client.register_node(node.clone()).await {
            Ok(response) => {
                info!(node_id = %response.node_id, "worker registered");
                return Ok(());
            }
            Err(error) => warn!(%error, ?delay, "worker registration failed; retrying"),
        }
        tokio::select! {
            () = cancel.cancelled() => anyhow::bail!("worker cancelled before registration"),
            () = tokio::time::sleep(delay) => {}
        }
        delay = (delay * 2).min(Duration::from_secs(15));
    }
}

async fn process_assignment(
    client: &SandboxClient,
    config: &SandboxConfig,
    node_id: NodeId,
    runtime: RuntimeRef,
    state: Arc<WorkerState>,
    assignment: Assignment,
) -> Result<()> {
    let lock = {
        let mut locks = state.sandbox_locks.lock().await;
        locks
            .entry(assignment.sandbox_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _sandbox_guard = lock.lock().await;
    let sandbox_id = assignment.sandbox_id;
    let result = match &assignment.kind {
        AssignmentKind::Create {
            spec,
            isolation,
            tunnels,
        } => create_with_tunnels(runtime.as_ref(), sandbox_id, spec, *isolation, tunnels)
            .await
            .map(|()| {
                let resources = spec.resources.clone();
                (SandboxState::Running, None, Some(resources))
            }),
        AssignmentKind::Exec { command } => runtime.exec(sandbox_id, command).await.map(|output| {
            let success = output.exit_code == 0;
            (SandboxState::Running, Some((output, success)), None)
        }),
        AssignmentKind::Expose { tunnel } => runtime
            .expose(sandbox_id, tunnel)
            .await
            .map(|()| (SandboxState::Running, None, None)),
        AssignmentKind::Unexpose { tunnel } => runtime
            .unexpose(sandbox_id, tunnel)
            .await
            .map(|()| (SandboxState::Running, None, None)),
        AssignmentKind::Delete => runtime
            .delete(sandbox_id)
            .await
            .map(|()| (SandboxState::Stopped, None, None)),
    };

    let mut capacity_changed = false;
    let request = match result {
        Ok((sandbox_state, output_and_success, allocation)) => {
            if let Some(resources) = allocation {
                state.allocations.lock().await.insert(sandbox_id, resources);
                capacity_changed = true;
            }
            if matches!(assignment.kind, AssignmentKind::Delete) {
                state.allocations.lock().await.remove(&sandbox_id);
                capacity_changed = true;
            }
            let (output, success) = output_and_success
                .map_or((None, true), |(output, success)| (Some(output), success));
            CompleteAssignmentRequest {
                assignment_id: assignment.id,
                operation_id: assignment.operation_id,
                sandbox_id,
                success,
                sandbox_state,
                output,
                error: if success {
                    None
                } else {
                    Some("command exited with a non-zero status".into())
                },
            }
        }
        Err(error) => CompleteAssignmentRequest {
            assignment_id: assignment.id,
            operation_id: assignment.operation_id,
            sandbox_id,
            success: false,
            sandbox_state: match assignment.kind {
                AssignmentKind::Exec { .. }
                | AssignmentKind::Expose { .. }
                | AssignmentKind::Unexpose { .. } => SandboxState::Running,
                _ => SandboxState::Failed,
            },
            output: None,
            error: Some(error.to_string()),
        },
    };
    // Publish changed capacity before completing the operation. A caller that
    // immediately creates another sandbox after a successful delete should
    // never see stale no-capacity, and a successful create must consume
    // capacity before it is reported ready.
    if capacity_changed {
        let heartbeat = heartbeat(config, &state).await;
        client
            .heartbeat(node_id, &heartbeat)
            .await
            .context("publish capacity after assignment")?;
    }
    client
        .complete_assignment(&request)
        .await
        .context("report assignment completion")
}

fn node_record(
    config: &SandboxConfig,
    id: NodeId,
    supported_tiers: BTreeSet<sandbox_core::model::IsolationTier>,
    supports_http_tunnels: bool,
) -> NodeRecord {
    NodeRecord {
        id,
        name: config.node.name.clone(),
        region: config.node.region.clone(),
        zone: config.node.zone.clone(),
        labels: config.node.labels.clone(),
        capacity: NodeCapacity {
            total: config.node.resources.clone(),
            available: config.node.resources.clone(),
            max_sandboxes: config.node.max_sandboxes,
            running_sandboxes: 0,
        },
        supported_tiers,
        warm_images: BTreeSet::new(),
        pressure: 0.0,
        draining: false,
        supports_http_tunnels,
        last_seen: Utc::now(),
    }
}

async fn create_with_tunnels(
    runtime: &dyn sandbox_runtime::SandboxRuntime,
    sandbox_id: SandboxId,
    spec: &sandbox_core::model::SandboxSpec,
    isolation: sandbox_core::model::IsolationTier,
    tunnels: &[sandbox_core::model::Tunnel],
) -> Result<(), sandbox_runtime::RuntimeError> {
    runtime.create(sandbox_id, spec, isolation).await?;
    let mut exposed = Vec::new();
    for tunnel in tunnels {
        if let Err(error) = runtime.expose(sandbox_id, tunnel).await {
            for exposed_tunnel in exposed.iter().rev() {
                let _cleanup = runtime.unexpose(sandbox_id, exposed_tunnel).await;
            }
            let _cleanup = runtime.delete(sandbox_id).await;
            return Err(error);
        }
        exposed.push(tunnel.clone());
    }
    Ok(())
}

async fn heartbeat(config: &SandboxConfig, state: &WorkerState) -> HeartbeatRequest {
    let allocations = state.allocations.lock().await;
    let mut available = config.node.resources.clone();
    for resources in allocations.values() {
        available.cpu_millis = available.cpu_millis.saturating_sub(resources.cpu_millis);
        available.memory_mib = available.memory_mib.saturating_sub(resources.memory_mib);
        available.disk_mib = available.disk_mib.saturating_sub(resources.disk_mib);
        available.pids = available.pids.saturating_sub(resources.pids);
    }
    let pressure =
        1.0 - (available.memory_mib as f32 / config.node.resources.memory_mib.max(1) as f32);
    HeartbeatRequest {
        capacity: NodeCapacity {
            total: config.node.resources.clone(),
            available,
            max_sandboxes: config.node.max_sandboxes,
            running_sandboxes: u32::try_from(allocations.len()).unwrap_or(u32::MAX),
        },
        pressure,
        warm_images: BTreeSet::new(),
    }
}

async fn load_or_create_node_id(state_dir: &Path) -> Result<NodeId> {
    let path = state_dir.join("node-id");
    match tokio::fs::read_to_string(&path).await {
        Ok(value) => return value.trim().parse().context("parse persisted node ID"),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(error).context("read persisted node ID");
        }
        Err(_) => {}
    }
    tokio::fs::create_dir_all(state_dir)
        .await
        .context("create worker state directory")?;
    let id = NodeId::new();
    let temporary = state_dir.join(format!("node-id.{}.tmp", std::process::id()));
    tokio::fs::write(&temporary, format!("{id}\n"))
        .await
        .context("write temporary node ID")?;
    tokio::fs::rename(&temporary, &path)
        .await
        .context("persist node ID atomically")?;
    Ok(id)
}
