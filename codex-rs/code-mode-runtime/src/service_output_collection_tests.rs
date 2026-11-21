//! Regression coverage for termination while the output queue is busy.

use std::sync::Arc;
use std::time::Duration;

use codex_code_mode_protocol::CodeModeOutputTruncation;
use codex_code_mode_protocol::CodeModeSessionDelegate;
use codex_code_mode_protocol::CodeModeSessionOptions;
use codex_code_mode_protocol::ExecuteRequest;
use codex_code_mode_protocol::NoopCodeModeSessionDelegate;
use codex_code_mode_protocol::RuntimeResponse;
use codex_code_mode_protocol::WaitOutcome;

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
    let response = tokio::time::timeout(Duration::from_secs(5), session.terminate(cell_id.into()))
        .await
        .expect("termination must unblock output")
        .unwrap();
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
    closed: tokio::sync::Notify,
}

impl CodeModeSessionDelegate for Gate {
    fn invoke_tool<'a>(
        &'a self,
        call: codex_code_mode_protocol::CodeModeNestedToolCall,
        cancel: tokio_util::sync::CancellationToken,
    ) -> codex_code_mode_protocol::ToolInvocationFuture<'a> {
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
    tokio::time::timeout(Duration::from_secs(5), gate.reached.notified())
        .await
        .unwrap();
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
async fn unread_session_limit_evicts_the_oldest_cell() {
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
    for cell in &cells[1..] {
        let response = session.wait(wait_request(cell, 4), None).await.unwrap();
        assert!(matches!(
            response,
            WaitOutcome::LiveCell(RuntimeResponse::Result { .. })
        ));
    }
}

#[tokio::test]
async fn draining_a_yield_frees_space_for_later_output() {
    const RUN_BYTES: usize = 10 * 1024 * 1024;
    let session = server_session();
    let started = session.execute(
        execute_request(&format!(r#"text("a".repeat({RUN_BYTES})); yield_control(); text("b".repeat({RUN_BYTES}));"#), 1024 * 1024),
        Arc::new(NoopCodeModeSessionDelegate),
        None,
    ).await.unwrap();
    let cell_id = started.cell_id.clone();
    let mut response = started.initial_response().await.unwrap();
    let mut retained = [0; 2];
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
    tokio::time::timeout(Duration::from_secs(5), gate.reached.notified())
        .await
        .unwrap();
    drop(started);
    tokio::time::timeout(Duration::from_secs(5), gate.closed.notified())
        .await
        .unwrap();
    assert!(matches!(
        session.wait(wait_request(&cell_id, 4), None).await.unwrap(),
        WaitOutcome::MissingCell(_)
    ));
}
