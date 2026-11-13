//! Registered ownership for agent operations; passive inspection uses native threads.
use super::LocalAgentRuntime;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::{AgentPath, ThreadId};

impl LocalAgentRuntime {
    pub(crate) fn registered_root(&self, actor: ThreadId) -> Option<ThreadId> {
        self.registry.agent_metadata_for_thread(actor)?;
        self.registry.agent_id_for_path(&AgentPath::root())
    }

    pub(crate) async fn read_thread_details(
        &self,
        target: ThreadId,
    ) -> CodexResult<codex_app_server_protocol::Thread> {
        self.upgrade()?.read_thread_view(target, true).await
    }
}
