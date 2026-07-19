use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures_util::{SinkExt, StreamExt};
use http::{HeaderName, HeaderValue, Method};
use sandbox_core::api::{LocalRelayClientMessage, LocalRelayServerMessage, RelayWebSocketFrame};
use tokio::{
    net::TcpStream,
    sync::{Mutex, mpsc},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};
use url::Url;
use uuid::Uuid;

const MAX_RELAY_BODY_BYTES: usize = 64 * 1_048_576;

pub(crate) async fn run(
    port: u16,
    mut relay_url: Url,
    subdomain: Option<String>,
    token: Option<&str>,
    json: bool,
) -> Result<()> {
    let local_address = ensure_local_listener(port).await?;
    relay_url
        .set_scheme(match relay_url.scheme() {
            "https" => "wss",
            "http" => "ws",
            "wss" => "wss",
            "ws" => "ws",
            _ => bail!("the local relay URL must use HTTPS or HTTP"),
        })
        .map_err(|()| anyhow::anyhow!("invalid local relay URL"))?;
    relay_url.set_path("/v1/local-tunnels/connect");
    relay_url.set_query(None);
    if let Some(subdomain) = &subdomain {
        relay_url
            .query_pairs_mut()
            .append_pair("subdomain", subdomain);
    }

    let mut request = relay_url
        .as_str()
        .into_client_request()
        .context("build local relay request")?;
    request.headers_mut().insert(
        http::header::USER_AGENT,
        HeaderValue::from_static(concat!("sandbox/", env!("CARGO_PKG_VERSION"))),
    );
    if let Some(token) = token {
        request.headers_mut().insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .context("build relay authorization header")?,
        );
    }
    let (socket, _) = connect_async(request)
        .await
        .with_context(|| format!("connect to the Sandbox relay at {relay_url}"))?;
    let (mut socket_tx, mut socket_rx) = socket.split();
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<Message>(256);
    let writer = tokio::spawn(async move {
        let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                message = outbound_rx.recv() => {
                    let Some(message) = message else { return };
                    if socket_tx.send(message).await.is_err() { return; }
                }
                _ = heartbeat.tick() => {
                    if socket_tx.send(Message::Ping(Vec::new().into())).await.is_err() { return; }
                }
            }
        }
    });
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .context("build local HTTP client")?;
    let websocket_inputs: Arc<Mutex<HashMap<Uuid, mpsc::Sender<RelayWebSocketFrame>>>> =
        Arc::default();
    let mut ready = false;

    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c(), if ready => {
                signal.context("listen for Ctrl-C")?;
                break;
            }
            message = socket_rx.next() => {
                let Some(message) = message else {
                    if ready { bail!("the Sandbox relay disconnected unexpectedly"); }
                    bail!("the Sandbox relay closed before issuing a public URL");
                };
                match message.context("read from the Sandbox relay")? {
                    Message::Text(text) => {
                        let message = serde_json::from_str::<LocalRelayServerMessage>(&text)
                            .context("decode Sandbox relay message")?;
                        match message {
                            LocalRelayServerMessage::Ready { public_url, expires_at, .. } => {
                                ready = true;
                                if json {
                                    println!("{}", serde_json::to_string(&serde_json::json!({
                                        "local_url": format!("http://{local_address}"),
                                        "provider": "sandbox",
                                        "public_url": public_url,
                                        "expires_at": expires_at,
                                    }))?);
                                } else {
                                    println!("{public_url}");
                                    eprintln!("Public URL: anyone with this address can access your local service.");
                                    eprintln!("Press Ctrl-C to stop sharing.");
                                }
                            }
                            LocalRelayServerMessage::HttpRequest { request_id, method, uri, headers, body_base64 } => {
                                let outbound = outbound_tx.clone();
                                let http = http.clone();
                                tokio::spawn(async move {
                                    let response = relay_http_request(http, local_address, method, uri, headers, body_base64).await;
                                    let message = match response {
                                        Ok((status, headers, body_base64)) => LocalRelayClientMessage::HttpResponse {
                                            request_id,
                                            status,
                                            headers,
                                            body_base64,
                                        },
                                        Err(error) => LocalRelayClientMessage::HttpResponse {
                                            request_id,
                                            status: 502,
                                            headers: vec![("content-type".into(), "text/plain; charset=utf-8".into())],
                                            body_base64: BASE64.encode(format!("local service error: {error}")),
                                        },
                                    };
                                    send_protocol_message(&outbound, &message).await;
                                });
                            }
                            LocalRelayServerMessage::WebSocketOpen { request_id, uri, headers } => {
                                let outbound = outbound_tx.clone();
                                let inputs = websocket_inputs.clone();
                                let (input_tx, input_rx) = mpsc::channel(256);
                                inputs.lock().await.insert(request_id, input_tx);
                                tokio::spawn(async move {
                                    relay_local_websocket(local_address, request_id, uri, headers, input_rx, outbound.clone()).await;
                                    inputs.lock().await.remove(&request_id);
                                });
                            }
                            LocalRelayServerMessage::WebSocketFrame { request_id, frame } => {
                                let input = websocket_inputs.lock().await.get(&request_id).cloned();
                                if let Some(input) = input {
                                    let _ = input.send(frame).await;
                                }
                            }
                            LocalRelayServerMessage::WebSocketClose { request_id } => {
                                websocket_inputs.lock().await.remove(&request_id);
                            }
                            LocalRelayServerMessage::Error { message } => bail!("Sandbox relay: {message}"),
                        }
                    }
                    Message::Ping(data) => {
                        let _ = outbound_tx.send(Message::Pong(data)).await;
                    }
                    Message::Close(frame) => {
                        let reason = frame.map(|frame| frame.reason.to_string()).unwrap_or_default();
                        if reason.is_empty() { bail!("the Sandbox relay closed the tunnel"); }
                        bail!("the Sandbox relay closed the tunnel: {reason}");
                    }
                    Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
                }
            }
        }
    }

    writer.abort();
    websocket_inputs.lock().await.clear();
    Ok(())
}

async fn relay_http_request(
    http: reqwest::Client,
    local_address: SocketAddr,
    method: String,
    uri: String,
    headers: Vec<(String, String)>,
    body_base64: String,
) -> Result<(u16, Vec<(String, String)>, String)> {
    let url = local_url(local_address, &uri, "http")?;
    let method = Method::from_bytes(method.as_bytes()).context("invalid HTTP method")?;
    let body = BASE64.decode(body_base64).context("invalid request body")?;
    if body.len() > MAX_RELAY_BODY_BYTES {
        bail!("request body exceeded 64 MiB");
    }
    let mut request = http.request(method, url).body(body);
    for (name, value) in headers {
        let Ok(name) = HeaderName::try_from(name) else {
            continue;
        };
        if skip_local_header(&name) {
            continue;
        }
        let Ok(value) = HeaderValue::try_from(value) else {
            continue;
        };
        request = request.header(name, value);
    }
    let mut response = request.send().await.context("request local service")?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter(|(name, _)| !is_hop_by_hop(name))
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), value.to_owned()))
        })
        .collect();
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("read local service response")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RELAY_BODY_BYTES {
            bail!("local service response exceeded 64 MiB");
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, headers, BASE64.encode(body)))
}

async fn relay_local_websocket(
    local_address: SocketAddr,
    request_id: Uuid,
    uri: String,
    headers: Vec<(String, String)>,
    mut input: mpsc::Receiver<RelayWebSocketFrame>,
    outbound: mpsc::Sender<Message>,
) {
    let result = async {
        let url = local_url(local_address, &uri, "ws")?;
        let mut request = url
            .as_str()
            .into_client_request()
            .context("build local WebSocket request")?;
        for (name, value) in headers {
            let Ok(name) = HeaderName::try_from(name) else { continue };
            if skip_local_websocket_header(&name) { continue; }
            let Ok(value) = HeaderValue::try_from(value) else { continue };
            request.headers_mut().append(name, value);
        }
        let (socket, _) = connect_async(request)
            .await
            .context("connect to local WebSocket")?;
        send_protocol_message(
            &outbound,
            &LocalRelayClientMessage::WebSocketReady { request_id, error: None },
        )
        .await;
        let (mut local_tx, mut local_rx) = socket.split();
        loop {
            tokio::select! {
                frame = input.recv() => {
                    let Some(frame) = frame else { break };
                    let closing = matches!(frame, RelayWebSocketFrame::Close { .. });
                    local_tx.send(relay_frame_to_tungstenite(frame)).await.context("write local WebSocket")?;
                    if closing { break; }
                }
                message = local_rx.next() => {
                    let Some(message) = message else { break };
                    let frame = tungstenite_frame_to_relay(message.context("read local WebSocket")?);
                    let closing = matches!(frame, RelayWebSocketFrame::Close { .. });
                    send_protocol_message(
                        &outbound,
                        &LocalRelayClientMessage::WebSocketFrame { request_id, frame },
                    ).await;
                    if closing { break; }
                }
            }
        }
        Result::<()>::Ok(())
    }
    .await;

    if let Err(error) = result {
        send_protocol_message(
            &outbound,
            &LocalRelayClientMessage::WebSocketReady {
                request_id,
                error: Some(error.to_string()),
            },
        )
        .await;
    }
    send_protocol_message(
        &outbound,
        &LocalRelayClientMessage::WebSocketClosed { request_id },
    )
    .await;
}

fn local_url(local_address: SocketAddr, uri: &str, scheme: &str) -> Result<Url> {
    if !uri.starts_with('/') || uri.starts_with("//") {
        bail!("invalid relay request target");
    }
    let url = Url::parse(&format!("{scheme}://{local_address}{uri}"))
        .context("build local request URL")?;
    let host_matches = match (url.host(), local_address.ip()) {
        (Some(url::Host::Ipv4(actual)), IpAddr::V4(expected)) => actual == expected,
        (Some(url::Host::Ipv6(actual)), IpAddr::V6(expected)) => actual == expected,
        _ => false,
    };
    if !host_matches || url.port() != Some(local_address.port()) {
        bail!("relay request escaped the local service origin");
    }
    Ok(url)
}

async fn ensure_local_listener(port: u16) -> Result<SocketAddr> {
    let addresses: [std::net::SocketAddr; 2] = [
        ([127, 0, 0, 1], port).into(),
        ([0, 0, 0, 0, 0, 0, 0, 1], port).into(),
    ];
    for address in addresses {
        if matches!(
            tokio::time::timeout(Duration::from_millis(500), TcpStream::connect(address)).await,
            Ok(Ok(_))
        ) {
            return Ok(address);
        }
    }
    bail!(
        "nothing is listening on localhost:{port}; start your app first, then run `sandbox http {port}`"
    )
}

fn skip_local_header(name: &HeaderName) -> bool {
    *name == http::header::HOST || is_hop_by_hop(name)
}

fn skip_local_websocket_header(name: &HeaderName) -> bool {
    skip_local_header(name)
        || *name == http::header::ORIGIN
        || matches!(
            name.as_str(),
            "sec-websocket-key" | "sec-websocket-version" | "sec-websocket-extensions"
        )
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

fn tungstenite_frame_to_relay(message: Message) -> RelayWebSocketFrame {
    match message {
        Message::Text(text) => RelayWebSocketFrame::Text(text.to_string()),
        Message::Binary(data) => RelayWebSocketFrame::Binary(BASE64.encode(data)),
        Message::Ping(data) => RelayWebSocketFrame::Ping(BASE64.encode(data)),
        Message::Pong(data) => RelayWebSocketFrame::Pong(BASE64.encode(data)),
        Message::Close(frame) => RelayWebSocketFrame::Close {
            code: frame.as_ref().map(|frame| frame.code.into()),
            reason: frame.map_or_else(String::new, |frame| frame.reason.to_string()),
        },
        Message::Frame(_) => RelayWebSocketFrame::Close {
            code: None,
            reason: String::new(),
        },
    }
}

fn relay_frame_to_tungstenite(frame: RelayWebSocketFrame) -> Message {
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
                code: CloseCode::from(code),
                reason: reason.into(),
            }))
        }
    }
}

async fn send_protocol_message(
    outbound: &mpsc::Sender<Message>,
    message: &LocalRelayClientMessage,
) {
    if let Ok(json) = serde_json::to_string(message) {
        let _ = outbound.send(Message::Text(json.into())).await;
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    use super::{ensure_local_listener, local_url};

    #[test]
    fn local_url_never_accepts_an_absolute_target() {
        let local_address = SocketAddr::from((Ipv4Addr::LOCALHOST, 4321));
        assert!(local_url(local_address, "https://example.com/", "http").is_err());
        assert!(local_url(local_address, "//example.com/", "http").is_err());
        assert_eq!(
            local_url(local_address, "/hello?name=world", "http")
                .expect("local URL")
                .as_str(),
            "http://127.0.0.1:4321/hello?name=world"
        );
        assert_eq!(
            local_url(
                SocketAddr::from((Ipv6Addr::LOCALHOST, 4321)),
                "/hello?name=world",
                "http"
            )
            .expect("IPv6 local URL")
            .as_str(),
            "http://[::1]:4321/hello?name=world"
        );
    }

    #[tokio::test]
    async fn detects_a_local_listener() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind local listener");
        let port = listener.local_addr().expect("listener address").port();
        let address = ensure_local_listener(port)
            .await
            .expect("detect local listener");
        assert_eq!(address, listener.local_addr().expect("listener address"));
    }

    #[tokio::test]
    async fn detects_an_ipv6_only_local_listener() {
        let Ok(listener) = tokio::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).await else {
            return;
        };
        let expected = listener.local_addr().expect("listener address");
        let address = ensure_local_listener(expected.port())
            .await
            .expect("detect IPv6 local listener");
        assert_eq!(address, expected);
    }
}
