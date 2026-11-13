//! Exact nonlocal IDs use the original resource host, never a local-error fallback.
use super::*;
use codex_app_server_protocol::{
    AgentForeignOperation, AgentForeignOrigin, AgentForeignRequest, AgentForeignResponse,
};
use codex_protocol::ThreadId;

pub(super) fn target(invocation: &ToolInvocation, value: &str) -> Option<ThreadId> {
    let runtime = &invocation.session.services.local_agent_runtime;
    runtime.register_session_root(
        invocation.session.thread_id,
        invocation.turn.parent_thread_id,
    );
    let id = ThreadId::from_string(value).ok()?;
    (id.to_string() == value && runtime.registered_root(id).is_none()).then_some(id)
}

pub(super) async fn route(
    invocation: &ToolInvocation,
    target: ThreadId,
    operation: AgentForeignOperation,
) -> Result<AgentForeignResponse, FunctionCallError> {
    let runtime = &invocation.session.services.local_agent_runtime;
    let actor = invocation.session.thread_id;
    let root = runtime.registered_root(actor).ok_or_else(|| {
        FunctionCallError::RespondToModel("calling agent has no native owner".into())
    })?;
    let path = runtime
        .ensure_agent_known(actor)
        .map_err(collab_spawn_error)?
        .agent_path
        .ok_or_else(|| {
            FunctionCallError::RespondToModel("calling agent has no native path".into())
        })?;
    let callback = runtime
        .host_resources
        .get()
        .and_then(|host| host.route_foreign.as_ref())
        .ok_or_else(|| {
            FunctionCallError::RespondToModel("foreign agent routing is unavailable".into())
        })?;
    let cancellation = invocation.cancellation_token.child_token();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    let request = AgentForeignRequest {
        source: AgentForeignOrigin {
            root_thread_id: root,
            actor_id: actor,
            agent_path: path,
            turn_id: invocation.turn.sub_id.clone(),
            call_id: invocation.call_id.clone(),
        },
        target_thread_id: target,
        operation,
    };
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(FunctionCallError::RespondToModel("foreign operation canceled".into())),
        result = callback(request, cancellation.clone()) => result.map_err(|error| collab_v2_agent_error(target, error)),
    }
}

pub(super) fn invalid_response() -> FunctionCallError {
    FunctionCallError::RespondToModel("foreign host returned a mismatched response".into())
}
