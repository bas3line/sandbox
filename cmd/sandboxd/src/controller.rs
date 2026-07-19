use std::{sync::Arc, time::Duration as StdDuration};

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
    AssignmentId, NodeId, OperationId, SandboxId,
    api::{
        ApiErrorBody, CompleteAssignmentRequest, CreateSandboxRequest, CreateSandboxResponse,
        ExecSandboxRequest, HealthResponse, HeartbeatRequest, LeaseAssignmentsResponse, ListQuery,
        ListSandboxesResponse, OperationResponse, RegisterNodeRequest, RegisterNodeResponse,
    },
    config::SandboxConfig,
    model::{
        Assignment, AssignmentKind, AssignmentState, LifecycleEvent, Operation, OperationState,
        Sandbox, SandboxState,
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
}

pub async fn serve(
    config: Arc<SandboxConfig>,
    store: StoreRef,
    bus: BusRef,
    scheduler: AegisScheduler,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let store_name = store.backend_name();
    let state = AppState {
        config: config.clone(),
        store,
        bus,
        scheduler,
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
        .route("/v1/operations/{id}", get(get_operation))
        .route("/v1/nodes/register", post(register_node))
        .route("/v1/nodes/{id}/heartbeat", post(heartbeat_node))
        .route("/v1/nodes/{id}/assignments", get(lease_assignments))
        .route("/v1/assignments/complete", post(complete_assignment))
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
    })
}

async fn create_sandbox(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateSandboxRequest>,
) -> Result<(StatusCode, Json<CreateSandboxResponse>), ApiError> {
    require_operator(&state, &headers)?;
    request.spec.validate().map_err(ApiError::bad_request)?;
    if request.spec.ttl_seconds > state.config.policy.max_ttl_seconds {
        return Err(ApiError::bad_request(format!(
            "ttl exceeds server maximum of {} seconds",
            state.config.policy.max_ttl_seconds
        )));
    }
    let nodes = state.store.list_nodes().await?;
    let now = Utc::now();
    let decision = state.scheduler.schedule(&request.spec, &nodes, now)?;
    let sandbox_id = SandboxId::new();
    let operation_id = OperationId::new();
    let sandbox = Sandbox {
        id: sandbox_id,
        spec: request.spec.clone(),
        state: SandboxState::Scheduled,
        node_id: decision.node_id,
        isolation: decision.isolation,
        risk_score: decision.risk_score,
        created_at: now,
        updated_at: now,
        expires_at: now
            + Duration::seconds(i64::try_from(request.spec.ttl_seconds).unwrap_or(i64::MAX)),
        failure: None,
    };
    let operation = new_operation(operation_id, sandbox_id, now);
    let assignment = Assignment {
        id: AssignmentId::new(),
        operation_id,
        sandbox_id,
        node_id: decision.node_id,
        kind: AssignmentKind::Create {
            spec: request.spec,
            isolation: decision.isolation,
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
