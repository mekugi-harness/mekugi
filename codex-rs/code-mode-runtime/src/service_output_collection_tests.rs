//! Regression coverage for termination while the output queue is busy.

use std::sync::Arc;

use codex_code_mode_protocol::CodeModeOutputTruncation;
use codex_code_mode_protocol::CodeModeSessionDelegate;
use codex_code_mode_protocol::CodeModeSessionOptions;
use codex_code_mode_protocol::CodeModeToolKind;
use codex_code_mode_protocol::ExecuteRequest;
use codex_code_mode_protocol::NoopCodeModeSessionDelegate;
use codex_code_mode_protocol::RuntimeResponse;
use codex_code_mode_protocol::ToolDefinition;
use codex_code_mode_protocol::WaitOutcome;
use codex_protocol::ToolName;
use serde_json::json;

use super::InProcessCodeModeSession;

fn server_session() -> InProcessCodeModeSession {
    InProcessCodeModeSession::with_options(
        CodeModeSessionOptions {
            output_truncation: CodeModeOutputTruncation::Server,
            ..CodeModeSessionOptions::default()
        },
        /*task_failure_handler*/ None,
    )
}

async fn start(session: &InProcessCodeModeSession, source: &str) -> RuntimeResponse {
    session
        .execute(
            ExecuteRequest {
                tool_call_id: "bounded-output".to_owned(),
                enabled_tools: Vec::new(),
                source: source.to_owned(),
                yield_time_ms: Some(60_000),
                max_output_tokens: Some(100),
            },
            Arc::new(NoopCodeModeSessionDelegate),
            /*preempt*/ None,
        )
        .await
        .unwrap()
        .initial_response()
        .await
        .unwrap()
}

#[tokio::test]
async fn termination_releases_a_flooding_bounded_runtime() {
    let session = server_session();
    let emitted = 40_000;
    let response = start(
        &session,
        &format!(
            "for (let i = 0; ; i++) {{
                text('output');
                if (i === {emitted}) yield_control();
            }}"
        ),
    )
    .await;
    let RuntimeResponse::Yielded { cell_id, .. } = response else {
        panic!("expected yielded cell");
    };
    let response = session.terminate(cell_id.into()).await.unwrap();
    assert!(matches!(
        response,
        WaitOutcome::LiveCell(RuntimeResponse::Terminated { .. })
    ));
    session.shutdown().await.unwrap();
}

type Item = codex_code_mode_protocol::FunctionCallOutputContentItem;

#[derive(Default)]
struct Gate {
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
    closed: tokio::sync::Notify,
}

impl CodeModeSessionDelegate for Gate {
    fn invoke_tool<'a>(
        &'a self,
        call: codex_code_mode_protocol::CodeModeNestedToolCall,
        cancel: tokio_util::sync::CancellationToken,
    ) -> codex_code_mode_protocol::ToolInvocationFuture<'a> {
        if call.tool_name.name == "release" {
            return Box::pin(async move {
                self.reached.notify_one();
                tokio::select! {
                    _ = self.release.notified() => Ok(json!(null)),
                    _ = cancel.cancelled() => Err("cancelled".to_string()),
                }
            });
        }
        NoopCodeModeSessionDelegate.invoke_tool(call, cancel)
    }

    fn notify(
        &self,
        _call_id: String,
        _cell_id: codex_code_mode_protocol::CellId,
        _text: String,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> codex_code_mode_protocol::NotificationFuture<'_> {
        self.reached.notify_one();
        Box::pin(async { Ok(()) })
    }

    fn cell_closed(&self, _cell_id: &codex_code_mode_protocol::CellId) {
        self.closed.notify_one();
    }
}

fn release_tool() -> ToolDefinition {
    ToolDefinition {
        name: "release".to_string(),
        tool_name: ToolName::plain("release"),
        description: String::new(),
        kind: CodeModeToolKind::Function,
        input_schema: None,
        pauses_cell_deadline: false,
        input_schema_max_bytes: None,
        output_schema: None,
    }
}

fn execute_request(source: &str, max_output_tokens: usize) -> ExecuteRequest {
    ExecuteRequest {
        tool_call_id: "bounded-output".to_string(),
        enabled_tools: Vec::new(),
        source: source.to_string(),
        yield_time_ms: Some(60_000),
        max_output_tokens: Some(max_output_tokens),
    }
}

fn wait_request(
    cell_id: &codex_code_mode_protocol::CellId,
    max_output_tokens: usize,
) -> codex_code_mode_protocol::WaitRequest {
    codex_code_mode_protocol::WaitRequest {
        cell_id: cell_id.clone(),
        yield_time_ms: 60_000,
        max_output_tokens: Some(max_output_tokens),
    }
}

fn append_body(outcome: WaitOutcome, body: &mut String, byte_limit: usize) -> bool {
    let WaitOutcome::LiveCell(response) = outcome else {
        return false;
    };
    let before = body.len();
    for item in response.content_items() {
        if let Item::InputText { text } = item {
            body.push_str(text);
        }
    }
    assert!(body.len() - before <= byte_limit);
    true
}

#[tokio::test]
async fn terminating_with_an_active_wait_delivers_each_byte_once_in_pages() {
    let session = Arc::new(server_session());
    let gate = Arc::new(Gate::default());
    let started = session.execute(
        execute_request(r#"yield_control(); text("x\n".repeat(100)); notify("at gate"); await new Promise(() => {});"#, 4),
        gate.clone(),
        None,
    ).await.unwrap();
    let cell_id = started.cell_id.clone();
    assert!(matches!(
        started.initial_response().await.unwrap(),
        RuntimeResponse::Yielded { .. }
    ));
    let wait = tokio::spawn({
        let session = session.clone();
        let request = wait_request(&cell_id, 4);
        async move { session.wait(request, None).await.unwrap() }
    });
    gate.reached.notified().await;
    let mut body = String::new();
    let terminated = session
        .terminate(codex_code_mode_protocol::TerminateRequest {
            max_output_tokens: Some(4),
            ..cell_id.clone().into()
        })
        .await
        .unwrap();
    assert!(append_body(terminated, &mut body, 16));
    append_body(wait.await.unwrap(), &mut body, 16);
    for _ in 0..14 {
        if !append_body(
            session.wait(wait_request(&cell_id, 4), None).await.unwrap(),
            &mut body,
            16,
        ) {
            assert_eq!(body, "x\n".repeat(100));
            return;
        }
    }
    panic!("terminated pages did not release the cell");
}

#[tokio::test]
async fn interrupt_terminates_active_cell_without_returning_or_retaining_pages() {
    let session = server_session();
    let response = start(
        &session,
        r#"text("x".repeat(1500)); yield_control(); await new Promise(() => {});"#,
    )
    .await;
    let RuntimeResponse::Yielded { cell_id, .. } = response else {
        panic!("expected yield")
    };
    assert_eq!(
        session
            .terminate(codex_code_mode_protocol::TerminateRequest {
                discard_output: true,
                ..cell_id.clone().into()
            })
            .await
            .unwrap(),
        WaitOutcome::LiveCell(RuntimeResponse::Terminated {
            cell_id: cell_id.clone(),
            content_items: Vec::new(),
            code_mode_host_duration: None,
        }),
    );
    assert!(matches!(
        session.wait(wait_request(&cell_id, 4), None).await.unwrap(),
        WaitOutcome::MissingCell(_)
    ));
}

#[tokio::test]
async fn unread_session_limit_evicts_the_oldest_finished_cell_and_releases_pages() {
    let session = server_session();
    let mut cells = Vec::new();
    for _ in 0..5 {
        let response = start(&session, r#"text("x".repeat(14 * 1024 * 1024));"#).await;
        cells.push(response.more_output_cell_id().unwrap().clone());
    }
    let missing = session
        .wait(wait_request(&cells[0], 4), None)
        .await
        .unwrap();
    let WaitOutcome::MissingCell(response) = missing else {
        panic!("oldest cell still retained")
    };
    assert!(matches!(response.content_items(),
        [Item::InputText { text }] if text == &format!("Script error:\nexec cell {} not found", cells[0])
    ));
    assert!(
        !session.pages.lock().unwrap().contains_key(&cells[0]),
        "wait on an evicted cell should release its pages entry immediately"
    );
    for cell in &cells[1..] {
        let response = session.wait(wait_request(cell, 4), None).await.unwrap();
        assert!(matches!(
            response,
            WaitOutcome::LiveCell(RuntimeResponse::Result { .. })
        ));
    }
}

#[tokio::test]
async fn session_eviction_keeps_running_cell_alive_and_reports_discarded_bytes() {
    const CHUNK_BYTES: usize = 14 * 1024 * 1024;
    let session = server_session();
    let gate = Arc::new(Gate::default());
    let mut request = execute_request(
        &format!(
            r#"text("a".repeat({CHUNK_BYTES})); yield_control(); await tools.release({{}}); text("after-eviction\n");"#
        ),
        4,
    );
    request.enabled_tools = vec![release_tool()];
    let started = session.execute(request, gate.clone(), None).await.unwrap();
    let running_cell = started.cell_id.clone();
    let initial = started.initial_response().await.unwrap();
    assert!(matches!(initial, RuntimeResponse::Yielded { .. }));
    let delivered_bytes: usize = initial
        .content_items()
        .iter()
        .map(|item| match item {
            Item::InputText { text } => text.len(),
            _ => 0,
        })
        .sum();
    gate.reached.notified().await;

    // Push other cells over the 64 MiB session cap to evict the running cell's unread bytes.
    for _ in 0..4 {
        let response = start(&session, &format!(r#"text("b".repeat({CHUNK_BYTES}));"#)).await;
        assert!(response.more_output_cell_id().is_some());
    }

    // Release the running cell; it must keep executing and complete normally.
    gate.release.notify_one();

    let mut texts = Vec::new();
    loop {
        let WaitOutcome::LiveCell(response) = session
            .wait(wait_request(&running_cell, 100), None)
            .await
            .unwrap()
        else {
            panic!("evicted running cell must not answer not-found");
        };
        for item in response.content_items() {
            if let Item::InputText { text } = item {
                texts.push(text.clone());
            }
        }
        if matches!(
            response,
            RuntimeResponse::Result {
                error_text: None,
                ..
            }
        ) && response.more_output_cell_id().is_none()
        {
            break;
        }
    }
    let dropped = CHUNK_BYTES - delivered_bytes;
    assert_eq!(
        texts,
        vec![
            format!("Code Mode output limit reached: discarded {dropped} bytes"),
            "after-eviction\n".to_string(),
        ]
    );
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn session_eviction_does_not_fail_in_flight_exec_with_not_found() {
    const CHUNK_BYTES: usize = 14 * 1024 * 1024;
    let session = Arc::new(server_session());
    let gate = Arc::new(Gate::default());
    let mut request = execute_request(
        &format!(
            r#"text("a".repeat({CHUNK_BYTES})); await tools.release({{}}); text("completed-after-evict\n");"#
        ),
        100,
    );
    request.enabled_tools = vec![release_tool()];
    let started = session.execute(request, gate.clone(), None).await.unwrap();
    let exec_task = tokio::spawn(started.initial_response());
    gate.reached.notified().await;

    for _ in 0..4 {
        let response = start(&session, &format!(r#"text("b".repeat({CHUNK_BYTES}));"#)).await;
        assert!(response.more_output_cell_id().is_some());
    }

    gate.release.notify_one();
    let response = exec_task.await.unwrap().unwrap();
    let RuntimeResponse::Result {
        error_text: None,
        content_items,
        ..
    } = response
    else {
        panic!("expected normal completion for evicted running exec, got {response:?}");
    };
    assert_eq!(
        content_items,
        vec![
            Item::InputText {
                text: format!("Code Mode output limit reached: discarded {CHUNK_BYTES} bytes"),
            },
            Item::InputText {
                text: "completed-after-evict\n".to_string(),
            },
        ]
    );
    session.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn draining_a_yield_frees_space_for_later_output() {
    const RUN_BYTES: usize = 10 * 1024 * 1024;
    let session = server_session();
    let gate = Arc::new(Gate::default());
    let mut request = execute_request(
        &format!(
            r#"text("a".repeat({RUN_BYTES})); yield_control(); await tools.release({{}}); text("b".repeat({RUN_BYTES}));"#
        ),
        1024 * 1024,
    );
    request.enabled_tools = vec![release_tool()];
    let started = session.execute(request, gate.clone(), None).await.unwrap();
    let cell_id = started.cell_id.clone();
    let mut response = started.initial_response().await.unwrap();
    gate.reached.notified().await;

    let mut retained = [0; 2];
    let mut released = false;
    loop {
        for item in response.content_items() {
            if let Item::InputText { text } = item {
                let byte = text.as_bytes()[0];
                assert!(
                    matches!(byte, b'a' | b'b') && text.bytes().all(|candidate| candidate == byte)
                );
                retained[usize::from(byte - b'a')] += text.len();
            }
        }
        if !released && retained[0] == RUN_BYTES {
            gate.release.notify_one();
            released = true;
        }
        if matches!(response, RuntimeResponse::Result { .. })
            && response.more_output_cell_id().is_none()
        {
            break;
        }
        response = match session
            .wait(wait_request(&cell_id, 1024 * 1024), None)
            .await
            .unwrap()
        {
            WaitOutcome::LiveCell(response) => response,
            WaitOutcome::MissingCell(_) => panic!("lost cell before completion"),
        };
    }
    assert_eq!(retained, [RUN_BYTES; 2]);
}

#[tokio::test]
async fn dropping_exec_before_its_first_response_releases_the_cell() {
    let session = server_session();
    let gate = Arc::new(Gate::default());
    let started = session
        .execute(
            execute_request(
                r#"text("unread".repeat(100)); notify("ready"); await new Promise(() => {});"#,
                4,
            ),
            gate.clone(),
            None,
        )
        .await
        .unwrap();
    let cell_id = started.cell_id.clone();
    gate.reached.notified().await;
    drop(started);
    gate.closed.notified().await;
    assert!(matches!(
        session.wait(wait_request(&cell_id, 4), None).await.unwrap(),
        WaitOutcome::MissingCell(_)
    ));
}

#[tokio::test]
async fn dropped_terminate_after_claiming_event_releases_pages_entry() {
    let session = Arc::new(server_session());
    let gate = Arc::new(Gate::default());
    let mut request = execute_request(r#"text("unread".repeat(100)); await tools.release({});"#, 4);
    request.enabled_tools = vec![release_tool()];
    let started = session.execute(request, gate.clone(), None).await.unwrap();
    let cell_id = started.cell_id.clone();
    let exec_task = tokio::spawn(started.initial_response());
    gate.reached.notified().await;

    // Poll terminate until it calls request_termination (cancelling the cell),
    // wait until the initial exec observer observes ClosedCell (meaning
    // finish_termination already delivered the Terminated event to terminate's
    // oneshot), then drop terminate without ever polling finish_wait.
    let mut terminate_fut = Box::pin(session.terminate(
        codex_code_mode_protocol::TerminateRequest {
            max_output_tokens: Some(4),
            discard_output: false,
            ..cell_id.clone().into()
        },
    ));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(std::future::Future::poll(terminate_fut.as_mut(), &mut cx).is_pending());

    let exec_err = exec_task
        .await
        .unwrap()
        .expect_err("concurrent terminate should close the initial observer");
    assert!(exec_err.contains("closed"), "unexpected error: {exec_err}");
    drop(terminate_fut);
    gate.closed.notified().await;

    assert!(
        !session.pages.lock().unwrap().contains_key(&cell_id),
        "stale pages entry remained after dropped terminate"
    );
    assert!(matches!(
        session.wait(wait_request(&cell_id, 4), None).await.unwrap(),
        WaitOutcome::MissingCell(_)
    ));
}
