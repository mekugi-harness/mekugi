use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use app_test_support::DEFAULT_APP_SERVER_TEST_ARGS;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::to_response;
use app_test_support::websocket::DEFAULT_READ_TIMEOUT;
use app_test_support::websocket::WsClient;
use app_test_support::websocket::assert_no_message;
use app_test_support::websocket::connect_websocket;
use app_test_support::websocket::connect_websocket_with_bearer;
use app_test_support::websocket::connectable_bind_addr;
use app_test_support::websocket::create_config_toml;
use app_test_support::websocket::read_error_for_id;
use app_test_support::websocket::read_jsonrpc_message;
use app_test_support::websocket::read_response_for_id;
use app_test_support::websocket::send_initialize_request;
use app_test_support::websocket::send_request;
use app_test_support::websocket::spawn_websocket_server;
use app_test_support::websocket::spawn_websocket_server_with_args;
use app_test_support::websocket::start_thread;
use app_test_support::websocket::websocket_request;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use codex_app_server_protocol::ConfigWarningNotification;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadLoadedListParams;
use codex_app_server_protocol::ThreadLoadedListResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_core::config::set_project_trust_level;
use codex_http_client::HttpClient;
use codex_http_client::HttpClientBuilder;
use codex_http_client::HttpResponse;
use codex_protocol::config_types::TrustLevel;
use core_test_support::responses::ev_completed;
use core_test_support::responses::sse;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::start_streaming_sse_server;
use hmac::Hmac;
use hmac::Mac;
use http::StatusCode;
use serde_json::json;
use sha2::Sha256;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use tempfile::TempDir;
use time::OffsetDateTime;
use tokio::process::Command;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio::time::sleep;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Error as WsError;

type HmacSha256 = Hmac<Sha256>;

#[tokio::test]
async fn websocket_transport_routes_per_connection_handshake_and_responses() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), "never")?;

    let (mut process, bind_addr) = spawn_websocket_server(codex_home.path()).await?;

    let mut ws1 = connect_websocket(bind_addr).await?;
    let mut ws2 = connect_websocket(bind_addr).await?;

    send_initialize_request(&mut ws1, /*id*/ 1, "ws_client_one").await?;
    let first_init = read_response_for_id(&mut ws1, /*id*/ 1).await?;
    assert_eq!(first_init.id, RequestId::Integer(1));

    // Initialize responses are request-scoped and must not leak to other
    // connections.
    assert_no_message(&mut ws2, Duration::from_millis(250)).await?;

    send_config_read_request(&mut ws2, /*id*/ 2).await?;
    let not_initialized = read_error_for_id(&mut ws2, /*id*/ 2).await?;
    assert_eq!(not_initialized.error.message, "Not initialized");

    send_initialize_request(&mut ws2, /*id*/ 3, "ws_client_two").await?;
    let second_init = read_response_for_id(&mut ws2, /*id*/ 3).await?;
    assert_eq!(second_init.id, RequestId::Integer(3));

    // Same request-id on different connections must route independently.
    send_config_read_request(&mut ws1, /*id*/ 77).await?;
    send_config_read_request(&mut ws2, /*id*/ 77).await?;
    let ws1_config = read_response_for_id(&mut ws1, /*id*/ 77).await?;
    let ws2_config = read_response_for_id(&mut ws2, /*id*/ 77).await?;

    assert_eq!(ws1_config.id, RequestId::Integer(77));
    assert_eq!(ws2_config.id, RequestId::Integer(77));
    assert!(ws1_config.result.get("config").is_some());
    assert!(ws2_config.result.get("config").is_some());

    process
        .kill()
        .await
        .context("failed to stop websocket app-server process")?;
    Ok(())
}

#[tokio::test]
async fn thread_start_routes_project_exec_policy_warning_to_requester() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), "never")?;

    let project = TempDir::new()?;
    std::fs::create_dir(project.path().join(".git"))?;
    let rules_dir = project.path().join(".codex/rules");
    std::fs::create_dir_all(&rules_dir)?;
    let rules_path = rules_dir.join("broken.rules");
    std::fs::write(&rules_path, "prefix_rule(")?;
    set_project_trust_level(codex_home.path(), project.path(), TrustLevel::Trusted)?;

    let (mut process, bind_addr) = spawn_websocket_server(codex_home.path()).await?;
    let mut requester = connect_websocket(bind_addr).await?;
    let mut other_client = connect_websocket(bind_addr).await?;

    send_initialize_request(&mut requester, /*id*/ 1, "requester").await?;
    read_response_for_id(&mut requester, /*id*/ 1).await?;
    send_initialize_request(&mut other_client, /*id*/ 2, "other_client").await?;
    read_response_for_id(&mut other_client, /*id*/ 2).await?;

    send_request(
        &mut requester,
        "thread/start",
        /*id*/ 3,
        Some(serde_json::to_value(ThreadStartParams {
            cwd: Some(project.path().display().to_string()),
            model: Some("mock-model".to_string()),
            ..Default::default()
        })?),
    )
    .await?;

    let target_id = RequestId::Integer(3);
    let warning_summary = "Error parsing rules; custom rules not applied.";
    let is_exec_policy_warning = |notification: &JSONRPCNotification| {
        notification.method == "configWarning"
            && notification
                .params
                .as_ref()
                .and_then(|params| params.get("summary"))
                .and_then(serde_json::Value::as_str)
                == Some(warning_summary)
    };
    let mut response = None;
    let mut warning = None;
    while response.is_none() || warning.is_none() {
        match read_jsonrpc_message(&mut requester).await? {
            JSONRPCMessage::Response(candidate) if candidate.id == target_id => {
                response = Some(candidate);
            }
            JSONRPCMessage::Notification(candidate) if is_exec_policy_warning(&candidate) => {
                warning = Some(candidate);
            }
            _ => {}
        }
    }

    let _: ThreadStartResponse = to_response(response.context("missing thread/start response")?)?;
    let warning: ConfigWarningNotification = serde_json::from_value(
        warning
            .context("missing exec-policy configWarning")?
            .params
            .context("configWarning should include params")?,
    )?;
    assert_eq!(
        warning
            .path
            .as_deref()
            .map(Path::new)
            .and_then(Path::file_name),
        Some(std::ffi::OsStr::new("broken.rules"))
    );

    match timeout(Duration::from_millis(250), async {
        loop {
            let message = read_jsonrpc_message(&mut other_client).await?;
            if let JSONRPCMessage::Notification(notification) = message
                && is_exec_policy_warning(&notification)
            {
                return Ok::<_, anyhow::Error>(notification);
            }
        }
    })
    .await
    {
        Ok(Ok(_)) => bail!("exec-policy configWarning leaked to another connection"),
        Ok(Err(err)) => return Err(err),
        Err(_) => {}
    }

    process
        .kill()
        .await
        .context("failed to stop websocket app-server process")?;
    Ok(())
}

#[tokio::test]
async fn websocket_transport_serves_health_endpoints_on_same_listener() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), "never")?;

    let (mut process, bind_addr) = spawn_websocket_server(codex_home.path()).await?;
    let client = HttpClientBuilder::new().build_direct()?;

    let readyz = http_get(&client, bind_addr, "/readyz").await?;
    assert_eq!(readyz.status(), StatusCode::OK);

    let healthz = http_get(&client, bind_addr, "/healthz").await?;
    assert_eq!(healthz.status(), StatusCode::OK);

    let mut ws = connect_websocket(bind_addr).await?;
    send_initialize_request(&mut ws, /*id*/ 1, "ws_health_client").await?;
    let init = read_response_for_id(&mut ws, /*id*/ 1).await?;
    assert_eq!(init.id, RequestId::Integer(1));

    process
        .kill()
        .await
        .context("failed to stop websocket app-server process")?;
    Ok(())
}

#[tokio::test]
async fn websocket_transport_rejects_browser_origin_without_auth() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), "never")?;

    let (mut process, bind_addr) = spawn_websocket_server(codex_home.path()).await?;

    let mut ws = connect_websocket(bind_addr).await?;
    send_initialize_request(&mut ws, /*id*/ 1, "ws_loopback_client").await?;
    let init = read_response_for_id(&mut ws, /*id*/ 1).await?;
    assert_eq!(init.id, RequestId::Integer(1));
    drop(ws);

    assert_websocket_connect_rejected_with_headers(
        bind_addr,
        /*bearer_token*/ None,
        Some("https://evil.example"),
        StatusCode::FORBIDDEN,
    )
    .await?;

    process
        .kill()
        .await
        .context("failed to stop websocket app-server process")?;
    Ok(())
}

#[tokio::test]
async fn websocket_transport_rejects_missing_and_invalid_capability_tokens() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    let token_file = codex_home.path().join("app-server-token");
    std::fs::write(&token_file, "super-secret-token\n")?;
    create_config_toml(codex_home.path(), &server.uri(), "never")?;
    let auth_args = vec![
        "--ws-auth".to_string(),
        "capability-token".to_string(),
        "--ws-token-file".to_string(),
        token_file.display().to_string(),
    ];

    let (mut process, bind_addr) =
        spawn_websocket_server_with_args(codex_home.path(), "ws://0.0.0.0:0", &auth_args).await?;

    assert_websocket_connect_rejected(bind_addr, /*bearer_token*/ None).await?;
    assert_websocket_connect_rejected(bind_addr, Some("wrong-token")).await?;

    let mut ws = connect_websocket_with_bearer(bind_addr, Some("super-secret-token")).await?;
    send_initialize_request(&mut ws, /*id*/ 1, "ws_auth_client").await?;
    let init = read_response_for_id(&mut ws, /*id*/ 1).await?;
    assert_eq!(init.id, RequestId::Integer(1));

    process
        .kill()
        .await
        .context("failed to stop websocket app-server process")?;
    Ok(())
}

#[tokio::test]
async fn websocket_transport_verifies_signed_short_lived_bearer_tokens() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    let shared_secret_file = codex_home.path().join("app-server-signing-secret");
    let shared_secret = "0123456789abcdef0123456789abcdef";
    std::fs::write(&shared_secret_file, format!("{shared_secret}\n"))?;
    create_config_toml(codex_home.path(), &server.uri(), "never")?;
    let auth_args = vec![
        "--ws-auth".to_string(),
        "signed-bearer-token".to_string(),
        "--ws-shared-secret-file".to_string(),
        shared_secret_file.display().to_string(),
        "--ws-issuer".to_string(),
        "codex-enroller".to_string(),
        "--ws-audience".to_string(),
        "codex-app-server".to_string(),
        "--ws-max-clock-skew-seconds".to_string(),
        "1".to_string(),
    ];

    let (mut process, bind_addr) =
        spawn_websocket_server_with_args(codex_home.path(), "ws://127.0.0.1:0", &auth_args).await?;
    let expired_token = signed_bearer_token(
        shared_secret.as_bytes(),
        json!({
            "exp": OffsetDateTime::now_utc().unix_timestamp() - 30,
            "iss": "codex-enroller",
            "aud": "codex-app-server",
        }),
    )?;
    assert_websocket_connect_rejected(bind_addr, Some(expired_token.as_str())).await?;

    let malformed_token = "not-a-jwt";
    assert_websocket_connect_rejected(bind_addr, Some(malformed_token)).await?;

    let not_yet_valid_token = signed_bearer_token(
        shared_secret.as_bytes(),
        json!({
            "exp": OffsetDateTime::now_utc().unix_timestamp() + 60,
            "nbf": OffsetDateTime::now_utc().unix_timestamp() + 30,
            "iss": "codex-enroller",
            "aud": "codex-app-server",
        }),
    )?;
    assert_websocket_connect_rejected(bind_addr, Some(not_yet_valid_token.as_str())).await?;

    let wrong_issuer_token = signed_bearer_token(
        shared_secret.as_bytes(),
        json!({
            "exp": OffsetDateTime::now_utc().unix_timestamp() + 60,
            "iss": "someone-else",
            "aud": "codex-app-server",
        }),
    )?;
    assert_websocket_connect_rejected(bind_addr, Some(wrong_issuer_token.as_str())).await?;

    let wrong_audience_token = signed_bearer_token(
        shared_secret.as_bytes(),
        json!({
            "exp": OffsetDateTime::now_utc().unix_timestamp() + 60,
            "iss": "codex-enroller",
            "aud": "wrong-audience",
        }),
    )?;
    assert_websocket_connect_rejected(bind_addr, Some(wrong_audience_token.as_str())).await?;

    let wrong_signature_token = signed_bearer_token(
        b"fedcba9876543210fedcba9876543210",
        json!({
            "exp": OffsetDateTime::now_utc().unix_timestamp() + 60,
            "iss": "codex-enroller",
            "aud": "codex-app-server",
        }),
    )?;
    assert_websocket_connect_rejected(bind_addr, Some(wrong_signature_token.as_str())).await?;

    let valid_token = signed_bearer_token(
        shared_secret.as_bytes(),
        json!({
            "exp": OffsetDateTime::now_utc().unix_timestamp() + 60,
            "iss": "codex-enroller",
            "aud": "codex-app-server",
        }),
    )?;
    let mut ws = connect_websocket_with_bearer(bind_addr, Some(valid_token.as_str())).await?;
    send_initialize_request(&mut ws, /*id*/ 1, "ws_signed_auth_client").await?;
    let init = read_response_for_id(&mut ws, /*id*/ 1).await?;
    assert_eq!(init.id, RequestId::Integer(1));

    process
        .kill()
        .await
        .context("failed to stop websocket app-server process")?;
    Ok(())
}

#[tokio::test]
async fn websocket_transport_rejects_short_signed_bearer_secret_configuration() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    let shared_secret_file = codex_home.path().join("app-server-signing-secret");
    std::fs::write(&shared_secret_file, "too-short\n")?;
    create_config_toml(codex_home.path(), &server.uri(), "never")?;

    let output = run_websocket_server_to_completion_with_args(
        codex_home.path(),
        "ws://127.0.0.1:0",
        &[
            "--ws-auth".to_string(),
            "signed-bearer-token".to_string(),
            "--ws-shared-secret-file".to_string(),
            shared_secret_file.display().to_string(),
        ],
    )
    .await?;
    assert!(
        !output.status.success(),
        "short shared secret should fail websocket server startup"
    );
    let stderr = String::from_utf8(output.stderr).context("stderr should be valid utf-8")?;
    assert!(
        stderr.contains("must be at least 32 bytes"),
        "unexpected stderr: {stderr}"
    );

    Ok(())
}

#[tokio::test]
async fn websocket_transport_rejects_unauthenticated_non_loopback_startup() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), "never")?;

    let output =
        run_websocket_server_to_completion_with_args(codex_home.path(), "ws://0.0.0.0:0", &[])
            .await?;
    assert!(
        !output.status.success(),
        "unauthenticated non-loopback listener should fail websocket server startup"
    );
    let stderr = String::from_utf8(output.stderr).context("stderr should be valid utf-8")?;
    assert!(
        stderr.contains("refusing to start non-loopback websocket listener"),
        "unexpected stderr: {stderr}"
    );

    Ok(())
}

#[tokio::test]
async fn websocket_disconnect_cancels_guardian_review_and_releases_thread() -> Result<()> {
    let (release, gate) = tokio::sync::oneshot::channel();
    let (guardian, _) = start_streaming_sse_server(vec![vec![StreamingSseChunk {
        gate: Some(gate),
        body: sse(vec![ev_completed("review")]),
    }]])
    .await;
    let home = TempDir::new()?;
    app_test_support::MockResponsesConfig::new(guardian.uri())
        .with_approval_policy("on-request")
        .with_root_config("approvals_reviewer = \"auto_review\"")
        .with_extra_config("[auto_review]\ntimeout_ms = 0")
        .write(home.path())?;
    let (mut process, addr) = spawn_websocket_server(home.path()).await?;
    let mut owner = connect_websocket(addr).await?;
    let mut observer = connect_websocket(addr).await?;
    for client in [&mut owner, &mut observer] {
        send_request(
            client,
            "initialize",
            /*id*/ 1,
            Some(json!({
                "clientInfo": {"name":"guardian-test","version":"1"},
                "capabilities": {"experimentalApi":true}
            })),
        )
        .await?;
        read_response_for_id(client, /*id*/ 1).await?;
    }
    let thread_id = start_thread(&mut owner, /*id*/ 2).await?;
    send_request(&mut owner, "thread/inject_items", /*id*/ 3, Some(json!({
        "threadId":thread_id, "items":[{
            "type":"function_call","name":"exec_command","call_id":"review",
            "arguments":r#"{"cmd":"touch marker","sandbox_permissions":"require_escalated","justification":"create the marker"}"#
        }]
    }))).await?;
    read_response_for_id(&mut owner, /*id*/ 3).await?;
    send_request(
        &mut owner,
        "thread/guardianReview",
        /*id*/ 4,
        Some(json!({"threadId":thread_id})),
    )
    .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        guardian.wait_for_request_count(/*count*/ 1),
    )
    .await?;
    owner.close(None).await?;
    drop(owner);

    send_request(&mut observer, "thread/inject_items", /*id*/ 5, Some(json!({
        "threadId":thread_id, "items":[{"type":"function_call_output","call_id":"review","output":"recorded"}]
    }))).await?;
    // Disconnect must release the thread before the 30-second connection drain timeout.
    let response = timeout(
        Duration::from_secs(10),
        read_response_for_id(&mut observer, /*id*/ 5),
    )
    .await??;
    assert_eq!(response.result, json!({}));
    drop(release);
    process.kill().await?;
    Ok(())
}

#[tokio::test]
async fn websocket_disconnect_keeps_last_subscribed_thread_loaded_until_idle_timeout() -> Result<()>
{
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri(), "never")?;

    let (mut process, bind_addr) = spawn_websocket_server_with_args(
        codex_home.path(),
        "ws://127.0.0.1:0",
        &["-c".to_string(), "thread_unload_delay_secs=2".to_string()],
    )
    .await?;

    let mut ws1 = connect_websocket(bind_addr).await?;
    send_initialize_request(&mut ws1, /*id*/ 1, "ws_thread_owner").await?;
    read_response_for_id(&mut ws1, /*id*/ 1).await?;

    let thread_id = start_thread(&mut ws1, /*id*/ 2).await?;
    assert_loaded_threads(&mut ws1, /*id*/ 3, &[thread_id.as_str()]).await?;

    let mut ws2 = connect_websocket(bind_addr).await?;
    send_initialize_request(&mut ws2, /*id*/ 4, "ws_reconnect_client").await?;
    read_response_for_id(&mut ws2, /*id*/ 4).await?;

    ws1.close(None).await.context("failed to close websocket")?;
    drop(ws1);

    wait_for_loaded_threads(&mut ws2, /*first_id*/ 5, &[thread_id.as_str()]).await?;
    wait_for_loaded_threads(&mut ws2, /*first_id*/ 100, &[]).await?;

    process
        .kill()
        .await
        .context("failed to stop websocket app-server process")?;
    Ok(())
}
async fn assert_websocket_connect_rejected(
    bind_addr: SocketAddr,
    bearer_token: Option<&str>,
) -> Result<()> {
    assert_websocket_connect_rejected_with_headers(
        bind_addr,
        bearer_token,
        /*origin*/ None,
        StatusCode::UNAUTHORIZED,
    )
    .await
}

async fn assert_websocket_connect_rejected_with_headers(
    bind_addr: SocketAddr,
    bearer_token: Option<&str>,
    origin: Option<&str>,
    expected_status: StatusCode,
) -> Result<()> {
    let url = format!("ws://{}", connectable_bind_addr(bind_addr));
    let request = websocket_request(url.as_str(), bearer_token, origin)?;

    match connect_async(request).await {
        Ok((_stream, response)) => {
            bail!(
                "expected websocket handshake rejection, got {}",
                response.status()
            )
        }
        Err(WsError::Http(response)) => {
            assert_eq!(response.status(), expected_status);
            Ok(())
        }
        Err(err) => bail!("expected http rejection during websocket handshake: {err}"),
    }
}

async fn run_websocket_server_to_completion_with_args(
    codex_home: &Path,
    listen_url: &str,
    extra_args: &[String],
) -> Result<std::process::Output> {
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
        .env("RUST_LOG", "warn");
    timeout(DEFAULT_READ_TIMEOUT, cmd.output())
        .await
        .context("timed out waiting for websocket app-server to exit")?
        .context("failed to run websocket app-server")
}

async fn http_get(client: &HttpClient, bind_addr: SocketAddr, path: &str) -> Result<HttpResponse> {
    let connectable_bind_addr = connectable_bind_addr(bind_addr);
    let deadline = Instant::now() + DEFAULT_READ_TIMEOUT;
    loop {
        match client
            .get(format!("http://{connectable_bind_addr}{path}"))
            .send()
            .await
            .with_context(|| format!("failed to GET http://{connectable_bind_addr}{path}"))
        {
            Ok(response) => return Ok(response),
            Err(err) => {
                if Instant::now() >= deadline {
                    bail!("failed to GET http://{connectable_bind_addr}{path}: {err}");
                }
                sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn assert_loaded_threads(stream: &mut WsClient, id: i64, expected: &[&str]) -> Result<()> {
    let response = request_loaded_threads(stream, id).await?;
    let mut actual = response.data;
    actual.sort();
    let mut expected = expected
        .iter()
        .map(|thread_id| (*thread_id).to_string())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(actual, expected);
    assert_eq!(response.next_cursor, None);
    Ok(())
}

async fn wait_for_loaded_threads(
    stream: &mut WsClient,
    first_id: i64,
    expected: &[&str],
) -> Result<()> {
    let mut next_id = first_id;
    let expected = expected
        .iter()
        .map(|thread_id| (*thread_id).to_string())
        .collect::<Vec<_>>();
    timeout(DEFAULT_READ_TIMEOUT, async {
        loop {
            let response = request_loaded_threads(stream, next_id).await?;
            next_id += 1;
            let mut actual = response.data;
            actual.sort();
            if actual == expected {
                return Ok::<(), anyhow::Error>(());
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("timed out waiting for loaded thread list")??;
    Ok(())
}

async fn request_loaded_threads(
    stream: &mut WsClient,
    id: i64,
) -> Result<ThreadLoadedListResponse> {
    send_request(
        stream,
        "thread/loaded/list",
        id,
        Some(serde_json::to_value(ThreadLoadedListParams::default())?),
    )
    .await?;
    let response = read_response_for_id(stream, id).await?;
    to_response::<ThreadLoadedListResponse>(response)
}

async fn send_config_read_request(stream: &mut WsClient, id: i64) -> Result<()> {
    send_request(
        stream,
        "config/read",
        id,
        Some(json!({ "includeLayers": false })),
    )
    .await
}

fn signed_bearer_token(shared_secret: &[u8], claims: serde_json::Value) -> Result<String> {
    let header_segment = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let claims_segment = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims)?);
    let payload = format!("{header_segment}.{claims_segment}");
    let mut mac = HmacSha256::new_from_slice(shared_secret).context("failed to create hmac")?;
    mac.update(payload.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!("{payload}.{signature}"))
}
