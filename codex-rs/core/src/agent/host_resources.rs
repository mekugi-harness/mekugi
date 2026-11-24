//! Capture storage is supplied by the authenticated host of the native tree.

use codex_protocol::agent_capture::*;
use codex_protocol::error::Result;
use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub type CaptureResourceCallback<Request, Response> =
    Arc<dyn Fn(Request, CancellationToken) -> BoxFuture<'static, Result<Response>> + Send + Sync>;

/// Strict fail-open runtime timeout for pre-execution capture observation barriers.
pub const BEFORE_TOOL_BARRIER_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct AgentHostResources {
    pub route_foreign: Option<
        CaptureResourceCallback<
            codex_app_server_protocol::AgentForeignRequest,
            codex_app_server_protocol::AgentForeignResponse,
        >,
    >,
    pub prepare_child:
        Option<CaptureResourceCallback<ChildHostAdmissionRequest, ChildHostAdmissionResponse>>,
    pub before_tool: Option<
        CaptureResourceCallback<
            codex_app_server_protocol::ExecutionBeforeToolParams,
            codex_app_server_protocol::ExecutionBeforeToolResponse,
        >,
    >,
    /// Synchronous launch-local observation; never an RPC or an operation result.
    pub record_feature_use:
        Option<Arc<dyn Fn(codex_protocol::feature_use::FeatureUseFact) + Send + Sync>>,
    pub freeze_progress: CaptureResourceCallback<ProgressFreezeRequest, ProgressFreezeResponse>,
    pub prepare_progress_output:
        CaptureResourceCallback<ProgressOutputRequest, ProgressOutputResponse>,
    pub prepare_final_capture: CaptureResourceCallback<FinalCaptureRequest, FinalCaptureResponse>,
    pub project_final_capture:
        CaptureResourceCallback<FinalCaptureProjectRequest, FinalCaptureProjectResponse>,
}

impl super::control::LocalAgentRuntime {
    pub(crate) fn has_before_tool_observer(&self) -> bool {
        self.host_resources
            .get()
            .and_then(|host| host.before_tool.as_ref())
            .is_some()
    }

    /// Hold local `exec_command` (`Bash`) or `apply_patch` execution briefly
    /// for a registered capture observer. Fails open on timeout, disconnect,
    /// cancellation, or storage errors; incurs zero wait when unobserved.
    pub(crate) async fn observe_before_tool(
        &self,
        request: codex_app_server_protocol::ExecutionBeforeToolParams,
        cancellation: CancellationToken,
    ) {
        let Some(observe) = self
            .host_resources
            .get()
            .and_then(|host| host.before_tool.as_ref())
        else {
            return;
        };
        let barrier_cancel = cancellation.child_token();
        let _cancel_on_drop = barrier_cancel.clone().drop_guard();
        let _ =
            tokio::time::timeout(BEFORE_TOOL_BARRIER_TIMEOUT, observe(request, barrier_cancel))
                .await;
    }

    /// Admit the reserved identity to host capture storage before any child input
    /// can acquire references. Non-hosted native runtimes need no callback.
    pub(crate) async fn prepare_child_host(
        &self,
        request: ChildHostAdmissionRequest,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let Some(prepare) = self
            .host_resources
            .get()
            .and_then(|host| host.prepare_child.as_ref())
        else {
            return Ok(());
        };
        let expected = request.thread_id;
        let _cancel_on_drop = cancellation.clone().drop_guard();
        let response = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(codex_protocol::error::CodexErr::TurnAborted),
            response = prepare(request, cancellation.clone()) => response?,
        };
        if response.thread_id != expected {
            return Err(codex_protocol::error::CodexErr::InvalidRequest(
                "host admitted a different child identity".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn admission_resources(
        prepare: CaptureResourceCallback<ChildHostAdmissionRequest, ChildHostAdmissionResponse>,
    ) -> AgentHostResources {
        AgentHostResources {
            prepare_child: Some(prepare),
            before_tool: None,
            record_feature_use: None,
            route_foreign: None,
            freeze_progress: Arc::new(|_, _| Box::pin(async { panic!("not progress") })),
            prepare_progress_output: Arc::new(|_, _| Box::pin(async { panic!("not progress") })),
            prepare_final_capture: Arc::new(|_, _| Box::pin(async { panic!("not final capture") })),
            project_final_capture: Arc::new(|_, _| Box::pin(async { panic!("not final capture") })),
        }
    }

    #[tokio::test]
    async fn different_host_identity_is_rejected_before_child_startup() {
        let (session, _, _) = crate::session::tests::make_session_and_context_with_rx().await;
        let resources = admission_resources(Arc::new(|_, _| {
            Box::pin(async {
                Ok(ChildHostAdmissionResponse {
                    thread_id: codex_protocol::ThreadId::new(),
                })
            })
        }));
        let runtime = &session.services.local_agent_runtime;
        assert!(runtime.host_resources.set(resources).is_ok());
        let error = runtime
            .prepare_child_host(
                ChildHostAdmissionRequest {
                    root_thread_id: session.session_id().into(),
                    parent_thread_id: session.thread_id,
                    thread_id: codex_protocol::ThreadId::new(),
                    local_cwd: None,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("different child identity"));
    }

    #[tokio::test]
    async fn before_tool_barrier_is_zero_wait_when_unobserved() {
        let (session, _, _) = crate::session::tests::make_session_and_context_with_rx().await;
        let runtime = &session.services.local_agent_runtime;
        assert!(!runtime.has_before_tool_observer());
        let start = tokio::time::Instant::now();
        runtime
            .observe_before_tool(
                codex_app_server_protocol::ExecutionBeforeToolParams {
                    thread_id: session.thread_id.to_string(),
                    turn_id: "turn-1".into(),
                    item_id: "call-1".into(),
                    tool: "Bash".into(),
                    cwd: "/tmp".into(),
                    input: "echo hi".into(),
                    session_tmp: None,
                },
                CancellationToken::new(),
            )
            .await;
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn before_tool_barrier_proceeds_immediately_on_ack_and_fails_open_on_error() {
        let (session, _, _) = crate::session::tests::make_session_and_context_with_rx().await;
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut resources =
            admission_resources(Arc::new(|_, _| Box::pin(async { panic!("unused") })));
        let seen_cb = Arc::clone(&seen);
        resources.before_tool = Some(Arc::new(move |req, _| {
            let fail = req.item_id == "call-err";
            seen_cb.lock().unwrap().push(req);
            Box::pin(async move {
                if fail {
                    Err(codex_protocol::error::CodexErr::Fatal("observer error".into()))
                } else {
                    Ok(codex_app_server_protocol::ExecutionBeforeToolResponse {})
                }
            })
        }));
        let runtime = &session.services.local_agent_runtime;
        assert!(runtime.host_resources.set(resources).is_ok());
        assert!(runtime.has_before_tool_observer());
        for item_id in ["call-ok", "call-err"] {
            runtime
                .observe_before_tool(
                    codex_app_server_protocol::ExecutionBeforeToolParams {
                        thread_id: session.thread_id.to_string(),
                        turn_id: "turn-1".into(),
                        item_id: item_id.into(),
                        tool: "apply_patch".into(),
                        cwd: "/tmp".into(),
                        input: "*** Begin Patch".into(),
                        session_tmp: None,
                    },
                    CancellationToken::new(),
                )
                .await;
        }
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn before_tool_barrier_fails_open_at_timeout_and_cancels_observer() {
        let (session, _, _) = crate::session::tests::make_session_and_context_with_rx().await;
        let canceled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let canceled_cb = Arc::clone(&canceled);
        let mut resources =
            admission_resources(Arc::new(|_, _| Box::pin(async { panic!("unused") })));
        resources.before_tool = Some(Arc::new(move |_, token| {
            let canceled_cb = Arc::clone(&canceled_cb);
            Box::pin(async move {
                token.cancelled().await;
                canceled_cb.store(true, std::sync::atomic::Ordering::SeqCst);
                Err(codex_protocol::error::CodexErr::TurnAborted)
            })
        }));
        let runtime = &session.services.local_agent_runtime;
        assert!(runtime.host_resources.set(resources).is_ok());
        let start = tokio::time::Instant::now();
        runtime
            .observe_before_tool(
                codex_app_server_protocol::ExecutionBeforeToolParams {
                    thread_id: session.thread_id.to_string(),
                    turn_id: "turn-1".into(),
                    item_id: "call-slow".into(),
                    tool: "Bash".into(),
                    cwd: "/tmp".into(),
                    input: "sleep 1".into(),
                    session_tmp: None,
                },
                CancellationToken::new(),
            )
            .await;
        assert!(start.elapsed() >= BEFORE_TOOL_BARRIER_TIMEOUT);
        assert!(start.elapsed() < BEFORE_TOOL_BARRIER_TIMEOUT + Duration::from_millis(500));
    }
}
