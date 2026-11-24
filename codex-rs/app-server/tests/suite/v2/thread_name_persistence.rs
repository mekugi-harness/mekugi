//! Naming persists an empty paginated thread for resume before and after a restart.

use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_repeating_assistant;
use codex_app_server_protocol::ThreadHistoryMode;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadSetNameParams;
use codex_app_server_protocol::ThreadSetNameResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::Duration;
use tokio::time::timeout;

const READ_TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::test]
async fn named_empty_paginated_thread_resumes_after_restart() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let codex_home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri()).write(codex_home.path())?;
    let mut app_server = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;
    let start_id = app_server
        .send_thread_start_request_with_auto_env(ThreadStartParams {
            history_mode: Some(ThreadHistoryMode::Paginated),
            ..Default::default()
        })
        .await?;
    let ThreadStartResponse { thread, .. } =
        timeout(READ_TIMEOUT, app_server.read_response(start_id)).await??;
    let thread_id = thread.id;
    let name = "Scheduled task";
    let set_name_id = app_server
        .send_thread_set_name_request(ThreadSetNameParams {
            thread_id: thread_id.clone(),
            name: name.to_string(),
        })
        .await?;
    let _: ThreadSetNameResponse =
        timeout(READ_TIMEOUT, app_server.read_response(set_name_id)).await??;

    // Run-now resumes immediately after naming, before any user turn exists.
    let resume_id = app_server
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread_id.clone(),
            exclude_turns: true,
            ..Default::default()
        })
        .await?;
    let ThreadResumeResponse { thread, .. } =
        timeout(READ_TIMEOUT, app_server.read_response(resume_id)).await??;
    assert_eq!(thread.id, thread_id);
    assert_eq!(thread.name.as_deref(), Some(name));
    assert!(
        timeout(READ_TIMEOUT, app_server.shutdown_gracefully())
            .await??
            .success()
    );
    drop(app_server);

    let mut app_server = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;
    let resume_id = app_server
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread_id.clone(),
            exclude_turns: true,
            ..Default::default()
        })
        .await?;
    let ThreadResumeResponse { thread, .. } =
        timeout(READ_TIMEOUT, app_server.read_response(resume_id)).await??;
    assert_eq!(thread.id, thread_id);
    assert_eq!(thread.name.as_deref(), Some(name));

    let input = vec![UserInput::Text {
        text: "Scheduled run".to_string(),
        text_elements: Vec::new(),
    }];
    let completed = timeout(
        READ_TIMEOUT,
        app_server.start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread_id.clone(),
            input,
            ..Default::default()
        }),
    )
    .await??;
    assert_eq!(completed.thread_id, thread_id);
    assert_eq!(completed.turn.status, TurnStatus::Completed);
    assert_eq!(completed.turn.error, None);
    Ok(())
}

#[tokio::test]
async fn automatic_title_uses_the_selected_provider_without_blocking_the_turn() -> Result<()> {
    use core_test_support::responses;
    use core_test_support::streaming_sse::{StreamingSseChunk, start_streaming_sse_server};

    let (release, waiting) = tokio::sync::oneshot::channel();
    let response = |id: &str, text: &str, tokens, gate| {
        vec![StreamingSseChunk {
            gate,
            body: responses::sse(vec![
                responses::ev_response_created(id),
                responses::ev_assistant_message(id, text),
                responses::ev_completed_with_tokens(id, tokens),
            ]),
        }]
    };
    let (server, _) = start_streaming_sse_server(vec![
        response("answer", "Done", 29, None),
        response("title", "Repair Index", 11, Some(waiting)),
    ])
    .await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(server.uri()).write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_env_overrides(&[("MEKUGI_AUTO_TITLE", Some("1"))])
        .build_initialized()
        .await?;
    let ThreadStartResponse { thread, .. } = app.start_thread(ThreadStartParams::default()).await?;
    let completed = timeout(
        READ_TIMEOUT,
        app.start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: "Repair the search index".into(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        }),
    )
    .await??;
    assert_eq!(completed.turn.status, TurnStatus::Completed);
    timeout(READ_TIMEOUT, server.wait_for_request_count(2)).await?;
    let requests = server.requests().await;
    let title: serde_json::Value = serde_json::from_slice(&requests[1])?;
    let answer: serde_json::Value = serde_json::from_slice(&requests[0])?;
    assert_eq!(title["model"], answer["model"]);
    assert_eq!(
        title["input"]
            .as_array()
            .and_then(|items| items.last())
            .expect("title prompt")["content"][0]["text"],
        "Repair the search index"
    );
    assert_eq!(title["tools"], serde_json::json!([]));
    release.send(()).expect("unblock title request");
    let notification = timeout(
        READ_TIMEOUT,
        app.read_stream_until_notification_message("thread/name/updated"),
    )
    .await??;
    let name: codex_app_server_protocol::ThreadNameUpdatedNotification =
        serde_json::from_value(notification.params.expect("notification params"))?;
    assert_eq!(name.thread_id, thread.id);
    assert_eq!(name.thread_name.as_deref(), Some("Repair Index"));
    assert!(
        timeout(READ_TIMEOUT, app.shutdown_gracefully())
            .await??
            .success()
    );
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let resume = app
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread.id.clone(),
            exclude_turns: true,
            ..Default::default()
        })
        .await?;
    let resumed: ThreadResumeResponse = timeout(READ_TIMEOUT, app.read_response(resume)).await??;
    assert_eq!(resumed.thread.context_tokens, Some(29));
    server.shutdown().await;
    Ok(())
}
