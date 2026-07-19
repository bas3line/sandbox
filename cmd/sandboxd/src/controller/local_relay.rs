use std::{collections::HashMap, fs, io::Write, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    body::{Body, to_bytes},
    extract::{
        FromRequestParts, Query, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::{Duration as ChronoDuration, Utc};
use futures_util::{SinkExt, StreamExt};
use sandbox_core::{
    api::{LocalRelayClientMessage, LocalRelayServerMessage, RelayWebSocketFrame},
    model::validate_dns_label,
};
use serde::Deserialize;
use tempfile::NamedTempFile;
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use tracing::{info, warn};
use uuid::Uuid;

use super::{ApiError, AppState, require_operator};

const RELAY_ROUTE_PREFIX: &str = "local-relay-";
type PendingHttpResponses = Arc<Mutex<HashMap<Uuid, oneshot::Sender<RelayedHttpResponse>>>>;
type PendingWebSockets = Arc<Mutex<HashMap<Uuid, oneshot::Sender<Result<(), String>>>>>;
type WebSocketFrameSenders = Arc<Mutex<HashMap<Uuid, mpsc::Sender<RelayWebSocketFrame>>>>;

#[derive(Clone, Default)]
pub(super) struct LocalRelayRegistry {
    sessions: Arc<RwLock<HashMap<String, LocalRelaySession>>>,
}

#[derive(Clone)]
struct LocalRelaySession {
    client_key: String,
    outbound: mpsc::Sender<LocalRelayServerMessage>,
    pending_http: PendingHttpResponses,
    pending_websockets: PendingWebSockets,
    websocket_frames: WebSocketFrameSenders,
}

struct RelayedHttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body_base64: String,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct ConnectQuery {
    subdomain: Option<String>,
}

impl LocalRelayRegistry {
    pub(super) fn initialize(config_dir: &str) -> anyhow::Result<Self> {
        let directory = PathBuf::from(config_dir);
        if let Ok(entries) = fs::read_dir(&directory) {
            for entry in entries {
                let entry = entry?;
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(RELAY_ROUTE_PREFIX)
                    && entry.path().extension().is_some_and(|value| value == "yml")
                {
                    fs::remove_file(entry.path())?;
                }
            }
        }
        Ok(Self::default())
    }

    async fn get(&self, hostname: &str) -> Option<LocalRelaySession> {
        self.sessions.read().await.get(hostname).cloned()
    }

    pub(super) async fn contains(&self, hostname: &str) -> bool {
        self.sessions.read().await.contains_key(hostname)
    }
}

pub(super) async fn connect(
    State(state): State<AppState>,
    Query(query): Query<ConnectQuery>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    if !state.config.tunnel.enabled || !state.config.tunnel.local_relay_enabled {
        return Err(ApiError::service_unavailable(
            "local HTTP relay is disabled on this Sandbox server",
        ));
    }
    if state.config.tunnel.local_relay_require_auth {
        require_operator(&state, &headers)?;
    }
    if let Some(subdomain) = &query.subdomain {
        validate_dns_label(subdomain).map_err(ApiError::bad_request)?;
    }
    let client_key = relay_client_key(&headers);
    let sessions = state.local_relays.sessions.read().await;
    if sessions.len() >= state.config.tunnel.max_local_relays {
        return Err(ApiError::service_unavailable(
            "this Sandbox server has reached its local relay limit",
        ));
    }
    let client_sessions = sessions
        .values()
        .filter(|session| session.client_key == client_key)
        .count();
    drop(sessions);
    if client_sessions >= state.config.tunnel.max_local_relays_per_client {
        return Err(ApiError::conflict(format!(
            "this client already has the maximum of {} local relays",
            state.config.tunnel.max_local_relays_per_client
        )));
    }

    Ok(websocket
        .on_upgrade(move |socket| serve_connection(socket, state, query.subdomain, client_key)))
}

pub(super) async fn proxy(State(state): State<AppState>, request: Request<Body>) -> Response {
    let hostname = match request_host(request.headers()) {
        Some(hostname) => hostname,
        None => return plain(StatusCode::BAD_REQUEST, "missing Host header"),
    };
    let Some(session) = state.local_relays.get(&hostname).await else {
        return plain(StatusCode::NOT_FOUND, "tunnel not found");
    };
    let uri = request.uri().clone();
    let headers = relay_headers(request.headers(), &hostname);

    if is_websocket_upgrade(request.headers()) {
        let protocols = requested_protocols(request.headers());
        let (mut parts, _) = request.into_parts();
        let websocket = match WebSocketUpgrade::from_request_parts(&mut parts, &state).await {
            Ok(websocket) => websocket,
            Err(rejection) => return rejection.into_response(),
        };
        let websocket = if protocols.is_empty() {
            websocket
        } else {
            websocket.protocols(protocols)
        };
        return websocket.on_upgrade(move |socket| proxy_websocket(socket, session, uri, headers));
    }

    proxy_http(state, session, request, uri, headers).await
}

async fn serve_connection(
    mut socket: WebSocket,
    state: AppState,
    requested_subdomain: Option<String>,
    client_key: String,
) {
    let base_domain = match state.config.tunnel.base_domain() {
        Ok(domain) => domain.to_owned(),
        Err(error) => {
            send_socket_error(&mut socket, error.to_string()).await;
            return;
        }
    };
    let session_id = Uuid::now_v7();
    let subdomain = requested_subdomain.unwrap_or_else(|| format!("local-{}", session_id.simple()));
    let hostname = format!("{subdomain}.{base_domain}");
    let public_url = format!("{}://{hostname}", state.config.tunnel.public_scheme);
    let route_path = relay_route_path(&state, session_id);
    let (outbound, mut outbound_rx) = mpsc::channel(256);
    let session = LocalRelaySession {
        client_key,
        outbound: outbound.clone(),
        pending_http: Arc::default(),
        pending_websockets: Arc::default(),
        websocket_frames: Arc::default(),
    };

    {
        let mut sessions = state.local_relays.sessions.write().await;
        if sessions.contains_key(&hostname) {
            send_socket_error(
                &mut socket,
                format!("subdomain {subdomain} is already in use"),
            )
            .await;
            return;
        }
        if let Err(error) = write_relay_route(&state, session_id, &hostname) {
            warn!(%error, %hostname, "failed to create local relay edge route");
            send_socket_error(&mut socket, "failed to register the public edge route").await;
            return;
        }
        sessions.insert(hostname.clone(), session.clone());
    }

    let ttl = Duration::from_secs(state.config.tunnel.local_relay_ttl_seconds);
    let expires_at = Utc::now()
        + ChronoDuration::seconds(
            i64::try_from(state.config.tunnel.local_relay_ttl_seconds).unwrap_or(i64::MAX),
        );
    if outbound
        .send(LocalRelayServerMessage::Ready {
            public_url: public_url.clone(),
            hostname: hostname.clone(),
            expires_at,
        })
        .await
        .is_err()
    {
        cleanup_connection(&state, &hostname, &route_path).await;
        return;
    }

    info!(%hostname, %expires_at, "local HTTP relay connected");
    let (mut socket_tx, mut socket_rx) = socket.split();
    let writer = tokio::spawn(async move {
        let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                message = outbound_rx.recv() => {
                    let Some(message) = message else { return };
                    let Ok(json) = serde_json::to_string(&message) else { return };
                    if socket_tx.send(Message::Text(json.into())).await.is_err() { return; }
                }
                _ = heartbeat.tick() => {
                    if socket_tx.send(Message::Ping(Vec::new().into())).await.is_err() { return; }
                }
            }
        }
    });

    let expires = tokio::time::sleep(ttl);
    tokio::pin!(expires);
    loop {
        tokio::select! {
            _ = &mut expires => break,
            message = socket_rx.next() => {
                let Some(Ok(message)) = message else { break };
                match message {
                    Message::Text(text) => {
                        match serde_json::from_str::<LocalRelayClientMessage>(&text) {
                            Ok(message) => handle_client_message(&session, message).await,
                            Err(error) => warn!(%error, %hostname, "ignored invalid local relay message"),
                        }
                    }
                    Message::Close(_) => break,
                    Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {}
                }
            }
        }
    }

    writer.abort();
    cleanup_connection(&state, &hostname, &route_path).await;
    info!(%hostname, "local HTTP relay disconnected");
}

async fn proxy_http(
    state: AppState,
    session: LocalRelaySession,
    request: Request<Body>,
    uri: Uri,
    headers: Vec<(String, String)>,
) -> Response {
    let request_id = Uuid::now_v7();
    let method = request.method().to_string();
    let body = match to_bytes(
        request.into_body(),
        state.config.tunnel.local_relay_body_limit_bytes,
    )
    .await
    {
        Ok(body) => body,
        Err(_) => return plain(StatusCode::PAYLOAD_TOO_LARGE, "request body is too large"),
    };
    let (response_tx, response_rx) = oneshot::channel();
    session
        .pending_http
        .lock()
        .await
        .insert(request_id, response_tx);
    let message = LocalRelayServerMessage::HttpRequest {
        request_id,
        method,
        uri: uri.to_string(),
        headers,
        body_base64: BASE64.encode(body),
    };
    if session.outbound.send(message).await.is_err() {
        session.pending_http.lock().await.remove(&request_id);
        return plain(StatusCode::BAD_GATEWAY, "local service disconnected");
    }
    let response = match tokio::time::timeout(Duration::from_secs(30), response_rx).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => return plain(StatusCode::BAD_GATEWAY, "local service disconnected"),
        Err(_) => {
            session.pending_http.lock().await.remove(&request_id);
            return plain(StatusCode::GATEWAY_TIMEOUT, "local service timed out");
        }
    };
    let body = match BASE64.decode(response.body_base64) {
        Ok(body) if body.len() <= state.config.tunnel.local_relay_body_limit_bytes => body,
        _ => {
            return plain(
                StatusCode::BAD_GATEWAY,
                "local service returned an invalid body",
            );
        }
    };
    let status = match StatusCode::from_u16(response.status) {
        Ok(status) => status,
        Err(_) => {
            return plain(
                StatusCode::BAD_GATEWAY,
                "local service returned an invalid status",
            );
        }
    };
    let mut output = Response::builder().status(status);
    if let Some(output_headers) = output.headers_mut() {
        for (name, value) in response.headers {
            let Ok(name) = HeaderName::try_from(name) else {
                continue;
            };
            if is_hop_by_hop(&name) {
                continue;
            }
            let Ok(value) = HeaderValue::try_from(value) else {
                continue;
            };
            output_headers.append(name, value);
        }
    }
    output.body(Body::from(body)).unwrap_or_else(|_| {
        plain(
            StatusCode::BAD_GATEWAY,
            "could not build the local service response",
        )
    })
}

async fn proxy_websocket(
    socket: WebSocket,
    session: LocalRelaySession,
    uri: Uri,
    headers: Vec<(String, String)>,
) {
    let request_id = Uuid::now_v7();
    let (ready_tx, ready_rx) = oneshot::channel();
    session
        .pending_websockets
        .lock()
        .await
        .insert(request_id, ready_tx);
    let (frame_tx, mut frame_rx) = mpsc::channel(256);
    session
        .websocket_frames
        .lock()
        .await
        .insert(request_id, frame_tx);
    if session
        .outbound
        .send(LocalRelayServerMessage::WebSocketOpen {
            request_id,
            uri: uri.to_string(),
            headers,
        })
        .await
        .is_err()
    {
        cleanup_websocket(&session, request_id).await;
        return;
    }
    match tokio::time::timeout(Duration::from_secs(10), ready_rx).await {
        Ok(Ok(Ok(()))) => {}
        _ => {
            cleanup_websocket(&session, request_id).await;
            return;
        }
    }

    let (mut public_tx, mut public_rx) = socket.split();
    loop {
        tokio::select! {
            message = public_rx.next() => {
                let Some(Ok(message)) = message else { break };
                let frame = axum_frame_to_relay(message);
                let closing = matches!(frame, RelayWebSocketFrame::Close { .. });
                if session.outbound.send(LocalRelayServerMessage::WebSocketFrame { request_id, frame }).await.is_err() || closing {
                    break;
                }
            }
            frame = frame_rx.recv() => {
                let Some(frame) = frame else { break };
                let closing = matches!(frame, RelayWebSocketFrame::Close { .. });
                if public_tx.send(relay_frame_to_axum(frame)).await.is_err() || closing {
                    break;
                }
            }
        }
    }
    let _ = session
        .outbound
        .send(LocalRelayServerMessage::WebSocketClose { request_id })
        .await;
    cleanup_websocket(&session, request_id).await;
}

async fn handle_client_message(session: &LocalRelaySession, message: LocalRelayClientMessage) {
    match message {
        LocalRelayClientMessage::HttpResponse {
            request_id,
            status,
            headers,
            body_base64,
        } => {
            if let Some(sender) = session.pending_http.lock().await.remove(&request_id) {
                let _ = sender.send(RelayedHttpResponse {
                    status,
                    headers,
                    body_base64,
                });
            }
        }
        LocalRelayClientMessage::WebSocketReady { request_id, error } => {
            if let Some(sender) = session.pending_websockets.lock().await.remove(&request_id) {
                let _ = sender.send(error.map_or(Ok(()), Err));
            }
        }
        LocalRelayClientMessage::WebSocketFrame { request_id, frame } => {
            let sender = session
                .websocket_frames
                .lock()
                .await
                .get(&request_id)
                .cloned();
            if let Some(sender) = sender {
                let _ = sender.send(frame).await;
            }
        }
        LocalRelayClientMessage::WebSocketClosed { request_id } => {
            if let Some(sender) = session.websocket_frames.lock().await.remove(&request_id) {
                let _ = sender
                    .send(RelayWebSocketFrame::Close {
                        code: None,
                        reason: String::new(),
                    })
                    .await;
            }
        }
    }
}

async fn cleanup_websocket(session: &LocalRelaySession, request_id: Uuid) {
    session.pending_websockets.lock().await.remove(&request_id);
    session.websocket_frames.lock().await.remove(&request_id);
}

async fn cleanup_connection(state: &AppState, hostname: &str, route_path: &PathBuf) {
    state.local_relays.sessions.write().await.remove(hostname);
    if let Err(error) = fs::remove_file(route_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        warn!(%error, %hostname, "failed to remove local relay edge route");
    }
}

fn write_relay_route(state: &AppState, session_id: Uuid, hostname: &str) -> anyhow::Result<()> {
    let directory = PathBuf::from(&state.config.tunnel.config_dir);
    fs::create_dir_all(&directory)?;
    let route_name = format!("local-{}", session_id.simple());
    let body = format!(
        "http:\n  routers:\n    {route_name}:\n      rule: \"Host(`{hostname}`)\"\n      entryPoints:\n        - {entrypoint}\n      service: {route_name}\n  services:\n    {route_name}:\n      loadBalancer:\n        passHostHeader: true\n        servers:\n          - url: \"{upstream}\"\n",
        entrypoint = state.config.tunnel.edge_entrypoint,
        upstream = state.config.tunnel.local_relay_upstream,
    );
    let mut temporary = NamedTempFile::new_in(&directory)?;
    temporary.write_all(body.as_bytes())?;
    temporary.flush()?;
    temporary.persist(relay_route_path(state, session_id))?;
    Ok(())
}

fn relay_route_path(state: &AppState, session_id: Uuid) -> PathBuf {
    PathBuf::from(&state.config.tunnel.config_dir)
        .join(format!("{RELAY_ROUTE_PREFIX}{}.yml", session_id.simple()))
}

fn relay_client_key(headers: &HeaderMap) -> String {
    for name in ["cf-connecting-ip", "x-forwarded-for"] {
        if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok())
            && let Some(first) = value.split(',').next()
        {
            let first = first.trim();
            if !first.is_empty() {
                return first.chars().take(128).collect();
            }
        }
    }
    "unknown".into()
}

fn request_host(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(':').next())
        .map(|value| value.trim_end_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn relay_headers(headers: &HeaderMap, public_host: &str) -> Vec<(String, String)> {
    let mut output = headers
        .iter()
        .filter(|(name, _)| **name != axum::http::header::HOST && !is_hop_by_hop(name))
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), value.to_owned()))
        })
        .collect::<Vec<_>>();
    output.push(("x-forwarded-host".into(), public_host.into()));
    output.push(("x-forwarded-proto".into(), "https".into()));
    output
}

fn requested_protocols(headers: &HeaderMap) -> Vec<String> {
    headers
        .get(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
        && headers
            .get(axum::http::header::CONNECTION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
            })
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn axum_frame_to_relay(message: Message) -> RelayWebSocketFrame {
    match message {
        Message::Text(text) => RelayWebSocketFrame::Text(text.to_string()),
        Message::Binary(data) => RelayWebSocketFrame::Binary(BASE64.encode(data)),
        Message::Ping(data) => RelayWebSocketFrame::Ping(BASE64.encode(data)),
        Message::Pong(data) => RelayWebSocketFrame::Pong(BASE64.encode(data)),
        Message::Close(frame) => RelayWebSocketFrame::Close {
            code: frame.as_ref().map(|frame| frame.code),
            reason: frame.map_or_else(String::new, |frame| frame.reason.to_string()),
        },
    }
}

fn relay_frame_to_axum(frame: RelayWebSocketFrame) -> Message {
    match frame {
        RelayWebSocketFrame::Text(text) => Message::Text(text.into()),
        RelayWebSocketFrame::Binary(data) => {
            Message::Binary(BASE64.decode(data).unwrap_or_default().into())
        }
        RelayWebSocketFrame::Ping(data) => {
            Message::Ping(BASE64.decode(data).unwrap_or_default().into())
        }
        RelayWebSocketFrame::Pong(data) => {
            Message::Pong(BASE64.decode(data).unwrap_or_default().into())
        }
        RelayWebSocketFrame::Close { code, reason } => {
            Message::Close(code.map(|code| CloseFrame {
                code,
                reason: reason.into(),
            }))
        }
    }
}

async fn send_socket_error(socket: &mut WebSocket, message: impl Into<String>) {
    let message = LocalRelayServerMessage::Error {
        message: message.into(),
    };
    if let Ok(json) = serde_json::to_string(&message) {
        let _ = socket.send(Message::Text(json.into())).await;
    }
    let _ = socket.close().await;
}

fn plain(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{relay_client_key, request_host};

    #[test]
    fn normalizes_public_host() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("Demo.Tunnel.Example:443"));
        assert_eq!(
            request_host(&headers).as_deref(),
            Some("demo.tunnel.example")
        );
    }

    #[test]
    fn prefers_cloudflare_client_address() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.8"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("192.0.2.2"));
        assert_eq!(relay_client_key(&headers), "203.0.113.8");
    }
}
