//! Resolves canonical task paths through the native agent registry.

use super::LocalAgentControl;
use super::LocalAgentRuntime;
use crate::agent::api::AgentTarget;
use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::protocol::SessionSource;

impl LocalAgentControl {
    pub(crate) fn resolve_target(
        &self,
        caller: ThreadId,
        target: &AgentTarget,
    ) -> CodexResult<ThreadId> {
        match target {
            AgentTarget::Id(thread_id) => Ok(*thread_id),
            AgentTarget::Reference(reference) => {
                self.runtime.ensure_agent_known(caller)?;
                self.runtime.resolve_path_reference(reference)
            }
        }
    }
}

impl LocalAgentRuntime {
    pub(crate) async fn resolve_agent_reference(
        &self,
        _current_thread_id: ThreadId,
        _current_session_source: &SessionSource,
        agent_reference: &str,
    ) -> CodexResult<ThreadId> {
        self.resolve_path_reference(agent_reference)
    }

    pub(super) fn resolve_path_reference(&self, agent_reference: &str) -> CodexResult<ThreadId> {
        let agent_path =
            AgentPath::try_from(agent_reference).map_err(CodexErr::UnsupportedOperation)?;
        if let Some(thread_id) = self.registry.agent_id_for_path(&agent_path) {
            return Ok(thread_id);
        }
        Err(CodexErr::UnsupportedOperation(format!(
            "live agent path `{}` not found",
            agent_path.as_str()
        )))
    }
}
