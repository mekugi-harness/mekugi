use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use codex_code_mode::CellId;
use codex_code_mode::CodeModeNestedToolCall;
use codex_code_mode::CodeModeSessionDelegate;
use codex_code_mode::CodeModeSessionProvider;
use codex_code_mode::FunctionCallOutputContentItem;
use codex_code_mode::GrpcCodeModeSessionProvider;
use codex_code_mode::NotificationFuture;
use codex_code_mode::RuntimeResponse;
use codex_code_mode::ToolDefinition;
use codex_code_mode::ToolInvocationFuture;
use codex_code_mode::WaitOutcome;
use codex_code_mode::WaitRequest;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::HostHarness;
use super::TEST_TIMEOUT;
use super::execute;
use super::request;
use super::tool;

struct HeldTool {
    started: Semaphore,
    release: Semaphore,
    notified: Semaphore,
}

impl HeldTool {
    fn new() -> Self {
        Self {
            started: Semaphore::new(0),
            release: Semaphore::new(0),
            notified: Semaphore::new(0),
        }
    }
}

impl CodeModeSessionDelegate for HeldTool {
    fn invoke_tool<'a>(
        &'a self,
        _: CodeModeNestedToolCall,
        _: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async move {
            self.started.add_permits(1);
            self.release
                .acquire()
                .await
                .map_err(|error| error.to_string())?
                .forget();
            Ok(json!({}))
        })
    }

    fn notify<'a>(
        &'a self,
        _: String,
        _: CellId,
        text: String,
        _: CancellationToken,
    ) -> NotificationFuture<'a> {
        assert_eq!(text, "after pause");
        self.notified.add_permits(1);
        Box::pin(async { Ok(()) })
    }

    fn cell_closed(&self, _: &CellId) {}
}

#[tokio::test]
async fn an_earlier_cell_can_close_without_waking_the_paused_observation() -> Result<()> {
    let host = HostHarness::start("grpc://127.0.0.1:0").await?;
    let session = GrpcCodeModeSessionProvider::new(host.endpoint)
        .create_session()
        .await
        .map_err(anyhow::Error::msg)?;
    let earlier = Arc::new(HeldTool::new());
    let mut background = request(
        r#"yield_control(); await tools.hold({}); store("earlier_done", "yes"); text("earlier result");"#,
    );
    background.enabled_tools = vec![tool("hold")];
    let background = session
        .execute(background, earlier.clone(), None)
        .await
        .map_err(anyhow::Error::msg)?;
    let background_id = background.cell_id.clone();
    assert!(matches!(
        background
            .initial_response()
            .await
            .map_err(anyhow::Error::msg)?,
        RuntimeResponse::Yielded { .. }
    ));

    let later = Arc::new(HeldTool::new());
    let mut foreground = request(r#"await tools.hold({}); text("original foreground cell");"#);
    foreground.enabled_tools = vec![ToolDefinition {
        pauses_cell_deadline: true,
        ..tool("hold")
    }];
    foreground.yield_time_ms = Some(500);
    let interrupted = CancellationToken::new();
    let foreground = session
        .execute(foreground, later.clone(), Some(interrupted.clone()))
        .await
        .map_err(anyhow::Error::msg)?;
    let foreground_id = foreground.cell_id.clone();
    let mut outcome = std::pin::pin!(foreground.initial_response());

    timeout(TEST_TIMEOUT, earlier.started.acquire())
        .await
        .context("earlier callback did not start")??
        .forget();
    timeout(TEST_TIMEOUT, later.started.acquire())
        .await
        .context("pausing callback did not start")??
        .forget();
    earlier.release.add_permits(1);
    // Store writes become visible to other cells only when the earlier
    // script completes. Observe completion without consuming its result.
    timeout(TEST_TIMEOUT, async {
        loop {
            let probe = execute(
                &session,
                request(r#"text(load("earlier_done") === "yes" ? "committed" : "pending");"#),
                Arc::new(codex_code_mode::NoopCodeModeSessionDelegate),
            ).await?;
            if matches!(probe, RuntimeResponse::Result { content_items, .. }
                if content_items == vec![FunctionCallOutputContentItem::InputText { text: "committed".into() }])
            {
                return Ok::<(), anyhow::Error>(());
            }
        }
    }).await.context("earlier cell did not commit")??;
    assert!(timeout(Duration::from_secs(1), &mut outcome).await.is_err());

    // Preempt the observation, not the other cell or the gRPC session.
    interrupted.cancel();
    assert!(matches!(
        timeout(TEST_TIMEOUT, outcome)
            .await?
            .map_err(anyhow::Error::msg)?,
        RuntimeResponse::Yielded { .. }
    ));
    later.release.add_permits(1);
    for (id, text) in [
        (foreground_id, "original foreground cell"),
        (background_id, "earlier result"),
    ] {
        let outcome = session
            .wait(
                WaitRequest {
                    cell_id: id.clone(),
                    yield_time_ms: 1_000,
                    max_output_tokens: None,
                },
                None,
            )
            .await
            .map_err(anyhow::Error::msg)?;
        assert!(matches!(
            outcome,
            WaitOutcome::LiveCell(RuntimeResponse::Result { cell_id, content_items, error_text: None, .. })
                if cell_id == id && content_items == vec![FunctionCallOutputContentItem::InputText { text: text.to_string() }]
        ));
    }
    session.shutdown().await.map_err(anyhow::Error::msg)?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn an_unresponsive_host_still_expires_after_acknowledging_a_paused_observation() -> Result<()>
{
    let host = HostHarness::start("grpc://127.0.0.1:0").await?;
    let session = GrpcCodeModeSessionProvider::new(host.endpoint.clone())
        .create_session()
        .await
        .map_err(anyhow::Error::msg)?;
    let held = Arc::new(HeldTool::new());
    let mut execute = request(r#"const held = tools.hold({}); notify("after pause"); await held;"#);
    execute.enabled_tools = vec![ToolDefinition {
        pauses_cell_deadline: true,
        ..tool("hold")
    }];
    // If the client never receives the pause, the ordinary deadline is 181s,
    // so this test cannot pass on the ordinary timeout.
    execute.yield_time_ms = Some(120_000);
    let cell = session
        .execute(execute, held.clone(), None)
        .await
        .map_err(anyhow::Error::msg)?;
    let outcome = cell.initial_response();
    timeout(TEST_TIMEOUT, held.started.acquire())
        .await??
        .forget();
    // Both the first pulse and the later notification share the lease stream.
    timeout(TEST_TIMEOUT, held.notified.acquire())
        .await??
        .forget();

    // Keep the socket open but stall the isolated host. The test harness
    // kills its own child on drop, including when the assertion fails.
    let pid = host
        ._child
        .id()
        .context("running test host PID")?
        .to_string();
    let stopped = tokio::process::Command::new("kill")
        .args(["-STOP", &pid])
        .status()
        .await?;
    assert!(stopped.success());
    let error = timeout(Duration::from_secs(61), outcome)
        .await?
        .expect_err("stalled host must not finish the observation");
    assert_eq!(
        error,
        "gRPC code-mode host timed out waiting for execution outcome response"
    );
    Ok(())
}
