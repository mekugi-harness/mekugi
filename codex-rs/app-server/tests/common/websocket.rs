//! Shared app-server WebSocket process fixtures and JSON-RPC helpers.

use crate::DEFAULT_APP_SERVER_TEST_ARGS;
use crate::to_response;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::InitializeParams;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use futures::SinkExt;
use futures::StreamExt;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::Command;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio::time::sleep;
use tokio::time::timeout;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WebSocketMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::header::ORIGIN;

// macOS and Windows CI can spend tens of seconds starting the app-server test
// binary under Bazel before it accepts JSON-RPC or reports its websocket bind
// address.
#[cfg(any(target_os = "macos", windows))]
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(not(any(target_os = "macos", windows)))]
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);

pub type WsClient = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
pub async fn spawn_websocket_server(codex_home: &Path) -> Result<(Child, SocketAddr)> {
    spawn_websocket_server_with_args(codex_home, "ws://127.0.0.1:0", &[]).await
}

pub async fn spawn_websocket_server_with_args(
    codex_home: &Path,
    listen_url: &str,
    extra_args: &[String],
) -> Result<(Child, SocketAddr)> {
    let program = codex_utils_cargo_bin::cargo_bin("mekugi-app-server")
        .context("should find app-server binary")?;
    let mut cmd = Command::new(program);
    cmd.arg("--listen")
        .arg(listen_url)
        .args(DEFAULT_APP_SERVER_TEST_ARGS)
        .args(extra_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env("CODEX_HOME", codex_home)
        .env("RUST_LOG", "warn")
        .env("OTEL_METRIC_EXPORT_INTERVAL", "200");

    let mut process = cmd
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn websocket app-server process")?;

    let stderr = process
        .stderr
        .take()
        .context("failed to capture websocket app-server stderr")?;
    let mut stderr_reader = BufReader::new(stderr).lines();
    let deadline = Instant::now() + DEFAULT_READ_TIMEOUT;
    let bind_addr = loop {
        let line = timeout(
            deadline.saturating_duration_since(Instant::now()),
            stderr_reader.next_line(),
        )
        .await
        .context("timed out waiting for websocket app-server to report bound websocket address")?
        .context("failed to read websocket app-server stderr")?
        .context("websocket app-server exited before reporting bound websocket address")?;
        eprintln!("[websocket app-server stderr] {line}");

        let stripped_line = {
            let mut stripped = String::with_capacity(line.len());
            let mut chars = line.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '\u{1b}' && matches!(chars.peek(), Some(&'[')) {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                    continue;
                }
                stripped.push(ch);
            }
            stripped
        };

        if let Some(bind_addr) = stripped_line
            .split_whitespace()
            .find_map(|token| token.strip_prefix("ws://"))
            .and_then(|addr| addr.parse::<SocketAddr>().ok())
        {
            break bind_addr;
        }
    };

    tokio::spawn(async move {
        while let Ok(Some(line)) = stderr_reader.next_line().await {
            eprintln!("[websocket app-server stderr] {line}");
        }
    });

    Ok((process, bind_addr))
}

pub async fn connect_websocket(bind_addr: SocketAddr) -> Result<WsClient> {
    connect_websocket_with_bearer(bind_addr, /*bearer_token*/ None).await
}

pub async fn connect_websocket_with_bearer(
    bind_addr: SocketAddr,
    bearer_token: Option<&str>,
) -> Result<WsClient> {
    let url = format!("ws://{}", connectable_bind_addr(bind_addr));
    let request = websocket_request(url.as_str(), bearer_token, /*origin*/ None)?;
    let deadline = Instant::now() + DEFAULT_READ_TIMEOUT;
    loop {
        match connect_async(request.clone()).await {
            Ok((stream, _response)) => return Ok(stream),
            Err(err) => {
                if Instant::now() >= deadline {
                    bail!("failed to connect websocket to {url}: {err}");
                }
                sleep(Duration::from_millis(50)).await;
            }
        }
    }
}
pub fn websocket_request(
    url: &str,
    bearer_token: Option<&str>,
    origin: Option<&str>,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>> {
    let mut request = url
        .into_client_request()
        .context("failed to create websocket request")?;
    if let Some(bearer_token) = bearer_token {
        request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {bearer_token}"))
                .context("invalid bearer token header")?,
        );
    }
    if let Some(origin) = origin {
        request.headers_mut().insert(
            ORIGIN,
            HeaderValue::from_str(origin).context("invalid origin header")?,
        );
    }
    Ok(request)
}

pub async fn send_initialize_request(
    stream: &mut WsClient,
    id: i64,
    client_name: &str,
) -> Result<()> {
    let params = InitializeParams {
        client_info: ClientInfo {
            name: client_name.to_string(),
            title: Some("WebSocket Test Client".to_string()),
            version: "0.1.0".to_string(),
        },
        capabilities: None,
    };
    send_request(
        stream,
        "initialize",
        id,
        Some(serde_json::to_value(params)?),
    )
    .await
}

pub async fn start_thread(stream: &mut WsClient, id: i64) -> Result<String> {
    send_request(
        stream,
        "thread/start",
        id,
        Some(serde_json::to_value(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })?),
    )
    .await?;
    let response = read_response_for_id(stream, id).await?;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(response)?;
    Ok(thread.id)
}
pub async fn send_request(
    stream: &mut WsClient,
    method: &str,
    id: i64,
    params: Option<serde_json::Value>,
) -> Result<()> {
    let message = JSONRPCMessage::Request(JSONRPCRequest {
        id: RequestId::Integer(id),
        method: method.to_string(),
        params,
        trace: None,
    });
    send_jsonrpc(stream, message).await
}

pub async fn send_jsonrpc(stream: &mut WsClient, message: JSONRPCMessage) -> Result<()> {
    let payload = serde_json::to_string(&message)?;
    stream
        .send(WebSocketMessage::Text(payload.into()))
        .await
        .context("failed to send websocket frame")
}

pub async fn read_response_for_id(stream: &mut WsClient, id: i64) -> Result<JSONRPCResponse> {
    let target_id = RequestId::Integer(id);
    loop {
        let message = read_jsonrpc_message(stream).await?;
        if let JSONRPCMessage::Response(response) = message
            && response.id == target_id
        {
            return Ok(response);
        }
    }
}

pub async fn read_notification_for_method(
    stream: &mut WsClient,
    method: &str,
) -> Result<JSONRPCNotification> {
    loop {
        let message = read_jsonrpc_message(stream).await?;
        if let JSONRPCMessage::Notification(notification) = message
            && notification.method == method
        {
            return Ok(notification);
        }
    }
}

pub async fn read_response_and_notification_for_method(
    stream: &mut WsClient,
    id: i64,
    method: &str,
) -> Result<(JSONRPCResponse, JSONRPCNotification)> {
    let target_id = RequestId::Integer(id);
    let mut response = None;
    let mut notification = None;

    while response.is_none() || notification.is_none() {
        let message = read_jsonrpc_message(stream).await?;
        match message {
            JSONRPCMessage::Response(candidate) if candidate.id == target_id => {
                response = Some(candidate);
            }
            JSONRPCMessage::Notification(candidate)
                if candidate.method == method && notification.is_some() =>
            {
                bail!(
                    "received duplicate notification for method `{method}` before completing paired read"
                );
            }
            JSONRPCMessage::Notification(candidate) if candidate.method == method => {
                notification = Some(candidate);
            }
            _ => {}
        }
    }

    let Some(response) = response else {
        bail!("response must be set before returning");
    };
    let Some(notification) = notification else {
        bail!("notification must be set before returning");
    };

    Ok((response, notification))
}

pub async fn read_error_for_id(stream: &mut WsClient, id: i64) -> Result<JSONRPCError> {
    let target_id = RequestId::Integer(id);
    loop {
        let message = read_jsonrpc_message(stream).await?;
        if let JSONRPCMessage::Error(err) = message
            && err.id == target_id
        {
            return Ok(err);
        }
    }
}

pub async fn read_jsonrpc_message(stream: &mut WsClient) -> Result<JSONRPCMessage> {
    loop {
        let frame = timeout(DEFAULT_READ_TIMEOUT, stream.next())
            .await
            .context("timed out waiting for websocket frame")?
            .context("websocket stream ended unexpectedly")?
            .context("failed to read websocket frame")?;

        match frame {
            WebSocketMessage::Text(text) => return Ok(serde_json::from_str(text.as_ref())?),
            WebSocketMessage::Ping(payload) => {
                stream.send(WebSocketMessage::Pong(payload)).await?;
            }
            WebSocketMessage::Pong(_) => {}
            WebSocketMessage::Close(frame) => {
                bail!("websocket closed unexpectedly: {frame:?}")
            }
            WebSocketMessage::Binary(_) => bail!("unexpected binary websocket frame"),
            WebSocketMessage::Frame(_) => {}
        }
    }
}

pub async fn assert_no_message(stream: &mut WsClient, wait_for: Duration) -> Result<()> {
    match timeout(wait_for, stream.next()).await {
        Ok(Some(Ok(frame))) => bail!("unexpected frame while waiting for silence: {frame:?}"),
        Ok(Some(Err(err))) => bail!("unexpected websocket read error: {err}"),
        Ok(None) => bail!("websocket closed unexpectedly while waiting for silence"),
        Err(_) => Ok(()),
    }
}

pub fn create_config_toml(
    codex_home: &Path,
    server_uri: &str,
    approval_policy: &str,
) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "{approval_policy}"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}

pub fn connectable_bind_addr(bind_addr: SocketAddr) -> SocketAddr {
    match bind_addr {
        SocketAddr::V4(addr) if addr.ip().is_unspecified() => {
            SocketAddr::from(([127, 0, 0, 1], addr.port()))
        }
        SocketAddr::V6(addr) if addr.ip().is_unspecified() => {
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], addr.port()))
        }
        _ => bind_addr,
    }
}
