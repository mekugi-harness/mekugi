//! Capture storage is supplied by the authenticated host of the native tree.

use codex_protocol::agent_capture::*;
use codex_protocol::error::Result;
use futures::future::BoxFuture;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub type CaptureResourceCallback<Request, Response> =
    Arc<dyn Fn(Request, CancellationToken) -> BoxFuture<'static, Result<Response>> + Send + Sync>;

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
}
