mod local_relay;

use std::{collections::BTreeSet, sync::Arc, time::Duration as StdDuration};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{Duration, Utc};
use sandbox_aegis::{AegisScheduler, ScheduleError};
use sandbox_core::{
    AssignmentId, NodeId, OperationId, SandboxId, TunnelId,
    api::{
        ApiErrorBody, CompleteAssignmentRequest, CreateSandboxRequest, CreateSandboxResponse,
        CreateTunnelRequest, ExecSandboxRequest, HealthResponse, HeartbeatRequest,
        LeaseAssignmentsResponse, ListQuery, ListSandboxesResponse, OperationResponse,
        RegisterNodeRequest, RegisterNodeResponse, TunnelAuthorizationQuery, TunnelCapabilities,
        TunnelOperationResponse,
    },
    config::SandboxConfig,
    model::{
        Assignment, AssignmentKind, AssignmentState, DataSensitivity, ExposureProtocol,
        LifecycleEvent, Operation, OperationState, PortExposure, Sandbox, SandboxState, Tunnel,
        TunnelState,
    },
};
use sandbox_events::BusRef;
use sandbox_storage::{StoreError, StoreRef};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;
use tower_http::{catch_panic::CatchPanicLayer, timeout::TimeoutLayer, trace::TraceLayer};
use tracing::{info, warn};

#[derive(Clone)]
struct AppState {
    config: Arc<SandboxConfig>,
    store: StoreRef,
    bus: BusRef,
    scheduler: AegisScheduler,
    local_relays: local_relay::LocalRelayRegistry,
}

pub async fn serve(
    config: Arc<SandboxConfig>,
    store: StoreRef,
    bus: BusRef,
    scheduler: AegisScheduler,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let store_name = store.backend_name();
    let local_relays = local_relay::LocalRelayRegistry::initialize(&config.tunnel.config_dir)?;
    let state = AppState {
        config: config.clone(),
        store,
        bus,
        scheduler,
        local_relays,
    };
    let reaper_state = state.clone();
    let reaper_cancel = cancel.clone();
    tokio::spawn(async move { reap_expired(reaper_state, reaper_cancel).await });
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(health))
        .route("/v1/sandboxes", post(create_sandbox).get(list_sandboxes))
        .route(
            "/v1/sandboxes/{id}",
            get(get_sandbox).delete(delete_sandbox),
        )
        .route("/v1/sandboxes/{id}/exec", post(exec_sandbox))
        .route("/v1/sandboxes/{id}/tunnels", post(create_tunnel))
        .route(
            "/v1/sandboxes/{id}/tunnels/{tunnel_id}",
            axum::routing::delete(delete_tunnel),
        )
        .route("/v1/tunnels/authorize", get(authorize_tunnel_domain))
        .route("/v1/local-tunnels/connect", get(local_relay::connect))
        .route("/v1/operations/{id}", get(get_operation))
        .route("/v1/nodes/register", post(register_node))
        .route("/v1/nodes/{id}/heartbeat", post(heartbeat_node))
        .route("/v1/nodes/{id}/assignments", get(lease_assignments))
        .route("/v1/assignments/complete", post(complete_assignment))
        .fallback(local_relay::proxy)
        .layer(DefaultBodyLimit::max(
            config.server.request_body_limit_bytes,
        ))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            StdDuration::from_secs(30),
        ))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.server.bind).await?;
    info!(address = %config.server.bind, store = store_name, "sandbox controller listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(cancel.cancelled_owned())
        .await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        store: state.store.backend_name().into(),
        now: Utc::now(),
        tunnels: TunnelCapabilities {
            enabled: state.config.tunnel.enabled,
            base_domain: state.config.tunnel.base_domain.clone(),
            public_scheme: state
                .config
                .tunnel
                .enabled
                .then(|| state.config.tunnel.public_scheme.clone()),
            protocols: state
                .config
                .tunnel
                .enabled
                .then_some(vec![ExposureProtocol::Http])
                .unwrap_or_default(),
            local_relay_enabled: state.config.tunnel.enabled
                && state.config.tunnel.local_relay_enabled,
            local_relay_url: (state.config.tunnel.enabled
                && state.config.tunnel.local_relay_enabled)
                .then(|| {
                    state.config.tunnel.base_domain.as_ref().map(|domain| {
                        format!("{}://relay.{domain}", state.config.tunnel.public_scheme)
                    })
                })
                .flatten(),
        },
    })
}

async fn create_sandbox(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateSandboxRequest>,
) -> Result<(StatusCode, Json<CreateSandboxResponse>), ApiError> {
    require_operator(&state, &headers)?;
    let mut spec = request.spec;
    spec.validate().map_err(ApiError::bad_request)?;
    if spec.ttl_seconds > state.config.policy.max_ttl_seconds {
        return Err(ApiError::bad_request(format!(
            "ttl exceeds server maximum of {} seconds",
            state.config.policy.max_ttl_seconds
        )));
    }
    let sandbox_id = SandboxId::new();
    let tunnels =
        materialize_tunnels(&state, sandbox_id, &spec.exposures, spec.sensitivity).await?;
    spec.exposures = tunnels.iter().map(tunnel_exposure).collect();
    let nodes = state.store.list_nodes().await?;
    let now = Utc::now();
    let decision = state.scheduler.schedule(&spec, &nodes, now)?;
    let operation_id = OperationId::new();
    let sandbox = Sandbox {
        id: sandbox_id,
        spec: spec.clone(),
        state: SandboxState::Scheduled,
        node_id: decision.node_id,
        isolation: decision.isolation,
        risk_score: decision.risk_score,
        created_at: now,
        updated_at: now,
        expires_at: now + Duration::seconds(i64::try_from(spec.ttl_seconds).unwrap_or(i64::MAX)),
        failure: None,
        tunnels: tunnels.clone(),
    };
    let operation = new_operation(operation_id, sandbox_id, now);
    let assignment = Assignment {
        id: AssignmentId::new(),
        operation_id,
        sandbox_id,
        node_id: decision.node_id,
        kind: AssignmentKind::Create {
            spec,
            isolation: decision.isolation,
            tunnels,
        },
        state: AssignmentState::Pending,
        attempt: 0,
        lease_until: None,
        created_at: now,
    };
    state
        .store
        .create_sandbox(sandbox.clone(), assignment, operation.clone())
        .await?;
    publish(&state, "sandbox.scheduled", &sandbox).await;
    Ok((
        StatusCode::CREATED,
        Json(CreateSandboxResponse { sandbox, operation }),
    ))
}

async fn create_tunnel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<SandboxId>,
    Json(request): Json<CreateTunnelRequest>,
) -> Result<(StatusCode, Json<TunnelOperationResponse>), ApiError> {
    require_operator(&state, &headers)?;
    let exposure = PortExposure {
        container_port: request.container_port,
        protocol: request.protocol,
        subdomain: request.subdomain,
        authenticated: request.authenticated,
    };
    exposure.validate().map_err(ApiError::bad_request)?;
    let mut sandbox = state.store.get_sandbox(id).await?;
    if sandbox.state != SandboxState::Running {
        return Err(ApiError::conflict(format!(
            "sandbox is {:?}, not running",
            sandbox.state
        )));
    }
    if sandbox.tunnels.len() >= state.config.tunnel.max_per_sandbox {
        return Err(ApiError::conflict(format!(
            "sandbox already has the configured maximum of {} tunnels",
            state.config.tunnel.max_per_sandbox
        )));
    }
    if sandbox
        .tunnels
        .iter()
        .any(|tunnel| tunnel.container_port == exposure.container_port)
    {
        return Err(ApiError::conflict(format!(
            "container port {} already has a tunnel",
            exposure.container_port
        )));
    }
    let node = state
        .store
        .list_nodes()
        .await?
        .into_iter()
        .find(|node| node.id == sandbox.node_id)
        .ok_or_else(|| ApiError::service_unavailable("sandbox worker is not registered"))?;
    if !node.supports_http_tunnels {
        return Err(ApiError::service_unavailable(
            "the selected worker does not support HTTP tunnels",
        ));
    }
    let tunnel = materialize_tunnels(
        &state,
        id,
        std::slice::from_ref(&exposure),
        sandbox.spec.sensitivity,
    )
    .await?
    .into_iter()
    .next()
    .ok_or_else(|| ApiError::internal("tunnel allocation returned no record"))?;
    let original = sandbox.clone();
    sandbox.tunnels.push(tunnel.clone());
    sandbox.spec.exposures.push(tunnel_exposure(&tunnel));
    sandbox.updated_at = Utc::now();
    state.store.update_sandbox(sandbox.clone()).await?;
    let operation = new_operation(OperationId::new(), id, sandbox.updated_at);
    let assignment = Assignment {
        id: AssignmentId::new(),
        operation_id: operation.id,
        sandbox_id: id,
        node_id: sandbox.node_id,
        kind: AssignmentKind::Expose {
            tunnel: tunnel.clone(),
        },
        state: AssignmentState::Pending,
        attempt: 0,
        lease_until: None,
        created_at: sandbox.updated_at,
    };
    if let Err(error) = state
        .store
        .create_assignment(assignment, operation.clone())
        .await
    {
        let _rollback = state.store.update_sandbox(original).await;
        return Err(error.into());
    }
    publish(&state, "sandbox.tunnel_pending", &sandbox).await;
    Ok((
        StatusCode::ACCEPTED,
        Json(TunnelOperationResponse { tunnel, operation }),
    ))
}

async fn delete_tunnel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, tunnel_id)): Path<(SandboxId, TunnelId)>,
) -> Result<(StatusCode, Json<TunnelOperationResponse>), ApiError> {
    require_operator(&state, &headers)?;
    let mut sandbox = state.store.get_sandbox(id).await?;
    if sandbox.state != SandboxState::Running {
        return Err(ApiError::conflict(format!(
            "sandbox is {:?}, not running",
            sandbox.state
        )));
    }
    let position = sandbox
        .tunnels
        .iter()
        .position(|tunnel| tunnel.id == tunnel_id)
        .ok_or_else(|| ApiError::not_found(format!("tunnel {tunnel_id}")))?;
    if matches!(
        sandbox.tunnels[position].state,
        TunnelState::Pending | TunnelState::Removing
    ) {
        return Err(ApiError::conflict(format!(
            "tunnel is {:?}; wait for its current operation",
            sandbox.tunnels[position].state
        )));
    }
    let original = sandbox.clone();
    sandbox.tunnels[position].state = TunnelState::Removing;
    sandbox.tunnels[position].failure = None;
    sandbox.updated_at = Utc::now();
    let tunnel = sandbox.tunnels[position].clone();
    state.store.update_sandbox(sandbox.clone()).await?;
    let operation = new_operation(OperationId::new(), id, sandbox.updated_at);
    let assignment = Assignment {
        id: AssignmentId::new(),
        operation_id: operation.id,
        sandbox_id: id,
        node_id: sandbox.node_id,
        kind: AssignmentKind::Unexpose {
            tunnel: tunnel.clone(),
        },
        state: AssignmentState::Pending,
        attempt: 0,
        lease_until: None,
        created_at: sandbox.updated_at,
    };
    if let Err(error) = state
        .store
        .create_assignment(assignment, operation.clone())
        .await
    {
        let _rollback = state.store.update_sandbox(original).await;
        return Err(error.into());
    }
    publish(&state, "sandbox.tunnel_removing", &sandbox).await;
    Ok((
        StatusCode::ACCEPTED,
        Json(TunnelOperationResponse { tunnel, operation }),
    ))
}

async fn authorize_tunnel_domain(
    State(state): State<AppState>,
    Query(query): Query<TunnelAuthorizationQuery>,
) -> StatusCode {
    if !state.config.tunnel.enabled || query.domain.len() > 253 {
        return StatusCode::NOT_FOUND;
    }
    let domain = query.domain.trim_end_matches('.').to_ascii_lowercase();
    let suffix = match state.config.tunnel.base_domain() {
        Ok(base) => format!(".{base}"),
        Err(_) => return StatusCode::NOT_FOUND,
    };
    if !domain.ends_with(&suffix) || domain.len() <= suffix.len() {
        return StatusCode::NOT_FOUND;
    }
    if state.config.tunnel.local_relay_enabled
        && (domain == format!("relay{suffix}") || state.local_relays.contains(&domain).await)
    {
        return StatusCode::NO_CONTENT;
    }
    match state.store.find_tunnel_by_hostname(&domain).await {
        Ok(Some(_)) => StatusCode::NO_CONTENT,
        Ok(None) | Err(_) => StatusCode::NOT_FOUND,
    }
}

async fn materialize_tunnels(
    state: &AppState,
    sandbox_id: SandboxId,
    exposures: &[PortExposure],
    sensitivity: DataSensitivity,
) -> Result<Vec<Tunnel>, ApiError> {
    if exposures.is_empty() {
        return Ok(Vec::new());
    }
    if !state.config.tunnel.enabled {
        return Err(ApiError::service_unavailable(
            "public tunnels are disabled on this deployment",
        ));
    }
    state
        .config
        .tunnel
        .validate()
        .map_err(|error| ApiError::internal(error.to_string()))?;
    if exposures.len() > state.config.tunnel.max_per_sandbox {
        return Err(ApiError::bad_request(format!(
            "at most {} tunnels are allowed per sandbox",
            state.config.tunnel.max_per_sandbox
        )));
    }
    if matches!(
        sensitivity,
        DataSensitivity::Confidential | DataSensitivity::Restricted
    ) {
        return Err(ApiError::bad_request(
            "public tunnels are not allowed for confidential or restricted sandboxes",
        ));
    }
    let base_domain = state
        .config
        .tunnel
        .base_domain()
        .map_err(|error| ApiError::internal(error.to_string()))?;
    let existing_hosts = state
        .store
        .list_sandboxes(None)
        .await?
        .into_iter()
        .flat_map(|sandbox| sandbox.tunnels)
        .map(|tunnel| tunnel.hostname)
        .collect::<BTreeSet<_>>();
    let mut allocated_hosts = BTreeSet::new();
    let mut allocated_ports = BTreeSet::new();
    let mut tunnels = Vec::with_capacity(exposures.len());
    for exposure in exposures {
        exposure.validate().map_err(ApiError::bad_request)?;
        if exposure.protocol != ExposureProtocol::Http {
            return Err(ApiError::bad_request(
                "public URLs currently support HTTP and WebSocket services only",
            ));
        }
        if exposure.authenticated {
            return Err(ApiError::bad_request(
                "built-in tunnel authentication is not implemented; put an identity-aware proxy in front of the edge",
            ));
        }
        if !allocated_ports.insert(exposure.container_port) {
            return Err(ApiError::bad_request(format!(
                "container port {} was requested more than once",
                exposure.container_port
            )));
        }
        let subdomain = exposure.subdomain.clone().unwrap_or_else(|| {
            format!(
                "s-{}-p{}",
                sandbox_id.to_string().replace('-', ""),
                exposure.container_port
            )
        });
        sandbox_core::model::validate_dns_label(&subdomain).map_err(ApiError::bad_request)?;
        let hostname = format!("{subdomain}.{base_domain}");
        if existing_hosts.contains(&hostname) || !allocated_hosts.insert(hostname.clone()) {
            return Err(ApiError::conflict(format!(
                "tunnel hostname {hostname} is already allocated"
            )));
        }
        tunnels.push(Tunnel {
            id: TunnelId::new(),
            container_port: exposure.container_port,
            protocol: exposure.protocol,
            subdomain,
            public_url: format!("{}://{hostname}", state.config.tunnel.public_scheme),
            hostname,
            authenticated: false,
            state: TunnelState::Pending,
            failure: None,
        });
    }
    Ok(tunnels)
}

fn tunnel_exposure(tunnel: &Tunnel) -> PortExposure {
    PortExposure {
        container_port: tunnel.container_port,
        protocol: tunnel.protocol,
        subdomain: Some(tunnel.subdomain.clone()),
        authenticated: tunnel.authenticated,
    }
}

async fn list_sandboxes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<ListSandboxesResponse>, ApiError> {
    require_operator(&state, &headers)?;
    let sandboxes = state.store.list_sandboxes(query.tenant.as_deref()).await?;
    Ok(Json(ListSandboxesResponse { sandboxes }))
}

async fn get_sandbox(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<SandboxId>,
) -> Result<Json<Sandbox>, ApiError> {
    require_operator(&state, &headers)?;
    Ok(Json(state.store.get_sandbox(id).await?))
}

async fn exec_sandbox(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<SandboxId>,
    Json(request): Json<ExecSandboxRequest>,
) -> Result<(StatusCode, Json<OperationResponse>), ApiError> {
    require_operator(&state, &headers)?;
    request.command.validate().map_err(ApiError::bad_request)?;
    let sandbox = state.store.get_sandbox(id).await?;
    if sandbox.state != SandboxState::Running {
        return Err(ApiError::conflict(format!(
            "sandbox is {:?}, not running",
            sandbox.state
        )));
    }
    let now = Utc::now();
    let operation = new_operation(OperationId::new(), id, now);
    let assignment = Assignment {
        id: AssignmentId::new(),
        operation_id: operation.id,
        sandbox_id: id,
        node_id: sandbox.node_id,
        kind: AssignmentKind::Exec {
            command: request.command,
        },
        state: AssignmentState::Pending,
        attempt: 0,
        lease_until: None,
        created_at: now,
    };
    state
        .store
        .create_assignment(assignment, operation.clone())
        .await?;
    Ok((StatusCode::ACCEPTED, Json(OperationResponse { operation })))
}

async fn delete_sandbox(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<SandboxId>,
) -> Result<(StatusCode, Json<OperationResponse>), ApiError> {
    require_operator(&state, &headers)?;
    let sandbox = state.store.get_sandbox(id).await?;
    if matches!(
        sandbox.state,
        SandboxState::Stopped | SandboxState::Stopping
    ) {
        return Err(ApiError::conflict(format!(
            "sandbox is already {:?}",
            sandbox.state
        )));
    }
    let operation = enqueue_delete(&state, sandbox).await?;
    Ok((StatusCode::ACCEPTED, Json(OperationResponse { operation })))
}

async fn enqueue_delete(state: &AppState, mut sandbox: Sandbox) -> Result<Operation, ApiError> {
    let now = Utc::now();
    sandbox.state = SandboxState::Stopping;
    for tunnel in &mut sandbox.tunnels {
        tunnel.state = TunnelState::Removing;
        tunnel.failure = None;
    }
    sandbox.updated_at = now;
    state.store.update_sandbox(sandbox.clone()).await?;
    let operation = new_operation(OperationId::new(), sandbox.id, now);
    let assignment = Assignment {
        id: AssignmentId::new(),
        operation_id: operation.id,
        sandbox_id: sandbox.id,
        node_id: sandbox.node_id,
        kind: AssignmentKind::Delete,
        state: AssignmentState::Pending,
        attempt: 0,
        lease_until: None,
        created_at: now,
    };
    state
        .store
        .create_assignment(assignment, operation.clone())
        .await?;
    publish(state, "sandbox.stopping", &sandbox).await;
    Ok(operation)
}

async fn reap_expired(state: AppState, cancel: CancellationToken) {
    let mut interval = tokio::time::interval(StdDuration::from_secs(30));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _ = interval.tick() => {
                let now = Utc::now();
                match state.store.list_sandboxes(None).await {
                    Ok(sandboxes) => {
                        for sandbox in sandboxes.into_iter().filter(|sandbox| {
                            sandbox.expires_at <= now && matches!(sandbox.state, SandboxState::Scheduled | SandboxState::Creating | SandboxState::Running)
                        }) {
                            let id = sandbox.id;
                            if let Err(error) = enqueue_delete(&state, sandbox).await {
                                warn!(%id, message = %error.message, "failed to reap expired sandbox");
                            }
                        }
                    }
                    Err(error) => warn!(%error, "failed to scan expired sandboxes"),
                }
            }
        }
    }
}

async fn get_operation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<OperationId>,
) -> Result<Json<OperationResponse>, ApiError> {
    require_operator(&state, &headers)?;
    Ok(Json(OperationResponse {
        operation: state.store.get_operation(id).await?,
    }))
}

async fn register_node(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<RegisterNodeRequest>,
) -> Result<Json<RegisterNodeResponse>, ApiError> {
    require_node(&state, &headers)?;
    request.node.last_seen = Utc::now();
    let node_id = request.node.id;
    state.store.upsert_node(request.node).await?;
    Ok(Json(RegisterNodeResponse {
        node_id,
        heartbeat_interval_seconds: state.config.server.heartbeat_interval_seconds,
    }))
}

async fn heartbeat_node(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<NodeId>,
    Json(request): Json<HeartbeatRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_node(&state, &headers)?;
    state.store.heartbeat_node(id, request).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

#[derive(Debug, Deserialize)]
struct LeaseQuery {
    #[serde(default = "default_lease_limit")]
    limit: usize,
}
const fn default_lease_limit() -> usize {
    8
}

async fn lease_assignments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<NodeId>,
    Query(query): Query<LeaseQuery>,
) -> Result<Json<LeaseAssignmentsResponse>, ApiError> {
    require_node(&state, &headers)?;
    let assignments = state
        .store
        .lease_assignments(
            id,
            query.limit.clamp(1, 100),
            state.config.server.assignment_lease_seconds,
        )
        .await?;
    Ok(Json(LeaseAssignmentsResponse { assignments }))
}

async fn complete_assignment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<CompleteAssignmentRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_node(&state, &headers)?;
    if let Some(output) = &mut request.output {
        truncate_string(&mut output.stdout, state.config.policy.max_output_bytes);
        truncate_string(&mut output.stderr, state.config.policy.max_output_bytes);
    }
    let sandbox_id = request.sandbox_id;
    let success = request.success;
    state.store.complete_assignment(request).await?;
    if let Ok(sandbox) = state.store.get_sandbox(sandbox_id).await {
        publish(
            &state,
            if success {
                "sandbox.operation_succeeded"
            } else {
                "sandbox.operation_failed"
            },
            &sandbox,
        )
        .await;
    }
    Ok(Json(serde_json::json!({"ok": true})))
}

fn new_operation(id: OperationId, sandbox_id: SandboxId, now: chrono::DateTime<Utc>) -> Operation {
    Operation {
        id,
        sandbox_id,
        state: OperationState::Pending,
        output: None,
        error: None,
        created_at: now,
        updated_at: now,
    }
}

async fn publish(state: &AppState, event_type: &str, sandbox: &Sandbox) {
    let event = LifecycleEvent {
        event_id: uuid::Uuid::now_v7(),
        event_type: event_type.into(),
        tenant: sandbox.spec.tenant.clone(),
        sandbox_id: sandbox.id,
        node_id: Some(sandbox.node_id),
        timestamp: Utc::now(),
        attributes: std::collections::BTreeMap::from([
            (
                "state".into(),
                format!("{:?}", sandbox.state).to_lowercase(),
            ),
            (
                "isolation".into(),
                format!("{:?}", sandbox.isolation).to_lowercase(),
            ),
        ]),
    };
    if let Err(error) = state.bus.publish(&event).await {
        warn!(%error, %event_type, "event publish failed");
    }
}

fn require_operator(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    authorize(
        headers,
        state.config.server.api_token.as_ref(),
        state.config.server.allow_unauthenticated_dev,
    )
}

fn require_node(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    authorize(
        headers,
        state.config.server.node_token.as_ref(),
        state.config.server.allow_unauthenticated_dev,
    )
}

fn authorize(
    headers: &HeaderMap,
    expected: Option<&SecretString>,
    allow_dev: bool,
) -> Result<(), ApiError> {
    if allow_dev && expected.is_none() {
        return Ok(());
    }
    let expected =
        expected.ok_or_else(|| ApiError::internal("authentication is not configured"))?;
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
    if provided
        .as_bytes()
        .ct_eq(expected.expose_secret().as_bytes())
        .into()
    {
        Ok(())
    } else {
        Err(ApiError::unauthorized("invalid bearer token"))
    }
}

fn truncate_string(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn bad_request(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: error.to_string(),
        }
    }
    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: message.into(),
        }
    }
    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: message.into(),
        }
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
        }
    }
    fn service_unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "unavailable",
            message: message.into(),
        }
    }
    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::NotFound(message) => Self {
                status: StatusCode::NOT_FOUND,
                code: "not_found",
                message,
            },
            StoreError::Conflict(message) => Self::conflict(message),
            other => {
                warn!(error = %other, "store request failed");
                Self::internal("state store failure")
            }
        }
    }
}

impl From<ScheduleError> for ApiError {
    fn from(error: ScheduleError) -> Self {
        match error {
            ScheduleError::PrivilegedExecution => Self::bad_request(error),
            ScheduleError::NoEligibleNode => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "no_capacity",
                message: error.to_string(),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorBody {
                code: self.code.into(),
                message: self.message,
                request_id: None,
            }),
        )
            .into_response()
    }
}
