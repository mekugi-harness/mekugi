//! Only exact nonlocal thread IDs are candidates for cross-tree operations.
use super::*;
use crate::thread_manager::observation::ForeignAgent;
use codex_protocol::ThreadId;

pub(super) async fn target(
    invocation: &ToolInvocation,
    value: &str,
) -> Result<Option<ForeignAgent>, FunctionCallError> {
    let runtime = &invocation.session.services.local_agent_runtime;
    let caller = invocation.session.thread_id;
    runtime.register_session_root(caller, invocation.turn.parent_thread_id);
    let Ok(id) = ThreadId::from_string(value) else {
        return Ok(None);
    };
    if id.to_string() != value || runtime.registered_root(id).is_some() {
        return Ok(None);
    }
    runtime
        .foreign_agent(caller, id, invocation.cancellation_token.clone())
        .await
        .map(Some)
        .map_err(|error| collab_v2_agent_error(id, error))
}
